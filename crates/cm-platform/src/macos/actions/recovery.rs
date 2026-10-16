//! macOS restore and purge of quarantined objects (recovery/v1, MVP 10.4). A child of `actions`, so it reuses the
//! descriptor helpers, the namespace gate and the `Stop` vocabulary without widening them.
//!
//! Both sides are opened as directories by path with the last component never followed (`O_DIRECTORY |
//! O_NOFOLLOW`), then judged on the descriptor, with dataless materialization off:
//! - Quarantine side: the folder must be on the recorded volume (`volume_offline` when the nearest existing folder
//!   above it is on another volume, `volume_changed` when another volume answers at the path), private to this user
//!   (owner, no group/other bits) and pass the trusted-namespace gate (else `race_safety_unavailable`). The object is
//!   examined with `fstatat(.., AT_SYMLINK_NOFOLLOW)`: recorded inode on that device, a regular file with one link,
//!   not dataless, not locked.
//! - Original side (restore): the original folder must carry its recorded `(volume_id, file_id)` (`parent_changed`
//!   when replaced or when a link or file is at its name, `parent_missing` when gone) and pass the same gate as the
//!   executor (`actions` §7).
//!
//! Restore: `renameatx_np(quarantine_fd, name, original_parent_fd, original_name, RENAME_EXCL)`; an occupied name is
//! `destination_exists` (looked at first, guaranteed by `RENAME_EXCL`) and nothing is overwritten. Afterwards the
//! object at the original name must be the recorded inode; otherwise the outcome is unknown with guidance and nothing
//! is moved back automatically.
//!
//! Purge: the same source verification, plus not in use by an inspectable process and an unchanged logical size; an
//! event-only descriptor is held on the verified object and `unlinkat(quarantine_fd, name)` removes the name. The
//! held descriptor must then report zero links: the recorded object, not whatever had its name, was removed.
//! Quarantine holds single-link regular files only (the executor never quarantines anything else), so purge never
//! recurses: anything else at the name is not the recorded object and is refused. Empty quarantine folders are kept.
//!
//! Residual risks (documented, not hidden): folders above each side are resolved by ordinary path lookup; a process
//! of this same user (or root) can swap a name between the last look and the rename/unlink (§6). Restore and purge
//! detect such a swap afterwards and report it as unknown.
//!
//! Metadata: a same-volume rename keeps the inode, so the mode, owner, ACL, extended attributes, BSD flags (such as
//! hidden) and the modification, access and creation times travel with the object (round-trip test). Limits: the
//! status change time (`ctime`) is updated by each move.

use std::ffi::{CStr, CString, c_int};
use std::os::unix::ffi::OsStrExt;

use cm_core::capability::CapabilityState;
use cm_core::execute::{ActionOutcome, FileActions, Reconciled};
use cm_core::path::RawPath;
use cm_core::plan::WireIdentity;
use cm_core::recovery::{MetadataCoverage, QuarantineState, QuarantinedObject, RecoveryActions, RecoveryKind, RecoveryOutcome};

use super::{
    AtPath, MacActions, Stop, acl, at_name, at_path, dataless, euid, kind, namespace_gate, no_materialize, os_path, protected_flags, split_last,
    unix_bytes, unlink_at, unlink_errno, valid_component,
};
use crate::macos::ids;
use crate::macos::sys::{Fd, SF_DATALESS, describe, errno};

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[
    (
        "recovery.restore",
        CapabilityState::Unverified,
        "Implemented on macOS: the quarantined object (private quarantine folder, recorded inode, single-link regular file) and the original folder (recorded identity and volume, trusted-namespace gate) are verified on descriptors, the exclusive rename never replaces an occupied name and the restored object's identity is confirmed afterwards; a same-user rename race is narrowed, not closed. Exercised only on a hosted CI runner, not verified on a reference Mac",
    ),
    (
        "recovery.purge",
        CapabilityState::Unverified,
        "Implemented on macOS: unlinkat of exactly the verified quarantined object inside the private quarantine folder (unchanged size, not in use), confirmed through a descriptor held on it; a same-user swap is detected afterwards. Exercised only on a hosted CI runner, not verified on a reference Mac",
    ),
];

/// Whether restore/purge of quarantined objects is implemented with identity verification before and after on macOS
/// in this build (quarantine details report `complete` only then). Real-device acceptance is tracked separately by the
/// `recovery.*` capability rows, which stay unverified.
pub const RESTORE_VERIFIED: bool = true;

/// The recovery actions for `recovery/v1` plans and their reconciliation.
pub fn recovery_actions() -> impl RecoveryActions {
    MacRecovery::default()
}

/// macOS implementation of [`RecoveryActions`]. Every call re-verifies; the state is the executor's short-lived
/// open-descriptor snapshot (in-use check before a purge).
#[derive(Debug, Default)]
pub struct MacRecovery {
    actions: MacActions,
}

const OCCUPIED: &str = "something now exists at the original name; both are kept";

fn convert(o: ActionOutcome) -> RecoveryOutcome {
    match o {
        ActionOutcome::Succeeded { .. } => RecoveryOutcome::Done,
        ActionOutcome::Skipped { code, message } => RecoveryOutcome::Skipped { code, message },
        ActionOutcome::Failed { code, message } => RecoveryOutcome::Failed { code, message },
        ActionOutcome::Unknown { message } => RecoveryOutcome::Unknown { message },
    }
}

