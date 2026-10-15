//! `self install --dir PATH` (MVP 14.1, CLI-01): copy this release layout into a directory the
//! person chose, never over anything that already exists, and report every other `cleanmanager`
//! on PATH instead of replacing or shadowing it.
//!
//! - The source must be a complete release layout (every file of [`RELEASE_FILE`] present with its
//!   SHA-256); the target must be new or empty. Nothing outside the target directory is written,
//!   except the optional user PATH entry.
//! - `--add-to-path` appends the directory to the *user* PATH (never the system PATH) only when no
//!   different `cleanmanager` is reachable on PATH; the exact entry is recorded in
//!   [`INSTALL_STATE_FILE`] so `self uninstall` removes exactly that entry and nothing else.
//! - A failed copy removes what it created; the source is never modified.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::release::{self, INSTALL_STATE_FILE, InstallState, RELEASE_FILE, ReleaseManifest, exe_name, safe_relative, same_path};

/// Another `cleanmanager` reachable through PATH.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathHit {
    pub file: PathBuf,
    /// `same_release` (byte-identical program) or `different`.
    pub relation: &'static str,
}

/// Every `cleanmanager` command file reachable through `path_var`, in PATH order.
pub fn path_hits(path_var: Option<&OsStr>, our_sha256: Option<&str>) -> Vec<PathHit> {
    let names: &[&str] = if cfg!(windows) { &["cleanmanager.exe", "cleanmanager.com", "cleanmanager.cmd", "cleanmanager.bat", "cleanmanager.ps1"] } else { &["cleanmanager"] };
    let mut out: Vec<PathHit> = Vec::new();
    for dir in path_var.map(std::env::split_paths).into_iter().flatten() {
        let dir = PathBuf::from(dir.to_string_lossy().trim_matches('"'));
        if dir.as_os_str().is_empty() {
            continue;
        }
        for n in names {
            let file = dir.join(n);
            if !file.is_file() || out.iter().any(|h| same_path(&h.file, &file)) {
                continue;
            }
            let same = our_sha256.is_some_and(|ours| release::sha256_file(&file).is_ok_and(|(h, _)| h == ours));
            out.push(PathHit { file, relation: if same { "same_release" } else { "different" } });
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// User PATH

/// The per-user PATH value (`HKCU\Environment\Path` on Windows). Tests use an in-memory store.
pub trait UserPathStore {
    /// The value and whether it is an expandable string (`REG_EXPAND_SZ`).
    fn read(&self) -> Result<Option<(String, bool)>, String>;
    fn write(&self, value: &str, expandable: bool) -> Result<(), String>;
}

fn norm_entry(e: &str) -> String {
    let t = e.trim().trim_matches('"').trim_end_matches(['\\', '/']);
    if cfg!(windows) { t.to_lowercase() } else { t.to_owned() }
}

fn sep() -> char {
    if cfg!(windows) { ';' } else { ':' }
}

pub fn path_contains(store: &dyn UserPathStore, dir: &str) -> Result<bool, String> {
    Ok(store.read()?.is_some_and(|(v, _)| v.split(sep()).any(|e| norm_entry(e) == norm_entry(dir))))
}

/// Append `dir` (no-op when present). Returns whether the value changed.
pub fn append_entry(store: &dyn UserPathStore, dir: &str) -> Result<bool, String> {
    let (value, expandable) = store.read()?.unwrap_or((String::new(), true));
    if value.split(sep()).any(|e| norm_entry(e) == norm_entry(dir)) {
        return Ok(false);
    }
    let trimmed = value.trim_end_matches(sep());
    let new = if trimmed.is_empty() { dir.to_owned() } else { format!("{trimmed}{}{dir}", sep()) };
    store.write(&new, expandable)?;
    Ok(true)
}

/// Remove exactly the entries equal to `dir`; every other entry (and its order) is kept.
pub fn remove_entry(store: &dyn UserPathStore, dir: &str) -> Result<bool, String> {
    let Some((value, expandable)) = store.read()? else { return Ok(false) };
    let kept: Vec<&str> = value.split(sep()).filter(|e| norm_entry(e) != norm_entry(dir)).collect();
    let new = kept.join(&sep().to_string());
    if new == value {
        return Ok(false);
    }
    store.write(&new, expandable)?;
    Ok(true)
}

#[cfg(windows)]
pub mod user_path {
    //! `HKCU\<subkey>\Path` through the registry API, preserving `REG_EXPAND_SZ`.
    use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegQueryValueExW,
        RegSetValueExW,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub struct Registry {
        /// Under HKCU: `Environment` for the real user PATH; tests pass a scratch key.
        pub subkey: String,
    }

    struct Key(HKEY);

    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: opened by RegCreateKeyExW below and closed once.
            unsafe { RegCloseKey(self.0) };
        }
    }

    impl Registry {
        fn open(&self) -> Result<Key, String> {
            let sub = wide(&self.subkey);
            let mut hk: HKEY = std::ptr::null_mut();
            // SAFETY: valid NUL-terminated subkey, out-pointer to a local handle.
            let rc = unsafe {
                RegCreateKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_QUERY_VALUE | KEY_SET_VALUE, std::ptr::null(), &mut hk, std::ptr::null_mut())
            };
            if rc != 0 {
                return Err(format!("cannot open HKCU\\{} (error {rc})", self.subkey));
            }
            Ok(Key(hk))
        }
    }

    impl super::UserPathStore for Registry {
        fn read(&self) -> Result<Option<(String, bool)>, String> {
            let key = self.open()?;
            let name = wide("Path");
            let (mut kind, mut size) = (0u32, 0u32);
            // SAFETY: size query with a null buffer.
            let rc = unsafe { RegQueryValueExW(key.0, name.as_ptr(), std::ptr::null(), &mut kind, std::ptr::null_mut(), &mut size) };
            if rc == ERROR_FILE_NOT_FOUND {
                return Ok(None);
            }
            if rc != 0 {
                return Err(format!("cannot read the user PATH (error {rc})"));
            }
            if kind != REG_SZ && kind != REG_EXPAND_SZ {
                return Err("the user PATH is not a string value; it is left unchanged".into());
            }
            let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
            let mut got = (buf.len() * 2) as u32;
            // SAFETY: buffer of `got` bytes.
            let rc = unsafe { RegQueryValueExW(key.0, name.as_ptr(), std::ptr::null(), &mut kind, buf.as_mut_ptr().cast(), &mut got) };
            if rc != 0 {
                return Err(format!("cannot read the user PATH (error {rc})"));
            }
            let units = &buf[..(got as usize / 2)];
            let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
            Ok(Some((String::from_utf16(&units[..end]).map_err(|_| "the user PATH is not valid UTF-16; it is left unchanged".to_string())?, kind == REG_EXPAND_SZ)))
        }

        fn write(&self, value: &str, expandable: bool) -> Result<(), String> {
            let key = self.open()?;
            let name = wide("Path");
            let data = wide(value);
            // SAFETY: data is a NUL-terminated UTF-16 buffer of the given byte length.
            let rc = unsafe {
                RegSetValueExW(key.0, name.as_ptr(), 0, if expandable { REG_EXPAND_SZ } else { REG_SZ }, data.as_ptr().cast(), (data.len() * 2) as u32)
            };
            if rc != 0 {
                return Err(format!("cannot write the user PATH (error {rc})"));
            }
            if self.subkey.eq_ignore_ascii_case("Environment") {
                broadcast();
            }
            Ok(())
        }
    }

    /// Tell running programs (Explorer, new terminals) that the environment changed.
    fn broadcast() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE};
        let what = wide("Environment");
        let mut result = 0usize;
        // SAFETY: broadcast with a NUL-terminated string that outlives the bounded (5 s) call.
        unsafe { SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, 0, what.as_ptr() as isize, SMTO_ABORTIFHUNG, 5000, &mut result) };
    }

    /// Delete a scratch test key.
    #[cfg(test)]
    pub fn delete_test_key(subkey: &str) {
        let sub = wide(subkey);
        // SAFETY: valid NUL-terminated subkey under HKCU.
        unsafe { windows_sys::Win32::System::Registry::RegDeleteTreeW(HKEY_CURRENT_USER, sub.as_ptr()) };
    }
}

