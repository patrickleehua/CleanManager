use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use super::*;
use crate::windows::directory::DirectoryBackend;
use crate::windows::privileges::disable_bypass_privileges;
use crate::windows::testvhd as vhd;
use cm_core::scan::SkippedRow;

#[derive(Default)]
struct MemSink {
    objects: Vec<ObjectRow>,
    entries: Vec<EntryRow>,
}

impl ScanSink for MemSink {
    fn write(&mut self, o: &[ObjectRow], e: &[EntryRow], _s: &[SkippedRow]) -> Result<(), String> {
        self.objects.extend_from_slice(o);
        self.entries.extend_from_slice(e);
        Ok(())
    }
    fn cancel_requested(&mut self) -> bool {
        false
    }
}

/// (relative path → (kind, logical, allocated, boundary, object id)) for entries under `prefix`.
type View = BTreeMap<String, (ObjectKind, Option<u64>, Option<u64>, Option<BoundaryKind>, u64)>;

fn view(sink: &MemSink, prefix: &str) -> View {
    fn path(by_id: &HashMap<u64, &EntryRow>, start: &EntryRow) -> String {
        let mut parts = Vec::new();
        let mut e = start;
        while let Some(p) = e.parent_entry_id {
            parts.push(e.name.display());
            e = by_id[&p];
        }
        parts.reverse();
        parts.join("\\")
    }
    let by_id: HashMap<u64, &EntryRow> = sink.entries.iter().map(|e| (e.entry_id, e)).collect();
    let objects: HashMap<u64, &ObjectRow> = sink.objects.iter().map(|o| (o.object_id, o)).collect();
    sink.entries
        .iter()
        .filter(|e| e.parent_entry_id.is_some())
        .map(|e| (path(&by_id, e), e))
        .filter(|(p, _)| p.starts_with(prefix))
        .map(|(p, e)| {
            let o = objects[&e.object_id];
            (p, (e.kind, o.logical, o.allocated, e.boundary, e.object_id))
        })
        .collect()
}

fn raw(p: &Path) -> RawPath {
    RawPath::windows(p.as_os_str().encode_wide().collect()).unwrap()
}

#[test]
fn directory_scope_is_refused_before_any_read() {
    let base = std::env::temp_dir().join(format!("cm-ntfs-scope-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    assert_eq!(NtfsEnumBackend.probe(&raw(&base)).state, CapabilityState::Unsupported);
    let mut sink = MemSink::default();
    let report = NtfsEnumBackend.scan(&raw(&base), &mut sink, &ScanOptions::default());
    assert_eq!(report.state, ScanState::Failed);
    assert_eq!(report.issues[0].code, "scope_requires_volume");
    assert!(sink.entries.is_empty(), "nothing was enumerated");
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn ntfs_enum_matches_directory_baseline_on_a_test_volume() {
    if !vhd::allowed() {
        eprintln!("skipped: set CM_ALLOW_VHD_TESTS=1 to mount a disposable NTFS VHD");
        return;
    }
    let base = std::env::temp_dir().join(format!("cm-ntfs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let letter = vhd::free_letter();
    let _disk = vhd::Vhd::create(base.join("ntfs.vhdx"), "ntfs", letter);
    let root = PathBuf::from(format!("{letter}:\\"));
    let data = root.join("data");
    std::fs::create_dir_all(data.join("a").join("深 层")).unwrap();
    std::fs::create_dir_all(data.join("b")).unwrap();
    std::fs::create_dir_all(base.join("outside")).unwrap();
    for i in 0..40 {
        std::fs::write(data.join("a").join(format!("f{i:02}.bin")), vec![i as u8; i * 97]).unwrap();
    }
    std::fs::write(data.join("a").join("深 层").join("中文.txt"), "你好").unwrap();
    std::fs::write(data.join("a").join("linked.dat"), vec![9u8; 5000]).unwrap();
    std::fs::hard_link(data.join("a").join("linked.dat"), data.join("b").join("second-name.dat")).unwrap();
    let jn = Command::new("cmd").args(["/C", "mklink", "/J"]).arg(data.join("jn")).arg(base.join("outside")).output().unwrap();
    assert!(jn.status.success());
    let sparse = data.join("b").join("sparse.bin");
    std::fs::write(&sparse, b"s").unwrap();
    assert!(Command::new("fsutil").args(["sparse", "setflag"]).arg(&sparse).output().unwrap().status.success());
    assert!(Command::new("fsutil").args(["file", "seteof"]).arg(&sparse).arg("33554432").output().unwrap().status.success());

    disable_bypass_privileges().unwrap();
    let t = Instant::now();
    let mut dir_sink = MemSink::default();
    let dir_report = DirectoryBackend.scan(&raw(&root), &mut dir_sink, &ScanOptions::default());
    let dir_ms = t.elapsed().as_millis();
    let t = Instant::now();
    let mut ntfs_sink = MemSink::default();
    let ntfs_report = NtfsEnumBackend.scan(&raw(&root), &mut ntfs_sink, &ScanOptions::default());
    let ntfs_ms = t.elapsed().as_millis();
    eprintln!("tiny volume, end to end incl. supplement: directory {dir_ms} ms, ntfs_enum {ntfs_ms} ms (not a benchmark)");
    assert_eq!(ntfs_report.state, ScanState::Completed, "{:?}", ntfs_report.issues);
    assert!(matches!(dir_report.state, ScanState::Completed | ScanState::Partial));

    let a = view(&dir_sink, "data");
    let b = view(&ntfs_sink, "data");
    let strip = |v: &View| v.iter().map(|(k, (kind, l, al, bd, _))| (k.clone(), (*kind, *l, *al, *bd))).collect::<BTreeMap<_, _>>();
    assert_eq!(strip(&a), strip(&b), "same names, kinds, sizes and boundaries under \\data");
    assert!(b.len() >= 46);
    for v in [&a, &b] {
        assert_eq!(v["data\\a\\linked.dat"].4, v["data\\b\\second-name.dat"].4, "both hard-link names share one object");
        assert_eq!(v["data\\jn"].3, Some(BoundaryKind::Junction));
        assert!(v["data\\b\\sparse.bin"].2.is_some_and(|x| x < 1 << 20));
    }
    let names: BTreeSet<_> = b.keys().collect();
    assert!(!names.iter().any(|n| n.starts_with("data\\jn\\")), "junction not traversed");
    drop(_disk);
    let _ = std::fs::remove_dir_all(&base);
}
