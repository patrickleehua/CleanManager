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
    /// The CLI's refusal code for this row (`permission_required`, `policy_denied`, `unsupported`...); `None` when the
    /// app can be uninstalled from here.
    pub refusal_code: Option<String>,
    pub settings_uri: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UninstallRunDto {
    pub operation_id: String,
    pub plan_id: String,
    /// The application, or every application of a batch (UNI-11) joined for display.
    pub app_name: String,
    /// Applications in the plan (stage 1; 0 for a leftovers plan).
    pub app_count: usize,
    /// `uninstall` (stage 1) or `leftovers` (stage 2).
    pub stage: &'static str,
    pub state: String,
    /// Stage 1 finished and at least one application's uninstaller succeeded: its leftovers can be reviewed.
    pub leftovers_reviewable: bool,
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
    /// D7: what would probably remain, by group (an estimate shown in review, never part of the plan), over the batch.
    pub leftover_estimate: BTreeMap<String, usize>,
    /// UNI-11: the applications in the plan, in plan order.
    pub apps: Vec<BatchAppDto>,
    /// Applications refused exactly as on their own (protected class, unverifiable route...); not in the plan.
    pub refused: Vec<BatchRefusalDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BatchAppDto {
    pub app_instance_id: String,
    pub name: String,
    pub leftover_estimate: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BatchRefusalDto {
    pub app_instance_id: String,
    pub name: Option<String>,
    /// The reason word of [`refused`] (`policy_denied`, `unsupported`...).
    pub reason: String,
    pub message: String,
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
    /// The application of a single uninstall; `null` for a batch (see `apps`).
    pub app: Value,
    pub is_admin: bool,
    /// Every reviewable object of the operation (all applications).
    pub groups: Vec<LeftoverGroupDto>,
    pub default_selected: usize,
    /// Home-folder configuration files that mention the install folder: a read-only hint, never planned.
    pub external_references: Vec<String>,
    /// UNI-11: the same review per application, in plan order. An application whose uninstaller failed, was skipped
    /// or did not run lists no objects: it is still installed and keeps everything.
    pub apps: Vec<LeftoverAppDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LeftoverAppDto {
    pub app: Value,
    /// `completed`, `partial`, `failed`, `skipped`, `not_started`, `unknown` (see `cm_app::uninstall::AppOutcome`).
    pub status: &'static str,
    pub code: Option<String>,
    pub message: Option<String>,
    pub leftovers_reviewable: bool,
    pub groups: Vec<LeftoverGroupDto>,
    pub default_selected: usize,
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

/// The IPC shape of a stage-1 batch: per application its estimate, the batch total, and the refused applications.
#[allow(clippy::type_complexity)]
pub fn batch_dto(plan_id: &str, apps: &[(&str, &str, &BTreeMap<cm_core::uninstall::Group, usize>)], refusals: &[(&str, Option<&str>, &cm_app::uninstall::Refused)]) -> UninstallPlanDto {
    let mut total: BTreeMap<String, usize> = BTreeMap::new();
    let apps = apps
        .iter()
        .map(|(id, name, estimate)| {
            let est: BTreeMap<String, usize> = estimate.iter().map(|(g, n)| (enum_word(g), *n)).collect();
            for (g, n) in &est {
                *total.entry(g.clone()).or_insert(0) += n;
            }
            BatchAppDto { app_instance_id: (*id).to_owned(), name: (*name).to_owned(), leftover_estimate: est }
        })
        .collect();
    let refused = refusals
        .iter()
        .map(|(id, name, r)| {
            let e = refused(r);
            BatchRefusalDto { app_instance_id: (*id).to_owned(), name: name.map(str::to_owned), reason: e.reason.to_string(), message: e.message }
        })
        .collect();
    UninstallPlanDto { plan_id: plan_id.to_owned(), leftover_estimate: total, apps, refused }
}

/// Split a final selection into what the core adds by name (`extra`) and preselected objects the person cleared.
pub fn split_selection(all: &[cm_core::uninstall::Leftover], chosen: &[String]) -> (Vec<String>, Vec<String>) {
    let pre: Vec<&str> = all.iter().filter(|l| l.selected).map(|l| l.object_id.as_str()).collect();
    let extra = chosen.iter().filter(|c| !pre.contains(&c.as_str())).cloned().collect();
    let dropped = pre.iter().filter(|p| !chosen.iter().any(|c| c == *p)).map(|p| (*p).to_owned()).collect();
    (extra, dropped)
}

/// The app name of a stored uninstall plan (stage 1), for recent runs; the applications of a batch, joined.
fn plan_app(plan: &PlanV2) -> Option<String> {
    let names: Vec<String> = cm_app::uninstall::planned_apps(plan).into_iter().map(|a| a.identity.name).collect();
    (!names.is_empty()).then(|| names_text(&names))
}

/// Display names of several applications: all of them up to three, then the first two and a count.
pub fn names_text(names: &[String]) -> String {
    match names.len() {
        0..=3 => names.join("、"),
        n => format!("{} 等 {n} 个应用", names[..2].join("、")),
    }
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
        // UNI-11: leftovers exist only for applications whose own uninstaller succeeded.
        let report = stores.audit.operation_report(&r.operation_id).ok().flatten().and_then(|j| serde_json::from_str::<cm_app::execute::OperationReport>(&j).ok());
        let outcomes = cm_app::uninstall::app_outcomes(&plan, report.as_ref());
        out.push(UninstallRunDto {
            operation_id: r.operation_id,
            plan_id: r.plan_id,
            app_name: name,
            app_count: outcomes.len(),
            stage: if stage1 { "uninstall" } else { "leftovers" },
            state: r.state,
            leftovers_reviewable: stage1 && outcomes.iter().any(|o| o.leftovers_reviewable),
            started_at: r.started_at,
            finished_at: r.finished_at,
        });
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Platform work (the uninstall module of `crate::plat::os`: the same API on Windows and macOS)

mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};

    use cm_core::host::HostProbe;

    use super::*;
    use crate::cleanup::{new_id, now_unix};
    use crate::plat::os::uninstall::{UninstallEnv, ai_last_used, records, route, route_capability};

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
                // The same decision `apps uninstall list` reports (UNI-01): an adapter, or why there is none.
                let route = route(&env, r);
                let state = match route_capability(&route).state {
                    cm_core::capability::CapabilityState::Available => "available",
                    cm_core::capability::CapabilityState::Unsupported => "unsupported",
                    _ => "unverified",
                };
                let (adapter, reason, refusal_code, settings) = match &route {
                    Ok(c) => (Some(json!({"adapter_id": c.kind.adapter_id(), "display": c.display, "package_manager": c.kind.package_manager()})), None, None, None),
                    Err(x) => (None, Some(x.message.clone()), Some(x.code.to_string()), x.settings_uri.map(str::to_owned)),
                };
                // UNI-07: AI CLIs report when they were last used.
                let last_used = r.ai_tool.as_deref().and_then(|t| ai_last_used(&env, t)).map(cm_core::time::utc_from_unix);
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
                    refusal_code,
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

    /// Stage 1 (UNI-01/02/03/10/11) for one or several applications in one plan: never the variant that may end
    /// processes. Refused applications are reported and left out; the request fails only when all are refused.
    pub fn plan(s: &AppState, instances: &[String]) -> IpcResult<UninstallPlanDto> {
        let env = env()?;
        let st = stores(s)?;
        let plan_id = new_id("up");
        let pdigest = crate::cleanup::policy_digest(&st);
        let b = cm_app::uninstall::create_uninstall_batch(&st, &env, instances, false, &plan_id, now_unix(), &pdigest).map_err(|r| refused(&r))?;
        let apps: Vec<_> = b.apps.iter().map(|a| (a.record.instance_id.as_str(), a.record.name.as_str(), &a.estimate)).collect();
        let refusals: Vec<_> = b.refused.iter().map(|x| (x.app_instance_id.as_str(), x.name.as_deref(), &x.refused)).collect();
        let dto = batch_dto(&b.plan.body.plan_id, &apps, &refusals);
        let names: Vec<String> = dto.apps.iter().map(|a| a.name.clone()).collect();
        note_plan(s, &plan_id, json!({"app_name": names_text(&names), "leftover_estimate": dto.leftover_estimate, "apps": dto.apps, "refused": dto.refused}));
        Ok(dto)
    }

    fn app_json(i: &cm_core::uninstall::AppIdentity) -> Value {
        json!({"app_instance_id": i.instance_id, "name": i.name, "publisher": i.publisher, "source": i.source})
    }

    pub fn leftovers(s: &AppState, operation_id: &str) -> IpcResult<LeftoversDto> {
        let env = env()?;
        let st = stores(s)?;
        let by_app = cm_app::uninstall::leftovers_by_app(&st, &env, operation_id).map_err(|r| refused(&r))?;
        // A failed, skipped or stale uninstall keeps everything of that application (UNI-11).
        if let Some(r) = cm_app::uninstall::none_reviewable(operation_id, &by_app) {
            return Err(refused(&r));
        }
        let all: Vec<cm_core::uninstall::Leftover> = by_app.iter().flat_map(|a| a.items.iter().cloned()).collect();
        let groups = grouped(&all, env.is_admin);
        let default_selected = groups.iter().flat_map(|g| &g.items).filter(|i| i.selected).count();
        let mut refs: Vec<String> = Vec::new();
        for r in by_app.iter().flat_map(|a| &a.external_references) {
            if !refs.contains(r) {
                refs.push(r.clone());
            }
        }
        let apps: Vec<LeftoverAppDto> = by_app
            .iter()
            .map(|a| {
                let groups = grouped(&a.items, env.is_admin);
                LeftoverAppDto {
                    app: app_json(&a.identity),
                    status: a.outcome.status,
                    code: a.outcome.code.clone(),
                    message: a.outcome.message.clone(),
                    leftovers_reviewable: a.outcome.leftovers_reviewable,
                    default_selected: groups.iter().flat_map(|g| &g.items).filter(|i| i.selected).count(),
                    groups,
                    external_references: a.external_references.clone(),
                }
            })
            .collect();
        Ok(LeftoversDto {
            operation_id: operation_id.into(),
            app: if by_app.len() == 1 { app_json(&by_app[0].identity) } else { Value::Null },
            is_admin: env.is_admin,
            groups,
            default_selected,
            external_references: refs,
            apps,
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
        let by_app = cm_app::uninstall::leftovers_by_app(&st, &env, operation_id).map_err(|r| refused(&r))?;
        let names: Vec<String> = by_app.iter().filter(|a| a.outcome.leftovers_reviewable).map(|a| a.identity.name.clone()).collect();
        let all: Vec<cm_core::uninstall::Leftover> = by_app.into_iter().flat_map(|a| a.items).collect();
        let (extra, cleared) = split_selection(&all, chosen);
        let plan_id = new_id("lp");
        let pdigest = crate::cleanup::policy_digest(&st);
        let host = cm_platform::host::OsHostProbe.current("desktop");
        let now = now_unix();
        let (plan, _) = cm_app::uninstall::create_leftovers_plan(&st, &env, operation_id, &extra, &cleared, &host, &plan_id, now, &pdigest).map_err(|r| refused(&r))?;
        note_plan(s, &plan.body.plan_id, json!({"app_name": names_text(&names), "operation_id": operation_id}));
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

    /// The update inventory of this machine (Windows: installed apps plus Scoop).
    #[cfg(windows)]
    fn update_apps() -> IpcResult<Vec<cm_core::apps::AppInstance>> {
        Ok(cm_app::updates::windows_apps())
    }

    /// The inventory locations (`CM_UNINSTALL_TEST_ROOT`/`HOME` redirect them in tests).
    #[cfg(target_os = "macos")]
    fn mac_env() -> IpcResult<cm_platform::macos::apps::AppsEnv> {
        cm_platform::macos::apps::AppsEnv::from_process().map_err(|e| ipc(ErrorCode::InvalidArgument, "invalid_argument", e))
    }

    /// The update inventory of this Mac (bundles plus Homebrew formulae).
    #[cfg(target_os = "macos")]
    fn update_apps() -> IpcResult<Vec<cm_core::apps::AppInstance>> {
        Ok(cm_app::updates::mac_apps(&mac_env()?))
    }

    /// Run the package managers' own queries (`apps updates`): winget on Windows.
    #[cfg(windows)]
    fn run_check(dir: &std::path::Path, apps: &[cm_core::apps::AppInstance]) -> IpcResult<std::io::Result<cm_app::updates::CheckReport>> {
        let ch = cm_app::updates::WindowsChannels { winget: cm_platform::windows::updates::winget_exe(apps) };
        Ok(cm_app::updates::check(dir, apps, &ch, &cm_core::time::now_utc()))
    }

    /// Run the package managers' own queries (`apps updates`): Homebrew and the App Store on macOS.
    #[cfg(target_os = "macos")]
    fn run_check(dir: &std::path::Path, apps: &[cm_core::apps::AppInstance]) -> IpcResult<std::io::Result<cm_app::updates::CheckReport>> {
        let ch = cm_app::updates::MacChannels::new(mac_env()?);
        Ok(cm_app::updates::check(dir, apps, &ch, &cm_core::time::now_utc()))
    }

    pub fn updates(s: &AppState, check: bool) -> IpcResult<Value> {
        let dir = s.store_dir()?.to_path_buf();
        let apps = update_apps()?;
        let report = if check {
            if CHECK_BUSY.swap(true, Ordering::AcqRel) {
                return Err(ipc(ErrorCode::Conflict, "scan_running", "An update check is still running"));
            }
            let r = run_check(&dir, &apps);
            CHECK_BUSY.store(false, Ordering::Release);
            r?.map_err(|e| ipc(ErrorCode::IoError, "io_error", format!("cannot save update results: {e}")))?
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
        #[cfg(windows)]
        let (apps, winget, scoop, homebrew, signer) = {
            let apps = cm_app::updates::windows_apps();
            let (winget, scoop) = cm_app::updates::windows_tools(&apps);
            (apps, winget, scoop, None, cm_platform::windows::updates::installed_signer)
        };
        #[cfg(target_os = "macos")]
        let (apps, winget, scoop, homebrew, signer) = {
            let env = mac_env()?;
            (cm_app::updates::mac_apps(&env), None, None, cm_app::updates::mac_brew_tool(&env), cm_platform::macos::updates::installed_signer)
        };
        let inputs = cm_app::updates::PlanInputs { results: &cache.results, apps: &apps, ids, winget, scoop, homebrew, installed_signer: &signer };
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

    /// Open one of the fixed system pages (`crate::plat::system_page`; no shell, no other target).
    pub fn open(target: &str) -> IpcResult<bool> {
        let Some(uri) = crate::plat::system_page(target) else {
            return Err(ipc(ErrorCode::InvalidArgument, "invalid_argument", format!("unknown system page {target:?}")));
        };
        Ok(crate::plat::open_uri(uri))
    }
}

// ---------------------------------------------------------------------------------------------
// IPC

#[tauri::command(rename_all = "snake_case")]
pub async fn uninstall_list(state: St<'_>) -> IpcResult<UninstallListDto> {
    let s = state.inner().clone();
    blocking(move || imp::list(&s)).await
}

/// UNI-11: one plan for the selected applications (one or several).
#[tauri::command(rename_all = "snake_case")]
pub async fn uninstall_plan(state: St<'_>, app_instance_ids: Vec<String>) -> IpcResult<UninstallPlanDto> {
    let s = state.inner().clone();
    blocking(move || {
        let ids = args::record_ids("app instance", &app_instance_ids, cm_app::uninstall::MAX_BATCH)?;
        imp::plan(&s, &ids)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn uninstall_leftovers(state: St<'_>, operation_id: String) -> IpcResult<LeftoversDto> {
    let s = state.inner().clone();
    blocking(move || {
        let op = args::record_id("operation", &operation_id)?;
        imp::leftovers(&s, &op)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn uninstall_leftovers_plan(state: St<'_>, operation_id: String, object_ids: Vec<String>) -> IpcResult<CreatedPlanDto> {
    let s = state.inner().clone();
    blocking(move || {
        let op = args::record_id("operation", &operation_id)?;
        let ids = args::record_ids("leftover", &object_ids, MAX_LEFTOVERS)?;
        imp::leftovers_plan(&s, &op, &ids)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn uninstall_restore_plan(state: St<'_>, operation_id: String) -> IpcResult<CreatedPlanDto> {
    let s = state.inner().clone();
    blocking(move || {
        let op = args::record_id("operation", &operation_id)?;
        imp::restore_plan(&s, &op)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn updates_list(state: St<'_>, check: bool) -> IpcResult<Value> {
    let s = state.inner().clone();
    blocking(move || imp::updates(&s, check)).await
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
        imp::update_plan(&s, &ids)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn open_system_page(target: String) -> IpcResult<bool> {
    blocking(move || imp::open(&target)).await
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
    fn a_batch_reports_each_application_and_each_refusal() {
        // UNI-11: estimates per application and summed; refused applications keep their reason word.
        let foo: BTreeMap<Group, usize> = [(Group::CachesLogs, 2), (Group::Settings, 1)].into_iter().collect();
        let bar: BTreeMap<Group, usize> = [(Group::CachesLogs, 1)].into_iter().collect();
        let agent = cm_app::uninstall::Refused { code: "policy_denied".into(), message: "endpoint security agent".into(), settings_uri: None };
        let dto = batch_dto("up_1", &[("app_foo", "Foo", &foo), ("app_bar", "Bar", &bar)], &[("app_agent", Some("Agent"), &agent)]);
        assert_eq!(dto.plan_id, "up_1");
        assert_eq!(dto.apps.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Foo", "Bar"]);
        assert_eq!((dto.leftover_estimate["caches_logs"], dto.leftover_estimate["settings"]), (3, 1));
        assert_eq!((dto.refused[0].reason.as_str(), dto.refused[0].name.as_deref()), ("policy_denied", Some("Agent")));
        assert_eq!(names_text(&["A".into(), "B".into()]), "A、B");
        assert_eq!(names_text(&["A".into(), "B".into(), "C".into(), "D".into()]), "A、B 等 4 个应用");
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
