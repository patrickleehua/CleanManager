//! `plan apply`, `operations show`, `operations reconcile` (EXE-*, CLI-02/05/09). The stored plan type decides
//! the executor: `recovery/v1` plans and their operations go to `recovery_cmd`, `plan/v2` plans (manual selection,
//! startup, startup recovery) to `cm_app::plans` by their kind, `plan/v1` to the cleanup executor.

use std::process::ExitCode;

use cm_app::execute::{ApplyContext, ApplyError, OperationReport, apply_plan, reconcile_operation};
use cm_app::plans::PlanType;
use cm_core::authz::current_principal;
use cm_core::error::{ErrorCode, ExitClass};
use cm_core::host::HostProbe;
use cm_core::protocol::{ErrorDetails, error_record, record};
use cm_platform::host::OsHostProbe;
use serde_json::{Value, json};

use crate::args::Parsed;
use crate::commands::{fields, open_stores, store_error};
use crate::plan_cmd::now_unix;
use crate::{emit, fail};

#[cfg(windows)]
fn actions(store_dir: &std::path::Path) -> impl cm_core::execute::FileActions {
    cm_platform::windows::actions::WindowsActions::with_store(store_dir.to_path_buf())
}

/// macOS file actions (`cm_platform::macos::actions`): every live check reports unsupported until they are verified,
/// so plans are skipped item by item and nothing is touched.
#[cfg(target_os = "macos")]
fn actions(store_dir: &std::path::Path) -> impl cm_core::execute::FileActions {
    cm_platform::macos::actions::file_actions(store_dir)
}

fn current_rule_set_digest(p: &Parsed) -> Result<String, String> {
    let dir = p.raw_option("rules").map(std::path::PathBuf::from).unwrap_or_else(crate::analysis_cmd::default_rules_dir);
    crate::analysis_cmd::load_rules(&dir).map(|r| r.digest)
}

pub fn report_record(req: &str, r: &OperationReport) -> Value {
    trace_report(r);
    record("operation_report", req, fields(json!({
        "operation_id": r.operation_id,
        "plan_id": r.plan_id,
        "plan_digest": r.plan_digest,
        "status": r.status,
        "started_at": r.started_at,
        "finished_at": r.finished_at,
        "succeeded": r.succeeded,
        "skipped": r.skipped,
        "failed": r.failed,
        "not_started": r.not_started,
        "unknown": r.unknown,
        "items": r.items.iter().map(|i| json!({"action_id": i.action_id, "status": i.status, "code": i.code, "message": i.message, "recovery": i.recovery})).collect::<Vec<_>>(),
        "processed_logical_bytes": r.processed_logical_bytes,
        "quarantined_logical_bytes": r.quarantined_logical_bytes,
        "estimated_freed_bytes": r.estimated_freed_bytes,
        "volume_free_before_bytes": r.volume_free_before_bytes,
        "volume_free_after_bytes": r.volume_free_after_bytes,
        "observed_free_delta_bytes": r.observed_free_delta_bytes,
    })))
}

/// CLI-14: every operation report that leaves this process, with each action's outcome and skip reason, on stderr
/// only. The authorization source is a label (`interactive_review`, a policy ID), never receipt contents.
fn trace_report(r: &OperationReport) {
    cm_core::debug_trace!(
        "apply",
        "operation={} plan={} status={} authorization_source={} succeeded={} skipped={} failed={} not_started={} unknown={} processed_bytes={}",
        r.operation_id, r.plan_id, r.status, r.authorization_source, r.succeeded, r.skipped, r.failed, r.not_started, r.unknown, r.processed_logical_bytes
    );
    for i in &r.items {
        cm_core::debug_trace!("apply.action", "{} status={:?} code={} recovery={} message=\"{}\"", i.action_id, i.status, i.code, i.recovery, i.message);
    }
}

pub fn exit_for(r: &OperationReport) -> ExitCode {
    let class = match r.status.as_str() {
        "completed" => ExitClass::Success,
        "partial" => ExitClass::Partial,
        "cancelled" => ExitClass::Error(ErrorCode::Cancelled),
        "reconciliation_required" => ExitClass::ReconciliationRequired,
        _ => ExitClass::Error(ErrorCode::IoError),
    };
    ExitCode::from(class.code() as u8)
}

