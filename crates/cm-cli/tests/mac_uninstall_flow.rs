//! App uninstall on macOS (parity 9.1 macOS half, UNI-01..11) through the real binary, on fixtures only:
//! `CM_UNINSTALL_TEST_ROOT`/`HOME` redirect the inventory (bundles, Caskroom) and `~/Library`, and `CM_UPDATES_TEST_BREW`
//! is a stand-in `brew` script; while redirected the binary never uses the real Homebrew, applications or Library. The
//! "running apps" are copies of `/bin/sleep` and `/bin/bash` inside fixture bundles. Moves to the user's real Trash (a
//! fixture bundle, fixture leftovers) run only with `CM_ALLOW_MAC_DEVICE_TESTS=1` or on a hosted runner
//! (`GITHUB_ACTIONS=true`) and remove exactly what they moved; otherwise only the plans are checked. Receipts are written
//! through `cm_store` as the terminal review would. Records are checked against `schemas/v1`.
#![cfg(target_os = "macos")]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use serde_json::Value;

fn schema() -> Value {
    serde_json::from_str(include_str!("../../../schemas/v1/protocol.schema.json")).unwrap()
}

/// The subset of JSON Schema that `schemas/v1` uses for these records.
fn check(v: &Value, s: &Value, defs: &Value, at: &str) -> Result<(), String> {
    if let Some(r) = s.get("$ref").and_then(Value::as_str) {
        let name = r.strip_prefix("#/$defs/").ok_or_else(|| format!("{at}: unsupported $ref {r}"))?;
        return check(v, &defs[name], defs, at);
    }
    if let Some(any) = s.get("anyOf").or_else(|| s.get("oneOf")).and_then(Value::as_array) {
        let ok = any.iter().filter(|alt| check(v, alt, defs, at).is_ok()).count();
        if ok == 0 || (s.get("oneOf").is_some() && ok != 1) {
            return Err(format!("{at}: {v} matches {ok} alternatives"));
        }
    }
    if let Some(c) = s.get("const")
        && c != v
    {
        return Err(format!("{at}: {v} is not {c}"));
    }
    if let Some(e) = s.get("enum").and_then(Value::as_array)
        && !e.contains(v)
    {
        return Err(format!("{at}: {v} not in {e:?}"));
    }
    if let Some(t) = s.get("type") {
        let types: Vec<&str> = match t {
            Value::String(t) => vec![t.as_str()],
            Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        let is = |t: &str| match t {
            "string" => v.is_string(),
            "object" => v.is_object(),
            "array" => v.is_array(),
            "integer" => v.is_i64() || v.is_u64(),
            "number" => v.is_number(),
            "boolean" => v.is_boolean(),
            "null" => v.is_null(),
            _ => false,
        };
        if !types.iter().any(|t| is(t)) {
            return Err(format!("{at}: {v} is not {types:?}"));
        }
    }
    if let (Some(min), Some(text)) = (s.get("minLength").and_then(Value::as_u64), v.as_str())
        && (text.chars().count() as u64) < min
    {
        return Err(format!("{at}: shorter than {min}"));
    }
    if let (Some(n), Some(min)) = (v.as_f64(), s.get("minimum").and_then(Value::as_f64))
        && n < min
    {
        return Err(format!("{at}: {n} < {min}"));
    }
    if let (Some(n), Some(max)) = (v.as_f64(), s.get("maximum").and_then(Value::as_f64))
        && n > max
    {
        return Err(format!("{at}: {n} > {max}"));
    }
    if let Some(obj) = v.as_object()
        && let Some(props) = s.get("properties").and_then(Value::as_object)
    {
        for r in s.get("required").and_then(Value::as_array).into_iter().flatten() {
            let r = r.as_str().unwrap();
            if !obj.contains_key(r) {
                return Err(format!("{at}: required field {r} missing"));
            }
        }
        for (k, val) in obj {
            match props.get(k) {
                Some(ps) => check(val, ps, defs, &format!("{at}.{k}"))?,
                None if s.get("additionalProperties") == Some(&Value::Bool(false)) => return Err(format!("{at}: field {k} is not in the schema")),
                None => {}
            }
        }
    }
    if let (Some(items), Some(arr)) = (s.get("items"), v.as_array()) {
        for (i, item) in arr.iter().enumerate() {
            check(item, items, defs, &format!("{at}[{i}]"))?;
        }
    }
    Ok(())
}

fn validate(record: &Value, schema: &Value) {
    let kind = record["kind"].as_str().expect("kind");
    let defs = &schema["$defs"];
    assert!(defs[kind].is_object(), "kind {kind} is not in schemas/v1");
    check(record, &defs[kind], defs, kind).unwrap_or_else(|e| panic!("{e}\n{record:#}"));
}

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        // A locked fixture folder (the "root-owned" stand-in) is opened again so it can be removed.
        let _ = std::fs::set_permissions(self.0.join("root/Applications/Locked"), std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn device_tests_allowed() -> bool {
    std::env::var("CM_ALLOW_MAC_DEVICE_TESTS").is_ok_and(|v| v == "1") || std::env::var("GITHUB_ACTIONS").is_ok_and(|v| v == "true")
}

/// Removes from the user's real Trash exactly the fixture objects this test moved there (unique names), nothing else.
struct RealTrashGuard(Vec<PathBuf>);

impl Drop for RealTrashGuard {
    fn drop(&mut self) {
        for p in &self.0 {
            if p.symlink_metadata().is_ok() {
                let r = if p.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) };
                if let Err(e) = r {
                    eprintln!("could not remove {} from the Trash: {e}", p.display());
                }
            }
        }
    }
}

