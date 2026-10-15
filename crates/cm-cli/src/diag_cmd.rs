//! Optional AI diagnosis CLI (AID-11, task 15.8): `diag config|preview|create|show|list`.
//!
//! Read-only with respect to user files. Records use the separate `diag/1` contract (errors use
//! the v1 `error` record). Nothing here waits for input: a missing provider, key or scan fails
//! at once with `unsupported`/`invalid_argument`. `create` sends only a payload whose digest the
//! caller took from `preview` (`--payload-digest`); it never approves anything, never creates a
//! plan and never prints shell cleanup commands. Provider keys are configured in the desktop app
//! (system credential store); the CLI only reads them for the confirmed request.

use std::process::ExitCode;

use cm_app::diag::{DiagAppError, PrepareRequest, ScanContext, Service, current_observation};
use cm_core::diag::{DiagState, Diagnosis, Disclosure, FailureCode, Origin, record};
use cm_core::error::{ErrorCode, ExitClass};
use cm_core::protocol::{ErrorDetails, error_record};
use serde_json::json;

use crate::args::Parsed;
use crate::commands::{fields, open_stores, store_dir};
use crate::{emit, fail};

const CONFIGURE: &str = "Configure a provider in the desktop app (Settings > AI 诊断); local scan, rules, plans and recovery work without it.";

fn service(req: &str, p: &Parsed) -> Result<Service, ExitCode> {
    let Some(dir) = store_dir(p) else {
        return Err(fail(req, ErrorCode::InvalidArgument, "no store directory; pass --store or set CM_STORE_DIR", p.format));
    };
    Ok(Service::new(&dir, cm_platform::credentials::os_store()))
}

fn diag_error(req: &str, e: &DiagAppError, p: &Parsed) -> ExitCode {
    let action = matches!(e, DiagAppError::NotConfigured(_)).then_some(CONFIGURE);
    emit(&error_record(req, e.code(), &e.to_string(), action, ErrorDetails::default()), p.format);
    ExitCode::from(ExitClass::Error(e.code()).code() as u8)
}

fn prepare_request(p: &Parsed) -> Result<PrepareRequest, String> {
    let scan_id = p.option("scan").ok_or("--scan SCAN_ID is required")?.to_owned();
    let entry_id: u64 = p.option("entry").ok_or("--entry ENTRY_ID is required")?.parse().map_err(|_| "--entry must be a numeric entry id")?;
    let origin = match p.option("origin").unwrap_or("disk") {
        "disk" => Origin::Disk,
        "cleanup" => Origin::Cleanup,
        "apps" => Origin::Apps,
        other => return Err(format!("--origin must be disk, cleanup or apps, not {other}")),
    };
    let mut disclosure = Disclosure::default();
    if let Some(list) = p.option("disclose") {
        for item in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match item {
                "full_path" => disclosure.full_path = true,
                "child_summary" => disclosure.child_summary = true,
                other => return Err(format!("--disclose accepts full_path and child_summary, not {other}")),
            }
        }
    }
    Ok(PrepareRequest {
        scan_id,
        entry_id,
        origin,
        provider_id: p.option("provider").map(str::to_owned),
        disclosure,
        question: p.option("question").map(str::to_owned),
        statements: vec![],
        parent_diagnosis_id: None,
    })
}

