//! Fixed plans (PLN-01/02/03): a finite list of file actions frozen from reviewed candidates.
//!
//! A directory candidate is expanded to the files observed under it; directories themselves are
//! never acted on, so files created after planning can never be swept up. Every item binds a
//! CM-OBS-v1 metadata fingerprint; the body digest is CM-C14N-v1. Changing the selection, the
//! mode or any bound version produces a new plan with a new digest.

use serde::{Deserialize, Serialize};

use crate::analysis::{Candidate, CandidateSet, CandidateState, IndexView};
use crate::bytes::Bytes;
use crate::c14n::{C14nError, digest};
use crate::model::{CloudState, ObjectKind, Source, Timestamp};
use crate::obs::{NativeTime, ObsContract, ObservationRecord, TimeUnit};
use crate::path::RawPath;
use crate::rules::{Mode, Tier};

/// Default plan lifetime (PLN-03).
pub const PLAN_TTL_SECONDS: i64 = 900;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireIdentity {
    pub volume_id: String,
    pub file_id: String,
    pub generation: Option<String>,
    pub quality: String,
    pub observed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanItem {
    pub action_id: String,
    pub candidate_id: String,
    pub entry_id: String,
    pub path: RawPath,
    pub identity: WireIdentity,
    pub observed_fingerprint_sha256: String,
    pub logical_bytes: Bytes,
    pub allocated_bytes: Option<Bytes>,
    pub must_not_follow_links: bool,
}

/// Protocol `plan_body` (plan/v1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanBody {
    pub engine_contract: String,
    pub plan_id: String,
    pub scope_id: String,
    pub scan_id: String,
    pub scan_revision: u32,
    pub rule_set_digest: String,
    pub policy_digest: String,
    pub created_at: String,
    pub expires_at: String,
    pub mode: Mode,
    pub items: Vec<PlanItem>,
}

impl PlanBody {
    pub fn digest(&self) -> Result<String, C14nError> {
        digest(&serde_json::to_value(self).expect("plan body serializes"))
    }

    pub fn total_logical(&self) -> Option<Bytes> {
        self.items.iter().try_fold(Bytes(0), |acc, i| acc.checked_add(i.logical_bytes).ok())
    }
}

/// A stored plan: the body, its digest and each item's full observation (authoritative).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub body: PlanBody,
    pub digest: String,
    pub observations: Vec<ObservationRecord>,
    /// Candidate tiers at planning time (policy matching needs them; not part of the digest).
    pub tiers: Vec<Tier>,
    pub rule_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("empty selection")]
    Empty,
    #[error("candidate {0} is not in this candidate set")]
    UnknownCandidate(String),
    #[error("candidate {0} cannot be planned: {1}")]
    NotPlannable(String, String),
    #[error("mode {0:?} is not allowed for candidate {1}")]
    ModeNotAllowed(Mode, String),
    #[error("candidate {0} contains a file of unknown size")]
    UnknownSize(String),
    #[error("{0}")]
    Canonical(#[from] C14nError),
}

pub struct PlanInput<'a> {
    pub plan_id: &'a str,
    pub scope_id: &'a str,
    pub created_unix: i64,
    /// Scan finish time: when the bound observations were taken.
    pub observed_at: &'a str,
    pub policy_digest: &'a str,
}

fn wire_identity(view: &dyn IndexView, entry: u64, observed_at: &str) -> WireIdentity {
    let f = view.info(entry);
    WireIdentity {
        volume_id: f.volume_id.clone(),
        file_id: f.file_id.clone(),
        generation: None,
        quality: if f.quality == crate::model::IdentityQuality::Strong { "strong".into() } else { "weak".into() },
        observed_at: observed_at.into(),
    }
}

fn core_identity(w: &WireIdentity) -> crate::model::Identity {
    let wire = serde_json::to_value(w).expect("identity serializes");
    serde_json::from_value(wire).unwrap_or_else(|_| crate::model::Identity {
        volume_id: w.volume_id.clone(),
        file_id_kind: crate::model::FileIdKind::FileId128,
        file_id_hex: "0".repeat(32),
        generation: None,
        quality: crate::model::IdentityQuality::Weak,
        observed_at: Timestamp(w.observed_at.clone()),
    })
}

/// CM-OBS-v1 record for one entry, built only from index metadata.
pub fn observe(view: &dyn IndexView, entry: u64, observed_at: &str) -> ObservationRecord {
    let f = view.info(entry);
    let mut chain = Vec::new();
    let mut cur = view.parent(entry);
    while let Some(p) = cur {
        chain.push(core_identity(&wire_identity(view, p, observed_at)));
        cur = view.parent(p);
    }
    let root = core_identity(&wire_identity(view, view.root(), observed_at));
    let mut sources = std::collections::BTreeMap::new();
    let src = |present: bool| present.then_some(Source::DirectoryEnumeration);
    sources.insert("logical_bytes".into(), src(f.logical.is_some()));
    sources.insert("allocated_bytes".into(), src(f.allocated.is_some()));
    sources.insert("modified".into(), src(f.modified_unix.is_some()));
    sources.insert("change_marker".into(), None);
    sources.insert("link_count".into(), f.link_count.map(|_| Source::HandleQuery));
    sources.insert("reparse_tag".into(), None);
    sources.insert("cloud".into(), src(f.cloud != CloudState::Unknown));
    ObservationRecord {
        contract: ObsContract::V1,
        scope_root: root,
        volume_id: f.volume_id.clone(),
        entity: core_identity(&wire_identity(view, entry, observed_at)),
        parent_chain: chain,
        path: view.path(entry),
        kind: f.kind,
        logical_bytes: f.logical.map(Bytes),
        allocated_bytes: f.allocated.map(Bytes),
        modified: f.modified_unix.map(|s| NativeTime { unit: TimeUnit::UnixNanoseconds, value: (i128::from(s) * 1_000_000_000).to_string() }),
        change_marker: None,
        link_count: f.link_count,
        reparse_tag: None,
        cloud: f.cloud,
        field_sources: sources,
    }
}

