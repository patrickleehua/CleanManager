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
CREATE TABLE IF NOT EXISTS plans (
    plan_id TEXT PRIMARY KEY,
    plan_digest TEXT NOT NULL,
    contract TEXT NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    plan_json TEXT NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS receipts (
    receipt_id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL REFERENCES plans(plan_id),
    plan_digest TEXT NOT NULL,
    receipt_json TEXT NOT NULL,
    revoked_at TEXT
) STRICT;
CREATE TABLE IF NOT EXISTS policies (
    policy_id TEXT PRIMARY KEY,
    policy_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS quota_reservations (
    reservation_id TEXT PRIMARY KEY,
    policy_id TEXT NOT NULL,
    operation_id TEXT NOT NULL UNIQUE,
    plan_id TEXT NOT NULL,
    reserved_bytes INTEGER NOT NULL,
    reserved_items INTEGER NOT NULL,
    consumed_bytes INTEGER NOT NULL DEFAULT 0,
    consumed_items INTEGER NOT NULL DEFAULT 0,
    held_unknown_bytes INTEGER NOT NULL DEFAULT 0,
    held_unknown_items INTEGER NOT NULL DEFAULT 0,
    state TEXT NOT NULL CHECK (state IN ('reserved','settled','reconciliation_hold')),
    created_unix INTEGER NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS operation_reports (
    operation_id TEXT PRIMARY KEY REFERENCES operations(operation_id),
    report_json TEXT NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS op_locks (
    identity TEXT PRIMARY KEY,
    operation_id TEXT NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS quarantine_items (
    quarantine_id TEXT PRIMARY KEY,
    operation_id TEXT NOT NULL,
    action_id TEXT NOT NULL,
    plan_id TEXT NOT NULL,
    record_json TEXT NOT NULL,
    state TEXT NOT NULL,
    quarantined_at TEXT NOT NULL,
    reminder_at TEXT NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS registry_backups (
    backup_id TEXT PRIMARY KEY,
    operation_id TEXT NOT NULL,
    action_id TEXT NOT NULL,
    plan_id TEXT NOT NULL,
    record_json TEXT NOT NULL,
    record_sha256 TEXT NOT NULL,
    state TEXT NOT NULL,
    created_at TEXT NOT NULL
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
CREATE TABLE IF NOT EXISTS maintenance_runs (
    operation_id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL,
    finished_at TEXT NOT NULL,
    report_json TEXT NOT NULL
) STRICT;
";

impl AuditStore {
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if check != "ok" {
            return Err(StoreError::AuditUnavailable(format!("integrity check: {check}")));
        }
        // Older schemas are migrated (transactionally, after a copy) before the current schema applies.
        crate::migrate::prepare(&mut conn, crate::migrate::StoreKind::Audit, path)?;
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

/// (quarantine_id, record_json, state, quarantined_at, reminder_at)
pub type QuarantineRow = (String, String, String, String, String);

/// One journal row: the durable intent and, once known, the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalRow {
    pub action_id: String,
    pub intent_json: String,
    pub result_status: Option<String>,
    pub result_json: Option<String>,
}

impl AuditStore {
    pub fn operation_for_dedupe(&self, dedupe_key: &str) -> Result<Option<(String, String)>> {
        Ok(self
            .conn
            .query_row("SELECT operation_id, state FROM operations WHERE dedupe_key=?1", [dedupe_key], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?)
    }

    /// Take identity locks for an operation atomically; returns the operation holding a
    /// conflicting lock, if any (EXE-04). Locks of finished operations were already released.
    pub fn lock_identities(&self, operation_id: &str, identities: &[String]) -> Result<Option<String>> {
        let tx = rusqlite::Transaction::new_unchecked(&self.conn, rusqlite::TransactionBehavior::Immediate)?;
        for id in identities {
            let holder: Option<String> = tx.query_row("SELECT operation_id FROM op_locks WHERE identity=?1", [id], |r| r.get(0)).optional()?;
            if let Some(h) = holder
                && h != operation_id
            {
                tx.rollback()?;
                return Ok(Some(h));
            }
        }
        for id in identities {
            tx.execute("INSERT OR IGNORE INTO op_locks(identity, operation_id) VALUES(?1, ?2)", params![id, operation_id])?;
        }
        tx.commit()?;
        Ok(None)
    }

    pub fn release_locks(&self, operation_id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM op_locks WHERE operation_id=?1", [operation_id])?;
        Ok(())
    }

    /// Durable intent, written before the OS call (EXE-05).
    pub fn record_intent(&self, operation_id: &str, action_id: &str, intent_json: &str, now: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO action_journal(operation_id,action_id,intent_json,intent_at) VALUES(?1,?2,?3,?4)",
            params![operation_id, action_id, intent_json, now],
        )?;
        Ok(())
    }

    pub fn record_result(&self, operation_id: &str, action_id: &str, status: &str, result_json: &str, now: &str) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE action_journal SET result_status=?3, result_json=?4, result_at=?5 WHERE operation_id=?1 AND action_id=?2",
            params![operation_id, action_id, status, result_json, now],
        )?;
        if n == 0 {
            // Results for actions that never got an intent (not started) are recorded directly.
            self.conn.execute(
                "INSERT INTO action_journal(operation_id,action_id,intent_json,intent_at,result_status,result_json,result_at)
                 VALUES(?1,?2,'{}',?5,?3,?4,?5)",
                params![operation_id, action_id, status, result_json, now],
            )?;
        }
        Ok(())
    }

    pub fn journal(&self, operation_id: &str) -> Result<Vec<JournalRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT action_id, intent_json, result_status, result_json FROM action_journal WHERE operation_id=?1 ORDER BY action_id",
        )?;
        let rows = stmt.query_map([operation_id], |r| {
            Ok(JournalRow { action_id: r.get(0)?, intent_json: r.get(1)?, result_status: r.get(2)?, result_json: r.get(3)? })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn operation_plan(&self, operation_id: &str) -> Result<String> {
        self.conn
            .query_row("SELECT plan_id FROM operations WHERE operation_id=?1", [operation_id], |r| r.get(0))
            .optional()?
            .ok_or_else(|| StoreError::NotFound(operation_id.into()))
    }

    /// When an operation was registered and when its journal last recorded an intent or a result (`None`: nothing yet).
    pub fn operation_times(&self, operation_id: &str) -> Result<(String, Option<String>)> {
        let started: String = self
            .conn
            .query_row("SELECT started_at FROM operations WHERE operation_id=?1", [operation_id], |r| r.get(0))
            .optional()?
            .ok_or_else(|| StoreError::NotFound(operation_id.into()))?;
        let last: Option<String> = self.conn.query_row(
            "SELECT MAX(COALESCE(result_at, intent_at)) FROM action_journal WHERE operation_id=?1",
            [operation_id],
            |r| r.get(0),
        )?;
        Ok((started, last))
    }

    pub fn finish_operation(&self, operation_id: &str, state: &str, report_json: &str, now: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("UPDATE operations SET state=?2, finished_at=?3 WHERE operation_id=?1", params![operation_id, state, now])?;
        tx.execute(
            "INSERT INTO operation_reports(operation_id, report_json) VALUES(?1,?2)
             ON CONFLICT(operation_id) DO UPDATE SET report_json=excluded.report_json",
            params![operation_id, report_json],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn operation_report(&self, operation_id: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT report_json FROM operation_reports WHERE operation_id=?1", [operation_id], |r| r.get(0)).optional()?)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn save_quarantine(&self, quarantine_id: &str, operation_id: &str, action_id: &str, plan_id: &str, record_json: &str, now: &str, reminder_at: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO quarantine_items(quarantine_id,operation_id,action_id,plan_id,record_json,state,quarantined_at,reminder_at)
             VALUES(?1,?2,?3,?4,?5,'recoverable',?6,?7)",
            params![quarantine_id, operation_id, action_id, plan_id, record_json, now, reminder_at],
        )?;
        Ok(())
    }

    pub fn quarantine_items(&self) -> Result<Vec<QuarantineRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT quarantine_id, record_json, state, quarantined_at, reminder_at FROM quarantine_items ORDER BY quarantined_at, quarantine_id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn set_quarantine_state(&self, quarantine_id: &str, state: &str) -> Result<()> {
        self.conn.execute("UPDATE quarantine_items SET state=?2 WHERE quarantine_id=?1", params![quarantine_id, state])?;
        Ok(())
    }
}

/// One quarantine manifest row. `state` is the durable bookkeeping state (`recoverable`,
/// `restored`, `purged`); the live state of the object is probed separately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuarantineEntry {
    pub quarantine_id: String,
    pub operation_id: String,
    pub action_id: String,
    pub plan_id: String,
    pub record_json: String,
    pub state: String,
    pub quarantined_at: String,
    pub reminder_at: String,
}

/// One registered operation, newest first in listings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationRow {
    pub operation_id: String,
    pub plan_id: String,
    pub plan_digest: String,
    pub state: String,
    pub started_at: String,
    pub finished_at: Option<String>,
}

/// One stored plan (any type) in listings; the plan body is loaded separately by type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRow {
    pub plan_id: String,
    pub plan_digest: String,
    /// `plan/v1` or `recovery/v1`.
    pub contract: String,
    pub created_at: String,
    pub expires_at: String,
}

/// Opaque keyset cursor: the insertion key (rowid) of the last row and the page size it was
/// issued for. Rows are never renumbered, so a page boundary is stable while history grows.
/// The quarantine listing's cursor kind: newest first (R3-05).
const QUARANTINE_CURSOR: &str = "quarantine_newest";

fn encode_cursor(kind: &str, rowid: i64, page_size: u32) -> String {
    use base64::Engine;
    let text = serde_json::json!([kind, rowid, page_size]).to_string();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(text)
}

fn decode_cursor(kind: &str, cursor: &str, page_size: u32) -> Result<i64> {
    use base64::Engine;
    let bad = || StoreError::CursorStale("malformed cursor".into());
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(cursor).map_err(|_| bad())?;
    let (k, rowid, n): (String, i64, u32) = serde_json::from_slice(&bytes).map_err(|_| bad())?;
    if k != kind || n != page_size {
        return Err(StoreError::CursorStale("cursor belongs to another listing or page size".into()));
    }
    Ok(rowid)
}

fn check_page(page_size: u32) -> Result<()> {
    if page_size == 0 || page_size > crate::index::MAX_PAGE {
        return Err(StoreError::InvalidArgument(format!("page_size must be 1..={}", crate::index::MAX_PAGE)));
    }
    Ok(())
}

impl AuditStore {
    fn quarantine_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<QuarantineEntry> {
        Ok(QuarantineEntry {
            quarantine_id: r.get(0)?,
            operation_id: r.get(1)?,
            action_id: r.get(2)?,
            plan_id: r.get(3)?,
            record_json: r.get(4)?,
            state: r.get(5)?,
            quarantined_at: r.get(6)?,
            reminder_at: r.get(7)?,
        })
    }

    pub fn quarantine_entry(&self, quarantine_id: &str) -> Result<QuarantineEntry> {
        self.conn
            .query_row(
                "SELECT quarantine_id, operation_id, action_id, plan_id, record_json, state, quarantined_at, reminder_at FROM quarantine_items WHERE quarantine_id=?1",
                [quarantine_id],
                Self::quarantine_row,
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound(quarantine_id.into()))
    }

    /// Items still held in quarantine (`recoverable`) in quarantine order, keyset-paginated. Reaching
    /// `reminder_at` changes nothing here: expired items stay listed until a person decides.
    pub fn quarantine_page(&self, page_size: u32, cursor: Option<&str>) -> Result<(Vec<QuarantineEntry>, Option<String>)> {
        check_page(page_size)?;
        // R3-05: most recently quarantined first, like `operations list` (one order for the CLI and the desktop; the
        // cursor names its own order, so a cursor of the older oldest-first listing is stale, never misread).
        let before = cursor.map(|c| decode_cursor(QUARANTINE_CURSOR, c, page_size)).transpose()?.unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare(
            "SELECT quarantine_id, operation_id, action_id, plan_id, record_json, state, quarantined_at, reminder_at, rowid FROM quarantine_items
             WHERE state='recoverable' AND rowid < ?1 ORDER BY rowid DESC LIMIT ?2",
        )?;
        let mut rows: Vec<(QuarantineEntry, i64)> = stmt
            .query_map(params![before, i64::from(page_size) + 1], |r| Ok((Self::quarantine_row(r)?, r.get(8)?)))?
            .collect::<std::result::Result<_, _>>()?;
        let next = (rows.len() > page_size as usize).then(|| {
            rows.truncate(page_size as usize);
            encode_cursor(QUARANTINE_CURSOR, rows.last().expect("page is not empty").1, page_size)
        });
        let rows = rows.into_iter().map(|(e, _)| e).collect();
        Ok((rows, next))
    }

    /// Quarantine rows written by one operation (any state).
    pub fn quarantine_for_operation(&self, operation_id: &str) -> Result<Vec<QuarantineEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT quarantine_id, operation_id, action_id, plan_id, record_json, state, quarantined_at, reminder_at FROM quarantine_items
             WHERE operation_id=?1 ORDER BY quarantine_id",
        )?;
        let rows = stmt.query_map([operation_id], Self::quarantine_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Operation history, most recently registered first, keyset-paginated (REC-04). Never deleted by an index rebuild.
    pub fn operations_page(&self, page_size: u32, cursor: Option<&str>) -> Result<(Vec<OperationRow>, Option<String>)> {
        check_page(page_size)?;
        let before = cursor.map(|c| decode_cursor("operations", c, page_size)).transpose()?.unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare(
            "SELECT operation_id, plan_id, plan_digest, state, started_at, finished_at, rowid FROM operations
             WHERE rowid < ?1 ORDER BY rowid DESC LIMIT ?2",
        )?;
        let mut rows: Vec<(OperationRow, i64)> = stmt
            .query_map(params![before, i64::from(page_size) + 1], |r| {
                Ok((OperationRow { operation_id: r.get(0)?, plan_id: r.get(1)?, plan_digest: r.get(2)?, state: r.get(3)?, started_at: r.get(4)?, finished_at: r.get(5)? }, r.get(6)?))
            })?
            .collect::<std::result::Result<_, _>>()?;
        let next = (rows.len() > page_size as usize).then(|| {
            rows.truncate(page_size as usize);
            encode_cursor("operations", rows.last().expect("page is not empty").1, page_size)
        });
        let rows = rows.into_iter().map(|(o, _)| o).collect();
        Ok((rows, next))
    }

    /// Stored plans of every type, most recently created first, keyset-paginated (P08 timeline).
    pub fn plans_page(&self, page_size: u32, cursor: Option<&str>) -> Result<(Vec<PlanRow>, Option<String>)> {
        check_page(page_size)?;
        let before = cursor.map(|c| decode_cursor("plans", c, page_size)).transpose()?.unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare(
            "SELECT plan_id, plan_digest, contract, created_at, expires_at, rowid FROM plans WHERE rowid < ?1 ORDER BY rowid DESC LIMIT ?2",
        )?;
        let mut rows: Vec<(PlanRow, i64)> = stmt
            .query_map(params![before, i64::from(page_size) + 1], |r| {
                Ok((PlanRow { plan_id: r.get(0)?, plan_digest: r.get(1)?, contract: r.get(2)?, created_at: r.get(3)?, expires_at: r.get(4)? }, r.get(5)?))
            })?
            .collect::<std::result::Result<_, _>>()?;
        let next = (rows.len() > page_size as usize).then(|| {
            rows.truncate(page_size as usize);
            encode_cursor("plans", rows.last().expect("page is not empty").1, page_size)
        });
        Ok((rows.into_iter().map(|(p, _)| p).collect(), next))
    }

    pub fn save_recovery_plan(&self, plan: &cm_core::recovery::RecoveryPlan) -> Result<()> {
        self.conn.execute(
            "INSERT INTO plans(plan_id,plan_digest,contract,created_at,expires_at,plan_json) VALUES(?1,?2,?3,?4,?5,?6)",
            params![plan.body.plan_id, plan.digest, plan.body.engine_contract, plan.body.created_at, plan.body.expires_at,
                serde_json::to_string(plan).expect("recovery plan serializes")],
        )?;
        Ok(())
    }

    pub fn load_recovery_plan(&self, plan_id: &str) -> Result<cm_core::recovery::RecoveryPlan> {
        let (contract, text): (String, String) = self
            .conn
            .query_row("SELECT contract, plan_json FROM plans WHERE plan_id=?1", [plan_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?
            .ok_or_else(|| StoreError::NotFound(plan_id.into()))?;
        if contract != cm_core::recovery::RECOVERY_CONTRACT {
            return Err(StoreError::InvalidState(format!("plan {plan_id} is a {contract} plan, not a recovery plan")));
        }
        let plan: cm_core::recovery::RecoveryPlan = serde_json::from_str(&text).map_err(|e| StoreError::AuditUnavailable(e.to_string()))?;
        if plan.body.digest().ok().as_deref() != Some(plan.digest.as_str()) {
            return Err(StoreError::AuditUnavailable(format!("plan {plan_id} digest does not match its body")));
        }
        Ok(plan)
    }

    /// Recovery receipts share the receipts table (and `revoke_receipts`) with cleanup receipts.
    pub fn save_recovery_receipt(&self, r: &cm_core::recovery::RecoveryReceipt) -> Result<()> {
        if self.plan_contract(&r.plan_id)? != cm_core::recovery::RECOVERY_CONTRACT {
            return Err(StoreError::InvalidState(format!("plan {} is not a recovery plan", r.plan_id)));
        }
        self.conn.execute(
            "INSERT INTO receipts(receipt_id,plan_id,plan_digest,receipt_json,revoked_at) VALUES(?1,?2,?3,?4,NULL)",
            params![r.receipt_id, r.plan_id, r.plan_digest, serde_json::to_string(r).expect("receipt serializes")],
        )?;
        Ok(())
    }

    /// Recovery receipts of a plan. Rows that are not recovery receipts are ignored, never
    /// reinterpreted (a cleanup approval cannot authorize a restore or purge).
    pub fn recovery_receipts_for(&self, plan_id: &str) -> Result<Vec<cm_core::recovery::RecoveryReceipt>> {
        let mut stmt = self.conn.prepare("SELECT receipt_json, revoked_at FROM receipts WHERE plan_id=?1 ORDER BY receipt_id")?;
        let rows = stmt.query_map([plan_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (json, revoked) = row?;
            if let Ok(mut rec) = serde_json::from_str::<cm_core::recovery::RecoveryReceipt>(&json) {
                rec.revoked_at = revoked;
                out.push(rec);
            }
        }
        Ok(out)
    }
}

/// Rolling usage of a policy window: reserved (not yet settled) plus consumed plus amounts
/// held because their outcome is unknown. Released amounts no longer count (PLN-06).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WindowUsage {
    pub bytes: u64,
    pub items: u64,
}

impl AuditStore {
    /// Raw connection for tamper tests only.
    #[doc(hidden)]
    pub fn conn_for_tests(&self) -> &Connection {
        &self.conn
    }

    pub fn save_plan(&self, plan: &cm_core::plan::Plan) -> Result<()> {
        self.conn.execute(
            "INSERT INTO plans(plan_id,plan_digest,contract,created_at,expires_at,plan_json) VALUES(?1,?2,?3,?4,?5,?6)",
            params![plan.body.plan_id, plan.digest, plan.body.engine_contract, plan.body.created_at, plan.body.expires_at,
                serde_json::to_string(plan).expect("plan serializes")],
        )?;
        Ok(())
    }

    /// The stored, authoritative plan type (`plan/v1`, `recovery/v1`); callers dispatch on it.
    pub fn plan_contract(&self, plan_id: &str) -> Result<String> {
        self.conn
            .query_row("SELECT contract FROM plans WHERE plan_id=?1", [plan_id], |r| r.get(0))
            .optional()?
            .ok_or_else(|| StoreError::NotFound(plan_id.into()))
    }

    pub fn load_plan(&self, plan_id: &str) -> Result<cm_core::plan::Plan> {
        let (contract, text): (String, String) = self
            .conn
            .query_row("SELECT contract, plan_json FROM plans WHERE plan_id=?1", [plan_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?
            .ok_or_else(|| StoreError::NotFound(plan_id.into()))?;
        if contract != "plan/v1" {
            return Err(StoreError::InvalidState(format!("plan {plan_id} is a {contract} plan, not a cleanup plan")));
        }
        let plan: cm_core::plan::Plan = serde_json::from_str(&text).map_err(|e| StoreError::AuditUnavailable(e.to_string()))?;
        // The stored body must still hash to the stored digest; tampering is never trusted.
        if plan.body.digest().ok().as_deref() != Some(plan.digest.as_str()) {
            return Err(StoreError::AuditUnavailable(format!("plan {plan_id} digest does not match its body")));
        }
        Ok(plan)
    }

    pub fn save_receipt(&self, r: &cm_core::authz::Receipt) -> Result<()> {
        self.conn.execute(
            "INSERT INTO receipts(receipt_id,plan_id,plan_digest,receipt_json,revoked_at) VALUES(?1,?2,?3,?4,NULL)",
            params![r.receipt_id, r.plan_id, r.plan_digest, serde_json::to_string(r).expect("receipt serializes")],
        )?;
        Ok(())
    }

    pub fn receipts_for(&self, plan_id: &str) -> Result<Vec<cm_core::authz::Receipt>> {
        let mut stmt = self.conn.prepare("SELECT receipt_json, revoked_at FROM receipts WHERE plan_id=?1 ORDER BY receipt_id")?;
        let rows = stmt.query_map([plan_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (json, revoked) = row?;
            let mut rec: cm_core::authz::Receipt = serde_json::from_str(&json).map_err(|e| StoreError::AuditUnavailable(e.to_string()))?;
            rec.revoked_at = revoked;
            out.push(rec);
        }
        Ok(out)
    }

    pub fn revoke_receipts(&self, plan_id: &str, now: &str) -> Result<usize> {
        Ok(self.conn.execute("UPDATE receipts SET revoked_at=?2 WHERE plan_id=?1 AND revoked_at IS NULL", params![plan_id, now])?)
    }

    pub fn save_policy(&self, policy: &cm_core::authz::Policy, now: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO policies(policy_id,policy_json,updated_at) VALUES(?1,?2,?3)
             ON CONFLICT(policy_id) DO UPDATE SET policy_json=excluded.policy_json, updated_at=excluded.updated_at",
            params![policy.policy_id, serde_json::to_string(policy).expect("policy serializes"), now],
        )?;
        Ok(())
    }

    pub fn policies(&self) -> Result<Vec<cm_core::authz::Policy>> {
        let mut stmt = self.conn.prepare("SELECT policy_json FROM policies ORDER BY policy_id")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|j| serde_json::from_str(&j?).map_err(|e| StoreError::AuditUnavailable(e.to_string()))).collect()
    }

    fn window_usage(conn: &Connection, policy_id: &str, since_unix: i64) -> Result<WindowUsage> {
        let (b, i): (i64, i64) = conn.query_row(
            "SELECT COALESCE(SUM(CASE state WHEN 'reserved' THEN reserved_bytes ELSE consumed_bytes + held_unknown_bytes END),0),
                    COALESCE(SUM(CASE state WHEN 'reserved' THEN reserved_items ELSE consumed_items + held_unknown_items END),0)
             FROM quota_reservations WHERE policy_id=?1 AND created_unix>=?2",
            params![policy_id, since_unix],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(WindowUsage { bytes: b as u64, items: i as u64 })
    }

    pub fn policy_usage(&self, policy_id: &str, now_unix: i64) -> Result<WindowUsage> {
        Self::window_usage(&self.conn, policy_id, now_unix - 86_400)
    }

    /// Atomically reserve quota for one operation (cross-process: BEGIN IMMEDIATE takes the
    /// database write lock before reading usage). The same operation never reserves twice.
    pub fn reserve_quota(&self, policy: &cm_core::authz::Policy, operation_id: &str, plan_id: &str, bytes: u64, items: u64, now_unix: i64) -> Result<bool> {
        let tx = rusqlite::Transaction::new_unchecked(&self.conn, rusqlite::TransactionBehavior::Immediate)?;
        let existing: Option<String> =
            tx.query_row("SELECT reservation_id FROM quota_reservations WHERE operation_id=?1", [operation_id], |r| r.get(0)).optional()?;
        if existing.is_some() {
            tx.commit()?;
            return Ok(true);
        }
        let used = Self::window_usage(&tx, &policy.policy_id, now_unix - 86_400)?;
        if used.bytes.saturating_add(bytes) > policy.window_max_bytes || used.items.saturating_add(items) > policy.window_max_items {
            tx.commit()?;
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO quota_reservations(reservation_id,policy_id,operation_id,plan_id,reserved_bytes,reserved_items,state,created_unix)
             VALUES(?1,?2,?3,?4,?5,?6,'reserved',?7)",
            params![format!("q_{operation_id}"), policy.policy_id, operation_id, plan_id, bytes as i64, items as i64, now_unix],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Settle a reservation from per-action outcomes: succeeded amounts are consumed, unknown
    /// amounts stay held until reconciliation, the rest is released.
    pub fn settle_quota(&self, operation_id: &str, consumed: (u64, u64), unknown: (u64, u64)) -> Result<()> {
        let state = if unknown.1 > 0 { "reconciliation_hold" } else { "settled" };
        self.conn.execute(
            "UPDATE quota_reservations SET consumed_bytes=?2, consumed_items=?3, held_unknown_bytes=?4, held_unknown_items=?5, state=?6
             WHERE operation_id=?1",
            params![operation_id, consumed.0 as i64, consumed.1 as i64, unknown.0 as i64, unknown.1 as i64, state],
        )?;
        Ok(())
    }
}

/// `plan/v2` plans, their kind-bound receipts and the startup/registry prior-state records (add-mole-parity-suite).
impl AuditStore {
    pub fn save_plan_v2(&self, plan: &cm_core::plan::v2::PlanV2) -> Result<()> {
        self.conn.execute(
            "INSERT INTO plans(plan_id,plan_digest,contract,created_at,expires_at,plan_json) VALUES(?1,?2,?3,?4,?5,?6)",
            params![plan.body.plan_id, plan.digest, plan.body.engine_contract, plan.body.created_at, plan.body.expires_at,
                serde_json::to_string(plan).expect("plan serializes")],
        )?;
        Ok(())
    }

    /// A stored `plan/v2` plan whose body still hashes to its digest. Kind rules are re-checked by the caller.
    pub fn load_plan_v2(&self, plan_id: &str) -> Result<cm_core::plan::v2::PlanV2> {
        let (contract, text): (String, String) = self
            .conn
            .query_row("SELECT contract, plan_json FROM plans WHERE plan_id=?1", [plan_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?
            .ok_or_else(|| StoreError::NotFound(plan_id.into()))?;
        if contract != cm_core::plan::v2::PLAN_V2_CONTRACT {
            return Err(StoreError::InvalidState(format!("plan {plan_id} is a {contract} plan, not a plan/v2 plan")));
        }
        let plan: cm_core::plan::v2::PlanV2 = serde_json::from_str(&text).map_err(|e| StoreError::AuditUnavailable(e.to_string()))?;
        if plan.body.digest().ok().as_deref() != Some(plan.digest.as_str()) || plan.body.plan_id != plan_id {
            return Err(StoreError::AuditUnavailable(format!("plan {plan_id} digest does not match its body")));
        }
        Ok(plan)
    }

    /// Kind receipts share the receipts table (and `revoke_receipts`) with the other receipt types.
    pub fn save_kind_receipt(&self, r: &cm_core::authz::KindReceipt) -> Result<()> {
        if self.plan_contract(&r.plan_id)? != cm_core::plan::v2::PLAN_V2_CONTRACT {
            return Err(StoreError::InvalidState(format!("plan {} is not a plan/v2 plan", r.plan_id)));
        }
        self.conn.execute(
            "INSERT INTO receipts(receipt_id,plan_id,plan_digest,receipt_json,revoked_at) VALUES(?1,?2,?3,?4,NULL)",
            params![r.receipt_id, r.plan_id, r.plan_digest, serde_json::to_string(r).expect("receipt serializes")],
        )?;
        Ok(())
    }

    /// Kind receipts of a plan. Rows of other receipt types are ignored, never reinterpreted.
    pub fn kind_receipts_for(&self, plan_id: &str) -> Result<Vec<cm_core::authz::KindReceipt>> {
        let mut stmt = self.conn.prepare("SELECT receipt_json, revoked_at FROM receipts WHERE plan_id=?1 ORDER BY receipt_id")?;
        let rows = stmt.query_map([plan_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (json, revoked) = row?;
            if let Ok(mut rec) = serde_json::from_str::<cm_core::authz::KindReceipt>(&json) {
                rec.revoked_at = revoked;
                out.push(rec);
            }
        }
        Ok(out)
    }

    /// Store a prior-state record before the change it backs up (durable: synchronous FULL, never pruned with the index).
    pub fn save_startup_backup(&self, operation_id: &str, action_id: &str, plan_id: &str, b: &cm_core::startup::StartupBackup) -> Result<()> {
        self.conn.execute(
            "INSERT INTO registry_backups(backup_id,operation_id,action_id,plan_id,record_json,record_sha256,state,created_at) VALUES(?1,?2,?3,?4,?5,?6,'recorded',?7)",
            params![b.backup_id, operation_id, action_id, plan_id, serde_json::to_string(b).expect("backup serializes"), b.sha256(), b.created_at],
        )?;
        Ok(())
    }

    /// A stored prior-state record, verified against the hash recorded with it.
    pub fn startup_backup(&self, backup_id: &str) -> Result<(cm_core::startup::StartupBackup, String)> {
        let (json, sha, state): (String, String, String) = self
            .conn
            .query_row("SELECT record_json, record_sha256, state FROM registry_backups WHERE backup_id=?1", [backup_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?
            .ok_or_else(|| StoreError::NotFound(backup_id.into()))?;
        let b: cm_core::startup::StartupBackup = serde_json::from_str(&json).map_err(|e| StoreError::AuditUnavailable(e.to_string()))?;
        if b.sha256() != sha || b.backup_id != backup_id {
            return Err(StoreError::AuditUnavailable(format!("backup {backup_id} does not match its recorded hash")));
        }
        Ok((b, state))
    }

    /// Prior-state records written by one operation, in action order: (action_id, record, state).
    pub fn startup_backups_for_operation(&self, operation_id: &str) -> Result<Vec<(String, cm_core::startup::StartupBackup, String)>> {
        let mut stmt = self.conn.prepare("SELECT action_id, backup_id FROM registry_backups WHERE operation_id=?1 ORDER BY action_id")?;
        let ids: Vec<(String, String)> = stmt.query_map([operation_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?;
        ids.into_iter().map(|(a, id)| self.startup_backup(&id).map(|(b, s)| (a, b, s))).collect()
    }

    pub fn set_startup_backup_state(&self, backup_id: &str, state: &str) -> Result<()> {
        self.conn.execute("UPDATE registry_backups SET state=?2 WHERE backup_id=?1", params![backup_id, state])?;
        Ok(())
    }

    /// Store an uninstall registry-key backup (UNI-06) in the same durable table, before the delete it backs up.
    pub fn save_key_backup(&self, operation_id: &str, action_id: &str, plan_id: &str, b: &cm_core::uninstall::KeyBackup) -> Result<()> {
        self.conn.execute(
            "INSERT INTO registry_backups(backup_id,operation_id,action_id,plan_id,record_json,record_sha256,state,created_at) VALUES(?1,?2,?3,?4,?5,?6,'recorded',?7)",
            params![b.backup_id, operation_id, action_id, plan_id, serde_json::to_string(b).expect("backup serializes"), b.sha256(), b.created_at],
        )?;
        Ok(())
    }

    /// A stored uninstall key backup, verified against the hash recorded with it: (record, state).
    pub fn key_backup(&self, backup_id: &str) -> Result<(cm_core::uninstall::KeyBackup, String)> {
        let (json, sha, state): (String, String, String) = self
            .conn
            .query_row("SELECT record_json, record_sha256, state FROM registry_backups WHERE backup_id=?1", [backup_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?
            .ok_or_else(|| StoreError::NotFound(backup_id.into()))?;
        let b: cm_core::uninstall::KeyBackup = serde_json::from_str(&json).map_err(|e| StoreError::AuditUnavailable(e.to_string()))?;
        if b.sha256() != sha || b.backup_id != backup_id {
            return Err(StoreError::AuditUnavailable(format!("backup {backup_id} does not match its recorded hash")));
        }
        Ok((b, state))
    }

    /// Every backup row of one operation (startup and key backups alike): (action_id, backup_id, state).
    pub fn backup_ids_for_operation(&self, operation_id: &str) -> Result<Vec<(String, String, String)>> {
        let mut stmt = self.conn.prepare("SELECT action_id, backup_id, state FROM registry_backups WHERE operation_id=?1 ORDER BY action_id")?;
        let rows = stmt.query_map([operation_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }
}

/// Maintenance run records (parity MNT-07): one `maintenance_report` per operation, newest first in history.
impl AuditStore {
    pub fn save_maintenance_run(&self, operation_id: &str, plan_id: &str, finished_at: &str, report_json: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO maintenance_runs(operation_id,plan_id,finished_at,report_json) VALUES(?1,?2,?3,?4)",
            params![operation_id, plan_id, finished_at, report_json],
        )?;
        Ok(())
    }

    pub fn maintenance_runs(&self, limit: u32) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT report_json FROM maintenance_runs ORDER BY finished_at DESC, operation_id DESC LIMIT ?1")?;
        let rows = stmt.query_map([limit], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }

    /// Registry/startup backups not yet restored (kept so earlier removals stay restorable; LIF-02).
    pub fn retained_backup_count(&self) -> Result<u64> {
        let n: i64 = self.conn.query_row("SELECT count(*) FROM registry_backups WHERE state <> 'restored'", [], |r| r.get(0))?;
        Ok(n as u64)
    }
}

/// Prior values of maintenance setting changes (parity 14.5, MNT-07), in the same durable backup table, stored before
/// the write they back up.
impl AuditStore {
    pub fn save_setting_backup(&self, operation_id: &str, action_id: &str, plan_id: &str, b: &cm_core::maintenance::settings::SettingBackup) -> Result<()> {
        self.conn.execute(
            "INSERT INTO registry_backups(backup_id,operation_id,action_id,plan_id,record_json,record_sha256,state,created_at) VALUES(?1,?2,?3,?4,?5,?6,'recorded',?7)",
            params![b.backup_id, operation_id, action_id, plan_id, serde_json::to_string(b).expect("backup serializes"), b.sha256(), b.created_at],
        )?;
        Ok(())
    }

    /// A stored setting backup, verified against the hash recorded with it: (record, state).
    pub fn setting_backup(&self, backup_id: &str) -> Result<(cm_core::maintenance::settings::SettingBackup, String)> {
        let (json, sha, state): (String, String, String) = self
            .conn
            .query_row("SELECT record_json, record_sha256, state FROM registry_backups WHERE backup_id=?1", [backup_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?
            .ok_or_else(|| StoreError::NotFound(backup_id.into()))?;
        let b: cm_core::maintenance::settings::SettingBackup = serde_json::from_str(&json).map_err(|e| StoreError::AuditUnavailable(e.to_string()))?;
        if b.sha256() != sha || b.backup_id != backup_id {
            return Err(StoreError::AuditUnavailable(format!("backup {backup_id} does not match its recorded hash")));
        }
        Ok((b, state))
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
    fn operations_and_quarantine_are_keyset_paginated() {
        let dir = scratch("audit-pages");
        let a = AuditStore::open(&dir.join("audit.db")).unwrap();
        for i in 0..5 {
            a.begin_operation(&format!("op{i}"), "p", "d", &format!("k{i}"), &host(1), &format!("2026-10-15T00:00:0{i}Z")).unwrap();
            a.save_quarantine(&format!("q{i}"), &format!("op{i}"), "a", "p", "{}", &format!("2026-10-15T00:00:0{i}Z"), "2026-11-14T00:00:00Z").unwrap();
        }
        a.set_quarantine_state("q2", "restored").unwrap();
        let (first, next) = a.operations_page(2, None).unwrap();
        assert_eq!(first.iter().map(|o| o.operation_id.as_str()).collect::<Vec<_>>(), ["op4", "op3"], "most recent first");
        let (second, next) = a.operations_page(2, next.as_deref()).unwrap();
        assert_eq!(second.iter().map(|o| o.operation_id.as_str()).collect::<Vec<_>>(), ["op2", "op1"]);
        let (last, next) = a.operations_page(2, next.as_deref()).unwrap();
        assert_eq!((last.len(), next), (1, None));
        let (q, next) = a.quarantine_page(3, None).unwrap();
        assert_eq!(q.iter().map(|e| e.quarantine_id.as_str()).collect::<Vec<_>>(), ["q4", "q3", "q1"], "most recent first; restored items leave the list");
        let (q, _) = a.quarantine_page(3, next.as_deref()).unwrap();
        assert_eq!(q.iter().map(|e| e.quarantine_id.as_str()).collect::<Vec<_>>(), ["q0"]);
        assert!(matches!(a.quarantine_page(2, next.as_deref()), Err(StoreError::CursorStale(_))), "cursor bound to its page size");
        let oldest_first = encode_cursor("quarantine", 2, 3);
        assert!(matches!(a.quarantine_page(3, Some(&oldest_first)), Err(StoreError::CursorStale(_))), "a cursor of the old order is stale");
        assert!(matches!(a.operations_page(3, next.as_deref()), Err(StoreError::CursorStale(_))), "cursor bound to its listing");
        assert!(matches!(a.operations_page(501, None), Err(StoreError::InvalidArgument(_))));
    }

    #[test]
    fn plans_are_listed_newest_first_with_their_type() {
        use cm_core::recovery::{QuarantineManifest, RecoveryKind, build_recovery_plan};
        let dir = scratch("audit-plans");
        let a = AuditStore::open(&dir.join("audit.db")).unwrap();
        let id = |f: &str| cm_core::plan::WireIdentity { volume_id: "v".into(), file_id: format!("inode64:{f:0>16}"), generation: None, quality: "strong".into(), observed_at: "2026-10-15T00:00:00Z".into() };
        for i in 0..3 {
            let m = QuarantineManifest {
                original_path: cm_core::path::RawPath::unix(b"/r/a".to_vec()).unwrap(), original_identity: id("1"), original_parent: Some(id("2")),
                location: cm_core::path::RawPath::unix(b"/q/a".to_vec()).unwrap(), logical_bytes: cm_core::bytes::Bytes(1),
            };
            a.save_recovery_plan(&build_recovery_plan(RecoveryKind::Restore, &[("q1".into(), m)], &format!("rp{i}"), 1_800_000_000 + i).unwrap()).unwrap();
        }
        let (first, next) = a.plans_page(2, None).unwrap();
        assert_eq!(first.iter().map(|p| p.plan_id.as_str()).collect::<Vec<_>>(), ["rp2", "rp1"]);
        assert_eq!(first[0].contract, "recovery/v1");
        let (rest, end) = a.plans_page(2, next.as_deref()).unwrap();
        assert_eq!((rest.len(), end), (1, None));
        assert!(matches!(a.plans_page(2, Some("bogus")), Err(StoreError::CursorStale(_))));
    }

    #[test]
    fn plan_type_is_authoritative_and_receipts_do_not_cross_types() {
        use cm_core::recovery::{QuarantineManifest, RecoveryKind, RecoveryReceipt, build_recovery_plan};
        let dir = scratch("audit-recovery");
        let a = AuditStore::open(&dir.join("audit.db")).unwrap();
        let id = |f: &str| cm_core::plan::WireIdentity { volume_id: "v".into(), file_id: format!("inode64:{f:0>16}"), generation: None, quality: "strong".into(), observed_at: "2026-10-15T00:00:00Z".into() };
        let m = QuarantineManifest {
            original_path: cm_core::path::RawPath::unix(b"/r/a".to_vec()).unwrap(), original_identity: id("1"), original_parent: Some(id("2")),
            location: cm_core::path::RawPath::unix(b"/q/a".to_vec()).unwrap(), logical_bytes: cm_core::bytes::Bytes(1),
        };
        let plan = build_recovery_plan(RecoveryKind::Restore, &[("q1".into(), m)], "rp1", 1_800_000_000).unwrap();
        a.save_recovery_plan(&plan).unwrap();
        assert_eq!(a.plan_contract("rp1").unwrap(), "recovery/v1");
        assert!(matches!(a.load_plan("rp1"), Err(StoreError::InvalidState(_))), "never read as a cleanup plan");
        assert_eq!(a.load_recovery_plan("rp1").unwrap(), plan);
        // A cleanup-shaped receipt stored against the recovery plan is ignored, never reinterpreted.
        a.save_receipt(&cm_core::authz::Receipt {
            receipt_id: "c1".into(), plan_id: "rp1".into(), plan_digest: plan.digest.clone(), mode: cm_core::rules::Mode::Quarantine,
            principal: "user:a".into(), source: cm_core::authz::ReceiptSource::InteractiveReview, policy_id: None, policy_version: None,
            created_at: "t".into(), expires_at: plan.body.expires_at.clone(), revoked_at: None,
        }).unwrap();
        assert!(a.recovery_receipts_for("rp1").unwrap().is_empty());
        let r = RecoveryReceipt {
            receipt_id: "r1".into(), plan_id: "rp1".into(), plan_digest: plan.digest.clone(), recovery_kind: RecoveryKind::Restore,
            principal: "user:a".into(), source: cm_core::authz::ReceiptSource::InteractiveReview, created_at: "t".into(),
            expires_at: plan.body.expires_at.clone(), revoked_at: None,
        };
        a.save_recovery_receipt(&r).unwrap();
        assert_eq!(a.recovery_receipts_for("rp1").unwrap(), vec![r.clone()]);
        a.revoke_receipts("rp1", "2026-10-15T00:00:01Z").unwrap();
        assert!(a.recovery_receipts_for("rp1").unwrap()[0].revoked_at.is_some());
        assert!(a.save_recovery_receipt(&RecoveryReceipt { receipt_id: "r2".into(), plan_id: "missing".into(), ..r }).is_err());
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
