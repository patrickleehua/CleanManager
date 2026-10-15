//! Windows maintenance tasks, first batch (parity 14.2, MNT-01/02/03; catalog in `cm_core::maintenance`).
//!
//! Each task is a fixed implementation: a read-only probe, then one action. Nothing here takes a command string.
//! - `windows.dns-flush`: `DnsFlushResolverCache` from System32 `dnsapi.dll`.
//! - `windows.shell-restart`: StartMenuExperienceHost/SearchHost (SearchApp on older builds) of **this session**, whose
//!   image is under `%windir%\SystemApps`, are asked to close through Restart Manager without force (`RmShutdown` with
//!   no flags: the same graceful shutdown an installer uses). Windows starts them again on next use. A host that does
//!   not close is reported as `attention`; nothing is ever terminated.
//! - `windows.icon-cache-rebuild`: `iconcache_*.db`/`thumbcache_*.db` directly in the Explorer cache folder (never
//!   links/reparse points) are deleted only when they can be opened exclusively; files in use stay (`attention`).
//! - `windows.trim-when-due`: fixed SSD volumes with TRIM enabled whose last optimization (Defrag statistics, read-only)
//!   is older than the due period are retrimmed with System32 `defrag.exe X: /L` (administrator).
//! - `windows.image-health-check`: DISM API `DismCheckImageHealth` without a scan (= `/CheckHealth`, read-only,
//!   administrator); repairable or unrepairable stores are `attention` with the manual repair route.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cm_core::maintenance::{Execution, Probe, TaskBackend, TaskOutcome, TaskSpec, ids};
use windows_sys::Win32::Foundation::{ERROR_FAIL_SHUTDOWN, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION, FILETIME, FreeLibrary, GetLastError, HANDLE, HMODULE, INVALID_HANDLE_VALUE};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_path(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

fn owned(h: HANDLE) -> Option<OwnedHandle> {
    // SAFETY: `h` is a fresh, valid handle owned by the caller from here on.
    (!h.is_null() && h != INVALID_HANDLE_VALUE).then(|| unsafe { OwnedHandle::from_raw_handle(h) })
}

fn last_error() -> u32 {
    // SAFETY: reads the calling thread's last-error value.
    unsafe { GetLastError() }
}

/// `C:\Windows\System32`.
pub fn system_dir() -> Option<PathBuf> {
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut buf = [0u16; 512];
    // SAFETY: buffer of the declared length.
    let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (n > 0 && n < buf.len()).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n])))
}

/// A System32 DLL loaded by full search restriction, freed on drop.
struct SystemDll(HMODULE);

