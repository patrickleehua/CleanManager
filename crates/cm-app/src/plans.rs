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
//!
//! Maintenance setting changes (parity 14.5): [`run_setting_change`] backs up the prior value before it writes (called by
//! the maintenance runner), [`create_settings_restore_plan`] freezes those backups into a `recovery` plan and its
//! `settings_change` actions put the prior value back only while the setting still has the value written.

use cm_core::authz::{KindReceipt, kind_receipt_valid};
use cm_core::execute::{ActionOutcome, ActionTarget, FileActions, LiveCheck, Reconciled, Recovery, TreeMember};
use cm_core::maintenance::TaskOutcome;
use cm_core::maintenance::settings::{SETTING_BACKUP_PREFIX, SettingBackup, SettingSpec, SettingsActions};
use cm_core::manual_trash::{BIN_NAME, PUT_BACK_GUIDANCE, UNKNOWN_GUIDANCE, in_recycle_bin};
use cm_core::path::RawPath;
use cm_core::plan::v2::{ActionMeta, FsAction, PlanAction, PlanKind, PlanProblem, PlanV2, SettingsChangeAction, StartupOperation, StartupToggleAction, check_plan_v2};
use cm_core::plan::{ItemKind, parse_utc};
use cm_core::rules::Mode;
use cm_core::startup::{RegValue, StartupActions, StartupBackup, StartupObservation, StartupOutcome, observation_digest};
use cm_core::time::utc_from_unix;
use cm_store::{StoreError, Stores};

