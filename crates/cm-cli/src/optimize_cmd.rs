//! `optimize list|plan create|diagnose|history`, the maintenance `plan apply` path and the hidden elevated worker
//! (parity 13.1, 14.1, 14.2, 14.4; MNT-01..07, PRT-06; docs/system-maintenance.md, docs/privilege-boundary.md).
//!
//! - `optimize list [--search TEXT]`: the task catalog with the read-only applicability probe of each task and the user
//!   protection state, plus the prohibited list (MNT-05: a search for a registry optimizer finds no task, only why).
//! - `optimize plan create [--tasks ID,ID]`: a `maintenance` plan pending review (no `--tasks`: every task that is not
//!   protected and not known to be inapplicable). `plan review` approves, `plan apply` runs it here.
//! - `plan apply` of a maintenance plan: administrator tasks ask for **one** UAC elevation of a short-lived worker before
//!   any task runs; declined → those tasks are `skipped`, the rest run. The result is a `maintenance_report`.
//! - `optimize diagnose [--window SECONDS]`: read-only performance diagnosis (MNT-06), never ends a process.
//! - `optimize history [--limit N]`: recorded runs (MNT-07); maintenance has no undo.
//!
//! Tests: `CM_MAINTENANCE_TEST_SCRIPT=<json>` replaces every task by a scripted result (nothing on the machine is
//! touched) and then `CM_MAINTENANCE_TEST_ELEVATION=declined|direct` replaces the UAC launcher by a declining one or by
//! an ordinary unelevated child of this binary (the real pipe protocol, no prompt). Without the script both are ignored.

use std::process::ExitCode;

use cm_core::error::{ErrorCode, ExitClass};
use cm_core::maintenance::{Execution, MaintenanceReport, Probe, TaskBackend, TaskOutcome, TaskPlatform, TaskSpec};
use cm_core::protocol::record;
use serde_json::{Value, json};

use crate::args::Parsed;
use crate::commands::{fields, open_stores, store_error};
use crate::{emit, fail};

/// Scripted task results for tests: `{"task_id": {"probe": "applicable|not_needed|not_applicable|unavailable|unknown",
/// "outcome": "applied|unchanged|attention|failed", "code": "...", "reason": "..."}}`; unlisted tasks are applicable
/// and applied.
struct Scripted(serde_json::Map<String, Value>);

impl Scripted {
    fn from_env() -> Option<Scripted> {
        let path = std::env::var_os("CM_MAINTENANCE_TEST_SCRIPT")?;
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str::<Value>(&text).ok()?.as_object().cloned().map(Scripted)
    }

    fn field(&self, id: &str, k: &str, default: &str) -> String {
        self.0.get(id).and_then(|v| v[k].as_str()).unwrap_or(default).to_owned()
    }
}

impl TaskBackend for Scripted {
    fn probe(&self, t: &TaskSpec) -> Probe {
        let (code, reason) = (self.field(t.id, "code", "scripted"), self.field(t.id, "reason", "scripted"));
        match self.field(t.id, "probe", "applicable").as_str() {
            "not_needed" => Probe::NotNeeded { code, summary: reason },
            "not_applicable" => Probe::NotApplicable { code, reason },
            "unavailable" => Probe::Unavailable { code, reason },
            "unknown" => Probe::Unknown { reason },
            _ => Probe::Applicable { detail: "scripted".into() },
        }
    }

    fn execute(&self, t: &TaskSpec) -> Execution {
        let outcome = match self.field(t.id, "outcome", "applied").as_str() {
            "unchanged" => TaskOutcome::Unchanged,
            "attention" => TaskOutcome::Attention,
            "failed" => TaskOutcome::Failed,
            _ => TaskOutcome::Applied,
        };
        Execution::new(outcome, &self.field(t.id, "code", "scripted"), self.field(t.id, "reason", "scripted result"))
    }
}

