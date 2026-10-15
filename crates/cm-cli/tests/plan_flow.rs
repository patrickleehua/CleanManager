//! End-to-end CLI flow on Windows (MVP 8.6 / 12.2): scan → analyze → plan → authorization gate
//! → apply → repeat → operations show. Uses a throwaway Edge-like profile fixture and the real
//! enabled rule pack; nothing outside the scratch directory is touched.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-flow-{}-{name}", std::process::id()));
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
        .env("CM_RESOLVE_LOCALAPPDATA", local)
        .env("CM_RESOLVE_TEMP", local.join("Temp"))
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "exactly one record: {text}");
    (out.status.code().unwrap(), serde_json::from_str(&text).unwrap())
}

#[test]
fn plan_requires_approval_and_machines_cannot_approve() {
    let base = scratch("flow");
    let (local, store) = (base.join("local"), base.join("store"));
    let cache = local.join("Microsoft").join("Edge").join("User Data").join("Default").join("Code Cache").join("js");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("index"), vec![1u8; 2048]).unwrap();
    std::fs::write(cache.join("f_0001"), vec![2u8; 4096]).unwrap();
    let udd = local.join("Microsoft").join("Edge").join("User Data");

    let (code, scan) = cli(&["scan", "--root", udd.to_str().unwrap()], &store, &local);
    assert_eq!(code, 0, "{scan}");
    let (_, analysis) = cli(&["analyze", "--scan", scan["scan_id"].as_str().unwrap()], &store, &local);
    assert_eq!(analysis["candidate_count"], 1, "{analysis}");
    let set = analysis["candidate_set_id"].as_str().unwrap().to_owned();
    let (code, plan) = cli(&["plan", "create", "--set", &set, "--mode", "quarantine", "--all-default-selected"], &store, &local);
    assert_eq!(code, 0, "{plan}");
    assert_eq!(plan["authorization_status"], "required");
    assert_eq!(plan["body"]["items"].as_array().unwrap().len(), 2, "two observed files, directories untouched");
    let plan_id = plan["body"]["plan_id"].as_str().unwrap().to_owned();

    // CLI-06/PLN-04: no authorization → exit 3 immediately with a reviewable plan id; nothing moved.
    let (code, err) = cli(&["plan", "apply", &plan_id], &store, &local);
    assert_eq!((code, err["code"].as_str()), (3, Some("needs_approval")));
    assert_eq!(err["details"]["plan_id"], plan_id.as_str());
    assert!(cache.join("f_0001").exists());

    // CLI-07: machine callers cannot approve plans or enable policies.
    let (code, err) = cli(&["plan", "review", &plan_id], &store, &local);
    assert_eq!((code, err["code"].as_str()), (2, Some("invalid_argument")));
    let scope = plan["body"]["scope_id"].as_str().unwrap().to_owned();
    let (code, pols) = cli(&["policy", "create", "--scope", &scope, "--rules", "microsoft.edge.code-cache@1.0.0", "--mode", "quarantine"], &store, &local);
    assert_eq!(code, 0);
    let pol = pols["items"][0]["policy"]["policy_id"].as_str().unwrap().to_owned();
    assert_eq!(pols["items"][0]["policy"]["state"], "paused");
    let (code, _) = cli(&["policy", "enable", &pol], &store, &local);
    assert_eq!(code, 2);
    let (code, _) = cli(&["plan", "apply", &plan_id], &store, &local);
    assert_eq!(code, 3, "a paused policy authorizes nothing");
    std::fs::remove_dir_all(&base).ok();
}

/// The reviewed path end to end on real NTFS, using the user-temp rule (review-only, no owner
/// process): a receipt equivalent to what the trusted terminal review writes (the review itself
/// needs a TTY) lets `plan apply` quarantine exactly the selected stale file; the fresh file is
/// never a candidate and applying again replays the recorded result.
#[test]
fn approved_plan_quarantines_planned_files_once() {
    let base = scratch("approved");
    let (local, store) = (base.join("local"), base.join("store"));
    let temp = local.join("Temp");
    std::fs::create_dir_all(&temp).unwrap();
    let stale = temp.join("old.tmp");
    std::fs::write(&stale, vec![3u8; 8192]).unwrap();
    let month_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400);
    std::fs::File::options().write(true).open(&stale).unwrap().set_modified(month_ago).unwrap();
    std::fs::write(temp.join("fresh.tmp"), b"new").unwrap();

    let (_, scan) = cli(&["scan", "--root", temp.to_str().unwrap()], &store, &local);
    let (_, analysis) = cli(&["analyze", "--scan", scan["scan_id"].as_str().unwrap()], &store, &local);
    assert_eq!(analysis["candidate_count"], 1, "{analysis}");
    let set = analysis["candidate_set_id"].as_str().unwrap().to_owned();
    let (_, page) = cli(&["candidates", "list", "--set", &set], &store, &local);
    assert_eq!(page["items"][0]["risk"], "medium", "review-only rules are never default-selected");
    let cand = page["items"][0]["candidate_id"].as_str().unwrap().to_owned();
    let (code, plan) = cli(&["plan", "create", "--set", &set, "--mode", "quarantine", "--candidates", &cand], &store, &local);
    assert_eq!(code, 0, "{plan}");
    let plan_id = plan["body"]["plan_id"].as_str().unwrap().to_owned();
    let digest = plan["plan_digest"].as_str().unwrap().to_owned();
    {
        let stores = cm_store::Stores::open(&store).unwrap();
        stores.audit.save_receipt(&cm_core::authz::Receipt {
            receipt_id: "r_test".into(),
            plan_id: plan_id.clone(),
            plan_digest: digest,
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
    let (_, shown) = cli(&["plan", "show", &plan_id], &store, &local);
    assert_eq!(shown["authorization_status"], "authorized", "{shown}");

    let (code, report) = cli(&["plan", "apply", &plan_id], &store, &local);
    assert_eq!(code, 0, "{report}");
    assert_eq!((report["status"].as_str(), report["succeeded"].as_u64()), (Some("completed"), Some(1)));
    assert_eq!(report["items"][0]["recovery"], "quarantine", "{report}");
    assert!(!stale.exists() && temp.join("fresh.tmp").exists());

    // Re-applying replays the recorded operation instead of acting again.
    let (code, again) = cli(&["plan", "apply", &plan_id], &store, &local);
    assert_eq!((code, &again["operation_id"]), (0, &report["operation_id"]), "{again}");
    let (code, shown) = cli(&["operations", "show", report["operation_id"].as_str().unwrap()], &store, &local);
    assert_eq!((code, shown["succeeded"].as_u64()), (0, Some(1)), "{shown}");
    std::fs::remove_dir_all(&base).ok();
}
