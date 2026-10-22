use std::cell::Cell;

use super::*;
use crate::execute::{ApplyContext, ApplyError};
use crate::plans::{V2Backends, apply_plan_v2, executable};
use cm_core::authz::{KindReceipt, ReceiptSource};
use cm_core::execute::{ActionOutcome, ActionTarget, FileActions, LiveCheck};
use cm_core::host::HostRef;
use cm_core::path::RawPath;
use cm_core::process::{ProcessFacts, Protection, Signature, SignatureState, explain};
use cm_store::Stores;

const T0: i64 = 1_800_000_000;

fn facts(pid: u32, name: &str, cpu: f64) -> ProcessFacts {
    ProcessFacts {
        pid,
        parent_pid: None,
        name: name.into(),
        image_path: RawPath::windows(format!(r"C:\Tools\{name}").encode_utf16().collect()).ok(),
        started_at: Some("134050000000000000".into()),
        started_at_utc: Some("2025-10-15T11:06:40Z".into()),
        cpu_percent: Some(cpu),
        memory_bytes: Some(1),
        disk_bytes_per_sec: None,
        network_bytes_per_sec: None,
        signature: Signature { state: SignatureState::Unsigned, publisher: None, reason: None },
        os_critical: Some(false),
        notes: vec![],
    }
}

