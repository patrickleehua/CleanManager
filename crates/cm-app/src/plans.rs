//! `plan/v2` services shared by the CLI and the desktop: kind dispatch, authorization, application and reconciliation.
//!
//! The stored contract decides how a plan is shown, reviewed and applied (`plan/v1` → cleanup executor, `recovery/v1`
//! → quarantine recovery, `plan/v2` → this module by `plan_kind`); a caller never chooses. `plan/v2` plans run through
//! the same dedupe, identity locks, durable intent → action → result journal and report shape as the other two. Before
//! anything is touched the kind rules are checked again (a body that violates its kind is refused), and only an
//! interactive review of the exact plan authorizes it: policies are never consulted here, so a manual selection,
//! startup or recovery plan stays `needs_approval` even while a cleanup policy is enabled.
//!
//! Executed kinds: `manual_selection` (fs_trash at the original place, fs_delete of recycled objects), `startup`
//! (enable/disable through the enable record, stale Run-value removal after a stored backup) and `recovery` (put back
//! enable records and removed values, never over something changed or recreated since). Other kinds are refused.

use cm_core::authz::{KindReceipt, kind_receipt_valid};
use cm_core::execute::{ActionOutcome, ActionTarget, FileActions, LiveCheck, Reconciled, Recovery, TreeMember};
use cm_core::manual_trash::{PUT_BACK_GUIDANCE, UNKNOWN_GUIDANCE, in_recycle_bin};
use cm_core::path::RawPath;
use cm_core::plan::v2::{ActionMeta, FsAction, PlanAction, PlanKind, PlanProblem, PlanV2, StartupOperation, StartupToggleAction, check_plan_v2};
use cm_core::plan::{ItemKind, parse_utc};
use cm_core::rules::Mode;
use cm_core::startup::{RegValue, StartupActions, StartupBackup, StartupObservation, StartupOutcome, observation_digest};
use cm_core::time::utc_from_unix;
use cm_store::{StoreError, Stores};

use crate::execute::{ActionResult, ActionStatus, ApplyContext, ApplyError, FaultPoint, OperationReport, final_status, summarize};

/// How a stored plan is handled, from its authoritative stored contract (and kind).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanType {
    /// `plan/v1`: the cleanup executor.
    CleanupV1,
    /// `recovery/v1`: quarantine restore/purge.
    RecoveryV1,
    V2(PlanKind),
}

pub fn plan_type(stores: &Stores, plan_id: &str) -> Result<PlanType, StoreError> {
    match stores.audit.plan_contract(plan_id)?.as_str() {
        "plan/v1" => Ok(PlanType::CleanupV1),
        cm_core::recovery::RECOVERY_CONTRACT => Ok(PlanType::RecoveryV1),
        cm_core::plan::v2::PLAN_V2_CONTRACT => Ok(PlanType::V2(stores.audit.load_plan_v2(plan_id)?.body.plan_kind)),
        other => Err(StoreError::InvalidState(format!("plan {plan_id} has unknown contract {other}"))),
    }
}

/// Kinds this build can apply.
pub fn executable(kind: PlanKind) -> bool {
    matches!(kind, PlanKind::ManualSelection | PlanKind::Startup | PlanKind::Recovery | PlanKind::Process | PlanKind::Update | PlanKind::Uninstall)
}

/// Authorization state from stored kind receipts (never from a client): `authorized`, `expired`, `revoked`, `required`.
pub fn authorization(stores: &Stores, plan: &PlanV2, principal: &str, now: i64) -> Result<(&'static str, Option<KindReceipt>), StoreError> {
    let receipts = stores.audit.kind_receipts_for(&plan.body.plan_id)?;
    let valid = receipts.iter().find(|r| kind_receipt_valid(r, plan, principal, now)).cloned();
    let expired = parse_utc(&plan.body.expires_at).is_none_or(|e| now >= e);
    let status = match (&valid, expired) {
        (Some(_), _) => "authorized",
        (None, true) => "expired",
        (None, false) if receipts.iter().any(|r| r.revoked_at.is_some()) => "revoked",
        (None, false) => "required",
    };
    Ok((status, valid))
}

