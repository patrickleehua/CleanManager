//! Read-only parity commands (`startup list`, `status`, `insights`, `apps list` and `apps uninstall list`): one clean JSON record (or JSONL stream), no
//! side effects, unknown values as null with a reason. Structure only: real values differ between machines.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// D15: these tools are off by default. One scratch store with experimental features turned on, the way a person
/// opts in, instead of the real per-user store.
fn store() -> &'static PathBuf {
    static STORE: OnceLock<PathBuf> = OnceLock::new();
    STORE.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("cm-cli-parity-ro-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        let o = Command::new(env!("CARGO_BIN_EXE_cleanmanager")).args(["experimental", "--state", "on", "--json"]).env("CM_STORE_DIR", &p).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stdout));
        p
    })
}

fn run(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_cleanmanager")).args(args).env("CM_STORE_DIR", store()).output().expect("binary runs");
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
        // 11.2: only Run values and Startup-folder files have a verified mechanism; listing never changes anything.
        if i["toggle_capability"] == "available" {
            assert!(matches!(i["source"].as_str(), Some("run_key" | "startup_folder")), "{i}");
        }
        if matches!(i["source"].as_str(), Some("scheduled_task" | "store_startup_task")) {
            assert_ne!(i["toggle_capability"], "available", "no verified toggle: settings page only");
        }
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
    for l in &lines {
        assert_network_totals(l);
    }
}

/// STS-07: the total is the sum of the counted interfaces only; loopback is never listed; tunnel and virtual adapters
/// are listed separately; the double-count flag is set exactly when an unknown interface is counted.
#[cfg(windows)]
fn assert_network_totals(v: &serde_json::Value) {
    let n = &v["network"];
    assert!(n.is_object(), "status_snapshot carries network totals on Windows: {v}");
    assert!(matches!(n["classification"].as_str(), Some("interface_type" | "unavailable")));
    let counted = n["counted"].as_array().unwrap();
    let separate = n["separate"].as_array().unwrap();
    let sum: f64 = counted.iter().map(|i| i["received_bytes_per_sec"].as_f64().unwrap()).sum();
    assert!((sum - n["received_bytes_per_sec"].as_f64().unwrap()).abs() < 1e-6);
    assert!(counted.iter().all(|i| matches!(i["class"].as_str(), Some("physical" | "unknown"))));
    assert!(separate.iter().all(|i| matches!(i["class"].as_str(), Some("tunnel" | "virtual"))));
    assert_eq!(n["may_double_count"], n["classification"] == "unavailable" || counted.iter().any(|i| i["class"] == "unknown"));
    assert!(n["loopback_excluded"].is_u64());
    let total = v["metrics"].as_array().unwrap().iter().find(|m| m["name"] == "network.received_bytes_per_sec").unwrap();
    assert_eq!(total["value"], n["received_bytes_per_sec"]);
    assert_eq!(total["reason"], n["note"]);
}

#[cfg(windows)]
#[test]
fn status_snapshot_reports_deduplicated_network_totals() {
    let (code, out) = run(&["status", "--json"]);
    assert_eq!(code, 0);
    let v = single(&out);
    assert_network_totals(&v);
    assert_eq!(v["network"]["classification"], "interface_type", "the IP Helper interface table is readable on Windows");
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

/// UNI-01: `apps list` reports each instance's uninstall capability as `apps uninstall list` decides it (no longer the
/// read-only release's blanket `unsupported`); protected instances are never available. Both only read.
#[cfg(windows)]
#[test]
fn apps_list_uninstall_capability_matches_the_uninstall_route() {
    let (code, out) = run(&["apps", "uninstall", "list", "--json"]);
    assert!(code == 0 || code == 5, "{out}");
    let routes: std::collections::BTreeMap<String, serde_json::Value> =
        single(&out)["items"].as_array().unwrap().iter().map(|i| (i["app_instance_id"].as_str().unwrap().to_owned(), i["uninstall"].clone())).collect();
    let (mut cursor, mut seen, mut matched) = (None::<String>, 0, 0);
    loop {
        let mut args = vec!["apps", "list", "--limit", "500", "--json"];
        if let Some(c) = &cursor {
            args.extend(["--cursor", c.as_str()]);
        }
        let (code, out) = run(&args);
        assert_eq!(code, 0, "{out}");
        let v = single(&out);
        for a in v["items"].as_array().unwrap() {
            seen += 1;
            let cap = &a["uninstall_capability"];
            assert!(!cap["reason"].as_str().is_some_and(|r| r.starts_with("Read-only first release")), "{a}");
            if a["protected_class"] == true {
                assert_ne!(cap["state"], "available", "{a}");
            }
            match routes.get(a["app_instance_id"].as_str().unwrap()) {
                Some(r) if a["protected_class"] == false || r["state"] != "available" => {
                    assert_eq!(cap["state"], r["state"], "{a} vs {r}");
                    assert_eq!(cap["reason"], r["reason"], "{a} vs {r}");
                    matched += 1;
                }
                Some(_) => {}
                None => assert_eq!(cap["state"], "unverified", "no uninstall record of its own: {a}"),
            }
        }
        match v["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => break,
        }
    }
    assert!(seen > 0 && matched > 0, "seen {seen}, matched {matched}");
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
