use std::cell::{Cell, RefCell};

use super::*;
use cm_core::authz::{KindReceipt, ReceiptSource};
use cm_core::host::HostRef;
use cm_core::maintenance::{Execution, Probe, build_maintenance_plan, ids, resolve_tasks};
use cm_core::plan::v2::PlanV2;

const T0: i64 = 1_800_000_000;
const PRINCIPAL: &str = "user:alice";
const P: TaskPlatform = TaskPlatform::Windows;

fn stores(name: &str) -> Stores {
    let dir = std::env::temp_dir().join(format!("cm-app-maint-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Stores::open(&dir).unwrap()
}

/// Scripted tasks: every probe is applicable unless overridden; executions are counted (nothing touches the system).
#[derive(Default)]
struct Scripted {
    probes: Vec<(&'static str, Probe)>,
    outcome: Vec<(&'static str, TaskOutcome)>,
    executed: RefCell<Vec<String>>,
}

impl TaskBackend for Scripted {
    fn probe(&self, t: &TaskSpec) -> Probe {
        self.probes.iter().find(|(id, _)| *id == t.id).map(|(_, p)| p.clone()).unwrap_or(Probe::Applicable { detail: String::new() })
    }
    fn execute(&self, t: &TaskSpec) -> Execution {
        self.executed.borrow_mut().push(t.id.into());
        let o = self.outcome.iter().find(|(id, _)| *id == t.id).map(|(_, o)| *o).unwrap_or(TaskOutcome::Applied);
        Execution::new(o, "scripted", "scripted result")
    }
}

/// Records how often elevation was requested and with what grant.
struct FakeElevation<'a> {
    decline: bool,
    starts: Cell<u32>,
    granted: RefCell<Vec<String>>,
    tasks: &'a Scripted,
}

struct FakeSession<'a>(&'a Scripted, &'a RefCell<Vec<String>>);

impl ElevatedSession for FakeSession<'_> {
    fn run(&mut self, task: &TaskSpec) -> Result<TaskRun, String> {
        assert!(self.1.borrow().iter().any(|g| g == task.id), "only granted tasks reach the worker");
        Ok(run_task(self.0, task, true, &|| T0))
    }
    fn close(self: Box<Self>) {}
}

impl Elevation for FakeElevation<'_> {
    fn start(&self, grant: &WorkerGrant) -> Result<Box<dyn ElevatedSession + '_>, ElevationRefused> {
        self.starts.set(self.starts.get() + 1);
        grant.validate(P).unwrap();
        *self.granted.borrow_mut() = grant.tasks.iter().map(|t| t.task_id.clone()).collect();
        if self.decline {
            return Err(ElevationRefused::Declined("UAC prompt cancelled".into()));
        }
        Ok(Box::new(FakeSession(self.tasks, &self.granted)))
    }
}

fn leak<T>(v: T) -> &'static T {
    Box::leak(Box::new(v))
}

fn host() -> HostRef {
    HostRef { kind: "cli".into(), pid: 1, started_at: "2026-10-15T00:00:00Z".into() }
}

fn plan(s: &Stores, plan_id: &str, task_ids: &[&str]) -> PlanV2 {
    let tasks = resolve_tasks(task_ids, P).unwrap();
    let plan = build_maintenance_plan(&tasks, P, plan_id, T0, "2".repeat(64).as_str()).unwrap();
    s.audit.save_plan_v2(&plan).unwrap();
    plan
}

fn approve(s: &Stores, plan: &PlanV2, source: ReceiptSource) {
    s.audit
        .save_kind_receipt(&KindReceipt {
            receipt_id: format!("r_{}_{source:?}", plan.body.plan_id),
            plan_id: plan.body.plan_id.clone(),
            plan_digest: plan.digest.clone(),
            plan_kind: plan.body.plan_kind,
            principal: PRINCIPAL.into(),
            source,
            created_at: plan.body.created_at.clone(),
            expires_at: plan.body.expires_at.clone(),
            revoked_at: None,
        })
        .unwrap();
}

fn apply(s: &Stores, b: &MaintenanceBackends<'_>, plan_id: &str) -> Result<MaintenanceReport, ApplyError> {
    let h = host();
    let ctx = ApplyContext { principal: PRINCIPAL, host: &h, current_rule_set_digest: "", now: &|| T0 + 10, cancel: &|| false, fault: &|_, _| false, protect: &|_| Ok(()) };
    apply_maintenance_plan(s, b, plan_id, &ctx)
}

fn outcome(r: &MaintenanceReport, id: &str) -> (TaskOutcome, String) {
    let t = r.items.iter().find(|t| t.task_id == id).unwrap();
    (t.outcome, t.code.clone())
}

const ALL: [&str; 5] = [ids::SHELL_RESTART, ids::ICON_CACHE, ids::DNS_FLUSH, ids::TRIM, ids::IMAGE_HEALTH];

