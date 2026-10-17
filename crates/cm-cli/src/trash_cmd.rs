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

/// Windows: Shell recycle bin readiness, Restart Manager holders, `$Recycle.Bin\<SID>` listing.
#[cfg(windows)]
mod plat {
    use std::process::ExitCode;

    use cm_core::manual_trash::{Inspection, TrashRefusal};
    use cm_core::path::RawPath;
    use serde_json::{Value, json};

    use crate::args::Parsed;

    pub type Listing = cm_platform::windows::actions::recycle_bin::BinListing;

    /// What the platform calls its bin, in messages.
    pub const BIN: &str = "Recycle Bin";

    /// MTR-03: why a plan touching `path` is refused up front (no usable recycle bin), as the full error message.
    pub fn not_ready(path: &RawPath) -> Option<(cm_core::error::ErrorCode, String)> {
        cm_platform::windows::actions::recycle_bin_ready(path)
            .err()
            .map(|why| (cm_core::error::ErrorCode::Unsupported, format!("{}: {why}; there is no permanent-delete alternative", path.display())))
    }

    /// Whether objects of this kind may meet a platform refusal (none on Windows).
    pub fn may_refuse(_kind: cm_core::model::ObjectKind) -> bool {
        false
    }

    /// Objects this platform's trash action cannot take, beyond the core refusals (none on Windows).
    pub fn refusal(_ins: &Inspection) -> Option<TrashRefusal> {
        None
    }

    /// Holder descriptions and a note on what could not be seen.
    pub fn holders(paths: &[RawPath]) -> (Option<Vec<String>>, Option<String>) {
        (cm_platform::windows::restart_manager::holders(paths).ok().map(|h| h.iter().map(|x| x.describe()).collect()), None)
    }

    /// Extra guidance after the "plan it" line of a trashable object (none on Windows).
    pub fn put_back_hint() -> Option<&'static str> {
        None
    }

    pub fn list_bins(_req: &str, _p: &Parsed) -> Result<(Vec<Listing>, Vec<Value>), ExitCode> {
        let (listings, failed) = cm_platform::windows::actions::recycle_bin::list_user_bins();
        let issues = failed.iter().map(|(d, e)| json!({"code": "bin_unreadable", "scope_display": d.display().to_string(), "message": e})).collect();
        Ok((listings, issues))
    }
}

/// macOS: NSFileManager Trash readiness, open-descriptor holders, `~/.Trash` and `/Volumes/*/.Trashes/<uid>` listing.
#[cfg(target_os = "macos")]
mod plat {
    use std::path::PathBuf;
    use std::process::ExitCode;

    use cm_core::error::ErrorCode;
    use cm_core::manual_trash::{Inspection, TrashRefusal};
    use cm_core::model::ObjectKind;
    use cm_core::path::RawPath;
    use cm_core::protocol::{ErrorDetails, error_record};
    use cm_platform::macos::actions::trash::{ListError, TrashListing, folder_refusal, list_trash_dir, open_holders, trash_ready, volume_trash_dirs};
    use serde_json::{Value, json};

    use crate::args::Parsed;
    use crate::emit;

    pub type Listing = TrashListing;

    pub const BIN: &str = "Trash";

    /// Where to find what was moved, and how to return it: Finder keeps the original location privately.
    pub const PUT_BACK: &str = "To put it back: open the Trash in Finder, select the item and choose File › Put Back (or drag it out).";

    /// Next step when privacy protection (TCC) or permissions block the Trash listing.
    const GRANT_ACCESS: &str = "Give the program that runs cleanmanager (for example Terminal) Full Disk Access in System Settings › Privacy & Security › Full Disk Access, then retry; or open the Trash in Finder";
    /// Next step when the folder's own permissions block it.
    const FIX_PERMISSIONS: &str = "The Trash folder should belong to you and be readable by you; check its permissions, or open the Trash in Finder";

    pub fn not_ready(path: &RawPath) -> Option<(ErrorCode, String)> {
        trash_ready(path)
            .err()
            .map(|why| (ErrorCode::Unsupported, format!("trash_unavailable: {}: {why}; there is no permanent-delete alternative", path.display())))
    }

    pub fn may_refuse(kind: ObjectKind) -> bool {
        kind == ObjectKind::Directory
    }

