use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use super::*;
use cm_core::execute::TreeMember;
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
    /// Directories by path (directory items); `tree_ok` lets `trash_tree` succeed.
    dirs: RefCell<BTreeMap<String, String>>,
    tree_ok: Cell<bool>,
    tree_calls: Cell<u32>,
    /// A file created inside a directory right after this action id runs (mid-plan arrival).
    arrive_after: RefCell<Option<(String, String)>>,
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
        if let Some((after, path)) = self.arrive_after.borrow().clone() && after == t.action_id {
            self.files.borrow_mut().insert(path, "inode64:00000000000000ee".into());
        }
        if self.unknown_on.borrow().as_deref() == Some(t.action_id) {
            return ActionOutcome::Unknown { message: "simulated lost acknowledgement".into() };
        }
        ActionOutcome::Succeeded { recovery: Recovery::Unavailable }
    }
    fn quarantine_dir_for(&self, _: &ActionTarget<'_>, op: &str) -> Option<RawPath> {
        RawPath::unix(format!("/q/{op}").into_bytes()).ok()
    }
    fn reconcile(&self, t: &ActionTarget<'_>, _: Mode, _: Option<&RawPath>) -> Reconciled {
        let key = t.path.lossy();
        match self.files.borrow().get(&key).or(self.dirs.borrow().get(&key)) {
            Some(id) if *id == t.identity.file_id => Reconciled::NotPerformed,
            Some(_) => Reconciled::StillUnknown,
            None => Reconciled::Performed,
        }
    }
    fn free_space(&self, _: &RawPath) -> Option<u64> {
        Some(1_000_000 + 100 * 3u64.saturating_sub(self.files.borrow().len() as u64))
    }
    fn remove_empty_dir(&self, t: &ActionTarget<'_>) -> ActionOutcome {
        self.calls.set(self.calls.get() + 1);
        let key = t.path.lossy();
        match self.dirs.borrow().get(&key) {
            None => return ActionOutcome::Skipped { code: "missing".into(), message: "gone".into() },
            Some(id) if *id != t.identity.file_id => return ActionOutcome::Skipped { code: "identity_changed".into(), message: "swapped".into() },
            Some(_) => {}
        }
        let inside = format!("{key}/");
        if self.files.borrow().keys().chain(self.dirs.borrow().keys()).any(|k| k.starts_with(&inside)) {
            return ActionOutcome::Skipped { code: "new_descendant".into(), message: "not empty".into() };
        }
        self.dirs.borrow_mut().remove(&key);
        ActionOutcome::Succeeded { recovery: Recovery::Unavailable }
    }
    fn trash_tree(&self, root: &ActionTarget<'_>, members: &[TreeMember<'_>]) -> ActionOutcome {
        self.tree_calls.set(self.tree_calls.get() + 1);
        let inside = format!("{}/", root.path.lossy());
        let live: Vec<String> = self.files.borrow().keys().chain(self.dirs.borrow().keys()).filter(|k| k.starts_with(&inside)).cloned().collect();
        if !self.tree_ok.get() || live.len() != members.len() || !members.iter().all(|m| live.contains(&m.path.lossy())) {
            return ActionOutcome::Skipped { code: "tree_mismatch".into(), message: "differs".into() };
        }
        for k in live {
            self.files.borrow_mut().remove(&k);
            self.dirs.borrow_mut().remove(&k);
        }
        self.dirs.borrow_mut().remove(&root.path.lossy());
        ActionOutcome::Succeeded { recovery: Recovery::Trash }
    }
}

fn obj(id: u64, kind: ObjectKind, size: Option<u64>) -> ObjectRow {
    ObjectRow { object_id: id, volume_id: "v".into(), file_id: format!("inode64:{id:016x}"), generation: None, quality: IdentityQuality::Strong,
        kind, logical: size, allocated: size, link_count: Some(1), cloud: CloudState::NotCloud, modified_unix: Some(T0 - 1000),
        shared_blocks: None }
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
    env_with(name, mode, "cache")
}