#[test]
fn pins_persist_and_float_to_the_top() {
    let d = std::env::temp_dir().join(format!("cm-app-pins-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    assert!(load_pins(&d).is_empty());
    assert_eq!(set_pin(&d, "Code.exe", true).unwrap(), ["Code.exe"]);
    assert_eq!(set_pin(&d, "code.EXE", true).unwrap(), ["code.EXE"], "case-insensitive, no duplicates");
    assert!(set_pin(&d, r"C:\x\a.exe", true).is_err());
    let rows = explain(vec![facts(10, "busy.exe", 90.0), facts(11, "code.exe", 1.0), facts(12, "idle.exe", 0.0)], 1, r"C:\Windows", &[], &load_pins(&d));
    let w = list_wire(rows.clone(), SortKey::Cpu, None, 2, "t", 500, &[]);
    let pids: Vec<u64> = w["items"].as_array().unwrap().iter().map(|i| i["pid"].as_u64().unwrap()).collect();
    assert_eq!(pids, [11, 10], "pinned first, then CPU, limited to 2");
    assert_eq!((w["total"].as_u64(), w["matched"].as_u64(), w["unidentified"].as_u64()), (Some(3), Some(3), Some(2)));
    let w = list_wire(rows, SortKey::Cpu, Some("idle"), 50, "t", 500, &[]);
    assert_eq!(w["matched"], 2, "the search keeps pinned rows");
    assert_eq!(set_pin(&d, "code.exe", false).unwrap(), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(&d);
}

struct Ender {
    ended: Cell<u32>,
    outcome: EndOutcome,
}

impl ProcessActions for Ender {
    fn end(&self, _: &ProcessEndAction, own_pid: u32) -> EndOutcome {
        assert_eq!(own_pid, std::process::id());
        self.ended.set(self.ended.get() + 1);
        self.outcome.clone()
    }
    fn running(&self, _: &ProcessEndAction) -> Option<bool> {
        Some(self.ended.get() == 0)
    }
}

struct NoFiles;

impl FileActions for NoFiles {
    fn check(&self, _: &ActionTarget<'_>) -> LiveCheck {
        unreachable!()
    }
    fn quarantine(&self, _: &ActionTarget<'_>, _: &RawPath, _: &str) -> ActionOutcome {
        unreachable!()
    }
    fn trash(&self, _: &ActionTarget<'_>) -> ActionOutcome {
        unreachable!()
    }
    fn delete(&self, _: &ActionTarget<'_>) -> ActionOutcome {
        unreachable!()
    }
    fn quarantine_dir_for(&self, _: &ActionTarget<'_>, _: &str) -> Option<RawPath> {
        None
    }
    fn reconcile(&self, _: &ActionTarget<'_>, _: cm_core::rules::Mode, _: Option<&RawPath>) -> Reconciled {
        Reconciled::StillUnknown
    }
    fn free_space(&self, _: &RawPath) -> Option<u64> {
        None
    }
}

/// STS-04: ending needs a reviewed plan of exactly one process; protected processes never get a plan.
#[test]
fn end_plans_need_review_and_protected_rows_get_none() {
    let rows = explain(vec![facts(10, "busy.exe", 90.0), facts(11, "lsass.exe", 1.0)], 1, r"C:\Windows", &[], &[]);
    assert!(matches!(end_plan(&rows[1], None, "pp_x", T0, NO_RULES_DIGEST), Err(EndPlanError::Refused(EndRefusal::Protected(Protection::CriticalSystem)))));
    let plan = end_plan(&rows[0], None, "pp_1", T0, NO_RULES_DIGEST).unwrap();
    assert_eq!((plan.body.plan_kind, plan.body.items.len()), (PlanKind::Process, 1));
    assert!(plan.meta[0].note.as_deref().unwrap().contains("busy.exe (PID 10, started 2025-10-15T11:06:40Z): unidentified"), "{:?}", plan.meta[0].note);
    // WFA-93: the action binds the exact start key and also carries the UTC time people read.
    let PlanAction::ProcessEnd(a) = &plan.body.items[0] else { panic!("one process_end") };
    assert_eq!((a.started_at.as_str(), a.started_at_utc.as_deref()), ("134050000000000000", Some("2025-10-15T11:06:40Z")));
    assert!(executable(PlanKind::Process));
    let dir = std::env::temp_dir().join(format!("cm-app-proc-plan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let stores = Stores::open(&dir).unwrap();
    stores.audit.save_plan_v2(&plan).unwrap();
    let ender = Ender { ended: Cell::new(0), outcome: EndOutcome::Ended };
    let b = V2Backends { files: &NoFiles, startup: None, holders: &|_| None, protect_item: &|_| Ok(()), processes: Some(&ender), updates: None, uninstall: None };
    let h = HostRef { kind: "cli".into(), pid: 1, started_at: "2026-10-15T00:00:00Z".into() };
    let ctx = ApplyContext { principal: "user:a", host: &h, current_rule_set_digest: "", now: &|| T0 + 5, cancel: &|| false, fault: &|_, _| false, protect: &|_| Ok(()) };
    assert!(matches!(apply_plan_v2(&stores, &b, "pp_1", &ctx), Err(ApplyError::NeedsApproval(_))));
    assert_eq!(ender.ended.get(), 0);
    stores
        .audit
        .save_kind_receipt(&KindReceipt {
            receipt_id: "r".into(),
            plan_id: "pp_1".into(),
            plan_digest: plan.digest.clone(),
            plan_kind: PlanKind::Process,
            principal: "user:a".into(),
            source: ReceiptSource::InteractiveReview,
            created_at: plan.body.created_at.clone(),
            expires_at: plan.body.expires_at.clone(),
            revoked_at: None,
        })
        .unwrap();
    let report = apply_plan_v2(&stores, &b, "pp_1", &ctx).unwrap();
    assert_eq!((report.status.as_str(), report.items[0].code.as_str()), ("completed", "ended"));
    assert_eq!(ender.ended.get(), 1);
    drop(stores);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn end_outcomes_map_to_results() {
    let a = ProcessEndAction { action_id: "a01".into(), pid: 5, started_at: "1".into(), started_at_utc: None, name: "x.exe".into(), image_path: None, image_sha256: None };
    let r = |o: EndOutcome| run_end(Some(&Ender { ended: Cell::new(0), outcome: o }), &a);
    assert_eq!(r(EndOutcome::Protected(Protection::SecuritySoftware)).code, "protected");
    assert_eq!(r(EndOutcome::Changed("pid reused".into())).status, ActionStatus::Skipped);
    assert_eq!(r(EndOutcome::AlreadyExited).code, "precondition_gone");
    assert_eq!(r(EndOutcome::Unknown("?".into())).status, ActionStatus::Unknown);
    assert_eq!(run_end(None, &a).code, "unsupported");
    assert_eq!(reconcile_end(Some(&Ender { ended: Cell::new(0), outcome: EndOutcome::Ended }), &a), Reconciled::NotPerformed);
    assert_eq!(reconcile_end(None, &a), Reconciled::StillUnknown);
}
