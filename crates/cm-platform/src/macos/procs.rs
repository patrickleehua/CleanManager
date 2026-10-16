//! Process snapshot and open-file holders through libproc (`<libproc.h>`, `<sys/proc_info.h>`).
//!
//! - [`snapshot`] lists every pid (`proc_listallpids`) with its parent, owner, start time, state
//!   (zombies included) and executable path. The full BSD record (`PROC_PIDTBSDINFO`) is only
//!   readable for the caller's own processes unless privileged; for the others the short record
//!   is used and the start time stays unknown (`limited`), never guessed.
//! - [`FdIndex`] is a per-operation cache of which processes hold which `(st_dev, st_ino)` open,
//!   from `PROC_PIDLISTFDS` + `proc_pidfdinfo(PROC_PIDFDVNODEPATHINFO)` plus each process's
//!   working directory. Build it once per operation and query it per object; it is a snapshot
//!   and goes stale. Processes whose descriptors cannot be read (other users' processes without
//!   privileges) are counted, so a query reports "unknown beyond N processes", never "no holders".
//!   Memory-mapped files that have no descriptor are not reported.

// Consumers (in-use checks before trash/uninstall) land with the later macOS work packages.
#![allow(dead_code)]

use std::collections::HashMap;
use std::ffi::{c_int, c_void};
use std::mem::size_of;

use cm_core::time::utc_from_unix;

use super::sys::errno;

/// `PROC_PIDFDVNODEPATHINFO` (`<sys/proc_info.h>`); not exported by the `libc` crate.
const PROC_PIDFDVNODEPATHINFO: c_int = 2;

/// `arg` of the BSD-info flavors that makes the kernel also look up zombies (`proc_info.c`).
const FIND_ZOMBIES: u64 = 1;

/// `struct proc_fileinfo` (`<sys/proc_info.h>`).
#[repr(C)]
#[derive(Clone, Copy)]
struct ProcFileInfo {
    fi_openflags: u32,
    fi_status: u32,
    fi_offset: i64,
    fi_type: i32,
    fi_guardflags: u32,
}

/// `struct vnode_fdinfowithpath { struct proc_fileinfo pfi; struct vnode_info_path pvip; }`
#[repr(C)]
struct VnodeFdInfoWithPath {
    pfi: ProcFileInfo,
    pvip: libc::vnode_info_path,
}

// `PROC_PIDFDVNODEPATHINFO_SIZE` is `sizeof(struct vnode_fdinfowithpath)`: 24 + (136 + 16) + MAXPATHLEN.
const _: () = assert!(size_of::<ProcFileInfo>() == 24);
const _: () = assert!(size_of::<VnodeFdInfoWithPath>() == 24 + 152 + 1024);

/// One process as the kernel reported it at snapshot time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcInfo {
    pub pid: i32,
    pub ppid: i32,
    /// Effective user.
    pub uid: u32,
    /// Start time as Unix `(seconds, microseconds)`; `None` when withheld (`limited`).
    pub start: Option<(u64, u64)>,
    /// `p_stat` (`SIDL`, `SRUN`, `SSLEEP`, `SSTOP`, `SZOMB`).
    pub status: u32,
    pub zombie: bool,
    /// `pbi_name` (up to 32 bytes) or, when empty, `pbi_comm` (16 bytes); lossy UTF-8 for display.
    pub name: String,
    /// Executable path bytes; `None` when unavailable (zombies, exited, some system processes).
    pub path: Option<Vec<u8>>,
    /// Only the short record was readable (another user's process without privileges).
    pub limited: bool,
}

impl ProcInfo {
    /// Start time in the text form `host.rs` records for task hosts.
    pub(crate) fn started_at(&self) -> Option<String> {
        self.start.and_then(|(s, _)| i64::try_from(s).ok()).map(utc_from_unix)
    }
}

/// All processes at one moment.
#[derive(Debug, Clone, Default)]
pub(crate) struct Snapshot {
    pub procs: Vec<ProcInfo>,
    /// Processes seen only through the short record (start time unknown).
    pub limited: usize,
}

