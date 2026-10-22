//! Scan lifecycle commands (CLI-02/03/09, MVP 12.1/12.3) and the read-only entry pages.
//!
//! - `scan --root P`: hosts the scan in this process. `--format jsonl` streams `progress`
//!   records (the first one right after the scan record exists, carrying `task_id` = scan_id)
//!   and then exactly one `scan_result`. `--timeout S` is a wait budget: the host records a
//!   cancel request for its own scan, stops at the next safe boundary and exits 8; confirmed rows
//!   stay published and nothing is rolled back.
//! - `scan --root P --detach`: starts a separate `cleanmanager` process as the host and returns
//!   its `scan_status` as soon as the scan record exists. The scan lives exactly as long as that
//!   process; there is no daemon.
//! - `scan show ID`: `scan_status` while created/running/stopping, `scan_result` once terminal.
//!   A scan whose host exited is published as `interrupted` with a `host_exited` issue first.
//! - `scan wait ID [--timeout S]`: polls until terminal (exit classified like the scan) or the
//!   budget runs out (the current non-terminal `scan_status`, exit 8, nothing cancelled).
//! - `scan cancel ID`: a receipt only, never a terminal state.
//! - `entries list --scan ID`: bounded pages of a published revision (IDX-01..03).

use std::cell::Cell;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use cm_app::scan::{ScanHooks, ScanRequest, TIMEOUT_ISSUE, run_scan_hooked};
use cm_core::error::{ErrorCode, ExitClass};
use cm_core::host::HostProbe;
use cm_core::model::ScanState;
use cm_core::protocol::record;
use cm_core::scan::{ScanBackend, ScanOptions};
use cm_platform::host::OsHostProbe;
use cm_store::index::{DEFAULT_PAGE, EntryQuery, Filter, KindFilter, MAX_PAGE, ScanStatusView, Sort};
use cm_store::{StoreError, Stores};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::args::Parsed;
use crate::commands::{fields, open_stores, store_dir, store_error};
use crate::{Format, emit, fail};

/// Exit status of a run whose wait budget ran out: the system-failure class (8), meaning "query
/// and reconcile by ID"; it never means cancelled-by-user (130) and never implies a rollback.
const TIMEOUT_EXIT: u8 = 8;
const MAX_TIMEOUT_SECONDS: u64 = 86_400;
const DEFAULT_WAIT_SECONDS: u64 = 60;

#[cfg(windows)]
pub fn backend() -> impl ScanBackend {
    cm_platform::windows::directory::DirectoryBackend
}

/// macOS: the portable POSIX walker stays the default until the real-Mac benchmark (MVP 6.2) and
/// the G0 decision (6.3) choose between it and `mac.bulk`. Both produce identical row shapes; only
/// `mac.bulk` observes APFS block sharing (`shared_blocks`), which `mac.posix` reports as unknown.
#[cfg(target_os = "macos")]
pub fn backend() -> impl ScanBackend {
    cm_platform::macos::posix::PosixBackend
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn backend() -> impl ScanBackend {
    UnsupportedBackend
}

#[cfg(not(any(windows, target_os = "macos")))]
struct UnsupportedBackend;

#[cfg(not(any(windows, target_os = "macos")))]
impl ScanBackend for UnsupportedBackend {
    fn name(&self) -> &'static str {
        "mac.posix"
    }
    fn probe(&self, _: &cm_core::path::RawPath) -> cm_core::scan::RootProbe {
        cm_core::scan::RootProbe {
            filesystem: None,
            volume_id: None,
            backend: "mac.posix".into(),
            state: cm_core::capability::CapabilityState::Unsupported,
            reason: "no scan backend for this platform".into(),
        }
    }
    fn scan(&self, _: &cm_core::path::RawPath, _: &mut dyn cm_core::scan::ScanSink, _: &ScanOptions) -> cm_core::scan::ScanReport {
        cm_core::scan::ScanReport {
            state: ScanState::Failed,
            backend: "mac.posix".into(),
            backend_reason: "no scan backend for this platform".into(),
            entries: 0,
            skipped_ranges: 0,
            boundaries: 0,
            elevated_read: vec![],
            issues: vec![cm_core::scan::ScanIssue { code: "unsupported".into(), scope_display: String::new(), message: "no scan backend for this platform".into() }],
        }
    }
}

