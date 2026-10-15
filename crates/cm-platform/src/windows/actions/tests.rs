//! Real-NTFS tests for the Windows executor actions (temp directories only).

use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use cm_core::analysis::{EntryFacts, MemView};
use cm_core::capability::Platform;
use cm_core::model::{CloudState, IdentityQuality, ObjectKind};
use cm_core::plan::{WireIdentity, observe};

use super::*;

const AT: &str = "2026-10-15T00:00:00Z";

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-actions-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn raw(p: &Path) -> RawPath {
    RawPath::windows(wide(p)).unwrap()
}

fn facts(p: &Path, object_id: u64) -> EntryFacts {
    let md = std::fs::symlink_metadata(p).unwrap();
    let (volume_id, file_id) = super::super::directory::identity_of(&wide(p)).unwrap();
    EntryFacts {
        kind: if md.is_dir() { ObjectKind::Directory } else { ObjectKind::File },
        boundary: false,
        object_id,
        logical: md.is_file().then_some(md.len()),
        allocated: None,
        volume_id,
        file_id,
        quality: IdentityQuality::Strong,
        cloud: CloudState::NotCloud,
        modified_unix: Some(cm_core::time::filetime_to_unix(md.last_write_time())),
        link_count: None,
        skipped_here: 0,
    }
}

/// A planned file: identity and CM-OBS-v1 observation built from the live tree via `observe`.
struct Planned {
    path: RawPath,
    identity: WireIdentity,
    obs: ObservationRecord,
    chain: Vec<Identity>,
    probes: Vec<String>,
    owner: Option<String>,
}

impl Planned {
    fn target(&self) -> ActionTarget<'_> {
        ActionTarget {
            action_id: "a1",
            path: &self.path,
            identity: &self.identity,
            parent_chain: &self.chain,
            observation: &self.obs,
            probes: &self.probes,
            owner_id: self.owner.as_deref(),
        }
    }
}

fn plan(root: &Path, file: &Path) -> Planned {
    let rel = file.strip_prefix(root).unwrap();
    let mut view = MemView { platform: Some(Platform::Windows), ..Default::default() };
    view.names.insert(0, raw(root));
    view.parents.insert(0, None);
    view.facts.insert(0, facts(root, 0));
    let mut cur = root.to_path_buf();
    let mut id = 0u64;
    for comp in rel.components() {
        let parent = id;
        id += 1;
        cur = cur.join(comp);
        view.names.insert(id, raw(Path::new(comp.as_os_str())));
        view.parents.insert(id, Some(parent));
        view.kids.entry(parent).or_default().push(id);
        view.facts.insert(id, facts(&cur, id));
    }
    let obs = observe(&view, id, AT);
    assert_eq!(obs.path, raw(file));
    let f = &view.facts[&id];
    let identity = WireIdentity { volume_id: f.volume_id.clone(), file_id: f.file_id.clone(), generation: None, quality: "strong".into(), observed_at: AT.into() };
    Planned { path: raw(file), identity, chain: obs.parent_chain.clone(), obs, probes: Vec::new(), owner: None }
}

