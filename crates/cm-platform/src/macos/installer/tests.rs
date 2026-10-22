//! Installer recognition and live-state evidence on a real macOS file system (fixtures under the canonical
//! temporary directory). The attached-image test creates and attaches a disposable image only with
//! `CM_ALLOW_DMG_TESTS=1`.

use std::process::Command;

use super::*;
use crate::macos::testfs::Fixture;

/// A synthetic UDIF image: zero data followed by a 512-byte `koly` trailer.
fn synthetic_dmg(tag: u8) -> Vec<u8> {
    let mut b = vec![tag; 2048];
    let mut trailer = vec![0u8; 512];
    trailer[..4].copy_from_slice(b"koly");
    trailer[4..8].copy_from_slice(&4u32.to_be_bytes());
    b.extend_from_slice(&trailer);
    b
}

/// A synthetic flat package: the `xar!` header and an (unparseable) table of contents.
fn synthetic_pkg() -> Vec<u8> {
    let mut b = b"xar!".to_vec();
    b.extend_from_slice(&[0, 28, 0, 1]);
    b.resize(512, 0x5a);
    b
}

fn kind_of(p: &Path) -> Option<InstallerKind> {
    match classify_file(p) {
        Verdict::Installer { kind, .. } => Some(kind),
        Verdict::NotInstaller { .. } | Verdict::InUse { .. } => None,
    }
}

fn zip(dir: &Path, out: &str, files: &[&str]) -> bool {
    Command::new("/usr/bin/zip").current_dir(dir).arg("-q").arg("-r").arg(out).args(files).output().is_ok_and(|o| o.status.success())
}