impl SystemDll {
    fn load(name: &str) -> Option<SystemDll> {
        use windows_sys::Win32::System::LibraryLoader::{LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
        let n = wide(name);
        // SAFETY: NUL-terminated name; only System32 is searched.
        let h = unsafe { LoadLibraryExW(n.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
        (!h.is_null()).then_some(SystemDll(h))
    }

    /// The raw entry point `name` (a NUL-terminated ASCII string).
    fn proc(&self, name: &[u8]) -> Option<unsafe extern "system" fn() -> isize> {
        debug_assert_eq!(name.last(), Some(&0));
        // SAFETY: valid module, NUL-terminated name.
        unsafe { windows_sys::Win32::System::LibraryLoader::GetProcAddress(self.0, name.as_ptr()) }
    }
}

impl Drop for SystemDll {
    fn drop(&mut self) {
        // SAFETY: module loaded by LoadLibraryExW, freed once.
        unsafe { FreeLibrary(self.0) };
    }
}

/// The fixed Windows task implementations.
pub struct WindowsTasks {
    /// `%LOCALAPPDATA%\Microsoft\Windows\Explorer`; tests point it at a scratch folder.
    pub explorer_dir: Option<PathBuf>,
    pub is_admin: bool,
    pub now: fn() -> i64,
    /// TRIM is due when a volume was last optimized longer ago than this (or never).
    pub trim_due_days: i64,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Days after which a retrim is due (Windows schedules weekly; this product asks only when well overdue).
pub const TRIM_DUE_DAYS: i64 = 30;

impl WindowsTasks {
    pub fn real() -> WindowsTasks {
        let explorer_dir = std::env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(l).join("Microsoft").join("Windows").join("Explorer"));
        WindowsTasks { explorer_dir, is_admin: super::startup_actions::is_elevated(), now: now_unix, trim_due_days: TRIM_DUE_DAYS }
    }
}

impl TaskBackend for WindowsTasks {
    fn probe(&self, t: &TaskSpec) -> Probe {
        match t.id {
            ids::DNS_FLUSH => dns_probe(),
            ids::SHELL_RESTART => shell_probe(),
            ids::ICON_CACHE => match &self.explorer_dir {
                Some(d) => icon_probe(d),
                None => Probe::Unknown { reason: "the local application data folder is not known".into() },
            },
            ids::TRIM => trim_probe((self.now)(), self.trim_due_days),
            ids::IMAGE_HEALTH => image_probe(self.is_admin),
            other => Probe::Unavailable { code: "task_unknown".into(), reason: format!("no Windows implementation of {other}") },
        }
    }

    fn execute(&self, t: &TaskSpec) -> Execution {
        if t.requires_admin && !self.is_admin {
            return Execution::new(TaskOutcome::Skipped, "permission_required", "needs administrator authorization; not run");
        }
        match t.id {
            ids::DNS_FLUSH => dns_flush(),
            ids::SHELL_RESTART => shell_restart(),
            ids::ICON_CACHE => match &self.explorer_dir {
                Some(d) => icon_rebuild(d),
                None => Execution::new(TaskOutcome::Skipped, "state_unknown", "the local application data folder is not known"),
            },
            ids::TRIM => trim_run((self.now)(), self.trim_due_days, Duration::from_secs(u64::from(t.timeout_seconds))),
            ids::IMAGE_HEALTH => image_check(),
            other => Execution::new(TaskOutcome::Unavailable, "task_unknown", format!("no Windows implementation of {other}")),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// DNS

fn dns_probe() -> Probe {
    match SystemDll::load("dnsapi.dll").and_then(|d| d.proc(b"DnsFlushResolverCache\0").map(|_| ())) {
        Some(()) => Probe::Applicable { detail: "the DNS client cache can be flushed".into() },
        None => Probe::Unavailable { code: "api_missing".into(), reason: "DnsFlushResolverCache is not available on this system".into() },
    }
}

fn dns_flush() -> Execution {
    let Some(dll) = SystemDll::load("dnsapi.dll") else { return Execution::new(TaskOutcome::Unavailable, "api_missing", "dnsapi.dll is not available") };
    let Some(f) = dll.proc(b"DnsFlushResolverCache\0") else { return Execution::new(TaskOutcome::Unavailable, "api_missing", "DnsFlushResolverCache is not available") };
    // SAFETY: `BOOL WINAPI DnsFlushResolverCache(void)`; the module stays loaded during the call.
    let ok = unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, unsafe extern "system" fn() -> i32>(f)() };
    if ok != 0 {
        Execution::new(TaskOutcome::Applied, "flushed", "DNS resolver cache flushed; names are looked up again on next use")
    } else {
        Execution::new(TaskOutcome::Failed, "flush_failed", format!("the DNS client refused the flush (error {}); is the DNS Client service running?", last_error()))
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Start menu / Search hosts

/// Image names of the shell hosts this task may restart (Windows 11, and SearchApp on Windows 10 builds).
pub const SHELL_HOSTS: [&str; 3] = ["startmenuexperiencehost.exe", "searchhost.exe", "searchapp.exe"];

/// A shell host only if both its name and its image under `%windir%\SystemApps\` match (`system_apps` lowercase,
/// with a trailing backslash).
pub fn is_shell_host(name: &str, image: &str, system_apps: &str) -> bool {
    let n = name.to_lowercase();
    let i = image.to_lowercase();
    SHELL_HOSTS.contains(&n.as_str()) && i.starts_with(system_apps) && i.ends_with(&format!("\\{n}")) && !i.contains("..")
}

#[derive(Clone)]
struct ShellProc {
    pid: u32,
    name: String,
    start: FILETIME,
}

fn process_image(pid: u32) -> Option<(String, FILETIME)> {
    use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
    // SAFETY: limited query access only; the handle is owned below.
    let h = owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })?;
    let mut buf = vec![0u16; 32_768];
    let mut len = buf.len() as u32;
    // SAFETY: buffer of `len` characters.
    if unsafe { QueryFullProcessImageNameW(h.as_raw_handle(), PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) } == 0 {
        return None;
    }
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut c, mut e, mut k, mut u) = (zero, zero, zero, zero);
    // SAFETY: valid handle and out-pointers.
    if unsafe { GetProcessTimes(h.as_raw_handle(), &mut c, &mut e, &mut k, &mut u) } == 0 {
        return None;
    }
    Some((String::from_utf16_lossy(&buf[..len as usize]), c))
}

/// (pid, image name) of every process.
pub(crate) fn process_list() -> Vec<(u32, String)> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    // SAFETY: process snapshot; the handle is owned below.
    let Some(snap) = owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else { return Vec::new() };
    // SAFETY: plain data; dwSize set as required.
    let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut out = Vec::new();
    // SAFETY: valid snapshot and entry buffer.
    let mut ok = unsafe { Process32FirstW(snap.as_raw_handle(), &mut e) };
    while ok != 0 {
        let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
        out.push((e.th32ProcessID, String::from_utf16_lossy(&e.szExeFile[..n])));
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snap.as_raw_handle(), &mut e) };
    }
    out
}

fn session_of(pid: u32) -> Option<u32> {
    let mut s = 0u32;
    // SAFETY: valid out-pointer.
    (unsafe { windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId(pid, &mut s) } != 0).then_some(s)
}

fn shell_processes() -> Result<Vec<ShellProc>, String> {
    let windir = system_dir().and_then(|s| s.parent().map(Path::to_path_buf)).ok_or("the Windows folder is not known")?;
    let system_apps = format!("{}\\systemapps\\", windir.display().to_string().to_lowercase().trim_end_matches('\\'));
    let me = session_of(std::process::id()).ok_or("this session is not known")?;
    let mut out = Vec::new();
    for (pid, name) in process_list() {
        if !SHELL_HOSTS.contains(&name.to_lowercase().as_str()) || session_of(pid) != Some(me) {
            continue;
        }
        if let Some((image, start)) = process_image(pid)
            && is_shell_host(&name, &image, &system_apps)
        {
            out.push(ShellProc { pid, name, start });
        }
    }
    Ok(out)
}

fn shell_probe() -> Probe {
    match shell_processes() {
        Err(e) => Probe::Unknown { reason: e },
        Ok(p) if p.is_empty() => Probe::NotNeeded { code: "not_running".into(), summary: "Start and Search are not running in this session; they start fresh on next use".into() },
        Ok(p) => Probe::Applicable { detail: format!("running: {}", p.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(", ")) },
    }
}

fn shell_restart() -> Execution {
    use windows_sys::Win32::System::RestartManager::{CCH_RM_SESSION_KEY, RM_UNIQUE_PROCESS, RmEndSession, RmRegisterResources, RmShutdown, RmStartSession};
    let procs = match shell_processes() {
        Ok(p) if p.is_empty() => return Execution::new(TaskOutcome::Unchanged, "not_running", "Start and Search are not running; nothing to restart"),
        Ok(p) => p,
        Err(e) => return Execution::new(TaskOutcome::Skipped, "state_unknown", e),
    };
    let names: Vec<String> = procs.iter().map(|p| p.name.clone()).collect();
    let mut session = 0u32;
    let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
    // SAFETY: valid out-pointers; the session is ended below.
    let rc = unsafe { RmStartSession(&mut session, 0, key.as_mut_ptr()) };
    if rc != 0 {
        return Execution::new(TaskOutcome::Failed, "restart_manager_unavailable", format!("Restart Manager did not start (error {rc})"));
    }
    let apps: Vec<RM_UNIQUE_PROCESS> = procs.iter().map(|p| RM_UNIQUE_PROCESS { dwProcessId: p.pid, ProcessStartTime: p.start }).collect();
    // SAFETY: the array outlives the call; the start times bind exactly these process instances.
    let rc = unsafe { RmRegisterResources(session, 0, std::ptr::null(), apps.len() as u32, apps.as_ptr(), 0, std::ptr::null()) };
    // Graceful only: no RmForceShutdown, so a host that does not agree to close keeps running.
    // SAFETY: registered session; no status callback.
    let rc = if rc == 0 { unsafe { RmShutdown(session, 0, None) } } else { rc };
    // SAFETY: session from RmStartSession, ended once.
    unsafe { RmEndSession(session) };
    match rc {
        0 => {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut back = Vec::new();
            while Instant::now() < deadline {
                back = shell_processes().unwrap_or_default().into_iter().filter(|p| !procs.iter().any(|o| o.pid == p.pid)).map(|p| p.name).collect();
                if !back.is_empty() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            let again = if back.is_empty() { "Windows starts them again on next use".to_owned() } else { format!("Windows started {} again", back.join(", ")) };
            Execution::new(TaskOutcome::Applied, "restarted", format!("closed {} gracefully; {again}", names.join(", ")))
        }
        ERROR_FAIL_SHUTDOWN => Execution::new(
            TaskOutcome::Attention,
            "did_not_close",
            format!("{} did not agree to close and keeps running (nothing was forced); sign out and back in to restart them", names.join(", ")),
        ),
        other => Execution::new(TaskOutcome::Failed, "restart_failed", format!("Restart Manager could not close {} (error {other})", names.join(", "))),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Icon and thumbnail cache

/// `iconcache_*.db` / `thumbcache_*.db`, nothing else.
pub fn is_cache_file_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    (n.starts_with("iconcache_") || n.starts_with("thumbcache_"))
        && n.ends_with(".db")
        && n.len() <= 64
        && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
}

const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// Cache files directly in `dir`; `Err` when `dir` itself is a link or unreadable.
pub fn cache_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    use std::os::windows::fs::MetadataExt;
    let md = std::fs::symlink_metadata(dir).map_err(|e| format!("cache folder not readable: {e}"))?;
    if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !md.is_dir() {
        return Err("the cache folder is a link or not a folder; not touched".into());
    }
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).map_err(|e| format!("cache folder not readable: {e}"))? {
        let e = e.map_err(|e| e.to_string())?;
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_file() && !ft.is_symlink() && is_cache_file_name(&name) {
            out.push(e.path());
        }
    }
    out.sort();
    Ok(out)
}

#[derive(Debug, PartialEq, Eq)]
pub enum CacheFile {
    Removed(u64),
    InUse,
    Kept(String),
}

/// Open exclusively without following links; `delete` then marks it deleted on close. A file any process has open
/// (Explorer usually does) fails the open with a sharing violation and is left alone.
pub fn handle_cache_file(path: &Path, delete: bool) -> CacheFile {
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, DELETE, FILE_ATTRIBUTE_TAG_INFO, FILE_DISPOSITION_INFO, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FileAttributeTagInfo, FileDispositionInfo,
        GetFileInformationByHandleEx, GetFileSizeEx, OPEN_EXISTING, SetFileInformationByHandle,
    };
    let w = wide_path(path);
    // SAFETY: NUL-terminated path; no sharing at all; links are opened, not followed.
    let h = unsafe { CreateFileW(w.as_ptr(), DELETE | FILE_READ_ATTRIBUTES, 0, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT, std::ptr::null_mut()) };
    let Some(h) = owned(h) else {
        let e = last_error();
        return if e == ERROR_SHARING_VIOLATION || e == ERROR_LOCK_VIOLATION { CacheFile::InUse } else { CacheFile::Kept(format!("not opened (error {e})")) };
    };
    let mut tag = FILE_ATTRIBUTE_TAG_INFO { FileAttributes: 0, ReparseTag: 0 };
    // SAFETY: valid handle and buffer of the declared size.
    if unsafe { GetFileInformationByHandleEx(h.as_raw_handle(), FileAttributeTagInfo, (&mut tag as *mut FILE_ATTRIBUTE_TAG_INFO).cast(), size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32) } == 0 {
        return CacheFile::Kept("attributes not readable".into());
    }
    if tag.FileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY) != 0 {
        return CacheFile::Kept("a link or folder, not a cache file".into());
    }
    let mut size = 0i64;
    // SAFETY: valid handle.
    unsafe { GetFileSizeEx(h.as_raw_handle(), &mut size) };
    if !delete {
        return CacheFile::Removed(0);
    }
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: valid handle opened with DELETE access.
    if unsafe { SetFileInformationByHandle(h.as_raw_handle(), FileDispositionInfo, (&info as *const FILE_DISPOSITION_INFO).cast(), size_of::<FILE_DISPOSITION_INFO>() as u32) } == 0 {
        return CacheFile::Kept(format!("not deleted (error {})", last_error()));
    }
    CacheFile::Removed(size.max(0) as u64)
}