fn write(p: &Path, body: &str) {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(p, body).unwrap();
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

fn skipped(o: &ActionOutcome) -> &str {
    match o {
        ActionOutcome::Skipped { code, .. } => code,
        other => panic!("expected skipped, got {other:?}"),
    }
}

fn failed(o: &ActionOutcome) -> &str {
    match o {
        ActionOutcome::Failed { code, .. } => code,
        other => panic!("expected failed, got {other:?}"),
    }
}

const A: WindowsActions = WindowsActions::DEFAULT;

#[test]
fn identity_matches_the_scanner_format() {
    let root = scratch("identity");
    let f = root.join("f.txt");
    write(&f, "x");
    let (_, v) = full_path(&wide(&f)).unwrap();
    let l = live(&open_attrs(&v).unwrap()).unwrap();
    let (vol, fid) = super::super::directory::identity_of(&wide(&f)).unwrap();
    assert_eq!((l.volume_id, l.file_id), (vol, fid));
    let p = plan(&root, &f);
    assert_eq!(p.chain.len(), 1, "immediate parent is the scope root");
}

#[test]
fn path_splitting_keeps_roots() {
    let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    let s = |v: &[u16]| String::from_utf16_lossy(v);
    let (p, n) = split_parent(&w("\\\\?\\C:\\a\\b.txt")).unwrap();
    assert_eq!((s(&p).as_str(), s(&n).as_str()), ("\\\\?\\C:\\a", "b.txt"));
    let (p, n) = split_parent(&w("\\\\?\\C:\\b.txt")).unwrap();
    assert_eq!((s(&p).as_str(), s(&n).as_str()), ("\\\\?\\C:\\", "b.txt"));
    assert!(split_parent(&w("\\\\?\\C:\\")).is_none());
    let (p, _) = split_parent(&w("\\\\?\\UNC\\srv\\share\\x")).unwrap();
    assert_eq!(s(&p), "\\\\?\\UNC\\srv\\share\\");
    assert!(split_parent(&w("\\\\?\\UNC\\srv\\share\\")).is_none());
    assert!(valid_component("op-1") && !valid_component("..") && !valid_component("a\\b") && !valid_component(""));
}

#[test]
fn delete_removes_exactly_the_verified_file() {
    let root = scratch("delete");
    let f = root.join("d").join("target.tmp");
    let keep = root.join("d").join("keep.tmp");
    write(&f, "target");
    write(&keep, "keep");
    let p = plan(&root, &f);
    assert_eq!(p.chain.len(), 2);
    assert_eq!(A.check(&p.target()), LiveCheck::Match);
    assert_eq!(A.reconcile(&p.target(), Mode::Delete, None), Reconciled::NotPerformed);
    assert_eq!(A.delete(&p.target()), ActionOutcome::Succeeded { recovery: Recovery::Unavailable });
    assert!(!f.exists());
    assert_eq!(read(&keep), "keep");
    assert_eq!(A.check(&p.target()), LiveCheck::Missing);
    assert_eq!(A.reconcile(&p.target(), Mode::Delete, None), Reconciled::Performed);
    assert_eq!(skipped(&A.delete(&p.target())), "missing");
}

#[test]
fn replaced_file_is_skipped_and_the_replacement_survives() {
    let root = scratch("swap");
    let f = root.join("cache.bin");
    write(&f, "planned");
    let p = plan(&root, &f);
    std::fs::rename(&f, root.join("moved.bin")).unwrap();
    write(&f, "replace"); // same name, same size
    assert_eq!(A.check(&p.target()), LiveCheck::IdentityChanged);
    assert_eq!(skipped(&A.delete(&p.target())), "identity_changed");
    assert_eq!(skipped(&A.quarantine(&p.target(), &raw(&root.join("q")), "x")), "identity_changed");
    assert_eq!(skipped(&A.trash(&p.target())), "identity_changed");
    assert_eq!(read(&f), "replace");
    assert_eq!(read(&root.join("moved.bin")), "planned");
    assert_eq!(A.reconcile(&p.target(), Mode::Delete, None), Reconciled::StillUnknown);
}

#[test]
fn links_at_the_target_path_are_never_followed() {
    let root = scratch("links");
    let outside = scratch("links-outside");
    let sentinel = outside.join("sentinel.txt");
    write(&sentinel, "sentinel");

    // Junction swapped in for a planned file.
    let j = root.join("j.tmp");
    write(&j, "x");
    let pj = plan(&root, &j);
    std::fs::remove_file(&j).unwrap();
    let out = Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&j).arg(&outside).output().unwrap();
    assert!(out.status.success(), "mklink /J: {}", String::from_utf8_lossy(&out.stdout));
    // Symlink swapped in for another planned file.
    let s = root.join("s.tmp");
    write(&s, "x");
    let ps = plan(&root, &s);
    std::fs::remove_file(&s).unwrap();
    std::os::windows::fs::symlink_file(&sentinel, &s).unwrap();

    for p in [&pj, &ps] {
        assert_eq!(A.check(&p.target()), LiveCheck::LinkOrReparse);
        assert_eq!(skipped(&A.delete(&p.target())), "link_or_reparse");
        assert_eq!(skipped(&A.quarantine(&p.target(), &raw(&root.join("q")), "x")), "link_or_reparse");
        assert_eq!(skipped(&A.trash(&p.target())), "link_or_reparse");
    }
    for link in [&j, &s] {
        assert_ne!(std::fs::symlink_metadata(link).unwrap().file_attributes() & ATTR_REPARSE_POINT, 0, "link still in place");
    }
    assert_eq!(read(&sentinel), "sentinel");
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
    std::fs::remove_dir(&j).unwrap(); // removes the junction itself, not its target
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn file_in_use_is_reported_and_skipped() {
    let root = scratch("busy");
    let f = root.join("locked.log");
    write(&f, "busy");
    let p = plan(&root, &f);
    let holder = std::fs::OpenOptions::new().read(true).share_mode(0).open(&f).unwrap();
    assert_eq!(A.check(&p.target()), LiveCheck::InUse);
    assert_eq!(skipped(&A.delete(&p.target())), "in_use");
    assert_eq!(skipped(&A.quarantine(&p.target(), &raw(&root.join("q")), "x")), "in_use");
    drop(holder);
    assert_eq!(read(&f), "busy");
    assert_eq!(A.check(&p.target()), LiveCheck::Match);
}

#[test]
fn replaced_parent_is_detected_even_when_the_file_moved_along() {
    let root = scratch("parent");
    let f = root.join("d").join("f.tmp");
    write(&f, "x");
    let p = plan(&root, &f);
    std::fs::rename(root.join("d"), root.join("d-old")).unwrap();
    std::fs::create_dir(root.join("d")).unwrap();
    std::fs::rename(root.join("d-old").join("f.tmp"), &f).unwrap();
    assert_eq!(A.check(&p.target()), LiveCheck::ParentChanged);
    assert_eq!(skipped(&A.delete(&p.target())), "parent_changed");
    assert!(f.exists());
}

#[test]
fn changed_metadata_is_stale() {
    let root = scratch("meta");
    let f = root.join("grow.log");
    write(&f, "1");
    let p = plan(&root, &f);
    write(&f, "12345");
    assert!(matches!(A.check(&p.target()), LiveCheck::MetadataChanged(_)));
    assert_eq!(skipped(&A.delete(&p.target())), "metadata_changed");
    assert_eq!(read(&f), "12345");
}

#[test]
fn quarantine_moves_on_the_same_volume() {
    let root = scratch("quarantine");
    let f = root.join("d").join("old.cache");
    write(&f, "payload");
    let p = plan(&root, &f);
    let q = root.join("store").join("quarantine").join("op1");
    let out = A.quarantine(&p.target(), &raw(&q), "0001-old.cache");
    let dest = q.join("0001-old.cache");
    assert_eq!(out, ActionOutcome::Succeeded { recovery: Recovery::Quarantine { location: raw(&dest) } });
    assert!(!f.exists());
    assert_eq!(read(&dest), "payload");
    let (vol, fid) = super::super::directory::identity_of(&wide(&dest)).unwrap();
    assert_eq!((vol, fid), (p.identity.volume_id.clone(), p.identity.file_id.clone()));
    assert_eq!(A.reconcile(&p.target(), Mode::Quarantine, Some(&raw(&dest))), Reconciled::Performed);
    assert_eq!(A.reconcile(&p.target(), Mode::Quarantine, None), Reconciled::StillUnknown);
}

#[test]
fn quarantine_never_replaces_an_existing_name() {
    let root = scratch("noreplace");
    let f = root.join("a.tmp");
    write(&f, "planned");
    let p = plan(&root, &f);
    let q = root.join("q");
    write(&q.join("taken"), "existing");
    assert_eq!(failed(&A.quarantine(&p.target(), &raw(&q), "taken")), "destination_exists");
    assert_eq!(read(&f), "planned");
    assert_eq!(read(&q.join("taken")), "existing");
    assert_eq!(A.reconcile(&p.target(), Mode::Quarantine, Some(&raw(&q.join("taken")))), Reconciled::NotPerformed);
    assert_eq!(failed(&A.quarantine(&p.target(), &raw(&q), "..\\escape")), "invalid_name");
    assert_eq!(read(&f), "planned");
}

#[test]
fn quarantine_refuses_another_volume() {
    if !super::super::testvhd::allowed() {
        eprintln!("skipped: set CM_ALLOW_VHD_TESTS=1 (elevated) to mount a scratch volume");
        return;
    }
    let root = scratch("xvol");
    let f = root.join("a.tmp");
    write(&f, "planned");
    let p = plan(&root, &f);
    let letter = super::super::testvhd::free_letter();
    let _vhd = super::super::testvhd::Vhd::create(root.join("x.vhdx"), "ntfs", letter);
    let q = PathBuf::from(format!("{letter}:\\q"));
    assert_eq!(failed(&A.quarantine(&p.target(), &raw(&q), "a.tmp")), "cross_volume");
    assert_eq!(read(&f), "planned");
    assert!(!q.join("a.tmp").exists());
}

/// Final path of an open handle (test-side, to find and clean the recycled item).
fn held_final_path(f: &std::fs::File) -> Option<PathBuf> {
    use std::os::windows::io::AsRawHandle;
    let mut buf = vec![0u16; 32_768];
    // SAFETY: valid handle; buffer length in characters.
    let n = unsafe { GetFinalPathNameByHandleW(f.as_raw_handle(), buf.as_mut_ptr(), buf.len() as u32, VOLUME_NAME_DOS) } as usize;
    (n > 0 && n < buf.len()).then(|| os_path(&buf[..n]))
}

#[test]
fn trash_moves_to_the_recycle_bin() {
    let root = scratch("trash");
    let f = root.join("recycle-me.tmp");
    write(&f, "to the bin");
    let p = plan(&root, &f);
    // Attribute-only, share-all handle so the test can locate the recycled item afterwards.
    let watch = std::fs::OpenOptions::new().access_mode(FILE_READ_ATTRIBUTES).share_mode(SHARE_ALL).custom_flags(FLAG_OPEN_REPARSE_POINT).open(&f).unwrap();
    assert_eq!(A.trash(&p.target()), ActionOutcome::Succeeded { recovery: Recovery::Trash });
    assert!(!f.exists());
    assert_eq!(A.reconcile(&p.target(), Mode::Trash, None), Reconciled::StillUnknown);
    let recycled = held_final_path(&watch).expect("recycled path");
    drop(watch);
    let text = recycled.to_string_lossy().to_ascii_lowercase();
    assert!(text.contains("\\$recycle.bin\\"), "{text}");
    // Clean up the $R item and its $I metadata record.
    let name = recycled.file_name().unwrap().to_string_lossy().into_owned();
    std::fs::remove_file(&recycled).unwrap();
    if let Some(rest) = name.strip_prefix("$R") {
        let _ = std::fs::remove_file(recycled.with_file_name(format!("$I{rest}")));
    }
}

/// `\\localhost\C$\...` form of a local path (admin share; elevated test shell).
fn unc(p: &Path) -> Option<PathBuf> {
    let s = p.to_string_lossy();
    let (drive, rest) = s.split_once(":\\")?;
    let u = PathBuf::from(format!("\\\\localhost\\{drive}$\\{rest}"));
    u.exists().then_some(u)
}

#[test]
fn trash_on_a_volume_without_recycle_bin_never_deletes() {
    let root = scratch("netbin");
    let local = root.join("n.tmp");
    write(&local, "keep me");
    let Some(uroot) = unc(&root) else {
        eprintln!("skipped: admin share \\\\localhost\\<drive>$ not reachable");
        return;
    };
    let uf = uroot.join("n.tmp");
    let p = plan(&uroot, &uf);
    assert_eq!(failed(&A.trash(&p.target())), "trash_unavailable");
    assert_eq!(read(&local), "keep me");
    // Drive the Shell directly (bypassing the up-front volume check): the progress-sink guard
    // must refuse a delete that would not go to a recycle bin.
    let display = full_path(&wide(&uf)).unwrap().0;
    let out = shell::recycle(display, p.identity.volume_id.clone(), p.identity.file_id.clone());
    eprintln!("shell outcome on network path: {out:?}");
    assert_eq!(read(&local), "keep me", "a network delete must never be permanent");
    assert!(out.refused.is_some(), "the guard must refuse a non-recycling delete");
}

#[test]
fn owner_process_probe() {
    let root = scratch("owner");
    let f = root.join("f");
    write(&f, "x");
    let mut p = plan(&root, &f);
    p.probes = vec!["owner_process".into()];
    p.owner = Some("example.unknown".into());
    assert!(matches!(A.check(&p.target()), LiveCheck::Unknown(_)));
    p.owner = None;
    assert!(matches!(A.check(&p.target()), LiveCheck::Unknown(_)));
    let me = std::env::current_exe().unwrap();
    assert!(process_running(&me.file_name().unwrap().to_string_lossy()).unwrap());
    assert!(!process_running("cm-no-such-process-1234.exe").unwrap());
}

#[test]
fn free_space_and_quarantine_dir() {
    let root = scratch("space");
    let f = root.join("f");
    write(&f, "x");
    assert!(A.free_space(&raw(&root)).is_some_and(|n| n > 0));
    assert!(A.free_space(&raw(&f)).is_some_and(|n| n > 0));
    let p = plan(&root, &f);
    let dir = A.quarantine_dir_for(&p.target(), "op-42").expect("same-volume quarantine dir");
    let dir = os_path(units(&dir).unwrap());
    assert!(dir.ends_with("op-42"));
    let probe = dir.ancestors().find(|a| a.exists()).unwrap();
    let (vol, _) = super::super::directory::identity_of(&wide(probe)).unwrap();
    assert!(same_volume(&vol, &p.identity.volume_id));
    assert!(A.quarantine_dir_for(&p.target(), "..").is_none());
    assert!(A.quarantine_dir_for(&p.target(), "a\\b").is_none());
}

fn current_sid() -> String {
    let out = Command::new("whoami").args(["/user", "/fo", "csv", "/nh"]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    text.trim().rsplit(',').next().unwrap().trim_matches('"').to_owned()
}

fn icacls(path: &Path, args: &[&str]) {
    let out = Command::new("icacls").arg(path).args(args).output().unwrap();
    assert!(out.status.success(), "icacls {args:?}: {}", String::from_utf8_lossy(&out.stdout));
}

/// Removes the test's deny ACEs again, even when an assertion fails.
struct DenyGuard(Vec<PathBuf>, String);

impl Drop for DenyGuard {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = Command::new("icacls").arg(p).args(["/remove:d", &format!("*{}", self.1)]).output();
        }
    }
}

