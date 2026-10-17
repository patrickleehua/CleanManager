//! System status and processes on macOS through the real binary (parity 12.1/12.3, STS-01/04/06/07, N13): one
//! snapshot and a JSONL stream whose metrics are numbers or explicitly unknown, the process list with protection,
//! and ending a child this test spawned through a `process` plan (receipt written through `cm_store` as the terminal
//! review would; it needs a TTY). Only that child is ever signalled. Gated commands need experimental features,
//! turned on in the scratch store.
#![cfg(target_os = "macos")]

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn scratch(tag: &str, experimental: bool) -> PathBuf {
    let tmp = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
    let dir = tmp.join(format!("cm-cli-macsts-{tag}-{}-{}-{nanos:08x}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&dir).unwrap();
    if experimental {
        let o = Command::new(env!("CARGO_BIN_EXE_cleanmanager")).args(["experimental", "--state", "on", "--json"]).env("CM_STORE_DIR", &dir).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stdout));
    }
    dir
}

/// Every stdout line as a JSON record.
fn run_lines(store: &Path, args: &[&str]) -> (i32, Vec<Value>) {
    let o = Command::new(env!("CARGO_BIN_EXE_cleanmanager")).args(args).arg("--json").env("CM_STORE_DIR", store).env_remove("CLEANMANAGER_LANG").output().unwrap();
    let text = String::from_utf8(o.stdout).expect("UTF-8");
    let lines = text.lines().map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}"))).collect();
    (o.status.code().unwrap(), lines)
}

fn run(store: &Path, args: &[&str]) -> (i32, Value) {
    let (code, mut lines) = run_lines(store, args);
    assert_eq!(lines.len(), 1, "exactly one record for {args:?}: {lines:?}");
    (code, lines.remove(0))
}

/// Top-level fields exactly as the schema declares for the record's kind.
fn check_envelope(v: &Value) {
    let schema: Value = serde_json::from_str(include_str!("../../../schemas/v1/protocol.schema.json")).unwrap();
    let kind = v["kind"].as_str().unwrap();
    let def = &schema["$defs"][kind];
    assert!(def.is_object(), "{kind} not in schema");
    let props = def["properties"].as_object().unwrap();
    for k in v.as_object().unwrap().keys() {
        assert!(props.contains_key(k), "{kind}: {k} not in schema");
    }
    for r in def["required"].as_array().unwrap() {
        assert!(v.get(r.as_str().unwrap()).is_some(), "{kind}: missing {r}");
    }
}

/// STS-01: a metric is a number, or null with availability not `available` and a reason. Never 0 for unknown.
fn check_metrics(snapshot: &Value) {
    let metrics = snapshot["metrics"].as_array().unwrap();
    assert!(!metrics.is_empty());
    for m in metrics {
        if m["availability"] == "available" {
            assert!(m["value"].is_number(), "{m}");
        } else {
            assert!(m["value"].is_null(), "{m}");
            assert!(m["reason"].as_str().is_some_and(|r| !r.is_empty()), "{m}");
        }
    }
    let get = |n: &str| metrics.iter().find(|m| m["name"] == n).unwrap_or_else(|| panic!("{n} missing"));
    for n in ["cpu.total_percent", "memory.used_percent", "disk.system.free_percent", "process.zombie_count"] {
        assert_eq!(get(n)["availability"], "available", "{}", get(n));
    }
    for n in ["thermal.max_zone_celsius", "fan.rpm"] {
        assert_eq!((get(n)["availability"].as_str(), get(n)["value"].is_null()), (Some("unavailable"), true));
    }
    let net = &snapshot["network"];
    assert_eq!(net["classification"], "interface_type", "{net}");
    assert!(net["loopback_excluded"].as_u64().unwrap() >= 1);
}

#[test]
fn status_snapshot_and_jsonl_stream() {
    let s = scratch("status", true);
    let (code, snap) = run(&s, &["status"]);
    assert_eq!((code, snap["kind"].as_str()), (0, Some("status_snapshot")), "{snap}");
    check_envelope(&snap);
    check_metrics(&snap);
    assert!(snap["health"]["score"].is_u64());
    assert_eq!(snap["health"]["weights"], serde_json::json!({"cpu": 30, "memory": 25, "disk": 20, "thermal": 15, "io": 10}));
    // STS-06: one compact record per line, each with its own sampling time.
    let (code, lines) = run_lines(&s, &["status", "--interval", "1", "--count", "3"]);
    assert_eq!(code, 0);
    assert_eq!(lines.len(), 3, "{lines:?}");
    let mut times = Vec::new();
    for l in &lines {
        assert_eq!(l["kind"], "status_snapshot");
        check_envelope(l);
        check_metrics(l);
        assert!(l["stale"].is_boolean());
        times.push(l["sampled_at"].as_str().unwrap().to_owned());
    }
    assert!(times.windows(2).all(|w| w[0] <= w[1]), "{times:?}");
    let _ = std::fs::remove_dir_all(&s);
}

