//! Windows file actions for the executor (MVP 9.4): handle-verified quarantine/delete and recycle-bin moves.
//!
//! Verification (`check`, and again inside every action):
//! - The planned ancestors (`parent_chain`, scope root last) are opened top-down with
//!   `FILE_FLAG_OPEN_REPARSE_POINT` and *without* `FILE_SHARE_DELETE`, so while they are held no
//!   other process can rename, replace or delete them; each must carry its planned identity.
//! - The target itself is opened with `FILE_FLAG_OPEN_REPARSE_POINT` (links are inspected, never
//!   followed) and its volume/file ID, reparse state, type, hard-link count, logical size and
//!   last-write time are compared on that handle. No file content is ever read.
//! - Delete and quarantine then act on the *same* handle (`SetFileInformationByHandle`), opened
//!   without `FILE_SHARE_DELETE` so the verified name cannot be renamed away in between.
//!
//! Residual risks (documented, not hidden):
//! - Ancestors above the scope root are resolved by ordinary path lookup.
//! - The recycle bin is a Shell, path-based operation: between the last identity check (inside
//!   `PreDeleteItem`) and the Shell's own open, a same-name swap could make it recycle a different
//!   file. That is accepted only because the result is recoverable from the Recycle Bin, and the
//!   outcome is confirmed afterwards through a handle held on the verified file.
//! - A handle does not stop processes that already have the file open from writing to it.

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use cm_core::execute::{ActionOutcome, ActionTarget, FileActions, LiveCheck, Reconciled, Recovery, TreeMember};
use cm_core::model::Identity;
use cm_core::obs::{ObservationRecord, TimeUnit};
use cm_core::path::RawPath;
use cm_core::rules::Mode;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_ATTRIBUTE_TAG_INFO, FILE_BASIC_INFO, FILE_DISPOSITION_FLAG_DELETE,
    FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX, FILE_ID_INFO, FILE_RENAME_INFO,
    FILE_STANDARD_INFO, FileAttributeTagInfo, FileBasicInfo, FileDispositionInfo, FileDispositionInfoEx, FileIdInfo,
    FileRenameInfo, FileRenameInfoEx, FileStandardInfo, GetDiskFreeSpaceExW, GetDriveTypeW, GetFileInformationByHandle,
    GetFileInformationByHandleEx, GetFinalPathNameByHandleW, GetVolumePathNameW, SetFileInformationByHandle,
};

use super::handle::full_path;

const DELETE: u32 = 0x0001_0000;
const FILE_READ_ATTRIBUTES: u32 = 0x0080;
const SYNCHRONIZE: u32 = 0x0010_0000;
const SHARE_READ: u32 = 0x1;
const SHARE_WRITE: u32 = 0x2;
const SHARE_DELETE: u32 = 0x4;
const SHARE_ALL: u32 = SHARE_READ | SHARE_WRITE | SHARE_DELETE;
/// Pins a directory: others may read/write it but cannot rename, replace or delete it.
const SHARE_PIN: u32 = SHARE_READ | SHARE_WRITE;
const OPEN_EXISTING: u32 = 3;
const FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const ATTR_DIRECTORY: u32 = 0x10;
const ATTR_REPARSE_POINT: u32 = 0x400;
const VOLUME_NAME_DOS: u32 = 0x0;
const VOLUME_NAME_GUID: u32 = 0x1;
const DRIVE_REMOTE: u32 = 4;
/// FILETIME of 1970-01-01T00:00:00Z.
const UNIX_EPOCH_FILETIME: i128 = 116_444_736_000_000_000;

const ERROR_INVALID_FUNCTION: i32 = 1;
const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_PATH_NOT_FOUND: i32 = 3;
const ERROR_ACCESS_DENIED: i32 = 5;
const ERROR_NOT_SAME_DEVICE: i32 = 17;
const ERROR_SHARING_VIOLATION: i32 = 32;
const ERROR_NOT_SUPPORTED: i32 = 50;
const ERROR_FILE_EXISTS: i32 = 80;
const ERROR_INVALID_PARAMETER: i32 = 87;
const ERROR_INVALID_LEVEL: i32 = 124;
const ERROR_ALREADY_EXISTS: i32 = 183;

/// Windows implementation of the executor's file actions. Every call re-verifies; the only
/// state is which store directory hosts same-volume quarantine (default: the user store).
#[derive(Debug, Clone, Default)]
pub struct WindowsActions {
    store_dir: Option<std::path::PathBuf>,
}

impl WindowsActions {
    pub const DEFAULT: WindowsActions = WindowsActions { store_dir: None };

    /// Quarantine under `dir` (the `--store` in use) when it is on the target's volume.
    pub fn with_store(dir: std::path::PathBuf) -> Self {
        WindowsActions { store_dir: Some(dir) }
    }
}

// ---------------------------------------------------------------------------------------------
// Handles and live metadata

struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateFileW and is closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

fn open(path: &[u16], access: u32, share: u32, flags: u32) -> io::Result<Owned> {
    let z = nul(path);
    // SAFETY: NUL-terminated wide path; no security attributes or template.
    let h = unsafe { CreateFileW(z.as_ptr(), access, share, std::ptr::null(), OPEN_EXISTING, flags | FLAG_OPEN_REPARSE_POINT, std::ptr::null_mut()) };
    if h == INVALID_HANDLE_VALUE { Err(io::Error::last_os_error()) } else { Ok(Owned(h)) }
}

/// Attribute-only, link-preserving open of a file or directory; never blocks other users.
fn open_attrs(path: &[u16]) -> io::Result<Owned> {
    open(path, FILE_READ_ATTRIBUTES | SYNCHRONIZE, SHARE_ALL, FLAG_BACKUP_SEMANTICS)
}

fn nul(path: &[u16]) -> Vec<u16> {
    let mut z = path.to_vec();
    z.push(0);
    z
}

fn query<T>(h: &Owned, class: i32) -> io::Result<T> {
    // SAFETY: T is the plain-data structure documented for `class`; zero is a valid bit pattern.
    let mut info: T = unsafe { std::mem::zeroed() };
    // SAFETY: buffer is a T of the declared size.
    let ok = unsafe { GetFileInformationByHandleEx(h.0, class, (&mut info as *mut T).cast(), size_of::<T>() as u32) };
    if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(info) }
}

/// Metadata of an opened object, with identity in the `win.directory` protocol form.
#[derive(Debug, Clone)]
struct Live {
    volume_id: String,
    file_id: String,
    attributes: u32,
    size: u64,
    last_write: i64,
    links: u32,
    directory: bool,
}

impl Live {
    fn reparse(&self) -> bool {
        self.attributes & ATTR_REPARSE_POINT != 0
    }
    fn is(&self, volume_id: &str, file_id: &str) -> bool {
        same_volume(&self.volume_id, volume_id) && self.file_id == file_id
    }
    fn is_identity(&self, id: &Identity) -> bool {
        self.is(&id.volume_id, &id.wire_file_id())
    }
}

fn hex(id: &[u8; 16]) -> String {
    // FILE_ID_128 is little-endian; print most significant byte first (as `win.directory`).
    id.iter().rev().map(|b| format!("{b:02x}")).collect()
}

fn volume_guid(h: &Owned) -> String {
    let mut buf = vec![0u16; 1024];
    // SAFETY: buffer length is passed in characters.
    let n = unsafe { GetFinalPathNameByHandleW(h.0, buf.as_mut_ptr(), buf.len() as u32, VOLUME_NAME_GUID) } as usize;
    if n == 0 || n >= buf.len() {
        return String::new();
    }
    let s = String::from_utf16_lossy(&buf[..n]);
    s.find('}').map(|end| s[..=end].to_owned()).unwrap_or_default()
}

