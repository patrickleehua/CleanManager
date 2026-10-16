//! Fixture bundles under a redirected root and home (never the real `/Applications` or Homebrew),
//! plus one read-only look at the real `/System/Applications`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cm_core::apps::{
    AppInstance, AppSource, EvidenceKind, HomebrewKind, InstallRecordStatus, Ownership, ProtectedReason, UnknownOwnership, assess_directory, associate,
};
use cm_core::capability::CapabilityState;
use cm_core::model::Timestamp;
use cm_core::path::RawPath;

use super::*;
use crate::macos::ffi::cf::PlistFormat;
use crate::macos::testfs::Fixture;

struct Env {
    fx: Fixture,
    env: AppsEnv,
}

impl Env {
    fn new(tag: &str) -> Env {
        let fx = Fixture::new(tag);
        let env = AppsEnv { root: Some(fx.mkdir("root")), home: Some(fx.mkdir("home")) };
        Env { fx, env }
    }

    fn root(&self) -> &Path {
        self.env.root.as_deref().unwrap()
    }

    fn home(&self) -> &Path {
        self.env.home.as_deref().unwrap()
    }

    fn inventory(&self) -> Vec<AppInstance> {
        let (apps, issues) = inventory(&self.env);
        assert!(issues.is_empty(), "{issues:?}");
        apps
    }
}

fn info(pairs: &[(&str, &str)], format: PlistFormat) -> Vec<u8> {
    let map: BTreeMap<String, Plist> = pairs.iter().map(|(k, v)| ((*k).to_owned(), Plist::String((*v).to_owned()))).collect();
    cf::plist_to_bytes(&Plist::Dict(map), format).unwrap()
}

/// A fake bundle at `bundle` (absolute) with the given `Contents/Info.plist` bytes.
fn bundle(bundle: &Path, plist: &[u8]) -> PathBuf {
    let contents = bundle.join("Contents");
    std::fs::create_dir_all(contents.join("MacOS")).unwrap();
    std::fs::write(contents.join("Info.plist"), plist).unwrap();
    bundle.to_path_buf()
}

fn at<'a>(apps: &'a [AppInstance], path: &Path) -> &'a AppInstance {
    let raw = RawPath::unix(path.as_os_str().as_bytes().to_vec()).unwrap();
    apps.iter().find(|a| a.install_location.as_ref() == Some(&raw)).unwrap_or_else(|| panic!("no instance at {}: {apps:#?}", path.display()))
}

fn has(app: &AppInstance, kind: EvidenceKind) -> bool {
    app.evidence.iter().any(|e| e.kind == kind)
}

fn detail(app: &AppInstance, kind: EvidenceKind) -> String {
    app.evidence.iter().find(|e| e.kind == kind).map(|e| e.detail.clone()).unwrap_or_else(|| panic!("no {kind:?} evidence: {app:#?}"))
}

#[test]
fn same_bundle_id_copies_stay_separate_instances() {
    let e = Env::new("apps-dup");
    let plist = info(&[("CFBundleIdentifier", "com.example.editor"), ("CFBundleName", "Editor"), ("CFBundleShortVersionString", "2.1")], PlistFormat::Xml);
    let a = bundle(&e.root().join("Applications/Editor.app"), &plist);
    let b = bundle(&e.home().join("Applications/Editor.app"), &plist);
    let c = bundle(&e.root().join("Applications/Old Versions/Editor.app"), &plist);
    let apps = e.inventory();
    assert_eq!(apps.len(), 3, "{apps:#?}");
    let (a, b, c) = (at(&apps, &a), at(&apps, &b), at(&apps, &c));
    let mut ids = vec![&a.instance_id, &b.instance_id, &c.instance_id];
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3, "APP-01: one instance per bundle path");
    for app in [a, b, c] {
        assert_eq!(app.name, "Editor");
        assert_eq!(app.version.as_deref(), Some("2.1"));
        assert!(matches!(&app.source, AppSource::AppBundle { bundle_id, .. } if bundle_id == "com.example.editor"), "{app:#?}");
        assert_eq!(detail(app, EvidenceKind::BundleId), "com.example.editor");
        let id = app.location_identity.as_ref().expect("bundle identity");
        assert!(id.file_id.starts_with("inode64:") && !id.volume_id.is_empty());
        assert_eq!(app.protected_class, None);
        assert_eq!(app.publisher, None, "an unsigned fixture has no publisher");
        assert!(has(app, EvidenceKind::TeamId));
        assert_eq!(app.uninstall_capability.state, CapabilityState::Unsupported);
        assert_eq!(app.uninstall_capability.reason.as_deref(), Some(MAC_READ_ONLY_REASON));
        assert_eq!(app.to_wire()["source"], "app_bundle");
    }
    assert_ne!(a.location_identity, b.location_identity);
}

