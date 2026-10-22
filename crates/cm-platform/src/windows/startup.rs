//! Read-only Windows startup inventory (parity 11.1, STA-01).
//!
//! Sources:
//! - `Run`, `RunOnce` and policy `Run` keys under HKCU and HKLM (64- and 32-bit views), with Explorer's
//!   `StartupApproved` records for the enabled/disabled state Task Manager shows.
//! - The per-user and common Startup folders (known folders), `.lnk` targets read with `IShellLinkW` after
//!   `IPersistFile::Load(STGM_READ)`; the link is never resolved (`Resolve` can search and rewrite links).
//! - Task Scheduler tasks with a logon or boot trigger, read through the Task Scheduler COM API.
//! - Packaged-app startup tasks (`windows.startupTask`) registered for the current user.
//!
//! Safety boundary: registry keys are opened with `KEY_READ` only, files are only listed, stat'ed or read, COM
//! objects are only queried, and nothing is executed: command lines are split into a program path for display and
//! an existence check, never started. The publisher is the signer of a verified Authenticode signature with
//! revocation and network retrieval disabled. Changes (11.2/11.3) live in `startup_actions.rs`; this module stays
//! read-only and only reports, per item, the verified location a change would use and whether it is available.
//!
//! Tests point the registry at a throwaway key (`StartupEnv::registry_root`, under `HKCU\Software\CleanManagerTest\`)
//! and the Startup folders at temporary directories, so real startup entries are never touched by a test.

use std::collections::HashMap;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;

use cm_core::apps::{InventoryIssue, clean_text};
use cm_core::capability::CapabilityState;
use cm_core::path::RawPath;
use cm_core::plan::v2::Hive;
use cm_core::startup::{
    ADMIN_REQUIRED, OPEN_SETTINGS_GUIDANCE, PublisherEvidence, RegView, StartupItem, StartupLocation, StartupScope, StartupSource, TargetCheck,
    approval_enabled, fold_state, is_script_host, item_id, normalize, protected_reason, split_command, toggle_capability,
};
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY, REG_DWORD, REG_EXPAND_SZ, REG_SZ, RegCloseKey, RegEnumKeyExW,
    RegEnumValueW, RegOpenKeyExW, RegQueryValueExW,
};

pub(crate) const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_ONCE: &str = r"Software\Microsoft\Windows\CurrentVersion\RunOnce";
const POLICY_RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer\Run";
pub(crate) const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";
/// Only redirection roots below this key are accepted (tests).
pub const TEST_ROOT_PREFIX: &str = r"Software\CleanManagerTest\";

/// Where startup definitions are read from (and, in `startup_actions`, written to).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupEnv {
    /// Empty: the real HKCU/HKLM. Otherwise a throwaway key under `HKCU\Software\CleanManagerTest\` that holds `HKCU\…`
    /// and `HKLM\…` copies of the keys; scheduled and packaged-app tasks are then not read.
    pub registry_root: String,
    /// Replacement Startup folders (user, common). Used only when given; with a redirected registry the real folders
    /// are never read.
    pub user_folder: Option<PathBuf>,
    pub common_folder: Option<PathBuf>,
    /// Whether this process may change machine-wide entries (an elevated token). Never acquired by this tool.
    pub is_admin: bool,
}

impl StartupEnv {
    /// The real hives and folders, with the process token's elevation.
    pub fn real() -> StartupEnv {
        StartupEnv { is_admin: super::startup_actions::is_elevated(), ..Default::default() }
    }

    /// The real environment unless the test variables redirect it: `CM_STARTUP_TEST_ROOT` (a key under
    /// `HKCU\Software\CleanManagerTest\`), `CM_STARTUP_TEST_FOLDER` (user Startup folder) and `CM_ASSUME_STANDARD_USER=1`
    /// (decide as a non-administrator; only ever restricts).
    pub fn from_process() -> Result<StartupEnv, String> {
        let mut env = StartupEnv::real();
        if let Ok(root) = std::env::var("CM_STARTUP_TEST_ROOT") {
            env.registry_root = Self::checked_root(&root)?;
            env.user_folder = std::env::var_os("CM_STARTUP_TEST_FOLDER").map(PathBuf::from);
        }
        if std::env::var("CM_ASSUME_STANDARD_USER").is_ok_and(|v| v == "1") {
            env.is_admin = false;
        }
        Ok(env)
    }

    /// Accept only a key strictly below the test prefix, without traversal or unusual characters.
    pub fn checked_root(root: &str) -> Result<String, String> {
        let r = root.trim_end_matches('\\');
        let ok = r.len() > TEST_ROOT_PREFIX.len()
            && r.is_char_boundary(TEST_ROOT_PREFIX.len())
            && r[..TEST_ROOT_PREFIX.len()].eq_ignore_ascii_case(TEST_ROOT_PREFIX)
            && r.split('\\').all(|c| !c.is_empty() && c != "." && c != ".." && c.chars().all(|ch| ch.is_ascii_alphanumeric() || "-_. ".contains(ch)));
        if ok { Ok(r.to_owned()) } else { Err(format!(r"startup test root must be a key under HKCU\{TEST_ROOT_PREFIX}")) }
    }

