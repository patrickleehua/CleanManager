//! Application update detection and verified execution model (parity UPD-01..04, tasks 10.1-10.3).
//!
//! - UPD-01: every result names its channel, current and available version and check time. An app with no channel,
//!   or whose channel could not answer, is `cannot_check` with a reason; `up_to_date` exists only with a channel that
//!   answered ([`UpdateCheck::new`] enforces it).
//! - UPD-02: updates run only as fixed package-manager argv (`tool_adapter`) after the downloaded package's signer was
//!   checked against the installed app's signer ([`publisher_consistent`]); a mismatch or an unverifiable package
//!   refuses the update before anything is installed and points to the official page instead. Store apps are routed
//!   to the Store; self-updating (Squirrel/Electron) apps to the app itself.
//! - UPD-03: a running app is not updated (the user is asked to close it and apply again); batch results are per item.
//! - UPD-04: detection only runs when checks are enabled; when off, nothing is executed and the last results (with
//!   their time) or `not_checked` are shown. Checks send only what the channel needs (the package manager's own query).

use serde::{Deserialize, Serialize};

use crate::apps::{AppInstance, AppSource};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Winget,
    MicrosoftStore,
    Scoop,
    /// Detected self-updater (Squirrel/Electron); never checked or executed by CleanManager.
    SelfUpdater,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateState {
    UpdateAvailable,
    UpToDate,
    CannotCheck,
    NotChecked,
}

/// How an available update can be taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    /// A reviewed `update` plan with a package-manager `tool_adapter` action.
    PackageManager,
    /// Open the Microsoft Store downloads and updates page.
    OpenStore,
    /// Open the app itself (it updates through its own verified mechanism).
    OpenApp,
    /// Open the publisher's official page.
    OpenOfficialPage,
    None,
}

pub const STORE_UPDATES_URI: &str = "ms-windows-store://downloadsandupdates";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateCheck {
    pub app_instance_id: String,
    pub name: String,
    pub current_version: Option<String>,
    pub available_version: Option<String>,
    pub channel: Option<Channel>,
    /// Identifier inside the channel (winget package ID, Scoop app name, Store product).
    pub channel_id: Option<String>,
    pub state: UpdateState,
    pub reason: Option<String>,
    pub checked_at: Option<String>,
    pub route: Route,
    pub official_url: Option<String>,
}

impl UpdateCheck {
    /// Enforces UPD-01: `up_to_date`/`update_available` need a channel and a check time; otherwise `cannot_check`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        app: &AppInstance, channel: Option<Channel>, channel_id: Option<String>, available: Option<String>, state: UpdateState, reason: Option<String>,
        checked_at: Option<String>, route: Route,
    ) -> Self {
        let mut c = UpdateCheck {
            app_instance_id: app.instance_id.clone(),
            name: app.name.clone(),
            current_version: app.version.clone(),
            available_version: available,
            channel,
            channel_id,
            state,
            reason,
            checked_at,
            route,
            official_url: None,
        };
        let answered = matches!(c.state, UpdateState::UpToDate | UpdateState::UpdateAvailable);
        if answered && (c.channel.is_none() || c.checked_at.is_none() || c.channel == Some(Channel::SelfUpdater)) {
            c.state = UpdateState::CannotCheck;
            c.available_version = None;
            c.reason = Some(NO_CHANNEL.into());
            c.route = Route::None;
        }
        if c.state != UpdateState::UpdateAvailable && c.route == Route::PackageManager {
            c.route = Route::None;
        }
        c
    }

    pub fn to_wire(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("update check serializes")
    }
}

pub const NO_CHANNEL: &str = "No update channel: not installed through winget, Microsoft Store or Scoop, and no verifiable update feed";
pub const SELF_UPDATER: &str =
    "Updates itself (Squirrel/Electron update feed); CleanManager cannot verify that feed, so open the app and use its own update command";
pub const STORE_SELF: &str = "Microsoft Store checks its own apps; open the Store's Downloads and updates page";
pub const CHECKS_OFF: &str = "Update checks are turned off; nothing was sent";

