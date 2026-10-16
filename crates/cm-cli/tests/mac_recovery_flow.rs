//! End-to-end recovery on macOS (MVP 10.4 macOS half, G1 core path): scan → analyze → plan (quarantine) → receipt →
//! apply → quarantine list → recovery plan (restore) → receipt → apply → the same inode is back; plus the refusal
//! paths (occupied original name, replaced quarantined object, replaced or untrusted original folder, cleanup and
//! other-kind approvals), a separately authorized purge that removes only quarantine content, reconciliation of an
//! interrupted restore and the REC-05 report (a same-volume move frees nothing; a negative observed delta is reported
//! as observed). Files live under the canonical temporary directory; a fixture rule pack (`CM_RULES_DIR`) and
//! `CM_RESOLVE_TEMP` keep rule resolution inside the fixture (docs/platforms/macos.md §11). Receipts are written
//! through `cm_store` as the trusted terminal review would (it needs a TTY), as the Windows flow tests do.
#![cfg(target_os = "macos")]

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use cm_core::execute::Reconciled;
use cm_core::host::HostProbe;
use cm_core::path::RawPath;
use cm_core::recovery::{MetadataCoverage, QuarantineState, QuarantinedObject, RecoveryActions, RecoveryKind, RecoveryOutcome};
use serde_json::{Value, json};

const RULE_ID: &str = "test.mac.user-temp.stale-files";

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A scratch tree under the canonical temporary directory: `tmp` (the resolved user-temp root and scan scope),
/// `store` and `rules`. Removed on drop.
struct Fixture {
    base: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Fixture {
        let tmp = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
        let base = tmp.join(format!("cm-cli-macrec-{tag}-{}-{}-{nanos:08x}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
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
        "tested_registry_ref": format!("{RULE_ID}@1.0.0"), "evidence_refs": ["fixture_mac_recovery_flow"]
    });
    let registry = json!({
        "schema_version": "1.0", "registry_id": format!("{RULE_ID}@1.0.0"), "rule_id": RULE_ID, "rule_version": "1.0.0", "synthetic": false,
        "subject": {"kind": "system", "id": "macos", "name": "macOS user temporary folder (test fixture)"},
        "version_range": {"min_inclusive": "11.0", "max_exclusive": null},
        "platforms": [{"platform": "macos", "os_versions": ["14"], "architectures": ["arm64"]}],
        "tested_at": "2026-10-17", "result": "passed",
        "fixtures": {"positive": ["30-day-old top-level file"], "negative": ["fresh file"]},
        "known_effects": ["test fixture"], "evidence_refs": ["fixture_mac_recovery_flow"], "thresholds": thresholds
    });
    std::fs::write(dir.join("macos").join(format!("{RULE_ID}.json")), serde_json::to_vec_pretty(&rule).unwrap()).unwrap();
    std::fs::write(dir.join("registry").join(format!("{RULE_ID}@1.0.0.json")), serde_json::to_vec_pretty(&registry).unwrap()).unwrap();
}

fn ino(p: &Path) -> u64 {
    std::fs::symlink_metadata(p).unwrap().ino()
}

/// scan → analyze → quarantine plan for every candidate → no approval refused → cleanup receipt → apply. Returns the
/// cleanup report.
fn quarantine_all(fx: &Fixture, expected: usize) -> Value {
    let (code, scan) = fx.cli(&["scan", "--root", fx.tmp().to_str().unwrap()]);
    assert_eq!(code, 0, "{scan}");
    let (code, analysis) = fx.cli(&["analyze", "--scan", scan["scan_id"].as_str().unwrap()]);
    assert_eq!(code, 0, "{analysis}");
    let set = analysis["candidate_set_id"].as_str().unwrap().to_owned();
    let (_, page) = fx.cli(&["candidates", "list", "--set", &set]);
    let cands: Vec<&str> = page["items"].as_array().unwrap().iter().map(|c| c["candidate_id"].as_str().unwrap()).collect();
    assert_eq!(cands.len(), expected, "{page}");
    let (code, plan) = fx.cli(&["plan", "create", "--set", &set, "--mode", "quarantine", "--candidates", &cands.join(",")]);
    assert_eq!(code, 0, "{plan}");
    let plan_id = plan["body"]["plan_id"].as_str().unwrap().to_owned();
    let (code, err) = fx.cli(&["plan", "apply", &plan_id]);
    assert_eq!((code, err["code"].as_str()), (3, Some("needs_approval")), "nothing runs without a receipt: {err}");
    cleanup_receipt(fx, &plan);
    let (code, report) = fx.cli(&["plan", "apply", &plan_id]);
    assert_eq!(code, 0, "{report}");
    report
}

fn cleanup_receipt(fx: &Fixture, plan: &Value) {
    let stores = cm_store::Stores::open(&fx.store()).unwrap();
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

fn recovery_receipt(fx: &Fixture, plan: &Value) {
    let kind = match plan["body"]["recovery_kind"].as_str().unwrap() {
        "restore" => RecoveryKind::Restore,
        _ => RecoveryKind::Purge,
    };
    let stores = cm_store::Stores::open(&fx.store()).unwrap();
    stores
        .audit
        .save_recovery_receipt(&cm_core::recovery::RecoveryReceipt {
            receipt_id: format!("rr_{}", plan["body"]["plan_id"].as_str().unwrap()),
            plan_id: plan["body"]["plan_id"].as_str().unwrap().into(),
            plan_digest: plan["plan_digest"].as_str().unwrap().into(),
            recovery_kind: kind,
            principal: cm_core::authz::current_principal(),
            source: cm_core::authz::ReceiptSource::InteractiveReview,
            created_at: cm_core::time::now_utc(),
            expires_at: plan["body"]["expires_at"].as_str().unwrap().into(),
            revoked_at: None,
        })
        .unwrap();
}

/// `recovery plan create --restore|--purge IDS` plus its recovery receipt; returns the plan record.
fn approved_recovery(fx: &Fixture, flag: &str, ids: &[&str]) -> Value {
    let (code, plan) = fx.cli(&["recovery", "plan", "create", flag, &ids.join(",")]);
    assert_eq!(code, 0, "{plan}");
    recovery_receipt(fx, &plan);
    plan
}

fn n(report: &Value, k: &str) -> Option<i128> {
    report[k].as_str().map(|s| s.parse::<i128>().unwrap())
}

/// REC-05: both free-space observations are present and the signed delta is exactly after − before (never clamped).
fn delta_is_measured(report: &Value) -> i128 {
    let (b, a) = (n(report, "volume_free_before_bytes"), n(report, "volume_free_after_bytes"));
    let (Some(b), Some(a)) = (b, a) else { panic!("statfs observations are reported on macOS: {report}") };
    assert_eq!(n(report, "observed_free_delta_bytes"), Some(a - b), "signed delta reported as observed, never clamped: {report}");
    a - b
}

fn by_original<'a>(list: &'a Value, name: &str) -> &'a Value {
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["original_path"]["display"].as_str().unwrap().ends_with(&format!("/{name}")))
        .unwrap_or_else(|| panic!("{name} is listed: {list}"))
}

