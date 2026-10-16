//! Windows startup changes (parity 11.2/11.3, STA-02/03).
//!
//! - Enable/disable writes only Explorer's enable record (`…\Explorer\StartupApproved\{Run,Run32,StartupFolder}`, the
//!   value Task Manager writes: 0x02 = enabled, 0x03 + FILETIME = disabled). The `Run` value or Startup-folder file is
//!   never touched, so the change is reversible by writing the prior record back.
//! - A stale `Run` value (target missing on a mounted volume) is deleted only after the caller stored its backup
//!   (value type and data, and its enable record); a backup failure stops before anything is deleted.
//! - Every change first re-reads the live state and compares its digest with the planned one; anything changed since
//!   planning is skipped. Restores never overwrite: a value recreated since is left alone, and an enable record changed
//!   since is left alone.
//! - Machine-wide (HKLM) locations need an elevated token; this module never elevates and refuses without one.
//! - All keys go through [`StartupEnv`], so tests act on a throwaway key under `HKCU\Software\CleanManagerTest\`.

use cm_core::plan::v2::Hive;
use cm_core::startup::{
    RegValue, RemovalBackup, StartupActions, StartupBackup, StartupLocation, StartupObservation, StartupOutcome, TargetCheck, ToggleBackup, approval_record,
    observation_digest,
};
use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_BINARY, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};

use super::startup::StartupEnv;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Whether the process token is elevated (an administrator's full token). Only queried, never acquired.
pub fn is_elevated() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: current-process pseudo handle; valid out-pointer.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return false;
    }
    let mut info = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut len = 0u32;
    // SAFETY: buffer is a TOKEN_ELEVATION of the declared size.
    let ok = unsafe { GetTokenInformation(token, TokenElevation, (&mut info as *mut TOKEN_ELEVATION).cast(), size_of::<TOKEN_ELEVATION>() as u32, &mut len) };
    // SAFETY: handle from OpenProcessToken, closed once.
    unsafe { CloseHandle(token) };
    ok != 0 && info.TokenIsElevated != 0
}

/// A key opened for this module's reads and writes; closed on drop.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: handle from a successful open/create, closed once.
        unsafe { RegCloseKey(self.0) };
    }
}

impl Key {
    fn open(root: HKEY, sub: &str, access: u32) -> Result<Key, u32> {
        let z = wide(sub);
        let mut out: HKEY = std::ptr::null_mut();
        // SAFETY: NUL-terminated subkey, valid out-pointer.
        let rc = unsafe { RegOpenKeyExW(root, z.as_ptr(), 0, access, &mut out) };
        if rc == ERROR_SUCCESS { Ok(Key(out)) } else { Err(rc) }
    }

    /// Open, creating the key (and missing parents) if needed.
    fn create(root: HKEY, sub: &str, access: u32) -> Result<Key, u32> {
        let z = wide(sub);
        let mut out: HKEY = std::ptr::null_mut();
        // SAFETY: NUL-terminated subkey; no class, default security; valid out-pointer.
        let rc = unsafe {
            RegCreateKeyExW(root, z.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, access, std::ptr::null(), &mut out, std::ptr::null_mut())
        };
        if rc == ERROR_SUCCESS { Ok(Key(out)) } else { Err(rc) }
    }

    /// `Ok(None)` when the value does not exist.
    fn query(&self, name: &str) -> Result<Option<(u32, Vec<u8>)>, u32> {
        let wname = wide(name);
        let mut size = 0u32;
        let mut kind = 0u32;
        for _ in 0..4 {
            // SAFETY: size query with a null data pointer.
            let rc = unsafe { RegQueryValueExW(self.0, wname.as_ptr(), std::ptr::null(), &mut kind, std::ptr::null_mut(), &mut size) };
            match rc {
                ERROR_SUCCESS => {}
                ERROR_FILE_NOT_FOUND => return Ok(None),
                other => return Err(other),
            }
            let mut data = vec![0u8; size as usize];
            let mut got = size;
            // SAFETY: data buffer of `got` bytes.
            let rc = unsafe { RegQueryValueExW(self.0, wname.as_ptr(), std::ptr::null(), &mut kind, data.as_mut_ptr(), &mut got) };
            match rc {
                ERROR_SUCCESS => {
                    data.truncate(got as usize);
                    return Ok(Some((kind, data)));
                }
                ERROR_MORE_DATA => size = got,
                ERROR_FILE_NOT_FOUND => return Ok(None),
                other => return Err(other),
            }
        }
        Err(ERROR_MORE_DATA)
    }

