//! Read-only parity commands (`startup list`, `status`, `insights`): one clean JSON record (or JSONL stream), no
//! side effects, unknown values as null with a reason. Structure only: real values differ between machines.

use std::process::Command;

fn run(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_cleanmanager")).args(args).output().expect("binary runs");
    (out.status.code().unwrap_or(-1), String::from_utf8(out.stdout).expect("stdout is UTF-8"))
}

fn single(stdout: &str) -> serde_json::Value {
    assert!(!stdout.starts_with('\u{feff}') && !stdout.contains('\u{1b}'));
    assert_eq!(stdout.lines().count(), 1, "exactly one record on stdout");
    serde_json::from_str(stdout.trim_end()).expect("valid JSON")
}

#[test]
fn bad_stream_arguments_are_rejected() {
    for args in [vec!["status", "--interval", "0"], vec!["status", "--count", "x"], vec!["insights", "--scan", "s1"], vec!["startup", "list", "--limit", "5"]] {
        let mut a = args;
        a.push("--json");
        let (code, out) = run(&a);
        assert_eq!(code, 2, "{a:?}");
        assert_eq!(single(&out)["code"], "invalid_argument");
    }
}

#[cfg(windows)]
#[test]
fn startup_list_is_read_only_inventory() {
    let (code, out) = run(&["startup", "list", "--json"]);
    assert!(code == 0 || code == 5, "success, or partial when a source was unreadable");
    let v = single(&out);
    assert_eq!(v["kind"], "startup_list");
    assert_eq!(code == 5, !v["issues"].as_array().unwrap().is_empty());
    let items = v["items"].as_array().unwrap();
    assert!(!items.is_empty());
    for i in items {
        assert_ne!(i["toggle_capability"], "available", "toggling is not implemented (11.2)");
        if i["protected"] == true {
            assert_eq!(i["toggle_capability"], "unsupported");
        }
    }
}

#[cfg(windows)]
#[test]
fn status_snapshot_declares_unknowns() {
    let (code, out) = run(&["status", "--json"]);
    assert_eq!(code, 0);
    let v = single(&out);
    assert_eq!(v["kind"], "status_snapshot");
    assert_eq!(v["stale"], false);
    for m in v["metrics"].as_array().unwrap() {
        assert_eq!(m["value"].is_null(), m["availability"] != "available", "{}", m["name"]);
        if m["value"].is_null() {
            assert!(m["reason"].is_string());
        }
    }
    let fan = v["metrics"].as_array().unwrap().iter().find(|m| m["name"] == "fan.rpm").unwrap();
    assert!(fan["value"].is_null());
    assert_eq!(v["health"]["weights"], serde_json::json!({"cpu": 30, "memory": 25, "disk": 20, "thermal": 15, "io": 10}));
    assert_eq!(v["health"]["dimensions"].as_array().unwrap().len(), 5);
}

#[cfg(windows)]
#[test]
fn status_stream_is_jsonl_with_increasing_sample_times() {
    let (code, out) = run(&["status", "--format", "jsonl", "--interval", "1", "--count", "2"]);
    assert_eq!(code, 0);
    let lines: Vec<serde_json::Value> = out.lines().map(|l| serde_json::from_str(l).expect("each line is one JSON record")).collect();
    assert_eq!(lines.len(), 2);
    assert!(lines.iter().all(|l| l["kind"] == "status_snapshot"));
    assert!(lines[1]["sampled_at"].as_str() >= lines[0]["sampled_at"].as_str());
}

#[cfg(windows)]
#[test]
fn insights_offer_guidance_never_trash() {
    let (code, out) = run(&["insights", "--json"]);
    assert_eq!(code, 0);
    let v = single(&out);
    assert_eq!(v["kind"], "insight_list");
    let items = v["items"].as_array().unwrap();
    assert!(items.iter().any(|i| i["kind"] == "windows.component_store"));
    for i in items {
        assert!(["read_only_guidance", "open_system_settings", "open_owner_tool"].contains(&i["handling"].as_str().unwrap()));
        assert!(!i["guidance"].as_array().unwrap().is_empty());
        assert_eq!(i["logical_bytes"].is_null(), i["size_basis"] == "not_measured");
        assert!(i.get("allowed_modes").is_none());
    }
}

#[cfg(not(windows))]
#[test]
fn parity_commands_are_unsupported_off_windows() {
    for args in [vec!["startup", "list", "--json"], vec!["status", "--json"], vec!["insights", "--json"]] {
        let (code, out) = run(&args);
        assert_eq!(code, 7);
        assert_eq!(single(&out)["code"], "unsupported");
    }
}