fn location(item: &Value) -> PathBuf {
    PathBuf::from(item["quarantined_path"]["display"].as_str().unwrap())
}

fn codes(report: &Value) -> Vec<String> {
    report["items"].as_array().unwrap().iter().map(|i| i["code"].as_str().unwrap().to_owned()).collect()
}

#[test]
fn quarantine_restore_and_purge_round_trip() {
    let fx = Fixture::new("flow");
    let temp = fx.tmp();
    let keep = fx.stale("keep.tmp", &[1u8; 4096]);
    fx.stale("clash.tmp", &[2u8; 2048]);
    fx.stale("swap.tmp", &[3u8; 1024]);
    let original_ino = ino(&keep);

    // Cleanup: quarantine the three stale files. REC-05: processed = quarantined, nothing freed by a same-volume move.
    let report = quarantine_all(&fx, 3);
    assert_eq!((report["status"].as_str(), report["succeeded"].as_u64()), (Some("completed"), Some(3)), "{report}");
    assert_eq!(report["processed_logical_bytes"], "7168");
    assert_eq!(report["quarantined_logical_bytes"], "7168");
    assert_eq!(report["estimated_freed_bytes"], "0", "a same-volume quarantine frees no data: {report}");
    delta_is_measured(&report);
    assert!(!keep.exists());
    let cleanup_op = report["operation_id"].as_str().unwrap().to_owned();
    let (code, rec) = fx.cli(&["operations", "reconcile", &cleanup_op]);
    assert_eq!((code, rec["succeeded"].as_u64()), (0, Some(3)), "reconciling a finished operation changes nothing: {rec}");

    // History and quarantine listing from new processes (durable manifests only).
    let (code, ops) = fx.cli(&["operations", "list"]);
    assert_eq!(code, 0, "{ops}");
    assert_eq!(ops["items"][0]["operation_id"], cleanup_op.as_str());
    assert_eq!((ops["items"][0]["action"].as_str(), ops["items"][0]["recovery"]["availability"].as_str()), (Some("quarantine"), Some("in_app_restore")));
    let (code, list) = fx.cli(&["quarantine", "list"]);
    assert_eq!(code, 0, "{list}");
    assert_eq!((list["items"].as_array().unwrap().len(), list["total_logical_bytes"].as_str()), (3, Some("7168")), "{list}");
    let k = by_original(&list, "keep.tmp");
    assert_eq!((k["state"].as_str(), k["restore_capability"].as_str(), k["reminder_due"].as_bool()), (Some("recoverable"), Some("available"), Some(false)), "{k}");
    assert_eq!(k["available_actions"], json!(["restore", "purge"]));
    assert_eq!(k["metadata_restored"], json!(["timestamps", "attributes", "acl", "xattrs"]));
    assert_eq!(k["operation_id"], cleanup_op.as_str());
    let qid = |name: &str| by_original(&list, name)["quarantine_id"].as_str().unwrap().to_owned();
    let (keep_id, clash_id, swap_id) = (qid("keep.tmp"), qid("clash.tmp"), qid("swap.tmp"));
    let keep_loc = location(k);
    assert!(keep_loc.starts_with(fx.store()), "quarantine stays inside the store: {}", keep_loc.display());
    assert_eq!(ino(&keep_loc), original_ino, "quarantine keeps the inode");
    assert_eq!(std::fs::metadata(keep_loc.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700, "private quarantine folder");

    // Refusal setups: a new file at clash.tmp's original name; swap.tmp's quarantined object replaced.
    std::fs::write(temp.join("clash.tmp"), b"newer").unwrap();
    let swap_loc = location(by_original(&list, "swap.tmp"));
    let swap_moved = fx.base.join("swap-moved");
    std::fs::rename(&swap_loc, &swap_moved).unwrap();
    std::fs::write(&swap_loc, b"planted").unwrap();
    let (_, list) = fx.cli(&["recovery", "list"]);
    assert_eq!(by_original(&list, "clash.tmp")["state"], "target_conflict");
    assert_eq!(by_original(&list, "swap.tmp")["state"], "identity_changed");

    // Recovery plan: its own type, digest and authorization.
    let ids = [keep_id.as_str(), clash_id.as_str(), swap_id.as_str()].join(",");
    let (code, rplan) = fx.cli(&["recovery", "plan", "create", "--restore", &ids]);
    assert_eq!(code, 0, "{rplan}");
    assert_eq!((rplan["body"]["engine_contract"].as_str(), rplan["authorization_status"].as_str()), (Some("recovery/v1"), Some("required")));
    let rp = rplan["body"]["plan_id"].as_str().unwrap().to_owned();
    let (code, err) = fx.cli(&["plan", "apply", &rp]);
    assert_eq!((code, err["code"].as_str()), (3, Some("needs_approval")), "{err}");
    cleanup_receipt(&fx, &rplan); // a cleanup-type approval never authorizes a restore
    let (code, _) = fx.cli(&["plan", "apply", &rp]);
    assert_eq!(code, 3);
    let (code, _) = fx.cli(&["plan", "review", &rp]);
    assert_eq!(code, 2, "machine callers cannot approve recovery plans");
    assert!(!keep.exists(), "nothing restored before approval");
    recovery_receipt(&fx, &rplan);
    let (_, shown) = fx.cli(&["plan", "show", &rp]);
    assert_eq!((shown["authorization_status"].as_str(), shown["body"]["recovery_kind"].as_str()), (Some("authorized"), Some("restore")));

    let (code, restored) = fx.cli(&["plan", "apply", &rp]);
    assert_eq!(code, 5, "partial: {restored}");
    assert_eq!((restored["succeeded"].as_u64(), restored["skipped"].as_u64()), (Some(1), Some(2)), "{restored}");
    let c = codes(&restored);
    assert!(c.iter().any(|c| c == "restored") && c.iter().any(|c| c == "destination_exists") && c.iter().any(|c| c == "identity_changed"), "{c:?}");
    assert_eq!((restored["quarantined_logical_bytes"].as_str(), restored["estimated_freed_bytes"].as_str()), (Some("0"), Some("0")));
    assert_eq!(restored["processed_logical_bytes"], "4096");
    delta_is_measured(&restored);
    assert_eq!(ino(&keep), original_ino, "the same inode is back");
    assert_eq!(std::fs::read(&keep).unwrap(), vec![1u8; 4096]);
    assert!(!keep_loc.exists());
    assert_eq!(std::fs::read(temp.join("clash.tmp")).unwrap(), b"newer", "never overwritten");
    assert_eq!(std::fs::read(&swap_loc).unwrap(), b"planted");
    assert_eq!(std::fs::read(&swap_moved).unwrap(), vec![3u8; 1024]);
    assert!(!temp.join("swap.tmp").exists());
    let (code, again) = fx.cli(&["plan", "apply", &rp]);
    assert_eq!((code, &again["operation_id"]), (5, &restored["operation_id"]), "replayed, not repeated");
    let restore_op = restored["operation_id"].as_str().unwrap().to_owned();
    let (code, rec) = fx.cli(&["operations", "reconcile", &restore_op]);
    assert_eq!((code, rec["succeeded"].as_u64(), rec["skipped"].as_u64()), (5, Some(1), Some(2)), "{rec}");

    // The conflicting item stays restorable; purging it needs its own plan and approval and removes only it.
    let (_, list) = fx.cli(&["recovery", "list"]);
    assert_eq!(list["items"].as_array().unwrap().len(), 2, "{list}");
    let clash_loc = location(by_original(&list, "clash.tmp"));
    let (code, pplan) = fx.cli(&["recovery", "plan", "create", "--purge", &clash_id]);
    assert_eq!(code, 0, "{pplan}");
    assert_eq!(pplan["body"]["items"][0]["target"]["kind"], "permanent_delete");
    let pp = pplan["body"]["plan_id"].as_str().unwrap().to_owned();
    let (code, _) = fx.cli(&["plan", "apply", &pp]);
    assert_eq!(code, 3, "the restore approval does not cover the purge");
    recovery_receipt(&fx, &pplan);
    let (code, purged) = fx.cli(&["plan", "apply", &pp]);
    assert_eq!(code, 0, "{purged}");
    assert_eq!((purged["estimated_freed_bytes"].as_str(), purged["items"][0]["recovery"].as_str()), (Some("2048"), Some("unavailable")), "{purged}");
    delta_is_measured(&purged);
    assert!(!clash_loc.exists());
    assert!(clash_loc.parent().unwrap().is_dir(), "the quarantine folder itself is kept");
    assert_eq!(std::fs::read(temp.join("clash.tmp")).unwrap(), b"newer", "the original-name file is not quarantine content");
    assert_eq!(std::fs::read(&swap_loc).unwrap(), b"planted", "other quarantine content is untouched");
    assert_eq!(std::fs::read(&keep).unwrap(), vec![1u8; 4096]);
    let (code, err) = fx.cli(&["recovery", "plan", "create", "--restore", &clash_id]);
    assert_eq!((code, err["code"].as_str()), (6, Some("conflict")), "a purged item cannot be planned again: {err}");

    // A replaced quarantined object is never purged either.
    let (code, pplan) = fx.cli(&["recovery", "plan", "create", "--purge", &swap_id]);
    assert_eq!(code, 0, "{pplan}");
    recovery_receipt(&fx, &pplan);
    let (code, refused) = fx.cli(&["plan", "apply", pplan["body"]["plan_id"].as_str().unwrap()]);
    assert_eq!((code, codes(&refused)), (5, vec!["identity_changed".to_owned()]), "{refused}");
    assert_eq!(std::fs::read(&swap_loc).unwrap(), b"planted");

    let (_, ops) = fx.cli(&["operations", "list", "--limit", "3"]);
    let actions: Vec<(&str, &str)> =
        ops["items"].as_array().unwrap().iter().map(|i| (i["action"].as_str().unwrap(), i["recovery"]["availability"].as_str().unwrap())).collect();
    assert_eq!(actions, [("purge", "not_applicable"), ("purge", "unavailable"), ("restore", "not_applicable")], "{ops}");

    // REC-01: quarantine content is never a cleanup candidate.
    let (_, scan) = fx.cli(&["scan", "--root", temp.to_str().unwrap()]);
    let (_, analysis) = fx.cli(&["analyze", "--scan", scan["scan_id"].as_str().unwrap()]);
    let (_, page) = fx.cli(&["candidates", "list", "--set", analysis["candidate_set_id"].as_str().unwrap()]);
    for c in page["items"].as_array().unwrap() {
        assert!(!PathBuf::from(c["path"]["display"].as_str().unwrap()).starts_with(fx.store()), "{c}");
    }
}

/// The original folder replaced by a new one of the same name, then made writable by everyone: both restores are
/// refused and the object stays in quarantine; with the original folder back and trusted a new plan restores it.
#[test]
fn restore_into_a_replaced_or_untrusted_original_folder_is_refused() {
    let fx = Fixture::new("parent");
    let f = fx.stale("p.tmp", b"planned");
    let original_ino = ino(&f);
    quarantine_all(&fx, 1);
    let (_, list) = fx.cli(&["quarantine", "list"]);
    let item = by_original(&list, "p.tmp");
    let (id, loc) = (item["quarantine_id"].as_str().unwrap().to_owned(), location(item));

    let old = fx.base.join("tmp-old");
    std::fs::rename(fx.tmp(), &old).unwrap();
    std::fs::create_dir(fx.tmp()).unwrap();
    let (_, list) = fx.cli(&["recovery", "list"]);
    assert_eq!(by_original(&list, "p.tmp")["state"], "parent_changed", "{list}");
    let plan = approved_recovery(&fx, "--restore", &[&id]);
    let (code, report) = fx.cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()]);
    assert_eq!((code, codes(&report)), (5, vec!["parent_changed".to_owned()]), "{report}");
    assert_eq!(report["items"][0]["recovery"], "quarantine", "{report}");
    assert!(loc.exists() && !fx.tmp().join("p.tmp").exists() && !old.join("p.tmp").exists());

    std::fs::remove_dir(fx.tmp()).unwrap();
    std::fs::rename(&old, fx.tmp()).unwrap();
    std::fs::set_permissions(fx.tmp(), std::fs::Permissions::from_mode(0o777)).unwrap();
    let plan = approved_recovery(&fx, "--restore", &[&id]);
    let (code, report) = fx.cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()]);
    assert_eq!((code, codes(&report)), (5, vec!["race_safety_unavailable".to_owned()]), "{report}");
    assert!(loc.exists() && !f.exists());

    std::fs::set_permissions(fx.tmp(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let (_, list) = fx.cli(&["recovery", "list"]);
    assert_eq!(by_original(&list, "p.tmp")["state"], "recoverable", "{list}");
    let plan = approved_recovery(&fx, "--restore", &[&id]);
    let (code, report) = fx.cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()]);
    assert_eq!((code, codes(&report)), (0, vec!["restored".to_owned()]), "{report}");
    assert_eq!((ino(&f), std::fs::read(&f).unwrap()), (original_ino, b"planned".to_vec()));
    assert!(!loc.exists());
}