use crate::execute::{ActionResult, ActionStatus, ApplyContext, ApplyError, Authorization, FaultPoint, OperationReport, final_status, summarize};

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
            // An object that is gone (e.g. another plan moved it) is missing, not protected (WFA-33).
            let (code, message) = crate::execute::skipped_protected(b.files, t, format!("{} is protected: {why}", t.path.display()));
            let message = if code == "protected" { message } else { format!("{} no longer exists at its planned place; nothing was moved", t.path.display()) };
            return res(id, ActionStatus::Skipped, &code, message, "not_applicable");
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
                return res(id, ActionStatus::Skipped, &format!("precondition_{code}"), format!("{other}; nothing was moved"), "not_applicable");
            }
        }
    };
    match outcome {
        ActionOutcome::Succeeded { recovery: Recovery::Trash } => res(id, ActionStatus::Succeeded, "done", format!("moved to the {BIN_NAME}. {PUT_BACK_GUIDANCE}"), "trash"),
        ActionOutcome::Succeeded { .. } => res(id, ActionStatus::Unknown, "outcome_unconfirmed", format!("the platform reported a move that was not to the {BIN_NAME}. {UNKNOWN_GUIDANCE}"), "unknown"),
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
        let (code, message) = crate::execute::skipped_protected(b.files, &target, why);
        return res(id, ActionStatus::Skipped, &code, message, "not_applicable");
    }
    let outcome = if a.target.kind == ItemKind::Directory {
        b.files.remove_empty_dir(&target)
    } else {
        match b.files.check(&target) {
            LiveCheck::Match => b.files.delete(&target),
            LiveCheck::InUse => return res(id, ActionStatus::Failed, "in_use", in_use_message((b.holders)(std::slice::from_ref(&a.target.path))), "trash"),
            other => {
                let code = serde_json::to_value(&other).ok().and_then(|v| v["check"].as_str().map(str::to_owned)).unwrap_or_else(|| "precondition".into());
                return res(id, ActionStatus::Skipped, &format!("precondition_{code}"), format!("{other}; nothing was deleted"), "trash");
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
    settings: Option<&'a dyn SettingsActions>,
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
        if loc.system_wide() && !sa.is_admin() {
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

    /// Put a backed-up setting value back (recovery plans; the action id is the backup id).
    fn settings_restore(&self, a: &SettingsChangeAction) -> ActionResult {
        let id = &a.action_id;
        let Some(sa) = self.settings else { return res(id, ActionStatus::Skipped, "unsupported", "setting restores are not available on this platform", "not_applicable") };
        let Some(spec) = cm_core::maintenance::settings::by_id(&a.setting_id) else {
            return res(id, ActionStatus::Skipped, "setting_unknown", format!("setting {} is not known to this build", a.setting_id), "not_applicable");
        };
        let (backup, state) = match self.stores.audit.setting_backup(id) {
            Ok(b) => b,
            Err(e) => return res(id, ActionStatus::Failed, "backup_unavailable", e.to_string(), "not_applicable"),
        };
        if backup.setting_id != a.setting_id || backup.written_value != a.from_value || backup.prior_value != a.to_value {
            return res(id, ActionStatus::Skipped, "backup_changed", "the recorded prior value differs from the planned one", "not_applicable");
        }
        if state != "recorded" {
            return res(id, ActionStatus::Skipped, "already_restored", format!("this backup is {state}; nothing was written"), "not_applicable");
        }
        let current = match sa.read(spec) {
            Ok(v) => v,
            Err(e) => return res(id, ActionStatus::Skipped, "state_unknown", format!("{e}; skipped rather than tried"), "not_applicable"),
        };
        if current == a.to_value {
            let _ = self.stores.audit.set_startup_backup_state(id, "restored");
            return res(id, ActionStatus::Succeeded, "already_prior", format!("{} already has its prior value {current}; nothing was written", spec.title), "not_applicable");
        }
        if current != a.from_value {
            return res(id, ActionStatus::Skipped, "changed_since", format!("{} was changed since ({current}, not {}); it is left as is", spec.title, a.from_value), "not_applicable");
        }
        if let Err(e) = sa.write(spec, &a.to_value) {
            return res(id, ActionStatus::Failed, "write_failed", e, "not_applicable");
        }
        match sa.read(spec) {
            Ok(v) if v == a.to_value => {
                let _ = self.stores.audit.set_startup_backup_state(id, "restored");
                res(id, ActionStatus::Succeeded, "restored", format!("{}: prior value {} put back ({})", spec.title, a.to_value, sa.location(spec)), "not_applicable")
            }
            Ok(v) => res(id, ActionStatus::Unknown, "outcome_unconfirmed", format!("written, but the setting now reads {v}"), "unknown"),
            Err(e) => res(id, ActionStatus::Unknown, "outcome_unconfirmed", format!("written, but the setting cannot be read back: {e}"), "unknown"),
        }
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
    apply_plan_v2_with_settings(stores, b, None, plan_id, ctx)
}

/// [`apply_plan_v2`] with the platform's settings backend, which `settings_change` restores of recovery plans need
/// (`None` = they are skipped `unsupported`).
pub fn apply_plan_v2_with_settings(
    stores: &Stores, b: &V2Backends<'_>, settings: Option<&dyn SettingsActions>, plan_id: &str, ctx: &ApplyContext<'_>,
) -> Result<OperationReport, ApplyError> {
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
    // PRT-02: every write stops while the protection list is unusable, before an operation is registered.
    crate::execute::require_protection(stores)?;
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
    let run = Run { stores, plan: &plan, operation_id: operation_id.clone(), ctx, settings };
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
            PlanAction::SettingsChange(sc) if plan.body.plan_kind == PlanKind::Recovery => run.settings_restore(sc),
            PlanAction::ToolAdapter(t) if plan.body.plan_kind == PlanKind::Update => crate::updates::run_update(b.updates, t, m),
            other => res(id, ActionStatus::Skipped, "unsupported", format!("{} actions are not executed by this build", other.action_type().as_str()), "not_applicable"),
        };
        if (ctx.fault)(FaultPoint::AfterAction, id) {
            return Err(ApplyError::Crashed("after_action".into()));
        }
        record(&r)?;
        results.push(r);
    }
    let auth = Authorization { source: "interactive_review", reconciled_at: None };
    finish(stores, b, &plan, &operation_id, &started_at, results, cancelled, free_before, &auth, ctx)
}

#[allow(clippy::too_many_arguments)]
fn finish(
    stores: &Stores, b: &V2Backends<'_>, plan: &PlanV2, operation_id: &str, started_at: &str, results: Vec<ActionResult>, cancelled: bool,
    free_before: Option<u64>, auth: &Authorization<'_>, ctx: &ApplyContext<'_>,
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

/// Resolve `plan/v2` actions that have an intent but no result after the host stopped. Never performs an action.
pub fn reconcile_operation_v2(stores: &Stores, b: &V2Backends<'_>, operation_id: &str, ctx: &ApplyContext<'_>) -> Result<OperationReport, ApplyError> {
    let plan = stores.audit.load_plan_v2(&stores.audit.operation_plan(operation_id)?)?;
    let journal = stores.audit.journal(operation_id)?;
    let prior = crate::execute::prior_report(stores, operation_id)?;
    let mut resolved = false;
    let mut results = Vec::with_capacity(plan.body.items.len());
    for (idx, a) in plan.body.items.iter().enumerate() {
        let id = a.action_id();
        let row = journal.iter().find(|r| r.action_id == id);
        let existing: Option<ActionResult> = row.and_then(|r| r.result_json.as_deref()).and_then(|j| serde_json::from_str(j).ok());
        let r = match (row, existing) {
            (_, Some(r)) if r.status != ActionStatus::Unknown => r,
            (None, _) => res(id, ActionStatus::NotStarted, "never_started", "no action was started", "not_applicable"),
            (Some(_), _) => {
                resolved = true;
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
    // Nothing was unconfirmed: the recorded report stands as it is.
    if let Some(report) = prior.as_ref().filter(|_| !resolved) {
        return Ok(report.clone());
    }
    let started = crate::execute::started_at(stores, operation_id, prior.as_ref(), ctx);
    // plan/v2 kinds are authorized only by an interactive review; a reconciliation never replaces that (WFA-40).
    let source = prior.as_ref().map(|r| r.authorization_source.clone()).filter(|s| s != crate::execute::LEGACY_RECONCILIATION_SOURCE)
        .unwrap_or_else(|| "interactive_review".into());
    let auth = Authorization { source: &source, reconciled_at: crate::execute::reconciled_at(resolved, prior.as_ref(), ctx) };
    finish(stores, b, &plan, operation_id, &started, results, false, None, &auth, ctx)
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
    #[error("operation belongs to a {0} plan; only maintenance setting changes are restored here")]
    NotMaintenance(&'static str),
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
            RestoreError::NotStartup(_) | RestoreError::NotMaintenance(_) | RestoreError::NotFinished(_) => ErrorCode::InvalidArgument,
            RestoreError::Refused("unsupported", _) => ErrorCode::Unsupported,
            RestoreError::Refused("internal_error", _) => ErrorCode::InternalError,
            RestoreError::Refused(..) => ErrorCode::InvalidArgument,
            RestoreError::Store(e) => e.code(),
        }
    }
}

/// Backup id of one setting change of an operation.
pub fn setting_backup_id(operation_id: &str, action_id: &str) -> String {
    format!("{SETTING_BACKUP_PREFIX}{}_{action_id}", operation_id.strip_prefix("op_").unwrap_or(operation_id))
}

/// What one maintenance setting change did (14.5, MNT-03): its outcome, code, reason and recovery availability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingRun {
    pub outcome: TaskOutcome,
    pub code: String,
    pub reason: String,
    /// `in_app_restore` once a value was written after its backup was stored.
    pub recovery: &'static str,
}

fn setting_run(outcome: TaskOutcome, code: &str, reason: impl Into<String>, recovery: &'static str) -> SettingRun {
    SettingRun { outcome, code: code.into(), reason: reason.into(), recovery }
}

/// Run one `settings_change` action of a maintenance plan: only while the setting still has the reviewed value, and
/// only after its prior value is stored as a backup. The write is read back before it counts as applied.
pub fn run_setting_change(
    stores: &Stores, settings: Option<&dyn SettingsActions>, operation_id: &str, plan_id: &str, a: &SettingsChangeAction, spec: &SettingSpec, now: &dyn Fn() -> i64,
) -> SettingRun {
    let Some(sa) = settings else { return setting_run(TaskOutcome::Unavailable, "settings_unavailable", "this platform cannot change this setting in this build", "not_applicable") };
    let current = match sa.read(spec) {
        Ok(v) => v,
        Err(e) => return setting_run(TaskOutcome::Skipped, "state_unknown", format!("{e}; skipped rather than tried"), "not_applicable"),
    };
    if current == a.to_value {
        return setting_run(TaskOutcome::Unchanged, "already_set", format!("{}: already {current}; nothing was written", spec.title), "not_applicable");
    }
    if current != a.from_value {
        return setting_run(
            TaskOutcome::Skipped,
            "changed_since_review",
            format!("{} is now {current}; the plan was reviewed when it was {}. Nothing was written; create a new plan", spec.title, a.from_value),
            "not_applicable",
        );
    }
    let backup = SettingBackup {
        backup_id: setting_backup_id(operation_id, &a.action_id),
        setting_id: spec.id.into(),
        location: sa.location(spec),
        prior_value: current,
        written_value: a.to_value.clone(),
        created_at: utc_from_unix(now()),
    };
    if let Err(e) = stores.audit.save_setting_backup(operation_id, &a.action_id, plan_id, &backup) {
        return setting_run(TaskOutcome::Failed, "backup_not_stored", format!("the prior value could not be backed up ({e}); nothing was written"), "not_applicable");
    }
    if let Err(e) = sa.write(spec, &a.to_value) {
        let _ = stores.audit.set_startup_backup_state(&backup.backup_id, "not_written");
        return setting_run(TaskOutcome::Failed, "write_failed", format!("{e}; the setting keeps {}", backup.prior_value), "not_applicable");
    }
    match sa.read(spec) {
        Ok(v) if v == a.to_value => setting_run(
            TaskOutcome::Applied,
            "set",
            format!(
                "{}: set to {} ({}); the prior value {} is backed up. Restore it with `cleanmanager optimize restore --operation {operation_id}`",
                spec.title, a.to_value, backup.location, backup.prior_value
            ),
            "in_app_restore",
        ),
        Ok(v) => setting_run(TaskOutcome::Failed, "not_persisted", format!("written, but the setting reads {v}; the backup of {} is kept", backup.prior_value), "in_app_restore"),
        Err(e) => setting_run(TaskOutcome::Failed, "not_persisted", format!("written, but it cannot be read back ({e}); the backup of {} is kept", backup.prior_value), "in_app_restore"),
    }
}

/// Freeze the recorded setting backups of a finished maintenance operation into a `recovery` plan and store it.
pub fn create_settings_restore_plan(stores: &Stores, operation_id: &str, plan_id: &str, now: i64, policy_digest: &str) -> Result<PlanV2, RestoreError> {
    let source = stores.audit.load_plan_v2(&stores.audit.operation_plan(operation_id)?)?;
    if source.body.plan_kind != PlanKind::Maintenance {
        return Err(RestoreError::NotMaintenance(source.body.plan_kind.as_str()));
    }
    let report: Option<OperationReport> = stores.audit.operation_report(operation_id)?.and_then(|j| serde_json::from_str(&j).ok());
    let Some(report) = report else { return Err(RestoreError::NotFinished(operation_id.into())) };
    let mut backups = Vec::new();
    for (action_id, backup_id, state) in stores.audit.backup_ids_for_operation(operation_id)? {
        let written = report.items.iter().any(|i| i.action_id == action_id && i.recovery == "in_app_restore");
        if !backup_id.starts_with(SETTING_BACKUP_PREFIX) || state != "recorded" || !written {
            continue;
        }
        backups.push(stores.audit.setting_backup(&backup_id)?.0);
    }
    let plan = cm_core::maintenance::settings::build_settings_recovery_plan(&backups, plan_id, now, policy_digest).map_err(|r| RestoreError::Refused(r.code, r.message))?;
    stores.audit.save_plan_v2(&plan)?;
    Ok(plan)
}

/// History facts of a `plan/v2` operation: (`action`, availability, guidance) for protocol `operation_summary`.
///
/// Every kind this build executes describes its own recovery (WFA-40): a Recycle Bin move is put back from the bin, a
/// recorded prior state (startup entry, setting, registry key or startup value of uninstall leftovers) is restorable
/// here while its backup is unused, an ended process, a vendor uninstaller and a package-manager update cannot be undone
/// by CleanManager, and a restore has nothing left to undo. `None` only when the plan cannot be read (or is of a kind
/// this build never executes).
pub fn history_facts(stores: &Stores, plan_id: &str, operation_id: &str) -> Option<(&'static str, &'static str, String)> {
    let plan = stores.audit.load_plan_v2(plan_id).ok()?;
    let report: Option<OperationReport> = stores.audit.operation_report(operation_id).ok().flatten().and_then(|j| serde_json::from_str(&j).ok());
    // Whether an action of this operation that `pick` selects took effect (a missing or unreadable report counts none).
    let succeeded = |pick: &dyn Fn(&PlanAction) -> bool| {
        report.as_ref().is_some_and(|r| {
            r.items.iter().any(|i| i.status == ActionStatus::Succeeded && plan.body.items.iter().any(|a| a.action_id() == i.action_id && pick(a)))
        })
    };
    let succeeded_id = |action_id: &str| report.as_ref().is_some_and(|r| r.items.iter().any(|i| i.action_id == action_id && i.status == ActionStatus::Succeeded));
    Some(match (plan.body.plan_kind, plan.body.items.first()?) {
        (PlanKind::ManualSelection, PlanAction::FsTrash(_)) => ("trash", "system_recycle_bin", format!("Moved to the {BIN_NAME}. {PUT_BACK_GUIDANCE} CleanManager cannot restore it.")),
        (PlanKind::ManualSelection, _) => ("delete", "unavailable", format!("Permanently deleted from the {BIN_NAME}; there is no undo.")),
        (PlanKind::Startup, _) => {
            let open = stores.audit.startup_backups_for_operation(operation_id).ok()?.iter().any(|(_, _, s)| s == "recorded");
            if open {
                ("startup", "in_app_restore", format!("Restorable here: `cleanmanager startup restore --operation {operation_id}` (skipped if the entry changed or was recreated since)."))
            } else {
                ("startup", "not_applicable", "Its prior state was restored since, or nothing was changed.".into())
            }
        }
        // A restore puts recorded prior states back; there is nothing left to undo. Named like the desktop names the kind.
        (PlanKind::Recovery, _) if plan.body.items.iter().any(|a| matches!(a, PlanAction::SettingsChange(_))) => {
            ("maintenance_restore", "not_applicable", "Prior setting values were put back where the setting had not changed since; nothing is left to undo.".into())
        }
        (PlanKind::Recovery, _) if plan.meta.iter().any(|m| m.uninstall.is_some()) => (
            "uninstall_restore",
            "not_applicable",
            "Registry keys and startup values were recreated from their backups where nothing had been recreated since; nothing is left to undo.".into(),
        ),
        (PlanKind::Recovery, _) => ("startup_restore", "not_applicable", "Prior startup states were put back where nothing had changed since; nothing is left to undo.".into()),
        // MNT-07: maintenance runs are recorded per task (`optimize history`) and never undone; only a recorded setting
        // change (14.5) has a restore entry.
        (PlanKind::Maintenance, _) => {
            let open = stores.audit.backup_ids_for_operation(operation_id).ok()?.iter().any(|(_, id, s)| id.starts_with(SETTING_BACKUP_PREFIX) && s == "recorded");
            if open {
                ("maintenance", "in_app_restore", format!("Setting changes are restorable here: `cleanmanager optimize restore --operation {operation_id}` (skipped if the setting changed since); other maintenance tasks are not undone."))
            } else {
                ("maintenance", "unavailable", "Maintenance tasks are not undone; `cleanmanager optimize history` lists each task's outcome and reason.".into())
            }
        }
        // STS-04: an ended process cannot be brought back.
        (PlanKind::Process, _) => ("process", "unavailable", "An ended process cannot be brought back; start the program again if you need it. Nothing else was changed.".into()),
        // UPD-02/03: the package manager installed the update; CleanManager keeps no copy of the previous version.
        (PlanKind::Update, _) => (
            "update",
            "unavailable",
            "Updates are installed by the package manager that manages the app (winget, Scoop, Homebrew); CleanManager does not roll them back. Install an earlier version with that package manager if you need it.".into(),
        ),
        // UNI-03: stage 1 runs each application's own route. A vendor uninstaller or package manager cannot be undone by
        // CleanManager (only a reinstall brings the application back); the macOS bundle route moved the bundle to the
        // Trash, where it can be put back. Leftovers are a separately reviewed stage-2 plan.
        (PlanKind::Uninstall, _) if !crate::uninstall::planned_apps(&plan).is_empty() => {
            let to_bin = |a: &PlanAction| matches!(a, PlanAction::ToolAdapter(t) if t.adapter_id == cm_core::uninstall::AdapterKind::MacTrashAppBundle.adapter_id());
            let bundle = succeeded(&to_bin);
            let vendor = succeeded(&|a| matches!(a, PlanAction::ToolAdapter(_)) && !to_bin(a));
            let leftovers = format!("Review what it left with `cleanmanager apps uninstall leftovers --operation {operation_id}`.");
            match (bundle, vendor) {
                (true, others) => (
                    "uninstall",
                    "system_recycle_bin",
                    format!(
                        "The application bundle was moved to the {BIN_NAME}. {PUT_BACK_GUIDANCE}{} {leftovers}",
                        if others { " Applications removed by their own uninstaller or package manager cannot be undone by CleanManager; reinstall them if you need them." } else { "" }
                    ),
                ),
                (false, true) => (
                    "uninstall",
                    "unavailable",
                    format!("Uninstalled by the application's own uninstaller or package manager; CleanManager cannot undo it (reinstall the application from its vendor if you need it). {leftovers}"),
                ),
                (false, false) => ("uninstall", "not_applicable", "No uninstaller completed; the applications stay installed and nothing was removed.".into()),
            }
        }
        // UNI-06: leftovers — files in the Recycle Bin, registry keys and startup values restorable from their backups.
        (PlanKind::Uninstall, _) => {
            let backups = stores.audit.backup_ids_for_operation(operation_id).ok()?;
            let restorable = backups.iter().any(|(action_id, _, state)| state == "recorded" && succeeded_id(action_id));
            let trashed = succeeded(&|a| matches!(a, PlanAction::FsTrash(_)));
            let bin = if trashed { format!(" Files and folders were moved to the {BIN_NAME}: {PUT_BACK_GUIDANCE}") } else { String::new() };
            if restorable {
                (
                    "uninstall_leftovers",
                    "in_app_restore",
                    format!("Registry keys and startup values are restorable here from their backups: `cleanmanager apps uninstall restore --operation {operation_id}` (skipped where something was recreated since; never overwrites).{bin}"),
                )
            } else if trashed {
                ("uninstall_leftovers", "system_recycle_bin", format!("Moved to the {BIN_NAME}. {PUT_BACK_GUIDANCE} CleanManager cannot restore files from it."))
            } else {
                ("uninstall_leftovers", "not_applicable", "Its registry keys and startup values were restored since, or nothing was removed.".into())
            }
        }
        // Never executed by this build (`executable`): no operation of this kind exists.
        (PlanKind::Cleanup, _) => return None,
    })
}

#[cfg(test)]
mod tests;
