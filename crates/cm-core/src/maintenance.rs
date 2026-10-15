//! System maintenance (add-mole-parity-suite MNT-01..07, D10, PRT-06).
//!
//! - The catalog is code: each task is a fixed implementation with an ID@version, the problem it solves, its impact,
//!   whether it needs an administrator and when it applies (MNT-01). There is no "speed up x%" promise anywhere.
//! - A run is a reviewed `maintenance` plan (`plan/v2`): `optimize plan create` → `plan review` → `plan apply`. The
//!   catalog digest is the plan's `rule_set_digest`, so a plan made for another catalog version is never run blindly.
//! - Every task is probed first (MNT-02); "not applicable" and "state unclear" skip it instead of trying. Each result
//!   is one of the six outcomes of MNT-03 with a reason; the run is `all_clear` only without `failed`/`attention`.
//! - Administrator tasks are listed together and elevated once (MNT-04): a short-lived worker that accepts only the
//!   [`WorkerRequest::Run`] of a task granted for this plan's digest ([`WorkerSession`]); anything else is refused.
//! - High-risk or unprovable tweaks are not tasks: they are [`PROHIBITED`] entries with the reason (MNT-05).
//! - [`diagnose`] is read-only (MNT-06): it lists findings and next steps, never ends a process or changes a setting.
//! - Runs are recorded per task (MNT-07); maintenance has no generic undo.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::plan::v2::{ActionMeta, HeaderInput, MaintenanceTaskAction, PlanAction, PlanKind, PlanV2};

/// The six task outcomes (MNT-03).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskOutcome {
    /// The action ran and changed something.
    Applied,
    /// Checked: nothing needed doing (e.g. TRIM within its schedule, image healthy).
    Unchanged,
    /// Not run: not applicable, user protected, elevation declined, or state unclear.
    Skipped,
    /// The platform interface this task needs is missing.
    Unavailable,
    /// Ran or checked, and found something the user must follow up.
    Attention,
    /// The action was attempted and did not succeed.
    Failed,
}

impl TaskOutcome {
    pub const ALL: [TaskOutcome; 6] =
        [TaskOutcome::Applied, TaskOutcome::Unchanged, TaskOutcome::Skipped, TaskOutcome::Unavailable, TaskOutcome::Attention, TaskOutcome::Failed];

    pub fn as_str(self) -> &'static str {
        match self {
            TaskOutcome::Applied => "applied",
            TaskOutcome::Unchanged => "unchanged",
            TaskOutcome::Skipped => "skipped",
            TaskOutcome::Unavailable => "unavailable",
            TaskOutcome::Attention => "attention",
            TaskOutcome::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPlatform {
    Windows,
    Macos,
}

impl TaskPlatform {
    pub fn current() -> TaskPlatform {
        if cfg!(windows) { TaskPlatform::Windows } else { TaskPlatform::Macos }
    }
}

/// One catalog task (MNT-01).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSpec {
    pub id: &'static str,
    pub version: &'static str,
    pub platform: TaskPlatform,
    pub title: &'static str,
    pub problem: &'static str,
    pub impact: &'static str,
    pub requires_admin: bool,
    /// When the task applies (shown before running; the probe decides per run).
    pub applies_when: &'static str,
    pub estimated_seconds: u32,
    /// Hard limit for one run of this task (the elevated worker enforces it too).
    pub timeout_seconds: u32,
    /// Search words (English and Chinese) besides the title.
    pub keywords: &'static [&'static str],
}

pub mod ids {
    pub const SHELL_RESTART: &str = "windows.shell-restart";
    pub const ICON_CACHE: &str = "windows.icon-cache-rebuild";
    pub const DNS_FLUSH: &str = "windows.dns-flush";
    pub const TRIM: &str = "windows.trim-when-due";
    pub const IMAGE_HEALTH: &str = "windows.image-health-check";
}

