//! Windows process list and the `process_end` action (parity 12.3, STS-04).
//!
//! Listing is read-only: a ToolHelp snapshot for PIDs, parents and names; per process a
//! `PROCESS_QUERY_LIMITED_INFORMATION` handle for the image path, creation time, CPU times, I/O transfer counters,
//! private memory and the OS critical flag. Rates come from two readings `window` apart. Signatures are verified
//! with WinVerifyTrust: the embedded Authenticode signature first, else the system catalog that lists the file's hash
//! (how in-box Windows binaries are signed); no UI, no revocation or network retrieval. A value that cannot be read
//! (protected processes, other users' processes) stays unknown with a note.
//!
//! Ending a process opens it once with query + terminate + synchronize rights and re-identifies it through that same
//! handle (creation time, image path, executable SHA-256, live protection) before `TerminateProcess`, so a reused PID
//! or a replaced executable is never ended. Critical system processes, CleanManager and security software are refused
//! before any terminate right is requested.

use std::collections::HashMap;
use std::os::windows::ffi::OsStrExt;
use std::time::{Duration, Instant};

use cm_core::path::RawPath;
use cm_core::plan::v2::ProcessEndAction;
use cm_core::process::{EndOutcome, ProcessActions, ProcessFacts, Protection, Signature, SignatureState, protection};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, FILETIME, GetLastError, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessIoCounters, GetProcessTimes, IO_COUNTERS, IsProcessCritical, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
};

fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

fn raw(path: &str) -> Option<RawPath> {
    RawPath::windows(std::ffi::OsStr::new(path).encode_wide().collect()).ok()
}

/// An owned process handle.
struct Proc(HANDLE);

impl Drop for Proc {
    fn drop(&mut self) {
        // SAFETY: handle from OpenProcess, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

impl Proc {
    fn open(pid: u32, access: u32) -> Result<Proc, u32> {
        // SAFETY: plain OpenProcess; a null handle is an error.
        let h = unsafe { OpenProcess(access, 0, pid) };
        if h.is_null() {
            // SAFETY: reads the calling thread's last error.
            return Err(unsafe { GetLastError() });
        }
        Ok(Proc(h))
    }

    fn image(&self) -> Option<String> {
        let mut buf = vec![0u16; 32_768];
        let mut len = buf.len() as u32;
        // SAFETY: buffer and length describe the same allocation.
        let ok = unsafe { QueryFullProcessImageNameW(self.0, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) } != 0;
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }

    /// (creation FILETIME ticks, kernel + user 100 ns units)
    fn times(&self) -> Option<(u64, u64)> {
        let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut e, mut k, mut u) = (z, z, z, z);
        // SAFETY: four valid out-parameters.
        let ok = unsafe { GetProcessTimes(self.0, &mut c, &mut e, &mut k, &mut u) } != 0;
        let t = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
        ok.then(|| (t(c), t(k) + t(u)))
    }

    fn io_bytes(&self) -> Option<u64> {
        let mut io: IO_COUNTERS = IO_COUNTERS { ReadOperationCount: 0, WriteOperationCount: 0, OtherOperationCount: 0, ReadTransferCount: 0, WriteTransferCount: 0, OtherTransferCount: 0 };
        // SAFETY: valid out-parameter.
        let ok = unsafe { GetProcessIoCounters(self.0, &mut io) } != 0;
        ok.then(|| io.ReadTransferCount.saturating_add(io.WriteTransferCount))
    }

    fn private_bytes(&self) -> Option<u64> {
        let mut m = PROCESS_MEMORY_COUNTERS_EX { cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
        // SAFETY: the EX structure begins with PROCESS_MEMORY_COUNTERS; cb states its full size.
        let ok = unsafe { K32GetProcessMemoryInfo(self.0, (&mut m as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(), m.cb) } != 0;
        ok.then_some(m.PrivateUsage as u64)
    }

    fn critical(&self) -> Option<bool> {
        let mut c = 0;
        // SAFETY: valid out-parameter.
        let ok = unsafe { IsProcessCritical(self.0, &mut c) } != 0;
        ok.then_some(c != 0)
    }
}

/// ToolHelp snapshot: (pid, parent pid, exe name).
fn snapshot() -> Result<Vec<(u32, u32, String)>, String> {
    // SAFETY: documented snapshot call; the handle is closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return Err("the process snapshot could not be taken".into());
    }
    let mut out = Vec::new();
    let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
    // SAFETY: entry has dwSize set; snapshot handle is valid.
    let mut more = unsafe { Process32FirstW(snap, &mut e) } != 0;
    while more {
        let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
        out.push((e.th32ProcessID, e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..len])));
        // SAFETY: as above.
        more = unsafe { Process32NextW(snap, &mut e) } != 0;
    }
    // SAFETY: closes the snapshot handle once.
    unsafe { CloseHandle(snap) };
    Ok(out)
}

