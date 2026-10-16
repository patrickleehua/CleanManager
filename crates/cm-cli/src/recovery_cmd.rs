//! Quarantine, recovery-plan and history commands (REC-01..05, CLI-02/05).
//!
//! `recovery plan create` freezes quarantined items into a `recovery/v1` plan. The shared
//! `plan show|review|revoke|apply` commands dispatch on the stored, authoritative plan type, so
//! a recovery plan is reviewed and applied by the recovery service and never by the cleanup
//! executor (and vice versa). Only a human review in a terminal authorizes a recovery plan.

use std::process::ExitCode;

use cm_app::execute::{ApplyError, OperationReport};
use cm_app::recovery::{
    HistoryItem, QuarantineView, RecoveryContext, apply_recovery_plan, create_recovery_plan, operation_history, quarantine_page, quarantine_show,
    reconcile_recovery_operation, recovery_authorization,
};
use cm_core::authz::{ReceiptSource, current_principal};
use cm_core::error::ErrorCode;
use cm_core::host::HostProbe;
use cm_core::protocol::record;
use cm_core::recovery::{QuarantineState, RecoveryActions, RecoveryKind, RecoveryPlan, RecoveryReceipt};
use cm_core::time::now_utc;
use cm_platform::host::OsHostProbe;
use cm_store::Stores;
use serde_json::{Value, json};

use crate::args::Parsed;
use crate::commands::{fields, open_stores, store_error};
use crate::plan_cmd::{now_unix, prompt};
use crate::{emit, fail};

#[cfg(windows)]
pub fn actions() -> impl RecoveryActions {
    cm_platform::windows::actions::recovery::WindowsRecovery
}

/// Whether this build has a verified restore/purge implementation for the platform.
#[cfg(windows)]
const RESTORE_VERIFIED: bool = true;
#[cfg(target_os = "macos")]
const RESTORE_VERIFIED: bool = cm_platform::macos::actions::recovery::RESTORE_VERIFIED;

/// macOS recovery actions (`cm_platform::macos::actions::recovery`; pending, MVP 10.4): nothing is touched until verified.
#[cfg(target_os = "macos")]
pub fn actions() -> impl RecoveryActions {
    cm_platform::macos::actions::recovery::recovery_actions()
}

fn page_args(req: &str, p: &Parsed) -> Result<u32, ExitCode> {
    match p.option("limit").map(str::parse::<u32>).unwrap_or(Ok(cm_store::index::DEFAULT_PAGE)) {
        Ok(n) => Ok(n),
        Err(_) => Err(fail(req, ErrorCode::InvalidArgument, "--limit must be a number", p.format)),
    }
}

fn state_name(s: &QuarantineState) -> String {
    serde_json::to_value(s).expect("state serializes").as_str().expect("unit variant").to_owned()
}

/// Protocol `quarantine_item`.
fn quarantine_item(v: &QuarantineView, actions: &dyn RecoveryActions) -> Value {
    let complete = v.manifest.is_some() && RESTORE_VERIFIED;
    json!({
        "quarantine_id": v.entry.quarantine_id,
        "plan_id": v.entry.plan_id,
        "operation_id": v.entry.operation_id,
        "original_path": v.original_path,
        "quarantined_path": v.location,
        "logical_bytes": v.logical_bytes,
        "quarantined_at": v.entry.quarantined_at,
        "retention_reminder_at": v.entry.reminder_at,
        "reminder_due": v.reminder_due,
        "state": state_name(&v.state),
        "restore_capability": if complete { "available" } else { "unsupported" },
        "available_actions": if complete { vec!["restore", "purge"] } else { vec![] },
        "metadata_restored": if complete { actions.metadata_coverage().preserved } else { vec![] },
    })
}

fn quarantine_list_record(req: &str, items: &[QuarantineView], page_size: u32, next: Option<String>, actions: &dyn RecoveryActions) -> Value {
    let total: u64 = items.iter().map(|v| v.logical_bytes.0).sum();
    record("quarantine_list", req, fields(json!({
        "items": items.iter().map(|v| quarantine_item(v, actions)).collect::<Vec<_>>(),
        "page_size": page_size,
        "next_cursor": next,
        "total_logical_bytes": total.to_string(),
    })))
}

/// `recovery list` / `quarantine list`: items still held in quarantine with live state.
pub fn recovery_list(req: &str, p: &Parsed) -> ExitCode {
    let limit = match page_args(req, p) {
        Ok(n) => n,
        Err(c) => return c,
    };
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let a = actions();
    match quarantine_page(&stores, &a, limit, p.option("cursor"), now_unix()) {
        Ok((items, next)) => {
            emit(&quarantine_list_record(req, &items, limit, next, &a), p.format);
            ExitCode::SUCCESS
        }
        Err(e) => store_error(req, &e, p.format),
    }
}

