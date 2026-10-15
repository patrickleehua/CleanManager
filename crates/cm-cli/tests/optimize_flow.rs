//! System maintenance through the real binary (parity 13.1, 14.1, 14.4; MNT-02..07, PRT-06, EXE-08).
//!
//! Every task is scripted (`CM_MAINTENANCE_TEST_SCRIPT`), so no shell host is restarted, no cache file, DNS cache, SSD
//! or component store is touched. Elevation never prompts: `CM_MAINTENANCE_TEST_ELEVATION=declined` replaces UAC by a
//! declining launcher and `direct` by an ordinary unelevated child of this binary that speaks the real worker protocol
//! over the real named pipe. Receipts are written through `cm_store` as the terminal review would (it needs a TTY).
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-optimize-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn schema() -> Value {
    serde_json::from_str(include_str!("../../../schemas/v1/protocol.schema.json")).unwrap()
}

/// The record carries exactly the schema's top-level fields of its kind.
fn check_shape(rec: &Value) {
    let s = schema();
    let kind = rec["kind"].as_str().unwrap();
    let props = s["$defs"][kind]["properties"].as_object().unwrap_or_else(|| panic!("schema has no record {kind}"));
    let mut want: Vec<&String> = props.keys().collect();
    let mut got: Vec<&String> = rec.as_object().unwrap().keys().collect();
    want.sort();
    got.sort();
    assert_eq!(got, want, "{kind}: top-level fields differ from the schema");
}

struct Env<'a> {
    store: &'a Path,
    script: &'a Path,
    elevation: Option<&'a str>,
}

fn cli(args: &[&str], env: &Env<'_>) -> (i32, Value) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cleanmanager"));
    cmd.args(args).arg("--json").env("CM_STORE_DIR", env.store).env("CM_MAINTENANCE_TEST_SCRIPT", env.script).env_remove("CM_MAINTENANCE_TEST_ELEVATION");
    if let Some(e) = env.elevation {
        cmd.env("CM_MAINTENANCE_TEST_ELEVATION", e);
    }
    let out = cmd.output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "exactly one record: {text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    if v["kind"] != "error" && v["kind"] != "plan" {
        check_shape(&v);
    }
    (out.status.code().unwrap(), v)
}

fn approve(store: &Path, plan: &Value) {
    let stores = cm_store::Stores::open(store).unwrap();
    let kind: cm_core::plan::v2::PlanKind = serde_json::from_value(plan["body"]["plan_kind"].clone()).unwrap();
    assert_eq!(kind, cm_core::plan::v2::PlanKind::Maintenance);
    stores
        .audit
        .save_kind_receipt(&cm_core::authz::KindReceipt {
            receipt_id: format!("r_{}", plan["body"]["plan_id"].as_str().unwrap()),
            plan_id: plan["body"]["plan_id"].as_str().unwrap().into(),
            plan_digest: plan["plan_digest"].as_str().unwrap().into(),
            plan_kind: kind,
            principal: cm_core::authz::current_principal(),
            source: cm_core::authz::ReceiptSource::InteractiveReview,
            created_at: plan["body"]["created_at"].as_str().unwrap().into(),
            expires_at: plan["body"]["expires_at"].as_str().unwrap().into(),
            revoked_at: None,
        })
        .unwrap();
}

fn item<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["items"].as_array().unwrap().iter().find(|i| i["task_id"] == id).unwrap_or_else(|| panic!("{id} in {report}"))
}

const SCRIPT: &str = r#"{
  "windows.shell-restart": {"probe": "not_needed", "code": "not_running", "reason": "Start is not running"},
  "windows.dns-flush": {"outcome": "applied", "code": "flushed", "reason": "scripted flush"},
  "windows.trim-when-due": {"probe": "not_needed", "code": "not_due", "reason": "C: last optimized 2026-10-10T00:00:00Z"},
  "windows.image-health-check": {"outcome": "attention", "code": "repairable", "reason": "scripted: repairable"}
}"#;

