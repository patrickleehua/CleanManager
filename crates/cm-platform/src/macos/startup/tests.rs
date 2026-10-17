use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;

use cm_core::plan::v2::StartupOperation;
use cm_core::startup::change_decision;

use super::*;
use crate::macos::testfs::Fixture;

fn xml(body: &str) -> Vec<u8> {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>{body}</dict></plist>\n"
    )
    .into_bytes()
}

fn s(k: &str, v: &str) -> String {
    format!("<key>{k}</key><string>{v}</string>")
}

fn args(v: &[&str]) -> String {
    let items: String = v.iter().map(|a| format!("<string>{a}</string>")).collect();
    format!("<key>ProgramArguments</key><array>{items}</array>")
}

fn env_of(fx: &Fixture) -> StartupEnv {
    StartupEnv { root: Some(fx.path().to_path_buf()), user_agents: Some(fx.path().join("home/Library/LaunchAgents")), uid: 0 }
}

fn by_name<'a>(items: &'a [StartupItem], name: &str) -> &'a StartupItem {
    items.iter().find(|i| i.name == name).unwrap_or_else(|| panic!("no row {name}: {items:#?}"))
}

fn target(i: &StartupItem) -> String {
    i.target.as_ref().map(|t| t.display()).unwrap_or_default()
}

