//! `plan/v2` plans (manual selection to the Recycle Bin, startup changes and startup restores) in the
//! desktop review and apply flow (MTR-01..05, STA-01..04).
//!
//! Safety properties:
//! - The stored contract and `plan_kind` decide everything (`cm_app::plans::plan_type`); the WebView names a plan by id
//!   only. Before review, approval and application the kind rules and the body digest are checked again
//!   (`check_plan_v2`): a body that violates its kind is refused, never reinterpreted.
//! - Only an interactive review of the exact plan authorizes it: approval writes the same `KindReceipt`
//!   (`InteractiveReview`) as `cleanmanager plan review`, after the single-use review token issued with the plan's
//!   detail and the typed digest prefix. Policies are never consulted for these kinds.
//! - Plans that permanently delete (`fs_delete` of recycled objects) need an extra irreversibility confirmation the
//!   desktop does not offer yet: they are refused here (`needs_cli_confirmation`) and reviewed in a terminal.
//! - Application runs through `cm_app::plans::apply_plan_v2` with the same backends as `cleanmanager plan apply`
//!   (verified Windows file actions, startup changes through the process' own token, the protection checker before
//!   every action). A plan that needs an administrator is refused before anything starts when this process is not
//!   elevated; this tool never elevates itself. Kinds this window does not apply are refused (`unsupported`).

use cm_core::authz::{KindReceipt, ReceiptSource, current_principal};
use cm_core::error::ErrorCode;
use cm_core::plan::ItemKind;
use cm_core::plan::v2::{Hive, PlanAction, PlanKind, PlanProblem, PlanV2, check_plan_v2};
use cm_core::plan::parse_utc;
use cm_core::rules::Mode;
use cm_core::startup::StartupScope;
use cm_core::time::now_utc;
use cm_store::Stores;

use crate::cleanup::{AuthorizationDto, PlanDetailDto, PlanItemDto, cli, enum_text, new_id, now_unix, operation_ref};
use crate::dto::{IpcError, IpcResult, last_component};
use crate::host::{AppState, lock};

fn ipc(code: ErrorCode, reason: &'static str, message: impl Into<String>) -> IpcError {
    IpcError::new(code, reason, message)
}

/// Whether the plan permanently deletes anything (recycled objects).
pub fn has_permanent(plan: &PlanV2) -> bool {
    plan.body.items.iter().any(|a| matches!(a, PlanAction::FsDelete(_)))
}

/// Desktop name of a plan/v2 plan: `manual_trash`, `manual_purge`, `startup`, `startup_restore`, or the raw kind.
pub fn kind_name(plan: &PlanV2) -> &'static str {
    kind_word(plan.body.plan_kind, has_permanent(plan))
}

pub fn kind_word(kind: PlanKind, permanent: bool) -> &'static str {
    match kind {
        PlanKind::ManualSelection if permanent => "manual_purge",
        PlanKind::ManualSelection => "manual_trash",
        PlanKind::Startup => "startup",
        PlanKind::Recovery => "startup_restore",
        other => other.as_str(),
    }
}

/// Recycle Bin moves are shown with the trash mode; other kinds have no file mode.
pub fn mode_of(plan: &PlanV2) -> Option<Mode> {
    (kind_name(plan) == "manual_trash").then_some(Mode::Trash)
}

/// Kinds this window applies (a subset of what the build can execute; the rest stay with the CLI).
pub fn desktop_applies(kind: PlanKind) -> bool {
    cm_app::plans::executable(kind) && matches!(kind, PlanKind::ManualSelection | PlanKind::Startup | PlanKind::Recovery)
}

/// `expired`, `digest_mismatch`... (stale) or `plan_kind_violation`, when the core would refuse the plan now.
pub fn stale_reason(plan: &PlanV2, now: i64) -> Option<String> {
    match check_plan_v2(plan, now) {
        Ok(()) => None,
        Err(PlanProblem::Stale(r)) => Some(enum_text(&r)),
        Err(PlanProblem::Kind(_)) => Some("plan_kind_violation".into()),
    }
}

pub fn authorization(stores: &Stores, plan: &PlanV2, now: i64) -> IpcResult<AuthorizationDto> {
    let (status, r) = cm_app::plans::authorization(stores, plan, &current_principal(), now)?;
    Ok(AuthorizationDto {
        status,
        source: r.as_ref().map(|r| r.source),
        policy_id: None,
        policy_version: None,
        principal: r.as_ref().map(|r| r.principal.clone()),
        authorized_at: r.as_ref().map(|r| r.created_at.clone()),
        expires_at: r.as_ref().map(|r| r.expires_at.clone()),
    })
}

