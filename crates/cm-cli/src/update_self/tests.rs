//! LIF-01 with test keys, an in-memory transport and scratch install directories (no network).

use std::cell::RefCell;
use std::collections::BTreeMap;

use ed25519_dalek::{Signer, SigningKey};
use serde_json::json;

use super::*;

const TARGET: &str = "windows-x86_64";

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-selfupdate-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn signing() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

fn trusted(k: &SigningKey) -> TrustedKey {
    let public = k.verifying_key().to_bytes();
    TrustedKey { key_id: key_id(&public), key: k.verifying_key() }
}

fn envelope(payload: &[u8], k: &SigningKey) -> Vec<u8> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let sig = k.sign(payload).to_bytes();
    serde_json::to_vec(&json!({
        "contract": ENVELOPE_CONTRACT,
        "payload_base64": b64.encode(payload),
        "signatures": [{"key_id": key_id(&k.verifying_key().to_bytes()), "algorithm": "ed25519", "signature_base64": b64.encode(sig)}],
    }))
    .unwrap()
}

/// Files served by URL plus a log of every request.
struct Mem {
    files: BTreeMap<String, Vec<u8>>,
    requests: RefCell<Vec<String>>,
}

impl Transport for Mem {
    fn get(&self, location: &str, max: u64) -> Result<Vec<u8>, String> {
        self.requests.borrow_mut().push(location.to_owned());
        let b = self.files.get(location).cloned().ok_or_else(|| format!("404 {location}"))?;
        if b.len() as u64 > max { Err("too large".into()) } else { Ok(b) }
    }
}

const NEW_EXE: &[u8] = b"new program 0.2.0";
const NEW_RULE: &[u8] = b"{\"rule\":\"v2\"}";

fn manifest_payload(version: &str, files: &[(&str, &[u8])], audit_schema: u32) -> Vec<u8> {
    let list: Vec<_> = files.iter().map(|(p, b)| json!({"path": p, "url": format!("files/{p}"), "sha256": sha256_bytes(b), "size": b.len()})).collect();
    serde_json::to_vec(&json!({
        "contract": MANIFEST_CONTRACT, "product": "cleanmanager-cli", "version": version, "published_at": "2026-10-15T00:00:00Z",
        "compat": {"protocol_schema": "1.0", "index_schema": 1, "audit_schema": audit_schema},
        "targets": {TARGET: {"files": list}},
    }))
    .unwrap()
}

const BASE: &str = "https://updates.example.test/cli/manifest.json";

fn serve(envelope: Vec<u8>, files: &[(&str, &[u8])]) -> Mem {
    let mut m = BTreeMap::new();
    m.insert(BASE.to_owned(), envelope);
    for (p, b) in files {
        m.insert(format!("https://updates.example.test/cli/files/{p}"), b.to_vec());
    }
    Mem { files: m, requests: RefCell::new(Vec::new()) }
}

/// A 0.1.0 release layout: program, one rule, a file the new release drops, and the manifest.
fn install(name: &str) -> (PathBuf, ReleaseManifest) {
    let dir = scratch(name);
    let files: [(&str, &[u8]); 3] = [("cleanmanager.exe", b"old program 0.1.0"), ("rules/windows/a.json", b"{\"rule\":\"v1\"}"), ("OLD-NOTES.txt", b"notes")];
    for (p, b) in files {
        let path = dir.join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b).unwrap();
    }
    let m = ReleaseManifest {
        version: "0.1.0".into(), target: TARGET.into(), install_source: "zip".into(),
        files: files.iter().map(|(p, b)| ReleaseFile { path: (*p).into(), sha256: sha256_bytes(b), size: b.len() as u64 }).collect(),
    };
    std::fs::write(dir.join(RELEASE_FILE), m.to_json()).unwrap();
    (dir, m)
}

fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, d: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(d).unwrap().map(Result::unwrap) {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                out.insert(p.strip_prefix(root).unwrap().display().to_string().replace('\\', "/"), std::fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn ok_smoke(_: &Path, _: &str) -> Result<(), String> {
    Ok(())
}

fn ctx<'a>(dir: &'a Path, keys: &'a [TrustedKey], t: &'a Mem, smoke: &'a dyn Fn(&Path, &str) -> Result<(), String>, fs: &'a dyn FsOps) -> Context<'a> {
    Context {
        install_dir: dir, current_version: "0.1.0", target: TARGET, manifest_location: BASE, keys, transport: t,
        store_audit_schema: Some(1), store_index_schema: Some(1), smoke, fs,
    }
}