fn files_under(view: &dyn IndexView, entry: u64) -> Vec<u64> {
    let mut out = Vec::new();
    let mut stack = vec![entry];
    while let Some(id) = stack.pop() {
        let f = view.info(id);
        if f.boundary {
            continue;
        }
        match f.kind {
            ObjectKind::Directory => stack.extend_from_slice(view.children(id)),
            ObjectKind::File => out.push(id),
            _ => {}
        }
    }
    out.sort_unstable();
    out
}

/// Freeze a selection into a plan. Only allowed (or explicitly reviewed review-only) candidates
/// with the chosen mode permitted are accepted; blocked and pending candidates are refused.
pub fn build_plan(view: &dyn IndexView, set: &CandidateSet, selected: &[String], mode: Mode, input: &PlanInput<'_>) -> Result<Plan, PlanError> {
    if selected.is_empty() {
        return Err(PlanError::Empty);
    }
    let mut chosen: Vec<&Candidate> = Vec::new();
    for id in selected {
        let c = set.candidates.iter().find(|c| &c.candidate_id == id).ok_or_else(|| PlanError::UnknownCandidate(id.clone()))?;
        match c.state {
            CandidateState::Allowed => {}
            other => return Err(PlanError::NotPlannable(id.clone(), format!("{other:?}"))),
        }
        if c.tier == Tier::Blocked {
            return Err(PlanError::NotPlannable(id.clone(), "blocked".into()));
        }
        if !c.allowed_modes.contains(&mode) {
            return Err(PlanError::ModeNotAllowed(mode, id.clone()));
        }
        if !chosen.iter().any(|x| x.candidate_id == c.candidate_id) {
            chosen.push(c);
        }
    }
    let mut items = Vec::new();
    let mut observations = Vec::new();
    let mut tiers = Vec::new();
    let mut rule_refs = Vec::new();
    let mut seen_objects = std::collections::BTreeSet::new();
    for c in &chosen {
        for r in &c.rules {
            rule_refs.push(format!("{}@{}", r.rule_id, r.version));
        }
        for file in files_under(view, c.entry_id) {
            let f = view.info(file);
            if !seen_objects.insert(f.object_id) {
                continue; // one action per object
            }
            let logical = f.logical.ok_or_else(|| PlanError::UnknownSize(c.candidate_id.clone()))?;
            let obs = observe(view, file, input.observed_at);
            let fingerprint = obs.fingerprint()?;
            items.push(PlanItem {
                action_id: String::new(),
                candidate_id: c.candidate_id.clone(),
                entry_id: file.to_string(),
                path: view.path(file),
                identity: wire_identity(view, file, input.observed_at),
                observed_fingerprint_sha256: fingerprint,
                logical_bytes: Bytes(logical),
                allocated_bytes: f.allocated.map(Bytes),
                must_not_follow_links: true,
            });
            observations.push(obs);
            tiers.push(c.tier);
        }
    }
    for (i, item) in items.iter_mut().enumerate() {
        item.action_id = format!("a{i:07}");
    }
    rule_refs.sort();
    rule_refs.dedup();
    let body = PlanBody {
        engine_contract: "plan/v1".into(),
        plan_id: input.plan_id.into(),
        scope_id: input.scope_id.into(),
        scan_id: set.scan_id.clone(),
        scan_revision: set.revision,
        rule_set_digest: set.rule_set_digest.clone(),
        policy_digest: input.policy_digest.into(),
        created_at: crate::time::utc_from_unix(input.created_unix),
        expires_at: crate::time::utc_from_unix(input.created_unix + PLAN_TTL_SECONDS),
        mode,
        items,
    };
    let digest = body.digest()?;
    Ok(Plan { body, digest, observations, tiers, rule_refs })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StaleReason {
    Expired,
    RuleSetChanged,
    DigestMismatch,
}

/// Plan-level validity at `now` (PLN-03). Item-level re-observation happens at execution.
pub fn check_plan(plan: &Plan, now_unix: i64, current_rule_set_digest: &str) -> Result<(), StaleReason> {
    if plan.body.digest().ok().as_deref() != Some(plan.digest.as_str()) {
        return Err(StaleReason::DigestMismatch);
    }
    let expires = parse_utc(&plan.body.expires_at).ok_or(StaleReason::Expired)?;
    if now_unix >= expires {
        return Err(StaleReason::Expired);
    }
    if plan.body.rule_set_digest != current_rule_set_digest {
        return Err(StaleReason::RuleSetChanged);
    }
    Ok(())
}

/// Parse `YYYY-MM-DDTHH:MM:SSZ` to Unix seconds (inverse of `utc_from_unix`).
pub fn parse_utc(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'Z' {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s[r].parse::<i64>().ok();
    let (y, m, d, hh, mm, ss) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + hh * 3_600 + mm * 60 + ss)
}

#[cfg(test)]
mod tests;
