//! Uninstall actions on the shared executor with an injected backend: no process is started or ended, no registry
//! key is touched; the fake records what the executor asked for.

use std::cell::{Cell, RefCell};

use super::*;
use crate::execute::ApplyContext;
use crate::plans::{V2Backends, apply_plan_v2};
use cm_core::authz::{KindReceipt, ReceiptSource};
use cm_core::execute::{ActionOutcome, ActionTarget, FileActions, LiveCheck, Reconciled};
use cm_core::host::HostRef;
use cm_core::path::RawPath;
use cm_core::plan::v2::{ExecutableRef, Hive, RegistryBackupDeleteAction, ToolAdapterAction};
use cm_core::startup::{RegValue, RegView};
use cm_core::uninstall::{ADAPTER_VERSION, AdapterChoice, AdapterKind, KeyTree, RecordSource, RegLocation, Rescan, RunningProcess, UninstallRecord, adapter_digest};

const T0: i64 = 1_800_000_000;
const PRINCIPAL: &str = "user:alice";

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cm-app-uninstall-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct NoFiles;

impl FileActions for NoFiles {
    fn check(&self, _: &ActionTarget<'_>) -> LiveCheck {
        unreachable!("no file actions here")
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

/// Scripted platform: what is running, what closes, whether a person confirms, the uninstaller's exit and the rescan.
struct Fake {
    running: RunningState,
    closes: bool,
    confirm: bool,
    exit: Option<i32>,
    rescan: Rescan,
    /// UNI-11: the uninstaller of this program exits with 1 (the others with `exit`).
    fail_program: Option<String>,
    /// UNI-11: only this instance has running processes (`None`: `running` applies to every app).
    running_app: Option<String>,
    key: RefCell<Option<KeyTree>>,
    ran: Cell<u32>,
    ended: Cell<u32>,
    deleted: Cell<u32>,
}

impl Fake {
    fn new(running: RunningState) -> Fake {
        Fake {
            running, closes: false, confirm: false, exit: Some(0), rescan: Rescan { install_dir_present: Some(false), registration_present: Some(false) },
            fail_program: None, running_app: None, key: RefCell::new(None), ran: Cell::new(0), ended: Cell::new(0), deleted: Cell::new(0),
        }
    }
}

impl UninstallActions for Fake {
    fn is_admin(&self) -> bool {
        false
    }
    fn running(&self, identity: &AppIdentity) -> RunningState {
        match &self.running_app {
            Some(id) if *id != identity.instance_id => RunningState::Known { processes: vec![] },
            _ => self.running.clone(),
        }
    }
    fn request_quit(&self, p: &[RunningProcess]) -> Vec<RunningProcess> {
        if self.closes { vec![] } else { p.to_vec() }
    }
    fn confirm_end(&self, _: &[RunningProcess]) -> bool {
        self.confirm
    }
    fn end_process(&self, _: &RunningProcess) -> Result<(), String> {
        self.ended.set(self.ended.get() + 1);
        Ok(())
    }
    fn verify(&self, _: &UninstallRecord, _: &AdapterChoice, _: &str, _: Option<&str>) -> Result<(), (String, String)> {
        Ok(())
    }
    fn run(&self, choice: &AdapterChoice, _: u32) -> Result<(Option<i32>, u64), String> {
        self.ran.set(self.ran.get() + 1);
        Ok((if self.fail_program.as_deref() == Some(choice.program.as_str()) { Some(1) } else { self.exit }, 42))
    }
    fn rescan(&self, _: &AppIdentity) -> Rescan {
        self.rescan
    }
    fn read_key(&self, _: &RegLocation) -> Result<Option<KeyTree>, String> {
        Ok(self.key.borrow().clone())
    }
    fn delete_key(&self, _: &RegLocation) -> Result<(), (String, String)> {
        self.deleted.set(self.deleted.get() + 1);
        *self.key.borrow_mut() = None;
        Ok(())
    }
    fn restore_key(&self, b: &KeyBackup) -> Result<(), (String, String)> {
        if self.key.borrow().is_some() {
            return Err(("destination_exists".into(), "exists again".into()));
        }
        *self.key.borrow_mut() = Some(b.tree.clone());
        Ok(())
    }
}

fn record() -> UninstallRecord {
    named("app_foo", "Foo")
}

/// A registered app `name` installed in `C:\<name>` with its uninstaller `C:\<name>\u.exe`.
fn named(id: &str, name: &str) -> UninstallRecord {
    UninstallRecord {
        instance_id: id.into(), name: name.into(), version: None, publisher: Some("Contoso".into()), source: RecordSource::RegistryUninstall, key: None,
        key_name: Some(name.into()), product_code: None, package_full_name: None, package_family: None, uninstall_string: Some(format!(r#""C:\{name}\u.exe""#)),
        quiet_uninstall_string: None, install_location: Some(format!(r"C:\{name}")), system_component: false, winget: None, package_name: None, ai_tool: None, protected_class: None,
    }
}

fn stage_one_plan(force: QuitForce, plan_id: &str) -> PlanV2 {
    batch_plan(force, plan_id, &[record()])
}

/// One stage-1 plan for these apps, with the action ids `create_uninstall_batch` gives them (UNI-11).
fn batch_plan(force: QuitForce, plan_id: &str, recs: &[UninstallRecord]) -> PlanV2 {
    let mut items = Vec::new();
    let mut meta = Vec::new();
    for (n, rec) in recs.iter().enumerate() {
        let dir = rec.install_location.clone().unwrap();
        let choice = AdapterChoice {
            kind: AdapterKind::RegisteredCommand, program: format!(r"{dir}\u.exe"), argv: vec![], raw_args: true, publisher_required: Some("Contoso".into()),
            microsoft_signed: false, display: "u.exe".into(), keeps_own_ui: true,
        };
        let m = UninstallMeta::App { identity: AppIdentity::of(rec), record: Box::new(rec.clone()), adapter: choice.clone(), signer: Some("Contoso".into()) };
        items.push(PlanAction::AppQuit(AppQuitAction { action_id: batch_action_id(n, false), app_instance_id: rec.instance_id.clone(), app_identity: dir.clone(), force }));
        items.push(PlanAction::ToolAdapter(ToolAdapterAction {
            action_id: batch_action_id(n, true), adapter_id: choice.kind.adapter_id().into(), adapter_version: ADAPTER_VERSION.into(),
            executable: ExecutableRef { path: RawPath::windows(choice.program.encode_utf16().collect()).unwrap(), sha256: "a".repeat(64) }, argv: vec![],
            preview_digest: adapter_digest(rec, &choice), timeout_seconds: 60,
        }));
        meta.push(ActionMeta { uninstall: Some(m.clone()), ..Default::default() });
        meta.push(ActionMeta { uninstall: Some(m), ..Default::default() });
    }
    let header = HeaderInput {
        plan_id, plan_kind: PlanKind::Uninstall, created_unix: T0, scope_id: None, scan_id: None, scan_revision: None, rule_set_digest: NO_RULES_DIGEST, policy_digest: NO_RULES_DIGEST,
    };
    cm_core::plan::v2::build(&header, items, meta).unwrap()
}

fn approve(stores: &Stores, plan: &PlanV2) {
    // Restore plans are stored by `create_restore_plan` already.
    if stores.audit.load_plan_v2(&plan.body.plan_id).is_err() {
        stores.audit.save_plan_v2(plan).unwrap();
    }
    stores
        .audit
        .save_kind_receipt(&KindReceipt {
            receipt_id: format!("r_{}", plan.body.plan_id), plan_id: plan.body.plan_id.clone(), plan_digest: plan.digest.clone(), plan_kind: plan.body.plan_kind,
            principal: PRINCIPAL.into(), source: ReceiptSource::InteractiveReview, created_at: plan.body.created_at.clone(), expires_at: plan.body.expires_at.clone(), revoked_at: None,
        })
        .unwrap();
}

fn apply(stores: &Stores, fake: &Fake, plan_id: &str) -> OperationReport {
    let b = V2Backends { files: &NoFiles, startup: None, holders: &|_| None, protect_item: &|_| Ok(()), processes: None, updates: None, uninstall: Some(fake) };
    let h = HostRef { kind: "cli".into(), pid: 1, started_at: "2026-10-15T00:00:00Z".into() };
    let ctx = ApplyContext { principal: PRINCIPAL, host: &h, current_rule_set_digest: "", now: &|| T0 + 10, cancel: &|| false, fault: &|_, _| false, protect: &|_| Ok(()) };
    apply_plan_v2(stores, &b, plan_id, &ctx).unwrap()
}

fn proc() -> RunningProcess {
    RunningProcess { pid: 4242, name: "foo.exe".into(), path: Some(r"C:\Foo\foo.exe".into()) }
}

#[test]
fn an_app_that_refuses_to_quit_is_never_uninstalled_or_ended_without_confirmation() {
    // UNI-02 scenario: the close request times out; cancelling (no confirmation) deletes and runs nothing.
    let stores = Stores::open(&dir("quit")).unwrap();
    for (force, confirm) in [(QuitForce::Never, true), (QuitForce::AskUser, false)] {
        let plan = stage_one_plan(force, &format!("up_{force:?}_{confirm}"));
        approve(&stores, &plan);
        let fake = Fake { confirm, ..Fake::new(RunningState::Known { processes: vec![proc()] }) };
        let r = apply(&stores, &fake, &plan.body.plan_id);
        assert_eq!((r.items[0].status, r.items[0].code.as_str()), (ActionStatus::Failed, "still_running"), "{:?}", r.items[0]);
        assert!(r.items[0].message.contains("foo.exe (pid 4242)"));
        assert_eq!((r.items[1].status, r.items[1].code.as_str()), (ActionStatus::Skipped, "app_still_running"));
        assert_eq!((fake.ran.get(), fake.ended.get()), (0, 0), "nothing ran, nothing was ended");
        assert_ne!(r.status, "completed");
    }
    // Only the separately reviewed plan, plus a person's confirmation, ends it.
    let plan = stage_one_plan(QuitForce::AskUser, "up_end");
    approve(&stores, &plan);
    let fake = Fake { confirm: true, ..Fake::new(RunningState::Known { processes: vec![proc()] }) };
    let r = apply(&stores, &fake, "up_end");
    assert_eq!((r.items[0].code.as_str(), fake.ended.get(), fake.ran.get()), ("ended_on_request", 1, 1));
}

#[test]
fn unknown_running_state_skips_the_uninstall() {
    let stores = Stores::open(&dir("unknown")).unwrap();
    let plan = stage_one_plan(QuitForce::Never, "up_unknown");
    approve(&stores, &plan);
    let fake = Fake::new(RunningState::Unknown { reason: "image unreadable".into() });
    let r = apply(&stores, &fake, "up_unknown");
    assert_eq!((r.items[0].status, r.items[0].code.as_str()), (ActionStatus::Skipped, "running_state_unknown"));
    assert_eq!(r.items[1].code, "app_still_running");
    assert_eq!(fake.ran.get(), 0);
}

#[test]
fn a_successful_uninstaller_that_leaves_the_folder_is_partial() {
    // UNI-03 scenario: exit code 0 but the rescan still finds the install folder.
    let stores = Stores::open(&dir("partial")).unwrap();
    let plan = stage_one_plan(QuitForce::Never, "up_partial");
    approve(&stores, &plan);
    let fake = Fake { rescan: Rescan { install_dir_present: Some(true), registration_present: Some(false) }, ..Fake::new(RunningState::Known { processes: vec![] }) };
    let r = apply(&stores, &fake, "up_partial");
    assert_eq!((r.items[0].code.as_str(), r.items[1].status, r.items[1].code.as_str()), ("not_running", ActionStatus::Succeeded, "leftovers_remain"));
    assert!(r.items[1].message.contains("code 0") && r.items[1].message.contains("42 ms") && r.items[1].message.contains(&r.operation_id), "{}", r.items[1].message);
    assert_eq!(r.status, "partial", "never complete while something is left");
    // Clean rescan: complete.
    let plan = stage_one_plan(QuitForce::Never, "up_clean");
    approve(&stores, &plan);
    let fake = Fake::new(RunningState::Known { processes: vec![] });
    assert_eq!(apply(&stores, &fake, "up_clean").status, "completed");
    // The stage-1 identity is available for the separately reviewed leftovers plan.
    let (_, apps, report) = stage_one(&stores, &r.operation_id).unwrap();
    assert_eq!((apps.len(), apps[0].identity.instance_id.as_str(), report.is_some()), (1, "app_foo", true));
    assert_eq!(apps[0].action_ids, ["q0000000", "u0000001"], "a single uninstall keeps its action ids");
}

fn key_plan(plan_id: &str, observed: &KeyTree) -> (PlanV2, RegLocation) {
    let loc = RegLocation { root: r"Software\CleanManagerTest\fake".into(), hive: Hive::Hkcu, view: RegView::Default, key_path: r"SOFTWARE\Foo".into() };
    let items = vec![PlanAction::RegistryBackupDelete(RegistryBackupDeleteAction {
        action_id: "k0000000".into(), hive: Hive::Hkcu, key_path: loc.key_path.clone(), value_name: None, owner_evidence: "matches the product name".into(),
        backup_required: true, observed_sha256: Some(key_digest(&loc, observed)),
    })];
    let meta = vec![ActionMeta { uninstall: Some(UninstallMeta::RegistryKey { location: loc.clone() }), ..Default::default() }];
    let header = HeaderInput {
        plan_id, plan_kind: PlanKind::Uninstall, created_unix: T0, scope_id: None, scan_id: None, scan_revision: None, rule_set_digest: NO_RULES_DIGEST, policy_digest: NO_RULES_DIGEST,
    };
    (cm_core::plan::v2::build(&header, items, meta).unwrap(), loc)
}

#[test]
fn registry_leftovers_need_a_stored_backup_and_restores_never_overwrite() {
    let stores = Stores::open(&dir("keys")).unwrap();
    let tree = KeyTree { values: vec![("Theme".into(), RegValue::new(1, b"d\0\0\0"))], subkeys: vec![] };
    // UNI-06 scenario: the backup cannot be written → the delete is skipped and reported.
    let (plan, _) = key_plan("lp_fail", &tree);
    approve(&stores, &plan);
    let op = format!("op_{}", &plan.digest[..16]);
    let clash = KeyBackup { backup_id: format!("ub_{}_k0000000", &op[3..]), location: RegLocation { root: String::new(), hive: Hive::Hkcu, view: RegView::Default, key_path: "x".into() },
        tree: KeyTree::default(), owner_evidence: String::new(), created_at: "2026-10-15T00:00:00Z".into() };
    stores.audit.save_key_backup("op_other", "k", "p", &clash).unwrap();
    let fake = Fake { key: RefCell::new(Some(tree.clone())), ..Fake::new(RunningState::Known { processes: vec![] }) };
    let r = apply(&stores, &fake, "lp_fail");
    assert_eq!((r.items[0].status, r.items[0].code.as_str()), (ActionStatus::Skipped, "backup_failed"));
    assert_eq!(fake.deleted.get(), 0, "no backup, no delete");
    // Changed since planning → skipped.
    let (plan, _) = key_plan("lp_changed", &KeyTree::default());
    approve(&stores, &plan);
    assert_eq!(apply(&stores, &fake, "lp_changed").items[0].code, "precondition_changed");
    assert_eq!(fake.deleted.get(), 0);
    // Normal: backup stored, then deleted; the restore plan recreates it, but never over a recreated key.
    let (plan, loc) = key_plan("lp_ok", &tree);
    approve(&stores, &plan);
    let r = apply(&stores, &fake, "lp_ok");
    assert_eq!((r.items[0].code.as_str(), fake.deleted.get(), r.status.as_str()), ("done", 1, "completed"));
    let rp = create_restore_plan(&stores, &r.operation_id, "rp_1", T0 + 20, NO_RULES_DIGEST).unwrap();
    assert_eq!(rp.body.plan_kind, PlanKind::Recovery);
    let PlanAction::RegistryRestore(rr) = &rp.body.items[0] else { panic!() };
    assert_eq!((rr.on_existing.as_str(), rr.value_name.as_ref()), ("skip", None));
    *fake.key.borrow_mut() = Some(KeyTree::default());
    approve(&stores, &rp);
    let rr = apply(&stores, &fake, "rp_1");
    assert_eq!((rr.items[0].status, rr.items[0].code.as_str()), (ActionStatus::Skipped, "destination_exists"));
    assert_eq!(fake.key.borrow().as_ref(), Some(&KeyTree::default()), "the recreated key is left as it is");
    // Not yet recreated: restored from the backup.
    *fake.key.borrow_mut() = None;
    let rp2 = create_restore_plan(&stores, &r.operation_id, "rp_2", T0 + 30, NO_RULES_DIGEST).unwrap();
    approve(&stores, &rp2);
    assert_eq!(apply(&stores, &fake, "rp_2").items[0].code, "done");
    assert_eq!(fake.key.borrow().as_ref(), Some(&tree));
    assert_eq!(loc.hive, Hive::Hkcu);
}

#[test]
fn uninstall_kinds_are_dispatched_here_and_others_are_not() {
    let plan = stage_one_plan(QuitForce::Never, "x");
    assert!(plan.body.items.iter().all(|a| handles(PlanKind::Uninstall, a)));
    assert!(!handles(PlanKind::Update, &plan.body.items[1]), "update tool adapters belong to the update service");
    assert!(!plan.body.policy_matchable, "uninstall plans are never policy-matched");
}

#[test]
fn a_batch_keeps_going_when_one_uninstaller_fails() {
    // UNI-11 scenario: two applications, one vendor uninstaller returns failure; the other completes.
    let stores = Stores::open(&dir("batch")).unwrap();
    let plan = batch_plan(QuitForce::Never, "up_batch", &[named("app_foo", "Foo"), named("app_bar", "Bar")]);
    assert_eq!(plan.body.items.iter().map(|a| a.action_id()).collect::<Vec<_>>(), ["q0000000", "u0000001", "u0010000", "u0010001"]);
    approve(&stores, &plan);
    let fake = Fake { fail_program: Some(r"C:\Foo\u.exe".into()), ..Fake::new(RunningState::Known { processes: vec![] }) };
    let r = apply(&stores, &fake, "up_batch");
    assert_eq!(fake.ran.get(), 2, "the failure of one uninstaller does not stop the next application");
    assert_eq!((r.items[1].status, r.items[1].code.as_str()), (ActionStatus::Failed, "uninstaller_failed"));
    assert_eq!((r.items[3].status, r.items[3].code.as_str()), (ActionStatus::Succeeded, "done"));
    assert_eq!(r.status, "partial", "never completed while one application failed");
    let out = app_outcomes(&plan, Some(&r));
    let by = |id: &str| out.iter().find(|o| o.app_instance_id == id).unwrap().clone();
    assert_eq!((by("app_foo").status, by("app_foo").code.as_deref(), by("app_foo").leftovers_reviewable), ("failed", Some("uninstaller_failed"), false));
    assert_eq!((by("app_bar").status, by("app_bar").leftovers_reviewable), ("completed", true));
    assert_eq!(by("app_bar").action_ids, ["u0010000", "u0010001"]);
    // The stored operation lists both applications for the leftover review, in plan order.
    let (_, apps, report) = stage_one(&stores, &r.operation_id).unwrap();
    assert_eq!((apps.iter().map(|a| a.identity.name.as_str()).collect::<Vec<_>>(), report.is_some()), (vec!["Foo", "Bar"], true));
}

#[test]
fn an_app_that_does_not_quit_stops_only_its_own_uninstaller() {
    // UNI-02 per application: Foo refuses to close, Bar is not running; Bar is uninstalled, Foo is not, nothing is ended.
    let stores = Stores::open(&dir("batch_quit")).unwrap();
    let plan = batch_plan(QuitForce::Never, "up_bq", &[named("app_foo", "Foo"), named("app_bar", "Bar")]);
    approve(&stores, &plan);
    let fake = Fake { running_app: Some("app_foo".into()), ..Fake::new(RunningState::Known { processes: vec![proc()] }) };
    let r = apply(&stores, &fake, "up_bq");
    let codes: Vec<&str> = r.items.iter().map(|i| i.code.as_str()).collect();
    assert_eq!(codes, ["still_running", "app_still_running", "not_running", "done"]);
    assert_eq!((fake.ran.get(), fake.ended.get()), (1, 0));
    let out = app_outcomes(&plan, Some(&r));
    assert_eq!((out[0].status, out[0].code.as_deref(), out[0].leftovers_reviewable), ("skipped", Some("still_running"), false));
    assert_eq!((out[1].status, out[1].leftovers_reviewable), ("completed", true));
}

#[test]
fn outcomes_are_per_application_and_pending_before_a_report() {
    let plan = batch_plan(QuitForce::Never, "up_p", &[named("app_foo", "Foo"), named("app_bar", "Bar")]);
    let apps = planned_apps(&plan);
    assert_eq!(apps.iter().map(|a| a.action_ids.len()).collect::<Vec<_>>(), [2, 2]);
    assert_eq!(apps[0].record_digest, named("app_foo", "Foo").digest());
    assert!(app_outcomes(&plan, None).iter().all(|o| o.status == "pending" && !o.leftovers_reviewable));
    // A leftovers plan (no uninstaller) has no applications.
    let (keys, _) = key_plan("lp_x", &KeyTree::default());
    assert!(planned_apps(&keys).is_empty());
}

#[test]
fn a_whole_batch_is_refused_only_when_every_application_is() {
    let one = Refused { code: "policy_denied".into(), message: "endpoint security agent".into(), settings_uri: None };
    assert_eq!(batch_refusal(vec![("Agent".into(), one.clone())]), one, "a single refusal is reported as it is");
    let r = batch_refusal(vec![("Agent".into(), one.clone()), ("Runtime".into(), Refused { message: "shared runtime".into(), ..one.clone() })]);
    assert_eq!(r.code, "policy_denied");
    assert!(r.message.contains("Agent: endpoint security agent") && r.message.contains("Runtime: shared runtime"), "{}", r.message);
    let mixed = batch_refusal(vec![("Agent".into(), one), ("Tool".into(), Refused { code: "unsupported".into(), message: "no route".into(), settings_uri: Some("ms-settings:appsfeatures".into()) })]);
    assert_eq!((mixed.code.as_str(), mixed.settings_uri.as_deref()), ("invalid_argument", Some("ms-settings:appsfeatures")));
}
