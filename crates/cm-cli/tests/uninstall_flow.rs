//! End-to-end application uninstall (parity 9.1–9.8) on fixtures only: the registry is redirected to a throwaway key
//! (`CM_UNINSTALL_TEST_ROOT` / `CM_STARTUP_TEST_ROOT` under `HKCU\Software\CleanManagerTest\…`, created and deleted
//! here with reg.exe) and the home folder to a scratch directory (`CM_UNINSTALL_TEST_HOME`). The "vendor uninstaller"
//! and the "running app" are copies of the system's cmd.exe in the scratch install folder, started with fixture
//! arguments; no real application is uninstalled and the real uninstall and Run keys are only queried, before and
//! after, to prove they are unchanged. Receipts are written through `cm_store` as the terminal review would. Leftovers
//! moved to the Recycle Bin by the run are removed from it again by a guard matching the bin's own records, so a run
//! leaves the user's bin as it found it.
#![cfg(windows)]

use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

/// Removes from the user's Recycle Bin whatever this test moved there (matched by the bin's own `$I` records).
struct BinGuard(String);

impl BinGuard {
    /// Bin records (`$I` file, original path) of this test's scratch folder.
    fn ours(&self) -> Vec<(PathBuf, String)> {
        use cm_platform::windows::actions::recycle_bin::{parse_info, user_bin_dirs};
        let mut out = Vec::new();
        for dir in user_bin_dirs() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                if !e.file_name().to_string_lossy().starts_with("$I") {
                    continue;
                }
                if let Some((orig, _, _)) = std::fs::read(e.path()).ok().and_then(|d| parse_info(&d))
                    && orig.to_ascii_lowercase().contains(&self.0)
                {
                    out.push((e.path(), orig));
                }
            }
        }
        out
    }
}

impl Drop for BinGuard {
    fn drop(&mut self) {
        for (info, _) in self.ours() {
            let name = info.file_name().unwrap().to_string_lossy().replacen("$I", "$R", 1);
            let r = info.with_file_name(name);
            let _ = if r.is_dir() { std::fs::remove_dir_all(&r) } else { std::fs::remove_file(&r) };
            let _ = std::fs::remove_file(&info);
        }
    }
}

/// A `.lnk` written by the Windows Script Host (the CLI never writes links).
fn shortcut(link: &Path, target: &Path) {
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let script = format!("$s = (New-Object -ComObject WScript.Shell).CreateShortcut('{}'); $s.TargetPath = '{}'; $s.Save()", link.display(), target.display());
    let ok = Command::new(format!(r"{sys}\System32\WindowsPowerShell\v1.0\powershell.exe")).args(["-NoProfile", "-NonInteractive", "-Command", &script]).status().unwrap();
    assert!(ok.success() && link.is_file(), "{}", link.display());
}

const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-uninst-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn reg(args: &[&str]) -> (bool, String) {
    let out = Command::new("reg.exe").args(args).output().unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned())
}

struct TestKey(String);

