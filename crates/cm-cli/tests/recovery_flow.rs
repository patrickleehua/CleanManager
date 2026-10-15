//! End-to-end recovery on real NTFS (MVP 10.4 Windows half / 12.2): scan → analyze → plan
//! (quarantine) → receipt → apply → quarantine list → recovery plan (restore) → receipt → apply
//! → the file is back with the same identity; plus the refusal paths (occupied original name,
//! changed quarantined object, cleanup/old approvals) and a separately authorized purge.
//! Receipts are written through `cm_store` as the trusted terminal review would (it needs a
//! TTY). `CM_STORE_DIR` keeps the quarantine inside the scratch directory.
#![cfg(windows)]

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-recovery-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn cli(args: &[&str], store: &Path, local: &Path) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_cleanmanager"))
        .args(args)
        .arg("--store")
        .arg(store)
        .arg("--json")
        .env("CM_STORE_DIR", store)
        .env("CM_RESOLVE_LOCALAPPDATA", local)
        .env("CM_RESOLVE_TEMP", local.join("Temp"))
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "exactly one record: {text}");
    (out.status.code().unwrap(), serde_json::from_str(&text).unwrap())
}

fn file_id(p: &Path) -> String {
    let w: Vec<u16> = p.as_os_str().encode_wide().collect();
    cm_platform::windows::directory::identity_of(&w).unwrap().1
}

fn stale(p: &Path, body: &[u8]) {
    std::fs::write(p, body).unwrap();
    let month_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400);
    std::fs::File::options().write(true).open(p).unwrap().set_modified(month_ago).unwrap();
}

fn cleanup_receipt(store: &Path, plan: &Value) {
    let stores = cm_store::Stores::open(store).unwrap();
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
}

fn recovery_receipt(store: &Path, plan: &Value) {
    let kind = match plan["body"]["recovery_kind"].as_str().unwrap() {
        "restore" => cm_core::recovery::RecoveryKind::Restore,
        _ => cm_core::recovery::RecoveryKind::Purge,
    };
    let stores = cm_store::Stores::open(store).unwrap();
    stores.audit.save_recovery_receipt(&cm_core::recovery::RecoveryReceipt {
        receipt_id: format!("rr_{}", plan["body"]["plan_id"].as_str().unwrap()),
        plan_id: plan["body"]["plan_id"].as_str().unwrap().into(),
        plan_digest: plan["plan_digest"].as_str().unwrap().into(),
        recovery_kind: kind,
        principal: cm_core::authz::current_principal(),
        source: cm_core::authz::ReceiptSource::InteractiveReview,
        created_at: cm_core::time::now_utc(),
        expires_at: plan["body"]["expires_at"].as_str().unwrap().into(),
        revoked_at: None,
    }).unwrap();
}

fn delta_is_measured(report: &Value) {
    let n = |k: &str| report[k].as_str().map(|s| s.parse::<i128>().unwrap());
    if let (Some(b), Some(a)) = (n("volume_free_before_bytes"), n("volume_free_after_bytes")) {
        assert_eq!(n("observed_free_delta_bytes"), Some(a - b), "signed delta reported as observed, never clamped: {report}");
    }
}

fn by_original<'a>(list: &'a Value, name: &str) -> &'a Value {
    list["items"].as_array().unwrap().iter().find(|i| i["original_path"]["display"].as_str().unwrap().ends_with(name)).unwrap()
}

