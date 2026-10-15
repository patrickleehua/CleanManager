//! Command dispatch. Each command emits exactly one terminal record and maps its outcome to the
//! stable exit classification (CLI-05).

use std::path::PathBuf;
use std::process::ExitCode;

use cm_app::scan::{ScanRequest, run_scan};
use cm_core::capability::CapabilityProvider;
use cm_core::error::{ErrorCode, ExitClass};
use cm_core::host::HostProbe;
use cm_core::model::ScanState;
use cm_core::protocol::{capabilities_record, record};
use cm_core::scan::{ScanBackend, ScanOptions};
use cm_platform::host::OsHostProbe;
use cm_store::index::ScanStatusView;
use cm_store::{StoreError, Stores};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::args::Parsed;
use crate::{Format, emit, fail};

pub fn run(req: &str, p: Parsed) -> ExitCode {
    let words: Vec<&str> = p.words.iter().map(String::as_str).collect();
    let result = match words.as_slice() {
        ["capabilities"] => p.allow(&[], &[]).map(|_| capabilities(req, &p)),
        ["scan"] => p.allow(&["root", "store"], &[]).map(|_| scan(req, &p)),
        ["scan", "show", id] => p.allow(&["store"], &[]).map(|_| scan_show(req, &p, id)),
        ["scan", "cancel", id] => p.allow(&["store"], &[]).map(|_| scan_cancel(req, &p, id)),
        ["analyze"] => p.allow(&["scan", "store", "rules", "preview-rule"], &[]).map(|_| crate::analysis_cmd::analyze_cmd(req, &p)),
        ["candidates", "list"] => p.allow(&["set", "store", "limit", "cursor"], &[]).map(|_| crate::analysis_cmd::candidates_list(req, &p)),
        ["plan", "create"] => p.allow(&["set", "store", "mode", "candidates"], &["all-default-selected"]).map(|_| crate::plan_cmd::plan_create(req, &p)),
        ["plan", "show", id] => p.allow(&["store"], &[]).map(|_| crate::plan_cmd::plan_show(req, &p, id)),
        ["plan", "review", id] => p.allow(&["store"], &[]).map(|_| crate::plan_cmd::plan_review(req, &p, id)),
        ["plan", "revoke", id] => p.allow(&["store"], &[]).map(|_| crate::plan_cmd::plan_revoke(req, &p, id)),
        ["plan", "apply", id] => p.allow(&["store", "rules"], &[]).map(|_| crate::apply_cmd::plan_apply(req, &p, id)),
        ["operations", "show", id] => p.allow(&["store"], &[]).map(|_| crate::apply_cmd::operations_show(req, &p, id)),
        ["operations", "reconcile", id] => p.allow(&["store"], &[]).map(|_| crate::apply_cmd::operations_reconcile(req, &p, id)),
        ["operations", "list"] => p.allow(&["store", "limit", "cursor"], &[]).map(|_| crate::recovery_cmd::operations_list(req, &p)),
        ["recovery" | "quarantine", "list"] => p.allow(&["store", "limit", "cursor"], &[]).map(|_| crate::recovery_cmd::recovery_list(req, &p)),
        ["recovery", "show", id] => p.allow(&["store"], &[]).map(|_| crate::recovery_cmd::recovery_show(req, &p, id)),
        ["recovery", "plan", "create"] => p.allow(&["store", "restore", "purge"], &[]).map(|_| crate::recovery_cmd::recovery_plan_create(req, &p)),
        ["policy", "create"] => p.allow(&["store", "scope", "rules", "mode"], &[]).map(|_| crate::plan_cmd::policy_create(req, &p)),
        ["apps", "list"] => p.allow(&["limit", "cursor"], &[]).map(|_| crate::apps_cmd::apps_list(req, &p)),
        ["startup", "list"] => p.allow(&[], &[]).map(|_| crate::startup_cmd::startup_list(req, &p)),
        ["status"] => p.allow(&["interval", "count"], &["watch"]).map(|_| crate::status_cmd::status(req, &p)),
        ["insights"] => p.allow(&[], &[]).map(|_| crate::insights_cmd::insights_list(req, &p)),
        ["policy", "list"] => p.allow(&["store"], &[]).map(|_| crate::plan_cmd::policy_list(req, &p)),
        ["policy", "enable", id] => p.allow(&["store"], &[]).map(|_| crate::plan_cmd::policy_set_state(req, &p, id, cm_core::authz::PolicyState::Enabled)),
        ["policy", "pause", id] => p.allow(&["store"], &[]).map(|_| crate::plan_cmd::policy_set_state(req, &p, id, cm_core::authz::PolicyState::Paused)),
        ["policy", "revoke", id] => p.allow(&["store"], &[]).map(|_| crate::plan_cmd::policy_set_state(req, &p, id, cm_core::authz::PolicyState::Revoked)),
        ["protect", "list"] => p.allow(&["store"], &[]).map(|_| crate::protect_cmd::protect_list(req, &p)),
        ["protect", "add"] => p.allow(&["store", "path", "match", "glob", "rule", "task", "note"], &[]).map(|_| crate::protect_cmd::protect_add(req, &p)),
        ["protect", "remove", id] => p.allow(&["store"], &[]).map(|_| crate::protect_cmd::protect_remove(req, &p, id)),
        ["protect", "check"] => p.allow(&["store", "path", "rule"], &[]).map(|_| crate::protect_cmd::protect_check(req, &p)),
        [] => Err("missing command".to_string()),
        other => Err(format!("unknown command: {}", other.join(" "))),
    };
    match result {
        Ok(code) => code,
        Err(msg) => fail(req, ErrorCode::InvalidArgument, &msg, p.format),
    }
}

