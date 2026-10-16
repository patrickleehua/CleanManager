//! `apps list` (APP-01..04): read-only installed-application inventory. No uninstaller is ever
//! invoked; each instance's `uninstall_capability` is the route `apps uninstall list` reports for it (UNI-01):
//! `available`, `unverified` with the reason, or `unsupported` for protected classes.

use std::process::ExitCode;

use cm_core::error::ErrorCode;
use cm_core::protocol::record;
use serde_json::json;

use crate::args::Parsed;
use crate::commands::fields;
use crate::{emit, fail};

pub fn apps_list(req: &str, p: &Parsed) -> ExitCode {
    let limit: usize = match p.option("limit").map(str::parse).unwrap_or(Ok(50)) {
        Ok(n) if (1..=500).contains(&n) => n,
        _ => return fail(req, ErrorCode::InvalidArgument, "--limit must be 1..=500", p.format),
    };
    #[cfg(windows)]
    let (mut apps, _issues) = cm_platform::windows::apps::inventory();
    #[cfg(windows)]
    cm_platform::windows::uninstall::apply_uninstall_capability(&cm_platform::windows::uninstall::UninstallEnv::real(), &mut apps);
    // macOS: read-only bundle inventory; `CM_UNINSTALL_TEST_ROOT` + `CM_UNINSTALL_TEST_HOME` redirect it for tests.
    #[cfg(target_os = "macos")]
    let (mut apps, _issues) = match cm_platform::macos::apps::AppsEnv::from_process() {
        Ok(env) => cm_platform::macos::apps::inventory(&env),
        Err(e) => return fail(req, ErrorCode::InvalidArgument, &e, p.format),
    };
    apps.sort_by(|a, b| a.instance_id.cmp(&b.instance_id));
    // Opaque cursor: the last instance id of the previous page (inventory order is by id).
    let start = match p.option("cursor") {
        None => 0,
        Some(c) => apps.iter().position(|a| a.instance_id.as_str() > c).unwrap_or(apps.len()),
    };
    let end = (start + limit).min(apps.len());
    let items: Vec<_> = apps[start..end].iter().map(|a| a.to_wire()).collect();
    let next = (end < apps.len()).then(|| apps[end - 1].instance_id.clone());
    emit(&record("app_list", req, fields(json!({"items": items, "next_cursor": next, "page_size": limit}))), p.format);
    ExitCode::SUCCESS
}
