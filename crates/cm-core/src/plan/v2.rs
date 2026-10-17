//! Plan kinds and typed actions (contract `plan/v2`, add-mole-parity-suite D1/D6).
//!
//! - `plan_kind` is part of the body, so it is covered by the CM-C14N-v1 digest: a receipt for a
//!   plan of one kind can never authorize a body of another kind.
//! - Each kind has a fixed set of allowed actions. [`validate`] enforces it when a plan is built and
//!   again when a stored plan is applied (a body that violates its kind is refused before any action).
//! - Only `cleanup` may be matched by a bounded policy ([`PlanKind::policy_matchable`]); every other kind
//!   needs an interactive review of the exact plan. `requires_admin` must equal what the actions need.
//! - `manual_selection` only moves objects at their original place to the platform trash (`fs_trash`,
//!   `original`) or permanently deletes objects already in it (`fs_delete`, `platform_trash`), never both
//!   in one plan.
//!
//! `plan/v1` (cleanup) and `recovery/v1` (quarantine restore/purge) bodies are unchanged and stay valid;
//! they are the `cleanup` and quarantine-`recovery` kinds of the older contracts.

use serde::{Deserialize, Serialize};

use crate::bytes::Bytes;
use crate::c14n::{C14nError, digest};
use crate::obs::ObservationRecord;
use crate::path::RawPath;
use crate::plan::{ItemKind, PLAN_TTL_SECONDS, StaleReason, WireIdentity, parse_utc};
use crate::startup::{StartupLocation, StartupScope, StartupSource};

pub const PLAN_V2_CONTRACT: &str = "plan/v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanKind {
    Cleanup,
    ManualSelection,
    Uninstall,
    Maintenance,
    Startup,
    Update,
    Recovery,
    /// End one running process (STS-04): exactly one `process_end` action, reviewed each time.
    Process,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionType {
    FsTrash,
    FsQuarantine,
    FsDelete,
    ToolAdapter,
    AppQuit,
    RegistryBackupDelete,
    RegistryRestore,
    StartupToggle,
    MaintenanceTask,
    SettingsChange,
    ProcessEnd,
}

impl ActionType {
    pub fn as_str(self) -> &'static str {
        match self {
            ActionType::FsTrash => "fs_trash",
            ActionType::FsQuarantine => "fs_quarantine",
            ActionType::FsDelete => "fs_delete",
            ActionType::ToolAdapter => "tool_adapter",
            ActionType::AppQuit => "app_quit",
            ActionType::RegistryBackupDelete => "registry_backup_delete",
            ActionType::RegistryRestore => "registry_restore",
            ActionType::StartupToggle => "startup_toggle",
            ActionType::MaintenanceTask => "maintenance_task",
            ActionType::SettingsChange => "settings_change",
            ActionType::ProcessEnd => "process_end",
        }
    }
}

impl PlanKind {
    pub const ALL: [PlanKind; 8] = [
        PlanKind::Cleanup,
        PlanKind::ManualSelection,
        PlanKind::Uninstall,
        PlanKind::Maintenance,
        PlanKind::Startup,
        PlanKind::Update,
        PlanKind::Recovery,
        PlanKind::Process,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            PlanKind::Cleanup => "cleanup",
            PlanKind::ManualSelection => "manual_selection",
            PlanKind::Uninstall => "uninstall",
            PlanKind::Maintenance => "maintenance",
            PlanKind::Startup => "startup",
            PlanKind::Update => "update",
            PlanKind::Recovery => "recovery",
            PlanKind::Process => "process",
        }
    }

    /// The protocol table "plan/v2 计划种类与动作" (docs/cli-protocol.md).
    pub fn allowed_actions(self) -> &'static [ActionType] {
        use ActionType::*;
        match self {
            PlanKind::Cleanup => &[FsTrash, FsQuarantine, FsDelete, ToolAdapter],
            PlanKind::ManualSelection => &[FsTrash, FsDelete],
            PlanKind::Uninstall => &[AppQuit, ToolAdapter, FsTrash, FsQuarantine, RegistryBackupDelete, StartupToggle],
            PlanKind::Maintenance => &[MaintenanceTask, SettingsChange],
            PlanKind::Startup => &[StartupToggle],
            PlanKind::Update => &[AppQuit, ToolAdapter],
            PlanKind::Recovery => &[RegistryRestore, StartupToggle, SettingsChange],
            PlanKind::Process => &[ProcessEnd],
        }
    }

    /// Only cleanup plans may ever be matched by a bounded preauthorization policy.
    pub fn policy_matchable(self) -> bool {
        self == PlanKind::Cleanup
    }
}

