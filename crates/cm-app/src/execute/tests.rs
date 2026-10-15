use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use super::*;
use cm_core::analysis::{AnalysisInput, ResolvedRoot, analyze};
use cm_core::authz::{Policy, PolicyState};
use cm_core::capability::Platform;
use cm_core::model::{CloudState, IdentityQuality, ObjectKind, ScanState};
use cm_core::plan::{PlanInput, build_plan};
use cm_core::rules::{ActionKind, Impact, Match, ObjectType, Owner, OwnerKind, RebuildClass, RootResolver, Rule, RuleSet, RuleStatus, Thresholds, Tier};
use cm_core::scan::{EntryRow, ObjectRow};
use cm_store::index::ScanInit;

const T0: i64 = 1_800_000_000;
const PRINCIPAL: &str = "user:alice";

/// In-memory file system keyed by path text: path → (file_id, size).
#[derive(Default)]
struct Fake {
    files: RefCell<BTreeMap<String, String>>,
    quarantine: RefCell<BTreeMap<String, String>>,
    calls: Cell<u32>,
    unknown_on: RefCell<Option<String>>,
}

impl Fake {
    fn with(paths: &[(&str, &str)]) -> Fake {
        let f = Fake::default();
        for (p, id) in paths {
            f.files.borrow_mut().insert(p.to_string(), id.to_string());
        }
        f
    }
}

impl FileActions for Fake {
    fn check(&self, t: &ActionTarget<'_>) -> LiveCheck {
        match self.files.borrow().get(&t.path.lossy()) {
            None => LiveCheck::Missing,
            Some(id) if *id != t.identity.file_id => LiveCheck::IdentityChanged,
            Some(_) => LiveCheck::Match,
        }
    }
    fn quarantine(&self, t: &ActionTarget<'_>, dir: &RawPath, name: &str) -> ActionOutcome {
        self.calls.set(self.calls.get() + 1);
        let id = self.files.borrow_mut().remove(&t.path.lossy()).unwrap();
        let loc = format!("{}/{name}", dir.lossy());
        self.quarantine.borrow_mut().insert(loc.clone(), id);
        ActionOutcome::Succeeded { recovery: Recovery::Quarantine { location: RawPath::unix(loc.into_bytes()).unwrap() } }
    }
    fn trash(&self, t: &ActionTarget<'_>) -> ActionOutcome {
        self.delete(t);
        ActionOutcome::Succeeded { recovery: Recovery::Trash }
    }
    fn delete(&self, t: &ActionTarget<'_>) -> ActionOutcome {
        self.calls.set(self.calls.get() + 1);
        self.files.borrow_mut().remove(&t.path.lossy());
        if self.unknown_on.borrow().as_deref() == Some(t.action_id) {
            return ActionOutcome::Unknown { message: "simulated lost acknowledgement".into() };
        }
        ActionOutcome::Succeeded { recovery: Recovery::Unavailable }
    }
    fn quarantine_dir_for(&self, _: &ActionTarget<'_>, op: &str) -> Option<RawPath> {
        RawPath::unix(format!("/q/{op}").into_bytes()).ok()
    }
    fn reconcile(&self, t: &ActionTarget<'_>, _: Mode, _: Option<&RawPath>) -> Reconciled {
        match self.files.borrow().get(&t.path.lossy()) {
            Some(id) if *id == t.identity.file_id => Reconciled::NotPerformed,
            Some(_) => Reconciled::StillUnknown,
            None => Reconciled::Performed,
        }
    }
    fn free_space(&self, _: &RawPath) -> Option<u64> {
        Some(1_000_000 + 100 * (3 - self.files.borrow().len() as u64))
    }
}

fn obj(id: u64, kind: ObjectKind, size: Option<u64>) -> ObjectRow {
    ObjectRow { object_id: id, volume_id: "v".into(), file_id: format!("inode64:{id:016x}"), generation: None, quality: IdentityQuality::Strong,
        kind, logical: size, allocated: size, link_count: Some(1), cloud: CloudState::NotCloud, modified_unix: Some(T0 - 1000) }
}

fn ent(id: u64, parent: Option<u64>, name: &str, kind: ObjectKind) -> EntryRow {
    EntryRow { entry_id: id, parent_entry_id: parent, name: RawPath::unix(name.as_bytes().to_vec()).unwrap(), object_id: id, kind, boundary: None }
}

