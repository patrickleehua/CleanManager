//! Restricted root resolvers (ANA-01): each resolver kind maps to a fixed, documented location
//! derived from the OS environment and a built-in owner table. There is no way for a rule to
//! supply its own path or script. Resolved roots carry file identity so analysis matches them by
//! identity, not by path text (8.3 names and case differences do not matter).
//!
//! Also the installed-version facts the tested registry is checked against (CAT-03): versions
//! come from installer records (uninstall registry entries, the Node.js install key) or the OS,
//! never from files a rule names.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use cm_core::analysis::ResolvedRoot;
use cm_core::rules::{Rule, RootResolver};

use super::directory::identity_of;

/// Base locations; defaults come from the environment, overridable for controlled fixtures.
#[derive(Debug, Clone)]
pub struct ResolverEnv {
    pub local_app_data: Option<PathBuf>,
    pub temp: Option<PathBuf>,
    pub app_data: Option<PathBuf>,
    pub user_profile: Option<PathBuf>,
    pub downloads: Option<PathBuf>,
    /// Package-manager cache overrides the tools themselves honour (`GOCACHE`, `CARGO_HOME`...).
    pub tool_env: BTreeMap<String, PathBuf>,
    /// A removable volume root the user explicitly selected (CAT-08); never set implicitly.
    pub removable_volume: Option<PathBuf>,
}

impl ResolverEnv {
    pub fn from_os() -> Self {
        let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let tool_env = TOOL_CACHES.iter().filter_map(|(_, env, _, _)| env.and_then(|e| var(e).map(|v| (e.to_string(), v)))).collect();
        ResolverEnv {
            local_app_data: var("LOCALAPPDATA"),
            temp: var("TEMP"),
            app_data: var("APPDATA"),
            user_profile: var("USERPROFILE"),
            downloads: known_downloads().or_else(|| var("USERPROFILE").map(|p| p.join("Downloads"))),
            tool_env,
            removable_volume: None,
        }
    }

    fn base(&self, b: Base) -> Option<&PathBuf> {
        match b {
            Base::LocalAppData => self.local_app_data.as_ref(),
            Base::AppData => self.app_data.as_ref(),
            Base::UserProfile => self.user_profile.as_ref(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Base {
    LocalAppData,
    AppData,
    UserProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Profiles {
    /// Chromium: `Default` and `Profile N` directories.
    Chromium,
    /// Firefox: every profile directory under `Profiles`.
    Any,
}

/// Built-in owner table: browser owner id → user-data directory; each profile is a root.
const BROWSERS: &[(&str, Base, &[&str], Profiles)] = &[
    ("microsoft.edge", Base::LocalAppData, &["Microsoft", "Edge", "User Data"], Profiles::Chromium),
    ("google.chrome", Base::LocalAppData, &["Google", "Chrome", "User Data"], Profiles::Chromium),
    // Firefox keeps its disposable cache under LOCALAPPDATA, apart from the roaming profile.
    ("mozilla.firefox", Base::LocalAppData, &["Mozilla", "Firefox", "Profiles"], Profiles::Any),
];

/// Application data directories (`application_cache`, `app_support`).
const APP_DIRS: &[(RootResolver, &str, Base, &[&str])] = &[
    (RootResolver::ApplicationCache, "microsoft.vscode", Base::AppData, &["Code"]),
    (RootResolver::AppSupport, "microsoft.wsl", Base::LocalAppData, &["wsl"]),
];

/// Fixed per-user system locations.
const SYSTEM_DIRS: &[(RootResolver, &str, Base, &[&str])] = &[
    (RootResolver::CrashReports, "windows.wer", Base::LocalAppData, &["Microsoft", "Windows", "WER"]),
    (RootResolver::CrashReports, "windows.crash-dumps", Base::LocalAppData, &["CrashDumps"]),
    (RootResolver::UserCache, "windows.directx", Base::LocalAppData, &["D3DSCache"]),
];

/// Package-manager caches: owner → (override variable the tool honours, default base, parts).
const TOOL_CACHES: &[(&str, Option<&str>, Base, &[&str])] = &[
    ("npm", Some("npm_config_cache"), Base::LocalAppData, &["npm-cache"]),
    ("go", Some("GOCACHE"), Base::LocalAppData, &["go-build"]),
    ("cargo", Some("CARGO_HOME"), Base::UserProfile, &[".cargo"]),
    ("pip", Some("PIP_CACHE_DIR"), Base::LocalAppData, &["pip", "cache"]),
    ("uv", Some("UV_CACHE_DIR"), Base::LocalAppData, &["uv", "cache"]),
    ("nuget", Some("NUGET_HTTP_CACHE_PATH"), Base::LocalAppData, &["NuGet", "v3-cache"]),
];

fn join(base: &Path, parts: &[&str]) -> PathBuf {
    parts.iter().fold(base.to_path_buf(), |p, c| p.join(c))
}

fn profiles(user_data: &Path, style: Profiles) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(user_data) else { return vec![] };
    let mut out: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink()))
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            style == Profiles::Any || n == "Default" || n.starts_with("Profile ")
        })
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

fn known_downloads() -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Downloads, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    // SAFETY: documented known-folder query; the returned string is freed once with CoTaskMemFree.
    let p = unsafe { SHGetKnownFolderPath(&FOLDERID_Downloads, KF_FLAG_DEFAULT, None) }.ok()?;
    // SAFETY: SHGetKnownFolderPath returned a valid NUL-terminated string.
    let s = unsafe { p.to_string() }.ok();
    // SAFETY: allocated by the shell with CoTaskMemAlloc.
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s.map(PathBuf::from)
}

