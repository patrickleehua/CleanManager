//! Quarantine, recovery and history services (REC-01..05).
//!
//! Recovery plans (`recovery/v1`) run through the same dedupe, identity locks, durable
//! intent/result journal and report shape as cleanup plans, but are a separate plan type with
//! separate receipts: only an interactive review of the exact recovery plan authorizes it, and
//! no policy ever matches one. Restore never overwrites; purge deletes exactly the verified
//! quarantined object. Reaching a retention reminder only flags the item; nothing here deletes
//! on a timer.

use cm_core::bytes::Bytes;
use cm_core::execute::Reconciled;
use cm_core::host::HostRef;
use cm_core::path::RawPath;
use cm_core::plan::parse_utc;
use cm_core::recovery::{
    ManifestProblem, QuarantineManifest, QuarantineState, QuarantinedObject, RecoveryActions, RecoveryKind, RecoveryOutcome, RecoveryPlan,
    RecoveryPlanError, RecoveryReceipt, build_recovery_plan, check_recovery_plan, recovery_receipt_valid,
};
use cm_core::rules::Mode;
use cm_core::time::utc_from_unix;
use cm_store::audit::{OperationRow, QuarantineEntry};
use cm_store::{StoreError, Stores};
use serde::Deserialize;

use crate::execute::{ActionResult, ActionStatus, ApplyError, Authorization, FaultPoint, OperationReport, final_status, summarize};

pub struct RecoveryContext<'a> {
    pub principal: &'a str,
    pub host: &'a HostRef,
    pub now: &'a dyn Fn() -> i64,
    /// Polled before each action; true stops scheduling new actions.
    pub cancel: &'a dyn Fn() -> bool,
    pub fault: &'a dyn Fn(FaultPoint, &str) -> bool,
}

// ---------------------------------------------------------------------------------------------
// Quarantine listing (REC-01/03/04)

/// A quarantined object as shown to people: durable manifest plus its live state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuarantineView {
    pub entry: QuarantineEntry,
    pub original_path: RawPath,
    pub location: RawPath,
    pub logical_bytes: Bytes,
    /// `None` when the manifest cannot verify a restore (shown as `manifest_incomplete`).
    pub manifest: Option<QuarantineManifest>,
    pub state: QuarantineState,
    /// The retention reminder is due: review it. Nothing is deleted because of this.
    pub reminder_due: bool,
}

/// The fields every manifest the executor writes carries, read without the restore checks.
#[derive(Deserialize)]
struct ManifestPaths {
    original_path: RawPath,
    location: RawPath,
    logical_bytes: Bytes,
}

fn view(entry: QuarantineEntry, actions: &dyn RecoveryActions, now: i64) -> Result<QuarantineView, StoreError> {
    let paths: ManifestPaths = serde_json::from_str(&entry.record_json)
        .map_err(|e| StoreError::AuditUnavailable(format!("quarantine record {} is damaged: {e}", entry.quarantine_id)))?;
    let manifest = QuarantineManifest::parse(&entry.record_json).ok();
    let state = match &manifest {
        None => QuarantineState::ManifestIncomplete,
        Some(m) => {
            let parent = m.original_parent.as_ref().expect("parsed manifests carry a parent");
            actions.probe(&QuarantinedObject {
                action_id: &entry.action_id, location: &m.location, identity: &m.original_identity, original_path: &m.original_path,
                original_parent: parent, logical_bytes: m.logical_bytes.0,
            })
        }
    };
    let reminder_due = parse_utc(&entry.reminder_at).is_some_and(|r| now >= r);
    Ok(QuarantineView { original_path: paths.original_path, location: paths.location, logical_bytes: paths.logical_bytes, manifest, state, reminder_due, entry })
}

/// One page of items still held in quarantine, oldest first.
pub fn quarantine_page(stores: &Stores, actions: &dyn RecoveryActions, page_size: u32, cursor: Option<&str>, now: i64) -> Result<(Vec<QuarantineView>, Option<String>), StoreError> {
    let (rows, next) = stores.audit.quarantine_page(page_size, cursor)?;
    let items = rows.into_iter().map(|e| view(e, actions, now)).collect::<Result<_, _>>()?;
    Ok((items, next))
}

