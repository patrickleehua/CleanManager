//! MVP 12.1 (CLI-01/02/03/04/06), parity 8.2 (CLI-12) and 17.5 (LIF-05) through the real binary:
//! read-only commands default to one machine record when stdout is not a terminal, with no BOM,
//! ANSI or log lines; every record names a schema kind and carries exactly the schema's top-level
//! fields; pagination is bounded and cursor-bound; argument errors are typed; no command accepts
//! a self-approval flag; and machine JSON is identical in every human language.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-contract-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn schema() -> Value {
    serde_json::from_str(include_str!("../../../schemas/v1/protocol.schema.json")).unwrap()
}

struct Out {
    code: i32,
    stdout: Vec<u8>,
}

fn raw(store: &Path, args: &[&str], env: &[(&str, &str)]) -> Out {
    let mut c = Command::new(env!("CARGO_BIN_EXE_cleanmanager"));
    c.args(args).env("CM_STORE_DIR", store).env_remove("CLEANMANAGER_LANG");
    for (k, v) in env {
        c.env(k, v);
    }
    let o = c.output().unwrap();
    Out { code: o.status.code().unwrap(), stdout: o.stdout }
}

/// Stdout hygiene plus the schema's envelope: kind in `oneOf`, exactly the declared top-level fields.
fn record(out: &Out, schema: &Value) -> Value {
    assert!(!out.stdout.starts_with(b"\xef\xbb\xbf"), "BOM on stdout");
    assert!(!out.stdout.contains(&0x1b), "ANSI escape on stdout");
    let text = std::str::from_utf8(&out.stdout).expect("UTF-8 stdout");
    assert_eq!(text.lines().count(), 1, "exactly one record and no log lines: {text}");
    assert!(text.ends_with('\n') && !text.trim_end().contains('\n'));
    let v: Value = serde_json::from_str(text.trim_end()).expect("JSON record");
    let kind = v["kind"].as_str().expect("kind");
    assert_eq!(v["schema_version"], "1.0");
    assert!(v["request_id"].as_str().is_some_and(|r| r.starts_with("req_")));
    let def = &schema["$defs"][kind];
    assert!(def.is_object(), "kind {kind} is not in schemas/v1");
    let props = def["properties"].as_object().unwrap();
    for k in v.as_object().unwrap().keys() {
        assert!(props.contains_key(k), "{kind}: field {k} is not in the schema");
    }
    for r in def["required"].as_array().unwrap() {
        assert!(v.get(r.as_str().unwrap()).is_some(), "{kind}: required field {r} missing");
    }
    v
}

fn fixture(base: &Path) -> PathBuf {
    let root = base.join("tree");
    for d in ["alpha", "beta", "gamma"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
        for (i, n) in [10usize, 2000, 300].iter().enumerate() {
            std::fs::write(root.join(d).join(format!("f{i}.bin")), vec![b'x'; *n]).unwrap();
        }
    }
    std::fs::write(root.join("中文 名称.txt"), "unicode").unwrap();
    root
}

