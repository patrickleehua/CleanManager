//! MVP 14.1: chosen install directory, never overwriting, PATH conflict reporting, exact PATH entry.

use std::cell::RefCell;

use super::*;
use crate::release::{ReleaseFile, sha256_bytes};

pub fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-selfinstall-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// A release layout in `dir`: program, two rules, notices and the manifest.
pub fn layout(dir: &Path, install_source: &str) -> ReleaseManifest {
    let files: [(&str, &[u8]); 4] = [
        (exe_name(), b"program bytes"),
        ("rules/windows/a.json", b"{\"a\":1}"),
        ("rules/registry/a@1.0.0.json", b"{\"r\":1}"),
        ("THIRD-PARTY-NOTICES.txt", b"notices"),
    ];
    for (p, b) in files {
        let path = dir.join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b).unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.join(exe_name()), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let m = ReleaseManifest {
        version: "0.1.0".into(), target: release::target_id(), install_source: install_source.into(),
        files: files.iter().map(|(p, b)| ReleaseFile { path: (*p).into(), sha256: sha256_bytes(b), size: b.len() as u64 }).collect(),
    };
    std::fs::write(dir.join(RELEASE_FILE), m.to_json()).unwrap();
    m
}

/// In-memory user PATH.
pub struct MemPath(pub RefCell<Option<(String, bool)>>);

impl UserPathStore for MemPath {
    fn read(&self) -> Result<Option<(String, bool)>, String> {
        Ok(self.0.borrow().clone())
    }
    fn write(&self, value: &str, expandable: bool) -> Result<(), String> {
        *self.0.borrow_mut() = Some((value.to_owned(), expandable));
        Ok(())
    }
}

fn path_of(dirs: &[&Path]) -> std::ffi::OsString {
    std::env::join_paths(dirs).unwrap()
}

/// A PATH value in this platform's syntax (`;` on Windows, `:` elsewhere).
pub fn path_value(entries: &[&str]) -> String {
    std::env::join_paths(entries).unwrap().into_string().unwrap()
}

/// Two unrelated PATH directories in this platform's spelling.
pub fn other_dirs() -> (&'static str, &'static str) {
    if cfg!(windows) { (r"C:\A", r"C:\B") } else { ("/opt/a", "/opt/b") }
}

#[test]
fn installs_into_a_new_directory_and_records_exactly_its_path_entry() {
    let root = scratch("ok");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let m = layout(&src, "zip");
    let target = root.join("Tools").join("CleanManager");
    let existing: [&str; 2] = if cfg!(windows) { [r"%USERPROFILE%\bin", r"C:\Other"] } else { ["/Users/u/bin", "/opt/other"] };
    let up = MemPath(RefCell::new(Some((path_value(&existing), true))));
    let empty = root.join("empty-path-dir");
    std::fs::create_dir_all(&empty).unwrap();
    let pl = plan(&src, &target, true, Some(&path_of(&[&empty])), Some(&up as &dyn UserPathStore)).unwrap();
    assert_eq!((pl.path_entry, pl.hits.len()), ("would_add", 0));
    assert!(!target.exists(), "planning writes nothing");
    assert_eq!(execute(&pl, Some(&up as &dyn UserPathStore)).unwrap(), "added");
    for f in &m.files {
        assert_eq!(release::sha256_file(&target.join(&f.path)).unwrap().0, f.sha256, "{}", f.path);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(target.join(exe_name())).unwrap().permissions().mode() & 0o777, 0o755, "the program stays executable");
    }
    let installed = ReleaseManifest::load(&target).unwrap().unwrap();
    assert_eq!(installed.install_source, "self_install");
    let state = InstallState::load(&target).unwrap();
    assert_eq!(state.path_entry_added.as_deref(), Some(target.display().to_string().as_str()));
    let (value, expandable) = up.read().unwrap().unwrap();
    assert!(expandable, "REG_EXPAND_SZ is preserved");
    assert_eq!(value, path_value(&[existing[0], existing[1], &target.display().to_string()]));
    // Installing again over it is a conflict, never an overwrite.
    assert!(matches!(plan(&src, &target, false, None, None), Err(InstallError::Conflict(m)) if m.contains("already installed")));
}

#[test]
fn never_installs_over_existing_files_or_from_an_incomplete_layout() {
    let root = scratch("refuse");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    layout(&src, "zip");
    let busy = root.join("busy");
    std::fs::create_dir_all(&busy).unwrap();
    std::fs::write(busy.join(exe_name()), b"someone else's program").unwrap();
    assert!(matches!(plan(&src, &busy, false, None, None), Err(InstallError::Conflict(_))));
    assert_eq!(std::fs::read(busy.join(exe_name())).unwrap(), b"someone else's program");
    assert!(matches!(plan(&src, Path::new("relative\\dir"), false, None, None), Err(InstallError::Invalid(_))));
    assert!(matches!(plan(&src, &src.join("nested"), false, None, None), Err(InstallError::Invalid(_))));
    std::fs::write(src.join("rules/windows/a.json"), b"changed").unwrap();
    assert!(matches!(plan(&src, &root.join("new"), false, None, None), Err(InstallError::Invalid(m)) if m.contains("incomplete or modified")));
    let loose = root.join("loose");
    std::fs::create_dir_all(&loose).unwrap();
    assert!(matches!(plan(&loose, &root.join("new"), false, None, None), Err(InstallError::Invalid(m)) if m.contains("not a release layout")));
}