/// `rule_id` starting with `purge.` makes the Cache directory itself a planned directory item.
fn env_with(name: &str, mode: Mode, rule_id: &str) -> Env {
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
    let rules = RuleSet { rules: vec![rule(rule_id, "Cache")], digest: "rs".into() };
    let roots = [ResolvedRoot { rule_id: rule_id.into(), path: RawPath::unix(b"/r/app".to_vec()).unwrap(), volume_id: "v".into(), file_id: "inode64:0000000000000002".into() }];
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

// ---------------------------------------------------------------------------------------------
// Directory items (project artifacts): leaves first, then verified empty-directory removal.

fn dir_fake() -> Fake {
    let f = fake();
    f.dirs.borrow_mut().insert("/r/app/Cache".into(), "inode64:0000000000000003".into());
    f
}

fn plan_items(e: &Env, plan_id: &str) -> Vec<cm_core::plan::PlanItem> {
    e.stores.audit.load_plan(plan_id).unwrap().body.items
}

#[test]
fn directory_items_follow_their_files_and_are_digest_covered() {
    let e = env_with("dir-plan", Mode::Delete, "purge.test");
    let items = plan_items(&e, "p1");
    let kinds: Vec<ItemKind> = items.iter().map(|i| i.kind).collect();
    assert_eq!(kinds, [ItemKind::File, ItemKind::File, ItemKind::File, ItemKind::Directory]);
    let body = serde_json::to_value(&e.stores.audit.load_plan("p1").unwrap().body).unwrap();
    assert_eq!(body["items"][3]["kind"], "directory");
    assert!(body["items"][0].get("kind").is_none(), "file items keep the plan/v1 shape");
    // Quarantine keeps directories (restores need the original parents) and other catalogs stay files-only.
    assert!(plan_items(&env_with("dir-q", Mode::Quarantine, "purge.test"), "p1").iter().all(|i| i.kind == ItemKind::File));
    assert!(plan_items(&env("dir-none", Mode::Delete), "p1").iter().all(|i| i.kind == ItemKind::File));
}

#[test]
fn emptied_directory_is_removed_after_its_files() {
    let e = env_with("dir-ok", Mode::Delete, "purge.test");
    let f = dir_fake();
    approve(&e, "p1");
    let r = run(&e, &f, "p1", T0 + 10, None, None).unwrap();
    assert_eq!((r.status.as_str(), r.succeeded), ("completed", 4), "{r:?}");
    assert!(f.dirs.borrow().is_empty() && f.files.borrow().is_empty());
    assert_eq!(r.items[3].message, "empty directory removed");
    assert_eq!(r.processed_logical_bytes, "300", "directories add no bytes");
}

#[test]
fn a_file_arriving_mid_plan_keeps_the_directory() {
    let e = env_with("dir-new", Mode::Delete, "purge.test");
    let f = dir_fake();
    *f.arrive_after.borrow_mut() = Some(("a0000002".into(), "/r/app/Cache/late".into()));
    approve(&e, "p1");
    let r = run(&e, &f, "p1", T0 + 10, None, None).unwrap();
    assert_eq!((r.status.as_str(), r.succeeded, r.skipped), ("partial", 3, 1));
    assert_eq!(r.items[3].code, "precondition_new_descendant");
    assert!(f.files.borrow().contains_key("/r/app/Cache/late"), "the new file is never touched");
    assert!(f.dirs.borrow().contains_key("/r/app/Cache"));
}

#[test]
fn a_kept_file_keeps_its_directory_without_trying() {
    let e = env_with("dir-kept", Mode::Delete, "purge.test");
    let f = dir_fake();
    f.files.borrow_mut().insert("/r/app/Cache/b".into(), "inode64:00000000000000ff".into());
    approve(&e, "p1");
    let r = run(&e, &f, "p1", T0 + 10, None, None).unwrap();
    assert_eq!(r.items[3].code, "precondition_descendant_kept");
    assert_eq!(f.calls.get(), 2, "two deletes, no directory attempt");
    assert!(f.dirs.borrow().contains_key("/r/app/Cache"));
}

#[test]
fn trash_mode_prefers_the_whole_verified_tree_and_falls_back_to_leaves() {
    let e = env_with("dir-tree", Mode::Trash, "purge.test");
    let f = dir_fake();
    f.tree_ok.set(true);
    approve(&e, "p1");
    let r = run(&e, &f, "p1", T0 + 10, None, None).unwrap();
    assert_eq!((r.status.as_str(), r.succeeded, f.tree_calls.get(), f.calls.get()), ("completed", 4, 1, 0), "{r:?}");
    assert!(r.items.iter().all(|i| i.recovery == "trash"));

    let e = env_with("dir-tree-fallback", Mode::Trash, "purge.test");
    let f = dir_fake();
    approve(&e, "p1");
    let r = run(&e, &f, "p1", T0 + 10, None, None).unwrap();
    assert_eq!((r.status.as_str(), r.succeeded, f.tree_calls.get()), ("completed", 4, 1), "{r:?}");
    assert_eq!((r.items[0].recovery.as_str(), r.items[3].recovery.as_str()), ("trash", "not_applicable"));
    assert!(f.dirs.borrow().is_empty());
}

#[test]
fn interrupted_directory_removal_is_reconciled_as_a_delete() {
    let e = env_with("dir-crash", Mode::Delete, "purge.test");
    let f = dir_fake();
    approve(&e, "p1");
    assert!(matches!(run(&e, &f, "p1", T0 + 10, None, Some((FaultPoint::AfterAction, "a0000003"))), Err(ApplyError::Crashed(_))));
    let moved = e.stores.audit.reconcile_hosts(|_| cm_core::host::Liveness::Exited).unwrap();
    let host = HostRef { kind: "cli".into(), pid: 2, started_at: "t".into() };
    let ctx = ApplyContext { principal: PRINCIPAL, host: &host, current_rule_set_digest: "rs", now: &|| T0 + 20, cancel: &|| false, fault: &|_, _| false, protect: &|_| Ok(()) };
    let r = reconcile_operation(&e.stores, &f, &moved[0], &ctx).unwrap();
    assert_eq!((r.items[3].status, r.items[3].recovery.as_str()), (ActionStatus::Succeeded, "not_applicable"), "{r:?}");
}

#[test]
fn a_protected_member_keeps_the_tree_out_of_the_recycle_bin() {
    let e = env_with("dir-tree-protected", Mode::Trash, "purge.test");
    let f = dir_fake();
    f.tree_ok.set(true);
    approve(&e, "p1");
    let host = HostRef { kind: "cli".into(), pid: 1, started_at: "t".into() };
    let ctx = ApplyContext {
        principal: PRINCIPAL, host: &host, current_rule_set_digest: "rs", now: &|| T0 + 10, cancel: &|| false, fault: &|_, _| false,
        protect: &|t| if t.path.lossy().ends_with("/b") { Err("protected by user entry u1".into()) } else { Ok(()) },
    };
    let r = apply_plan(&e.stores, &f, "p1", &ctx).unwrap();
    assert_eq!(f.tree_calls.get(), 0, "no whole-tree move when anything inside is protected");
    assert_eq!((r.succeeded, r.skipped), (2, 2), "{r:?}");
    assert!(f.files.borrow().contains_key("/r/app/Cache/b") && f.dirs.borrow().contains_key("/r/app/Cache"));
}
