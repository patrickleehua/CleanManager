//! `insights` (parity DIN-01/02, task 7.1): known large system and virtualization objects with explanations and the
//! official tool that manages each, as guidance text. No insight has a trash or delete entry and no tool is run.

use std::process::ExitCode;

use crate::args::Parsed;

#[cfg(windows)]
pub fn insights_list(req: &str, p: &Parsed) -> ExitCode {
    use cm_core::protocol::record;
    use serde_json::json;

    let items: Vec<_> = cm_platform::windows::insights::list().iter().map(|i| i.to_wire()).collect();
    crate::emit(&record("insight_list", req, crate::commands::fields(json!({"items": items}))), p.format);
    ExitCode::SUCCESS
}

#[cfg(not(windows))]
pub fn insights_list(req: &str, p: &Parsed) -> ExitCode {
    crate::fail(req, cm_core::error::ErrorCode::Unsupported, "disk insights are not implemented on this platform yet", p.format)
}