    /// The macOS trash action moves single verified files; a folder cannot be verified as one object in this build.
    pub fn refusal(ins: &Inspection) -> Option<TrashRefusal> {
        folder_refusal(ins)
    }

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

    /// The home Trash: `$CM_RESOLVE_HOME/.Trash` when that test redirect is set (then no volume is listed), otherwise
    /// `$HOME/.Trash`.
    fn home_trash() -> (Option<PathBuf>, bool) {
        if let Some(h) = std::env::var_os("CM_RESOLVE_HOME").filter(|v| !v.is_empty()) {
            return (Some(PathBuf::from(h).join(".Trash")), true);
        }
        (std::env::var_os("HOME").filter(|v| !v.is_empty()).map(|h| PathBuf::from(h).join(".Trash")), false)
    }

    fn issue(dir: &std::path::Path, e: &ListError) -> Value {
        let code = match e {
            ListError::PermissionRequired { .. } => "permission_required",
            ListError::Unreadable(_) => "bin_unreadable",
        };
        let message = match e {
            ListError::PermissionRequired { .. } if e.privacy_blocked() => format!("{} (blocked by privacy protection or another policy layer; permission_cause=undetermined). {GRANT_ACCESS}", e.message()),
            ListError::PermissionRequired { .. } => format!("{} (its permissions do not allow listing it). {FIX_PERMISSIONS}", e.message()),
            ListError::Unreadable(_) => e.message().to_owned(),
        };
        json!({"code": code, "scope_display": dir.display().to_string(), "message": message})
    }

    /// Every Trash of this user. The home Trash failing to list is an error (`permission_required` when blocked), never
    /// an empty list; a volume's Trash failing is an issue next to the others.
    pub fn list_bins(req: &str, p: &Parsed) -> Result<(Vec<Listing>, Vec<Value>), ExitCode> {
        let (home, redirected) = home_trash();
        let mut listings = Vec::new();
        let mut issues = Vec::new();
        if let Some(dir) = home {
            match list_trash_dir(&dir) {
                Ok(l) => listings.push(l),
                // No Trash folder yet: truly nothing in it.
                Err(ListError::Unreadable(_)) if std::fs::symlink_metadata(&dir).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) => {}
                Err(e) => {
                    let (code, action) = match &e {
                        ListError::PermissionRequired { .. } if e.privacy_blocked() => (ErrorCode::PermissionRequired, Some(GRANT_ACCESS)),
                        ListError::PermissionRequired { .. } => (ErrorCode::PermissionRequired, Some(FIX_PERMISSIONS)),
                        ListError::Unreadable(_) => (ErrorCode::IoError, None),
                    };
                    let message = issue(&dir, &e)["message"].as_str().unwrap_or_default().to_owned();
                    emit(&error_record(req, code, &message, action, ErrorDetails::default()), p.format);
                    return Err(ExitCode::from(cm_core::error::ExitClass::Error(code).code() as u8));
                }
            }
        }
        if !redirected {
            let (dirs, failed) = volume_trash_dirs();
            issues.extend(failed.iter().map(|(d, e)| issue(d, e)));
            for d in dirs {
                match list_trash_dir(&d) {
                    Ok(l) => listings.push(l),
                    Err(e) => issues.push(issue(&d, &e)),
                }
            }
        }
        Ok((listings, issues))
    }
}

#[cfg(any(windows, target_os = "macos"))]
mod imp {
    use std::process::ExitCode;

    use cm_core::analysis::IndexView;
    use cm_core::error::ErrorCode;
    use cm_core::manual_trash::{BinItem, Inspection, ManualError, ManualInput, SelectionGuard, VIEW_ONLY_EXPLANATION, build_purge_plan, build_trash_plan, inspect};
    use cm_core::model::{CloudState, ObjectKind};
    use cm_core::protocol::record;
    use cm_store::Stores;
    use cm_store::index::ScanStatusView;
    use serde_json::{Value, json};

    use super::{ids, plat};
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