    pub fn is_redirected(&self) -> bool {
        !self.registry_root.is_empty()
    }

    /// Physical (root key, subkey, view flags) of a logical key in `hive`.
    pub(crate) fn physical(&self, hive: Hive, path: &str, view: RegView) -> (HKEY, String, u32) {
        if self.is_redirected() {
            return (HKEY_CURRENT_USER, format!(r"{}\{}\{path}", self.registry_root, hive.as_str()), 0);
        }
        let root = if hive == Hive::Hklm { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER };
        let flags = match view {
            RegView::Default => 0,
            RegView::Wow64_64 => KEY_WOW64_64KEY,
            RegView::Wow64_32 => KEY_WOW64_32KEY,
        };
        (root, path.to_owned(), flags)
    }

    pub(crate) fn open(&self, hive: Hive, path: &str, view: RegView) -> Result<RegKey, u32> {
        let (root, sub, flags) = self.physical(hive, path, view);
        RegKey::open(root, &sub, flags)
    }

    /// Display form of a logical key as it is physically read.
    pub(crate) fn label(&self, hive: Hive, path: &str) -> String {
        if self.is_redirected() { format!(r"HKCU\{}\{}\{path}", self.registry_root, hive.as_str()) } else { format!(r"{}\{path}", hive.as_str()) }
    }
}
const SYSTEM_APP_DATA: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData";
const PACKAGES: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The only registry access mask this module (and `insights`) requests: `KEY_READ` plus an optional view flag.
pub(crate) fn read_access(view: u32) -> u32 {
    KEY_READ | (view & (KEY_WOW64_64KEY | KEY_WOW64_32KEY))
}

/// A registry key opened read-only; closed on drop.
pub(crate) struct RegKey(HKEY);

impl Drop for RegKey {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful RegOpenKeyExW and is closed once.
        unsafe { RegCloseKey(self.0) };
    }
}

/// UTF-16 registry string data without the terminator.
fn utf16_data(data: &[u8]) -> String {
    let mut units: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    if let Some(end) = units.iter().position(|&u| u == 0) {
        units.truncate(end);
    }
    String::from_utf16_lossy(&units)
}

impl RegKey {
    pub(crate) fn open(root: HKEY, path: &str, view: u32) -> Result<RegKey, u32> {
        let z = wide(path);
        let mut out: HKEY = std::ptr::null_mut();
        // SAFETY: NUL-terminated subkey, valid out-pointer, read-only access mask.
        let rc = unsafe { RegOpenKeyExW(root, z.as_ptr(), 0, read_access(view), &mut out) };
        if rc == ERROR_SUCCESS { Ok(RegKey(out)) } else { Err(rc) }
    }

    pub(crate) fn child(&self, name: &str, view: u32) -> Result<RegKey, u32> {
        RegKey::open(self.0, name, view)
    }