/// Real macOS recovery actions, with a background writer on the same volume during each restore (REC-05 fixture).
struct WritesDuringRestore<A> {
    inner: A,
    ballast: PathBuf,
}

const BALLAST: usize = 64 << 20;

impl<A: RecoveryActions> RecoveryActions for WritesDuringRestore<A> {
    fn probe(&self, o: &QuarantinedObject<'_>) -> QuarantineState {
        self.inner.probe(o)
    }
    fn restore(&self, o: &QuarantinedObject<'_>) -> RecoveryOutcome {
        let out = self.inner.restore(o);
        let f = std::fs::File::create(&self.ballast).unwrap();
        std::io::Write::write_all(&mut &f, &vec![0xA5u8; BALLAST]).unwrap();
        f.sync_all().unwrap();
        out
    }
    fn purge(&self, o: &QuarantinedObject<'_>) -> RecoveryOutcome {
        self.inner.purge(o)
    }
    fn reconcile(&self, o: &QuarantinedObject<'_>, kind: RecoveryKind) -> Reconciled {
        self.inner.reconcile(o, kind)
    }
    fn free_space(&self, path: &RawPath) -> Option<u64> {
        self.inner.free_space(path)
    }
    fn metadata_coverage(&self) -> MetadataCoverage {
        self.inner.metadata_coverage()
    }
}

