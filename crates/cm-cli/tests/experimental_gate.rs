//! Design D15 through the real binary: a capability whose release gate has not passed is off by default. Every
//! gated command group, and `plan apply|review` of a stored plan/v2 of a gated kind, refuses with one `unsupported`
//! record before doing any work; `experimental --state on|off` is the one opt-in, kept in the shared store.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-gate-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn run(store: &Path, args: &[&str]) -> (i32, Value) {
    let o = Command::new(env!("CARGO_BIN_EXE_cleanmanager")).args(args).arg("--json").env("CM_STORE_DIR", store).env_remove("CLEANMANAGER_LANG").output().unwrap();
    let out = String::from_utf8(o.stdout).unwrap();
    assert_eq!(out.lines().count(), 1, "exactly one record: {out}");
    (o.status.code().unwrap_or(-1), serde_json::from_str(out.trim_end()).unwrap())
}

fn assert_off(code: i32, e: &Value, capability: &str, gate: &str) {
    assert_eq!((code, e["kind"].as_str(), e["code"].as_str()), (7, Some("error"), Some("unsupported")), "{e}");
    let msg = e["message"].as_str().unwrap();
    assert!(msg.contains(capability) && msg.contains(&format!("gate {gate}")) && msg.contains("cleanmanager experimental --state on"), "{msg}");
    assert_eq!(e["required_action"], "cleanmanager experimental --state on");
    assert_eq!(e["retryable"], false);
}

/// One command per gated group; each would read or write something if it ran.
const GATED: &[(&[&str], &str, &str)] = &[
    (&["clean", "list"], "clean.catalog", "G1"),
    (&["purge", "roots"], "purge.artifacts", "G1"),
    (&["installer", "locations"], "installer.cleanup", "G1"),
    (&["trash", "list"], "trash.manual", "G1"),
    (&["insights"], "insights.overview", "G1"),
    (&["insights", "overview"], "insights.overview", "G1"),
    (&["apps", "uninstall", "list"], "apps.uninstall", "G3"),
    (&["apps", "updates", "show"], "apps.updates", "G3"),
    (&["apps", "updates", "config", "--checks", "off"], "apps.updates", "G3"),
    (&["apps", "update", "--app", "app_x"], "apps.updates", "G3"),
    (&["startup", "list"], "startup.toggle", "G3"),
    (&["startup", "disable", "st_x"], "startup.toggle", "G3"),
    (&["optimize", "list"], "maintenance.run", "G3"),
    (&["optimize", "diagnose"], "maintenance.run", "G3"),
    (&["status"], "status.metrics", "G3"),
    (&["status", "alerts", "--state", "on"], "status.metrics", "G3"),
    (&["process", "list"], "process.end", "G3"),
    (&["process", "end", "4"], "process.end", "G3"),
    (&["self", "update", "--check"], "lifecycle.self_update", "G3"),
    (&["diag", "config"], "diag.ai", "AID"),
    (&["diag", "list"], "diag.ai", "AID"),
];

#[test]
fn every_gated_command_group_refuses_by_default_before_any_work() {
    let s = scratch("off");
    for (args, cap, gate) in GATED {
        let (code, e) = run(&s, args);
        assert_off(code, &e, cap, gate);
    }
    // Nothing was written: no settings file of any gated tool, and the switch itself was never created.
    let names: Vec<String> = std::fs::read_dir(&s).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(names.is_empty(), "{names:?}");
    // Core commands are not gated.
    let (code, caps) = run(&s, &["capabilities"]);
    assert_eq!((code, caps["kind"].as_str()), (0, Some("capabilities")));
    let (code, prot) = run(&s, &["protect", "list"]);
    assert_eq!((code, prot["kind"].as_str()), (0, Some("protection_list")), "{prot}");
    let (code, ops) = run(&s, &["operations", "list"]);
    assert_eq!(code, 0, "{ops}");
    let _ = std::fs::remove_dir_all(&s);
}

