//! Depth-first, fd-relative walker shared by `mac.posix` and `mac.bulk` (SCN-01/02/03/05/06/07).
//!
//! - The root is opened once with `O_DIRECTORY | O_NOFOLLOW`; every deeper directory is opened
//!   with `openat(parent_fd, name, O_DIRECTORY | O_NOFOLLOW)`, never by a re-parsed path.
//! - Each opened directory is `fstat`-checked against the `(st_dev, st_ino)` its parent listing
//!   reported: a replaced directory is skipped (`identity_changed`), another device is a mount
//!   boundary and is not entered.
//! - Symlinks, mount points and dataless (provider-backed) directories are recorded as
//!   boundaries and not traversed. Files are never opened, so no content is read or recalled.
//! - Unreadable subtrees and per-entry failures become skipped ranges, never empty directories
//!   or zero sizes. Hard links share one object keyed by `(st_dev, st_ino)`.
//! - Names are the raw bytes the kernel returned; nothing is Unicode-normalized.
//! - The scope's root is re-checked (MVP 5.5, SCN-07) every [`RECHECK_EVERY_DIRS`] directories,
//!   after any failed open or listing, and once before reporting `completed`: the held root
//!   descriptor must still answer and the selected path must still name the same directory on
//!   the same volume. A replaced or remounted volume (`volume_changed` / `volume_offline`) or a
//!   replaced root directory (`root_changed`) stops the walk with `partial`; rows already
//!   confirmed are kept, and nothing on the new volume or under the new directory is read, so
//!   it never inherits the old scope. `EDEADLK` (materialization refused, TN3150) is a
//!   `cloud_materialization_avoided` skip and is never retried.
//! - Privacy prompts (docs/platforms/macos.md §6): the root and every privacy-protected location
//!   the walk reaches ([`privacy::classify`]: Desktop, Documents, Downloads, iCloud Drive, File
//!   Provider storage, Photos, Contacts, Calendars, one app's container) are first opened through
//!   [`privacy::Access::run`], since macOS holds that `open(2)` while it shows its "would like to
//!   access…" prompt (and a slow volume holds it too). One not answered within
//!   [`privacy::prompt_wait`] is a `permission_required` skipped range: unknown, never empty. Its
//!   issue names the cause that can be proven: `privacy_prompt_pending` for a location whose
//!   prompt is known on a local volume (the open parent's `fstatfs`), `undetermined` otherwise,
//!   the root of the scan included. At most [`MAX_GATE_ISSUES`] such issues are listed one by
//!   one; the rest are counted in one closing issue. Everything below an opened protected
//!   location is walked as usual, without further gating.

use std::collections::HashMap;
use std::ffi::{CStr, CString, OsStr};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cm_core::capability::CapabilityState;
use cm_core::permission::PermissionCause;
use cm_core::model::{BoundaryKind, CloudState, IdentityQuality, ObjectKind, ScanState, SkipReason};
use cm_core::path::RawPath;
use cm_core::scan::{EntryRow, ObjectRow, RootProbe, ScanIssue, ScanOptions, ScanReport, ScanSink, SkippedRow};

use super::ids;
use super::privacy::{self, GateError};
use super::sys::{Fd, Kind, Meta, NoMaterialize, Record, SF_DATALESS, describe, volume_of};

/// Unanswered first accesses listed one issue each per scan; further ones (each still a skipped
/// range) are counted in one closing issue.
pub(crate) const MAX_GATE_ISSUES: usize = 8;

/// Directory descriptors kept open along the current path. Deeper trees release the
/// shallowest ones and re-derive them (with identity checks) from the nearest held ancestor.
const MAX_HELD_DIR_FDS: usize = 64;
/// Directories entered between two root identity re-checks (`open` + `fstat` + `fstatfs` +
/// one volume-UUID `getattrlist` + `fstat` of the held root).
pub(crate) const RECHECK_EVERY_DIRS: usize = 256;

