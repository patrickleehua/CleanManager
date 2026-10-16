//! macOS file actions for the executor (MVP 9.5; the action half of parity 6.2): descriptor-verified quarantine and
//! delete, and Trash moves through the system interface with identity checks before and after.
//!
//! Verification (`check`, and again inside every action), all on one thread with dataless materialization turned
//! off (TN3150), so nothing here downloads a cloud placeholder:
//! - The scope root is opened `O_DIRECTORY | O_NOFOLLOW` and every planned directory below it is entered with `openat`
//!   (no follow) after an `fstatat` look; each level must carry its planned `(volume_id, file_id)` (`ids` spelling).
//! - Namespace gate (docs/platforms/macos.md §7): every directory from the scope root to the parent must be owned by
//!   the effective user (or root), not writable by group or others (mode bits or an allowing ACL entry), not sticky,
//!   and on a volume that honours ownership. Otherwise the action is `race_safety_unavailable`: another principal could
//!   swap the last name between the check and the action.
//! - The target is examined with `fstatat(parent, name, AT_SYMLINK_NOFOLLOW)`: identity, type, single link, size,
//!   modification time and link count against the plan's observation. Dataless objects are `unsupported`; immutable,
//!   restricted, append-only or no-unlink objects are `protected`; an open descriptor in a process of this user is
//!   `in_use`. Other users' processes cannot be inspected without privileges: that limit is traced, never read as
//!   "not in use".
//!
//! Actions:
//! - quarantine: `renameatx_np(parent, name, quarantine, qname, RENAME_EXCL)` (never replaces), then the object at the
//!   quarantine name must be the planned one; otherwise the outcome is unknown with put-back guidance (nothing is
//!   moved back automatically).
//! - delete: `unlinkat(parent, name)`; empty planned directories: `unlinkat(.., AT_REMOVEDIR)` after listing them.
//! - trash: see [`trash`].
//!
//! Residual risks (documented, not hidden): ancestors above the scope root are resolved by ordinary path lookup;
//! `fstatat` followed by `renameatx_np`/`unlinkat` is two calls, so a process of this same user (or root) can still
//! swap the last name in between (§6 places same-UID processes outside what this tool can isolate). Quarantine
//! detects such a swap afterwards; delete cannot. The Trash interface is path based (see [`trash`]).

pub mod recovery;
pub mod trash;

use std::collections::HashMap;
use std::ffi::{CString, OsStr, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use cm_core::capability::CapabilityState;
use cm_core::execute::{ActionOutcome, ActionTarget, FileActions, LiveCheck, Reconciled, Recovery};
use cm_core::model::Identity;
use cm_core::obs::{ObservationRecord, TimeUnit};
use cm_core::path::RawPath;
use cm_core::rules::Mode;

use super::ids;
use super::procs::{FdIndex, HolderReport};
use super::sys::{Fd, NoMaterialize, SF_DATALESS, describe, errno, read_names};

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[
    (
        "execute.trash",
        CapabilityState::Unverified,
        "Implemented on macOS: NSFileManager moves the identity-verified file to the Trash and the result is confirmed through a descriptor held on it; the move itself is path based. Exercised only on a hosted CI runner, not on a reference Mac",
    ),
    (
        "execute.quarantine",
        CapabilityState::Unverified,
        "Implemented on macOS: descriptor-verified parent chain, trusted-namespace gate (else race_safety_unavailable), exclusive same-volume rename into a private quarantine and an identity check afterwards; a same-user rename race is narrowed, not closed. Not verified on a reference Mac",
    ),
    (
        "execute.delete",
        CapabilityState::Unverified,
        "Implemented on macOS: unlinkat behind the same verification and namespace gate; a same-user rename between the last check and the unlink is not detectable. Not verified on a reference Mac",
    ),
];

/// `SF_RESTRICTED` / `SF_NOUNLINK` / `SF_APPEND` / `UF_APPEND` from `<sys/stat.h>` (not all exported by `libc`).
const SF_RESTRICTED: u32 = 0x0008_0000;
const SF_NOUNLINK: u32 = 0x0010_0000;
const SF_APPEND: u32 = 0x0004_0000;
const UF_APPEND: u32 = 0x0000_0004;

/// How long one open-descriptor snapshot answers in-use questions (building it walks every process).
const HOLDER_TTL: Duration = Duration::from_secs(1);

/// macOS implementation of the executor's file actions. Every call re-verifies; the state is which store directory
/// hosts same-volume quarantine and a short-lived open-descriptor snapshot.
#[derive(Debug, Default)]
pub struct MacActions {
    store_dir: Option<PathBuf>,
    holders: Mutex<Option<(Instant, FdIndex)>>,
}

impl MacActions {
    /// Quarantine under `dir` (the `--store` in use) when it is on the target's volume.
    pub fn with_store(dir: PathBuf) -> Self {
        MacActions { store_dir: Some(dir), holders: Mutex::new(None) }
    }

    /// Who holds `(dev, ino)` open, from a snapshot at most [`HOLDER_TTL`] old.
    fn holders_of(&self, dev: i32, ino: u64) -> Result<HolderReport, String> {
        let mut slot = self.holders.lock().unwrap_or_else(|p| p.into_inner());
        if slot.as_ref().is_none_or(|(at, _)| at.elapsed() >= HOLDER_TTL) {
            let index = FdIndex::build().map_err(|e| format!("cannot list processes: {}", describe(e)))?;
            *slot = Some((Instant::now(), index));
        }
        Ok(slot.as_ref().map(|(_, i)| i.holders(dev, ino)).unwrap_or(HolderReport { holders: vec![], uninspected: 0 }))
    }

    /// In-use check of a verified object: any descriptor (or working directory) of an inspectable process. This
    /// process's own descriptors are the executor's verification handles (a planned directory is held as the parent
    /// of the items removed before it, and the snapshot may be up to [`HOLDER_TTL`] old), so they never count; its
    /// working directory does.
    fn in_use(&self, live: &libc::stat) -> Result<(), Stop> {
        let mut report = self.holders_of(live.st_dev, live.st_ino).map_err(Stop::Error)?;
        let me = std::process::id() as i32;
        report.holders.retain(|h| !(h.pid == me && matches!(h.hold, super::procs::Hold::Fd(_))));
        if let Some(h) = report.holders.first() {
            cm_core::debug_trace!("execute", "in use: pid {} ({}) holds the object; {} holder(s)", h.pid, h.name, report.holders.len());
            return Err(Stop::InUse);
        }
        if !report.complete() {
            // Limit, not absence: another user's process may hold it unseen.
            cm_core::debug_trace!("execute", "no visible holder; {} process(es) of other users could not be inspected", report.uninspected);
        }
        Ok(())
    }
}

/// The executor's file actions for this store (`plan apply`, `operations reconcile`, plan/v2 backends).
pub fn file_actions(store_dir: &Path) -> impl FileActions {
    MacActions::with_store(store_dir.to_path_buf())
}

// ---------------------------------------------------------------------------------------------
// Paths

fn unix_bytes(p: &RawPath) -> Option<&[u8]> {
    match p {
        RawPath::Unix(b) => Some(b),
        RawPath::Windows(_) => None,
    }
}

fn os_path(b: &[u8]) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(b))
}

