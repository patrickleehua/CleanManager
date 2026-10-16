//! End-to-end CLI flow on macOS (MVP 9.5): scan → analyze → plan create (explicit `--candidates`) → authorization
//! receipt → plan apply → operations show / reconcile, in quarantine, delete and trash modes, on files created under
//! the canonical temporary directory. A fixture rule pack (`CM_RULES_DIR`, user-temp files at least 7 days old) and
//! `CM_RESOLVE_TEMP` keep rule resolution inside the fixture (docs/platforms/macos.md §11). Core scan/plan commands
//! are not gated, so experimental features stay off. The Trash leg moves a file to the real Trash and therefore runs
//! only with `CM_ALLOW_MAC_DEVICE_TESTS=1` or on a hosted runner (`GITHUB_ACTIONS=true`).
#![cfg(target_os = "macos")]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use cm_core::host::HostProbe;
use serde_json::{Value, json};

const RULE_ID: &str = "test.mac.user-temp.stale-files";

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn device_tests_allowed() -> bool {
    std::env::var("CM_ALLOW_MAC_DEVICE_TESTS").is_ok_and(|v| v == "1") || std::env::var("GITHUB_ACTIONS").is_ok_and(|v| v == "true")
}

/// A scratch tree under the canonical temporary directory (`/private/var/...`, never the `/var` link): `tmp` (the
/// resolved user-temp root and scan scope), `store` and `rules`. Removed on drop.
struct Fixture {
    base: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Fixture {
        let tmp = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
        let base = tmp.join(format!("cm-cli-mac-{tag}-{}-{}-{nanos:08x}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(base.join("tmp")).unwrap();
        write_rules(&base.join("rules"));
        Fixture { base }
    }
    fn tmp(&self) -> PathBuf {
        self.base.join("tmp")
    }
    fn store(&self) -> PathBuf {
        self.base.join("store")
    }
    /// A file older than the rule's 7-day threshold.
    fn stale(&self, name: &str, body: &[u8]) -> PathBuf {
        let p = self.tmp().join(name);
        std::fs::write(&p, body).unwrap();
        let month_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400);
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(month_ago).unwrap();
        p
    }
    fn cli(&self, args: &[&str]) -> (i32, Value) {
        let out = Command::new(env!("CARGO_BIN_EXE_cleanmanager"))
            .args(args)
            .arg("--store")
            .arg(self.store())
            .arg("--json")
            .env("CM_RULES_DIR", self.base.join("rules"))
            .env("CM_RESOLVE_TEMP", self.tmp())
            .env("CM_RESOLVE_HOME", self.base.join("home"))
            .output()
            .unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        assert_eq!(text.lines().count(), 1, "exactly one record for {args:?}: {text}\n{}", String::from_utf8_lossy(&out.stderr));
        (out.status.code().unwrap(), serde_json::from_str(&text).unwrap())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(self.tmp(), std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// One enabled macOS rule with its tested-registry entry: files directly in the user temp folder, 7+ days old.
fn write_rules(dir: &Path) {
    std::fs::create_dir_all(dir.join("macos")).unwrap();
    std::fs::create_dir_all(dir.join("registry")).unwrap();
    let thresholds = json!({"min_age_days": 7, "age_basis": "modified_time", "min_size_bytes": null, "retention_days": null});
    let rule = json!({
        "schema_version": "2.0", "rule_id": RULE_ID, "version": "1.0.0", "status": "enabled", "platforms": ["macos"],
        "category": "user_essentials", "max_tier": "review_only", "rebuild_class": "not_rebuildable",
        "owner": {"kind": "system", "id": "macos"}, "root_resolver": "user_temp",
        "match": {"object_type": "file", "relative_components": [], "name_glob": "*", "extensions": [], "require_cachedir_tag": false,
            "archive_payload": false, "max_depth": 0},
        "exclude": [], "probes": ["open_file_handle"], "thresholds": thresholds, "action_kind": "filesystem", "adapter_ref": null,
        "allowed_modes": ["trash", "quarantine", "delete"], "requires_admin": false, "protect_descendants": true, "follow_links": false,
        "impact": {"summary": "Test fixture: stale files directly in a redirected user temp folder.", "regeneration": "Not regenerated.",
            "offline_effect": "None; test fixture."},
        "tested_registry_ref": format!("{RULE_ID}@1.0.0"), "evidence_refs": ["fixture_mac_plan_flow"]
    });
    let registry = json!({
        "schema_version": "1.0", "registry_id": format!("{RULE_ID}@1.0.0"), "rule_id": RULE_ID, "rule_version": "1.0.0", "synthetic": false,
        "subject": {"kind": "system", "id": "macos", "name": "macOS user temporary folder (test fixture)"},
        "version_range": {"min_inclusive": "11.0", "max_exclusive": null},
        "platforms": [{"platform": "macos", "os_versions": ["14"], "architectures": ["arm64"]}],
        "tested_at": "2026-10-17", "result": "passed",
        "fixtures": {"positive": ["30-day-old top-level file"], "negative": ["fresh file"]},
        "known_effects": ["test fixture"], "evidence_refs": ["fixture_mac_plan_flow"], "thresholds": thresholds
    });
    std::fs::write(dir.join("macos").join(format!("{RULE_ID}.json")), serde_json::to_vec_pretty(&rule).unwrap()).unwrap();
    std::fs::write(dir.join("registry").join(format!("{RULE_ID}@1.0.0.json")), serde_json::to_vec_pretty(&registry).unwrap()).unwrap();
}

/// scan + analyze of the fixture's temp folder; candidate ids by file name.
fn analyze(fx: &Fixture) -> (String, Vec<(String, String)>) {
    let (code, scan) = fx.cli(&["scan", "--root", fx.tmp().to_str().unwrap()]);
    assert_eq!(code, 0, "{scan}");
    let (code, analysis) = fx.cli(&["analyze", "--scan", scan["scan_id"].as_str().unwrap()]);
    assert_eq!(code, 0, "{analysis}");
    let set = analysis["candidate_set_id"].as_str().unwrap().to_owned();
    let (_, page) = fx.cli(&["candidates", "list", "--set", &set]);
    let items = page["items"].as_array().cloned().unwrap_or_default();
    assert!(!items.is_empty(), "the fixture rule resolves to the redirected temp folder: {analysis}\n{page}");
    let ids = items
        .iter()
        .map(|c| {
            let name = c["path"]["display"].as_str().unwrap().rsplit('/').next().unwrap().to_owned();
            (name, c["candidate_id"].as_str().unwrap().to_owned())
        })
        .collect();
    (set, ids)
}

fn candidate<'a>(ids: &'a [(String, String)], name: &str) -> &'a str {
    &ids.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("{name} is a candidate: {ids:?}")).1
}

/// Create a plan for the named candidates and approve it with a receipt equivalent to the terminal review (which
/// needs a TTY), as the Windows flow tests do.
fn approved_plan(fx: &Fixture, set: &str, mode: &str, candidates: &[&str]) -> Value {
    let (code, plan) = fx.cli(&["plan", "create", "--set", set, "--mode", mode, "--candidates", &candidates.join(",")]);
    assert_eq!(code, 0, "{plan}");
    assert_eq!(plan["authorization_status"], "required");
    let plan_id = plan["body"]["plan_id"].as_str().unwrap().to_owned();
    // Without a receipt nothing runs (exit 3) and nothing is touched.
    let (code, err) = fx.cli(&["plan", "apply", &plan_id]);
    assert_eq!((code, err["code"].as_str()), (3, Some("needs_approval")), "{err}");
    let stores = cm_store::Stores::open(&fx.store()).unwrap();
    stores
        .audit
        .save_receipt(&cm_core::authz::Receipt {
            receipt_id: format!("r_{plan_id}"),
            plan_id: plan_id.clone(),
            plan_digest: plan["plan_digest"].as_str().unwrap().into(),
            mode: serde_json::from_value(json!(mode)).unwrap(),
            principal: cm_core::authz::current_principal(),
            source: cm_core::authz::ReceiptSource::InteractiveReview,
            policy_id: None,
            policy_version: None,
            created_at: cm_core::time::now_utc(),
            expires_at: plan["body"]["expires_at"].as_str().unwrap().into(),
            revoked_at: None,
        })
        .unwrap();
    plan
}

fn apply(fx: &Fixture, plan: &Value) -> Value {
    let plan_id = plan["body"]["plan_id"].as_str().unwrap();
    let (_, shown) = fx.cli(&["plan", "show", plan_id]);
    assert_eq!(shown["authorization_status"], "authorized", "{shown}");
    let (code, report) = fx.cli(&["plan", "apply", plan_id]);
    assert!(code == 0 || report["status"].is_string(), "{report}");
    report
}

#[test]
fn approved_plans_quarantine_and_delete_exactly_the_planned_files() {
    let fx = Fixture::new("flow");
    let q = fx.stale("quarantine-me.tmp", &[3u8; 8192]);
    let d = fx.stale("delete-me.tmp", &[4u8; 4096]);
    let fresh = fx.tmp().join("fresh.tmp");
    std::fs::write(&fresh, b"new").unwrap();
    let (set, ids) = analyze(&fx);
    assert!(!ids.iter().any(|(n, _)| n == "fresh.tmp"), "a fresh file is never a candidate: {ids:?}");

    // Quarantine: moved into the store's same-volume quarantine, recorded with its location.
    let plan = approved_plan(&fx, &set, "quarantine", &[candidate(&ids, "quarantine-me.tmp")]);
    let report = apply(&fx, &plan);
    assert_eq!((report["status"].as_str(), report["succeeded"].as_u64()), (Some("completed"), Some(1)), "{report}");
    assert_eq!(report["items"][0]["recovery"], "quarantine", "{report}");
    assert!(!q.exists());
    let op = report["operation_id"].as_str().unwrap().to_owned();
    let action = plan["body"]["items"][0]["action_id"].as_str().unwrap();
    let moved = fx.store().join("quarantine").join(&op).join(action);
    assert_eq!(std::fs::read(&moved).unwrap(), vec![3u8; 8192], "the planned file is in quarantine");
    // Re-applying replays the recorded operation instead of acting again.
    let (code, again) = fx.cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()]);
    assert_eq!((code, &again["operation_id"]), (0, &report["operation_id"]), "{again}");
    let (code, shown) = fx.cli(&["operations", "show", &op]);
    assert_eq!((code, shown["succeeded"].as_u64()), (0, Some(1)), "{shown}");
    // Reconciling a finished operation changes nothing.
    let (code, rec) = fx.cli(&["operations", "reconcile", &op]);
    assert_eq!((code, rec["succeeded"].as_u64()), (0, Some(1)), "{rec}");

    // Delete: permanent, only the planned file.
    let plan = approved_plan(&fx, &set, "delete", &[candidate(&ids, "delete-me.tmp")]);
    let report = apply(&fx, &plan);
    assert_eq!((report["status"].as_str(), report["succeeded"].as_u64()), (Some("completed"), Some(1)), "{report}");
    assert_eq!(report["items"][0]["recovery"], "unavailable", "{report}");
    assert!(!d.exists());
    assert_eq!(std::fs::read(&fresh).unwrap(), b"new", "unplanned files are untouched");
}