/// One directory listing as produced by a backend.
pub(crate) struct Listing {
    pub records: Result<Vec<Record>, i32>,
    /// `(code, message)` issues found while listing (e.g. malformed bulk records).
    pub issues: Vec<(&'static str, String)>,
    /// Backend-reason annotations, added once per scan (e.g. a fallback was used).
    pub notes: Vec<&'static str>,
}

impl Listing {
    pub(crate) fn plain(records: Result<Vec<Record>, i32>) -> Self {
        Listing { records, issues: Vec::new(), notes: Vec::new() }
    }
}

/// Produces the records of one opened directory.
pub(crate) trait Lister {
    fn list(&mut self, dir: &Fd) -> Listing;
}

fn root_bytes(root: &RawPath) -> Option<Vec<u8>> {
    let RawPath::Unix(b) = root else { return None };
    if b.first() == Some(&b'/') {
        return Some(b.clone());
    }
    // Relative roots are anchored at the working directory for naming only; nothing is resolved.
    let mut abs = std::env::current_dir().ok()?.into_os_string().into_vec();
    if abs.last() != Some(&b'/') {
        abs.push(b'/');
    }
    abs.extend_from_slice(b);
    Some(abs)
}

fn join(parent: &[u8], name: &[u8]) -> Vec<u8> {
    let mut v = parent.to_vec();
    if v.last() != Some(&b'/') {
        v.push(b'/');
    }
    v.extend_from_slice(name);
    v
}

fn shown(path: &[u8]) -> String {
    RawPath::unix(path.to_vec()).map(|p| p.display()).unwrap_or_else(|_| String::from_utf8_lossy(path).into_owned())
}

fn classify(e: i32) -> (SkipReason, &'static str) {
    match e {
        libc::EACCES | libc::EPERM => (SkipReason::PermissionDenied, "permission_required"),
        // Materialization was refused under the no-materialize policy (TN3150).
        libc::EDEADLK => (SkipReason::CloudBoundary, "cloud_materialization_avoided"),
        _ => (SkipReason::Error, "io_error"),
    }
}

/// Opens one directory without following a link: the scan root by path (`parent` `None`) or a
/// child of `parent` by name. Injected so tests can stand in for an unanswered privacy prompt.
pub(crate) type Opener = Arc<dyn Fn(Option<&Fd>, &CStr) -> Result<Fd, i32> + Send + Sync>;

/// First opens that may wait for a macOS privacy prompt: the root and each protected location.
pub(crate) struct PrivacyGate {
    /// `$HOME`, besides every `/Users/<name>`, for [`privacy::classify`].
    pub home: Option<PathBuf>,
    pub wait: Duration,
    pub open: Opener,
    /// Where waiting workers are counted (tests use their own).
    pub registry: &'static privacy::Registry,
}

fn plain_open(parent: Option<&Fd>, name: &CStr) -> Result<Fd, i32> {
    match parent {
        None => Fd::open_dir(name),
        Some(p) => p.open_child_dir(name),
    }
}

fn as_path(path: &[u8]) -> &Path {
    Path::new(OsStr::from_bytes(path))
}

impl PrivacyGate {
    pub(crate) fn from_os() -> Self {
        PrivacyGate { home: privacy::os_home(), wait: privacy::prompt_wait(), open: Arc::new(plain_open), registry: privacy::global() }
    }

    /// Whether `path` is itself a protected location (below one, its first open was already answered).
    fn guards(&self, path: &[u8]) -> bool {
        privacy::protected_root(as_path(path), self.home.as_deref()).is_some_and(|r| r == as_path(path))
    }

