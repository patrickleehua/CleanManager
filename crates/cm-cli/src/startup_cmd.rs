//! `startup list` (parity STA-01, task 11.1): read-only startup inventory. Nothing is enabled, disabled or removed;
//! toggling is task 11.2 and every item reports it as not available.

use std::process::ExitCode;

use crate::args::Parsed;

#[cfg(windows)]
pub fn startup_list(req: &str, p: &Parsed) -> ExitCode {
    use cm_core::error::ExitClass;
    use cm_core::protocol::record;
    use serde_json::json;

    let (items, issues) = cm_platform::windows::startup::inventory();
    let items: Vec<_> = items.iter().map(|i| i.to_wire()).collect();
    let issue_records: Vec<_> = issues.iter().map(|i| json!({"code": i.code, "scope_display": i.source, "message": i.message})).collect();
    crate::emit(&record("startup_list", req, crate::commands::fields(json!({"items": items, "issues": issue_records}))), p.format);
    // A source that could not be read makes the list incomplete: partial, not success (CLI-05).
    ExitCode::from(if issues.is_empty() { ExitClass::Success } else { ExitClass::Partial }.code() as u8)
}

#[cfg(not(windows))]
pub fn startup_list(req: &str, p: &Parsed) -> ExitCode {
    crate::fail(req, cm_core::error::ErrorCode::Unsupported, "startup inventory is not implemented on this platform yet", p.format)
}