/// Candidate base paths for a rule (before identity resolution).
pub fn base_paths(rule: &Rule, env: &ResolverEnv) -> Vec<PathBuf> {
    let owner = rule.owner.id.as_str();
    let fixed = |table: &[(RootResolver, &str, Base, &[&str])]| -> Vec<PathBuf> {
        table
            .iter()
            .filter(|(r, id, _, _)| *r == rule.root_resolver && *id == owner)
            .filter_map(|(_, _, base, parts)| env.base(*base).map(|b| join(b, parts)))
            .collect()
    };
    match rule.root_resolver {
        RootResolver::UserTemp => env.temp.iter().cloned().collect(),
        RootResolver::BrowserProfile => BROWSERS
            .iter()
            .filter(|(id, ..)| *id == owner)
            .filter_map(|(_, base, parts, style)| env.base(*base).map(|b| profiles(&join(b, parts), *style)))
            .flatten()
            .collect(),
        RootResolver::ApplicationCache | RootResolver::AppSupport => fixed(APP_DIRS),
        RootResolver::CrashReports | RootResolver::UserCache => fixed(SYSTEM_DIRS),
        RootResolver::PackageManagerCache => TOOL_CACHES
            .iter()
            .filter(|(id, ..)| *id == owner)
            .filter_map(|(_, var, base, parts)| var.and_then(|v| env.tool_env.get(v).cloned()).or_else(|| env.base(*base).map(|b| join(b, parts))))
            .collect(),
        RootResolver::InstallerLocation => env.downloads.iter().cloned().collect(),
        RootResolver::RemovableVolumeRoot => env.removable_volume.iter().cloned().collect(),
        // Other resolver kinds have no Windows mapping yet: the rule stays inapplicable.
        _ => vec![],
    }
}

/// Resolve every enabled rule's roots to identities. Missing locations are simply absent.
pub fn resolve(rules: &[Rule], env: &ResolverEnv) -> Vec<ResolvedRoot> {
    let mut out = Vec::new();
    for rule in rules {
        for path in base_paths(rule, env) {
            let wide: Vec<u16> = OsString::from(path.as_os_str()).encode_wide().collect();
            if let (Ok((volume_id, file_id)), Ok(raw)) = (identity_of(&wide), cm_core::path::RawPath::windows(wide.clone())) {
                out.push(ResolvedRoot { rule_id: rule.rule_id.clone(), path: raw, volume_id, file_id });
            }
        }
    }
    out
}

/// Accept a user-selected volume for removable-metadata cleanup (CAT-08): a drive root (`E:\`)
/// of a removable drive that is not the system drive. Anything else is refused.
pub fn removable_volume_root(arg: &Path) -> Result<PathBuf, String> {
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    const DRIVE_REMOVABLE: u32 = 2;
    let s = arg.to_string_lossy();
    let b = s.as_bytes();
    let letter = match b {
        [l, b':'] | [l, b':', b'\\'] if l.is_ascii_alphabetic() => l.to_ascii_uppercase() as char,
        _ => return Err(format!("{s} is not a drive root such as E:\\")),
    };
    let root = format!("{letter}:\\");
    let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    if system.trim_end_matches('\\').eq_ignore_ascii_case(&root[..2]) {
        return Err(format!("{root} is the system drive"));
    }
    let z: Vec<u16> = root.encode_utf16().chain([0]).collect();
    // SAFETY: NUL-terminated drive root string.
    let kind = unsafe { GetDriveTypeW(z.as_ptr()) };
    if kind != DRIVE_REMOVABLE {
        return Err(format!("{root} is not a removable drive (drive type {kind})"));
    }
    Ok(PathBuf::from(root))
}

// ---------------------------------------------------------------------------------------------
// Installed versions (CAT-03)