    pub(crate) fn subkeys(&self) -> Result<Vec<String>, u32> {
        let mut out = Vec::new();
        let mut buf = vec![0u16; 32_768];
        for index in 0u32.. {
            let mut len = buf.len() as u32;
            // SAFETY: buffer of `len` units; class and time outputs are not requested.
            let rc = unsafe {
                RegEnumKeyExW(self.0, index, buf.as_mut_ptr(), &mut len, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
            };
            match rc {
                ERROR_SUCCESS => out.push(String::from_utf16_lossy(&buf[..len as usize])),
                ERROR_NO_MORE_ITEMS => break,
                other => return Err(other),
            }
        }
        Ok(out)
    }

    /// All values as (name, type, data).
    pub(crate) fn values(&self) -> Result<Vec<(String, u32, Vec<u8>)>, u32> {
        let mut out = Vec::new();
        let mut name = vec![0u16; 32_768];
        let mut data = vec![0u8; 4096];
        let mut index = 0u32;
        loop {
            let mut name_len = name.len() as u32;
            let mut data_len = data.len() as u32;
            let mut kind = 0u32;
            // SAFETY: both buffers are sized by the length arguments.
            let rc = unsafe { RegEnumValueW(self.0, index, name.as_mut_ptr(), &mut name_len, std::ptr::null(), &mut kind, data.as_mut_ptr(), &mut data_len) };
            match rc {
                ERROR_SUCCESS => {
                    out.push((String::from_utf16_lossy(&name[..name_len as usize]), kind, data[..data_len as usize].to_vec()));
                    index += 1;
                }
                // Grow the data buffer and retry the same index.
                ERROR_MORE_DATA if (data_len as usize) > data.len() && data_len < (16 << 20) => data = vec![0u8; data_len as usize],
                ERROR_NO_MORE_ITEMS => break,
                other => return Err(other),
            }
        }
        Ok(out)
    }

    pub(crate) fn value(&self, name: &str) -> Option<(u32, Vec<u8>)> {
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

    /// REG_SZ / REG_EXPAND_SZ text, unexpanded.
    pub(crate) fn string(&self, name: &str) -> Option<String> {
        let (kind, data) = self.value(name)?;
        (kind == REG_SZ || kind == REG_EXPAND_SZ).then(|| utf16_data(&data))
    }

    pub(crate) fn dword(&self, name: &str) -> Option<u32> {
        let (kind, data) = self.value(name)?;
        (kind == REG_DWORD && data.len() >= 4).then(|| u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
    }
}

/// Expand `%VAR%` from the process environment; `None` if a reference is undefined.
pub(crate) fn expand_env(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after.find('%')?;
        out.push_str(&std::env::var(&after[..end]).ok()?);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

fn windows_dir() -> String {
    std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())
}

/// Read-only existence check of a target, distinguishing an offline volume from a missing file (STA-03). Network
/// paths are not probed.
fn check_path(path: &str) -> TargetCheck {
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    let b = path.as_bytes();
    if b.len() < 3 || !(b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')) {
        return TargetCheck::Unchecked; // UNC, device or relative path
    }
    let root = wide(&path[..3]);
    // SAFETY: NUL-terminated root path.
    match unsafe { GetDriveTypeW(root.as_ptr()) } {
        1 => return TargetCheck::VolumeOffline, // DRIVE_NO_ROOT_DIR
        4 => return TargetCheck::Unchecked,     // DRIVE_REMOTE
        _ => {}
    }
    match std::fs::symlink_metadata(path) {
        Ok(_) => TargetCheck::Exists,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => TargetCheck::Missing,
        Err(_) => TargetCheck::Unchecked,
    }
}

/// Resolve a startup command to a target path and its check. Bare program names are looked up in the Windows
/// and System32 directories only (no PATH search, no App Paths); unresolved bare names stay unknown.
pub(crate) fn resolve_command(command: &str) -> (Option<String>, TargetCheck, Option<&'static str>) {
    let exists = |p: &str| expand_env(p).is_some_and(|e| check_path(&e) == TargetCheck::Exists);
    let Some(split) = split_command(command, &exists) else { return (None, TargetCheck::Unchecked, Some("Command line is empty")) };
    let Some(program) = expand_env(&split.program) else { return (None, TargetCheck::Unchecked, Some("Command uses an undefined environment variable")) };
    let host_note = is_script_host(&program).then_some("Target is a script or DLL host; the program it runs is in its arguments");
    if split.bare {
        let win = windows_dir();
        let names = if program.contains('.') { vec![program.clone()] } else { vec![format!("{program}.exe"), program.clone()] };
        for dir in [format!(r"{win}\System32"), win.clone()] {
            for n in &names {
                let p = format!(r"{dir}\{n}");
                if check_path(&p) == TargetCheck::Exists {
                    return (Some(p), TargetCheck::Exists, host_note);
                }
            }
        }
        return (None, TargetCheck::Unchecked, Some("Program name without a directory; not found in the Windows directories"));
    }
    let check = check_path(&program);
    (Some(program), check, host_note)
}

fn raw(path: &str) -> Option<RawPath> {
    RawPath::windows(std::ffi::OsStr::new(path).encode_wide().collect()).ok()
}

/// Signer display name of a verified Authenticode signature (embedded signatures only; catalog-signed files
/// report none). No UI, no revocation check, no network retrieval.
pub(crate) fn authenticode_signer(path: &str) -> Option<String> {
    use windows_sys::Win32::Security::Cryptography::{CERT_NAME_SIMPLE_DISPLAY_TYPE, CertGetNameStringW};
    use windows_sys::Win32::Security::WinTrust::{
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE,
        WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvSignerFromChain,
        WTHelperProvDataFromStateData, WinVerifyTrust,
    };
    let wpath = wide(path);
    let mut file = WINTRUST_FILE_INFO { cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32, pcwszFilePath: wpath.as_ptr(), ..Default::default() };
    let mut data = WINTRUST_DATA {
        cbStruct: size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL | WTD_REVOCATION_CHECK_NONE,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: fully initialized WINTRUST_DATA pointing at live buffers; state is closed below.
    let status = unsafe { WinVerifyTrust(std::ptr::null_mut(), &mut action, (&mut data as *mut WINTRUST_DATA).cast()) };
    let mut name = None;
    if status == 0 {
        // SAFETY: state data from a successful verify; pointers are checked before use.
        unsafe {
            let prov = WTHelperProvDataFromStateData(data.hWVTStateData);
            let signer = if prov.is_null() { std::ptr::null_mut() } else { WTHelperGetProvSignerFromChain(prov, 0, 0, 0) };
            if !signer.is_null() && (*signer).csCertChain > 0 && !(*signer).pasCertChain.is_null() {
                let cert = (*(*signer).pasCertChain).pCert;
                if !cert.is_null() {
                    let mut buf = vec![0u16; 512];
                    let n = CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, std::ptr::null(), buf.as_mut_ptr(), buf.len() as u32);
                    if n > 1 {
                        name = clean_text(&String::from_utf16_lossy(&buf[..n as usize - 1]));
                    }
                }
            }
        }
    }
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: closes the state opened by the verify call above.
    unsafe { WinVerifyTrust(std::ptr::null_mut(), &mut action, (&mut data as *mut WINTRUST_DATA).cast()) };
    name
}

struct Builder {
    items: Vec<StartupItem>,
    issues: Vec<InventoryIssue>,
    signers: HashMap<String, Option<String>>,
    is_admin: bool,
}

/// One observed entry before protection and state folding.
struct Entry {
    source: StartupSource,
    scope: StartupScope,
    name: String,
    definition: String,
    target: Option<String>,
    check: TargetCheck,
    enabled: bool,
    publisher: Option<(String, PublisherEvidence)>,
    task_path: Option<String>,
    /// Read-only regardless of the target (RunOnce, policy, policy-managed packaged tasks).
    fixed: Option<&'static str>,
    note: Option<&'static str>,
    /// Verified change mechanism (Run values, Startup-folder files); `None` = settings page only.
    location: Option<StartupLocation>,
    /// A `Run` value: removable after backup when stale.
    removable: bool,
}

impl Builder {
    fn issue(&mut self, code: &str, source: &str, message: String) {
        self.issues.push(InventoryIssue { code: code.into(), source: source.into(), message });
    }

    fn signer(&mut self, path: &str) -> Option<String> {
        let key = path.to_lowercase();
        if let Some(s) = self.signers.get(&key) {
            return s.clone();
        }
        let s = std::fs::metadata(path).is_ok_and(|m| m.is_file()).then(|| authenticode_signer(path)).flatten();
        self.signers.insert(key, s.clone());
        s
    }

    fn push(&mut self, e: Entry) {
        let publisher = match e.publisher {
            Some(p) => Some(p),
            None if e.check == TargetCheck::Exists => e.target.as_deref().and_then(|t| self.signer(t)).map(|s| (s, PublisherEvidence::Authenticode)),
            None => None,
        };
        let (state, state_note) = fold_state(e.enabled, e.check);
        let protected = protected_reason(&e.name, publisher.as_ref().map(|p| p.0.as_str()), e.target.as_deref(), e.task_path.as_deref());
        let (toggle, protected_flag, reason) = match (protected, e.fixed) {
            (Some(why), _) => (CapabilityState::Unsupported, true, Some(why)),
            (None, Some(why)) => (CapabilityState::Unsupported, false, Some(why)),
            (None, None) => match toggle_capability(false, e.location.is_some(), e.scope, self.is_admin) {
                CapabilityState::Unverified => (CapabilityState::Unverified, false, Some(OPEN_SETTINGS_GUIDANCE)),
                CapabilityState::PermissionRequired => (CapabilityState::PermissionRequired, false, Some(ADMIN_REQUIRED)),
                other => (other, false, None),
            },
        };
        let changeable = protected.is_none() && e.fixed.is_none();
        let reason = [reason, state_note, e.note].into_iter().flatten().collect::<Vec<_>>().join("; ");
        let reason = clean_text(&reason);
        self.items.push(StartupItem {
            item_id: item_id(e.source, e.scope, &e.definition),
            name: clean_text(&e.name).unwrap_or_else(|| "(unnamed)".into()),
            source: e.source,
            definition: e.definition,
            target: e.target.as_deref().and_then(raw),
            publisher: publisher.as_ref().map(|p| p.0.clone()),
            publisher_evidence: publisher.map(|p| p.1).unwrap_or(PublisherEvidence::None),
            scope: e.scope,
            state,
            toggle_capability: toggle,
            protected: protected_flag,
            reason,
            location: if changeable { e.location } else { None },
            removable_when_stale: changeable && e.removable,
        });
    }
}

fn view_label(view: u32) -> &'static str {
    match view {
        KEY_WOW64_32KEY => " (32-bit view)",
        _ => "",
    }
}

/// Enabled flags recorded by Explorer under `StartupApproved\{sub}`, keyed by lower-case value name.
fn approvals(env: &StartupEnv, hive: Hive, sub: &str) -> HashMap<String, bool> {
    let Ok(key) = env.open(hive, &format!(r"{APPROVED}\{sub}"), RegView::Wow64_64) else { return HashMap::new() };
    key.values().unwrap_or_default().into_iter().filter_map(|(n, _, d)| approval_enabled(&d).map(|e| (n.to_lowercase(), e))).collect()
}

/// (hive, key path, view, scope, StartupApproved sub-key of its enable records, fixed read-only reason).
type RunLocation<'a> = (Hive, &'a str, RegView, StartupScope, Option<&'a str>, Option<&'static str>);

fn read_run_keys(b: &mut Builder, env: &StartupEnv) {
    let once: Option<&'static str> = Some("RunOnce entries run a single time, often as installer steps, and stay read-only");
    let policy: Option<&'static str> = Some("Set by policy; change it with Group Policy or the managing tool");
    let locations: [RunLocation; 8] = [
        (Hive::Hkcu, RUN, RegView::Default, StartupScope::User, Some("Run"), None),
        (Hive::Hkcu, RUN_ONCE, RegView::Default, StartupScope::User, None, once),
        (Hive::Hkcu, POLICY_RUN, RegView::Default, StartupScope::User, None, policy),
        (Hive::Hklm, RUN, RegView::Wow64_64, StartupScope::System, Some("Run"), None),
        (Hive::Hklm, RUN, RegView::Wow64_32, StartupScope::System, Some("Run32"), None),
        (Hive::Hklm, RUN_ONCE, RegView::Wow64_64, StartupScope::System, None, once),
        (Hive::Hklm, RUN_ONCE, RegView::Wow64_32, StartupScope::System, None, once),
        (Hive::Hklm, POLICY_RUN, RegView::Wow64_64, StartupScope::System, None, policy),
    ];
    for (hive, path, view, scope, approval_sub, fixed) in locations {
        // A redirected (test) registry has a single view.
        if env.is_redirected() && view == RegView::Wow64_32 {
            continue;
        }
        let view_flag = if view == RegView::Wow64_32 { KEY_WOW64_32KEY } else { 0 };
        let source = format!("{}{}", env.label(hive, path), view_label(view_flag));
        let approved = approval_sub.map(|sub| approvals(env, hive, sub));
        let key = match env.open(hive, path, view) {
            Ok(k) => k,
            Err(ERROR_FILE_NOT_FOUND) => continue,
            Err(rc) => {
                b.issue("registry_open_failed", &source, format!("cannot open key (error {rc})"));
                continue;
            }
        };
        let values = match key.values() {
            Ok(v) => v,
            Err(rc) => {
                b.issue("registry_enum_failed", &source, format!("cannot list values (error {rc})"));
                continue;
            }
        };
        for (name, kind, data) in values {
            if kind != REG_SZ && kind != REG_EXPAND_SZ {
                continue;
            }
            let command = utf16_data(&data);
            let (target, check, note) = resolve_command(&command);
            let enabled = approved.as_ref().and_then(|a| a.get(&name.to_lowercase()).copied()).unwrap_or(true);
            let shown = if name.is_empty() { "(Default)".to_owned() } else { name.clone() };
            let location = approval_sub.filter(|_| !name.is_empty()).map(|sub| StartupLocation::RunValue {
                root: env.registry_root.clone(), hive, view, key: path.to_owned(), value_name: name.clone(), approval_key: format!(r"{APPROVED}\{sub}"),
            });
            b.push(Entry {
                source: StartupSource::RunKey,
                scope,
                name: shown.clone(),
                definition: format!("{source} : {shown}"),
                target,
                check,
                enabled,
                publisher: None,
                task_path: None,
                fixed,
                note,
                removable: location.is_some(),
                location,
            });
        }
    }
}

/// `IShellLinkW` target of a shortcut, read without resolving. Must run on a COM-initialized thread.
fn link_target(path: &str) -> Option<String> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{HSTRING, Interface};
    // SAFETY: COM calls on an initialized thread; buffers are sized by their slices.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        file.Load(&HSTRING::from(path), STGM_READ).ok()?;
        let mut buf = vec![0u16; 32_768];
        link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
        let end = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
        clean_text(&String::from_utf16_lossy(&buf[..end]))
    }
}