#[test]
fn mnt05_catalog_and_prohibited_search() {
    let store = scratch("catalog");
    let script = store.join("script.json");
    std::fs::write(&script, SCRIPT).unwrap();
    let env = Env { store: &store, script: &script, elevation: None };
    let (code, cat) = cli(&["optimize", "list"], &env);
    assert_eq!(code, 0);
    let items = cat["items"].as_array().unwrap();
    assert_eq!(items.len(), 5);
    assert!(items.iter().all(|i| i["problem"].is_string() && i["impact"].is_string() && i["requires_admin"].is_boolean() && i["reversible"] == false));
    assert_eq!(item(&cat, "windows.trim-when-due")["applicability"], "not_needed");
    assert!(cat["prohibited"].as_array().unwrap().len() >= 8);
    // "注册表优化": no task, only the reason it is not offered.
    let (code, found) = cli(&["optimize", "list", "--search", "注册表优化"], &env);
    assert_eq!(code, 0);
    assert!(found["items"].as_array().unwrap().is_empty());
    let p = found["prohibited"].as_array().unwrap();
    assert!(p.iter().any(|x| x["id"] == "registry-optimizer" && x["reason"].as_str().unwrap().contains("no task")));
    // Naming a prohibited entry as a task is refused.
    let (code, err) = cli(&["optimize", "plan", "create", "--tasks", "registry-optimizer"], &env);
    assert_eq!((code, err["code"].as_str()), (3, Some("policy_denied")), "{err}");
    let _ = std::fs::remove_dir_all(&store);
}

/// MNT-04 + PRT-06 + MNT-07: a reviewed plan; the declined elevation skips only administrator tasks; a task protected
/// after planning is skipped as protected; history lists the run without undo.
#[test]
fn mnt04_declined_elevation_and_prt06_protection() {
    let store = scratch("declined");
    let script = store.join("script.json");
    std::fs::write(&script, SCRIPT).unwrap();
    let env = Env { store: &store, script: &script, elevation: Some("declined") };
    let (code, plan) = cli(&["optimize", "plan", "create", "--tasks", "windows.dns-flush,windows.icon-cache-rebuild,windows.trim-when-due,windows.image-health-check"], &env);
    assert_eq!(code, 0, "{plan}");
    assert_eq!((plan["body"]["plan_kind"].as_str(), plan["body"]["requires_admin"].as_bool(), plan["authorization_status"].as_str()), (Some("maintenance"), Some(true), Some("required")));
    let plan_id = plan["body"]["plan_id"].as_str().unwrap().to_owned();
    // Non-interactive callers never approve: apply without a review is needs_approval.
    let (code, err) = cli(&["plan", "apply", &plan_id], &env);
    assert_eq!(err["code"], "needs_approval", "{err}");
    assert_ne!(code, 0);
    approve(&store, &plan);
    // The user protects the icon cache task after planning (PRT-06).
    let (code, _) = cli(&["protect", "add", "--task", "windows.icon-cache-rebuild"], &env);
    assert_eq!(code, 0);
    let (code, report) = cli(&["plan", "apply", &plan_id], &env);
    assert_eq!(report["kind"], "maintenance_report", "{report}");
    assert_eq!(code, 5, "partial: tasks were skipped and one needs attention");
    assert_eq!(report["elevation"], "declined");
    assert_eq!((item(&report, "windows.dns-flush")["outcome"].as_str(), item(&report, "windows.dns-flush")["code"].as_str()), (Some("applied"), Some("flushed")));
    assert_eq!(item(&report, "windows.icon-cache-rebuild")["code"], "protected");
    for id in ["windows.trim-when-due", "windows.image-health-check"] {
        assert_eq!((item(&report, id)["outcome"].as_str(), item(&report, id)["code"].as_str()), (Some("skipped"), Some("elevation_declined")));
    }
    assert_eq!(report["counts"]["skipped"], 3);
    assert_eq!(report["undo"], "unavailable");
    // A protected task named explicitly in a new plan is refused; the batch plan leaves it out.
    let (code, err) = cli(&["optimize", "plan", "create", "--tasks", "windows.icon-cache-rebuild"], &env);
    assert_eq!((code, err["code"].as_str()), (3, Some("policy_denied")));
    let (code, batch) = cli(&["optimize", "plan", "create"], &env);
    assert_eq!(code, 0);
    let ids: Vec<&str> = batch["body"]["items"].as_array().unwrap().iter().map(|a| a["task_id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["windows.dns-flush", "windows.image-health-check"], "protected and not-needed tasks are not batch-selected");
    let (code, hist) = cli(&["optimize", "history"], &env);
    assert_eq!(code, 0);
    assert_eq!(hist["items"].as_array().unwrap().len(), 1);
    assert_eq!(hist["items"][0]["operation_id"], report["operation_id"]);
    assert_eq!(hist["undo"], "unavailable");
    let (_, ops) = cli(&["operations", "list"], &env);
    let op = ops["items"].as_array().unwrap().iter().find(|o| o["operation_id"] == report["operation_id"]).unwrap();
    assert_eq!((op["action"].as_str(), op["recovery"]["availability"].as_str()), (Some("maintenance"), Some("unavailable")));
    let _ = std::fs::remove_dir_all(&store);
}

/// 13.1: with elevation accepted, one worker process (here an unelevated child, no UAC) runs exactly the plan's
/// administrator tasks over the verified pipe; user-level tasks stay in the CLI process.
#[test]
fn exe08_worker_child_runs_only_the_plans_admin_tasks() {
    let store = scratch("worker");
    let script = store.join("script.json");
    std::fs::write(&script, SCRIPT).unwrap();
    let env = Env { store: &store, script: &script, elevation: Some("direct") };
    let (code, plan) = cli(&["optimize", "plan", "create", "--tasks", "windows.dns-flush,windows.trim-when-due,windows.image-health-check"], &env);
    assert_eq!(code, 0);
    approve(&store, &plan);
    let (code, report) = cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()], &env);
    assert_eq!(report["kind"], "maintenance_report", "{report}");
    assert_eq!(report["elevation"], "granted");
    assert_eq!((item(&report, "windows.trim-when-due")["outcome"].as_str(), item(&report, "windows.trim-when-due")["code"].as_str()), (Some("unchanged"), Some("not_due")), "MNT-02");
    assert_eq!((item(&report, "windows.image-health-check")["outcome"].as_str(), item(&report, "windows.image-health-check")["code"].as_str()), (Some("attention"), Some("repairable")));
    assert_eq!(item(&report, "windows.dns-flush")["elevated"], false, "user-level task ran in the CLI process");
    assert_eq!(report["all_clear"], false, "MNT-03: attention is never all clear");
    assert_eq!(code, 5);
    let _ = std::fs::remove_dir_all(&store);
}

