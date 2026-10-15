//! Read-only performance diagnosis input (parity MNT-06): two CPU-time readings of every process a window apart,
//! memory and commit load, fixed-volume free space, startup item counts and attached disk images. Processes are only
//! opened with `PROCESS_QUERY_LIMITED_INFORMATION`; nothing is ended, suspended or changed.

use std::collections::BTreeMap;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::Duration;

use cm_core::maintenance::{DiagnosisInput, ProcessSample, VolumeSpace};
use windows_sys::Win32::Foundation::FILETIME;

fn ft(f: FILETIME) -> u64 {
    (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)
}

struct Reading {
    cpu_100ns: u64,
    created_100ns: u64,
    image: Option<String>,
    working_set: u64,
}

fn read(pid: u32) -> Option<Reading> {
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
    // SAFETY: limited query access only.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return None;
    }
    // SAFETY: fresh handle owned from here on.
    let h = unsafe { OwnedHandle::from_raw_handle(h) };
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut c, mut e, mut k, mut u) = (zero, zero, zero, zero);
    // SAFETY: valid handle and out-pointers.
    if unsafe { GetProcessTimes(h.as_raw_handle(), &mut c, &mut e, &mut k, &mut u) } == 0 {
        return None;
    }
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: buffer of `len` characters.
    let image = (unsafe { QueryFullProcessImageNameW(h.as_raw_handle(), PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) } != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]));
    // SAFETY: plain data; size set below.
    let mut mem: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    mem.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // SAFETY: valid handle; buffer of the declared size.
    let ws = if unsafe { K32GetProcessMemoryInfo(h.as_raw_handle(), &mut mem, mem.cb) } != 0 { mem.WorkingSetSize as u64 } else { 0 };
    Some(Reading { cpu_100ns: ft(k) + ft(u), created_100ns: ft(c), image, working_set: ws })
}

fn now_100ns() -> u64 {
    let mut f = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    // SAFETY: valid out-pointer.
    unsafe { windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime(&mut f) };
    ft(f)
}

fn processes(window: Duration) -> Vec<ProcessSample> {
    let first: BTreeMap<u32, (String, Reading)> = super::maintenance::process_list().into_iter().filter(|(pid, _)| *pid != 0).filter_map(|(pid, n)| read(pid).map(|r| (pid, (n, r)))).collect();
    let t0 = now_100ns();
    std::thread::sleep(window);
    let t1 = now_100ns();
    let span = t1.saturating_sub(t0).max(1) as f64;
    let mut out = Vec::new();
    for (pid, name) in super::maintenance::process_list() {
        let (Some((n0, r0)), Some(r1)) = (first.get(&pid), read(pid)) else { continue };
        // The same process instance only (a reused PID has another creation time).
        if *n0 != name || r0.created_100ns != r1.created_100ns {
            continue;
        }
        let age = t1.saturating_sub(r1.created_100ns).max(1);
        out.push(ProcessSample {
            pid,
            name,
            image: r1.image,
            window_core_percent: r1.cpu_100ns.saturating_sub(r0.cpu_100ns) as f64 * 100.0 / span,
            lifetime_core_percent: r1.cpu_100ns as f64 * 100.0 / age as f64,
            age_seconds: age / 10_000_000,
            working_set_bytes: r1.working_set,
        });
    }
    out
}

fn memory() -> (Option<f64>, Option<f64>) {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: plain data; length set below.
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: valid buffer of the declared length.
    if unsafe { GlobalMemoryStatusEx(&mut m) } == 0 {
        return (None, None);
    }
    let commit = (m.ullTotalPageFile > 0).then(|| (m.ullTotalPageFile - m.ullAvailPageFile) as f64 * 100.0 / m.ullTotalPageFile as f64);
    (Some(f64::from(m.dwMemoryLoad)), commit)
}

fn volumes() -> Vec<VolumeSpace> {
    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives};
    const DRIVE_FIXED: u32 = 3;
    // SAFETY: no arguments.
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let root: Vec<u16> = format!("{}:\\", char::from(b'A' + i as u8)).encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: NUL-terminated root.
        if unsafe { GetDriveTypeW(root.as_ptr()) } != DRIVE_FIXED {
            continue;
        }
        let (mut free, mut total, mut total_free) = (0u64, 0u64, 0u64);
        // SAFETY: NUL-terminated root; valid out-pointers.
        if unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut free, &mut total, &mut total_free) } != 0 {
            out.push(VolumeSpace { display: format!("{}:", char::from(b'A' + i as u8)), free_bytes: free, total_bytes: total });
        }
    }
    out
}

fn attached_images() -> Option<Vec<String>> {
    use windows_sys::Win32::Storage::Vhd::GetAllAttachedVirtualDiskPhysicalPaths;
    let mut size = 0u32;
    // SAFETY: size query with a null buffer.
    let rc = unsafe { GetAllAttachedVirtualDiskPhysicalPaths(&mut size, std::ptr::null_mut()) };
    if rc == 0 && size == 0 {
        return Some(Vec::new());
    }
    if rc != 0 && rc != 122 && rc != 234 {
        return None;
    }
    let mut buf = vec![0u16; (size as usize).div_ceil(2).max(1)];
    // SAFETY: buffer of `size` bytes.
    if unsafe { GetAllAttachedVirtualDiskPhysicalPaths(&mut size, buf.as_mut_ptr()) } != 0 {
        return None;
    }
    Some(buf.split(|&c| c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect())
}

/// Sample everything the diagnosis looks at; `window` is the CPU measurement interval.
pub fn sample(window: Duration) -> DiagnosisInput {
    let (memory_load_percent, commit_used_percent) = memory();
    let (items, _) = super::startup::inventory();
    let enabled = items.iter().filter(|i| i.state == cm_core::startup::StartupState::Enabled).count();
    DiagnosisInput {
        window_seconds: window.as_secs_f64(),
        processes: processes(window),
        memory_load_percent,
        commit_used_percent,
        volumes: volumes(),
        startup_items: Some((enabled, items.len())),
        attached_images: attached_images(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sampler only reads: this process is still alive afterwards and appears with plausible numbers.
    #[test]
    fn sampling_reads_processes_memory_and_volumes() {
        let s = sample(Duration::from_millis(300));
        let me = s.processes.iter().find(|p| p.pid == std::process::id()).expect("this process is sampled");
        assert!(me.window_core_percent >= 0.0 && me.lifetime_core_percent >= 0.0);
        assert!(s.memory_load_percent.is_some_and(|m| (0.0..=100.0).contains(&m)));
        assert!(s.volumes.iter().any(|v| v.total_bytes > 0));
        let _ = cm_core::maintenance::diagnose(&s);
    }
}
