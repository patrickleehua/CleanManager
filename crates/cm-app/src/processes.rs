//! Process list services shared by the CLI and the desktop (parity 12.3, STS-04).
//!
//! - [`list_wire`] turns explained rows into the `process_list` record: search, sort and pins applied, pinned rows
//!   first. Pins are names kept in `<store>/process-pins.json` (this tool's own setting).
//! - [`end_plan`] freezes one process into a `process` plan; only an interactive review authorizes it and the
//!   platform re-identifies the process at execution ([`run_end`]).

use std::path::Path;

use cm_core::plan::v2::{ActionMeta, BuildError, HeaderInput, NO_RULES_DIGEST, PlanAction, PlanKind, PlanV2, ProcessEndAction, build};
use cm_core::process::{EndOutcome, EndRefusal, ProcessActions, ProcessView, SortKey, end_action, matches, sort};
use cm_core::execute::Reconciled;

use crate::execute::{ActionResult, ActionStatus};

pub const PINS_FILE: &str = "process-pins.json";

/// Pinned process names (case-insensitive); missing or unreadable file = none.
pub fn load_pins(store: &Path) -> Vec<String> {
    std::fs::read_to_string(store.join(PINS_FILE)).ok().and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok()).unwrap_or_default()
}

/// Pin or unpin a process name; returns the new list.
pub fn set_pin(store: &Path, name: &str, pinned: bool) -> std::io::Result<Vec<String>> {
    let name = name.trim();
    if name.is_empty() || name.len() > 260 || name.contains(['\\', '/']) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "a pin is a process name such as code.exe"));
    }
    let mut pins = load_pins(store);
    pins.retain(|p| !p.eq_ignore_ascii_case(name));
    if pinned {
        pins.push(name.to_owned());
    }
    pins.sort_by_key(|p| p.to_lowercase());
    std::fs::create_dir_all(store)?;
    let tmp = store.join(format!("{PINS_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&pins).expect("pins serialize"))?;
    std::fs::rename(&tmp, store.join(PINS_FILE))?;
    Ok(pins)
}

/// The `process_list` record body: rows filtered by `search`, sorted by `key` (pinned first), at most `limit`.
pub fn list_wire(mut rows: Vec<ProcessView>, key: SortKey, search: Option<&str>, limit: usize, sampled_at: &str, window_ms: u64, notes: &[&str]) -> serde_json::Value {
    let total = rows.len();
    rows.retain(|r| search.is_none_or(|q| matches(r, q)));
    sort(&mut rows, key);
    let matched = rows.len();
    rows.truncate(limit);
    let unidentified = rows.iter().filter(|r| !r.identified()).count();
    serde_json::json!({
        "sampled_at": sampled_at,
        "window_ms": window_ms,
        "sort": key,
        "search": search,
        "total": total,
        "matched": matched,
        "unidentified": unidentified,
        "items": rows.iter().map(ProcessView::to_wire).collect::<Vec<_>>(),
        "notes": notes,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum EndPlanError {
    #[error(transparent)]
    Refused(#[from] EndRefusal),
    #[error(transparent)]
    Build(#[from] BuildError),
}

/// A `process` plan ending exactly this process (one action; never policy-matchable).
pub fn end_plan(row: &ProcessView, image_sha256: Option<String>, plan_id: &str, now_unix: i64, policy_digest: &str) -> Result<PlanV2, EndPlanError> {
    let action = end_action(row, image_sha256)?;
    let header = HeaderInput {
        plan_id,
        plan_kind: PlanKind::Process,
        created_unix: now_unix,
        scope_id: None,
        scan_id: None,
        scan_revision: None,
        rule_set_digest: NO_RULES_DIGEST,
        policy_digest,
    };
    // The review shows the start time people read (UTC); the action binds the platform's exact start key.
    let note = format!(
        "{} (PID {}{}){}",
        row.facts.name,
        row.facts.pid,
        row.facts.started_at_utc.as_deref().map(|t| format!(", started {t}")).unwrap_or_default(),
        row.purpose.map(|p| format!(": {p}")).or_else(|| row.unidentified_reason.as_ref().map(|r| format!(": unidentified, {r}"))).unwrap_or_default()
    );
    Ok(build(&header, vec![PlanAction::ProcessEnd(action)], vec![ActionMeta { note: Some(note), ..Default::default() }])?)
}

fn res(id: &str, status: ActionStatus, code: &str, message: impl Into<String>) -> ActionResult {
    ActionResult { action_id: id.into(), status, code: code.into(), message: message.into(), recovery: "not_applicable".into() }
}

/// Execute one `process_end` action (called by the plan executor after authorization).
pub fn run_end(b: Option<&dyn ProcessActions>, a: &ProcessEndAction) -> ActionResult {
    let id = &a.action_id;
    let Some(b) = b else { return res(id, ActionStatus::Skipped, "unsupported", "ending processes is not available on this platform") };
    match b.end(a, std::process::id()) {
        EndOutcome::Ended => res(id, ActionStatus::Succeeded, "ended", format!("{} (PID {}) was ended; unsaved work in it is lost", a.name, a.pid)),
        EndOutcome::AlreadyExited => res(id, ActionStatus::Skipped, "precondition_gone", format!("{} (PID {}) had already exited; nothing was ended", a.name, a.pid)),
        EndOutcome::Changed(why) => res(id, ActionStatus::Skipped, "precondition_changed", format!("{why}; nothing was ended")),
        EndOutcome::Protected(p) => res(id, ActionStatus::Skipped, "protected", p.reason()),
        EndOutcome::PermissionRequired(why) => res(id, ActionStatus::Skipped, "permission_required", why),
        EndOutcome::Failed(why) => res(id, ActionStatus::Failed, "end_failed", why),
        EndOutcome::Unknown(why) => res(id, ActionStatus::Unknown, "outcome_unconfirmed", why),
    }
}

/// Reconciliation of an interrupted `process_end`: still running = not performed, gone = performed.
pub fn reconcile_end(b: Option<&dyn ProcessActions>, a: &ProcessEndAction) -> Reconciled {
    match b.and_then(|b| b.running(a)) {
        Some(true) => Reconciled::NotPerformed,
        Some(false) => Reconciled::Performed,
        None => Reconciled::StillUnknown,
    }
}

pub const ALERTS_FILE: &str = "status-alerts.json";

/// Sustained-CPU alert settings (STS-05); missing or unreadable = the opt-in default (off).
pub fn load_alerts(store: &Path) -> cm_core::alerts::AlertSettings {
    std::fs::read_to_string(store.join(ALERTS_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save_alerts(store: &Path, s: &cm_core::alerts::AlertSettings) -> Result<(), String> {
    s.validate()?;
    std::fs::create_dir_all(store).map_err(|e| e.to_string())?;
    let tmp = store.join(format!("{ALERTS_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(s).expect("settings serialize")).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, store.join(ALERTS_FILE)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
