//! Batch uninstall in one plan (parity UNI-11, task 9.9) end to end, on fixtures only — the same isolation as
//! `uninstall_flow.rs`: the registry is redirected to a throwaway key under `HKCU\Software\CleanManagerTest\…` (created
//! and deleted here with reg.exe) and the home folder to a scratch directory. The "vendor uninstallers" are copies of
//! the system's cmd.exe in the scratch install folders; one of them exits with a failure code. No real application is
//! uninstalled; the real uninstall and Run keys are only queried, before and after, to prove they are unchanged. The
//! leftovers plan is created but not applied, so nothing reaches the Recycle Bin; the bin guard proves that too.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Bin records (`$I` files) whose original path lies in this test's scratch folder; removed again on drop.
struct BinGuard(String);

impl BinGuard {
    fn ours(&self) -> Vec<PathBuf> {
        use cm_platform::windows::actions::recycle_bin::{parse_info, user_bin_dirs};
        let mut out = Vec::new();
        for dir in user_bin_dirs() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with("$I")
                    && std::fs::read(e.path()).ok().and_then(|d| parse_info(&d)).is_some_and(|(orig, _, _)| orig.to_ascii_lowercase().contains(&self.0))
                {
                    out.push(e.path());
                }
            }
        }
        out
    }
}

impl Drop for BinGuard {
    fn drop(&mut self) {
        for info in self.ours() {
            let r = info.with_file_name(info.file_name().unwrap().to_string_lossy().replacen("$I", "$R", 1));
            let _ = if r.is_dir() { std::fs::remove_dir_all(&r) } else { std::fs::remove_file(&r) };
            let _ = std::fs::remove_file(&info);
        }
    }
}

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-unbatch-{}-{name}", std::process::id()));
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

struct TestKey(String);

