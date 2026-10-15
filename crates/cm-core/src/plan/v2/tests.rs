use super::*;
use crate::authz::{KindReceipt, Receipt, ReceiptSource, kind_receipt_valid};

const GOLDEN: [(&str, &str); 11] = [
    ("cleanup", include_str!("../../../../../examples/contracts/plan-v2-cleanup.json")),
    ("manual", include_str!("../../../../../examples/contracts/plan-v2-manual-selection.json")),
    ("manual-purge", include_str!("../../../../../examples/contracts/plan-v2-manual-trash-purge.json")),
    ("uninstall", include_str!("../../../../../examples/contracts/plan-v2-uninstall.json")),
    ("maintenance", include_str!("../../../../../examples/contracts/plan-v2-maintenance.json")),
    ("startup", include_str!("../../../../../examples/contracts/plan-v2-startup.json")),
    ("update", include_str!("../../../../../examples/contracts/plan-v2-update.json")),
    ("recovery", include_str!("../../../../../examples/contracts/plan-v2-recovery.json")),
    ("manual-folder", include_str!("../../../../../examples/contracts/plan-v2-manual-folder.json")),
    ("startup-disable", include_str!("../../../../../examples/contracts/plan-v2-startup-disable.json")),
    ("process", include_str!("../../../../../examples/contracts/plan-v2-process.json")),
];

fn golden(name: &str) -> PlanBodyV2 {
    let text = GOLDEN.iter().find(|(n, _)| *n == name).unwrap().1;
    let v: serde_json::Value = serde_json::from_str(text).unwrap();
    serde_json::from_value(v["body"].clone()).unwrap()
}