/// Platform backends a `plan/v2` operation may need.
pub struct V2Backends<'a> {
    pub files: &'a dyn FileActions,
    pub startup: Option<&'a dyn StartupActions>,
    /// Names of processes holding these files open (Windows Restart Manager); `None` = cannot be determined.
    pub holders: &'a dyn Fn(&[RawPath]) -> Option<Vec<String>>,
    /// User protection of a startup item by id (PRT-06); `Err(reason)` skips it as `protected`.
    pub protect_item: &'a dyn Fn(&str) -> Result<(), String>,
    /// `process_end` (STS-04): re-identify and end one process; `None` = unsupported here.
    pub processes: Option<&'a dyn cm_core::process::ProcessActions>,
    /// `update` plans' package-manager steps (UPD-02/03); `None` = unsupported here.
    pub updates: Option<&'a dyn cm_core::updates::UpdateBackend>,
    /// `uninstall` plans (UNI-01..09) and restores of their registry backups; `None` = unsupported here.
    pub uninstall: Option<&'a dyn cm_core::uninstall::UninstallActions>,
}

fn res(action_id: &str, status: ActionStatus, code: &str, message: impl Into<String>, recovery: &str) -> ActionResult {
    ActionResult { action_id: action_id.into(), status, code: code.into(), message: message.into(), recovery: recovery.into() }
}

fn observation_target<'a>(a: &'a FsAction, m: &'a ActionMeta, probes: &'a [String]) -> ActionTarget<'a> {
    let obs = m.observation.as_ref().expect("checked by check_plan_v2");
    ActionTarget { action_id: &a.action_id, path: &a.target.path, identity: &a.target.identity, parent_chain: &obs.parent_chain, observation: obs, probes, owner_id: None }
}

/// Identities an operation locks against other plans.
fn lock_keys(plan: &PlanV2) -> Vec<String> {
    let mut keys = Vec::new();
    for (a, m) in plan.body.items.iter().zip(&plan.meta) {
        if let Some(fs) = a.fs() {
            keys.push(format!("{}|{}", fs.target.identity.volume_id, fs.target.identity.file_id));
            keys.extend(m.members.iter().map(|mm| format!("{}|{}", mm.identity.volume_id, mm.identity.file_id)));
        }
        if let Some(loc) = &m.startup {
            keys.push(format!("startup|{}", serde_json::to_string(loc).expect("location serializes")));
        }
    }
    keys.sort();
    keys.dedup();
    keys
}

fn in_use_message(holders: Option<Vec<String>>) -> String {
    match holders {
        Some(h) if !h.is_empty() => format!("in use by {}; close it and retry (CleanManager never ends processes)", h.join(", ")),
        _ => "in use; the process holding it could not be determined. Close programs using it and retry".into(),
    }
}

/// Move one manually selected file or folder to the Recycle Bin (never degraded to a permanent delete).
fn trash_one(b: &V2Backends<'_>, ctx: &ApplyContext<'_>, a: &FsAction, m: &ActionMeta) -> ActionResult {
    let id = &a.action_id;
    let target = observation_target(a, m, &[]);
    let members: Vec<(ActionTarget<'_>, TreeMember<'_>)> = m
        .members
        .iter()
        .map(|mm| {
            let t = ActionTarget { action_id: id, path: &mm.path, identity: &mm.identity, parent_chain: &mm.observation.parent_chain, observation: &mm.observation, probes: &[], owner_id: None };
            (t, TreeMember { path: &mm.path, identity: &mm.identity, observation: &mm.observation, directory: mm.directory })
        })
        .collect();
    // PRT-01: the object and, for a folder, everything moved with it pass the protection re-check.
    for t in std::iter::once(&target).chain(members.iter().map(|(t, _)| t)) {
        if let Err(why) = (ctx.protect)(t) {
            return res(id, ActionStatus::Skipped, "protected", format!("{} is protected: {why}", t.path.display()), "not_applicable");
        }
    }
    let holders = || {
        let paths: Vec<RawPath> = std::iter::once(a.target.path.clone()).chain(m.members.iter().filter(|mm| !mm.directory).map(|mm| mm.path.clone())).take(1000).collect();
        (b.holders)(&paths)
    };
    let outcome = if a.target.kind == ItemKind::Directory {
        let tree: Vec<TreeMember<'_>> = members.iter().map(|(_, tm)| *tm).collect();
        b.files.trash_tree(&target, &tree)
    } else {
        match b.files.check(&target) {
            LiveCheck::Match => b.files.trash(&target),
            LiveCheck::InUse => return res(id, ActionStatus::Failed, "in_use", in_use_message(holders()), "not_applicable"),
            other => {
                let code = serde_json::to_value(&other).ok().and_then(|v| v["check"].as_str().map(str::to_owned)).unwrap_or_else(|| "precondition".into());
                return res(id, ActionStatus::Skipped, &format!("precondition_{code}"), format!("{other:?}; nothing was moved"), "not_applicable");
            }
        }
    };
    match outcome {
        ActionOutcome::Succeeded { recovery: Recovery::Trash } => res(id, ActionStatus::Succeeded, "done", format!("moved to the Recycle Bin. {PUT_BACK_GUIDANCE}"), "trash"),
        ActionOutcome::Succeeded { .. } => res(id, ActionStatus::Unknown, "outcome_unconfirmed", format!("the platform reported a move that was not to the Recycle Bin. {UNKNOWN_GUIDANCE}"), "unknown"),
        ActionOutcome::Unknown { message } => res(id, ActionStatus::Unknown, "outcome_unconfirmed", format!("{message}. {UNKNOWN_GUIDANCE}"), "unknown"),
        ActionOutcome::Skipped { code, .. } if code == "in_use" => res(id, ActionStatus::Failed, "in_use", in_use_message(holders()), "not_applicable"),
        ActionOutcome::Skipped { code, message } => res(id, ActionStatus::Skipped, &crate_precondition(&code), format!("{message}; nothing was moved"), "not_applicable"),
        ActionOutcome::Failed { code, message } => {
            // A folder that could not be moved because something inside is open is reported as in use, with the holders.
            let h = if code == "in_use" || a.target.kind == ItemKind::Directory { holders().filter(|h| !h.is_empty()) } else { None };
            match h {
                Some(h) => res(id, ActionStatus::Failed, "in_use", in_use_message(Some(h)), "not_applicable"),
                None => res(id, ActionStatus::Failed, &code, format!("{message}; nothing was deleted"), "not_applicable"),
            }
        }
    }
}