fn live(h: &Owned) -> io::Result<Live> {
    let (serial, id) = match query::<FILE_ID_INFO>(h, FileIdInfo) {
        Ok(info) => (info.VolumeSerialNumber, info.FileId.Identifier),
        Err(_) => {
            // FAT/exFAT: 32-bit serial and 64-bit index, same fallback as the scanner.
            // SAFETY: plain-data structure; zero is valid.
            let mut basic: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
            // SAFETY: valid handle and out-pointer.
            if unsafe { GetFileInformationByHandle(h.0, &mut basic) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let index = (u64::from(basic.nFileIndexHigh) << 32) | u64::from(basic.nFileIndexLow);
            let mut id = [0u8; 16];
            id[..8].copy_from_slice(&index.to_le_bytes());
            (u64::from(basic.dwVolumeSerialNumber), id)
        }
    };
    let tag: FILE_ATTRIBUTE_TAG_INFO = query(h, FileAttributeTagInfo)?;
    let std_info: FILE_STANDARD_INFO = query(h, FileStandardInfo)?;
    let basic: FILE_BASIC_INFO = query(h, FileBasicInfo)?;
    Ok(Live {
        volume_id: format!("{}|{serial:016x}", volume_guid(h)),
        file_id: format!("fileid128:{}", hex(&id)),
        attributes: tag.FileAttributes,
        size: std_info.EndOfFile.max(0) as u64,
        last_write: basic.LastWriteTime,
        links: std_info.NumberOfLinks,
        directory: std_info.Directory || tag.FileAttributes & ATTR_DIRECTORY != 0,
    })
}

/// Volume IDs are `{volume_guid}|{serial:016x}`; the GUID part may be empty when the volume has
/// no GUID name, in which case the serial alone decides.
fn same_volume(a: &str, b: &str) -> bool {
    let split = |s: &str| s.rsplit_once('|').map(|(g, s)| (g.to_owned(), s.to_owned())).unwrap_or((String::new(), s.to_owned()));
    let (ga, sa) = split(a);
    let (gb, sb) = split(b);
    sa == sb && (ga == gb || ga.is_empty() || gb.is_empty())
}

fn final_path(h: &Owned) -> Option<String> {
    let mut buf = vec![0u16; 32_768];
    // SAFETY: buffer length is passed in characters.
    let n = unsafe { GetFinalPathNameByHandleW(h.0, buf.as_mut_ptr(), buf.len() as u32, VOLUME_NAME_DOS) } as usize;
    (n > 0 && n < buf.len()).then(|| String::from_utf16_lossy(&buf[..n]))
}

// ---------------------------------------------------------------------------------------------
// Paths

fn units(p: &RawPath) -> Option<&[u16]> {
    match p {
        RawPath::Windows(u) => Some(u),
        RawPath::Unix(_) => None,
    }
}

fn wide(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().collect()
}

fn os_path(u: &[u16]) -> PathBuf {
    use std::os::windows::ffi::OsStringExt;
    PathBuf::from(std::ffi::OsString::from_wide(u))
}

const BS: u16 = b'\\' as u16;

/// Length of the root (`\\?\C:\`, `\\?\UNC\server\share\`, `\\?\Volume{..}\`) of a verbatim path.
fn root_len(v: &[u16]) -> usize {
    let prefix = 4; // \\?\
    let rest = &v[prefix.min(v.len())..];
    let next = |from: usize| v[from..].iter().position(|&c| c == BS).map(|i| from + i + 1);
    if rest.len() >= 4 && String::from_utf16_lossy(&rest[..4]).eq_ignore_ascii_case("UNC\\") {
        next(prefix + 4).and_then(next).unwrap_or(v.len())
    } else {
        next(prefix).unwrap_or(v.len())
    }
}

/// Split a verbatim path into its parent directory and last component.
fn split_parent(v: &[u16]) -> Option<(Vec<u16>, Vec<u16>)> {
    let mut end = v.len();
    while end > 0 && v[end - 1] == BS {
        end -= 1;
    }
    let root = root_len(v);
    if end <= root {
        return None;
    }
    let sep = v[..end].iter().rposition(|&c| c == BS)?;
    let name = v[sep + 1..end].to_vec();
    let parent = if sep < root { v[..root].to_vec() } else { v[..sep].to_vec() };
    Some((parent, name))
}

fn verbatim(p: &RawPath) -> Result<Vec<u16>, Stop> {
    let u = units(p).ok_or_else(|| Stop::Unsupported("not a Windows path".into()))?;
    full_path(u).map(|(_, v)| v).map_err(|e| Stop::Error(format!("cannot resolve path: {e}")))
}

fn volume_root(path: &[u16]) -> Option<Vec<u16>> {
    let z = nul(path);
    let mut buf = vec![0u16; 32_768];
    // SAFETY: NUL-terminated input; output length bounded by the buffer.
    if unsafe { GetVolumePathNameW(z.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } == 0 {
        return None;
    }
    let n = buf.iter().position(|&c| c == 0)?;
    buf.truncate(n);
    (!buf.is_empty()).then_some(buf)
}

/// A single path component safe to join under a directory.
fn valid_component(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.chars().any(|c| matches!(c, '\\' | '/' | ':' | '\0' | '*' | '?' | '"' | '<' | '>' | '|'))
}

// ---------------------------------------------------------------------------------------------
// Verification

#[derive(Debug, Clone, PartialEq, Eq)]
enum Stop {
    Missing,
    Reparse,
    Identity,
    Parent,
    NotFile,
    HardLinks(u32),
    Metadata(String),
    InUse,
    Denied(String),
    Unsupported(String),
    Error(String),
}

impl Stop {
    fn from_open(e: &io::Error) -> Stop {
        match e.raw_os_error() {
            Some(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) => Stop::Missing,
            Some(ERROR_SHARING_VIOLATION) => Stop::InUse,
            Some(ERROR_ACCESS_DENIED) => Stop::Denied(format!("access denied: {e}")),
            _ => Stop::Error(format!("cannot open: {e}")),
        }
    }

    fn check(self) -> LiveCheck {
        match self {
            Stop::Missing => LiveCheck::Missing,
            Stop::Reparse => LiveCheck::LinkOrReparse,
            Stop::Identity => LiveCheck::IdentityChanged,
            Stop::Parent => LiveCheck::ParentChanged,
            Stop::NotFile => LiveCheck::MetadataChanged("kind: no longer a regular file".into()),
            Stop::HardLinks(n) => LiveCheck::Unsupported(format!("file has {n} hard links")),
            Stop::Metadata(m) => LiveCheck::MetadataChanged(m),
            Stop::InUse => LiveCheck::InUse,
            Stop::Denied(m) => LiveCheck::Protected(m),
            Stop::Unsupported(m) => LiveCheck::Unsupported(m),
            Stop::Error(m) => LiveCheck::Unknown(m),
        }
    }

    /// Every stop happens before any change, so it is a skip (precondition) or a failure.
    fn outcome(self) -> ActionOutcome {
        let skip = |code: &str, message: String| ActionOutcome::Skipped { code: code.into(), message };
        match self {
            Stop::Missing => skip("missing", "the planned file no longer exists".into()),
            Stop::Reparse => skip("link_or_reparse", "a link or reparse point is at the planned path; not followed".into()),
            Stop::Identity => skip("identity_changed", "the path names a different file than the plan".into()),
            Stop::Parent => skip("parent_changed", "a planned parent directory was replaced or moved".into()),
            Stop::NotFile => skip("metadata_changed", "the planned path is no longer a regular file".into()),
            Stop::HardLinks(n) => skip("unsupported", format!("file has {n} hard links")),
            Stop::Metadata(m) => skip("metadata_changed", m),
            Stop::InUse => skip("in_use", "the file or a planned parent is in use".into()),
            Stop::Unsupported(m) => skip("unsupported", m),
            Stop::Denied(m) => ActionOutcome::Failed { code: "access_denied".into(), message: m },
            Stop::Error(m) => ActionOutcome::Failed { code: "check_failed".into(), message: m },
        }
    }
}

/// Size/time drift against the planned observation (fields the plan did not record are skipped).
fn metadata_change(obs: &ObservationRecord, l: &Live) -> Option<String> {
    if let Some(b) = obs.logical_bytes {
        if b.0 != l.size {
            return Some(format!("logical_bytes changed: planned {}, now {}", b.0, l.size));
        }
    }
    let m = obs.modified.as_ref()?;
    let Ok(planned) = m.value.parse::<i128>() else { return Some("planned modified time is unreadable".into()) };
    let ticks = i128::from(l.last_write);
    let same = match m.unit {
        TimeUnit::WindowsFiletime100ns => planned == ticks,
        TimeUnit::UnixNanoseconds => {
            let now_ns = (ticks - UNIX_EPOCH_FILETIME) * 100;
            if planned % 1_000_000_000 == 0 {
                // Observed with whole-second precision (index `modified_unix`).
                planned.div_euclid(1_000_000_000) == now_ns.div_euclid(1_000_000_000)
            } else {
                planned == now_ns
            }
        }
    };
    (!same).then(|| "modified time changed".into())
}

/// `directory`: the planned object is a directory (kind must match; its size and time are not
/// compared because removing its planned contents changes them).
fn verify_live_as(t: &ActionTarget<'_>, l: &Live, directory: bool) -> Result<(), Stop> {
    if l.reparse() {
        return Err(Stop::Reparse);
    }
    if !l.is(&t.identity.volume_id, &t.identity.file_id) {
        return Err(Stop::Identity);
    }
    if directory {
        return if l.directory { Ok(()) } else { Err(Stop::Metadata("kind: no longer a directory".into())) };
    }
    if l.directory {
        return Err(Stop::NotFile);
    }
    if l.links > 1 {
        return Err(Stop::HardLinks(l.links));
    }
    match metadata_change(t.observation, l) {
        Some(m) => Err(Stop::Metadata(m)),
        None => Ok(()),
    }
}

/// The verified target plus pinned, verified ancestors (held until dropped).
struct Verified {
    target: Owned,
    live: Live,
    verbatim: Vec<u16>,
    _pins: Vec<Owned>,
}

/// Pin and verify the planned ancestors, then open and verify the target with `access`/`share`.
fn verify(t: &ActionTarget<'_>, access: u32, share: u32, flags: u32) -> Result<Verified, Stop> {
    verify_as(t, access, share, flags, false)
}

fn verify_as(t: &ActionTarget<'_>, access: u32, share: u32, flags: u32, directory: bool) -> Result<Verified, Stop> {
    let v = verbatim(t.path)?;
    let mut ancestors = Vec::with_capacity(t.parent_chain.len());
    let mut cur = v.clone();
    for _ in t.parent_chain {
        let (parent, _) = split_parent(&cur).ok_or(Stop::Parent)?;
        ancestors.push(parent.clone());
        cur = parent;
    }
    let mut pins = Vec::with_capacity(ancestors.len());
    // Top-down: each pinned directory can no longer be renamed, so the next lookup goes through it.
    for (path, expected) in ancestors.iter().zip(t.parent_chain).rev() {
        let h = open(path, FILE_READ_ATTRIBUTES | SYNCHRONIZE, SHARE_PIN, FLAG_BACKUP_SEMANTICS).map_err(|e| Stop::from_open(&e))?;
        let l = live(&h).map_err(|e| Stop::Error(format!("cannot query parent: {e}")))?;
        if !l.is_identity(expected) || !l.directory {
            return Err(Stop::Parent);
        }
        pins.push(h);
    }
    let target = match open(&v, access, share, flags) {
        Ok(h) => h,
        Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) => {
            // Directories (a swapped-in junction, say) refuse write opens without backup
            // semantics; classify with an attribute-only open before reporting a denial.
            let l = open_attrs(&v).and_then(|h| live(&h)).map_err(|_| Stop::from_open(&e))?;
            verify_live_as(t, &l, directory)?;
            return Err(Stop::from_open(&e));
        }
        Err(e) => return Err(Stop::from_open(&e)),
    };
    let l = live(&target).map_err(|e| Stop::Error(format!("cannot query target: {e}")))?;
    verify_live_as(t, &l, directory)?;
    Ok(Verified { target, live: l, verbatim: v, _pins: pins })
}

/// What sits at a path now, relative to the planned identity (no pins, no content access).
#[derive(Debug, PartialEq, Eq)]
enum AtPath {
    Planned,
    Other,
    Missing,
    Error(String),
}

fn at_path(path: &[u16], volume_id: &str, file_id: &str) -> AtPath {
    let v = match full_path(path) {
        Ok((_, v)) => v,
        Err(e) => return AtPath::Error(e.to_string()),
    };
    match open_attrs(&v).and_then(|h| live(&h)) {
        Ok(l) if l.is(volume_id, file_id) => AtPath::Planned,
        Ok(_) => AtPath::Other,
        Err(e) if matches!(e.raw_os_error(), Some(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND)) => AtPath::Missing,
        Err(e) => AtPath::Error(e.to_string()),
    }
}

// ---------------------------------------------------------------------------------------------
// Owner process probe

fn owner_executable(owner: &str) -> Option<&'static str> {
    match owner {
        "microsoft.edge" => Some("msedge.exe"),
        "google.chrome" => Some("chrome.exe"),
        _ => None,
    }
}

