//! `win.directory`: scoped, read-only directory enumeration (SCN-01/02/03/05/06/07).
//!
//! - Lists each directory through its handle with `FileIdExtdDirectoryInfo` (128-bit IDs,
//!   reparse tags), falling back to `FileIdBothDirectoryInfo` where unsupported (FAT/exFAT).
//! - Never follows symlinks, junctions, mount points or cloud-provider directories; they are
//!   recorded as boundaries. Files are never opened, so no content is read or recalled.
//! - Re-opens each subdirectory and checks its volume serial and file ID against the parent
//!   listing; a replaced directory is skipped, a changed volume stops the scan.
//! - Unreadable subtrees become skipped ranges, never zero-sized directories.
//! - The root is judged on its own handle before any row exists: a link (`root_is_link`) or a
//!   non-directory (`not_a_directory`) fails the scan instead of being followed or recorded.
//! - Listings are read buffer by buffer and rows are written in batches; cancellation is polled
//!   between directories and after every batch, so one huge directory still stops promptly.

use std::collections::{HashMap, VecDeque};
use std::io;

use cm_core::capability::CapabilityState;
use cm_core::model::{BoundaryKind, CloudState, IdentityQuality, ObjectKind, ScanState, SkipReason};
use cm_core::path::RawPath;
use cm_core::scan::{EntryRow, ObjectRow, RootProbe, ScanBackend, ScanIssue, ScanOptions, ScanReport, ScanSink, SkippedRow, issue};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ID_BOTH_DIR_INFO, FILE_ID_EXTD_DIR_INFO, FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo,
    FileIdExtdDirectoryInfo, FileIdExtdDirectoryRestartInfo, GetFileInformationByHandleEx,
};

use super::handle::{FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, Handle, SYNCHRONIZE, full_path};

const ATTR_DIRECTORY: u32 = 0x10;
const ATTR_REPARSE_POINT: u32 = 0x400;
const ATTR_SPARSE: u32 = 0x200;
const ATTR_COMPRESSED: u32 = 0x800;
const ATTR_OFFLINE: u32 = 0x1000;
const ATTR_RECALL_ON_OPEN: u32 = 0x0004_0000;
const ATTR_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
const TAG_MOUNT_POINT: u32 = 0xA000_0003;
const TAG_SYMLINK: u32 = 0xA000_000C;
/// Reparse-tag bit Windows sets for tags that name another entity (symlinks, junctions, ...).
const TAG_NAME_SURROGATE: u32 = 0x2000_0000;
const ERROR_ACCESS_DENIED: i32 = 5;
const ERROR_NO_MORE_FILES: i32 = 18;
const ERROR_INVALID_PARAMETER: i32 = 87;
const ERROR_NOT_SUPPORTED: i32 = 50;
const ERROR_INVALID_FUNCTION: i32 = 1;

fn is_cloud_tag(tag: u32) -> bool {
    tag & 0xFFFF_0FFF == 0x9000_001A
}

/// One directory-listing record, independent of the information class that produced it.
#[derive(Debug, Clone)]
pub struct Record {
    pub name: Vec<u16>,
    pub attributes: u32,
    pub reparse_tag: Option<u32>,
    pub file_id: [u8; 16],
    pub end_of_file: u64,
    pub allocation: u64,
    /// FILETIME of the last write.
    pub last_write: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListClass {
    Extd,
    Both,
}

/// One directory's listing, read one 64 KiB buffer at a time so the caller can write batches and
/// observe a cancel request while a huge directory is still being listed.
struct Listing<'a> {
    dir: &'a Handle,
    class: ListClass,
    buf: Vec<u64>,
    first: bool,
}

impl<'a> Listing<'a> {
    fn new(dir: &'a Handle, class: ListClass) -> Self {
        Listing { dir, class, buf: vec![0u64; 64 * 1024 / 8], first: true }
    }

    /// No buffer was returned yet, so falling back to another information class repeats nothing.
    fn nothing_returned(&self) -> bool {
        self.first
    }