impl TestKey {
    fn new(name: &str) -> TestKey {
        let k = TestKey(format!(r"Software\CleanManagerTest\clib{}_{name}", std::process::id()));
        let _ = reg(&["delete", &format!(r"HKCU\{}", k.0), "/f"]);
        k
    }
    fn set(&self, key: &str, name: &str, data: &str) {
        let (ok, out) = reg(&["add", &format!(r"HKCU\{}\HKCU\{key}", self.0), "/v", name, "/t", "REG_SZ", "/d", data, "/f"]);
        assert!(ok, "{out}");
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

/// The real uninstall keys and Run values, reduced to lines that carry a fixture marker. Other software on the
/// machine (a CI runner's updaters, for one) may change unrelated entries during the run; what must hold is that
/// nothing this test plants or names ever reaches the real keys.
fn real_snapshot() -> String {
    const MARKERS: [&str; 7] = ["goodapp", "badapp", "contoso falcon", "mystery tool", "cleanmanagertest", "cm-cli-uninst", "cleanmanager"];
    let (_, a) = reg(&["query", &format!(r"HKCU\{UNINSTALL}"), "/s"]);
    let (_, b) = reg(&["query", &format!(r"HKLM\{UNINSTALL}"), "/s"]);
    let (_, c) = reg(&["query", &format!(r"HKCU\{RUN}")]);
    let all = format!("{a}\n{b}\n{c}");
    all.lines().filter(|l| MARKERS.iter().any(|m| l.to_ascii_lowercase().contains(m))).collect::<Vec<_>>().join("\n")
}

struct Fixture {
    key: TestKey,
    home: PathBuf,
    store: PathBuf,
}

impl Fixture {
    fn cli(&self, args: &[&str]) -> (i32, Value) {
        let out = Command::new(env!("CARGO_BIN_EXE_cleanmanager"))
            .args(args)
            .arg("--json")
            .env("CM_STORE_DIR", &self.store)
            .env("CM_UNINSTALL_TEST_ROOT", &self.key.0)
            .env("CM_STARTUP_TEST_ROOT", &self.key.0)
            .env("CM_STARTUP_TEST_FOLDER", self.home.join(r"AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup"))
            .env("CM_UNINSTALL_TEST_HOME", &self.home)
            .env("CM_UNINSTALL_QUIT_WAIT_MS", "300")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        assert_eq!(text.lines().count(), 1, "exactly one record: {text}");
        (out.status.code().unwrap(), serde_json::from_str(&text).unwrap())
    }

    /// A registered app in `<home>\Programs\<name>` whose uninstaller is a cmd.exe copy run with `args`, plus a cache.
    fn app(&self, name: &str, publisher: &str, args: &str) -> PathBuf {
        let install = self.home.join(format!(r"Programs\{name}"));
        std::fs::create_dir_all(install.join("payload")).unwrap();
        std::fs::write(install.join(r"payload\data.bin"), b"payload").unwrap();
        cmd_copy(&install.join("uninst.exe"));
        let cache = self.home.join(format!(r"AppData\Roaming\{name}\Cache"));
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("c.bin"), b"cache").unwrap();
        let k = format!(r"{UNINSTALL}\{name}");
        self.key.set(&k, "DisplayName", name);
        self.key.set(&k, "Publisher", publisher);
        self.key.set(&k, "InstallLocation", &install.to_string_lossy());
        self.key.set(&k, "UninstallString", &format!("\"{}\" {args}", install.join("uninst.exe").display()));
        install
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

fn ids(v: &Value, key: &str) -> Vec<String> {
    v[key].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_owned()).collect()
}

#[test]
fn one_plan_for_two_apps_one_uninstaller_fails_the_other_completes_and_protected_apps_stay_refused() {
    let before = real_snapshot();
    let base = scratch("batch");
    let bin = BinGuard(base.file_name().unwrap().to_string_lossy().to_ascii_lowercase());
    let fx = Fixture { key: TestKey::new("batch"), home: base.join("home"), store: base.join("store") };
    // GoodApp's uninstaller removes its payload and exits 0; BadApp's exits 3 and removes nothing. Both are cmd.exe
    // copies (catalog-signed by Microsoft, so the registered publisher must say so).
    let good = fx.app("GoodApp", "Microsoft Corporation", &format!("/c rmdir /s /q \"{}\"", base.join(r"home\Programs\GoodApp\payload").display()));
    let bad = fx.app("BadApp", "Microsoft Corporation", "/c exit 3");
    // UNI-10: an endpoint security agent (by name) never gets an uninstall, alone or in a batch.
    fx.app("Contoso Falcon Sensor", "Microsoft Corporation", "/c exit 0");
    fx.key.set(&format!(r"{UNINSTALL}\Mystery"), "DisplayName", "Mystery Tool");

    let (code, list) = fx.cli(&["apps", "uninstall", "list"]);
    assert_eq!(code, 0, "{list}");
    let id = |name: &str| -> String {
        list["items"].as_array().unwrap().iter().find(|i| i["name"] == name).unwrap_or_else(|| panic!("{name}: {list}"))["app_instance_id"].as_str().unwrap().to_owned()
    };
    let (good_id, bad_id, agent_id, mystery_id) = (id("GoodApp"), id("BadApp"), id("Contoso Falcon Sensor"), id("Mystery Tool"));

    // On its own, the agent is refused before anything is planned.
    let (code, err) = fx.cli(&["apps", "uninstall", "plan", "create", "--instance", &agent_id]);
    assert_eq!((err["kind"].as_str(), err["code"].as_str()), (Some("error"), Some("policy_denied")), "{err}");
    assert_ne!(code, 0);
    // A batch of only refused applications is refused as a whole, naming each one's reason.
    let (_, err) = fx.cli(&["apps", "uninstall", "plan", "create", "--instance", &agent_id, "--instance", &mystery_id]);
    assert_eq!(err["kind"], "error", "{err}");
    let msg = err["message"].as_str().unwrap();
    assert!(msg.contains("Contoso Falcon Sensor") && msg.contains("Mystery Tool"), "{msg}");
    // The same application twice is a mistake, not a bigger batch.
    let (_, err) = fx.cli(&["apps", "uninstall", "plan", "create", "--instance", &good_id, "--instance", &good_id]);
    assert_eq!(err["code"], "invalid_argument", "{err}");

    // UNI-11: one plan for BadApp and GoodApp; the agent in the same request is refused exactly as on its own and the
    // plan for the others is unaffected.
    let (code, plan) = fx.cli(&["apps", "uninstall", "plan", "create", "--instance", &bad_id, "--instance", &agent_id, "--instance", &good_id]);
    assert_eq!(code, 5, "partial: one requested application is not in the plan: {plan}");
    envelope(&plan);
    assert_eq!(plan["body"]["plan_kind"], "uninstall");
    let actions: Vec<(&str, &str)> = plan["body"]["items"].as_array().unwrap().iter().map(|a| (a["action_id"].as_str().unwrap(), a["action_type"].as_str().unwrap())).collect();
    assert_eq!(actions, [("q0000000", "app_quit"), ("u0000001", "tool_adapter"), ("u0010000", "app_quit"), ("u0010001", "tool_adapter")], "two phases per application");
    let apps = plan["uninstall_apps"].as_array().unwrap();
    assert_eq!(apps.iter().map(|a| a["app_instance_id"].as_str().unwrap()).collect::<Vec<_>>(), [bad_id.as_str(), good_id.as_str()]);
    assert_eq!(ids(&apps[1], "action_ids"), ["u0010000", "u0010001"]);
    assert!(apps.iter().all(|a| a["leftover_estimate"].is_object()) && plan["leftover_estimate"].is_object());
    let refused = plan["uninstall_refused"].as_array().unwrap();
    assert_eq!(refused.len(), 1, "{plan}");
    assert_eq!((refused[0]["app_instance_id"].as_str(), refused[0]["code"].as_str()), (Some(agent_id.as_str()), Some("policy_denied")));
    assert!(!plan["body"]["items"].to_string().contains("Falcon"), "the protected app has no action in the plan");

    // One application's uninstaller fails; the other application still runs and completes. Results per application.
    approve(&fx.store, &plan);
    let (code, report) = fx.cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()]);
    envelope(&report);
    assert_eq!((code, report["status"].as_str()), (5, Some("partial")), "{report}");
    let codes: Vec<&str> = report["items"].as_array().unwrap().iter().map(|i| i["code"].as_str().unwrap()).collect();
    assert_eq!(codes, ["not_running", "uninstaller_failed", "not_running", "leftovers_remain"], "{report}");
    let per_app = report["uninstall_apps"].as_array().unwrap();
    assert_eq!(per_app.len(), 2, "{report}");
    assert_eq!((per_app[0]["name"].as_str(), per_app[0]["status"].as_str(), per_app[0]["leftovers_reviewable"].as_bool()), (Some("BadApp"), Some("failed"), Some(false)));
    assert!(per_app[0]["message"].as_str().unwrap().contains("code 3"), "{report}");
    assert_eq!((per_app[1]["name"].as_str(), per_app[1]["status"].as_str(), per_app[1]["leftovers_reviewable"].as_bool()), (Some("GoodApp"), Some("partial"), Some(true)));
    assert!(!good.join("payload").exists() && bad.join(r"payload\data.bin").exists(), "only GoodApp's uninstaller removed something");
    let op = report["operation_id"].as_str().unwrap().to_owned();
    // The stored report says the same.
    let (_, shown) = fx.cli(&["operations", "show", &op]);
    assert_eq!(shown["uninstall_apps"], report["uninstall_apps"]);