/// MNT-04: administrator tasks are listed together and elevated once; a declined prompt skips exactly those, the
/// user-level tasks still run, and the run is recorded with the elevation state (MNT-07).
#[test]
fn mnt04_declined_elevation_skips_admin_tasks_and_the_rest_continue() {
    let s = stores("declined");
    let p = plan(&s, "mp_declined", &ALL);
    approve(&s, &p, ReceiptSource::InteractiveReview);
    let tasks: &'static Scripted = leak(Scripted::default());
    let elev = leak(FakeElevation { decline: true, starts: Cell::new(0), granted: RefCell::new(vec![]), tasks });
    let b = MaintenanceBackends { platform: P, tasks, elevation: elev, is_admin: false, protect_task: &|_| Ok(()) };
    let r = apply(&s, &b, "mp_declined").unwrap();
    assert_eq!(elev.starts.get(), 1, "one elevation request per run");
    assert_eq!(*elev.granted.borrow(), [ids::TRIM, ids::IMAGE_HEALTH], "the request lists every administrator task");
    assert_eq!(r.elevation, ElevationState::Declined);
    for id in [ids::TRIM, ids::IMAGE_HEALTH] {
        assert_eq!(outcome(&r, id), (TaskOutcome::Skipped, "elevation_declined".into()));
    }
    for id in [ids::SHELL_RESTART, ids::ICON_CACHE, ids::DNS_FLUSH] {
        assert_eq!(outcome(&r, id).0, TaskOutcome::Applied);
    }
    assert_eq!(*tasks.executed.borrow(), [ids::SHELL_RESTART, ids::ICON_CACHE, ids::DNS_FLUSH], "admin tasks never ran in-process");
    assert_eq!((r.counts.applied, r.counts.skipped), (3, 2));
    assert!(r.all_clear && r.undo == "unavailable" && r.entry == "cli");
    // History has the run; the generic operation report carries the outcome names and no undo.
    let h = history(&s, 10).unwrap();
    assert_eq!(h, vec![r.clone()]);
    let op: OperationReport = serde_json::from_str(&s.audit.operation_report(&r.operation_id).unwrap().unwrap()).unwrap();
    assert_eq!(op.status, "partial");
    assert!(op.items.iter().all(|i| i.recovery != "trash" && i.recovery != "quarantine"));
    assert_eq!(op.items.iter().filter(|i| i.code == "applied" && i.recovery == "unavailable").count(), 3);
    // EXE-03: applying again replays the stored run, nothing runs twice.
    let again = apply(&s, &b, "mp_declined").unwrap();
    assert_eq!(again, r);
    assert_eq!(tasks.executed.borrow().len(), 3);
}

/// MNT-04: a granted elevation runs administrator tasks only in the worker; already elevated runs them in-process
/// without asking.
#[test]
fn mnt04_granted_and_already_elevated() {
    let s = stores("granted");
    let p = plan(&s, "mp_granted", &[ids::DNS_FLUSH, ids::TRIM]);
    approve(&s, &p, ReceiptSource::InteractiveReview);
    let tasks: &'static Scripted = leak(Scripted { probes: vec![(ids::TRIM, Probe::NotNeeded { code: "not_due".into(), summary: "last optimized 3 days ago".into() })], ..Default::default() });
    let elev = leak(FakeElevation { decline: false, starts: Cell::new(0), granted: RefCell::new(vec![]), tasks });
    let b = MaintenanceBackends { platform: P, tasks, elevation: elev, is_admin: false, protect_task: &|_| Ok(()) };
    let r = apply(&s, &b, "mp_granted").unwrap();
    assert_eq!((r.elevation, elev.starts.get()), (ElevationState::Granted, 1));
    assert_eq!(outcome(&r, ids::TRIM), (TaskOutcome::Unchanged, "not_due".into()), "MNT-02: TRIM not due is unchanged");
    assert!(r.items.iter().find(|t| t.task_id == ids::TRIM).unwrap().elevated);
    assert!(!r.items.iter().find(|t| t.task_id == ids::DNS_FLUSH).unwrap().elevated);

    let s = stores("admin");
    let p = plan(&s, "mp_admin", &[ids::IMAGE_HEALTH]);
    approve(&s, &p, ReceiptSource::InteractiveReview);
    let elev2 = leak(FakeElevation { decline: false, starts: Cell::new(0), granted: RefCell::new(vec![]), tasks });
    let b = MaintenanceBackends { platform: P, tasks, elevation: elev2, is_admin: true, protect_task: &|_| Ok(()) };
    let r = apply(&s, &b, "mp_admin").unwrap();
    assert_eq!((r.elevation, elev2.starts.get()), (ElevationState::AlreadyElevated, 0));
    assert_eq!(outcome(&r, ids::IMAGE_HEALTH).0, TaskOutcome::Applied);
}

