//! Plan and policy commands (PLN-01..07, CLI-02/06/07).
//!
//! Machines may create plans and (paused) policies. Approving a plan and enabling a policy are
//! human acts: they require an interactive terminal and a typed confirmation. A TTY is a
//! presentation control, not proof of a human; it does not isolate a same-user process that can
//! drive a console (documented threat boundary, PLN-07).

use std::io::{BufRead, IsTerminal, Write};
use std::process::ExitCode;

use cm_core::analysis::CandidateSet;
use cm_core::authz::{Policy, PolicyState, Receipt, ReceiptSource, current_principal, receipt_valid};
use cm_core::error::ErrorCode;
use cm_core::plan::{Plan, PlanInput, build_plan, parse_utc};
use cm_core::protocol::record;
use cm_core::rules::{Mode, Tier};
use cm_core::time::{now_utc, utc_from_unix};
use cm_store::Stores;
use serde_json::{Value, json};

use crate::args::Parsed;
use crate::commands::{fields, open_stores, store_error};
use crate::{emit, fail};

pub fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn parse_mode(s: Option<&str>) -> Result<Mode, String> {
    match s {
        Some("trash") => Ok(Mode::Trash),
        Some("quarantine") => Ok(Mode::Quarantine),
        Some("delete") => Ok(Mode::Delete),
        Some(other) => Err(format!("unknown mode {other}")),
        None => Err("--mode trash|quarantine|delete is required".into()),
    }
}

fn policy_digest(stores: &Stores) -> String {
    let policies = stores.audit.policies().unwrap_or_default();
    cm_core::c14n::digest(&serde_json::to_value(&policies).unwrap()).expect("plain JSON")
}

/// Authorization state as computed by the core, never as asserted by a client.
pub fn authorization_record(req: &str, stores: &Stores, plan: &Plan) -> Value {
    let now = now_unix();
    let principal = current_principal();
    let receipts = stores.audit.receipts_for(&plan.body.plan_id).unwrap_or_default();
    let valid = receipts.iter().find(|r| receipt_valid(r, plan, &principal, now));
    let expired = parse_utc(&plan.body.expires_at).is_none_or(|e| now >= e);
    let status = match (valid, expired) {
        (Some(_), _) => "authorized",
        (None, true) => "expired",
        (None, false) if receipts.iter().any(|r| r.revoked_at.is_some()) => "revoked",
        (None, false) => "required",
    };
    let r = valid.cloned();
    record("authorization_status", req, fields(json!({
        "plan_id": plan.body.plan_id,
        "plan_digest": plan.digest,
        "status": status,
        "source": r.as_ref().map(|r| r.source),
        "policy_id": r.as_ref().and_then(|r| r.policy_id.clone()),
        "policy_version": r.as_ref().and_then(|r| r.policy_version.map(|v| v.to_string())),
        "principal": r.as_ref().map(|r| r.principal.clone()),
        "authorized_at": r.as_ref().map(|r| r.created_at.clone()),
        "expires_at": r.as_ref().map(|r| r.expires_at.clone()),
        "quota": null,
    })))
}

pub fn plan_record(req: &str, stores: &Stores, plan: &Plan) -> Value {
    let auth = authorization_record(req, stores, plan);
    record("plan", req, fields(json!({
        "body": plan.body,
        "plan_digest": plan.digest,
        "authorization_status": auth["status"],
    })))
}