fn known_folder(id: &windows::core::GUID) -> Option<String> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{KNOWN_FOLDER_FLAG, SHGetKnownFolderPath};
    // SAFETY: the returned buffer is freed with CoTaskMemFree exactly once.
    unsafe {
        let p = SHGetKnownFolderPath(id, KNOWN_FOLDER_FLAG(0), None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

fn read_startup_folders(b: &mut Builder, env: &StartupEnv) {
    use windows::Win32::UI::Shell::{FOLDERID_CommonStartup, FOLDERID_Startup};
    let folders = [
        (FOLDERID_Startup, StartupScope::User, Hive::Hkcu, &env.user_folder),
        (FOLDERID_CommonStartup, StartupScope::System, Hive::Hklm, &env.common_folder),
    ];
    for (id, scope, hive, replacement) in folders {
        let dir = match replacement {
            Some(p) => Some(p.to_string_lossy().into_owned()),
            // A redirected (test) registry never reads the real folders.
            None if env.is_redirected() => continue,
            None => known_folder(&id),
        };
        let approved = approvals(env, hive, "StartupFolder");
        match dir {
            Some(dir) => read_folder(b, env, &dir, scope, hive, &approved),
            None => b.issue("known_folder_unavailable", &format!("{} Startup folder", if scope == StartupScope::User { "Your" } else { "The all-users" }), "the Startup folder location could not be resolved".into()),
        }
    }
}

/// Entries of one Startup folder; `approved` holds Explorer's per-file enabled flags (lower-case file names).
fn read_folder(b: &mut Builder, env: &StartupEnv, dir: &str, scope: StartupScope, hive: Hive, approved: &HashMap<String, bool>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => return b.issue("folder_unreadable", dir, e.to_string()),
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if meta.is_dir() || file_name.eq_ignore_ascii_case("desktop.ini") {
            continue;
        }
        let full = format!(r"{dir}\{file_name}");
        let is_link = file_name.to_ascii_lowercase().ends_with(".lnk");
        let (target, check, note) = if is_link {
            match link_target(&full) {
                Some(t) => {
                    let c = check_path(&t);
                    (Some(t), c, None)
                }
                None => (None, TargetCheck::Unchecked, Some("Shortcut has no file-system target (advertised or URL shortcut)")),
            }
        } else {
            (Some(full.clone()), TargetCheck::Exists, None)
        };
        let stem = std::path::Path::new(&file_name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or(file_name.clone());
        let location = raw(dir).map(|folder| StartupLocation::FolderFile {
            root: env.registry_root.clone(), hive, folder, file_name: file_name.clone(), approval_key: format!(r"{APPROVED}\StartupFolder"),
        });
        b.push(Entry {
            source: StartupSource::StartupFolder,
            scope,
            name: stem,
            definition: full,
            target,
            check,
            enabled: approved.get(&file_name.to_lowercase()).copied().unwrap_or(true),
            publisher: None,
            task_path: None,
            fixed: None,
            note,
            location,
            removable: false,
        });
    }
}

const SERVICE_ACCOUNTS: &[&str] = &["s-1-5-18", "s-1-5-19", "s-1-5-20", "system", "nt authority\\system", "local service", "network service"];

fn read_tasks(b: &mut Builder) {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::Win32::System::TaskScheduler::{ITaskFolder, ITaskService, TaskScheduler};
    use windows::Win32::System::Variant::VARIANT;
    use windows::core::BSTR;
    const SOURCE: &str = "Task Scheduler";
    // SAFETY: COM calls on an initialized thread.
    let root: windows::core::Result<ITaskFolder> = unsafe {
        CoCreateInstance::<_, ITaskService>(&TaskScheduler, None, CLSCTX_INPROC_SERVER).and_then(|svc| {
            let empty = VARIANT::default();
            svc.Connect(&empty, &empty, &empty, &empty)?;
            svc.GetFolder(&BSTR::from("\\"))
        })
    };
    let root = match root {
        Ok(r) => r,
        Err(e) => return b.issue("task_scheduler_unavailable", SOURCE, format!("cannot connect to Task Scheduler ({e})")),
    };
    let mut denied = 0u32;
    let mut stack = vec![root];
    while let Some(folder) = stack.pop() {
        // SAFETY: COM property reads on a live folder object.
        let folder_path = unsafe { folder.Path() }.map(|p| p.to_string()).unwrap_or_default();
        // SAFETY: as above; TASK_ENUM_HIDDEN includes hidden tasks, nothing is modified.
        match unsafe { folder.GetTasks(windows::Win32::System::TaskScheduler::TASK_ENUM_HIDDEN.0) } {
            Ok(tasks) => {
                let count = unsafe { tasks.Count() }.unwrap_or(0);
                for i in 1..=count {
                    // SAFETY: 1-based index within Count.
                    if let Ok(task) = unsafe { tasks.get_Item(&VARIANT::from(i)) } {
                        task_entry(b, &task);
                    }
                }
            }
            Err(e) if e.code().0 as u32 == 0x8007_0005 => denied += 1,
            Err(e) => b.issue("task_folder_unreadable", &format!("{SOURCE} {folder_path}"), e.to_string()),
        }
        // SAFETY: as above.
        match unsafe { folder.GetFolders(0) } {
            Ok(subs) => {
                let count = unsafe { subs.Count() }.unwrap_or(0);
                for i in 1..=count {
                    // SAFETY: 1-based index within Count.
                    if let Ok(sub) = unsafe { subs.get_Item(&VARIANT::from(i)) } {
                        stack.push(sub);
                    }
                }
            }
            Err(e) if e.code().0 as u32 == 0x8007_0005 => denied += 1,
            Err(e) => b.issue("task_folder_unreadable", &format!("{SOURCE} {folder_path}"), e.to_string()),
        }
    }
    if denied > 0 {
        b.issue("permission_denied", SOURCE, format!("{denied} task folders could not be read with the current permissions"));
    }
}

fn task_entry(b: &mut Builder, task: &windows::Win32::System::TaskScheduler::IRegisteredTask) {
    use windows::Win32::System::TaskScheduler::{IExecAction, ILogonTrigger, TASK_ACTION_EXEC, TASK_ACTION_TYPE, TASK_TRIGGER_BOOT, TASK_TRIGGER_LOGON, TASK_TRIGGER_TYPE2};
    use windows::core::{BSTR, Interface};
    // SAFETY: COM property reads only; out-parameters are local and initialized.
    unsafe {
        let Ok(def) = task.Definition() else { return };
        let Ok(triggers) = def.Triggers() else { return };
        let mut starts = false;
        let mut machine_wide = false;
        let mut count = 0i32;
        let _ = triggers.Count(&mut count);
        for i in 1..=count {
            let Ok(t) = triggers.get_Item(i) else { continue };
            let mut kind = TASK_TRIGGER_TYPE2(0);
            if t.Type(&mut kind).is_err() {
                continue;
            }
            if kind == TASK_TRIGGER_BOOT {
                starts = true;
                machine_wide = true;
            } else if kind == TASK_TRIGGER_LOGON {
                starts = true;
                let mut user = BSTR::new();
                if let Ok(lt) = t.cast::<ILogonTrigger>() {
                    let _ = lt.UserId(&mut user);
                }
                if user.is_empty() {
                    machine_wide = true;
                }
            }
        }
        if !starts {
            return;
        }
        let path = task.Path().map(|p| p.to_string()).unwrap_or_default();
        let name = task.Name().map(|n| n.to_string()).unwrap_or_else(|_| path.clone());
        let enabled = task.Enabled().map(|v| v.0 != 0).unwrap_or(true);
        if let Ok(principal) = def.Principal() {
            let mut user = BSTR::new();
            let _ = principal.UserId(&mut user);
            if SERVICE_ACCOUNTS.contains(&user.to_string().to_lowercase().as_str()) {
                machine_wide = true;
            }
        }
        let mut target = None;
        let mut check = TargetCheck::Unchecked;
        let mut note = None;
        if let Ok(actions) = def.Actions() {
            let mut n = 0i32;
            let _ = actions.Count(&mut n);
            for i in 1..=n {
                let Ok(a) = actions.get_Item(i) else { continue };
                let mut kind = TASK_ACTION_TYPE(-1);
                if a.Type(&mut kind).is_err() || kind != TASK_ACTION_EXEC {
                    note = Some("Task action is not a program (COM handler or message)");
                    continue;
                }
                let Ok(exec) = a.cast::<IExecAction>() else { continue };
                let mut program = BSTR::new();
                let _ = exec.Path(&mut program);
                let quoted = format!("\"{}\"", program.to_string().trim_matches('"'));
                let (t, c, n2) = resolve_command(&quoted);
                target = t;
                check = c;
                note = n2;
                break;
            }
        }
        b.push(Entry {
            source: StartupSource::ScheduledTask,
            scope: if machine_wide { StartupScope::System } else { StartupScope::User },
            name,
            definition: format!("Task Scheduler {path}"),
            target,
            check,
            enabled,
            publisher: None,
            task_path: Some(path),
            fixed: None,
            note,
            location: None,
            removable: false,
        });
    }
}

/// Executable declared for `TaskId` in a package manifest (`windows.startupTask` extension). Plain text search: the
/// attribute pair lives on the same `Extension` element in every manifest schema version that supports it.
pub(crate) fn manifest_startup_executable(manifest: &str, task_id: &str) -> Option<String> {
    let needle = format!("TaskId=\"{task_id}\"");
    let at = manifest.find(&needle)?;
    let head = &manifest[..at];
    let ext_start = head.rfind("<")?;
    let ext_start = head[..ext_start].rfind("Extension").unwrap_or(ext_start);
    let attr = |window: &str| {
        let exe_at = window.rfind("Executable=\"")? + "Executable=\"".len();
        let rest = &window[exe_at..];
        clean_text(&rest[..rest.find('"')?])
    };
    // Without its own Executable the extension runs the enclosing Application's executable.
    attr(&manifest[ext_start..at]).or_else(|| {
        let app = head.rfind("<Application ")?;
        let tag_end = app + manifest[app..].find('>')?;
        attr(&manifest[app..tag_end])
    })
}

fn package_roots() -> HashMap<String, String> {
    // Package family name (Name_PublisherId) -> package root folder.
    let mut out = HashMap::new();
    let Ok(repo) = RegKey::open(HKEY_CURRENT_USER, PACKAGES, 0) else { return out };
    for full in repo.subkeys().unwrap_or_default() {
        let parts: Vec<&str> = full.split('_').collect();
        if parts.len() != 5 {
            continue;
        }
        let family = format!("{}_{}", parts[0], parts[4]).to_lowercase();
        if let Some(root) = repo.child(&full, 0).ok().and_then(|k| k.string("PackageRootFolder")) {
            out.entry(family).or_insert(root);
        }
    }
    out
}

fn read_store_tasks(b: &mut Builder) {
    let source = format!(r"HKCU\{SYSTEM_APP_DATA}");
    let apps = match RegKey::open(HKEY_CURRENT_USER, SYSTEM_APP_DATA, 0) {
        Ok(k) => k,
        Err(ERROR_FILE_NOT_FOUND) => return,
        Err(rc) => return b.issue("registry_open_failed", &source, format!("cannot open packaged-app data (error {rc})")),
    };
    let roots = package_roots();
    for family in apps.subkeys().unwrap_or_default() {
        let Ok(fk) = apps.child(&family, 0) else { continue };
        for task_id in fk.subkeys().unwrap_or_default() {
            let Some(state) = fk.child(&task_id, 0).ok().and_then(|k| k.dword("State")) else { continue };
            if state > 4 {
                continue;
            }
            let publisher_id = family.rsplit('_').next().unwrap_or_default();
            let root = roots.get(&family.to_lowercase());
            let target = root.and_then(|r| {
                let manifest = std::fs::read_to_string(format!(r"{r}\AppxManifest.xml")).ok()?;
                manifest_startup_executable(&manifest, &task_id).map(|exe| format!(r"{r}\{}", exe.replace('/', "\\")))
            });
            let check = target.as_deref().map(check_path).unwrap_or(TargetCheck::Unchecked);
            let fixed: Option<&'static str> = matches!(state, 3 | 4).then_some("Set by policy (StartupTask DisabledByPolicy/EnabledByPolicy)");
            let windows_pkg = publisher_id == "cw5n1h2txyewy";
            let note = match (root, &target) {
                (None, _) => Some("Package is not registered for this user; target unknown"),
                (Some(_), None) => Some("The startup executable could not be read from the package manifest; target unknown"),
                _ => None,
            };
            b.push(Entry {
                source: StartupSource::StoreStartupTask,
                scope: StartupScope::User,
                name: format!("{} ({task_id})", family.split('_').next().unwrap_or(&family)),
                definition: format!(r"{source}\{family}\{task_id}"),
                target,
                check,
                enabled: matches!(state, 2 | 4),
                publisher: windows_pkg.then(|| ("Microsoft Windows".to_owned(), PublisherEvidence::PackageIdentity)),
                task_path: None,
                fixed,
                note,
                location: None,
                removable: false,
            });
        }
    }
}

/// Run `f` on a fresh thread with COM initialized (single-threaded apartment), as the shell and Task Scheduler
/// objects expect. Returns `None` if the thread panicked.
pub(crate) fn on_com_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize};
    std::thread::spawn(move || {
        // SAFETY: fresh thread; balanced with CoUninitialize when it succeeded.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        let out = f();
        if hr.is_ok() {
            // SAFETY: matches the successful CoInitializeEx on this thread; all COM objects were dropped in `f`.
            unsafe { CoUninitialize() };
        }
        out
    })
    .join()
    .ok()
}

/// Read-only inventory of startup items on this machine. Sources that fail are reported as issues (the list is
/// then incomplete, not empty). Nothing is enabled, disabled, deleted, started or written.
pub fn inventory() -> (Vec<StartupItem>, Vec<InventoryIssue>) {
    inventory_with(&StartupEnv::real())
}

/// [`inventory`] of an explicit environment (a redirected test registry reads only its Run keys and given folders).
pub fn inventory_with(env: &StartupEnv) -> (Vec<StartupItem>, Vec<InventoryIssue>) {
    let env = env.clone();
    let out = on_com_thread(move || {
        let mut b = Builder { items: Vec::new(), issues: Vec::new(), signers: HashMap::new(), is_admin: env.is_admin };
        read_run_keys(&mut b, &env);
        read_startup_folders(&mut b, &env);
        if !env.is_redirected() {
            read_tasks(&mut b);
            read_store_tasks(&mut b);
        }
        (b.items, b.issues)
    });
    match out {
        Some((items, issues)) => (normalize(items), issues),
        None => (Vec::new(), vec![InventoryIssue { code: "internal_error".into(), source: "startup inventory".into(), message: "inventory thread failed".into() }]),
    }
}

#[cfg(test)]
mod tests;
