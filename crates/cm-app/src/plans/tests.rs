use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use super::*;
use cm_core::analysis::{EntryFacts, MemView};
use cm_core::authz::{Policy, PolicyState, Receipt, ReceiptSource};
use cm_core::capability::{CapabilityState, Platform};
use cm_core::host::HostRef;
use cm_core::manual_trash::{BinItem, ManualInput, SelectionGuard, build_purge_plan, build_trash_plan};
use cm_core::model::{CloudState, IdentityQuality, ObjectKind};
use cm_core::plan::v2::{Hive, NO_RULES_DIGEST};
use cm_core::protect::UserProtection;
use cm_core::startup::{PublisherEvidence, RegView, StartupItem, StartupLocation, StartupScope, StartupSource, StartupState, build_startup_plan};

const T0: i64 = 1_800_000_000;
const PRINCIPAL: &str = "user:alice";

fn stores(name: &str) -> Stores {
    let dir = std::env::temp_dir().join(format!("cm-app-plans-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Stores::open(&dir).unwrap()
}

/// In-memory files by display path → file id.
#[derive(Default)]
struct Fake {
    files: RefCell<BTreeMap<String, String>>,
    in_use: RefCell<Option<String>>,
    unknown: Cell<bool>,
    fail_delete: RefCell<Option<String>>,
    calls: Cell<u32>,
}

impl FileActions for Fake {
    fn check(&self, t: &ActionTarget<'_>) -> LiveCheck {
        let key = t.path.display();
        if self.in_use.borrow().as_deref() == Some(key.as_str()) {
            return LiveCheck::InUse;
        }
        match self.files.borrow().get(&key) {
            None => LiveCheck::Missing,
            Some(id) if *id != t.identity.file_id => LiveCheck::IdentityChanged,
            Some(_) => LiveCheck::Match,
        }
    }
    fn quarantine(&self, _: &ActionTarget<'_>, _: &RawPath, _: &str) -> ActionOutcome {
        panic!("manual plans never quarantine")
    }
    fn trash(&self, t: &ActionTarget<'_>) -> ActionOutcome {
        self.calls.set(self.calls.get() + 1);
        if self.unknown.get() {
            return ActionOutcome::Unknown { message: "the file left its path but was not confirmed in the recycle bin".into() };
        }
        self.files.borrow_mut().remove(&t.path.display());
        ActionOutcome::Succeeded { recovery: Recovery::Trash }
    }
    fn delete(&self, t: &ActionTarget<'_>) -> ActionOutcome {
        self.calls.set(self.calls.get() + 1);
        if self.fail_delete.borrow().as_deref() == Some(t.path.display().as_str()) {
            return ActionOutcome::Failed { code: "access_denied".into(), message: "refused".into() };
        }
        self.files.borrow_mut().remove(&t.path.display());
        ActionOutcome::Succeeded { recovery: Recovery::Unavailable }
    }
    fn quarantine_dir_for(&self, _: &ActionTarget<'_>, _: &str) -> Option<RawPath> {
        None
    }
    fn reconcile(&self, t: &ActionTarget<'_>, _: Mode, _: Option<&RawPath>) -> Reconciled {
        match self.files.borrow().get(&t.path.display()) {
            Some(id) if *id == t.identity.file_id => Reconciled::NotPerformed,
            Some(_) => Reconciled::StillUnknown,
            None => Reconciled::Performed,
        }
    }
    fn free_space(&self, _: &RawPath) -> Option<u64> {
        Some(1_000_000 - 10 * self.calls.get() as u64)
    }
    fn remove_empty_dir(&self, t: &ActionTarget<'_>) -> ActionOutcome {
        self.delete(t)
    }
    fn trash_tree(&self, root: &ActionTarget<'_>, members: &[TreeMember<'_>]) -> ActionOutcome {
        for m in members {
            self.files.borrow_mut().remove(&m.path.display());
        }
        self.trash(root)
    }
}

struct View {
    view: MemView,
    next: u64,
}

impl View {
    fn new(root: &str) -> View {
        let mut v = View { view: MemView { platform: Some(Platform::Windows), ..Default::default() }, next: 0 };
        v.view.root = v.add(None, root, ObjectKind::Directory, None);
        v
    }
    fn add(&mut self, parent: Option<u64>, name: &str, kind: ObjectKind, size: Option<u64>) -> u64 {
        self.next += 1;
        let id = self.next;
        self.view.names.insert(id, RawPath::windows(name.encode_utf16().collect()).unwrap());
        self.view.parents.insert(id, parent);
        if let Some(p) = parent {
            self.view.kids.entry(p).or_default().push(id);
        }
        let file_id = format!("fileid128:{id:032x}");
        self.view.facts.insert(id, EntryFacts {
            kind, boundary: false, object_id: id, logical: size, allocated: size, volume_id: "v".into(), file_id: file_id.clone(),
            quality: IdentityQuality::Strong, cloud: CloudState::NotCloud, modified_unix: Some(T0 - 100), link_count: Some(1), skipped_here: 0,
        });
        self.view.identities.insert(("v".into(), file_id), id);
        id
    }
    /// Register every file of the view in the fake file system.
    fn fake(&self) -> Fake {
        let f = Fake::default();
        for (&id, facts) in &self.view.facts {
            f.files.borrow_mut().insert(cm_core::analysis::IndexView::path(&self.view, id).display(), facts.file_id.clone());
        }
        f
    }
}

fn input(plan_id: &str) -> ManualInput<'_> {
    ManualInput { plan_id, scope_id: Some("scope_x"), scan_id: Some("s_x"), scan_revision: Some(1), created_unix: T0, observed_at: "2026-10-15T00:00:00Z", policy_digest: NO_RULES_DIGEST }
}

