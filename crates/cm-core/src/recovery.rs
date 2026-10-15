//! Recovery plans (REC-01..04, contract `recovery/v1`): restore or purge quarantined objects.
//!
//! A recovery plan is its own fixed plan type with its own digest, TTL and receipts; it never
//! reuses a cleanup approval. Items bind the quarantined object's identity, the original
//! parent's identity and the original path. Restore never overwrites: an occupied original
//! name is skipped (`on_conflict: skip`). Purge permanently deletes exactly the verified
//! quarantined object. Retention reminders only remind; nothing here deletes on a timer.

use serde::{Deserialize, Serialize};

use crate::authz::ReceiptSource;
use crate::bytes::Bytes;
use crate::c14n::{C14nError, digest};
use crate::plan::{PLAN_TTL_SECONDS, StaleReason, WireIdentity, parse_utc};
use crate::path::RawPath;

pub const RECOVERY_CONTRACT: &str = "recovery/v1";
/// Default retention reminder after quarantine (D9). Reaching it never deletes anything.
pub const RETENTION_REMINDER_SECONDS: i64 = 30 * 86_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryKind {
    Restore,
    Purge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Original,
    Alternate,
    PermanentDelete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryTarget {
    pub kind: TargetKind,
    pub path: Option<RawPath>,
}

/// Protocol `recovery_item`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryItem {
    pub action_id: String,
    pub quarantine_id: String,
    pub quarantined_path: RawPath,
    pub quarantined_identity: WireIdentity,
    pub original_parent_identity: WireIdentity,
    pub original_path: RawPath,
    pub target: RecoveryTarget,
    /// Always `skip`: an occupied destination is never replaced (REC-02).
    pub on_conflict: String,
    pub logical_bytes: Bytes,
}

/// Protocol `recovery_body`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryBody {
    pub engine_contract: String,
    pub plan_id: String,
    pub recovery_kind: RecoveryKind,
    pub created_at: String,
    pub expires_at: String,
    pub items: Vec<RecoveryItem>,
}

impl RecoveryBody {
    pub fn digest(&self) -> Result<String, C14nError> {
        digest(&serde_json::to_value(self).expect("recovery body serializes"))
    }
}

/// A stored recovery plan (authoritative body plus its digest).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryPlan {
    pub body: RecoveryBody,
    pub digest: String,
}

/// Durable quarantine manifest, as the executor records it next to each quarantined object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantineManifest {
    pub original_path: RawPath,
    pub original_identity: WireIdentity,
    /// Missing when the planned object had no recorded parent: restore cannot be verified.
    pub original_parent: Option<WireIdentity>,
    pub location: RawPath,
    pub logical_bytes: Bytes,
}

/// Why a manifest cannot back a restore (REC-01: never reported as recoverable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestProblem {
    Unreadable(String),
    NoParentIdentity,
    WeakIdentity,
}

