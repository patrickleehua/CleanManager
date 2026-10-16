//! The cleanup catalog on macOS end to end through the binary: rule roots resolve inside a fixture tree through
//! `CM_RESOLVE_HOME` / `CM_RESOLVE_TEMP` / `CM_RESOLVE_DOWNLOADS` (each reported as `resolver_override`), a
//! privacy-protected location is reported as `permission_required` instead of read, the experimental macOS pack
//! preselects nothing, and `--volume` refuses anything but a mounted, local, non-startup volume.
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
    for d in ["temp", "downloads"] {
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
    for var in ["CM_RESOLVE_HOME", "CM_RESOLVE_TEMP", "CM_RESOLVE_DOWNLOADS"] {
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
