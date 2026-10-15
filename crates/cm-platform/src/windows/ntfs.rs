//! `win.ntfs_enum` (experimental, G0): whole-volume NTFS name enumeration via
//! FSCTL_ENUM_USN_DATA, then handle-based supplement of sizes, link counts and reparse tags.
//!
//! It reads the volume's entire namespace regardless of directory ACLs, so it is only valid for
//! an explicitly selected *volume root* with administrator rights. A directory scope is refused
//! before anything is read (SCN-01/02); callers fall back to `win.directory`. Output rows use the
//! same semantics as the directory baseline so the two can be compared on the same volume.

use std::collections::{HashMap, VecDeque};
use std::io;

use cm_core::capability::CapabilityState;
use cm_core::model::{BoundaryKind, CloudState, IdentityQuality, ObjectKind, ScanState};
use cm_core::path::RawPath;
use cm_core::scan::{EntryRow, ObjectRow, RootProbe, ScanBackend, ScanIssue, ScanOptions, ScanReport, ScanSink};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_TAG_INFO, FILE_BASIC_INFO, FileBasicInfo, FILE_ID_DESCRIPTOR, FILE_STANDARD_INFO, FileAttributeTagInfo, FileIdType,
    FileStandardInfo, FindClose, FindFirstFileNameW, FindNextFileNameW, GetFileInformationByHandleEx, GetVolumePathNameW,
    OpenFileById,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{FSCTL_ENUM_USN_DATA, MFT_ENUM_DATA_V0};

use super::handle::{FILE_READ_ATTRIBUTES, Handle, SYNCHRONIZE, compressed_size, full_path};

const GENERIC_READ: u32 = 0x8000_0000;
const SHARE_ALL: u32 = 0x7;
const OPEN_EXISTING: u32 = 3;
const FLAG_BACKUP: u32 = 0x0200_0000;
const FLAG_OPEN_REPARSE: u32 = 0x0020_0000;
const ATTR_DIRECTORY: u32 = 0x10;
const ATTR_REPARSE: u32 = 0x400;
const ATTR_SPARSE: u32 = 0x200;
const ATTR_COMPRESSED: u32 = 0x800;
const ATTR_PLACEHOLDER: u32 = 0x1000 | 0x0004_0000 | 0x0040_0000;
const TAG_SYMLINK: u32 = 0xA000_000C;
const ERROR_HANDLE_EOF: i32 = 38;
const ERROR_ACCESS_DENIED: i32 = 5;
const FRN_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: handle owned by this wrapper, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

struct Node {
    parent: u64,
    name: Vec<u16>,
    attributes: u32,
}

/// `Some("X:")` when `display` is exactly a volume root such as `X:\`.
fn volume_root(display: &[u16]) -> Option<Vec<u16>> {
    let mut z = display.to_vec();
    z.push(0);
    let mut out = vec![0u16; 1024];
    // SAFETY: NUL-terminated input; output bounded by the buffer length.
    if unsafe { GetVolumePathNameW(z.as_ptr(), out.as_mut_ptr(), out.len() as u32) } == 0 {
        return None;
    }
    let n = out.iter().position(|&c| c == 0)?;
    let mut given = display.to_vec();
    if given.last() != Some(&(b'\\' as u16)) {
        given.push(b'\\' as u16);
    }
    (out[..n] == given[..] && n == 3 && out[1] == b':' as u16).then(|| out[..2].to_vec())
}

fn open_volume(drive: &[u16]) -> io::Result<Owned> {
    let mut path: Vec<u16> = "\\\\.\\".encode_utf16().collect();
    path.extend_from_slice(drive);
    path.push(0);
    // SAFETY: NUL-terminated device path; read-only open.
    let h = unsafe { CreateFileW(path.as_ptr(), GENERIC_READ, SHARE_ALL, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut()) };
    if h == INVALID_HANDLE_VALUE { Err(io::Error::last_os_error()) } else { Ok(Owned(h)) }
}

