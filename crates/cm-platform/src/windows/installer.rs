//! Windows side of installer cleanup (INC-01..05): declared locations with source labels,
//! bounded content reads for recognition, Authenticode signer, attached-image and open-file
//! checks, and the installed-application list for hints. Every read is of a plain local file
//! opened without following links; cloud placeholders and offline files are never opened.

use std::os::windows::fs::{FileExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use cm_core::installer::{ByteSource, InstallerKind, InstallerProbe, Verdict, chromium_download_dir, classify, firefox_download_dir};
use cm_core::path::RawPath;

use super::purge::{open_plain_file, os, raw};

const FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const ERROR_SHARING_VIOLATION: i32 = 32;

pub const SOURCE_DOWNLOADS: &str = "downloads";
pub const SOURCE_DESKTOP: &str = "desktop";
pub const SOURCE_BROWSER: &str = "browser_downloads";

fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    // SAFETY: documented known-folder query; the string is freed once with CoTaskMemFree.
    let p = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }.ok()?;
    // SAFETY: valid NUL-terminated string returned above.
    let s = unsafe { p.to_string() }.ok();
    // SAFETY: allocated by the shell with CoTaskMemAlloc.
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s.map(PathBuf::from)
}

fn read_small(p: &Path, limit: u64) -> Option<String> {
    use std::io::Read;
    let mut s = String::new();
    open_plain_file(p)?.take(limit).read_to_string(&mut s).ok()?;
    Some(s)
}

/// Custom download folders configured in Chromium-family browsers and Firefox (read-only).
fn browser_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        for base in [&["Google", "Chrome"][..], &["Microsoft", "Edge"], &["BraveSoftware", "Brave-Browser"]] {
            let data = base.iter().fold(local.clone(), |p, c| p.join(c)).join("User Data");
            let Ok(rd) = std::fs::read_dir(&data) else { continue };
            for profile in rd.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_dir()).take(32) {
                if let Some(dir) = read_small(&profile.join("Preferences"), 16 << 20).as_deref().and_then(chromium_download_dir) {
                    out.push(PathBuf::from(dir));
                }
            }
        }
    }
    if let Some(roaming) = std::env::var_os("APPDATA").map(PathBuf::from) {
        if let Ok(rd) = std::fs::read_dir(roaming.join("Mozilla").join("Firefox").join("Profiles")) {
            for profile in rd.filter_map(Result::ok).map(|e| e.path()).take(32) {
                if let Some(dir) = read_small(&profile.join("prefs.js"), 4 << 20).as_deref().and_then(firefox_download_dir) {
                    out.push(PathBuf::from(dir));
                }
            }
        }
    }
    out
}

fn inside(child: &Path, parent: &Path) -> bool {
    let (c, p) = (child.to_string_lossy().to_lowercase(), parent.to_string_lossy().to_lowercase());
    let p = p.trim_end_matches('\\');
    c == p || c.starts_with(&format!("{p}\\"))
}

/// Declared locations with their source labels: Downloads, Desktop, then browser download
/// folders. Missing folders and links are left out; a folder inside an earlier one is covered by it.
pub fn declared_locations() -> Vec<(RawPath, &'static str)> {
    use windows::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Downloads};
    let mut listed: Vec<(PathBuf, &'static str)> = Vec::new();
    let all = [(known_folder(&FOLDERID_Downloads), SOURCE_DOWNLOADS), (known_folder(&FOLDERID_Desktop), SOURCE_DESKTOP)]
        .into_iter()
        .filter_map(|(p, l)| p.map(|p| (p, l)))
        .chain(browser_dirs().into_iter().map(|p| (p, SOURCE_BROWSER)));
    for (p, label) in all {
        let plain = std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink() && std::os::windows::fs::MetadataExt::file_attributes(&m) & 0x400 == 0);
        if plain && !listed.iter().any(|(q, _)| inside(&p, q) || inside(q, &p)) {
            listed.push((p, label));
        }
    }
    listed.into_iter().filter_map(|(p, l)| raw(&p).map(|r| (r, l))).collect()
}

/// Bounded random-access reads of a plain local file.
pub struct FileSource {
    file: std::fs::File,
    len: u64,
}

impl FileSource {
    pub fn open(p: &Path) -> Option<FileSource> {
        let file = open_plain_file(p)?;
        let len = file.metadata().ok()?.len();
        Some(FileSource { file, len })
    }
}

impl ByteSource for FileSource {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, offset: u64, len: usize) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; len.min(usize::try_from(self.len.saturating_sub(offset)).unwrap_or(usize::MAX))];
        let mut done = 0;
        while done < buf.len() {
            match self.file.seek_read(&mut buf[done..], offset + done as u64) {
                Ok(0) => break,
                Ok(n) => done += n,
                Err(_) => return None,
            }
        }
        buf.truncate(done);
        Some(buf)
    }
}