pub fn plan_create(req: &str, p: &Parsed) -> ExitCode {
    let Some(set_id) = p.option("set") else { return fail(req, ErrorCode::InvalidArgument, "plan create needs --set CANDIDATE_SET_ID", p.format) };
    let mode = match parse_mode(p.option("mode")) {
        Ok(m) => m,
        Err(e) => return fail(req, ErrorCode::InvalidArgument, &e, p.format),
    };
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let set: CandidateSet = match stores.index.candidate_set(set_id).and_then(|v| serde_json::from_value(v).map_err(|e| cm_store::StoreError::IndexCorrupt(e.to_string()))) {
        Ok(s) => s,
        Err(e) => return store_error(req, &e, p.format),
    };
    // Selection is explicit: listed candidate ids, or every default-selected allowed candidate.
    let selected: Vec<String> = match (p.option("candidates"), p.flag("all-default-selected")) {
        (Some(list), false) => list.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect(),
        (None, true) => set.candidates.iter().filter(|c| c.tier == Tier::DefaultSelected && c.allowed_modes.contains(&mode)).map(|c| c.candidate_id.clone()).collect(),
        _ => return fail(req, ErrorCode::InvalidArgument, "pass either --candidates ID,ID or --all-default-selected", p.format),
    };
    let view = match stores.index.view(&set.scan_id) {
        Ok(v) => v,
        Err(e) => return store_error(req, &e, p.format),
    };
    let status = match stores.index.scan_status(&set.scan_id) {
        Ok(s) => s,
        Err(e) => return store_error(req, &e, p.format),
    };
    let plan_id = format!("p_{}", &req[4..]);
    let pdigest = policy_digest(&stores);
    let input = PlanInput {
        plan_id: &plan_id,
        scope_id: &status.scope_id,
        created_unix: now_unix(),
        observed_at: status.finished_at.as_deref().unwrap_or(&status.started_at),
        policy_digest: &pdigest,
    };
    let plan = match build_plan(&view, &set, &selected, mode, &input) {
        Ok(p) => p,
        Err(e) => return fail(req, ErrorCode::PolicyDenied, &e.to_string(), p.format),
    };
    if let Err(e) = stores.audit.save_plan(&plan) {
        return store_error(req, &e, p.format);
    }
    emit(&plan_record(req, &stores, &plan), p.format);
    ExitCode::SUCCESS
}

pub fn plan_show(req: &str, p: &Parsed, id: &str) -> ExitCode {
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    match stores.audit.load_plan(id) {
        Ok(plan) => {
            emit(&plan_record(req, &stores, &plan), p.format);
            ExitCode::SUCCESS
        }
        Err(e) => store_error(req, &e, p.format),
    }
}

fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

fn prompt(line: &str) -> Option<String> {
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "{line}");
    let _ = out.flush();
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer).ok()?;
    Some(answer.trim().to_owned())
}

pub fn plan_review(req: &str, p: &Parsed, id: &str) -> ExitCode {
    if !interactive() {
        return fail(req, ErrorCode::InvalidArgument, "plan review requires an interactive terminal; machine callers cannot approve plans", p.format);
    }
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let plan = match stores.audit.load_plan(id) {
        Ok(p) => p,
        Err(e) => return store_error(req, &e, p.format),
    };
    if parse_utc(&plan.body.expires_at).is_none_or(|e| now_unix() >= e) {
        return fail(req, ErrorCode::PlanStale, "plan expired; create a new plan", p.format);
    }
    let total = plan.body.total_logical().map(|b| b.to_string()).unwrap_or_else(|| "unknown".into());
    println!("Plan {}  mode={:?}  items={}  logical_bytes={total}", plan.body.plan_id, plan.body.mode, plan.body.items.len());
    println!("Rules: {}", plan.rule_refs.join(", "));
    println!("Expires: {}   Digest: {}", plan.body.expires_at, plan.digest);
    match plan.body.mode {
        Mode::Delete => println!("Permanent delete: there is no undo."),
        Mode::Trash => println!("Moved to the Recycle Bin; restore with the system UI."),
        Mode::Quarantine => println!("Moved to a same-volume quarantine; this does not free space until it is purged."),
    }
    for item in plan.body.items.iter().take(20) {
        println!("  {}  {} bytes  {}", item.action_id, item.logical_bytes, item.path.display());
    }
    if plan.body.items.len() > 20 {
        println!("  ... {} more items (plan show {} lists them all)", plan.body.items.len() - 20, plan.body.plan_id);
    }
    let code = &plan.digest[..8];
    match prompt(&format!("Type {code} to approve this exact plan, anything else to cancel: ")) {
        Some(answer) if answer == code => {}
        _ => return fail(req, ErrorCode::Cancelled, "not approved", p.format),
    }
    let receipt = Receipt {
        receipt_id: format!("r_{}", &req[4..]),
        plan_id: plan.body.plan_id.clone(),
        plan_digest: plan.digest.clone(),
        mode: plan.body.mode,
        principal: current_principal(),
        source: ReceiptSource::InteractiveReview,
        policy_id: None,
        policy_version: None,
        created_at: now_utc(),
        expires_at: plan.body.expires_at.clone(),
        revoked_at: None,
    };
    if let Err(e) = stores.audit.save_receipt(&receipt) {
        return store_error(req, &e, p.format);
    }
    emit(&authorization_record(req, &stores, &plan), p.format);
    ExitCode::SUCCESS
}

