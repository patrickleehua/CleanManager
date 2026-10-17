//! `apps updates`, `apps updates show|config`, `apps update` (parity UPD-01..04, tasks 10.1-10.3).
//!
//! - `apps updates` is the user-triggered check: winget is asked (`winget list`), Scoop is read from its local
//!   directory, self-updating apps are recognized from their files. With checks turned off (`apps updates config
//!   --checks off`) nothing is run or sent and the last results are shown with their original time.
//! - `apps updates show` never checks: cached results only.
//! - `apps update --app ID[,ID...]` creates one `update` plan pending review for the verifiable package-manager updates
//!   and lists where the others go (Store, the app itself, the official page). `plan review` approves it; `plan apply`
//!   verifies each downloaded package's signer against the installed app before upgrading, item by item.
//! - macOS: `apps updates` asks Homebrew (`brew outdated --json=v2`, auto-update off) and fetches each https Sparkle
//!   feed; App Store and Electron apps are reported from local files. `apps update` plans only Homebrew upgrades
//!   (`brew upgrade --cask|--formula`); App Store and Sparkle/Electron apps are routed to the store or the app.

use std::process::ExitCode;

use cm_core::error::ErrorCode;
use serde_json::json;

use crate::args::Parsed;
use crate::fail;

fn store(req: &str, p: &Parsed) -> Result<std::path::PathBuf, ExitCode> {
    crate::commands::store_dir(p).ok_or_else(|| fail(req, ErrorCode::InvalidArgument, "no store directory; pass --store or set CM_STORE_DIR", p.format))
}

#[cfg(any(windows, target_os = "macos"))]
fn emit_list(req: &str, p: &Parsed, r: &cm_app::updates::CheckReport) -> ExitCode {
    crate::emit(&cm_core::protocol::record("app_update_list", req, crate::commands::fields(r.to_wire())), p.format);
    ExitCode::SUCCESS
}

/// `apps updates`: check now (when enabled).
#[cfg(windows)]
pub fn updates_check(req: &str, p: &Parsed) -> ExitCode {
    let dir = match store(req, p) {
        Ok(d) => d,
        Err(c) => return c,
    };
    let apps = cm_app::updates::windows_apps();
    let ch = cm_app::updates::WindowsChannels { winget: cm_platform::windows::updates::winget_exe(&apps) };
    match cm_app::updates::check(&dir, &apps, &ch, &cm_core::time::now_utc()) {
        Ok(r) => emit_list(req, p, &r),
        Err(e) => fail(req, ErrorCode::IoError, &e.to_string(), p.format),
    }
}

/// `apps updates show`: cached results only, never a check.
#[cfg(windows)]
pub fn updates_show(req: &str, p: &Parsed) -> ExitCode {
    match store(req, p) {
        Ok(dir) => emit_list(req, p, &cm_app::updates::cached(&dir, &cm_app::updates::windows_apps())),
        Err(c) => c,
    }
}

/// The inventory locations (`CM_UNINSTALL_TEST_ROOT`/`HOME` redirect them in tests).
#[cfg(target_os = "macos")]
fn mac_env(req: &str, p: &Parsed) -> Result<cm_platform::macos::apps::AppsEnv, ExitCode> {
    cm_platform::macos::apps::AppsEnv::from_process().map_err(|e| fail(req, ErrorCode::InvalidArgument, &e, p.format))
}

/// `apps updates` on macOS: check now (when enabled).
#[cfg(target_os = "macos")]
pub fn updates_check(req: &str, p: &Parsed) -> ExitCode {
    let (dir, env) = match (store(req, p), mac_env(req, p)) {
        (Ok(d), Ok(e)) => (d, e),
        (Err(c), _) | (_, Err(c)) => return c,
    };
    let apps = cm_app::updates::mac_apps(&env);
    let ch = cm_app::updates::MacChannels::new(env);
    match cm_app::updates::check(&dir, &apps, &ch, &cm_core::time::now_utc()) {
        Ok(r) => emit_list(req, p, &r),
        Err(e) => fail(req, ErrorCode::IoError, &e.to_string(), p.format),
    }
}

/// `apps updates show` on macOS: cached results only, never a check.
#[cfg(target_os = "macos")]
pub fn updates_show(req: &str, p: &Parsed) -> ExitCode {
    match (store(req, p), mac_env(req, p)) {
        (Ok(dir), Ok(env)) => emit_list(req, p, &cm_app::updates::cached(&dir, &cm_app::updates::mac_apps(&env))),
        (Err(c), _) | (_, Err(c)) => c,
    }
}