fn process_running(exe: &str) -> io::Result<bool> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    // SAFETY: process snapshot of the whole system; no process ID needed.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let snap = Owned(snap);
    // SAFETY: plain-data structure; zero is valid; dwSize set as the API requires.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: valid snapshot handle and entry buffer.
    let mut more = unsafe { Process32FirstW(snap.0, &mut entry) } != 0;
    while more {
        let n = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
        if String::from_utf16_lossy(&entry.szExeFile[..n]).eq_ignore_ascii_case(exe) {
            return Ok(true);
        }
        // SAFETY: as above.
        more = unsafe { Process32NextW(snap.0, &mut entry) } != 0;
    }
    Ok(false)
}

fn owner_check(t: &ActionTarget<'_>) -> Option<LiveCheck> {
    if !t.probes.iter().any(|p| p == "owner_process") {
        return None;
    }
    let Some(owner) = t.owner_id else { return Some(LiveCheck::Unknown("owner_process probe without an owner".into())) };
    // Owners outside this table are mapped by the catalog probes (unmapped stays Unknown there).
    let Some(exe) = owner_executable(owner) else { return super::probes::owner_running_check(owner) };
    match process_running(exe) {
        Ok(true) => Some(LiveCheck::OwnerRunning),
        Ok(false) => None,
        Err(e) => Some(LiveCheck::Unknown(format!("process probe failed: {e}"))),
    }
}

// ---------------------------------------------------------------------------------------------
// Handle operations

fn privileges_dropped() -> Result<(), ActionOutcome> {
    // The executor must never act with an ACL bypass (SeBackup/SeRestore) enabled.
    super::privileges::disable_bypass_privileges()
        .map_err(|e| ActionOutcome::Failed { code: "privilege_state".into(), message: format!("cannot disable backup/restore privileges: {e}") })
}

fn unsupported_class(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(ERROR_INVALID_PARAMETER | ERROR_NOT_SUPPORTED | ERROR_INVALID_FUNCTION | ERROR_INVALID_LEVEL))
}

