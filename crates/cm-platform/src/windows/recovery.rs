//! Windows restore and purge of quarantined objects (MVP 10.2). Declared as a child of
//! `actions` (via `#[path]`) so it reuses the verified-handle helpers without widening them.
//!
//! Restore:
//! - The volume at the quarantine location and at the original parent must be the recorded one
//!   (`volume_offline` / `volume_changed` otherwise: a manifest is never applied to whatever
//!   volume now answers at the same drive letter).
//! - The quarantined object is opened with `FILE_FLAG_OPEN_REPARSE_POINT`, `DELETE` and without
//!   `FILE_SHARE_DELETE`; its volume/file ID, reparse state, type and link count are checked on
//!   that handle.
//! - The original parent is opened and pinned (no `FILE_SHARE_DELETE`) and must carry the
//!   recorded identity. The rename is issued on the object's handle *relative to the pinned
//!   parent handle* (`RootDirectory`), without replace: an occupied name is `destination_exists`
//!   and nothing is overwritten or renamed to another name.
//! - Success is confirmed from the handles' final paths, not from a path lookup.
//!
//! Purge: the same source verification plus an unchanged logical size, then a delete
//! disposition on that handle. Only the verified object is removed; empty quarantine folders
//! are left in place.
//!
//! Metadata: a same-volume rename keeps the NTFS file record, so timestamps, attributes, the
//! security descriptor and alternate data streams travel with the object (see the round-trip
//! test). Limits: the change time (`ChangeTime`) is updated by each move, and permissions
//! inheritable from the quarantine folder are not applied to the object.

use cm_core::execute::Reconciled;
use cm_core::recovery::{MetadataCoverage, QuarantineState, QuarantinedObject, RecoveryActions, RecoveryKind, RecoveryOutcome};

use super::*;

/// Windows implementation of [`RecoveryActions`]. Stateless; every call re-verifies.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsRecovery;

fn convert(o: ActionOutcome) -> RecoveryOutcome {
    match o {
        ActionOutcome::Succeeded { .. } => RecoveryOutcome::Done,
        ActionOutcome::Skipped { code, message } => RecoveryOutcome::Skipped { code, message },
        ActionOutcome::Failed { code, message } => RecoveryOutcome::Failed { code, message },
        ActionOutcome::Unknown { message } => RecoveryOutcome::Unknown { message },
    }
}

fn skip(code: &str, message: impl Into<String>) -> RecoveryOutcome {
    RecoveryOutcome::Skipped { code: code.into(), message: message.into() }
}

fn fail(code: &str, message: impl Into<String>) -> RecoveryOutcome {
    RecoveryOutcome::Failed { code: code.into(), message: message.into() }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Volume {
    Recorded,
    Offline,
    Changed,
}

/// Whether the volume answering at the root of `path` is the recorded one.
fn volume_at(path: &[u16], volume_id: &str) -> Volume {
    let root = &path[..root_len(path)];
    match open_attrs(root).and_then(|h| live(&h)) {
        Err(_) => Volume::Offline,
        Ok(l) if same_volume(&l.volume_id, volume_id) => Volume::Recorded,
        Ok(_) => Volume::Changed,
    }
}

fn volume_stop(v: Volume, what: &str) -> Option<RecoveryOutcome> {
    match v {
        Volume::Recorded => None,
        Volume::Offline => Some(skip("volume_offline", format!("the volume of the {what} is not available; the manifest is kept"))),
        Volume::Changed => Some(skip("volume_changed", format!("a different volume is mounted at the {what}; nothing is restored onto it"))),
    }
}

fn resolve(p: &RawPath) -> Result<Vec<u16>, RecoveryOutcome> {
    let u = units(p).ok_or_else(|| fail("unsupported", "not a Windows path"))?;
    full_path(u).map(|(_, v)| v).map_err(|e| fail("check_failed", format!("cannot resolve path: {e}")))
}

/// The verified quarantined object, opened with `access`/`share`.
struct Source {
    handle: Owned,
    live: Live,
    verbatim: Vec<u16>,
}

fn open_source(o: &QuarantinedObject<'_>, access: u32) -> Result<Source, RecoveryOutcome> {
    let v = resolve(o.location)?;
    if let Some(stop) = volume_stop(volume_at(&v, &o.identity.volume_id), "quarantine location") {
        return Err(stop);
    }
    let check = |l: &Live| -> Result<(), Stop> {
        if l.reparse() {
            return Err(Stop::Reparse);
        }
        if !l.is(&o.identity.volume_id, &o.identity.file_id) {
            return Err(Stop::Identity);
        }
        if l.directory {
            return Err(Stop::NotFile);
        }
        if l.links > 1 {
            return Err(Stop::HardLinks(l.links));
        }
        Ok(())
    };
    let handle = match open(&v, access, SHARE_PIN, 0) {
        Ok(h) => h,
        Err(e) => {
            // Classify a refused open (a swapped-in directory, say) before reporting a denial.
            if e.raw_os_error() == Some(ERROR_ACCESS_DENIED)
                && let Ok(l) = open_attrs(&v).and_then(|h| live(&h))
                && let Err(stop) = check(&l)
            {
                return Err(convert(stop.outcome()));
            }
            return Err(convert(Stop::from_open(&e).outcome()));
        }
    };
    let l = live(&handle).map_err(|e| fail("check_failed", format!("cannot query quarantined object: {e}")))?;
    check(&l).map_err(|stop| convert(stop.outcome()))?;
    Ok(Source { handle, live: l, verbatim: v })
}

#[repr(C)]
struct IoStatusBlock {
    status: isize,
    information: usize,
}

// The Win32 wrapper turns every rename target into an absolute NT path, which cannot be
// combined with `RootDirectory`; the native call keeps the target relative to the pinned folder.
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtSetInformationFile(file: HANDLE, iosb: *mut IoStatusBlock, info: *const core::ffi::c_void, len: u32, class: i32) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}

