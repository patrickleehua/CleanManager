use std::cell::RefCell;

use super::*;
use crate::apps::{AppInstance, AppSource, RegistryHive, RegistryView};
use crate::path::RawPath;
use crate::plan::v2::{ExecutableRef, ToolAdapterAction};

fn pad(s: &str, w: usize) -> String {
    let used: usize = s.chars().map(char_width).sum();
    format!("{s}{}", " ".repeat(w.saturating_sub(used)))
}

fn row(cells: [&str; 5]) -> String {
    format!("{}{}{}{}{}", pad(cells[0], 30), pad(cells[1], 34), pad(cells[2], 16), pad(cells[3], 14), cells[4])
}

/// Synthetic `winget list` output in the zh-CN layout: spinner residue, agreement text, CJK names, footer.
fn fixture() -> String {
    [
        "   - \r   \\ \r".to_owned(),
        "“msstore”源要求在使用前查看以下协议。".to_owned(),
        String::new(),
        row(["名称", "ID", "版本", "可用", "源"]),
        "-".repeat(100),
        row(["Example Editor", "Example.Editor", "1.2.0", "1.3.0", "winget"]),
        row(["示例同步工具", "Example.Sync", "7.75.18", "", "winget"]),
        row(["Store Thing", "9NBLGGH4NNS1", "2.0", "2.1", "msstore"]),
        row(["Very Long Product Name That …", "Example.LongName", "3.0", "", "winget"]),
        row(["Unpackaged Tool", r"ARP\Machine\X64\UnpackagedTool", "0.9", "", ""]),
        row(["Photos", r"MSIX\Microsoft.Windows.Photos_2026.1.0.0_x64__8wekyb3d8bbwe", "2026.1.0.0", "", ""]),
        String::new(),
        "2 个升级可用。".to_owned(),
    ]
    .join("\r\n")
}

fn raw(p: &str) -> RawPath {
    RawPath::windows(p.encode_utf16().collect()).unwrap()
}

fn app(name: &str, version: &str, source: AppSource, location: Option<&str>) -> AppInstance {
    let mut a = AppInstance::new(name.into(), source, location.map(raw));
    a.version = Some(version.into());
    a
}

fn reg(key: &str) -> AppSource {
    AppSource::RegistryUninstall { hive: RegistryHive::Hkcu, view: RegistryView::Shared, key: key.into() }
}

#[test]
fn winget_table_parses_in_display_columns() {
    let rows = parse_winget_list(&fixture()).unwrap();
    assert_eq!(rows.len(), 6);
    assert_eq!((rows[0].id.as_str(), rows[0].available.as_deref(), rows[0].source.as_deref()), ("Example.Editor", Some("1.3.0"), Some("winget")));
    assert_eq!((rows[1].name.as_str(), rows[1].id.as_str(), rows[1].version.as_str(), rows[1].available.as_deref()), ("示例同步工具", "Example.Sync", "7.75.18", None));
    assert_eq!(rows[2].source.as_deref(), Some("msstore"));
    assert_eq!(rows[4].source, None);
    assert!(parse_winget_list("No installed package found matching input criteria.").is_err());
}

