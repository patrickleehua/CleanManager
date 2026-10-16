//! Discovery review shared by `purge` and `installer` (Windows and macOS; platform-neutral): the last discovery run
//! per store, preview records and plan creation from the stored candidate sets.

use std::process::ExitCode;

use cm_core::analysis::{Candidate, CandidateSet, CandidateState};
use cm_core::error::ErrorCode;
use cm_core::path::RawPath;
use cm_core::protocol::record;
use cm_core::rules::{Mode, Tier};
use serde_json::{Value, json};

use crate::args::Parsed;
use crate::commands::{fields, open_stores, store_error};
use crate::{emit, fail};

/// Per-store record of the latest discovery run of a command group: which candidate set came
/// from which root, so `plan create` can find a candidate without a set id.
pub(crate) const LAST_RUN_PURGE: &str = "purge-last-run.json";

pub(crate) fn last_run_write(dir: &std::path::Path, file: &str, sets: &[(RawPath, String, String)]) {
    let v = json!({"schema_version": 1, "sets": sets.iter().map(|(root, scan, set)| json!({"root": root, "scan_id": scan, "set_id": set})).collect::<Vec<_>>()});
    let tmp = dir.join(format!("{file}.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, v.to_string()).is_ok() && std::fs::rename(&tmp, dir.join(file)).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

pub(crate) fn last_run_sets(dir: &std::path::Path, file: &str) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(dir.join(file)) else { return vec![] };
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    v["sets"].as_array().map(|a| a.iter().filter_map(|s| s["set_id"].as_str().map(str::to_owned)).collect()).unwrap_or_default()
}

pub(crate) fn plannable(c: &Candidate) -> bool {
    c.state == CandidateState::Allowed && c.tier != Tier::Blocked && !c.allowed_modes.is_empty()
}

pub(crate) fn selected_by_default(c: &Candidate) -> bool {
    plannable(c) && c.tier == Tier::DefaultSelected
}

/// Protocol `preview_item.skip_reason` from analysis reasons (first match by severity).
pub(crate) fn skip_reason(c: &Candidate) -> Option<&'static str> {
    let has = |p: &str| c.reasons.iter().any(|r| r.starts_with(p));
    if c.state == CandidateState::Blocked {
        return Some(if has("hard_deny") {
            "hard_denied"
        } else if has("vcs_tracked") || has("vcs_metadata_inside") {
            "vcs_tracked"
        } else if has("mounted") {
            "mounted"
        } else {
            "protected"
        });
    }
    if c.state != CandidateState::Allowed {
        return Some(if has("vcs_unknown") {
            "vcs_unknown"
        } else if has("mounted_image") {
            "mounted"
        } else if has("in_use:") {
            "in_use"
        } else if has("mounted_state_unknown") || has("in_use_unknown") {
            "probe_unknown"
        } else {
            "partial_evidence"
        });
    }
    has("recent_project").then_some("recent")
}

pub(crate) struct PreviewRow<'a> {
    pub c: &'a Candidate,
    pub tier_reason: String,
    pub rebuild_class: &'static str,
    pub source_label: Option<String>,
    pub notes: Vec<String>,
}

pub(crate) fn preview_item(r: &PreviewRow<'_>) -> Value {
    let c = r.c;
    let rule = c.rules.first();
    let mut v = json!({
        "object_id": c.candidate_id,
        "entry_id": c.entry_id.to_string(),
        "path": c.path,
        "category": c.category,
        "rule_id": rule.map(|r| r.rule_id.clone()),
        "rule_version": rule.map(|r| r.version.clone()),
        "tier": c.tier,
        "tier_reason": r.tier_reason,
        "rebuild_class": r.rebuild_class,
        "registry_status": "covered",
        "selected_by_default": selected_by_default(c),
        "skip_reason": skip_reason(c),
        "probes": [],
        "logical_bytes": (c.logical.unknown_count == 0).then(|| c.logical.known.0.to_string()),
        "reclaimable_estimate_bytes": null,
        "impact": c.impact,
        "notes": r.notes,
    });
    if let Some(s) = &r.source_label {
        v["source_label"] = json!(s);
    }
    v
}