/// Mark the handle's file for deletion (POSIX semantics where supported).
fn dispose(h: &Owned) -> io::Result<()> {
    let ex = FILE_DISPOSITION_INFO_EX { Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS };
    // SAFETY: buffer is a FILE_DISPOSITION_INFO_EX of the declared size.
    if unsafe { SetFileInformationByHandle(h.0, FileDispositionInfoEx, (&ex as *const FILE_DISPOSITION_INFO_EX).cast(), size_of::<FILE_DISPOSITION_INFO_EX>() as u32) } != 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    if !unsupported_class(&e) {
        return Err(e);
    }
    let legacy = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: buffer is a FILE_DISPOSITION_INFO of the declared size.
    if unsafe { SetFileInformationByHandle(h.0, FileDispositionInfo, (&legacy as *const FILE_DISPOSITION_INFO).cast(), size_of::<FILE_DISPOSITION_INFO>() as u32) } != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Rename the handle's file to `dest` (full verbatim path), never replacing an existing name.
fn rename_no_replace(h: &Owned, dest: &[u16]) -> io::Result<()> {
    let name_bytes = dest.len() * 2;
    let size = std::mem::offset_of!(FILE_RENAME_INFO, FileName) + name_bytes + 2;
    let mut buf = vec![0u64; size.div_ceil(8)];
    let info = buf.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: `buf` is 8-byte aligned, zeroed and large enough for the header plus the name and NUL.
    unsafe {
        (*info).Anonymous.Flags = 0; // Ex: no FILE_RENAME_FLAG_REPLACE_IF_EXISTS; legacy: ReplaceIfExists = false.
        (*info).RootDirectory = std::ptr::null_mut();
        (*info).FileNameLength = name_bytes as u32;
        std::ptr::copy_nonoverlapping(dest.as_ptr(), std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(), dest.len());
    }
    for class in [FileRenameInfoEx, FileRenameInfo] {
        // SAFETY: `info` points to `size` initialized bytes laid out as FILE_RENAME_INFO.
        if unsafe { SetFileInformationByHandle(h.0, class, info.cast_const().cast(), size as u32) } != 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if class == FileRenameInfo || !unsupported_class(&e) {
            return Err(e);
        }
    }
    unreachable!("loop returns on the legacy class")
}

// ---------------------------------------------------------------------------------------------
// Recycle bin

fn recycle_bin_available(display: &[u16]) -> Result<(), String> {
    let root = volume_root(display).ok_or("cannot determine the volume root")?;
    recycle_bin_on(&root)
}

/// Where Explorer keeps the current user's per-volume Recycle Bin settings (`Volume\{GUID}\NukeOnDelete`).
const BITBUCKET_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket";
/// Group Policy "Do not move deleted files to the Recycle Bin" (user or machine).
const RECYCLE_POLICY_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer";
/// Test seam: a key under `HKCU\Software\CleanManagerTest\` read *in addition to* the real BitBucket settings, so it can
/// only turn a volume's bin off, never on.
const BITBUCKET_TEST_ENV: &str = "CM_TEST_BITBUCKET_ROOT";

fn reg_dword(hive: windows_sys::Win32::System::Registry::HKEY, sub: &str, name: &str) -> Option<u32> {
    use windows_sys::Win32::System::Registry::{RRF_RT_REG_DWORD, RegGetValueW};
    let sub: Vec<u16> = sub.encode_utf16().chain([0]).collect();
    let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let (mut v, mut size) = (0u32, 4u32);
    // SAFETY: NUL-terminated key and value names; four-byte buffer for a REG_DWORD; read-only query.
    let rc = unsafe { RegGetValueW(hive, sub.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&mut v as *mut u32).cast(), &mut size) };
    (rc == 0).then_some(v)
}

/// `{GUID}` of the volume mounted at `root` (as Explorer names its BitBucket subkey).
fn volume_guid_of_root(root: &[u16]) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::GetVolumeNameForVolumeMountPointW;
    let mut r = root.to_vec();
    if r.last() != Some(&BS) {
        r.push(BS);
    }
    let z = nul(&r);
    let mut buf = [0u16; 64];
    // SAFETY: NUL-terminated mount point with a trailing backslash; buffer length in characters.
    if unsafe { GetVolumeNameForVolumeMountPointW(z.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } == 0 {
        return None;
    }
    let s = String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(buf.len())]);
    let (start, end) = (s.find('{')?, s.find('}')?);
    (start < end).then(|| s[start..=end].to_owned())
}

/// Whether the current user turned the bin off for this volume ("Don't move files to the Recycle Bin"), or a policy
/// turned it off everywhere. Unreadable settings are not a refusal: the Shell call below and the progress-sink guard at
/// execution still stop a delete that would not go to a bin.
fn recycle_bin_turned_off(root: &[u16]) -> Option<String> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    if [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE].into_iter().any(|h| reg_dword(h, RECYCLE_POLICY_KEY, "NoRecycleFiles").is_some_and(|v| v != 0)) {
        return Some("the Recycle Bin is turned off by policy (\"Do not move deleted files to the Recycle Bin\")".into());
    }
    let guid = volume_guid_of_root(root)?;
    let test = std::env::var(BITBUCKET_TEST_ENV).ok().filter(|r| super::startup::StartupEnv::checked_root(r).is_ok());
    let nuke = std::iter::once(BITBUCKET_KEY.to_owned()).chain(test).any(|base| reg_dword(HKEY_CURRENT_USER, &format!(r"{base}\Volume\{guid}"), "NukeOnDelete").is_some_and(|v| v != 0));
    nuke.then(|| "the Recycle Bin is turned off for this volume (its Recycle Bin properties say \"Don't move files to the Recycle Bin. Remove files immediately when deleted.\")".into())
}

/// Readiness of the bin of the volume mounted at `root` (from `GetVolumePathNameW`).
fn recycle_bin_on(root: &[u16]) -> Result<(), String> {
    let z = nul(root);
    // SAFETY: NUL-terminated root path.
    if unsafe { GetDriveTypeW(z.as_ptr()) } == DRIVE_REMOTE {
        return Err("network volumes have no recycle bin".into());
    }
    if let Some(why) = recycle_bin_turned_off(root) {
        return Err(why);
    }
    let mut info = windows::Win32::UI::Shell::SHQUERYRBINFO { cbSize: size_of::<windows::Win32::UI::Shell::SHQUERYRBINFO>() as u32, ..Default::default() };
    // SAFETY: NUL-terminated root and a sized SHQUERYRBINFO.
    unsafe { windows::Win32::UI::Shell::SHQueryRecycleBinW(windows::core::PCWSTR(z.as_ptr()), &mut info) }
        .map_err(|e| format!("volume has no usable recycle bin: {e}"))
}

pub(crate) mod shell {
    //! Shell `IFileOperation` recycle with a fail-closed progress sink. It runs on its own STA
    //! thread so the caller's COM apartment is never changed.

    use std::cell::RefCell;
    use std::rc::Rc;

