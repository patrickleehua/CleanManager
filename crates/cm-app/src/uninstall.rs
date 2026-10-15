//! Uninstall plans (parity UNI-01..10, design D7) on the shared `plan/v2` executor.
//!
//! - [`create_uninstall_plan`] (stage 1) freezes `app_quit` + one `tool_adapter` for an instance after the platform
//!   verified the program's path, hash and signer; refused sources come back with a capability explanation and the
//!   Settings entry. The leftovers it would probably leave are reported as an estimate only, never authorized.
//! - [`run_action`] executes the uninstall-specific actions inside `plans::apply_plan_v2`: the close request (never an
//!   automatic forced end), the verified uninstaller run with exit code, duration and rescan (UNI-03), and leftover
//!   registry deletes after their backup is stored with the operation (UNI-06). File leftovers are `fs_trash` and
//!   startup leftovers `startup_toggle remove_with_backup`, run by the shared code paths.
//! - [`create_leftovers_plan`] (stage 2) is built from a finished stage-1 operation and reviewed separately: only
//!   default-selected objects (caches and logs) plus objects named explicitly by id; kept, blocked and manual objects
//!   are never planned. [`create_restore_plan`] puts deleted keys and removed startup values back where nothing was
//!   recreated since.
//! - UNI-11: one stage-1 plan may hold several applications (`create_uninstall_batch`). Each keeps its own close
//!   request and uninstaller (its uninstaller waits only for its own close request), its own outcome
//!   ([`app_outcomes`]) and its own leftover review; one that fails, is skipped or went stale stops nothing else, and
//!   its leftovers are never reviewed or removed. Protected classes are refused per application, as for one.

use cm_core::plan::v2::{
    ActionMeta, AppQuitAction, ExecutableRef, HeaderInput, NO_RULES_DIGEST, PlanAction, PlanKind, PlanV2, QuitForce, RegistryBackupDeleteAction, RegistryRestoreAction,
    ToolAdapterAction,
};
use cm_core::time::utc_from_unix;
use cm_core::uninstall::{
    ADAPTER_VERSION, AppIdentity, KEY_BACKUP_PREFIX, KeyBackup, RunningState, StepStatus, UNINSTALLER_TIMEOUT_SECONDS, UninstallActions, UninstallMeta, adapter_digest,
    end_refusal, key_digest, uninstaller_outcome,
};
use cm_store::Stores;

use crate::execute::{ActionResult, ActionStatus, ApplyContext, OperationReport};

fn res(action_id: &str, status: ActionStatus, code: &str, message: impl Into<String>) -> ActionResult {
    ActionResult { action_id: action_id.into(), status, code: code.into(), message: message.into(), recovery: "not_applicable".into() }
}

/// Uninstall-specific actions of `uninstall` plans and the key restores of their `recovery` plans.
pub fn handles(kind: PlanKind, a: &PlanAction) -> bool {
    match (kind, a) {
        (PlanKind::Uninstall, PlanAction::AppQuit(_) | PlanAction::ToolAdapter(_) | PlanAction::RegistryBackupDelete(_)) => true,
        (PlanKind::Recovery, PlanAction::RegistryRestore(r)) => r.backup_id.starts_with(KEY_BACKUP_PREFIX),
        _ => false,
    }
}

/// Operation status of an uninstall plan: never `completed` while the rescan found something left (UNI-03/09).
pub fn overall_status(plan: &PlanV2, results: &[ActionResult], base: &'static str) -> &'static str {
    if plan.body.plan_kind != PlanKind::Uninstall {
        return base;
    }
    let codes: Vec<&str> = results.iter().map(|r| r.code.as_str()).collect();
    cm_core::uninstall::overall_status(base, &codes)
}

/// Where an executing action came from (plan, operation and the results so far).
pub struct Step<'a> {
    pub stores: &'a Stores,
    pub plan: &'a PlanV2,
    pub index: usize,
    pub prior: &'a [ActionResult],
    pub operation_id: &'a str,
    pub ctx: &'a ApplyContext<'a>,
}

