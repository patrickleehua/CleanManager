//! `plan/v2` plans in the desktop review and apply flow: manual selection to the Recycle Bin, startup changes and
//! restores (MTR-01..05, STA-01..04), application uninstall and its leftovers (UNI-01..10), app updates (UPD-01..04),
//! ending one process (STS-04) and maintenance runs (MNT-01..07).
//!
//! Safety properties:
//! - The stored contract and `plan_kind` decide everything (`cm_app::plans::plan_type`); the WebView names a plan by id
//!   only. Before review, approval and application the kind rules and the body digest are checked again
//!   (`check_plan_v2`): a body that violates its kind is refused, never reinterpreted.
//! - Only an interactive review of the exact plan authorizes it: approval writes the same `KindReceipt`
//!   (`InteractiveReview`) as `cleanmanager plan review`, after the single-use review token issued with the plan's
//!   detail and the typed digest prefix. Policies are never consulted for these kinds.
//! - Two kinds of plan need a confirmation only a terminal asks for and are refused here (`needs_cli_confirmation`):
//!   plans that permanently delete (`fs_delete` of recycled objects) and uninstall plans that may end still-running
//!   processes after a person types END (`app_quit` with `force: ask_user`, created by `--end-running`).
//! - Application runs through `cm_app::plans::apply_plan_v2_with_settings` with the same backends as `cleanmanager plan
//!   apply` (`crate::plat::with_backends`: the platform's file actions, startup changes through the process' own token
//!   on Windows, the protection checker before every action, process re-identification, signature-verified updates,
//!   verified uninstallers; on macOS also the settings backend that puts back a maintenance run's setting changes). The
//!   desktop's uninstall backend never confirms ending a process. A plan that needs an administrator is refused before
//!   anything starts when this process is not elevated (always on macOS); this tool never elevates itself for those.
//! - Maintenance plans run through `cm_app::maintenance::apply_maintenance_plan`: administrator tasks are listed
//!   before any task runs and Windows asks once (UAC) to start the same short-lived, single-grant worker the CLI
//!   starts (this executable with the hidden worker argument, see `main.rs`); declining skips only those tasks. macOS
//!   has no privilege path in this build: administrator tasks are skipped `elevation_unavailable`.

use cm_core::authz::{KindReceipt, ReceiptSource, current_principal};
use cm_core::error::ErrorCode;
use cm_core::plan::ItemKind;
use cm_core::plan::v2::{Hive, PlanAction, PlanKind, PlanProblem, PlanV2, QuitForce, check_plan_v2};
use cm_core::plan::parse_utc;
use cm_core::rules::Mode;
use cm_core::startup::StartupScope;
use cm_core::time::now_utc;
use cm_store::Stores;
use serde_json::json;

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

/// UNI-02: an uninstall plan that may end still-running processes once a person types END in a terminal.
pub fn may_end_processes(plan: &PlanV2) -> bool {
    plan.body.items.iter().any(|a| matches!(a, PlanAction::AppQuit(q) if q.force == QuitForce::AskUser))
}

/// Stage 1 of an uninstall (the verified uninstaller), as opposed to its separately reviewed leftovers.
pub fn runs_uninstaller(plan: &PlanV2) -> bool {
    plan.body.plan_kind == PlanKind::Uninstall && plan.body.items.iter().any(|a| matches!(a, PlanAction::ToolAdapter(_)))
}

/// Desktop name of a plan/v2 plan: `manual_trash`, `manual_purge`, `startup`, `startup_restore`, `uninstall`,
/// `uninstall_leftovers`, `uninstall_restore`, `update`, `process`, `maintenance`, `maintenance_restore` (setting
/// changes of a maintenance run put back, 14.5), or the raw kind.
pub fn kind_name(plan: &PlanV2) -> &'static str {
    match plan.body.plan_kind {
        PlanKind::Uninstall if !runs_uninstaller(plan) => "uninstall_leftovers",
        PlanKind::Recovery if plan.meta.iter().any(|m| m.uninstall.is_some()) => "uninstall_restore",
        PlanKind::Recovery if plan.body.items.iter().any(|a| matches!(a, PlanAction::SettingsChange(_))) => "maintenance_restore",
        kind => kind_word(kind, has_permanent(plan)),
    }
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
    kind == PlanKind::Maintenance
        || (cm_app::plans::executable(kind)
            && matches!(kind, PlanKind::ManualSelection | PlanKind::Startup | PlanKind::Recovery | PlanKind::Process | PlanKind::Update | PlanKind::Uninstall))
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