    use windows::Win32::Foundation::E_ABORT;
    use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize};
    use windows::Win32::UI::Shell::{
        FOF_ALLOWUNDO, FOF_NO_UI, FOFX_EARLYFAILURE, FOFX_RECYCLEONDELETE, FileOperation, IFileOperation, IFileOperationProgressSink,
        IFileOperationProgressSink_Impl, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH, TSF_DELETE_RECYCLE_IF_POSSIBLE,
    };
    use windows::core::{HRESULT, PCWSTR, Ref, Result as WinResult, implement};

    #[derive(Debug)]
    pub struct ShellOutcome {
        /// `PerformOperations` result.
        pub performed: Result<(), String>,
        pub aborted: bool,
        /// Why the guard refused the delete, if it did.
        pub refused: Option<String>,
    }

    #[implement(IFileOperationProgressSink)]
    struct Guard {
        volume_id: String,
        file_id: String,
        /// The planned item is a directory (whole-tree recycle) rather than a file.
        directory: bool,
        refused: Rc<RefCell<Option<String>>>,
    }

    impl Guard {
        fn refuse(&self, why: String) -> WinResult<()> {
            *self.refused.borrow_mut() = Some(why);
            Err(E_ABORT.into())
        }
    }

    impl IFileOperationProgressSink_Impl for Guard_Impl {
        fn PreDeleteItem(&self, dwflags: u32, psiitem: Ref<IShellItem>) -> WinResult<()> {
            // Without TSF_DELETE_RECYCLE_IF_POSSIBLE the engine would delete permanently
            // (too large for the bin, no bin on the volume, bin disabled): abort instead.
            if dwflags & TSF_DELETE_RECYCLE_IF_POSSIBLE.0 as u32 == 0 {
                return self.refuse("the item cannot be recycled and would be permanently deleted".into());
            }
            let Ok(item) = psiitem.ok() else { return self.refuse("no item".into()) };
            // SAFETY: valid shell item; the returned string is freed with CoTaskMemFree.
            let path = match unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) } {
                Ok(p) => {
                    // SAFETY: the shell returned a NUL-terminated string it allocated.
                    let v = unsafe { p.as_wide() }.to_vec();
                    // SAFETY: allocated by the shell with CoTaskMemAlloc.
                    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
                    v
                }
                Err(e) => return self.refuse(format!("cannot read item path: {e}")),
            };
            // Last identity check before the shell's own (path-based) open.
            match super::at_path(&path, &self.volume_id, &self.file_id) {
                super::AtPath::Planned => {}
                other => {
                    let what = match other {
                        super::AtPath::Other => "another file is there now".to_owned(),
                        super::AtPath::Missing => "it is gone".to_owned(),
                        super::AtPath::Error(e) => format!("it could not be checked: {e}"),
                        super::AtPath::Planned => unreachable!("matched above"),
                    };
                    return self.refuse(format!("item is no longer the planned file ({what})"));
                }
            }
            match super::full_path(&path).and_then(|(_, v)| super::open_attrs(&v)).and_then(|h| super::live(&h)) {
                Ok(l) if !l.reparse() && l.directory == self.directory => Ok(()),
                Ok(_) => self.refuse("item is a link, reparse point or directory".into()),
                Err(e) => self.refuse(format!("cannot re-verify item: {e}")),
            }
        }
        fn StartOperations(&self) -> WinResult<()> {
            Ok(())
        }
        fn FinishOperations(&self, _: HRESULT) -> WinResult<()> {
            Ok(())
        }
        fn PreRenameItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> WinResult<()> {
            Err(E_ABORT.into())
        }
        fn PostRenameItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR, _: HRESULT, _: Ref<IShellItem>) -> WinResult<()> {
            Ok(())
        }
        fn PreMoveItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR) -> WinResult<()> {
            Err(E_ABORT.into())
        }
        fn PostMoveItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR, _: HRESULT, _: Ref<IShellItem>) -> WinResult<()> {
            Ok(())
        }
        fn PreCopyItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR) -> WinResult<()> {
            Err(E_ABORT.into())
        }
        fn PostCopyItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR, _: HRESULT, _: Ref<IShellItem>) -> WinResult<()> {
            Ok(())
        }
        fn PostDeleteItem(&self, _: u32, _: Ref<IShellItem>, _: HRESULT, _: Ref<IShellItem>) -> WinResult<()> {
            Ok(())
        }
        fn PreNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> WinResult<()> {
            Err(E_ABORT.into())
        }
        fn PostNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR, _: &PCWSTR, _: u32, _: HRESULT, _: Ref<IShellItem>) -> WinResult<()> {
            Ok(())
        }
        fn UpdateProgress(&self, _: u32, _: u32) -> WinResult<()> {
            Ok(())
        }
        fn ResetTimer(&self) -> WinResult<()> {
            Ok(())
        }
        fn PauseTimer(&self) -> WinResult<()> {
            Ok(())
        }
        fn ResumeTimer(&self) -> WinResult<()> {
            Ok(())
        }
    }

    fn run(path: &[u16], volume_id: String, file_id: String, directory: bool) -> ShellOutcome {
        let refused = Rc::new(RefCell::new(None));
        let guard = Guard { volume_id, file_id, directory, refused: Rc::clone(&refused) };
        let sink: IFileOperationProgressSink = guard.into();
        let mut z = path.to_vec();
        z.push(0);
        let setup = || -> WinResult<IFileOperation> {
            // SAFETY: COM is initialized on this thread; arguments are valid for each call.
            unsafe {
                let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;
                op.SetOperationFlags(FOF_ALLOWUNDO | FOF_NO_UI | FOFX_RECYCLEONDELETE | FOFX_EARLYFAILURE)?;
                let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(z.as_ptr()), None)?;
                op.DeleteItem(&item, &sink)?;
                Ok(op)
            }
        };
        let op = match setup() {
            Ok(op) => op,
            Err(e) => return ShellOutcome { performed: Err(format!("cannot prepare the recycle operation: {e}")), aborted: true, refused: None },
        };
        // The same sink also watches the whole operation, so every delete passes the guard.
        // SAFETY: valid operation and sink.
        let cookie = unsafe { op.Advise(&sink) }.ok();
        // SAFETY: valid, fully configured operation.
        let performed = unsafe { op.PerformOperations() }.map_err(|e| e.to_string());
        // SAFETY: valid operation.
        let aborted = unsafe { op.GetAnyOperationsAborted() }.map(|b| b.as_bool()).unwrap_or(true);
        if let Some(c) = cookie {
            // SAFETY: cookie returned by Advise on this operation.
            let _ = unsafe { op.Unadvise(c) };
        }
        drop(op);
        drop(sink);
        let refused = refused.borrow().clone();
        ShellOutcome { performed, aborted, refused }
    }

    /// Recycle `path` (display form) if the item at it still has the planned identity.
    pub fn recycle(path: Vec<u16>, volume_id: String, file_id: String) -> ShellOutcome {
        recycle_item(path, volume_id, file_id, false)
    }

    /// [`recycle`] for a planned file (`directory == false`) or a whole planned directory.
    pub fn recycle_item(path: Vec<u16>, volume_id: String, file_id: String, directory: bool) -> ShellOutcome {
        let worker = std::thread::spawn(move || {
            // SAFETY: fresh thread; balanced with CoUninitialize below when it succeeded.
            let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
            if hr.is_err() {
                return ShellOutcome { performed: Err(format!("COM initialization failed (HRESULT 0x{:08X})", hr.0 as u32)), aborted: true, refused: None };
            }
            let out = run(&path, volume_id, file_id, directory);
            // SAFETY: matches the successful CoInitializeEx on this thread.
            unsafe { CoUninitialize() };
            out
        });
        worker.join().unwrap_or_else(|_| ShellOutcome { performed: Err("recycle worker panicked".into()), aborted: true, refused: None })
    }
}

// ---------------------------------------------------------------------------------------------
// FileActions

