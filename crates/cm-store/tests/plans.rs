//! Plans, authorization and policy quotas (MVP group 8) over a real store.

use std::collections::BTreeMap;

use cm_core::analysis::{AnalysisInput, CandidateSet, ResolvedRoot, analyze};
use cm_core::authz::{Policy, PolicyMismatch, PolicyState, Receipt, ReceiptSource, policy_covers, receipt_valid};
use cm_core::capability::Platform;
use cm_core::host::HostRef;
use cm_core::model::{CloudState, IdentityQuality, ObjectKind, ScanState};
use cm_core::path::RawPath;
use cm_core::plan::{PLAN_TTL_SECONDS, PlanError, PlanInput, StaleReason, build_plan, check_plan, parse_utc};
use cm_core::rules::{ActionKind, Impact, Match, Mode, ObjectType, Owner, OwnerKind, RebuildClass, RootResolver, Rule, RuleSet, RuleStatus, Thresholds, Tier};
use cm_core::scan::{EntryRow, ObjectRow};
use cm_store::Stores;
use cm_store::index::ScanInit;

const NOW: i64 = 1_800_000_000;

fn scratch(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("cm-plans-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn obj(id: u64, kind: ObjectKind, size: Option<u64>) -> ObjectRow {
    ObjectRow { object_id: id, volume_id: "v".into(), file_id: format!("inode64:{id:016x}"), generation: None,
        quality: IdentityQuality::Strong, kind, logical: size, allocated: size, link_count: Some(1), cloud: CloudState::NotCloud,
        modified_unix: Some(NOW - 90 * 86_400), shared_blocks: None }
}

fn ent(id: u64, parent: Option<u64>, name: &str, object: u64, kind: ObjectKind) -> EntryRow {
    EntryRow { entry_id: id, parent_entry_id: parent, name: RawPath::unix(name.as_bytes().to_vec()).unwrap(), object_id: object, kind, boundary: None }
}

fn rule(id: &str, comps: &[&str], tier: Tier) -> Rule {
    Rule {
        schema_version: "2.0".into(), rule_id: id.into(), version: "1.0.0".into(), status: RuleStatus::Enabled,
        platforms: vec![Platform::Macos], category: "app_caches".into(), max_tier: tier, rebuild_class: RebuildClass::LocalRebuild,
        owner: Owner { kind: OwnerKind::App, id: "t".into() }, root_resolver: RootResolver::ApplicationCache,
        matcher: Match { object_type: ObjectType::Any, relative_components: comps.iter().map(|s| s.to_string()).collect(), name_glob: None,
            extensions: vec![], require_cachedir_tag: false, archive_payload: false, max_depth: 0 },
        exclude: vec![], probes: vec![], thresholds: Thresholds { min_age_days: None, age_basis: None, min_size_bytes: None, retention_days: None },
        action_kind: ActionKind::Filesystem, adapter_ref: None, allowed_modes: vec![Mode::Trash, Mode::Quarantine], requires_admin: false,
        protect_descendants: true, follow_links: false,
        impact: Impact { summary: "s".into(), regeneration: "r".into(), offline_effect: "o".into() },
        tested_registry_ref: Some("x@1.0.0".into()), evidence_refs: vec!["e".into()],
    }
}

/// /r/app/{Cache/{a,b,sub/c,linkB->a}, Locked/{Cookies}} ; cache files 100,200,300
fn setup(name: &str, extra_file: bool) -> (Stores, CandidateSet) {
    let dir = scratch(name);
    let stores = Stores::open(&dir).unwrap();
    let host = HostRef { kind: "cli".into(), pid: 1, started_at: "t".into() };
    let rev = stores.index.begin_scan(&ScanInit { scan_id: "s".into(), scope_id: "scope".into(), root: RawPath::unix(b"/r".to_vec()).unwrap(),
        backend: "test".into(), host, started_at: "t".into() }).unwrap();
    let mut objects = vec![obj(1, ObjectKind::Directory, None), obj(2, ObjectKind::Directory, None), obj(3, ObjectKind::Directory, None),
        obj(4, ObjectKind::File, Some(100)), obj(5, ObjectKind::File, Some(200)), obj(6, ObjectKind::Directory, None), obj(7, ObjectKind::File, Some(300)),
        obj(8, ObjectKind::Directory, None), obj(9, ObjectKind::File, Some(1))];
    let mut entries = vec![ent(1, None, "/r", 1, ObjectKind::Directory), ent(2, Some(1), "app", 2, ObjectKind::Directory),
        ent(3, Some(2), "Cache", 3, ObjectKind::Directory), ent(4, Some(3), "a", 4, ObjectKind::File), ent(5, Some(3), "b", 5, ObjectKind::File),
        ent(6, Some(3), "sub", 6, ObjectKind::Directory), ent(7, Some(6), "c", 7, ObjectKind::File), ent(10, Some(3), "linkB", 4, ObjectKind::File),
        ent(8, Some(2), "Locked", 8, ObjectKind::Directory), ent(9, Some(8), "Cookies", 9, ObjectKind::File)];
    if extra_file {
        objects.push(obj(11, ObjectKind::File, Some(5)));
        entries.push(ent(11, Some(3), "new-after-plan", 11, ObjectKind::File));
    }
    stores.index.write_batch("s", rev, &objects, &entries, &[]).unwrap();
    stores.index.publish("s", rev, ScanState::Completed, "t").unwrap();
    let view = stores.index.view("s").unwrap();
    let rules = RuleSet { rules: vec![rule("cache", &["Cache"], Tier::DefaultSelected), rule("locked", &["Locked"], Tier::DefaultSelected)], digest: "rs1".into() };
    let roots = vec![
        ResolvedRoot { rule_id: "cache".into(), path: RawPath::unix(b"/r/app".to_vec()).unwrap(), volume_id: "v".into(), file_id: "inode64:0000000000000002".into() },
        ResolvedRoot { rule_id: "locked".into(), path: RawPath::unix(b"/r/app".to_vec()).unwrap(), volume_id: "v".into(), file_id: "inode64:0000000000000002".into() },
    ];
    let set = analyze(&view, &rules, &AnalysisInput { scan_id: "s", revision: rev, now_unix: NOW, roots: &roots, protected: &[] });
    stores.index.save_candidates(&set).unwrap();
    (stores, set)
}

fn cid(set: &CandidateSet, ends: &str) -> String {
    set.candidates.iter().find(|c| c.path.lossy().ends_with(ends)).unwrap().candidate_id.clone()
}

fn input(plan_id: &str) -> PlanInput<'_> {
    PlanInput { plan_id, scope_id: "scope", created_unix: NOW, observed_at: "2027-01-15T08:00:00Z", policy_digest: "0" }
}