    /// Open `name` (in `parent`, or the root path) on a worker thread, waiting at most `wait`. `Err` is why the
    /// gate gave no result; the worker uses its own duplicate of `parent`, so a late answer never opens relative to
    /// a reused descriptor number. A parent on a network, removable or automounted volume makes a missing answer
    /// `undetermined` (a slow volume answers the same way); the root, with no open parent, is judged by its path.
    fn open(&self, parent: Option<&Fd>, name: &CStr, path: &[u8]) -> Result<Result<Fd, i32>, GateError> {
        let remote = parent.is_some_and(privacy::remote_or_removable);
        let parent = match parent.map(Fd::duplicate).transpose() {
            Ok(p) => p,
            Err(e) => return Ok(Err(e)),
        };
        let (name, open) = (name.to_owned(), Arc::clone(&self.open));
        // Keyed on the location: a container that never prompts is not held up by another one.
        let access = privacy::Access::of(as_path(path), self.home.as_deref()).wait(self.wait).in_registry(self.registry).remote(remote);
        access.run(move || {
            // The scan thread's policy does not carry over; opening a directory does not materialize it anyway.
            let _policy = NoMaterialize::enter();
            open(parent.as_ref(), &name)
        })
    }
}

/// Why an expected directory could not be entered.
enum OpenFail {
    Errno(i32),
    /// The privacy gate gave no result (no answer in time, not tried, or no worker thread).
    Gate(GateError),
    /// Different object (or now a symlink / non-directory) under the listed name.
    Changed,
    /// Another device under the listed name: a mount appeared or was not reported.
    OtherVolume,
    /// The directory became dataless after it was listed.
    Dataless,
}

fn open_verified(parent: &Fd, name: &[u8], dev: i32, ino: u64, root_dev: i32) -> Result<Fd, OpenFail> {
    let c = CString::new(name).map_err(|_| OpenFail::Errno(libc::EINVAL))?;
    verified(parent.open_child_dir(&c), dev, ino, root_dev)
}

/// [`open_verified`] for a protected location: opened through the privacy gate.
fn open_gated(gate: &PrivacyGate, parent: &Fd, name: &[u8], path: &[u8], dev: i32, ino: u64, root_dev: i32) -> Result<Fd, OpenFail> {
    let c = CString::new(name).map_err(|_| OpenFail::Errno(libc::EINVAL))?;
    verified(gate.open(Some(parent), &c, path).map_err(OpenFail::Gate)?, dev, ino, root_dev)
}

/// Check a just-opened child against the identity its parent listing reported.
fn verified(opened: Result<Fd, i32>, dev: i32, ino: u64, root_dev: i32) -> Result<Fd, OpenFail> {
    let fd = opened.map_err(|e| match e {
        // O_NOFOLLOW on a symlink, or a non-directory now sits under the name.
        libc::ELOOP | libc::ENOTDIR => OpenFail::Changed,
        e => OpenFail::Errno(e),
    })?;
    let st = fd.stat().map_err(OpenFail::Errno)?;
    if st.st_dev != root_dev {
        return Err(OpenFail::OtherVolume);
    }
    if st.st_dev != dev || st.st_ino != ino {
        return Err(OpenFail::Changed);
    }
    if st.st_flags & SF_DATALESS != 0 {
        return Err(OpenFail::Dataless);
    }
    Ok(fd)
}

struct Pending {
    entry_id: u64,
    name: Vec<u8>,
    dev: i32,
    ino: u64,
}

struct Frame {
    path: Vec<u8>,
    name: Vec<u8>,
    dev: i32,
    ino: u64,
    fd: Option<Fd>,
    /// Child directories still to visit, in reverse listing order.
    pending: Vec<Pending>,
}

/// Make sure the deepest frame holds a descriptor, re-deriving it from the nearest ancestor
/// that still holds one, one verified `openat` per level.
fn ensure_top_fd(frames: &mut [Frame], root_dev: i32) -> Result<(), OpenFail> {
    let Some(last) = frames.len().checked_sub(1) else { return Ok(()) };
    if frames[last].fd.is_some() {
        return Ok(());
    }
    let Some(start) = (0..last).rev().find(|&i| frames[i].fd.is_some()) else { return Err(OpenFail::Errno(libc::EBADF)) };
    let mut opened: Option<Fd> = None;
    for k in start + 1..=last {
        let next = {
            let parent = match (&opened, &frames[start].fd) {
                (Some(fd), _) | (None, Some(fd)) => fd,
                (None, None) => return Err(OpenFail::Errno(libc::EBADF)),
            };
            open_verified(parent, &frames[k].name, frames[k].dev, frames[k].ino, root_dev)?
        };
        opened = Some(next);
    }
    frames[last].fd = opened;
    Ok(())
}

fn release_excess(frames: &mut [Frame]) {
    if frames.iter().filter(|f| f.fd.is_some()).count() > MAX_HELD_DIR_FDS
        && let Some(f) = frames.iter_mut().skip(1).find(|f| f.fd.is_some())
    {
        f.fd = None;
    }
}

struct Ctx<'a> {
    sink: &'a mut dyn ScanSink,
    batch: usize,
    volume_id: String,
    strong: bool,
    root_dev: i32,
    objects: Vec<ObjectRow>,
    entries: Vec<EntryRow>,
    skipped: Vec<SkippedRow>,
    object_ids: HashMap<(i32, u64), u64>,
    next_object: u64,
    next_entry: u64,
    report: ScanReport,
    /// Unanswered first accesses listed so far, and those only counted (with their common cause).
    gate_issues: usize,
    gate_omitted: u64,
    gate_omitted_cause: Option<PermissionCause>,
}