/// FILE_INFORMATION_CLASS values (ntifs.h).
const NT_FILE_RENAME_INFORMATION: i32 = 10;
const NT_FILE_RENAME_INFORMATION_EX: i32 = 65;

/// Rename the handle's file to `name` inside the directory `dir` (by handle), never replacing.
fn rename_into(h: &Owned, dir: &Owned, name: &[u16]) -> io::Result<()> {
    let name_bytes = name.len() * 2;
    // FILE_RENAME_INFORMATION has the same layout as the Win32 FILE_RENAME_INFO.
    let size = std::mem::offset_of!(FILE_RENAME_INFO, FileName) + name_bytes + 2;
    let mut buf = vec![0u64; size.div_ceil(8)];
    let info = buf.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: `buf` is 8-byte aligned, zeroed and large enough for the header plus the name and NUL.
    unsafe {
        (*info).Anonymous.Flags = 0; // no FILE_RENAME_REPLACE_IF_EXISTS / ReplaceIfExists = FALSE
        (*info).RootDirectory = dir.0;
        (*info).FileNameLength = name_bytes as u32;
        std::ptr::copy_nonoverlapping(name.as_ptr(), std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(), name.len());
    }
    for class in [NT_FILE_RENAME_INFORMATION_EX, NT_FILE_RENAME_INFORMATION] {
        let mut iosb = IoStatusBlock { status: 0, information: 0 };
        // SAFETY: valid handles; `info` points to `size` initialized bytes of the declared class.
        let status = unsafe { NtSetInformationFile(h.0, &mut iosb, info.cast_const().cast(), size as u32, class) };
        if status >= 0 {
            return Ok(());
        }
        // SAFETY: pure status translation.
        let e = io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32);
        if class == NT_FILE_RENAME_INFORMATION || !unsupported_class(&e) {
            return Err(e);
        }
    }
    unreachable!("loop returns on the legacy class")
}

fn not_found(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND))
}

/// Original parent (pinned) and the original name, after volume and identity checks.
fn original_parent(o: &QuarantinedObject<'_>) -> Result<(Owned, Live, Vec<u16>, Vec<u16>), RecoveryOutcome> {
    let ov = resolve(o.original_path)?;
    let (parent, name) = split_parent(&ov).ok_or_else(|| fail("invalid_target", "the original path has no parent"))?;
    if !valid_component(&String::from_utf16_lossy(&name)) {
        return Err(fail("invalid_target", "the original name is not a single path component"));
    }
    if let Some(stop) = volume_stop(volume_at(&parent, &o.original_parent.volume_id), "original location") {
        return Err(stop);
    }
    let pin = match open(&parent, FILE_READ_ATTRIBUTES | SYNCHRONIZE, SHARE_PIN, FLAG_BACKUP_SEMANTICS) {
        Ok(h) => h,
        Err(e) if not_found(&e) => return Err(skip("parent_missing", "the original folder no longer exists; restoring elsewhere needs a new plan")),
        Err(e) if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION) => return Err(skip("in_use", "the original folder is being renamed or deleted")),
        Err(e) => return Err(fail("check_failed", format!("cannot open the original folder: {e}"))),
    };
    let l = live(&pin).map_err(|e| fail("check_failed", format!("cannot query the original folder: {e}")))?;
    if l.reparse() || !l.directory || !l.is(&o.original_parent.volume_id, &o.original_parent.file_id) {
        return Err(skip("parent_changed", "the original folder was replaced or moved; restoring elsewhere needs a new plan"));
    }
    Ok((pin, l, ov, name))
}

