//! `protect list|add|remove|check` (PRT-01..03, CLI-10). One list file shared with the desktop
//! app (`protection.json` in the store directory, written atomically under a lock).
//!
//! Adding protection only narrows what may be cleaned, so machine callers may add. Removing a
//! user entry widens it: like approving a plan, it needs an interactive terminal and a typed
//! confirmation (PLN-07 threat boundary applies: a TTY is not proof of a human). Hard-deny and
//! default-category protection are listed but can never be removed. An unreadable or invalid
//! list stops every write with `protection_unavailable` (PRT-02); `check` still answers.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cm_core::analysis::{ProtectionClass, class_of_name};
use cm_core::error::{ErrorCode, ExitClass};
use cm_core::protect::{self, HardDenySet, MatchKind, ProtectionDecision, ProtectionUnavailable, Subject, UserEntry, UserProtection};
use cm_core::protocol::{ErrorDetails, error_record, record};
use cm_platform::protection::{PROTECTION_FILE, UpdateError};
use serde_json::{Value, json};

use crate::args::Parsed;
use crate::commands::fields;
use crate::plan_cmd::{interactive, prompt};
use crate::{Format, emit, fail};

fn store_dir(req: &str, p: &Parsed) -> Result<PathBuf, ExitCode> {
    p.raw_option("store")
        .map(PathBuf::from)
        .or_else(cm_platform::paths::default_store_dir)
        .ok_or_else(|| fail(req, ErrorCode::InvalidArgument, "no store directory; pass --store or set CM_STORE_DIR", p.format))
}

fn unavailable(req: &str, dir: &Path, e: &ProtectionUnavailable, format: Format) -> ExitCode {
    let action = format!("Repair or move aside {}; every write stays stopped until the list loads", dir.join(PROTECTION_FILE).display());
    emit(&error_record(req, e.code(), &e.to_string(), Some(&action), ErrorDetails::default()), format);
    ExitCode::from(ExitClass::Error(e.code()).code() as u8)
}

fn update_error<E: std::fmt::Display>(req: &str, dir: &Path, e: UpdateError<E>, format: Format) -> ExitCode {
    match e {
        UpdateError::Unavailable(u) => unavailable(req, dir, &u, format),
        UpdateError::Rejected(r) => fail(req, ErrorCode::InvalidArgument, &r.to_string(), format),
        UpdateError::Busy => fail(req, ErrorCode::Conflict, "the protection list is locked by another writer; retry", format),
        UpdateError::Io(io) => fail(req, ErrorCode::IoError, &format!("protection list not saved: {io}"), format),
    }
}

fn match_name(kind: MatchKind) -> &'static str {
    match kind {
        MatchKind::Exact => "exact",
        MatchKind::Glob => "glob",
        MatchKind::Descendants => "descendants",
        MatchKind::Ancestors => "ancestors",
        MatchKind::Rule => "rule",
        MatchKind::MaintenanceTask => "maintenance_task",
    }
}

fn target_of(e: &UserEntry) -> &str {
    e.path.as_deref().or(e.glob.as_deref()).or(e.rule_id.as_deref()).or(e.task_id.as_deref()).unwrap_or("")
}