fn icon_probe(dir: &Path) -> Probe {
    if !dir.exists() {
        return Probe::NotApplicable { code: "no_cache_folder".into(), reason: "there is no Explorer cache folder in this profile".into() };
    }
    match cache_files(dir) {
        Err(e) => Probe::Unknown { reason: e },
        Ok(f) if f.is_empty() => Probe::NotNeeded { code: "no_cache_files".into(), summary: "no icon or thumbnail cache files; Explorer builds them on next view".into() },
        Ok(f) => {
            let in_use = f.iter().filter(|p| handle_cache_file(p, false) == CacheFile::InUse).count();
            Probe::Applicable { detail: format!("{} cache files, {in_use} in use now", f.len()) }
        }
    }
}

fn icon_rebuild(dir: &Path) -> Execution {
    let files = match cache_files(dir) {
        Ok(f) if f.is_empty() => return Execution::new(TaskOutcome::Unchanged, "no_cache_files", "no icon or thumbnail cache files"),
        Ok(f) => f,
        Err(e) => return Execution::new(TaskOutcome::Skipped, "state_unknown", e),
    };
    let (mut removed, mut bytes, mut in_use, mut kept) = (0usize, 0u64, 0usize, Vec::new());
    for f in &files {
        match handle_cache_file(f, true) {
            CacheFile::Removed(b) => {
                removed += 1;
                bytes += b;
            }
            CacheFile::InUse => in_use += 1,
            CacheFile::Kept(why) => kept.push(format!("{}: {why}", f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())),
        }
    }
    let mib = bytes as f64 / f64::from(1u32 << 20);
    match (removed, in_use, kept.is_empty()) {
        (_, 0, true) => Execution::new(TaskOutcome::Applied, "rebuilt", format!("removed {removed} cache files ({mib:.1} MiB); Explorer redraws icons and thumbnails on next view")),
        (0, _, true) => Execution::new(
            TaskOutcome::Attention,
            "in_use",
            format!("all {in_use} cache files are in use by Explorer and were left alone; sign out and back in, then run this task again"),
        ),
        (_, _, true) => Execution::new(
            TaskOutcome::Attention,
            "partly_in_use",
            format!("removed {removed} cache files ({mib:.1} MiB); {in_use} in use by Explorer were left alone (sign out and back in, then run again)"),
        ),
        (0, 0, false) => Execution::new(TaskOutcome::Failed, "not_removed", kept.join("; ")),
        _ => Execution::new(TaskOutcome::Attention, "partly_kept", format!("removed {removed}, {in_use} in use, kept: {}", kept.join("; "))),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// TRIM when due

/// A 16-byte `SYSTEMTIME` (Defrag statistics `LastRunTime`) as Unix seconds; all zero = never.
pub fn systemtime_bytes_to_unix(b: &[u8]) -> Option<i64> {
    if b.len() != 16 {
        return None;
    }
    let w = |i: usize| i64::from(u16::from_le_bytes([b[2 * i], b[2 * i + 1]]));
    let (y, m, d, hh, mm, ss) = (w(0), w(1), w(3), w(4), w(5), w(6));
    if !(1601..=9999).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // Days from civil (proleptic Gregorian), Howard Hinnant's algorithm.
    let (yy, mo) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * mo + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

pub fn trim_due(last: Option<i64>, now: i64, due_days: i64) -> bool {
    last.is_none_or(|l| now - l > due_days * 86_400)
}

#[derive(Debug, Clone)]
struct SsdVolume {
    letter: char,
    last: Option<i64>,
}

/// `\\?\Volume{GUID}\` of a drive root.
fn volume_guid(root: &str) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::GetVolumeNameForVolumeMountPointW;
    let r = wide(root);
    let mut buf = [0u16; 64];
    // SAFETY: NUL-terminated root; buffer of the declared length.
    (unsafe { GetVolumeNameForVolumeMountPointW(r.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } != 0).then(|| {
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..n])
    })
}

fn file_system(root: &str) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW;
    let r = wide(root);
    let mut fs = [0u16; 32];
    // SAFETY: NUL-terminated root; only the file-system name buffer is requested.
    let ok = unsafe { GetVolumeInformationW(r.as_ptr(), std::ptr::null_mut(), 0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), fs.as_mut_ptr(), fs.len() as u32) };
    (ok != 0).then(|| String::from_utf16_lossy(&fs[..fs.iter().position(|&c| c == 0).unwrap_or(fs.len())]))
}

