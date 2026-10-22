//! Windows application uninstall (parity UNI-01..10, tasks 9.1–9.8): records, verified uninstaller runs, process
//! quit requests, registry backups and identity-evidence leftover discovery.
//!
//! - Records come from the same uninstall keys and package repository as `apps list` (same instance ids), plus Scoop
//!   apps and npm-global AI CLIs. The uninstall command is read only from the instance's own registration and is
//!   never passed to a shell; the decision about it is `cm_core::uninstall::select_adapter`.
//! - Before an uninstaller runs: the record is re-read and must hash as planned, the program must be a regular local
//!   file with the planned SHA-256, and its Authenticode signer (embedded, or the Windows catalog for system files)
//!   must still be the planned one. Vendor uninstallers keep their own UI (no hidden window, no silent switches).
//! - Running apps get `WM_CLOSE` on their top-level windows and a bounded wait; nothing is ended unless a person
//!   confirmed it for that plan, and critical processes and CleanManager itself are never ended (UNI-02).
//! - Registry keys are only read here except in [`WindowsUninstall::delete_key`] / [`WindowsUninstall::restore_key`],
//!   which the shared executor calls after the backup is stored (UNI-06). Restores never overwrite a recreated key.
//! - Everything goes through [`UninstallEnv`], so tests act on a throwaway key under `HKCU\Software\CleanManagerTest\`
//!   and a scratch home folder; the real uninstall keys, apps and data are never touched by a test.

use std::collections::BTreeSet;
use std::os::windows::fs::MetadataExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use cm_core::apps::{AppInstance, AppSource, InventoryIssue, ProtectedReason, RegistryHive, RegistryView, clean_text, instance_id_of, protected_reason};
use cm_core::capability::{Capability, CapabilityState};
use cm_core::path::RawPath;
use cm_core::plan::v2::Hive;
use cm_core::startup::{RegValue, RegView};
use cm_core::uninstall::{
    AI_CLIS, AdapterChoice, AdapterEnv, AdapterKind, AppIdentity, Binding, BrokenReference, DirEntryInfo, Group, InstalledApp, KeyBackup, KeyTree, Leftover,
    LeftoverKind, RecordSource, Refusal, RegLocation, Rescan, RunningProcess, RunningState, UninstallActions, UninstallRecord, WingetEvidence, ai_child_group, classify_child,
    end_refusal, finish_leftover, is_microsoft_signer, match_name, object_id, orphan_leftovers, ownership_marker, path_within, publisher_matches, publisher_variants,
    registry_denied, select_adapter, sensitive_warnings, tool_instance_id,
};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HANDLE, HWND, LPARAM};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS, KEY_WOW64_32KEY, KEY_WOW64_64KEY, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegDeleteTreeW, RegOpenKeyExW, RegSetValueExW,
};

use super::startup::{RegKey, StartupEnv, TEST_ROOT_PREFIX};

const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
const APP_PATHS: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths";
const PACKAGES: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages";
const WINDOWS_PUBLISHER_ID: &str = "cw5n1h2txyewy";
/// winget's own package family (App Installer).
const APP_INSTALLER_PREFIX: &str = "microsoft.desktopappinstaller_";
const ATTR_REPARSE: u32 = 0x400;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Where records, data folders and processes are read from. Tests redirect the registry and the home folder.
#[derive(Debug, Clone)]
pub struct UninstallEnv {
    /// Empty: the real HKCU/HKLM. Otherwise a key under `HKCU\Software\CleanManagerTest\` holding `HKCU\…`/`HKLM\…` copies.
    pub registry_root: String,
    /// Replacement user home: `AppData\Roaming`, `AppData\Local`, `ProgramData`, `scoop` and the Start menu live below it.
    pub home: Option<PathBuf>,
    pub is_admin: bool,
    /// Bounded wait after the close request (UNI-02).
    pub quit_wait: Duration,
}

impl UninstallEnv {
    pub fn real() -> UninstallEnv {
        UninstallEnv {
            registry_root: String::new(), home: None, is_admin: super::startup_actions::is_elevated(),
            quit_wait: Duration::from_secs(cm_core::uninstall::QUIT_WAIT_SECONDS),
        }
    }

    /// The real environment unless the test variables redirect it: `CM_UNINSTALL_TEST_ROOT` (a key under
    /// `HKCU\Software\CleanManagerTest\`), `CM_UNINSTALL_TEST_HOME` (a scratch home folder; required with the root),
    /// `CM_UNINSTALL_QUIT_WAIT_MS` and `CM_ASSUME_STANDARD_USER=1` (only ever restrict).
    pub fn from_process() -> Result<UninstallEnv, String> {
        let mut env = UninstallEnv::real();
        if let Ok(root) = std::env::var("CM_UNINSTALL_TEST_ROOT") {
            env.registry_root = StartupEnv::checked_root(&root).map_err(|_| format!(r"uninstall test root must be a key under HKCU\{TEST_ROOT_PREFIX}"))?;
            let home = std::env::var_os("CM_UNINSTALL_TEST_HOME").ok_or("CM_UNINSTALL_TEST_ROOT needs CM_UNINSTALL_TEST_HOME")?;
            env.home = Some(PathBuf::from(home));
        }
        if let Some(ms) = std::env::var("CM_UNINSTALL_QUIT_WAIT_MS").ok().and_then(|v| v.parse::<u64>().ok()) {
            env.quit_wait = Duration::from_millis(ms.min(60_000));
        }
        if std::env::var("CM_ASSUME_STANDARD_USER").is_ok_and(|v| v == "1") {
            env.is_admin = false;
        }
        Ok(env)
    }

    pub fn is_redirected(&self) -> bool {
        !self.registry_root.is_empty()
    }

    /// Physical (root key, subkey, view flags) of a logical key. Redirected 32-bit HKLM keys live under `WOW6432Node`.
    fn physical(&self, hive: Hive, path: &str, view: RegView) -> (HKEY, String, u32) {
        if self.is_redirected() {
            let p = match (hive, view) {
                (Hive::Hklm, RegView::Wow64_32) if path.len() > 9 && path[..9].eq_ignore_ascii_case(r"SOFTWARE\") => format!(r"SOFTWARE\WOW6432Node\{}", &path[9..]),
                _ => path.to_owned(),
            };
            return (HKEY_CURRENT_USER, format!(r"{}\{}\{p}", self.registry_root, hive.as_str()), 0);
        }
        let root = if hive == Hive::Hklm { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER };
        let flags = match view {
            RegView::Default => 0,
            RegView::Wow64_64 => KEY_WOW64_64KEY,
            RegView::Wow64_32 => KEY_WOW64_32KEY,
        };
        (root, path.to_owned(), flags)
    }

    fn open(&self, loc: &RegLocation) -> Result<RegKey, u32> {
        self.check_root(loc)?;
        let (root, sub, flags) = self.physical(loc.hive, &loc.key_path, loc.view);
        RegKey::open(root, &sub, flags)
    }

    /// A location planned against another root is never acted on here.
    fn check_root(&self, loc: &RegLocation) -> Result<(), u32> {
        if loc.root == self.registry_root { Ok(()) } else { Err(ERROR_FILE_NOT_FOUND) }
    }

    fn loc(&self, hive: Hive, view: RegView, key_path: String) -> RegLocation {
        RegLocation { root: self.registry_root.clone(), hive, view, key_path }
    }

    /// The startup environment matching this one (same registry redirection, Startup folder under the test home).
    pub fn startup_env(&self) -> StartupEnv {
        if !self.is_redirected() {
            return StartupEnv { is_admin: self.is_admin, ..StartupEnv::real() };
        }
        let user_folder = self.home.as_ref().map(|h| h.join(r"AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup"));
        StartupEnv { registry_root: self.registry_root.clone(), user_folder, common_folder: None, is_admin: self.is_admin }
    }

    pub fn folders(&self) -> Folders {
        let env_dir = |k: &str| std::env::var_os(k).map(PathBuf::from);
        match &self.home {
            Some(h) => Folders {
                home: h.clone(), roaming: h.join(r"AppData\Roaming"), local: h.join(r"AppData\Local"), program_data: h.join("ProgramData"),
                start_menu: vec![h.join(r"AppData\Roaming\Microsoft\Windows\Start Menu\Programs")], desktop: vec![h.join("Desktop")], scoop: h.join("scoop"),
                node: Some(h.join(r"Program Files\nodejs")),
            },
            None => {
                let home = env_dir("USERPROFILE").unwrap_or_default();
                let roaming = env_dir("APPDATA").unwrap_or_else(|| home.join(r"AppData\Roaming"));
                let program_data = env_dir("ProgramData").unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
                Folders {
                    local: env_dir("LOCALAPPDATA").unwrap_or_else(|| home.join(r"AppData\Local")),
                    start_menu: vec![roaming.join(r"Microsoft\Windows\Start Menu\Programs"), program_data.join(r"Microsoft\Windows\Start Menu\Programs")],
                    desktop: vec![home.join("Desktop"), env_dir("PUBLIC").map(|p| p.join("Desktop")).unwrap_or_else(|| PathBuf::from(r"C:\Users\Public\Desktop"))],
                    scoop: env_dir("SCOOP").unwrap_or_else(|| home.join("scoop")),
                    node: None,
                    home, roaming, program_data,
                }
            }
        }
    }
}

/// Folders the uninstall flow reads (real or under the test home).
#[derive(Debug, Clone)]
pub struct Folders {
    pub home: PathBuf,
    pub roaming: PathBuf,
    pub local: PathBuf,
    pub program_data: PathBuf,
    pub start_menu: Vec<PathBuf>,
    pub desktop: Vec<PathBuf>,
    pub scoop: PathBuf,
    /// Node.js install folder under the test home; `None` = the real install location (`adapters::locate`).
    pub node: Option<PathBuf>,
}

impl Folders {
    fn npm_modules(&self) -> PathBuf {
        self.roaming.join(r"npm\node_modules")
    }
}

fn utf16(data: &[u8]) -> String {
    let mut units: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    if let Some(end) = units.iter().position(|&u| u == 0) {
        units.truncate(end);
    }
    String::from_utf16_lossy(&units)
}

/// Registry text, environment references expanded for REG_EXPAND_SZ (or literal `%` text that expands).
fn text(k: &RegKey, name: &str) -> Option<String> {
    let (kind, data) = k.value(name)?;
    if kind != REG_SZ && kind != REG_EXPAND_SZ {
        return None;
    }
    let s = utf16(&data);
    let s = if kind == REG_EXPAND_SZ || s.contains('%') { super::startup::expand_env(&s).unwrap_or(s) } else { s };
    clean_text(&s)
}

/// Normalize a recorded location as the inventory does: trimmed, unquoted, expanded, no trailing separator, absolute.
fn normalize_location(s: &str) -> Option<String> {
    let t = s.trim().trim_matches('"').trim();
    let t = if t.contains('%') { super::startup::expand_env(t)? } else { t.to_owned() };
    let mut t = t.replace('/', "\\");
    while t.len() > 3 && t.ends_with('\\') {
        t.pop();
    }
    let b = t.as_bytes();
    let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\';
    (drive || t.starts_with(r"\\")).then_some(t)
}

fn raw(p: &str) -> Option<RawPath> {
    RawPath::windows(p.encode_utf16().collect()).ok()
}

fn is_product_code(key: &str) -> bool {
    let b = key.as_bytes();
    b.len() == 38 && b[0] == b'{' && b[37] == b'}' && b[1..37].iter().enumerate().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { *c == b'-' } else { c.is_ascii_hexdigit() })
}

