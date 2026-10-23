//! `trash` command group (parity MTR-01..05, tasks 6.2/6.5): manual selection to the Recycle Bin (Windows) or the Trash
//! (macOS).
//!
//! - `trash --scan ID --entry ID[,ID]` freezes entries of a published scan into a `manual_selection` plan: `fs_trash` at
//!   the original place only, never policy-matchable. It only creates the plan; `plan review` (a person, in a terminal)
//!   approves it and `plan apply` runs it. There is no flag that approves or applies. A volume without a usable bin
//!   refuses the plan up front; nothing is ever deleted instead.
//! - `trash inspect --scan ID --entry ID` is read-only: index facts (size, coverage gaps, protected objects), the
//!   processes holding it open (Restart Manager on Windows, open descriptors on macOS; skipped for cloud placeholders)
//!   and the path as data. Revealing in Explorer/Finder is a desktop action; this command opens, reads and downloads
//!   nothing.
//! - `trash list` lists the current user's bin items; `trash purge --item ID[,ID]` freezes some of them into a separate
//!   `manual_selection` plan of permanent deletes, which needs an extra irreversibility confirmation in review.
//!
//! The command bodies are shared; `plat` supplies the per-platform readiness check, holder query and bin listing.

fn ids(list: &str) -> Vec<String> {
    list.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect()
}

/// MTR-03 / WFA-35: refuse a plan that would move `paths` to the bin when any of their volumes has no usable bin
/// (turned off, by policy, network, none on the volume). The error is `unsupported` with the `trash_unavailable`
/// message naming the volumes; `quarantine` adds `--mode quarantine` as the alternative to the next step. Nothing is
/// deleted instead and the mode is never switched. Called after the plan was built (so protection refusals come
/// first) and before it is stored.
pub(crate) fn bins_ready(req: &str, p: &crate::args::Parsed, paths: &[cm_core::path::RawPath], quarantine: bool) -> Result<(), std::process::ExitCode> {
    use cm_platform::trash_readiness::{message, next_step, unready};
    let unready = unready(paths);
    if unready.is_empty() {
        return Ok(());
    }
    for u in &unready {
        cm_core::debug_trace!("trash.ready", "volume=\"{}\" unready: {}", u.volume, u.reason);
    }
    let code = cm_core::error::ErrorCode::Unsupported;
    let next = next_step(&unready, quarantine.then_some("--mode quarantine"));
    let record = cm_core::protocol::error_record(req, code, &message(&unready), Some(&next), Default::default());
    crate::emit(&record, p.format);
    Err(std::process::ExitCode::from(cm_core::error::ExitClass::Error(code).code() as u8))
}

/// [`bins_ready`] for a cleanup (plan/v1) selection: only in trash mode, for the selected candidates' volumes.
pub(crate) fn plan_bins_ready(
    req: &str, p: &crate::args::Parsed, set: &cm_core::analysis::CandidateSet, selected: &[String], mode: cm_core::rules::Mode,
) -> Result<(), std::process::ExitCode> {
    if mode != cm_core::rules::Mode::Trash {
        return Ok(());
    }
    let paths: Vec<cm_core::path::RawPath> = set.candidates.iter().filter(|c| selected.contains(&c.candidate_id)).map(|c| c.path.clone()).collect();
    bins_ready(req, p, &paths, true)
}

/// Windows: Restart Manager holders (the bin listing and readiness are shared, `cm_app::trash`).
#[cfg(windows)]
mod plat {
    use cm_core::path::RawPath;

    /// Holder descriptions and a note on what could not be seen.
    pub fn holders(paths: &[RawPath]) -> (Option<Vec<String>>, Option<String>) {
        (cm_platform::windows::restart_manager::holders(paths).ok().map(|h| h.iter().map(|x| x.describe()).collect()), None)
    }

    /// Extra guidance after the "plan it" line of a trashable object (none on Windows).
    pub fn put_back_hint() -> Option<&'static str> {
        None
    }
}

/// macOS: open-descriptor holders (the Trash listing and readiness are shared, `cm_app::trash`).
#[cfg(target_os = "macos")]
mod plat {
    use cm_core::path::RawPath;
    use cm_platform::macos::actions::trash::open_holders;

    /// Where to find what was moved, and how to return it: Finder keeps the original location privately.
    pub const PUT_BACK: &str = "To put it back: open the Trash in Finder, select the item and choose File › Put Back (or drag it out).";

    fn limit_note(uninspected: usize) -> Option<String> {
        (uninspected > 0).then(|| format!("{uninspected} process(es) of other users could not be inspected; a holder among them would not be listed"))
    }

