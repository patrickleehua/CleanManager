//! MVP 12.3 / CLI-02/03/09: a scan hosted by one `cleanmanager` process is queried, cancelled and
//! waited on from other processes; killing the host leaves a queryable state that becomes
//! `interrupted` (never "running forever"); a cancel receipt is not a terminal state; a wait or
//! host timeout never claims a rollback. Batches are slowed with the debug-only
//! `CM_TEST_SCAN_SLOW_MS` hook so the scan is observably alive.
#![cfg(any(windows, target_os = "macos"))]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const SLOW_MS: &str = "100";

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-life-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// 40 directories x 3 files (161 entries). The backend commits one batch per directory, so at
/// 100 ms per batch the scan takes about 4 s.
fn fixture(base: &Path) -> PathBuf {
    let root = base.join("tree");
    for d in 0..40 {
        let dir = root.join(format!("d{d:02}"));
        std::fs::create_dir_all(&dir).unwrap();
        for f in 0..3 {
            std::fs::write(dir.join(format!("f{f:02}.bin")), vec![b'x'; 64 + f]).unwrap();
        }
    }
    root
}

fn cmd(store: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_cleanmanager"));
    c.env("CM_STORE_DIR", store).env("CM_TEST_SCAN_SLOW_MS", SLOW_MS);
    c
}

/// One CLI call from "another process": exit code and its single record.
fn cli(store: &Path, args: &[&str]) -> (i32, Value) {
    let out = cmd(store).args(args).arg("--json").output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "exactly one record: {text}");
    (out.status.code().unwrap(), serde_json::from_str(&text).unwrap())
}

/// Start a hosted scan streaming JSONL; returns the host and the scan id from its first progress record.
fn start_scan(store: &Path, root: &Path, extra: &[&str]) -> (Child, BufReader<std::process::ChildStdout>, String) {
    let mut child = cmd(store)
        .args(["scan", "--root"])
        .arg(root)
        .args(["--format", "jsonl", "--non-interactive"])
        .args(extra)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    out.read_line(&mut first).unwrap();
    let progress: Value = serde_json::from_str(&first).unwrap_or_else(|e| panic!("first line {first:?}: {e}"));
    assert_eq!(progress["kind"], "progress");
    assert_eq!(progress["sequence"], 0);
    assert!(progress["total_units"].is_null(), "no fabricated denominator");
    let id = progress["task_id"].as_str().unwrap().to_owned();
    (child, out, id)
}

fn rest(out: BufReader<std::process::ChildStdout>) -> Vec<Value> {
    out.lines().map(|l| serde_json::from_str(&l.unwrap()).unwrap()).collect()
}