/// One uninstall entry as a record (same instance id as `apps list`), or `None` for updates and nameless entries.
fn registry_record(env: &UninstallEnv, k: &RegKey, key_name: &str, hive: Hive, view: RegView) -> Option<UninstallRecord> {
    let name = text(k, "DisplayName")?;
    if k.string("ParentKeyName").is_some_and(|s| !s.trim_matches('\0').is_empty()) {
        return None;
    }
    if text(k, "ReleaseType").is_some_and(|r| {
        let r = r.to_ascii_lowercase();
        r.contains("update") || r.contains("hotfix") || r.contains("service pack")
    }) {
        return None;
    }
    let (rh, rv) = match (hive, view) {
        (Hive::Hklm, RegView::Wow64_32) => (RegistryHive::Hklm, RegistryView::View32),
        (Hive::Hklm, _) => (RegistryHive::Hklm, RegistryView::View64),
        (Hive::Hkcu, _) => (RegistryHive::Hkcu, RegistryView::Shared),
    };
    let msi = k.dword("WindowsInstaller") == Some(1) && is_product_code(key_name);
    let source = if msi {
        AppSource::Msi { hive: rh, view: rv, product_code: key_name.to_ascii_uppercase() }
    } else {
        AppSource::RegistryUninstall { hive: rh, view: rv, key: key_name.to_owned() }
    };
    let location = k.string("InstallLocation").and_then(|s| normalize_location(&s));
    let system_component = k.dword("SystemComponent") == Some(1);
    let publisher = text(k, "Publisher");
    let winget = text(k, "WinGetPackageIdentifier").map(|package_id| WingetEvidence { package_id, source: text(k, "WinGetSourceIdentifier") });
    Some(UninstallRecord {
        instance_id: instance_id_of(&source, location.as_deref().and_then(raw).as_ref()),
        protected_class: protected_reason(&name, publisher.as_deref(), system_component),
        name,
        version: text(k, "DisplayVersion"),
        publisher,
        source: if msi { RecordSource::Msi } else { RecordSource::RegistryUninstall },
        key: Some(env.loc(hive, view, format!(r"{UNINSTALL}\{key_name}"))),
        key_name: Some(key_name.to_owned()),
        product_code: msi.then(|| key_name.to_ascii_uppercase()),
        package_full_name: None,
        package_family: None,
        uninstall_string: k.string("UninstallString").map(|s| s.trim_matches('\0').trim().to_owned()).filter(|s| !s.is_empty()),
        quiet_uninstall_string: k.string("QuietUninstallString").map(|s| s.trim_matches('\0').trim().to_owned()).filter(|s| !s.is_empty()),
        install_location: location,
        system_component,
        winget,
        package_name: None,
        ai_tool: None,
        mac: None,
    })
}

fn read_uninstall_keys(env: &UninstallEnv, out: &mut Vec<UninstallRecord>, issues: &mut Vec<InventoryIssue>) {
    for (hive, view) in [(Hive::Hklm, RegView::Wow64_64), (Hive::Hklm, RegView::Wow64_32), (Hive::Hkcu, RegView::Default)] {
        let loc = env.loc(hive, view, UNINSTALL.to_owned());
        let parent = match env.open(&loc) {
            Ok(k) => k,
            Err(ERROR_FILE_NOT_FOUND) => continue,
            Err(rc) => {
                issues.push(InventoryIssue { code: "registry_open_failed".into(), source: loc.display(), message: format!("cannot open uninstall key (error {rc})") });
                continue;
            }
        };
        let flags = env.physical(hive, UNINSTALL, view).2;
        for name in parent.subkeys().unwrap_or_default() {
            if let Ok(k) = parent.child(&name, flags) {
                out.extend(registry_record(env, &k, &name, hive, view));
            }
        }
    }
}

/// Packaged apps of the current user (real hives only; the package repository is never redirected).
fn read_packages(out: &mut Vec<UninstallRecord>) {
    let Ok(parent) = RegKey::open(HKEY_CURRENT_USER, PACKAGES, 0) else { return };
    for full in parent.subkeys().unwrap_or_default() {
        let parts: Vec<&str> = full.split('_').collect();
        if parts.len() != 5 || parts[0].is_empty() || parts[4].is_empty() {
            continue;
        }
        let Ok(k) = parent.child(&full, 0) else { continue };
        let family = format!("{}_{}", parts[0], parts[4]);
        let root = k.string("PackageRootFolder").and_then(|s| normalize_location(&s));
        let framework = k.dword("Framework") == Some(1);
        let under_windows = root.as_deref().is_some_and(|r| r.to_ascii_lowercase().contains(r":\windows\"));
        let name = text(&k, "DisplayName").filter(|d| !d.starts_with('@') && !d.starts_with("ms-resource:")).unwrap_or_else(|| parts[0].to_owned());
        let protected = if framework {
            Some(ProtectedReason::SharedRuntime)
        } else if parts[4] == WINDOWS_PUBLISHER_ID || under_windows {
            Some(ProtectedReason::OsPackage)
        } else {
            protected_reason(&name, None, false).or_else(|| protected_reason(parts[0], None, false))
        };
        let source = AppSource::Msix { package_family: family.clone(), full_name: full.clone() };
        out.push(UninstallRecord {
            instance_id: instance_id_of(&source, root.as_deref().and_then(raw).as_ref()),
            name, version: clean_text(parts[1]), publisher: None, source: RecordSource::Msix, key: None, key_name: None, product_code: None,
            package_full_name: Some(full.clone()), package_family: Some(family), uninstall_string: None, quiet_uninstall_string: None, install_location: root,
            system_component: false, winget: None, package_name: None, ai_tool: None, protected_class: protected, mac: None,
        });
    }
}

fn json_field(path: &Path, field: &str) -> Option<String> {
    // A top-level `"field": "text"` pair; display only, so a simple scan is enough (no JSON dependency here).
    let text = std::fs::read_to_string(path).ok()?;
    let key = format!("\"{field}\"");
    let after = &text[text.find(&key)? + key.len()..];
    let after = after.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    let end = after.find('"')?;
    clean_text(&after[..end]).filter(|v| v.len() <= 64)
}

/// Scoop apps (`<scoop>\apps\<name>`) and npm-global AI CLIs: package-manager records the uninstall keys do not hold.
fn read_tools(env: &UninstallEnv, out: &mut Vec<UninstallRecord>) {
    let f = env.folders();
    if let Ok(rd) = std::fs::read_dir(f.scoop.join("apps")) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.eq_ignore_ascii_case("scoop") || !e.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let loc = e.path().to_string_lossy().into_owned();
            out.push(UninstallRecord {
                instance_id: tool_instance_id(RecordSource::Scoop, &name), version: json_field(&e.path().join(r"current\manifest.json"), "version"),
                name: name.clone(), publisher: None, source: RecordSource::Scoop, key: None, key_name: None, product_code: None, package_full_name: None,
                package_family: None, uninstall_string: None, quiet_uninstall_string: None, install_location: Some(loc), system_component: false, winget: None,
                package_name: Some(name), ai_tool: None, protected_class: None, mac: None,
            });
        }
    }
    for t in AI_CLIS {
        let dir = t.npm_package.split('/').fold(f.npm_modules(), |p, c| p.join(c));
        let manifest = dir.join("package.json");
        if !manifest.is_file() {
            continue;
        }
        out.push(UninstallRecord {
            instance_id: tool_instance_id(RecordSource::NpmGlobal, t.npm_package), name: t.name.into(), version: json_field(&manifest, "version"), publisher: None,
            source: RecordSource::NpmGlobal, key: None, key_name: None, product_code: None, package_full_name: None, package_family: None, uninstall_string: None,
            quiet_uninstall_string: None, install_location: Some(dir.to_string_lossy().into_owned()), system_component: false, winget: None,
            package_name: Some(t.npm_package.into()), ai_tool: Some(t.id.into()), protected_class: None, mac: None,
        });
    }
}

