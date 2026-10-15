//! Versioned scan index (IDX-01..IDX-05).
//!
//! A scan writes batches into a *staging* revision; queries only see *published* revisions.
//! Sizes are stored as 8-byte big-endian BLOBs so SQLite's memcmp ordering equals exact u64
//! ordering (no signed 64-bit truncation, no text sorting). Unknown sizes stay NULL.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use cm_core::bytes::{Bytes, KnownTotal};
use cm_core::capability::Platform;
use cm_core::model::{BoundaryKind, IdentityQuality, ObjectKind, ScanState};
use cm_core::path::RawPath;
pub use cm_core::scan::{EntryRow, ObjectRow, SkippedRow};
use cm_core::scan::ScanSink;

use crate::audit::{HostRef, Liveness};
use crate::{Result, StoreError};

pub const INDEX_SCHEMA_VERSION: &str = "1";
pub const MAX_PAGE: u32 = 500;
pub const DEFAULT_PAGE: u32 = 50;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS scans (
    scan_id TEXT PRIMARY KEY,
    scope_id TEXT NOT NULL,
    platform TEXT NOT NULL CHECK (platform IN ('windows','macos')),
    root_raw BLOB NOT NULL,
    backend TEXT NOT NULL,
    state TEXT NOT NULL,
    revision INTEGER NOT NULL,
    completeness TEXT,
    host_kind TEXT NOT NULL,
    host_pid INTEGER NOT NULL,
    host_started_at TEXT NOT NULL,
    heartbeat_at TEXT NOT NULL,
    cancel_requested INTEGER NOT NULL DEFAULT 0,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    report_json TEXT
) STRICT;
CREATE TABLE IF NOT EXISTS revisions (
    scan_id TEXT NOT NULL REFERENCES scans(scan_id),
    revision INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('staging','published')),
    PRIMARY KEY (scan_id, revision)
) STRICT;
CREATE TABLE IF NOT EXISTS objects (
    scan_id TEXT NOT NULL, revision INTEGER NOT NULL, object_id INTEGER NOT NULL,
    volume_id TEXT NOT NULL, file_id TEXT NOT NULL, generation TEXT, quality TEXT NOT NULL,
    kind TEXT NOT NULL, logical_be BLOB, allocated_be BLOB, link_count INTEGER, cloud TEXT NOT NULL, modified_unix INTEGER,
    PRIMARY KEY (scan_id, revision, object_id)
) STRICT;
CREATE TABLE IF NOT EXISTS entries (
    scan_id TEXT NOT NULL, revision INTEGER NOT NULL, entry_id INTEGER NOT NULL,
    parent_entry_id INTEGER, name_raw BLOB NOT NULL, name_display TEXT NOT NULL,
    object_id INTEGER NOT NULL, kind TEXT NOT NULL, boundary TEXT,
    sort_logical_be BLOB, sort_allocated_be BLOB, unknown_descendants INTEGER,
    PRIMARY KEY (scan_id, revision, entry_id)
) STRICT;
CREATE INDEX IF NOT EXISTS entries_parent ON entries(scan_id, revision, parent_entry_id, entry_id);
CREATE INDEX IF NOT EXISTS entries_logical ON entries(scan_id, revision, sort_logical_be, entry_id);
CREATE TABLE IF NOT EXISTS candidate_sets (
    candidate_set_id TEXT PRIMARY KEY, scan_id TEXT NOT NULL, revision INTEGER NOT NULL,
    rule_set_digest TEXT NOT NULL, protection_version TEXT NOT NULL, created_at TEXT NOT NULL, set_json TEXT NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS skipped (
    scan_id TEXT NOT NULL, revision INTEGER NOT NULL, parent_entry_id INTEGER,
    path_raw BLOB NOT NULL, reason TEXT NOT NULL
) STRICT;
";

fn be(v: Option<u64>) -> Option<Vec<u8>> {
    v.map(|n| n.to_be_bytes().to_vec())
}

fn from_be(b: Option<Vec<u8>>) -> Result<Option<u64>> {
    b.map(|bytes| {
        let arr: [u8; 8] = bytes.try_into().map_err(|_| StoreError::IndexCorrupt("size column is not 8 bytes".into()))?;
        Ok(u64::from_be_bytes(arr))
    })
    .transpose()
}

fn enum_text<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|j| j.as_str().map(str::to_owned)).expect("unit enum")
}

fn enum_parse<T: for<'de> Deserialize<'de>>(s: &str) -> Result<T> {
    serde_json::from_value(serde_json::Value::from(s)).map_err(|_| StoreError::IndexCorrupt(format!("unknown enum value {s}")))
}

fn raw_of(p: &RawPath) -> Vec<u8> {
    p.raw_bytes()
}

fn path_from_raw(platform: Platform, raw: Vec<u8>) -> Result<RawPath> {
    let r = match platform {
        Platform::Windows => {
            if raw.len() % 2 != 0 {
                return Err(StoreError::IndexCorrupt("odd UTF-16 name".into()));
            }
            RawPath::windows(raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect())
        }
        Platform::Macos => RawPath::unix(raw),
    };
    r.map_err(|e| StoreError::IndexCorrupt(e.to_string()))
}

/// Join a child name onto a parent path (see `RawPath::join`).
pub fn join(parent: &RawPath, name: &RawPath) -> Result<RawPath> {
    parent.join(name).map_err(|_| StoreError::InvalidArgument("cannot join paths from different platforms".into()))
}