/// Execute one uninstall-specific action ([`handles`]). Never panics on bad data: it reports a skip instead.
pub fn run_action(ua: Option<&dyn UninstallActions>, s: &Step<'_>) -> ActionResult {
    let a = &s.plan.body.items[s.index];
    let id = a.action_id();
    let Some(ua) = ua else { return res(id, ActionStatus::Skipped, "unsupported", "application uninstall is not available on this platform") };
    let meta = s.plan.meta[s.index].uninstall.as_ref();
    match (a, meta) {
        (PlanAction::AppQuit(q), Some(UninstallMeta::App { identity, .. })) => quit(ua, id, q, identity),
        (PlanAction::ToolAdapter(t), Some(UninstallMeta::App { identity, record, adapter, signer })) => {
            // UNI-02: nothing is uninstalled unless the app quit first. UNI-11: only this app's own close request counts;
            // another application of the same batch that did not quit does not stop this one.
            let quit_ok = s.plan.body.items[..s.index]
                .iter()
                .zip(s.prior)
                .filter(|(p, _)| matches!(p, PlanAction::AppQuit(q) if q.app_instance_id == identity.instance_id))
                .all(|(_, r)| r.status == ActionStatus::Succeeded);
            if !quit_ok {
                return res(id, ActionStatus::Skipped, "app_still_running", "the application did not quit (or its state is unknown); nothing was uninstalled or deleted");
            }
            if let Err((code, message)) = ua.verify(record, adapter, &t.executable.sha256, signer.as_deref()) {
                return res(id, ActionStatus::Skipped, &code, message);
            }
            let (exit, ms) = match ua.run(adapter, t.timeout_seconds) {
                Ok(r) => r,
                Err(e) => return res(id, ActionStatus::Failed, "start_failed", format!("{e}; nothing was run")),
            };
            let rescan = ua.rescan(identity);
            let (status, code, message) = uninstaller_outcome(adapter.kind, exit, ms, rescan);
            let message = message.replace("<this operation>", s.operation_id);
            let status = match status {
                StepStatus::Succeeded => ActionStatus::Succeeded,
                StepStatus::Skipped => ActionStatus::Skipped,
                StepStatus::Failed => ActionStatus::Failed,
                StepStatus::Unknown => ActionStatus::Unknown,
            };
            res(id, status, code, message)
        }
        (PlanAction::RegistryBackupDelete(r), Some(UninstallMeta::RegistryKey { location })) => {
            if location.hive == cm_core::plan::v2::Hive::Hklm && !ua.is_admin() {
                return res(id, ActionStatus::Skipped, "permission_required", cm_core::startup::ADMIN_REQUIRED);
            }
            let tree = match ua.read_key(location) {
                Ok(Some(t)) => t,
                Ok(None) => return res(id, ActionStatus::Skipped, "precondition_missing", format!("{} no longer exists", location.display())),
                Err(e) => return res(id, ActionStatus::Failed, "io_error", e),
            };
            if Some(key_digest(location, &tree)) != r.observed_sha256 {
                return res(id, ActionStatus::Skipped, "precondition_changed", format!("{} changed since planning; nothing was deleted", location.display()));
            }
            // UNI-06: the backup is stored with the operation first; if that fails, the key stays.
            let backup = KeyBackup {
                backup_id: format!("{KEY_BACKUP_PREFIX}{}_{id}", &s.operation_id[3..]), location: location.clone(), tree, owner_evidence: r.owner_evidence.clone(),
                created_at: utc_from_unix((s.ctx.now)()),
            };
            if let Err(e) = s.stores.audit.save_key_backup(s.operation_id, id, &s.plan.body.plan_id, &backup) {
                return res(id, ActionStatus::Skipped, "backup_failed", format!("the backup of {} could not be stored ({e}); the key was not deleted", location.display()));
            }
            match ua.delete_key(location) {
                Ok(()) => res(id, ActionStatus::Succeeded, "done", format!("deleted after its backup was stored (cleanmanager apps uninstall restore --operation {})", s.operation_id)),
                Err((code, message)) => {
                    let _ = s.stores.audit.set_startup_backup_state(&backup.backup_id, "unused");
                    res(id, if code.starts_with("precondition") || code == "policy_denied" { ActionStatus::Skipped } else { ActionStatus::Failed }, &code, message)
                }
            }
        }
        (PlanAction::RegistryRestore(r), Some(UninstallMeta::RegistryKey { location })) => {
            let backup = match s.stores.audit.key_backup(&r.backup_id) {
                Ok((b, _)) if r.backup_sha256.as_deref() == Some(b.sha256().as_str()) && &b.location == location => b,
                Ok(_) => return res(id, ActionStatus::Skipped, "backup_changed", "the stored backup differs from the planned one"),
                Err(e) => return res(id, ActionStatus::Failed, "backup_unavailable", e.to_string()),
            };
            if location.hive == cm_core::plan::v2::Hive::Hklm && !ua.is_admin() {
                return res(id, ActionStatus::Skipped, "permission_required", cm_core::startup::ADMIN_REQUIRED);
            }
            match ua.restore_key(&backup) {
                Ok(()) => {
                    let _ = s.stores.audit.set_startup_backup_state(&r.backup_id, "restored");
                    res(id, ActionStatus::Succeeded, "done", format!("{} recreated from its backup", location.display()))
                }
                Err((code, message)) => res(id, if code == "io_error" { ActionStatus::Failed } else { ActionStatus::Skipped }, &code, message),
            }
        }
        _ => res(id, ActionStatus::Skipped, "unsupported", "execution data does not match this action"),
    }
}

/// UNI-02: close request with a bounded wait; a forced end only in a plan reviewed for it and after a person confirmed it.
fn quit(ua: &dyn UninstallActions, id: &str, q: &AppQuitAction, identity: &AppIdentity) -> ActionResult {
    let processes = match ua.running(identity) {
        RunningState::Unknown { reason } => return res(id, ActionStatus::Skipped, "running_state_unknown", format!("{reason}; the uninstall is skipped")),
        RunningState::Known { processes } => processes,
    };
    if processes.is_empty() {
        return res(id, ActionStatus::Succeeded, "not_running", format!("{} is not running", identity.name));
    }
    let still = ua.request_quit(&processes);
    if still.is_empty() {
        return res(id, ActionStatus::Succeeded, "closed", format!("{} closed after a close request", identity.name));
    }
    let names: Vec<String> = still.iter().map(|p| p.describe()).collect();
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let protected: Vec<String> = still.iter().filter_map(|p| end_refusal(p, std::process::id(), &root).map(|why| format!("{} ({why})", p.describe()))).collect();
    if q.force == QuitForce::Never || !protected.is_empty() {
        let hint = if q.force == QuitForce::Never {
            format!("close it yourself and apply again, or create a separately reviewed plan that may end it: cleanmanager apps uninstall plan create --instance {} --end-running", q.app_instance_id)
        } else {
            format!("these are never ended: {}", protected.join(", "))
        };
        return res(id, ActionStatus::Failed, "still_running", format!("still running after the close request: {}; nothing was uninstalled. {hint}", names.join(", ")));
    }
    if !ua.confirm_end(&still) {
        return res(id, ActionStatus::Failed, "still_running", format!("still running: {}; ending them was not confirmed, so nothing was ended, uninstalled or deleted", names.join(", ")));
    }
    for p in &still {
        if let Err(e) = ua.end_process(p) {
            return res(id, ActionStatus::Failed, "still_running", format!("{e}; nothing was uninstalled"));
        }
    }
    res(id, ActionStatus::Succeeded, "ended_on_request", format!("ended after confirmation: {}", names.join(", ")))
}