const NEW_FILES: &[(&str, &[u8])] = &[("cleanmanager.exe", NEW_EXE), ("rules/windows/a.json", NEW_RULE)];

#[test]
fn a_verified_update_replaces_the_layout_and_keeps_a_backup() {
    let (dir, old) = install("ok");
    let k = signing();
    let keys = [trusted(&k)];
    let t = serve(envelope(&manifest_payload("0.2.0", NEW_FILES, 1), &k), NEW_FILES);
    let o = apply(&ctx(&dir, &keys, &t, &ok_smoke, &RealFs), &old);
    assert_eq!(o.status, Status::Updated, "{o:?}");
    assert!(o.signature_verified && o.checksum_verified);
    let after = tree(&dir);
    assert_eq!(after["cleanmanager.exe"], NEW_EXE);
    assert_eq!(after["rules/windows/a.json"], NEW_RULE);
    assert!(!after.contains_key("OLD-NOTES.txt"), "entries the new release dropped move to the backup");
    let release = ReleaseManifest::parse(std::str::from_utf8(&after[RELEASE_FILE]).unwrap()).unwrap();
    assert_eq!((release.version.as_str(), release.install_source.as_str(), release.files.len()), ("0.2.0", "zip", 2));
    assert_eq!(after[&format!("{BACKUP_DIR}/0.1.0/cleanmanager.exe")], b"old program 0.1.0");
    assert_eq!(after[&format!("{BACKUP_DIR}/0.1.0/OLD-NOTES.txt")], b"notes");
    assert!(!after.keys().any(|k| k.starts_with(STAGING_PREFIX)));
}

#[test]
fn a_checksum_mismatch_aborts_and_the_current_version_is_untouched() {
    let (dir, old) = install("checksum");
    let before = tree(&dir);
    let k = signing();
    let keys = [trusted(&k)];
    // The signed manifest describes NEW_EXE; the server delivers something else.
    let t = serve(envelope(&manifest_payload("0.2.0", NEW_FILES, 1), &k), &[("cleanmanager.exe", b"tampered program"), ("rules/windows/a.json", NEW_RULE)]);
    let o = apply(&ctx(&dir, &keys, &t, &ok_smoke, &RealFs), &old);
    assert_eq!(o.status, Status::VerificationFailed, "{o:?}");
    assert!(o.signature_verified && !o.checksum_verified);
    assert!(o.reason.as_deref().unwrap().contains("checksum of cleanmanager.exe does not match"));
    assert_eq!(tree(&dir), before, "nothing replaced, no staging or backup left behind");
}

#[test]
fn bad_or_untrusted_signatures_stop_before_any_download() {
    let (dir, old) = install("sig");
    let before = tree(&dir);
    let k = signing();
    let other = SigningKey::from_bytes(&[9u8; 32]);
    let payload = manifest_payload("0.2.0", NEW_FILES, 1);
    let mut tampered: Value = serde_json::from_slice(&envelope(&payload, &k)).unwrap();
    let b64 = base64::engine::general_purpose::STANDARD;
    tampered["payload_base64"] = json!(b64.encode(manifest_payload("0.2.1", NEW_FILES, 1)));
    for (name, env, keys) in [
        ("untrusted key", envelope(&payload, &other), vec![trusted(&k)]),
        ("tampered payload", serde_json::to_vec(&tampered).unwrap(), vec![trusted(&k)]),
        ("no embedded key", envelope(&payload, &k), vec![]),
        ("not an envelope", payload.clone(), vec![trusted(&k)]),
    ] {
        let t = serve(env, NEW_FILES);
        let o = apply(&ctx(&dir, &keys, &t, &ok_smoke, &RealFs), &old);
        assert_eq!(o.status, Status::VerificationFailed, "{name}: {o:?}");
        assert!(!o.signature_verified, "{name}");
        assert_eq!(t.requests.borrow().as_slice(), [BASE], "{name}: only the envelope was fetched");
        assert_eq!(tree(&dir), before, "{name}");
    }
}

