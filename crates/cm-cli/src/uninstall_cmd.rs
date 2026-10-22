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
//! - `apps uninstall orphans` (read-only, review_only) and `apps uninstall restore --operation OP` (recovery plan). On
//!   macOS `apps uninstall orphans plan create --items ID,…` plans moving the named orphans to the Trash (reviewed on
//!   its own; nothing is preselected); on Windows orphans are listed for review only.
//!
//! Nothing here runs an uninstaller or deletes anything: `plan review` approves and `plan apply` executes. Tests
//! redirect the registry and home folder with `CM_UNINSTALL_TEST_ROOT` / `CM_UNINSTALL_TEST_HOME` (on macOS: a fake `/`
//! and home, see `cm_platform::macos::uninstall`).
//!
//! The same command bodies serve Windows and macOS (`plat` names the platform module); macOS routes are the bundle to
//! the Trash, the Homebrew adapters and `npm uninstall --global` for AI command-line tools; orphans are startup-entry
//! based on Windows and `~/Library` bundle-ID based on macOS.

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

#[cfg(any(windows, target_os = "macos"))]
mod imp {
    use super::*;
    use crate::commands::fields;
    use cm_core::protocol::record;
    #[cfg(target_os = "macos")]
    use cm_platform::macos::uninstall as plat;
    #[cfg(windows)]
    use cm_platform::windows::uninstall as plat;
    use plat::{UninstallEnv, records, route, route_capability};

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
                let last_used = r.ai_tool.as_deref().and_then(|t| plat::ai_last_used(&env, t)).map(cm_core::time::utc_from_unix);
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

    /// UNI-08 on macOS: `~/Library` leftovers of apps removed earlier, by bundle-ID evidence; never preselected.
    #[cfg(target_os = "macos")]
    pub fn orphans(req: &str, p: &Parsed) -> ExitCode {
        let env = match env(req, p) {
            Ok(e) => e,
            Err(c) => return c,
        };
        let items = plat::orphans(&env, true);
        emit(&record("uninstall_leftovers", req, fields(json!({
            "operation_id": null, "app": null, "groups": grouped(&items), "default_selected": 0, "external_references": [],
        }))), p.format);
        ExitCode::SUCCESS
    }

    /// UNI-08 on macOS: a separately reviewed Trash plan for the orphaned leftovers named with `--items`.
    #[cfg(target_os = "macos")]
    pub fn orphans_plan(req: &str, p: &Parsed) -> ExitCode {
        let items = p.option("items").map(ids).unwrap_or_default();
        let env = match env(req, p) {
            Ok(e) => e,
            Err(c) => return c,
        };
        let stores = match open_stores(req, p) {
            Ok(s) => s,
            Err(c) => return c,
        };
        let plan_id = format!("orp_{}", &req[4..]);
        let pdigest = crate::plan_cmd::current_policy_digest(&stores);
        let host = cm_core::host::HostProbe::current(&cm_platform::host::OsHostProbe, "cli");
        match cm_app::uninstall::create_orphans_plan(&stores, &env, &items, &host, &plan_id, crate::plan_cmd::now_unix(), &pdigest) {
            Ok((plan, _)) => {
                emit(&crate::plan_cmd::v2_plan_record(req, &stores, &plan), p.format);
                ExitCode::SUCCESS
            }
            Err(r) => refused(req, p, &r),
        }
    }

    #[cfg(windows)]
    pub fn orphans_plan(req: &str, p: &Parsed) -> ExitCode {
        fail(req, ErrorCode::Unsupported, "on Windows, orphaned leftovers are listed for review only in this build (`apps uninstall orphans`); remove them yourself after review", p.format)
    }