/// Every uninstall record of this environment (registry, packages, Scoop, npm-global AI CLIs), by instance id.
pub fn records(env: &UninstallEnv) -> (Vec<UninstallRecord>, Vec<InventoryIssue>) {
    let mut out = Vec::new();
    let mut issues = Vec::new();
    read_uninstall_keys(env, &mut out, &mut issues);
    if !env.is_redirected() {
        read_packages(&mut out);
    }
    read_tools(env, &mut out);
    let mut seen = BTreeSet::new();
    out.retain(|r| seen.insert(r.instance_id.clone()));
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.instance_id.cmp(&b.instance_id)));
    (out, issues)
}

pub fn installed_apps(records: &[UninstallRecord]) -> Vec<InstalledApp> {
    records
        .iter()
        .map(|r| InstalledApp { instance_id: r.instance_id.clone(), name: r.name.clone(), publisher: r.publisher.clone(), install_location: r.install_location.clone(), key_name: r.key_name.clone() })
        .collect()
}

fn system_dir() -> String {
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut buf = vec![0u16; 520];
    // SAFETY: buffer of the given length.
    let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() { r"C:\Windows\System32".into() } else { String::from_utf16_lossy(&buf[..n]) }
}

/// winget.exe inside the App Installer package folder (not the per-user app execution alias, which is a link).
fn winget_exe() -> Option<String> {
    let parent = RegKey::open(HKEY_CURRENT_USER, PACKAGES, 0).ok()?;
    let arch = if cfg!(target_arch = "aarch64") { "_arm64_" } else { "_x64_" };
    parent.subkeys().ok()?.into_iter().filter(|n| n.to_ascii_lowercase().starts_with(APP_INSTALLER_PREFIX) && n.to_ascii_lowercase().contains(arch)).find_map(|n| {
        let root = parent.child(&n, 0).ok()?.string("PackageRootFolder").and_then(|s| normalize_location(&s))?;
        let exe = format!(r"{root}\winget.exe");
        regular_file(Path::new(&exe)).is_ok().then_some(exe)
    })
}

fn msix_manifest(rec: &UninstallRecord) -> Option<String> {
    let m = format!(r"{}\AppxManifest.xml", rec.install_location.as_deref()?);
    regular_file(Path::new(&m)).is_ok().then_some(m)
}

/// Trusted tool locations for one record (UNI-01).
pub fn adapter_env(env: &UninstallEnv, rec: &UninstallRecord) -> AdapterEnv {
    let f = env.folders();
    let scoop_ok = f.scoop.join(r"apps\scoop\current\bin\scoop.ps1").is_file();
    let node = match (&f.node, rec.source == RecordSource::NpmGlobal) {
        (_, false) => None,
        // Test home: a fixture Node.js folder there, never the machine's (nothing real is run by a test).
        (Some(dir), true) => {
            let (exe, script) = (dir.join("node.exe"), dir.join(r"node_modules\npm\bin\npm-cli.js"));
            (regular_file(&exe).is_ok() && regular_file(&script).is_ok()).then(|| (exe.to_string_lossy().into_owned(), script.to_string_lossy().into_owned()))
        }
        (None, true) => cm_core::adapter::ADAPTERS
            .iter()
            .find(|s| s.tool_id == "npm")
            .and_then(|spec| super::adapters::locate(spec).ok())
            .and_then(|(exe, script)| Some((exe.to_string_lossy().into_owned(), script?.to_string_lossy().into_owned()))),
    };
    AdapterEnv {
        system_dir: system_dir(),
        scoop_root: scoop_ok.then(|| f.scoop.to_string_lossy().into_owned()),
        winget_exe: if rec.winget.is_some() && !env.is_redirected() { winget_exe() } else { None },
        node,
        msix_manifest: if rec.source == RecordSource::Msix { msix_manifest(rec) } else { None },
        brew: None,
    }
}

/// UNI-01: the route `apps uninstall list` reports for one record (adapter, or the refusal with its reason).
pub fn route(env: &UninstallEnv, rec: &UninstallRecord) -> Result<AdapterChoice, Refusal> {
    select_adapter(rec, &adapter_env(env, rec), &|p: &str| Path::new(p).is_file())
}

/// Capability form of a route: `available`; `unsupported` for protected classes (`policy_denied`: no uninstall entry at
/// all); `unverified` with the reason otherwise (no verifiable route on this machine, see the Settings entry).
pub fn route_capability(route: &Result<AdapterChoice, Refusal>) -> Capability {
    match route {
        Ok(_) => Capability::new("app.uninstall", CapabilityState::Available, None),
        Err(r) if r.code == "policy_denied" => Capability::new("app.uninstall", CapabilityState::Unsupported, Some(&r.message)),
        Err(r) => Capability::new("app.uninstall", CapabilityState::Unverified, Some(&r.message)),
    }
}

/// `apps list`: each inventory instance's uninstall capability from the same decision `apps uninstall list` makes for
/// that instance id (replaces the read-only first release's blanket `unsupported`). Protected instances are never
/// `available`; an instance without an uninstall record of its own is `unverified`.
pub fn apply_uninstall_capability(env: &UninstallEnv, apps: &mut [AppInstance]) {
    let (recs, _) = records(env);
    for a in apps {
        let cap = match recs.iter().find(|r| r.instance_id == a.instance_id) {
            Some(r) => route_capability(&route(env, r)),
            None => Capability::new("app.uninstall", CapabilityState::Unverified, Some("no uninstall record matches this instance; see `cleanmanager apps uninstall list`")),
        };
        a.uninstall_capability = match (a.protected_class, cap.state) {
            (Some(_), CapabilityState::Available) => Capability::new("app.uninstall", CapabilityState::Unsupported, Some(cm_core::uninstall::SETTINGS_GUIDANCE)),
            _ => cap,
        };
    }
}

// ---------------------------------------------------------------------------------------------
// Program verification (UNI-03)

/// A regular, local, non-link file.
pub fn regular_file(p: &Path) -> Result<(), String> {
    let m = std::fs::symlink_metadata(p).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("{} does not exist", p.display()),
        _ => format!("{} cannot be inspected (os error {})", p.display(), e.raw_os_error().unwrap_or(0)),
    })?;
    if m.file_attributes() & ATTR_REPARSE != 0 || !m.is_file() {
        return Err(format!("{} is not a regular file", p.display()));
    }
    if p.to_string_lossy().starts_with(r"\\") {
        return Err(format!("{} is on a network location", p.display()));
    }
    Ok(())
}

pub fn sha256_file(p: &Path) -> Result<String, String> {
    let f = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    cm_core::adapter::sha256_hex(f).map_err(|e| format!("{}: {e}", p.display()))
}

/// Signer of the Windows catalog that lists this file's hash (system files are usually catalog-signed, not embedded).
pub fn catalog_signer(path: &str) -> Option<String> {
    use windows_sys::Win32::Security::Cryptography::Catalog::{
        CATALOG_INFO, CryptCATAdminAcquireContext2, CryptCATAdminCalcHashFromFileHandle2, CryptCATAdminEnumCatalogFromHash, CryptCATAdminReleaseCatalogContext,
        CryptCATAdminReleaseContext, CryptCATCatalogInfoFromContext,
    };
    use windows_sys::Win32::Security::WinTrust::{
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_CATALOG_INFO, WINTRUST_DATA, WINTRUST_DATA_0, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_CATALOG, WTD_REVOCATION_CHECK_NONE,
        WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE, WinVerifyTrust,
    };
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, OPEN_EXISTING};
    let wpath = wide(path);
    // SAFETY: NUL-terminated path; read-only open of a file, closed below.
    let file: HANDLE = unsafe { CreateFileW(wpath.as_ptr(), 0x8000_0000, FILE_SHARE_READ, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut()) };
    if file.is_null() || file == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return None;
    }
    let mut admin: isize = 0;
    let alg = wide("SHA256");
    let mut out = None;
    // SAFETY: valid out-pointer; default subsystem; released below.
    if unsafe { CryptCATAdminAcquireContext2(&mut admin, std::ptr::null(), alg.as_ptr(), std::ptr::null(), 0) } != 0 {
        let mut len = 0u32;
        // SAFETY: size query, then a buffer of that size.
        unsafe { CryptCATAdminCalcHashFromFileHandle2(admin, file, &mut len, std::ptr::null_mut(), 0) };
        let mut hash = vec![0u8; len as usize];
        // SAFETY: buffer of `len` bytes.
        if len > 0 && unsafe { CryptCATAdminCalcHashFromFileHandle2(admin, file, &mut len, hash.as_mut_ptr(), 0) } != 0 {
            // SAFETY: hash buffer is valid; catalog context released below.
            let cat = unsafe { CryptCATAdminEnumCatalogFromHash(admin, hash.as_ptr(), len, 0, std::ptr::null_mut()) };
            if cat != 0 {
                let mut info = CATALOG_INFO { cbStruct: size_of::<CATALOG_INFO>() as u32, wszCatalogFile: [0; 260] };
                // SAFETY: catalog context from the enumeration above.
                if unsafe { CryptCATCatalogInfoFromContext(cat, &mut info, 0) } != 0 {
                    let tag: Vec<u16> = hash.iter().map(|b| format!("{b:02X}")).collect::<String>().encode_utf16().chain(std::iter::once(0)).collect();
                    let mut ci = WINTRUST_CATALOG_INFO {
                        cbStruct: size_of::<WINTRUST_CATALOG_INFO>() as u32, pcwszCatalogFilePath: info.wszCatalogFile.as_ptr(), pcwszMemberTag: tag.as_ptr(),
                        pcwszMemberFilePath: wpath.as_ptr(), hMemberFile: file, pbCalculatedFileHash: hash.as_mut_ptr(), cbCalculatedFileHash: len, hCatAdmin: admin,
                        ..Default::default()
                    };
                    let mut data = WINTRUST_DATA {
                        cbStruct: size_of::<WINTRUST_DATA>() as u32, dwUIChoice: WTD_UI_NONE, fdwRevocationChecks: WTD_REVOKE_NONE, dwUnionChoice: WTD_CHOICE_CATALOG,
                        Anonymous: WINTRUST_DATA_0 { pCatalog: &mut ci }, dwStateAction: WTD_STATEACTION_VERIFY,
                        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL | WTD_REVOCATION_CHECK_NONE, ..Default::default()
                    };
                    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
                    // SAFETY: fully initialized WINTRUST_DATA pointing at live buffers; state closed below.
                    let status = unsafe { WinVerifyTrust(std::ptr::null_mut(), &mut action, (&mut data as *mut WINTRUST_DATA).cast()) };
                    if status == 0 {
                        out = signer_from_state(data.hWVTStateData);
                    }
                    data.dwStateAction = WTD_STATEACTION_CLOSE;
                    // SAFETY: closes the state opened above.
                    unsafe { WinVerifyTrust(std::ptr::null_mut(), &mut action, (&mut data as *mut WINTRUST_DATA).cast()) };
                }
                // SAFETY: context from the enumeration above, released once.
                unsafe { CryptCATAdminReleaseCatalogContext(admin, cat, 0) };
            }
        }
        // SAFETY: context acquired above, released once.
        unsafe { CryptCATAdminReleaseContext(admin, 0) };
    }
    // SAFETY: handle opened above, closed once.
    unsafe { CloseHandle(file) };
    out
}

