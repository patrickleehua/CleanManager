//! macOS maintenance tasks on fixtures (parity 14.3/14.5). Report tasks read fixture homes only; the setting round
//! trip uses an absolute-path preference domain inside a fixture. Tests that run real tools against the runner's own
//! user state (Quick Look reset, the real `com.apple.desktopservices` domain, restored afterwards) run only with
//! `CM_ALLOW_MAC_DEVICE_TESTS=1` or on a hosted runner (`GITHUB_ACTIONS=true`).

use std::collections::BTreeMap;

use super::*;
use crate::macos::testfs::Fixture;
use cm_core::maintenance::{TaskPlatform, catalog, find, settings};

fn device_tests_allowed() -> bool {
    std::env::var("CM_ALLOW_MAC_DEVICE_TESTS").is_ok_and(|v| v == "1") || std::env::var("GITHUB_ACTIONS").is_ok_and(|v| v == "true")
}

fn xml(p: &Plist) -> Vec<u8> {
    cf::plist_to_bytes(p, cf::PlistFormat::Xml).unwrap()
}

fn dict(pairs: &[(&str, Plist)]) -> Plist {
    Plist::Dict(pairs.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect::<BTreeMap<_, _>>())
}

fn tasks(home: &Path, prefs: &Path) -> MacTasks {
    MacTasks { home: Some(home.to_path_buf()), settings: MacSettings { test_dir: Some(prefs.to_path_buf()) } }
}

#[test]
fn mdutil_status_lines_are_recognized() {
    assert_eq!(parse_mdutil_status("/:\n\tIndexing enabled. \n"), Some(SpotlightState::Enabled));
    assert_eq!(parse_mdutil_status("/:\n\tIndexing disabled.\n"), Some(SpotlightState::Disabled));
    assert_eq!(parse_mdutil_status("/:\n\tIndexing and searching disabled.\n"), Some(SpotlightState::Disabled));
    assert_eq!(parse_mdutil_status("/:\n\tError: unknown indexing state.\n"), Some(SpotlightState::Unknown));
    assert_eq!(parse_mdutil_status("/:\n\tSpotlight server is disabled.\n"), Some(SpotlightState::ServerDisabled));
    assert_eq!(parse_mdutil_status("something else"), None);
}

#[test]
fn every_macos_catalog_task_has_a_probe_and_admin_tasks_never_run() {
    let fx = Fixture::new("maint-catalog");
    let t = tasks(fx.path(), fx.path());
    for spec in catalog(TaskPlatform::Macos) {
        let p = t.probe(spec);
        assert!(!matches!(&p, Probe::Unavailable { code, .. } if code == "task_unknown"), "{} has no probe", spec.id);
        if spec.requires_admin {
            let e = t.execute(spec);
            assert_eq!((e.outcome, e.code.as_str()), (TaskOutcome::Skipped, "elevation_unavailable"), "{}", spec.id);
        }
    }
    // System tools the catalog relies on exist on every supported macOS.
    for id in [ids::MAC_QUICKLOOK, ids::MAC_SPOTLIGHT_STATUS, ids::MAC_DNS_FLUSH, ids::MAC_DISK_VERIFY] {
        assert!(matches!(t.probe(find(id).unwrap()), Probe::Applicable { .. }), "{id}: {:?}", t.probe(find(id).unwrap()));
    }
    // A home without the folders: report tasks do not apply.
    for id in [ids::MAC_BROKEN_PREFS, ids::MAC_SAVED_STATE, ids::MAC_LAUNCH_AGENTS] {
        assert!(matches!(t.probe(find(id).unwrap()), Probe::NotApplicable { .. }), "{id}");
    }
}

