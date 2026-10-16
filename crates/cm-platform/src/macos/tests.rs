//! Real-file-system tests for `mac.posix` and `mac.bulk`. They run on macOS only; every case
//! runs against both backends, and the two backends are reconciled row by row.

use std::cell::Cell;
use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use cm_core::capability::CapabilityState;
use cm_core::model::{BoundaryKind, CloudState, IdentityQuality, ObjectKind, ScanState, SkipReason};
use cm_core::path::RawPath;
use cm_core::scan::{EntryRow, ObjectRow, ScanBackend, ScanOptions, ScanReport, ScanSink, SkippedRow};
use cm_fixtures::{Profile, create, plan, prepare_sentinel, verify, verify_sentinel};

use super::bulk::{BulkBackend, BulkLister, reclaimable_estimate};
use super::posix::{PosixBackend, PosixLister};
use super::sys::Fd;
use super::walk::{self, Lister, Listing};

#[derive(Default)]
struct MemSink {
    objects: Vec<ObjectRow>,
    entries: Vec<EntryRow>,
    skipped: Vec<SkippedRow>,
    writes: usize,
    cancel_after_writes: Option<usize>,
    /// `writes` when cancellation was first reported to the backend.
    cancel_seen_at: Option<usize>,
}

impl ScanSink for MemSink {
    fn write(&mut self, o: &[ObjectRow], e: &[EntryRow], s: &[SkippedRow]) -> Result<(), String> {
        self.objects.extend_from_slice(o);
        self.entries.extend_from_slice(e);
        self.skipped.extend_from_slice(s);
        self.writes += 1;
        Ok(())
    }
    fn cancel_requested(&mut self) -> bool {
        let cancel = self.cancel_after_writes.is_some_and(|n| self.writes >= n);
        if cancel && self.cancel_seen_at.is_none() {
            self.cancel_seen_at = Some(self.writes);
        }
        cancel
    }
}

impl MemSink {
    fn name_of(&self, e: &EntryRow) -> String {
        e.name.display()
    }
    fn by_name(&self, name: &str) -> &EntryRow {
        self.entries.iter().find(|e| self.name_of(e) == name).unwrap_or_else(|| panic!("no entry {name}"))
    }
    fn by_raw(&self, name: &[u8]) -> Option<&EntryRow> {
        self.entries.iter().find(|e| e.name.raw_bytes() == name)
    }
    fn object(&self, id: u64) -> &ObjectRow {
        self.objects.iter().find(|o| o.object_id == id).unwrap()
    }
    fn has(&self, name: &str) -> bool {
        self.entries.iter().any(|e| self.name_of(e) == name)
    }
}

fn backends() -> [&'static dyn ScanBackend; 2] {
    [&PosixBackend, &BulkBackend]
}

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-macscan-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn raw(p: &Path) -> RawPath {
    RawPath::unix(p.as_os_str().as_bytes().to_vec()).unwrap()
}

fn scan_with(backend: &dyn ScanBackend, root: &Path, sink: &mut MemSink) -> ScanReport {
    backend.scan(&raw(root), sink, &ScanOptions { batch_entries: 4, ..Default::default() })
}

fn is_root_user() -> bool {
    // SAFETY: plain getter.
    unsafe { libc::geteuid() == 0 }
}

#[test]
fn tiny_fixture_is_enumerated_exactly_and_read_only() {
    for backend in backends() {
        let base = scratch(&format!("tiny-{}", backend.name()));
        let (root, sentinel) = (base.join("root"), base.join("sentinel"));
        prepare_sentinel(&sentinel).unwrap();
        let manifest = create(Profile::Tiny, 3, &root, &sentinel).unwrap();
        let mut sink = MemSink::default();
        let report = scan_with(backend, &root, &mut sink);
        assert_eq!(report.state, ScanState::Completed, "{}: {:?}", backend.name(), report.issues);
        assert!(
            report.issues.iter().all(|i| i.code == "materialization_policy_unavailable"),
            "{}: {:?}",
            backend.name(),
            report.issues
        );
        assert!(report.backend_reason.contains("ordinary permissions") || is_root_user());

        let files: Vec<_> = sink.entries.iter().filter(|e| e.kind == ObjectKind::File).collect();
        // 5 files + 1 hard link name + the fixture's own root marker file
        assert_eq!(files.len() as u64, manifest.plan.counts.files + manifest.plan.counts.hardlinks + 1);
        let dirs = sink.entries.iter().filter(|e| e.kind == ObjectKind::Directory && e.parent_entry_id.is_some()).count() as u64;
        assert_eq!(dirs, manifest.plan.counts.directories);

        let original = sink.by_name("original.dat");
        let link = sink.by_name("hardlink-to-original.dat");
        assert_eq!(original.object_id, link.object_id, "hard links share one object");
        let obj = sink.object(original.object_id);
        assert_eq!(obj.logical, Some(2_048));
        assert_eq!(obj.link_count, Some(2));
        assert!(obj.allocated.is_some_and(|a| a >= 2_048), "allocation measured: {:?}", obj.allocated);
        assert!(obj.file_id.starts_with("inode64:") && obj.file_id.len() == "inode64:".len() + 16);
        assert!(obj.modified_unix.is_some());
        assert_eq!(sink.object(sink.by_name("zero.bin").object_id).logical, Some(0));
        let strong_fs = matches!(backend.probe(&raw(&root)).filesystem.as_deref(), Some("apfs" | "hfs"));
        if strong_fs {
            assert!(sink.objects.iter().all(|o| o.quality == IdentityQuality::Strong));
            assert!(sink.objects[0].volume_id.starts_with("uuid:"), "{}", sink.objects[0].volume_id);
        }

        let chinese = sink.by_name("中文 文件.txt");
        assert_eq!(chinese.name, RawPath::Unix("中文 文件.txt".as_bytes().to_vec()), "raw bytes kept exactly");
        assert!(sink.entries.iter().any(|e| sink.name_of(e).len() == 184), "180-char name kept exactly");

        for link in ["link-to-sentinel-file", "link-to-sentinel-dir"] {
            let e = sink.by_name(link);
            assert_eq!((e.kind, e.boundary), (ObjectKind::Symlink, Some(BoundaryKind::Symlink)));
        }
        assert!(!sink.has("inner.txt") && !sink.has("sentinel.txt"), "links not followed");
        assert_eq!(report.boundaries, 2);
        assert!(sink.objects.iter().all(|o| o.cloud == CloudState::NotCloud));

        verify(&root).expect("scan did not modify the fixture");
        verify_sentinel(&sentinel).expect("scan did not touch the sentinel");
        std::fs::remove_dir_all(&base).unwrap();
    }
}

