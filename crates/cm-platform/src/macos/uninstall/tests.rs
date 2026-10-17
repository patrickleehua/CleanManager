//! Fixture bundles under a redirected root and home only: no real application, Homebrew or `~/Library` is touched.
//! The one real Trash move (a fixture bundle) runs only with `CM_ALLOW_MAC_DEVICE_TESTS=1` or on a hosted runner, and
//! removes exactly what it moved.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command};

use cm_core::uninstall::{Group, Marker};

use super::*;
use crate::macos::ffi::cf::PlistFormat;
use crate::macos::testfs::Fixture;

fn device_tests_allowed() -> bool {
    std::env::var("CM_ALLOW_MAC_DEVICE_TESTS").is_ok_and(|v| v == "1") || std::env::var("GITHUB_ACTIONS").is_ok_and(|v| v == "true")
}

struct Env {
    fx: Fixture,
    env: UninstallEnv,
}

impl Env {
    fn new(tag: &str) -> Env {
        let fx = Fixture::new(tag);
        let apps = AppsEnv { root: Some(fx.mkdir("root")), home: Some(fx.mkdir("home")) };
        Env { env: UninstallEnv { apps, quit_wait: Duration::from_millis(1500), is_admin: false }, fx }
    }

    fn root(&self) -> PathBuf {
        self.env.apps.root.clone().unwrap()
    }

    fn home(&self) -> PathBuf {
        self.env.apps.home.clone().unwrap()
    }

    fn record(&self, name: &str) -> UninstallRecord {
        let (recs, _) = records(&self.env);
        recs.into_iter().find(|r| r.name == name).unwrap_or_else(|| panic!("no record {name}"))
    }
}

fn info(pairs: &[(&str, &str)]) -> Vec<u8> {
    let map: BTreeMap<String, Plist> = pairs.iter().map(|(k, v)| ((*k).to_owned(), Plist::String((*v).to_owned()))).collect();
    cf::plist_to_bytes(&Plist::Dict(map), PlistFormat::Xml).unwrap()
}