/// Windows first batch (14.2). macOS tasks (14.3) are planned and not in this build.
pub const WINDOWS_TASKS: &[TaskSpec] = &[
    TaskSpec {
        id: ids::SHELL_RESTART,
        version: "1.0.0",
        platform: TaskPlatform::Windows,
        title: "Restart Start menu and Search",
        problem: "Start menu or Search does not open, shows stale results or is slow to respond",
        impact: "Start and Search close and start again on next use; open windows and other apps are not touched",
        requires_admin: false,
        applies_when: "StartMenuExperienceHost or SearchHost runs in your session from the Windows system apps folder",
        estimated_seconds: 10,
        timeout_seconds: 60,
        keywords: &["start menu", "开始菜单", "search", "搜索", "shell", "startmenuexperiencehost", "searchhost"],
    },
    TaskSpec {
        id: ids::ICON_CACHE,
        version: "1.0.0",
        platform: TaskPlatform::Windows,
        title: "Rebuild icon and thumbnail cache",
        problem: "Icons or thumbnails are blank, wrong or outdated",
        impact: "Icons and thumbnails are drawn again on first view; only cache files not in use are removed",
        requires_admin: false,
        applies_when: "iconcache_*/thumbcache_* files exist in your Explorer cache folder",
        estimated_seconds: 5,
        timeout_seconds: 60,
        keywords: &["icon", "图标", "thumbnail", "缩略图", "iconcache", "thumbcache", "cache", "缓存"],
    },
    TaskSpec {
        id: ids::DNS_FLUSH,
        version: "1.0.0",
        platform: TaskPlatform::Windows,
        title: "Flush DNS resolver cache",
        problem: "Sites resolve to old addresses after a network or DNS change",
        impact: "The first lookup of each name is slightly slower once",
        requires_admin: false,
        applies_when: "The Windows DNS client interface is present",
        estimated_seconds: 1,
        timeout_seconds: 30,
        keywords: &["dns", "域名", "解析", "flushdns", "network", "网络"],
    },
    TaskSpec {
        id: ids::TRIM,
        version: "1.0.0",
        platform: TaskPlatform::Windows,
        title: "Retrim solid-state drives when due",
        problem: "An SSD has not been retrimmed for a long time (scheduled optimization did not run)",
        impact: "Short disk activity; data is not moved. Runs only on SSD volumes whose last optimization is overdue",
        requires_admin: true,
        applies_when: "A fixed SSD volume with TRIM enabled was last optimized more than 30 days ago (or never)",
        estimated_seconds: 60,
        timeout_seconds: 900,
        keywords: &["trim", "ssd", "固态", "retrim", "optimize drives", "优化驱动器"],
    },
    TaskSpec {
        id: ids::IMAGE_HEALTH,
        version: "1.0.0",
        platform: TaskPlatform::Windows,
        title: "Check system image health (read-only)",
        problem: "Windows updates or features fail and component store corruption is suspected",
        impact: "Read-only check of the component store; nothing is repaired automatically",
        requires_admin: true,
        applies_when: "The Windows servicing (DISM) interface is present",
        estimated_seconds: 20,
        timeout_seconds: 600,
        keywords: &["dism", "checkhealth", "system image", "系统映像", "component store", "组件存储", "sfc", "corruption", "损坏"],
    },
];

/// The catalog of a platform (macOS: planned, empty in this build).
pub fn catalog(platform: TaskPlatform) -> &'static [TaskSpec] {
    match platform {
        TaskPlatform::Windows => WINDOWS_TASKS,
        TaskPlatform::Macos => &[],
    }
}

pub fn find(task_id: &str) -> Option<&'static TaskSpec> {
    WINDOWS_TASKS.iter().find(|t| t.id == task_id)
}

/// `rule_set_digest` of maintenance plans: the platform catalog (ID, version, admin flag).
pub fn catalog_digest(platform: TaskPlatform) -> String {
    let rows: Vec<serde_json::Value> =
        catalog(platform).iter().map(|t| serde_json::json!({"task_id": t.id, "task_version": t.version, "requires_admin": t.requires_admin})).collect();
    crate::c14n::digest(&serde_json::Value::Array(rows)).expect("catalog rows are canonical")
}

/// Something the catalog deliberately does not offer (MNT-05).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Prohibited {
    pub id: &'static str,
    pub title: &'static str,
    #[serde(skip)]
    pub keywords: &'static [&'static str],
    pub reason: &'static str,
}

