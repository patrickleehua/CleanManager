//! WFA-16 / BUG-WFA-01 and WFA-11 / BUG-WFA-06 through the real binary.
//!
//! - Only the scan index (`index.db`) is damaged (its first 92 bytes overwritten, as in the acceptance run):
//!   history, quarantine and recovery keep working; every command that needs the index returns the typed
//!   `index_unavailable` with `cleanmanager doctor --repair-index` as the next step; `doctor` reports it;
//!   `doctor --repair-index` keeps the damaged file as `index.db.damaged-<time>`, creates an empty index, and
//!   new scans work again. On Windows the same holds for a real quarantined file, which is restored while
//!   the index is damaged.
//! - A scan whose host was killed after recording its own report is `interrupted` with `host_exited`.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-index-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn cli_env(store: &Path, local: Option<&Path>, args: &[&str]) -> (i32, Value) {
    let mut c = Command::new(env!("CARGO_BIN_EXE_cleanmanager"));
    c.args(args).arg("--store").arg(store).arg("--json").env("CM_STORE_DIR", store).env_remove("CLEANMANAGER_LANG");
    if let Some(local) = local {
        c.env("CM_RESOLVE_LOCALAPPDATA", local).env("CM_RESOLVE_TEMP", local.join("Temp"));
    }
    let out = c.output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "exactly one record for {args:?}: {text}");
    (out.status.code().unwrap(), serde_json::from_str(&text).unwrap())
}

fn cli(store: &Path, args: &[&str]) -> (i32, Value) {
    cli_env(store, None, args)
}

/// The acceptance repro: overwrite the first 92 bytes of `index.db` while no process has it open.
fn damage_index(store: &Path) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().write(true).open(store.join("index.db")).unwrap();
    f.write_all(&[0x42u8; 92]).unwrap();
}

fn assert_index_unavailable(code: i32, e: &Value) {
    assert_eq!((code, e["kind"].as_str(), e["code"].as_str()), (8, Some("error"), Some("index_unavailable")), "{e}");
    assert_eq!(e["required_action"], "cleanmanager doctor --repair-index", "{e}");
    assert_eq!(e["retryable"], false, "{e}");
}

fn check<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["checks"].as_array().unwrap().iter().find(|c| c["check"] == name).unwrap_or_else(|| panic!("no {name} check: {report}"))
}

