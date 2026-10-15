use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use super::*;
use cm_core::authz::{Receipt, ReceiptSource};
use cm_core::plan::{Plan, PlanBody, StaleReason, WireIdentity};
use cm_core::recovery::MetadataCoverage;

const T0: i64 = 1_800_000_000;
const PRINCIPAL: &str = "user:alice";

/// In-memory volume: quarantine location → file id, original path → file id.
#[derive(Default)]
struct Fake {
    quarantine: RefCell<BTreeMap<String, String>>,
    originals: RefCell<BTreeMap<String, String>>,
    offline: Cell<bool>,
    calls: Cell<u32>,
    unknown_on: RefCell<Option<String>>,
}

impl Fake {
    fn source(&self, o: &QuarantinedObject<'_>) -> Result<(), RecoveryOutcome> {
        let skip = |code: &str| Err(RecoveryOutcome::Skipped { code: code.into(), message: code.into() });
        if self.offline.get() {
            return skip("volume_offline");
        }
        match self.quarantine.borrow().get(&o.location.lossy()) {
            None => skip("missing"),
            Some(id) if *id != o.identity.file_id => skip("identity_changed"),
            Some(_) => Ok(()),
        }
    }
}

impl RecoveryActions for Fake {
    fn probe(&self, o: &QuarantinedObject<'_>) -> QuarantineState {
        match self.source(o) {
            Err(RecoveryOutcome::Skipped { code, .. }) if code == "volume_offline" => QuarantineState::VolumeOffline,
            Err(RecoveryOutcome::Skipped { code, .. }) if code == "missing" => QuarantineState::Missing,
            Err(_) => QuarantineState::IdentityChanged,
            Ok(()) if self.originals.borrow().contains_key(&o.original_path.lossy()) => QuarantineState::TargetConflict,
            Ok(()) => QuarantineState::Recoverable,
        }
    }
    fn restore(&self, o: &QuarantinedObject<'_>) -> RecoveryOutcome {
        self.calls.set(self.calls.get() + 1);
        if let Err(skip) = self.source(o) {
            return skip;
        }
        if self.originals.borrow().contains_key(&o.original_path.lossy()) {
            return RecoveryOutcome::Skipped { code: "destination_exists".into(), message: "occupied".into() };
        }
        let id = self.quarantine.borrow_mut().remove(&o.location.lossy()).unwrap();
        self.originals.borrow_mut().insert(o.original_path.lossy(), id);
        if self.unknown_on.borrow().as_deref() == Some(o.action_id) {
            return RecoveryOutcome::Unknown { message: "simulated lost acknowledgement".into() };
        }
        RecoveryOutcome::Done
    }
    fn purge(&self, o: &QuarantinedObject<'_>) -> RecoveryOutcome {
        self.calls.set(self.calls.get() + 1);
        if let Err(skip) = self.source(o) {
            return skip;
        }
        self.quarantine.borrow_mut().remove(&o.location.lossy());
        RecoveryOutcome::Done
    }
    fn reconcile(&self, o: &QuarantinedObject<'_>, kind: RecoveryKind) -> Reconciled {
        let in_q = self.quarantine.borrow().get(&o.location.lossy()) == Some(&o.identity.file_id);
        let back = self.originals.borrow().get(&o.original_path.lossy()) == Some(&o.identity.file_id);
        match (kind, in_q, back) {
            (_, true, _) => Reconciled::NotPerformed,
            (RecoveryKind::Restore, false, true) | (RecoveryKind::Purge, false, _) => Reconciled::Performed,
            _ => Reconciled::StillUnknown,
        }
    }
    fn free_space(&self, _: &RawPath) -> Option<u64> {
        // Same-volume moves change nothing; a purge frees its 100 bytes.
        Some(1_000_000 - 100 * (self.quarantine.borrow().len() + self.originals.borrow().len()) as u64)
    }
    fn metadata_coverage(&self) -> MetadataCoverage {
        MetadataCoverage { preserved: vec![], limits: vec![] }
    }
}

fn ident(fid: u32) -> WireIdentity {
    WireIdentity { volume_id: "v".into(), file_id: format!("inode64:{fid:016x}"), generation: None, quality: "strong".into(), observed_at: "2026-10-15T00:00:00Z".into() }
}