    /// The next buffer of records (without `.` and `..`); `None` once the listing is complete.
    fn next_chunk(&mut self) -> io::Result<Option<Vec<Record>>> {
        let info_class = match (self.class, self.first) {
            (ListClass::Extd, true) => FileIdExtdDirectoryRestartInfo,
            (ListClass::Extd, false) => FileIdExtdDirectoryInfo,
            (ListClass::Both, true) => FileIdBothDirectoryRestartInfo,
            (ListClass::Both, false) => FileIdBothDirectoryInfo,
        };
        // SAFETY: 8-byte aligned buffer of the declared size.
        let ok = unsafe { GetFileInformationByHandleEx(self.dir.raw(), info_class, self.buf.as_mut_ptr().cast(), (self.buf.len() * 8) as u32) };
        if ok == 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(ERROR_NO_MORE_FILES) {
                return Ok(None);
            }
            return Err(err);
        }
        self.first = false;
        let mut out = Vec::new();
        let base = self.buf.as_ptr().cast::<u8>();
        let mut offset = 0usize;
        loop {
            // SAFETY: the API wrote a chain of records inside the buffer; offsets come from it.
            let rec = unsafe { parse_record(base.add(offset), self.class) };
            if rec.name != [b'.' as u16] && rec.name != [b'.' as u16, b'.' as u16] {
                out.push(rec);
            }
            // SAFETY: NextEntryOffset is the first u32 of both record layouts.
            let next = unsafe { *(base.add(offset) as *const u32) } as usize;
            if next == 0 {
                break;
            }
            offset += next;
        }
        Ok(Some(out))
    }
}

/// # Safety
/// `p` must point at a complete record of the given class inside a valid buffer.
unsafe fn parse_record(p: *const u8, class: ListClass) -> Record {
    unsafe {
        match class {
            ListClass::Extd => {
                let r = &*(p as *const FILE_ID_EXTD_DIR_INFO);
                let name = std::slice::from_raw_parts(r.FileName.as_ptr(), r.FileNameLength as usize / 2).to_vec();
                Record {
                    name,
                    attributes: r.FileAttributes,
                    reparse_tag: (r.FileAttributes & ATTR_REPARSE_POINT != 0).then_some(r.ReparsePointTag),
                    file_id: r.FileId.Identifier,
                    end_of_file: r.EndOfFile as u64,
                    allocation: r.AllocationSize as u64,
                    last_write: r.LastWriteTime,
                }
            }
            ListClass::Both => {
                let r = &*(p as *const FILE_ID_BOTH_DIR_INFO);
                let name = std::slice::from_raw_parts(r.FileName.as_ptr(), r.FileNameLength as usize / 2).to_vec();
                let mut id = [0u8; 16];
                id[..8].copy_from_slice(&r.FileId.to_le_bytes());
                // EaSize carries the reparse tag for reparse points in this information class.
                Record {
                    name,
                    attributes: r.FileAttributes,
                    reparse_tag: (r.FileAttributes & ATTR_REPARSE_POINT != 0).then_some(r.EaSize),
                    file_id: id,
                    end_of_file: r.EndOfFile as u64,
                    allocation: r.AllocationSize as u64,
                    last_write: r.LastWriteTime,
                }
            }
        }
    }
}

fn hex(id: &[u8; 16]) -> String {
    // FILE_ID_128 is little-endian; print most significant byte first.
    id.iter().rev().map(|b| format!("{b:02x}")).collect()
}

fn join(parent: &[u16], name: &[u16]) -> Vec<u16> {
    let mut v = parent.to_vec();
    if v.last() != Some(&(b'\\' as u16)) {
        v.push(b'\\' as u16);
    }
    v.extend_from_slice(name);
    v
}

fn skip_reason(e: &io::Error) -> SkipReason {
    match e.raw_os_error() {
        Some(ERROR_ACCESS_DENIED) => SkipReason::PermissionDenied,
        _ => SkipReason::Error,
    }
}

struct Ctx<'a> {
    sink: &'a mut dyn ScanSink,
    batch: usize,
    volume_id: String,
    strong: bool,
    objects: Vec<ObjectRow>,
    entries: Vec<EntryRow>,
    skipped: Vec<SkippedRow>,
    object_ids: HashMap<[u8; 16], u64>,
    next_object: u64,
    next_entry: u64,
    report: ScanReport,
}