    // Leftovers are reviewed per application; the failed application keeps everything (it is still installed).
    let (code, left) = fx.cli(&["apps", "uninstall", "leftovers", "--operation", &op]);
    assert_eq!(code, 0, "{left}");
    envelope(&left);
    assert!(left["app"].is_null(), "several applications: no single app at the top: {left}");
    let la = left["apps"].as_array().unwrap();
    assert_eq!((la[0]["app"]["name"].as_str(), la[0]["leftovers_reviewable"].as_bool(), la[0]["groups"].as_array().unwrap().len()), (Some("BadApp"), Some(false), 0));
    assert_eq!((la[1]["app"]["name"].as_str(), la[1]["leftovers_reviewable"].as_bool()), (Some("GoodApp"), Some(true)));
    let all = left["groups"].to_string().to_ascii_lowercase();
    assert!(all.contains(r"roaming\\goodapp") && !all.contains("badapp"), "{left}");
    // UNI-05 holds in a batch: only caches and logs are preselected.
    let items: Vec<Value> = left["groups"].as_array().unwrap().iter().flat_map(|g| g["items"].as_array().unwrap().clone()).collect();
    assert!(items.iter().all(|i| i["selected"] == false || i["group"] == "caches_logs"), "{left}");
    // The leftovers plan never touches the failed application.
    let (code, lplan) = fx.cli(&["apps", "uninstall", "leftovers", "plan", "create", "--operation", &op]);
    assert_eq!(code, 0, "{lplan}");
    let body = lplan["body"].to_string().to_ascii_lowercase();
    assert!(body.contains(r"goodapp\\cache") && !body.contains("badapp"), "{lplan}");
    assert!(bad.join(r"payload\data.bin").exists() && base.join(r"home\AppData\Roaming\BadApp\Cache\c.bin").exists());

    // A single uninstall whose uninstaller failed has no leftovers to review either.
    let (code, splan) = fx.cli(&["apps", "uninstall", "plan", "create", "--instance", &bad_id]);
    assert_eq!(code, 0, "{splan}");
    approve(&fx.store, &splan);
    let (_, sreport) = fx.cli(&["plan", "apply", splan["body"]["plan_id"].as_str().unwrap()]);
    assert_eq!(sreport["uninstall_apps"][0]["status"], "failed", "{sreport}");
    let (_, err) = fx.cli(&["apps", "uninstall", "leftovers", "--operation", sreport["operation_id"].as_str().unwrap()]);
    assert_eq!(err["code"], "invalid_argument", "{err}");
    assert!(err["message"].as_str().unwrap().contains("BadApp"), "{err}");

    assert!(bin.ours().is_empty(), "nothing was moved to the Recycle Bin");
    drop(fx);
    let _ = std::fs::remove_dir_all(&base);
    assert_eq!(real_snapshot(), before, "real uninstall keys and Run values are untouched");
    drop(bin);
}