#[test]
fn fixed_file_list_deterministic_digest_and_new_plan_on_change() {
    let (stores, set) = setup("fixed", false);
    let view = stores.index.view("s").unwrap();
    let cache = cid(&set, "Cache");
    let a = build_plan(&view, &set, std::slice::from_ref(&cache), Mode::Trash, &input("p1")).unwrap();
    let names: Vec<String> = a.body.items.iter().map(|i| i.path.lossy()).collect();
    assert_eq!(names, ["/r/app/Cache/a", "/r/app/Cache/b", "/r/app/Cache/sub/c"], "files only; the hard-link name adds no second action");
    assert_eq!(a.body.total_logical().unwrap().0, 600);
    let again = build_plan(&view, &set, std::slice::from_ref(&cache), Mode::Trash, &input("p1")).unwrap();
    assert_eq!(a.digest, again.digest, "same body, same digest in every entry point");
    let quarantine = build_plan(&view, &set, std::slice::from_ref(&cache), Mode::Quarantine, &input("p1")).unwrap();
    assert_ne!(a.digest, quarantine.digest, "changing the mode is a new plan body");
    assert!(matches!(build_plan(&view, &set, &[cid(&set, "Locked")], Mode::Trash, &input("p2")), Err(PlanError::NotPlannable(..))));
    assert!(matches!(build_plan(&view, &set, std::slice::from_ref(&cache), Mode::Delete, &input("p3")), Err(PlanError::ModeNotAllowed(..))));
    assert!(matches!(build_plan(&view, &set, &[], Mode::Trash, &input("p4")), Err(PlanError::Empty)));
    stores.audit.save_plan(&a).unwrap();
    assert_eq!(stores.audit.load_plan("p1").unwrap().digest, a.digest);

    // A file that appears later belongs to a later scan and never joins the stored plan.
    let (stores2, set2) = setup("fixed-later", true);
    let view2 = stores2.index.view("s").unwrap();
    let later = build_plan(&view2, &set2, &[cid(&set2, "Cache")], Mode::Trash, &input("p1")).unwrap();
    assert_eq!(later.body.items.len(), 4);
    assert_ne!(later.digest, a.digest);
    assert_eq!(stores.audit.load_plan("p1").unwrap().body.items.len(), 3);
}