#[test]
fn binary_info_plist_parses_and_names_fall_back_in_order() {
    let e = Env::new("apps-binary");
    let one = bundle(
        &e.root().join("Applications/One.app"),
        &info(
            &[("CFBundleIdentifier", "com.example.one"), ("CFBundleDisplayName", "One Display"), ("CFBundleName", "One"), ("CFBundleShortVersionString", "1.0"), ("CFBundleVersion", "100")],
            PlistFormat::Binary,
        ),
    );
    assert!(std::fs::read(one.join("Contents/Info.plist")).unwrap().starts_with(b"bplist00"));
    let two = bundle(&e.root().join("Applications/Utilities/Two.app"), &info(&[("CFBundleIdentifier", "com.example.two"), ("CFBundleVersion", "7")], PlistFormat::Binary));
    let apps = e.inventory();
    let (one, two) = (at(&apps, &one), at(&apps, &two));
    assert_eq!((one.name.as_str(), one.version.as_deref()), ("One Display", Some("1.0")));
    assert_eq!(detail(one, EvidenceKind::DisplayName), "One Display (CFBundleDisplayName)");
    assert_eq!((two.name.as_str(), two.version.as_deref()), ("Two", Some("7")), "file name and CFBundleVersion fall back");
    assert_eq!(detail(two, EvidenceKind::DisplayName), "Two (bundle file name)");
}

#[test]
fn unreadable_info_plist_is_evidence_and_nothing_is_invented() {
    let e = Env::new("apps-corrupt");
    let corrupt = bundle(&e.root().join("Applications/Broken.app"), b"<?xml version=\"1.0\"?><plist><dict><key>CFBundleIdentifier</key><string>com.x");
    let missing = e.fx.mkdir("root/Applications/Empty.app/Contents");
    let not_dict = bundle(&e.root().join("Applications/Array.app"), &cf::plist_to_bytes(&Plist::Array(vec![]), PlistFormat::Xml).unwrap());
    let apps = e.inventory();
    for (path, name) in [(corrupt, "Broken"), (missing.parent().unwrap().to_path_buf(), "Empty"), (not_dict, "Array")] {
        let app = at(&apps, &path);
        assert!(has(app, EvidenceKind::InfoPlistUnreadable), "{app:#?}");
        assert_eq!(app.name, name);
        assert_eq!(app.version, None);
        assert_eq!(app.source, AppSource::Unknown, "no bundle ID is invented");
        assert!(!has(app, EvidenceKind::BundleId));
        assert_eq!(app.to_wire()["source"], "unknown");
    }
}

#[test]
fn mas_receipt_makes_an_app_store_instance() {
    let e = Env::new("apps-mas");
    let p = bundle(&e.root().join("Applications/Store.app"), &info(&[("CFBundleIdentifier", "com.example.store"), ("CFBundleName", "Store")], PlistFormat::Xml));
    e.fx.write(p.join("Contents/_MASReceipt/receipt").strip_prefix(e.fx.path()).unwrap(), b"opaque receipt");
    let plain = bundle(&e.root().join("Applications/Plain.app"), &info(&[("CFBundleIdentifier", "com.example.store")], PlistFormat::Xml));
    let apps = e.inventory();
    let store = at(&apps, &p);
    assert!(matches!(&store.source, AppSource::AppStore { bundle_id, .. } if bundle_id == "com.example.store"), "{store:#?}");
    assert!(has(store, EvidenceKind::MasReceipt));
    assert_eq!(store.to_wire()["source"], "app_store");
    let plain = at(&apps, &plain);
    assert!(matches!(plain.source, AppSource::AppBundle { .. }) && !has(plain, EvidenceKind::MasReceipt));
    assert_ne!(store.instance_id, plain.instance_id);
}