    /// Holders of the object (folders: its files) and a note on what could not be seen; `None` when not queried.
    fn holders_of(ins: &Inspection, view: &cm_core::analysis::MemView) -> (Option<Vec<String>>, Option<String>) {
        // Never for cloud placeholders (no open, no download); folders query their files.
        if ins.cloud == CloudState::Placeholder || ins.refusal.as_ref().is_some_and(|r| r.reason == "cloud_placeholder") {
            return (None, None);
        }
        let mut paths = vec![ins.path.clone()];
        if ins.kind == ObjectKind::Directory {
            paths.clear();
            let mut stack = vec![ins.entry_id];
            while let Some(e) = stack.pop() {
                let f = view.info(e);
                if f.boundary {
                    continue;
                }
                match f.kind {
                    ObjectKind::Directory => stack.extend_from_slice(view.children(e)),
                    ObjectKind::File if paths.len() < 1000 => paths.push(view.path(e)),
                    _ => {}
                }
            }
        }
        plat::holders(&paths)
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
        // MTR-03: a volume without a usable recycle bin fails the plan; nothing is ever deleted instead.
        for &e in &selected {
            let path = scan.view.path(e);
            if let Some((code, message)) = plat::not_ready(&path) {
                return fail(req, code, &message, p.format);
            }
        }
        let hard = cm_platform::protection::hard_deny_set();
        let user = cm_platform::protection::load(&scan.stores.dir);
        let problem = user.as_ref().err().map(ToString::to_string);
        let guard = SelectionGuard { hard: Some(&hard), user: user.as_ref().map_err(|_| problem.as_deref().unwrap_or("unavailable")) };
        // Objects the platform's trash action cannot take are refused before anything is planned.
        for &e in selected.iter().filter(|&&e| plat::may_refuse(scan.view.info(e).kind)) {
            let ins = inspect(&scan.view, e, &guard);
            if ins.refusal.is_none()
                && let Some(r) = plat::refusal(&ins)
            {
                let err = ManualError::Refused { entry_id: e.to_string(), path: ins.path.display(), refusal: Box::new(r), protected: vec![], gaps: vec![] };
                return refusal_error(req, p, &err);
            }
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
            Err(e) => return refusal_error(req, p, &e),
        };
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
        let mut ins = inspect(&scan.view, entry, &guard);
        if ins.refusal.is_none() {
            ins.refusal = plat::refusal(&ins);
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

    fn bin_item(view: &cm_core::analysis::MemView, i: &BinItem) -> Value {
        json!({
            "trash_item_id": i.trash_item_id,
            "bin_location": view.path(i.data_entry).display(),
            "original_path": i.original_path,
            "deleted_at": i.deleted_at,
            "object_kind": kind_name(i.kind),
            "logical_bytes": i.logical_bytes.map(|b| b.to_string()),
            "purge_capability": if i.refusal.is_none() { "available" } else { "unsupported" },
            "reason": i.refusal,
        })
    }

    pub fn trash_list(req: &str, p: &Parsed) -> ExitCode {
        let (listings, issues) = match plat::list_bins(req, p) {
            Ok(l) => l,
            Err(c) => return c,
        };
        let items: Vec<Value> = listings.iter().flat_map(|l| l.items.iter().map(|i| bin_item(&l.view, i))).collect();
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
        let (listings, _) = match plat::list_bins(req, p) {
            Ok(l) => l,
            Err(c) => return c,
        };
        // One plan per recycle bin (volume): every selected item must come from the same listing.
        let Some(listing) = listings.iter().find(|l| l.items.iter().any(|i| wanted.contains(&i.trash_item_id))) else {
            return fail(req, ErrorCode::InvalidArgument, &format!("no such item in your {}; run `cleanmanager trash list`", plat::BIN), p.format);
        };
        let mut chosen = Vec::new();
        for id in &wanted {
            match listing.items.iter().find(|i| &i.trash_item_id == id) {
                Some(i) => chosen.push(i),
                None => return fail(req, ErrorCode::InvalidArgument, &format!("item {id} is not in the same {}; plan each volume separately", plat::BIN), p.format),
            }
        }
        let plan_id = format!("mp_{}", &req[4..]);
        let pdigest = current_policy_digest(&stores);
        let input = ManualInput {
            plan_id: &plan_id, scope_id: None, scan_id: None, scan_revision: None, created_unix: now_unix(), observed_at: &listing.observed_at, policy_digest: &pdigest,
        };
        let plan = match build_purge_plan(&listing.view, &chosen, &input) {
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