fn capabilities(req: &str, p: &Parsed) -> ExitCode {
    let host = cm_platform::host();
    emit(&capabilities_record(req, host.platform(), &host.capabilities()), p.format);
    ExitCode::SUCCESS
}

fn store_dir(p: &Parsed) -> Option<PathBuf> {
    p.raw_option("store").map(PathBuf::from).or_else(cm_platform::paths::default_store_dir)
}

pub fn open_stores(req: &str, p: &Parsed) -> Result<Stores, ExitCode> {
    let Some(dir) = store_dir(p) else {
        return Err(fail(req, ErrorCode::InvalidArgument, "no store directory; pass --store or set CM_STORE_DIR", p.format));
    };
    Stores::open(&dir).map_err(|e| store_error(req, &e, p.format))
}

pub fn store_error(req: &str, e: &StoreError, format: Format) -> ExitCode {
    fail(req, e.code(), &e.to_string(), format)
}

pub fn fields(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}

#[cfg(windows)]
fn backend() -> impl ScanBackend {
    cm_platform::windows::directory::DirectoryBackend
}

/// macOS: the portable POSIX walker is the default until the bulk backend's record layout is
/// verified on real hardware (MVP 5.x); both produce identical row shapes.
#[cfg(target_os = "macos")]
fn backend() -> impl ScanBackend {
    cm_platform::macos::posix::PosixBackend
}

#[cfg(not(any(windows, target_os = "macos")))]
fn backend() -> impl ScanBackend {
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

fn scan(req: &str, p: &Parsed) -> ExitCode {
    let Some(root_os) = p.raw_option("root") else {
        return fail(req, ErrorCode::InvalidArgument, "scan needs --root PATH", p.format);
    };
    let root = match cm_platform::paths::raw_path(root_os) {
        Ok(r) => r,
        Err(e) => return fail(req, ErrorCode::InvalidArgument, &format!("invalid root: {e}"), p.format),
    };
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(code) => return code,
    };
    if std::path::Path::new(root_os).exists()
        && let Err(e) = stores.ensure_outside(std::path::Path::new(root_os))
    {
        return store_error(req, &e, p.format);
    }
    let digest: String = Sha256::digest(root.raw_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect();
    let host = OsHostProbe.current("cli");
    let scan_id = format!("s_{}", &req[4..]);
    let request = ScanRequest { scan_id: scan_id.clone(), scope_id: format!("scope_{digest}"), root, host };
    let outcome = match run_scan(&stores.index, &backend(), &request, &ScanOptions::default()) {
        Ok(o) => o,
        Err(e) => return store_error(req, &e, p.format),
    };
    match scan_result(req, &stores, &scan_id) {
        Ok(v) => {
            emit(&v, p.format);
            let class = match outcome.state {
                ScanState::Completed => ExitClass::Success,
                ScanState::Partial | ScanState::Interrupted => ExitClass::Partial,
                ScanState::Cancelled => ExitClass::Error(ErrorCode::Cancelled),
                _ if outcome.report.issues.iter().any(|i| i.code == "permission_required") => ExitClass::Error(ErrorCode::PermissionRequired),
                _ if outcome.report.issues.iter().any(|i| i.code == "unsupported") => ExitClass::Error(ErrorCode::Unsupported),
                _ => ExitClass::Error(ErrorCode::IoError),
            };
            ExitCode::from(class.code() as u8)
        }
        Err(e) => store_error(req, &e, p.format),
    }
}

/// Protocol `scan_result` for a published scan.
fn scan_result(req: &str, stores: &Stores, scan_id: &str) -> Result<Value, StoreError> {
    let status = stores.index.scan_status(scan_id)?;
    let report = stores.index.scan_report(scan_id)?.unwrap_or(Value::Null);
    let summary = if status.entries_seen > 0 { Some(stores.index.summarize(scan_id, &[1])?) } else { None };
    let (logical, unknown, allocated) = match summary {
        Some(s) => (s.logical.known.to_string(), s.logical.unknown_count, s.allocated.exact().map(|b| b.to_string())),
        None => ("0".to_string(), 0, None),
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
            "issues": report["issues"].as_array().cloned().unwrap_or_default(),
        })),
    ))
}

fn scan_status_record(req: &str, s: &ScanStatusView) -> Value {
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
            "known_logical_bytes": "0",
            "last_issue": null,
        })),
    )
}

fn scan_show(req: &str, p: &Parsed, id: &str) -> ExitCode {
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(code) => return code,
    };
    // A scan whose host exited is reported as interrupted, never as running forever (CLI-09).
    if let Err(e) = stores.index.reconcile_hosts(|h| OsHostProbe.liveness(h), &cm_core::time::now_utc()) {
        return store_error(req, &e, p.format);
    }
    match stores.index.scan_status(id) {
        Ok(s) if !s.state.is_terminal() => {
            emit(&scan_status_record(req, &s), p.format);
            ExitCode::SUCCESS
        }
        Ok(_) => match scan_result(req, &stores, id) {
            Ok(v) => {
                emit(&v, p.format);
                ExitCode::SUCCESS
            }
            Err(e) => store_error(req, &e, p.format),
        },
        Err(e) => store_error(req, &e, p.format),
    }
}

fn scan_cancel(req: &str, p: &Parsed, id: &str) -> ExitCode {
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(code) => return code,
    };
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