/// What occupies a path now, if anything (no link is followed, no content is read).
fn occupied(path: &[u16]) -> io::Result<bool> {
    match open_attrs(path) {
        Ok(_) => Ok(true),
        Err(e) if not_found(&e) => Ok(false),
        Err(e) if matches!(e.raw_os_error(), Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION)) => Ok(true),
        Err(e) => Err(e),
    }
}

fn same_text(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.chars().zip(b.chars()).all(|(x, y)| x.to_lowercase().eq(y.to_lowercase()))
}

impl RecoveryActions for WindowsRecovery {
    fn probe(&self, o: &QuarantinedObject<'_>) -> QuarantineState {
        let Ok(v) = resolve(o.location) else { return QuarantineState::Unknown };
        match volume_at(&v, &o.identity.volume_id) {
            Volume::Offline => return QuarantineState::VolumeOffline,
            Volume::Changed => return QuarantineState::VolumeChanged,
            Volume::Recorded => {}
        }
        match open_attrs(&v).and_then(|h| live(&h)) {
            Ok(l) if l.reparse() || l.directory || !l.is(&o.identity.volume_id, &o.identity.file_id) => return QuarantineState::IdentityChanged,
            Ok(_) => {}
            Err(e) if not_found(&e) => return QuarantineState::Missing,
            Err(_) => return QuarantineState::Unknown,
        }
        let Ok(ov) = resolve(o.original_path) else { return QuarantineState::Unknown };
        let Some((parent, _)) = split_parent(&ov) else { return QuarantineState::Unknown };
        match volume_at(&parent, &o.original_parent.volume_id) {
            Volume::Offline => return QuarantineState::VolumeOffline,
            Volume::Changed => return QuarantineState::VolumeChanged,
            Volume::Recorded => {}
        }
        match open_attrs(&parent).and_then(|h| live(&h)) {
            Ok(l) if l.reparse() || !l.directory || !l.is(&o.original_parent.volume_id, &o.original_parent.file_id) => return QuarantineState::ParentChanged,
            Ok(_) => {}
            Err(e) if not_found(&e) => return QuarantineState::ParentChanged,
            Err(_) => return QuarantineState::Unknown,
        }
        match occupied(&ov) {
            Ok(true) => QuarantineState::TargetConflict,
            Ok(false) => QuarantineState::Recoverable,
            Err(_) => QuarantineState::Unknown,
        }
    }

    fn restore(&self, o: &QuarantinedObject<'_>) -> RecoveryOutcome {
        if let Err(out) = privileges_dropped() {
            return convert(out);
        }
        let src = match open_source(o, DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE) {
            Ok(s) => s,
            Err(out) => return out,
        };
        let (pin, parent_live, original, name) = match original_parent(o) {
            Ok(p) => p,
            Err(out) => return out,
        };
        if !same_volume(&parent_live.volume_id, &src.live.volume_id) {
            return skip("volume_changed", "the original folder is on another volume; refusing copy-and-delete");
        }
        // Early, readable refusal; the no-replace rename below is what guarantees it.
        match occupied(&original) {
            Ok(true) => return skip("destination_exists", "something now exists at the original name; both are kept"),
            Ok(false) => {}
            Err(e) => return fail("check_failed", format!("cannot check the original name: {e}")),
        }
        if let Err(e) = rename_into(&src.handle, &pin, &name) {
            return match e.raw_os_error() {
                Some(ERROR_FILE_EXISTS | ERROR_ALREADY_EXISTS) => skip("destination_exists", "something now exists at the original name; both are kept"),
                Some(ERROR_SHARING_VIOLATION) => skip("in_use", format!("rename refused: {e}")),
                Some(ERROR_NOT_SAME_DEVICE) => skip("volume_changed", format!("rename crosses volumes: {e}")),
                Some(ERROR_ACCESS_DENIED) => fail("access_denied", format!("rename refused: {e}")),
                _ => fail("rename_failed", format!("rename refused: {e}")),
            };
        }
        // Confirm through the handles: the object's final path is the pinned folder plus its name.
        let expected = final_path(&pin).map(|p| format!("{}\\{}", p.trim_end_matches('\\'), String::from_utf16_lossy(&name)));
        match (final_path(&src.handle), expected) {
            (Some(now), Some(want)) if same_text(&now, &want) => RecoveryOutcome::Done,
            (Some(now), Some(want)) => RecoveryOutcome::Unknown { message: format!("renamed, but the object is at {now} instead of {want}") },
            _ => RecoveryOutcome::Unknown { message: "renamed, but the final location cannot be confirmed".into() },
        }
    }