impl QuarantineManifest {
    /// Parse a stored manifest; anything that cannot verify a restore is an incomplete manifest.
    pub fn parse(json: &str) -> Result<QuarantineManifest, ManifestProblem> {
        let m: QuarantineManifest = serde_json::from_str(json).map_err(|e| ManifestProblem::Unreadable(e.to_string()))?;
        match &m.original_parent {
            None => Err(ManifestProblem::NoParentIdentity),
            Some(p) if p.quality != "strong" || m.original_identity.quality != "strong" => Err(ManifestProblem::WeakIdentity),
            Some(_) => Ok(m),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecoveryPlanError {
    #[error("empty selection")]
    Empty,
    #[error("quarantine item {0} is listed twice")]
    Duplicate(String),
    #[error("quarantine item {0} has an incomplete manifest and cannot be restored or purged by this tool")]
    ManifestIncomplete(String),
    #[error("{0}")]
    Canonical(#[from] C14nError),
}

/// Freeze a selection of quarantined objects into a recovery plan.
pub fn build_recovery_plan(kind: RecoveryKind, selected: &[(String, QuarantineManifest)], plan_id: &str, created_unix: i64) -> Result<RecoveryPlan, RecoveryPlanError> {
    if selected.is_empty() {
        return Err(RecoveryPlanError::Empty);
    }
    let mut items = Vec::with_capacity(selected.len());
    for (i, (qid, m)) in selected.iter().enumerate() {
        if selected[..i].iter().any(|(other, _)| other == qid) {
            return Err(RecoveryPlanError::Duplicate(qid.clone()));
        }
        let parent = m.original_parent.clone().ok_or_else(|| RecoveryPlanError::ManifestIncomplete(qid.clone()))?;
        let target = match kind {
            RecoveryKind::Restore => RecoveryTarget { kind: TargetKind::Original, path: Some(m.original_path.clone()) },
            RecoveryKind::Purge => RecoveryTarget { kind: TargetKind::PermanentDelete, path: None },
        };
        items.push(RecoveryItem {
            action_id: format!("r{i:07}"),
            quarantine_id: qid.clone(),
            quarantined_path: m.location.clone(),
            quarantined_identity: m.original_identity.clone(),
            original_parent_identity: parent,
            original_path: m.original_path.clone(),
            target,
            on_conflict: "skip".into(),
            logical_bytes: m.logical_bytes,
        });
    }
    let body = RecoveryBody {
        engine_contract: RECOVERY_CONTRACT.into(),
        plan_id: plan_id.into(),
        recovery_kind: kind,
        created_at: crate::time::utc_from_unix(created_unix),
        expires_at: crate::time::utc_from_unix(created_unix + PLAN_TTL_SECONDS),
        items,
    };
    let digest = body.digest()?;
    Ok(RecoveryPlan { body, digest })
}

/// Plan-level validity at `now` (digest and expiry; there are no rules to re-check).
pub fn check_recovery_plan(plan: &RecoveryPlan, now_unix: i64) -> Result<(), StaleReason> {
    if plan.body.digest().ok().as_deref() != Some(plan.digest.as_str()) || plan.body.engine_contract != RECOVERY_CONTRACT {
        return Err(StaleReason::DigestMismatch);
    }
    match parse_utc(&plan.body.expires_at) {
        Some(e) if now_unix < e => Ok(()),
        _ => Err(StaleReason::Expired),
    }
}

/// Authorization of one recovery plan. A distinct type from the cleanup `Receipt`: neither
/// deserializes as the other, so a cleanup approval can never authorize a restore or purge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryReceipt {
    pub receipt_id: String,
    pub plan_id: String,
    pub plan_digest: String,
    pub recovery_kind: RecoveryKind,
    pub principal: String,
    pub source: ReceiptSource,
    pub created_at: String,
    pub expires_at: String,
    pub revoked_at: Option<String>,
}

/// Only an interactive review authorizes recovery; policies never match recovery plans.
pub fn recovery_receipt_valid(r: &RecoveryReceipt, plan: &RecoveryPlan, principal: &str, now_unix: i64) -> bool {
    r.source == ReceiptSource::InteractiveReview
        && r.revoked_at.is_none()
        && r.plan_id == plan.body.plan_id
        && r.plan_digest == plan.digest
        && r.recovery_kind == plan.body.recovery_kind
        && r.principal == principal
        && parse_utc(&r.expires_at).is_some_and(|e| now_unix < e)
}

// ---------------------------------------------------------------------------------------------
// Platform interface

/// Everything a platform needs to re-verify and act on one quarantined object.
#[derive(Debug, Clone, Copy)]
pub struct QuarantinedObject<'a> {
    pub action_id: &'a str,
    pub location: &'a RawPath,
    /// Identity of the quarantined object (a same-volume move keeps it).
    pub identity: &'a WireIdentity,
    pub original_path: &'a RawPath,
    pub original_parent: &'a WireIdentity,
    pub logical_bytes: u64,
}

impl<'a> QuarantinedObject<'a> {
    pub fn from_item(i: &'a RecoveryItem) -> Self {
        QuarantinedObject {
            action_id: &i.action_id,
            location: &i.quarantined_path,
            identity: &i.quarantined_identity,
            original_path: &i.original_path,
            original_parent: &i.original_parent_identity,
            logical_bytes: i.logical_bytes.0,
        }
    }
}

/// Live state of a quarantined object, for listings (protocol `quarantine_item.state`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuarantineState {
    Recoverable,
    /// Something now occupies the original name; restore would be skipped.
    TargetConflict,
    VolumeOffline,
    /// A different volume is mounted where the quarantine or original location was.
    VolumeChanged,
    IdentityChanged,
    Missing,
    /// The original parent directory is gone or was replaced.
    ParentChanged,
    ManifestIncomplete,
    /// The probe itself failed; never treated as recoverable.
    Unknown,
}

/// Result of a restore or purge action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum RecoveryOutcome {
    Done,
    /// Precondition not met; nothing was changed.
    Skipped { code: String, message: String },
    /// The OS refused; nothing was changed.
    Failed { code: String, message: String },
    /// The OS call may or may not have taken effect.
    Unknown { message: String },
}

