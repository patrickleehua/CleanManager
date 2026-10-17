use std::os::unix::process::ExitStatusExt;

use super::*;

fn is_root() -> bool {
    own_uid() == 0
}

fn fake(pid: u32, name: &str, path: Option<&str>, uid: u32, sig: Signature, apple: bool) -> MacProcess {
    MacProcess {
        facts: ProcessFacts {
            pid,
            parent_pid: Some(1),
            name: name.into(),
            image_path: path.map(|p| RawPath::unix(p.as_bytes().to_vec()).unwrap()),
            started_at: Some("1700000000000000".into()),
            cpu_percent: None,
            memory_bytes: None,
            disk_bytes_per_sec: None,
            network_bytes_per_sec: None,
            signature: sig,
            os_critical: None,
            notes: vec![],
        },
        uid,
        zombie: false,
        authority: None,
        apple,
    }
}

fn apple_sig() -> Signature {
    Signature { state: SignatureState::Signed, publisher: Some("Apple".into()), reason: None }
}

#[test]
fn knowledge_needs_name_location_and_signer_to_agree() {
    let me = own_uid();
    assert!(identify(&fake(1, "launchd", Some("/sbin/launchd"), 0, apple_sig(), true)).is_ok());
    let moved = identify(&fake(900, "WindowServer", Some("/tmp/WindowServer"), me, apple_sig(), true)).unwrap_err();
    assert!(moved.contains("unexpected location"), "{moved}");
    let unsigned = Signature { state: SignatureState::Unsigned, publisher: None, reason: None };
    let fake_finder = identify(&fake(901, "Finder", Some("/System/Library/CoreServices/Finder.app/Contents/MacOS/Finder"), me, unsigned, false)).unwrap_err();
    assert!(fake_finder.contains("not signed by its vendor"), "{fake_finder}");
    let chrome = Signature { state: SignatureState::Signed, publisher: Some("EQHXZ8M8AV".into()), reason: None };
    let path = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
    assert!(identify(&fake(902, "Google Chrome", Some(path), me, chrome.clone(), false)).is_ok());
    let other_team = Signature { publisher: Some("ABCDE12345".into()), ..chrome };
    assert!(identify(&fake(903, "Google Chrome", Some(path), me, other_team, false)).is_err());
    assert!(identify(&fake(904, "mystery-helper", Some("/tmp/x"), me, Signature::unknown("x"), false)).unwrap_err().contains("Not in"));
}

#[test]
fn protection_covers_critical_self_other_users_and_security_software() {
    let me = own_uid();
    let other = if me == 0 { 501 } else { 0 };
    let own_pid = std::process::id();
    let sig = || Signature::unknown("x");
    assert_eq!(protection(&fake(1, "launchd", None, 0, sig(), false), own_pid, me), Some(Protection::CriticalSystem));
    for n in ["WindowServer", "loginwindow", "kernel_task"] {
        assert_eq!(protection(&fake(500, n, None, me, sig(), false), own_pid, me), Some(Protection::CriticalSystem), "{n}");
    }
    assert_eq!(protection(&fake(own_pid, "whatever", None, me, sig(), false), own_pid, me), Some(Protection::CleanManager));
    assert_eq!(protection(&fake(501, "cleanmanager", None, me, sig(), false), own_pid, me), Some(Protection::CleanManager));
    assert_eq!(protection(&fake(502, "mdworker_shared", None, other, sig(), false), own_pid, me), Some(Protection::OtherUser));
    let mut av = fake(503, "agent", None, me, sig(), false);
    av.authority = Some("Developer ID Application: CrowdStrike Inc. (X9E956P446)".into());
    assert_eq!(protection(&av, own_pid, me), Some(Protection::SecuritySoftware));
    assert_eq!(protection(&fake(504, "sleep", None, me, sig(), false), own_pid, me), None);
    assert!(Protection::CriticalSystem.reason().contains("macOS"));
}

#[test]
fn owner_app_is_the_outermost_bundle() {
    assert_eq!(owner_app("/Applications/Google Chrome.app/Contents/Frameworks/G.framework/Helpers/Google Chrome Helper.app/Contents/MacOS/Google Chrome Helper").as_deref(), Some("Google Chrome"));
    assert_eq!(owner_app("/usr/bin/sleep"), None);
    assert_eq!(owner_app("/x/.app/y"), None);
}

