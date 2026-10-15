//! 软件 › 卸载 (UNI-01..10) and 软件 › 更新 (UPD-01..04) for the desktop, over the same services the CLI uses
//! (`cm_app::uninstall`, `cm_app::updates`; `apps uninstall …`, `apps updates`, `apps update`).
//!
//! Safety properties of this IPC surface:
//! - The WebView names apps, operations and leftover objects by the ids the host listed (`app_instance_id`,
//!   `operation_id`, `object_id`); it never sends a path, a command line or a plan body.
//! - Listing is read-only (registry, package manager and folder reads). An update check runs the package manager's own
//!   query only when the person asks and checks are on; with checks off nothing is run or sent (UPD-04).
//! - Every change is a stored plan pending review: stage 1 of an uninstall (close request + verified uninstaller,
//!   never the `--end-running` variant, which stays in a terminal), the separately reviewed leftovers plan, a restore
//!   of backed-up registry keys, and one update plan for the verifiable package-manager updates. Nothing here
//!   authorizes or executes; `approve_plan` and `start_apply` do, through the shared review flow.
//! - `open_system_page` opens one of two fixed system pages (Settings › Apps, the Store's updates); no other target.

use std::collections::{BTreeMap, HashMap};

use cm_core::error::ErrorCode;
use cm_core::plan::v2::{PlanKind, PlanV2};
use serde::Serialize;
use serde_json::{Value, json};

use crate::args;
use crate::cleanup::CreatedPlanDto;
use crate::commands::{St, blocking};
use crate::dto::{IpcError, IpcResult, IssueDto};
use crate::host::{AppState, lock};

/// Most leftover objects named in one plan request.
pub const MAX_LEFTOVERS: usize = 500;
/// Most apps in one update plan (the CLI's `--app` limit).
pub const MAX_UPDATE_APPS: usize = 50;

fn ipc(code: ErrorCode, reason: &'static str, message: impl Into<String>) -> IpcError {
    IpcError::new(code, reason, message)
}

/// Facts kept by the host between calls: review notes of plans created here and the last old-downloads review.
#[derive(Default)]
pub struct SuiteCache {
    /// What the review page shows next to a plan created in this window (leftover estimate, routed apps...).
    pub plan_notes: HashMap<String, Value>,
    /// The last old-downloads review: its root and listed files by item id (see `system`).
    pub downloads: Option<crate::system::DownloadsCache>,
}

pub fn note_plan(s: &AppState, plan_id: &str, v: Value) {
    let mut c = lock(&s.suite);
    if c.plan_notes.len() > 200 {
        c.plan_notes.clear();
    }
    c.plan_notes.insert(plan_id.to_owned(), v);
}

/// A core refusal as an IPC error (the reason word the UI explains; the Settings route stays in the message).
pub fn refused(r: &cm_app::uninstall::Refused) -> IpcError {
    let reason = match r.code.as_str() {
        "policy_denied" => "policy_denied",
        "unsupported" => "unsupported",
        "permission_required" => "permission_required",
        "conflict" => "conflict",
        "io_error" => "io_error",
        "internal_error" => "internal",
        _ => "invalid_argument",
    };
    let mut message = r.message.clone();
    if let Some(u) = &r.settings_uri {
        message.push_str(&format!(" ({u})"));
    }
    IpcError::new(r.error_code(), reason, message)
}

// ---------------------------------------------------------------------------------------------
// Uninstall DTOs