/// A refusal with its protocol error code (`unsupported`, `policy_denied`, …) and the Settings alternative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub code: String,
    pub message: String,
    pub settings_uri: Option<String>,
}

impl Refused {
    fn new(code: &str, message: impl Into<String>) -> Refused {
        Refused { code: code.into(), message: message.into(), settings_uri: None }
    }

    pub fn error_code(&self) -> cm_core::error::ErrorCode {
        use cm_core::error::ErrorCode;
        match self.code.as_str() {
            "policy_denied" => ErrorCode::PolicyDenied,
            "unsupported" => ErrorCode::Unsupported,
            "permission_required" => ErrorCode::PermissionRequired,
            "conflict" => ErrorCode::Conflict,
            "io_error" => ErrorCode::IoError,
            "internal_error" => ErrorCode::InternalError,
            _ => ErrorCode::InvalidArgument,
        }
    }
}

impl From<cm_store::StoreError> for Refused {
    fn from(e: cm_store::StoreError) -> Refused {
        Refused::new("io_error", e.to_string())
    }
}

/// Most applications in one uninstall plan (UNI-11).
pub const MAX_BATCH: usize = 20;

/// Action id of the `n`-th application's close request or uninstaller in a stage-1 plan (UNI-11). The first keeps the
/// single-uninstall ids (`q0000000`, `u0000001`); the n-th is `u{n:03}0000` / `u{n:03}0001`, so ids sort in plan order
/// (plan/v2 actions are ordered by `action_id`).
pub fn batch_action_id(n: usize, uninstaller: bool) -> String {
    match (n, uninstaller) {
        (0, false) => "q0000000".into(),
        (_, false) => format!("u{n:03}0000"),
        (_, true) => format!("u{n:03}0001"),
    }
}

/// One application of a stage-1 plan (UNI-11): its identity, the digest of its record as planned and its actions
/// (close request, uninstaller) in plan order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedApp {
    pub identity: AppIdentity,
    pub record_digest: String,
    pub action_ids: Vec<String>,
}

/// The applications of a stage-1 plan in plan order; empty for any other plan (leftovers, restores).
pub fn planned_apps(plan: &PlanV2) -> Vec<PlannedApp> {
    let mut out: Vec<PlannedApp> = Vec::new();
    if plan.body.plan_kind != PlanKind::Uninstall {
        return out;
    }
    for (a, m) in plan.body.items.iter().zip(&plan.meta) {
        let Some(UninstallMeta::App { identity, record, .. }) = &m.uninstall else { continue };
        match out.iter_mut().find(|p| p.identity.instance_id == identity.instance_id) {
            Some(p) => p.action_ids.push(a.action_id().to_owned()),
            None => out.push(PlannedApp { identity: identity.clone(), record_digest: record.digest(), action_ids: vec![a.action_id().to_owned()] }),
        }
    }
    out
}

/// How one application of a stage-1 operation ended (UNI-11: results are reported per application).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AppOutcome {
    pub app_instance_id: String,
    pub name: String,
    /// `completed`, `partial` (the uninstaller succeeded but something remains or a restart is needed), `failed`,
    /// `skipped`, `not_started`, `unknown`, or `pending` while the operation has no report.
    pub status: &'static str,
    /// Code and message of the step that decided it: the first step of this app that did not succeed, else its uninstaller.
    pub code: Option<String>,
    pub message: Option<String>,
    pub action_ids: Vec<String>,
    /// Its uninstaller ran and succeeded. Only then are its leftovers reviewed; a failed, skipped or stale uninstall
    /// keeps everything of that app.
    pub leftovers_reviewable: bool,
}

/// Per-application outcomes of a stage-1 plan, in plan order (`report` `None`: not finished).
pub fn app_outcomes(plan: &PlanV2, report: Option<&OperationReport>) -> Vec<AppOutcome> {
    planned_apps(plan)
        .into_iter()
        .map(|app| {
            let result = |id: &str| report.and_then(|r| r.items.iter().find(|i| i.action_id == id));
            let uninstaller = app.action_ids.iter().find(|id| plan.body.items.iter().any(|a| a.action_id() == id.as_str() && matches!(a, PlanAction::ToolAdapter(_))));
            let run = uninstaller.and_then(|id| result(id));
            let first_problem = app.action_ids.iter().filter_map(|id| result(id)).find(|r| r.status != ActionStatus::Succeeded);
            let decisive = first_problem.or(run);
            let status = match (report, run) {
                (None, _) => "pending",
                (Some(_), None) => "unknown",
                (Some(_), Some(r)) => match r.status {
                    ActionStatus::Succeeded if cm_core::uninstall::PARTIAL_CODES.contains(&r.code.as_str()) => "partial",
                    ActionStatus::Succeeded => "completed",
                    ActionStatus::Failed => "failed",
                    ActionStatus::Skipped => "skipped",
                    ActionStatus::NotStarted => "not_started",
                    ActionStatus::Unknown => "unknown",
                },
            };
            AppOutcome {
                app_instance_id: app.identity.instance_id.clone(),
                name: app.identity.name.clone(),
                status,
                code: decisive.map(|r| r.code.clone()),
                message: decisive.map(|r| r.message.clone()),
                action_ids: app.action_ids,
                leftovers_reviewable: run.is_some_and(|r| r.status == ActionStatus::Succeeded),
            }
        })
        .collect()
}

