//! End to end over the real Windows backend, stores and actions: what the desktop does from a
//! published scan to a reviewed plan, its application, the report, quarantine and a reviewed
//! restore. Only files under a temporary directory are touched.

use std::sync::atomic::AtomicBool;

use cm_app::scan::{ScanRequest, run_scan};
use cm_core::host::HostProbe;
use cm_core::rules::Mode;
use cm_store::Stores;
use cm_store::index::{EntryQuery, Filter, KindFilter, Sort};

use crate::host::AppState;
use crate::{cleanup, ops};

fn candidate_set(scan_id: &str, entry_id: u64, path: &cm_core::path::RawPath, bytes: u64, digest: &str) -> cm_core::analysis::CandidateSet {
    serde_json::from_value(serde_json::json!({
        "candidate_set_id": "cs_flow", "scan_id": scan_id, "revision": 1, "rule_set_digest": digest, "protection_version": "v",
        "candidates": [{
            "candidate_id": "c_flow", "entry_id": entry_id, "path": path, "kind": "directory",
            "rules": [{"rule_id": "test.cache", "version": "1.0.0"}], "category": "app_cache",
            "state": "allowed", "tier": "default_selected", "reasons": ["test rule"], "protection": null,
            "logical": {"known": bytes.to_string(), "unknown_count": 0},
            "allowed_modes": ["quarantine"], "probes": [], "impact": "rebuilt by the test", "owner_id": null,
        }, {
            "candidate_id": "c_blocked", "entry_id": entry_id, "path": path, "kind": "directory",
            "rules": [{"rule_id": "test.cache", "version": "1.0.0"}], "category": "app_cache",
            "state": "blocked", "tier": "blocked", "reasons": ["protected"], "protection": "database",
            "logical": {"known": "1", "unknown_count": 0},
            "allowed_modes": [], "probes": [], "impact": "", "owner_id": null,
        }],
        "inapplicable": [],
    }))
    .expect("candidate set shape")
}