    #[cfg(windows)]
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

#[cfg(any(windows, target_os = "macos"))]
pub use imp::{leftovers, leftovers_plan, list, orphans, orphans_plan, plan_create};

#[cfg(not(any(windows, target_os = "macos")))]
fn not_here(req: &str, p: &Parsed) -> ExitCode {
    let _ = (ids, grouped, refused);
    fail(req, ErrorCode::Unsupported, "application uninstall is implemented for Windows and macOS in this build (see docs/app-uninstall.md)", p.format)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn list(req: &str, p: &Parsed) -> ExitCode {
    not_here(req, p)
}
#[cfg(not(any(windows, target_os = "macos")))]
pub fn plan_create(req: &str, p: &Parsed) -> ExitCode {
    not_here(req, p)
}
#[cfg(not(any(windows, target_os = "macos")))]
pub fn leftovers(req: &str, p: &Parsed) -> ExitCode {
    not_here(req, p)
}
#[cfg(not(any(windows, target_os = "macos")))]
pub fn leftovers_plan(req: &str, p: &Parsed) -> ExitCode {
    not_here(req, p)
}
#[cfg(not(any(windows, target_os = "macos")))]
pub fn orphans(req: &str, p: &Parsed) -> ExitCode {
    not_here(req, p)
}
#[cfg(not(any(windows, target_os = "macos")))]
pub fn orphans_plan(req: &str, p: &Parsed) -> ExitCode {
    not_here(req, p)
}

/// Seam for `plan apply` (plan/v2 backends): `uninstall` plans on macOS (quit, bundle to the Trash, Homebrew adapters).
/// UNI-02: ending still-running processes needs a plan reviewed for it and, at apply time, a person typing END.
#[cfg(target_os = "macos")]
pub(crate) fn mac_uninstall_backend() -> Option<Box<dyn cm_core::uninstall::UninstallActions>> {
    let env = cm_platform::macos::uninstall::UninstallEnv::from_process().ok()?;
    let confirm = Box::new(|procs: &[cm_core::uninstall::RunningProcess]| {
        let names: Vec<String> = procs.iter().map(|x| x.describe()).collect();
        crate::plan_cmd::interactive()
            && crate::plan_cmd::prompt(&format!("Still running: {}. Type END to end them now, anything else to cancel (nothing is uninstalled then): ", names.join(", "))).as_deref()
                == Some("END")
    });
    Some(Box::new(cm_platform::macos::uninstall::MacUninstall::new(env, confirm)))
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
            PlanAction::ToolAdapter(t) if t.adapter_id == cm_core::uninstall::AdapterKind::MacTrashAppBundle.adapter_id() => {
                format!("  {}  {note}; the bundle must still be the planned one, then its location is checked again", t.action_id)
            }
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
    use cm_core::uninstall::{Binding, Group, LEFTOVER_TIERS, Leftover, LeftoverKind, Marker, RegLocation, finish_leftover, object_id};
    use serde_json::Value;

    #[test]
    fn ids_split_and_trim() {
        assert_eq!(super::ids(" a, b ,,c"), ["a", "b", "c"]);
    }

    fn enum_of(def: &Value, field: &str) -> Vec<String> {
        let p = &def["properties"][field];
        let e = if p["enum"].is_array() { &p["enum"] } else { &p["anyOf"][0]["enum"] };
        e.as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect()
    }

    /// WFA-113: every leftover the platforms can produce — each kind, each binding (including guidance-only manual
    /// items such as a startup entry whose program still exists or a desktop shortcut), with or without a marker and
    /// whatever tier the platform started from — serializes to a record that satisfies `schemas/v1`
    /// `uninstall_leftover`: required fields only, enum values (never an empty tier) and the preselection rules.
    #[test]
    fn every_leftover_kind_and_binding_satisfies_the_schema() {
        let schema: Value = serde_json::from_str(include_str!("../../../schemas/v1/protocol.schema.json")).unwrap();
        let def = &schema["$defs"]["uninstall_leftover"];
        let required: Vec<&str> = def["required"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        let (tiers, kinds, groups, actions, markers) = (enum_of(def, "tier"), enum_of(def, "kind"), enum_of(def, "group"), enum_of(def, "action"), enum_of(def, "marker"));
        assert_eq!(tiers, LEFTOVER_TIERS, "the core's tier list is the schema's");
        let loc = RegLocation { root: String::new(), hive: cm_core::plan::v2::Hive::Hkcu, view: cm_core::startup::RegView::Default, key_path: r"Software\Vendor\App".into() };
        let bindings = [
            Binding::Path { path: r"C:\Users\A\AppData\Local\App\Cache".into() },
            Binding::Registry { location: loc },
            Binding::Startup { startup_item_id: "st_x".into() },
            Binding::Manual { guidance: "its program still exists; uninstall first, then review leftovers again".into() },
        ];
        let all_kinds = [LeftoverKind::Directory, LeftoverKind::File, LeftoverKind::RegistryKey, LeftoverKind::StartupEntry, LeftoverKind::Shortcut, LeftoverKind::Service];
        let all_groups = [Group::Bundle, Group::CachesLogs, Group::Settings, Group::PersonalData, Group::Startup, Group::System];
        let mut checked = 0;
        for kind in all_kinds {
            for group in all_groups {
                for binding in &bindings {
                    for marker in [None, Some(Marker::SharedOwner), Some(Marker::OwnerUnknown), Some(Marker::AppReappeared)] {
                        for tier in ["", "review_only", "blocked", "kept", "default_selected", "bogus"] {
                            for requires_admin in [false, true] {
                                let display = format!("{kind:?} {group:?} item");
                                let l = finish_leftover(Leftover {
                                    object_id: object_id(kind, &display), kind, group, display, evidence: vec!["evidence".into()], marker, tier: tier.into(),
                                    selected: false, warnings: vec![], logical_bytes: Some(1), note: None, requires_admin, binding: binding.clone(),
                                });
                                let w = super::leftover_wire(&l);
                                let o = w.as_object().unwrap();
                                let mut keys: Vec<&str> = o.keys().map(String::as_str).collect();
                                keys.sort_unstable();
                                let mut want = required.clone();
                                want.sort_unstable();
                                assert_eq!(keys, want, "exactly the schema's fields: {w}");
                                let s = |f: &str| w[f].as_str().unwrap_or_default().to_owned();
                                assert!(tiers.contains(&s("tier")), "tier {:?} (started from {tier:?}) is not in the schema: {w}", s("tier"));
                                assert!(kinds.contains(&s("kind")) && groups.contains(&s("group")) && actions.contains(&s("action")), "{w}");
                                assert!(w["marker"].is_null() || markers.contains(&s("marker")), "{w}");
                                if w["selected"] == true {
                                    assert_eq!((s("group").as_str(), s("tier").as_str()), ("caches_logs", "default_selected"), "{w}");
                                }
                                if !w["marker"].is_null() {
                                    assert_eq!(s("tier"), "kept", "{w}");
                                }
                                if s("action") == "manual" {
                                    assert!(w["guidance"].is_string() && w["selected"] == false && s("tier") != "default_selected", "manual items are never preselected: {w}");
                                }
                                checked += 1;
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(checked, 6 * 6 * 4 * 4 * 6 * 2);
    }
}
