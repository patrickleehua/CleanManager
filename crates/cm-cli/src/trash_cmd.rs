//! `trash` command group (parity MTR-01..05, task 6.5): manual selection to the Recycle Bin.
//!
//! - `trash --scan ID --entry ID[,ID]` freezes entries of a published scan into a `manual_selection` plan: `fs_trash` at
//!   the original place only, never policy-matchable. It only creates the plan; `plan review` (a person, in a terminal)
//!   approves it and `plan apply` runs it. There is no flag that approves or applies.
//! - `trash inspect --scan ID --entry ID` is read-only: index facts (size, coverage gaps, protected objects), the
//!   processes holding it open (Restart Manager; skipped for cloud placeholders) and the path as data. Revealing in
//!   Explorer is a desktop action; this command opens, reads and downloads nothing.
//! - `trash list` lists the current user's Recycle Bin items; `trash purge --item ID[,ID]` freezes some of them into a
//!   separate `manual_selection` plan of permanent deletes, which needs an extra irreversibility confirmation in review.

fn ids(list: &str) -> Vec<String> {
    list.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect()
}

#[cfg(windows)]
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

    use super::ids;
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

    fn holders_of(ins: &Inspection, view: &cm_core::analysis::MemView) -> Option<Vec<String>> {
        // Never for cloud placeholders (no open, no download); folders query their files.
        if ins.cloud == CloudState::Placeholder || ins.refusal.as_ref().is_some_and(|r| r.reason == "cloud_placeholder") {
            return None;
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
        cm_platform::windows::restart_manager::holders(&paths).ok().map(|h| h.iter().map(|x| x.describe()).collect())
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
            if let Err(why) = cm_platform::windows::actions::recycle_bin_ready(&path) {
                return fail(req, ErrorCode::Unsupported, &format!("{}: {why}; there is no permanent-delete alternative", path.display()), p.format);
            }
        }
        let hard = cm_platform::protection::hard_deny_set();
        let user = cm_platform::protection::load(&scan.stores.dir);
        let problem = user.as_ref().err().map(ToString::to_string);
        let guard = SelectionGuard { hard: Some(&hard), user: user.as_ref().map_err(|_| problem.as_deref().unwrap_or("unavailable")) };
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
        let ins = inspect(&scan.view, entry, &guard);
        let holders = holders_of(&ins, &scan.view);
        let trashable = ins.refusal.is_none();
        let mut actions = vec!["show_path", "copy_path"];
        if trashable {
            actions.push("move_to_trash");
        }
        let guidance = match &ins.refusal {
            None => format!("Plan it with `cleanmanager trash --scan {} --entry {entry}`; a person reviews it before anything moves.", scan.status.scan_id),
            Some(r) if r.code == "policy_denied" && r.reason == "protected" => VIEW_ONLY_EXPLANATION.to_string(),
            Some(r) => r.message.clone(),
        };
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
        let (listings, failed) = cm_platform::windows::actions::recycle_bin::list_user_bins();
        let items: Vec<Value> = listings.iter().flat_map(|l| l.items.iter().map(|i| bin_item(&l.view, i))).collect();
        let issues: Vec<Value> = failed.iter().map(|(d, e)| json!({"code": "bin_unreadable", "scope_display": d.display().to_string(), "message": e})).collect();
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
        let (listings, _) = cm_platform::windows::actions::recycle_bin::list_user_bins();
        // One plan per recycle bin (volume): every selected item must come from the same listing.
        let Some(listing) = listings.iter().find(|l| l.items.iter().any(|i| wanted.contains(&i.trash_item_id))) else {
            return fail(req, ErrorCode::InvalidArgument, "no such item in your Recycle Bin; run `cleanmanager trash list`", p.format);
        };
        let mut chosen = Vec::new();
        for id in &wanted {
            match listing.items.iter().find(|i| &i.trash_item_id == id) {
                Some(i) => chosen.push(i),
                None => return fail(req, ErrorCode::InvalidArgument, &format!("item {id} is not in the same Recycle Bin; plan each volume separately"), p.format),
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

#[cfg(windows)]
pub use imp::{trash_inspect, trash_list, trash_plan, trash_purge};

#[cfg(not(windows))]
use {crate::args::Parsed, std::process::ExitCode};

#[cfg(not(windows))]
fn unsupported(req: &str, p: &Parsed) -> ExitCode {
    let _ = ids;
    crate::fail(req, cm_core::error::ErrorCode::Unsupported, "manual trash is not implemented on this platform yet", p.format)
}

#[cfg(not(windows))]
pub fn trash_plan(req: &str, p: &Parsed) -> ExitCode {
    unsupported(req, p)
}

#[cfg(not(windows))]
pub fn trash_inspect(req: &str, p: &Parsed) -> ExitCode {
    unsupported(req, p)
}

#[cfg(not(windows))]
pub fn trash_list(req: &str, p: &Parsed) -> ExitCode {
    unsupported(req, p)
}

#[cfg(not(windows))]
pub fn trash_purge(req: &str, p: &Parsed) -> ExitCode {
    unsupported(req, p)
}