/// One refusal for a whole batch: the single refusal as it is, or every application's reason (UNI-11).
pub fn batch_refusal(refused: Vec<(String, Refused)>) -> Refused {
    if refused.len() == 1 {
        return refused.into_iter().next().map(|(_, r)| r).expect("one refusal");
    }
    let same = refused.windows(2).all(|w| w[0].1.code == w[1].1.code);
    let code = if same { refused.first().map(|(_, r)| r.code.clone()).unwrap_or_else(|| "invalid_argument".into()) } else { "invalid_argument".into() };
    let settings_uri = refused.iter().find_map(|(_, r)| r.settings_uri.clone());
    let reasons: Vec<String> = refused.into_iter().map(|(who, r)| format!("{who}: {}", r.message)).collect();
    Refused { code, message: format!("none of the selected applications can be uninstalled here: {}", reasons.join("; ")), settings_uri }
}

/// The stage-1 applications of a finished (or running) uninstall operation.
pub fn stage_one(stores: &Stores, operation_id: &str) -> Result<(PlanV2, Vec<PlannedApp>, Option<OperationReport>), Refused> {
    let plan = stores.audit.load_plan_v2(&stores.audit.operation_plan(operation_id)?)?;
    let apps = planned_apps(&plan);
    if apps.is_empty() {
        return Err(Refused::new("invalid_argument", format!("operation {operation_id} is not an application uninstall")));
    }
    let report: Option<OperationReport> = stores.audit.operation_report(operation_id)?.and_then(|j| serde_json::from_str(&j).ok());
    Ok((plan, apps, report))
}

/// Freeze the recorded backups of an uninstall-leftovers operation into a `recovery` plan (keys and startup values).
pub fn create_restore_plan(stores: &Stores, operation_id: &str, plan_id: &str, now: i64, policy_digest: &str) -> Result<PlanV2, Refused> {
    let source = stores.audit.load_plan_v2(&stores.audit.operation_plan(operation_id)?)?;
    if source.body.plan_kind != PlanKind::Uninstall {
        return Err(Refused::new("invalid_argument", format!("operation {operation_id} is not an uninstall operation")));
    }
    let report: Option<OperationReport> = stores.audit.operation_report(operation_id)?.and_then(|j| serde_json::from_str(&j).ok());
    let Some(report) = report else { return Err(Refused::new("invalid_argument", format!("operation {operation_id} has not finished"))) };
    let mut items = Vec::new();
    let mut meta = Vec::new();
    for (action_id, backup_id, state) in stores.audit.backup_ids_for_operation(operation_id)? {
        let done = report.items.iter().any(|i| i.action_id == action_id && i.status == ActionStatus::Succeeded);
        if !done || state != "recorded" {
            continue;
        }
        let action_id = format!("r{:07}", items.len());
        if backup_id.starts_with(KEY_BACKUP_PREFIX) {
            let (b, _) = stores.audit.key_backup(&backup_id)?;
            items.push(PlanAction::RegistryRestore(RegistryRestoreAction {
                action_id, backup_id: b.backup_id.clone(), hive: b.location.hive, key_path: b.location.key_path.clone(), value_name: None, backup_sha256: Some(b.sha256()),
                on_existing: "skip".into(),
            }));
            meta.push(ActionMeta { note: Some(b.location.display()), uninstall: Some(UninstallMeta::RegistryKey { location: b.location }), ..Default::default() });
        } else {
            let (b, _) = stores.audit.startup_backup(&backup_id)?;
            let cm_core::startup::StartupLocation::RunValue { hive, key, value_name, .. } = &b.location else { continue };
            items.push(PlanAction::RegistryRestore(RegistryRestoreAction {
                action_id, backup_id: b.backup_id.clone(), hive: *hive, key_path: key.clone(), value_name: Some(value_name.clone()), backup_sha256: Some(b.sha256()),
                on_existing: "skip".into(),
            }));
            meta.push(ActionMeta { startup: Some(b.location.clone()), note: Some(value_name.clone()), ..Default::default() });
        }
    }
    if items.is_empty() {
        return Err(Refused::new("invalid_argument", "the operation recorded no registry backup that can still be restored"));
    }
    let header = HeaderInput {
        plan_id, plan_kind: PlanKind::Recovery, created_unix: now, scope_id: None, scan_id: None, scan_revision: None, rule_set_digest: NO_RULES_DIGEST, policy_digest,
    };
    let plan = cm_core::plan::v2::build(&header, items, meta).map_err(|e| Refused::new("internal_error", e.to_string()))?;
    stores.audit.save_plan_v2(&plan)?;
    Ok(plan)
}

#[cfg(windows)]
pub use win::*;

#[cfg(windows)]
mod win {
    use std::collections::BTreeMap;
    use std::path::Path;