#[test]
fn broken_preference_files_are_reported_and_never_touched() {
    let fx = Fixture::new("maint-prefs");
    let prefs = fx.mkdir("Library/Preferences");
    fx.write("Library/Preferences/com.example.good.plist", &xml(&dict(&[("A", Plist::Integer(1))])));
    fx.write("Library/Preferences/com.example.binary.plist", &cf::plist_to_bytes(&dict(&[("B", Plist::Bool(true))]), cf::PlistFormat::Binary).unwrap());
    let broken = fx.write("Library/Preferences/com.example.broken.plist", b"<?xml version=\"1.0\"?><plist><dict><key>a</key>");
    fx.write("Library/Preferences/com.example.empty.plist", b"");
    fx.write("Library/Preferences/ByHost/com.example.host.0000.plist", b"bplist00\xff\xff");
    fx.write("Library/Preferences/notes.txt", b"not a plist, not judged");
    std::os::unix::fs::symlink(&broken, prefs.join("com.example.link.plist")).unwrap();
    let before = std::fs::read(&broken).unwrap();

    let scan = scan_preferences(&prefs).unwrap();
    assert_eq!(scan.checked, 5, "{scan:?}");
    let names: Vec<&str> = scan.broken.iter().map(|b| b.split(':').next().unwrap()).collect();
    assert_eq!(names, ["ByHost/com.example.host.0000.plist", "com.example.broken.plist", "com.example.empty.plist"]);

    let t = tasks(fx.path(), fx.path());
    let spec = find(ids::MAC_BROKEN_PREFS).unwrap();
    assert!(matches!(t.probe(spec), Probe::Applicable { .. }));
    let e = t.execute(spec);
    assert_eq!((e.outcome, e.code.as_str()), (TaskOutcome::Attention, "broken_preferences"), "{}", e.summary);
    assert!(e.summary.contains("3 of 5") && e.summary.contains("Nothing was changed"), "{}", e.summary);
    assert_eq!(std::fs::read(&broken).unwrap(), before, "report only: the broken file is untouched");
    assert!(prefs.join("com.example.empty.plist").exists());

    for f in ["com.example.broken.plist", "com.example.empty.plist", "ByHost/com.example.host.0000.plist", "com.example.link.plist"] {
        std::fs::remove_file(prefs.join(f)).unwrap();
    }
    let e = t.execute(spec);
    assert_eq!((e.outcome, e.code.as_str()), (TaskOutcome::Unchanged, "no_broken_preferences"), "{}", e.summary);
}

#[test]
fn saved_states_of_missing_apps_are_reported() {
    assert_eq!(saved_state_bundle_id("com.apple.Terminal.savedState"), Some("com.apple.Terminal"));
    assert_eq!(saved_state_bundle_id(".savedState"), None);
    assert_eq!(saved_state_bundle_id("com.example.app"), None);
    assert_eq!(app_installed("com.apple.finder"), Ok(true), "LaunchServices resolves Finder");
    assert_eq!(app_installed("com.cleanmanager.test.not-installed.e6b1"), Ok(false));

    let fx = Fixture::new("maint-saved");
    let dir = fx.mkdir("Library/Saved Application State");
    fx.mkdir("Library/Saved Application State/com.apple.finder.savedState");
    fx.mkdir("Library/Saved Application State/com.cleanmanager.test.gone.savedState");
    fx.write("Library/Saved Application State/stray.txt", b"x");
    let e = stale_saved_state(&dir, &app_installed);
    assert_eq!((e.outcome, e.code.as_str()), (TaskOutcome::Attention, "stale_saved_state"), "{}", e.summary);
    assert!(e.summary.contains("com.cleanmanager.test.gone") && !e.summary.contains("com.apple.finder,"), "{}", e.summary);
    assert!(dir.join("com.cleanmanager.test.gone.savedState").is_dir(), "report only");
    let e = stale_saved_state(&dir, &|_| Err("lookup down".into()));
    assert_eq!((e.outcome, e.code.as_str()), (TaskOutcome::Skipped, "state_unknown"));
    std::fs::remove_dir(dir.join("com.cleanmanager.test.gone.savedState")).unwrap();
    let e = tasks(fx.path(), fx.path()).execute(find(ids::MAC_SAVED_STATE).unwrap());
    assert_eq!((e.outcome, e.code.as_str()), (TaskOutcome::Unchanged, "no_stale_saved_state"), "{}", e.summary);
}