#[test]
fn the_switch_is_one_local_setting_shared_with_the_desktop() {
    let s = scratch("switch");
    let (code, show) = run(&s, &["experimental"]);
    assert_eq!((code, show["kind"].as_str(), show["experimental_features"].as_bool()), (0, Some("experimental_features"), Some(false)));
    let rows = show["capabilities"].as_array().unwrap();
    assert_eq!(rows.len(), 14);
    assert!(rows.iter().all(|r| r["gate_passed"] == false && r["enabled"] == false));
    let (code, e) = run(&s, &["experimental", "--state", "maybe"]);
    assert_eq!((code, e["code"].as_str()), (2, Some("invalid_argument")));

    let (code, on) = run(&s, &["experimental", "--state", "on"]);
    assert_eq!((code, on["experimental_features"].as_bool()), (0, Some(true)));
    assert!(on["capabilities"].as_array().unwrap().iter().all(|r| r["enabled"] == true && r["gate_passed"] == false));
    // Persisted in the store the desktop reads through the same service.
    let (_, again) = run(&s, &["experimental"]);
    assert_eq!(again["experimental_features"], true);
    assert!(cm_app::features::load(Some(&s)).experimental_features);
    assert!(cm_app::features::enabled(Some(&s), "process.end"));
    let (code, cfg) = run(&s, &["apps", "updates", "config"]);
    assert_eq!((code, cfg["kind"].as_str()), (0, Some("app_update_settings")), "{cfg}");

    let (code, off) = run(&s, &["experimental", "--state", "off"]);
    assert_eq!((code, off["experimental_features"].as_bool()), (0, Some(false)));
    let (code, e) = run(&s, &["apps", "updates", "config"]);
    assert_off(code, &e, "apps.updates", "G3");
    let _ = std::fs::remove_dir_all(&s);
}

#[test]
fn capabilities_report_gated_tools_off_and_never_available() {
    let s = scratch("caps");
    let gated = |v: &Value| -> Vec<Value> {
        v["capabilities"].as_array().unwrap().iter().filter(|c| cm_core::capability::gate_of(c["name"].as_str().unwrap()).is_some()).cloned().collect()
    };
    let (_, off) = run(&s, &["capabilities"]);
    let rows = gated(&off);
    assert_eq!(rows.len(), 14);
    for c in &rows {
        assert_eq!(c["state"], "unsupported", "{c}");
        if cfg!(windows) {
            let gate = cm_core::capability::gate_of(c["name"].as_str().unwrap()).unwrap().gate;
            assert_eq!(c["reason"].as_str().unwrap(), format!("off until gate {gate} passes; experimental features are disabled"));
        }
    }
    run(&s, &["experimental", "--state", "on"]);
    let (_, on) = run(&s, &["capabilities"]);
    for c in gated(&on) {
        assert_eq!(c["state"], if cfg!(windows) { "unverified" } else { "unsupported" }, "{c}");
    }
    assert!(on["capabilities"].as_array().unwrap().iter().all(|c| c["state"] != "available"));
    // Core capabilities do not change with the switch.
    let core = |v: &Value| v["capabilities"].as_array().unwrap().iter().filter(|c| cm_core::capability::gate_of(c["name"].as_str().unwrap()).is_none()).cloned().collect::<Vec<_>>();
    assert_eq!(core(&off), core(&on));
    let _ = std::fs::remove_dir_all(&s);
}

/// A plan/v2 of a gated kind, created while the switch was on, cannot be reviewed or applied once it is off.
#[cfg(windows)]
#[test]
fn a_stored_plan_of_a_gated_kind_refuses_review_and_apply() {
    let s = scratch("plan");
    run(&s, &["experimental", "--state", "on"]);
    let mut child = Command::new(r"C:\Windows\System32\PING.EXE").args(["-n", "60", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
    let (code, plan) = run(&s, &["process", "end", &child.id().to_string()]);
    child.kill().unwrap();
    let _ = child.wait();
    assert_eq!((code, plan["body"]["plan_kind"].as_str()), (0, Some("process")), "{plan}");
    let id = plan["body"]["plan_id"].as_str().unwrap().to_owned();

    run(&s, &["experimental", "--state", "off"]);
    for verb in ["apply", "review"] {
        let (code, e) = run(&s, &["plan", verb, &id]);
        assert_off(code, &e, "process.end", "G3");
        assert_eq!(e["details"]["plan_id"], id.as_str());
    }
    // `plan show` stays available: the stored plan can still be inspected and revoked.
    let (code, shown) = run(&s, &["plan", "show", &id]);
    assert_eq!(code, 0, "{shown}");
    // Unknown plans are reported by the command itself.
    let (code, e) = run(&s, &["plan", "apply", "p_none"]);
    assert_ne!(e["code"], "unsupported", "{e}");
    assert_ne!(code, 0);
    let _ = std::fs::remove_dir_all(&s);
}