#[derive(Debug, Clone, Serialize)]
pub struct UninstallAppDto {
    pub app_instance_id: String,
    pub name: String,
    pub version: Option<String>,
    pub publisher: Option<String>,
    /// `registry_uninstall`, `msi`, `msix_store`, `scoop`, `npm_global` or `unknown`.
    pub source: String,
    pub location: Option<String>,
    pub last_used_at: Option<String>,
    /// Security agents, system components, shared runtimes and CleanManager itself: no uninstall entry at all.
    pub protected_class: bool,
    /// `available`, `unverified` (no verifiable route: use Settings) or `unsupported` (protected).
    pub state: &'static str,
    pub adapter: Option<Value>,
    pub reason: Option<String>,
    pub settings_uri: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UninstallRunDto {
    pub operation_id: String,
    pub plan_id: String,
    pub app_name: String,
    /// `uninstall` (stage 1) or `leftovers` (stage 2).
    pub stage: &'static str,
    pub state: String,
    pub started_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UninstallListDto {
    pub supported: bool,
    pub is_admin: bool,
    pub items: Vec<UninstallAppDto>,
    pub issues: Vec<IssueDto>,
    /// Recent uninstall operations (stage 1 can be followed by a leftovers review).
    pub recent: Vec<UninstallRunDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UninstallPlanDto {
    pub plan_id: String,
    /// D7: what would probably remain, by group (an estimate shown in review, never part of the plan).
    pub leftover_estimate: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LeftoverDto {
    pub object_id: String,
    pub kind: Value,
    pub group: String,
    pub display: String,
    pub evidence: Vec<String>,
    /// `shared_owner`, `owner_unknown` or `app_reappeared`: kept, never planned.
    pub marker: Option<Value>,
    /// `default_selected`, `review_only`, `blocked` or `kept`.
    pub tier: String,
    /// Preselected by the core (caches and logs only).
    pub selected: bool,
    /// Can be put into the leftovers plan from this window.
    pub selectable: bool,
    /// Why not (`kept`, `blocked`, `manual`, `needs_admin`).
    pub why_not: Option<&'static str>,
    pub warnings: Vec<String>,
    pub logical_bytes: Option<String>,
    pub note: Option<String>,
    pub requires_admin: bool,
    /// `fs_trash`, `registry_backup_delete` (backed up first), `startup_toggle` or `manual`.
    pub action: &'static str,
    pub guidance: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LeftoverGroupDto {
    pub group: String,
    pub items: Vec<LeftoverDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LeftoversDto {
    pub operation_id: String,
    pub app: Value,
    pub is_admin: bool,
    pub groups: Vec<LeftoverGroupDto>,
    pub default_selected: usize,
    /// Home-folder configuration files that mention the install folder: a read-only hint, never planned.
    pub external_references: Vec<String>,
}

/// Group order on the page: what is preselected first, what needs most care last.
pub const GROUP_ORDER: [&str; 6] = ["caches_logs", "bundle", "startup", "settings", "personal_data", "system"];

fn enum_word<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|x| x.as_str().map(str::to_owned)).unwrap_or_default()
}

/// One leftover as the page shows it: selectable only when the core would plan it from here.
pub fn leftover_dto(l: &cm_core::uninstall::Leftover, is_admin: bool) -> LeftoverDto {
    use cm_core::uninstall::Binding;
    let (action, guidance) = match &l.binding {
        Binding::Path { .. } => ("fs_trash", None),
        Binding::Registry { .. } => ("registry_backup_delete", None),
        Binding::Startup { .. } => ("startup_toggle", None),
        Binding::Manual { guidance } => ("manual", Some(guidance.clone())),
    };
    let why_not = if l.marker.is_some() || l.tier == "kept" {
        Some("kept")
    } else if l.tier == "blocked" {
        Some("blocked")
    } else if action == "manual" {
        Some("manual")
    } else if l.requires_admin && !is_admin {
        Some("needs_admin")
    } else {
        None
    };
    LeftoverDto {
        object_id: l.object_id.clone(),
        kind: serde_json::to_value(l.kind).unwrap_or(Value::Null),
        group: enum_word(&l.group),
        display: l.display.clone(),
        evidence: l.evidence.clone(),
        marker: l.marker.map(|m| serde_json::to_value(m).unwrap_or(Value::Null)),
        tier: l.tier.clone(),
        selected: l.selected && why_not.is_none(),
        selectable: why_not.is_none(),
        why_not,
        warnings: l.warnings.clone(),
        logical_bytes: l.logical_bytes.map(|b| b.to_string()),
        note: l.note.clone(),
        requires_admin: l.requires_admin,
        action,
        guidance,
    }
}

pub fn grouped(items: &[cm_core::uninstall::Leftover], is_admin: bool) -> Vec<LeftoverGroupDto> {
    let mut groups: BTreeMap<String, Vec<LeftoverDto>> = BTreeMap::new();
    for l in items {
        let d = leftover_dto(l, is_admin);
        groups.entry(d.group.clone()).or_default().push(d);
    }
    let rank = |g: &str| GROUP_ORDER.iter().position(|x| *x == g).unwrap_or(GROUP_ORDER.len());
    let mut out: Vec<LeftoverGroupDto> = groups.into_iter().map(|(group, items)| LeftoverGroupDto { group, items }).collect();
    out.sort_by_key(|g| rank(&g.group));
    out
}

/// Split a final selection into what the core adds by name (`extra`) and preselected objects the person cleared.
pub fn split_selection(all: &[cm_core::uninstall::Leftover], chosen: &[String]) -> (Vec<String>, Vec<String>) {
    let pre: Vec<&str> = all.iter().filter(|l| l.selected).map(|l| l.object_id.as_str()).collect();
    let extra = chosen.iter().filter(|c| !pre.contains(&c.as_str())).cloned().collect();
    let dropped = pre.iter().filter(|p| !chosen.iter().any(|c| c == *p)).map(|p| (*p).to_owned()).collect();
    (extra, dropped)
}

/// The app name of a stored uninstall plan (stage 1), for recent runs.
fn plan_app(plan: &PlanV2) -> Option<String> {
    plan.meta.iter().find_map(|m| match &m.uninstall {
        Some(cm_core::uninstall::UninstallMeta::App { identity, .. }) => Some(identity.name.clone()),
        _ => None,
    })
}

/// Recent uninstall operations, newest first (at most `limit`).
pub fn recent_runs(stores: &cm_store::Stores, notes: &HashMap<String, Value>, limit: usize) -> Vec<UninstallRunDto> {
    let Ok((rows, _)) = stores.audit.operations_page(200, None) else { return vec![] };
    let mut out = Vec::new();
    for r in rows {
        if out.len() >= limit {
            break;
        }
        if stores.audit.plan_contract(&r.plan_id).ok().as_deref() != Some(cm_core::plan::v2::PLAN_V2_CONTRACT) {
            continue;
        }
        let Ok(plan) = stores.audit.load_plan_v2(&r.plan_id) else { continue };
        if plan.body.plan_kind != PlanKind::Uninstall {
            continue;
        }
        let stage1 = crate::v2::runs_uninstaller(&plan);
        let name = plan_app(&plan).or_else(|| notes.get(&r.plan_id).and_then(|n| n["app_name"].as_str().map(str::to_owned))).unwrap_or_else(|| "—".into());
        out.push(UninstallRunDto {
            operation_id: r.operation_id,
            plan_id: r.plan_id,
            app_name: name,
            stage: if stage1 { "uninstall" } else { "leftovers" },
            state: r.state,
            started_at: r.started_at,
            finished_at: r.finished_at,
        });
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Windows

#[cfg(windows)]
mod win {
    use std::sync::atomic::{AtomicBool, Ordering};

    use cm_core::host::HostProbe;
    use cm_platform::windows::uninstall::{UninstallEnv, adapter_env, records};

    use super::*;
    use crate::cleanup::{new_id, now_unix};

    fn env() -> IpcResult<UninstallEnv> {
        UninstallEnv::from_process().map_err(|e| ipc(ErrorCode::InvalidArgument, "invalid_argument", e))
    }

    fn stores(s: &AppState) -> IpcResult<cm_store::Stores> {
        crate::suite::own_stores(s)
    }

    pub fn list(s: &AppState) -> IpcResult<UninstallListDto> {
        let env = env()?;
        let (recs, issues) = records(&env);
        let items = recs
            .iter()
            .map(|r| {
                let route = cm_core::uninstall::select_adapter(r, &adapter_env(&env, r), &|x: &str| std::path::Path::new(x).is_file());
                let (state, adapter, reason, settings) = match &route {
                    Ok(c) => ("available", Some(json!({"adapter_id": c.kind.adapter_id(), "display": c.display, "package_manager": c.kind.package_manager()})), None, None),
                    Err(x) if x.code == "policy_denied" => ("unsupported", None, Some(x.message.clone()), x.settings_uri.map(str::to_owned)),
                    Err(x) => ("unverified", None, Some(x.message.clone()), x.settings_uri.map(str::to_owned)),
                };
                // UNI-07: AI CLIs report when they were last used.
                let last_used = r.ai_tool.as_deref().and_then(|t| cm_platform::windows::uninstall::ai_last_used(&env, t)).map(cm_core::time::utc_from_unix);
                UninstallAppDto {
                    app_instance_id: r.instance_id.clone(),
                    name: r.name.clone(),
                    version: r.version.clone(),
                    publisher: r.publisher.clone(),
                    source: r.wire_source().into(),
                    location: r.install_location.clone(),
                    last_used_at: last_used,
                    protected_class: r.protected_class.is_some() || state == "unsupported",
                    state,
                    adapter,
                    reason,
                    settings_uri: settings.or_else(|| (state != "available").then(|| cm_core::uninstall::SETTINGS_URI.into())),
                }
            })
            .collect();
        let st = stores(s)?;
        let recent = recent_runs(&st, &lock(&s.suite).plan_notes, 6);
        Ok(UninstallListDto {
            supported: true,
            is_admin: env.is_admin,
            items,
            issues: issues.iter().map(|i| IssueDto { code: i.code.clone(), scope_display: i.source.clone(), message: i.message.clone() }).collect(),
            recent,
        })
    }

    /// Stage 1 (UNI-01/02/03/10): never the variant that may end processes.
    pub fn plan(s: &AppState, instance: &str) -> IpcResult<UninstallPlanDto> {
        let env = env()?;
        let st = stores(s)?;
        let plan_id = new_id("up");
        let pdigest = crate::cleanup::policy_digest(&st);
        let s1 = cm_app::uninstall::create_uninstall_plan(&st, &env, instance, false, &plan_id, now_unix(), &pdigest).map_err(|r| refused(&r))?;
        let estimate: BTreeMap<String, usize> = s1.estimate.iter().map(|(g, n)| (enum_word(g), *n)).collect();
        note_plan(s, &plan_id, json!({"app_name": s1.record.name, "leftover_estimate": estimate}));
        Ok(UninstallPlanDto { plan_id, leftover_estimate: estimate })
    }

    pub fn leftovers(s: &AppState, operation_id: &str) -> IpcResult<LeftoversDto> {
        let env = env()?;
        let st = stores(s)?;
        let (identity, items, refs) = cm_app::uninstall::leftovers(&st, &env, operation_id).map_err(|r| refused(&r))?;
        let groups = grouped(&items, env.is_admin);
        let default_selected = groups.iter().flat_map(|g| &g.items).filter(|i| i.selected).count();
        Ok(LeftoversDto {
            operation_id: operation_id.into(),
            app: json!({"app_instance_id": identity.instance_id, "name": identity.name, "publisher": identity.publisher, "source": identity.source}),
            is_admin: env.is_admin,
            groups,
            default_selected,
            external_references: refs,
        })
    }

    static LEFTOVERS_BUSY: AtomicBool = AtomicBool::new(false);

    /// Stage 2: the final selection (preselected caches/logs the person kept plus objects they added).
    pub fn leftovers_plan(s: &AppState, operation_id: &str, chosen: &[String]) -> IpcResult<CreatedPlanDto> {
        if LEFTOVERS_BUSY.swap(true, Ordering::AcqRel) {
            return Err(ipc(ErrorCode::Conflict, "scan_running", "Another leftovers plan is still being prepared"));
        }
        struct Release;
        impl Drop for Release {
            fn drop(&mut self) {
                LEFTOVERS_BUSY.store(false, Ordering::Release);
            }
        }
        let _release = Release;
        let env = env()?;
        let st = stores(s)?;
        let (identity, all, _) = cm_app::uninstall::leftovers(&st, &env, operation_id).map_err(|r| refused(&r))?;
        let (extra, cleared) = split_selection(&all, chosen);
        let plan_id = new_id("lp");
        let pdigest = crate::cleanup::policy_digest(&st);
        let host = cm_platform::host::OsHostProbe.current("desktop");
        let now = now_unix();
        let (plan, _) = cm_app::uninstall::create_leftovers_plan(&st, &env, operation_id, &extra, &cleared, &host, &plan_id, now, &pdigest).map_err(|r| refused(&r))?;
        note_plan(s, &plan.body.plan_id, json!({"app_name": identity.name, "operation_id": operation_id}));
        Ok(CreatedPlanDto::new(plan.body.plan_id.clone()))
    }

    pub fn restore_plan(s: &AppState, operation_id: &str) -> IpcResult<CreatedPlanDto> {
        let st = stores(s)?;
        let plan_id = new_id("rp");
        let pdigest = crate::cleanup::policy_digest(&st);
        let plan = cm_app::uninstall::create_restore_plan(&st, operation_id, &plan_id, now_unix(), &pdigest).map_err(|r| refused(&r))?;
        Ok(CreatedPlanDto::new(plan.body.plan_id))
    }

    // -----------------------------------------------------------------------------------------
    // Updates

    static CHECK_BUSY: AtomicBool = AtomicBool::new(false);

    pub fn updates(s: &AppState, check: bool) -> IpcResult<Value> {
        let dir = s.store_dir()?.to_path_buf();
        let apps = cm_app::updates::windows_apps();
        let report = if check {
            if CHECK_BUSY.swap(true, Ordering::AcqRel) {
                return Err(ipc(ErrorCode::Conflict, "scan_running", "An update check is still running"));
            }
            let ch = cm_app::updates::WindowsChannels { winget: cm_platform::windows::updates::winget_exe(&apps) };
            let r = cm_app::updates::check(&dir, &apps, &ch, &cm_core::time::now_utc());
            CHECK_BUSY.store(false, Ordering::Release);
            r.map_err(|e| ipc(ErrorCode::IoError, "io_error", format!("cannot save update results: {e}")))?
        } else {
            cm_app::updates::cached(&dir, &apps)
        };
        let mut w = report.to_wire();
        w["supported"] = Value::Bool(true);
        Ok(w)
    }

    pub fn update_plan(s: &AppState, ids: &[String]) -> IpcResult<Value> {
        let st = stores(s)?;
        let cache = cm_app::updates::load(&st.dir);
        if cache.results.is_empty() {
            return Err(ipc(ErrorCode::InvalidArgument, "not_checked", "No update check results yet; check for updates first"));
        }
        let apps = cm_app::updates::windows_apps();
        let (winget, scoop) = cm_app::updates::windows_tools(&apps);
        let inputs = cm_app::updates::PlanInputs { results: &cache.results, apps: &apps, ids, winget, scoop, installed_signer: &cm_platform::windows::updates::installed_signer };
        let plan_id = new_id("ud");
        let pdigest = crate::cleanup::policy_digest(&st);
        let (plan, routed) = match cm_app::updates::plan_updates(&inputs, &plan_id, now_unix(), &pdigest) {
            Ok(r) => r,
            Err(e @ cm_app::updates::UpdatePlanError::UnknownApp(_)) => return Err(ipc(ErrorCode::InvalidArgument, "unknown_app", e.to_string())),
            Err(e) => return Err(ipc(ErrorCode::InternalError, "internal", e.to_string())),
        };
        if let Some(pl) = &plan {
            st.audit.save_plan_v2(pl)?;
            note_plan(s, &pl.body.plan_id, json!({"routed": routed}));
        }
        Ok(json!({"plan_id": plan.as_ref().map(|p| p.body.plan_id.clone()), "planned": plan.as_ref().map_or(0, |p| p.body.items.len()), "routed": routed}))
    }

    /// Open one of the fixed system pages with File Explorer (no shell, no other target).
    pub fn open(target: &str) -> IpcResult<bool> {
        let uri = match target {
            "apps_features" => cm_core::uninstall::SETTINGS_URI,
            "store_updates" => cm_core::updates::STORE_UPDATES_URI,
            other => return Err(ipc(ErrorCode::InvalidArgument, "invalid_argument", format!("unknown system page {other:?}"))),
        };
        let windir = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Windows".into());
        Ok(std::process::Command::new(windir.join("explorer.exe")).arg(uri).spawn().is_ok())
    }
}

#[cfg(not(windows))]
fn unsupported(what: &str) -> IpcError {
    ipc(ErrorCode::Unsupported, "unsupported", format!("{what} is implemented on Windows only in this build"))
}

// ---------------------------------------------------------------------------------------------
// IPC

#[tauri::command(rename_all = "snake_case")]
pub async fn uninstall_list(state: St<'_>) -> IpcResult<UninstallListDto> {
    let s = state.inner().clone();
    blocking(move || {
        #[cfg(windows)]
        return win::list(&s);
        #[cfg(not(windows))]
        {
            let _ = s;
            Ok(UninstallListDto { supported: false, is_admin: false, items: vec![], issues: vec![], recent: vec![] })
        }
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn uninstall_plan(state: St<'_>, app_instance_id: String) -> IpcResult<UninstallPlanDto> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::record_id("app instance", &app_instance_id)?;
        #[cfg(windows)]
        return win::plan(&s, &id);
        #[cfg(not(windows))]
        {
            let _ = (s, id);
            Err(unsupported("application uninstall"))
        }
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn uninstall_leftovers(state: St<'_>, operation_id: String) -> IpcResult<LeftoversDto> {
    let s = state.inner().clone();
    blocking(move || {
        let op = args::record_id("operation", &operation_id)?;
        #[cfg(windows)]
        return win::leftovers(&s, &op);
        #[cfg(not(windows))]
        {
            let _ = (s, op);
            Err(unsupported("application uninstall"))
        }
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn uninstall_leftovers_plan(state: St<'_>, operation_id: String, object_ids: Vec<String>) -> IpcResult<CreatedPlanDto> {
    let s = state.inner().clone();
    blocking(move || {
        let op = args::record_id("operation", &operation_id)?;
        let ids = args::record_ids("leftover", &object_ids, MAX_LEFTOVERS)?;
        #[cfg(windows)]
        return win::leftovers_plan(&s, &op, &ids);
        #[cfg(not(windows))]
        {
            let _ = (s, op, ids);
            Err(unsupported("application uninstall"))
        }
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn uninstall_restore_plan(state: St<'_>, operation_id: String) -> IpcResult<CreatedPlanDto> {
    let s = state.inner().clone();
    blocking(move || {
        let op = args::record_id("operation", &operation_id)?;
        #[cfg(windows)]
        return win::restore_plan(&s, &op);
        #[cfg(not(windows))]
        {
            let _ = (s, op);
            Err(unsupported("application uninstall"))
        }
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn updates_list(state: St<'_>, check: bool) -> IpcResult<Value> {
    let s = state.inner().clone();
    blocking(move || {
        #[cfg(windows)]
        return win::updates(&s, check);
        #[cfg(not(windows))]
        {
            let _ = (s, check);
            Ok(json!({"supported": false, "checks_enabled": false, "checked": false, "checked_at": null, "counts": {}, "items": []}))
        }
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn updates_set_checks(state: St<'_>, enabled: bool) -> IpcResult<Value> {
    let s = state.inner().clone();
    blocking(move || {
        let dir = s.store_dir()?;
        let settings = cm_app::updates::set_checks(dir, enabled).map_err(|e| ipc(ErrorCode::IoError, "io_error", format!("cannot save the setting: {e}")))?;
        Ok(json!({"checks_enabled": settings.checks_enabled, "last_checked_at": cm_app::updates::load(dir).last_checked_at}))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn updates_plan(state: St<'_>, app_instance_ids: Vec<String>) -> IpcResult<Value> {
    let s = state.inner().clone();
    blocking(move || {
        let ids = args::record_ids("app instance", &app_instance_ids, MAX_UPDATE_APPS)?;
        #[cfg(windows)]
        return win::update_plan(&s, &ids);
        #[cfg(not(windows))]
        {
            let _ = (s, ids);
            Err(unsupported("app updates"))
        }
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn open_system_page(target: String) -> IpcResult<bool> {
    blocking(move || {
        #[cfg(windows)]
        return win::open(&target);
        #[cfg(not(windows))]
        {
            let _ = target;
            Err(unsupported("opening system pages"))
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use cm_core::uninstall::{Binding, Group, Leftover, LeftoverKind, Marker};

    fn lo(id: &str, group: Group, tier: &str, selected: bool, binding: Binding) -> Leftover {
        Leftover {
            object_id: id.into(), kind: LeftoverKind::Directory, group, display: format!("C:\\Users\\Lin\\AppData\\Local\\Tools\\{id}"), evidence: vec!["exact name".into()],
            marker: None, tier: tier.into(), selected, warnings: vec![], logical_bytes: Some(10), note: None, requires_admin: false, binding,
        }
    }

    fn path() -> Binding {
        Binding::Path { path: "C:\\x".into() }
    }

    #[test]
    fn leftovers_are_grouped_and_only_plannable_objects_are_selectable() {
        let mut kept = lo("lo_kept", Group::Settings, "kept", false, path());
        kept.marker = Some(Marker::SharedOwner);
        let mut admin = lo("lo_admin", Group::System, "review_only", false, path());
        admin.requires_admin = true;
        let items = vec![
            lo("lo_cache", Group::CachesLogs, "default_selected", true, path()),
            lo("lo_models", Group::PersonalData, "review_only", false, path()),
            kept,
            lo("lo_blocked", Group::PersonalData, "blocked", false, path()),
            lo("lo_svc", Group::System, "review_only", false, Binding::Manual { guidance: "stop the service yourself".into() }),
            admin,
        ];
        let groups = grouped(&items, false);
        assert_eq!(groups.iter().map(|g| g.group.as_str()).collect::<Vec<_>>(), ["caches_logs", "settings", "personal_data", "system"]);
        let by = |id: &str| groups.iter().flat_map(|g| &g.items).find(|i| i.object_id == id).unwrap().clone();
        assert!(by("lo_cache").selected && by("lo_cache").selectable);
        assert!(!by("lo_models").selected && by("lo_models").selectable, "personal data is never preselected");
        assert_eq!((by("lo_kept").selectable, by("lo_kept").why_not), (false, Some("kept")));
        assert_eq!(by("lo_blocked").why_not, Some("blocked"));
        assert_eq!((by("lo_svc").why_not, by("lo_svc").action), (Some("manual"), "manual"));
        assert_eq!(by("lo_admin").why_not, Some("needs_admin"));
        assert_eq!(grouped(&items, true).iter().flat_map(|g| &g.items).find(|i| i.object_id == "lo_admin").unwrap().why_not, None);
    }

    #[test]
    fn a_final_selection_names_additions_and_cleared_preselections() {
        let all = vec![
            lo("lo_a", Group::CachesLogs, "default_selected", true, path()),
            lo("lo_b", Group::CachesLogs, "default_selected", true, path()),
            lo("lo_c", Group::Settings, "review_only", false, path()),
        ];
        let (extra, cleared) = split_selection(&all, &["lo_a".into(), "lo_c".into()]);
        assert_eq!(extra, ["lo_c"]);
        assert_eq!(cleared, ["lo_b"]);
        let (extra, cleared) = split_selection(&all, &["lo_a".into(), "lo_b".into()]);
        assert!(extra.is_empty() && cleared.is_empty());
    }
}