pub fn recovery_show(req: &str, p: &Parsed, id: &str) -> ExitCode {
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let a = actions();
    match quarantine_show(&stores, &a, id, now_unix()) {
        Ok(v) => {
            emit(&quarantine_list_record(req, std::slice::from_ref(&v), 1, None, &a), p.format);
            ExitCode::SUCCESS
        }
        Err(e) => store_error(req, &e, p.format),
    }
}

fn status_of(stores: &Stores, plan: &RecoveryPlan) -> Result<(&'static str, Option<RecoveryReceipt>), cm_store::StoreError> {
    recovery_authorization(stores, plan, &current_principal(), now_unix())
}

pub fn authorization_record(req: &str, stores: &Stores, plan: &RecoveryPlan) -> Value {
    let (status, r) = status_of(stores, plan).unwrap_or(("required", None));
    record("authorization_status", req, fields(json!({
        "plan_id": plan.body.plan_id,
        "plan_digest": plan.digest,
        "status": status,
        "source": r.as_ref().map(|r| r.source),
        "policy_id": null,
        "policy_version": null,
        "principal": r.as_ref().map(|r| r.principal.clone()),
        "authorized_at": r.as_ref().map(|r| r.created_at.clone()),
        "expires_at": r.as_ref().map(|r| r.expires_at.clone()),
        "quota": null,
    })))
}

pub fn plan_record(req: &str, stores: &Stores, plan: &RecoveryPlan) -> Value {
    let auth = authorization_record(req, stores, plan);
    record("plan", req, fields(json!({"body": plan.body, "plan_digest": plan.digest, "authorization_status": auth["status"]})))
}

fn ids(list: &str) -> Vec<String> {
    list.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect()
}

/// `recovery plan create --restore ID,... | --purge ID,...`. Creating authorizes nothing.
pub fn recovery_plan_create(req: &str, p: &Parsed) -> ExitCode {
    let (kind, list) = match (p.option("restore"), p.option("purge")) {
        (Some(l), None) => (RecoveryKind::Restore, ids(l)),
        (None, Some(l)) => (RecoveryKind::Purge, ids(l)),
        _ => return fail(req, ErrorCode::InvalidArgument, "pass exactly one of --restore ID,ID or --purge ID,ID", p.format),
    };
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let plan_id = format!("rp_{}", &req[4..]);
    match create_recovery_plan(&stores, kind, &list, &plan_id, now_unix()) {
        Ok(plan) => {
            emit(&plan_record(req, &stores, &plan), p.format);
            ExitCode::SUCCESS
        }
        Err(e) => fail(req, e.code(), &e.to_string(), p.format),
    }
}

fn load(req: &str, p: &Parsed, stores: &Stores, id: &str) -> Result<RecoveryPlan, ExitCode> {
    stores.audit.load_recovery_plan(id).map_err(|e| store_error(req, &e, p.format))
}

pub fn plan_show(req: &str, p: &Parsed, stores: &Stores, id: &str) -> ExitCode {
    match load(req, p, stores, id) {
        Ok(plan) => {
            emit(&plan_record(req, stores, &plan), p.format);
            ExitCode::SUCCESS
        }
        Err(c) => c,
    }
}

pub fn plan_revoke(req: &str, p: &Parsed, stores: &Stores, id: &str) -> ExitCode {
    let plan = match load(req, p, stores, id) {
        Ok(plan) => plan,
        Err(c) => return c,
    };
    if let Err(e) = stores.audit.revoke_receipts(id, &now_utc()) {
        return store_error(req, &e, p.format);
    }
    emit(&authorization_record(req, stores, &plan), p.format);
    ExitCode::SUCCESS
}