    fn set(&self, name: &str, kind: u32, data: &[u8]) -> Result<(), u32> {
        let wname = wide(name);
        // SAFETY: NUL-terminated name; data buffer of the given length.
        let rc = unsafe { RegSetValueExW(self.0, wname.as_ptr(), 0, kind, data.as_ptr(), data.len() as u32) };
        if rc == ERROR_SUCCESS { Ok(()) } else { Err(rc) }
    }

    fn delete(&self, name: &str) -> Result<(), u32> {
        let wname = wide(name);
        // SAFETY: NUL-terminated value name.
        match unsafe { RegDeleteValueW(self.0, wname.as_ptr()) } {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            other => Err(other),
        }
    }
}

fn os_error(rc: u32) -> String {
    std::io::Error::from_raw_os_error(rc as i32).to_string()
}

fn failed(code: &str, message: impl Into<String>) -> StartupOutcome {
    StartupOutcome::Failed { code: code.into(), message: message.into() }
}

fn skipped(code: &str, message: impl Into<String>) -> StartupOutcome {
    StartupOutcome::Skipped { code: code.into(), message: message.into() }
}

fn write_refused(rc: u32, hive: Hive) -> StartupOutcome {
    match (rc, hive) {
        (ERROR_ACCESS_DENIED, Hive::Hklm) => failed("permission_required", format!("{}; {}", os_error(rc), cm_core::startup::ADMIN_REQUIRED)),
        (ERROR_ACCESS_DENIED, _) => failed("access_denied", os_error(rc)),
        _ => failed("write_failed", os_error(rc)),
    }
}

fn now_filetime() -> u64 {
    let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() / 100).unwrap_or(0) as u64;
    unix + 116_444_736_000_000_000
}

/// Windows implementation of startup changes over one [`StartupEnv`].
#[derive(Debug, Clone)]
pub struct WindowsStartup {
    pub env: StartupEnv,
}

impl WindowsStartup {
    fn same_root(&self, loc: &StartupLocation) -> Result<(), StartupOutcome> {
        if loc.root() == self.env.registry_root {
            Ok(())
        } else {
            Err(skipped("root_mismatch", "the plan was made against another registry root; nothing changed"))
        }
    }

    /// (root, subkey, flags) of the enable-record key. Approval keys are always in the 64-bit view.
    fn approval_key(&self, loc: &StartupLocation) -> (HKEY, String, u32) {
        self.env.physical(loc.hive(), loc.approval_key(), cm_core::startup::RegView::Wow64_64)
    }

    fn read_approval(&self, loc: &StartupLocation) -> Result<Option<RegValue>, String> {
        let (root, sub, flags) = self.approval_key(loc);
        match Key::open(root, &sub, KEY_QUERY_VALUE | flags) {
            Ok(k) => k.query(loc.approval_value()).map(|v| v.map(|(t, d)| RegValue::new(t, &d))).map_err(os_error),
            Err(ERROR_FILE_NOT_FOUND) => Ok(None),
            Err(rc) => Err(format!("cannot open the enable records: {}", os_error(rc))),
        }
    }

    /// Write (`Some`) or remove (`None`) the enable record.
    fn write_approval(&self, loc: &StartupLocation, value: Option<&RegValue>) -> Result<(), u32> {
        let (root, sub, flags) = self.approval_key(loc);
        match value {
            Some(v) => {
                let data = v.data().ok_or(87u32)?; // ERROR_INVALID_PARAMETER
                Key::create(root, &sub, KEY_SET_VALUE | KEY_QUERY_VALUE | flags)?.set(loc.approval_value(), v.value_type, &data)
            }
            None => match Key::open(root, &sub, KEY_SET_VALUE | flags) {
                Ok(k) => k.delete(loc.approval_value()),
                Err(ERROR_FILE_NOT_FOUND) => Ok(()),
                Err(rc) => Err(rc),
            },
        }
    }