fn unix(p: &str) -> RawPath {
    RawPath::unix(p.as_bytes().to_vec()).unwrap()
}

fn stores(name: &str) -> Stores {
    let dir = std::env::temp_dir().join(format!("cm-recovery-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Stores::open(&dir).unwrap()
}

/// Quarantine rows q1..qn for /r/fN (file id N) held at /q/op/aN, and the matching fake volume.
fn seed(s: &Stores, n: u32) -> Fake {
    let f = Fake::default();
    for i in 1..=n {
        let m = QuarantineManifest {
            original_path: unix(&format!("/r/f{i}")),
            original_identity: ident(i),
            original_parent: Some(ident(0xff)),
            location: unix(&format!("/q/op/a{i}")),
            logical_bytes: Bytes(100),
        };
        s.audit.save_quarantine(&format!("q{i}"), "op_seed", &format!("a{i}"), "p_seed", &serde_json::to_string(&m).unwrap(), &utc_from_unix(T0 + i64::from(i)),
            &utc_from_unix(T0 + 30 * 86_400)).unwrap();
        f.quarantine.borrow_mut().insert(format!("/q/op/a{i}"), ident(i).file_id);
    }
    f
}

fn plan(s: &Stores, kind: RecoveryKind, ids: &[&str], plan_id: &str, at: i64) -> RecoveryPlan {
    create_recovery_plan(s, kind, &ids.iter().map(|x| x.to_string()).collect::<Vec<_>>(), plan_id, at).unwrap()
}

fn approve(s: &Stores, p: &RecoveryPlan) {
    s.audit.save_recovery_receipt(&RecoveryReceipt {
        receipt_id: format!("r-{}", p.body.plan_id), plan_id: p.body.plan_id.clone(), plan_digest: p.digest.clone(), recovery_kind: p.body.recovery_kind,
        principal: PRINCIPAL.into(), source: ReceiptSource::InteractiveReview, created_at: p.body.created_at.clone(), expires_at: p.body.expires_at.clone(),
        revoked_at: None,
    }).unwrap();
}

fn run(s: &Stores, a: &dyn RecoveryActions, plan_id: &str, now: i64, fault: Option<(FaultPoint, &str)>) -> Result<OperationReport, ApplyError> {
    let host = HostRef { kind: "cli".into(), pid: 1, started_at: "t".into() };
    let ctx = RecoveryContext { principal: PRINCIPAL, host: &host, now: &move || now, cancel: &|| false, fault: &|p, id| fault.is_some_and(|(fp, fa)| fp == p && fa == id) };
    apply_recovery_plan(s, a, plan_id, &ctx)
}

#[test]
fn only_an_approval_of_this_exact_recovery_plan_authorizes_it() {
    let s = stores("auth");
    let f = seed(&s, 1);
    let old = plan(&s, RecoveryKind::Restore, &["q1"], "rp_old", T0);
    approve(&s, &old);
    let new = plan(&s, RecoveryKind::Restore, &["q1"], "rp_new", T0 + 1);
    // A cleanup-shaped approval written against the recovery plan id does not count either.
    s.audit.save_receipt(&Receipt {
        receipt_id: "cleanup".into(), plan_id: "rp_new".into(), plan_digest: new.digest.clone(), mode: Mode::Quarantine, principal: PRINCIPAL.into(),
        source: ReceiptSource::InteractiveReview, policy_id: None, policy_version: None, created_at: "t".into(), expires_at: new.body.expires_at.clone(), revoked_at: None,
    }).unwrap();
    assert!(matches!(run(&s, &f, "rp_new", T0 + 10, None), Err(ApplyError::NeedsApproval(_))), "old and cleanup approvals never cover a new plan");
    let purge = plan(&s, RecoveryKind::Purge, &["q1"], "rp_purge", T0 + 2);
    s.audit.save_recovery_receipt(&RecoveryReceipt { plan_id: "rp_purge".into(), receipt_id: "x".into(), recovery_kind: RecoveryKind::Restore, ..receipt_of(&s, &old) }).unwrap();
    assert!(matches!(run(&s, &f, "rp_purge", T0 + 10, None), Err(ApplyError::NeedsApproval(_))), "a restore approval never authorizes a purge");
    assert_eq!(recovery_authorization(&s, &purge, PRINCIPAL, T0 + 10).unwrap().0, "required");
    assert_eq!(f.calls.get(), 0);
    assert_eq!(recovery_authorization(&s, &old, PRINCIPAL, T0 + 10).unwrap().0, "authorized");
    assert!(matches!(run(&s, &f, "rp_old", T0 + 900, None), Err(ApplyError::Stale(StaleReason::Expired))));
    assert_eq!(f.calls.get(), 0);
}

fn receipt_of(s: &Stores, p: &RecoveryPlan) -> RecoveryReceipt {
    s.audit.recovery_receipts_for(&p.body.plan_id).unwrap().remove(0)
}

#[test]
fn restore_moves_back_once_and_reports_no_freed_space() {
    let s = stores("restore");
    let f = seed(&s, 2);
    let p = plan(&s, RecoveryKind::Restore, &["q1", "q2"], "rp", T0);
    approve(&s, &p);
    let r = run(&s, &f, "rp", T0 + 10, None).unwrap();
    assert_eq!((r.status.as_str(), r.succeeded, r.processed_logical_bytes.as_str()), ("completed", 2, "200"));
    assert_eq!((r.quarantined_logical_bytes.as_str(), r.estimated_freed_bytes.as_str(), r.observed_free_delta_bytes.as_deref()), ("0", "0", Some("0")));
    assert!(r.items.iter().all(|i| i.code == "restored" && i.recovery == "not_applicable"));
    assert_eq!(f.originals.borrow().len(), 2);
    assert_eq!(run(&s, &f, "rp", T0 + 20, None).unwrap(), r, "a repeat replays the report");
    assert_eq!(f.calls.get(), 2);
    assert!(quarantine_page(&s, &f, 50, None, T0 + 30).unwrap().0.is_empty(), "restored items leave the quarantine list");
    assert!(matches!(create_recovery_plan(&s, RecoveryKind::Purge, &["q1".into()], "rp2", T0 + 40), Err(RecoveryError::NotRecoverable(..))));
    let (history, _) = operation_history(&s, 50, None).unwrap();
    assert_eq!((history[0].action.as_deref(), history[0].availability), (Some("restore"), "not_applicable"));
}

#[test]
fn occupied_destination_is_skipped_and_both_objects_are_kept() {
    let s = stores("conflict");
    let f = seed(&s, 2);
    f.originals.borrow_mut().insert("/r/f1".into(), "inode64:00000000000000aa".into());
    let (list, _) = quarantine_page(&s, &f, 50, None, T0).unwrap();
    assert_eq!((list[0].state.clone(), list[1].state.clone()), (QuarantineState::TargetConflict, QuarantineState::Recoverable));
    let p = plan(&s, RecoveryKind::Restore, &["q1", "q2"], "rp", T0);
    approve(&s, &p);
    let r = run(&s, &f, "rp", T0 + 10, None).unwrap();
    assert_eq!((r.status.as_str(), r.succeeded, r.skipped), ("partial", 1, 1));
    let skipped = r.items.iter().find(|i| i.status == ActionStatus::Skipped).unwrap();
    assert_eq!((skipped.code.as_str(), skipped.recovery.as_str()), ("destination_exists", "quarantine"));
    assert_eq!(f.originals.borrow()["/r/f1"], "inode64:00000000000000aa", "the new file is not overwritten");
    assert!(f.quarantine.borrow().contains_key("/q/op/a1"), "the quarantined file stays restorable");
    assert_eq!(quarantine_page(&s, &f, 50, None, T0 + 20).unwrap().0.len(), 1);
}

#[test]
fn changed_source_and_offline_volume_are_skipped_and_kept_listed() {
    let s = stores("changed");
    let f = seed(&s, 1);
    f.quarantine.borrow_mut().insert("/q/op/a1".into(), "inode64:00000000000000bb".into());
    let p = plan(&s, RecoveryKind::Restore, &["q1"], "rp", T0);
    approve(&s, &p);
    let r = run(&s, &f, "rp", T0 + 10, None).unwrap();
    assert_eq!((r.status.as_str(), r.items[0].code.as_str()), ("partial", "identity_changed"));
    assert!(f.originals.borrow().is_empty());
    f.offline.set(true);
    let (list, _) = quarantine_page(&s, &f, 50, None, T0 + 20).unwrap();
    assert_eq!(list[0].state, QuarantineState::VolumeOffline, "the manifest is kept, not restored to a new volume");
    let p = plan(&s, RecoveryKind::Restore, &["q1"], "rp2", T0 + 20);
    approve(&s, &p);
    assert_eq!(run(&s, &f, "rp2", T0 + 30, None).unwrap().items[0].code, "volume_offline");
}

#[test]
fn purge_is_separate_and_retention_reminders_never_delete() {
    let s = stores("purge");
    let f = seed(&s, 2);
    let (list, _) = quarantine_page(&s, &f, 50, None, T0 + 31 * 86_400).unwrap();
    assert!(list.iter().all(|v| v.reminder_due && v.state == QuarantineState::Recoverable), "due reminders keep the items");
    assert_eq!((f.calls.get(), f.quarantine.borrow().len()), (0, 2));
    let p = plan(&s, RecoveryKind::Purge, &["q2"], "pp", T0 + 31 * 86_400);
    assert_eq!(p.body.items[0].target.path, None);
    assert!(matches!(run(&s, &f, "pp", T0 + 31 * 86_400 + 1, None), Err(ApplyError::NeedsApproval(_))));
    approve(&s, &p);
    let r = run(&s, &f, "pp", T0 + 31 * 86_400 + 1, None).unwrap();
    assert_eq!((r.status.as_str(), r.estimated_freed_bytes.as_str(), r.items[0].recovery.as_str()), ("completed", "100", "unavailable"));
    assert_eq!(r.observed_free_delta_bytes.as_deref(), Some("100"));
    let (list, _) = quarantine_page(&s, &f, 50, None, T0 + 32 * 86_400).unwrap();
    assert_eq!(list.iter().map(|v| v.entry.quarantine_id.as_str()).collect::<Vec<_>>(), ["q1"]);
    let (history, _) = operation_history(&s, 50, None).unwrap();
    assert_eq!((history[0].action.as_deref(), history[0].availability), (Some("purge"), "unavailable"));
}

#[test]
fn crash_after_restore_is_reconciled_without_repeating() {
    let s = stores("crash");
    let f = seed(&s, 2);
    let p = plan(&s, RecoveryKind::Restore, &["q1", "q2"], "rp", T0);
    approve(&s, &p);
    assert!(matches!(run(&s, &f, "rp", T0 + 10, Some((FaultPoint::AfterAction, "r0000000"))), Err(ApplyError::Crashed(_))));
    let moved = s.audit.reconcile_hosts(|_| cm_core::host::Liveness::Exited).unwrap();
    let calls = f.calls.get();
    let host = HostRef { kind: "cli".into(), pid: 2, started_at: "t".into() };
    let ctx = RecoveryContext { principal: PRINCIPAL, host: &host, now: &|| T0 + 20, cancel: &|| false, fault: &|_, _| false };
    let r = reconcile_recovery_operation(&s, &f, &moved[0], &ctx).unwrap();
    assert_eq!((r.status.as_str(), r.succeeded, r.not_started), ("partial", 1, 1));
    assert_eq!(f.calls.get(), calls, "reconciliation never acts");
    assert_eq!(s.audit.quarantine_entry("q1").unwrap().state, "restored");
    assert_eq!(s.audit.quarantine_entry("q2").unwrap().state, "recoverable");
}

#[test]
fn unknown_restore_holds_the_lock_and_needs_reconciliation() {
    let s = stores("unknown");
    let f = seed(&s, 1);
    *f.unknown_on.borrow_mut() = Some("r0000000".into());
    let p = plan(&s, RecoveryKind::Restore, &["q1"], "rp", T0);
    approve(&s, &p);
    let r = run(&s, &f, "rp", T0 + 10, None).unwrap();
    assert_eq!((r.status.as_str(), r.items[0].recovery.as_str()), ("reconciliation_required", "unknown"));
    let purge = plan(&s, RecoveryKind::Purge, &["q1"], "pp", T0 + 11);
    approve(&s, &purge);
    assert!(matches!(run(&s, &f, "pp", T0 + 12, None), Err(ApplyError::Conflict(_))), "no other plan touches an unconfirmed object");
    let (history, _) = operation_history(&s, 50, None).unwrap();
    assert_eq!(history[0].availability, "unknown");
}

#[test]
fn incomplete_manifests_are_listed_but_never_planned() {
    let s = stores("incomplete");
    let f = Fake::default();
    let m = serde_json::json!({"original_path": unix("/r/x"), "original_identity": ident(1), "original_parent": null, "location": unix("/q/op/x"), "logical_bytes": "5"});
    s.audit.save_quarantine("qx", "op", "a", "p", &m.to_string(), &utc_from_unix(T0), &utc_from_unix(T0 + 1)).unwrap();
    let v = quarantine_show(&s, &f, "qx", T0).unwrap();
    assert_eq!((v.state, v.manifest.is_none()), (QuarantineState::ManifestIncomplete, true));
    let err = create_recovery_plan(&s, RecoveryKind::Restore, &["qx".into()], "rp", T0).unwrap_err();
    assert_eq!(err.code(), cm_core::error::ErrorCode::Unsupported);
    s.audit.save_quarantine("qbad", "op", "a", "p", "{}", &utc_from_unix(T0), &utc_from_unix(T0 + 1)).unwrap();
    assert!(matches!(quarantine_show(&s, &f, "qbad", T0), Err(StoreError::AuditUnavailable(_))), "damaged records are not shown as recoverable");
}

/// Store a finished cleanup operation of `mode` with one succeeded action.
fn cleanup_op(s: &Stores, mode: Mode, n: u8) -> String {
    let body = PlanBody {
        engine_contract: "plan/v1".into(), plan_id: format!("p{n}"), scope_id: "scope".into(), scan_id: "s".into(), scan_revision: 1,
        rule_set_digest: "0".repeat(64), policy_digest: "0".repeat(64), created_at: utc_from_unix(T0), expires_at: utc_from_unix(T0 + 900), mode, items: vec![],
    };
    let plan = Plan { digest: body.digest().unwrap(), body, observations: vec![], tiers: vec![], rule_refs: vec!["rule@1.0.0".into()], item_meta: vec![] };
    s.audit.save_plan(&plan).unwrap();
    let op = format!("op{n}");
    let host = HostRef { kind: "cli".into(), pid: 1, started_at: "t".into() };
    s.audit.begin_operation(&op, &plan.body.plan_id, &plan.digest, &op, &host, &utc_from_unix(T0 + i64::from(n))).unwrap();
    let report = OperationReport {
        operation_id: op.clone(), plan_id: plan.body.plan_id.clone(), plan_digest: plan.digest.clone(), status: "completed".into(), started_at: utc_from_unix(T0),
        finished_at: utc_from_unix(T0), succeeded: 1, skipped: 0, failed: 0, not_started: 0, unknown: 0,
        items: vec![result("a0000000", ActionStatus::Succeeded, "done", "", "x")], processed_logical_bytes: "1".into(), quarantined_logical_bytes: "0".into(),
        estimated_freed_bytes: "0".into(), volume_free_before_bytes: None, volume_free_after_bytes: None, observed_free_delta_bytes: None,
        authorization_source: "interactive_review".into(),
    };
    s.audit.finish_operation(&op, "completed", &serde_json::to_string(&report).unwrap(), &utc_from_unix(T0)).unwrap();
    op
}

#[test]
fn history_reports_truthful_recovery_availability() {
    let s = stores("history");
    cleanup_op(&s, Mode::Trash, 1);
    cleanup_op(&s, Mode::Delete, 2);
    let q = cleanup_op(&s, Mode::Quarantine, 3);
    s.audit.save_quarantine("q", &q, "a0000000", "p3", "{}", &utc_from_unix(T0), &utc_from_unix(T0)).unwrap();
    let (page, next) = operation_history(&s, 2, None).unwrap();
    assert_eq!(page.iter().map(|h| (h.action.as_deref().unwrap(), h.availability)).collect::<Vec<_>>(),
        [("quarantine", "in_app_restore"), ("delete", "unavailable")]);
    assert_eq!(page[0].rule_refs, ["rule@1.0.0"]);
    assert!(page[1].guidance.contains("no undo"));
    let (page, next) = operation_history(&s, 2, next.as_deref()).unwrap();
    assert_eq!((page[0].action.as_deref(), page[0].availability, next), (Some("trash"), "system_recycle_bin", None));
    assert!(page[0].guidance.contains("Recycle Bin"));
    s.audit.set_quarantine_state("q", "purged").unwrap();
    assert_eq!(operation_history(&s, 1, None).unwrap().0[0].availability, "not_applicable");
}