/// What a plan of these kinds means for the review page (beyond its rows).
fn kind_detail(s: &AppState, plan: &PlanV2) -> Option<serde_json::Value> {
    let noted = lock(&s.suite).plan_notes.get(&plan.body.plan_id).cloned();
    let b = &plan.body;
    let v = match b.plan_kind {
        PlanKind::Uninstall => {
            let app = plan.meta.iter().find_map(|m| match &m.uninstall {
                Some(cm_core::uninstall::UninstallMeta::App { identity, adapter, signer, .. }) => Some(json!({
                    "name": identity.name, "publisher": identity.publisher, "version": identity.version, "source": identity.source,
                    "adapter": adapter.display, "signer": signer,
                })),
                _ => None,
            });
            // UNI-11: every application of a batch, in plan order, with the actions bound to it (review groups by them).
            let apps: Vec<serde_json::Value> = cm_app::uninstall::planned_apps(plan)
                .into_iter()
                .map(|a| {
                    let (adapter, signer) = plan
                        .meta
                        .iter()
                        .find_map(|m| match &m.uninstall {
                            Some(cm_core::uninstall::UninstallMeta::App { identity, adapter, signer, .. }) if identity.instance_id == a.identity.instance_id => {
                                Some((adapter.display.clone(), signer.clone()))
                            }
                            _ => None,
                        })
                        .unwrap_or_default();
                    json!({
                        "app_instance_id": a.identity.instance_id, "name": a.identity.name, "publisher": a.identity.publisher, "version": a.identity.version,
                        "source": a.identity.source, "adapter": adapter, "signer": signer, "action_ids": a.action_ids,
                    })
                })
                .collect();
            json!({
                "stage": if runs_uninstaller(plan) { "uninstall" } else { "leftovers" },
                "app": app,
                "apps": apps,
                "may_end_processes": may_end_processes(plan),
                "registry_keys": b.items.iter().filter(|a| matches!(a, PlanAction::RegistryBackupDelete(_))).count(),
                "noted": noted,
            })
        }
        PlanKind::Maintenance => {
            let admin = b.items.iter().filter(|a| a.needs_admin()).count();
            json!({"admin_tasks": admin, "is_admin": is_admin(), "noted": noted})
        }
        PlanKind::Update | PlanKind::Process => json!({"noted": noted}),
        PlanKind::Recovery if matches!(kind_name(plan), "uninstall_restore" | "maintenance_restore") => json!({"noted": noted}),
        _ => return None,
    };
    Some(v)
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
        terminal_only: terminal_only(&plan).map(str::to_owned),
        kind_detail: kind_detail(s, &plan),
    })
}

/// Why only a terminal can approve this plan (`permanent_delete`, `end_processes`), if so.
pub fn terminal_only(plan: &PlanV2) -> Option<&'static str> {
    if has_permanent(plan) {
        Some("permanent_delete")
    } else if may_end_processes(plan) {
        Some("end_processes")
    } else {
        None
    }
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

/// The app an uninstall action is bound to (from its execution data), if recorded.
fn uninstall_app(m: &cm_core::plan::v2::ActionMeta) -> Option<&cm_core::uninstall::AppIdentity> {
    match &m.uninstall {
        Some(cm_core::uninstall::UninstallMeta::App { identity, .. }) => Some(identity),
        _ => None,
    }
}