pub(crate) fn preview_record(req: &str, command: &str, scan: Option<(&str, u32)>, non_interactive: bool, rows: &[PreviewRow<'_>]) -> Value {
    let selected: Vec<&Candidate> = rows.iter().map(|r| r.c).filter(|c| selected_by_default(c)).collect();
    let summary = json!({
        "selected_count": selected.len(),
        "selected_known_bytes": selected.iter().fold(0u64, |a, c| a.saturating_add(c.logical.known.0)).to_string(),
        "selected_unknown_size_count": selected.iter().map(|c| c.logical.unknown_count).sum::<u64>(),
        "review_only_count": rows.iter().filter(|r| r.c.tier == Tier::ReviewOnly).count(),
        "blocked_count": rows.iter().filter(|r| r.c.tier == Tier::Blocked).count(),
        "skipped_count": rows.iter().filter(|r| skip_reason(r.c).is_some()).count(),
    });
    record("selection_preview", req, fields(json!({
        "command": command,
        "scan_id": scan.map(|s| s.0),
        "scan_revision": scan.map(|s| s.1),
        "dry_run": true,
        "non_interactive": non_interactive,
        "items": rows.iter().map(preview_item).collect::<Vec<_>>(),
        "next_cursor": null,
        "page_size": rows.len().clamp(1, 500),
        "summary": summary,
        "convertible_to_plan": rows.iter().any(|r| plannable(r.c)),
    })))
}

pub(crate) fn non_interactive(p: &Parsed) -> bool {
    p.flag("non-interactive") || !crate::plan_cmd::interactive()
}

/// `--mode` for write commands: recoverable trash by default; permanent delete only when named.
pub(crate) fn mode(p: &Parsed) -> Result<Mode, String> {
    match p.option("mode") {
        None | Some("trash") => Ok(Mode::Trash),
        Some("quarantine") => Ok(Mode::Quarantine),
        Some("delete") => Ok(Mode::Delete),
        Some(other) => Err(format!("unknown mode {other}; use trash (default), quarantine or delete")),
    }
}

/// A selection made by a group-specific rule (e.g. "every installer from this source").
pub(crate) type Pick<'a> = (&'a dyn Fn(&Candidate) -> bool, &'a str);

/// Create a plan from one of the group's stored candidate sets. `is_group_set` rejects sets of
/// other commands or of an older catalog. Non-interactive default selection only ever takes
/// default-selected candidates; explicit ids (or an interactive `pick`) may widen it, and the
/// plan still needs approval.
pub(crate) fn plan_from_sets(req: &str, p: &Parsed, last_run: &str, is_group_set: &dyn Fn(&CandidateSet) -> bool, pick: Option<Pick<'_>>) -> ExitCode {
    let mode = match mode(p) {
        Ok(m) => m,
        Err(e) => return fail(req, ErrorCode::InvalidArgument, &e, p.format),
    };
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let ids: Vec<String> = match p.option("set") {
        Some(id) => vec![id.to_owned()],
        None => last_run_sets(&stores.dir, last_run),
    };
    let mut sets = Vec::new();
    for id in &ids {
        match stores.index.candidate_set(id).and_then(|v| serde_json::from_value::<CandidateSet>(v).map_err(|e| cm_store::StoreError::IndexCorrupt(e.to_string()))) {
            Ok(s) if is_group_set(&s) => sets.push(s),
            Ok(_) if p.option("set").is_some() => return fail(req, ErrorCode::InvalidArgument, "the set was not produced by this command group with the current catalog", p.format),
            Ok(_) => {}
            Err(e) if p.option("set").is_some() => return store_error(req, &e, p.format),
            Err(_) => {}
        }
    }
    if sets.is_empty() {
        return fail(req, ErrorCode::InvalidArgument, "no candidate set: run the discovery (scan) first or pass --set ID", p.format);
    }
    let (set, selected): (&CandidateSet, Vec<String>) = match (p.option("candidates"), p.flag("all-default-selected"), pick) {
        (None, false, Some((rule, what))) => {
            let with: Vec<&CandidateSet> = sets.iter().filter(|s| s.candidates.iter().any(|c| plannable(c) && rule(c))).collect();
            match with.as_slice() {
                [one] => (*one, one.candidates.iter().filter(|c| plannable(c) && rule(c)).map(|c| c.candidate_id.clone()).collect()),
                [] => return fail(req, ErrorCode::InvalidArgument, &format!("no plannable candidate {what}"), p.format),
                _ => return fail(req, ErrorCode::InvalidArgument, &format!("candidates {what} come from several locations; pass --set ID"), p.format),
            }
        }
        (Some(list), false, None) => {
            let wanted: Vec<String> = list.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect();
            let owners: Vec<&CandidateSet> = sets.iter().filter(|s| wanted.iter().any(|w| s.candidates.iter().any(|c| &c.candidate_id == w))).collect();
            match owners.as_slice() {
                [one] => (*one, wanted),
                [] => return fail(req, ErrorCode::InvalidArgument, "none of the candidates is in the latest discovery; pass --set ID", p.format),
                _ => return fail(req, ErrorCode::InvalidArgument, "the candidates come from several roots; create one plan per root with --set ID", p.format),
            }
        }
        (None, true, None) => {
            let with: Vec<&CandidateSet> = sets.iter().filter(|s| s.candidates.iter().any(selected_by_default)).collect();
            match with.as_slice() {
                [one] => (*one, one.candidates.iter().filter(|c| selected_by_default(c)).map(|c| c.candidate_id.clone()).collect()),
                [] => return fail(req, ErrorCode::InvalidArgument, "nothing is selected by default (review-only items need explicit --candidates)", p.format),
                many => {
                    let list: Vec<&str> = many.iter().map(|s| s.candidate_set_id.as_str()).collect();
                    return fail(req, ErrorCode::InvalidArgument, &format!("several roots have default selections; pass --set with one of {}", list.join(", ")), p.format);
                }
            }
        }
        _ => return fail(req, ErrorCode::InvalidArgument, "pass exactly one of --candidates ID,ID or --all-default-selected (installer: or --source LABEL)", p.format),
    };
    // CLI-11: previewed rows whose observed state changed since discovery are listed as stale, not planned.
    let (selected, stale) = crate::clean_cmd::split_stale(&set.candidates, selected);
    if selected.is_empty() {
        return fail(req, ErrorCode::PlanStale, &format!("every selected item changed since discovery ({}); run the preview again", crate::clean_cmd::stale_summary(&stale)), p.format);
    }
    let view = match stores.index.view(&set.scan_id) {
        Ok(v) => v,
        Err(e) => return store_error(req, &e, p.format),
    };
    let status = match stores.index.scan_status(&set.scan_id) {
        Ok(s) => s,
        Err(e) => return store_error(req, &e, p.format),
    };
    let policies = stores.audit.policies().unwrap_or_default();
    let pdigest = cm_core::c14n::digest(&serde_json::to_value(&policies).unwrap()).expect("plain JSON");
    let plan_id = format!("p_{}", &req[4..]);
    let input = cm_core::plan::PlanInput {
        plan_id: &plan_id,
        scope_id: &status.scope_id,
        created_unix: crate::plan_cmd::now_unix(),
        observed_at: status.finished_at.as_deref().unwrap_or(&status.started_at),
        policy_digest: &pdigest,
    };
    let plan = match cm_core::plan::build_plan(&view, set, &selected, mode, &input) {
        Ok(p) => p,
        Err(e) => return fail(req, ErrorCode::PolicyDenied, &e.to_string(), p.format),
    };
    if let Err(e) = stores.audit.save_plan(&plan) {
        return store_error(req, &e, p.format);
    }
    emit(&crate::clean_cmd::with_stale(crate::plan_cmd::plan_record(req, &stores, &plan), &stale), p.format);
    ExitCode::SUCCESS
}