/// Field-by-field reconciliation of the two backends on one fixture (task 5.2).
#[test]
fn bulk_and_posix_rows_reconcile_on_the_same_fixture() {
    let base = scratch("reconcile");
    let (root, sentinel) = (base.join("root"), base.join("sentinel"));
    prepare_sentinel(&sentinel).unwrap();
    create(Profile::Tiny, 5, &root, &sentinel).unwrap();
    let fifo = CString::new(root.join("pipe").as_os_str().as_bytes()).unwrap();
    // SAFETY: NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);

    // Every field both backends produce; `object_id`/`entry_id` are per-scan counters and
    // `shared_blocks` exists only in `mac.bulk` (posix.rs), so it is checked separately.
    let rows = |backend: &dyn ScanBackend| {
        let mut sink = MemSink::default();
        let report = scan_with(backend, &root, &mut sink);
        assert_eq!(report.state, ScanState::Completed, "{}: {:?}", backend.name(), report.issues);
        let mut v: Vec<_> = sink
            .entries
            .iter()
            .map(|e| {
                let o = sink.object(e.object_id);
                let path = path_of(&sink, e);
                let common = (
                    (path, e.kind, e.boundary, e.parent_entry_id.is_none()),
                    (o.file_id.clone(), o.volume_id.clone(), o.generation.clone(), o.quality, o.kind),
                    (o.logical, o.allocated, o.link_count, o.cloud, o.modified_unix),
                );
                (common, o.shared_blocks)
            })
            .collect();
        v.sort_by(|a, b| a.0.0.0.cmp(&b.0.0.0));
        v
    };
    let (posix, bulk) = (rows(&PosixBackend), rows(&BulkBackend));
    assert_eq!(posix.len(), bulk.len());
    for ((p, p_shared), (b, _)) in posix.iter().zip(&bulk) {
        assert_eq!(p, b);
        assert_eq!(*p_shared, None, "mac.posix never reports block sharing");
    }
    let apfs = BulkBackend.probe(&raw(&root)).filesystem.as_deref() == Some("apfs");
    for ((row, shared), _) in bulk.iter().zip(&posix) {
        let file = row.1.4 == ObjectKind::File;
        if !file {
            assert_eq!(*shared, None, "sharing is reported for files only: {:?}", row.0);
        } else if apfs {
            assert_eq!(*shared, Some(false), "fresh fixture files share no blocks: {:?}", row.0);
        }
    }
    let pipe = posix.iter().find(|r| r.0.0.0.ends_with(b"/pipe")).expect("fifo recorded");
    assert_eq!((pipe.0.0.1, pipe.0.2.0), (ObjectKind::Other, None), "a FIFO is recorded, never opened");
    std::fs::remove_dir_all(&base).unwrap();
}

fn path_of(sink: &MemSink, e: &EntryRow) -> Vec<u8> {
    let mut parts = vec![e.name.raw_bytes()];
    let mut parent = e.parent_entry_id;
    while let Some(p) = parent {
        let pe = sink.entries.iter().find(|x| x.entry_id == p).unwrap();
        parts.push(pe.name.raw_bytes());
        parent = pe.parent_entry_id;
    }
    parts.reverse();
    parts.join(&b'/')
}

