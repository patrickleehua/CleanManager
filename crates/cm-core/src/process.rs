//! Process list model (parity STS-04, task 12.3).
//!
//! - Facts come from the platform ([`ProcessFacts`]): PID, parent, executable location, creation time, signature,
//!   CPU/memory/disk rates. A value the platform cannot read stays `None` with the field's reason, never 0.
//! - Purpose text comes only from the local knowledge table ([`KNOWN`]), and only when the name, the location and the
//!   signer agree with the entry. Anything else is `identified: false`, with an AI-diagnosis entry hint (a field: this
//!   module sends nothing anywhere).
//! - The status view offers no handling except ending a process, and only through a reviewed `process` plan of exactly
//!   one action ([`end_action`]). Critical system processes, CleanManager itself and security software are never
//!   offered and are refused again at execution ([`protection`]).

use serde::{Deserialize, Serialize};

use crate::path::RawPath;
use crate::plan::v2::ProcessEndAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureState {
    /// Verified embedded Authenticode signature.
    Signed,
    /// Verified through a system security catalog (most in-box Windows binaries).
    CatalogSigned,
    Unsigned,
    /// The file could not be read or verified (access denied, no path).
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    pub state: SignatureState,
    pub publisher: Option<String>,
    pub reason: Option<String>,
}

impl Signature {
    pub fn unknown(reason: impl Into<String>) -> Self {
        Signature { state: SignatureState::Unknown, publisher: None, reason: Some(reason.into()) }
    }

    fn verified(&self) -> bool {
        matches!(self.state, SignatureState::Signed | SignatureState::CatalogSigned)
    }
}

/// What the platform observed for one process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessFacts {
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub name: String,
    pub image_path: Option<RawPath>,
    /// Creation time in the platform's exact form (Windows: FILETIME ticks; macOS: microseconds since the Unix epoch;
    /// decimal string). With the PID it names one process instance, so plans bind it; people read `started_at_utc`.
    pub started_at: Option<String>,
    /// The same creation time as an RFC 3339 UTC timestamp (whole seconds), for people and agents.
    #[serde(default)]
    pub started_at_utc: Option<String>,
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
    pub disk_bytes_per_sec: Option<f64>,
    pub network_bytes_per_sec: Option<f64>,
    pub signature: Signature,
    /// The OS marks the process critical (ending it stops the system).
    pub os_critical: Option<bool>,
    /// Why some fields are unknown (e.g. "access denied: protected process").
    pub notes: Vec<String>,
}

/// Why ending a process is not offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protection {
    /// Session/kernel infrastructure (csrss, wininit, lsass, smss, services, winlogon, System, ...).
    CriticalSystem,
    /// The OS flags the process critical.
    OsCritical,
    /// CleanManager itself.
    CleanManager,
    /// Antivirus / endpoint protection.
    SecuritySoftware,
    /// Runs as another user (macOS): never ended from this user's session.
    OtherUser,
}

impl Protection {
    pub fn reason(self) -> &'static str {
        match self {
            Protection::CriticalSystem if cfg!(target_os = "macos") => "Critical macOS process (launchd, kernel_task, WindowServer, loginwindow): ending it logs you out or stops the system",
            Protection::CriticalSystem => "Critical Windows process: ending it stops or restarts the system",
            Protection::OsCritical => "Windows marks this process critical: ending it stops the system",
            Protection::CleanManager => "This is CleanManager itself",
            Protection::SecuritySoftware => "Security software is never ended from CleanManager; use the product's own console",
            Protection::OtherUser => "This process belongs to another user or the system; CleanManager only ends your own processes",
        }
    }
}

/// Names that are always critical on Windows (lower case). PID 0 and 4 are the idle and System processes.
pub const CRITICAL_NAMES: &[&str] = &[
    "system",
    "system idle process",
    "idle",
    "registry",
    "memory compression",
    "secure system",
    "smss.exe",
    "csrss.exe",
    "wininit.exe",
    "winlogon.exe",
    "services.exe",
    "lsass.exe",
    "lsaiso.exe",
    "fontdrvhost.exe",
    "dwm.exe",
];