fn cstring(b: &[u8]) -> Result<CString, Stop> {
    CString::new(b).map_err(|_| Stop::Error("path contains NUL".into()))
}

/// A single path component safe to use relative to a directory descriptor.
pub(crate) fn valid_component(name: &[u8]) -> bool {
    !name.is_empty() && name.len() <= 255 && name != b"." && name != b".." && !name.contains(&b'/') && !name.contains(&0)
}

/// An absolute planned path split at its scope root.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ScopeSplit {
    pub root: Vec<u8>,
    /// Planned directories between the scope root and the target, top-down.
    pub below: Vec<Vec<u8>>,
    pub name: Vec<u8>,
}

/// Split an absolute path whose last `depth` components lie below the scope root.
pub(crate) fn split_scope(path: &[u8], depth: usize) -> Result<ScopeSplit, Stop> {
    if path.first() != Some(&b'/') {
        return Err(Stop::Unsupported("not an absolute macOS path".into()));
    }
    let comps: Vec<&[u8]> = path.split(|&b| b == b'/').filter(|c| !c.is_empty()).collect();
    if comps.iter().any(|c| *c == b"." || *c == b"..") {
        return Err(Stop::Error("path is not normalized".into()));
    }
    if depth == 0 || comps.len() < depth {
        return Err(Stop::Parent);
    }
    let split = comps.len() - depth;
    let mut root = Vec::new();
    for c in &comps[..split] {
        root.push(b'/');
        root.extend_from_slice(c);
    }
    if root.is_empty() {
        root.push(b'/');
    }
    let below = comps[split..comps.len() - 1].iter().map(|c| c.to_vec()).collect();
    Ok(ScopeSplit { root, below, name: comps[comps.len() - 1].to_vec() })
}

/// `(parent, name)` of an absolute path; `None` for `/` itself.
fn split_last(path: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let end = path.iter().rposition(|&b| b != b'/')? + 1;
    let sep = path[..end].iter().rposition(|&b| b == b'/')?;
    let parent = if sep == 0 { b"/".to_vec() } else { path[..sep].to_vec() };
    Some((parent, path[sep + 1..end].to_vec()))
}

fn c_chars(chars: &[libc::c_char]) -> Vec<u8> {
    chars.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect()
}

fn euid() -> u32 {
    // SAFETY: plain getter without arguments.
    unsafe { libc::geteuid() }
}

// ---------------------------------------------------------------------------------------------
// Verification

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Stop {
    Missing,
    Reparse,
    Identity,
    Parent,
    NotFile,
    HardLinks(u32),
    Metadata(String),
    InUse,
    /// Immutable, restricted, append-only or no-unlink flags (on the object or its parent).
    Protected(String),
    /// The namespace from the scope root to the parent is not trusted (§7).
    Gate(String),
    Denied(String),
    Unsupported(String),
    Error(String),
}

impl Stop {
    /// A directory open or lookup failure on the planned chain.
    fn from_dir(e: i32) -> Stop {
        match e {
            libc::ENOENT => Stop::Missing,
            libc::ELOOP | libc::ENOTDIR => Stop::Parent,
            libc::EACCES | libc::EPERM => Stop::Denied(format!("access denied: {}", describe(e))),
            libc::EDEADLK => Stop::Unsupported("cloud_materialization_avoided: a planned directory is not available locally".into()),
            _ => Stop::Error(format!("cannot open a planned directory: {}", describe(e))),
        }
    }

    /// A lookup failure of the target name itself.
    fn from_target(e: i32) -> Stop {
        match e {
            libc::ENOENT => Stop::Missing,
            libc::EACCES | libc::EPERM => Stop::Denied(format!("access denied: {}", describe(e))),
            _ => Stop::Error(format!("cannot examine the target: {}", describe(e))),
        }
    }

