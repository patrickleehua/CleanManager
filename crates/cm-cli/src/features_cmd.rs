//! `experimental [--state on|off]` and the release-gate check every gated command passes first (design D15).
//!
//! A capability whose gate has not passed is off: its commands, and `plan review`/`plan apply` of a stored plan/v2
//! of its kind, refuse with one `unsupported` error record before doing any work. The switch is a local setting in
//! the shared store (`cm_app::features`), read the same way by the desktop app. MVP core commands are not gated.

use std::process::ExitCode;

use cm_core::error::{ErrorCode, ExitClass};
use cm_core::protocol::{ErrorDetails, error_record, record};
use serde_json::json;

use crate::args::Parsed;
use crate::commands::{fields, store_dir};
use crate::{emit, fail};

/// The gated capability a command belongs to (command words only, positional value excluded).
pub fn capability_for(words: &[&str]) -> Option<&'static str> {
    Some(match words {
        ["clean", ..] => "clean.catalog",
        ["purge", ..] => "purge.artifacts",
        ["installer", ..] => "installer.cleanup",
        ["trash", ..] => "trash.manual",
        ["insights", ..] => "insights.overview",
        ["apps", "uninstall", ..] => "apps.uninstall",
        ["apps", "updates" | "update", ..] => "apps.updates",
        ["startup", ..] => "startup.toggle",
        ["optimize", ..] => "maintenance.run",
        ["status", ..] => "status.metrics",
        ["process", ..] => "process.end",
        ["self", "update"] => "lifecycle.self_update",
        ["diag", ..] => "diag.ai",
        _ => return None,
    })
}

fn refuse(req: &str, p: &Parsed, off: cm_app::features::Disabled, plan_id: Option<&str>) -> ExitCode {
    let details = ErrorDetails { plan_id: plan_id.map(str::to_owned), ..ErrorDetails::default() };
    emit(&error_record(req, ErrorCode::Unsupported, &off.to_string(), Some(cm_app::features::OPT_IN_COMMAND), details), p.format);
    ExitCode::from(ExitClass::Error(ErrorCode::Unsupported).code() as u8)
}

/// Refuse a gated command while its capability is off. `plan review|apply ID` are checked by the stored plan's kind.
pub fn guard(req: &str, p: &Parsed) -> Result<(), ExitCode> {
    let words: Vec<&str> = p.words.iter().map(String::as_str).collect();
    let dir = store_dir(p);
    if let ["plan", "review" | "apply", id] = words.as_slice() {
        // An unopenable store or an unknown plan is reported by the command itself.
        let audit = dir.as_deref().map(|d| d.join(cm_store::AUDIT_FILE)).filter(|f| f.is_file());
        let kind = audit.and_then(|f| cm_store::AuditStore::open(&f).ok()).and_then(|a| a.load_plan_v2(id).ok()).map(|pl| pl.body.plan_kind);
        if let Some(cap) = kind.and_then(cm_app::features::plan_kind_capability) {
            return cm_app::features::check(dir.as_deref(), cap).map_err(|off| refuse(req, p, off, Some(id)));
        }
        return Ok(());
    }
    let head = match crate::args::spec_for(&words) {
        Some(s) => s.words,
        None => return Ok(()),
    };
    match capability_for(head) {
        Some(cap) => cm_app::features::check(dir.as_deref(), cap).map_err(|off| refuse(req, p, off, None)),
        None => Ok(()),
    }
}

/// `experimental`: show the switch and each gated capability; `--state on|off` changes this local setting only.
pub fn experimental(req: &str, p: &Parsed) -> ExitCode {
    let Some(dir) = store_dir(p) else {
        return fail(req, ErrorCode::InvalidArgument, "no store directory; pass --store or set CM_STORE_DIR", p.format);
    };
    match p.option("state") {
        None => {}
        Some(v @ ("on" | "off")) => {
            if let Err(e) = cm_app::features::set_experimental(&dir, v == "on") {
                return fail(req, ErrorCode::IoError, &e.to_string(), p.format);
            }
        }
        Some(_) => return fail(req, ErrorCode::InvalidArgument, "--state must be on or off", p.format),
    }
    let s = cm_app::features::load(Some(&dir));
    let body = json!({
        "experimental_features": s.experimental_features,
        "capabilities": cm_app::features::rows(Some(&dir)),
        "note": "Capabilities whose release gate has not passed are off unless experimental features are on. They are for evaluation on this machine only; turning this on passes no gate and changes no plan review.",
    });
    emit(&record("experimental_features", req, fields(body)), p.format);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_gated_command_maps_to_a_known_capability_and_core_commands_do_not() {
        for spec in crate::args::COMMANDS {
            if let Some(cap) = capability_for(spec.words) {
                assert!(cm_core::capability::gate_of(cap).is_some(), "{:?} -> {cap}", spec.words);
            }
        }
        for core in [
            &["scan"][..],
            &["analyze"],
            &["candidates", "list"],
            &["plan", "create"],
            &["plan", "apply"],
            &["operations", "list"],
            &["recovery", "list"],
            &["quarantine", "list"],
            &["capabilities"],
            &["doctor"],
            &["completion", "print"],
            &["self", "uninstall"],
            &["self", "install"],
            &["apps", "list"],
            &["protect", "list"],
            &["policy", "list"],
            &["experimental"],
            &["help"],
        ] {
            assert_eq!(capability_for(core), None, "{core:?}");
        }
        assert_eq!(capability_for(&["apps", "uninstall", "leftovers", "plan", "create"]), Some("apps.uninstall"));
        assert_eq!(capability_for(&["process", "end"]), Some("process.end"));
        assert_eq!(capability_for(&["self", "update"]), Some("lifecycle.self_update"));
    }
}