/// (no seek penalty, TRIM enabled) of the device under drive `letter`; read-only queries on a zero-access handle.
fn ssd_trim(letter: char) -> Option<(bool, bool)> {
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::{
        DEVICE_SEEK_PENALTY_DESCRIPTOR, DEVICE_TRIM_DESCRIPTOR, IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, STORAGE_PROPERTY_QUERY, StorageDeviceSeekPenaltyProperty,
        StorageDeviceTrimProperty,
    };
    let dev = wide(&format!(r"\\.\{letter}:"));
    // SAFETY: zero access rights (metadata queries only), shared.
    let h = owned(unsafe { CreateFileW(dev.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut()) })?;
    fn query<T: Default>(h: &OwnedHandle, id: i32) -> Option<T> {
        let q = STORAGE_PROPERTY_QUERY { PropertyId: id, QueryType: PropertyStandardQuery, AdditionalParameters: [0] };
        let mut out = T::default();
        let mut n = 0u32;
        // SAFETY: input and output buffers of the declared sizes.
        let ok = unsafe {
            DeviceIoControl(h.as_raw_handle(), IOCTL_STORAGE_QUERY_PROPERTY, (&q as *const STORAGE_PROPERTY_QUERY).cast(), size_of::<STORAGE_PROPERTY_QUERY>() as u32, (&mut out as *mut T).cast(), size_of::<T>() as u32, &mut n, std::ptr::null_mut())
        };
        (ok != 0).then_some(out)
    }
    let seek: DEVICE_SEEK_PENALTY_DESCRIPTOR = query(&h, StorageDeviceSeekPenaltyProperty)?;
    let trim: DEVICE_TRIM_DESCRIPTOR = query(&h, StorageDeviceTrimProperty)?;
    Some((!seek.IncursSeekPenalty, trim.TrimEnabled))
}