#[test]
fn images_and_packages_are_recognized_by_content_not_by_name() {
    let fx = Fixture::new("inst-content");
    let dmg = fx.write("Tool.dmg", &synthetic_dmg(0));
    let renamed = fx.write("Tool-copy.bin", &synthetic_dmg(1));
    let pkg = fx.write("Tool.pkg", &synthetic_pkg());
    let xip = fx.write("Xcode.xip", &synthetic_pkg());
    let fake = fx.write("setup.dmg", b"This is a text file that was renamed to setup.dmg to look like an image........");
    let pdf = fx.write("notes.pdf", b"%PDF-1.7 just a document, nothing to install here at all......................");
    assert_eq!(kind_of(&dmg), Some(InstallerKind::Dmg));
    assert_eq!(kind_of(&renamed), Some(InstallerKind::Dmg), "content decides, not the extension");
    assert_eq!(kind_of(&pkg), Some(InstallerKind::Pkg));
    match classify_file(&xip) {
        Verdict::Installer { kind: InstallerKind::Pkg, evidence } => assert!(evidence.starts_with("xip"), "{evidence}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(kind_of(&fake), None);
    assert_eq!(kind_of(&pdf), None);
    // Links are never followed.
    let link = fx.path().join("link.dmg");
    std::os::unix::fs::symlink(&dmg, &link).unwrap();
    assert_eq!(kind_of(&link), None);
}

#[test]
fn zips_count_by_payload_or_review_only_by_name() {
    let fx = Fixture::new("inst-zip");
    fx.write("report.txt", b"quarterly numbers");
    fx.write("Tool.dmg", &synthetic_dmg(0));
    fx.write("Fixture.app/Contents/Info.plist", b"<plist/>");
    if !(zip(fx.path(), "docs.zip", &["report.txt"])
        && zip(fx.path(), "bundle.zip", &["report.txt", "Tool.dmg"])
        && zip(fx.path(), "Fixture.app.zip", &["Fixture.app"])
        && zip(fx.path(), "Fixture-1.2-macos.zip", &["Fixture.app"]))
    {
        eprintln!("skipped: /usr/bin/zip cannot write archives here");
        return;
    }
    assert_eq!(kind_of(&fx.path().join("docs.zip")), None, "an archive of documents is not an installer");
    match classify_file(&fx.path().join("bundle.zip")) {
        Verdict::Installer { kind: InstallerKind::Archive, evidence } => assert!(evidence.contains("installer payload"), "{evidence}"),
        other => panic!("{other:?}"),
    }
    for name in ["Fixture.app.zip", "Fixture-1.2-macos.zip"] {
        match classify_file(&fx.path().join(name)) {
            Verdict::Installer { kind: InstallerKind::Archive, evidence } => assert!(evidence.starts_with("archive by name only"), "{name}: {evidence}"),
            other => panic!("{name}: {other:?}"),
        }
    }
    assert_eq!(zip_name_hint("Notes.zip"), None);
    assert_eq!(zip_name_hint("tool-darwin-arm64.zip").as_deref(), Some("\"darwin\" in the name"));
    assert_eq!(zip_name_hint("tool.dmg"), None);
}

#[test]
fn pkgutil_output_gives_the_signer_only_when_trusted() {
    let developer = "Package \"Foo.pkg\":\n   Status: signed by a developer certificate issued by Apple for distribution\n   Notarization: trusted by the Apple notary service\n   Certificate Chain:\n    1. Developer ID Installer: Example Corp (ABCDE12345)\n       Expires: 2030-01-01\n    2. Developer ID Certification Authority\n    3. Apple Root CA\n";
    assert_eq!(parse_pkgutil(developer).as_deref(), Some("Example Corp (ABCDE12345)"));
    assert_eq!(parse_pkgutil("Package \"x.pkg\":\n   Status: signed Apple Software\n   Certificate Chain:\n    1. Software Update\n").as_deref(), Some("Apple"));
    assert_eq!(parse_pkgutil("Package \"x.pkg\":\n   Status: no signature\n"), None);
    assert_eq!(parse_pkgutil("Package \"x.pkg\":\n   Status: signed by untrusted certificate\n    1. Someone\n"), None);
    assert_eq!(parse_pkgutil("garbage"), None);
}

#[test]
fn a_built_package_is_a_pkg_without_a_signer() {
    let fx = Fixture::new("inst-pkgbuild");
    let out = fx.path().join("Fixture.pkg");
    let built = Command::new("/usr/bin/pkgbuild")
        .args(["--nopayload", "--identifier", "org.cleanmanager.test.fixture", "--version", "1.0"])
        .arg(&out)
        .output()
        .is_ok_and(|o| o.status.success());
    if !built {
        eprintln!("skipped: pkgbuild cannot build a package here");
        return;
    }
    assert_eq!(kind_of(&out), Some(InstallerKind::Pkg));
    let probe = MacInstallerProbe::default();
    assert!(probe.signs(InstallerKind::Pkg) && !probe.signs(InstallerKind::Dmg));
    assert_eq!(probe.signer(&raw(&out).unwrap()), None, "pkgbuild without --sign writes an unsigned package");
    assert_eq!(probe.mounted(&raw(&out).unwrap()), Some(false));
}

#[test]
fn open_files_are_reported_in_use() {
    let fx = Fixture::new("inst-busy");
    let f = fx.write("busy.pkg", &synthetic_pkg());
    assert_eq!(MacInstallerProbe::default().in_use(&raw(&f).unwrap()), Some(false));
    let holder = std::fs::File::open(&f).unwrap();
    assert_eq!(MacInstallerProbe::default().in_use(&raw(&f).unwrap()), Some(true));
    drop(holder);
    assert_eq!(MacInstallerProbe::default().in_use(&raw(&f).unwrap()), Some(false));
    assert_eq!(MacInstallerProbe::default().in_use(&raw(&fx.path().join("gone.pkg")).unwrap()), None);
}

#[test]
fn hdiutil_info_lists_attached_images() {
    // Read-only query; on a machine with nothing attached the list is simply empty.
    let list = attached_images().expect("hdiutil info -plist");
    eprintln!("{} image(s) attached", list.len());
    let info = Plist::Dict(
        [(
            "images".to_owned(),
            Plist::Array(vec![
                Plist::Dict([("image-path".to_owned(), Plist::String("/Users/x/Downloads/A.dmg".into()))].into_iter().collect()),
                Plist::Dict(Default::default()),
            ]),
        )]
        .into_iter()
        .collect(),
    );
    assert_eq!(image_paths(&info), [PathBuf::from("/Users/x/Downloads/A.dmg")]);
    assert!(image_paths(&Plist::Dict(Default::default())).is_empty());
}

#[test]
fn attached_images_are_mounted_and_in_use() {
    if std::env::var_os("CM_ALLOW_DMG_TESTS").is_none() {
        eprintln!("skipped: set CM_ALLOW_DMG_TESTS=1 to attach a disposable disk image");
        return;
    }
    let fx = Fixture::new("inst-dmg");
    let image = fx.path().join("Fixture.dmg");
    let mnt = fx.mkdir("mnt");
    fx.write("src/ReadMe.txt", b"disposable image content");
    // A compressed UDIF image, the format downloads come in.
    let ok = Command::new(HDIUTIL).args(["create", "-quiet", "-volname", "cminst", "-format", "UDZO", "-srcfolder"]).arg(fx.path().join("src")).arg("-o").arg(&image).status().unwrap();
    assert!(ok.success());
    assert_eq!(kind_of(&image), Some(InstallerKind::Dmg), "UDIF images end in a koly trailer: {:?}", classify_file(&image));
    let r = raw(&image).unwrap();
    assert_eq!(MacInstallerProbe::default().mounted(&r), Some(false));
    let ok = Command::new(HDIUTIL).args(["attach", "-quiet", "-nobrowse", "-mountpoint"]).arg(&mnt).arg(&image).status().unwrap();
    assert!(ok.success());
    let probe = MacInstallerProbe::default();
    let (mounted, in_use) = (probe.mounted(&r), probe.in_use(&r));
    let _ = Command::new(HDIUTIL).args(["detach", "-quiet", "-force"]).arg(&mnt).status();
    assert_eq!((mounted, in_use), (Some(true), Some(true)));
    assert_eq!(MacInstallerProbe::default().mounted(&r), Some(false), "detached again");
}

#[test]
fn declared_locations_are_plain_directories_with_labels() {
    let locs = declared_locations();
    for (p, label) in &locs {
        let path = os(p).unwrap();
        assert!(path.is_dir(), "{}", path.display());
        assert!(LABELS.contains(label));
    }
    eprintln!("declared locations: {:?}", locs.iter().map(|(p, l)| (p.display(), *l)).collect::<Vec<_>>());
    assert_eq!(signer_note(InstallerKind::Pkg, Some("Example Corp (ABCDE12345)")), "signed by Example Corp (ABCDE12345) (pkgutil --check-signature)");
}

#[test]
fn the_capability_row_is_unverified_with_a_reason() {
    assert_eq!(CAPS.len(), 1);
    assert_eq!((CAPS[0].0, CAPS[0].1), ("installer.cleanup", CapabilityState::Unverified));
    assert!(CAPS[0].2.contains("gate G1 not passed"));
}
