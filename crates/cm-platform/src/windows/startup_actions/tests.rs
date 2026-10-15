//! Startup changes on a throwaway registry key (`HKCU\Software\CleanManagerTest\…`, created and deleted here) and
//! temporary folders. The real Run keys and enable records are only read, before and after, to prove they are untouched.

use std::collections::BTreeMap;

use cm_core::capability::CapabilityState;
use cm_core::plan::v2::StartupOperation;
use cm_core::startup::{StartupItem, StartupScope, StartupState, change_decision};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyW, RegDeleteTreeW};

use super::super::startup::{APPROVED, RUN, RegKey, inventory_with};
use super::*;

/// A throwaway key under `HKCU\Software\CleanManagerTest\`, deleted (with everything below it) on drop.
struct TestRoot(String);

impl TestRoot {
    fn new(name: &str) -> TestRoot {
        let root = format!(r"Software\CleanManagerTest\t{}_{name}", std::process::id());
        let r = TestRoot(root);
        r.delete();
        r
    }
    fn delete(&self) {
        let z = wide(&self.0);
        // SAFETY: NUL-terminated subkey of HKCU; only the test key is removed.
        unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, z.as_ptr()) };
        // The shared parent goes too once no other test key is under it (RegDeleteKey refuses a key with subkeys).
        let parent = wide(r"Software\CleanManagerTest");
        // SAFETY: NUL-terminated subkey of HKCU.
        unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, parent.as_ptr()) };
    }
    fn env(&self, is_admin: bool) -> StartupEnv {
        StartupEnv { registry_root: self.0.clone(), user_folder: None, common_folder: None, is_admin }
    }
    /// Create `{root}\{hive}\{Run}` and set a REG_SZ value.
    fn run_value(&self, hive: &str, name: &str, command: &str) {
        let k = Key::create(HKEY_CURRENT_USER, &format!(r"{}\{hive}\{RUN}", self.0), KEY_SET_VALUE).unwrap();
        let data: Vec<u8> = command.encode_utf16().chain(std::iter::once(0)).flat_map(|u| u.to_le_bytes()).collect();
        k.set(name, REG_SZ, &data).unwrap();
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        self.delete();
    }
}

/// Values of the real per-user Run key and its enable records (read-only).
fn real_snapshot() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for key in [RUN.to_owned(), format!(r"{APPROVED}\Run"), format!(r"{APPROVED}\StartupFolder")] {
        if let Ok(k) = RegKey::open(HKEY_CURRENT_USER, &key, 0) {
            for (n, t, d) in k.values().unwrap_or_default() {
                out.insert(format!("{key}:{n}"), format!("{t}:{}", cm_core::startup::hex(&d)));
            }
        }
    }
    out
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cm-startup-act-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn item<'a>(items: &'a [StartupItem], name: &str) -> &'a StartupItem {
    items.iter().find(|i| i.name == name).unwrap_or_else(|| panic!("{name} listed: {items:?}"))
}