/// Every file record name on the volume: FRN → (parent FRN, name, attributes).
fn enumerate(volume: &Owned) -> io::Result<HashMap<u64, Node>> {
    let mut nodes = HashMap::new();
    let mut input = MFT_ENUM_DATA_V0 { StartFileReferenceNumber: 0, LowUsn: 0, HighUsn: i64::MAX };
    let mut buf = vec![0u64; (1 << 20) / 8];
    loop {
        let mut returned = 0u32;
        // SAFETY: valid input struct and 8-byte-aligned output buffer of the declared size.
        let ok = unsafe {
            DeviceIoControl(volume.0, FSCTL_ENUM_USN_DATA, (&input as *const MFT_ENUM_DATA_V0).cast(), size_of::<MFT_ENUM_DATA_V0>() as u32,
                buf.as_mut_ptr().cast(), (buf.len() * 8) as u32, &mut returned, std::ptr::null_mut())
        };
        if ok == 0 {
            let e = io::Error::last_os_error();
            return if e.raw_os_error() == Some(ERROR_HANDLE_EOF) { Ok(nodes) } else { Err(e) };
        }
        let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), returned as usize) };
        if bytes.len() < 8 {
            return Ok(nodes);
        }
        input.StartFileReferenceNumber = u64::from_le_bytes(bytes[..8].try_into().unwrap());
        let mut off = 8usize;
        while off + 60 <= bytes.len() {
            let rec = &bytes[off..];
            let len = u32::from_le_bytes(rec[0..4].try_into().unwrap()) as usize;
            if len == 0 || off + len > bytes.len() {
                break;
            }
            // USN_RECORD_V2 layout: frn @8, parent @16, attributes @52, name len @56, name offset @58.
            let frn = u64::from_le_bytes(rec[8..16].try_into().unwrap());
            let parent = u64::from_le_bytes(rec[16..24].try_into().unwrap());
            let attributes = u32::from_le_bytes(rec[52..56].try_into().unwrap());
            let name_len = u16::from_le_bytes(rec[56..58].try_into().unwrap()) as usize;
            let name_off = u16::from_le_bytes(rec[58..60].try_into().unwrap()) as usize;
            let name: Vec<u16> = rec[name_off..name_off + name_len].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            nodes.insert(frn, Node { parent, name, attributes });
            off += len;
        }
    }
}

struct Supplement {
    modified_unix: Option<i64>,
    end_of_file: u64,
    allocation: u64,
    links: u32,
    reparse_tag: Option<u32>,
    compressed: Option<u64>,
}

fn open_by_id(volume: &Owned, frn: u64) -> io::Result<Owned> {
    let mut desc: FILE_ID_DESCRIPTOR = unsafe { std::mem::zeroed() };
    desc.dwSize = size_of::<FILE_ID_DESCRIPTOR>() as u32;
    desc.Type = FileIdType;
    desc.Anonymous.FileId = frn as i64;
    // SAFETY: descriptor filled for a 64-bit file ID; attribute-only access.
    let h = unsafe { OpenFileById(volume.0, &desc, FILE_READ_ATTRIBUTES | SYNCHRONIZE, SHARE_ALL, std::ptr::null(), FLAG_BACKUP | FLAG_OPEN_REPARSE) };
    if h == INVALID_HANDLE_VALUE { Err(io::Error::last_os_error()) } else { Ok(Owned(h)) }
}