/// UPD-01: an app without a channel is "cannot check" with a reason, never "up to date".
#[test]
fn no_channel_is_cannot_check_never_latest() {
    let apps = vec![
        app("Example Editor", "1.2.0", reg("ExampleEditor"), None),
        app("示例同步工具", "7.75.18", reg("ExampleSync"), None),
        app("Unpackaged Tool", "0.9", reg("UnpackagedTool"), None),
        app("Orphan", "1.0", reg("Orphan"), None),
        app("Photos", "2026.1.0.0", AppSource::Msix { package_family: "Microsoft.Windows.Photos_8wekyb3d8bbwe".into(), full_name: "Microsoft.Windows.Photos_2026.1.0.0_x64__8wekyb3d8bbwe".into() }, None),
        app("Very Long Product Name That Goes On", "3.0", reg("Long"), None),
        app("Chatty", "5.0", reg("Chatty"), Some(r"C:\Users\A\AppData\Local\Chatty")),
    ];
    let squirrel = |a: &AppInstance| (a.name == "Chatty").then(|| SelfUpdaterProbe { evidence: "Update.exe and app-5.0".into() });
    let input = DetectInput { apps: &apps, winget: Some(parse_winget_list(&fixture())), scoop: &[], self_updater: &squirrel, checked_at: "2026-10-15T09:00:00Z" };
    let r = detect(&input);
    let by = |n: &str| r.iter().find(|c| c.name == n).unwrap();
    let editor = by("Example Editor");
    assert_eq!((editor.state, editor.channel, editor.available_version.as_deref(), editor.route), (UpdateState::UpdateAvailable, Some(Channel::Winget), Some("1.3.0"), Route::PackageManager));
    assert_eq!(editor.checked_at.as_deref(), Some("2026-10-15T09:00:00Z"));
    assert_eq!(editor.current_version.as_deref(), Some("1.2.0"));
    assert_eq!((by("示例同步工具").state, by("示例同步工具").route), (UpdateState::UpToDate, Route::None));
    assert_eq!(by("Very Long Product Name That Goes On").state, UpdateState::UpToDate, "ellipsis-truncated names still match");
    for n in ["Unpackaged Tool", "Orphan"] {
        let c = by(n);
        assert_eq!((c.state, c.channel), (UpdateState::CannotCheck, None), "{n}");
        assert_eq!(c.reason.as_deref(), Some(NO_CHANNEL));
    }
    let photos = by("Photos");
    assert_eq!((photos.state, photos.channel, photos.route), (UpdateState::CannotCheck, Some(Channel::MicrosoftStore), Route::OpenStore));
    let chatty = by("Chatty");
    assert_eq!((chatty.state, chatty.channel, chatty.route), (UpdateState::CannotCheck, Some(Channel::SelfUpdater), Route::OpenApp));
    assert!(chatty.reason.as_deref().unwrap().contains("Squirrel"));
    assert!(r.iter().all(|c| c.state != UpdateState::UpToDate || c.channel.is_some_and(|ch| ch != Channel::SelfUpdater)));
    // The constructor itself refuses "up to date" without a channel.
    let forced = UpdateCheck::new(&apps[3], None, None, None, UpdateState::UpToDate, None, Some("t".into()), Route::None);
    assert_eq!(forced.state, UpdateState::CannotCheck);
    // A failed winget query is a reason, not "latest".
    let failed = DetectInput { apps: &apps[..1], winget: Some(Err("winget is not installed".into())), scoop: &[], self_updater: &|_| None, checked_at: "t" };
    let f = detect(&failed);
    assert_eq!(f[0].state, UpdateState::CannotCheck);
    assert!(f[0].reason.as_deref().unwrap().contains("not installed"));
}

#[test]
fn scoop_compares_with_the_local_bucket() {
    let apps = vec![app("ripgrep", "?", AppSource::Unknown, Some(r"C:\Users\A\scoop\apps\ripgrep\current")), app("jq", "?", AppSource::Unknown, Some(r"C:\Users\A\scoop\apps\jq\current"))];
    let scoop = vec![
        ScoopApp {
            name: "ripgrep".into(),
            installed_version: "14.0.0".into(),
            bucket: Some("main".into()),
            bucket_version: Some("14.1.1".into()),
            bucket_updated_at: Some("2026-10-14T00:00:00Z".into()),
            homepage: Some("https://github.com/BurntSushi/ripgrep".into()),
            install_dir: r"C:\Users\A\scoop\apps\ripgrep".into(),
        },
        ScoopApp {
            name: "jq".into(),
            installed_version: "1.7".into(),
            bucket: Some("main".into()),
            bucket_version: Some("1.7".into()),
            bucket_updated_at: None,
            homepage: None,
            install_dir: r"C:\Users\A\scoop\apps\jq".into(),
        },
    ];
    let r = detect(&DetectInput { apps: &apps, winget: None, scoop: &scoop, self_updater: &|_| None, checked_at: "t" });
    assert_eq!((r[0].state, r[0].channel, r[0].available_version.as_deref(), r[0].current_version.as_deref()), (UpdateState::UpdateAvailable, Some(Channel::Scoop), Some("14.1.1"), Some("14.0.0")));
    assert!(r[0].reason.as_deref().unwrap().contains("local bucket copy from 2026-10-14"));
    assert_eq!(r[1].state, UpdateState::UpToDate);
    assert_eq!(scoop_manifest_version("\u{feff}{\"version\": \"2.1\", \"homepage\": \"https://x.example\"}").as_deref(), Some("2.1"));
    assert_eq!(scoop_manifest_homepage("{\"homepage\": \"file:///c:/x\"}"), None);
    assert_eq!(scoop_install_bucket("{\"bucket\": \"extras\"}").as_deref(), Some("extras"));
}

