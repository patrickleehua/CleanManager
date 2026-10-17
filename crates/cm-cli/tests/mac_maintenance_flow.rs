//! macOS system maintenance through the real binary (parity 14.3/14.5; MNT-01..05/07, PRT-06).
//!
//! - Scripted runs (`CM_MAINTENANCE_TEST_SCRIPT`) touch nothing on the machine.
//! - Real runs read a fixture home only (`CM_RESOLVE_HOME`: broken preferences, saved states of missing apps, stale
//!   LaunchAgents) and change the network `.DS_Store` setting in an absolute-path preference domain inside the fixture
//!   (`CM_MAINTENANCE_TEST_PREFS_DIR`), never the user's `com.apple.desktopservices`. `mdutil -s /` is read-only. The
//!   Quick Look reset runs against the runner's own cache only with `CM_ALLOW_MAC_DEVICE_TESTS=1` or on a hosted runner.
//! - There is no privilege path on macOS: administrator tasks are always skipped `elevation_unavailable` (MNT-04).
//!
//! Receipts are written through `cm_store` as the terminal review would (it needs a TTY).
#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn device_tests_allowed() -> bool {
    std::env::var("CM_ALLOW_MAC_DEVICE_TESTS").is_ok_and(|v| v == "1") || std::env::var("GITHUB_ACTIONS").is_ok_and(|v| v == "true")
}

fn scratch(name: &str, experimental: bool) -> PathBuf {
    let base = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let p = base.join(format!("cm-cli-mac-maint-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(p.join("store")).unwrap();
    if experimental {
        let o = Command::new(env!("CARGO_BIN_EXE_cleanmanager")).args(["experimental", "--state", "on", "--json"]).env("CM_STORE_DIR", p.join("store")).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stdout));
    }
    p
}

fn schema() -> Value {
    serde_json::from_str(include_str!("../../../schemas/v1/protocol.schema.json")).unwrap()
}

/// The record carries every required top-level field of its kind and nothing the schema does not define.
fn check_shape(rec: &Value) {
    let s = schema();
    let kind = rec["kind"].as_str().unwrap();
    let def = &s["$defs"][kind];
    let props = def["properties"].as_object().unwrap_or_else(|| panic!("schema has no record {kind}"));
    let got = rec.as_object().unwrap();
    for k in got.keys() {
        assert!(props.contains_key(k), "{kind}: field {k} is not in the schema");
    }
    for k in def["required"].as_array().into_iter().flatten() {
        assert!(got.contains_key(k.as_str().unwrap()), "{kind}: required field {k} missing");
    }
}

#[derive(Default)]
struct Env {
    store: PathBuf,
    script: Option<PathBuf>,
    home: Option<PathBuf>,
    prefs: Option<PathBuf>,
}

fn cli(args: &[&str], env: &Env) -> (i32, Value) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cleanmanager"));
    cmd.args(args).arg("--json").env("CM_STORE_DIR", &env.store);
    for v in ["CM_MAINTENANCE_TEST_SCRIPT", "CM_MAINTENANCE_TEST_ELEVATION", "CM_RESOLVE_HOME", "CM_MAINTENANCE_TEST_PREFS_DIR"] {
        cmd.env_remove(v);
    }
    if let Some(s) = &env.script {
        // The elevation variable has no effect on macOS (no worker exists); set to show it changes nothing.
        cmd.env("CM_MAINTENANCE_TEST_SCRIPT", s).env("CM_MAINTENANCE_TEST_ELEVATION", "direct");
    }
    if let Some(h) = &env.home {
        cmd.env("CM_RESOLVE_HOME", h);
    }
    if let Some(p) = &env.prefs {
        cmd.env("CM_MAINTENANCE_TEST_PREFS_DIR", p);
    }
    let out = cmd.output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "exactly one record: {text} {}", String::from_utf8_lossy(&out.stderr));
    let v: Value = serde_json::from_str(&text).unwrap();
    if v["kind"] != "error" && v["kind"] != "plan" {
        check_shape(&v);
    }
    (out.status.code().unwrap(), v)
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
            created_at: plan["body"]["created_at"].as_str().unwrap().into(),
            expires_at: plan["body"]["expires_at"].as_str().unwrap().into(),
            revoked_at: None,
        })
        .unwrap();
}

fn item<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["items"].as_array().unwrap().iter().find(|i| i["task_id"] == id).unwrap_or_else(|| panic!("{id} in {report}"))
}

