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
