//! Plan application (EXE-01..08, PLN-03..06, REC-01/05).
//!
//! Sequence per operation: plan validity → authorization (receipt or policy + atomic quota) →
//! single operation per plan (dedupe) → identity locks across plans → for each planned file:
//! stop checks (cancel, expiry, revocation) → durable intent → live check → OS action → durable
//! result. Unknown outcomes put the operation into `reconciliation_required`; nothing is ever
//! retried or rolled back automatically.
//!
//! Directory items (project artifacts) run after every file item, deepest first, and only when
//! everything planned inside them succeeded; the platform then removes each one only if it is
//! the planned object and empty at that moment. In trash mode a planned directory whose live
//! subtree still equals the plan may first go to the Recycle Bin as one object; any refusal
//! falls back to leaves + empty-directory removal.

use serde::{Deserialize, Serialize};

use cm_core::authz::{Policy, Receipt, ReceiptSource, policy_covers, receipt_valid};
use cm_core::execute::{ActionOutcome, ActionTarget, FileActions, LiveCheck, Reconciled, Recovery, TreeMember};
use cm_core::host::HostRef;
use cm_core::path::RawPath;
use cm_core::plan::{ItemKind, Plan, StaleReason, check_plan, parse_utc};
use cm_core::rules::Mode;
use cm_core::time::utc_from_unix;
use cm_store::{StoreError, Stores};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus {
    Succeeded,
    Skipped,
    Failed,
    NotStarted,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionResult {
    pub action_id: String,
    pub status: ActionStatus,
    pub code: String,
    pub message: String,
    /// `trash`, `quarantine`, `unavailable`, `not_applicable` or `unknown` (protocol enum).
    pub recovery: String,
}

/// Protocol `operation_report` content (without the record envelope).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationReport {
    pub operation_id: String,
    pub plan_id: String,
    pub plan_digest: String,
    pub status: String,
    pub started_at: String,
    pub finished_at: String,
    pub succeeded: u64,
    pub skipped: u64,
    pub failed: u64,
    pub not_started: u64,
    pub unknown: u64,
    pub items: Vec<ActionResult>,
    pub processed_logical_bytes: String,
    /// Logical bytes now held in a same-volume quarantine: moved, not freed (REC-05).
    #[serde(default = "zero_bytes")]
    pub quarantined_logical_bytes: String,
    /// What the succeeded actions free by themselves (logical estimate): permanent deletes and
    /// purges only; quarantine, Recycle Bin and restore moves free nothing on the volume.
    #[serde(default = "zero_bytes")]
    pub estimated_freed_bytes: String,
    pub volume_free_before_bytes: Option<String>,
    pub volume_free_after_bytes: Option<String>,
    pub observed_free_delta_bytes: Option<String>,
    pub authorization_source: String,
}

pub(crate) fn zero_bytes() -> String {
    "0".into()
}

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("plan {0} needs approval")]
    NeedsApproval(String),
    #[error("plan is stale: {0:?}")]
    Stale(StaleReason),
    #[error("policy denied: {0}")]
    PolicyDenied(String),
    #[error("conflict with operation {0}")]
    Conflict(String),
    #[error("simulated crash at {0}")]
    Crashed(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl ApplyError {
    pub fn code(&self) -> cm_core::error::ErrorCode {
        use cm_core::error::ErrorCode;
        match self {
            ApplyError::NeedsApproval(_) => ErrorCode::NeedsApproval,
            ApplyError::PolicyDenied(_) => ErrorCode::PolicyDenied,
            ApplyError::Stale(_) => ErrorCode::PlanStale,
            ApplyError::Conflict(_) => ErrorCode::Conflict,
            ApplyError::Crashed(_) => ErrorCode::InternalError,
            ApplyError::Store(e) => e.code(),
        }
    }
}

/// Where a test may simulate a crash (EXE-05 fault injection).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultPoint {
    AfterIntent,
    AfterAction,
}