/// Signer display name from a successful WinVerifyTrust state.
///
/// # Safety
/// `state` must be the `hWVTStateData` of a successful `WTD_STATEACTION_VERIFY` call that has not been closed.
unsafe fn signer_from_state(state: HANDLE) -> Option<String> {
    use windows_sys::Win32::Security::Cryptography::{CERT_NAME_SIMPLE_DISPLAY_TYPE, CertGetNameStringW};
    use windows_sys::Win32::Security::WinTrust::{WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData};
    // SAFETY: caller guarantees a live verified state; every pointer is checked before use.
    unsafe {
        let prov = WTHelperProvDataFromStateData(state);
        let signer = if prov.is_null() { std::ptr::null_mut() } else { WTHelperGetProvSignerFromChain(prov, 0, 0, 0) };
        if signer.is_null() || (*signer).csCertChain == 0 || (*signer).pasCertChain.is_null() {
            return None;
        }
        let cert = (*(*signer).pasCertChain).pCert;
        if cert.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 512];
        let n = CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, std::ptr::null(), buf.as_mut_ptr(), buf.len() as u32);
        (n > 1).then(|| String::from_utf16_lossy(&buf[..n as usize - 1]))
    }
}

/// Catalog signer of a file (in-box Windows binaries). No UI, no network.
fn catalog_signer(path: &str) -> Option<String> {
    use windows_sys::Win32::Security::Cryptography::BCRYPT_SHA256_ALGORITHM;
    use windows_sys::Win32::Security::Cryptography::Catalog::{
        CATALOG_INFO, CryptCATAdminAcquireContext2, CryptCATAdminCalcHashFromFileHandle2, CryptCATAdminEnumCatalogFromHash, CryptCATAdminReleaseCatalogContext,
        CryptCATAdminReleaseContext, CryptCATCatalogInfoFromContext,
    };
    use windows_sys::Win32::Security::WinTrust::{
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_CATALOG_INFO, WINTRUST_DATA, WINTRUST_DATA_0, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_CATALOG,
        WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE, WinVerifyTrust,
    };
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    let wpath = wide(path);
    // SAFETY: read-only open for hashing; closed below.
    let file = unsafe { CreateFileW(wpath.as_ptr(), 0x8000_0000, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut()) };
    if file == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut admin: isize = 0;
    let mut result = None;
    // SAFETY: out-parameter; SHA-256 catalogs; released below.
    if unsafe { CryptCATAdminAcquireContext2(&mut admin, std::ptr::null(), BCRYPT_SHA256_ALGORITHM, std::ptr::null(), 0) } != 0 {
        let mut size = 0u32;
        // SAFETY: size query, then a buffer of that size.
        unsafe { CryptCATAdminCalcHashFromFileHandle2(admin, file, &mut size, std::ptr::null_mut(), 0) };
        let mut hash = vec![0u8; size as usize];
        // SAFETY: buffer of `size` bytes.
        if size > 0 && unsafe { CryptCATAdminCalcHashFromFileHandle2(admin, file, &mut size, hash.as_mut_ptr(), 0) } != 0 {
            let mut prev: isize = 0;
            // SAFETY: valid admin context and hash buffer.
            let cat = unsafe { CryptCATAdminEnumCatalogFromHash(admin, hash.as_ptr(), size, 0, &mut prev) };
            if cat != 0 {
                let mut info = CATALOG_INFO { cbStruct: size_of::<CATALOG_INFO>() as u32, ..Default::default() };
                // SAFETY: catalog context from the enumeration above.
                if unsafe { CryptCATCatalogInfoFromContext(cat, &mut info, 0) } != 0 {
                    let tag: Vec<u16> = hash.iter().map(|b| format!("{b:02X}")).collect::<String>().encode_utf16().chain(std::iter::once(0)).collect();
                    let mut ci = WINTRUST_CATALOG_INFO {
                        cbStruct: size_of::<WINTRUST_CATALOG_INFO>() as u32,
                        pcwszCatalogFilePath: info.wszCatalogFile.as_ptr(),
                        pcwszMemberTag: tag.as_ptr(),
                        pcwszMemberFilePath: wpath.as_ptr(),
                        hMemberFile: file,
                        pbCalculatedFileHash: hash.as_mut_ptr(),
                        cbCalculatedFileHash: size,
                        hCatAdmin: admin,
                        ..Default::default()
                    };
                    let mut data = WINTRUST_DATA {
                        cbStruct: size_of::<WINTRUST_DATA>() as u32,
                        dwUIChoice: WTD_UI_NONE,
                        fdwRevocationChecks: WTD_REVOKE_NONE,
                        dwUnionChoice: WTD_CHOICE_CATALOG,
                        Anonymous: WINTRUST_DATA_0 { pCatalog: &mut ci },
                        dwStateAction: WTD_STATEACTION_VERIFY,
                        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL | WTD_REVOCATION_CHECK_NONE,
                        ..Default::default()
                    };
                    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
                    // SAFETY: fully initialized structures pointing at live buffers; state closed below.
                    let status = unsafe { WinVerifyTrust(std::ptr::null_mut(), &mut action, (&mut data as *mut WINTRUST_DATA).cast()) };
                    if status == 0 {
                        // SAFETY: state of the successful verify above, not yet closed.
                        result = unsafe { signer_from_state(data.hWVTStateData) };
                    }
                    data.dwStateAction = WTD_STATEACTION_CLOSE;
                    // SAFETY: closes the verify state.
                    unsafe { WinVerifyTrust(std::ptr::null_mut(), &mut action, (&mut data as *mut WINTRUST_DATA).cast()) };
                }
                // SAFETY: releases the catalog context from the enumeration.
                unsafe { CryptCATAdminReleaseCatalogContext(admin, cat, 0) };
            }
        }
        // SAFETY: releases the admin context acquired above.
        unsafe { CryptCATAdminReleaseContext(admin, 0) };
    }
    // SAFETY: closes the file handle once.
    unsafe { CloseHandle(file) };
    result
}