#[test]
fn a_changed_file_is_skipped_and_kept() {
    let fx = Fixture::new("stale");
    let f = fx.stale("grows.tmp", b"1");
    let (set, ids) = analyze(&fx);
    let plan = approved_plan(&fx, &set, "delete", &[candidate(&ids, "grows.tmp")]);
    std::fs::write(&f, b"12345").unwrap();
    let report = apply(&fx, &plan);
    assert_eq!(report["skipped"].as_u64(), Some(1), "{report}");
    assert_eq!(report["items"][0]["code"], "precondition_metadata_changed", "{report}");
    assert_eq!(std::fs::read(&f).unwrap(), b"12345");
}

#[test]
fn an_untrusted_scope_is_race_safety_unavailable_and_untouched() {
    let fx = Fixture::new("gate");
    let f = fx.stale("shared.tmp", b"keep");
    let (set, ids) = analyze(&fx);
    let plan = approved_plan(&fx, &set, "delete", &[candidate(&ids, "shared.tmp")]);
    // Another user could now swap names in the scope: the executor refuses instead of racing.
    std::fs::set_permissions(fx.tmp(), std::fs::Permissions::from_mode(0o777)).unwrap();
    let report = apply(&fx, &plan);
    assert_eq!(report["skipped"].as_u64(), Some(1), "{report}");
    let item = &report["items"][0];
    assert_eq!(item["code"], "race_safety_unavailable", "{report}");
    assert_eq!(std::fs::read(&f).unwrap(), b"keep");
}

