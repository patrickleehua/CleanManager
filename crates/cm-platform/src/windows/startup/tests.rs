use std::collections::BTreeSet;

use cm_core::startup::{StartupScope, StartupSource, StartupState};

use super::*;

/// The module source, used to prove at test time that it contains no write, toggle or execute path.
const MODULE_SOURCE: &str = include_str!("../startup.rs");

#[test]
fn module_has_no_write_toggle_or_execute_path() {
    // Assembled from parts so this test file does not itself contain the tokens.
    let forbidden = [
        ["RegSet", "Value"].concat(),
        ["RegCreate", "Key"].concat(),
        ["RegDelete", "Key"].concat(),
        ["RegDelete", "Value"].concat(),
        ["KEY_", "WRITE"].concat(),
        ["KEY_ALL", "_ACCESS"].concat(),
        ["KEY_SET", "_VALUE"].concat(),
        ["Shell", "Execute"].concat(),
        ["Create", "Process"].concat(),
        ["std::process", "::Command"].concat(),
        ["put_", "Enabled"].concat(),
        ["Set", "Enabled"].concat(),
        ["Register", "TaskDefinition"].concat(),
        ["Delete", "Task"].concat(),
        [".Ru", "n("].concat(),
        [".Resol", "ve("].concat(),
        ["STGM_", "WRITE"].concat(),
        ["STGM_", "READWRITE"].concat(),
        ["fs::rem", "ove"].concat(),
        ["fs::wr", "ite"].concat(),
    ];
    for token in &forbidden {
        assert!(!MODULE_SOURCE.contains(token.as_str()), "startup.rs must not reference {token}");
    }
    assert_eq!(read_access(KEY_WOW64_64KEY), KEY_READ | KEY_WOW64_64KEY);
    assert_eq!(read_access(0), KEY_READ);
}