fn crate_precondition(code: &str) -> String {
    const LIVE: [&str; 10] = ["missing", "link_or_reparse", "identity_changed", "parent_changed", "metadata_changed", "in_use", "owner_running", "unsupported", "new_descendant", "tree_mismatch"];
    if LIVE.contains(&code) { format!("precondition_{code}") } else { code.to_owned() }
}

/// Permanently delete one object that is already in the recycle bin (MTR-03).
fn purge_one(b: &V2Backends<'_>, ctx: &ApplyContext<'_>, a: &FsAction, m: &ActionMeta) -> ActionResult {
    let id = &a.action_id;
    if !in_recycle_bin(&a.target.path) {
        return res(id, ActionStatus::Skipped, "not_in_trash", "only objects already in the recycle bin are deleted permanently", "not_applicable");
    }
    let target = observation_target(a, m, &[]);
    if let Err(why) = (ctx.protect)(&target) {
        return res(id, ActionStatus::Skipped, "protected", why, "not_applicable");
    }
    let outcome = if a.target.kind == ItemKind::Directory {
        b.files.remove_empty_dir(&target)
    } else {
        match b.files.check(&target) {
            LiveCheck::Match => b.files.delete(&target),
            LiveCheck::InUse => return res(id, ActionStatus::Failed, "in_use", in_use_message((b.holders)(std::slice::from_ref(&a.target.path))), "trash"),
            other => {
                let code = serde_json::to_value(&other).ok().and_then(|v| v["check"].as_str().map(str::to_owned)).unwrap_or_else(|| "precondition".into());
                return res(id, ActionStatus::Skipped, &format!("precondition_{code}"), format!("{other:?}; nothing was deleted"), "trash");
            }
        }
    };
    match outcome {
        ActionOutcome::Succeeded { .. } => res(id, ActionStatus::Succeeded, "purged", "permanently deleted from the recycle bin; there is no undo", "unavailable"),
        ActionOutcome::Unknown { message } => res(id, ActionStatus::Unknown, "outcome_unconfirmed", message, "unknown"),
        ActionOutcome::Skipped { code, message } => res(id, ActionStatus::Skipped, &crate_precondition(&code), message, "trash"),
        ActionOutcome::Failed { code, message } => res(id, ActionStatus::Failed, &code, message, "trash"),
    }
}

fn startup_result(id: &str, outcome: StartupOutcome, done: &str, recovery_when_done: &str) -> ActionResult {
    match outcome {
        StartupOutcome::Done => res(id, ActionStatus::Succeeded, "done", done, recovery_when_done),
        StartupOutcome::Skipped { code, message } => res(id, ActionStatus::Skipped, &code, message, "not_applicable"),
        StartupOutcome::Failed { code, message } => res(id, ActionStatus::Failed, &code, message, "not_applicable"),
        StartupOutcome::Unknown { message } => res(id, ActionStatus::Unknown, "outcome_unconfirmed", message, "unknown"),
    }
}

struct Run<'a> {
    stores: &'a Stores,
    plan: &'a PlanV2,
    operation_id: String,
    ctx: &'a ApplyContext<'a>,
}

