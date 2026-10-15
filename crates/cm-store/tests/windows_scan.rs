//! End to end on Windows: win.directory → staging revision → publish → revision-bound queries.
#![cfg(windows)]

use std::os::windows::ffi::OsStrExt;

use cm_core::host::HostProbe;
use cm_core::model::ScanState;
use cm_core::path::RawPath;
use cm_core::scan::{ScanBackend, ScanOptions};
use cm_fixtures::{Profile, create, prepare_sentinel, verify, verify_sentinel};
use cm_platform::host::OsHostProbe;
use cm_platform::windows::directory::DirectoryBackend;
use cm_platform::windows::privileges::disable_bypass_privileges;
use cm_store::Stores;
use cm_store::index::{EntryQuery, Filter, KindFilter, ScanInit, Sort, StagingSink};

#[test]
fn fixture_scan_publishes_queryable_revision_without_touching_targets() {
    disable_bypass_privileges().unwrap();
    let base = std::env::temp_dir().join(format!("cm-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (root, sentinel, store_dir) = (base.join("root"), base.join("sentinel"), base.join("store"));
    prepare_sentinel(&sentinel).unwrap();
    let manifest = create(Profile::P0_10k, 7, &root, &sentinel).unwrap();
    let stores = Stores::open(&store_dir).unwrap();
    stores.ensure_outside(&root).unwrap();

    let raw = RawPath::windows(root.as_os_str().encode_wide().collect()).unwrap();
    let host = OsHostProbe.current("cli");
    let rev = stores.index.begin_scan(&ScanInit { scan_id: "e2e".into(), scope_id: "scope".into(), root: raw.clone(),
        backend: "win.directory".into(), host, started_at: cm_core::time::now_utc() }).unwrap();
    let mut sink = StagingSink { store: &stores.index, scan_id: "e2e".into(), revision: rev };
    let report = DirectoryBackend.scan(&raw, &mut sink, &ScanOptions::default());
    assert_eq!(report.state, ScanState::Completed, "{:?}", report.issues);
    stores.index.record_report("e2e", &report).unwrap();
    let state = stores.index.publish("e2e", rev, report.state, &cm_core::time::now_utc()).unwrap();
    assert_eq!(state, ScanState::Completed);

    let status = stores.index.scan_status("e2e").unwrap();
    assert_eq!(status.completeness.as_deref(), Some("verified_scope"));
    // root + 100 dirs + 10,000 files + the fixture root marker
    assert_eq!(status.entries_seen, 1 + manifest.plan.counts.directories + manifest.plan.counts.files + 1);
    let stored = stores.index.scan_report("e2e").unwrap().unwrap();
    assert!(stored["backend_reason"].as_str().unwrap().contains("ordinary permissions"));

    let largest = stores.index.list_entries(&EntryQuery { scan_id: "e2e".into(), sort: Sort::LogicalDesc,
        filter: Filter { kind: KindFilter::File, name_contains: None, parent_entry_id: None }, page_size: 5, cursor: None }).unwrap();
    assert_eq!(largest.items.len(), 5);
    let sizes: Vec<u64> = largest.items.iter().map(|i| i.logical_bytes.unwrap().0).collect();
    assert!(sizes.windows(2).all(|w| w[0] >= w[1]) && sizes[0] <= 4_096);

    let root_entry = stores.index.list_entries(&EntryQuery { scan_id: "e2e".into(), sort: Sort::LogicalDesc,
        filter: Filter { kind: KindFilter::Directory, name_contains: None, parent_entry_id: None }, page_size: 1, cursor: None }).unwrap();
    let total = root_entry.items[0].logical_bytes.unwrap().0;
    let marker = std::fs::metadata(root.join(cm_fixtures::ROOT_MARKER)).unwrap().len();
    assert_eq!(total, manifest.plan.logical_bytes.parse::<u64>().unwrap() + marker, "root subtree equals manifest bytes");

    verify(&root).unwrap();
    verify_sentinel(&sentinel).unwrap();
    drop(stores);
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn scan_analyze_persist_and_page_candidates() {
    use cm_core::analysis::{AnalysisInput, CandidateState, ResolvedRoot, analyze};
    use cm_core::capability::Platform;
    use cm_core::rules::{ActionKind, Impact, Match, Mode, ObjectType, Owner, OwnerKind, RebuildClass, RootResolver, Rule, RuleSet, RuleStatus, Thresholds, Tier};
    use cm_platform::windows::directory::identity_of;

    disable_bypass_privileges().unwrap();
    let base = std::env::temp_dir().join(format!("cm-analyze-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (root, store_dir) = (base.join("root"), base.join("store"));
    let app = root.join("App");
    std::fs::create_dir_all(app.join("Code Cache")).unwrap();
    std::fs::create_dir_all(app.join("GPUCache")).unwrap();
    std::fs::write(app.join("Code Cache").join("js.bin"), vec![1u8; 700]).unwrap();
    std::fs::write(app.join("GPUCache").join("data"), vec![2u8; 300]).unwrap();
    std::fs::write(app.join("GPUCache").join("Cookies"), b"session").unwrap();
    let temp = root.join("Temp");
    std::fs::create_dir_all(&temp).unwrap();
    std::fs::write(temp.join("a.tmp"), vec![3u8; 50]).unwrap();
    std::fs::hard_link(temp.join("a.tmp"), temp.join("b.tmp")).unwrap();

    let stores = Stores::open(&store_dir).unwrap();
    let raw = RawPath::windows(root.as_os_str().encode_wide().collect()).unwrap();
    let rev = stores.index.begin_scan(&ScanInit { scan_id: "an".into(), scope_id: "scope".into(), root: raw.clone(),
        backend: "win.directory".into(), host: OsHostProbe.current("cli"), started_at: cm_core::time::now_utc() }).unwrap();
    let mut sink = StagingSink { store: &stores.index, scan_id: "an".into(), revision: rev };
    let report = DirectoryBackend.scan(&raw, &mut sink, &ScanOptions::default());
    stores.index.publish("an", rev, report.state, &cm_core::time::now_utc()).unwrap();

    let rule = |id: &str, comps: &[&str], glob: Option<&str>| Rule {
        schema_version: "2.0".into(), rule_id: id.into(), version: "1.0.0".into(), status: RuleStatus::Enabled,
        platforms: vec![Platform::Windows], category: "app_caches".into(), max_tier: Tier::DefaultSelected,
        rebuild_class: RebuildClass::LocalRebuild, owner: Owner { kind: OwnerKind::App, id: "test.app".into() },
        root_resolver: RootResolver::ApplicationCache,
        matcher: Match { object_type: ObjectType::Any, relative_components: comps.iter().map(|s| s.to_string()).collect(),
            name_glob: glob.map(str::to_owned), extensions: vec![], require_cachedir_tag: false, archive_payload: false, max_depth: 0 },
        exclude: vec![], probes: vec![], thresholds: Thresholds { min_age_days: None, age_basis: None, min_size_bytes: None, retention_days: None },
        action_kind: ActionKind::Filesystem, adapter_ref: None, allowed_modes: vec![Mode::Trash], requires_admin: false,
        protect_descendants: true, follow_links: false,
        impact: Impact { summary: "rebuilt".into(), regeneration: "x".into(), offline_effect: "x".into() },
        tested_registry_ref: Some("t@1.0.0".into()), evidence_refs: vec!["test".into()],
    };
    let rules = RuleSet { rules: vec![rule("code", &["Code Cache"], None), rule("gpu", &["GPUCache"], None), rule("temp", &[], Some("*.tmp"))], digest: "test".into() };
    let id = |p: &std::path::Path| identity_of(&p.as_os_str().encode_wide().collect::<Vec<_>>()).unwrap();
    let resolved: Vec<ResolvedRoot> = [("code", &app), ("gpu", &app), ("temp", &temp)]
        .iter()
        .map(|(r, p)| { let (v, f) = id(p); ResolvedRoot { rule_id: (*r).into(), path: raw.clone(), volume_id: v, file_id: f } })
        .collect();
    let view = stores.index.view("an").unwrap();
    let set = analyze(&view, &rules, &AnalysisInput { scan_id: "an", revision: rev, now_unix: i64::MAX / 2, roots: &resolved, protected: &[] });
    let state = |name: &str| set.candidates.iter().find(|c| c.path.lossy().ends_with(name)).map(|c| (c.state, c.tier));
    assert_eq!(state("Code Cache"), Some((CandidateState::Allowed, Tier::DefaultSelected)));
    assert_eq!(state("GPUCache").map(|s| s.0), Some(CandidateState::Blocked), "Cookies inside blocks the parent");
    let temps: Vec<_> = set.candidates.iter().filter(|c| c.path.lossy().ends_with(".tmp")).collect();
    assert_eq!(temps.len(), 1, "hard-linked a.tmp/b.tmp yield one action");
    assert_eq!(temps[0].tier, Tier::ReviewOnly);
    let code = set.candidates.iter().find(|c| c.path.lossy().ends_with("Code Cache")).unwrap();
    assert_eq!(code.logical.exact().map(|b| b.0), Some(700));

    stores.index.save_candidates(&set).unwrap();
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let (page, next) = stores.index.list_candidates(&set.candidate_set_id, 1, cursor.as_deref()).unwrap();
        seen.extend(page.into_iter().map(|c| c["candidate_id"].as_str().unwrap().to_owned()));
        match next { Some(c) => cursor = Some(c), None => break }
    }
    assert_eq!(seen.len(), set.candidates.len());
    assert!(stores.index.list_candidates(&set.candidate_set_id, 501, None).is_err());
    drop(stores);
    std::fs::remove_dir_all(&base).unwrap();
}
