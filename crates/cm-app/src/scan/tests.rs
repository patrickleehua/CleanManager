use std::cell::RefCell;
use std::path::PathBuf;

use super::*;
use cm_core::capability::CapabilityState;
use cm_core::model::{CloudState, IdentityQuality, ObjectKind};
use cm_core::scan::RootProbe;
use cm_store::index::{EntryQuery, Filter, KindFilter, Sort};

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-app-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn host() -> HostRef {
    HostRef { kind: "cli".into(), pid: std::process::id(), started_at: "2026-10-15T00:00:00Z".into() }
}

fn row(id: u64, parent: Option<u64>, kind: ObjectKind) -> (ObjectRow, EntryRow) {
    let o = ObjectRow {
        object_id: id,
        volume_id: "v".into(),
        file_id: format!("inode64:{id:016x}"),
        generation: None,
        quality: IdentityQuality::Strong,
        kind,
        logical: (kind == ObjectKind::File).then_some(10),
        allocated: None,
        link_count: None,
        modified_unix: None,
        cloud: CloudState::NotCloud,
    };
    let name = if parent.is_none() { "/r".to_string() } else { format!("n{id}") };
    let e = EntryRow { entry_id: id, parent_entry_id: parent, name: RawPath::unix(name.into_bytes()).unwrap(), object_id: id, kind, boundary: None };
    (o, e)
}

/// Writes one batch per "directory"; before each batch it polls for cancellation, then lets
/// the test inject actions (e.g. a cancel from another connection) and observe state.
struct ScriptedBackend<'a> {
    batches: u64,
    on_batch: RefCell<Box<dyn FnMut(u64) + 'a>>,
    after_cancel_seen: RefCell<Box<dyn FnMut() + 'a>>,
}

impl ScanBackend for ScriptedBackend<'_> {
    fn name(&self) -> &'static str {
        "test.scripted"
    }
    fn probe(&self, _: &RawPath) -> RootProbe {
        RootProbe { filesystem: None, volume_id: None, backend: "test.scripted".into(), state: CapabilityState::Unverified, reason: "test".into() }
    }
    fn scan(&self, _: &RawPath, sink: &mut dyn ScanSink, _: &ScanOptions) -> ScanReport {
        let mut report = ScanReport {
            state: ScanState::Completed,
            backend: self.name().into(),
            backend_reason: "scripted".into(),
            entries: 0,
            skipped_ranges: 0,
            boundaries: 0,
            elevated_read: vec![],
            issues: vec![],
        };
        let (o, e) = row(1, None, ObjectKind::Directory);
        sink.write(&[o], &[e], &[]).unwrap();
        report.entries += 1;
        for b in 0..self.batches {
            (self.on_batch.borrow_mut())(b);
            if sink.cancel_requested() {
                // Simulates I/O that cannot be interrupted immediately.
                (self.after_cancel_seen.borrow_mut())();
                report.state = ScanState::Cancelled;
                return report;
            }
            let (o, e) = row(10 + b, Some(1), ObjectKind::File);
            sink.write(&[o], &[e], &[]).unwrap();
            report.entries += 1;
        }
        report
    }
}

fn request(id: &str) -> ScanRequest {
    ScanRequest { scan_id: id.into(), scope_id: "scope".into(), root: RawPath::unix(b"/r".to_vec()).unwrap(), host: host() }
}

fn files(index: &IndexStore, scan: &str) -> usize {
    index
        .list_entries(&EntryQuery { scan_id: scan.into(), sort: Sort::NameAsc,
            filter: Filter { kind: KindFilter::File, name_contains: None, parent_entry_id: None }, page_size: 500, cursor: None })
        .unwrap()
        .items
        .len()
}

fn cancel_at(point: u64, scan: &str) -> (ScanState, usize, Vec<ScanState>) {
    let dir = scratch(&format!("cancel-{point}"));
    let db = dir.join("index.db");
    let index = IndexStore::open(&db).unwrap();
    let other = IndexStore::open(&db).unwrap(); // a second connection, as another process would use
    let observed = RefCell::new(Vec::new());
    let backend = ScriptedBackend {
        batches: 5,
        on_batch: RefCell::new(Box::new(|b| {
            observed.borrow_mut().push(other.scan_status(scan).unwrap().state);
            if b == point {
                let receipt = other.request_cancel(scan).unwrap();
                assert!(receipt.accepted);
            }
        })),
        after_cancel_seen: RefCell::new(Box::new(|| {
            observed.borrow_mut().push(other.scan_status(scan).unwrap().state);
        })),
    };
    let out = run_scan(&index, &backend, &request(scan), &ScanOptions::default()).unwrap();
    let n = files(&index, scan);
    drop(backend);
    let seen = observed.into_inner();
    let late = other.request_cancel(scan).unwrap();
    assert_eq!((late.accepted, late.state_at_receipt.as_str()), (false, "already_terminal"));
    (out.state, n, seen)
}