fn seconds(p: &Parsed, min: u64, default: Option<u64>) -> Result<Option<u64>, String> {
    match p.option("timeout") {
        None => Ok(default),
        Some(s) => match s.parse::<u64>() {
            Ok(n) if (min..=MAX_TIMEOUT_SECONDS).contains(&n) => Ok(Some(n)),
            _ => Err(format!("--timeout must be whole seconds in {min}..={MAX_TIMEOUT_SECONDS}")),
        },
    }
}

/// Test hook (debug builds only): slow every batch down so lifecycle tests can act on a live
/// scan from another process. Release builds ignore it.
fn test_slowdown() -> Option<Duration> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var("CM_TEST_SCAN_SLOW_MS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|n| *n > 0).map(Duration::from_millis)
}

fn progress_record(req: &str, scan_id: &str, sequence: u64, phase: &str, completed: u64) -> Value {
    record(
        "progress",
        req,
        fields(json!({
            "task_id": scan_id, "sequence": sequence, "phase": phase, "completed_units": completed,
            // Enumeration has no known denominator: never a fabricated percentage (CLI-03).
            "total_units": null, "emitted_at": cm_core::time::now_utc(),
        })),
    )
}

/// Exit classification of a terminal scan (CLI-05): partial and interrupted keep their results (5).
fn exit_for(state: ScanState, issues: &[Value]) -> ExitCode {
    let has = |code: &str| issues.iter().any(|i| i["code"] == code);
    let class = match state {
        ScanState::Completed => ExitClass::Success,
        _ if has(TIMEOUT_ISSUE) => return ExitCode::from(TIMEOUT_EXIT),
        ScanState::Partial | ScanState::Interrupted => ExitClass::Partial,
        ScanState::Cancelled => ExitClass::Error(ErrorCode::Cancelled),
        _ if has("permission_required") => ExitClass::Error(ErrorCode::PermissionRequired),
        _ if has("unsupported") => ExitClass::Error(ErrorCode::Unsupported),
        _ => ExitClass::Error(ErrorCode::IoError),
    };
    ExitCode::from(class.code() as u8)
}

fn valid_scan_id(id: &str) -> bool {
    id.len() <= 80 && id.strip_prefix("s_").is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
}

