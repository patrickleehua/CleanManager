//! Versioned, transactional store migrations (MVP 14.3; IDX-05, REC-04, GUI-07).
//!
//! Both databases carry `meta.schema_version`. When a newer program opens an older store, the
//! steps from [`INDEX_MIGRATIONS`] / [`AUDIT_MIGRATIONS`] run before the current schema is
//! applied, under these rules:
//!
//! - **One transaction.** Every step and the version bump run inside one `BEGIN IMMEDIATE`
//!   transaction; any failure rolls the whole migration back, so the store is left exactly at its
//!   old version and stays usable by the old program.
//! - **A copy first (audit).** Before the audit store changes, a consistent copy is written with
//!   `VACUUM INTO` next to it (`audit.db.pre-v{N}-{time}.bak`) and kept. The index is rebuildable
//!   and gets no copy (IDX-05): if its migration fails it is reported as incompatible and may be
//!   rebuilt, which never touches the audit store.
//! - **Logs kept.** Every attempt appends a JSON line to `migrations.log` in the store directory
//!   (started / completed / failed / refused) and is flushed before the result is returned; a
//!   rollback never removes a log line.
//! - **Never replays old actions.** The facts that decide what may run again — operations and
//!   their states, the action journal and its results, quarantine records, registry/startup
//!   backups, plans and receipts — are fingerprinted before and after the steps inside the same
//!   transaction. A migration that adds, removes or changes any of them is rolled back. Opening a
//!   store never executes a plan; a migrated `reconciliation_required` operation stays exactly that.
//! - **No downgrade, no re-initialization.** A store newer than this program, a non-numeric
//!   version or a version without a complete chain of steps is refused and left untouched.

use std::io::Write;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use sha2::{Digest, Sha256};

use crate::{Result, StoreError};

/// Which database a migration belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreKind {
    Index,
    Audit,
}

impl StoreKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StoreKind::Index => "index",
            StoreKind::Audit => "audit",
        }
    }

    /// The schema version this program writes.
    pub fn current(self) -> u32 {
        let v = match self {
            StoreKind::Index => crate::index::INDEX_SCHEMA_VERSION,
            StoreKind::Audit => crate::audit::AUDIT_SCHEMA_VERSION,
        };
        v.parse().expect("schema versions are integers")
    }

    fn registry(self) -> &'static [Migration] {
        match self {
            StoreKind::Index => INDEX_MIGRATIONS,
            StoreKind::Audit => AUDIT_MIGRATIONS,
        }
    }

    fn error(self, message: String) -> StoreError {
        match self {
            StoreKind::Index => StoreError::IndexCorrupt(message),
            StoreKind::Audit => StoreError::AuditUnavailable(message),
        }
    }
}

/// One schema step `from` → `to` (`to == from + 1`). `apply` runs inside the migration
/// transaction (it cannot commit it) and must not execute plans or touch anything outside the database.
pub struct Migration {
    pub from: u32,
    pub to: u32,
    pub description: &'static str,
    pub apply: fn(&Transaction<'_>) -> rusqlite::Result<()>,
}

/// Index steps. Version 1 is the first released schema.
pub const INDEX_MIGRATIONS: &[Migration] = &[Migration {
    from: 1,
    to: 2,
    description: "objects.shared_blocks (APFS clone block sharing; NULL = not determined)",
    apply: index_v1_to_v2,
}];

/// v1 → v2: a nullable column; existing rows keep NULL ("not determined"), which is what v1 scans observed.
fn index_v1_to_v2(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch("ALTER TABLE objects ADD COLUMN shared_blocks INTEGER")
}
/// Audit/recovery steps. Version 1 is the first released schema; there is nothing to migrate yet.
pub const AUDIT_MIGRATIONS: &[Migration] = &[];

/// Append-only log next to the databases.
pub const LOG_FILE: &str = "migrations.log";

/// What [`migrate`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// No schema yet (a new database).
    Fresh,
    UpToDate,
    Migrated { from: u32, to: u32, backup: Option<PathBuf> },
}

