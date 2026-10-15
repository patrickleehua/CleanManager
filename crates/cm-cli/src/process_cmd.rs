//! `process list|show|pin|unpin|end` (parity STS-04, task 12.3).
//!
//! The list explains each process from local facts (path, signer, parent, owning app, known purpose) and marks unknown
//! ones `identified: false` with an AI-diagnosis entry. The only handling is ending one process, and `process end`
//! only creates a `process` plan pending review: `plan review` (terminal) approves it, `plan apply` re-identifies the
//! process (PID + start time + executable) and ends it. Critical system processes, security software and CleanManager
//! itself are refused (`policy_denied`). Pins are this tool's own setting (`<store>/process-pins.json`).

use std::process::ExitCode;

use cm_core::error::ErrorCode;

use crate::args::Parsed;
use crate::fail;

/// Window over which CPU and I/O rates are measured.
#[cfg(windows)]
const WINDOW_MS: u64 = 1000;

#[cfg(windows)]
fn apps_map() -> Vec<(String, String)> {
    cm_platform::windows::apps::inventory().0.into_iter().filter_map(|a| a.install_location.map(|l| (a.name, l.display()))).collect()
}

#[cfg(windows)]
fn store_dir(p: &Parsed) -> Option<std::path::PathBuf> {
    crate::commands::store_dir(p)
}

#[cfg(windows)]
fn views(p: &Parsed, verify: bool) -> Result<(Vec<cm_core::process::ProcessView>, String), String> {
    let facts = cm_platform::windows::processes::list_with(std::time::Duration::from_millis(WINDOW_MS), verify)?;
    let sampled_at = cm_core::time::now_utc();
    let pins = store_dir(p).map(|d| cm_app::processes::load_pins(&d)).unwrap_or_default();
    let rows = cm_core::process::explain(facts, std::process::id(), &cm_platform::windows::processes::windows_dir(), &apps_map(), &pins);
    Ok((rows, sampled_at))
}

#[cfg(windows)]
fn emit_list(req: &str, p: &Parsed, rows: Vec<cm_core::process::ProcessView>, sampled_at: &str, key: cm_core::process::SortKey, search: Option<&str>, limit: usize) -> ExitCode {
    use cm_platform::windows::processes::{DISK_NOTE, NETWORK_REASON};
    let body = cm_app::processes::list_wire(rows, key, search, limit, sampled_at, WINDOW_MS, &[DISK_NOTE, NETWORK_REASON]);
    crate::emit(&cm_core::protocol::record("process_list", req, crate::commands::fields(body)), p.format);
    ExitCode::SUCCESS
}

/// `process list [--by cpu|memory|disk|network|name] [--search TEXT] [--limit N]`
#[cfg(windows)]
pub fn process_list(req: &str, p: &Parsed) -> ExitCode {
    let key = match p.option("by").map(cm_core::process::SortKey::parse) {
        None => cm_core::process::SortKey::Cpu,
        Some(Some(k)) => k,
        Some(None) => return fail(req, ErrorCode::InvalidArgument, "--by must be cpu, memory, disk, network or name", p.format),
    };
    let limit: usize = match p.option("limit").map(str::parse).unwrap_or(Ok(50)) {
        Ok(n) if (1..=500).contains(&n) => n,
        _ => return fail(req, ErrorCode::InvalidArgument, "--limit must be 1..=500", p.format),
    };
    match views(p, true) {
        Ok((rows, at)) => emit_list(req, p, rows, &at, key, p.option("search"), limit),
        Err(e) => fail(req, ErrorCode::IoError, &e, p.format),
    }
}

/// `process show PID`: one process with its local facts (a `process_list` of one row).
#[cfg(windows)]
pub fn process_show(req: &str, p: &Parsed, pid: &str) -> ExitCode {
    let Ok(pid) = pid.parse::<u32>() else { return fail(req, ErrorCode::InvalidArgument, "PID must be a number", p.format) };
    match views(p, true) {
        Ok((rows, at)) => {
            let rows: Vec<_> = rows.into_iter().filter(|r| r.facts.pid == pid).collect();
            if rows.is_empty() {
                return fail(req, ErrorCode::InvalidArgument, &format!("no running process {pid}"), p.format);
            }
            emit_list(req, p, rows, &at, cm_core::process::SortKey::Cpu, None, 1)
        }
        Err(e) => fail(req, ErrorCode::IoError, &e, p.format),
    }
}