fn c_text(chars: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = chars.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Every pid currently known to the kernel (pid 0, the kernel, excluded).
pub(crate) fn list_pids() -> Result<Vec<i32>, i32> {
    let mut cap = 1024usize;
    loop {
        let mut pids = vec![0 as libc::pid_t; cap];
        let bytes = c_int::try_from(cap * size_of::<libc::pid_t>()).map_err(|_| libc::ENOMEM)?;
        // SAFETY: writable buffer of `bytes` bytes; returns the number of pids written (0 on failure,
        // since every system has launchd).
        let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast::<c_void>(), bytes) };
        if n <= 0 {
            return Err(errno());
        }
        let n = n as usize;
        if n < cap {
            pids.truncate(n);
            pids.retain(|&p| p > 0);
            return Ok(pids);
        }
        // Full buffer: the list may have been cut short; grow and ask again.
        cap = cap.checked_mul(2).filter(|&c| c <= 1 << 22).ok_or(libc::ENOMEM)?;
    }
}

fn bsd_info(pid: i32) -> Result<libc::proc_bsdinfo, i32> {
    // SAFETY: zeroed POD out-parameter of the declared type.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = size_of::<libc::proc_bsdinfo>() as c_int;
    // SAFETY: buffer is a properly sized proc_bsdinfo. `arg = 1` also finds zombies.
    let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, FIND_ZOMBIES, (&mut info as *mut libc::proc_bsdinfo).cast(), size) };
    if n == size { Ok(info) } else { Err(errno()) }
}

fn short_info(pid: i32) -> Result<libc::proc_bsdshortinfo, i32> {
    // SAFETY: zeroed POD out-parameter of the declared type.
    let mut info: libc::proc_bsdshortinfo = unsafe { std::mem::zeroed() };
    let size = size_of::<libc::proc_bsdshortinfo>() as c_int;
    // SAFETY: buffer is a properly sized proc_bsdshortinfo. `arg = 1` also finds zombies.
    let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDT_SHORTBSDINFO, FIND_ZOMBIES, (&mut info as *mut libc::proc_bsdshortinfo).cast(), size) };
    if n == size { Ok(info) } else { Err(errno()) }
}

/// Executable path of `pid` as raw bytes.
pub(crate) fn exe_path(pid: i32) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: writable buffer of the documented maximum size.
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    Some(buf)
}

/// One process. `Err(ESRCH)` when it no longer exists.
pub(crate) fn process(pid: i32) -> Result<ProcInfo, i32> {
    match bsd_info(pid) {
        Ok(i) => {
            let name = if i.pbi_name[0] != 0 { c_text(&i.pbi_name) } else { c_text(&i.pbi_comm) };
            Ok(ProcInfo {
                pid,
                ppid: i.pbi_ppid as i32,
                uid: i.pbi_uid,
                start: Some((i.pbi_start_tvsec, i.pbi_start_tvusec)),
                status: i.pbi_status,
                zombie: i.pbi_status == libc::SZOMB,
                name,
                path: exe_path(pid),
                limited: false,
            })
        }
        Err(libc::ESRCH) => Err(libc::ESRCH),
        Err(first) => {
            let i = short_info(pid).map_err(|e| if e == libc::ESRCH { e } else { first })?;
            Ok(ProcInfo {
                pid,
                ppid: i.pbsi_ppid as i32,
                uid: i.pbsi_uid,
                start: None,
                status: i.pbsi_status,
                zombie: i.pbsi_status == libc::SZOMB,
                name: c_text(&i.pbsi_comm),
                path: exe_path(pid),
                limited: true,
            })
        }
    }
}

/// All processes. Processes that exit while the snapshot is taken are left out.
pub(crate) fn snapshot() -> Result<Snapshot, i32> {
    let mut snap = Snapshot::default();
    for pid in list_pids()? {
        match process(pid) {
            Ok(p) => {
                snap.limited += usize::from(p.limited);
                snap.procs.push(p);
            }
            Err(libc::ESRCH) => {}
            Err(_) => snap.limited += 1,
        }
    }
    Ok(snap)
}

/// How a process holds an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hold {
    /// An open descriptor.
    Fd(i32),
    /// The process's current working directory.
    Cwd,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Holder {
    pub pid: i32,
    pub name: String,
    pub path: Option<Vec<u8>>,
    pub hold: Hold,
}

