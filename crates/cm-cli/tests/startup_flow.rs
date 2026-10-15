//! End-to-end startup changes (parity 11.2/11.3) on a throwaway registry key: `CM_STARTUP_TEST_ROOT` points the CLI at
//! `HKCU\Software\CleanManagerTest\…` (created and deleted here with reg.exe), so real startup entries are never
//! changed; the real per-user Run key and its enable records are only queried, before and after, to prove it.
//! Receipts are written through `cm_store` as the terminal review would (it needs a TTY).
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED_RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-startup-{}-{name}", std::process::id()));
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

fn reg(args: &[&str]) -> (bool, String) {
    let out = Command::new("reg.exe").args(args).output().unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The throwaway key; deleted (with its shared parent when empty) on drop.
struct TestKey(String);

impl TestKey {
    fn new(name: &str) -> TestKey {
        let k = TestKey(format!(r"Software\CleanManagerTest\cli{}_{name}", std::process::id()));
        let _ = reg(&["delete", &format!(r"HKCU\{}", k.0), "/f"]);
        k
    }
    fn set(&self, hive: &str, name: &str, data: &str) {
        let (ok, out) = reg(&["add", &format!(r"HKCU\{}\{hive}\{RUN}", self.0), "/v", name, "/t", "REG_SZ", "/d", data, "/f"]);
        assert!(ok, "{out}");
    }
    fn query(&self, hive: &str, key: &str, name: &str) -> Option<String> {
        let (ok, out) = reg(&["query", &format!(r"HKCU\{}\{hive}\{key}", self.0), "/v", name]);
        ok.then(|| out.lines().find(|l| l.trim_start().starts_with(name)).map(|l| l.trim().to_owned()).unwrap_or_default())
    }
}

impl Drop for TestKey {
    fn drop(&mut self) {
        let _ = reg(&["delete", &format!(r"HKCU\{}", self.0), "/f"]);
        let (_, rest) = reg(&["query", r"HKCU\Software\CleanManagerTest"]);
        if !rest.lines().any(|l| l.to_ascii_lowercase().contains(r"cleanmanagertest\")) {
            let _ = reg(&["delete", r"HKCU\Software\CleanManagerTest", "/f"]);
        }
    }
}

fn real_snapshot() -> String {
    let (_, a) = reg(&["query", &format!(r"HKCU\{RUN}")]);
    let (_, b) = reg(&["query", &format!(r"HKCU\{APPROVED_RUN}")]);
    format!("{a}\n{b}")
}

fn cli(args: &[&str], store: &Path, key: &TestKey, standard_user: bool) -> (i32, Value) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cleanmanager"));
    cmd.args(args).arg("--json").env("CM_STORE_DIR", store).env("CM_STARTUP_TEST_ROOT", &key.0);
    if args != ["startup", "list"] {
        cmd.arg("--store").arg(store);
    }
    if standard_user {
        cmd.env("CM_ASSUME_STANDARD_USER", "1");
    }
    let out = cmd.output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "exactly one record: {text}");
    (out.status.code().unwrap(), serde_json::from_str(&text).unwrap())
}

fn approve(store: &Path, plan: &Value) {
    let stores = cm_store::Stores::open(store).unwrap();
    let kind: cm_core::plan::v2::PlanKind = serde_json::from_value(plan["body"]["plan_kind"].clone()).unwrap();
    stores.audit.save_kind_receipt(&cm_core::authz::KindReceipt {
        receipt_id: format!("r_{}", plan["body"]["plan_id"].as_str().unwrap()),
        plan_id: plan["body"]["plan_id"].as_str().unwrap().into(),
        plan_digest: plan["plan_digest"].as_str().unwrap().into(),
        plan_kind: kind,
        principal: cm_core::authz::current_principal(),
        source: cm_core::authz::ReceiptSource::InteractiveReview,
        created_at: cm_core::time::now_utc(),
        expires_at: plan["body"]["expires_at"].as_str().unwrap().into(),
        revoked_at: None,
    }).unwrap();
}

fn item<'a>(list: &'a Value, name: &str) -> &'a Value {
    list["items"].as_array().unwrap().iter().find(|i| i["name"] == name).unwrap_or_else(|| panic!("{name} listed: {list}"))
}

