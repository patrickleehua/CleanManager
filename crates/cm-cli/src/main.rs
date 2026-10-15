//! `cleanmanager` command-line entry point. Runs without any GUI process (CLI-01).

mod analysis_cmd;
mod apply_cmd;
mod apps_cmd;
mod args;
mod clean_cmd;
mod commands;
mod completion_cmd;
mod diag_cmd;
mod doctor_cmd;
mod i18n;
mod insights_cmd;
mod install_self;
mod installer_cmd;
mod instance_lock;
mod optimize_cmd;
mod plan_cmd;
mod process_cmd;
mod protect_cmd;
mod purge_cmd;
mod recovery_cmd;
mod release;
mod scan_cmd;
mod self_cmd;
mod startup_cmd;
mod status_cmd;
mod trash_cmd;
mod uninstall_cmd;
mod uninstall_self;
mod update_self;
mod updates_cmd;

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
    match format {
        // UTF-8 without BOM, one compact record per line, nothing else on stdout. Machine records
        // never depend on the human language (LIF-05).
        Format::Json => {
            let _ = writeln!(out, "{}", serde_json::to_string(value).expect("records serialize"));
        }
        // Human text: localized titles/explanations; codes, states and IDs verbatim.
        Format::Text => {
            let _ = write!(out, "{}", i18n::render(value));
        }
    }
    let _ = out.flush();
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
    // The short-lived maintenance worker (parity 13.1): a hidden first argument, never a user command or table row.
    if raw.first().is_some_and(|a| a == cm_core::maintenance::WORKER_SUBCOMMAND) {
        return optimize_cmd::worker_main(&raw[1..]);
    }
    let req = request_id();
    let fallback = if std::io::stdout().is_terminal() { Format::Text } else { Format::Json };
    match args::parse(&raw) {
        Ok(parsed) => {
            i18n::set(parsed.lang);
            commands::run(&req, parsed)
        }
        Err(msg) => fail(&req, ErrorCode::InvalidArgument, &msg, fallback),
    }
}
