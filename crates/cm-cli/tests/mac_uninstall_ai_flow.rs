//! AI command-line tools (UNI-07) and orphaned leftovers (UNI-08) on macOS through the real binary, on fixtures only:
//! `CM_UNINSTALL_TEST_ROOT`/`HOME` redirect the inventory, the npm prefixes and `~/Library`; a fixture `bin/node` script
//! under the fake Homebrew prefix stands in for Node.js, so no real npm, package, `~/.claude` or Library is touched.
//! Moves to the user's real Trash (a fixture cache file, a fixture orphan) run only with `CM_ALLOW_MAC_DEVICE_TESTS=1`
//! or on a hosted runner (`GITHUB_ACTIONS=true`) and remove exactly what they moved; otherwise only the plans are
//! checked. Receipts are written through `cm_store` as the terminal review would. Records are checked against
//! `schemas/v1`.
#![cfg(target_os = "macos")]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
    node_log: PathBuf,
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
            .env("CM_UNINSTALL_QUIT_WAIT_MS", "1500")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let text = String::from_utf8(o.stdout).expect("UTF-8 stdout");
        assert_eq!(text.lines().count(), 1, "exactly one record for {args:?}: {text}\n{}", String::from_utf8_lossy(&o.stderr));
        (o.status.code().unwrap(), serde_json::from_str(text.trim_end()).expect("JSON record"))
    }

    fn library(&self) -> PathBuf {
        self.home.join("Library")
    }

    fn prefix(&self) -> PathBuf {
        self.root.join("opt/homebrew")
    }
}

fn write(path: &Path, bytes: impl AsRef<[u8]>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn plist_xml(body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>{body}</dict></plist>\n"
    )
}

fn touch_old(paths: &[PathBuf]) {
    for p in paths {
        assert!(Command::new("/usr/bin/touch").args(["-h", "-t", "202001010000"]).arg(p).status().unwrap().success(), "touch {}", p.display());
    }
}

