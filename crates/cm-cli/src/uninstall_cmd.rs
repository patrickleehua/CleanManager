//! `apps uninstall …` (parity UNI-01..10, tasks 9.1–9.8): two separately reviewed `uninstall` plans.
//!
//! - `apps uninstall list` (read-only): every instance with the uninstall route this build would use, or why there is
//!   none (capability explanation and the Settings entry; protected classes have no route at all).
//! - `apps uninstall plan create --instance ID [--instance ID …] [--end-running]`: stage 1 (quit + verified
//!   uninstaller), for one application or a batch in one plan (UNI-11: each application keeps its own steps and
//!   result; refused ones are listed and left out). `--end-running` is the separately reviewed variant that may end
//!   still-running processes, and only after a person confirms it in a terminal at apply time; it is refused for
//!   non-interactive callers.
//! - `apps uninstall leftovers --operation OP` (read-only grouped preview) and `apps uninstall leftovers plan create
//!   --operation OP [--items ID,…]`: stage 2, only caches/logs by default plus objects named by id.
//! - `apps uninstall orphans` (read-only, review_only) and `apps uninstall restore --operation OP` (recovery plan).
//!
//! Nothing here runs an uninstaller or deletes anything: `plan review` approves and `plan apply` executes. Tests
//! redirect the registry and home folder with `CM_UNINSTALL_TEST_ROOT` / `CM_UNINSTALL_TEST_HOME`.

use std::process::ExitCode;

use cm_core::error::ErrorCode;
use serde_json::{Value, json};

