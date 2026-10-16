//! LIF-02 / RES-07 with scratch install and store directories only.

use std::cell::RefCell;

use super::*;
use crate::install_self::tests::{MemPath, layout, other_dirs, path_value, scratch};
use crate::release::{InstallState, sha256_bytes};
use crate::update_self::InstallMethod;

fn standalone() -> Detected {
    Detected { method: InstallMethod::Standalone, update_command: None, uninstall_command: None }
}

struct Fixture {
    install: PathBuf,
    store: PathBuf,
    profile: PathBuf,
    other_profile: PathBuf,
    path: MemPath,
}

const PROFILE_HEAD: &str = "Set-Alias ll Get-ChildItem\n";
const PROFILE_TAIL: &str = "# my own line\n";

/// An installed release with a PATH entry and a completion block, plus a store with history,
/// settings, a protection list, an index and a file CleanManager does not know.
fn fixture(name: &str) -> Fixture {
    let root = scratch(&format!("uninst-{name}"));
    let install = root.join("Tools").join("CleanManager");
    std::fs::create_dir_all(&install).unwrap();
    layout(&install, "self_install");
    let state = InstallState { installed_at: "2026-10-15T00:00:00Z".into(), path_entry_added: Some(install.display().to_string()) };
    std::fs::write(install.join(INSTALL_STATE_FILE), state.to_json()).unwrap();
    std::fs::create_dir_all(install.join(BACKUP_DIR).join("0.0.9")).unwrap();
    std::fs::write(install.join(BACKUP_DIR).join("0.0.9").join(release::exe_name()), b"older program").unwrap();
    let store = root.join("store");
    drop(cm_store::Stores::open(&store).unwrap());
    for (f, body) in [("protection.json", "{}"), ("app-updates.json", "{}"), ("purge-last-run.json", "{}"), ("notes-by-user.txt", "mine")] {
        std::fs::write(store.join(f), body).unwrap();
    }
    let block = format!(
        "{}\n& '{}' completion print powershell --format text | Out-String | Invoke-Expression\n{}\n",
        crate::completion_cmd::BEGIN,
        install.join(release::exe_name()).display(),
        crate::completion_cmd::END
    );
    let profile = root.join("profile.ps1");
    std::fs::write(&profile, format!("{PROFILE_HEAD}{block}{PROFILE_TAIL}")).unwrap();
    // A block that loads another copy is not ours to remove.
    let other_profile = root.join(".bashrc");
    std::fs::write(&other_profile, format!("{}\neval \"$('D:/Elsewhere/cleanmanager.exe' completion print bash)\"\n{}\n", crate::completion_cmd::BEGIN, crate::completion_cmd::END)).unwrap();
    let (a, b) = other_dirs();
    let path = MemPath(RefCell::new(Some((path_value(&[a, &install.display().to_string(), b]), true))));
    Fixture { install, store, profile, other_profile, path }
}

fn inputs<'a>(f: &'a Fixture, d: &'a Detected, remove_user_data: bool) -> Inputs<'a> {
    Inputs {
        install_dir: &f.install,
        store_dir: Some(&f.store),
        detected: d,
        remove_user_data,
        profiles: vec![("powershell", f.profile.clone()), ("bash", f.other_profile.clone())],
        user_path: Some(&f.path),
        credential_targets: Vec::new(),
        resident_startup: None,
    }
}

fn no_credentials(_: &str) -> Result<(), String> {
    Err("not expected".into())
}

fn no_startup() -> Result<bool, String> {
    Err("not expected".into())
}

fn no_defer(_: &Path, _: &Path) -> Result<(), String> {
    Err("not expected".into())
}

fn effects<'a>(f: &'a Fixture) -> Effects<'a> {
    Effects { user_path: Some(&f.path), current_exe: None, delete_credential: &no_credentials, defer: &no_defer, remove_resident_startup: &no_startup }
}

fn actions(p: &Plan) -> Vec<(String, &'static str, &'static str)> {
    p.items.iter().map(|i| (i.path.file_name().unwrap_or_default().to_string_lossy().into_owned(), i.component, i.action)).collect()
}