/// Security product process names (lower case), in addition to publisher markers of the apps protected class.
pub const SECURITY_NAMES: &[&str] = &[
    "msmpeng.exe",
    "nissrv.exe",
    "mpdefendercoreservice.exe",
    "securityhealthservice.exe",
    "securityhealthsystray.exe",
    "sgrmbroker.exe",
    "mssense.exe",
    "sensecncproxy.exe",
    "csfalconservice.exe",
    "sentinelagent.exe",
    "cb.exe",
    "repmgr.exe",
    "avp.exe",
    "ekrn.exe",
    "bdagent.exe",
    "vsserv.exe",
    "mcshield.exe",
    "ccsvchst.exe",
    "360tray.exe",
    "zhudongfangyu.exe",
    "qqpcrtp.exe",
    "hipsdaemon.exe",
];

/// CleanManager executables (lower case).
pub const SELF_NAMES: &[&str] = &["cleanmanager.exe", "cleanmanager-desktop.exe", "cm-desktop.exe"];

/// Protection decision from facts only. `own_pid` is the caller's process.
pub fn protection(f: &ProcessFacts, own_pid: u32) -> Option<Protection> {
    let name = f.name.to_lowercase();
    if f.pid == own_pid || SELF_NAMES.contains(&name.as_str()) {
        return Some(Protection::CleanManager);
    }
    if f.pid == 0 || f.pid == 4 || CRITICAL_NAMES.contains(&name.as_str()) {
        return Some(Protection::CriticalSystem);
    }
    if f.os_critical == Some(true) {
        return Some(Protection::OsCritical);
    }
    let publisher = f.signature.publisher.as_deref();
    if SECURITY_NAMES.contains(&name.as_str())
        || crate::apps::protected_reason(&f.name, publisher, false) == Some(crate::apps::ProtectedReason::EndpointSecurityOrManagement)
    {
        return Some(Protection::SecuritySoftware);
    }
    None
}

/// One entry of the local knowledge table.
pub struct Known {
    /// Executable name, lower case.
    pub name: &'static str,
    /// Expected directory below `%SystemRoot%` (lower case, `""` = the Windows directory itself); `None` = any location.
    pub system_dir: Option<&'static str>,
    /// Expected signer display name (case-insensitive prefix), when the binary is signed by its vendor.
    pub signer: Option<&'static str>,
    pub purpose: &'static str,
}

const MS: Option<&str> = Some("Microsoft");

const fn k(name: &'static str, system_dir: Option<&'static str>, signer: Option<&'static str>, purpose: &'static str) -> Known {
    Known { name, system_dir, signer, purpose }
}