pub fn apply_error(req: &str, plan_id: &str, e: &ApplyError, p: &Parsed) -> ExitCode {
    let required = match e {
        ApplyError::NeedsApproval(_) => Some(format!("Review the plan in the desktop app or run `cleanmanager plan review {plan_id}` in a terminal")),
        ApplyError::Stale(_) => Some("Create a new plan from a fresh analysis".to_string()),
        _ => None,
    };
    let details = ErrorDetails { plan_id: Some(plan_id.into()), operation_id: None, scope_id: None };
    emit(&error_record(req, e.code(), &e.to_string(), required.as_deref(), details), p.format);
    ExitCode::from(ExitClass::Error(e.code()).code() as u8)
}

pub fn plan_apply(req: &str, p: &Parsed, plan_id: &str) -> ExitCode {
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    // CLI-05: dispatch on the stored, authoritative plan type, never on a caller-supplied mode.
    let stored_type = crate::plan_cmd::stored_type(&stores, plan_id);
    cm_core::debug_trace!("apply", "plan={plan_id} stored_type={stored_type:?}");
    let _apply = cm_core::trace::span("apply.run");
    match stored_type {
        Some(PlanType::RecoveryV1) => return crate::recovery_cmd::plan_apply(req, p, &stores, plan_id),
        Some(PlanType::V2(kind)) => return v2_apply(req, p, &stores, plan_id, kind),
        _ => {}
    }
    let stored = stores.audit.load_plan(plan_id).ok();
    // Built-in catalogs (purge, installer) ship with this binary: their digest is checked by the core.
    let builtin = stored.as_ref().map(|pl| pl.body.rule_set_digest.clone()).filter(|d| cm_core::plan::is_builtin_catalog_digest(d));
    let digest = match builtin.map(Ok).unwrap_or_else(|| current_rule_set_digest(p)) {
        Ok(d) => d,
        Err(e) => return fail(req, ErrorCode::InternalError, &format!("rule set unavailable: {e}"), p.format),
    };
    let principal = current_principal();
    let host = OsHostProbe.current("cli");
    // An unknown plan id still reaches apply_plan, which reports it; protection is only consulted per action.
    let rule_refs = stored.as_ref().map(|pl| pl.rule_refs.clone()).unwrap_or_default();
    let base = cm_platform::protection::checker(&stores.dir, rule_refs);
    // PUR-03: purge plans re-read version-control state per artifact before acting.
    #[cfg(windows)]
    let gate = stored.as_ref().map(cm_platform::windows::purge::vcs_gate);
    // macOS: `None` until purge plans are created there (no version-control gate is needed for other plans).
    #[cfg(target_os = "macos")]
    let gate = stored.as_ref().and_then(crate::purge_cmd::mac_vcs_gate);
    let protect = |t: &cm_core::execute::ActionTarget<'_>| base(t).and_then(|()| gate.as_ref().map_or(Ok(()), |g| g(t)));
    let ctx = ApplyContext {
        principal: &principal,
        host: &host,
        current_rule_set_digest: &digest,
        now: &now_unix,
        cancel: &|| false,
        fault: &|_, _| false,
        protect: &protect,
    };
    match apply_plan(&stores, &actions(&stores.dir), plan_id, &ctx) {
        Ok(r) => {
            emit(&report_record(req, &r), p.format);
            exit_for(&r)
        }
        Err(e) => apply_error(req, plan_id, &e, p),
    }
}

pub fn operations_show(req: &str, p: &Parsed, id: &str) -> ExitCode {
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let _ = stores.audit.reconcile_hosts(|h| OsHostProbe.liveness(h));
    match stores.audit.operation_report(id) {
        Ok(Some(json)) => match serde_json::from_str::<OperationReport>(&json) {
            Ok(r) => {
                let mut rec = report_record(req, &r);
                crate::uninstall_cmd::add_app_outcomes(&stores, &r, &mut rec);
                emit(&rec, p.format);
                exit_for(&r)
            }
            Err(e) => fail(req, ErrorCode::InternalError, &e.to_string(), p.format),
        },
        Ok(None) => match stores.audit.operation_state(id) {
            Ok(state) if state == "reconciliation_required" => fail(req, ErrorCode::InternalError,
                &format!("operation {id} stopped with unconfirmed actions; run `cleanmanager operations reconcile {id}`"), p.format),
            Ok(state) => {
                emit(&record("operation_status", req, fields(json!({"operation_id": id, "state": state}))), p.format);
                ExitCode::SUCCESS
            }
            Err(e) => store_error(req, &e, p.format),
        },
        Err(e) => store_error(req, &e, p.format),
    }
}