impl Ctx<'_> {
    fn flush(&mut self) -> Result<(), String> {
        if self.objects.is_empty() && self.entries.is_empty() && self.skipped.is_empty() {
            return Ok(());
        }
        self.sink.write(&self.objects, &self.entries, &self.skipped)?;
        self.objects.clear();
        self.entries.clear();
        self.skipped.clear();
        Ok(())
    }

    fn maybe_flush(&mut self) -> Result<(), String> {
        if self.entries.len() + self.skipped.len() >= self.batch { self.flush() } else { Ok(()) }
    }

    /// Objects are keyed by `(st_dev, st_ino)` so hard links share one object. Inode 0 cannot
    /// be trusted and gets a fresh, weak identity; so does any object on another device
    /// (a mount point's root), since its inode is not an inode of the scope's volume.
    fn object(&mut self, meta: &Meta, kind: ObjectKind, logical: Option<u64>, allocated: Option<u64>, link_count: Option<u32>, cloud: CloudState) -> u64 {
        let keyed = meta.ino != 0;
        if keyed && let Some(id) = self.object_ids.get(&(meta.dev, meta.ino)) {
            return *id;
        }
        self.next_object += 1;
        let id = self.next_object;
        if keyed {
            self.object_ids.insert((meta.dev, meta.ino), id);
        }
        let strong = self.strong && keyed && meta.dev == self.root_dev;
        self.objects.push(ObjectRow {
            object_id: id,
            volume_id: self.volume_id.clone(),
            file_id: ids::file_id(meta.ino),
            generation: None,
            quality: if strong { IdentityQuality::Strong } else { IdentityQuality::Weak },
            kind,
            logical,
            allocated,
            link_count,
            cloud,
            modified_unix: meta.mtime,
            shared_blocks: if kind == ObjectKind::File { meta.shared_blocks } else { None },
        });
        id
    }

    fn entry(&mut self, parent: Option<u64>, name: Vec<u8>, object: u64, kind: ObjectKind, boundary: Option<BoundaryKind>) -> Result<u64, String> {
        let name = RawPath::unix(name).map_err(|e| e.to_string())?;
        self.next_entry += 1;
        let id = self.next_entry;
        self.entries.push(EntryRow { entry_id: id, parent_entry_id: parent, name, object_id: object, kind, boundary });
        self.report.entries += 1;
        if boundary.is_some() {
            self.report.boundaries += 1;
        }
        Ok(id)
    }

    fn skip(&mut self, parent: Option<u64>, path: &[u8], reason: SkipReason) {
        match RawPath::unix(path.to_vec()) {
            Ok(path) => {
                self.skipped.push(SkippedRow { parent_entry_id: parent, path, reason });
                self.report.skipped_ranges += 1;
            }
            Err(e) => self.issue("internal_error", path, e.to_string()),
        }
    }

    fn issue(&mut self, code: &str, path: &[u8], message: String) {
        self.report.issues.push(ScanIssue { code: code.into(), scope_display: shown(path), message });
    }

    fn open_failed(&mut self, entry_id: u64, path: &[u8], fail: OpenFail) {
        if let OpenFail::Gate(GateError::Unanswered { cause, .. }) = &fail {
            if self.gate_issues >= MAX_GATE_ISSUES {
                // Still a skipped range of its own; its issue is folded into the closing count.
                self.gate_omitted += 1;
                self.gate_omitted_cause = Some(match self.gate_omitted_cause {
                    Some(c) if c != *cause => PermissionCause::Undetermined,
                    _ => *cause,
                });
                self.skip(Some(entry_id), path, SkipReason::PermissionDenied);
                return;
            }
            self.gate_issues += 1;
        }
        let (reason, code, message) = fail_info(fail);
        self.issue(code, path, message);
        self.skip(Some(entry_id), path, reason);
    }

    /// The closing issue for unanswered first accesses beyond [`MAX_GATE_ISSUES`].
    fn gate_summary(&mut self, root: &[u8], wait: Duration) {
        let Some(cause) = self.gate_omitted_cause.filter(|_| self.gate_omitted > 0) else { return };
        let message = format!(
            "{}: {} more location(s) were not read: macOS did not answer their first access within {} s, or they were not tried while other first accesses were still waiting ({MAX_GATE_ISSUES} are listed one by one). Each is a skipped range of this scan, unknown and never empty. Answer any \"would like to access…\" prompt, then scan again",
            privacy::prefix(cause),
            self.gate_omitted,
            wait.as_secs_f32()
        );
        self.issue("permission_required", root, message);
    }
}

fn fail_info(fail: OpenFail) -> (SkipReason, &'static str, String) {
    match fail {
            OpenFail::Errno(e) => {
                let (reason, code) = classify(e);
                (reason, code, describe(e))
            }
            OpenFail::Gate(why) => {
                let reason = if why.cause().is_some() { SkipReason::PermissionDenied } else { SkipReason::Error };
                (reason, why.issue_code(), why.into_message())
            }
            OpenFail::Changed => (SkipReason::Error, "identity_changed", "directory was replaced after it was listed; not traversed".into()),
            OpenFail::OtherVolume => (
                SkipReason::VolumeChanged,
                "mount_boundary",
                "another volume is mounted here (not reported by the listing); not traversed".into(),
            ),
            OpenFail::Dataless => (
                SkipReason::CloudBoundary,
                "cloud_materialization_avoided",
                "directory became dataless after it was listed; not entered".into(),
            ),
    }
}

