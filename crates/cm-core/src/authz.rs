//! Authorization (PLN-04/05/06/07). Authorization is a separate fact from plan integrity: a
//! receipt binds a plan digest, mode and principal, and comes only from an interactive human
//! review or from a bounded policy the user enabled. Approved flags, elevation, request ids and
//! non-interactive switches never authorize anything.

use serde::{Deserialize, Serialize};

use crate::plan::{Plan, parse_utc};
use crate::rules::{Mode, Tier};

pub const GIB: u64 = 1 << 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptSource {
    InteractiveReview,
    Policy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub receipt_id: String,
    pub plan_id: String,
    pub plan_digest: String,
    pub mode: Mode,
    pub principal: String,
    pub source: ReceiptSource,
    pub policy_id: Option<String>,
    pub policy_version: Option<u32>,
    pub created_at: String,
    pub expires_at: String,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyState {
    Enabled,
    Paused,
    Revoked,
}

/// A bounded preauthorization the user created deliberately (default: none exist).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub policy_id: String,
    pub version: u32,
    pub state: PolicyState,
    pub principal: String,
    /// Exact scope (protocol scope_id) the policy covers.
    pub scope_id: String,
    /// Exact `rule_id@version` strings; a rule upgrade is never covered automatically.
    pub rules: Vec<String>,
    pub mode: Mode,
    pub created_at: String,
    pub expires_at: String,
    pub single_max_bytes: u64,
    pub single_max_items: u64,
    pub window_max_bytes: u64,
    pub window_max_items: u64,
}

impl Policy {
    /// The documented default template: 30 days, 1 GiB / 1,000 items per plan, 2 GiB / 2,000
    /// items per rolling 24 hours. Created disabled-by-intent: callers must enable explicitly.
    pub fn template(policy_id: &str, principal: &str, scope_id: &str, rules: Vec<String>, mode: Mode, now_unix: i64) -> Policy {
        Policy {
            policy_id: policy_id.into(),
            version: 1,
            state: PolicyState::Paused,
            principal: principal.into(),
            scope_id: scope_id.into(),
            rules,
            mode,
            created_at: crate::time::utc_from_unix(now_unix),
            expires_at: crate::time::utc_from_unix(now_unix + 30 * 86_400),
            single_max_bytes: GIB,
            single_max_items: 1_000,
            window_max_bytes: 2 * GIB,
            window_max_items: 2_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reason", content = "detail")]
pub enum PolicyMismatch {
    NotEnabled,
    Expired,
    OtherPrincipal,
    ScopeDiffers,
    ModeDiffers,
    RuleNotCovered(String),
    TierNotDefaultSelected,
    SizeUnknown,
    SingleLimit,
    QuotaExceeded,
}

/// Whether a policy covers a whole plan (quota is checked atomically by the store afterwards).
pub fn policy_covers(policy: &Policy, plan: &Plan, principal: &str, now_unix: i64) -> Result<(), PolicyMismatch> {
    if policy.state != PolicyState::Enabled {
        return Err(PolicyMismatch::NotEnabled);
    }
    if parse_utc(&policy.expires_at).is_none_or(|e| now_unix >= e) {
        return Err(PolicyMismatch::Expired);
    }
    if policy.principal != principal {
        return Err(PolicyMismatch::OtherPrincipal);
    }
    if policy.scope_id != plan.body.scope_id {
        return Err(PolicyMismatch::ScopeDiffers);
    }
    if policy.mode != plan.body.mode {
        return Err(PolicyMismatch::ModeDiffers);
    }
    for r in &plan.rule_refs {
        if !policy.rules.contains(r) {
            return Err(PolicyMismatch::RuleNotCovered(r.clone()));
        }
    }
    if plan.tiers.iter().any(|t| *t != Tier::DefaultSelected) {
        return Err(PolicyMismatch::TierNotDefaultSelected);
    }
    let total = plan.body.total_logical().ok_or(PolicyMismatch::SizeUnknown)?;
    if total.0 > policy.single_max_bytes || plan.body.items.len() as u64 > policy.single_max_items {
        return Err(PolicyMismatch::SingleLimit);
    }
    Ok(())
}

/// A receipt authorizes exactly this plan body (digest), mode and principal until it expires
/// or is revoked; anything else needs a new approval (PLN-04).
pub fn receipt_valid(r: &Receipt, plan: &Plan, principal: &str, now_unix: i64) -> bool {
    r.revoked_at.is_none()
        && r.plan_id == plan.body.plan_id
        && r.plan_digest == plan.digest
        && r.mode == plan.body.mode
        && r.principal == principal
        && parse_utc(&r.expires_at).is_some_and(|e| now_unix < e)
}

/// Authorization of one `plan/v2` plan. It binds the plan kind as well as the digest, and is a distinct type from the
/// cleanup [`Receipt`] and the quarantine `RecoveryReceipt`: none of them deserializes as another, so an approval of
/// one kind can never authorize a plan of another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KindReceipt {
    pub receipt_id: String,
    pub plan_id: String,
    pub plan_digest: String,
    pub plan_kind: crate::plan::v2::PlanKind,
    pub principal: String,
    pub source: ReceiptSource,
    pub created_at: String,
    pub expires_at: String,
    pub revoked_at: Option<String>,
}

/// A `plan/v2` receipt authorizes exactly this body. Kinds that can never be policy-matched (everything but cleanup)
/// accept only an interactive review.
pub fn kind_receipt_valid(r: &KindReceipt, plan: &crate::plan::v2::PlanV2, principal: &str, now_unix: i64) -> bool {
    (r.source == ReceiptSource::InteractiveReview || plan.body.plan_kind.policy_matchable())
        && r.revoked_at.is_none()
        && r.plan_id == plan.body.plan_id
        && r.plan_digest == plan.digest
        && r.plan_kind == plan.body.plan_kind
        && r.principal == principal
        && parse_utc(&r.expires_at).is_some_and(|e| now_unix < e)
}

/// The OS user that authorizations bind to.
pub fn current_principal() -> String {
    let user = std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "unknown".into());
    let domain = std::env::var("USERDOMAIN").unwrap_or_default();
    if domain.is_empty() { format!("user:{user}") } else { format!("user:{domain}\\{user}") }
}
