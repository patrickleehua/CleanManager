//! P07 application inventory and P10 protection settings (APP-01..04, PRT-01..05, GUI-01).
//!
//! Safety properties of this IPC surface:
//! - The application inventory is read-only: no uninstaller, no package removal, no launch.
//! - Protection entries come from host-held sources only: an entry of a published scan
//!   (resolved to its path from the index), a folder the person picked in the host's native
//!   dialog (an opaque root token), or a rule id from the current rule pack. The WebView never
//!   sends a path. Adding protection only narrows what may be cleaned. Removing a user entry
//!   widens it, so the person must type the entry id (as `protect remove` requires a typed
//!   confirmation in a terminal); hard-deny and default categories can never be removed.
//! - The list is written atomically under the lock the CLI also uses. A failed save leaves the
//!   stored list untouched and is reported, never shown as saved.

use std::collections::{BTreeMap, BTreeSet};

use cm_core::analysis::ProtectionClass;
use cm_core::error::ErrorCode;
use cm_core::protect::{self, MatchKind, UserEntry, UserProtection};
use cm_platform::protection::UpdateError;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::args;
use crate::commands::{St, blocking};
use crate::dto::{IpcError, IpcResult, display_of};
use crate::host::{AppState, lock};

fn ipc(code: ErrorCode, reason: &'static str, message: impl Into<String>) -> IpcError {
    IpcError::new(code, reason, message)
}

// ---------------------------------------------------------------------------------------------
// Protection