    fn run_key(&self, loc: &StartupLocation, access: u32, create: bool) -> Result<Option<Key>, u32> {
        let StartupLocation::RunValue { hive, view, key, .. } = loc else { return Ok(None) };
        let (root, sub, flags) = self.env.physical(*hive, key, *view);
        let r = if create { Key::create(root, &sub, access | flags) } else { Key::open(root, &sub, access | flags) };
        match r {
            Ok(k) => Ok(Some(k)),
            Err(ERROR_FILE_NOT_FOUND) => Ok(None),
            Err(rc) => Err(rc),
        }
    }

    fn checked(&self, loc: &StartupLocation, expected: &str) -> Result<StartupObservation, StartupOutcome> {
        self.same_root(loc)?;
        let obs = self.observe(loc).map_err(|e| failed("check_failed", e))?;
        if observation_digest(loc, &obs) != expected {
            return Err(skipped("changed_since_plan", "the entry or its enable state changed after planning; nothing changed"));
        }
        if obs.definition.is_none() {
            return Err(skipped("missing", "the entry no longer exists"));
        }
        Ok(obs)
    }
}

impl StartupActions for WindowsStartup {
    fn observe(&self, loc: &StartupLocation) -> Result<StartupObservation, String> {
        let definition = match loc {
            StartupLocation::RunValue { value_name, .. } => match self.run_key(loc, KEY_QUERY_VALUE, false) {
                Ok(Some(k)) => k.query(value_name).map_err(os_error)?.map(|(t, d)| RegValue::new(t, &d)),
                Ok(None) => None,
                Err(rc) => return Err(format!("cannot open the Run key: {}", os_error(rc))),
            },
            StartupLocation::FolderFile { folder, file_name, .. } => {
                let path = std::path::PathBuf::from(folder.lossy()).join(file_name);
                let w: Vec<u16> = std::os::windows::ffi::OsStrExt::encode_wide(path.as_os_str()).collect();
                match super::directory::identity_of(&w) {
                    Ok((vol, fid)) => Some(RegValue::new(0, format!("{vol}|{fid}").as_bytes())),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => return Err(format!("cannot identify the Startup-folder file: {e}")),
                }
            }
            StartupLocation::LaunchdUser { .. } | StartupLocation::LaunchdSystem { .. } | StartupLocation::SmItem { .. } => {
                return Err("not a Windows startup location".into());
            }
        };
        Ok(StartupObservation { definition, approval: self.read_approval(loc)? })
    }

    fn is_admin(&self) -> bool {
        self.env.is_admin
    }

    fn set_enabled(&self, loc: &StartupLocation, enabled: bool, expected: &str, backup: ToggleBackup<'_>) -> StartupOutcome {
        let obs = match self.checked(loc, expected) {
            Ok(o) => o,
            Err(out) => return out,
        };
        let written = RegValue::new(REG_BINARY, &approval_record(enabled, now_filetime()));
        // The prior state is stored with the operation first; without it nothing is changed.
        if let Err(e) = backup(&obs, Some(&written)) {
            return failed("backup_failed", format!("{e}; nothing changed"));
        }
        if let Err(rc) = self.write_approval(loc, Some(&written)) {
            return write_refused(rc, loc.hive());
        }
        match self.read_approval(loc) {
            Ok(Some(v)) if v == written => StartupOutcome::Done,
            Ok(_) => StartupOutcome::Unknown { message: "the enable record does not read back as written".into() },
            Err(e) => StartupOutcome::Unknown { message: format!("cannot read the enable record back: {e}") },
        }
    }