    pub(crate) fn check(self) -> LiveCheck {
        match self {
            Stop::Missing => LiveCheck::Missing,
            Stop::Reparse => LiveCheck::LinkOrReparse,
            Stop::Identity => LiveCheck::IdentityChanged,
            Stop::Parent => LiveCheck::ParentChanged,
            Stop::NotFile => LiveCheck::MetadataChanged("kind: no longer a regular file".into()),
            Stop::HardLinks(n) => LiveCheck::Unsupported(format!("file has {n} hard links")),
            Stop::Metadata(m) => LiveCheck::MetadataChanged(m),
            Stop::InUse => LiveCheck::InUse,
            Stop::Protected(m) | Stop::Denied(m) => LiveCheck::Protected(m),
            Stop::Gate(m) => LiveCheck::Unsupported(format!("race_safety_unavailable: {m}")),
            Stop::Unsupported(m) => LiveCheck::Unsupported(m),
            Stop::Error(m) => LiveCheck::Unknown(m),
        }
    }

    /// Every stop happens before any change, so it is a skip (precondition) or a failure.
    pub(crate) fn outcome(self) -> ActionOutcome {
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
            Stop::Protected(m) => skip("protected", m),
            Stop::Gate(m) => skip("race_safety_unavailable", format!("the planned location is not a trusted namespace ({m}); another user could swap the name between the check and the action")),
            Stop::Unsupported(m) => skip("unsupported", m),
            Stop::Denied(m) => ActionOutcome::Failed { code: "access_denied".into(), message: m },
            Stop::Error(m) => ActionOutcome::Failed { code: "check_failed".into(), message: m },
        }
    }
}

/// Dataless materialization off for the calling thread while the guard lives; failing to set it stops the action.
pub(crate) fn no_materialize() -> Result<NoMaterialize, Stop> {
    NoMaterialize::enter().map_err(|e| Stop::Unsupported(format!("cloud_materialization_avoided: cannot turn off dataless materialization: {}", describe(e))))
}

/// Volume id and mount flags per device, computed once per verification.
#[derive(Default)]
struct Volumes(HashMap<i32, (String, u32)>);

impl Volumes {
    fn of(&mut self, fd: &Fd, dev: i32) -> Result<(String, u32), Stop> {
        if let Some(v) = self.0.get(&dev) {
            return Ok(v.clone());
        }
        let sfs = fd.statfs().map_err(|e| Stop::Error(format!("cannot query the volume: {}", describe(e))))?;
        let id = ids::volume_id(fd, dev).map_err(|e| Stop::Error(format!("cannot identify the volume: {}", describe(e))))?;
        self.0.insert(dev, (id.clone(), sfs.f_flags));
        Ok((id, sfs.f_flags))
    }
}

fn kind(st: &libc::stat) -> libc::mode_t {
    st.st_mode & libc::S_IFMT
}

fn protected_flags(flags: u32) -> Option<&'static str> {
    if flags & SF_RESTRICTED != 0 {
        Some("protected by System Integrity Protection (restricted)")
    } else if flags & (libc::UF_IMMUTABLE | libc::SF_IMMUTABLE) != 0 {
        Some("marked immutable (locked)")
    } else if flags & SF_NOUNLINK != 0 {
        Some("marked no-unlink")
    } else if flags & (SF_APPEND | UF_APPEND) != 0 {
        Some("marked append-only")
    } else {
        None
    }
}

fn dataless(flags: u32) -> Option<Stop> {
    (flags & SF_DATALESS != 0)
        .then(|| Stop::Unsupported("cloud placeholder (dataless): its content is not on this Mac and is never downloaded to act on it".into()))
}

/// The trusted-namespace rule for one directory on the chain (§7). `acl_open` reports whether an ACL entry lets
/// another principal add, remove or rename entries.
pub(crate) fn namespace_gate(st: &libc::stat, mount_flags: u32, euid: u32, acl_open: impl FnOnce() -> Result<bool, String>) -> Result<(), String> {
    if mount_flags & libc::MNT_IGNORE_OWNERSHIP as u32 != 0 {
        return Err("the volume ignores ownership".into());
    }
    if st.st_uid != euid && st.st_uid != 0 {
        return Err(format!("a directory is owned by another user (uid {})", st.st_uid));
    }
    if st.st_mode & libc::S_ISVTX != 0 {
        return Err("a directory is shared (sticky)".into());
    }
    if st.st_mode & (libc::S_IWGRP | libc::S_IWOTH) != 0 {
        return Err("a directory is writable by its group or by everyone".into());
    }
    match acl_open() {
        Ok(false) => Ok(()),
        Ok(true) => Err("an access control entry lets another principal change a directory".into()),
        Err(e) => Err(e),
    }
}

/// Planned directory checks: identity, kind, cloud state and the namespace gate.
fn check_dir(fd: &Fd, st: &libc::stat, expected: &Identity, vols: &mut Volumes) -> Result<(), Stop> {
    let (volume, flags) = vols.of(fd, st.st_dev)?;
    if kind(st) != libc::S_IFDIR || volume != expected.volume_id || ids::file_id(st.st_ino) != expected.wire_file_id() {
        return Err(Stop::Parent);
    }
    if let Some(stop) = dataless(st.st_flags) {
        return Err(stop);
    }
    namespace_gate(st, flags, euid(), || acl::grants_change(fd)).map_err(Stop::Gate)
}