fn stop(s: Stop) -> RecoveryOutcome {
    convert(s.outcome())
}

fn skip(code: &str, message: impl Into<String>) -> RecoveryOutcome {
    RecoveryOutcome::Skipped { code: code.into(), message: message.into() }
}

fn fail(code: &str, message: impl Into<String>) -> RecoveryOutcome {
    RecoveryOutcome::Failed { code: code.into(), message: message.into() }
}

// ---------------------------------------------------------------------------------------------
// Recorded folders

/// Why a recorded folder cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DirProblem {
    /// Nothing at the path, and the nearest existing folder above it is on the recorded volume.
    Missing,
    /// The recorded volume does not answer there (the nearest existing folder above is on another volume).
    Offline,
    /// A different volume answers at the path.
    Changed,
    /// A link or a non-directory is at the folder's name (never followed).
    NotDir,
    Denied(String),
    Unsupported(String),
    Error(String),
}

#[derive(Debug, Clone, Copy)]
enum Side {
    Quarantine,
    Original,
}

impl Side {
    fn what(self) -> &'static str {
        match self {
            Side::Quarantine => "quarantine location",
            Side::Original => "original location",
        }
    }
}

fn dir_outcome(p: DirProblem, side: Side) -> RecoveryOutcome {
    match (p, side) {
        (DirProblem::Missing, Side::Quarantine) => skip("missing", "the quarantine folder no longer exists"),
        (DirProblem::Missing, Side::Original) => skip("parent_missing", "the original folder no longer exists; restoring elsewhere needs a new plan"),
        (DirProblem::Offline, s) => skip("volume_offline", format!("the volume of the {} is not available; the manifest is kept", s.what())),
        (DirProblem::Changed, s) => skip("volume_changed", format!("a different volume is mounted at the {}; nothing is restored onto it or from it", s.what())),
        (DirProblem::NotDir, Side::Quarantine) => skip("quarantine_unsafe", "a link or a file is at the quarantine folder's name; not followed"),
        (DirProblem::NotDir, Side::Original) => {
            skip("parent_changed", "a link or a file is now at the original folder's name; not followed; restoring elsewhere needs a new plan")
        }
        (DirProblem::Denied(m), _) => fail("access_denied", m),
        (DirProblem::Unsupported(m), _) => skip("unsupported", m),
        (DirProblem::Error(m), _) => fail("check_failed", m),
    }
}

/// A recorded folder, held open, with its live metadata and mount flags.
struct Dir {
    fd: Fd,
    st: libc::stat,
    mount_flags: u32,
}

/// Volume id of the nearest existing folder above `path` (links followed: this only tells which volume answers there).
fn nearest_volume(path: &[u8]) -> Option<String> {
    for p in os_path(path).ancestors().skip(1) {
        let Ok(c) = CString::new(p.as_os_str().as_bytes()) else { return None };
        if let Ok(fd) = Fd::open_dir_following(&c) {
            let st = fd.stat().ok()?;
            return ids::volume_id(&fd, st.st_dev).ok();
        }
    }
    None
}

/// Open the folder at `path` (last component never followed) and require it on the recorded volume.
fn open_recorded(path: &[u8], volume_id: &str) -> Result<Dir, DirProblem> {
    let c = CString::new(path).map_err(|_| DirProblem::Error("path contains NUL".into()))?;
    let fd = match Fd::open_dir(&c) {
        Ok(fd) => fd,
        Err(libc::ENOENT) => {
            return Err(match nearest_volume(path) {
                Some(v) if v == volume_id => DirProblem::Missing,
                Some(_) => DirProblem::Offline,
                None => DirProblem::Error("no existing folder above the recorded location can be examined".into()),
            });
        }
        Err(libc::ELOOP | libc::ENOTDIR) => return Err(DirProblem::NotDir),
        Err(e @ (libc::EACCES | libc::EPERM)) => return Err(DirProblem::Denied(format!("access denied: {}", describe(e)))),
        Err(libc::EDEADLK) => return Err(DirProblem::Unsupported("cloud_materialization_avoided: the folder is not available locally".into())),
        Err(e) => return Err(DirProblem::Error(format!("cannot open the folder: {}", describe(e)))),
    };
    let st = fd.stat().map_err(|e| DirProblem::Error(format!("cannot query the folder: {}", describe(e))))?;
    let sfs = fd.statfs().map_err(|e| DirProblem::Error(format!("cannot query the volume: {}", describe(e))))?;
    let volume = ids::volume_id(&fd, st.st_dev).map_err(|e| DirProblem::Error(format!("cannot identify the volume: {}", describe(e))))?;
    if volume != volume_id {
        return Err(DirProblem::Changed);
    }
    if st.st_flags & SF_DATALESS != 0 {
        return Err(DirProblem::Unsupported("cloud placeholder (dataless) folder: never downloaded to act on it".into()));
    }
    Ok(Dir { fd, st, mount_flags: sfs.f_flags })
}

/// `(folder, name)` of an absolute file path whose name is a single component.
fn file_path(p: &RawPath) -> Option<(Vec<u8>, Vec<u8>)> {
    split_last(unix_bytes(p)?).filter(|(_, n)| valid_component(n))
}

// ---------------------------------------------------------------------------------------------
// The quarantined object and the original folder

