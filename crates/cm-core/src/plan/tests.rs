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

/// A tiny Windows view: `C:\r` with the given (parent, name, file size or `None` for a folder) entries.
fn view_of(entries: &[(u64, &str, Option<u64>)]) -> crate::analysis::MemView {
    use crate::analysis::{EntryFacts, MemView};
    use crate::model::IdentityQuality;
    let mut v = MemView { platform: Some(crate::capability::Platform::Windows), root: 1, ..Default::default() };
    let add = |v: &mut MemView, id: u64, parent: Option<u64>, name: &str, size: Option<u64>| {
        v.names.insert(id, RawPath::windows(name.encode_utf16().collect()).unwrap());
        v.parents.insert(id, parent);
        if let Some(p) = parent {
            v.kids.entry(p).or_default().push(id);
        }
        let kind = if size.is_some() { ObjectKind::File } else { ObjectKind::Directory };
        v.facts.insert(id, EntryFacts {
            kind, boundary: false, object_id: id, logical: size, allocated: size, volume_id: "v".into(), file_id: format!("id{id}"),
            quality: IdentityQuality::Strong, cloud: CloudState::NotCloud, modified_unix: Some(1_700_000_000), link_count: None, skipped_here: 0,
        });
    };
    add(&mut v, 1, None, "C:\r", None);
    for (i, (parent, name, size)) in entries.iter().enumerate() {
        add(&mut v, i as u64 + 2, Some(*parent), name, *size);
    }
    v
}

fn folder_candidate(id: &str, entry: u64, path: RawPath, rule: &str) -> Candidate {
    serde_json::from_value(serde_json::json!({
        "candidate_id": id, "entry_id": entry, "path": path, "kind": "directory", "rules": [{"rule_id": rule, "version": "1.0.0"}],
        "category": "app_caches", "state": "allowed", "tier": "default_selected", "reasons": [], "protection": null,
        "logical": {"known": "0", "unknown_count": 0}, "allowed_modes": ["trash", "quarantine"], "probes": [], "impact": "", "owner_id": null,
    }))
    .unwrap()
}

/// WFA-113: a folder emptied by an earlier quarantine contributes no action and is left out (with its rule); a selection
/// of only such folders is an empty selection, never a zero-action plan.
#[test]
fn a_plan_never_has_zero_actions() {
    use crate::analysis::IndexView;
    // 2 = Code Cache (only an empty folder inside), 3 = its js, 4 = GPUCache, 5 = GPUCache\data_0
    let view = view_of(&[(1, "Code Cache", None), (2, "js", None), (1, "GPUCache", None), (4, "data_0", Some(9216))]);
    let set = CandidateSet {
        candidate_set_id: "cs".into(), scan_id: "s".into(), revision: 1, rule_set_digest: "d".into(), protection_version: "v".into(),
        candidates: vec![folder_candidate("c_code", 2, view.path(2), "app.code"), folder_candidate("c_gpu", 4, view.path(4), "app.gpu")],
        inapplicable: vec![],
    };
    let input = PlanInput { plan_id: "p", scope_id: "scope", created_unix: 1_800_000_000, observed_at: "2026-10-23T00:00:00Z", policy_digest: "pd" };
    for mode in [Mode::Quarantine, Mode::Trash] {
        assert_eq!(build_plan(&view, &set, &["c_code".into()], mode, &input).unwrap_err(), PlanError::Empty, "{mode:?}");
        let plan = build_plan(&view, &set, &["c_code".into(), "c_gpu".into()], mode, &input).unwrap();
        assert_eq!(plan.body.items.iter().map(|i| i.candidate_id.as_str()).collect::<Vec<_>>(), ["c_gpu"]);
        assert_eq!(plan.rule_refs, ["app.gpu@1.0.0"], "the left-out folder's rule is not the plan's");
    }
    assert!(PlanError::Empty.to_string().starts_with("empty selection: "), "{}", PlanError::Empty);
    // The view-free checks come first and keep their order: unknown, then not plannable, then the mode.
    assert_eq!(check_selection(&set, &[], Mode::Trash).unwrap_err(), PlanError::Empty);
    assert_eq!(check_selection(&set, &["c_x".into()], Mode::Trash).unwrap_err(), PlanError::UnknownCandidate("c_x".into()));
    assert!(matches!(check_selection(&set, &["c_gpu".into()], Mode::Delete), Err(PlanError::ModeNotAllowed(Mode::Delete, _))));
    assert_eq!(check_selection(&set, &["c_gpu".into(), "c_gpu".into()], Mode::Trash).unwrap().len(), 1);
}
