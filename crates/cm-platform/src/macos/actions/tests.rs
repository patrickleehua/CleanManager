//! Real-APFS tests for the macOS executor actions (fixtures under the canonical temporary directory only). Tests that
//! touch the user's real Trash run only with `CM_ALLOW_MAC_DEVICE_TESTS=1` or on a hosted CI runner
//! (`GITHUB_ACTIONS=true`); tests that attach a disk image only with `CM_ALLOW_DMG_TESTS=1` (macos.md §11).

use std::cell::RefCell;
use std::ffi::CString;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use cm_core::analysis::{EntryFacts, MemView};
use cm_core::capability::Platform;
use cm_core::model::{CloudState, IdentityQuality, ObjectKind};
use cm_core::plan::{WireIdentity, observe};

use super::trash::TrashError;
use super::*;
use crate::macos::testfs::Fixture;

const AT: &str = "2026-10-17T00:00:00Z";

/// Tests that change real per-user state (the Trash): on a person's Mac only when asked, always on a hosted runner.
pub(crate) fn device_tests_allowed() -> bool {
    std::env::var("CM_ALLOW_MAC_DEVICE_TESTS").is_ok_and(|v| v == "1") || std::env::var("GITHUB_ACTIONS").is_ok_and(|v| v == "true")
}

fn dmg_tests_allowed() -> bool {
    std::env::var_os("CM_ALLOW_DMG_TESTS").is_some()
}

fn raw(p: &Path) -> RawPath {
    RawPath::unix(p.as_os_str().as_bytes().to_vec()).unwrap()
}

fn facts(p: &Path, object_id: u64) -> EntryFacts {
    let md = std::fs::symlink_metadata(p).unwrap();
    let dir = if md.is_dir() { p } else { p.parent().unwrap() };
    let fd = Fd::open_dir(&CString::new(dir.as_os_str().as_bytes()).unwrap()).unwrap();
    EntryFacts {
        kind: if md.is_dir() { ObjectKind::Directory } else { ObjectKind::File },
        boundary: false,
        object_id,
        logical: md.is_file().then_some(md.len()),
        allocated: None,
        volume_id: ids::volume_id(&fd, md.dev() as i32).unwrap(),
        file_id: ids::file_id(md.ino()),
        quality: IdentityQuality::Strong,
        cloud: CloudState::NotCloud,
        modified_unix: Some(md.mtime()),
        link_count: md.is_file().then_some(md.nlink() as u32),
        skipped_here: 0,
    }
}

/// A planned object: identity and CM-OBS-v1 observation built from the live tree via `observe`, as the scanner does.
struct Planned {
    path: RawPath,
    identity: WireIdentity,
    obs: ObservationRecord,
    chain: Vec<Identity>,
    probes: Vec<String>,
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
            owner_id: None,
        }
    }
}