    use cm_core::host::HostRef;
    use cm_core::manual_trash::{ManualInput, SelectionGuard, build_trash_plan};
    use cm_core::path::RawPath;
    use cm_core::plan::v2::{StartupOperation, StartupToggleAction};
    use cm_core::startup::StartupActions;
    use cm_core::uninstall::{Binding, Group, Leftover, UninstallRecord, select_adapter};
    use cm_platform::windows::uninstall::{DiscoveryInput, UninstallEnv, WindowsUninstall, adapter_env, discover, installed_apps, records, verify_program};

    use super::*;

    /// A stage-1 plan with its (non-binding) leftover estimate by group.
    pub struct StageOne {
        pub plan: PlanV2,
        pub record: UninstallRecord,
        pub estimate: BTreeMap<Group, usize>,
    }

    /// One application accepted into a batch: its record, its actions and its (non-binding) leftover estimate.
    pub struct BatchApp {
        pub record: UninstallRecord,
        pub action_ids: Vec<String>,
        pub estimate: BTreeMap<Group, usize>,
    }

    /// One application refused exactly as a single uninstall of it would be (UNI-10/11); the others are unaffected.
    pub struct BatchRefusal {
        pub app_instance_id: String,
        pub name: Option<String>,
        pub refused: Refused,
    }

    /// A stage-1 plan for several applications (UNI-11) and the applications that were refused.
    pub struct StageOneBatch {
        pub plan: PlanV2,
        pub apps: Vec<BatchApp>,
        pub refused: Vec<BatchRefusal>,
    }

    /// Stage 1 (UNI-01/02/03/10): `app_quit` + verified `tool_adapter` for one instance; `end_running` is the separately
    /// reviewed variant that may end processes after a person confirms it at apply time.
    pub fn create_uninstall_plan(stores: &Stores, env: &UninstallEnv, instance_id: &str, end_running: bool, plan_id: &str, now: i64, policy_digest: &str) -> Result<StageOne, Refused> {
        let mut b = create_uninstall_batch(stores, env, &[instance_id.to_owned()], end_running, plan_id, now, policy_digest)?;
        let app = b.apps.remove(0);
        Ok(StageOne { plan: b.plan, record: app.record, estimate: app.estimate })
    }

    /// Stage 1 for several instances in one plan (UNI-11). Each application gets its own close request and verified
    /// uninstaller, checked exactly as for a single uninstall; a refused application (protected class, unverifiable
    /// route, unsigned program…) is reported and left out without affecting the others. Only when every one is refused
    /// is the whole request refused.
    pub fn create_uninstall_batch(
        stores: &Stores, env: &UninstallEnv, instance_ids: &[String], end_running: bool, plan_id: &str, now: i64, policy_digest: &str,
    ) -> Result<StageOneBatch, Refused> {
        if instance_ids.is_empty() {
            return Err(Refused::new("invalid_argument", "name at least one application instance"));
        }
        if instance_ids.len() > MAX_BATCH {
            return Err(Refused::new("invalid_argument", format!("at most {MAX_BATCH} applications in one uninstall plan")));
        }
        for (i, id) in instance_ids.iter().enumerate() {
            if instance_ids[..i].contains(id) {
                return Err(Refused::new("invalid_argument", format!("{id} is named twice")));
            }
        }
        let (all, _) = records(env);
        let mut items = Vec::new();
        let mut meta = Vec::new();
        let mut apps = Vec::new();
        let mut refused = Vec::new();
        for id in instance_ids {
            match app_actions(&all, env, id, end_running, apps.len()) {
                Ok((rec, identity, acts)) => {
                    let action_ids = acts.iter().map(|(a, _)| a.action_id().to_owned()).collect();
                    for (a, m) in acts {
                        items.push(a);
                        meta.push(m);
                    }
                    // Estimate only (D7): what would remain if the uninstaller removed nothing else; never part of the plan.
                    let others: Vec<_> = installed_apps(&all).into_iter().filter(|a| a.instance_id != rec.instance_id).collect();
                    let mut estimate = BTreeMap::new();
                    for l in discover(env, &DiscoveryInput { identity: &identity, installed: &others, startup: &[] }) {
                        *estimate.entry(l.group).or_insert(0) += 1;
                    }
                    apps.push(BatchApp { record: rec, action_ids, estimate });
                }
                Err(r) => refused.push(BatchRefusal { app_instance_id: id.clone(), name: all.iter().find(|x| &x.instance_id == id).map(|x| x.name.clone()), refused: r }),
            }
        }
        if apps.is_empty() {
            return Err(batch_refusal(refused.into_iter().map(|b| (b.name.unwrap_or(b.app_instance_id), b.refused)).collect()));
        }
        let header = HeaderInput {
            plan_id, plan_kind: PlanKind::Uninstall, created_unix: now, scope_id: None, scan_id: None, scan_revision: None, rule_set_digest: NO_RULES_DIGEST, policy_digest,
        };
        let plan = cm_core::plan::v2::build(&header, items, meta).map_err(|e| Refused::new("internal_error", e.to_string()))?;
        stores.audit.save_plan_v2(&plan)?;
        Ok(StageOneBatch { plan, apps, refused })
    }

