//! P06 execution, P08 activity and P09 quarantine/recovery (EXE-*, REC-01..05, GUI-01/06).
//!
//! Safety properties of this IPC surface:
//! - `start_apply` names a stored plan by id. The shared executor (`cm_app::execute::apply_plan`
//!   or `cm_app::recovery::apply_recovery_plan`, dispatched on the stored plan type) checks plan
//!   validity, the stored authorization (receipt or policy + quota), dedupe and identity locks,
//!   re-checks protection and live preconditions before each action, and journals every intent
//!   and result. The WebView cannot widen a plan or pass a path; an unauthorized plan is
//!   refused before anything is touched.
//! - `cancel_apply` only stops scheduling new actions; the action in flight finishes and is
//!   journaled. There is no retry and no "run the batch again": the same plan returns the
//!   same operation (EXE-03).
//! - `reconcile_operation` only inspects; it never starts an action.
//! - `create_recovery_plan` freezes quarantine ids into a `recovery/v1` plan; it authorizes
//!   nothing and goes through the same review page as a cleanup plan.
//! - `plan/v2` plans (manual trash, startup, startup restore) dispatch to `crate::v2`, which refuses kinds this window
//!   does not apply and plans that need an administrator while this process is not elevated, before anything starts.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cm_app::execute::{ActionResult, ActionStatus, ApplyError, OperationReport};
use cm_core::authz::{current_principal, policy_covers, receipt_valid};
use cm_core::error::ErrorCode;
use cm_core::host::HostProbe;
use cm_core::plan::v2::PLAN_V2_CONTRACT;
use cm_core::recovery::{RECOVERY_CONTRACT, RecoveryKind};
use cm_core::rules::Mode;
use cm_platform::host::OsHostProbe;
use cm_store::Stores;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::args;
use crate::cleanup::now_unix;
use crate::commands::{St, blocking};
use crate::dto::{IpcError, IpcResult, last_component};
use crate::host::{AppState, lock};

pub const EVENT_APPLY_PROGRESS: &str = "apply-progress";
pub const EVENT_APPLY_FINISHED: &str = "apply-finished";
const PROGRESS_INTERVAL: Duration = Duration::from_millis(400);
const MAX_RECOVERY_SELECTION: usize = 500;

pub struct ActiveApply {
    pub plan_id: String,
    pub operation_id: String,
    pub total: u64,
    pub cancel: Arc<AtomicBool>,
    pub done: Arc<AtomicBool>,
}

/// (cancel flag, done flag) of an operation this process is running right now.
pub fn running_apply(state: &AppState) -> Option<(Arc<AtomicBool>, Arc<AtomicBool>)> {
    lock(&state.apply).as_ref().filter(|a| !a.done.load(Ordering::Acquire)).map(|a| (a.cancel.clone(), a.done.clone()))
}

fn ipc(code: ErrorCode, reason: &'static str, message: impl Into<String>) -> IpcError {
    IpcError::new(code, reason, message)
}

impl From<ApplyError> for IpcError {
    fn from(e: ApplyError) -> Self {
        let reason = match &e {
            ApplyError::NeedsApproval(_) => "needs_approval",
            ApplyError::Stale(_) => "plan_stale",
            ApplyError::PolicyDenied(_) => "policy_denied",
            ApplyError::Conflict(_) => "operation_conflict",
            ApplyError::Crashed(_) => "internal",
            ApplyError::ProtectionUnavailable(_) => "protection_unavailable",
            ApplyError::Store(_) => "store_error",
        };
        IpcError::new(e.code(), reason, e.to_string())
    }
}

/// `cleanup`, `restore`, `purge` or a plan/v2 kind name, the cleanup mode, digest and item count, from the stored plan.
fn plan_kind(stores: &Stores, plan_id: &str) -> IpcResult<(&'static str, Option<Mode>, String, u64)> {
    let contract = stores.audit.plan_contract(plan_id)?;
    if contract == PLAN_V2_CONTRACT {
        let p = stores.audit.load_plan_v2(plan_id)?;
        return Ok((crate::v2::kind_name(&p), crate::v2::mode_of(&p), p.digest.clone(), p.body.items.len() as u64));
    }
    if contract == RECOVERY_CONTRACT {
        let p = stores.audit.load_recovery_plan(plan_id)?;
        let kind = match p.body.recovery_kind {
            RecoveryKind::Restore => "restore",
            RecoveryKind::Purge => "purge",
        };
        Ok((kind, None, p.digest, p.body.items.len() as u64))
    } else {
        let p = stores.audit.load_plan(plan_id)?;
        Ok(("cleanup", Some(p.body.mode), p.digest, p.body.items.len() as u64))
    }
}