#[test]
fn equal_older_or_store_incompatible_releases_change_nothing() {
    let (dir, old) = install("versions");
    let before = tree(&dir);
    let k = signing();
    let keys = [trusted(&k)];
    for (version, audit, status) in [("0.1.0", 1, Status::UpToDate), ("0.0.9", 1, Status::UpToDate), ("0.2.0", 0, Status::Incompatible)] {
        let t = serve(envelope(&manifest_payload(version, NEW_FILES, audit), &k), NEW_FILES);
        let c = ctx(&dir, &keys, &t, &ok_smoke, &RealFs);
        assert_eq!(apply(&c, &old).status, status, "{version}");
        assert_eq!(check(&c).status, status, "{version}");
        assert_eq!(t.requests.borrow().iter().filter(|r| r.contains("/files/")).count(), 0);
    }
    let t = serve(envelope(&manifest_payload("0.2.0", NEW_FILES, 1), &k), NEW_FILES);
    let c = check(&ctx(&dir, &keys, &t, &ok_smoke, &RealFs));
    assert_eq!((c.status, c.to_version.as_deref()), (Status::UpdateAvailable, Some("0.2.0")));
    assert_eq!(tree(&dir), before);
}

#[test]
fn a_program_that_does_not_start_as_the_new_version_is_not_installed() {
    let (dir, old) = install("smoke");
    let before = tree(&dir);
    let k = signing();
    let keys = [trusted(&k)];
    let t = serve(envelope(&manifest_payload("0.2.0", NEW_FILES, 1), &k), NEW_FILES);
    let bad = |_: &Path, _: &str| Err("it reports version 0.1.0".to_string());
    let o = apply(&ctx(&dir, &keys, &t, &bad, &RealFs), &old);
    assert_eq!(o.status, Status::VerificationFailed);
    assert!(o.checksum_verified && o.reason.unwrap().contains("did not start"));
    assert_eq!(tree(&dir), before);
}

/// Fails the `n`th rename (1-based), and optionally every rename after it.
struct FailingFs {
    n: usize,
    sticky: bool,
    count: RefCell<usize>,
}

impl FsOps for FailingFs {
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        *self.count.borrow_mut() += 1;
        let c = *self.count.borrow();
        if c == self.n || (self.sticky && c > self.n) {
            return Err(std::io::Error::other("injected rename failure"));
        }
        std::fs::rename(from, to)
    }
}

#[test]
fn a_failed_swap_is_rolled_back_to_the_old_version() {
    let k = signing();
    let keys = [trusted(&k)];
    // Fail at each rename position in turn: the old layout must always come back intact.
    for n in 1..=7 {
        let (dir, old) = install(&format!("swap-{n}"));
        let before = tree(&dir);
        let t = serve(envelope(&manifest_payload("0.2.0", NEW_FILES, 1), &k), NEW_FILES);
        let fs = FailingFs { n, sticky: false, count: RefCell::new(0) };
        let o = apply(&ctx(&dir, &keys, &t, &ok_smoke, &fs), &old);
        assert_eq!(o.status, Status::RolledBack, "rename {n}: {o:?}");
        assert_eq!(tree(&dir), before, "rename {n}");
    }
}

#[test]
fn an_incomplete_rollback_is_reported_with_the_backup_location() {
    let (dir, old) = install("rollback-fails");
    let k = signing();
    let keys = [trusted(&k)];
    let t = serve(envelope(&manifest_payload("0.2.0", NEW_FILES, 1), &k), NEW_FILES);
    let fs = FailingFs { n: 4, sticky: true, count: RefCell::new(0) };
    let o = apply(&ctx(&dir, &keys, &t, &ok_smoke, &fs), &old);
    assert_eq!(o.status, Status::RollbackIncomplete, "{o:?}");
    assert!(o.backup.is_some() && o.reason.unwrap().contains(BACKUP_DIR));
}

#[test]
fn manifests_are_bounded_to_this_product_and_target() {
    let ok = manifest_payload("0.2.0", NEW_FILES, 1);
    assert!(parse_manifest(&ok, TARGET).is_ok());
    assert!(parse_manifest(&ok, "macos-aarch64").is_err());
    for files in [
        vec![("rules/a.json", NEW_RULE)],
        vec![("cleanmanager.exe", NEW_EXE), ("../evil.exe", NEW_EXE)],
        vec![("cleanmanager.exe", NEW_EXE), (RELEASE_FILE, NEW_EXE)],
        vec![("cleanmanager.exe", NEW_EXE), (".cleanmanager-update-backup/x", NEW_EXE)],
        vec![("cleanmanager.exe", NEW_EXE), ("CLEANMANAGER.EXE", NEW_EXE)],
    ] {
        assert!(parse_manifest(&manifest_payload("0.2.0", &files, 1), TARGET).is_err(), "{files:?}");
    }
    assert!(parse_manifest(&manifest_payload("latest", NEW_FILES, 1), TARGET).is_err());
}