    pub fn holders(paths: &[RawPath]) -> (Option<Vec<String>>, Option<String>) {
        match open_holders(paths) {
            Ok(h) => {
                let note = limit_note(h.uninspected);
                (Some(h.holders), note)
            }
            Err(e) => (None, Some(e)),
        }
    }

    pub fn put_back_hint() -> Option<&'static str> {
        Some(PUT_BACK)
    }
}

#[cfg(any(windows, target_os = "macos"))]
mod imp {
    use std::process::ExitCode;

    use cm_app::trash::{BIN, Bin, PickError};
    use cm_core::analysis::IndexView;
    use cm_core::error::ErrorCode;
    use cm_core::manual_trash::{BinItem, Inspection, ManualError, ManualInput, SelectionGuard, VIEW_ONLY_EXPLANATION, build_trash_plan};
    use cm_core::model::ObjectKind;
    use cm_core::protocol::{ErrorDetails, error_record, record};
    use cm_store::Stores;
    use cm_store::index::ScanStatusView;
    use serde_json::{Value, json};

    use super::{bins_ready, ids, plat};
    use crate::args::Parsed;
    use crate::commands::{fields, open_stores, store_error};
    use crate::plan_cmd::{current_policy_digest, now_unix, v2_plan_record};
    use crate::{emit, fail};

    struct Scan {
        stores: Stores,
        status: ScanStatusView,
        view: cm_core::analysis::MemView,
    }

    fn published(req: &str, p: &Parsed) -> Result<Scan, ExitCode> {
        let Some(scan_id) = p.option("scan") else { return Err(fail(req, ErrorCode::InvalidArgument, "needs --scan SCAN_ID", p.format)) };
        let stores = open_stores(req, p)?;
        let status = match stores.index.scan_status(scan_id) {
            Ok(s) if s.published => s,
            Ok(_) => return Err(fail(req, ErrorCode::InvalidArgument, "scan is not published yet", p.format)),
            Err(e) => return Err(store_error(req, &e, p.format)),
        };
        let view = stores.index.view(scan_id).map_err(|e| store_error(req, &e, p.format))?;
        Ok(Scan { stores, status, view })
    }

    /// Entry ids from a published scan; never a path.
    fn entries(req: &str, p: &Parsed, view: &cm_core::analysis::MemView) -> Result<Vec<u64>, ExitCode> {
        let Some(list) = p.option("entry") else { return Err(fail(req, ErrorCode::InvalidArgument, "needs --entry ENTRY_ID[,ENTRY_ID]", p.format)) };
        let mut out = Vec::new();
        for id in ids(list) {
            match id.parse::<u64>().ok().filter(|e| view.facts.contains_key(e)) {
                Some(e) => out.push(e),
                None => return Err(fail(req, ErrorCode::InvalidArgument, &format!("entry {id} is not in this scan (use entry ids from the scan, not paths)"), p.format)),
            }
        }
        Ok(out)
    }

    /// Holders of the object (folders: its files) and a note on what could not be seen; `None` when not queried
    /// (never for cloud placeholders: no open, no download).
    fn holders_of(ins: &Inspection, view: &cm_core::analysis::MemView) -> (Option<Vec<String>>, Option<String>) {
        match cm_app::trash::holder_paths(ins, view) {
            Some(paths) => plat::holders(&paths),
            None => (None, None),
        }
    }

    fn refusal_error(req: &str, p: &Parsed, e: &ManualError) -> ExitCode {
        fail(req, e.code(), &e.to_string(), p.format)
    }

    pub fn trash_plan(req: &str, p: &Parsed) -> ExitCode {
        let scan = match published(req, p) {
            Ok(s) => s,
            Err(c) => return c,
        };
        let selected = match entries(req, p, &scan.view) {
            Ok(e) => e,
            Err(c) => return c,
        };
        let hard = cm_platform::protection::hard_deny_set();
        let user = cm_platform::protection::load(&scan.stores.dir);
        let problem = user.as_ref().err().map(ToString::to_string);
        let guard = SelectionGuard { hard: Some(&hard), user: user.as_ref().map_err(|_| problem.as_deref().unwrap_or("unavailable")) };
        // Objects the platform's trash action cannot take are refused before anything is planned.
        if let Err(e) = cm_app::trash::platform_plan_check(&scan.view, &selected, &guard) {
            return refusal_error(req, p, &e);
        }
        let plan_id = format!("mp_{}", &req[4..]);
        let pdigest = current_policy_digest(&scan.stores);
        let observed_at = scan.status.finished_at.clone().unwrap_or_else(|| scan.status.started_at.clone());
        let input = ManualInput {
            plan_id: &plan_id, scope_id: Some(&scan.status.scope_id), scan_id: Some(&scan.status.scan_id), scan_revision: Some(scan.status.revision),
            created_unix: now_unix(), observed_at: &observed_at, policy_digest: &pdigest,
        };
        let plan = match build_trash_plan(&scan.view, &selected, &guard, &input) {
            Ok(pl) => pl,
            // WFA-47/63: a link into a protected system location is refused as protected, naming the location.
            Err(e) => return refusal_error(req, p, &cm_app::trash::name_protected_link(&scan.view, e, &hard)),
        };
        // MTR-03 / WFA-35: a volume without a usable recycle bin fails the plan before it is stored; nothing is ever
        // deleted instead.
        let paths: Vec<cm_core::path::RawPath> = selected.iter().map(|&e| scan.view.path(e)).collect();
        if let Err(c) = bins_ready(req, p, &paths, false) {
            return c;
        }
        if let Err(e) = scan.stores.audit.save_plan_v2(&plan) {
            return store_error(req, &e, p.format);
        }
        emit(&v2_plan_record(req, &scan.stores, &plan), p.format);
        ExitCode::SUCCESS
    }