fn signer_from_state(state: HANDLE) -> Option<String> {
    use windows_sys::Win32::Security::Cryptography::{CERT_NAME_SIMPLE_DISPLAY_TYPE, CertGetNameStringW};
    use windows_sys::Win32::Security::WinTrust::{WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData};
    // SAFETY: state data from a successful verify; pointers are checked before use.
    unsafe {
        let prov = WTHelperProvDataFromStateData(state);
        let signer = if prov.is_null() { std::ptr::null_mut() } else { WTHelperGetProvSignerFromChain(prov, 0, 0, 0) };
        if signer.is_null() || (*signer).csCertChain == 0 || (*signer).pasCertChain.is_null() {
            return None;
        }
        let cert = (*(*signer).pasCertChain).pCert;
        if cert.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 512];
        let n = CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, std::ptr::null(), buf.as_mut_ptr(), buf.len() as u32);
        if n > 1 { clean_text(&String::from_utf16_lossy(&buf[..n as usize - 1])) } else { None }
    }
}

/// Verified signer: embedded Authenticode first, then the Windows catalog. `None` = unsigned or not verifiable.
pub fn signer_of(path: &str) -> Option<String> {
    super::startup::authenticode_signer(path).or_else(|| catalog_signer(path))
}

/// UNI-03 plan-time check of the program a choice runs: (SHA-256, signer). Refusals carry the protocol code.
pub fn verify_program(choice: &AdapterChoice) -> Result<(String, Option<String>), (&'static str, String)> {
    let p = Path::new(&choice.program);
    regular_file(p).map_err(|e| ("unsupported", format!("uninstaller not usable: {e}")))?;
    let sha = sha256_file(p).map_err(|e| ("io_error", e))?;
    if choice.kind == AdapterKind::Msix {
        return Ok((sha, None));
    }
    let signer = signer_of(&choice.program);
    if let Some(publisher) = &choice.publisher_required {
        match &signer {
            None => return Err(("policy_denied", format!("{} has no valid signature, so its publisher cannot be verified; {}", choice.program, cm_core::uninstall::SETTINGS_GUIDANCE))),
            Some(s) if !publisher_matches(s, publisher) => {
                return Err(("policy_denied", format!("{} is signed by \"{s}\", not by the recorded publisher \"{publisher}\"; it is not run", choice.program)));
            }
            Some(_) => {}
        }
    }
    if choice.microsoft_signed && !signer.as_deref().is_some_and(is_microsoft_signer) {
        return Err(("policy_denied", format!("{} is not signed by Microsoft", choice.program)));
    }
    Ok((sha, signer))
}

// ---------------------------------------------------------------------------------------------
// Processes (UNI-02)

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: handle from a successful open, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

fn open_process(pid: u32, access: u32) -> Option<Handle> {
    use windows_sys::Win32::System::Threading::OpenProcess;
    // SAFETY: plain open; the handle is closed by `Handle`.
    let h = unsafe { OpenProcess(access, 0, pid) };
    (!h.is_null()).then_some(Handle(h))
}

fn image_path(pid: u32) -> Option<String> {
    use windows_sys::Win32::System::Threading::{PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
    let h = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let mut buf = vec![0u16; 32_768];
    let mut len = buf.len() as u32;
    // SAFETY: buffer of `len` units.
    (unsafe { QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) } != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

/// (pid, image file name) of every process.
fn processes() -> Result<Vec<(u32, String)>, String> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    // SAFETY: snapshot handle closed by `Handle`.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err("process list unavailable".into());
    }
    let snap = Handle(snap);
    let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
    let mut out = Vec::new();
    // SAFETY: entry sized as required.
    let mut ok = unsafe { Process32FirstW(snap.0, &mut e) } != 0;
    while ok {
        let end = e.szExeFile.iter().position(|&u| u == 0).unwrap_or(e.szExeFile.len());
        out.push((e.th32ProcessID, String::from_utf16_lossy(&e.szExeFile[..end])));
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snap.0, &mut e) } != 0;
    }
    Ok(out)
}

/// Executable names directly in the install folder or one level below (for the unknown-state check).
fn exe_names(dir: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut stack = vec![(PathBuf::from(dir), 0)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten().take(2000) {
            let name = e.file_name().to_string_lossy().to_lowercase();
            match e.file_type() {
                Ok(t) if t.is_dir() && depth < 1 => stack.push((e.path(), depth + 1)),
                Ok(t) if t.is_file() && name.ends_with(".exe") => {
                    out.insert(name);
                }
                _ => {}
            }
        }
    }
    out
}

/// Processes of an app: image under its install location. A process whose image cannot be read but whose name is one
/// of the app's executables makes the state unknown (the uninstall is then skipped).
pub fn running_under(dir: &str) -> RunningState {
    let list = match processes() {
        Ok(l) => l,
        Err(e) => return RunningState::Unknown { reason: e },
    };
    let names = exe_names(dir);
    let own = std::process::id();
    let mut found = Vec::new();
    for (pid, name) in list {
        if pid == own || pid <= 4 {
            continue;
        }
        match image_path(pid) {
            Some(p) if path_within(&p, dir) => found.push(RunningProcess { pid, name, path: Some(p) }),
            Some(_) => {}
            None if names.contains(&name.to_lowercase()) => {
                return RunningState::Unknown { reason: format!("{name} (pid {pid}) may belong to the app, but its image cannot be read") };
            }
            None => {}
        }
    }
    RunningState::Known { processes: found }
}

fn alive(p: &RunningProcess) -> bool {
    use windows_sys::Win32::System::Threading::{PROCESS_SYNCHRONIZE, WaitForSingleObject};
    let Some(h) = open_process(p.pid, PROCESS_SYNCHRONIZE) else { return false };
    // SAFETY: valid process handle; zero timeout poll.
    let running = unsafe { WaitForSingleObject(h.0, 0) } == windows_sys::Win32::Foundation::WAIT_TIMEOUT;
    // The pid may have been reused by another program since.
    running && image_path(p.pid).map(|i| Some(i.to_lowercase()) == p.path.as_ref().map(|x| x.to_lowercase())).unwrap_or(true)
}

