//! Read-only Windows application inventory (MVP 11.1/11.3, APP-01..04).
//!
//! Sources:
//! - `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall` in the 64- and 32-bit views,
//!   and the same path under HKCU (shared between views, read once).
//! - Packaged apps registered for the current user under
//!   `HKCU\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages`.
//!
//! Safety boundary: every key is opened with `KEY_READ` (plus a WOW64 view flag) and only
//! descriptive values are queried. Uninstall/modify command lines are never read, parsed or
//! executed, nothing is written to the registry, and no installer is started. Install
//! locations are only opened for attributes (`identity_of`), never listed or read.

use std::os::windows::ffi::OsStrExt;

use cm_core::apps::{
    AppInstance, AppSource, Evidence, EvidenceKind, InventoryIssue, LocationIdentity, ProtectedReason, RegistryHive, RegistryView, clean_text,
    merge_instances, protected_reason,
};
use cm_core::path::RawPath;
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY, REG_DWORD, REG_EXPAND_SZ, REG_SZ, RegCloseKey,
    RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW,
};

const UNINSTALL_PATH: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
const PACKAGES_PATH: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages";
/// Publisher ID of packages signed as part of Windows itself.
const WINDOWS_PUBLISHER_ID: &str = "cw5n1h2txyewy";
const ERROR_ACCESS_DENIED: u32 = 5;

/// The only access mask this module ever requests. `view` adds a WOW64 view flag, nothing else.
fn read_access(view: u32) -> u32 {
    debug_assert!(matches!(view, 0 | KEY_WOW64_64KEY | KEY_WOW64_32KEY));
    KEY_READ | (view & (KEY_WOW64_64KEY | KEY_WOW64_32KEY))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn hive_text(h: RegistryHive) -> &'static str {
    match h {
        RegistryHive::Hklm => "HKLM",
        RegistryHive::Hkcu => "HKCU",
    }
}

fn view_text(v: RegistryView) -> &'static str {
    match v {
        RegistryView::View64 => "64-bit view",
        RegistryView::View32 => "32-bit view",
        RegistryView::Shared => "shared view",
    }
}

/// A registry key opened read-only; closed on drop.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful RegOpenKeyExW and is closed once.
        unsafe { RegCloseKey(self.0) };
    }
}

struct RegString {
    units: Vec<u16>,
    expand: bool,
}

impl Key {
    fn open(parent: HKEY, path: &[u16], view: u32) -> Result<Key, u32> {
        let mut z = path.to_vec();
        if z.last() != Some(&0) {
            z.push(0);
        }
        let mut out: HKEY = std::ptr::null_mut();
        // SAFETY: NUL-terminated subkey, valid out-pointer, read-only access mask.
        let rc = unsafe { RegOpenKeyExW(parent, z.as_ptr(), 0, read_access(view), &mut out) };
        if rc == ERROR_SUCCESS { Ok(Key(out)) } else { Err(rc) }
    }

    fn child(&self, name: &[u16], view: u32) -> Result<Key, u32> {
        Key::open(self.0, name, view)
    }