    /// The two stage-1 actions of one application (the `n`-th of its plan), after its route, program and signer were verified.
    #[allow(clippy::type_complexity)]
    fn app_actions(all: &[UninstallRecord], env: &UninstallEnv, instance_id: &str, end_running: bool, n: usize) -> Result<(UninstallRecord, AppIdentity, Vec<(PlanAction, ActionMeta)>), Refused> {
        let Some(rec) = all.iter().find(|r| r.instance_id == instance_id).cloned() else {
            return Err(Refused::new("invalid_argument", format!("no installed application {instance_id}; run `cleanmanager apps uninstall list`")));
        };
        let choice = select_adapter(&rec, &adapter_env(env, &rec), &|p: &str| Path::new(p).is_file()).map_err(|r| Refused {
            code: r.code.into(),
            message: r.message,
            settings_uri: r.settings_uri.map(str::to_owned),
        })?;
        let (sha, signer) = verify_program(&choice).map_err(|(c, m)| Refused { code: c.into(), message: m, settings_uri: Some(cm_core::uninstall::SETTINGS_URI.into()) })?;
        let mut identity = AppIdentity::of(&rec);
        // No recorded location: a vendor uninstaller's own folder (never a Windows folder) identifies the app's processes.
        if identity.install_location.is_none() && choice.kind == cm_core::uninstall::AdapterKind::RegisteredCommand {
            let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
            identity.install_location = Path::new(&choice.program).parent().map(|p| p.to_string_lossy().into_owned()).filter(|d| !cm_core::uninstall::path_within(d, &windows));
        }
        let app_meta = UninstallMeta::App { identity: identity.clone(), record: Box::new(rec.clone()), adapter: choice.clone(), signer };
        let items = vec![
            PlanAction::AppQuit(AppQuitAction {
                action_id: batch_action_id(n, false), app_instance_id: rec.instance_id.clone(),
                app_identity: rec.install_location.clone().or_else(|| rec.package_full_name.clone()).unwrap_or_else(|| rec.name.clone()),
                force: if end_running { QuitForce::AskUser } else { QuitForce::Never },
            }),
            PlanAction::ToolAdapter(ToolAdapterAction {
                action_id: batch_action_id(n, true), adapter_id: choice.kind.adapter_id().into(), adapter_version: ADAPTER_VERSION.into(),
                executable: ExecutableRef { path: RawPath::windows(choice.program.encode_utf16().collect()).map_err(|e| Refused::new("unsupported", e.to_string()))?, sha256: sha },
                argv: choice.argv.clone(), preview_digest: adapter_digest(&rec, &choice), timeout_seconds: UNINSTALLER_TIMEOUT_SECONDS,
            }),
        ];
        let meta = vec![
            ActionMeta { note: Some(format!("quit {} (close request{})", rec.name, if end_running { "; may end it after you confirm" } else { "" })), uninstall: Some(app_meta.clone()), ..Default::default() },
            ActionMeta { note: Some(choice.display.clone()), uninstall: Some(app_meta), ..Default::default() },
        ];
        Ok((rec, identity, items.into_iter().zip(meta).collect()))
    }

    /// The leftover review of one application of a stage-1 operation (UNI-04/05/07/09/11).
    pub struct AppLeftovers {
        pub outcome: AppOutcome,
        pub identity: AppIdentity,
        /// Empty when its uninstaller did not succeed: nothing of an application that is still installed is a leftover.
        pub items: Vec<Leftover>,
        pub external_references: Vec<String>,
    }

    /// The grouped, read-only leftover preview of a finished stage-1 operation, per application in plan order.
    /// Applications whose uninstaller failed, was skipped or did not run keep everything (no items); an object claimed
    /// by two applications of the batch is listed once, under the first.
    pub fn leftovers_by_app(stores: &Stores, env: &UninstallEnv, operation_id: &str) -> Result<Vec<AppLeftovers>, Refused> {
        let (plan, apps, report) = stage_one(stores, operation_id)?;
        let Some(report) = report else {
            return Err(Refused::new("invalid_argument", format!("operation {operation_id} has not finished; leftovers are reviewed after the uninstaller ran")));
        };
        let outcomes = app_outcomes(&plan, Some(&report));
        let (all, _) = records(env);
        // The registration exactly as it was before a successful uninstaller is a leftover of it, not a reinstall; a
        // changed or new registration of the same app is `app_reappeared` and keeps everything. An application of the
        // batch whose uninstaller did not succeed is still installed and protects what it shares.
        let gone: Vec<(&str, &str)> =
            apps.iter().zip(&outcomes).filter(|(_, o)| o.leftovers_reviewable).map(|(a, _)| (a.identity.instance_id.as_str(), a.record_digest.as_str())).collect();
        let live: Vec<UninstallRecord> = all.into_iter().filter(|r| !gone.iter().any(|(i, d)| r.instance_id == *i && r.digest() == *d)).collect();
        let installed = installed_apps(&live);
        let (startup, _) = cm_platform::windows::startup::inventory_with(&env.startup_env());
        let mut seen = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        for (app, outcome) in apps.into_iter().zip(outcomes) {
            let (items, refs) = if outcome.leftovers_reviewable {
                let items: Vec<Leftover> = discover(env, &DiscoveryInput { identity: &app.identity, installed: &installed, startup: &startup }).into_iter().filter(|l| seen.insert(l.object_id.clone())).collect();
                (items, cm_platform::windows::uninstall::external_references(env, &app.identity))
            } else {
                (vec![], vec![])
            };
            out.push(AppLeftovers { outcome, identity: app.identity, items, external_references: refs });
        }
        Ok(out)
    }

    /// Why an operation has nothing to review: every application's uninstaller failed, was skipped or did not run.
    pub fn none_reviewable(operation_id: &str, apps: &[AppLeftovers]) -> Option<Refused> {
        if apps.iter().any(|a| a.outcome.leftovers_reviewable) {
            return None;
        }
        let why: Vec<String> = apps.iter().map(|a| format!("{} ({}{})", a.outcome.name, a.outcome.status, a.outcome.code.as_deref().map(|c| format!(": {c}")).unwrap_or_default())).collect();
        Some(Refused::new(
            "invalid_argument",
            format!("the uninstall in operation {operation_id} did not complete for {}; its leftovers are not reviewed or removed while the application is still installed", why.join(", ")),
        ))
    }