#[test]
fn file_urls_resolve_relative_to_the_manifest_and_never_escape_to_local_files() {
    assert_eq!(resolve(BASE, "files/x.exe").unwrap(), "https://updates.example.test/cli/files/x.exe");
    assert_eq!(resolve(BASE, "https://cdn.example.test/x").unwrap(), "https://cdn.example.test/x");
    for bad in ["http://x/y", "file:///C:/x", "C:\\x.exe", "../x", "ftp://x"] {
        assert!(resolve(BASE, bad).is_err(), "{bad}");
    }
    let local = resolve("D:\\offline\\manifest.json", "files/x.exe").unwrap();
    assert!(local.replace('\\', "/").ends_with("offline/files/x.exe"), "{local}");
}

#[test]
fn versions_follow_semver_precedence() {
    use std::cmp::Ordering::*;
    for (a, b, o) in [("0.2.0", "0.1.9", Greater), ("1.0.0", "1.0.0-rc.1", Greater), ("1.0.0-rc.2", "1.0.0-rc.10", Less), ("1.0.0+b1", "1.0.0", Equal), ("1.0.0-alpha", "1.0.0-alpha.1", Less)] {
        assert_eq!(compare_versions(a, b), Some(o), "{a} vs {b}");
    }
    assert_eq!(compare_versions("1.0", "1.0.0"), None);
}

#[test]
fn package_manager_installs_are_detected_and_never_self_updated() {
    let env = |k: &str| -> Option<String> {
        match k {
            "LOCALAPPDATA" => Some(r"C:\Users\u\AppData\Local".into()),
            "USERPROFILE" => Some(r"C:\Users\u".into()),
            "APPDATA" => Some(r"C:\Users\u\AppData\Roaming".into()),
            "ProgramFiles" => Some(r"C:\Program Files".into()),
            _ => None,
        }
    };
    let rel = ReleaseManifest { version: "0.1.0".into(), target: TARGET.into(), install_source: "zip".into(), files: vec![] };
    let cases: [(&str, InstallMethod, Option<&str>); 9] = [
        (r"C:\Users\u\AppData\Local\Microsoft\WinGet\Packages\Example.CleanManager_Microsoft.Winget.Source_8wekyb3d8bbwe\cleanmanager.exe", InstallMethod::Winget, Some("winget upgrade --id Example.CleanManager --exact")),
        (r"C:\Program Files\WinGet\Packages\Example.CleanManager_x\cleanmanager.exe", InstallMethod::Winget, Some("winget upgrade --id Example.CleanManager --exact")),
        (r"C:\Users\u\scoop\apps\cleanmanager\current\cleanmanager.exe", InstallMethod::Scoop, Some("scoop update cleanmanager")),
        (r"C:\Users\u\AppData\Roaming\npm\node_modules\@example\cleanmanager\bin\cleanmanager.exe", InstallMethod::Npm, Some("npm update -g @example/cleanmanager")),
        (r"C:\Users\u\AppData\Roaming\npm\cleanmanager.exe", InstallMethod::Npm, Some("npm update -g cleanmanager")),
        (r"C:\Users\u\.cargo\bin\cleanmanager.exe", InstallMethod::Cargo, Some("cargo install --path crates/cm-cli --locked --force")),
        ("/opt/homebrew/bin/cleanmanager", InstallMethod::Homebrew, Some("brew upgrade cleanmanager")),
        (r"D:\Tools\CleanManager\cleanmanager.exe", InstallMethod::Standalone, None),
        (r"c:\users\U\SCOOP\apps\cm2\1.0\cleanmanager.exe", InstallMethod::Scoop, Some("scoop update cm2")),
    ];
    for (path, method, cmd) in cases {
        let d = detect(Path::new(path), Some(&rel), &env);
        assert_eq!((d.method, d.update_command.as_deref()), (method, cmd), "{path}");
        assert_eq!(d.method.managed(), cmd.is_some());
    }
    assert_eq!(detect(Path::new(r"D:\src\target\debug\cleanmanager.exe"), None, &env).method, InstallMethod::Unpackaged);
    let packaged = ReleaseManifest { install_source: "winget".into(), ..rel };
    assert_eq!(detect(Path::new(r"D:\Tools\cleanmanager.exe"), Some(&packaged), &env).method, InstallMethod::Winget);
}