impl Run<'_> {
    fn backup_id(&self, action_id: &str) -> String {
        format!("sb_{}_{action_id}", &self.operation_id[3..])
    }

    fn save_backup(&self, action_id: &str, s: &StartupToggleAction, loc: &cm_core::startup::StartupLocation, obs: &StartupObservation, written: Option<&RegValue>) -> Result<(), String> {
        let b = StartupBackup {
            backup_id: self.backup_id(action_id),
            startup_item_id: s.startup_item_id.clone(),
            operation: s.operation,
            location: loc.clone(),
            prior_definition: obs.definition.clone(),
            prior_approval: obs.approval.clone(),
            written_approval: written.cloned(),
            created_at: utc_from_unix((self.ctx.now)()),
        };
        self.stores.audit.save_startup_backup(&self.operation_id, action_id, &self.plan.body.plan_id, &b).map_err(|e| format!("backup not stored: {e}"))
    }

    fn startup(&self, b: &V2Backends<'_>, a: &StartupToggleAction, m: &ActionMeta) -> ActionResult {
        let id = &a.action_id;
        let Some(sa) = b.startup else { return res(id, ActionStatus::Skipped, "unsupported", "startup changes are not available on this platform", "not_applicable") };
        let Some(loc) = &m.startup else { return res(id, ActionStatus::Skipped, "unsupported", "no verified location for this item", "not_applicable") };
        if let Err(why) = (b.protect_item)(&a.startup_item_id) {
            return res(id, ActionStatus::Skipped, "protected", why, "not_applicable");
        }
        if loc.hive() == cm_core::plan::v2::Hive::Hklm && !sa.is_admin() {
            return res(id, ActionStatus::Skipped, "permission_required", cm_core::startup::ADMIN_REQUIRED, "not_applicable");
        }
        let expected = a.observed_sha256.as_deref().unwrap_or_default();
        match a.operation {
            StartupOperation::Enable | StartupOperation::Disable => {
                let enabled = a.operation == StartupOperation::Enable;
                let mut backup = |obs: &StartupObservation, written: Option<&RegValue>| self.save_backup(id, a, loc, obs, written);
                let done = format!("{}; the prior state is recorded (cleanmanager startup restore --operation {})", if enabled { "enabled" } else { "disabled" }, self.operation_id);
                startup_result(id, sa.set_enabled(loc, enabled, expected, &mut backup), &done, "not_applicable")
            }
            StartupOperation::RemoveWithBackup => {
                let mut backup = |obs: &StartupObservation| self.save_backup(id, a, loc, obs, None);
                let done = format!("stale entry removed after its backup was stored (cleanmanager startup restore --operation {})", self.operation_id);
                startup_result(id, sa.remove_stale(loc, expected, &mut backup), &done, "not_applicable")
            }
            StartupOperation::Restore => {
                let Some(bid) = &a.backup_id else { return res(id, ActionStatus::Skipped, "backup_missing", "no recorded prior state", "not_applicable") };
                let backup = match self.stores.audit.startup_backup(bid) {
                    Ok((bk, _)) if Some(bk.sha256().as_str()) == a.observed_sha256.as_deref() && &bk.location == loc => bk,
                    Ok(_) => return res(id, ActionStatus::Skipped, "backup_changed", "the recorded prior state differs from the planned one", "not_applicable"),
                    Err(e) => return res(id, ActionStatus::Failed, "backup_unavailable", e.to_string(), "not_applicable"),
                };
                let r = startup_result(id, sa.restore_approval(loc, backup.written_approval.as_ref(), backup.prior_approval.as_ref()), "prior enable state restored", "not_applicable");
                if r.status == ActionStatus::Succeeded {
                    let _ = self.stores.audit.set_startup_backup_state(bid, "restored");
                }
                r
            }
        }
    }

    fn registry_restore(&self, b: &V2Backends<'_>, a: &cm_core::plan::v2::RegistryRestoreAction) -> ActionResult {
        let id = &a.action_id;
        let Some(sa) = b.startup else { return res(id, ActionStatus::Skipped, "unsupported", "registry restore is not available on this platform", "not_applicable") };
        let backup = match self.stores.audit.startup_backup(&a.backup_id) {
            Ok((bk, _)) if a.backup_sha256.as_deref().is_some_and(|s| s == bk.sha256()) => bk,
            Ok(_) => return res(id, ActionStatus::Skipped, "backup_changed", "the stored backup differs from the planned one", "not_applicable"),
            Err(e) => return res(id, ActionStatus::Failed, "backup_unavailable", e.to_string(), "not_applicable"),
        };
        if a.hive == cm_core::plan::v2::Hive::Hklm && !sa.is_admin() {
            return res(id, ActionStatus::Skipped, "permission_required", cm_core::startup::ADMIN_REQUIRED, "not_applicable");
        }
        let r = startup_result(id, sa.restore_value(&backup), "value recreated from its backup", "not_applicable");
        if r.status == ActionStatus::Succeeded {
            let _ = self.stores.audit.set_startup_backup_state(&a.backup_id, "restored");
        }
        r
    }

    fn intent(&self, a: &PlanAction) -> serde_json::Value {
        match a {
            PlanAction::FsTrash(f) | PlanAction::FsDelete(f) => serde_json::json!({"action_type": a.action_type(), "path": f.target.path, "identity": f.target.identity,
                "fingerprint": f.target.observed_fingerprint_sha256, "kind": f.target.kind}),
            other => serde_json::json!({"action_type": other.action_type(), "action": other}),
        }
    }
}

