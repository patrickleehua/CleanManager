//! Parity 17.1/17.2 and MVP 14.1 through the real binary, with scratch install/store directories,
//! a local signed manifest and a test key (debug builds only trust `CM_UPDATE_TEST_PUBLIC_KEY`);
//! no network. Copies of the test binary stand in for installed releases.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;

use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-life-{}-{name}", std::process::id()));
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

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn sha(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn test_key_hex() -> String {
    hex(&key(7).verifying_key().to_bytes())
}

fn target() -> String {
    format!("windows-{}", std::env::consts::ARCH)
}

/// A release layout in `dir` whose program is a copy of the test binary.
fn release_copy(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let exe = dir.join("cleanmanager.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_cleanmanager"), &exe).unwrap();
    let bytes = std::fs::read(&exe).unwrap();
    let manifest = json!({"contract": "cleanmanager-release/1", "product": "cleanmanager-cli", "version": env!("CARGO_PKG_VERSION"),
        "target": target(), "install_source": "zip", "files": [{"path": "cleanmanager.exe", "sha256": sha(&bytes), "size": bytes.len()}]});
    std::fs::write(dir.join("cleanmanager-release.json"), serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
    exe
}

/// A signed channel in `dir`: `manifest.json` describes `described` but serves `served`.
fn channel(dir: &Path, version: &str, described: &[u8], served: &[u8], signer: &SigningKey) -> PathBuf {
    std::fs::create_dir_all(dir.join("files")).unwrap();
    std::fs::write(dir.join("files").join("cleanmanager.exe"), served).unwrap();
    let payload = serde_json::to_vec(&json!({
        "contract": "cleanmanager-update/1", "product": "cleanmanager-cli", "version": version, "published_at": "2026-10-15T00:00:00Z",
        "compat": {"protocol_schema": "1.0", "index_schema": 1, "audit_schema": 1},
        "targets": {target(): {"files": [{"path": "cleanmanager.exe", "url": "files/cleanmanager.exe", "sha256": sha(described), "size": described.len()}]}},
    }))
    .unwrap();
    let b64 = base64::engine::general_purpose::STANDARD;
    let key_id = sha(&signer.verifying_key().to_bytes())[..16].to_owned();
    let envelope = json!({"contract": "cleanmanager-update-envelope/1", "payload_base64": b64.encode(&payload),
        "signatures": [{"key_id": key_id, "algorithm": "ed25519", "signature_base64": b64.encode(signer.sign(&payload).to_bytes())}]});
    let path = dir.join("manifest.json");
    std::fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
    path
}

struct Out {
    code: i32,
    rec: Value,
}

fn run(exe: &Path, args: &[&str], store: &Path, extra: &[(&str, &str)]) -> Out {
    let mut c = Command::new(exe);
    c.args(args).arg("--json").env("CM_STORE_DIR", store).env_remove("CM_RULES_DIR").env_remove("CLEANMANAGER_LANG").env("CM_UPDATE_TEST_PUBLIC_KEY", test_key_hex());
    for (k, v) in extra {
        c.env(k, v);
    }
    let o = c.output().unwrap();
    let text = String::from_utf8(o.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "one record: {text}");
    Out { code: o.status.code().unwrap(), rec: serde_json::from_str(text.trim()).unwrap() }
}

/// The record's kind is in schemas/v1 and it carries exactly the declared top-level fields.
fn schema_shape(v: &Value) {
    let schema: Value = serde_json::from_str(include_str!("../../../schemas/v1/protocol.schema.json")).unwrap();
    let kind = v["kind"].as_str().unwrap();
    let def = &schema["$defs"][kind];
    let props = def["properties"].as_object().unwrap_or_else(|| panic!("{kind} not in schema"));
    for k in v.as_object().unwrap().keys() {
        assert!(props.contains_key(k), "{kind}: {k} not in schema");
    }
    for r in def["required"].as_array().unwrap() {
        assert!(v.get(r.as_str().unwrap()).is_some(), "{kind}: {r} missing");
    }
}

#[test]
fn a_development_build_cannot_self_update_and_says_why() {
    let store = scratch("dev-store");
    let o = run(Path::new(env!("CARGO_BIN_EXE_cleanmanager")), &["self", "update", "--check"], &store, &[]);
    assert_eq!((o.code, o.rec["kind"].as_str(), o.rec["status"].as_str(), o.rec["install_method"].as_str()), (7, Some("release_info"), Some("cannot_check"), Some("unpackaged")));
    assert!(o.rec["reason"].as_str().unwrap().contains("not a release layout"));
    schema_shape(&o.rec);
}

#[test]
fn a_package_manager_copy_is_pointed_to_its_manager_and_never_replaced() {
    let root = scratch("scoop");
    let exe = release_copy(&root.join("scoop").join("apps").join("cleanmanager").join("current"));
    let before = std::fs::read(&exe).unwrap();
    let scoop = root.join("scoop").display().to_string();
    for args in [&["self", "update"][..], &["self", "update", "--check"][..]] {
        let o = run(&exe, args, &root.join("store"), &[("SCOOP", &scoop)]);
        assert_eq!(o.code, 0);
        assert_eq!((o.rec["status"].as_str(), o.rec["install_method"].as_str()), (Some("use_package_manager"), Some("scoop")));
        assert_eq!(o.rec["manager_command"], "scoop update cleanmanager");
        schema_shape(&o.rec);
    }
    assert_eq!(std::fs::read(&exe).unwrap(), before);
}

#[test]
fn a_checksum_mismatch_aborts_and_the_current_version_stays_usable() {
    let root = scratch("checksum");
    let exe = release_copy(&root.join("inst"));
    let good = std::fs::read(&exe).unwrap();
    let manifest = channel(&root.join("channel"), "99.0.0", &good, b"tampered program", &key(7));
    let store = root.join("store");
    let o = run(&exe, &["self", "update", "--manifest", manifest.to_str().unwrap()], &store, &[]);
    assert_eq!((o.code, o.rec["kind"].as_str(), o.rec["status"].as_str()), (3, Some("self_update_result"), Some("verification_failed")), "{}", o.rec);
    assert_eq!((o.rec["signature_verified"].as_bool(), o.rec["checksum_verified"].as_bool()), (Some(true), Some(false)));
    schema_shape(&o.rec);
    assert_eq!(std::fs::read(&exe).unwrap(), good, "the program was not replaced");
    let names: Vec<String> = std::fs::read_dir(root.join("inst")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(!names.iter().any(|n| n.contains("staging") || n.contains("backup")), "{names:?}");
    // The current version still runs.
    let caps = run(&exe, &["capabilities"], &store, &[]);
    assert_eq!((caps.code, caps.rec["kind"].as_str()), (0, Some("capabilities")));
}

#[test]
fn a_program_that_does_not_start_as_the_signed_version_is_not_installed() {
    let root = scratch("smoke");
    let exe = release_copy(&root.join("inst"));
    let bytes = std::fs::read(&exe).unwrap();
    // Checksums match, but the program reports its real version, not 99.0.0.
    let manifest = channel(&root.join("channel"), "99.0.0", &bytes, &bytes, &key(7));
    let o = run(&exe, &["self", "update", "--manifest", manifest.to_str().unwrap()], &root.join("store"), &[]);
    assert_eq!((o.code, o.rec["status"].as_str(), o.rec["checksum_verified"].as_bool()), (3, Some("verification_failed"), Some(true)), "{}", o.rec);
    assert!(o.rec["reason"].as_str().unwrap().contains("did not start as 99.0.0"));
    assert_eq!(std::fs::read(&exe).unwrap(), bytes);
}

#[test]
fn an_untrusted_signature_is_refused() {
    let root = scratch("untrusted");
    let exe = release_copy(&root.join("inst"));
    let bytes = std::fs::read(&exe).unwrap();
    let manifest = channel(&root.join("channel"), "99.0.0", &bytes, &bytes, &key(9));
    for args in [&["self", "update", "--check", "--manifest", manifest.to_str().unwrap()][..], &["self", "update", "--manifest", manifest.to_str().unwrap()][..]] {
        let o = run(&exe, args, &root.join("store"), &[]);
        assert_eq!((o.code, o.rec["status"].as_str()), (3, Some("verification_failed")), "{}", o.rec);
        assert!(o.rec["reason"].as_str().unwrap().contains("signature"));
    }
    // Without the test key the build has no trusted key at all.
    let o = run(&exe, &["self", "update", "--manifest", manifest.to_str().unwrap()], &root.join("store"), &[("CM_UPDATE_TEST_PUBLIC_KEY", "")]);
    assert_eq!((o.code, o.rec["status"].as_str()), (7, Some("cannot_check")));
    assert_eq!(std::fs::read(&exe).unwrap(), bytes);
}

#[test]
fn the_lifecycle_lock_serializes_updates_and_writes() {
    let root = scratch("lock");
    let exe = release_copy(&root.join("inst"));
    let bytes = std::fs::read(&exe).unwrap();
    let manifest = channel(&root.join("channel"), "99.0.0", &bytes, &bytes, &key(7));
    let store = root.join("store");
    std::fs::create_dir_all(&store).unwrap();
    let lock = File::options().read(true).write(true).create(true).truncate(false).open(store.join("lifecycle.lock")).unwrap();
    let write = ["apps", "updates", "config", "--checks", "off"];
    // An update in progress (exclusive): no write starts, no second update starts.
    lock.try_lock().unwrap();
    let w = run(&exe, &write, &store, &[]);
    assert_eq!((w.code, w.rec["kind"].as_str(), w.rec["code"].as_str()), (6, Some("error"), Some("conflict")), "{}", w.rec);
    let u = run(&exe, &["self", "update", "--manifest", manifest.to_str().unwrap()], &store, &[]);
    assert_eq!((u.code, u.rec["status"].as_str()), (6, Some("locked")), "{}", u.rec);
    lock.unlock().unwrap();
    // A write in progress (shared): other writes proceed, an update waits its turn.
    lock.try_lock_shared().unwrap();
    let w = run(&exe, &write, &store, &[]);
    assert_eq!(w.code, 0, "{}", w.rec);
    let u = run(&exe, &["self", "update", "--manifest", manifest.to_str().unwrap()], &store, &[]);
    assert_eq!(u.rec["status"], "locked");
    lock.unlock().unwrap();
    assert_eq!(std::fs::read(&exe).unwrap(), bytes);
}

#[test]
fn self_uninstall_previews_a_plan_that_keeps_user_data_and_changes_nothing() {
    let root = scratch("uninstall");
    let inst = root.join("inst");
    let exe = release_copy(&inst);
    let store = root.join("store");
    {
        let s = cm_store::Stores::open(&store).unwrap();
        let host = cm_core::host::HostRef { kind: "cli".into(), pid: 1, started_at: "2026-10-15T00:00:00Z".into() };
        s.audit.begin_operation("op1", "p1", "d", "k", &host, "2026-10-15T00:00:00Z").unwrap();
        s.audit.set_operation_state("op1", "completed").unwrap();
        s.audit.save_quarantine("q1", "op1", "a01", "p1", "{\"logical_bytes\":\"4096\"}", "2026-10-15T00:00:00Z", "2026-11-15T00:00:00Z").unwrap();
    }
    std::fs::write(store.join("protection.json"), "{}").unwrap();
    let before: Vec<_> = [&inst, &store].iter().flat_map(|d| std::fs::read_dir(d).unwrap().map(|e| e.unwrap().path())).collect();
    for (who, args) in [
        (exe.clone(), vec!["self", "uninstall", "--store", store.to_str().unwrap()]),
        (PathBuf::from(env!("CARGO_BIN_EXE_cleanmanager")), vec!["self", "uninstall", "--install-dir", inst.to_str().unwrap(), "--store", store.to_str().unwrap()]),
    ] {
        let o = run(&who, &args, &store, &[]);
        assert_eq!((o.code, o.rec["kind"].as_str(), o.rec["status"].as_str()), (0, Some("self_remove_preview"), Some("preview")), "{}", o.rec);
        schema_shape(&o.rec);
        assert_eq!((o.rec["quarantine_items"].as_u64(), o.rec["quarantine_logical_bytes"].as_str()), (Some(1), Some("4096")));
        let warnings: Vec<&str> = o.rec["warnings"].as_array().unwrap().iter().map(|w| w["code"].as_str().unwrap()).collect();
        assert!(warnings.contains(&"quarantine_not_empty"), "{warnings:?}");
        let keep: Vec<&str> = o.rec["keep"].as_array().unwrap().iter().map(|k| k["data"].as_str().unwrap()).collect();
        for k in ["protection_list", "history", "quarantine"] {
            assert!(keep.contains(&k), "{keep:?}");
        }
        let removes: Vec<(&str, &str)> = o.rec["remove"].as_array().unwrap().iter().map(|r| (r["component"].as_str().unwrap(), r["display"].as_str().unwrap())).collect();
        assert!(removes.iter().any(|(c, d)| *c == "cli" && d.ends_with("cleanmanager.exe")), "{removes:?}");
        assert!(removes.iter().any(|(c, d)| *c == "caches" && d.ends_with("index.db")), "{removes:?}");
        assert!(!removes.iter().any(|(_, d)| d.ends_with("audit.db") || d.ends_with("protection.json")), "{removes:?}");
        assert_eq!(o.rec["requires_interactive_confirmation"], true);
    }
    let after: Vec<_> = [&inst, &store].iter().flat_map(|d| std::fs::read_dir(d).unwrap().map(|e| e.unwrap().path())).collect();
    assert_eq!(before, after, "a preview removes nothing");
}

#[test]
fn self_install_previews_reports_other_programs_on_path_and_never_overwrites() {
    let root = scratch("install");
    let exe = release_copy(&root.join("inst"));
    let other = root.join("other-bin");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("cleanmanager.exe"), b"a different cleanmanager").unwrap();
    let path = std::env::join_paths([&other]).unwrap();
    let target = root.join("chosen").join("CleanManager");
    let o = run(&exe, &["self", "install", "--dir", target.to_str().unwrap(), "--add-to-path"], &root.join("store"), &[("PATH", path.to_str().unwrap())]);
    assert_eq!((o.code, o.rec["kind"].as_str(), o.rec["status"].as_str()), (0, Some("self_install_result"), Some("preview")), "{}", o.rec);
    schema_shape(&o.rec);
    assert_eq!(o.rec["path_entry"], "refused_conflict");
    assert_eq!(o.rec["path_conflicts"][0]["relation"], "different");
    assert!(!target.exists(), "a preview writes nothing");
    // A directory that already holds files is a conflict; nothing there is touched.
    let o = run(&exe, &["self", "install", "--dir", other.to_str().unwrap()], &root.join("store"), &[]);
    assert_eq!((o.code, o.rec["code"].as_str()), (6, Some("conflict")));
    assert_eq!(std::fs::read(other.join("cleanmanager.exe")).unwrap(), b"a different cleanmanager");
    // The development binary has no release layout to install from.
    let o = run(Path::new(env!("CARGO_BIN_EXE_cleanmanager")), &["self", "install", "--dir", target.to_str().unwrap()], &root.join("store"), &[]);
    assert_eq!((o.code, o.rec["code"].as_str()), (2, Some("invalid_argument")));
}
