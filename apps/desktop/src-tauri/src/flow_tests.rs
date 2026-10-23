//! End to end over the real platform backend, stores and actions (Windows, or macOS through the same `crate::plat`
//! backends as the CLI): what the desktop does from a published scan to a reviewed plan, its application, the report,
//! quarantine and a reviewed restore. Only files under a temporary directory are touched.

use std::sync::atomic::AtomicBool;

use cm_app::scan::{ScanRequest, run_scan};
use cm_core::host::HostProbe;
use cm_core::rules::Mode;
use cm_store::Stores;
use cm_store::index::{EntryQuery, Filter, KindFilter, Sort};

use crate::host::AppState;
use crate::{cleanup, ops};

/// The temporary directory; canonical on macOS (`/var` is a link to `/private/var`, and the quarantine's
/// trusted-namespace gate walks the real parent chain).
fn temp_base(tag: &str) -> std::path::PathBuf {
    let tmp = std::env::temp_dir();
    #[cfg(target_os = "macos")]
    let tmp = std::fs::canonicalize(&tmp).unwrap();
    tmp.join(format!("cm-desktop-{tag}-{}", std::process::id()))
}

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
    let base = temp_base("flow");
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
    let out = run_scan(&stores.index, crate::plat::scan_backend().as_ref(), request, &cm_core::scan::ScanOptions::default()).unwrap();
    assert_eq!(out.state, cm_core::model::ScanState::Completed);
}