impl Ctx<'_> {
    fn flush(&mut self) -> Result<(), String> {
        if self.objects.is_empty() && self.entries.is_empty() && self.skipped.is_empty() {
            return Ok(());
        }
        self.sink.write(&self.objects, &self.entries, &self.skipped)?;
        self.objects.clear();
        self.entries.clear();
        self.skipped.clear();
        Ok(())
    }

    /// Write a full batch, if one is pending. `Ok(true)` when rows were written: the caller then
    /// polls for cancellation, which also covers a single directory with very many entries.
    fn maybe_flush(&mut self) -> Result<bool, String> {
        if self.entries.len() + self.skipped.len() >= self.batch { self.flush().map(|()| true) } else { Ok(false) }
    }

    /// Objects are keyed by file ID so hard links share one object. A zero ID (some FAT
    /// implementations) cannot be trusted: such objects get a fresh, weak identity.
    fn object(&mut self, rec: &Record, kind: ObjectKind, logical: Option<u64>, allocated: Option<u64>, cloud: CloudState) -> u64 {
        let zero = rec.file_id == [0u8; 16];
        if !zero && let Some(id) = self.object_ids.get(&rec.file_id) {
            return *id;
        }
        self.next_object += 1;
        let id = self.next_object;
        if !zero {
            self.object_ids.insert(rec.file_id, id);
        }
        self.objects.push(ObjectRow {
            object_id: id,
            volume_id: self.volume_id.clone(),
            file_id: format!("fileid128:{}", hex(&rec.file_id)),
            generation: None,
            quality: if self.strong && !zero { IdentityQuality::Strong } else { IdentityQuality::Weak },
            kind,
            logical,
            allocated,
            link_count: None,
            cloud,
            modified_unix: (rec.last_write > 0).then(|| cm_core::time::filetime_to_unix(rec.last_write as u64)),
            shared_blocks: None,
        });
        id
    }

    fn entry(&mut self, parent: Option<u64>, name: Vec<u16>, object: u64, kind: ObjectKind, boundary: Option<BoundaryKind>) -> Result<u64, String> {
        self.next_entry += 1;
        let id = self.next_entry;
        let name = RawPath::windows(name).map_err(|e| e.to_string())?;
        self.entries.push(EntryRow { entry_id: id, parent_entry_id: parent, name, object_id: object, kind, boundary });
        self.report.entries += 1;
        if boundary.is_some() {
            self.report.boundaries += 1;
        }
        Ok(id)
    }

    fn skip(&mut self, parent: Option<u64>, path: &[u16], reason: SkipReason) -> Result<(), String> {
        let path = RawPath::windows(path.to_vec()).map_err(|e| e.to_string())?;
        self.skipped.push(SkippedRow { parent_entry_id: parent, path, reason });
        self.report.skipped_ranges += 1;
        Ok(())
    }

    fn issue(&mut self, code: &str, path: &[u16], message: String) {
        self.report.issues.push(ScanIssue { code: code.into(), scope_display: String::from_utf16_lossy(path), message });
    }
}

/// Allocation and link count of a compressed or sparse file, measured through an
/// attribute-only handle. Returns unknown when the handle's identity differs from the listing.
fn supplement(verbatim: &[u16], serial: u64, expected: [u8; 16]) -> (Option<u64>, Option<u32>) {
    let Ok(h) = Handle::open(verbatim, FILE_READ_ATTRIBUTES | SYNCHRONIZE) else { return (None, None) };
    match h.id_info() {
        Ok((s, id)) if s == serial && id == expected => (h.compressed_size().ok(), h.link_count().ok()),
        _ => (None, None),
    }
}

struct Work {
    entry_id: u64,
    display: Vec<u16>,
    verbatim: Vec<u16>,
    expected_id: [u8; 16],
}