impl Ctx<'_> {
    /// Record one listed name; child directories to traverse are appended to `pending`.
    fn record(&mut self, parent: u64, dir_path: &[u8], rec: Record, pending: &mut Vec<Pending>) -> Result<(), String> {
        let meta = match rec.meta {
            Ok(m) => m,
            Err(libc::ENOENT) => {
                self.issue("entry_vanished", &join(dir_path, &rec.name), "entry disappeared between listing and metadata query".into());
                return Ok(());
            }
            Err(e) => {
                let path = join(dir_path, &rec.name);
                let (reason, code) = classify(e);
                self.issue(code, &path, format!("metadata unavailable: {}", describe(e)));
                self.skip(Some(parent), &path, reason);
                return Ok(());
            }
        };
        let dataless = meta.flags & SF_DATALESS != 0;
        let cloud = if dataless { CloudState::Placeholder } else { CloudState::NotCloud };
        match meta.kind {
            Kind::Symlink => {
                let o = self.object(&meta, ObjectKind::Symlink, None, None, None, cloud);
                self.entry(Some(parent), rec.name, o, ObjectKind::Symlink, Some(BoundaryKind::Symlink))?;
            }
            Kind::Directory if meta.mount || meta.dev != self.root_dev => {
                let o = self.object(&meta, ObjectKind::Directory, None, None, None, cloud);
                self.entry(Some(parent), rec.name, o, ObjectKind::Directory, Some(BoundaryKind::MountPoint))?;
            }
            Kind::Directory if dataless => {
                // Listing a dataless directory can materialize it (SCN-05): recorded, not entered.
                let o = self.object(&meta, ObjectKind::Directory, None, None, None, cloud);
                self.entry(Some(parent), rec.name, o, ObjectKind::Directory, Some(BoundaryKind::CloudProvider))?;
            }
            Kind::Directory => {
                let o = self.object(&meta, ObjectKind::Directory, None, None, None, cloud);
                let id = self.entry(Some(parent), rec.name.clone(), o, ObjectKind::Directory, None)?;
                pending.push(Pending { entry_id: id, name: rec.name, dev: meta.dev, ino: meta.ino });
            }
            Kind::File => {
                // Sizes come from metadata only; a dataless file is never opened.
                let o = self.object(&meta, ObjectKind::File, meta.size, meta.alloc, meta.nlink, cloud);
                self.entry(Some(parent), rec.name, o, ObjectKind::File, None)?;
            }
            Kind::Other => {
                // FIFOs, sockets and devices are never opened; sizes are not meaningful.
                let o = self.object(&meta, ObjectKind::Other, None, None, None, cloud);
                self.entry(Some(parent), rec.name, o, ObjectKind::Other, None)?;
            }
        }
        Ok(())
    }

    /// List one opened directory. `None` when the listing failed (a skipped range is recorded).
    fn list(&mut self, lister: &mut dyn Lister, dir: &Fd, entry_id: u64, path: &[u8]) -> Option<Vec<Pending>> {
        let listing = lister.list(dir);
        for (code, message) in listing.issues {
            self.issue(code, path, message);
        }
        for note in listing.notes {
            if !self.report.backend_reason.contains(note) {
                self.report.backend_reason.push_str("; ");
                self.report.backend_reason.push_str(note);
            }
        }
        let records = match listing.records {
            Ok(r) => r,
            Err(e) => {
                let (reason, code) = classify(e);
                self.issue(code, path, describe(e));
                self.skip(Some(entry_id), path, reason);
                return None;
            }
        };
        let mut pending = Vec::new();
        for rec in records {
            if let Err(msg) = self.record(entry_id, path, rec, &mut pending) {
                self.issue("internal_error", path, msg);
            }
        }
        pending.reverse();
        Some(pending)
    }
}

/// The scope root as it was when the scan started (MVP 5.5).
struct RootWatch {
    /// An own descriptor of the root directory (the walk's may be released with its frame).
    held: Fd,
    path: CString,
    dev: i32,
    ino: u64,
    volume_id: String,
}

/// Why the scope stopped being the one the scan started on.
struct RootChange {
    code: &'static str,
    reason: SkipReason,
    message: String,
}

