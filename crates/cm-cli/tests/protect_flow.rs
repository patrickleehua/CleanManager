//! `protect` commands (PRT-01..03, CLI-10): machine callers may add but never remove, entries
//! pointing at hard-deny locations are refused, and an unusable list stops every write with
//! `protection_unavailable` while `protect check` keeps answering.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// On macOS `$TMPDIR` is reached through the `/var` symlink, whose target `/private/var` is a hard-deny root (a link on
/// the way only adds denials), so the scratch directory is named by its real `/private/var/...` path there.
fn temp_base() -> PathBuf {
    let t = std::env::temp_dir();
    if cfg!(windows) { t } else { std::fs::canonicalize(&t).unwrap() }
}

fn scratch(name: &str) -> PathBuf {
    let p = temp_base().join(format!("cm-cli-protect-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn cli(args: &[&str], store: &Path) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_cleanmanager")).args(args).arg("--store").arg(store).arg("--json").output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "exactly one record: {text}");
    (out.status.code().unwrap(), serde_json::from_str(&text).unwrap())
}

fn user_ids(list: &Value) -> Vec<String> {
    list["items"].as_array().unwrap().iter().filter(|i| i["layer"] == "user").map(|i| i["protection_id"].as_str().unwrap().to_owned()).collect()
}

fn system_dir() -> &'static str {
    if cfg!(windows) { "C:\\Windows\\System32" } else { "/System/Library" }
}