#[test]
fn killed_host_leaves_a_queryable_scan_that_becomes_interrupted() {
    let base = scratch("kill");
    let (root, store) = (fixture(&base), base.join("store"));
    let (mut host, out, id) = start_scan(&store, &root, &[]);

    // Another process sees the live scan as running, hosted by that process.
    let (code, live) = cli(&store, &["scan", "show", &id]);
    assert_eq!(code, 0);
    assert_eq!(live["kind"], "scan_status", "{live}");
    assert!(["running", "stopping"].contains(&live["state"].as_str().unwrap()));
    assert_eq!(live["host"]["process_id"].as_u64(), Some(u64::from(host.id())));
    std::thread::sleep(Duration::from_millis(400));

    host.kill().unwrap();
    host.wait().unwrap();
    // The stream has no terminal record: callers must treat it as incomplete and query by id.
    assert!(rest(out).iter().all(|r| r["kind"] == "progress"), "a killed host emits no terminal record");

    let (code, after) = cli(&store, &["scan", "show", &id]);
    assert_eq!(after["kind"], "scan_result", "{after}");
    assert_eq!((after["status"].as_str(), after["completeness"].as_str()), (Some("interrupted"), Some("partial_scope")));
    assert!(after["issues"].as_array().unwrap().iter().any(|i| i["code"] == "host_exited"), "{after}");
    assert_eq!(code, 5, "interrupted keeps its results: partial");
    assert!(after["entries_seen"].as_u64().unwrap() >= 1);
    // The committed rows are published and browsable.
    let (code, page) = cli(&store, &["entries", "list", "--scan", &id, "--limit", "5"]);
    assert_eq!(code, 0, "{page}");
    assert!(!page["items"].as_array().unwrap().is_empty());
    // A cancel after the fact is answered truthfully, and waiting returns at once.
    let (_, receipt) = cli(&store, &["scan", "cancel", &id]);
    assert_eq!((receipt["accepted"].as_bool(), receipt["state_at_receipt"].as_str()), (Some(false), Some("already_terminal")));
    let (code, waited) = cli(&store, &["scan", "wait", &id, "--timeout", "5"]);
    assert_eq!((code, waited["status"].as_str()), (5, Some("interrupted")));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn cancel_from_another_process_is_a_receipt_then_the_scan_ends_cancelled() {
    let base = scratch("cancel");
    let (root, store) = (fixture(&base), base.join("store"));
    let (mut host, out, id) = start_scan(&store, &root, &[]);

    let (code, receipt) = cli(&store, &["scan", "cancel", &id]);
    assert_eq!(code, 0);
    assert_eq!(receipt["kind"], "scan_cancel_receipt");
    assert_eq!(receipt["accepted"], true);
    assert_eq!(receipt["state_at_receipt"], "running");
    assert!(receipt["note"].as_str().unwrap().starts_with("Receipt only"));

    // Waiting from a third process returns the terminal record and the scan's own exit class.
    let (code, done) = cli(&store, &["scan", "wait", &id, "--timeout", "60"]);
    assert_eq!(done["kind"], "scan_result", "{done}");
    assert_eq!((code, done["status"].as_str()), (130, Some("cancelled")));
    assert_eq!(done["completeness"], "partial_scope");

    let status = host.wait().unwrap();
    assert_eq!(status.code(), Some(130));
    let lines = rest(out);
    let last = lines.last().unwrap();
    assert_eq!((last["kind"].as_str(), last["status"].as_str()), (Some("scan_result"), Some("cancelled")));
    assert_eq!(lines.iter().filter(|r| r["kind"] == "scan_result").count(), 1, "exactly one terminal record");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn wait_timeout_reports_the_running_state_and_cancels_nothing() {
    let base = scratch("wait");
    let (root, store) = (fixture(&base), base.join("store"));
    let (mut host, out, id) = start_scan(&store, &root, &[]);

    let started = Instant::now();
    let (code, status) = cli(&store, &["scan", "wait", &id, "--timeout", "1"]);
    assert!(started.elapsed() < Duration::from_secs(20));
    assert_eq!(code, 8, "a spent wait budget is classified 8: query again, nothing rolled back");
    assert_eq!(status["kind"], "scan_status", "{status}");
    assert!(["running", "stopping"].contains(&status["state"].as_str().unwrap()));
    assert_eq!(status["cancel_requested"], false, "waiting never cancels");

    // The host is unaffected and finishes normally.
    assert_eq!(host.wait().unwrap().code(), Some(0));
    let last = rest(out).pop().unwrap();
    assert_eq!(last["status"], "completed");
    let (code, done) = cli(&store, &["scan", "show", &id]);
    assert_eq!((code, done["status"].as_str()), (0, Some("completed")));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn host_timeout_stops_at_a_safe_boundary_without_claiming_rollback() {
    let base = scratch("budget");
    let (root, store) = (fixture(&base), base.join("store"));
    let out = cmd(&store).args(["scan", "--root"]).arg(&root).args(["--timeout", "1", "--json"]).output().unwrap();
    let rec: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(8), "{rec}");
    assert_eq!((rec["kind"].as_str(), rec["status"].as_str()), (Some("scan_result"), Some("cancelled")));
    let issue = rec["issues"].as_array().unwrap().iter().find(|i| i["code"] == "timeout").expect("timeout issue");
    assert!(issue["message"].as_str().unwrap().contains("nothing was rolled back"));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn detached_scan_returns_at_once_and_is_hosted_by_another_process() {
    let base = scratch("detach");
    let (root, store) = (fixture(&base), base.join("store"));
    let started = Instant::now();
    // Time the record, not pipe EOF: concurrently spawned test processes may inherit this pipe on Windows.
    let mut caller = cmd(&store).args(["scan", "--root"]).arg(&root).args(["--detach", "--non-interactive", "--json"]).stdout(Stdio::piped()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(caller.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let returned = started.elapsed();
    let code = caller.wait().unwrap().code().unwrap();
    let live: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(code, 0, "{live}");
    assert_eq!(live["kind"], "scan_status");
    // It answered while the background host was still scanning (a finished scan would answer scan_result).
    assert!(["running", "stopping"].contains(&live["state"].as_str().unwrap()), "{live}");
    assert!(returned < Duration::from_secs(30), "{returned:?}");
    let id = live["scan_id"].as_str().unwrap().to_owned();
    assert_ne!(live["host"]["process_id"].as_u64(), Some(u64::from(std::process::id())));
    let (code, done) = cli(&store, &["scan", "wait", &id, "--timeout", "120"]);
    assert_eq!((code, done["status"].as_str()), (0, Some("completed")), "{done}");
    assert_eq!(done["entries_seen"], 161);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn lifecycle_arguments_are_checked() {
    let base = scratch("args");
    let store = base.join("store");
    for args in [
        vec!["scan", "wait", "s_x", "--timeout", "-1"],
        vec!["scan", "wait", "s_x", "--timeout", "999999"],
        vec!["scan", "--root", ".", "--timeout", "0"],
        vec!["scan", "--root", ".", "--host-scan-id", "../x"],
        vec!["scan", "show"],
    ] {
        let (code, err) = cli(&store, &args);
        assert_eq!((code, err["code"].as_str()), (2, Some("invalid_argument")), "{args:?}: {err}");
    }
    let (code, err) = cli(&store, &["scan", "show", "s_missing"]);
    assert_ne!(code, 0);
    assert_eq!(err["kind"], "error");
    let _ = std::fs::remove_dir_all(&base);
}