/// Every row kind from fixture plists under a test root (launchctl is never run while redirected).
#[test]
fn fixture_inventory_reports_every_row_kind() {
    let fx = Fixture::new("startup-inv");
    let root = fx.path().to_path_buf();
    let bin = fx.write("bin/agent", b"#!/bin/sh\nexit 0\n");
    let bin_s = bin.to_str().unwrap();
    fx.write(
        "home/Library/LaunchAgents/com.example.valid.plist",
        &xml(&format!("{}{}<key>RunAtLoad</key><true/>", s("Label", "com.example.valid"), args(&[bin_s, "--background"]))),
    );
    let missing = root.join("gone/Tool.app/Contents/MacOS/Tool");
    fx.write("home/Library/LaunchAgents/com.example.broken.plist", &xml(&format!("{}{}", s("Label", "com.example.broken"), s("Program", missing.to_str().unwrap()))));
    fx.write(
        "home/Library/LaunchAgents/com.example.off.plist",
        &xml(&format!("{}{}<key>Disabled</key><true/>", s("Label", "com.example.off"), s("Program", bin_s))),
    );
    // A binary plist with a conditional KeepAlive, installed for every user.
    let mut d = BTreeMap::new();
    d.insert("Label".to_owned(), Plist::String("com.example.binary".into()));
    d.insert("ProgramArguments".to_owned(), Plist::Array(vec![Plist::String(bin_s.into())]));
    d.insert("KeepAlive".to_owned(), Plist::Dict(BTreeMap::from([("SuccessfulExit".to_owned(), Plist::Bool(false))])));
    let binary = cf::plist_to_bytes(&Plist::Dict(d), PlistFormat::Binary).unwrap();
    assert!(binary.starts_with(b"bplist00"));
    fx.write("Library/LaunchAgents/com.example.binary.plist", &binary);
    fx.write("Library/LaunchDaemons/com.example.corrupt.plist", b"<?xml version=\"1.0\"?><plist><dict><key>Label");
    let offline = format!("/Volumes/CM-Offline-{}-{:x}/App.app/Contents/MacOS/App", std::process::id(), root.as_os_str().len());
    fx.write("Library/LaunchDaemons/com.example.offline.plist", &xml(&format!("{}{}", s("Label", "com.example.offline"), s("Program", &offline))));
    fx.write("Library/LaunchAgents/com.example.nolabel.plist", &xml(&args(&[bin_s])));
    fx.write("Library/LaunchAgents/com.example.bare.plist", &xml(&format!("{}{}", s("Label", "com.example.bare"), args(&["true", "-x"]))));
    fx.write("Library/LaunchAgents/notes.txt", b"not a plist");
    fx.write("System/Library/LaunchDaemons/com.apple.fixture.plist", &xml(&format!("{}{}", s("Label", "com.apple.fixture"), s("Program", "/usr/bin/true"))));

    let inv = inventory_detailed(&env_of(&fx));
    let items = &inv.items;
    assert!(inv.issues.is_empty(), "{:?}", inv.issues);
    assert_eq!(items.len(), 10, "nine plists and the Service Management row: {items:#?}");

    let valid = by_name(items, "com.example.valid");
    assert_eq!((valid.source, valid.scope, valid.state, valid.protected), (StartupSource::LaunchdUser, StartupScope::User, StartupState::Enabled, false));
    assert_eq!(target(valid), bin_s);
    assert_eq!((valid.publisher.as_deref(), valid.publisher_evidence), (None, PublisherEvidence::None), "an unsigned script has no publisher");
    let r = valid.reason.as_deref().unwrap();
    assert!(r.contains(TEST_STATE_NOTE) && r.contains("Launch: RunAtLoad") && r.ends_with(TOGGLE_UNSUPPORTED), "{r}");
    assert!(valid.definition.ends_with("home/Library/LaunchAgents/com.example.valid.plist"), "{}", valid.definition);
    let job = &inv.jobs[&valid.item_id];
    assert_eq!((job.label.as_deref(), job.arguments.len(), job.run_at_load, job.keep_alive, job.format), (Some("com.example.valid"), 2, true, KeepAlive::No, "xml"));

    let broken = by_name(items, "com.example.broken");
    assert_eq!(broken.state, StartupState::Broken);
    assert!(broken.reason.as_deref().unwrap().contains("missing on a mounted volume"));

    let off = by_name(items, "com.example.off");
    assert_eq!(off.state, StartupState::Disabled);
    assert_eq!(inv.jobs[&off.item_id].disabled_key, Some(true));

    let bin_row = by_name(items, "com.example.binary");
    assert_eq!((bin_row.source, bin_row.scope, bin_row.state), (StartupSource::LaunchdSystem, StartupScope::System, StartupState::Enabled));
    assert_eq!((inv.jobs[&bin_row.item_id].format, inv.jobs[&bin_row.item_id].keep_alive), ("binary", KeepAlive::Conditional));
    assert!(bin_row.reason.as_deref().unwrap().contains("KeepAlive (conditional)"));

    let corrupt = by_name(items, "com.example.corrupt");
    assert_eq!((corrupt.state, corrupt.target.is_none()), (StartupState::Unknown, true));
    assert!(corrupt.reason.as_deref().unwrap().contains("could not be read as a launchd job"), "{:?}", corrupt.reason);
    assert!(!inv.jobs.contains_key(&corrupt.item_id));

    let off_vol = by_name(items, "com.example.offline");
    assert_eq!(off_vol.state, StartupState::TargetUnreachable, "an offline volume is unknown, never broken");
    assert!(off_vol.reason.as_deref().unwrap().contains("not treated as broken"));

    let nolabel = by_name(items, "com.example.nolabel");
    assert_eq!(nolabel.state, StartupState::Unknown);
    assert!(nolabel.reason.as_deref().unwrap().contains("No Label"));

    let bare = by_name(items, "com.example.bare");
    assert_eq!((bare.state, target(bare).as_str()), (StartupState::Enabled, "/usr/bin/true"));

    let os = by_name(items, "com.apple.fixture");
    assert!(os.protected && os.scope == StartupScope::System, "{os:?}");
    assert_eq!((os.publisher.as_deref(), os.publisher_evidence), (Some("Apple"), PublisherEvidence::CodeSignature));
    assert!(os.reason.as_deref().unwrap().starts_with("Part of macOS"));

    let sm: Vec<_> = items.iter().filter(|i| matches!(i.source, StartupSource::SmLoginItem | StartupSource::SmBackgroundItem)).collect();
    assert_eq!(sm.len(), 1);
    assert_eq!((sm[0].state, sm[0].definition.as_str()), (StartupState::Unknown, LOGIN_ITEMS_SETTINGS));
    assert!(sm[0].reason.as_deref().unwrap().contains(LOGIN_ITEMS_SETTINGS));

    // STA-02/04: nothing toggles on macOS; protected rows refuse as protected, the rest as unsupported.
    for i in items {
        assert_eq!(i.toggle_capability, CapabilityState::Unsupported, "{i:?}");
        assert!(i.reason.as_deref().is_some_and(|r| !r.is_empty()) && i.location.is_none() && !i.removable_when_stale, "{i:?}");
        let w = i.to_wire();
        assert_eq!(w["toggle_capability"], "unsupported");
    }
    assert_eq!(change_decision(valid, StartupOperation::Disable, true).unwrap_err().code, "unsupported");
    assert_eq!(change_decision(os, StartupOperation::Disable, true).unwrap_err().code, "policy_denied");
    assert_eq!(change_decision(broken, StartupOperation::RemoveWithBackup, true).unwrap_err().code, "unsupported");
    assert_eq!(by_name(items, "com.example.offline").to_wire()["state"], "target_unreachable");
    assert_eq!(corrupt.to_wire()["state"], "unknown");

    // Stable IDs (from the plist path, not the label or target).
    let again = inventory_detailed(&env_of(&fx));
    assert_eq!(items.iter().map(|i| &i.item_id).collect::<Vec<_>>(), again.items.iter().map(|i| &i.item_id).collect::<Vec<_>>());
}

#[test]
fn an_unreadable_folder_is_an_issue_not_an_empty_list() {
    let fx = Fixture::new("startup-locked");
    fx.write("Library/LaunchDaemons/com.example.x.plist", &xml(&s("Label", "com.example.x")));
    let locked = fx.path().join("Library/LaunchDaemons");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let (items, issues) = inventory_with(&env_of(&fx));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return; // root reads through the mode bits
    }
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].code, "permission_denied");
    assert!(issues[0].source.ends_with("Library/LaunchDaemons"));
    assert_eq!(items.len(), 1, "only the Service Management row: {items:?}");
}

