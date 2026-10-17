//! Reversible setting changes of the maintenance catalog (parity 14.5, D16 N15, MNT-07).
//!
//! A setting task is planned as a `settings_change` action (not a `maintenance_task`): the plan freezes the value
//! observed when it was made (`from_value`) and the one it writes (`to_value`). At run time the prior value is stored as
//! a [`SettingBackup`] **before** anything is written, and the write only happens while the setting still has the
//! reviewed value. A finished run's backups become a `recovery` plan of `settings_change` actions (one per backup; the
//! action id is the backup id) that put the prior value back only while the setting still has the value written.
//!
//! Values are text: `true`, `false` or [`ABSENT`] (the key is not set; restoring it removes the key). Anything else
//! (another value type) is never written: such a setting is "state unclear" and left alone.

use serde::{Deserialize, Serialize};

use super::{Refusal, TaskPlatform, ids, refuse};
use crate::plan::v2::{ActionMeta, HeaderInput, PlanAction, PlanKind, PlanV2, SettingsChangeAction};

/// The value of a setting whose key is not set.
pub const ABSENT: &str = "absent";
/// Backup ids of setting changes (startup and uninstall backups use other prefixes).
pub const SETTING_BACKUP_PREFIX: &str = "setb_";

/// One reversible setting a catalog task changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingSpec {
    pub id: &'static str,
    /// The catalog task that plans this setting.
    pub task_id: &'static str,
    pub platform: TaskPlatform,
    /// Preference domain (application ID) and key.
    pub domain: &'static str,
    pub key: &'static str,
    /// The value the task writes.
    pub target: &'static str,
    pub title: &'static str,
}

pub const SETTINGS: &[SettingSpec] = &[SettingSpec {
    id: "macos.desktopservices.DSDontWriteNetworkStores",
    task_id: ids::MAC_NETWORK_DSSTORE,
    platform: TaskPlatform::Macos,
    domain: "com.apple.desktopservices",
    key: "DSDontWriteNetworkStores",
    target: "true",
    title: "Finder does not write .DS_Store files on network volumes",
}];

pub fn by_id(setting_id: &str) -> Option<&'static SettingSpec> {
    SETTINGS.iter().find(|s| s.id == setting_id)
}

/// The setting a catalog task changes, if it is a setting task.
pub fn for_task(task_id: &str) -> Option<&'static SettingSpec> {
    SETTINGS.iter().find(|s| s.task_id == task_id)
}

/// A value this build reads and writes (`true`, `false`, [`ABSENT`]).
pub fn is_known_value(v: &str) -> bool {
    matches!(v, "true" | "false" | ABSENT)
}

/// Reads and writes the settings of [`SETTINGS`] (cm-platform, or an in-memory fake in tests).
pub trait SettingsActions {
    /// The current value: `true`, `false`, [`ABSENT`], or `other:<type>` for a value of another type.
    fn read(&self, s: &SettingSpec) -> Result<String, String>;
    /// Write `value` (`true`/`false`; [`ABSENT`] removes the key). Anything else is refused.
    fn write(&self, s: &SettingSpec, value: &str) -> Result<(), String>;
    /// Where the setting is stored (shown in results; tests point it at a fixture).
    fn location(&self, s: &SettingSpec) -> String {
        format!("{} {}", s.domain, s.key)
    }
}

/// The prior value of one setting change, stored before the write (durable, restorable later).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingBackup {
    pub backup_id: String,
    pub setting_id: String,
    /// Where it was written (preference domain, or a fixture path in tests).
    pub location: String,
    pub prior_value: String,
    pub written_value: String,
    pub created_at: String,
}

impl SettingBackup {
    pub fn sha256(&self) -> String {
        crate::c14n::digest(&serde_json::to_value(self).expect("backup serializes")).expect("plain JSON")
    }
}

/// The planned action of a setting task, from the value observed now.
pub fn change_action(spec: &SettingSpec, action_id: String, observed: &str) -> Result<PlanAction, Refusal> {
    if !is_known_value(observed) {
        return Err(refuse("invalid_argument", format!("{} currently has a value of another type ({observed}); it is not changed", spec.title)));
    }
    Ok(PlanAction::SettingsChange(SettingsChangeAction { action_id, setting_id: spec.id.into(), from_value: observed.into(), to_value: spec.target.into(), restore_on_exit: false }))
}

/// Freeze recorded setting backups into a `recovery` plan: each action writes the prior value back while the setting
/// still has the value the change wrote (the action id is the backup id).
pub fn build_settings_recovery_plan(backups: &[SettingBackup], plan_id: &str, created_unix: i64, policy_digest: &str) -> Result<PlanV2, Refusal> {
    if backups.is_empty() {
        return Err(refuse("invalid_argument", "the operation recorded no setting change to restore (or it was restored already)"));
    }
    let mut items = Vec::new();
    let mut meta = Vec::new();
    for b in backups {
        let Some(spec) = by_id(&b.setting_id) else { return Err(refuse("unsupported", format!("setting {} is not known to this build", b.setting_id))) };
        if !is_known_value(&b.prior_value) || !is_known_value(&b.written_value) {
            return Err(refuse("unsupported", format!("backup {} holds a value this build does not write", b.backup_id)));
        }
        items.push(PlanAction::SettingsChange(SettingsChangeAction {
            action_id: b.backup_id.clone(),
            setting_id: b.setting_id.clone(),
            from_value: b.written_value.clone(),
            to_value: b.prior_value.clone(),
            restore_on_exit: false,
        }));
        meta.push(ActionMeta { note: Some(format!("restore: {} ({})", spec.title, b.location)), ..Default::default() });
    }
    let header = HeaderInput {
        plan_id,
        plan_kind: PlanKind::Recovery,
        created_unix,
        scope_id: None,
        scan_id: None,
        scan_revision: None,
        rule_set_digest: crate::plan::v2::NO_RULES_DIGEST,
        policy_digest,
    };
    crate::plan::v2::build(&header, items, meta).map_err(|e| refuse("internal_error", e.to_string()))
}