    /// Stage 2: freeze default-selected leftovers plus explicitly named objects into a separately reviewed `uninstall`
    /// plan; with a batch, of every application whose uninstaller succeeded (UNI-11).
    #[allow(clippy::too_many_arguments)]
    pub fn create_leftovers_plan(
        stores: &Stores, env: &UninstallEnv, operation_id: &str, extra: &[String], exclude: &[String], host: &HostRef, plan_id: &str, now: i64, policy_digest: &str,
    ) -> Result<(PlanV2, Vec<Leftover>), Refused> {
        let by_app = leftovers_by_app(stores, env, operation_id)?;
        if let Some(r) = none_reviewable(operation_id, &by_app) {
            return Err(r);
        }
        let batch = by_app.len() > 1;
        // Which application each object belongs to (shown in review when the operation had several).
        let owner: BTreeMap<String, String> = by_app.iter().flat_map(|a| a.items.iter().map(|l| (l.object_id.clone(), a.identity.name.clone()))).collect();
        let all: Vec<Leftover> = by_app.into_iter().flat_map(|a| a.items).collect();
        // UNI-05: a person may clear preselected caches/logs; only preselected objects can be excluded.
        for x in exclude {
            if !all.iter().any(|l| &l.object_id == x && l.selected) {
                return Err(Refused::new("invalid_argument", format!("{x} is not a preselected leftover of this operation (only caches/logs chosen by default can be excluded)")));
            }
        }
        for x in extra {
            match all.iter().find(|l| &l.object_id == x) {
                None => return Err(Refused::new("invalid_argument", format!("{x} is not a leftover of this operation (use object ids from `apps uninstall leftovers`)"))),
                Some(l) if l.tier == "kept" || l.tier == "blocked" => return Err(Refused::new("policy_denied", format!("{} is {}: {}", l.display, l.tier, l.evidence.join("; ")))),
                Some(l) if matches!(l.binding, Binding::Manual { .. }) => {
                    let Binding::Manual { guidance } = &l.binding else { unreachable!() };
                    return Err(Refused::new("unsupported", format!("{}: {guidance}", l.display)));
                }
                Some(l) if l.requires_admin && !env.is_admin => return Err(Refused::new("permission_required", format!("{} needs an administrator terminal", l.display))),
                Some(_) => {}
            }
        }
        let chosen: Vec<Leftover> = all.iter().filter(|l| (l.selected && !exclude.contains(&l.object_id)) || extra.contains(&l.object_id)).cloned().collect();
        if chosen.is_empty() {
            return Err(Refused::new("invalid_argument", "nothing is selected: no cache or log leftovers were found; name objects with --items to include them"));
        }
        let mut items = Vec::new();
        let mut meta = Vec::new();
        let hard = cm_platform::protection::hard_deny_set();
        let user = cm_platform::protection::load(&stores.dir);
        let problem = user.as_ref().err().map(ToString::to_string);
        let guard = SelectionGuard { hard: Some(&hard), user: user.as_ref().map_err(|_| problem.as_deref().unwrap_or("unavailable")) };
        let startup_env = env.startup_env();
        let (startup, _) = cm_platform::windows::startup::inventory_with(&startup_env);
        let ws = WindowsUninstall::non_interactive(env.clone());
        for (n, l) in chosen.iter().enumerate() {
            // UNI-11: with several applications every note starts with the application it belongs to (text for people).
            let app = |s: String| match owner.get(&l.object_id).filter(|_| batch) {
                Some(name) => format!("{name} · {s}"),
                None => s,
            };
            match &l.binding {
                Binding::Path { path } => {
                    for (a, m) in file_actions(stores, env, host, plan_id, n, path, &guard, now, policy_digest)? {
                        items.push(a);
                        meta.push(ActionMeta { note: Some(app(format!("{:?}: {}", l.group, l.display))), ..m });
                    }
                }
                Binding::Registry { location } => {
                    let tree = ws.read_key(location).map_err(|e| Refused::new("io_error", e))?.ok_or_else(|| Refused::new("conflict", format!("{} no longer exists", location.display())))?;
                    items.push(PlanAction::RegistryBackupDelete(RegistryBackupDeleteAction {
                        action_id: format!("k{n:07}"), hive: location.hive, key_path: location.key_path.clone(), value_name: None, owner_evidence: l.evidence.join("; "),
                        backup_required: true, observed_sha256: Some(key_digest(location, &tree)),
                    }));
                    meta.push(ActionMeta { note: Some(app(location.display())), uninstall: Some(UninstallMeta::RegistryKey { location: location.clone() }), ..Default::default() });
                }
                Binding::Startup { startup_item_id } => {
                    let item = startup.iter().find(|s| &s.item_id == startup_item_id).ok_or_else(|| Refused::new("conflict", format!("{} changed; list leftovers again", l.display)))?;
                    cm_core::startup::change_decision(item, StartupOperation::RemoveWithBackup, startup_env.is_admin).map_err(|r| Refused::new(r.code, r.message))?;
                    let loc = item.location.clone().expect("checked by change_decision");
                    let wst = cm_platform::windows::startup_actions::WindowsStartup { env: startup_env.clone() };
                    let obs = wst.observe(&loc).map_err(|e| Refused::new("io_error", e))?;
                    items.push(PlanAction::StartupToggle(StartupToggleAction {
                        action_id: format!("s{n:07}"), startup_item_id: item.item_id.clone(), source: item.source, operation: StartupOperation::RemoveWithBackup, scope: item.scope,
                        definition: Some(item.definition.clone()), observed_sha256: Some(cm_core::startup::observation_digest(&loc, &obs)), backup_id: None,
                    }));
                    meta.push(ActionMeta { startup: Some(loc), note: Some(app(item.name.clone())), ..Default::default() });
                }
                Binding::Manual { .. } => {}
            }
        }
        if items.is_empty() {
            return Err(Refused::new("invalid_argument", "the selected leftovers are already empty or gone; nothing to plan"));
        }
        let header = HeaderInput {
            plan_id, plan_kind: PlanKind::Uninstall, created_unix: now, scope_id: None, scan_id: None, scan_revision: None, rule_set_digest: NO_RULES_DIGEST, policy_digest,
        };
        let plan = cm_core::plan::v2::build(&header, items, meta).map_err(|e| Refused::new("internal_error", e.to_string()))?;
        stores.audit.save_plan_v2(&plan)?;
        Ok((plan, chosen))
    }