/// Size/time/link drift against the planned observation (fields the plan did not record are skipped).
fn metadata_change(obs: &ObservationRecord, st: &libc::stat) -> Option<String> {
    let size = u64::try_from(st.st_size).unwrap_or(0);
    if let Some(b) = obs.logical_bytes {
        if b.0 != size {
            return Some(format!("logical_bytes changed: planned {}, now {size}", b.0));
        }
    }
    if let Some(n) = obs.link_count {
        if n != u32::from(st.st_nlink) {
            return Some(format!("link_count changed: planned {n}, now {}", st.st_nlink));
        }
    }
    let m = obs.modified.as_ref()?;
    let Ok(planned) = m.value.parse::<i128>() else { return Some("planned modified time is unreadable".into()) };
    let same = match m.unit {
        TimeUnit::UnixNanoseconds => {
            let now_ns = i128::from(st.st_mtime) * 1_000_000_000 + i128::from(st.st_mtime_nsec);
            if planned % 1_000_000_000 == 0 {
                // Observed with whole-second precision (index `modified_unix`).
                planned.div_euclid(1_000_000_000) == i128::from(st.st_mtime)
            } else {
                planned == now_ns
            }
        }
        TimeUnit::WindowsFiletime100ns => false,
    };
    (!same).then(|| "modified time changed".into())
}

/// The target's live metadata against the plan. `volume` is the parent's volume id; an object on another device
/// (something mounted at the name) is never the planned one. `directory`: the planned object is a directory (kind
/// must match; size and time are not compared because removing its planned contents changes them).
pub(crate) fn verify_live(t: &ActionTarget<'_>, st: &libc::stat, parent_dev: i32, volume: &str, directory: bool) -> Result<(), Stop> {
    if kind(st) == libc::S_IFLNK {
        return Err(Stop::Reparse);
    }
    if st.st_dev != parent_dev || volume != t.identity.volume_id || ids::file_id(st.st_ino) != t.identity.file_id {
        return Err(Stop::Identity);
    }
    if let Some(stop) = dataless(st.st_flags) {
        return Err(stop);
    }
    if let Some(why) = protected_flags(st.st_flags) {
        return Err(Stop::Protected(format!("the planned object is {why}")));
    }
    if directory {
        return if kind(st) == libc::S_IFDIR { Ok(()) } else { Err(Stop::Metadata("kind: no longer a directory".into())) };
    }
    if kind(st) != libc::S_IFREG {
        return Err(Stop::NotFile);
    }
    if st.st_nlink > 1 {
        return Err(Stop::HardLinks(u32::from(st.st_nlink)));
    }
    match metadata_change(t.observation, st) {
        Some(m) => Err(Stop::Metadata(m)),
        None => Ok(()),
    }
}

/// The verified parent directory (held open), the target's name in it and its live metadata.
pub(crate) struct Verified {
    pub parent: Fd,
    pub name: CString,
    pub live: libc::stat,
}

/// Walk the planned chain from the scope root with descriptors, verifying identity and the namespace gate at every
/// level, then examine the target without following it.
pub(crate) fn verify(t: &ActionTarget<'_>, directory: bool) -> Result<Verified, Stop> {
    let bytes = unix_bytes(t.path).ok_or_else(|| Stop::Unsupported("not a macOS path".into()))?;
    let ScopeSplit { root, below, name } = split_scope(bytes, t.parent_chain.len())?;
    let expected: Vec<&Identity> = t.parent_chain.iter().rev().collect(); // scope root first
    let mut vols = Volumes::default();
    let mut dir = Fd::open_dir(&cstring(&root)?).map_err(Stop::from_dir)?;
    let mut st = dir.stat().map_err(|e| Stop::Error(format!("cannot query the scope root: {}", describe(e))))?;
    check_dir(&dir, &st, expected[0], &mut vols)?;
    for (comp, want) in below.iter().zip(&expected[1..]) {
        // Look before entering: a link is never followed and a dataless directory is never opened.
        let seen = dir.stat_child(comp).map_err(Stop::from_dir)?;
        if kind(&seen) != libc::S_IFDIR {
            return Err(Stop::Parent);
        }
        if let Some(stop) = dataless(seen.st_flags) {
            return Err(stop);
        }
        let next = dir.open_child_dir(&cstring(comp)?).map_err(Stop::from_dir)?;
        st = next.stat().map_err(|e| Stop::Error(format!("cannot query a planned directory: {}", describe(e))))?;
        if (st.st_dev, st.st_ino) != (seen.st_dev, seen.st_ino) {
            return Err(Stop::Parent);
        }
        check_dir(&next, &st, want, &mut vols)?;
        dir = next;
    }
    if let Some(why) = protected_flags(st.st_flags) {
        return Err(Stop::Protected(format!("the parent directory is {why}")));
    }
    let live = dir.stat_child(&name).map_err(Stop::from_target)?;
    let (volume, _) = vols.of(&dir, st.st_dev)?;
    verify_live(t, &live, st.st_dev, &volume, directory)?;
    Ok(Verified { parent: dir, name: cstring(&name)?, live })
}

/// What a name inside a verified parent holds now, relative to the verified object.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AtPath {
    Planned,
    Other,
    Missing,
    Error(String),
}

fn at_name(parent: &Fd, name: &CString, planned: &libc::stat) -> AtPath {
    match parent.stat_child(name.as_bytes()) {
        Ok(st) if (st.st_dev, st.st_ino) == (planned.st_dev, planned.st_ino) => AtPath::Planned,
        Ok(_) => AtPath::Other,
        Err(libc::ENOENT) => AtPath::Missing,
        Err(e) => AtPath::Error(describe(e)),
    }
}