pub const PROHIBITED: &[Prohibited] = &[
    Prohibited {
        id: "registry-optimizer",
        title: "Registry cleaning or \"optimization\"",
        keywords: &["registry", "注册表", "regedit", "registry cleaner", "registry optimizer", "注册表优化", "注册表清理"],
        reason: "Removing or rewriting registry data has no measurable benefit and can break applications or Windows; there is no task for it",
    },
    Prohibited {
        id: "performance-privacy-tweaks",
        title: "Registry performance or privacy tweaks",
        keywords: &["tweak", "调优", "telemetry", "遥测", "privacy tweak", "隐私", "性能调优", "group policy", "组策略"],
        reason: "Undocumented setting changes have unprovable effects and are hard to undo; change settings in Windows Settings instead",
    },
    Prohibited {
        id: "service-disable",
        title: "Disabling or deleting system services",
        keywords: &["service", "服务", "disable service", "禁用服务", "sysmain", "superfetch", "wuauserv"],
        reason: "Disabled services break updates, security and features in ways that show up much later",
    },
    Prohibited {
        id: "search-indexing-disable",
        title: "Turning off Windows Search indexing",
        keywords: &["indexing", "索引", "wsearch", "搜索索引", "disable search", "关闭搜索"],
        reason: "Search, Outlook and Settings depend on the index; manage indexed locations in Settings > Privacy & security > Searching Windows",
    },
    Prohibited {
        id: "system-debloat",
        title: "System slimming or component removal",
        keywords: &["debloat", "精简", "remove component", "组件移除", "bloatware", "remove built-in", "删除自带"],
        reason: "Removing Windows components or built-in apps breaks servicing and updates; uninstall apps you chose to install from the Apps page",
    },
    Prohibited {
        id: "memory-release",
        title: "Memory \"release\" or RAM boosting",
        keywords: &["memory", "内存", "ram", "free memory", "内存释放", "booster", "清理内存"],
        reason: "Windows manages memory itself; forcing pages out only makes the next use slower. The diagnosis shows real memory pressure instead",
    },
    Prohibited {
        id: "pagefile-tweak",
        title: "Page file or virtual memory tweaks",
        keywords: &["pagefile", "页面文件", "虚拟内存", "virtual memory", "swap", "交换"],
        reason: "Wrong page file sizes cause crashes and lost crash dumps; Windows sizes it automatically",
    },
    Prohibited {
        id: "rebuild-app-registrations",
        title: "Re-registering all apps or rebuilding app registration databases",
        keywords: &["re-register", "reregister", "appx", "重新注册", "应用注册", "launchservices", "重建数据库"],
        reason: "Mass re-registration can leave apps unusable and cannot be undone; repair one app from Settings > Apps instead",
    },
    Prohibited {
        id: "ssd-full-defrag",
        title: "Full defragmentation of SSDs",
        keywords: &["defrag", "碎片整理", "defragment"],
        reason: "SSDs only need retrim, which the TRIM task does when due; full defragmentation adds wear without benefit",
    },
    Prohibited {
        id: "one-click-speedup",
        title: "One-click speed-up",
        keywords: &["speed up", "speedup", "一键加速", "加速", "boost", "提速"],
        reason: "There is no honest percentage to promise; each task here says what problem it solves and reports what actually happened",
    },
];

fn matches(query: &str, words: impl IntoIterator<Item = String>) -> bool {
    words.into_iter().any(|w| !w.is_empty() && (w.contains(query) || query.contains(&w)))
}

/// Catalog search (MNT-05): matching tasks and the prohibited entries that explain what is not offered.
pub fn search(query: &str, platform: TaskPlatform) -> (Vec<&'static TaskSpec>, Vec<&'static Prohibited>) {
    let q = query.trim().to_lowercase();
    if q.chars().count() < 2 {
        return (catalog(platform).iter().collect(), Vec::new());
    }
    let tasks = catalog(platform)
        .iter()
        .filter(|t| matches(&q, [t.id, t.title, t.problem].into_iter().map(str::to_lowercase).chain(t.keywords.iter().map(|k| k.to_lowercase()))))
        .collect();
    let prohibited = PROHIBITED.iter().filter(|p| matches(&q, [p.id, p.title].into_iter().map(str::to_lowercase).chain(p.keywords.iter().map(|k| k.to_lowercase())))).collect();
    (tasks, prohibited)
}

/// Applicability before execution (MNT-02).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// Run the action.
    Applicable { detail: String },
    /// Nothing to do now (e.g. TRIM within its schedule) → `unchanged` with the reason.
    NotNeeded { code: String, summary: String },
    /// Does not apply to this system → `skipped`.
    NotApplicable { code: String, reason: String },
    /// The platform interface is missing → `unavailable`.
    Unavailable { code: String, reason: String },
    /// State could not be determined → `skipped`; a task is never tried on a guess.
    Unknown { reason: String },
}