/// Whether a disk image file is attached now, via the virtual-disk API (read-only query).
pub fn image_attached(p: &Path, kind: InstallerKind) -> Option<bool> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Storage::Vhd::{
        GetVirtualDiskPhysicalPath, OPEN_VIRTUAL_DISK_FLAG_NONE, OpenVirtualDisk, VIRTUAL_DISK_ACCESS_ATTACH_RO, VIRTUAL_DISK_ACCESS_GET_INFO,
        VIRTUAL_STORAGE_TYPE, VIRTUAL_STORAGE_TYPE_DEVICE_ISO, VIRTUAL_STORAGE_TYPE_DEVICE_VHD, VIRTUAL_STORAGE_TYPE_DEVICE_VHDX,
        VIRTUAL_STORAGE_TYPE_VENDOR_MICROSOFT,
    };
    let device = match kind {
        InstallerKind::Iso => VIRTUAL_STORAGE_TYPE_DEVICE_ISO,
        InstallerKind::Vhd => VIRTUAL_STORAGE_TYPE_DEVICE_VHD,
        InstallerKind::Vhdx => VIRTUAL_STORAGE_TYPE_DEVICE_VHDX,
        _ => return Some(false),
    };
    let st = VIRTUAL_STORAGE_TYPE { DeviceId: device, VendorId: VIRTUAL_STORAGE_TYPE_VENDOR_MICROSOFT };
    let path: Vec<u16> = p.as_os_str().to_string_lossy().encode_utf16().chain([0]).collect();
    let mut h: HANDLE = std::ptr::null_mut();
    // SAFETY: valid storage type, NUL-terminated path, default (null) parameters and out-handle.
    let rc = unsafe { OpenVirtualDisk(&st, path.as_ptr(), VIRTUAL_DISK_ACCESS_GET_INFO | VIRTUAL_DISK_ACCESS_ATTACH_RO, OPEN_VIRTUAL_DISK_FLAG_NONE, std::ptr::null(), &mut h) };
    if rc != 0 {
        return None;
    }
    let mut buf = vec![0u16; 512];
    let mut bytes = (buf.len() * 2) as u32;
    // SAFETY: valid handle; buffer length is passed in bytes.
    let rc = unsafe { GetVirtualDiskPhysicalPath(h, &mut bytes, buf.as_mut_ptr()) };
    // SAFETY: handle from OpenVirtualDisk, closed once.
    unsafe { CloseHandle(h) };
    match rc {
        0 => Some(true),
        // ERROR_VIRTDISK_NOT_VIRTUAL_DISK / ERROR_DEV_NOT_EXIST / ERROR_NOT_READY / ERROR_FILE_NOT_FOUND: not attached.
        0xC03A_0015 | 55 | 21 | 2 => Some(false),
        _ => None,
    }
}

/// Whether a plain local file (never a link, cloud placeholder or offline file) refuses an ordinary shared read open
/// with a sharing violation: another process holds it open without read sharing, so its content cannot be read.
pub fn held_without_read_sharing(p: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const SKIP: u32 = 0x400 | 0x1000 | 0x4_0000 | 0x40_0000; // reparse point, offline, recall on open / on data access
    let plain = std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file() && m.file_attributes() & SKIP == 0);
    plain && std::fs::OpenOptions::new().read(true).custom_flags(FLAG_OPEN_REPARSE_POINT).open(p).err().and_then(|e| e.raw_os_error()) == Some(ERROR_SHARING_VIOLATION)
}

/// Whether another process holds the file open (an exclusive read open is refused).
pub fn open_elsewhere(p: &Path) -> Option<bool> {
    match std::fs::OpenOptions::new().read(true).share_mode(0).custom_flags(FLAG_OPEN_REPARSE_POINT).open(p) {
        Ok(_) => Some(false),
        Err(e) if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION) => Some(true),
        Err(_) => None,
    }
}

/// Installed applications as (display name, publisher) for installer hints.
pub fn installed_apps() -> Vec<(String, Option<String>)> {
    super::apps::inventory().0.into_iter().map(|a| (a.name, a.publisher)).collect()
}

/// The platform evidence used by `cm_core::installer::analyze_installers`.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinInstallerProbe;

impl InstallerProbe for WinInstallerProbe {
    fn classify(&self, path: &RawPath) -> Verdict {
        let Some(p) = os(path) else { return Verdict::NotInstaller { reason: "not a Windows path".into() } };
        match FileSource::open(&p) {
            Some(src) => classify(&src),
            None if held_without_read_sharing(&p) => {
                Verdict::InUse { reason: "another process holds the file open without read sharing, so its content could not be read".into() }
            }
            None => Verdict::NotInstaller { reason: "not a readable plain local file (link, cloud placeholder or offline)".into() },
        }
    }

    fn signer(&self, path: &RawPath) -> Option<String> {
        let p = os(path)?;
        open_plain_file(&p)?; // never let signature verification recall cloud content
        super::startup::authenticode_signer(&p.to_string_lossy())
    }

    fn mounted(&self, path: &RawPath) -> Option<bool> {
        let p = os(path)?;
        let kind = match classify(&FileSource::open(&p)?) {
            Verdict::Installer { kind, .. } => kind,
            Verdict::NotInstaller { .. } | Verdict::InUse { .. } => return None,
        };
        image_attached(&p, kind)
    }

    fn in_use(&self, path: &RawPath) -> Option<bool> {
        open_elsewhere(&os(path)?)
    }

    /// Restart Manager's list of holders (read-only; nothing is ended or opened).
    fn holders(&self, path: &RawPath) -> Option<Vec<String>> {
        super::restart_manager::holders(std::slice::from_ref(path)).ok().map(|h| h.iter().map(super::restart_manager::Holder::describe).collect())
    }
}

#[cfg(test)]
mod tests;
