//! Disk insights on macOS through the binary (parity 7.2/7.3, DIN-01..04): with experimental features on in a scratch
//! store and `CM_RESOLVE_HOME` pointing at a fixture home, `insights` lists the fake virtual machine and container
//! folders by allocated size, an unreadable device-backup folder as `permission_required`, the local snapshots as a
//! count without a size, swap and the purgeable estimate; `insights overview` turns them into cards; `insights
//! downloads` reviews the fixture Downloads folder with nothing selected. Nothing outside the fixture is changed.
#![cfg(target_os = "macos")]

use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use serde_json::Value;

fn scratch(name: &str) -> PathBuf {
    let t = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let p = t.join(format!("cm-cli-macins-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn cli(args: &[&str], root: &Path) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_cleanmanager"))
        .args(args)
        .arg("--json")
        .env("CM_STORE_DIR", root.join("store"))
        .env_remove("CLEANMANAGER_LANG")
        .env_remove("CM_RESOLVE_DOWNLOADS")
        .env("CM_RESOLVE_HOME", root.join("home"))
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "exactly one record: {text}");
    (out.status.code().unwrap(), serde_json::from_str(&text).unwrap())
}

fn sparse(path: &Path, logical: u64, written: usize) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(&vec![7u8; written]).unwrap();
    f.seek(SeekFrom::Start(logical - 1)).unwrap();
    f.write_all(&[1]).unwrap();
}

fn write(p: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

#[test]
fn insights_through_the_binary_with_a_fixture_home() {
    let root = scratch("flow");
    let home = root.join("home");
    let docker = home.join("Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw");
    sparse(&docker, 32 << 30, 4096);
    sparse(&home.join(".colima/_lima/colima/diffdisk"), 60 << 30, 8192);
    write(&home.join("Virtual Machines.localized/Win.vmwarevm/disk.vmdk"), &[1u8; 2048]);
    let backups = home.join("Library/Application Support/MobileSync/Backup");
    write(&backups.join("00008030-001/Manifest.db"), b"db");
    std::fs::set_permissions(&backups, std::fs::Permissions::from_mode(0o000)).unwrap();
    let old = SystemTime::now() - Duration::from_secs(120 * 86_400);
    for (name, size) in [("Installer.dmg", 5000usize), ("notes.pdf", 40)] {
        let p = home.join("Downloads").join(name);
        write(&p, &vec![0u8; size]);
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(old).unwrap();
    }
    write(&home.join(".npm/_cacache/x"), &[0u8; 1000]);

    // D15: off by default.
    let (code, e) = cli(&["insights"], &root);
    assert_eq!((code, e["code"].as_str()), (7, Some("unsupported")), "{e}");
    let (code, _) = cli(&["experimental", "--state", "on"], &root);
    assert_eq!(code, 0);

    let (code, list) = cli(&["insights"], &root);
    std::fs::set_permissions(&backups, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!((code, list["kind"].as_str()), (0, Some("insight_list")), "{list}");
    let items = list["items"].as_array().unwrap();
    for i in items {
        eprintln!("{i}");
        assert!(["read_only_guidance", "open_system_settings", "open_owner_tool"].contains(&i["handling"].as_str().unwrap()), "{i}");
        assert!(!i["guidance"].as_array().unwrap().is_empty() && i.get("allowed_modes").is_none());
        assert_eq!(i["logical_bytes"].is_null(), i["size_basis"] == "not_measured", "{i}");
        if i["logical_bytes"].is_null() {
            assert!(i["size_note"].is_string(), "unknown sizes say why: {i}");
        }
    }
    let by = |kind: &str| items.iter().find(|i| i["kind"] == kind).unwrap_or_else(|| panic!("{kind} missing: {list}"));
    let d = by("macos.docker_disk");
    let allocated = std::fs::metadata(&docker).unwrap().blocks() * 512;
    assert_eq!((d["size_basis"].as_str(), d["logical_bytes"].as_str()), (Some("allocated"), Some(allocated.to_string().as_str())));
    assert_eq!(by("macos.colima_vms")["size_basis"], "allocated");
    assert_eq!((by("macos.vmware_vms")["count"].as_u64(), by("macos.vmware_vms")["size_basis"].as_str()), (Some(1), Some("allocated")));
    let b = by("macos.device_backups");
    assert!(b["logical_bytes"].is_null() && b["size_note"].as_str().unwrap().starts_with("permission_required"), "{b}");
    let s = by("macos.local_snapshots");
    assert!(s["logical_bytes"].is_null() && s["count"].is_u64(), "a known count, no size: {s}");
    assert_eq!(by("macos.swap")["size_basis"], "system_query");
    assert_eq!(by("macos.purgeable_space")["size_basis"], "estimate");

    let (code, o) = cli(&["insights", "overview", "--older-than-days", "90"], &root);
    assert_eq!((code, o["kind"].as_str()), (0, Some("insight_overview")), "{o}");
    let cards = o["cards"].as_array().unwrap();
    let card = |k: &str| cards.iter().find(|c| c["kind"] == k).unwrap_or_else(|| panic!("{k} card missing: {o}"));
    assert!(cards.iter().all(|c| c["observed_at"].is_string() && (c["logical_bytes"].is_string() || c["size_note"].is_string())), "{o}");
    let dl = card("old_downloads");
    assert_eq!((dl["logical_bytes"].as_str(), dl["count"].as_u64()), (Some("5040"), Some(2)));
    let snap = card("snapshots");
    assert!(snap["logical_bytes"].is_null() && snap["count"].is_u64(), "{snap}");
    assert!(card("purgeable_space")["size_note"].as_str().unwrap().starts_with("Estimate: "));
    assert!(card("virtual_machines")["logical_bytes"].is_string());
    assert!(card("developer_tools")["logical_bytes"].is_string());
    assert_eq!(o["old_downloads"]["item_count"], 2);

    let (code, r) = cli(&["insights", "downloads", "--older-than-days", "90", "--limit", "1"], &root);
    assert_eq!((code, r["kind"].as_str(), r["handling"].as_str()), (0, Some("old_downloads_review"), Some("manual_trash_review")), "{r}");
    assert_eq!((r["shown"].as_u64(), r["item_count"].as_u64(), r["total_logical_bytes"].as_str()), (Some(1), Some(2), Some("5040")));
    assert_eq!((r["items"][0]["logical_bytes"].as_str(), r["items"][0]["selected"].as_bool(), r["items"][0]["installer_like"].as_bool()), (Some("5000"), Some(false), Some(true)));
    assert!(r["root"]["display"].as_str().is_some_and(|p| p.ends_with("/home/Downloads")), "{r}");

    // Read-only: every fixture file is still there.
    assert!(docker.exists() && home.join("Downloads/Installer.dmg").exists() && backups.join("00008030-001/Manifest.db").exists());
    let _ = std::fs::remove_dir_all(&root);
}
