//! Startup items (parity STA-01..04, tasks 11.1–11.3): inventory model, change decisions and plans.
//!
//! - An item is identified by where it is defined (source, scope, registry view / file / task path), never by its
//!   display name, so two entries with the same name stay separate.
//! - `state` folds the platform's enable/disable record together with a read-only check of the target: a target
//!   missing on a mounted volume is `broken`; a target on a volume that is not mounted is `target_unreachable`
//!   (unknown, never broken: STA-03). A target that could not be checked keeps the recorded state.
//! - Changes (11.2/11.3) are `startup` plans (`plan/v2`) of `startup_toggle` actions, reviewed by a person and applied
//!   by the shared executor. Enable/disable writes only the enable record Task Manager uses (`StartupApproved`); the
//!   definition is untouched. A stale Run value is deleted only after its backup is stored with the operation. Every
//!   change records the prior state, and a `recovery` plan puts it back (never over a value recreated since).
//! - Only sources with a verified mechanism toggle (Run values, Startup-folder files). Others offer the system settings
//!   page instead; protected items (STA-04: system components, security/management agents, this tool) never toggle;
//!   system-scope items need an administrator and this tool never elevates itself.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::capability::CapabilityState;
use crate::path::RawPath;
use crate::plan::v2::{ActionMeta, Hive, HeaderInput, PlanAction, PlanKind, PlanV2, RegistryRestoreAction, StartupOperation, StartupToggleAction};

/// Former read-only reason (11.1); kept for callers that still show it.
pub const TOGGLE_NOT_IMPLEMENTED: &str = "Read-only inventory: enabling or disabling startup items is not available in this build";
/// Reason for sources without a verified toggle mechanism (scheduled tasks, packaged-app tasks).
pub const OPEN_SETTINGS_GUIDANCE: &str =
    "No verified toggle for this source: change it in Settings > Apps > Startup (ms-settings:startupapps) or Task Scheduler";
/// Reason for system-scope items when this process is not elevated.
pub const ADMIN_REQUIRED: &str = "System-wide entry: run CleanManager from an administrator terminal to change it (it never elevates itself)";

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
    /// macOS: signer (Team ID / authority) of a valid code signature on the target or its owning bundle.
    CodeSignature,
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
    /// Structured location for sources with a verified toggle mechanism (Run values, Startup-folder files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<StartupLocation>,
    /// The definition is a value of a `Run` key (not RunOnce or policy): the only kind a stale entry is removed from.
    #[serde(default)]
    pub removable_when_stale: bool,
}

/// Registry view of a key on 64-bit Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegView {
    Default,
    Wow64_64,
    Wow64_32,
}

/// Where a toggleable entry and its enable record live. Key paths are logical (as on a real machine); `root` is the
/// physical redirection they were read through (empty = the real hives; tests use a throwaway key), so a plan made
/// against one root can never act on another.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum StartupLocation {
    /// A `Run` value; its enable record is the same-named value under `approval_key`.
    RunValue { root: String, hive: Hive, view: RegView, key: String, value_name: String, approval_key: String },
    /// A file in a Startup folder; its enable record is the value named like the file under `approval_key`.
    FolderFile { root: String, hive: Hive, folder: RawPath, file_name: String, approval_key: String },
    /// macOS: a launchd job plist of the user's domain (`~/Library/LaunchAgents`), changed through `launchctl` with the
    /// plist backed up first (D9). `root` is the test redirection it was read through (empty = the real home).
    LaunchdUser { root: String, plist: RawPath, label: String },
    /// macOS: a launchd job plist of the system domain (`/Library/LaunchAgents`, `/Library/LaunchDaemons`); system-wide.
    LaunchdSystem { root: String, plist: RawPath, label: String },
    /// macOS: a Service Management login item or background item, identified by its bundle/service identifier.
    SmItem { root: String, identifier: String, background: bool },
}