fn host() -> HostRef {
    HostRef { kind: "cli".into(), pid: 1, started_at: "2026-10-15T00:00:00Z".into() }
}

fn backends<'a>(files: &'a dyn FileActions, startup: Option<&'a dyn StartupActions>, holders: &'a dyn Fn(&[RawPath]) -> Option<Vec<String>>) -> V2Backends<'a> {
    V2Backends { files, startup, holders, protect_item: &|_| Ok(()), processes: None, updates: None, uninstall: None }
}

fn approve(stores: &Stores, plan: &PlanV2, source: ReceiptSource) {
    stores.audit.save_kind_receipt(&KindReceipt {
        receipt_id: format!("r_{}_{source:?}", plan.body.plan_id), plan_id: plan.body.plan_id.clone(), plan_digest: plan.digest.clone(), plan_kind: plan.body.plan_kind,
        principal: PRINCIPAL.into(), source, created_at: plan.body.created_at.clone(), expires_at: plan.body.expires_at.clone(), revoked_at: None,
    }).unwrap();
}

fn apply(stores: &Stores, b: &V2Backends<'_>, plan_id: &str) -> Result<OperationReport, ApplyError> {
    let h = host();
    let ctx = ApplyContext { principal: PRINCIPAL, host: &h, current_rule_set_digest: "", now: &|| T0 + 10, cancel: &|| false, fault: &|_, _| false, protect: &|_| Ok(()) };
    apply_plan_v2(stores, b, plan_id, &ctx)
}

fn manual_plan(v: &View, entries: &[u64], plan_id: &str) -> PlanV2 {
    let user = UserProtection::empty(Platform::Windows);
    build_trash_plan(&v.view, entries, &SelectionGuard { hard: None, user: Ok(&user) }, &input(plan_id)).unwrap()
}

/// MTR-01: an enabled, fully matching cleanup policy never authorizes a manual selection, nor does a policy-sourced or
/// cleanup-typed receipt; only an interactive review of the exact plan does. Re-applying replays the report.
#[test]
fn manual_plans_need_a_review_even_with_an_enabled_policy() {
    let s = stores("policy");
    let mut v = View::new("C:\\r");
    let video = v.add(Some(v.view.root), "video.mov", ObjectKind::File, Some(5_000));
    let fake = v.fake();
    let plan = manual_plan(&v, &[video], "mp_policy");
    s.audit.save_plan_v2(&plan).unwrap();
    assert_eq!(plan_type(&s, "mp_policy").unwrap(), PlanType::V2(PlanKind::ManualSelection));
    let mut pol = Policy::template("pol", PRINCIPAL, "scope_x", vec![], Mode::Trash, T0);
    pol.state = PolicyState::Enabled;
    s.audit.save_policy(&pol, "2026-10-15T00:00:00Z").unwrap();
    let none = |_: &[RawPath]| None;
    let b = backends(&fake, None, &none);
    assert!(matches!(apply(&s, &b, "mp_policy"), Err(ApplyError::NeedsApproval(_))));
    approve(&s, &plan, ReceiptSource::Policy);
    s.audit.save_receipt(&Receipt {
        receipt_id: "rc".into(), plan_id: "mp_policy".into(), plan_digest: plan.digest.clone(), mode: Mode::Trash, principal: PRINCIPAL.into(),
        source: ReceiptSource::InteractiveReview, policy_id: None, policy_version: None, created_at: "t".into(), expires_at: plan.body.expires_at.clone(), revoked_at: None,
    }).unwrap();
    assert!(matches!(apply(&s, &b, "mp_policy"), Err(ApplyError::NeedsApproval(_))), "neither a policy nor a cleanup receipt authorizes it");
    assert_eq!(fake.calls.get(), 0, "nothing touched");
    approve(&s, &plan, ReceiptSource::InteractiveReview);
    assert_eq!(authorization(&s, &plan, PRINCIPAL, T0 + 10).unwrap().0, "authorized");
    let r = apply(&s, &b, "mp_policy").unwrap();
    assert_eq!(r.status, "completed", "{r:?}");
    assert_eq!(r.items[0].recovery, "trash");
    assert!(r.items[0].message.contains(cm_core::manual_trash::PUT_BACK_GUIDANCE), "put-back guidance: {}", r.items[0].message);
    assert_eq!((r.processed_logical_bytes.as_str(), r.estimated_freed_bytes.as_str()), ("5000", "0"), "a bin move frees nothing by itself");
    assert!(r.volume_free_before_bytes.is_some() && r.observed_free_delta_bytes.is_some());
    assert_eq!(apply(&s, &b, "mp_policy").unwrap(), r, "same plan, same operation");
    let (h, _) = crate::recovery::operation_history(&s, 10, None).unwrap();
    assert_eq!((h[0].action.as_deref(), h[0].availability), (Some("trash"), "system_recycle_bin"));
}