#[test]
fn the_default_plan_removes_only_the_program_caches_and_its_own_system_changes() {
    let f = fixture("default");
    let d = standalone();
    let p = plan(&inputs(&f, &d, false));
    let a = actions(&p);
    // System changes first, then caches, the program last (the running copy goes last).
    assert_eq!(a.first().map(|x| (x.1, x.2)), Some(("system_setting", "remove_path_entry")));
    assert!(a.contains(&("profile.ps1".into(), "system_setting", "remove_profile_block")));
    assert!(!a.iter().any(|x| x.0 == ".bashrc"), "a block loading another copy is kept");
    for cache in ["index.db", "purge-last-run.json"] {
        assert!(a.contains(&(cache.into(), "caches", "remove_file")), "{cache}");
    }
    for kept in ["audit.db", "protection.json", "app-updates.json", "notes-by-user.txt", "store"] {
        assert!(!a.iter().any(|x| x.0 == kept), "{kept} must be kept");
    }
    for prog in [release::exe_name(), "a.json", "THIRD-PARTY-NOTICES.txt", RELEASE_FILE, INSTALL_STATE_FILE] {
        assert!(a.contains(&(prog.into(), "cli", "remove_file")), "{prog}");
    }
    assert_eq!(a.last().map(|x| (x.0.as_str(), x.2)), Some(("CleanManager", "remove_empty_dir")));
    let kinds: Vec<_> = p.keep.iter().map(|k| k.data).collect();
    assert!(kinds.contains(&"configuration") && kinds.contains(&"protection_list") && kinds.contains(&"history"), "{kinds:?}");
    assert!(p.notes.iter().any(|n| n.contains("notes-by-user.txt")));
    if cfg!(windows) {
        assert!(p.notes[0].contains("No service, scheduled task, helper or background process") && p.notes[0].contains("CleanManager.Resident (not present)"));
    } else {
        assert!(p.notes[0].contains("creates no startup entry") && !p.notes[0].contains("HKCU"), "{}", p.notes[0]);
    }
    assert!(!a.iter().any(|x| x.2 == "remove_run_value"));
    assert_eq!(p.quarantine_items, 0);
    assert!(p.warnings.is_empty(), "{:?}", p.warnings);
    // The digest binds the exact plan.
    assert_eq!(plan(&inputs(&f, &d, false)).digest, p.digest);
}

#[test]
fn executing_the_plan_restores_path_and_profile_and_keeps_user_data() {
    let f = fixture("execute");
    let d = standalone();
    let p = plan(&inputs(&f, &d, false));
    let done = execute(&p, &f.install, &effects(&f));
    assert!(done.iter().all(|x| x.outcome == "removed"), "{done:?}");
    assert!(!f.install.exists(), "the install directory is gone");
    assert!(f.install.parent().unwrap().exists(), "only CleanManager's own directory is removed");
    let (a, b) = other_dirs();
    assert_eq!(f.path.read().unwrap().unwrap(), (path_value(&[a, b]), true));
    assert_eq!(std::fs::read_to_string(&f.profile).unwrap(), format!("{PROFILE_HEAD}{PROFILE_TAIL}"));
    assert!(std::fs::read_to_string(&f.other_profile).unwrap().contains(crate::completion_cmd::BEGIN));
    for kept in ["audit.db", "protection.json", "app-updates.json", "notes-by-user.txt"] {
        assert!(f.store.join(kept).exists(), "{kept}");
    }
    assert!(!f.store.join("index.db").exists());
    // The kept history still opens.
    cm_store::Stores::open(&f.store).unwrap();
}