#[test]
fn unreadable_subtree_is_skipped_not_empty() {
    if is_root_user() {
        eprintln!("skipped: root bypasses POSIX permissions");
        return;
    }
    for backend in backends() {
        let base = scratch(&format!("denied-{}", backend.name()));
        let locked = base.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join("inside.txt"), "x").unwrap();
        std::fs::write(base.join("visible.txt"), "yy").unwrap();
        let chmod = Command::new("chmod").arg("000").arg(&locked).status().unwrap();
        assert!(chmod.success());
        let mut sink = MemSink::default();
        let report = scan_with(backend, &base, &mut sink);
        let _ = Command::new("chmod").arg("755").arg(&locked).status();
        assert!(report.elevated_read.is_empty());
        assert_eq!(report.state, ScanState::Completed, "the store downgrades; the backend reached the end");
        assert_eq!(sink.skipped.len(), 1, "{}: {:?}", backend.name(), report.issues);
        assert_eq!(sink.skipped[0].reason, SkipReason::PermissionDenied);
        assert_eq!(sink.skipped[0].path.raw_bytes(), locked.as_os_str().as_bytes());
        assert!(report.issues.iter().any(|i| i.code == "permission_required"));
        assert!(sink.has("visible.txt"));
        assert!(sink.has("locked"), "the denied directory itself is still listed");
        assert!(!sink.has("inside.txt"));
        std::fs::remove_dir_all(&base).unwrap();
    }
}

#[test]
fn names_are_raw_bytes_never_normalized() {
    for backend in backends() {
        let base = scratch(&format!("names-{}", backend.name()));
        let nfd = "e\u{301}cole.txt".as_bytes().to_vec(); // decomposed é
        std::fs::write(base.join(OsStr::from_bytes(&nfd)), b"nfd").unwrap();
        let invalid = b"bad-\xff\xfe.bin".to_vec();
        let invalid_ok = match std::fs::write(base.join(OsStr::from_bytes(&invalid)), b"x") {
            Ok(()) => true,
            // APFS and HFS+ reject names that are not valid UTF-8.
            Err(e) if e.raw_os_error() == Some(libc::EILSEQ) => false,
            Err(e) => panic!("{e}"),
        };
        let mut sink = MemSink::default();
        let report = scan_with(backend, &base, &mut sink);
        assert_eq!(report.state, ScanState::Completed, "{:?}", report.issues);
        let listed = sink.by_raw(&nfd).expect("NFD name returned with its original bytes");
        assert_eq!(sink.object(listed.object_id).logical, Some(3));
        if invalid_ok {
            assert!(sink.by_raw(&invalid).is_some(), "invalid UTF-8 kept as raw bytes");
        } else {
            eprintln!("note: file system rejected invalid UTF-8 names; raw-byte path covered by parser tests");
        }
        std::fs::remove_dir_all(&base).unwrap();
    }
}

#[test]
fn directory_symlink_is_a_boundary_and_root_symlink_is_refused() {
    for backend in backends() {
        let base = scratch(&format!("links-{}", backend.name()));
        let (root, outside) = (base.join("root"), base.join("outside"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "out of scope").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("ln")).unwrap();
        let mut sink = MemSink::default();
        let report = scan_with(backend, &root, &mut sink);
        assert_eq!(report.state, ScanState::Completed);
        let ln = sink.by_name("ln");
        assert_eq!((ln.kind, ln.boundary), (ObjectKind::Symlink, Some(BoundaryKind::Symlink)));
        assert!(!sink.has("secret.txt"));

        let mut sink = MemSink::default();
        let report = scan_with(backend, &root.join("ln"), &mut sink);
        assert_eq!(report.state, ScanState::Failed, "the selected root itself is not followed");
        assert_eq!(report.issues[0].code, "root_unavailable");
        assert!(sink.entries.is_empty());
        std::fs::remove_dir_all(&base).unwrap();
    }
}

#[test]
fn cancellation_stops_scheduling_and_keeps_confirmed_rows() {
    for backend in backends() {
        let base = scratch(&format!("cancel-{}", backend.name()));
        let (root, sentinel) = (base.join("root"), base.join("sentinel"));
        prepare_sentinel(&sentinel).unwrap();
        create(Profile::Deep, 1, &root, &sentinel).unwrap();
        let mut sink = MemSink { cancel_after_writes: Some(1), ..Default::default() };
        let report = scan_with(backend, &root, &mut sink);
        assert_eq!(report.state, ScanState::Cancelled);
        assert!(!sink.entries.is_empty(), "confirmed rows are kept");
        assert!((sink.entries.len() as u64) < plan(Profile::Deep, 1).counts.files * 2, "work stopped early");
        // The host shows `stopping` from the first positive poll (cm-app LifecycleSink); after it
        // the backend only hands over the rows it already confirmed, in at most one final batch.
        let seen = sink.cancel_seen_at.expect("cancellation was polled");
        assert!(sink.writes <= seen + 1, "{}: {} writes after cancel was seen at {seen}", backend.name(), sink.writes);
        for e in &sink.entries {
            assert!(e.parent_entry_id.is_none_or(|p| sink.entries.iter().any(|x| x.entry_id == p)), "kept rows are a connected tree");
        }
        std::fs::remove_dir_all(&base).unwrap();
    }
}

#[test]
fn deep_tree_beyond_the_held_descriptor_budget_is_complete() {
    for backend in backends() {
        let base = scratch(&format!("deep-{}", backend.name()));
        let mut deep = base.clone();
        for i in 0..150 {
            deep = deep.join(format!("{i:03}"));
            std::fs::create_dir(&deep).unwrap();
            std::fs::write(deep.join("f"), [1u8; 3]).unwrap();
        }
        let mut sink = MemSink::default();
        let report = scan_with(backend, &base, &mut sink);
        assert_eq!(report.state, ScanState::Completed, "{:?}", report.issues);
        assert_eq!(sink.entries.iter().filter(|e| e.kind == ObjectKind::File).count(), 150);
        assert_eq!(sink.entries.iter().filter(|e| e.kind == ObjectKind::Directory).count(), 151);
        std::fs::remove_dir_all(&base).unwrap();
    }
}