/// MTR-04 and D6: an in-use file fails with its holders; an unconfirmed move is unknown with put-back guidance.
#[test]
fn in_use_and_unconfirmed_moves_are_reported_truthfully() {
    let s = stores("inuse");
    let mut v = View::new("C:\\r");
    let log = v.add(Some(v.view.root), "app.log", ObjectKind::File, Some(10));
    let fake = v.fake();
    *fake.in_use.borrow_mut() = Some("C:\\r\\app.log".into());
    let plan = manual_plan(&v, &[log], "mp_inuse");
    s.audit.save_plan_v2(&plan).unwrap();
    approve(&s, &plan, ReceiptSource::InteractiveReview);
    let holders = |paths: &[RawPath]| {
        assert_eq!(paths[0].display(), "C:\\r\\app.log");
        Some(vec!["notepad.exe (Notepad, pid 4242)".to_string()])
    };
    let r = apply(&s, &backends(&fake, None, &holders), "mp_inuse").unwrap();
    assert_eq!((r.items[0].status, r.items[0].code.as_str()), (ActionStatus::Failed, "in_use"));
    assert!(r.items[0].message.contains("notepad.exe") && r.items[0].message.contains("retry"), "{}", r.items[0].message);
    assert_eq!(fake.calls.get(), 0, "never moved, never ends the process");

    let s = stores("unknown");
    let fake = v.fake();
    fake.unknown.set(true);
    let plan = manual_plan(&v, &[log], "mp_unknown");
    s.audit.save_plan_v2(&plan).unwrap();
    approve(&s, &plan, ReceiptSource::InteractiveReview);
    let none = |_: &[RawPath]| None;
    let r = apply(&s, &backends(&fake, None, &none), "mp_unknown").unwrap();
    assert_eq!((r.status.as_str(), r.items[0].status), ("reconciliation_required", ActionStatus::Unknown));
    assert!(r.items[0].message.contains(cm_core::manual_trash::UNKNOWN_GUIDANCE), "{}", r.items[0].message);
}

/// MTR-03: a recycled item's steps run in order and stop at the first failure; the report counts only what was freed.
#[test]
fn purge_steps_stop_per_item_at_the_first_failure() {
    let s = stores("purge");
    let mut v = View::new("C:\\$Recycle.Bin\\S-1-5-21-1");
    let r = v.view.root;
    let rdir = v.add(Some(r), "$RA", ObjectKind::Directory, None);
    v.add(Some(rdir), "one.txt", ObjectKind::File, Some(3));
    let ia = v.add(Some(r), "$IA", ObjectKind::File, Some(60));
    let rb = v.add(Some(r), "$RB.mov", ObjectKind::File, Some(900));
    let ib = v.add(Some(r), "$IB.mov", ObjectKind::File, Some(60));
    let fake = v.fake();
    *fake.fail_delete.borrow_mut() = Some("C:\\$Recycle.Bin\\S-1-5-21-1\\$RA\\one.txt".into());
    let items = [
        BinItem { trash_item_id: "tb_a".into(), original_path: None, deleted_at: None, kind: ObjectKind::Directory, logical_bytes: Some(3), data_entry: rdir, info_entry: Some(ia), refusal: None },
        BinItem { trash_item_id: "tb_b".into(), original_path: None, deleted_at: None, kind: ObjectKind::File, logical_bytes: Some(900), data_entry: rb, info_entry: Some(ib), refusal: None },
    ];
    let plan = build_purge_plan(&v.view, &items.iter().collect::<Vec<_>>(), &input("mp_purge")).unwrap();
    s.audit.save_plan_v2(&plan).unwrap();
    approve(&s, &plan, ReceiptSource::InteractiveReview);
    let none = |_: &[RawPath]| None;
    let rep = apply(&s, &backends(&fake, None, &none), "mp_purge").unwrap();
    let codes: Vec<(&str, ActionStatus)> = rep.items.iter().map(|i| (i.code.as_str(), i.status)).collect();
    assert_eq!(codes, [
        ("access_denied", ActionStatus::Failed),
        ("precondition_descendant_kept", ActionStatus::Skipped),
        ("precondition_descendant_kept", ActionStatus::Skipped),
        ("purged", ActionStatus::Succeeded),
        ("purged", ActionStatus::Succeeded),
    ]);
    assert_eq!(rep.status, "partial");
    assert_eq!(rep.estimated_freed_bytes, "960", "only what was permanently deleted");
    assert!(rep.volume_free_before_bytes.is_some() && rep.volume_free_after_bytes.is_some(), "free-space observation");
    assert!(fake.files.borrow().contains_key("C:\\$Recycle.Bin\\S-1-5-21-1\\$IA"), "the record of a kept item stays");
}