fn rule(id: &str, comp: &str) -> Rule {
    Rule {
        schema_version: "2.0".into(), rule_id: id.into(), version: "1.0.0".into(), status: RuleStatus::Enabled, platforms: vec![Platform::Macos],
        category: "app_caches".into(), max_tier: Tier::DefaultSelected, rebuild_class: RebuildClass::LocalRebuild,
        owner: Owner { kind: OwnerKind::App, id: "t".into() }, root_resolver: RootResolver::ApplicationCache,
        matcher: Match { object_type: ObjectType::Any, relative_components: vec![comp.into()], name_glob: None, extensions: vec![],
            require_cachedir_tag: false, archive_payload: false, max_depth: 0 },
        exclude: vec![], probes: vec![], thresholds: Thresholds { min_age_days: None, age_basis: None, min_size_bytes: None, retention_days: None },
        action_kind: ActionKind::Filesystem, adapter_ref: None, allowed_modes: vec![Mode::Trash, Mode::Quarantine, Mode::Delete],
        requires_admin: false, protect_descendants: true, follow_links: false,
        impact: Impact { summary: "s".into(), regeneration: "r".into(), offline_effect: "o".into() },
        tested_registry_ref: None, evidence_refs: vec!["e".into()],
    }
}

struct Env {
    stores: Stores,
}