impl StartupLocation {
    /// Registry hive of a Windows location. macOS locations report the hive of the equivalent scope (system-wide =
    /// `Hklm`), so callers that only distinguish user and machine scope can use [`StartupLocation::system_wide`].
    pub fn hive(&self) -> Hive {
        match self {
            StartupLocation::RunValue { hive, .. } | StartupLocation::FolderFile { hive, .. } => *hive,
            StartupLocation::LaunchdSystem { .. } => Hive::Hklm,
            StartupLocation::LaunchdUser { .. } | StartupLocation::SmItem { .. } => Hive::Hkcu,
        }
    }

    /// Changing it affects every user of the machine (needs an administrator).
    pub fn system_wide(&self) -> bool {
        self.hive() == Hive::Hklm
    }

    pub fn root(&self) -> &str {
        match self {
            StartupLocation::RunValue { root, .. }
            | StartupLocation::FolderFile { root, .. }
            | StartupLocation::LaunchdUser { root, .. }
            | StartupLocation::LaunchdSystem { root, .. }
            | StartupLocation::SmItem { root, .. } => root,
        }
    }

    /// Name of the enable-record value (macOS: the launchd label or Service Management identifier).
    pub fn approval_value(&self) -> &str {
        match self {
            StartupLocation::RunValue { value_name, .. } => value_name,
            StartupLocation::FolderFile { file_name, .. } => file_name,
            StartupLocation::LaunchdUser { label, .. } | StartupLocation::LaunchdSystem { label, .. } => label,
            StartupLocation::SmItem { identifier, .. } => identifier,
        }
    }

    /// Key of the enable record (Windows `StartupApproved`); empty for macOS locations, which have none.
    pub fn approval_key(&self) -> &str {
        match self {
            StartupLocation::RunValue { approval_key, .. } | StartupLocation::FolderFile { approval_key, .. } => approval_key,
            StartupLocation::LaunchdUser { .. } | StartupLocation::LaunchdSystem { .. } | StartupLocation::SmItem { .. } => "",
        }
    }

    /// The inventory source this location belongs to.
    pub fn source(&self) -> StartupSource {
        match self {
            StartupLocation::RunValue { .. } => StartupSource::RunKey,
            StartupLocation::FolderFile { .. } => StartupSource::StartupFolder,
            StartupLocation::LaunchdUser { .. } => StartupSource::LaunchdUser,
            StartupLocation::LaunchdSystem { .. } => StartupSource::LaunchdSystem,
            StartupLocation::SmItem { background: false, .. } => StartupSource::SmLoginItem,
            StartupLocation::SmItem { background: true, .. } => StartupSource::SmBackgroundItem,
        }
    }
}

/// A registry value as read: type and raw data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegValue {
    pub value_type: u32,
    /// Lower-case hex of the raw data.
    pub data_hex: String,
}

impl RegValue {
    pub fn new(value_type: u32, data: &[u8]) -> Self {
        RegValue { value_type, data_hex: hex(data) }
    }

    pub fn data(&self) -> Option<Vec<u8>> {
        unhex(&self.data_hex)
    }
}

pub fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// Live state of a toggleable entry, read without changing anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupObservation {
    /// The `Run` value (type and data), or for a Startup-folder file its volume/file identity as text. `None` = gone.
    pub definition: Option<RegValue>,
    /// The enable record (`StartupApproved` value data). `None` = no record (runs as enabled).
    pub approval: Option<RegValue>,
}

impl StartupObservation {
    /// Enabled as Explorer/Task Manager would treat it (no or unreadable record = enabled).
    pub fn enabled(&self) -> bool {
        self.approval.as_ref().and_then(RegValue::data).and_then(|d| approval_enabled(&d)).unwrap_or(true)
    }
}

/// SHA-256 binding a location to what was observed there; recomputed live before any change.
pub fn observation_digest(loc: &StartupLocation, obs: &StartupObservation) -> String {
    let v = serde_json::json!({"location": loc, "observation": obs});
    crate::c14n::digest(&v).expect("plain JSON")
}