use crate::args::Parsed;
use crate::commands::open_stores;
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
    use crate::commands::fields;
    use cm_core::protocol::record;
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
        // UNI-11: `--instance` may be repeated; every application of the batch is checked exactly as on its own.
        let instances: Vec<String> = p.values("instance").into_iter().map(str::to_owned).collect();
        if instances.is_empty() {
            return fail(req, ErrorCode::InvalidArgument, "needs --instance APP_INSTANCE_ID (repeat it to uninstall several applications in one plan)", p.format);
        }
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
        match cm_app::uninstall::create_uninstall_batch(&stores, &env, &instances, end_running, &plan_id, crate::plan_cmd::now_unix(), &pdigest) {
            Ok(b) => {
                let mut rec = crate::plan_cmd::v2_plan_record(req, &stores, &b.plan);
                // D7: shown in review as an estimate (summed over the batch); the leftovers are a second, separately
                // reviewed plan. UNI-11: per application, its actions and estimate; refused applications are listed.
                let mut total: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
                let apps: Vec<Value> = b
                    .apps
                    .iter()
                    .map(|a| {
                        let est: serde_json::Map<String, Value> = a.estimate.iter().map(|(g, n)| (group_word(g), json!(n))).collect();
                        for (g, n) in &a.estimate {
                            *total.entry(group_word(g)).or_insert(0) += n;
                        }
                        json!({"app_instance_id": a.record.instance_id, "name": a.record.name, "action_ids": a.action_ids, "leftover_estimate": est})
                    })
                    .collect();
                rec["leftover_estimate"] = json!(total);
                rec["uninstall_apps"] = Value::Array(apps);
                rec["uninstall_refused"] = Value::Array(
                    b.refused
                        .iter()
                        .map(|x| json!({"app_instance_id": x.app_instance_id, "name": x.name, "code": x.refused.code, "message": x.refused.message, "settings_uri": x.refused.settings_uri}))
                        .collect(),
                );
                emit(&rec, p.format);
                // Some applications were refused: the plan holds the others, the exit says not everything was planned.
                ExitCode::from(if b.refused.is_empty() { cm_core::error::ExitClass::Success } else { cm_core::error::ExitClass::Partial }.code() as u8)
            }
            Err(r) => refused(req, p, &r),
        }
    }

    fn group_word(g: &cm_core::uninstall::Group) -> String {
        serde_json::to_value(g).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
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
        let by_app = match cm_app::uninstall::leftovers_by_app(&stores, &env, op) {
            Ok(a) => a,
            Err(r) => return refused(req, p, &r),
        };
        // A failed, skipped or stale uninstall keeps everything of that application (UNI-11).
        if let Some(r) = cm_app::uninstall::none_reviewable(op, &by_app) {
            return refused(req, p, &r);
        }
        let app_wire = |i: &cm_core::uninstall::AppIdentity| json!({"app_instance_id": i.instance_id, "name": i.name, "publisher": i.publisher, "source": i.source});
        // Top level: every reviewable object of the operation (one application: exactly its review, as before).
        let all: Vec<cm_core::uninstall::Leftover> = by_app.iter().flat_map(|a| a.items.iter().cloned()).collect();
        let mut refs: Vec<String> = Vec::new();
        for r in by_app.iter().flat_map(|a| &a.external_references) {
            if !refs.contains(r) {
                refs.push(r.clone());
            }
        }
        let apps: Vec<Value> = by_app
            .iter()
            .map(|a| {
                json!({
                    "app": app_wire(&a.identity), "status": a.outcome.status, "code": a.outcome.code, "message": a.outcome.message,
                    "leftovers_reviewable": a.outcome.leftovers_reviewable, "groups": grouped(&a.items), "default_selected": a.items.iter().filter(|l| l.selected).count(),
                    "external_references": a.external_references,
                })
            })
            .collect();
        let app = if by_app.len() == 1 { app_wire(&by_app[0].identity) } else { Value::Null };
        emit(&record("uninstall_leftovers", req, fields(json!({
            "operation_id": op, "app": app, "groups": grouped(&all), "default_selected": all.iter().filter(|l| l.selected).count(), "external_references": refs,
            "apps": apps,
        }))), p.format);
        ExitCode::SUCCESS
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

/// Seam for `plan apply` (plan/v2 backends): `uninstall` plans on macOS (quit, bundle to Trash, Homebrew adapters).
/// `None` = unsupported here.
#[cfg(target_os = "macos")]
pub(crate) fn mac_uninstall_backend() -> Option<Box<dyn cm_core::uninstall::UninstallActions>> {
    None
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

/// UNI-11: an `operation_report` of a stage-1 uninstall also reports each application's own result.
pub fn add_app_outcomes(stores: &cm_store::Stores, report: &cm_app::execute::OperationReport, rec: &mut Value) {
    let Ok(plan) = stores.audit.load_plan_v2(&report.plan_id) else { return };
    let outcomes = cm_app::uninstall::app_outcomes(&plan, Some(report));
    if !outcomes.is_empty() {
        rec["uninstall_apps"] = json!(outcomes);
    }
}

/// Review lines for uninstall plans (`plan review`): what runs, what may be ended, and that leftovers come separately.
/// UNI-11: a plan for several applications is reviewed application by application.
pub fn review_lines(plan: &cm_core::plan::v2::PlanV2) -> Vec<String> {
    use cm_core::plan::v2::{PlanAction, QuitForce};
    let mut out = Vec::new();
    let apps = cm_app::uninstall::planned_apps(plan);
    if apps.len() > 1 {
        out.push(format!("{} applications, each with its own close request and uninstaller; one that fails or is skipped does not stop the others:", apps.len()));
    }
    for (a, m) in plan.body.items.iter().zip(&plan.meta) {
        if apps.len() > 1
            && let Some((i, app)) = apps.iter().enumerate().find(|(_, p)| p.action_ids.first().map(String::as_str) == Some(a.action_id()))
        {
            out.push(format!("Application {}/{}: {} ({})", i + 1, apps.len(), app.identity.name, app.identity.instance_id));
        }
        let note = m.note.clone().unwrap_or_default();
        out.push(match a {
            PlanAction::AppQuit(q) if q.force == QuitForce::AskUser => format!("  {}  {note}: processes still running after the close request are ENDED only if you confirm when applying", q.action_id),
            PlanAction::AppQuit(q) => format!("  {}  {note}: never ended automatically; if it does not close, nothing is uninstalled", q.action_id),
            PlanAction::ToolAdapter(t) => format!("  {}  runs {note}; exit code and duration are recorded, then the folder and registration are checked again", t.action_id),
            PlanAction::RegistryBackupDelete(r) => format!("  {}  delete {}\\{} after its backup is stored ({})", r.action_id, r.hive.as_str(), r.key_path, r.owner_evidence),
            PlanAction::FsTrash(f) => format!("  {}  to the {}: {}  {note}", f.action_id, cm_core::manual_trash::BIN_NAME, f.target.path.display()),
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