/// UPD-04: with checks off, the last results keep their time; apps never checked are "not checked".
#[test]
fn disabled_checks_show_cached_or_not_checked() {
    let apps = vec![app("A", "1", reg("A"), None), app("B", "1", reg("B"), None)];
    let cached = vec![UpdateCheck::new(&apps[0], Some(Channel::Winget), Some("A.A".into()), Some("2".into()), UpdateState::UpdateAvailable, None, Some("2026-10-01T00:00:00Z".into()), Route::PackageManager)];
    let r = when_disabled(&apps, &cached);
    assert_eq!((r[0].state, r[0].checked_at.as_deref()), (UpdateState::UpdateAvailable, Some("2026-10-01T00:00:00Z")));
    assert_eq!((r[1].state, r[1].checked_at.as_deref(), r[1].reason.as_deref()), (UpdateState::NotChecked, None, Some(CHECKS_OFF)));
}

#[test]
fn publisher_comparison_ignores_form_not_identity() {
    assert!(publisher_consistent("Microsoft Corporation", "microsoft corporation"));
    assert!(publisher_consistent("Example Software, Inc.", "Example Software Inc"));
    assert!(publisher_consistent("ByteDance Ltd.", "ByteDance Ltd"));
    assert!(!publisher_consistent("Example Software, Inc.", "Example Softwares Inc"));
    assert!(!publisher_consistent("Microsoft Corporation", "Evil Microsoft Corporation"));
    assert!(!publisher_consistent("", ""));
}

struct Fake {
    signer: Option<String>,
    running: Option<bool>,
    fail_upgrade: bool,
    calls: RefCell<Vec<Vec<String>>>,
    version_after: RefCell<Option<String>>,
}

impl Fake {
    fn new(signer: Option<&str>) -> Self {
        Fake { signer: signer.map(str::to_owned), running: Some(false), fail_upgrade: false, calls: RefCell::new(vec![]), version_after: RefCell::new(Some("1.2.0".into())) }
    }
    fn upgrades(&self) -> usize {
        self.calls.borrow().iter().filter(|a| a[0] == "upgrade" || a[0] == "update").count()
    }
}

impl UpdateBackend for Fake {
    fn run(&self, _exe: &str, argv: &[String], _t: u32) -> Result<ToolOutput, String> {
        self.calls.borrow_mut().push(argv.to_vec());
        if (argv[0] == "upgrade") && self.fail_upgrade {
            return Ok(ToolOutput { exit_code: Some(-1978335189), output: "Installer failed with exit code: 1603".into(), timed_out: false });
        }
        if argv[0] == "upgrade" {
            *self.version_after.borrow_mut() = Some("1.3.0".into());
        }
        Ok(ToolOutput { exit_code: Some(0), output: String::new(), timed_out: false })
    }
    fn file_sha256(&self, _p: &str) -> Option<String> {
        Some("e".repeat(64))
    }
    fn signer(&self, _p: &str) -> Option<String> {
        self.signer.clone()
    }
    fn downloaded(&self, _t: &UpdateTarget, dir: &str) -> Vec<String> {
        vec![format!("{dir}\\Example_1.3.0.exe")]
    }
    fn scratch_dir(&self, id: &str) -> Result<String, String> {
        Ok(format!("C:\\tmp\\{id}"))
    }
    fn running(&self, _t: &UpdateTarget) -> Option<bool> {
        self.running
    }
    fn installed_version(&self, _t: &UpdateTarget) -> Option<String> {
        self.version_after.borrow().clone()
    }
}

fn target() -> UpdateTarget {
    UpdateTarget {
        app_instance_id: "app_x".into(),
        name: "Example Editor".into(),
        channel: Channel::Winget,
        package_id: "Example.Editor".into(),
        from_version: Some("1.2.0".into()),
        to_version: "1.3.0".into(),
        installed_signer: Some("Example Software, Inc.".into()),
        install_location: Some(r"C:\Program Files\Example".into()),
        official_url: Some("https://example.com/download".into()),
    }
}

fn action(t: &UpdateTarget) -> ToolAdapterAction {
    let (id, argv) = upgrade_argv(t).unwrap();
    ToolAdapterAction {
        action_id: "a01".into(),
        adapter_id: id.into(),
        adapter_version: ADAPTER_VERSION.into(),
        executable: ExecutableRef { path: raw(r"C:\Program Files\WindowsApps\Microsoft.DesktopAppInstaller\winget.exe"), sha256: "e".repeat(64) },
        argv,
        preview_digest: t.digest(),
        timeout_seconds: 900,
    }
}