/// One review row per action, in plan order (paging is the caller's).
pub fn item_rows(plan: &PlanV2) -> Vec<PlanItemDto> {
    let kind = plan.body.plan_kind;
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
                detail: None,
            };
            match a {
                PlanAction::FsTrash(f) | PlanAction::FsDelete(f) | PlanAction::FsQuarantine(f) => {
                    let d = f.target.path.display();
                    let item_kind = if f.target.kind == ItemKind::Directory { "directory" } else { "file" };
                    let mut row = base(last_component(&d).to_owned(), d.clone(), item_kind, None);
                    row.allocated_bytes = f.target.allocated_bytes.map(|b| b.to_string());
                    row.candidate_id = f.target.candidate_id.clone();
                    if matches!(a, PlanAction::FsDelete(_)) {
                        // A recycled item's original location (text for people only).
                        row.original_path_display = m.note.clone();
                    }
                    if kind == PlanKind::Uninstall {
                        // Which leftover group the object came from (text for people only).
                        row.detail = m.note.as_ref().map(|n| json!({"leftover": n}));
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
                    let mut row = base(name, path, if r.value_name.is_some() { "registry_value" } else { "registry_key" }, Some(hive_scope(r.hive)));
                    row.detail = Some(json!({"on_existing": r.on_existing}));
                    row
                }
                PlanAction::AppQuit(q) => {
                    let name = uninstall_app(m).map(|i| i.name.clone()).unwrap_or_else(|| q.app_identity.clone());
                    let mut row = base(name, q.app_identity.clone(), "app_quit", None);
                    row.detail = Some(json!({"force": q.force, "app_instance_id": q.app_instance_id}));
                    row
                }
                PlanAction::ToolAdapter(t) if kind == PlanKind::Update => {
                    let target = m.note.as_deref().and_then(|n| serde_json::from_str::<cm_core::updates::UpdateTarget>(n).ok());
                    let mut row = match &target {
                        Some(x) => base(x.name.clone(), format!("{} → {}", x.from_version.as_deref().unwrap_or("?"), x.to_version), "app_update", None),
                        None => base(t.adapter_id.clone(), t.argv.join(" "), "app_update", None),
                    };
                    row.detail = Some(json!({
                        "adapter_id": t.adapter_id,
                        "command": format!("{} {}", last_component(&t.executable.path.display()), t.argv.join(" ")),
                        "channel": target.as_ref().map(|x| x.channel),
                        "package_id": target.as_ref().map(|x| x.package_id.clone()),
                        "installed_signer": target.as_ref().and_then(|x| x.installed_signer.clone()),
                        "official_url": target.as_ref().and_then(|x| x.official_url.clone()),
                        "timeout_seconds": t.timeout_seconds,
                    }));
                    row
                }
                PlanAction::ToolAdapter(t) => {
                    let name = uninstall_app(m).map(|i| i.name.clone()).unwrap_or_else(|| t.adapter_id.clone());
                    let mut row = base(name, m.note.clone().unwrap_or_else(|| t.executable.path.display()), "uninstaller", None);
                    row.detail = Some(json!({
                        "adapter_id": t.adapter_id,
                        "program": t.executable.path.display(),
                        "timeout_seconds": t.timeout_seconds,
                        "app_instance_id": uninstall_app(m).map(|i| i.instance_id.clone()),
                    }));
                    row
                }
                PlanAction::RegistryBackupDelete(r) => {
                    let path = format!("{}\\{}", r.hive.as_str(), r.key_path);
                    let name = r.key_path.rsplit('\\').next().unwrap_or(&r.key_path).to_owned();
                    let mut row = base(name, path, "registry_key", Some(hive_scope(r.hive)));
                    row.detail = Some(json!({"owner_evidence": r.owner_evidence, "backup_required": r.backup_required}));
                    row
                }
                PlanAction::ProcessEnd(p) => {
                    let image = p.image_path.as_ref().map(cm_core::path::RawPath::display).unwrap_or_default();
                    let mut row = base(format!("{} (PID {})", p.name, p.pid), image, "process", None);
                    row.detail = Some(json!({"pid": p.pid, "started_at": p.started_at, "image_bound": p.image_sha256.is_some(), "note": m.note}));
                    row
                }
                PlanAction::MaintenanceTask(t) => {
                    let title = cm_core::maintenance::catalog(cm_core::maintenance::TaskPlatform::current())
                        .iter()
                        .find(|s| s.id == t.task_id)
                        .map(|s| s.title.to_owned())
                        .unwrap_or_else(|| t.task_id.clone());
                    let mut row = base(title, format!("{}@{}", t.task_id, t.task_version), "maintenance_task", Some(if t.requires_admin { "machine" } else { "user" }));
                    row.detail = Some(json!({"task_id": t.task_id, "task_version": t.task_version, "requires_admin": t.requires_admin}));
                    row
                }
                PlanAction::SettingsChange(c) => {
                    let name = m.note.clone().unwrap_or_else(|| c.setting_id.clone());
                    let mut row = base(name, format!("{}: {} → {}", c.setting_id, c.from_value, c.to_value), "setting", Some("user"));
                    row.detail = Some(json!({"setting_id": c.setting_id, "from_value": c.from_value, "to_value": c.to_value, "restore_on_exit": c.restore_on_exit}));
                    row
                }
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
    // MTR-03 / UNI-02: confirmations only a terminal review asks for.
    match terminal_only(&plan) {
        Some("permanent_delete") => {
            return Err(ipc(
                ErrorCode::Unsupported,
                "needs_cli_confirmation",
                format!("This plan permanently deletes objects; review it in a terminal with `cleanmanager plan review {plan_id}`"),
            ));
        }
        Some(_) => {
            return Err(ipc(
                ErrorCode::Unsupported,
                "needs_cli_confirmation",
                format!("This plan may end still-running processes after you type END; review and apply it in a terminal with `cleanmanager plan review {plan_id}`"),
            ));
        }
        None => {}
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
pub fn is_admin() -> Option<bool> {
    crate::plat::is_admin()
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
    // UNI-02: ending processes needs END typed in a terminal; this window never confirms it.
    if may_end_processes(&plan) {
        return Err(ipc(ErrorCode::Unsupported, "needs_cli_confirmation", format!("This plan may end still-running processes; apply it in a terminal with `cleanmanager plan apply {plan_id}`")));
    }
    // STA-02: system-wide changes need an elevated token; refused before anything starts. Maintenance plans ask
    // Windows once for their administrator tasks instead (MNT-04).
    if plan.body.plan_kind != PlanKind::Maintenance && plan.body.requires_admin && is_admin() != Some(true) {
        return Err(ipc(ErrorCode::PermissionRequired, "permission_required", cm_core::startup::ADMIN_REQUIRED));
    }
    if authorization(stores, &plan, now)?.status != "authorized" {
        return Err(cm_app::execute::ApplyError::NeedsApproval(plan_id.into()).into());
    }
    Ok(())
}

fn backend_error(e: String) -> cm_app::execute::ApplyError {
    cm_app::execute::ApplyError::Store(cm_store::StoreError::InvalidArgument(e))
}

/// Apply a stored, authorized plan/v2 plan (called on the operation thread).
pub fn apply(stores: &Stores, plan_id: &str, cancel: &dyn Fn() -> bool) -> Result<cm_app::execute::OperationReport, cm_app::execute::ApplyError> {
    use cm_core::host::HostProbe;
    if stores.audit.load_plan_v2(plan_id)?.body.plan_kind == PlanKind::Maintenance {
        return crate::system::apply_maintenance(stores, plan_id, cancel);
    }
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
    // Recovery plans may restore maintenance setting changes (14.5) through the settings backend (macOS only).
    let settings = crate::plat::settings_backend();
    crate::plat::with_backends(stores, |b| cm_app::plans::apply_plan_v2_with_settings(stores, b, settings.as_deref(), plan_id, &ctx)).map_err(backend_error)?
}

/// Read-only reconciliation of a plan/v2 operation (never starts an action).
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
    crate::plat::with_backends(stores, |b| cm_app::plans::reconcile_operation_v2(stores, b, operation_id, &ctx)).map_err(backend_error)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use cm_core::plan::v2::{ActionMeta, AppQuitAction, ExecutableRef, HeaderInput, MaintenanceTaskAction, NO_RULES_DIGEST, ProcessEndAction, RegistryBackupDeleteAction, ToolAdapterAction, build};

    fn header(kind: PlanKind) -> HeaderInput<'static> {
        HeaderInput { plan_id: "p_test", plan_kind: kind, created_unix: 1_800_000_000, scope_id: None, scan_id: None, scan_revision: None, rule_set_digest: NO_RULES_DIGEST, policy_digest: "pd" }
    }

    fn exe() -> ExecutableRef {
        ExecutableRef { path: cm_core::path::RawPath::windows("C:\\Tools\\unins000.exe".encode_utf16().collect()).unwrap(), sha256: "a".repeat(64) }
    }

    fn record() -> cm_core::uninstall::UninstallRecord {
        serde_json::from_value(serde_json::json!({"instance_id": "app_x", "name": "Tools", "source": "registry_uninstall", "system_component": false})).expect("record shape")
    }

    fn choice() -> cm_core::uninstall::AdapterChoice {
        cm_core::uninstall::AdapterChoice {
            kind: cm_core::uninstall::AdapterKind::RegisteredCommand, program: "C:\\Tools\\unins000.exe".into(), argv: vec![], raw_args: false, publisher_required: None,
            microsoft_signed: false, display: "Tools uninstaller".into(), keeps_own_ui: true,
        }
    }

    fn app_meta() -> ActionMeta {
        let rec = record();
        let meta = cm_core::uninstall::UninstallMeta::App { identity: cm_core::uninstall::AppIdentity::of(&rec), record: Box::new(rec), adapter: choice(), signer: None };
        ActionMeta { note: Some("Tools uninstaller".into()), uninstall: Some(meta), ..Default::default() }
    }

    fn quit(force: QuitForce) -> PlanAction {
        PlanAction::AppQuit(AppQuitAction { action_id: "q0000000".into(), app_instance_id: "app_x".into(), app_identity: "C:\\Tools".into(), force })
    }

    fn adapter() -> PlanAction {
        let c = choice();
        PlanAction::ToolAdapter(ToolAdapterAction {
            action_id: "u0000001".into(), adapter_id: c.kind.adapter_id().into(), adapter_version: "1.0.0".into(), executable: exe(),
            argv: c.argv.clone(), preview_digest: cm_core::uninstall::adapter_digest(&record(), &c), timeout_seconds: 3600,
        })
    }

    fn key_meta(path: &str) -> ActionMeta {
        let location = cm_core::uninstall::RegLocation { root: String::new(), hive: Hive::Hkcu, view: cm_core::startup::RegView::Default, key_path: path.into() };
        ActionMeta { uninstall: Some(cm_core::uninstall::UninstallMeta::RegistryKey { location }), ..Default::default() }
    }

    #[test]
    fn kind_words_follow_the_plan_kind_and_permanence() {
        assert_eq!(kind_word(PlanKind::ManualSelection, false), "manual_trash");
        assert_eq!(kind_word(PlanKind::ManualSelection, true), "manual_purge");
        assert_eq!(kind_word(PlanKind::Startup, false), "startup");
        assert_eq!(kind_word(PlanKind::Recovery, false), "startup_restore");
        assert_eq!(kind_word(PlanKind::Process, false), "process");
        assert_eq!(kind_word(PlanKind::Maintenance, false), "maintenance");
        assert_eq!(kind_word(PlanKind::Update, false), "update");
    }

    #[test]
    fn reviewed_kinds_the_window_knows_are_applied_here() {
        assert!(desktop_applies(PlanKind::ManualSelection));
        assert!(desktop_applies(PlanKind::Startup));
        assert!(desktop_applies(PlanKind::Recovery));
        assert!(desktop_applies(PlanKind::Process));
        assert!(desktop_applies(PlanKind::Update));
        assert!(desktop_applies(PlanKind::Uninstall));
        assert!(desktop_applies(PlanKind::Maintenance));
        assert!(!desktop_applies(PlanKind::Cleanup), "cleanup plans are plan/v1 here");
    }

    #[test]
    fn uninstall_stages_and_the_end_running_variant_are_told_apart() {
        let meta = || vec![app_meta(), app_meta()];
        let stage1 = build(&header(PlanKind::Uninstall), vec![quit(QuitForce::Never), adapter()], meta()).unwrap();
        assert_eq!(kind_name(&stage1), "uninstall");
        assert!(!may_end_processes(&stage1));
        assert_eq!(terminal_only(&stage1), None);
        let rows = item_rows(&stage1);
        assert_eq!((rows[0].item_kind.as_str(), rows[1].item_kind.as_str()), ("app_quit", "uninstaller"));
        assert_eq!(rows[0].name, "Tools");
        assert_eq!(rows[0].detail.as_ref().unwrap()["force"], "never");
        assert_eq!(rows[1].path_display, "Tools uninstaller");
        // UNI-11: review rows name the application they belong to, so a batch is reviewed application by application.
        assert_eq!((rows[0].detail.as_ref().unwrap()["app_instance_id"].as_str(), rows[1].detail.as_ref().unwrap()["app_instance_id"].as_str()), (Some("app_x"), Some("app_x")));

        let forced = build(&header(PlanKind::Uninstall), vec![quit(QuitForce::AskUser), adapter()], meta()).unwrap();
        assert!(may_end_processes(&forced), "the --end-running variant stays with the terminal");
        assert_eq!(terminal_only(&forced), Some("end_processes"));

        let key = PlanAction::RegistryBackupDelete(RegistryBackupDeleteAction {
            action_id: "k0000000".into(), hive: Hive::Hkcu, key_path: "Software\\Vendor\\Tools".into(), value_name: None, owner_evidence: "exact product key".into(),
            backup_required: true, observed_sha256: Some("c".repeat(64)),
        });
        let leftovers = build(&header(PlanKind::Uninstall), vec![key], vec![key_meta(r"Software\Vendor\Tools")]).unwrap();
        assert_eq!(kind_name(&leftovers), "uninstall_leftovers");
        let row = &item_rows(&leftovers)[0];
        assert_eq!((row.name.as_str(), row.path_display.as_str(), row.scope.as_deref()), ("Tools", "HKCU\\Software\\Vendor\\Tools", Some("user")));
        assert_eq!(row.detail.as_ref().unwrap()["backup_required"], true);
    }

    #[test]
    fn process_and_maintenance_rows_name_what_they_act_on() {
        let p = PlanAction::ProcessEnd(ProcessEndAction { action_id: "e01".into(), pid: 4242, started_at: "2026-10-15T00:00:00Z".into(), name: "code.exe".into(), image_path: None, image_sha256: None });
        let plan = build(&header(PlanKind::Process), vec![p], vec![ActionMeta::default()]).unwrap();
        let row = &item_rows(&plan)[0];
        assert_eq!((row.name.as_str(), row.item_kind.as_str()), ("code.exe (PID 4242)", "process"));
        let t = PlanAction::MaintenanceTask(MaintenanceTaskAction { action_id: "m01".into(), task_id: "windows.dns-flush".into(), task_version: "1.0.0".into(), requires_admin: false, volume: None });
        let plan = build(&header(PlanKind::Maintenance), vec![t], vec![ActionMeta::default()]).unwrap();
        let row = &item_rows(&plan)[0];
        assert_eq!((row.path_display.as_str(), row.item_kind.as_str(), row.scope.as_deref()), ("windows.dns-flush@1.0.0", "maintenance_task", Some("user")));
    }

    /// 14.5: putting back a maintenance run's setting changes is its own kind on the review page, one row per setting.
    #[test]
    fn a_settings_restore_is_named_and_listed_per_setting() {
        use cm_core::maintenance::settings::{SettingBackup, build_settings_recovery_plan};
        let b = SettingBackup {
            backup_id: "setb_0001".into(),
            setting_id: "macos.desktopservices.DSDontWriteNetworkStores".into(),
            location: "com.apple.desktopservices DSDontWriteNetworkStores".into(),
            prior_value: "absent".into(),
            written_value: "true".into(),
            created_at: "2026-10-15T00:00:00Z".into(),
        };
        let plan = build_settings_recovery_plan(&[b], "rp_test", 1_800_000_000, "pd").unwrap();
        assert_eq!(kind_name(&plan), "maintenance_restore");
        assert!(desktop_applies(plan.body.plan_kind));
        let row = &item_rows(&plan)[0];
        assert_eq!((row.item_kind.as_str(), row.action_type.as_deref(), row.scope.as_deref()), ("setting", Some("settings_change"), Some("user")));
        assert_eq!(row.path_display, "macos.desktopservices.DSDontWriteNetworkStores: true → absent");
        assert!(row.name.starts_with("restore: "), "{}", row.name);
        assert_eq!(row.detail.as_ref().unwrap()["to_value"], "absent");
    }

    #[test]
    fn startup_scopes_use_the_ui_words() {
        assert_eq!(scope_word(StartupScope::User), "user");
        assert_eq!(scope_word(StartupScope::System), "machine");
        assert_eq!(hive_scope(Hive::Hklm), "machine");
    }
}