mod reg {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, KEY_ENUMERATE_SUB_KEYS, KEY_QUERY_VALUE, KEY_WOW64_32KEY, KEY_WOW64_64KEY, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegCloseKey,
        RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
    };
    pub use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    pub const VIEW_64: u32 = KEY_WOW64_64KEY;
    pub const VIEW_32: u32 = KEY_WOW64_32KEY;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    pub struct Key(HKEY);

    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: opened by RegOpenKeyExW and closed once.
            unsafe { RegCloseKey(self.0) };
        }
    }

    impl Key {
        pub fn open(parent: HKEY, path: &str, view: u32) -> Option<Key> {
            let mut out: HKEY = std::ptr::null_mut();
            // SAFETY: NUL-terminated path; read-only access.
            let rc = unsafe { RegOpenKeyExW(parent, wide(path).as_ptr(), 0, KEY_QUERY_VALUE | KEY_ENUMERATE_SUB_KEYS | view, &mut out) };
            (rc == ERROR_SUCCESS).then_some(Key(out))
        }

        pub fn subkeys(&self) -> Vec<String> {
            let mut out = Vec::new();
            for index in 0..10_000u32 {
                let mut buf = [0u16; 256];
                let mut len = buf.len() as u32;
                // SAFETY: valid key handle and buffer of `len` units.
                let rc = unsafe {
                    RegEnumKeyExW(self.0, index, buf.as_mut_ptr(), &mut len, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
                };
                if rc != ERROR_SUCCESS {
                    break;
                }
                out.push(String::from_utf16_lossy(&buf[..len as usize]));
            }
            out
        }

        pub fn string(&self, sub: Option<&str>, name: &str) -> Option<String> {
            let sub = sub.map(wide);
            let name = wide(name);
            let mut size = 0u32;
            let subp = sub.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
            // SAFETY: size query with a null buffer.
            let rc = unsafe { RegGetValueW(self.0, subp, name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut size) };
            if rc != ERROR_SUCCESS || size == 0 || size > 64 * 1024 {
                return None;
            }
            let mut buf = vec![0u16; (size as usize).div_ceil(2)];
            let mut got = (buf.len() * 2) as u32;
            // SAFETY: buffer of `got` bytes; RegGetValueW NUL-terminates REG_SZ.
            let rc = unsafe { RegGetValueW(self.0, subp, name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut got) };
            if rc != ERROR_SUCCESS {
                return None;
            }
            let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..n]))
        }

        pub fn dword(&self, name: &str) -> Option<u32> {
            let mut v = 0u32;
            let mut size = 4u32;
            // SAFETY: four-byte buffer for a REG_DWORD.
            let rc = unsafe { RegGetValueW(self.0, std::ptr::null(), wide(name).as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&mut v as *mut u32).cast(), &mut size) };
            (rc == ERROR_SUCCESS).then_some(v)
        }
    }
}

/// Windows version as `major.minor.build` from the OS record.
pub fn os_version() -> Option<String> {
    let k = reg::Key::open(reg::HKEY_LOCAL_MACHINE, "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion", reg::VIEW_64)?;
    Some(format!("{}.{}.{}", k.dword("CurrentMajorVersionNumber")?, k.dword("CurrentMinorVersionNumber")?, k.string(None, "CurrentBuildNumber")?))
}

/// Node.js install directory recorded by its MSI (HKLM, admin-writable), else the default.
pub fn nodejs_install_dir() -> Option<PathBuf> {
    let recorded = reg::Key::open(reg::HKEY_LOCAL_MACHINE, "SOFTWARE\\Node.js", reg::VIEW_64).and_then(|k| k.string(None, "InstallPath"));
    recorded.map(PathBuf::from).or_else(|| std::env::var_os("ProgramFiles").map(|p| PathBuf::from(p).join("nodejs"))).filter(|p| p.is_absolute())
}