/// Holders of one object, with the limit of what could be seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HolderReport {
    pub holders: Vec<Holder>,
    /// Processes whose descriptors were not readable (another user's, or protected); a holder
    /// among them is invisible. Zero only when every process was inspected.
    pub uninspected: usize,
}

impl HolderReport {
    /// Whether "no holders" is a confirmed fact rather than the absence of visible ones.
    pub(crate) fn complete(&self) -> bool {
        self.uninspected == 0
    }
}

/// Open vnodes of every inspectable process, keyed by `(st_dev, st_ino)`.
#[derive(Debug, Default)]
pub(crate) struct FdIndex {
    by_object: HashMap<(i32, u64), Vec<Holder>>,
    pub inspected: usize,
    /// Pids whose descriptors could not be read.
    pub uninspected: Vec<i32>,
}

fn list_fds(pid: i32) -> Result<Vec<libc::proc_fdinfo>, i32> {
    // SAFETY: NULL buffer asks for the size in bytes the descriptor table needs now.
    let need = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
    if need <= 0 {
        return Err(errno());
    }
    // Room for descriptors opened between the two calls.
    let cap = need as usize / size_of::<libc::proc_fdinfo>() + 64;
    let mut fds = vec![libc::proc_fdinfo { proc_fd: 0, proc_fdtype: 0 }; cap];
    let bytes = c_int::try_from(cap * size_of::<libc::proc_fdinfo>()).map_err(|_| libc::ENOMEM)?;
    // SAFETY: writable array of `cap` proc_fdinfo records.
    let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, fds.as_mut_ptr().cast(), bytes) };
    if n <= 0 {
        return Err(errno());
    }
    fds.truncate(n as usize / size_of::<libc::proc_fdinfo>());
    Ok(fds)
}

/// `(st_dev, st_ino)` of a vnode descriptor; `None` when it closed or changed meanwhile.
fn fd_vnode(pid: i32, fd: i32) -> Option<(i32, u64)> {
    // SAFETY: zeroed POD out-parameter of the declared type.
    let mut info: VnodeFdInfoWithPath = unsafe { std::mem::zeroed() };
    let size = size_of::<VnodeFdInfoWithPath>() as c_int;
    // SAFETY: buffer is a properly sized vnode_fdinfowithpath.
    let n = unsafe { libc::proc_pidfdinfo(pid, fd, PROC_PIDFDVNODEPATHINFO, (&mut info as *mut VnodeFdInfoWithPath).cast(), size) };
    let st = &info.pvip.vip_vi.vi_stat;
    (n == size).then_some((st.vst_dev as i32, st.vst_ino))
}

fn cwd_vnode(pid: i32) -> Option<(i32, u64)> {
    // SAFETY: zeroed POD out-parameter of the declared type.
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = size_of::<libc::proc_vnodepathinfo>() as c_int;
    // SAFETY: buffer is a properly sized proc_vnodepathinfo.
    let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, 0, (&mut info as *mut libc::proc_vnodepathinfo).cast(), size) };
    let st = &info.pvi_cdir.vip_vi.vi_stat;
    (n == size && st.vst_ino != 0).then_some((st.vst_dev as i32, st.vst_ino))
}

impl FdIndex {
    /// One pass over every process. Only fails when the process list itself is unavailable.
    pub(crate) fn build() -> Result<FdIndex, i32> {
        let started = std::time::Instant::now();
        let mut index = FdIndex::default();
        for pid in list_pids()? {
            let fds = match list_fds(pid) {
                Ok(f) => f,
                // Exited meanwhile: holds nothing.
                Err(libc::ESRCH) => continue,
                Err(_) => {
                    index.uninspected.push(pid);
                    continue;
                }
            };
            index.inspected += 1;
            let mut held: Vec<((i32, u64), Hold)> = fds
                .iter()
                .filter(|f| f.proc_fdtype == libc::PROX_FDTYPE_VNODE as u32)
                .filter_map(|f| fd_vnode(pid, f.proc_fd).map(|k| (k, Hold::Fd(f.proc_fd))))
                .collect();
            held.extend(cwd_vnode(pid).map(|k| (k, Hold::Cwd)));
            if held.is_empty() {
                continue;
            }
            let (name, path) = match process(pid) {
                Ok(p) => (p.name, p.path),
                Err(_) => (String::new(), exe_path(pid)),
            };
            for (key, hold) in held {
                index.by_object.entry(key).or_default().push(Holder { pid, name: name.clone(), path: path.clone(), hold });
            }
        }
        cm_core::debug_trace!(
            "process",
            "open-file index: {} processes inspected, {} not inspectable, {} objects in {}ms",
            index.inspected,
            index.uninspected.len(),
            index.by_object.len(),
            started.elapsed().as_millis()
        );
        Ok(index)
    }