/// Local process knowledge (Windows). Purpose text is shown only when name, location and signer all agree.
pub const KNOWN: &[Known] = &[
    k("system", None, None, "Windows kernel and drivers; disk and network I/O done by drivers is counted here"),
    k("registry", None, None, "Kernel process holding the registry hives in memory"),
    k("memory compression", None, None, "Holds compressed memory pages so less data goes to the paging file"),
    k("smss.exe", Some("system32"), MS, "Session Manager: starts user sessions at boot"),
    k("csrss.exe", Some("system32"), MS, "Client/Server Runtime: console windows and process/thread bookkeeping for a session"),
    k("wininit.exe", Some("system32"), MS, "Windows start-up process: starts services.exe and lsass.exe"),
    k("winlogon.exe", Some("system32"), MS, "Sign-in, lock screen and Ctrl+Alt+Del handling"),
    k("services.exe", Some("system32"), MS, "Service Control Manager: starts and stops Windows services"),
    k("lsass.exe", Some("system32"), MS, "Local Security Authority: sign-in, credentials and security policy"),
    k("svchost.exe", Some("system32"), MS, "Host for Windows services; its load comes from the services it runs"),
    k("dwm.exe", Some("system32"), MS, "Desktop Window Manager: draws windows, animations and transparency"),
    k("explorer.exe", Some(""), MS, "Windows shell: taskbar, Start, desktop and File Explorer windows"),
    k("fontdrvhost.exe", Some("system32"), MS, "User-mode font driver host"),
    k("sihost.exe", Some("system32"), MS, "Shell infrastructure host: notifications, Action Center and some shell UI"),
    k("taskhostw.exe", Some("system32"), MS, "Host for scheduled tasks and DLL-based tasks"),
    k("runtimebroker.exe", Some("system32"), MS, "Checks permissions of Store apps (camera, location, ...)"),
    k("searchhost.exe", None, MS, "Windows Search user interface"),
    k("searchindexer.exe", Some("system32"), MS, "Windows Search indexer: keeps the file search index current"),
    k("searchprotocolhost.exe", Some("system32"), MS, "Reads file contents for the Windows Search indexer"),
    k("searchfilterhost.exe", Some("system32"), MS, "Extracts text from files for the Windows Search indexer"),
    k("startmenuexperiencehost.exe", None, MS, "Start menu"),
    k("shellexperiencehost.exe", None, MS, "Shell UI such as the clock, calendar flyout and notification area"),
    k("textinputhost.exe", None, MS, "Touch keyboard, emoji panel and input method UI"),
    k("ctfmon.exe", Some("system32"), MS, "Text input framework: keyboard layouts and input methods"),
    k("conhost.exe", Some("system32"), MS, "Console window host for a command-line program"),
    k("dllhost.exe", Some("system32"), MS, "COM surrogate: runs components such as thumbnail extraction outside Explorer"),
    k("audiodg.exe", Some("system32"), MS, "Windows audio engine (effects and mixing)"),
    k("spoolsv.exe", Some("system32"), MS, "Print spooler"),
    k("wmiprvse.exe", Some("system32\\wbem"), MS, "WMI provider host: answers system management queries from programs"),
    k("msmpeng.exe", None, MS, "Microsoft Defender Antivirus scanning engine"),
    k("nissrv.exe", None, MS, "Microsoft Defender network inspection service"),
    k("mpdefendercoreservice.exe", None, MS, "Microsoft Defender core service"),
    k("securityhealthservice.exe", Some("system32"), MS, "Windows Security health service"),
    k("securityhealthsystray.exe", Some("system32"), MS, "Windows Security notification icon"),
    k("sgrmbroker.exe", Some("system32"), MS, "System Guard runtime monitor"),
    k("tiworker.exe", None, MS, "Windows Modules Installer worker: installs updates and optional features"),
    k("trustedinstaller.exe", Some("servicing"), MS, "Windows Modules Installer service"),
    k("musnotification.exe", Some("system32"), MS, "Windows Update notifications"),
    k("compattelrunner.exe", Some("system32"), MS, "Compatibility telemetry runner (scheduled)"),
    k("wudfhost.exe", Some("system32"), MS, "User-mode driver host (sensors, fingerprint readers, ...)"),
    k("lsaiso.exe", Some("system32"), MS, "Credential Guard isolated LSA"),
    k("smartscreen.exe", Some("system32"), MS, "Microsoft Defender SmartScreen reputation checks"),
    k("applicationframehost.exe", Some("system32"), MS, "Frames the windows of Store apps"),
    k("systemsettings.exe", Some("immersivecontrolpanel"), MS, "Windows Settings app"),
    k("taskmgr.exe", Some("system32"), MS, "Task Manager"),
    k("msedge.exe", None, MS, "Microsoft Edge browser (one process per tab group, extension and helper)"),
    k("msedgewebview2.exe", None, MS, "WebView2 runtime: web content inside desktop apps"),
    k("onedrive.exe", None, MS, "OneDrive sync client"),
    k("widgets.exe", None, MS, "Windows Widgets board"),
    k("phoneexperiencehost.exe", None, MS, "Phone Link app"),
    k("chrome.exe", None, Some("Google"), "Google Chrome browser (one process per tab group, extension and helper)"),
    k("firefox.exe", None, Some("Mozilla"), "Mozilla Firefox browser"),
    k("code.exe", None, MS, "Visual Studio Code editor and its helpers"),
    k("cleanmanager.exe", None, None, "CleanManager command line"),
];

/// Expected location check: `%SystemRoot%\<dir>\<name>`, case-insensitive.
fn in_system_dir(path: &str, windows_dir: &str, dir: &str) -> bool {
    let p = path.to_lowercase().replace('/', "\\");
    let base = windows_dir.trim_end_matches('\\').to_lowercase();
    let expected = if dir.is_empty() { base } else { format!("{base}\\{dir}") };
    p.rsplit_once('\\').is_some_and(|(parent, _)| parent == expected)
}