pub fn diag_config(req: &str, p: &Parsed) -> ExitCode {
    let svc = match service(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    match svc.status() {
        Ok(st) => {
            emit(&record("diagnosis_config", req, fields(serde_json::to_value(&st).expect("serializable"))), p.format);
            ExitCode::SUCCESS
        }
        Err(e) => diag_error(req, &e, p),
    }
}

/// Shared by `preview` and `create`: configuration first (fails fast), then the scan.
fn preview(req: &str, p: &Parsed) -> Result<(Service, cm_core::diag::Preview), ExitCode> {
    let r = prepare_request(p).map_err(|m| fail(req, ErrorCode::InvalidArgument, &m, p.format))?;
    let svc = service(req, p)?;
    svc.provider(r.provider_id.as_deref()).map_err(|e| diag_error(req, &e, p))?;
    let stores = open_stores(req, p)?;
    let ctx = ScanContext::load(&stores, &r.scan_id).map_err(|e| diag_error(req, &e, p))?;
    let pv = svc.prepare(&ctx, r).map_err(|e| diag_error(req, &e, p))?;
    Ok((svc, pv))
}

pub fn diag_preview(req: &str, p: &Parsed) -> ExitCode {
    match preview(req, p) {
        Ok((svc, pv)) => {
            svc.discard(&pv.preview_id);
            emit(&record("diagnosis_preview", req, fields(json!({"preview": pv}))), p.format);
            ExitCode::SUCCESS
        }
        Err(c) => c,
    }
}

fn exit_for(d: &Diagnosis) -> ExitCode {
    let class = match (d.state, d.failure.as_ref().map(|f| f.code)) {
        (DiagState::Completed, _) => ExitClass::Success,
        (DiagState::Cancelled, _) => ExitClass::Error(ErrorCode::Cancelled),
        (_, Some(FailureCode::AuthFailed)) => ExitClass::Error(ErrorCode::PermissionRequired),
        (_, Some(FailureCode::NotConfigured | FailureCode::CredentialMissing | FailureCode::CredentialStoreUnavailable | FailureCode::UnsupportedMode)) => {
            ExitClass::Error(ErrorCode::Unsupported)
        }
        _ => ExitClass::Error(ErrorCode::IoError),
    };
    ExitCode::from(class.code() as u8)
}

pub fn diag_create(req: &str, p: &Parsed) -> ExitCode {
    let Some(digest) = p.option("payload-digest").map(str::to_owned) else {
        return fail(req, ErrorCode::InvalidArgument, "--payload-digest is required: run `diag preview` with the same options, review the payload, then pass its payload_digest", p.format);
    };
    let (svc, pv) = match preview(req, p) {
        Ok(x) => x,
        Err(c) => return c,
    };
    if pv.payload_digest != digest {
        svc.discard(&pv.preview_id);
        return fail(req, ErrorCode::Conflict, "the payload differs from the reviewed preview (scan, options, provider or CLI changed); preview again", p.format);
    }
    let d = match svc.confirm(&pv.preview_id, &digest).and_then(|d| svc.execute(&d.diagnosis_id)) {
        Ok(d) => d,
        Err(e) => return diag_error(req, &e, p),
    };
    emit(&record("diagnosis_record", req, fields(json!({"diagnosis": d}))), p.format);
    exit_for(&d)
}

pub fn diag_show(req: &str, p: &Parsed, id: &str) -> ExitCode {
    let svc = match service(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let current = match open_stores(req, p) {
        Ok(stores) => match svc.get(id, cm_core::diag::ground::CurrentObservation::NotChecked) {
            Ok(d) => current_observation(&stores, &d),
            Err(e) => return diag_error(req, &e, p),
        },
        Err(c) => return c,
    };
    match svc.get(id, current) {
        Ok(d) => {
            emit(&record("diagnosis_record", req, fields(json!({"diagnosis": d}))), p.format);
            ExitCode::SUCCESS
        }
        Err(e) => diag_error(req, &e, p),
    }
}

pub fn diag_list(req: &str, p: &Parsed) -> ExitCode {
    let svc = match service(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let limit = match p.option("limit").map(str::parse::<usize>).transpose() {
        Ok(l) => l.unwrap_or(50),
        Err(_) => return fail(req, ErrorCode::InvalidArgument, "--limit must be a number 1..=100", p.format),
    };
    match svc.list(limit, p.option("cursor")) {
        Ok((items, next)) => {
            emit(&record("diagnosis_list", req, fields(json!({"items": items, "next_cursor": next}))), p.format);
            ExitCode::SUCCESS
        }
        Err(e) => diag_error(req, &e, p),
    }
}
