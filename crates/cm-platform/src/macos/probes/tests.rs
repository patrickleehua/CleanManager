use std::process::{Child, Command};

use super::*;
use crate::macos::testfs::Fixture;

const CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const CHROME_HELPER: &str =
    "/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Versions/1/Helpers/Google Chrome Helper (Renderer).app/Contents/MacOS/Google Chrome Helper (Renderer)";

#[test]
fn owner_probe_is_tri_state_and_enumeration_failure_is_unknown() {
    let running = ProcessSnapshot::fixed(Ok(&[("launchd", Some("/sbin/launchd")), ("Google Chrome", Some(CHROME))]));
    assert_eq!(probe_owner(Some("google.chrome"), &running).state, ProbeState::InUse);
    assert_eq!(probe_owner(Some("microsoft.edge"), &running).state, ProbeState::NotInUse);
    let failed = ProcessSnapshot::fixed(Err("snapshot denied"));
    assert_eq!(probe_owner(Some("microsoft.edge"), &failed).state, ProbeState::Unknown, "a failed listing is never 'not running'");
    assert_eq!(probe_owner(Some("unmapped.app"), &running).state, ProbeState::Unknown);
    assert_eq!(probe_owner(None, &running).state, ProbeState::Unknown);
    // Helpers inside the bundle count; a same-named folder elsewhere on the path does too (the bundle is a component).
    let helper = ProcessSnapshot::fixed(Ok(&[("Google Chrome He", Some(CHROME_HELPER))]));
    assert_eq!(probe_owner(Some("google.chrome"), &helper).state, ProbeState::InUse);
    let user_app = ProcessSnapshot::fixed(Ok(&[("Electron", Some("/Users/a/Applications/visual studio code.app/Contents/MacOS/Electron"))]));
    assert_eq!(probe_owner(Some("microsoft.vscode"), &user_app).state, ProbeState::InUse, "bundle names compare case-insensitively");
    // Tools by executable name; a bundle name inside a file name is not a component.
    let tools = ProcessSnapshot::fixed(Ok(&[("cargo", Some("/Users/a/.cargo/bin/cargo")), ("x", Some("/tmp/Google Chrome.app.zip"))]));
    assert_eq!(probe_owner(Some("cargo"), &tools).state, ProbeState::InUse);
    assert_eq!(probe_owner(Some("go"), &tools).state, ProbeState::NotInUse);
    assert_eq!(probe_owner(Some("google.chrome"), &tools).state, ProbeState::NotInUse);
    // Without a readable executable path the process name is compared.
    let pathless = ProcessSnapshot::fixed(Ok(&[("Google Chrome", None), ("uv", None)]));
    assert_eq!(probe_owner(Some("google.chrome"), &pathless).state, ProbeState::InUse);
    assert_eq!(probe_owner(Some("uv"), &pathless).state, ProbeState::InUse);
    assert!(ProcessSnapshot::take().procs.is_ok_and(|p| p.iter().any(|p| p.pid == 1)), "the real snapshot lists launchd");
}

#[test]
fn interpreters_are_never_owner_processes() {
    for (owner, m) in OWNERS {
        for e in m.executables {
            assert!(!["node", "python", "python3", "java", "ruby", "sh", "bash", "zsh"].contains(e), "{owner}: {e}");
        }
        assert!(m.bundles.iter().all(|b| b.ends_with(".app")), "{owner}");
    }
}

/// A copy of `/bin/sleep` inside a fixture application bundle, running until dropped.
struct FixtureApp(Child);

impl FixtureApp {
    fn start(fx: &Fixture) -> FixtureApp {
        let exe = fx.mkdir("Fixture Owner.app/Contents/MacOS").join("fixture-owner");
        std::fs::copy("/bin/sleep", &exe).unwrap();
        FixtureApp(Command::new(&exe).arg("60").spawn().unwrap())
    }
}