/// Metadata kept across quarantine and restore, as verified by the platform's tests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MetadataCoverage {
    /// Protocol `metadata_restored` values.
    pub preserved: Vec<&'static str>,
    /// Known limits (e.g. change time updates), shown to people.
    pub limits: Vec<&'static str>,
}

pub trait RecoveryActions {
    /// Observe a quarantined object and its original location; changes nothing.
    fn probe(&self, object: &QuarantinedObject<'_>) -> QuarantineState;
    /// Move the verified object back under its verified original parent; never replace.
    fn restore(&self, object: &QuarantinedObject<'_>) -> RecoveryOutcome;
    /// Permanently delete exactly the verified quarantined object.
    fn purge(&self, object: &QuarantinedObject<'_>) -> RecoveryOutcome;
    /// Resolve an action that has an intent but no recorded result (EXE-05).
    fn reconcile(&self, object: &QuarantinedObject<'_>, kind: RecoveryKind) -> crate::execute::Reconciled;
    /// Free bytes on the volume holding `path` (REC-05).
    fn free_space(&self, path: &RawPath) -> Option<u64>;
    /// What a quarantine → restore round trip preserves on this platform.
    fn metadata_coverage(&self) -> MetadataCoverage;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident(fid: &str) -> WireIdentity {
        WireIdentity { volume_id: "v".into(), file_id: format!("inode64:{fid:0>16}"), generation: None, quality: "strong".into(), observed_at: "2026-10-15T00:00:00Z".into() }
    }

    fn manifest(name: &str, parent: Option<WireIdentity>) -> QuarantineManifest {
        QuarantineManifest {
            original_path: RawPath::unix(format!("/r/{name}").into_bytes()).unwrap(),
            original_identity: ident("1"),
            original_parent: parent,
            location: RawPath::unix(format!("/q/op/{name}").into_bytes()).unwrap(),
            logical_bytes: Bytes(10),
        }
    }

    #[test]
    fn plans_are_fixed_and_kind_changes_the_digest() {
        let sel = vec![("q1".to_string(), manifest("a", Some(ident("2"))))];
        let restore = build_recovery_plan(RecoveryKind::Restore, &sel, "rp", 1_800_000_000).unwrap();
        let purge = build_recovery_plan(RecoveryKind::Purge, &sel, "rp", 1_800_000_000).unwrap();
        assert_ne!(restore.digest, purge.digest);
        assert_eq!(restore.body.items[0].target.path, Some(sel[0].1.original_path.clone()));
        assert_eq!(purge.body.items[0].target, RecoveryTarget { kind: TargetKind::PermanentDelete, path: None });
        assert_eq!(check_recovery_plan(&restore, 1_800_000_000 + 899), Ok(()));
        assert_eq!(check_recovery_plan(&restore, 1_800_000_000 + 900), Err(StaleReason::Expired));
        let mut tampered = restore.clone();
        tampered.body.items[0].on_conflict = "overwrite".into();
        assert_eq!(check_recovery_plan(&tampered, 1_800_000_000), Err(StaleReason::DigestMismatch));
    }

    #[test]
    fn incomplete_or_duplicate_selections_are_refused() {
        assert_eq!(build_recovery_plan(RecoveryKind::Restore, &[], "rp", 0), Err(RecoveryPlanError::Empty));
        let no_parent = vec![("q1".to_string(), manifest("a", None))];
        assert!(matches!(build_recovery_plan(RecoveryKind::Restore, &no_parent, "rp", 0), Err(RecoveryPlanError::ManifestIncomplete(_))));
        let dup = vec![("q1".to_string(), manifest("a", Some(ident("2")))), ("q1".to_string(), manifest("a", Some(ident("2"))))];
        assert!(matches!(build_recovery_plan(RecoveryKind::Restore, &dup, "rp", 0), Err(RecoveryPlanError::Duplicate(_))));
        let json = serde_json::to_string(&manifest("a", None)).unwrap();
        assert_eq!(QuarantineManifest::parse(&json), Err(ManifestProblem::NoParentIdentity));
        assert!(matches!(QuarantineManifest::parse("{}"), Err(ManifestProblem::Unreadable(_))));
    }

    /// REC-01: quarantine locations are hard-denied, so no cleanup candidate or cleanup action
    /// can ever target quarantined content (restore/purge go through recovery plans only).
    #[test]
    fn quarantine_locations_are_hard_denied() {
        let win = |s: &str| RawPath::windows(s.encode_utf16().collect()).unwrap();
        for p in [
            "D:\\.cleanmanager-quarantine",
            "D:\\.CleanManager-Quarantine\\op_0123\\a0000000",
            "\\\\?\\E:\\.cleanmanager-quarantine\\op_0123\\a0000001",
            "C:\\Users\\a\\AppData\\Local\\CleanManager\\store\\quarantine\\op_0123\\a0000000",
        ] {
            assert!(crate::protect::prefilter(&win(p)).is_err(), "{p}");
        }
        assert!(crate::protect::prefilter(&win("D:\\work\\cleanmanager-quarantine-notes.txt")).is_ok());
        let mac = RawPath::unix(b"/Users/a/Library/Application Support/CleanManager/store/quarantine/op/a0".to_vec()).unwrap();
        assert!(crate::protect::prefilter(&mac).is_err());
    }

    #[test]
    fn only_a_matching_interactive_recovery_receipt_authorizes() {
        let sel = vec![("q1".to_string(), manifest("a", Some(ident("2"))))];
        let plan = build_recovery_plan(RecoveryKind::Restore, &sel, "rp", 1_800_000_000).unwrap();
        let r = RecoveryReceipt {
            receipt_id: "r".into(), plan_id: "rp".into(), plan_digest: plan.digest.clone(), recovery_kind: RecoveryKind::Restore,
            principal: "user:a".into(), source: ReceiptSource::InteractiveReview, created_at: plan.body.created_at.clone(),
            expires_at: plan.body.expires_at.clone(), revoked_at: None,
        };
        assert!(recovery_receipt_valid(&r, &plan, "user:a", 1_800_000_010));
        assert!(!recovery_receipt_valid(&r, &plan, "user:b", 1_800_000_010));
        assert!(!recovery_receipt_valid(&RecoveryReceipt { source: ReceiptSource::Policy, ..r.clone() }, &plan, "user:a", 1_800_000_010));
        assert!(!recovery_receipt_valid(&RecoveryReceipt { recovery_kind: RecoveryKind::Purge, ..r.clone() }, &plan, "user:a", 1_800_000_010));
        assert!(!recovery_receipt_valid(&RecoveryReceipt { revoked_at: Some("t".into()), ..r.clone() }, &plan, "user:a", 1_800_000_010));
        // A cleanup receipt never parses as a recovery receipt, nor the other way round.
        let cleanup = crate::authz::Receipt {
            receipt_id: "c".into(), plan_id: "rp".into(), plan_digest: plan.digest.clone(), mode: crate::rules::Mode::Quarantine,
            principal: "user:a".into(), source: ReceiptSource::InteractiveReview, policy_id: None, policy_version: None,
            created_at: "t".into(), expires_at: plan.body.expires_at.clone(), revoked_at: None,
        };
        assert!(serde_json::from_str::<RecoveryReceipt>(&serde_json::to_string(&cleanup).unwrap()).is_err());
        assert!(serde_json::from_str::<crate::authz::Receipt>(&serde_json::to_string(&r).unwrap()).is_err());
    }
}