/// A stored body that hashes correctly but breaks its kind is refused before any action; unknown kinds are refused.
#[test]
fn kind_violations_and_unexecuted_kinds_are_refused_up_front() {
    let s = stores("kind");
    let mut v = View::new("C:\\r");
    let video = v.add(Some(v.view.root), "video.mov", ObjectKind::File, Some(5));
    let fake = v.fake();
    let mut plan = manual_plan(&v, &[video], "mp_kind");
    plan.body.plan_kind = PlanKind::Startup;
    plan.digest = plan.body.digest().unwrap();
    s.audit.save_plan_v2(&plan).unwrap();
    approve(&s, &plan, ReceiptSource::InteractiveReview);
    let none = |_: &[RawPath]| None;
    assert!(matches!(apply(&s, &backends(&fake, None, &none), "mp_kind"), Err(ApplyError::PolicyDenied(m)) if m.contains("does not allow")));
    // Uninstall plans are executed now (parity 9.x); maintenance plans are applied by their own worker, never here.
    let mut plan = manual_plan(&v, &[video], "mp_maintenance");
    plan.body.plan_kind = PlanKind::Maintenance;
    plan.digest = plan.body.digest().unwrap();
    s.audit.save_plan_v2(&plan).unwrap();
    approve(&s, &plan, ReceiptSource::InteractiveReview);
    assert!(matches!(apply(&s, &backends(&fake, None, &none), "mp_maintenance"), Err(ApplyError::PolicyDenied(_))));
    assert_eq!(fake.calls.get(), 0);
}

/// In-memory startup entries keyed by value name.
#[derive(Default)]
struct FakeStartup {
    entries: RefCell<BTreeMap<String, StartupObservation>>,
    admin: bool,
}

impl StartupActions for FakeStartup {
    fn observe(&self, loc: &StartupLocation) -> Result<StartupObservation, String> {
        Ok(self.entries.borrow().get(loc.approval_value()).cloned().unwrap_or(StartupObservation { definition: None, approval: None }))
    }
    fn is_admin(&self) -> bool {
        self.admin
    }
    fn set_enabled(&self, loc: &StartupLocation, enabled: bool, expected: &str, backup: cm_core::startup::ToggleBackup<'_>) -> StartupOutcome {
        let obs = self.observe(loc).unwrap();
        if observation_digest(loc, &obs) != expected {
            return StartupOutcome::Skipped { code: "changed_since_plan".into(), message: "changed".into() };
        }
        let written = RegValue::new(3, &cm_core::startup::approval_record(enabled, 7));
        if let Err(e) = backup(&obs, Some(&written)) {
            return StartupOutcome::Failed { code: "backup_failed".into(), message: e };
        }
        self.entries.borrow_mut().get_mut(loc.approval_value()).unwrap().approval = Some(written);
        StartupOutcome::Done
    }
    fn remove_stale(&self, loc: &StartupLocation, _: &str, backup: cm_core::startup::RemovalBackup<'_>) -> StartupOutcome {
        let obs = self.observe(loc).unwrap();
        if let Err(e) = backup(&obs) {
            return StartupOutcome::Failed { code: "backup_failed".into(), message: e };
        }
        self.entries.borrow_mut().remove(loc.approval_value());
        StartupOutcome::Done
    }
    fn restore_approval(&self, loc: &StartupLocation, written: Option<&RegValue>, prior: Option<&RegValue>) -> StartupOutcome {
        let mut e = self.entries.borrow_mut();
        let cur = e.get_mut(loc.approval_value()).unwrap();
        if cur.approval.as_ref() != written {
            return StartupOutcome::Skipped { code: "changed_since".into(), message: "changed".into() };
        }
        cur.approval = prior.cloned();
        StartupOutcome::Done
    }
    fn restore_value(&self, b: &StartupBackup) -> StartupOutcome {
        let mut e = self.entries.borrow_mut();
        if e.contains_key(b.location.approval_value()) {
            return StartupOutcome::Skipped { code: "destination_exists".into(), message: "exists".into() };
        }
        e.insert(b.location.approval_value().into(), StartupObservation { definition: b.prior_definition.clone(), approval: b.prior_approval.clone() });
        StartupOutcome::Done
    }
}