/// Called by `IndexStore::open` / `AuditStore::open` before the current schema is applied.
pub fn prepare(conn: &mut Connection, kind: StoreKind, db_path: &Path) -> Result<Outcome> {
    let target = kind.current();
    match migrate(conn, kind, db_path, target, kind.registry()) {
        Ok(o) => Ok(o),
        // The index is rebuildable: report it as incompatible so the caller offers a rebuild.
        Err(StoreError::IndexCorrupt(msg)) if kind == StoreKind::Index => {
            Err(StoreError::IndexIncompatible { found: read_version(conn).ok().flatten().unwrap_or_else(|| "?".into()), expected: format!("{target} ({msg})") })
        }
        Err(e) => Err(e),
    }
}

/// `meta.schema_version`, or `None` when the database has no schema yet.
pub fn read_version(conn: &Connection) -> rusqlite::Result<Option<String>> {
    let has_meta: bool = conn.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='meta'", [], |r| r.get::<_, i64>(0)).map(|n| n > 0)?;
    if !has_meta {
        return Ok(None);
    }
    conn.query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| r.get(0)).optional()
}

/// Schema versions of the stores in `dir` without creating, migrating or locking anything
/// (read-only connections). `None` = the database does not exist or has no schema.
pub fn store_versions(dir: &Path) -> (Option<String>, Option<String>) {
    let read = |name: &str| -> Option<String> {
        let path = dir.join(name);
        if !path.is_file() {
            return None;
        }
        let conn = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()?;
        read_version(&conn).ok().flatten()
    };
    (read(crate::INDEX_FILE), read(crate::AUDIT_FILE))
}

/// Bring `conn` from its recorded version to `target` with `steps` (see the module rules).
pub fn migrate(conn: &mut Connection, kind: StoreKind, db_path: &Path, target: u32, steps: &[Migration]) -> Result<Outcome> {
    let Some(found) = read_version(conn)? else { return Ok(Outcome::Fresh) };
    let dir = db_path.parent().map(Path::to_path_buf).unwrap_or_default();
    let found_n: u32 = match found.parse() {
        Ok(n) => n,
        Err(_) => {
            log(&dir, kind, &found, target, "refused", Some("schema version is not a number"));
            return Err(kind.error(format!("schema version {found:?} is not a number; the store was left unchanged")));
        }
    };
    if found_n == target {
        return Ok(Outcome::UpToDate);
    }
    if found_n > target {
        log(&dir, kind, &found, target, "refused", Some("store is newer than this program"));
        return Err(kind.error(format!(
            "schema version {found_n} is newer than this program supports ({target}); use the newer CleanManager — the store was left unchanged"
        )));
    }
    let mut chain = Vec::new();
    let mut at = found_n;
    while at < target {
        match steps.iter().find(|m| m.from == at && m.to == at + 1) {
            Some(m) => chain.push(m),
            None => {
                log(&dir, kind, &found, target, "refused", Some(&format!("no migration step from version {at}")));
                return Err(kind.error(format!("schema version {found_n} cannot be migrated to {target} (no step from {at}); the store was left unchanged")));
            }
        }
        at += 1;
    }
    let backup = if kind == StoreKind::Audit { Some(backup(conn, db_path, found_n).map_err(|e| kind.error(format!("cannot copy the store before migrating: {e}; nothing was changed")))?) } else { None };
    log(&dir, kind, &found, target, "started", backup.as_ref().map(|b| b.display().to_string()).as_deref());
    match run_chain(conn, kind, &chain, target) {
        Ok(()) => {
            log(&dir, kind, &found, target, "completed", None);
            Ok(Outcome::Migrated { from: found_n, to: target, backup })
        }
        Err(why) => {
            log(&dir, kind, &found, target, "failed", Some(&why));
            let kept = backup.map(|b| format!("; a copy taken before the attempt is at {}", b.display())).unwrap_or_default();
            Err(kind.error(format!("migration from version {found_n} to {target} failed and was rolled back ({why}); the store is still at version {found_n}{kept}; see {LOG_FILE}")))
        }
    }
}

