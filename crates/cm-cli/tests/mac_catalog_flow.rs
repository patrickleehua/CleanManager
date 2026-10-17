//! The cleanup catalog on macOS end to end through the binary: rule roots resolve inside a fixture tree through
//! `CM_RESOLVE_HOME` / `CM_RESOLVE_TEMP` / `CM_RESOLVE_DOWNLOADS` / `CM_RESOLVE_SYSTEM` (each reported as
//! `resolver_override`), a privacy-protected location (Safari, other apps' containers) is reported as
//! `permission_required` instead of read, the experimental macOS pack (waves 1 and 2) preselects nothing, and
//! `--volume` refuses anything but a mounted, local, non-startup volume.
#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)
}

/// Scratch tree under the canonical temporary directory (`/private/var/...`, not the `/var` link).
fn scratch(name: &str) -> PathBuf {
    let t = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let p = t.join(format!("cm-cli-maccat-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn cli(args: &[&str], root: &Path) -> (i32, Value) {
    let fx = root.join("fixture");
    let out = Command::new(env!("CARGO_BIN_EXE_cleanmanager"))
        .args(args)
        .arg("--json")
        .env("CM_STORE_DIR", root.join("store"))
        .env_remove("CM_RULES_DIR")
        .env_remove("CLEANMANAGER_LANG")
        .env("CM_RESOLVE_HOME", fx.join("home"))
        .env("CM_RESOLVE_TEMP", fx.join("temp"))
        .env("CM_RESOLVE_DOWNLOADS", fx.join("downloads"))
        .env("CM_RESOLVE_SYSTEM", fx.join("system"))
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "exactly one record: {text}");
    (out.status.code().unwrap(), serde_json::from_str(&text).unwrap())
}

fn write(p: &Path) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b"cached bytes").unwrap();
}

/// Fixture home with one Chrome code cache (and a non-profile near miss) and a Safari cache; scanned once.
fn scanned(root: &Path) -> String {
    let home = root.join("fixture/home");
    write(&home.join("Library/Caches/Google/Chrome/Default/Code Cache/js/index"));
    write(&home.join("Library/Caches/Google/Chrome/Guest Profile/Code Cache/js/index"));
    write(&home.join("Library/Caches/com.apple.Safari/WebKitCache/Version 16/Records/x"));
    for d in ["temp", "downloads", "system"] {
        std::fs::create_dir_all(root.join("fixture").join(d)).unwrap();
    }
    let (code, scan) = cli(&["scan", "--root", root.join("fixture").to_str().unwrap()], root);
    assert_eq!(code, 0, "{scan}");
    scan["scan_id"].as_str().unwrap().to_owned()
}

fn issue_codes(analysis: &Value) -> Vec<(String, String)> {
    analysis["issues"].as_array().unwrap().iter().map(|i| (i["code"].as_str().unwrap().to_owned(), i["scope_display"].as_str().unwrap().to_owned())).collect()
}