/// Every plan/v2 golden vector parses into the Rust types, re-serializes to the same body and hashes to the same
/// CM-C14N-v1 digest as the Python generator, and satisfies its kind's rules.
#[test]
fn golden_vectors_round_trip_with_the_same_digest() {
    let mut kinds = std::collections::BTreeSet::new();
    let mut actions = std::collections::BTreeSet::new();
    for (name, text) in GOLDEN {
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        let body: PlanBodyV2 = serde_json::from_value(v["body"].clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(serde_json::to_value(&body).unwrap(), v["body"], "{name}: lossless round trip");
        assert_eq!(body.digest().unwrap(), v["plan_digest"].as_str().unwrap(), "{name}: same digest as the generator");
        validate(&body).unwrap_or_else(|e| panic!("{name}: {e}"));
        kinds.insert(body.plan_kind);
        actions.extend(body.items.iter().map(PlanAction::action_type));
    }
    assert_eq!(kinds.len(), PlanKind::ALL.len(), "every kind has a vector");
    assert_eq!(actions.len(), 11, "every action type has a vector");
}

#[test]
fn only_cleanup_is_policy_matchable() {
    let matchable: Vec<_> = PlanKind::ALL.into_iter().filter(|k| k.policy_matchable()).collect();
    assert_eq!(matchable, [PlanKind::Cleanup]);
    for kind in PlanKind::ALL.into_iter().filter(|k| *k != PlanKind::Cleanup) {
        let mut body = golden("manual");
        body.plan_kind = kind;
        body.items = golden(match kind {
            PlanKind::ManualSelection => "manual",
            PlanKind::Uninstall => "uninstall",
            PlanKind::Maintenance => "maintenance",
            PlanKind::Startup => "startup",
            PlanKind::Update => "update",
            PlanKind::Process => "process",
            _ => "recovery",
        })
        .items;
        body.requires_admin = body.items.iter().any(PlanAction::needs_admin);
        assert_eq!(validate(&body), Ok(()), "{kind:?}");
        body.policy_matchable = true;
        assert_eq!(validate(&body), Err(KindError::PolicyMatchable(kind)), "{kind:?}");
    }
}

#[test]
fn per_kind_action_sets_are_enforced() {
    // An fs_delete outside the recycle bin is never a manual selection action.
    let mut body = golden("manual-purge");
    let PlanAction::FsDelete(a) = &mut body.items[0] else { panic!() };
    a.target_location = TargetLocation::Original;
    assert!(matches!(validate(&body), Err(KindError::Action { .. })));
    // A trash move of something already in the bin is not a manual selection action either.
    let mut body = golden("manual");
    let PlanAction::FsTrash(a) = &mut body.items[0] else { panic!() };
    a.target_location = TargetLocation::PlatformTrash;
    assert!(matches!(validate(&body), Err(KindError::Action { .. })));
    // Quarantine is not a manual selection mode.
    let mut body = golden("manual");
    let PlanAction::FsTrash(a) = body.items[0].clone() else { panic!() };
    body.items[0] = PlanAction::FsQuarantine(a);
    assert!(matches!(validate(&body), Err(KindError::ActionNotAllowed { kind: PlanKind::ManualSelection, action: ActionType::FsQuarantine, .. })));
    // Trash and purge never mix in one manual plan.
    let mut body = golden("manual");
    let mut purge = golden("manual-purge").items[0].clone();
    if let PlanAction::FsDelete(a) = &mut purge {
        a.action_id = "a02".into();
    }
    body.items.push(purge);
    assert_eq!(validate(&body), Err(KindError::MixedManualSelection));
    // A startup plan holds startup toggles only; file actions are refused.
    let mut body = golden("startup");
    body.items.push(golden("manual").items[0].clone());
    if let PlanAction::FsTrash(a) = &mut body.items[1] {
        a.action_id = "a02".into();
    }
    assert!(matches!(validate(&body), Err(KindError::ActionNotAllowed { kind: PlanKind::Startup, action: ActionType::FsTrash, .. })));
    // A restore toggle only in recovery plans; a plain toggle never in them.
    let mut body = golden("startup");
    let PlanAction::StartupToggle(t) = &mut body.items[0] else { panic!() };
    t.operation = StartupOperation::Restore;
    t.backup_id = Some("rb".into());
    assert!(matches!(validate(&body), Err(KindError::Action { .. })));
    // Registry restores never overwrite.
    let mut body = golden("recovery");
    let PlanAction::RegistryRestore(r) = &mut body.items[0] else { panic!() };
    r.on_existing = "overwrite".into();
    assert!(matches!(validate(&body), Err(KindError::Action { .. })));
    // Duplicate ids and empty plans.
    let mut body = golden("startup");
    body.items.push(body.items[0].clone());
    assert_eq!(validate(&body), Err(KindError::DuplicateAction("a01".into())));
    let mut body = golden("startup");
    body.items.clear();
    assert_eq!(validate(&body), Err(KindError::Empty));
}

#[test]
fn requires_admin_follows_the_actions() {
    let mut body = golden("startup");
    let PlanAction::StartupToggle(t) = &mut body.items[0] else { panic!() };
    t.scope = StartupScope::System;
    assert_eq!(validate(&body), Err(KindError::RequiresAdmin { stated: false, needed: true }));
    body.requires_admin = true;
    assert_eq!(validate(&body), Ok(()));
    let mut body = golden("manual");
    body.requires_admin = true;
    assert_eq!(validate(&body), Err(KindError::RequiresAdmin { stated: true, needed: false }));
}

/// The kind is digest-covered: the same actions under another kind are another plan.
#[test]
fn kind_changes_the_digest() {
    let manual = golden("manual");
    let mut uninstall = manual.clone();
    uninstall.plan_kind = PlanKind::Uninstall;
    assert_eq!(validate(&uninstall), Ok(()));
    assert_ne!(manual.digest().unwrap(), uninstall.digest().unwrap());
}

#[test]
fn no_rules_digest_is_the_empty_array() {
    assert_eq!(crate::c14n::digest(&serde_json::json!([])).unwrap(), NO_RULES_DIGEST);
}

fn stored(body: PlanBodyV2) -> PlanV2 {
    let digest = body.digest().unwrap();
    let meta = vec![ActionMeta { startup: Some(StartupLocation::RunValue {
        root: String::new(), hive: Hive::Hkcu, view: crate::startup::RegView::Default, key: "k".into(), value_name: "v".into(), approval_key: "a".into(),
    }), ..Default::default() }];
    PlanV2 { body, digest, meta }
}

#[test]
fn stored_plans_are_checked_before_use() {
    let mut body = golden("startup");
    let PlanAction::StartupToggle(t) = &mut body.items[0] else { panic!() };
    t.observed_sha256 = Some("7".repeat(64));
    let created = parse_utc(&body.created_at).unwrap();
    let plan = stored(body);
    assert_eq!(check_plan_v2(&plan, created + 10), Ok(()));
    assert_eq!(check_plan_v2(&plan, created + PLAN_TTL_SECONDS), Err(PlanProblem::Stale(StaleReason::Expired)));
    let mut tampered = plan.clone();
    tampered.body.plan_kind = PlanKind::Uninstall;
    assert_eq!(check_plan_v2(&tampered, created + 10), Err(PlanProblem::Stale(StaleReason::DigestMismatch)));
    // A body that hashes to its digest but breaks its kind's rules is refused as such.
    let mut forged = plan.clone();
    forged.body.policy_matchable = true;
    forged.digest = forged.body.digest().unwrap();
    assert_eq!(check_plan_v2(&forged, created + 10), Err(PlanProblem::Kind(KindError::PolicyMatchable(PlanKind::Startup))));
    // Execution data that does not belong to the body.
    let mut no_meta = plan.clone();
    no_meta.meta[0].startup = None;
    assert_eq!(check_plan_v2(&no_meta, created + 10), Err(PlanProblem::Stale(StaleReason::DigestMismatch)));
}

#[test]
fn kind_receipts_bind_kind_and_never_cross_types() {
    let mut body = golden("startup");
    let PlanAction::StartupToggle(t) = &mut body.items[0] else { panic!() };
    t.observed_sha256 = Some("7".repeat(64));
    let plan = stored(body);
    let now = parse_utc(&plan.body.created_at).unwrap() + 5;
    let r = KindReceipt {
        receipt_id: "r".into(), plan_id: plan.body.plan_id.clone(), plan_digest: plan.digest.clone(), plan_kind: PlanKind::Startup,
        principal: "user:a".into(), source: ReceiptSource::InteractiveReview, created_at: plan.body.created_at.clone(),
        expires_at: plan.body.expires_at.clone(), revoked_at: None,
    };
    assert!(kind_receipt_valid(&r, &plan, "user:a", now));
    assert!(!kind_receipt_valid(&r, &plan, "user:b", now));
    assert!(!kind_receipt_valid(&KindReceipt { source: ReceiptSource::Policy, ..r.clone() }, &plan, "user:a", now), "only a review authorizes non-cleanup kinds");
    assert!(!kind_receipt_valid(&KindReceipt { plan_kind: PlanKind::Maintenance, ..r.clone() }, &plan, "user:a", now));
    assert!(!kind_receipt_valid(&KindReceipt { revoked_at: Some("t".into()), ..r.clone() }, &plan, "user:a", now));
    let json = serde_json::to_string(&r).unwrap();
    assert!(serde_json::from_str::<Receipt>(&json).is_err());
    assert!(serde_json::from_str::<crate::recovery::RecoveryReceipt>(&json).is_err());
    let cleanup = Receipt {
        receipt_id: "c".into(), plan_id: plan.body.plan_id.clone(), plan_digest: plan.digest.clone(), mode: crate::rules::Mode::Trash,
        principal: "user:a".into(), source: ReceiptSource::InteractiveReview, policy_id: None, policy_version: None,
        created_at: "t".into(), expires_at: plan.body.expires_at.clone(), revoked_at: None,
    };
    assert!(serde_json::from_str::<KindReceipt>(&serde_json::to_string(&cleanup).unwrap()).is_err());
}
