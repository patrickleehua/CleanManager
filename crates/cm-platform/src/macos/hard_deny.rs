//! macOS hard-deny roots for the protection policy (parity change PRT-03).
//!
//! - [`hard_deny_roots`] resolves the system locations that no rule, user entry, manual
//!   selection or authorization may touch, by volume + file identity in the same form the scan
//!   backends record (`uuid:`/`fsid:` volume, `inode64:` file). Locations that do not exist are
//!   skipped; the textual pre-filter in `cm_core::protect` still covers their spellings. APFS is
//!   case-insensitive by default, so identity also catches case variants the text table misses.
//! - [`identity_chain`] collects what `cm_core::protect::check_path` needs for one path: the
//!   object's identity (a symlink is not followed), every ancestor's identity as the kernel
//!   resolves it, the object's real path and the resolution of every symlink on the way.
//!   Firmlinks (`/Users` → `/System/Volumes/Data/Users`) are transparent and need no handling.

use std::collections::HashMap;
use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use cm_core::path::RawPath;
use cm_core::protect::{self, HardDenyClass, HardDenyRoot, HardDenySet, IdentityChain, LinkResolution, ObjectIdentity, ProtectionDecision, ResolvedTarget};

use super::sys::{Fd, describe, volume_of};

fn cstr(p: &Path) -> Result<CString, String> {
    CString::new(p.as_os_str().as_bytes()).map_err(|_| "path contains NUL".to_owned())
}

fn raw(p: &Path) -> Option<RawPath> {
    RawPath::unix(p.as_os_str().as_bytes().to_vec()).ok()
}