/// What sits at an absolute path now, relative to a planned identity (read-only; no gate).
pub(crate) fn at_path(path: &[u8], volume_id: &str, file_id: &str) -> AtPath {
    let Some((parent, name)) = split_last(path) else { return AtPath::Error("not a file path".into()) };
    let Ok(cparent) = CString::new(parent) else { return AtPath::Error("path contains NUL".into()) };
    let dir = match Fd::open_dir_following(&cparent) {
        Ok(d) => d,
        Err(libc::ENOENT | libc::ENOTDIR) => return AtPath::Missing,
        Err(e) => return AtPath::Error(describe(e)),
    };
    let st = match dir.stat_child(&name) {
        Ok(st) => st,
        Err(libc::ENOENT) => return AtPath::Missing,
        Err(e) => return AtPath::Error(describe(e)),
    };
    match ids::volume_id(&dir, st.st_dev) {
        Ok(v) if v == volume_id && ids::file_id(st.st_ino) == file_id => AtPath::Planned,
        Ok(_) => AtPath::Other,
        Err(e) => AtPath::Error(describe(e)),
    }
}

/// Volume id (scanner spelling) and mount point of the directory at `dir` (links followed: this only chooses a
/// location; the move itself re-checks the quarantine directory on its descriptor). `None` on a read-only volume.
fn volume_of_dir(dir: &Path) -> Option<(String, PathBuf)> {
    let fd = Fd::open_dir_following(&CString::new(dir.as_os_str().as_bytes()).ok()?).ok()?;
    let st = fd.stat().ok()?;
    let sfs = fd.statfs().ok()?;
    if sfs.f_flags & libc::MNT_RDONLY as u32 != 0 {
        return None;
    }
    Some((ids::volume_id(&fd, st.st_dev).ok()?, os_path(&c_chars(&sfs.f_mntonname))))
}

fn unlink_errno(e: i32) -> &'static str {
    match e {
        libc::EACCES | libc::EPERM => "access_denied",
        libc::EBUSY => "in_use",
        _ => "delete_failed",
    }
}

/// `unlinkat(parent, name, flags)`; `Err(errno)` when refused.
fn unlink_at(parent: &Fd, name: &CString, flags: c_int) -> Result<(), i32> {
    // SAFETY: valid directory fd and NUL-terminated single-component name.
    if unsafe { libc::unlinkat(parent.raw(), name.as_ptr(), flags) } == 0 { Ok(()) } else { Err(errno()) }
}

