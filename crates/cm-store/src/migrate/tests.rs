//! Migration rules with synthetic steps on real audit/index databases in scratch directories.

use rusqlite::{Connection, Transaction};

use super::*;
use crate::audit::AuditStore;
use crate::index::IndexStore;
use crate::testutil::scratch;

fn host() -> cm_core::host::HostRef {
    cm_core::host::HostRef { kind: "cli".into(), pid: 1, started_at: "2026-10-15T00:00:00Z".into() }
}

/// A v1 audit store with an operation awaiting reconciliation, a journal row without a result
/// and a quarantine record: exactly what must never be replayed or lost by a migration.
fn audit_v1(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = scratch(name);
    let path = dir.join(crate::AUDIT_FILE);
    let a = AuditStore::open(&path).unwrap();
    a.begin_operation("op_pending", "p1", "d1", "k1", &host(), "2026-10-15T00:00:00Z").unwrap();
    a.record_intent("op_pending", "a01", "{\"intent\":1}", "2026-10-15T00:00:01Z").unwrap();
    a.set_operation_state("op_pending", "reconciliation_required").unwrap();
    a.save_quarantine("q1", "op_pending", "a01", "p1", "{\"logical_bytes\":\"10\"}", "2026-10-15T00:00:02Z", "2026-11-15T00:00:02Z").unwrap();
    drop(a);
    (dir, path)
}

fn snapshot(path: &std::path::Path) -> (String, String, Option<String>, String) {
    let c = Connection::open(path).unwrap();
    let state: String = c.query_row("SELECT state FROM operations WHERE operation_id='op_pending'", [], |r| r.get(0)).unwrap();
    let q: String = c.query_row("SELECT state FROM quarantine_items WHERE quarantine_id='q1'", [], |r| r.get(0)).unwrap();
    let result: Option<String> = c.query_row("SELECT result_status FROM action_journal WHERE action_id='a01'", [], |r| r.get(0)).unwrap();
    let v: String = c.query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| r.get(0)).unwrap();
    (state, q, result, v)
}

fn log_events(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(dir.join(LOG_FILE))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["event"].as_str().unwrap().to_owned())
        .collect()
}

fn add_column(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch("ALTER TABLE operations ADD COLUMN migrated_note TEXT; CREATE TABLE IF NOT EXISTS v2_extra (k TEXT PRIMARY KEY) STRICT;")
}

fn half_then_fail(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch("CREATE TABLE half_done (k TEXT) STRICT; INSERT INTO half_done VALUES ('x');")?;
    tx.execute_batch("THIS IS NOT SQL")
}

fn replays(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    // Turning a reconciliation-required operation back into `created` would let it run again.
    tx.execute("UPDATE operations SET state='created' WHERE state='reconciliation_required'", []).map(|_| ())
}

fn drops_journal(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM action_journal", []).map(|_| ())
}

const GOOD: &[Migration] = &[Migration { from: 1, to: 2, description: "add a column", apply: add_column }];

#[test]
fn successful_migration_is_versioned_logged_backed_up_and_keeps_history() {
    let (dir, path) = audit_v1("mig-ok");
    let mut conn = Connection::open(&path).unwrap();
    let out = migrate(&mut conn, StoreKind::Audit, &path, 2, GOOD).unwrap();
    let Outcome::Migrated { from: 1, to: 2, backup: Some(backup) } = out else { panic!("{out:?}") };
    drop(conn);
    assert_eq!(snapshot(&path), ("reconciliation_required".into(), "recoverable".into(), None, "2".into()));
    // The pre-migration copy is a complete v1 store.
    assert_eq!(snapshot(&backup).3, "1");
    assert_eq!(log_events(&dir), ["started", "completed"]);
    // Running again is a no-op.
    let mut conn = Connection::open(&path).unwrap();
    assert_eq!(migrate(&mut conn, StoreKind::Audit, &path, 2, GOOD).unwrap(), Outcome::UpToDate);
}

#[test]
fn a_failing_step_rolls_everything_back_and_keeps_the_log() {
    let (dir, path) = audit_v1("mig-fail");
    let steps = [
        Migration { from: 1, to: 2, description: "add a column", apply: add_column },
        Migration { from: 2, to: 3, description: "breaks halfway", apply: half_then_fail },
    ];
    let mut conn = Connection::open(&path).unwrap();
    let err = migrate(&mut conn, StoreKind::Audit, &path, 3, &steps).unwrap_err();
    let msg = err.to_string();
    assert!(matches!(err, StoreError::AuditUnavailable(_)) && msg.contains("rolled back") && msg.contains("still at version 1"), "{msg}");
    // Nothing of either step survived; the store is the old one and the old program can open it.
    let tables: i64 = conn.query_row("SELECT count(*) FROM sqlite_master WHERE name IN ('half_done','v2_extra')", [], |r| r.get(0)).unwrap();
    assert_eq!(tables, 0);
    let cols: i64 = conn.query_row("SELECT count(*) FROM pragma_table_info('operations') WHERE name='migrated_note'", [], |r| r.get(0)).unwrap();
    assert_eq!(cols, 0);
    drop(conn);
    assert_eq!(snapshot(&path), ("reconciliation_required".into(), "recoverable".into(), None, "1".into()));
    AuditStore::open(&path).expect("the old store is intact and opens normally");
    assert_eq!(log_events(&dir), ["started", "failed"]);
}