pub fn detail(s: &AppState, stores: &Stores, plan_id: &str, contract: String) -> IpcResult<PlanDetailDto> {
    let plan = stores.audit.load_plan_v2(plan_id)?;
    let now = now_unix();
    let authorization = authorization(stores, &plan, now)?;
    let stale = stale_reason(&plan, now);
    let expires = parse_utc(&plan.body.expires_at).unwrap_or(0);
    let mut alloc_known: u128 = 0;
    let mut alloc_unknown = 0u64;
    for f in plan.body.items.iter().filter_map(PlanAction::fs) {
        match f.target.allocated_bytes {
            Some(b) => alloc_known += u128::from(b.0),
            None => alloc_unknown += 1,
        }
    }
    let review_token = (authorization.status == "required" && stale.is_none()).then(|| lock(&s.reviews).issue(plan_id, &plan.digest));
    let scan_root_display = plan.body.scan_id.as_deref().and_then(|id| stores.index.scan_status(id).ok()).map(|v| v.root.display());
    let b = &plan.body;
    Ok(PlanDetailDto {
        plan_id: b.plan_id.clone(),
        contract,
        kind: kind_name(&plan),
        mode: mode_of(&plan),
        digest_prefix: plan.digest[..8].to_owned(),
        digest: plan.digest.clone(),
        created_at: b.created_at.clone(),
        expires_at: b.expires_at.clone(),
        expired: now >= expires,
        seconds_left: (expires - now).max(0),
        scan_id: b.scan_id.clone(),
        scan_revision: b.scan_revision,
        scope_id: b.scope_id.clone(),
        scan_root_display,
        rule_refs: vec![],
        rule_set_digest: Some(b.rule_set_digest.clone()),
        stale_reason: stale,
        item_count: b.items.len() as u64,
        total_logical_bytes: Some(b.total_logical().to_string()),
        allocated_known_bytes: Some(alloc_known.to_string()),
        allocated_unknown_count: alloc_unknown,
        default_selected_items: 0,
        authorization,
        policies: vec![],
        not_included: None,
        operation: operation_ref(stores, plan_id, &plan.digest)?,
        review_token,
        cli: cli(plan_id),
        requires_admin: b.requires_admin,
        catalog: None,
        plan_kind_v2: Some(b.plan_kind.as_str().to_owned()),
    })
}

fn scope_word(scope: StartupScope) -> &'static str {
    match scope {
        StartupScope::User => "user",
        StartupScope::System => "machine",
    }
}

fn hive_scope(h: Hive) -> &'static str {
    match h {
        Hive::Hkcu => "user",
        Hive::Hklm => "machine",
    }
}

/// `fs_trash`, `fs_delete`, `startup_disable`, `startup_enable`, `startup_remove_with_backup`, `startup_restore`,
/// `registry_restore`, or the protocol action type.
pub fn action_word(a: &PlanAction) -> String {
    match a {
        PlanAction::StartupToggle(t) => format!("startup_{}", enum_text(&t.operation)),
        other => other.action_type().as_str().to_owned(),
    }
}

/// One review row per action, in plan order (paging is the caller's).
pub fn item_rows(plan: &PlanV2) -> Vec<PlanItemDto> {
    plan.body
        .items
        .iter()
        .zip(&plan.meta)
        .map(|(a, m)| {
            let action_type = Some(action_word(a));
            let base = |name: String, path_display: String, item_kind: &str, scope: Option<&str>| PlanItemDto {
                action_id: a.action_id().to_owned(),
                name,
                path_display,
                logical_bytes: a.logical_bytes().to_string(),
                allocated_bytes: None,
                candidate_id: None,
                original_path_display: None,
                quarantine_id: None,
                item_kind: item_kind.to_owned(),
                action_type: action_type.clone(),
                scope: scope.map(str::to_owned),
            };
            match a {
                PlanAction::FsTrash(f) | PlanAction::FsDelete(f) | PlanAction::FsQuarantine(f) => {
                    let d = f.target.path.display();
                    let kind = if f.target.kind == ItemKind::Directory { "directory" } else { "file" };
                    let mut row = base(last_component(&d).to_owned(), d.clone(), kind, None);
                    row.allocated_bytes = f.target.allocated_bytes.map(|b| b.to_string());
                    row.candidate_id = f.target.candidate_id.clone();
                    if matches!(a, PlanAction::FsDelete(_)) {
                        // A recycled item's original location (text for people only).
                        row.original_path_display = m.note.clone();
                    }
                    row
                }
                PlanAction::StartupToggle(t) => {
                    let def = t.definition.clone().unwrap_or_default();
                    let name = m.note.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| def.clone());
                    base(name, def, "startup_item", Some(scope_word(t.scope)))
                }
                PlanAction::RegistryRestore(r) => {
                    let mut path = format!("{}\\{}", r.hive.as_str(), r.key_path);
                    if let Some(v) = &r.value_name {
                        path.push_str(&format!(" : {v}"));
                    }
                    let name = m.note.clone().or_else(|| r.value_name.clone()).unwrap_or_else(|| r.key_path.clone());
                    base(name, path, "registry_value", Some(hive_scope(r.hive)))
                }
                other => base(other.action_type().as_str().to_owned(), String::new(), "other", None),
            }
        })
        .collect()
}

