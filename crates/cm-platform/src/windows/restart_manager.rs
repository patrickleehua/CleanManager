//! Which processes hold files open (MTR-04), through the Windows Restart Manager.
//!
//! Read-only: a session is started, the file paths are registered and the affected-process list is read; nothing is
//! shut down or restarted (`RmShutdown`/`RmRestart` are never called) and no file is opened or read, so cloud
//! placeholders are not hydrated. A failure means "unknown holder", never "not in use".

use cm_core::path::RawPath;
use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
use windows_sys::Win32::System::RestartManager::{CCH_RM_SESSION_KEY, RM_PROCESS_INFO, RmEndSession, RmGetList, RmRegisterResources, RmStartSession};

/// A process holding one of the queried files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    pub pid: u32,
    /// Friendly application name as Restart Manager reports it.
    pub app_name: String,
    /// Image file name (`notepad.exe`), when the process can be queried.
    pub image: Option<String>,
    /// Service short name, for services.
    pub service: Option<String>,
}

impl Holder {
    /// `notepad.exe (Notepad, pid 1234)`.
    pub fn describe(&self) -> String {
        let main = self.image.clone().unwrap_or_else(|| self.app_name.clone());
        let mut extra = Vec::new();
        if self.image.is_some() && !self.app_name.is_empty() && !self.app_name.eq_ignore_ascii_case(&main) {
            extra.push(self.app_name.clone());
        }
        if let Some(s) = &self.service {
            extra.push(format!("service {s}"));
        }
        extra.push(format!("pid {}", self.pid));
        format!("{main} ({})", extra.join(", "))
    }
}

fn utf16_z(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

struct Session(u32);

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: handle from a successful RmStartSession, ended once.
        unsafe { RmEndSession(self.0) };
    }
}

fn image_name(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
    // SAFETY: limited query access only; the handle is closed below.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return None;
    }
    let mut buf = vec![0u16; 32_768];
    let mut len = buf.len() as u32;
    // SAFETY: buffer of `len` characters.
    let ok = unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) };
    // SAFETY: handle from OpenProcess, closed once.
    unsafe { CloseHandle(h) };
    if ok == 0 {
        return None;
    }
    let full = String::from_utf16_lossy(&buf[..len as usize]);
    full.rsplit('\\').next().map(str::to_owned).filter(|s| !s.is_empty())
}

/// Processes holding any of `paths` open. `Err` when Restart Manager could not answer.
pub fn holders(paths: &[RawPath]) -> Result<Vec<Holder>, String> {
    let files: Vec<Vec<u16>> = paths
        .iter()
        .filter_map(|p| match p {
            RawPath::Windows(u) => Some(u.iter().copied().chain(std::iter::once(0)).collect()),
            RawPath::Unix(_) => None,
        })
        .collect();
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
    let mut handle = 0u32;
    // SAFETY: key buffer of CCH_RM_SESSION_KEY + 1 characters; valid out-pointer.
    let rc = unsafe { RmStartSession(&mut handle, 0, key.as_mut_ptr()) };
    if rc != ERROR_SUCCESS {
        return Err(format!("RmStartSession failed ({rc})"));
    }
    let session = Session(handle);
    let ptrs: Vec<*const u16> = files.iter().map(|f| f.as_ptr()).collect();
    // SAFETY: `ptrs` points at NUL-terminated paths that outlive the call; no applications or services.
    let rc = unsafe { RmRegisterResources(session.0, ptrs.len() as u32, ptrs.as_ptr(), 0, std::ptr::null(), 0, std::ptr::null()) };
    if rc != ERROR_SUCCESS {
        return Err(format!("RmRegisterResources failed ({rc})"));
    }
    let mut infos: Vec<RM_PROCESS_INFO> = Vec::new();
    for _ in 0..4 {
        let mut needed = 0u32;
        let mut count = infos.len() as u32;
        let mut reasons = 0u32;
        // SAFETY: `infos` holds `count` zeroable records; out-pointers are valid.
        let rc = unsafe { RmGetList(session.0, &mut needed, &mut count, if infos.is_empty() { std::ptr::null_mut() } else { infos.as_mut_ptr() }, &mut reasons) };
        match rc {
            ERROR_SUCCESS => {
                infos.truncate(count as usize);
                let mut out: Vec<Holder> = infos
                    .iter()
                    .map(|i| {
                        let pid = i.Process.dwProcessId;
                        let service = Some(utf16_z(&i.strServiceShortName)).filter(|s| !s.is_empty());
                        Holder { pid, app_name: utf16_z(&i.strAppName), image: image_name(pid), service }
                    })
                    .collect();
                out.sort_by_key(|h| h.pid);
                out.dedup_by_key(|h| h.pid);
                return Ok(out);
            }
            ERROR_MORE_DATA => {
                // SAFETY: RM_PROCESS_INFO is plain data; zero is a valid bit pattern.
                infos = (0..needed.max(1) + 4).map(|_| unsafe { std::mem::zeroed() }).collect();
            }
            other => return Err(format!("RmGetList failed ({other})")),
        }
    }
    Err("RmGetList kept growing".into())
}

#[cfg(test)]
mod tests {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;

    use super::*;

    #[test]
    fn an_open_file_reports_this_process_and_a_closed_one_reports_nobody() {
        let dir = std::env::temp_dir().join(format!("cm-rm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("locked.log");
        std::fs::write(&f, "held").unwrap();
        let raw = RawPath::windows(f.as_os_str().encode_wide().collect()).unwrap();
        assert_eq!(holders(std::slice::from_ref(&raw)).unwrap(), vec![], "nobody holds a closed file");
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&f).unwrap();
        let found = holders(std::slice::from_ref(&raw)).unwrap();
        let me = found.iter().find(|h| h.pid == std::process::id()).unwrap_or_else(|| panic!("this process holds it: {found:?}"));
        let image = me.image.clone().unwrap();
        let exe = std::env::current_exe().unwrap();
        assert!(image.eq_ignore_ascii_case(&exe.file_name().unwrap().to_string_lossy()), "{image}");
        assert!(me.describe().contains(&format!("pid {}", std::process::id())));
        drop(held);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