#[test]
fn the_list_reads_this_process_and_withholds_other_users_usage() {
    let rows = list_with(Duration::from_millis(300), true).unwrap();
    let me = rows.iter().find(|r| r.facts.pid == std::process::id()).expect("this process is listed");
    assert!(me.facts.cpu_percent.is_some_and(|c| (0.0..=100.0).contains(&c)), "{me:?}");
    assert!(me.facts.memory_bytes.is_some_and(|m| m > 0), "{me:?}");
    assert!(me.facts.started_at.as_deref().is_some_and(|s| s.len() >= 16 && s.bytes().all(|b| b.is_ascii_digit())), "{me:?}");
    assert!(me.facts.network_bytes_per_sec.is_none());
    eprintln!("this test binary: {:?}", me.facts.signature);
    let launchd = rows.iter().find(|r| r.facts.pid == 1).expect("launchd");
    eprintln!("launchd: path={:?} signature={:?} notes={:?}", launchd.facts.image_path.as_ref().map(RawPath::display), launchd.facts.signature, launchd.facts.notes);
    if !is_root() {
        let others: Vec<&MacProcess> = rows.iter().filter(|r| r.uid != own_uid() && !r.zombie).collect();
        assert!(!others.is_empty(), "system daemons run as other users");
        for o in others {
            assert!(o.facts.cpu_percent.is_none() && o.facts.memory_bytes.is_none(), "withheld, never 0: {o:?}");
            assert!(o.facts.notes.iter().any(|n| n == OTHER_USER_NOTE), "{o:?}");
        }
    }
    let views = explain(rows, std::process::id(), own_uid(), &["launchd".into()]);
    let l = views.iter().find(|v| v.facts.pid == 1).unwrap();
    assert_eq!(l.protection, Some(Protection::CriticalSystem));
    assert!(l.pinned);
    let w = l.to_wire();
    assert_eq!(w["end_capability"]["protection"], "critical_system");
    for v in &views {
        assert_eq!(v.identified(), v.to_wire()["ai_diagnosis"].is_null(), "unidentified rows carry the AI entry");
    }
    let identified: Vec<&str> = views.iter().filter(|v| v.identified()).map(|v| v.facts.name.as_str()).collect();
    eprintln!("identified: {identified:?}");
}

fn spawn_sleep() -> std::process::Child {
    std::process::Command::new("/bin/sleep").arg("60").spawn().unwrap()
}

/// End by re-identification: wrong start time or a protected target is refused; the planned child gets SIGTERM.
#[test]
fn ending_a_spawned_child_re_identifies_it_first() {
    let mut child = spawn_sleep();
    let pid = child.id();
    let row = view(pid).unwrap().expect("child listed");
    assert_eq!(row.protection, None, "{row:?}");
    let action = cm_core::process::end_action(&row, row.facts.image_path.as_ref().and_then(file_sha256)).unwrap();
    assert!(action.image_sha256.is_some());
    let b = MacProcesses;
    assert_eq!(b.running(&action), Some(true));
    let stale = ProcessEndAction { started_at: "1".into(), ..action.clone() };
    assert!(matches!(b.end(&stale, std::process::id()), EndOutcome::Changed(_)));
    let moved = ProcessEndAction { image_path: Some(RawPath::unix(b"/bin/cat".to_vec()).unwrap()), ..action.clone() };
    assert!(matches!(b.end(&moved, std::process::id()), EndOutcome::Changed(_)));
    let tampered = ProcessEndAction { image_sha256: Some("00".repeat(32)), ..action.clone() };
    assert!(matches!(b.end(&tampered, std::process::id()), EndOutcome::Changed(_)));
    assert!(child.try_wait().unwrap().is_none(), "nothing was signalled yet");
    assert_eq!(b.end(&action, std::process::id()), EndOutcome::Ended);
    let status = child.wait().unwrap();
    assert_eq!(status.signal(), Some(libc::SIGTERM));
    assert_eq!(b.running(&action), Some(false));
    assert_eq!(b.end(&action, std::process::id()), EndOutcome::AlreadyExited);
}

#[test]
fn protected_targets_are_refused_at_execution() {
    let b = MacProcesses;
    let a = |pid: u32, name: &str| ProcessEndAction { action_id: "a01".into(), pid, started_at: "0".into(), name: name.into(), image_path: None, image_sha256: None };
    assert_eq!(b.end(&a(1, "launchd"), std::process::id()), EndOutcome::Protected(Protection::CriticalSystem));
    assert_eq!(b.end(&a(4242, "WindowServer"), std::process::id()), EndOutcome::Protected(Protection::CriticalSystem));
    assert_eq!(b.end(&a(std::process::id(), "x"), std::process::id()), EndOutcome::Protected(Protection::CleanManager));
    if !is_root() {
        // A live process of another user (a root daemon) is refused before any signal, whatever the plan claims.
        let rows = list_with(Duration::from_millis(0), false).unwrap();
        // The oldest such process (lowest pid, a launchd daemon started at boot) stays alive; short-lived ones may exit.
        let other = rows
            .iter()
            .filter(|r| r.uid != own_uid() && r.facts.pid > 1 && r.facts.parent_pid == Some(1) && !r.zombie && !CRITICAL_NAMES.contains(&r.facts.name.as_str()))
            .min_by_key(|r| r.facts.pid)
            .expect("another user's process");
        let action = a(other.facts.pid, &other.facts.name);
        assert_eq!(b.end(&action, std::process::id()), EndOutcome::Protected(Protection::OtherUser), "{other:?}");
    }
}

#[test]
fn capability_row_is_unverified_with_a_reason() {
    assert_eq!((CAPS[0].0, CAPS[0].1), ("process.end", CapabilityState::Unverified));
}
