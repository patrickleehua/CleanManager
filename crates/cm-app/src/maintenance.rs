//! `maintenance` plans (parity 14.1, MNT-02/03/04/07, PRT-06): the catalog view, the run of a reviewed plan, history.
//!
//! A run starts like every `plan/v2` operation (dedupe, kind rules and digest re-checked, interactive-review receipt
//! only, journaled intent → result) and then:
//! 1. checks each planned task against this build's catalog (other ID/version/admin flag → skipped, never run) and the
//!    user protection list (PRT-06 → skipped `protected`);
//! 2. lists the remaining administrator tasks and asks for elevation **once**, before any task runs (MNT-04): already
//!    elevated → run in-process; otherwise one short-lived worker via [`Elevation`]. Declined or unavailable → those tasks
//!    are skipped and the user-level tasks still run;
//! 3. probes and runs each task (`cm_core::maintenance::run_task`), records the six-outcome result per task, and stores
//!    a `maintenance_report` (MNT-07; no generic undo) next to the ordinary operation report.

use cm_core::maintenance::{
    ElevationState, MaintenanceReport, OutcomeCounts, TaskBackend, TaskOutcome, TaskPlatform, TaskRef, TaskRun, TaskSpec, WorkerGrant, planned_task, run_task,
};
use cm_core::plan::parse_utc;
use cm_core::plan::v2::{PlanAction, PlanKind, PlanProblem, check_plan_v2};
use cm_core::time::utc_from_unix;
use cm_store::{StoreError, Stores};

use crate::execute::{ActionResult, ActionStatus, ApplyContext, ApplyError, OperationReport, summarize};

/// Why no elevated worker is available for this run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElevationRefused {
    /// The user declined the UAC prompt.
    Declined(String),
    /// The worker could not be started, connected or verified.
    Unavailable(String),
}

/// One running elevated worker: runs granted tasks, then is closed (it also exits on its own when idle).
pub trait ElevatedSession {
    fn run(&mut self, task: &TaskSpec) -> Result<TaskRun, String>;
    fn close(self: Box<Self>);
}

/// Starts at most one worker per run for exactly the granted administrator tasks of one plan digest.
pub trait Elevation {
    fn start(&self, grant: &WorkerGrant) -> Result<Box<dyn ElevatedSession + '_>, ElevationRefused>;
}

pub struct MaintenanceBackends<'a> {
    pub platform: TaskPlatform,
    /// Fixed task implementations run in this process (user-level tasks, or all of them when already elevated).
    pub tasks: &'a dyn TaskBackend,
    pub elevation: &'a dyn Elevation,
    /// This process has an elevated token.
    pub is_admin: bool,
    /// User protection of a task id (PRT-06); `Err(reason)` skips it as `protected`. Fails closed.
    pub protect_task: &'a dyn Fn(&str) -> Result<(), String>,
}

/// Protocol `maintenance_task_info` rows: catalog, probe result and protection (read-only).
pub fn catalog_view(b: &MaintenanceBackends<'_>) -> Vec<serde_json::Value> {
    cm_core::maintenance::catalog(b.platform)
        .iter()
        .map(|t| {
            let protected = (b.protect_task)(t.id).is_err();
            let (applicability, reason) = if protected { ("not_applicable", Some("protected by the user (PRT-06); skipped in every run".to_owned())) } else { b.tasks.probe(t).applicability() };
            serde_json::json!({
                "task_id": t.id, "task_version": t.version, "platform": t.platform, "title": t.title, "problem": t.problem, "impact": t.impact,
                "requires_admin": t.requires_admin, "applies_when": t.applies_when, "estimated_seconds": t.estimated_seconds,
                "applicability": applicability, "applicability_reason": reason, "user_protected": protected, "reversible": false,
            })
        })
        .collect()
}