impl Drop for FixtureApp {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_running_fixture_process_makes_its_owner_in_use() {
    const FIXTURE: OwnerMatch = OwnerMatch { bundles: &["Fixture Owner.app"], executables: &[] };
    let fx = Fixture::new("probe-owner");
    let app = FixtureApp::start(&fx);
    let pid = app.0.id();
    let mut seen = None;
    // The new process shows its executable path once exec has completed.
    for _ in 0..50 {
        let r = probe_match(&FIXTURE, &ProcessSnapshot::take());
        if r.state == ProbeState::InUse {
            seen = Some(r);
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let r = seen.expect("the fixture process is seen running");
    assert!(r.detail.contains(&format!("pid {pid}")) && r.detail.contains("Fixture Owner.app"), "{}", r.detail);
    drop(app);
    assert_eq!(probe_match(&FIXTURE, &ProcessSnapshot::take()).state, ProbeState::NotInUse, "gone once it exited");
}

#[test]
fn an_open_descriptor_makes_the_tree_in_use() {
    let fx = Fixture::new("probe-handles");
    fx.write("a.bin", b"x");
    let held_file = fx.write("sub/index", b"x");
    let snap = ProcessSnapshot::take();
    let free = probe_open_files(fx.path(), false, &snap);
    assert_ne!(free.state, ProbeState::InUse, "{free:?}");
    let h = std::fs::File::open(&held_file).unwrap();
    let r = probe_open_files(fx.path(), false, &ProcessSnapshot::take());
    assert_eq!(r.state, ProbeState::InUse, "{r:?}");
    assert!(r.detail.contains(&format!("pid {}", std::process::id())), "{}", r.detail);
    assert_ne!(probe_open_files(fx.path(), true, &ProcessSnapshot::take()).state, ProbeState::InUse, "database probe only looks at database/log files");
    drop(h);
    let db = fx.write("sub/state.db-wal", b"x");
    let h = std::fs::File::open(&db).unwrap();
    assert_eq!(probe_open_files(fx.path(), true, &ProcessSnapshot::take()).state, ProbeState::InUse);
    drop(h);
    // With nothing held the answer is not_in_use only when every process could be inspected.
    let r = probe_open_files(fx.path(), true, &ProcessSnapshot::take());
    assert_ne!(r.state, ProbeState::InUse, "{r:?}");
    if r.state == ProbeState::Unknown {
        assert!(r.detail.contains("could not be inspected"), "{}", r.detail);
    }
}

#[test]
fn a_missing_or_linked_root_is_unknown_not_free() {
    let fx = Fixture::new("probe-missing");
    let snap = ProcessSnapshot::take();
    assert_eq!(probe_open_files(&fx.path().join("nope"), false, &snap).state, ProbeState::Unknown);
    assert_eq!(probe_downloads(&fx.path().join("nope"), &snap).state, ProbeState::Unknown);
    let target = fx.mkdir("target");
    std::os::unix::fs::symlink(&target, fx.path().join("link")).unwrap();
    assert_eq!(probe_open_files(&fx.path().join("link"), false, &snap).state, ProbeState::Unknown);
}

#[test]
fn held_download_markers_are_in_use() {
    let fx = Fixture::new("probe-downloads");
    let marker = fx.write("setup.dmg.crdownload", b"x");
    let bundle_part = fx.write("Archive.zip.download/Archive.zip", b"x");
    fx.write("done.zip", b"x");
    let free = probe_downloads(fx.path(), &ProcessSnapshot::take());
    assert_ne!(free.state, ProbeState::InUse, "{free:?}");
    let h = std::fs::OpenOptions::new().append(true).open(&marker).unwrap();
    assert_eq!(probe_downloads(fx.path(), &ProcessSnapshot::take()).state, ProbeState::InUse);
    assert_eq!(probe_downloads(&marker, &ProcessSnapshot::take()).state, ProbeState::InUse, "a marker file itself can be the candidate");
    drop(h);
    let h = std::fs::File::open(&bundle_part).unwrap();
    assert_eq!(probe_downloads(&fx.path().join("Archive.zip.download"), &ProcessSnapshot::take()).state, ProbeState::InUse, "inside a Safari download bundle");
    drop(h);
    let unrelated = std::fs::File::open(fx.path().join("done.zip")).unwrap();
    assert_ne!(probe_downloads(fx.path(), &ProcessSnapshot::take()).state, ProbeState::InUse, "only markers count");
    drop(unrelated);
}

#[test]
fn endpoint_security_locations_are_in_use() {
    for p in ["/Library/Application Support/CrowdStrike/Falcon", "/Library/Application Support/JAMF/bin", "/Users/a/Library/Caches/com.microsoft.wdav/x"] {
        assert_eq!(probe_endpoint(Path::new(p)).state, ProbeState::InUse, "{p}");
    }
    assert_eq!(probe_endpoint(Path::new("/Users/a/Library/Caches/Google/Chrome/Default/Code Cache")).state, ProbeState::NotInUse);
}

#[test]
fn every_declared_probe_is_answered_and_unknown_kinds_are_unknown() {
    let fx = Fixture::new("probe-all");
    fx.write("x", b"x");
    let raw = RawPath::unix(fx.path().as_os_str().as_bytes().to_vec()).unwrap();
    let kinds: Vec<String> = ["owner_process", "open_file_handle", "vcs_tracked", "owner_process", "elevated_token"].iter().map(|s| s.to_string()).collect();
    let r = probe_candidate(&raw, &kinds, Some("google.chrome"), &ProcessSnapshot::fixed(Ok(&[("idle", Some("/usr/bin/idle"))])));
    let by: Vec<&str> = r.iter().map(|p| p.probe.as_str()).collect();
    assert_eq!(by, ["elevated_token", "open_file_handle", "owner_process", "vcs_tracked"], "sorted, once each");
    assert_eq!(r[2].state, ProbeState::NotInUse);
    assert_eq!(r[3].state, ProbeState::Unknown);
    assert_eq!(r[0].state, if is_root() { ProbeState::NotInUse } else { ProbeState::Unknown }, "no elevation path on macOS");
    let win = RawPath::windows("C:\\x".encode_utf16().collect()).unwrap();
    assert!(probe_candidate(&win, &kinds, None, &ProcessSnapshot::fixed(Ok(&[]))).iter().all(|p| p.state == ProbeState::Unknown));
}

#[test]
fn every_shipped_owner_with_an_owner_probe_has_a_process_mapping() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rules/macos");
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let rule = cm_core::rules::Rule::parse(&std::fs::read_to_string(&p).unwrap()).unwrap();
        if rule.probes.iter().any(|k| k == "owner_process") {
            assert!(owner_match(&rule.owner.id).is_some(), "{} declares owner_process but {} has no mapping", rule.rule_id, rule.owner.id);
        }
    }
}