struct Fx {
    base: Scratch,
    root: PathBuf,
    home: PathBuf,
    brew: PathBuf,
    brew_log: PathBuf,
    uniq: String,
}

impl Fx {
    fn store(&self) -> PathBuf {
        self.base.0.join("store")
    }

    fn cli(&self, args: &[&str]) -> (i32, Value) {
        let o = Command::new(env!("CARGO_BIN_EXE_cleanmanager"))
            .args(args)
            .arg("--json")
            .env("CM_STORE_DIR", self.store())
            .env("CM_UNINSTALL_TEST_ROOT", &self.root)
            .env("CM_UNINSTALL_TEST_HOME", &self.home)
            .env("CM_UPDATES_TEST_BREW", &self.brew)
            .env("CM_UNINSTALL_QUIT_WAIT_MS", "1500")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let text = String::from_utf8(o.stdout).expect("UTF-8 stdout");
        assert_eq!(text.lines().count(), 1, "exactly one record for {args:?}: {text}\n{}", String::from_utf8_lossy(&o.stderr));
        (o.status.code().unwrap(), serde_json::from_str(text.trim_end()).expect("JSON record"))
    }

    fn brew_runs(&self) -> Vec<String> {
        std::fs::read_to_string(&self.brew_log).map(|t| t.lines().map(str::to_owned).collect()).unwrap_or_default()
    }

    fn apps(&self) -> PathBuf {
        self.root.join("Applications")
    }

    fn library(&self) -> PathBuf {
        self.home.join("Library")
    }
}

fn info_xml(pairs: &[(&str, &str)]) -> String {
    let body: String = pairs.iter().map(|(k, v)| format!("<key>{k}</key><string>{v}</string>")).collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>{body}</dict></plist>\n"
    )
}