impl FileActions for WindowsActions {
    fn check(&self, t: &ActionTarget<'_>) -> LiveCheck {
        let v = match verify(t, FILE_READ_ATTRIBUTES | SYNCHRONIZE, SHARE_ALL, FLAG_BACKUP_SEMANTICS) {
            Ok(v) => v,
            Err(stop) => return stop.check(),
        };
        let Verified { target, verbatim, _pins: pins, .. } = v;
        drop(target);
        // In-use probe: an exclusive open (no sharing) fails if any other handle is open. It
        // requests DELETE only to exercise delete sharing; nothing is changed or read.
        match open(&verbatim, FILE_READ_ATTRIBUTES | DELETE | SYNCHRONIZE, 0, 0) {
            Ok(h) => match live(&h) {
                Ok(l) if l.is(&t.identity.volume_id, &t.identity.file_id) => {}
                Ok(_) => return LiveCheck::IdentityChanged,
                Err(e) => return LiveCheck::Unknown(format!("cannot query target: {e}")),
            },
            Err(e) => match e.raw_os_error() {
                Some(ERROR_SHARING_VIOLATION) => return LiveCheck::InUse,
                Some(ERROR_ACCESS_DENIED) => return LiveCheck::Protected(format!("delete access denied: {e}")),
                Some(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) => return LiveCheck::Missing,
                _ => return LiveCheck::Unknown(format!("in-use probe failed: {e}")),
            },
        }
        drop(pins);
        // Catalog probe kinds (endpoint-security path, held download markers): `probes::execution_check`.
        owner_check(t).or_else(|| super::probes::execution_check(t)).unwrap_or(LiveCheck::Match)
    }

    fn quarantine(&self, t: &ActionTarget<'_>, quarantine_dir: &RawPath, name: &str) -> ActionOutcome {
        let failed = |code: &str, message: String| ActionOutcome::Failed { code: code.into(), message };
        if let Err(o) = privileges_dropped() {
            return o;
        }
        if !valid_component(name) {
            return failed("invalid_name", "quarantine name must be a single path component".into());
        }
        let Some(qunits) = units(quarantine_dir) else { return failed("unsupported", "not a Windows path".into()) };
        let qv = match full_path(qunits) {
            Ok((_, v)) => v,
            Err(e) => return failed("quarantine_unavailable", format!("cannot resolve quarantine directory: {e}")),
        };
        if let Err(e) = std::fs::create_dir_all(os_path(&qv)) {
            return failed("quarantine_unavailable", format!("cannot create quarantine directory: {e}"));
        }
        // Pin the quarantine directory so the destination cannot be swapped during the rename.
        let qdir = match open(&qv, FILE_READ_ATTRIBUTES | SYNCHRONIZE, SHARE_PIN, FLAG_BACKUP_SEMANTICS) {
            Ok(h) => h,
            Err(e) => return failed("quarantine_unavailable", format!("cannot open quarantine directory: {e}")),
        };
        let qlive = match live(&qdir) {
            Ok(l) => l,
            Err(e) => return failed("quarantine_unavailable", format!("cannot query quarantine directory: {e}")),
        };
        if qlive.reparse() || !qlive.directory {
            return failed("quarantine_unsafe", "quarantine path is a link, reparse point or not a directory".into());
        }
        let v = match verify(t, DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE, SHARE_PIN, 0) {
            Ok(v) => v,
            Err(stop) => return stop.outcome(),
        };
        if !same_volume(&qlive.volume_id, &v.live.volume_id) {
            return failed("cross_volume", "quarantine directory is on another volume; refusing copy-and-delete".into());
        }
        let mut dest = qv.clone();
        if dest.last() != Some(&BS) {
            dest.push(BS);
        }
        dest.extend(name.encode_utf16());
        match rename_no_replace(&v.target, &dest) {
            Ok(()) => {
                let location = qunits.to_vec();
                let location = RawPath::windows(location).and_then(|q| q.join(&RawPath::windows(name.encode_utf16().collect())?));
                match location {
                    Ok(location) => ActionOutcome::Succeeded { recovery: Recovery::Quarantine { location } },
                    Err(e) => ActionOutcome::Unknown { message: format!("moved, but the location cannot be recorded: {e}") },
                }
            }
            Err(e) => match e.raw_os_error() {
                Some(ERROR_FILE_EXISTS | ERROR_ALREADY_EXISTS) => failed("destination_exists", "a file with that name is already in quarantine; nothing replaced".into()),
                Some(ERROR_NOT_SAME_DEVICE) => failed("cross_volume", format!("rename crosses volumes: {e}")),
                Some(ERROR_ACCESS_DENIED) => failed("access_denied", format!("rename refused: {e}")),
                Some(ERROR_SHARING_VIOLATION) => failed("in_use", format!("rename refused: {e}")),
                _ => failed("rename_failed", format!("rename refused: {e}")),
            },
        }
    }

    fn trash(&self, t: &ActionTarget<'_>) -> ActionOutcome {
        let unavailable = |message: String| ActionOutcome::Failed { code: "trash_unavailable".into(), message };
        let Some(u) = units(t.path) else { return ActionOutcome::Failed { code: "unsupported".into(), message: "not a Windows path".into() } };
        let display = match full_path(u) {
            Ok((d, _)) => d,
            Err(e) => return ActionOutcome::Failed { code: "check_failed".into(), message: format!("cannot resolve path: {e}") },
        };
        if let Err(o) = privileges_dropped() {
            return o;
        }
        if let Err(why) = recycle_bin_available(&display) {
            return unavailable(why);
        }
        // Recycling is a rename, which needs DELETE on the file (or FILE_DELETE_CHILD on its
        // parent, which NTFS folds into the same open check). Probe it so a refusal is reported
        // as such instead of as an unavailable bin; the probe handle is closed before the Shell runs.
        if let Err(stop) = verify(t, DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE, SHARE_ALL, 0) {
            return stop.outcome();
        }
        // Verify with share-all so the Shell can move the file; ancestors stay pinned and the
        // verified handle is kept to confirm where the object ended up.
        let v = match verify(t, FILE_READ_ATTRIBUTES | SYNCHRONIZE, SHARE_ALL, 0) {
            Ok(v) => v,
            Err(stop) => return stop.outcome(),
        };
        // RESIDUAL RACE: IFileOperation is path based. Identity is re-checked in PreDeleteItem,
        // yet the name could still be swapped before the Shell opens it. Accepted only because
        // a recycled object is recoverable; the outcome is confirmed below via `v.target`.
        #[cfg(test)]
        tests::before_shell();
        let shell = shell::recycle(display, t.identity.volume_id.clone(), t.identity.file_id.clone());
        let now = at_path(&v.verbatim, &t.identity.volume_id, &t.identity.file_id);
        let in_bin = final_path(&v.target).is_some_and(|p| p.to_ascii_lowercase().contains("\\$recycle.bin\\"));
        let reported_ok = shell.performed.is_ok() && !shell.aborted;
        match now {
            AtPath::Planned if reported_ok => ActionOutcome::Unknown { message: "the Shell reported success but the file is still at its path".into() },
            AtPath::Planned => {
                let why = shell.refused.or(shell.performed.err()).unwrap_or_else(|| "operation aborted".into());
                unavailable(format!("not moved to the recycle bin: {why}"))
            }
            AtPath::Missing | AtPath::Other if in_bin => ActionOutcome::Succeeded { recovery: Recovery::Trash },
            AtPath::Missing | AtPath::Other => ActionOutcome::Unknown { message: "the file left its path but was not confirmed in the recycle bin".into() },
            AtPath::Error(e) => ActionOutcome::Unknown { message: format!("cannot confirm the recycle result: {e}") },
        }
    }

    fn delete(&self, t: &ActionTarget<'_>) -> ActionOutcome {
        if let Err(o) = privileges_dropped() {
            return o;
        }
        let v = match verify(t, DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE, SHARE_PIN, 0) {
            Ok(v) => v,
            Err(stop) => return stop.outcome(),
        };
        if let Err(e) = dispose(&v.target) {
            let code = match e.raw_os_error() {
                Some(ERROR_ACCESS_DENIED) => "access_denied",
                Some(ERROR_SHARING_VIOLATION) => "in_use",
                _ => "delete_failed",
            };
            return ActionOutcome::Failed { code: code.into(), message: format!("delete refused: {e}") };
        }
        let path = v.verbatim.clone();
        drop(v); // closing the handle commits the delete
        match at_path(&path, &t.identity.volume_id, &t.identity.file_id) {
            AtPath::Missing | AtPath::Other => ActionOutcome::Succeeded { recovery: Recovery::Unavailable },
            AtPath::Planned => ActionOutcome::Unknown { message: "file is still present after delete (other handles may keep it pending)".into() },
            AtPath::Error(e) => ActionOutcome::Unknown { message: format!("delete pending or unconfirmed: {e}") },
        }
    }

