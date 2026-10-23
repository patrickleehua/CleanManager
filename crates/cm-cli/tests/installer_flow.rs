//! End-to-end `installer` flow on Windows (INC-01..05, CLI-12): content recognition in a fixture
//! location, review-only previews, source selection only for people, and identity-bound plans
//! that skip a file replaced after planning. Nothing outside the scratch directory is touched
//! (`CM_INSTALLER_LOCATIONS` points discovery at the fixture).
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-installer-{}-{name}", std::process::id()));
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

fn cli(args: &[&str], store: &Path, locations: &str) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_cleanmanager"))
        .args(args)
        .arg("--store")
        .arg(store)
        .arg("--json")
        .env("CM_INSTALLER_LOCATIONS", locations)
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "exactly one record: {text}");
    (out.status.code().unwrap(), serde_json::from_str(&text).unwrap())
}

/// A minimal PE image followed by an NSIS installer data header.
fn nsis_exe(tag: u8) -> Vec<u8> {
    let mut b = vec![0u8; 0x400];
    b[..2].copy_from_slice(b"MZ");
    b[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
    b[0x40..0x44].copy_from_slice(b"PE\0\0");
    b[0x46..0x48].copy_from_slice(&1u16.to_le_bytes());
    let s = 0x40 + 24;
    b[s..s + 8].copy_from_slice(b".text\0\0\0");
    b[s + 16..s + 20].copy_from_slice(&0x200u32.to_le_bytes());
    b[s + 20..s + 24].copy_from_slice(&0x200u32.to_le_bytes());
    b.extend_from_slice(&[0, 0, 0, 0, 0xef, 0xbe, 0xad, 0xde]);
    b.extend_from_slice(b"NullsoftInst");
    b.resize(0x800, tag);
    b
}

/// A compound file whose root storage has the Windows Installer class id.
fn msi() -> Vec<u8> {
    let mut b = vec![0u8; 1024];
    b[..8].copy_from_slice(&[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]);
    b[0x1e..0x20].copy_from_slice(&9u16.to_le_bytes());
    b[512 + 0x50..512 + 0x60].copy_from_slice(&[0x84, 0x10, 0x0c, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
    b
}

#[test]
fn installers_are_recognized_by_content_and_plans_are_identity_bound() {
    let base = scratch("flow");
    let (store, dl) = (base.join("store"), base.join("Downloads"));
    std::fs::create_dir_all(&dl).unwrap();
    std::fs::write(dl.join("FixtureSetup.exe"), nsis_exe(1)).unwrap();
    std::fs::write(dl.join("tool.msi"), msi()).unwrap();
    std::fs::write(dl.join("notes.pdf"), b"%PDF-1.7 not an installer at all, just a document........").unwrap();
    std::fs::write(dl.join("setup.exe"), b"plain text renamed to setup.exe, which proves nothing about it").unwrap();
    std::fs::write(dl.join("report.txt"), b"quarterly numbers").unwrap();
    let tar = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join("System32").join("tar.exe");
    let zipped = Command::new(&tar).arg("-a").arg("-cf").arg(dl.join("docs.zip")).arg("-C").arg(&dl).arg("report.txt").output().is_ok_and(|o| o.status.success());
    let locations = format!("downloads={}", dl.display());

    let (code, locs) = cli(&["installer", "locations"], &store, &locations);
    assert_eq!((code, locs["locations"][0]["source_label"].as_str()), (0, Some("downloads")), "{locs}");

    let (code, preview) = cli(&["installer", "--dry-run", "--non-interactive"], &store, &locations);
    assert_eq!((code, preview["command"].as_str()), (0, Some("installer.preview")), "{preview}");
    let items = preview["items"].as_array().unwrap();
    let names: Vec<&str> = items.iter().map(|i| i["path"]["display"].as_str().unwrap().rsplit('\\').next().unwrap()).collect();
    assert_eq!(names, ["FixtureSetup.exe", "tool.msi"], "documents, archives of documents and renamed files are not candidates (zip fixture: {zipped})");
    for i in items {
        assert_eq!((i["tier"].as_str(), i["selected_by_default"].as_bool(), i["source_label"].as_str()), (Some("review_only"), Some(false), Some("downloads")), "{i}");
    }
    assert_eq!(preview["summary"]["selected_count"], 0);

    // Machines never get an automatic or by-source selection of installers.
    let (code, _) = cli(&["installer", "plan", "create", "--all-default-selected", "--non-interactive"], &store, &locations);
    assert_eq!(code, 2);
    let (code, err) = cli(&["installer", "plan", "create", "--source", "downloads", "--non-interactive"], &store, &locations);
    assert_eq!((code, err["code"].as_str()), (2, Some("invalid_argument")), "{err}");

    let exe = items[0]["object_id"].as_str().unwrap();
    let (code, plan) = cli(&["installer", "plan", "create", "--candidates", exe, "--mode", "delete", "--non-interactive"], &store, &locations);
    assert_eq!((code, plan["authorization_status"].as_str()), (0, Some("required")), "{plan}");
    assert_eq!(plan["body"]["items"].as_array().unwrap().len(), 1);

    // A new download replaces the planned file under the same name after planning.
    std::fs::remove_file(dl.join("FixtureSetup.exe")).unwrap();
    std::fs::write(dl.join("FixtureSetup.exe"), nsis_exe(2)).unwrap();
    {
        let stores = cm_store::Stores::open(&store).unwrap();
        stores.audit.save_receipt(&cm_core::authz::Receipt {
            receipt_id: "r_installer".into(),
            plan_id: plan["body"]["plan_id"].as_str().unwrap().into(),
            plan_digest: plan["plan_digest"].as_str().unwrap().into(),
            mode: cm_core::rules::Mode::Delete,
            principal: cm_core::authz::current_principal(),
            source: cm_core::authz::ReceiptSource::InteractiveReview,
            policy_id: None,
            policy_version: None,
            created_at: cm_core::time::now_utc(),
            expires_at: plan["body"]["expires_at"].as_str().unwrap().into(),
            revoked_at: None,
        }).unwrap();
    }
    let (_, report) = cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()], &store, &locations);
    assert_eq!((report["status"].as_str(), report["items"][0]["code"].as_str()), (Some("partial"), Some("precondition_identity_changed")), "{report}");
    assert_eq!(std::fs::read(dl.join("FixtureSetup.exe")).unwrap(), nsis_exe(2), "the new download survives");
    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn installers_open_elsewhere_are_skipped() {
    use std::os::windows::fs::OpenOptionsExt;
    let base = scratch("busy");
    let (store, dl) = (base.join("store"), base.join("Downloads"));
    std::fs::create_dir_all(&dl).unwrap();
    std::fs::write(dl.join("busy.msi"), msi()).unwrap();
    let locations = format!("downloads={}", dl.display());
    let holder = std::fs::OpenOptions::new().read(true).share_mode(1).open(dl.join("busy.msi")).unwrap();
    let (_, preview) = cli(&["installer", "scan"], &store, &locations);
    let item = &preview["items"][0];
    assert_eq!((item["skip_reason"].as_str(), item["selected_by_default"].as_bool()), (Some("in_use"), Some(false)), "{preview}");
    let (code, err) = cli(&["installer", "plan", "create", "--candidates", item["object_id"].as_str().unwrap()], &store, &locations);
    assert_eq!((code, err["code"].as_str()), (3, Some("policy_denied")), "an in-use installer cannot be planned now: {err}");
    // BUG-WFA-19: the refusal is written for people, not as Rust debug text.
    let message = err["message"].as_str().unwrap();
    assert!(message.contains("pending_evidence:") && !message.contains("PendingEvidence"), "{message}");
    drop(holder);
    std::fs::remove_dir_all(&base).ok();
}

/// WFA-58 (INC-04): an installer another process holds open without read sharing (`FileShare.None`) cannot be read,
/// yet it is listed as in use, names its holder, counts in the summary and cannot be planned.
#[test]
fn installers_held_without_read_sharing_are_listed_in_use_with_their_holder() {
    use std::os::windows::fs::OpenOptionsExt;
    let base = scratch("held");
    let (store, dl) = (base.join("store"), base.join("Downloads"));
    std::fs::create_dir_all(&dl).unwrap();
    std::fs::write(dl.join("held.msi"), msi()).unwrap();
    std::fs::write(dl.join("held-notes.pdf"), b"%PDF-1.7 a document held open by a viewer, never an installer......").unwrap();
    let locations = format!("downloads={}", dl.display());
    let holders: Vec<_> = ["held.msi", "held-notes.pdf"].iter().map(|n| std::fs::OpenOptions::new().read(true).share_mode(0).open(dl.join(n)).unwrap()).collect();
    let (code, preview) = cli(&["installer", "scan"], &store, &locations);
    assert_eq!(code, 0, "{preview}");
    let items = preview["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "only the file whose name suggests an installer is listed: {preview}");
    let item = &items[0];
    assert!(item["path"]["display"].as_str().unwrap().ends_with("held.msi"), "{item}");
    assert_eq!((item["skip_reason"].as_str(), item["selected_by_default"].as_bool(), item["tier"].as_str()), (Some("in_use"), Some(false), Some("review_only")), "{item}");
    let notes: Vec<&str> = item["notes"].as_array().unwrap().iter().map(|n| n.as_str().unwrap()).collect();
    let me = format!("pid {}", std::process::id());
    assert!(notes.iter().any(|n| n.starts_with("in use by") && n.contains(&me)), "the holder is named: {notes:?}");
    assert!(notes.iter().any(|n| n.starts_with("not recognized by content:")), "{notes:?}");
    assert_eq!((preview["summary"]["skipped_count"].as_u64(), preview["convertible_to_plan"].as_bool()), (Some(1), Some(false)), "{preview}");
    let (code, err) = cli(&["installer", "plan", "create", "--candidates", item["object_id"].as_str().unwrap()], &store, &locations);
    assert_eq!((code, err["code"].as_str()), (3, Some("policy_denied")), "{err}");
    drop(holders);
    // Released, it is recognized by content again.
    let (_, preview) = cli(&["installer", "scan"], &store, &locations);
    let item = &preview["items"][0];
    assert_eq!((item["skip_reason"].as_str(), item["rule_id"].as_str()), (None, Some("installer.msi")), "{preview}");
    std::fs::remove_dir_all(&base).ok();
}

/// WFA-58 / NEW-10: a row that cannot be planned now is refused for that reason before the live re-check. An installer
/// held open by another process (here: with write sharing, so it can still change) that grew after discovery is
/// refused as not plannable (`policy_denied`, its in-use reason named), never as `plan_stale`; the attached-image case
/// follows the same order (an attached image keeps changing while mounted).
#[test]
fn an_unplannable_installer_is_refused_before_the_live_check() {
    use std::os::windows::fs::OpenOptionsExt;
    let base = scratch("busy-changed");
    let (store, dl) = (base.join("store"), base.join("Downloads"));
    std::fs::create_dir_all(&dl).unwrap();
    std::fs::write(dl.join("busy.msi"), msi()).unwrap();
    let locations = format!("downloads={}", dl.display());
    // Read and write sharing: the file stays changeable while it is held.
    let holder = std::fs::OpenOptions::new().read(true).share_mode(3).open(dl.join("busy.msi")).unwrap();
    let (_, preview) = cli(&["installer", "scan"], &store, &locations);
    let item = &preview["items"][0];
    assert_eq!(item["skip_reason"].as_str(), Some("in_use"), "{preview}");
    let mut grown = msi();
    grown.extend_from_slice(&[0u8; 4096]);
    std::fs::write(dl.join("busy.msi"), grown).unwrap();
    let (code, err) = cli(&["installer", "plan", "create", "--candidates", item["object_id"].as_str().unwrap()], &store, &locations);
    assert_eq!((code, err["code"].as_str()), (3, Some("policy_denied")), "refused as not plannable, not as changed: {err}");
    let message = err["message"].as_str().unwrap();
    assert!(message.contains("cannot be planned: pending_evidence:") && message.contains("in_use"), "{message}");
    drop(holder);
    std::fs::remove_dir_all(&base).ok();
}
