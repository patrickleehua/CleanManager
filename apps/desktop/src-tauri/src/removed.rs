//! "Removed since scan" in the desktop: the overlay of `cm_app::removed` (the same one `entries list` reports), cached
//! per scan and audit revision so listing a page costs one small audit query when nothing changed. Read-only: the
//! overlay comes from the audit store and the scan's own index rows; nothing on disk is read again.
//!
//! - `list_entries`, `treemap_layer` and search results carry each entry's `removed` fact and, for folders, what was
//!   removed beneath them; `scan_status` carries the scan's total (`removed_since_scan`).
//! - Candidate views built from a stored scan (rule cleanup here, build artifacts and installers through
//!   [`removed_entries`]) leave such candidates out with a count, and plan creation refuses them (`already_removed`),
//!   so a second plan never includes what an earlier one already removed. The executor still re-checks every object
//!   before acting.

use std::collections::HashMap;
use std::sync::Arc;

use cm_app::removed::RemovedOverlay;
use cm_store::Stores;
use serde::Serialize;

use crate::commands::{St, blocking};
use crate::dto::{IpcResult, RemovedDto, RemovedTotalDto, ScanStatusDto};
use crate::host::{AppState, lock};

/// Overlays kept (one per scan recently shown); more are computed again when needed.
const CACHE_CAP: usize = 16;
/// Entries one `removed_entries` call may ask about (a discovery's rows).
const MAX_ENTRIES: usize = 5_000;

#[derive(Default)]
pub struct RemovedCache {
    by_scan: HashMap<String, Arc<RemovedOverlay>>,
}

/// The overlay of `scan_id` at the audit store's current revision (cached).
pub fn overlay(s: &AppState, stores: &Stores, scan_id: &str) -> IpcResult<Arc<RemovedOverlay>> {
    let token = stores.audit.revision_token()?;
    if let Some(o) = lock(&s.removed).by_scan.get(scan_id).filter(|o| o.audit_revision == token) {
        return Ok(o.clone());
    }
    let o = Arc::new(cm_app::removed::removed_overlay(stores, scan_id)?);
    let mut cache = lock(&s.removed);
    if cache.by_scan.len() >= CACHE_CAP {
        cache.by_scan.clear();
    }
    cache.by_scan.insert(scan_id.to_owned(), o.clone());
    Ok(o)
}

/// `removed_since_scan` of a status: for a finished, published scan with entries.
pub fn since_scan(s: &AppState, stores: &Stores, v: &ScanStatusDto) -> IpcResult<Option<RemovedTotalDto>> {
    let finished = !matches!(v.state, cm_core::model::ScanState::Created | cm_core::model::ScanState::Running | cm_core::model::ScanState::Stopping);
    if !finished || !v.published || v.root_entry_id.is_none() || v.state == cm_core::model::ScanState::Failed {
        return Ok(None);
    }
    let o = overlay(s, stores, &v.scan_id)?;
    Ok(Some(RemovedTotalDto::of(&o, o.total)))
}

/// One entry the page asked about that a later operation touched: removed (itself, or with a folder above it), or a
/// folder with removed objects inside.
#[derive(Debug, Clone, Serialize)]
pub struct RemovedEntryDto {
    pub entry_id: String,
    /// `None` for a folder that is still there with removed objects inside.
    pub removed: Option<RemovedDto>,
    /// Removed objects inside (0 when the entry itself was removed).
    pub removed_count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanRemovedDto {
    pub scan_id: String,
    pub total: RemovedTotalDto,
    /// Only the asked entries that were touched.
    pub entries: Vec<RemovedEntryDto>,
}

/// Which of the given entries of a scan later operations touched (the build-artifact and installer pages hold their
/// discovery in the page and leave such rows out). Entries not in the scan are ignored.
pub fn removed_of(s: &AppState, stores: &Stores, scan_id: &str, entry_ids: &[u64]) -> IpcResult<ScanRemovedDto> {
    use cm_app::removed::Touched;
    let o = overlay(s, stores, scan_id)?;
    let mut entries = Vec::new();
    if !o.is_empty() {
        for &id in entry_ids.iter().take(MAX_ENTRIES) {
            let item = match stores.index.get_entry(scan_id, id) {
                Ok(i) => i,
                Err(cm_store::StoreError::NotFound(_)) => continue,
                Err(e) => return Err(e.into()),
            };
            let (removed, removed_count) = match o.touched(id, &item.path) {
                None => continue,
                Some(Touched::Removed(r)) => (
                    Some(RemovedDto { mode: r.mark.mode.as_str(), operation_id: r.mark.operation_id.clone(), at: r.mark.at.clone(), recoverable: r.mark.recoverable, inherited: r.inherited }),
                    0,
                ),
                Some(Touched::Inside(t)) => (None, t.count),
            };
            entries.push(RemovedEntryDto { entry_id: id.to_string(), removed, removed_count });
        }
    }
    Ok(ScanRemovedDto { scan_id: scan_id.to_owned(), total: RemovedTotalDto::of(&o, o.total), entries })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn removed_entries(state: St<'_>, scan_id: String, entry_ids: Vec<String>) -> IpcResult<ScanRemovedDto> {
    let s = state.inner().clone();
    blocking(move || {
        let id = crate::args::scan_id(&scan_id)?;
        let ids = entry_ids.iter().take(MAX_ENTRIES).map(|e| crate::args::entry_id(e)).collect::<Result<Vec<u64>, _>>()?;
        s.with_stores(|stores| removed_of(&s, stores, &id, &ids))
    })
    .await
}