impl Probe {
    /// Protocol `maintenance_task_info.applicability`.
    pub fn applicability(&self) -> (&'static str, Option<String>) {
        match self {
            Probe::Applicable { detail } => ("applicable", Some(detail.clone()).filter(|d| !d.is_empty())),
            Probe::NotNeeded { summary, .. } => ("not_needed", Some(summary.clone())),
            Probe::NotApplicable { reason, .. } | Probe::Unavailable { reason, .. } => ("not_applicable", Some(reason.clone())),
            Probe::Unknown { reason } => ("unknown", Some(reason.clone())),
        }
    }
}

/// What an action reports (only `applied`, `unchanged`, `attention` or `failed` are expected here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Execution {
    pub outcome: TaskOutcome,
    pub code: String,
    pub summary: String,
}

impl Execution {
    pub fn new(outcome: TaskOutcome, code: &str, summary: impl Into<String>) -> Execution {
        Execution { outcome, code: code.into(), summary: summary.into() }
    }
}

/// Fixed task implementations of one platform (cm-platform), or scripted ones in tests.
pub trait TaskBackend {
    /// Read-only applicability probe.
    fn probe(&self, task: &TaskSpec) -> Probe;
    /// The task's fixed action; called only after an `Applicable` probe.
    fn execute(&self, task: &TaskSpec) -> Execution;
}

/// One task's recorded result (MNT-03/07); also the worker's reply payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRun {
    pub task_id: String,
    pub task_version: String,
    pub requires_admin: bool,
    pub outcome: TaskOutcome,
    pub code: String,
    pub reason: String,
    /// Ran in the elevated worker (or an already elevated process).
    pub elevated: bool,
    pub started_at: String,
    pub finished_at: String,
}

impl TaskRun {
    /// A result recorded without running the task (protected, declined, unknown task ...).
    pub fn not_run(task_id: &str, task_version: &str, requires_admin: bool, outcome: TaskOutcome, code: &str, reason: impl Into<String>, at: &str) -> TaskRun {
        TaskRun {
            task_id: task_id.into(),
            task_version: task_version.into(),
            requires_admin,
            outcome,
            code: code.into(),
            reason: reason.into(),
            elevated: false,
            started_at: at.into(),
            finished_at: at.into(),
        }
    }
}

/// Probe, then act: the per-task half of the framework (MNT-02/03). Used in-process and inside the elevated worker.
pub fn run_task(backend: &dyn TaskBackend, task: &TaskSpec, elevated: bool, now: &dyn Fn() -> i64) -> TaskRun {
    let started_at = crate::time::utc_from_unix(now());
    let (outcome, code, reason) = match backend.probe(task) {
        Probe::Applicable { .. } => {
            let e = backend.execute(task);
            (e.outcome, e.code, e.summary)
        }
        Probe::NotNeeded { code, summary } => (TaskOutcome::Unchanged, code, summary),
        Probe::NotApplicable { code, reason } => (TaskOutcome::Skipped, code, reason),
        Probe::Unavailable { code, reason } => (TaskOutcome::Unavailable, code, reason),
        Probe::Unknown { reason } => (TaskOutcome::Skipped, "state_unknown".into(), format!("{reason}; skipped rather than tried")),
    };
    TaskRun {
        task_id: task.id.into(),
        task_version: task.version.into(),
        requires_admin: task.requires_admin,
        outcome,
        code,
        reason: if reason.is_empty() { outcome.as_str().into() } else { reason },
        elevated,
        started_at,
        finished_at: crate::time::utc_from_unix(now()),
    }
}

/// How administrator tasks were authorized in a run (MNT-04/07).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElevationState {
    /// No administrator task was to be run.
    NotNeeded,
    /// The process already had an elevated token.
    AlreadyElevated,
    /// One UAC elevation of the short-lived worker.
    Granted,
    /// The user declined UAC: administrator tasks are skipped.
    Declined,
    /// The worker could not be started or verified: administrator tasks are skipped.
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeCounts {
    pub applied: u64,
    pub unchanged: u64,
    pub skipped: u64,
    pub unavailable: u64,
    pub attention: u64,
    pub failed: u64,
}

impl OutcomeCounts {
    pub fn of(items: &[TaskRun]) -> OutcomeCounts {
        let c = |o| items.iter().filter(|r| r.outcome == o).count() as u64;
        OutcomeCounts {
            applied: c(TaskOutcome::Applied),
            unchanged: c(TaskOutcome::Unchanged),
            skipped: c(TaskOutcome::Skipped),
            unavailable: c(TaskOutcome::Unavailable),
            attention: c(TaskOutcome::Attention),
            failed: c(TaskOutcome::Failed),
        }
    }