#[test]
fn missing_root_fails_with_a_typed_issue_not_an_empty_scan() {
    for backend in backends() {
        let base = scratch(&format!("missing-{}", backend.name()));
        let mut sink = MemSink::default();
        let report = scan_with(backend, &base.join("does-not-exist"), &mut sink);
        assert_eq!(report.state, ScanState::Failed);
        assert_eq!(report.issues[0].code, "root_unavailable");
        assert!(sink.entries.is_empty());
        let report = backend.scan(&RawPath::windows(vec![b'C' as u16]).unwrap(), &mut sink, &ScanOptions::default());
        assert_eq!((report.state, report.issues[0].code.as_str()), (ScanState::Failed, "unsupported_path"));
        std::fs::remove_dir_all(&base).unwrap();
    }
}

#[test]
fn probe_reports_filesystem_and_stays_unverified() {
    for backend in backends() {
        let base = scratch(&format!("probe-{}", backend.name()));
        let p = backend.probe(&raw(&base));
        assert!(p.filesystem.is_some());
        assert!(p.volume_id.is_some());
        assert_eq!(p.backend, backend.name());
        assert_eq!(p.state, CapabilityState::Unverified);
        assert!(p.reason.contains("selected root"));
        std::fs::remove_dir_all(&base).unwrap();
    }
}

#[test]
fn mounted_volume_inside_the_scope_is_a_boundary() {
    if std::env::var_os("CM_ALLOW_DMG_TESTS").is_none() {
        eprintln!("skipped: set CM_ALLOW_DMG_TESTS=1 to attach a disposable disk image");
        return;
    }
    let base = scratch("mount");
    let root = base.join("root");
    let mnt = root.join("mnt");
    std::fs::create_dir_all(&mnt).unwrap();
    std::fs::write(root.join("top.txt"), b"a").unwrap();
    let image = base.join("vol.dmg");
    let ok = Command::new("hdiutil")
        .args(["create", "-quiet", "-size", "4m", "-fs", "HFS+", "-volname", "cmtest", "-o"])
        .arg(&image)
        .status()
        .unwrap();
    assert!(ok.success());
    let ok = Command::new("hdiutil").args(["attach", "-quiet", "-nobrowse", "-mountpoint"]).arg(&mnt).arg(&image).status().unwrap();
    assert!(ok.success());
    std::fs::write(mnt.join("other-volume.txt"), b"b").unwrap();
    for backend in backends() {
        let mut sink = MemSink::default();
        let report = scan_with(backend, &root, &mut sink);
        assert_eq!(report.state, ScanState::Completed, "{}: {:?}", backend.name(), report.issues);
        let m = sink.by_name("mnt");
        assert_eq!(m.boundary, Some(BoundaryKind::MountPoint), "{}", backend.name());
        assert!(!sink.has("other-volume.txt"), "nothing is read from the mounted volume");
        assert!(sink.has("top.txt"));
    }
    let _ = Command::new("hdiutil").args(["detach", "-quiet", "-force"]).arg(&mnt).status();
    std::fs::remove_dir_all(&base).unwrap();
}

/// A fresh lister of the named backend (the walk is driven directly to inject faults).
fn lister_for(backend: &str) -> Box<dyn Lister> {
    match backend {
        "mac.posix" => Box::new(PosixLister),
        _ => Box::new(BulkLister::new()),
    }
}

const NAMES: [&str; 2] = ["mac.posix", "mac.bulk"];

/// Wraps a backend's lister; `hook(call_no, dir, inner)` produces each listing.
struct Hooked<F: FnMut(usize, &Fd, &mut dyn Lister) -> Listing> {
    inner: Box<dyn Lister>,
    calls: usize,
    hook: F,
}

impl<F: FnMut(usize, &Fd, &mut dyn Lister) -> Listing> Lister for Hooked<F> {
    fn list(&mut self, dir: &Fd) -> Listing {
        self.calls += 1;
        (self.hook)(self.calls, dir, self.inner.as_mut())
    }
}

fn walk_hooked(backend: &'static str, root: &Path, sink: &mut MemSink, recheck_every: usize, hook: impl FnMut(usize, &Fd, &mut dyn Lister) -> Listing) -> (ScanReport, usize) {
    let mut lister = Hooked { inner: lister_for(backend), calls: 0, hook };
    let options = ScanOptions { batch_entries: 4, ..Default::default() };
    let report = walk::scan_checked(backend, "test walk", &raw(root), sink, &options, &mut lister, recheck_every);
    (report, lister.calls)
}

fn shared_of(sink: &MemSink, name: &str) -> Option<bool> {
    sink.object(sink.by_name(name).object_id).shared_blocks
}

