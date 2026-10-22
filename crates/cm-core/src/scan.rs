//! Backend-neutral scan interfaces. Platform backends produce rows; a sink (the index store)
//! persists them. Backends never decide cleanup, never write targets and never follow links.

use serde::Serialize;

use crate::capability::CapabilityState;
use crate::model::{BoundaryKind, CloudState, IdentityQuality, ObjectKind, ScanState, SkipReason};
use crate::path::RawPath;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRow {
    pub object_id: u64,
    pub volume_id: String,
    /// Protocol wire form, e.g. `fileid128:<hex>`.
    pub file_id: String,
    pub generation: Option<String>,
    pub quality: IdentityQuality,
    pub kind: ObjectKind,
    pub logical: Option<u64>,
    pub allocated: Option<u64>,
    pub link_count: Option<u32>,
    pub cloud: CloudState,
    /// Last modification time in Unix seconds (from metadata only); `None` when unavailable.
    /// It is not a last-use time and never satisfies an age rule when missing (CAT-07).
    pub modified_unix: Option<i64>,
    /// Some of the object's blocks are shared with another object (APFS clone): `Some(true)` when the platform saw
    /// sharing, `Some(false)` when it established there is none, `None` when it did not or could not tell (Windows).
    /// A shared or unknown object's reclaimable space is not its allocated size (`shared_blocks_unknown`).
    pub shared_blocks: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryRow {
    pub entry_id: u64,
    /// `None` only for the scope root entry, whose `name` is the full root path.
    pub parent_entry_id: Option<u64>,
    pub name: RawPath,
    pub object_id: u64,
    pub kind: ObjectKind,
    /// Set when the entry is a link/mount/cloud boundary that was recorded but not traversed.
    pub boundary: Option<BoundaryKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedRow {
    pub parent_entry_id: Option<u64>,
    pub path: RawPath,
    pub reason: SkipReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScanIssue {
    pub code: String,
    pub scope_display: String,
    pub message: String,
}

/// Typed `ScanIssue::code` values that every backend and host reports with the same meaning.
/// A root refusal is reported before any row is written: the scan ends `failed` with no entries.
pub mod issue {
    /// The root exists but is not a directory (a regular file, a device, ...).
    pub const NOT_A_DIRECTORY: &str = "not_a_directory";
    /// The root itself is a link: a symbolic link, junction, mount point or another reparse point
    /// the scanner does not traverse. It is never followed; the user scans the target directly.
    pub const ROOT_IS_LINK: &str = "root_is_link";
    /// The root does not exist, its volume is offline, or it cannot be opened for another reason.
    pub const ROOT_UNAVAILABLE: &str = "root_unavailable";
    /// Reading the root or a subtree needs rights the scan does not have; never elevated silently.
    pub const PERMISSION_REQUIRED: &str = "permission_required";
}

/// Where scan rows go. Implemented by the index store.
pub trait ScanSink {
    fn write(&mut self, objects: &[ObjectRow], entries: &[EntryRow], skipped: &[SkippedRow]) -> Result<(), String>;
    /// Polled between directories and after every batch written while one directory is listed, so a
    /// single huge directory cannot outrun a cancel request; a `true` stops scheduling new work.
    fn cancel_requested(&mut self) -> bool;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RootProbe {
    pub filesystem: Option<String>,
    pub volume_id: Option<String>,
    pub backend: String,
    pub state: CapabilityState,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Rows per sink batch.
    pub batch_entries: usize,
    /// CleanManager's own data directories. A backend records a directory with the same
    /// identity as an `own_store` boundary and does not traverse it (it would otherwise observe
    /// its own index writes); the default store location is always added by the platform.
    pub own_store_dirs: Vec<RawPath>,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions { batch_entries: 4_096, own_store_dirs: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScanReport {
    /// Terminal state reached by the backend; the store still downgrades `completed` when
    /// ranges were skipped.
    pub state: ScanState,
    pub backend: String,
    pub backend_reason: String,
    pub entries: u64,
    pub skipped_ranges: u64,
    pub boundaries: u64,
    /// ACL-bypass privileges that were enabled while scanning (empty for an ordinary-permission
    /// scan). Results read with bypass rights are labelled, never presented as ordinary access.
    pub elevated_read: Vec<String>,
    pub issues: Vec<ScanIssue>,
}

pub trait ScanBackend {
    fn name(&self) -> &'static str;
    /// Inspect only the requested root and its volume; never the whole volume namespace.
    fn probe(&self, root: &RawPath) -> RootProbe;
    fn scan(&self, root: &RawPath, sink: &mut dyn ScanSink, options: &ScanOptions) -> ScanReport;
}