pub fn scan(req: &str, p: &Parsed) -> ExitCode {
    let Some(root_os) = p.raw_option("root") else {
        return fail(req, ErrorCode::InvalidArgument, "scan needs --root PATH", p.format);
    };
    let timeout = match seconds(p, 1, None) {
        Ok(t) => t,
        Err(e) => return fail(req, ErrorCode::InvalidArgument, &e, p.format),
    };
    let root = match cm_platform::paths::raw_path(root_os) {
        Ok(r) => r,
        Err(e) => return fail(req, ErrorCode::InvalidArgument, &format!("invalid root: {e}"), p.format),
    };
    let scan_id = match p.option("host-scan-id") {
        Some(id) if p.flag("detach") => return fail(req, ErrorCode::InvalidArgument, &format!("--host-scan-id {id} cannot be combined with --detach"), p.format),
        Some(id) if valid_scan_id(id) => id.to_owned(),
        Some(id) => return fail(req, ErrorCode::InvalidArgument, &format!("invalid scan id {id}"), p.format),
        None => format!("s_{}", &req[4..]),
    };
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(code) => return code,
    };
    if Path::new(root_os).exists()
        && let Err(e) = stores.ensure_outside(Path::new(root_os))
    {
        return store_error(req, &e, p.format);
    }
    // An unusable index is reported before any work, in particular before a background host is started.
    if let Err(e) = stores.index.ready() {
        return store_error(req, &e, p.format);
    }
    if p.flag("detach") {
        // The background host runs without --debug: its stderr is closed, so it would trace into nothing.
        cm_core::debug_trace!("scan", "host=detached scan_id={scan_id}");
        return detach(req, p, &stores, root_os, &scan_id, timeout);
    }
    cm_core::debug_trace!("scan", "host=this_process scan_id={scan_id} stream={} timeout={timeout:?}", p.stream);
    let digest: String = Sha256::digest(root.raw_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect();
    let host = OsHostProbe.current("cli");
    let request = ScanRequest { scan_id: scan_id.clone(), scope_id: format!("scope_{digest}"), root, host };
    let slow = test_slowdown();
    let mut options = ScanOptions::default();
    if slow.is_some() {
        options.batch_entries = 1;
    }
    let sequence = Cell::new(0u64);
    let last = Cell::new(None::<Instant>);
    let progress = |phase: &str, n: u64| {
        if p.stream {
            emit(&progress_record(req, &scan_id, sequence.get(), phase, n), Format::Json);
            sequence.set(sequence.get() + 1);
        }
    };
    let started = |_: &str| progress("discovering", 0);
    let batch = |n: u64| {
        if let Some(d) = slow {
            std::thread::sleep(d);
        }
        // At most four progress records per second; the terminal record is always last.
        if last.get().is_none_or(|t| t.elapsed() >= Duration::from_millis(250)) {
            last.set(Some(Instant::now()));
            progress("discovering", n);
        }
    };
    let hooks = ScanHooks { started: Some(&started), progress: Some(&batch), deadline: timeout.map(|s| Instant::now() + Duration::from_secs(s)) };
    let outcome = match run_scan_hooked(&stores.index, &backend(), &request, &options, &hooks) {
        Ok(o) => o,
        Err(e) => return store_error(req, &e, p.format),
    };
    match scan_result(req, &stores, &scan_id) {
        Ok(v) => {
            emit(&v, p.format);
            let issues: Vec<Value> = outcome.report.issues.iter().map(|i| json!({"code": i.code})).collect();
            exit_for(outcome.state, &issues)
        }
        Err(e) => store_error(req, &e, p.format),
    }
}

/// Start another `cleanmanager` process as the scan host and return once the record exists.
fn detach(req: &str, p: &Parsed, stores: &Stores, root: &std::ffi::OsString, scan_id: &str, timeout: Option<u64>) -> ExitCode {
    let Some(dir) = store_dir(p) else { return fail(req, ErrorCode::InvalidArgument, "no store directory; pass --store or set CM_STORE_DIR", p.format) };
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return fail(req, ErrorCode::InternalError, &format!("cannot locate this executable: {e}"), p.format),
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("scan").arg("--root").arg(root).arg("--store").arg(&dir).args(["--host-scan-id", scan_id, "--json", "--non-interactive"]);
    if let Some(t) = timeout {
        cmd.args(["--timeout", &t.to_string()]);
    }
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let child = spawn_detached(&mut cmd);
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return fail(req, ErrorCode::InternalError, &format!("cannot start the background scan host: {e}"), p.format),
    };
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        match stores.index.scan_status(scan_id) {
            Ok(s) => return show_status(req, p, stores, &s, scan_id),
            Err(StoreError::NotFound(_)) => {}
            Err(e) => return store_error(req, &e, p.format),
        }
        if let Ok(Some(status)) = child.try_wait() {
            return fail(req, ErrorCode::InternalError, &format!("the background scan host exited ({status}) before the scan started; run without --detach to see why"), p.format);
        }
        if Instant::now() >= until {
            return fail(req, ErrorCode::InternalError, &format!("the background scan host did not start {scan_id} within 30 s; query scan show {scan_id} later"), p.format);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(windows)]
fn spawn_detached(cmd: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
    // CreateProcess inherits every inheritable handle: without this the background host would hold
    // our caller's stdout/stderr pipes open, and a caller reading to EOF would wait for the whole scan.
    for handle in [std::io::stdin().as_raw_handle(), std::io::stdout().as_raw_handle(), std::io::stderr().as_raw_handle()] {
        if !handle.is_null() {
            // SAFETY: a standard handle of this process; only its inherit flag changes.
            unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) };
        }
    }
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    // Outlive a caller whose job object closes with it, when the job allows breaking away.
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB)
        .spawn()
        .or_else(|_| cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP).spawn())
}

#[cfg(unix)]
fn spawn_detached(cmd: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0).spawn()
}