/// The verified quarantined object: its folder (held open), name and live metadata.
struct Source {
    dir: Dir,
    name: CString,
    live: libc::stat,
}

/// The quarantine folder is a trusted namespace: private to this user and passing the executor's gate.
fn private_quarantine(d: &Dir) -> Result<(), String> {
    if d.st.st_uid != euid() || d.st.st_mode & 0o077 != 0 {
        return Err("the quarantine folder is not private to this user".into());
    }
    namespace_gate(&d.st, d.mount_flags, euid(), || acl::grants_change(&d.fd))
}

/// The object at the quarantine name against its recorded identity (`dir_dev`: the verified folder's device).
fn check_object(identity: &WireIdentity, st: &libc::stat, dir_dev: i32) -> Result<(), Stop> {
    if kind(st) == libc::S_IFLNK {
        return Err(Stop::Reparse);
    }
    if st.st_dev != dir_dev || ids::file_id(st.st_ino) != identity.file_id {
        return Err(Stop::Identity);
    }
    if let Some(s) = dataless(st.st_flags) {
        return Err(s);
    }
    if let Some(why) = protected_flags(st.st_flags) {
        return Err(Stop::Protected(format!("the quarantined object is {why}")));
    }
    if kind(st) != libc::S_IFREG {
        return Err(Stop::NotFile);
    }
    if st.st_nlink > 1 {
        return Err(Stop::HardLinks(u32::from(st.st_nlink)));
    }
    Ok(())
}

fn open_source(o: &QuarantinedObject<'_>) -> Result<Source, RecoveryOutcome> {
    if unix_bytes(o.location).is_none() {
        return Err(fail("unsupported", "not a macOS path"));
    }
    let (dir, name) = file_path(o.location).ok_or_else(|| fail("invalid_target", "the quarantine location is not a file path"))?;
    let dir = open_recorded(&dir, &o.identity.volume_id).map_err(|p| dir_outcome(p, Side::Quarantine))?;
    private_quarantine(&dir).map_err(|m| stop(Stop::Gate(m)))?;
    let live = dir.fd.stat_child(&name).map_err(|e| stop(Stop::from_target(e)))?;
    check_object(o.identity, &live, dir.st.st_dev).map_err(stop)?;
    let name = CString::new(name).map_err(|_| fail("invalid_target", "the quarantine name contains NUL"))?;
    Ok(Source { dir, name, live })
}

/// The verified original folder (held open) and the original name.
struct Original {
    dir: Dir,
    name: CString,
}

fn original_parent(o: &QuarantinedObject<'_>) -> Result<Original, RecoveryOutcome> {
    if unix_bytes(o.original_path).is_none() {
        return Err(fail("unsupported", "not a macOS path"));
    }
    let (dir, name) = file_path(o.original_path).ok_or_else(|| fail("invalid_target", "the original path has no folder or its name is not a single path component"))?;
    let dir = open_recorded(&dir, &o.original_parent.volume_id).map_err(|p| dir_outcome(p, Side::Original))?;
    if ids::file_id(dir.st.st_ino) != o.original_parent.file_id {
        return Err(skip("parent_changed", "the original folder was replaced or moved; restoring elsewhere needs a new plan"));
    }
    if let Some(why) = protected_flags(dir.st.st_flags) {
        return Err(stop(Stop::Protected(format!("the original folder is {why}"))));
    }
    namespace_gate(&dir.st, dir.mount_flags, euid(), || acl::grants_change(&dir.fd)).map_err(|m| stop(Stop::Gate(m)))?;
    let name = CString::new(name).map_err(|_| fail("invalid_target", "the original name contains NUL"))?;
    Ok(Original { dir, name })
}

/// An event-only, no-follow descriptor on the verified object: reads nothing, blocks neither unlink nor unmount.
struct Held(c_int);

impl Held {
    fn open(dir: &Fd, name: &CStr) -> Result<Held, i32> {
        let flags: c_int = libc::O_EVTONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        // SAFETY: valid directory fd and NUL-terminated single-component name; no mode without O_CREAT.
        let fd = unsafe { libc::openat(dir.raw(), name.as_ptr(), flags) };
        if fd < 0 { Err(errno()) } else { Ok(Held(fd)) }
    }

    fn stat(&self) -> Result<libc::stat, i32> {
        // SAFETY: zeroed POD out-parameter of the declared type.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: valid fd and out-pointer.
        if unsafe { libc::fstat(self.0, &mut st) } != 0 { Err(errno()) } else { Ok(st) }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        // SAFETY: this value owns the descriptor and closes it exactly once.
        unsafe { libc::close(self.0) };
    }
}

// ---------------------------------------------------------------------------------------------
// RecoveryActions