/// Display width of one character in a console table (East Asian wide/fullwidth = 2).
pub fn char_width(c: char) -> usize {
    let u = c as u32;
    let wide = matches!(u,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x20000..=0x3FFFD);
    if wide { 2 } else { 1 }
}

/// The text of `line` between display columns `[from, to)` (to = `None`: end of line), trimmed.
fn columns(line: &str, from: usize, to: Option<usize>) -> String {
    let mut col = 0;
    let mut out = String::new();
    for c in line.chars() {
        let w = char_width(c);
        if col >= from && to.is_none_or(|t| col < t) {
            out.push(c);
        }
        col += w;
    }
    out.trim().to_owned()
}

/// One row of `winget list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WingetRow {
    pub name: String,
    pub id: String,
    pub version: String,
    pub available: Option<String>,
    /// `winget`, `msstore`, or `None` for installed records winget could not correlate with a source.
    pub source: Option<String>,
}

/// Parse `winget list` table output (any UI language). Column starts come from the header line above the dashed
/// rule; widths are display columns, so CJK names line up. Progress spinners (`\r`) and footers are ignored.
pub fn parse_winget_list(text: &str) -> Result<Vec<WingetRow>, String> {
    let lines: Vec<String> = text.lines().map(|l| l.rsplit('\r').next().unwrap_or(l).to_owned()).collect();
    let rule = lines.iter().position(|l| l.trim().len() >= 10 && l.trim().chars().all(|c| c == '-')).ok_or("winget output has no table header")?;
    let header = lines.get(rule.wrapping_sub(1)).ok_or("winget output has no table header")?;
    let mut starts = Vec::new();
    let (mut col, mut prev_space) = (0usize, true);
    for c in header.chars() {
        if !c.is_whitespace() && prev_space {
            starts.push(col);
        }
        prev_space = c.is_whitespace();
        col += char_width(c);
    }
    if starts.len() < 3 {
        return Err(format!("unexpected winget header: {}", header.trim()));
    }
    // Name, Id, Version, [Available], [Source]: Available/Source are the 4th/5th columns when present.
    let mut rows = Vec::new();
    for line in &lines[rule + 1..] {
        if line.trim().is_empty() {
            continue;
        }
        let cell = |i: usize| columns(line, starts[i], starts.get(i + 1).copied());
        let (name, id, version) = (cell(0), cell(1), cell(2));
        // Footer lines ("3 upgrades available.") have no ID column content and no version.
        if id.is_empty() || version.is_empty() || id.contains(' ') {
            continue;
        }
        let (available, source) = match starts.len() {
            n if n >= 5 => (cell(3), cell(4)),
            4 => {
                let last = cell(3);
                if last == "winget" || last == "msstore" { (String::new(), last) } else { (last, String::new()) }
            }
            _ => (String::new(), String::new()),
        };
        rows.push(WingetRow {
            name,
            id,
            version,
            available: (!available.is_empty()).then_some(available),
            source: (!source.is_empty()).then_some(source),
        });
    }
    Ok(rows)
}

/// A Scoop app read from the local Scoop directory (installed manifest and the bucket's current manifest).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoopApp {
    pub name: String,
    pub installed_version: String,
    pub bucket: Option<String>,
    /// Version in the local bucket copy; `None` when the bucket manifest is missing.
    pub bucket_version: Option<String>,
    /// When the local bucket copy was last updated (UTC); the check is only as fresh as this.
    pub bucket_updated_at: Option<String>,
    pub homepage: Option<String>,
    pub install_dir: String,
}

/// `version` field of a Scoop manifest.
pub fn scoop_manifest_version(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json.trim_start_matches('\u{feff}')).ok()?;
    v["version"].as_str().map(str::to_owned)
}

pub fn scoop_manifest_homepage(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json.trim_start_matches('\u{feff}')).ok()?;
    v["homepage"].as_str().filter(|h| h.starts_with("https://") || h.starts_with("http://")).map(str::to_owned)
}

/// `bucket` field of Scoop's `install.json`.
pub fn scoop_install_bucket(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json.trim_start_matches('\u{feff}')).ok()?;
    v["bucket"].as_str().map(str::to_owned)
}