#[test]
fn reviewed_plan_applies_reports_quarantine_truthfully_and_restores() {
    let base = std::env::temp_dir().join(format!("cm-desktop-flow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("root");
    let store = base.join("store");
    let rules = base.join("rules");
    std::fs::create_dir_all(root.join("cache")).unwrap();
    std::fs::create_dir_all(rules.join(if cfg!(windows) { "windows" } else { "macos" })).unwrap();
    std::fs::write(root.join("cache").join("a.bin"), vec![1u8; 3000]).unwrap();
    std::fs::write(root.join("cache").join("b.bin"), vec![2u8; 1000]).unwrap();
    std::fs::write(root.join("keep.txt"), b"keep").unwrap();
    let digest = crate::rules::load(Some(&rules)).expect("empty rule pack loads").digest;

    // Scan and publish.
    let stores = Stores::open(&store).unwrap();
    let request = ScanRequest {
        scan_id: "s_flow".into(),
        scope_id: "scope_flow".into(),
        root: cm_platform::paths::raw_path(root.as_os_str()).unwrap(),
        host: cm_platform::host::OsHostProbe.current("desktop"),
    };
    cm_core_scan(&stores, &request);
    let page = stores
        .index
        .list_entries(&EntryQuery {
            scan_id: "s_flow".into(),
            sort: Sort::NameAsc,
            filter: Filter { kind: KindFilter::Directory, name_contains: Some("cache".into()), parent_entry_id: None },
            page_size: 5,
            cursor: None,
        })
        .unwrap();
    let cache = &page.items[0];
    let set = candidate_set("s_flow", cache.entry_id.parse().unwrap(), &cache.path, 4000, &digest);
    stores.index.save_candidates(&set).unwrap();

    let s = AppState::with_store(Some(store.clone()), Some(rules.clone()), None);
    // A set read back from the index has no live catalog facts: nothing is plannable from it.
    s.with_stores(|st| {
        assert_eq!(cleanup::create(&s, st, "cs_flow", &["c_flow".into()], Mode::Quarantine).unwrap_err().reason, "not_selectable");
        Ok(())
    })
    .unwrap();
    // The facts an analysis in this process produced (as `analyze_scan` caches them).
    let facts = set.candidates.iter().map(|c| (c.candidate_id.clone(), cm_core::analysis::CatalogFacts::default())).collect();
    *crate::host::lock(&s.candidates) =
        Some(std::sync::Arc::new(cleanup::Loaded { set, catalog: Some(cm_core::analysis::CatalogSet { facts, undetermined: vec![] }) }));
    s.with_stores(|st| {
        // Analysis with the (empty) installed pack runs on the published scan.
        let set = cleanup::latest(&s, st, "s_flow")?.expect("stored set");
        assert_eq!((set.groups.low.count, set.groups.blocked.count), (1, 1));

        // Blocked candidates cannot be planned; the selectable one can.
        let refused = cleanup::create(&s, st, "cs_flow", &["c_blocked".into()], Mode::Quarantine).unwrap_err();
        assert_eq!(refused.reason, "not_selectable");
        let plan_id = cleanup::create(&s, st, "cs_flow", &["c_flow".into()], Mode::Quarantine)?.plan_id;

        // Full review detail: every file, the digest, no authorization yet, a review token.
        let d = cleanup::detail(&s, st, &plan_id)?;
        assert_eq!((d.kind, d.mode, d.item_count, d.total_logical_bytes.as_deref()), ("cleanup", Some(Mode::Quarantine), 2, Some("4000")));
        assert_eq!(d.authorization.status, "required");
        assert_eq!(d.stale_reason, None);
        assert_eq!(d.not_included.as_ref().map(|n| n.blocked), Some(1));
        assert!(d.cli.review.ends_with(&plan_id));
        let token = d.review_token.clone().expect("review token while approval is possible");
        let items = cleanup::items(st, &plan_id, 1, None)?;
        assert_eq!((items.total, items.items.len(), items.next_cursor.as_deref()), (2, 1, Some("o1")));

        // Nothing runs without approval; approval needs the token and the typed digest prefix.
        assert_eq!(ops::precheck_for_tests(&s, st, &plan_id).unwrap_err().reason, "needs_approval");
        assert_eq!(cleanup::approve(&s, st, &plan_id, &token, "00000000").unwrap_err().reason, "confirmation_mismatch");
        assert_eq!(cleanup::approve(&s, st, &plan_id, "rv_forged", &d.digest_prefix).unwrap_err().reason, "review_expired");
        let auth = cleanup::approve(&s, st, &plan_id, &token, &d.digest_prefix.to_uppercase())?;
        assert_eq!((auth.status, auth.source), ("authorized", Some(cm_core::authz::ReceiptSource::InteractiveReview)));
        assert!(cleanup::approve(&s, st, &plan_id, &token, &d.digest_prefix).is_err(), "a review token is single use");
        ops::precheck_for_tests(&s, st, &plan_id)?;
        Ok(plan_id)
    })
    .and_then(|plan_id| {
        // Apply on its own connections, as the background thread does.
        let worker = Stores::open(&store).unwrap();
        let report = ops::run_for_tests(&worker, &plan_id, &digest, &AtomicBool::new(false)).expect("applied");
        assert_eq!((report.status.as_str(), report.succeeded), ("completed", 2));
        assert_eq!(report.quarantined_logical_bytes, "4000", "moved into quarantine");
        assert_eq!(report.estimated_freed_bytes, "0", "a same-volume quarantine frees nothing by itself");
        assert!(!root.join("cache").join("a.bin").exists());
        assert!(root.join("keep.txt").exists(), "only planned files were touched");

        s.with_stores(|st| {
            let op = ops::operation(&s, st, &report.operation_id)?;
            assert_eq!((op.state.as_str(), op.counts.succeeded, op.total, op.needs_reconcile), ("completed", 2, 2, false));
            assert_eq!(ops::operation_items(st, &report.operation_id, "problems", 50, None)?.total, 0);
            assert_eq!(ops::operation_items(st, &report.operation_id, "all", 50, None)?.items[0].status, "succeeded");
            // The same plan never runs twice: the detail points at the existing operation.
            assert_eq!(cleanup::detail(&s, st, &plan_id)?.operation.map(|o| o.operation_id), Some(report.operation_id.clone()));

            let q = ops::quarantine(st, 50, None)?;
            assert_eq!((q.held_count, q.held_logical_bytes.as_deref()), (Some(2), Some("4000")));
            assert!(q.items.iter().all(|i| i.state == "recoverable" && i.actionable));
            let ids: Vec<String> = q.items.iter().map(|i| i.quarantine_id.clone()).collect();
            let rp = ops::recovery_plan(st, cm_core::recovery::RecoveryKind::Restore, &ids)?.plan_id;
            let d = cleanup::detail(&s, st, &rp)?;
            assert_eq!((d.kind, d.item_count, d.authorization.status), ("restore", 2, "required"));
            assert_eq!(ops::precheck_for_tests(&s, st, &rp).unwrap_err().reason, "needs_approval");
            cleanup::approve(&s, st, &rp, d.review_token.as_deref().unwrap(), &d.digest_prefix)?;
            Ok(rp)
        })
    })
    .and_then(|rp| {
        let worker = Stores::open(&store).unwrap();
        let report = ops::run_for_tests(&worker, &rp, &digest, &AtomicBool::new(false)).expect("restored");
        assert_eq!((report.status.as_str(), report.succeeded), ("completed", 2));
        assert_eq!(std::fs::read(root.join("cache").join("b.bin")).unwrap(), vec![2u8; 1000], "restored in place");
        s.with_stores(|st| {
            let h = ops::history(st, 10, None)?;
            assert_eq!(h.items.iter().map(|i| i.action.as_deref()).collect::<Vec<_>>(), [Some("restore"), Some("quarantine")]);
            assert_eq!(ops::quarantine(st, 50, None)?.held_count, Some(0));
            let plans = ops::plans(st, 10, None)?;
            assert_eq!(plans.items.len(), 2);
            assert!(plans.items.iter().all(|p| p.operation_id.is_some()));
            Ok(())
        })
    })
    .unwrap();
    drop(stores);
    drop(s);
    let _ = std::fs::remove_dir_all(&base);
}

fn cm_core_scan(stores: &Stores, request: &ScanRequest) {
    let backend = cm_platform::windows::directory::DirectoryBackend;
    let out = run_scan(&stores.index, &backend, request, &cm_core::scan::ScanOptions::default()).unwrap();
    assert_eq!(out.state, cm_core::model::ScanState::Completed);
}

/// A manual Recycle Bin plan (plan/v2) goes through the same review page: detail with a review token, every item,
/// approval with the typed digest prefix, precheck, plan list and revocation. Nothing is applied here, so the real
/// Recycle Bin is never touched.
#[test]
fn manual_trash_plan_is_reviewed_like_any_plan() {
    let base = std::env::temp_dir().join(format!("cm-desktop-mtr-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("root");
    let store = base.join("store");
    std::fs::create_dir_all(root.join("old")).unwrap();
    std::fs::write(root.join("old").join("a.bin"), vec![1u8; 300]).unwrap();
    std::fs::write(root.join("keep.txt"), b"keep").unwrap();
    let stores = Stores::open(&store).unwrap();
    let request = ScanRequest {
        scan_id: "s_mtr".into(),
        scope_id: "scope_mtr".into(),
        root: cm_platform::paths::raw_path(root.as_os_str()).unwrap(),
        host: cm_platform::host::OsHostProbe.current("desktop"),
    };
    cm_core_scan(&stores, &request);
    let entry: u64 = stores
        .index
        .list_entries(&EntryQuery {
            scan_id: "s_mtr".into(),
            sort: Sort::NameAsc,
            filter: Filter { kind: KindFilter::Directory, name_contains: Some("old".into()), parent_entry_id: None },
            page_size: 5,
            cursor: None,
        })
        .unwrap()
        .items[0]
        .entry_id
        .parse()
        .unwrap();
    let s = AppState::with_store(Some(store.clone()), None, None);
    let ins = crate::suite::win::trash_inspect(&s, "s_mtr", entry).unwrap();
    assert!(ins.trashable, "{:?}", ins.refusal);
    assert_eq!((ins.name.as_str(), ins.files, ins.logical_bytes.as_deref()), ("old", 1, Some("300")));
    let plan_id = crate::suite::win::trash_plan(&s, "s_mtr", &[entry]).unwrap().plan_id;
    s.with_stores(|st| {
        let d = cleanup::detail(&s, st, &plan_id)?;
        assert_eq!((d.kind, d.mode, d.plan_kind_v2.as_deref(), d.requires_admin, d.catalog), ("manual_trash", Some(Mode::Trash), Some("manual_selection"), false, None));
        assert_eq!((d.authorization.status, d.stale_reason.as_deref(), d.item_count), ("required", None, 1));
        let token = d.review_token.clone().expect("review token");
        let items = cleanup::items(st, &plan_id, 50, None)?;
        let i = &items.items[0];
        assert_eq!((i.name.as_str(), i.item_kind.as_str(), i.action_type.as_deref(), i.logical_bytes.as_str()), ("old", "directory", Some("fs_trash"), "300"));

        assert_eq!(ops::precheck_for_tests(&s, st, &plan_id).unwrap_err().reason, "needs_approval");
        assert_eq!(cleanup::approve(&s, st, &plan_id, &token, "00000000").unwrap_err().reason, "confirmation_mismatch");
        let auth = cleanup::approve(&s, st, &plan_id, &token, &d.digest_prefix)?;
        assert_eq!((auth.status, auth.source), ("authorized", Some(cm_core::authz::ReceiptSource::InteractiveReview)));
        ops::precheck_for_tests(&s, st, &plan_id)?;
        let row = ops::plans(st, 10, None)?.items.into_iter().find(|p| p.plan_id == plan_id).expect("listed");
        assert_eq!((row.kind, row.authorization, row.problem), ("manual_trash", Some("authorized"), None));
        assert_eq!(cleanup::policy_from_plan(st, &plan_id).unwrap_err().reason, "not_cleanup_plan");

        assert_eq!(cleanup::revoke(st, &plan_id)?.status, "revoked");
        assert_eq!(ops::precheck_for_tests(&s, st, &plan_id).unwrap_err().reason, "needs_approval");
        Ok(())
    })
    .unwrap();
    assert!(root.join("old").join("a.bin").exists(), "creating, approving and revoking touch nothing");
    drop(stores);
    drop(s);
    let _ = std::fs::remove_dir_all(&base);
}