/// SCN-06 / TC-MAC-005 (hosted-runner part): a clone pair is two entities whose blocks are
/// shared; a plain file is not; the estimate stays null with `shared_blocks_unknown`.
#[test]
fn apfs_clones_report_shared_blocks_and_plain_files_do_not() {
    let base = scratch("clone");
    if BulkBackend.probe(&raw(&base)).filesystem.as_deref() != Some("apfs") {
        eprintln!("skipped: clonefile needs APFS");
        std::fs::remove_dir_all(&base).unwrap();
        return;
    }
    let data: Vec<u8> = (0..256 * 1024u32).map(|i| (i % 251) as u8).collect();
    let original = base.join("original.bin");
    std::fs::write(&original, &data).unwrap();
    std::fs::write(base.join("plain.bin"), &data).unwrap();
    let (src, dst) = (CString::new(original.as_os_str().as_bytes()).unwrap(), CString::new(base.join("clone.bin").as_os_str().as_bytes()).unwrap());
    // SAFETY: NUL-terminated paths; flags 0.
    assert_eq!(unsafe { libc::clonefile(src.as_ptr(), dst.as_ptr(), 0) }, 0, "{}", std::io::Error::last_os_error());

    let mut sink = MemSink::default();
    let report = scan_with(&BulkBackend, &base, &mut sink);
    assert_eq!(report.state, ScanState::Completed, "{:?}", report.issues);
    for name in ["original.bin", "clone.bin", "plain.bin"] {
        let o = sink.object(sink.by_name(name).object_id);
        // Raw observation, kept in the CI log.
        eprintln!("raw {name}: logical={:?} allocated={:?} shared_blocks={:?} file_id={}", o.logical, o.allocated, o.shared_blocks, o.file_id);
        assert_eq!(o.logical, Some(data.len() as u64));
        assert_eq!(reclaimable_estimate(o), (None, "shared_blocks_unknown"), "{name}: exclusive blocks are never estimated");
    }
    assert_eq!(shared_of(&sink, "original.bin"), Some(true), "the clone source shares its blocks");
    assert_eq!(shared_of(&sink, "clone.bin"), Some(true), "the clone shares its blocks");
    assert_eq!(shared_of(&sink, "plain.bin"), Some(false), "a plain file was established as not shared");
    assert_ne!(sink.by_name("original.bin").object_id, sink.by_name("clone.bin").object_id, "a clone is a separate entity, not a hard link");

    let mut sink = MemSink::default();
    scan_with(&PosixBackend, &base, &mut sink);
    assert!(sink.objects.iter().all(|o| o.shared_blocks.is_none()), "mac.posix does not observe sharing");
    std::fs::remove_dir_all(&base).unwrap();
}

/// SCN-05 / MVP 5.4: a listing refused with `EDEADLK` (materialization avoided) is a cloud
/// boundary skip, recorded once and never retried.
#[test]
fn refused_materialization_is_a_cloud_skip_and_is_not_retried() {
    for backend in NAMES {
        let base = scratch(&format!("edeadlk-{backend}"));
        std::fs::create_dir_all(base.join("provider/sub")).unwrap();
        std::fs::write(base.join("provider/remote.txt"), b"r").unwrap();
        std::fs::write(base.join("local.txt"), b"l").unwrap();
        let provider = std::fs::metadata(base.join("provider")).unwrap().ino();
        let refused = Cell::new(0);
        let mut sink = MemSink::default();
        let (report, _) = walk_hooked(backend, &base, &mut sink, walk::RECHECK_EVERY_DIRS, |_, dir, inner| {
            if dir.stat().unwrap().st_ino == provider {
                refused.set(refused.get() + 1);
                return Listing::plain(Err(libc::EDEADLK));
            }
            inner.list(dir)
        });
        assert_eq!(refused.get(), 1, "{backend}: listed once, no retry");
        assert_eq!(report.state, ScanState::Completed, "{backend}: the store downgrades for the skipped range");
        assert_eq!(sink.skipped.len(), 1);
        assert_eq!(sink.skipped[0].reason, SkipReason::CloudBoundary);
        assert!(sink.skipped[0].path.raw_bytes().ends_with(b"/provider"));
        assert!(report.issues.iter().any(|i| i.code == "cloud_materialization_avoided"), "{:?}", report.issues);
        assert!(sink.has("local.txt") && sink.has("provider") && !sink.has("remote.txt") && !sink.has("sub"));
        std::fs::remove_dir_all(&base).unwrap();
    }
}

