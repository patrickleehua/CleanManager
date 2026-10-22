//! When the scan index runs SQLite's `PRAGMA quick_check` (PERF-WFA-01).
//!
//! A full check reads every page: at 1.5 M rows it costs about a second or more on every open, which
//! made every index command slow. The index therefore checks itself only when there is a reason:
//!
//! - it was never checked (a store written by an older program) or its last passed check is older
//!   than [`RECHECK_DAYS`] (`meta.integrity_checked_unix`, written after every passed check);
//! - an unclean-shutdown marker is set (`meta.integrity_due`): a scan whose host process exited
//!   before publishing, or a SQLite operation that reported corruption at run time;
//! - a schema migration is pending (checked before it runs; the migration checks again inside its transaction);
//! - `doctor` asks for it ([`Check::Now`]), and `doctor --repair-index` before deciding anything.
//!
//! Detection stays honest between checks: the open itself fails on a damaged header, and any SQLite
//! operation that reports `SQLITE_CORRUPT`/`SQLITE_NOTADB` surfaces as `IndexCorrupt` (typed
//! `index_unavailable`) and sets the marker. The audit store keeps its check on every open: it is
//! small, and its damage must block writes before they happen.

use rusqlite::{Connection, OptionalExtension};

/// Days between scheduled checks of an otherwise healthy index.
pub const RECHECK_DAYS: i64 = 7;
const CHECKED_AT: &str = "integrity_checked_unix";
const DUE: &str = "integrity_due";

/// How an open treats the integrity check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// Only when due (see the module rules).
    Scheduled,
    /// Always (`doctor`, repair).
    Now,
}

pub(crate) fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn has_meta(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='meta'", [], |r| r.get::<_, i64>(0)).map(|n| n > 0)
}

fn meta(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0)).optional()
}

/// Why a check is due now, or `None`. A database without a schema yet is never due (there is nothing to check).
pub(crate) fn due(conn: &Connection, check: Check, now: i64) -> rusqlite::Result<Option<String>> {
    if !has_meta(conn)? {
        return Ok(None);
    }
    if check == Check::Now {
        return Ok(Some("requested".into()));
    }
    if let Some(reason) = meta(conn, DUE)? {
        return Ok(Some(format!("marked: {reason}")));
    }
    match meta(conn, CHECKED_AT)?.and_then(|v| v.parse::<i64>().ok()) {
        None => Ok(Some("never checked".into())),
        Some(at) if now.saturating_sub(at) >= RECHECK_DAYS * 86_400 || at > now + 86_400 => Ok(Some("interval".into())),
        Some(_) => Ok(None),
    }
}

/// `PRAGMA quick_check`: `Ok(None)` when it passed, `Ok(Some(problem))` when it did not.
pub(crate) fn quick_check(conn: &Connection) -> rusqlite::Result<Option<String>> {
    let mut stmt = conn.prepare("PRAGMA quick_check")?;
    let lines: Vec<String> = stmt.query_map([], |r| r.get(0))?.take(5).collect::<rusqlite::Result<_>>()?;
    Ok(if lines.len() == 1 && lines[0] == "ok" { None } else { Some(lines.join("; ")) })
}

/// Record a passed check and clear the marker. Best effort: a store that cannot be written (read-only
/// media, a long write lock held by a publishing scan) is simply checked again next time, so this never
/// waits long. The marker is cleared only after the timestamp was written; a half-done record only
/// means one more check.
pub(crate) fn record_ok(conn: &Connection, now: i64) {
    let _ = conn.busy_timeout(std::time::Duration::from_millis(250));
    let _ = conn
        .execute("INSERT OR REPLACE INTO meta(key,value) VALUES(?1,?2)", rusqlite::params![CHECKED_AT, now.to_string()])
        .and_then(|_| conn.execute("DELETE FROM meta WHERE key=?1", [DUE]));
    // rusqlite's default for every connection.
    let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
}

/// Set the unclean-shutdown marker so the next open checks the whole database. Best effort, like
/// [`record_ok`]: on a database too damaged to take the write, the next open fails on its own.
pub(crate) fn mark_due(conn: &Connection, reason: &str) {
    let _ = conn.busy_timeout(std::time::Duration::from_millis(250));
    let _ = conn.execute(MARK_DUE_SQL, [reason]);
    let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
}

/// The marker statement, also used inside a caller's transaction.
pub(crate) const MARK_DUE_SQL: &str = "INSERT OR REPLACE INTO meta(key,value) VALUES('integrity_due', ?1)";

/// Last passed check (unix seconds) and the pending marker, for `doctor`.
pub(crate) fn status(conn: &Connection) -> rusqlite::Result<(Option<i64>, Option<String>)> {
    if !has_meta(conn)? {
        return Ok((None, None));
    }
    Ok((meta(conn, CHECKED_AT)?.and_then(|v| v.parse().ok()), meta(conn, DUE)?))
}
