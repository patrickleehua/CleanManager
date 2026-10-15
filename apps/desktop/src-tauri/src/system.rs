//! 优化 (MNT-01..07), 状态 › 进程 (STS-04/05) and 分析 › 空间洞察's overview cards and old-downloads review (DIN-03/04)
//! for the desktop, over the services the CLI uses (`cm_app::{maintenance, processes, insights}`; `optimize …`,
//! `process …`, `status alerts`, `insights overview|downloads`).
//!
//! Safety properties of this IPC surface:
//! - Catalog, search, diagnosis, history, process list and insights only read. The diagnosis never ends a process
//!   and changes no setting.
//! - Writes of CleanManager's own settings (process pins, alert settings) go to its store, as the CLI's do.
//! - Every change to the machine is a stored plan pending review: a maintenance plan (catalog tasks by id), a plan
//!   ending exactly one process (re-identified by PID, start time and executable at execution; protected processes
//!   are refused), and a manual-selection plan moving chosen old downloads to the Recycle Bin (files named by the
//!   item ids of the last review, bound through a fresh read-only scan of the Downloads folder).
//! - Maintenance runs (`apply_maintenance`) list the administrator tasks before any task runs and ask Windows once
//!   (UAC) to start the short-lived worker; declining or an unavailable worker skips only those tasks (MNT-04).
//!   The worker is this executable started with the hidden worker argument (`worker_main`), serving only the
//!   granted tasks of one plan digest to the parent that started it, exactly as the CLI's worker does.

use cm_core::error::ErrorCode;
use cm_core::path::RawPath;
use serde::Serialize;
use serde_json::{Value, json};

use crate::args;
use crate::cleanup::CreatedPlanDto;
use crate::commands::{St, blocking};
use crate::dto::{IpcError, IpcResult, last_component};
use crate::host::{AppState, lock};

/// Most old downloads in one Recycle Bin plan request.
pub const MAX_DOWNLOADS: usize = 500;
/// Rows of the old-downloads review handed to the page (the totals cover every file).
pub const DOWNLOADS_SHOWN: usize = 500;

fn ipc(code: ErrorCode, reason: &'static str, message: impl Into<String>) -> IpcError {
    IpcError::new(code, reason, message)
}

/// The last old-downloads review: its root and the listed files by item id.
#[derive(Debug, Clone, Default)]
pub struct DownloadsCache {
    pub root: Option<RawPath>,
    pub items: Vec<(String, RawPath)>,
}