    fn quarantine_dir_for(&self, t: &ActionTarget<'_>, operation_id: &str) -> Option<RawPath> {
        if !valid_component(operation_id) {
            return None;
        }
        let target_volume = &t.identity.volume_id;
        if let Some(store) = self.store_dir.clone().or_else(crate::paths::default_store_dir) {
            // The store may not exist yet: its nearest existing ancestor decides the volume.
            let probe = store.ancestors().find(|p| p.exists());
            let on_volume = probe.and_then(|p| super::directory::identity_of(&wide(p)).ok()).is_some_and(|(vol, _)| same_volume(&vol, target_volume));
            if on_volume {
                return RawPath::windows(wide(&store.join("quarantine").join(operation_id))).ok();
            }
        }
        let root = volume_root(units(t.path)?)?;
        let dir = os_path(&root).join(".cleanmanager-quarantine").join(operation_id);
        std::fs::create_dir_all(&dir).ok()?;
        let (vol, _) = super::directory::identity_of(&wide(&dir)).ok()?;
        if !same_volume(&vol, target_volume) {
            return None;
        }
        RawPath::windows(wide(&dir)).ok()
    }

    fn reconcile(&self, t: &ActionTarget<'_>, mode: Mode, quarantine_location: Option<&RawPath>) -> Reconciled {
        let Some(u) = units(t.path) else { return Reconciled::StillUnknown };
        let (vol, fid) = (&t.identity.volume_id, &t.identity.file_id);
        let original = at_path(u, vol, fid);
        match mode {
            Mode::Delete => match original {
                AtPath::Planned => Reconciled::NotPerformed,
                AtPath::Missing => Reconciled::Performed,
                // A different file at the name, or an unreadable path, proves nothing.
                AtPath::Other | AtPath::Error(_) => Reconciled::StillUnknown,
            },
            Mode::Quarantine => {
                let moved = quarantine_location.and_then(units).map(|q| at_path(q, vol, fid));
                if moved == Some(AtPath::Planned) {
                    Reconciled::Performed
                } else if original == AtPath::Planned {
                    Reconciled::NotPerformed
                } else {
                    Reconciled::StillUnknown
                }
            }
            // Recycle Bin contents are not inspected (item names are generated by the Shell and
            // identity is not tracked there), so a vanished file stays unknown for a person.
            Mode::Trash => match original {
                AtPath::Planned => Reconciled::NotPerformed,
                _ => Reconciled::StillUnknown,
            },
        }
    }

    fn free_space(&self, path: &RawPath) -> Option<u64> {
        let (display, _) = full_path(units(path)?).ok()?;
        let root = volume_root(&display)?;
        let z = nul(&root);
        let mut free = 0u64;
        // SAFETY: NUL-terminated directory; only the total-free out-pointer is requested.
        let ok = unsafe { GetDiskFreeSpaceExW(z.as_ptr(), std::ptr::null_mut(), std::ptr::null_mut(), &mut free) };
        (ok != 0).then_some(free)
    }

    fn remove_empty_dir(&self, t: &ActionTarget<'_>) -> ActionOutcome {
        if let Err(o) = privileges_dropped() {
            return o;
        }
        // Pinned, verified ancestors; the directory itself is opened for DELETE + listing without
        // delete sharing, so the name cannot be swapped between the checks and the disposition.
        let v = match verify_as(t, DELETE | FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE, SHARE_PIN, FLAG_BACKUP_SEMANTICS, true) {
            Ok(v) => v,
            Err(stop) => return stop.outcome(),
        };
        let new_descendant = || ActionOutcome::Skipped {
            code: "new_descendant".into(),
            message: "something not in the plan is inside the directory; it is kept and nothing inside is touched".into(),
        };
        match has_entries(&v.target) {
            Ok(false) => {}
            Ok(true) => return new_descendant(),
            Err(e) => return ActionOutcome::Failed { code: "check_failed".into(), message: format!("cannot list the directory: {e}") },
        }
        // The file system refuses to mark a non-empty directory for deletion, and nothing can be
        // created in it once marked, so an entry arriving after the listing is never swept up.
        if let Err(e) = dispose(&v.target) {
            return match e.raw_os_error() {
                Some(ERROR_DIR_NOT_EMPTY) => new_descendant(),
                Some(ERROR_ACCESS_DENIED) => ActionOutcome::Failed { code: "access_denied".into(), message: format!("delete refused: {e}") },
                Some(ERROR_SHARING_VIOLATION) => ActionOutcome::Failed { code: "in_use".into(), message: format!("delete refused: {e}") },
                _ => ActionOutcome::Failed { code: "delete_failed".into(), message: format!("delete refused: {e}") },
            };
        }
        let path = v.verbatim.clone();
        drop(v); // closing the handle commits the removal
        match at_path(&path, &t.identity.volume_id, &t.identity.file_id) {
            AtPath::Missing | AtPath::Other => ActionOutcome::Succeeded { recovery: Recovery::Unavailable },
            AtPath::Planned => ActionOutcome::Unknown { message: "directory is still present after removal (other handles may keep it pending)".into() },
            AtPath::Error(e) => ActionOutcome::Unknown { message: format!("removal pending or unconfirmed: {e}") },
        }
    }