fn norm_name(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Whether a winget row is the record of this installed instance.
fn winget_row_matches(app: &AppInstance, row: &WingetRow) -> bool {
    match &app.source {
        AppSource::Msix { full_name, .. } if row.id.eq_ignore_ascii_case(&format!("MSIX\\{full_name}")) => return true,
        AppSource::RegistryUninstall { key, .. } if row.id.to_lowercase().ends_with(&format!("\\{}", key.to_lowercase())) && row.id.starts_with("ARP\\") => {
            return true;
        }
        AppSource::Msi { product_code, .. } if row.id.to_lowercase().ends_with(&product_code.to_lowercase()) && row.id.starts_with("ARP\\") => return true,
        _ => {}
    }
    // Correlated rows show the package ID; match by display name (winget truncates long names with an ellipsis) and version.
    let name = row.name.trim_end_matches(['…', '.']);
    let (a, r) = (norm_name(&app.name), norm_name(name));
    let name_ok = !r.is_empty() && (a == r || (row.name.ends_with('…') && a.starts_with(&r)));
    let version_ok = app.version.as_deref().is_none_or(|v| row.version.trim_start_matches("> ").trim_start_matches("< ") == v || row.version == "Unknown");
    name_ok && version_ok
}

/// A self-updating app (Squirrel/Electron) found by the platform probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfUpdaterProbe {
    pub evidence: String,
}

/// Inputs of one detection pass.
pub struct DetectInput<'a> {
    pub apps: &'a [AppInstance],
    /// `None`: winget was not queried (checks off); `Some(Err)`: the query failed (reason shown per app).
    pub winget: Option<Result<Vec<WingetRow>, String>>,
    pub scoop: &'a [ScoopApp],
    pub self_updater: &'a dyn Fn(&AppInstance) -> Option<SelfUpdaterProbe>,
    pub checked_at: &'a str,
}

/// UPD-01: one result per app from the channel answers.
pub fn detect(input: &DetectInput<'_>) -> Vec<UpdateCheck> {
    let mut out = Vec::new();
    for app in input.apps {
        // Scoop apps live in the Scoop directory; the inventory record (if any) is matched by install location.
        let scoop = input.scoop.iter().find(|s| {
            app.install_location.as_ref().is_some_and(|l| {
                let l = l.display().to_lowercase();
                let d = s.install_dir.to_lowercase();
                l == d || l.starts_with(&format!("{d}\\"))
            })
        });
        if let Some(s) = scoop {
            out.push(scoop_check(app, s, input.checked_at));
            continue;
        }
        let row = match &input.winget {
            Some(Ok(rows)) => rows.iter().find(|r| winget_row_matches(app, r)),
            _ => None,
        };
        let mut check = match (row, &input.winget) {
            (Some(r), _) if r.source.as_deref() == Some("winget") || r.source.as_deref() == Some("msstore") => {
                let channel = if r.source.as_deref() == Some("msstore") { Channel::MicrosoftStore } else { Channel::Winget };
                let route = if channel == Channel::MicrosoftStore { Route::OpenStore } else { Route::PackageManager };
                let state = if r.available.is_some() { UpdateState::UpdateAvailable } else { UpdateState::UpToDate };
                UpdateCheck::new(app, Some(channel), Some(r.id.clone()), r.available.clone(), state, None, Some(input.checked_at.into()), route)
            }
            _ if matches!(app.source, AppSource::Msix { .. }) => UpdateCheck::new(
                app,
                Some(Channel::MicrosoftStore),
                None,
                None,
                UpdateState::CannotCheck,
                Some(STORE_SELF.into()),
                Some(input.checked_at.into()),
                Route::OpenStore,
            ),
            (_, Some(Err(e))) => UpdateCheck::new(app, None, None, None, UpdateState::CannotCheck, Some(format!("winget could not be queried: {e}")), None, Route::None),
            _ => UpdateCheck::new(app, None, None, None, UpdateState::CannotCheck, Some(NO_CHANNEL.into()), Some(input.checked_at.into()), Route::None),
        };
        if check.channel.is_none()
            && let Some(p) = (input.self_updater)(app)
        {
            check.channel = Some(Channel::SelfUpdater);
            check.reason = Some(format!("{SELF_UPDATER} ({})", p.evidence));
            check.route = Route::OpenApp;
        }
        if check.state == UpdateState::CannotCheck && check.channel.is_none() {
            check.checked_at = Some(input.checked_at.into());
        }
        out.push(check);
    }
    out
}