/// User protection of a maintenance task id (PRT-06); fails closed when the list cannot be read.
pub fn task_protection(store_dir: Option<&std::path::Path>) -> impl Fn(&str) -> Result<(), String> {
    let user = store_dir.map(cm_platform::protection::load);
    move |task_id: &str| -> Result<(), String> {
        let Some(user) = &user else { return Ok(()) };
        let user = user.as_ref().map_err(|e| format!("protection_unavailable: {e}"))?;
        match cm_core::protect::evaluate_task(task_id, user) {
            cm_core::protect::ProtectionDecision::Allowed => Ok(()),
            denied => Err(format!("{denied:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Old downloads (DIN-04)

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DownloadItemDto {
    pub item_id: String,
    pub name: String,
    pub path_display: String,
    pub logical_bytes: String,
    pub modified_at: String,
    pub age_days: u64,
    /// Looks like an installer package; such files can also go through the installer review.
    pub installer_like: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DownloadsDto {
    pub root_display: Option<String>,
    pub older_than_days: u32,
    pub observed_at: String,
    pub item_count: u64,
    pub total_logical_bytes: Option<String>,
    pub note: Option<String>,
    /// Largest first; nothing is selected.
    pub items: Vec<DownloadItemDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OverviewDto {
    pub supported: bool,
    pub generated_at: String,
    /// DIN-03 cards: known size or unknown with a note, count, oldest observation.
    pub cards: Vec<Value>,
    pub downloads: Option<DownloadsDto>,
}

/// Review rows with host item ids (`dl_<n>`), and the cache that resolves them later.
pub fn downloads_dto(r: &cm_core::insights::OldDownloadsReview, shown: usize) -> (DownloadsDto, DownloadsCache) {
    let ids: Vec<(String, RawPath)> = r.items.iter().enumerate().map(|(i, x)| (format!("dl_{i}"), x.path.clone())).collect();
    let items = r
        .items
        .iter()
        .zip(&ids)
        .take(shown)
        .map(|(x, (id, _))| {
            let d = x.path.display();
            DownloadItemDto {
                item_id: id.clone(),
                name: last_component(&d).to_owned(),
                path_display: d.clone(),
                logical_bytes: x.logical_bytes.to_string(),
                modified_at: x.modified_at.clone(),
                age_days: x.age_days,
                installer_like: x.installer_like,
            }
        })
        .collect();
    let dto = DownloadsDto {
        root_display: r.root.as_ref().map(RawPath::display),
        older_than_days: r.older_than_days,
        observed_at: r.observed_at.clone(),
        item_count: r.item_count,
        total_logical_bytes: r.total_logical_bytes.map(|b| b.to_string()),
        note: r.note.clone(),
        items,
    };
    (dto, DownloadsCache { root: r.root.clone(), items: ids })
}

/// Overview cards as the page shows them (paths and links stay in the host).
pub fn cards_of(wire: &Value) -> Vec<Value> {
    wire["cards"]
        .as_array()
        .map(|cards| {
            cards
                .iter()
                .map(|c| json!({"card_id": c["card_id"], "kind": c["kind"], "title": c["title"], "logical_bytes": c["logical_bytes"], "count": c["count"], "size_note": c["size_note"], "observed_at": c["observed_at"]}))
                .collect()
        })
        .unwrap_or_default()
}

/// The entry of `target` in a scan of an ancestor folder (case-insensitive path walk from the root).
pub fn find_entry(view: &dyn cm_core::analysis::IndexView, target: &RawPath) -> Option<u64> {
    let want = target.display().to_lowercase();
    let mut cur = view.root();
    loop {
        let here = view.path(cur).display().to_lowercase();
        if here == want {
            return Some(cur);
        }
        cur = view.children(cur).iter().copied().find(|&c| {
            let p = view.path(c).display().to_lowercase();
            want == p || want.strip_prefix(&p).is_some_and(|rest| rest.starts_with('\\') || rest.starts_with('/'))
        })?;
    }
}

// ---------------------------------------------------------------------------------------------
// Maintenance

#[derive(Debug, Clone, Serialize)]
pub struct MaintenanceSearchDto {
    pub query: String,
    pub task_ids: Vec<String>,
    /// MNT-05: what is not offered, and why (a search for a registry optimizer finds only this).
    pub prohibited: Vec<Value>,
}

pub fn search_catalog(query: &str) -> MaintenanceSearchDto {
    let (tasks, prohibited) = cm_core::maintenance::search(query, cm_core::maintenance::TaskPlatform::current());
    MaintenanceSearchDto {
        query: query.into(),
        task_ids: tasks.iter().map(|t| t.id.to_owned()).collect(),
        prohibited: prohibited.iter().map(|p| serde_json::to_value(p).unwrap_or(Value::Null)).collect(),
    }
}

/// Run a stored, reviewed maintenance plan with the given backends; the operation report the shared history reads.
pub fn run_maintenance(
    stores: &cm_store::Stores, b: &cm_app::maintenance::MaintenanceBackends<'_>, plan_id: &str, cancel: &dyn Fn() -> bool, host: &cm_core::host::HostRef,
) -> Result<cm_app::execute::OperationReport, cm_app::execute::ApplyError> {
    let principal = cm_core::authz::current_principal();
    let ctx = cm_app::execute::ApplyContext {
        principal: &principal,
        host,
        current_rule_set_digest: "",
        now: &crate::cleanup::now_unix,
        cancel,
        fault: &|_, _| false,
        protect: &|_| Ok(()),
    };
    let report = cm_app::maintenance::apply_maintenance_plan(stores, b, plan_id, &ctx)?;
    let json = stores.audit.operation_report(&report.operation_id)?.ok_or_else(|| cm_app::execute::ApplyError::Conflict(report.operation_id.clone()))?;
    serde_json::from_str(&json).map_err(|e| cm_app::execute::ApplyError::Store(cm_store::StoreError::InvalidState(e.to_string())))
}

#[cfg(windows)]
mod elevation {
    use std::path::PathBuf;
    use std::time::Duration;

    use cm_app::maintenance::{ElevatedSession, Elevation, ElevationRefused};
    use cm_core::maintenance::{TaskRun, TaskSpec, WorkerGrant};
    use cm_platform::windows::elevation::{StartError, UacLauncher, WorkerConfig, WorkerConnection, start_worker};

    /// One short-lived worker per run, started through one UAC prompt (the CLI's `WorkerElevation`).
    pub struct WorkerElevation {
        pub exe: PathBuf,
    }

    struct Session(WorkerConnection);

    impl ElevatedSession for Session {
        fn run(&mut self, task: &TaskSpec) -> Result<TaskRun, String> {
            self.0.run(task)
        }
        fn close(self: Box<Self>) {
            self.0.close();
        }
    }

    impl Elevation for WorkerElevation {
        fn start(&self, grant: &WorkerGrant) -> Result<Box<dyn ElevatedSession + '_>, ElevationRefused> {
            let cfg = WorkerConfig { exe: self.exe.clone(), connect_timeout: Duration::from_secs(60), idle_seconds: cm_platform::windows::elevation::DEFAULT_IDLE_SECONDS };
            match start_worker(&UacLauncher, &cfg, grant) {
                Ok(c) => Ok(Box::new(Session(c))),
                Err(StartError::Declined) => Err(ElevationRefused::Declined("the Windows administrator prompt was declined".into())),
                Err(StartError::Unavailable(e)) => Err(ElevationRefused::Unavailable(e)),
            }
        }
    }
}

/// Apply a stored maintenance plan from the operation thread (real tasks; one UAC prompt for administrator tasks).
#[cfg(windows)]
pub fn apply_maintenance(stores: &cm_store::Stores, plan_id: &str, cancel: &dyn Fn() -> bool) -> Result<cm_app::execute::OperationReport, cm_app::execute::ApplyError> {
    use cm_core::host::HostProbe;
    let tasks = cm_platform::windows::maintenance::WindowsTasks::real();
    let is_admin = tasks.is_admin;
    let exe = std::env::current_exe().map_err(|e| cm_app::execute::ApplyError::Store(cm_store::StoreError::InvalidArgument(format!("own executable unknown: {e}"))))?;
    let elevation = elevation::WorkerElevation { exe };
    let protect = task_protection(Some(&stores.dir));
    let b = cm_app::maintenance::MaintenanceBackends { platform: cm_core::maintenance::TaskPlatform::Windows, tasks: &tasks, elevation: &elevation, is_admin, protect_task: &protect };
    let host = cm_platform::host::OsHostProbe.current("desktop");
    run_maintenance(stores, &b, plan_id, cancel, &host)
}

/// The hidden elevated worker entry: `cleanmanager-desktop __maintenance-worker --pipe … --parent-pid … …`.
/// Returns the process exit code when the arguments ask for the worker (0 served, 2 refused), otherwise `None`.
#[cfg(windows)]
pub fn worker_main() -> Option<i32> {
    let mut it = std::env::args_os().skip(1);
    if it.next()? != cm_core::maintenance::WORKER_SUBCOMMAND {
        return None;
    }
    let rest: Option<Vec<String>> = it.map(|a| a.to_str().map(str::to_owned)).collect();
    let Some(rest) = rest else { return Some(2) };
    let Ok(wa) = cm_platform::windows::elevation::WorkerArgs::parse(&rest) else { return Some(2) };
    let tasks = cm_platform::windows::maintenance::WindowsTasks::real();
    Some(match cm_platform::windows::elevation::serve(&wa, &tasks, &crate::cleanup::now_unix) {
        Ok(_) => 0,
        Err(_) => 2,
    })
}

// ---------------------------------------------------------------------------------------------
// Windows

#[cfg(windows)]
mod win {
    use std::time::Duration;

    use cm_core::host::HostProbe;
    use cm_core::maintenance::TaskPlatform;

    use super::*;
    use crate::cleanup::{new_id, now_unix};

    // ---- insights ------------------------------------------------------------------------------

    pub fn overview(s: &AppState, days: u32) -> OverviewDto {
        let known = cm_platform::windows::insights::list();
        let downloads = cm_platform::windows::insights::old_downloads(days);
        let dirs = cm_platform::windows::insights::card_dirs();
        let at = cm_core::time::now_utc();
        let wire = cm_app::insights::overview_wire(&known, &downloads, &dirs, &at);
        let (dto, cache) = downloads_dto(&downloads, DOWNLOADS_SHOWN);
        lock(&s.suite).downloads = Some(cache);
        OverviewDto { supported: true, generated_at: at, cards: cards_of(&wire), downloads: Some(dto) }
    }

    pub fn downloads_plan(s: &AppState, ids: &[String]) -> IpcResult<CreatedPlanDto> {
        use cm_core::manual_trash::{ManualInput, SelectionGuard, build_trash_plan};
        let _busy = crate::suite::claim(s)?;
        let cache = lock(&s.suite).downloads.clone().ok_or_else(|| ipc(ErrorCode::InvalidArgument, "not_reviewed", "Review old downloads again; the list is no longer available"))?;
        let root = cache.root.clone().ok_or_else(|| ipc(ErrorCode::InvalidArgument, "no_downloads", "No Downloads folder was found"))?;
        let mut targets = Vec::with_capacity(ids.len());
        for id in ids {
            let Some((_, p)) = cache.items.iter().find(|(i, _)| i == id) else {
                return Err(ipc(ErrorCode::InvalidArgument, "unknown_item", format!("{id} is not in the last review; review old downloads again")));
            };
            targets.push(p.clone());
        }
        // MTR-03: a volume without a usable recycle bin fails the plan; nothing is ever deleted instead.
        if let Err(why) = cm_platform::windows::actions::recycle_bin_ready(&root) {
            return Err(ipc(ErrorCode::Unsupported, "recycle_bin_unavailable", format!("{}: {why}; there is no permanent-delete alternative", root.display())));
        }
        let stores = crate::suite::own_stores(s)?;
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let scan_id = format!("s_desk_dl_{}_{nanos}", std::process::id());
        let request = cm_app::scan::ScanRequest {
            scan_id: scan_id.clone(),
            scope_id: format!("scope_{}", crate::suite::root_id(&root)),
            root: root.clone(),
            host: cm_platform::host::OsHostProbe.current("desktop"),
        };
        cm_app::scan::run_scan(&stores.index, &cm_platform::windows::directory::DirectoryBackend, &request, &cm_core::scan::ScanOptions::default())
            .map_err(|e| ipc(ErrorCode::IoError, "scan_failed", format!("{}: {e}", root.display())))?;
        let status = stores.index.scan_status(&scan_id)?;
        let view = stores.index.view(&scan_id)?;
        let mut entries = Vec::with_capacity(targets.len());
        for t in &targets {
            match find_entry(&view, t) {
                Some(e) => entries.push(e),
                None => return Err(ipc(ErrorCode::PlanStale, "selection_stale", format!("{} is gone or moved since the review; review old downloads again", t.display()))),
            }
        }
        let hard = cm_platform::protection::hard_deny_set();
        let user = cm_platform::protection::load(&stores.dir);
        let problem = user.as_ref().err().map(ToString::to_string);
        let guard = SelectionGuard { hard: Some(&hard), user: user.as_ref().map_err(|_| problem.as_deref().unwrap_or("unavailable")) };
        let plan_id = new_id("mp");
        let pdigest = crate::cleanup::policy_digest(&stores);
        let observed_at = status.finished_at.clone().unwrap_or_else(|| status.started_at.clone());
        let input = ManualInput {
            plan_id: &plan_id,
            scope_id: Some(&status.scope_id),
            scan_id: Some(&status.scan_id),
            scan_revision: Some(status.revision),
            created_unix: now_unix(),
            observed_at: &observed_at,
            policy_digest: &pdigest,
        };
        let plan = build_trash_plan(&view, &entries, &guard, &input).map_err(|e| IpcError::new(e.code(), "plan_refused", e.to_string()))?;
        stores.audit.save_plan_v2(&plan)?;
        Ok(CreatedPlanDto::new(plan_id))
    }

    // ---- maintenance -------------------------------------------------------------------------

    pub fn catalog(s: &AppState) -> Value {
        let platform = TaskPlatform::current();
        let tasks = cm_platform::windows::maintenance::WindowsTasks::real();
        let is_admin = tasks.is_admin;
        let protect = task_protection(s.store_dir.as_deref());
        let elevation = cm_app::maintenance::NoElevation("catalog view only");
        let b = cm_app::maintenance::MaintenanceBackends { platform, tasks: &tasks, elevation: &elevation, is_admin, protect_task: &protect };
        let items = cm_app::maintenance::catalog_view(&b);
        let defaults: Vec<&str> = cm_app::maintenance::default_selection(&b).iter().map(|t| t.id).collect();
        json!({
            "supported": !cm_core::maintenance::catalog(platform).is_empty(),
            "platform": platform,
            "items": items,
            "default_selection": defaults,
            "prohibited": cm_core::maintenance::PROHIBITED,
            "is_admin": is_admin,
            "elevation": if is_admin { "already_elevated" } else { "one_prompt_per_run" },
            "observed_at": cm_core::time::now_utc(),
        })
    }

    pub fn plan(s: &AppState, ids: &[String]) -> IpcResult<CreatedPlanDto> {
        let platform = TaskPlatform::current();
        let stores = crate::suite::own_stores(s)?;
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let refusal = |r: cm_core::maintenance::Refusal| {
            let code = match r.code {
                "policy_denied" => ErrorCode::PolicyDenied,
                "unsupported" => ErrorCode::Unsupported,
                "internal_error" => ErrorCode::InternalError,
                _ => ErrorCode::InvalidArgument,
            };
            IpcError::new(code, "refused", r.message)
        };
        let tasks = cm_core::maintenance::resolve_tasks(&refs, platform).map_err(refusal)?;
        let protect = task_protection(Some(&stores.dir));
        // PRT-06: a protected task named explicitly is refused here; it would only be skipped later.
        if let Some((t, why)) = tasks.iter().find_map(|t| protect(t.id).err().map(|w| (t, w))) {
            return Err(ipc(ErrorCode::PolicyDenied, "protected", format!("task {} is protected ({why}); remove the protection entry first", t.id)));
        }
        let plan_id = new_id("mt");
        let pdigest = crate::cleanup::policy_digest(&stores);
        let plan = cm_core::maintenance::build_maintenance_plan(&tasks, platform, &plan_id, now_unix(), &pdigest).map_err(refusal)?;
        stores.audit.save_plan_v2(&plan)?;
        Ok(CreatedPlanDto::new(plan_id))
    }

    pub fn diagnose() -> Value {
        let input = cm_platform::windows::maintenance_diag::sample(Duration::from_secs(2));
        let findings = cm_core::maintenance::diagnose(&input);
        let mut ps: Vec<_> = input.processes.iter().filter(|x| x.pid != 0).collect();
        ps.sort_by(|a, b| b.window_core_percent.total_cmp(&a.window_core_percent));
        let top: Vec<Value> = ps
            .iter()
            .take(5)
            .map(|x| json!({"process_id": x.pid, "name": x.name, "image": x.image, "cpu_core_percent": (x.window_core_percent * 10.0).round() / 10.0, "working_set_bytes": x.working_set_bytes.to_string()}))
            .collect();
        json!({
            "supported": true,
            "sampled_at": cm_core::time::now_utc(),
            "window_seconds": 2,
            "read_only": true,
            "findings": findings,
            "top_processes": top,
            "memory_load_percent": input.memory_load_percent,
            "commit_used_percent": input.commit_used_percent.map(|c| (c * 10.0).round() / 10.0),
        })
    }

    // ---- processes ---------------------------------------------------------------------------

    const WINDOW_MS: u64 = 1000;

    fn apps_map() -> Vec<(String, String)> {
        cm_platform::windows::apps::inventory().0.into_iter().filter_map(|a| a.install_location.map(|l| (a.name, l.display()))).collect()
    }

    /// `image_path` as display text (the page never sees a raw path object).
    pub(super) fn display_paths(body: &mut Value) {
        if let Some(items) = body["items"].as_array_mut() {
            for i in items {
                let d = serde_json::from_value::<RawPath>(i["image_path"].clone()).ok().map(|p| p.display());
                i["image_path"] = d.map_or(Value::Null, Value::String);
            }
        }
    }

    pub fn processes(s: &AppState, key: cm_core::process::SortKey, search: Option<&str>, limit: usize) -> IpcResult<Value> {
        use cm_platform::windows::processes::{DISK_NOTE, NETWORK_REASON, list_with, windows_dir};
        let facts = list_with(Duration::from_millis(WINDOW_MS), true).map_err(|e| ipc(ErrorCode::IoError, "io_error", e))?;
        let pins = s.store_dir.as_deref().map(cm_app::processes::load_pins).unwrap_or_default();
        let rows = cm_core::process::explain(facts, std::process::id(), &windows_dir(), &apps_map(), &pins);
        let mut body = cm_app::processes::list_wire(rows, key, search, limit, &cm_core::time::now_utc(), WINDOW_MS, &[DISK_NOTE, NETWORK_REASON]);
        display_paths(&mut body);
        body["supported"] = Value::Bool(true);
        body["pins"] = json!(pins);
        Ok(body)
    }

    pub fn end_plan(s: &AppState, pid: u32) -> IpcResult<CreatedPlanDto> {
        use cm_platform::windows::processes::{file_sha256, list_with, signature_of, windows_dir};
        let stores = crate::suite::own_stores(s)?;
        let facts = list_with(Duration::from_millis(200), false).map_err(|e| ipc(ErrorCode::IoError, "io_error", e))?;
        let Some(mut f) = facts.into_iter().find(|f| f.pid == pid) else {
            return Err(ipc(ErrorCode::InvalidArgument, "not_running", format!("process {pid} is no longer running; refresh the list")));
        };
        let image = f.image_path.as_ref().map(RawPath::display);
        if let Some(i) = &image {
            f.signature = signature_of(i);
        }
        let row = cm_core::process::explain(vec![f], std::process::id(), &windows_dir(), &apps_map(), &[]).remove(0);
        let plan_id = new_id("pp");
        let pdigest = crate::cleanup::policy_digest(&stores);
        let plan = match cm_app::processes::end_plan(&row, image.as_deref().and_then(file_sha256), &plan_id, now_unix(), &pdigest) {
            Ok(p) => p,
            Err(cm_app::processes::EndPlanError::Refused(r @ cm_core::process::EndRefusal::Protected(_))) => return Err(ipc(ErrorCode::PolicyDenied, "protected", r.to_string())),
            Err(e) => return Err(ipc(ErrorCode::InvalidArgument, "refused", e.to_string())),
        };
        stores.audit.save_plan_v2(&plan)?;
        crate::software::note_plan(s, &plan_id, json!({"pid": pid, "name": row.facts.name, "purpose": row.purpose, "unidentified_reason": row.unidentified_reason}));
        Ok(CreatedPlanDto::new(plan_id))
    }
}

#[cfg(not(windows))]
fn unsupported(what: &str) -> IpcError {
    ipc(ErrorCode::Unsupported, "unsupported", format!("{what} is implemented on Windows only in this build"))
}

// ---------------------------------------------------------------------------------------------
// IPC

#[tauri::command(rename_all = "snake_case")]
pub async fn insights_overview(state: St<'_>, older_than_days: Option<u32>) -> IpcResult<OverviewDto> {
    let s = state.inner().clone();
    blocking(move || {
        let days = older_than_days.unwrap_or(cm_app::insights::DEFAULT_DAYS);
        if !(1..=3650).contains(&days) {
            return Err(ipc(ErrorCode::InvalidArgument, "invalid_argument", "older_than_days must be 1..=3650"));
        }
        #[cfg(windows)]
        return Ok(win::overview(&s, days));
        #[cfg(not(windows))]
        {
            let _ = (s, days);
            Ok(OverviewDto { supported: false, generated_at: cm_core::time::now_utc(), cards: vec![], downloads: None })
        }
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn downloads_trash_plan(state: St<'_>, item_ids: Vec<String>) -> IpcResult<CreatedPlanDto> {
    let s = state.inner().clone();
    blocking(move || {
        let ids = args::record_ids("download", &item_ids, MAX_DOWNLOADS)?;
        #[cfg(windows)]
        return win::downloads_plan(&s, &ids);
        #[cfg(not(windows))]
        {
            let _ = (s, ids);
            Err(unsupported("manual trash"))
        }
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn maintenance_catalog(state: St<'_>) -> IpcResult<Value> {
    let s = state.inner().clone();
    blocking(move || {
        #[cfg(windows)]
        return Ok(win::catalog(&s));
        #[cfg(not(windows))]
        {
            let _ = s;
            Ok(json!({"supported": false, "items": [], "default_selection": [], "prohibited": cm_core::maintenance::PROHIBITED, "is_admin": false, "elevation": "unavailable"}))
        }
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn maintenance_search(query: String) -> IpcResult<MaintenanceSearchDto> {
    blocking(move || {
        let q = query.trim();
        if q.chars().count() > 80 || q.chars().any(char::is_control) {
            return Err(ipc(ErrorCode::InvalidArgument, "invalid_argument", "search text is too long or malformed"));
        }
        Ok(search_catalog(q))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn maintenance_plan(state: St<'_>, task_ids: Vec<String>) -> IpcResult<CreatedPlanDto> {
    let s = state.inner().clone();
    blocking(move || {
        let ids = args::record_ids("task", &task_ids, 32)?;
        #[cfg(windows)]
        return win::plan(&s, &ids);
        #[cfg(not(windows))]
        {
            let _ = (s, ids);
            Err(unsupported("maintenance tasks"))
        }
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn maintenance_history(state: St<'_>, limit: Option<u32>) -> IpcResult<Value> {
    let s = state.inner().clone();
    blocking(move || {
        let limit = limit.unwrap_or(20).clamp(1, 200);
        let runs = s.with_stores(|st| Ok(cm_app::maintenance::history(st, limit)?))?;
        Ok(json!({"items": runs, "undo": "unavailable"}))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn maintenance_diagnose() -> IpcResult<Value> {
    blocking(move || {
        #[cfg(windows)]
        return Ok(win::diagnose());
        #[cfg(not(windows))]
        Ok(json!({"supported": false, "findings": [], "top_processes": [], "read_only": true}))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn process_list(state: St<'_>, sort: String, search: Option<String>, limit: Option<u32>) -> IpcResult<Value> {
    let s = state.inner().clone();
    blocking(move || {
        let key = cm_core::process::SortKey::parse(&sort).ok_or_else(|| ipc(ErrorCode::InvalidArgument, "invalid_argument", "sort must be cpu, memory, disk, network or name"))?;
        let search = args::name_filter(search.as_deref())?;
        let limit = limit.unwrap_or(200).clamp(1, 500) as usize;
        #[cfg(windows)]
        return win::processes(&s, key, search.as_deref(), limit);
        #[cfg(not(windows))]
        {
            let _ = (s, key, search, limit);
            Ok(json!({"supported": false, "items": [], "pins": []}))
        }
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn process_pin(state: St<'_>, name: String, pinned: bool) -> IpcResult<Vec<String>> {
    let s = state.inner().clone();
    blocking(move || {
        let dir = s.store_dir()?;
        cm_app::processes::set_pin(dir, &name, pinned).map_err(|e| match e.kind() {
            std::io::ErrorKind::InvalidInput => ipc(ErrorCode::InvalidArgument, "invalid_argument", e.to_string()),
            _ => ipc(ErrorCode::IoError, "io_error", e.to_string()),
        })
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn process_end_plan(state: St<'_>, pid: u32) -> IpcResult<CreatedPlanDto> {
    let s = state.inner().clone();
    blocking(move || {
        #[cfg(windows)]
        return win::end_plan(&s, pid);
        #[cfg(not(windows))]
        {
            let _ = (s, pid);
            Err(unsupported("ending processes"))
        }
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn alerts_settings(state: St<'_>) -> IpcResult<cm_core::alerts::AlertSettings> {
    let s = state.inner().clone();
    blocking(move || Ok(cm_app::processes::load_alerts(s.store_dir()?))).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn alerts_save(state: St<'_>, settings: cm_core::alerts::AlertSettings) -> IpcResult<cm_core::alerts::AlertSettings> {
    let s = state.inner().clone();
    blocking(move || {
        let dir = s.store_dir()?;
        cm_app::processes::save_alerts(dir, &settings).map_err(|e| ipc(ErrorCode::InvalidArgument, "invalid_argument", e))?;
        Ok(cm_app::processes::load_alerts(dir))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use cm_core::insights::{DownloadEntry, old_downloads};

    fn p(s: &str) -> RawPath {
        RawPath::windows(s.encode_utf16().collect()).unwrap()
    }

    #[test]
    fn old_downloads_are_listed_largest_first_with_nothing_selected_and_opaque_ids() {
        let now = 1_800_000_000;
        let entries = vec![
            DownloadEntry { path: p(r"C:\Users\Lin\Downloads\small.zip"), logical_bytes: 10, modified_unix: now - 200 * 86_400 },
            DownloadEntry { path: p(r"C:\Users\Lin\Downloads\big.iso"), logical_bytes: 9_000, modified_unix: now - 120 * 86_400 },
            DownloadEntry { path: p(r"C:\Users\Lin\Downloads\new.pdf"), logical_bytes: 50_000, modified_unix: now - 3 * 86_400 },
        ];
        let r = old_downloads(Some(p(r"C:\Users\Lin\Downloads")), entries, now, 90, "2026-10-15T00:00:00Z", None);
        let (dto, cache) = downloads_dto(&r, 10);
        assert_eq!(dto.items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["big.iso", "small.zip"], "recent files are not old; largest first");
        assert_eq!(dto.item_count, 2);
        assert_eq!(dto.total_logical_bytes.as_deref(), Some("9010"));
        assert!(dto.items[0].installer_like);
        assert_eq!(cache.items[0].0, dto.items[0].item_id);
        assert_eq!(cache.items[0].1.display(), r"C:\Users\Lin\Downloads\big.iso");
        let (short, _) = downloads_dto(&r, 1);
        assert_eq!((short.items.len(), short.item_count), (1, 2), "the totals cover every file even when fewer rows are shown");
    }

    #[test]
    fn a_registry_optimizer_search_finds_only_the_reason() {
        let r = search_catalog("注册表优化");
        assert!(r.task_ids.is_empty());
        assert!(r.prohibited.iter().any(|x| x["id"] == "registry-optimizer"));
        assert!(r.prohibited.iter().all(|x| x.get("keywords").is_none()));
        let dns = search_catalog("dns");
        assert!(dns.task_ids.iter().any(|t| t == "windows.dns-flush") || cfg!(not(windows)));
    }

    #[test]
    fn overview_cards_keep_sizes_and_drop_paths() {
        let wire = json!({"cards": [{"card_id": "c1", "kind": "old_downloads", "title": "Old downloads", "logical_bytes": "12", "count": 3, "size_note": null, "observed_at": "t", "link": {"path": "C:\\x"}}]});
        let cards = cards_of(&wire);
        assert_eq!(cards[0]["logical_bytes"], "12");
        assert!(cards[0].get("link").is_none());
    }

    /// Scripted tasks: nothing on the machine is touched.
    struct Scripted;

    impl cm_core::maintenance::TaskBackend for Scripted {
        fn probe(&self, _: &cm_core::maintenance::TaskSpec) -> cm_core::maintenance::Probe {
            cm_core::maintenance::Probe::Applicable { detail: "scripted".into() }
        }
        fn execute(&self, _: &cm_core::maintenance::TaskSpec) -> cm_core::maintenance::Execution {
            cm_core::maintenance::Execution::new(cm_core::maintenance::TaskOutcome::Applied, "scripted", "scripted result")
        }
    }

    fn temp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("cm-desktop-{tag}-{}-{}", std::process::id(), crate::cleanup::now_unix()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// MNT-04 wiring of the desktop: a reviewed maintenance plan runs user tasks, and administrator tasks are skipped
    /// (never run unelevated) when no elevated worker is available; the shared operation report records it.
    #[cfg(windows)]
    #[test]
    fn maintenance_runs_reviewed_plans_and_skips_admin_tasks_without_a_worker() {
        use cm_core::maintenance::{TaskPlatform, build_maintenance_plan, ids, resolve_tasks};
        let dir = temp("maint");
        let stores = cm_store::Stores::open(&dir).unwrap();
        let tasks = resolve_tasks(&[ids::DNS_FLUSH, ids::IMAGE_HEALTH], TaskPlatform::Windows).unwrap();
        let plan = build_maintenance_plan(&tasks, TaskPlatform::Windows, "mt_test", crate::cleanup::now_unix(), "pd").unwrap();
        stores.audit.save_plan_v2(&plan).unwrap();
        let elevation = cm_app::maintenance::NoElevation("no worker in this test");
        let host = cm_core::host::HostProbe::current(&cm_platform::host::OsHostProbe, "desktop");
        let b = cm_app::maintenance::MaintenanceBackends { platform: TaskPlatform::Windows, tasks: &Scripted, elevation: &elevation, is_admin: false, protect_task: &|_| Ok(()) };
        // Not reviewed yet: refused before anything runs.
        assert!(matches!(run_maintenance(&stores, &b, "mt_test", &|| false, &host), Err(cm_app::execute::ApplyError::NeedsApproval(_))));
        stores
            .audit
            .save_kind_receipt(&cm_core::authz::KindReceipt {
                receipt_id: "r_test".into(), plan_id: "mt_test".into(), plan_digest: plan.digest.clone(), plan_kind: plan.body.plan_kind,
                principal: cm_core::authz::current_principal(), source: cm_core::authz::ReceiptSource::InteractiveReview, created_at: cm_core::time::now_utc(),
                expires_at: plan.body.expires_at.clone(), revoked_at: None,
            })
            .unwrap();
        let r = run_maintenance(&stores, &b, "mt_test", &|| false, &host).unwrap();
        assert_eq!(r.status, "partial");
        let codes: Vec<&str> = r.items.iter().map(|i| i.code.as_str()).collect();
        assert_eq!(codes, ["applied", "skipped"]);
        assert!(r.items[1].message.starts_with("elevation_unavailable"));
        let hist = cm_app::maintenance::history(&stores, 5).unwrap();
        assert_eq!(hist[0].operation_id, r.operation_id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Old downloads are bound through a fresh scan of their folder: a nested file is found by its path.
    #[cfg(windows)]
    #[test]
    fn a_reviewed_download_is_found_in_a_scan_of_its_folder() {
        use cm_core::host::HostProbe;
        let dir = temp("dl");
        let root = dir.join("Downloads");
        std::fs::create_dir_all(root.join("Sub Folder")).unwrap();
        std::fs::write(root.join("Sub Folder").join("Setup.EXE"), b"x").unwrap();
        std::fs::write(root.join("a.zip"), b"yy").unwrap();
        let stores = cm_store::Stores::open(&dir.join("store")).unwrap();
        let raw = cm_platform::paths::raw_path(root.as_os_str()).unwrap();
        let request = cm_app::scan::ScanRequest { scan_id: "s_dl_test".into(), scope_id: "scope_dl".into(), root: raw.clone(), host: cm_platform::host::OsHostProbe.current("desktop") };
        cm_app::scan::run_scan(&stores.index, &cm_platform::windows::directory::DirectoryBackend, &request, &cm_core::scan::ScanOptions::default()).unwrap();
        let view = stores.index.view("s_dl_test").unwrap();
        let target = cm_platform::paths::raw_path(root.join("sub folder").join("setup.exe").as_os_str()).unwrap();
        let e = find_entry(&view, &target).expect("found case-insensitively");
        use cm_core::analysis::IndexView;
        assert!(view.path(e).display().ends_with("Setup.EXE"));
        let missing = cm_platform::paths::raw_path(root.join("gone.iso").as_os_str()).unwrap();
        assert_eq!(find_entry(&view, &missing), None);
        let sibling = cm_platform::paths::raw_path(root.join("a.zipx").as_os_str()).unwrap();
        assert_eq!(find_entry(&view, &sibling), None, "a name prefix is not a match");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn process_image_paths_become_text() {
        let mut body = json!({"items": [{"image_path": p(r"C:\Windows\explorer.exe")}, {"image_path": null}]});
        win::display_paths(&mut body);
        assert_eq!(body["items"][0]["image_path"], r"C:\Windows\explorer.exe");
        assert!(body["items"][1]["image_path"].is_null());
    }
}