/// Record one listed child of `work`: links, mount points and cloud or own-store directories as
/// boundaries, other directories queued for traversal, files with their measured sizes.
fn record(ctx: &mut Ctx<'_>, rec: &Record, work: &Work, queue: &mut VecDeque<Work>, own_store: &[[u8; 16]], serial: u64) -> Result<(), String> {
    let is_dir = rec.attributes & ATTR_DIRECTORY != 0;
    let placeholder = rec.attributes & (ATTR_OFFLINE | ATTR_RECALL_ON_OPEN | ATTR_RECALL_ON_DATA_ACCESS) != 0
        || rec.reparse_tag.is_some_and(is_cloud_tag);
    let cloud = if placeholder { CloudState::Placeholder } else { CloudState::NotCloud };
    match (rec.reparse_tag, is_dir) {
        (Some(TAG_SYMLINK), _) => {
            let o = ctx.object(rec, ObjectKind::Symlink, None, None, cloud);
            ctx.entry(Some(work.entry_id), rec.name.clone(), o, ObjectKind::Symlink, Some(BoundaryKind::Symlink)).map(|_| ())
        }
        (Some(TAG_MOUNT_POINT), _) => {
            let o = ctx.object(rec, ObjectKind::Reparse, None, None, cloud);
            ctx.entry(Some(work.entry_id), rec.name.clone(), o, ObjectKind::Reparse, Some(BoundaryKind::Junction)).map(|_| ())
        }
        (Some(tag), true) if is_cloud_tag(tag) => {
            // Unverified cloud providers are not traversed: listing could hydrate content (SCN-05).
            let o = ctx.object(rec, ObjectKind::Directory, None, None, cloud);
            ctx.entry(Some(work.entry_id), rec.name.clone(), o, ObjectKind::Directory, Some(BoundaryKind::CloudProvider)).map(|_| ())
        }
        (Some(_), true) => {
            let o = ctx.object(rec, ObjectKind::Reparse, None, None, cloud);
            ctx.entry(Some(work.entry_id), rec.name.clone(), o, ObjectKind::Reparse, Some(BoundaryKind::Junction)).map(|_| ())
        }
        (_, true) if own_store.contains(&rec.file_id) => {
            ctx.issue("own_store_skipped", &join(&work.display, &rec.name), OWN_STORE_MESSAGE.into());
            let o = ctx.object(rec, ObjectKind::Directory, None, None, cloud);
            ctx.entry(Some(work.entry_id), rec.name.clone(), o, ObjectKind::Directory, Some(BoundaryKind::OwnStore)).map(|_| ())
        }
        (_, true) => {
            let o = ctx.object(rec, ObjectKind::Directory, None, None, cloud);
            ctx.entry(Some(work.entry_id), rec.name.clone(), o, ObjectKind::Directory, None).map(|id| {
                queue.push_back(Work {
                    entry_id: id,
                    display: join(&work.display, &rec.name),
                    verbatim: join(&work.verbatim, &rec.name),
                    expected_id: rec.file_id,
                });
            })
        }
        (_, false) => {
            // Compressed/sparse allocation from a directory listing is not trusted: it
            // is measured through an attribute-only handle whose identity is checked.
            // Cloud placeholders are never opened, so their allocation stays as listed.
            let (alloc, links) = if rec.attributes & (ATTR_COMPRESSED | ATTR_SPARSE) != 0 && !placeholder {
                supplement(&join(&work.verbatim, &rec.name), serial, rec.file_id)
            } else {
                (Some(rec.allocation), None)
            };
            let o = ctx.object(rec, ObjectKind::File, Some(rec.end_of_file), alloc, cloud);
            if let Some(n) = links
                && let Some(obj) = ctx.objects.iter_mut().rev().find(|x| x.object_id == o)
            {
                obj.link_count = Some(n);
            }
            ctx.entry(Some(work.entry_id), rec.name.clone(), o, ObjectKind::File, None).map(|_| ())
        }
    }
}

pub struct DirectoryBackend;

/// File IDs (on the scanned volume `serial`) of CleanManager's own data directories: the ones
/// the caller named plus the default store location. Unresolvable paths are simply absent.
fn own_store_ids(dirs: &[RawPath], serial: u64) -> Vec<[u8; 16]> {
    let mut paths: Vec<Vec<u16>> = dirs.iter().filter_map(|p| match p {
        RawPath::Windows(u) => Some(u.clone()),
        RawPath::Unix(_) => None,
    }).collect();
    if let Some(dir) = crate::paths::default_store_dir() {
        use std::os::windows::ffi::OsStrExt;
        paths.push(dir.as_os_str().encode_wide().collect());
    }
    let mut out = Vec::new();
    for units in paths {
        let Ok((_, verbatim)) = full_path(&units) else { continue };
        let Ok(h) = Handle::open(&verbatim, FILE_READ_ATTRIBUTES | SYNCHRONIZE) else { continue };
        if let Ok((s, id)) = h.id_info()
            && s == serial
            && !out.contains(&id)
        {
            out.push(id);
        }
    }
    out
}