impl TestKey {
    fn new(name: &str) -> TestKey {
        let k = TestKey(format!(r"Software\CleanManagerTest\cliu{}_{name}", std::process::id()));
        let _ = reg(&["delete", &format!(r"HKCU\{}", k.0), "/f"]);
        k
    }
    fn set(&self, hive: &str, key: &str, name: &str, data: &str) {
        let (ok, out) = reg(&["add", &format!(r"HKCU\{}\{hive}\{key}", self.0), "/v", name, "/t", "REG_SZ", "/d", data, "/f"]);
        assert!(ok, "{out}");
    }
    fn exists(&self, hive: &str, key: &str) -> bool {
        reg(&["query", &format!(r"HKCU\{}\{hive}\{key}", self.0)]).0
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
    let (_, a) = reg(&["query", &format!(r"HKCU\{UNINSTALL}")]);
    let (_, b) = reg(&["query", &format!(r"HKLM\{UNINSTALL}")]);
    let (_, c) = reg(&["query", &format!(r"HKCU\{RUN}")]);
    format!("{a}\n{b}\n{c}")
}

struct Fixture {
    key: TestKey,
    home: PathBuf,
    store: PathBuf,
}

impl Fixture {
    fn cli(&self, args: &[&str]) -> (i32, Value) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cleanmanager"));
        cmd.args(args)
            .arg("--json")
            .env("CM_STORE_DIR", &self.store)
            .env("CM_UNINSTALL_TEST_ROOT", &self.key.0)
            .env("CM_STARTUP_TEST_ROOT", &self.key.0)
            .env("CM_STARTUP_TEST_FOLDER", self.home.join(r"AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup"))
            .env("CM_UNINSTALL_TEST_HOME", &self.home)
            .env("CM_UNINSTALL_QUIT_WAIT_MS", "300")
            .stdin(Stdio::null());
        let out = cmd.output().unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        assert_eq!(text.lines().count(), 1, "exactly one record: {text}");
        (out.status.code().unwrap(), serde_json::from_str(&text).unwrap())
    }
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

fn cmd_copy(to: &Path) {
    let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    std::fs::copy(format!(r"{sys}\System32\cmd.exe"), to).unwrap();
}

/// The record's kind is in schemas/v1 and it carries exactly the declared top-level fields (as cli_contract checks).
fn envelope(v: &Value) {
    let schema: Value = serde_json::from_str(include_str!("../../../schemas/v1/protocol.schema.json")).unwrap();
    let kind = v["kind"].as_str().unwrap();
    let def = &schema["$defs"][kind];
    assert!(def.is_object(), "kind {kind} is not in schemas/v1");
    let props = def["properties"].as_object().unwrap();
    assert!(v.as_object().unwrap().keys().all(|k| props.contains_key(k)), "{kind}: undeclared field in {v}");
    assert!(def["required"].as_array().unwrap().iter().all(|r| v.get(r.as_str().unwrap()).is_some()), "{kind}: missing field in {v}");
}

fn all_items(v: &Value) -> Vec<Value> {
    v["groups"].as_array().unwrap().iter().flat_map(|g| g["items"].as_array().unwrap().clone()).collect()
}

#[test]
fn two_stage_uninstall_with_fixture_uninstaller_leftovers_and_restore() {
    let before = real_snapshot();
    let base = scratch("flow");
    let bin = BinGuard(base.file_name().unwrap().to_string_lossy().to_ascii_lowercase());
    let fx = Fixture { key: TestKey::new("flow"), home: base.join("home"), store: base.join("store") };
    let install = fx.home.join(r"Programs\FooApp");
    std::fs::create_dir_all(install.join("payload")).unwrap();
    std::fs::write(install.join(r"payload\data.bin"), b"payload").unwrap();
    cmd_copy(&install.join("uninst.exe"));
    cmd_copy(&install.join("fooapp.exe"));
    let roaming = fx.home.join(r"AppData\Roaming");
    for d in [roaming.join(r"FooApp\Cache"), roaming.join(r"FooApp\models"), fx.home.join(r"AppData\Local\FooApp\logs")] {
        std::fs::create_dir_all(&d).unwrap();
    }
    std::fs::write(roaming.join(r"FooApp\Cache\c.bin"), b"cache").unwrap();
    std::fs::write(roaming.join(r"FooApp\models\m.gguf"), vec![0u8; 4096]).unwrap();
    std::fs::write(roaming.join(r"FooApp\settings.json"), b"{}").unwrap();
    std::fs::write(fx.home.join(r"AppData\Local\FooApp\logs\l.log"), b"log").unwrap();
    // The fixture uninstaller (cmd.exe copy, catalog-signed by Microsoft) removes only the payload, like a vendor
    // uninstaller that leaves its folder behind.
    let k = format!(r"{UNINSTALL}\FooApp");
    fx.key.set("HKCU", &k, "DisplayName", "FooApp");
    fx.key.set("HKCU", &k, "Publisher", "Microsoft Corporation");
    fx.key.set("HKCU", &k, "InstallLocation", &install.to_string_lossy());
    fx.key.set("HKCU", &k, "UninstallString", &format!("\"{}\" /c rmdir /s /q \"{}\"", install.join("uninst.exe").display(), install.join("payload").display()));
    fx.key.set("HKCU", r"SOFTWARE\FooApp", "Theme", "dark");
    fx.key.set("HKCU", RUN, "FooTray", &format!("\"{}\"", install.join("tray.exe").display()));
    // An unsigned uninstaller and an app without any registered uninstaller.
    let other = fx.home.join(r"Programs\Unsigned");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("uninst.exe"), b"MZ fixture, not signed").unwrap();
    fx.key.set("HKCU", &format!(r"{UNINSTALL}\Unsigned"), "DisplayName", "Unsigned Thing");
    fx.key.set("HKCU", &format!(r"{UNINSTALL}\Unsigned"), "Publisher", "Contoso");
    fx.key.set("HKCU", &format!(r"{UNINSTALL}\Unsigned"), "UninstallString", &format!("\"{}\"", other.join("uninst.exe").display()));
    fx.key.set("HKCU", &format!(r"{UNINSTALL}\Mystery"), "DisplayName", "Mystery Tool");
    // An npm-global AI CLI (UNI-07): recognized with version and last use from its data folder.
    let pkg = fx.home.join(r"AppData\Roaming\npm\node_modules\@openai\codex");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(pkg.join("package.json"), br#"{"name": "@openai/codex", "version": "0.9.1"}"#).unwrap();
    std::fs::create_dir_all(fx.home.join(r".codex\sessions")).unwrap();
    // UNI-09 teardown objects: Start menu and desktop shortcuts to the app; UNI-05: a git config naming its program.
    let menu_link = fx.home.join(r"AppData\Roaming\Microsoft\Windows\Start Menu\Programs\FooApp\FooApp.lnk");
    let desk_link = fx.home.join(r"Desktop\FooApp.lnk");
    shortcut(&menu_link, &install.join("fooapp.exe"));
    shortcut(&desk_link, &install.join("fooapp.exe"));
    let gitconfig = fx.home.join(".gitconfig");
    let git_text = format!("[core]\n\teditor = '{}' --wait\n", install.join("fooapp.exe").to_string_lossy().replace('\\', "/"));
    std::fs::write(&gitconfig, &git_text).unwrap();

    // 9.1 / UNI-01: routes per instance; unknown sources point to Settings, never to deleting the folder.
    let (code, list) = fx.cli(&["apps", "uninstall", "list"]);
    assert_eq!(code, 0, "{list}");
    envelope(&list);
    let find = |name: &str| list["items"].as_array().unwrap().iter().find(|i| i["name"] == name).cloned().unwrap_or_else(|| panic!("{name}: {list}"));
    let foo = find("FooApp");
    assert_eq!(foo["uninstall"]["state"], "available");
    let codex = find("Codex CLI");
    assert_eq!((codex["source"].as_str(), codex["version"].as_str()), (Some("npm_global"), Some("0.9.1")));
    assert!(codex["last_used_at"].is_string() && foo["last_used_at"].is_null(), "{codex}");
    assert_eq!(codex["uninstall"]["state"], "unverified", "no Node.js in the test home: never the machine's own: {codex}");
    // UNI-07: with (fixture) Node.js, npm itself removes the CLI; the plan binds node.exe and the fixed npm argv.
    let node_dir = fx.home.join(r"Program Files\nodejs");
    std::fs::create_dir_all(node_dir.join(r"node_modules\npm\bin")).unwrap();
    cmd_copy(&node_dir.join("node.exe"));
    std::fs::write(node_dir.join(r"node_modules\npm\bin\npm-cli.js"), b"// fixture, never run\n").unwrap();
    let (_, list2) = fx.cli(&["apps", "uninstall", "list"]);
    let codex = list2["items"].as_array().unwrap().iter().find(|i| i["name"] == "Codex CLI").cloned().unwrap();
    assert_eq!((codex["uninstall"]["state"].as_str(), codex["uninstall"]["adapter"]["adapter_id"].as_str()), (Some("available"), Some("npm.uninstall-global")), "{codex}");
    let (code, cplan) = fx.cli(&["apps", "uninstall", "plan", "create", "--instance", codex["app_instance_id"].as_str().unwrap()]);
    assert_eq!(code, 0, "{cplan}");
    let adapter = cplan["body"]["items"].as_array().unwrap().iter().find(|a| a["action_type"] == "tool_adapter").cloned().unwrap_or_else(|| panic!("{cplan}"));
    assert_eq!(adapter["adapter_id"], "npm.uninstall-global");
    let npm_cli = node_dir.join(r"node_modules\npm\bin\npm-cli.js").to_string_lossy().into_owned();
    assert_eq!(adapter["argv"], serde_json::json!([npm_cli, "uninstall", "--global", "@openai/codex"]), "{adapter}");
    assert!(adapter["executable"].to_string().to_ascii_lowercase().contains(r"nodejs\\node.exe"), "{adapter}");
    assert!(pkg.join("package.json").exists(), "planned only; nothing ran");
    let mystery = find("Mystery Tool");
    assert_eq!((mystery["uninstall"]["state"].as_str(), mystery["uninstall"]["settings_uri"].as_str()), (Some("unverified"), Some("ms-settings:appsfeatures")));
    let (code, err) = fx.cli(&["apps", "uninstall", "plan", "create", "--instance", mystery["app_instance_id"].as_str().unwrap()]);
    assert_eq!(err["code"], "unsupported", "{err}");
    assert_ne!(code, 0);
    assert!(err["required_action"].as_str().unwrap().contains("ms-settings:appsfeatures"));
    // UNI-03: an unsigned uninstaller is refused before anything runs.
    let (_, err) = fx.cli(&["apps", "uninstall", "plan", "create", "--instance", find("Unsigned Thing")["app_instance_id"].as_str().unwrap()]);
    assert_eq!(err["code"], "policy_denied", "{err}");
    assert!(other.join("uninst.exe").exists());
    let foo_id = foo["app_instance_id"].as_str().unwrap().to_owned();
    // UNI-02: --end-running is never accepted from an automated caller.
    let (_, err) = fx.cli(&["apps", "uninstall", "plan", "create", "--instance", &foo_id, "--end-running", "--non-interactive"]);
    assert_eq!(err["code"], "invalid_argument", "{err}");

    // 9.2: the app is running and does not close (a windowless fixture waiting on its input).
    let mut app = Command::new(install.join("fooapp.exe")).args(["/c", "pause"]).stdin(Stdio::piped()).stdout(Stdio::null()).spawn().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let (code, plan) = fx.cli(&["apps", "uninstall", "plan", "create", "--instance", &foo_id]);
    assert_eq!(code, 0, "{plan}");
    assert_eq!((plan["body"]["plan_kind"].as_str(), plan["body"]["policy_matchable"].as_bool()), (Some("uninstall"), Some(false)));
    let types: Vec<&str> = plan["body"]["items"].as_array().unwrap().iter().map(|a| a["action_type"].as_str().unwrap()).collect();
    assert_eq!(types, ["app_quit", "tool_adapter"]);
    assert_eq!(plan["body"]["items"][0]["force"], "never");
    assert!(plan["leftover_estimate"].is_object(), "an estimate only: {plan}");
    envelope(&plan);
    let plan_id = plan["body"]["plan_id"].as_str().unwrap().to_owned();
    let (_, err) = fx.cli(&["plan", "apply", &plan_id]);
    assert_eq!(err["code"], "needs_approval");
    approve(&fx.store, &plan);
    let (code, report) = fx.cli(&["plan", "apply", &plan_id]);
    assert_ne!(code, 0, "{report}");
    assert_eq!((report["items"][0]["status"].as_str(), report["items"][0]["code"].as_str()), (Some("failed"), Some("still_running")), "{report}");
    assert_eq!(report["items"][1]["code"], "app_still_running");
    assert!(install.join(r"payload\data.bin").exists(), "cancel/refusal: nothing ran, nothing was deleted");
    assert!(app.try_wait().unwrap().is_none(), "never ended automatically");
    app.kill().unwrap();
    let _ = app.wait();

    // 9.3 / UNI-03: the uninstaller succeeds, but its folder is still there → partial, leftovers come separately.
    let (code, plan) = fx.cli(&["apps", "uninstall", "plan", "create", "--instance", &foo_id]);
    assert_eq!(code, 0, "{plan}");
    approve(&fx.store, &plan);
    let (_, report) = fx.cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()]);
    assert_eq!(report["items"][0]["code"], "not_running", "{report}");
    assert_eq!((report["items"][1]["status"].as_str(), report["items"][1]["code"].as_str()), (Some("succeeded"), Some("leftovers_remain")), "{report}");
    let msg = report["items"][1]["message"].as_str().unwrap();
    assert!(msg.contains("exited with code 0") && msg.contains(" ms"), "{msg}");
    assert_eq!(report["status"], "partial");
    assert!(!install.join("payload").exists() && install.exists());
    let op = report["operation_id"].as_str().unwrap().to_owned();

    // 9.4 / 9.5: grouped leftovers with evidence; only caches and logs preselected, models and settings not.
    let (code, left) = fx.cli(&["apps", "uninstall", "leftovers", "--operation", &op]);
    assert_eq!(code, 0, "{left}");
    envelope(&left);
    let items = all_items(&left);
    assert!(items.iter().all(|i| i["selected"] == false || (i["group"] == "caches_logs" && i["tier"] == "default_selected")), "UNI-05: {left}");
    let item = |suffix: &str| items.iter().find(|i| i["display"].as_str().unwrap().ends_with(suffix)).cloned().unwrap_or_else(|| panic!("{suffix}: {left}"));
    assert_eq!((item(r"FooApp\Cache")["selected"].as_bool(), item(r"FooApp\Cache")["group"].as_str()), (Some(true), Some("caches_logs")));
    assert_eq!((item(r"Local\FooApp")["selected"].as_bool()), Some(true), "a folder of logs only");
    assert_eq!((item(r"FooApp\models")["selected"].as_bool(), item(r"FooApp\models")["group"].as_str()), (Some(false), Some("personal_data")));
    assert_eq!(item("settings.json")["selected"], false);
    let bundle = item(r"Programs\FooApp");
    assert_eq!((bundle["group"].as_str(), bundle["selected"].as_bool()), (Some("bundle"), Some(false)));
    let key = item(r"HKCU\SOFTWARE\FooApp");
    assert_eq!((key["action"].as_str(), key["selected"].as_bool()), (Some("registry_backup_delete"), Some(false)));
    let registration = item(r"Uninstall\FooApp");
    let tray = items.iter().find(|i| i["display"].as_str().unwrap().contains("FooTray")).cloned().unwrap_or_else(|| panic!("startup entry: {left}"));
    assert_eq!((tray["group"].as_str(), tray["action"].as_str()), (Some("startup"), Some("startup_toggle")));
    // UNI-09: each shortcut is its own teardown object; a desktop shortcut is reported with guidance only.
    let menu = items.iter().find(|i| i["display"].as_str().unwrap().eq_ignore_ascii_case(&menu_link.to_string_lossy())).cloned().unwrap_or_else(|| panic!("{left}"));
    assert!(menu["display"].as_str().unwrap().contains("Start Menu"), "{menu}");
    assert_eq!((menu["kind"].as_str(), menu["group"].as_str(), menu["action"].as_str(), menu["selected"].as_bool()), (Some("shortcut"), Some("startup"), Some("fs_trash"), Some(false)));
    let desk = items.iter().find(|i| i["display"].as_str().unwrap().eq_ignore_ascii_case(&desk_link.to_string_lossy())).cloned().unwrap_or_else(|| panic!("{left}"));
    assert_eq!((desk["action"].as_str(), desk["selected"].as_bool()), (Some("manual"), Some(false)), "{desk}");
    // UNI-05: the git config naming the app's program is a read-only hint, never an object of the preview or a plan.
    let refs = left["external_references"].as_array().unwrap();
    assert!(refs.len() == 1 && refs[0].as_str().unwrap().contains(".gitconfig"), "{left}");
    assert!(!items.iter().any(|i| i["display"].as_str().unwrap().ends_with(".gitconfig")));
    let (_, err) = fx.cli(&["apps", "uninstall", "leftovers", "plan", "create", "--operation", &op, "--items", desk["object_id"].as_str().unwrap()]);
    assert_eq!(err["code"], "unsupported", "manual objects are never planned: {err}");

    // 9.6 / 9.8: the leftovers plan is a separate review: caches/logs plus explicitly named objects.
    let extra = format!("{},{},{},{}", key["object_id"].as_str().unwrap(), registration["object_id"].as_str().unwrap(), tray["object_id"].as_str().unwrap(), menu["object_id"].as_str().unwrap());
    let (code, lplan) = fx.cli(&["apps", "uninstall", "leftovers", "plan", "create", "--operation", &op, "--items", &extra]);
    assert_eq!(code, 0, "{lplan}");
    let types: Vec<&str> = lplan["body"]["items"].as_array().unwrap().iter().map(|a| a["action_type"].as_str().unwrap()).collect();
    assert_eq!(types.iter().filter(|t| **t == "fs_trash").count(), 3, "{types:?}");
    assert_eq!(types.iter().filter(|t| **t == "registry_backup_delete").count(), 2);
    assert_eq!(types.iter().filter(|t| **t == "startup_toggle").count(), 1);
    let (_, err) = fx.cli(&["apps", "uninstall", "leftovers", "plan", "create", "--operation", &op, "--items", "lo_nope"]);
    assert_eq!(err["code"], "invalid_argument");
    approve(&fx.store, &lplan);
    // UNI-09: one teardown step fails (the Start menu shortcut is held open): every step is reported on its own and the
    // operation is partial, never completed.
    let held = std::fs::OpenOptions::new().read(true).share_mode(1 | 2).open(&menu_link).unwrap();
    let (code, lreport) = fx.cli(&["plan", "apply", lplan["body"]["plan_id"].as_str().unwrap()]);
    drop(held);
    assert_eq!((code, lreport["status"].as_str()), (5, Some("partial")), "{lreport}");
    let planned: Vec<&str> = lplan["body"]["items"].as_array().unwrap().iter().map(|a| a["action_id"].as_str().unwrap()).collect();
    let reported: Vec<&str> = lreport["items"].as_array().unwrap().iter().map(|i| i["action_id"].as_str().unwrap()).collect();
    assert_eq!(reported, planned, "one result per step");
    let failed: Vec<&Value> = lreport["items"].as_array().unwrap().iter().filter(|i| i["status"] != "succeeded").collect();
    assert_eq!(failed.len(), 1, "{lreport}");
    assert_eq!((failed[0]["status"].as_str(), failed[0]["code"].as_str()), (Some("failed"), Some("in_use")), "{lreport}");
    assert!(menu_link.exists());
    assert!(!roaming.join(r"FooApp\Cache\c.bin").exists() && roaming.join(r"FooApp\models\m.gguf").exists(), "models kept");
    assert!(!fx.key.exists("HKCU", r"SOFTWARE\FooApp") && !fx.key.exists("HKCU", &k));
    let (_, q) = reg(&["query", &format!(r"HKCU\{}\HKCU\{RUN}", fx.key.0), "/v", "FooTray"]);
    assert!(!q.contains("FooTray"), "stale startup entry removed after its backup");

    // UNI-06: restore through a reviewed recovery plan, never over a key recreated since.
    let lop = lreport["operation_id"].as_str().unwrap().to_owned();
    let (code, rplan) = fx.cli(&["apps", "uninstall", "restore", "--operation", &lop]);
    assert_eq!((code, rplan["body"]["plan_kind"].as_str()), (0, Some("recovery")), "{rplan}");
    fx.key.set("HKCU", r"SOFTWARE\FooApp", "Theme", "recreated");
    approve(&fx.store, &rplan);
    let (_, rreport) = fx.cli(&["plan", "apply", rplan["body"]["plan_id"].as_str().unwrap()]);
    let codes: Vec<&str> = rreport["items"].as_array().unwrap().iter().map(|i| i["code"].as_str().unwrap()).collect();
    assert!(codes.contains(&"destination_exists"), "{rreport}");
    assert!(fx.key.exists("HKCU", &k), "the uninstall registration came back from its backup: {rreport}");
    let (_, theme) = reg(&["query", &format!(r"HKCU\{}\HKCU\SOFTWARE\FooApp", fx.key.0), "/v", "Theme"]);
    assert!(theme.contains("recreated"), "the recreated key is left as it is");

    // The failed step is planned again on its own once the shortcut is free: completed, moved to the Recycle Bin.
    let (code, splan) = fx.cli(&["apps", "uninstall", "leftovers", "plan", "create", "--operation", &op, "--items", menu["object_id"].as_str().unwrap()]);
    assert_eq!(code, 0, "{splan}");
    approve(&fx.store, &splan);
    let (code, sreport) = fx.cli(&["plan", "apply", splan["body"]["plan_id"].as_str().unwrap()]);
    assert_eq!((code, sreport["status"].as_str()), (0, Some("completed")), "{sreport}");
    assert!(!menu_link.exists() && desk_link.exists(), "only the planned shortcut moved");
    assert_eq!(std::fs::read_to_string(&gitconfig).unwrap(), git_text, "hints are never edited");
    let recycled: Vec<String> = bin.ours().into_iter().map(|(_, orig)| orig.to_ascii_lowercase()).collect();
    assert!(recycled.iter().any(|o| o.ends_with(r"programs\fooapp\fooapp.lnk")) && recycled.iter().any(|o| o.ends_with(r"cache\c.bin")), "{recycled:?}");

    drop(fx);
    let _ = std::fs::remove_dir_all(&base);
    assert_eq!(real_snapshot(), before, "real uninstall keys and Run values are untouched");
    drop(bin);
}