fn scoop_check(app: &AppInstance, s: &ScoopApp, checked_at: &str) -> UpdateCheck {
    let mut app = app.clone();
    app.version = Some(s.installed_version.clone());
    let mut c = match &s.bucket_version {
        None => UpdateCheck::new(
            &app,
            Some(Channel::Scoop),
            Some(s.name.clone()),
            None,
            UpdateState::CannotCheck,
            Some(format!("The {} bucket has no manifest for {} any more", s.bucket.as_deref().unwrap_or("(unknown)"), s.name)),
            Some(checked_at.into()),
            Route::None,
        ),
        Some(v) if v != &s.installed_version => UpdateCheck::new(
            &app,
            Some(Channel::Scoop),
            Some(s.name.clone()),
            Some(v.clone()),
            UpdateState::UpdateAvailable,
            None,
            Some(checked_at.into()),
            Route::PackageManager,
        ),
        Some(_) => UpdateCheck::new(&app, Some(Channel::Scoop), Some(s.name.clone()), None, UpdateState::UpToDate, None, Some(checked_at.into()), Route::None),
    };
    if let Some(t) = &s.bucket_updated_at {
        c.reason = Some(match c.reason.take() {
            Some(r) => format!("{r}; local bucket copy from {t}"),
            None => format!("Compared with the local bucket copy from {t}"),
        });
    }
    c.official_url = s.homepage.clone();
    c
}

/// Results to show when checks are off (UPD-04): the cached result with its original time, else `not_checked`.
pub fn when_disabled(apps: &[AppInstance], cached: &[UpdateCheck]) -> Vec<UpdateCheck> {
    apps.iter()
        .map(|app| match cached.iter().find(|c| c.app_instance_id == app.instance_id) {
            Some(c) => c.clone(),
            None => UpdateCheck::new(app, None, None, None, UpdateState::NotChecked, Some(CHECKS_OFF.into()), None, Route::None),
        })
        .collect()
}

/// Signer names compared case- and punctuation-insensitively, ignoring legal-form suffixes ("Inc.", "Corporation").
pub fn normalize_publisher(s: &str) -> String {
    const SUFFIXES: &[&str] = &["incorporated", "inc", "corporation", "corp", "co", "ltd", "limited", "llc", "gmbh", "ag", "sa", "bv", "pty", "plc", "kk"];
    let words: Vec<String> = s
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    let mut keep = words.len();
    while keep > 1 && SUFFIXES.contains(&words[keep - 1].as_str()) {
        keep -= 1;
    }
    words[..keep].join(" ")
}

/// UPD-02: the package's verified signer must be the installed app's verified signer.
pub fn publisher_consistent(installed: &str, package: &str) -> bool {
    let (a, b) = (normalize_publisher(installed), normalize_publisher(package));
    !a.is_empty() && a == b
}

/// What an `update` tool action binds, frozen into the plan (its SHA-256 is the action's `preview_digest`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateTarget {
    pub app_instance_id: String,
    pub name: String,
    pub channel: Channel,
    pub package_id: String,
    pub from_version: Option<String>,
    pub to_version: String,
    /// Verified signer of the installed executables at planning time; `None` = unverifiable (never executed).
    pub installed_signer: Option<String>,
    pub install_location: Option<String>,
    pub official_url: Option<String>,
}

impl UpdateTarget {
    /// CM-C14N-v1 digest (the target has only strings, so canonicalization cannot fail).
    pub fn digest(&self) -> String {
        crate::c14n::digest(&serde_json::to_value(self).expect("target serializes")).expect("strings canonicalize")
    }
}