/// Protocol `protection_list`: hard deny, then default categories, then the user's entries.
fn list_record(req: &str, user: &UserProtection, hard: &HardDenySet) -> Value {
    let mut items = vec![json!({
        "protection_id": "hd_textual", "layer": "hard_deny", "match": "glob",
        "display": format!("Textual pre-filter ({}): system directories, volume and share roots, device and administrative paths, traversal and control characters", protect::HARD_DENY_VERSION),
        "removable": false, "note": null, "added_at": null,
    })];
    let roots: BTreeSet<(String, &'static str, bool)> = hard.roots().map(|r| (r.display.clone(), r.class.as_str(), r.class.covers_descendants())).collect();
    for (display, class, subtree) in roots {
        let id = cm_core::c14n::digest(&json!([class, display])).expect("plain JSON");
        items.push(json!({
            "protection_id": format!("hd_{}", &id[..12]), "layer": "hard_deny", "match": if subtree { "descendants" } else { "exact" },
            "display": format!("{display} ({class})"), "removable": false, "note": null, "added_at": null,
        }));
    }
    for c in ProtectionClass::ALL {
        items.push(json!({
            "protection_id": format!("dc_{}", c.as_str()), "layer": "default_category", "match": "descendants",
            "display": c.describe(), "removable": false, "note": null, "added_at": null,
        }));
    }
    for e in user.entries() {
        items.push(json!({
            "protection_id": e.id, "layer": "user", "match": match_name(e.kind), "display": target_of(e),
            "removable": true, "note": e.note, "added_at": e.added_at,
        }));
    }
    record("protection_list", req, fields(json!({ "items": items })))
}

pub fn protect_list(req: &str, p: &Parsed) -> ExitCode {
    let dir = match store_dir(req, p) {
        Ok(d) => d,
        Err(c) => return c,
    };
    match cm_platform::protection::load(&dir) {
        Ok(user) => {
            emit(&list_record(req, &user, &cm_platform::protection::hard_deny_set()), p.format);
            ExitCode::SUCCESS
        }
        Err(e) => unavailable(req, &dir, &e, p.format),
    }
}

fn platform() -> cm_core::capability::Platform {
    if cfg!(windows) { cm_core::capability::Platform::Windows } else { cm_core::capability::Platform::Macos }
}

/// Next free `u_N` id.
fn next_id(list: &UserProtection) -> String {
    let n = list.entries().iter().filter_map(|e| e.id.strip_prefix("u_").and_then(|s| s.parse::<u64>().ok())).max().unwrap_or(0);
    format!("u_{}", n + 1)
}

/// Option text that must survive as stored text (the list stores paths as strings).
fn text_option(p: &Parsed, key: &str) -> Result<Option<String>, String> {
    match p.raw_option(key) {
        None => Ok(None),
        Some(os) => os.to_str().map(|s| Some(s.to_owned())).ok_or_else(|| format!("--{key} is not valid Unicode and cannot be stored in the protection list")),
    }
}

pub fn protect_add(req: &str, p: &Parsed) -> ExitCode {
    let bad = |m: &str| fail(req, ErrorCode::InvalidArgument, m, p.format);
    let given: Vec<&str> = ["path", "glob", "rule", "task"].into_iter().filter(|k| p.raw_option(k).is_some()).collect();
    if given.len() != 1 {
        return bad("pass exactly one of --path P, --glob G, --rule RULE_ID, --task TASK_ID");
    }
    if p.option("match").is_some() && given[0] != "path" {
        return bad("--match applies to --path only");
    }
    let value = match text_option(p, given[0]) {
        Ok(v) => v.unwrap_or_default(),
        Err(e) => return bad(&e),
    };
    let added_at = cm_core::time::now_utc();
    let mut entry = match given[0] {
        "path" => {
            let kind = match p.option("match") {
                None | Some("descendants") => MatchKind::Descendants,
                Some("exact") => MatchKind::Exact,
                Some("ancestors") => MatchKind::Ancestors,
                Some(other) => return bad(&format!("unknown --match {other}; use descendants, exact or ancestors")),
            };
            UserEntry::path("u_0", kind, &value, &added_at)
        }
        "glob" => UserEntry::glob("u_0", &value, &added_at),
        "rule" => UserEntry::rule("u_0", &value, &added_at),
        _ => UserEntry::task("u_0", &value, &added_at),
    };
    match text_option(p, "note") {
        Ok(Some(n)) => entry = entry.with_note(&n),
        Ok(None) => {}
        Err(e) => return bad(&e),
    }
    // Shape first (relative paths, traversal, textual hard-deny), then identity: an entry that
    // resolves into a hard-deny location by identity (case variant, link) is refused (PRT-02).
    if let Err(e) = UserProtection::empty(platform()).validate(&entry) {
        return bad(&e.to_string());
    }
    let hard = cm_platform::protection::hard_deny_set();
    if entry.path.is_some() {
        let raw = match p.raw_option("path").map(|os| cm_platform::paths::raw_path(os)) {
            Some(Ok(r)) => r,
            _ => return bad("invalid --path"),
        };
        if let Err(e) = protect::check_entry_identity(&cm_platform::protection::identity_chain(&raw), &hard) {
            return bad(&e.to_string());
        }
    }
    let dir = match store_dir(req, p) {
        Ok(d) => d,
        Err(c) => return c,
    };
    match cm_platform::protection::update(&dir, |list| {
        entry.id = next_id(list);
        list.add(entry)
    }) {
        Ok(((), list)) => {
            emit(&list_record(req, &list, &hard), p.format);
            ExitCode::SUCCESS
        }
        Err(e) => update_error(req, &dir, e, p.format),
    }
}

pub fn protect_remove(req: &str, p: &Parsed, id: &str) -> ExitCode {
    // Removing protection widens what may be cleaned: a human act, never a machine one.
    if !interactive() {
        return fail(req, ErrorCode::InvalidArgument, "removing protection requires an interactive terminal; machine callers may add protection but never remove it", p.format);
    }
    let dir = match store_dir(req, p) {
        Ok(d) => d,
        Err(c) => return c,
    };
    let list = match cm_platform::protection::load(&dir) {
        Ok(l) => l,
        Err(e) => return unavailable(req, &dir, &e, p.format),
    };
    let Some(entry) = list.entries().iter().find(|e| e.id == id) else {
        let msg = if id.starts_with("hd_") || id.starts_with("dc_") {
            "hard-deny and default-category protection cannot be removed".to_owned()
        } else {
            format!("no user protection entry {id}")
        };
        return fail(req, ErrorCode::InvalidArgument, &msg, p.format);
    };
    println!("Protection {}: {} {}  (added {})", entry.id, match_name(entry.kind), target_of(entry), entry.added_at);
    if let Some(n) = &entry.note {
        println!("Note: {n}");
    }
    println!("Removing it lets rules and plans touch these objects again (hard-deny and default categories still apply).");
    match prompt(&format!("Type {id} to remove this protection, anything else to cancel: ")) {
        Some(a) if a == id => {}
        _ => return fail(req, ErrorCode::Cancelled, "not removed", p.format),
    }
    match cm_platform::protection::update(&dir, |list| list.remove(id).map(|_| ()).ok_or_else(|| format!("no user protection entry {id}"))) {
        Ok(((), list)) => {
            emit(&list_record(req, &list, &cm_platform::protection::hard_deny_set()), p.format);
            ExitCode::SUCCESS
        }
        Err(e) => update_error(req, &dir, e, p.format),
    }
}

/// Default class of a live path as analysis classifies an indexed entry: the strongest class
/// on the object and its ancestors (a non-reviewable class wins, else the nearest). The
/// object's descendants are not walked.
fn default_class(path: &Path) -> Option<ProtectionClass> {
    let mut found = Vec::new();
    for (i, p) in path.ancestors().enumerate() {
        let Some(name) = p.file_name() else { continue };
        let meta = std::fs::symlink_metadata(p).ok();
        // A missing ancestor is a directory by position; a missing object is classified as a file.
        let is_dir = meta.as_ref().map_or(i > 0, |m| m.is_dir());
        let has_git = is_dir && std::fs::symlink_metadata(p.join(".git")).is_ok();
        found.extend(class_of_name(&name.to_string_lossy().to_lowercase(), is_dir, has_git));
    }
    found.iter().copied().find(|c| !c.is_reviewable()).or_else(|| found.first().copied())
}

fn verdict(layer: &str, d: &ProtectionDecision, clear: &str, detail: Option<String>, entry: Option<&str>) -> Value {
    match d {
        ProtectionDecision::Allowed => json!({"layer": layer, "result": "clear", "detail": clear, "protection_id": null}),
        ProtectionDecision::Denied { reason, .. } => {
            json!({"layer": layer, "result": "denied", "detail": detail.unwrap_or_else(|| reason.clone()), "protection_id": entry})
        }
    }
}

pub fn protect_check(req: &str, p: &Parsed) -> ExitCode {
    let Some(os) = p.raw_option("path") else { return fail(req, ErrorCode::InvalidArgument, "protect check needs --path P", p.format) };
    let raw = match cm_platform::paths::raw_path(os) {
        Ok(r) => r,
        Err(e) => return fail(req, ErrorCode::InvalidArgument, &format!("invalid --path: {e}"), p.format),
    };
    let dir = match store_dir(req, p) {
        Ok(d) => d,
        Err(c) => return c,
    };
    let path = Path::new(os);
    let chain = cm_platform::protection::identity_chain(&raw);
    let hard = cm_platform::protection::hard_deny_set();
    let class = default_class(path);
    let recursive = std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir());
    let subject = Subject { path: &raw, chain: &chain, default_class: class, rule_id: p.option("rule"), recursive };
    let class_detail = class.map(|c| {
        let how = if c.is_reviewable() { "never an automatic candidate or policy match; only per-object review may plan it" } else { "never planned" };
        format!("default protection class {}: {} ({how})", c.as_str(), c.describe())
    });
    let mut layers = vec![
        verdict("hard_deny", &protect::check_path(&raw, &chain, &hard), "not a hard-deny location", None, None),
        verdict("default_category", &protect::check_default_class(&subject), "no default protection class (descendants not examined)", class_detail, None),
    ];
    layers.push(match cm_platform::protection::load(&dir) {
        Ok(user) => {
            let (d, e) = protect::check_user(&subject, &user);
            verdict("user", &d, "no user protection entry matches", None, e.map(|e| e.id.as_str()))
        }
        Err(e) => json!({"layer": "user", "result": "unavailable", "protection_id": null,
            "detail": format!("protection list unavailable ({e}); every write is stopped until it is repaired")}),
    });
    let first = layers.iter().find(|l| l["result"] != "clear");
    let out = record("protection_check", req, fields(json!({
        "path": raw,
        "decision": if first.is_some() { "denied" } else { "allowed" },
        "layer": first.map(|l| l["layer"].clone()),
        "reason": first.map(|l| l["detail"].clone()),
        "layers": layers,
    })));
    emit(&out, p.format);
    ExitCode::SUCCESS
}
