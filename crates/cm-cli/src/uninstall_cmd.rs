//! `apps uninstall …` (parity UNI-01..10, tasks 9.1–9.8): two separately reviewed `uninstall` plans.
//!
//! - `apps uninstall list` (read-only): every instance with the uninstall route this build would use, or why there is
//!   none (capability explanation and the Settings entry; protected classes have no route at all).
//! - `apps uninstall plan create --instance ID [--end-running]`: stage 1 (quit + verified uninstaller). `--end-running`
//!   is the separately reviewed variant that may end still-running processes, and only after a person confirms it in a
//!   terminal at apply time; it is refused for non-interactive callers.
//! - `apps uninstall leftovers --operation OP` (read-only grouped preview) and `apps uninstall leftovers plan create
//!   --operation OP [--items ID,…]`: stage 2, only caches/logs by default plus objects named by id.
//! - `apps uninstall orphans` (read-only, review_only) and `apps uninstall restore --operation OP` (recovery plan).
//!
//! Nothing here runs an uninstaller or deletes anything: `plan review` approves and `plan apply` executes. Tests
//! redirect the registry and home folder with `CM_UNINSTALL_TEST_ROOT` / `CM_UNINSTALL_TEST_HOME`.

use std::process::ExitCode;

use cm_core::error::ErrorCode;
use cm_core::protocol::record;
use serde_json::{Value, json};

use crate::args::Parsed;
use crate::commands::{fields, open_stores};
use crate::{emit, fail};

fn ids(list: &str) -> Vec<String> {
    list.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect()
}

/// Error record with the Settings alternative as the required action.
fn refused(req: &str, p: &Parsed, r: &cm_app::uninstall::Refused) -> ExitCode {
    use cm_core::protocol::{ErrorDetails, error_record};
    let action = r.settings_uri.as_ref().map(|u| format!("Open {u} (Settings > Apps > Installed apps) or use the vendor's own uninstaller"));
    let code = r.error_code();
    emit(&error_record(req, code, &r.message, action.as_deref(), ErrorDetails::default()), p.format);
    ExitCode::from(cm_core::error::ExitClass::Error(code).code() as u8)
}

fn leftover_wire(l: &cm_core::uninstall::Leftover) -> Value {
    let (action, guidance) = match &l.binding {
        cm_core::uninstall::Binding::Path { .. } => ("fs_trash", None),
        cm_core::uninstall::Binding::Registry { .. } => ("registry_backup_delete", None),
        cm_core::uninstall::Binding::Startup { .. } => ("startup_toggle", None),
        cm_core::uninstall::Binding::Manual { guidance } => ("manual", Some(guidance.clone())),
    };
    json!({
        "object_id": l.object_id, "kind": l.kind, "group": l.group, "display": l.display, "evidence": l.evidence, "marker": l.marker, "tier": l.tier,
        "selected": l.selected, "warnings": l.warnings, "logical_bytes": l.logical_bytes.map(|b| b.to_string()), "note": l.note, "requires_admin": l.requires_admin,
        "action": action, "guidance": guidance,
    })
}

fn grouped(items: &[cm_core::uninstall::Leftover]) -> Value {
    let mut groups: std::collections::BTreeMap<String, Vec<Value>> = std::collections::BTreeMap::new();
    for l in items {
        let g = serde_json::to_value(l.group).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
        groups.entry(g).or_default().push(leftover_wire(l));
    }
    Value::Array(groups.into_iter().map(|(g, items)| json!({"group": g, "items": items})).collect())
}

#[cfg(windows)]
mod imp {
    use super::*;
    use cm_platform::windows::uninstall::{UninstallEnv, records, route, route_capability};

    fn env(req: &str, p: &Parsed) -> Result<UninstallEnv, ExitCode> {
        UninstallEnv::from_process().map_err(|e| fail(req, ErrorCode::InvalidArgument, &e, p.format))
    }