/// Fixed argv of the package-manager steps. Only these shapes are ever executed for updates.
pub mod argv {
    /// Download the exact version's installer for signature verification (nothing is installed).
    pub fn winget_download(id: &str, version: &str, dir: &str) -> Vec<String> {
        [
            "download", "--id", id, "--exact", "--version", version, "--source", "winget", "--download-directory", dir, "--disable-interactivity",
            "--accept-source-agreements", "--accept-package-agreements",
        ]
        .map(str::to_owned)
        .to_vec()
    }

    /// Upgrade to exactly the verified version (winget checks the installer hash from the same manifest).
    pub fn winget_upgrade(id: &str, version: &str) -> Vec<String> {
        [
            "upgrade", "--id", id, "--exact", "--version", version, "--source", "winget", "--silent", "--disable-interactivity",
            "--accept-source-agreements", "--accept-package-agreements",
        ]
        .map(str::to_owned)
        .to_vec()
    }

    pub fn scoop_download(app: &str) -> Vec<String> {
        ["download", app].map(str::to_owned).to_vec()
    }

    pub fn scoop_update(app: &str) -> Vec<String> {
        ["update", app].map(str::to_owned).to_vec()
    }
}

/// Outcome of one external step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub exit_code: Option<i32>,
    pub output: String,
    pub timed_out: bool,
}

/// Platform side of update execution. Every call that could reach the network goes through `run`.
pub trait UpdateBackend {
    /// Run a fixed executable with a fixed argv (no shell), with a timeout.
    fn run(&self, exe: &str, argv: &[String], timeout_seconds: u32) -> Result<ToolOutput, String>;
    fn file_sha256(&self, path: &str) -> Option<String>;
    /// Verified signer of a downloaded package (`None` = unsigned or unverifiable).
    fn signer(&self, path: &str) -> Option<String>;
    /// Downloaded package files for this target after the download step.
    fn downloaded(&self, target: &UpdateTarget, dir: &str) -> Vec<String>;
    /// A fresh empty directory for one download.
    fn scratch_dir(&self, action_id: &str) -> Result<String, String>;
    /// Whether processes of the app are running (`None` = cannot tell).
    fn running(&self, target: &UpdateTarget) -> Option<bool>;
    /// The installed version now (after the upgrade).
    fn installed_version(&self, target: &UpdateTarget) -> Option<String>;
}

pub const WINGET_ADAPTER: &str = "winget.upgrade";
pub const SCOOP_ADAPTER: &str = "scoop.update";
pub const ADAPTER_VERSION: &str = "1";

/// The fixed upgrade argv for a target (what a plan's `tool_adapter` action must carry).
pub fn upgrade_argv(t: &UpdateTarget) -> Option<(&'static str, Vec<String>)> {
    match t.channel {
        Channel::Winget => Some((WINGET_ADAPTER, argv::winget_upgrade(&t.package_id, &t.to_version))),
        Channel::Scoop => Some((SCOOP_ADAPTER, argv::scoop_update(&t.package_id))),
        Channel::MicrosoftStore | Channel::SelfUpdater => None,
    }
}

/// Result of one update action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    Updated { to_version: String },
    /// Not attempted; the app was not touched.
    Skipped { code: &'static str, message: String },
    Failed { code: String, message: String },
    /// The tool ran but the result could not be confirmed.
    Unknown { message: String },
}

fn tail(s: &str, n: usize) -> String {
    let t: Vec<char> = s.trim().chars().collect();
    let start = t.len().saturating_sub(n);
    t[start..].iter().collect::<String>().replace(['\r', '\n'], " ")
}

fn official(t: &UpdateTarget) -> String {
    match &t.official_url {
        Some(u) => format!("; get the update from the official page instead: {u}"),
        None => "; get the update from the publisher's official page instead".into(),
    }
}

