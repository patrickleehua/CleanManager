//! Privacy-prompt reporting through the binary (docs/platforms/macos.md §6). Real TCC prompts cannot be produced on a
//! hosted runner, so the debug-build hook `CM_TEST_PRIVACY_HOLD` stands in for one: a gated first access at or below
//! the held path never returns. With `CM_PRIVACY_PROMPT_WAIT_SECS=1`:
//! - a scan of a home whose Downloads is held is `partial` (exit 5): Downloads is a skipped range with a
//!   `permission_required` issue carrying the structured `permission_cause: privacy_prompt_pending`, the rest is read;
//! - a held scan root that is no known prompt location (a slow volume answers the same way) fails with
//!   `permission_cause: undetermined` and an honest "did not answer within 1 s";
//! - `insights overview` reports the held Downloads folder with no size, no file count (null, never 0) and the cause.
#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::Value;

fn scratch(name: &str) -> PathBuf {
    let t = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let p = t.join(format!("cm-cli-macprivacy-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn write(p: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// Run the binary with a scratch store, `home` as `$HOME` and the resolver's home, and `held` held.
fn cli(args: &[&str], root: &Path, home: &Path, held: &Path) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_cleanmanager"))
        .args(args)
        .arg("--json")
        .env("CM_STORE_DIR", root.join("store"))
        .env_remove("CLEANMANAGER_LANG")
        .env_remove("CM_RESOLVE_DOWNLOADS")
        .env("HOME", home)
        .env("CM_RESOLVE_HOME", home)
        .env("CM_TEST_PRIVACY_HOLD", held)
        .env("CM_PRIVACY_PROMPT_WAIT_SECS", "1")
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "exactly one record: {text}\n{}", String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap(), serde_json::from_str(&text).unwrap())
}

fn issues(v: &Value) -> Vec<Value> {
    v["issues"].as_array().cloned().unwrap_or_default()
}

#[test]
fn a_held_downloads_folder_is_a_pending_skipped_range_with_a_structured_cause() {
    let root = scratch("scan");
    let home = root.join("home");
    write(&home.join("Downloads/held.dmg"), &[0u8; 64]);
    write(&home.join("Projects/readme.txt"), b"read as usual");
    let t = Instant::now();
    let (code, scan) = cli(&["scan", "--root", home.to_str().unwrap()], &root, &home, &home.join("Downloads"));
    assert!(t.elapsed() < Duration::from_secs(30), "the scan is not held by the prompt: {:?}", t.elapsed());
    assert_eq!((code, scan["kind"].as_str(), scan["status"].as_str()), (5, Some("scan_result"), Some("partial")), "{scan}");
    assert_eq!(scan["skipped_ranges"], 1, "{scan}");
    let downloads = home.join("Downloads").display().to_string();
    let held: Vec<Value> = issues(&scan).into_iter().filter(|i| i["scope_display"] == downloads.as_str()).collect();
    assert_eq!(held.len(), 1, "{scan}");
    assert_eq!((held[0]["code"].as_str(), held[0]["permission_cause"].as_str()), (Some("permission_required"), Some("privacy_prompt_pending")), "{scan}");
    let message = held[0]["message"].as_str().unwrap();
    assert!(message.contains("permission_cause=privacy_prompt_pending") && message.contains("within 1 s"), "the text keeps it too: {message}");
    assert!(issues(&scan).iter().all(|i| i["permission_cause"].is_null() || i["code"] == "permission_required"), "{scan}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_held_root_that_is_no_known_prompt_location_is_undetermined() {
    let root = scratch("slow");
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let share = root.join("share");
    write(&share.join("big.iso"), &[0u8; 64]);
    let (code, scan) = cli(&["scan", "--root", share.to_str().unwrap()], &root, &home, &share);
    assert_eq!((code, scan["status"].as_str()), (4, Some("failed")), "permission_required, never an empty scan: {scan}");
    assert_eq!(scan["entries_seen"], 0, "{scan}");
    let all = issues(&scan);
    assert_eq!(all.len(), 1, "{scan}");
    assert_eq!((all[0]["code"].as_str(), all[0]["permission_cause"].as_str()), (Some("permission_required"), Some("undetermined")), "{scan}");
    let message = all[0]["message"].as_str().unwrap();
    assert!(message.contains("did not answer within 1 s") && message.contains("slow or unresponsive volume"), "{message}");
    assert!(!message.contains("privacy_prompt_pending"), "{message}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn insights_report_a_held_downloads_folder_as_unknown_with_its_cause() {
    let root = scratch("insights");
    let home = root.join("home");
    write(&home.join("Downloads/old.dmg"), &[0u8; 64]);
    let held = home.join("Downloads");
    let (code, _) = cli(&["experimental", "--state", "on"], &root, &home, &held);
    assert_eq!(code, 0);
    let (code, o) = cli(&["insights", "overview"], &root, &home, &held);
    assert_eq!((code, o["kind"].as_str()), (0, Some("insight_overview")), "{o}");
    let dl = &o["old_downloads"];
    assert!(dl["item_count"].is_null() && dl["total_logical_bytes"].is_null(), "unknown, never 0: {o}");
    assert_eq!(dl["permission_cause"], "privacy_prompt_pending", "{o}");
    let card = o["cards"].as_array().unwrap().iter().find(|c| c["kind"] == "old_downloads").unwrap_or_else(|| panic!("{o}"));
    assert!(card["count"].is_null() && card["logical_bytes"].is_null(), "{card}");
    assert_eq!(card["permission_cause"], "privacy_prompt_pending", "{card}");

    let (code, r) = cli(&["insights", "downloads"], &root, &home, &held);
    assert_eq!((code, r["kind"].as_str()), (0, Some("old_downloads_review")), "{r}");
    assert!(r["item_count"].is_null() && r["total_logical_bytes"].is_null(), "{r}");
    assert_eq!(r["permission_cause"], "privacy_prompt_pending", "{r}");
    let _ = std::fs::remove_dir_all(&root);
}
