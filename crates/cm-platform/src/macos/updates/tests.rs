//! Fixture bundles, Caskroom/Cellar folders, appcast samples and stand-in `brew` scripts only: the real Homebrew is
//! only looked at (its ownership), never run, and nothing touches the network.

use std::collections::BTreeMap;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use cm_core::apps::{AppInstance, AppSource, HomebrewKind};
use cm_core::updates::mac::{APPCAST_MAX, AppcastItem, parse_brew_outdated};

use super::*;
use crate::macos::ffi::cf::PlistFormat;
use crate::macos::testfs::Fixture;

const MODERN: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" xmlns:dc="http://purl.org/dc/elements/1.1/">
  <channel>
    <title>Sparkly Changelog</title>
    <link>https://updates.example.test/sparkly/appcast.xml</link>
    <item>
      <title>Version 1.2</title>
      <pubDate>Mon, 05 Oct 2026 10:00:00 +0000</pubDate>
      <sparkle:version>120</sparkle:version>
      <sparkle:shortVersionString>1.2</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>13.0</sparkle:minimumSystemVersion>
      <enclosure url="https://updates.example.test/sparkly/Sparkly-1.2.zip" length="1234" type="application/octet-stream"
                 sparkle:edSignature="c2lnbmF0dXJl"/>
    </item>
    <item>
      <title>Version 2.0 beta</title>
      <sparkle:version>200</sparkle:version>
      <sparkle:shortVersionString>2.0b1</sparkle:shortVersionString>
      <sparkle:channel>beta</sparkle:channel>
      <enclosure url="https://updates.example.test/sparkly/Sparkly-2.0b1.zip" length="1" type="application/octet-stream"/>
    </item>
    <item>
      <title>Windows build</title>
      <enclosure url="https://updates.example.test/sparkly/setup.exe" sparkle:os="windows" sparkle:version="300" sparkle:shortVersionString="3.0" length="1" type="application/octet-stream"/>
    </item>
  </channel>
</rss>
"#;

/// Sparkle 1 style: versions only as `<enclosure>` attributes.
const LEGACY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>Old App</title>
    <item>
      <title>Old App 4.5.1</title>
      <enclosure url="https://example.test/old-4.5.1.dmg" sparkle:version="4501" sparkle:shortVersionString="4.5.1" length="9" type="application/x-apple-diskimage"/>
    </item>
  </channel>
</rss>
"#;

fn info(pairs: &[(&str, &str)]) -> Vec<u8> {
    let map: BTreeMap<String, Plist> = pairs.iter().map(|(k, v)| ((*k).to_owned(), Plist::String((*v).to_owned()))).collect();
    cf::plist_to_bytes(&Plist::Dict(map), PlistFormat::Xml).unwrap()
}

fn env(fx: &Fixture) -> AppsEnv {
    AppsEnv { root: Some(fx.mkdir("root")), home: Some(fx.mkdir("home")) }
}

fn app_at(path: &Path, source: AppSource) -> AppInstance {
    AppInstance::new("X".into(), source, RawPath::unix(path.as_os_str().as_bytes().to_vec()).ok())
}

fn script(fx: &Fixture, rel: &str, body: &str, mode: u32) -> PathBuf {
    let p = fx.write(rel, body.as_bytes());
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
    p
}