pub struct ApplyContext<'a> {
    pub principal: &'a str,
    pub host: &'a HostRef,
    pub current_rule_set_digest: &'a str,
    pub now: &'a dyn Fn() -> i64,
    /// Polled before each action; true stops scheduling new actions.
    pub cancel: &'a dyn Fn() -> bool,
    pub fault: &'a dyn Fn(FaultPoint, &str) -> bool,
    /// Protection re-check immediately before each action (PRT-01/EXE-01): `Err(reason)` skips
    /// the item as `protected`. Implementations must fail closed when protection is unavailable.
    pub protect: &'a dyn Fn(&ActionTarget<'_>) -> Result<(), String>,
}

fn identity_key(i: &cm_core::plan::WireIdentity) -> String {
    format!("{}|{}", i.volume_id, i.file_id)
}

pub(crate) fn summarize(results: &[ActionResult]) -> (u64, u64, u64, u64, u64) {
    let c = |s| results.iter().filter(|r| r.status == s).count() as u64;
    (c(ActionStatus::Succeeded), c(ActionStatus::Skipped), c(ActionStatus::Failed), c(ActionStatus::NotStarted), c(ActionStatus::Unknown))
}

pub(crate) fn final_status(results: &[ActionResult], cancelled: bool) -> &'static str {
    let (ok, skipped, failed, _, unknown) = summarize(results);
    if unknown > 0 {
        "reconciliation_required"
    } else if cancelled {
        "cancelled"
    } else if ok as usize == results.len() {
        "completed"
    } else if ok > 0 || (failed == 0 && skipped > 0) {
        // Items skipped by live preconditions (owner running, in use, changed) are an
        // incomplete run, not a system failure.
        "partial"
    } else {
        "failed"
    }
}

/// Protocol messages are never empty (`action_result.message`).
fn done_message(r: &Recovery) -> &'static str {
    match r {
        Recovery::Trash => "moved to the Recycle Bin",
        Recovery::Quarantine { .. } => "moved to the same-volume quarantine",
        Recovery::Unavailable => "permanently deleted",
    }
}

fn recovery_name(r: &Recovery) -> &'static str {
    match r {
        Recovery::Trash => "trash",
        Recovery::Quarantine { .. } => "quarantine",
        Recovery::Unavailable => "unavailable",
    }
}

fn find_authorization(stores: &Stores, plan: &Plan, ctx: &ApplyContext<'_>, operation_id: &str) -> Result<(String, Option<Policy>), ApplyError> {
    let now = (ctx.now)();
    if stores.audit.receipts_for(&plan.body.plan_id)?.iter().any(|r| receipt_valid(r, plan, ctx.principal, now)) {
        return Ok(("interactive_review".into(), None));
    }
    let mut last = None;
    for policy in stores.audit.policies()? {
        match policy_covers(&policy, plan, ctx.principal, now) {
            Ok(()) => {
                let bytes = plan.body.total_logical().map(|b| b.0).unwrap_or(u64::MAX);
                if stores.audit.reserve_quota(&policy, operation_id, &plan.body.plan_id, bytes, plan.body.items.len() as u64, now)? {
                    let receipt = Receipt {
                        receipt_id: format!("r_{operation_id}"),
                        plan_id: plan.body.plan_id.clone(),
                        plan_digest: plan.digest.clone(),
                        mode: plan.body.mode,
                        principal: ctx.principal.into(),
                        source: ReceiptSource::Policy,
                        policy_id: Some(policy.policy_id.clone()),
                        policy_version: Some(policy.version),
                        created_at: utc_from_unix(now),
                        expires_at: plan.body.expires_at.clone(),
                        revoked_at: None,
                    };
                    stores.audit.save_receipt(&receipt)?;
                    return Ok((format!("policy:{}@{}", policy.policy_id, policy.version), Some(policy)));
                }
                return Err(ApplyError::PolicyDenied(format!("policy {} has no remaining quota", policy.policy_id)));
            }
            Err(m) => last = Some(m),
        }
    }
    let _ = last;
    Err(ApplyError::NeedsApproval(plan.body.plan_id.clone()))
}

