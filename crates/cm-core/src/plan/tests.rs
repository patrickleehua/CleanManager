// Tests for plan building live with the store-backed integration tests.

use super::*;
use crate::execute::LiveCheck;
use crate::protect::{DenyLayer, ProtectionDecision};

/// BUG-WFA-19: messages people read name the code and say what it means; none is Rust debug text.
#[test]
fn refusal_and_skip_messages_are_written_for_people() {
    let debugish = |s: &str| ["{", "}", "(\"", "Some(", "PendingEvidence", "HardDeny", "IdentityChanged", "OwnerRunning", "MetadataChanged", "Expired"].iter().any(|d| s.contains(d));
    let texts = [
        PlanError::ModeNotAllowed(Mode::Delete, "c_1".into()).to_string(),
        not_plannable_reason(CandidateState::PendingEvidence).to_owned(),
        not_plannable_reason(CandidateState::ProtectedReviewable).to_owned(),
        StaleReason::Expired.to_string(),
        StaleReason::RuleSetChanged.to_string(),
        crate::plan::v2::PlanProblem::Stale(StaleReason::DigestMismatch).to_string(),
        LiveCheck::IdentityChanged.to_string(),
        LiveCheck::OwnerRunning.to_string(),
        LiveCheck::MetadataChanged("size 4000 -> 4500".into()).to_string(),
        LiveCheck::InUse.to_string(),
        ProtectionDecision::Denied { layer: DenyLayer::HardDeny, reason: "object identity could not be established".into() }.to_string(),
    ];
    for t in &texts {
        assert!(!t.is_empty() && !debugish(t), "{t}");
    }
    assert_eq!(texts[0], "mode delete is not allowed for candidate c_1");
    assert!(texts[1].starts_with("pending_evidence: "), "{}", texts[1]);
    assert!(texts[3].starts_with("expired: "), "{}", texts[3]);
    assert!(texts[8].contains("size 4000 -> 4500"), "{}", texts[8]);
    assert_eq!(texts[10], "hard_deny: protected by the built-in hard-deny list: object identity could not be established");
    assert_eq!(ProtectionDecision::Allowed.to_string(), "allowed");
}
