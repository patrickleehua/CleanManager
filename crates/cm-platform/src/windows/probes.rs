//! Tri-state in-use probes for catalog candidates (CAT-06), Windows.
//!
//! Every probe answers `in_use`, `not_in_use` or `unknown`, and `unknown` is never read as "not in
//! use": enumeration failures, access denials, cloud placeholders and oversized trees all give
//! `unknown`. Probes only open files for attribute/sharing checks (no data is read, nothing is
//! changed) and never follow links. They run at preview/plan time; the executor re-checks each
//! item before acting (see [`execution_check`] and the exclusive open in `actions.rs`).

use std::collections::BTreeSet;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cm_core::catalog::{ProbeResult, ProbeState};
use cm_core::execute::{ActionTarget, LiveCheck};
use cm_core::path::RawPath;
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SHARING_VIOLATION, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_DATA, OPEN_EXISTING};

/// Files examined per candidate and time budget; beyond either the answer is `unknown`.
pub const MAX_PROBE_FILES: usize = 20_000;
pub const PROBE_BUDGET: Duration = Duration::from_secs(10);

const SYNCHRONIZE: u32 = 0x0010_0000;
const ATTR_REPARSE: u32 = 0x400;
const ATTR_OFFLINE: u32 = 0x1000;
const ATTR_RECALL_ON_OPEN: u32 = 0x4_0000;
const ATTR_RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;

/// Owner id → process image names whose presence means the owner is running.
const OWNER_PROCESSES: &[(&str, &[&str])] = &[
    ("google.chrome", &["chrome.exe"]),
    ("microsoft.edge", &["msedge.exe"]),
    ("mozilla.firefox", &["firefox.exe"]),
    ("microsoft.vscode", &["Code.exe"]),
    ("go", &["go.exe"]),
    ("cargo", &["cargo.exe"]),
];

pub fn owner_processes(owner: &str) -> Option<&'static [&'static str]> {
    OWNER_PROCESSES.iter().find(|(id, _)| *id == owner).map(|(_, p)| *p)
}

/// Directory names of endpoint-security / device-management products whose data is never a
/// cleanup target (PRT-03 complement; Mole `file_ops` EDR exclusions).
const ENDPOINT_SECURITY: &[&str] = &[
    "windows defender", "windows defender advanced threat protection", "microsoft security client", "crowdstrike", "sentinelone",
    "sentinel agent", "symantec", "symantec endpoint protection", "mcafee", "carbonblack", "carbon black", "confer", "sophos", "eset",
    "kaspersky lab", "trend micro", "cylance", "palo alto networks", "traps", "cortex xdr", "tanium", "qualys", "bitdefender",
    "malwarebytes", "intune management extension", "microsoft intune management extension",
];

/// Extensions of in-progress download files (Chromium, Firefox, generic).
const DOWNLOAD_MARKERS: &[&str] = &["crdownload", "part", "partial", "download", "opdownload"];
const DB_EXT: &[&str] = &["db", "sqlite", "sqlite3", "db-wal", "db-shm", "sqlite-wal", "sqlite-shm", "db-journal", "ldb", "log", "mdb", "accdb"];

/// One process listing shared by every owner probe of a preview.
#[derive(Debug, Clone)]
pub struct ProcessSnapshot {
    names: Result<BTreeSet<String>, String>,
}

impl ProcessSnapshot {
    pub fn take() -> Self {
        ProcessSnapshot { names: process_names() }
    }

    /// For tests: a fixed listing or a failed enumeration.
    pub fn fixed(names: Result<&[&str], &str>) -> Self {
        ProcessSnapshot { names: names.map(|n| n.iter().map(|s| s.to_ascii_lowercase()).collect()).map_err(str::to_owned) }
    }
}

fn process_names() -> Result<BTreeSet<String>, String> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    // SAFETY: whole-system process snapshot; no process ID needed.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return Err(format!("process snapshot failed: {}", std::io::Error::last_os_error()));
    }
    let mut out = BTreeSet::new();
    // SAFETY: plain-data structure; zero is valid; dwSize set as the API requires.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: valid snapshot handle and entry buffer.
    let mut more = unsafe { Process32FirstW(snap, &mut entry) } != 0;
    while more {
        let n = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
        out.insert(String::from_utf16_lossy(&entry.szExeFile[..n]).to_ascii_lowercase());
        // SAFETY: as above.
        more = unsafe { Process32NextW(snap, &mut entry) } != 0;
    }
    // SAFETY: snapshot handle owned here, closed once.
    unsafe { CloseHandle(snap) };
    if out.is_empty() { Err("process snapshot returned no processes".into()) } else { Ok(out) }
}

