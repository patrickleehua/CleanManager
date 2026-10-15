//! Platform-neutral execution interface (EXE-01/02/05/08). The application layer drives the
//! durable intent → live check → action → result sequence; a platform implements the checks
//! and the three file actions. Implementations must act on the object they verified (handle
//! based where the OS allows), never fall back from trash/quarantine to permanent delete, never
//! follow links, and report `Unknown` instead of guessing when an outcome cannot be confirmed.

use serde::{Deserialize, Serialize};

use crate::model::Identity;
use crate::obs::ObservationRecord;
use crate::path::RawPath;
use crate::plan::WireIdentity;

/// Everything the platform needs to re-verify and act on one planned file.
#[derive(Debug, Clone)]
pub struct ActionTarget<'a> {
    pub action_id: &'a str,
    pub path: &'a RawPath,
    pub identity: &'a WireIdentity,
    /// Identities of the planned parent chain (immediate parent first).
    pub parent_chain: &'a [Identity],
    pub observation: &'a ObservationRecord,
    /// Probes required by the rules that produced the candidate (e.g. `owner_process`).
    pub probes: &'a [String],
    /// Owner of the rule (e.g. `microsoft.edge`) for the owner-process probe.
    pub owner_id: Option<&'a str>,
}

/// Result of re-observing a target immediately before acting (EXE-01).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "check", content = "detail")]
pub enum LiveCheck {
    Match,
    Missing,
    IdentityChanged,
    ParentChanged,
    MetadataChanged(String),
    InUse,
    OwnerRunning,
    LinkOrReparse,
    Protected(String),
    Unsupported(String),
    /// The check itself could not be completed; never treated as a match.
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Recovery {
    /// In the platform recycle bin / Trash; restored with the system UI.
    Trash,
    /// Same-volume quarantine location this tool can restore from.
    Quarantine { location: RawPath },
    /// Permanent delete: nothing to restore.
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum ActionOutcome {
    Succeeded { recovery: Recovery },
    /// Precondition not met; nothing was changed.
    Skipped { code: String, message: String },
    /// The OS refused; nothing was changed.
    Failed { code: String, message: String },
    /// The OS call may or may not have taken effect.
    Unknown { message: String },
}

/// What reconciliation found for an action whose result was never recorded (EXE-05).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reconciled {
    /// The original object is still at its planned place: the action did not happen.
    NotPerformed,
    /// The object is at the quarantine location (or provably gone for delete).
    Performed,
    /// Evidence is insufficient; stays unknown for a person to decide.
    StillUnknown,
}

/// One planned object under a directory that [`FileActions::trash_tree`] may move as a whole.
#[derive(Debug, Clone, Copy)]
pub struct TreeMember<'a> {
    pub path: &'a RawPath,
    pub identity: &'a WireIdentity,
    pub observation: &'a ObservationRecord,
    pub directory: bool,
}

fn unsupported(what: &str) -> ActionOutcome {
    ActionOutcome::Skipped { code: "unsupported".into(), message: format!("{what} is not supported by this platform backend") }
}

pub trait FileActions {
    fn check(&self, target: &ActionTarget<'_>) -> LiveCheck;
    /// Move into `quarantine_dir` (same volume) under `name`, never replacing anything there.
    fn quarantine(&self, target: &ActionTarget<'_>, quarantine_dir: &RawPath, name: &str) -> ActionOutcome;
    /// Move to the platform recycle bin; refuse (never permanently delete) if that is impossible.
    fn trash(&self, target: &ActionTarget<'_>) -> ActionOutcome;
    /// Permanent delete of exactly the verified object.
    fn delete(&self, target: &ActionTarget<'_>) -> ActionOutcome;
    /// Same-volume quarantine directory for this target, if the platform supports one.
    fn quarantine_dir_for(&self, target: &ActionTarget<'_>, operation_id: &str) -> Option<RawPath>;
    /// Reconcile an action with an intent but no recorded result.
    fn reconcile(&self, target: &ActionTarget<'_>, mode: crate::rules::Mode, quarantine_location: Option<&RawPath>) -> Reconciled;
    /// Free bytes on the volume holding `path`, for before/after observations (REC-05).
    fn free_space(&self, path: &RawPath) -> Option<u64>;
    /// Remove a planned directory only if it is still the planned object, is not a link or
    /// reparse point, passes the platform checks and is empty at this moment; never recursive.
    /// Anything new inside is `Skipped { code: "new_descendant" }`.
    fn remove_empty_dir(&self, target: &ActionTarget<'_>) -> ActionOutcome {
        let _ = target;
        unsupported("empty-directory removal")
    }
    /// Move a planned directory to the recycle bin as one object, only when its live subtree is
    /// exactly `members` (same identities and metadata, nothing extra, no links). Any skip or
    /// failure leaves everything in place so the caller can fall back to per-item actions.
    fn trash_tree(&self, root: &ActionTarget<'_>, members: &[TreeMember<'_>]) -> ActionOutcome {
        let _ = (root, members);
        unsupported("whole-directory recycling")
    }
}