/// Verified signature of an executable: embedded Authenticode, else catalog.
pub fn signature_of(path: &str) -> Signature {
    if std::fs::metadata(path).is_err() {
        return Signature::unknown("the executable could not be read");
    }
    if let Some(p) = super::startup::authenticode_signer(path) {
        return Signature { state: SignatureState::Signed, publisher: Some(p), reason: None };
    }
    match catalog_signer(path) {
        Some(p) => Signature { state: SignatureState::CatalogSigned, publisher: Some(p), reason: None },
        None => Signature { state: SignatureState::Unsigned, publisher: None, reason: Some("no valid embedded or catalog signature".into()) },
    }
}

pub fn file_sha256(path: &str) -> Option<String> {
    std::fs::File::open(path).ok().and_then(|f| cm_core::adapter::sha256_hex(f).ok())
}

struct Reading {
    ppid: u32,
    name: String,
    image: Option<String>,
    created: Option<u64>,
    cpu: Option<u64>,
    io: Option<u64>,
    private: Option<u64>,
    critical: Option<bool>,
    note: Option<String>,
}

fn read_all() -> Result<HashMap<u32, Reading>, String> {
    let mut out = HashMap::new();
    for (pid, ppid, name) in snapshot()? {
        let mut r = Reading { ppid, name, image: None, created: None, cpu: None, io: None, private: None, critical: None, note: None };
        if pid != 0 {
            match Proc::open(pid, PROCESS_QUERY_LIMITED_INFORMATION) {
                Ok(h) => {
                    r.image = h.image();
                    let t = h.times();
                    r.created = t.map(|t| t.0).filter(|c| *c > 0);
                    r.cpu = t.map(|t| t.1);
                    r.io = h.io_bytes();
                    r.private = h.private_bytes();
                    r.critical = h.critical();
                }
                Err(ERROR_ACCESS_DENIED) => r.note = Some("access denied (protected or another user's process): only the name and PID are known".into()),
                Err(e) => r.note = Some(format!("process could not be opened (error {e})")),
            }
        }
        out.insert(pid, r);
    }
    Ok(out)
}

/// Signatures of distinct executables, verified on a few threads (WinVerifyTrust is thread-safe). Keys are lower case.
fn signatures_of(mut paths: Vec<String>) -> HashMap<String, Signature> {
    paths.sort_by_key(|p| p.to_lowercase());
    paths.dedup_by_key(|p| p.to_lowercase());
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 8);
    let chunk = paths.len().div_ceil(workers).max(1);
    std::thread::scope(|scope| {
        let handles: Vec<_> = paths.chunks(chunk).map(|c| scope.spawn(move || c.iter().map(|p| (p.to_lowercase(), signature_of(p))).collect::<Vec<_>>())).collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    })
}