    fn trash_tree(&self, t: &ActionTarget<'_>, members: &[TreeMember<'_>]) -> ActionOutcome {
        let skip = |code: &str, message: String| ActionOutcome::Skipped { code: code.into(), message };
        let Some(u) = units(t.path) else { return skip("unsupported", "not a Windows path".into()) };
        let display = match full_path(u) {
            Ok((d, _)) => d,
            Err(e) => return ActionOutcome::Failed { code: "check_failed".into(), message: format!("cannot resolve path: {e}") },
        };
        if let Err(o) = privileges_dropped() {
            return o;
        }
        if let Err(why) = recycle_bin_available(&display) {
            return ActionOutcome::Failed { code: "trash_unavailable".into(), message: why };
        }
        // Ancestors stay pinned; the attribute-only handle on the directory follows it into the bin.
        let v = match verify_as(t, FILE_READ_ATTRIBUTES | SYNCHRONIZE, SHARE_ALL, FLAG_BACKUP_SEMANTICS, true) {
            Ok(v) => v,
            Err(stop) => return stop.outcome(),
        };
        match compare_tree(&v.verbatim, members) {
            Ok(d) if d.exact(members.len()) => {}
            Ok(d) => return skip("tree_mismatch", d.summary()),
            Err(e) => return skip("tree_mismatch", format!("cannot enumerate the directory: {e}")),
        }
        // RESIDUAL RACE (as for files): the Shell moves by path, so an entry created inside after
        // the comparison would go along. It stays recoverable in the bin and is reported below.
        let (vol, fid) = (t.identity.volume_id.clone(), t.identity.file_id.clone());
        let shell = shell::recycle_item(display, vol, fid, true);
        let now = at_path(&v.verbatim, &t.identity.volume_id, &t.identity.file_id);
        let moved = final_path(&v.target).filter(|p| p.to_ascii_lowercase().contains("\\$recycle.bin\\"));
        let reported_ok = shell.performed.is_ok() && !shell.aborted;
        match now {
            AtPath::Planned if reported_ok => ActionOutcome::Unknown { message: "the Shell reported success but the directory is still at its path".into() },
            AtPath::Planned => {
                let why = shell.refused.or(shell.performed.err()).unwrap_or_else(|| "operation aborted".into());
                ActionOutcome::Failed { code: "trash_unavailable".into(), message: format!("not moved to the recycle bin as a whole: {why}") }
            }
            AtPath::Missing | AtPath::Other => match moved {
                Some(bin) => match compare_tree(&bin.encode_utf16().collect::<Vec<u16>>(), members) {
                    Ok(d) if d.exact(members.len()) => ActionOutcome::Succeeded { recovery: Recovery::Trash },
                    Ok(d) => ActionOutcome::Unknown { message: format!("the directory is in the Recycle Bin but differs from the plan ({}); review it there", d.summary()) },
                    Err(e) => ActionOutcome::Unknown { message: format!("the directory is in the Recycle Bin; its contents cannot be confirmed: {e}") },
                },
                None => ActionOutcome::Unknown { message: "the directory left its path but was not confirmed in the recycle bin".into() },
            },
            AtPath::Error(e) => ActionOutcome::Unknown { message: format!("cannot confirm the recycle result: {e}") },
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Directories: emptiness on a verified handle and whole-subtree comparison with the plan

const FILE_LIST_DIRECTORY: u32 = 0x0001;
const ERROR_NO_MORE_FILES: i32 = 18;
const ERROR_DIR_NOT_EMPTY: i32 = 145;

/// Whether the directory open on `h` (with `FILE_LIST_DIRECTORY`) has any entry besides `.` and
/// `..`, listed on that same handle so the answer is about the verified object.
fn has_entries(h: &Owned) -> io::Result<bool> {
    use windows_sys::Win32::Storage::FileSystem::{FILE_ID_BOTH_DIR_INFO, FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo};
    let mut buf = vec![0u64; 8 * 1024]; // 64 KiB, 8-byte aligned records
    let mut class = FileIdBothDirectoryRestartInfo;
    loop {
        // SAFETY: the buffer has the declared size; the class fills FILE_ID_BOTH_DIR_INFO records.
        let ok = unsafe { GetFileInformationByHandleEx(h.0, class, buf.as_mut_ptr().cast(), (buf.len() * 8) as u32) };
        if ok == 0 {
            let e = io::Error::last_os_error();
            return if e.raw_os_error() == Some(ERROR_NO_MORE_FILES) { Ok(false) } else { Err(e) };
        }
        let base = buf.as_ptr().cast::<u8>();
        let mut off = 0usize;
        loop {
            // SAFETY: `off` follows the NextEntryOffset chain written by the call inside `buf`;
            // each record holds FileNameLength bytes of name after its fixed part.
            let (next, name) = unsafe {
                let rec = base.add(off).cast::<FILE_ID_BOTH_DIR_INFO>();
                let len = (*rec).FileNameLength as usize / 2;
                ((*rec).NextEntryOffset, std::slice::from_raw_parts(std::ptr::addr_of!((*rec).FileName).cast::<u16>(), len))
            };
            if name != [b'.' as u16] && name != [b'.' as u16, b'.' as u16] {
                return Ok(true);
            }
            if next == 0 {
                break;
            }
            off += next as usize;
        }
        class = FileIdBothDirectoryInfo;
    }
}

/// A live subtree compared with planned members by identity (never by name).
#[derive(Debug, Default)]
struct TreeDiff {
    matched: usize,
    unplanned: usize,
    changed: Vec<String>,
}

impl TreeDiff {
    fn exact(&self, members: usize) -> bool {
        self.unplanned == 0 && self.changed.is_empty() && self.matched == members
    }
    fn summary(&self) -> String {
        let mut s = format!("{} planned items found, {} not in the plan", self.matched, self.unplanned);
        if let Some(first) = self.changed.first() {
            s.push_str(&format!(", {} changed (first: {first})", self.changed.len()));
        }
        s
    }
}

/// Walk the subtree under `root` (verbatim path) without following links; every entry must be a
/// planned member with unchanged identity, kind and (files) size, time and single link.
fn compare_tree(root: &[u16], members: &[TreeMember<'_>]) -> io::Result<TreeDiff> {
    let by_id: std::collections::HashMap<&str, &TreeMember<'_>> = members.iter().map(|m| (m.identity.file_id.as_str(), m)).collect();
    let mut seen = std::collections::HashSet::new();
    let mut diff = TreeDiff::default();
    let mut stack = vec![os_path(root)];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let child = entry?.path();
            let l = match open_attrs(&wide(&child)).and_then(|h| live(&h)) {
                Ok(l) => l,
                Err(e) => {
                    diff.changed.push(format!("unreadable entry: {e}"));
                    continue;
                }
            };
            let Some(m) = by_id.get(l.file_id.as_str()).filter(|m| same_volume(&l.volume_id, &m.identity.volume_id)) else {
                diff.unplanned += 1;
                continue;
            };
            let change = if l.reparse() {
                Some("a link or reparse point".to_string())
            } else if l.directory != m.directory {
                Some("kind changed".into())
            } else if !l.directory && l.links > 1 {
                Some(format!("file has {} hard links", l.links))
            } else if !l.directory {
                metadata_change(m.observation, &l)
            } else {
                None
            };
            if let Some(c) = change {
                diff.changed.push(c);
            } else if seen.insert(l.file_id.clone()) {
                diff.matched += 1;
                if l.directory {
                    stack.push(child);
                }
            }
        }
    }
    Ok(diff)
}

/// Restore and purge of quarantined objects (`windows/recovery.rs`); a child module so it reuses
/// the private verified-handle helpers above.
#[path = "recovery.rs"]
pub mod recovery;

/// Read-only listing of the current user's Recycle Bin for manual purge plans (MTR-03).
#[path = "recycle_bin.rs"]
pub mod recycle_bin;

/// Whether the volume holding `path` has a usable recycle bin. Manual trash plans are refused up front without one
/// (MTR-03): a move is never degraded to a permanent delete.
pub fn recycle_bin_ready(path: &RawPath) -> Result<(), String> {
    let u = units(path).ok_or("not a Windows path")?;
    let (display, _) = full_path(u).map_err(|e| format!("cannot resolve path: {e}"))?;
    recycle_bin_available(&display)
}

/// Every volume among `paths` without a usable recycle bin, each volume checked once, in first-seen order:
/// `(volume root or the unresolvable path, why)`. Used to refuse a trash-mode plan up front (MTR-03, WFA-35).
pub fn recycle_bins_unready(paths: &[RawPath]) -> Vec<(String, String)> {
    let mut checked = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for p in paths {
        let root = units(p)
            .ok_or_else(|| "not a Windows path".to_string())
            .and_then(|u| full_path(u).map_err(|e| format!("cannot resolve path: {e}")))
            .and_then(|(display, _)| volume_root(&display).ok_or_else(|| "cannot determine the volume root".to_string()));
        match root {
            Err(why) => out.push((p.display(), why)),
            Ok(root) => {
                let shown = String::from_utf16_lossy(&root);
                if checked.insert(shown.to_lowercase())
                    && let Err(why) = recycle_bin_on(&root)
                {
                    out.push((shown, why));
                }
            }
        }
    }
    out
}

/// `{GUID}` of the volume holding `path` (how Explorer names its per-volume Recycle Bin settings); tests use it to
/// write a redirected setting under `CM_TEST_BITBUCKET_ROOT`.
pub fn recycle_bin_volume_key(path: &RawPath) -> Option<String> {
    let (display, _) = full_path(units(path)?).ok()?;
    volume_guid_of_root(&volume_root(&display)?)
}

#[cfg(test)]
mod tests;