    fn kind_name(k: ObjectKind) -> Value {
        serde_json::to_value(k).unwrap_or(Value::Null)
    }

    pub fn trash_inspect(req: &str, p: &Parsed) -> ExitCode {
        let scan = match published(req, p) {
            Ok(s) => s,
            Err(c) => return c,
        };
        let entry = match entries(req, p, &scan.view) {
            Ok(e) if e.len() == 1 => e[0],
            Ok(_) => return fail(req, ErrorCode::InvalidArgument, "inspect takes exactly one --entry", p.format),
            Err(c) => return c,
        };
        let hard = cm_platform::protection::hard_deny_set();
        let user = cm_platform::protection::load(&scan.stores.dir);
        let problem = user.as_ref().err().map(ToString::to_string);
        let guard = SelectionGuard { hard: Some(&hard), user: user.as_ref().map_err(|_| problem.as_deref().unwrap_or("unavailable")) };
        // The platform's refusals, and a link into a protected system location view-only as protected, naming the
        // location (WFA-47/63).
        let (mut ins, _) = cm_app::trash::inspect_entry(&scan.view, entry, &guard, &hard);
        // WFA-35: a volume without a usable bin offers no move (the plan would be refused).
        if ins.refusal.is_none() {
            ins.refusal = cm_app::trash::bin_refusal(&ins.path, "trash_unavailable");
        }
        let (holders, holders_note) = holders_of(&ins, &scan.view);
        let trashable = ins.refusal.is_none();
        let mut actions = vec!["show_path", "copy_path"];
        if trashable {
            actions.push("move_to_trash");
        }
        let mut guidance = match &ins.refusal {
            None => format!("Plan it with `cleanmanager trash --scan {} --entry {entry}`; a person reviews it before anything moves.", scan.status.scan_id),
            Some(r) if r.code == "policy_denied" && r.reason == "protected" => VIEW_ONLY_EXPLANATION.to_string(),
            Some(r) => r.message.clone(),
        };
        if trashable && let Some(hint) = plat::put_back_hint() {
            guidance = format!("{guidance} {hint}");
        }
        if let Some(note) = holders_note {
            guidance = format!("{guidance} Holders: {note}.");
        }
        let hit = |h: &cm_core::manual_trash::Hit| json!({"path": h.path, "reason": h.reason});
        let parent = scan.view.parent(entry).map(|e| scan.view.path(e).display());
        emit(&record("trash_inspection", req, fields(json!({
            "scan_id": scan.status.scan_id,
            "entry_id": entry.to_string(),
            "path": ins.path,
            "parent_display": parent,
            "object_kind": kind_name(ins.kind),
            "cloud": ins.cloud,
            "logical_bytes": ins.logical_bytes.map(|b| b.to_string()),
            "allocated_bytes": ins.allocated_bytes.map(|b| b.to_string()),
            "files": ins.files,
            "directories": ins.directories,
            "protected": ins.protected.iter().map(hit).collect::<Vec<_>>(),
            "protected_count": ins.protected_count,
            "coverage_gaps": ins.gaps.iter().map(hit).collect::<Vec<_>>(),
            "gap_count": ins.gap_count,
            "trashable": trashable,
            "refusal": ins.refusal.as_ref().map(|r| json!({"code": r.code, "reason": r.reason, "message": r.message})),
            "holders": holders,
            "available_actions": actions,
            "guidance": guidance,
        }))), p.format);
        ExitCode::SUCCESS
    }

    fn bin_item(bin: &Bin, i: &BinItem) -> Value {
        json!({
            "trash_item_id": i.trash_item_id,
            "bin_location": bin.location(i),
            "original_path": i.original_path,
            "deleted_at": i.deleted_at,
            "object_kind": kind_name(i.kind),
            "logical_bytes": i.logical_bytes.map(|b| b.to_string()),
            "purge_capability": if i.refusal.is_none() { "available" } else { "unsupported" },
            "reason": i.refusal,
        })
    }