/// Note for the whole list (shown once by the caller).
pub const DISK_NOTE: &str = "disk_bytes_per_sec is the process's read+write transfer rate (file, device and network I/O counters); Task Manager's Disk column uses ETW tracing and can differ";
pub const NETWORK_REASON: &str = "Per-process network rates need ETW kernel tracing (administrator); not sampled";

/// Read-only process list with rates over `window` and signatures (cached per executable path).
pub fn list(window: Duration) -> Result<Vec<ProcessFacts>, String> {
    list_with(window, true)
}

/// As [`list`]; `signatures: false` skips signature verification (alert sampling needs CPU only).
pub fn list_with(window: Duration, verify: bool) -> Result<Vec<ProcessFacts>, String> {
    let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
    let first = read_all()?;
    let t0 = Instant::now();
    std::thread::sleep(window);
    let second = read_all()?;
    let secs = t0.elapsed().as_secs_f64().max(0.001);
    let signatures: HashMap<String, Signature> = if verify { signatures_of(second.values().filter_map(|r| r.image.clone()).collect()) } else { HashMap::new() };
    let mut out = Vec::with_capacity(second.len());
    for (pid, r) in second {
        let before = first.get(&pid).filter(|b| b.created == r.created);
        let rate = |a: Option<u64>, b: Option<u64>| a.zip(b).map(|(x, y)| x.saturating_sub(y) as f64);
        let cpu_percent = rate(r.cpu, before.and_then(|b| b.cpu)).map(|d| ((d / 1e7) / secs / cpus * 100.0 * 10.0).round() / 10.0);
        let disk = rate(r.io, before.and_then(|b| b.io)).map(|d| (d / secs).round());
        let signature = match &r.image {
            Some(_) if !verify => Signature::unknown("not verified for this sample"),
            Some(p) => signatures.get(&p.to_lowercase()).cloned().unwrap_or_else(|| Signature::unknown("not verified")),
            None => Signature::unknown("the executable path could not be read"),
        };
        let mut notes: Vec<String> = r.note.into_iter().collect();
        if pid == 0 {
            notes.push("System Idle Process: its CPU share is the processor's idle time".into());
        }
        out.push(ProcessFacts {
            pid,
            parent_pid: (pid != 0 && r.ppid != 0).then_some(r.ppid),
            name: if pid == 0 { "System Idle Process".into() } else { r.name },
            image_path: r.image.as_deref().and_then(raw),
            started_at: r.created.map(|c| c.to_string()),
            cpu_percent: if pid == 0 { None } else { cpu_percent },
            memory_bytes: r.private,
            disk_bytes_per_sec: disk,
            network_bytes_per_sec: None,
            signature,
            os_critical: r.critical,
            notes,
        });
    }
    out.sort_by_key(|f| f.pid);
    Ok(out)
}

/// `%SystemRoot%`.
pub fn windows_dir() -> String {
    std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())
}

/// Facts of one live process from an open handle (for re-identification).
fn facts_from_handle(h: &Proc, pid: u32) -> (Option<u64>, Option<String>, ProcessFacts) {
    let created = h.times().map(|t| t.0);
    let image = h.image();
    let name = image.as_deref().and_then(|p| p.rsplit('\\').next()).unwrap_or("").to_owned();
    let signature = image.as_deref().map(signature_of).unwrap_or_else(|| Signature::unknown("no path"));
    let f = ProcessFacts {
        pid,
        parent_pid: None,
        name,
        image_path: image.as_deref().and_then(raw),
        started_at: created.map(|c| c.to_string()),
        cpu_percent: None,
        memory_bytes: None,
        disk_bytes_per_sec: None,
        network_bytes_per_sec: None,
        signature,
        os_critical: h.critical(),
        notes: vec![],
    };
    (created, image, f)
}

/// The Windows `process_end` backend.
pub struct WindowsProcesses;