/// Apply a stored plan. Returns the final report, or an error before anything was touched.
pub fn apply_plan(stores: &Stores, actions: &dyn FileActions, plan_id: &str, ctx: &ApplyContext<'_>) -> Result<OperationReport, ApplyError> {
    let plan = stores.audit.load_plan(plan_id)?;
    let dedupe_key = format!("{}#{}", plan.body.plan_id, plan.digest);
    // EXE-03: the same plan never produces a second operation.
    if let Some((op, _)) = stores.audit.operation_for_dedupe(&dedupe_key)? {
        if let Some(json) = stores.audit.operation_report(&op)? {
            return serde_json::from_str(&json).map_err(|e| ApplyError::Store(StoreError::AuditUnavailable(e.to_string())));
        }
        return Err(ApplyError::Conflict(op));
    }
    check_plan(&plan, (ctx.now)(), ctx.current_rule_set_digest).map_err(ApplyError::Stale)?;
    let operation_id = format!("op_{}", &plan.digest[..16]);
    let (auth_source, policy) = find_authorization(stores, &plan, ctx, &operation_id)?;
    let identities: Vec<String> = plan.body.items.iter().map(|i| identity_key(&i.identity)).collect();
    let release = |stores: &Stores| {
        if policy.is_some() {
            let _ = stores.audit.settle_quota(&operation_id, (0, 0), (0, 0));
        }
    };
    if let Some(holder) = stores.audit.lock_identities(&operation_id, &identities)? {
        release(stores);
        return Err(ApplyError::Conflict(holder));
    }
    let started_at = utc_from_unix((ctx.now)());
    stores.audit.begin_operation(&operation_id, plan_id, &plan.digest, &dedupe_key, ctx.host, &started_at)?;
    stores.audit.set_operation_state(&operation_id, "running")?;
    let free_before = plan.body.items.first().and_then(|i| actions.free_space(&i.path));

    let n = plan.body.items.len();
    let probes: Vec<Vec<String>> = (0..n).map(|i| plan.item_meta.get(i).map(|m| m.probes.clone()).unwrap_or_default()).collect();
    let target = |idx: usize| {
        let item = &plan.body.items[idx];
        ActionTarget {
            action_id: &item.action_id,
            path: &item.path,
            identity: &item.identity,
            parent_chain: &plan.observations[idx].parent_chain,
            observation: &plan.observations[idx],
            probes: &probes[idx],
            owner_id: plan.item_meta.get(idx).and_then(|m| m.owner_id.as_deref()),
        }
    };
    let mut slots: Vec<Option<ActionResult>> = vec![None; n];
    let mut intended = vec![false; n];
    let mut cancelled = false;
    let expires = parse_utc(&plan.body.expires_at).unwrap_or(i64::MIN);
    let now_text = || utc_from_unix((ctx.now)());
    let record = |r: &ActionResult| -> Result<(), ApplyError> {
        stores.audit.record_result(&operation_id, &r.action_id, serde_json::to_value(r.status).unwrap().as_str().unwrap(), &serde_json::to_string(r).unwrap(), &now_text())?;
        Ok(())
    };

    // Trash mode: a planned directory goes to the Recycle Bin as one object when its live subtree
    // still equals the plan; otherwise its items fall through to the per-item pass below.
    if plan.body.mode == Mode::Trash {
        for root in tree_roots(&plan) {
            if stop_reason(stores, &plan, ctx, &auth_source, expires, &mut cancelled)?.is_some() {
                break; // the per-item pass records every remaining item as not started
            }
            let members: Vec<usize> = (0..n).filter(|&m| m == root || is_ancestor(&plan, root, m)).collect();
            // Every member must pass the protection re-check; otherwise the per-item pass reports
            // the protected ones and keeps their directories.
            if members.iter().any(|&m| (ctx.protect)(&target(m)).is_err()) {
                continue;
            }
            for &m in &members {
                let item = &plan.body.items[m];
                let intent = serde_json::json!({"mode": plan.body.mode, "action": "trash_tree", "root": plan.body.items[root].action_id,
                    "path": item.path, "identity": item.identity, "fingerprint": item.observed_fingerprint_sha256});
                stores.audit.record_intent(&operation_id, &item.action_id, &intent.to_string(), &now_text())?;
                intended[m] = true;
            }
            let root_id = &plan.body.items[root].action_id;
            if (ctx.fault)(FaultPoint::AfterIntent, root_id) {
                return Err(ApplyError::Crashed("after_intent".into()));
            }
            let tree: Vec<TreeMember<'_>> = members
                .iter()
                .filter(|&&m| m != root)
                .map(|&m| TreeMember {
                    path: &plan.body.items[m].path,
                    identity: &plan.body.items[m].identity,
                    observation: &plan.observations[m],
                    directory: plan.body.items[m].kind == ItemKind::Directory,
                })
                .collect();
            let outcome = actions.trash_tree(&target(root), &tree);
            if (ctx.fault)(FaultPoint::AfterAction, root_id) {
                return Err(ApplyError::Crashed("after_action".into()));
            }
            let (status, code, message, recovery) = match &outcome {
                ActionOutcome::Succeeded { .. } => (ActionStatus::Succeeded, "done", "moved to the Recycle Bin with its directory".to_string(), "trash"),
                ActionOutcome::Unknown { message } => (ActionStatus::Unknown, "outcome_unconfirmed", message.clone(), "unknown"),
                _ => continue, // nothing changed: fall back to leaves + empty-directory removal
            };
            for &m in &members {
                let r = ActionResult { action_id: plan.body.items[m].action_id.clone(), status, code: code.into(), message: message.clone(), recovery: recovery.into() };
                record(&r)?;
                slots[m] = Some(r);
            }
        }
    }

    // Leaves first, then directories deepest first (bottom-up).
    for idx in schedule(&plan) {
        if slots[idx].is_some() {
            continue;
        }
        let item = &plan.body.items[idx];
        let directory = item.kind == ItemKind::Directory;
        // EXE-06: cancellation, expiry and revocation stop new actions only.
        if let Some((code, message)) = stop_reason(stores, &plan, ctx, &auth_source, expires, &mut cancelled)? {
            slots[idx] = Some(ActionResult { action_id: item.action_id.clone(), status: ActionStatus::NotStarted, code: code.into(), message: message.into(), recovery: "not_applicable".into() });
            continue;
        }
        // A directory is attempted only once everything planned inside it is gone.
        if directory && (0..n).any(|d| is_ancestor(&plan, idx, d) && slots[d].as_ref().is_none_or(|r| r.status != ActionStatus::Succeeded)) {
            let r = ActionResult { action_id: item.action_id.clone(), status: ActionStatus::Skipped, code: "precondition_descendant_kept".into(),
                message: "a planned item inside was not removed; the directory is kept".into(), recovery: "not_applicable".into() };
            record(&r)?;
            slots[idx] = Some(r);
            continue;
        }
        let target = target(idx);
        if !intended[idx] {
            let action = if directory { "remove_empty_dir" } else { "file" };
            let intent = serde_json::json!({"mode": plan.body.mode, "action": action, "path": item.path, "identity": item.identity, "fingerprint": item.observed_fingerprint_sha256});
            stores.audit.record_intent(&operation_id, &item.action_id, &intent.to_string(), &now_text())?;
        }
        if (ctx.fault)(FaultPoint::AfterIntent, &item.action_id) {
            return Err(ApplyError::Crashed("after_intent".into()));
        }
        let outcome = if let Err(reason) = (ctx.protect)(&target) {
            ActionOutcome::Skipped { code: "protected".into(), message: reason }
        } else if directory {
            actions.remove_empty_dir(&target)
        } else {
            match actions.check(&target) {
            LiveCheck::Match => match plan.body.mode {
                Mode::Delete => actions.delete(&target),
                Mode::Trash => actions.trash(&target),
                Mode::Quarantine => match actions.quarantine_dir_for(&target, &operation_id) {
                    Some(dir) => actions.quarantine(&target, &dir, &item.action_id),
                    None => ActionOutcome::Skipped { code: "quarantine_unavailable".into(), message: "no same-volume quarantine location".into() },
                },
            },
            other => {
                let code = serde_json::to_value(&other).ok().and_then(|v| v["check"].as_str().map(str::to_owned)).unwrap_or_else(|| "precondition".into());
                ActionOutcome::Skipped { code: format!("precondition_{code}"), message: format!("{other:?}") }
            }
            }
        };
        if (ctx.fault)(FaultPoint::AfterAction, &item.action_id) {
            return Err(ApplyError::Crashed("after_action".into()));
        }
        let result = match &outcome {
            ActionOutcome::Succeeded { .. } if directory => ActionResult { action_id: item.action_id.clone(), status: ActionStatus::Succeeded, code: "done".into(),
                message: "empty directory removed".into(), recovery: "not_applicable".into() },
            ActionOutcome::Succeeded { recovery } => {
                if let Recovery::Quarantine { location } = recovery {
                    let record = serde_json::json!({
                        "original_path": item.path, "original_identity": item.identity,
                        "original_parent": plan.observations[idx].parent_chain.first(),
                        "location": location, "logical_bytes": item.logical_bytes,
                    });
                    let now = (ctx.now)();
                    stores.audit.save_quarantine(&format!("q_{}_{}", &operation_id[3..], item.action_id), &operation_id, &item.action_id,
                        plan_id, &record.to_string(), &utc_from_unix(now), &utc_from_unix(now + 30 * 86_400))?;
                }
                ActionResult { action_id: item.action_id.clone(), status: ActionStatus::Succeeded, code: "done".into(), message: done_message(recovery).into(), recovery: recovery_name(recovery).into() }
            }
            ActionOutcome::Skipped { code, message } => ActionResult { action_id: item.action_id.clone(), status: ActionStatus::Skipped, code: precondition_code(code), message: message.clone(), recovery: "not_applicable".into() },
            ActionOutcome::Failed { code, message } => ActionResult { action_id: item.action_id.clone(), status: ActionStatus::Failed, code: code.clone(), message: message.clone(), recovery: "not_applicable".into() },
            ActionOutcome::Unknown { message } => ActionResult { action_id: item.action_id.clone(), status: ActionStatus::Unknown, code: "outcome_unconfirmed".into(), message: message.clone(), recovery: "unknown".into() },
        };
        record(&result)?;
        slots[idx] = Some(result);
    }
    let results: Vec<ActionResult> = slots.into_iter().map(|s| s.expect("every planned item is scheduled once")).collect();
    finish(stores, actions, &plan, &operation_id, &started_at, results, cancelled, free_before, &auth_source, policy.is_some(), ctx)
}