/// The task implementations of this process: scripted in tests, the fixed platform tasks otherwise.
fn backend() -> (Box<dyn TaskBackend>, bool, bool) {
    if let Some(s) = Scripted::from_env() {
        return (Box::new(s), false, true);
    }
    #[cfg(windows)]
    {
        let t = cm_platform::windows::maintenance::WindowsTasks::real();
        let admin = t.is_admin;
        (Box::new(t), admin, false)
    }
    #[cfg(not(windows))]
    {
        (Box::new(Scripted(serde_json::Map::new())), false, false)
    }
}

fn protection(stores_dir: Option<&std::path::Path>) -> impl Fn(&str) -> Result<(), String> {
    let user = stores_dir.map(cm_platform::protection::load);
    move |task_id: &str| -> Result<(), String> {
        let Some(user) = &user else { return Ok(()) };
        let user = user.as_ref().map_err(|e| format!("protection_unavailable: {e}"))?;
        match cm_core::protect::evaluate_task(task_id, user) {
            cm_core::protect::ProtectionDecision::Allowed => Ok(()),
            denied => Err(format!("{denied:?}")),
        }
    }
}

fn no_elevation() -> cm_app::maintenance::NoElevation {
    cm_app::maintenance::NoElevation("this platform has no elevated worker in this build")
}

/// `optimize list [--search TEXT]` → `maintenance_catalog`.
pub fn list(req: &str, p: &Parsed) -> ExitCode {
    let platform = TaskPlatform::current();
    let (tasks, is_admin, _) = backend();
    let dir = crate::commands::store_dir(p);
    let protect = protection(dir.as_deref());
    let elevation = no_elevation();
    let b = cm_app::maintenance::MaintenanceBackends { platform, tasks: tasks.as_ref(), elevation: &elevation, is_admin, protect_task: &protect };
    let mut items = cm_app::maintenance::catalog_view(&b);
    let query = p.option("search").map(str::to_owned);
    let prohibited: Vec<&cm_core::maintenance::Prohibited> = match &query {
        Some(q) => {
            let (matched, prohibited) = cm_core::maintenance::search(q, platform);
            items.retain(|i| matched.iter().any(|t| i["task_id"] == t.id));
            prohibited
        }
        None => cm_core::maintenance::PROHIBITED.iter().collect(),
    };
    emit(&record("maintenance_catalog", req, fields(json!({
        "platform": platform,
        "query": query,
        "items": items,
        "prohibited": prohibited,
        "elevation": if is_admin { "already_elevated" } else { "one_prompt_per_run" },
    }))), p.format);
    ExitCode::SUCCESS
}

/// `optimize plan create [--tasks ID,ID]` → `plan` (maintenance) pending review.
pub fn plan_create(req: &str, p: &Parsed) -> ExitCode {
    let platform = TaskPlatform::current();
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let protect = protection(Some(&stores.dir));
    let tasks: Vec<&'static TaskSpec> = match p.option("tasks") {
        Some(list) => {
            let ids: Vec<&str> = list.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
            match cm_core::maintenance::resolve_tasks(&ids, platform) {
                Ok(t) => t,
                Err(r) => return fail(req, refusal_code(r.code), &r.message, p.format),
            }
        }
        None => {
            let (backend, is_admin, _) = backend();
            let elevation = no_elevation();
            let b = cm_app::maintenance::MaintenanceBackends { platform, tasks: backend.as_ref(), elevation: &elevation, is_admin, protect_task: &protect };
            let t = cm_app::maintenance::default_selection(&b);
            if t.is_empty() {
                return fail(req, ErrorCode::InvalidArgument, "no maintenance task applies now (protected or not applicable); see `cleanmanager optimize list`", p.format);
            }
            t
        }
    };
    // PRT-06: a protected task named explicitly is refused here; it would only be skipped later.
    if let Some((t, why)) = tasks.iter().find_map(|t| protect(t.id).err().map(|w| (t, w))) {
        return fail(req, ErrorCode::PolicyDenied, &format!("task {} is protected by the user ({why}); remove the protection entry first", t.id), p.format);
    }
    let plan_id = format!("mp_{}", &req[4..]);
    let pdigest = crate::plan_cmd::current_policy_digest(&stores);
    let plan = match cm_core::maintenance::build_maintenance_plan(&tasks, platform, &plan_id, crate::plan_cmd::now_unix(), &pdigest) {
        Ok(pl) => pl,
        Err(r) => return fail(req, refusal_code(r.code), &r.message, p.format),
    };
    if let Err(e) = stores.audit.save_plan_v2(&plan) {
        return store_error(req, &e, p.format);
    }
    emit(&crate::plan_cmd::v2_plan_record(req, &stores, &plan), p.format);
    ExitCode::SUCCESS
}

