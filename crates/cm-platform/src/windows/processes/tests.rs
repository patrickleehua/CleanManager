//! Only a child process spawned by the test itself is ever ended here. Protection tests use that child's PID with a
//! protected name, so even a broken check could not end anything else.

use std::process::{Child, Command};
use std::time::Duration;

use super::*;
use cm_core::process::{explain, protection};

fn sleeper() -> Child {
    // PING.EXE with a long count: a harmless child that runs until ended.
    Command::new(r"C:\Windows\System32\PING.EXE").args(["-n", "60", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().expect("spawn child")
}

fn facts_of(pid: u32) -> ProcessFacts {
    list(Duration::from_millis(150)).unwrap().into_iter().find(|f| f.pid == pid).expect("child listed")
}

fn action_for(f: &ProcessFacts) -> ProcessEndAction {
    ProcessEndAction {
        action_id: "a01".into(),
        pid: f.pid,
        started_at: f.started_at.clone().unwrap(),
        started_at_utc: f.started_at_utc.clone(),
        name: f.name.clone(),
        image_path: f.image_path.clone(),
        image_sha256: f.image_path.as_ref().and_then(|p| file_sha256(&p.display())),
    }
}

#[test]
fn list_reads_own_process_and_marks_critical_ones() {
    let own = std::process::id();
    let all = list(Duration::from_millis(200)).unwrap();
    assert!(all.len() > 20);
    let me = all.iter().find(|f| f.pid == own).expect("own process listed");
    assert!(me.image_path.is_some() && me.started_at.is_some() && me.memory_bytes.is_some_and(|m| m > 0));
    assert!(me.cpu_percent.is_some());
    assert!(me.network_bytes_per_sec.is_none(), "network rate is unknown, never 0");
    // WFA-93: the FILETIME key stays for identity; people read the same instant as RFC 3339 UTC.
    let key: u64 = me.started_at.as_deref().unwrap().parse().unwrap();
    let utc = me.started_at_utc.as_deref().expect("start time in UTC");
    assert_eq!(utc, start_utc(key));
    assert!(utc.len() == 20 && utc.ends_with('Z') && utc.as_bytes()[10] == b'T', "{utc}");
    let started = cm_core::time::filetime_to_unix(key);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    assert!((now - 86_400 * 2..=now + 5).contains(&started), "this test process started moments ago: {utc}");
    assert_eq!(start_utc(134_371_365_203_426_535), "2026-10-22T09:55:20Z", "the acceptance run's raw value");
    let views = explain(all.clone(), own, &windows_dir(), &[], &[]);
    for name in ["csrss.exe", "wininit.exe", "services.exe", "lsass.exe", "smss.exe", "System"] {
        if let Some(v) = views.iter().find(|v| v.facts.name.eq_ignore_ascii_case(name)) {
            assert!(v.protection.is_some(), "{name} must be protected");
        }
    }
    assert!(views.iter().any(|v| v.facts.name.eq_ignore_ascii_case("csrss.exe")), "csrss is always present");
    let me = views.iter().find(|v| v.facts.pid == own).unwrap();
    assert_eq!(me.protection, Some(Protection::CleanManager));
}

#[test]
fn in_box_binaries_verify_through_the_catalog_or_embedded_signature() {
    let s = signature_of(&format!(r"{}\System32\notepad.exe", windows_dir()));
    assert!(matches!(s.state, SignatureState::Signed | SignatureState::CatalogSigned), "{s:?}");
    assert!(s.publisher.as_deref().is_some_and(|p| p.starts_with("Microsoft")), "{s:?}");
    let missing = signature_of(r"C:\definitely\not\here.exe");
    assert_eq!(missing.state, SignatureState::Unknown);
}

/// STS-04: a reviewed end of the test's own child ends exactly that process.
#[test]
fn ends_a_spawned_child_after_reidentifying_it() {
    let mut child = sleeper();
    let f = facts_of(child.id());
    assert_eq!(protection(&f, std::process::id()), None);
    let a = action_for(&f);
    assert_eq!(WindowsProcesses.running(&a), Some(true));
    assert_eq!(WindowsProcesses.end(&a, std::process::id()), EndOutcome::Ended);
    assert!(child.wait().is_ok());
    assert_eq!(WindowsProcesses.running(&a), Some(false));
    assert!(matches!(WindowsProcesses.end(&a, std::process::id()), EndOutcome::AlreadyExited | EndOutcome::Changed(_)));
}

/// A different start time (reused PID), a different executable, a protected name or our own PID: nothing is ended.
#[test]
fn refuses_changed_identity_protected_names_and_self() {
    let mut child = sleeper();
    let f = facts_of(child.id());
    let own = std::process::id();
    let mut reused = action_for(&f);
    reused.started_at = "1".into();
    assert!(matches!(WindowsProcesses.end(&reused, own), EndOutcome::Changed(_)));
    let mut other_exe = action_for(&f);
    other_exe.image_path = RawPath::windows(r"C:\Windows\System32\notepad.exe".encode_utf16().collect()).ok();
    assert!(matches!(WindowsProcesses.end(&other_exe, own), EndOutcome::Changed(_)));
    let mut hash = action_for(&f);
    hash.image_sha256 = Some("0".repeat(64));
    assert!(matches!(WindowsProcesses.end(&hash, own), EndOutcome::Changed(_)));
    for name in ["csrss.exe", "lsass.exe", "winlogon.exe", "MsMpEng.exe", "cleanmanager.exe"] {
        let mut protected = action_for(&f);
        protected.name = name.into();
        assert!(matches!(WindowsProcesses.end(&protected, own), EndOutcome::Protected(_)), "{name}");
    }
    let mut me = action_for(&f);
    me.pid = own;
    assert_eq!(WindowsProcesses.end(&me, own), EndOutcome::Protected(Protection::CleanManager));
    assert_eq!(WindowsProcesses.running(&action_for(&f)), Some(true), "the child is still running");
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn running_under_finds_our_own_directory() {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().to_string_lossy().into_owned();
    assert_eq!(running_under(&dir), Some(true));
    assert_eq!(running_under(r"C:\definitely\not\installed"), Some(false));
}