#[test]
fn permission_refusal_never_touches_the_file() {
    let root = scratch("denied");
    let dir = root.join("locked");
    let f = dir.join("keep.dat");
    write(&f, "protected");
    let p = plan(&root, &f);
    let sid = current_sid();
    assert!(sid.starts_with("S-1-"), "{sid}");
    let guard = DenyGuard(vec![f.clone(), dir.clone()], sid.clone());
    icacls(&dir, &["/deny", &format!("*{sid}:(DC)")]);
    // Specific right DE = DELETE only (the simple right "D" would also deny SYNCHRONIZE).
    icacls(&f, &["/deny", &format!("*{sid}:(DE)")]);

    let ok = |o: &ActionOutcome| matches!(o, ActionOutcome::Skipped { .. }) || matches!(o, ActionOutcome::Failed { code, .. } if code == "access_denied");
    assert_eq!(read(&f), "protected", "deny ACEs must not block reading");
    let outcomes = [A.delete(&p.target()), A.quarantine(&p.target(), &raw(&root.join("q")), "keep.dat"), A.trash(&p.target())];
    // The actions disabled the process's SeBackup/SeRestore, so the denial holds even when elevated.
    assert!(super::super::privileges::enabled_bypass_privileges().unwrap().is_empty());
    for o in &outcomes {
        assert!(ok(o), "permission refusal must fail or skip, got {o:?}");
        assert!(!matches!(o, ActionOutcome::Succeeded { .. }));
    }
    assert!(outcomes.iter().any(|o| matches!(o, ActionOutcome::Failed { code, .. } if code == "access_denied")), "{outcomes:?}");
    assert!(matches!(A.check(&p.target()), LiveCheck::Protected(_)));
    assert_eq!(read(&f), "protected");
    let (vol, fid) = super::super::directory::identity_of(&wide(&f)).unwrap();
    assert_eq!((vol, fid), (p.identity.volume_id.clone(), p.identity.file_id.clone()));
    assert!(!root.join("q").join("keep.dat").exists());

    drop(guard);
    std::fs::remove_dir_all(&root).unwrap();
}