#[test]
fn quarantine_restore_and_purge_round_trip() {
    let base = scratch("flow");
    let (local, store) = (base.join("local"), base.join("store"));
    let temp = local.join("Temp");
    std::fs::create_dir_all(&temp).unwrap();
    for (name, body) in [("keep.tmp", &[1u8; 4096][..]), ("clash.tmp", &[2u8; 2048][..]), ("swap.tmp", &[3u8; 1024][..])] {
        stale(&temp.join(name), body);
    }
    let original_id = file_id(&temp.join("keep.tmp"));

    // Cleanup: quarantine the three stale files.
    let (_, scan) = cli(&["scan", "--root", temp.to_str().unwrap()], &store, &local);
    let (_, analysis) = cli(&["analyze", "--scan", scan["scan_id"].as_str().unwrap()], &store, &local);
    let set = analysis["candidate_set_id"].as_str().unwrap().to_owned();
    let (_, page) = cli(&["candidates", "list", "--set", &set], &store, &local);
    let cands: Vec<&str> = page["items"].as_array().unwrap().iter().map(|c| c["candidate_id"].as_str().unwrap()).collect();
    let (code, plan) = cli(&["plan", "create", "--set", &set, "--mode", "quarantine", "--candidates", &cands.join(",")], &store, &local);
    assert_eq!(code, 0, "{plan}");
    assert_eq!(plan["body"]["items"].as_array().unwrap().len(), 3, "{plan}");
    cleanup_receipt(&store, &plan);
    let (code, report) = cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()], &store, &local);
    assert_eq!(code, 0, "{report}");
    // REC-05: processed = quarantined, nothing claimed as freed by a same-volume move.
    assert_eq!(report["processed_logical_bytes"], "7168");
    assert_eq!(report["quarantined_logical_bytes"], "7168");
    assert_eq!(report["estimated_freed_bytes"], "0");
    delta_is_measured(&report);
    assert!(!temp.join("keep.tmp").exists());
    let cleanup_op = report["operation_id"].as_str().unwrap().to_owned();

    // History and quarantine listing after a "restart" (new processes, durable manifests only).
    let (code, ops) = cli(&["operations", "list"], &store, &local);
    assert_eq!(code, 0, "{ops}");
    assert_eq!(ops["items"][0]["operation_id"], cleanup_op.as_str());
    assert_eq!((ops["items"][0]["action"].as_str(), ops["items"][0]["recovery"]["availability"].as_str()), (Some("quarantine"), Some("in_app_restore")));
    let (code, list) = cli(&["quarantine", "list"], &store, &local);
    assert_eq!(code, 0, "{list}");
    assert_eq!(list["items"].as_array().unwrap().len(), 3);
    assert_eq!(list["total_logical_bytes"], "7168");
    let keep = by_original(&list, "keep.tmp");
    assert_eq!((keep["state"].as_str(), keep["restore_capability"].as_str(), keep["reminder_due"].as_bool()), (Some("recoverable"), Some("available"), Some(false)));
    assert_eq!(keep["operation_id"], cleanup_op.as_str());
    assert!(keep["metadata_restored"].as_array().unwrap().iter().any(|m| m == "acl"));
    let qid = |name: &str| by_original(&list, name)["quarantine_id"].as_str().unwrap().to_owned();
    let (keep_id, clash_id, swap_id) = (qid("keep.tmp"), qid("clash.tmp"), qid("swap.tmp"));
    let keep_loc = PathBuf::from(keep["quarantined_path"]["display"].as_str().unwrap());
    assert!(keep_loc.starts_with(&store), "quarantine stays inside the store: {}", keep_loc.display());
    assert_eq!(file_id(&keep_loc), original_id, "quarantine keeps the identity");

    // Refusal setups: a new file at clash.tmp's original name; swap.tmp's quarantined copy replaced.
    std::fs::write(temp.join("clash.tmp"), b"newer").unwrap();
    let swap_loc = PathBuf::from(by_original(&list, "swap.tmp")["quarantined_path"]["display"].as_str().unwrap());
    std::fs::rename(&swap_loc, base.join("swap-moved")).unwrap();
    std::fs::write(&swap_loc, b"planted").unwrap();
    let (_, list) = cli(&["recovery", "list"], &store, &local);
    assert_eq!(by_original(&list, "clash.tmp")["state"], "target_conflict");
    assert_eq!(by_original(&list, "swap.tmp")["state"], "identity_changed");

    // Recovery plan: independent type, digest and authorization.
    let ids = [keep_id.as_str(), clash_id.as_str(), swap_id.as_str()].join(",");
    let (code, rplan) = cli(&["recovery", "plan", "create", "--restore", &ids], &store, &local);
    assert_eq!(code, 0, "{rplan}");
    assert_eq!((rplan["body"]["engine_contract"].as_str(), rplan["authorization_status"].as_str()), (Some("recovery/v1"), Some("required")));
    let rp = rplan["body"]["plan_id"].as_str().unwrap().to_owned();
    let (code, err) = cli(&["plan", "apply", &rp], &store, &local);
    assert_eq!((code, err["code"].as_str()), (3, Some("needs_approval")), "{err}");
    cleanup_receipt(&store, &rplan); // a cleanup-type approval never authorizes a restore
    let (code, _) = cli(&["plan", "apply", &rp], &store, &local);
    assert_eq!(code, 3);
    let (code, _) = cli(&["plan", "review", &rp], &store, &local);
    assert_eq!(code, 2, "machine callers cannot approve recovery plans");
    recovery_receipt(&store, &rplan);
    let (_, shown) = cli(&["plan", "show", &rp], &store, &local);
    assert_eq!((shown["authorization_status"].as_str(), shown["body"]["recovery_kind"].as_str()), (Some("authorized"), Some("restore")));

    let (code, restored) = cli(&["plan", "apply", &rp], &store, &local);
    assert_eq!(code, 5, "partial: {restored}");
    assert_eq!((restored["succeeded"].as_u64(), restored["skipped"].as_u64()), (Some(1), Some(2)), "{restored}");
    let codes: Vec<&str> = restored["items"].as_array().unwrap().iter().map(|i| i["code"].as_str().unwrap()).collect();
    assert!(codes.contains(&"restored") && codes.contains(&"destination_exists") && codes.contains(&"identity_changed"), "{codes:?}");
    assert_eq!((restored["quarantined_logical_bytes"].as_str(), restored["estimated_freed_bytes"].as_str()), (Some("0"), Some("0")));
    delta_is_measured(&restored);
    assert_eq!(file_id(&temp.join("keep.tmp")), original_id, "the same object is back");
    assert_eq!(std::fs::read(temp.join("keep.tmp")).unwrap(), vec![1u8; 4096]);
    assert_eq!(std::fs::read(temp.join("clash.tmp")).unwrap(), b"newer", "never overwritten");
    assert_eq!(std::fs::read(&swap_loc).unwrap(), b"planted");
    assert!(!temp.join("swap.tmp").exists());
    let (code, again) = cli(&["plan", "apply", &rp], &store, &local);
    assert_eq!((code, &again["operation_id"]), (5, &restored["operation_id"]), "replayed, not repeated");

    // The conflicting item stays restorable; purging it needs its own plan and approval.
    let (_, list) = cli(&["recovery", "list"], &store, &local);
    assert_eq!(list["items"].as_array().unwrap().len(), 2);
    let (code, pplan) = cli(&["recovery", "plan", "create", "--purge", &clash_id], &store, &local);
    assert_eq!(code, 0, "{pplan}");
    assert_eq!(pplan["body"]["items"][0]["target"]["kind"], "permanent_delete");
    let pp = pplan["body"]["plan_id"].as_str().unwrap().to_owned();
    let (code, _) = cli(&["plan", "apply", &pp], &store, &local);
    assert_eq!(code, 3, "the restore approval does not cover the purge");
    recovery_receipt(&store, &pplan);
    let clash_loc = PathBuf::from(by_original(&list, "clash.tmp")["quarantined_path"]["display"].as_str().unwrap());
    let (code, purged) = cli(&["plan", "apply", &pp], &store, &local);
    assert_eq!(code, 0, "{purged}");
    assert_eq!((purged["estimated_freed_bytes"].as_str(), purged["items"][0]["recovery"].as_str()), (Some("2048"), Some("unavailable")));
    assert!(!clash_loc.exists());
    assert_eq!(std::fs::read(temp.join("clash.tmp")).unwrap(), b"newer");
    let (code, err) = cli(&["recovery", "plan", "create", "--restore", &clash_id], &store, &local);
    assert_eq!((code, err["code"].as_str()), (6, Some("conflict")), "a purged item cannot be planned again");

    let (_, ops) = cli(&["operations", "list", "--limit", "2"], &store, &local);
    assert_eq!((ops["items"][0]["action"].as_str(), ops["items"][0]["recovery"]["availability"].as_str()), (Some("purge"), Some("unavailable")));
    assert_eq!((ops["items"][1]["action"].as_str(), ops["items"][1]["recovery"]["availability"].as_str()), (Some("restore"), Some("not_applicable")));
    let cursor = ops["next_cursor"].as_str().unwrap().to_owned();
    let (_, older) = cli(&["operations", "list", "--limit", "2", "--cursor", &cursor], &store, &local);
    assert_eq!(older["items"][0]["operation_id"], cleanup_op.as_str());
    assert_eq!(older["next_cursor"], Value::Null);

    // REC-01: the quarantine is never proposed as a cleanup candidate: re-analysis of the
    // cleaned root sees only the restored file, not the quarantined objects.
    let (_, scan) = cli(&["scan", "--root", temp.to_str().unwrap()], &store, &local);
    let (_, analysis) = cli(&["analyze", "--scan", scan["scan_id"].as_str().unwrap()], &store, &local);
    let (_, page) = cli(&["candidates", "list", "--set", analysis["candidate_set_id"].as_str().unwrap()], &store, &local);
    for c in page["items"].as_array().unwrap() {
        assert!(!PathBuf::from(c["path"]["display"].as_str().unwrap()).starts_with(&store), "{c}");
    }
    std::fs::remove_dir_all(&base).ok();
}