/// Registry hive of a registry action. Only these two are ever touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Hive {
    #[serde(rename = "HKCU")]
    Hkcu,
    #[serde(rename = "HKLM")]
    Hklm,
}

impl Hive {
    pub fn as_str(self) -> &'static str {
        match self {
            Hive::Hkcu => "HKCU",
            Hive::Hklm => "HKLM",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetLocation {
    /// The object at its original place.
    Original,
    /// The object is already inside the platform recycle bin / Trash.
    PlatformTrash,
}

/// Protocol `plan_target`: the object a file action binds (identity and CM-OBS-v1 fingerprint).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanTarget {
    pub entry_id: String,
    pub candidate_id: Option<String>,
    pub path: RawPath,
    pub identity: WireIdentity,
    pub observed_fingerprint_sha256: String,
    pub logical_bytes: Bytes,
    pub allocated_bytes: Option<Bytes>,
    /// Absent = file (as in `plan/v1`); `directory` is digest-covered.
    #[serde(default, skip_serializing_if = "ItemKind::is_file")]
    pub kind: ItemKind,
}

/// What a whole-directory action moves: the scanned members under the directory, bound by digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubtreeBinding {
    /// Members below the directory (files and directories, the directory itself excluded).
    pub items: u64,
    pub logical_bytes: Bytes,
    /// [`members_digest`] of the members recorded with the plan.
    pub members_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FsAction {
    pub action_id: String,
    pub target: PlanTarget,
    pub target_location: TargetLocation,
    pub must_not_follow_links: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtree: Option<SubtreeBinding>,
    /// Recycle-bin entry this action belongs to (a purge of one entry is several ordered actions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trash_entry_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutableRef {
    pub path: RawPath,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolAdapterAction {
    pub action_id: String,
    pub adapter_id: String,
    pub adapter_version: String,
    pub executable: ExecutableRef,
    pub argv: Vec<String>,
    pub preview_digest: String,
    pub timeout_seconds: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuitForce {
    Never,
    AskUser,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppQuitAction {
    pub action_id: String,
    pub app_instance_id: String,
    pub app_identity: String,
    pub force: QuitForce,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryBackupDeleteAction {
    pub action_id: String,
    pub hive: Hive,
    pub key_path: String,
    pub value_name: Option<String>,
    pub owner_evidence: String,
    pub backup_required: bool,
    /// Uninstall leftovers (UNI-06): [`crate::uninstall::key_digest`] of the key as planned; anything changed since is skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryRestoreAction {
    pub action_id: String,
    pub backup_id: String,
    pub hive: Hive,
    pub key_path: String,
    /// Absent: the backup is of a whole key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_name: Option<String>,
    /// SHA-256 of the stored backup record; a different record is never restored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_sha256: Option<String>,
    /// Always `skip`: an existing value or key is never overwritten.
    pub on_existing: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupOperation {
    Enable,
    Disable,
    RemoveWithBackup,
    Restore,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartupToggleAction {
    pub action_id: String,
    pub startup_item_id: String,
    pub source: StartupSource,
    pub operation: StartupOperation,
    pub scope: StartupScope,
    /// Where the entry is defined (display).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<String>,
    /// [`crate::startup::observation_digest`] of the definition and its enable record at planning time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_sha256: Option<String>,
    /// `restore` only: the prior-state record written by the operation being undone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceTaskAction {
    pub action_id: String,
    pub task_id: String,
    pub task_version: String,
    pub requires_admin: bool,
    /// The volume a volume task acts on (mount point chosen when planning, e.g. Spotlight rebuild); digest-covered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsChangeAction {
    pub action_id: String,
    pub setting_id: String,
    pub from_value: String,
    pub to_value: String,
    pub restore_on_exit: bool,
}

/// End one process instance (STS-04). `pid` plus `started_at` (creation time, FILETIME ticks as a decimal string)
/// name exactly one process: a reused PID is a different process and is never ended. `image_sha256` binds the
/// executable observed at planning time when it could be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessEndAction {
    pub action_id: String,
    pub pid: u32,
    pub started_at: String,
    pub name: String,
    pub image_path: Option<RawPath>,
    pub image_sha256: Option<String>,
}

/// Protocol `plan_action`, tagged by `action_type`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action_type", rename_all = "snake_case")]
pub enum PlanAction {
    FsTrash(FsAction),
    FsQuarantine(FsAction),
    FsDelete(FsAction),
    ToolAdapter(ToolAdapterAction),
    AppQuit(AppQuitAction),
    RegistryBackupDelete(RegistryBackupDeleteAction),
    RegistryRestore(RegistryRestoreAction),
    StartupToggle(StartupToggleAction),
    MaintenanceTask(MaintenanceTaskAction),
    SettingsChange(SettingsChangeAction),
    ProcessEnd(ProcessEndAction),
}

impl PlanAction {
    pub fn action_type(&self) -> ActionType {
        match self {
            PlanAction::FsTrash(_) => ActionType::FsTrash,
            PlanAction::FsQuarantine(_) => ActionType::FsQuarantine,
            PlanAction::FsDelete(_) => ActionType::FsDelete,
            PlanAction::ToolAdapter(_) => ActionType::ToolAdapter,
            PlanAction::AppQuit(_) => ActionType::AppQuit,
            PlanAction::RegistryBackupDelete(_) => ActionType::RegistryBackupDelete,
            PlanAction::RegistryRestore(_) => ActionType::RegistryRestore,
            PlanAction::StartupToggle(_) => ActionType::StartupToggle,
            PlanAction::MaintenanceTask(_) => ActionType::MaintenanceTask,
            PlanAction::SettingsChange(_) => ActionType::SettingsChange,
            PlanAction::ProcessEnd(_) => ActionType::ProcessEnd,
        }
    }

    pub fn action_id(&self) -> &str {
        match self {
            PlanAction::FsTrash(a) | PlanAction::FsQuarantine(a) | PlanAction::FsDelete(a) => &a.action_id,
            PlanAction::ToolAdapter(a) => &a.action_id,
            PlanAction::AppQuit(a) => &a.action_id,
            PlanAction::RegistryBackupDelete(a) => &a.action_id,
            PlanAction::RegistryRestore(a) => &a.action_id,
            PlanAction::StartupToggle(a) => &a.action_id,
            PlanAction::MaintenanceTask(a) => &a.action_id,
            PlanAction::SettingsChange(a) => &a.action_id,
            PlanAction::ProcessEnd(a) => &a.action_id,
        }
    }

    /// The file action, for the three file action types.
    pub fn fs(&self) -> Option<&FsAction> {
        match self {
            PlanAction::FsTrash(a) | PlanAction::FsQuarantine(a) | PlanAction::FsDelete(a) => Some(a),
            _ => None,
        }
    }

    /// Whether this action alone needs an administrator (system-scope startup items, HKLM, admin tasks).
    pub fn needs_admin(&self) -> bool {
        match self {
            PlanAction::StartupToggle(a) => a.scope == StartupScope::System,
            PlanAction::RegistryBackupDelete(a) => a.hive == Hive::Hklm,
            PlanAction::RegistryRestore(a) => a.hive == Hive::Hklm,
            PlanAction::MaintenanceTask(a) => a.requires_admin,
            _ => false,
        }
    }

    /// Logical bytes of a file action's target.
    pub fn logical_bytes(&self) -> u64 {
        self.fs().map(|a| a.target.logical_bytes.0).unwrap_or(0)
    }
}

/// Protocol `plan_body_v2`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanBodyV2 {
    pub engine_contract: String,
    pub plan_id: String,
    pub plan_kind: PlanKind,
    pub policy_matchable: bool,
    pub requires_admin: bool,
    pub scope_id: Option<String>,
    pub scan_id: Option<String>,
    pub scan_revision: Option<u32>,
    pub rule_set_digest: String,
    pub policy_digest: String,
    pub created_at: String,
    pub expires_at: String,
    pub items: Vec<PlanAction>,
}

impl PlanBodyV2 {
    pub fn digest(&self) -> Result<String, C14nError> {
        digest(&serde_json::to_value(self).expect("plan body serializes"))
    }

    pub fn total_logical(&self) -> u64 {
        self.items.iter().map(PlanAction::logical_bytes).fold(0u64, u64::saturating_add)
    }
}

/// Why a body does not satisfy the rules of its kind.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KindError {
    #[error("engine_contract must be plan/v2")]
    Contract,
    #[error("a plan needs at least one action")]
    Empty,
    #[error("action id {0} is used twice")]
    DuplicateAction(String),
    #[error("plan kind {} does not allow action {} ({action_id})", .kind.as_str(), .action.as_str())]
    ActionNotAllowed { kind: PlanKind, action: ActionType, action_id: String },
    #[error("plan kind {} can never be matched by a policy", .0.as_str())]
    PolicyMatchable(PlanKind),
    #[error("action {action_id}: {detail}")]
    Action { action_id: String, detail: String },
    #[error("requires_admin is {stated} but the actions need {needed}")]
    RequiresAdmin { stated: bool, needed: bool },
    #[error("a manual selection plan either moves objects to the trash or deletes objects already in it, not both")]
    MixedManualSelection,
    #[error("a process plan ends exactly one process (each end is confirmed on its own)")]
    ProcessNotSingle,
}

/// Enforce the per-kind rules. Called when a plan is built and again before a stored plan is applied.
pub fn validate(body: &PlanBodyV2) -> Result<(), KindError> {
    if body.engine_contract != PLAN_V2_CONTRACT {
        return Err(KindError::Contract);
    }
    if body.items.is_empty() {
        return Err(KindError::Empty);
    }
    if body.policy_matchable && !body.plan_kind.policy_matchable() {
        return Err(KindError::PolicyMatchable(body.plan_kind));
    }
    let mut ids = std::collections::BTreeSet::new();
    let action_err = |a: &PlanAction, detail: &str| KindError::Action { action_id: a.action_id().to_owned(), detail: detail.to_owned() };
    for a in &body.items {
        if !ids.insert(a.action_id()) {
            return Err(KindError::DuplicateAction(a.action_id().to_owned()));
        }
        let t = a.action_type();
        if !body.plan_kind.allowed_actions().contains(&t) {
            return Err(KindError::ActionNotAllowed { kind: body.plan_kind, action: t, action_id: a.action_id().to_owned() });
        }
        if let Some(fs) = a.fs() {
            if !fs.must_not_follow_links {
                return Err(action_err(a, "file actions never follow links"));
            }
            if body.plan_kind == PlanKind::ManualSelection {
                match (t, fs.target_location) {
                    (ActionType::FsTrash, TargetLocation::Original) | (ActionType::FsDelete, TargetLocation::PlatformTrash) => {}
                    (ActionType::FsTrash, _) => return Err(action_err(a, "manual selection moves only objects at their original place to the trash")),
                    _ => return Err(action_err(a, "manual selection permanently deletes only objects already in the platform trash")),
                }
            }
            if fs.subtree.is_some() && fs.target.kind != ItemKind::Directory {
                return Err(action_err(a, "only a directory target binds a subtree"));
            }
        }
        match a {
            PlanAction::RegistryRestore(r) if r.on_existing != "skip" => return Err(action_err(a, "registry restore never overwrites (on_existing must be skip)")),
            PlanAction::RegistryBackupDelete(r) if !r.backup_required => return Err(action_err(a, "registry deletes always back up first")),
            PlanAction::StartupToggle(s) => {
                let restore = s.operation == StartupOperation::Restore;
                if restore != (body.plan_kind == PlanKind::Recovery) {
                    return Err(action_err(a, "startup restore belongs to recovery plans, other startup changes do not"));
                }
                if restore && s.backup_id.is_none() {
                    return Err(action_err(a, "a startup restore names the recorded prior state (backup_id)"));
                }
            }
            _ => {}
        }
    }
    if body.plan_kind == PlanKind::Process && body.items.len() != 1 {
        return Err(KindError::ProcessNotSingle);
    }
    if body.plan_kind == PlanKind::ManualSelection {
        let first = body.items[0].action_type();
        if body.items.iter().any(|a| a.action_type() != first) {
            return Err(KindError::MixedManualSelection);
        }
    }
    let needed = body.items.iter().any(PlanAction::needs_admin);
    if body.requires_admin != needed {
        return Err(KindError::RequiresAdmin { stated: body.requires_admin, needed });
    }
    Ok(())
}

/// One member of a whole-directory action (not digest-covered by itself: see [`members_digest`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberMeta {
    pub path: RawPath,
    pub identity: WireIdentity,
    pub observation: ObservationRecord,
    pub directory: bool,
}

/// Digest of a directory action's members: identity, kind and observation fingerprint, sorted by identity.
pub fn members_digest(members: &[MemberMeta]) -> Result<String, C14nError> {
    let mut rows: Vec<serde_json::Value> = Vec::with_capacity(members.len());
    for m in members {
        rows.push(serde_json::json!({
            "volume_id": m.identity.volume_id, "file_id": m.identity.file_id, "directory": m.directory, "fingerprint": m.observation.fingerprint()?,
        }));
    }
    rows.sort_by(|a, b| (a["volume_id"].as_str(), a["file_id"].as_str()).cmp(&(b["volume_id"].as_str(), b["file_id"].as_str())));
    digest(&serde_json::Value::Array(rows))
}

/// Execution data per action (same order as `body.items`). Each piece is checked against the digest-covered
/// body before use: observations by fingerprint, members by [`members_digest`], startup locations by the
/// live observation digest the platform recomputes.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ActionMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<ObservationRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<MemberMeta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup: Option<StartupLocation>,
    /// Text for people (e.g. a recycled item's original location). Never used to decide anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// `uninstall` plans (and restores of their registry backups): the bound record or registry location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uninstall: Option<crate::uninstall::UninstallMeta>,
}

/// A stored `plan/v2` plan: body, digest and per-action execution data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanV2 {
    pub body: PlanBodyV2,
    pub digest: String,
    pub meta: Vec<ActionMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    #[error(transparent)]
    Kind(#[from] KindError),
    #[error("execution data does not match action {0}")]
    Meta(String),
    #[error("{0}")]
    Canonical(#[from] C14nError),
}

/// What makes a stored or built plan unusable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanProblem {
    #[error("plan is stale: {0:?}")]
    Stale(StaleReason),
    #[error(transparent)]
    Kind(#[from] KindError),
}

/// Common header fields of a new v2 body.
pub struct HeaderInput<'a> {
    pub plan_id: &'a str,
    pub plan_kind: PlanKind,
    pub created_unix: i64,
    pub scope_id: Option<&'a str>,
    pub scan_id: Option<&'a str>,
    pub scan_revision: Option<u32>,
    /// Rules or catalogs the plan depends on; kinds without rules use [`NO_RULES_DIGEST`].
    pub rule_set_digest: &'a str,
    pub policy_digest: &'a str,
}

/// `rule_set_digest` of kinds that do not depend on a rule set (SHA-256 of the empty JSON array).
pub const NO_RULES_DIGEST: &str = "4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945";

/// Freeze actions and their execution data into a validated plan.
pub fn build(header: &HeaderInput<'_>, items: Vec<PlanAction>, meta: Vec<ActionMeta>) -> Result<PlanV2, BuildError> {
    let body = PlanBodyV2 {
        engine_contract: PLAN_V2_CONTRACT.into(),
        plan_id: header.plan_id.into(),
        plan_kind: header.plan_kind,
        // A new plan is only marked matchable for the kind that may ever be matched.
        policy_matchable: header.plan_kind.policy_matchable(),
        requires_admin: items.iter().any(PlanAction::needs_admin),
        scope_id: header.scope_id.map(str::to_owned),
        scan_id: header.scan_id.map(str::to_owned),
        scan_revision: header.scan_revision,
        rule_set_digest: header.rule_set_digest.into(),
        policy_digest: header.policy_digest.into(),
        created_at: crate::time::utc_from_unix(header.created_unix),
        expires_at: crate::time::utc_from_unix(header.created_unix + PLAN_TTL_SECONDS),
        items,
    };
    validate(&body)?;
    let digest = body.digest()?;
    let plan = PlanV2 { body, digest, meta };
    check_meta(&plan)?;
    Ok(plan)
}

/// The execution data must be the one the digest-covered body describes.
pub fn check_meta(plan: &PlanV2) -> Result<(), BuildError> {
    if plan.meta.len() != plan.body.items.len() {
        return Err(BuildError::Meta("(count)".into()));
    }
    for (a, m) in plan.body.items.iter().zip(&plan.meta) {
        let bad = || BuildError::Meta(a.action_id().to_owned());
        if let Some(fs) = a.fs() {
            let obs = m.observation.as_ref().ok_or_else(bad)?;
            if obs.fingerprint()? != fs.target.observed_fingerprint_sha256
                || obs.path != fs.target.path
                || obs.entity.volume_id != fs.target.identity.volume_id
                || obs.entity.wire_file_id() != fs.target.identity.file_id
            {
                return Err(bad());
            }
            match &fs.subtree {
                Some(s) if s.items != m.members.len() as u64 || members_digest(&m.members)? != s.members_sha256 => return Err(bad()),
                None if !m.members.is_empty() => return Err(bad()),
                _ => {}
            }
        }
        if let PlanAction::StartupToggle(s) = a
            && s.operation != StartupOperation::Restore
            && (m.startup.is_none() || s.observed_sha256.is_none())
        {
            return Err(bad());
        }
        if crate::uninstall::binds_meta(plan.body.plan_kind, a) {
            crate::uninstall::check_action_meta(a, m).map_err(|_| bad())?;
        }
    }
    Ok(())
}

/// Plan-level validity at `now`: digest, kind rules, execution data, expiry. Item-level live checks happen at execution.
pub fn check_plan_v2(plan: &PlanV2, now_unix: i64) -> Result<(), PlanProblem> {
    if plan.body.digest().ok().as_deref() != Some(plan.digest.as_str()) {
        return Err(PlanProblem::Stale(StaleReason::DigestMismatch));
    }
    validate(&plan.body)?;
    if check_meta(plan).is_err() {
        return Err(PlanProblem::Stale(StaleReason::DigestMismatch));
    }
    match parse_utc(&plan.body.expires_at) {
        Some(e) if now_unix < e => Ok(()),
        _ => Err(PlanProblem::Stale(StaleReason::Expired)),
    }
}

#[cfg(test)]
mod tests;