/// UPD-02/03: execute one `update` tool action. Order: bound target and argv, tool identity, running app, verified
/// download (signer must equal the installed signer), then the upgrade, then the installed version. Anything that
/// fails before the upgrade leaves the app untouched (`Skipped`).
pub fn execute(b: &dyn UpdateBackend, a: &crate::plan::v2::ToolAdapterAction, note: Option<&str>) -> UpdateOutcome {
    let skip = |code: &'static str, message: String| UpdateOutcome::Skipped { code, message };
    let Some(t) = note.and_then(|n| serde_json::from_str::<UpdateTarget>(n).ok()) else {
        return skip("target_changed", "the update target recorded with the plan is missing".into());
    };
    if t.digest() != a.preview_digest {
        return skip("target_changed", "the update target differs from the reviewed plan".into());
    }
    match upgrade_argv(&t) {
        Some((id, argv)) if id == a.adapter_id && a.adapter_version == ADAPTER_VERSION && argv == a.argv => {}
        _ => return skip("adapter_mismatch", "the action is not the fixed package-manager upgrade for this target".into()),
    }
    let exe = a.executable.path.display();
    if b.file_sha256(&exe).as_deref() != Some(a.executable.sha256.as_str()) {
        return skip("tool_changed", format!("{exe} changed since the plan was made; create a new plan"));
    }
    match b.running(&t) {
        Some(false) => {}
        Some(true) => return skip("app_running", format!("{} is running; close it and apply the plan again (CleanManager does not close apps to update them)", t.name)),
        None => return skip("running_unknown", format!("could not tell whether {} is running; close it and apply the plan again", t.name)),
    }
    let Some(installed_signer) = t.installed_signer.clone() else {
        return skip("unverifiable", format!("the installed {} has no verifiable signer, so a new package cannot be checked against it{}", t.name, official(&t)));
    };
    let dir = match b.scratch_dir(&a.action_id) {
        Ok(d) => d,
        Err(e) => return UpdateOutcome::Failed { code: "download_failed".into(), message: e },
    };
    let download = match t.channel {
        Channel::Winget => argv::winget_download(&t.package_id, &t.to_version, &dir),
        _ => argv::scoop_download(&t.package_id),
    };
    match b.run(&exe, &download, a.timeout_seconds) {
        Ok(o) if o.exit_code == Some(0) && !o.timed_out => {}
        Ok(o) => return skip("download_failed", format!("the package could not be downloaded for verification (exit {:?}): {}", o.exit_code, tail(&o.output, 300))),
        Err(e) => return skip("download_failed", format!("the package could not be downloaded for verification: {e}")),
    }
    let files = b.downloaded(&t, &dir);
    if files.is_empty() {
        return skip("unverifiable", format!("no downloaded package was found to verify{}", official(&t)));
    }
    for f in &files {
        match b.signer(f) {
            None => return skip("unverifiable", format!("the downloaded package is unsigned or its signature does not verify{}", official(&t))),
            Some(s) if !publisher_consistent(&installed_signer, &s) => {
                return skip("signature_mismatch", format!("the package is signed by \"{s}\" but the installed app by \"{installed_signer}\"; update refused, the app was not changed"));
            }
            Some(_) => {}
        }
    }
    match b.run(&exe, &a.argv, a.timeout_seconds) {
        Ok(o) if o.timed_out => return UpdateOutcome::Unknown { message: format!("the package manager did not finish within {} s", a.timeout_seconds) },
        Ok(o) if o.exit_code != Some(0) => {
            let code = o.exit_code.map_or("tool_failed".to_owned(), |c| format!("tool_exit_{c}"));
            return UpdateOutcome::Failed { code, message: format!("the package manager reported a failure: {}", tail(&o.output, 300)) };
        }
        Ok(_) => {}
        Err(e) => return UpdateOutcome::Failed { code: "tool_failed".into(), message: e },
    }
    match b.installed_version(&t) {
        Some(v) if v == t.to_version => UpdateOutcome::Updated { to_version: v },
        Some(v) => UpdateOutcome::Unknown { message: format!("the package manager reported success but the installed version is {v}, not {}", t.to_version) },
        None => UpdateOutcome::Unknown { message: "the installed version could not be read after the update".into() },
    }
}

#[cfg(test)]
mod tests;
