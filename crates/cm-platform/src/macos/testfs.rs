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