#[derive(Debug, Clone)]
pub struct ScanInit {
    pub scan_id: String,
    pub scope_id: String,
    pub root: RawPath,
    pub backend: String,
    pub host: HostRef,
    pub started_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScanStatusView {
    pub scan_id: String,
    pub scope_id: String,
    pub state: ScanState,
    pub revision: u32,
    pub published: bool,
    pub completeness: Option<String>,
    pub cancel_requested: bool,
    pub host: HostRef,
    pub heartbeat_at: String,
    pub entries_seen: u64,
    pub skipped_ranges: u64,
    pub started_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelReceipt {
    pub scan_id: String,
    pub accepted: bool,
    /// `created`/`running`/`stopping`, or `already_terminal`.
    pub state_at_receipt: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    LogicalDesc,
    AllocatedDesc,
    NameAsc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KindFilter {
    Any,
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    pub kind: KindFilter,
    pub name_contains: Option<String>,
    /// Direct children of this entry; `None` searches the whole revision (e.g. largest files).
    pub parent_entry_id: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct EntryQuery {
    pub scan_id: String,
    pub sort: Sort,
    pub filter: Filter,
    pub page_size: u32,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeBasis {
    Entity,
    SubtreeKnown,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntryItem {
    pub entry_id: String,
    pub parent_entry_id: Option<String>,
    pub object_id: String,
    pub path: RawPath,
    pub kind: ObjectKind,
    pub logical_bytes: Option<Bytes>,
    pub allocated_bytes: Option<Bytes>,
    pub size_basis: SizeBasis,
    pub unknown_descendants: Option<u64>,
    pub link_count: Option<u32>,
    pub boundary: Option<BoundaryKind>,
    pub identity_quality: IdentityQuality,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntriesPage {
    pub scan_id: String,
    pub scope_id: String,
    pub revision: u32,
    pub sort: Sort,
    pub filter: Filter,
    pub page_size: u32,
    pub items: Vec<EntryItem>,
    pub next_cursor: Option<String>,
    pub unknown_size_items: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CursorKey {
    v: u8,
    scan_id: String,
    revision: u32,
    binding: String,
    last_key: Option<String>,
    last_entry_id: u64,
}

/// Overlap-free space for a selection (IDX-04): selected entries that are descendants of other
/// selected entries are folded in, and each object is counted once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct SelectionSummary {
    pub logical: KnownTotal,
    pub allocated: KnownTotal,
    pub covered_entries: u64,
    pub overlapping_selected: u64,
    pub unread_ranges: u64,
}

pub struct IndexStore {
    conn: Connection,
}

impl IndexStore {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        let check: String = conn
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .map_err(|e| StoreError::IndexCorrupt(e.to_string()))?;
        if check != "ok" {
            return Err(StoreError::IndexCorrupt(check));
        }
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(SCHEMA).map_err(|e| StoreError::IndexCorrupt(e.to_string()))?;
        let found: Option<String> =
            conn.query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| r.get(0)).optional()?;
        match found {
            None => {
                conn.execute("INSERT INTO meta(key,value) VALUES('store_kind','index'),('schema_version',?1)", [INDEX_SCHEMA_VERSION])?;
            }
            Some(v) if v == INDEX_SCHEMA_VERSION => {}
            Some(v) => return Err(StoreError::IndexIncompatible { found: v, expected: INDEX_SCHEMA_VERSION.into() }),
        }
        Ok(IndexStore { conn })
    }

    /// Start a scan and its staging revision.
    pub fn begin_scan(&self, init: &ScanInit) -> Result<u32> {
        let revision = 1u32;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO scans(scan_id,scope_id,platform,root_raw,backend,state,revision,host_kind,host_pid,host_started_at,heartbeat_at,started_at)
             VALUES(?1,?2,?3,?4,?5,'running',?6,?7,?8,?9,?10,?10)",
            params![
                init.scan_id, init.scope_id, enum_text(&init.root.platform()), raw_of(&init.root), init.backend, revision,
                init.host.kind, init.host.pid, init.host.started_at, init.started_at
            ],
        )?;
        tx.execute("INSERT INTO revisions(scan_id,revision,status) VALUES(?1,?2,'staging')", params![init.scan_id, revision])?;
        tx.commit()?;
        Ok(revision)
    }

    fn require_staging(&self, scan_id: &str, revision: u32) -> Result<()> {
        let status: Option<String> = self
            .conn
            .query_row("SELECT status FROM revisions WHERE scan_id=?1 AND revision=?2", params![scan_id, revision], |r| r.get(0))
            .optional()?;
        match status.as_deref() {
            Some("staging") => Ok(()),
            Some(_) => Err(StoreError::InvalidState("revision already published".into())),
            None => Err(StoreError::NotFound(format!("{scan_id}@{revision}"))),
        }
    }

    /// Commit one batch atomically into the staging revision.
    pub fn write_batch(&self, scan_id: &str, revision: u32, objects: &[ObjectRow], entries: &[EntryRow], skipped: &[SkippedRow]) -> Result<()> {
        self.require_staging(scan_id, revision)?;
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut o = tx.prepare_cached(
                "INSERT INTO objects(scan_id,revision,object_id,volume_id,file_id,generation,quality,kind,logical_be,allocated_be,link_count,cloud,modified_unix)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            )?;
            for x in objects {
                o.execute(params![
                    scan_id, revision, x.object_id as i64, x.volume_id, x.file_id, x.generation, enum_text(&x.quality),
                    enum_text(&x.kind), be(x.logical), be(x.allocated), x.link_count, enum_text(&x.cloud), x.modified_unix
                ])?;
            }
            let mut e = tx.prepare_cached(
                "INSERT INTO entries(scan_id,revision,entry_id,parent_entry_id,name_raw,name_display,object_id,kind,boundary)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            )?;
            for x in entries {
                e.execute(params![
                    scan_id, revision, x.entry_id as i64, x.parent_entry_id.map(|p| p as i64), raw_of(&x.name),
                    x.name.display(), x.object_id as i64, enum_text(&x.kind), x.boundary.as_ref().map(enum_text)
                ])?;
            }
            let mut s = tx.prepare_cached("INSERT INTO skipped(scan_id,revision,parent_entry_id,path_raw,reason) VALUES(?1,?2,?3,?4,?5)")?;
            for x in skipped {
                s.execute(params![scan_id, revision, x.parent_entry_id.map(|p| p as i64), raw_of(&x.path), enum_text(&x.reason)])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn heartbeat(&self, scan_id: &str, now: &str) -> Result<()> {
        self.conn.execute("UPDATE scans SET heartbeat_at=?2 WHERE scan_id=?1", params![scan_id, now])?;
        Ok(())
    }

    /// Record a cancel request. It is a receipt only; the host stops at its next safe boundary.
    pub fn request_cancel(&self, scan_id: &str) -> Result<CancelReceipt> {
        let state: String = self
            .conn
            .query_row("SELECT state FROM scans WHERE scan_id=?1", [scan_id], |r| r.get(0))
            .optional()?
            .ok_or_else(|| StoreError::NotFound(scan_id.into()))?;
        let parsed: ScanState = enum_parse(&state)?;
        if parsed.is_terminal() {
            return Ok(CancelReceipt { scan_id: scan_id.into(), accepted: false, state_at_receipt: "already_terminal".into() });
        }
        self.conn.execute("UPDATE scans SET cancel_requested=1 WHERE scan_id=?1", [scan_id])?;
        Ok(CancelReceipt { scan_id: scan_id.into(), accepted: true, state_at_receipt: state })
    }

    pub fn cancel_requested(&self, scan_id: &str) -> Result<bool> {
        Ok(self.conn.query_row("SELECT cancel_requested FROM scans WHERE scan_id=?1", [scan_id], |r| r.get::<_, i64>(0))? != 0)
    }

    pub fn set_stopping(&self, scan_id: &str) -> Result<()> {
        self.conn.execute("UPDATE scans SET state='stopping' WHERE scan_id=?1 AND state='running'", [scan_id])?;
        Ok(())
    }

    /// Publish the staging revision with a terminal state. `completed` with unread ranges is
    /// downgraded to `partial` (SCN-03). Aggregates are computed here, once per revision.
    pub fn publish(&self, scan_id: &str, revision: u32, requested: ScanState, finished_at: &str) -> Result<ScanState> {
        if !requested.is_terminal() {
            return Err(StoreError::InvalidState("publish needs a terminal state".into()));
        }
        self.require_staging(scan_id, revision)?;
        let skipped: i64 =
            self.conn.query_row("SELECT count(*) FROM skipped WHERE scan_id=?1 AND revision=?2", params![scan_id, revision], |r| r.get(0))?;
        let state = if requested == ScanState::Completed && skipped > 0 { ScanState::Partial } else { requested };
        let completeness = if state == ScanState::Completed { "verified_scope" } else { "partial_scope" };
        let tx = self.conn.unchecked_transaction()?;
        compute_aggregates(&tx, scan_id, revision)?;
        tx.execute("UPDATE revisions SET status='published' WHERE scan_id=?1 AND revision=?2", params![scan_id, revision])?;
        tx.execute(
            "UPDATE scans SET state=?2, completeness=?3, finished_at=?4 WHERE scan_id=?1",
            params![scan_id, enum_text(&state), completeness, finished_at],
        )?;
        tx.commit()?;
        Ok(state)
    }

    /// Mark scans whose host process exited as `interrupted` and publish what was committed.
    pub fn reconcile_hosts(&self, liveness: impl Fn(&HostRef) -> Liveness, now: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT scan_id, revision, host_kind, host_pid, host_started_at FROM scans WHERE state IN ('created','running','stopping')",
        )?;
        let rows: Vec<(String, u32, HostRef)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, HostRef { kind: r.get(2)?, pid: r.get(3)?, started_at: r.get(4)? })))?
            .collect::<std::result::Result<_, _>>()?;
        drop(stmt);
        let mut interrupted = Vec::new();
        for (scan_id, revision, host) in rows {
            if liveness(&host) == Liveness::Exited {
                self.publish(&scan_id, revision, ScanState::Interrupted, now)?;
                interrupted.push(scan_id);
            }
        }
        Ok(interrupted)
    }

    pub fn scan_status(&self, scan_id: &str) -> Result<ScanStatusView> {
        let row = self
            .conn
            .query_row(
                "SELECT s.scope_id, s.state, s.revision, r.status, s.completeness, s.cancel_requested, s.host_kind, s.host_pid,
                        s.host_started_at, s.heartbeat_at,
                        (SELECT count(*) FROM entries e WHERE e.scan_id=s.scan_id AND e.revision=s.revision),
                        (SELECT count(*) FROM skipped k WHERE k.scan_id=s.scan_id AND k.revision=s.revision),
                        s.started_at, s.finished_at
                 FROM scans s JOIN revisions r ON r.scan_id=s.scan_id AND r.revision=s.revision WHERE s.scan_id=?1",
                [scan_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, u32>(2)?, r.get::<_, String>(3)?,
                        r.get::<_, Option<String>>(4)?, r.get::<_, i64>(5)?, r.get::<_, String>(6)?, r.get::<_, u32>(7)?,
                        r.get::<_, String>(8)?, r.get::<_, String>(9)?, r.get::<_, i64>(10)?, r.get::<_, i64>(11)?,
                        r.get::<_, String>(12)?, r.get::<_, Option<String>>(13)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound(scan_id.into()))?;
        Ok(ScanStatusView {
            scan_id: scan_id.into(),
            scope_id: row.0,
            state: enum_parse(&row.1)?,
            revision: row.2,
            published: row.3 == "published",
            completeness: row.4,
            cancel_requested: row.5 != 0,
            host: HostRef { kind: row.6, pid: row.7, started_at: row.8 },
            heartbeat_at: row.9,
            entries_seen: row.10 as u64,
            skipped_ranges: row.11 as u64,
            started_at: row.12,
            finished_at: row.13,
        })
    }

    /// Persist the backend's report (backend reason, issues, elevated-read label) so it can be
    /// queried after the scan, not only by the process that ran it (SCN-02/03).
    pub fn record_report(&self, scan_id: &str, report: &cm_core::scan::ScanReport) -> Result<()> {
        let json = serde_json::to_string(report).expect("report serializes");
        let n = self.conn.execute("UPDATE scans SET report_json=?2, backend=?3 WHERE scan_id=?1", params![scan_id, json, report.backend])?;
        if n == 0 {
            return Err(StoreError::NotFound(scan_id.into()));
        }
        Ok(())
    }

    pub fn scan_report(&self, scan_id: &str) -> Result<Option<serde_json::Value>> {
        let text: Option<Option<String>> =
            self.conn.query_row("SELECT report_json FROM scans WHERE scan_id=?1", [scan_id], |r| r.get(0)).optional()?;
        match text {
            None => Err(StoreError::NotFound(scan_id.into())),
            Some(None) => Ok(None),
            Some(Some(s)) => Ok(Some(serde_json::from_str(&s).map_err(|e| StoreError::IndexCorrupt(e.to_string()))?)),
        }
    }

    /// Load a published revision as an analysis view (identity index included).
    pub fn view(&self, scan_id: &str) -> Result<cm_core::analysis::MemView> {
        use cm_core::analysis::{EntryFacts, MemView};
        let (revision, _, platform) = self.published(scan_id)?;
        let mut view = MemView { platform: Some(platform), ..Default::default() };
        let mut skipped: HashMap<u64, u64> = HashMap::new();
        {
            let mut stmt = self.conn.prepare("SELECT parent_entry_id FROM skipped WHERE scan_id=?1 AND revision=?2 AND parent_entry_id IS NOT NULL")?;
            let mut rows = stmt.query(params![scan_id, revision])?;
            while let Some(r) = rows.next()? {
                *skipped.entry(r.get::<_, i64>(0)? as u64).or_default() += 1;
            }
        }
        let mut stmt = self.conn.prepare(
            "SELECT e.entry_id, e.parent_entry_id, e.name_raw, e.kind, e.boundary, e.object_id, o.logical_be, o.quality, o.cloud,
                    o.modified_unix, o.volume_id, o.file_id, o.link_count, o.allocated_be
             FROM entries e JOIN objects o ON o.scan_id=e.scan_id AND o.revision=e.revision AND o.object_id=e.object_id
             WHERE e.scan_id=?1 AND e.revision=?2 ORDER BY e.entry_id",
        )?;
        let mut rows = stmt.query(params![scan_id, revision])?;
        while let Some(r) = rows.next()? {
            let id = r.get::<_, i64>(0)? as u64;
            let parent = r.get::<_, Option<i64>>(1)?.map(|p| p as u64);
            if parent.is_none() {
                view.root = id;
            }
            view.names.insert(id, path_from_raw(platform, r.get(2)?)?);
            view.parents.insert(id, parent);
            if let Some(p) = parent {
                view.kids.entry(p).or_default().push(id);
            }
            let kind: ObjectKind = enum_parse(&r.get::<_, String>(3)?)?;
            view.facts.insert(id, EntryFacts {
                kind,
                boundary: r.get::<_, Option<String>>(4)?.is_some(),
                object_id: r.get::<_, i64>(5)? as u64,
                logical: if kind == ObjectKind::Directory { None } else { from_be(r.get(6)?)? },
                allocated: if kind == ObjectKind::Directory { None } else { from_be(r.get(13)?)? },
                volume_id: r.get(10)?,
                file_id: r.get(11)?,
                quality: enum_parse(&r.get::<_, String>(7)?)?,
                cloud: enum_parse(&r.get::<_, String>(8)?)?,
                modified_unix: r.get(9)?,
                link_count: r.get(12)?,
                skipped_here: skipped.get(&id).copied().unwrap_or(0),
            });
            view.identities.entry((r.get(10)?, r.get(11)?)).or_insert(id);
        }
        Ok(view)
    }

    /// Persist an analysis result (derived data: rebuilt with the index).
    pub fn save_candidates(&self, set: &cm_core::analysis::CandidateSet) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO candidate_sets(candidate_set_id,scan_id,revision,rule_set_digest,protection_version,created_at,set_json)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![set.candidate_set_id, set.scan_id, set.revision, set.rule_set_digest, set.protection_version,
                cm_core::time::now_utc(), serde_json::to_string(set).expect("serializable")],
        )?;
        Ok(())
    }

    pub fn candidate_set(&self, candidate_set_id: &str) -> Result<serde_json::Value> {
        let text: String = self
            .conn
            .query_row("SELECT set_json FROM candidate_sets WHERE candidate_set_id=?1", [candidate_set_id], |r| r.get(0))
            .optional()?
            .ok_or_else(|| StoreError::NotFound(candidate_set_id.into()))?;
        serde_json::from_str(&text).map_err(|e| StoreError::IndexCorrupt(e.to_string()))
    }

    /// One page of candidates ordered by candidate_id; the cursor binds the set and page size.
    pub fn list_candidates(&self, candidate_set_id: &str, page_size: u32, cursor: Option<&str>) -> Result<(Vec<serde_json::Value>, Option<String>)> {
        if page_size == 0 || page_size > MAX_PAGE {
            return Err(StoreError::InvalidArgument(format!("page_size must be 1..={MAX_PAGE}")));
        }
        let set = match self.candidate_set(candidate_set_id) {
            Ok(s) => s,
            Err(StoreError::NotFound(_)) if cursor.is_some() => return Err(StoreError::CursorStale("candidate set no longer available".into())),
            Err(e) => return Err(e),
        };
        let mut all: Vec<serde_json::Value> = set["candidates"].as_array().cloned().unwrap_or_default();
        all.sort_by(|a, b| a["candidate_id"].as_str().cmp(&b["candidate_id"].as_str()));
        let start = match cursor {
            None => 0,
            Some(c) => {
                let (set_id, size, last) = c.split_once('|').and_then(|(a, rest)| rest.split_once('|').map(|(b, l)| (a, b, l)))
                    .ok_or_else(|| StoreError::CursorStale("malformed cursor".into()))?;
                if set_id != candidate_set_id || size != page_size.to_string() {
                    return Err(StoreError::CursorStale("cursor belongs to another set or page size".into()));
                }
                all.iter().position(|v| v["candidate_id"].as_str().is_some_and(|id| id > last)).unwrap_or(all.len())
            }
        };
        let end = (start + page_size as usize).min(all.len());
        let page = all[start..end].to_vec();
        let next = (end < all.len()).then(|| format!("{candidate_set_id}|{page_size}|{}", page.last().unwrap()["candidate_id"].as_str().unwrap()));
        Ok((page, next))
    }

    /// Remove an old revision (retention). Cursors bound to it become stale.
    pub fn drop_scan(&self, scan_id: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for table in ["entries", "objects", "skipped", "revisions", "scans"] {
            tx.execute(&format!("DELETE FROM {table} WHERE scan_id=?1"), [scan_id])?;
        }
        tx.commit()?;
        Ok(())
    }

    fn published(&self, scan_id: &str) -> Result<(u32, String, Platform)> {
        let row: Option<(u32, String, String, String)> = self
            .conn
            .query_row(
                "SELECT s.revision, r.status, s.scope_id, s.platform FROM scans s JOIN revisions r ON r.scan_id=s.scan_id AND r.revision=s.revision WHERE s.scan_id=?1",
                [scan_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        match row {
            Some((rev, status, scope, platform)) if status == "published" => Ok((rev, scope, enum_parse(&platform)?)),
            Some(_) => Err(StoreError::InvalidState("scan revision is still staging; query scan status instead".into())),
            None => Err(StoreError::NotFound(scan_id.into())),
        }
    }

    fn binding(scan_id: &str, revision: u32, sort: Sort, filter: &Filter, page_size: u32) -> String {
        let text = serde_json::to_string(&(scan_id, revision, sort, filter, page_size)).expect("serializable");
        Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// One revision-bound page (IDX-01/02/03). The order is total: size (exact u64, unknown
    /// last) or raw name, then entry_id.
    pub fn list_entries(&self, q: &EntryQuery) -> Result<EntriesPage> {
        if q.page_size == 0 || q.page_size > MAX_PAGE {
            return Err(StoreError::InvalidArgument(format!("page_size must be 1..={MAX_PAGE}")));
        }
        let cursor_key = q
            .cursor
            .as_deref()
            .map(|c| {
                let bytes = URL_SAFE_NO_PAD.decode(c).map_err(|_| StoreError::CursorStale("malformed cursor".into()))?;
                serde_json::from_slice::<CursorKey>(&bytes).map_err(|_| StoreError::CursorStale("malformed cursor".into()))
            })
            .transpose()?;
        let (revision, scope_id, platform) = match self.published(&q.scan_id) {
            Ok(v) => v,
            Err(StoreError::NotFound(_)) if cursor_key.is_some() => {
                return Err(StoreError::CursorStale("the revision bound to this cursor is no longer available".into()));
            }
            Err(e) => return Err(e),
        };
        let binding = Self::binding(&q.scan_id, revision, q.sort, &q.filter, q.page_size);
        if let Some(k) = &cursor_key
            && (k.v != 1 || k.scan_id != q.scan_id || k.revision != revision || k.binding != binding)
        {
            return Err(StoreError::CursorStale("cursor belongs to another revision, sort, filter or page size".into()));
        }

        let mut sql = String::from(
            "SELECT e.entry_id, e.parent_entry_id, e.object_id, e.kind, e.boundary, e.sort_logical_be, e.sort_allocated_be,
                    e.unknown_descendants, o.link_count, o.quality, e.name_raw
             FROM entries e JOIN objects o ON o.scan_id=e.scan_id AND o.revision=e.revision AND o.object_id=e.object_id
             WHERE e.scan_id=?1 AND e.revision=?2",
        );
        let mut args: Vec<SqlValue> = vec![SqlValue::Text(q.scan_id.clone()), SqlValue::Integer(i64::from(revision))];
        match q.filter.kind {
            KindFilter::Any => {}
            KindFilter::File => sql.push_str(" AND e.kind='file'"),
            KindFilter::Directory => sql.push_str(" AND e.kind='directory'"),
        }
        if let Some(parent) = q.filter.parent_entry_id {
            args.push(SqlValue::Integer(parent as i64));
            sql.push_str(&format!(" AND e.parent_entry_id=?{}", args.len()));
        }
        if let Some(text) = &q.filter.name_contains {
            args.push(SqlValue::Text(text.clone()));
            sql.push_str(&format!(" AND instr(e.name_display, ?{}) > 0", args.len()));
        }
        let col = match q.sort {
            Sort::LogicalDesc => "e.sort_logical_be",
            Sort::AllocatedDesc => "e.sort_allocated_be",
            Sort::NameAsc => "e.name_raw",
        };
        if let Some(k) = &cursor_key {
            let last = k.last_key.as_deref().map(|h| URL_SAFE_NO_PAD.decode(h)).transpose().map_err(|_| StoreError::CursorStale("malformed cursor".into()))?;
            args.push(SqlValue::Integer(k.last_entry_id as i64));
            let id = args.len();
            match (q.sort, last) {
                (Sort::NameAsc, Some(key)) => {
                    args.push(SqlValue::Blob(key));
                    let kk = args.len();
                    sql.push_str(&format!(" AND ({col} > ?{kk} OR ({col} = ?{kk} AND e.entry_id > ?{id}))"));
                }
                (Sort::NameAsc, None) => return Err(StoreError::CursorStale("malformed cursor".into())),
                (_, Some(key)) => {
                    args.push(SqlValue::Blob(key));
                    let kk = args.len();
                    sql.push_str(&format!(" AND (({col} IS NOT NULL AND ({col} < ?{kk} OR ({col} = ?{kk} AND e.entry_id > ?{id}))) OR {col} IS NULL)"));
                }
                (_, None) => sql.push_str(&format!(" AND {col} IS NULL AND e.entry_id > ?{id}")),
            }
        }
        sql.push_str(&match q.sort {
            Sort::NameAsc => format!(" ORDER BY {col} ASC, e.entry_id ASC"),
            _ => format!(" ORDER BY {col} IS NULL, {col} DESC, e.entry_id ASC"),
        });
        sql.push_str(&format!(" LIMIT {}", q.page_size + 1));

        type Row = (i64, Option<i64>, i64, String, Option<String>, Option<Vec<u8>>, Option<Vec<u8>>, Option<i64>, Option<u32>, String, Vec<u8>);
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows: Vec<Row> = stmt
            .query_map(params_from_iter(args.iter()), |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?))
            })?
            .collect::<std::result::Result<_, _>>()?;
        let has_more = rows.len() > q.page_size as usize;
        rows.truncate(q.page_size as usize);

        let mut paths = PathCache::new(&self.conn, &q.scan_id, revision, platform);
        let mut items = Vec::with_capacity(rows.len());
        let mut unknown = 0u64;
        for row in &rows {
            let kind: ObjectKind = enum_parse(&row.3)?;
            let logical = from_be(row.5.clone())?;
            if logical.is_none() {
                unknown += 1;
            }
            let basis = match (kind, logical) {
                (_, None) => SizeBasis::Unknown,
                (ObjectKind::Directory, Some(_)) => SizeBasis::SubtreeKnown,
                _ => SizeBasis::Entity,
            };
            items.push(EntryItem {
                entry_id: row.0.to_string(),
                parent_entry_id: row.1.map(|p| p.to_string()),
                object_id: row.2.to_string(),
                path: paths.path(row.0 as u64)?,
                kind,
                logical_bytes: logical.map(Bytes),
                allocated_bytes: from_be(row.6.clone())?.map(Bytes),
                size_basis: basis,
                unknown_descendants: row.7.map(|n| n as u64),
                link_count: row.8,
                boundary: row.4.as_deref().map(enum_parse).transpose()?,
                identity_quality: enum_parse(&row.9)?,
            });
        }
        let next_cursor = if has_more {
            let last = rows.last().expect("non-empty when more rows exist");
            let key = match q.sort {
                Sort::LogicalDesc => last.5.clone(),
                Sort::AllocatedDesc => last.6.clone(),
                Sort::NameAsc => Some(last.10.clone()),
            };
            let ck = CursorKey {
                v: 1,
                scan_id: q.scan_id.clone(),
                revision,
                binding,
                last_key: key.map(|k| URL_SAFE_NO_PAD.encode(k)),
                last_entry_id: last.0 as u64,
            };
            Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&ck).expect("serializable")))
        } else {
            None
        };
        Ok(EntriesPage {
            scan_id: q.scan_id.clone(),
            scope_id,
            revision,
            sort: q.sort,
            filter: q.filter.clone(),
            page_size: q.page_size,
            items,
            next_cursor,
            unknown_size_items: unknown,
        })
    }

