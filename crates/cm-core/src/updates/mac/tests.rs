//! macOS channel logic on captured tool output and synthetic probes: no tool runs, no network.

use std::cell::Cell;
use std::cmp::Ordering;

use super::*;
use crate::apps::{AppInstance, AppSource, HomebrewKind, ProtectedReason};
use crate::path::RawPath;
use crate::updates::{Route, UpdateState, upgrade_argv};

const AT: &str = "2026-10-17T08:00:00Z";

/// `brew outdated --json=v2` as Homebrew 4.x prints it (core formula, tap formula pinned, cask).
const BREW_V2: &str = r#"{
  "formulae": [
    {
      "name": "wget",
      "installed_versions": [
        "1.24.5"
      ],
      "current_version": "1.25.0",
      "pinned": false,
      "pinned_version": null
    },
    {
      "name": "hashicorp/tap/terraform",
      "installed_versions": [
        "1.9.0"
      ],
      "current_version": "1.9.8",
      "pinned": true,
      "pinned_version": "1.9.0"
    }
  ],
  "casks": [
    {
      "name": "firefox",
      "installed_versions": [
        "130.0"
      ],
      "current_version": "131.0.2"
    }
  ]
}
"#;

fn app(name: &str, source: AppSource) -> AppInstance {
    let mut a = AppInstance::new(name.into(), source, RawPath::unix(format!("/Applications/{name}.app").into_bytes()).ok());
    a.version = Some("1.0".into());
    a
}

fn bundle(name: &str) -> AppInstance {
    app(name, AppSource::AppBundle { bundle_id: format!("com.example.{}", name.to_lowercase()), path: format!("/Applications/{name}.app") })
}

fn cask(name: &str, token: &str) -> AppInstance {
    app(name, AppSource::Homebrew { token: token.into(), kind: HomebrewKind::Cask })
}

fn item(version: &str, short: &str) -> AppcastItem {
    AppcastItem { version: Some(version.into()), short_version: Some(short.into()), ed_signature: true, ..AppcastItem::default() }
}

fn feed(url: &str, bundle_version: &str) -> SparkleFeed {
    SparkleFeed { feed_url: Some(url.into()), public_ed_key: true, bundle_version: Some(bundle_version.into()), short_version: Some("1.0".into()) }
}