fn offline_drive() -> Option<char> {
    ('D'..='Z').rev().find(|c| !std::path::Path::new(&format!(r"{c}:\")).exists())
}

#[test]
fn test_roots_are_confined_to_the_test_key() {
    assert!(StartupEnv::checked_root(r"Software\CleanManagerTest\abc_1").is_ok());
    for bad in [r"Software\Microsoft\Windows\CurrentVersion\Run", r"Software\CleanManagerTest", r"Software\CleanManagerTest\", r"Software\CleanManagerTest\..\Microsoft", r"Software\CleanManagerTest\a\..", ""] {
        assert!(StartupEnv::checked_root(bad).is_err(), "{bad}");
    }
    let env = StartupEnv { registry_root: r"Software\CleanManagerTest\x".into(), ..Default::default() };
    let (root, sub, flags) = env.physical(Hive::Hklm, RUN, cm_core::startup::RegView::Wow64_32);
    assert_eq!((root, flags), (HKEY_CURRENT_USER, 0), "a redirected HKLM is a key under HKCU, never HKLM");
    assert_eq!(sub, format!(r"Software\CleanManagerTest\x\HKLM\{RUN}"));
}

/// STA-02 / 11.2: inventory capabilities, admin gate (injected non-admin state), reversible disable/enable with the
/// prior state recorded first, and changes since planning skipped.
#[test]
fn enable_records_toggle_reversibly_on_a_throwaway_key() {
    let before = real_snapshot();
    let root = TestRoot::new("toggle");
    let dir = tmp("toggle");
    let tool = dir.join("tool.exe");
    std::fs::write(&tool, b"MZ not really").unwrap();
    root.run_value("HKCU", "CMTestApp", &format!("\"{}\" --background", tool.display()));
    root.run_value("HKLM", "CMMachineApp", &format!("\"{}\"", tool.display()));

    // Not an administrator (injected): the machine-wide item says so; the user item toggles.
    let (items, issues) = inventory_with(&root.env(false));
    assert!(issues.is_empty(), "{issues:?}");
    assert!(items.iter().all(|i| i.definition.contains("CleanManagerTest")), "only the test key is read: {items:?}");
    let user = item(&items, "CMTestApp");
    assert_eq!((user.scope, user.state, user.toggle_capability), (StartupScope::User, StartupState::Enabled, CapabilityState::Available));
    let machine = item(&items, "CMMachineApp");
    assert_eq!((machine.scope, machine.toggle_capability), (StartupScope::System, CapabilityState::PermissionRequired));
    assert_eq!(change_decision(machine, StartupOperation::Disable, false).unwrap_err().code, "permission_required");
    assert_eq!(change_decision(machine, StartupOperation::Disable, true), Ok(()));
    let (items_admin, _) = inventory_with(&root.env(true));
    assert_eq!(item(&items_admin, "CMMachineApp").toggle_capability, CapabilityState::Available);

    let ws = WindowsStartup { env: root.env(false) };
    let loc = user.location.clone().unwrap();
    let obs = ws.observe(&loc).unwrap();
    assert!(obs.definition.is_some() && obs.approval.is_none() && obs.enabled());
    let expected = observation_digest(&loc, &obs);

    // A backup that cannot be stored stops the change.
    let out = ws.set_enabled(&loc, false, &expected, &mut |_, _| Err("disk full".into()));
    assert!(matches!(&out, StartupOutcome::Failed { code, .. } if code == "backup_failed"), "{out:?}");
    assert_eq!(ws.observe(&loc).unwrap(), obs, "nothing changed");

    let mut recorded = None;
    assert_eq!(ws.set_enabled(&loc, false, &expected, &mut |o, w| {
        recorded = Some((o.clone(), w.cloned()));
        Ok(())
    }), StartupOutcome::Done);
    let (prior, written) = recorded.unwrap();
    assert_eq!(prior, obs, "the prior state is what was planned");
    let now = ws.observe(&loc).unwrap();
    assert!(!now.enabled());
    assert_eq!(now.approval, written);
    let data = written.as_ref().unwrap().data().unwrap();
    assert_eq!((data.len(), data[0]), (12, 3), "Task Manager's disabled record");
    let (items, _) = inventory_with(&root.env(false));
    assert_eq!(item(&items, "CMTestApp").state, StartupState::Disabled);
    // The same plan again: the state changed since planning, so nothing is written.
    let again = ws.set_enabled(&loc, false, &expected, &mut |_, _| Ok(()));
    assert!(matches!(&again, StartupOutcome::Skipped { code, .. } if code == "changed_since_plan"), "{again:?}");

    // Restore puts the prior state back (no record at all), and only while the record is still ours.
    assert_eq!(ws.restore_approval(&loc, written.as_ref(), prior.approval.as_ref()), StartupOutcome::Done);
    assert_eq!(ws.observe(&loc).unwrap(), obs);
    let twice = ws.restore_approval(&loc, written.as_ref(), prior.approval.as_ref());
    assert!(matches!(&twice, StartupOutcome::Skipped { code, .. } if code == "changed_since"), "{twice:?}");

    // A location from another root is never acted on.
    let mut other = loc.clone();
    if let StartupLocation::RunValue { root, .. } = &mut other {
        *root = String::new();
    }
    let out = ws.set_enabled(&other, false, &expected, &mut |_, _| Ok(()));
    assert!(matches!(&out, StartupOutcome::Skipped { code, .. } if code == "root_mismatch"), "{out:?}");

    drop(root);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(real_snapshot(), before, "real Run values and enable records are untouched");
}

/// STA-03 / 11.3: stale (missing on a mounted volume) vs. offline volume, backup-before-delete, and a restore that
/// never overwrites a value recreated since.
#[test]
fn stale_run_values_are_removed_only_after_backup_and_restored_unless_recreated() {
    let before = real_snapshot();
    let root = TestRoot::new("stale");
    let dir = tmp("stale");
    let missing = dir.join("uninstalled.exe");
    root.run_value("HKCU", "CMStale", &format!("\"{}\" /tray", missing.display()));
    if let Some(letter) = offline_drive() {
        root.run_value("HKCU", "CMOffline", &format!(r#""{letter}:\Apps\tool.exe""#));
    }
    let env = root.env(false);
    let (items, _) = inventory_with(&env);
    let stale = item(&items, "CMStale");
    assert_eq!(stale.state, StartupState::Broken);
    assert!(stale.removable_when_stale);
    assert_eq!(change_decision(stale, StartupOperation::RemoveWithBackup, false), Ok(()));
    let ws = WindowsStartup { env: env.clone() };
    if offline_drive().is_some() {
        let off = item(&items, "CMOffline");
        assert_eq!(off.state, StartupState::TargetUnreachable, "an offline volume is unknown, never stale");
        assert_eq!(change_decision(off, StartupOperation::RemoveWithBackup, false).unwrap_err().code, "conflict");
        let oloc = off.location.clone().unwrap();
        let oexp = observation_digest(&oloc, &ws.observe(&oloc).unwrap());
        let out = ws.remove_stale(&oloc, &oexp, &mut |_| Ok(()));
        assert!(matches!(&out, StartupOutcome::Skipped { code, .. } if code == "target_unreachable"), "{out:?}");
    }

    let loc = stale.location.clone().unwrap();
    let obs = ws.observe(&loc).unwrap();
    let expected = observation_digest(&loc, &obs);
    // Backup failure: nothing removed.
    let out = ws.remove_stale(&loc, &expected, &mut |_| Err("audit store unavailable".into()));
    assert!(matches!(&out, StartupOutcome::Failed { code, .. } if code == "backup_failed"), "{out:?}");
    assert!(ws.observe(&loc).unwrap().definition.is_some());

    let mut backed_up = None;
    assert_eq!(ws.remove_stale(&loc, &expected, &mut |o| {
        backed_up = Some(o.clone());
        Ok(())
    }), StartupOutcome::Done);
    assert!(ws.observe(&loc).unwrap().definition.is_none());
    let prior = backed_up.unwrap();
    let backup = StartupBackup {
        backup_id: "sb_test".into(), startup_item_id: stale.item_id.clone(), operation: StartupOperation::RemoveWithBackup, location: loc.clone(),
        prior_definition: prior.definition.clone(), prior_approval: prior.approval.clone(), written_approval: None, created_at: "2026-10-15T00:00:00Z".into(),
    };
    assert_eq!(ws.restore_value(&backup), StartupOutcome::Done);
    assert_eq!(ws.observe(&loc).unwrap().definition, prior.definition, "recreated exactly (type and data)");
    // Recreated since (here: by the restore itself, then changed by someone): never overwritten.
    root.run_value("HKCU", "CMStale", r#""C:\Other\new.exe""#);
    let out = ws.restore_value(&backup);
    assert!(matches!(&out, StartupOutcome::Skipped { code, .. } if code == "destination_exists"), "{out:?}");
    assert_ne!(ws.observe(&loc).unwrap().definition, prior.definition, "the newer value is kept");

    drop(root);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(real_snapshot(), before, "real Run values and enable records are untouched");
}

/// Startup-folder files toggle through `StartupApproved\StartupFolder`; the file itself is never touched.
#[test]
fn startup_folder_files_toggle_without_touching_the_file() {
    let before = real_snapshot();
    let root = TestRoot::new("folder");
    let folder = tmp("folder");
    std::fs::write(folder.join("launch.bat"), b"@echo off").unwrap();
    let env = StartupEnv { user_folder: Some(folder.clone()), ..root.env(false) };
    let (items, _) = inventory_with(&env);
    let it = item(&items, "launch");
    assert_eq!(it.toggle_capability, CapabilityState::Available);
    let loc = it.location.clone().unwrap();
    let ws = WindowsStartup { env };
    let obs = ws.observe(&loc).unwrap();
    assert_eq!(ws.set_enabled(&loc, false, &observation_digest(&loc, &obs), &mut |_, _| Ok(())), StartupOutcome::Done);
    assert!(!ws.observe(&loc).unwrap().enabled());
    assert_eq!(std::fs::read(folder.join("launch.bat")).unwrap(), b"@echo off");
    drop(root);
    let _ = std::fs::remove_dir_all(&folder);
    assert_eq!(real_snapshot(), before);
}