    /// Overlap-free summary of selected entries in the published revision (IDX-04).
    pub fn summarize(&self, scan_id: &str, entry_ids: &[u64]) -> Result<SelectionSummary> {
        let (revision, _, _) = self.published(scan_id)?;
        let tree = Tree::load(&self.conn, scan_id, revision)?;
        let selected: BTreeSet<u64> = entry_ids.iter().copied().collect();
        for id in &selected {
            if !tree.parent.contains_key(id) {
                return Err(StoreError::NotFound(format!("entry {id}")));
            }
        }
        let mut summary = SelectionSummary::default();
        // A selected entry under another selected entry adds nothing new.
        let roots: Vec<u64> = selected.iter().copied().filter(|id| !tree.ancestors(*id).any(|a| selected.contains(&a))).collect();
        summary.overlapping_selected = (selected.len() - roots.len()) as u64;
        let mut objects = BTreeSet::new();
        let mut stack = roots.clone();
        while let Some(id) = stack.pop() {
            summary.covered_entries += 1;
            let obj = tree.object[&id];
            let kind = tree.kind[&id];
            if kind != ObjectKind::Directory && objects.insert(obj) {
                summary.logical.add(tree.logical[&obj].map(Bytes))?;
                summary.allocated.add(tree.allocated[&obj].map(Bytes))?;
            }
            summary.unread_ranges += tree.skipped_under.get(&id).copied().unwrap_or(0);
            if let Some(children) = tree.children.get(&id) {
                stack.extend(children.iter().copied());
            }
        }
        for _ in 0..summary.unread_ranges {
            summary.logical.add(None)?;
            summary.allocated.add(None)?;
        }
        Ok(summary)
    }
}