    pub fn list(req: &str, p: &Parsed) -> ExitCode {
        let env = match env(req, p) {
            Ok(e) => e,
            Err(c) => return c,
        };
        let (recs, issues) = records(&env);
        let items: Vec<Value> = recs
            .iter()
            .map(|r| {
                // The same decision `apps list` reports as the instance's uninstall capability.
                let route = route(&env, r);
                let state = route_capability(&route).state;
                let (adapter, reason, settings) = match &route {
                    Ok(c) => (Some(json!({"adapter_id": c.kind.adapter_id(), "display": c.display, "package_manager": c.kind.package_manager()})), None, None),
                    Err(x) => (None, Some(x.message.clone()), x.settings_uri),
                };
                // UNI-07: AI CLIs report when they were last used (newest entry of their data folder).
                let last_used = r.ai_tool.as_deref().and_then(|t| cm_platform::windows::uninstall::ai_last_used(&env, t)).map(cm_core::time::utc_from_unix);
                json!({
                    "app_instance_id": r.instance_id, "name": r.name, "version": r.version, "publisher": r.publisher, "source": r.wire_source(), "last_used_at": last_used,
                    "location": r.install_location, "protected_class": r.protected_class.is_some() || route.as_ref().is_err_and(|x| x.code == "policy_denied"),
                    "uninstall": {"state": state, "adapter": adapter, "reason": reason, "settings_uri": settings},
                })
            })
            .collect();
        let issue_records: Vec<Value> = issues.iter().map(|i| json!({"code": i.code, "scope_display": i.source, "message": i.message})).collect();
        emit(&record("app_uninstall_list", req, fields(json!({"items": items, "issues": issue_records}))), p.format);
        ExitCode::from(if issues.is_empty() { cm_core::error::ExitClass::Success } else { cm_core::error::ExitClass::Partial }.code() as u8)
    }

    pub fn plan_create(req: &str, p: &Parsed) -> ExitCode {
        let Some(instance) = p.option("instance") else { return fail(req, ErrorCode::InvalidArgument, "needs --instance APP_INSTANCE_ID", p.format) };
        let end_running = p.flag("end-running");
        // UNI-02: a plan that may end processes is asked for by a person in a terminal, never by an automated caller.
        if end_running && p.non_interactive() {
            return fail(req, ErrorCode::InvalidArgument, "--end-running is only accepted in an interactive terminal (a person decides to end processes)", p.format);
        }
        let env = match env(req, p) {
            Ok(e) => e,
            Err(c) => return c,
        };
        let stores = match open_stores(req, p) {
            Ok(s) => s,
            Err(c) => return c,
        };
        let plan_id = format!("up_{}", &req[4..]);
        let pdigest = crate::plan_cmd::current_policy_digest(&stores);
        match cm_app::uninstall::create_uninstall_plan(&stores, &env, instance, end_running, &plan_id, crate::plan_cmd::now_unix(), &pdigest) {
            Ok(s1) => {
                let mut rec = crate::plan_cmd::v2_plan_record(req, &stores, &s1.plan);
                // D7: shown in review as an estimate; the leftovers are a second, separately reviewed plan.
                let est: serde_json::Map<String, Value> = s1.estimate.iter().map(|(g, n)| (serde_json::to_value(g).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default(), json!(n))).collect();
                rec["leftover_estimate"] = Value::Object(est);
                emit(&rec, p.format);
                ExitCode::SUCCESS
            }
            Err(r) => refused(req, p, &r),
        }
    }

    pub fn leftovers(req: &str, p: &Parsed) -> ExitCode {
        let Some(op) = p.option("operation") else { return fail(req, ErrorCode::InvalidArgument, "needs --operation OP_ID (the uninstall operation)", p.format) };
        let env = match env(req, p) {
            Ok(e) => e,
            Err(c) => return c,
        };
        let stores = match open_stores(req, p) {
            Ok(s) => s,
            Err(c) => return c,
        };
        match cm_app::uninstall::leftovers(&stores, &env, op) {
            Ok((identity, items, refs)) => {
                let app = json!({"app_instance_id": identity.instance_id, "name": identity.name, "publisher": identity.publisher, "source": identity.source});
                let selected = items.iter().filter(|l| l.selected).count();
                emit(&record("uninstall_leftovers", req, fields(json!({
                    "operation_id": op, "app": app, "groups": grouped(&items), "default_selected": selected, "external_references": refs,
                }))), p.format);
                ExitCode::SUCCESS
            }
            Err(r) => refused(req, p, &r),
        }
    }

    pub fn leftovers_plan(req: &str, p: &Parsed) -> ExitCode {
        let Some(op) = p.option("operation") else { return fail(req, ErrorCode::InvalidArgument, "needs --operation OP_ID (the uninstall operation)", p.format) };
        let extra = p.option("items").map(ids).unwrap_or_default();
        let exclude = p.option("exclude").map(ids).unwrap_or_default();
        let env = match env(req, p) {
            Ok(e) => e,
            Err(c) => return c,
        };
        let stores = match open_stores(req, p) {
            Ok(s) => s,
            Err(c) => return c,
        };
        let plan_id = format!("lp_{}", &req[4..]);
        let pdigest = crate::plan_cmd::current_policy_digest(&stores);
        let host = cm_core::host::HostProbe::current(&cm_platform::host::OsHostProbe, "cli");
        match cm_app::uninstall::create_leftovers_plan(&stores, &env, op, &extra, &exclude, &host, &plan_id, crate::plan_cmd::now_unix(), &pdigest) {
            Ok((plan, _)) => {
                emit(&crate::plan_cmd::v2_plan_record(req, &stores, &plan), p.format);
                ExitCode::SUCCESS
            }
            Err(r) => refused(req, p, &r),
        }
    }