#[test]
fn brew_outdated_v2_parses_formulae_and_casks() {
    let rows = parse_brew_outdated(BREW_V2).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows[0],
        BrewOutdated { name: "wget".into(), kind: HomebrewKind::Formula, installed_versions: vec!["1.24.5".into()], current_version: "1.25.0".into(), pinned: false }
    );
    assert!(rows[1].pinned && rows[1].name == "hashicorp/tap/terraform");
    assert_eq!((rows[2].kind, rows[2].name.as_str(), rows[2].current_version.as_str()), (HomebrewKind::Cask, "firefox", "131.0.2"));
    // Nothing outdated, and a cask entry with a string `installed_versions` (older Homebrew).
    assert_eq!(parse_brew_outdated(r#"{"formulae":[],"casks":[]}"#).unwrap(), vec![]);
    let old = parse_brew_outdated(r#"{"casks":[{"name":"iterm2","installed_versions":"3.4","current_version":"3.5"}]}"#).unwrap();
    assert_eq!(old[0].installed_versions, ["3.4"]);
}

#[test]
fn malformed_brew_output_is_an_error_not_an_empty_answer() {
    for bad in [
        "",
        "Error: Unknown command: outdated",
        "[]",
        r#"{"formulae": {}}"#,
        r#"{"something": []}"#,
        r#"{"formulae":[{"installed_versions":["1"],"current_version":"2"}]}"#,
        r#"{"casks":[{"name":"--force","current_version":"2"}]}"#,
        r#"{"casks":[{"name":"../evil","current_version":"2"}]}"#,
        r#"{"casks":[{"name":"firefox","current_version":""}]}"#,
        r#"{"formulae":[{"name":"wget","installed_versions":["1"],"current_version":"2"}],"casks":[{"name":"x y","current_version":"1"}]}"#,
    ] {
        assert!(parse_brew_outdated(bad).is_err(), "accepted: {bad}");
    }
}

#[test]
fn homebrew_names_and_package_ids_are_argument_safe() {
    for ok in ["wget", "python@3.12", "hashicorp/tap/terraform", "visual-studio-code", "c++utilities", "font_x"] {
        assert!(valid_brew_name(ok), "{ok}");
    }
    for bad in ["", "-v", "--cask", "/etc/passwd", "a/../b", "a b", "a;b", "tap/", ".hidden", "a//b", &"x".repeat(201)] {
        assert!(!valid_brew_name(bad), "{bad}");
    }
    assert_eq!(parse_homebrew_package_id("cask:firefox"), Some((HomebrewKind::Cask, "firefox")));
    assert_eq!(parse_homebrew_package_id(&homebrew_package_id(HomebrewKind::Formula, "user/tap/x")), Some((HomebrewKind::Formula, "user/tap/x")));
    assert_eq!(parse_homebrew_package_id("firefox"), None);
    assert_eq!(parse_homebrew_package_id("bottle:firefox"), None);
    assert_eq!(parse_homebrew_package_id("cask:--force"), None);
}

#[test]
fn homebrew_upgrade_argv_is_fixed_per_kind() {
    let target = |id: &str| crate::updates::UpdateTarget {
        app_instance_id: "app_x".into(),
        name: "Firefox".into(),
        channel: Channel::Homebrew,
        package_id: id.into(),
        from_version: Some("130.0".into()),
        to_version: "131.0.2".into(),
        installed_signer: None,
        install_location: Some("/Applications/Firefox.app".into()),
        official_url: None,
    };
    let (id, argv) = upgrade_argv(&target("cask:firefox")).unwrap();
    assert_eq!((id, argv), (crate::updates::HOMEBREW_ADAPTER, vec!["upgrade".to_owned(), "--cask".into(), "firefox".into()]));
    assert_eq!(upgrade_argv(&target("formula:wget")).unwrap().1, ["upgrade", "--formula", "wget"]);
    assert_eq!(upgrade_argv(&target("wget")), None, "a package ID without its kind is never executed");
    assert_eq!(upgrade_argv(&target("cask:-rf")), None);
    let mut status_only = target("cask:firefox");
    for c in [Channel::Sparkle, Channel::MacAppStore, Channel::Electron] {
        status_only.channel = c;
        assert_eq!(upgrade_argv(&status_only), None, "{c:?} is status only");
    }
}

#[test]
fn version_order_follows_sparkle() {
    use Ordering::*;
    for (a, b, o) in [
        ("1.0", "1.0", Equal),
        ("1.0", "1.1", Less),
        ("1.10", "1.9", Greater),
        ("1.0", "1.0b1", Greater),
        ("1.0b1", "1.0b2", Less),
        ("1.5.1", "1.5", Greater),
        ("1.5", "1.5b3", Greater),
        ("120", "100", Greater),
        ("2.0 (4501)", "2.0 (4500)", Greater),
        ("1.0.0", "1.0", Greater),
        ("001.2", "1.2", Equal),
        ("99999999999999999999", "99999999999999999998", Greater),
        ("6.2.1", "6.10", Less),
    ] {
        assert_eq!(compare_versions(a, b), o, "{a} vs {b}");
        assert_eq!(compare_versions(b, a), o.reverse(), "{b} vs {a}");
    }
}

#[test]
fn sparkle_compares_bundle_versions_and_shows_the_short_version() {
    let a = bundle("Sparkly");
    let f = feed("https://updates.example.test/sparkly/appcast.xml", "100");
    let items = vec![item("100", "1.0"), item("120", "1.2"), item("110", "1.1")];
    let c = sparkle_check(&a, &f, Some(Ok(items.clone())), Some("15.6"), AT);
    assert_eq!((c.state, c.available_version.as_deref(), c.channel, c.route), (UpdateState::UpdateAvailable, Some("1.2"), Some(Channel::Sparkle), Route::OpenApp));
    assert_eq!(c.channel_id.as_deref(), Some("https://updates.example.test/sparkly/appcast.xml"));
    let why = c.reason.unwrap();
    assert!(why.contains("1.2 (120)") && why.contains("never downloads") && why.contains("SUPublicEDKey"), "{why}");
    // Same version: up to date, and only because the feed answered.
    let c = sparkle_check(&a, &feed("https://updates.example.test/a.xml", "120"), Some(Ok(items)), None, AT);
    assert_eq!((c.state, c.available_version, c.checked_at.as_deref()), (UpdateState::UpToDate, None, Some(AT)));
}

#[test]
fn sparkle_skips_other_channels_systems_and_os_requirements() {
    let a = bundle("Sparkly");
    let f = feed("https://u.example.test/a.xml", "100");
    let beta = AppcastItem { channel: Some("beta".into()), ..item("200", "2.0b1") };
    let windows = AppcastItem { os: Some("windows".into()), ..item("300", "3.0") };
    let needs_new_os = AppcastItem { minimum_system_version: Some("26.0".into()), ..item("150", "1.5") };
    let fine = AppcastItem { os: Some("macos".into()), minimum_system_version: Some("13.0".into()), ..item("110", "1.1") };
    let c = sparkle_check(&a, &f, Some(Ok(vec![beta.clone(), windows.clone(), needs_new_os.clone(), fine])), Some("15.6.1"), AT);
    assert_eq!((c.state, c.available_version.as_deref()), (UpdateState::UpdateAvailable, Some("1.1")));
    // Only a newer release that needs a newer macOS: up to date for this Mac, and the reason says why.
    let c = sparkle_check(&a, &f, Some(Ok(vec![beta, windows, needs_new_os])), Some("15.6.1"), AT);
    assert_eq!(c.state, UpdateState::UpToDate);
    assert!(c.reason.as_deref().unwrap().contains("needs macOS 26.0"), "{:?}", c.reason);
    // Unknown OS version: limits are not applied.
    let c = sparkle_check(&a, &f, Some(Ok(vec![AppcastItem { minimum_system_version: Some("26.0".into()), ..item("150", "1.5") }])), None, AT);
    assert_eq!(c.state, UpdateState::UpdateAvailable);
}

#[test]
fn sparkle_without_an_answer_is_never_up_to_date() {
    let a = bundle("Sparkly");
    let https = feed("https://u.example.test/a.xml", "100");
    type Fetched = Option<Result<Vec<AppcastItem>, String>>;
    let cases: Vec<(SparkleFeed, Fetched, &str)> = vec![
        (SparkleFeed { feed_url: None, ..https.clone() }, None, "no SUFeedURL"),
        (feed("http://u.example.test/a.xml", "100"), None, "not an https"),
        (https.clone(), None, "not fetched"),
        (https.clone(), Some(Err("connection refused".into())), "connection refused"),
        (https.clone(), Some(Ok(vec![])), "no macOS release"),
        (https.clone(), Some(Ok(vec![AppcastItem { title: Some("Notes only".into()), ..AppcastItem::default() }])), "no macOS release"),
        (SparkleFeed { bundle_version: None, short_version: None, ..https }, Some(Ok(vec![item("1", "1")])), "no CFBundleVersion"),
    ];
    for (f, fetched, why) in cases {
        let c = sparkle_check(&a, &f, fetched, None, AT);
        assert_eq!(c.state, UpdateState::CannotCheck, "{why}");
        assert_eq!((c.channel, c.route, c.checked_at.as_deref()), (Some(Channel::Sparkle), Route::OpenApp, Some(AT)));
        assert!(c.reason.as_deref().unwrap().contains(why), "{why}: {:?}", c.reason);
    }
    // No EdDSA key / no signature: reported, not hidden.
    let unsigned = SparkleFeed { public_ed_key: false, ..feed("https://u.example.test/a.xml", "1") };
    let c = sparkle_check(&a, &unsigned, Some(Ok(vec![AppcastItem { ed_signature: false, ..item("2", "2") }])), None, AT);
    let why = c.reason.unwrap();
    assert!(why.contains("no SUPublicEDKey") && why.contains("no sparkle:edSignature"), "{why}");
}

#[test]
fn homebrew_answers_per_package() {
    let rows = Ok(parse_brew_outdated(BREW_V2).unwrap());
    let ff = cask("Firefox", "firefox");
    let c = homebrew_check(&ff, HomebrewKind::Cask, "firefox", Some(&rows), AT);
    assert_eq!(
        (c.state, c.current_version.as_deref(), c.available_version.as_deref(), c.route, c.channel_id.as_deref()),
        (UpdateState::UpdateAvailable, Some("130.0"), Some("131.0.2"), Route::PackageManager, Some("cask:firefox"))
    );
    let ok = homebrew_check(&cask("Iterm", "iterm2"), HomebrewKind::Cask, "iterm2", Some(&rows), AT);
    assert_eq!((ok.state, ok.route), (UpdateState::UpToDate, Route::None));
    assert!(ok.reason.unwrap().contains("brew update"));
    // A formula and a cask of the same name are different packages.
    let wget_cask = homebrew_check(&cask("wget", "wget"), HomebrewKind::Cask, "wget", Some(&rows), AT);
    assert_eq!(wget_cask.state, UpdateState::UpToDate);
    // Tap formula matched by its short name; pinned: available but not offered as an upgrade.
    let tf = homebrew_check(&cask("terraform", "terraform"), HomebrewKind::Formula, "terraform", Some(&rows), AT);
    assert_eq!((tf.state, tf.route), (UpdateState::UpdateAvailable, Route::None));
    assert!(tf.reason.unwrap().contains("brew unpin terraform"));
    // brew failed or was not queried: cannot_check with the reason.
    let err = homebrew_check(&ff, HomebrewKind::Cask, "firefox", Some(&Err("brew refused: /opt/homebrew/bin is writable by others".into())), AT);
    assert_eq!(err.state, UpdateState::CannotCheck);
    assert!(err.reason.unwrap().contains("writable by others"));
    assert_eq!(homebrew_check(&ff, HomebrewKind::Cask, "firefox", None, AT).state, UpdateState::CannotCheck);
}

struct Probes {
    fetches: Cell<u32>,
}

fn probe_for(a: &AppInstance) -> MacProbe {
    match a.name.as_str() {
        "Sparkly" | "Selfish" => MacProbe { sparkle: Some(feed("https://u.example.test/a.xml", "100")), cask_self_updating: a.name == "Selfish", ..MacProbe::default() },
        "Insecure" => MacProbe { sparkle: Some(feed("http://u.example.test/a.xml", "100")), ..MacProbe::default() },
        "Chatty" => MacProbe { electron: Some("electron-updater (app-update.yml, provider github)".into()), ..MacProbe::default() },
        "Latest" => MacProbe { cask_self_updating: true, ..MacProbe::default() },
        _ => MacProbe::default(),
    }
}

/// UPD-01 scenario: an app with no channel is `cannot_check`, never "up to date"; every result has a time.
#[test]
fn mac_detect_routes_every_app_and_never_claims_latest_without_a_channel() {
    let mut system = bundle("Calculator");
    system.protected_class = Some(ProtectedReason::OsPackage);
    let apps = vec![
        bundle("Plain"),
        app("Pages", AppSource::AppStore { bundle_id: "com.apple.iWork.Pages".into(), path: "/Applications/Pages.app".into() }),
        cask("Firefox", "firefox"),
        bundle("Sparkly"),
        bundle("Insecure"),
        bundle("Chatty"),
        cask("Selfish", "selfish"),
        cask("Latest", "latest-thing"),
        system,
    ];
    let p = Probes { fetches: Cell::new(0) };
    let fetch = |url: &str| -> Result<Vec<AppcastItem>, String> {
        p.fetches.set(p.fetches.get() + 1);
        assert!(url.starts_with("https://"));
        Ok(vec![item("120", "1.2")])
    };
    let out = mac_detect(&MacDetectInput {
        apps: &apps,
        homebrew: Some(Ok(parse_brew_outdated(BREW_V2).unwrap())),
        probe: &probe_for,
        fetch_appcast: &fetch,
        os_version: Some("15.6"),
        checked_at: AT,
    });
    let by = |n: &str| out.iter().find(|c| c.name == n).unwrap();
    assert_eq!(out.len(), apps.len());
    assert!(out.iter().all(|c| c.checked_at.as_deref() == Some(AT)));

    let plain = by("Plain");
    assert_eq!((plain.state, plain.channel, plain.route), (UpdateState::CannotCheck, None, Route::None));
    assert_eq!(plain.reason.as_deref(), Some(MAC_NO_CHANNEL));

    let mas = by("Pages");
    assert_eq!((mas.state, mas.channel, mas.route), (UpdateState::CannotCheck, Some(Channel::MacAppStore), Route::OpenStore));
    assert_eq!(mas.official_url.as_deref(), Some(MAC_APP_STORE_UPDATES_URI));
    assert_eq!(crate::updates::store_updates_uri(mas.channel), MAC_APP_STORE_UPDATES_URI);
    assert_eq!(crate::updates::store_updates_uri(Some(Channel::MicrosoftStore)), crate::updates::STORE_UPDATES_URI);

    assert_eq!((by("Firefox").state, by("Firefox").channel), (UpdateState::UpdateAvailable, Some(Channel::Homebrew)));
    assert_eq!((by("Sparkly").state, by("Sparkly").channel), (UpdateState::UpdateAvailable, Some(Channel::Sparkle)));
    assert_eq!(by("Insecure").state, UpdateState::CannotCheck);
    let chatty = by("Chatty");
    assert_eq!((chatty.state, chatty.channel, chatty.route), (UpdateState::CannotCheck, Some(Channel::Electron), Route::OpenApp));
    assert!(chatty.reason.as_deref().unwrap().contains("electron-updater"));
    // An auto_updates cask falls through to its own Sparkle feed; without one it is cannot_check, not "not outdated".
    assert_eq!((by("Selfish").state, by("Selfish").channel), (UpdateState::UpdateAvailable, Some(Channel::Sparkle)));
    let latest = by("Latest");
    assert_eq!((latest.state, latest.channel, latest.route), (UpdateState::CannotCheck, Some(Channel::Homebrew), Route::OpenApp));
    let sys = by("Calculator");
    assert_eq!((sys.state, sys.official_url.as_deref()), (UpdateState::CannotCheck, Some(SOFTWARE_UPDATE_URI)));
    // Only the two https Sparkle feeds were fetched (not the http one, not anything else).
    assert_eq!(p.fetches.get(), 2);
    assert!(out.iter().filter(|c| c.state == UpdateState::UpToDate).all(|c| c.channel.is_some()));
}

#[test]
fn detection_only_channels_cannot_be_up_to_date() {
    let a = bundle("X");
    for ch in [Channel::Electron, Channel::MacAppStore, Channel::SelfUpdater] {
        let c = crate::updates::UpdateCheck::new(&a, Some(ch), None, None, UpdateState::UpToDate, None, Some(AT.into()), Route::None);
        assert_eq!(c.state, UpdateState::CannotCheck, "{ch:?}");
    }
}