fn note(t: &UpdateTarget) -> String {
    serde_json::to_string(t).unwrap()
}

#[test]
fn verified_update_runs_download_then_pinned_upgrade() {
    let t = target();
    let f = Fake::new(Some("Example Software Inc"));
    assert_eq!(execute(&f, &action(&t), Some(&note(&t))), UpdateOutcome::Updated { to_version: "1.3.0".into() });
    let calls = f.calls.borrow();
    assert_eq!(calls[0][0], "download");
    assert!(calls[0].windows(2).any(|w| w == ["--version", "1.3.0"]));
    assert_eq!(calls[1], argv::winget_upgrade("Example.Editor", "1.3.0"));
}

/// UPD-02: a package signed by someone else is refused before anything is installed; the app is unchanged.
#[test]
fn signer_mismatch_is_refused_and_the_app_is_unchanged() {
    let t = target();
    let f = Fake::new(Some("Other Publisher LLC"));
    match execute(&f, &action(&t), Some(&note(&t))) {
        UpdateOutcome::Skipped { code, message } => {
            assert_eq!(code, "signature_mismatch");
            assert!(message.contains("Other Publisher LLC") && message.contains("not changed"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(f.upgrades(), 0, "the upgrade never ran");
    assert_eq!(f.installed_version(&t).as_deref(), Some("1.2.0"));
    // Unsigned package or unknown installed signer: also refused, pointing to the official page.
    let f = Fake::new(None);
    assert!(matches!(execute(&f, &action(&t), Some(&note(&t))), UpdateOutcome::Skipped { code: "unverifiable", message } if message.contains("https://example.com/download")));
    let mut unsigned_app = target();
    unsigned_app.installed_signer = None;
    let f = Fake::new(Some("Example Software, Inc."));
    assert!(matches!(execute(&f, &action(&unsigned_app), Some(&note(&unsigned_app))), UpdateOutcome::Skipped { code: "unverifiable", .. }));
    assert!(f.calls.borrow().is_empty(), "nothing is downloaded when the installed app cannot be verified");
}

/// UPD-03: a running app is not updated; the user is asked to close it.
#[test]
fn running_or_unknown_running_state_skips() {
    let t = target();
    let mut f = Fake::new(Some("Example Software, Inc."));
    f.running = Some(true);
    assert!(matches!(execute(&f, &action(&t), Some(&note(&t))), UpdateOutcome::Skipped { code: "app_running", .. }));
    f.running = None;
    assert!(matches!(execute(&f, &action(&t), Some(&note(&t))), UpdateOutcome::Skipped { code: "running_unknown", .. }));
    assert!(f.calls.borrow().is_empty());
}

#[test]
fn bound_target_argv_and_tool_are_checked() {
    let t = target();
    let f = Fake::new(Some("Example Software, Inc."));
    let mut other = t.clone();
    other.to_version = "9.9".into();
    assert!(matches!(execute(&f, &action(&t), Some(&note(&other))), UpdateOutcome::Skipped { code: "target_changed", .. }));
    let mut a = action(&t);
    a.argv.push("--force".into());
    assert!(matches!(execute(&f, &a, Some(&note(&t))), UpdateOutcome::Skipped { code: "adapter_mismatch", .. }));
    let mut a = action(&t);
    a.executable.sha256 = "0".repeat(64);
    assert!(matches!(execute(&f, &a, Some(&note(&t))), UpdateOutcome::Skipped { code: "tool_changed", .. }));
    assert!(matches!(execute(&f, &action(&t), None), UpdateOutcome::Skipped { code: "target_changed", .. }));
    let mut fail = Fake::new(Some("Example Software, Inc."));
    fail.fail_upgrade = true;
    assert!(matches!(execute(&fail, &action(&t), Some(&note(&t))), UpdateOutcome::Failed { code, message } if code == "tool_exit_-1978335189" && message.contains("1603")));
}

#[test]
fn macos_channels_have_their_wire_names() {
    let names = [Channel::Sparkle, Channel::MacAppStore, Channel::Homebrew, Channel::Electron].map(|c| serde_json::to_value(c).unwrap());
    assert_eq!(names, ["sparkle", "mac_app_store", "homebrew", "electron"].map(serde_json::Value::from));
}
