//! Persistence for CleanManager.
//!
//! Two databases with different durability duties (D4, IDX-05):
//! - `index.db`: scan facts. Rebuildable; may be discarded when damaged or incompatible.
//! - `audit.db`: operations, intents, authorizations and quarantine manifests. Never removed by
//!   an index rebuild and never silently re-initialized.
//!
//! The audit store is opened with the store; an existing index is opened on first use, so history,
//! quarantine and recovery keep working while the index is damaged (BUG-WFA-01). An unusable index
//! is the typed `index_unavailable` error with `doctor --repair-index` as its required action;
//! [`Stores::repair_index`] moves the damaged files aside (never deletes them) and starts an empty index.
//!
//! The store directory must live outside every scan root so scanning never observes its own writes.

pub mod audit;
pub mod index;
pub mod integrity;
pub mod migrate;

use std::path::{Path, PathBuf};

pub use audit::AuditStore;
pub use index::IndexStore;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("index schema version {found} is not supported (expected {expected}); history and recovery are not affected: repair the index with `cleanmanager doctor --repair-index` (the old file is kept) and scan again")]
    IndexIncompatible { found: String, expected: String },
    #[error("index database is damaged: {0}; history and recovery are not affected: repair the index with `cleanmanager doctor --repair-index` (the damaged file is kept) and scan again")]
    IndexCorrupt(String),
    #[error("audit database problem: {0}; operation history is preserved, repair is required before writes")]
    AuditUnavailable(String),
    #[error("store directory {store} is inside scan root {root}")]
    StoreInsideScope { store: String, root: String },
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("cursor_stale: {0}")]
    CursorStale(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid state: {0}")]
    InvalidState(String),
    #[error("core: {0}")]
    Core(#[from] cm_core::error::CoreError),
}

/// The next step for an unusable index (the `required_action` of `index_unavailable`).
pub const REPAIR_INDEX_COMMAND: &str = "cleanmanager doctor --repair-index";

impl StoreError {
    pub fn code(&self) -> cm_core::error::ErrorCode {
        use cm_core::error::ErrorCode;
        match self {
            StoreError::InvalidArgument(_) | StoreError::StoreInsideScope { .. } => ErrorCode::InvalidArgument,
            StoreError::CursorStale(_) => ErrorCode::CursorStale,
            StoreError::NotFound(_) => ErrorCode::InvalidArgument,
            StoreError::IndexCorrupt(_) | StoreError::IndexIncompatible { .. } => ErrorCode::IndexUnavailable,
            StoreError::Core(e) => e.code(),
            _ => ErrorCode::InternalError,
        }
    }

    /// The rebuildable index cannot be used (damaged or an unsupported schema); history and recovery are unaffected.
    pub fn is_index_unusable(&self) -> bool {
        matches!(self, StoreError::IndexCorrupt(_) | StoreError::IndexIncompatible { .. })
    }

    /// What the caller should do next, for errors that have one fixed next step.
    pub fn required_action(&self) -> Option<&'static str> {
        self.is_index_unusable().then_some(REPAIR_INDEX_COMMAND)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Both stores opened from one per-user directory.
pub struct Stores {
    pub dir: PathBuf,
    pub index: IndexStore,
    pub audit: AuditStore,
}

pub const INDEX_FILE: &str = "index.db";
pub const AUDIT_FILE: &str = "audit.db";

/// What [`Stores::repair_index`] found and did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRepair {
    /// The index before the repair: `healthy`, `missing`, `corrupt` or `incompatible`.
    pub previous_state: &'static str,
    /// A damaged index was moved aside and an empty one created (scans must be run again).
    pub repaired: bool,
    /// The moved files (`index.db.damaged-<time>` and its `-wal`/`-shm`), kept next to the store.
    pub kept: Vec<PathBuf>,
    /// Why the index was unusable.
    pub problem: Option<String>,
}

impl Stores {
    /// Open (creating if needed) both stores. The audit store opens now and its problems fail the
    /// call; an existing index opens on first use (a new store gets its empty index at once), so an
    /// index problem is reported by the calls that need the index, never auto-fixed.
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let audit = AuditStore::open(&dir.join(AUDIT_FILE))?;
        let path = dir.join(INDEX_FILE);
        let new = !path.exists();
        let index = IndexStore::deferred(&path);
        if new {
            // Creating an empty index is cheap; a failure here is reported again on first use.
            let _ = index.verify();
        }
        Ok(Stores { dir: dir.to_path_buf(), index, audit })
    }

    /// Move the index and its WAL/SHM files aside as `index.db.damaged-<time>` (`-wal`, `-shm`) and
    /// create an empty index. Nothing is deleted; `audit.db` is never touched; scans must be run again.
    /// A file another process holds open cannot be moved on Windows: then everything moved so far is
    /// put back and the I/O error is returned.
    pub fn rebuild_index(dir: &Path) -> Result<(IndexStore, Vec<PathBuf>)> {
        let stamp = cm_core::time::now_utc().replace([':', '-'], "");
        let taken = |base: &str| ["", "-wal", "-shm"].iter().any(|x| dir.join(format!("{base}{x}")).exists());
        let mut base = format!("{INDEX_FILE}.damaged-{stamp}");
        let mut n = 1;
        while taken(&base) {
            base = format!("{INDEX_FILE}.damaged-{stamp}-{n}");
            n += 1;
        }
        let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
        for suffix in ["", "-wal", "-shm"] {
            let from = dir.join(format!("{INDEX_FILE}{suffix}"));
            if std::fs::symlink_metadata(&from).is_err() {
                continue;
            }
            let to = dir.join(format!("{base}{suffix}"));
            if let Err(e) = std::fs::rename(&from, &to) {
                // The index and its WAL must stay together: a WAL left behind would be replayed into the new index.
                for (a, b) in moved.iter().rev() {
                    let _ = std::fs::rename(b, a);
                }
                return Err(std::io::Error::new(e.kind(), format!("cannot move {} aside: {e}", from.display())).into());
            }
            moved.push((from, to));
        }
        let kept: Vec<PathBuf> = moved.into_iter().map(|(_, to)| to).collect();
        let detail = format!("moved aside as {}; an empty index was created and scans must be run again", kept.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "));
        crate::migrate::log(dir, crate::migrate::StoreKind::Index, "-", crate::migrate::StoreKind::Index.current(), "rebuilt", Some(&detail));
        let index = IndexStore::open(&dir.join(INDEX_FILE))?;
        Ok((index, kept))
    }

    /// `doctor --repair-index`: check the index fully; only an unusable one (damaged, or a schema
    /// this program cannot use) is moved aside and replaced by an empty index. A healthy index is
    /// left alone; a busy or unreadable file is reported and nothing changes.
    pub fn repair_index(dir: &Path) -> Result<IndexRepair> {
        let path = dir.join(INDEX_FILE);
        if !path.exists() {
            return Ok(IndexRepair { previous_state: "missing", repaired: false, kept: vec![], problem: None });
        }
        // Read-only first: a damaged file and its WAL stay exactly as they are until they are moved. A file that
        // passes is then opened normally, which migrates an older schema or reports one this program cannot use.
        let found = index::probe_read_only(&path).and_then(|()| IndexStore::open(&path)?.verify());
        match found {
            Ok(()) => Ok(IndexRepair { previous_state: "healthy", repaired: false, kept: vec![], problem: None }),
            Err(e) if e.is_index_unusable() => {
                let previous_state = if matches!(e, StoreError::IndexIncompatible { .. }) { "incompatible" } else { "corrupt" };
                let (_, kept) = Self::rebuild_index(dir)?;
                Ok(IndexRepair { previous_state, repaired: true, kept, problem: Some(e.to_string()) })
            }
            Err(e) => Err(e),
        }
    }

    /// Refuse a scan whose root contains this store (the scan would observe its own writes).
    pub fn ensure_outside(&self, scan_root: &Path) -> Result<()> {
        let store = std::fs::canonicalize(&self.dir)?;
        let root = std::fs::canonicalize(scan_root)?;
        if store.starts_with(&root) {
            return Err(StoreError::StoreInsideScope { store: store.display().to_string(), root: root.display().to_string() });
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::PathBuf;

    pub fn scratch(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("cm-store-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