/// Human review of a recovery plan in a terminal; writes a recovery receipt for this exact plan.
pub fn plan_review(req: &str, p: &Parsed, stores: &Stores, id: &str) -> ExitCode {
    let plan = match load(req, p, stores, id) {
        Ok(plan) => plan,
        Err(c) => return c,
    };
    if cm_core::recovery::check_recovery_plan(&plan, now_unix()).is_err() {
        return fail(req, ErrorCode::PlanStale, "plan expired; create a new recovery plan", p.format);
    }
    let kind = plan.body.recovery_kind;
    println!("Recovery plan {}  kind={kind:?}  items={}", plan.body.plan_id, plan.body.items.len());
    println!("Expires: {}   Digest: {}", plan.body.expires_at, plan.digest);
    match kind {
        RecoveryKind::Restore => println!("Moves each item back to its original folder. An occupied name is skipped; nothing is overwritten."),
        RecoveryKind::Purge => println!("Permanently deletes each quarantined item. There is no undo."),
    }
    for item in plan.body.items.iter().take(20) {
        match kind {
            RecoveryKind::Restore => println!("  {}  {}  {} -> {}", item.action_id, item.quarantine_id, item.quarantined_path.display(), item.original_path.display()),
            RecoveryKind::Purge => println!("  {}  {}  {} bytes  {}", item.action_id, item.quarantine_id, item.logical_bytes, item.quarantined_path.display()),
        }
    }
    if plan.body.items.len() > 20 {
        println!("  ... {} more items (plan show {} lists them all)", plan.body.items.len() - 20, plan.body.plan_id);
    }
    let code = &plan.digest[..8];
    match prompt(&format!("Type {code} to approve this exact recovery plan, anything else to cancel: ")) {
        Some(answer) if answer == code => {}
        _ => return fail(req, ErrorCode::Cancelled, "not approved", p.format),
    }
    let receipt = RecoveryReceipt {
        receipt_id: format!("r_{}", &req[4..]),
        plan_id: plan.body.plan_id.clone(),
        plan_digest: plan.digest.clone(),
        recovery_kind: kind,
        principal: current_principal(),
        source: ReceiptSource::InteractiveReview,
        created_at: now_utc(),
        expires_at: plan.body.expires_at.clone(),
        revoked_at: None,
    };
    if let Err(e) = stores.audit.save_recovery_receipt(&receipt) {
        return store_error(req, &e, p.format);
    }
    emit(&authorization_record(req, stores, &plan), p.format);
    ExitCode::SUCCESS
}

fn finish(req: &str, p: &Parsed, plan_id: &str, r: Result<OperationReport, ApplyError>) -> ExitCode {
    match r {
        Ok(r) => {
            emit(&crate::apply_cmd::report_record(req, &r), p.format);
            crate::apply_cmd::exit_for(&r)
        }
        Err(e) => crate::apply_cmd::apply_error(req, plan_id, &e, p),
    }
}

pub fn plan_apply(req: &str, p: &Parsed, stores: &Stores, plan_id: &str) -> ExitCode {
    let principal = current_principal();
    let host = OsHostProbe.current("cli");
    let ctx = RecoveryContext { principal: &principal, host: &host, now: &now_unix, cancel: &|| false, fault: &|_, _| false };
    finish(req, p, plan_id, apply_recovery_plan(stores, &actions(), plan_id, &ctx))
}

pub fn operations_reconcile(req: &str, p: &Parsed, stores: &Stores, operation_id: &str) -> ExitCode {
    let principal = current_principal();
    let host = OsHostProbe.current("cli");
    let ctx = RecoveryContext { principal: &principal, host: &host, now: &now_unix, cancel: &|| false, fault: &|_, _| false };
    finish(req, p, operation_id, reconcile_recovery_operation(stores, &actions(), operation_id, &ctx))
}

/// Protocol `operation_summary`.
fn history_item(h: &HistoryItem) -> Value {
    let counts = h.report.as_ref().map(|r| json!({"succeeded": r.succeeded, "skipped": r.skipped, "failed": r.failed, "not_started": r.not_started, "unknown": r.unknown}));
    json!({
        "operation_id": h.row.operation_id,
        "plan_id": h.row.plan_id,
        "plan_contract": h.contract,
        "action": h.action,
        "state": h.row.state,
        "started_at": h.row.started_at,
        "finished_at": h.row.finished_at,
        "authorization_source": h.report.as_ref().map(|r| r.authorization_source.clone()),
        "rule_refs": h.rule_refs,
        "counts": counts,
        "processed_logical_bytes": h.report.as_ref().map(|r| r.processed_logical_bytes.clone()),
        "recovery": {"availability": h.availability, "guidance": h.guidance},
    })
}

/// `operations list`: history newest first, with truthful recovery availability (REC-04).
pub fn operations_list(req: &str, p: &Parsed) -> ExitCode {
    let limit = match page_args(req, p) {
        Ok(n) => n,
        Err(c) => return c,
    };
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let _ = stores.audit.reconcile_hosts(|h| OsHostProbe.liveness(h));
    match operation_history(&stores, limit, p.option("cursor")) {
        Ok((items, next)) => {
            emit(&record("operation_list", req, fields(json!({
                "items": items.iter().map(history_item).collect::<Vec<_>>(),
                "page_size": limit,
                "next_cursor": next,
            }))), p.format);
            ExitCode::SUCCESS
        }
        Err(e) => store_error(req, &e, p.format),
    }
}