const OWN_STORE_MESSAGE: &str = "CleanManager's own data directory: recorded but not scanned, so the scan never reads its own index writes; its size is not counted";

/// Protocol identity (`volume_id`, `file_id`) of a path, in the same form `win.directory`
/// records, so rule roots and protected locations can be matched by identity, not by text.
pub fn identity_of(path: &[u16]) -> io::Result<(String, String)> {
    let (_, verbatim) = full_path(path)?;
    let h = Handle::open(&verbatim, FILE_READ_ATTRIBUTES | SYNCHRONIZE)?;
    let (serial, id) = h.id_info()?;
    Ok((format!("{}|{serial:016x}", h.volume_guid().unwrap_or_default()), format!("fileid128:{}", hex(&id))))
}

/// Why the opened root must not be scanned, as `(issue code, message)`, judged on the handle
/// itself (opened without following links) before any row exists. A link is refused, never
/// followed, and a non-directory is never recorded as one. A cloud-provider directory is the one
/// reparse point accepted as a root: it is listed by metadata only, and its placeholder
/// subdirectories stay `cloud_provider` boundaries (SCN-05).
fn root_refusal(h: &Handle) -> Option<(&'static str, String)> {
    let (attributes, tag) = match h.attribute_tag() {
        Ok(v) => v,
        Err(e) => {
            let code = if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) { issue::PERMISSION_REQUIRED } else { issue::ROOT_UNAVAILABLE };
            return Some((code, format!("cannot inspect the root's type: {e}")));
        }
    };
    let is_dir = attributes & ATTR_DIRECTORY != 0;
    if attributes & ATTR_REPARSE_POINT != 0 && !(is_dir && is_cloud_tag(tag)) {
        let message = match tag {
            TAG_SYMLINK => "the root is a symbolic link".to_owned(),
            TAG_MOUNT_POINT => "the root is a junction or volume mount point".to_owned(),
            t if t & TAG_NAME_SURROGATE != 0 => format!("the root is a link (reparse tag {t:#010x})"),
            t => format!("the root is a reparse point the scanner does not traverse (tag {t:#010x})"),
        };
        return Some((
            issue::ROOT_IS_LINK,
            format!("{message}; links are never followed and nothing behind it was read. Scan the folder it points to directly"),
        ));
    }
    (!is_dir).then(|| (issue::NOT_A_DIRECTORY, "root is not a directory".to_owned()))
}

impl DirectoryBackend {
    fn units(root: &RawPath) -> Option<Vec<u16>> {
        match root {
            RawPath::Windows(u) => Some(u.clone()),
            RawPath::Unix(_) => None,
        }
    }
}

