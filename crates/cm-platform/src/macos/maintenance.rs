//! macOS maintenance tasks, first and second batch (parity 14.3/14.5, MNT-01..05, D10; catalog in `cm_core::maintenance`).
//!
//! Each task is a fixed implementation: a read-only probe, then one action. Tools run through [`super::cmd::Tool`]
//! (absolute, root-owned binaries, fixed argument vectors, no shell, a timeout and output caps).
//!
//! **No privilege path in this build** (docs/platforms/macos.md §11): there is no privileged helper, no `osascript`
//! with administrator privileges and no `AuthorizationExecuteWithPrivileges`. Administrator tasks (icon services cache,
//! DNS flush — `killall -HUP mDNSResponder` needs root —, `diskutil verifyVolume`, Spotlight rebuild) are listed and
//! planned, and every run records them `skipped` / `elevation_unavailable`; [`MacTasks::execute`] never runs them.
//!
//! - `macos.quicklook-reset`: `/usr/bin/qlmanage -r cache` (the user's thumbnail cache).
//! - `macos.spotlight-status`: `/usr/bin/mdutil -s /`, read-only; indexing off or unknown is `attention`.
//! - `macos.broken-preferences`: every `*.plist` directly in `~/Library/Preferences` and its `ByHost` folder is parsed
//!   with CFPropertyList; files that do not parse are listed (`attention`). Nothing is deleted, moved or rewritten.
//! - `macos.stale-saved-state`: `~/Library/Saved Application State/<bundle id>.savedState` whose bundle id
//!   LaunchServices cannot resolve to an installed app (`-[NSWorkspace URLForApplicationWithBundleIdentifier:]`) are
//!   listed (`attention`); nothing is deleted.
//! - `macos.launch-agents-report`: `~/Library/LaunchAgents/*.plist` whose `Program` / `ProgramArguments[0]` is an
//!   absolute path that no longer exists, or that do not parse, are listed (`attention`); nothing is changed. (A minimal
//!   scan of its own; the startup inventory reports the same jobs with more detail.)
//! - `macos.network-dsstore-off` (14.5): a setting task, planned as a `settings_change` action and run by the
//!   maintenance runner through [`MacSettings`] (CFPreferences `com.apple.desktopservices` / `DSDontWriteNetworkStores`).
//!
//! Test redirection: `CM_RESOLVE_HOME` moves the home folder the report tasks read (as for the catalog resolver);
//! `CM_MAINTENANCE_TEST_PREFS_DIR` makes [`MacSettings`] use the absolute-path preference domain
//! `<dir>/com.apple.desktopservices` instead of the user's real domain.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cm_core::capability::CapabilityState;
use cm_core::maintenance::settings::{ABSENT, SettingSpec, SettingsActions};
use cm_core::maintenance::{Execution, Probe, TaskBackend, TaskOutcome, TaskSpec, ids};

use super::cmd::{Tool, ToolOutput, trusted_program};
use super::ffi::cf::{self, Plist};
use super::ffi::objc::{AutoreleasePool, Id, Sel, class, ns_string, sel};

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[(
    "maintenance.run",
    CapabilityState::Unverified,
    "macOS maintenance catalog implemented: Quick Look reset, Spotlight status, broken-preference, stale saved-state and stale LaunchAgent reports (report only), and the reversible network .DS_Store setting (prior value backed up, restorable with `optimize restore`). Administrator tasks (icon cache, DNS flush, disk verify, Spotlight rebuild) are always skipped `elevation_unavailable`: no privilege path exists on macOS in this build. Exercised only on a hosted CI runner, not on a reference Mac; gate G3 not passed",
)];

pub const QLMANAGE: &str = "/usr/bin/qlmanage";
pub const MDUTIL: &str = "/usr/bin/mdutil";
pub const DSCACHEUTIL: &str = "/usr/bin/dscacheutil";
pub const KILLALL: &str = "/usr/bin/killall";
pub const DISKUTIL: &str = "/usr/sbin/diskutil";
/// The system icon services store the icon cache task would rebuild.
pub const ICON_STORE: &str = "/Library/Caches/com.apple.iconservices.store";

/// Why administrator tasks never run on macOS in this build.
pub const NO_PRIVILEGE_PATH: &str = "this build has no privilege path on macOS (no privileged helper; never osascript or AuthorizationExecuteWithPrivileges)";
const ADMIN_DETAIL: &str = "needs administrator authorization, which this build cannot obtain on macOS: it is listed but skipped when run";