/// Human approval from the review page: the same kind receipt `plan review` writes in a terminal.
pub fn approve(s: &AppState, stores: &Stores, plan_id: &str, token: &str, typed: &str) -> IpcResult<AuthorizationDto> {
    let now = now_unix();
    let plan = stores.audit.load_plan_v2(plan_id)?;
    match check_plan_v2(&plan, now) {
        Ok(()) => {}
        Err(PlanProblem::Kind(k)) => return Err(ipc(ErrorCode::PolicyDenied, "plan_kind_violation", format!("The plan violates its kind ({k}); it cannot be approved"))),
        Err(PlanProblem::Stale(r)) => return Err(ipc(ErrorCode::PlanStale, "plan_stale", format!("The plan can no longer be approved ({}); create a new plan", enum_text(&r)))),
    }
    // MTR-03: a permanent delete needs its own irreversibility confirmation, which only the terminal review asks for.
    if has_permanent(&plan) {
        return Err(ipc(
            ErrorCode::Unsupported,
            "needs_cli_confirmation",
            format!("This plan permanently deletes objects; review it in a terminal with `cleanmanager plan review {plan_id}`"),
        ));
    }
    lock(&s.reviews).consume(token, plan_id, &plan.digest, typed)?;
    let receipt = KindReceipt {
        receipt_id: new_id("r"),
        plan_id: plan.body.plan_id.clone(),
        plan_digest: plan.digest.clone(),
        plan_kind: plan.body.plan_kind,
        principal: current_principal(),
        source: ReceiptSource::InteractiveReview,
        created_at: now_utc(),
        expires_at: plan.body.expires_at.clone(),
        revoked_at: None,
    };
    stores.audit.save_kind_receipt(&receipt)?;
    authorization(stores, &plan, now)
}

/// Whether this process may change machine-wide entries (`None` where not determined).
#[cfg(windows)]
pub fn is_admin() -> Option<bool> {
    cm_platform::windows::startup::StartupEnv::from_process().ok().map(|e| e.is_admin)
}

#[cfg(not(windows))]
pub fn is_admin() -> Option<bool> {
    None
}

/// Refuse early, before a thread starts (the executor repeats every check itself).
pub fn precheck(stores: &Stores, plan_id: &str) -> IpcResult<()> {
    let now = now_unix();
    let plan = stores.audit.load_plan_v2(plan_id)?;
    match check_plan_v2(&plan, now) {
        Ok(()) => {}
        Err(PlanProblem::Kind(k)) => return Err(ipc(ErrorCode::PolicyDenied, "plan_kind_violation", format!("The plan violates its kind: {k}"))),
        Err(PlanProblem::Stale(r)) => return Err(cm_app::execute::ApplyError::Stale(r).into()),
    }
    if !desktop_applies(plan.body.plan_kind) {
        return Err(ipc(ErrorCode::Unsupported, "unsupported", format!("{} plans are not applied from this window; use `cleanmanager plan apply {plan_id}`", plan.body.plan_kind.as_str())));
    }
    // STA-02: system-wide changes need an elevated token; refused before anything starts.
    if plan.body.requires_admin && is_admin() != Some(true) {
        return Err(ipc(ErrorCode::PermissionRequired, "permission_required", cm_core::startup::ADMIN_REQUIRED));
    }
    if authorization(stores, &plan, now)?.status != "authorized" {
        return Err(cm_app::execute::ApplyError::NeedsApproval(plan_id.into()).into());
    }
    Ok(())
}