fn outcome(report: &Value, id: &str) -> (String, String) {
    let i = item(report, id);
    (i["outcome"].as_str().unwrap().to_owned(), i["code"].as_str().unwrap().to_owned())
}

const ADMIN: [&str; 4] = ["macos.icon-cache-rebuild", "macos.dns-flush", "macos.disk-verify", "macos.spotlight-rebuild"];

/// D15 gate, MNT-01 catalog shape, MNT-05 prohibited search, and the macOS elevation mode.
#[test]
fn macos_catalog_shape_gate_and_prohibited_search() {
    let dir = scratch("catalog-off", false);
    let off = Env { store: dir.join("store"), ..Default::default() };
    let (code, err) = cli(&["optimize", "list"], &off);
    assert_ne!(code, 0, "gated tools are off by default: {err}");
    assert_eq!(err["kind"], "error");

    let dir = scratch("catalog", true);
    let env = Env { store: dir.join("store"), home: Some(dir.join("home")), prefs: Some(dir.clone()), ..Default::default() };
    let (code, cat) = cli(&["optimize", "list"], &env);
    assert_eq!(code, 0, "{cat}");
    assert_eq!((cat["platform"].as_str(), cat["elevation"].as_str()), (Some("macos"), Some("unavailable")));
    let items = cat["items"].as_array().unwrap();
    assert_eq!(items.len(), 10, "{cat}");
    for i in items {
        assert!(i["task_id"].as_str().unwrap().starts_with("macos.") && i["problem"].is_string() && i["impact"].is_string() && i["requires_admin"].is_boolean());
        assert_eq!(i["reversible"], i["task_id"] == "macos.network-dsstore-off", "{i}");
        assert_eq!(i["requires_admin"], ADMIN.contains(&i["task_id"].as_str().unwrap()), "{i}");
    }
    assert_eq!(item(&cat, "macos.broken-preferences")["applicability"], "not_applicable", "no fixture home folders yet");
    assert_eq!(item(&cat, "macos.network-dsstore-off")["applicability"], "applicable");
    let (code, found) = cli(&["optimize", "list", "--search", "注册表优化"], &env);
    assert_eq!(code, 0);
    assert!(found["items"].as_array().unwrap().is_empty());
    assert!(found["prohibited"].as_array().unwrap().iter().any(|x| x["id"] == "registry-optimizer"));
    let (code, found) = cli(&["optimize", "list", "--search", "launchservices"], &env);
    assert_eq!(code, 0);
    assert!(found["prohibited"].as_array().unwrap().iter().any(|x| x["id"] == "rebuild-app-registrations"), "MNT-05: no LaunchServices rebuild");
    let (code, err) = cli(&["optimize", "plan", "create", "--tasks", "windows.dns-flush"], &env);
    assert_eq!((code, err["code"].as_str()), (7, Some("unsupported")), "{err}");
    let (code, err) = cli(&["optimize", "plan", "create", "--tasks", "macos.spotlight-rebuild"], &env);
    assert_eq!(err["code"], "invalid_argument", "a volume task needs --volume: {err}");
    assert_ne!(code, 0);
    let (_, err) = cli(&["optimize", "plan", "create", "--tasks", "macos.spotlight-rebuild", "--volume", "/usr/bin"], &env);
    assert_eq!(err["code"], "invalid_argument", "not a mount point: {err}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// MNT-04: scripted tasks (nothing touched); every administrator task is skipped `elevation_unavailable`, even with the
/// test elevation variable set, and the user-level tasks still run.
#[test]
fn mnt04_admin_tasks_are_skipped_and_the_rest_continue() {
    let dir = scratch("scripted", true);
    let script = dir.join("script.json");
    std::fs::write(&script, r#"{"macos.spotlight-status": {"outcome": "attention", "code": "indexing_disabled", "reason": "scripted"}}"#).unwrap();
    let env = Env { store: dir.join("store"), script: Some(script), home: Some(dir.join("home")), prefs: Some(dir.clone()) };
    let tasks = "macos.quicklook-reset,macos.icon-cache-rebuild,macos.dns-flush,macos.spotlight-status,macos.disk-verify";
    let (code, plan) = cli(&["optimize", "plan", "create", "--tasks", tasks], &env);
    assert_eq!(code, 0, "{plan}");
    assert_eq!((plan["body"]["plan_kind"].as_str(), plan["body"]["requires_admin"].as_bool()), (Some("maintenance"), Some(true)));
    approve(&env.store, &plan);
    let (code, report) = cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()], &env);
    assert_eq!(report["kind"], "maintenance_report", "{report}");
    assert_eq!(code, 5, "partial");
    assert_eq!(report["elevation"], "unavailable");
    for id in ["macos.icon-cache-rebuild", "macos.dns-flush", "macos.disk-verify"] {
        assert_eq!(outcome(&report, id), ("skipped".into(), "elevation_unavailable".into()), "{id}");
        assert_eq!(item(&report, id)["elevated"], false);
    }
    assert_eq!(outcome(&report, "macos.quicklook-reset").0, "applied");
    assert_eq!(outcome(&report, "macos.spotlight-status"), ("attention".into(), "indexing_disabled".into()));
    assert_eq!(report["all_clear"], false);
    assert_eq!(report["counts"]["skipped"], 3);
    let _ = std::fs::remove_dir_all(&dir);
}

fn xml_plist(body: &str) -> String {
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">{body}</plist>\n")
}