/// Files looked at per report task; more are counted, not read.
const MAX_FILES: usize = 20_000;
/// A preference or job file larger than this is not read (no property list of a preference domain is this large).
const MAX_PLIST_BYTES: u64 = 32 << 20;
/// Names shown in one result; the rest is counted.
const SHOWN: usize = 10;

/// The fixed macOS task implementations.
pub struct MacTasks {
    /// The home folder the report tasks read (`CM_RESOLVE_HOME` in fixtures).
    pub home: Option<PathBuf>,
    pub settings: MacSettings,
}

impl MacTasks {
    pub fn from_process() -> MacTasks {
        MacTasks { home: super::purge::home(), settings: MacSettings::from_process() }
    }

    fn home_dir(&self, rel: &str) -> Option<PathBuf> {
        self.home.as_ref().map(|h| h.join(rel))
    }
}

impl TaskBackend for MacTasks {
    fn probe(&self, t: &TaskSpec) -> Probe {
        match t.id {
            ids::MAC_QUICKLOOK => tools_probe(&[QLMANAGE], "the Quick Look thumbnail cache can be reset"),
            ids::MAC_ICON_CACHE => icon_probe(),
            ids::MAC_DNS_FLUSH => tools_probe(&[DSCACHEUTIL, KILLALL], ADMIN_DETAIL),
            ids::MAC_SPOTLIGHT_STATUS => tools_probe(&[MDUTIL], "read-only status of the startup volume's index"),
            ids::MAC_BROKEN_PREFS => folder_probe(self.home_dir("Library/Preferences"), "preferences"),
            ids::MAC_SAVED_STATE => folder_probe(self.home_dir("Library/Saved Application State"), "saved application state"),
            ids::MAC_DISK_VERIFY => tools_probe(&[DISKUTIL], ADMIN_DETAIL),
            ids::MAC_LAUNCH_AGENTS => folder_probe(self.home_dir("Library/LaunchAgents"), "LaunchAgents"),
            ids::MAC_NETWORK_DSSTORE => setting_probe(&self.settings, t.id),
            ids::MAC_SPOTLIGHT_REBUILD => tools_probe(&[MDUTIL], "rebuilds the index of the volume chosen when planning; needs administrator authorization, which this build cannot obtain on macOS: it is listed but skipped when run"),
            other => Probe::Unavailable { code: "task_unknown".into(), reason: format!("no macOS implementation of {other}") },
        }
    }

