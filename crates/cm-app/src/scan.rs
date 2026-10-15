//! Scan lifecycle (SCN-03/04/07, CLI-09): running → (stopping) → completed | partial |
//! cancelled | failed, hosted by the calling process. Cancellation is requested through the
//! store (from any process) and observed between directories; the scan shows `stopping` until
//! the backend returns, and confirmed rows are always published.

use cm_core::host::HostRef;
use cm_core::model::ScanState;
use cm_core::path::RawPath;
use cm_core::scan::{EntryRow, ObjectRow, ScanBackend, ScanOptions, ScanReport, ScanSink, SkippedRow};
use cm_core::time::now_utc;
use cm_store::IndexStore;
use cm_store::index::{ScanInit, StagingSink};

/// Wraps the staging sink: heartbeats on every batch and marks the scan `stopping` as soon as
/// a cancel request is observed, so other processes never see a cancelled scan as running.
struct LifecycleSink<'a> {
    inner: StagingSink<'a>,
    stopping: bool,
    entries: u64,
    progress: Option<&'a dyn Fn(u64)>,
}

impl ScanSink for LifecycleSink<'_> {
    fn write(&mut self, objects: &[ObjectRow], entries: &[EntryRow], skipped: &[SkippedRow]) -> Result<(), String> {
        self.inner.write(objects, entries, skipped)?;
        self.entries += entries.len() as u64;
        if let Some(report) = self.progress {
            report(self.entries);
        }
        self.inner.store.heartbeat(&self.inner.scan_id, &now_utc()).map_err(|e| e.to_string())
    }

    fn cancel_requested(&mut self) -> bool {
        if self.stopping {
            return true;
        }
        if self.inner.cancel_requested() {
            self.stopping = true;
            let _ = self.inner.store.set_stopping(&self.inner.scan_id);
        }
        self.stopping
    }
}

#[derive(Debug, Clone)]
pub struct ScanRequest {
    pub scan_id: String,
    pub scope_id: String,
    pub root: RawPath,
    pub host: HostRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanOutcome {
    pub scan_id: String,
    /// Terminal state as published (the store downgrades `completed` with unread ranges).
    pub state: ScanState,
    pub report: ScanReport,
}

/// Run one scan to a published terminal state. Errors from the store are returned; backend
/// problems are part of the report and still publish whatever was confirmed.
pub fn run_scan(index: &IndexStore, backend: &dyn ScanBackend, request: &ScanRequest, options: &ScanOptions) -> Result<ScanOutcome, cm_store::StoreError> {
    run_scan_with_progress(index, backend, request, options, None)
}

/// Like `run_scan`, reporting the number of committed entries after every batch.
pub fn run_scan_with_progress(
    index: &IndexStore,
    backend: &dyn ScanBackend,
    request: &ScanRequest,
    options: &ScanOptions,
    progress: Option<&dyn Fn(u64)>,
) -> Result<ScanOutcome, cm_store::StoreError> {
    let revision = index.begin_scan(&ScanInit {
        scan_id: request.scan_id.clone(),
        scope_id: request.scope_id.clone(),
        root: request.root.clone(),
        backend: backend.name().into(),
        host: request.host.clone(),
        started_at: now_utc(),
    })?;
    let mut sink = LifecycleSink { inner: StagingSink { store: index, scan_id: request.scan_id.clone(), revision }, stopping: false, entries: 0, progress };
    let mut report = backend.scan(&request.root, &mut sink, options);
    // A cancel observed during the last batch still ends as cancelled, never completed.
    if sink.stopping && report.state == ScanState::Completed {
        report.state = ScanState::Cancelled;
    }
    index.record_report(&request.scan_id, &report)?;
    let state = index.publish(&request.scan_id, revision, report.state, &now_utc())?;
    Ok(ScanOutcome { scan_id: request.scan_id.clone(), state, report })
}

#[cfg(test)]
mod tests;