#[test]
fn expiry_boundary_rule_changes_and_tampering_make_plans_stale() {
    let (stores, set) = setup("ttl", false);
    let view = stores.index.view("s").unwrap();
    let plan = build_plan(&view, &set, &[cid(&set, "Cache")], Mode::Trash, &input("p1")).unwrap();
    let expires = parse_utc(&plan.body.expires_at).unwrap();
    assert_eq!(expires - NOW, PLAN_TTL_SECONDS);
    assert_eq!(check_plan(&plan, expires - 1, "rs1"), Ok(()));
    assert_eq!(check_plan(&plan, expires, "rs1"), Err(StaleReason::Expired), "the expiry instant itself is too late");
    assert_eq!(check_plan(&plan, NOW, "rs2"), Err(StaleReason::RuleSetChanged));
    let mut tampered = plan.clone();
    tampered.body.mode = Mode::Delete;
    assert_eq!(check_plan(&tampered, NOW, "rs1"), Err(StaleReason::DigestMismatch));
    stores.audit.save_plan(&plan).unwrap();
    stores.audit.conn_for_tests().execute("UPDATE plans SET plan_json=replace(plan_json,'\"trash\"','\"delete\"')", []).unwrap();
    assert!(stores.audit.load_plan("p1").is_err(), "a stored body that no longer matches its digest is refused");
}

fn receipt(plan: &cm_core::plan::Plan) -> Receipt {
    Receipt { receipt_id: "r1".into(), plan_id: plan.body.plan_id.clone(), plan_digest: plan.digest.clone(), mode: plan.body.mode,
        principal: "user:alice".into(), source: ReceiptSource::InteractiveReview, policy_id: None, policy_version: None,
        created_at: plan.body.created_at.clone(), expires_at: plan.body.expires_at.clone(), revoked_at: None }
}

#[test]
fn receipts_bind_digest_mode_principal_and_lifetime() {
    let (stores, set) = setup("receipt", false);
    let view = stores.index.view("s").unwrap();
    let plan = build_plan(&view, &set, &[cid(&set, "Cache")], Mode::Trash, &input("p1")).unwrap();
    let r = receipt(&plan);
    assert!(receipt_valid(&r, &plan, "user:alice", NOW));
    assert!(!receipt_valid(&r, &plan, "user:mallory", NOW), "another principal");
    assert!(!receipt_valid(&r, &plan, "user:alice", NOW + PLAN_TTL_SECONDS), "expired with the plan");
    let other = build_plan(&view, &set, &[cid(&set, "Cache")], Mode::Quarantine, &input("p1")).unwrap();
    assert!(!receipt_valid(&r, &other, "user:alice", NOW), "a changed body or mode needs a new approval");
    stores.audit.save_plan(&plan).unwrap();
    stores.audit.save_receipt(&r).unwrap();
    stores.audit.revoke_receipts("p1", "t").unwrap();
    let stored = &stores.audit.receipts_for("p1").unwrap()[0];
    assert!(!receipt_valid(stored, &plan, "user:alice", NOW), "revoked");
}