#[test]
fn migrations_that_would_replay_or_drop_actions_are_refused() {
    for (name, step) in [("mig-replay", replays as fn(&Transaction<'_>) -> rusqlite::Result<()>), ("mig-drop", drops_journal)] {
        let (_dir, path) = audit_v1(name);
        let steps = [Migration { from: 1, to: 2, description: "unsafe", apply: step }];
        let mut conn = Connection::open(&path).unwrap();
        let msg = migrate(&mut conn, StoreKind::Audit, &path, 2, &steps).unwrap_err().to_string();
        assert!(msg.contains("no old action can run again"), "{name}: {msg}");
        drop(conn);
        assert_eq!(snapshot(&path), ("reconciliation_required".into(), "recoverable".into(), None, "1".into()), "{name}");
    }
}

#[test]
fn newer_unknown_or_gapped_versions_are_refused_without_changes() {
    let (dir, path) = audit_v1("mig-refuse");
    let conn = Connection::open(&path).unwrap();
    for (version, target, needle) in [("7", 2u32, "newer than this program"), ("abc", 2, "not a number"), ("1", 3, "no step from 2")] {
        conn.execute("UPDATE meta SET value=?1 WHERE key='schema_version'", [version]).unwrap();
        let mut c2 = Connection::open(&path).unwrap();
        let msg = migrate(&mut c2, StoreKind::Audit, &path, target, GOOD).unwrap_err().to_string();
        assert!(msg.contains(needle) && msg.contains("left unchanged"), "{version}: {msg}");
        let v: String = c2.query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| r.get(0)).unwrap();
        assert_eq!(v, version);
    }
    assert_eq!(log_events(&dir), ["refused", "refused", "refused"]);
    // No copy is taken when nothing is attempted.
    assert!(!std::fs::read_dir(&dir).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().ends_with(".bak")));
}

#[test]
fn open_refuses_an_unmigratable_audit_store_and_keeps_its_records() {
    let (_dir, path) = audit_v1("mig-open-audit");
    Connection::open(&path).unwrap().execute("UPDATE meta SET value='0' WHERE key='schema_version'", []).unwrap();
    let err = AuditStore::open(&path).err().expect("refused");
    assert!(matches!(err, StoreError::AuditUnavailable(_)), "{err}");
    let c = Connection::open(&path).unwrap();
    let n: i64 = c.query_row("SELECT count(*) FROM operations", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}

#[test]
fn an_unmigratable_index_is_reported_as_rebuildable_and_never_touches_the_audit_store() {
    let (dir, audit) = audit_v1("mig-open-index");
    let index = dir.join(crate::INDEX_FILE);
    drop(IndexStore::open(&index).unwrap());
    Connection::open(&index).unwrap().execute("UPDATE meta SET value='0' WHERE key='schema_version'", []).unwrap();
    assert!(matches!(IndexStore::open(&index), Err(StoreError::IndexIncompatible { .. })));
    let (_, kept) = crate::Stores::rebuild_index(&dir).unwrap();
    assert!(!kept.is_empty() && kept.iter().all(|k| k.exists()), "the incompatible index is kept, never deleted: {kept:?}");
    assert_eq!(log_events(&dir).last().map(String::as_str), Some("rebuilt"), "the rebuild is logged with the kept names");
    assert_eq!(snapshot(&audit), ("reconciliation_required".into(), "recoverable".into(), None, "1".into()));
    assert_eq!(store_versions(&dir), (Some(crate::index::INDEX_SCHEMA_VERSION.into()), Some("1".into())));
}

#[test]
fn index_v1_gains_the_shared_blocks_column_and_keeps_its_rows() {
    let dir = scratch("mig-index-v1-v2");
    let index = dir.join(crate::INDEX_FILE);
    drop(IndexStore::open(&index).unwrap());
    {
        // Recreate the released v1 shape: no shared_blocks column, one object row, version 1.
        let c = Connection::open(&index).unwrap();
        c.execute_batch(
            "ALTER TABLE objects DROP COLUMN shared_blocks;
             INSERT INTO objects(scan_id,revision,object_id,volume_id,file_id,generation,quality,kind,logical_be,allocated_be,link_count,cloud,modified_unix)
             VALUES('s1',1,1,'v','inode64:01',NULL,'strong','file',NULL,NULL,1,'not_cloud',NULL);
             UPDATE meta SET value='1' WHERE key='schema_version';",
        )
        .unwrap();
    }
    drop(IndexStore::open(&index).unwrap());
    assert_eq!(store_versions(&dir).0.as_deref(), Some("2"));
    assert_eq!(log_events(&dir), ["started", "completed"]);
    let c = Connection::open(&index).unwrap();
    let kept: (String, Option<i64>) = c.query_row("SELECT file_id, shared_blocks FROM objects WHERE object_id=1", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(kept, ("inode64:01".into(), None), "v1 rows keep NULL = not determined");
    c.execute("UPDATE objects SET shared_blocks=1 WHERE object_id=1", []).unwrap();
}

#[test]
fn current_registries_form_complete_chains() {
    for kind in [StoreKind::Index, StoreKind::Audit] {
        let mut at = 1;
        for m in kind.registry() {
            assert_eq!((m.from, m.to), (at, at + 1), "{} steps must be consecutive from version 1", kind.as_str());
            at += 1;
        }
        assert_eq!(at, kind.current(), "{} steps must end at the current schema version", kind.as_str());
    }
}
