//! End-to-end manual trash on real NTFS (parity task group 6): scan → `trash inspect` → `trash` (plan only) →
//! needs_approval even with an enabled policy → review receipt → apply (Recycle Bin, identity-verified) → `trash list`
//! → `trash purge` (separate plan) → apply with free-space observation. Receipts are written through `cm_store` as
//! the terminal review would (it needs a TTY). Only files created in a scratch folder are moved; anything of theirs
//! left in the Recycle Bin by a failing run is removed by a guard that matches the bin's own records.
#![cfg(windows)]

use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use cm_core::analysis::IndexView;
use serde_json::Value;

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-trash-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn cli(args: &[&str], store: &Path) -> (i32, Value) {
    // `trash list` reads only the Recycle Bin and takes no store.
    let store_args: &[&std::ffi::OsStr] = if args == ["trash", "list"] { &[] } else { &["--store".as_ref(), store.as_os_str()] };
    let out = Command::new(env!("CARGO_BIN_EXE_cleanmanager")).args(args).args(store_args).arg("--json").env("CM_STORE_DIR", store).output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "exactly one record: {text}");
    (out.status.code().unwrap(), serde_json::from_str(&text).unwrap())
}

/// Removes from the user's Recycle Bin whatever this test moved there (matched by the bin's own `$I` records).
struct BinGuard(String);

impl Drop for BinGuard {
    fn drop(&mut self) {
        use cm_platform::windows::actions::recycle_bin::{parse_info, user_bin_dirs};
        for dir in user_bin_dirs() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let Some(suffix) = name.strip_prefix("$I") else { continue };
                let Some((orig, _, _)) = std::fs::read(e.path()).ok().and_then(|d| parse_info(&d)) else { continue };
                if orig.to_ascii_lowercase().contains(&self.0) {
                    let r = dir.join(format!("$R{suffix}"));
                    let _ = if r.is_dir() { std::fs::remove_dir_all(&r) } else { std::fs::remove_file(&r) };
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
}

fn entry_of(store: &Path, scan: &str, path: &Path) -> String {
    let stores = cm_store::Stores::open(store).unwrap();
    let view = stores.index.view(scan).unwrap();
    let want = path.file_name().unwrap().to_string_lossy().to_ascii_lowercase();
    let parent = path.parent().unwrap().file_name().unwrap().to_string_lossy().to_ascii_lowercase();
    let found = view.facts.keys().copied().find(|&e| {
        view.name(e).lossy().to_ascii_lowercase() == want && view.parent(e).is_some_and(|p| view.name(p).lossy().to_ascii_lowercase().ends_with(&parent))
    });
    found.unwrap_or_else(|| panic!("{} indexed", path.display())).to_string()
}

fn approve(store: &Path, plan: &Value) {
    let stores = cm_store::Stores::open(store).unwrap();
    let kind: cm_core::plan::v2::PlanKind = serde_json::from_value(plan["body"]["plan_kind"].clone()).unwrap();
    stores.audit.save_kind_receipt(&cm_core::authz::KindReceipt {
        receipt_id: format!("r_{}", plan["body"]["plan_id"].as_str().unwrap()),
        plan_id: plan["body"]["plan_id"].as_str().unwrap().into(),
        plan_digest: plan["plan_digest"].as_str().unwrap().into(),
        plan_kind: kind,
        principal: cm_core::authz::current_principal(),
        source: cm_core::authz::ReceiptSource::InteractiveReview,
        created_at: cm_core::time::now_utc(),
        expires_at: plan["body"]["expires_at"].as_str().unwrap().into(),
        revoked_at: None,
    }).unwrap();
}

fn write(p: &Path, bytes: usize) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, vec![7u8; bytes]).unwrap();
}

