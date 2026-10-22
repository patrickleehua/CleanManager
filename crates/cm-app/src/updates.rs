//! App update services shared by the CLI and the desktop (parity UPD-01..04, tasks 10.1-10.3).
//!
//! - Settings and the last results live in `<store>/app-updates.json` (this tool's own file).
//! - [`check`] runs detection only when checks are enabled; when they are off it returns the cached results (each with
//!   its original check time) or `not_checked`, and the channels are never called (UPD-04).
//! - [`plan_updates`] puts the selected package-manager updates into one `update` plan (one `tool_adapter` action per
//!   app, reviewed together, executed item by item) and routes the rest: Store apps to the Store, self-updating apps
//!   to the app, unverifiable ones to the official page. On macOS Homebrew updates are the plan's actions (Homebrew
//!   verifies its downloads); Mac App Store apps go to the App Store and Sparkle/Electron apps to the app itself.
//! - macOS ([`MacChannels`]): `brew outdated` once per check (only when a Homebrew package is installed), each
//!   https `SUFeedURL` appcast fetched once (GET of exactly that URL: no app list, no system profile), file probes
//!   for everything else (`cm_core::updates::mac`).
//! - [`run_update`] is the executor arm for `update` plans: [`cm_core::updates::execute`] mapped to an action result.

use std::path::Path;

