//! `apps updates` / `apps update` on macOS (parity 10.1) through the real binary, on fixtures only:
//! `CM_UNINSTALL_TEST_ROOT`/`HOME` redirect the inventory (bundles, Caskroom, Cellar), `CM_UPDATES_TEST_BREW` is a
//! stand-in `brew` script and `CM_UPDATES_TEST_APPCASTS` a folder of appcast files. While redirected the binary never
//! uses the network or the real Homebrew. Records are checked against `schemas/v1`.
#![cfg(target_os = "macos")]

use std::os::unix::fs::PermissionsExt;
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
    let p = base.join(format!("cm-cli-mac-updates-{}-{name}", std::process::id()));
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

struct Fx {
    base: Scratch,
    root: PathBuf,
    home: PathBuf,
    brew: PathBuf,
    appcasts: PathBuf,
    brew_log: PathBuf,
}

impl Fx {
    fn run(&self, args: &[&str]) -> Run {
        let o = Command::new(env!("CARGO_BIN_EXE_cleanmanager"))
            .args(args)
            .env("CM_STORE_DIR", self.base.0.join("store"))
            .env("CM_UNINSTALL_TEST_ROOT", &self.root)
            .env("CM_UNINSTALL_TEST_HOME", &self.home)
            .env("CM_UPDATES_TEST_BREW", &self.brew)
            .env("CM_UPDATES_TEST_APPCASTS", &self.appcasts)
            .output()
            .unwrap();
        let text = String::from_utf8(o.stdout).expect("UTF-8 stdout");
        assert_eq!(text.lines().count(), 1, "exactly one record: {text}\n{}", String::from_utf8_lossy(&o.stderr));
        Run { code: o.status.code().unwrap(), record: serde_json::from_str(text.trim_end()).expect("JSON record") }
    }