fn plan(root: &Path, item: &Path) -> Planned {
    let rel = item.strip_prefix(root).unwrap();
    let mut view = MemView { platform: Some(Platform::Macos), ..Default::default() };
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
    assert_eq!(obs.path, raw(item));
    let f = &view.facts[&id];
    let identity = WireIdentity { volume_id: f.volume_id.clone(), file_id: f.file_id.clone(), generation: None, quality: "strong".into(), observed_at: AT.into() };
    Planned { path: raw(item), identity, chain: obs.parent_chain.clone(), obs, probes: Vec::new() }
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

/// Fresh actions (and so a fresh open-descriptor snapshot) for each use.
fn a() -> MacActions {
    MacActions::default()
}

fn chmod(p: &Path, mode: u32) {
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn chflags(p: &Path, flags: u32) {
    let c = CString::new(p.as_os_str().as_bytes()).unwrap();
    // SAFETY: NUL-terminated path; user flags only (no privilege needed for the owner).
    assert_eq!(unsafe { libc::lchflags(c.as_ptr(), flags) }, 0, "chflags {}: {}", p.display(), std::io::Error::last_os_error());
}

/// Clears user flags again, even when an assertion fails (the fixture cannot remove a locked file).
struct Unlock(PathBuf);

impl Drop for Unlock {
    fn drop(&mut self) {
        if let Ok(c) = CString::new(self.0.as_os_str().as_bytes()) {
            // SAFETY: NUL-terminated path.
            unsafe { libc::lchflags(c.as_ptr(), 0) };
        }
    }
}

/// A child process of this user holding `path` open on its standard input.
struct Holder(Child);

impl Holder {
    fn of(path: &Path) -> Holder {
        let f = std::fs::File::open(path).unwrap();
        Holder(Command::new("/bin/sleep").arg("120").stdin(f).stdout(Stdio::null()).spawn().unwrap())
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

// ---------------------------------------------------------------------------------------------
// Pure helpers

#[test]
fn scope_split_keeps_the_root_and_rejects_short_paths() {
    let s = split_scope(b"/a/b/c/d.txt", 2).unwrap();
    assert_eq!(s, ScopeSplit { root: b"/a/b".to_vec(), below: vec![b"c".to_vec()], name: b"d.txt".to_vec() });
    let s = split_scope(b"//f", 1).unwrap();
    assert_eq!(s, ScopeSplit { root: b"/".to_vec(), below: vec![], name: b"f".to_vec() });
    assert_eq!(split_scope(b"/a/b", 3).unwrap_err(), Stop::Parent);
    assert_eq!(split_scope(b"/a/b", 0).unwrap_err(), Stop::Parent);
    assert!(matches!(split_scope(b"a/b", 1), Err(Stop::Unsupported(_))));
    assert!(matches!(split_scope(b"/a/../b", 1), Err(Stop::Error(_))));
    assert_eq!(split_last(b"/a/b/c"), Some((b"/a/b".to_vec(), b"c".to_vec())));
    assert_eq!(split_last(b"/c"), Some((b"/".to_vec(), b"c".to_vec())));
    assert_eq!(split_last(b"/"), None);
    assert!(valid_component(b"op-1") && !valid_component(b"..") && !valid_component(b"a/b") && !valid_component(b""));
    assert!(!valid_component(&[b'x'; 256]));
}

fn stat_with(mode: libc::mode_t, uid: u32) -> libc::stat {
    // SAFETY: plain-data structure; zero is a valid bit pattern.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    st.st_mode = libc::S_IFDIR | mode;
    st.st_uid = uid;
    st
}

#[test]
fn namespace_gate_trusts_only_private_directories_of_this_user_or_root() {
    let me = 501;
    let ok = || Ok(false);
    assert_eq!(namespace_gate(&stat_with(0o755, me), 0, me, ok), Ok(()));
    assert_eq!(namespace_gate(&stat_with(0o755, 0), 0, me, ok), Ok(()));
    assert!(namespace_gate(&stat_with(0o755, 502), 0, me, ok).is_err(), "another user's directory");
    assert!(namespace_gate(&stat_with(0o775, me), 0, me, ok).is_err(), "group-writable");
    assert!(namespace_gate(&stat_with(0o777, me), 0, me, ok).is_err(), "world-writable");
    assert!(namespace_gate(&stat_with(0o1700, me), 0, me, ok).is_err(), "sticky");
    assert!(namespace_gate(&stat_with(0o700, me), libc::MNT_IGNORE_OWNERSHIP as u32, me, ok).is_err(), "ownership ignored");
    assert!(namespace_gate(&stat_with(0o700, me), 0, me, || Ok(true)).is_err(), "allowing ACL entry");
    assert!(namespace_gate(&stat_with(0o700, me), 0, me, || Err("unreadable".into())).is_err(), "unreadable ACL");
}

#[test]
fn cloud_placeholders_and_locked_objects_are_classified_before_acting() {
    let fx = Fixture::new("act-flags");
    let f = fx.write("f.bin", b"x");
    let p = plan(fx.path(), &f);
    let st = std::fs::symlink_metadata(&f).unwrap();
    // SAFETY: plain-data structure; zero is a valid bit pattern.
    let mut live: libc::stat = unsafe { std::mem::zeroed() };
    live.st_mode = libc::S_IFREG | 0o644;
    live.st_dev = st.dev() as i32;
    live.st_ino = st.ino();
    live.st_nlink = 1;
    live.st_size = 1;
    live.st_mtime = st.mtime();
    let vol = p.identity.volume_id.clone();
    assert_eq!(verify_live(&p.target(), &live, live.st_dev, &vol, false), Ok(()));
    for (flags, want) in [(SF_DATALESS, "unsupported"), (libc::UF_IMMUTABLE, "protected"), (libc::SF_IMMUTABLE, "protected"), (SF_RESTRICTED, "protected"), (SF_NOUNLINK, "protected")] {
        live.st_flags = flags;
        let stop = verify_live(&p.target(), &live, live.st_dev, &vol, false).unwrap_err();
        assert_eq!(skipped(&stop.clone().outcome()), want, "{flags:#x}");
        assert!(!matches!(stop.check(), LiveCheck::Match));
    }
    live.st_flags = 0;
    live.st_nlink = 2;
    assert_eq!(verify_live(&p.target(), &live, live.st_dev, &vol, false), Err(Stop::HardLinks(2)));
    live.st_nlink = 1;
    assert_eq!(verify_live(&p.target(), &live, live.st_dev + 1, &vol, false), Err(Stop::Identity), "another device at the name");
    live.st_mode = libc::S_IFLNK | 0o755;
    assert_eq!(verify_live(&p.target(), &live, live.st_dev, &vol, false), Err(Stop::Reparse));
}

#[test]
fn race_safety_unavailable_is_a_skip_with_its_own_code() {
    let o = Stop::Gate("x".into()).outcome();
    assert_eq!(skipped(&o), "race_safety_unavailable");
    assert!(matches!(Stop::Gate("x".into()).check(), LiveCheck::Unsupported(m) if m.starts_with("race_safety_unavailable")));
}

#[test]
fn trash_errors_map_to_their_codes_and_never_to_success() {
    let e = |domain: &str, code: isize| TrashError { domain: domain.into(), code, message: "m".into() };
    assert_eq!(e("NSCocoaErrorDomain", 3328).outcome_code(), "trash_unavailable");
    assert!(e("NSCocoaErrorDomain", 3328).describe().contains("no Trash"));
    assert_eq!(e("NSCocoaErrorDomain", 513).outcome_code(), "access_denied");
    assert_eq!(e("NSPOSIXErrorDomain", libc::EPERM as isize).outcome_code(), "access_denied");
    assert_eq!(e("NSCocoaErrorDomain", 4).outcome_code(), "trash_unavailable");
}

// ---------------------------------------------------------------------------------------------
// Verification and in-place refusals

#[test]
fn identity_matches_the_scanner_format() {
    let fx = Fixture::new("act-identity");
    let f = fx.write("f.txt", b"x");
    let p = plan(fx.path(), &f);
    assert_eq!(p.chain.len(), 1, "immediate parent is the scope root");
    assert_eq!(a().check(&p.target()), LiveCheck::Match);
    assert!(at_path(&raw(&f).raw_bytes(), &p.identity.volume_id, &p.identity.file_id) == AtPath::Planned);
}

#[test]
fn delete_removes_exactly_the_verified_file() {
    let fx = Fixture::new("act-delete");
    let f = fx.write("d/target.tmp", b"target");
    let keep = fx.write("d/keep.tmp", b"keep");
    let p = plan(fx.path(), &f);
    assert_eq!(p.chain.len(), 2);
    assert_eq!(a().check(&p.target()), LiveCheck::Match);
    assert_eq!(a().reconcile(&p.target(), Mode::Delete, None), Reconciled::NotPerformed);
    assert_eq!(a().delete(&p.target()), ActionOutcome::Succeeded { recovery: Recovery::Unavailable });
    assert!(!f.exists());
    assert_eq!(read(&keep), "keep");
    assert_eq!(a().check(&p.target()), LiveCheck::Missing);
    assert_eq!(a().reconcile(&p.target(), Mode::Delete, None), Reconciled::Performed);
    assert_eq!(skipped(&a().delete(&p.target())), "missing");
}

#[test]
fn replaced_file_is_skipped_and_the_replacement_survives() {
    let fx = Fixture::new("act-swap");
    let f = fx.write("cache.bin", b"planned");
    let p = plan(fx.path(), &f);
    std::fs::rename(&f, fx.path().join("moved.bin")).unwrap();
    write(&f, "replace"); // same name, same size
    assert_eq!(a().check(&p.target()), LiveCheck::IdentityChanged);
    assert_eq!(skipped(&a().delete(&p.target())), "identity_changed");
    assert_eq!(skipped(&a().quarantine(&p.target(), &raw(&fx.path().join("q")), "x")), "identity_changed");
    assert_eq!(skipped(&a().trash(&p.target())), "identity_changed");
    assert_eq!(read(&f), "replace");
    assert_eq!(read(&fx.path().join("moved.bin")), "planned");
    assert_eq!(a().reconcile(&p.target(), Mode::Delete, None), Reconciled::StillUnknown);
}

#[test]
fn links_are_never_followed_at_the_target_or_on_the_parent_chain() {
    let fx = Fixture::new("act-links");
    let outside = Fixture::new("act-links-outside");
    let sentinel = outside.write("sentinel.txt", b"sentinel");

    // Symlink swapped in for a planned file.
    let s = fx.write("s.tmp", b"x");
    let ps = plan(fx.path(), &s);
    std::fs::remove_file(&s).unwrap();
    std::os::unix::fs::symlink(&sentinel, &s).unwrap();
    assert_eq!(a().check(&ps.target()), LiveCheck::LinkOrReparse);
    assert_eq!(skipped(&a().delete(&ps.target())), "link_or_reparse");
    assert_eq!(skipped(&a().quarantine(&ps.target(), &raw(&fx.path().join("q")), "x")), "link_or_reparse");
    assert_eq!(skipped(&a().trash(&ps.target())), "link_or_reparse");
    assert!(std::fs::symlink_metadata(&s).unwrap().file_type().is_symlink(), "link still in place");

    // Symlink swapped in for a planned parent directory (pointing at a directory with the same file name).
    let f = fx.write("d/f.tmp", b"x");
    let pf = plan(fx.path(), &f);
    std::fs::rename(fx.path().join("d"), fx.path().join("d-old")).unwrap();
    outside.write("f.tmp", b"x");
    std::os::unix::fs::symlink(outside.path(), fx.path().join("d")).unwrap();
    assert_eq!(a().check(&pf.target()), LiveCheck::ParentChanged);
    assert_eq!(skipped(&a().delete(&pf.target())), "parent_changed");
    assert_eq!(read(&sentinel), "sentinel");
    assert_eq!(read(&outside.path().join("f.tmp")), "x");
}

#[test]
fn replaced_parent_is_detected_even_when_the_file_moved_along() {
    let fx = Fixture::new("act-parent");
    let f = fx.write("d/f.tmp", b"x");
    let p = plan(fx.path(), &f);
    std::fs::rename(fx.path().join("d"), fx.path().join("d-old")).unwrap();
    std::fs::create_dir(fx.path().join("d")).unwrap();
    std::fs::rename(fx.path().join("d-old/f.tmp"), &f).unwrap();
    assert_eq!(a().check(&p.target()), LiveCheck::ParentChanged);
    assert_eq!(skipped(&a().delete(&p.target())), "parent_changed");
    assert!(f.exists());
}

#[test]
fn changed_metadata_is_stale() {
    let fx = Fixture::new("act-meta");
    let f = fx.write("grow.log", b"1");
    let p = plan(fx.path(), &f);
    write(&f, "12345");
    assert!(matches!(a().check(&p.target()), LiveCheck::MetadataChanged(_)));
    assert_eq!(skipped(&a().delete(&p.target())), "metadata_changed");
    assert_eq!(read(&f), "12345");
    // A second hard link appearing after the plan is refused as well.
    let g = fx.write("g.log", b"1");
    let pg = plan(fx.path(), &g);
    std::fs::hard_link(&g, fx.path().join("g-link.log")).unwrap();
    assert!(!matches!(a().check(&pg.target()), LiveCheck::Match));
    assert!(matches!(a().delete(&pg.target()), ActionOutcome::Skipped { .. }));
    assert!(g.exists());
}

#[test]
fn file_held_open_by_another_process_is_in_use() {
    let fx = Fixture::new("act-busy");
    let f = fx.write("locked.log", b"busy");
    let p = plan(fx.path(), &f);
    let holder = Holder::of(&f);
    assert_eq!(a().check(&p.target()), LiveCheck::InUse);
    assert_eq!(skipped(&a().delete(&p.target())), "in_use");
    assert_eq!(skipped(&a().quarantine(&p.target(), &raw(&fx.path().join("q")), "x")), "in_use");
    assert_eq!(skipped(&a().trash(&p.target())), "in_use");
    drop(holder);
    assert_eq!(read(&f), "busy");
    assert_eq!(a().check(&p.target()), LiveCheck::Match);
}

#[test]
fn untrusted_namespaces_are_race_safety_unavailable_and_untouched() {
    for (mode, why) in [(0o777, "world-writable"), (0o775, "group-writable"), (0o1755, "sticky")] {
        let fx = Fixture::new("act-gate");
        let f = fx.write("shared/f.tmp", b"keep");
        let p = plan(fx.path(), &f);
        chmod(&fx.path().join("shared"), mode);
        let check = a().check(&p.target());
        assert!(matches!(&check, LiveCheck::Unsupported(m) if m.starts_with("race_safety_unavailable")), "{why}: {check:?}");
        assert_eq!(skipped(&a().delete(&p.target())), "race_safety_unavailable", "{why}");
        assert_eq!(skipped(&a().quarantine(&p.target(), &raw(&fx.path().join("q")), "f")), "race_safety_unavailable", "{why}");
        assert_eq!(skipped(&a().trash(&p.target())), "race_safety_unavailable", "{why}");
        assert_eq!(read(&f), "keep");
        assert!(!fx.path().join("q/f").exists());
    }
    // The scope root itself counts too.
    let fx = Fixture::new("act-gate-root");
    let f = fx.write("f.tmp", b"keep");
    let p = plan(fx.path(), &f);
    chmod(fx.path(), 0o777);
    assert_eq!(skipped(&a().delete(&p.target())), "race_safety_unavailable");
    chmod(fx.path(), 0o755);
    assert_eq!(a().check(&p.target()), LiveCheck::Match);
}

fn chmod_acl(p: &Path, args: &[&str]) {
    let out = Command::new("/bin/chmod").args(args).arg(p).output().unwrap();
    assert!(out.status.success(), "chmod {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn acl_entries_letting_others_change_a_directory_are_untrusted() {
    let fx = Fixture::new("act-acl");
    let f = fx.write("d/f.tmp", b"keep");
    let p = plan(fx.path(), &f);
    let d = fx.path().join("d");
    /// Removes the test's ACL again, even when an assertion fails (a deny-delete entry would keep the fixture).
    struct ClearAcl(PathBuf);
    impl Drop for ClearAcl {
        fn drop(&mut self) {
            let _ = Command::new("/bin/chmod").arg("-N").arg(&self.0).status();
        }
    }
    let _clear = ClearAcl(d.clone());
    // A deny entry (as on a home folder) and an allow entry for this user change nothing.
    chmod_acl(&d, &["+a", "everyone deny delete"]);
    let me = String::from_utf8(Command::new("/usr/bin/id").arg("-un").output().unwrap().stdout).unwrap();
    chmod_acl(&d, &["+a", &format!("user:{} allow add_file,delete_child", me.trim())]);
    assert_eq!(a().check(&p.target()), LiveCheck::Match);
    // An allow entry that lets everyone add or remove entries makes the namespace untrusted.
    chmod_acl(&d, &["+a", "everyone allow add_file,delete_child"]);
    assert!(matches!(a().check(&p.target()), LiveCheck::Unsupported(m) if m.starts_with("race_safety_unavailable")));
    assert_eq!(skipped(&a().delete(&p.target())), "race_safety_unavailable");
    assert_eq!(read(&f), "keep");
    chmod_acl(&d, &["-N"]);
    assert_eq!(a().check(&p.target()), LiveCheck::Match);
}

#[test]
fn locked_files_and_locked_parents_are_protected_and_untouched() {
    let fx = Fixture::new("act-uchg");
    let f = fx.write("locked.bin", b"locked");
    let p = plan(fx.path(), &f);
    let _unlock = Unlock(f.clone());
    chflags(&f, libc::UF_IMMUTABLE);
    assert!(matches!(a().check(&p.target()), LiveCheck::Protected(_)));
    assert_eq!(skipped(&a().delete(&p.target())), "protected");
    assert_eq!(skipped(&a().quarantine(&p.target(), &raw(&fx.path().join("q")), "x")), "protected");
    assert_eq!(skipped(&a().trash(&p.target())), "protected");
    assert_eq!(read(&f), "locked");
    chflags(&f, 0);
    assert_eq!(a().check(&p.target()), LiveCheck::Match);

    // A locked parent directory: its entries cannot be renamed or removed either.
    let g = fx.write("d/inner.bin", b"inner");
    let pg = plan(fx.path(), &g);
    let d = fx.path().join("d");
    let _unlock_dir = Unlock(d.clone());
    chflags(&d, libc::UF_IMMUTABLE);
    assert!(matches!(a().check(&pg.target()), LiveCheck::Protected(_)));
    assert_eq!(skipped(&a().delete(&pg.target())), "protected");
    assert_eq!(read(&g), "inner");
    chflags(&d, 0);
}

#[test]
fn probes_without_a_macos_answer_are_never_assumed_to_pass() {
    let fx = Fixture::new("act-probe");
    let f = fx.write("f", b"x");
    let mut p = plan(fx.path(), &f);
    p.probes = vec!["open_file_handle".into()];
    assert_eq!(a().check(&p.target()), LiveCheck::Match, "answered by the in-use check");
    p.probes = vec!["owner_process".into()];
    assert!(matches!(a().check(&p.target()), LiveCheck::Unknown(_)));
}

// ---------------------------------------------------------------------------------------------
// Quarantine

thread_local! {
    /// Runs once inside `quarantine`, after verification and right before the rename (race fixture).
    static BEFORE_RENAME: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    /// Runs once inside `trash`, after the last look at the name and right before the system call (race fixture).
    static BEFORE_TRASH: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

pub(super) fn before_rename() {
    if let Some(f) = BEFORE_RENAME.with(|h| h.borrow_mut().take()) {
        f();
    }
}

pub(super) fn before_trash() {
    if let Some(f) = BEFORE_TRASH.with(|h| h.borrow_mut().take()) {
        f();
    }
}

#[test]
fn quarantine_moves_on_the_same_volume() {
    let fx = Fixture::new("act-quarantine");
    let f = fx.write("d/old.cache", b"payload");
    let p = plan(fx.path(), &f);
    let q = fx.path().join("store/quarantine/op1");
    let out = a().quarantine(&p.target(), &raw(&q), "0001-old.cache");
    let dest = q.join("0001-old.cache");
    assert_eq!(out, ActionOutcome::Succeeded { recovery: Recovery::Quarantine { location: raw(&dest) } });
    assert!(!f.exists());
    assert_eq!(read(&dest), "payload");
    assert_eq!(std::fs::metadata(&q).unwrap().permissions().mode() & 0o777, 0o700, "private quarantine");
    let md = std::fs::metadata(&dest).unwrap();
    assert_eq!(ids::file_id(md.ino()), p.identity.file_id);
    assert_eq!(a().reconcile(&p.target(), Mode::Quarantine, Some(&raw(&dest))), Reconciled::Performed);
    assert_eq!(a().reconcile(&p.target(), Mode::Quarantine, None), Reconciled::StillUnknown);
}

#[test]
fn quarantine_never_replaces_an_existing_name() {
    let fx = Fixture::new("act-noreplace");
    let f = fx.write("a.tmp", b"planned");
    let p = plan(fx.path(), &f);
    let q = fx.path().join("q");
    std::fs::DirBuilder::new().mode(0o700).create(&q).unwrap();
    write(&q.join("taken"), "existing");
    assert_eq!(failed(&a().quarantine(&p.target(), &raw(&q), "taken")), "destination_exists");
    assert_eq!(read(&f), "planned");
    assert_eq!(read(&q.join("taken")), "existing");
    assert_eq!(a().reconcile(&p.target(), Mode::Quarantine, Some(&raw(&q.join("taken")))), Reconciled::NotPerformed);
    assert_eq!(failed(&a().quarantine(&p.target(), &raw(&q), "../escape")), "invalid_name");
    // A quarantine directory others can write to is refused before anything moves.
    chmod(&q, 0o777);
    assert_eq!(failed(&a().quarantine(&p.target(), &raw(&q), "fresh")), "quarantine_unsafe");
    // A link in place of the quarantine directory is never followed.
    let elsewhere = Fixture::new("act-noreplace-elsewhere");
    let lq = fx.path().join("lq");
    std::os::unix::fs::symlink(elsewhere.path(), &lq).unwrap();
    assert!(matches!(a().quarantine(&p.target(), &raw(&lq), "fresh"), ActionOutcome::Failed { .. }));
    assert_eq!(std::fs::read_dir(elsewhere.path()).unwrap().count(), 0);
    assert_eq!(read(&f), "planned");
}

/// EXE-02 / design D6 race fixture for quarantine: the verified file is renamed away and another file takes its name
/// between the last check and the rename. The impostor lands in quarantine, the outcome is unknown with put-back
/// guidance, nothing is moved back automatically and nothing is deleted.
#[test]
fn a_name_swapped_before_the_rename_is_unknown_and_nothing_is_lost() {
    let fx = Fixture::new("act-qrace");
    let f = fx.write("video.mov", b"the planned file");
    let p = plan(fx.path(), &f);
    let (f2, moved) = (f.clone(), fx.path().join("video.moved-away"));
    let moved2 = moved.clone();
    BEFORE_RENAME.with(|h| {
        *h.borrow_mut() = Some(Box::new(move || {
            std::fs::rename(&f2, &moved2).unwrap();
            std::fs::write(&f2, "an impostor with the same name").unwrap();
        }))
    });
    let q = fx.path().join("q");
    let out = a().quarantine(&p.target(), &raw(&q), "0001-video.mov");
    let ActionOutcome::Unknown { message } = &out else { panic!("{out:?}") };
    assert!(message.contains("move it back"), "{message}");
    assert_eq!(read(&q.join("0001-video.mov")), "an impostor with the same name", "not moved back automatically");
    assert_eq!(read(&moved), "the planned file", "the planned file is not touched");
    assert!(!f.exists());
    assert_eq!(a().reconcile(&p.target(), Mode::Quarantine, Some(&raw(&q.join("0001-video.mov")))), Reconciled::StillUnknown);
}

#[test]
fn quarantine_refuses_another_volume() {
    if !dmg_tests_allowed() {
        eprintln!("skipped: set CM_ALLOW_DMG_TESTS=1 to attach a disposable disk image");
        return;
    }
    let fx = Fixture::new("act-xvol");
    let f = fx.write("a.tmp", b"planned");
    let p = plan(fx.path(), &f);
    let mnt = fx.mkdir("mnt");
    let image = fx.path().join("x.dmg");
    let ok = Command::new("hdiutil").args(["create", "-quiet", "-size", "8m", "-fs", "APFS", "-volname", "cmxvol", "-o"]).arg(&image).status().unwrap();
    assert!(ok.success());
    let ok = Command::new("hdiutil").args(["attach", "-quiet", "-nobrowse", "-owners", "on", "-mountpoint"]).arg(&mnt).arg(&image).status().unwrap();
    assert!(ok.success());
    struct Detach(PathBuf);
    impl Drop for Detach {
        fn drop(&mut self) {
            let _ = Command::new("hdiutil").args(["detach", "-quiet", "-force"]).arg(&self.0).status();
        }
    }
    let _detach = Detach(mnt.clone());
    let q = mnt.join("q");
    assert_eq!(failed(&a().quarantine(&p.target(), &raw(&q), "a.tmp")), "cross_volume");
    assert_eq!(read(&f), "planned");
    assert!(!q.join("a.tmp").exists());
    // A file on the image gets a quarantine on that same volume, private to this user.
    let g = mnt.join("g.tmp");
    write(&g, "on the image");
    let pg = plan(&mnt, &g);
    let dir = MacActions::with_store(fx.path().join("store")).quarantine_dir_for(&pg.target(), "op-9").expect("same-volume quarantine on the image");
    let dir = os_path(unix_bytes(&dir).unwrap());
    assert!(dir.starts_with(&mnt), "{}", dir.display());
    assert_eq!(std::fs::metadata(dir.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
}

#[test]
fn free_space_and_quarantine_dir() {
    let fx = Fixture::new("act-space");
    let f = fx.write("f", b"x");
    assert!(a().free_space(&raw(fx.path())).is_some_and(|n| n > 0));
    assert!(a().free_space(&raw(&f)).is_some_and(|n| n > 0));
    assert!(a().free_space(&raw(&fx.path().join("gone/already"))).is_some_and(|n| n > 0), "nearest existing ancestor");
    let p = plan(fx.path(), &f);
    let store = fx.path().join("store");
    let actions = MacActions::with_store(store.clone());
    let dir = actions.quarantine_dir_for(&p.target(), "op-42").expect("same-volume quarantine dir");
    assert_eq!(dir, raw(&store.join("quarantine/op-42")));
    assert!(actions.quarantine_dir_for(&p.target(), "..").is_none());
    assert!(actions.quarantine_dir_for(&p.target(), "a/b").is_none());
}

// ---------------------------------------------------------------------------------------------
// Directory items: verified empty removal

fn planned_dir(root: &Path, d: &Path) -> Planned {
    plan(root, d)
}

#[test]
fn empty_directories_are_removed_bottom_up_after_their_files() {
    let fx = Fixture::new("act-emptydir");
    let nm = fx.path().join("proj/node_modules");
    let f = fx.write("proj/node_modules/pkg/index.js", b"module.exports = 1;");
    let keep = fx.write("proj/package.json", b"{}");
    let (pf, pkg, pnm) = (plan(fx.path(), &f), planned_dir(fx.path(), &nm.join("pkg")), planned_dir(fx.path(), &nm));
    assert_eq!(skipped(&a().remove_empty_dir(&pkg.target())), "new_descendant");
    assert!(f.exists());
    assert_eq!(a().delete(&pf.target()), ActionOutcome::Succeeded { recovery: Recovery::Unavailable });
    assert_eq!(a().remove_empty_dir(&pkg.target()), ActionOutcome::Succeeded { recovery: Recovery::Unavailable });
    assert_eq!(a().remove_empty_dir(&pnm.target()), ActionOutcome::Succeeded { recovery: Recovery::Unavailable });
    assert!(!nm.exists());
    assert_eq!(read(&keep), "{}", "the project itself is untouched");
    assert_eq!(skipped(&a().remove_empty_dir(&pnm.target())), "missing");
    assert_eq!(a().reconcile(&pnm.target(), Mode::Delete, None), Reconciled::Performed);
}

#[test]
fn a_file_appearing_mid_plan_keeps_the_directory_and_the_file() {
    let fx = Fixture::new("act-newdesc");
    let dist = fx.path().join("dist");
    let planned = fx.write("dist/bundle.js", b"old");
    let (pf, pd) = (plan(fx.path(), &planned), planned_dir(fx.path(), &dist));
    assert_eq!(a().delete(&pf.target()), ActionOutcome::Succeeded { recovery: Recovery::Unavailable });
    let late = fx.write("dist/late.js", b"new work");
    assert_eq!(skipped(&a().remove_empty_dir(&pd.target())), "new_descendant");
    assert_eq!(read(&late), "new work");
    std::fs::remove_file(&late).unwrap();
    std::fs::create_dir(dist.join("sub")).unwrap();
    assert_eq!(skipped(&a().remove_empty_dir(&pd.target())), "new_descendant");
    assert!(dist.join("sub").is_dir());
    assert_eq!(a().reconcile(&pd.target(), Mode::Delete, None), Reconciled::NotPerformed);
}

#[test]
fn replaced_linked_or_held_directories_are_never_removed() {
    let fx = Fixture::new("act-dirswap");
    let outside = Fixture::new("act-dirswap-outside");
    outside.write("sentinel.txt", b"sentinel");
    let (x, y, z) = (fx.mkdir("target"), fx.mkdir("build"), fx.mkdir("obj"));
    let (px, py, pz) = (planned_dir(fx.path(), &x), planned_dir(fx.path(), &y), planned_dir(fx.path(), &z));
    std::fs::rename(&x, fx.path().join("target-old")).unwrap();
    std::fs::create_dir(&x).unwrap();
    assert_eq!(skipped(&a().remove_empty_dir(&px.target())), "identity_changed");
    assert!(x.is_dir() && fx.path().join("target-old").is_dir());
    std::fs::remove_dir(&y).unwrap();
    std::os::unix::fs::symlink(outside.path(), &y).unwrap();
    assert_eq!(skipped(&a().remove_empty_dir(&py.target())), "link_or_reparse");
    assert!(matches!(a().trash_tree(&py.target(), &[]), ActionOutcome::Skipped { .. }));
    assert_eq!(read(&outside.path().join("sentinel.txt")), "sentinel");
    // A process whose working directory is the planned directory keeps it.
    let mut child = Command::new("/bin/sleep").arg("120").current_dir(&z).spawn().unwrap();
    let held = a().remove_empty_dir(&pz.target());
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(skipped(&held), "in_use");
    assert!(z.is_dir());
    assert_eq!(a().remove_empty_dir(&pz.target()), ActionOutcome::Succeeded { recovery: Recovery::Unavailable });
    // A planned file where a directory is expected is not a directory item.
    let f = fx.write("f.tmp", b"x");
    let pf = plan(fx.path(), &f);
    assert!(matches!(a().remove_empty_dir(&pf.target()), ActionOutcome::Skipped { .. }));
    assert_eq!(read(&f), "x");
}

// ---------------------------------------------------------------------------------------------
// Trash (the user's real Trash: device tests only)

/// Removes what a test moved to the Trash, found through a descriptor held on it (never by listing the Trash).
struct TrashGuard(Vec<std::fs::File>);

impl Drop for TrashGuard {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        for f in &self.0 {
            match trash::fd_path(f.as_raw_fd()) {
                Some(p) => {
                    if let Err(e) = std::fs::remove_file(os_path(&p)) {
                        eprintln!("could not remove {} from the Trash: {e}", String::from_utf8_lossy(&p));
                    }
                }
                None => eprintln!("could not locate a test item in the Trash"),
            }
        }
    }
}

fn watch(p: &Path) -> std::fs::File {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().read(true).custom_flags(libc::O_EVTONLY | libc::O_NOFOLLOW).open(p).unwrap()
}

#[test]
fn trash_moves_the_verified_file_to_the_trash() {
    if !device_tests_allowed() {
        eprintln!("skipped: set CM_ALLOW_MAC_DEVICE_TESTS=1 to move a test file to your Trash");
        return;
    }
    let fx = Fixture::new("act-trash");
    let f = fx.write("trash-me.tmp", b"to the Trash");
    let keep = fx.write("keep.tmp", b"keep");
    let p = plan(fx.path(), &f);
    let w = watch(&f);
    let out = a().trash(&p.target());
    let guard = TrashGuard(vec![w]);
    assert_eq!(out, ActionOutcome::Succeeded { recovery: Recovery::Trash });
    assert!(!f.exists());
    assert_eq!(read(&keep), "keep");
    {
        use std::os::fd::AsRawFd;
        let now = trash::fd_path(guard.0[0].as_raw_fd()).expect("held path");
        assert!(String::from_utf8_lossy(&now).contains("/.Trash"), "{}", String::from_utf8_lossy(&now));
    }
    assert_eq!(a().reconcile(&p.target(), Mode::Trash, None), Reconciled::StillUnknown);
}

/// Parity 6.2 / design D6 race fixture: the verified file is renamed away and an impostor takes its name between the
/// last look and the system's path-based move. The impostor is what reaches the Trash (recoverable there); the
/// planned file is not deleted; the outcome is unknown with put-back guidance, never success.
#[test]
fn a_name_swapped_before_the_trash_call_is_unknown_and_nothing_is_lost() {
    if !device_tests_allowed() {
        eprintln!("skipped: set CM_ALLOW_MAC_DEVICE_TESTS=1 to move a test file to your Trash");
        return;
    }
    let fx = Fixture::new("act-trashrace");
    let f = fx.write("video.mov", b"the planned file");
    let p = plan(fx.path(), &f);
    let (f2, moved) = (f.clone(), fx.path().join("video.moved-away"));
    let moved2 = moved.clone();
    let impostor: std::rc::Rc<RefCell<Option<std::fs::File>>> = std::rc::Rc::default();
    let slot = std::rc::Rc::clone(&impostor);
    BEFORE_TRASH.with(|h| {
        *h.borrow_mut() = Some(Box::new(move || {
            std::fs::rename(&f2, &moved2).unwrap();
            std::fs::write(&f2, "an impostor with the same name").unwrap();
            *slot.borrow_mut() = Some(watch(&f2));
        }))
    });
    let out = a().trash(&p.target());
    let _guard = TrashGuard(impostor.borrow_mut().take().into_iter().collect());
    let ActionOutcome::Unknown { message } = &out else { panic!("{out:?}") };
    assert!(message.contains("Put Back"), "{message}");
    assert_eq!(read(&moved), "the planned file", "the planned file is not deleted or moved");
    assert!(!f.exists(), "the impostor went to the Trash, where it can be put back");
}

#[test]
fn trash_on_a_volume_without_a_trash_never_deletes() {
    if !dmg_tests_allowed() {
        eprintln!("skipped: set CM_ALLOW_DMG_TESTS=1 to attach a disposable disk image");
        return;
    }
    // A read-only image: the system cannot move anything to a Trash there.
    let fx = Fixture::new("act-rotrash");
    let src = fx.mkdir("src");
    write(&src.join("keep.bin"), "keep me");
    let image = fx.path().join("ro.dmg");
    let ok = Command::new("hdiutil").args(["create", "-quiet", "-fs", "HFS+", "-format", "UDRO", "-srcfolder"]).arg(&src).arg("-o").arg(&image).status().unwrap();
    assert!(ok.success());
    let mnt = fx.mkdir("mnt");
    let ok = Command::new("hdiutil").args(["attach", "-quiet", "-nobrowse", "-readonly", "-mountpoint"]).arg(&mnt).arg(&image).status().unwrap();
    assert!(ok.success());
    struct Detach(PathBuf);
    impl Drop for Detach {
        fn drop(&mut self) {
            let _ = Command::new("hdiutil").args(["detach", "-quiet", "-force"]).arg(&self.0).status();
        }
    }
    let _detach = Detach(mnt.clone());
    let f = mnt.join("keep.bin");
    let p = plan(&mnt, &f);
    let out = a().trash(&p.target());
    assert!(matches!(&out, ActionOutcome::Failed { .. } | ActionOutcome::Skipped { .. }), "{out:?}");
    assert_eq!(read(&f), "keep me");
}

/// The Trash while macOS shows its privacy prompt for it: the first open does not return, so after the deadline the
/// listing is `PromptPending` (never an empty Trash); answered in time it lists as usual, and a denial is
/// `PermissionRequired`.
#[test]
fn a_trash_held_by_a_privacy_prompt_is_pending_not_empty() {
    use std::time::{Duration, Instant};
    use trash::{ListError, list_trash_dir_gated};
    let fx = Fixture::new("trash-prompt");
    let bin = fx.mkdir(".Trash");
    fx.write(".Trash/old.txt", b"old");

    let listed = list_trash_dir_gated(&bin, Duration::from_secs(5), |c| {
        std::thread::sleep(Duration::from_millis(300));
        crate::macos::sys::Fd::open_dir(c)
    })
    .unwrap();
    assert_eq!(listed.items.len(), 1);

    let denied = list_trash_dir_gated(&bin, Duration::from_secs(5), |_| Err(libc::EPERM)).unwrap_err();
    assert!(denied.privacy_blocked(), "{denied:?}");

    let (answer, prompt) = std::sync::mpsc::channel::<()>();
    let t = Instant::now();
    let pending = list_trash_dir_gated(&bin, Duration::from_millis(200), move |c| {
        let _ = prompt.recv(); // held in the kernel until the person answers
        crate::macos::sys::Fd::open_dir(c)
    })
    .unwrap_err();
    assert!(t.elapsed() < Duration::from_secs(3), "the listing is not held by the prompt: {:?}", t.elapsed());
    let ListError::PromptPending(why) = &pending else { panic!("{pending:?}") };
    assert!(why.starts_with(crate::macos::privacy::PERMISSION_PENDING) && why.contains(&*bin.to_string_lossy()), "{why}");
    assert!(!pending.privacy_blocked(), "pending is not a denial");
    answer.send(()).unwrap(); // the parked open ends; its descriptor is dropped
}