fn refusal_code(code: &str) -> ErrorCode {
    match code {
        "policy_denied" => ErrorCode::PolicyDenied,
        "unsupported" => ErrorCode::Unsupported,
        "internal_error" => ErrorCode::InternalError,
        _ => ErrorCode::InvalidArgument,
    }
}

pub fn report_record(req: &str, r: &MaintenanceReport) -> Value {
    let v = serde_json::to_value(r).expect("report serializes");
    record("maintenance_report", req, fields(v))
}

fn exit_for(r: &MaintenanceReport) -> ExitCode {
    let c = r.counts;
    let class = if r.items.iter().all(|i| i.outcome == TaskOutcome::Failed) {
        ExitClass::Error(ErrorCode::IoError)
    } else if r.all_clear && c.skipped == 0 && c.unavailable == 0 {
        ExitClass::Success
    } else {
        ExitClass::Partial
    };
    ExitCode::from(class.code() as u8)
}

#[cfg(windows)]
mod elevation {
    use std::path::PathBuf;
    use std::time::Duration;

    use cm_app::maintenance::{ElevatedSession, Elevation, ElevationRefused};
    use cm_core::maintenance::{TaskRun, TaskSpec, WorkerGrant};
    use cm_platform::windows::elevation::{StartError, WorkerConfig, WorkerConnection, WorkerLauncher, start_worker};

    /// One short-lived worker per run, started through the given launcher (UAC in production).
    pub struct WorkerElevation {
        pub launcher: Box<dyn WorkerLauncher>,
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
            match start_worker(self.launcher.as_ref(), &cfg, grant) {
                Ok(c) => Ok(Box::new(Session(c))),
                Err(StartError::Declined) => Err(ElevationRefused::Declined("the UAC prompt was declined".into())),
                Err(StartError::Unavailable(e)) => Err(ElevationRefused::Unavailable(e)),
            }
        }
    }
}

#[cfg(windows)]
fn elevation_backend(scripted: bool) -> Box<dyn cm_app::maintenance::Elevation> {
    use cm_platform::windows::elevation::{DecliningLauncher, DirectLauncher, UacLauncher, WorkerLauncher};
    let exe = std::env::current_exe().unwrap_or_default();
    let launcher: Box<dyn WorkerLauncher> = match (scripted, std::env::var("CM_MAINTENANCE_TEST_ELEVATION").as_deref()) {
        (true, Ok("declined")) => Box::new(DecliningLauncher),
        (true, Ok("direct")) => Box::new(DirectLauncher),
        _ => Box::new(UacLauncher),
    };
    Box::new(elevation::WorkerElevation { launcher, exe })
}

#[cfg(not(windows))]
fn elevation_backend(_: bool) -> Box<dyn cm_app::maintenance::Elevation> {
    Box::new(no_elevation())
}

/// `plan apply` of a stored maintenance plan (dispatched from `apply_cmd` by the stored kind).
pub fn apply(req: &str, p: &Parsed, stores: &cm_store::Stores, plan_id: &str) -> ExitCode {
    use cm_core::host::HostProbe;
    let (tasks, is_admin, scripted) = backend();
    let elevation = elevation_backend(scripted);
    let protect = protection(Some(&stores.dir));
    let b = cm_app::maintenance::MaintenanceBackends { platform: TaskPlatform::current(), tasks: tasks.as_ref(), elevation: elevation.as_ref(), is_admin, protect_task: &protect };
    let principal = cm_core::authz::current_principal();
    let host = cm_platform::host::OsHostProbe.current("cli");
    let ctx = cm_app::execute::ApplyContext {
        principal: &principal,
        host: &host,
        current_rule_set_digest: "",
        now: &crate::plan_cmd::now_unix,
        cancel: &|| false,
        fault: &|_, _| false,
        protect: &|_| Ok(()),
    };
    match cm_app::maintenance::apply_maintenance_plan(stores, &b, plan_id, &ctx) {
        Ok(r) => {
            emit(&report_record(req, &r), p.format);
            exit_for(&r)
        }
        Err(e) => crate::apply_cmd::apply_error(req, plan_id, &e, p),
    }
}