#[test]
fn early_mid_and_late_cancellation_keep_confirmed_rows_and_show_stopping() {
    for (point, kept) in [(0u64, 0usize), (2, 2), (4, 4)] {
        let (state, n, seen) = cancel_at(point, &format!("s{point}"));
        assert_eq!(state, ScanState::Cancelled, "cancel at batch {point}");
        assert_eq!(n, kept, "rows confirmed before the cancel are published");
        assert_eq!(*seen.last().unwrap(), ScanState::Stopping, "while I/O drains the scan shows stopping");
        assert!(seen[..seen.len() - 1].iter().all(|s| *s == ScanState::Running), "never reported finished early: {seen:?}");
    }
}

#[test]
fn uncancelled_scan_completes_and_publishes_everything() {
    let dir = scratch("complete");
    let index = IndexStore::open(&dir.join("index.db")).unwrap();
    let backend = ScriptedBackend { batches: 3, on_batch: RefCell::new(Box::new(|_| {})), after_cancel_seen: RefCell::new(Box::new(|| {})) };
    let out = run_scan(&index, &backend, &request("ok"), &ScanOptions::default()).unwrap();
    assert_eq!(out.state, ScanState::Completed);
    assert_eq!(files(&index, "ok"), 3);
    assert_eq!(index.scan_report("ok").unwrap().unwrap()["backend"], "test.scripted");
}

#[cfg(windows)]
#[test]
fn volume_going_offline_mid_scan_publishes_partial_results() {
    use cm_platform::windows::directory::DirectoryBackend;
    use std::os::windows::ffi::OsStrExt;
    use std::process::Command;

    if std::env::var("CM_ALLOW_VHD_TESTS").as_deref() != Ok("1") {
        eprintln!("skipped: set CM_ALLOW_VHD_TESTS=1 to mount a disposable VHD");
        return;
    }
    let dir = scratch("offline");
    let letter = ('M'..='Z').rev().find(|c| !std::path::Path::new(&format!("{c}:\\")).exists()).unwrap();
    let vhd = dir.join("offline.vhdx");
    std::fs::write(dir.join("c.dp"), format!(
        "create vdisk file=\"{v}\" maximum=64 type=expandable\nselect vdisk file=\"{v}\"\nattach vdisk\ncreate partition primary\nformat fs=ntfs quick label=CMTEST\nassign letter={letter}\n",
        v = vhd.display())).unwrap();
    assert!(Command::new("diskpart").arg("/s").arg(dir.join("c.dp")).output().unwrap().status.success());
    for d in ["a", "b", "c"] {
        std::fs::create_dir_all(format!("{letter}:\\data\\{d}")).unwrap();
        std::fs::write(format!("{letter}:\\data\\{d}\\f.txt"), b"x").unwrap();
    }
    let dismount = format!("Dismount-DiskImage -ImagePath \"{}\" | Out-Null", vhd.display());

    struct Offline<'a> {
        dismount: &'a str,
    }
    impl ScanBackend for Offline<'_> {
        fn name(&self) -> &'static str {
            "win.directory"
        }
        fn probe(&self, root: &RawPath) -> RootProbe {
            DirectoryBackend.probe(root)
        }
        fn scan(&self, root: &RawPath, sink: &mut dyn ScanSink, o: &ScanOptions) -> ScanReport {
            struct Hook<'s, 'd> {
                inner: &'s mut dyn ScanSink,
                calls: usize,
                dismount: &'d str,
            }
            impl ScanSink for Hook<'_, '_> {
                fn write(&mut self, o: &[ObjectRow], e: &[EntryRow], s: &[SkippedRow]) -> Result<(), String> {
                    self.inner.write(o, e, s)
                }
                fn cancel_requested(&mut self) -> bool {
                    self.calls += 1;
                    if self.calls == 3 {
                        let _ = Command::new("powershell").args(["-NoProfile", "-Command", self.dismount]).output();
                    }
                    self.inner.cancel_requested()
                }
            }
            cm_platform::windows::privileges::disable_bypass_privileges().unwrap();
            DirectoryBackend.scan(root, &mut Hook { inner: sink, calls: 0, dismount: self.dismount }, o)
        }
    }
    let index = IndexStore::open(&dir.join("index.db")).unwrap();
    let root: Vec<u16> = std::ffi::OsStr::new(&format!("{letter}:\\data")).encode_wide().collect();
    let req = ScanRequest { scan_id: "off".into(), scope_id: "s".into(), root: RawPath::windows(root).unwrap(), host: host() };
    let out = run_scan(&index, &Offline { dismount: &dismount }, &req, &ScanOptions { batch_entries: 1 }).unwrap();
    let _ = Command::new("powershell").args(["-NoProfile", "-Command", &format!("{dismount}; Remove-Item \"{}\"", vhd.display())]).output();
    assert_eq!(out.state, ScanState::Partial, "{:?}", out.report.issues);
    assert!(out.report.skipped_ranges >= 1);
    assert!(files(&index, "off") >= 1, "rows read before the volume disappeared are kept");
    assert_eq!(index.scan_status("off").unwrap().completeness.as_deref(), Some("partial_scope"));
}