/// In-memory view of one revision used for aggregation and selection summaries.
struct Tree {
    parent: HashMap<u64, Option<u64>>,
    children: HashMap<u64, Vec<u64>>,
    object: HashMap<u64, u64>,
    kind: HashMap<u64, ObjectKind>,
    logical: HashMap<u64, Option<u64>>,
    allocated: HashMap<u64, Option<u64>>,
    skipped_under: HashMap<u64, u64>,
}

impl Tree {
    fn load(conn: &Connection, scan_id: &str, revision: u32) -> Result<Self> {
        let mut t = Tree {
            parent: HashMap::new(),
            children: HashMap::new(),
            object: HashMap::new(),
            kind: HashMap::new(),
            logical: HashMap::new(),
            allocated: HashMap::new(),
            skipped_under: HashMap::new(),
        };
        let mut stmt = conn.prepare("SELECT entry_id, parent_entry_id, object_id, kind FROM entries WHERE scan_id=?1 AND revision=?2")?;
        let mut rows = stmt.query(params![scan_id, revision])?;
        while let Some(r) = rows.next()? {
            let id = r.get::<_, i64>(0)? as u64;
            let parent = r.get::<_, Option<i64>>(1)?.map(|p| p as u64);
            t.parent.insert(id, parent);
            if let Some(p) = parent {
                t.children.entry(p).or_default().push(id);
            }
            t.object.insert(id, r.get::<_, i64>(2)? as u64);
            t.kind.insert(id, enum_parse(&r.get::<_, String>(3)?)?);
        }
        let mut stmt = conn.prepare("SELECT object_id, logical_be, allocated_be FROM objects WHERE scan_id=?1 AND revision=?2")?;
        let mut rows = stmt.query(params![scan_id, revision])?;
        while let Some(r) = rows.next()? {
            let id = r.get::<_, i64>(0)? as u64;
            t.logical.insert(id, from_be(r.get(1)?)?);
            t.allocated.insert(id, from_be(r.get(2)?)?);
        }
        for id in t.object.values() {
            if !t.logical.contains_key(id) {
                return Err(StoreError::IndexCorrupt(format!("entry references missing object {id}")));
            }
        }
        let mut stmt = conn.prepare("SELECT parent_entry_id FROM skipped WHERE scan_id=?1 AND revision=?2")?;
        let mut rows = stmt.query(params![scan_id, revision])?;
        while let Some(r) = rows.next()? {
            if let Some(p) = r.get::<_, Option<i64>>(0)? {
                *t.skipped_under.entry(p as u64).or_default() += 1;
            }
        }
        Ok(t)
    }