impl RecoveryActions for MacRecovery {
    fn probe(&self, o: &QuarantinedObject<'_>) -> QuarantineState {
        let _guard = no_materialize().ok();
        let (Some((qdir, qname)), Some((pdir, pname))) = (file_path(o.location), file_path(o.original_path)) else { return QuarantineState::Unknown };
        match open_recorded(&qdir, &o.identity.volume_id) {
            Err(DirProblem::Offline) => return QuarantineState::VolumeOffline,
            Err(DirProblem::Changed) => return QuarantineState::VolumeChanged,
            Err(DirProblem::Missing) => return QuarantineState::Missing,
            Err(DirProblem::NotDir) => return QuarantineState::IdentityChanged,
            Err(_) => return QuarantineState::Unknown,
            Ok(d) => match d.fd.stat_child(&qname) {
                Ok(st) => match check_object(o.identity, &st, d.st.st_dev) {
                    Ok(()) => {}
                    Err(Stop::Reparse | Stop::Identity | Stop::NotFile | Stop::HardLinks(_)) => return QuarantineState::IdentityChanged,
                    Err(_) => return QuarantineState::Unknown,
                },
                Err(libc::ENOENT) => return QuarantineState::Missing,
                Err(_) => return QuarantineState::Unknown,
            },
        }
        match open_recorded(&pdir, &o.original_parent.volume_id) {
            Err(DirProblem::Offline) => QuarantineState::VolumeOffline,
            Err(DirProblem::Changed) => QuarantineState::VolumeChanged,
            Err(DirProblem::Missing | DirProblem::NotDir) => QuarantineState::ParentChanged,
            Err(_) => QuarantineState::Unknown,
            Ok(d) if ids::file_id(d.st.st_ino) != o.original_parent.file_id => QuarantineState::ParentChanged,
            Ok(d) => match d.fd.stat_child(&pname) {
                Ok(_) => QuarantineState::TargetConflict,
                Err(libc::ENOENT) => QuarantineState::Recoverable,
                Err(_) => QuarantineState::Unknown,
            },
        }
    }

    fn restore(&self, o: &QuarantinedObject<'_>) -> RecoveryOutcome {
        let _guard = match no_materialize() {
            Ok(g) => g,
            Err(s) => return stop(s),
        };
        let src = match open_source(o) {
            Ok(s) => s,
            Err(out) => return out,
        };
        let orig = match original_parent(o) {
            Ok(p) => p,
            Err(out) => return out,
        };
        if orig.dir.st.st_dev != src.dir.st.st_dev {
            return skip("volume_changed", "the original folder is on another volume than the quarantine; refusing copy-and-delete");
        }
        // Early, readable refusal; the exclusive rename below is what guarantees it.
        match orig.dir.fd.stat_child(orig.name.as_bytes()) {
            Ok(_) => return skip("destination_exists", OCCUPIED),
            Err(libc::ENOENT) => {}
            Err(e) => return fail("check_failed", format!("cannot check the original name: {}", describe(e))),
        }
        #[cfg(test)]
        tests::before_restore();
        // SAFETY: valid directory fds and NUL-terminated single-component names; RENAME_EXCL never replaces.
        let rc = unsafe { libc::renameatx_np(src.dir.fd.raw(), src.name.as_ptr(), orig.dir.fd.raw(), orig.name.as_ptr(), libc::RENAME_EXCL) };
        if rc != 0 {
            let e = errno();
            return match e {
                libc::EEXIST => skip("destination_exists", OCCUPIED),
                // The source left its name, or the held original folder was removed meanwhile; nothing moved.
                libc::ENOENT => match at_name(&src.dir.fd, &src.name, &src.live) {
                    AtPath::Planned => skip("parent_missing", "the original folder was removed during the restore; nothing moved"),
                    _ => stop(Stop::Missing),
                },
                libc::EXDEV => skip("volume_changed", format!("rename crosses volumes: {}", describe(e))),
                libc::EBUSY => skip("in_use", format!("rename refused: {}", describe(e))),
                libc::EACCES | libc::EPERM => fail("access_denied", format!("rename refused: {}", describe(e))),
                libc::ENOTSUP | libc::EINVAL => fail("rename_failed", "the volume cannot rename without replacing (no exclusive rename); nothing moved"),
                _ => fail("rename_failed", format!("rename refused: {}", describe(e))),
            };
        }
        let (shown, from) = (o.original_path.display(), o.location.display());
        // The object now under the original name must be the recorded one.
        match at_name(&orig.dir.fd, &orig.name, &src.live) {
            AtPath::Planned => RecoveryOutcome::Done,
            AtPath::Other => RecoveryOutcome::Unknown {
                message: format!(
                    "the name in quarantine was replaced just before the move: the object now at {shown} is not the quarantined one. It was not moved back automatically; check it, and look for the quarantined object near {from}"
                ),
            },
            AtPath::Missing => RecoveryOutcome::Unknown { message: format!("the move was reported but nothing is at {shown}; check it and the quarantine at {from}") },
            AtPath::Error(e) => RecoveryOutcome::Unknown { message: format!("moved, but the restored object at {shown} cannot be confirmed: {e}") },
        }
    }