#[test]
fn other_cleanmanager_programs_on_path_are_reported_and_block_the_path_entry() {
    let root = scratch("conflict");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    layout(&src, "zip");
    let other = root.join("other-bin");
    let same = root.join("same-bin");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::create_dir_all(&same).unwrap();
    std::fs::write(other.join(exe_name()), b"a different cleanmanager").unwrap();
    std::fs::copy(src.join(exe_name()), same.join(exe_name())).unwrap();
    let up = MemPath(RefCell::new(None));
    let pv = path_of(&[&other, &same, &other]);
    let target = root.join("dest");
    let pl = plan(&src, &target, true, Some(&pv), Some(&up as &dyn UserPathStore)).unwrap();
    let rel: Vec<_> = pl.hits.iter().map(|h| h.relation).collect();
    assert_eq!(rel, ["different", "same_release"], "duplicates are reported once, in PATH order");
    assert_eq!(pl.path_entry, "refused_conflict");
    assert_eq!(execute(&pl, Some(&up as &dyn UserPathStore)).unwrap(), "refused_conflict");
    assert!(up.read().unwrap().is_none(), "PATH untouched");
    assert_eq!(std::fs::read(other.join(exe_name())).unwrap(), b"a different cleanmanager", "the other program is untouched");
    assert!(InstallState::load(&target).unwrap().path_entry_added.is_none());
}

#[test]
fn a_failed_copy_removes_everything_it_created() {
    let root = scratch("rollback");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    layout(&src, "zip");
    let target = root.join("a").join("b");
    let pl = plan(&src, &target, false, None, None).unwrap();
    std::fs::remove_file(src.join("THIRD-PARTY-NOTICES.txt")).unwrap();
    assert!(matches!(execute(&pl, None), Err(InstallError::Io(m)) if m.contains("removed")));
    assert!(!root.join("a").exists());
}

#[cfg(windows)]
#[test]
fn path_entries_are_removed_exactly() {
    let up = MemPath(RefCell::new(Some((r"C:\A;D:\Tools\CM\;C:\B;d:\tools\cm".into(), false))));
    assert!(remove_entry(&up, r"D:\Tools\CM").unwrap());
    assert_eq!(up.read().unwrap().unwrap(), (r"C:\A;C:\B".to_owned(), false));
    assert!(!remove_entry(&up, r"D:\Tools\CM").unwrap());
    assert!(!append_entry(&up, r"c:\a\").unwrap(), "already present");
}

/// Outside Windows entries are `:`-separated and compared case-sensitively, as the shell looks them up.
#[cfg(not(windows))]
#[test]
fn path_entries_are_removed_exactly() {
    let up = MemPath(RefCell::new(Some(("/opt/a:/Tools/CM/:/opt/b:/tools/cm".into(), false))));
    assert!(remove_entry(&up, "/Tools/CM").unwrap());
    assert_eq!(up.read().unwrap().unwrap(), ("/opt/a:/opt/b:/tools/cm".to_owned(), false));
    assert!(!remove_entry(&up, "/Tools/CM").unwrap());
    assert!(!append_entry(&up, "/opt/a/").unwrap(), "already present");
}

/// The user PATH is a Windows setting (`HKCU\Environment`); elsewhere `--add-to-path` reports `unsupported` and
/// changes nothing.
#[cfg(not(windows))]
#[test]
fn there_is_no_user_path_to_change_outside_windows() {
    assert!(os_user_path().is_none());
    let root = scratch("no-user-path");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    layout(&src, "zip");
    let empty = root.join("empty-path-dir");
    std::fs::create_dir_all(&empty).unwrap();
    let target = root.join("Tools").join("CleanManager");
    let pl = plan(&src, &target, true, Some(&path_of(&[&empty])), None).unwrap();
    assert_eq!((pl.path_entry, pl.hits.len()), ("unsupported", 0));
    assert_eq!(execute(&pl, None).unwrap(), "unsupported");
    assert!(InstallState::load(&target).unwrap().path_entry_added.is_none());
}

#[cfg(windows)]
#[test]
fn the_registry_store_round_trips_expandable_values() {
    let key = format!("Software\\CleanManagerTest-{}", std::process::id());
    let reg = super::user_path::Registry { subkey: key.clone() };
    assert!(reg.read().unwrap().is_none());
    reg.write(r"%USERPROFILE%\x;C:\y", true).unwrap();
    assert!(append_entry(&reg, r"D:\Tools\CleanManager").unwrap());
    assert_eq!(reg.read().unwrap().unwrap(), (r"%USERPROFILE%\x;C:\y;D:\Tools\CleanManager".to_owned(), true));
    assert!(remove_entry(&reg, r"D:\Tools\CleanManager").unwrap());
    assert_eq!(reg.read().unwrap().unwrap(), (r"%USERPROFILE%\x;C:\y".to_owned(), true));
    super::user_path::delete_test_key(&key);
}