/// Tasks a batch plan selects when none are named: everything not protected and not known to be inapplicable.
pub fn default_selection(b: &MaintenanceBackends<'_>) -> Vec<&'static TaskSpec> {
    cm_core::maintenance::catalog(b.platform)
        .iter()
        .filter(|t| (b.protect_task)(t.id).is_ok())
        .filter(|t| matches!(b.tasks.probe(t), cm_core::maintenance::Probe::Applicable { .. } | cm_core::maintenance::Probe::Unknown { .. }))
        .collect()
}

fn action_result(action_id: &str, r: &TaskRun) -> ActionResult {
    let status = match r.outcome {
        TaskOutcome::Applied | TaskOutcome::Unchanged | TaskOutcome::Attention => ActionStatus::Succeeded,
        TaskOutcome::Skipped | TaskOutcome::Unavailable => ActionStatus::Skipped,
        TaskOutcome::Failed => ActionStatus::Failed,
    };
    // MNT-07: a maintenance action is never undone by this tool.
    let recovery = if r.outcome == TaskOutcome::Applied { "unavailable" } else { "not_applicable" };
    ActionResult { action_id: action_id.into(), status, code: r.outcome.as_str().into(), message: format!("{}: {}", r.code, r.reason), recovery: recovery.into() }
}

/// Apply a stored, reviewed `maintenance` plan. Errors are returned before anything ran.
pub fn apply_maintenance_plan(stores: &Stores, b: &MaintenanceBackends<'_>, plan_id: &str, ctx: &ApplyContext<'_>) -> Result<MaintenanceReport, ApplyError> {
    let plan = stores.audit.load_plan_v2(plan_id)?;
    if plan.body.plan_kind != PlanKind::Maintenance {
        return Err(ApplyError::PolicyDenied(format!("plan {plan_id} is a {} plan, not a maintenance plan", plan.body.plan_kind.as_str())));
    }
    let dedupe_key = format!("{}#{}", plan.body.plan_id, plan.digest);
    // EXE-03: the same plan never produces a second operation.
    if let Some((op, _)) = stores.audit.operation_for_dedupe(&dedupe_key)? {
        return match stored_run(stores, &op)? {
            Some(r) => Ok(r),
            None => Err(ApplyError::Conflict(op)),
        };
    }
    match check_plan_v2(&plan, (ctx.now)()) {
        Ok(()) => {}
        Err(PlanProblem::Stale(r)) => return Err(ApplyError::Stale(r)),
        Err(PlanProblem::Kind(k)) => return Err(ApplyError::PolicyDenied(format!("plan violates its kind: {k}"))),
    }
    let authorized = |now: i64| -> Result<bool, StoreError> { Ok(crate::plans::authorization(stores, &plan, ctx.principal, now)?.0 == "authorized") };
    if !authorized((ctx.now)())? {
        return Err(ApplyError::NeedsApproval(plan_id.into()));
    }
    let operation_id = format!("op_{}", &plan.digest[..16]);
    // One maintenance run at a time on this store.
    if let Some(holder) = stores.audit.lock_identities(&operation_id, &["maintenance|run".to_owned()])? {
        return Err(ApplyError::Conflict(holder));
    }
    let started_at = utc_from_unix((ctx.now)());
    stores.audit.begin_operation(&operation_id, plan_id, &plan.digest, &dedupe_key, ctx.host, &started_at)?;
    stores.audit.set_operation_state(&operation_id, "running")?;
    let at = || utc_from_unix((ctx.now)());

    // 1. What each action is allowed to run.
    let mut decided: Vec<Result<&'static TaskSpec, TaskRun>> = Vec::with_capacity(plan.body.items.len());
    for a in &plan.body.items {
        let PlanAction::MaintenanceTask(m) = a else {
            decided.push(Err(TaskRun::not_run(a.action_id(), "-", a.needs_admin(), TaskOutcome::Skipped, "unsupported", format!("{} actions are not executed by this build", a.action_type().as_str()), &at())));
            continue;
        };
        let d = match planned_task(m, b.platform) {
            Err((code, why)) => Err(TaskRun::not_run(&m.task_id, &m.task_version, m.requires_admin, TaskOutcome::Skipped, code, why, &at())),
            Ok(spec) => match (b.protect_task)(spec.id) {
                Err(why) => Err(TaskRun::not_run(spec.id, spec.version, spec.requires_admin, TaskOutcome::Skipped, "protected", format!("protected by the user (PRT-06): {why}"), &at())),
                Ok(()) => Ok(spec),
            },
        };
        decided.push(d);
    }

    // 2. One elevation for all administrator tasks, before any task runs (MNT-04).
    let admin: Vec<&TaskSpec> = decided.iter().filter_map(|d| d.as_ref().ok().copied()).filter(|t| t.requires_admin).collect();
    let mut refused: Option<(TaskOutcome, &'static str, String)> = None;
    let mut session: Option<Box<dyn ElevatedSession + '_>> = None;
    let elevation = if admin.is_empty() {
        ElevationState::NotNeeded
    } else if b.is_admin {
        ElevationState::AlreadyElevated
    } else {
        let grant = WorkerGrant { plan_digest: plan.digest.clone(), tasks: admin.iter().map(|t| TaskRef { task_id: t.id.into(), task_version: t.version.into() }).collect() };
        match b.elevation.start(&grant) {
            Ok(s) => {
                session = Some(s);
                ElevationState::Granted
            }
            Err(ElevationRefused::Declined(why)) => {
                refused = Some((TaskOutcome::Skipped, "elevation_declined", format!("administrator authorization was declined ({why}); not run")));
                ElevationState::Declined
            }
            Err(ElevationRefused::Unavailable(why)) => {
                refused = Some((TaskOutcome::Skipped, "elevation_unavailable", format!("no verified elevated worker: {why}; not run")));
                ElevationState::Unavailable
            }
        }
    };

    // 3. Run in plan order, journaling each action.
    let expires = parse_utc(&plan.body.expires_at).unwrap_or(i64::MIN);
    let mut runs: Vec<TaskRun> = Vec::with_capacity(plan.body.items.len());
    let mut results: Vec<ActionResult> = Vec::with_capacity(plan.body.items.len());
    let mut cancelled = false;
    let mut crash: Option<ApplyError> = None;
    for (a, d) in plan.body.items.iter().zip(decided) {
        let id = a.action_id();
        // EXE-06: cancellation, expiry and revocation stop new tasks only.
        let stop = if cancelled || (ctx.cancel)() {
            cancelled = true;
            Some(("cancelled", "operation cancelled before this task started"))
        } else if (ctx.now)() >= expires {
            Some(("plan_expired", "plan expired before this task started"))
        } else if !authorized((ctx.now)())? {
            Some(("authorization_revoked", "authorization was revoked or expired"))
        } else {
            None
        };
        let run = match (stop, d) {
            (_, Err(r)) => r,
            (Some((code, why)), Ok(spec)) => TaskRun::not_run(spec.id, spec.version, spec.requires_admin, TaskOutcome::Skipped, code, why, &at()),
            (None, Ok(spec)) if spec.requires_admin && !b.is_admin && session.is_none() => {
                let (o, code, why) = refused.clone().expect("no session means elevation was refused");
                TaskRun::not_run(spec.id, spec.version, true, o, code, why, &at())
            }
            (None, Ok(spec)) => {
                let intent = serde_json::json!({"action_type": "maintenance_task", "task_id": spec.id, "task_version": spec.version, "elevated": spec.requires_admin});
                stores.audit.record_intent(&operation_id, id, &intent.to_string(), &at())?;
                if (ctx.fault)(crate::execute::FaultPoint::AfterIntent, id) {
                    crash = Some(ApplyError::Crashed("after_intent".into()));
                    break;
                }
                let r = match (&mut session, spec.requires_admin && !b.is_admin) {
                    (Some(s), true) => s.run(spec).unwrap_or_else(|e| TaskRun::not_run(spec.id, spec.version, true, TaskOutcome::Failed, "worker_error", e, &at())),
                    _ => run_task(b.tasks, spec, b.is_admin, ctx.now),
                };
                if (ctx.fault)(crate::execute::FaultPoint::AfterAction, id) {
                    crash = Some(ApplyError::Crashed("after_action".into()));
                    break;
                }
                r
            }
        };
        let ar = action_result(id, &run);
        let status = serde_json::to_value(ar.status).expect("status serializes");
        stores.audit.record_result(&operation_id, id, status.as_str().expect("string"), &serde_json::to_string(&ar).expect("result serializes"), &at())?;
        results.push(ar);
        runs.push(run);
    }
    if let Some(s) = session {
        s.close();
    }
    if let Some(e) = crash {
        return Err(e);
    }

    let counts = OutcomeCounts::of(&runs);
    let finished_at = at();
    let report = MaintenanceReport {
        operation_id: operation_id.clone(),
        plan_id: plan.body.plan_id.clone(),
        plan_digest: plan.digest.clone(),
        entry: ctx.host.kind.clone(),
        principal: ctx.principal.into(),
        elevation,
        started_at: started_at.clone(),
        finished_at: finished_at.clone(),
        counts,
        all_clear: counts.all_clear(),
        items: runs,
        undo: "unavailable".into(),
    };
    let (ok, skipped, failed, not_started, unknown) = summarize(&results);
    // MNT-03: findings that need the user keep the operation from reading as complete.
    let status = if cancelled {
        "cancelled"
    } else if failed > 0 && ok == 0 {
        "failed"
    } else if failed > 0 || skipped > 0 || counts.attention > 0 {
        "partial"
    } else {
        "completed"
    };
    let op = OperationReport {
        operation_id: operation_id.clone(),
        plan_id: plan.body.plan_id.clone(),
        plan_digest: plan.digest.clone(),
        status: status.into(),
        started_at,
        finished_at: finished_at.clone(),
        succeeded: ok,
        skipped,
        failed,
        not_started,
        unknown,
        items: results,
        processed_logical_bytes: "0".into(),
        quarantined_logical_bytes: "0".into(),
        estimated_freed_bytes: "0".into(),
        volume_free_before_bytes: None,
        volume_free_after_bytes: None,
        observed_free_delta_bytes: None,
        authorization_source: "interactive_review".into(),
    };
    stores.audit.save_maintenance_run(&operation_id, plan_id, &finished_at, &serde_json::to_string(&report).expect("report serializes"))?;
    stores.audit.release_locks(&operation_id)?;
    stores.audit.finish_operation(&operation_id, &op.status, &serde_json::to_string(&op).expect("report serializes"), &finished_at)?;
    Ok(report)
}

fn stored_run(stores: &Stores, operation_id: &str) -> Result<Option<MaintenanceReport>, StoreError> {
    Ok(history(stores, 500)?.into_iter().find(|r| r.operation_id == operation_id))
}

/// Recorded maintenance runs, newest first (MNT-07).
pub fn history(stores: &Stores, limit: u32) -> Result<Vec<MaintenanceReport>, StoreError> {
    stores.audit.maintenance_runs(limit)?.iter().map(|j| serde_json::from_str(j).map_err(|e| StoreError::AuditUnavailable(e.to_string()))).collect()
}

/// An [`Elevation`] that never elevates (platforms or entries without a worker): administrator tasks are skipped.
pub struct NoElevation(pub &'static str);

impl Elevation for NoElevation {
    fn start(&self, _: &WorkerGrant) -> Result<Box<dyn ElevatedSession + '_>, ElevationRefused> {
        Err(ElevationRefused::Unavailable(self.0.into()))
    }
}

#[cfg(test)]
mod tests;