#[test]
fn manual_trash_round_trip_through_review_bin_and_purge() {
    let base = scratch("flow");
    let marker = base.file_name().unwrap().to_string_lossy().to_ascii_lowercase();
    let _guard = BinGuard(marker.clone());
    let (data, store) = (base.join("data"), base.join("store"));
    write(&data.join("big.bin"), 64 * 1024);
    write(&data.join("renders").join("a.png"), 1000);
    write(&data.join("renders").join("deep").join("b.png"), 2000);
    write(&data.join("home").join(".ssh").join("id_ed25519"), 10);
    write(&data.join("home").join("notes.txt"), 10);
    write(&data.join("app.log"), 50);

    let (code, scan) = cli(&["scan", "--root", data.to_str().unwrap()], &store);
    assert_eq!(code, 0, "{scan}");
    let scan_id = scan["scan_id"].as_str().unwrap().to_owned();
    let e = |p: &Path| entry_of(&store, &scan_id, p);

    // MTR-02/05: inspect is read-only and explains why a folder with protected contents is view-only.
    let (code, ins) = cli(&["trash", "inspect", "--scan", &scan_id, "--entry", &e(&data.join("home"))], &store);
    assert_eq!(code, 0, "{ins}");
    assert_eq!((ins["kind"].as_str(), ins["trashable"].as_bool()), (Some("trash_inspection"), Some(false)));
    assert_eq!(ins["refusal"]["reason"], "protected_descendants", "{ins}");
    assert!(ins["protected"][0]["path"]["display"].as_str().unwrap().ends_with(".ssh"));
    assert_eq!(ins["available_actions"], serde_json::json!(["show_path", "copy_path"]), "no reveal is run by the CLI");
    let (code, err) = cli(&["trash", "--scan", &scan_id, "--entry", &e(&data.join("home"))], &store);
    assert_eq!((code, err["code"].as_str()), (3, Some("policy_denied")), "{err}");
    let (code, err) = cli(&["trash", "--scan", &scan_id, "--entry", "999999999"], &store);
    assert_eq!((code, err["code"].as_str()), (2, Some("invalid_argument")), "{err}");
    let (_, ins) = cli(&["trash", "inspect", "--scan", &scan_id, "--entry", &e(&data.join("renders"))], &store);
    assert_eq!((ins["trashable"].as_bool(), ins["files"].as_u64(), ins["logical_bytes"].as_str()), (Some(true), Some(2), Some("3000")), "{ins}");

    // MTR-01: only a plan; an enabled cleanup policy changes nothing about that.
    let (code, plan) = cli(&["trash", "--scan", &scan_id, "--entry", &format!("{},{}", e(&data.join("big.bin")), e(&data.join("renders")))], &store);
    assert_eq!(code, 0, "{plan}");
    assert_eq!((plan["body"]["plan_kind"].as_str(), plan["body"]["policy_matchable"].as_bool()), (Some("manual_selection"), Some(false)));
    assert_eq!(plan["authorization_status"], "required");
    let items = plan["body"]["items"].as_array().unwrap();
    assert!(items.iter().all(|a| a["action_type"] == "fs_trash" && a["target_location"] == "original"));
    assert_eq!(items[1]["target"]["kind"], "directory");
    assert_eq!(items[1]["subtree"]["items"], 3, "two files and one folder inside");
    {
        let stores = cm_store::Stores::open(&store).unwrap();
        let mut pol = cm_core::authz::Policy::template("pol_any", &cm_core::authz::current_principal(), scan["scope_id"].as_str().unwrap(), vec![],
            cm_core::rules::Mode::Trash, cm_platform_now());
        pol.state = cm_core::authz::PolicyState::Enabled;
        stores.audit.save_policy(&pol, &cm_core::time::now_utc()).unwrap();
    }
    let plan_id = plan["body"]["plan_id"].as_str().unwrap().to_owned();
    let (code, err) = cli(&["plan", "apply", &plan_id], &store);
    assert_eq!((code, err["code"].as_str()), (3, Some("needs_approval")), "{err}");
    assert!(data.join("big.bin").exists() && data.join("renders").exists(), "nothing moved without a review");
    let (code, shown) = cli(&["plan", "show", &plan_id], &store);
    assert_eq!((code, shown["body"]["plan_kind"].as_str()), (0, Some("manual_selection")));

    approve(&store, &plan);
    let (code, report) = cli(&["plan", "apply", &plan_id], &store);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["status"], "completed");
    assert!(report["items"].as_array().unwrap().iter().all(|i| i["recovery"] == "trash" && i["message"].as_str().unwrap().contains("Restore")));
    assert_eq!((report["processed_logical_bytes"].as_str(), report["estimated_freed_bytes"].as_str()), (Some("68536"), Some("0")));
    assert!(!data.join("big.bin").exists() && !data.join("renders").exists());
    let (_, history) = cli(&["operations", "list"], &store);
    assert_eq!((history["items"][0]["action"].as_str(), history["items"][0]["recovery"]["availability"].as_str()), (Some("trash"), Some("system_recycle_bin")));

    // MTR-03: the recycled objects are listed; deleting them permanently is a separate plan with its own review.
    let (code, bin) = cli(&["trash", "list"], &store);
    assert!(code == 0 || code == 5, "{bin}");
    let ours: Vec<&Value> = bin["items"].as_array().unwrap().iter().filter(|i| i["original_path"].as_str().is_some_and(|p| p.to_ascii_lowercase().contains(&marker))).collect();
    assert_eq!(ours.len(), 2, "{bin}");
    let ids: Vec<&str> = ours.iter().map(|i| i["trash_item_id"].as_str().unwrap()).collect();
    let (code, purge) = cli(&["trash", "purge", "--item", &ids.join(",")], &store);
    assert_eq!(code, 0, "{purge}");
    let acts = purge["body"]["items"].as_array().unwrap();
    assert!(acts.iter().all(|a| a["action_type"] == "fs_delete" && a["target_location"] == "platform_trash"), "{purge}");
    let purge_id = purge["body"]["plan_id"].as_str().unwrap().to_owned();
    let (code, _) = cli(&["plan", "apply", &purge_id], &store);
    assert_eq!(code, 3, "a separate approval is needed");
    approve(&store, &purge);
    let (code, report) = cli(&["plan", "apply", &purge_id], &store);
    assert_eq!(code, 0, "{report}");
    assert!(report["estimated_freed_bytes"].as_str().unwrap().parse::<u64>().unwrap() >= 68_536, "{report}");
    assert!(report["volume_free_before_bytes"].is_string() && report["observed_free_delta_bytes"].is_string(), "free space observed: {report}");
    let (_, bin) = cli(&["trash", "list"], &store);
    assert!(!bin["items"].as_array().unwrap().iter().any(|i| i["original_path"].as_str().is_some_and(|p| p.to_ascii_lowercase().contains(&marker))), "purged");
    let (_, history) = cli(&["operations", "list"], &store);
    assert_eq!((history["items"][0]["action"].as_str(), history["items"][0]["recovery"]["availability"].as_str()), (Some("delete"), Some("unavailable")));
}

