//! Parity 12.3/12.4, 7.3 and 10.3 through the real binary (Windows). Only a child process this test spawns could
//! ever be ended, and none is: `process end` only creates a plan, which stays `needs_approval` for machine callers.
//! Update checks are turned off first, so no package manager runs and no network request is made.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-sts-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    experimental_on(&p);
    experimental_on(&p.join("store"));
    p
}

/// D15: the gated tools under test are off by default. Opt in to experimental features in the scratch stores (the
/// directory itself and its `store` child) the way a person does, through the CLI.
fn experimental_on(store: &Path) {
    let o = Command::new(env!("CARGO_BIN_EXE_cleanmanager")).args(["experimental", "--state", "on", "--json"]).env("CM_STORE_DIR", store).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stdout));
}

fn run(store: &Path, args: &[&str]) -> (i32, Value) {
    let o = Command::new(env!("CARGO_BIN_EXE_cleanmanager")).args(args).env("CM_STORE_DIR", store).env_remove("CLEANMANAGER_LANG").output().unwrap();
    let text = String::from_utf8(o.stdout).expect("UTF-8");
    let line = text.lines().last().unwrap_or_else(|| panic!("no record for {args:?}"));
    (o.status.code().unwrap(), serde_json::from_str(line).expect("JSON record"))
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

/// UPD-04 at the CLI: with checks off, `apps updates` reports nothing checked (no channel is run).
#[test]
fn update_checks_off_report_not_checked() {
    let s = scratch("updates");
    let (code, cfg) = run(&s, &["apps", "updates", "config", "--checks", "off", "--json"]);
    assert_eq!((code, cfg["kind"].as_str(), cfg["checks_enabled"].as_bool()), (0, Some("app_update_settings"), Some(false)));
    check_envelope(&cfg);
    let t0 = std::time::Instant::now();
    let (code, list) = run(&s, &["apps", "updates", "--json"]);
    assert_eq!((code, list["kind"].as_str()), (0, Some("app_update_list")));
    check_envelope(&list);
    assert_eq!((list["checked"].as_bool(), list["checked_at"].is_null()), (Some(false), true));
    assert!(list["items"].as_array().unwrap().iter().all(|i| i["state"] == "not_checked" && i["checked_at"].is_null()));
    assert!(t0.elapsed() < std::time::Duration::from_secs(60));
    let (code, _) = run(&s, &["apps", "updates", "config", "--checks", "maybe", "--json"]);
    assert_eq!(code, 2);
    let (code, e) = run(&s, &["apps", "update", "--app", "app_x", "--json"]);
    assert_eq!((code, e["code"].as_str()), (2, Some("invalid_argument")), "no results yet: nothing to plan");
    let _ = std::fs::remove_dir_all(&s);
}

/// STS-04 at the CLI: listing explains, critical processes are refused, an end plan needs a human review.
#[test]
fn process_list_and_end_plans() {
    let s = scratch("process");
    let (code, list) = run(&s, &["process", "list", "--by", "memory", "--limit", "5", "--json"]);
    assert_eq!((code, list["kind"].as_str()), (0, Some("process_list")));
    check_envelope(&list);
    assert_eq!(list["items"].as_array().unwrap().len(), 5);
    for i in list["items"].as_array().unwrap() {
        assert_eq!(i["identified"].as_bool() == Some(false), i["ai_diagnosis"].is_object(), "unidentified rows carry the AI entry");
    }
    let (code, _) = run(&s, &["process", "list", "--by", "io", "--json"]);
    assert_eq!(code, 2);
    // csrss is always present; its end plan is refused without anything being opened for termination.
    let (_, all) = run(&s, &["process", "list", "--search", "csrss.exe", "--limit", "500", "--json"]);
    let csrss = all["items"].as_array().unwrap().iter().find(|i| i["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case("csrss.exe"))).expect("csrss listed");
    assert_eq!(csrss["end_capability"]["protection"], "critical_system");
    let (code, e) = run(&s, &["process", "end", &csrss["pid"].to_string(), "--json"]);
    assert_eq!((code, e["code"].as_str()), (3, Some("policy_denied")));
    // A child of this test: the plan is created, but nothing ends it without an interactive review.
    let mut child = Command::new(r"C:\Windows\System32\PING.EXE").args(["-n", "60", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
    let (code, plan) = run(&s, &["process", "end", &child.id().to_string(), "--json"]);
    assert_eq!((code, plan["kind"].as_str(), plan["body"]["plan_kind"].as_str()), (0, Some("plan"), Some("process")));
    assert_eq!(plan["authorization_status"], "required");
    let id = plan["body"]["plan_id"].as_str().unwrap().to_owned();
    let (code, e) = run(&s, &["plan", "apply", &id, "--json"]);
    assert_eq!((code, e["code"].as_str()), (3, Some("needs_approval")));
    let (code, _) = run(&s, &["plan", "review", &id, "--non-interactive", "--json"]);
    assert_ne!(code, 0, "machine callers never approve");
    assert!(child.try_wait().unwrap().is_none(), "the child is still running");
    child.kill().unwrap();
    child.wait().unwrap();
    let (code, pins) = run(&s, &["process", "pin", "code.exe", "--json"]);
    assert_eq!((code, pins["pins"][0].as_str()), (0, Some("code.exe")));
    let _ = std::fs::remove_dir_all(&s);
}

/// STS-05 settings are opt-in and validated; DIN-04 review has nothing selected.
#[test]
fn alert_settings_and_old_downloads_review() {
    let s = scratch("alerts");
    let (code, a) = run(&s, &["status", "alerts", "--json"]);
    assert_eq!((code, a["enabled"].as_bool()), (0, Some(false)));
    check_envelope(&a);
    let (code, a) = run(&s, &["status", "alerts", "--state", "on", "--cpu-threshold", "90", "--json"]);
    assert_eq!((code, a["enabled"].as_bool(), a["cpu_threshold_percent"].as_f64()), (0, Some(true), Some(90.0)));
    let (code, _) = run(&s, &["status", "alerts", "--window-seconds", "1", "--json"]);
    assert_eq!(code, 2);
    let (code, r) = run(&s, &["insights", "downloads", "--older-than-days", "1", "--limit", "3", "--json"]);
    assert_eq!((code, r["kind"].as_str(), r["handling"].as_str()), (0, Some("old_downloads_review"), Some("manual_trash_review")));
    check_envelope(&r);
    assert!(r["items"].as_array().unwrap().iter().all(|i| i["selected"] == false));
    let _ = std::fs::remove_dir_all(&s);
}