/// `optimize diagnose [--window SECONDS]` → `maintenance_diagnosis` (read-only).
pub fn diagnose(req: &str, p: &Parsed) -> ExitCode {
    let window = match p.option("window").map(str::parse::<u64>).unwrap_or(Ok(2)) {
        Ok(n) if (1..=30).contains(&n) => n,
        _ => return fail(req, ErrorCode::InvalidArgument, "--window must be 1 to 30 seconds", p.format),
    };
    #[cfg(windows)]
    let input = cm_platform::windows::maintenance_diag::sample(std::time::Duration::from_secs(window));
    #[cfg(not(windows))]
    let input: cm_core::maintenance::DiagnosisInput = return fail(req, ErrorCode::Unsupported, "the diagnosis is not implemented on this platform yet", p.format);
    let findings = cm_core::maintenance::diagnose(&input);
    let top: Vec<Value> = {
        let mut ps: Vec<_> = input.processes.iter().filter(|x| x.pid != 0).collect();
        ps.sort_by(|a, b| b.window_core_percent.total_cmp(&a.window_core_percent));
        ps.iter().take(5).map(|x| json!({"process_id": x.pid, "name": x.name, "image": x.image, "cpu_core_percent": (x.window_core_percent * 10.0).round() / 10.0,
            "working_set_bytes": x.working_set_bytes.to_string()})).collect()
    };
    emit(&record("maintenance_diagnosis", req, fields(json!({
        "sampled_at": cm_core::time::now_utc(),
        "window_seconds": window,
        "read_only": true,
        "findings": findings,
        "top_processes": top,
        "memory_load_percent": input.memory_load_percent,
        "commit_used_percent": input.commit_used_percent.map(|c| (c * 10.0).round() / 10.0),
        "note": "Read-only: no process is ended and no setting is changed. Open the status view or Task Manager to act.",
    }))), p.format);
    ExitCode::SUCCESS
}

/// `optimize history [--limit N]` → `maintenance_history`.
pub fn history(req: &str, p: &Parsed) -> ExitCode {
    let limit = match p.option("limit").map(str::parse::<u32>).unwrap_or(Ok(20)) {
        Ok(n) if (1..=500).contains(&n) => n,
        _ => return fail(req, ErrorCode::InvalidArgument, "--limit must be 1 to 500", p.format),
    };
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    match cm_app::maintenance::history(&stores, limit) {
        Ok(runs) => {
            emit(&record("maintenance_history", req, fields(json!({
                "items": runs,
                "undo": "unavailable",
                "note": "Maintenance tasks are not undone; each run lists its tasks, outcomes and reasons.",
            }))), p.format);
            ExitCode::SUCCESS
        }
        Err(e) => store_error(req, &e, p.format),
    }
}

/// The hidden worker entry (`cleanmanager __maintenance-worker ...`): never a user command. Exit code 0 = served,
/// 2 = refused to start (bad arguments, pipe or parent).
pub fn worker_main(args: &[std::ffi::OsString]) -> ExitCode {
    #[cfg(windows)]
    {
        let args: Option<Vec<String>> = args.iter().map(|a| a.to_str().map(str::to_owned)).collect();
        let Some(args) = args else { return ExitCode::from(2) };
        let wa = match cm_platform::windows::elevation::WorkerArgs::parse(&args) {
            Ok(w) => w,
            Err(_) => return ExitCode::from(2),
        };
        let (tasks, _, _) = backend();
        match cm_platform::windows::elevation::serve(&wa, tasks.as_ref(), &crate::plan_cmd::now_unix) {
            Ok(_) => ExitCode::SUCCESS,
            Err(_) => ExitCode::from(2),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        ExitCode::from(2)
    }
}