    fn ancestors(&self, id: u64) -> impl Iterator<Item = u64> + '_ {
        let mut cur = self.parent.get(&id).copied().flatten();
        std::iter::from_fn(move || {
            let here = cur?;
            cur = self.parent.get(&here).copied().flatten();
            Some(here)
        })
    }
}

/// Directory subtree totals: each object is counted once per directory subtree (hard links
/// inside one subtree do not double count); unknown sizes and unread ranges are counted as
/// unknown descendants, never as zero.
fn compute_aggregates(conn: &Connection, scan_id: &str, revision: u32) -> Result<()> {
    let tree = Tree::load(conn, scan_id, revision)?;
    let mut entries_of_object: HashMap<u64, Vec<u64>> = HashMap::new();
    for (entry, obj) in &tree.object {
        if tree.kind[entry] != ObjectKind::Directory {
            entries_of_object.entry(*obj).or_default().push(*entry);
        }
    }
    let mut logical: HashMap<u64, u64> = HashMap::new();
    let mut allocated: HashMap<u64, u64> = HashMap::new();
    // A subtree total that cannot be represented in u64 is reported as unknown, never wrapped
    // or saturated (it is physically impossible on real volumes but must not corrupt sorting).
    let mut overflowed: BTreeSet<u64> = BTreeSet::new();
    let mut alloc_unknown: HashMap<u64, u64> = HashMap::new();
    let mut unknown: HashMap<u64, u64> = HashMap::new();
    for (obj, entries) in &entries_of_object {
        let mut dirs = BTreeSet::new();
        for e in entries {
            dirs.extend(tree.ancestors(*e));
        }
        for d in dirs {
            match tree.logical[obj] {
                Some(v) => {
                    let slot = logical.entry(d).or_default();
                    match slot.checked_add(v) {
                        Some(sum) => *slot = sum,
                        None => {
                            overflowed.insert(d);
                        }
                    }
                }
                None => *unknown.entry(d).or_default() += 1,
            }
            match tree.allocated[obj] {
                Some(v) => {
                    let slot = allocated.entry(d).or_default();
                    match slot.checked_add(v) {
                        Some(sum) => *slot = sum,
                        None => {
                            overflowed.insert(d);
                        }
                    }
                }
                None => *alloc_unknown.entry(d).or_default() += 1,
            }
        }
    }
    for (dir, count) in &tree.skipped_under {
        for d in std::iter::once(*dir).chain(tree.ancestors(*dir)) {
            *unknown.entry(d).or_default() += count;
            *alloc_unknown.entry(d).or_default() += count;
        }
    }
    let mut upd = conn.prepare_cached(
        "UPDATE entries SET sort_logical_be=?3, sort_allocated_be=?4, unknown_descendants=?5 WHERE scan_id=?1 AND revision=?2 AND entry_id=?6",
    )?;
    for (entry, kind) in &tree.kind {
        let (l, a, u) = if *kind == ObjectKind::Directory {
            let unknown_here = unknown.get(entry).copied().unwrap_or(0);
            // A directory's size is the known part of its subtree; with nothing known and
            // something unread it is unknown rather than zero.
            let l = match (logical.get(entry), unknown_here) {
                (Some(v), _) => Some(*v),
                (None, 0) => Some(0),
                (None, _) => None,
            };
            let a = match (allocated.get(entry), alloc_unknown.get(entry).copied().unwrap_or(0)) {
                (Some(v), _) => Some(*v),
                (None, 0) => Some(0),
                (None, _) => None,
            };
            if overflowed.contains(entry) {
                (None, None, Some(unknown_here as i64 + 1))
            } else {
                (l, a, Some(unknown_here as i64))
            }
        } else {
            let obj = tree.object[entry];
            (tree.logical[&obj], tree.allocated[&obj], None)
        };
        upd.execute(params![scan_id, revision, be(l), be(a), u, *entry as i64])?;
    }
    Ok(())
}