/// Apply a stored `plan/v2` plan. Returns the final report, or an error before anything was touched.
pub fn apply_plan_v2(stores: &Stores, b: &V2Backends<'_>, plan_id: &str, ctx: &ApplyContext<'_>) -> Result<OperationReport, ApplyError> {
    let plan = stores.audit.load_plan_v2(plan_id)?;
    let dedupe_key = format!("{}#{}", plan.body.plan_id, plan.digest);
    // EXE-03: the same plan never produces a second operation.
    if let Some((op, _)) = stores.audit.operation_for_dedupe(&dedupe_key)? {
        if let Some(json) = stores.audit.operation_report(&op)? {
            return serde_json::from_str(&json).map_err(|e| ApplyError::Store(StoreError::AuditUnavailable(e.to_string())));
        }
        return Err(ApplyError::Conflict(op));
    }
    match check_plan_v2(&plan, (ctx.now)()) {
        Ok(()) => {}
        Err(PlanProblem::Stale(r)) => return Err(ApplyError::Stale(r)),
        Err(PlanProblem::Kind(k)) => return Err(ApplyError::PolicyDenied(format!("plan violates its kind: {k}"))),
    }
    if !executable(plan.body.plan_kind) {
        return Err(ApplyError::PolicyDenied(format!("plan kind {} cannot be applied by this build", plan.body.plan_kind.as_str())));
    }
    // Only an interactive review of this exact plan; policies are never consulted for plan/v2 kinds.
    let authorized = |now: i64| -> Result<bool, StoreError> { Ok(authorization(stores, &plan, ctx.principal, now)?.0 == "authorized") };
    if !authorized((ctx.now)())? {
        return Err(ApplyError::NeedsApproval(plan_id.into()));
    }
    let operation_id = format!("op_{}", &plan.digest[..16]);
    if let Some(holder) = stores.audit.lock_identities(&operation_id, &lock_keys(&plan))? {
        return Err(ApplyError::Conflict(holder));
    }
    let started_at = utc_from_unix((ctx.now)());
    stores.audit.begin_operation(&operation_id, plan_id, &plan.digest, &dedupe_key, ctx.host, &started_at)?;
    stores.audit.set_operation_state(&operation_id, "running")?;
    let first_path = plan.body.items.iter().find_map(|a| a.fs().map(|f| f.target.path.clone()));
    let free_before = first_path.as_ref().and_then(|p| b.files.free_space(p));
    let run = Run { stores, plan: &plan, operation_id: operation_id.clone(), ctx };
    let expires = parse_utc(&plan.body.expires_at).unwrap_or(i64::MIN);
    let mut results: Vec<ActionResult> = Vec::with_capacity(plan.body.items.len());
    let mut cancelled = false;
    let record = |r: &ActionResult| -> Result<(), ApplyError> {
        let status = serde_json::to_value(r.status).expect("status serializes");
        stores.audit.record_result(&operation_id, &r.action_id, status.as_str().expect("string"), &serde_json::to_string(r).expect("result serializes"), &utc_from_unix((ctx.now)()))?;
        Ok(())
    };
    for (idx, a) in plan.body.items.iter().enumerate() {
        let id = a.action_id();
        let m = &plan.meta[idx];
        // EXE-06: cancellation, expiry and revocation stop new actions only.
        let stop = if cancelled || (ctx.cancel)() {
            cancelled = true;
            Some(("cancelled", "operation cancelled before this action started"))
        } else if (ctx.now)() >= expires {
            Some(("plan_expired", "plan expired before this action started"))
        } else if !authorized((ctx.now)())? {
            Some(("authorization_revoked", "authorization was revoked or expired"))
        } else {
            None
        };
        if let Some((code, message)) = stop {
            results.push(res(id, ActionStatus::NotStarted, code, message, "not_applicable"));
            continue;
        }
        // A recycled item is several ordered actions: once one does not succeed, the rest of that item is kept.
        if let PlanAction::FsDelete(f) = a
            && let Some(group) = &f.trash_entry_id
            && plan.body.items[..idx].iter().zip(&results).any(|(p, r)| matches!(p, PlanAction::FsDelete(pf) if pf.trash_entry_id.as_ref() == Some(group)) && r.status != ActionStatus::Succeeded)
        {
            let r = res(id, ActionStatus::Skipped, "precondition_descendant_kept", "an earlier step of this recycled item did not complete; the rest of it is kept", "trash");
            record(&r)?;
            results.push(r);
            continue;
        }
        stores.audit.record_intent(&operation_id, id, &run.intent(a).to_string(), &utc_from_unix((ctx.now)()))?;
        if (ctx.fault)(FaultPoint::AfterIntent, id) {
            return Err(ApplyError::Crashed("after_intent".into()));
        }
        let r = match a {
            PlanAction::FsTrash(f) => trash_one(b, ctx, f, m),
            PlanAction::FsDelete(f) => purge_one(b, ctx, f, m),
            PlanAction::StartupToggle(s) => run.startup(b, s, m),
            a if crate::uninstall::handles(plan.body.plan_kind, a) => {
                let step = crate::uninstall::Step { stores, plan: &plan, index: idx, prior: &results, operation_id: &operation_id, ctx };
                crate::uninstall::run_action(b.uninstall, &step)
            }
            PlanAction::RegistryRestore(rr) => run.registry_restore(b, rr),
            PlanAction::ProcessEnd(pe) => crate::processes::run_end(b.processes, pe),
            PlanAction::ToolAdapter(t) if plan.body.plan_kind == PlanKind::Update => crate::updates::run_update(b.updates, t, m),
            other => res(id, ActionStatus::Skipped, "unsupported", format!("{} actions are not executed by this build", other.action_type().as_str()), "not_applicable"),
        };
        if (ctx.fault)(FaultPoint::AfterAction, id) {
            return Err(ApplyError::Crashed("after_action".into()));
        }
        record(&r)?;
        results.push(r);
    }
    finish(stores, b, &plan, &operation_id, &started_at, results, cancelled, free_before, "interactive_review", ctx)
}