fn startup_item(name: &str, hive: Hive, state: StartupState) -> StartupItem {
    let scope = if hive == Hive::Hklm { StartupScope::System } else { StartupScope::User };
    StartupItem {
        item_id: format!("st_{name}"), name: name.into(), source: StartupSource::RunKey, definition: format!("{} Run : {name}", hive.as_str()), target: None,
        publisher: None, publisher_evidence: PublisherEvidence::None, scope, state, toggle_capability: CapabilityState::Available, protected: false, reason: None,
        location: Some(StartupLocation::RunValue { root: String::new(), hive, view: RegView::Default, key: "Run".into(), value_name: name.into(), approval_key: "Approved\\Run".into() }),
        removable_when_stale: true,
    }
}

/// 11.2/11.3 through the shared executor: the prior state is stored with the operation before the change, a recovery
/// plan built from it puts it back, and history says where to restore from.
#[test]
fn startup_changes_record_prior_state_and_restore_through_a_recovery_plan() {
    let s = stores("startup");
    let fs = Fake::default();
    let st = FakeStartup { admin: false, ..Default::default() };
    let def = RegValue::new(1, b"c\0m\0d\0");
    st.entries.borrow_mut().insert("App".into(), StartupObservation { definition: Some(def.clone()), approval: None });
    st.entries.borrow_mut().insert("Gone".into(), StartupObservation { definition: Some(def.clone()), approval: None });
    let none = |_: &[RawPath]| None;
    let b = backends(&fs, Some(&st), &none);

    let app = startup_item("App", Hive::Hkcu, StartupState::Enabled);
    let obs = st.observe(app.location.as_ref().unwrap()).unwrap();
    let plan = build_startup_plan(&app, StartupOperation::Disable, &obs, false, "sp_1", T0, NO_RULES_DIGEST).unwrap();
    assert!(!plan.body.policy_matchable && !plan.body.requires_admin);
    s.audit.save_plan_v2(&plan).unwrap();
    assert!(matches!(apply(&s, &b, "sp_1"), Err(ApplyError::NeedsApproval(_))));
    approve(&s, &plan, ReceiptSource::InteractiveReview);
    let r = apply(&s, &b, "sp_1").unwrap();
    assert_eq!(r.status, "completed", "{r:?}");
    assert!(!st.observe(app.location.as_ref().unwrap()).unwrap().enabled());
    let backups = s.audit.startup_backups_for_operation(&r.operation_id).unwrap();
    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0].1.prior_approval, None, "the prior state (no record) is stored");
    let (h, _) = crate::recovery::operation_history(&s, 10, None).unwrap();
    assert_eq!((h[0].action.as_deref(), h[0].availability), (Some("startup"), "in_app_restore"));

    let rp = create_startup_restore_plan(&s, &r.operation_id, "rp_1", T0 + 20, NO_RULES_DIGEST).unwrap();
    assert_eq!(rp.body.plan_kind, PlanKind::Recovery);
    assert!(matches!(rp.body.items[0], PlanAction::StartupToggle(ref t) if t.operation == StartupOperation::Restore));
    approve(&s, &rp, ReceiptSource::InteractiveReview);
    let h2 = host();
    let ctx = ApplyContext { principal: PRINCIPAL, host: &h2, current_rule_set_digest: "", now: &|| T0 + 30, cancel: &|| false, fault: &|_, _| false, protect: &|_| Ok(()) };
    let rr = apply_plan_v2(&s, &b, "rp_1", &ctx).unwrap();
    assert_eq!(rr.status, "completed", "{rr:?}");
    assert!(st.observe(app.location.as_ref().unwrap()).unwrap().enabled(), "prior state restored");
    assert!(create_startup_restore_plan(&s, &r.operation_id, "rp_2", T0 + 40, NO_RULES_DIGEST).is_err(), "nothing left to restore");
    // WFA-40: the restore is named as one (never "restore from quarantine") and the change it undid has nothing left.
    let (h, _) = crate::recovery::operation_history(&s, 10, None).unwrap();
    let row = |op: &str| h.iter().find(|x| x.row.operation_id == op).map(|x| (x.action.clone().unwrap_or_default(), x.availability)).unwrap();
    assert_eq!(row(&rr.operation_id), ("startup_restore".into(), "not_applicable"));
    assert_eq!(row(&r.operation_id), ("startup".into(), "not_applicable"));

    // Stale removal and its restore (skipped when the value is recreated).
    let gone = startup_item("Gone", Hive::Hkcu, StartupState::Broken);
    let obs = st.observe(gone.location.as_ref().unwrap()).unwrap();
    let plan = build_startup_plan(&gone, StartupOperation::RemoveWithBackup, &obs, false, "sp_2", T0, NO_RULES_DIGEST).unwrap();
    s.audit.save_plan_v2(&plan).unwrap();
    approve(&s, &plan, ReceiptSource::InteractiveReview);
    let r = apply(&s, &b, "sp_2").unwrap();
    assert_eq!(r.status, "completed");
    assert!(st.entries.borrow().get("Gone").is_none());
    let rp = create_startup_restore_plan(&s, &r.operation_id, "rp_3", T0 + 20, NO_RULES_DIGEST).unwrap();
    assert!(matches!(&rp.body.items[0], PlanAction::RegistryRestore(x) if x.on_existing == "skip" && x.value_name.as_deref() == Some("Gone")));
    st.entries.borrow_mut().insert("Gone".into(), StartupObservation { definition: Some(RegValue::new(1, b"n\0")), approval: None });
    approve(&s, &rp, ReceiptSource::InteractiveReview);
    let rr = apply_plan_v2(&s, &b, "rp_3", &ctx).unwrap();
    assert_eq!(rr.items[0].code, "destination_exists");
    assert_eq!(st.entries.borrow()["Gone"].definition, Some(RegValue::new(1, b"n\0")), "the recreated value is kept");
}