#[derive(Debug, Clone, Serialize)]
pub struct ProtectionItemDto {
    pub protection_id: String,
    /// `hard_deny`, `default_category` or `user`.
    pub layer: &'static str,
    #[serde(rename = "match")]
    pub kind: &'static str,
    pub display: String,
    pub removable: bool,
    pub note: Option<String>,
    pub added_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProtectionListDto {
    pub items: Vec<ProtectionItemDto>,
    /// The user list cannot be read: every write is stopped until it is repaired (PRT-02).
    pub problem: Option<String>,
    pub file_display: Option<String>,
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

fn target_of(e: &UserEntry) -> String {
    e.path.as_deref().or(e.glob.as_deref()).or(e.rule_id.as_deref()).or(e.task_id.as_deref()).unwrap_or("").to_owned()
}

/// Hard deny, then default categories, then the person's own entries (same order as the CLI).
pub fn protection_list(store_dir: &std::path::Path) -> ProtectionListDto {
    let hard = cm_platform::protection::hard_deny_set();
    let mut items = vec![ProtectionItemDto {
        protection_id: "hd_textual".into(),
        layer: "hard_deny",
        kind: "glob",
        display: format!("System directories, volume and share roots, device and administrative paths ({})", protect::HARD_DENY_VERSION),
        removable: false,
        note: None,
        added_at: None,
    }];
    let roots: BTreeSet<(String, &'static str, bool)> = hard.roots().map(|r| (r.display.clone(), r.class.as_str(), r.class.covers_descendants())).collect();
    for (display, class, subtree) in roots {
        let id = cm_core::c14n::digest(&json!([class, display])).expect("plain JSON");
        items.push(ProtectionItemDto {
            protection_id: format!("hd_{}", &id[..12]),
            layer: "hard_deny",
            kind: if subtree { "descendants" } else { "exact" },
            display: format!("{display} ({class})"),
            removable: false,
            note: None,
            added_at: None,
        });
    }
    for c in ProtectionClass::ALL {
        items.push(ProtectionItemDto {
            protection_id: format!("dc_{}", c.as_str()),
            layer: "default_category",
            kind: "descendants",
            display: c.describe().into(),
            removable: false,
            note: None,
            added_at: None,
        });
    }
    let file_display = Some(display_of(&store_dir.join(cm_platform::protection::PROTECTION_FILE)));
    match cm_platform::protection::load(store_dir) {
        Ok(user) => {
            for e in user.entries() {
                items.push(ProtectionItemDto {
                    protection_id: e.id.clone(),
                    layer: "user",
                    kind: match_name(e.kind),
                    display: target_of(e),
                    removable: true,
                    note: e.note.clone(),
                    added_at: Some(e.added_at.clone()),
                });
            }
            ProtectionListDto { items, problem: None, file_display }
        }
        Err(e) => ProtectionListDto { items, problem: Some(e.to_string()), file_display },
    }
}

/// Where a new protection entry comes from. Never a path typed or sent by the WebView.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProtectTarget {
    /// An entry of a published scan; the host reads its path from the index.
    Entry { scan_id: String, entry_id: String, recursive: bool },
    /// A folder chosen in the host's own dialog (`choose_folder`), named by its opaque token.
    Folder { root_token: String },
    /// A rule id of the installed rule pack.
    Rule { rule_id: String },
}

fn platform() -> cm_core::capability::Platform {
    if cfg!(windows) { cm_core::capability::Platform::Windows } else { cm_core::capability::Platform::Macos }
}

fn next_id(list: &UserProtection) -> String {
    let n = list.entries().iter().filter_map(|e| e.id.strip_prefix("u_").and_then(|s| s.parse::<u64>().ok())).max().unwrap_or(0);
    format!("u_{}", n + 1)
}

fn update_error<E: std::fmt::Display>(e: UpdateError<E>) -> IpcError {
    match e {
        UpdateError::Unavailable(u) => ipc(ErrorCode::InternalError, "protection_unavailable", format!("Not saved: the protection list cannot be read ({u}); it was left unchanged for repair")),
        UpdateError::Rejected(r) => ipc(ErrorCode::InvalidArgument, "protection_rejected", format!("Not saved: {r}")),
        UpdateError::Busy => ipc(ErrorCode::Conflict, "protection_busy", "Not saved: another program is writing the protection list; try again"),
        UpdateError::Io(io) => ipc(ErrorCode::IoError, "io_error", format!("Not saved: {io}; the previous list is unchanged")),
    }
}

/// Add one user entry under the shared writer lock. Validation runs before anything is written
/// (shape, textual hard deny, then identity against the hard-deny set).
pub fn add(s: &AppState, target: ProtectTarget, note: Option<String>) -> IpcResult<ProtectionListDto> {
    let dir = s.store_dir()?.to_path_buf();
    let added_at = cm_core::time::now_utc();
    let (entry, raw): (UserEntry, Option<cm_core::path::RawPath>) = match target {
        ProtectTarget::Entry { scan_id, entry_id, recursive } => {
            let scan_id = args::scan_id(&scan_id)?;
            let id = args::entry_id(&entry_id)?;
            let item = s.with_index(|idx| Ok(idx.get_entry(&scan_id, id)?))?;
            let text = item.path.display();
            if item.path.lossy() != text || text.contains('\u{fffd}') {
                return Err(ipc(ErrorCode::InvalidArgument, "unsupported_name", "This name cannot be stored exactly in the protection list"));
            }
            let kind = if recursive { MatchKind::Descendants } else { MatchKind::Exact };
            (UserEntry::path("u_0", kind, &text, &added_at), Some(item.path))
        }
        ProtectTarget::Folder { root_token } => {
            let token = args::root_token(&root_token)?;
            let path = lock(&s.roots).resolve(&token).ok_or_else(|| ipc(ErrorCode::InvalidArgument, "unknown_root", "This folder is no longer offered; choose it again"))?;
            let text = path.to_str().ok_or_else(|| ipc(ErrorCode::InvalidArgument, "unsupported_name", "This folder name cannot be stored in the protection list"))?.to_owned();
            let raw = cm_platform::paths::raw_path(path.as_os_str()).map_err(|e| ipc(ErrorCode::InvalidArgument, "invalid_argument", e.to_string()))?;
            (UserEntry::path("u_0", MatchKind::Descendants, &text, &added_at), Some(raw))
        }
        ProtectTarget::Rule { rule_id } => {
            let rule_id = args::record_id("rule", &rule_id)?;
            let rules = crate::rules::load(s.rules_dir.as_deref()).map_err(|e| ipc(ErrorCode::InternalError, "rules_unavailable", e))?;
            if !rules.rules.iter().any(|r| r.rule_id == rule_id) {
                return Err(ipc(ErrorCode::InvalidArgument, "unknown_rule", "This rule is not part of the installed rule pack"));
            }
            (UserEntry::rule("u_0", &rule_id, &added_at), None)
        }
    };
    let mut entry = entry;
    if let Some(n) = note.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        if n.chars().count() > 200 || n.chars().any(char::is_control) {
            return Err(ipc(ErrorCode::InvalidArgument, "invalid_argument", "The note must be one line of at most 200 characters"));
        }
        entry = entry.with_note(n);
    }
    UserProtection::empty(platform()).validate(&entry).map_err(|e| ipc(ErrorCode::InvalidArgument, "protection_rejected", e.to_string()))?;
    if let Some(raw) = &raw {
        let hard = cm_platform::protection::hard_deny_set();
        protect::check_entry_identity(&cm_platform::protection::identity_chain(raw), &hard)
            .map_err(|e| ipc(ErrorCode::InvalidArgument, "protection_rejected", e.to_string()))?;
    }
    cm_platform::protection::update(&dir, |list| {
        entry.id = next_id(list);
        list.add(entry)
    })
    .map_err(update_error)?;
    Ok(protection_list(&dir))
}

/// Remove one user entry; the typed confirmation must equal its id.
pub fn remove(s: &AppState, id: &str, confirm: &str) -> IpcResult<ProtectionListDto> {
    let dir = s.store_dir()?.to_path_buf();
    if id.starts_with("hd_") || id.starts_with("dc_") {
        return Err(ipc(ErrorCode::InvalidArgument, "not_removable", "Hard-deny and default-category protection cannot be removed"));
    }
    if confirm != id {
        return Err(ipc(ErrorCode::InvalidArgument, "confirmation_mismatch", "Type the entry id to remove this protection"));
    }
    cm_platform::protection::update(&dir, |list| list.remove(id).map(|_| ()).ok_or_else(|| format!("no user protection entry {id}"))).map_err(update_error)?;
    Ok(protection_list(&dir))
}

// ---------------------------------------------------------------------------------------------
// Applications

#[derive(Debug, Clone, Serialize)]
pub struct AppSizeDto {
    /// `known` (in the scan), `not_in_scan`, `no_location` or `unresolved`.
    pub state: &'static str,
    pub logical_known_bytes: Option<String>,
    pub unknown_descendants: Option<u64>,
    pub entry_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppDto {
    pub instance_id: String,
    pub name: String,
    pub version: Option<String>,
    pub publisher: Option<String>,
    /// `registry_uninstall`, `msi`, `msix_store` or `unknown`.
    pub source: &'static str,
    pub location_display: Option<String>,
    pub evidence: Vec<String>,
    pub protected_reason: Option<cm_core::apps::ProtectedReason>,
    pub uninstall_state: cm_core::capability::CapabilityState,
    pub uninstall_reason: Option<String>,
    pub update_status: cm_core::apps::UpdateStatus,
    /// Names of other instances whose location is the same or nested (APP-02).
    pub shared_with: Vec<String>,
    pub size: AppSizeDto,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppsDto {
    pub items: Vec<AppDto>,
    pub issues: Vec<crate::dto::IssueDto>,
    pub scan_id: Option<String>,
    pub scan_root_display: Option<String>,
    pub supported: bool,
}

/// The installed-application inventory of this platform (the CLI's `apps list`).
fn inventory() -> (Vec<cm_core::apps::AppInstance>, Vec<cm_core::apps::InventoryIssue>, bool) {
    let (a, i) = crate::plat::apps_inventory();
    (a, i, true)
}

pub fn apps(s: &AppState, scan_id: Option<&str>) -> IpcResult<AppsDto> {
    let (instances, issues, supported) = inventory();
    let report = cm_core::apps::associate(&instances, &cm_core::model::Timestamp(cm_core::time::now_utc()));
    let names: BTreeMap<&str, &str> = instances.iter().map(|a| (a.instance_id.as_str(), a.name.as_str())).collect();
    let assoc: BTreeMap<&str, &cm_core::apps::Association> = report.associations.iter().map(|a| (a.instance_id.as_str(), a)).collect();
    // Install locations found in the chosen published scan, matched by identity, not by text.
    let mut found: BTreeMap<(String, String), crate::dto::EntryDto> = BTreeMap::new();
    let mut root_display = None;
    if let Some(scan) = scan_id {
        let ids: Vec<(String, String)> = instances.iter().filter_map(|a| a.location_identity.as_ref().map(|i| (i.volume_id.clone(), i.file_id.clone()))).collect();
        s.with_index(|idx| {
            root_display = idx.scan_status(scan).ok().map(|v| v.root.display());
            for chunk in ids.chunks(cm_store::index::MAX_PAGE as usize) {
                for (identity, e) in idx.entries_by_identity(scan, chunk)? {
                    found.entry(identity).or_insert_with(|| crate::dto::EntryDto::from_item(&e));
                }
            }
            Ok(())
        })?;
    }
    let items = instances
        .iter()
        .map(|a| {
            let size = match (&a.install_location, &a.location_identity) {
                (None, _) => AppSizeDto { state: "no_location", logical_known_bytes: None, unknown_descendants: None, entry_id: None },
                (Some(_), None) => AppSizeDto { state: "unresolved", logical_known_bytes: None, unknown_descendants: None, entry_id: None },
                (Some(_), Some(id)) => match found.get(&(id.volume_id.clone(), id.file_id.clone())) {
                    Some(e) => AppSizeDto { state: "known", logical_known_bytes: e.logical_bytes.clone(), unknown_descendants: e.unknown_descendants, entry_id: Some(e.entry_id.clone()) },
                    None => AppSizeDto { state: "not_in_scan", logical_known_bytes: None, unknown_descendants: None, entry_id: None },
                },
            };
            AppDto {
                instance_id: a.instance_id.clone(),
                name: a.name.clone(),
                version: a.version.clone(),
                publisher: a.publisher.clone(),
                source: a.source.wire_name(),
                location_display: a.install_location.as_ref().map(|p| p.display()),
                evidence: a.to_wire()["identity_evidence"].as_array().map(|v| v.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default(),
                protected_reason: a.protected_class,
                uninstall_state: a.uninstall_capability.state,
                uninstall_reason: a.uninstall_capability.reason.clone(),
                update_status: a.update_status,
                shared_with: assoc.get(a.instance_id.as_str()).map(|x| x.shared_with.iter().map(|id| names.get(id.as_str()).copied().unwrap_or(id).to_owned()).collect()).unwrap_or_default(),
                size,
            }
        })
        .collect();
    let issues = issues.into_iter().map(|i| crate::dto::IssueDto::new(i.code, i.source, i.message)).collect();
    Ok(AppsDto { items, issues, scan_id: scan_id.map(str::to_owned), scan_root_display: root_display, supported })
}

// ---------------------------------------------------------------------------------------------
// IPC

#[tauri::command(rename_all = "snake_case")]
pub async fn list_protection(state: St<'_>) -> IpcResult<ProtectionListDto> {
    let s = state.inner().clone();
    blocking(move || Ok(protection_list(s.store_dir()?))).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn add_protection(state: St<'_>, target: ProtectTarget, note: Option<String>) -> IpcResult<ProtectionListDto> {
    let s = state.inner().clone();
    blocking(move || add(&s, target, note)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remove_protection(state: St<'_>, protection_id: String, confirm: String) -> IpcResult<ProtectionListDto> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::record_id("protection", &protection_id)?;
        let confirm = args::confirmation(&confirm)?;
        remove(&s, &id, &confirm)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_apps(state: St<'_>, scan_id: Option<String>) -> IpcResult<AppsDto> {
    let s = state.inner().clone();
    blocking(move || {
        let scan = scan_id.as_deref().map(args::scan_id).transpose()?;
        apps(&s, scan.as_deref())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(dir: &std::path::Path) -> AppState {
        AppState::with_store(Some(dir.to_path_buf()), None, None)
    }

    #[test]
    fn protection_targets_never_carry_a_path_from_the_webview() {
        let bad = serde_json::json!({"source": "path", "path": "C:\\Windows"});
        assert!(serde_json::from_value::<ProtectTarget>(bad).is_err());
        let extra = serde_json::json!({"source": "rule", "rule_id": "x", "path": "C:\\"});
        assert!(serde_json::from_value::<ProtectTarget>(extra).is_err());
        assert!(serde_json::from_value::<ProtectTarget>(serde_json::json!({"source": "folder", "root_token": "r1"})).is_ok());
    }

    #[test]
    fn add_remove_needs_confirmation_and_failures_leave_the_list_unchanged() {
        let dir = std::env::temp_dir().join(format!("cm-desktop-protect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s = state(&dir);
        // macOS: the temporary directory lies under /private/var, a hard-deny location that is already protected and
        // cannot be added again; the protected folder lives under this crate's target directory there.
        #[cfg(windows)]
        let folder = dir.join("keep-me");
        #[cfg(target_os = "macos")]
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join(format!("cm-desktop-protect-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let token = lock(&s.roots).offer(folder.clone());
        let list = add(&s, ProtectTarget::Folder { root_token: token }, Some("project".into())).unwrap();
        let user: Vec<_> = list.items.iter().filter(|i| i.layer == "user").collect();
        assert_eq!(user.len(), 1);
        assert_eq!((user[0].protection_id.as_str(), user[0].kind, user[0].note.as_deref()), ("u_1", "descendants", Some("project")));
        assert!(list.items.iter().any(|i| i.layer == "hard_deny" && !i.removable));

        assert_eq!(remove(&s, "u_1", "u_2").unwrap_err().reason, "confirmation_mismatch");
        assert_eq!(remove(&s, "dc_database", "dc_database").unwrap_err().reason, "not_removable");
        assert_eq!(add(&s, ProtectTarget::Folder { root_token: "r999".into() }, None).unwrap_err().reason, "unknown_root");

        // An unreadable list is never overwritten by a GUI save.
        let file = dir.join(cm_platform::protection::PROTECTION_FILE);
        let good = std::fs::read_to_string(&file).unwrap();
        std::fs::write(&file, b"{broken").unwrap();
        let token = lock(&s.roots).offer(folder.clone());
        assert_eq!(add(&s, ProtectTarget::Folder { root_token: token }, None).unwrap_err().reason, "protection_unavailable");
        assert_eq!(std::fs::read(&file).unwrap(), b"{broken", "left for repair, not replaced");
        assert!(protection_list(&dir).problem.is_some());
        std::fs::write(&file, good).unwrap();

        let list = remove(&s, "u_1", "u_1").unwrap();
        assert!(list.items.iter().all(|i| i.layer != "user"));
        let _ = std::fs::remove_dir_all(&folder);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