type Fault<'a> = &'a dyn Fn(cm_app::execute::FaultPoint, &str) -> bool;

/// Apply a stored recovery plan in this process (as `plan apply` does), with `actions` and a fault hook.
fn ctx_apply(stores: &cm_store::Stores, actions: &dyn RecoveryActions, plan_id: &str, fault: Fault<'_>) -> Result<cm_app::execute::OperationReport, cm_app::execute::ApplyError> {
    let principal = cm_core::authz::current_principal();
    let host = cm_platform::host::OsHostProbe.current("cli");
    let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let ctx = cm_app::recovery::RecoveryContext { principal: &principal, host: &host, now: &now, cancel: &|| false, fault };
    cm_app::recovery::apply_recovery_plan(stores, actions, plan_id, &ctx)
}

/// REC-05: background writes during a restore make the volume's free space fall. The report keeps the processed
/// amount apart, claims nothing as freed and shows the negative observed delta exactly (never clamped to zero).
#[test]
fn a_negative_observed_free_space_delta_is_reported_as_observed() {
    let fx = Fixture::new("delta");
    let f = fx.stale("n.tmp", &[7u8; 8192]);
    quarantine_all(&fx, 1);
    let (_, list) = fx.cli(&["quarantine", "list"]);
    let id = by_original(&list, "n.tmp")["quarantine_id"].as_str().unwrap().to_owned();
    let plan = approved_recovery(&fx, "--restore", &[&id]);
    let actions = WritesDuringRestore { inner: cm_platform::macos::actions::recovery::recovery_actions(), ballast: fx.base.join("ballast.bin") };
    let stores = cm_store::Stores::open(&fx.store()).unwrap();
    let report = ctx_apply(&stores, &actions, plan["body"]["plan_id"].as_str().unwrap(), &|_, _| false).unwrap();
    let report = serde_json::to_value(&report).unwrap();
    assert_eq!((report["status"].as_str(), report["succeeded"].as_u64()), (Some("completed"), Some(1)), "{report}");
    assert_eq!((report["processed_logical_bytes"].as_str(), report["estimated_freed_bytes"].as_str()), (Some("8192"), Some("0")), "{report}");
    let delta = delta_is_measured(&report);
    assert!(delta < 0, "free space fell while {BALLAST} bytes were written; reported as observed: {report}");
    assert_eq!(std::fs::read(&f).unwrap(), vec![7u8; 8192]);
    // The CLI shows the same stored report.
    let (_, shown) = fx.cli(&["operations", "show", report["operation_id"].as_str().unwrap()]);
    assert_eq!(shown["observed_free_delta_bytes"], report["observed_free_delta_bytes"], "{shown}");
}

