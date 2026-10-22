//! Shapes sent over IPC. Byte counts are decimal strings (never JSON numbers) so the WebView
//! can hold them as BigInt without precision loss; unknown is `null`, never `"0"`.

use cm_core::error::ErrorCode;
use cm_core::model::{BoundaryKind, IdentityQuality, ObjectKind, ScanState};
use cm_core::path::RawPath;
use cm_store::StoreError;
use cm_store::index::{EntryItem, KindFilter, SelectionSummary, SizeBasis, Sort};
use serde::Serialize;

use crate::args::ArgError;

/// Error returned to the WebView: a stable protocol code, a finer `reason` the UI branches on,
/// and a human message. Paths inside messages are display strings only.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct IpcError {
    pub code: ErrorCode,
    pub reason: &'static str,
    pub message: String,
}

impl IpcError {
    pub fn new(code: ErrorCode, reason: &'static str, message: impl Into<String>) -> Self {
        IpcError { code, reason, message: message.into() }
    }
}

impl From<ArgError> for IpcError {
    fn from(e: ArgError) -> Self {
        IpcError::new(ErrorCode::InvalidArgument, "invalid_argument", e.0)
    }
}

impl From<StoreError> for IpcError {
    fn from(e: StoreError) -> Self {
        let reason = match &e {
            StoreError::InvalidArgument(_) => "invalid_argument",
            StoreError::CursorStale(_) => "cursor_stale",
            StoreError::NotFound(_) => "not_found",
            StoreError::InvalidState(_) => "not_published",
            StoreError::IndexIncompatible { .. } => "index_incompatible",
            StoreError::IndexCorrupt(_) => "index_corrupt",
            StoreError::StoreInsideScope { .. } => "store_inside_scope",
            StoreError::AuditUnavailable(_) => "audit_unavailable",
            StoreError::Io(_) => "io_error",
            StoreError::Sqlite(_) | StoreError::Core(_) => "internal",
        };
        let code = match &e {
            StoreError::Io(_) => ErrorCode::IoError,
            _ => e.code(),
        };
        IpcError::new(code, reason, e.to_string())
    }
}

pub type IpcResult<T> = Result<T, IpcError>;

