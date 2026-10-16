//! Uninstall backend on a throwaway registry key (`HKCU\Software\CleanManagerTest\…`, created and deleted here), scratch
//! folders and copies of harmless system programs started by the test itself. No real application is uninstalled, no
//! real uninstall or Run key is written (they are only read, before and after, to prove it), no real app data is touched.

use std::collections::BTreeMap;

use cm_core::uninstall::{AdapterChoice, AdapterKind, Group, KeyBackup, Marker, select_adapter};
use windows_sys::Win32::System::Registry::{RegDeleteKeyW, RegDeleteTreeW};

use super::*;

struct TestRoot(String);

impl TestRoot {
    fn new(name: &str) -> TestRoot {
        let r = TestRoot(format!(r"Software\CleanManagerTest\u{}_{name}", std::process::id()));
        r.delete();
        r
    }
    fn delete(&self) {
        let z = wide(&self.0);
        // SAFETY: NUL-terminated subkey of HKCU; only the test key is removed.
        unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, z.as_ptr()) };
        let z = wide(&self.0);
        // SAFETY: as above (the now empty key itself).
        unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, z.as_ptr()) };
        // The shared parent `Software\CleanManagerTest` is left in place: deleting it while a test running in parallel
        // opens a key below it fails that test with ERROR_KEY_DELETED (1018). The CLI uninstall tests remove the tree.
    }
    fn env(&self, home: &Path) -> UninstallEnv {
        UninstallEnv { registry_root: self.0.clone(), home: Some(home.to_path_buf()), is_admin: false, quit_wait: Duration::from_millis(600) }
    }
    /// `{root}\{hive}\{path}` value.
    fn set(&self, hive: &str, path: &str, name: &str, value: &str) {
        let (k, _) = create_key(HKEY_CURRENT_USER, &format!(r"{}\{hive}\{path}", self.0), 0).unwrap();
        let data: Vec<u8> = value.encode_utf16().chain(std::iter::once(0)).flat_map(|u| u.to_le_bytes()).collect();
        let wname = wide(name);
        // SAFETY: NUL-terminated name; data buffer of the given length.
        assert_eq!(unsafe { RegSetValueExW(k.0, wname.as_ptr(), 0, REG_SZ, data.as_ptr(), data.len() as u32) }, ERROR_SUCCESS);
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        self.delete();
    }
}

/// Subkey names of the real uninstall keys and per-user Run values (read-only).
fn real_snapshot() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (root, path, flags) in [(HKEY_CURRENT_USER, UNINSTALL, 0), (HKEY_LOCAL_MACHINE, UNINSTALL, KEY_WOW64_64KEY), (HKEY_LOCAL_MACHINE, UNINSTALL, KEY_WOW64_32KEY)] {
        if let Ok(k) = RegKey::open(root, path, flags) {
            for n in k.subkeys().unwrap_or_default() {
                out.insert(format!("{path}:{flags}:{n}"), String::new());
            }
        }
    }
    if let Ok(k) = RegKey::open(HKEY_CURRENT_USER, r"Software\Microsoft\Windows\CurrentVersion\Run", 0) {
        for (n, t, d) in k.values().unwrap_or_default() {
            out.insert(format!("run:{n}"), format!("{t}:{}", cm_core::startup::hex(&d)));
        }
    }
    out
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cm-uninst-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn cmd_copy(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::copy(format!(r"{}\cmd.exe", system_dir()), &p).unwrap();
    p
}

