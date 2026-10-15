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
use cm_core::plan::v2::Hive;
use cm_core::startup::{RegValue, RegView};
use cm_core::uninstall::{AdapterChoice, AdapterKind, KeyTree, RecordSource, RegLocation, Rescan, RunningProcess, UninstallRecord};

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
    key: RefCell<Option<KeyTree>>,
    ran: Cell<u32>,
    ended: Cell<u32>,
    deleted: Cell<u32>,
}

impl Fake {
    fn new(running: RunningState) -> Fake {
        Fake {
            running, closes: false, confirm: false, exit: Some(0), rescan: Rescan { install_dir_present: Some(false), registration_present: Some(false) },
            key: RefCell::new(None), ran: Cell::new(0), ended: Cell::new(0), deleted: Cell::new(0),
        }
    }
}

impl UninstallActions for Fake {
    fn is_admin(&self) -> bool {
        false
    }
    fn running(&self, _: &AppIdentity) -> RunningState {
        self.running.clone()
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
    fn run(&self, _: &AdapterChoice, _: u32) -> Result<(Option<i32>, u64), String> {
        self.ran.set(self.ran.get() + 1);
        Ok((self.exit, 42))
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
    UninstallRecord {
        instance_id: "app_foo".into(), name: "Foo".into(), version: None, publisher: Some("Contoso".into()), source: RecordSource::RegistryUninstall, key: None,
        key_name: Some("Foo".into()), product_code: None, package_full_name: None, package_family: None, uninstall_string: Some(r#""C:\Foo\u.exe""#.into()),
        quiet_uninstall_string: None, install_location: Some(r"C:\Foo".into()), system_component: false, winget: None, package_name: None, ai_tool: None, protected_class: None,
    }
}

fn stage_one_plan(force: QuitForce, plan_id: &str) -> PlanV2 {
    let rec = record();
    let choice = AdapterChoice {
        kind: AdapterKind::RegisteredCommand, program: r"C:\Foo\u.exe".into(), argv: vec![], raw_args: true, publisher_required: Some("Contoso".into()), microsoft_signed: false,
        display: "u.exe".into(), keeps_own_ui: true,
    };
    let m = UninstallMeta::App { identity: AppIdentity::of(&rec), record: Box::new(rec.clone()), adapter: choice.clone(), signer: Some("Contoso".into()) };
    let items = vec![
        PlanAction::AppQuit(AppQuitAction { action_id: "q0000000".into(), app_instance_id: "app_foo".into(), app_identity: r"C:\Foo".into(), force }),
        PlanAction::ToolAdapter(ToolAdapterAction {
            action_id: "u0000001".into(), adapter_id: choice.kind.adapter_id().into(), adapter_version: ADAPTER_VERSION.into(),
            executable: ExecutableRef { path: RawPath::windows(r"C:\Foo\u.exe".encode_utf16().collect()).unwrap(), sha256: "a".repeat(64) }, argv: vec![],
            preview_digest: adapter_digest(&rec, &choice), timeout_seconds: 60,
        }),
    ];
    let meta = vec![ActionMeta { uninstall: Some(m.clone()), ..Default::default() }, ActionMeta { uninstall: Some(m), ..Default::default() }];
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
    let (_, identity, report) = stage_one(&stores, &r.operation_id).unwrap();
    assert_eq!((identity.instance_id.as_str(), report.is_some()), ("app_foo", true));
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