/// `owner_process`: in_use if any of the owner's processes runs; unknown if the owner has no
/// mapping or the listing failed.
pub fn probe_owner(owner: Option<&str>, snap: &ProcessSnapshot) -> ProbeResult {
    const P: &str = "owner_process";
    let Some(owner) = owner else { return ProbeResult::new(P, ProbeState::Unknown, "rule has no owner") };
    let Some(exes) = owner_processes(owner) else { return ProbeResult::new(P, ProbeState::Unknown, format!("no process mapping for owner {owner}")) };
    match &snap.names {
        Err(e) => ProbeResult::new(P, ProbeState::Unknown, e.clone()),
        Ok(names) => match exes.iter().find(|e| names.contains(&e.to_ascii_lowercase())) {
            Some(exe) => ProbeResult::new(P, ProbeState::InUse, format!("{exe} is running")),
            None => ProbeResult::new(P, ProbeState::NotInUse, format!("{} not running", exes.join(", "))),
        },
    }
}

/// Live state of a single file's handles.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FileState {
    Free,
    Held,
    Gone,
    Unknown(String),
}

fn wide_z(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain([0]).collect()
}

/// Exclusive-open probe: no sharing, data-read access only to take part in sharing checks, never
/// following a reparse point. A sharing violation means another handle is open.
fn file_state(p: &Path) -> FileState {
    let z = wide_z(p);
    // SAFETY: NUL-terminated path; no security attributes or template.
    let h = unsafe { CreateFileW(z.as_ptr(), FILE_READ_DATA | SYNCHRONIZE, 0, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT, std::ptr::null_mut()) };
    if h != INVALID_HANDLE_VALUE {
        // SAFETY: handle opened above, closed once.
        unsafe { CloseHandle(h) };
        return FileState::Free;
    }
    let e = std::io::Error::last_os_error();
    match e.raw_os_error().map(|c| c as u32) {
        Some(ERROR_SHARING_VIOLATION) => FileState::Held,
        Some(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) => FileState::Gone,
        Some(ERROR_ACCESS_DENIED) => FileState::Unknown(format!("access denied: {}", p.display())),
        _ => FileState::Unknown(format!("{}: {e}", p.display())),
    }
}

/// Files at or under `root` without following links, bounded by count and time.
fn walk(root: &Path, mut keep: impl FnMut(&Path) -> bool) -> Result<Vec<PathBuf>, String> {
    let started = Instant::now();
    let meta = std::fs::symlink_metadata(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), meta)];
    let mut seen = 0usize;
    while let Some((p, m)) = stack.pop() {
        let attrs = m.file_attributes();
        if attrs & ATTR_REPARSE != 0 {
            if p == root {
                return Err(format!("{} is a link or reparse point", root.display()));
            }
            continue; // links below are boundaries; analysis already blocks candidates containing them
        }
        if m.is_dir() {
            let rd = std::fs::read_dir(&p).map_err(|e| format!("cannot list {}: {e}", p.display()))?;
            for e in rd {
                let e = e.map_err(|e| format!("cannot list {}: {e}", p.display()))?;
                let m = e.metadata().map_err(|e| format!("{}: {e}", p.display()))?;
                stack.push((e.path(), m));
            }
        } else {
            seen += 1;
            if seen > MAX_PROBE_FILES || started.elapsed() > PROBE_BUDGET {
                return Err(format!("more than {MAX_PROBE_FILES} files or {}s; not every file could be probed", PROBE_BUDGET.as_secs()));
            }
            if attrs & (ATTR_OFFLINE | ATTR_RECALL_ON_OPEN | ATTR_RECALL_ON_DATA_ACCESS) != 0 {
                return Err(format!("{} is not locally present; probing it could download it", p.display()));
            }
            if keep(&p) {
                out.push(p);
            }
        }
    }
    Ok(out)
}

fn ext_in(p: &Path, set: &[&str]) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| set.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// `open_file_handle` (every file) or `open_database_handle` (database/journal/log files only).
pub fn probe_open_files(root: &Path, databases_only: bool) -> ProbeResult {
    let name = if databases_only { "open_database_handle" } else { "open_file_handle" };
    let files = match walk(root, |p| !databases_only || ext_in(p, DB_EXT)) {
        Ok(f) => f,
        Err(e) => return ProbeResult::new(name, ProbeState::Unknown, e),
    };
    let mut unknown = None;
    for f in &files {
        match file_state(f) {
            FileState::Held => return ProbeResult::new(name, ProbeState::InUse, format!("{} is open in another process", f.display())),
            FileState::Unknown(why) => unknown = unknown.or(Some(why)),
            FileState::Free | FileState::Gone => {}
        }
    }
    match unknown {
        Some(why) => ProbeResult::new(name, ProbeState::Unknown, why),
        None => ProbeResult::new(name, ProbeState::NotInUse, format!("{} file(s) not open elsewhere", files.len())),
    }
}