    /// Every bin of this user, or the error record when the bin cannot be listed at all (never an empty list).
    fn list_bins(req: &str, p: &Parsed) -> Result<(Vec<Bin>, Vec<cm_app::trash::BinIssue>), ExitCode> {
        cm_app::trash::list_bins().map_err(|e| {
            emit(&error_record(req, e.code, &e.message, e.required_action, ErrorDetails::default()), p.format);
            ExitCode::from(cm_core::error::ExitClass::Error(e.code).code() as u8)
        })
    }

    pub fn trash_list(req: &str, p: &Parsed) -> ExitCode {
        let (bins, issues) = match list_bins(req, p) {
            Ok(l) => l,
            Err(c) => return c,
        };
        let items: Vec<Value> = bins.iter().flat_map(|b| b.items.iter().map(|i| bin_item(b, i))).collect();
        let issues: Vec<Value> = issues.iter().map(|i| json!({"code": i.code, "scope_display": i.scope_display, "message": i.message})).collect();
        let partial = !issues.is_empty();
        emit(&record("trash_list", req, fields(json!({"items": items, "issues": issues}))), p.format);
        ExitCode::from(if partial { cm_core::error::ExitClass::Partial } else { cm_core::error::ExitClass::Success }.code() as u8)
    }

    pub fn trash_purge(req: &str, p: &Parsed) -> ExitCode {
        let Some(list) = p.option("item") else { return fail(req, ErrorCode::InvalidArgument, "needs --item TRASH_ITEM_ID[,ID] (from `trash list`)", p.format) };
        let wanted = ids(list);
        let stores = match open_stores(req, p) {
            Ok(s) => s,
            Err(c) => return c,
        };
        let (bins, _) = match list_bins(req, p) {
            Ok(l) => l,
            Err(c) => return c,
        };
        // One plan per recycle bin (volume): every selected item must come from the same listing.
        let (bin, chosen) = match cm_app::trash::pick(&bins, &wanted) {
            Ok(v) => v,
            Err(PickError::NotListed) => return fail(req, ErrorCode::InvalidArgument, &format!("no such item in your {BIN}; run `cleanmanager trash list`"), p.format),
            Err(PickError::OtherBin(id)) => return fail(req, ErrorCode::InvalidArgument, &format!("item {id} is not in the same {BIN}; plan each volume separately"), p.format),
        };
        let plan_id = format!("mp_{}", &req[4..]);
        let pdigest = current_policy_digest(&stores);
        let plan = match cm_app::trash::purge_plan(bin, &chosen, &plan_id, &pdigest, now_unix()) {
            Ok(pl) => pl,
            Err(e) => return refusal_error(req, p, &e),
        };
        if let Err(e) = stores.audit.save_plan_v2(&plan) {
            return store_error(req, &e, p.format);
        }
        emit(&v2_plan_record(req, &stores, &plan), p.format);
        ExitCode::SUCCESS
    }
}

#[cfg(any(windows, target_os = "macos"))]
pub use imp::{trash_inspect, trash_list, trash_plan, trash_purge};

#[cfg(not(any(windows, target_os = "macos")))]
use {crate::args::Parsed, std::process::ExitCode};

#[cfg(not(any(windows, target_os = "macos")))]
fn unsupported(req: &str, p: &Parsed) -> ExitCode {
    let _ = ids;
    crate::fail(req, cm_core::error::ErrorCode::Unsupported, "manual trash is not implemented on this platform yet", p.format)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn trash_plan(req: &str, p: &Parsed) -> ExitCode {
    unsupported(req, p)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn trash_inspect(req: &str, p: &Parsed) -> ExitCode {
    unsupported(req, p)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn trash_list(req: &str, p: &Parsed) -> ExitCode {
    unsupported(req, p)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn trash_purge(req: &str, p: &Parsed) -> ExitCode {
    unsupported(req, p)
}

/// Seam for `plan apply` (plan/v2 backends): names of the processes holding these files open on macOS (open
/// descriptors and working directories of this user's processes; other users' processes cannot be inspected, which
/// the last entry says when it matters). `None` from the closure = no holder could be determined (shown as unknown;
/// D6: nothing is ended).
#[cfg(target_os = "macos")]
#[allow(clippy::type_complexity)]
pub(crate) fn mac_holders() -> Option<Box<dyn Fn(&[cm_core::path::RawPath]) -> Option<Vec<String>>>> {
    Some(Box::new(cm_platform::macos::actions::trash::apply_holders))
}