/// Why no new action may start now (EXE-06), as a `not_started` code and message.
fn stop_reason(stores: &Stores, plan: &Plan, ctx: &ApplyContext<'_>, auth_source: &str, expires: i64, cancelled: &mut bool) -> Result<Option<(&'static str, &'static str)>, ApplyError> {
    if *cancelled || (ctx.cancel)() {
        *cancelled = true;
        return Ok(Some(("cancelled", "operation cancelled before this action started")));
    }
    if (ctx.now)() >= expires {
        return Ok(Some(("plan_expired", "plan expired before this action started")));
    }
    if auth_source == "interactive_review" && !stores.audit.receipts_for(&plan.body.plan_id)?.iter().any(|r| receipt_valid(r, plan, ctx.principal, (ctx.now)())) {
        return Ok(Some(("authorization_revoked", "authorization was revoked or expired")));
    }
    Ok(None)
}

/// Whether directory item `dir` is a planned ancestor of item `other` (by identity, not text).
fn is_ancestor(plan: &Plan, dir: usize, other: usize) -> bool {
    let d = &plan.body.items[dir];
    dir != other
        && d.kind == ItemKind::Directory
        && plan.observations[other].parent_chain.iter().any(|p| p.volume_id == d.identity.volume_id && p.wire_file_id() == d.identity.file_id)
}