/// Purpose from the knowledge table, only when every known attribute agrees; otherwise why it did not.
pub fn identify(f: &ProcessFacts, windows_dir: &str) -> Result<&'static str, String> {
    let name = f.name.to_lowercase();
    let Some(entry) = KNOWN.iter().find(|e| e.name == name) else {
        return Err("Not in CleanManager's local process knowledge".into());
    };
    if let Some(dir) = entry.system_dir {
        match f.image_path.as_ref().map(RawPath::display) {
            Some(p) if in_system_dir(&p, windows_dir, dir) => {}
            Some(p) => return Err(format!("Named like a known Windows process but runs from an unexpected location: {p}")),
            // Kernel/minimal processes have no readable image; only those without a directory rule can match blind.
            None if f.pid == 4 || name == "registry" || name == "memory compression" => {}
            None => return Err("Named like a known process but its location could not be read".into()),
        }
    }
    if let Some(signer) = entry.signer {
        match (&f.signature.publisher, f.signature.verified()) {
            (Some(p), true) if p.to_lowercase().starts_with(&signer.to_lowercase()) => {}
            (Some(p), true) => return Err(format!("Named like a known process but signed by {p}")),
            _ if f.signature.state == SignatureState::Unknown => return Err("Named like a known process but its signature could not be checked".into()),
            _ => return Err("Named like a known process but not signed by its vendor".into()),
        }
    }
    Ok(entry.purpose)
}

/// The application an executable belongs to: the inventory entry whose install location contains it (longest wins).
pub fn owner_app<'a>(image: Option<&RawPath>, apps: &'a [(String, String)]) -> Option<&'a str> {
    let path = image?.display().to_lowercase();
    apps.iter()
        .filter(|(_, loc)| {
            let loc = loc.trim_end_matches('\\').to_lowercase();
            !loc.is_empty() && path.starts_with(&loc) && path[loc.len()..].starts_with('\\')
        })
        .max_by_key(|(_, loc)| loc.len())
        .map(|(name, _)| name.as_str())
}

/// One row of the process list with explanation and protection.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessView {
    pub facts: ProcessFacts,
    pub parent_name: Option<String>,
    pub owner_app: Option<String>,
    pub purpose: Option<&'static str>,
    pub unidentified_reason: Option<String>,
    pub protection: Option<Protection>,
    pub pinned: bool,
}

/// Text for the AI-diagnosis entry of an unidentified process (field only; nothing is sent from here).
pub const AI_DIAGNOSIS_HINT: &str =
    "Purpose not identified locally. You can ask the optional AI diagnosis about this process; it shows exactly what would be sent (name, path, signer, parent) before sending";

impl ProcessView {
    pub fn identified(&self) -> bool {
        self.purpose.is_some()
    }

    pub fn to_wire(&self) -> serde_json::Value {
        let f = &self.facts;
        serde_json::json!({
            "pid": f.pid,
            "parent_pid": f.parent_pid,
            "parent_name": self.parent_name,
            "name": f.name,
            "image_path": f.image_path,
            "started_at": f.started_at,
            "started_at_utc": f.started_at_utc,
            "cpu_percent": f.cpu_percent,
            "memory_bytes": f.memory_bytes.map(|b| b.to_string()),
            "disk_bytes_per_sec": f.disk_bytes_per_sec,
            "network_bytes_per_sec": f.network_bytes_per_sec,
            "signature": f.signature,
            "owner_app": self.owner_app,
            "identified": self.identified(),
            "purpose": self.purpose,
            "unidentified_reason": self.unidentified_reason,
            "ai_diagnosis": (!self.identified()).then_some(serde_json::json!({"available": true, "hint": AI_DIAGNOSIS_HINT})),
            "pinned": self.pinned,
            "end_capability": match self.protection {
                Some(p) => serde_json::json!({"state": "unsupported", "protection": p, "reason": p.reason()}),
                None => serde_json::json!({"state": "available", "protection": null, "reason": "Ends only through a reviewed plan for this one process"}),
            },
            "notes": f.notes,
        })
    }
}