/// Last optimization of a volume from the Defrag statistics (readable by users); `Ok(None)` = never recorded.
fn last_optimized(guid_path: &str) -> Result<Option<i64>, String> {
    use windows_sys::Win32::System::Registry::{HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY, RegCloseKey, RegOpenKeyExW, RegQueryValueExW};
    let guid = guid_path.trim_start_matches(r"\\?\").trim_end_matches('\\');
    let key = wide(&format!(r"SOFTWARE\Microsoft\Dfrg\Statistics\{guid}"));
    let mut hk: HKEY = std::ptr::null_mut();
    // SAFETY: NUL-terminated subkey; read access only; closed below.
    let rc = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, key.as_ptr(), 0, KEY_READ | KEY_WOW64_64KEY, &mut hk) };
    if rc == 2 {
        return Ok(None);
    }
    if rc != 0 {
        return Err(format!("optimization statistics not readable (error {rc})"));
    }
    let name = wide("LastRunTime");
    let mut buf = [0u8; 16];
    let mut len = buf.len() as u32;
    // SAFETY: open key; buffer of `len` bytes.
    let rc = unsafe { RegQueryValueExW(hk, name.as_ptr(), std::ptr::null(), std::ptr::null_mut(), buf.as_mut_ptr(), &mut len) };
    // SAFETY: key opened above, closed once.
    unsafe { RegCloseKey(hk) };
    match rc {
        0 => Ok(systemtime_bytes_to_unix(&buf[..len as usize])),
        2 => Ok(None),
        other => Err(format!("last optimization time not readable (error {other})")),
    }
}