/// `incomplete_download_handle`: an in-progress download marker that another process holds open.
pub fn probe_downloads(root: &Path) -> ProbeResult {
    const P: &str = "incomplete_download_handle";
    let markers = match walk(root, |p| ext_in(p, DOWNLOAD_MARKERS)) {
        Ok(m) => m,
        Err(e) => return ProbeResult::new(P, ProbeState::Unknown, e),
    };
    for m in &markers {
        match file_state(m) {
            FileState::Held => return ProbeResult::new(P, ProbeState::InUse, format!("download in progress: {}", m.display())),
            FileState::Unknown(why) => return ProbeResult::new(P, ProbeState::Unknown, why),
            _ => {}
        }
    }
    ProbeResult::new(P, ProbeState::NotInUse, format!("{} download marker(s), none held open", markers.len()))
}

/// `endpoint_security_path`: the path is inside an endpoint-security or management product's data.
pub fn probe_endpoint(path: &Path) -> ProbeResult {
    const P: &str = "endpoint_security_path";
    let hit = path.components().filter_map(|c| c.as_os_str().to_str()).map(str::to_ascii_lowercase).find(|c| ENDPOINT_SECURITY.contains(&c.as_str()));
    match hit {
        Some(c) => ProbeResult::new(P, ProbeState::InUse, format!("inside endpoint-security data ({c})")),
        None => ProbeResult::new(P, ProbeState::NotInUse, "not an endpoint-security location"),
    }
}

fn os_path(p: &RawPath) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    match p {
        RawPath::Windows(u) => Some(PathBuf::from(std::ffi::OsString::from_wide(u))),
        RawPath::Unix(_) => None,
    }
}

/// Run every probe a candidate's rules declared. Kinds this platform cannot answer are `unknown`.
pub fn probe_candidate(path: &RawPath, probes: &[String], owner: Option<&str>, snap: &ProcessSnapshot) -> Vec<ProbeResult> {
    let Some(p) = os_path(path) else { return probes.iter().map(|k| ProbeResult::new(k, ProbeState::Unknown, "not a Windows path")).collect() };
    let mut kinds: Vec<&str> = probes.iter().map(String::as_str).collect();
    kinds.sort_unstable();
    kinds.dedup();
    kinds
        .into_iter()
        .map(|k| match k {
            "owner_process" => probe_owner(owner, snap),
            "open_file_handle" => probe_open_files(&p, false),
            "open_database_handle" => probe_open_files(&p, true),
            "incomplete_download_handle" => probe_downloads(&p),
            "endpoint_security_path" => probe_endpoint(&p),
            other => ProbeResult::new(other, ProbeState::Unknown, "probe not available on Windows in this build"),
        })
        .collect()
}

/// Execution-time re-check for the catalog probe kinds the executor's own check does not cover
/// (it already re-checks the owner table it knows and exclusively opens the item itself).
/// `None` means nothing to add.
pub fn execution_check(t: &ActionTarget<'_>) -> Option<LiveCheck> {
    let path = os_path(t.path)?;
    for p in t.probes {
        match p.as_str() {
            "endpoint_security_path" => {
                let r = probe_endpoint(&path);
                if r.state != ProbeState::NotInUse {
                    return Some(LiveCheck::Protected(r.detail));
                }
            }
            "incomplete_download_handle" => {
                // A held marker next to the item means its download is still being written.
                if let Some(dir) = path.parent() {
                    let r = probe_downloads_shallow(dir);
                    match r.state {
                        ProbeState::InUse => return Some(LiveCheck::InUse),
                        ProbeState::Unknown => return Some(LiveCheck::Unknown(r.detail)),
                        ProbeState::NotInUse => {}
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Markers directly in `dir` only (execution check of one item stays cheap).
fn probe_downloads_shallow(dir: &Path) -> ProbeResult {
    const P: &str = "incomplete_download_handle";
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => return ProbeResult::new(P, ProbeState::Unknown, format!("cannot list {}: {e}", dir.display())),
    };
    for e in rd.flatten() {
        let p = e.path();
        if ext_in(&p, DOWNLOAD_MARKERS) && e.file_type().is_ok_and(|t| t.is_file()) && file_state(&p) == FileState::Held {
            return ProbeResult::new(P, ProbeState::InUse, format!("download in progress: {}", p.display()));
        }
    }
    ProbeResult::new(P, ProbeState::NotInUse, "no held download marker")
}

/// Owner-process re-check for owners the executor's own table does not map.
pub fn owner_running_check(owner: &str) -> Option<LiveCheck> {
    let r = probe_owner(Some(owner), &ProcessSnapshot::take());
    match r.state {
        ProbeState::InUse => Some(LiveCheck::OwnerRunning),
        ProbeState::Unknown => Some(LiveCheck::Unknown(r.detail)),
        ProbeState::NotInUse => None,
    }
}

#[cfg(test)]
mod tests;