#[test]
fn test_roots_are_checked() {
    let fx = Fixture::new("startup-root");
    assert!(StartupEnv::checked_root(Path::new("relative/dir")).is_err());
    assert!(StartupEnv::checked_root(Path::new("/")).is_err());
    assert!(StartupEnv::checked_root(&fx.path().join("missing")).is_err());
    assert_eq!(StartupEnv::checked_root(fx.path()).unwrap(), fx.path());
    let real = StartupEnv::real();
    assert!(!real.is_redirected());
    assert!(real.folders().iter().any(|f| f.dir == Path::new("/System/Library/LaunchDaemons") && f.tier == Tier::Os));
}

#[test]
fn launchctl_output_parsing() {
    let new = "disabled services = {\n\t\"com.example.a\" => disabled\n\t\"com.example.b\" => enabled\n}\n\nlogin item associations = {\n\t\"com.example.c\" => disabled\n}\n";
    let got = parse_print_disabled(new);
    assert_eq!(got, BTreeMap::from([("com.example.a".to_owned(), true), ("com.example.b".to_owned(), false)]));
    let old = "disabled services = {\n\t\"com.example.a\" => true\n\t\"com.example.b\" => false\n}\n";
    assert_eq!(parse_print_disabled(old), got);
    assert!(parse_print_disabled("").is_empty());

    let print = "gui/501 = {\n\ttype = login\n\tservices = {\n\t\t       0      - \tcom.example.idle\n\t\t     487       0 \tcom.example.running\n\t\t       -     -9 \tcom.example.exited\n\t}\n\n\tendpoints = {\n\t\t\"com.example.port\"\n\t}\n}\n";
    let s = parse_services(print).unwrap();
    assert_eq!(s.get("com.example.idle"), Some(&None));
    assert_eq!(s.get("com.example.running"), Some(&Some(487)));
    assert_eq!(s.get("com.example.exited"), Some(&None));
    assert_eq!(s.len(), 3);
    assert_eq!(parse_services("gui/501 = {\n}\n"), None);
}

#[test]
fn offline_volume_detection() {
    assert!(volume_offline(Path::new(&format!("/Volumes/CM-Not-Mounted-{}/x", std::process::id()))));
    assert!(!volume_offline(Path::new("/Applications/x")));
    assert!(!volume_offline(Path::new("/Volumes")));
    assert_eq!(check_target(Path::new("/usr/bin/true")), TargetCheck::Exists);
    assert_eq!(check_target(Path::new("/usr/bin/true/sub")), TargetCheck::Missing);
    assert_eq!(resolve_program("true").unwrap(), Path::new("/usr/bin/true"));
    assert!(resolve_program("./rel").is_err());
    assert!(resolve_program("cm-no-such-program").is_err());
}

/// The real locations of the runner's user, read-only: /System jobs are listed and protected, nothing toggles, and
/// launchd's state was read for at least some rows.
#[test]
fn live_inventory_is_read_only_and_protects_system_jobs() {
    let started = std::time::Instant::now();
    let inv = inventory_detailed(&StartupEnv::real());
    let took = started.elapsed();
    let count = |st: StartupState| inv.items.iter().filter(|i| i.state == st).count();
    eprintln!(
        "live startup inventory: {} rows in {took:?} (enabled {}, disabled {}, broken {}, unreachable {}, unknown {}); issues {:?}",
        inv.items.len(),
        count(StartupState::Enabled),
        count(StartupState::Disabled),
        count(StartupState::Broken),
        count(StartupState::TargetUnreachable),
        count(StartupState::Unknown),
        inv.issues
    );
    for i in inv.items.iter().take(400).filter(|i| !i.protected).chain(inv.items.iter().filter(|i| i.protected).take(5)) {
        eprintln!("  {} [{:?}/{:?}] {:?} pub={:?} {}", i.name, i.source, i.state, target(i), i.publisher, i.reason.as_deref().unwrap_or(""));
    }
    let system: Vec<_> = inv.items.iter().filter(|i| i.definition.starts_with("/System/Library/Launch")).collect();
    assert!(!system.is_empty(), "macOS ships launchd jobs in /System/Library");
    assert!(system.iter().all(|i| i.protected && i.scope == StartupScope::System), "{system:#?}");
    assert!(inv.items.iter().all(|i| i.toggle_capability == CapabilityState::Unsupported && i.location.is_none()));
    assert!(system.iter().any(|i| i.publisher.as_deref() == Some("Apple")), "Apple-signed targets");
    assert!(inv.items.iter().any(|i| i.reason.as_deref().is_some_and(|r| r.contains("Loaded in "))), "launchctl state was read");
    assert!(!inv.items.iter().any(|i| i.reason.as_deref().is_some_and(|r| r.contains(TEST_STATE_NOTE))));
}