#[allow(clippy::too_many_arguments)]
fn finish(
    stores: &Stores, b: &V2Backends<'_>, plan: &PlanV2, operation_id: &str, started_at: &str, results: Vec<ActionResult>, cancelled: bool,
    free_before: Option<u64>, auth_source: &str, ctx: &ApplyContext<'_>,
) -> Result<OperationReport, ApplyError> {
    for r in results.iter().filter(|r| r.status == ActionStatus::NotStarted) {
        stores.audit.record_result(operation_id, &r.action_id, "not_started", &serde_json::to_string(r).expect("result serializes"), &utc_from_unix((ctx.now)()))?;
    }
    let first_path = plan.body.items.iter().find_map(|a| a.fs().map(|f| f.target.path.clone()));
    let free_after = first_path.as_ref().and_then(|p| b.files.free_space(p));
    let (ok, skipped, failed, not_started, unknown) = summarize(&results);
    let size_of = |id: &str| plan.body.items.iter().find(|a| a.action_id() == id).map(PlanAction::logical_bytes).unwrap_or(0);
    let succeeded = |r: &&ActionResult| r.status == ActionStatus::Succeeded;
    let processed: u64 = results.iter().filter(succeeded).map(|r| size_of(&r.action_id)).sum();
    // Only permanent deletes free space by themselves; a Recycle Bin move frees nothing until the bin is emptied (REC-05).
    let freed: u64 = results.iter().filter(succeeded).filter(|r| plan.body.items.iter().any(|a| a.action_id() == r.action_id && matches!(a, PlanAction::FsDelete(_)))).map(|r| size_of(&r.action_id)).sum();
    let report = OperationReport {
        operation_id: operation_id.into(),
        plan_id: plan.body.plan_id.clone(),
        plan_digest: plan.digest.clone(),
        status: crate::uninstall::overall_status(plan, &results, final_status(&results, cancelled)).into(),
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
        estimated_freed_bytes: freed.to_string(),
        volume_free_before_bytes: free_before.map(|v| v.to_string()),
        volume_free_after_bytes: free_before.and(free_after).map(|v| v.to_string()),
        observed_free_delta_bytes: free_before.zip(free_after).map(|(bf, af)| (i128::from(af) - i128::from(bf)).to_string()),
        authorization_source: auth_source.into(),
    };
    // Locks stay while outcomes are unknown so no other plan touches those objects (EXE-04/05).
    if unknown == 0 {
        stores.audit.release_locks(operation_id)?;
    }
    stores.audit.finish_operation(operation_id, &report.status, &serde_json::to_string(&report).expect("report serializes"), &report.finished_at)?;
    Ok(report)
}

