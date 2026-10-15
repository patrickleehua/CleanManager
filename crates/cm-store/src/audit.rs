//! Audit store: operations and per-action journal (extended by the executor). Unlike the index,
//! this database is never discarded; incompatibility or damage blocks writes instead.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::{Result, StoreError};

pub const AUDIT_SCHEMA_VERSION: &str = "1";

pub use cm_core::host::{HostRef, Liveness};

pub struct AuditStore {
    pub(crate) conn: Connection,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS operations (
    operation_id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL,
    plan_digest TEXT NOT NULL,
    dedupe_key TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL CHECK (state IN ('created','validating','running','stopping','completed','partial','cancelled','failed','reconciliation_required')),
    host_kind TEXT NOT NULL,
    host_pid INTEGER NOT NULL,
    host_started_at TEXT NOT NULL,
    heartbeat_at TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT
) STRICT;
CREATE TABLE IF NOT EXISTS action_journal (
    operation_id TEXT NOT NULL REFERENCES operations(operation_id),
    action_id TEXT NOT NULL,
    intent_json TEXT NOT NULL,
    intent_at TEXT NOT NULL,
    result_status TEXT CHECK (result_status IN ('succeeded','skipped','failed','not_started','unknown')),
    result_json TEXT,
    result_at TEXT,
    PRIMARY KEY (operation_id, action_id)
) STRICT;
";

impl AuditStore {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if check != "ok" {
            return Err(StoreError::AuditUnavailable(format!("integrity check: {check}")));
        }
        conn.execute_batch(SCHEMA)?;
        let found: Option<String> =
            conn.query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| r.get(0)).optional()?;
        match found {
            None => {
                conn.execute("INSERT INTO meta(key,value) VALUES('store_kind','audit'),('schema_version',?1)", [AUDIT_SCHEMA_VERSION])?;
            }
            Some(v) if v == AUDIT_SCHEMA_VERSION => {}
            Some(v) => return Err(StoreError::AuditUnavailable(format!("schema version {v} is not supported"))),
        }
        Ok(AuditStore { conn })
    }

    /// Register a new operation; the same dedupe key returns the existing operation (EXE-03).
    pub fn begin_operation(&self, operation_id: &str, plan_id: &str, plan_digest: &str, dedupe_key: &str, host: &HostRef, now: &str) -> Result<String> {
        let existing: Option<(String, String)> = self
            .conn
            .query_row("SELECT operation_id, plan_digest FROM operations WHERE dedupe_key=?1", [dedupe_key], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        if let Some((id, digest)) = existing {
            if digest != plan_digest {
                return Err(StoreError::InvalidState("dedupe key reused for a different plan digest (conflict)".into()));
            }
            return Ok(id);
        }
        self.conn.execute(
            "INSERT INTO operations(operation_id,plan_id,plan_digest,dedupe_key,state,host_kind,host_pid,host_started_at,heartbeat_at,started_at)
             VALUES(?1,?2,?3,?4,'created',?5,?6,?7,?8,?8)",
            params![operation_id, plan_id, plan_digest, dedupe_key, host.kind, host.pid, host.started_at, now],
        )?;
        Ok(operation_id.to_owned())
    }

    pub fn set_operation_state(&self, operation_id: &str, state: &str) -> Result<()> {
        let n = self.conn.execute("UPDATE operations SET state=?2 WHERE operation_id=?1", params![operation_id, state])?;
        if n == 0 {
            return Err(StoreError::NotFound(operation_id.into()));
        }
        Ok(())
    }

    pub fn operation_state(&self, operation_id: &str) -> Result<String> {
        self.conn
            .query_row("SELECT state FROM operations WHERE operation_id=?1", [operation_id], |r| r.get(0))
            .optional()?
            .ok_or_else(|| StoreError::NotFound(operation_id.into()))
    }

    /// Operations whose host has exited before reaching a terminal state cannot be assumed to
    /// have done or not done anything: they move to `reconciliation_required` (EXE-05, CLI-09).
    pub fn reconcile_hosts(&self, liveness: impl Fn(&HostRef) -> Liveness) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT operation_id, host_kind, host_pid, host_started_at FROM operations
             WHERE state IN ('created','validating','running','stopping')",
        )?;
        let rows: Vec<(String, HostRef)> = stmt
            .query_map([], |r| Ok((r.get(0)?, HostRef { kind: r.get(1)?, pid: r.get(2)?, started_at: r.get(3)? })))?
            .collect::<std::result::Result<_, _>>()?;
        let mut moved = Vec::new();
        for (id, host) in rows {
            if liveness(&host) == Liveness::Exited {
                self.set_operation_state(&id, "reconciliation_required")?;
                moved.push(id);
            }
        }
        Ok(moved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::scratch;

    fn host(pid: u32) -> HostRef {
        HostRef { kind: "cli".into(), pid, started_at: "2026-10-15T00:00:00Z".into() }
    }

    #[test]
    fn same_dedupe_key_returns_existing_operation_and_conflicts_on_other_digest() {
        let dir = scratch("audit-dedupe");
        let a = AuditStore::open(&dir.join("audit.db")).unwrap();
        let first = a.begin_operation("op1", "p1", "d1", "k1", &host(1), "t").unwrap();
        let again = a.begin_operation("op2", "p1", "d1", "k1", &host(2), "t").unwrap();
        assert_eq!(first, again);
        assert!(a.begin_operation("op3", "p2", "other", "k1", &host(3), "t").is_err());
    }

    #[test]
    fn dead_host_moves_running_operation_to_reconciliation_but_unknown_does_not() {
        let dir = scratch("audit-reconcile");
        let a = AuditStore::open(&dir.join("audit.db")).unwrap();
        a.begin_operation("dead", "p", "d", "k1", &host(10), "t").unwrap();
        a.begin_operation("unsure", "p", "d", "k2", &host(11), "t").unwrap();
        a.set_operation_state("dead", "running").unwrap();
        a.set_operation_state("unsure", "running").unwrap();
        let moved = a.reconcile_hosts(|h| if h.pid == 10 { Liveness::Exited } else { Liveness::Unknown }).unwrap();
        assert_eq!(moved, vec!["dead".to_string()]);
        assert_eq!(a.operation_state("dead").unwrap(), "reconciliation_required");
        assert_eq!(a.operation_state("unsure").unwrap(), "running");
    }

    #[test]
    fn incompatible_audit_schema_blocks_instead_of_reinitializing() {
        let dir = scratch("audit-version");
        let path = dir.join("audit.db");
        {
            let a = AuditStore::open(&path).unwrap();
            a.begin_operation("keep", "p", "d", "k", &host(1), "t").unwrap();
            a.conn.execute("UPDATE meta SET value='999' WHERE key='schema_version'", []).unwrap();
        }
        assert!(matches!(AuditStore::open(&path), Err(StoreError::AuditUnavailable(_))));
        let raw = Connection::open(&path).unwrap();
        let n: i64 = raw.query_row("SELECT count(*) FROM operations", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "history is kept");
    }
}