    /// Sub-key names (UTF-16, no terminator).
    fn subkeys(&self) -> Result<Vec<Vec<u16>>, u32> {
        let mut out = Vec::new();
        let mut buf = vec![0u16; 512];
        for index in 0u32.. {
            let mut len = buf.len() as u32;
            // SAFETY: buffer of `len` units; class and time outputs are not requested.
            let rc = unsafe {
                RegEnumKeyExW(self.0, index, buf.as_mut_ptr(), &mut len, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
            };
            match rc {
                ERROR_SUCCESS => out.push(buf[..len as usize].to_vec()),
                ERROR_NO_MORE_ITEMS => break,
                ERROR_MORE_DATA => {
                    // Key names are limited to 255 characters; grow once and retry.
                    buf = vec![0u16; 32_768];
                    let mut len = buf.len() as u32;
                    // SAFETY: as above with the larger buffer.
                    let rc = unsafe {
                        RegEnumKeyExW(self.0, index, buf.as_mut_ptr(), &mut len, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
                    };
                    if rc != ERROR_SUCCESS {
                        return Err(rc);
                    }
                    out.push(buf[..len as usize].to_vec());
                }
                other => return Err(other),
            }
        }
        Ok(out)
    }

    fn raw_value(&self, name: &str) -> Option<(u32, Vec<u8>)> {
        let wname = wide(name);
        let mut size = 0u32;
        let mut kind = 0u32;
        for _ in 0..4 {
            // SAFETY: size query with a null data pointer.
            let rc = unsafe { RegQueryValueExW(self.0, wname.as_ptr(), std::ptr::null(), &mut kind, std::ptr::null_mut(), &mut size) };
            if rc != ERROR_SUCCESS {
                return None;
            }
            let mut data = vec![0u8; size as usize + 2];
            let mut got = data.len() as u32;
            // SAFETY: data buffer of `got` bytes.
            let rc = unsafe { RegQueryValueExW(self.0, wname.as_ptr(), std::ptr::null(), &mut kind, data.as_mut_ptr(), &mut got) };
            match rc {
                ERROR_SUCCESS => {
                    data.truncate(got as usize);
                    return Some((kind, data));
                }
                ERROR_MORE_DATA => size = got,
                _ => return None,
            }
        }
        None
    }

    fn string(&self, name: &str) -> Option<RegString> {
        let (kind, data) = self.raw_value(name)?;
        if kind != REG_SZ && kind != REG_EXPAND_SZ {
            return None;
        }
        let mut units: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        if let Some(end) = units.iter().position(|&u| u == 0) {
            units.truncate(end);
        }
        Some(RegString { units, expand: kind == REG_EXPAND_SZ })
    }

    fn text(&self, name: &str) -> Option<String> {
        self.string(name).and_then(|s| clean_text(&String::from_utf16_lossy(&s.units)))
    }

    fn dword(&self, name: &str) -> Option<u32> {
        let (kind, data) = self.raw_value(name)?;
        (kind == REG_DWORD && data.len() >= 4).then(|| u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
    }
}

/// Expand `%VAR%` references from the process environment; `None` if any is undefined.
fn expand_env(units: &[u16]) -> Option<Vec<u16>> {
    let pct = u16::from(b'%');
    let mut out = Vec::with_capacity(units.len());
    let mut rest = units;
    while let Some(start) = rest.iter().position(|&u| u == pct) {
        out.extend_from_slice(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after.iter().position(|&u| u == pct)?;
        let name = String::from_utf16(&after[..end]).ok()?;
        let value = std::env::var_os(&name)?;
        out.extend(value.encode_wide());
        rest = &after[end + 1..];
    }
    out.extend_from_slice(rest);
    Some(out)
}

fn is_space(u: u16) -> bool {
    matches!(u, 0x20 | 0x09)
}

/// Normalize a recorded location: trim blanks and surrounding quotes, expand environment
/// references, drop trailing separators (keeping `X:\`), and require an absolute path.
fn normalize_location(s: &RegString) -> Result<Vec<u16>, &'static str> {
    let mut u: &[u16] = &s.units;
    while u.first().is_some_and(|&c| is_space(c)) {
        u = &u[1..];
    }
    while u.last().is_some_and(|&c| is_space(c)) {
        u = &u[..u.len() - 1];
    }
    let quote = u16::from(b'"');
    if u.len() >= 2 && u[0] == quote && u[u.len() - 1] == quote {
        u = &u[1..u.len() - 1];
    }
    let mut units = if s.expand || u.contains(&u16::from(b'%')) { expand_env(u).ok_or("undefined environment reference")? } else { u.to_vec() };
    let bs = u16::from(b'\\');
    let fs = u16::from(b'/');
    while units.len() > 3 && matches!(units.last(), Some(&c) if c == bs || c == fs) {
        units.pop();
    }
    if units.is_empty() {
        return Err("empty");
    }
    let drive = units.len() >= 3 && units[0] < 0x80 && (units[0] as u8).is_ascii_alphabetic() && units[1] == u16::from(b':') && (units[2] == bs || units[2] == fs);
    let unc = units.len() >= 3 && units[0] == bs && units[1] == bs;
    if !(drive || unc) {
        return Err("not an absolute path");
    }
    Ok(units)
}

/// Attach a location and its identity; an unresolvable location stays unknown with evidence.
fn resolve_location(units: Vec<u16>, evidence: &mut Vec<Evidence>) -> (Option<RawPath>, Option<LocationIdentity>) {
    let Ok(path) = RawPath::windows(units.clone()) else {
        evidence.push(Evidence::new(EvidenceKind::LocationUnresolved, "recorded location is not a valid path"));
        return (None, None);
    };
    let os = String::from_utf16_lossy(&units);
    let is_dir = std::fs::symlink_metadata(&os).map(|m| m.is_dir()).unwrap_or(false);
    if !is_dir {
        evidence.push(Evidence::new(EvidenceKind::LocationUnresolved, "recorded location is missing or not a directory"));
        return (Some(path), None);
    }
    match super::directory::identity_of(&units) {
        Ok((volume_id, file_id)) => {
            evidence.push(Evidence::new(EvidenceKind::LocationIdentity, format!("{volume_id} {file_id}")));
            (Some(path), Some(LocationIdentity { volume_id, file_id }))
        }
        Err(e) => {
            evidence.push(Evidence::new(EvidenceKind::LocationUnresolved, format!("identity unavailable: {e}")));
            (Some(path), None)
        }
    }
}

fn is_product_code(key: &str) -> bool {
    let b = key.as_bytes();
    b.len() == 38
        && b[0] == b'{'
        && b[37] == b'}'
        && b[1..37].iter().enumerate().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { *c == b'-' } else { c.is_ascii_hexdigit() })
}

/// One uninstall entry, or `None` for updates and nameless entries.
fn uninstall_entry(key: &Key, key_name: &[u16], hive: RegistryHive, view: RegistryView) -> Option<AppInstance> {
    let name = key.text("DisplayName")?;
    if key.string("ParentKeyName").is_some_and(|s| !s.units.is_empty()) {
        return None;
    }
    if let Some(release) = key.text("ReleaseType") {
        let r = release.to_ascii_lowercase();
        if r.contains("update") || r.contains("hotfix") || r.contains("service pack") {
            return None;
        }
    }
    let key_text = String::from_utf16_lossy(key_name);
    let windows_installer = key.dword("WindowsInstaller") == Some(1);
    let system_component = key.dword("SystemComponent") == Some(1);
    let source = if windows_installer && is_product_code(&key_text) {
        AppSource::Msi { hive, view, product_code: key_text.to_ascii_uppercase() }
    } else {
        AppSource::RegistryUninstall { hive, view, key: key_text.clone() }
    };

    let mut evidence = vec![
        Evidence::new(EvidenceKind::RegistryKey, format!("{}\\{UNINSTALL_PATH}\\{key_text} ({})", hive_text(hive), view_text(view))),
        Evidence::new(EvidenceKind::DisplayName, name.clone()),
    ];
    if windows_installer {
        evidence.push(Evidence::new(EvidenceKind::WindowsInstaller, "WindowsInstaller=1"));
    }
    if system_component {
        evidence.push(Evidence::new(EvidenceKind::SystemComponent, "SystemComponent=1"));
    }
    let (location, identity) = match key.string("InstallLocation") {
        None => (None, None),
        Some(s) if s.units.iter().all(|&u| is_space(u)) => (None, None),
        Some(s) => match normalize_location(&s) {
            Ok(units) => {
                evidence.push(Evidence::new(EvidenceKind::InstallLocation, String::from_utf16_lossy(&units)));
                resolve_location(units, &mut evidence)
            }
            Err(why) => {
                evidence.push(Evidence::new(EvidenceKind::LocationUnresolved, format!("InstallLocation {why}")));
                (None, None)
            }
        },
    };

    let mut app = AppInstance::new(name, source, location);
    app.location_identity = identity;
    app.version = key.text("DisplayVersion");
    app.publisher = key.text("Publisher");
    app.protected_class = protected_reason(&app.name, app.publisher.as_deref(), system_component);
    if let Some(reason) = app.protected_class {
        evidence.push(Evidence::new(EvidenceKind::ProtectedClass, format!("{reason:?}")));
    }
    app.evidence = evidence;
    Some(app)
}

fn read_uninstall(hive: RegistryHive, view: RegistryView, apps: &mut Vec<AppInstance>, issues: &mut Vec<InventoryIssue>) {
    let (root, flag) = match (hive, view) {
        (RegistryHive::Hklm, RegistryView::View64) => (HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY),
        (RegistryHive::Hklm, _) => (HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY),
        (RegistryHive::Hkcu, _) => (HKEY_CURRENT_USER, 0),
    };
    let source = format!("{}\\{UNINSTALL_PATH} ({})", hive_text(hive), view_text(view));
    let issue = |code: &str, message: String| InventoryIssue { code: code.into(), source: source.clone(), message };
    let parent = match Key::open(root, &wide(UNINSTALL_PATH), flag) {
        Ok(k) => k,
        Err(ERROR_FILE_NOT_FOUND) => return,
        Err(rc) => return issues.push(issue("registry_open_failed", format!("cannot open uninstall key (error {rc})"))),
    };
    let names = match parent.subkeys() {
        Ok(n) => n,
        Err(rc) => return issues.push(issue("registry_enum_failed", format!("cannot list uninstall entries (error {rc})"))),
    };
    let mut denied = 0u32;
    for name in names {
        match parent.child(&name, flag) {
            Ok(k) => apps.extend(uninstall_entry(&k, &name, hive, view)),
            Err(ERROR_FILE_NOT_FOUND) => {} // removed while listing
            Err(ERROR_ACCESS_DENIED) => denied += 1,
            Err(rc) => issues.push(issue("registry_entry_unreadable", format!("an uninstall entry could not be opened (error {rc})"))),
        }
    }
    if denied > 0 {
        issues.push(issue("permission_denied", format!("{denied} uninstall entries could not be read")));
    }
}

/// Split `Name_Version_Arch_ResourceId_PublisherId`.
fn split_full_name(full: &str) -> Option<(&str, &str, &str)> {
    let parts: Vec<&str> = full.split('_').collect();
    if parts.len() != 5 || parts[0].is_empty() || parts[4].is_empty() {
        return None;
    }
    Some((parts[0], parts[1], parts[4]))
}

fn package_entry(key: &Key, full_name: &str) -> Option<AppInstance> {
    let (pkg_name, version, publisher_id) = split_full_name(full_name)?;
    let family = format!("{pkg_name}_{publisher_id}");
    let mut evidence = vec![
        Evidence::new(EvidenceKind::RegistryKey, format!("HKCU\\{PACKAGES_PATH}\\{full_name}")),
        Evidence::new(EvidenceKind::PackageFullName, full_name.to_owned()),
    ];
    let display = key.text("DisplayName");
    let name = match display {
        Some(d) if !d.starts_with('@') && !d.starts_with("ms-resource:") => {
            evidence.push(Evidence::new(EvidenceKind::DisplayName, d.clone()));
            d
        }
        Some(_) => {
            evidence.push(Evidence::new(EvidenceKind::IndirectDisplayName, "display name is a resource reference; package name shown"));
            pkg_name.to_owned()
        }
        None => pkg_name.to_owned(),
    };
    let framework = key.dword("Framework") == Some(1);
    if framework {
        evidence.push(Evidence::new(EvidenceKind::Framework, "Framework=1"));
    }
    let mut under_windows = false;
    let (location, identity) = match key.string("PackageRootFolder") {
        Some(s) if !s.units.is_empty() => match normalize_location(&s) {
            Ok(units) => {
                let lower = String::from_utf16_lossy(&units).to_ascii_lowercase();
                under_windows = lower.contains(r":\windows\");
                evidence.push(Evidence::new(EvidenceKind::PackageRoot, String::from_utf16_lossy(&units)));
                resolve_location(units, &mut evidence)
            }
            Err(why) => {
                evidence.push(Evidence::new(EvidenceKind::LocationUnresolved, format!("PackageRootFolder {why}")));
                (None, None)
            }
        },
        _ => (None, None),
    };
    let mut app = AppInstance::new(name, AppSource::Msix { package_family: family, full_name: full_name.to_owned() }, location);
    app.location_identity = identity;
    app.version = clean_text(version);
    app.protected_class = if framework {
        Some(ProtectedReason::SharedRuntime)
    } else if publisher_id == WINDOWS_PUBLISHER_ID || under_windows {
        Some(ProtectedReason::OsPackage)
    } else {
        protected_reason(&app.name, None, false).or_else(|| protected_reason(pkg_name, None, false))
    };
    if let Some(reason) = app.protected_class {
        evidence.push(Evidence::new(EvidenceKind::ProtectedClass, format!("{reason:?}")));
    }
    app.evidence = evidence;
    Some(app)
}

fn read_packages(apps: &mut Vec<AppInstance>, issues: &mut Vec<InventoryIssue>) {
    let source = format!("HKCU\\{PACKAGES_PATH}");
    let issue = |code: &str, message: String| InventoryIssue { code: code.into(), source: source.clone(), message };
    let parent = match Key::open(HKEY_CURRENT_USER, &wide(PACKAGES_PATH), 0) {
        Ok(k) => k,
        Err(ERROR_FILE_NOT_FOUND) => return,
        Err(rc) => return issues.push(issue("registry_open_failed", format!("cannot open package repository (error {rc})"))),
    };
    let names = match parent.subkeys() {
        Ok(n) => n,
        Err(rc) => return issues.push(issue("registry_enum_failed", format!("cannot list packages (error {rc})"))),
    };
    let mut malformed = 0u32;
    for name in names {
        let full = String::from_utf16_lossy(&name);
        match parent.child(&name, 0) {
            Ok(k) => match package_entry(&k, &full) {
                Some(app) => apps.push(app),
                None => malformed += 1,
            },
            Err(ERROR_FILE_NOT_FOUND) => {}
            Err(rc) => issues.push(issue("registry_entry_unreadable", format!("a package entry could not be opened (error {rc})"))),
        }
    }
    if malformed > 0 {
        issues.push(issue("unrecognized_package_entry", format!("{malformed} package entries did not have a package full name")));
    }
}

/// Read-only inventory of installed applications on this machine. Sources that fail are
/// reported as issues (incomplete, not empty); nothing is executed or written.
pub fn inventory() -> (Vec<AppInstance>, Vec<InventoryIssue>) {
    let mut apps = Vec::new();
    let mut issues = Vec::new();
    read_uninstall(RegistryHive::Hklm, RegistryView::View64, &mut apps, &mut issues);
    read_uninstall(RegistryHive::Hklm, RegistryView::View32, &mut apps, &mut issues);
    read_uninstall(RegistryHive::Hkcu, RegistryView::Shared, &mut apps, &mut issues);
    read_packages(&mut apps, &mut issues);
    (merge_instances(apps), issues)
}

#[cfg(test)]
mod tests;