#[test]
fn launch_agents_with_missing_programs_are_reported() {
    let fx = Fixture::new("maint-agents");
    let dir = fx.mkdir("Library/LaunchAgents");
    fx.write("Library/LaunchAgents/com.example.ok.plist", &xml(&dict(&[("Label", Plist::String("com.example.ok".into())), ("Program", Plist::String("/usr/bin/true".into()))])));
    fx.write(
        "Library/LaunchAgents/com.example.gone.plist",
        &xml(&dict(&[
            ("Label", Plist::String("com.example.gone".into())),
            ("ProgramArguments", Plist::Array(vec![Plist::String("/Applications/Gone.app/Contents/MacOS/helper".into()), Plist::String("--x".into())])),
        ])),
    );
    fx.write("Library/LaunchAgents/com.example.relative.plist", &xml(&dict(&[("Label", Plist::String("rel".into())), ("Program", Plist::String("helper".into()))])));
    fx.write("Library/LaunchAgents/com.example.broken.plist", b"<plist><dict><key>Label");
    let (checked, findings) = scan_launch_agents(&dir).unwrap();
    assert_eq!(checked, 4);
    assert_eq!(findings.len(), 2, "{findings:?}");
    assert!(findings.iter().any(|f| matches!(f, AgentFinding::MissingProgram { label, .. } if label == "com.example.gone")));
    assert!(findings.iter().any(|f| matches!(f, AgentFinding::Unreadable { file, .. } if file == "com.example.broken.plist")));
    let t = tasks(fx.path(), fx.path());
    let e = t.execute(find(ids::MAC_LAUNCH_AGENTS).unwrap());
    assert_eq!((e.outcome, e.code.as_str()), (TaskOutcome::Attention, "stale_launch_agents"), "{}", e.summary);
    assert!(dir.join("com.example.gone.plist").exists(), "report only");
    std::fs::remove_file(dir.join("com.example.gone.plist")).unwrap();
    std::fs::remove_file(dir.join("com.example.broken.plist")).unwrap();
    let e = t.execute(find(ids::MAC_LAUNCH_AGENTS).unwrap());
    assert_eq!((e.outcome, e.code.as_str()), (TaskOutcome::Unchanged, "no_stale_launch_agents"), "{}", e.summary);
}

#[test]
fn settings_round_trip_in_a_fixture_domain() {
    let fx = Fixture::new("maint-settings");
    let s = MacSettings { test_dir: Some(fx.path().to_path_buf()) };
    let spec = settings::for_task(ids::MAC_NETWORK_DSSTORE).unwrap();
    let file = fx.path().join(format!("{}.plist", spec.domain));
    assert_eq!(s.read(spec).unwrap(), ABSENT);
    let t = tasks(fx.path(), fx.path());
    assert!(matches!(t.probe(find(ids::MAC_NETWORK_DSSTORE).unwrap()), Probe::Applicable { .. }));
    s.write(spec, "true").unwrap();
    assert_eq!(s.read(spec).unwrap(), "true");
    let on_disk = cf::plist_from_bytes(&std::fs::read(&file).unwrap()).unwrap().0;
    assert_eq!(on_disk.get(spec.key), Some(&Plist::Bool(true)), "written to the fixture domain only");
    assert!(matches!(t.probe(find(ids::MAC_NETWORK_DSSTORE).unwrap()), Probe::NotNeeded { .. }));
    s.write(spec, "false").unwrap();
    assert_eq!(s.read(spec).unwrap(), "false");
    s.write(spec, ABSENT).unwrap();
    assert_eq!(s.read(spec).unwrap(), ABSENT);
    assert!(s.write(spec, "1").is_err(), "only known values are written");
    let domain = fx.path().join(spec.domain).to_str().unwrap().to_owned();
    cf::pref_set(&domain, spec.key, Some(&Plist::Integer(1))).unwrap();
    assert_eq!(s.read(spec).unwrap(), "other:integer");
    assert!(matches!(t.probe(find(ids::MAC_NETWORK_DSSTORE).unwrap()), Probe::Unknown { .. }), "another value type is never changed");
    cf::pref_set(&domain, spec.key, None).unwrap();
}

