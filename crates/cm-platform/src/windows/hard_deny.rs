//! Platform hard-deny roots for the protection policy (parity change PRT-03).
//!
//! - [`hard_deny_roots`] resolves the Windows system locations that no rule, user entry,
//!   manual selection or authorization may touch, by volume + file identity (the same
//!   `volume_id`/`file_id` form `win.directory` records). Locations that do not exist or
//!   cannot be opened on this machine are skipped; the textual pre-filter in
//!   `cm_core::protect` still covers their well-known spellings.
//! - [`identity_chain`] collects what `cm_core::protect::check_path` needs for one path: the
//!   object's own identity (links are not followed), the identity of every ancestor, the
//!   object's real path, and the resolution of every symlink/junction/mount point on the way.
//!   Only name-surrogate reparse points are followed, so cloud placeholders are never recalled.

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use cm_core::path::RawPath;
use cm_core::protect::{self, HardDenyClass, HardDenyRoot, HardDenySet, IdentityChain, LinkResolution, ObjectIdentity, ProtectionDecision, ResolvedTarget};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_TAG_INFO, FileAttributeTagInfo, GetDriveTypeW, GetFileInformationByHandleEx, GetFinalPathNameByHandleW,
    GetLogicalDrives,
};

use super::directory::identity_of;
use super::handle::{FILE_READ_ATTRIBUTES, Handle, SYNCHRONIZE, full_path};

const ATTR_REPARSE_POINT: u32 = 0x400;
const TAG_NAME_SURROGATE: u32 = 0x2000_0000;
const SHARE_ALL: u32 = 0x1 | 0x2 | 0x4;
const OPEN_EXISTING: u32 = 3;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const VOLUME_NAME_DOS: u32 = 0x0;
const VOLUME_NAME_GUID: u32 = 0x1;
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_RAMDISK: u32 = 6;

fn wide(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().collect()
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn identity(path: &[u16]) -> io::Result<ObjectIdentity> {
    identity_of(path).map(|(v, f)| ObjectIdentity::new(v, f))
}

/// Candidate hard-deny locations for this machine, before identity resolution.
fn candidates() -> Vec<(HardDenyClass, PathBuf)> {
    use HardDenyClass::*;
    let mut out: Vec<(HardDenyClass, PathBuf)> = Vec::new();

    let system_root = env_path("SystemRoot").or_else(|| env_path("windir")).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    for sub in ["", "System32", "SysWOW64", "WinSxS"] {
        out.push((SystemRoot, if sub.is_empty() { system_root.clone() } else { system_root.join(sub) }));
    }
    let system_drive = env_path("SystemDrive").unwrap_or_else(|| PathBuf::from("C:"));
    let system_drive_root = PathBuf::from(format!("{}\\", system_drive.display()));

    for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Some(p) = env_path(var) {
            out.push((ProgramFiles, p));
        }
    }
    out.push((ProgramFiles, system_drive_root.join("Program Files")));
    out.push((ProgramFiles, system_drive_root.join("Program Files (x86)")));

    let program_data = env_path("ProgramData").or_else(|| env_path("ALLUSERSPROFILE")).unwrap_or_else(|| system_drive_root.join("ProgramData"));
    out.push((ProgramDataRoot, program_data.clone()));
    let ms = program_data.join("Microsoft");
    out.push((SecuritySoftware, ms.join("Windows Defender")));
    out.push((SecuritySoftware, ms.join("Windows Defender Advanced Threat Protection")));
    out.push((SecuritySoftware, ms.join("Windows Security Health")));
    out.push((Credentials, ms.join("Crypto")));

    let public = env_path("PUBLIC");
    let users_root = public.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(|| system_drive_root.join("Users"));
    out.push((UsersRoot, users_root.clone()));
    out.push((DefaultProfile, users_root.join("Default")));
    out.push((PublicProfile, public.unwrap_or_else(|| users_root.join("Public"))));

    let mut profiles: Vec<PathBuf> = std::fs::read_dir(&users_root)
        .map(|it| it.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect())
        .unwrap_or_default();
    if let Some(p) = env_path("USERPROFILE") {
        profiles.push(p);
    }
    for profile in profiles {
        let roaming = profile.join("AppData").join("Roaming").join("Microsoft");
        let local = profile.join("AppData").join("Local").join("Microsoft");
        for p in [roaming.join("Protect"), roaming.join("Credentials"), roaming.join("SystemCertificates"), roaming.join("Crypto"), roaming.join("Vault")] {
            out.push((Credentials, p));
        }
        out.push((Credentials, local.join("Credentials")));
        out.push((Credentials, local.join("Vault")));
        out.push((UserProfileRoot, profile));
    }

    // SAFETY: no arguments; returns a bitmask of present drive letters.
    let mask = unsafe { GetLogicalDrives() };
    for (i, letter) in ('A'..='Z').enumerate() {
        if mask & (1 << i) == 0 {
            continue;
        }
        let root = PathBuf::from(format!("{letter}:\\"));
        let mut z = wide(&root);
        z.push(0);
        // SAFETY: NUL-terminated root path. Network, optical and unknown drives are skipped so
        // a disconnected share or an empty tray never blocks the policy load.
        let kind = unsafe { GetDriveTypeW(z.as_ptr()) };
        if !matches!(kind, DRIVE_REMOVABLE | DRIVE_FIXED | DRIVE_RAMDISK) {
            continue;
        }
        out.push((VolumeRoot, root.clone()));
        out.push((VolumeMetadata, root.join("System Volume Information")));
        out.push((Recovery, root.join("Recovery")));
        out.push((InstallerState, root.join("Config.Msi")));
        // Volume-root quarantine used when the store is on another volume (actions.rs).
        out.push((ToolStore, root.join(".cleanmanager-quarantine")));
        let bin = root.join("$Recycle.Bin");
        if let Ok(it) = std::fs::read_dir(&bin) {
            for e in it.flatten() {
                out.push((RecycleBinRoot, e.path()));
            }
        }
        out.push((RecycleBinRoot, bin));
    }

    if let Some(store) = crate::paths::default_store_dir() {
        out.push((ToolStore, store));
    }
    out
}