/// Refuse early, before a thread starts, when nothing could authorize the plan (the executor
/// repeats every check itself).
fn precheck(s: &AppState, stores: &Stores, plan_id: &str) -> IpcResult<()> {
    let now = now_unix();
    let principal = current_principal();
    let contract = stores.audit.plan_contract(plan_id)?;
    if contract == PLAN_V2_CONTRACT {
        cm_app::execute::require_protection(stores)?;
        return crate::v2::precheck(stores, plan_id);
    }
    if contract == RECOVERY_CONTRACT {
        let plan = stores.audit.load_recovery_plan(plan_id)?;
        if crate::cleanup::recovery_authorization(stores, &plan, now)?.status != "authorized" {
            return Err(ApplyError::NeedsApproval(plan_id.into()).into());
        }
        return Ok(());
    }
    let plan = stores.audit.load_plan(plan_id)?;
    // PRT-02 / BUG-WFA-03: nothing starts (and nothing is consumed) while the protection list is unusable; the executor
    // repeats this before it registers an operation.
    cm_app::execute::require_protection(stores)?;
    let digest = crate::cleanup::rules_digest_for(s, &plan).map_err(|e| ipc(ErrorCode::InternalError, "rules_unavailable", e))?;
    cm_core::plan::check_plan(&plan, now, &digest).map_err(ApplyError::Stale)?;
    let receipt = stores.audit.receipts_for(plan_id)?.iter().any(|r| receipt_valid(r, &plan, &principal, now));
    let policy = stores.audit.policies()?.iter().any(|p| policy_covers(p, &plan, &principal, now).is_ok());
    if !receipt && !policy {
        return Err(ApplyError::NeedsApproval(plan_id.into()).into());
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct ApplyStartDto {
    pub plan_id: String,
    pub operation_id: String,
    pub total: u64,
    /// The plan already ran: its existing operation is shown, nothing runs again (EXE-03).
    pub existing: bool,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub struct Counts {
    pub succeeded: u64,
    pub skipped: u64,
    pub failed: u64,
    pub not_started: u64,
    pub unknown: u64,
    /// Intent recorded, no result yet: the action in flight (or unconfirmed after a crash).
    pub in_flight: u64,
}

fn journal_counts(stores: &Stores, operation_id: &str) -> Counts {
    let mut c = Counts::default();
    for row in stores.audit.journal(operation_id).unwrap_or_default() {
        match row.result_status.as_deref() {
            Some("succeeded") => c.succeeded += 1,
            Some("skipped") => c.skipped += 1,
            Some("failed") => c.failed += 1,
            Some("not_started") => c.not_started += 1,
            Some("unknown") => c.unknown += 1,
            _ => c.in_flight += 1,
        }
    }
    c
}

#[derive(Debug, Clone, Serialize)]
struct ProgressEvent {
    plan_id: String,
    operation_id: String,
    total: u64,
    counts: Counts,
    cancel_requested: bool,
}

#[derive(Debug, Clone, Serialize)]
struct FinishedEvent {
    plan_id: String,
    operation_id: String,
    status: Option<String>,
    error: Option<IpcError>,
}

fn run(stores: &Stores, plan_id: &str, rules_digest: &str, cancel: &AtomicBool) -> Result<OperationReport, ApplyError> {
    let principal = current_principal();
    let host = OsHostProbe.current("desktop");
    let cancelled = || cancel.load(Ordering::Acquire);
    match stores.audit.plan_contract(plan_id).ok().as_deref() {
        Some(RECOVERY_CONTRACT) => {
            let ctx = cm_app::recovery::RecoveryContext { principal: &principal, host: &host, now: &now_unix, cancel: &cancelled, fault: &|_, _| false };
            return cm_app::recovery::apply_recovery_plan(stores, &crate::plat::recovery_actions(), plan_id, &ctx);
        }
        Some(PLAN_V2_CONTRACT) => return crate::v2::apply(stores, plan_id, &cancelled),
        _ => {}
    }
    // Same wiring as `cleanmanager plan apply`: the platform's verified actions with the store's
    // quarantine, and the protection checker consulted immediately before every action.
    let stored = stores.audit.load_plan(plan_id).ok();
    let rule_refs = stored.as_ref().map(|p| p.rule_refs.clone()).unwrap_or_default();
    let base = cm_platform::protection::checker(&stores.dir, rule_refs);
    // PUR-03 / INC-04: discovery plans re-check version control (and, on macOS, attached images) per action.
    let gate = stored.as_ref().and_then(crate::plat::apply_gate);
    let protect = |t: &cm_core::execute::ActionTarget<'_>| base(t).and_then(|()| gate.as_ref().map_or(Ok(()), |g| g(t)));
    let ctx = cm_app::execute::ApplyContext {
        principal: &principal,
        host: &host,
        current_rule_set_digest: rules_digest,
        now: &now_unix,
        cancel: &cancelled,
        fault: &|_, _| false,
        protect: &protect,
    };
    let actions = crate::plat::file_actions(&stores.dir);
    cm_app::execute::apply_plan(stores, &actions, plan_id, &ctx)
}

/// Start applying a stored, authorized plan on a background thread of this process.
pub fn start(app: &AppHandle, state: &Arc<AppState>, plan_id: &str) -> IpcResult<ApplyStartDto> {
    let dir = state.store_dir()?.to_path_buf();
    let (_, _, digest, total) = state.with_stores(|st| plan_kind(st, plan_id))?;
    let operation_id = format!("op_{}", &digest[..16]);
    if let Some((op, _)) = state.with_stores(|st| Ok(st.audit.operation_for_dedupe(&format!("{plan_id}#{digest}"))?))? {
        return Ok(ApplyStartDto { plan_id: plan_id.into(), operation_id: op, total, existing: true });
    }
    state.with_stores(|st| precheck(state, st, plan_id))?;
    // Cleanup plans: the installed rule pack, or the plan's own digest for built-in catalogs (purge, installers).
    let rules_digest = state
        .with_stores(|st| Ok(st.audit.load_plan(plan_id).ok()))?
        .and_then(|p| crate::cleanup::rules_digest_for(state, &p).ok())
        .unwrap_or_default();

    let mut slot = lock(&state.apply);
    if let Some(a) = slot.as_ref()
        && !a.done.load(Ordering::Acquire)
    {
        return Err(ipc(ErrorCode::Conflict, "operation_running", format!("Operation {} is still running; wait for it or stop it first", a.operation_id)));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    *slot = Some(ActiveApply { plan_id: plan_id.into(), operation_id: operation_id.clone(), total, cancel: cancel.clone(), done: done.clone() });
    drop(slot);

    let (t_app, t_plan, t_op, t_cancel, t_done, t_dir) = (app.clone(), plan_id.to_owned(), operation_id.clone(), cancel.clone(), done.clone(), dir.clone());
    let spawned = std::thread::Builder::new().name("cm-apply".into()).spawn(move || {
        let outcome = Stores::open(&t_dir).map_err(ApplyError::Store).and_then(|stores| run(&stores, &t_plan, &rules_digest, &t_cancel));
        t_done.store(true, Ordering::Release);
        let event = match outcome {
            Ok(r) => FinishedEvent { plan_id: t_plan, operation_id: r.operation_id, status: Some(r.status), error: None },
            Err(e) => FinishedEvent { plan_id: t_plan, operation_id: t_op, status: None, error: Some(e.into()) },
        };
        let _ = t_app.emit(EVENT_APPLY_FINISHED, event);
    });
    if let Err(e) = spawned {
        done.store(true, Ordering::Release);
        return Err(ipc(ErrorCode::InternalError, "internal", format!("could not start the operation thread: {e}")));
    }
    let (p_app, p_plan, p_op) = (app.clone(), plan_id.to_owned(), operation_id.clone());
    let _ = std::thread::Builder::new().name("cm-apply-progress".into()).spawn(move || {
        let Ok(stores) = Stores::open(&dir) else { return };
        let mut last: Option<(Counts, bool)> = None;
        loop {
            let finished = done.load(Ordering::Acquire);
            let key = (journal_counts(&stores, &p_op), cancel.load(Ordering::Acquire));
            if last.as_ref() != Some(&key) {
                let _ = p_app.emit(EVENT_APPLY_PROGRESS, ProgressEvent { plan_id: p_plan.clone(), operation_id: p_op.clone(), total, counts: key.0.clone(), cancel_requested: key.1 });
                last = Some(key);
            }
            if finished {
                break;
            }
            std::thread::sleep(PROGRESS_INTERVAL);
        }
    });
    Ok(ApplyStartDto { plan_id: plan_id.into(), operation_id, total, existing: false })
}

// ---------------------------------------------------------------------------------------------
// Operation detail

#[derive(Debug, Clone, Serialize)]
pub struct OperationDto {
    pub operation_id: String,
    pub plan_id: String,
    /// `cleanup`, `restore`, `purge`, `manual_trash`, `manual_purge`, `startup`, `startup_restore` (other plan/v2
    /// kinds by their raw name) or `unknown` (plan unreadable).
    pub plan_kind: &'static str,
    pub mode: Option<Mode>,
    pub state: String,
    pub running_here: bool,
    pub cancel_requested: bool,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub authorization_source: Option<String>,
    pub total: u64,
    pub counts: Counts,
    pub report_available: bool,
    pub processed_logical_bytes: Option<String>,
    pub quarantined_logical_bytes: Option<String>,
    pub estimated_freed_bytes: Option<String>,
    pub volume_free_before_bytes: Option<String>,
    pub volume_free_after_bytes: Option<String>,
    pub observed_free_delta_bytes: Option<String>,
    /// Outcomes are unconfirmed: a read-only reconciliation must run before anything else.
    pub needs_reconcile: bool,
    /// UNI-11: each application's own result of a stage-1 uninstall (one entry per application; empty otherwise).
    pub uninstall_apps: Vec<cm_app::uninstall::AppOutcome>,
}

fn report_of(stores: &Stores, operation_id: &str) -> IpcResult<Option<OperationReport>> {
    Ok(stores.audit.operation_report(operation_id)?.and_then(|j| serde_json::from_str::<OperationReport>(&j).ok()))
}

pub fn operation(s: &AppState, stores: &Stores, operation_id: &str) -> IpcResult<OperationDto> {
    let state = stores.audit.operation_state(operation_id)?;
    let plan_id = stores.audit.operation_plan(operation_id)?;
    let (kind, mode, total) = match plan_kind(stores, &plan_id) {
        Ok((k, m, _, t)) => (k, m, t),
        Err(_) => ("unknown", None, 0),
    };
    let active = lock(&s.apply).as_ref().filter(|a| a.operation_id == operation_id && !a.done.load(Ordering::Acquire)).map(|a| a.cancel.load(Ordering::Acquire));
    let report = report_of(stores, operation_id)?;
    let counts = match &report {
        Some(r) => Counts { succeeded: r.succeeded, skipped: r.skipped, failed: r.failed, not_started: r.not_started, unknown: r.unknown, in_flight: 0 },
        None => journal_counts(stores, operation_id),
    };
    let started = stores.audit.operations_page(cm_store::index::MAX_PAGE, None).ok().and_then(|(rows, _)| rows.into_iter().find(|r| r.operation_id == operation_id));
    let uninstall_apps = if kind == "uninstall" {
        stores.audit.load_plan_v2(&plan_id).map(|p| cm_app::uninstall::app_outcomes(&p, report.as_ref())).unwrap_or_default()
    } else {
        vec![]
    };
    Ok(OperationDto {
        uninstall_apps,
        operation_id: operation_id.into(),
        plan_id,
        plan_kind: kind,
        mode,
        needs_reconcile: state == "reconciliation_required" || report.as_ref().is_some_and(|r| r.unknown > 0),
        state,
        running_here: active.is_some(),
        cancel_requested: active.unwrap_or(false),
        started_at: report.as_ref().map(|r| r.started_at.clone()).or_else(|| started.as_ref().map(|r| r.started_at.clone())),
        finished_at: report.as_ref().map(|r| r.finished_at.clone()).or_else(|| started.and_then(|r| r.finished_at)),
        authorization_source: report.as_ref().map(|r| r.authorization_source.clone()),
        total,
        counts,
        report_available: report.is_some(),
        processed_logical_bytes: report.as_ref().map(|r| r.processed_logical_bytes.clone()),
        quarantined_logical_bytes: report.as_ref().map(|r| r.quarantined_logical_bytes.clone()),
        estimated_freed_bytes: report.as_ref().map(|r| r.estimated_freed_bytes.clone()),
        volume_free_before_bytes: report.as_ref().and_then(|r| r.volume_free_before_bytes.clone()),
        volume_free_after_bytes: report.as_ref().and_then(|r| r.volume_free_after_bytes.clone()),
        observed_free_delta_bytes: report.as_ref().and_then(|r| r.observed_free_delta_bytes.clone()),
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct OperationItemDto {
    pub action_id: String,
    pub name: String,
    pub path_display: String,
    pub original_path_display: Option<String>,
    pub logical_bytes: Option<String>,
    /// `succeeded`, `skipped`, `failed`, `not_started`, `unknown`, `in_flight` or `pending`.
    pub status: String,
    pub code: Option<String>,
    pub message: Option<String>,
    pub recovery: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OperationItemsDto {
    pub operation_id: String,
    pub total: u64,
    pub items: Vec<OperationItemDto>,
    pub next_cursor: Option<String>,
}

fn status_word(s: ActionStatus) -> String {
    serde_json::to_value(s).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

/// Every planned action with its recorded outcome (or `pending`/`in_flight`), paged. `filter`
/// `problems` keeps everything that did not succeed.
pub fn operation_items(stores: &Stores, operation_id: &str, filter: &str, page_size: u32, cursor: Option<&str>) -> IpcResult<OperationItemsDto> {
    let size = args::page_size(Some(page_size))? as usize;
    let start = args::offset_cursor(cursor)?;
    let plan_id = stores.audit.operation_plan(operation_id)?;
    // (action id, name, path, original path, bytes) of every planned item, in plan order.
    let contract = stores.audit.plan_contract(&plan_id)?;
    let planned: Vec<(String, String, String, Option<String>, String)> = if contract == PLAN_V2_CONTRACT {
        crate::v2::item_rows(&stores.audit.load_plan_v2(&plan_id)?)
            .into_iter()
            .map(|r| (r.action_id, r.name, r.path_display, r.original_path_display, r.logical_bytes))
            .collect()
    } else if contract == RECOVERY_CONTRACT {
        stores.audit.load_recovery_plan(&plan_id)?.body.items.iter().map(|i| {
            let o = i.original_path.display();
            (i.action_id.clone(), last_component(&o).to_owned(), i.quarantined_path.display(), Some(o), i.logical_bytes.to_string())
        }).collect()
    } else {
        stores.audit.load_plan(&plan_id)?.body.items.iter().map(|i| {
            let d = i.path.display();
            (i.action_id.clone(), last_component(&d).to_owned(), d, None, i.logical_bytes.to_string())
        }).collect()
    };
    let report = report_of(stores, operation_id)?;
    let journal = stores.audit.journal(operation_id)?;
    let result_of = |id: &str| -> (String, Option<ActionResult>) {
        if let Some(r) = report.as_ref().and_then(|r| r.items.iter().find(|x| x.action_id == id)) {
            return (status_word(r.status), Some(r.clone()));
        }
        match journal.iter().find(|j| j.action_id == id) {
            Some(j) => match j.result_json.as_deref().and_then(|t| serde_json::from_str::<ActionResult>(t).ok()) {
                Some(r) => (status_word(r.status), Some(r)),
                None => ("in_flight".into(), None),
            },
            None => ("pending".into(), None),
        }
    };
    let mut rows: Vec<OperationItemDto> = Vec::new();
    let mut matched = 0usize;
    let mut more = false;
    for (id, name, path, orig, bytes) in planned {
        let (status, r) = result_of(&id);
        let keep = match filter {
            "problems" => status != "succeeded",
            "all" => true,
            other => status == other,
        };
        if !keep {
            continue;
        }
        if matched >= start && rows.len() < size {
            rows.push(OperationItemDto {
                action_id: id,
                name,
                path_display: path,
                original_path_display: orig,
                logical_bytes: Some(bytes),
                status,
                code: r.as_ref().map(|r| r.code.clone()),
                message: r.as_ref().map(|r| r.message.clone()),
                recovery: r.as_ref().map(|r| r.recovery.clone()),
            });
        } else if rows.len() >= size {
            more = true;
        }
        matched += 1;
    }
    Ok(OperationItemsDto { operation_id: operation_id.into(), total: matched as u64, items: rows, next_cursor: more.then(|| format!("o{}", start + size)) })
}

fn reconcile(stores: &Stores, operation_id: &str) -> IpcResult<OperationReport> {
    reconcile_impl(stores, operation_id).map_err(IpcError::from)
}

fn reconcile_impl(stores: &Stores, operation_id: &str) -> Result<OperationReport, ApplyError> {
    let principal = current_principal();
    let host = OsHostProbe.current("desktop");
    let plan = stores.audit.operation_plan(operation_id)?;
    match stores.audit.plan_contract(&plan).ok().as_deref() {
        Some(RECOVERY_CONTRACT) => {
            let ctx = cm_app::recovery::RecoveryContext { principal: &principal, host: &host, now: &now_unix, cancel: &|| false, fault: &|_, _| false };
            return cm_app::recovery::reconcile_recovery_operation(stores, &crate::plat::recovery_actions(), operation_id, &ctx);
        }
        Some(PLAN_V2_CONTRACT) => return crate::v2::reconcile(stores, operation_id),
        _ => {}
    }
    // Reconciliation only inspects; it never starts a new action, so protection is not consulted.
    let ctx = cm_app::execute::ApplyContext {
        principal: &principal,
        host: &host,
        current_rule_set_digest: "",
        now: &now_unix,
        cancel: &|| false,
        fault: &|_, _| false,
        protect: &|_| Ok(()),
    };
    cm_app::execute::reconcile_operation(stores, &crate::plat::file_actions(&stores.dir), operation_id, &ctx)
}

// ---------------------------------------------------------------------------------------------
// History and plans (P08)

#[derive(Debug, Clone, Serialize)]
pub struct HistoryItemDto {
    pub operation_id: String,
    pub plan_id: String,
    pub contract: Option<String>,
    /// `trash`, `quarantine`, `delete`, `restore` or `purge`.
    pub action: Option<String>,
    pub state: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub authorization_source: Option<String>,
    pub rule_refs: Vec<String>,
    pub counts: Option<Counts>,
    pub processed_logical_bytes: Option<String>,
    /// `in_app_restore`, `system_recycle_bin`, `unavailable`, `not_applicable` or `unknown`.
    pub availability: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryPageDto {
    pub items: Vec<HistoryItemDto>,
    pub next_cursor: Option<String>,
}

pub fn history(stores: &Stores, page_size: u32, cursor: Option<&str>) -> IpcResult<HistoryPageDto> {
    let (items, next) = cm_app::recovery::operation_history(stores, page_size, cursor)?;
    Ok(HistoryPageDto {
        items: items
            .into_iter()
            .map(|h| HistoryItemDto {
                // plan/v2 kinds without a file action word (uninstall, update, process) are named by their kind.
                action: h.action.clone().or_else(|| {
                    (h.contract.as_deref() == Some(PLAN_V2_CONTRACT)).then(|| stores.audit.load_plan_v2(&h.row.plan_id).ok().map(|p| crate::v2::kind_name(&p).to_owned())).flatten()
                }),
                operation_id: h.row.operation_id,
                plan_id: h.row.plan_id,
                contract: h.contract,
                state: h.row.state,
                started_at: h.row.started_at,
                finished_at: h.row.finished_at,
                authorization_source: h.report.as_ref().map(|r| r.authorization_source.clone()),
                rule_refs: h.rule_refs,
                counts: h.report.as_ref().map(|r| Counts { succeeded: r.succeeded, skipped: r.skipped, failed: r.failed, not_started: r.not_started, unknown: r.unknown, in_flight: 0 }),
                processed_logical_bytes: h.report.as_ref().map(|r| r.processed_logical_bytes.clone()),
                availability: h.availability,
            })
            .collect(),
        next_cursor: next,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanRowDto {
    pub plan_id: String,
    pub contract: String,
    pub kind: &'static str,
    pub mode: Option<Mode>,
    pub item_count: Option<u64>,
    pub created_at: String,
    pub expires_at: String,
    pub expired: bool,
    pub authorization: Option<&'static str>,
    pub operation_id: Option<String>,
    /// The stored plan cannot be read (damaged or from an incompatible version).
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlansPageDto {
    pub items: Vec<PlanRowDto>,
    pub next_cursor: Option<String>,
}

pub fn plans(stores: &Stores, page_size: u32, cursor: Option<&str>) -> IpcResult<PlansPageDto> {
    let now = now_unix();
    let (rows, next) = stores.audit.plans_page(page_size, cursor)?;
    let items = rows
        .into_iter()
        .map(|r| {
            let expired = cm_core::plan::parse_utc(&r.expires_at).is_none_or(|e| now >= e);
            let operation_id = stores.audit.operation_for_dedupe(&format!("{}#{}", r.plan_id, r.plan_digest)).ok().flatten().map(|(op, _)| op);
            let (kind, mode, count, auth, problem) = if r.contract == PLAN_V2_CONTRACT {
                match stores.audit.load_plan_v2(&r.plan_id) {
                    Ok(p) => {
                        let a = crate::v2::authorization(stores, &p, now).ok().map(|a| a.status);
                        (crate::v2::kind_name(&p), crate::v2::mode_of(&p), Some(p.body.items.len() as u64), a, None)
                    }
                    Err(e) => ("unknown", None, None, None, Some(e.to_string())),
                }
            } else if r.contract == RECOVERY_CONTRACT {
                match stores.audit.load_recovery_plan(&r.plan_id) {
                    Ok(p) => {
                        let a = crate::cleanup::recovery_authorization(stores, &p, now).ok().map(|a| a.status);
                        let k = if p.body.recovery_kind == RecoveryKind::Restore { "restore" } else { "purge" };
                        (k, None, Some(p.body.items.len() as u64), a, None)
                    }
                    Err(e) => ("restore", None, None, None, Some(e.to_string())),
                }
            } else {
                match stores.audit.load_plan(&r.plan_id) {
                    Ok(p) => {
                        let a = crate::cleanup::cleanup_authorization(stores, &p, now).ok().map(|a| a.status);
                        ("cleanup", Some(p.body.mode), Some(p.body.items.len() as u64), a, None)
                    }
                    Err(e) => ("cleanup", None, None, None, Some(e.to_string())),
                }
            };
            PlanRowDto {
                plan_id: r.plan_id,
                contract: r.contract,
                kind,
                mode,
                item_count: count,
                created_at: r.created_at,
                expires_at: r.expires_at,
                expired,
                authorization: auth,
                operation_id,
                problem,
            }
        })
        .collect();
    Ok(PlansPageDto { items, next_cursor: next })
}

// ---------------------------------------------------------------------------------------------
// Quarantine (P09)

#[derive(Debug, Clone, Serialize)]
pub struct QuarantineItemDto {
    pub quarantine_id: String,
    pub plan_id: String,
    pub operation_id: String,
    pub name: String,
    pub original_path_display: String,
    pub location_display: String,
    pub logical_bytes: String,
    pub quarantined_at: String,
    pub reminder_at: String,
    pub reminder_due: bool,
    /// Live state: `recoverable`, `target_conflict`, `volume_offline`, `missing`...
    pub state: String,
    /// A restore/purge plan can be created for it (verified manifest and platform actions).
    pub actionable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuarantinePageDto {
    pub items: Vec<QuarantineItemDto>,
    pub next_cursor: Option<String>,
    /// Totals over every item still held (first page only).
    pub held_count: Option<u64>,
    pub held_logical_bytes: Option<String>,
    pub metadata_preserved: Vec<&'static str>,
    pub metadata_limits: Vec<&'static str>,
}

fn held_totals(stores: &Stores) -> IpcResult<(u64, u128)> {
    let mut count = 0u64;
    let mut bytes = 0u128;
    let mut cursor: Option<String> = None;
    loop {
        let (rows, next) = stores.audit.quarantine_page(cm_store::index::MAX_PAGE, cursor.as_deref())?;
        for r in &rows {
            count += 1;
            let v: serde_json::Value = serde_json::from_str(&r.record_json).unwrap_or_default();
            bytes += v["logical_bytes"].as_str().and_then(|s| s.parse::<u64>().ok()).or_else(|| v["logical_bytes"].as_u64()).map(u128::from).unwrap_or(0);
        }
        match next {
            Some(n) => cursor = Some(n),
            None => return Ok((count, bytes)),
        }
    }
}

pub fn quarantine(stores: &Stores, page_size: u32, cursor: Option<&str>) -> IpcResult<QuarantinePageDto> {
    use cm_core::recovery::RecoveryActions;
    let actions = crate::plat::recovery_actions();
    let (items, next) = cm_app::recovery::quarantine_page(stores, &actions, page_size, cursor, now_unix())?;
    let totals = if cursor.is_none() { Some(held_totals(stores)?) } else { None };
    let coverage = actions.metadata_coverage();
    Ok(QuarantinePageDto {
        items: items
            .into_iter()
            .map(|v| {
                let state = serde_json::to_value(&v.state).ok().and_then(|x| x.as_str().map(str::to_owned)).unwrap_or_default();
                let orig = v.original_path.display();
                QuarantineItemDto {
                    quarantine_id: v.entry.quarantine_id,
                    plan_id: v.entry.plan_id,
                    operation_id: v.entry.operation_id,
                    name: last_component(&orig).to_owned(),
                    original_path_display: orig,
                    location_display: v.location.display(),
                    logical_bytes: v.logical_bytes.to_string(),
                    quarantined_at: v.entry.quarantined_at,
                    reminder_at: v.entry.reminder_at,
                    reminder_due: v.reminder_due,
                    actionable: crate::plat::RESTORE_VERIFIED && v.manifest.is_some(),
                    state,
                }
            })
            .collect(),
        next_cursor: next,
        held_count: totals.map(|t| t.0),
        held_logical_bytes: totals.map(|t| t.1.to_string()),
        metadata_preserved: coverage.preserved,
        metadata_limits: coverage.limits,
    })
}

pub fn recovery_plan(stores: &Stores, kind: RecoveryKind, ids: &[String]) -> IpcResult<crate::cleanup::CreatedPlanDto> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let plan_id = format!("rp_desk_{}_{nanos}", std::process::id());
    cm_app::recovery::create_recovery_plan(stores, kind, ids, &plan_id, now_unix()).map_err(|e| {
        let reason = match &e {
            cm_app::recovery::RecoveryError::NotRecoverable(..) => "not_recoverable",
            cm_app::recovery::RecoveryError::Plan(cm_core::recovery::RecoveryPlanError::ManifestIncomplete(_)) => "manifest_incomplete",
            _ => "plan_refused",
        };
        IpcError::new(e.code(), reason, e.to_string())
    })?;
    Ok(crate::cleanup::CreatedPlanDto::new(plan_id))
}

// ---------------------------------------------------------------------------------------------
// IPC

#[derive(Debug, Clone, Serialize)]
pub struct ActiveApplyDto {
    pub plan_id: String,
    pub operation_id: String,
    pub total: u64,
    pub running: bool,
    pub cancel_requested: bool,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn start_apply(app: AppHandle, state: St<'_>, plan_id: String) -> IpcResult<ApplyStartDto> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::record_id("plan", &plan_id)?;
        crate::gate::require_plan(&s, &id)?;
        start(&app, &s, &id)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn cancel_apply(state: St<'_>, operation_id: String) -> IpcResult<bool> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::record_id("operation", &operation_id)?;
        let guard = lock(&s.apply);
        match guard.as_ref() {
            Some(a) if a.operation_id == id && !a.done.load(Ordering::Acquire) => {
                a.cancel.store(true, Ordering::Release);
                Ok(true)
            }
            _ => Ok(false),
        }
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn active_apply(state: St<'_>) -> IpcResult<Option<ActiveApplyDto>> {
    let s = state.inner().clone();
    blocking(move || {
        Ok(lock(&s.apply).as_ref().map(|a| ActiveApplyDto {
            plan_id: a.plan_id.clone(),
            operation_id: a.operation_id.clone(),
            total: a.total,
            running: !a.done.load(Ordering::Acquire),
            cancel_requested: a.cancel.load(Ordering::Acquire),
        }))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn operation_detail(state: St<'_>, operation_id: String) -> IpcResult<OperationDto> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::record_id("operation", &operation_id)?;
        s.with_stores(|st| operation(&s, st, &id))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_operation_items(state: St<'_>, operation_id: String, filter: String, page_size: Option<u32>, cursor: Option<String>) -> IpcResult<OperationItemsDto> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::record_id("operation", &operation_id)?;
        if !["all", "problems", "succeeded", "skipped", "failed", "not_started", "unknown"].contains(&filter.as_str()) {
            return Err(ipc(ErrorCode::InvalidArgument, "invalid_argument", "unknown item filter"));
        }
        let size = args::page_size(page_size)?;
        s.with_stores(|st| operation_items(st, &id, &filter, size, cursor.as_deref()))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn reconcile_operation(state: St<'_>, operation_id: String) -> IpcResult<OperationDto> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::record_id("operation", &operation_id)?;
        if lock(&s.apply).as_ref().is_some_and(|a| a.operation_id == id && !a.done.load(Ordering::Acquire)) {
            return Err(ipc(ErrorCode::Conflict, "operation_running", "The operation is still running in this window"));
        }
        s.with_stores(|st| {
            reconcile(st, &id)?;
            operation(&s, st, &id)
        })
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_operations(state: St<'_>, page_size: Option<u32>, cursor: Option<String>) -> IpcResult<HistoryPageDto> {
    let s = state.inner().clone();
    blocking(move || {
        let size = args::page_size(page_size)?;
        let cursor = args::cursor(cursor.as_deref())?;
        s.with_stores(|st| {
            let _ = st.audit.reconcile_hosts(|h| OsHostProbe.liveness(h));
            history(st, size, cursor.as_deref())
        })
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_plans(state: St<'_>, page_size: Option<u32>, cursor: Option<String>) -> IpcResult<PlansPageDto> {
    let s = state.inner().clone();
    blocking(move || {
        let size = args::page_size(page_size)?;
        let cursor = args::cursor(cursor.as_deref())?;
        s.with_stores(|st| plans(st, size, cursor.as_deref()))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_quarantine(state: St<'_>, page_size: Option<u32>, cursor: Option<String>) -> IpcResult<QuarantinePageDto> {
    let s = state.inner().clone();
    blocking(move || {
        let size = args::page_size(page_size)?;
        let cursor = args::cursor(cursor.as_deref())?;
        s.with_stores(|st| quarantine(st, size, cursor.as_deref()))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn create_recovery_plan(state: St<'_>, kind: String, quarantine_ids: Vec<String>) -> IpcResult<crate::cleanup::CreatedPlanDto> {
    let s = state.inner().clone();
    blocking(move || {
        let kind = match kind.as_str() {
            "restore" => RecoveryKind::Restore,
            "purge" => RecoveryKind::Purge,
            _ => return Err(ipc(ErrorCode::InvalidArgument, "invalid_argument", "kind must be restore or purge")),
        };
        let ids = args::record_ids("quarantine", &quarantine_ids, MAX_RECOVERY_SELECTION)?;
        s.with_stores(|st| recovery_plan(st, kind, &ids))
    })
    .await
}

#[cfg(test)]
pub fn precheck_for_tests(s: &AppState, stores: &Stores, plan_id: &str) -> IpcResult<()> {
    precheck(s, stores, plan_id)
}

#[cfg(test)]
pub fn run_for_tests(stores: &Stores, plan_id: &str, digest: &str, cancel: &AtomicBool) -> Result<OperationReport, ApplyError> {
    run(stores, plan_id, digest, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_errors_keep_their_reason_for_the_ui() {
        let e: IpcError = ApplyError::NeedsApproval("p".into()).into();
        assert_eq!((e.code, e.reason), (ErrorCode::NeedsApproval, "needs_approval"));
        let e: IpcError = ApplyError::Stale(cm_core::plan::StaleReason::RuleSetChanged).into();
        assert_eq!(e.reason, "plan_stale");
        let e: IpcError = ApplyError::ProtectionUnavailable("not valid JSON".into()).into();
        assert_eq!((e.code, e.reason), (ErrorCode::ProtectionUnavailable, "protection_unavailable"));
    }

    /// PRT-02 / BUG-WFA-03: starting an apply is refused while the protection list is unusable, before a thread starts
    /// or anything is recorded (the executor repeats the check).
    #[test]
    fn an_unusable_protection_list_refuses_the_start() {
        let dir = std::env::temp_dir().join(format!("cm-desk-protect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let stores = Stores::open(&dir).unwrap();
        std::fs::write(dir.join(cm_platform::protection::PROTECTION_FILE), b"{not json").unwrap();
        let e: IpcError = cm_app::execute::require_protection(&stores).unwrap_err().into();
        assert_eq!((e.code, e.reason), (ErrorCode::ProtectionUnavailable, "protection_unavailable"));
        drop(stores);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