/// Visible top-level windows of these processes.
fn windows_of(pids: &BTreeSet<u32>) -> Vec<HWND> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsWindowVisible};
    struct Ctx<'a> {
        pids: &'a BTreeSet<u32>,
        out: Vec<HWND>,
    }
    unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
        // SAFETY: lparam is the &mut Ctx passed to EnumWindows below, alive for the whole enumeration.
        let ctx = unsafe { &mut *(lparam as *mut Ctx<'_>) };
        let mut pid = 0u32;
        // SAFETY: valid window handle from the enumeration.
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        // SAFETY: as above.
        if ctx.pids.contains(&pid) && unsafe { IsWindowVisible(hwnd) } != 0 {
            ctx.out.push(hwnd);
        }
        1
    }
    let mut ctx = Ctx { pids, out: Vec::new() };
    // SAFETY: callback only reads the context through lparam during this call.
    unsafe { EnumWindows(Some(each), &mut ctx as *mut Ctx<'_> as LPARAM) };
    ctx.out
}

/// UNI-02: ask the app to close (WM_CLOSE to its visible top-level windows), wait up to `wait`; returns what still runs.
pub fn request_quit(processes: &[RunningProcess], wait: Duration) -> Vec<RunningProcess> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
    let pids: BTreeSet<u32> = processes.iter().map(|p| p.pid).collect();
    for hwnd in windows_of(&pids) {
        // SAFETY: posting a close request to a window of the app; no data pointers.
        unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
    }
    let deadline = Instant::now() + wait;
    loop {
        let still: Vec<RunningProcess> = processes.iter().filter(|p| alive(p)).cloned().collect();
        if still.is_empty() || Instant::now() >= deadline {
            return still;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// End one process a person confirmed ending; refused for critical processes and CleanManager itself.
pub fn end_process(p: &RunningProcess) -> Result<(), String> {
    use windows_sys::Win32::System::Threading::{PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject};
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    if let Some(why) = end_refusal(p, std::process::id(), &root) {
        return Err(format!("{} is not ended: {why}", p.describe()));
    }
    if image_path(p.pid).map(|i| i.to_lowercase()) != p.path.as_ref().map(|x| x.to_lowercase()) {
        return Err(format!("{} is no longer the planned program; nothing was ended", p.describe()));
    }
    let h = open_process(p.pid, PROCESS_TERMINATE | PROCESS_SYNCHRONIZE).ok_or_else(|| format!("{} cannot be opened", p.describe()))?;
    // SAFETY: valid handle with terminate access.
    if unsafe { TerminateProcess(h.0, 1) } == 0 {
        return Err(format!("{} could not be ended: {}", p.describe(), std::io::Error::last_os_error()));
    }
    // SAFETY: valid handle; bounded wait.
    unsafe { WaitForSingleObject(h.0, 5000) };
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Running the bound invocation (UNI-03)

/// Run a stage-1 invocation and wait for it (bounded). Vendor uninstallers keep their own window; nothing runs
/// through a shell. Returns (exit code, duration in ms).
pub fn run_choice(choice: &AdapterChoice, timeout_seconds: u32) -> Result<(Option<i32>, u64), String> {
    if choice.kind == AdapterKind::Msix {
        let r = remove_package(choice.argv.get(1).map(String::as_str).unwrap_or_default());
        cm_core::debug_trace!("process", "msix removal through PackageManager: {}", match &r { Ok((code, ms)) => format!("result={code:?} in {ms}ms"), Err(e) => format!("refused: {e}") });
        return r;
    }
    let program = Path::new(&choice.program);
    let cwd = program.parent().filter(|p| p.is_dir()).map(Path::to_path_buf).unwrap_or_else(std::env::temp_dir);
    let mut cmd = std::process::Command::new(program);
    if choice.raw_args {
        for a in &choice.argv {
            cmd.raw_arg(a);
        }
    } else {
        cmd.args(&choice.argv);
    }
    cmd.current_dir(&cwd).stdin(std::process::Stdio::null());
    if choice.kind.package_manager() {
        cmd.env("NO_UPDATE_NOTIFIER", "1").env("npm_config_update_notifier", "false");
    }
    let started = Instant::now();
    let mut child = cmd.spawn().map_err(|e| format!("{} could not be started: {e}", choice.program))?;
    let deadline = started + Duration::from_secs(u64::from(timeout_seconds));
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                cm_core::debug_trace!("process", "uninstaller \"{}\" kind={:?} exit={} in {}ms", choice.program, choice.kind, cm_core::trace::exit(status.code()), started.elapsed().as_millis());
                return Ok((status.code(), started.elapsed().as_millis() as u64));
            }
            Ok(None) if Instant::now() >= deadline => {
                cm_core::debug_trace!("process", "uninstaller \"{}\" kind={:?} still running after {}ms (timeout; not ended)", choice.program, choice.kind, started.elapsed().as_millis());
                return Ok((None, started.elapsed().as_millis() as u64));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Remove an MSIX package for the current user through the Windows package manager (no PowerShell).
pub fn remove_package(full_name: &str) -> Result<(Option<i32>, u64), String> {
    if full_name.is_empty() || !full_name.chars().all(|c| c.is_ascii_alphanumeric() || "._-~".contains(c)) {
        return Err("package full name is not valid".into());
    }
    // Only a package registered for this user is removed; anything else is refused before the API is called.
    let registered = RegKey::open(HKEY_CURRENT_USER, PACKAGES, 0).ok().and_then(|k| k.subkeys().ok()).is_some_and(|n| n.iter().any(|x| x == full_name));
    if !registered {
        return Err(format!("package {full_name} is not registered for this user; nothing was removed"));
    }
    let started = Instant::now();
    let name = full_name.to_owned();
    let r = std::thread::spawn(move || -> windows_core::Result<()> {
        let pm = windows::Management::Deployment::PackageManager::new()?;
        let result = pm.RemovePackageAsync(&windows_core::HSTRING::from(name.as_str()))?.join()?;
        result.ExtendedErrorCode()?.ok()
    })
    .join()
    .map_err(|_| "package removal failed unexpectedly".to_owned())?;
    let ms = started.elapsed().as_millis() as u64;
    match r {
        Ok(()) => Ok((Some(0), ms)),
        Err(e) => Ok((Some(e.code().0), ms)),
    }
}

// ---------------------------------------------------------------------------------------------
// Registry trees (UNI-06)

struct WKey(HKEY);

impl Drop for WKey {
    fn drop(&mut self) {
        // SAFETY: handle from a successful create/open, closed once.
        unsafe { RegCloseKey(self.0) };
    }
}

fn read_tree(k: &RegKey, flags: u32, depth: usize) -> Result<KeyTree, String> {
    if depth > 16 {
        return Err("key is nested too deeply".into());
    }
    let mut values: Vec<(String, RegValue)> = k.values().map_err(|rc| format!("values unreadable (error {rc})"))?.into_iter().map(|(n, t, d)| (n, RegValue::new(t, &d))).collect();
    values.sort_by_key(|a| a.0.to_lowercase());
    let mut subkeys = Vec::new();
    for name in k.subkeys().map_err(|rc| format!("subkeys unreadable (error {rc})"))? {
        let child = k.child(&name, flags).map_err(|rc| format!("{name}: unreadable (error {rc})"))?;
        subkeys.push((name, read_tree(&child, flags, depth + 1)?));
    }
    subkeys.sort_by_key(|a| a.0.to_lowercase());
    Ok(KeyTree { values, subkeys })
}

fn create_key(parent: HKEY, sub: &str, flags: u32) -> Result<(WKey, bool), u32> {
    let z = wide(sub);
    let mut out: HKEY = std::ptr::null_mut();
    let mut disposition = 0u32;
    // SAFETY: NUL-terminated subkey; default security; valid out-pointers.
    let rc = unsafe { RegCreateKeyExW(parent, z.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_ALL_ACCESS | flags, std::ptr::null(), &mut out, &mut disposition) };
    // REG_CREATED_NEW_KEY = 1
    if rc == ERROR_SUCCESS { Ok((WKey(out), disposition == 1)) } else { Err(rc) }
}

fn write_tree(k: &WKey, tree: &KeyTree, flags: u32) -> Result<(), u32> {
    for (name, v) in &tree.values {
        let data = v.data().ok_or(13u32)?;
        let wname = wide(name);
        // SAFETY: NUL-terminated name; data buffer of the given length.
        let rc = unsafe { RegSetValueExW(k.0, wname.as_ptr(), 0, v.value_type, data.as_ptr(), data.len() as u32) };
        if rc != ERROR_SUCCESS {
            return Err(rc);
        }
    }
    for (name, sub) in &tree.subkeys {
        let (child, _) = create_key(k.0, name, flags)?;
        write_tree(&child, sub, flags)?;
    }
    Ok(())
}

fn os_error(rc: u32) -> String {
    std::io::Error::from_raw_os_error(rc as i32).to_string()
}

/// A person's answer to "end these still-running processes now?" (UNI-02).
pub type ConfirmEnd = Box<dyn Fn(&[RunningProcess]) -> bool>;

/// Platform uninstall backend for the shared executor.
pub struct WindowsUninstall {
    pub env: UninstallEnv,
    /// A person confirms ending still-running processes (interactive apply only; otherwise always false).
    pub confirm: ConfirmEnd,
}

impl WindowsUninstall {
    pub fn non_interactive(env: UninstallEnv) -> WindowsUninstall {
        WindowsUninstall { env, confirm: Box::new(|_| false) }
    }

    fn record(&self, instance_id: &str) -> Option<UninstallRecord> {
        records(&self.env).0.into_iter().find(|r| r.instance_id == instance_id)
    }
}

impl UninstallActions for WindowsUninstall {
    fn is_admin(&self) -> bool {
        self.env.is_admin
    }

    fn running(&self, identity: &AppIdentity) -> RunningState {
        if identity.source == RecordSource::NpmGlobal {
            // Node runs npm tools, so the image path says nothing: ask Restart Manager who holds the package's files.
            let Some(dir) = identity.install_location.as_deref().filter(|d| Path::new(d).is_dir()) else { return RunningState::Known { processes: vec![] } };
            let mut files = Vec::new();
            let mut stack = vec![PathBuf::from(dir)];
            while let Some(d) = stack.pop() {
                for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                    match e.file_type() {
                        Ok(t) if t.is_dir() && files.len() < 300 => stack.push(e.path()),
                        Ok(t) if t.is_file() && files.len() < 300 => files.extend(raw(&e.path().to_string_lossy())),
                        _ => {}
                    }
                }
            }
            return match super::restart_manager::holders(&files) {
                Ok(h) => RunningState::Known {
                    processes: h.into_iter().filter(|x| x.service.is_none()).map(|x| RunningProcess { pid: x.pid, name: x.image.clone().unwrap_or(x.app_name), path: image_path(x.pid) }).collect(),
                },
                Err(e) => RunningState::Unknown { reason: format!("the processes using {dir} cannot be determined: {e}") },
            };
        }
        match identity.install_location.as_deref() {
            Some(dir) if Path::new(dir).is_dir() => running_under(dir),
            Some(_) => RunningState::Known { processes: vec![] },
            None => RunningState::Unknown { reason: "the install location is not recorded, so its processes cannot be identified".into() },
        }
    }

    fn request_quit(&self, processes: &[RunningProcess]) -> Vec<RunningProcess> {
        request_quit(processes, self.env.quit_wait)
    }

    fn confirm_end(&self, processes: &[RunningProcess]) -> bool {
        (self.confirm)(processes)
    }

    fn end_process(&self, process: &RunningProcess) -> Result<(), String> {
        end_process(process)
    }

    fn verify(&self, rec: &UninstallRecord, choice: &AdapterChoice, sha256: &str, signer: Option<&str>) -> Result<(), (String, String)> {
        let live = self.record(&rec.instance_id).ok_or(("record_changed".to_owned(), "the application's registration is gone; nothing was run".to_owned()))?;
        if live.digest() != rec.digest() {
            return Err(("record_changed".into(), "the application's registration changed since planning; create a new plan".into()));
        }
        let (sha, live_signer) = verify_program(choice).map_err(|(c, m)| (c.to_owned(), m))?;
        if sha != sha256 {
            return Err(("program_changed".into(), format!("{} changed since planning (SHA-256 differs); nothing was run", choice.program)));
        }
        if live_signer.as_deref() != signer {
            return Err(("policy_denied".into(), format!("the signer of {} changed since planning", choice.program)));
        }
        Ok(())
    }

    fn run(&self, choice: &AdapterChoice, timeout_seconds: u32) -> Result<(Option<i32>, u64), String> {
        run_choice(choice, timeout_seconds)
    }

    fn rescan(&self, identity: &AppIdentity) -> Rescan {
        let install_dir_present = identity.install_location.as_deref().map(|d| Path::new(d).exists());
        let registration_present = match (&identity.key, identity.source) {
            (Some(k), _) => match self.env.open(k) {
                Ok(_) => Some(true),
                Err(ERROR_FILE_NOT_FOUND) => Some(false),
                Err(_) => None,
            },
            (None, RecordSource::Msix) => Some(self.record(&identity.instance_id).is_some()),
            (None, _) => install_dir_present,
        };
        Rescan { install_dir_present, registration_present }
    }

    fn read_key(&self, loc: &RegLocation) -> Result<Option<KeyTree>, String> {
        let flags = self.env.physical(loc.hive, &loc.key_path, loc.view).2;
        match self.env.open(loc) {
            Ok(k) => read_tree(&k, flags, 0).map(Some),
            Err(ERROR_FILE_NOT_FOUND) => Ok(None),
            Err(rc) => Err(format!("{}: {}", loc.display(), os_error(rc))),
        }
    }

    fn delete_key(&self, loc: &RegLocation) -> Result<(), (String, String)> {
        if registry_denied(&loc.key_path) {
            return Err(("policy_denied".into(), format!("{} is not an application's own key", loc.display())));
        }
        self.env.check_root(loc).map_err(|_| ("policy_denied".to_owned(), "planned against another registry root".to_owned()))?;
        let (root, sub, flags) = self.env.physical(loc.hive, &loc.key_path, loc.view);
        let (parent_path, name) = sub.rsplit_once('\\').ok_or(("policy_denied".to_owned(), "not a subkey".to_owned()))?;
        let mut parent: HKEY = std::ptr::null_mut();
        let pz = wide(parent_path);
        // SAFETY: NUL-terminated subkey; valid out-pointer.
        let rc = unsafe { RegOpenKeyExW(root, pz.as_ptr(), 0, KEY_ALL_ACCESS | flags, &mut parent) };
        if rc != ERROR_SUCCESS {
            return Err((if rc == 5 { "permission_required" } else { "io_error" }.into(), format!("{}: {}", loc.display(), os_error(rc))));
        }
        let parent = WKey(parent);
        let nz = wide(name);
        // SAFETY: deletes this one subkey (and its subtree) below the opened parent.
        let rc = unsafe { RegDeleteTreeW(parent.0, nz.as_ptr()) };
        match rc {
            ERROR_SUCCESS => {
                use windows_sys::Win32::System::Registry::RegDeleteKeyExW;
                // RegDeleteTree empties the key; remove the key itself too (view-aware).
                // SAFETY: NUL-terminated name below the opened parent.
                let rc = unsafe { RegDeleteKeyExW(parent.0, nz.as_ptr(), flags, 0) };
                if rc == ERROR_SUCCESS || rc == ERROR_FILE_NOT_FOUND { Ok(()) } else { Err(("io_error".into(), os_error(rc))) }
            }
            ERROR_FILE_NOT_FOUND => Err(("precondition_missing".into(), "the key no longer exists".into())),
            5 => Err(("permission_required".into(), os_error(rc))),
            other => Err(("io_error".into(), os_error(other))),
        }
    }

    fn restore_key(&self, backup: &KeyBackup) -> Result<(), (String, String)> {
        let loc = &backup.location;
        if registry_denied(&loc.key_path) {
            return Err(("policy_denied".into(), format!("{} is not an application's own key", loc.display())));
        }
        self.env.check_root(loc).map_err(|_| ("policy_denied".to_owned(), "backup belongs to another registry root".to_owned()))?;
        if !matches!(self.read_key(loc), Ok(None)) {
            return Err(("destination_exists".into(), format!("{} exists again (recreated since); it is left as it is", loc.display())));
        }
        let (root, sub, flags) = self.env.physical(loc.hive, &loc.key_path, loc.view);
        let (k, created) = create_key(root, &sub, flags).map_err(|rc| ("io_error".to_owned(), os_error(rc)))?;
        if !created {
            return Err(("destination_exists".into(), format!("{} was recreated meanwhile; it is left as it is", loc.display())));
        }
        write_tree(&k, &backup.tree, flags).map_err(|rc| ("io_error".to_owned(), format!("restore incomplete: {}", os_error(rc))))
    }
}

// ---------------------------------------------------------------------------------------------
// Leftover discovery (UNI-04/05/07/08/09)

fn mtime_idle(m: &std::fs::Metadata) -> Option<u64> {
    m.modified().ok().and_then(|t| SystemTime::now().duration_since(t).ok()).map(|d| d.as_secs())
}

/// Bounded size of a folder (files only; links are not followed). `None` when the walk hit its bound or failed.
fn folder_size(p: &Path) -> Option<u64> {
    let mut total = 0u64;
    let mut seen = 0usize;
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).ok()?.flatten() {
            seen += 1;
            if seen > 50_000 {
                return None;
            }
            let m = e.path().symlink_metadata().ok()?;
            if m.file_attributes() & ATTR_REPARSE != 0 {
                continue;
            }
            if m.is_dir() { stack.push(e.path()) } else { total += m.len() }
        }
    }
    Some(total)
}

fn entries(dir: &Path) -> Vec<DirEntryInfo> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    rd.flatten()
        .filter_map(|e| {
            let m = e.path().symlink_metadata().ok()?;
            if m.file_attributes() & ATTR_REPARSE != 0 {
                return None;
            }
            Some(DirEntryInfo {
                path: e.path().to_string_lossy().into_owned(), name: e.file_name().to_string_lossy().into_owned(), is_dir: m.is_dir(),
                logical_bytes: if m.is_dir() { None } else { Some(m.len()) }, idle_seconds: mtime_idle(&m),
            })
        })
        .collect()
}

fn path_leftover(display: &str, kind: LeftoverKind, group: Group, evidence: Vec<String>, bytes: Option<u64>) -> Leftover {
    Leftover {
        object_id: object_id(kind, display), kind, group, display: display.to_owned(), evidence, marker: None, tier: String::new(), selected: false, warnings: vec![],
        logical_bytes: bytes, note: None, requires_admin: false, binding: Binding::Path { path: display.to_owned() },
    }
}

/// Objects of one matched data folder: per child (grouped), or the whole folder when it holds only caches/logs.
fn data_folder_objects(dir: &DirEntryInfo, evidence: &str, group_of: &dyn Fn(&str, bool) -> Group, out: &mut Vec<Leftover>) {
    let children = entries(Path::new(&dir.path));
    let all_cache = children.iter().all(|c| group_of(&c.name, c.is_dir) == Group::CachesLogs);
    if all_cache {
        out.push(path_leftover(&dir.path, LeftoverKind::Directory, Group::CachesLogs, vec![evidence.to_owned(), "contains only caches and logs".into()], folder_size(Path::new(&dir.path))));
        return;
    }
    for c in children {
        let kind = if c.is_dir { LeftoverKind::Directory } else { LeftoverKind::File };
        let bytes = if c.is_dir { folder_size(Path::new(&c.path)) } else { c.logical_bytes };
        out.push(path_leftover(&c.path, kind, group_of(&c.name, c.is_dir), vec![evidence.to_owned()], bytes));
    }
}

/// Git worktrees inside an AI tool's data are blocked (uncommitted work cannot be ruled out without running git).
fn contains_worktree(p: &Path, depth: usize) -> bool {
    if p.join(".git").exists() {
        return true;
    }
    depth > 0 && std::fs::read_dir(p).map(|rd| rd.flatten().any(|e| e.file_type().is_ok_and(|t| t.is_dir()) && contains_worktree(&e.path(), depth - 1))).unwrap_or(false)
}

/// Shortcut target without resolving (IShellLinkW + IPersistFile::Load(STGM_READ)) on a COM thread.
fn link_targets(links: Vec<PathBuf>) -> Vec<(PathBuf, Option<String>)> {
    super::startup::on_com_thread(move || {
        use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ};
        use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
        use windows::core::{HSTRING, Interface};
        links
            .into_iter()
            .map(|l| {
                // SAFETY: COM calls on an initialized thread; buffers are sized by their slices.
                let t = unsafe {
                    (|| -> Option<String> {
                        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
                        let file: IPersistFile = link.cast().ok()?;
                        file.Load(&HSTRING::from(l.as_os_str()), STGM_READ).ok()?;
                        let mut buf = vec![0u16; 32_768];
                        link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
                        let end = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
                        clean_text(&String::from_utf16_lossy(&buf[..end]))
                    })()
                };
                (l, t)
            })
            .collect()
    })
    .unwrap_or_default()
}

fn lnk_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() && depth > 0 => lnk_files(&p, depth - 1, out),
            Ok(t) if t.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) => out.push(p),
            _ => {}
        }
    }
}