use cm_core::apps::AppInstance;
use cm_core::path::RawPath;
use cm_core::plan::v2::{ActionMeta, BuildError, ExecutableRef, HeaderInput, NO_RULES_DIGEST, PlanAction, PlanKind, PlanV2, ToolAdapterAction, build};
use cm_core::updates::{
    ADAPTER_VERSION, Channel, DetectInput, Route, ScoopApp, SelfUpdaterProbe, UpdateBackend, UpdateCheck, UpdateOutcome, UpdateState, UpdateTarget, detect,
    parse_winget_list, store_updates_uri, upgrade_argv, verified_by_channel, when_disabled,
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
    let mut s: UpdateStore = std::fs::read_to_string(store.join(UPDATES_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    // Results saved by an older build keep UPD-01 too (e.g. a Microsoft Store row once stored as `up_to_date`).
    s.results = std::mem::take(&mut s.results).into_iter().map(UpdateCheck::enforced).collect();
    s
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
    /// A platform whose channels are decided per app outside winget/Scoop (macOS: Homebrew, Sparkle, App Store,
    /// Electron) returns the whole detection pass here. `None` (the default) runs the winget/Scoop pass.
    fn platform_detect(&self, _apps: &[AppInstance], _checked_at: &str) -> Option<Vec<UpdateCheck>> {
        None
    }
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
                w["official_url"] = store_updates_uri(c.channel).into();
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
    let items = match ch.platform_detect(apps, now_utc) {
        Some(items) => items,
        None => {
            let winget = Some(ch.winget_list().and_then(|t| parse_winget_list(&t)));
            let scoop = ch.scoop_apps();
            let probe = |a: &AppInstance| ch.self_updater(a);
            detect(&DetectInput { apps, winget, scoop: &scoop, self_updater: &probe, checked_at: now_utc })
        }
    };
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
    /// macOS: Homebrew's `brew` (passed the user-owned trust policy when the plan was made).
    pub homebrew: Option<UpdateTool>,
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
            let why = check.reason.clone().unwrap_or_else(|| format!("no update is available ({})", check.state.as_str()));
            let url = (check.route == Route::OpenStore).then(|| store_updates_uri(check.channel).to_owned()).or(check.official_url.clone());
            routed.push(route(if check.route == Route::PackageManager { Route::None } else { check.route }, why, url));
            continue;
        }
        let tool = match check.channel {
            Some(Channel::Winget) => i.winget.as_ref(),
            Some(Channel::Scoop) => i.scoop.as_ref(),
            Some(Channel::MicrosoftStore) => {
                routed.push(route(Route::OpenStore, "Microsoft Store updates its own apps".into(), Some(store_updates_uri(check.channel).into())));
                continue;
            }
            Some(Channel::Homebrew) if check.route == Route::PackageManager => i.homebrew.as_ref(),
            Some(Channel::Homebrew) => {
                routed.push(route(Route::None, check.reason.clone().unwrap_or_else(|| "Homebrew will not upgrade this package".into()), None));
                continue;
            }
            Some(Channel::MacAppStore) => {
                routed.push(route(Route::OpenStore, "The Mac App Store updates its own apps".into(), Some(store_updates_uri(check.channel).into())));
                continue;
            }
            Some(Channel::Sparkle | Channel::Electron) => {
                let why = check.reason.clone().unwrap_or_else(|| "the app installs its own updates; open it and use its update command".into());
                routed.push(route(Route::OpenApp, why, check.official_url.clone()));
                continue;
            }
            _ => None,
        };
        let (Some(tool), Some(package_id), Some(to_version)) = (tool, check.channel_id.clone(), check.available_version.clone()) else {
            routed.push(route(Route::OpenOfficialPage, "the package manager for this app was not found on this computer".into(), check.official_url.clone()));
            continue;
        };
        let signer = (i.installed_signer)(app);
        let channel = check.channel.expect("matched above");
        // Homebrew verifies its own downloads (D8); the claimed signer is recorded and compared after the upgrade.
        if signer.is_none() && !verified_by_channel(channel) {
            routed.push(route(
                Route::OpenOfficialPage,
                "the installed app has no verifiable signer, so a downloaded package cannot be checked against it".into(),
                check.official_url.clone(),
            ));
            continue;
        }
        let target = UpdateTarget {
            app_instance_id: id.clone(),
            name: app.name.clone(),
            channel,
            package_id,
            from_version: check.current_version.clone(),
            to_version,
            installed_signer: signer,
            install_location: app.install_location.as_ref().map(RawPath::display),
            official_url: check.official_url.clone(),
        };
        let Some((adapter_id, argv)) = upgrade_argv(&target) else {
            routed.push(route(Route::OpenOfficialPage, "the package identifier cannot be passed to the package manager safely".into(), check.official_url.clone()));
            continue;
        };
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
        Some(t) if verified_by_channel(t.channel) => format!(
            "{}: {} -> {} via {} ({}), Homebrew verifies the download's checksum{}",
            t.name,
            t.from_version.as_deref().unwrap_or("?"),
            t.to_version,
            t.channel.label(),
            a.argv.join(" "),
            t.installed_signer.as_deref().map(|s| format!("; the app must still be signed by {s} afterwards")).unwrap_or_default()
        ),
        Some(t) => format!(
            "{}: {} -> {} via {} ({}), package signer must be {}",
            t.name,
            t.from_version.as_deref().unwrap_or("?"),
            t.to_version,
            t.channel.label(),
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

/// macOS channels: `brew outdated`, Sparkle appcasts (https, user-triggered only) and file probes.
#[cfg(target_os = "macos")]
pub struct MacChannels {
    pub env: cm_platform::macos::apps::AppsEnv,
    pub brew: Option<std::path::PathBuf>,
}

#[cfg(target_os = "macos")]
impl MacChannels {
    pub fn new(env: cm_platform::macos::apps::AppsEnv) -> MacChannels {
        let brew = cm_platform::macos::updates::brew_exe(&env);
        MacChannels { env, brew }
    }

    /// The appcast bytes: an https GET of exactly the feed URL; while the inventory is redirected (tests) only the
    /// fixture directory is read and the network is never used.
    fn appcast(&self, url: &str) -> Result<Vec<u8>, String> {
        use cm_core::updates::mac::APPCAST_MAX;
        if self.env.is_redirected() {
            let dir = std::env::var_os(cm_platform::macos::updates::TEST_APPCAST_DIR_VAR)
                .ok_or("network access is off while CM_UNINSTALL_TEST_ROOT redirects the inventory")?;
            let name: String = url.trim_start_matches("https://").chars().map(|c| if c.is_ascii_alphanumeric() || ".-".contains(c) { c } else { '_' }).collect();
            return std::fs::read(std::path::Path::new(&dir).join(name)).map_err(|e| format!("no fixture appcast for {url}: {e}"));
        }
        use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
        let tls = TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier).build();
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(30)))
            .timeout_connect(Some(std::time::Duration::from_secs(10)))
            .https_only(true)
            .max_redirects(5)
            .tls_config(tls)
            .user_agent(format!("CleanManager/{} (update check)", cm_core::ENGINE_VERSION))
            .build()
            .into();
        let shown = url.split(['?', '#']).next().unwrap_or_default();
        let resp = agent.get(url).call().map_err(|e| {
            cm_core::debug_trace!("http", "GET {shown} failed: {e}");
            format!("{e}")
        })?;
        cm_core::debug_trace!("http", "GET {shown} status={}", resp.status().as_u16());
        resp.into_body().with_config().limit(APPCAST_MAX).read_to_vec().map_err(|e| format!("{e}"))
    }
}