/// `process pin|unpin NAME`: this tool's own list of names kept at the top.
pub fn process_pin(req: &str, p: &Parsed, name: &str, pinned: bool) -> ExitCode {
    let Some(dir) = crate::commands::store_dir(p) else {
        return fail(req, ErrorCode::InvalidArgument, "no store directory; pass --store or set CM_STORE_DIR", p.format);
    };
    match cm_app::processes::set_pin(&dir, name, pinned) {
        Ok(pins) => {
            crate::emit(&cm_core::protocol::record("process_pins", req, crate::commands::fields(serde_json::json!({"pins": pins}))), p.format);
            ExitCode::SUCCESS
        }
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => fail(req, ErrorCode::InvalidArgument, &e.to_string(), p.format),
        Err(e) => fail(req, ErrorCode::IoError, &e.to_string(), p.format),
    }
}

/// `process end PID`: a `process` plan pending review (nothing is ended here).
#[cfg(windows)]
pub fn process_end(req: &str, p: &Parsed, pid: &str) -> ExitCode {
    use cm_platform::windows::processes::{file_sha256, list_with, signature_of, windows_dir};
    let Ok(pid) = pid.parse::<u32>() else { return fail(req, ErrorCode::InvalidArgument, "PID must be a number", p.format) };
    let stores = match crate::commands::open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let facts = match list_with(std::time::Duration::from_millis(200), false) {
        Ok(f) => f,
        Err(e) => return fail(req, ErrorCode::IoError, &e, p.format),
    };
    let Some(mut f) = facts.into_iter().find(|f| f.pid == pid) else {
        return fail(req, ErrorCode::InvalidArgument, &format!("no running process {pid}"), p.format);
    };
    let image = f.image_path.as_ref().map(cm_core::path::RawPath::display);
    if let Some(i) = &image {
        f.signature = signature_of(i);
    }
    let row = cm_core::process::explain(vec![f], std::process::id(), &windows_dir(), &apps_map(), &[]).remove(0);
    let plan_id = format!("pp_{}", &req[4..]);
    let pdigest = crate::plan_cmd::current_policy_digest(&stores);
    let plan = match cm_app::processes::end_plan(&row, image.as_deref().and_then(file_sha256), &plan_id, crate::plan_cmd::now_unix(), &pdigest) {
        Ok(pl) => pl,
        Err(cm_app::processes::EndPlanError::Refused(r @ cm_core::process::EndRefusal::Protected(_))) => return fail(req, ErrorCode::PolicyDenied, &r.to_string(), p.format),
        Err(e) => return fail(req, ErrorCode::InvalidArgument, &e.to_string(), p.format),
    };
    if let Err(e) = stores.audit.save_plan_v2(&plan) {
        return crate::commands::store_error(req, &e, p.format);
    }
    crate::emit(&crate::plan_cmd::v2_plan_record(req, &stores, &plan), p.format);
    ExitCode::SUCCESS
}

#[cfg(not(windows))]
pub fn process_list(req: &str, p: &Parsed) -> ExitCode {
    fail(req, ErrorCode::Unsupported, "the process list is not implemented on this platform yet", p.format)
}

#[cfg(not(windows))]
pub fn process_show(req: &str, p: &Parsed, _: &str) -> ExitCode {
    fail(req, ErrorCode::Unsupported, "the process list is not implemented on this platform yet", p.format)
}

#[cfg(not(windows))]
pub fn process_end(req: &str, p: &Parsed, _: &str) -> ExitCode {
    fail(req, ErrorCode::Unsupported, "ending processes is not implemented on this platform yet", p.format)
}