/// Resolve `plan/v2` actions that have an intent but no result after the host stopped. Never performs an action.
pub fn reconcile_operation_v2(stores: &Stores, b: &V2Backends<'_>, operation_id: &str, ctx: &ApplyContext<'_>) -> Result<OperationReport, ApplyError> {
    let plan = stores.audit.load_plan_v2(&stores.audit.operation_plan(operation_id)?)?;
    let journal = stores.audit.journal(operation_id)?;
    let mut results = Vec::with_capacity(plan.body.items.len());
    for (idx, a) in plan.body.items.iter().enumerate() {
        let id = a.action_id();
        let row = journal.iter().find(|r| r.action_id == id);
        let existing: Option<ActionResult> = row.and_then(|r| r.result_json.as_deref()).and_then(|j| serde_json::from_str(j).ok());
        let r = match (row, existing) {
            (_, Some(r)) if r.status != ActionStatus::Unknown => r,
            (None, _) => res(id, ActionStatus::NotStarted, "never_started", "no action was started", "not_applicable"),
            (Some(_), _) => {
                let verdict = reconcile_one(stores, b, a, &plan.meta[idx]);
                let r = match verdict {
                    Reconciled::NotPerformed => res(id, ActionStatus::NotStarted, "reconciled_not_performed", "the object or entry is still as planned", "not_applicable"),
                    Reconciled::Performed => {
                        let recovery = match a {
                            PlanAction::FsTrash(_) => "trash",
                            PlanAction::FsDelete(_) => "unavailable",
                            _ => "not_applicable",
                        };
                        res(id, ActionStatus::Succeeded, "reconciled_performed", "the action was found to have taken effect", recovery)
                    }
                    Reconciled::StillUnknown => {
                        let extra = if matches!(a, PlanAction::FsTrash(_)) { format!(" {UNKNOWN_GUIDANCE}") } else { String::new() };
                        res(id, ActionStatus::Unknown, "outcome_unconfirmed", format!("evidence insufficient; needs a person to check.{extra}"), "unknown")
                    }
                };
                let status = serde_json::to_value(r.status).expect("status serializes");
                stores.audit.record_result(operation_id, id, status.as_str().expect("string"), &serde_json::to_string(&r).expect("result serializes"), &utc_from_unix((ctx.now)()))?;
                r
            }
        };
        results.push(r);
    }
    let started = stores.audit.operation_report(operation_id)?.and_then(|j| serde_json::from_str::<OperationReport>(&j).ok()).map(|r| r.started_at)
        .unwrap_or_else(|| utc_from_unix((ctx.now)()));
    finish(stores, b, &plan, operation_id, &started, results, false, None, "reconciliation", ctx)
}

fn reconcile_one(stores: &Stores, b: &V2Backends<'_>, a: &PlanAction, m: &ActionMeta) -> Reconciled {
    match a {
        PlanAction::FsTrash(f) => b.files.reconcile(&observation_target(f, m, &[]), Mode::Trash, None),
        PlanAction::FsDelete(f) => b.files.reconcile(&observation_target(f, m, &[]), Mode::Delete, None),
        PlanAction::StartupToggle(s) => {
            let (Some(sa), Some(loc)) = (b.startup, &m.startup) else { return Reconciled::StillUnknown };
            let Ok(now) = sa.observe(loc) else { return Reconciled::StillUnknown };
            match s.operation {
                StartupOperation::Enable | StartupOperation::Disable => {
                    if s.observed_sha256.as_deref() == Some(observation_digest(loc, &now).as_str()) {
                        Reconciled::NotPerformed
                    } else if now.definition.is_some() && now.enabled() == (s.operation == StartupOperation::Enable) {
                        Reconciled::Performed
                    } else {
                        Reconciled::StillUnknown
                    }
                }
                StartupOperation::RemoveWithBackup => match &now.definition {
                    None => Reconciled::Performed,
                    Some(_) if s.observed_sha256.as_deref() == Some(observation_digest(loc, &now).as_str()) => Reconciled::NotPerformed,
                    Some(_) => Reconciled::StillUnknown,
                },
                StartupOperation::Restore => {
                    let Some(Ok((bk, _))) = s.backup_id.as_ref().map(|id| stores.audit.startup_backup(id)) else { return Reconciled::StillUnknown };
                    if now.approval == bk.prior_approval {
                        Reconciled::Performed
                    } else if now.approval == bk.written_approval {
                        Reconciled::NotPerformed
                    } else {
                        Reconciled::StillUnknown
                    }
                }
            }
        }
        PlanAction::RegistryRestore(r) => {
            let (Some(sa), Ok((bk, _))) = (b.startup, stores.audit.startup_backup(&r.backup_id)) else { return Reconciled::StillUnknown };
            match sa.observe(&bk.location) {
                Ok(now) if now.definition.is_none() => Reconciled::NotPerformed,
                Ok(now) if now.definition == bk.prior_definition => Reconciled::Performed,
                _ => Reconciled::StillUnknown,
            }
        }
        PlanAction::ProcessEnd(pe) => crate::processes::reconcile_end(b.processes, pe),
        _ => Reconciled::StillUnknown,
    }
}