impl RootWatch {
    /// `None` while the selected path still names the scanned directory on the same volume
    /// and the held root still answers.
    fn changed(&self) -> Option<RootChange> {
        let volume = |message: String| Some(RootChange { code: "volume_changed", reason: SkipReason::VolumeChanged, message });
        let root = |message: String| Some(RootChange { code: "root_changed", reason: SkipReason::Error, message });
        match Fd::open_dir(&self.path).and_then(|fd| fd.stat().map(|st| (fd, st))) {
            Err(e) => return root(format!("the selected root no longer opens ({}); the scan does not continue", describe(e))),
            Ok((fd, st)) if st.st_dev != self.dev => {
                let now = volume_of(&fd, st.st_dev).map(|v| v.id).unwrap_or_else(|_| "unknown".into());
                return volume(if now == self.volume_id {
                    format!("the volume {now} was remounted under the selected root; the scan does not continue on the new mount")
                } else {
                    format!("the selected root is now on volume {now}, not {}; nothing on the new volume is read", self.volume_id)
                });
            }
            Ok((_, st)) if st.st_ino != self.ino => {
                return root("the selected path now names a different directory; it does not inherit this scan".into());
            }
            // A new volume can reuse both the device number and the root inode (a disk image
            // re-attached on the same disk node): only the volume UUID tells them apart.
            Ok((fd, st)) => {
                if let Ok(v) = volume_of(&fd, st.st_dev)
                    && v.id != self.volume_id
                {
                    return volume(format!(
                        "the selected root is now on volume {}, not {} (same device number); nothing on the new volume is read",
                        v.id, self.volume_id
                    ));
                }
            }
        }
        match self.held.stat() {
            Err(e) => Some(RootChange {
                code: "volume_offline",
                reason: SkipReason::VolumeChanged,
                message: format!("the scanned volume no longer answers ({}); the scan does not continue", describe(e)),
            }),
            Ok(st) if st.st_dev != self.dev || st.st_ino != self.ino => volume("the scanned root changed identity under its descriptor".into()),
            Ok(_) => None,
        }
    }
}

pub(crate) fn probe(backend: &'static str, root: &RawPath, method: &str) -> RootProbe {
    probe_gated(backend, root, method, &PrivacyGate::from_os())
}

/// [`probe`] with the privacy gate injected.
pub(crate) fn probe_gated(backend: &'static str, root: &RawPath, method: &str, gate: &PrivacyGate) -> RootProbe {
    let fail = |reason: String, state| RootProbe { filesystem: None, volume_id: None, backend: backend.into(), state, reason };
    let Some(path) = root_bytes(root) else { return fail("not a macOS path".into(), CapabilityState::Unsupported) };
    let Ok(cpath) = CString::new(path.clone()) else { return fail("path contains NUL".into(), CapabilityState::Unsupported) };
    let fd = match gate.open(None, &cpath, &path) {
        Err(why) if why.cause().is_some() => return fail(why.into_message(), CapabilityState::PermissionRequired),
        Err(why) => return fail(why.into_message(), CapabilityState::Unsupported),
        Ok(Ok(fd)) => fd,
        Ok(Err(e @ (libc::EACCES | libc::EPERM))) => return fail(format!("root not readable: {}", describe(e)), CapabilityState::PermissionRequired),
        Ok(Err(libc::ELOOP)) => return fail("root is a symbolic link; links are not followed".into(), CapabilityState::Unsupported),
        Ok(Err(e)) => return fail(format!("root unavailable: {}", describe(e)), CapabilityState::Unsupported),
    };
    let volume = fd.stat().ok().and_then(|st| volume_of(&fd, st.st_dev).ok());
    RootProbe {
        filesystem: volume.as_ref().map(|v| v.filesystem.clone()),
        volume_id: volume.as_ref().map(|v| v.id.clone()),
        backend: backend.into(),
        // Real-device evidence decides `available`; until then the backend is unverified.
        state: CapabilityState::Unverified,
        reason: format!(
            "{method} limited to the selected root on {}; mount points, symlinks and dataless directories are not entered",
            volume.map(|v| v.filesystem).unwrap_or_else(|| "an unknown file system".into())
        ),
    }
}

pub(crate) fn scan(backend: &'static str, method: &str, root: &RawPath, sink: &mut dyn ScanSink, options: &ScanOptions, lister: &mut dyn Lister) -> ScanReport {
    scan_checked(backend, method, root, sink, options, lister, RECHECK_EVERY_DIRS)
}

