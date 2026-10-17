//! Test fixtures for the macOS modules: one fresh directory per fixture under the *canonical*
//! temporary directory. `$TMPDIR` is `/var/folders/...`, but `/var` is a symlink to
//! `/private/var`, and the hard-deny layer judges the link and its target; fixtures therefore
//! always live under the resolved `/private/var/folders/...` spelling. Removed on drop, even
//! when a test left a directory without permissions.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A unique scratch directory, deleted with everything in it on drop.
#[derive(Debug)]
pub(crate) struct Fixture {
    root: PathBuf,
}

impl Fixture {
    /// `<canonical TMPDIR>/cm-<tag>-<pid>-<n>-<nanos>`; panics if it cannot be created fresh.
    pub(crate) fn new(tag: &str) -> Fixture {
        let base = std::fs::canonicalize(std::env::temp_dir()).expect("temporary directory resolves");
        assert!(!base.starts_with("/var/"), "canonical temp dir expected, got {}", base.display());
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let root = base.join(format!("cm-{tag}-{}-{n}-{nanos:08x}", std::process::id()));
        std::fs::create_dir(&root).unwrap_or_else(|e| panic!("fixture {}: {e}", root.display()));
        Fixture { root }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.root
    }

    /// Write `bytes` to `rel` (parents created); returns the absolute path.
    pub(crate) fn write(&self, rel: impl AsRef<Path>, bytes: &[u8]) -> PathBuf {
        let p = self.root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, bytes).unwrap();
        p
    }

    /// Create directory `rel` (parents created); returns the absolute path.
    pub(crate) fn mkdir(&self, rel: impl AsRef<Path>) -> PathBuf {
        let p = self.root.join(rel);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}

/// Give every directory below `dir` (symlinks not followed) owner rwx so it can be removed.
fn unlock(dir: &Path) {
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            unlock(&e.path());
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::fs::remove_dir_all(&self.root).is_err() {
            unlock(&self.root);
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

/// Copy the system binary `program` to `exe` so a fixture process runs from there. Some macOS releases kill a copy of an
/// Apple platform binary at launch (SIGKILL, seen on macOS 27.0.1 for `/bin/bash`, `/bin/sleep` and `/bin/sh`); only
/// then is the copy signed ad hoc, which makes it an ordinary binary. Where the plain copy runs it is left as it is.
pub(crate) fn copy_system_binary(program: &str, exe: &Path) {
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Stdio};
    // The probe runs a throwaway copy: a file the system once killed at launch stays unusable here even after signing.
    let probe_path = exe.with_extension("probe");
    std::fs::copy(program, &probe_path).unwrap();
    let probe = Command::new(&probe_path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap();
    std::fs::remove_file(&probe_path).unwrap();
    std::fs::copy(program, exe).unwrap();
    if probe.signal() == Some(libc::SIGKILL) {
        let signed = Command::new("/usr/bin/codesign").args(["-s", "-", "-f"]).arg(exe).output().unwrap();
        assert!(signed.status.success(), "ad-hoc signing {}: {}", exe.display(), String::from_utf8_lossy(&signed.stderr));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixtures_are_unique_canonical_and_cleaned_up() {
        let (a, b) = (Fixture::new("t"), Fixture::new("t"));
        assert_ne!(a.path(), b.path());
        assert!(a.path().starts_with("/private/") || !Path::new("/private").exists(), "{}", a.path().display());
        let locked = a.mkdir("x/locked");
        a.write("x/locked/f", b"1");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let root = a.path().to_path_buf();
        drop(a);
        assert!(!root.exists());
    }
}