fn is_symlink(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

/// Identifies objects without following them; caches volume ids per device.
#[derive(Default)]
struct Identifier {
    volumes: HashMap<i32, String>,
}

impl Identifier {
    fn identity(&mut self, path: &Path) -> Result<ObjectIdentity, String> {
        let (fd, st) = match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => {
                let pfd = Fd::open_dir_following(&cstr(parent)?).map_err(describe)?;
                let st = pfd.stat_child(name.as_bytes()).map_err(describe)?;
                if st.st_mode & libc::S_IFMT == libc::S_IFDIR {
                    // A directory may be a mount point: its own descriptor names its volume.
                    let name = CString::new(name.as_bytes()).map_err(|_| "path contains NUL")?;
                    (pfd.open_child_dir(&name).map_err(describe)?, st)
                } else {
                    (pfd, st)
                }
            }
            _ => {
                let fd = Fd::open_dir(&cstr(path)?).map_err(describe)?;
                let st = fd.stat().map_err(describe)?;
                (fd, st)
            }
        };
        let volume = match self.volumes.get(&st.st_dev) {
            Some(v) => v.clone(),
            None => {
                let v = volume_of(&fd, st.st_dev).map_err(describe)?.id;
                self.volumes.insert(st.st_dev, v.clone());
                v
            }
        };
        Ok(ObjectIdentity::new(volume, format!("inode64:{:016x}", st.st_ino)))
    }

    /// Final location of a symlink and the identities of it and its ancestors.
    fn resolve(&mut self, link: &Path) -> Result<ResolvedTarget, String> {
        let real = std::fs::canonicalize(link).map_err(|e| format!("target cannot be resolved: {e}"))?;
        let object = self.identity(&real).map_err(|e| format!("target cannot be identified: {e}"))?;
        let ancestors = real.ancestors().skip(1).map(|a| self.identity(a)).collect::<Result<Vec<_>, _>>().map_err(|e| format!("target ancestor cannot be identified: {e}"))?;
        let path = raw(&real).ok_or("target path is empty")?;
        Ok(ResolvedTarget { path, object, ancestors })
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn children(dir: &str) -> Vec<PathBuf> {
    std::fs::read_dir(dir).map(|it| it.flatten().map(|e| e.path()).collect()).unwrap_or_default()
}

/// Candidate hard-deny locations for this machine, before identity resolution.
fn candidates() -> Vec<(HardDenyClass, PathBuf)> {
    use HardDenyClass::*;
    let mut out: Vec<(HardDenyClass, PathBuf)> = Vec::new();
    let mut add = |class, p: &str| out.push((class, PathBuf::from(p)));
    for p in ["/System", "/bin", "/sbin", "/dev", "/private/etc", "/private/var/db", "/private/var/root", "/private/var/vm", "/Library/Apple"] {
        add(SystemRoot, p);
    }
    for p in ["/usr", "/usr/local", "/private", "/private/var", "/private/var/folders", "/private/tmp", "/Library", "/opt/homebrew"] {
        add(SystemContainer, p);
    }
    add(Credentials, "/Library/Keychains");
    add(Credentials, "/Library/Application Support/com.apple.TCC");
    add(SecuritySoftware, "/Library/SystemExtensions");
    add(InstallerState, "/Library/Updates");
    add(UsersRoot, "/Users");
    add(PublicProfile, "/Users/Shared");
    for p in ["/", "/System/Volumes/Data"] {
        add(VolumeRoot, p);
    }
    for p in ["/.Spotlight-V100", "/.fseventsd", "/.DocumentRevisions-V100"] {
        add(VolumeMetadata, p);
    }
    // `/usr` minus `/usr/local`: every other child is the operating system.
    for child in children("/usr") {
        if child.file_name() != Some(OsStr::new("local")) {
            out.push((SystemRoot, child));
        }
    }
    for volume in children("/Volumes") {
        for (class, sub) in [(VolumeMetadata, ".Spotlight-V100"), (VolumeMetadata, ".fseventsd"), (VolumeMetadata, ".DocumentRevisions-V100"), (RecycleBinRoot, ".Trashes")] {
            out.push((class, volume.join(sub)));
        }
        out.push((VolumeRoot, volume));
    }
    let mut profiles: Vec<PathBuf> = children("/Users")
        .into_iter()
        .filter(|p| p.file_name().is_some_and(|n| n != "Shared" && !n.as_bytes().starts_with(b".")) && p.is_dir())
        .collect();
    profiles.extend(home());
    for profile in profiles {
        let library = profile.join("Library");
        out.push((Credentials, library.join("Keychains")));
        out.push((Credentials, library.join("Application Support").join("com.apple.TCC")));
        out.push((SystemContainer, library));
        out.push((RecycleBinRoot, profile.join(".Trash")));
        out.push((UserProfileRoot, profile));
    }
    if let Some(store) = crate::paths::default_store_dir() {
        out.push((ToolStore, store));
    }
    out
}

/// Hard-deny roots of this machine, resolved by identity. A root that is itself a symlink also
/// contributes its target's identity, so neither spelling escapes.
pub fn hard_deny_roots() -> Vec<HardDenyRoot> {
    let mut ids = Identifier::default();
    let mut out = Vec::new();
    for (class, path) in candidates() {
        let Ok(id) = ids.identity(&path) else { continue };
        let display = path.display().to_string();
        if let Some(t) = is_symlink(&path).then(|| ids.resolve(&path).ok()).flatten() {
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
        RawPath::Unix(bytes) => protect::check_path(path, &identity_chain(bytes), hard),
        RawPath::Windows(_) => ProtectionDecision::Denied { layer: protect::DenyLayer::HardDeny, reason: "not a macOS path".into() },
    }
}

/// Identity facts for one absolute path; see the module docs. Failures are recorded as `None` /
/// `Err` so the core check fails closed.
pub fn identity_chain(path: &[u8]) -> IdentityChain {
    let mut chain = IdentityChain::default();
    if path.first() != Some(&b'/') || path.contains(&0) {
        return chain;
    }
    let mut ids = Identifier::default();
    let mut prefixes = vec![PathBuf::from("/")];
    for comp in path.split(|b| *b == b'/').filter(|c| !c.is_empty()) {
        let next = prefixes[prefixes.len() - 1].join(OsStr::from_bytes(comp));
        prefixes.push(next);
    }
    let last = prefixes.len() - 1;
    for (i, p) in prefixes.iter().enumerate() {
        let id = ids.identity(p).ok();
        if i > 0 && is_symlink(p) {
            let link = raw(p).unwrap_or_else(|| RawPath::Unix(b"?".to_vec()));
            chain.links.push(LinkResolution { link, on_object: i == last, target: ids.resolve(p) });
        }
        if i == last {
            chain.object = id;
        } else {
            chain.ancestors.push(id);
        }
    }
    let object = &prefixes[last];
    chain.real_path = match (object.parent(), object.file_name()) {
        (Some(parent), Some(name)) => std::fs::canonicalize(parent).ok().and_then(|real| raw(&real.join(name))),
        _ => raw(object),
    };
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(p: &str) -> IdentityChain {
        identity_chain(p.as_bytes())
    }

    fn unix(p: &str) -> RawPath {
        RawPath::unix(p.as_bytes().to_vec()).unwrap()
    }

    #[test]
    fn system_roots_resolve_and_deny_by_identity() {
        let hard = hard_deny_set();
        assert!(hard.roots().any(|r| r.display == "/System" && r.class == HardDenyClass::SystemRoot));
        assert!(hard.roots().any(|r| r.display == "/" && r.class == HardDenyClass::VolumeRoot));
        assert!(protect::check_identity(&chain("/System/Library"), &hard).is_denied());
        assert!(protect::check_identity(&chain("/usr/bin"), &hard).is_denied());
        // Case variant: identity, not text, decides on a case-insensitive volume.
        assert!(protect::hard_deny_hit(&chain("/SYSTEM/Library"), &hard).is_some() || std::fs::symlink_metadata("/SYSTEM").is_err());
    }

    #[test]
    fn a_symlink_into_a_system_directory_is_denied_and_usr_local_is_rule_territory() {
        let hard = hard_deny_set();
        let dir = std::env::temp_dir().join(format!("cm-hd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let link = dir.join("sys");
        std::os::unix::fs::symlink("/System/Library", &link).unwrap();
        let text = link.to_str().unwrap();
        assert!(protect::check_path(&unix(text), &chain(text), &hard).is_denied());
        let plain = dir.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        let text = plain.to_str().unwrap();
        assert!(protect::check_identity(&chain(text), &hard).is_allowed(), "{:?}", protect::check_identity(&chain(text), &hard));
        std::fs::remove_dir_all(&dir).ok();
        assert!(!hard.roots().any(|r| r.display == "/usr/local" && r.class.covers_descendants()));
    }

    #[test]
    fn unresolvable_paths_fail_closed() {
        let hard = hard_deny_set();
        assert!(protect::check_identity(&chain("relative/path"), &hard).is_denied());
        assert!(protect::check_identity(&chain("/no/such/cm-path"), &hard).is_denied());
    }
}