#[derive(Debug, Clone, Copy)]
enum NameMatch {
    Exact(&'static str),
    Prefix(&'static str),
}

/// Owner id → uninstall-record display name and publisher fragment.
const OWNER_APPS: &[(&str, NameMatch, &str)] = &[
    ("google.chrome", NameMatch::Exact("Google Chrome"), "Google"),
    ("microsoft.edge", NameMatch::Exact("Microsoft Edge"), "Microsoft"),
    ("microsoft.vscode", NameMatch::Prefix("Microsoft Visual Studio Code"), "Microsoft"),
    ("mozilla.firefox", NameMatch::Prefix("Mozilla Firefox"), "Mozilla"),
    ("go", NameMatch::Prefix("Go Programming Language"), "Go"),
];

/// Uninstall records (display name, version, publisher) from HKLM (both views) and HKCU.
fn uninstall_records() -> Vec<(String, String, String)> {
    const PATH: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
    let mut out = Vec::new();
    for (hive, view) in [(reg::HKEY_LOCAL_MACHINE, reg::VIEW_64), (reg::HKEY_LOCAL_MACHINE, reg::VIEW_32), (reg::HKEY_CURRENT_USER, reg::VIEW_64)] {
        let Some(k) = reg::Key::open(hive, PATH, view) else { continue };
        for sub in k.subkeys() {
            if let (Some(name), Some(version)) = (k.string(Some(&sub), "DisplayName"), k.string(Some(&sub), "DisplayVersion")) {
                out.push((name.trim().to_owned(), version.trim().to_owned(), k.string(Some(&sub), "Publisher").unwrap_or_default()));
            }
        }
    }
    out
}

/// npm's own package version (`"version": "x.y.z"` in its package.json), digits and dots only.
fn npm_version() -> Option<String> {
    let pkg = nodejs_install_dir()?.join("node_modules").join("npm").join("package.json");
    let text = std::fs::read_to_string(pkg).ok()?;
    let after = text.split_once("\"version\"")?.1.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    let v = &after[..after.find('"')?];
    cm_core::catalog::parse_version(v).map(|_| v.to_owned())
}

/// Installed version per rule owner. An owner whose installs disagree, or that has no installer
/// record, is absent (unknown), which keeps its rules at most review_only.
pub fn installed_versions(rules: &[Rule]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let owners: std::collections::BTreeSet<&str> = rules.iter().map(|r| r.owner.id.as_str()).collect();
    let records = if owners.iter().any(|o| OWNER_APPS.iter().any(|(id, ..)| id == o)) { uninstall_records() } else { vec![] };
    for owner in owners {
        let found = if owner == "windows" || owner.starts_with("windows.") {
            os_version()
        } else if owner == "npm" {
            npm_version()
        } else if let Some((_, m, publisher)) = OWNER_APPS.iter().find(|(id, ..)| *id == owner) {
            let named = |name: &str| match m {
                NameMatch::Exact(n) => name == *n,
                NameMatch::Prefix(n) => name.starts_with(n),
            };
            let mut versions: Vec<&str> = records.iter().filter(|(name, _, p)| p.contains(publisher) && named(name)).map(|(_, v, _)| v.as_str()).collect();
            versions.sort_unstable();
            versions.dedup();
            (versions.len() == 1).then(|| versions[0].to_owned())
        } else {
            None
        };
        if let Some(v) = found {
            out.insert(owner.to_owned(), v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_version_is_read_from_the_os_record() {
        let v = os_version().expect("CurrentVersion is readable");
        assert!(cm_core::catalog::parse_version(&v).is_some_and(|p| p.len() == 3 && p[0] >= 10), "{v}");
    }

    #[test]
    fn system_drive_and_non_roots_are_never_removable_volumes() {
        let sys = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        assert!(removable_volume_root(Path::new(&format!("{sys}\\"))).unwrap_err().contains("system drive"));
        for bad in ["E:\\folder", "\\\\server\\share", "relative", "E:x", "\\\\?\\E:\\"] {
            assert!(removable_volume_root(Path::new(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn resolvers_only_map_fixed_locations() {
        let env = ResolverEnv {
            local_app_data: Some(PathBuf::from("C:\\L")),
            temp: None,
            app_data: Some(PathBuf::from("C:\\R")),
            user_profile: Some(PathBuf::from("C:\\U")),
            downloads: Some(PathBuf::from("C:\\U\\Downloads")),
            tool_env: [("GOCACHE".to_string(), PathBuf::from("D:\\gocache"))].into(),
            removable_volume: None,
        };
        let r = |resolver, owner: &str| {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rules/windows/windows.user-temp.stale-files.json");
            let mut rule = Rule::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
            rule.root_resolver = resolver;
            rule.owner.id = owner.into();
            base_paths(&rule, &env)
        };
        assert_eq!(r(RootResolver::CrashReports, "windows.wer"), vec![PathBuf::from("C:\\L\\Microsoft\\Windows\\WER")]);
        assert_eq!(r(RootResolver::ApplicationCache, "microsoft.vscode"), vec![PathBuf::from("C:\\R\\Code")]);
        assert_eq!(r(RootResolver::PackageManagerCache, "go"), vec![PathBuf::from("D:\\gocache")], "the tool's own override wins");
        assert_eq!(r(RootResolver::PackageManagerCache, "npm"), vec![PathBuf::from("C:\\L\\npm-cache")]);
        assert_eq!(r(RootResolver::InstallerLocation, "windows"), vec![PathBuf::from("C:\\U\\Downloads")]);
        assert!(r(RootResolver::RemovableVolumeRoot, "apple.metadata").is_empty(), "no volume unless the user selected one");
        assert!(r(RootResolver::CrashReports, "unknown.owner").is_empty());
        assert!(r(RootResolver::SystemLogs, "windows").is_empty());
    }
}