/// The real user PATH store, where one exists.
pub fn os_user_path() -> Option<Box<dyn UserPathStore>> {
    #[cfg(windows)]
    {
        Some(Box::new(user_path::Registry { subkey: "Environment".into() }))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

// ---------------------------------------------------------------------------------------------
// Plan and copy

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    pub source_dir: PathBuf,
    pub target_dir: PathBuf,
    pub release: ReleaseManifest,
    pub hits: Vec<PathHit>,
    /// `not_requested`, `would_add`, `already_present`, `refused_conflict`.
    pub path_entry: &'static str,
}

#[derive(Debug)]
pub enum InstallError {
    Invalid(String),
    Conflict(String),
    Io(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::Invalid(s) | InstallError::Conflict(s) | InstallError::Io(s) => f.write_str(s),
        }
    }
}

/// Check the source layout and the chosen directory; nothing is written.
pub fn plan(source_dir: &Path, target: &Path, add_to_path: bool, path_var: Option<&OsStr>, user_path: Option<&dyn UserPathStore>) -> Result<InstallPlan, InstallError> {
    if !target.is_absolute() {
        return Err(InstallError::Invalid("--dir must be an absolute path".into()));
    }
    let target = release::normalize(target);
    let release = ReleaseManifest::load(source_dir).map_err(InstallError::Invalid)?.ok_or_else(|| {
        InstallError::Invalid(format!("this copy is not a release layout (no {RELEASE_FILE} next to the program); install from the release zip"))
    })?;
    for f in &release.files {
        let p = source_dir.join(safe_relative(&f.path).map_err(InstallError::Invalid)?);
        match release::sha256_file(&p) {
            Ok((h, n)) if h == f.sha256 && n == f.size => {}
            _ => return Err(InstallError::Invalid(format!("the release layout is incomplete or modified ({}); reinstall it from the release zip", f.path))),
        }
    }
    if same_path(source_dir, &target) || target.starts_with(source_dir) || source_dir.starts_with(&target) {
        return Err(InstallError::Invalid("--dir must not be the running copy's directory or overlap it".into()));
    }
    if target.exists() {
        if !target.is_dir() {
            return Err(InstallError::Conflict(format!("{} exists and is not a directory", release::display(&target))));
        }
        if let Ok(Some(existing)) = ReleaseManifest::load(&target) {
            return Err(InstallError::Conflict(format!(
                "CleanManager {} is already installed in {}; update that copy with its own `self update` instead of installing over it",
                existing.version,
                release::display(&target)
            )));
        }
        if std::fs::read_dir(&target).map_err(|e| InstallError::Io(e.to_string()))?.next().is_some() {
            return Err(InstallError::Conflict(format!("{} is not empty; choose a new or empty directory (nothing there is overwritten)", release::display(&target))));
        }
    }
    let ours = release.files.iter().find(|f| f.path == exe_name()).map(|f| f.sha256.clone());
    let hits = path_hits(path_var, ours.as_deref());
    let path_entry = if !add_to_path {
        "not_requested"
    } else if hits.iter().any(|h| h.relation == "different") {
        "refused_conflict"
    } else if user_path.and_then(|u| path_contains(u, &target.display().to_string()).ok()).unwrap_or(false) {
        "already_present"
    } else if user_path.is_none() {
        "unsupported"
    } else {
        "would_add"
    };
    Ok(InstallPlan { source_dir: source_dir.to_path_buf(), target_dir: target, release, hits, path_entry })
}