    fn purge(&self, o: &QuarantinedObject<'_>) -> RecoveryOutcome {
        let _guard = match no_materialize() {
            Ok(g) => g,
            Err(s) => return stop(s),
        };
        let src = match open_source(o) {
            Ok(s) => s,
            Err(out) => return out,
        };
        if let Err(s) = self.actions.in_use(&src.live) {
            return stop(s);
        }
        // Changed content is never purged silently (REC-03).
        let size = u64::try_from(src.live.st_size).unwrap_or(0);
        if size != o.logical_bytes {
            return skip("metadata_changed", format!("logical size changed in quarantine: recorded {}, now {size}", o.logical_bytes));
        }
        let held = match Held::open(&src.dir.fd, &src.name) {
            Ok(h) => h,
            Err(libc::ELOOP) => return stop(Stop::Reparse),
            Err(e) => return stop(Stop::from_target(e)),
        };
        match held.stat() {
            Ok(st) if (st.st_dev, st.st_ino) == (src.live.st_dev, src.live.st_ino) => {}
            Ok(_) => return stop(Stop::Identity),
            Err(e) => return fail("check_failed", format!("cannot query the held object: {}", describe(e))),
        }
        #[cfg(test)]
        tests::before_purge();
        // RESIDUAL RACE (§6): the kernel looks the name up again; only a process of this user or root can have swapped
        // it in the private quarantine since the check. The held descriptor tells afterwards which object went.
        if let Err(e) = unlink_at(&src.dir.fd, &src.name, 0) {
            return fail(unlink_errno(e), format!("delete refused: {}", describe(e)));
        }
        match held.stat() {
            Ok(st) if st.st_nlink == 0 => RecoveryOutcome::Done,
            Ok(_) => {
                let now = super::trash::fd_path(held.0).map(|p| String::from_utf8_lossy(&p).into_owned()).unwrap_or_else(|| "an unknown location".into());
                RecoveryOutcome::Unknown {
                    message: format!(
                        "the name in quarantine was replaced just before the delete: another object was removed and the quarantined object still exists (now at {now}); check the quarantine"
                    ),
                }
            }
            Err(e) => RecoveryOutcome::Unknown { message: format!("deleted, but the result cannot be confirmed: {}", describe(e)) },
        }
    }

    fn reconcile(&self, o: &QuarantinedObject<'_>, kind: RecoveryKind) -> Reconciled {
        let (Some(q), Some(orig)) = (unix_bytes(o.location), unix_bytes(o.original_path)) else { return Reconciled::StillUnknown };
        let _guard = no_materialize().ok();
        let (vol, fid) = (&o.identity.volume_id, &o.identity.file_id);
        match (kind, at_path(q, vol, fid), at_path(orig, vol, fid)) {
            (_, AtPath::Planned, _) => Reconciled::NotPerformed,
            (RecoveryKind::Restore, _, AtPath::Planned) => Reconciled::Performed,
            // Gone from a quarantine folder that is still there on the recorded volume, and not restored.
            (RecoveryKind::Purge, AtPath::Missing, AtPath::Missing | AtPath::Other) => match split_last(q).map(|(d, _)| open_recorded(&d, vol)) {
                Some(Ok(_)) => Reconciled::Performed,
                _ => Reconciled::StillUnknown,
            },
            _ => Reconciled::StillUnknown,
        }
    }

    fn free_space(&self, path: &RawPath) -> Option<u64> {
        self.actions.free_space(path)
    }