    fn purge(&self, o: &QuarantinedObject<'_>) -> RecoveryOutcome {
        if let Err(out) = privileges_dropped() {
            return convert(out);
        }
        let src = match open_source(o, DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE) {
            Ok(s) => s,
            Err(out) => return out,
        };
        // Changed content is never purged silently (REC-03).
        if src.live.size != o.logical_bytes {
            return skip("metadata_changed", format!("logical size changed in quarantine: recorded {}, now {}", o.logical_bytes, src.live.size));
        }
        if let Err(e) = dispose(&src.handle) {
            let code = match e.raw_os_error() {
                Some(ERROR_ACCESS_DENIED) => "access_denied",
                Some(ERROR_SHARING_VIOLATION) => "in_use",
                _ => "delete_failed",
            };
            return fail(code, format!("delete refused: {e}"));
        }
        let path = src.verbatim.clone();
        drop(src); // closing the handle commits the delete
        match at_path(&path, &o.identity.volume_id, &o.identity.file_id) {
            AtPath::Missing | AtPath::Other => RecoveryOutcome::Done,
            AtPath::Planned => RecoveryOutcome::Unknown { message: "object is still present after delete (other handles may keep it pending)".into() },
            AtPath::Error(e) => RecoveryOutcome::Unknown { message: format!("delete pending or unconfirmed: {e}") },
        }
    }

    fn reconcile(&self, o: &QuarantinedObject<'_>, kind: RecoveryKind) -> Reconciled {
        let (Some(q), Some(orig)) = (units(o.location), units(o.original_path)) else { return Reconciled::StillUnknown };
        let (vol, fid) = (&o.identity.volume_id, &o.identity.file_id);
        let in_quarantine = at_path(q, vol, fid);
        let at_original = at_path(orig, vol, fid);
        match (kind, in_quarantine, at_original) {
            (_, AtPath::Planned, _) => Reconciled::NotPerformed,
            (RecoveryKind::Restore, _, AtPath::Planned) => Reconciled::Performed,
            (RecoveryKind::Purge, AtPath::Missing, AtPath::Missing | AtPath::Other) => {
                // Gone from quarantine on the recorded, mounted volume and not restored elsewhere.
                match resolve(o.location) {
                    Ok(v) if volume_at(&v, vol) == Volume::Recorded => Reconciled::Performed,
                    _ => Reconciled::StillUnknown,
                }
            }
            _ => Reconciled::StillUnknown,
        }
    }

    fn free_space(&self, path: &RawPath) -> Option<u64> {
        WindowsActions::DEFAULT.free_space(path)
    }