    fn brew_runs(&self) -> usize {
        std::fs::read_to_string(&self.brew_log).map(|t| t.lines().count()).unwrap_or(0)
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

fn bundle(path: &Path, plist: impl AsRef<[u8]>) {
    write(&path.join("Contents/Info.plist"), plist);
    std::fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
}

const APPCAST: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>Sparkly</title>
    <item>
      <title>Version 1.2</title>
      <sparkle:version>120</sparkle:version>
      <sparkle:shortVersionString>1.2</sparkle:shortVersionString>
      <enclosure url="https://updates.example.test/sparkly/Sparkly-1.2.zip" length="1" type="application/octet-stream" sparkle:edSignature="c2ln"/>
    </item>
  </channel>
</rss>
"#;

const OUTDATED: &str = r#"{"formulae":[],"casks":[{"name":"firefox","installed_versions":["130.0"],"current_version":"131.0.2"}]}"#;

fn fixture(name: &str) -> Fx {
    let base = scratch(name);
    let root = base.0.join("root");
    let home = base.0.join("home");
    let apps = root.join("Applications");
    bundle(
        &apps.join("Sparkly.app"),
        info_xml(&[
            ("CFBundleIdentifier", "com.example.sparkly"),
            ("CFBundleName", "Sparkly"),
            ("CFBundleShortVersionString", "1.0"),
            ("CFBundleVersion", "100"),
            ("SUFeedURL", "https://updates.example.test/sparkly/appcast.xml"),
            ("SUPublicEDKey", "pfIShU4dEXqPd5ObYNfDBiQWcXozk7estwzTnF9BamQ="),
        ]),
    );
    bundle(
        &apps.join("Offline.app"),
        info_xml(&[("CFBundleIdentifier", "com.example.offline"), ("CFBundleName", "Offline"), ("CFBundleVersion", "1"), ("SUFeedURL", "https://offline.example.test/appcast.xml")]),
    );
    bundle(&apps.join("Plain.app"), info_xml(&[("CFBundleIdentifier", "com.example.plain"), ("CFBundleName", "Plain"), ("CFBundleShortVersionString", "3.0")]));
    let chat = apps.join("Chatty.app");
    bundle(&chat, info_xml(&[("CFBundleIdentifier", "com.example.chatty"), ("CFBundleName", "Chatty"), ("CFBundleShortVersionString", "5.0")]));
    std::fs::create_dir_all(chat.join("Contents/Frameworks/Electron Framework.framework")).unwrap();
    write(&chat.join("Contents/Resources/app-update.yml"), "provider: github\nowner: example\nrepo: chatty\n");
    let pages = apps.join("Pages.app");
    bundle(&pages, info_xml(&[("CFBundleIdentifier", "com.apple.iWork.Pages"), ("CFBundleName", "Pages"), ("CFBundleShortVersionString", "14.0")]));
    write(&pages.join("Contents/_MASReceipt/receipt"), b"opaque");
    bundle(&apps.join("Firefox.app"), info_xml(&[("CFBundleIdentifier", "org.mozilla.firefox"), ("CFBundleName", "Firefox"), ("CFBundleShortVersionString", "130.0")]));
    write(
        &root.join("opt/homebrew/Caskroom/firefox/.metadata/130.0/20260101000000.000/Casks/firefox.json"),
        br#"{"token":"firefox","version":"131.0.2","auto_updates":null,"artifacts":[{"app":["Firefox.app"]}]}"#,
    );
    std::fs::create_dir_all(root.join("opt/homebrew/Caskroom/firefox/130.0")).unwrap();
    write(&root.join("opt/homebrew/Cellar/wget/1.25.0/INSTALL_RECEIPT.json"), br#"{"installed_on_request":true}"#);
    let appcasts = base.0.join("appcasts");
    write(&appcasts.join("updates.example.test_sparkly_appcast.xml"), APPCAST);
    let brew_log = base.0.join("brew.log");
    let brew = base.0.join("brew/bin/brew");
    let script = [
        "#!/bin/sh".to_owned(),
        format!("echo \"$*\" >> '{}'", brew_log.display()),
        "[ \"$HOMEBREW_NO_AUTO_UPDATE\" = 1 ] && [ \"$HOMEBREW_NO_ANALYTICS\" = 1 ] && [ \"$HOMEBREW_NO_ENV_HINTS\" = 1 ] || exit 7".to_owned(),
        "[ \"$*\" = 'outdated --json=v2' ] || exit 8".to_owned(),
        format!("printf '%s\\n' '{OUTDATED}'"),
        String::new(),
    ]
    .join("\n");
    write(&brew, script);
    std::fs::set_permissions(&brew, std::fs::Permissions::from_mode(0o755)).unwrap();
    Fx { base, root, home, brew, appcasts, brew_log }
}

fn item<'a>(items: &'a [Value], name: &str) -> &'a Value {
    items.iter().find(|i| i["name"] == name).unwrap_or_else(|| panic!("no {name}: {items:#?}"))
}

fn state(items: &[Value], name: &str) -> (String, Option<String>) {
    let i = item(items, name);
    (i["state"].as_str().unwrap().to_owned(), i["channel"].as_str().map(str::to_owned))
}

#[test]
fn apps_updates_reports_each_macos_channel() {
    let s = schema();
    let fx = fixture("list");
    // D15: gated until experimental features are on; nothing is checked then.
    let gated = fx.run(&["apps", "updates", "--json"]);
    assert_eq!((gated.code, gated.record["code"].as_str()), (7, Some("unsupported")), "{}", gated.record);
    assert_eq!(fx.brew_runs(), 0);
    assert_eq!(fx.run(&["experimental", "--state", "on", "--json"]).code, 0);

    let out = fx.run(&["apps", "updates", "--json"]);
    assert_eq!(out.code, 0, "{}", out.record);
    validate(&out.record, &s);
    assert_eq!((out.record["kind"].as_str(), out.record["checked"].as_bool()), (Some("app_update_list"), Some(true)));
    assert_eq!(fx.brew_runs(), 1, "brew outdated ran once");
    let items = out.record["items"].as_array().unwrap();
    assert_eq!(state(items, "Sparkly"), ("update_available".into(), Some("sparkle".into())));
    let sp = item(items, "Sparkly");
    assert_eq!((sp["current_version"].as_str(), sp["available_version"].as_str(), sp["route"].as_str()), (Some("1.0"), Some("1.2"), Some("open_app")));
    assert_eq!(state(items, "Firefox"), ("update_available".into(), Some("homebrew".into())));
    let ff = item(items, "Firefox");
    assert_eq!((ff["available_version"].as_str(), ff["route"].as_str(), ff["channel_id"].as_str()), (Some("131.0.2"), Some("package_manager"), Some("cask:firefox")));
    assert_eq!(state(items, "wget"), ("up_to_date".into(), Some("homebrew".into())));
    assert_eq!(state(items, "Pages"), ("cannot_check".into(), Some("mac_app_store".into())));
    assert_eq!(item(items, "Pages")["official_url"], "macappstore://showUpdatesPage");
    assert_eq!(state(items, "Chatty"), ("cannot_check".into(), Some("electron".into())));
    assert!(item(items, "Chatty")["reason"].as_str().unwrap().contains("electron-updater"));
    // UPD-01: no channel, or a feed that could not be read: cannot_check with a reason, never up to date.
    assert_eq!(state(items, "Plain"), ("cannot_check".into(), None));
    assert!(item(items, "Plain")["reason"].as_str().unwrap().starts_with("No update channel"));
    assert_eq!(state(items, "Offline"), ("cannot_check".into(), Some("sparkle".into())));
    assert!(item(items, "Offline")["reason"].as_str().unwrap().contains("could not be read"), "{}", item(items, "Offline"));
    assert!(items.iter().all(|i| i["checked_at"].is_string()));
    assert_eq!(out.record["counts"]["up_to_date"], 1);

    // `apps update`: only the Homebrew upgrade becomes a plan action; the rest is routed.
    let ids: Vec<&str> = ["Firefox", "Pages", "Sparkly", "Plain"].iter().map(|n| item(items, n)["app_instance_id"].as_str().unwrap()).collect();
    let plan = fx.run(&["apps", "update", "--app", &ids.join(","), "--json"]);
    assert_eq!(plan.code, 0, "{}", plan.record);
    validate(&plan.record, &s);
    let actions = plan.record["plan"]["body"]["items"].as_array().unwrap();
    assert_eq!(actions.len(), 1, "{:#}", plan.record);
    assert_eq!(actions[0]["argv"], serde_json::json!(["upgrade", "--cask", "firefox"]));
    assert_eq!(actions[0]["adapter_id"], "homebrew.upgrade");
    let routed = plan.record["routed"].as_array().unwrap();
    let route = |n: &str| routed.iter().find(|r| r["name"] == n).map(|r| (r["route"].as_str().unwrap().to_owned(), r["url"].as_str().map(str::to_owned))).unwrap();
    assert_eq!(route("Pages"), ("open_store".into(), Some("macappstore://showUpdatesPage".into())));
    assert_eq!(route("Sparkly").0, "open_app");
    assert_eq!(fx.brew_runs(), 1, "planning does not run brew");

    // UPD-04: checks off: no brew run, no appcast read; the last results keep their time.
    let cfg = fx.run(&["apps", "updates", "config", "--checks", "off", "--json"]);
    assert_eq!((cfg.code, cfg.record["checks_enabled"].as_bool()), (0, Some(false)));
    std::fs::remove_file(fx.appcasts.join("updates.example.test_sparkly_appcast.xml")).unwrap();
    let off = fx.run(&["apps", "updates", "--json"]);
    assert_eq!(off.code, 0, "{}", off.record);
    validate(&off.record, &s);
    assert_eq!((off.record["checked"].as_bool(), off.record["checks_enabled"].as_bool()), (Some(false), Some(false)));
    assert_eq!(off.record["checked_at"], out.record["checked_at"]);
    assert_eq!(fx.brew_runs(), 1, "checks off: brew is not run");
    assert_eq!(item(off.record["items"].as_array().unwrap(), "Sparkly")["state"], "update_available", "cached result shown");
    let show = fx.run(&["apps", "updates", "show", "--json"]);
    assert_eq!(show.code, 0);
    assert_eq!(fx.brew_runs(), 1);
}

#[test]
fn never_checked_with_checks_off_is_not_checked() {
    let fx = fixture("off");
    assert_eq!(fx.run(&["experimental", "--state", "on", "--json"]).code, 0);
    assert_eq!(fx.run(&["apps", "updates", "config", "--checks", "off", "--json"]).code, 0);
    let out = fx.run(&["apps", "updates", "--json"]);
    assert_eq!(out.code, 0, "{}", out.record);
    validate(&out.record, &schema());
    let items = out.record["items"].as_array().unwrap();
    assert!(!items.is_empty());
    assert!(items.iter().all(|i| i["state"] == "not_checked" && i["checked_at"].is_null()), "{items:#?}");
    assert_eq!(fx.brew_runs(), 0);
}
