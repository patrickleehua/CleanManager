//! `cleanmanager` command-line entry point. Runs without any GUI process (CLI-01).

mod analysis_cmd;
mod apply_cmd;
mod apps_cmd;
mod args;
mod commands;
mod insights_cmd;
mod plan_cmd;
mod protect_cmd;
mod recovery_cmd;
mod startup_cmd;
mod status_cmd;

use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use cm_core::error::{ErrorCode, ExitClass};
use cm_core::protocol::{ErrorDetails, error_record};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Text,
}

pub fn request_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("req_{:x}_{:x}", std::process::id(), nanos)
}

pub fn emit(value: &Value, format: Format) {
    let mut out = std::io::stdout().lock();
    let text = match format {
        Format::Json => serde_json::to_string(value).expect("records serialize"),
        Format::Text => serde_json::to_string_pretty(value).expect("records serialize"),
    };
    // UTF-8 without BOM, one terminal record, nothing else on stdout.
    let _ = writeln!(out, "{text}");
}

pub fn fail(req: &str, code: ErrorCode, message: &str, format: Format) -> ExitCode {
    emit(&error_record(req, code, message, None, ErrorDetails::default()), format);
    ExitCode::from(ExitClass::Error(code).code() as u8)
}

fn main() -> ExitCode {
    // Ordinary-permission context: never inherit an ACL bypass from an elevated token.
    #[cfg(windows)]
    let _ = cm_platform::windows::privileges::disable_bypass_privileges();
    let raw: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let req = request_id();
    let fallback = if std::io::stdout().is_terminal() { Format::Text } else { Format::Json };
    match args::parse(&raw) {
        Ok(parsed) => commands::run(&req, parsed),
        Err(msg) => fail(&req, ErrorCode::InvalidArgument, &msg, fallback),
    }
}