fn write(path: &Path, bytes: impl AsRef<[u8]>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// A fake app bundle: `Contents/Info.plist` and a copy of a system binary as its executable.
/// Copy the system binary `program` to `exe`. Some macOS releases kill a copy of an Apple platform binary at launch
/// (SIGKILL, seen on macOS 27.0.1); only then is the copy signed ad hoc. Where the plain copy runs it is left as it is.
fn copy_system_binary(program: &str, exe: &Path) {
    use std::os::unix::process::ExitStatusExt;
    // The probe runs a throwaway copy: a file the system once killed at launch stays unusable here even after signing.
    let probe_path = exe.with_extension("probe");
    std::fs::copy(program, &probe_path).unwrap();
    let probe = Command::new(&probe_path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap();
    std::fs::remove_file(&probe_path).unwrap();
    std::fs::copy(program, exe).unwrap();
    if probe.signal() == Some(9) {
        let signed = Command::new("/usr/bin/codesign").args(["-s", "-", "-f"]).arg(exe).output().unwrap();
        assert!(signed.status.success(), "ad-hoc signing {}: {}", exe.display(), String::from_utf8_lossy(&signed.stderr));
    }
}

fn bundle(path: &Path, bid: &str, name: &str, program: &str) -> PathBuf {
    write(&path.join("Contents/Info.plist"), info_xml(&[("CFBundleIdentifier", bid), ("CFBundleName", name), ("CFBundleExecutable", name), ("CFBundleShortVersionString", "1.0")]));
    let exe = path.join("Contents/MacOS").join(name);
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    copy_system_binary(program, &exe);
    exe
}

fn cask(root: &Path, token: &str, app: &str) {
    write(
        &root.join(format!("opt/homebrew/Caskroom/{token}/.metadata/1.0/20260101000000.000/Casks/{token}.json")),
        format!(r#"{{"token":"{token}","version":"1.0","artifacts":[{{"app":["{app}"]}}]}}"#),
    );
    std::fs::create_dir_all(root.join(format!("opt/homebrew/Caskroom/{token}/1.0"))).unwrap();
}

fn fixture(tag: &str) -> Fx {
    let base = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("cm-cli-mac-unin-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
    let uniq = format!("{}{tag}{nanos:08x}", std::process::id());
    let (root, home) = (base.join("root"), base.join("home"));
    let apps = root.join("Applications");
    // Homebrew casks: one whose `brew uninstall` succeeds, one whose fails.
    bundle(&apps.join("Brewed.app"), "com.example.brewed", "Brewed", "/usr/bin/true");
    cask(&root, "brewed-app", "Brewed.app");
    bundle(&apps.join("Failing.app"), "com.example.failing", "Failing", "/usr/bin/true");
    cask(&root, "failing-app", "Failing.app");
    // Refused routes: an App Store app, a bundle this user cannot move (its folder is not writable: the stand-in for a
    // root-owned or package-installed bundle) and an app that is part of macOS.
    let store = apps.join("Store.app");
    bundle(&store, "com.example.store", "Store", "/usr/bin/true");
    write(&store.join("Contents/_MASReceipt/receipt"), b"opaque");
    bundle(&apps.join("Locked/Locked.app"), "com.example.locked", "Locked", "/usr/bin/true");
    bundle(&root.join("System/Applications/Notes.app"), "com.apple.Notes", "Notes", "/usr/bin/true");
    // Leftovers of Brewed and Failing under the test home.
    let lib = home.join("Library");
    write(&lib.join(format!("Caches/com.example.brewed/blob-{uniq}")), [7u8; 512]);
    write(&lib.join(format!("Caches/Brewed/decoy-{uniq}")), b"name only");
    write(&lib.join("Preferences/com.example.brewed.plist"), info_xml(&[("Theme", "dark")]));
    write(&lib.join("Containers/com.example.brewed/Data/Library/x"), b"protected");
    write(&lib.join("Caches/com.example.failing/c.bin"), b"kept: the app is still installed");
    // The stand-in brew: Homebrew's fixed environment and argv only; a successful cask removal removes the bundle and
    // its Caskroom record, as Homebrew does.
    let brew_log = base.join("brew.log");
    let brew = base.join("brew/bin/brew");
    let script = format!(
        "#!/bin/sh\necho \"$*\" >> '{log}'\n[ \"$HOMEBREW_NO_AUTO_UPDATE\" = 1 ] && [ \"$HOMEBREW_NO_ANALYTICS\" = 1 ] || exit 7\ncase \"$*\" in\n  'uninstall --cask brewed-app') rm -rf '{room}/brewed-app' '{apps}/Brewed.app'; exit 0;;\n  'uninstall --cask failing-app') echo 'Error: fixture failure' >&2; exit 1;;\n  *) exit 9;;\nesac\n",
        log = brew_log.display(),
        room = root.join("opt/homebrew/Caskroom").display(),
        apps = apps.display()
    );
    write(&brew, script);
    std::fs::set_permissions(&brew, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(apps.join("Locked"), std::fs::Permissions::from_mode(0o555)).unwrap();
    Fx { base: Scratch(base), root, home, brew, brew_log, uniq }
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

fn item<'a>(list: &'a Value, name: &str) -> &'a Value {
    list["items"].as_array().unwrap().iter().find(|i| i["name"] == name).unwrap_or_else(|| panic!("{name}: {list:#}"))
}

fn all_items(v: &Value) -> Vec<Value> {
    v["groups"].as_array().unwrap().iter().flat_map(|g| g["items"].as_array().unwrap().clone()).collect()
}

fn types(plan: &Value) -> Vec<String> {
    plan["body"]["items"].as_array().unwrap().iter().map(|a| a["action_type"].as_str().unwrap().to_owned()).collect()
}

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn routes_refusals_and_a_batch_where_one_cask_fails_and_the_other_completes() {
    let s = schema();
    let fx = fixture("batch");
    // D15: gated until experimental features are on.
    let (code, gated) = fx.cli(&["apps", "uninstall", "list"]);
    assert_eq!((code, gated["code"].as_str()), (7, Some("unsupported")), "{gated}");
    assert_eq!(fx.cli(&["experimental", "--state", "on"]).0, 0);

    // UNI-01: one route per source; refusals carry their reason and are never escalated.
    let (code, list) = fx.cli(&["apps", "uninstall", "list"]);
    assert_eq!(code, 0, "{list}");
    validate(&list, &s);
    let route = |name: &str| {
        let i = item(&list, name);
        (i["uninstall"]["state"].as_str().unwrap().to_owned(), i["uninstall"]["adapter"]["adapter_id"].as_str().map(str::to_owned))
    };
    assert_eq!(route("Brewed"), ("available".into(), Some("homebrew.uninstall-cask".into())));
    assert_eq!(item(&list, "Brewed")["source"], "homebrew");
    assert_eq!(item(&list, "Brewed")["uninstall"]["adapter"]["package_manager"], true);
    assert_eq!(route("Store").0, "unsupported");
    assert!(item(&list, "Store")["uninstall"]["reason"].as_str().unwrap().contains("Launchpad"));
    assert_eq!(route("Locked").0, "unsupported");
    assert!(item(&list, "Locked")["uninstall"]["reason"].as_str().unwrap().contains("administrator"));
    assert_eq!((route("Notes").0.as_str(), item(&list, "Notes")["protected_class"].as_bool()), ("unsupported", Some(true)));
    let id = |name: &str| item(&list, name)["app_instance_id"].as_str().unwrap().to_owned();
    // `apps list` reports the same decision as each instance's uninstall capability.
    let (code, apps) = fx.cli(&["apps", "list", "--limit", "500"]);
    assert_eq!(code, 0, "{apps}");
    let cap = |name: &str| item(&apps, name)["uninstall_capability"]["state"].as_str().unwrap().to_owned();
    assert_eq!((cap("Brewed").as_str(), cap("Locked").as_str(), cap("Notes").as_str()), ("available", "unsupported", "unsupported"));

    // UNI-10: the macOS app is refused on its own; a bundle this user cannot change needs administrator rights.
    let (code, err) = fx.cli(&["apps", "uninstall", "plan", "create", "--instance", &id("Notes")]);
    assert_eq!((err["kind"].as_str(), err["code"].as_str()), (Some("error"), Some("policy_denied")), "{err}");
    assert_ne!(code, 0);
    let (_, err) = fx.cli(&["apps", "uninstall", "plan", "create", "--instance", &id("Locked")]);
    assert_eq!(err["code"], "permission_required", "{err}");
    assert!(err["message"].as_str().unwrap().contains("Finder › Move to Trash"), "{err}");
    let (_, err) = fx.cli(&["apps", "uninstall", "plan", "create", "--instance", &id("Store")]);
    assert_eq!(err["code"], "permission_required", "{err}");

    // UNI-11: one plan for Failing and Brewed; Notes in the same request is refused exactly as on its own.
    let (code, plan) = fx.cli(&["apps", "uninstall", "plan", "create", "--instance", &id("Failing"), "--instance", &id("Notes"), "--instance", &id("Brewed")]);
    assert_eq!(code, 5, "{plan}");
    validate(&plan, &s);
    assert_eq!(types(&plan), ["app_quit", "tool_adapter", "app_quit", "tool_adapter"]);
    let adapters: Vec<&Value> = plan["body"]["items"].as_array().unwrap().iter().filter(|a| a["action_type"] == "tool_adapter").collect();
    assert_eq!(adapters[0]["argv"], serde_json::json!(["uninstall", "--cask", "failing-app"]));
    assert_eq!(adapters[1]["argv"], serde_json::json!(["uninstall", "--cask", "brewed-app"]));
    assert!(adapters.iter().all(|a| a["adapter_id"] == "homebrew.uninstall-cask" && !a["argv"].to_string().contains("zap")));
    let refused = plan["uninstall_refused"].as_array().unwrap();
    assert_eq!((refused.len(), refused[0]["code"].as_str()), (1, Some("policy_denied")), "{plan}");
    assert!(plan["leftover_estimate"].is_object());
    assert!(fx.brew_runs().is_empty(), "planning runs nothing");

    approve(&fx.store(), &plan);
    let (code, report) = fx.cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()]);
    validate(&report, &s);
    assert_eq!((code, report["status"].as_str()), (5, Some("partial")), "{report}");
    let codes: Vec<&str> = report["items"].as_array().unwrap().iter().map(|i| i["code"].as_str().unwrap()).collect();
    assert_eq!(codes, ["not_running", "uninstaller_failed", "not_running", "done"], "{report}");
    assert!(report["items"][3]["message"].as_str().unwrap().contains("brew exited with code 0"), "{report}");
    let per_app = report["uninstall_apps"].as_array().unwrap();
    assert_eq!((per_app[0]["name"].as_str(), per_app[0]["status"].as_str(), per_app[0]["leftovers_reviewable"].as_bool()), (Some("Failing"), Some("failed"), Some(false)));
    assert_eq!((per_app[1]["name"].as_str(), per_app[1]["status"].as_str(), per_app[1]["leftovers_reviewable"].as_bool()), (Some("Brewed"), Some("completed"), Some(true)));
    assert_eq!(fx.brew_runs(), ["uninstall --cask failing-app", "uninstall --cask brewed-app"]);
    assert!(!fx.apps().join("Brewed.app").exists() && fx.apps().join("Failing.app").exists());
    let op = report["operation_id"].as_str().unwrap().to_owned();

    // UNI-04/05: leftovers by bundle-ID evidence; the name-only decoy is review-only; Containers only reported; the
    // failed application keeps everything.
    let (code, left) = fx.cli(&["apps", "uninstall", "leftovers", "--operation", &op]);
    assert_eq!(code, 0, "{left}");
    validate(&left, &s);
    let la = left["apps"].as_array().unwrap();
    assert_eq!((la[0]["app"]["name"].as_str(), la[0]["leftovers_reviewable"].as_bool(), la[0]["groups"].as_array().unwrap().len()), (Some("Failing"), Some(false), 0));
    let items = all_items(&left);
    let find = |suffix: &str| items.iter().find(|i| i["display"].as_str().unwrap().ends_with(suffix)).cloned().unwrap_or_else(|| panic!("{suffix}: {left:#}"));
    let cache = find("Caches/com.example.brewed");
    assert_eq!((cache["group"].as_str(), cache["selected"].as_bool(), cache["action"].as_str()), (Some("caches_logs"), Some(true), Some("fs_trash")));
    assert_eq!(cache["logical_bytes"], "512");
    let decoy = find("Caches/Brewed");
    assert_eq!((decoy["selected"].as_bool(), decoy["tier"].as_str()), (Some(false), Some("review_only")), "{decoy}");
    assert!(decoy["evidence"][0].as_str().unwrap().starts_with("name only"));
    let prefs = find("Preferences/com.example.brewed.plist");
    assert_eq!((prefs["group"].as_str(), prefs["selected"].as_bool()), (Some("settings"), Some(false)));
    let container = find("Containers/com.example.brewed");
    assert_eq!((container["tier"].as_str(), container["action"].as_str(), container["logical_bytes"].is_null()), (Some("blocked"), Some("manual"), true));
    assert!(!left["groups"].to_string().contains("com.example.failing"), "{left}");
    assert!(items.iter().all(|i| i["selected"] == false || i["group"] == "caches_logs"), "UNI-05: {left}");

    // Stage 2 is its own plan: the cache by default, the decoy only when named; protected objects are refused.
    let (_, err) = fx.cli(&["apps", "uninstall", "leftovers", "plan", "create", "--operation", &op, "--items", container["object_id"].as_str().unwrap()]);
    assert_eq!(err["code"], "permission_required", "{err}");
    let (code, lplan) = fx.cli(&["apps", "uninstall", "leftovers", "plan", "create", "--operation", &op, "--items", decoy["object_id"].as_str().unwrap()]);
    assert_eq!(code, 0, "{lplan}");
    validate(&lplan, &s);
    assert_eq!(types(&lplan), ["fs_trash", "fs_trash"], "{lplan}");
    let body = lplan["body"].to_string();
    assert!(body.contains(&format!("blob-{}", fx.uniq)) && body.contains(&format!("decoy-{}", fx.uniq)) && !body.contains("com.example.failing"), "{lplan}");
    if !device_tests_allowed() {
        eprintln!("skipped moving the fixture leftovers to the real Trash: set CM_ALLOW_MAC_DEVICE_TESTS=1");
        return;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
    let _guard = RealTrashGuard(vec![home.join(".Trash").join(format!("blob-{}", fx.uniq)), home.join(".Trash").join(format!("decoy-{}", fx.uniq))]);
    approve(&fx.store(), &lplan);
    let (code, lreport) = fx.cli(&["plan", "apply", lplan["body"]["plan_id"].as_str().unwrap()]);
    if lreport["items"].as_array().is_some_and(|i| i.iter().any(|x| x["code"] == "trash_unavailable")) {
        eprintln!("the runner's volume has no Trash: {lreport}");
        return;
    }
    assert_eq!((code, lreport["status"].as_str()), (0, Some("completed")), "{lreport}");
    assert!(!fx.library().join(format!("Caches/com.example.brewed/blob-{}", fx.uniq)).exists());
    assert!(fx.library().join("Preferences/com.example.brewed.plist").exists() && fx.library().join("Containers/com.example.brewed/Data/Library/x").exists());
}

#[test]
fn a_running_bundle_is_asked_to_quit_and_moved_to_the_trash_never_deleted() {
    let s = schema();
    let fx = fixture("trash");
    assert_eq!(fx.cli(&["experimental", "--state", "on"]).0, 0);
    // A bundle running a process that ignores the quit request: reported, never ended, nothing moved.
    let stubborn = bundle(&fx.apps().join("Stubborn.app"), "com.example.stubborn", "Stubborn", "/bin/bash");
    let name = format!("Sleepy{}", fx.uniq);
    let sleepy_app = fx.apps().join(format!("{name}.app"));
    let sleepy = bundle(&sleepy_app, "com.example.sleepy", &name, "/bin/sleep");
    let (_, list) = fx.cli(&["apps", "uninstall", "list"]);
    let id = |n: &str| item(&list, n)["app_instance_id"].as_str().unwrap().to_owned();
    assert_eq!(item(&list, &name)["uninstall"]["adapter"]["adapter_id"], "macos.trash-app-bundle");

    // The script reports once its trap is set, so the quit request cannot arrive before it.
    let ready = fx.base.0.join("trap-set");
    let script = format!("trap '' TERM; : > '{}'; while :; do sleep 1; done", ready.display());
    let mut child = Running(Command::new(&stubborn).args(["-c", &script]).spawn().unwrap());
    for _ in 0..100 {
        if ready.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(ready.exists(), "the fixture script never started");
    let (code, plan) = fx.cli(&["apps", "uninstall", "plan", "create", "--instance", &id("Stubborn")]);
    assert_eq!(code, 0, "{plan}");
    validate(&plan, &s);
    assert_eq!(types(&plan), ["app_quit", "tool_adapter"]);
    assert_eq!((plan["body"]["items"][0]["force"].as_str(), plan["body"]["items"][1]["argv"].as_array().map(Vec::len)), (Some("never"), Some(0)));
    // UNI-02: a plan that may end processes is never created for an automated caller.
    let (_, err) = fx.cli(&["apps", "uninstall", "plan", "create", "--instance", &id("Stubborn"), "--end-running", "--non-interactive"]);
    assert_eq!(err["code"], "invalid_argument", "{err}");
    approve(&fx.store(), &plan);
    let (code, report) = fx.cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()]);
    assert_ne!(code, 0, "{report}");
    assert_eq!((report["items"][0]["status"].as_str(), report["items"][0]["code"].as_str()), (Some("failed"), Some("still_running")), "{report}");
    assert_eq!(report["items"][1]["code"], "app_still_running", "{report}");
    assert!(child.0.try_wait().unwrap().is_none(), "never ended automatically");
    assert!(fx.apps().join("Stubborn.app/Contents/Info.plist").exists(), "nothing moved");
    let _ = child.0.kill();

    // A running app that honours the quit request; its bundle then goes to the Trash (device tests only).
    let _sleepy_run = Running(Command::new(&sleepy).arg("120").spawn().unwrap());
    std::thread::sleep(std::time::Duration::from_millis(500));
    let (code, plan) = fx.cli(&["apps", "uninstall", "plan", "create", "--instance", &id(&name)]);
    assert_eq!(code, 0, "{plan}");
    let adapter = &plan["body"]["items"][1];
    assert_eq!((adapter["adapter_id"].as_str(), adapter["argv"].as_array().map(Vec::len)), (Some("macos.trash-app-bundle"), Some(0)), "{plan}");
    assert!(adapter["executable"].to_string().contains(&name) || plan["meta"].to_string().contains(&name), "{plan}");
    if !device_tests_allowed() {
        eprintln!("skipped moving the fixture bundle to the real Trash: set CM_ALLOW_MAC_DEVICE_TESTS=1");
        return;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
    let _guard = RealTrashGuard(vec![home.join(".Trash").join(format!("{name}.app"))]);
    approve(&fx.store(), &plan);
    let (code, report) = fx.cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()]);
    validate(&report, &s);
    if report["items"][1]["code"] == "trash_unavailable" {
        eprintln!("the runner's volume has no Trash: {report}");
        return;
    }
    assert_eq!((code, report["status"].as_str()), (0, Some("completed")), "{report}");
    assert_eq!(report["items"][0]["code"], "closed", "{report}");
    assert_eq!(report["items"][1]["code"], "done", "{report}");
    assert!(report["items"][1]["message"].as_str().unwrap().contains("moved to the Trash"), "{report}");
    assert!(!sleepy_app.exists(), "moved");
    assert!(home.join(".Trash").join(format!("{name}.app")).is_dir(), "in the Trash, never deleted");
    assert_eq!(report["uninstall_apps"][0]["status"], "completed");
}