/// [`scan`] with an explicit root re-check interval (tests use 1).
pub(crate) fn scan_checked(
    backend: &'static str,
    method: &str,
    root: &RawPath,
    sink: &mut dyn ScanSink,
    options: &ScanOptions,
    lister: &mut dyn Lister,
    recheck_every: usize,
) -> ScanReport {
    scan_gated(backend, method, root, sink, options, lister, recheck_every, &PrivacyGate::from_os())
}

/// [`scan_checked`] with the privacy gate injected.
#[allow(clippy::too_many_arguments)]
pub(crate) fn scan_gated(
    backend: &'static str,
    method: &str,
    root: &RawPath,
    sink: &mut dyn ScanSink,
    options: &ScanOptions,
    lister: &mut dyn Lister,
    recheck_every: usize,
    gate: &PrivacyGate,
) -> ScanReport {
    // SAFETY: plain getter without arguments.
    let euid = unsafe { libc::geteuid() };
    let mut report = ScanReport {
        state: ScanState::Failed,
        backend: backend.into(),
        backend_reason: format!("{method} within the selected root"),
        entries: 0,
        skipped_ranges: 0,
        boundaries: 0,
        elevated_read: if euid == 0 { vec!["root".into()] } else { Vec::new() },
        issues: Vec::new(),
    };
    let Some(path) = root_bytes(root) else {
        report.issues.push(ScanIssue { code: "unsupported_path".into(), scope_display: root.display(), message: "not a macOS path".into() });
        return report;
    };
    let Ok(cpath) = CString::new(path.clone()) else {
        report.issues.push(ScanIssue { code: "root_unavailable".into(), scope_display: root.display(), message: "path contains NUL".into() });
        return report;
    };
    // Held for the whole scan: every scan syscall runs on this thread.
    let policy = NoMaterialize::enter();
    let root_fd = match gate.open(None, &cpath, &path) {
        Ok(Ok(fd)) => fd,
        Err(why) => {
            // Unknown, not empty: nothing below the root was read.
            report.issues.push(ScanIssue { code: why.issue_code().into(), scope_display: root.display(), message: why.into_message() });
            return report;
        }
        Ok(Err(e)) => {
            let code = if matches!(e, libc::EACCES | libc::EPERM) { "permission_required" } else { "root_unavailable" };
            let message = if e == libc::ELOOP { "root is a symbolic link; links are not followed".into() } else { describe(e) };
            report.issues.push(ScanIssue { code: code.into(), scope_display: root.display(), message });
            return report;
        }
    };
    let root_meta = match root_fd.stat() {
        Ok(st) => Meta::from_stat(&st),
        Err(e) => {
            report.issues.push(ScanIssue { code: "root_unavailable".into(), scope_display: root.display(), message: describe(e) });
            return report;
        }
    };
    let volume = match volume_of(&root_fd, root_meta.dev) {
        Ok(v) => v,
        Err(e) => {
            report.issues.push(ScanIssue { code: "root_unavailable".into(), scope_display: root.display(), message: describe(e) });
            return report;
        }
    };
    // Strong identity needs a stable volume (UUID) and a file system whose inode numbers are
    // persistent file IDs; everything else is analysis-only.
    let strong = volume.has_uuid && matches!(volume.filesystem.as_str(), "apfs" | "hfs");
    report.backend_reason = if report.elevated_read.is_empty() {
        format!("{method} within the selected root on {} with ordinary permissions", volume.filesystem)
    } else {
        format!(
            "{method} within the selected root on {} WITH root privileges (POSIX permission checks bypassed; TCC still applies)",
            volume.filesystem
        )
    };
    if !volume.has_uuid {
        report.backend_reason.push_str("; volume UUID unavailable, using fsid (not stable across remounts)");
    }
    if let Err(e) = &policy {
        report.issues.push(ScanIssue {
            code: "materialization_policy_unavailable".into(),
            scope_display: root.display(),
            message: format!("could not disable dataless materialization for the scan thread ({}); dataless items are still not entered", describe(*e)),
        });
    }
    let root_dev = root_meta.dev;
    let watch = match root_fd.reopen_self() {
        Ok(held) => RootWatch { held, path: cpath, dev: root_dev, ino: root_meta.ino, volume_id: volume.id.clone() },
        Err(e) => {
            report.issues.push(ScanIssue { code: "root_unavailable".into(), scope_display: root.display(), message: describe(e) });
            return report;
        }
    };
    let mut ctx = Ctx {
        sink,
        batch: options.batch_entries.max(1),
        volume_id: volume.id,
        strong,
        root_dev,
        objects: Vec::new(),
        entries: Vec::new(),
        skipped: Vec::new(),
        object_ids: HashMap::new(),
        next_object: 0,
        next_entry: 0,
        report,
        gate_issues: 0,
        gate_omitted: 0,
        gate_omitted_cause: None,
    };

    let root_dataless = root_meta.flags & SF_DATALESS != 0;
    let root_cloud = if root_dataless { CloudState::Placeholder } else { CloudState::NotCloud };
    let root_obj = ctx.object(&root_meta, ObjectKind::Directory, None, None, None, root_cloud);
    let root_entry = match ctx.entry(None, path.clone(), root_obj, ObjectKind::Directory, None) {
        Ok(id) => id,
        Err(e) => {
            ctx.issue("internal_error", &path, e);
            return ctx.report;
        }
    };

    let mut state = ScanState::Completed;
    let mut frames: Vec<Frame> = Vec::new();
    if ctx.sink.cancel_requested() {
        state = ScanState::Cancelled;
    } else if root_dataless {
        ctx.issue("cloud_materialization_avoided", &path, "the selected root is dataless; listing it could download it".into());
        ctx.skip(Some(root_entry), &path, SkipReason::CloudBoundary);
    } else if let Some(pending) = ctx.list(lister, &root_fd, root_entry, &path) {
        frames.push(Frame { path: path.clone(), name: Vec::new(), dev: root_dev, ino: root_meta.ino, fd: Some(root_fd), pending });
    }
    if let Err(msg) = ctx.maybe_flush() {
        ctx.issue("store_error", &path, msg);
        state = ScanState::Failed;
        frames.clear();
    }

    let recheck_every = recheck_every.max(1);
    let mut since_check = 0usize;
    while state == ScanState::Completed {
        let Some(last) = frames.len().checked_sub(1) else { break };
        let Some(child) = frames[last].pending.pop() else {
            frames.pop();
            continue;
        };
        if ctx.sink.cancel_requested() {
            state = ScanState::Cancelled;
            break;
        }
        let child_path = join(&frames[last].path, &child.name);
        since_check += 1;
        if since_check >= recheck_every {
            since_check = 0;
            if let Some(change) = watch.changed() {
                ctx.issue(change.code, &path, change.message);
                ctx.skip(Some(child.entry_id), &child_path, change.reason);
                state = ScanState::Partial;
                break;
            }
        }
        // Any failure below re-checks the root before the next directory is entered.
        if let Err(fail) = ensure_top_fd(&mut frames, root_dev) {
            since_check = recheck_every;
            // The parent can no longer be reached safely: none of its remaining children are read.
            let parent_path = frames[last].path.clone();
            let rest: Vec<Pending> = std::iter::once(child).chain(frames[last].pending.drain(..).rev()).collect();
            let (reason, code, message) = fail_info(fail);
            ctx.issue(code, &parent_path, format!("cannot re-open the directory to continue: {message}"));
            for c in rest {
                ctx.skip(Some(c.entry_id), &join(&parent_path, &c.name), reason);
            }
            frames.pop();
            continue;
        }
        let opened = match &frames[last].fd {
            Some(parent) if gate.guards(&child_path) => open_gated(gate, parent, &child.name, &child_path, child.dev, child.ino, root_dev),
            Some(parent) => open_verified(parent, &child.name, child.dev, child.ino, root_dev),
            None => Err(OpenFail::Errno(libc::EBADF)),
        };
        let fd = match opened {
            Ok(fd) => fd,
            Err(fail) => {
                since_check = recheck_every;
                ctx.open_failed(child.entry_id, &child_path, fail);
                continue;
            }
        };
        let pending = ctx.list(lister, &fd, child.entry_id, &child_path);
        if pending.is_none() {
            since_check = recheck_every;
        }
        if let Err(msg) = ctx.maybe_flush() {
            ctx.issue("store_error", &child_path, msg);
            state = ScanState::Failed;
            break;
        }
        if let Some(pending) = pending.filter(|p| !p.is_empty()) {
            frames.push(Frame { path: child_path, name: child.name, dev: child.dev, ino: child.ino, fd: Some(fd), pending });
            release_excess(&mut frames);
        }
    }
    drop(frames);
    ctx.gate_summary(&path, gate.wait);
    // Completed is claimed only for the root the scan started on (SCN-07).
    if state == ScanState::Completed
        && let Some(change) = watch.changed()
    {
        ctx.issue(change.code, &path, change.message);
        state = ScanState::Partial;
    }
    if let Err(msg) = ctx.flush() {
        ctx.report.issues.push(ScanIssue { code: "store_error".into(), scope_display: String::new(), message: msg });
        state = ScanState::Failed;
    }
    drop(policy);
    ctx.report.state = state;
    ctx.report
}