/// Protocol `scan_result` for a published scan.
pub fn scan_result(req: &str, stores: &Stores, scan_id: &str) -> Result<Value, StoreError> {
    let status = stores.index.scan_status(scan_id)?;
    let report = stores.index.scan_report(scan_id)?.unwrap_or(Value::Null);
    // A failed scan has no usable subtotal: its sizes are unknown (null), never "0" (SCN-03).
    // Other terminal states report the known part of what was confirmed, "0" only when that is zero.
    let failed = status.state == ScanState::Failed;
    let summary = if status.entries_seen > 0 && !failed { Some(stores.index.summarize(scan_id, &[1])?) } else { None };
    let (logical, unknown, allocated) = match summary {
        Some(s) => (Some(s.logical.known.to_string()), s.logical.unknown_count, s.allocated.exact().map(|b| b.to_string())),
        None if failed => (None, 0, None),
        None => (Some("0".to_string()), 0, None),
    };
    Ok(record(
        "scan_result",
        req,
        fields(json!({
            "scan_id": scan_id,
            "scope_id": status.scope_id,
            "revision": status.revision,
            "backend": report["backend"].as_str().unwrap_or("unknown"),
            "status": status.state,
            "completeness": status.completeness,
            "started_at": status.started_at,
            "finished_at": status.finished_at,
            "entries_seen": status.entries_seen,
            "skipped_ranges": status.skipped_ranges,
            "logical_bytes": logical,
            "allocated_bytes": allocated,
            "unknown_size_count": unknown,
            // The structured `permission_cause` of each issue whose message names one (macos.md §6).
            "issues": report["issues"].as_array().cloned().map(|mut i| {
                cm_core::permission::annotate_issues(&mut i);
                i
            }).unwrap_or_default(),
        })),
    ))
}

pub fn scan_status_record(req: &str, s: &ScanStatusView) -> Value {
    record(
        "scan_status",
        req,
        fields(json!({
            "scan_id": s.scan_id,
            "scope_id": s.scope_id,
            "state": s.state,
            "host": {"kind": s.host.kind, "process_id": s.host.pid, "started_at": s.host.started_at, "last_heartbeat_at": s.heartbeat_at},
            "cancel_requested": s.cancel_requested,
            "entries_seen": s.entries_seen,
            "skipped_ranges": s.skipped_ranges,
            // Sizes are aggregated when the revision is published; while running only counts are known.
            "known_logical_bytes": "0",
            "last_issue": null,
        })),
    )
}

/// Current state of one scan after host reconciliation: running → `scan_status`, terminal → `scan_result`.
fn current(stores: &Stores, scan_id: &str) -> Result<ScanStatusView, StoreError> {
    // A scan whose host exited is reported as interrupted, never as running forever (CLI-09).
    stores.index.reconcile_hosts(|h| OsHostProbe.liveness(h), &cm_core::time::now_utc())?;
    stores.index.scan_status(scan_id)
}

fn show_status(req: &str, p: &Parsed, stores: &Stores, s: &ScanStatusView, scan_id: &str) -> ExitCode {
    if !s.state.is_terminal() {
        emit(&scan_status_record(req, s), p.format);
        return ExitCode::SUCCESS;
    }
    match scan_result(req, stores, scan_id) {
        Ok(v) => {
            emit(&v, p.format);
            exit_for(s.state, v["issues"].as_array().map(Vec::as_slice).unwrap_or_default())
        }
        Err(e) => store_error(req, &e, p.format),
    }
}

pub fn scan_show(req: &str, p: &Parsed, id: &str) -> ExitCode {
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(code) => return code,
    };
    match current(&stores, id) {
        Ok(s) => show_status(req, p, &stores, &s, id),
        Err(e) => store_error(req, &e, p.format),
    }
}