#[test]
fn policies_cover_only_exact_scope_rules_mode_tier_and_known_sizes() {
    let (stores, set) = setup("policy", false);
    let view = stores.index.view("s").unwrap();
    let plan = build_plan(&view, &set, &[cid(&set, "Cache")], Mode::Trash, &input("p1")).unwrap();
    let mut p = Policy::template("pol", "user:alice", "scope", vec!["cache@1.0.0".into()], Mode::Trash, NOW);
    assert_eq!(p.state, PolicyState::Paused, "templates start paused; the user enables them deliberately");
    assert_eq!(policy_covers(&p, &plan, "user:alice", NOW), Err(PolicyMismatch::NotEnabled));
    p.state = PolicyState::Enabled;
    assert_eq!(policy_covers(&p, &plan, "user:alice", NOW), Ok(()));
    let mut upgraded = p.clone();
    upgraded.rules = vec!["cache@0.9.0".into()];
    assert!(matches!(policy_covers(&upgraded, &plan, "user:alice", NOW), Err(PolicyMismatch::RuleNotCovered(_))));
    let mut wrong_mode = p.clone();
    wrong_mode.mode = Mode::Quarantine;
    assert_eq!(policy_covers(&wrong_mode, &plan, "user:alice", NOW), Err(PolicyMismatch::ModeDiffers));
    let mut tiny = p.clone();
    tiny.single_max_bytes = 10;
    assert_eq!(policy_covers(&tiny, &plan, "user:alice", NOW), Err(PolicyMismatch::SingleLimit));
    let mut review = plan.clone();
    review.tiers[0] = Tier::ReviewOnly;
    assert_eq!(policy_covers(&p, &review, "user:alice", NOW), Err(PolicyMismatch::TierNotDefaultSelected));
    assert_eq!(policy_covers(&p, &plan, "user:alice", NOW + 31 * 86_400), Err(PolicyMismatch::Expired));
    let mut revoked = p.clone();
    revoked.state = PolicyState::Revoked;
    assert_eq!(policy_covers(&revoked, &plan, "user:alice", NOW), Err(PolicyMismatch::NotEnabled));
}

#[test]
fn quotas_are_atomic_idempotent_and_hold_unknown_outcomes() {
    let dir = scratch("quota");
    let stores = Stores::open(&dir).unwrap();
    let mut p = Policy::template("pol", "user:alice", "scope", vec![], Mode::Trash, NOW);
    p.window_max_bytes = 1_000;
    p.window_max_items = 10;
    assert!(stores.audit.reserve_quota(&p, "op1", "p1", 600, 3, NOW).unwrap());
    assert!(stores.audit.reserve_quota(&p, "op1", "p1", 600, 3, NOW).unwrap(), "the same operation never reserves twice");
    assert!(!stores.audit.reserve_quota(&p, "op2", "p2", 600, 3, NOW).unwrap(), "two plans together would exceed the window");
    stores.audit.settle_quota("op1", (200, 1), (100, 1)).unwrap();
    assert_eq!(stores.audit.policy_usage("pol", NOW).unwrap().bytes, 300, "consumed plus held unknown; the rest is released");
    assert!(stores.audit.reserve_quota(&p, "op3", "p3", 700, 3, NOW).unwrap());
    assert_eq!(stores.audit.policy_usage("pol", NOW + 86_401).unwrap().bytes, 0, "rolling 24-hour window");

    // Two connections racing for the last allowance: at most one wins.
    let dir2 = scratch("quota-race");
    let _ = Stores::open(&dir2).unwrap();
    let mut q = p.clone();
    q.window_max_bytes = 500;
    let handles: Vec<_> = (0..2)
        .map(|n| {
            let (d, q) = (dir2.clone(), q.clone());
            std::thread::spawn(move || {
                let s = Stores::open(&d).unwrap();
                s.audit.reserve_quota(&q, &format!("race{n}"), "p", 400, 1, NOW).unwrap()
            })
        })
        .collect();
    let wins = handles.into_iter().map(|h| h.join().unwrap()).filter(|w| *w).count();
    assert_eq!(wins, 1);
    let _ = BTreeMap::<u8, u8>::new();
}