/// Directory items with no planned directory above them.
fn tree_roots(plan: &Plan) -> Vec<usize> {
    let n = plan.body.items.len();
    (0..n).filter(|&i| plan.body.items[i].kind == ItemKind::Directory && !(0..n).any(|d| is_ancestor(plan, d, i))).collect()
}

/// Execution order: files in plan order, then directories deepest first (bottom-up).
fn schedule(plan: &Plan) -> Vec<usize> {
    let mut order: Vec<usize> = (0..plan.body.items.len()).collect();
    order.sort_by_key(|&i| match plan.body.items[i].kind {
        ItemKind::File => (0, 0, i),
        ItemKind::Directory => (1, usize::MAX - plan.observations[i].parent_chain.len(), i),
    });
    order
}

/// Platforms re-verify inside the action itself; a failed re-check there is the same
/// precondition failure as one found by `check`, so both report one code family.
fn precondition_code(code: &str) -> String {
    const LIVE: [&str; 9] = ["missing", "link_or_reparse", "identity_changed", "parent_changed", "metadata_changed", "in_use", "owner_running", "unsupported", "new_descendant"];
    if LIVE.contains(&code) { format!("precondition_{code}") } else { code.to_owned() }
}

#[allow(clippy::too_many_arguments)]
fn finish(
    stores: &Stores, actions: &dyn FileActions, plan: &Plan, operation_id: &str, started_at: &str, results: Vec<ActionResult>,
    cancelled: bool, free_before: Option<u64>, auth_source: &str, policy: bool, ctx: &ApplyContext<'_>,
) -> Result<OperationReport, ApplyError> {
    for r in results.iter().filter(|r| r.status == ActionStatus::NotStarted) {
        stores.audit.record_result(operation_id, &r.action_id, "not_started", &serde_json::to_string(r).unwrap(), &utc_from_unix((ctx.now)()))?;
    }
    let free_after = plan.body.items.first().and_then(|i| actions.free_space(&i.path));
    let (ok, skipped, failed, not_started, unknown) = summarize(&results);
    let size_of = |id: &str| plan.body.items.iter().find(|i| i.action_id == id).map(|i| i.logical_bytes.0).unwrap_or(0);
    let processed: u64 = results.iter().filter(|r| r.status == ActionStatus::Succeeded).map(|r| size_of(&r.action_id)).sum();
    let unknown_bytes: u64 = results.iter().filter(|r| r.status == ActionStatus::Unknown).map(|r| size_of(&r.action_id)).sum();
    let quarantined: u64 = results.iter().filter(|r| r.status == ActionStatus::Succeeded && r.recovery == "quarantine").map(|r| size_of(&r.action_id)).sum();
    let estimated_freed = if plan.body.mode == Mode::Delete { processed } else { 0 };
    let status = final_status(&results, cancelled);
    let report = OperationReport {
        operation_id: operation_id.into(),
        plan_id: plan.body.plan_id.clone(),
        plan_digest: plan.digest.clone(),
        status: status.into(),
        started_at: started_at.into(),
        finished_at: utc_from_unix((ctx.now)()),
        succeeded: ok,
        skipped,
        failed,
        not_started,
        unknown,
        items: results,
        processed_logical_bytes: processed.to_string(),
        quarantined_logical_bytes: quarantined.to_string(),
        estimated_freed_bytes: estimated_freed.to_string(),
        volume_free_before_bytes: free_before.map(|b| b.to_string()),
        volume_free_after_bytes: free_before.and(free_after).map(|b| b.to_string()),
        observed_free_delta_bytes: free_before.zip(free_after).map(|(b, a)| (i128::from(a) - i128::from(b)).to_string()),
        authorization_source: auth_source.into(),
    };
    if policy {
        stores.audit.settle_quota(operation_id, (processed, ok), (unknown_bytes, unknown))?;
    }
    // Locks stay while outcomes are unknown so no other plan touches those objects (EXE-04/05).
    if unknown == 0 {
        stores.audit.release_locks(operation_id)?;
    }
    stores.audit.finish_operation(operation_id, status, &serde_json::to_string(&report).unwrap(), &report.finished_at)?;
    Ok(report)
}

