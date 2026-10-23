//! Repairing the scan index (BUG-WFA-01), shared by `cleanmanager doctor --repair-index` and the desktop
//! host's `repair_index` command: a full check of `index.db`; only an unusable one (damaged, or a schema this
//! program cannot use) is moved aside — kept as `index.db.damaged-<time>` with its WAL/SHM, never deleted —
//! and replaced by an empty index. A healthy index is left alone. The audit store, and with it history,
//! quarantine and recovery, is never touched. Both entries return the same `index_repair` object.

use std::path::Path;

use serde::Serialize;

/// The `index_repair` object of `doctor_report` (and the desktop command's result).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IndexRepairReport {
    /// `repaired` or `not_needed`.
    pub status: &'static str,
    /// `healthy`, `missing`, `corrupt` or `incompatible`.
    pub previous_state: &'static str,
    /// Where the moved files are kept (empty unless repaired).
    pub kept_files: Vec<String>,
    pub message: String,
}

impl IndexRepairReport {
    pub fn to_wire(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("report serializes")
    }
}

/// Why nothing was changed.
#[derive(Debug)]
pub enum RepairError {
    /// The index or its WAL could not be moved aside: on Windows another process (a running scan, the CLI or
    /// the desktop app) holds it open. Everything moved so far was put back.
    Busy(std::io::Error),
    /// Any other store problem, reported with its own code.
    Store(cm_store::StoreError),
}

/// Check the index in `dir` and repair it only when it is unusable. The caller makes sure it holds no
/// connection to the index itself (on Windows an open file cannot be moved).
pub fn repair(dir: &Path) -> Result<IndexRepairReport, RepairError> {
    let unchanged = |previous_state: &'static str, message: &str| IndexRepairReport { status: "not_needed", previous_state, kept_files: vec![], message: message.into() };
    if !dir.is_dir() {
        return Ok(unchanged("missing", "There is no store yet; nothing was changed."));
    }
    match cm_store::Stores::repair_index(dir) {
        Ok(r) if r.repaired => {
            let kept: Vec<String> = r.kept.iter().map(|k| k.display().to_string()).collect();
            let message = format!(
                "The scan index was unusable ({}). It was moved aside as {} and an empty index was created. Operation history, quarantine and recovery were not touched. Earlier scans and candidate sets are no longer available: run scan again.",
                r.problem.unwrap_or_default(),
                kept.join(", ")
            );
            Ok(IndexRepairReport { status: "repaired", previous_state: r.previous_state, kept_files: kept, message })
        }
        Ok(r) if r.previous_state == "missing" => Ok(unchanged("missing", "There is no scan index yet; nothing was changed (the next scan creates it).")),
        Ok(r) => Ok(unchanged(r.previous_state, "The scan index passed the full integrity check; nothing was changed.")),
        Err(cm_store::StoreError::Io(e)) => Err(RepairError::Busy(e)),
        Err(e) => Err(RepairError::Store(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("cm-app-index-repair-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn only_an_unusable_index_is_moved_aside_and_the_audit_store_is_kept() {
        let dir = scratch("cases");
        let missing = repair(&dir).unwrap();
        assert_eq!((missing.status, missing.previous_state), ("not_needed", "missing"));
        assert!(!dir.exists(), "a missing store is not created");

        let stores = cm_store::Stores::open(&dir).unwrap();
        let host = cm_core::host::HostRef { kind: "cli".into(), pid: 1, started_at: "2026-10-23T00:00:00Z".into() };
        stores.audit.begin_operation("op1", "p1", "d", "k", &host, "2026-10-23T00:00:00Z").unwrap();
        drop(stores);
        let healthy = repair(&dir).unwrap();
        assert_eq!((healthy.status, healthy.previous_state, healthy.kept_files.len()), ("not_needed", "healthy", 0));
        assert_eq!(healthy.to_wire()["message"], "The scan index passed the full integrity check; nothing was changed.");

        std::fs::write(dir.join(cm_store::INDEX_FILE), [0x42u8; 8192]).unwrap();
        let repaired = repair(&dir).unwrap();
        assert_eq!((repaired.status, repaired.previous_state), ("repaired", "corrupt"), "{repaired:?}");
        assert_eq!(repaired.kept_files.len(), 1, "{repaired:?}");
        assert!(repaired.kept_files[0].contains("index.db.damaged-") && repaired.message.contains("run scan again"), "{repaired:?}");
        let wire = repaired.to_wire();
        let mut keys: Vec<&String> = wire.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["kept_files", "message", "previous_state", "status"], "the doctor_report index_repair object");
        let stores = cm_store::Stores::open(&dir).unwrap();
        assert_eq!(stores.audit.operation_state("op1").unwrap(), "created", "history is never touched");
        assert!(stores.index.list_scans(1, None).unwrap().0.is_empty(), "an empty index replaced the damaged one");
        drop(stores);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