#[cfg(target_os = "macos")]
impl UpdateChannels for MacChannels {
    fn winget_list(&self) -> Result<String, String> {
        Err("winget is a Windows package manager".into())
    }

    fn scoop_apps(&self) -> Vec<ScoopApp> {
        Vec::new()
    }

    fn self_updater(&self, _app: &AppInstance) -> Option<SelfUpdaterProbe> {
        None
    }

    fn platform_detect(&self, apps: &[AppInstance], checked_at: &str) -> Option<Vec<UpdateCheck>> {
        use cm_core::updates::mac::{MacDetectInput, mac_detect, parse_brew_outdated};
        use cm_platform::macos::updates as mu;
        // Homebrew is asked once, and only when a Homebrew package is installed.
        let homebrew = apps.iter().any(|a| matches!(a.source, cm_core::apps::AppSource::Homebrew { .. })).then(|| match &self.brew {
            Some(exe) => mu::brew_outdated(exe, &self.env).and_then(|t| parse_brew_outdated(&t)),
            None => Err(format!("Homebrew's brew was not found ({})", mu::BREW_PATHS.join(", "))),
        });
        let probe = |a: &AppInstance| mu::probe(a, &self.env);
        let fetch = |url: &str| self.appcast(url).and_then(|b| mu::parse_appcast(&b));
        let os = mu::os_version();
        Some(mac_detect(&MacDetectInput { apps, homebrew, probe: &probe, fetch_appcast: &fetch, os_version: os.as_deref(), checked_at }))
    }
}

/// Installed apps for update checks on macOS: the bundle inventory plus formulae installed on request. Local reads only.
#[cfg(target_os = "macos")]
pub fn mac_apps(env: &cm_platform::macos::apps::AppsEnv) -> Vec<AppInstance> {
    let mut apps = cm_platform::macos::apps::inventory(env).0;
    apps.extend(cm_platform::macos::updates::formula_instances(env));
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.instance_id.cmp(&b.instance_id)));
    apps
}

/// Homebrew's `brew` to bind into update actions (path + SHA-256 now), when it passes the user-owned trust policy.
#[cfg(target_os = "macos")]
pub fn mac_brew_tool(env: &cm_platform::macos::apps::AppsEnv) -> Option<UpdateTool> {
    use std::os::unix::ffi::OsStrExt;
    let exe = cm_platform::macos::updates::brew_trusted(&cm_platform::macos::updates::brew_exe(env)?).ok()?;
    let sha256 = cm_core::adapter::sha256_hex(std::fs::File::open(&exe).ok()?).ok()?;
    Some(UpdateTool { exe: RawPath::unix(exe.as_os_str().as_bytes().to_vec()).ok()?, sha256 })
}

#[cfg(test)]
mod tests;