#[test]
fn appcast_parses_with_nsxmldocument() {
    let items = parse_appcast(MODERN.as_bytes()).unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(
        items[0],
        AppcastItem {
            title: Some("Version 1.2".into()),
            version: Some("120".into()),
            short_version: Some("1.2".into()),
            minimum_system_version: Some("13.0".into()),
            maximum_system_version: None,
            channel: None,
            os: None,
            ed_signature: true,
        }
    );
    assert_eq!((items[1].channel.as_deref(), items[1].ed_signature), (Some("beta"), false));
    assert_eq!((items[2].os.as_deref(), items[2].version.as_deref(), items[2].short_version.as_deref()), (Some("windows"), Some("300"), Some("3.0")));
    let legacy = parse_appcast(LEGACY.as_bytes()).unwrap();
    assert_eq!((legacy[0].version.as_deref(), legacy[0].short_version.as_deref()), (Some("4501"), Some("4.5.1")));
    // A feed with a channel but no items is an empty answer, not an error.
    assert_eq!(parse_appcast(br#"<rss version="2.0"><channel><title>t</title></channel></rss>"#).unwrap(), vec![]);
}

#[test]
fn malformed_appcasts_are_errors() {
    for (bad, why) in [
        (&b""[..], "well-formed"),
        (&b"<rss><channel><item></channel></rss>"[..], "well-formed"),
        (&b"this is not xml"[..], "well-formed"),
        (&b"<feed xmlns=\"http://www.w3.org/2005/Atom\"></feed>"[..], "root element <feed>"),
        (&b"<rss version=\"2.0\"></rss>"[..], "no <channel>"),
    ] {
        match parse_appcast(bad) {
            Err(e) => assert!(e.contains(why), "{e}"),
            Ok(v) => panic!("accepted {:?}: {v:?}", String::from_utf8_lossy(bad)),
        }
    }
    assert!(parse_appcast(&vec![b' '; (APPCAST_MAX + 1) as usize]).unwrap_err().contains("larger"));
}

/// External entities are never loaded: a feed cannot make the parser read local files.
#[test]
fn appcast_external_entities_are_not_loaded() {
    let fx = Fixture::new("upd-xxe");
    let secret = fx.write("secret.txt", b"cm-secret-4711");
    let xml = format!(
        r#"<?xml version="1.0"?><!DOCTYPE rss [<!ENTITY leak SYSTEM "file://{}">]>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"><channel><item><title>&leak;</title><sparkle:version>1</sparkle:version></item></channel></rss>"#,
        secret.display()
    );
    match parse_appcast(xml.as_bytes()) {
        Ok(items) => assert!(items.iter().all(|i| !i.title.as_deref().unwrap_or("").contains("cm-secret")), "{items:?}"),
        Err(e) => assert!(!e.contains("cm-secret"), "{e}"),
    }
}

#[test]
fn probe_reads_sparkle_electron_and_cask_flags() {
    let fx = Fixture::new("upd-probe");
    let e = env(&fx);
    let root = e.root.clone().unwrap();
    let sparkly = root.join("Applications/Sparkly.app");
    fx.write(
        sparkly.join("Contents/Info.plist").strip_prefix(fx.path()).unwrap(),
        &info(&[
            ("CFBundleIdentifier", "com.example.sparkly"),
            ("CFBundleVersion", "100"),
            ("CFBundleShortVersionString", "1.0"),
            ("SUFeedURL", "https://updates.example.test/sparkly/appcast.xml"),
            ("SUPublicEDKey", "pfIShU4dEXqPd5ObYNfDBiQWcXozk7estwzTnF9BamQ="),
        ]),
    );
    let p = probe(&app_at(&sparkly, AppSource::Unknown), &e);
    assert_eq!(
        p.sparkle,
        Some(SparkleFeed {
            feed_url: Some("https://updates.example.test/sparkly/appcast.xml".into()),
            public_ed_key: true,
            bundle_version: Some("100".into()),
            short_version: Some("1.0".into())
        })
    );
    assert_eq!((p.electron, p.cask_self_updating), (None, false));

    // Sparkle.framework without SUFeedURL: Sparkle, feed unknown.
    let fw = root.join("Applications/Runtime Feed.app");
    fx.write(fw.join("Contents/Info.plist").strip_prefix(fx.path()).unwrap(), &info(&[("CFBundleVersion", "5")]));
    fx.mkdir(fw.join("Contents/Frameworks/Sparkle.framework").strip_prefix(fx.path()).unwrap());
    let p = probe(&app_at(&fw, AppSource::Unknown), &e);
    assert_eq!(p.sparkle.map(|s| (s.feed_url, s.public_ed_key)), Some((None, false)));

    // Electron: electron-updater (with provider) and Squirrel.Mac.
    let chat = root.join("Applications/Chatty.app");
    fx.write(chat.join("Contents/Info.plist").strip_prefix(fx.path()).unwrap(), &info(&[("CFBundleVersion", "1")]));
    fx.mkdir(chat.join("Contents/Frameworks/Electron Framework.framework").strip_prefix(fx.path()).unwrap());
    fx.write(chat.join("Contents/Resources/app-update.yml").strip_prefix(fx.path()).unwrap(), b"owner: example\nrepo: chatty\nprovider: github\n");
    assert_eq!(probe(&app_at(&chat, AppSource::Unknown), &e).electron.as_deref(), Some("electron-updater (Contents/Resources/app-update.yml, provider github)"));
    let sq = root.join("Applications/Squirrely.app");
    fx.mkdir(sq.join("Contents/Frameworks/Electron Framework.framework").strip_prefix(fx.path()).unwrap());
    fx.mkdir(sq.join("Contents/Frameworks/Squirrel.framework").strip_prefix(fx.path()).unwrap());
    assert!(probe(&app_at(&sq, AppSource::Unknown), &e).electron.unwrap().starts_with("Squirrel.Mac"));
    let plain = root.join("Applications/Plain.app");
    fx.write(plain.join("Contents/Info.plist").strip_prefix(fx.path()).unwrap(), &info(&[("CFBundleVersion", "1")]));
    assert_eq!(probe(&app_at(&plain, AppSource::Unknown), &e), MacProbe::default());

    // Cask metadata: auto_updates / version :latest.
    let meta = |token: &str, json: &str| fx.write(format!("root/opt/homebrew/Caskroom/{token}/.metadata/1.0/20260101000000.000/Casks/{token}.json"), json.as_bytes());
    meta("selfish", r#"{"token":"selfish","version":"1.0","auto_updates":true,"artifacts":[{"app":["Selfish.app"]}]}"#);
    meta("nightly", r#"{"token":"nightly","version":"latest","auto_updates":null}"#);
    meta("firefox", r#"{"token":"firefox","version":"130.0","auto_updates":null}"#);
    let cask = |t: &str| AppSource::Homebrew { token: t.into(), kind: HomebrewKind::Cask };
    assert!(probe(&app_at(&plain, cask("selfish")), &e).cask_self_updating);
    assert!(probe(&app_at(&plain, cask("nightly")), &e).cask_self_updating);
    assert!(!probe(&app_at(&plain, cask("firefox")), &e).cask_self_updating);
    assert!(!probe(&app_at(&plain, cask("../etc")), &e).cask_self_updating);
}

#[test]
fn formulae_installed_on_request_are_listed() {
    let fx = Fixture::new("upd-cellar");
    let e = env(&fx);
    fx.write("root/opt/homebrew/Cellar/wget/1.24.5/INSTALL_RECEIPT.json", br#"{"installed_on_request":true,"installed_as_dependency":false}"#);
    fx.write("root/opt/homebrew/Cellar/wget/1.21.0/INSTALL_RECEIPT.json", br#"{"installed_on_request":true}"#);
    fx.write("root/opt/homebrew/Cellar/libidn2/2.3.7/INSTALL_RECEIPT.json", br#"{"installed_on_request":false,"installed_as_dependency":true}"#);
    fx.write("root/opt/homebrew/Cellar/python@3.12/3.12.7_1/INSTALL_RECEIPT.json", br#"{"installed_on_request":true}"#);
    fx.write("root/opt/homebrew/Cellar/python@3.12/3.12.8/INSTALL_RECEIPT.json", br#"{"installed_on_request":true}"#);
    fx.mkdir("root/opt/homebrew/opt");
    std::os::unix::fs::symlink("../Cellar/python@3.12/3.12.7_1", e.root.as_ref().unwrap().join("opt/homebrew/opt/python@3.12")).unwrap();
    let mut list = formula_instances(&e);
    list.sort_by(|a, b| a.name.cmp(&b.name));
    let got: Vec<(&str, Option<&str>)> = list.iter().map(|a| (a.name.as_str(), a.version.as_deref())).collect();
    assert_eq!(got, [("python@3.12", Some("3.12.7_1")), ("wget", Some("1.24.5"))], "linked keg wins, else the newest; dependencies are left out");
    assert!(matches!(&list[1].source, AppSource::Homebrew { token, kind: HomebrewKind::Formula } if token == "wget"));
}

#[test]
fn user_owned_tool_policy() {
    let fx = Fixture::new("upd-trust");
    let ok = script(&fx, "bin/brew", "#!/bin/sh\nexit 0\n", 0o755);
    // SAFETY: no preconditions.
    let uid = unsafe { libc::geteuid() };
    assert_eq!(super::super::cmd::trusted_user_program(&ok, uid).unwrap(), ok);
    assert_eq!(brew_trusted(&ok).unwrap(), ok);
    assert!(super::super::cmd::trusted_user_program(Path::new("/usr/bin/true"), uid).is_ok(), "root-owned tools stay trusted");
    // Owned by another (non-root) user: refused.
    assert!(super::super::cmd::trusted_user_program(&ok, uid + 1).unwrap_err().to_string().contains("not root or the current user"));
    // World-writable file, or group-writable for an ordinary group (staff): refused.
    let ww = script(&fx, "ww/brew", "#!/bin/sh\n", 0o757);
    assert!(brew_trusted(&ww).unwrap_err().contains("writable by others"));
    let gw = script(&fx, "gw/brew", "#!/bin/sh\n", 0o775);
    // SAFETY: valid NUL-terminated path; staff is gid 20.
    let c = std::ffi::CString::new(gw.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::chown(c.as_ptr(), u32::MAX, 20) }, 0);
    assert!(brew_trusted(&gw).unwrap_err().contains("writable by others"));
    // A world-writable directory above the tool: refused.
    let open_dir = fx.mkdir("open");
    let inner = script(&fx, "open/bin/brew", "#!/bin/sh\n", 0o755);
    std::fs::set_permissions(&open_dir, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(brew_trusted(&inner).unwrap_err().contains("writable by others"));
    std::fs::set_permissions(&open_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    // A linked brew (Intel layout) is refused: it would run with the wrong prefix.
    let link = fx.path().join("brew-link");
    std::os::unix::fs::symlink(&ok, &link).unwrap();
    assert!(brew_trusted(&link).unwrap_err().contains("symbolic link"));
    // Not executable, not a file.
    let plain = script(&fx, "plain/brew", "x", 0o644);
    assert!(brew_trusted(&plain).is_err());
    assert!(brew_trusted(&fx.mkdir("dir.brew")).is_err());
    assert!(brew_trusted(Path::new("relative/brew")).is_err());
}

/// The hosted runner's real Homebrew (if any) satisfies the user-owned policy; it is only inspected, never run.
#[test]
fn real_homebrew_layout_is_accepted_by_the_policy() {
    let Some(brew) = BREW_PATHS.iter().map(Path::new).find(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_file())) else {
        eprintln!("no unlinked Homebrew brew on this machine; skipped");
        return;
    };
    let owner = std::fs::metadata(brew).unwrap().uid();
    // SAFETY: no preconditions.
    if owner != 0 && owner != unsafe { libc::geteuid() } {
        eprintln!("{} belongs to uid {owner}, not this user; skipped", brew.display());
        return;
    }
    assert!(brew_trusted(brew).is_ok(), "{}: {:?}", brew.display(), brew_trusted(brew));
}

const FAKE_OUTDATED: &str = r#"{"formulae":[{"name":"wget","installed_versions":["1.24.5"],"current_version":"1.25.0","pinned":false,"pinned_version":null}],"casks":[{"name":"firefox","installed_versions":["130.0"],"current_version":"131.0.2"}]}"#;

fn fake_brew(fx: &Fixture, rel: &str) -> PathBuf {
    let body = format!(
        "#!/bin/sh\n\
         [ \"$HOMEBREW_NO_AUTO_UPDATE\" = 1 ] && [ \"$HOMEBREW_NO_ANALYTICS\" = 1 ] && [ \"$HOMEBREW_NO_ENV_HINTS\" = 1 ] || {{ echo 'homebrew env missing' >&2; exit 7; }}\n\
         [ -n \"$HOME\" ] || {{ echo 'HOME missing' >&2; exit 7; }}\n\
         [ \"$*\" = 'outdated --json=v2' ] || {{ echo \"unexpected argv: $*\" >&2; exit 8; }}\n\
         printf '%s\\n' '{FAKE_OUTDATED}'\n"
    );
    script(fx, rel, &body, 0o755)
}

#[test]
fn brew_outdated_runs_with_the_fixed_environment_and_argv() {
    let fx = Fixture::new("upd-brew");
    let e = env(&fx);
    let brew = fake_brew(&fx, "bin/brew");
    let text = brew_outdated(&brew, &e).unwrap();
    let rows = parse_brew_outdated(&text).unwrap();
    assert_eq!(rows.len(), 2);
    let failing = script(&fx, "bad/brew", "#!/bin/sh\necho 'Error: no such command' >&2\nexit 2\n", 0o755);
    assert!(brew_outdated(&failing, &e).unwrap_err().contains("no such command"));
    let silent = script(&fx, "silent/brew", "#!/bin/sh\nexit 0\n", 0o755);
    assert!(brew_outdated(&silent, &e).is_err(), "no output is not an answer");
    let shared = script(&fx, "shared/brew", "#!/bin/sh\n", 0o757);
    assert!(brew_outdated(&shared, &e).unwrap_err().contains("writable by others"));
    // Redirected: only the stand-in variable names a brew, never the real one.
    if std::env::var_os(TEST_BREW_VAR).is_none() {
        assert_eq!(brew_exe(&e), None);
    }
}

#[test]
fn backend_reads_versions_and_running_state() {
    let fx = Fixture::new("upd-backend");
    let e = env(&fx);
    fx.mkdir("root/opt/homebrew/Caskroom/firefox/130.0");
    fx.mkdir("root/opt/homebrew/Caskroom/firefox/.metadata");
    fx.mkdir("root/opt/homebrew/Cellar/wget/1.25.0");
    let b = MacUpdates { env: e };
    let mut t = UpdateTarget {
        app_instance_id: "app_x".into(),
        name: "Firefox".into(),
        channel: Channel::Homebrew,
        package_id: "cask:firefox".into(),
        from_version: Some("130.0".into()),
        to_version: "131.0.2".into(),
        installed_signer: None,
        install_location: Some("/Applications/Firefox.app".into()),
        official_url: None,
    };
    assert_eq!(b.installed_version(&t).as_deref(), Some("130.0"));
    fx.mkdir("root/opt/homebrew/Caskroom/firefox/131.0.2");
    assert_eq!(b.installed_version(&t).as_deref(), Some("131.0.2"));
    t.package_id = "formula:wget".into();
    t.to_version = "1.25.0".into();
    assert_eq!(b.installed_version(&t).as_deref(), Some("1.25.0"));
    t.package_id = "wget".into();
    assert_eq!(b.installed_version(&t), None);
    // This test binary runs from its own directory: "running" there, not in an unrelated folder.
    let exe_dir = std::env::current_exe().unwrap().parent().unwrap().to_string_lossy().into_owned();
    t.install_location = Some(exe_dir);
    assert_eq!(b.running(&t), Some(true));
    t.install_location = Some(fx.path().join("Nothing.app").to_string_lossy().into_owned());
    assert_eq!(b.running(&t), Some(false));
    t.install_location = None;
    assert_eq!(b.running(&t), None);
    assert!(b.downloaded(&t, "/tmp").is_empty());
    let sha = b.file_sha256("/usr/bin/true").unwrap();
    assert_eq!(sha.len(), 64);
}
