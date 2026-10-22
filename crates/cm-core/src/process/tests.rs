use super::*;
use crate::plan::v2::{HeaderInput, KindError, NO_RULES_DIGEST, PlanAction, PlanKind, build, validate};

fn raw(p: &str) -> RawPath {
    RawPath::windows(p.encode_utf16().collect()).unwrap()
}

fn facts(pid: u32, name: &str, path: Option<&str>, signer: Option<&str>) -> ProcessFacts {
    ProcessFacts {
        pid,
        parent_pid: Some(1000),
        name: name.into(),
        image_path: path.map(raw),
        started_at: Some("134050000000000000".into()),
        started_at_utc: Some("2025-10-15T11:06:40Z".into()),
        cpu_percent: Some(1.0),
        memory_bytes: Some(1 << 20),
        disk_bytes_per_sec: Some(0.0),
        network_bytes_per_sec: None,
        signature: match signer {
            Some(s) => Signature { state: SignatureState::CatalogSigned, publisher: Some(s.into()), reason: None },
            None => Signature { state: SignatureState::Unsigned, publisher: None, reason: None },
        },
        os_critical: Some(false),
        notes: vec![],
    }
}

const WIN: &str = r"C:\Windows";

fn view(f: ProcessFacts) -> ProcessView {
    explain(vec![f], 1, WIN, &[], &[]).remove(0)
}

/// STS-04: an unfamiliar process shows local facts, is marked unidentified and carries the AI diagnosis entry.
#[test]
fn unknown_process_is_unidentified_with_ai_entry() {
    let v = view(facts(4321, "weird-helper.exe", Some(r"C:\Users\A\AppData\Local\Temp\weird-helper.exe"), None));
    assert!(!v.identified());
    let w = v.to_wire();
    assert_eq!(w["identified"], false);
    assert!(w["purpose"].is_null());
    assert_eq!(w["ai_diagnosis"]["available"], true);
    assert!(w["ai_diagnosis"]["hint"].as_str().unwrap().contains("AI diagnosis"));
    assert_eq!(w["image_path"]["display"], r"C:\Users\A\AppData\Local\Temp\weird-helper.exe");
    assert_eq!(w["parent_pid"], 1000);
    assert_eq!(w["signature"]["state"], "unsigned");
    assert!(w["network_bytes_per_sec"].is_null(), "unknown stays null, never 0");
    assert_eq!(w["end_capability"]["state"], "available");
}

#[test]
fn known_processes_need_name_location_and_signer_to_agree() {
    let ok = view(facts(800, "svchost.exe", Some(r"C:\WINDOWS\System32\svchost.exe"), Some("Microsoft Windows")));
    assert!(ok.identified());
    assert!(ok.to_wire()["ai_diagnosis"].is_null());
    let elsewhere = view(facts(801, "svchost.exe", Some(r"C:\Users\A\svchost.exe"), Some("Microsoft Windows")));
    assert!(!elsewhere.identified());
    assert!(elsewhere.unidentified_reason.unwrap().contains("unexpected location"));
    let unsigned = view(facts(802, "svchost.exe", Some(r"C:\Windows\System32\svchost.exe"), None));
    assert!(!unsigned.identified());
    let other_signer = view(facts(803, "chrome.exe", Some(r"C:\X\chrome.exe"), Some("Evil Corp")));
    assert!(other_signer.unidentified_reason.unwrap().contains("signed by Evil Corp"));
    let explorer = view(facts(804, "explorer.exe", Some(r"c:\windows\explorer.exe"), Some("Microsoft Windows")));
    assert!(explorer.identified());
    let wmi = view(facts(805, "WmiPrvSE.exe", Some(r"C:\Windows\System32\wbem\WmiPrvSE.exe"), Some("Microsoft Windows")));
    assert!(wmi.identified());
}

/// STS-04: system-critical processes, CleanManager itself and security software can never be ended.
#[test]
fn critical_self_and_security_processes_are_protected() {
    for name in ["csrss.exe", "wininit.exe", "lsass.exe", "smss.exe", "services.exe", "winlogon.exe", "System", "Registry"] {
        let v = view(facts(900, name, Some(r"C:\Windows\System32\x.exe"), Some("Microsoft Windows")));
        assert_eq!(v.protection, Some(Protection::CriticalSystem), "{name}");
        assert_eq!(end_action(&v, None), Err(EndRefusal::Protected(Protection::CriticalSystem)));
        assert_eq!(v.to_wire()["end_capability"]["state"], "unsupported");
    }
    assert_eq!(view(facts(4, "anything", None, None)).protection, Some(Protection::CriticalSystem));
    let own = explain(vec![facts(77, "renamed.exe", None, None)], 77, WIN, &[], &[]).remove(0);
    assert_eq!(own.protection, Some(Protection::CleanManager));
    assert_eq!(view(facts(78, "cleanmanager.exe", None, None)).protection, Some(Protection::CleanManager));
    assert_eq!(view(facts(79, "MsMpEng.exe", None, Some("Microsoft Corporation"))).protection, Some(Protection::SecuritySoftware));
    assert_eq!(view(facts(80, "agent.exe", None, Some("CrowdStrike, Inc."))).protection, Some(Protection::SecuritySoftware));
    let mut flagged = facts(81, "vendor-critical.exe", None, None);
    flagged.os_critical = Some(true);
    assert_eq!(view(flagged).protection, Some(Protection::OsCritical));
    assert_eq!(view(facts(82, "notepad.exe", None, None)).protection, None);
}