#[test]
fn a_non_empty_quarantine_is_reported_and_never_deleted() {
    let f = fixture("quarantine");
    {
        let s = cm_store::Stores::open(&f.store).unwrap();
        let host = cm_core::host::HostRef { kind: "cli".into(), pid: 1, started_at: "2026-10-15T00:00:00Z".into() };
        s.audit.begin_operation("op1", "p1", "d", "k", &host, "2026-10-15T00:00:00Z").unwrap();
        s.audit.set_operation_state("op1", "completed").unwrap();
        s.audit.save_quarantine("q1", "op1", "a01", "p1", "{\"logical_bytes\":\"2097152\"}", "2026-10-15T00:00:00Z", "2026-11-15T00:00:00Z").unwrap();
    }
    std::fs::create_dir_all(f.store.join("quarantine").join("op1")).unwrap();
    std::fs::write(f.store.join("quarantine").join("op1").join("a01"), b"quarantined bytes").unwrap();
    let d = standalone();
    for remove_user_data in [false, true] {
        let p = plan(&inputs(&f, &d, remove_user_data));
        assert_eq!((p.quarantine_items, p.quarantine_bytes), (1, 2_097_152));
        let w: Vec<_> = p.warnings.iter().map(|w| w.code).collect();
        assert!(w.contains(&"quarantine_not_empty"), "{w:?}");
        assert_eq!(w.contains(&"user_data_kept"), remove_user_data);
        let q = &p.warnings.iter().find(|w| w.code == "quarantine_not_empty").unwrap().message;
        assert!(q.contains("recovery plan create --restore") && q.contains("--purge"), "{q}");
        assert!(p.keep.iter().any(|k| k.data == "quarantine"));
        assert!(!p.items.iter().any(|i| i.component == "user_data" || i.path.starts_with(f.store.join("quarantine"))));
    }
}

#[test]
fn remove_user_data_with_an_empty_quarantine_removes_only_known_data() {
    let f = fixture("userdata");
    let d = standalone();
    let p = plan(&inputs(&f, &d, true));
    let a = actions(&p);
    for data in ["audit.db", "protection.json", "app-updates.json"] {
        assert!(a.contains(&(data.into(), "user_data", "remove_file")), "{data}");
    }
    assert!(!a.iter().any(|x| x.0 == "notes-by-user.txt"));
    assert!(!a.iter().any(|x| x.2 == "remove_empty_dir" && x.0 == "store"), "a store with unknown files stays");
    let done = execute(&p, &f.install, &effects(&f));
    assert!(done.iter().all(|x| x.outcome == "removed"), "{done:?}");
    assert!(!f.store.join("audit.db").exists());
    assert_eq!(std::fs::read_to_string(f.store.join("notes-by-user.txt")).unwrap(), "mine");
}

#[test]
fn modified_or_foreign_files_are_kept() {
    let f = fixture("modified");
    std::fs::write(f.install.join("rules/windows/a.json"), b"edited by the user").unwrap();
    std::fs::write(f.install.join("my-script.ps1"), b"user file").unwrap();
    let d = standalone();
    let p = plan(&inputs(&f, &d, false));
    assert!(p.warnings.iter().any(|w| w.code == "modified_since_install" && w.message.contains("a.json")));
    // A file changed after the plan was shown is kept too.
    std::fs::write(f.install.join("THIRD-PARTY-NOTICES.txt"), b"changed later").unwrap();
    let done = execute(&p, &f.install, &effects(&f));
    let skipped: Vec<_> = done.iter().filter(|x| x.outcome == "skipped").filter_map(|x| x.reason.clone()).collect();
    assert!(skipped.iter().any(|r| r.contains("changed since the plan")), "{skipped:?}");
    for kept in ["rules/windows/a.json", "my-script.ps1", "THIRD-PARTY-NOTICES.txt"] {
        assert!(f.install.join(kept).exists(), "{kept}");
    }
    assert!(!f.install.join(release::exe_name()).exists());
}

#[test]
fn package_manager_and_loose_copies_keep_their_program_files() {
    let f = fixture("managed");
    let winget = Detected { method: InstallMethod::Winget, update_command: Some("winget upgrade x".into()), uninstall_command: Some("winget uninstall --id X --exact".into()) };
    let p = plan(&inputs(&f, &winget, false));
    assert!(!p.items.iter().any(|i| i.component == "cli"));
    assert!(p.warnings.iter().any(|w| w.code == "package_manager" && w.message.contains("winget uninstall --id X --exact")));
    std::fs::remove_file(f.install.join(RELEASE_FILE)).unwrap();
    let loose = Detected { method: InstallMethod::Unpackaged, update_command: None, uninstall_command: None };
    let p = plan(&inputs(&f, &loose, false));
    assert!(!p.items.iter().any(|i| i.component == "cli"));
    assert!(p.warnings.iter().any(|w| w.code == "no_release_manifest"));
    assert_eq!(sha256_bytes(&std::fs::read(f.install.join(release::exe_name())).unwrap()), sha256_bytes(b"program bytes"));
}