    fn execute(&self, t: &TaskSpec) -> Execution {
        if t.requires_admin {
            return Execution::new(TaskOutcome::Skipped, "elevation_unavailable", format!("needs administrator authorization; {NO_PRIVILEGE_PATH}; not run"));
        }
        let timeout = Duration::from_secs(u64::from(t.timeout_seconds));
        match t.id {
            ids::MAC_QUICKLOOK => quicklook_reset(timeout),
            ids::MAC_SPOTLIGHT_STATUS => spotlight_status(timeout),
            ids::MAC_BROKEN_PREFS => match self.home_dir("Library/Preferences") {
                Some(d) => broken_preferences(&d),
                None => Execution::new(TaskOutcome::Skipped, "state_unknown", "the home folder is not known"),
            },
            ids::MAC_SAVED_STATE => match self.home_dir("Library/Saved Application State") {
                Some(d) => stale_saved_state(&d, &app_installed),
                None => Execution::new(TaskOutcome::Skipped, "state_unknown", "the home folder is not known"),
            },
            ids::MAC_LAUNCH_AGENTS => match self.home_dir("Library/LaunchAgents") {
                Some(d) => launch_agents_report(&d),
                None => Execution::new(TaskOutcome::Skipped, "state_unknown", "the home folder is not known"),
            },
            ids::MAC_NETWORK_DSSTORE => Execution::new(TaskOutcome::Skipped, "planned_as_setting", "this task changes a setting; its plan carries a settings_change action"),
            other => Execution::new(TaskOutcome::Unavailable, "task_unknown", format!("no macOS implementation of {other}")),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Probes

fn tools_probe(tools: &[&str], detail: &str) -> Probe {
    for t in tools {
        if let Err(e) = trusted_program(Path::new(t)) {
            return Probe::Unavailable { code: "tool_missing".into(), reason: format!("{t} is not available as a system-owned tool: {e}") };
        }
    }
    Probe::Applicable { detail: detail.into() }
}

fn icon_probe() -> Probe {
    match std::fs::symlink_metadata(ICON_STORE) {
        Ok(m) if m.is_dir() => Probe::Applicable { detail: format!("{ICON_STORE} exists; {ADMIN_DETAIL}") },
        Ok(_) => Probe::Unknown { reason: format!("{ICON_STORE} is not a folder") },
        Err(e) if e.kind() == ErrorKind::NotFound => Probe::NotNeeded { code: "no_icon_store".into(), summary: "there is no icon services store to rebuild; it is created on demand".into() },
        Err(e) => Probe::Unknown { reason: format!("{ICON_STORE} cannot be checked: {e}") },
    }
}

fn folder_probe(dir: Option<PathBuf>, what: &str) -> Probe {
    let Some(dir) = dir else { return Probe::Unknown { reason: "the home folder is not known".into() } };
    match std::fs::symlink_metadata(&dir) {
        Ok(m) if m.is_dir() => Probe::Applicable { detail: format!("reads {} (report only)", dir.display()) },
        Ok(_) => Probe::Unknown { reason: format!("{} is not a plain folder; not read", dir.display()) },
        Err(e) if e.kind() == ErrorKind::NotFound => Probe::NotApplicable { code: "no_folder".into(), reason: format!("there is no {what} folder ({})", dir.display()) },
        Err(e) => Probe::Unknown { reason: format!("{} cannot be read: {e}", dir.display()) },
    }
}

fn setting_probe(s: &MacSettings, task_id: &str) -> Probe {
    let Some(spec) = cm_core::maintenance::settings::for_task(task_id) else { return Probe::Unavailable { code: "task_unknown".into(), reason: format!("{task_id} is not a setting task") } };
    match s.read(spec) {
        Ok(v) if v == spec.target => Probe::NotNeeded { code: "already_set".into(), summary: format!("{} is already {v}", spec.key) },
        Ok(v) if v == "false" || v == ABSENT => {
            Probe::Applicable { detail: format!("{} is {v} now; the prior value is backed up before it is set to {} and can be restored", spec.key, spec.target) }
        }
        Ok(v) => Probe::Unknown { reason: format!("{} has a value of another type ({v}); it is not changed", spec.key) },
        Err(e) => Probe::Unknown { reason: e },
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Tools

fn tool_failure(name: &str, out: &ToolOutput) -> String {
    if out.timed_out {
        return format!("{name} did not finish in time and was ended");
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let first = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    match (out.exit_code, out.signal) {
        (Some(c), _) => format!("{name} exited with {c}{}", if first.is_empty() { String::new() } else { format!(": {first}") }),
        (None, Some(s)) => format!("{name} ended by signal {s}"),
        _ => format!("{name} did not finish"),
    }
}

fn quicklook_reset(timeout: Duration) -> Execution {
    match Tool::new(QLMANAGE).args(["-r", "cache"]).timeout(timeout).output_caps(64 << 10, 64 << 10).run() {
        Ok(out) if out.success() => Execution::new(TaskOutcome::Applied, "reset", "Quick Look thumbnail cache reset; thumbnails are generated again on next view"),
        Ok(out) => Execution::new(TaskOutcome::Failed, if out.timed_out { "timed_out" } else { "tool_failed" }, tool_failure("qlmanage", &out)),
        Err(e) => Execution::new(TaskOutcome::Unavailable, "tool_missing", e.to_string()),
    }
}

/// What `mdutil -s` says about one volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotlightState {
    Enabled,
    Disabled,
    /// "Spotlight server is disabled."
    ServerDisabled,
    /// "Error: unknown indexing state."
    Unknown,
}

/// Parse `mdutil -s` output (stdout and stderr together); `None` when nothing recognizable is in it.
pub fn parse_mdutil_status(text: &str) -> Option<SpotlightState> {
    let t = text.to_ascii_lowercase();
    if t.contains("spotlight server is disabled") {
        Some(SpotlightState::ServerDisabled)
    } else if t.contains("indexing enabled") {
        Some(SpotlightState::Enabled)
    } else if t.contains("indexing disabled") || t.contains("indexing and searching disabled") {
        Some(SpotlightState::Disabled)
    } else if t.contains("unknown indexing state") {
        Some(SpotlightState::Unknown)
    } else {
        None
    }
}

fn spotlight_status(timeout: Duration) -> Execution {
    let out = match Tool::new(MDUTIL).args(["-s", "/"]).timeout(timeout).output_caps(64 << 10, 64 << 10).run() {
        Ok(o) => o,
        Err(e) => return Execution::new(TaskOutcome::Unavailable, "tool_missing", e.to_string()),
    };
    if out.timed_out {
        return Execution::new(TaskOutcome::Failed, "timed_out", tool_failure("mdutil", &out));
    }
    let text = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    match parse_mdutil_status(&text) {
        Some(SpotlightState::Enabled) => Execution::new(TaskOutcome::Unchanged, "indexing_enabled", "Spotlight indexing is on for the startup volume; nothing was changed"),
        Some(SpotlightState::Disabled) => Execution::new(
            TaskOutcome::Attention,
            "indexing_disabled",
            "Spotlight indexing is off for the startup volume, so searches miss files. Nothing was changed; check System Settings > Spotlight (Search Privacy), or turn it on in Terminal with administrator rights (`sudo mdutil -i on /`)",
        ),
        Some(SpotlightState::ServerDisabled) => Execution::new(TaskOutcome::Attention, "spotlight_server_disabled", "the Spotlight server is disabled on this Mac; nothing was changed"),
        Some(SpotlightState::Unknown) => Execution::new(
            TaskOutcome::Attention,
            "indexing_state_unknown",
            "Spotlight reports an unknown indexing state for the startup volume; nothing was changed. If searches stay wrong, rebuild the index (macos.spotlight-rebuild, needs administrator authorization)",
        ),
        None if !out.success() => Execution::new(TaskOutcome::Failed, "tool_failed", tool_failure("mdutil", &out)),
        None => Execution::new(TaskOutcome::Failed, "unrecognized_output", format!("mdutil -s / printed nothing recognizable: {}", text.trim().chars().take(200).collect::<String>())),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Report tasks

fn shown(names: &[String]) -> String {
    let mut s = names.iter().take(SHOWN).cloned().collect::<Vec<_>>().join(", ");
    if names.len() > SHOWN {
        s.push_str(&format!(" and {} more", names.len() - SHOWN));
    }
    s
}

/// Plain entries of `dir` (names sorted), with whether each is a directory and whether it is a symlink.
fn entries(dir: &Path) -> Result<Vec<(String, PathBuf, std::fs::FileType)>, String> {
    let md = std::fs::symlink_metadata(dir).map_err(|e| format!("{} cannot be read: {e}", dir.display()))?;
    if !md.is_dir() {
        return Err(format!("{} is not a plain folder; not read", dir.display()));
    }
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).map_err(|e| format!("{} cannot be listed: {e}", dir.display()))? {
        let e = e.map_err(|e| format!("{} cannot be listed: {e}", dir.display()))?;
        let Ok(ft) = e.file_type() else { continue };
        out.push((e.file_name().to_string_lossy().into_owned(), e.path(), ft));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// What the broken-preference scan found.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PrefsScan {
    pub checked: usize,
    /// `name: why` of files that do not parse as property lists.
    pub broken: Vec<String>,
    /// Files that could not be read (permissions) or were too large to read.
    pub unread: Vec<String>,
    /// More files than [`MAX_FILES`]; the rest was not looked at.
    pub truncated: bool,
}

/// Parse every `*.plist` regular file directly in `dir` and in `dir/ByHost`. Links are not followed.
pub fn scan_preferences(dir: &Path) -> Result<PrefsScan, String> {
    let mut scan = PrefsScan::default();
    let mut folders = vec![(String::new(), dir.to_path_buf())];
    while let Some((prefix, folder)) = folders.pop() {
        let list = match entries(&folder) {
            Ok(l) => l,
            Err(e) if prefix.is_empty() => return Err(e),
            Err(_) => {
                scan.unread.push(format!("{prefix}(folder)"));
                continue;
            }
        };
        for (name, path, ft) in list {
            if prefix.is_empty() && name == "ByHost" && ft.is_dir() {
                folders.push(("ByHost/".into(), path));
                continue;
            }
            if !ft.is_file() || !name.ends_with(".plist") {
                continue;
            }
            if scan.checked >= MAX_FILES {
                scan.truncated = true;
                break;
            }
            scan.checked += 1;
            let label = format!("{prefix}{name}");
            match std::fs::symlink_metadata(&path) {
                Ok(m) if m.len() > MAX_PLIST_BYTES => {
                    scan.unread.push(format!("{label} (too large to read)"));
                    continue;
                }
                Ok(_) => {}
                Err(e) => {
                    scan.unread.push(format!("{label} ({e})"));
                    continue;
                }
            }
            match std::fs::read(&path) {
                Ok(bytes) => {
                    if let Err(e) = cf::plist_from_bytes(&bytes) {
                        let why = if bytes.is_empty() { "empty file".to_owned() } else { e.chars().take(120).collect() };
                        scan.broken.push(format!("{label}: {why}"));
                    }
                }
                Err(e) => scan.unread.push(format!("{label} ({e})")),
            }
        }
    }
    scan.broken.sort();
    scan.unread.sort();
    Ok(scan)
}

pub fn broken_preferences(dir: &Path) -> Execution {
    let scan = match scan_preferences(dir) {
        Ok(s) => s,
        Err(e) => return Execution::new(TaskOutcome::Skipped, "state_unknown", e),
    };
    let unread = if scan.unread.is_empty() { String::new() } else { format!("; {} could not be read: {}", scan.unread.len(), shown(&scan.unread)) };
    let more = if scan.truncated { format!("; only the first {MAX_FILES} files were checked") } else { String::new() };
    if scan.broken.is_empty() {
        return Execution::new(TaskOutcome::Unchanged, "no_broken_preferences", format!("checked {} preference files; all parse{unread}{more}", scan.checked));
    }
    Execution::new(
        TaskOutcome::Attention,
        "broken_preferences",
        format!(
            "{} of {} preference files do not parse: {}{unread}{more}. Nothing was changed; quit the app concerned and move its file to the Trash so the app recreates its defaults",
            scan.broken.len(),
            scan.checked,
            shown(&scan.broken)
        ),
    )
}

/// `<bundle id>.savedState` → the bundle id.
pub fn saved_state_bundle_id(name: &str) -> Option<&str> {
    name.strip_suffix(".savedState").filter(|id| !id.is_empty() && id.len() <= 255 && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')))
}

/// Saved states whose bundle id `installed` does not know; `installed` errors make the whole result unknown.
pub fn stale_saved_state(dir: &Path, installed: &dyn Fn(&str) -> Result<bool, String>) -> Execution {
    let list = match entries(dir) {
        Ok(l) => l,
        Err(e) => return Execution::new(TaskOutcome::Skipped, "state_unknown", e),
    };
    let (mut checked, mut stale, mut other) = (0usize, Vec::new(), 0usize);
    for (name, _, ft) in list.iter().take(MAX_FILES) {
        if !ft.is_dir() {
            continue;
        }
        let Some(id) = saved_state_bundle_id(name) else {
            other += 1;
            continue;
        };
        checked += 1;
        match installed(id) {
            Ok(true) => {}
            Ok(false) => stale.push(id.to_owned()),
            Err(e) => return Execution::new(TaskOutcome::Skipped, "state_unknown", format!("installed apps cannot be looked up ({e}); skipped rather than guessed")),
        }
    }
    let other = if other > 0 { format!("; {other} other entries were not judged") } else { String::new() };
    if stale.is_empty() {
        return Execution::new(TaskOutcome::Unchanged, "no_stale_saved_state", format!("checked {checked} saved app states; every app is installed{other}"));
    }
    Execution::new(
        TaskOutcome::Attention,
        "stale_saved_state",
        format!(
            "{} of {checked} saved app states belong to apps that are not installed: {}{other}. Nothing was removed; they can be moved to the Trash from {} in the Finder",
            stale.len(),
            shown(&stale),
            dir.display()
        ),
    )
}

/// One LaunchAgent that needs attention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentFinding {
    /// The program it starts does not exist (label, program).
    MissingProgram { file: String, label: String, program: String },
    /// The file does not parse as a job definition.
    Unreadable { file: String, why: String },
}

/// The program a job definition starts (`Program`, else `ProgramArguments[0]`).
pub(crate) fn agent_program(job: &Plist) -> Option<&str> {
    job.get("Program").and_then(Plist::as_str).or_else(|| match job.get("ProgramArguments") {
        Some(Plist::Array(a)) => a.first().and_then(Plist::as_str),
        _ => None,
    })
}

/// Jobs in `dir` whose absolute program path is gone, or whose file does not parse. Relative programs, `BundleProgram`
/// jobs and unreadable program paths are not judged.
pub fn scan_launch_agents(dir: &Path) -> Result<(usize, Vec<AgentFinding>), String> {
    let mut findings = Vec::new();
    let mut checked = 0usize;
    for (name, path, ft) in entries(dir)?.into_iter().take(MAX_FILES) {
        if !name.ends_with(".plist") || ft.is_dir() {
            continue;
        }
        checked += 1;
        let bytes = match std::fs::metadata(&path) {
            Ok(m) if m.len() > MAX_PLIST_BYTES => {
                findings.push(AgentFinding::Unreadable { file: name, why: "too large to be a job definition".into() });
                continue;
            }
            Ok(_) => std::fs::read(&path),
            Err(e) => Err(e),
        };
        let job = match bytes {
            Ok(b) => cf::plist_from_bytes(&b).map(|(p, _)| p),
            Err(e) if e.kind() == ErrorKind::NotFound => Err("a link to a file that no longer exists".into()),
            Err(e) => Err(e.to_string()),
        };
        let job = match job {
            Ok(j @ Plist::Dict(_)) => j,
            Ok(_) => {
                findings.push(AgentFinding::Unreadable { file: name, why: "not a dictionary".into() });
                continue;
            }
            Err(why) => {
                findings.push(AgentFinding::Unreadable { file: name, why: why.chars().take(120).collect() });
                continue;
            }
        };
        let label = job.get("Label").and_then(Plist::as_str).unwrap_or("(no label)").to_owned();
        let Some(program) = agent_program(&job).filter(|p| p.starts_with('/')) else { continue };
        if let Err(e) = std::fs::metadata(program)
            && e.kind() == ErrorKind::NotFound
        {
            findings.push(AgentFinding::MissingProgram { file: name, label, program: program.to_owned() });
        }
    }
    Ok((checked, findings))
}

pub fn launch_agents_report(dir: &Path) -> Execution {
    let (checked, findings) = match scan_launch_agents(dir) {
        Ok(r) => r,
        Err(e) => return Execution::new(TaskOutcome::Skipped, "state_unknown", e),
    };
    if findings.is_empty() {
        return Execution::new(TaskOutcome::Unchanged, "no_stale_launch_agents", format!("checked {checked} LaunchAgents; every program they start exists"));
    }
    let names: Vec<String> = findings
        .iter()
        .map(|f| match f {
            AgentFinding::MissingProgram { label, program, .. } => format!("{label} ({program} is gone)"),
            AgentFinding::Unreadable { file, why } => format!("{file} ({why})"),
        })
        .collect();
    Execution::new(
        TaskOutcome::Attention,
        "stale_launch_agents",
        format!(
            "{} of {checked} LaunchAgents start a program that no longer exists or cannot be read: {}. Nothing was changed; review them with `cleanmanager startup list` or remove the file from {}",
            findings.len(),
            shown(&names),
            dir.display()
        ),
    )
}

// ---------------------------------------------------------------------------------------------------------------------
// LaunchServices lookup

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    /// `void objc_msgSend(void);` (`objc/message.h`): cast to the method's exact type before calling.
    fn objc_msgSend();
}

/// `objc_msgSend` as `unsafe extern "C" fn(id, SEL, args...) -> ret` (on arm64 it must be called with the method's
/// exact C type; it is not variadic).
macro_rules! msg_fn {
    (($($arg:ty),*) -> $ret:ty) => {
        // SAFETY: function pointers share one size and representation; the caller sends only selectors whose
        // implementation has exactly this C signature.
        unsafe {
            std::mem::transmute::<unsafe extern "C" fn(), unsafe extern "C" fn(Id, Sel $(, $arg)*) -> $ret>(objc_msgSend as unsafe extern "C" fn())
        }
    };
}

/// Whether LaunchServices knows an installed app with this bundle id
/// (`-[NSWorkspace URLForApplicationWithBundleIdentifier:]` returns a URL).
pub fn app_installed(bundle_id: &str) -> Result<bool, String> {
    let _pool = AutoreleasePool::new();
    let ws_class = class(c"NSWorkspace").ok_or("NSWorkspace is not available")?;
    // `@property(class, readonly, strong) NSWorkspace *sharedWorkspace;`
    let shared = msg_fn!(() -> Id);
    // SAFETY: class receiver; returns the shared (+0) instance.
    let ws = unsafe { shared(ws_class, sel(c"sharedWorkspace")) };
    if ws.is_null() {
        return Err("no shared workspace".into());
    }
    let id = ns_string(bundle_id).ok_or("the bundle id cannot be converted")?;
    // `- (NSURL *)URLForApplicationWithBundleIdentifier:(NSString *)bundleIdentifier;`
    let url_for = msg_fn!((Id) -> Id);
    // SAFETY: live workspace and NSString; the result is nil or an autoreleased NSURL kept alive by `_pool`, only tested.
    let url = unsafe { url_for(ws, sel(c"URLForApplicationWithBundleIdentifier:"), id.as_ptr()) };
    Ok(!url.is_null())
}

// ---------------------------------------------------------------------------------------------------------------------
// Settings (14.5)

/// Reads and writes the catalog's settings through CFPreferences (the current user's domain, or an absolute-path
/// domain under `CM_MAINTENANCE_TEST_PREFS_DIR` in tests).
#[derive(Debug, Clone, Default)]
pub struct MacSettings {
    pub test_dir: Option<PathBuf>,
}

pub const PREFS_TEST_VAR: &str = "CM_MAINTENANCE_TEST_PREFS_DIR";

impl MacSettings {
    pub fn from_process() -> MacSettings {
        MacSettings { test_dir: std::env::var_os(PREFS_TEST_VAR).filter(|v| !v.is_empty()).map(PathBuf::from).filter(|p| p.is_absolute()) }
    }

    fn domain(&self, s: &SettingSpec) -> Result<String, String> {
        match &self.test_dir {
            None => Ok(s.domain.to_owned()),
            Some(d) => d.join(s.domain).to_str().map(str::to_owned).ok_or_else(|| "the test preferences folder is not valid text".into()),
        }
    }
}

fn kind_of(p: &Plist) -> &'static str {
    match p {
        Plist::String(_) => "string",
        Plist::Integer(_) => "integer",
        Plist::Real(_) => "real",
        Plist::Bool(_) => "bool",
        Plist::Date(_) => "date",
        Plist::Data(_) => "data",
        Plist::Array(_) => "array",
        Plist::Dict(_) => "dictionary",
    }
}

impl SettingsActions for MacSettings {
    fn read(&self, s: &SettingSpec) -> Result<String, String> {
        match cf::pref_get(&self.domain(s)?, s.key)? {
            None => Ok(ABSENT.into()),
            Some(Plist::Bool(b)) => Ok(b.to_string()),
            Some(other) => Ok(format!("other:{}", kind_of(&other))),
        }
    }

    fn write(&self, s: &SettingSpec, value: &str) -> Result<(), String> {
        let v = match value {
            "true" => Some(Plist::Bool(true)),
            "false" => Some(Plist::Bool(false)),
            ABSENT => None,
            other => return Err(format!("refusing to write an unrecognized value {other:?}")),
        };
        let domain = self.domain(s)?;
        cf::pref_set(&domain, s.key, v.as_ref())?;
        // Read back from the layer just written (`pref_get` uses CFPreferencesCopyValue, so a key another process wrote
        // is seen as removed once it is): a key still present is an error, never "done".
        if v.is_none() && cf::pref_get(&domain, s.key)?.is_some() {
            return Err(format!("{} could not be removed from {domain}", s.key));
        }
        Ok(())
    }

    fn location(&self, s: &SettingSpec) -> String {
        match &self.test_dir {
            Some(_) => format!("{} {}", self.domain(s).unwrap_or_default(), s.key),
            None => format!("{} {} (current user)", s.domain, s.key),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Volume choice (Spotlight rebuild)

/// `path` must be the mount point of a mounted volume (`statfs` names it as `f_mntonname`).
pub fn check_mount_point(path: &str) -> Result<(), String> {
    let c = std::ffi::CString::new(path).map_err(|_| "the volume path contains a NUL byte".to_owned())?;
    // SAFETY: plain C struct filled by statfs.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: NUL-terminated path and a valid out-pointer.
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return Err(format!("{path} is not a mounted volume: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: statfs NUL-terminates f_mntonname within the array.
    let mnt = unsafe { std::ffi::CStr::from_ptr(st.f_mntonname.as_ptr()) }.to_string_lossy().into_owned();
    if mnt == path.trim_end_matches('/') || (mnt == "/" && path == "/") { Ok(()) } else { Err(format!("{path} is inside the volume mounted at {mnt}; name the mount point")) }
}

#[cfg(test)]
mod tests;