    /// MNT-03: never "all done" while anything failed or needs attention.
    pub fn all_clear(&self) -> bool {
        self.failed == 0 && self.attention == 0
    }
}

/// Protocol `maintenance_report`: one recorded run (MNT-07). Maintenance has no generic undo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceReport {
    pub operation_id: String,
    pub plan_id: String,
    pub plan_digest: String,
    /// Entry that ran it: `cli` or `desktop`.
    pub entry: String,
    pub principal: String,
    pub elevation: ElevationState,
    pub started_at: String,
    pub finished_at: String,
    pub items: Vec<TaskRun>,
    pub counts: OutcomeCounts,
    pub all_clear: bool,
    /// Always `unavailable`: maintenance actions are not undone (MNT-07).
    pub undo: String,
}

/// Why a maintenance plan cannot be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// Protocol error code: `invalid_argument`, `policy_denied`, `unsupported`, `internal_error`.
    pub code: &'static str,
    pub message: String,
}

fn refuse(code: &'static str, message: impl Into<String>) -> Refusal {
    Refusal { code, message: message.into() }
}

/// Resolve requested task IDs against the catalog; prohibited names are refused with their reason.
pub fn resolve_tasks(task_ids: &[&str], platform: TaskPlatform) -> Result<Vec<&'static TaskSpec>, Refusal> {
    if task_ids.is_empty() {
        return Err(refuse("invalid_argument", "name at least one task (cleanmanager optimize list)"));
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for id in task_ids {
        if !seen.insert(*id) {
            return Err(refuse("invalid_argument", format!("task {id} is named twice")));
        }
        match catalog(platform).iter().find(|t| t.id == *id) {
            Some(t) => out.push(t),
            None => {
                if let Some(p) = PROHIBITED.iter().find(|p| p.id == *id) {
                    return Err(refuse("policy_denied", format!("{} is not offered: {}", p.title, p.reason)));
                }
                if find(id).is_some() {
                    return Err(refuse("unsupported", format!("task {id} belongs to another platform")));
                }
                return Err(refuse("invalid_argument", format!("no maintenance task {id}; run `cleanmanager optimize list`")));
            }
        }
    }
    Ok(out)
}