#[test]
fn cask_metadata_maps_a_bundle_to_its_token() {
    let e = Env::new("apps-cask");
    let fox = bundle(&e.root().join("Applications/Firefox.app"), &info(&[("CFBundleIdentifier", "org.mozilla.firefox")], PlistFormat::Xml));
    let bar = bundle(&e.home().join("Applications/Bar.app"), &info(&[("CFBundleIdentifier", "com.example.bar")], PlistFormat::Xml));
    let nested = bundle(&e.root().join("Applications/Mozilla/Firefox.app"), &info(&[("CFBundleIdentifier", "org.mozilla.firefox")], PlistFormat::Xml));
    e.fx.write(
        "root/opt/homebrew/Caskroom/firefox/.metadata/120.0/20240101000000.000/Casks/firefox.json",
        br#"{"token":"firefox","artifacts":[{"app":["Firefox.app"]},{"zap":[{"trash":["~/Library/Caches/Firefox"]}]}]}"#,
    );
    e.fx.write("root/opt/homebrew/Caskroom/firefox/120.0/.keep", b"");
    e.fx.write(
        "root/usr/local/Caskroom/bar/.metadata/3/20240101/Casks/bar.json",
        br#"{"token":"bar","artifacts":[{"app":["Bar Installer.app",{"target":"Bar.app"}]},{"uninstall":[{"quit":"com.example.bar"}]}]}"#,
    );
    let apps = e.inventory();
    let fox = at(&apps, &fox);
    assert_eq!(fox.source, AppSource::Homebrew { token: "firefox".into(), kind: HomebrewKind::Cask }, "{fox:#?}");
    assert!(detail(fox, EvidenceKind::CaskroomToken).starts_with("firefox (app artifact Firefox.app in "));
    assert_eq!(fox.to_wire()["source"], "homebrew");
    assert_eq!(at(&apps, &bar).source, AppSource::Homebrew { token: "bar".into(), kind: HomebrewKind::Cask }, "a target rename is followed");
    let nested = at(&apps, &nested);
    assert!(matches!(nested.source, AppSource::AppBundle { .. }) && !has(nested, EvidenceKind::CaskroomToken), "casks install at the top level only");
    assert_ne!(fox.instance_id, nested.instance_id);
}

#[test]
fn cask_artifact_names() {
    let v: serde_json::Value = serde_json::from_str(
        r#"{"artifacts":[{"app":["A.app","B.app",{"target":"C.app"},"Sub/D.app"]},{"app":["E.app",{"target":"/Abs/F.app"}]},{"pkg":["x.pkg"]},{"app":["NotAnApp"]}]}"#,
    )
    .unwrap();
    assert_eq!(cask_app_names(&v), ["A.app", "C.app", "D.app"]);
    assert!(cask_app_names(&serde_json::json!({"token": "x"})).is_empty());
}

#[test]
fn pkg_receipts_are_evidence_only() {
    let e = Env::new("apps-pkg");
    let p = bundle(&e.root().join("Applications/Tool.app"), &info(&[("CFBundleIdentifier", "com.example.tool")], PlistFormat::Xml));
    let receipt = |id: &str, version: &str| {
        let bytes = info(&[("PackageIdentifier", id), ("PackageVersion", version), ("InstallPrefixPath", "/")], PlistFormat::Binary);
        e.fx.write(format!("root/var/db/receipts/{id}.plist"), &bytes);
        e.fx.write(format!("root/var/db/receipts/{id}.bom"), b"bom");
    };
    receipt("com.example.tool.pkg", "4.0");
    receipt("com.example.toolbox", "1.0");
    let apps = e.inventory();
    let app = at(&apps, &p);
    let pkgs: Vec<_> = app.evidence.iter().filter(|e| e.kind == EvidenceKind::PkgReceipt).collect();
    assert_eq!(pkgs.len(), 1, "{app:#?}");
    assert!(pkgs[0].detail.starts_with("com.example.tool.pkg 4.0 ("), "{}", pkgs[0].detail);
    assert!(matches!(app.source, AppSource::AppBundle { .. }), "a receipt is never the source");
}