/// Enable-record data written for `enabled` (Task Manager's format: 0x02 enabled; 0x03 disabled + FILETIME).
pub fn approval_record(enabled: bool, now_filetime: u64) -> Vec<u8> {
    let mut d = vec![if enabled { 2u8 } else { 3u8 }, 0, 0, 0];
    d.extend_from_slice(&if enabled { 0u64 } else { now_filetime }.to_le_bytes());
    d
}

/// Prior-state record stored with the operation before a startup change, so a recovery plan can put it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartupBackup {
    pub backup_id: String,
    pub startup_item_id: String,
    pub operation: StartupOperation,
    pub location: StartupLocation,
    /// The `Run` value before the change (always kept for removals).
    pub prior_definition: Option<RegValue>,
    /// The enable record before the change.
    pub prior_approval: Option<RegValue>,
    /// The enable record this change wrote (`None` for a removal).
    pub written_approval: Option<RegValue>,
    pub created_at: String,
}

impl StartupBackup {
    pub fn sha256(&self) -> String {
        crate::c14n::digest(&serde_json::to_value(self).expect("backup serializes")).expect("plain JSON")
    }
}

/// Why a change cannot be planned or applied (`code` is the protocol error/skip code).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub message: String,
}

fn refuse(code: &'static str, message: impl Into<String>) -> Refusal {
    Refusal { code, message: message.into() }
}

/// Whether `op` may be planned for `item` by a process that is (`is_admin`) or is not elevated (STA-02/03/04).
pub fn change_decision(item: &StartupItem, op: StartupOperation, is_admin: bool) -> Result<(), Refusal> {
    if item.protected {
        return Err(refuse("policy_denied", format!("{} is protected: {}", item.name, item.reason.as_deref().unwrap_or("system or security entry"))));
    }
    if item.location.is_none() {
        let why = item.reason.clone().unwrap_or_else(|| OPEN_SETTINGS_GUIDANCE.into());
        return Err(refuse("unsupported", why));
    }
    if item.scope == StartupScope::System && !is_admin {
        return Err(refuse("permission_required", ADMIN_REQUIRED));
    }
    match op {
        StartupOperation::Restore => Err(refuse("invalid_argument", "restore comes from an operation's recorded prior state (startup restore --operation)")),
        StartupOperation::Disable if item.state == StartupState::Disabled => Err(refuse("conflict", "already disabled")),
        StartupOperation::Enable if item.state == StartupState::Enabled => Err(refuse("conflict", "already enabled")),
        StartupOperation::RemoveWithBackup => match item.state {
            _ if !item.removable_when_stale => Err(refuse("unsupported", "only stale Run-key values are removed; this source keeps its definition")),
            StartupState::Broken => Ok(()),
            StartupState::TargetUnreachable => Err(refuse("conflict", "target volume is offline: state unknown, not stale")),
            _ => Err(refuse("conflict", "the target exists or could not be checked; only stale entries are removed")),
        },
        _ => Ok(()),
    }
}

/// Wire capability of an item for a process that is (`is_admin`) or is not elevated.
pub fn toggle_capability(protected: bool, has_mechanism: bool, scope: StartupScope, is_admin: bool) -> CapabilityState {
    if protected {
        CapabilityState::Unsupported
    } else if !has_mechanism {
        CapabilityState::Unverified
    } else if scope == StartupScope::System && !is_admin {
        CapabilityState::PermissionRequired
    } else {
        CapabilityState::Available
    }
}