    /// `fs_trash` actions for one leftover, bound to a fresh scan and the same protection and coverage checks as a manual
    /// selection (UNI-06: files go to the Recycle Bin). A folder is scanned itself and its contents are moved (the
    /// emptied folder stays: its parent may be a whole data root, which is never scanned); a file is bound through a
    /// scan of its own folder, never of a home, data root or desktop folder.
    #[allow(clippy::too_many_arguments)]
    fn file_actions(
        stores: &Stores, env_folders: &UninstallEnv, host: &HostRef, plan_id: &str, n: usize, path: &str, guard: &SelectionGuard<'_>, now: i64,
        policy_digest: &str,
    ) -> Result<Vec<(PlanAction, ActionMeta)>, Refused> {
        let p = Path::new(path);
        let is_dir = p.symlink_metadata().map(|m| m.is_dir()).map_err(|e| Refused::new("conflict", format!("{path}: {e}")))?;
        let root = if is_dir { p } else { p.parent().ok_or_else(|| Refused::new("invalid_argument", "no parent folder"))? };
        // A file is bound through a scan of its folder; never of a whole home, data root or desktop folder.
        let f = env_folders.folders();
        let broad = [f.home, f.roaming, f.local, f.program_data].into_iter().chain(f.desktop);
        if !is_dir && broad.into_iter().any(|b| b.to_string_lossy().eq_ignore_ascii_case(&root.to_string_lossy())) {
            return Err(Refused::new("unsupported", format!("{path} sits directly in {}; delete it yourself after review", root.display())));
        }
        let raw_root = RawPath::windows(root.as_os_str().to_string_lossy().encode_utf16().collect()).map_err(|e| Refused::new("invalid_argument", e.to_string()))?;
        if let Err(why) = cm_platform::windows::actions::recycle_bin_ready(&raw_root) {
            return Err(Refused::new("unsupported", format!("{path}: {why}; there is no permanent-delete alternative")));
        }
        let scan_id = format!("{plan_id}_s{n}");
        let request = crate::scan::ScanRequest { scan_id: scan_id.clone(), scope_id: format!("scope_{plan_id}_{n}"), root: raw_root, host: host.clone() };
        let backend = cm_platform::windows::directory::DirectoryBackend;
        let outcome = crate::scan::run_scan(&stores.index, &backend, &request, &cm_core::scan::ScanOptions::default())?;
        if outcome.state != cm_core::model::ScanState::Completed {
            return Err(Refused::new("conflict", format!("{path} could not be read completely ({:?}); it is not planned", outcome.state)));
        }
        let status = stores.index.scan_status(&scan_id)?;
        let view = stores.index.view(&scan_id)?;
        use cm_core::analysis::IndexView;
        let entries: Vec<u64> = if is_dir {
            view.children(view.root()).to_vec()
        } else {
            let name = p.file_name().map(|f| f.to_string_lossy().to_lowercase()).unwrap_or_default();
            vec![*view.children(view.root()).iter().find(|&&c| view.path(c).display().to_lowercase().ends_with(&format!("\\{name}"))).ok_or_else(|| Refused::new("conflict", format!("{path} is gone")))?]
        };
        if entries.is_empty() {
            return Ok(vec![]);
        }
        let observed_at = status.finished_at.clone().unwrap_or(status.started_at.clone());
        let input = ManualInput {
            plan_id, scope_id: Some(&status.scope_id), scan_id: Some(&scan_id), scan_revision: Some(status.revision), created_unix: now, observed_at: &observed_at, policy_digest,
        };
        let built = build_trash_plan(&view, &entries, guard, &input).map_err(|e| Refused { code: match e.code() {
            cm_core::error::ErrorCode::PolicyDenied => "policy_denied".into(),
            cm_core::error::ErrorCode::Unsupported => "unsupported".into(),
            cm_core::error::ErrorCode::PermissionRequired => "permission_required".into(),
            _ => "invalid_argument".into(),
        }, message: format!("{path}: {e}"), settings_uri: None })?;
        Ok(built
            .body
            .items
            .into_iter()
            .zip(built.meta)
            .enumerate()
            .map(|(i, (mut a, m))| {
                if let PlanAction::FsTrash(f) = &mut a {
                    f.action_id = format!("f{n:04}{i:04}");
                }
                (a, m)
            })
            .collect())
    }
}

#[cfg(test)]
mod tests;