    fn metadata_coverage(&self) -> MetadataCoverage {
        MetadataCoverage {
            preserved: vec!["timestamps", "attributes", "acl", "xattrs"],
            limits: vec![
                "same-volume rename only: the inode moves, nothing is copied",
                "the status change time (ctime) is updated by each move; modification, access and creation times are kept",
                "permissions inheritable from the quarantine folder are not applied; the object keeps its own mode, owner and access control list",
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::os::macos::fs::MetadataExt as _;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};

    use super::*;
    use crate::macos::testfs::Fixture;

    const AT: &str = "2026-10-17T00:00:00Z";
    /// `UF_HIDDEN` from `<sys/stat.h>`.
    const UF_HIDDEN: u32 = 0x0000_8000;

    thread_local! {
        /// Runs once inside `restore`, after verification and right before the rename (race fixture).
        static BEFORE_RESTORE: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        /// Runs once inside `purge`, after verification and right before the unlink (race fixture).
        static BEFORE_PURGE: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    }

    pub(super) fn before_restore() {
        if let Some(f) = BEFORE_RESTORE.with(|h| h.borrow_mut().take()) {
            f();
        }
    }

    pub(super) fn before_purge() {
        if let Some(f) = BEFORE_PURGE.with(|h| h.borrow_mut().take()) {
            f();
        }
    }

    /// Fresh recovery actions (and so a fresh open-descriptor snapshot) for each use.
    fn r() -> MacRecovery {
        MacRecovery::default()
    }

    fn raw(p: &Path) -> RawPath {
        RawPath::unix(p.as_os_str().as_bytes().to_vec()).unwrap()
    }

    fn ident(p: &Path) -> WireIdentity {
        let md = std::fs::symlink_metadata(p).unwrap();
        let dir = if md.is_dir() { p } else { p.parent().unwrap() };
        let fd = Fd::open_dir(&CString::new(dir.as_os_str().as_bytes()).unwrap()).unwrap();
        WireIdentity {
            volume_id: ids::volume_id(&fd, md.dev() as i32).unwrap(),
            file_id: ids::file_id(md.ino()),
            generation: None,
            quality: "strong".into(),
            observed_at: AT.into(),
        }
    }

    /// A quarantined file: moved (same volume, same inode) from `orig/name` into the private folder `q/name`.
    struct Q {
        location: RawPath,
        identity: WireIdentity,
        original: RawPath,
        parent: WireIdentity,
        size: u64,
        loc: PathBuf,
        orig: PathBuf,
    }

    impl Q {
        fn object(&self) -> QuarantinedObject<'_> {
            QuarantinedObject {
                action_id: "r0",
                location: &self.location,
                identity: &self.identity,
                original_path: &self.original,
                original_parent: &self.parent,
                logical_bytes: self.size,
            }
        }
    }

    fn quarantined(fx: &Fixture, name: &str, body: &str, prepare: impl FnOnce(&Path)) -> Q {
        let dir = fx.mkdir("orig");
        let orig = dir.join(name);
        std::fs::write(&orig, body).unwrap();
        prepare(&orig);
        let identity = ident(&orig);
        let q = fx.path().join("q");
        if !q.exists() {
            std::fs::DirBuilder::new().mode(0o700).create(&q).unwrap();
        }
        let loc = q.join(name);
        std::fs::rename(&orig, &loc).unwrap();
        Q { location: raw(&loc), identity, original: raw(&orig), parent: ident(&dir), size: body.len() as u64, loc, orig }
    }

    fn code(o: &RecoveryOutcome) -> &str {
        match o {
            RecoveryOutcome::Skipped { code, .. } | RecoveryOutcome::Failed { code, .. } => code,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    fn chmod(p: &Path, mode: u32) {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    fn chflags(p: &Path, flags: u32) {
        let c = CString::new(p.as_os_str().as_bytes()).unwrap();
        // SAFETY: NUL-terminated path; user flags only (no privilege needed for the owner).
        assert_eq!(unsafe { libc::lchflags(c.as_ptr(), flags) }, 0, "chflags {}: {}", p.display(), std::io::Error::last_os_error());
    }

    fn run(cmd: &str, args: &[&str], p: &Path) -> String {
        let out = Command::new(cmd).args(args).arg(p).output().unwrap();
        assert!(out.status.success(), "{cmd} {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Clears user flags again, even when an assertion fails (the fixture cannot remove a locked file).
    struct Unlock(PathBuf);

    impl Drop for Unlock {
        fn drop(&mut self) {
            if let Ok(c) = CString::new(self.0.as_os_str().as_bytes()) {
                // SAFETY: NUL-terminated path.
                unsafe { libc::lchflags(c.as_ptr(), 0) };
            }
        }
    }

    /// A child process of this user holding `path` open on its standard input.
    struct Holder(Child);

    impl Holder {
        fn of(path: &Path) -> Holder {
            let f = std::fs::File::open(path).unwrap();
            Holder(Command::new("/bin/sleep").arg("120").stdin(f).stdout(Stdio::null()).spawn().unwrap())
        }
    }

    impl Drop for Holder {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn restore_round_trip_keeps_identity_and_metadata() {
        let fx = Fixture::new("rec-roundtrip");
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(40 * 86_400);
        let q = quarantined(&fx, "keep.cache", "payload", |p| {
            run("/usr/bin/xattr", &["-w", "com.cleanmanager.test", "side"], p);
            run("/bin/chmod", &["+a", "everyone allow read"], p);
            chmod(p, 0o640);
            let f = std::fs::File::options().write(true).open(p).unwrap();
            f.set_times(std::fs::FileTimes::new().set_modified(past).set_accessed(past)).unwrap();
            drop(f);
            chflags(p, UF_HIDDEN);
        });
        let before = std::fs::symlink_metadata(&q.loc).unwrap();
        assert_eq!(r().probe(&q.object()), QuarantineState::Recoverable);
        assert_eq!(r().reconcile(&q.object(), RecoveryKind::Restore), Reconciled::NotPerformed);
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(r().restore(&q.object()), RecoveryOutcome::Done);
        assert!(!q.loc.exists());
        let after = std::fs::symlink_metadata(&q.orig).unwrap();
        assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()), "the same inode is back");
        assert_eq!((after.mtime(), after.mtime_nsec()), (before.mtime(), before.mtime_nsec()), "modification time");
        assert_eq!((after.atime(), after.atime_nsec()), (before.atime(), before.atime_nsec()), "access time");
        assert_eq!(after.created().unwrap(), before.created().unwrap(), "creation time");
        assert_ne!((after.ctime(), after.ctime_nsec()), (before.ctime(), before.ctime_nsec()), "documented limit: the move updates ctime");
        assert_eq!((after.mode() & 0o7777, after.uid(), after.gid()), (0o640, before.uid(), before.gid()), "mode and owner");
        assert_ne!(after.st_flags() & UF_HIDDEN, 0, "BSD flags (hidden)");
        assert_eq!(run("/usr/bin/xattr", &["-p", "com.cleanmanager.test"], &q.orig).trim(), "side", "extended attribute");
        assert!(run("/bin/ls", &["-led"], &q.orig).contains("everyone allow read"), "access control entry");
        assert_eq!(read(&q.orig), "payload");
        assert_eq!(ident(&q.orig).file_id, q.identity.file_id);
        assert_eq!(r().reconcile(&q.object(), RecoveryKind::Restore), Reconciled::Performed);
        assert_eq!(r().probe(&q.object()), QuarantineState::Missing);
        assert_eq!(code(&r().restore(&q.object())), "missing");
        let cov = r().metadata_coverage();
        assert_eq!(cov.preserved, ["timestamps", "attributes", "acl", "xattrs"]);
        assert!(cov.limits.iter().any(|l| l.contains("ctime")));
    }

    #[test]
    fn occupied_original_name_is_never_overwritten() {
        let fx = Fixture::new("rec-occupied");
        let q = quarantined(&fx, "a.tmp", "quarantined", |_| {});
        std::fs::write(&q.orig, "newer").unwrap();
        assert_eq!(r().probe(&q.object()), QuarantineState::TargetConflict);
        assert_eq!(code(&r().restore(&q.object())), "destination_exists");
        assert_eq!((read(&q.orig), read(&q.loc)), ("newer".into(), "quarantined".into()));
        // A directory or a dangling link squatting on the name is refused the same way.
        std::fs::remove_file(&q.orig).unwrap();
        std::fs::create_dir(&q.orig).unwrap();
        assert_eq!(code(&r().restore(&q.object())), "destination_exists");
        std::fs::remove_dir(&q.orig).unwrap();
        std::os::unix::fs::symlink(fx.path().join("nowhere"), &q.orig).unwrap();
        assert_eq!(code(&r().restore(&q.object())), "destination_exists");
        std::fs::remove_file(&q.orig).unwrap();
        // A name taken after the last look: the exclusive rename still refuses.
        let orig = q.orig.clone();
        BEFORE_RESTORE.with(|h| *h.borrow_mut() = Some(Box::new(move || std::fs::write(&orig, "raced in").unwrap())));
        assert_eq!(code(&r().restore(&q.object())), "destination_exists");
        assert_eq!((read(&q.orig), read(&q.loc)), ("raced in".into(), "quarantined".into()));
    }

    #[test]
    fn changed_source_or_parent_is_refused() {
        let fx = Fixture::new("rec-changed");
        let q = quarantined(&fx, "a.tmp", "planned", |_| {});
        let moved = fx.path().join("moved");
        std::fs::rename(&q.loc, &moved).unwrap();
        std::fs::write(&q.loc, "planted").unwrap();
        assert_eq!(r().probe(&q.object()), QuarantineState::IdentityChanged);
        assert_eq!(code(&r().restore(&q.object())), "identity_changed");
        assert_eq!(code(&r().purge(&q.object())), "identity_changed");
        assert_eq!(read(&q.loc), "planted");
        assert!(!q.orig.exists());
        assert_eq!(r().reconcile(&q.object(), RecoveryKind::Purge), Reconciled::StillUnknown);
        std::fs::remove_file(&q.loc).unwrap();
        std::os::unix::fs::symlink(&moved, &q.loc).unwrap();
        assert_eq!(r().probe(&q.object()), QuarantineState::IdentityChanged);
        assert_eq!(code(&r().restore(&q.object())), "link_or_reparse");
        assert_eq!(code(&r().purge(&q.object())), "link_or_reparse");
        std::fs::remove_file(&q.loc).unwrap();
        std::fs::rename(&moved, &q.loc).unwrap();
        assert_eq!(r().probe(&q.object()), QuarantineState::Recoverable);

        // A quarantine folder others can change is not a trusted namespace.
        let qdir = q.loc.parent().unwrap().to_path_buf();
        chmod(&qdir, 0o755);
        assert_eq!(code(&r().restore(&q.object())), "race_safety_unavailable");
        assert_eq!(code(&r().purge(&q.object())), "race_safety_unavailable");
        chmod(&qdir, 0o700);

        // The original folder: replaced by a new one of the same name, gone, a link to the old one, untrusted.
        let dir = q.orig.parent().unwrap().to_path_buf();
        let old = fx.path().join("orig-old");
        std::fs::rename(&dir, &old).unwrap();
        std::fs::create_dir(&dir).unwrap();
        assert_eq!(r().probe(&q.object()), QuarantineState::ParentChanged);
        assert_eq!(code(&r().restore(&q.object())), "parent_changed");
        std::fs::remove_dir(&dir).unwrap();
        assert_eq!(r().probe(&q.object()), QuarantineState::ParentChanged);
        assert_eq!(code(&r().restore(&q.object())), "parent_missing");
        std::os::unix::fs::symlink(&old, &dir).unwrap();
        assert_eq!(code(&r().restore(&q.object())), "parent_changed", "a link at the folder's name is never followed");
        assert!(!old.join("a.tmp").exists());
        std::fs::remove_file(&dir).unwrap();
        std::fs::rename(&old, &dir).unwrap();
        chmod(&dir, 0o777);
        assert_eq!(code(&r().restore(&q.object())), "race_safety_unavailable");
        chmod(&dir, 0o755);
        let _unlock = Unlock(dir.clone());
        chflags(&dir, libc::UF_IMMUTABLE);
        assert_eq!(code(&r().restore(&q.object())), "protected");
        chflags(&dir, 0);
        assert!(q.loc.exists() && !q.orig.exists(), "nothing moved by a refusal");
        // With everything back in place the restore goes through.
        assert_eq!(r().restore(&q.object()), RecoveryOutcome::Done);
        assert_eq!(read(&q.orig), "planned");
    }

    /// Race fixture: the quarantined object is renamed away and another file takes its quarantine name between the
    /// last check and the rename. The impostor lands at the original name, the outcome is unknown with guidance and the
    /// quarantined object is untouched.
    #[test]
    fn a_name_swapped_in_quarantine_before_the_restore_is_unknown_and_nothing_is_lost() {
        let fx = Fixture::new("rec-rrace");
        let q = quarantined(&fx, "video.mov", "the quarantined file", |_| {});
        let (loc, moved) = (q.loc.clone(), q.loc.with_extension("moved-away"));
        let moved2 = moved.clone();
        BEFORE_RESTORE.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                std::fs::rename(&loc, &moved2).unwrap();
                std::fs::write(&loc, "an impostor with the same name").unwrap();
            }))
        });
        let out = r().restore(&q.object());
        let RecoveryOutcome::Unknown { message } = &out else { panic!("{out:?}") };
        assert!(message.contains("not moved back"), "{message}");
        assert_eq!(read(&q.orig), "an impostor with the same name");
        assert_eq!(read(&moved), "the quarantined file");
        assert_eq!(r().reconcile(&q.object(), RecoveryKind::Restore), Reconciled::StillUnknown);
    }