/// Copy, verify, record; then add the PATH entry when planned. Returns the final PATH status.
pub fn execute(p: &InstallPlan, user_path: Option<&dyn UserPathStore>) -> Result<&'static str, InstallError> {
    let mut created_dirs: Vec<PathBuf> = Vec::new();
    let mut created_files: Vec<PathBuf> = Vec::new();
    let result = (|| -> Result<(), String> {
        for d in p.target_dir.ancestors().collect::<Vec<_>>().into_iter().rev() {
            if !d.exists() {
                std::fs::create_dir(d).map_err(|e| format!("cannot create {}: {e}", release::display(d)))?;
                created_dirs.push(d.to_path_buf());
            }
        }
        for f in &p.release.files {
            let rel = safe_relative(&f.path)?;
            let dest = p.target_dir.join(&rel);
            for d in dest.ancestors().skip(1).take_while(|d| *d != p.target_dir).collect::<Vec<_>>().into_iter().rev() {
                if !d.exists() {
                    std::fs::create_dir(d).map_err(|e| format!("cannot create {}: {e}", release::display(d)))?;
                    created_dirs.push(d.to_path_buf());
                }
            }
            copy_new(&p.source_dir.join(&rel), &dest)?;
            created_files.push(dest.clone());
            match release::sha256_file(&dest) {
                Ok((h, n)) if h == f.sha256 && n == f.size => {}
                _ => return Err(format!("the copied {} does not match the release manifest", f.path)),
            }
        }
        let manifest = ReleaseManifest { install_source: "self_install".into(), ..p.release.clone() };
        write_new(&p.target_dir.join(RELEASE_FILE), &manifest.to_json())?;
        created_files.push(p.target_dir.join(RELEASE_FILE));
        Ok(())
    })();
    if let Err(e) = result {
        for f in created_files.iter().rev() {
            let _ = std::fs::remove_file(f);
        }
        for d in created_dirs.iter().rev() {
            let _ = std::fs::remove_dir(d);
        }
        return Err(InstallError::Io(format!("{e}; everything this install created was removed")));
    }
    let mut state = InstallState { installed_at: cm_core::time::now_utc(), path_entry_added: None };
    let status = match (p.path_entry, user_path) {
        ("would_add", Some(store)) => match append_entry(store, &p.target_dir.display().to_string()) {
            Ok(true) => {
                state.path_entry_added = Some(p.target_dir.display().to_string());
                "added"
            }
            Ok(false) => "already_present",
            Err(_) => "failed",
        },
        (other, _) => other,
    };
    write_new(&p.target_dir.join(INSTALL_STATE_FILE), &state.to_json()).map_err(InstallError::Io)?;
    Ok(status)
}

fn copy_new(from: &Path, to: &Path) -> Result<(), String> {
    let mut src = std::fs::File::open(from).map_err(|e| format!("cannot read {}: {e}", release::display(from)))?;
    let mut dst = std::fs::OpenOptions::new().write(true).create_new(true).open(to).map_err(|e| format!("cannot create {}: {e}", release::display(to)))?;
    std::io::copy(&mut src, &mut dst).map_err(|e| format!("cannot copy to {}: {e}", release::display(to)))?;
    dst.sync_all().map_err(|e| e.to_string())
}

fn write_new(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(path).map_err(|e| format!("cannot create {}: {e}", release::display(path)))?;
    f.write_all(text.as_bytes()).and_then(|()| f.sync_all()).map_err(|e| e.to_string())
}

#[cfg(test)]
pub mod tests;