    fn remove_stale(&self, loc: &StartupLocation, expected: &str, backup: RemovalBackup<'_>) -> StartupOutcome {
        let StartupLocation::RunValue { value_name, hive, .. } = loc else { return skipped("unsupported", "only Run values are removed") };
        let obs = match self.checked(loc, expected) {
            Ok(o) => o,
            Err(out) => return out,
        };
        let def = obs.definition.as_ref().expect("checked");
        if def.value_type != REG_SZ && def.value_type != REG_EXPAND_SZ {
            return skipped("unsupported", "the value is not a command line");
        }
        let data = def.data().unwrap_or_default();
        let units: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
        // STA-03: still stale right now — missing on a mounted volume. Offline or unchecked targets are never removed.
        match super::startup::resolve_command(&String::from_utf16_lossy(&units)).1 {
            TargetCheck::Missing => {}
            TargetCheck::VolumeOffline => return skipped("target_unreachable", "the target volume is offline: state unknown, not stale; nothing removed"),
            _ => return skipped("not_stale", "the target exists again or cannot be checked; nothing removed"),
        }
        if let Err(e) = backup(&obs) {
            return failed("backup_failed", format!("{e}; nothing removed"));
        }
        let key = match self.run_key(loc, KEY_SET_VALUE | KEY_QUERY_VALUE, false) {
            Ok(Some(k)) => k,
            Ok(None) => return skipped("missing", "the Run key no longer exists"),
            Err(rc) => return write_refused(rc, *hive),
        };
        if let Err(rc) = key.delete(value_name) {
            return write_refused(rc, *hive);
        }
        // Its enable record would be orphaned; it is in the backup too.
        let _ = self.write_approval(loc, None);
        match self.observe(loc) {
            Ok(o) if o.definition.is_none() => StartupOutcome::Done,
            Ok(_) => StartupOutcome::Unknown { message: "the value is still present after the delete".into() },
            Err(e) => StartupOutcome::Unknown { message: format!("cannot confirm the delete: {e}") },
        }
    }

    fn restore_approval(&self, loc: &StartupLocation, written: Option<&RegValue>, prior: Option<&RegValue>) -> StartupOutcome {
        if let Err(out) = self.same_root(loc) {
            return out;
        }
        let obs = match self.observe(loc) {
            Ok(o) => o,
            Err(e) => return failed("check_failed", e),
        };
        if obs.definition.is_none() {
            return skipped("missing", "the entry no longer exists; nothing restored");
        }
        if obs.approval.as_ref() != written {
            return skipped("changed_since", "the enable state was changed after this operation; it is left as it is");
        }
        if let Err(rc) = self.write_approval(loc, prior) {
            return write_refused(rc, loc.hive());
        }
        match self.read_approval(loc) {
            Ok(now) if now.as_ref() == prior => StartupOutcome::Done,
            Ok(_) => StartupOutcome::Unknown { message: "the enable record does not read back as restored".into() },
            Err(e) => StartupOutcome::Unknown { message: format!("cannot read the enable record back: {e}") },
        }
    }

    fn restore_value(&self, b: &StartupBackup) -> StartupOutcome {
        let loc = &b.location;
        let StartupLocation::RunValue { value_name, hive, .. } = loc else { return skipped("unsupported", "only Run values are restored") };
        if let Err(out) = self.same_root(loc) {
            return out;
        }
        let Some(prior) = &b.prior_definition else { return failed("backup_incomplete", "the backup holds no value") };
        let Some(data) = prior.data() else { return failed("backup_incomplete", "the backup data is unreadable") };
        let obs = match self.observe(loc) {
            Ok(o) => o,
            Err(e) => return failed("check_failed", e),
        };
        if obs.definition.is_some() {
            return skipped("destination_exists", "a value with this name exists again; nothing overwritten");
        }
        let key = match self.run_key(loc, KEY_SET_VALUE | KEY_QUERY_VALUE, true) {
            Ok(Some(k)) => k,
            Ok(None) => return failed("write_failed", "the Run key cannot be opened"),
            Err(rc) => return write_refused(rc, *hive),
        };
        // Last check right before the write: still absent (RegSetValue would replace).
        match key.query(value_name) {
            Ok(None) => {}
            Ok(Some(_)) => return skipped("destination_exists", "a value with this name appeared; nothing overwritten"),
            Err(rc) => return failed("check_failed", os_error(rc)),
        }
        if let Err(rc) = key.set(value_name, prior.value_type, &data) {
            return write_refused(rc, *hive);
        }
        if b.prior_approval.is_some() && obs.approval.is_none() {
            let _ = self.write_approval(loc, b.prior_approval.as_ref());
        }
        match self.observe(loc) {
            Ok(o) if o.definition.as_ref() == Some(prior) => StartupOutcome::Done,
            Ok(_) => StartupOutcome::Unknown { message: "the value does not read back as restored".into() },
            Err(e) => StartupOutcome::Unknown { message: format!("cannot confirm the restore: {e}") },
        }
    }
}

#[cfg(test)]
mod tests;