    #[test]
    fn purge_deletes_exactly_the_verified_object() {
        let fx = Fixture::new("rec-purge");
        let q = quarantined(&fx, "a.tmp", "gone", |_| {});
        let sibling = q.loc.parent().unwrap().join("b.tmp");
        std::fs::write(&sibling, "kept").unwrap();
        {
            let _holder = Holder::of(&q.loc);
            assert_eq!(code(&r().purge(&q.object())), "in_use");
        }
        std::fs::write(&q.loc, "grown!").unwrap();
        assert_eq!(code(&r().purge(&q.object())), "metadata_changed", "changed content is never purged silently");
        std::fs::write(&q.loc, "gone").unwrap();
        {
            let _unlock = Unlock(q.loc.clone());
            chflags(&q.loc, libc::UF_IMMUTABLE);
            assert_eq!(code(&r().purge(&q.object())), "protected");
            chflags(&q.loc, 0);
        }
        assert_eq!(r().reconcile(&q.object(), RecoveryKind::Purge), Reconciled::NotPerformed);
        assert_eq!(r().purge(&q.object()), RecoveryOutcome::Done);
        assert!(!q.loc.exists() && !q.orig.exists());
        assert_eq!(read(&sibling), "kept");
        assert!(q.loc.parent().unwrap().is_dir(), "the quarantine folder is kept");
        assert_eq!(r().reconcile(&q.object(), RecoveryKind::Purge), Reconciled::Performed);
        assert_eq!(r().probe(&q.object()), QuarantineState::Missing);
        assert_eq!(code(&r().purge(&q.object())), "missing");
        assert!(r().free_space(&q.location).is_some_and(|n| n > 0));
    }