/// /r/app/Cache/{a,b,c} (100 bytes each) and /r/app/Other/{a} ; plans: p1 = Cache, p2 = Cache again (overlap)
fn env(name: &str, mode: Mode) -> Env {
    let dir = std::env::temp_dir().join(format!("cm-exec-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let stores = Stores::open(&dir).unwrap();
    let host = HostRef { kind: "cli".into(), pid: 1, started_at: "t".into() };
    stores.index.begin_scan(&ScanInit { scan_id: "s".into(), scope_id: "scope".into(), root: RawPath::unix(b"/r".to_vec()).unwrap(),
        backend: "test".into(), host, started_at: "t".into() }).unwrap();
    let objects = [obj(1, ObjectKind::Directory, None), obj(2, ObjectKind::Directory, None), obj(3, ObjectKind::Directory, None),
        obj(4, ObjectKind::File, Some(100)), obj(5, ObjectKind::File, Some(100)), obj(6, ObjectKind::File, Some(100))];
    let entries = [ent(1, None, "/r", ObjectKind::Directory), ent(2, Some(1), "app", ObjectKind::Directory), ent(3, Some(2), "Cache", ObjectKind::Directory),
        ent(4, Some(3), "a", ObjectKind::File), ent(5, Some(3), "b", ObjectKind::File), ent(6, Some(3), "c", ObjectKind::File)];
    stores.index.write_batch("s", 1, &objects, &entries, &[]).unwrap();
    stores.index.publish("s", 1, ScanState::Completed, "t").unwrap();
    let view = stores.index.view("s").unwrap();
    let rules = RuleSet { rules: vec![rule("cache", "Cache")], digest: "rs".into() };
    let roots = [ResolvedRoot { rule_id: "cache".into(), path: RawPath::unix(b"/r/app".to_vec()).unwrap(), volume_id: "v".into(), file_id: "inode64:0000000000000002".into() }];
    let set = analyze(&view, &rules, &AnalysisInput { scan_id: "s", revision: 1, now_unix: T0, roots: &roots, protected: &[] });
    let cid = set.candidates[0].candidate_id.clone();
    for (pid, created) in [("p1", T0), ("p2", T0 + 1)] {
        let plan = build_plan(&view, &set, std::slice::from_ref(&cid), mode,
            &PlanInput { plan_id: pid, scope_id: "scope", created_unix: created, observed_at: "t", policy_digest: "0" }).unwrap();
        stores.audit.save_plan(&plan).unwrap();
    }
    Env { stores }
}

fn fake() -> Fake {
    Fake::with(&[("/r/app/Cache/a", "inode64:0000000000000004"), ("/r/app/Cache/b", "inode64:0000000000000005"), ("/r/app/Cache/c", "inode64:0000000000000006")])
}

fn approve(e: &Env, plan_id: &str) {
    let plan = e.stores.audit.load_plan(plan_id).unwrap();
    e.stores.audit.save_receipt(&Receipt { receipt_id: format!("r-{plan_id}"), plan_id: plan_id.into(), plan_digest: plan.digest.clone(), mode: plan.body.mode,
        principal: PRINCIPAL.into(), source: ReceiptSource::InteractiveReview, policy_id: None, policy_version: None,
        created_at: plan.body.created_at.clone(), expires_at: plan.body.expires_at.clone(), revoked_at: None }).unwrap();
}

fn run(e: &Env, actions: &dyn FileActions, plan_id: &str, now: i64, cancel_after: Option<u32>, fault: Option<(FaultPoint, &str)>) -> Result<OperationReport, ApplyError> {
    let host = HostRef { kind: "cli".into(), pid: 1, started_at: "t".into() };
    let polls = Cell::new(0u32);
    let ctx = ApplyContext {
        principal: PRINCIPAL,
        host: &host,
        current_rule_set_digest: "rs",
        now: &move || now,
        cancel: &|| {
            polls.set(polls.get() + 1);
            cancel_after.is_some_and(|n| polls.get() > n)
        },
        fault: &|p, a| fault.is_some_and(|(fp, fa)| fp == p && fa == a),
        protect: &|_| Ok(()),
    };
    apply_plan(&e.stores, actions, plan_id, &ctx)
}

#[test]
fn without_authorization_nothing_is_touched() {
    let e = env("noauth", Mode::Delete);
    let f = fake();
    assert!(matches!(run(&e, &f, "p1", T0, None, None), Err(ApplyError::NeedsApproval(_))));
    assert_eq!(f.calls.get(), 0);
    assert_eq!(f.files.borrow().len(), 3);
}

#[test]
fn approved_plan_completes_and_repeats_return_the_same_operation() {
    let e = env("ok", Mode::Delete);
    let f = fake();
    approve(&e, "p1");
    let r = run(&e, &f, "p1", T0 + 10, None, None).unwrap();
    assert_eq!((r.status.as_str(), r.succeeded, r.processed_logical_bytes.as_str()), ("completed", 3, "300"));
    assert_eq!(r.observed_free_delta_bytes.as_deref(), Some("300"));
    assert_eq!((r.quarantined_logical_bytes.as_str(), r.estimated_freed_bytes.as_str()), ("0", "300"));
    let again = run(&e, &f, "p1", T0 + 20, None, None).unwrap();
    assert_eq!(again, r, "same operation report, no second side effect");
    assert_eq!(f.calls.get(), 3);
}

#[test]
fn protection_added_after_planning_skips_the_item() {
    let e = env("protect", Mode::Delete);
    let f = fake();
    approve(&e, "p1");
    let host = HostRef { kind: "cli".into(), pid: 1, started_at: "t".into() };
    let ctx = ApplyContext {
        principal: PRINCIPAL, host: &host, current_rule_set_digest: "rs", now: &|| T0 + 10, cancel: &|| false, fault: &|_, _| false,
        protect: &|t| if t.path.lossy().ends_with("/b") { Err("protected by user entry u1".into()) } else { Ok(()) },
    };
    let r = apply_plan(&e.stores, &f, "p1", &ctx).unwrap();
    assert_eq!((r.status.as_str(), r.succeeded, r.skipped), ("partial", 2, 1));
    assert!(r.items.iter().any(|i| i.code == "protected" && i.message.contains("u1")));
    assert!(f.files.borrow().contains_key("/r/app/Cache/b"), "the protected file is never touched");
}

#[test]
fn expired_or_rule_changed_plans_are_stale() {
    let e = env("stale", Mode::Delete);
    let f = fake();
    approve(&e, "p1");
    assert!(matches!(run(&e, &f, "p1", T0 + 900, None, None), Err(ApplyError::Stale(StaleReason::Expired))));
    assert_eq!(f.calls.get(), 0);
}

#[test]
fn replaced_object_is_skipped_and_report_is_partial() {
    let e = env("partial", Mode::Delete);
    let f = fake();
    f.files.borrow_mut().insert("/r/app/Cache/b".into(), "inode64:00000000000000ff".into());
    approve(&e, "p1");
    let r = run(&e, &f, "p1", T0 + 10, None, None).unwrap();
    assert_eq!((r.status.as_str(), r.succeeded, r.skipped), ("partial", 2, 1));
    assert!(r.items.iter().any(|i| i.code == "precondition_identity_changed"));
    assert!(f.files.borrow().contains_key("/r/app/Cache/b"), "the replacement survives");
}

#[test]
fn cancellation_and_revocation_stop_new_actions_only() {
    let e = env("cancel", Mode::Delete);
    let f = fake();
    approve(&e, "p1");
    let r = run(&e, &f, "p1", T0 + 10, Some(1), None).unwrap();
    assert_eq!((r.status.as_str(), r.succeeded, r.not_started), ("cancelled", 1, 2));
    assert_eq!(f.files.borrow().len(), 2, "completed actions are not rolled back");
}

#[test]
fn overlapping_plans_conflict_while_outcomes_are_unknown() {
    let e = env("conflict", Mode::Delete);
    let f = fake();
    *f.unknown_on.borrow_mut() = Some("a0000001".into());
    approve(&e, "p1");
    approve(&e, "p2");
    let r = run(&e, &f, "p1", T0 + 10, None, None).unwrap();
    assert_eq!((r.status.as_str(), r.unknown), ("reconciliation_required", 1));
    assert!(matches!(run(&e, &f, "p2", T0 + 11, None, None), Err(ApplyError::Conflict(_))), "locks are kept until reconciled");
}

#[test]
fn policy_authorization_consumes_quota_and_holds_unknown() {
    let e = env("policy", Mode::Delete);
    let f = fake();
    *f.unknown_on.borrow_mut() = Some("a0000002".into());
    let mut p = Policy::template("pol", PRINCIPAL, "scope", vec!["cache@1.0.0".into()], Mode::Delete, T0);
    p.state = PolicyState::Enabled;
    p.window_max_bytes = 350;
    e.stores.audit.save_policy(&p, "t").unwrap();
    let r = run(&e, &f, "p1", T0 + 10, None, None).unwrap();
    assert!(r.authorization_source.starts_with("policy:pol"), "no interactive prompt needed");
    assert_eq!(r.status, "reconciliation_required");
    assert_eq!(e.stores.audit.policy_usage("pol", T0 + 10).unwrap().bytes, 300, "200 consumed + 100 held as unknown");
    assert!(matches!(run(&e, &f, "p2", T0 + 11, None, None), Err(ApplyError::PolicyDenied(_))), "window exhausted");
}

#[test]
fn crash_windows_are_reconciled_without_repeating_side_effects() {
    for (point, expect) in [(FaultPoint::AfterIntent, ("partial", 1, 2)), (FaultPoint::AfterAction, ("partial", 2, 1))] {
        let e = env(&format!("crash-{point:?}"), Mode::Delete);
        let f = fake();
        approve(&e, "p1");
        assert!(matches!(run(&e, &f, "p1", T0 + 10, None, Some((point, "a0000001"))), Err(ApplyError::Crashed(_))));
        let calls_before = f.calls.get();
        let moved = e.stores.audit.reconcile_hosts(|_| cm_core::host::Liveness::Exited).unwrap();
        assert_eq!(moved.len(), 1, "a dead host's running operation needs reconciliation");
        let host = HostRef { kind: "cli".into(), pid: 2, started_at: "t".into() };
        let ctx = ApplyContext { principal: PRINCIPAL, host: &host, current_rule_set_digest: "rs", now: &|| T0 + 20, cancel: &|| false, fault: &|_, _| false, protect: &|_| Ok(()) };
        let r = reconcile_operation(&e.stores, &f, &moved[0], &ctx).unwrap();
        assert_eq!((r.status.as_str(), r.succeeded, r.not_started), expect, "{point:?}");
        assert_eq!(f.calls.get(), calls_before, "reconciliation never performs actions");
    }
}

#[test]
fn quarantine_records_a_restorable_manifest() {
    let e = env("quarantine", Mode::Quarantine);
    let f = fake();
    approve(&e, "p1");
    let r = run(&e, &f, "p1", T0 + 10, None, None).unwrap();
    assert_eq!(r.status, "completed");
    assert!(r.items.iter().all(|i| i.recovery == "quarantine"));
    // REC-05: moved into quarantine, not freed; the observed delta is reported as measured.
    assert_eq!((r.processed_logical_bytes.as_str(), r.quarantined_logical_bytes.as_str(), r.estimated_freed_bytes.as_str()), ("300", "300", "0"));
    assert_eq!(e.stores.audit.quarantine_items().unwrap().len(), 3);
    assert_eq!(f.quarantine.borrow().len(), 3);
}