/// Freeze tasks into a `maintenance` plan pending review (D1/D10). Order follows the catalog.
pub fn build_maintenance_plan(tasks: &[&'static TaskSpec], platform: TaskPlatform, plan_id: &str, created_unix: i64, policy_digest: &str) -> Result<PlanV2, Refusal> {
    if tasks.is_empty() {
        return Err(refuse("invalid_argument", "no task to plan"));
    }
    let digest = catalog_digest(platform);
    let mut ordered: Vec<&TaskSpec> = catalog(platform).iter().filter(|t| tasks.iter().any(|x| x.id == t.id)).collect();
    ordered.dedup_by_key(|t| t.id);
    if ordered.len() != tasks.len() {
        return Err(refuse("invalid_argument", "tasks must be distinct catalog tasks of this platform"));
    }
    let items = ordered
        .iter()
        .enumerate()
        .map(|(i, t)| {
            PlanAction::MaintenanceTask(MaintenanceTaskAction {
                action_id: format!("m{i:07}"),
                task_id: t.id.into(),
                task_version: t.version.into(),
                requires_admin: t.requires_admin,
            })
        })
        .collect();
    let meta = ordered.iter().map(|t| ActionMeta { note: Some(t.title.into()), ..Default::default() }).collect();
    let header = HeaderInput { plan_id, plan_kind: PlanKind::Maintenance, created_unix, scope_id: None, scan_id: None, scan_revision: None, rule_set_digest: &digest, policy_digest };
    crate::plan::v2::build(&header, items, meta).map_err(|e| refuse("internal_error", e.to_string()))
}

/// The tasks of a stored maintenance plan, each checked against this build's catalog: `Err` is the recorded reason to
/// skip it (unknown task, other version or admin flag) — a plan never runs code it was not reviewed for.
pub fn planned_task(a: &MaintenanceTaskAction, platform: TaskPlatform) -> Result<&'static TaskSpec, (&'static str, String)> {
    match catalog(platform).iter().find(|t| t.id == a.task_id) {
        None => Err(("task_unknown", format!("task {} is not in this build's catalog", a.task_id))),
        Some(t) if t.version != a.task_version || t.requires_admin != a.requires_admin => {
            Err(("task_version_changed", format!("the plan was made for {}@{}; this build has {}@{}; create a new plan", a.task_id, a.task_version, t.id, t.version)))
        }
        Some(t) => Ok(t),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Elevated worker protocol (13.1, EXE-08, MNT-04)

pub const WORKER_PROTOCOL: &str = "cm-maintenance-worker/1";
/// Longest request line accepted by the worker; longer input is refused unread.
pub const WORKER_MAX_LINE: usize = 4096;
/// Hidden first argument that turns the CLI binary into the worker (not a user command).
pub const WORKER_SUBCOMMAND: &str = "__maintenance-worker";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRef {
    pub task_id: String,
    pub task_version: String,
}

/// What one worker may run: administrator tasks of one plan digest, fixed when the worker is started (and so covered
/// by the single UAC consent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerGrant {
    pub plan_digest: String,
    pub tasks: Vec<TaskRef>,
}

impl WorkerGrant {
    /// Worker arguments: `--plan-digest D --tasks id@ver,id@ver`.
    pub fn to_args(&self) -> Vec<String> {
        let tasks: Vec<String> = self.tasks.iter().map(|t| format!("{}@{}", t.task_id, t.task_version)).collect();
        vec!["--plan-digest".into(), self.plan_digest.clone(), "--tasks".into(), tasks.join(",")]
    }

    pub fn parse(plan_digest: &str, tasks: &str) -> Result<WorkerGrant, String> {
        let tasks = tasks
            .split(',')
            .map(|s| s.split_once('@').map(|(id, v)| TaskRef { task_id: id.into(), task_version: v.into() }).ok_or_else(|| format!("bad task reference {s}")))
            .collect::<Result<Vec<_>, _>>()?;
        let g = WorkerGrant { plan_digest: plan_digest.into(), tasks };
        g.validate(TaskPlatform::current())?;
        Ok(g)
    }

    /// Only catalog administrator tasks at this build's versions, each once, for a well-formed digest.
    pub fn validate(&self, platform: TaskPlatform) -> Result<(), String> {
        if self.plan_digest.len() != 64 || !self.plan_digest.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
            return Err("plan digest must be 64 lowercase hex characters".into());
        }
        if self.tasks.is_empty() {
            return Err("no task granted".into());
        }
        let mut seen = BTreeSet::new();
        for t in &self.tasks {
            if !seen.insert(t.task_id.as_str()) {
                return Err(format!("task {} granted twice", t.task_id));
            }
            match catalog(platform).iter().find(|s| s.id == t.task_id) {
                Some(s) if s.version == t.task_version && s.requires_admin => {}
                Some(s) if !s.requires_admin => return Err(format!("task {} does not need an administrator", t.task_id)),
                Some(_) => return Err(format!("task {} is not at version {}", t.task_id, t.task_version)),
                None => return Err(format!("task {} is not in the catalog", t.task_id)),
            }
        }
        Ok(())
    }
}

/// Parent → worker. Anything that does not parse as exactly one of these is refused (no command strings exist).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerRequest {
    Run { plan_digest: String, task_id: String, task_version: String },
    Done,
}

/// Worker → parent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerReply {
    Hello { protocol: String, pid: u32, elevated: bool },
    Result { run: TaskRun },
    Refused { code: String, message: String },
    Bye,
}

impl WorkerRequest {
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("request serializes")
    }
}

impl WorkerReply {
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("reply serializes")
    }

    pub fn parse(line: &str) -> Option<WorkerReply> {
        serde_json::from_str(line).ok()
    }
}

/// The worker's request check. One refusal ends the session: the parent never sends anything invalid.
pub struct WorkerSession {
    grant: WorkerGrant,
    platform: TaskPlatform,
    ran: BTreeSet<String>,
}

impl WorkerSession {
    pub fn new(grant: WorkerGrant, platform: TaskPlatform) -> Result<WorkerSession, String> {
        grant.validate(platform)?;
        Ok(WorkerSession { grant, platform, ran: BTreeSet::new() })
    }