#[test]
fn end_action_binds_one_instance_and_plans_hold_exactly_one() {
    let v = view(facts(4321, "notepad.exe", Some(r"C:\Windows\System32\notepad.exe"), None));
    let a = end_action(&v, Some("ab".repeat(32))).unwrap();
    assert_eq!((a.pid, a.started_at.as_str(), a.name.as_str()), (4321, "134050000000000000", "notepad.exe"));
    // WFA-93: the exact start key binds the instance; the plan also carries the same time for people.
    assert_eq!(a.started_at_utc.as_deref(), Some("2025-10-15T11:06:40Z"));
    assert_eq!(v.to_wire()["started_at_utc"], "2025-10-15T11:06:40Z");
    let old = serde_json::json!({"action_id": "a01", "pid": 1, "started_at": "1", "name": "x.exe", "image_path": null, "image_sha256": null});
    let parsed: ProcessEndAction = serde_json::from_value(old.clone()).expect("plans made before started_at_utc still load");
    assert_eq!(serde_json::to_value(&parsed).unwrap(), old, "and serialize exactly as before, so their digests hold");
    let mut no_time = v.clone();
    no_time.facts.started_at = None;
    assert_eq!(end_action(&no_time, None), Err(EndRefusal::NoIdentity));
    let header = HeaderInput {
        plan_id: "p_proc",
        plan_kind: PlanKind::Process,
        created_unix: 1_800_000_000,
        scope_id: None,
        scan_id: None,
        scan_revision: None,
        rule_set_digest: NO_RULES_DIGEST,
        policy_digest: NO_RULES_DIGEST,
    };
    let plan = build(&header, vec![PlanAction::ProcessEnd(a.clone())], vec![Default::default()]).unwrap();
    assert!(!plan.body.policy_matchable && !plan.body.requires_admin);
    let mut two = plan.body.clone();
    let mut b = a.clone();
    b.action_id = "a02".into();
    two.items.push(PlanAction::ProcessEnd(b));
    assert_eq!(validate(&two), Err(KindError::ProcessNotSingle));
    let wire = serde_json::to_value(&plan.body.items[0]).unwrap();
    assert_eq!(wire["action_type"], "process_end");
}

#[test]
fn sort_search_and_pins() {
    let mut a = facts(1, "alpha.exe", None, None);
    a.cpu_percent = Some(5.0);
    a.memory_bytes = Some(10);
    let mut b = facts(2, "beta.exe", None, None);
    b.cpu_percent = Some(50.0);
    b.memory_bytes = None;
    let mut c = facts(3, "gamma.exe", Some(r"C:\Tools\gamma.exe"), Some("Gamma Soft"));
    c.cpu_percent = None;
    c.memory_bytes = Some(99);
    let apps = vec![("Gamma Suite".to_owned(), r"C:\Tools".to_owned())];
    let mut rows = explain(vec![a, b, c], 99, WIN, &apps, &["ALPHA.EXE".into()]);
    assert_eq!(rows[2].owner_app.as_deref(), Some("Gamma Suite"));
    sort(&mut rows, SortKey::Cpu);
    assert_eq!(rows.iter().map(|r| r.facts.pid).collect::<Vec<_>>(), [1, 2, 3], "pinned first, unknown CPU last");
    sort(&mut rows, SortKey::Memory);
    assert_eq!(rows.iter().map(|r| r.facts.pid).collect::<Vec<_>>(), [1, 3, 2]);
    rows.iter_mut().for_each(|r| r.pinned = false);
    sort(&mut rows, SortKey::Cpu);
    assert_eq!(rows.iter().map(|r| r.facts.pid).collect::<Vec<_>>(), [2, 1, 3]);
    let hits: Vec<u32> = rows.iter().filter(|r| matches(r, "gamma soft")).map(|r| r.facts.pid).collect();
    assert_eq!(hits, [3]);
    assert_eq!(rows.iter().filter(|r| matches(r, "2")).count(), 1);
    assert_eq!(SortKey::parse("disk"), Some(SortKey::Disk));
    assert_eq!(SortKey::parse("io"), None);
}

#[test]
fn owner_app_needs_a_path_boundary() {
    let apps = vec![("Short".to_owned(), r"C:\App".to_owned()), ("Long".to_owned(), r"C:\App\Sub\".to_owned())];
    assert_eq!(owner_app(Some(&raw(r"C:\App\Sub\x.exe")), &apps), Some("Long"));
    assert_eq!(owner_app(Some(&raw(r"C:\Apple\x.exe")), &apps), None);
    assert_eq!(owner_app(None, &apps), None);
}