impl ScanBackend for DirectoryBackend {
    fn name(&self) -> &'static str {
        "win.directory"
    }

    fn probe(&self, root: &RawPath) -> RootProbe {
        let fail = |reason: String, state| RootProbe { filesystem: None, volume_id: None, backend: "win.directory".into(), state, reason };
        let Some(units) = Self::units(root) else { return fail("not a Windows path".into(), CapabilityState::Unsupported) };
        let (_, verbatim) = match full_path(&units) {
            Ok(p) => p,
            Err(e) => return fail(format!("cannot resolve root: {e}"), CapabilityState::Unsupported),
        };
        match Handle::open(&verbatim, FILE_READ_ATTRIBUTES | SYNCHRONIZE) {
            Ok(h) => {
                if let Some((code, reason)) = root_refusal(&h) {
                    let state = if code == issue::PERMISSION_REQUIRED { CapabilityState::PermissionRequired } else { CapabilityState::Unsupported };
                    return fail(reason, state);
                }
                let fs = h.filesystem().ok();
                let serial = h.id_info().ok().map(|(s, _)| s);
                RootProbe {
                    filesystem: fs.clone(),
                    volume_id: serial.map(|s| format!("{}|{s:016x}", h.volume_guid().unwrap_or_default())),
                    backend: "win.directory".into(),
                    // Real-device evidence decides `available`; until then the backend is unverified.
                    state: CapabilityState::Unverified,
                    reason: format!(
                        "Directory enumeration limited to the selected root on {}; whole-volume fast paths are not used for a directory scope",
                        fs.unwrap_or_else(|| "an unknown file system".into())
                    ),
                }
            }
            Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) => fail(format!("root not readable: {e}"), CapabilityState::PermissionRequired),
            Err(e) => fail(format!("root unavailable: {e}"), CapabilityState::Unsupported),
        }
    }

    fn scan(&self, root: &RawPath, sink: &mut dyn ScanSink, options: &ScanOptions) -> ScanReport {
        let mut report = ScanReport {
            state: ScanState::Failed,
            backend: self.name().into(),
            backend_reason: "directory enumeration within the selected root".into(),
            entries: 0,
            skipped_ranges: 0,
            boundaries: 0,
            elevated_read: super::privileges::enabled_bypass_privileges()
                .map(|v| v.into_iter().map(str::to_owned).collect())
                .unwrap_or_else(|_| vec!["unknown".into()]),
            issues: Vec::new(),
        };
        let Some(units) = Self::units(root) else {
            report.issues.push(ScanIssue { code: "unsupported_path".into(), scope_display: root.display(), message: "not a Windows path".into() });
            return report;
        };
        let (display, verbatim) = match full_path(&units) {
            Ok(p) => p,
            Err(e) => {
                report.issues.push(ScanIssue { code: "root_unavailable".into(), scope_display: root.display(), message: e.to_string() });
                return report;
            }
        };
        let root_handle = match Handle::open(&verbatim, FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE) {
            Ok(h) => h,
            Err(e) => {
                let code = if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) { "permission_required" } else { "root_unavailable" };
                report.issues.push(ScanIssue { code: code.into(), scope_display: root.display(), message: e.to_string() });
                return report;
            }
        };
        // Check this handle before creating the root entry: a regular file or a link can be opened
        // with these rights too, but must never be recorded as a directory or followed.
        if let Some((code, message)) = root_refusal(&root_handle) {
            report.issues.push(ScanIssue { code: code.into(), scope_display: root.display(), message });
            return report;
        }
        let (serial, root_id) = match root_handle.id_info() {
            Ok(v) => v,
            Err(e) => {
                report.issues.push(ScanIssue { code: "root_unavailable".into(), scope_display: root.display(), message: e.to_string() });
                return report;
            }
        };
        let fs = root_handle.filesystem().unwrap_or_default();
        let strong = matches!(fs.as_str(), "NTFS" | "ReFS");
        let volume_id = format!("{}|{serial:016x}", root_handle.volume_guid().unwrap_or_default());
        report.backend_reason = if report.elevated_read.is_empty() {
            format!("directory enumeration within the selected root on {fs} with ordinary permissions")
        } else {
            format!("directory enumeration within the selected root on {fs} WITH ACL-bypass privileges enabled: {}", report.elevated_read.join(", "))
        };
        let own_store = own_store_ids(&options.own_store_dirs, serial);
        let mut class = ListClass::Extd;
        let mut ctx = Ctx {
            sink,
            batch: options.batch_entries.max(1),
            volume_id,
            strong,
            objects: Vec::new(),
            entries: Vec::new(),
            skipped: Vec::new(),
            object_ids: HashMap::new(),
            next_object: 0,
            next_entry: 0,
            report,
        };

        let root_rec = Record { name: display.clone(), attributes: ATTR_DIRECTORY, reparse_tag: None, file_id: root_id, end_of_file: 0, allocation: 0, last_write: 0 };
        let root_obj = ctx.object(&root_rec, ObjectKind::Directory, None, None, CloudState::NotCloud);
        let root_entry = match ctx.entry(None, display.clone(), root_obj, ObjectKind::Directory, None) {
            Ok(id) => id,
            Err(e) => {
                ctx.issue("internal_error", &display, e);
                return ctx.report;
            }
        };
        if own_store.contains(&root_id) {
            ctx.issue("own_store_skipped", &display, OWN_STORE_MESSAGE.into());
            if let Some(e) = ctx.entries.last_mut() {
                e.boundary = Some(BoundaryKind::OwnStore);
                ctx.report.boundaries += 1;
            }
            ctx.report.state = match ctx.flush() {
                Ok(()) => ScanState::Completed,
                Err(msg) => {
                    ctx.issue("store_error", &display, msg);
                    ScanState::Failed
                }
            };
            return ctx.report;
        }
        let mut queue = VecDeque::from([Work { entry_id: root_entry, display, verbatim, expected_id: root_id }]);
        let mut held_root = Some(root_handle);
        let mut state = ScanState::Completed;

        while let Some(work) = queue.pop_front() {
            if ctx.sink.cancel_requested() {
                state = ScanState::Cancelled;
                break;
            }
            let dir = match held_root.take() {
                Some(h) => h,
                None => match Handle::open(&work.verbatim, FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE) {
                    Ok(h) => h,
                    Err(e) => {
                        let reason = skip_reason(&e);
                        let parent = work.entry_id;
                        if let Err(msg) = ctx.skip(Some(parent), &work.display, reason) {
                            ctx.issue("internal_error", &work.display, msg);
                        }
                        ctx.issue(if reason == SkipReason::PermissionDenied { "permission_required" } else { "io_error" }, &work.display, e.to_string());
                        continue;
                    }
                },
            };
            match dir.id_info() {
                Ok((s, _)) if s != serial => {
                    ctx.issue("volume_changed", &work.display, "volume identity changed during the scan; stopping".into());
                    let _ = ctx.skip(Some(work.entry_id), &work.display, SkipReason::VolumeChanged);
                    state = ScanState::Partial;
                    break;
                }
                Ok((_, id)) if id != work.expected_id => {
                    ctx.issue("identity_changed", &work.display, "directory was replaced after it was listed; not traversed".into());
                    let _ = ctx.skip(Some(work.entry_id), &work.display, SkipReason::Error);
                    continue;
                }
                Ok(_) => {}
                Err(e) => {
                    ctx.issue("io_error", &work.display, e.to_string());
                    let _ = ctx.skip(Some(work.entry_id), &work.display, SkipReason::Error);
                    continue;
                }
            }
            let mut listing = Listing::new(&dir, class);
            'listing: loop {
                let chunk = match listing.next_chunk() {
                    Ok(Some(chunk)) => chunk,
                    Ok(None) => break,
                    Err(e) if class == ListClass::Extd
                        && listing.nothing_returned()
                        && matches!(e.raw_os_error(), Some(ERROR_INVALID_PARAMETER | ERROR_NOT_SUPPORTED | ERROR_INVALID_FUNCTION)) =>
                    {
                        class = ListClass::Both;
                        ctx.report.backend_reason.push_str("; extended directory information unsupported, using FileIdBothDirectoryInfo");
                        listing = Listing::new(&dir, class);
                        continue;
                    }
                    Err(e) => {
                        // Rows already written for this directory stay; the unread rest is a skipped range.
                        ctx.issue(if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) { "permission_required" } else { "io_error" }, &work.display, e.to_string());
                        let _ = ctx.skip(Some(work.entry_id), &work.display, skip_reason(&e));
                        break;
                    }
                };
                for rec in chunk {
                    if let Err(msg) = record(&mut ctx, &rec, &work, &mut queue, &own_store, serial) {
                        ctx.issue("internal_error", &work.display, msg);
                    }
                    match ctx.maybe_flush() {
                        Ok(false) => {}
                        // Polled after every written batch, not only between directories: a cancel
                        // request stops even a directory with hundreds of thousands of entries.
                        Ok(true) if ctx.sink.cancel_requested() => {
                            state = ScanState::Cancelled;
                            break 'listing;
                        }
                        Ok(true) => {}
                        Err(msg) => {
                            ctx.issue("store_error", &work.display, msg);
                            state = ScanState::Failed;
                            break 'listing;
                        }
                    }
                }
            }
            if matches!(state, ScanState::Cancelled | ScanState::Failed) {
                break;
            }
        }
        if let Err(msg) = ctx.flush() {
            ctx.report.issues.push(ScanIssue { code: "store_error".into(), scope_display: String::new(), message: msg });
            state = ScanState::Failed;
        }
        ctx.report.state = state;
        ctx.report
    }
}

#[cfg(test)]
mod tests;
