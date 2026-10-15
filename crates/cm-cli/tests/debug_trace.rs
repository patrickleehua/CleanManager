//! CLI-14 (parity 8.4): the global `--debug` trace goes to stderr only. Every command group below is run twice
//! against the same store, without and with `--debug`: stdout must be the same bytes once the fields that differ
//! on every run anyway (request-derived IDs, digests, timestamps, live free space) are normalized, exit codes must
//! match, stderr must carry phases with durations and the chosen backend, and nothing is traced without the flag.
//! A configured (throw-away) AI provider key never appears in the trace; only its presence does.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-debug-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// D15: gated tools are off by default; opt in the way a person does. Ignored by builds without the gate.
fn experimental_on(store: &Path) {
    let _ = Command::new(env!("CARGO_BIN_EXE_cleanmanager")).args(["experimental", "--state", "on", "--json"]).env("CM_STORE_DIR", store).output();
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run(args: &[&str], store: &Path, local: &Path, debug: bool) -> Run {
    let mut c = Command::new(env!("CARGO_BIN_EXE_cleanmanager"));
    c.args(args).arg("--store").arg(store).arg("--json");
    if debug {
        c.arg("--debug");
    }
    let out = c
        .env("CM_RESOLVE_LOCALAPPDATA", local)
        .env("CM_RESOLVE_TEMP", local.join("Temp"))
        .env_remove("CLEANMANAGER_LANG")
        .output()
        .unwrap();
    Run { code: out.status.code().unwrap(), stdout: String::from_utf8(out.stdout).unwrap(), stderr: String::from_utf8(out.stderr).unwrap() }
}

fn record(r: &Run) -> Value {
    assert_eq!(r.stdout.lines().count(), 1, "exactly one record: {}", r.stdout);
    serde_json::from_str(&r.stdout).unwrap()
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// `req_<pid>_<nanos>`-derived IDs (`s_…`, `p_…`) and digests/hash-derived IDs (`op_<16 hex>`, 64-hex digests).
fn token(t: &str) -> String {
    let parts: Vec<&str> = t.split('_').collect();
    if parts.len() >= 3 && parts[0].bytes().all(|b| b.is_ascii_lowercase()) && is_hex(parts[1]) && is_hex(parts[2]) && parts[2].len() >= 8 {
        return format!("{}_<id>{}", parts[0], parts[3..].iter().map(|p| format!("_{p}")).collect::<String>());
    }
    match parts.split_last() {
        Some((last, head)) if is_hex(last) && last.len() >= 16 => head.iter().map(|p| format!("{p}_")).collect::<String>() + "<hash>",
        _ => t.to_owned(),
    }
}

/// RFC 3339 UTC timestamp starting at `i` (`2026-10-16T12:34:56Z`, optional fraction): its length.
fn timestamp_len(b: &[u8], i: usize) -> Option<usize> {
    let shape = b"dddd-dd-ddTdd:dd:dd";
    let s = b.get(i..i + shape.len())?;
    if !s.iter().zip(shape).all(|(c, k)| if *k == b'd' { c.is_ascii_digit() } else { c == k }) {
        return None;
    }
    let mut j = i + shape.len();
    if b.get(j) == Some(&b'.') {
        j += 1;
        while b.get(j).is_some_and(u8::is_ascii_digit) {
            j += 1;
        }
    }
    (b.get(j) == Some(&b'Z')).then_some(j + 1 - i)
}

/// Normalize the raw stdout text in place (layout and every other byte kept).
fn normalize(text: &str, blank_keys: &[&str]) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        if let Some(n) = timestamp_len(b, i) {
            out.push_str("<time>");
            i += n;
        } else if b[i].is_ascii_alphanumeric() || b[i] == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push_str(&token(&text[start..i]));
        } else {
            let ch = text[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    // Live values (free space): blank the value of each named key, string or null.
    for key in blank_keys {
        let pat = format!("\"{key}\":");
        let mut rest = out.as_str();
        let mut acc = String::new();
        while let Some(pos) = rest.find(&pat) {
            acc.push_str(&rest[..pos + pat.len()]);
            rest = &rest[pos + pat.len()..];
            let end = rest.find([',', '}']).unwrap_or(rest.len());
            acc.push_str("<live>");
            rest = &rest[end..];
        }
        acc.push_str(rest);
        out = acc;
    }
    out
}

const LIVE: &[&str] = &["volume_free_before_bytes", "volume_free_after_bytes", "observed_free_delta_bytes"];

#[test]
fn normalization_only_hides_per_run_fields() {
    let a = r#"{"finished_at":"2026-10-15T16:36:28Z","logical_bytes":"3","request_id":"req_708c_18dec14157d009d4","scan_id":"s_708c_18dec14157d009d4","op":"op_0123456789abcdef","volume_free_after_bytes":"100"}"#;
    let b = r#"{"finished_at":"2026-10-16T01:02:03.5Z","logical_bytes":"3","request_id":"req_1f_18dec1416a6119d0","scan_id":"s_1f_18dec1416a6119d0","op":"op_fedcba9876543210","volume_free_after_bytes":null}"#;
    assert_eq!(normalize(a, LIVE), normalize(b, LIVE));
    assert_ne!(normalize(a, LIVE), normalize(&a.replace("\"3\"", "\"4\""), LIVE), "real values still compare");
    assert_ne!(normalize(a, LIVE), normalize(&a.replace("scan_id", "scan_ID"), LIVE));
}

/// Trace lines are `[debug +Nms] phase: message`.
fn assert_trace_shape(stderr: &str) {
    assert!(!stderr.is_empty(), "--debug writes a trace");
    for line in stderr.lines() {
        let rest = line.strip_prefix("[debug +").unwrap_or_else(|| panic!("trace line shape: {line}"));
        let (ms, rest) = rest.split_once("ms] ").unwrap_or_else(|| panic!("trace line shape: {line}"));
        assert!(ms.parse::<u64>().is_ok(), "{line}");
        assert!(rest.contains(": "), "{line}");
    }
}

/// Run `args` without and with `--debug` against the same store; stdout must match after normalization.
fn pair(args: &[&str], store: &Path, local: &Path) -> (Run, Run) {
    let plain = run(args, store, local, false);
    let traced = run(args, store, local, true);
    assert_eq!(plain.code, traced.code, "{args:?}: exit code\n{}\n{}", plain.stdout, traced.stdout);
    assert_eq!(normalize(&plain.stdout, LIVE), normalize(&traced.stdout, LIVE), "{args:?}: stdout differs with --debug");
    assert!(plain.stderr.is_empty(), "{args:?}: no trace without --debug: {}", plain.stderr);
    assert_trace_shape(&traced.stderr);
    assert!(traced.stderr.contains("cli.run: done in "), "{args:?}: {}", traced.stderr);
    (plain, traced)
}

fn old_file(path: &Path, bytes: usize) {
    std::fs::write(path, vec![7u8; bytes]).unwrap();
    let month_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400);
    std::fs::File::options().write(true).open(path).unwrap().set_modified(month_ago).unwrap();
}

fn approve(store: &Path, plan: &Value) {
    let stores = cm_store::Stores::open(store).unwrap();
    stores
        .audit
        .save_receipt(&cm_core::authz::Receipt {
            receipt_id: format!("r_{}", plan["body"]["plan_id"].as_str().unwrap()),
            plan_id: plan["body"]["plan_id"].as_str().unwrap().into(),
            plan_digest: plan["plan_digest"].as_str().unwrap().into(),
            mode: cm_core::rules::Mode::Quarantine,
            principal: cm_core::authz::current_principal(),
            source: cm_core::authz::ReceiptSource::InteractiveReview,
            policy_id: None,
            policy_version: None,
            created_at: cm_core::time::now_utc(),
            expires_at: plan["body"]["expires_at"].as_str().unwrap().into(),
            revoked_at: None,
        })
        .unwrap();
}

#[test]
fn debug_changes_no_stdout_record_and_traces_phases_backends_and_outcomes() {
    let base = scratch("groups");
    let (local, store) = (base.join("local"), base.join("store"));
    let temp = local.join("Temp");
    std::fs::create_dir_all(&temp).unwrap();
    std::fs::create_dir_all(&store).unwrap();
    experimental_on(&store);
    old_file(&temp.join("old1.tmp"), 8192);
    old_file(&temp.join("old2.tmp"), 8192);
    std::fs::write(temp.join("fresh.tmp"), b"new").unwrap();

    // Read-only groups.
    pair(&["capabilities"], &store, &local);
    pair(&["clean", "list"], &store, &local);
    pair(&["protect", "list"], &store, &local);
    pair(&["optimize", "list"], &store, &local);
    let (_, traced) = pair(&["diag", "config"], &store, &local);
    assert!(traced.stderr.contains("diag: configured="), "{}", traced.stderr);
    pair(&["doctor"], &store, &local);

    // Scan: same scan_result record; the trace names phases with durations and the backend.
    let (plain, traced) = pair(&["scan", "--root", temp.to_str().unwrap()], &store, &local);
    for needle in ["scan: scan_id=", "backend=", "scan.walk: start", "scan.walk: done in ", "scan.publish: done in ", "scan: published state="] {
        assert!(traced.stderr.contains(needle), "missing {needle:?} in\n{}", traced.stderr);
    }
    let scan = record(&plain);
    assert_eq!(scan["kind"], "scan_result");
    let scan_id = scan["scan_id"].as_str().unwrap().to_owned();
    // Paths follow the Doctor display rules: the home folder is `~`, in plain and in escaped form.
    let home = std::env::var("USERPROFILE").unwrap().to_lowercase();
    let lower = traced.stderr.to_lowercase();
    assert!(!lower.contains(&home) && !lower.contains(&home.replace('\\', "\\\\")), "{}", traced.stderr);
    if temp.to_string_lossy().to_lowercase().starts_with(&home) {
        assert!(traced.stderr.contains("root=\"~\\"), "{}", traced.stderr);
    }

    pair(&["entries", "list", "--scan", &scan_id], &store, &local);
    let (plain, _) = pair(&["analyze", "--scan", &scan_id], &store, &local);
    let analysis = record(&plain);
    let set = analysis["candidate_set_id"].as_str().unwrap().to_owned();
    let (_, traced) = pair(&["clean", "--scan", &scan_id, "--dry-run", "--non-interactive"], &store, &local);
    assert!(traced.stderr.contains("clean.analysis: done in ") && traced.stderr.contains("clean: scan="), "{}", traced.stderr);

    // Plans: one per candidate; creating the same selection with and without --debug gives the same record.
    let page = record(&run(&["candidates", "list", "--set", &set], &store, &local, false));
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{page}");
    let (c1, c2) = (items[0]["candidate_id"].as_str().unwrap(), items[1]["candidate_id"].as_str().unwrap());
    let (plain, traced) = pair(&["plan", "create", "--set", &set, "--mode", "quarantine", "--candidates", c1], &store, &local);
    assert!(traced.stderr.contains("plan: set=") && traced.stderr.contains("plan: built plan="), "{}", traced.stderr);
    let p1 = record(&plain);
    let p1_id = p1["body"]["plan_id"].as_str().unwrap().to_owned();
    pair(&["plan", "show", &p1_id], &store, &local);
    // No receipt: the same needs_approval record either way, and nothing is touched.
    let (_, traced) = pair(&["plan", "apply", &p1_id], &store, &local);
    assert!(traced.stderr.contains("apply: plan=") && traced.stderr.contains("stored_type="), "{}", traced.stderr);
    assert!(temp.join("old1.tmp").exists() && temp.join("old2.tmp").exists());

    // Approved apply of two equal plans (one file each): the same operation_report, per-action outcome on stderr only.
    let p2 = record(&run(&["plan", "create", "--set", &set, "--mode", "quarantine", "--candidates", c2], &store, &local, false));
    approve(&store, &p1);
    approve(&store, &p2);
    let plain = run(&["plan", "apply", &p1_id], &store, &local, false);
    let traced = run(&["plan", "apply", p2["body"]["plan_id"].as_str().unwrap()], &store, &local, true);
    assert_eq!((plain.code, traced.code), (0, 0), "{}\n{}", plain.stdout, traced.stdout);
    assert!(plain.stderr.is_empty());
    assert_eq!(normalize(&plain.stdout, LIVE), normalize(&traced.stdout, LIVE), "operation_report differs with --debug");
    assert_eq!(record(&traced)["succeeded"], 1);
    assert_trace_shape(&traced.stderr);
    for needle in ["apply: operation=", "status=completed", "authorization_source=interactive_review", "apply.action: ", "code=done", "apply.run: done in "] {
        assert!(traced.stderr.contains(needle), "missing {needle:?} in\n{}", traced.stderr);
    }
    assert!(!temp.join("old1.tmp").exists() && !temp.join("old2.tmp").exists() && temp.join("fresh.tmp").exists());
    // The receipt is never traced: only its source label.
    assert!(!traced.stderr.contains("r_p_"), "{}", traced.stderr);
    std::fs::remove_dir_all(&base).ok();
}

/// Removes the throw-away credential even when an assertion fails.
struct TestKey(String);

impl Drop for TestKey {
    fn drop(&mut self) {
        let _ = cm_platform::credentials::os_store().delete(&self.0);
    }
}

#[test]
fn debug_trace_names_key_presence_never_the_key() {
    let base = scratch("diag");
    let store = base.join("store");
    std::fs::create_dir_all(&store).unwrap();
    experimental_on(&store);
    let provider = format!("test-debug-{}", std::process::id());
    let key = format!("sk-test-not-a-real-key-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let target = cm_platform::credentials::target_for(&provider).unwrap();
    let creds = cm_platform::credentials::os_store();
    creds.write(&target, &cm_platform::credentials::Secret::new(key.clone()).unwrap()).unwrap();
    let _cleanup = TestKey(target);
    let config = serde_json::json!({"version": 1, "default_provider_id": provider, "providers": [{
        "provider_id": provider, "label": "debug trace test", "kind": "anthropic", "base_url": "https://api.anthropic.com",
        "model": "claude-opus-5-5", "executable": null, "updated_at": "2026-01-01T00:00:00Z",
    }]});
    std::fs::write(store.join(cm_app::diag::CONFIG_FILE), serde_json::to_string_pretty(&config).unwrap()).unwrap();

    let (plain, traced) = pair(&["diag", "config"], &store, &base);
    let status = record(&plain);
    assert_eq!(status["providers"][0]["credential"], "present", "{status}");
    assert!(traced.stderr.contains(&format!("provider={provider}")) && traced.stderr.contains("key=present"), "{}", traced.stderr);
    // Without a scan the preview fails at once; neither run may echo the key on any stream.
    let (plain_pv, traced_pv) = pair(&["diag", "preview", "--scan", "s_missing", "--entry", "1"], &store, &base);
    for r in [&plain, &traced, &plain_pv, &traced_pv] {
        assert!(!r.stderr.contains(&key) && !r.stdout.contains(&key), "the key leaked");
        assert!(!r.stderr.contains("sk-test-not-a-real-key"), "no part of the key is traced");
    }
    std::fs::remove_dir_all(&base).ok();
}