/// Run `f` with the platform backends of plan/v2 kinds, as `cleanmanager plan apply` builds them.
#[cfg(windows)]
fn with_backends<T>(stores: &Stores, f: impl FnOnce(&cm_app::plans::V2Backends<'_>) -> T) -> Result<T, String> {
    use cm_platform::windows::startup_actions::WindowsStartup;
    let env = cm_platform::windows::startup::StartupEnv::from_process()?;
    let startup = WindowsStartup { env };
    let files = cm_platform::windows::actions::WindowsActions::with_store(stores.dir.clone());
    let user = cm_platform::protection::load(&stores.dir);
    let holders = |paths: &[cm_core::path::RawPath]| -> Option<Vec<String>> {
        cm_platform::windows::restart_manager::holders(paths).ok().map(|h| h.iter().map(|x| x.describe()).collect())
    };
    let protect_item = |item_id: &str| -> Result<(), String> {
        let user = user.as_ref().map_err(|e| format!("protection_unavailable: {e}"))?;
        match cm_core::protect::evaluate_task(item_id, user) {
            cm_core::protect::ProtectionDecision::Allowed => Ok(()),
            denied => Err(format!("{denied:?}")),
        }
    };
    // Process, update and uninstall plans are not applied from this window (`desktop_applies`).
    let b = cm_app::plans::V2Backends { files: &files, startup: Some(&startup), holders: &holders, protect_item: &protect_item, processes: None, updates: None, uninstall: None };
    Ok(f(&b))
}

#[cfg(windows)]
fn backend_error(e: String) -> cm_app::execute::ApplyError {
    cm_app::execute::ApplyError::Store(cm_store::StoreError::InvalidArgument(e))
}

/// Apply a stored, authorized plan/v2 plan (called on the operation thread).
#[cfg(windows)]
pub fn apply(stores: &Stores, plan_id: &str, cancel: &dyn Fn() -> bool) -> Result<cm_app::execute::OperationReport, cm_app::execute::ApplyError> {
    use cm_core::host::HostProbe;
    let principal = current_principal();
    let host = cm_platform::host::OsHostProbe.current("desktop");
    // PRT-01: every file action (and everything a folder move carries) passes the protection re-check.
    let protect = cm_platform::protection::checker(&stores.dir, vec![]);
    let ctx = cm_app::execute::ApplyContext {
        principal: &principal,
        host: &host,
        current_rule_set_digest: "",
        now: &now_unix,
        cancel,
        fault: &|_, _| false,
        protect: &protect,
    };
    with_backends(stores, |b| cm_app::plans::apply_plan_v2(stores, b, plan_id, &ctx)).map_err(backend_error)?
}

/// Read-only reconciliation of a plan/v2 operation (never starts an action).
#[cfg(windows)]
pub fn reconcile(stores: &Stores, operation_id: &str) -> Result<cm_app::execute::OperationReport, cm_app::execute::ApplyError> {
    use cm_core::host::HostProbe;
    let principal = current_principal();
    let host = cm_platform::host::OsHostProbe.current("desktop");
    let ctx = cm_app::execute::ApplyContext {
        principal: &principal,
        host: &host,
        current_rule_set_digest: "",
        now: &now_unix,
        cancel: &|| false,
        fault: &|_, _| false,
        protect: &|_| Ok(()),
    };
    with_backends(stores, |b| cm_app::plans::reconcile_operation_v2(stores, b, operation_id, &ctx)).map_err(backend_error)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_words_follow_the_plan_kind_and_permanence() {
        assert_eq!(kind_word(PlanKind::ManualSelection, false), "manual_trash");
        assert_eq!(kind_word(PlanKind::ManualSelection, true), "manual_purge");
        assert_eq!(kind_word(PlanKind::Startup, false), "startup");
        assert_eq!(kind_word(PlanKind::Recovery, false), "startup_restore");
        assert_eq!(kind_word(PlanKind::Process, false), "process");
    }

    #[test]
    fn only_reviewed_kinds_the_window_knows_are_applied_here() {
        assert!(desktop_applies(PlanKind::ManualSelection));
        assert!(desktop_applies(PlanKind::Startup));
        assert!(desktop_applies(PlanKind::Recovery));
        assert!(!desktop_applies(PlanKind::Cleanup), "cleanup plans are plan/v1 here");
        assert!(!desktop_applies(PlanKind::Process));
        assert!(!desktop_applies(PlanKind::Uninstall));
    }

    #[test]
    fn startup_scopes_use_the_ui_words() {
        assert_eq!(scope_word(StartupScope::User), "user");
        assert_eq!(scope_word(StartupScope::System), "machine");
        assert_eq!(hive_scope(Hive::Hklm), "machine");
    }
}