#[test]
fn removing_all_user_data_also_removes_the_store_while_its_lock_is_held() {
    let f = fixture("userdata-all");
    std::fs::remove_file(f.store.join("notes-by-user.txt")).unwrap();
    let d = standalone();
    let first = plan(&inputs(&f, &d, true));
    // As in `self uninstall`: the exclusive lifecycle lock is taken (creating the lock file) between
    // showing the plan and re-checking it; the digest must not change because of it.
    let held = crate::instance_lock::exclusive(&f.store).unwrap();
    let again = plan(&inputs(&f, &d, true));
    assert_eq!(first.digest, again.digest);
    assert!(again.items.iter().any(|i| i.action == "remove_empty_dir" && i.path == f.store));
    let done = execute(&again, &f.install, &effects(&f));
    drop(held);
    assert!(done.iter().all(|x| x.outcome == "removed"), "{done:?}");
    assert!(!f.store.exists(), "the store directory is gone");
}

/// The resident Run value exists only on Windows; on macOS the resident component is not available (see startup_note).
#[cfg(windows)]
#[test]
fn the_resident_startup_value_is_listed_and_removed_by_its_own_name() {
    let f = fixture("resident");
    let d = standalone();
    let mut inp = inputs(&f, &d, false);
    inp.resident_startup = Some(r#"HKCU\Software\Microsoft\Windows\CurrentVersion\Run\CleanManager.Resident = "C:\x\cleanmanager-desktop.exe" --resident"#.into());
    let p = plan(&inp);
    let item = p.items.iter().find(|i| i.action == "remove_run_value").expect("listed");
    assert_eq!(item.component, "system_setting");
    assert!(item.display.contains("CleanManager.Resident"));
    assert!(p.notes[0].contains("present, listed for removal"));
    assert_ne!(p.digest, plan(&inputs(&f, &d, false)).digest, "the digest covers the startup value");
    let calls = std::cell::Cell::new(0);
    let remove = || {
        calls.set(calls.get() + 1);
        Ok(true)
    };
    let fx = Effects { remove_resident_startup: &remove, ..effects(&f) };
    let done = execute(&p, &f.install, &fx);
    assert_eq!(calls.get(), 1);
    let d = done.iter().find(|x| x.item_id == item.item_id).unwrap();
    assert_eq!(d.outcome, "removed");
}

/// Outside Windows there is no user PATH store (`self install --add-to-path` reports `unsupported`), so the plan never
/// lists a PATH entry and such an item fails without touching anything.
#[cfg(not(windows))]
#[test]
fn without_a_user_path_store_no_path_entry_is_planned_or_changed() {
    let f = fixture("no-user-path");
    let d = standalone();
    let p = plan(&Inputs { user_path: None, ..inputs(&f, &d, false) });
    assert!(!p.items.iter().any(|i| i.action == "remove_path_entry"), "{:?}", actions(&p));
    let item = Item { item_id: "i001".into(), component: "system_setting", action: "remove_path_entry", path: f.install.clone(), display: String::new(), sha256: None };
    let only = Plan { items: vec![item], ..p };
    let done = execute(&only, &f.install, &Effects { user_path: None, ..effects(&f) });
    assert_eq!((done[0].outcome, done[0].reason.as_deref()), ("failed", Some("no user PATH on this platform")));
}

#[test]
fn startup_note_names_only_this_platform_s_entry() {
    let note = startup_note(false);
    assert_eq!(note.contains("HKCU"), cfg!(windows), "{note}");
    if !cfg!(windows) {
        assert!(note.contains("macOS") && note.contains("no startup entry"), "{note}");
    }
}