/// A manual Recycle Bin / Trash plan (plan/v2) goes through the same review page: detail with a review token, every
/// item, approval with the typed digest prefix, precheck, plan list and revocation. Nothing is applied here, so the real
/// bin is never touched. Windows moves the folder; macOS moves single verified files and refuses the folder up front.
#[test]
fn manual_trash_plan_is_reviewed_like_any_plan() {
    let base = temp_base("mtr");
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("root");
    let store = base.join("store");
    std::fs::create_dir_all(root.join("trash-me")).unwrap();
    std::fs::write(root.join("trash-me").join("a.bin"), vec![1u8; 300]).unwrap();
    std::fs::write(root.join("keep.txt"), b"keep").unwrap();
    let stores = Stores::open(&store).unwrap();
    let request = ScanRequest {
        scan_id: "s_mtr".into(),
        scope_id: "scope_mtr".into(),
        root: cm_platform::paths::raw_path(root.as_os_str()).unwrap(),
        host: cm_platform::host::OsHostProbe.current("desktop"),
    };
    cm_core_scan(&stores, &request);
    let find = |kind: KindFilter, name: &str| -> u64 {
        stores
            .index
            .list_entries(&EntryQuery {
                scan_id: "s_mtr".into(),
                sort: Sort::NameAsc,
                filter: Filter { kind, name_contains: Some(name.into()), parent_entry_id: None },
                page_size: 5,
                cursor: None,
            })
            .unwrap()
            .items[0]
            .entry_id
            .parse()
            .unwrap()
    };
    // Not a name the temporary path contains (macOS: `/private/var/folders/…` holds "old").
    let folder = find(KindFilter::Directory, "trash-me");
    let s = AppState::with_store(Some(store.clone()), None, None);
    let (entry, name, kind) = if cfg!(windows) {
        (folder, "trash-me", "directory")
    } else {
        // macOS: a folder is refused before anything is planned (its contents cannot be verified as one object).
        let ins = crate::suite::imp::trash_inspect(&s, "s_mtr", folder).unwrap();
        assert!(!ins.trashable && ins.refusal.as_ref().is_some_and(|r| r.reason == "folder_move_unavailable"), "{:?}", ins.refusal);
        assert_eq!(crate::suite::imp::trash_plan(&s, "s_mtr", &[folder]).unwrap_err().reason, "folder_move_unavailable");
        (find(KindFilter::File, "a.bin"), "a.bin", "file")
    };
    let ins = crate::suite::imp::trash_inspect(&s, "s_mtr", entry).unwrap();
    assert!(ins.trashable, "{:?}", ins.refusal);
    assert_eq!((ins.name.as_str(), ins.files, ins.logical_bytes.as_deref()), (name, 1, Some("300")));
    let plan_id = crate::suite::imp::trash_plan(&s, "s_mtr", &[entry]).unwrap().plan_id;
    // D15: approving or applying a manual_selection plan needs trash.manual on (experimental features, off by default).
    let off = crate::gate::require_plan(&s, &plan_id).unwrap_err();
    assert_eq!((off.code, off.reason), (cm_core::error::ErrorCode::Unsupported, "experimental_off"));
    assert!(off.message.contains("trash.manual") && off.message.contains("G1"), "{}", off.message);
    cm_app::features::set_experimental(&store, true).unwrap();
    crate::gate::require_plan(&s, &plan_id).unwrap();
    s.with_stores(|st| {
        let d = cleanup::detail(&s, st, &plan_id)?;
        assert_eq!((d.kind, d.mode, d.plan_kind_v2.as_deref(), d.requires_admin, d.catalog), ("manual_trash", Some(Mode::Trash), Some("manual_selection"), false, None));
        assert_eq!((d.authorization.status, d.stale_reason.as_deref(), d.item_count), ("required", None, 1));
        let token = d.review_token.clone().expect("review token");
        let items = cleanup::items(st, &plan_id, 50, None)?;
        let i = &items.items[0];
        assert_eq!((i.name.as_str(), i.item_kind.as_str(), i.action_type.as_deref(), i.logical_bytes.as_str()), (name, kind, Some("fs_trash"), "300"));

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
    assert!(root.join("trash-me").join("a.bin").exists(), "creating, approving and revoking touch nothing");
    drop(stores);
    drop(s);
    let _ = std::fs::remove_dir_all(&base);
}

/// Scan `root` into `store` as `scan_id` and return the published view.
fn scan_into(stores: &Stores, root: &std::path::Path, scan_id: &str) -> cm_core::analysis::MemView {
    let request = ScanRequest {
        scan_id: scan_id.into(),
        scope_id: format!("scope_{scan_id}"),
        root: cm_platform::paths::raw_path(root.as_os_str()).unwrap(),
        host: cm_platform::host::OsHostProbe.current("desktop"),
    };
    cm_core_scan(stores, &request);
    stores.index.view(scan_id).unwrap()
}

/// Entry id of the object named `name` (case-insensitive) whose parent is named `parent`.
fn entry_named(view: &cm_core::analysis::MemView, parent: &str, name: &str) -> u64 {
    use cm_core::analysis::IndexView;
    let low = |e: u64| view.name(e).lossy().to_lowercase();
    view.facts
        .keys()
        .copied()
        .find(|&e| low(e) == name.to_lowercase() && view.parent(e).is_some_and(|p| low(p).ends_with(&parent.to_lowercase())))
        .unwrap_or_else(|| panic!("{parent}/{name} indexed"))
}

fn catalog_candidate(id: &str, view: &cm_core::analysis::MemView, entry: u64, state: &str, reasons: &[&str], rule: &str, logical: u64) -> serde_json::Value {
    use cm_core::analysis::IndexView;
    let kind = if view.info(entry).kind == cm_core::model::ObjectKind::Directory { "directory" } else { "file" };
    serde_json::json!({
        "candidate_id": id, "entry_id": entry, "path": view.path(entry), "kind": kind, "rules": [{"rule_id": rule, "version": "1.0.0"}],
        "category": "build_output", "state": state, "tier": if state == "allowed" { "default_selected" } else { "review_only" }, "reasons": reasons,
        "protection": null, "logical": {"known": logical.to_string(), "unknown_count": 0}, "allowed_modes": ["trash", "quarantine"], "probes": [],
        "impact": "rebuilt", "owner_id": null,
    })
}

/// NEW-09 / WFA-50, WFA-58, WFA-113 for discovery plans in the desktop: a folder whose inner file changed since
/// discovery is listed as stale (the CLI's folder-aware check) while the unchanged folder is planned; a mounted image is
/// refused as not plannable (never "changed"); a selection whose folders hold no files is an empty selection.
#[test]
fn catalog_plans_check_selections_like_the_cli() {
    use crate::suite::{Catalog, imp::catalog_plan};
    let base = temp_base("catalog-checks");
    let _ = std::fs::remove_dir_all(&base);
    let (root, store) = (base.join("root"), base.join("store"));
    for (dir, len) in [("rustold", 2000usize), ("rustnew", 300)] {
        std::fs::create_dir_all(root.join(dir).join("target").join("debug")).unwrap();
        std::fs::write(root.join(dir).join("target").join("debug").join("x.o"), vec![1u8; len]).unwrap();
    }
    std::fs::create_dir_all(root.join("emptied").join("target").join("debug")).unwrap();
    std::fs::write(root.join("disk.iso"), vec![0u8; 64]).unwrap();
    let stores = Stores::open(&store).unwrap();
    let view = scan_into(&stores, &root, "s_cat");
    let (old, new) = (entry_named(&view, "rustold", "target"), entry_named(&view, "rustnew", "target"));
    let emptied = entry_named(&view, "emptied", "target");
    let purge: cm_core::analysis::CandidateSet = serde_json::from_value(serde_json::json!({
        "candidate_set_id": "cs_purge", "scan_id": "s_cat", "revision": 1, "rule_set_digest": cm_core::purge::rule_set_digest(7, 5), "protection_version": "v",
        "candidates": [
            catalog_candidate("c_old", &view, old, "allowed", &[], "purge.cargo-target", 2000),
            catalog_candidate("c_new", &view, new, "allowed", &[], "purge.cargo-target", 300),
            catalog_candidate("c_empty", &view, emptied, "allowed", &[], "purge.cargo-target", 0),
        ],
        "inapplicable": [],
    }))
    .unwrap();
    stores.index.save_candidates(&purge).unwrap();
    // Discovered at 10 bytes; a mounted image keeps changing, so a live check alone would only call it changed.
    let image = entry_named(&view, "root", "disk.iso");
    let installers: cm_core::analysis::CandidateSet = serde_json::from_value(serde_json::json!({
        "candidate_set_id": "cs_inst", "scan_id": "s_cat", "revision": 1, "rule_set_digest": cm_core::installer::rule_set_digest(2), "protection_version": "v",
        "candidates": [catalog_candidate("c_img", &view, image, "pending_evidence", &["mounted_image: eject it first (File Explorer > Eject)"], "installer.iso", 10)],
        "inapplicable": [],
    }))
    .unwrap();
    stores.index.save_candidates(&installers).unwrap();

    // The inner file of one folder grows after discovery: that folder is stale, the other one is planned.
    std::fs::write(root.join("rustold").join("target").join("debug").join("x.o"), vec![1u8; 3000]).unwrap();
    let created = catalog_plan(&stores, Catalog::Purge, "cs_purge", &["c_old".into(), "c_new".into()], cm_core::rules::Mode::Quarantine).unwrap();
    assert_eq!(created.stale_items.len(), 1, "{:?}", created.stale_items);
    let st = &created.stale_items[0];
    assert_eq!((st.candidate_id.as_str(), st.reason), ("c_old", "changed"));
    assert!(st.detail.contains("x.o") && st.detail.contains("3000 bytes now, 2000 when scanned"), "{}", st.detail);
    let plan = stores.audit.load_plan(&created.plan_id).unwrap();
    assert!(plan.body.items.iter().all(|i| i.candidate_id == "c_new"), "only the unchanged folder is frozen");
    let only_old = catalog_plan(&stores, Catalog::Purge, "cs_purge", &["c_old".into()], cm_core::rules::Mode::Quarantine).unwrap_err();
    assert_eq!((only_old.code, only_old.reason), (cm_core::error::ErrorCode::PlanStale, "selection_stale"));

    // WFA-58: refused as not plannable, naming the mount, before any live check.
    let mounted = catalog_plan(&stores, Catalog::Installer, "cs_inst", &["c_img".into()], cm_core::rules::Mode::Trash).unwrap_err();
    assert_eq!((mounted.code, mounted.reason), (cm_core::error::ErrorCode::PolicyDenied, "not_selectable"));
    assert!(mounted.message.contains("(mounted)") && mounted.message.contains("eject it first"), "{}", mounted.message);

    // WFA-113: a folder that holds no files gives no action in quarantine mode, so nothing is planned.
    let empty = catalog_plan(&stores, Catalog::Purge, "cs_purge", &["c_empty".into()], cm_core::rules::Mode::Quarantine).unwrap_err();
    assert_eq!((empty.code, empty.reason), (cm_core::error::ErrorCode::PolicyDenied, "empty_selection"));
    assert_eq!(crate::cleanup::plan_error(cm_core::plan::PlanError::UnknownSize("c".into())).reason, "plan_refused");
    drop(stores);
    let _ = std::fs::remove_dir_all(&base);
}

/// `mklink /J` (no administrator needed); removed with `rmdir` semantics (the link only, never its target).
#[cfg(windows)]
fn junction(link: &std::path::Path, target: &std::path::Path) -> bool {
    std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(link).arg(target).output().is_ok_and(|o| o.status.success())
}

/// NEW-08 / WFA-47/63: the desktop refuses a junction into System32 as the CLI does: protected, naming the location,
/// both when inspecting and when planning; a link to an ordinary folder stays the plain link refusal.
#[cfg(windows)]
#[test]
fn a_link_into_system32_is_refused_as_protected_and_named() {
    let base = temp_base("link-sys");
    let _ = std::fs::remove_dir_all(&base);
    let (root, store) = (base.join("root"), base.join("store"));
    std::fs::create_dir_all(root.join("links")).unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join("target").join("x.bin"), b"x").unwrap();
    let system32 = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32");
    let (sys, plain) = (root.join("links").join("link-sys"), root.join("links").join("link-plain"));
    if !junction(&sys, &system32) || !junction(&plain, &root.join("target")) {
        eprintln!("skipped: junctions cannot be created here");
        return;
    }
    let stores = Stores::open(&store).unwrap();
    let view = scan_into(&stores, &root, "s_links");
    let (sys_e, plain_e) = (entry_named(&view, "links", "link-sys"), entry_named(&view, "links", "link-plain"));
    let s = AppState::with_store(Some(store.clone()), None, None);
    let ins = crate::suite::imp::trash_inspect(&s, "s_links", sys_e).unwrap();
    let r = ins.refusal.as_ref().expect("refused");
    assert_eq!((ins.trashable, r.code.as_str(), r.reason.as_str()), (false, "policy_denied", "protected"), "{r:?}");
    assert!(r.message.contains("resolves into a protected system location"), "{}", r.message);
    let target = ins.link_target_display.as_deref().unwrap_or_default().to_lowercase();
    assert!(target.ends_with("system32"), "names where the link leads: {target}");
    let err = crate::suite::imp::trash_plan(&s, "s_links", &[sys_e]).unwrap_err();
    assert_eq!((err.code, err.reason), (cm_core::error::ErrorCode::PolicyDenied, "protected"));
    assert!(err.message.to_lowercase().contains("system32"), "{}", err.message);
    let ins = crate::suite::imp::trash_inspect(&s, "s_links", plain_e).unwrap();
    assert_eq!((ins.refusal.as_ref().map(|r| r.reason.as_str()), ins.link_target_display.as_deref()), (Some("link_or_reparse"), None));
    assert_eq!(crate::suite::imp::trash_plan(&s, "s_links", &[plain_e]).unwrap_err().reason, "link_or_reparse");
    drop(s);
    drop(stores);
    std::fs::remove_dir(&sys).unwrap();
    std::fs::remove_dir(&plain).unwrap();
    assert!(system32.join("kernel32.dll").exists() && root.join("target").join("x.bin").exists());
    let _ = std::fs::remove_dir_all(&base);
}

/// NEW-05 / WFA-35: on a volume whose Recycle Bin is off, the inspection already says the object cannot be moved
/// (`recycle_bin_unavailable`, which the page words in its language), and the plan is refused the same way. The
/// turned-off setting is the test-only BitBucket redirect (`CM_TEST_BITBUCKET_ROOT`, a key under
/// `HKCU\Software\CleanManagerTest\`), set for a child run of this test only.
#[cfg(windows)]
#[test]
fn a_volume_without_a_usable_bin_is_not_movable_at_inspection() {
    const CHILD: &str = "CM_G2_BIN_OFF_BASE";
    let reg = |args: &[&str]| std::process::Command::new("reg").args(args).output().is_ok_and(|o| o.status.success());
    let Some(base) = std::env::var_os(CHILD).map(std::path::PathBuf::from) else {
        let base = temp_base("bin-off");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("root")).unwrap();
        std::fs::write(base.join("root").join("x.bin"), vec![7u8; 100]).unwrap();
        let raw = cm_platform::paths::raw_path(base.as_os_str()).unwrap();
        let guid = cm_platform::windows::actions::recycle_bin_volume_key(&raw).expect("the scratch volume has a GUID name");
        let key = format!(r"Software\CleanManagerTest\desk{}_bin", std::process::id());
        assert!(reg(&["add", &format!(r"HKCU\{key}\Volume\{guid}"), "/v", "NukeOnDelete", "/t", "REG_DWORD", "/d", "1", "/f"]));
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "flow_tests::a_volume_without_a_usable_bin_is_not_movable_at_inspection", "--nocapture", "--test-threads=1"])
            .env(CHILD, &base)
            .env("CM_TEST_BITBUCKET_ROOT", &key)
            .output()
            .unwrap();
        let _ = reg(&["delete", &format!(r"HKCU\{key}"), "/f"]);
        let rest = std::process::Command::new("reg").args(["query", r"HKCU\Software\CleanManagerTest"]).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        if !rest.lines().any(|l| l.to_ascii_lowercase().contains(r"cleanmanagertest\")) {
            let _ = reg(&["delete", r"HKCU\Software\CleanManagerTest", "/f"]);
        }
        let _ = std::fs::remove_dir_all(&base);
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success() && text.contains("1 passed"), "child run: {text}{}", String::from_utf8_lossy(&out.stderr));
        return;
    };
    let store = base.join("store");
    let stores = Stores::open(&store).unwrap();
    let view = scan_into(&stores, &base.join("root"), "s_binoff");
    let e = entry_named(&view, "root", "x.bin");
    let s = AppState::with_store(Some(store.clone()), None, None);
    let ins = crate::suite::imp::trash_inspect(&s, "s_binoff", e).unwrap();
    let r = ins.refusal.as_ref().expect("not movable");
    assert_eq!((ins.trashable, ins.recycle_bin_ready, r.code.as_str(), r.reason.as_str()), (false, false, "unsupported", "recycle_bin_unavailable"), "{r:?}");
    assert!(r.message.contains("no permanent-delete alternative"), "{}", r.message);
    let err = crate::suite::imp::trash_plan(&s, "s_binoff", &[e]).unwrap_err();
    assert_eq!((err.code, err.reason), (cm_core::error::ErrorCode::Unsupported, "recycle_bin_unavailable"));
    assert!(base.join("root").join("x.bin").exists());
}