/// Hard-deny roots of this machine, resolved by identity. A root that is itself a link also
/// contributes its target's identity, so neither spelling escapes.
pub fn hard_deny_roots() -> Vec<HardDenyRoot> {
    let mut out = Vec::new();
    for (class, path) in candidates() {
        let units = wide(&path);
        let Ok(id) = identity(&units) else { continue };
        let display = path.display().to_string();
        if let Some(t) = is_name_surrogate(&units).then(|| resolve_link(&units).ok()).flatten() {
            out.push(HardDenyRoot { class, volume_id: t.object.volume_id, file_id: t.object.file_id, display: display.clone() });
        }
        out.push(HardDenyRoot { class, volume_id: id.volume_id, file_id: id.file_id, display });
    }
    out
}

pub fn hard_deny_set() -> HardDenySet {
    HardDenySet::new(hard_deny_roots())
}

/// Hard-deny layer for one path: textual pre-filter plus identity checks.
pub fn check(path: &RawPath, hard: &HardDenySet) -> ProtectionDecision {
    match path {
        RawPath::Windows(units) => protect::check_path(path, &identity_chain(units), hard),
        RawPath::Unix(_) => ProtectionDecision::Denied { layer: protect::DenyLayer::HardDeny, reason: "not a Windows path".into() },
    }
}

/// Length of the root (`\\?\C:\`, `\\?\UNC\host\share\`, `\\?\Volume{..}\`) of a verbatim path.
fn verbatim_root_len(v: &[u16]) -> Option<usize> {
    let s = String::from_utf16_lossy(v);
    let rest = s.strip_prefix(r"\\?\")?;
    let b = rest.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\' {
        return Some(7);
    }
    if rest.len() >= 4 && rest[..4].eq_ignore_ascii_case(r"UNC\") {
        let mut seps = v.iter().enumerate().skip(8).filter(|(_, u)| **u == u16::from(b'\\'));
        seps.next()?;
        return Some(seps.next().map_or(v.len(), |(i, _)| i + 1));
    }
    if rest.len() >= 7 && rest[..7].eq_ignore_ascii_case("Volume{") {
        let end = v.iter().position(|u| *u == u16::from(b'}'))?;
        return (v.get(end + 1) == Some(&u16::from(b'\\'))).then_some(end + 2);
    }
    None
}

/// Root and every successive prefix of a verbatim path (root first, the path itself last).
fn prefixes(v: &[u16]) -> Option<Vec<Vec<u16>>> {
    let mut v = v.to_vec();
    if v.len() == 6 {
        v.push(u16::from(b'\\')); // `\\?\C:` -> `\\?\C:\`
    }
    let root = verbatim_root_len(&v)?;
    let mut out = vec![v[..root].to_vec()];
    let mut cur = v[..root].to_vec();
    for comp in v[root..].split(|u| *u == u16::from(b'\\')).filter(|c| !c.is_empty()) {
        if cur.last() != Some(&u16::from(b'\\')) {
            cur.push(u16::from(b'\\'));
        }
        cur.extend_from_slice(comp);
        out.push(cur.clone());
    }
    Some(out)
}

