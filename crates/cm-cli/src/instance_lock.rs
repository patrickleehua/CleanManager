//! Single-instance lock for product-lifecycle writes (LIF-01 "one update/install at a time").
//!
//! `<store>/lifecycle.lock` is an OS file lock (`LockFileEx` on Windows, `flock` elsewhere), so a
//! crashed process never leaves a stale lock behind:
//! - `self update`, `self install` and the confirmed `self uninstall` hold it **exclusively**;
//! - every other command that creates plans or changes CleanManager's own files holds it **shared**
//!   for its whole run, so an update never swaps the program or migrates the stores under a
//!   running write, and no new write starts while an update is replacing files.
//!
//! Shared holders never block each other. A second exclusive holder, or a write during an
//! exclusive hold, fails at once with `conflict` instead of waiting.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

pub const LOCK_FILE: &str = "lifecycle.lock";

/// Held until dropped.
#[derive(Debug)]
pub struct LifecycleLock {
    _file: File,
}

#[derive(Debug)]
pub enum LockError {
    /// Another process holds the lock in a conflicting mode.
    Busy(PathBuf),
    Io(PathBuf, std::io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::Busy(p) => write!(f, "another CleanManager update, install or uninstall is in progress (lock {}); try again when it has finished", p.display()),
            LockError::Io(p, e) => write!(f, "cannot open the lifecycle lock {}: {e}", p.display()),
        }
    }
}

fn open(dir: &Path) -> Result<(File, PathBuf), LockError> {
    let path = dir.join(LOCK_FILE);
    std::fs::create_dir_all(dir).map_err(|e| LockError::Io(path.clone(), e))?;
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path).map_err(|e| LockError::Io(path.clone(), e))?;
    Ok((file, path))
}

/// Exclusive hold for an update, install or uninstall.
pub fn exclusive(dir: &Path) -> Result<LifecycleLock, LockError> {
    let (file, path) = open(dir)?;
    match file.try_lock() {
        Ok(()) => Ok(LifecycleLock { _file: file }),
        Err(TryLockError::WouldBlock) => Err(LockError::Busy(path)),
        Err(TryLockError::Error(e)) => Err(LockError::Io(path, e)),
    }
}

/// Shared hold for an ordinary write command. `Ok(None)` when the store directory does not exist
/// yet (nothing to protect, and a read of `--help`-like commands never creates it).
pub fn shared(dir: &Path) -> Result<Option<LifecycleLock>, LockError> {
    if !dir.is_dir() {
        return Ok(None);
    }
    let (file, path) = open(dir)?;
    match file.try_lock_shared() {
        Ok(()) => Ok(Some(LifecycleLock { _file: file })),
        Err(TryLockError::WouldBlock) => Err(LockError::Busy(path)),
        Err(TryLockError::Error(e)) => Err(LockError::Io(path, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("cm-cli-lock-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn exclusive_excludes_everything_and_shared_coexist() {
        let dir = scratch("modes");
        let a = shared(&dir).unwrap().unwrap();
        let b = shared(&dir).unwrap().unwrap();
        assert!(matches!(exclusive(&dir), Err(LockError::Busy(_))), "an update must not start during writes");
        drop((a, b));
        let x = exclusive(&dir).unwrap();
        assert!(matches!(exclusive(&dir), Err(LockError::Busy(_))), "one update at a time");
        assert!(matches!(shared(&dir), Err(LockError::Busy(_))), "no write starts during an update");
        drop(x);
        assert!(shared(&dir).unwrap().is_some());
    }

    #[test]
    fn missing_store_is_not_created_by_a_shared_hold() {
        let dir = scratch("missing").join("not-yet");
        assert!(shared(&dir).unwrap().is_none());
        assert!(!dir.exists());
    }
}