/// Display form of a raw path's last component (the scope root keeps its full path).
pub fn last_component(display: &str) -> &str {
    let trimmed = display.trim_end_matches(['\\', '/']);
    match trimmed.rfind(['\\', '/']) {
        Some(i) if i + 1 < trimmed.len() => &trimmed[i + 1..],
        _ => display,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EntryDto {
    pub entry_id: String,
    pub parent_entry_id: Option<String>,
    pub name: String,
    pub path_display: String,
    pub kind: ObjectKind,
    pub logical_bytes: Option<String>,
    pub allocated_bytes: Option<String>,
    pub size_basis: SizeBasis,
    pub unknown_descendants: Option<u64>,
    pub boundary: Option<BoundaryKind>,
    pub identity_quality: IdentityQuality,
    pub link_count: Option<u32>,
    /// DIN-05: read-only "cleanable by hand" hint (listed directories only); never a selection.
    pub cleanable_hint: Option<cm_core::cleanable::CleanableHint>,
}

impl EntryDto {
    pub fn from_item(item: &EntryItem) -> Self {
        let display = item.path.display();
        let name = if item.parent_entry_id.is_none() { display.clone() } else { last_component(&display).to_owned() };
        EntryDto {
            entry_id: item.entry_id.clone(),
            parent_entry_id: item.parent_entry_id.clone(),
            name,
            path_display: display,
            kind: item.kind,
            logical_bytes: item.logical_bytes.map(|b| b.to_string()),
            allocated_bytes: item.allocated_bytes.map(|b| b.to_string()),
            size_basis: item.size_basis,
            unknown_descendants: item.unknown_descendants,
            boundary: item.boundary,
            identity_quality: item.identity_quality,
            link_count: item.link_count,
            cleanable_hint: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EntriesPageDto {
    pub scan_id: String,
    pub scope_id: String,
    pub revision: u32,
    pub sort: Sort,
    pub kind: KindFilter,
    pub name_contains: Option<String>,
    pub parent_entry_id: Option<String>,
    pub page_size: u32,
    pub items: Vec<EntryDto>,
    pub next_cursor: Option<String>,
    pub unknown_size_items: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SummaryDto {
    pub logical_known_bytes: String,
    pub logical_unknown_count: u64,
    /// Exact only when nothing is unknown; otherwise `null` with the known part separately.
    pub allocated_exact_bytes: Option<String>,
    pub allocated_known_bytes: String,
    pub allocated_unknown_count: u64,
    pub covered_entries: u64,
    pub overlapping_selected: u64,
    pub unread_ranges: u64,
}

impl From<SelectionSummary> for SummaryDto {
    fn from(s: SelectionSummary) -> Self {
        SummaryDto {
            logical_known_bytes: s.logical.known.to_string(),
            logical_unknown_count: s.logical.unknown_count,
            allocated_exact_bytes: s.allocated.exact().map(|b| b.to_string()),
            allocated_known_bytes: s.allocated.known.to_string(),
            allocated_unknown_count: s.allocated.unknown_count,
            covered_entries: s.covered_entries,
            overlapping_selected: s.overlapping_selected,
            unread_ranges: s.unread_ranges,
        }
    }
}

/// Children beyond the tiles returned for one treemap layer.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RestDto {
    pub count: u64,
    pub known_logical_bytes: String,
    pub unknown_count: u64,
    /// `false` when the remaining children were too many to total within the query budget;
    /// the counts above then cover only the part that was read.
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TreemapLayerDto {
    pub scan_id: String,
    pub revision: u32,
    pub parent_entry_id: String,
    /// Overlap-free subtree summary of the layer's directory (denominator for "share of known").
    pub parent_summary: SummaryDto,
    /// Largest children by known logical size (unknown sizes sort last and carry `null`).
    pub items: Vec<EntryDto>,
    pub rest: RestDto,
}

#[derive(Debug, Clone, Serialize)]
pub struct IssueDto {
    pub code: String,
    pub scope_display: String,
    pub message: String,
    /// Structured cause of a `permission_required` issue (docs/platforms/macos.md §6): the stored record's field or
    /// the message's `permission_cause=…` marker; absent when neither names one (nothing is guessed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_cause: Option<cm_core::permission::PermissionCause>,
}

impl IssueDto {
    /// An issue whose structured cause is read from its message ([`cm_core::permission::cause_in`]).
    pub fn new(code: impl Into<String>, scope_display: impl Into<String>, message: impl Into<String>) -> IssueDto {
        let message = message.into();
        IssueDto { code: code.into(), scope_display: scope_display.into(), permission_cause: cm_core::permission::cause_in(&message), message }
    }

    /// From a stored wire issue (`{code, scope_display, message[, permission_cause]}`); its own field wins.
    pub fn from_wire(i: &serde_json::Value) -> IssueDto {
        let text = |k: &str| i[k].as_str().unwrap_or("").to_owned();
        let mut d = IssueDto::new(text("code"), text("scope_display"), text("message"));
        if let Some(c) = i["permission_cause"].as_str().and_then(cm_core::permission::PermissionCause::parse) {
            d.permission_cause = Some(c);
        }
        d
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanStatusDto {
    pub scan_id: String,
    pub scope_id: Option<String>,
    pub state: ScanState,
    pub revision: Option<u32>,
    pub published: bool,
    /// `verified_scope` or `partial_scope` once published.
    pub completeness: Option<String>,
    pub cancel_requested: bool,
    pub host_kind: Option<String>,
    pub host_pid: Option<u32>,
    /// This desktop process is running the scan right now.
    pub hosted_here: bool,
    pub heartbeat_at: Option<String>,
    pub entries_seen: u64,
    pub skipped_ranges: u64,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub root_display: Option<String>,
    pub root_entry_id: Option<String>,
    pub summary: Option<SummaryDto>,
    pub backend: Option<String>,
    pub backend_reason: Option<String>,
    pub elevated_read: Vec<String>,
    pub issues: Vec<IssueDto>,
    pub issues_total: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RootOfferDto {
    pub token: String,
    pub label: String,
    /// `user_home`, `user_folder`, `volume` or `chosen`.
    pub kind: &'static str,
    pub path_display: String,
    pub problem: Option<IpcError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StartScanDto {
    pub scan_id: String,
    pub root_display: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CancelDto {
    pub scan_id: String,
    pub accepted: bool,
    pub state_at_receipt: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CapabilitiesDto {
    pub platform: cm_core::capability::Platform,
    pub engine_version: &'static str,
    pub schema_version: &'static str,
    pub capabilities: Vec<cm_core::capability::Capability>,
    pub store_dir_display: Option<String>,
    pub store_problem: Option<IpcError>,
    /// Result of dropping SeBackup/SeRestore at startup (Windows); `None` elsewhere.
    pub bypass_privileges_disabled: Option<bool>,
    pub bypass_privileges_note: Option<String>,
    pub interrupted_at_startup: Vec<String>,
    /// Operations found at startup whose host had exited: outcomes need reconciliation.
    pub operations_to_reconcile: Vec<String>,
    /// File actions run only for stored, authorized plans (never a path from the WebView).
    pub write_commands: bool,
    /// Whether this build has file actions for the platform (Windows and macOS; the capability rows say what is
    /// verified on a reference device).
    pub file_actions_verified: bool,
    pub rules_dir_display: Option<String>,
    /// Why the rule pack cannot be used (analysis and execution stop), if so.
    pub rules_problem: Option<String>,
    pub rule_count: Option<usize>,
    /// The OS user that receipts and policies bind to.
    pub principal: String,
    /// D15: capabilities whose release gate has not passed are off unless this is on (shared with the CLI).
    pub experimental_features: bool,
    /// One row per gated capability: its gate, whether it passed and whether it is on here.
    pub gates: Vec<cm_app::features::GateRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecentScanDto {
    pub scan_id: String,
    pub root_display: String,
    pub started_at: String,
    pub state: Option<ScanState>,
    pub published: bool,
    /// `desktop` or `cli`: which kind of process hosted the scan.
    pub host_kind: String,
    pub completeness: Option<String>,
    pub finished_at: Option<String>,
    pub problem: Option<IpcError>,
}

/// Event payloads.
#[derive(Debug, Clone, Serialize)]
pub struct ScanFinishedEvent {
    pub scan_id: String,
    pub state: Option<ScanState>,
    pub error: Option<IpcError>,
}

pub fn display_of(path: &std::path::Path) -> String {
    match cm_platform::paths::raw_path(path.as_os_str()) {
        Ok(raw) => RawPath::display(&raw),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_component_handles_both_separators_and_roots() {
        assert_eq!(last_component("C:\\Users\\Lin\\Downloads"), "Downloads");
        assert_eq!(last_component("/Users/lin/Library"), "Library");
        assert_eq!(last_component("D:\\"), "D:\\");
        assert_eq!(last_component("name"), "name");
    }

    #[test]
    fn store_errors_map_to_ui_reasons() {
        let e: IpcError = StoreError::CursorStale("x".into()).into();
        assert_eq!((e.code, e.reason), (ErrorCode::CursorStale, "cursor_stale"));
        let e: IpcError = StoreError::InvalidState("staging".into()).into();
        assert_eq!(e.reason, "not_published");
        let e: IpcError = ArgError("bad".into()).into();
        assert_eq!(e.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn summary_keeps_unknown_separate_from_exact() {
        let mut s = SelectionSummary::default();
        s.logical.add(Some(cm_core::bytes::Bytes(u64::MAX))).unwrap();
        s.allocated.add(None).unwrap();
        let d = SummaryDto::from(s);
        assert_eq!(d.logical_known_bytes, "18446744073709551615");
        assert_eq!(d.allocated_exact_bytes, None);
        assert_eq!(d.allocated_unknown_count, 1);
        let json = serde_json::to_value(&d).unwrap();
        assert!(json["logical_known_bytes"].is_string(), "byte counts never travel as JSON numbers");
    }

    /// The structured `permission_cause` reaches the page: from the stored field, else the message marker, else absent.
    #[test]
    fn issues_carry_their_permission_cause() {
        use cm_core::permission::PermissionCause;
        let pending = IssueDto::new("permission_required", "/Users/a/Documents", "permission_required (permission_cause=privacy_prompt_pending): macOS did not answer");
        assert_eq!(pending.permission_cause, Some(PermissionCause::PrivacyPromptPending));
        assert_eq!(serde_json::to_value(&pending).unwrap()["permission_cause"], "privacy_prompt_pending");
        let plain = IssueDto::new("root_missing", "D:\\x", "gone");
        assert!(serde_json::to_value(&plain).unwrap().get("permission_cause").is_none(), "no marker, no field");
        let stored = serde_json::json!({"code": "permission_required", "scope_display": "/y", "message": "m", "permission_cause": "undetermined"});
        assert_eq!(IssueDto::from_wire(&stored).permission_cause, Some(PermissionCause::Undetermined), "the stored field wins");
        let marked = serde_json::json!({"code": "permission_required", "scope_display": "/y", "message": "x (permission_cause=undetermined)"});
        assert_eq!(IssueDto::from_wire(&marked).permission_cause, Some(PermissionCause::Undetermined));
        let odd = serde_json::json!({"code": "c", "scope_display": "s", "message": "m", "permission_cause": "guessed"});
        assert_eq!(IssueDto::from_wire(&odd).permission_cause, None, "an unknown spelling is not passed on");
    }
}
