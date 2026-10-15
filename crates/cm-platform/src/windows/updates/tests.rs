//! Fixture directories only: no package manager is run and nothing is installed.

use super::*;

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-updates-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

#[test]
fn scoop_directory_is_read_without_network() {
    let root = scratch("scoop");
    write(&root.join(r"apps\ripgrep\current\manifest.json"), r#"{"version": "14.0.0", "homepage": "https://github.com/BurntSushi/ripgrep"}"#);
    write(&root.join(r"apps\ripgrep\current\install.json"), r#"{"bucket": "main", "architecture": "64bit"}"#);
    write(&root.join(r"buckets\main\bucket\ripgrep.json"), r#"{"version": "14.1.1"}"#);
    write(&root.join(r"buckets\main\.git\FETCH_HEAD"), "x");
    write(&root.join(r"apps\orphan\current\manifest.json"), r#"{"version": "1.0"}"#);
    write(&root.join(r"apps\scoop\current\manifest.json"), r#"{"version": "0.5"}"#);
    let apps = scoop_apps_in(&root);
    assert_eq!(apps.len(), 2, "scoop itself is not an app to update");
    let rg = &apps[1];
    assert_eq!((rg.name.as_str(), rg.installed_version.as_str(), rg.bucket_version.as_deref()), ("ripgrep", "14.0.0", Some("14.1.1")));
    assert!(rg.bucket_updated_at.is_some());
    assert_eq!(rg.homepage.as_deref(), Some("https://github.com/BurntSushi/ripgrep"));
    assert_eq!((apps[0].name.as_str(), apps[0].bucket.as_deref(), apps[0].bucket_version.as_deref()), ("orphan", None, None));
    let inst = scoop_instances(&apps, &[]);
    assert_eq!(inst.len(), 2);
    assert!(inst[1].install_location.as_ref().unwrap().display().ends_with(r"ripgrep\current"));
    assert!(scoop_apps_in(&root.join("missing")).is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn self_updaters_are_detected_from_files_only() {
    let root = scratch("squirrel");
    write(&root.join(r"Chatty\Update.exe"), "MZ");
    std::fs::create_dir_all(root.join(r"Chatty\app-5.0.1")).unwrap();
    write(&root.join(r"Editor\resources\app-update.yml"), "provider: generic");
    let app = |dir: &str| {
        let p = root.join(dir).to_string_lossy().into_owned();
        AppInstance::new(dir.into(), AppSource::Unknown, raw(&p))
    };
    assert!(self_updater(&app("Chatty")).unwrap().evidence.contains("app-5.0.1"));
    assert!(self_updater(&app("Editor")).unwrap().evidence.contains("electron-updater"));
    std::fs::create_dir_all(root.join("Plain")).unwrap();
    assert_eq!(self_updater(&app("Plain")), None);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn installed_signer_reads_top_level_executables() {
    let root = scratch("signer");
    let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    std::fs::create_dir_all(root.join("Signed")).unwrap();
    std::fs::copy(format!(r"{windir}\System32\notepad.exe"), root.join(r"Signed\notepad.exe")).unwrap();
    std::fs::create_dir_all(root.join("Unsigned")).unwrap();
    std::fs::copy(std::env::current_exe().unwrap(), root.join(r"Unsigned\tool.exe")).unwrap();
    let app = |dir: &str| AppInstance::new(dir.into(), AppSource::Unknown, raw(&root.join(dir).to_string_lossy()));
    assert!(installed_signer(&app("Signed")).is_some_and(|s| s.starts_with("Microsoft")));
    assert_eq!(installed_signer(&app("Unsigned")), None);
    assert_eq!(installed_signer(&AppInstance::new("x".into(), AppSource::Unknown, None)), None);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn downloaded_packages_exclude_manifests() {
    let root = scratch("download");
    write(&root.join("Example_1.3.0.exe"), "MZ");
    write(&root.join("Example_1.3.0.yaml"), "PackageIdentifier: Example");
    let t = UpdateTarget {
        app_instance_id: "app_x".into(),
        name: "Example".into(),
        channel: Channel::Winget,
        package_id: "Example.Example".into(),
        from_version: None,
        to_version: "1.3.0".into(),
        installed_signer: None,
        install_location: None,
        official_url: None,
    };
    let files = WindowsUpdates.downloaded(&t, &root.to_string_lossy());
    assert_eq!(files.len(), 1);
    assert!(files[0].ends_with("Example_1.3.0.exe"));
    assert_eq!(WindowsUpdates.running(&t), None, "no location: running state unknown, never assumed closed");
    let _ = std::fs::remove_dir_all(&root);
}