fn ssd_volumes() -> Result<Vec<SsdVolume>, String> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    const DRIVE_FIXED: u32 = 3;
    // SAFETY: no arguments.
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = char::from(b'A' + i as u8);
        let root = format!("{letter}:\\");
        let r = wide(&root);
        // SAFETY: NUL-terminated root path.
        if unsafe { GetDriveTypeW(r.as_ptr()) } != DRIVE_FIXED || !matches!(file_system(&root).as_deref(), Some("NTFS" | "ReFS")) {
            continue;
        }
        let Some((true, true)) = ssd_trim(letter) else { continue };
        let guid = volume_guid(&root).ok_or_else(|| format!("{letter}: volume name not readable"))?;
        out.push(SsdVolume { letter, last: last_optimized(&guid)? });
    }
    Ok(out)
}

fn describe_last(v: &SsdVolume) -> String {
    match v.last {
        Some(t) => format!("{}: last optimized {}", v.letter, cm_core::time::utc_from_unix(t)),
        None => format!("{}: never optimized", v.letter),
    }
}

fn trim_probe(now: i64, due_days: i64) -> Probe {
    match ssd_volumes() {
        Err(e) => Probe::Unknown { reason: e },
        Ok(v) if v.is_empty() => Probe::NotApplicable { code: "no_ssd".into(), reason: "no fixed SSD volume with TRIM enabled".into() },
        Ok(v) => {
            let (due, fine): (Vec<_>, Vec<_>) = v.iter().partition(|x| trim_due(x.last, now, due_days));
            if due.is_empty() {
                Probe::NotNeeded { code: "not_due".into(), summary: fine.iter().map(|x| describe_last(x)).collect::<Vec<_>>().join("; ") }
            } else {
                Probe::Applicable { detail: format!("due: {}", due.iter().map(|x| describe_last(x)).collect::<Vec<_>>().join("; ")) }
            }
        }
    }
}

