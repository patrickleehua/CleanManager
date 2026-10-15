//! Real-file-system tests for `mac.posix` and `mac.bulk`. They run on macOS only; every case
//! runs against both backends, and the two backends are reconciled row by row.

use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use cm_core::capability::CapabilityState;
use cm_core::model::{BoundaryKind, CloudState, IdentityQuality, ObjectKind, ScanState, SkipReason};
use cm_core::path::RawPath;
use cm_core::scan::{EntryRow, ObjectRow, ScanBackend, ScanOptions, ScanReport, ScanSink, SkippedRow};
use cm_fixtures::{Profile, create, plan, prepare_sentinel, verify, verify_sentinel};

use super::bulk::BulkBackend;
use super::posix::PosixBackend;

#[derive(Default)]
struct MemSink {
    objects: Vec<ObjectRow>,
    entries: Vec<EntryRow>,
    skipped: Vec<SkippedRow>,
    writes: usize,
    cancel_after_writes: Option<usize>,
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
        self.cancel_after_writes.is_some_and(|n| self.writes >= n)
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
    backend.scan(&raw(root), sink, &ScanOptions { batch_entries: 4 })
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
                (path, e.kind, e.boundary, o.file_id.clone(), o.volume_id.clone(), o.quality, o.logical, o.allocated.is_some(), o.link_count, o.cloud, o.modified_unix)
            })
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    };
    let (posix, bulk) = (rows(&PosixBackend), rows(&BulkBackend));
    assert_eq!(posix.len(), bulk.len());
    for (p, b) in posix.iter().zip(&bulk) {
        assert_eq!(p, b);
    }
    let pipe = posix.iter().find(|r| r.0.ends_with(b"/pipe")).expect("fifo recorded");
    assert_eq!((pipe.1, pipe.6), (ObjectKind::Other, None), "a FIFO is recorded, never opened");
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
