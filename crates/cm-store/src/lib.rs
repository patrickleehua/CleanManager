//! Persistence for CleanManager.
//!
//! Two databases with different durability duties (D4, IDX-05):
//! - `index.db`: scan facts. Rebuildable; may be discarded when damaged or incompatible.
//! - `audit.db`: operations, intents, authorizations and quarantine manifests. Never removed by
//!   an index rebuild and never silently re-initialized.
//!
//! The store directory must live outside every scan root so scanning never observes its own writes.

pub mod audit;
pub mod index;
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
    #[error("index schema version {found} is not supported (expected {expected}); rebuild the index")]
    IndexIncompatible { found: String, expected: String },
    #[error("index database is damaged: {0}; rebuild the index")]
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

impl StoreError {
    pub fn code(&self) -> cm_core::error::ErrorCode {
        use cm_core::error::ErrorCode;
        match self {
            StoreError::InvalidArgument(_) | StoreError::StoreInsideScope { .. } => ErrorCode::InvalidArgument,
            StoreError::CursorStale(_) => ErrorCode::CursorStale,
            StoreError::NotFound(_) => ErrorCode::InvalidArgument,
            StoreError::Core(e) => e.code(),
            _ => ErrorCode::InternalError,
        }
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

impl Stores {
    /// Open (creating if needed) both stores. An index problem is reported, not auto-fixed;
    /// the caller may then call `rebuild_index`, which leaves `audit.db` untouched.
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let audit = AuditStore::open(&dir.join(AUDIT_FILE))?;
        let index = IndexStore::open(&dir.join(INDEX_FILE))?;
        Ok(Stores { dir: dir.to_path_buf(), index, audit })
    }

    /// Discard only the rebuildable index (and its WAL/SHM files) and create an empty one.
    pub fn rebuild_index(dir: &Path) -> Result<IndexStore> {
        for name in [INDEX_FILE, "index.db-wal", "index.db-shm"] {
            match std::fs::remove_file(dir.join(name)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        IndexStore::open(&dir.join(INDEX_FILE))
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