#[test]
fn startup_changes_are_reviewed_plans_with_restorable_prior_state() {
    let before = real_snapshot();
    let base = scratch("flow");
    let store = base.join("store");
    let key = TestKey::new("flow");
    let tool = base.join("tool.exe");
    std::fs::write(&tool, b"MZ").unwrap();
    key.set("HKCU", "CMFlowApp", &format!("\"{}\" --tray", tool.display()));
    key.set("HKCU", "CMFlowStale", &format!("\"{}\"", base.join("uninstalled.exe").display()));
    key.set("HKCU", "CMFlowWindows", r#""C:\Windows\System32\notepad.exe""#);
    key.set("HKLM", "CMFlowMachine", &format!("\"{}\"", tool.display()));

    let (code, list) = cli(&["startup", "list"], &store, &key, true);
    assert_eq!(code, 0, "{list}");
    assert!(list["items"].as_array().unwrap().iter().all(|i| i["definition"].as_str().unwrap().contains("CleanManagerTest")), "only the test key: {list}");
    let app = item(&list, "CMFlowApp");
    assert_eq!((app["toggle_capability"].as_str(), app["state"].as_str()), (Some("available"), Some("enabled")));
    assert_eq!(item(&list, "CMFlowStale")["state"], "broken");
    let windows = item(&list, "CMFlowWindows");
    assert_eq!((windows["protected"].as_bool(), windows["toggle_capability"].as_str()), (Some(true), Some("unsupported")));
    let machine = item(&list, "CMFlowMachine");
    assert_eq!((machine["scope"].as_str(), machine["toggle_capability"].as_str()), (Some("system"), Some("permission_required")));
    let id = |v: &Value| v["startup_item_id"].as_str().unwrap().to_owned();

    // STA-04 / STA-02: protected items have no toggle; system-wide ones need an administrator (injected non-admin).
    let (code, err) = cli(&["startup", "disable", &id(windows)], &store, &key, true);
    assert_eq!((code, err["code"].as_str()), (3, Some("policy_denied")), "{err}");
    let (code, err) = cli(&["startup", "disable", &id(machine)], &store, &key, true);
    assert_eq!((code, err["code"].as_str()), (4, Some("permission_required")), "{err}");

    // 11.2: disable is a startup plan; nothing changes until a person approves it.
    let (code, plan) = cli(&["startup", "disable", &id(app)], &store, &key, true);
    assert_eq!(code, 0, "{plan}");
    assert_eq!((plan["body"]["plan_kind"].as_str(), plan["body"]["policy_matchable"].as_bool()), (Some("startup"), Some(false)));
    let plan_id = plan["body"]["plan_id"].as_str().unwrap().to_owned();
    let (code, err) = cli(&["plan", "apply", &plan_id], &store, &key, true);
    assert_eq!((code, err["code"].as_str()), (3, Some("needs_approval")), "{err}");
    assert!(key.query("HKCU", r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run", "CMFlowApp").is_none());
    approve(&store, &plan);
    let (code, report) = cli(&["plan", "apply", &plan_id], &store, &key, true);
    assert_eq!(code, 0, "{report}");
    let record = key.query("HKCU", r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run", "CMFlowApp").unwrap();
    assert!(record.contains("REG_BINARY") && record.split_whitespace().last().unwrap().starts_with("03"), "Task Manager's disabled record: {record}");
    let (_, list) = cli(&["startup", "list"], &store, &key, true);
    assert_eq!(item(&list, "CMFlowApp")["state"], "disabled");
    let op = report["operation_id"].as_str().unwrap().to_owned();
    let (_, history) = cli(&["operations", "list"], &store, &key, true);
    assert_eq!((history["items"][0]["action"].as_str(), history["items"][0]["recovery"]["availability"].as_str()), (Some("startup"), Some("in_app_restore")));

    // The recorded prior state comes back through a reviewed recovery plan.
    let (code, rplan) = cli(&["startup", "restore", "--operation", &op], &store, &key, true);
    assert_eq!((code, rplan["body"]["plan_kind"].as_str()), (0, Some("recovery")), "{rplan}");
    approve(&store, &rplan);
    let (code, report) = cli(&["plan", "apply", rplan["body"]["plan_id"].as_str().unwrap()], &store, &key, true);
    assert_eq!(code, 0, "{report}");
    assert!(key.query("HKCU", r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run", "CMFlowApp").is_none(), "no record, as before");
    let (_, list) = cli(&["startup", "list"], &store, &key, true);
    assert_eq!(item(&list, "CMFlowApp")["state"], "enabled");

    // 11.3: a stale Run value goes only after its backup; its restore never overwrites a value recreated since.
    let stale = id(item(&list, "CMFlowStale"));
    let (code, plan) = cli(&["startup", "remove-stale", &stale], &store, &key, true);
    assert_eq!(code, 0, "{plan}");
    approve(&store, &plan);
    let (code, report) = cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()], &store, &key, true);
    assert_eq!(code, 0, "{report}");
    assert!(key.query("HKCU", RUN, "CMFlowStale").is_none(), "removed");
    let (code, rplan) = cli(&["startup", "restore", "--operation", report["operation_id"].as_str().unwrap()], &store, &key, true);
    assert_eq!(code, 0, "{rplan}");
    assert_eq!(rplan["body"]["items"][0]["action_type"], "registry_restore");
    key.set("HKCU", "CMFlowStale", r#""C:\Somewhere\new.exe""#);
    approve(&store, &rplan);
    let (code, report) = cli(&["plan", "apply", rplan["body"]["plan_id"].as_str().unwrap()], &store, &key, true);
    assert_eq!((code, report["items"][0]["code"].as_str()), (5, Some("destination_exists")), "{report}");
    assert!(key.query("HKCU", RUN, "CMFlowStale").unwrap().contains(r"C:\Somewhere\new.exe"), "the recreated value is kept");

    drop(key);
    let _ = std::fs::remove_dir_all(&base);
    assert_eq!(real_snapshot(), before, "real Run values and enable records are untouched");
}