/// MVP 5.5 / SCN-07: the selected root replaced mid-scan. With a re-check before every
/// directory the walk stops at once; with the production interval the final re-check still
/// refuses to call the result `completed`. Confirmed rows stay; the new directory is never read.
#[test]
fn replaced_root_stops_the_walk_and_does_not_inherit_the_scope() {
    for backend in NAMES {
        for recheck in [1, walk::RECHECK_EVERY_DIRS] {
            let base = scratch(&format!("replaced-{backend}-{recheck}"));
            let (root, moved) = (base.join("root"), base.join("moved"));
            for d in ["a", "b", "c"] {
                std::fs::create_dir_all(root.join(d).join("deeper")).unwrap();
                std::fs::write(root.join(d).join(format!("in-{d}.txt")), b"x").unwrap();
            }
            let mut sink = MemSink::default();
            let (report, calls) = walk_hooked(backend, &root, &mut sink, recheck, |n, dir, inner| {
                let listing = inner.list(dir);
                if n == 1 {
                    std::fs::rename(&root, &moved).unwrap();
                    std::fs::create_dir(&root).unwrap();
                    std::fs::write(root.join("new.txt"), b"new").unwrap();
                }
                listing
            });
            assert_eq!(report.state, ScanState::Partial, "{backend}/{recheck}: {:?}", report.issues);
            assert!(report.issues.iter().any(|i| i.code == "root_changed"), "{:?}", report.issues);
            assert!(["a", "b", "c"].iter().all(|d| sink.has(d)), "confirmed rows are kept");
            assert!(!sink.has("new.txt"), "the replacement is never read");
            if recheck == 1 {
                assert_eq!(calls, 1, "no directory is entered after the change");
                assert_eq!(sink.skipped.len(), 1);
                assert!(!sink.has("in-a.txt") && !sink.has("in-c.txt"));
            } else {
                // Everything was read from the original directory through held descriptors.
                assert!(sink.has("in-a.txt") && sink.has("in-c.txt"));
            }
            std::fs::remove_dir_all(&base).unwrap();
        }
    }
}

fn hdiutil(args: &[&str], path: &Path) {
    let out = Command::new("hdiutil").args(args).arg(path).output().unwrap();
    assert!(out.status.success(), "hdiutil {args:?} {}: {}", path.display(), String::from_utf8_lossy(&out.stderr));
}

fn attach(image: &Path, at: &Path) {
    let out = Command::new("hdiutil").args(["attach", "-quiet", "-nobrowse", "-mountpoint"]).arg(at).arg(image).output().unwrap();
    assert!(out.status.success(), "attach {}: {}", image.display(), String::from_utf8_lossy(&out.stderr));
}

/// MVP 5.5 / SCN-07 "A mounted volume is replaced during scanning": the scanned volume is
/// force-detached and another volume is attached at the same path mid-scan.
#[test]
fn replaced_volume_stops_the_walk_and_does_not_inherit_the_scope() {
    if std::env::var_os("CM_ALLOW_DMG_TESTS").is_none() {
        eprintln!("skipped: set CM_ALLOW_DMG_TESTS=1 to attach disposable disk images");
        return;
    }
    let base = scratch("remount");
    let mnt = base.join("mnt");
    std::fs::create_dir_all(&mnt).unwrap();
    let images = [base.join("old.dmg"), base.join("new.dmg")];
    for (image, name) in images.iter().zip(["cmold", "cmnew"]) {
        hdiutil(&["create", "-quiet", "-size", "4m", "-fs", "HFS+", "-volname", name, "-o"], image);
    }
    for backend in NAMES {
        attach(&images[0], &mnt);
        for d in ["a", "b", "c"] {
            std::fs::create_dir_all(mnt.join(d).join("deeper")).unwrap();
            std::fs::write(mnt.join(d).join(format!("old-{d}.txt")), b"o").unwrap();
        }
        let mut sink = MemSink::default();
        let (report, calls) = walk_hooked(backend, &mnt, &mut sink, 1, |n, dir, inner| {
            let listing = inner.list(dir);
            if n == 1 {
                hdiutil(&["detach", "-quiet", "-force"], &mnt);
                attach(&images[1], &mnt);
                for d in ["a", "b", "c"] {
                    std::fs::create_dir_all(mnt.join(d)).unwrap();
                    std::fs::write(mnt.join(d).join("intruder.txt"), b"n").unwrap();
                }
            }
            listing
        });
        hdiutil(&["detach", "-quiet", "-force"], &mnt);
        eprintln!("{backend}: state={:?} issues={:?}", report.state, report.issues);
        assert_eq!(report.state, ScanState::Partial, "{backend}");
        // Disk images re-attached on the same disk node reuse st_dev and the root inode, so this
        // is the volume UUID re-check speaking (the dead old descriptor would say volume_offline).
        assert!(report.issues.iter().any(|i| i.code == "volume_changed"), "{:?}", report.issues);
        assert_eq!(calls, 1, "no directory of either volume is entered after the change");
        assert!(["a", "b", "c"].iter().all(|d| sink.has(d)), "confirmed rows are kept");
        assert!(!sink.has("intruder.txt") && !sink.has("old-a.txt"));
        assert_eq!(sink.skipped.len(), 1);
        assert_eq!(sink.skipped[0].reason, SkipReason::VolumeChanged);
    }
    std::fs::remove_dir_all(&base).unwrap();
}

// ---------------------------------------------------------------------------------------------
// MVP 6.2 / TC-P0-002 / TC-P0-004 benchmark harness (macOS part). Prints raw values only.
// ---------------------------------------------------------------------------------------------

/// Measures one backend scan without a database: `index_complete_ms` is the time until the
/// backend returned with every row handed to the sink (the CLI adds SQLite staging/publish).
struct BenchSink {
    start: Instant,
    first_batch: Option<Duration>,
    writes: u64,
    entries: u64,
    objects: u64,
    skipped: u64,
    unknown_size_files: u64,
    shared: [u64; 3],
    peak_queue_rows: u64,
    cancel_after_entries: Option<u64>,
    cancel_at: Option<Instant>,
    entries_at_cancel: u64,
    polls_after_cancel: u64,
}

