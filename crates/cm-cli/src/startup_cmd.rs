//! `startup list|disable|enable|remove-stale|restore` (parity STA-01..04, tasks 11.1–11.3).
//!
//! Changes only ever create `startup` (or, for `restore`, `recovery`) plans pending review: nothing is enabled,
//! disabled or removed here. `plan review` approves and `plan apply` runs them through the shared executor and journal.
//! Protected items are refused (`policy_denied`), sources without a verified mechanism point to the settings page
//! (`unsupported`), system-wide items need an elevated terminal (`permission_required`; this tool never elevates).
//! `CM_STARTUP_TEST_ROOT` / `CM_STARTUP_TEST_FOLDER` redirect everything to a throwaway key and folder (tests).

use std::process::ExitCode;

use crate::args::Parsed;

#[cfg(windows)]
fn env(req: &str, p: &Parsed) -> Result<cm_platform::windows::startup::StartupEnv, ExitCode> {
    cm_platform::windows::startup::StartupEnv::from_process().map_err(|e| crate::fail(req, cm_core::error::ErrorCode::InvalidArgument, &e, p.format))
}

#[cfg(windows)]
pub fn startup_list(req: &str, p: &Parsed) -> ExitCode {
    let env = match env(req, p) {
        Ok(e) => e,
        Err(c) => return c,
    };
    let (items, issues) = cm_platform::windows::startup::inventory_with(&env);
    emit_list(req, p, &items, &issues)
}

/// One `startup_list` record; a source that could not be read makes the list incomplete: partial, not success (CLI-05).
fn emit_list(req: &str, p: &Parsed, items: &[cm_core::startup::StartupItem], issues: &[cm_core::apps::InventoryIssue]) -> ExitCode {
    use cm_core::error::ExitClass;
    use cm_core::protocol::record;
    use serde_json::json;

    let items: Vec<_> = items.iter().map(|i| i.to_wire()).collect();
    let issue_records: Vec<_> = issues.iter().map(|i| json!({"code": i.code, "scope_display": i.source, "message": i.message})).collect();
    crate::emit(&record("startup_list", req, crate::commands::fields(json!({"items": items, "issues": issue_records}))), p.format);
    ExitCode::from(if issues.is_empty() { ExitClass::Success } else { ExitClass::Partial }.code() as u8)
}

fn refusal_code(code: &str) -> cm_core::error::ErrorCode {
    use cm_core::error::ErrorCode;
    match code {
        "policy_denied" => ErrorCode::PolicyDenied,
        "unsupported" => ErrorCode::Unsupported,
        "permission_required" => ErrorCode::PermissionRequired,
        "conflict" => ErrorCode::Conflict,
        "internal_error" => ErrorCode::InternalError,
        _ => ErrorCode::InvalidArgument,
    }
}

/// `startup disable|enable|remove-stale ITEM_ID`: a `startup` plan pending review.
#[cfg(windows)]
pub fn startup_plan(req: &str, p: &Parsed, op: &str, item_id: &str) -> ExitCode {
    use cm_core::error::ErrorCode;
    use cm_core::plan::v2::StartupOperation;
    use cm_core::startup::StartupActions;

    let operation = match op {
        "disable" => StartupOperation::Disable,
        "enable" => StartupOperation::Enable,
        _ => StartupOperation::RemoveWithBackup,
    };
    let env = match env(req, p) {
        Ok(e) => e,
        Err(c) => return c,
    };
    let stores = match crate::commands::open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let (items, _) = cm_platform::windows::startup::inventory_with(&env);
    let Some(item) = items.iter().find(|i| i.item_id == item_id) else {
        return crate::fail(req, ErrorCode::InvalidArgument, &format!("no startup item {item_id}; run `cleanmanager startup list`"), p.format);
    };
    // Refuse before reading anything else (protected, no mechanism, not an administrator, wrong state).
    if let Err(r) = cm_core::startup::change_decision(item, operation, env.is_admin) {
        return crate::fail(req, refusal_code(r.code), &r.message, p.format);
    }
    let ws = cm_platform::windows::startup_actions::WindowsStartup { env: env.clone() };
    let obs = match ws.observe(item.location.as_ref().expect("checked by change_decision")) {
        Ok(o) => o,
        Err(e) => return crate::fail(req, ErrorCode::IoError, &e, p.format),
    };
    let plan_id = format!("sp_{}", &req[4..]);
    let pdigest = crate::plan_cmd::current_policy_digest(&stores);
    let plan = match cm_core::startup::build_startup_plan(item, operation, &obs, env.is_admin, &plan_id, crate::plan_cmd::now_unix(), &pdigest) {
        Ok(pl) => pl,
        Err(r) => return crate::fail(req, refusal_code(r.code), &r.message, p.format),
    };
    if let Err(e) = stores.audit.save_plan_v2(&plan) {
        return crate::commands::store_error(req, &e, p.format);
    }
    crate::emit(&crate::plan_cmd::v2_plan_record(req, &stores, &plan), p.format);
    ExitCode::SUCCESS
}