#[test]
fn read_only_commands_emit_one_schema_record_without_a_terminal() {
    let s = schema();
    let base = scratch("readonly");
    let (root, store) = (fixture(&base), base.join("store"));
    // No --json: stdout is a pipe, so machine JSON is the default (CLI-06).
    let scan = record(&raw(&store, &["scan", "--root", root.to_str().unwrap()], &[]), &s);
    assert_eq!((scan["kind"].as_str(), scan["status"].as_str()), (Some("scan_result"), Some("completed")));
    let sid = scan["scan_id"].as_str().unwrap().to_owned();
    for args in [
        vec!["capabilities"],
        vec!["scan", "show", &sid],
        vec!["scan", "wait", &sid, "--timeout", "0"],
        vec!["entries", "list", "--scan", &sid],
        vec!["entries", "list", "--scan", &sid, "--kind", "file", "--sort", "name_asc", "--name", "f1"],
        vec!["operations", "list"],
        vec!["recovery", "list"],
        vec!["quarantine", "list"],
        vec!["protect", "list"],
        vec!["clean", "list"],
        vec!["doctor"],
        vec!["completion", "print", "bash"],
    ] {
        let out = raw(&store, &args, &[]);
        let v = record(&out, &s);
        assert_eq!(out.code, 0, "{args:?}: {v}");
        assert_ne!(v["kind"], "error", "{args:?}");
    }
    // Paths keep the raw platform encoding beside the display form (CLI-04).
    let page = record(&raw(&store, &["entries", "list", "--scan", &sid, "--name", "名称"], &[]), &s);
    let item = &page["items"][0];
    assert!(item["path"]["display"].as_str().unwrap().ends_with("中文 名称.txt"), "{page}");
    assert!(item["path"]["raw_base64"].is_string());
    assert!(item["logical_bytes"].is_string(), "byte counts are decimal strings");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn pages_are_bounded_complete_and_bound_to_their_cursor() {
    let s = schema();
    let base = scratch("pages");
    let (root, store) = (fixture(&base), base.join("store"));
    let sid = record(&raw(&store, &["scan", "--root", root.to_str().unwrap()], &[]), &s)["scan_id"].as_str().unwrap().to_owned();
    let all = record(&raw(&store, &["entries", "list", "--scan", &sid, "--limit", "500"], &[]), &s);
    let total = all["items"].as_array().unwrap().len();
    assert!(all["next_cursor"].is_null());
    let default = record(&raw(&store, &["entries", "list", "--scan", &sid], &[]), &s);
    assert_eq!(default["page_size"], 50, "default page size");
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut args = vec!["entries", "list", "--scan", &sid, "--limit", "4"];
        if let Some(c) = &cursor {
            args.extend(["--cursor", c.as_str()]);
        }
        let page = record(&raw(&store, &args, &[]), &s);
        assert!(page["items"].as_array().unwrap().len() <= 4);
        seen.extend(page["items"].as_array().unwrap().iter().map(|i| i["entry_id"].as_str().unwrap().to_owned()));
        match page["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => break,
        }
    }
    let unique: std::collections::BTreeSet<_> = seen.iter().collect();
    assert_eq!((seen.len(), unique.len()), (total, total), "no duplicates, nothing missing");
    let first = record(&raw(&store, &["entries", "list", "--scan", &sid, "--limit", "4"], &[]), &s);
    let c = first["next_cursor"].as_str().unwrap();
    for (args, code, err) in [
        (vec!["entries", "list", "--scan", &sid, "--limit", "0"], 2, "invalid_argument"),
        (vec!["entries", "list", "--scan", &sid, "--limit", "501"], 2, "invalid_argument"),
        (vec!["entries", "list", "--scan", &sid, "--limit", "5", "--cursor", c], 6, "cursor_stale"),
        (vec!["entries", "list", "--scan", &sid, "--limit", "4", "--sort", "name_asc", "--cursor", c], 6, "cursor_stale"),
        (vec!["entries", "list", "--scan", &sid, "--cursor", "garbage"], 6, "cursor_stale"),
        (vec!["operations", "list", "--limit", "501"], 2, "invalid_argument"),
    ] {
        let out = raw(&store, &args, &[]);
        let v = record(&out, &s);
        assert_eq!((out.code, v["code"].as_str()), (code, Some(err)), "{args:?}: {v}");
    }
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn argument_errors_are_typed_and_never_guessed() {
    let s = schema();
    let base = scratch("args");
    let store = base.join("store");
    for args in [
        vec!["no-such-command"],
        vec![],
        vec!["capabilities", "--root", "x"],
        vec!["capabilities", "--format", "yaml"],
        vec!["capabilities", "--json", "--format", "text"],
        vec!["capabilities", "--lang", "tlh"],
        vec!["scan", "show", "s1", "--store"],
        vec!["entries", "list"],
        vec!["entries", "list", "--scan", "s1", "--sort", "biggest"],
        vec!["plan", "apply", "p1", "-y"],
        vec!["completion", "print", "tcsh"],
    ] {
        let out = raw(&store, &args, &[]);
        let v = record(&out, &s);
        assert_eq!((out.code, v["code"].as_str()), (2, Some("invalid_argument")), "{args:?}: {v}");
        assert_eq!(v["retryable"], false);
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// Parity 8.2 / CLI-12: no command group accepts a `--yes`-style approval, and widening a
/// selection without a terminal is refused before anything runs; nothing is planned or changed.
#[test]
fn no_self_approval_and_no_widening_for_machine_callers() {
    let s = schema();
    let base = scratch("approval");
    let store = base.join("store");
    for args in [
        vec!["plan", "apply", "p_x"],
        vec!["plan", "create", "--set", "cs_x", "--mode", "trash"],
        vec!["clean", "--scan", "s_x", "--mode", "trash"],
        vec!["purge", "plan", "create"],
        vec!["installer", "plan", "create"],
        vec!["recovery", "plan", "create", "--restore", "q_x"],
        vec!["protect", "remove", "u_x"],
        vec!["policy", "enable", "pol_x"],
        vec!["completion", "install", "bash"],
        vec!["doctor", "--export", "x.json"],
    ] {
        for flag in ["--yes", "--force", "--approve", "--assume-yes"] {
            let mut a = args.clone();
            a.extend([flag, "true"]);
            let out = raw(&store, &a, &[]);
            let v = record(&out, &s);
            assert_eq!((out.code, v["code"].as_str()), (2, Some("invalid_argument")), "{a:?}: {v}");
            assert!(v["message"].as_str().unwrap().contains(&flag[2..]), "{a:?}: {v}");
        }
    }
    let out = raw(&store, &["installer", "plan", "create", "--source", "downloads", "--non-interactive"], &[]);
    let v = record(&out, &s);
    assert_eq!((out.code, v["code"].as_str()), (2, Some("invalid_argument")));
    assert!(v["message"].as_str().unwrap().contains("--candidates"), "{v}");
    // Without --non-interactive a pipe is still not a terminal: same refusal.
    let out = raw(&store, &["installer", "plan", "create", "--source", "downloads"], &[]);
    assert_eq!(out.code, 2);
    // Nothing above created a plan or touched the store's history.
    let ops = record(&raw(&store, &["operations", "list"], &[]), &s);
    assert!(ops["items"].as_array().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&base);
}

/// Parity 17.5 / LIF-05: human text follows the language; machine JSON (kind, code, status and every
/// other field) is identical apart from the per-call request id and clock readings.
#[test]
fn machine_json_is_identical_in_every_language() {
    let s = schema();
    let base = scratch("lang");
    let (root, store) = (fixture(&base), base.join("store"));
    let sid = record(&raw(&store, &["scan", "--root", root.to_str().unwrap()], &[]), &s)["scan_id"].as_str().unwrap().to_owned();
    let normalize = |mut v: Value| {
        for k in ["request_id", "generated_at", "requested_at"] {
            if v.get(k).is_some() {
                v[k] = Value::Null;
            }
        }
        v
    };
    for args in [
        vec!["capabilities"],
        vec!["scan", "show", &sid],
        vec!["entries", "list", "--scan", &sid, "--limit", "3"],
        vec!["no-such-command"],
        vec!["entries", "list", "--scan", &sid, "--limit", "0"],
        vec!["scan", "cancel", &sid],
        vec!["doctor"],
        vec!["help"],
    ] {
        let en = raw(&store, &[args.as_slice(), &["--lang", "en"]].concat(), &[]);
        let zh = raw(&store, &[args.as_slice(), &["--lang", "zh-CN"]].concat(), &[]);
        let env_zh = raw(&store, &args, &[("CLEANMANAGER_LANG", "zh-CN")]);
        assert_eq!((en.code, zh.code), (env_zh.code, env_zh.code), "{args:?}");
        let (a, b, c) = (normalize(record(&en, &s)), normalize(record(&zh, &s)), normalize(record(&env_zh, &s)));
        assert_eq!(a, b, "{args:?}: JSON differs between en and zh-CN");
        assert_eq!(a, c, "{args:?}: JSON differs with CLEANMANAGER_LANG");
    }
    // Human text does change, and keeps the machine values verbatim.
    let en = raw(&store, &["scan", "show", &sid, "--format", "text", "--lang", "en"], &[]);
    let zh = raw(&store, &["scan", "show", &sid, "--format", "text", "--lang", "zh-CN"], &[]);
    let (en, zh) = (String::from_utf8(en.stdout).unwrap(), String::from_utf8(zh.stdout).unwrap());
    assert!(en.starts_with("Scan result [scan_result]") && zh.starts_with("扫描结果 [scan_result]"), "{en}\n{zh}");
    assert!(en.contains("status: completed") && zh.contains("status: completed"));
    assert!(!zh.contains('\u{1b}'));
    let err = String::from_utf8(raw(&store, &["plan", "apply", "p_none", "--format", "text", "--lang", "zh-CN"], &[]).stdout).unwrap();
    assert!(err.starts_with("错误 ["), "{err}");
    let help = String::from_utf8(raw(&store, &["help", "--format", "text", "--lang", "zh-CN"], &[]).stdout).unwrap();
    assert!(help.contains("扫描指定根") && help.contains("scan wait"), "{help}");
    let _ = std::fs::remove_dir_all(&base);
}
