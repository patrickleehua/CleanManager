//! `insights` (parity DIN-01..04, tasks 7.1/7.3).
//!
//! - `insights`: known large system and virtualization objects with explanations and the official tool that manages
//!   each, as guidance text. No insight has a trash or delete entry and no tool is run.
//! - `insights overview`: overview cards (old downloads, virtual machines, restore points, system files, tool caches,
//!   model libraries, device backups), each with a known size or unknown and the time it was observed.
//! - `insights downloads [--older-than-days N] [--limit N]`: the old-downloads review, largest first, nothing selected;
//!   files are handled only through `trash` (manual selection) or the installer review.

use std::process::ExitCode;

use cm_core::error::ErrorCode;

use crate::args::Parsed;

#[cfg(any(windows, target_os = "macos"))]
pub fn insights_list(req: &str, p: &Parsed) -> ExitCode {
    use cm_core::protocol::record;
    use serde_json::json;

    #[cfg(windows)]
    let known = cm_platform::windows::insights::list();
    #[cfg(target_os = "macos")]
    let known = cm_platform::macos::insights::list();
    let items: Vec<_> = known.iter().map(|i| i.to_wire()).collect();
    crate::emit(&record("insight_list", req, crate::commands::fields(json!({"items": items}))), p.format);
    ExitCode::SUCCESS
}

fn days(req: &str, p: &Parsed) -> Result<u32, ExitCode> {
    match p.option("older-than-days").map(str::parse::<u32>) {
        None => Ok(cm_app::insights::DEFAULT_DAYS),
        Some(Ok(n)) if (1..=3650).contains(&n) => Ok(n),
        Some(_) => Err(crate::fail(req, ErrorCode::InvalidArgument, "--older-than-days must be 1..=3650", p.format)),
    }
}

#[cfg(any(windows, target_os = "macos"))]
pub fn insights_overview(req: &str, p: &Parsed) -> ExitCode {
    let d = match days(req, p) {
        Ok(d) => d,
        Err(c) => return c,
    };
    let body = cm_app::insights::overview(d);
    crate::emit(&cm_core::protocol::record("insight_overview", req, crate::commands::fields(body)), p.format);
    ExitCode::SUCCESS
}

#[cfg(any(windows, target_os = "macos"))]
pub fn insights_downloads(req: &str, p: &Parsed) -> ExitCode {
    let d = match days(req, p) {
        Ok(d) => d,
        Err(c) => return c,
    };
    let limit: usize = match p.option("limit").map(str::parse).unwrap_or(Ok(50)) {
        Ok(n) if (1..=500).contains(&n) => n,
        _ => return crate::fail(req, ErrorCode::InvalidArgument, "--limit must be 1..=500", p.format),
    };
    let body = cm_app::insights::old_downloads(d, limit);
    crate::emit(&cm_core::protocol::record("old_downloads_review", req, crate::commands::fields(body)), p.format);
    ExitCode::SUCCESS
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn insights_list(req: &str, p: &Parsed) -> ExitCode {
    crate::fail(req, ErrorCode::Unsupported, "disk insights are not implemented on this platform yet", p.format)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn insights_overview(req: &str, p: &Parsed) -> ExitCode {
    let _ = days;
    crate::fail(req, ErrorCode::Unsupported, "disk insights are not implemented on this platform yet", p.format)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn insights_downloads(req: &str, p: &Parsed) -> ExitCode {
    crate::fail(req, ErrorCode::Unsupported, "disk insights are not implemented on this platform yet", p.format)
}
