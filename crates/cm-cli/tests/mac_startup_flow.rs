//! `startup list` / `startup disable` on macOS (parity 11.1, STA-01..04) through the real binary. Fixture plists live
//! under `CM_STARTUP_TEST_ROOT` (a fake `/`) and `CM_STARTUP_TEST_FOLDER` (the user's LaunchAgents), so real launchd
//! jobs are never read for those runs and `launchctl` is never queried; one read-only run lists the runner's real jobs.
//! The command group is gated (D15): each scratch store opts in with `experimental --state on`. Every record is
//! checked against `schemas/v1` (`startup_list` / `startup_item`).
#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> Scratch {
    let base = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let p = base.join(format!("cm-cli-mac-startup-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_cleanmanager"))
        .args(["experimental", "--state", "on", "--json"])
        .env("CM_STORE_DIR", p.join("store"))
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stdout));
    Scratch(p)
}

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

struct Run {
    code: i32,
    record: Value,
}

fn run(base: &Path, args: &[&str], env: &[(&str, &Path)]) -> Run {
    let mut c = Command::new(env!("CARGO_BIN_EXE_cleanmanager"));
    c.args(args).env("CM_STORE_DIR", base.join("store")).env_remove("CM_STARTUP_TEST_ROOT").env_remove("CM_STARTUP_TEST_FOLDER");
    for (k, v) in env {
        c.env(k, v);
    }
    let o = c.output().unwrap();
    let text = String::from_utf8(o.stdout).expect("UTF-8 stdout");
    assert_eq!(text.lines().count(), 1, "exactly one record: {text}");
    Run { code: o.status.code().unwrap(), record: serde_json::from_str(text.trim_end()).expect("JSON record") }
}

fn write(path: &Path, bytes: impl AsRef<[u8]>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn job(label: &str, extra: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>{label}</string>{extra}</dict></plist>\n"
    )
}

fn by_name<'a>(items: &'a [Value], name: &str) -> &'a Value {
    items.iter().find(|i| i["name"] == name).unwrap_or_else(|| panic!("no row {name}: {items:#?}"))
}