pub fn operations_reconcile(req: &str, p: &Parsed, id: &str) -> ExitCode {
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    match stores.audit.operation_plan(id).ok().and_then(|plan| crate::plan_cmd::stored_type(&stores, &plan)) {
        Some(PlanType::RecoveryV1) => return crate::recovery_cmd::operations_reconcile(req, p, &stores, id),
        Some(PlanType::V2(_)) => return v2_reconcile(req, p, &stores, id),
        _ => {}
    }
    let principal = current_principal();
    let host = OsHostProbe.current("cli");
    // Reconciliation only inspects; it never starts a new action, so protection is not consulted.
    let ctx = ApplyContext { principal: &principal, host: &host, current_rule_set_digest: "", now: &now_unix, cancel: &|| false, fault: &|_, _| false, protect: &|_| Ok(()) };
    match reconcile_operation(&stores, &actions(&stores.dir), id, &ctx) {
        Ok(r) => {
            emit(&report_record(req, &r), p.format);
            exit_for(&r)
        }
        Err(e) => apply_error(req, id, &e, p),
    }
}

// ---------------------------------------------------------------------------------------------
// plan/v2

/// Run `f` with the platform backends of plan/v2 kinds (Windows: file actions, startup changes through the process'
/// startup environment, Restart Manager holders and the user protection list).
#[cfg(windows)]
fn with_v2_backends<T>(stores: &cm_store::Stores, f: impl FnOnce(&cm_app::plans::V2Backends<'_>, bool) -> T) -> Result<T, String> {
    use cm_platform::windows::startup_actions::WindowsStartup;
    let env = cm_platform::windows::startup::StartupEnv::from_process()?;
    let is_admin = env.is_admin;
    let startup = WindowsStartup { env };
    let files = actions(&stores.dir);
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
    let processes = cm_platform::windows::processes::WindowsProcesses;
    let updates = cm_platform::windows::updates::WindowsUpdates;
    // UNI-02: ending still-running processes needs a plan reviewed for it and, at apply time, a person typing END.
    let uninstall = cm_platform::windows::uninstall::WindowsUninstall {
        env: cm_platform::windows::uninstall::UninstallEnv::from_process()?,
        confirm: Box::new(|procs: &[cm_core::uninstall::RunningProcess]| {
            let names: Vec<String> = procs.iter().map(|x| x.describe()).collect();
            crate::plan_cmd::interactive()
                && crate::plan_cmd::prompt(&format!("Still running: {}. Type END to end them now, anything else to cancel (nothing is uninstalled then): ", names.join(", "))).as_deref()
                    == Some("END")
        }),
    };
    let b = cm_app::plans::V2Backends {
        files: &files,
        startup: Some(&startup),
        holders: &holders,
        protect_item: &protect_item,
        processes: Some(&processes),
        updates: Some(&updates),
        uninstall: Some(&uninstall),
    };
    Ok(f(&b, is_admin))
}

/// macOS: each backend comes from its command module's seam (`None` = that plan kind's actions are unsupported here).
#[cfg(target_os = "macos")]
fn with_v2_backends<T>(stores: &cm_store::Stores, f: impl FnOnce(&cm_app::plans::V2Backends<'_>, bool) -> T) -> Result<T, String> {
    let files = actions(&stores.dir);
    let startup = crate::startup_cmd::mac_startup_backend();
    let is_admin = startup.as_ref().is_some_and(|s| s.is_admin());
    let user = cm_platform::protection::load(&stores.dir);
    let protect_item = |item_id: &str| -> Result<(), String> {
        let user = user.as_ref().map_err(|e| format!("protection_unavailable: {e}"))?;
        match cm_core::protect::evaluate_task(item_id, user) {
            cm_core::protect::ProtectionDecision::Allowed => Ok(()),
            denied => Err(format!("{denied:?}")),
        }
    };
    let holders = crate::trash_cmd::mac_holders();
    let no_holders = |_: &[cm_core::path::RawPath]| -> Option<Vec<String>> { None };
    let processes = crate::process_cmd::mac_processes();
    let updates = crate::updates_cmd::mac_update_backend();
    let uninstall = crate::uninstall_cmd::mac_uninstall_backend();
    let b = cm_app::plans::V2Backends {
        files: &files,
        startup: startup.as_deref(),
        holders: holders.as_deref().unwrap_or(&no_holders),
        protect_item: &protect_item,
        processes: processes.as_deref(),
        updates: updates.as_deref(),
        uninstall: uninstall.as_deref(),
    };
    Ok(f(&b, is_admin))
}

fn v2_apply(req: &str, p: &Parsed, stores: &cm_store::Stores, plan_id: &str, kind: cm_core::plan::v2::PlanKind) -> ExitCode {
    // Maintenance runs elevate once through their own short-lived worker (MNT-04) and report per task.
    if kind == cm_core::plan::v2::PlanKind::Maintenance {
        return crate::optimize_cmd::apply(req, p, stores, plan_id);
    }
    if !cm_app::plans::executable(kind) {
        return fail(req, ErrorCode::Unsupported, &format!("plan kind {} cannot be applied by this build", kind.as_str()), p.format);
    }
    let requires_admin = stores.audit.load_plan_v2(plan_id).map(|pl| pl.body.requires_admin).unwrap_or(false);
    let principal = current_principal();
    let host = OsHostProbe.current("cli");
    // PRT-01: every file action (and everything a folder move carries) passes the protection re-check.
    let protect = cm_platform::protection::checker(&stores.dir, vec![]);
    let ctx = ApplyContext { principal: &principal, host: &host, current_rule_set_digest: "", now: &now_unix, cancel: &|| false, fault: &|_, _| false, protect: &protect };
    let out = with_v2_backends(stores, |b, is_admin| {
        // System-wide changes need an elevated token; this tool never elevates itself (STA-02).
        if requires_admin && !is_admin {
            return Err(());
        }
        Ok(cm_app::plans::apply_plan_v2(stores, b, plan_id, &ctx))
    });
    match out {
        Err(e) => fail(req, ErrorCode::InvalidArgument, &e, p.format),
        Ok(Err(())) => {
            let details = ErrorDetails { plan_id: Some(plan_id.into()), operation_id: None, scope_id: None };
            emit(&error_record(req, ErrorCode::PermissionRequired, cm_core::startup::ADMIN_REQUIRED, Some("Run `cleanmanager plan apply` from an administrator terminal"), details), p.format);
            ExitCode::from(ExitClass::Error(ErrorCode::PermissionRequired).code() as u8)
        }
        Ok(Ok(Ok(r))) => {
            let mut rec = report_record(req, &r);
            crate::uninstall_cmd::add_app_outcomes(stores, &r, &mut rec);
            emit(&rec, p.format);
            exit_for(&r)
        }
        Ok(Ok(Err(e))) => apply_error(req, plan_id, &e, p),
    }
}

fn v2_reconcile(req: &str, p: &Parsed, stores: &cm_store::Stores, id: &str) -> ExitCode {
    let principal = current_principal();
    let host = OsHostProbe.current("cli");
    // Reconciliation only inspects; it never starts a new action.
    let ctx = ApplyContext { principal: &principal, host: &host, current_rule_set_digest: "", now: &now_unix, cancel: &|| false, fault: &|_, _| false, protect: &|_| Ok(()) };
    match with_v2_backends(stores, |b, _| cm_app::plans::reconcile_operation_v2(stores, b, id, &ctx)) {
        Err(e) => fail(req, ErrorCode::InvalidArgument, &e, p.format),
        Ok(Ok(r)) => {
            emit(&report_record(req, &r), p.format);
            exit_for(&r)
        }
        Ok(Err(e)) => apply_error(req, id, &e, p),
    }
}
