use std::collections::BTreeSet;

use cm_core::insights::{Handling, SizeBasis};

use super::*;

const MODULE_SOURCE: &str = include_str!("../insights.rs");

/// DIN-02: the module only reads. No delete/move/trash call, no process launch, no registry write.
#[test]
fn module_has_no_write_or_execute_path() {
    let forbidden = [
        ["fs::rem", "ove"].concat(),
        ["fs::wr", "ite"].concat(),
        ["fs::ren", "ame"].concat(),
        ["Delete", "FileW"].concat(),
        ["Move", "FileEx"].concat(),
        ["IFile", "Operation"].concat(),
        ["Create", "Process"].concat(),
        ["Shell", "Execute"].concat(),
        ["std::process", "::Command"].concat(),
        ["ExecMethod", ""].concat(),
        ["RegSet", "Value"].concat(),
        ["KEY_", "WRITE"].concat(),
    ];
    for token in &forbidden {
        assert!(!MODULE_SOURCE.contains(token.as_str()), "insights.rs must not reference {token}");
    }
}

#[test]
fn entry_sizes_and_walks() {
    let dir = std::env::temp_dir().join(format!("cm-insights-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.bin"), vec![0u8; 1000]).unwrap();
    std::fs::write(dir.join("sub").join("b.bin"), vec![0u8; 24]).unwrap();
    let d = dir.to_str().unwrap();
    assert_eq!(entry_size(&format!(r"{d}\a.bin")), Ok(Some(1000)));
    assert_eq!(entry_size(&format!(r"{d}\missing.bin")), Ok(None));
    assert_eq!(entry_size(d), Ok(None), "a directory is not the file looked for");
    assert_eq!(walk_size(d, 100), Ok((1024, 2)));
    assert!(walk_size(d, 2).unwrap_err().contains("not measured"));
    assert!(walk_size(&format!(r"{d}\nope"), 100).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn volume_object_paths() {
    let p = r#"Win32_Volume.DeviceID="\\\\?\\Volume{a7617ca2-ac48-4203-9f45-37eb00000000}\\""#;
    assert_eq!(volume_from_object_path(p).as_deref(), Some(r"\\?\Volume{a7617ca2-ac48-4203-9f45-37eb00000000}\"));
    assert_eq!(volume_from_object_path("nothing"), None);
}

/// In-use system files are sized from their directory entry without being opened.
#[test]
fn paging_file_is_sized_without_opening_it() {
    let sys = system_drive();
    match entry_size(&format!(r"{sys}\pagefile.sys")) {
        Ok(Some(n)) => assert!(n > 0),
        Ok(None) => eprintln!("no paging file on {sys}"),
        Err(e) => panic!("root directory entry unreadable: {e}"),
    }
}

/// Real machine: structure only (which objects exist differs between machines).
#[test]
fn real_insights_are_guidance_only() {
    let items = list();
    let ids: BTreeSet<&str> = items.iter().map(|i| i.insight_id.as_str()).collect();
    assert_eq!(ids.len(), items.len());
    for i in &items {
        eprintln!("{} | {:?} | {:?} | {:?}", i.title, i.logical_bytes, i.count, i.size_note);
        assert!(matches!(i.handling, Handling::ReadOnlyGuidance | Handling::OpenSystemSettings | Handling::OpenOwnerTool), "{}", i.kind);
        assert!(!i.guidance.is_empty() && !i.explanation.is_empty());
        assert_eq!(i.logical_bytes.is_none(), i.size_basis == SizeBasis::NotMeasured, "{}", i.kind);
        if i.logical_bytes.is_none() {
            assert!(i.size_note.is_some(), "{} unknown size needs a reason", i.kind);
        }
        assert!(i.observed_at.is_some());
    }
    // WinSxS exists on every Windows installation and is never summed.
    let sxs = items.iter().find(|i| i.kind == "windows.component_store").expect("component store insight");
    assert_eq!(sxs.logical_bytes, None);
    // Shadow storage is either measured, or unknown with a reason (non-admin) - never silently 0 without a query.
    for s in items.iter().filter(|i| i.kind == "windows.shadow_storage") {
        assert!(s.size_basis == SizeBasis::SystemQuery || s.size_note.is_some());
    }
}

/// DIN-04 listing: files below Downloads with their modification time; links are not followed; nothing selected.
#[test]
fn download_entries_list_files_with_times() {
    let root = std::env::temp_dir().join(format!("cm-insights-dl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(r"sub\deeper")).unwrap();
    std::fs::write(root.join("old.iso"), vec![0u8; 300]).unwrap();
    std::fs::write(root.join(r"sub\deeper\new.txt"), b"x").unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(200 * 86_400);
    std::fs::File::options().write(true).open(root.join("old.iso")).unwrap().set_modified(old).unwrap();
    let (entries, note) = download_entries(&root);
    assert_eq!((entries.len(), note), (2, None));
    let now = cm_core::plan::parse_utc(&cm_core::time::now_utc()).unwrap();
    let review = cm_core::insights::old_downloads(raw(&root.to_string_lossy()), entries, now, 90, "t", None);
    assert_eq!(review.items.len(), 1);
    assert!(review.items[0].path.display().ends_with("old.iso") && review.items[0].installer_like && !review.items[0].selected);
    assert!(review.items[0].age_days >= 199);
    let _ = std::fs::remove_dir_all(&root);
}

/// Unplanned-read finding (WFA-68): `CM_RESOLVE_DOWNLOADS` points the old-downloads review at a fixture folder, the
/// redirect the macOS review honours too; with it set, the review lists the fixture and never the real Downloads.
#[test]
fn the_downloads_override_redirects_the_review_into_a_fixture() {
    assert_eq!(DOWNLOADS_OVERRIDE, "CM_RESOLVE_DOWNLOADS");
    let root = std::env::temp_dir().join(format!("cm-insights-dl-override-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("fixture-old.bin"), vec![0u8; 64]).unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(120 * 86_400);
    std::fs::File::options().write(true).open(root.join("fixture-old.bin")).unwrap().set_modified(old).unwrap();
    let redirected = downloads_root_from(Some(root.clone().into_os_string())).expect("the fixture folder");
    assert_eq!(redirected.display(), root.display().to_string());
    let review = old_downloads_in(Some(redirected), 90);
    assert_eq!(review.root.as_ref().map(|r| r.display()), Some(root.display().to_string()));
    assert_eq!(review.items.iter().map(|i| i.path.display()).collect::<Vec<_>>(), [root.join("fixture-old.bin").display().to_string()]);
    // A missing fixture is reported as unreadable, never replaced by the real folder.
    let gone = root.join("missing");
    let review = old_downloads_in(downloads_root_from(Some(gone.clone().into_os_string())), 90);
    assert_eq!((review.root.as_ref().map(|r| r.display()), review.items.len()), (Some(gone.display().to_string()), 0));
    // Unset or empty: the known folder (whatever it is on this machine), the same answer both ways.
    assert_eq!(downloads_root_from(Some(std::ffi::OsString::new())), downloads_root_from(None));
    let _ = std::fs::remove_dir_all(&root);
}