impl ProcessActions for WindowsProcesses {
    fn end(&self, a: &ProcessEndAction, own_pid: u32) -> EndOutcome {
        if a.pid == own_pid {
            return EndOutcome::Protected(Protection::CleanManager);
        }
        // Static protection by the planned name first: no terminate right is ever requested for these.
        let planned = ProcessFacts {
            pid: a.pid,
            parent_pid: None,
            name: a.name.clone(),
            image_path: None,
            started_at: None,
            cpu_percent: None,
            memory_bytes: None,
            disk_bytes_per_sec: None,
            network_bytes_per_sec: None,
            signature: Signature::unknown("not read"),
            os_critical: None,
            notes: vec![],
        };
        if let Some(p) = protection(&planned, own_pid) {
            return EndOutcome::Protected(p);
        }
        let query = match Proc::open(a.pid, PROCESS_QUERY_LIMITED_INFORMATION) {
            Ok(h) => h,
            Err(ERROR_INVALID_PARAMETER) => return EndOutcome::AlreadyExited,
            Err(ERROR_ACCESS_DENIED) => return EndOutcome::PermissionRequired("access denied: the process runs elevated or as another user".into()),
            Err(e) => return EndOutcome::Failed(format!("the process could not be opened (error {e})")),
        };
        let (created, image, live) = facts_from_handle(&query, a.pid);
        if created.map(|c| c.to_string()).as_deref() != Some(a.started_at.as_str()) {
            return EndOutcome::Changed("the PID now belongs to a different process; the planned process already exited".into());
        }
        if let Some(p) = protection(&live, own_pid) {
            return EndOutcome::Protected(p);
        }
        let planned_image = a.image_path.as_ref().map(RawPath::display);
        if planned_image.as_deref().map(str::to_lowercase) != image.as_deref().map(str::to_lowercase) {
            return EndOutcome::Changed("the process's executable path differs from the reviewed plan".into());
        }
        if let (Some(expected), Some(p)) = (&a.image_sha256, &image)
            && file_sha256(p).as_deref() != Some(expected.as_str())
        {
            return EndOutcome::Changed("the executable changed since the plan was reviewed".into());
        }
        drop(query);
        let h = match Proc::open(a.pid, PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE) {
            Ok(h) => h,
            Err(ERROR_INVALID_PARAMETER) => return EndOutcome::AlreadyExited,
            Err(ERROR_ACCESS_DENIED) => return EndOutcome::PermissionRequired("access denied: ending this process needs the rights of its owner".into()),
            Err(e) => return EndOutcome::Failed(format!("the process could not be opened to end it (error {e})")),
        };
        // Re-identify through the very handle that will be used to end it (no PID-reuse window).
        if h.times().map(|t| t.0.to_string()).as_deref() != Some(a.started_at.as_str()) {
            return EndOutcome::Changed("the PID now belongs to a different process; the planned process already exited".into());
        }
        // SAFETY: handle with PROCESS_TERMINATE for the re-identified instance.
        if unsafe { TerminateProcess(h.0, 1) } == 0 {
            // SAFETY: reads the calling thread's last error.
            let e = unsafe { GetLastError() };
            return if e == ERROR_ACCESS_DENIED { EndOutcome::PermissionRequired("Windows refused to end the process".into()) } else { EndOutcome::Failed(format!("TerminateProcess failed (error {e})")) };
        }
        // SAFETY: waiting on our own process handle with SYNCHRONIZE.
        match unsafe { WaitForSingleObject(h.0, 5_000) } {
            WAIT_OBJECT_0 => EndOutcome::Ended,
            _ => EndOutcome::Unknown("the end request was sent but the process had not exited after 5 s".into()),
        }
    }

    fn running(&self, a: &ProcessEndAction) -> Option<bool> {
        match Proc::open(a.pid, PROCESS_QUERY_LIMITED_INFORMATION) {
            Ok(h) => {
                let same = h.times().map(|t| t.0.to_string()).as_deref() == Some(a.started_at.as_str());
                // An exited process whose handles are still open answers queries; its exit code tells.
                let mut code = 0u32;
                // SAFETY: valid out-parameter; QUERY_LIMITED_INFORMATION is enough for GetExitCodeProcess.
                let ok = unsafe { GetExitCodeProcess(h.0, &mut code) } != 0;
                if !ok {
                    return None;
                }
                Some(same && code == STILL_ACTIVE as u32)
            }
            Err(ERROR_INVALID_PARAMETER) => Some(false),
            Err(_) => None,
        }
    }
}

/// Whether any process runs an executable inside `dir` (`None` = some processes could not be read and none matched).
pub fn running_under(dir: &str) -> Option<bool> {
    let prefix = format!("{}\\", dir.trim_end_matches('\\').to_lowercase());
    let mut unreadable = false;
    for (pid, _, _) in snapshot().ok()? {
        if pid == 0 || pid == 4 {
            continue;
        }
        match Proc::open(pid, PROCESS_QUERY_LIMITED_INFORMATION) {
            Ok(h) => {
                if h.image().is_some_and(|p| p.to_lowercase().starts_with(&prefix)) {
                    return Some(true);
                }
            }
            Err(_) => unreadable = true,
        }
    }
    // Processes we cannot open run as SYSTEM or another user; a per-user app the user could update is never among them.
    let _ = unreadable;
    Some(false)
}

#[cfg(test)]
mod tests;
