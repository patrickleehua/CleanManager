//! `apps list` on macOS (MVP 11.2) through the real binary, on fixture bundles only:
//! `CM_UNINSTALL_TEST_ROOT` is a fake `/` (`Applications/`, `opt/homebrew/Caskroom/`, `Library/`) and
//! `CM_UNINSTALL_TEST_HOME` a fake home, so the real `/Applications` and Homebrew are never read. Every
//! record is checked against `schemas/v1` (`app_list` / `app_instance`).
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
    let p = base.join(format!("cm-cli-mac-apps-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
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
    c.args(args).env("CM_STORE_DIR", base.join("store")).env_remove("CM_UNINSTALL_TEST_ROOT").env_remove("CM_UNINSTALL_TEST_HOME");
    for (k, v) in env {
        c.env(k, v);
    }
    let o = c.output().unwrap();
    let text = String::from_utf8(o.stdout).expect("UTF-8 stdout");
    assert_eq!(text.lines().count(), 1, "exactly one record: {text}");
    Run { code: o.status.code().unwrap(), record: serde_json::from_str(text.trim_end()).expect("JSON record") }
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

fn bundle(path: &Path, plist: impl AsRef<[u8]>) {
    write(&path.join("Contents/Info.plist"), plist);
    std::fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
}

fn by_location<'a>(items: &'a [Value], path: &Path) -> &'a Value {
    let display = path.to_str().unwrap();
    items.iter().find(|i| i["location"]["display"] == display).unwrap_or_else(|| panic!("no item at {display}: {items:#?}"))
}

fn evidence(item: &Value, kind: &str) -> Vec<String> {
    item["identity_evidence"].as_array().unwrap().iter().filter_map(Value::as_str).filter(|e| e.starts_with(&format!("{kind}: "))).map(str::to_owned).collect()
}