/// Freeze the recorded prior states of a finished startup operation into a `recovery` plan and store it.
pub fn create_startup_restore_plan(stores: &Stores, operation_id: &str, plan_id: &str, now: i64, policy_digest: &str) -> Result<PlanV2, RestoreError> {
    let source = stores.audit.load_plan_v2(&stores.audit.operation_plan(operation_id)?)?;
    if source.body.plan_kind != PlanKind::Startup {
        return Err(RestoreError::NotStartup(source.body.plan_kind.as_str()));
    }
    let report: Option<OperationReport> = stores.audit.operation_report(operation_id)?.and_then(|j| serde_json::from_str(&j).ok());
    let Some(report) = report else { return Err(RestoreError::NotFinished(operation_id.into())) };
    let mut backups = Vec::new();
    for (action_id, backup, state) in stores.audit.startup_backups_for_operation(operation_id)? {
        let done = report.items.iter().any(|i| i.action_id == action_id && i.status == ActionStatus::Succeeded);
        if !done || state != "recorded" {
            continue;
        }
        let name = source.body.items.iter().position(|a| a.action_id() == action_id).and_then(|i| source.meta[i].note.clone()).unwrap_or_else(|| backup.startup_item_id.clone());
        backups.push((name, backup));
    }
    let plan = cm_core::startup::build_startup_recovery_plan(&backups, plan_id, now, policy_digest).map_err(|r| RestoreError::Refused(r.code, r.message))?;
    stores.audit.save_plan_v2(&plan)?;
    Ok(plan)
}

#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    #[error("operation belongs to a {0} plan; only startup operations are restored here")]
    NotStartup(&'static str),
    #[error("operation {0} has not finished")]
    NotFinished(String),
    #[error("{1}")]
    Refused(&'static str, String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl RestoreError {
    pub fn code(&self) -> cm_core::error::ErrorCode {
        use cm_core::error::ErrorCode;
        match self {
            RestoreError::NotStartup(_) | RestoreError::NotFinished(_) => ErrorCode::InvalidArgument,
            RestoreError::Refused("unsupported", _) => ErrorCode::Unsupported,
            RestoreError::Refused("internal_error", _) => ErrorCode::InternalError,
            RestoreError::Refused(..) => ErrorCode::InvalidArgument,
            RestoreError::Store(e) => e.code(),
        }
    }
}

/// History facts of a `plan/v2` operation: (`action`, availability, guidance) for protocol `operation_summary`.
pub fn history_facts(stores: &Stores, plan_id: &str, operation_id: &str) -> Option<(&'static str, &'static str, String)> {
    let plan = stores.audit.load_plan_v2(plan_id).ok()?;
    Some(match (plan.body.plan_kind, plan.body.items.first()?) {
        (PlanKind::ManualSelection, PlanAction::FsTrash(_)) => ("trash", "system_recycle_bin", format!("Moved to the Recycle Bin. {PUT_BACK_GUIDANCE} CleanManager cannot restore it.")),
        (PlanKind::ManualSelection, _) => ("delete", "unavailable", "Permanently deleted from the Recycle Bin; there is no undo.".into()),
        (PlanKind::Startup, _) => {
            let open = stores.audit.startup_backups_for_operation(operation_id).ok()?.iter().any(|(_, _, s)| s == "recorded");
            if open {
                ("startup", "in_app_restore", format!("Restorable here: `cleanmanager startup restore --operation {operation_id}` (skipped if the entry changed or was recreated since)."))
            } else {
                ("startup", "not_applicable", "Its prior state was restored since, or nothing was changed.".into())
            }
        }
        (PlanKind::Recovery, _) => ("restore", "not_applicable", "Prior startup states were put back where nothing had changed since.".into()),
        // MNT-07: maintenance runs are recorded per task (`optimize history`) and never undone.
        (PlanKind::Maintenance, _) => ("maintenance", "unavailable", "Maintenance tasks are not undone; `cleanmanager optimize history` lists each task's outcome and reason.".into()),
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