impl ScanSink for BenchSink {
    fn write(&mut self, o: &[ObjectRow], e: &[EntryRow], s: &[SkippedRow]) -> Result<(), String> {
        self.first_batch.get_or_insert_with(|| self.start.elapsed());
        self.writes += 1;
        self.entries += e.len() as u64;
        self.objects += o.len() as u64;
        self.skipped += s.len() as u64;
        self.peak_queue_rows = self.peak_queue_rows.max((o.len() + e.len() + s.len()) as u64);
        for x in o {
            if x.kind == ObjectKind::File && x.logical.is_none() {
                self.unknown_size_files += 1;
            }
            self.shared[match x.shared_blocks {
                Some(true) => 0,
                Some(false) => 1,
                None => 2,
            }] += 1;
        }
        Ok(())
    }
    fn cancel_requested(&mut self) -> bool {
        if self.cancel_at.is_some() {
            self.polls_after_cancel += 1;
            return true;
        }
        if self.cancel_after_entries.is_some_and(|n| self.entries >= n) {
            self.cancel_at = Some(Instant::now());
            self.entries_at_cancel = self.entries;
            return true;
        }
        false
    }
}

fn rusage() -> (f64, f64, libc::c_long) {
    // SAFETY: zeroed POD out-parameter of the declared type.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: valid out-pointer; RUSAGE_SELF needs no other argument.
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let to_ms = |t: libc::timeval| t.tv_sec as f64 * 1e3 + f64::from(t.tv_usec) / 1e3;
    (to_ms(ru.ru_utime), to_ms(ru.ru_stime), ru.ru_maxrss)
}

fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn ms(d: Duration) -> String {
    format!("{:.3}", d.as_secs_f64() * 1e3)
}

fn opt_ms(d: Option<Duration>) -> String {
    d.map(ms).unwrap_or_else(|| "null".into())
}

/// One measured scan; returns `(json, entries, index_complete_ms)`.
fn bench_once(backend: &dyn ScanBackend, root: &Path, batch: usize, label: &str, round: usize, cancel_after: Option<u64>) -> (String, u64, f64) {
    let (u0, s0, _) = rusage();
    let mut sink = BenchSink {
        start: Instant::now(),
        first_batch: None,
        writes: 0,
        entries: 0,
        objects: 0,
        skipped: 0,
        unknown_size_files: 0,
        shared: [0; 3],
        peak_queue_rows: 0,
        cancel_after_entries: cancel_after,
        cancel_at: None,
        entries_at_cancel: 0,
        polls_after_cancel: 0,
    };
    let report = backend.scan(&raw(root), &mut sink, &ScanOptions { batch_entries: batch, ..Default::default() });
    let done = sink.start.elapsed();
    let returned = Instant::now();
    let (u1, s1, maxrss) = rusage();
    let cancel = match (cancel_after, sink.cancel_at) {
        (Some(n), Some(at)) => format!(
            ",\"cancel_after_entries\":{n},\"entries_at_cancel\":{},\"cancel_to_return_ms\":{},\"polls_after_cancel\":{}",
            sink.entries_at_cancel,
            ms(returned - at),
            sink.polls_after_cancel
        ),
        (Some(n), None) => format!(",\"cancel_after_entries\":{n},\"entries_at_cancel\":null,\"cancel_to_return_ms\":null"),
        _ => String::new(),
    };
    let json = format!(
        "{{\"kind\":\"{}\",\"backend\":\"{}\",\"label\":\"{label}\",\"round\":{round},\"state\":\"{:?}\",\"first_batch_ms\":{},\"index_complete_ms\":{},\
\"cpu_user_ms\":{:.3},\"cpu_sys_ms\":{:.3},\"process_max_rss_bytes\":{maxrss},\"entries\":{},\"objects\":{},\"skipped_ranges\":{},\"boundaries\":{},\
\"unknown_size_files\":{},\"shared_blocks\":{{\"true\":{},\"false\":{},\"null\":{}}},\"batches\":{},\"batch_entries\":{batch},\"peak_queue_rows\":{},\
\"issues\":{},\"backend_reason\":{}{cancel}}}",
        if cancel_after.is_some() { "cancel" } else { "scan" },
        backend.name(),
        report.state,
        opt_ms(sink.first_batch),
        ms(done),
        u1 - u0,
        s1 - s0,
        sink.entries,
        sink.objects,
        report.skipped_ranges,
        report.boundaries,
        sink.unknown_size_files,
        sink.shared[0],
        sink.shared[1],
        sink.shared[2],
        sink.writes,
        sink.peak_queue_rows,
        report.issues.len(),
        json_str(&report.backend_reason),
    );
    (json, sink.entries, done.as_secs_f64() * 1e3)
}