    /// The task a request may run, or the refusal (code, message).
    pub fn authorize(&mut self, line: &str) -> Result<Option<&'static TaskSpec>, (&'static str, String)> {
        if line.len() > WORKER_MAX_LINE {
            return Err(("line_too_long", "request exceeds the protocol limit".into()));
        }
        let req: WorkerRequest = serde_json::from_str(line.trim_end()).map_err(|_| ("malformed_request", "not a defined worker request; arbitrary commands are never run".to_string()))?;
        let (plan_digest, task_id, task_version) = match req {
            WorkerRequest::Done => return Ok(None),
            WorkerRequest::Run { plan_digest, task_id, task_version } => (plan_digest, task_id, task_version),
        };
        if plan_digest != self.grant.plan_digest {
            return Err(("wrong_plan", "the request is for another plan".into()));
        }
        if !self.grant.tasks.iter().any(|t| t.task_id == task_id && t.task_version == task_version) {
            return Err(("task_not_granted", format!("task {task_id}@{task_version} is not an administrator task of this plan")));
        }
        let spec = catalog(self.platform).iter().find(|s| s.id == task_id && s.version == task_version && s.requires_admin).ok_or(("task_not_granted", format!("task {task_id} is not in the catalog")))?;
        if !self.ran.insert(task_id.clone()) {
            return Err(("already_run", format!("task {task_id} already ran in this session")));
        }
        Ok(Some(spec))
    }

    /// Handle one line: the reply, and whether the session goes on.
    pub fn handle(&mut self, line: &str, run: &mut dyn FnMut(&'static TaskSpec) -> TaskRun) -> (WorkerReply, bool) {
        match self.authorize(line) {
            Ok(Some(spec)) => (WorkerReply::Result { run: run(spec) }, true),
            Ok(None) => (WorkerReply::Bye, false),
            Err((code, message)) => (WorkerReply::Refused { code: code.into(), message }, false),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Read-only performance diagnosis (MNT-06)

/// One process as sampled (two CPU-time readings `window_seconds` apart).
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessSample {
    pub pid: u32,
    pub name: String,
    /// Executable path when readable: tells which app it belongs to.
    pub image: Option<String>,
    /// CPU during the window, in percent of one logical CPU (0..=100 × CPUs).
    pub window_core_percent: f64,
    /// Average CPU since the process started, in percent of one logical CPU.
    pub lifetime_core_percent: f64,
    pub age_seconds: u64,
    pub working_set_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct VolumeSpace {
    pub display: String,
    pub free_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DiagnosisInput {
    pub window_seconds: f64,
    pub processes: Vec<ProcessSample>,
    pub memory_load_percent: Option<f64>,
    pub commit_used_percent: Option<f64>,
    pub volumes: Vec<VolumeSpace>,
    /// (enabled, total) startup items; `None` = not readable.
    pub startup_items: Option<(usize, usize)>,
    /// Attached disk image files; `None` = not readable.
    pub attached_images: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub code: &'static str,
    pub severity: &'static str,
    pub subject: String,
    pub detail: String,
    pub process_id: Option<u32>,
    pub duration_seconds: Option<u64>,
    /// Where to look next (`status.processes`, `status.memory`, `analyze`, `startup.list`, `explorer`); never an action.
    pub next_step: &'static str,
}

pub mod diag_thresholds {
    /// A process is busy when it used at least this much of one CPU during the window.
    pub const BUSY_CORE_PERCENT: f64 = 80.0;
    /// ... and runaway when it also averaged this much since start, for at least `RUNAWAY_MIN_AGE_SECONDS`.
    pub const RUNAWAY_LIFETIME_CORE_PERCENT: f64 = 50.0;
    pub const RUNAWAY_MIN_AGE_SECONDS: u64 = 600;
    pub const MEMORY_PRESSURE_PERCENT: f64 = 85.0;
    pub const COMMIT_PRESSURE_PERCENT: f64 = 90.0;
    pub const LOW_FREE_PERCENT: f64 = 10.0;
    pub const LOW_FREE_BYTES: u64 = 10 << 30;
    pub const MANY_STARTUP_ITEMS: usize = 15;
    /// Virtual machine hosts under this much of one CPU count as idle.
    pub const IDLE_VM_CORE_PERCENT: f64 = 2.0;
    pub const MAX_PROCESS_FINDINGS: usize = 5;
}

/// Virtual machine host processes (lowercase image names without extension).
const VM_HOSTS: &[&str] = &["vmmem", "vmmemwsl", "vmwp", "virtualboxvm", "vboxheadless", "vmware-vmx", "qemu-system-x86_64", "qemu-system-aarch64"];

fn stem(name: &str) -> String {
    let n = name.to_lowercase();
    n.strip_suffix(".exe").map(str::to_owned).unwrap_or(n)
}

/// Findings in priority order: runaway and busy processes, memory and commit pressure, low disk space, idle virtual
/// machines, attached disk images, many startup items. Read-only by construction: it only looks at the samples.
pub fn diagnose(input: &DiagnosisInput) -> Vec<Finding> {
    use diag_thresholds::*;
    let mut out = Vec::new();
    let mut busy: Vec<&ProcessSample> =
        input.processes.iter().filter(|p| p.pid != 0 && p.window_core_percent >= BUSY_CORE_PERCENT && !VM_HOSTS.contains(&stem(&p.name).as_str())).collect();
    busy.sort_by(|a, b| b.window_core_percent.total_cmp(&a.window_core_percent));
    for p in busy.into_iter().take(MAX_PROCESS_FINDINGS) {
        let runaway = p.lifetime_core_percent >= RUNAWAY_LIFETIME_CORE_PERCENT && p.age_seconds >= RUNAWAY_MIN_AGE_SECONDS;
        let app = p.image.as_deref().map(|i| format!(" ({i})")).unwrap_or_default();
        out.push(Finding {
            code: if runaway { "runaway_process" } else { "busy_process" },
            severity: "warning",
            subject: p.name.clone(),
            detail: if runaway {
                format!("{:.0}% of a CPU now and {:.0}% on average for {} min{app}; it is not ended automatically", p.window_core_percent, p.lifetime_core_percent, p.age_seconds / 60)
            } else {
                format!("{:.0}% of a CPU over the last {:.0} s{app}; it is not ended automatically", p.window_core_percent, input.window_seconds)
            },
            process_id: Some(p.pid),
            duration_seconds: Some(if runaway { p.age_seconds } else { input.window_seconds.round() as u64 }),
            next_step: "status.processes",
        });
    }
    if let Some(m) = input.memory_load_percent.filter(|m| *m >= MEMORY_PRESSURE_PERCENT) {
        out.push(Finding { code: "memory_pressure", severity: "warning", subject: "memory".into(), detail: format!("{m:.0}% of physical memory is in use"), process_id: None, duration_seconds: None, next_step: "status.memory" });
    }
    if let Some(c) = input.commit_used_percent.filter(|c| *c >= COMMIT_PRESSURE_PERCENT) {
        out.push(Finding {
            code: "commit_pressure",
            severity: "warning",
            subject: "page file".into(),
            detail: format!("{c:.0}% of the commit limit (memory plus page file) is used; apps may fail to allocate memory"),
            process_id: None,
            duration_seconds: None,
            next_step: "status.memory",
        });
    }
    for v in &input.volumes {
        if v.total_bytes == 0 {
            continue;
        }
        let pct = v.free_bytes as f64 * 100.0 / v.total_bytes as f64;
        if pct < LOW_FREE_PERCENT || v.free_bytes < LOW_FREE_BYTES {
            out.push(Finding {
                code: "low_disk_space",
                severity: "warning",
                subject: v.display.clone(),
                detail: format!("{:.1} GiB free ({pct:.0}%)", v.free_bytes as f64 / f64::from(1u32 << 30)),
                process_id: None,
                duration_seconds: None,
                next_step: "analyze",
            });
        }
    }
    for p in input.processes.iter().filter(|p| VM_HOSTS.contains(&stem(&p.name).as_str()) && p.window_core_percent < IDLE_VM_CORE_PERCENT) {
        out.push(Finding {
            code: "idle_virtual_machine",
            severity: "info",
            subject: p.name.clone(),
            detail: format!("a virtual machine host is running idle and holds {:.1} GiB; shut the VM down from its own manager if unused", p.working_set_bytes as f64 / f64::from(1u32 << 30)),
            process_id: Some(p.pid),
            duration_seconds: Some(p.age_seconds),
            next_step: "status.processes",
        });
    }
    for img in input.attached_images.iter().flatten() {
        out.push(Finding { code: "attached_disk_image", severity: "info", subject: img.clone(), detail: "a disk image is still attached; eject it in Explorer when done".into(), process_id: None, duration_seconds: None, next_step: "explorer" });
    }
    if let Some((enabled, total)) = input.startup_items.filter(|(e, _)| *e >= MANY_STARTUP_ITEMS) {
        out.push(Finding {
            code: "many_startup_items",
            severity: "info",
            subject: "startup".into(),
            detail: format!("{enabled} of {total} startup items are enabled; review them with `cleanmanager startup list`"),
            process_id: None,
            duration_seconds: None,
            next_step: "startup.list",
        });
    }
    out
}

#[cfg(test)]
mod tests;