pub fn quarantine_show(stores: &Stores, actions: &dyn RecoveryActions, quarantine_id: &str, now: i64) -> Result<QuarantineView, StoreError> {
    view(stores.audit.quarantine_entry(quarantine_id)?, actions, now)
}

// ---------------------------------------------------------------------------------------------
// Plan creation and authorization (REC-02/03)

#[derive(Debug, thiserror::Error)]
pub enum RecoveryError {
    #[error(transparent)]
    Plan(#[from] RecoveryPlanError),
    #[error("quarantine item {0} is {1}, not recoverable")]
    NotRecoverable(String, String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl RecoveryError {
    pub fn code(&self) -> cm_core::error::ErrorCode {
        use cm_core::error::ErrorCode;
        match self {
            RecoveryError::Plan(RecoveryPlanError::ManifestIncomplete(_)) => ErrorCode::Unsupported,
            RecoveryError::Plan(_) => ErrorCode::InvalidArgument,
            RecoveryError::NotRecoverable(..) => ErrorCode::Conflict,
            RecoveryError::Store(e) => e.code(),
        }
    }
}

/// Freeze quarantined items into a new restore or purge plan and store it. Creating a plan
/// authorizes nothing.
pub fn create_recovery_plan(stores: &Stores, kind: RecoveryKind, quarantine_ids: &[String], plan_id: &str, now: i64) -> Result<RecoveryPlan, RecoveryError> {
    let mut selected = Vec::with_capacity(quarantine_ids.len());
    for id in quarantine_ids {
        let entry = stores.audit.quarantine_entry(id)?;
        if entry.state != "recoverable" {
            return Err(RecoveryError::NotRecoverable(id.clone(), entry.state));
        }
        let manifest = match QuarantineManifest::parse(&entry.record_json) {
            Ok(m) => m,
            Err(ManifestProblem::Unreadable(_) | ManifestProblem::NoParentIdentity | ManifestProblem::WeakIdentity) => {
                return Err(RecoveryPlanError::ManifestIncomplete(id.clone()).into());
            }
        };
        selected.push((id.clone(), manifest));
    }
    let plan = build_recovery_plan(kind, &selected, plan_id, now)?;
    stores.audit.save_recovery_plan(&plan)?;
    Ok(plan)
}

/// Authorization state of a recovery plan, computed from stored receipts (never from a client).
pub fn recovery_authorization(stores: &Stores, plan: &RecoveryPlan, principal: &str, now: i64) -> Result<(&'static str, Option<RecoveryReceipt>), StoreError> {
    let receipts = stores.audit.recovery_receipts_for(&plan.body.plan_id)?;
    let valid = receipts.iter().find(|r| recovery_receipt_valid(r, plan, principal, now)).cloned();
    let expired = parse_utc(&plan.body.expires_at).is_none_or(|e| now >= e);
    let status = match (&valid, expired) {
        (Some(_), _) => "authorized",
        (None, true) => "expired",
        (None, false) if receipts.iter().any(|r| r.revoked_at.is_some()) => "revoked",
        (None, false) => "required",
    };
    Ok((status, valid))
}

// ---------------------------------------------------------------------------------------------
// Application (EXE-01..07 semantics for recovery actions)

fn identity_key(i: &cm_core::plan::WireIdentity) -> String {
    format!("{}|{}", i.volume_id, i.file_id)
}

fn result(action_id: &str, status: ActionStatus, code: &str, message: &str, recovery: &str) -> ActionResult {
    ActionResult { action_id: action_id.into(), status, code: code.into(), message: message.into(), recovery: recovery.into() }
}

/// Where the object can be recovered from after this outcome (protocol `action_result.recovery`).
fn outcome_result(kind: RecoveryKind, action_id: &str, outcome: &RecoveryOutcome) -> ActionResult {
    match outcome {
        RecoveryOutcome::Done => match kind {
            RecoveryKind::Restore => result(action_id, ActionStatus::Succeeded, "restored", "restored to its original location", "not_applicable"),
            RecoveryKind::Purge => result(action_id, ActionStatus::Succeeded, "purged", "permanently deleted; there is no undo", "unavailable"),
        },
        // Untouched: the object stays wherever it was (normally still in quarantine).
        RecoveryOutcome::Skipped { code, message } => {
            result(action_id, ActionStatus::Skipped, code, message, if code == "missing" { "not_applicable" } else { "quarantine" })
        }
        RecoveryOutcome::Failed { code, message } => result(action_id, ActionStatus::Failed, code, message, "quarantine"),
        RecoveryOutcome::Unknown { message } => result(action_id, ActionStatus::Unknown, "outcome_unconfirmed", message, "unknown"),
    }
}

fn record(stores: &Stores, operation_id: &str, r: &ActionResult, now: i64) -> Result<(), StoreError> {
    let status = serde_json::to_value(r.status).expect("status serializes");
    stores.audit.record_result(operation_id, &r.action_id, status.as_str().expect("status is a string"), &serde_json::to_string(r).expect("result serializes"), &utc_from_unix(now))
}

fn settled_state(kind: RecoveryKind) -> &'static str {
    match kind {
        RecoveryKind::Restore => "restored",
        RecoveryKind::Purge => "purged",
    }
}

/// Apply a stored recovery plan. Returns the final report, or an error before anything was touched.
pub fn apply_recovery_plan(stores: &Stores, actions: &dyn RecoveryActions, plan_id: &str, ctx: &RecoveryContext<'_>) -> Result<OperationReport, ApplyError> {
    let plan = stores.audit.load_recovery_plan(plan_id)?;
    let dedupe_key = format!("{}#{}", plan.body.plan_id, plan.digest);
    // EXE-03: the same plan never produces a second operation.
    if let Some((op, _)) = stores.audit.operation_for_dedupe(&dedupe_key)? {
        if let Some(json) = stores.audit.operation_report(&op)? {
            return serde_json::from_str(&json).map_err(|e| ApplyError::Store(StoreError::AuditUnavailable(e.to_string())));
        }
        return Err(ApplyError::Conflict(op));
    }
    check_recovery_plan(&plan, (ctx.now)()).map_err(ApplyError::Stale)?;
    let authorized = |now: i64| -> Result<bool, StoreError> {
        Ok(stores.audit.recovery_receipts_for(plan_id)?.iter().any(|r| recovery_receipt_valid(r, &plan, ctx.principal, now)))
    };
    if !authorized((ctx.now)())? {
        return Err(ApplyError::NeedsApproval(plan_id.into()));
    }
    let operation_id = format!("op_{}", &plan.digest[..16]);
    let identities: Vec<String> = plan.body.items.iter().map(|i| identity_key(&i.quarantined_identity)).collect();
    if let Some(holder) = stores.audit.lock_identities(&operation_id, &identities)? {
        return Err(ApplyError::Conflict(holder));
    }
    let started_at = utc_from_unix((ctx.now)());
    stores.audit.begin_operation(&operation_id, plan_id, &plan.digest, &dedupe_key, ctx.host, &started_at)?;
    stores.audit.set_operation_state(&operation_id, "running")?;
    let free_before = plan.body.items.first().and_then(|i| actions.free_space(&i.quarantined_path));
    let kind = plan.body.recovery_kind;
    let expires = parse_utc(&plan.body.expires_at).unwrap_or(i64::MIN);
    let mut results = Vec::with_capacity(plan.body.items.len());
    let mut cancelled = false;
    for item in &plan.body.items {
        let id = item.action_id.as_str();
        // EXE-06: cancellation, expiry and revocation stop new actions only.
        if cancelled || (ctx.cancel)() {
            cancelled = true;
            results.push(result(id, ActionStatus::NotStarted, "cancelled", "operation cancelled before this action started", "quarantine"));
            continue;
        }
        if (ctx.now)() >= expires {
            results.push(result(id, ActionStatus::NotStarted, "plan_expired", "plan expired before this action started", "quarantine"));
            continue;
        }
        if !authorized((ctx.now)())? {
            results.push(result(id, ActionStatus::NotStarted, "authorization_revoked", "authorization was revoked or expired", "quarantine"));
            continue;
        }
        // Another recovery plan may have settled this item since planning.
        let entry = stores.audit.quarantine_entry(&item.quarantine_id)?;
        if entry.state != "recoverable" {
            let r = result(id, ActionStatus::Skipped, "not_recoverable", &format!("quarantine item is already {}", entry.state), "not_applicable");
            record(stores, &operation_id, &r, (ctx.now)())?;
            results.push(r);
            continue;
        }
        let intent = serde_json::json!({"recovery_kind": kind, "quarantine_id": item.quarantine_id, "location": item.quarantined_path,
            "identity": item.quarantined_identity, "original_path": item.original_path});
        stores.audit.record_intent(&operation_id, id, &intent.to_string(), &utc_from_unix((ctx.now)()))?;
        if (ctx.fault)(FaultPoint::AfterIntent, id) {
            return Err(ApplyError::Crashed("after_intent".into()));
        }
        let object = QuarantinedObject::from_item(item);
        let outcome = match kind {
            RecoveryKind::Restore => actions.restore(&object),
            RecoveryKind::Purge => actions.purge(&object),
        };
        if (ctx.fault)(FaultPoint::AfterAction, id) {
            return Err(ApplyError::Crashed("after_action".into()));
        }
        if outcome == RecoveryOutcome::Done {
            stores.audit.set_quarantine_state(&item.quarantine_id, settled_state(kind))?;
        }
        let r = outcome_result(kind, id, &outcome);
        record(stores, &operation_id, &r, (ctx.now)())?;
        results.push(r);
    }
    let auth = Authorization { source: "interactive_review", reconciled_at: None };
    finish(stores, actions, &plan, &operation_id, &started_at, results, cancelled, free_before, &auth, ctx)
}

#[allow(clippy::too_many_arguments)]
fn finish(
    stores: &Stores, actions: &dyn RecoveryActions, plan: &RecoveryPlan, operation_id: &str, started_at: &str, results: Vec<ActionResult>,
    cancelled: bool, free_before: Option<u64>, auth: &Authorization<'_>, ctx: &RecoveryContext<'_>,
) -> Result<OperationReport, ApplyError> {
    for r in results.iter().filter(|r| r.status == ActionStatus::NotStarted) {
        record(stores, operation_id, r, (ctx.now)())?;
    }
    let free_after = plan.body.items.first().and_then(|i| actions.free_space(&i.quarantined_path));
    let (ok, skipped, failed, not_started, unknown) = summarize(&results);
    let size_of = |id: &str| plan.body.items.iter().find(|i| i.action_id == id).map(|i| i.logical_bytes.0).unwrap_or(0);
    let processed: u64 = results.iter().filter(|r| r.status == ActionStatus::Succeeded).map(|r| size_of(&r.action_id)).sum();
    let report = OperationReport {
        operation_id: operation_id.into(),
        plan_id: plan.body.plan_id.clone(),
        plan_digest: plan.digest.clone(),
        status: final_status(&results, cancelled).into(),
        started_at: started_at.into(),
        finished_at: utc_from_unix((ctx.now)()),
        succeeded: ok,
        skipped,
        failed,
        not_started,
        unknown,
        items: results,
        processed_logical_bytes: processed.to_string(),
        quarantined_logical_bytes: "0".into(),
        estimated_freed_bytes: if plan.body.recovery_kind == RecoveryKind::Purge { processed.to_string() } else { "0".into() },
        volume_free_before_bytes: free_before.map(|b| b.to_string()),
        volume_free_after_bytes: free_before.and(free_after).map(|b| b.to_string()),
        observed_free_delta_bytes: free_before.zip(free_after).map(|(b, a)| (i128::from(a) - i128::from(b)).to_string()),
        authorization_source: auth.source.into(),
        reconciled_at: auth.reconciled_at.clone(),
    };
    // Locks stay while outcomes are unknown so no other plan touches those objects (EXE-04/05).
    if unknown == 0 {
        stores.audit.release_locks(operation_id)?;
    }
    stores.audit.finish_operation(operation_id, &report.status, &serde_json::to_string(&report).expect("report serializes"), &report.finished_at)?;
    Ok(report)
}

/// Resolve recovery actions that have an intent but no result after the host stopped. Never
/// performs an action.
pub fn reconcile_recovery_operation(stores: &Stores, actions: &dyn RecoveryActions, operation_id: &str, ctx: &RecoveryContext<'_>) -> Result<OperationReport, ApplyError> {
    let plan = stores.audit.load_recovery_plan(&stores.audit.operation_plan(operation_id)?)?;
    let journal = stores.audit.journal(operation_id)?;
    let kind = plan.body.recovery_kind;
    let prior = crate::execute::prior_report(stores, operation_id)?;
    let mut resolved = false;
    let mut results = Vec::with_capacity(plan.body.items.len());
    for item in &plan.body.items {
        let id = item.action_id.as_str();
        let row = journal.iter().find(|r| r.action_id == item.action_id);
        let existing: Option<ActionResult> = row.and_then(|r| r.result_json.as_deref()).and_then(|j| serde_json::from_str(j).ok());
        let r = match (row, existing) {
            (_, Some(r)) if r.status != ActionStatus::Unknown => r,
            (None, _) => result(id, ActionStatus::NotStarted, "never_started", "no action was started", "quarantine"),
            (Some(_), _) => {
                resolved = true;
                let r = match actions.reconcile(&QuarantinedObject::from_item(item), kind) {
                    Reconciled::NotPerformed => result(id, ActionStatus::NotStarted, "reconciled_not_performed", "object still in quarantine", "quarantine"),
                    Reconciled::Performed => {
                        stores.audit.set_quarantine_state(&item.quarantine_id, settled_state(kind))?;
                        outcome_result(kind, id, &RecoveryOutcome::Done)
                    }
                    Reconciled::StillUnknown => result(id, ActionStatus::Unknown, "outcome_unconfirmed", "evidence insufficient; needs a person to check", "unknown"),
                };
                record(stores, operation_id, &r, (ctx.now)())?;
                r
            }
        };
        results.push(r);
    }
    // Nothing was unconfirmed: the recorded report stands as it is.
    if let Some(report) = prior.as_ref().filter(|_| !resolved) {
        return Ok(report.clone());
    }
    let now = utc_from_unix((ctx.now)());
    let started = match &prior {
        Some(r) => r.started_at.clone(),
        None => stores.audit.operation_times(operation_id).map(|(s, _)| s).unwrap_or_else(|_| now.clone()),
    };
    // Recovery plans are authorized only by an interactive review; a reconciliation never replaces that (WFA-40).
    let source = prior.as_ref().map(|r| r.authorization_source.clone()).filter(|s| s != crate::execute::LEGACY_RECONCILIATION_SOURCE)
        .unwrap_or_else(|| "interactive_review".into());
    let reconciled_at = if resolved { Some(now) } else { prior.as_ref().and_then(|r| r.reconciled_at.clone()) };
    let auth = Authorization { source: &source, reconciled_at };
    finish(stores, actions, &plan, operation_id, &started, results, false, None, &auth, ctx)
}

// ---------------------------------------------------------------------------------------------
// History (REC-04)

/// One operation in the history with truthful recovery availability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryItem {
    pub row: OperationRow,
    /// Authoritative plan type (`plan/v1`, `plan/v2`, `recovery/v1`), if the plan is still readable.
    pub contract: Option<String>,
    /// `trash`, `quarantine`, `delete`, `restore`, `purge` (cleanup and quarantine recovery), or a `plan/v2` kind's word
    /// ([`crate::plans::history_facts`]): `startup`, `maintenance`, `process`, `update`, `uninstall`,
    /// `uninstall_leftovers`, `uninstall_restore`, `startup_restore`, `maintenance_restore`.
    pub action: Option<String>,
    pub rule_refs: Vec<String>,
    /// The recorded report. An operation whose host stopped before its report (`reconciliation_required`) carries the
    /// provisional report `operations show` prints ([`crate::execute::interrupted_report`]): who authorized it and
    /// what the journal knows so far, nothing resolved (WFA-40).
    pub report: Option<OperationReport>,
    /// `in_app_restore`, `system_recycle_bin`, `unavailable`, `not_applicable` or `unknown`.
    pub availability: &'static str,
    pub guidance: String,
}

fn availability(item: &HistoryItem, stores: &Stores) -> Result<(&'static str, String), StoreError> {
    let op = &item.row.operation_id;
    let Some(report) = &item.report else {
        return Ok(match item.row.state.as_str() {
            "reconciliation_required" => ("unknown", format!("Outcome unconfirmed; run `cleanmanager operations reconcile {op}` before deciding anything.")),
            _ => ("unknown", "Operation has not finished; query it again with `cleanmanager operations show`.".to_string()),
        });
    };
    if report.unknown > 0 {
        return Ok(("unknown", format!("Some outcomes are unconfirmed; run `cleanmanager operations reconcile {op}` first.")));
    }
    if report.succeeded == 0 {
        return Ok(("not_applicable", "Nothing was changed by this operation.".into()));
    }
    // plan/v2 kinds describe their own recovery (manual trash → Recycle Bin, startup → recorded prior state, process end
    // and uninstallers → no undo, leftovers → their backups and the Recycle Bin…).
    if item.contract.as_deref() == Some(cm_core::plan::v2::PLAN_V2_CONTRACT) {
        return Ok(match crate::plans::history_facts(stores, &item.row.plan_id, op) {
            Some((_, availability, guidance)) => (availability, guidance),
            None => ("unknown", "The plan of this operation cannot be read; recovery availability is unknown.".into()),
        });
    }
    Ok(match item.action.as_deref() {
        Some("quarantine") => {
            if stores.audit.quarantine_for_operation(op)?.iter().any(|q| q.state == "recoverable") {
                ("in_app_restore", "Restorable here: `cleanmanager recovery list`, then `cleanmanager recovery plan create --restore ID`.".into())
            } else {
                ("not_applicable", "Its quarantined items were restored or purged since; see the later operations.".into())
            }
        }
        Some("trash") => (
            "system_recycle_bin",
            if cfg!(target_os = "macos") {
                "Put it back from the Trash (Finder, Trash, File › Put Back); CleanManager cannot restore it.".into()
            } else {
                "Restore from the system Recycle Bin (Explorer, Recycle Bin, Restore); CleanManager cannot restore it.".into()
            },
        ),
        Some("delete") => ("unavailable", "Permanently deleted; there is no undo.".into()),
        Some("purge") => ("unavailable", "Purged from quarantine permanently; there is no undo.".into()),
        Some("restore") => ("not_applicable", "Objects were moved back to their original locations.".into()),
        _ => ("unknown", "The plan of this operation cannot be read; recovery availability is unknown.".into()),
    })
}

/// One page of operation history, newest first.
pub fn operation_history(stores: &Stores, page_size: u32, cursor: Option<&str>) -> Result<(Vec<HistoryItem>, Option<String>), StoreError> {
    let (rows, next) = stores.audit.operations_page(page_size, cursor)?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let contract = stores.audit.plan_contract(&row.plan_id).ok();
        let (action, rule_refs) = match contract.as_deref() {
            Some("plan/v1") => match stores.audit.load_plan(&row.plan_id) {
                Ok(p) => (Some(match p.body.mode { Mode::Trash => "trash", Mode::Quarantine => "quarantine", Mode::Delete => "delete" }.to_string()), p.rule_refs),
                Err(_) => (None, vec![]),
            },
            Some(cm_core::plan::v2::PLAN_V2_CONTRACT) => (crate::plans::history_facts(stores, &row.plan_id, &row.operation_id).map(|f| f.0.to_string()), vec![]),
            Some(cm_core::recovery::RECOVERY_CONTRACT) => {
                let kind = stores.audit.load_recovery_plan(&row.plan_id).ok().map(|p| settled_state(p.body.recovery_kind));
                (kind.map(|k| if k == "restored" { "restore".to_string() } else { "purge".to_string() }), vec![])
            }
            _ => (None, vec![]),
        };
        let report = stores.audit.operation_report(&row.operation_id)?.and_then(|j| serde_json::from_str::<OperationReport>(&j).ok());
        let mut item = HistoryItem { row, contract, action, rule_refs, report, availability: "unknown", guidance: String::new() };
        (item.availability, item.guidance) = availability(&item, stores)?;
        // WFA-40: an interrupted operation still lists its authorization source and the counts known so far (its
        // recovery stays `unknown` until it is reconciled, decided above from the missing report).
        if item.report.is_none() && item.row.state == "reconciliation_required" {
            item.report = crate::execute::interrupted_report(stores, &item.row.operation_id).ok();
        }
        out.push(item);
    }
    Ok((out, next))
}

#[cfg(test)]
mod tests;