#[test]
fn environment_expansion_and_target_checks() {
    let windir = expand_env("%SystemRoot%").unwrap();
    assert!(!windir.contains('%'));
    assert_eq!(expand_env("%CM_SURELY_UNDEFINED_VAR%\\x"), None);
    assert_eq!(check_path(&format!(r"{windir}\System32\kernel32.dll")), TargetCheck::Exists);
    assert_eq!(check_path(&format!(r"{windir}\System32\cm-no-such-file-{}.exe", std::process::id())), TargetCheck::Missing);
    assert_eq!(check_path(r"\\server\share\app.exe"), TargetCheck::Unchecked, "network targets are not probed");
    assert_eq!(check_path("relative.exe"), TargetCheck::Unchecked);
    // A drive letter with no volume behind it is offline, never broken.
    let free = (b'D'..=b'Z').rev().map(|c| c as char).find(|c| !std::path::Path::new(&format!(r"{c}:\")).exists());
    if let Some(letter) = free {
        assert_eq!(check_path(&format!(r"{letter}:\Apps\tool.exe")), TargetCheck::VolumeOffline);
    }
    let (t, c, _) = resolve_command("rundll32.exe shell32.dll,Control_RunDLL");
    assert_eq!(c, TargetCheck::Exists);
    assert!(t.unwrap().to_lowercase().ends_with(r"\system32\rundll32.exe"));
    let (t, c, note) = resolve_command(r#""%SystemRoot%\System32\notepad.exe" /a"#);
    assert_eq!(c, TargetCheck::Exists);
    assert!(t.unwrap().to_lowercase().ends_with(r"\system32\notepad.exe"));
    assert!(note.is_none());
}

#[test]
fn manifest_startup_task_executable() {
    let manifest = r#"<Package><Applications><Application Id="App" Executable="App\Main.exe">
        <Extensions>
          <uap5:Extension Category="windows.startupTask" Executable="App\Tray.exe" EntryPoint="Windows.FullTrustApplication">
            <uap5:StartupTask TaskId="TrayTask" Enabled="false" DisplayName="Tray" />
          </uap5:Extension>
          <uap5:Extension Category="windows.startupTask" Executable="Other.exe" EntryPoint="Windows.FullTrustApplication">
            <uap5:StartupTask TaskId="Second" Enabled="true" DisplayName="Second" />
          </uap5:Extension>
        </Extensions></Application></Applications></Package>"#;
    assert_eq!(manifest_startup_executable(manifest, "TrayTask").as_deref(), Some(r"App\Tray.exe"));
    assert_eq!(manifest_startup_executable(manifest, "Second").as_deref(), Some("Other.exe"));
    assert_eq!(manifest_startup_executable(manifest, "Missing"), None);
    // An extension without its own Executable runs the enclosing application's executable.
    let inherit = r#"<Package><Applications><Application Id="App" Executable="Term.exe" EntryPoint="Windows.FullTrustApplication">
        <Extensions><uap5:Extension Category="windows.startupTask">
          <uap5:StartupTask TaskId="OnLogin" Enabled="false" DisplayName="ms-resource:AppName" />
        </uap5:Extension></Extensions></Application></Applications></Package>"#;
    assert_eq!(manifest_startup_executable(inherit, "OnLogin").as_deref(), Some("Term.exe"));
}

#[test]
fn signer_of_unsigned_file_is_none() {
    let dir = std::env::temp_dir().join(format!("cm-startup-sig-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("unsigned.exe");
    std::fs::write(&f, b"MZ not really a program").unwrap();
    assert_eq!(authenticode_signer(f.to_str().unwrap()), None);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Test-only: write a shortcut to `target` (the module under test never saves links).
fn make_link(link: &str, target: &str) {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{HSTRING, Interface};
    // SAFETY: COM calls on the COM thread created by `on_com_thread`.
    unsafe {
        let l: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
        l.SetPath(&HSTRING::from(target)).unwrap();
        l.cast::<IPersistFile>().unwrap().Save(&HSTRING::from(link), true).unwrap();
    }
}

/// STA-01 startup folder source on a synthetic folder: a shortcut to an existing program, a shortcut whose target
/// is gone (broken), a plain script, and an entry disabled in Explorer's approval record.
#[test]
fn startup_folder_entries_from_a_synthetic_folder() {
    let dir = std::env::temp_dir().join(format!("cm-startup-folder-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.to_str().unwrap().to_owned();
    let gone = format!(r"{d}\gone-tool.exe");
    std::fs::write(&gone, b"x").unwrap();
    std::fs::write(format!(r"{d}\run.bat"), b"@echo off").unwrap();
    std::fs::write(format!(r"{d}\desktop.ini"), b"[.ShellClassInfo]").unwrap();
    let windir = expand_env("%SystemRoot%").unwrap();
    let notepad = format!(r"{windir}\System32\notepad.exe");
    let (d2, gone2) = (d.clone(), gone.clone());
    let items = on_com_thread(move || {
        make_link(&format!(r"{d2}\Notes.lnk"), &notepad);
        make_link(&format!(r"{d2}\Gone.lnk"), &gone2);
        std::fs::remove_file(&gone2).unwrap();
        assert!(link_target(&format!(r"{d2}\Notes.lnk")).unwrap().to_lowercase().ends_with(r"\system32\notepad.exe"));
        let mut b = Builder { items: Vec::new(), issues: Vec::new(), signers: HashMap::new() };
        let approved: HashMap<String, bool> = [("run.bat".to_owned(), false)].into_iter().collect();
        read_folder(&mut b, &d2, StartupScope::User, &approved);
        assert!(b.issues.is_empty());
        b.items
    })
    .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let by_name = |n: &str| items.iter().find(|i| i.name == n).unwrap_or_else(|| panic!("{n} listed"));
    assert_eq!(items.len(), 3, "desktop.ini is not an entry");
    let notes = by_name("Notes");
    assert_eq!((notes.source, notes.scope, notes.state), (StartupSource::StartupFolder, StartupScope::User, StartupState::Enabled));
    assert!(notes.target.as_ref().unwrap().display().to_lowercase().ends_with(r"\system32\notepad.exe"));
    assert!(notes.protected, "a target inside Windows is read-only");
    let broken = by_name("Gone");
    assert_eq!(broken.state, StartupState::Broken);
    assert!(broken.reason.as_deref().unwrap().contains("missing"));
    assert_eq!(by_name("run").state, StartupState::Disabled);
    assert!(items.iter().all(|i| i.toggle_capability != CapabilityState::Available));
}

/// Real machine: structure only, never specific entries.
#[test]
fn real_inventory_is_well_formed() {
    let (items, issues) = inventory();
    for i in &issues {
        eprintln!("issue: {} {} {}", i.code, i.source, i.message);
    }
    let ids: BTreeSet<&str> = items.iter().map(|i| i.item_id.as_str()).collect();
    assert_eq!(ids.len(), items.len(), "item IDs are unique");
    let sources: BTreeSet<StartupSource> = items.iter().map(|i| i.source).collect();
    eprintln!("{} items, sources {sources:?}", items.len());
    // Windows always registers logon/boot tasks under \Microsoft\Windows.
    assert!(sources.contains(&StartupSource::ScheduledTask), "Task Scheduler logon/boot tasks are listed");
    for i in &items {
        assert!(i.item_id.starts_with("st_") && !i.name.is_empty() && !i.definition.is_empty());
        if i.protected {
            assert_eq!(i.toggle_capability, CapabilityState::Unsupported, "{}", i.name);
            assert!(i.reason.is_some());
        } else {
            assert_ne!(i.toggle_capability, CapabilityState::Available, "toggling is not implemented in this build");
        }
        if i.state == StartupState::Broken {
            assert!(i.target.is_some(), "broken needs a checked target");
        }
        if i.source == StartupSource::ScheduledTask && i.definition.to_lowercase().starts_with(r"task scheduler \microsoft\") {
            assert!(i.protected, "Windows component tasks are read-only: {}", i.definition);
        }
        if i.publisher.is_none() {
            assert_eq!(i.publisher_evidence, PublisherEvidence::None);
        }
        let w = i.to_wire();
        for key in ["startup_item_id", "name", "source", "definition", "target", "publisher", "publisher_evidence", "scope", "state", "toggle_capability", "protected", "reason"] {
            assert!(w.get(key).is_some(), "{key}");
        }
    }
    assert!(items.iter().any(|i| i.scope == StartupScope::System));
}
