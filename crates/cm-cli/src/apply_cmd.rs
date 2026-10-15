//! `plan apply`, `operations show`, `operations reconcile` (EXE-*, CLI-02/05/09). Recovery plans
//! and their operations are dispatched to `recovery_cmd` by the stored plan type.

use std::process::ExitCode;

use cm_app::execute::{ApplyContext, ApplyError, OperationReport, apply_plan, reconcile_operation};
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

/// Platforms without verified write actions (MVP 9.5 pending): every live check reports
/// unsupported, so plans are skipped item by item and nothing is touched.
#[cfg(not(windows))]
fn actions(_: &std::path::Path) -> impl cm_core::execute::FileActions {
    Unsupported
}

#[cfg(not(windows))]
struct Unsupported;

#[cfg(not(windows))]
impl cm_core::execute::FileActions for Unsupported {
    fn check(&self, _: &cm_core::execute::ActionTarget<'_>) -> cm_core::execute::LiveCheck {
        cm_core::execute::LiveCheck::Unsupported("file actions are not verified on this platform".into())
    }
    fn quarantine(&self, _: &cm_core::execute::ActionTarget<'_>, _: &cm_core::path::RawPath, _: &str) -> cm_core::execute::ActionOutcome {
        unsupported()
    }
    fn trash(&self, _: &cm_core::execute::ActionTarget<'_>) -> cm_core::execute::ActionOutcome {
        unsupported()
    }
    fn delete(&self, _: &cm_core::execute::ActionTarget<'_>) -> cm_core::execute::ActionOutcome {
        unsupported()
    }
    fn quarantine_dir_for(&self, _: &cm_core::execute::ActionTarget<'_>, _: &str) -> Option<cm_core::path::RawPath> {
        None
    }
    fn reconcile(&self, _: &cm_core::execute::ActionTarget<'_>, _: cm_core::rules::Mode, _: Option<&cm_core::path::RawPath>) -> cm_core::execute::Reconciled {
        cm_core::execute::Reconciled::StillUnknown
    }
    fn free_space(&self, _: &cm_core::path::RawPath) -> Option<u64> {
        None
    }
}

#[cfg(not(windows))]
fn unsupported() -> cm_core::execute::ActionOutcome {
    cm_core::execute::ActionOutcome::Skipped { code: "unsupported".into(), message: "file actions are not verified on this platform".into() }
}

fn current_rule_set_digest(p: &Parsed) -> Result<String, String> {
    let dir = p.raw_option("rules").map(std::path::PathBuf::from).unwrap_or_else(crate::analysis_cmd::default_rules_dir);
    crate::analysis_cmd::load_rules(&dir).map(|r| r.digest)
}

pub fn report_record(req: &str, r: &OperationReport) -> Value {
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
    if stores.audit.plan_contract(plan_id).ok().as_deref() == Some(cm_core::recovery::RECOVERY_CONTRACT) {
        return crate::recovery_cmd::plan_apply(req, p, &stores, plan_id);
    }
    let digest = match current_rule_set_digest(p) {
        Ok(d) => d,
        Err(e) => return fail(req, ErrorCode::InternalError, &format!("rule set unavailable: {e}"), p.format),
    };
    let principal = current_principal();
    let host = OsHostProbe.current("cli");
    // An unknown plan id still reaches apply_plan, which reports it; protection is only consulted per action.
    let rule_refs = stores.audit.load_plan(plan_id).map(|pl| pl.rule_refs).unwrap_or_default();
    let protect = cm_platform::protection::checker(&stores.dir, rule_refs);
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
                emit(&report_record(req, &r), p.format);
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
    let recovery = stores.audit.operation_plan(id).and_then(|plan| stores.audit.plan_contract(&plan));
    if recovery.ok().as_deref() == Some(cm_core::recovery::RECOVERY_CONTRACT) {
        return crate::recovery_cmd::operations_reconcile(req, p, &stores, id);
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