    #[test]
    fn a_name_swapped_before_the_purge_is_unknown_and_the_object_survives() {
        let fx = Fixture::new("rec-prace");
        let q = quarantined(&fx, "a.tmp", "planned", |_| {});
        let (loc, moved) = (q.loc.clone(), q.loc.with_extension("moved-away"));
        let moved2 = moved.clone();
        BEFORE_PURGE.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                std::fs::rename(&loc, &moved2).unwrap();
                std::fs::write(&loc, "impost").unwrap();
            }))
        });
        let out = r().purge(&q.object());
        let RecoveryOutcome::Unknown { message } = &out else { panic!("{out:?}") };
        assert!(message.contains("still exists"), "{message}");
        assert_eq!(read(&moved), "planned", "the quarantined object is not removed");
    }

    #[test]
    fn offline_or_replaced_volumes_keep_the_manifest() {
        let fx = Fixture::new("rec-volume");
        let q = quarantined(&fx, "a.tmp", "x", |_| {});
        let elsewhere = WireIdentity { volume_id: "uuid:00000000-0000-0000-0000-00000000DEAD".into(), ..q.identity.clone() };
        // The recorded volume is not mounted: nothing answers at the path and the folder above is another volume.
        let away = raw(&fx.path().join("unmounted/q/a.tmp"));
        let offline = QuarantinedObject { location: &away, identity: &elsewhere, ..q.object() };
        assert_eq!(r().probe(&offline), QuarantineState::VolumeOffline);
        assert_eq!(code(&r().restore(&offline)), "volume_offline");
        assert_eq!(code(&r().purge(&offline)), "volume_offline");
        assert_eq!(r().reconcile(&offline, RecoveryKind::Purge), Reconciled::StillUnknown);
        // Something answers at the recorded path, but on another volume.
        let swapped = QuarantinedObject { identity: &elsewhere, ..q.object() };
        assert_eq!(r().probe(&swapped), QuarantineState::VolumeChanged);
        assert_eq!(code(&r().restore(&swapped)), "volume_changed");
        assert_eq!(code(&r().purge(&swapped)), "volume_changed");
        let other_parent = WireIdentity { volume_id: elsewhere.volume_id.clone(), ..q.parent.clone() };
        let moved_parent = QuarantinedObject { original_parent: &other_parent, ..q.object() };
        assert_eq!(r().probe(&moved_parent), QuarantineState::VolumeChanged);
        assert_eq!(code(&r().restore(&moved_parent)), "volume_changed");
        // On the recorded volume, a vanished quarantine folder is simply missing.
        let gone = raw(&fx.path().join("gone/a.tmp"));
        let missing = QuarantinedObject { location: &gone, ..q.object() };
        assert_eq!(r().probe(&missing), QuarantineState::Missing);
        assert_eq!(code(&r().restore(&missing)), "missing");
        assert!(q.loc.exists() && !q.orig.exists());
    }

    #[test]
    fn capability_rows_stay_unverified_without_a_reference_mac() {
        for name in ["recovery.restore", "recovery.purge"] {
            let (_, state, reason) = CAPS.iter().find(|(n, _, _)| *n == name).copied().unwrap();
            assert_eq!(state, CapabilityState::Unverified);
            assert!(reason.contains("not verified on a reference Mac"), "{reason}");
        }
    }
}