#[test]
fn startup_list_reports_fixture_jobs_in_the_contract_shape() {
    let s = schema();
    let base = scratch("list");
    let root = base.0.join("root");
    let agents = base.0.join("agents");
    let bin = base.0.join("bin/agent");
    write(&bin, b"#!/bin/sh\nexit 0\n");
    let bin_s = bin.to_str().unwrap();
    write(&agents.join("com.example.valid.plist"), job("com.example.valid", &format!("<key>ProgramArguments</key><array><string>{bin_s}</string></array><key>RunAtLoad</key><true/>")));
    let missing = base.0.join("removed/Tool");
    write(&agents.join("com.example.broken.plist"), job("com.example.broken", &format!("<key>Program</key><string>{}</string>", missing.display())));
    write(&agents.join("com.example.off.plist"), job("com.example.off", &format!("<key>Program</key><string>{bin_s}</string><key>Disabled</key><true/>")));
    let binary = root.join("Library/LaunchDaemons/com.example.binary.plist");
    write(&binary, job("com.example.binary", &format!("<key>Program</key><string>{bin_s}</string><key>KeepAlive</key><true/>")));
    let plutil = Command::new("/usr/bin/plutil").args(["-convert", "binary1"]).arg(&binary).status().unwrap();
    assert!(plutil.success());
    assert!(std::fs::read(&binary).unwrap().starts_with(b"bplist00"));
    write(&root.join("Library/LaunchAgents/com.example.corrupt.plist"), b"<plist><dict><key>Label");
    write(&root.join("System/Library/LaunchAgents/com.apple.fixture.plist"), job("com.apple.fixture", "<key>Program</key><string>/usr/bin/true</string>"));
    let env = [("CM_STARTUP_TEST_ROOT", root.as_path()), ("CM_STARTUP_TEST_FOLDER", agents.as_path())];

    let out = run(&base.0, &["startup", "list", "--json"], &env);
    validate(&out.record, &s);
    assert_eq!((out.code, out.record["kind"].as_str()), (0, Some("startup_list")), "{}", out.record);
    assert_eq!(out.record["issues"], serde_json::json!([]));
    let items = out.record["items"].as_array().unwrap();
    assert_eq!(items.len(), 7, "six plists and the Service Management row: {items:#?}");
    let state = |n: &str| by_name(items, n)["state"].as_str().unwrap().to_owned();
    assert_eq!(state("com.example.valid"), "enabled");
    assert_eq!(state("com.example.broken"), "broken");
    assert_eq!(state("com.example.off"), "disabled");
    assert_eq!(state("com.example.binary"), "enabled");
    assert_eq!(state("com.example.corrupt"), "unknown");
    let valid = by_name(items, "com.example.valid");
    assert_eq!((valid["source"].as_str(), valid["scope"].as_str(), valid["protected"].as_bool()), (Some("launchd_user"), Some("user"), Some(false)));
    assert_eq!(valid["target"]["display"], bin_s);
    assert_eq!(valid["target"]["platform"], "macos");
    assert!(valid["reason"].as_str().unwrap().contains("launchctl was not queried"), "{valid:#}");
    let daemon = by_name(items, "com.example.binary");
    assert_eq!((daemon["source"].as_str(), daemon["scope"].as_str()), (Some("launchd_system"), Some("system")));
    let os = by_name(items, "com.apple.fixture");
    assert_eq!((os["protected"].as_bool(), os["publisher"].as_str(), os["publisher_evidence"].as_str()), (Some(true), Some("Apple"), Some("code_signature")));
    let sm: Vec<&Value> = items.iter().filter(|i| i["source"] == "sm_background_item").collect();
    assert_eq!(sm.len(), 1);
    assert_eq!((sm[0]["state"].as_str(), sm[0]["definition"].as_str()), (Some("unknown"), Some("x-apple.systempreferences:com.apple.LoginItems-Settings.extension")));
    for i in items {
        assert_eq!(i["toggle_capability"], "unsupported", "{i:#}");
        assert!(i["reason"].as_str().is_some_and(|r| !r.is_empty()), "{i:#}");
        if let Some(d) = i["target"]["display"].as_str() {
            assert!(d.starts_with(base.0.to_str().unwrap()) || d == "/usr/bin/true", "{i:#}");
        }
    }

    // STA-02/04: no plan is created; protected rows are refused as protected, the others as unsupported.
    let id = |n: &str| by_name(items, n)["startup_item_id"].as_str().unwrap().to_owned();
    let off = run(&base.0, &["startup", "disable", &id("com.example.valid"), "--json"], &env);
    validate(&off.record, &s);
    assert_eq!((off.record["kind"].as_str(), off.record["code"].as_str()), (Some("error"), Some("unsupported")), "{}", off.record);
    let prot = run(&base.0, &["startup", "disable", &id("com.apple.fixture"), "--json"], &env);
    assert_eq!(prot.record["code"], "policy_denied", "{}", prot.record);
    let stale = run(&base.0, &["startup", "remove-stale", &id("com.example.broken"), "--json"], &env);
    assert_eq!(stale.record["code"], "unsupported", "{}", stale.record);
    let unknown = run(&base.0, &["startup", "enable", "st_0000000000000000", "--json"], &env);
    assert_eq!(unknown.record["code"], "invalid_argument", "{}", unknown.record);
    let rel = run(&base.0, &["startup", "list", "--json"], &[("CM_STARTUP_TEST_ROOT", Path::new("relative/root"))]);
    assert_eq!(rel.record["code"], "invalid_argument", "{}", rel.record);
}

/// Read-only: the runner's real launchd jobs. /System jobs are listed and protected; nothing toggles.
#[test]
fn startup_list_reads_the_real_jobs_read_only() {
    let s = schema();
    let base = scratch("live");
    let out = run(&base.0, &["startup", "list", "--json"], &[]);
    validate(&out.record, &s);
    assert!(out.code == 0 || out.code == 5, "success or partial: {}", out.record);
    let items = out.record["items"].as_array().unwrap();
    let system: Vec<&Value> = items.iter().filter(|i| i["definition"].as_str().is_some_and(|d| d.starts_with("/System/Library/Launch"))).collect();
    assert!(!system.is_empty());
    assert!(system.iter().all(|i| i["protected"] == true && i["scope"] == "system"));
    assert!(items.iter().all(|i| i["toggle_capability"] == "unsupported"));
    let caps = run(&base.0, &["capabilities", "--json"], &[]);
    let rows = caps.record["capabilities"].as_array().unwrap();
    let row = |n: &str| rows.iter().find(|r| r["name"] == n).unwrap_or_else(|| panic!("{n}: {rows:#?}"));
    assert_eq!(row("startup.toggle")["state"], "unsupported");
    assert_eq!(row("startup.inventory")["state"], "unverified");
}