/// Builds full raw paths from parent links on demand (no full-path column, IDX design).
struct PathCache<'a> {
    conn: &'a Connection,
    scan_id: &'a str,
    revision: u32,
    platform: Platform,
    cache: HashMap<u64, RawPath>,
}

impl<'a> PathCache<'a> {
    fn new(conn: &'a Connection, scan_id: &'a str, revision: u32, platform: Platform) -> Self {
        PathCache { conn, scan_id, revision, platform, cache: HashMap::new() }
    }

    fn path(&mut self, entry_id: u64) -> Result<RawPath> {
        if let Some(p) = self.cache.get(&entry_id) {
            return Ok(p.clone());
        }
        let (parent, raw): (Option<i64>, Vec<u8>) = self.conn.query_row(
            "SELECT parent_entry_id, name_raw FROM entries WHERE scan_id=?1 AND revision=?2 AND entry_id=?3",
            params![self.scan_id, self.revision, entry_id as i64],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let name = path_from_raw(self.platform, raw)?;
        let full = match parent {
            None => name,
            Some(p) => {
                let parent_path = self.path(p as u64)?;
                join(&parent_path, &name)?
            }
        };
        self.cache.insert(entry_id, full.clone());
        Ok(full)
    }
}

/// Sink writing a backend's rows into one staging revision and relaying cancel requests.
pub struct StagingSink<'a> {
    pub store: &'a IndexStore,
    pub scan_id: String,
    pub revision: u32,
}

impl ScanSink for StagingSink<'_> {
    fn write(&mut self, objects: &[ObjectRow], entries: &[EntryRow], skipped: &[SkippedRow]) -> std::result::Result<(), String> {
        self.store.write_batch(&self.scan_id, self.revision, objects, entries, skipped).map_err(|e| e.to_string())
    }

    fn cancel_requested(&mut self) -> bool {
        self.store.cancel_requested(&self.scan_id).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests;