#[test]
fn add_list_check_and_machine_remove_refused() {
    let base = scratch("flow");
    let store = base.join("store");
    let keep = base.join("work").join("keep-cache");
    std::fs::create_dir_all(&keep).unwrap();
    std::fs::write(keep.join("index"), b"x").unwrap();
    let keep_s = keep.to_str().unwrap();

    let (code, list) = cli(&["protect", "list"], &store);
    assert_eq!(code, 0);
    assert_eq!(list["kind"], "protection_list");
    let items = list["items"].as_array().unwrap();
    assert!(items.iter().any(|i| i["layer"] == "hard_deny") && items.iter().any(|i| i["layer"] == "default_category"));
    assert!(items.iter().all(|i| i["removable"] == (i["layer"] == "user")));
    assert!(user_ids(&list).is_empty());

    let (code, list) = cli(&["protect", "add", "--path", keep_s, "--note", "offline build cache"], &store);
    assert_eq!(code, 0, "{list}");
    let entry = list["items"].as_array().unwrap().iter().find(|i| i["layer"] == "user").unwrap().clone();
    assert_eq!((entry["protection_id"].as_str(), entry["match"].as_str(), entry["display"].as_str()), (Some("u_1"), Some("descendants"), Some(keep_s)));
    assert_eq!(entry["note"], "offline build cache");
    let (code, list) = cli(&["protect", "add", "--rule", "google.chrome.code-cache"], &store);
    assert_eq!(code, 0);
    assert_eq!(user_ids(&list), ["u_1", "u_2"]);

    let inside = keep.join("index");
    let (code, check) = cli(&["protect", "check", "--path", inside.to_str().unwrap()], &store);
    assert_eq!(code, 0);
    assert_eq!(check["kind"], "protection_check");
    assert_eq!((check["decision"].as_str(), check["layer"].as_str()), (Some("denied"), Some("user")));
    let layers = check["layers"].as_array().unwrap();
    assert_eq!(layers.iter().map(|l| l["layer"].as_str().unwrap()).collect::<Vec<_>>(), ["hard_deny", "default_category", "user"]);
    assert_eq!((layers[0]["result"].as_str(), layers[2]["protection_id"].as_str()), (Some("clear"), Some("u_1")));
    let (_, check) = cli(&["protect", "check", "--path", base.join("work").to_str().unwrap(), "--rule", "google.chrome.code-cache"], &store);
    assert_eq!(check["layer"], "user", "a protected rule is explained too: {check}");
    let (_, check) = cli(&["protect", "check", "--path", system_dir()], &store);
    assert_eq!((check["decision"].as_str(), check["layer"].as_str()), (Some("denied"), Some("hard_deny")));
    let ssh = base.join(".ssh").join("id_ed25519");
    let (_, check) = cli(&["protect", "check", "--path", ssh.to_str().unwrap()], &store);
    assert_eq!(check["layers"][1]["result"], "denied", "{check}");
    assert!(check["layers"][1]["detail"].as_str().unwrap().contains("credentials"));

    // Removing protection is a human act; a pipe is not an interactive terminal.
    let (code, err) = cli(&["protect", "remove", "u_1"], &store);
    assert_eq!((code, err["code"].as_str()), (2, Some("invalid_argument")));
    assert!(err["message"].as_str().unwrap().contains("interactive"));
    assert_eq!(user_ids(&cli(&["protect", "list"], &store).1), ["u_1", "u_2"]);
    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn invalid_and_hard_deny_entries_are_refused() {
    let base = scratch("invalid");
    let store = base.join("store");
    for args in [
        &["protect", "add", "--path", "relative\\dir"][..],
        &["protect", "add", "--path", system_dir()],
        &["protect", "add", "--glob", "*.log"],
        &["protect", "add", "--rule", "a", "--task", "b"],
        &["protect", "add", "--rule", "a", "--match", "exact"],
        &["protect", "add", "--path", "/tmp/x/../y", "--match", "sideways"],
    ] {
        let (code, err) = cli(args, &store);
        assert_eq!((code, err["code"].as_str()), (2, Some("invalid_argument")), "{args:?}: {err}");
    }
    assert!(!store.join("protection.json").exists(), "nothing was written");
    std::fs::remove_dir_all(&base).ok();
}

/// An entry that names a harmless-looking path which resolves into a system directory through
/// a junction is refused by identity (PRT-02/04).
#[cfg(windows)]
#[test]
fn an_entry_reaching_a_system_directory_through_a_junction_is_refused() {
    let base = scratch("junction");
    let store = base.join("store");
    let link = base.join("link");
    let ok = Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg("C:\\Windows\\System32").output().unwrap().status.success();
    assert!(ok, "junction created");
    let target = link.join("drivers");
    let (code, err) = cli(&["protect", "add", "--path", target.to_str().unwrap()], &store);
    assert_eq!((code, err["code"].as_str()), (2, Some("invalid_argument")), "{err}");
    assert!(err["message"].as_str().unwrap().contains("hard-deny"), "{err}");
    std::fs::remove_dir(&link).ok();
    std::fs::remove_dir_all(&base).ok();
}

/// Analysis honors the shared list: a protected cache is `blocked` with its entry named, never
/// dropped; an unusable list leaves analysis running but nothing plannable (PRT-01/02).
#[cfg(windows)]
#[test]
fn analysis_blocks_user_protected_candidates_and_survives_a_corrupt_list() {
    let base = scratch("analyze");
    let (local, store) = (base.join("local"), base.join("store"));
    let cache = local.join("Microsoft").join("Edge").join("User Data").join("Default").join("Code Cache");
    std::fs::create_dir_all(cache.join("js")).unwrap();
    std::fs::write(cache.join("js").join("index"), vec![1u8; 2048]).unwrap();
    let udd = local.join("Microsoft").join("Edge").join("User Data");
    let run = |args: &[&str]| -> (i32, Value) {
        let out = Command::new(env!("CARGO_BIN_EXE_cleanmanager"))
            .args(args)
            .arg("--store")
            .arg(&store)
            .arg("--json")
            .env("CM_RESOLVE_LOCALAPPDATA", &local)
            .env("CM_RESOLVE_TEMP", local.join("Temp"))
            .output()
            .unwrap();
        (out.status.code().unwrap(), serde_json::from_str(String::from_utf8(out.stdout).unwrap().trim()).unwrap())
    };
    let (code, scan) = run(&["scan", "--root", udd.to_str().unwrap()]);
    assert_eq!(code, 0, "{scan}");
    let scan_id = scan["scan_id"].as_str().unwrap().to_owned();
    assert_eq!(run(&["protect", "add", "--path", cache.to_str().unwrap()]).0, 0);
    let (code, analysis) = run(&["analyze", "--scan", &scan_id]);
    assert_eq!(code, 0, "{analysis}");
    assert_eq!((analysis["candidate_count"].as_u64(), analysis["blocked_count"].as_u64()), (Some(1), Some(1)), "{analysis}");
    let (_, page) = run(&["candidates", "list", "--set", analysis["candidate_set_id"].as_str().unwrap()]);
    let item = &page["items"][0];
    assert_eq!(item["risk"], "blocked");
    assert!(item["reason"].as_str().unwrap().contains("user_protected:u_1"), "{item}");
    assert_eq!(item["allowed_modes"], serde_json::json!([]));

    std::fs::write(store.join("protection.json"), b"{not json").unwrap();
    let (code, analysis) = run(&["analyze", "--scan", &scan_id]);
    assert_eq!(code, 0, "read-only analysis continues: {analysis}");
    assert_eq!(analysis["pending_evidence_count"], 1);
    assert!(analysis["issues"].as_array().unwrap().iter().any(|i| i["code"] == "protection_unavailable"), "{analysis}");
    let (code, err) = run(&["plan", "create", "--set", analysis["candidate_set_id"].as_str().unwrap(), "--mode", "quarantine", "--all-default-selected"]);
    assert_ne!(code, 0, "nothing is plannable without a usable list: {err}");
    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn an_unusable_list_stops_writes_and_check_still_answers() {
    let base = scratch("corrupt");
    let store = base.join("store");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(store.join("protection.json"), b"{not json").unwrap();
    let (code, err) = cli(&["protect", "add", "--rule", "google.chrome.code-cache"], &store);
    assert_eq!((code, err["code"].as_str()), (8, Some("protection_unavailable")), "{err}");
    assert!(err["required_action"].as_str().unwrap().contains("protection.json"));
    let (code, err) = cli(&["protect", "list"], &store);
    assert_eq!((code, err["code"].as_str()), (8, Some("protection_unavailable")));
    let (code, check) = cli(&["protect", "check", "--path", base.to_str().unwrap()], &store);
    assert_eq!(code, 0);
    assert_eq!(check["layers"][2]["result"], "unavailable");
    assert_eq!(check["decision"], "denied", "fail closed: {check}");
    assert_eq!(std::fs::read(store.join("protection.json")).unwrap(), b"{not json", "left for repair");
    std::fs::remove_dir_all(&base).ok();
}