#[test]
fn apps_list_reports_redirected_bundles_in_the_contract_shape() {
    let s = schema();
    let base = scratch("list");
    let (root, home) = (base.0.join("root"), base.0.join("home"));
    let editor = info_xml(&[("CFBundleIdentifier", "com.example.editor"), ("CFBundleName", "Editor"), ("CFBundleShortVersionString", "2.1")]);
    let a = root.join("Applications/Editor.app");
    let b = home.join("Applications/Editor.app");
    bundle(&a, &editor);
    bundle(&b, &editor);
    let store = root.join("Applications/Store.app");
    bundle(&store, info_xml(&[("CFBundleIdentifier", "com.example.store"), ("CFBundleDisplayName", "Store Display")]));
    write(&store.join("Contents/_MASReceipt/receipt"), b"opaque");
    let fox = root.join("Applications/Firefox.app");
    bundle(&fox, info_xml(&[("CFBundleIdentifier", "org.mozilla.firefox"), ("CFBundleShortVersionString", "120.0")]));
    write(&root.join("opt/homebrew/Caskroom/firefox/.metadata/120.0/20240101000000.000/Casks/firefox.json"), br#"{"token":"firefox","artifacts":[{"app":["Firefox.app"]}]}"#);
    let broken = root.join("Applications/Utilities/Broken.app");
    bundle(&broken, b"<plist><dict><key>CFBundleIdentifier");
    let binary = root.join("Applications/Binary.app");
    bundle(&binary, info_xml(&[("CFBundleIdentifier", "com.example.binary"), ("CFBundleVersion", "42")]));
    let plutil = Command::new("/usr/bin/plutil").args(["-convert", "binary1"]).arg(binary.join("Contents/Info.plist")).status().unwrap();
    assert!(plutil.success());
    assert!(std::fs::read(binary.join("Contents/Info.plist")).unwrap().starts_with(b"bplist00"));
    std::fs::create_dir_all(root.join("Library/Application Support/Editor")).unwrap();
    std::fs::create_dir_all(home.join("Library/Application Support/Orphan")).unwrap();
    let env = [("CM_UNINSTALL_TEST_ROOT", root.as_path()), ("CM_UNINSTALL_TEST_HOME", home.as_path())];

    let out = run(&base.0, &["apps", "list", "--json", "--limit", "500"], &env);
    assert_eq!(out.code, 0, "{}", out.record);
    validate(&out.record, &s);
    assert_eq!(out.record["kind"], "app_list");
    assert_eq!(out.record["next_cursor"], Value::Null);
    let items = out.record["items"].as_array().unwrap();
    assert_eq!(items.len(), 6, "only the fixture bundles; data folders are not instances: {items:#?}");

    // APP-01: the same bundle ID in two places is two instances.
    let (ea, eb) = (by_location(items, &a), by_location(items, &b));
    assert_ne!(ea["app_instance_id"], eb["app_instance_id"]);
    for e in [ea, eb] {
        assert_eq!((e["name"].as_str(), e["version"].as_str(), e["source"].as_str()), (Some("Editor"), Some("2.1"), Some("app_bundle")));
        assert_eq!(evidence(e, "bundle_id"), ["bundle_id: com.example.editor"]);
        assert_eq!(evidence(e, "location_identity").len(), 1, "{e:#}");
        assert_eq!(e["protected_class"], false);
        assert_eq!(e["update_status"], "not_checked");
        // UNI-01: a user-writable bundle is moved to the Trash by `apps uninstall` (the same decision as its route).
        assert_eq!(e["uninstall_capability"]["state"], "available", "{e:#}");
        assert_eq!(e["location"]["platform"], "macos");
        assert_eq!(e["location"]["raw_encoding"], "unix_bytes_base64");
    }
    let st = by_location(items, &store);
    assert_eq!((st["source"].as_str(), st["name"].as_str()), (Some("app_store"), Some("Store Display")));
    assert_eq!(evidence(st, "mas_receipt").len(), 1);
    let fx = by_location(items, &fox);
    assert_eq!(fx["source"], "homebrew");
    assert!(evidence(fx, "caskroom_token")[0].starts_with("caskroom_token: firefox (app artifact Firefox.app in "), "{fx:#}");
    let br = by_location(items, &broken);
    assert_eq!((br["source"].as_str(), br["name"].as_str(), &br["version"]), (Some("unknown"), Some("Broken"), &Value::Null));
    assert_eq!(evidence(br, "info_plist_unreadable").len(), 1, "{br:#}");
    assert!(evidence(br, "bundle_id").is_empty());
    let bi = by_location(items, &binary);
    assert_eq!((bi["name"].as_str(), bi["version"].as_str()), (Some("Binary"), Some("42")));
    for i in items {
        assert!(i["location"]["display"].as_str().unwrap().starts_with(base.0.to_str().unwrap()), "never the real machine: {i:#}");
        assert_eq!(evidence(i, "team_id").len(), 1, "{i:#}");
    }

    // Pages cover the same instances in id order.
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut args = vec!["apps", "list", "--json", "--limit", "2"];
        if let Some(c) = &cursor {
            args.extend(["--cursor", c.as_str()]);
        }
        let page = run(&base.0, &args, &env);
        assert_eq!(page.code, 0);
        validate(&page.record, &s);
        seen.extend(page.record["items"].as_array().unwrap().iter().map(|i| i["app_instance_id"].as_str().unwrap().to_owned()));
        match page.record["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => break,
        }
    }
    let mut all: Vec<String> = items.iter().map(|i| i["app_instance_id"].as_str().unwrap().to_owned()).collect();
    all.sort();
    assert_eq!(seen, all);
}

#[test]
fn a_test_root_requires_a_test_home() {
    let s = schema();
    let base = scratch("needs-home");
    let root = base.0.join("root");
    std::fs::create_dir_all(root.join("Applications")).unwrap();
    let out = run(&base.0, &["apps", "list", "--json"], &[("CM_UNINSTALL_TEST_ROOT", root.as_path())]);
    validate(&out.record, &s);
    assert_eq!((out.code, out.record["kind"].as_str(), out.record["code"].as_str()), (2, Some("error"), Some("invalid_argument")), "{}", out.record);
    let empty = run(&base.0, &["apps", "list", "--json"], &[("CM_UNINSTALL_TEST_ROOT", root.as_path()), ("CM_UNINSTALL_TEST_HOME", base.0.join("home").as_path())]);
    validate(&empty.record, &s);
    assert_eq!(empty.code, 0);
    assert_eq!(empty.record["items"], serde_json::json!([]), "an empty fake root lists nothing");
}