/// Resolve actions that have an intent but no result after the hosting process stopped.
pub fn reconcile_operation(stores: &Stores, actions: &dyn FileActions, operation_id: &str, ctx: &ApplyContext<'_>) -> Result<OperationReport, ApplyError> {
    let plan = stores.audit.load_plan(&stores.audit.operation_plan(operation_id)?)?;
    let journal = stores.audit.journal(operation_id)?;
    let mut results = Vec::new();
    for (idx, item) in plan.body.items.iter().enumerate() {
        let row = journal.iter().find(|r| r.action_id == item.action_id);
        let existing: Option<ActionResult> = row.and_then(|r| r.result_json.as_deref()).and_then(|j| serde_json::from_str(j).ok());
        let result = match (row, existing) {
            (_, Some(r)) if r.status != ActionStatus::Unknown => r,
            (None, _) => ActionResult { action_id: item.action_id.clone(), status: ActionStatus::NotStarted, code: "never_started".into(), message: "no action was started".into(), recovery: "not_applicable".into() },
            (Some(row), _) => {
                // An empty-directory removal is a delete whatever the plan mode (nothing to recover).
                let tree = serde_json::from_str::<serde_json::Value>(&row.intent_json).ok().is_some_and(|v| v["action"] == "trash_tree");
                let empty_dir = item.kind == ItemKind::Directory && !tree;
                let mode = if empty_dir { Mode::Delete } else { plan.body.mode };
                let meta = plan.item_meta.get(idx);
                let probes = meta.map(|m| m.probes.clone()).unwrap_or_default();
                let target = ActionTarget {
                    action_id: &item.action_id, path: &item.path, identity: &item.identity,
                    parent_chain: &plan.observations[idx].parent_chain, observation: &plan.observations[idx], probes: &probes,
                    owner_id: meta.and_then(|m| m.owner_id.as_deref()),
                };
                let qloc = actions.quarantine_dir_for(&target, operation_id).and_then(|d| {
                    let name = match &d {
                        RawPath::Windows(_) => RawPath::windows(item.action_id.encode_utf16().collect()).ok()?,
                        RawPath::Unix(_) => RawPath::unix(item.action_id.as_bytes().to_vec()).ok()?,
                    };
                    d.join(&name).ok()
                });
                let r = match actions.reconcile(&target, mode, qloc.as_ref()) {
                    Reconciled::NotPerformed => ActionResult { action_id: item.action_id.clone(), status: ActionStatus::NotStarted, code: "reconciled_not_performed".into(), message: "object still at its planned place".into(), recovery: "not_applicable".into() },
                    Reconciled::Performed => ActionResult { action_id: item.action_id.clone(), status: ActionStatus::Succeeded, code: "reconciled_performed".into(), message: "the action was found to have taken effect".into(),
                        recovery: if empty_dir { "not_applicable" } else { match mode { Mode::Delete => "unavailable", Mode::Trash => "trash", Mode::Quarantine => "quarantine" } }.into() },
                    Reconciled::StillUnknown => ActionResult { action_id: item.action_id.clone(), status: ActionStatus::Unknown, code: "outcome_unconfirmed".into(), message: "evidence insufficient; needs a person to check".into(), recovery: "unknown".into() },
                };
                stores.audit.record_result(operation_id, &item.action_id, serde_json::to_value(r.status).unwrap().as_str().unwrap(), &serde_json::to_string(&r).unwrap(), &utc_from_unix((ctx.now)()))?;
                r
            }
        };
        results.push(result);
    }
    let started = stores.audit.operation_report(operation_id)?.and_then(|j| serde_json::from_str::<OperationReport>(&j).ok()).map(|r| r.started_at)
        .unwrap_or_else(|| utc_from_unix((ctx.now)()));
    finish(stores, actions, &plan, operation_id, &started, results, false, None, "reconciliation", false, ctx)
}

#[cfg(test)]
mod tests;