/// Create `dir` (and missing parents) private to this user, then require it to be a real directory owned by the
/// effective user with no group/other access bits.
fn ensure_private_dir(dir: &Path) -> Result<(), String> {
    if let Err(e) = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir) {
        return Err(format!("cannot create {}: {e}", dir.display()));
    }
    let md = std::fs::symlink_metadata(dir).map_err(|e| format!("cannot examine {}: {e}", dir.display()))?;
    if !md.file_type().is_dir() || md.uid() != euid() || md.mode() & 0o077 != 0 {
        return Err(format!("{} is not a private directory of this user", dir.display()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Access control lists (`<sys/acl.h>`, `<membership.h>`)

mod acl {
    use super::*;

    type Acl = *mut c_void;
    type Entry = *mut c_void;
    type Permset = *mut c_void;

    const ACL_TYPE_EXTENDED: c_int = 0x0000_0100;
    const ACL_FIRST_ENTRY: c_int = 0;
    const ACL_NEXT_ENTRY: c_int = -1;
    const ACL_EXTENDED_ALLOW: c_int = 1;
    const ID_TYPE_UID: c_int = 0;
    /// `ACL_ADD_FILE`, `ACL_DELETE`, `ACL_ADD_SUBDIRECTORY`, `ACL_DELETE_CHILD`, `ACL_WRITE_SECURITY`,
    /// `ACL_CHANGE_OWNER` (`<sys/kauth.h>` vnode rights): anything that changes a directory's entries or its guard.
    const CHANGING_PERMS: [c_int; 6] = [1 << 2, 1 << 4, 1 << 5, 1 << 6, 1 << 12, 1 << 13];

    unsafe extern "C" {
        /// `acl_t acl_get_fd_np(int fd, acl_type_t type);`
        fn acl_get_fd_np(fd: c_int, ty: c_int) -> Acl;
        /// `int acl_get_entry(acl_t acl, int entry_id, acl_entry_t *entry_p);`
        fn acl_get_entry(acl: Acl, entry_id: c_int, entry: *mut Entry) -> c_int;
        /// `int acl_get_tag_type(acl_entry_t entry_d, acl_tag_t *tag_type_p);`
        fn acl_get_tag_type(entry: Entry, tag: *mut c_int) -> c_int;
        /// `int acl_get_permset(acl_entry_t entry_d, acl_permset_t *permset_p);`
        fn acl_get_permset(entry: Entry, permset: *mut Permset) -> c_int;
        /// `int acl_get_perm_np(acl_permset_t permset_d, acl_perm_t perm);`
        fn acl_get_perm_np(permset: Permset, perm: c_int) -> c_int;
        /// `void *acl_get_qualifier(acl_entry_t entry_d);` (a `guid_t`, freed with `acl_free`)
        fn acl_get_qualifier(entry: Entry) -> *mut c_void;
        /// `int acl_free(void *obj_p);`
        fn acl_free(obj: *mut c_void) -> c_int;
        /// `int mbr_uuid_to_id(const uuid_t uu, id_t *id, int *id_type);`
        fn mbr_uuid_to_id(uu: *const u8, id: *mut libc::id_t, id_type: *mut c_int) -> c_int;
    }

    struct Owned(*mut c_void);

    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: allocated by the ACL library and freed exactly once.
            unsafe { acl_free(self.0) };
        }
    }

    /// Whether an allowing entry for anyone but the effective user grants a changing right on the directory.
    /// No ACL (or a file system without ACLs) is `false`; an unreadable ACL is an error (not trusted).
    pub(super) fn grants_change(fd: &Fd) -> Result<bool, String> {
        // SAFETY: valid descriptor; NULL means no ACL or an error in errno.
        let acl = unsafe { acl_get_fd_np(fd.raw(), ACL_TYPE_EXTENDED) };
        if acl.is_null() {
            return match errno() {
                libc::ENOENT | libc::ENOTSUP | libc::EOPNOTSUPP => Ok(false),
                e => Err(format!("cannot read a directory's access control list: {}", describe(e))),
            };
        }
        let acl = Owned(acl);
        let me = euid();
        let mut which = ACL_FIRST_ENTRY;
        loop {
            let mut entry: Entry = std::ptr::null_mut();
            // SAFETY: valid ACL and out-pointer; returns -1 past the last entry.
            if unsafe { acl_get_entry(acl.0, which, &mut entry) } != 0 {
                return Ok(false);
            }
            which = ACL_NEXT_ENTRY;
            let mut tag = 0;
            // SAFETY: entry returned by acl_get_entry for this ACL; valid out-pointer.
            if unsafe { acl_get_tag_type(entry, &mut tag) } != 0 || tag != ACL_EXTENDED_ALLOW {
                continue;
            }
            let mut perms: Permset = std::ptr::null_mut();
            // SAFETY: as above.
            if unsafe { acl_get_permset(entry, &mut perms) } != 0 {
                return Err("cannot read an access control entry".into());
            }
            // SAFETY: permset of this entry; a plain query per right.
            if !CHANGING_PERMS.iter().any(|&p| unsafe { acl_get_perm_np(perms, p) } == 1) {
                continue;
            }
            // SAFETY: entry of this ACL; the returned guid_t (16 bytes) is owned by the caller.
            let q = unsafe { acl_get_qualifier(entry) };
            if q.is_null() {
                return Ok(true);
            }
            let q = Owned(q);
            let (mut id, mut id_type): (libc::id_t, c_int) = (0, -1);
            // SAFETY: q points to a 16-byte uuid; valid out-pointers.
            let rc = unsafe { mbr_uuid_to_id(q.0.cast::<u8>(), &mut id, &mut id_type) };
            if rc != 0 || id_type != ID_TYPE_UID || id != me {
                return Ok(true);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Owner and catalog probes

/// Probes required by the rules that produced the candidate. `open_file_handle` is answered by the in-use check;
/// any other probe kind has no macOS answer in this build and is never assumed to pass.
/// Catalog probes re-run just before acting: the owner-process probe (`owner_process`, through the
/// process snapshot of `probes`) and the per-path probes of `probes::execution_check`. Open handles
/// are covered by `in_use`; any other probe kind stays Unknown, so the item is skipped.
fn probe_check(t: &ActionTarget<'_>) -> Option<LiveCheck> {
    if t.probes.iter().any(|p| p == "owner_process") {
        let Some(owner) = t.owner_id else { return Some(LiveCheck::Unknown("owner_process probe without an owner".into())) };
        if let Some(c) = super::probes::owner_running_check(owner) {
            return Some(c);
        }
    }
    if let Some(c) = super::probes::execution_check(t) {
        return Some(c);
    }
    const KNOWN: [&str; 5] = ["open_file_handle", "owner_process", "endpoint_security_path", "elevated_token", "incomplete_download_handle"];
    t.probes
        .iter()
        .find(|p| !KNOWN.contains(&p.as_str()))
        .map(|p| LiveCheck::Unknown(format!("probe `{p}` is not available on macOS in this build")))
}

// ---------------------------------------------------------------------------------------------
// FileActions

impl FileActions for MacActions {
    fn check(&self, t: &ActionTarget<'_>) -> LiveCheck {
        let _guard = match no_materialize() {
            Ok(g) => g,
            Err(stop) => return stop.check(),
        };
        let v = match verify(t, false) {
            Ok(v) => v,
            Err(stop) => return stop.check(),
        };
        if let Err(stop) = self.in_use(&v.live) {
            return stop.check();
        }
        probe_check(t).unwrap_or(LiveCheck::Match)
    }

    fn quarantine(&self, t: &ActionTarget<'_>, quarantine_dir: &RawPath, name: &str) -> ActionOutcome {
        let failed = |code: &str, message: String| ActionOutcome::Failed { code: code.into(), message };
        let _guard = match no_materialize() {
            Ok(g) => g,
            Err(stop) => return stop.outcome(),
        };
        if !valid_component(name.as_bytes()) {
            return failed("invalid_name", "quarantine name must be a single path component".into());
        }
        let Some(qbytes) = unix_bytes(quarantine_dir) else { return failed("unsupported", "not a macOS path".into()) };
        // Created private to this user; an existing one is judged below on its descriptor.
        if let Err(e) = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(os_path(qbytes)) {
            return failed("quarantine_unavailable", format!("cannot create quarantine directory: {e}"));
        }
        // Hold the quarantine directory so the destination cannot be swapped during the rename.
        let Ok(cq) = CString::new(qbytes) else { return failed("quarantine_unavailable", "quarantine path contains NUL".into()) };
        let qdir = match Fd::open_dir(&cq) {
            Ok(d) => d,
            Err(libc::ELOOP | libc::ENOTDIR) => return failed("quarantine_unsafe", "quarantine path is a link or not a directory".into()),
            Err(e) => return failed("quarantine_unavailable", format!("cannot open quarantine directory: {}", describe(e))),
        };
        let qst = match qdir.stat() {
            Ok(s) => s,
            Err(e) => return failed("quarantine_unavailable", format!("cannot query quarantine directory: {}", describe(e))),
        };
        if qst.st_uid != euid() || qst.st_mode & 0o077 != 0 {
            return failed("quarantine_unsafe", "quarantine directory is not private to this user".into());
        }
        let v = match verify(t, false) {
            Ok(v) => v,
            Err(stop) => return stop.outcome(),
        };
        if let Err(stop) = self.in_use(&v.live) {
            return stop.outcome();
        }
        if qst.st_dev != v.live.st_dev {
            return failed("cross_volume", "quarantine directory is on another volume; refusing copy-and-delete".into());
        }
        let qname = CString::new(name).expect("validated component");
        #[cfg(test)]
        tests::before_rename();
        // SAFETY: valid directory fds and NUL-terminated single-component names; RENAME_EXCL never replaces.
        let rc = unsafe { libc::renameatx_np(v.parent.raw(), v.name.as_ptr(), qdir.raw(), qname.as_ptr(), libc::RENAME_EXCL) };
        if rc != 0 {
            let e = errno();
            return match e {
                libc::EEXIST => failed("destination_exists", "a file with that name is already in quarantine; nothing replaced".into()),
                libc::EXDEV => failed("cross_volume", format!("rename crosses volumes: {}", describe(e))),
                libc::EACCES | libc::EPERM => failed("access_denied", format!("rename refused: {}", describe(e))),
                libc::EBUSY => failed("in_use", format!("rename refused: {}", describe(e))),
                libc::ENOTSUP | libc::EINVAL => failed("rename_failed", "the volume cannot rename without replacing (no exclusive rename); nothing moved".into()),
                _ => failed("rename_failed", format!("rename refused: {}", describe(e))),
            };
        }
        let location = RawPath::unix(name.as_bytes().to_vec()).and_then(|n| quarantine_dir.join(&n));
        let shown = location.as_ref().map(RawPath::display).unwrap_or_else(|_| format!("{}/{name}", quarantine_dir.display()));
        // The object now under the quarantine name must be the verified one.
        match at_name(&qdir, &qname, &v.live) {
            AtPath::Planned => match location {
                Ok(location) => ActionOutcome::Succeeded { recovery: Recovery::Quarantine { location } },
                Err(e) => ActionOutcome::Unknown { message: format!("moved, but the location cannot be recorded: {e}") },
            },
            AtPath::Other => ActionOutcome::Unknown {
                message: format!(
                    "the name was replaced just before the move: the object now in quarantine at {shown} is not the planned file. It was not moved back automatically; check it and move it back to {} if it belongs there",
                    t.path.display()
                ),
            },
            AtPath::Missing => ActionOutcome::Unknown { message: format!("the move was reported but nothing is at {shown}; check {} and the quarantine", t.path.display()) },
            AtPath::Error(e) => ActionOutcome::Unknown { message: format!("moved, but the quarantined object at {shown} cannot be confirmed: {e}") },
        }
    }

    fn trash(&self, t: &ActionTarget<'_>) -> ActionOutcome {
        trash::trash(self, t)
    }

    fn delete(&self, t: &ActionTarget<'_>) -> ActionOutcome {
        let _guard = match no_materialize() {
            Ok(g) => g,
            Err(stop) => return stop.outcome(),
        };
        let v = match verify(t, false) {
            Ok(v) => v,
            Err(stop) => return stop.outcome(),
        };
        if let Err(stop) = self.in_use(&v.live) {
            return stop.outcome();
        }
        // RESIDUAL RACE (§7): the name is looked up again by the kernel; only a process of this user or root can
        // have swapped it since the check above (namespace gate).
        if let Err(e) = unlink_at(&v.parent, &v.name, 0) {
            return ActionOutcome::Failed { code: unlink_errno(e).into(), message: format!("delete refused: {}", describe(e)) };
        }
        match at_name(&v.parent, &v.name, &v.live) {
            AtPath::Missing | AtPath::Other => ActionOutcome::Succeeded { recovery: Recovery::Unavailable },
            AtPath::Planned => ActionOutcome::Unknown { message: "file is still present after delete".into() },
            AtPath::Error(e) => ActionOutcome::Unknown { message: format!("delete unconfirmed: {e}") },
        }
    }

    fn quarantine_dir_for(&self, t: &ActionTarget<'_>, operation_id: &str) -> Option<RawPath> {
        if !valid_component(operation_id.as_bytes()) {
            return None;
        }
        // Decided by the planned volume, not the live object: reconciliation asks after the object may have moved.
        let volume = &t.identity.volume_id;
        let _guard = no_materialize().ok();
        if let Some(store) = self.store_dir.clone().or_else(crate::paths::default_store_dir) {
            // The store may not exist yet: its nearest existing ancestor decides the volume.
            if store.ancestors().find(|p| p.exists()).and_then(volume_of_dir).is_some_and(|(v, _)| &v == volume) {
                return RawPath::unix(store.join("quarantine").join(operation_id).as_os_str().as_bytes().to_vec()).ok();
            }
        }
        // Same-volume fallback at the volume's mount point, private to this user; never on a read-only (sealed
        // system) volume.
        let near = os_path(unix_bytes(t.path)?);
        let (v, mount) = near.ancestors().skip(1).find(|p| p.exists()).and_then(volume_of_dir)?;
        if &v != volume {
            return None;
        }
        let base = mount.join(format!(".cleanmanager-quarantine-{}", euid()));
        let dir = base.join(operation_id);
        ensure_private_dir(&base).ok()?;
        ensure_private_dir(&dir).ok()?;
        if volume_of_dir(&dir)?.0 != *volume {
            return None;
        }
        RawPath::unix(dir.as_os_str().as_bytes().to_vec()).ok()
    }

    fn reconcile(&self, t: &ActionTarget<'_>, mode: Mode, quarantine_location: Option<&RawPath>) -> Reconciled {
        let Some(path) = unix_bytes(t.path) else { return Reconciled::StillUnknown };
        let _guard = no_materialize().ok();
        let (vol, fid) = (&t.identity.volume_id, &t.identity.file_id);
        let original = at_path(path, vol, fid);
        match mode {
            Mode::Delete => match original {
                AtPath::Planned => Reconciled::NotPerformed,
                AtPath::Missing => Reconciled::Performed,
                // A different file at the name, or an unreadable path, proves nothing.
                AtPath::Other | AtPath::Error(_) => Reconciled::StillUnknown,
            },
            Mode::Quarantine => {
                let moved = quarantine_location.and_then(unix_bytes).map(|q| at_path(q, vol, fid));
                if moved == Some(AtPath::Planned) {
                    Reconciled::Performed
                } else if original == AtPath::Planned {
                    Reconciled::NotPerformed
                } else {
                    Reconciled::StillUnknown
                }
            }
            // The Trash is TCC-protected and its item names are chosen by the system, so a vanished file stays
            // unknown for a person to check there.
            Mode::Trash => match original {
                AtPath::Planned => Reconciled::NotPerformed,
                _ => Reconciled::StillUnknown,
            },
        }
    }

    fn free_space(&self, path: &RawPath) -> Option<u64> {
        let mut p = os_path(unix_bytes(path)?);
        // The planned file may be gone already: the nearest existing ancestor is on the same volume.
        loop {
            if let Ok(c) = CString::new(p.as_os_str().as_bytes()) {
                // SAFETY: zeroed POD out-parameter of the declared type.
                let mut sfs: libc::statfs = unsafe { std::mem::zeroed() };
                // SAFETY: NUL-terminated path and valid out-pointer.
                if unsafe { libc::statfs(c.as_ptr(), &mut sfs) } == 0 {
                    return sfs.f_bavail.checked_mul(u64::from(sfs.f_bsize));
                }
            }
            if !p.pop() {
                return None;
            }
        }
    }

    fn remove_empty_dir(&self, t: &ActionTarget<'_>) -> ActionOutcome {
        let _guard = match no_materialize() {
            Ok(g) => g,
            Err(stop) => return stop.outcome(),
        };
        let v = match verify(t, true) {
            Ok(v) => v,
            Err(stop) => return stop.outcome(),
        };
        // An open descriptor or a working directory inside keeps it (as an open handle does on Windows).
        if let Err(stop) = self.in_use(&v.live) {
            return stop.outcome();
        }
        let new_descendant = || ActionOutcome::Skipped {
            code: "new_descendant".into(),
            message: "something not in the plan is inside the directory; it is kept and nothing inside is touched".into(),
        };
        // List the verified directory itself through a no-follow descriptor.
        let dir = match v.parent.open_child_dir(&v.name) {
            Ok(d) => d,
            Err(libc::ELOOP | libc::ENOTDIR) => return Stop::Reparse.outcome(),
            Err(e) => return Stop::from_target(e).outcome(),
        };
        match dir.stat() {
            Ok(st) if (st.st_dev, st.st_ino) == (v.live.st_dev, v.live.st_ino) => {}
            Ok(_) => return Stop::Identity.outcome(),
            Err(e) => return ActionOutcome::Failed { code: "check_failed".into(), message: format!("cannot query the directory: {}", describe(e)) },
        }
        match read_names(&dir) {
            Ok(names) if names.is_empty() => {}
            Ok(_) => return new_descendant(),
            Err(e) => return ActionOutcome::Failed { code: "check_failed".into(), message: format!("cannot list the directory: {}", describe(e)) },
        }
        drop(dir);
        // The kernel refuses to remove a non-empty directory, so an entry arriving after the listing is never swept up.
        if let Err(e) = unlink_at(&v.parent, &v.name, libc::AT_REMOVEDIR) {
            return match e {
                libc::ENOTEMPTY | libc::EEXIST => new_descendant(),
                _ => ActionOutcome::Failed { code: unlink_errno(e).into(), message: format!("delete refused: {}", describe(e)) },
            };
        }
        match at_name(&v.parent, &v.name, &v.live) {
            AtPath::Missing | AtPath::Other => ActionOutcome::Succeeded { recovery: Recovery::Unavailable },
            AtPath::Planned => ActionOutcome::Unknown { message: "directory is still present after removal".into() },
            AtPath::Error(e) => ActionOutcome::Unknown { message: format!("removal unconfirmed: {e}") },
        }
    }
}

#[cfg(test)]
mod tests;