/// MTR-04: a file held open fails as `in_use` and names the holding process; nothing is moved.
#[test]
fn a_held_file_reports_its_holder() {
    let base = scratch("held");
    let _guard = BinGuard(base.file_name().unwrap().to_string_lossy().to_ascii_lowercase());
    let (data, store) = (base.join("data"), base.join("store"));
    write(&data.join("app.log"), 50);
    let (_, scan) = cli(&["scan", "--root", data.to_str().unwrap()], &store);
    let scan_id = scan["scan_id"].as_str().unwrap().to_owned();
    let entry = entry_of(&store, &scan_id, &data.join("app.log"));
    let held = std::fs::OpenOptions::new().read(true).share_mode(1 | 2).open(data.join("app.log")).unwrap();
    let (_, ins) = cli(&["trash", "inspect", "--scan", &scan_id, "--entry", &entry], &store);
    let me = std::env::current_exe().unwrap().file_name().unwrap().to_string_lossy().to_ascii_lowercase();
    assert!(ins["holders"].as_array().unwrap().iter().any(|h| h.as_str().unwrap().to_ascii_lowercase().contains(&me)), "{ins}");
    let (_, plan) = cli(&["trash", "--scan", &scan_id, "--entry", &entry], &store);
    approve(&store, &plan);
    let (code, report) = cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()], &store);
    assert_eq!(code, 8, "failed actions make a failed operation: {report}");
    let item = &report["items"][0];
    assert_eq!((item["status"].as_str(), item["code"].as_str()), (Some("failed"), Some("in_use")));
    assert!(item["message"].as_str().unwrap().to_ascii_lowercase().contains(&me), "{item}");
    drop(held);
    assert!(data.join("app.log").exists());
}

fn cm_platform_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}
