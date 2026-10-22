//! Plan and policy commands (PLN-01..07, CLI-02/06/07).
//!
//! Machines may create plans and (paused) policies. Approving a plan and enabling a policy are
//! human acts: they require an interactive terminal and a typed confirmation. A TTY is a
//! presentation control, not proof of a human; it does not isolate a same-user process that can
//! drive a console (documented threat boundary, PLN-07).

use std::io::{BufRead, IsTerminal, Write};
use std::process::ExitCode;

use cm_app::plans::{PlanType, plan_type};
use cm_core::analysis::CandidateSet;
use cm_core::authz::{KindReceipt, Policy, PolicyState, Receipt, ReceiptSource, current_principal, receipt_valid};
use cm_core::plan::v2::{PlanAction, PlanKind, PlanV2};
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
    let (now, principal) = (now_unix(), current_principal());
    // WFA-42: the receipt that authorizes this exact plan, or that authorized it (a used policy receipt or a review
    // stays the answer after the plan expired); a revoked one never counts.
    let receipts = stores.audit.receipts_for(&plan.body.plan_id).unwrap_or_default();
    let by = receipts.iter().find(|r| receipt_valid(r, plan, &principal, now)).or_else(|| {
        receipts.iter().filter(|r| r.revoked_at.is_none() && r.plan_digest == plan.digest && r.principal == principal).max_by(|a, b| a.created_at.cmp(&b.created_at))
    });
    let authorized_by = by.map(|r| json!({
        "source": r.source,
        "policy_id": r.policy_id,
        "policy_version": r.policy_version.map(|v| v.to_string()),
        "authorized_at": r.created_at,
    }));
    let mut rec = record("plan", req, fields(json!({
        "body": plan.body,
        "plan_digest": plan.digest,
        "authorization_status": auth["status"],
        "authorized_by": authorized_by,
    })));
    // While a policy could still authorize it: which one would, and why each other one does not (WFA-42/43).
    if matches!(auth["status"].as_str(), Some("required" | "revoked")) {
        rec["policy_matches"] = json!(cm_app::execute::policy_matches(stores, plan, &principal, now).unwrap_or_default());
    }
    rec
}

/// PRT-02: no plan is created while the protection list is unusable (BUG-WFA-03).
pub fn require_protection(req: &str, p: &Parsed, stores: &Stores) -> Result<(), ExitCode> {
    cm_platform::protection::load(&stores.dir).map(|_| ()).map_err(|e| crate::protect_cmd::unavailable(req, &stores.dir, &e, p.format))
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
    if let Err(c) = require_protection(req, p, &stores) {
        return c;
    }
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
    cm_core::debug_trace!("plan", "set={set_id} scan={} mode={mode:?} candidates={} selected={} explicit_ids={}", set.scan_id, set.candidates.len(), selected.len(), p.option("candidates").is_some());
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
        Err(e) => {
            cm_core::debug_trace!("plan", "refused: {e}");
            return fail(req, ErrorCode::PolicyDenied, &e.to_string(), p.format);
        }
    };
    cm_core::debug_trace!("plan", "built plan={} items={} digest={}", plan.body.plan_id, plan.body.items.len(), plan.digest);
    // MTR-03 / WFA-35: trash mode needs a usable bin on every target's volume; refused before the plan is stored.
    if let Err(c) = crate::trash_cmd::plan_bins_ready(req, p, &set, &selected, mode) {
        return c;
    }
    if let Err(e) = stores.audit.save_plan(&plan) {
        return store_error(req, &e, p.format);
    }
    emit(&plan_record(req, &stores, &plan), p.format);
    ExitCode::SUCCESS
}

/// The stored, authoritative plan type decides who shows, reviews, revokes and applies a plan: `plan/v1` here,
/// `recovery/v1` in `recovery_cmd`, `plan/v2` by its kind (the `v2_*` functions below and `apply_cmd`).
pub fn stored_type(stores: &Stores, id: &str) -> Option<PlanType> {
    plan_type(stores, id).ok()
}

pub fn plan_show(req: &str, p: &Parsed, id: &str) -> ExitCode {
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    match stored_type(&stores, id) {
        Some(PlanType::RecoveryV1) => return crate::recovery_cmd::plan_show(req, p, &stores, id),
        Some(PlanType::V2(_)) => return v2_show(req, p, &stores, id),
        _ => {}
    }
    match stores.audit.load_plan(id) {
        Ok(plan) => {
            emit(&plan_record(req, &stores, &plan), p.format);
            ExitCode::SUCCESS
        }
        Err(e) => store_error(req, &e, p.format),
    }
}