fn run_chain(conn: &mut Connection, kind: StoreKind, chain: &[&Migration], target: u32) -> std::result::Result<(), String> {
    // IMMEDIATE takes the write lock up front; dropping `tx` without `commit` rolls everything back.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| format!("cannot start the migration transaction: {e}"))?;
    let before = fingerprint(&tx, kind).map_err(|e| format!("cannot read the protected records: {e}"))?;
    for m in chain {
        (m.apply)(&tx).map_err(|e| format!("step {}→{} ({}): {e}", m.from, m.to, m.description))?;
        if tx.is_autocommit() {
            return Err(format!("step {}→{} ended the migration transaction itself", m.from, m.to));
        }
    }
    tx.execute("UPDATE meta SET value=?1 WHERE key='schema_version'", [target.to_string()]).map_err(|e| format!("version update: {e}"))?;
    let after = fingerprint(&tx, kind).map_err(|e| format!("cannot re-read the protected records: {e}"))?;
    if before != after {
        return Err("the migration would change operations, journal results, quarantine records, backups, plans or receipts; refused so no old action can run again".into());
    }
    let check: String = tx.query_row("PRAGMA quick_check", [], |r| r.get(0)).map_err(|e| format!("integrity check: {e}"))?;
    if check != "ok" {
        return Err(format!("integrity check after migration: {check}"));
    }
    tx.commit().map_err(|e| format!("commit: {e}"))
}

/// Copy the database with `VACUUM INTO` (a consistent snapshot, also of WAL contents).
fn backup(conn: &Connection, db_path: &Path, from: u32) -> std::result::Result<PathBuf, String> {
    let name = db_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "store.db".into());
    let stamp = cm_core::time::now_utc().replace([':', '-'], "");
    let dir = db_path.parent().unwrap_or(Path::new("."));
    let mut path = dir.join(format!("{name}.pre-v{from}-{stamp}.bak"));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{name}.pre-v{from}-{stamp}-{n}.bak"));
        n += 1;
    }
    conn.execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()]).map_err(|e| e.to_string())?;
    Ok(path)
}

/// SHA-256 over the rows that decide what may run (again) and what can be recovered.
fn fingerprint(conn: &Connection, kind: StoreKind) -> rusqlite::Result<String> {
    let queries: &[&str] = match kind {
        StoreKind::Index => &["SELECT scan_id, state, revision FROM scans ORDER BY scan_id"],
        StoreKind::Audit => &[
            "SELECT operation_id, plan_id, plan_digest, dedupe_key, state, finished_at FROM operations ORDER BY operation_id",
            "SELECT operation_id, action_id, intent_json, result_status, result_json FROM action_journal ORDER BY operation_id, action_id",
            "SELECT quarantine_id, operation_id, action_id, record_json, state FROM quarantine_items ORDER BY quarantine_id",
            "SELECT backup_id, operation_id, record_json, record_sha256, state FROM registry_backups ORDER BY backup_id",
            "SELECT plan_id, plan_digest, plan_json FROM plans ORDER BY plan_id",
            "SELECT receipt_id, plan_id, plan_digest, receipt_json, revoked_at FROM receipts ORDER BY receipt_id",
            "SELECT operation_id, report_json FROM operation_reports ORDER BY operation_id",
        ],
    };
    let mut h = Sha256::new();
    for q in queries {
        h.update(q.as_bytes());
        let mut stmt = conn.prepare(q)?;
        let cols = stmt.column_count();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            for i in 0..cols {
                let v: rusqlite::types::Value = row.get(i)?;
                let text = match v {
                    rusqlite::types::Value::Null => "\u{0}N".to_owned(),
                    rusqlite::types::Value::Integer(n) => format!("\u{0}I{n}"),
                    rusqlite::types::Value::Real(r) => format!("\u{0}R{r}"),
                    rusqlite::types::Value::Text(t) => format!("\u{0}T{}:{t}", t.len()),
                    rusqlite::types::Value::Blob(b) => format!("\u{0}B{}:{}", b.len(), b.iter().map(|x| format!("{x:02x}")).collect::<String>()),
                };
                h.update(text.as_bytes());
            }
            h.update(b"\x1e");
        }
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Append one JSON line to `migrations.log` and flush it. Logging problems never change the
/// outcome of a migration (the result is still returned), but they are not silent either: the
/// line goes to stderr when the file cannot be written.
fn log(dir: &Path, kind: StoreKind, from: &str, to: u32, event: &str, detail: Option<&str>) {
    let line = serde_json::json!({
        "at": cm_core::time::now_utc(),
        "store": kind.as_str(),
        "from_version": from,
        "to_version": to.to_string(),
        "event": event,
        "detail": detail,
        "engine_version": cm_core::ENGINE_VERSION,
    })
    .to_string();
    let written = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(LOG_FILE)).and_then(|mut f| {
        f.write_all(line.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()
    });
    if written.is_err() {
        eprintln!("{line}");
    }
}

#[cfg(test)]
mod tests;