/// PRT-06: a user-protected task is skipped as protected and is never selected by a batch plan; others run.
/// MNT-02: an inapplicable task is skipped; MNT-03: attention/failed keep the run from being all clear.
#[test]
fn prt06_protected_task_skipped_and_mnt03_attention_is_not_all_clear() {
    let s = stores("protected");
    let tasks: &'static Scripted = leak(Scripted {
        probes: vec![(ids::SHELL_RESTART, Probe::NotApplicable { code: "not_running".into(), reason: "Start is not running".into() })],
        outcome: vec![(ids::DNS_FLUSH, TaskOutcome::Attention)],
        ..Default::default()
    });
    let elev = leak(FakeElevation { decline: false, starts: Cell::new(0), granted: RefCell::new(vec![]), tasks });
    let protect = |id: &str| if id == ids::ICON_CACHE { Err("user entry u_1".to_string()) } else { Ok(()) };
    let b = MaintenanceBackends { platform: P, tasks, elevation: elev, is_admin: false, protect_task: &protect };
    let batch: Vec<&str> = default_selection(&b).iter().map(|t| t.id).collect();
    assert_eq!(batch, [ids::DNS_FLUSH, ids::TRIM, ids::IMAGE_HEALTH], "protected and inapplicable tasks are not batch-selected");
    let view = catalog_view(&b);
    let icon = view.iter().find(|v| v["task_id"] == ids::ICON_CACHE).unwrap();
    assert_eq!((icon["user_protected"].as_bool(), icon["applicability"].as_str()), (Some(true), Some("not_applicable")));
    assert!(view.iter().all(|v| v["reversible"] == false));

    let p = plan(&s, "mp_protected", &[ids::SHELL_RESTART, ids::ICON_CACHE, ids::DNS_FLUSH]);
    approve(&s, &p, ReceiptSource::InteractiveReview);
    let r = apply(&s, &b, "mp_protected").unwrap();
    assert_eq!(outcome(&r, ids::ICON_CACHE), (TaskOutcome::Skipped, "protected".into()));
    assert_eq!(outcome(&r, ids::SHELL_RESTART), (TaskOutcome::Skipped, "not_running".into()));
    assert_eq!(outcome(&r, ids::DNS_FLUSH).0, TaskOutcome::Attention);
    assert_eq!(*tasks.executed.borrow(), [ids::DNS_FLUSH], "only the applicable, unprotected task ran");
    assert_eq!((r.elevation, elev.starts.get()), (ElevationState::NotNeeded, 0));
    assert!(!r.all_clear, "attention is never all clear");
}

/// Only an interactive review authorizes a maintenance plan; a policy-sourced receipt never does (D1).
#[test]
fn maintenance_plans_need_an_interactive_review() {
    let s = stores("review");
    let p = plan(&s, "mp_review", &[ids::DNS_FLUSH]);
    let tasks: &'static Scripted = leak(Scripted::default());
    let elev = leak(FakeElevation { decline: false, starts: Cell::new(0), granted: RefCell::new(vec![]), tasks });
    let b = MaintenanceBackends { platform: P, tasks, elevation: elev, is_admin: false, protect_task: &|_| Ok(()) };
    assert!(matches!(apply(&s, &b, "mp_review"), Err(ApplyError::NeedsApproval(_))));
    approve(&s, &p, ReceiptSource::Policy);
    assert!(matches!(apply(&s, &b, "mp_review"), Err(ApplyError::NeedsApproval(_))));
    assert!(tasks.executed.borrow().is_empty());
    approve(&s, &p, ReceiptSource::InteractiveReview);
    assert_eq!(apply(&s, &b, "mp_review").unwrap().counts.applied, 1);
}

/// A stored action naming a task version this build does not have is skipped, not run (and other kinds are refused).
#[test]
fn other_catalog_versions_are_skipped() {
    let s = stores("version");
    let tasks = resolve_tasks(&[ids::DNS_FLUSH], P).unwrap();
    let mut p = build_maintenance_plan(&tasks, P, "mp_version", T0, "2".repeat(64).as_str()).unwrap();
    if let PlanAction::MaintenanceTask(m) = &mut p.body.items[0] {
        m.task_version = "0.9.0".into();
    }
    p.digest = p.body.digest().unwrap();
    s.audit.save_plan_v2(&p).unwrap();
    approve(&s, &p, ReceiptSource::InteractiveReview);
    let backend: &'static Scripted = leak(Scripted::default());
    let elev = leak(FakeElevation { decline: false, starts: Cell::new(0), granted: RefCell::new(vec![]), tasks: backend });
    let b = MaintenanceBackends { platform: P, tasks: backend, elevation: elev, is_admin: false, protect_task: &|_| Ok(()) };
    let r = apply(&s, &b, "mp_version").unwrap();
    assert_eq!(outcome(&r, ids::DNS_FLUSH), (TaskOutcome::Skipped, "task_version_changed".into()));
    assert!(backend.executed.borrow().is_empty());
}