/// Freeze one change of one item into a `startup` plan (review required; never policy-matched).
pub fn build_startup_plan(
    item: &StartupItem, op: StartupOperation, obs: &StartupObservation, is_admin: bool, plan_id: &str, created_unix: i64, policy_digest: &str,
) -> Result<PlanV2, Refusal> {
    change_decision(item, op, is_admin)?;
    let loc = item.location.clone().expect("checked by change_decision");
    if obs.definition.is_none() {
        return Err(refuse("conflict", "the entry no longer exists; list startup items again"));
    }
    let action = PlanAction::StartupToggle(StartupToggleAction {
        action_id: "s0000000".into(),
        startup_item_id: item.item_id.clone(),
        source: item.source,
        operation: op,
        scope: item.scope,
        definition: Some(item.definition.clone()),
        observed_sha256: Some(observation_digest(&loc, obs)),
        backup_id: None,
    });
    let header = HeaderInput {
        plan_id, plan_kind: PlanKind::Startup, created_unix, scope_id: None, scan_id: None, scan_revision: None,
        rule_set_digest: crate::plan::v2::NO_RULES_DIGEST, policy_digest,
    };
    let meta = ActionMeta { startup: Some(loc), note: Some(item.name.clone()), ..Default::default() };
    crate::plan::v2::build(&header, vec![action], vec![meta]).map_err(|e| refuse("internal_error", e.to_string()))
}

/// Freeze the recorded prior states of a startup operation into a `recovery` plan: enable records are put back
/// (`startup_toggle restore`, only if nobody changed them since), removed values are recreated (`registry_restore`,
/// skipped when the value exists again).
pub fn build_startup_recovery_plan(backups: &[(String, StartupBackup)], plan_id: &str, created_unix: i64, policy_digest: &str) -> Result<PlanV2, Refusal> {
    if backups.is_empty() {
        return Err(refuse("invalid_argument", "the operation recorded no startup change to restore"));
    }
    let mut items = Vec::new();
    let mut meta = Vec::new();
    for (i, (item_name, b)) in backups.iter().enumerate() {
        let action_id = format!("r{i:07}");
        let scope = if b.location.system_wide() { StartupScope::System } else { StartupScope::User };
        let source = b.location.source();
        match (&b.operation, &b.location) {
            (StartupOperation::RemoveWithBackup, StartupLocation::RunValue { hive, key, value_name, .. }) => {
                items.push(PlanAction::RegistryRestore(RegistryRestoreAction {
                    action_id, backup_id: b.backup_id.clone(), hive: *hive, key_path: key.clone(), value_name: Some(value_name.clone()),
                    backup_sha256: Some(b.sha256()), on_existing: "skip".into(),
                }));
            }
            (StartupOperation::Enable | StartupOperation::Disable, _) => {
                items.push(PlanAction::StartupToggle(StartupToggleAction {
                    action_id, startup_item_id: b.startup_item_id.clone(), source, operation: StartupOperation::Restore, scope,
                    definition: None, observed_sha256: Some(b.sha256()), backup_id: Some(b.backup_id.clone()),
                }));
            }
            _ => return Err(refuse("unsupported", format!("backup {} cannot be restored by this build", b.backup_id))),
        }
        meta.push(ActionMeta { startup: Some(b.location.clone()), note: Some(item_name.clone()), ..Default::default() });
    }
    let header = HeaderInput {
        plan_id, plan_kind: PlanKind::Recovery, created_unix, scope_id: None, scan_id: None, scan_revision: None,
        rule_set_digest: crate::plan::v2::NO_RULES_DIGEST, policy_digest,
    };
    crate::plan::v2::build(&header, items, meta).map_err(|e| refuse("internal_error", e.to_string()))
}

/// Result of one platform startup change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum StartupOutcome {
    Done,
    /// Precondition not met (changed since planning, recreated, protected...); nothing was changed.
    Skipped { code: String, message: String },
    /// The OS refused; nothing was changed.
    Failed { code: String, message: String },
    /// The write may or may not have taken effect.
    Unknown { message: String },
}

/// Stores the prior state (and the enable record about to be written) before a change; an `Err` stops the change.
pub type ToggleBackup<'a> = &'a mut dyn FnMut(&StartupObservation, Option<&RegValue>) -> Result<(), String>;
/// Stores the prior state before a removal; an `Err` stops the removal.
pub type RemovalBackup<'a> = &'a mut dyn FnMut(&StartupObservation) -> Result<(), String>;