fn fixture(tag: &str) -> Fx {
    let base = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("cm-cli-mac-ai-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
    let uniq = format!("{}{tag}{nanos:08x}", std::process::id());
    let (root, home) = (base.join("root"), base.join("home"));
    // A fixture Node.js under the Homebrew prefix of the fake root: it logs its arguments and, for the expected argv
    // only (and a prefix inside this fixture), removes the package folder as npm would.
    let prefix = root.join("opt/homebrew");
    let node_log = base.join("node.log");
    let node = prefix.join("bin/node");
    let script = format!(
        "#!/bin/sh\necho \"$*\" >> '{log}'\n[ \"$NO_UPDATE_NOTIFIER\" = 1 ] && [ -n \"$HOME\" ] || exit 7\n[ \"$2 $3 $4 $6\" = 'uninstall --global --prefix --ignore-scripts' ] || exit 9\ncase \"$5\" in '{base}'/*) ;; *) exit 6;; esac\nrm -rf \"$5/lib/node_modules/$7\"\n",
        log = node_log.display(),
        base = base.display()
    );
    write(&node, script);
    std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o755)).unwrap();
    write(&prefix.join("lib/node_modules/npm/bin/npm-cli.js"), "// npm stand-in\n");
    write(&prefix.join("lib/node_modules/@anthropic-ai/claude-code/package.json"), r#"{"name":"@anthropic-ai/claude-code","version":"1.0.5"}"#);
    write(&prefix.join("lib/node_modules/@anthropic-ai/claude-code/cli.js"), "// cli\n");
    // Claude Code's data folder in the test home.
    let d = home.join(".claude");
    write(&d.join(format!("shell-snapshots/snap-{uniq}.sh")), [5u8; 128]);
    write(&d.join("projects/p1/session.jsonl"), "{}\n");
    write(&d.join("settings.json"), "{}");
    write(&d.join(".credentials.json"), "{}");
    write(&d.join("worktrees/w1/.git/HEAD"), "ref: refs/heads/main\n");
    write(&home.join(".claude.json"), "{}");
    // An installed app and orphans under the test home's Library.
    let quill = root.join("Applications/Quill.app/Contents");
    write(
        &quill.join("Info.plist"),
        plist_xml("<key>CFBundleIdentifier</key><string>com.example.quill</string><key>CFBundleName</key><string>Quill</string><key>CFBundleExecutable</key><string>Quill</string>"),
    );
    std::fs::create_dir_all(quill.join("MacOS")).unwrap();
    std::fs::copy("/usr/bin/true", quill.join("MacOS/Quill")).unwrap();
    let lib = home.join("Library");
    let mut old = Vec::new();
    for f in [
        format!("Application Support/com.cmfixture.gone{uniq}/orphan-{uniq}.bin"),
        "Caches/com.example.quill/c".to_owned(),
        "Caches/com.apple.Safari/c".to_owned(),
        "Caches/Decoy Tool/c".to_owned(),
    ] {
        let p = lib.join(f);
        write(&p, [9u8; 256]);
        old.push(p.clone());
        old.push(p.parent().unwrap().to_path_buf());
    }
    let agent = lib.join(format!("LaunchAgents/com.cmfixture.updater{uniq}.plist"));
    write(
        &agent,
        plist_xml(&format!(
            "<key>Label</key><string>com.cmfixture.updater{uniq}</string><key>ProgramArguments</key><array><string>/Applications/Gone{uniq}.app/Contents/MacOS/upd</string></array>"
        )),
    );
    old.push(agent);
    touch_old(&old);
    write(&lib.join(format!("Caches/com.cmfixture.recent{uniq}/c")), "used today");
    Fx { base: Scratch(base), root, home, node_log, uniq }
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

#[test]
fn an_ai_cli_is_removed_by_npm_and_its_sessions_and_settings_are_never_preselected() {
    let s = schema();
    let fx = fixture("ai");
    // D15: gated until experimental features are on.
    let (code, gated) = fx.cli(&["apps", "uninstall", "list"]);
    assert_eq!((code, gated["code"].as_str()), (7, Some("unsupported")), "{gated}");
    assert_eq!(fx.cli(&["experimental", "--state", "on"]).0, 0);

    // UNI-07: the npm-global record with its source, version, route and last use.
    let (code, list) = fx.cli(&["apps", "uninstall", "list"]);
    assert_eq!(code, 0, "{list}");
    validate(&list, &s);
    let claude = item(&list, "Claude Code");
    assert_eq!((claude["source"].as_str(), claude["version"].as_str()), (Some("npm_global"), Some("1.0.5")), "{claude}");
    assert_eq!((claude["uninstall"]["state"].as_str(), claude["uninstall"]["adapter"]["adapter_id"].as_str()), (Some("available"), Some("npm.uninstall-global")));
    assert_eq!(claude["uninstall"]["adapter"]["package_manager"], true);
    assert!(claude["last_used_at"].is_string(), "{claude}");
    let id = claude["app_instance_id"].as_str().unwrap().to_owned();

    let (code, plan) = fx.cli(&["apps", "uninstall", "plan", "create", "--instance", &id]);
    assert_eq!(code, 0, "{plan}");
    validate(&plan, &s);
    assert_eq!(types(&plan), ["app_quit", "tool_adapter"]);
    let adapter = &plan["body"]["items"][1];
    assert_eq!(adapter["adapter_id"], "npm.uninstall-global");
    let argv: Vec<&str> = adapter["argv"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
    let prefix = fx.prefix().display().to_string();
    assert_eq!(argv[1..], ["uninstall", "--global", "--prefix", prefix.as_str(), "--ignore-scripts", "@anthropic-ai/claude-code"], "{plan}");
    assert!(std::fs::read_to_string(&fx.node_log).is_err(), "planning runs nothing");

    approve(&fx.store(), &plan);
    let (code, report) = fx.cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()]);
    validate(&report, &s);
    assert_eq!((code, report["status"].as_str()), (0, Some("completed")), "{report}");
    assert_eq!(report["uninstall_apps"][0]["status"], "completed", "{report}");
    assert!(!fx.prefix().join("lib/node_modules/@anthropic-ai/claude-code").exists(), "removed by the (fixture) package manager");
    let runs = std::fs::read_to_string(&fx.node_log).unwrap();
    assert_eq!(runs.lines().count(), 1, "{runs}");
    let op = report["operation_id"].as_str().unwrap().to_owned();

    // ANA-03 / UNI-07: the data folder per entry; only caches preselected; sessions, settings and credentials need to
    // be named one by one; the worktree is blocked; the home-folder settings file is reported only.
    let (code, left) = fx.cli(&["apps", "uninstall", "leftovers", "--operation", &op]);
    assert_eq!(code, 0, "{left}");
    validate(&left, &s);
    let items = all_items(&left);
    let find = |suffix: &str| items.iter().find(|i| i["display"].as_str().unwrap().ends_with(suffix)).cloned().unwrap_or_else(|| panic!("{suffix}: {left:#}"));
    let snaps = find(".claude/shell-snapshots");
    assert_eq!((snaps["selected"].as_bool(), snaps["group"].as_str(), snaps["action"].as_str()), (Some(true), Some("caches_logs"), Some("fs_trash")));
    for suffix in [".claude/projects", ".claude/settings.json", ".claude/.credentials.json"] {
        let i = find(suffix);
        assert_eq!((i["selected"].as_bool(), i["tier"].as_str()), (Some(false), Some("review_only")), "{i}");
    }
    let wt = find(".claude/worktrees");
    assert_eq!((wt["tier"].as_str(), wt["selected"].as_bool()), (Some("blocked"), Some(false)));
    assert_eq!(find(".claude.json")["action"], "manual");
    assert!(items.iter().all(|i| i["selected"] == false || i["group"] == "caches_logs"), "{left}");
    let (_, err) = fx.cli(&["apps", "uninstall", "leftovers", "plan", "create", "--operation", &op, "--items", wt["object_id"].as_str().unwrap()]);
    assert_eq!(err["code"], "permission_required", "{err}");

    let (code, lplan) = fx.cli(&["apps", "uninstall", "leftovers", "plan", "create", "--operation", &op]);
    assert_eq!(code, 0, "{lplan}");
    validate(&lplan, &s);
    assert_eq!(types(&lplan), ["fs_trash"], "{lplan}");
    let body = lplan["body"].to_string();
    assert!(body.contains(&format!("snap-{}.sh", fx.uniq)) && !body.contains("session.jsonl") && !body.contains("settings.json"), "{lplan}");
    if !device_tests_allowed() {
        eprintln!("skipped moving the fixture cache to the real Trash: set CM_ALLOW_MAC_DEVICE_TESTS=1");
        return;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
    let _guard = RealTrashGuard(vec![home.join(".Trash").join(format!("snap-{}.sh", fx.uniq))]);
    approve(&fx.store(), &lplan);
    let (code, lreport) = fx.cli(&["plan", "apply", lplan["body"]["plan_id"].as_str().unwrap()]);
    if lreport["items"].as_array().is_some_and(|i| i.iter().any(|x| x["code"] == "trash_unavailable")) {
        eprintln!("the runner's volume has no Trash: {lreport}");
        return;
    }
    assert_eq!((code, lreport["status"].as_str()), (0, Some("completed")), "{lreport}");
    assert!(!fx.home.join(format!(".claude/shell-snapshots/snap-{}.sh", fx.uniq)).exists());
    assert!(fx.home.join(".claude/projects/p1/session.jsonl").exists() && fx.home.join(".claude/settings.json").exists());
}

#[test]
fn orphans_are_found_by_bundle_id_and_moved_to_the_trash_only_when_named() {
    let s = schema();
    let fx = fixture("orph");
    let (code, gated) = fx.cli(&["apps", "uninstall", "orphans"]);
    assert_eq!((code, gated["code"].as_str()), (7, Some("unsupported")), "{gated}");
    let (code, _) = fx.cli(&["apps", "uninstall", "orphans", "plan", "create", "--items", "lo_x"]);
    assert_eq!(code, 7);
    assert_eq!(fx.cli(&["experimental", "--state", "on"]).0, 0);

    let (code, orphans) = fx.cli(&["apps", "uninstall", "orphans"]);
    assert_eq!(code, 0, "{orphans}");
    validate(&orphans, &s);
    assert_eq!(orphans["default_selected"], 0);
    let items = all_items(&orphans);
    let find = |suffix: &str| items.iter().find(|i| i["display"].as_str().unwrap().ends_with(suffix)).cloned();
    let gone = find(&format!("Application Support/com.cmfixture.gone{}", fx.uniq)).unwrap_or_else(|| panic!("{orphans:#}"));
    assert_eq!((gone["tier"].as_str(), gone["selected"].as_bool(), gone["action"].as_str()), (Some("review_only"), Some(false), Some("fs_trash")), "{gone}");
    assert_eq!(gone["logical_bytes"], "256");
    let job = find(&format!("com.cmfixture.updater{}.plist", fx.uniq)).unwrap_or_else(|| panic!("{orphans:#}"));
    assert!(job["evidence"].to_string().contains("no longer exists"), "{job}");
    let recent = find(&format!("Caches/com.cmfixture.recent{}", fx.uniq)).unwrap_or_else(|| panic!("{orphans:#}"));
    assert_eq!((recent["tier"].as_str(), recent["marker"].as_str()), (Some("kept"), Some("owner_unknown")));
    // The installed app's data, Apple's and a folder named only by a name are never orphans.
    for suffix in ["Caches/com.example.quill", "Caches/com.apple.Safari", "Caches/Decoy Tool"] {
        assert!(find(suffix).is_none(), "{suffix}: {orphans:#}");
    }
    assert!(items.iter().all(|i| i["selected"] == false));

    // Nothing is planned unless named; kept objects are refused.
    let (_, err) = fx.cli(&["apps", "uninstall", "orphans", "plan", "create"]);
    assert_eq!(err["code"], "invalid_argument", "{err}");
    let (_, err) = fx.cli(&["apps", "uninstall", "orphans", "plan", "create", "--items", recent["object_id"].as_str().unwrap()]);
    assert_eq!(err["code"], "policy_denied", "{err}");
    let (code, plan) = fx.cli(&["apps", "uninstall", "orphans", "plan", "create", "--items", gone["object_id"].as_str().unwrap()]);
    assert_eq!(code, 0, "{plan}");
    validate(&plan, &s);
    assert_eq!(types(&plan), ["fs_trash"], "{plan}");
    assert!(plan["body"].to_string().contains(&format!("orphan-{}.bin", fx.uniq)), "{plan}");
    if !device_tests_allowed() {
        eprintln!("skipped moving the fixture orphan to the real Trash: set CM_ALLOW_MAC_DEVICE_TESTS=1");
        return;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
    let _guard = RealTrashGuard(vec![home.join(".Trash").join(format!("orphan-{}.bin", fx.uniq))]);
    approve(&fx.store(), &plan);
    let (code, report) = fx.cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()]);
    validate(&report, &s);
    if report["items"].as_array().is_some_and(|i| i.iter().any(|x| x["code"] == "trash_unavailable")) {
        eprintln!("the runner's volume has no Trash: {report}");
        return;
    }
    assert_eq!((code, report["status"].as_str()), (0, Some("completed")), "{report}");
    assert!(!fx.library().join(format!("Application Support/com.cmfixture.gone{}/orphan-{}.bin", fx.uniq, fx.uniq)).exists());
}
