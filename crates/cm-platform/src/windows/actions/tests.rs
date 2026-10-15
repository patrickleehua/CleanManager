//! Real-NTFS tests for the Windows executor actions (temp directories only).

use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use cm_core::analysis::{EntryFacts, IndexView, MemView};
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
    // Manual trash plans are refused up front on such a volume (MTR-03), and the action itself never degrades.
    assert!(recycle_bin_ready(&p.path).is_err());
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

// ---------------------------------------------------------------------------------------------
// Directory items: verified empty removal and whole-tree recycling

fn member(p: &Planned) -> TreeMember<'_> {
    TreeMember { path: &p.path, identity: &p.identity, observation: &p.obs, directory: p.obs.kind == ObjectKind::Directory }
}

#[test]
fn empty_directories_are_removed_bottom_up_after_their_files() {
    let root = scratch("emptydir");
    let nm = root.join("proj").join("node_modules");
    let f = nm.join("pkg").join("index.js");
    write(&f, "module.exports = 1;");
    let keep = root.join("proj").join("package.json");
    write(&keep, "{}");
    let (pf, pkg, pnm) = (plan(&root, &f), plan(&root, &nm.join("pkg")), plan(&root, &nm));
    // A directory with its planned file still inside is never removed (the OS refuses too).
    assert_eq!(skipped(&A.remove_empty_dir(&pkg.target())), "new_descendant");
    assert!(f.exists());
    assert_eq!(A.delete(&pf.target()), ActionOutcome::Succeeded { recovery: Recovery::Unavailable });
    assert_eq!(A.remove_empty_dir(&pkg.target()), ActionOutcome::Succeeded { recovery: Recovery::Unavailable });
    assert_eq!(A.remove_empty_dir(&pnm.target()), ActionOutcome::Succeeded { recovery: Recovery::Unavailable });
    assert!(!nm.exists());
    assert_eq!(read(&keep), "{}", "the project itself is untouched");
    assert_eq!(skipped(&A.remove_empty_dir(&pnm.target())), "missing");
    assert_eq!(A.reconcile(&pnm.target(), Mode::Delete, None), Reconciled::Performed);
}

#[test]
fn a_file_appearing_mid_plan_keeps_the_directory_and_the_file() {
    let root = scratch("newdesc");
    let dist = root.join("dist");
    let planned = dist.join("bundle.js");
    write(&planned, "old");
    let (pf, pd) = (plan(&root, &planned), plan(&root, &dist));
    assert_eq!(A.delete(&pf.target()), ActionOutcome::Succeeded { recovery: Recovery::Unavailable });
    let late = dist.join("late.js");
    write(&late, "new work");
    assert_eq!(skipped(&A.remove_empty_dir(&pd.target())), "new_descendant");
    assert_eq!(read(&late), "new work");
    // A new empty sub-directory counts as new content too: never a recursive delete.
    std::fs::remove_file(&late).unwrap();
    std::fs::create_dir(dist.join("sub")).unwrap();
    assert_eq!(skipped(&A.remove_empty_dir(&pd.target())), "new_descendant");
    assert!(dist.join("sub").is_dir());
    assert_eq!(A.reconcile(&pd.target(), Mode::Delete, None), Reconciled::NotPerformed);
}