#[test]
fn fixture_roots_resolve_through_the_overrides_and_tcc_locations_are_reported() {
    let root = scratch("preview");
    let sid = scanned(&root);
    let rule = repo("rules/macos/google.chrome.code-cache.json");
    let (code, a) = cli(&["analyze", "--scan", &sid, "--preview-rule", rule.to_str().unwrap()], &root);
    assert_eq!(code, 0, "{a}");
    let codes = issue_codes(&a);
    for var in ["CM_RESOLVE_HOME", "CM_RESOLVE_TEMP", "CM_RESOLVE_DOWNLOADS", "CM_RESOLVE_SYSTEM"] {
        assert!(codes.contains(&("resolver_override".into(), var.into())), "{var}: {codes:?}");
    }
    assert_eq!(a["candidate_count"], 1, "{a}");
    let set = a["candidate_set_id"].as_str().unwrap();
    let (code, page) = cli(&["candidates", "list", "--set", set], &root);
    assert_eq!(code, 0, "{page}");
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let path = items[0]["path"]["display"].as_str().unwrap();
    assert!(path.ends_with("/fixture/home/Library/Caches/Google/Chrome/Default/Code Cache"), "{path}");
    assert_eq!(items[0]["allowed_modes"], Value::Array(vec![]), "a preview can never be planned");

    let safari = repo("rules/macos/apple.safari.cache.json");
    let (code, a) = cli(&["analyze", "--scan", &sid, "--preview-rule", safari.to_str().unwrap()], &root);
    assert_eq!(code, 0, "{a}");
    assert_eq!(a["candidate_count"], 0, "a TCC-protected location is not resolved: {a}");
    let issue = a["issues"].as_array().unwrap().iter().find(|i| i["code"] == "permission_required").unwrap_or_else(|| panic!("{a}"));
    assert_eq!(issue["scope_display"], "apple.safari.cache");
    assert!(issue["message"].as_str().unwrap().contains("permission_cause=undetermined"), "{issue}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_experimental_pack_is_listed_unverified_and_preselects_nothing() {
    let root = scratch("list");
    let sid = scanned(&root);
    let (code, _) = cli(&["experimental", "--state", "on"], &root);
    assert_eq!(code, 0);
    let (code, caps) = cli(&["capabilities"], &root);
    assert_eq!(code, 0);
    let row = caps["capabilities"].as_array().unwrap().iter().find(|c| c["name"] == "clean.catalog").unwrap();
    assert_eq!(row["state"], "unverified", "{row}");

    let (code, list) = cli(&["clean", "list"], &root);
    assert_eq!(code, 0, "{list}");
    assert_eq!(list["platform"], "macos");
    let categories = list["categories"].as_array().unwrap();
    assert!(categories.iter().all(|c| c["status"] != "available"), "nothing is available before real-Mac registration");
    let browsers = categories.iter().find(|c| c["category"] == "browsers").unwrap();
    assert_eq!(browsers["status"], "unverified");
    let chrome = browsers["rules"].as_array().unwrap().iter().find(|r| r["rule_id"] == "google.chrome.code-cache").unwrap();
    assert_eq!((chrome["status"].as_str(), chrome["registry_status"].as_str(), chrome["max_tier"].as_str()), (Some("experimental"), Some("not_registered"), Some("review_only")));

    let (code, preview) = cli(&["clean", "--scan", &sid, "--dry-run", "--non-interactive"], &root);
    assert_eq!(code, 0, "{preview}");
    assert_eq!(preview["kind"], "selection_preview");
    assert!(preview["items"].as_array().unwrap().iter().all(|i| i["selected_by_default"] == false), "{preview}");

    for bad in ["/", "/System/Volumes/Data", "/Volumes", "/Volumes/cm-no-such-volume"] {
        let (code, err) = cli(&["clean", "--scan", &sid, "--dry-run", "--volume", bad], &root);
        assert_ne!(code, 0, "{bad}");
        assert_eq!(err["code"], "invalid_argument", "{bad}: {err}");
        assert!(err["message"].as_str().unwrap().contains("--volume refused"), "{bad}: {err}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// The single candidate path of a preview of `rule` (relative to `rules/macos`), or none.
fn preview(sid: &str, rule: &str, root: &Path) -> (Value, Vec<String>) {
    let rule = repo(&format!("rules/macos/{rule}.json"));
    let (code, a) = cli(&["analyze", "--scan", sid, "--preview-rule", rule.to_str().unwrap()], root);
    assert_eq!(code, 0, "{a}");
    let Some(set) = a["candidate_set_id"].as_str() else { return (a, vec![]) };
    let (code, page) = cli(&["candidates", "list", "--set", set], root);
    assert_eq!(code, 0, "{page}");
    let items = page["items"].as_array().unwrap();
    assert!(items.iter().all(|i| i["allowed_modes"] == Value::Array(vec![])), "a preview can never be planned: {page}");
    (a, items.iter().map(|i| i["path"]["display"].as_str().unwrap().to_owned()).collect())
}

#[test]
fn wave_two_rules_resolve_through_the_overrides_and_containers_and_system_logs_stay_guarded() {
    let root = scratch("wave2");
    let fx = root.join("fixture");
    let home = fx.join("home");
    write(&home.join("Library/Application Support/Slack/Code Cache/js/index"));
    write(&home.join("Library/Application Support/Slack/IndexedDB/https_app.slack.com_0.indexeddb.leveldb/000003.log"));
    write(&home.join("Library/Containers/com.tencent.xinWeChat/Data/Library/Caches/com.tencent.xinWeChat/x"));
    write(&home.join("Library/Caches/Homebrew/downloads/0123--wget-1.24.5.bottle.tar.gz"));
    write(&home.join("Library/Caches/Homebrew/api/formula.jws.json"));
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400);
    let report = fx.join("system/Library/Logs/DiagnosticReports/kernel-2026-09-01.panic.ips");
    write(&report);
    std::fs::File::open(&report).unwrap().set_modified(old).unwrap();
    std::fs::create_dir_all(fx.join("temp")).unwrap();
    std::fs::create_dir_all(fx.join("downloads")).unwrap();
    let (code, scan) = cli(&["scan", "--root", fx.to_str().unwrap()], &root);
    assert_eq!(code, 0, "{scan}");
    let sid = scan["scan_id"].as_str().unwrap().to_owned();

    let (a, paths) = preview(&sid, "slack.code-cache", &root);
    assert!(issue_codes(&a).contains(&("resolver_override".into(), "CM_RESOLVE_SYSTEM".into())), "{a}");
    assert_eq!(paths.len(), 1, "{paths:?}");
    assert!(paths[0].ends_with("/fixture/home/Library/Application Support/Slack/Code Cache"), "{paths:?}");

    let (_, paths) = preview(&sid, "homebrew.downloads", &root);
    assert_eq!(paths.len(), 1, "{paths:?}");
    assert!(paths[0].ends_with("/Library/Caches/Homebrew/downloads/0123--wget-1.24.5.bottle.tar.gz"), "the API cache is not matched: {paths:?}");

    // Another app's container is protected by macOS privacy controls: reported, never read.
    let (a, paths) = preview(&sid, "tencent.wechat.cache", &root);
    assert!(paths.is_empty(), "{a}");
    let issue = a["issues"].as_array().unwrap().iter().find(|i| i["code"] == "permission_required").unwrap_or_else(|| panic!("{a}"));
    assert_eq!(issue["scope_display"], "tencent.wechat.cache");

    // The administrator-only system log rule resolves below the system stand-in (preview only: never plannable).
    let (_, paths) = preview(&sid, "macos.diagnostic-reports.system", &root);
    assert_eq!(paths.len(), 1, "{paths:?}");
    assert!(paths[0].ends_with("/fixture/system/Library/Logs/DiagnosticReports/kernel-2026-09-01.panic.ips"), "{paths:?}");

    let (code, _) = cli(&["experimental", "--state", "on"], &root);
    assert_eq!(code, 0);
    let (code, list) = cli(&["clean", "list"], &root);
    assert_eq!(code, 0, "{list}");
    let categories = list["categories"].as_array().unwrap();
    for cat in ["app_caches", "cloud_office", "virtualization"] {
        let c = categories.iter().find(|c| c["category"] == cat).unwrap_or_else(|| panic!("{cat}: {list}"));
        assert_eq!(c["status"], "unverified", "{cat}: experimental wave-2 rules only");
    }
    let (code, dry) = cli(&["clean", "--scan", &sid, "--dry-run", "--non-interactive"], &root);
    assert_eq!(code, 0, "{dry}");
    assert!(dry["items"].as_array().unwrap().iter().all(|i| i["selected_by_default"] == false), "{dry}");
    let _ = std::fs::remove_dir_all(&root);
}