/// `startup restore --operation OP_ID`: a `recovery` plan that puts back the recorded prior states.
pub fn startup_restore(req: &str, p: &Parsed) -> ExitCode {
    let Some(op) = p.option("operation") else {
        return crate::fail(req, cm_core::error::ErrorCode::InvalidArgument, "startup restore needs --operation OP_ID", p.format);
    };
    let stores = match crate::commands::open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let plan_id = format!("rp_{}", &req[4..]);
    let pdigest = crate::plan_cmd::current_policy_digest(&stores);
    match cm_app::plans::create_startup_restore_plan(&stores, op, &plan_id, crate::plan_cmd::now_unix(), &pdigest) {
        Ok(plan) => {
            crate::emit(&crate::plan_cmd::v2_plan_record(req, &stores, &plan), p.format);
            ExitCode::SUCCESS
        }
        Err(e) => crate::fail(req, e.code(), &e.to_string(), p.format),
    }
}

/// macOS: the read-only launchd / Service Management inventory (`cm_platform::macos::startup`).
#[cfg(target_os = "macos")]
fn mac_inventory(req: &str, p: &Parsed) -> Result<(Vec<cm_core::startup::StartupItem>, Vec<cm_core::apps::InventoryIssue>), ExitCode> {
    let env = cm_platform::macos::startup::StartupEnv::from_process().map_err(|e| crate::fail(req, cm_core::error::ErrorCode::InvalidArgument, &e, p.format))?;
    Ok(cm_platform::macos::startup::inventory_with(&env))
}

#[cfg(target_os = "macos")]
pub fn startup_list(req: &str, p: &Parsed) -> ExitCode {
    match mac_inventory(req, p) {
        Ok((items, issues)) => emit_list(req, p, &items, &issues),
        Err(c) => c,
    }
}

/// macOS: no item toggles in this build. The item is looked up so the refusal names the real reason (protected:
/// `policy_denied`; otherwise `unsupported` with the row's reason); no plan is ever created.
#[cfg(target_os = "macos")]
pub fn startup_plan(req: &str, p: &Parsed, op: &str, item_id: &str) -> ExitCode {
    use cm_core::error::ErrorCode;
    use cm_core::plan::v2::StartupOperation;

    let operation = match op {
        "disable" => StartupOperation::Disable,
        "enable" => StartupOperation::Enable,
        _ => StartupOperation::RemoveWithBackup,
    };
    let (items, _) = match mac_inventory(req, p) {
        Ok(v) => v,
        Err(c) => return c,
    };
    let Some(item) = items.iter().find(|i| i.item_id == item_id) else {
        return crate::fail(req, ErrorCode::InvalidArgument, &format!("no startup item {item_id}; run `cleanmanager startup list`"), p.format);
    };
    match cm_core::startup::change_decision(item, operation, false) {
        Err(r) => crate::fail(req, refusal_code(r.code), &r.message, p.format),
        Ok(()) => crate::fail(req, ErrorCode::Unsupported, cm_platform::macos::startup::TOGGLE_UNSUPPORTED, p.format),
    }
}

/// Seam for `plan apply` (plan/v2 backends): login/background item and launchd changes on macOS. `None`: this build only
/// lists them (`startup list`), so no macOS startup plan is ever created and a stored one is refused as unsupported.
#[cfg(target_os = "macos")]
pub(crate) fn mac_startup_backend() -> Option<Box<dyn cm_core::startup::StartupActions>> {
    None
}