/// The hidden worker refuses to start on foreign pipes, missing parents or non-administrator tasks (exit 2, no output).
#[test]
fn exe08_worker_entry_refuses_bad_starts() {
    let digest = "0".repeat(64);
    let cases: [&[&str]; 4] = [
        &["--pipe", r"\\evil\pipe\cleanmanager-maint-1-ab", "--parent-pid", "4", "--plan-digest", &digest, "--tasks", "windows.trim-when-due@1.0.0"],
        &["--pipe", r"\\.\pipe\cleanmanager-maint-1-abcdef", "--parent-pid", "4", "--plan-digest", &digest, "--tasks", "windows.dns-flush@1.0.0"],
        &["--pipe", r"\\.\pipe\cleanmanager-maint-1-abcdef", "--parent-pid", "4", "--plan-digest", &digest, "--tasks", "windows.trim-when-due@1.0.0"],
        &["--pipe", r"\\.\pipe\cleanmanager-maint-1-abcdef", "--parent-pid", "4", "--plan-digest", &digest, "--tasks", "windows.trim-when-due@1.0.0", "--command", "cmd.exe"],
    ];
    for args in cases {
        let out = Command::new(env!("CARGO_BIN_EXE_cleanmanager")).arg("__maintenance-worker").args(args).env("CM_MAINTENANCE_TEST_SCRIPT", "nul").output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{args:?}");
    }
    // It is not a user command either: `help` and completions do not list it.
    let store = scratch("hidden");
    let script = store.join("script.json");
    std::fs::write(&script, "{}").unwrap();
    let (_, cat) = cli(&["optimize", "list"], &Env { store: &store, script: &script, elevation: None });
    assert!(!cat.to_string().contains("__maintenance-worker"));
    let _ = std::fs::remove_dir_all(&store);
}

/// MNT-06: the diagnosis is read-only and says so; it names processes but never ends one.
#[test]
fn mnt06_diagnosis_is_read_only() {
    let store = scratch("diag");
    let script = store.join("script.json");
    std::fs::write(&script, "{}").unwrap();
    let mut sleeper = Command::new("cmd").args(["/c", "ping", "-n", "6", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
    let (code, d) = cli(&["optimize", "diagnose", "--window", "1"], &Env { store: &store, script: &script, elevation: None });
    assert_eq!(code, 0);
    assert_eq!(d["read_only"], true);
    assert!(d["findings"].is_array() && d["top_processes"].is_array());
    assert!(sleeper.try_wait().unwrap().is_none(), "a running process is left alone");
    let _ = sleeper.kill();
    let (code, err) = cli(&["optimize", "diagnose", "--window", "99"], &Env { store: &store, script: &script, elevation: None });
    assert_eq!((code, err["code"].as_str()), (2, Some("invalid_argument")));
    let _ = std::fs::remove_dir_all(&store);
}