pub fn interactive() -> bool {
    !crate::args::non_interactive_requested() && std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

pub fn prompt(line: &str) -> Option<String> {
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
    match stored_type(&stores, id) {
        Some(PlanType::RecoveryV1) => return crate::recovery_cmd::plan_review(req, p, &stores, id),
        Some(PlanType::V2(_)) => return v2_review(req, p, &stores, id),
        _ => {}
    }
    let plan = match stores.audit.load_plan(id) {
        Ok(p) => p,
        Err(e) => return store_error(req, &e, p.format),
    };
    if parse_utc(&plan.body.expires_at).is_none_or(|e| now_unix() >= e) {
        return fail(req, ErrorCode::PlanStale, "plan expired; create a new plan", p.format);
    }
    let total = plan.body.total_logical().map(|b| b.to_string()).unwrap_or_else(|| "unknown".into());
    println!("Plan {}  mode={}  items={}  logical_bytes={total}", plan.body.plan_id, plan.body.mode.as_str(), plan.body.items.len());
    println!("Rules: {}", plan.rule_refs.join(", "));
    println!("Expires: {}   Digest: {}", plan.body.expires_at, plan.digest);
    match plan.body.mode {
        Mode::Delete => println!("Permanent delete: there is no undo."),
        Mode::Trash => println!("Moved to the {}; restore with the system UI.", cm_core::manual_trash::BIN_NAME),
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
    match stored_type(&stores, id) {
        Some(PlanType::RecoveryV1) => return crate::recovery_cmd::plan_revoke(req, p, &stores, id),
        Some(PlanType::V2(_)) => return v2_revoke(req, p, &stores, id),
        _ => {}
    }
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
        println!("Policy {}: scope={} mode={} rules={}", policy.policy_id, policy.scope_id, policy.mode.as_str(), policy.rules.join(", "));
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

// ---------------------------------------------------------------------------------------------
// plan/v2 (manual selection, startup, startup recovery): shown, reviewed and revoked by kind.

fn load_v2(req: &str, p: &Parsed, stores: &Stores, id: &str) -> Result<PlanV2, ExitCode> {
    stores.audit.load_plan_v2(id).map_err(|e| store_error(req, &e, p.format))
}

/// Authorization state of a plan/v2 plan from stored kind receipts (policies never apply to these kinds).
pub fn v2_authorization_record(req: &str, stores: &Stores, plan: &PlanV2) -> Value {
    let (status, r) = cm_app::plans::authorization(stores, plan, &current_principal(), now_unix()).unwrap_or(("required", None));
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

pub fn v2_plan_record(req: &str, stores: &Stores, plan: &PlanV2) -> Value {
    let auth = v2_authorization_record(req, stores, plan);
    record("plan", req, fields(json!({"body": plan.body, "plan_digest": plan.digest, "authorization_status": auth["status"]})))
}

fn v2_show(req: &str, p: &Parsed, stores: &Stores, id: &str) -> ExitCode {
    match load_v2(req, p, stores, id) {
        Ok(plan) => {
            emit(&v2_plan_record(req, stores, &plan), p.format);
            ExitCode::SUCCESS
        }
        Err(c) => c,
    }
}

fn v2_revoke(req: &str, p: &Parsed, stores: &Stores, id: &str) -> ExitCode {
    let plan = match load_v2(req, p, stores, id) {
        Ok(plan) => plan,
        Err(c) => return c,
    };
    if let Err(e) = stores.audit.revoke_receipts(id, &now_utc()) {
        return store_error(req, &e, p.format);
    }
    emit(&v2_authorization_record(req, stores, &plan), p.format);
    ExitCode::SUCCESS
}

pub fn bytes_text(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{n} B") } else { format!("{v:.1} {} ({n} bytes)", UNITS[u]) }
}

/// What a person reads before approving a plan/v2 plan; returns whether an extra irreversibility confirmation is needed.
fn v2_summary(plan: &PlanV2) -> bool {
    let b = &plan.body;
    println!("Plan {}  kind={}  actions={}", b.plan_id, b.plan_kind.as_str(), b.items.len());
    println!("Expires: {}   Digest: {}", b.expires_at, plan.digest);
    if b.requires_admin && b.plan_kind == PlanKind::Maintenance {
        println!("Administrator tasks are marked below: `plan apply` asks Windows once (UAC) for a short-lived worker that runs only them; declining skips them.");
    } else if b.requires_admin {
        println!("Needs an administrator: apply it from an elevated terminal (CleanManager never elevates itself).");
    }
    let shown = b.items.iter().zip(&plan.meta).take(20);
    let permanent = b.items.iter().any(|a| matches!(a, PlanAction::FsDelete(_)));
    match b.plan_kind {
        PlanKind::Uninstall => {
            let _ = shown;
            crate::uninstall_cmd::review_lines(plan).iter().for_each(|l| println!("{l}"));
        }
        PlanKind::ManualSelection if permanent => {
            println!("PERMANENTLY deletes these objects from the {} ({}). There is no undo.", cm_core::manual_trash::BIN_NAME, bytes_text(b.total_logical()));
            for (a, m) in shown {
                let f = a.fs().expect("manual actions are file actions");
                let was = m.note.as_deref().map(|n| format!("  (was {n})")).unwrap_or_default();
                println!("  {}  {}  {}{was}", f.action_id, bytes_text(f.target.logical_bytes.0), f.target.path.display());
            }
        }
        PlanKind::ManualSelection => {
            println!("Moves these objects to the {} ({}); nothing is permanently deleted.", cm_core::manual_trash::BIN_NAME, bytes_text(b.total_logical()));
            println!("{}", cm_core::manual_trash::PUT_BACK_GUIDANCE);
            for (a, _) in shown {
                let f = a.fs().expect("manual actions are file actions");
                let inside = f.subtree.as_ref().map(|s| format!("  (folder, {} items inside)", s.items)).unwrap_or_default();
                println!("  {}  {}  {}{inside}", f.action_id, bytes_text(f.target.logical_bytes.0), f.target.path.display());
            }
        }
        _ => {
            for (a, m) in shown {
                let name = m.note.as_deref().unwrap_or("");
                match a {
                    PlanAction::StartupToggle(t) => println!("  {}  {} {name}  {}  scope={}", t.action_id, t.operation.as_str(), t.definition.as_deref().unwrap_or(""), t.scope.as_str()),
                    PlanAction::RegistryRestore(r) => println!("  {}  recreate {}\\{} : {} unless it exists again", r.action_id, r.hive.as_str(), r.key_path, r.value_name.as_deref().unwrap_or("(key)")),
                    PlanAction::MaintenanceTask(t) => println!(
                        "  {}  {}@{}  {name}{}{}",
                        t.action_id,
                        t.task_id,
                        t.task_version,
                        t.volume.as_deref().map(|v| format!("  volume {v}")).unwrap_or_default(),
                        if t.requires_admin { "  (administrator)" } else { "" }
                    ),
                    PlanAction::SettingsChange(c) => println!("  {}  set {}: {} -> {}  {name}", c.action_id, c.setting_id, c.from_value, c.to_value),
                    PlanAction::ProcessEnd(e) => println!("  {}  end {name}  {}", e.action_id, e.image_path.as_ref().map(|i| i.display()).unwrap_or_default()),
                    PlanAction::ToolAdapter(t) if b.plan_kind == PlanKind::Update => println!("  {}  {}", t.action_id, cm_app::updates::describe(t, m)),
                    other => println!("  {}  {}", other.action_id(), other.action_type().as_str()),
                }
            }
            if b.plan_kind == PlanKind::Process {
                println!("Ends this one process now; anything unsaved in it is lost. It is re-identified (PID, start time, executable) before it is ended.");
            }
            if b.plan_kind == PlanKind::Update {
                println!("Each update first downloads the package and checks its signer against the installed app; a mismatch or a running app skips that item.");
            }
            if b.plan_kind == PlanKind::Maintenance {
                println!("Each task checks first whether it applies and reports applied/unchanged/skipped/unavailable/attention/failed; maintenance is not undone.");
            }
            if b.plan_kind == PlanKind::Startup {
                println!("Only the enable record (as in Task Manager) changes, or a stale Run value after its backup; the prior state is recorded for `startup restore`.");
            }
        }
    }
    if b.items.len() > 20 {
        println!("  ... {} more actions (plan show {} lists them all)", b.items.len() - 20, b.plan_id);
    }
    permanent
}

fn v2_review(req: &str, p: &Parsed, stores: &Stores, id: &str) -> ExitCode {
    let plan = match load_v2(req, p, stores, id) {
        Ok(plan) => plan,
        Err(c) => return c,
    };
    match cm_core::plan::v2::check_plan_v2(&plan, now_unix()) {
        Ok(()) => {}
        Err(cm_core::plan::v2::PlanProblem::Kind(k)) => return fail(req, ErrorCode::PolicyDenied, &format!("plan violates its kind: {k}"), p.format),
        Err(_) => return fail(req, ErrorCode::PlanStale, "plan expired or changed; create a new plan", p.format),
    }
    let permanent = v2_summary(&plan);
    // MTR-03: a permanent delete needs its own explicit irreversibility confirmation before the digest.
    if permanent {
        match prompt("Type DELETE to confirm these objects cannot be recovered, anything else to cancel: ") {
            Some(a) if a == "DELETE" => {}
            _ => return fail(req, ErrorCode::Cancelled, "not approved", p.format),
        }
    }
    let code = &plan.digest[..8];
    match prompt(&format!("Type {code} to approve this exact plan, anything else to cancel: ")) {
        Some(answer) if answer == code => {}
        _ => return fail(req, ErrorCode::Cancelled, "not approved", p.format),
    }
    let receipt = KindReceipt {
        receipt_id: format!("r_{}", &req[4..]),
        plan_id: plan.body.plan_id.clone(),
        plan_digest: plan.digest.clone(),
        plan_kind: plan.body.plan_kind,
        principal: current_principal(),
        source: ReceiptSource::InteractiveReview,
        created_at: now_utc(),
        expires_at: plan.body.expires_at.clone(),
        revoked_at: None,
    };
    if let Err(e) = stores.audit.save_kind_receipt(&receipt) {
        return store_error(req, &e, p.format);
    }
    emit(&v2_authorization_record(req, stores, &plan), p.format);
    ExitCode::SUCCESS
}

/// Policy digest bound into new plans of every kind.
pub fn current_policy_digest(stores: &Stores) -> String {
    policy_digest(stores)
}
