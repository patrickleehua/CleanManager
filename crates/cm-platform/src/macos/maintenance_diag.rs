//! Read-only sample for the performance diagnosis on macOS (MNT-06), the counterpart of
//! `windows/maintenance_diag.rs`: process CPU over the window (libproc), memory load (host VM statistics through the
//! status sampler), free space of the local browsable volumes (`getmntinfo`), startup items (the launchd inventory) and
//! attached disk images (`hdiutil info`). Nothing is changed, no process is signalled.

use std::ffi::CStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cm_core::maintenance::{DiagnosisInput, ProcessSample, VolumeSpace};
use cm_core::status::{Availability, StatusSnapshot};

/// One diagnosis sample over `window` (at least one second). The process and memory samples run side by side, so the
/// whole call takes about one window plus the startup inventory.
pub fn sample(window: Duration) -> DiagnosisInput {
    let window = window.max(Duration::from_secs(1));
    let (processes, status) = std::thread::scope(|s| {
        let status = s.spawn(move || super::status::sample(window));
        (processes(window), status.join().ok())
    });
    let (items, _) = super::startup::inventory();
    let enabled = items.iter().filter(|i| i.state == cm_core::startup::StartupState::Enabled).count();
    DiagnosisInput {
        window_seconds: window.as_secs_f64(),
        processes,
        memory_load_percent: status.as_ref().and_then(memory_load_percent),
        // macOS has no commit limit (memory is compressed and swapped on demand).
        commit_used_percent: None,
        volumes: volumes(),
        startup_items: Some((enabled, items.len())),
        attached_images: super::installer::attached_image_paths().ok().map(|v| v.iter().map(|p| p.display().to_string()).collect()),
    }
}

/// Processes of this user with CPU over the window (in percent of one logical CPU) and since they started. Other users'
/// processes have no readable usage and are left out, as are zombies.
fn processes(window: Duration) -> Vec<ProcessSample> {
    let Ok(list) = super::processes::list_with(window, false) else { return Vec::new() };
    let ncpu = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
    let now_us = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_micros() as u64).unwrap_or(0);
    list.iter()
        .filter(|p| !p.zombie)
        .filter_map(|p| {
            let f = &p.facts;
            let window_core_percent = f.cpu_percent? * ncpu;
            let started_us = f.started_at.as_deref()?.parse::<u64>().ok()?;
            let age = now_us.saturating_sub(started_us) as f64 / 1e6;
            let usage = super::processes::usage(f.pid as i32).ok()?;
            let lifetime_core_percent = if age > 0.0 { usage.cpu_ns as f64 / 1e9 / age * 100.0 } else { 0.0 };
            Some(ProcessSample {
                pid: f.pid,
                name: f.name.clone(),
                image: f.image_path.as_ref().map(|r| r.display().to_string()),
                window_core_percent,
                lifetime_core_percent,
                age_seconds: age as u64,
                working_set_bytes: usage.footprint,
            })
        })
        .collect()
}

/// Used physical memory in percent, from the status sampler's total and available memory.
fn memory_load_percent(s: &StatusSnapshot) -> Option<f64> {
    let value = |name: &str| s.metrics.iter().find(|m| m.name == name && m.availability == Availability::Available).and_then(|m| m.value);
    let (total, available) = (value("memory.total_bytes")?, value("memory.available_bytes")?);
    (total > 0.0).then(|| ((total - available) / total * 100.0).clamp(0.0, 100.0))
}

/// Local volumes a user sees in the Finder (the system's internal APFS volumes are `MNT_DONTBROWSE`).
fn volumes() -> Vec<VolumeSpace> {
    let mut buf: *mut libc::statfs = std::ptr::null_mut();
    // SAFETY: getmntinfo stores a pointer to its own static buffer in `buf` and returns the entry count; MNT_NOWAIT
    // reads cached statistics without blocking on unresponsive file systems.
    let n = unsafe { libc::getmntinfo(&mut buf, libc::MNT_NOWAIT) };
    if n <= 0 || buf.is_null() {
        return Vec::new();
    }
    // SAFETY: getmntinfo returned `n` initialized entries at `buf`, valid until the next call on this thread.
    let entries = unsafe { std::slice::from_raw_parts(buf, n as usize) };
    entries
        .iter()
        .filter(|s| s.f_flags & libc::MNT_LOCAL as u32 != 0 && s.f_flags & libc::MNT_DONTBROWSE as u32 == 0 && s.f_blocks > 0)
        .map(|s| {
            // SAFETY: f_mntonname is a NUL-terminated C string inside the fixed-size array.
            let display = unsafe { CStr::from_ptr(s.f_mntonname.as_ptr()) }.to_string_lossy().into_owned();
            let block = u64::from(s.f_bsize);
            VolumeSpace { display, free_bytes: s.f_bavail.saturating_mul(block), total_bytes: s.f_blocks.saturating_mul(block) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sampler only reads: this process is still alive afterwards and appears with plausible numbers.
    #[test]
    fn sampling_reads_processes_memory_and_volumes() {
        let s = sample(Duration::from_secs(1));
        let me = s.processes.iter().find(|p| p.pid == std::process::id()).expect("this process is sampled");
        assert!(me.window_core_percent >= 0.0 && me.lifetime_core_percent >= 0.0 && me.working_set_bytes > 0);
        assert!(s.memory_load_percent.is_some_and(|m| (0.0..=100.0).contains(&m)), "{:?}", s.memory_load_percent);
        assert!(s.volumes.iter().any(|v| v.display == "/" && v.total_bytes > 0), "{:?}", s.volumes);
        assert!(s.startup_items.is_some_and(|(enabled, total)| enabled <= total && total > 0));
        let _ = cm_core::maintenance::diagnose(&s);
    }
}