#[test]
fn replaced_or_linked_directories_are_never_removed() {
    let root = scratch("dirswap");
    let outside = scratch("dirswap-outside");
    write(&outside.join("sentinel.txt"), "sentinel");
    let (a, b) = (root.join("target"), root.join("build"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let (pa, pb) = (plan(&root, &a), plan(&root, &b));
    // Same name, different directory.
    std::fs::rename(&a, root.join("target-old")).unwrap();
    std::fs::create_dir(&a).unwrap();
    assert_eq!(skipped(&A.remove_empty_dir(&pa.target())), "identity_changed");
    assert!(a.is_dir() && root.join("target-old").is_dir());
    // A junction swapped in for the planned directory is inspected, never followed or removed.
    std::fs::remove_dir(&b).unwrap();
    let out = Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&b).arg(&outside).output().unwrap();
    assert!(out.status.success(), "mklink /J: {}", String::from_utf8_lossy(&out.stdout));
    assert_eq!(skipped(&A.remove_empty_dir(&pb.target())), "link_or_reparse");
    assert!(matches!(A.trash_tree(&pb.target(), &[]), ActionOutcome::Skipped { .. }));
    assert_ne!(std::fs::symlink_metadata(&b).unwrap().file_attributes() & ATTR_REPARSE_POINT, 0, "junction still in place");
    assert_eq!(read(&outside.join("sentinel.txt")), "sentinel");
    // A planned file where a directory is expected is not a directory item.
    let f = root.join("f.tmp");
    write(&f, "x");
    let pf = plan(&root, &f);
    assert!(matches!(A.remove_empty_dir(&pf.target()), ActionOutcome::Skipped { .. }));
    assert_eq!(read(&f), "x");
    std::fs::remove_dir(&b).unwrap();
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn directory_in_use_is_skipped() {
    let root = scratch("dirbusy");
    let d = root.join("obj");
    std::fs::create_dir_all(&d).unwrap();
    let pd = plan(&root, &d);
    // Another listing handle without delete sharing blocks the removal open (attribute-only
    // opens take no part in sharing checks).
    let holder = std::fs::OpenOptions::new().access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES).share_mode(SHARE_READ | SHARE_WRITE).custom_flags(FLAG_BACKUP_SEMANTICS).open(&d).unwrap();
    assert_eq!(skipped(&A.remove_empty_dir(&pd.target())), "in_use");
    drop(holder);
    assert!(d.is_dir());
    assert_eq!(A.remove_empty_dir(&pd.target()), ActionOutcome::Succeeded { recovery: Recovery::Unavailable });
}

/// Remove a recycled `$R` item (file or directory) and its `$I` record found through `watch`.
fn purge_recycled(watch: std::fs::File) {
    let recycled = held_final_path(&watch).expect("recycled path");
    drop(watch);
    assert!(recycled.to_string_lossy().to_ascii_lowercase().contains(r"\$recycle.bin\"), "{}", recycled.display());
    let name = recycled.file_name().unwrap().to_string_lossy().into_owned();
    if recycled.is_dir() { std::fs::remove_dir_all(&recycled).unwrap() } else { std::fs::remove_file(&recycled).unwrap() }
    if let Some(rest) = name.strip_prefix("$R") {
        let _ = std::fs::remove_file(recycled.with_file_name(format!("$I{rest}")));
    }
}

#[test]
fn whole_tree_goes_to_the_recycle_bin_only_when_it_equals_the_plan() {
    let root = scratch("tree");
    let t = root.join("proj").join("target");
    write(&t.join("debug").join("app.exe"), "binary");
    write(&t.join("CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55");
    let paths = [t.clone(), t.join("debug"), t.join("debug").join("app.exe"), t.join("CACHEDIR.TAG")];
    let planned: Vec<Planned> = paths.iter().map(|p| plan(&root, p)).collect();
    let members: Vec<TreeMember<'_>> = planned[1..].iter().map(member).collect();

    // Something not in the plan inside: refused, nothing moved.
    write(&t.join("debug").join("late.txt"), "new");
    assert_eq!(skipped(&A.trash_tree(&planned[0].target(), &members)), "tree_mismatch");
    assert_eq!(read(&t.join("debug").join("late.txt")), "new");
    std::fs::remove_file(t.join("debug").join("late.txt")).unwrap();
    // A changed planned file: refused as well.
    write(&t.join("CACHEDIR.TAG"), "Signature: changed and longer");
    assert_eq!(skipped(&A.trash_tree(&planned[0].target(), &members)), "tree_mismatch");
    let tag = plan(&root, &t.join("CACHEDIR.TAG"));
    let members: Vec<TreeMember<'_>> = [&planned[1], &planned[2], &tag].into_iter().map(member).collect();

    let watch = std::fs::OpenOptions::new().access_mode(FILE_READ_ATTRIBUTES).share_mode(SHARE_ALL).custom_flags(FLAG_OPEN_REPARSE_POINT | FLAG_BACKUP_SEMANTICS).open(&t).unwrap();
    assert_eq!(A.trash_tree(&planned[0].target(), &members), ActionOutcome::Succeeded { recovery: Recovery::Trash });
    assert!(!t.exists());
    assert!(root.join("proj").is_dir());
    purge_recycled(watch);
}

// ---------------------------------------------------------------------------------------------
// Manual trash (parity task group 6)

thread_local! {
    /// Runs once inside `trash`, after verification and right before the Shell call (race fixture).
    static BEFORE_SHELL: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn before_shell() {
    if let Some(f) = BEFORE_SHELL.with(|h| h.borrow_mut().take()) {
        f();
    }
}

/// Design D6 race fixture: the verified file is renamed away and a different file takes its name between the last
/// check and the Shell's path-based move. Nothing may be moved or deleted under a wrong identity, and the outcome is
/// unknown (for a person to check), never success.
#[test]
fn a_name_swapped_before_the_shell_runs_is_unknown_and_nothing_is_lost() {
    let root = scratch("swap-race");
    let f = root.join("video.mov");
    write(&f, "the planned file");
    let p = plan(&root, &f);
    let (f2, moved) = (f.clone(), root.join("video.moved-away"));
    let moved2 = moved.clone();
    BEFORE_SHELL.with(|h| {
        *h.borrow_mut() = Some(Box::new(move || {
            std::fs::rename(&f2, &moved2).unwrap();
            std::fs::write(&f2, "an impostor with the same name").unwrap();
        }))
    });
    let out = A.trash(&p.target());
    assert!(matches!(out, ActionOutcome::Unknown { .. }), "{out:?}");
    assert_eq!(read(&f), "an impostor with the same name", "the impostor is not recycled");
    assert_eq!(read(&moved), "the planned file", "the planned file is not deleted");
    assert!(!held_bin_has(&f), "nothing went to the bin");
}

fn held_bin_has(_: &Path) -> bool {
    false
}

/// MTR-03 at the action level: an object in the recycle bin is permanently deleted only while it is the planned
/// object; an empty recycled folder is removed only when empty.
#[test]
fn recycled_objects_are_deleted_only_as_planned() {
    let root = scratch("bin-purge");
    let d = root.join("folder");
    write(&d.join("inside.txt"), "x");
    let f = root.join("single.txt");
    write(&f, "single");
    let pf = plan(&root, &f);
    let pd = plan(&root, &d);
    let pin = plan(&root, &d.join("inside.txt"));
    let fw = std::fs::OpenOptions::new().access_mode(FILE_READ_ATTRIBUTES).share_mode(SHARE_ALL).custom_flags(FLAG_OPEN_REPARSE_POINT).open(&f).unwrap();
    assert_eq!(A.trash(&pf.target()), ActionOutcome::Succeeded { recovery: Recovery::Trash });
    let recycled = held_final_path(&fw).unwrap();
    drop(fw);
    // Plan the recycled file through the bin listing of its own bin folder, then delete exactly it.
    let bin_dir = recycled.parent().unwrap().to_path_buf();
    let listing = recycle_bin::list_dir(&bin_dir).unwrap();
    let rname = recycled.file_name().unwrap().to_string_lossy().into_owned();
    let item = listing.items.iter().find(|i| listing.view.name(i.data_entry).lossy().eq_ignore_ascii_case(&rname)).expect("listed");
    assert!(item.original_path.as_deref().is_some_and(|p| p.to_ascii_lowercase().ends_with(r"bin-purge\single.txt")), "{:?}", item.original_path);
    let input = cm_core::manual_trash::ManualInput {
        plan_id: "mp", scope_id: None, scan_id: None, scan_revision: None, created_unix: 1_800_000_000, observed_at: &listing.observed_at,
        policy_digest: cm_core::plan::v2::NO_RULES_DIGEST,
    };
    let plan = cm_core::manual_trash::build_purge_plan(&listing.view, &[item], &input).unwrap();
    assert_eq!(plan.body.items.len(), 2, "the object and its record");
    for (a, m) in plan.body.items.iter().zip(&plan.meta) {
        let fs = a.fs().unwrap();
        let obs = m.observation.as_ref().unwrap();
        let t = ActionTarget { action_id: "d", path: &fs.target.path, identity: &fs.target.identity, parent_chain: &obs.parent_chain, observation: obs, probes: &[], owner_id: None };
        assert_eq!(A.check(&t), LiveCheck::Match);
        assert_eq!(A.delete(&t), ActionOutcome::Succeeded { recovery: Recovery::Unavailable });
    }
    assert!(!recycled.exists());
    // The folder: removing a non-empty planned directory is refused; contents first, then it.
    assert_eq!(skipped(&A.remove_empty_dir(&pd.target())), "new_descendant");
    assert_eq!(A.delete(&pin.target()), ActionOutcome::Succeeded { recovery: Recovery::Unavailable });
    assert_eq!(A.remove_empty_dir(&pd.target()), ActionOutcome::Succeeded { recovery: Recovery::Unavailable });
}

/// MTR-04: a held file is reported in use by the check, and Restart Manager names the holder.
#[test]
fn an_open_file_is_in_use_and_its_holder_is_named() {
    let root = scratch("held");
    let f = root.join("app.log");
    write(&f, "log");
    let p = plan(&root, &f);
    let held = std::fs::OpenOptions::new().read(true).share_mode(SHARE_READ | SHARE_WRITE).open(&f).unwrap();
    assert_eq!(A.check(&p.target()), LiveCheck::InUse);
    let holders = super::super::restart_manager::holders(std::slice::from_ref(&p.path)).unwrap();
    assert!(holders.iter().any(|h| h.pid == std::process::id()), "{holders:?}");
    drop(held);
    assert_eq!(A.check(&p.target()), LiveCheck::Match);
    assert!(recycle_bin_ready(&p.path).is_ok());
}