/// System-scope entries are refused without an elevated token (injected), at planning and at application.
#[test]
fn system_scope_changes_need_an_administrator() {
    let s = stores("admin");
    let fs = Fake::default();
    let st = FakeStartup { admin: false, ..Default::default() };
    st.entries.borrow_mut().insert("Svc".into(), StartupObservation { definition: Some(RegValue::new(1, b"x\0")), approval: None });
    let item = startup_item("Svc", Hive::Hklm, StartupState::Enabled);
    let obs = st.observe(item.location.as_ref().unwrap()).unwrap();
    assert_eq!(build_startup_plan(&item, StartupOperation::Disable, &obs, false, "sp_a", T0, NO_RULES_DIGEST).unwrap_err().code, "permission_required");
    // Planned by an administrator, applied by a process that is not one: skipped, nothing written.
    let plan = build_startup_plan(&item, StartupOperation::Disable, &obs, true, "sp_a", T0, NO_RULES_DIGEST).unwrap();
    assert!(plan.body.requires_admin);
    s.audit.save_plan_v2(&plan).unwrap();
    approve(&s, &plan, ReceiptSource::InteractiveReview);
    let none = |_: &[RawPath]| None;
    let r = apply(&s, &backends(&fs, Some(&st), &none), "sp_a").unwrap();
    assert_eq!(r.items[0].code, "permission_required");
    assert!(st.entries.borrow()["Svc"].approval.is_none());
}

/// EXE-05 for plan/v2: a crash after the action leaves an intent without a result; reconciliation decides from the
/// live state and never acts.
#[test]
fn interrupted_trash_moves_are_reconciled() {
    let s = stores("reconcile");
    let mut v = View::new("C:\\r");
    let a = v.add(Some(v.view.root), "a.bin", ObjectKind::File, Some(1));
    let fake = v.fake();
    let plan = manual_plan(&v, &[a], "mp_crash");
    s.audit.save_plan_v2(&plan).unwrap();
    approve(&s, &plan, ReceiptSource::InteractiveReview);
    let none = |_: &[RawPath]| None;
    let b = backends(&fake, None, &none);
    let h = host();
    let ctx = ApplyContext { principal: PRINCIPAL, host: &h, current_rule_set_digest: "", now: &|| T0 + 10, cancel: &|| false, fault: &|p, _| p == FaultPoint::AfterAction, protect: &|_| Ok(()) };
    assert!(matches!(apply_plan_v2(&s, &b, "mp_crash", &ctx), Err(ApplyError::Crashed(_))));
    let op = format!("op_{}", &plan.digest[..16]);
    // WFA-40: before it is reconciled, history names the interactive review that authorized it and what is known.
    assert_eq!(s.audit.reconcile_hosts(|_| cm_core::host::Liveness::Exited).unwrap(), std::slice::from_ref(&op));
    let (h, _) = crate::recovery::operation_history(&s, 10, None).unwrap();
    let listed = h[0].report.as_ref().expect("provisional report");
    assert_eq!((h[0].row.state.as_str(), listed.authorization_source.as_str(), listed.unknown, h[0].availability), ("reconciliation_required", "interactive_review", 1, "unknown"));
    let r = reconcile_operation_v2(&s, &b, &op, &ctx).unwrap();
    assert_eq!((r.items[0].code.as_str(), r.items[0].recovery.as_str()), ("reconciled_performed", "trash"));
    assert_eq!(fake.calls.get(), 1, "reconciliation never acts");
}

fn header(plan_id: &str, plan_kind: PlanKind) -> cm_core::plan::v2::HeaderInput<'_> {
    cm_core::plan::v2::HeaderInput {
        plan_id, plan_kind, created_unix: T0, scope_id: None, scan_id: None, scan_revision: None, rule_set_digest: NO_RULES_DIGEST, policy_digest: NO_RULES_DIGEST,
    }
}

