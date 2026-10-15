//! App update services shared by the CLI and the desktop (parity UPD-01..04, tasks 10.1-10.3).
//!
//! - Settings and the last results live in `<store>/app-updates.json` (this tool's own file).
//! - [`check`] runs detection only when checks are enabled; when they are off it returns the cached results (each with
//!   its original check time) or `not_checked`, and the channels are never called (UPD-04).
//! - [`plan_updates`] puts the selected package-manager updates into one `update` plan (one `tool_adapter` action per
//!   app, reviewed together, executed item by item) and routes the rest: Store apps to the Store, self-updating apps
//!   to the app, unverifiable ones to the official page.
//! - [`run_update`] is the executor arm for `update` plans: [`cm_core::updates::execute`] mapped to an action result.

use std::path::Path;

use cm_core::apps::AppInstance;
use cm_core::path::RawPath;
use cm_core::plan::v2::{ActionMeta, BuildError, ExecutableRef, HeaderInput, NO_RULES_DIGEST, PlanAction, PlanKind, PlanV2, ToolAdapterAction, build};
use cm_core::updates::{
    ADAPTER_VERSION, Channel, DetectInput, Route, STORE_UPDATES_URI, ScoopApp, SelfUpdaterProbe, UpdateBackend, UpdateCheck, UpdateOutcome, UpdateState, UpdateTarget,
    detect, parse_winget_list, upgrade_argv, when_disabled,
};
use serde::{Deserialize, Serialize};

use crate::execute::{ActionResult, ActionStatus};

pub const UPDATES_FILE: &str = "app-updates.json";