/// Run a fixed System32 program with fixed arguments; `Err` on start failure or timeout (the child is then ended).
fn run_system_tool(exe: &str, args: &[&str], timeout: Duration) -> Result<i32, String> {
    let path = system_dir().ok_or("System32 is not known")?.join(exe);
    let mut child = std::process::Command::new(&path)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("{exe} did not start: {e}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(s)) => return Ok(s.code().unwrap_or(-1)),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{exe} did not finish within {} s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(250)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn trim_run(now: i64, due_days: i64, timeout: Duration) -> Execution {
    let vols = match ssd_volumes() {
        Ok(v) => v,
        Err(e) => return Execution::new(TaskOutcome::Skipped, "state_unknown", e),
    };
    let due: Vec<&SsdVolume> = vols.iter().filter(|x| trim_due(x.last, now, due_days)).collect();
    if due.is_empty() {
        return Execution::new(TaskOutcome::Unchanged, "not_due", "no SSD volume is due for retrim");
    }
    let deadline = Instant::now() + timeout;
    let (mut done, mut failed) = (Vec::new(), Vec::new());
    for v in due {
        let left = deadline.saturating_duration_since(Instant::now());
        match run_system_tool("defrag.exe", &[&format!("{}:", v.letter), "/L"], left) {
            Ok(0) => done.push(format!("{}:", v.letter)),
            Ok(code) => failed.push(format!("{}: exit {code}", v.letter)),
            Err(e) => failed.push(format!("{}: {e}", v.letter)),
        }
    }
    match (done.is_empty(), failed.is_empty()) {
        (false, true) => Execution::new(TaskOutcome::Applied, "retrimmed", format!("retrimmed {}", done.join(", "))),
        (true, _) => Execution::new(TaskOutcome::Failed, "retrim_failed", failed.join("; ")),
        (false, false) => Execution::new(TaskOutcome::Attention, "partly_retrimmed", format!("retrimmed {}; not done: {}", done.join(", "), failed.join("; "))),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// System image health (DISM API, read-only CheckHealth)

const DISM_ONLINE_IMAGE: &str = "DISM_{53BFAE52-B167-4E2F-A258-0A37B57FF845}";
const DISM_ENTRIES: [&[u8]; 5] = [b"DismInitialize\0", b"DismOpenSession\0", b"DismCheckImageHealth\0", b"DismCloseSession\0", b"DismShutdown\0"];

fn image_probe(is_admin: bool) -> Probe {
    let Some(dll) = SystemDll::load("dismapi.dll") else { return Probe::Unavailable { code: "api_missing".into(), reason: "the DISM API is not available".into() } };
    if DISM_ENTRIES.iter().any(|e| dll.proc(e).is_none()) {
        return Probe::Unavailable { code: "api_missing".into(), reason: "the DISM API lacks the health check".into() };
    }
    let detail = if is_admin { "read-only component store check" } else { "read-only component store check; needs administrator authorization" };
    Probe::Applicable { detail: detail.into() }
}

fn image_check() -> Execution {
    type Init = unsafe extern "system" fn(i32, *const u16, *const u16) -> i32;
    type Open = unsafe extern "system" fn(*const u16, *const u16, *const u16, *mut u32) -> i32;
    type Check = unsafe extern "system" fn(u32, i32, HANDLE, *const core::ffi::c_void, *const core::ffi::c_void, *mut i32) -> i32;
    type Close = unsafe extern "system" fn(u32) -> i32;
    type Shutdown = unsafe extern "system" fn() -> i32;
    let Some(dll) = SystemDll::load("dismapi.dll") else { return Execution::new(TaskOutcome::Unavailable, "api_missing", "the DISM API is not available") };
    let procs: Vec<_> = DISM_ENTRIES.iter().filter_map(|e| dll.proc(e)).collect();
    if procs.len() != DISM_ENTRIES.len() {
        return Execution::new(TaskOutcome::Unavailable, "api_missing", "the DISM API lacks the health check");
    }
    // SAFETY: documented dismapi.h signatures (WINAPI); the module stays loaded while `dll` lives.
    let (init, open, check, close, shutdown) = unsafe {
        (
            std::mem::transmute::<unsafe extern "system" fn() -> isize, Init>(procs[0]),
            std::mem::transmute::<unsafe extern "system" fn() -> isize, Open>(procs[1]),
            std::mem::transmute::<unsafe extern "system" fn() -> isize, Check>(procs[2]),
            std::mem::transmute::<unsafe extern "system" fn() -> isize, Close>(procs[3]),
            std::mem::transmute::<unsafe extern "system" fn() -> isize, Shutdown>(procs[4]),
        )
    };
    const DISM_LOG_ERRORS: i32 = 0;
    // SAFETY: default log and scratch locations.
    let hr = unsafe { init(DISM_LOG_ERRORS, std::ptr::null(), std::ptr::null()) };
    if hr < 0 {
        return Execution::new(TaskOutcome::Failed, &format!("dism_0x{:08x}", hr as u32), "the DISM API did not initialize");
    }
    let image = wide(DISM_ONLINE_IMAGE);
    let mut session = 0u32;
    // SAFETY: the online image; session closed below.
    let hr = unsafe { open(image.as_ptr(), std::ptr::null(), std::ptr::null(), &mut session) };
    let result = if hr < 0 {
        Err(hr)
    } else {
        let mut state = -1i32;
        // SAFETY: ScanImage = FALSE (CheckHealth), no cancel event or progress callback.
        let hr = unsafe { check(session, 0, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), &mut state) };
        // SAFETY: session opened above.
        unsafe { close(session) };
        if hr < 0 { Err(hr) } else { Ok(state) }
    };
    // SAFETY: balanced with the successful initialization.
    unsafe { shutdown() };
    match result {
        Ok(0) => Execution::new(TaskOutcome::Unchanged, "healthy", "no component store corruption was detected (CheckHealth, read-only)"),
        Ok(1) => Execution::new(
            TaskOutcome::Attention,
            "repairable",
            "the component store is flagged as repairable; nothing was repaired. Run `DISM /Online /Cleanup-Image /RestoreHealth` in an administrator terminal, or Settings > System > Recovery > Fix problems using Windows Update",
        ),
        Ok(2) => Execution::new(
            TaskOutcome::Attention,
            "not_repairable",
            "the component store cannot be repaired in place; use Settings > System > Recovery (reinstall Windows keeping your files)",
        ),
        Ok(other) => Execution::new(TaskOutcome::Failed, "unknown_state", format!("the DISM API reported an unknown health state {other}")),
        Err(hr) if hr as u32 == 0x8007_02E4 => Execution::new(TaskOutcome::Skipped, "permission_required", "needs administrator authorization; not run"),
        Err(hr) => Execution::new(TaskOutcome::Failed, &format!("dism_0x{:08x}", hr as u32), "the DISM health check did not complete"),
    }
}

#[cfg(test)]
mod tests;
