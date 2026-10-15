//! Startup item inventory model (parity STA-01; read-only, toggling is task 11.2).
//!
//! - An item is identified by where it is defined (source, scope, registry view / file / task path), never by its
//!   display name, so two entries with the same name stay separate.
//! - `state` folds the platform's enable/disable record together with a read-only check of the target: a target
//!   missing on a mounted volume is `broken`; a target on a volume that is not mounted is `target_unreachable`
//!   (unknown, never broken: STA-03). A target that could not be checked keeps the recorded state.
//! - Nothing here is a toggle. `toggle_capability` stays `unverified` until 11.2 lands; protected items (STA-04:
//!   system components, security/management agents, this tool) are `unsupported`.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::capability::CapabilityState;
use crate::path::RawPath;

/// Toggle reason while startup changes are not implemented (11.2).
pub const TOGGLE_NOT_IMPLEMENTED: &str = "Read-only inventory: enabling or disabling startup items is not available in this build";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupSource {
    RunKey,
    StartupFolder,
    ScheduledTask,
    StoreStartupTask,
    SmLoginItem,
    SmBackgroundItem,
    LaunchdUser,
    LaunchdSystem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupScope {
    User,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupState {
    Enabled,
    Disabled,
    Broken,
    TargetUnreachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublisherEvidence {
    /// Signer of a verified Authenticode signature on the target file.
    Authenticode,
    /// Publisher of the owning package (MSIX/AppX identity).
    PackageIdentity,
    None,
}

/// Read-only observation of a startup target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetCheck {
    Exists,
    /// Not found, and the volume it lives on is mounted.
    Missing,
    /// The drive or share it lives on is not available.
    VolumeOffline,
    /// Not checked (network path, access denied, no target): keep the recorded state.
    Unchecked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupItem {
    pub item_id: String,
    pub name: String,
    pub source: StartupSource,
    /// Where the entry is defined (registry key and value, shortcut file, task path). Display only.
    pub definition: String,
    pub target: Option<RawPath>,
    pub publisher: Option<String>,
    pub publisher_evidence: PublisherEvidence,
    pub scope: StartupScope,
    pub state: StartupState,
    pub toggle_capability: CapabilityState,
    pub protected: bool,
    pub reason: Option<String>,
}

/// Stable item ID from the definition material (source, scope, location, value or file name), never the target.
pub fn item_id(source: StartupSource, scope: StartupScope, definition: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"cm-startup-item-v1\0");
    h.update(serde_json::to_string(&source).unwrap_or_default().as_bytes());
    h.update(b"\0");
    h.update(serde_json::to_string(&scope).unwrap_or_default().as_bytes());
    h.update(b"\0");
    h.update(definition.to_lowercase().as_bytes());
    let hex: String = h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("st_{hex}")
}

/// Explorer's `StartupApproved` value: 12 bytes, first byte even = enabled, odd = disabled (the remaining bytes
/// are the FILETIME of the change). `None` for data that does not have that shape: unknown, treated as enabled
/// because Explorer runs entries that have no approval record.
pub fn approval_enabled(data: &[u8]) -> Option<bool> {
    if data.len() < 4 {
        return None;
    }
    Some(data[0] & 1 == 0)
}

/// Fold the recorded enablement and the target check into the protocol state.
pub fn fold_state(enabled: bool, check: TargetCheck) -> (StartupState, Option<&'static str>) {
    match check {
        TargetCheck::Missing => (StartupState::Broken, Some(if enabled { "Target is missing on a mounted volume" } else { "Target is missing on a mounted volume (entry is disabled)" })),
        TargetCheck::VolumeOffline => (StartupState::TargetUnreachable, Some("Target volume is not mounted; not treated as broken")),
        TargetCheck::Exists | TargetCheck::Unchecked if enabled => (StartupState::Enabled, None),
        TargetCheck::Exists | TargetCheck::Unchecked => (StartupState::Disabled, None),
    }
}

/// How a command line was split into the program and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandTarget {
    /// The program path exactly as written (quotes removed, environment not expanded).
    pub program: String,
    /// True if the program has no directory part (resolved later against the system directories).
    pub bare: bool,
}

const PROGRAM_EXTENSIONS: &[&str] = &[".exe", ".com", ".bat", ".cmd", ".lnk", ".scr", ".pif", ".vbs", ".js", ".msc", ".ps1"];

/// Split a Windows startup command line into its program. Quoted programs are taken verbatim. For unquoted
/// commands, `exists` is asked about each space-separated prefix (shortest first, as `CreateProcess` searches) and
/// the first one that exists wins; otherwise the shortest prefix ending in a program extension, then the first
/// token. The command is never executed or passed to a shell.
pub fn split_command(command: &str, exists: &dyn Fn(&str) -> bool) -> Option<CommandTarget> {
    let cmd = command.trim();
    if cmd.is_empty() {
        return None;
    }
    let program = if let Some(rest) = cmd.strip_prefix('"') {
        let end = rest.find('"').unwrap_or(rest.len());
        rest[..end].trim().to_owned()
    } else {
        let spaces: Vec<usize> = cmd.char_indices().filter(|(_, c)| *c == ' ' || *c == '\t').map(|(i, _)| i).chain(std::iter::once(cmd.len())).collect();
        let prefixes: Vec<&str> = spaces.iter().map(|&i| cmd[..i].trim_end()).filter(|p| !p.is_empty()).collect();
        let has_ext = |p: &str| {
            let l = p.to_ascii_lowercase();
            PROGRAM_EXTENSIONS.iter().any(|e| l.ends_with(e))
        };
        prefixes
            .iter()
            .find(|p| has_ext(p) && exists(p))
            .or_else(|| prefixes.iter().find(|p| has_ext(p)))
            .or(prefixes.first())
            .map(|p| (*p).to_owned())?
    };
    if program.is_empty() {
        return None;
    }
    let bare = !program.contains('\\') && !program.contains('/');
    Some(CommandTarget { program, bare })
}

/// Hosts whose real target is their first argument (`rundll32.exe C:\x.dll,Entry`). Only for display of the
/// program; the inventory reports the host as the target and says so in `reason`.
pub fn is_script_host(program: &str) -> bool {
    let l = program.to_ascii_lowercase();
    let name = l.rsplit(['\\', '/']).next().unwrap_or(&l);
    matches!(name, "rundll32.exe" | "rundll32" | "cmd.exe" | "cmd" | "powershell.exe" | "powershell" | "pwsh.exe" | "wscript.exe" | "cscript.exe" | "mshta.exe" | "conhost.exe")
}

/// Why an item is read-only (STA-04): system components, security/management agents and this tool itself.
pub fn protected_reason(name: &str, publisher: Option<&str>, target_display: Option<&str>, task_path: Option<&str>) -> Option<&'static str> {
    let lower = |s: &str| s.to_lowercase();
    if task_path.is_some_and(|t| lower(t).starts_with(r"\microsoft\")) {
        return Some("Windows component task");
    }
    if let Some(t) = target_display.map(lower) {
        let windir = std::env::var("SystemRoot").map(|s| s.to_lowercase()).unwrap_or_else(|_| r"c:\windows".into());
        if t.starts_with(&format!("{windir}\\")) {
            return Some("Target is part of Windows");
        }
        if t.ends_with("\\cleanmanager.exe") || t.contains("\\cleanmanager\\") {
            return Some("CleanManager's own entry");
        }
    }
    if publisher.is_some_and(|p| matches!(p, "Microsoft Windows" | "Microsoft Windows Publisher")) {
        return Some("Published as part of Windows");
    }
    if crate::apps::protected_reason(name, publisher, false) == Some(crate::apps::ProtectedReason::EndpointSecurityOrManagement) {
        return Some("Security or device-management agent");
    }
    if lower(name).contains("cleanmanager") {
        return Some("CleanManager's own entry");
    }
    None
}

impl StartupItem {
    pub fn to_wire(&self) -> serde_json::Value {
        serde_json::json!({
            "startup_item_id": self.item_id,
            "name": self.name,
            "source": self.source,
            "definition": self.definition,
            "target": self.target,
            "publisher": self.publisher,
            "publisher_evidence": self.publisher_evidence,
            "scope": self.scope,
            "state": self.state,
            "toggle_capability": self.toggle_capability,
            "protected": self.protected,
            "reason": self.reason,
        })
    }
}

/// Deterministic order: scope, source, name, id. Duplicate IDs (the same definition read twice) keep the first.
pub fn normalize(mut items: Vec<StartupItem>) -> Vec<StartupItem> {
    items.sort_by(|a, b| (a.scope, a.source, a.name.to_lowercase(), &a.item_id).cmp(&(b.scope, b.source, b.name.to_lowercase(), &b.item_id)));
    let mut seen = std::collections::BTreeSet::new();
    items.retain(|i| seen.insert(i.item_id.clone()));
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_bytes() {
        assert_eq!(approval_enabled(&[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]), Some(true));
        assert_eq!(approval_enabled(&[6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]), Some(true));
        assert_eq!(approval_enabled(&[3, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]), Some(false));
        assert_eq!(approval_enabled(&[7, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]), Some(false));
        assert_eq!(approval_enabled(&[3]), None);
    }

    #[test]
    fn state_folding_never_calls_an_offline_target_broken() {
        assert_eq!(fold_state(true, TargetCheck::Exists).0, StartupState::Enabled);
        assert_eq!(fold_state(false, TargetCheck::Exists).0, StartupState::Disabled);
        assert_eq!(fold_state(true, TargetCheck::Unchecked).0, StartupState::Enabled);
        assert_eq!(fold_state(false, TargetCheck::Missing).0, StartupState::Broken);
        assert_eq!(fold_state(true, TargetCheck::VolumeOffline).0, StartupState::TargetUnreachable);
        assert!(fold_state(true, TargetCheck::VolumeOffline).1.unwrap().contains("not treated as broken"));
    }

    #[test]
    fn command_splitting() {
        let none = |_: &str| false;
        let t = |c: &str| split_command(c, &none).map(|t| t.program);
        assert_eq!(t(r#""C:\Program Files\App\app.exe" --background"#).as_deref(), Some(r"C:\Program Files\App\app.exe"));
        assert_eq!(t(r"C:\PROGRA~1\App\app.exe /min").as_deref(), Some(r"C:\PROGRA~1\App\app.exe"));
        assert_eq!(t(r"%windir%\system32\SecurityHealthSystray.exe").as_deref(), Some(r"%windir%\system32\SecurityHealthSystray.exe"));
        assert_eq!(t("rundll32.exe shell32.dll,Control_RunDLL").as_deref(), Some("rundll32.exe"));
        assert!(split_command("rundll32.exe x", &none).unwrap().bare);
        assert_eq!(t("   "), None);
        assert_eq!(t(r#""unterminated"#).as_deref(), Some("unterminated"));
        // Unquoted path with spaces: the existing prefix wins over the shortest token.
        let exists = |p: &str| p == r"C:\Program Files\My App\run.exe";
        let got = split_command(r"C:\Program Files\My App\run.exe -silent", &exists).unwrap();
        assert_eq!(got.program, r"C:\Program Files\My App\run.exe");
        // Without an existing prefix the first prefix ending in an extension is used.
        assert_eq!(t(r"C:\Program Files\My App\run.exe -silent").as_deref(), Some(r"C:\Program Files\My App\run.exe"));
        assert_eq!(t("noext arg").as_deref(), Some("noext"));
        assert!(is_script_host(r"C:\Windows\System32\rundll32.exe"));
        assert!(!is_script_host(r"C:\Program Files\App\app.exe"));
    }

    #[test]
    fn ids_follow_the_definition_not_the_name_or_target() {
        let a = item_id(StartupSource::RunKey, StartupScope::User, r"HKCU\...\Run : App");
        let b = item_id(StartupSource::RunKey, StartupScope::User, r"HKCU\...\Run : App");
        let c = item_id(StartupSource::RunKey, StartupScope::System, r"HKCU\...\Run : App");
        let d = item_id(StartupSource::StartupFolder, StartupScope::User, r"HKCU\...\Run : App");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
        assert!(a.starts_with("st_") && a.len() == 19);
    }

    #[test]
    fn protection_rules() {
        assert_eq!(protected_reason("Task", None, None, Some(r"\Microsoft\Windows\Defrag\ScheduledDefrag")), Some("Windows component task"));
        assert!(protected_reason("SecurityHealth", None, Some(r"C:\Windows\System32\SecurityHealthSystray.exe"), None).is_some());
        assert!(protected_reason("Example Defender Agent", None, Some(r"D:\x\agent.exe"), None).is_some());
        assert!(protected_reason("Tray", Some("CrowdStrike, Inc."), None, None).is_some());
        assert!(protected_reason("CleanManager", None, None, None).is_some());
        assert_eq!(protected_reason("Example Updater", Some("Example Vendor"), Some(r"C:\Program Files\Example\u.exe"), Some(r"\Example\Updater")), None);
    }

    #[test]
    fn normalize_dedups_and_orders() {
        let mk = |name: &str, scope| StartupItem {
            item_id: item_id(StartupSource::RunKey, scope, name),
            name: name.into(),
            source: StartupSource::RunKey,
            definition: name.into(),
            target: None,
            publisher: None,
            publisher_evidence: PublisherEvidence::None,
            scope,
            state: StartupState::Enabled,
            toggle_capability: CapabilityState::Unverified,
            protected: false,
            reason: None,
        };
        let items = normalize(vec![mk("b", StartupScope::System), mk("a", StartupScope::User), mk("b", StartupScope::System)]);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "a");
        let w = items[0].to_wire();
        assert_eq!(w["source"], "run_key");
        assert_eq!(w["toggle_capability"], "unverified");
        assert_eq!(w["publisher_evidence"], "none");
    }
}