/// Explain a set of facts: purpose, owner app, parent name, protection and pins.
pub fn explain(facts: Vec<ProcessFacts>, own_pid: u32, windows_dir: &str, apps: &[(String, String)], pins: &[String]) -> Vec<ProcessView> {
    let names: std::collections::BTreeMap<u32, String> = facts.iter().map(|f| (f.pid, f.name.clone())).collect();
    facts
        .into_iter()
        .map(|f| {
            let (purpose, unidentified_reason) = match identify(&f, windows_dir) {
                Ok(p) => (Some(p), None),
                Err(why) => (None, Some(why)),
            };
            ProcessView {
                parent_name: f.parent_pid.and_then(|p| names.get(&p).cloned()),
                owner_app: owner_app(f.image_path.as_ref(), apps).map(str::to_owned),
                protection: protection(&f, own_pid),
                pinned: pins.iter().any(|p| p.eq_ignore_ascii_case(&f.name)),
                purpose,
                unidentified_reason,
                facts: f,
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortKey {
    Cpu,
    Memory,
    Disk,
    Network,
    Name,
}

impl SortKey {
    pub fn parse(s: &str) -> Option<SortKey> {
        Some(match s {
            "cpu" => SortKey::Cpu,
            "memory" => SortKey::Memory,
            "disk" => SortKey::Disk,
            "network" => SortKey::Network,
            "name" => SortKey::Name,
            _ => return None,
        })
    }
}

/// Pinned rows first, then by the key (largest first; unknown values after known ones), then name and PID.
pub fn sort(rows: &mut [ProcessView], key: SortKey) {
    let value = |r: &ProcessView| -> Option<f64> {
        match key {
            SortKey::Cpu => r.facts.cpu_percent,
            SortKey::Memory => r.facts.memory_bytes.map(|b| b as f64),
            SortKey::Disk => r.facts.disk_bytes_per_sec,
            SortKey::Network => r.facts.network_bytes_per_sec,
            SortKey::Name => None,
        }
    };
    rows.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then_with(|| match (value(a), value(b)) {
                (Some(x), Some(y)) => y.total_cmp(&x),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.facts.name.to_lowercase().cmp(&b.facts.name.to_lowercase()))
            .then(a.facts.pid.cmp(&b.facts.pid))
    });
}

/// Case-insensitive search over name, PID, path, publisher, owner app and purpose. Pinned rows always stay.
pub fn matches(r: &ProcessView, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() || r.pinned {
        return true;
    }
    let f = &r.facts;
    let fields = [
        Some(f.name.clone()),
        Some(f.pid.to_string()),
        f.image_path.as_ref().map(RawPath::display),
        f.signature.publisher.clone(),
        r.owner_app.clone(),
        r.purpose.map(str::to_owned),
    ];
    fields.into_iter().flatten().any(|v| v.to_lowercase().contains(&q))
}

/// Why a process cannot be put into an end plan.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EndRefusal {
    #[error("{}", .0.reason())]
    Protected(Protection),
    #[error("the process start time could not be read, so the process cannot be identified safely")]
    NoIdentity,
}

/// The single action of a `process` plan for this row. `image_sha256` is the executable digest when readable.
pub fn end_action(r: &ProcessView, image_sha256: Option<String>) -> Result<ProcessEndAction, EndRefusal> {
    if let Some(p) = r.protection {
        return Err(EndRefusal::Protected(p));
    }
    let started_at = r.facts.started_at.clone().ok_or(EndRefusal::NoIdentity)?;
    Ok(ProcessEndAction {
        action_id: "a01".into(),
        pid: r.facts.pid,
        started_at,
        started_at_utc: r.facts.started_at_utc.clone(),
        name: r.facts.name.clone(),
        image_path: r.facts.image_path.clone(),
        image_sha256,
    })
}

/// Result of ending one process at execution time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndOutcome {
    /// The planned instance exited after the request.
    Ended,
    /// No process with this PID and start time exists any more.
    AlreadyExited,
    /// The PID now belongs to another process, or the executable changed.
    Changed(String),
    /// Refused by the live protection check.
    Protected(Protection),
    PermissionRequired(String),
    Failed(String),
    /// The request was sent but exit could not be confirmed.
    Unknown(String),
}

/// Platform side of `process_end` (Windows: re-identify by PID + creation time + image, then end).
pub trait ProcessActions {
    fn end(&self, action: &ProcessEndAction, own_pid: u32) -> EndOutcome;
    /// Whether the planned instance is still running (`None` = cannot tell). Used by reconciliation only.
    fn running(&self, action: &ProcessEndAction) -> Option<bool>;
}

#[cfg(test)]
mod tests;