    fn metadata_coverage(&self) -> MetadataCoverage {
        MetadataCoverage {
            preserved: vec!["timestamps", "attributes", "acl", "alternate_streams"],
            limits: vec![
                "same-volume rename only: the file record moves, nothing is copied",
                "the NTFS change time is updated by each move; creation, write and access times are kept",
                "permissions inheritable from the quarantine folder are not applied; the object keeps its own security descriptor",
                "extended attributes travel with the file record but are not covered by tests",
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use cm_core::plan::WireIdentity;

    use super::*;

    const R: WindowsRecovery = WindowsRecovery;

    fn scratch(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("cm-recovery-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn raw(p: &Path) -> RawPath {
        RawPath::windows(wide(p)).unwrap()
    }

    fn ident(p: &Path) -> WireIdentity {
        let (volume_id, file_id) = crate::windows::directory::identity_of(&wide(p)).unwrap();
        WireIdentity { volume_id, file_id, generation: None, quality: "strong".into(), observed_at: "2026-10-15T00:00:00Z".into() }
    }

    /// A quarantined file: moved (same volume, same file ID) from `dir/name` into `root/q/name`.
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
            QuarantinedObject { action_id: "r0", location: &self.location, identity: &self.identity, original_path: &self.original, original_parent: &self.parent, logical_bytes: self.size }
        }
    }

    fn quarantined(root: &Path, name: &str, body: &str, prepare: impl FnOnce(&Path)) -> Q {
        let dir = root.join("orig");
        std::fs::create_dir_all(&dir).unwrap();
        let orig = dir.join(name);
        std::fs::write(&orig, body).unwrap();
        prepare(&orig);
        let identity = ident(&orig);
        let q = root.join("q");
        std::fs::create_dir_all(&q).unwrap();
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

    fn icacls(path: &Path, args: &[&str]) -> String {
        let out = Command::new("icacls").arg(path).args(args).output().unwrap();
        assert!(out.status.success(), "icacls {args:?}: {}", String::from_utf8_lossy(&out.stdout));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn restore_round_trip_keeps_identity_and_metadata() {
        let root = scratch("roundtrip");
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(40 * 86_400);
        let q = quarantined(&root, "keep.cache", "payload", |p| {
            std::fs::write(format!("{}:cm_stream", p.display()), "side").unwrap();
            icacls(p, &["/grant", "*S-1-5-32-545:(R)"]);
            let f = std::fs::File::options().write(true).open(p).unwrap();
            f.set_modified(past).unwrap();
            drop(f);
            assert!(Command::new("attrib").arg("+h").arg(p).status().unwrap().success());
        });
        let before = std::fs::symlink_metadata(&q.loc).unwrap();
        assert_eq!(R.probe(&q.object()), QuarantineState::Recoverable);
        assert_eq!(R.reconcile(&q.object(), RecoveryKind::Restore), Reconciled::NotPerformed);
        assert_eq!(R.restore(&q.object()), RecoveryOutcome::Done);
        assert!(!q.loc.exists());
        assert_eq!(std::fs::read_to_string(&q.orig).unwrap(), "payload");
        assert_eq!(ident(&q.orig).file_id, q.identity.file_id, "the same object is back");
        let after = std::fs::symlink_metadata(&q.orig).unwrap();
        assert_eq!((after.creation_time(), after.last_write_time()), (before.creation_time(), before.last_write_time()), "timestamps");
        assert_ne!(after.file_attributes() & 0x2, 0, "hidden attribute");
        assert_eq!(std::fs::read_to_string(format!("{}:cm_stream", q.orig.display())).unwrap(), "side", "alternate data stream");
        assert!(icacls(&q.orig, &[]).contains("S-1-5-32-545:(R)") || icacls(&q.orig, &[]).contains("BUILTIN\\Users:(R)"), "explicit ACE");
        assert_eq!(R.reconcile(&q.object(), RecoveryKind::Restore), Reconciled::Performed);
        assert_eq!(R.probe(&q.object()), QuarantineState::Missing);
        assert_eq!(code(&R.restore(&q.object())), "missing");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn occupied_original_name_is_never_overwritten() {
        let root = scratch("occupied");
        let q = quarantined(&root, "a.tmp", "quarantined", |_| {});
        std::fs::write(&q.orig, "newer").unwrap();
        assert_eq!(R.probe(&q.object()), QuarantineState::TargetConflict);
        assert_eq!(code(&R.restore(&q.object())), "destination_exists");
        assert_eq!(std::fs::read_to_string(&q.orig).unwrap(), "newer");
        assert_eq!(std::fs::read_to_string(&q.loc).unwrap(), "quarantined");
        // A directory squatting on the name is refused the same way.
        std::fs::remove_file(&q.orig).unwrap();
        std::fs::create_dir(&q.orig).unwrap();
        assert_eq!(code(&R.restore(&q.object())), "destination_exists");
        assert!(q.orig.is_dir() && q.loc.is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn changed_source_or_parent_is_refused() {
        let root = scratch("changed");
        let q = quarantined(&root, "a.tmp", "planned", |_| {});
        std::fs::rename(&q.loc, root.join("moved")).unwrap();
        std::fs::write(&q.loc, "planted").unwrap();
        assert_eq!(R.probe(&q.object()), QuarantineState::IdentityChanged);
        assert_eq!(code(&R.restore(&q.object())), "identity_changed");
        assert_eq!(code(&R.purge(&q.object())), "identity_changed");
        assert_eq!(std::fs::read_to_string(&q.loc).unwrap(), "planted");
        assert!(!q.orig.exists());
        assert_eq!(R.reconcile(&q.object(), RecoveryKind::Purge), Reconciled::StillUnknown);
        std::fs::remove_file(&q.loc).unwrap();
        std::os::windows::fs::symlink_file(root.join("moved"), &q.loc).unwrap();
        assert_eq!(code(&R.restore(&q.object())), "link_or_reparse");
        std::fs::remove_file(&q.loc).unwrap();
        std::fs::rename(root.join("moved"), &q.loc).unwrap();

        // The original folder replaced by a new one of the same name.
        let dir = q.orig.parent().unwrap();
        std::fs::rename(dir, root.join("orig-old")).unwrap();
        std::fs::create_dir(dir).unwrap();
        assert_eq!(R.probe(&q.object()), QuarantineState::ParentChanged);
        assert_eq!(code(&R.restore(&q.object())), "parent_changed");
        std::fs::remove_dir(dir).unwrap();
        assert_eq!(code(&R.restore(&q.object())), "parent_missing");
        assert!(q.loc.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn offline_or_replaced_volumes_keep_the_manifest() {
        let root = scratch("volume");
        let q = quarantined(&root, "a.tmp", "x", |_| {});
        let letter = ('M'..='Z').rev().find(|c| !Path::new(&format!("{c}:\\")).exists()).unwrap();
        let away = raw(Path::new(&format!("{letter}:\\q\\a.tmp")));
        let offline = QuarantinedObject { location: &away, ..q.object() };
        assert_eq!(R.probe(&offline), QuarantineState::VolumeOffline);
        assert_eq!(code(&R.restore(&offline)), "volume_offline");
        assert_eq!(code(&R.purge(&offline)), "volume_offline");
        assert_eq!(R.reconcile(&offline, RecoveryKind::Purge), Reconciled::StillUnknown);
        // Same path, but the recorded volume serial differs: another volume now answers there.
        let (guid, _) = q.identity.volume_id.rsplit_once('|').unwrap();
        let other = WireIdentity { volume_id: format!("{guid}|00000000deadbeef"), ..q.identity.clone() };
        let swapped = QuarantinedObject { identity: &other, ..q.object() };
        assert_eq!(R.probe(&swapped), QuarantineState::VolumeChanged);
        assert_eq!(code(&R.restore(&swapped)), "volume_changed");
        let other_parent = WireIdentity { volume_id: format!("{guid}|00000000deadbeef"), ..q.parent.clone() };
        let moved_parent = QuarantinedObject { original_parent: &other_parent, ..q.object() };
        assert_eq!(code(&R.restore(&moved_parent)), "volume_changed");
        assert!(q.loc.exists() && !q.orig.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn purge_deletes_exactly_the_verified_object() {
        let root = scratch("purge");
        let q = quarantined(&root, "a.tmp", "gone", |_| {});
        let sibling = root.join("q").join("b.tmp");
        std::fs::write(&sibling, "kept").unwrap();
        let holder = std::fs::OpenOptions::new().read(true).share_mode(0).open(&q.loc).unwrap();
        assert_eq!(code(&R.purge(&q.object())), "in_use");
        assert_eq!(code(&R.restore(&q.object())), "in_use");
        drop(holder);
        std::fs::write(&q.loc, "grown!").unwrap();
        assert_eq!(code(&R.purge(&q.object())), "metadata_changed", "changed content is never purged silently");
        std::fs::write(&q.loc, "gone").unwrap();
        assert_eq!(R.reconcile(&q.object(), RecoveryKind::Purge), Reconciled::NotPerformed);
        assert_eq!(R.purge(&q.object()), RecoveryOutcome::Done);
        assert!(!q.loc.exists() && !q.orig.exists());
        assert_eq!(std::fs::read_to_string(&sibling).unwrap(), "kept");
        assert_eq!(R.reconcile(&q.object(), RecoveryKind::Purge), Reconciled::Performed);
        assert!(R.free_space(&q.location).is_some());
        let _ = std::fs::remove_dir_all(&root);
    }
}