/// Whether the object (not followed) is a symlink, junction or other name-surrogate reparse
/// point. Cloud placeholders and dedup points are not links and are never followed.
fn is_name_surrogate(path: &[u16]) -> bool {
    let Ok(h) = Handle::open(path, FILE_READ_ATTRIBUTES | SYNCHRONIZE) else { return false };
    let mut t: FILE_ATTRIBUTE_TAG_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: buffer is a FILE_ATTRIBUTE_TAG_INFO of the declared size.
    let ok = unsafe { GetFileInformationByHandleEx(h.raw(), FileAttributeTagInfo, (&mut t as *mut FILE_ATTRIBUTE_TAG_INFO).cast(), size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32) };
    ok != 0 && t.FileAttributes & ATTR_REPARSE_POINT != 0 && t.ReparseTag & TAG_NAME_SURROGATE != 0
}

/// Owned handle that follows links (used only to learn a link's final target).
struct Followed(HANDLE);

impl Followed {
    fn open(path: &[u16]) -> io::Result<Self> {
        let mut z = path.to_vec();
        z.push(0);
        // SAFETY: NUL-terminated wide path; attribute access only, no content is read.
        let h = unsafe { CreateFileW(z.as_ptr(), FILE_READ_ATTRIBUTES | SYNCHRONIZE, SHARE_ALL, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, std::ptr::null_mut()) };
        if h == INVALID_HANDLE_VALUE { Err(io::Error::last_os_error()) } else { Ok(Followed(h)) }
    }
}

impl Drop for Followed {
    fn drop(&mut self) {
        // SAFETY: handle from CreateFileW, closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// `\\?\` final path of an open handle (drive-letter form, else volume-GUID form).
fn final_path(h: HANDLE) -> io::Result<Vec<u16>> {
    for flags in [VOLUME_NAME_DOS, VOLUME_NAME_GUID] {
        let mut buf = vec![0u16; 32_768];
        // SAFETY: buffer length is passed in characters.
        let n = unsafe { GetFinalPathNameByHandleW(h, buf.as_mut_ptr(), buf.len() as u32, flags) } as usize;
        if n > 0 && n < buf.len() {
            buf.truncate(n);
            return Ok(buf);
        }
    }
    Err(io::Error::last_os_error())
}

fn resolve_link(link: &[u16]) -> Result<ResolvedTarget, String> {
    let h = Followed::open(link).map_err(|e| format!("target cannot be opened: {e}"))?;
    let real = final_path(h.0).map_err(|e| format!("target path unavailable: {e}"))?;
    drop(h);
    let chain = prefixes(&real).ok_or("target path has an unknown form")?;
    let (object_path, ancestors) = chain.split_last().ok_or("empty target path")?;
    let object = identity(object_path).map_err(|e| format!("target cannot be identified: {e}"))?;
    let ancestors = ancestors.iter().map(|a| identity(a)).collect::<io::Result<Vec<_>>>().map_err(|e| format!("target ancestor cannot be identified: {e}"))?;
    let path = RawPath::windows(real).map_err(|e| e.to_string())?;
    Ok(ResolvedTarget { path, object, ancestors })
}

/// Identity facts for one path; see the module docs. Failures are recorded as `None` /
/// `Err` so the core check fails closed.
pub fn identity_chain(path: &[u16]) -> IdentityChain {
    let mut chain = IdentityChain::default();
    let Ok((_, verbatim)) = full_path(path) else { return chain };
    let Some(all) = prefixes(&verbatim) else { return chain };
    let last = all.len() - 1;
    for (i, p) in all.iter().enumerate() {
        let id = identity(p).ok();
        if i > 0 && is_name_surrogate(p) {
            let link = RawPath::windows(p.clone()).unwrap_or_else(|_| RawPath::Windows(vec![u16::from(b'?')]));
            chain.links.push(LinkResolution { link, on_object: i == last, target: resolve_link(p) });
        }
        if i == last {
            chain.object = id;
        } else {
            chain.ancestors.push(id);
        }
    }
    chain.real_path = Handle::open(&all[last], FILE_READ_ATTRIBUTES | SYNCHRONIZE)
        .ok()
        .and_then(|h| final_path(h.raw()).ok())
        .and_then(|real| RawPath::windows(real).ok());
    chain
}

#[cfg(test)]
mod tests;