/// Long form of an existing path (8.3 short names expanded); `None` when it is gone or already long.
fn long_path(p: &str) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::GetLongPathNameW;
    let z = wide(p);
    let mut buf = vec![0u16; 32_768];
    // SAFETY: NUL-terminated input; output buffer of the given length.
    let n = unsafe { GetLongPathNameW(z.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let long = (n > 0 && n < buf.len()).then(|| String::from_utf16_lossy(&buf[..n]))?;
    (!long.eq_ignore_ascii_case(p)).then_some(long)
}

/// What the leftover preview needs besides the identity.
pub struct DiscoveryInput<'a> {
    pub identity: &'a AppIdentity,
    pub installed: &'a [InstalledApp],
    pub startup: &'a [cm_core::startup::StartupItem],
}

/// UNI-04..09: grouped leftovers of an uninstalled app, with markers; read-only.
pub fn discover(env: &UninstallEnv, input: &DiscoveryInput<'_>) -> Vec<Leftover> {
    let id = input.identity;
    let f = env.folders();
    let mut out: Vec<Leftover> = Vec::new();
    let add = |l: Leftover, m: Option<&cm_core::uninstall::NameMatch>, out: &mut Vec<Leftover>| {
        let mut l = l;
        if let Some(m) = m
            && let Some((marker, why)) = ownership_marker(id, &l.display, m, input.installed)
        {
            l.marker = Some(marker);
            l.evidence.push(why);
        }
        if !out.iter().any(|o| o.object_id == l.object_id) {
            out.push(l);
        }
    };
    // The install folder itself, if the uninstaller left it (bundle group; never preselected).
    if let Some(dir) = id.install_location.as_deref().filter(|d| Path::new(d).is_dir()) {
        let m = cm_core::uninstall::NameMatch { variant: String::new(), under_publisher: true, family_only: false };
        let mut l = path_leftover(dir, LeftoverKind::Directory, Group::Bundle, vec![format!("recorded install location of {}", id.name)], folder_size(Path::new(dir)));
        l.requires_admin = !path_within(dir, &f.home.to_string_lossy()) && !env.is_admin;
        add(l, Some(&m), &mut out);
    }
    // Known data-folder conventions: <root>\<Product> and <root>\<Publisher>\<Product>.
    let roots = [(&f.roaming, "%APPDATA%"), (&f.local, "%LOCALAPPDATA%"), (&f.program_data, "%PROGRAMDATA%")];
    let pubs: Vec<String> = id.publisher.as_deref().map(publisher_variants).unwrap_or_default();
    if id.ai_tool.is_none() {
        for (root, label) in roots {
            let system = label == "%PROGRAMDATA%";
            for e in entries(root).into_iter().filter(|e| e.is_dir) {
                let mut candidates = Vec::new();
                if let Some(m) = match_name(id, &e.name, false) {
                    candidates.push((e.clone(), m, format!(r"{label}\{} matches the product name", e.name)));
                } else if pubs.contains(&e.name.to_lowercase()) {
                    for c in entries(Path::new(&e.path)).into_iter().filter(|c| c.is_dir) {
                        if let Some(m) = match_name(id, &c.name, true) {
                            candidates.push((c.clone(), m, format!(r"{label}\{}\{} matches publisher\product", e.name, c.name)));
                        }
                    }
                }
                for (dir, m, why) in candidates {
                    let mut objs = Vec::new();
                    data_folder_objects(&dir, &why, &classify_child, &mut objs);
                    for mut l in objs {
                        if system {
                            l.group = Group::System;
                            l.requires_admin = !env.is_admin;
                        }
                        add(l, Some(&m), &mut out);
                    }
                }
            }
        }
        if let Some(fam) = &id.package_family {
            let p = f.local.join("Packages").join(fam);
            if p.is_dir() {
                let info = DirEntryInfo { path: p.to_string_lossy().into_owned(), name: fam.clone(), is_dir: true, logical_bytes: None, idle_seconds: None };
                let mut objs = Vec::new();
                data_folder_objects(&info, &format!(r"%LOCALAPPDATA%\Packages\{fam} is the package's data folder"), &classify_child, &mut objs);
                let m = cm_core::uninstall::NameMatch { variant: fam.to_lowercase(), under_publisher: true, family_only: false };
                for l in objs {
                    add(l, Some(&m), &mut out);
                }
            }
        }
    }
    // AI CLIs (UNI-07): their home data folder, per entry; sessions/settings unselected, worktrees blocked.
    if let Some(tool) = id.ai_tool.as_deref().and_then(cm_core::uninstall::ai_cli) {
        let dir = f.home.join(tool.home_dir);
        for c in entries(&dir) {
            let kind = if c.is_dir { LeftoverKind::Directory } else { LeftoverKind::File };
            let bytes = if c.is_dir { folder_size(Path::new(&c.path)) } else { c.logical_bytes };
            let mut l = path_leftover(&c.path, kind, ai_child_group(tool, &c.name, c.is_dir), vec![format!("{} data folder (~\\{})", tool.name, tool.home_dir)], bytes);
            if c.is_dir && contains_worktree(Path::new(&c.path), 3) {
                l.tier = "blocked".into();
                l.note = Some("contains a git worktree: uncommitted work cannot be ruled out; remove it with `git worktree remove` yourself".into());
            }
            add(l, None, &mut out);
        }
        for name in tool.home_files {
            let p = f.home.join(name);
            if let Some(m) = p.symlink_metadata().ok().filter(|m| m.is_file()) {
                let mut l = path_leftover(&p.to_string_lossy(), LeftoverKind::File, Group::Settings, vec![format!("{} settings file", tool.name)], Some(m.len()));
                // A file directly in the home folder is not planned (binding it would mean reading the whole home folder).
                l.binding = Binding::Manual { guidance: "settings file in your home folder: delete it yourself after review if you no longer need it".into() };
                add(l, None, &mut out);
            }
        }
    }
    // Registry leftovers (UNI-06): the registration if it remains, and Software\[Publisher\]Product keys.
    if let Some(k) = &id.key
        && let Ok(Some(_)) = WindowsUninstall::non_interactive(env.clone()).read_key(k)
    {
        let mut l = reg_leftover(k, format!("{} uninstall registration remains after the uninstaller", id.name));
        l.group = if k.hive == Hive::Hklm { Group::System } else { Group::Settings };
        l.requires_admin = k.hive == Hive::Hklm && !env.is_admin;
        let m = cm_core::uninstall::NameMatch { variant: String::new(), under_publisher: true, family_only: false };
        add(l, Some(&m), &mut out);
    }
    if id.ai_tool.is_none() {
        for (hive, view) in [(Hive::Hkcu, RegView::Default), (Hive::Hklm, RegView::Wow64_64), (Hive::Hklm, RegView::Wow64_32)] {
            let software = env.loc(hive, view, "SOFTWARE".into());
            let Ok(sw) = env.open(&software) else { continue };
            let flags = env.physical(hive, "SOFTWARE", view).2;
            for name in sw.subkeys().unwrap_or_default() {
                let mut found: Vec<(String, cm_core::uninstall::NameMatch, String)> = Vec::new();
                if let Some(m) = match_name(id, &name, false) {
                    found.push((format!(r"SOFTWARE\{name}"), m, format!(r"{}\SOFTWARE\{name} matches the product name", hive.as_str())));
                } else if pubs.contains(&name.to_lowercase())
                    && let Ok(pk) = sw.child(&name, flags)
                {
                    for sub in pk.subkeys().unwrap_or_default() {
                        if let Some(m) = match_name(id, &sub, true) {
                            found.push((format!(r"SOFTWARE\{name}\{sub}"), m, format!(r"{}\SOFTWARE\{name}\{sub} matches publisher\product", hive.as_str())));
                        }
                    }
                }
                for (path, m, why) in found {
                    if registry_denied(&path) {
                        continue;
                    }
                    let loc = env.loc(hive, view, path);
                    let mut l = reg_leftover(&loc, why);
                    l.group = if hive == Hive::Hklm { Group::System } else { Group::Settings };
                    l.requires_admin = hive == Hive::Hklm && !env.is_admin;
                    add(l, Some(&m), &mut out);
                }
            }
        }
    }
    // Teardown (UNI-09): startup entries, shortcuts, App Paths registrations and services pointing into the install folder.
    if let Some(dir) = id.install_location.as_deref() {
        // Links and registrations store long paths; a recorded location may be an 8.3 short form of the same folder.
        let long = long_path(dir);
        let within = |t: &str| path_within(t, dir) || long.as_deref().is_some_and(|l| path_within(t, l));
        for s in input.startup {
            let Some(t) = s.target.as_ref().map(|t| t.display()) else { continue };
            if !within(&t) {
                continue;
            }
            let removable = s.removable_when_stale && s.location.is_some() && !s.protected;
            let binding = if removable { Binding::Startup { startup_item_id: s.item_id.clone() } } else { Binding::Manual { guidance: cm_core::startup::OPEN_SETTINGS_GUIDANCE.into() } };
            let mut l = Leftover {
                object_id: object_id(LeftoverKind::StartupEntry, &s.item_id), kind: LeftoverKind::StartupEntry, group: Group::Startup,
                display: format!("startup entry {} ({}) → {t}", s.name, s.definition), evidence: vec![format!("starts a program in {dir}")], marker: None,
                tier: if removable { String::new() } else { "review_only".into() }, selected: false, warnings: vec![], logical_bytes: None,
                note: (!removable).then(|| "removed only when its program is gone (stale); otherwise use Settings > Apps > Startup".into()), requires_admin: false, binding,
            };
            if s.state != cm_core::startup::StartupState::Broken && removable {
                l.binding = Binding::Manual { guidance: "its program still exists; uninstall first, then review leftovers again".into() };
            }
            add(l, None, &mut out);
        }
        let mut links = Vec::new();
        for d in f.start_menu.iter().chain(f.desktop.iter()) {
            lnk_files(d, if f.desktop.contains(d) { 0 } else { 3 }, &mut links);
        }
        for (lnk, target) in link_targets(links) {
            if target.as_deref().is_some_and(within) {
                let display = lnk.to_string_lossy().into_owned();
                let mut l = path_leftover(&display, LeftoverKind::Shortcut, Group::Startup, vec![format!("shortcut to {}", target.unwrap_or_default())], std::fs::metadata(&lnk).ok().map(|m| m.len()));
                l.requires_admin = !path_within(&display, &f.home.to_string_lossy()) && !env.is_admin;
                // A file directly on a desktop is not planned (binding it would mean scanning the whole desktop folder).
                if lnk.parent().is_some_and(|p| f.desktop.iter().any(|d| d.as_path() == p)) {
                    l.binding = Binding::Manual { guidance: "shortcut on the desktop: delete it yourself after review (CleanManager does not scan your whole desktop to bind it)".into() };
                }
                add(l, None, &mut out);
            }
        }
        for (hive, view) in [(Hive::Hkcu, RegView::Default), (Hive::Hklm, RegView::Wow64_64), (Hive::Hklm, RegView::Wow64_32)] {
            let Ok(ap) = env.open(&env.loc(hive, view, APP_PATHS.into())) else { continue };
            let flags = env.physical(hive, APP_PATHS, view).2;
            for name in ap.subkeys().unwrap_or_default() {
                let target = ap.child(&name, flags).ok().and_then(|k| text(&k, ""));
                if target.as_deref().is_some_and(|t| within(t.trim_matches('"'))) {
                    let loc = env.loc(hive, view, format!(r"{APP_PATHS}\{name}"));
                    let mut l = reg_leftover(&loc, format!("App Paths registration of {name} points into {dir}"));
                    l.group = Group::Startup;
                    l.requires_admin = hive == Hive::Hklm && !env.is_admin;
                    add(l, None, &mut out);
                }
            }
        }
        if !env.is_redirected()
            && let Ok(services) = RegKey::open(HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Services", 0)
        {
            for name in services.subkeys().unwrap_or_default() {
                let image = services.child(&name, 0).ok().and_then(|k| text(&k, "ImagePath"));
                let program = image.as_deref().and_then(|i| cm_core::startup::split_command(i, &|p| Path::new(p).exists())).map(|t| t.program);
                if program.as_deref().is_some_and(within) {
                    let display = format!("service {name} ({})", image.unwrap_or_default());
                    add(Leftover {
                        object_id: object_id(LeftoverKind::Service, &name), kind: LeftoverKind::Service, group: Group::System, display, evidence: vec![format!("service program lives in {dir}")],
                        marker: None, tier: "review_only".into(), selected: false, warnings: vec![], logical_bytes: None, note: None, requires_admin: true,
                        binding: Binding::Manual { guidance: format!("CleanManager does not delete services: stop it and run `sc.exe delete {name}` from an administrator terminal, or reinstall and uninstall with the vendor's tool") },
                    }, None, &mut out);
                }
            }
        }
    }
    out.into_iter().map(finish_leftover).collect()
}

fn reg_leftover(loc: &RegLocation, why: String) -> Leftover {
    let display = loc.display();
    Leftover {
        object_id: object_id(LeftoverKind::RegistryKey, &display), kind: LeftoverKind::RegistryKey, group: Group::Settings, display, evidence: vec![why], marker: None,
        tier: String::new(), selected: false, warnings: vec![], logical_bytes: None, note: Some("backed up before it is deleted; restorable from history".into()),
        requires_admin: false, binding: Binding::Registry { location: loc.clone() },
    }
}

/// UNI-07: when an AI CLI was last used, from the newest entry of its data folder (Unix seconds); `None` = unknown.
pub fn ai_last_used(env: &UninstallEnv, tool_id: &str) -> Option<i64> {
    let tool = cm_core::uninstall::ai_cli(tool_id)?;
    let dir = env.folders().home.join(tool.home_dir);
    std::iter::once(dir.clone())
        .chain(std::fs::read_dir(&dir).ok()?.flatten().map(|e| e.path()))
        .filter_map(|p| p.symlink_metadata().ok()?.modified().ok())
        .max()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
}

/// `text` (lowercase) mentions `needle` as a whole path: the next character does not continue a file name, so
/// `C:\Tools\Foo` is not found in `C:\Tools\FooBar`.
fn mentions_path(text: &str, needle: &str) -> bool {
    text.match_indices(needle).any(|(i, _)| text[i + needle.len()..].chars().next().is_none_or(|c| !(c.is_alphanumeric() || "_-.".contains(c))))
}

/// Configuration files outside the app (git, shells, npm, editors, Windows Terminal) whose text mentions its install
/// folder: read-only hints (UNI-05). They are only read, never planned or changed.
pub fn external_references(env: &UninstallEnv, identity: &AppIdentity) -> Vec<String> {
    let Some(dir) = identity.install_location.as_deref().filter(|d| d.len() > 3) else { return vec![] };
    let f = env.folders();
    let candidates = [
        f.home.join(".gitconfig"), f.home.join(r".config\git\config"), f.home.join(".npmrc"), f.home.join(".bashrc"), f.home.join(".bash_profile"),
        f.home.join(".profile"), f.home.join(".zshrc"), f.home.join(r"Documents\WindowsPowerShell\Microsoft.PowerShell_profile.ps1"),
        f.home.join(r"Documents\PowerShell\Microsoft.PowerShell_profile.ps1"), f.roaming.join(r"Code\User\settings.json"),
        f.local.join(r"Packages\Microsoft.WindowsTerminal_8wekyb3d8bbwe\LocalState\settings.json"),
    ];
    let needle = dir.trim_end_matches('\\').to_lowercase();
    let forms = [needle.clone(), needle.replace('\\', "/"), needle.replace('\\', "\\\\")];
    candidates
        .iter()
        .filter(|p| !path_within(&p.to_string_lossy(), dir))
        .filter(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file() && m.len() < 4 << 20))
        .filter(|p| std::fs::read_to_string(p).is_ok_and(|t| {
            let l = t.to_lowercase();
            forms.iter().any(|n| mentions_path(&l, n))
        }))
        .map(|p| format!("{} mentions {dir} (read-only hint; CleanManager does not edit it)", p.display()))
        .collect()
}