/// A folder laid out like this platform's bin (synthetic names; the real bin is never read or changed here).
fn synthetic_bin(dir: &std::path::Path) -> cm_app::trash::Bin {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    #[cfg(windows)]
    {
        let info = |path: &str, size: u64, ft: u64| {
            let units: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
            let mut d = Vec::new();
            d.extend_from_slice(&2u64.to_le_bytes());
            d.extend_from_slice(&size.to_le_bytes());
            d.extend_from_slice(&ft.to_le_bytes());
            d.extend_from_slice(&(units.len() as u32).to_le_bytes());
            d.extend(units.iter().flat_map(|u| u.to_le_bytes()));
            d
        };
        std::fs::write(dir.join("$RAAA111.txt"), vec![1u8; 10]).unwrap();
        std::fs::write(dir.join("$IAAA111.txt"), info("D:\\work\\old.txt", 10, 133_000_000_000_000_000)).unwrap();
        std::fs::write(dir.join("$RBBB222.txt"), vec![2u8; 20]).unwrap();
        std::fs::write(dir.join("$IBBB222.txt"), info("D:\\work\\new.txt", 20, 133_100_000_000_000_000)).unwrap();
        let l = cm_platform::windows::actions::recycle_bin::list_dir(dir).unwrap();
        cm_app::trash::Bin { dir: l.bin_dir, view: l.view, items: l.items, observed_at: l.observed_at }
    }
    #[cfg(target_os = "macos")]
    {
        std::fs::write(dir.join("old.txt"), vec![1u8; 10]).unwrap();
        std::fs::write(dir.join("new.txt"), vec![2u8; 20]).unwrap();
        let l = cm_platform::macos::actions::trash::list_trash_dir(dir).unwrap();
        cm_app::trash::Bin { dir: l.trash_dir, view: l.view, items: l.items, observed_at: l.observed_at }
    }
}

