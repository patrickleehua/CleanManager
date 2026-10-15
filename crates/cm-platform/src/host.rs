//! Process liveness for task hosts (CLI-09). PID plus creation time identifies a host so a
//! reused PID is reported as the original host having exited.

use cm_core::host::{HostProbe, HostRef, Liveness};

pub struct OsHostProbe;

#[cfg(windows)]
mod imp {
    use super::*;
    use cm_core::time::{filetime_to_unix, utc_from_unix};
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, FILETIME, GetLastError, HANDLE, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    fn creation(handle: HANDLE) -> Option<String> {
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: valid process handle and writable FILETIME out-pointers.
        let ok = unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) };
        (ok != 0).then(|| {
            let ticks = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
            utc_from_unix(filetime_to_unix(ticks))
        })
    }

    pub fn current(kind: &str) -> HostRef {
        // SAFETY: the pseudo handle for the current process needs no closing.
        let started_at = creation(unsafe { GetCurrentProcess() }).unwrap_or_default();
        HostRef { kind: kind.into(), pid: std::process::id(), started_at }
    }

    pub fn liveness(host: &HostRef) -> Liveness {
        // SAFETY: plain query-only open; the handle is closed below.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, host.pid) };
        if handle.is_null() {
            // SAFETY: reads the calling thread's last error.
            return if unsafe { GetLastError() } == ERROR_INVALID_PARAMETER { Liveness::Exited } else { Liveness::Unknown };
        }
        let mut code = 0u32;
        // SAFETY: valid handle and out-pointer.
        let have_code = unsafe { GetExitCodeProcess(handle, &mut code) } != 0;
        let started = creation(handle);
        // SAFETY: handle came from OpenProcess.
        unsafe { CloseHandle(handle) };
        match (have_code, started) {
            (true, _) if code != STILL_ACTIVE as u32 => Liveness::Exited,
            (true, Some(s)) if s != host.started_at => Liveness::Exited,
            (true, Some(_)) => Liveness::Alive,
            _ => Liveness::Unknown,
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use cm_core::time::utc_from_unix;

    fn start_time(pid: u32) -> Option<String> {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: buffer is a properly sized proc_bsdinfo.
        let n = unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size) };
        (n == size).then(|| utc_from_unix(info.pbi_start_tvsec as i64))
    }

    pub fn current(kind: &str) -> HostRef {
        let pid = std::process::id();
        HostRef { kind: kind.into(), pid, started_at: start_time(pid).unwrap_or_default() }
    }

    pub fn liveness(host: &HostRef) -> Liveness {
        // SAFETY: signal 0 only checks for existence/permission.
        if unsafe { libc::kill(host.pid as libc::pid_t, 0) } != 0 {
            return if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) { Liveness::Exited } else { Liveness::Unknown };
        }
        match start_time(host.pid) {
            Some(s) if s == host.started_at => Liveness::Alive,
            Some(_) => Liveness::Exited,
            None => Liveness::Unknown,
        }
    }
}

impl HostProbe for OsHostProbe {
    fn current(&self, kind: &str) -> HostRef {
        imp::current(kind)
    }

    fn liveness(&self, host: &HostRef) -> Liveness {
        imp::liveness(host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_alive_with_a_creation_time() {
        let me = OsHostProbe.current("cli");
        assert!(!me.started_at.is_empty());
        assert_eq!(OsHostProbe.liveness(&me), Liveness::Alive);
    }

    #[test]
    fn reused_pid_with_other_start_time_is_not_the_original_host() {
        let mut me = OsHostProbe.current("cli");
        me.started_at = "2000-01-01T00:00:00Z".into();
        assert_eq!(OsHostProbe.liveness(&me), Liveness::Exited);
    }

    #[test]
    fn exited_child_process_is_reported_exited() {
        let mut child = if cfg!(windows) {
            std::process::Command::new("cmd").args(["/C", "exit 0"]).spawn().unwrap()
        } else {
            std::process::Command::new("true").spawn().unwrap()
        };
        let pid = child.id();
        let host = OsHostProbe.current("cli");
        let host = HostRef { pid, ..host };
        child.wait().unwrap();
        drop(child);
        assert_ne!(OsHostProbe.liveness(&host), Liveness::Alive);
    }
}