/// Platform startup changes. Every method re-reads the live state first and changes nothing unless it still matches.
pub trait StartupActions {
    /// Read-only observation of a location.
    fn observe(&self, loc: &StartupLocation) -> Result<StartupObservation, String>;
    /// Whether this process may write machine-wide locations (never elevates).
    fn is_admin(&self) -> bool;
    /// Write the enable record if the live state still hashes to `expected`; `backup` stores the prior state first
    /// (its failure stops the change).
    fn set_enabled(&self, loc: &StartupLocation, enabled: bool, expected: &str, backup: ToggleBackup<'_>) -> StartupOutcome;
    /// Delete a stale `Run` value (and its enable record) after `backup` stored both; the target must still be missing.
    fn remove_stale(&self, loc: &StartupLocation, expected: &str, backup: RemovalBackup<'_>) -> StartupOutcome;
    /// Put an enable record back to `prior` if it still holds what the operation wrote (`written`).
    fn restore_approval(&self, loc: &StartupLocation, written: Option<&RegValue>, prior: Option<&RegValue>) -> StartupOutcome;
    /// Recreate a removed `Run` value (and its enable record) unless a value with that name exists again.
    fn restore_value(&self, backup: &StartupBackup) -> StartupOutcome;
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
            location: None,
            removable_when_stale: false,
        };
        let items = normalize(vec![mk("b", StartupScope::System), mk("a", StartupScope::User), mk("b", StartupScope::System)]);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "a");
        let w = items[0].to_wire();
        assert_eq!(w["source"], "run_key");
        assert_eq!(w["toggle_capability"], "unverified");
        assert_eq!(w["publisher_evidence"], "none");
    }

    fn toggleable(scope: StartupScope, state: StartupState) -> StartupItem {
        let hive = if scope == StartupScope::System { Hive::Hklm } else { Hive::Hkcu };
        StartupItem {
            item_id: "st_x".into(), name: "X".into(), source: StartupSource::RunKey, definition: "Run : X".into(), target: None, publisher: None,
            publisher_evidence: PublisherEvidence::None, scope, state, toggle_capability: CapabilityState::Available, protected: false, reason: None,
            location: Some(StartupLocation::RunValue { root: String::new(), hive, view: RegView::Default, key: "Run".into(), value_name: "X".into(), approval_key: "A".into() }),
            removable_when_stale: true,
        }
    }

    /// STA-02/03/04 decisions with the elevation state injected (the dev shell itself is elevated).
    #[test]
    fn change_decisions() {
        let user = toggleable(StartupScope::User, StartupState::Enabled);
        assert_eq!(change_decision(&user, StartupOperation::Disable, false), Ok(()));
        assert_eq!(change_decision(&user, StartupOperation::Enable, false).unwrap_err().code, "conflict");
        let system = toggleable(StartupScope::System, StartupState::Enabled);
        assert_eq!(change_decision(&system, StartupOperation::Disable, false).unwrap_err().code, "permission_required");
        assert_eq!(change_decision(&system, StartupOperation::Disable, true), Ok(()));
        let protected = StartupItem { protected: true, reason: Some("Security or device-management agent".into()), ..user.clone() };
        assert_eq!(change_decision(&protected, StartupOperation::Disable, true).unwrap_err().code, "policy_denied");
        let unverified = StartupItem { location: None, source: StartupSource::ScheduledTask, reason: Some(OPEN_SETTINGS_GUIDANCE.into()), ..user.clone() };
        let r = change_decision(&unverified, StartupOperation::Disable, true).unwrap_err();
        assert_eq!(r.code, "unsupported");
        assert!(r.message.contains("ms-settings:startupapps"));
        // Stale: broken only; an offline volume is unknown, never stale; non-Run sources keep their definition.
        let broken = toggleable(StartupScope::User, StartupState::Broken);
        assert_eq!(change_decision(&broken, StartupOperation::RemoveWithBackup, false), Ok(()));
        let offline = toggleable(StartupScope::User, StartupState::TargetUnreachable);
        assert_eq!(change_decision(&offline, StartupOperation::RemoveWithBackup, false).unwrap_err().code, "conflict");
        assert_eq!(change_decision(&user, StartupOperation::RemoveWithBackup, false).unwrap_err().code, "conflict");
        let folder = StartupItem { removable_when_stale: false, ..broken.clone() };
        assert_eq!(change_decision(&folder, StartupOperation::RemoveWithBackup, false).unwrap_err().code, "unsupported");
        assert_eq!(change_decision(&user, StartupOperation::Restore, false).unwrap_err().code, "invalid_argument");
        assert_eq!(toggle_capability(false, true, StartupScope::System, false), CapabilityState::PermissionRequired);
        assert_eq!(toggle_capability(false, true, StartupScope::System, true), CapabilityState::Available);
        assert_eq!(toggle_capability(false, false, StartupScope::User, true), CapabilityState::Unverified);
        assert_eq!(toggle_capability(true, true, StartupScope::User, true), CapabilityState::Unsupported);
    }

    #[test]
    fn task_manager_records_and_plans() {
        assert_eq!(approval_record(true, 99), vec![2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let off = approval_record(false, 0x0102_0304_0506_0708);
        assert_eq!((off[0], &off[4..]), (3, &[8u8, 7, 6, 5, 4, 3, 2, 1][..]));
        assert_eq!(approval_enabled(&off), Some(false));
        let item = toggleable(StartupScope::User, StartupState::Enabled);
        let obs = StartupObservation { definition: Some(RegValue::new(1, b"a ")), approval: None };
        let plan = build_startup_plan(&item, StartupOperation::Disable, &obs, false, "sp", 1_800_000_000, crate::plan::v2::NO_RULES_DIGEST).unwrap();
        assert_eq!((plan.body.plan_kind, plan.body.policy_matchable, plan.body.requires_admin), (PlanKind::Startup, false, false));
        let PlanAction::StartupToggle(t) = &plan.body.items[0] else { panic!() };
        assert_eq!(t.observed_sha256.as_deref(), Some(observation_digest(item.location.as_ref().unwrap(), &obs).as_str()));
        let changed = StartupObservation { approval: Some(RegValue::new(3, &approval_record(false, 1))), ..obs.clone() };
        assert_ne!(observation_digest(item.location.as_ref().unwrap(), &obs), observation_digest(item.location.as_ref().unwrap(), &changed));
        let gone = StartupObservation { definition: None, approval: None };
        assert_eq!(build_startup_plan(&item, StartupOperation::Disable, &gone, false, "sp", 0, "x").unwrap_err().code, "conflict");
        assert_eq!(unhex(&hex(&[0, 255, 16])), Some(vec![0, 255, 16]));
        assert_eq!(unhex("abc"), None);
    }

    #[test]
    fn macos_locations_map_to_their_sources_and_scope() {
        let plist = RawPath::unix(b"/Users/a/Library/LaunchAgents/com.example.agent.plist".to_vec()).unwrap();
        let user = StartupLocation::LaunchdUser { root: String::new(), plist: plist.clone(), label: "com.example.agent".into() };
        let system = StartupLocation::LaunchdSystem { root: String::new(), plist, label: "com.example.daemon".into() };
        let login = StartupLocation::SmItem { root: String::new(), identifier: "com.example.helper".into(), background: false };
        let background = StartupLocation::SmItem { root: String::new(), identifier: "com.example.bg".into(), background: true };
        let got: Vec<(StartupSource, bool, &str, &str)> =
            [&user, &system, &login, &background].iter().map(|l| (l.source(), l.system_wide(), l.approval_value(), l.approval_key())).collect();
        assert_eq!(got, [
            (StartupSource::LaunchdUser, false, "com.example.agent", ""),
            (StartupSource::LaunchdSystem, true, "com.example.daemon", ""),
            (StartupSource::SmLoginItem, false, "com.example.helper", ""),
            (StartupSource::SmBackgroundItem, false, "com.example.bg", ""),
        ]);
        assert_eq!(serde_json::to_value(PublisherEvidence::CodeSignature).unwrap(), "code_signature");
    }
}