#[test]
fn signatures_are_verified_embedded_or_by_catalog() {
    // A signed system binary (catalog-signed on current Windows) and an unsigned fixture.
    let msiexec = format!(r"{}\msiexec.exe", system_dir());
    let s = signer_of(&msiexec).expect("msiexec is signed");
    assert!(is_microsoft_signer(&s), "{s}");
    let d = scratch("sig");
    let unsigned = d.join("uninst.exe");
    std::fs::write(&unsigned, b"MZ not really a program").unwrap();
    assert_eq!(signer_of(&unsigned.to_string_lossy()), None);
    let choice = |program: &Path, publisher: &str| AdapterChoice {
        kind: AdapterKind::RegisteredCommand, program: program.to_string_lossy().into_owned(), argv: vec![], raw_args: true, publisher_required: Some(publisher.into()),
        microsoft_signed: false, display: String::new(), keeps_own_ui: true,
    };
    assert_eq!(verify_program(&choice(&unsigned, "Contoso")).unwrap_err().0, "policy_denied", "unsigned uninstallers are refused");
    let copy = cmd_copy(&d, "uninst.exe");
    let err = verify_program(&choice(&copy, "Contoso Ltd.")).unwrap_err();
    assert_eq!(err.0, "policy_denied", "signer and publisher differ: {}", err.1);
    let (sha, signer) = verify_program(&choice(&copy, "Microsoft Corporation")).unwrap();
    assert_eq!(sha.len(), 64);
    assert!(signer.is_some_and(|s| is_microsoft_signer(&s)));
    assert!(verify_program(&choice(&d.join("missing.exe"), "x")).is_err());
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn records_come_from_the_redirected_registration_only() {
    let before = real_snapshot();
    let root = TestRoot::new("rec");
    let home = scratch("rec-home");
    let install = home.join(r"Programs\FooApp");
    std::fs::create_dir_all(&install).unwrap();
    let key = format!(r"{UNINSTALL}\FooApp");
    root.set("HKCU", &key, "DisplayName", "Foo App 2.0");
    root.set("HKCU", &key, "Publisher", "Contoso Ltd.");
    root.set("HKCU", &key, "InstallLocation", &install.to_string_lossy());
    root.set("HKCU", &key, "UninstallString", &format!("\"{}\\uninst.exe\" /remove", install.display()));
    root.set("HKCU", &key, "QuietUninstallString", "\"x\" /S");
    root.set("HKLM", &format!(r"{UNINSTALL}\{{11111111-2222-3333-4444-555555555555}}"), "DisplayName", "Bar Suite");
    root.set("HKLM", r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Wingetty", "DisplayName", "Wingetty");
    root.set("HKLM", r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Wingetty", "WinGetPackageIdentifier", "Contoso.Wingetty");
    root.set("HKCU", &format!(r"{UNINSTALL}\Sensor"), "DisplayName", "CrowdStrike Falcon Sensor");
    let env = root.env(&home);
    let (recs, issues) = records(&env);
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(recs.len(), 4, "only the test registrations: {recs:?}");
    let foo = recs.iter().find(|r| r.name == "Foo App 2.0").unwrap();
    assert_eq!(foo.source, RecordSource::RegistryUninstall);
    assert!(foo.uninstall_string.as_deref().unwrap().ends_with("/remove"));
    assert!(foo.key.as_ref().unwrap().root.contains("CleanManagerTest"));
    let c = select_adapter(foo, &adapter_env(&env, foo), &|p: &str| Path::new(p).exists()).unwrap();
    assert_eq!((c.kind, c.argv.as_slice()), (AdapterKind::RegisteredCommand, &["/remove".to_owned()][..]));
    let msi = recs.iter().find(|r| r.name == "Bar Suite").unwrap();
    assert_eq!(msi.source, RecordSource::RegistryUninstall, "WindowsInstaller is not set: not an MSI record");
    let w = recs.iter().find(|r| r.name == "Wingetty").unwrap();
    assert_eq!(w.winget.as_ref().unwrap().package_id, "Contoso.Wingetty");
    assert_eq!(w.key.as_ref().unwrap().view, RegView::Wow64_32);
    assert_eq!(select_adapter(w, &adapter_env(&env, w), &|_: &str| false).unwrap_err().code, "unsupported", "winget is never searched for in a test root");
    let sensor = recs.iter().find(|r| r.name.contains("Falcon")).unwrap();
    assert_eq!(select_adapter(sensor, &adapter_env(&env, sensor), &|_: &str| false).unwrap_err().code, "policy_denied");
    // npm-global AI CLI and Scoop app under the scratch home.
    let pkg = home.join(r"AppData\Roaming\npm\node_modules\@openai\codex");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(pkg.join("package.json"), br#"{"name": "@openai/codex", "version": "0.9.1"}"#).unwrap();
    std::fs::create_dir_all(home.join(r"scoop\apps\ripgrep\current")).unwrap();
    let (recs, _) = records(&env);
    let codex = recs.iter().find(|r| r.ai_tool.as_deref() == Some("codex")).unwrap();
    assert_eq!((codex.source, codex.version.as_deref()), (RecordSource::NpmGlobal, Some("0.9.1")));
    assert!(recs.iter().any(|r| r.source == RecordSource::Scoop && r.name == "ripgrep"));
    drop(root);
    std::fs::remove_dir_all(&home).ok();
    assert_eq!(real_snapshot(), before, "real uninstall keys and Run values are untouched");
}

#[test]
fn a_running_app_that_does_not_close_is_reported_and_only_ended_on_request() {
    // UNI-02: a windowless copy of cmd.exe waiting on its input stands in for an app that ignores the close request.
    let d = scratch("proc");
    let exe = cmd_copy(&d, "fooapp.exe");
    let mut child = std::process::Command::new(&exe)
        .args(["/c", "pause"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let RunningState::Known { processes } = running_under(&d.to_string_lossy()) else { panic!("state known") };
    let me = processes.iter().find(|p| p.pid == child.id()).expect("the fixture is found by its image path").clone();
    let still = request_quit(std::slice::from_ref(&me), Duration::from_millis(500));
    assert_eq!(still.len(), 1, "no window to close: still running, nothing ended automatically");
    assert!(child.try_wait().unwrap().is_none());
    let own = RunningProcess { pid: std::process::id(), name: "fooapp.exe".into(), path: me.path.clone() };
    assert!(end_process(&own).is_err(), "never itself");
    let impostor = RunningProcess { path: Some(r"C:\elsewhere\fooapp.exe".into()), ..me.clone() };
    assert!(end_process(&impostor).is_err(), "a different image than planned is not ended");
    end_process(&me).unwrap();
    assert!(child.wait().is_ok());
    assert!(!alive(&me));
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn registry_keys_are_backed_up_deleted_and_restored_without_overwriting() {
    let before = real_snapshot();
    let root = TestRoot::new("keys");
    let home = scratch("keys-home");
    root.set("HKCU", r"SOFTWARE\Contoso\FooApp", "Theme", "dark");
    root.set("HKCU", r"SOFTWARE\Contoso\FooApp\Recent", "1", "C:\\x.foo");
    let env = root.env(&home);
    let ws = WindowsUninstall::non_interactive(env.clone());
    let loc = env.loc(Hive::Hkcu, RegView::Default, r"SOFTWARE\Contoso\FooApp".into());
    let tree = ws.read_key(&loc).unwrap().unwrap();
    assert_eq!(tree.count(), (2, 1));
    let backup = KeyBackup { backup_id: "ub_t".into(), location: loc.clone(), tree: tree.clone(), owner_evidence: "test".into(), created_at: "2026-10-15T00:00:00Z".into() };
    ws.delete_key(&loc).unwrap();
    assert_eq!(ws.read_key(&loc).unwrap(), None);
    ws.restore_key(&backup).unwrap();
    assert_eq!(ws.read_key(&loc).unwrap().as_ref(), Some(&tree), "restored as backed up");
    // A recreated key is never overwritten.
    root.set("HKCU", r"SOFTWARE\Contoso\FooApp", "Theme", "light");
    assert_eq!(ws.restore_key(&backup).unwrap_err().0, "destination_exists");
    assert!(ws.read_key(&loc).unwrap().unwrap().values.iter().any(|(_, v)| v.data().is_some_and(|d| utf16(&d) == "light")));
    // Never a shared or system key, never another root.
    let denied = env.loc(Hive::Hkcu, RegView::Default, r"SOFTWARE\Microsoft\Foo".into());
    assert_eq!(ws.delete_key(&denied).unwrap_err().0, "policy_denied");
    let other_root = RegLocation { root: String::new(), ..loc.clone() };
    assert_eq!(ws.delete_key(&other_root).unwrap_err().0, "policy_denied");
    drop(root);
    std::fs::remove_dir_all(&home).ok();
    assert_eq!(real_snapshot(), before);
}

#[test]
fn package_removal_of_an_unknown_package_changes_nothing() {
    let err = remove_package("CleanManager.Test.NoSuchPackage_1.0.0.0_x64__0000000000000").unwrap_err();
    assert!(err.contains("not registered"), "{err}");
    assert!(remove_package("bad name; rm").is_err());
}

#[test]
fn leftovers_are_grouped_with_identity_evidence_and_markers() {
    let root = TestRoot::new("left");
    let home = scratch("left-home");
    let env = root.env(&home);
    let roaming = home.join(r"AppData\Roaming");
    let local = home.join(r"AppData\Local");
    for d in [roaming.join(r"FooApp\Cache"), roaming.join(r"FooApp\models"), local.join(r"Contoso\FooApp\logs"), roaming.join("Update"), roaming.join("Foo"), local.join("FooAppX")] {
        std::fs::create_dir_all(&d).unwrap();
    }
    std::fs::write(roaming.join(r"FooApp\settings.json"), b"{}").unwrap();
    std::fs::write(roaming.join(r"FooApp\models\m.gguf"), vec![0u8; 2048]).unwrap();
    std::fs::write(local.join(r"Contoso\FooApp\logs\a.log"), b"log").unwrap();
    root.set("HKCU", r"SOFTWARE\Contoso\FooApp", "Theme", "dark");
    root.set("HKCU", r"SOFTWARE\Microsoft\FooApp", "x", "y");
    let identity = AppIdentity {
        instance_id: "app_foo".into(), name: "FooApp".into(), publisher: Some("Contoso Ltd.".into()), version: None, source: RecordSource::RegistryUninstall,
        install_location: Some(home.join(r"Programs\FooApp").to_string_lossy().into_owned()), key: None, key_name: Some("FooApp".into()), product_code: None,
        package_family: None, package_name: None, ai_tool: None,
    };
    let beta = InstalledApp { instance_id: "app_beta".into(), name: "FooApp Beta".into(), publisher: None, install_location: None, key_name: None };
    let out = discover(&env, &DiscoveryInput { identity: &identity, installed: &[], startup: &[] });
    let find = |suffix: &str| out.iter().find(|l| l.display.ends_with(suffix)).unwrap_or_else(|| panic!("{suffix}: {out:#?}"));
    assert!(find(r"FooApp\Cache").selected);
    let models = find(r"FooApp\models");
    assert_eq!((models.group, models.selected), (Group::PersonalData, false));
    assert_eq!(models.logical_bytes, Some(2048));
    assert!(!find(r"FooApp\settings.json").selected);
    assert!(find(r"Contoso\FooApp").selected, "publisher\\product folder of logs only");
    let key = find(r"SOFTWARE\Contoso\FooApp");
    assert_eq!((key.kind, key.selected), (LeftoverKind::RegistryKey, false));
    assert!(!out.iter().any(|l| l.display.contains("Microsoft") || l.display.ends_with("Update") || l.display.ends_with(r"\Foo") || l.display.ends_with("FooAppX")),
        "generic, shared-system and non-exact names never match: {out:#?}");
    // A beta of the same app still installed: its possible data stays.
    let out = discover(&env, &DiscoveryInput { identity: &identity, installed: std::slice::from_ref(&beta), startup: &[] });
    assert!(out.iter().filter(|l| l.display.contains(r"Roaming\FooApp")).all(|l| l.marker == Some(Marker::SharedOwner) && !l.selected && l.tier == "kept"));
    drop(root);
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn ai_cli_data_is_split_and_worktrees_are_blocked() {
    let root = TestRoot::new("ai");
    let home = scratch("ai-home");
    let env = root.env(&home);
    let data = home.join(".codex");
    for d in [data.join("sessions"), data.join("log"), data.join(r"worktrees\w1\.git")] {
        std::fs::create_dir_all(&d).unwrap();
    }
    std::fs::write(data.join("auth.json"), b"{}").unwrap();
    std::fs::write(data.join("config.toml"), b"").unwrap();
    let identity = AppIdentity {
        instance_id: "app_codex".into(), name: "Codex CLI".into(), publisher: None, version: None, source: RecordSource::NpmGlobal, install_location: None, key: None,
        key_name: None, product_code: None, package_family: None, package_name: Some("@openai/codex".into()), ai_tool: Some("codex".into()),
    };
    let out = discover(&env, &DiscoveryInput { identity: &identity, installed: &[], startup: &[] });
    let get = |n: &str| out.iter().find(|l| l.display.ends_with(n)).unwrap_or_else(|| panic!("{n}: {out:#?}"));
    assert!(get(r".codex\log").selected);
    assert!(!get(r".codex\sessions").selected && get(r".codex\sessions").group == Group::PersonalData);
    assert!(!get("config.toml").selected && get("config.toml").group == Group::Settings);
    assert!(!get("auth.json").selected && !get("auth.json").warnings.is_empty());
    assert_eq!(get("worktrees").tier, "blocked");
    drop(root);
    std::fs::remove_dir_all(&home).ok();
}

fn identity_at(install: &Path) -> AppIdentity {
    AppIdentity {
        instance_id: "app_foo".into(), name: "FooApp".into(), publisher: Some("Contoso Ltd.".into()), version: None, source: RecordSource::RegistryUninstall,
        install_location: Some(install.to_string_lossy().into_owned()), key: None, key_name: Some("FooApp".into()), product_code: None, package_family: None,
        package_name: None, ai_tool: None,
    }
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// Test-only: write a shortcut to `target` on a COM thread (the module under test only reads links).
fn make_links(links: Vec<(PathBuf, String)>) {
    super::super::startup::on_com_thread(move || {
        use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
        use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
        use windows::core::{HSTRING, Interface};
        for (link, target) in links {
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            // SAFETY: COM calls on the COM thread created by `on_com_thread`.
            unsafe {
                let l: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
                l.SetPath(&HSTRING::from(target.as_str())).unwrap();
                l.cast::<IPersistFile>().unwrap().Save(&HSTRING::from(link.as_os_str()), true).unwrap();
            }
        }
    })
    .unwrap();
}

/// Backdate a folder's modification time (orphan idleness, UNI-08).
fn backdate(dir: &Path, days: u64) {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_WRITE_ATTRIBUTES; FILE_FLAG_BACKUP_SEMANTICS opens a directory.
    let f = std::fs::OpenOptions::new().access_mode(0x100).custom_flags(0x0200_0000).open(dir).unwrap();
    f.set_modified(SystemTime::now() - Duration::from_secs(days * 86_400)).unwrap();
}

#[test]
fn external_config_references_are_read_only_hints_never_leftovers() {
    // UNI-05: configuration outside the app's folders that mentions its install folder is reported, not planned.
    let root = TestRoot::new("ext");
    let home = scratch("ext-home");
    let env = root.env(&home);
    let install = home.join(r"Programs\FooApp");
    std::fs::create_dir_all(&install).unwrap();
    let exe = install.join("fooapp.exe");
    let fwd = exe.to_string_lossy().replace('\\', "/");
    let files = [
        (home.join(".gitconfig"), format!("[core]\n\teditor = \"{}\" --wait\n", fwd.to_uppercase())),
        (home.join(r"Documents\PowerShell\Microsoft.PowerShell_profile.ps1"), format!("Set-Alias foo \"{}\"\n", exe.display())),
        (home.join(r"AppData\Roaming\Code\User\settings.json"), format!("{{\"foo.path\": \"{}\"}}\n", exe.to_string_lossy().replace('\\', "\\\\"))),
        // A longer folder name that only starts like the install folder, a profile without it, a file nobody reads.
        (home.join(".npmrc"), format!("prefix={}X\\npm\n", install.display())),
        (home.join(".bashrc"), "alias ll='ls -l'\n".to_owned()),
        (home.join("notes.txt"), format!("{}\n", exe.display())),
    ];
    for (p, t) in &files {
        write(p, t);
    }
    let identity = identity_at(&install);
    let refs = external_references(&env, &identity);
    assert_eq!(refs.len(), 3, "{refs:#?}");
    for (p, _) in &files[..3] {
        assert!(refs.iter().any(|r| r.starts_with(&p.display().to_string()) && r.contains("read-only hint")), "{}: {refs:#?}", p.display());
    }
    // Hints never become leftovers (so never part of a plan), and the files are only read.
    let out = discover(&env, &DiscoveryInput { identity: &identity, installed: &[], startup: &[] });
    assert!(out.iter().any(|l| l.group == Group::Bundle), "the folder itself is the bundle leftover: {out:#?}");
    for (p, t) in &files {
        assert!(!out.iter().any(|l| l.display.eq_ignore_ascii_case(&p.to_string_lossy())), "{} is not a leftover", p.display());
        assert_eq!(&std::fs::read_to_string(p).unwrap(), t, "unchanged");
    }
    assert!(external_references(&env, &AppIdentity { install_location: None, ..identity }).is_empty(), "no recorded folder: no hints");
    drop(root);
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn orphans_come_from_broken_startup_entries_and_are_review_only() {
    // UNI-08: data folders of an app whose program is gone (its Run entry is broken) with a clear identity; never
    // preselected, recently used folders kept, generic names and still-installed apps never matched.
    let before = real_snapshot();
    let root = TestRoot::new("orph");
    let home = scratch("orph-home");
    let env = root.env(&home);
    let (roaming, local) = (home.join(r"AppData\Roaming"), home.join(r"AppData\Local"));
    let run = r"Software\Microsoft\Windows\CurrentVersion\Run";
    let gone = local.join(r"Programs\GoneApp\gone.exe");
    root.set("HKCU", run, "GoneTray", &format!("\"{}\" --tray", gone.display()));
    root.set("HKCU", run, "CommonThing", &format!("\"{}\"", home.join(r"Programs\Common Files\helper.exe").display()));
    root.set("HKCU", run, "KeepTray", &format!("\"{}\"", home.join(r"Programs\StillHere\s.exe").display()));
    root.set("HKCU", &format!(r"{UNINSTALL}\StillHere"), "DisplayName", "StillHere");
    for d in [roaming.join(r"GoneApp\cache"), local.join("GoneApp"), roaming.join("StillHere"), roaming.join("Common Files"), roaming.join("Update")] {
        std::fs::create_dir_all(&d).unwrap();
    }
    for d in [roaming.join("GoneApp"), roaming.join("StillHere"), roaming.join("Common Files"), roaming.join("Update")] {
        backdate(&d, 45);
    }
    let (startup, _) = super::super::startup::inventory_with(&env.startup_env());
    assert_eq!(startup.len(), 3, "only the test root's Run values: {startup:#?}");
    let installed = installed_apps(&records(&env).0);
    let out = orphans(&env, &startup, &installed);
    assert!(out.iter().all(|l| !l.selected && matches!(l.tier.as_str(), "review_only" | "kept")), "never preselected: {out:#?}");
    let entry = out.iter().find(|l| l.kind == LeftoverKind::StartupEntry && l.display.contains("GoneTray")).unwrap_or_else(|| panic!("{out:#?}"));
    assert!(entry.display.contains(&gone.display().to_string()) && entry.evidence.iter().any(|e| e.contains("program missing")), "explains the missing target");
    let old = out.iter().find(|l| l.display.eq_ignore_ascii_case(&roaming.join("GoneApp").to_string_lossy())).unwrap_or_else(|| panic!("{out:#?}"));
    assert_eq!((old.tier.as_str(), old.marker, old.kind), ("review_only", None, LeftoverKind::Directory));
    assert!(old.evidence.iter().any(|e| e.contains("GoneTray")), "{old:#?}");
    let fresh = out.iter().find(|l| l.display.eq_ignore_ascii_case(&local.join("GoneApp").to_string_lossy())).unwrap();
    assert_eq!((fresh.tier.as_str(), fresh.marker), ("kept", Some(Marker::OwnerUnknown)), "used recently: not an orphan");
    let dirs: Vec<&str> = out.iter().filter(|l| l.kind == LeftoverKind::Directory).map(|l| l.display.as_str()).collect();
    assert_eq!(dirs.len(), 2, "no folder of an installed app, no generic folder: {dirs:#?}");
    drop(root);
    std::fs::remove_dir_all(&home).ok();
    assert_eq!(real_snapshot(), before);
}

#[test]
fn shortcuts_and_registrations_into_the_install_folder_are_separate_teardown_leftovers() {
    // UNI-09: each Start menu shortcut, desktop shortcut and App Paths registration pointing into the install folder is
    // its own leftover (so each becomes its own reported step); links elsewhere are not touched.
    let root = TestRoot::new("lnk");
    let home = scratch("lnk-home");
    let env = root.env(&home);
    let install = home.join(r"Programs\FooApp");
    std::fs::create_dir_all(&install).unwrap();
    let exe = cmd_copy(&install, "fooapp.exe");
    let programs = home.join(r"AppData\Roaming\Microsoft\Windows\Start Menu\Programs");
    let (menu, desk, other) = (programs.join(r"FooApp\FooApp.lnk"), home.join(r"Desktop\FooApp.lnk"), programs.join("Notes.lnk"));
    let notepad = format!(r"{}\notepad.exe", system_dir());
    make_links(vec![(menu.clone(), exe.to_string_lossy().into_owned()), (desk.clone(), exe.to_string_lossy().into_owned()), (other.clone(), notepad)]);
    root.set("HKCU", &format!(r"{APP_PATHS}\fooapp.exe"), "", &exe.to_string_lossy());
    root.set("HKCU", &format!(r"{APP_PATHS}\notepad.exe"), "", r"C:\Windows\notepad.exe");
    let identity = identity_at(&install);
    let out = discover(&env, &DiscoveryInput { identity: &identity, installed: &[], startup: &[] });
    let by = |p: &Path| out.iter().find(|l| l.display.eq_ignore_ascii_case(&p.to_string_lossy())).unwrap_or_else(|| panic!("{}: {out:#?}", p.display()));
    let m = by(&menu);
    assert_eq!((m.kind, m.group, m.selected, m.tier.as_str()), (LeftoverKind::Shortcut, Group::Startup, false, "review_only"));
    assert_eq!(m.binding, Binding::Path { path: menu.to_string_lossy().into_owned() }, "moved to the Recycle Bin when selected");
    assert!(m.evidence.iter().any(|e| e.contains("fooapp.exe")));
    let d = by(&desk);
    assert!(matches!(&d.binding, Binding::Manual { guidance } if guidance.contains("desktop")), "desktop shortcuts are reported with guidance: {d:#?}");
    assert!(!out.iter().any(|l| l.display.contains("Notes.lnk") || l.display.contains(r"App Paths\notepad.exe")), "links elsewhere are not leftovers");
    let reg = out.iter().find(|l| l.display.ends_with(r"App Paths\fooapp.exe")).unwrap_or_else(|| panic!("{out:#?}"));
    assert_eq!((reg.kind, reg.group, reg.selected), (LeftoverKind::RegistryKey, Group::Startup, false));
    let ids: BTreeSet<&str> = out.iter().map(|l| l.object_id.as_str()).collect();
    assert_eq!(ids.len(), out.len(), "every step has its own id");
    drop(root);
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn npm_global_ai_cli_uninstall_binds_node_and_npm_cli_with_fixed_argv() {
    // UNI-07/UNI-01: the AI CLI is removed by npm itself (`node npm-cli.js uninstall --global <package>`); in a test root
    // only a fixture Node.js under the test home is ever considered, and nothing is run here.
    let root = TestRoot::new("npm");
    let home = scratch("npm-home");
    let env = root.env(&home);
    let pkg = home.join(r"AppData\Roaming\npm\node_modules\@anthropic-ai\claude-code");
    write(&pkg.join("package.json"), r#"{"name": "@anthropic-ai/claude-code", "version": "2.0.1"}"#);
    let rec = records(&env).0.into_iter().find(|r| r.ai_tool.as_deref() == Some("claude-code")).expect("recognized");
    assert_eq!((rec.source, rec.package_name.as_deref(), rec.version.as_deref()), (RecordSource::NpmGlobal, Some("@anthropic-ai/claude-code"), Some("2.0.1")));
    let none = route(&env, &rec).unwrap_err();
    assert_eq!(none.code, "unsupported", "no fixture Node.js: the machine's own is never used in a test root");
    assert_eq!(route_capability(&Err(none)).state, CapabilityState::Unverified);
    let node_dir = home.join(r"Program Files\nodejs");
    std::fs::create_dir_all(&node_dir).unwrap();
    let node = cmd_copy(&node_dir, "node.exe");
    let npm_cli = node_dir.join(r"node_modules\npm\bin\npm-cli.js");
    write(&npm_cli, "// fixture, never run\n");
    let r = route(&env, &rec);
    let c = r.as_ref().unwrap();
    assert_eq!((c.kind, c.kind.adapter_id(), c.raw_args), (AdapterKind::NpmGlobal, "npm.uninstall-global", false));
    assert_eq!(c.program, node.to_string_lossy());
    assert_eq!(c.argv, [npm_cli.to_string_lossy().into_owned(), "uninstall".into(), "--global".into(), "@anthropic-ai/claude-code".into()]);
    assert!(c.kind.package_manager() && c.display.contains("npm uninstall --global @anthropic-ai/claude-code"));
    assert_eq!(route_capability(&r).state, CapabilityState::Available);
    let (sha, _) = verify_program(c).unwrap();
    assert_eq!(sha, sha256_file(&node).unwrap(), "the plan binds the program's hash");
    assert!(pkg.join("package.json").exists(), "nothing was run");
    drop(root);
    std::fs::remove_dir_all(&home).ok();
}