/// Seconds a download + upgrade step may take.
pub const UPDATE_TIMEOUT_SECONDS: u32 = 900;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateSettings {
    /// User-triggered checks are allowed. Off = no detection request of any kind (UPD-04).
    pub checks_enabled: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        UpdateSettings { checks_enabled: true }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateStore {
    #[serde(default)]
    pub settings: UpdateSettings,
    #[serde(default)]
    pub last_checked_at: Option<String>,
    #[serde(default)]
    pub results: Vec<UpdateCheck>,
}

pub fn load(store: &Path) -> UpdateStore {
    std::fs::read_to_string(store.join(UPDATES_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save(store: &Path, s: &UpdateStore) -> std::io::Result<()> {
    std::fs::create_dir_all(store)?;
    let tmp = store.join(format!("{UPDATES_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(s).expect("update store serializes"))?;
    std::fs::rename(&tmp, store.join(UPDATES_FILE))
}

/// Turn checks on or off; returns the stored settings.
pub fn set_checks(store: &Path, enabled: bool) -> std::io::Result<UpdateSettings> {
    let mut s = load(store);
    s.settings.checks_enabled = enabled;
    save(store, &s)?;
    Ok(s.settings)
}

/// Detection sources. Each call may reach the network through the package manager; none is made when checks are off.
pub trait UpdateChannels {
    /// `winget list` output (the package manager queries its own sources only).
    fn winget_list(&self) -> Result<String, String>;
    /// Scoop apps from the local Scoop directory (no network).
    fn scoop_apps(&self) -> Vec<ScoopApp>;
    fn self_updater(&self, app: &AppInstance) -> Option<SelfUpdaterProbe>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    pub checks_enabled: bool,
    /// A detection pass ran now (false: cached results or not checked).
    pub checked: bool,
    pub checked_at: Option<String>,
    pub items: Vec<UpdateCheck>,
}

impl CheckReport {
    pub fn to_wire(&self) -> serde_json::Value {
        let mut items: Vec<serde_json::Value> = self.items.iter().map(UpdateCheck::to_wire).collect();
        for (w, c) in items.iter_mut().zip(&self.items) {
            if c.route == Route::OpenStore {
                w["official_url"] = STORE_UPDATES_URI.into();
            }
        }
        let count = |s: UpdateState| self.items.iter().filter(|c| c.state == s).count();
        serde_json::json!({
            "checks_enabled": self.checks_enabled,
            "checked": self.checked,
            "checked_at": self.checked_at,
            "counts": {"update_available": count(UpdateState::UpdateAvailable), "up_to_date": count(UpdateState::UpToDate),
                       "cannot_check": count(UpdateState::CannotCheck), "not_checked": count(UpdateState::NotChecked)},
            "items": items,
        })
    }
}

/// Results without any detection request: the cache (with original times) or `not_checked`.
pub fn cached(store: &Path, apps: &[AppInstance]) -> CheckReport {
    let s = load(store);
    CheckReport { checks_enabled: s.settings.checks_enabled, checked: false, checked_at: s.last_checked_at.clone(), items: when_disabled(apps, &s.results) }
}

/// UPD-01/04: a user-triggered check. With checks off nothing is called and the cached view is returned.
pub fn check(store: &Path, apps: &[AppInstance], ch: &dyn UpdateChannels, now_utc: &str) -> std::io::Result<CheckReport> {
    let mut s = load(store);
    if !s.settings.checks_enabled {
        return Ok(cached(store, apps));
    }
    let winget = Some(ch.winget_list().and_then(|t| parse_winget_list(&t)));
    let scoop = ch.scoop_apps();
    let probe = |a: &AppInstance| ch.self_updater(a);
    let items = detect(&DetectInput { apps, winget, scoop: &scoop, self_updater: &probe, checked_at: now_utc });
    s.last_checked_at = Some(now_utc.into());
    s.results = items.clone();
    save(store, &s)?;
    Ok(CheckReport { checks_enabled: true, checked: true, checked_at: Some(now_utc.into()), items })
}

/// A package-manager executable bound into update actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateTool {
    pub exe: RawPath,
    pub sha256: String,
}

/// Where a selected app goes when it is not in the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Routed {
    pub app_instance_id: String,
    pub name: String,
    pub route: Route,
    pub reason: String,
    pub url: Option<String>,
}

pub struct PlanInputs<'a> {
    pub results: &'a [UpdateCheck],
    pub apps: &'a [AppInstance],
    pub ids: &'a [String],
    pub winget: Option<UpdateTool>,
    pub scoop: Option<UpdateTool>,
    /// Verified signer of the installed app's executables (`None` = unverifiable).
    pub installed_signer: &'a dyn Fn(&AppInstance) -> Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdatePlanError {
    #[error("unknown app instance {0}; run apps updates first and use its app_instance_id")]
    UnknownApp(String),
    #[error(transparent)]
    Build(#[from] BuildError),
}

/// UPD-02/03: one reviewed `update` plan for the verifiable package-manager updates among `ids`, plus routes for the rest.
pub fn plan_updates(i: &PlanInputs<'_>, plan_id: &str, now_unix: i64, policy_digest: &str) -> Result<(Option<PlanV2>, Vec<Routed>), UpdatePlanError> {
    let mut actions = Vec::new();
    let mut meta = Vec::new();
    let mut routed = Vec::new();
    for id in i.ids {
        let check = i.results.iter().find(|c| &c.app_instance_id == id).ok_or_else(|| UpdatePlanError::UnknownApp(id.clone()))?;
        let app = i.apps.iter().find(|a| &a.instance_id == id).ok_or_else(|| UpdatePlanError::UnknownApp(id.clone()))?;
        let route = |route: Route, reason: String, url: Option<String>| Routed { app_instance_id: id.clone(), name: app.name.clone(), route, reason, url };
        if check.state != UpdateState::UpdateAvailable {
            let why = check.reason.clone().unwrap_or_else(|| format!("no update is available ({:?})", check.state).to_lowercase());
            let url = (check.route == Route::OpenStore).then(|| STORE_UPDATES_URI.to_owned()).or(check.official_url.clone());
            routed.push(route(if check.route == Route::PackageManager { Route::None } else { check.route }, why, url));
            continue;
        }
        let tool = match check.channel {
            Some(Channel::Winget) => i.winget.as_ref(),
            Some(Channel::Scoop) => i.scoop.as_ref(),
            Some(Channel::MicrosoftStore) => {
                routed.push(route(Route::OpenStore, "Microsoft Store updates its own apps".into(), Some(STORE_UPDATES_URI.into())));
                continue;
            }
            _ => None,
        };
        let (Some(tool), Some(package_id), Some(to_version)) = (tool, check.channel_id.clone(), check.available_version.clone()) else {
            routed.push(route(Route::OpenOfficialPage, "the package manager for this app was not found on this computer".into(), check.official_url.clone()));
            continue;
        };
        let Some(signer) = (i.installed_signer)(app) else {
            routed.push(route(
                Route::OpenOfficialPage,
                "the installed app has no verifiable signer, so a downloaded package cannot be checked against it".into(),
                check.official_url.clone(),
            ));
            continue;
        };
        let target = UpdateTarget {
            app_instance_id: id.clone(),
            name: app.name.clone(),
            channel: check.channel.expect("matched above"),
            package_id,
            from_version: check.current_version.clone(),
            to_version,
            installed_signer: Some(signer),
            install_location: app.install_location.as_ref().map(RawPath::display),
            official_url: check.official_url.clone(),
        };
        let (adapter_id, argv) = upgrade_argv(&target).expect("package-manager channel");
        actions.push(PlanAction::ToolAdapter(ToolAdapterAction {
            action_id: format!("a{:02}", actions.len() + 1),
            adapter_id: adapter_id.into(),
            adapter_version: ADAPTER_VERSION.into(),
            executable: ExecutableRef { path: tool.exe.clone(), sha256: tool.sha256.clone() },
            argv,
            preview_digest: target.digest(),
            timeout_seconds: UPDATE_TIMEOUT_SECONDS,
        }));
        meta.push(ActionMeta { note: Some(serde_json::to_string(&target).expect("target serializes")), ..Default::default() });
    }
    if actions.is_empty() {
        return Ok((None, routed));
    }
    let header = HeaderInput {
        plan_id,
        plan_kind: PlanKind::Update,
        created_unix: now_unix,
        scope_id: None,
        scan_id: None,
        scan_revision: None,
        rule_set_digest: NO_RULES_DIGEST,
        policy_digest,
    };
    Ok((Some(build(&header, actions, meta)?), routed))
}

fn res(id: &str, status: ActionStatus, code: &str, message: impl Into<String>) -> ActionResult {
    ActionResult { action_id: id.into(), status, code: code.into(), message: message.into(), recovery: "not_applicable".into() }
}

/// Executor arm for one `tool_adapter` action of an `update` plan (UPD-02/03).
pub fn run_update(b: Option<&dyn UpdateBackend>, a: &ToolAdapterAction, m: &ActionMeta) -> ActionResult {
    let id = &a.action_id;
    let Some(b) = b else { return res(id, ActionStatus::Skipped, "unsupported", "app updates are not available on this platform") };
    match cm_core::updates::execute(b, a, m.note.as_deref()) {
        UpdateOutcome::Updated { to_version } => res(id, ActionStatus::Succeeded, "updated", format!("updated to {to_version}")),
        UpdateOutcome::Skipped { code, message } => res(id, ActionStatus::Skipped, code, message),
        UpdateOutcome::Failed { code, message } => res(id, ActionStatus::Failed, &code, message),
        UpdateOutcome::Unknown { message } => res(id, ActionStatus::Unknown, "outcome_unconfirmed", message),
    }
}

/// Display line of an update action for reviews.
pub fn describe(a: &ToolAdapterAction, m: &ActionMeta) -> String {
    match m.note.as_deref().and_then(|n| serde_json::from_str::<UpdateTarget>(n).ok()) {
        Some(t) => format!(
            "{}: {} -> {} via {:?} ({}), package signer must be {}",
            t.name,
            t.from_version.as_deref().unwrap_or("?"),
            t.to_version,
            t.channel,
            a.argv.join(" "),
            t.installed_signer.as_deref().unwrap_or("?")
        ),
        None => format!("{} {}", a.adapter_id, a.argv.join(" ")),
    }
}

/// Windows channels: winget (`winget list`), the local Scoop directory and self-updater file probes.
#[cfg(windows)]
pub struct WindowsChannels {
    pub winget: Option<std::path::PathBuf>,
}

#[cfg(windows)]
impl UpdateChannels for WindowsChannels {
    fn winget_list(&self) -> Result<String, String> {
        match &self.winget {
            Some(exe) => cm_platform::windows::updates::winget_list(exe),
            None => Err("winget (App Installer) is not installed".into()),
        }
    }

    fn scoop_apps(&self) -> Vec<ScoopApp> {
        windows_scoop()
    }

    fn self_updater(&self, app: &AppInstance) -> Option<SelfUpdaterProbe> {
        cm_platform::windows::updates::self_updater(app)
    }
}

#[cfg(windows)]
fn windows_scoop() -> Vec<ScoopApp> {
    let root = std::env::var_os("SCOOP").map(std::path::PathBuf::from).or_else(|| std::env::var_os("USERPROFILE").map(|h| std::path::PathBuf::from(h).join("scoop")));
    root.map(|r| cm_platform::windows::updates::scoop_apps_in(&r)).unwrap_or_default()
}

/// Installed apps for update checks: the inventory plus Scoop apps (Scoop does not register them). Local reads only.
#[cfg(windows)]
pub fn windows_apps() -> Vec<AppInstance> {
    let mut apps = cm_platform::windows::apps::inventory().0;
    let extra = cm_platform::windows::updates::scoop_instances(&windows_scoop(), &apps);
    apps.extend(extra);
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.instance_id.cmp(&b.instance_id)));
    apps
}

/// The package-manager executables to bind into update actions (path + SHA-256 now).
#[cfg(windows)]
pub fn windows_tools(apps: &[AppInstance]) -> (Option<UpdateTool>, Option<UpdateTool>) {
    use cm_platform::windows::processes::file_sha256;
    let tool = |p: std::path::PathBuf| -> Option<UpdateTool> {
        let s = p.to_string_lossy().into_owned();
        Some(UpdateTool { sha256: file_sha256(&s)?, exe: RawPath::windows(s.encode_utf16().collect()).ok()? })
    };
    (cm_platform::windows::updates::winget_exe(apps).and_then(tool), cm_platform::windows::updates::scoop_exe().and_then(tool))
}

#[cfg(test)]
mod tests;
