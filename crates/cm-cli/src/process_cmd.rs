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
const WINDOW_MS: u64 = 1000;

#[cfg(windows)]
fn apps_map() -> Vec<(String, String)> {
    cm_platform::windows::apps::inventory().0.into_iter().filter_map(|a| a.install_location.map(|l| (a.name, l.display()))).collect()
}

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

#[cfg(target_os = "macos")]
fn views(p: &Parsed, verify: bool) -> Result<(Vec<cm_core::process::ProcessView>, String), String> {
    let pins = store_dir(p).map(|d| cm_app::processes::load_pins(&d)).unwrap_or_default();
    let rows = cm_platform::macos::processes::views(std::time::Duration::from_millis(WINDOW_MS), verify, &pins)?;
    Ok((rows, cm_core::time::now_utc()))
}

fn emit_list(req: &str, p: &Parsed, rows: Vec<cm_core::process::ProcessView>, sampled_at: &str, key: cm_core::process::SortKey, search: Option<&str>, limit: usize) -> ExitCode {
    #[cfg(windows)]
    use cm_platform::windows::processes::{DISK_NOTE, NETWORK_REASON};
    #[cfg(target_os = "macos")]
    use cm_platform::macos::processes::{DISK_NOTE, NETWORK_REASON};
    let body = cm_app::processes::list_wire(rows, key, search, limit, sampled_at, WINDOW_MS, &[DISK_NOTE, NETWORK_REASON]);
    let rec = cm_core::protocol::record("process_list", req, crate::commands::fields(body));
    match p.format {
        // People read one line per process with its start time in UTC (never the platform's raw start key).
        crate::Format::Text => {
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            let _ = write!(out, "{}", text_list(&rec));
            let _ = out.flush();
        }
        crate::Format::Json => crate::emit(&rec, p.format),
    }
    ExitCode::SUCCESS
}

fn mem_text(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let (mut v, mut u) = (n as f64, 0);
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{n} B") } else { format!("{v:.1} {}", UNITS[u]) }
}

/// Human text of a `process_list` record (localized labels; codes, names and times verbatim).
fn text_list(rec: &serde_json::Value) -> String {
    use crate::i18n::t;
    let unknown = t("label.unknown");
    let mut out = format!("{} [process_list]\n", t("kind.process_list"));
    out.push_str(&format!("  sampled_at: {}  window_ms: {}\n", rec["sampled_at"].as_str().unwrap_or(""), rec["window_ms"]));
    let items = rec["items"].as_array().map(Vec::as_slice).unwrap_or_default();
    out.push_str(&format!("  total: {}  matched: {}  {}: {}  {}: {}\n", rec["total"], rec["matched"], t("label.shown"), items.len(), t("label.unidentified"), rec["unidentified"]));
    for i in items {
        let cpu = i["cpu_percent"].as_f64().map_or_else(|| unknown.to_owned(), |c| format!("{c:.1}%"));
        let mem = i["memory_bytes"].as_str().and_then(|m| m.parse::<u64>().ok()).map_or_else(|| unknown.to_owned(), mem_text);
        let started = i["started_at_utc"].as_str().unwrap_or(unknown);
        let sig = &i["signature"];
        let signer = sig["publisher"].as_str().map(|s| format!(" ({s})")).unwrap_or_default();
        let about = match (i["purpose"].as_str(), i["unidentified_reason"].as_str()) {
            (Some(p), _) => p.to_owned(),
            (None, Some(r)) => format!("{}: {r}", t("label.unidentified")),
            (None, None) => t("label.unidentified").to_owned(),
        };
        out.push_str(&format!(
            "    - {}PID {}  {}  CPU {cpu}  {} {mem}  {} {started}  {} {}{signer}  · {about}\n",
            if i["pinned"].as_bool() == Some(true) { "* " } else { "" },
            i["pid"],
            i["name"].as_str().unwrap_or(""),
            t("label.memory"),
            t("label.started_utc"),
            t("label.signature"),
            sig["state"].as_str().unwrap_or(unknown),
        ));
    }
    for n in rec["notes"].as_array().into_iter().flatten().filter_map(|n| n.as_str()) {
        out.push_str(&format!("  {n}\n"));
    }
    out.push_str(&format!("  ({})\n", t("label.machine_hint")));
    out
}


/// `process list [--by cpu|memory|disk|network|name] [--search TEXT] [--limit N]`
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

/// `process end PID` on macOS: a `process` plan pending review (nothing is ended here). The plan binds the PID, the
/// start time (microseconds since the epoch), the executable path and its SHA-256.
#[cfg(target_os = "macos")]
pub fn process_end(req: &str, p: &Parsed, pid: &str) -> ExitCode {
    use cm_platform::macos::processes::{file_sha256, view};
    let Ok(pid) = pid.parse::<u32>() else { return fail(req, ErrorCode::InvalidArgument, "PID must be a number", p.format) };
    let stores = match crate::commands::open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let row = match view(pid) {
        Ok(Some(r)) => r,
        Ok(None) => return fail(req, ErrorCode::InvalidArgument, &format!("no running process {pid}"), p.format),
        Err(e) => return fail(req, ErrorCode::IoError, &e, p.format),
    };
    let plan_id = format!("pp_{}", &req[4..]);
    let pdigest = crate::plan_cmd::current_policy_digest(&stores);
    let digest = row.facts.image_path.as_ref().and_then(file_sha256);
    let plan = match cm_app::processes::end_plan(&row, digest, &plan_id, crate::plan_cmd::now_unix(), &pdigest) {
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

/// Seam for `plan apply` (plan/v2 backends): `process_end` re-identification (pid + start time + executable) on macOS.
#[cfg(target_os = "macos")]
pub(crate) fn mac_processes() -> Option<Box<dyn cm_core::process::ProcessActions>> {
    Some(Box::new(cm_platform::macos::processes::MacProcesses))
}

#[cfg(test)]
mod tests {
    /// WFA-93: people see the start time as UTC, never the platform's raw start key.
    #[test]
    fn text_shows_utc_start_times_not_raw_keys() {
        let rec = serde_json::json!({
            "kind": "process_list", "sampled_at": "2026-10-22T09:55:24Z", "window_ms": 1000, "total": 2, "matched": 1, "unidentified": 1, "notes": ["a note"],
            "items": [{"pid": 26468, "name": "PING.EXE", "started_at": "134371365203426535", "started_at_utc": "2026-10-22T09:55:20Z", "cpu_percent": 0.0,
                "memory_bytes": "1048576", "signature": {"state": "catalog_signed", "publisher": "Microsoft Windows"}, "purpose": null,
                "unidentified_reason": "Named like a known Windows process but runs from an unexpected location", "pinned": true}],
        });
        let text = super::text_list(&rec);
        assert!(text.contains("2026-10-22T09:55:20Z") && !text.contains("134371365203426535"), "{text}");
        assert!(text.contains("PID 26468") && text.contains("PING.EXE") && text.contains("catalog_signed (Microsoft Windows)") && text.contains("1.0 MiB"), "{text}");
        assert!(text.contains("* PID") && text.contains("a note"), "{text}");
    }
}