pub fn scan_wait(req: &str, p: &Parsed, id: &str) -> ExitCode {
    let budget = match seconds(p, 0, Some(DEFAULT_WAIT_SECONDS)) {
        Ok(t) => Duration::from_secs(t.unwrap_or(DEFAULT_WAIT_SECONDS)),
        Err(e) => return fail(req, ErrorCode::InvalidArgument, &e, p.format),
    };
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let until = Instant::now() + budget;
    loop {
        let s = match current(&stores, id) {
            Ok(s) => s,
            Err(e) => return store_error(req, &e, p.format),
        };
        if s.state.is_terminal() {
            return show_status(req, p, &stores, &s, id);
        }
        if Instant::now() >= until {
            // The wait ended, the scan did not: report where it stands, cancel nothing, claim nothing.
            emit(&scan_status_record(req, &s), p.format);
            return ExitCode::from(TIMEOUT_EXIT);
        }
        std::thread::sleep(Duration::from_millis(200).min(until.saturating_duration_since(Instant::now()).max(Duration::from_millis(1))));
    }
}

pub fn scan_cancel(req: &str, p: &Parsed, id: &str) -> ExitCode {
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(code) => return code,
    };
    // An exited host cannot honour the request: publish it as interrupted first, then the receipt says already_terminal.
    if let Err(e) = stores.index.reconcile_hosts(|h| OsHostProbe.liveness(h), &cm_core::time::now_utc()) {
        return store_error(req, &e, p.format);
    }
    match stores.index.request_cancel(id) {
        Ok(r) => {
            emit(
                &record(
                    "scan_cancel_receipt",
                    req,
                    fields(json!({
                        "scan_id": r.scan_id,
                        "accepted": r.accepted,
                        "state_at_receipt": r.state_at_receipt,
                        "requested_at": cm_core::time::now_utc(),
                        "note": "Receipt only: the scan stops at the next safe boundary; query scan show for the terminal record.",
                    })),
                ),
                p.format,
            );
            ExitCode::SUCCESS
        }
        Err(e) => store_error(req, &e, p.format),
    }
}

fn page_size(p: &Parsed) -> Result<u32, String> {
    match p.option("limit") {
        None => Ok(DEFAULT_PAGE),
        Some(s) => match s.parse::<u32>() {
            Ok(n) if (1..=MAX_PAGE).contains(&n) => Ok(n),
            _ => Err(format!("--limit must be 1..={MAX_PAGE}")),
        },
    }
}