#[test]
fn data_folders_without_an_install_stay_unknown() {
    let e = Env::new("apps-unknown");
    let p = bundle(&e.root().join("Applications/Notes Pro.app"), &info(&[("CFBundleIdentifier", "com.example.notespro"), ("CFBundleName", "Notes Pro")], PlistFormat::Xml));
    let ghost = e.fx.mkdir("home/Library/Application Support/Ghostwriter");
    let alike = e.fx.mkdir("home/Library/Application Support/Notes Pro");
    let apps = e.inventory();
    let report = associate(&apps, &Timestamp("2026-01-01T00:00:00Z".into()));
    let raw = |p: &Path| RawPath::unix(p.as_os_str().as_bytes().to_vec()).unwrap();
    let a = assess_directory(&raw(&ghost), None, InstallRecordStatus::NotFound, &apps, &report.associations);
    assert!(matches!(a.ownership, Ownership::Unknown { reason: UnknownOwnership::NoInstallationRecord, .. }), "{a:?}");
    assert!(!a.cleanup_candidate() && !a.auto_select());
    let a = assess_directory(&raw(&alike), None, InstallRecordStatus::NotFound, &apps, &report.associations);
    assert!(matches!(a.ownership, Ownership::Unknown { reason: UnknownOwnership::NameResemblanceOnly, .. }), "{a:?}");
    assert!(!a.cleanup_candidate() && !a.auto_select());
    let inside = assess_directory(&raw(&p.join("Contents")), None, InstallRecordStatus::NotFound, &apps, &report.associations);
    assert!(matches!(inside.ownership, Ownership::Associated { ref instance_ids, shared: false } if instance_ids == &[at(&apps, &p).instance_id.clone()]));
    assert!(!inside.cleanup_candidate());
}

#[test]
fn links_and_fake_system_applications() {
    let e = Env::new("apps-link");
    let real = bundle(&e.root().join("Applications/Vendor/Real.app"), &info(&[("CFBundleIdentifier", "com.example.real")], PlistFormat::Xml));
    let link = e.home().join("Applications/Linked.app");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    std::os::unix::fs::symlink(e.root().join("Applications/Vendor"), e.home().join("Applications/VendorLink")).unwrap();
    let sys = bundle(&e.root().join("System/Applications/Utilities/Sys.app"), &info(&[("CFBundleIdentifier", "com.example.sys")], PlistFormat::Xml));
    let apps = e.inventory();
    assert_eq!(apps.len(), 3, "a linked folder is not entered: {apps:#?}");
    let linked = at(&apps, &link);
    assert_eq!(linked.location_identity, None);
    assert!(detail(linked, EvidenceKind::LocationUnresolved).contains("symbolic link"), "{linked:#?}");
    let sys = at(&apps, &sys);
    assert_eq!(sys.protected_class, Some(ProtectedReason::OsPackage));
    assert_eq!(sys.to_wire()["protected_class"], true);
    assert_eq!(at(&apps, &real).protected_class, None);
}

#[test]
fn redirection_resolves_every_system_location_under_the_root() {
    let env = AppsEnv { root: Some(PathBuf::from("/tmp/fake")), home: Some(PathBuf::from("/tmp/home")) };
    assert!(env.is_redirected());
    assert_eq!(env.system("/opt/homebrew/Caskroom"), Path::new("/tmp/fake/opt/homebrew/Caskroom"));
    let roots: Vec<_> = env.roots().into_iter().map(|(p, _)| p).collect();
    assert_eq!(roots, [PathBuf::from("/tmp/fake/Applications"), PathBuf::from("/tmp/home/Applications"), PathBuf::from("/tmp/fake/System/Applications")]);
    assert!(!AppsEnv::real().is_redirected());
}

#[test]
fn inventory_capability_is_declared_unverified() {
    assert_eq!(CAPS.len(), 1);
    assert_eq!((CAPS[0].0, CAPS[0].1), ("apps.inventory", CapabilityState::Unverified));
}

/// Read-only: the runner's own `/System/Applications` (Calculator is always there).
#[test]
fn real_system_applications_are_protected_os_packages() {
    let (apps, _issues) = collect(&AppsEnv::real(), &[(PathBuf::from("/System/Applications"), Domain::System)]);
    assert!(apps.len() > 5, "{apps:#?}");
    for app in &apps {
        assert_eq!(app.protected_class, Some(ProtectedReason::OsPackage), "{app:#?}");
        assert!(has(app, EvidenceKind::ProtectedClass));
        assert!(app.install_location.as_ref().is_some_and(|l| l.lossy().starts_with("/System/Applications/")));
    }
    let calc = at(&apps, Path::new("/System/Applications/Calculator.app"));
    assert!(matches!(&calc.source, AppSource::AppBundle { bundle_id, .. } if bundle_id == "com.apple.calculator"), "{calc:#?}");
    assert_eq!(calc.publisher.as_deref(), Some("Apple"));
    assert!(calc.version.is_some() && calc.location_identity.is_some(), "{calc:#?}");
    assert!(apps.iter().any(|a| a.install_location.as_ref().is_some_and(|l| l.lossy().starts_with("/System/Applications/Utilities/"))), "Utilities is included");
}