fn supplement(volume: &Owned, frn: u64, attributes: u32) -> io::Result<Supplement> {
    let h = open_by_id(volume, frn)?;
    let mut std_info: FILE_STANDARD_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: buffer sized for FILE_STANDARD_INFO.
    if unsafe { GetFileInformationByHandleEx(h.0, FileStandardInfo, (&mut std_info as *mut FILE_STANDARD_INFO).cast(), size_of::<FILE_STANDARD_INFO>() as u32) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut basic: FILE_BASIC_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: buffer sized for FILE_BASIC_INFO.
    let modified_unix = (unsafe { GetFileInformationByHandleEx(h.0, FileBasicInfo, (&mut basic as *mut FILE_BASIC_INFO).cast(), size_of::<FILE_BASIC_INFO>() as u32) } != 0
        && basic.LastWriteTime > 0)
        .then(|| cm_core::time::filetime_to_unix(basic.LastWriteTime as u64));
    let mut tag = None;
    if attributes & ATTR_REPARSE != 0 {
        let mut t: FILE_ATTRIBUTE_TAG_INFO = unsafe { std::mem::zeroed() };
        // SAFETY: buffer sized for FILE_ATTRIBUTE_TAG_INFO.
        if unsafe { GetFileInformationByHandleEx(h.0, FileAttributeTagInfo, (&mut t as *mut FILE_ATTRIBUTE_TAG_INFO).cast(), size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32) } != 0 {
            tag = Some(t.ReparseTag);
        }
    }
    let compressed = if attributes & (ATTR_COMPRESSED | ATTR_SPARSE) != 0 {
        compressed_size(h.0).ok()
    } else {
        None
    };
    Ok(Supplement {
        modified_unix,
        end_of_file: std_info.EndOfFile as u64,
        allocation: std_info.AllocationSize as u64,
        links: std_info.NumberOfLinks,
        reparse_tag: tag,
        compressed,
    })
}

/// All hard-link names of a file as volume-relative paths (`\dir\name`).
fn link_names(full: &[u16]) -> Vec<Vec<u16>> {
    let mut z = full.to_vec();
    z.push(0);
    let mut out = Vec::new();
    let mut buf = vec![0u16; 32_768];
    let mut len = buf.len() as u32;
    // SAFETY: NUL-terminated path; buffer length passed in characters.
    let find = unsafe { FindFirstFileNameW(z.as_ptr(), 0, &mut len, buf.as_mut_ptr()) };
    if find == INVALID_HANDLE_VALUE {
        return out;
    }
    loop {
        let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
        out.push(buf[..n].to_vec());
        len = buf.len() as u32;
        // SAFETY: same find handle and buffer.
        if unsafe { FindNextFileNameW(find, &mut len, buf.as_mut_ptr()) } == 0 {
            break;
        }
    }
    // SAFETY: handle from FindFirstFileNameW.
    unsafe { FindClose(find) };
    out
}

fn join(parent: &[u16], name: &[u16]) -> Vec<u16> {
    let mut v = parent.to_vec();
    if v.last() != Some(&(b'\\' as u16)) {
        v.push(b'\\' as u16);
    }
    v.extend_from_slice(name);
    v
}

pub struct NtfsEnumBackend;

impl NtfsEnumBackend {
    fn display(root: &RawPath) -> Option<Vec<u16>> {
        match root {
            RawPath::Windows(u) => full_path(u).ok().map(|(d, _)| d),
            RawPath::Unix(_) => None,
        }
    }
}

impl ScanBackend for NtfsEnumBackend {
    fn name(&self) -> &'static str {
        "win.ntfs_enum"
    }

    fn probe(&self, root: &RawPath) -> RootProbe {
        let probe = |state, reason: &str| RootProbe { filesystem: None, volume_id: None, backend: "win.ntfs_enum".into(), state, reason: reason.into() };
        let Some(display) = Self::display(root) else { return probe(CapabilityState::Unsupported, "not a Windows path") };
        let Some(drive) = volume_root(&display) else {
            return probe(CapabilityState::Unsupported, "requires an explicitly selected volume root; a directory scope uses win.directory so the read stays inside the directory");
        };
        match open_volume(&drive) {
            Ok(_) => probe(CapabilityState::Unverified, "volume handle available; experimental backend pending real-device comparison with win.directory"),
            Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) => probe(CapabilityState::PermissionRequired, "whole-volume enumeration needs administrator rights"),
            Err(e) => probe(CapabilityState::Unsupported, &format!("volume unavailable: {e}")),
        }
    }

    fn scan(&self, root: &RawPath, sink: &mut dyn ScanSink, options: &ScanOptions) -> ScanReport {
        let mut report = ScanReport {
            state: ScanState::Failed,
            backend: self.name().into(),
            backend_reason: "whole-volume NTFS enumeration with handle supplement".into(),
            entries: 0,
            skipped_ranges: 0,
            boundaries: 0,
            elevated_read: vec!["volume_handle".into()],
            issues: Vec::new(),
        };
        let fail = |mut report: ScanReport, code: &str, message: String| {
            report.issues.push(ScanIssue { code: code.into(), scope_display: root.display(), message });
            report
        };
        let Some(display) = Self::display(root) else { return fail(report, "unsupported_path", "not a Windows path".into()) };
        let Some(drive) = volume_root(&display) else {
            // Refused before any read: a directory scope never widens into the volume namespace.
            return fail(report, "scope_requires_volume", "win.ntfs_enum only accepts a volume root; use win.directory".into());
        };
        let root_handle = match Handle::open(&display, FILE_READ_ATTRIBUTES | SYNCHRONIZE) {
            Ok(h) => h,
            Err(e) => return fail(report, "root_unavailable", e.to_string()),
        };
        if root_handle.filesystem().ok().as_deref() != Some("NTFS") {
            return fail(report, "unsupported_filesystem", "win.ntfs_enum requires NTFS".into());
        }
        let (serial, id128) = match root_handle.id_info() {
            Ok(v) => v,
            Err(e) => return fail(report, "root_unavailable", e.to_string()),
        };
        let root_frn = u64::from_le_bytes(id128[..8].try_into().unwrap());
        let volume = match open_volume(&drive) {
            Ok(v) => v,
            Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) => return fail(report, "permission_required", "administrator rights required".into()),
            Err(e) => return fail(report, "root_unavailable", e.to_string()),
        };
        let nodes = match enumerate(&volume) {
            Ok(n) => n,
            Err(e) => return fail(report, "io_error", format!("FSCTL_ENUM_USN_DATA: {e}")),
        };
        let volume_id = format!("{}|{serial:016x}", root_handle.volume_guid().unwrap_or_default());
        let mut children: HashMap<u64, Vec<u64>> = HashMap::new();
        for (frn, node) in &nodes {
            children.entry(node.parent & FRN_MASK).or_default().push(*frn);
        }
        for list in children.values_mut() {
            list.sort_unstable();
        }

        let batch = options.batch_entries.max(1);
        let (mut objects, mut entries) = (Vec::new(), Vec::new());
        let mut object_of: HashMap<u64, u64> = HashMap::new();
        let mut dir_entry_by_path: HashMap<Vec<u16>, u64> = HashMap::new();
        let mut multi_link: Vec<(u64, Vec<u16>)> = Vec::new();
        let (mut next_object, mut next_entry) = (0u64, 0u64);
        let mut new_object = |frn: u64, kind, logical, allocated, links, cloud, modified: Option<i64>, objects: &mut Vec<ObjectRow>| -> u64 {
            *object_of.entry(frn).or_insert_with(|| {
                next_object += 1;
                objects.push(ObjectRow {
                    object_id: next_object,
                    volume_id: volume_id.clone(),
                    file_id: format!("ntfs64:{frn:016x}"),
                    generation: Some(format!("{}", frn >> 48)),
                    quality: IdentityQuality::Strong,
                    kind,
                    logical,
                    allocated,
                    link_count: links,
                    cloud,
                    modified_unix: modified,
                });
                next_object
            })
        };
        let root_obj = new_object(root_frn, ObjectKind::Directory, None, None, None, CloudState::NotCloud, None, &mut objects);
        next_entry += 1;
        let root_entry = next_entry;
        let root_name = RawPath::windows(display.clone()).expect("non-empty root");
        entries.push(EntryRow { entry_id: root_entry, parent_entry_id: None, name: root_name, object_id: root_obj, kind: ObjectKind::Directory, boundary: None });
        dir_entry_by_path.insert(vec![b'\\' as u16], root_entry);
        report.entries += 1;

        let mut queue = VecDeque::from([(root_frn & FRN_MASK, root_entry, display.clone(), vec![b'\\' as u16])]);
        let mut state = ScanState::Completed;
        while let Some((dir_frn, dir_entry, dir_full, dir_rel)) = queue.pop_front() {
            if sink.cancel_requested() {
                state = ScanState::Cancelled;
                break;
            }
            let kids: Vec<u64> = children.get(&dir_frn).cloned().unwrap_or_default();
            for frn in kids {
                let node = &nodes[&frn];
                let full = join(&dir_full, &node.name);
                let rel = join(&dir_rel, &node.name);
                let is_dir = node.attributes & ATTR_DIRECTORY != 0;
                let cloud = if node.attributes & ATTR_PLACEHOLDER != 0 { CloudState::Placeholder } else { CloudState::NotCloud };
                let sup = supplement(&volume, frn, node.attributes);
                let (kind, boundary, logical, allocated, links) = match (&sup, is_dir) {
                    (Ok(s), _) if s.reparse_tag == Some(TAG_SYMLINK) => (ObjectKind::Symlink, Some(BoundaryKind::Symlink), None, None, None),
                    (Ok(s), true) if s.reparse_tag.is_some() => (ObjectKind::Reparse, Some(BoundaryKind::Junction), None, None, None),
                    (_, true) => (ObjectKind::Directory, None, None, None, None),
                    (Ok(s), false) => (ObjectKind::File, None, Some(s.end_of_file), Some(s.compressed.unwrap_or(s.allocation)), Some(s.links)),
                    (Err(e), false) => {
                        report.issues.push(ScanIssue { code: "supplement_failed".into(), scope_display: String::from_utf16_lossy(&full), message: e.to_string() });
                        (ObjectKind::File, None, None, None, None)
                    }
                };
                let modified = sup.as_ref().ok().and_then(|s| s.modified_unix);
                let obj = new_object(frn, kind, logical, allocated, links, cloud, modified, &mut objects);
                next_entry += 1;
                let Ok(name) = RawPath::windows(node.name.clone()) else { continue };
                entries.push(EntryRow { entry_id: next_entry, parent_entry_id: Some(dir_entry), name, object_id: obj, kind, boundary });
                report.entries += 1;
                if boundary.is_some() {
                    report.boundaries += 1;
                }
                if kind == ObjectKind::Directory {
                    dir_entry_by_path.insert(rel.clone(), next_entry);
                    queue.push_back((frn & FRN_MASK, next_entry, full, rel));
                } else if links.is_some_and(|n| n > 1) {
                    multi_link.push((obj, full));
                }
            }
            if entries.len() >= batch {
                if let Err(m) = sink.write(&objects, &entries, &[]) {
                    report.issues.push(ScanIssue { code: "store_error".into(), scope_display: String::new(), message: m });
                    state = ScanState::Failed;
                    break;
                }
                objects.clear();
                entries.clear();
            }
        }
        // Hard links: the enumeration yields one name per file record; add the other names.
        if state == ScanState::Completed {
            for (obj, full) in &multi_link {
                for rel in link_names(full) {
                    let (parent, name) = match rel.iter().rposition(|&c| c == b'\\' as u16) {
                        Some(0) => (vec![b'\\' as u16], rel[1..].to_vec()),
                        Some(i) => (rel[..i].to_vec(), rel[i + 1..].to_vec()),
                        None => continue,
                    };
                    let Some(&parent_entry) = dir_entry_by_path.get(&parent) else { continue };
                    let full_name = join(&display, &rel[1..]);
                    if full_name == *full {
                        continue;
                    }
                    next_entry += 1;
                    let Ok(name) = RawPath::windows(name) else { continue };
                    entries.push(EntryRow { entry_id: next_entry, parent_entry_id: Some(parent_entry), name, object_id: *obj, kind: ObjectKind::File, boundary: None });
                    report.entries += 1;
                }
            }
        }
        if let Err(m) = sink.write(&objects, &entries, &[]) {
            report.issues.push(ScanIssue { code: "store_error".into(), scope_display: String::new(), message: m });
            state = ScanState::Failed;
        }
        report.state = state;
        report
    }
}

#[cfg(test)]
mod tests;
