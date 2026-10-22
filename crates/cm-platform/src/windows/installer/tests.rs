//! Installer recognition on real files: temp fixtures for writes, read-only reads of system files.

use std::os::windows::fs::OpenOptionsExt;
use std::process::Command;

use super::*;

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-installer-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn verdict(p: &Path) -> Verdict {
    WinInstallerProbe.classify(&raw(p).unwrap())
}

fn is_installer(v: &Verdict) -> bool {
    matches!(v, Verdict::Installer { .. })
}

#[test]
fn ordinary_and_renamed_files_are_not_installers() {
    let root = scratch("plain");
    let fake = root.join("setup.exe");
    std::fs::write(&fake, b"This is a text file that was renamed to setup.exe to look like an installer.").unwrap();
    assert!(!is_installer(&verdict(&fake)));
    let iso = root.join("disk.iso");
    std::fs::write(&iso, vec![b'x'; 40_000]).unwrap();
    assert!(!is_installer(&verdict(&iso)));
    // A real program that is not an installer.
    let notepad = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())).join("System32").join("notepad.exe");
    if notepad.exists() {
        assert!(!is_installer(&verdict(&notepad)), "{:?}", verdict(&notepad));
    }
}

#[test]
fn zip_archives_need_installer_payload() {
    let root = scratch("zip");
    std::fs::write(root.join("report.txt"), "quarterly numbers").unwrap();
    std::fs::write(root.join("tool.msi"), "pretend payload").unwrap();
    let docs = root.join("docs.zip");
    let with = root.join("bundle.zip");
    // The system bsdtar writes ZIP archives when the name ends in .zip.
    let tar = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join("System32").join("tar.exe");
    let zip = |out: &Path, files: &[&str]| Command::new(&tar).arg("-a").arg("-cf").arg(out).arg("-C").arg(&root).args(files).output().is_ok_and(|o| o.status.success());
    if !zip(&docs, &["report.txt"]) || !zip(&with, &["report.txt", "tool.msi"]) {
        eprintln!("skipped: tar.exe cannot write zip archives here");
        return;
    }
    assert!(!is_installer(&verdict(&docs)), "an archive of documents is not an installer");
    assert!(matches!(verdict(&with), Verdict::Installer { kind: InstallerKind::Archive, .. }));
}

#[test]
fn cached_windows_installer_packages_are_msi_with_a_signer() {
    // Read-only look at the system's cached packages (never modified).
    let dir = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())).join("Installer");
    let Some(msi) = std::fs::read_dir(&dir).ok().and_then(|rd| rd.filter_map(Result::ok).map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("msi")))) else {
        eprintln!("skipped: no cached .msi under {}", dir.display());
        return;
    };
    assert!(matches!(verdict(&msi), Verdict::Installer { kind: InstallerKind::Msi, .. }), "{}: {:?}", msi.display(), verdict(&msi));
    eprintln!("{} signer: {:?}", msi.display(), WinInstallerProbe.signer(&raw(&msi).unwrap()));
}

#[test]
fn open_files_are_reported_in_use() {
    let root = scratch("busy");
    let f = root.join("a.msi");
    std::fs::write(&f, b"x").unwrap();
    assert_eq!(open_elsewhere(&f), Some(false));
    let holder = std::fs::OpenOptions::new().read(true).share_mode(1).open(&f).unwrap();
    assert_eq!(open_elsewhere(&f), Some(true));
    drop(holder);
    assert_eq!(open_elsewhere(&f), Some(false));
}

/// WFA-58: a file opened elsewhere without read sharing (`FileShare.None`) cannot be read; classification says so
/// (instead of "not an installer"), and the holder is named. Released, it is recognized by content again.
#[test]
fn a_file_held_without_read_sharing_is_in_use_with_its_holder() {
    let root = scratch("held");
    let f = root.join("held.msi");
    // A minimal compound file whose root storage carries the Windows Installer package class id.
    let mut msi = vec![0u8; 1024];
    msi[..8].copy_from_slice(&[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]);
    msi[0x1e..0x20].copy_from_slice(&9u16.to_le_bytes());
    msi[512 + 0x50..512 + 0x60].copy_from_slice(&[0x84, 0x10, 0x0c, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
    std::fs::write(&f, msi).unwrap();
    assert!(matches!(verdict(&f), Verdict::Installer { kind: InstallerKind::Msi, .. }), "{:?}", verdict(&f));
    let holder = std::fs::OpenOptions::new().read(true).share_mode(0).open(&f).unwrap();
    assert!(held_without_read_sharing(&f));
    assert!(matches!(verdict(&f), Verdict::InUse { .. }), "{:?}", verdict(&f));
    let held_by = WinInstallerProbe.holders(&raw(&f).unwrap()).expect("Restart Manager answers");
    let me = format!("pid {}", std::process::id());
    assert!(held_by.iter().any(|h| h.contains(&me)), "{held_by:?}");
    drop(holder);
    assert!(!held_without_read_sharing(&f));
    assert!(matches!(verdict(&f), Verdict::Installer { kind: InstallerKind::Msi, .. }));
}

#[test]
fn attached_images_are_detected() {
    if !super::super::testvhd::allowed() {
        eprintln!("skipped: set CM_ALLOW_VHD_TESTS=1 (elevated) to attach a scratch image");
        return;
    }
    let root = scratch("vhd");
    let file = root.join("img.vhdx");
    let letter = super::super::testvhd::free_letter();
    let vhd = super::super::testvhd::Vhd::create(file.clone(), "ntfs", letter);
    assert_eq!(image_attached(&file, InstallerKind::Vhdx), Some(true));
    assert_eq!(WinInstallerProbe.mounted(&raw(&file).unwrap()), Some(true));
    assert_eq!(open_elsewhere(&file), Some(true), "an attached image is also open, so execution skips it as in use");
    vhd.dismount();
    assert_eq!(image_attached(&file, InstallerKind::Vhdx), Some(false));
    assert!(matches!(verdict(&file), Verdict::Installer { kind: InstallerKind::Vhdx, .. }));
}

#[test]
fn declared_locations_are_plain_directories_with_labels() {
    let locs = declared_locations();
    for (p, label) in &locs {
        let path = os(p).unwrap();
        assert!(path.is_dir(), "{}", path.display());
        assert!([SOURCE_DOWNLOADS, SOURCE_DESKTOP, SOURCE_BROWSER].contains(label));
    }
    eprintln!("declared locations: {:?}", locs.iter().map(|(p, l)| (p.display(), *l)).collect::<Vec<_>>());
}