/// Store `plan` and a finished operation of it in which exactly the actions `ok` succeeded (the others were skipped),
/// as the executor records it.
fn finished_op(s: &Stores, plan: &PlanV2, ok: &[&str]) -> String {
    s.audit.save_plan_v2(plan).unwrap();
    let op = format!("op_{}", &plan.digest[..16]);
    let at = utc_from_unix(T0 + 10);
    s.audit.begin_operation(&op, &plan.body.plan_id, &plan.digest, &format!("{}#{}", plan.body.plan_id, plan.digest), &host(), &at).unwrap();
    let items: Vec<ActionResult> = plan
        .body
        .items
        .iter()
        .map(|a| match ok.contains(&a.action_id()) {
            true => res(a.action_id(), ActionStatus::Succeeded, "done", "done", "not_applicable"),
            false => res(a.action_id(), ActionStatus::Skipped, "skipped", "skipped", "not_applicable"),
        })
        .collect();
    let (succeeded, skipped, failed, not_started, unknown) = summarize(&items);
    let report = OperationReport {
        operation_id: op.clone(), plan_id: plan.body.plan_id.clone(), plan_digest: plan.digest.clone(), status: final_status(&items, false).into(), started_at: at.clone(),
        finished_at: at.clone(), succeeded, skipped, failed, not_started, unknown, items, processed_logical_bytes: "0".into(), quarantined_logical_bytes: "0".into(),
        estimated_freed_bytes: "0".into(), volume_free_before_bytes: None, volume_free_after_bytes: None, observed_free_delta_bytes: None,
        authorization_source: "interactive_review".into(), reconciled_at: None,
    };
    s.audit.finish_operation(&op, &report.status, &serde_json::to_string(&report).unwrap(), &at).unwrap();
    op
}