/// The real `com.apple.desktopservices` domain of the runner's user: set, read back, then the prior value is put back.
#[test]
fn settings_round_trip_in_the_real_domain_on_a_runner() {
    if !device_tests_allowed() {
        eprintln!("skipped: set CM_ALLOW_MAC_DEVICE_TESTS=1 to write com.apple.desktopservices (restored afterwards)");
        return;
    }
    let s = MacSettings::default();
    let spec = settings::for_task(ids::MAC_NETWORK_DSSTORE).unwrap();
    let prior = s.read(spec).unwrap();
    assert!(settings::is_known_value(&prior), "unexpected prior value {prior}");
    struct Restore<'a>(&'a MacSettings, &'a SettingSpec, String);
    impl Drop for Restore<'_> {
        fn drop(&mut self) {
            let _ = self.0.write(self.1, &self.2);
        }
    }
    let guard = Restore(&s, spec, prior.clone());
    let other = if prior == "true" { "false" } else { "true" };
    s.write(spec, other).unwrap();
    assert_eq!(s.read(spec).unwrap(), other);
    drop(guard);
    assert_eq!(s.read(spec).unwrap(), prior, "the prior value is back");
}

#[test]
fn mount_points_are_checked() {
    check_mount_point("/").unwrap();
    assert!(check_mount_point("/usr/bin").is_err());
    assert!(check_mount_point("/definitely/not/here").is_err());
}

#[test]
fn spotlight_status_reads_the_startup_volume() {
    let e = spotlight_status(Duration::from_secs(30));
    assert!(matches!(e.outcome, TaskOutcome::Unchanged | TaskOutcome::Attention), "{e:?}");
    eprintln!("spotlight status on this host: {} ({})", e.code, e.summary);
}

#[test]
fn quicklook_reset_runs_on_a_runner() {
    if !device_tests_allowed() {
        eprintln!("skipped: set CM_ALLOW_MAC_DEVICE_TESTS=1 to reset your Quick Look cache");
        return;
    }
    let e = quicklook_reset(Duration::from_secs(60));
    assert_eq!((e.outcome, e.code.as_str()), (TaskOutcome::Applied, "reset"), "{}", e.summary);
}

/// A key written by another process (here `/usr/bin/defaults`, as `plan apply` and the later restore are separate
/// processes) is removed by [`MacSettings::write`] with `absent`; the strategies are logged for diagnosis.
#[test]
fn a_key_written_by_another_process_is_removed() {
    let defaults = |args: &[&str]| {
        let o = std::process::Command::new("/usr/bin/defaults").args(args).output().unwrap();
        format!("{}{}", String::from_utf8_lossy(&o.stdout).trim(), String::from_utf8_lossy(&o.stderr).trim())
    };
    let spec = settings::for_task(ids::MAC_NETWORK_DSSTORE).unwrap();
    // Plain removal, as diagnosis.
    let fx = Fixture::new("maint-xproc-plain");
    let domain = fx.path().join(spec.domain).to_str().unwrap().to_owned();
    defaults(&["write", &domain, spec.key, "-bool", "true"]);
    let before = cf::pref_get(&domain, spec.key).unwrap();
    cf::pref_set(&domain, spec.key, None).unwrap();
    let after = cf::pref_get(&domain, spec.key).unwrap();
    eprintln!("plain removal: before={before:?} after={after:?} defaults-read={}", defaults(&["read", &domain, spec.key]));
    // The settings backend.
    let fx = Fixture::new("maint-xproc");
    let s = MacSettings { test_dir: Some(fx.path().to_path_buf()) };
    let domain = fx.path().join(spec.domain).to_str().unwrap().to_owned();
    defaults(&["write", &domain, spec.key, "-bool", "true"]);
    assert_eq!(s.read(spec).unwrap(), "true");
    let r = s.write(spec, ABSENT);
    eprintln!("backend removal: {r:?} read={:?} defaults-read={}", s.read(spec), defaults(&["read", &domain, spec.key]));
    r.unwrap();
    assert_eq!(s.read(spec).unwrap(), ABSENT);
}