/// `apps updates config [--checks on|off]`.
pub fn updates_config(req: &str, p: &Parsed) -> ExitCode {
    let dir = match store(req, p) {
        Ok(d) => d,
        Err(c) => return c,
    };
    let settings = match p.option("checks") {
        None => cm_app::updates::load(&dir).settings,
        Some(v @ ("on" | "off")) => match cm_app::updates::set_checks(&dir, v == "on") {
            Ok(s) => s,
            Err(e) => return fail(req, ErrorCode::IoError, &e.to_string(), p.format),
        },
        Some(_) => return fail(req, ErrorCode::InvalidArgument, "--checks must be on or off", p.format),
    };
    let s = cm_app::updates::load(&dir);
    let body = json!({"checks_enabled": settings.checks_enabled, "periodic_checks": false, "last_checked_at": s.last_checked_at,
        "note": "Checks run only when you ask (apps updates); off means no update request of any kind is made."});
    crate::emit(&cm_core::protocol::record("app_update_settings", req, crate::commands::fields(body)), p.format);
    ExitCode::SUCCESS
}

/// `apps update --app ID[,ID...]`: one `update` plan pending review, plus where the other apps go.
#[cfg(any(windows, target_os = "macos"))]
pub fn update_plan(req: &str, p: &Parsed) -> ExitCode {
    let Some(list) = p.option("app") else { return fail(req, ErrorCode::InvalidArgument, "apps update needs --app ID[,ID...] from apps updates", p.format) };
    let ids: Vec<String> = list.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect();
    if ids.is_empty() || ids.len() > 50 {
        return fail(req, ErrorCode::InvalidArgument, "--app takes 1..=50 app instance IDs", p.format);
    }
    let stores = match crate::commands::open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let cache = cm_app::updates::load(&stores.dir);
    if cache.results.is_empty() {
        return fail(req, ErrorCode::InvalidArgument, "no update check results yet; run apps updates first", p.format);
    }
    #[cfg(windows)]
    let (apps, winget, scoop, homebrew, signer) = {
        let apps = cm_app::updates::windows_apps();
        let (winget, scoop) = cm_app::updates::windows_tools(&apps);
        (apps, winget, scoop, None, cm_platform::windows::updates::installed_signer)
    };
    #[cfg(target_os = "macos")]
    let (apps, winget, scoop, homebrew, signer) = {
        let env = match mac_env(req, p) {
            Ok(e) => e,
            Err(c) => return c,
        };
        (cm_app::updates::mac_apps(&env), None, None, cm_app::updates::mac_brew_tool(&env), cm_platform::macos::updates::installed_signer)
    };
    let inputs = cm_app::updates::PlanInputs { results: &cache.results, apps: &apps, ids: &ids, winget, scoop, homebrew, installed_signer: &signer };
    let plan_id = format!("up_{}", &req[4..]);
    let pdigest = crate::plan_cmd::current_policy_digest(&stores);
    let (plan, routed) = match cm_app::updates::plan_updates(&inputs, &plan_id, crate::plan_cmd::now_unix(), &pdigest) {
        Ok(r) => r,
        Err(e @ cm_app::updates::UpdatePlanError::UnknownApp(_)) => return fail(req, ErrorCode::InvalidArgument, &e.to_string(), p.format),
        Err(e) => return fail(req, ErrorCode::InternalError, &e.to_string(), p.format),
    };
    if let Some(pl) = &plan
        && let Err(e) = stores.audit.save_plan_v2(pl)
    {
        return crate::commands::store_error(req, &e, p.format);
    }
    let plan_rec = plan.as_ref().map(|pl| {
        let mut r = crate::plan_cmd::v2_plan_record(req, &stores, pl);
        for k in ["schema_version", "kind", "request_id"] {
            r.as_object_mut().map(|o| o.remove(k));
        }
        r
    });
    let body = json!({"plan": plan_rec, "routed": routed,
        "next": plan.as_ref().map(|pl| format!("cleanmanager plan review {} (in a terminal), then cleanmanager plan apply {}", pl.body.plan_id, pl.body.plan_id))});
    crate::emit(&cm_core::protocol::record("app_update_plan", req, crate::commands::fields(body)), p.format);
    ExitCode::SUCCESS
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn updates_check(req: &str, p: &Parsed) -> ExitCode {
    fail(req, ErrorCode::Unsupported, "app update checks are not implemented on this platform yet", p.format)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn updates_show(req: &str, p: &Parsed) -> ExitCode {
    fail(req, ErrorCode::Unsupported, "app update checks are not implemented on this platform yet", p.format)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn update_plan(req: &str, p: &Parsed) -> ExitCode {
    fail(req, ErrorCode::Unsupported, "app updates are not implemented on this platform yet", p.format)
}

/// Seam for `plan apply` (plan/v2 backends): the `update` plans' channel steps on macOS (only Homebrew's fixed
/// `brew upgrade` runs). `None` when the inventory redirection is misconfigured.
#[cfg(target_os = "macos")]
pub(crate) fn mac_update_backend() -> Option<Box<dyn cm_core::updates::UpdateBackend>> {
    cm_platform::macos::updates::MacUpdates::from_process().ok().map(|b| Box::new(b) as Box<dyn cm_core::updates::UpdateBackend>)
}
