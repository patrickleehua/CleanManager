use std::path::{Path, PathBuf};
use std::process::Command;

use super::*;
use cm_fixtures::{Profile, create, plan, prepare_sentinel, verify, verify_sentinel};

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
    fn object(&self, id: u64) -> &ObjectRow {
        self.objects.iter().find(|o| o.object_id == id).unwrap()
    }
}

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-winscan-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn raw(p: &Path) -> RawPath {
    use std::os::windows::ffi::OsStrExt;
    RawPath::windows(p.as_os_str().encode_wide().collect()).unwrap()
}

fn scan(root: &Path, sink: &mut MemSink) -> ScanReport {
    // Tests model an ordinary-permission scan even when the harness token has backup rights.
    super::super::privileges::disable_bypass_privileges().unwrap();
    DirectoryBackend.scan(&raw(root), sink, &ScanOptions { batch_entries: 4, ..Default::default() })
}

#[test]
fn tiny_fixture_is_enumerated_exactly_and_read_only() {
    let base = scratch("tiny");
    let (root, sentinel) = (base.join("root"), base.join("sentinel"));
    prepare_sentinel(&sentinel).unwrap();
    let manifest = create(Profile::Tiny, 3, &root, &sentinel).unwrap();
    let mut sink = MemSink::default();
    let report = scan(&root, &mut sink);
    assert_eq!(report.state, ScanState::Completed, "{:?}", report.issues);

    let files: Vec<_> = sink.entries.iter().filter(|e| e.kind == ObjectKind::File).collect();
    // 5 files + 1 hard link name + the fixture's own root marker file
    assert_eq!(files.len() as u64, manifest.plan.counts.files + manifest.plan.counts.hardlinks + 1);
    let dirs = sink.entries.iter().filter(|e| e.kind == ObjectKind::Directory && e.parent_entry_id.is_some()).count() as u64;
    assert_eq!(dirs, manifest.plan.counts.directories);

    let original = sink.by_name("original.dat");
    let link = sink.by_name("hardlink-to-original.dat");
    assert_eq!(original.object_id, link.object_id, "hard links share one object");
    assert_eq!(sink.object(original.object_id).logical, Some(2_048));
    assert_eq!(sink.object(sink.by_name("zero.bin").object_id).logical, Some(0));
    assert!(sink.objects.iter().all(|o| o.quality == IdentityQuality::Strong));

    let chinese = sink.by_name("中文 文件.txt");
    let RawPath::Windows(units) = &chinese.name else { panic!() };
    assert_eq!(units, &"中文 文件.txt".encode_utf16().collect::<Vec<_>>());
    assert!(sink.entries.iter().any(|e| sink.name_of(e).len() == 184), "180-char name kept exactly");

    for link in ["link-to-sentinel-file", "link-to-sentinel-dir"] {
        let e = sink.by_name(link);
        assert_eq!((e.kind, e.boundary), (ObjectKind::Symlink, Some(BoundaryKind::Symlink)));
    }
    assert!(!sink.entries.iter().any(|e| ["inner.txt", "sentinel.txt"].contains(&sink.name_of(e).as_str())), "links not followed");

    verify(&root).expect("scan did not modify the fixture");
    verify_sentinel(&sentinel).expect("scan did not touch the sentinel");
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn junctions_are_boundaries_and_not_traversed() {
    let base = scratch("junction");
    let (root, outside) = (base.join("root"), base.join("outside"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "out of scope").unwrap();
    let status = Command::new("cmd").args(["/C", "mklink", "/J"]).arg(root.join("jn")).arg(&outside).output().unwrap();
    assert!(status.status.success(), "mklink /J failed: {}", String::from_utf8_lossy(&status.stderr));
    let mut sink = MemSink::default();
    let report = scan(&root, &mut sink);
    assert_eq!(report.state, ScanState::Completed);
    let jn = sink.by_name("jn");
    assert_eq!((jn.kind, jn.boundary), (ObjectKind::Reparse, Some(BoundaryKind::Junction)));
    assert!(!sink.entries.iter().any(|e| sink.name_of(e) == "secret.txt"));
    std::fs::remove_dir(root.join("jn")).unwrap();
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn unreadable_subtree_is_skipped_not_empty() {
    let base = scratch("denied");
    let locked = base.join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(locked.join("inside.txt"), "x").unwrap();
    std::fs::write(base.join("visible.txt"), "yy").unwrap();
    let deny = Command::new("icacls").arg(&locked).args(["/deny", "*S-1-1-0:(RX)"]).output().unwrap();
    assert!(deny.status.success(), "{}", String::from_utf8_lossy(&deny.stdout));
    let mut sink = MemSink::default();
    let report = scan(&base, &mut sink);
    let _ = Command::new("icacls").arg(&locked).args(["/remove:d", "*S-1-1-0"]).output();
    assert!(report.elevated_read.is_empty());
    assert!(report.backend_reason.contains("ordinary permissions"));
    assert_eq!(sink.skipped.len(), 1, "{:?}", report.issues);
    assert_eq!(sink.skipped[0].reason, SkipReason::PermissionDenied);
    assert!(report.issues.iter().any(|i| i.code == "permission_required"));
    assert!(sink.entries.iter().any(|e| sink.name_of(e) == "visible.txt"));
    assert!(!sink.entries.iter().any(|e| sink.name_of(e) == "inside.txt"));
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn long_paths_beyond_max_path_are_scanned() {
    let base = scratch("long");
    let mut deep = base.clone();
    for i in 0..12 {
        deep = deep.join(format!("{i:02}-{}", "d".repeat(28)));
    }
    let verbatim = PathBuf::from(format!("\\\\?\\{}", deep.display()));
    std::fs::create_dir_all(&verbatim).unwrap();
    std::fs::write(verbatim.join("deep.txt"), "abc").unwrap();
    assert!(verbatim.as_os_str().len() > 300);
    let mut sink = MemSink::default();
    let report = scan(&base, &mut sink);
    assert_eq!(report.state, ScanState::Completed, "{:?}", report.issues);
    let deep_file = sink.by_name("deep.txt");
    assert_eq!(sink.object(deep_file.object_id).logical, Some(3));
    std::fs::remove_dir_all(PathBuf::from(format!("\\\\?\\{}", base.display()))).unwrap();
}

#[test]
fn cancellation_stops_scheduling_and_keeps_confirmed_rows() {
    let base = scratch("cancel");
    let (root, sentinel) = (base.join("root"), base.join("sentinel"));
    prepare_sentinel(&sentinel).unwrap();
    create(Profile::Deep, 1, &root, &sentinel).unwrap();
    let mut sink = MemSink { cancel_after_writes: Some(1), ..Default::default() };
    let report = scan(&root, &mut sink);
    assert_eq!(report.state, ScanState::Cancelled);
    assert!(!sink.entries.is_empty(), "confirmed rows are kept");
    assert!((sink.entries.len() as u64) < plan(Profile::Deep, 1).counts.files * 2, "work stopped early");
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn missing_root_fails_with_a_typed_issue_not_an_empty_scan() {
    let base = scratch("missing");
    let mut sink = MemSink::default();
    let report = scan(&base.join("does-not-exist"), &mut sink);
    assert_eq!(report.state, ScanState::Failed);
    assert_eq!(report.issues[0].code, "root_unavailable");
    assert!(sink.entries.is_empty());
}

#[test]
fn probe_reports_filesystem_and_stays_unverified() {
    let base = scratch("probe");
    let p = DirectoryBackend.probe(&raw(&base));
    assert_eq!(p.filesystem.as_deref(), Some("NTFS"));
    assert_eq!(p.state, CapabilityState::Unverified);
    assert!(p.reason.contains("selected root"));
}

#[test]
fn sparse_compressed_and_alternate_streams_report_measured_allocation() {
    let base = scratch("sparse");
    let sparse = base.join("sparse.bin");
    std::fs::write(&sparse, b"x").unwrap();
    let flag = Command::new("fsutil").args(["sparse", "setflag"]).arg(&sparse).output().unwrap();
    assert!(flag.status.success(), "fsutil sparse: {}", String::from_utf8_lossy(&flag.stdout));
    let eof = Command::new("fsutil").args(["file", "seteof"]).arg(&sparse).arg("104857600").output().unwrap();
    assert!(eof.status.success(), "fsutil seteof: {}", String::from_utf8_lossy(&eof.stdout));
    let compressed = base.join("compressed.bin");
    std::fs::write(&compressed, vec![0u8; 1 << 20]).unwrap();
    let out = Command::new("compact").args(["/c", "/q"]).arg(&compressed).output().unwrap();
    assert!(out.status.success());
    let ads = base.join("with-stream.txt");
    std::fs::write(&ads, b"12345").unwrap();
    std::fs::write(format!("{}:hidden", ads.display()), vec![7u8; 8192]).unwrap();

    let mut sink = MemSink::default();
    let report = scan(&base, &mut sink);
    assert_eq!(report.state, ScanState::Completed, "{:?}", report.issues);
    let s = sink.object(sink.by_name("sparse.bin").object_id);
    assert_eq!(s.logical, Some(104_857_600));
    assert!(s.allocated.is_some_and(|a| a < 1 << 20), "sparse allocation measured, not the logical size: {:?}", s.allocated);
    assert_eq!(s.link_count, Some(1));
    let c = sink.object(sink.by_name("compressed.bin").object_id);
    assert_eq!(c.logical, Some(1 << 20));
    assert!(c.allocated.is_some_and(|a| a < 1 << 20), "compressed allocation measured: {:?}", c.allocated);
    let a = sink.object(sink.by_name("with-stream.txt").object_id);
    assert_eq!(a.logical, Some(5), "logical size covers the default stream only; alternate streams are not counted");
    std::fs::remove_dir_all(&base).unwrap();
}

use crate::windows::testvhd as vhd;

#[test]
fn cloud_placeholders_are_never_hydrated_or_populated_by_a_scan() {
    use crate::windows::testcloud::{FETCH_DATA, FETCH_PLACEHOLDERS, SyncRoot};
    use std::sync::atomic::Ordering;

    let base = scratch("cloud");
    let root = base.join("syncroot");
    std::fs::create_dir_all(&root).unwrap();
    let provider = SyncRoot::register(&root);
    provider.placeholder("remote-video.mp4", 10 * 1024 * 1024, false);
    provider.placeholder("remote-folder", 0, true);
    std::fs::write(root.join("local.txt"), b"local").unwrap();
    let (data_before, populate_before) = (FETCH_DATA.load(Ordering::SeqCst), FETCH_PLACEHOLDERS.load(Ordering::SeqCst));

    let mut sink = MemSink::default();
    let report = scan(&root, &mut sink);
    assert_eq!(report.state, ScanState::Completed, "{:?}", report.issues);
    assert_eq!(FETCH_DATA.load(Ordering::SeqCst), data_before, "the scan triggered no hydration");
    assert_eq!(FETCH_PLACEHOLDERS.load(Ordering::SeqCst), populate_before, "the scan populated no placeholder directory");

    let video = sink.object(sink.by_name("remote-video.mp4").object_id);
    assert_eq!(video.cloud, CloudState::Placeholder);
    assert_eq!(video.logical, Some(10 * 1024 * 1024), "size comes from metadata only");
    let folder = sink.by_name("remote-folder");
    assert_eq!(folder.boundary, Some(BoundaryKind::CloudProvider), "unverified provider directory is not traversed");
    assert_eq!(sink.object(sink.by_name("local.txt").object_id).cloud, CloudState::NotCloud);

    // Positive control: real content access is attributable to the provider counter.
    let target = root.join("remote-video.mp4");
    std::thread::spawn(move || {
        let _ = std::fs::read(target);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while FETCH_DATA.load(Ordering::SeqCst) == data_before && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(FETCH_DATA.load(Ordering::SeqCst) > data_before, "reading a placeholder does reach the provider");
    drop(provider);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn exfat_volume_uses_fallback_listing_and_weak_identity() {
    if !vhd::allowed() {
        eprintln!("skipped: set CM_ALLOW_VHD_TESTS=1 to mount a disposable exFAT VHD");
        return;
    }
    let base = scratch("exfat");
    let letter = vhd::free_letter();
    let _disk = vhd::Vhd::create(base.join("exfat.vhdx"), "exfat", letter);
    let root = PathBuf::from(format!("{letter}:\\data"));
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub").join("中文.txt"), vec![1u8; 3000]).unwrap();
    assert_eq!(DirectoryBackend.probe(&raw(&root)).filesystem.as_deref(), Some("exFAT"));
    let mut sink = MemSink::default();
    let report = scan(&root, &mut sink);
    assert_eq!(report.state, ScanState::Completed, "{:?}", report.issues);
    assert!(report.backend_reason.contains("exFAT"));
    assert!(sink.objects.iter().all(|o| o.quality == IdentityQuality::Weak), "non-NTFS identities are weak");
    assert_eq!(sink.object(sink.by_name("中文.txt").object_id).logical, Some(3000));
}

#[test]
fn volume_replaced_during_scan_stops_and_keeps_confirmed_results() {
    if !vhd::allowed() {
        eprintln!("skipped: set CM_ALLOW_VHD_TESTS=1 to mount disposable VHDs");
        return;
    }
    let base = scratch("swap");
    let letter = vhd::free_letter();
    let b = vhd::Vhd::create(base.join("b.vhdx"), "ntfs", letter);
    std::fs::create_dir_all(format!("{letter}:\\data\\sub")).unwrap();
    std::fs::write(format!("{letter}:\\data\\sub\\other-volume.txt"), b"b").unwrap();
    b.dismount();
    let a = vhd::Vhd::create(base.join("a.vhdx"), "ntfs", letter);
    std::fs::create_dir_all(format!("{letter}:\\data\\sub")).unwrap();
    std::fs::write(format!("{letter}:\\data\\top.txt"), b"a").unwrap();

    struct SwapSink<'a> {
        inner: MemSink,
        calls: usize,
        swap: Box<dyn FnMut() + 'a>,
    }
    impl ScanSink for SwapSink<'_> {
        fn write(&mut self, o: &[ObjectRow], e: &[EntryRow], s: &[SkippedRow]) -> Result<(), String> {
            self.inner.write(o, e, s)
        }
        fn cancel_requested(&mut self) -> bool {
            self.calls += 1;
            if self.calls == 2 {
                (self.swap)(); // the root was listed; replace the volume before descending
            }
            false
        }
    }
    let mut sink = SwapSink {
        inner: MemSink::default(),
        calls: 0,
        swap: Box::new(|| {
            a.dismount();
            b.mount_at(letter);
        }),
    };
    super::super::privileges::disable_bypass_privileges().unwrap();
    let root = PathBuf::from(format!("{letter}:\\data"));
    let report = DirectoryBackend.scan(&raw(&root), &mut sink, &ScanOptions { batch_entries: 4, ..Default::default() });
    assert_eq!(report.state, ScanState::Partial, "{:?}", report.issues);
    assert!(report.issues.iter().any(|i| i.code == "volume_changed"));
    assert!(sink.inner.skipped.iter().any(|s| s.reason == SkipReason::VolumeChanged));
    assert!(sink.inner.entries.iter().any(|e| e.name.display() == "top.txt"), "results from the original volume are kept");
    assert!(!sink.inner.entries.iter().any(|e| e.name.display() == "other-volume.txt"), "nothing is read from the new volume");
}

#[test]
fn own_store_directory_is_a_reported_boundary_not_a_refusal() {
    let root = scratch("own-store");
    let store = root.join("AppData").join("CleanManager").join("store");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(store.join("index.db"), vec![0u8; 4096]).unwrap();
    std::fs::write(root.join("keep.txt"), b"hello").unwrap();
    super::super::privileges::disable_bypass_privileges().unwrap();
    let mut sink = MemSink::default();
    let opts = ScanOptions { batch_entries: 4, own_store_dirs: vec![raw(&store)] };
    let report = DirectoryBackend.scan(&raw(&root), &mut sink, &opts);
    assert_eq!(report.state, ScanState::Completed, "a skipped own store is not an unread range: {:?}", report.issues);
    let st = sink.by_name("store");
    assert_eq!((st.kind, st.boundary), (ObjectKind::Directory, Some(BoundaryKind::OwnStore)));
    assert!(!sink.entries.iter().any(|e| e.name.display() == "index.db"), "the store is never traversed");
    assert!(sink.entries.iter().any(|e| e.name.display() == "keep.txt"));
    assert!(report.issues.iter().any(|i| i.code == "own_store_skipped" && i.scope_display.ends_with("store")), "reported, not silent");
    assert!(sink.skipped.is_empty());

    // Scanning the store itself records only the root as the boundary.
    let mut sink = MemSink::default();
    let report = DirectoryBackend.scan(&raw(&store), &mut sink, &opts);
    assert_eq!(sink.entries.len(), 1);
    assert_eq!(sink.entries[0].boundary, Some(BoundaryKind::OwnStore));
    assert!(report.issues.iter().any(|i| i.code == "own_store_skipped"));
    let _ = std::fs::remove_dir_all(&root);
}