/// EXE-05 for recovery: a restore that happened but whose result was never recorded (the process stopped right
/// after the move) is reconciled from the file system without acting again.
#[test]
fn an_interrupted_restore_is_reconciled_from_the_file_system() {
    let fx = Fixture::new("crash");
    let f = fx.stale("c.tmp", b"moved before the crash");
    quarantine_all(&fx, 1);
    let (_, list) = fx.cli(&["quarantine", "list"]);
    let id = by_original(&list, "c.tmp")["quarantine_id"].as_str().unwrap().to_owned();
    let plan = approved_recovery(&fx, "--restore", &[&id]);
    let stores = cm_store::Stores::open(&fx.store()).unwrap();
    let crash = |p: cm_app::execute::FaultPoint, _: &str| p == cm_app::execute::FaultPoint::AfterAction;
    let err = ctx_apply(&stores, &cm_platform::macos::actions::recovery::recovery_actions(), plan["body"]["plan_id"].as_str().unwrap(), &crash).unwrap_err();
    assert!(matches!(err, cm_app::execute::ApplyError::Crashed(_)), "{err:?}");
    drop(stores);
    assert_eq!(std::fs::read(&f).unwrap(), b"moved before the crash", "the move itself happened");
    let op = format!("op_{}", &plan["plan_digest"].as_str().unwrap()[..16]);
    let (code, rec) = fx.cli(&["operations", "reconcile", &op]);
    assert_eq!((code, codes(&rec)), (0, vec!["restored".to_owned()]), "{rec}");
    let (_, list) = fx.cli(&["recovery", "list"]);
    assert_eq!(list["items"].as_array().unwrap().len(), 0, "the item is settled as restored: {list}");
}