#[test]
fn status_and_process_commands_are_gated() {
    let s = scratch("gated", false);
    for args in [&["status"][..], &["process", "list"], &["process", "end", "1"]] {
        let (code, e) = run(&s, args);
        assert_eq!((code, e["code"].as_str()), (7, Some("unsupported")), "{args:?}: {e}");
    }
    let _ = std::fs::remove_dir_all(&s);
}

fn approve(store: &Path, plan: &Value) {
    let stores = cm_store::Stores::open(store).unwrap();
    let kind: cm_core::plan::v2::PlanKind = serde_json::from_value(plan["body"]["plan_kind"].clone()).unwrap();
    stores
        .audit
        .save_kind_receipt(&cm_core::authz::KindReceipt {
            receipt_id: format!("r_{}", plan["body"]["plan_id"].as_str().unwrap()),
            plan_id: plan["body"]["plan_id"].as_str().unwrap().into(),
            plan_digest: plan["plan_digest"].as_str().unwrap().into(),
            plan_kind: kind,
            principal: cm_core::authz::current_principal(),
            source: cm_core::authz::ReceiptSource::InteractiveReview,
            created_at: cm_core::time::now_utc(),
            expires_at: plan["body"]["expires_at"].as_str().unwrap().into(),
            revoked_at: None,
        })
        .unwrap();
}

fn is_root() -> bool {
    Command::new("/usr/bin/id").arg("-u").output().is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
}

/// STS-04 on macOS: the list explains and protects; a reviewed plan ends exactly the spawned child with SIGTERM.
#[test]
fn process_list_protection_and_ending_a_spawned_child() {
    let s = scratch("process", true);
    let (code, list) = run(&s, &["process", "list", "--by", "memory", "--limit", "500"]);
    assert_eq!((code, list["kind"].as_str()), (0, Some("process_list")), "{list}");
    check_envelope(&list);
    let items = list["items"].as_array().unwrap();
    for i in items {
        assert_eq!(i["identified"].as_bool() == Some(false), i["ai_diagnosis"].is_object(), "unidentified rows carry the AI entry: {i}");
        assert!(i["network_bytes_per_sec"].is_null());
    }
    let (code, shown) = run(&s, &["process", "show", "1"]);
    assert_eq!(code, 0, "{shown}");
    let launchd = &shown["items"][0];
    assert_eq!((launchd["name"].as_str(), launchd["end_capability"]["protection"].as_str()), (Some("launchd"), Some("critical_system")), "{launchd}");
    let (code, e) = run(&s, &["process", "end", "1"]);
    assert_eq!((code, e["code"].as_str()), (3, Some("policy_denied")), "{e}");
    if !is_root() {
        // The oldest launchd daemon of another user stays alive; short-lived ones may exit before `process end`.
        let other = items
            .iter()
            .filter(|i| i["end_capability"]["protection"] == "other_user" && i["parent_pid"] == 1)
            .min_by_key(|i| i["pid"].as_u64())
            .expect("a process of another user");
        assert!(other["memory_bytes"].is_null() && other["cpu_percent"].is_null(), "withheld, never 0: {other}");
        let (code, e) = run(&s, &["process", "end", &other["pid"].to_string()]);
        assert_eq!((code, e["code"].as_str()), (3, Some("policy_denied")), "{e}");
    }
    let (code, e) = run(&s, &["process", "end", "999999999"]);
    assert_eq!((code, e["code"].as_str()), (2, Some("invalid_argument")));

    let mut child = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let pid = child.id().to_string();
    let (code, shown) = run(&s, &["process", "show", &pid]);
    assert_eq!(code, 0, "{shown}");
    assert_eq!(shown["items"][0]["end_capability"]["state"], "available", "{shown}");
    let (code, plan) = run(&s, &["process", "end", &pid]);
    assert_eq!((code, plan["kind"].as_str(), plan["body"]["plan_kind"].as_str()), (0, Some("plan"), Some("process")), "{plan}");
    assert_eq!(plan["authorization_status"], "required");
    let id = plan["body"]["plan_id"].as_str().unwrap().to_owned();
    let (code, e) = run(&s, &["plan", "apply", &id]);
    assert_eq!((code, e["code"].as_str()), (3, Some("needs_approval")), "{e}");
    assert!(child.try_wait().unwrap().is_none(), "nothing ends without a review");
    approve(&s, &plan);
    let (code, report) = run(&s, &["plan", "apply", &id]);
    assert_eq!(code, 0, "{report}");
    let item = &report["items"][0];
    assert_eq!((item["status"].as_str(), item["code"].as_str()), (Some("succeeded"), Some("ended")), "{report}");
    let status = child.wait().unwrap();
    assert_eq!(status.signal(), Some(libc_sigterm()), "ended with SIGTERM: {status:?}");
    let _ = std::fs::remove_dir_all(&s);
}

/// SIGTERM (15 on macOS).
fn libc_sigterm() -> i32 {
    15
}