/// WFA-40: every `plan/v2` kind this build executes says how its operation is recovered (never "plan cannot be read"):
/// an ended process and an update cannot be undone, maintenance tasks are not undone, restores leave nothing to undo,
/// uninstall leftovers are restorable from their backups and the Recycle Bin. (Manual selection and startup: the flows
/// above; stage-1 uninstalls: `uninstall::tests`.)
#[test]
fn history_describes_the_recovery_of_every_plan_kind() {
    use cm_core::plan::v2::{ExecutableRef, MaintenanceTaskAction, ProcessEndAction, RegistryBackupDeleteAction, RegistryRestoreAction, ToolAdapterAction};
    use cm_core::uninstall::{KEY_BACKUP_PREFIX, KeyBackup, KeyTree, RegLocation, UninstallMeta, key_digest};
    let s = stores("history_kinds");
    let build = |id: &str, kind: PlanKind, items: Vec<PlanAction>, meta: Vec<ActionMeta>| cm_core::plan::v2::build(&header(id, kind), items, meta).unwrap();
    let facts = |plan: &PlanV2, op: &str| {
        let (action, availability, guidance) = history_facts(&s, &plan.body.plan_id, op).expect("every executed kind has facts");
        let (h, _) = crate::recovery::operation_history(&s, 50, None).unwrap();
        let row = h.iter().find(|x| x.row.operation_id == op).unwrap();
        assert_eq!((row.action.as_deref(), row.availability, row.guidance.as_str()), (Some(action), availability, guidance.as_str()), "history uses the facts");
        (action, availability, guidance)
    };

    let process = build("hp_process", PlanKind::Process, vec![PlanAction::ProcessEnd(ProcessEndAction {
        action_id: "e0000000".into(), pid: 4242, started_at: "133000000000000000".into(), started_at_utc: None, name: "foo.exe".into(), image_path: None, image_sha256: None,
    })], vec![ActionMeta::default()]);
    let op = finished_op(&s, &process, &["e0000000"]);
    let (action, availability, guidance) = facts(&process, &op);
    assert_eq!((action, availability), ("process", "unavailable"));
    assert!(guidance.contains("cannot be brought back"), "{guidance}");

    let tool = |id: &str| ToolAdapterAction {
        action_id: id.into(), adapter_id: "winget.upgrade".into(), adapter_version: "1".into(),
        executable: ExecutableRef { path: RawPath::windows("C:\\winget.exe".encode_utf16().collect()).unwrap(), sha256: "a".repeat(64) }, argv: vec![],
        preview_digest: "b".repeat(64), timeout_seconds: 60,
    };
    let update = build("hp_update", PlanKind::Update, vec![PlanAction::ToolAdapter(tool("u0000000"))], vec![ActionMeta::default()]);
    let op = finished_op(&s, &update, &["u0000000"]);
    let (action, availability, guidance) = facts(&update, &op);
    assert_eq!((action, availability), ("update", "unavailable"));
    assert!(guidance.contains("does not roll them back"), "{guidance}");

    let task = MaintenanceTaskAction { action_id: "t0000000".into(), task_id: "win.dns_flush".into(), task_version: "1".into(), requires_admin: false, volume: None };
    let maintenance = build("hp_maint", PlanKind::Maintenance, vec![PlanAction::MaintenanceTask(task)], vec![ActionMeta::default()]);
    let op = finished_op(&s, &maintenance, &["t0000000"]);
    assert_eq!(facts(&maintenance, &op).1, "unavailable", "maintenance tasks are not undone");

    // Restores, named by what they put back (the desktop names them the same way).
    let setting = SettingsChangeAction { action_id: "sb_x_t0000001".into(), setting_id: "s".into(), from_value: "on".into(), to_value: "off".into(), restore_on_exit: false };
    let settings = build("hp_rset", PlanKind::Recovery, vec![PlanAction::SettingsChange(setting)], vec![ActionMeta::default()]);
    let op = finished_op(&s, &settings, &["sb_x_t0000001"]);
    let (action, availability, _) = facts(&settings, &op);
    assert_eq!((action, availability), ("maintenance_restore", "not_applicable"));
    let location = RegLocation { root: String::new(), hive: Hive::Hkcu, view: RegView::Default, key_path: "SOFTWARE\\Foo".into() };
    let key_restore = build("hp_rkey", PlanKind::Recovery, vec![PlanAction::RegistryRestore(RegistryRestoreAction {
        action_id: "r0000000".into(), backup_id: format!("{KEY_BACKUP_PREFIX}x_k0000000"), hive: Hive::Hkcu, key_path: location.key_path.clone(), value_name: None,
        backup_sha256: Some("c".repeat(64)), on_existing: "skip".into(),
    })], vec![ActionMeta { uninstall: Some(UninstallMeta::RegistryKey { location: location.clone() }), ..Default::default() }]);
    let op = finished_op(&s, &key_restore, &["r0000000"]);
    let (action, availability, _) = facts(&key_restore, &op);
    assert_eq!((action, availability), ("uninstall_restore", "not_applicable"), "a registry restore is not a restore from quarantine");

    // Uninstall leftovers: a file moved to the Recycle Bin and a registry key deleted after its backup.
    let mut v = View::new("C:\\Users\\alice\\AppData\\Local\\Foo");
    let cache = v.add(Some(v.view.root), "cache.bin", ObjectKind::File, Some(10));
    let trash = manual_plan(&v, &[cache], "hp_tmp");
    let tree = KeyTree::default();
    let delete_key = PlanAction::RegistryBackupDelete(RegistryBackupDeleteAction {
        action_id: "k0000000".into(), hive: Hive::Hkcu, key_path: location.key_path.clone(), value_name: None, owner_evidence: "product name".into(), backup_required: true,
        observed_sha256: Some(key_digest(&location, &tree)),
    });
    let mut items = trash.body.items.clone();
    let mut meta = trash.meta.clone();
    items.push(delete_key);
    meta.push(ActionMeta { uninstall: Some(UninstallMeta::RegistryKey { location: location.clone() }), ..Default::default() });
    let leftovers = build("hp_left", PlanKind::Uninstall, items, meta);
    let file_id = leftovers.body.items[0].action_id().to_owned();
    let op = finished_op(&s, &leftovers, &[file_id.as_str(), "k0000000"]);
    let backup = KeyBackup { backup_id: format!("{KEY_BACKUP_PREFIX}{}_k0000000", &op[3..]), location, tree, owner_evidence: "product name".into(), created_at: "t".into() };
    s.audit.save_key_backup(&op, "k0000000", "hp_left", &backup).unwrap();
    let (action, availability, guidance) = facts(&leftovers, &op);
    assert_eq!((action, availability), ("uninstall_leftovers", "in_app_restore"));
    assert!(guidance.contains(&format!("apps uninstall restore --operation {op}")) && guidance.contains(cm_core::manual_trash::BIN_NAME), "{guidance}");
    // Once the key is restored only the Recycle Bin is left.
    s.audit.set_startup_backup_state(&backup.backup_id, "restored").unwrap();
    assert_eq!(facts(&leftovers, &op).1, "system_recycle_bin");
    // Only the key was planned and it is restored: nothing is left to recover.
    let only_key = build("hp_left2", PlanKind::Uninstall, vec![leftovers.body.items[1].clone()], vec![leftovers.meta[1].clone()]);
    let op = finished_op(&s, &only_key, &["k0000000"]);
    let only_key_backup = KeyBackup { backup_id: format!("{KEY_BACKUP_PREFIX}{}_k0000000", &op[3..]), ..backup };
    s.audit.save_key_backup(&op, "k0000000", "hp_left2", &only_key_backup).unwrap();
    assert_eq!(facts(&only_key, &op).1, "in_app_restore");
    s.audit.set_startup_backup_state(&only_key_backup.backup_id, "restored").unwrap();
    let (action, availability, _) = facts(&only_key, &op);
    assert_eq!((action, availability), ("uninstall_leftovers", "not_applicable"));
    // A kind this build never executes has no facts (history says it cannot tell).
    assert!(!executable(PlanKind::Cleanup));
}