fn write(p: &Path, text: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// The real tasks on a fixture home and a fixture preference domain, then the setting's restore through a reviewed
/// recovery plan (14.3/14.5, MNT-03/07). The report is printed so the CI log keeps the per-task record.
#[test]
fn real_tasks_on_a_fixture_home_and_the_setting_restore() {
    let dir = scratch("real", true);
    let home = dir.join("home");
    let prefs_dir = dir.join("prefs");
    std::fs::create_dir_all(&prefs_dir).unwrap();
    write(&home.join("Library/Preferences/com.example.good.plist"), xml_plist("<dict><key>A</key><integer>1</integer></dict>").as_bytes());
    write(&home.join("Library/Preferences/com.example.broken.plist"), b"<?xml version=\"1.0\"?><plist><dict><key>A</key>");
    write(&home.join("Library/Saved Application State/com.apple.finder.savedState/data.data"), b"x");
    write(&home.join("Library/Saved Application State/com.cleanmanager.test.removed.savedState/data.data"), b"x");
    write(
        &home.join("Library/LaunchAgents/com.example.gone.plist"),
        xml_plist("<dict><key>Label</key><string>com.example.gone</string><key>Program</key><string>/Applications/Gone.app/Contents/MacOS/gone</string></dict>").as_bytes(),
    );
    write(&home.join("Library/LaunchAgents/com.example.ok.plist"), xml_plist("<dict><key>Label</key><string>com.example.ok</string><key>Program</key><string>/usr/bin/true</string></dict>").as_bytes());
    let env = Env { store: dir.join("store"), home: Some(home.clone()), prefs: Some(prefs_dir.clone()), ..Default::default() };

    let mut tasks = vec![
        "macos.dns-flush",
        "macos.spotlight-status",
        "macos.broken-preferences",
        "macos.stale-saved-state",
        "macos.disk-verify",
        "macos.launch-agents-report",
        "macos.network-dsstore-off",
        "macos.spotlight-rebuild",
    ];
    if device_tests_allowed() {
        tasks.insert(0, "macos.quicklook-reset");
    }
    let joined = tasks.join(",");
    let (code, plan) = cli(&["optimize", "plan", "create", "--tasks", &joined, "--volume", "/"], &env);
    assert_eq!(code, 0, "{plan}");
    let setting = plan["body"]["items"].as_array().unwrap().iter().find(|a| a["action_type"] == "settings_change").expect("a settings_change action").clone();
    assert_eq!((setting["from_value"].as_str(), setting["to_value"].as_str()), (Some("absent"), Some("true")), "{setting}");
    let rebuild = plan["body"]["items"].as_array().unwrap().iter().find(|a| a["task_id"] == "macos.spotlight-rebuild").unwrap();
    assert_eq!(rebuild["volume"], "/");
    approve(&env.store, &plan);
    let before = std::fs::read(home.join("Library/Preferences/com.example.broken.plist")).unwrap();
    let (code, report) = cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()], &env);
    eprintln!("maintenance_report on this host: {}", serde_json::to_string_pretty(&report).unwrap());
    assert_eq!(report["kind"], "maintenance_report", "{report}");
    assert_eq!(code, 5, "partial: admin tasks skipped, report tasks need attention");
    for id in ADMIN.iter().filter(|id| **id != "macos.icon-cache-rebuild") {
        assert_eq!(outcome(&report, id), ("skipped".into(), "elevation_unavailable".into()), "{id}");
    }
    assert_eq!(outcome(&report, "macos.broken-preferences"), ("attention".into(), "broken_preferences".into()));
    assert!(item(&report, "macos.broken-preferences")["reason"].as_str().unwrap().contains("com.example.broken.plist"));
    assert_eq!(std::fs::read(home.join("Library/Preferences/com.example.broken.plist")).unwrap(), before, "report only");
    assert_eq!(outcome(&report, "macos.stale-saved-state"), ("attention".into(), "stale_saved_state".into()));
    assert!(home.join("Library/Saved Application State/com.cleanmanager.test.removed.savedState").is_dir(), "report only");
    assert_eq!(outcome(&report, "macos.launch-agents-report"), ("attention".into(), "stale_launch_agents".into()));
    assert!(matches!(outcome(&report, "macos.spotlight-status").0.as_str(), "unchanged" | "attention"));
    assert_eq!(outcome(&report, "macos.network-dsstore-off"), ("applied".into(), "set".into()));
    if device_tests_allowed() {
        assert_eq!(outcome(&report, "macos.quicklook-reset"), ("applied".into(), "reset".into()));
    }
    // The fixture domain (never the user's) now has the value.
    let domain_file = prefs_dir.join("com.apple.desktopservices.plist");
    let text = std::fs::read(&domain_file).unwrap();
    assert!(text.windows(24).any(|w| w == b"DSDontWriteNetworkStores"), "written to the fixture domain");
    let op = report["operation_id"].as_str().unwrap().to_owned();
    let (_, ops) = cli(&["operations", "list"], &env);
    let row = ops["items"].as_array().unwrap().iter().find(|o| o["operation_id"] == op.as_str()).unwrap().clone();
    assert_eq!((row["action"].as_str(), row["recovery"]["availability"].as_str()), (Some("maintenance"), Some("in_app_restore")), "{row}");

    // MNT-07 / 14.5: restore the backed-up prior value through a reviewed recovery plan.
    let (code, rp) = cli(&["optimize", "restore", "--operation", &op], &env);
    assert_eq!(code, 0, "{rp}");
    assert_eq!(rp["body"]["plan_kind"], "recovery");
    let back = &rp["body"]["items"][0];
    assert_eq!((back["action_type"].as_str(), back["from_value"].as_str(), back["to_value"].as_str()), (Some("settings_change"), Some("true"), Some("absent")));
    let (_, err) = cli(&["plan", "apply", rp["body"]["plan_id"].as_str().unwrap()], &env);
    assert_eq!(err["code"], "needs_approval", "{err}");
    approve(&env.store, &rp);
    let (code, done) = cli(&["plan", "apply", rp["body"]["plan_id"].as_str().unwrap()], &env);
    eprintln!("restore report on this host: {done}");
    assert_eq!(code, 0, "{done}");
    assert_eq!((done["items"][0]["status"].as_str(), done["items"][0]["code"].as_str()), (Some("succeeded"), Some("restored")), "{done}");
    let (_, cat) = cli(&["optimize", "list"], &env);
    assert_eq!(item(&cat, "macos.network-dsstore-off")["applicability"], "applicable", "the key is unset again");
    let (code, again) = cli(&["optimize", "restore", "--operation", &op], &env);
    assert_ne!(code, 0, "nothing left to restore: {again}");
    let (_, hist) = cli(&["optimize", "history"], &env);
    assert_eq!(hist["items"][0]["operation_id"], op.as_str());
    let _ = std::fs::remove_dir_all(&dir);
}

/// MNT-06 on macOS: the diagnosis samples processes, memory, volumes, startup items and attached images, and only reads:
/// a running process of this user is left alone.
#[test]
fn mnt06_diagnosis_is_read_only() {
    let dir = scratch("diag", true);
    let env = Env { store: dir.join("store"), ..Env::default() };
    let mut sleeper = Command::new("/bin/sleep").arg("60").spawn().unwrap();
    let (code, d) = cli(&["optimize", "diagnose", "--window", "1"], &env);
    assert_eq!(code, 0, "{d}");
    assert_eq!(d["kind"], "maintenance_diagnosis");
    assert_eq!(d["read_only"], true);
    assert!(d["findings"].is_array() && d["top_processes"].is_array(), "{d}");
    assert!(sleeper.try_wait().unwrap().is_none(), "a running process is left alone");
    let _ = sleeper.kill();
    let _ = sleeper.wait();
    let (code, err) = cli(&["optimize", "diagnose", "--window", "99"], &env);
    assert_eq!((code, err["code"].as_str()), (2, Some("invalid_argument")));
    let _ = std::fs::remove_dir_all(&dir);
}