fn env_num(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Raw posix-vs-bulk timings on a caller-chosen, read-only tree (MVP 6.2). Every run prints
/// one `CM_BENCH {json}` line; nothing is dropped or ranked, and the closing summary only
/// restates median/min/max over all warm runs (verification-plan §4.2: ≥5 warm runs after
/// one unscored warm-up, ≥3 cold attempts after controlled restarts, alternating order).
///
/// - `CM_BENCH_ROOT` (required): the tree to scan, e.g. a `cm-fixtures create --profile p0-100k` root.
/// - `CM_BENCH_RUNS` (default 5): warm rounds; each round scans with both backends.
/// - `CM_BENCH_FIRST` (`posix`|`bulk`, default `posix`): backend of round 0.
/// - `CM_BENCH_COLD_ATTEMPT=1`: label round 0's first scan `cold_attempt` (set it only for the
///   first run after a reboot; whether caches were really cold stays unknown).
/// - `CM_BENCH_BATCH` (default 4096): rows per sink batch.
/// - `CM_BENCH_CANCEL_RUNS` (default `CM_BENCH_RUNS`): cancellations per backend at 10/50/90 %
///   of the last warm run's entries. The sink sees rows per batch, so a cancel point is reached
///   at batch granularity (`entries_at_cancel`) and one past the last batch is reported with
///   `null` latency; use a smaller `CM_BENCH_BATCH` for finer points (recorded in every line).
#[test]
#[ignore = "benchmark: CM_BENCH_ROOT=<dir> cargo test --release -p cm-platform --lib bench_posix_vs_bulk -- --ignored --nocapture"]
fn bench_posix_vs_bulk() {
    let Some(root) = std::env::var_os("CM_BENCH_ROOT").map(PathBuf::from) else {
        eprintln!("skipped: set CM_BENCH_ROOT to the directory to measure");
        return;
    };
    let runs = env_num("CM_BENCH_RUNS", 5).max(1);
    let cancel_runs = env_num("CM_BENCH_CANCEL_RUNS", runs);
    let batch = env_num("CM_BENCH_BATCH", ScanOptions::default().batch_entries).max(1);
    let cold = std::env::var("CM_BENCH_COLD_ATTEMPT").is_ok_and(|v| v == "1");
    let pair: [&dyn ScanBackend; 2] =
        if std::env::var("CM_BENCH_FIRST").is_ok_and(|v| v == "bulk") { [&BulkBackend, &PosixBackend] } else { [&PosixBackend, &BulkBackend] };
    let probe = BulkBackend.probe(&raw(&root));
    let os = Command::new("sw_vers").args(["-productVersion"]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default();
    let model = Command::new("sysctl").args(["-n", "hw.model"]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default();
    println!(
        "CM_BENCH {{\"kind\":\"environment\",\"root\":{},\"filesystem\":{},\"volume_id\":{},\"macos\":{},\"model\":{},\"arch\":\"{}\",\"build\":\"{}\",\
\"runs\":{runs},\"cancel_runs\":{cancel_runs},\"batch_entries\":{batch},\"cold_attempt_requested\":{cold},\"first\":\"{}\",\"euid\":{},\
\"note\":\"index_complete_ms excludes SQLite; process_max_rss_bytes is the process-wide peak so far; cold cache state is unknown\"}}",
        json_str(&root.display().to_string()),
        json_str(probe.filesystem.as_deref().unwrap_or("unknown")),
        json_str(probe.volume_id.as_deref().unwrap_or("unknown")),
        json_str(&os),
        json_str(&model),
        std::env::consts::ARCH,
        if cfg!(debug_assertions) { "debug" } else { "release" },
        pair[0].name(),
        // SAFETY: plain getter.
        unsafe { libc::geteuid() },
    );
    let mut warm: [Vec<(f64, u64)>; 2] = [Vec::new(), Vec::new()];
    for round in 0..=runs {
        let order = if round % 2 == 0 { [0, 1] } else { [1, 0] };
        for (i, b) in order.into_iter().enumerate() {
            let label = match (round, i) {
                (0, 0) if cold => "cold_attempt",
                (0, _) => "warmup_unscored",
                _ => "warm",
            };
            let (json, entries, total) = bench_once(pair[b], &root, batch, label, round, None);
            println!("CM_BENCH {json}");
            if round > 0 {
                warm[b].push((total, entries));
            }
        }
    }
    for (b, backend) in pair.iter().enumerate() {
        let total = warm[b].last().map_or(0, |w| w.1);
        for pct in [10u64, 50, 90] {
            for rep in 0..cancel_runs {
                let (json, _, _) = bench_once(*backend, &root, batch, &format!("cancel_{pct}pct"), rep, Some(total * pct / 100));
                println!("CM_BENCH {json}");
            }
        }
    }
    for (b, backend) in pair.iter().enumerate() {
        let mut t: Vec<f64> = warm[b].iter().map(|w| w.0).collect();
        let raw_list = t.iter().map(|v| format!("{v:.3}")).collect::<Vec<_>>().join(",");
        t.sort_by(f64::total_cmp);
        let median = if t.len() % 2 == 1 { t[t.len() / 2] } else { (t[t.len() / 2 - 1] + t[t.len() / 2]) / 2.0 };
        println!(
            "CM_BENCH {{\"kind\":\"summary\",\"backend\":\"{}\",\"warm_runs\":{},\"index_complete_ms_raw\":[{raw_list}],\"median_ms\":{median:.3},\"min_ms\":{:.3},\"max_ms\":{:.3}}}",
            backend.name(),
            t.len(),
            t[0],
            t[t.len() - 1]
        );
    }
}