/// EXE-05: an operation that stopped after its intents (a crash) is reconciled from the file system: the item found in
/// the operation's quarantine location is performed, the one still at its planned place was not.
#[test]
fn an_interrupted_operation_is_reconciled_from_the_file_system() {
    let fx = Fixture::new("reconcile");
    let a = fx.stale("a.tmp", b"moved before the crash");
    let b = fx.stale("b.tmp", b"never moved");
    let (set, ids) = analyze(&fx);
    let plan = approved_plan(&fx, &set, "quarantine", &[candidate(&ids, "a.tmp"), candidate(&ids, "b.tmp")]);
    let plan_id = plan["body"]["plan_id"].as_str().unwrap();
    let items = plan["body"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{plan}");
    let op = format!("op_reconcile{}", std::process::id());
    {
        let stores = cm_store::Stores::open(&fx.store()).unwrap();
        let host = cm_platform::host::OsHostProbe.current("cli");
        let now = cm_core::time::now_utc();
        stores.audit.begin_operation(&op, plan_id, plan["plan_digest"].as_str().unwrap(), &format!("dedupe-{op}"), &host, &now).unwrap();
        for item in items {
            let intent = json!({"mode": "quarantine", "action": "file", "path": item["path"], "identity": item["identity"]});
            stores.audit.record_intent(&op, item["action_id"].as_str().unwrap(), &intent.to_string(), &now).unwrap();
        }
    }
    // The crash happened after `a` was moved to the operation's quarantine location.
    let a_item = items.iter().find(|i| i["path"]["display"].as_str().unwrap().ends_with("/a.tmp")).unwrap();
    let qdir = fx.store().join("quarantine").join(&op);
    std::fs::create_dir_all(&qdir).unwrap();
    std::fs::rename(&a, qdir.join(a_item["action_id"].as_str().unwrap())).unwrap();

    let (_, rec) = fx.cli(&["operations", "reconcile", &op]);
    let by_path = |name: &str| {
        let id = items.iter().find(|i| i["path"]["display"].as_str().unwrap().ends_with(name)).unwrap()["action_id"].clone();
        rec["items"].as_array().unwrap().iter().find(|r| r["action_id"] == id).cloned().unwrap()
    };
    assert_eq!(by_path("/a.tmp")["code"], "reconciled_performed", "{rec}");
    assert_eq!(by_path("/a.tmp")["recovery"], "quarantine", "{rec}");
    assert_eq!(by_path("/b.tmp")["code"], "reconciled_not_performed", "{rec}");
    assert_eq!(std::fs::read(&b).unwrap(), b"never moved");
}

#[test]
fn an_approved_trash_plan_moves_the_file_to_the_trash() {
    if !device_tests_allowed() {
        eprintln!("skipped: set CM_ALLOW_MAC_DEVICE_TESTS=1 to move a test file to your Trash");
        return;
    }
    let fx = Fixture::new("trash");
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let name = format!("cm-trash-flow-{}-{nanos}.tmp", std::process::id());
    let f = fx.stale(&name, b"to the Trash");
    let (set, ids) = analyze(&fx);
    let plan = approved_plan(&fx, &set, "trash", &[candidate(&ids, &name)]);
    let report = apply(&fx, &plan);
    // Remove the test's own item from the Trash by its exact name (nothing is listed there).
    let in_trash = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".Trash").join(&name));
    if let Some(p) = &in_trash
        && let Err(e) = std::fs::remove_file(p)
    {
        eprintln!("could not remove {} from the Trash: {e}", p.display());
    }
    assert_eq!((report["status"].as_str(), report["succeeded"].as_u64()), (Some("completed"), Some(1)), "{report}");
    assert_eq!(report["items"][0]["recovery"], "trash", "{report}");
    assert!(!f.exists());
    let op = report["operation_id"].as_str().unwrap();
    let (code, rec) = fx.cli(&["operations", "reconcile", op]);
    assert_eq!((code, rec["succeeded"].as_u64()), (0, Some(1)), "{rec}");
}