/// UNI-08: orphan candidates from broken startup entries; review_only, never preselected.
pub fn orphans(env: &UninstallEnv, startup: &[cm_core::startup::StartupItem], installed: &[InstalledApp]) -> Vec<Leftover> {
    let refs: Vec<BrokenReference> = startup
        .iter()
        .filter(|s| s.state == cm_core::startup::StartupState::Broken && !s.protected)
        .filter_map(|s| Some(BrokenReference { startup_item_id: s.item_id.clone(), name: s.name.clone(), missing_target: s.target.as_ref()?.display() }))
        .collect();
    let f = env.folders();
    // Only folders named like a missing program's folder can be candidates: size just those.
    let names: BTreeSet<String> = refs.iter().filter_map(|r| cm_core::uninstall::app_folder_of(&r.missing_target)).map(|n| n.to_lowercase()).collect();
    let mut dirs = Vec::new();
    for root in [&f.roaming, &f.local] {
        dirs.extend(entries(root).into_iter().filter(|e| e.is_dir && names.contains(&e.name.to_lowercase())).map(|mut e| {
            e.logical_bytes = folder_size(Path::new(&e.path));
            e
        }));
    }
    let mut out = orphan_leftovers(&refs, &dirs, installed);
    for l in &mut out {
        if l.warnings.is_empty() {
            l.warnings = sensitive_warnings(&l.display);
        }
    }
    out
}

#[cfg(test)]
mod tests;