    pub fn orphans(req: &str, p: &Parsed) -> ExitCode {
        let env = match env(req, p) {
            Ok(e) => e,
            Err(c) => return c,
        };
        let (recs, _) = records(&env);
        let installed = cm_platform::windows::uninstall::installed_apps(&recs);
        let (startup, _) = cm_platform::windows::startup::inventory_with(&env.startup_env());
        let items = cm_platform::windows::uninstall::orphans(&env, &startup, &installed);
        emit(&record("uninstall_leftovers", req, fields(json!({
            "operation_id": null, "app": null, "groups": grouped(&items), "default_selected": 0, "external_references": [],
        }))), p.format);
        ExitCode::SUCCESS
    }
}

#[cfg(windows)]
pub use imp::{leftovers, leftovers_plan, list, orphans, plan_create};

#[cfg(not(windows))]
fn not_here(req: &str, p: &Parsed) -> ExitCode {
    let _ = (ids, grouped, refused);
    fail(req, ErrorCode::Unsupported, "application uninstall is implemented for Windows in this build (see docs/app-uninstall.md)", p.format)
}

#[cfg(not(windows))]
pub fn list(req: &str, p: &Parsed) -> ExitCode {
    not_here(req, p)
}
#[cfg(not(windows))]
pub fn plan_create(req: &str, p: &Parsed) -> ExitCode {
    not_here(req, p)
}
#[cfg(not(windows))]
pub fn leftovers(req: &str, p: &Parsed) -> ExitCode {
    not_here(req, p)
}
#[cfg(not(windows))]
pub fn leftovers_plan(req: &str, p: &Parsed) -> ExitCode {
    not_here(req, p)
}
#[cfg(not(windows))]
pub fn orphans(req: &str, p: &Parsed) -> ExitCode {
    not_here(req, p)
}

/// `apps uninstall restore --operation OP`: a `recovery` plan putting back deleted keys and removed startup values.
pub fn restore(req: &str, p: &Parsed) -> ExitCode {
    let Some(op) = p.option("operation") else { return fail(req, ErrorCode::InvalidArgument, "needs --operation OP_ID (the leftovers operation)", p.format) };
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let plan_id = format!("rp_{}", &req[4..]);
    let pdigest = crate::plan_cmd::current_policy_digest(&stores);
    match cm_app::uninstall::create_restore_plan(&stores, op, &plan_id, crate::plan_cmd::now_unix(), &pdigest) {
        Ok(plan) => {
            emit(&crate::plan_cmd::v2_plan_record(req, &stores, &plan), p.format);
            ExitCode::SUCCESS
        }
        Err(r) => refused(req, p, &r),
    }
}

/// Review lines for uninstall plans (`plan review`): what runs, what may be ended, and that leftovers come separately.
pub fn review_lines(plan: &cm_core::plan::v2::PlanV2) -> Vec<String> {
    use cm_core::plan::v2::{PlanAction, QuitForce};
    let mut out = Vec::new();
    for (a, m) in plan.body.items.iter().zip(&plan.meta) {
        let note = m.note.clone().unwrap_or_default();
        out.push(match a {
            PlanAction::AppQuit(q) if q.force == QuitForce::AskUser => format!("  {}  {note}: processes still running after the close request are ENDED only if you confirm when applying", q.action_id),
            PlanAction::AppQuit(q) => format!("  {}  {note}: never ended automatically; if it does not close, nothing is uninstalled", q.action_id),
            PlanAction::ToolAdapter(t) => format!("  {}  runs {note}; exit code and duration are recorded, then the folder and registration are checked again", t.action_id),
            PlanAction::RegistryBackupDelete(r) => format!("  {}  delete {}\\{} after its backup is stored ({})", r.action_id, r.hive.as_str(), r.key_path, r.owner_evidence),
            PlanAction::FsTrash(f) => format!("  {}  to the Recycle Bin: {}  {note}", f.action_id, f.target.path.display()),
            PlanAction::StartupToggle(s) => format!("  {}  remove stale startup entry {note} after its backup", s.action_id),
            other => format!("  {}  {}", other.action_id(), other.action_type().as_str()),
        });
    }
    if plan.body.items.iter().any(|a| matches!(a, PlanAction::ToolAdapter(_))) {
        out.push("Leftovers are not part of this plan: after it ran, review them with `cleanmanager apps uninstall leftovers --operation <operation>`.".into());
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn ids_split_and_trim() {
        assert_eq!(super::ids(" a, b ,,c"), ["a", "b", "c"]);
    }
}