pub fn plan_revoke(req: &str, p: &Parsed, id: &str) -> ExitCode {
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let plan = match stores.audit.load_plan(id) {
        Ok(p) => p,
        Err(e) => return store_error(req, &e, p.format),
    };
    if let Err(e) = stores.audit.revoke_receipts(id, &now_utc()) {
        return store_error(req, &e, p.format);
    }
    emit(&authorization_record(req, &stores, &plan), p.format);
    ExitCode::SUCCESS
}

pub fn policy_create(req: &str, p: &Parsed) -> ExitCode {
    let (Some(scope), Some(rules)) = (p.option("scope"), p.option("rules")) else {
        return fail(req, ErrorCode::InvalidArgument, "policy create needs --scope SCOPE_ID --rules RULE@VERSION,...", p.format);
    };
    let mode = match parse_mode(p.option("mode")) {
        Ok(m) => m,
        Err(e) => return fail(req, ErrorCode::InvalidArgument, &e, p.format),
    };
    let rules: Vec<String> = rules.split(',').map(|s| s.trim().to_owned()).filter(|s| s.contains('@')).collect();
    if rules.is_empty() {
        return fail(req, ErrorCode::InvalidArgument, "rules must be exact RULE_ID@VERSION values", p.format);
    }
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let policy = Policy::template(&format!("pol_{}", &req[4..]), &current_principal(), scope, rules, mode, now_unix());
    if let Err(e) = stores.audit.save_policy(&policy, &now_utc()) {
        return store_error(req, &e, p.format);
    }
    emit(&policy_list_record(req, &stores), p.format);
    ExitCode::SUCCESS
}

pub fn policy_set_state(req: &str, p: &Parsed, id: &str, state: PolicyState) -> ExitCode {
    // Enabling grants future authority: it is a human act. Pausing/revoking only restricts.
    if state == PolicyState::Enabled && !interactive() {
        return fail(req, ErrorCode::InvalidArgument, "enabling a policy requires an interactive terminal; machine callers cannot grant authority", p.format);
    }
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let Some(mut policy) = stores.audit.policies().unwrap_or_default().into_iter().find(|x| x.policy_id == id) else {
        return fail(req, ErrorCode::InvalidArgument, "unknown policy", p.format);
    };
    if policy.state == PolicyState::Revoked {
        return fail(req, ErrorCode::Conflict, "a revoked policy cannot change state; create a new one", p.format);
    }
    if state == PolicyState::Enabled {
        println!("Policy {}: scope={} mode={:?} rules={}", policy.policy_id, policy.scope_id, policy.mode, policy.rules.join(", "));
        println!("Limits: {} bytes / {} items per plan; {} bytes / {} items per rolling 24 h; expires {}",
            policy.single_max_bytes, policy.single_max_items, policy.window_max_bytes, policy.window_max_items, policy.expires_at);
        match prompt(&format!("Type {} to enable it, anything else to cancel: ", policy.policy_id)) {
            Some(a) if a == policy.policy_id => {}
            _ => return fail(req, ErrorCode::Cancelled, "not enabled", p.format),
        }
    }
    policy.state = state;
    policy.version += 1;
    if let Err(e) = stores.audit.save_policy(&policy, &now_utc()) {
        return store_error(req, &e, p.format);
    }
    emit(&policy_list_record(req, &stores), p.format);
    ExitCode::SUCCESS
}

pub fn policy_list_record(req: &str, stores: &Stores) -> Value {
    let now = now_unix();
    let items: Vec<Value> = stores
        .audit
        .policies()
        .unwrap_or_default()
        .iter()
        .map(|pol| {
            let used = stores.audit.policy_usage(&pol.policy_id, now).unwrap_or_default();
            json!({"policy": pol, "window_used_bytes": used.bytes.to_string(), "window_used_items": used.items})
        })
        .collect();
    json!({"schema_version": "1.0", "kind": "policy_list", "request_id": req, "items": items, "as_of": utc_from_unix(now)})
}

pub fn policy_list(req: &str, p: &Parsed) -> ExitCode {
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    emit(&policy_list_record(req, &stores), p.format);
    ExitCode::SUCCESS
}