fn tree(base: &Path) -> PathBuf {
    let root = base.join("tree");
    std::fs::create_dir_all(root.join("d")).unwrap();
    std::fs::write(root.join("d").join("a.bin"), b"abc").unwrap();
    root
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn a_damaged_index_keeps_history_usable_and_doctor_repairs_it_by_moving_it_aside() {
    let base = scratch("repair");
    let store = base.join("store");
    let root = tree(&base);
    let (code, scan) = cli(&store, &["scan", "--root", root.to_str().unwrap()]);
    assert_eq!(code, 0, "{scan}");
    let scan_id = scan["scan_id"].as_str().unwrap().to_owned();
    {
        // History written before the damage (as a cleanup would have left it).
        let stores = cm_store::Stores::open(&store).unwrap();
        let host = cm_core::host::HostRef { kind: "cli".into(), pid: 1, started_at: "2026-10-22T00:00:00Z".into() };
        stores.audit.begin_operation("op_before", "p_before", "d", "k", &host, "2026-10-22T00:00:00Z").unwrap();
        stores.audit.set_operation_state("op_before", "completed").unwrap();
    }
    damage_index(&store);
    let damaged = std::fs::read(store.join("index.db")).unwrap();

    // History, quarantine and recovery do not need the index.
    let (code, ops) = cli(&store, &["operations", "list"]);
    assert_eq!((code, ops["kind"].as_str()), (0, Some("operation_list")), "{ops}");
    assert!(ops["items"].as_array().unwrap().iter().any(|o| o["operation_id"] == "op_before"), "{ops}");
    for args in [&["recovery", "list"][..], &["quarantine", "list"]] {
        let (code, list) = cli(&store, args);
        assert_eq!((code, list["kind"].as_str()), (0, Some("quarantine_list")), "{list}");
    }
    // Everything that needs the index says so, with the next step; a new scan is refused the same way.
    let (code, e) = cli(&store, &["scan", "show", &scan_id]);
    assert_index_unavailable(code, &e);
    let (code, e) = cli(&store, &["entries", "list", "--scan", &scan_id]);
    assert_index_unavailable(code, &e);
    let (code, e) = cli(&store, &["scan", "--root", root.to_str().unwrap()]);
    assert_index_unavailable(code, &e);
    // No background host is started for a scan that cannot be recorded.
    let (code, e) = cli(&store, &["scan", "--root", root.to_str().unwrap(), "--detach"]);
    assert_index_unavailable(code, &e);
    assert_eq!(std::fs::read(store.join("index.db")).unwrap(), damaged, "no command rewrites the damaged file");

    // doctor reports the index apart from the audit store.
    let (code, report) = cli(&store, &["doctor", "--no-redact"]);
    assert_eq!(code, 0, "{report}");
    assert_eq!(check(&report, "store.open")["status"], "ok");
    let index = check(&report, "store.index");
    assert_eq!(index["status"], "failed", "{index}");
    assert!(index["detail"].as_str().unwrap().contains("doctor --repair-index"), "{index}");
    assert!(report.get("index_repair").is_none(), "no repair without the flag");

    // The repair keeps the damaged file under a new name and starts an empty index.
    let (code, report) = cli(&store, &["doctor", "--repair-index", "--no-redact"]);
    assert_eq!(code, 0, "{report}");
    let repair = &report["index_repair"];
    assert_eq!((repair["status"].as_str(), repair["previous_state"].as_str()), (Some("repaired"), Some("corrupt")), "{repair}");
    let kept: Vec<PathBuf> = repair["kept_files"].as_array().unwrap().iter().map(|k| PathBuf::from(k.as_str().unwrap())).collect();
    assert_eq!(kept.len(), 1, "{repair}");
    assert!(kept[0].file_name().unwrap().to_string_lossy().starts_with("index.db.damaged-"), "{repair}");
    assert_eq!(std::fs::read(&kept[0]).unwrap(), damaged, "kept byte for byte, never deleted");
    assert!(repair["message"].as_str().unwrap().contains("run scan again"), "{repair}");
    assert_eq!(check(&report, "store.index")["status"], "ok", "the report after the repair sees the new index");
    let log = std::fs::read_to_string(store.join("migrations.log")).unwrap();
    assert!(log.lines().any(|l| l.contains("\"rebuilt\"") && l.contains("index.db.damaged-")), "{log}");

    // Old scans are gone (re-scan), history is untouched, and scanning works again.
    let (code, e) = cli(&store, &["scan", "show", &scan_id]);
    assert_eq!((code, e["code"].as_str()), (2, Some("invalid_argument")), "{e}");
    let (code, ops) = cli(&store, &["operations", "list"]);
    assert!(code == 0 && ops["items"].as_array().unwrap().iter().any(|o| o["operation_id"] == "op_before"), "{ops}");
    let (code, scan) = cli(&store, &["scan", "--root", root.to_str().unwrap()]);
    assert_eq!((code, scan["status"].as_str()), (0, Some("completed")), "{scan}");
    // A healthy index is left alone.
    let (code, report) = cli(&store, &["doctor", "--repair-index"]);
    assert_eq!(code, 0, "{report}");
    assert_eq!((report["index_repair"]["status"].as_str(), report["index_repair"]["previous_state"].as_str()), (Some("not_needed"), Some("healthy")));
    let damaged_copies = std::fs::read_dir(&store).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with("index.db.damaged-")).count();
    assert_eq!(damaged_copies, 1, "nothing more was moved");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn repair_without_a_store_or_index_changes_nothing() {
    let base = scratch("repair-none");
    let store = base.join("not-yet");
    let (code, report) = cli(&store, &["doctor", "--repair-index"]);
    assert_eq!((code, report["index_repair"]["status"].as_str(), report["index_repair"]["previous_state"].as_str()), (0, Some("not_needed"), Some("missing")), "{report}");
    assert!(!store.exists(), "doctor does not create a store");
    let _ = std::fs::remove_dir_all(&base);
}

/// BUG-WFA-06: the host confirmed every row and recorded its own report (no issues), then died before publishing.
#[test]
fn a_host_killed_after_its_own_report_is_interrupted_with_host_exited() {
    let base = scratch("late-kill");
    let store = base.join("store");
    {
        let stores = cm_store::Stores::open(&store).unwrap();
        // This process, with another creation time: a host that has exited (a reused PID is never the original host).
        let host = cm_core::host::HostRef { kind: "cli".into(), pid: std::process::id(), started_at: "2000-01-01T00:00:00Z".into() };
        let root = cm_core::path::RawPath::unix(b"/late".to_vec()).unwrap();
        let rev = stores
            .index
            .begin_scan(&cm_store::index::ScanInit { scan_id: "s_late".into(), scope_id: "scope_late".into(), root: root.clone(), backend: "test".into(),
                host, started_at: "2026-10-22T00:00:00Z".into() })
            .unwrap();
        let object = cm_store::index::ObjectRow { object_id: 1, volume_id: "v".into(), file_id: "inode64:0000000000000001".into(), generation: None,
            quality: cm_core::model::IdentityQuality::Strong, kind: cm_core::model::ObjectKind::Directory, logical: None, allocated: None, link_count: None,
            cloud: cm_core::model::CloudState::NotCloud, modified_unix: None, shared_blocks: None };
        let entry = cm_store::index::EntryRow { entry_id: 1, parent_entry_id: None, name: root, object_id: 1, kind: cm_core::model::ObjectKind::Directory, boundary: None };
        stores.index.write_batch("s_late", rev, &[object], &[entry], &[]).unwrap();
        stores.index.record_report("s_late", &cm_core::scan::ScanReport { state: cm_core::model::ScanState::Completed, backend: "test".into(),
            backend_reason: "test backend".into(), entries: 1, skipped_ranges: 0, boundaries: 0, elevated_read: vec![], issues: vec![] }).unwrap();
    }
    let (code, result) = cli(&store, &["scan", "show", "s_late"]);
    assert_eq!((code, result["kind"].as_str(), result["status"].as_str()), (5, Some("scan_result"), Some("interrupted")), "{result}");
    let codes: Vec<&str> = result["issues"].as_array().unwrap().iter().map(|i| i["code"].as_str().unwrap()).collect();
    assert_eq!(codes, ["host_exited"], "{result}");
    assert_eq!(result["completeness"], "partial_scope");
    let _ = std::fs::remove_dir_all(&base);
}

/// Real NTFS: a file quarantined before the damage is listed and restored while the index is damaged.
#[cfg(windows)]
#[test]
fn a_quarantined_file_is_restored_while_the_index_is_damaged() {
    let base = scratch("restore");
    let (local, store) = (base.join("local"), base.join("store"));
    let temp = local.join("Temp");
    std::fs::create_dir_all(&temp).unwrap();
    let file = temp.join("keep.tmp");
    std::fs::write(&file, [7u8; 4096]).unwrap();
    let month_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400);
    std::fs::File::options().write(true).open(&file).unwrap().set_modified(month_ago).unwrap();
    let run = |args: &[&str]| cli_env(&store, Some(&local), args);

    let (_, scan) = run(&["scan", "--root", temp.to_str().unwrap()]);
    let (_, analysis) = run(&["analyze", "--scan", scan["scan_id"].as_str().unwrap()]);
    let set = analysis["candidate_set_id"].as_str().unwrap().to_owned();
    let (_, page) = run(&["candidates", "list", "--set", &set]);
    let cands: Vec<&str> = page["items"].as_array().unwrap().iter().map(|c| c["candidate_id"].as_str().unwrap()).collect();
    let (code, plan) = run(&["plan", "create", "--set", &set, "--mode", "quarantine", "--candidates", &cands.join(",")]);
    assert_eq!(code, 0, "{plan}");
    let stores = cm_store::Stores::open(&store).unwrap();
    stores.audit.save_receipt(&cm_core::authz::Receipt {
        receipt_id: format!("r_{}", plan["body"]["plan_id"].as_str().unwrap()),
        plan_id: plan["body"]["plan_id"].as_str().unwrap().into(),
        plan_digest: plan["plan_digest"].as_str().unwrap().into(),
        mode: cm_core::rules::Mode::Quarantine,
        principal: cm_core::authz::current_principal(),
        source: cm_core::authz::ReceiptSource::InteractiveReview,
        policy_id: None,
        policy_version: None,
        created_at: cm_core::time::now_utc(),
        expires_at: plan["body"]["expires_at"].as_str().unwrap().into(),
        revoked_at: None,
    }).unwrap();
    drop(stores);
    let (code, report) = run(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()]);
    assert_eq!(code, 0, "{report}");
    assert!(!file.exists());

    damage_index(&store);
    let (code, list) = run(&["recovery", "list"]);
    assert_eq!(code, 0, "{list}");
    let item = list["items"].as_array().unwrap().iter().find(|i| i["original_path"]["display"].as_str().unwrap().ends_with("keep.tmp")).expect("listed").clone();
    let qid = item["quarantine_id"].as_str().unwrap().to_owned();
    let (code, shown) = run(&["recovery", "show", &qid]);
    assert_eq!(code, 0, "{shown}");
    let (code, rplan) = run(&["recovery", "plan", "create", "--restore", &qid]);
    assert_eq!(code, 0, "{rplan}");
    let stores = cm_store::Stores::open(&store).expect("the audit store opens while the index is damaged");
    stores.audit.save_recovery_receipt(&cm_core::recovery::RecoveryReceipt {
        receipt_id: format!("rr_{}", rplan["body"]["plan_id"].as_str().unwrap()),
        plan_id: rplan["body"]["plan_id"].as_str().unwrap().into(),
        plan_digest: rplan["plan_digest"].as_str().unwrap().into(),
        recovery_kind: cm_core::recovery::RecoveryKind::Restore,
        principal: cm_core::authz::current_principal(),
        source: cm_core::authz::ReceiptSource::InteractiveReview,
        created_at: cm_core::time::now_utc(),
        expires_at: rplan["body"]["expires_at"].as_str().unwrap().into(),
        revoked_at: None,
    }).unwrap();
    drop(stores);
    let (code, restored) = run(&["plan", "apply", rplan["body"]["plan_id"].as_str().unwrap()]);
    assert_eq!(code, 0, "{restored}");
    assert_eq!(std::fs::read(&file).unwrap(), [7u8; 4096], "restored while the index is damaged");
    let (code, e) = run(&["scan", "show", scan["scan_id"].as_str().unwrap()]);
    assert_index_unavailable(code, &e);
    let _ = std::fs::remove_dir_all(&base);
}