/// NEW-15 / WFA-65, MTR-03: the bin page lists this user's items (newest first, grouped by bin) and freezes chosen items
/// of one bin into a separate permanent-delete plan, which this window shows but never approves (a terminal asks for
/// DELETE first). Items of two bins, or items no longer listed, make no plan.
#[test]
fn the_bin_page_lists_items_and_freezes_a_terminal_reviewed_purge_plan() {
    let base = temp_base("bin-page");
    let _ = std::fs::remove_dir_all(&base);
    let store = base.join("store");
    let bins = [synthetic_bin(&base.join("bin-a")), synthetic_bin(&base.join("bin-b"))];
    let issues = [cm_app::trash::BinIssue { code: "bin_unreadable", scope_display: "E:\\$Recycle.Bin".into(), message: "denied".into() }];
    let dto = crate::suite::imp::bin_list_dto(&bins, &issues);
    assert_eq!((dto.total, dto.items.len(), dto.truncated, dto.bins.len()), (4, 4, false, 2));
    assert_ne!(dto.bins[0].bin_id, dto.bins[1].bin_id);
    assert!(dto.bins.iter().all(|b| b.item_count == 2));
    assert_eq!((dto.issues[0].code.as_str(), dto.issues.len()), ("bin_unreadable", 1));
    assert!(dto.items.iter().all(|i| i.purgeable && i.refusal.is_none() && i.object_kind == "file"));
    let names: Vec<&str> = dto.items.iter().map(|i| i.name.as_str()).collect();
    assert!(names.iter().all(|n| *n == "old.txt" || *n == "new.txt"), "{names:?}");
    if cfg!(windows) {
        assert_eq!(names[..2], ["new.txt", "new.txt"], "newest first by deletion time");
        assert_eq!(dto.items[0].original_path_display.as_deref(), Some("D:\\work\\new.txt"));
        assert!(dto.items[0].deleted_at.is_some() && dto.items[0].bin_location_display.contains("$RBBB222"));
    }
    let a: Vec<String> = dto.items.iter().filter(|i| i.bin_id == dto.bins[0].bin_id).map(|i| i.trash_item_id.clone()).collect();
    let other = dto.items.iter().find(|i| i.bin_id == dto.bins[1].bin_id).unwrap().trash_item_id.clone();

    let s = AppState::with_store(Some(store.clone()), None, None);
    s.with_stores(|st| {
        let mixed = crate::suite::imp::purge_from(st, &bins, &[a[0].clone(), other.clone()]).unwrap_err();
        assert_eq!((mixed.code, mixed.reason), (cm_core::error::ErrorCode::InvalidArgument, "bin_items_mixed"));
        assert_eq!(crate::suite::imp::purge_from(st, &bins, &["tb_gone".into()]).unwrap_err().reason, "bin_item_gone");
        let plan_id = crate::suite::imp::purge_from(st, &bins, &a)?.plan_id;
        let d = cleanup::detail(&s, st, &plan_id)?;
        assert_eq!((d.kind, d.plan_kind_v2.as_deref(), d.terminal_only.as_deref(), d.mode), ("manual_purge", Some("manual_selection"), Some("permanent_delete"), None));
        assert_eq!(d.authorization.status, "required");
        let items = cleanup::items(st, &plan_id, 50, None)?;
        assert!(items.items.iter().all(|i| i.action_type.as_deref() == Some("fs_delete")));
        // MTR-03: the irreversibility confirmation is asked in a terminal review only; this window never approves it.
        cm_app::features::set_experimental(&store, true).unwrap();
        let token = d.review_token.clone().unwrap_or_default();
        let refused = cleanup::approve(&s, st, &plan_id, &token, &d.digest_prefix).unwrap_err();
        assert_eq!(refused.reason, "needs_cli_confirmation");
        assert_eq!(cleanup::detail(&s, st, &plan_id)?.authorization.status, "required");
        Ok(())
    })
    .unwrap();
    assert!(base.join("bin-a").read_dir().unwrap().count() > 0, "planning deletes nothing");
    drop(s);
    let _ = std::fs::remove_dir_all(&base);
}