/// A fake bundle whose executable is a copy of `program` (a system binary), with an Info.plist.
fn bundle(at: &Path, bid: &str, name: &str, program: &str) -> PathBuf {
    let exe = at.join("Contents/MacOS").join(name);
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    crate::macos::testfs::copy_system_binary(program, &exe);
    std::fs::write(at.join("Contents/Info.plist"), info(&[("CFBundleIdentifier", bid), ("CFBundleName", name), ("CFBundleExecutable", name), ("CFBundleShortVersionString", "1.0")]))
        .unwrap();
    exe
}

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_until_listed(u: &MacUninstall, id: &AppIdentity, n: usize) -> Vec<RunningProcess> {
    for _ in 0..50 {
        if let RunningState::Known { processes } = u.running(id)
            && processes.len() >= n
        {
            return processes;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("the fixture process never showed its executable path");
}

#[test]
fn records_carry_bundle_facts_and_routes_follow_the_source() {
    let e = Env::new("unin-routes");
    let apps = e.root().join("Applications");
    bundle(&apps.join("Editor.app"), "com.example.editor", "Editor", "/usr/bin/true");
    let store = apps.join("Store.app");
    bundle(&store, "com.example.store", "Store", "/usr/bin/true");
    std::fs::create_dir_all(store.join("Contents/_MASReceipt")).unwrap();
    std::fs::write(store.join("Contents/_MASReceipt/receipt"), b"opaque").unwrap();
    bundle(&apps.join("Locked/Locked.app"), "com.example.locked", "Locked", "/usr/bin/true");
    bundle(&e.root().join("System/Applications/Notes.app"), "com.apple.Notes", "Notes", "/usr/bin/true");
    std::fs::set_permissions(apps.join("Locked"), std::fs::Permissions::from_mode(0o555)).unwrap();

    let editor = e.record("Editor");
    let mac = editor.mac.clone().unwrap();
    assert_eq!((mac.bundle_id.as_deref(), mac.admin_reason.as_deref()), (Some("com.example.editor"), None));
    assert!(mac.volume_id.is_some() && mac.file_id.is_some());
    let c = route(&e.env, &editor).unwrap();
    assert_eq!((c.kind, c.program.as_str()), (AdapterKind::MacTrashAppBundle, apps.join("Editor.app").to_str().unwrap()));
    let (sha, _) = verify_program(&c).unwrap();
    assert_eq!(sha.len(), 64);

    let store = route(&e.env, &e.record("Store")).unwrap_err();
    assert!(store.code == "permission_required" && store.message.contains("Launchpad"), "{store:?}");
    // SAFETY: plain getter.
    if unsafe { libc::geteuid() } != 0 {
        let locked = route(&e.env, &e.record("Locked")).unwrap_err();
        assert!(locked.code == "permission_required" && locked.message.contains("administrator"), "{locked:?}");
        assert_eq!(route_capability(&Err(locked)).state, CapabilityState::Unsupported);
    }
    let notes = route(&e.env, &e.record("Notes")).unwrap_err();
    assert!(notes.code == "policy_denied" && notes.message.contains("Part of macOS"), "{notes:?}");
    std::fs::set_permissions(apps.join("Locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_running_bundle_is_asked_to_quit_and_a_stubborn_one_is_reported_not_ended() {
    let e = Env::new("unin-quit");
    let apps = e.root().join("Applications");
    let exe = bundle(&apps.join("Sleepy.app"), "com.example.sleepy", "Sleepy", "/bin/sleep");
    let u = MacUninstall::non_interactive(e.env.clone());
    let id = AppIdentity::of(&e.record("Sleepy"));
    assert_eq!(u.running(&id), RunningState::Known { processes: vec![] });
    let child = Running(Command::new(&exe).arg("60").spawn().unwrap());
    let procs = wait_until_listed(&u, &id, 1);
    assert_eq!(procs[0].pid, child.0.id());
    assert!(u.request_quit(&procs).is_empty(), "SIGTERM is the quit request for a process without an app event loop");
    drop(child);

    // A process that ignores the request stays: reported, never ended automatically.
    let exe = bundle(&apps.join("Stubborn.app"), "com.example.stubborn", "Stubborn", "/bin/bash");
    let id = AppIdentity::of(&e.record("Stubborn"));
    // The script reports once its trap is set, so the quit request cannot arrive before it.
    let ready = e.fx.path().join("trap-set");
    let script = format!("trap '' TERM; : > '{}'; while :; do sleep 1; done", ready.display());
    let child = Running(Command::new(&exe).args(["-c", &script]).spawn().unwrap());
    for _ in 0..100 {
        if ready.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(ready.exists(), "the fixture script never started");
    let procs = wait_until_listed(&u, &id, 1);
    let still = u.request_quit(&procs);
    assert_eq!(still.len(), 1, "{procs:?}");
    assert!(!u.confirm_end(&still), "non-interactive: never confirmed");
    assert!(still_running(&still[0]));
    // Only a plan reviewed for it, after confirmation, ends it.
    u.end_process(&still[0]).unwrap();
    assert!(!still_running(&still[0]));
    drop(child);
}

#[test]
fn the_bundle_move_is_bound_to_its_identity_and_never_deletes() {
    let e = Env::new("unin-trash");
    let unique = format!("CMUninstFixture{}{}", std::process::id(), e.fx.path().file_name().unwrap().to_string_lossy().rsplit('-').next().unwrap());
    let app = e.root().join("Applications").join(format!("{unique}.app"));
    bundle(&app, "com.example.fixture", &unique, "/usr/bin/true");
    let rec = e.record(&unique);
    let mac = rec.mac.clone().unwrap();
    let (vid, fid) = (mac.volume_id.unwrap(), mac.file_id.unwrap());
    let path = app.to_str().unwrap();
    // Another object at the planned name (or a wrong identity) is never moved.
    let out = trash_bundle(path, &vid, "999999999");
    assert!(matches!(&out, ActionOutcome::Skipped { code, .. } if code == "identity_changed"), "{out:?}");
    assert!(app.is_dir());
    // A link at the planned name is not followed.
    let link = e.root().join("Applications/Link.app");
    std::os::unix::fs::symlink(&app, &link).unwrap();
    assert!(matches!(trash_bundle(link.to_str().unwrap(), &vid, &fid), ActionOutcome::Skipped { code, .. } if code == "link_or_reparse"));
    // The verified backend refuses a move it did not verify in the same action.
    let u = MacUninstall::non_interactive(e.env.clone());
    let choice = route(&e.env, &rec).unwrap();
    let err = u.run_step(&choice, 60).unwrap_err();
    assert_eq!((err.0, err.1.as_str()), (StepStatus::Skipped, "precondition_missing"));
    // A changed Info.plist after planning: program_changed, nothing moved.
    let (sha, signer) = verify_program(&choice).unwrap();
    assert!(u.verify(&rec, &choice, &"0".repeat(64), signer.as_deref()).is_err_and(|(c, _)| c == "program_changed"));
    if !device_tests_allowed() {
        eprintln!("skipped the real Trash move: set CM_ALLOW_MAC_DEVICE_TESTS=1 to move a fixture bundle to your Trash");
        return;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
    let in_trash = home.join(".Trash").join(format!("{unique}.app"));
    u.verify(&rec, &choice, &sha, signer.as_deref()).unwrap();
    match u.run_step(&choice, 60) {
        Ok((Some(0), _)) => {
            assert!(!app.exists(), "moved");
            let r = u.rescan(&AppIdentity::of(&rec));
            assert_eq!((r.install_dir_present, r.registration_present), (Some(false), Some(false)));
            assert!(in_trash.is_dir(), "in the user's Trash: {}", in_trash.display());
            std::fs::remove_dir_all(&in_trash).unwrap();
        }
        Err((_, code, message)) if code == "trash_unavailable" => eprintln!("the runner's volume has no Trash: {message}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn homebrew_runs_the_fixed_argv_through_the_trusted_runner() {
    let e = Env::new("unin-brew");
    let log = e.fx.path().join("brew.log");
    let brew = e.fx.path().join("brew/bin/brew");
    std::fs::create_dir_all(brew.parent().unwrap()).unwrap();
    let cask = e.root().join("opt/homebrew/Caskroom/fixture-cask");
    std::fs::create_dir_all(cask.join("1.0")).unwrap();
    let script = format!(
        "#!/bin/sh\necho \"$*\" >> '{}'\n[ \"$HOMEBREW_NO_AUTO_UPDATE\" = 1 ] || exit 7\n[ \"$*\" = 'uninstall --cask fixture-cask' ] || exit 8\nrm -rf '{}'\n",
        log.display(),
        cask.display()
    );
    std::fs::write(&brew, script).unwrap();
    std::fs::set_permissions(&brew, std::fs::Permissions::from_mode(0o755)).unwrap();
    let choice = AdapterChoice {
        kind: AdapterKind::HomebrewCask, program: brew.to_string_lossy().into_owned(), argv: ["uninstall", "--cask", "fixture-cask"].map(str::to_owned).to_vec(),
        raw_args: false, publisher_required: None, microsoft_signed: false, display: "brew".into(), keeps_own_ui: false,
    };
    let (sha, signer) = verify_program(&choice).unwrap();
    assert!(sha.len() == 64 && signer.is_none());
    let u = MacUninstall::non_interactive(e.env.clone());
    let (exit, _) = u.run_step(&choice, 60).unwrap();
    assert_eq!(exit, Some(0));
    assert_eq!(std::fs::read_to_string(&log).unwrap().trim(), "uninstall --cask fixture-cask");
    let id = AppIdentity { package_name: Some("fixture-cask".into()), source: RecordSource::Homebrew, install_location: None, ..AppIdentity::of(&rec_stub()) };
    assert_eq!(u.rescan(&id).registration_present, Some(false));
    // A brew others may change is refused before anything runs.
    std::fs::set_permissions(&brew, std::fs::Permissions::from_mode(0o757)).unwrap();
    assert!(verify_program(&choice).is_err_and(|(c, m)| c == "policy_denied" && m.contains("writable by others")));
}

fn rec_stub() -> UninstallRecord {
    serde_json::from_value(serde_json::json!({"instance_id": "app_x", "name": "X", "source": "homebrew", "system_component": false})).unwrap()
}

#[test]
fn leftovers_are_read_from_the_redirected_library_with_sizes_outside_containers() {
    let e = Env::new("unin-left");
    let lib = e.home().join("Library");
    let write = |rel: &str, bytes: &[u8]| {
        let p = lib.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    };
    write("Caches/com.example.editor/blob", &[1u8; 300]);
    write("Caches/Quillpad/decoy", b"x");
    write("Preferences/com.example.editor.plist", b"p");
    write("Containers/com.example.editor/Data/x", b"secret");
    write("Group Containers/ABCDE12345.com.example/x", b"g");
    let agent = cf::plist_to_bytes(
        &Plist::Dict(BTreeMap::from([
            ("Label".to_owned(), Plist::String("com.vendor.updater".into())),
            ("ProgramArguments".to_owned(), Plist::Array(vec![Plist::String("/Applications/Quillpad.app/Contents/MacOS/upd".into())])),
        ])),
        PlistFormat::Binary,
    )
    .unwrap();
    write("LaunchAgents/com.vendor.updater.plist", &agent);
    let mut rec = rec_stub();
    rec.name = "Quillpad".into();
    rec.source = RecordSource::AppBundle;
    rec.install_location = Some("/Applications/Quillpad.app".into());
    rec.mac = Some(MacRecord {
        bundle_id: Some("com.example.editor".into()), team_id: Some("ABCDE12345".into()), homebrew_kind: None, volume_id: None, file_id: None, admin_reason: None,
        pkg_receipts: vec![], npm_prefix: None,
    });
    let items = discover(&e.env, &AppIdentity::of(&rec), rec.mac.as_ref(), &[], true);
    let by = |suffix: &str| items.iter().find(|l| l.display.ends_with(suffix)).unwrap_or_else(|| panic!("{suffix}: {items:#?}"));
    assert_eq!((by("Caches/com.example.editor").logical_bytes, by("Caches/com.example.editor").selected), (Some(300), true));
    assert_eq!((by("Caches/Quillpad").selected, by("Caches/Quillpad").tier.as_str()), (false, "review_only"), "name only");
    assert_eq!(by("com.example.editor.plist").group, Group::Settings);
    assert_eq!(by("Containers/com.example.editor").logical_bytes, None, "never read inside Containers");
    assert_eq!(by("ABCDE12345.com.example").marker, Some(Marker::SharedOwner));
    assert!(by("com.vendor.updater.plist").evidence[0].contains("inside the removed bundle"), "the plist was parsed");
    assert!(items.iter().all(|l| l.display.starts_with(e.home().to_str().unwrap()) || l.kind == cm_core::uninstall::LeftoverKind::StartupEntry));
}