    /// Who holds the object `(dev, ino)` open, as of [`FdIndex::build`].
    pub(crate) fn holders(&self, dev: i32, ino: u64) -> HolderReport {
        HolderReport { holders: self.by_object.get(&(dev, ino)).cloned().unwrap_or_default(), uninspected: self.uninspected.len() }
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;

    use cm_core::host::HostProbe;

    use super::*;
    use crate::host::OsHostProbe;
    use crate::macos::testfs::Fixture;

    fn is_root() -> bool {
        // SAFETY: plain getter.
        unsafe { libc::geteuid() == 0 }
    }

    #[test]
    fn current_process_matches_the_host_probe() {
        let pid = std::process::id() as i32;
        let me = process(pid).unwrap();
        assert!(!me.limited && !me.zombie);
        assert_eq!(me.started_at(), Some(OsHostProbe.current("cli").started_at));
        // SAFETY: plain getters.
        let (ppid, uid) = unsafe { (libc::getppid(), libc::geteuid()) };
        assert_eq!((me.ppid, me.uid), (ppid, uid));
        let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
        assert_eq!(me.path.as_deref(), Some(std::os::unix::ffi::OsStrExt::as_bytes(exe.as_os_str())));
        assert!(!me.name.is_empty());
    }

    #[test]
    fn snapshot_lists_this_process_and_launchd_and_flags_withheld_starts() {
        let snap = snapshot().unwrap();
        let pid = std::process::id() as i32;
        assert!(snap.procs.iter().any(|p| p.pid == pid && p.start.is_some()));
        let launchd = snap.procs.iter().find(|p| p.pid == 1).expect("launchd");
        assert_eq!(launchd.uid, 0);
        // Another user's full record is withheld without privileges: then the start is unknown, never invented.
        assert!(snap.procs.iter().all(|p| p.limited == p.start.is_none()));
        eprintln!("launchd limited={} ({} of {} processes limited)", launchd.limited, snap.limited, snap.procs.len());
        assert_eq!(process(i32::MAX).unwrap_err(), libc::ESRCH);
    }

    #[test]
    fn zombie_child_is_reported_as_zombie() {
        let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let pid = child.id() as i32;
        // Wait for the exit without reaping, so the child stays a zombie.
        // SAFETY: zeroed POD out-parameter; WNOWAIT leaves the child waitable.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: our own child pid and a valid out-pointer.
        assert_eq!(unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOWAIT) }, 0);
        let p = process(pid).unwrap();
        assert!(p.zombie && p.status == libc::SZOMB, "{p:?}");
        child.wait().unwrap();
    }

    #[test]
    fn fd_index_finds_a_descriptor_this_test_holds_and_reports_its_limit() {
        let fx = Fixture::new("procs-fd");
        let path = fx.write("held.bin", b"held open");
        let file = std::fs::File::open(&path).unwrap();
        let meta = file.metadata().unwrap();
        let index = FdIndex::build().unwrap();
        let report = index.holders(meta.dev() as i32, meta.ino());
        let me = std::process::id() as i32;
        assert!(report.holders.iter().any(|h| h.pid == me && h.hold == Hold::Fd(file.as_raw_fd())), "{report:?}");
        if !is_root() {
            assert!(!report.complete() && report.uninspected > 0, "other users' processes are a limit, not an absence");
        }
        let cwd = std::fs::metadata(".").unwrap();
        assert!(index.holders(cwd.dev() as i32, cwd.ino()).holders.iter().any(|h| h.pid == me && h.hold == Hold::Cwd));
        let other = fx.write("not-held.bin", b"x");
        let other = std::fs::metadata(other).unwrap();
        assert!(index.holders(other.dev() as i32, other.ino()).holders.is_empty());
    }
}