/// `entries list --scan ID [--sort S] [--kind K] [--parent ENTRY_ID] [--name TEXT] [--limit N] [--cursor C]`.
pub fn entries_list(req: &str, p: &Parsed) -> ExitCode {
    let Some(scan_id) = p.option("scan") else { return fail(req, ErrorCode::InvalidArgument, "entries list needs --scan ID", p.format) };
    let sort = match p.option("sort").unwrap_or("logical_desc") {
        "logical_desc" => Sort::LogicalDesc,
        "allocated_desc" => Sort::AllocatedDesc,
        "name_asc" => Sort::NameAsc,
        other => return fail(req, ErrorCode::InvalidArgument, &format!("unknown sort {other}; use logical_desc, allocated_desc or name_asc"), p.format),
    };
    let kind = match p.option("kind").unwrap_or("any") {
        "any" => KindFilter::Any,
        "file" => KindFilter::File,
        "directory" => KindFilter::Directory,
        other => return fail(req, ErrorCode::InvalidArgument, &format!("unknown kind {other}; use any, file or directory"), p.format),
    };
    let parent = match p.option("parent").map(str::parse::<u64>) {
        None => None,
        Some(Ok(id)) => Some(id),
        Some(Err(_)) => return fail(req, ErrorCode::InvalidArgument, "--parent takes an entry_id from a previous page", p.format),
    };
    let limit = match page_size(p) {
        Ok(n) => n,
        Err(e) => return fail(req, ErrorCode::InvalidArgument, &e, p.format),
    };
    let name = p.option("name").filter(|s| !s.is_empty()).map(str::to_owned);
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let q = EntryQuery {
        scan_id: scan_id.to_owned(),
        sort,
        filter: Filter { kind, name_contains: name.clone(), parent_entry_id: parent },
        page_size: limit,
        cursor: p.option("cursor").map(str::to_owned),
    };
    match stores.index.list_entries(&q) {
        Ok(page) => {
            // DIN-05: read-only "cleanable by hand" hints for this page's directories (absent = none).
            let hints = crate::analysis_cmd::entry_hints(&page.items, &stores.dir);
            let items: Vec<serde_json::Value> = page
                .items
                .iter()
                .zip(hints)
                .map(|(item, hint)| {
                    let mut v = json!(item);
                    if let (Some(h), Some(obj)) = (hint, v.as_object_mut()) {
                        obj.insert("cleanable_hint".into(), json!(h));
                    }
                    v
                })
                .collect();
            emit(
                &record(
                    "entries_page",
                    req,
                    fields(json!({
                        "scan_id": page.scan_id,
                        "scope_id": page.scope_id,
                        "revision": page.revision,
                        "sort": page.sort,
                        "filter": {"kind": page.filter.kind, "name_contains": name, "parent_entry_id": parent.map(|e| e.to_string())},
                        "page_size": page.page_size,
                        "items": items,
                        "next_cursor": page.next_cursor,
                        "unknown_size_items": page.unknown_size_items,
                    })),
                ),
                p.format,
            );
            ExitCode::SUCCESS
        }
        Err(e) => store_error(req, &e, p.format),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cm_core::model::{CloudState, IdentityQuality, ObjectKind};
    use cm_core::path::RawPath;
    use cm_core::scan::{EntryRow, ObjectRow, ScanIssue, ScanReport};
    use cm_store::index::ScanInit;

    fn publish(stores: &Stores, scan_id: &str, state: ScanState) -> Value {
        let root = RawPath::unix(b"/r".to_vec()).unwrap();
        let host = cm_core::host::HostRef { kind: "cli".into(), pid: std::process::id(), started_at: "2026-10-22T00:00:00Z".into() };
        let rev = stores.index.begin_scan(&ScanInit { scan_id: scan_id.into(), scope_id: "scope".into(), root: root.clone(), backend: "test".into(),
            host, started_at: "2026-10-22T00:00:00Z".into() }).unwrap();
        let object = |id: u64, kind: ObjectKind, logical: Option<u64>| ObjectRow { object_id: id, volume_id: "v".into(), file_id: format!("inode64:{id:016x}"),
            generation: None, quality: IdentityQuality::Strong, kind, logical, allocated: logical, link_count: None, cloud: CloudState::NotCloud,
            modified_unix: None, shared_blocks: None };
        let file = RawPath::unix(b"a".to_vec()).unwrap();
        stores.index.write_batch(scan_id, rev, &[object(1, ObjectKind::Directory, None), object(2, ObjectKind::File, Some(0))],
            &[EntryRow { entry_id: 1, parent_entry_id: None, name: root, object_id: 1, kind: ObjectKind::Directory, boundary: None },
              EntryRow { entry_id: 2, parent_entry_id: Some(1), name: file, object_id: 2, kind: ObjectKind::File, boundary: None }], &[]).unwrap();
        let report = ScanReport { state, backend: "test".into(), backend_reason: "test".into(), entries: 2, skipped_ranges: 0, boundaries: 0,
            elevated_read: vec![], issues: vec![ScanIssue { code: "store_error".into(), scope_display: String::new(), message: "disk full".into() }] };
        stores.index.record_report(scan_id, &report).unwrap();
        stores.index.publish(scan_id, rev, state, "2026-10-22T00:00:01Z").unwrap();
        scan_result("req_test", stores, scan_id).unwrap()
    }

    #[test]
    fn failed_scans_report_unknown_sizes_and_others_keep_their_known_subtotal() {
        let dir = std::env::temp_dir().join(format!("cm-cli-scan-result-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let stores = Stores::open(&dir).unwrap();
        // Rows were written before the failure: their subtotal is still not reported (SCN-03).
        let failed = publish(&stores, "s_failed", ScanState::Failed);
        assert_eq!((failed["status"].as_str(), failed["entries_seen"].as_u64()), (Some("failed"), Some(2)), "{failed}");
        assert!(failed["logical_bytes"].is_null() && failed["allocated_bytes"].is_null(), "{failed}");
        // A known zero subtotal of a scan that ran stays "0".
        let cancelled = publish(&stores, "s_cancelled", ScanState::Cancelled);
        assert_eq!((cancelled["logical_bytes"].as_str(), cancelled["allocated_bytes"].as_str()), (Some("0"), Some("0")), "{cancelled}");
        drop(stores);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
