//! Parity suite commands (PUR-01..05, INC-01..05, MTR-01..05, STA-01..04, DIN-01/02, STS-01..03, GUI-12/13):
//! project-artifact purge, installer cleanup, manual trash, startup items, disk insights, system status, permission
//! readiness and "show in File Explorer".
//!
//! Safety properties of this IPC surface:
//! - The WebView never sends a filesystem path or a plan body. Purge roots are named by an opaque `root_id` the host
//!   derives from the configured path; a new root is picked with the native folder dialog in Rust and validated as
//!   `cleanmanager purge roots add` validates it. Scans and plans name store-issued ids (scan, entry, candidate set,
//!   candidate, startup item) that are resolved against the store or a fresh inventory, never against WebView text.
//! - Discovery (`purge_scan`, `installer_scan`) only reads: it scans the configured or declared locations with the
//!   shared scanner, analyzes them with the built-in catalogs and stores candidate sets, as the CLI's `purge scan` /
//!   `installer scan` do. Nothing outside this tool's store is written.
//! - Every change goes through a stored plan (`create_catalog_plan`, `trash_plan`, `startup_plan` freeze one and
//!   authorize nothing); `plan_detail` issues the single-use review token, `approve_plan` needs it plus the typed
//!   digest prefix, and `start_apply` runs the stored plan through the shared executors. There is no direct write.
//! - `reveal_entry` resolves the path from the published index and starts `explorer.exe /select,` (Windows) or
//!   `open -R` (macOS) without a shell; it opens a window and changes nothing. `status_snapshot`, `insights_list`, `startup_list`, `trash_list` and
//!   `readiness` are read-only.
//! - Manual trash and its plan-time checks are the CLI's (`cm_app::trash`, `cm_app::selection`): a link into a protected
//!   system location is refused as protected and named (WFA-47/63), a volume without a usable bin is not movable at
//!   inspection (WFA-35), a selected discovery folder whose files changed is listed as stale (WFA-50), and a row that
//!   cannot be planned now is refused for that reason first (WFA-58). `trash_purge_plan` freezes permanent deletes of
//!   listed bin items into a separate plan whose approval only a terminal asks for (MTR-03, `v2::terminal_only`).

use std::sync::atomic::{AtomicBool, Ordering};

use cm_core::analysis::{Candidate, CandidateSet, CandidateState};
use cm_core::error::ErrorCode;
use cm_core::path::RawPath;
use cm_core::rules::{Mode, Tier};
use cm_core::time::now_utc;
use cm_store::Stores;
use serde::Serialize;
use tauri::AppHandle;

use crate::args;
use crate::cleanup::{CreatedPlanDto, MAX_SELECTION, enum_text};
use crate::cleanup::{StaleItemDto, new_id, now_unix};
use crate::commands::{St, blocking};
use crate::dto::{IpcError, IpcResult, IssueDto, last_component};
use crate::host::AppState;

/// Entries per manual trash plan request.
pub const MAX_TRASH_SELECTION: usize = 500;

fn ipc(code: ErrorCode, reason: &'static str, message: impl Into<String>) -> IpcError {
    IpcError::new(code, reason, message)
}

fn issue(code: &str, scope: &str, message: impl Into<String>) -> IssueDto {
    IssueDto::new(code, scope, message)
}

/// Stable opaque id of a configured root: the first 16 hex digits of SHA-256 over the raw path bytes (the same
/// digest the CLI uses for the root's scope id).
pub fn root_id(root: &RawPath) -> String {
    let hex = cm_core::adapter::sha256_hex(root.raw_bytes().as_slice()).unwrap_or_default();
    hex.get(..16).unwrap_or_default().to_owned()
}

/// Own connections for long work, so the shared ones stay free for the UI.
pub(crate) fn own_stores(s: &AppState) -> IpcResult<Stores> {
    Ok(Stores::open(s.store_dir()?)?)
}

// ---------------------------------------------------------------------------------------------
// Busy guard: one catalog discovery at a time, never while a desktop scan or operation runs.

static CATALOG_BUSY: AtomicBool = AtomicBool::new(false);

pub(crate) struct Busy;

impl Drop for Busy {
    fn drop(&mut self) {
        CATALOG_BUSY.store(false, Ordering::Release);
    }
}

pub(crate) fn claim(s: &AppState) -> IpcResult<Busy> {
    if let Some((id, _)) = s.active_running() {
        return Err(ipc(ErrorCode::Conflict, "scan_running", format!("Scan {id} is still running; wait for it or cancel it first")));
    }
    if crate::ops::running_apply(s).is_some() {
        return Err(ipc(ErrorCode::Conflict, "operation_running", "An operation is still running; wait for it or stop it first"));
    }
    if CATALOG_BUSY.swap(true, Ordering::AcqRel) {
        return Err(ipc(ErrorCode::Conflict, "scan_running", "Another discovery is still running"));
    }
    Ok(Busy)
}

// ---------------------------------------------------------------------------------------------
// Preview rows (shared by purge and installer discovery; mirrors the CLI's `selection_preview`)

#[derive(Debug, Clone, Serialize)]
pub struct CatalogSetDto {
    pub candidate_set_id: String,
    pub scan_id: String,
    pub root_display: String,
    pub source_label: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CatalogSummaryDto {
    pub default_selected_count: u64,
    pub default_selected_known_bytes: String,
    pub default_selected_unknown_count: u64,
    pub review_only_count: u64,
    pub blocked_count: u64,
    pub skipped_count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogItemDto {
    pub candidate_set_id: String,
    pub candidate_id: String,
    pub entry_id: String,
    pub name: String,
    pub path_display: String,
    /// `default_selected`, `review_only` or `blocked`.
    pub tier: String,
    pub tier_reason: String,
    /// `hard_denied`, `vcs_tracked`, `mounted`, `protected`, `vcs_unknown`, `in_use`, `probe_unknown`,
    /// `partial_evidence` or `recent`.
    pub skip_reason: Option<String>,
    pub plannable: bool,
    pub selected_by_default: bool,
    /// Empty unless plannable.
    pub allowed_modes: Vec<Mode>,
    /// Known part of the logical size (decimal); `None` when nothing is known.
    pub logical_bytes: Option<String>,
    pub unknown_size_count: u64,
    pub rebuild_class: Option<String>,
    pub impact: String,
    pub notes: Vec<String>,
    /// Purge: the project folder; installers: the source label.
    pub group_label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogPreviewDto {
    pub supported: bool,
    /// `purge` or `installer`.
    pub catalog: &'static str,
    pub observed_at: String,
    pub sets: Vec<CatalogSetDto>,
    pub items: Vec<CatalogItemDto>,
    pub summary: CatalogSummaryDto,
    pub issues: Vec<IssueDto>,
}

/// The CLI's `plannable`: allowed, not blocked, with a mode (sizes and modes are checked again by `build_plan`).
pub fn plannable(c: &Candidate) -> bool {
    c.state == CandidateState::Allowed && c.tier != Tier::Blocked && !c.allowed_modes.is_empty()
}

pub fn selected_by_default(c: &Candidate) -> bool {
    plannable(c) && c.tier == Tier::DefaultSelected
}

/// Protocol `preview_item.skip_reason` from analysis reasons (first match by severity), as the CLI reports it.
pub fn skip_reason(c: &Candidate) -> Option<&'static str> {
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

/// Why a purge row has its tier (the CLI's wording).
pub fn purge_tier_reason(c: &Candidate, class: Option<cm_core::purge::ArtifactClass>) -> String {
    let blocking = c.reasons.iter().find(|r| {
        ["hard_deny", "user_protected", "secret_file", "protected:", "vcs_tracked", "vcs_metadata_inside", "cloud_placeholder", "vcs_unknown", "unread_or_unknown_size", "user_protection_unavailable"]
            .iter()
            .any(|p| r.starts_with(p))
    });
    if let Some(b) = blocking {
        return b.clone();
    }
    match (class, c.tier) {
        (Some(cm_core::purge::ArtifactClass::Dependency), _) => "dependencies: review only, reinstalling needs the network".into(),
        (_, Tier::ReviewOnly) if c.reasons.iter().any(|r| r.starts_with("recent_project")) => "recent project: listed, not preselected".into(),
        (_, Tier::ReviewOnly) => "build output that may hold hand-written files: review it".into(),
        _ => "build output next to its project marker; rebuilt locally".into(),
    }
}

/// Why an installer row has its tier (the CLI's wording).
pub fn installer_tier_reason(c: &Candidate) -> String {
    let blocking = c.reasons.iter().find(|r| ["hard_deny", "user_protected", "user_protection_unavailable", "mounted", "in_use"].iter().any(|p| r.starts_with(p)));
    match (blocking, c.state) {
        (Some(b), _) => b.clone(),
        (None, CandidateState::Allowed) => "installers are review-only: choose them one by one or by source".into(),
        (None, _) => "evidence incomplete".into(),
    }
}

pub fn rebuild_word(r: cm_core::rules::RebuildClass) -> &'static str {
    match r {
        cm_core::rules::RebuildClass::LocalRebuild => "local_rebuild",
        cm_core::rules::RebuildClass::NetworkRedownload => "network_redownload",
        cm_core::rules::RebuildClass::NotRebuildable => "not_rebuildable",
    }
}

fn known_bytes(c: &Candidate) -> Option<String> {
    (c.logical.known.0 > 0 || c.logical.unknown_count == 0).then(|| c.logical.known.0.to_string())
}

fn catalog_item(set_id: &str, c: &Candidate, tier_reason: String, rebuild: Option<&str>, notes: Vec<String>, group_label: String) -> CatalogItemDto {
    let display = c.path.display();
    let ok = plannable(c);
    CatalogItemDto {
        candidate_set_id: set_id.into(),
        candidate_id: c.candidate_id.clone(),
        entry_id: c.entry_id.to_string(),
        name: last_component(&display).to_owned(),
        path_display: display.clone(),
        tier: enum_text(&c.tier),
        tier_reason,
        skip_reason: skip_reason(c).map(str::to_owned),
        plannable: ok,
        selected_by_default: selected_by_default(c),
        allowed_modes: if ok { c.allowed_modes.clone() } else { vec![] },
        logical_bytes: known_bytes(c),
        unknown_size_count: c.logical.unknown_count,
        rebuild_class: rebuild.map(str::to_owned),
        impact: c.impact.clone(),
        notes,
        group_label,
    }
}

pub fn purge_item(set_id: &str, c: &Candidate, f: &cm_core::purge::PurgeFacts, recent_days: u32) -> CatalogItemDto {
    let artifact = c.rules.first().and_then(|r| cm_core::purge::artifact_for_rule(&r.rule_id));
    let project = f.project.display();
    let mut notes = vec![format!("project: {project} ({})", f.markers.join(", "))];
    if let Some(a) = artifact {
        notes.push(format!("to get it back: {}", a.restore));
    }
    if f.recent {
        notes.push(format!("project modified within {recent_days} days"));
    }
    catalog_item(set_id, c, purge_tier_reason(c, artifact.map(|a| a.class)), Some(rebuild_word(f.rebuild_class)), notes, project)
}

pub fn installer_item(set_id: &str, c: &Candidate, f: &cm_core::installer::InstallerFacts, fixture: bool) -> CatalogItemDto {
    let mut notes = vec![f.recognition_note()];
    notes.extend(f.in_use_note());
    if f.content_verified {
        notes.push(crate::plat::signer_note(f.kind, f.signer.as_deref()));
    }
    if let Some(h) = &f.installed_hint {
        notes.push(h.clone());
    }
    if fixture {
        notes.push("location from CM_INSTALLER_LOCATIONS (validation fixture), not a declared user folder".into());
    }
    if f.mounted == Some(true) {
        notes.push(crate::plat::EJECT_NOTE.into());
    }
    catalog_item(set_id, c, installer_tier_reason(c), Some("network_redownload"), notes, f.source_label.clone())
}

pub fn summarize<'a>(candidates: impl IntoIterator<Item = &'a Candidate>) -> CatalogSummaryDto {
    let mut out = CatalogSummaryDto::default();
    let mut known: u128 = 0;
    for c in candidates {
        if selected_by_default(c) {
            out.default_selected_count += 1;
            known += u128::from(c.logical.known.0);
            out.default_selected_unknown_count += c.logical.unknown_count;
        }
        match c.tier {
            Tier::ReviewOnly => out.review_only_count += 1,
            Tier::Blocked => out.blocked_count += 1,
            _ => {}
        }
        if skip_reason(c).is_some() {
            out.skipped_count += 1;
        }
    }
    out.default_selected_known_bytes = known.to_string();
    out
}

fn empty_preview(catalog: &'static str, supported: bool) -> CatalogPreviewDto {
    CatalogPreviewDto { supported, catalog, observed_at: now_utc(), sets: vec![], items: vec![], summary: summarize([]), issues: vec![] }
}

// ---------------------------------------------------------------------------------------------
// Purge roots

#[derive(Debug, Clone, Serialize)]
pub struct PurgeRootDto {
    pub root_id: String,
    pub display: String,
    /// `default` or `user`.
    pub source: String,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PurgeRootsDto {
    pub supported: bool,
    pub roots: Vec<PurgeRootDto>,
    pub recent_days: u32,
    pub max_depth: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallerLocationDto {
    pub display: String,
    pub source_label: String,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallerLocationsDto {
    pub supported: bool,
    pub locations: Vec<InstallerLocationDto>,
    pub max_depth: u32,
}

/// Which built-in catalog a plan request names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Catalog {
    Purge,
    Installer,
}

impl Catalog {
    pub fn parse(s: &str) -> IpcResult<Catalog> {
        match s {
            "purge" => Ok(Catalog::Purge),
            "installer" => Ok(Catalog::Installer),
            other => Err(ipc(ErrorCode::InvalidArgument, "invalid_argument", format!("unknown catalog {other:?}; expected purge or installer"))),
        }
    }

    /// The set was produced by this catalog as built into this binary (the CLI's `is_purge` / `is_installer_set`).
    pub fn owns(self, set: &CandidateSet) -> bool {
        let (current, prefix) = match self {
            Catalog::Purge => (cm_core::purge::is_current_digest(&set.rule_set_digest), cm_core::purge::RULE_PREFIX),
            Catalog::Installer => (cm_core::installer::is_current_digest(&set.rule_set_digest), cm_core::installer::RULE_PREFIX),
        };
        current && set.candidates.iter().all(|c| c.rules.iter().all(|r| r.rule_id.starts_with(prefix)))
    }
}

/// Discovery, manual trash, startup, insights and status over the platform backends of `crate::plat`.
pub(crate) mod imp {
    use std::path::Path;

    use cm_app::scan::{ScanRequest, run_scan};
    use cm_core::analysis::{AnalysisProtection, Candidate, CandidateSet, IndexView, UserList};
    use cm_core::error::ErrorCode;
    use cm_core::host::HostProbe;
    use cm_core::manual_trash::{BIN_NAME, Inspection, ManualError, ManualInput, SelectionGuard, VIEW_ONLY_EXPLANATION, build_trash_plan};
    use cm_core::path::RawPath;
    use cm_core::purge::{PurgeInput, PurgeSettings, analyze_purge};
    use cm_core::rules::Mode;
    use cm_core::scan::ScanOptions;
    use cm_platform::host::OsHostProbe;
    use crate::plat::os::purge::{FsProbe, default_roots, load_settings, os, save_settings};
    use cm_store::Stores;
    use tauri::AppHandle;
    use tauri_plugin_dialog::DialogExt;

    use super::*;
    use crate::host::AppState;

    fn settings(dir: &Path) -> IpcResult<PurgeSettings> {
        load_settings(dir).map_err(|e| ipc(ErrorCode::InternalError, "purge_settings_invalid", format!("{e}; fix or remove {}", cm_core::purge::SETTINGS_FILE)))
    }

    fn roots_dto(s: &PurgeSettings) -> PurgeRootsDto {
        let roots = s
            .roots(&default_roots())
            .into_iter()
            .map(|(path, source)| PurgeRootDto {
                root_id: root_id(&path),
                display: path.display(),
                source: enum_text(&source),
                exists: os(&path).is_some_and(|p| p.is_dir()),
            })
            .collect();
        PurgeRootsDto { supported: true, roots, recent_days: s.recent_days, max_depth: s.max_depth }
    }

    pub fn purge_roots(s: &AppState) -> IpcResult<PurgeRootsDto> {
        Ok(roots_dto(&settings(s.store_dir()?)?))
    }

    /// Validate and add a folder picked in the native dialog, as `purge roots add --path` does.
    pub fn purge_roots_add(app: &AppHandle, s: &AppState) -> IpcResult<Option<PurgeRootsDto>> {
        let dir = s.store_dir()?;
        let mut cfg = settings(dir)?;
        let Some(picked) = app.dialog().file().set_title("选择项目文件夹").blocking_pick_folder() else {
            return Ok(None);
        };
        let path = picked.into_path().map_err(|e| ipc(ErrorCode::InvalidArgument, "invalid_argument", format!("unsupported folder location: {e}")))?;
        if !path.is_dir() {
            return Err(ipc(ErrorCode::InvalidArgument, "not_directory", "This location is not an existing folder"));
        }
        let raw = crate::plat::os::purge::raw(&path).ok_or_else(|| ipc(ErrorCode::InvalidArgument, "invalid_argument", "invalid folder path"))?;
        if let Err(r) = cm_core::protect::prefilter(&raw) {
            return Err(ipc(ErrorCode::PolicyDenied, "not_project_root", format!("not a project root: {r}")));
        }
        if !cfg.add_root(raw, &default_roots()) {
            return Err(ipc(ErrorCode::InvalidArgument, "already_configured", "This folder is already a project root"));
        }
        save_settings(dir, &cfg).map_err(|e| ipc(ErrorCode::IoError, "io_error", format!("cannot save purge settings: {e}")))?;
        Ok(Some(roots_dto(&cfg)))
    }

    pub fn purge_roots_remove(s: &AppState, id: &str) -> IpcResult<PurgeRootsDto> {
        let dir = s.store_dir()?;
        let mut cfg = settings(dir)?;
        let defaults = default_roots();
        let Some((path, _)) = cfg.roots(&defaults).into_iter().find(|(p, _)| root_id(p) == id) else {
            return Err(ipc(ErrorCode::InvalidArgument, "unknown_root", "This root is no longer configured; reload the list"));
        };
        if !cfg.remove_root(&path, &defaults) {
            return Err(ipc(ErrorCode::InvalidArgument, "unknown_root", "This root is no longer configured; reload the list"));
        }
        save_settings(dir, &cfg).map_err(|e| ipc(ErrorCode::IoError, "io_error", format!("cannot save purge settings: {e}")))?;
        Ok(roots_dto(&cfg))
    }

    fn scan_id_for(tag: &str, i: usize) -> String {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        format!("s_desk_{tag}_{}_{nanos}_{i}", std::process::id())
    }

    /// The same scope id the CLI gives a discovery root.
    fn scope_for(root: &RawPath) -> String {
        format!("scope_{}", root_id(root))
    }

    /// The CLI's per-store record of the latest discovery (`purge-last-run.json` / `installer-last-run.json`), so
    /// `cleanmanager purge plan create` finds what the desktop discovered last.
    fn last_run_write(dir: &Path, file: &str, sets: &[(RawPath, String, String)]) {
        let v = serde_json::json!({"schema_version": 1, "sets": sets.iter().map(|(root, scan, set)| serde_json::json!({"root": root, "scan_id": scan, "set_id": set})).collect::<Vec<_>>()});
        let tmp = dir.join(format!("{file}.{}.tmp", std::process::id()));
        if std::fs::write(&tmp, v.to_string()).is_ok() && std::fs::rename(&tmp, dir.join(file)).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }

    struct Protection {
        hard: cm_core::protect::HardDenySet,
        user: Result<cm_core::protect::UserProtection, String>,
    }

    impl Protection {
        fn load(dir: &Path) -> Protection {
            Protection { hard: cm_platform::protection::hard_deny_set(), user: cm_platform::protection::load(dir).map_err(|e| e.to_string()) }
        }

        fn analysis(&self) -> AnalysisProtection<'_> {
            AnalysisProtection {
                hard_deny: Some(&self.hard),
                user: match &self.user {
                    Ok(list) => UserList::Loaded(list),
                    Err(why) => UserList::Unavailable(why),
                },
                worktrees: &[],
            }
        }

        fn issue(&self) -> Option<IssueDto> {
            self.user.as_ref().err().map(|p| {
                issue("protection_unavailable", cm_platform::protection::PROTECTION_FILE, format!("{p}; every candidate stays unplannable until the list is repaired"))
            })
        }
    }

    /// Scan one discovery root; `None` (with an issue) when it is skipped.
    fn scan_root(stores: &Stores, root: &RawPath, scan_id: &str, issues: &mut Vec<IssueDto>) -> IpcResult<Option<(cm_store::index::ScanStatusView, cm_core::analysis::MemView)>> {
        let display = root.display();
        let Some(path) = os(root).filter(|p| p.is_dir()) else {
            issues.push(issue("root_missing", &display, "This folder does not exist or its volume is offline; it was skipped"));
            return Ok(None);
        };
        if stores.ensure_outside(&path).is_err() {
            issues.push(issue("root_contains_store", &display, "This folder contains CleanManager's own data; it is never scanned"));
            return Ok(None);
        }
        let request = ScanRequest { scan_id: scan_id.into(), scope_id: scope_for(root), root: root.clone(), host: OsHostProbe.current("desktop") };
        if let Err(e) = run_scan(&stores.index, crate::plat::scan_backend().as_ref(), &request, &ScanOptions::default()) {
            issues.push(issue("scan_failed", &display, e.to_string()));
            return Ok(None);
        }
        Ok(Some((stores.index.scan_status(scan_id)?, stores.index.view(scan_id)?)))
    }

    /// `purge scan`: scan each configured root, analyze with the built-in catalog, store the sets, preview them.
    pub fn purge_scan(s: &AppState) -> IpcResult<CatalogPreviewDto> {
        let _busy = claim(s)?;
        let stores = own_stores(s)?;
        let cfg = settings(&stores.dir)?;
        let roots = cfg.roots(&default_roots());
        if roots.is_empty() {
            return Err(ipc(ErrorCode::InvalidArgument, "no_purge_roots", "No project roots are configured; add one first"));
        }
        let prot = Protection::load(&stores.dir);
        let mut out = empty_preview("purge", true);
        out.issues.extend(prot.issue());
        let now = now_unix();
        let mut runs = Vec::new();
        let mut all: Vec<Candidate> = Vec::new();
        for (i, (root, _)) in roots.iter().enumerate() {
            let scan_id = scan_id_for("purge", i);
            let Some((status, view)) = scan_root(&stores, root, &scan_id, &mut out.issues)? else { continue };
            let input = PurgeInput { scan_id: &scan_id, revision: status.revision, now_unix: now, recent_days: cfg.recent_days, max_depth: cfg.max_depth };
            let a = analyze_purge(&view, &input, &prot.analysis(), &FsProbe::default());
            stores.index.save_candidates(&a.set)?;
            let set_id = a.set.candidate_set_id.clone();
            out.items.extend(a.set.candidates.iter().zip(&a.facts).map(|(c, f)| purge_item(&set_id, c, f, cfg.recent_days)));
            out.sets.push(CatalogSetDto { candidate_set_id: set_id.clone(), scan_id: scan_id.clone(), root_display: root.display(), source_label: None });
            runs.push((root.clone(), scan_id, set_id));
            all.extend(a.set.candidates);
        }
        last_run_write(&stores.dir, "purge-last-run.json", &runs);
        out.summary = summarize(&all);
        out.observed_at = now_utc();
        Ok(out)
    }

    /// `CM_INSTALLER_LOCATIONS=label=PATH;label=PATH` replaces the declared locations (validation fixtures only).
    fn override_locations() -> Option<Vec<(RawPath, &'static str)>> {
        let v = std::env::var("CM_INSTALLER_LOCATIONS").ok()?;
        let labels = crate::plat::INSTALLER_LABELS;
        Some(
            v.split(';')
                .filter_map(|e| e.split_once('='))
                .filter_map(|(l, p)| labels.iter().find(|x| **x == l).and_then(|l| crate::plat::os::purge::raw(Path::new(p)).map(|r| (r, *l))))
                .collect(),
        )
    }

    fn locations() -> Vec<(RawPath, &'static str)> {
        override_locations().unwrap_or_else(crate::plat::os::installer::declared_locations)
    }

    pub fn installer_locations() -> InstallerLocationsDto {
        let locations = locations()
            .into_iter()
            .map(|(path, label)| InstallerLocationDto { display: path.display(), source_label: label.into(), exists: os(&path).is_some_and(|p| p.is_dir()) })
            .collect();
        InstallerLocationsDto { supported: true, locations, max_depth: cm_core::installer::DEFAULT_MAX_DEPTH }
    }

    /// `installer scan`: scan the declared locations (bounded depth), classify by content, store and preview.
    pub fn installer_scan(s: &AppState) -> IpcResult<CatalogPreviewDto> {
        let _busy = claim(s)?;
        let stores = own_stores(s)?;
        let fixture = override_locations().is_some();
        let prot = Protection::load(&stores.dir);
        let mut out = empty_preview("installer", true);
        out.issues.extend(prot.issue());
        let apps = crate::plat::os::installer::installed_apps();
        let probe = crate::plat::installer_probe();
        let mut runs = Vec::new();
        let mut all: Vec<Candidate> = Vec::new();
        for (i, (root, label)) in locations().iter().enumerate() {
            let scan_id = scan_id_for("inst", i);
            let Some((status, view)) = scan_root(&stores, root, &scan_id, &mut out.issues)? else { continue };
            let input = cm_core::installer::InstallerInput {
                scan_id: &scan_id,
                revision: status.revision,
                source_label: label,
                max_depth: cm_core::installer::DEFAULT_MAX_DEPTH,
                apps: &apps,
            };
            let a = cm_core::installer::analyze_installers(&view, &input, &prot.analysis(), &probe);
            stores.index.save_candidates(&a.set)?;
            if a.unclassified_placeholders > 0 {
                out.issues.push(issue("cloud_placeholders", &root.display(), format!("{} cloud-only file(s) were not opened, so they are not classified", a.unclassified_placeholders)));
            }
            let set_id = a.set.candidate_set_id.clone();
            out.items.extend(a.set.candidates.iter().zip(&a.facts).map(|(c, f)| installer_item(&set_id, c, f, fixture)));
            out.sets.push(CatalogSetDto { candidate_set_id: set_id.clone(), scan_id: scan_id.clone(), root_display: root.display(), source_label: Some((*label).into()) });
            runs.push((root.clone(), scan_id, set_id));
            all.extend(a.set.candidates);
        }
        last_run_write(&stores.dir, "installer-last-run.json", &runs);
        out.summary = summarize(&all);
        out.observed_at = now_utc();
        Ok(out)
    }

    /// Freeze chosen rows of a purge or installer set into a stored cleanup plan (authorizes nothing).
    pub fn catalog_plan(stores: &Stores, catalog: Catalog, set_id: &str, selected: &[String], mode: Mode) -> IpcResult<CreatedPlanDto> {
        let value = stores.index.candidate_set(set_id)?;
        let set: CandidateSet = serde_json::from_value(value).map_err(|e| ipc(ErrorCode::InternalError, "index_corrupt", e.to_string()))?;
        if !catalog.owns(&set) {
            return Err(ipc(ErrorCode::InvalidArgument, "wrong_catalog", "This candidate set was not produced by this discovery with the current catalog; run it again"));
        }
        // PRT-02: no plan while the protection list is unusable.
        if let Err(e) = cm_platform::protection::load(&stores.dir) {
            return Err(ipc(ErrorCode::ProtectionUnavailable, "protection_unavailable", format!("protection list unavailable: {e}")));
        }
        // WFA-58: a row that cannot be planned now (an attached image, an installer in use, a protected folder) is refused
        // for that reason before any live check; an attached image keeps changing, so it would only look changed.
        // A row a later operation already removed is refused too (the page leaves it out).
        let overlay = cm_app::removed::removed_overlay(stores, &set.scan_id)?;
        for id in selected {
            match set.candidates.iter().find(|c| &c.candidate_id == id) {
                None => return Err(ipc(ErrorCode::InvalidArgument, "unknown_candidate", format!("candidate {id} is not in this candidate set"))),
                Some(c) if !plannable(c) => {
                    let why = c.reasons.join("; ");
                    return Err(ipc(ErrorCode::PolicyDenied, "not_selectable", format!("candidate {id} cannot be planned ({}): {why}", skip_reason(c).unwrap_or("blocked"))));
                }
                Some(c) => crate::cleanup::already_removed(&overlay, c.entry_id, &c.path, id)?,
            }
        }
        // CLI-11 / WFA-50: rows whose observed state changed since discovery (a folder: any file the discovery saw in it)
        // are listed as stale, not planned (the CLI's check, `cm_app::selection`).
        let view = stores.index.view(&set.scan_id)?;
        let (keep, stale) = cm_app::selection::split_stale(&set.candidates, &view, selected.to_vec());
        let excluded: Vec<cm_core::plan::ExcludedItem> = stale.iter().map(cm_app::selection::Stale::excluded).collect();
        let stale: Vec<StaleItemDto> = stale.into_iter().map(|s| StaleItemDto { candidate_id: s.object_id, reason: s.reason, detail: s.detail }).collect();
        if keep.is_empty() {
            let list: Vec<String> = stale.iter().map(|s| format!("{}: {}", s.candidate_id, s.reason)).collect();
            return Err(ipc(ErrorCode::PlanStale, "selection_stale", format!("every selected item changed since discovery ({}); run the discovery again", list.join(", "))));
        }
        let status = stores.index.scan_status(&set.scan_id)?;
        let plan_id = new_id("p");
        let pdigest = crate::cleanup::policy_digest(stores);
        let input = cm_core::plan::PlanInput {
            plan_id: &plan_id,
            scope_id: &status.scope_id,
            created_unix: now_unix(),
            observed_at: status.finished_at.as_deref().unwrap_or(&status.started_at),
            policy_digest: &pdigest,
        };
        let mut plan = cm_core::plan::build_plan(&view, &set, &keep, mode, &input).map_err(crate::cleanup::plan_error)?;
        // R3-07: the left-out rows stay with the plan, so its review lists them with their reason.
        plan.excluded = excluded;
        // MTR-03 / WFA-35: trash mode needs a usable bin on every target's volume.
        crate::cleanup::bins_ready(&set, &keep, mode)?;
        stores.audit.save_plan(&plan)?;
        Ok(CreatedPlanDto { plan_id, stale_items: stale })
    }

    // -----------------------------------------------------------------------------------------
    // Manual trash

    struct Published {
        stores: Stores,
        status: cm_store::index::ScanStatusView,
        view: cm_core::analysis::MemView,
    }

    fn published(s: &AppState, scan_id: &str) -> IpcResult<Published> {
        let stores = own_stores(s)?;
        let status = stores.index.scan_status(scan_id)?;
        if !status.published {
            return Err(ipc(ErrorCode::InvalidArgument, "not_published", "The scan has not published results yet"));
        }
        let view = stores.index.view(scan_id)?;
        Ok(Published { stores, status, view })
    }

    fn known_entry(view: &cm_core::analysis::MemView, e: u64) -> IpcResult<u64> {
        if view.facts.contains_key(&e) { Ok(e) } else { Err(ipc(ErrorCode::InvalidArgument, "unknown_entry", format!("entry {e} is not in this scan"))) }
    }

    /// Processes holding the object (or a folder's files) open and a note on what could not be seen; never for cloud
    /// placeholders.
    fn holders_of(ins: &Inspection, view: &cm_core::analysis::MemView) -> (Option<Vec<String>>, Option<String>) {
        match cm_app::trash::holder_paths(ins, view) {
            Some(paths) => crate::plat::holders(&paths),
            None => (None, None),
        }
    }

    pub fn trash_inspect(s: &AppState, scan_id: &str, entry: u64) -> IpcResult<TrashInspectionDto> {
        let p = published(s, scan_id)?;
        let entry = known_entry(&p.view, entry)?;
        let hard = cm_platform::protection::hard_deny_set();
        let user = cm_platform::protection::load(&p.stores.dir);
        let problem = user.as_ref().err().map(ToString::to_string);
        let guard = SelectionGuard { hard: Some(&hard), user: user.as_ref().map_err(|_| problem.as_deref().unwrap_or("unavailable")) };
        // The CLI's inspection: the platform's refusals, and a link into a protected system location refused as
        // protected, naming where it leads (WFA-47/63).
        let (mut ins, link) = cm_app::trash::inspect_entry(&p.view, entry, &guard, &hard);
        let rb = crate::plat::bin_ready(&ins.path);
        // WFA-35 / NEW-05: a volume without a usable bin is not movable, said here rather than when the plan is made.
        if ins.refusal.is_none() {
            ins.refusal = cm_app::trash::bin_refusal(&ins.path, "recycle_bin_unavailable");
        }
        let (holders, holders_note) = holders_of(&ins, &p.view);
        let mut guidance = match &ins.refusal {
            None => format!("Moving it to the {BIN_NAME} creates a plan; a person reviews the plan before anything moves."),
            Some(r) if r.code == "policy_denied" && r.reason == "protected" => VIEW_ONLY_EXPLANATION.to_string(),
            Some(r) => r.message.clone(),
        };
        if ins.refusal.is_none()
            && let Some(hint) = crate::plat::PUT_BACK_HINT
        {
            guidance = format!("{guidance} {hint}");
        }
        if let Some(note) = holders_note {
            guidance = format!("{guidance} Holders: {note}.");
        }
        let hit = |h: &cm_core::manual_trash::Hit| PathReasonDto { path_display: h.path.display(), reason: h.reason.clone() };
        let display = ins.path.display();
        Ok(TrashInspectionDto {
            scan_id: p.status.scan_id.clone(),
            entry_id: entry.to_string(),
            name: last_component(&display).to_owned(),
            path_display: display.clone(),
            parent_display: p.view.parent(entry).map(|e| p.view.path(e).display()),
            object_kind: enum_text(&ins.kind),
            cloud: enum_text(&ins.cloud),
            logical_bytes: ins.logical_bytes.map(|b| b.to_string()),
            allocated_bytes: ins.allocated_bytes.map(|b| b.to_string()),
            files: ins.files,
            directories: ins.directories,
            protected: ins.protected.iter().map(hit).collect(),
            protected_count: ins.protected_count,
            coverage_gaps: ins.gaps.iter().map(hit).collect(),
            gap_count: ins.gap_count,
            trashable: ins.refusal.is_none(),
            refusal: ins.refusal.as_ref().map(|r| RefusalDto { code: r.code.into(), reason: r.reason.into(), message: r.message.clone() }),
            link_target_display: link.map(|l| l.target),
            holders,
            recycle_bin_ready: rb.is_ok(),
            recycle_bin_problem: rb.err(),
            guidance,
        })
    }

    fn manual_error(e: &ManualError) -> IpcError {
        let reason = match e {
            ManualError::Refused { refusal, .. } => refusal.reason,
            ManualError::Overlap { .. } => "overlap",
            ManualError::UnknownEntry(_) => "unknown_entry",
            ManualError::Empty => "invalid_argument",
            ManualError::Build(_) => "internal",
        };
        IpcError::new(e.code(), reason, e.to_string())
    }

    pub fn trash_plan(s: &AppState, scan_id: &str, entries: &[u64]) -> IpcResult<CreatedPlanDto> {
        let p = published(s, scan_id)?;
        let selected = entries.iter().map(|&e| known_entry(&p.view, e)).collect::<IpcResult<Vec<u64>>>()?;
        // An entry a later operation already removed is never planned again (the disk page does not offer it).
        let overlay = crate::removed::overlay(s, &p.stores, scan_id)?;
        for &e in &selected {
            crate::cleanup::already_removed(&overlay, e, &p.view.path(e), &format!("entry {e}"))?;
        }
        let hard = cm_platform::protection::hard_deny_set();
        let user = cm_platform::protection::load(&p.stores.dir);
        let problem = user.as_ref().err().map(ToString::to_string);
        let guard = SelectionGuard { hard: Some(&hard), user: user.as_ref().map_err(|_| problem.as_deref().unwrap_or("unavailable")) };
        // Objects the platform's trash action cannot take (macOS: folders) are refused before anything is planned.
        cm_app::trash::platform_plan_check(&p.view, &selected, &guard).map_err(|e| manual_error(&e))?;
        let plan_id = new_id("mp");
        let pdigest = crate::cleanup::policy_digest(&p.stores);
        let observed_at = p.status.finished_at.clone().unwrap_or_else(|| p.status.started_at.clone());
        let input = ManualInput {
            plan_id: &plan_id,
            scope_id: Some(&p.status.scope_id),
            scan_id: Some(&p.status.scan_id),
            scan_revision: Some(p.status.revision),
            created_unix: now_unix(),
            observed_at: &observed_at,
            policy_digest: &pdigest,
        };
        // WFA-47/63: a link into a protected system location is refused as protected, naming the location.
        let plan = build_trash_plan(&p.view, &selected, &guard, &input).map_err(|e| manual_error(&cm_app::trash::name_protected_link(&p.view, e, &hard)))?;
        // MTR-03 / WFA-35: a volume without a usable recycle bin fails the plan before it is stored (after the
        // protection refusals, as in the CLI); nothing is ever deleted instead.
        for &e in &selected {
            let path = p.view.path(e);
            if let Err(why) = crate::plat::bin_ready(&path) {
                return Err(ipc(ErrorCode::Unsupported, "recycle_bin_unavailable", format!("{}: {why}; there is no permanent-delete alternative", path.display())));
            }
        }
        p.stores.audit.save_plan_v2(&plan)?;
        Ok(CreatedPlanDto::new(plan_id))
    }

    // -----------------------------------------------------------------------------------------
    // The bin (MTR-03, WFA-65): the current user's items and a separate plan of permanent deletes

    /// Opaque, stable id of a bin folder (one per volume): the first 16 hex digits of SHA-256 over its path.
    fn bin_id(dir: &Path) -> String {
        let hex = cm_core::adapter::sha256_hex(dir.to_string_lossy().as_bytes()).unwrap_or_default();
        hex.get(..16).unwrap_or_default().to_owned()
    }

    /// The volume (Windows: `X:\`; macOS: the home folder or `/Volumes/X`) a bin folder belongs to, for people.
    fn bin_volume(dir: &Path) -> String {
        let up = if dir.file_name().is_some_and(|n| n == ".Trash") { dir.parent() } else { dir.ancestors().nth(2) };
        up.unwrap_or(dir).display().to_string()
    }

    /// The listing as the page shows it: newest first (unknown deletion time last), at most [`MAX_BIN_ITEMS`].
    pub fn bin_list_dto(bins: &[cm_app::trash::Bin], issues: &[cm_app::trash::BinIssue]) -> BinListDto {
        let mut items: Vec<BinItemDto> = bins
            .iter()
            .flat_map(|b| {
                let id = bin_id(&b.dir);
                b.items.iter().map(move |i| {
                    let location = b.location(i);
                    let name = i.original_path.as_deref().map(last_component).filter(|n| !n.is_empty()).unwrap_or_else(|| last_component(&location)).to_owned();
                    BinItemDto {
                        trash_item_id: i.trash_item_id.clone(),
                        bin_id: id.clone(),
                        name,
                        original_path_display: i.original_path.clone(),
                        bin_location_display: location.clone(),
                        deleted_at: i.deleted_at.clone(),
                        object_kind: enum_text(&i.kind),
                        logical_bytes: i.logical_bytes.map(|b| b.to_string()),
                        purgeable: i.refusal.is_none(),
                        refusal: i.refusal.clone(),
                    }
                })
            })
            .collect();
        items.sort_by(|a, b| b.deleted_at.is_some().cmp(&a.deleted_at.is_some()).then_with(|| b.deleted_at.cmp(&a.deleted_at)).then_with(|| a.name.cmp(&b.name)));
        let total = items.len() as u64;
        items.truncate(MAX_BIN_ITEMS);
        BinListDto {
            supported: true,
            observed_at: bins.iter().map(|b| b.observed_at.clone()).max().unwrap_or_else(now_utc),
            bins: bins.iter().map(|b| BinDto { bin_id: bin_id(&b.dir), volume_display: bin_volume(&b.dir), item_count: b.items.len() as u64 }).collect(),
            total,
            truncated: total > items.len() as u64,
            items,
            issues: issues.iter().map(|i| issue(i.code, &i.scope_display, i.message.clone())).collect(),
        }
    }

    fn bins() -> IpcResult<(Vec<cm_app::trash::Bin>, Vec<cm_app::trash::BinIssue>)> {
        cm_app::trash::list_bins().map_err(|e| {
            let reason = if e.code == ErrorCode::PermissionRequired { "permission_required" } else { "io_error" };
            let message = match e.required_action {
                Some(next) => format!("{} {next}", e.message),
                None => e.message,
            };
            IpcError::new(e.code, reason, message)
        })
    }

    /// `trash list`: read-only (the bin's own records, no content read).
    pub fn bin_list() -> IpcResult<BinListDto> {
        let (bins, issues) = bins()?;
        Ok(bin_list_dto(&bins, &issues))
    }

    /// `trash purge --item`: listed items of one bin frozen into a plan of permanent deletes (authorizes nothing).
    pub fn bin_purge_plan(stores: &Stores, ids: &[String]) -> IpcResult<CreatedPlanDto> {
        let (bins, _) = bins()?;
        purge_from(stores, &bins, ids)
    }

    pub fn purge_from(stores: &Stores, bins: &[cm_app::trash::Bin], ids: &[String]) -> IpcResult<CreatedPlanDto> {
        use cm_app::trash::PickError;
        let (bin, chosen) = cm_app::trash::pick(bins, ids).map_err(|e| match e {
            PickError::NotListed => ipc(ErrorCode::InvalidArgument, "bin_item_gone", format!("none of these items is in your {BIN_NAME} any more; reload the list")),
            PickError::OtherBin(id) => ipc(ErrorCode::InvalidArgument, "bin_items_mixed", format!("item {id} is in another volume's {BIN_NAME}; plan each volume separately")),
        })?;
        let plan_id = new_id("mp");
        let pdigest = crate::cleanup::policy_digest(stores);
        let plan = cm_app::trash::purge_plan(bin, &chosen, &plan_id, &pdigest, now_unix()).map_err(|e| manual_error(&e))?;
        stores.audit.save_plan_v2(&plan)?;
        Ok(CreatedPlanDto::new(plan_id))
    }

    // -----------------------------------------------------------------------------------------
    // Startup items

    #[cfg(windows)]
    fn env() -> IpcResult<cm_platform::windows::startup::StartupEnv> {
        cm_platform::windows::startup::StartupEnv::from_process().map_err(|e| ipc(ErrorCode::InvalidArgument, "invalid_argument", e))
    }

    #[cfg(windows)]
    pub fn startup_list() -> IpcResult<StartupListDto> {
        let env = env()?;
        let (items, issues) = cm_platform::windows::startup::inventory_with(&env);
        Ok(StartupListDto {
            supported: true,
            is_admin: env.is_admin,
            items: items.iter().map(|i| i.to_wire()).collect(),
            issues: issues.iter().map(|i| issue(&i.code, &i.source, i.message.clone())).collect(),
        })
    }

    #[cfg(windows)]
    pub fn startup_plan(stores: &Stores, item_id: &str, operation: cm_core::plan::v2::StartupOperation) -> IpcResult<CreatedPlanDto> {
        use cm_core::startup::StartupActions;
        let env = env()?;
        let (items, _) = cm_platform::windows::startup::inventory_with(&env);
        let Some(item) = items.iter().find(|i| i.item_id == item_id) else {
            return Err(ipc(ErrorCode::InvalidArgument, "unknown_item", "This startup item is no longer listed; reload the list"));
        };
        // Refuse before reading anything else (protected, no mechanism, not an administrator, wrong state).
        cm_core::startup::change_decision(item, operation, env.is_admin).map_err(|r| refusal(&r))?;
        let ws = cm_platform::windows::startup_actions::WindowsStartup { env: env.clone() };
        let obs = ws.observe(item.location.as_ref().expect("checked by change_decision")).map_err(|e| ipc(ErrorCode::IoError, "io_error", e))?;
        let plan_id = new_id("sp");
        let pdigest = crate::cleanup::policy_digest(stores);
        let plan = cm_core::startup::build_startup_plan(item, operation, &obs, env.is_admin, &plan_id, now_unix(), &pdigest).map_err(|r| refusal(&r))?;
        stores.audit.save_plan_v2(&plan)?;
        Ok(CreatedPlanDto::new(plan_id))
    }

    /// macOS: the read-only launchd / Service Management inventory (`startup list`).
    #[cfg(target_os = "macos")]
    fn mac_inventory() -> IpcResult<(Vec<cm_core::startup::StartupItem>, Vec<cm_core::apps::InventoryIssue>)> {
        let env = cm_platform::macos::startup::StartupEnv::from_process().map_err(|e| ipc(ErrorCode::InvalidArgument, "invalid_argument", e))?;
        Ok(cm_platform::macos::startup::inventory_with(&env))
    }

    #[cfg(target_os = "macos")]
    pub fn startup_list() -> IpcResult<StartupListDto> {
        let (items, issues) = mac_inventory()?;
        Ok(StartupListDto {
            supported: true,
            is_admin: false,
            items: items.iter().map(|i| i.to_wire()).collect(),
            issues: issues.iter().map(|i| issue(&i.code, &i.source, i.message.clone())).collect(),
        })
    }

    /// macOS: no item toggles in this build (as `cleanmanager startup disable|enable`). The item is looked up so the
    /// refusal names the real reason (protected: `policy_denied`; otherwise `unsupported`); no plan is ever created.
    #[cfg(target_os = "macos")]
    pub fn startup_plan(_: &Stores, item_id: &str, operation: cm_core::plan::v2::StartupOperation) -> IpcResult<CreatedPlanDto> {
        let (items, _) = mac_inventory()?;
        let Some(item) = items.iter().find(|i| i.item_id == item_id) else {
            return Err(ipc(ErrorCode::InvalidArgument, "unknown_item", "This startup item is no longer listed; reload the list"));
        };
        cm_core::startup::change_decision(item, operation, false).map_err(|r| refusal(&r))?;
        Err(ipc(ErrorCode::Unsupported, "unsupported", cm_platform::macos::startup::TOGGLE_UNSUPPORTED))
    }

    fn refusal(r: &cm_core::startup::Refusal) -> IpcError {
        IpcError::new(refusal_code(r.code), r.code, r.message.clone())
    }

    pub fn insights() -> InsightsDto {
        InsightsDto { supported: true, observed_at: now_utc(), items: crate::plat::os::insights::list().iter().map(|i| i.to_wire()).collect() }
    }

    /// The sampler (Windows: PDH handles, not `Send`; macOS: the previous readings) lives on one thread and answers requests over a channel, so
    /// CPU and throughput rates cover the interval since the previous snapshot. With sustained-CPU alerts turned on
    /// (STS-05, opt-in) the same thread also samples per-process CPU and runs the alert engine while the status pages
    /// ask for snapshots; alerts raised are returned with the snapshot. With alerts off no process is sampled.
    pub fn status(alerts: cm_core::alerts::AlertSettings) -> IpcResult<serde_json::Value> {
        use std::sync::mpsc::{Sender, channel};
        use std::sync::{Mutex, OnceLock};
        type Request = (Sender<serde_json::Value>, cm_core::alerts::AlertSettings);
        static SAMPLER: OnceLock<Mutex<Sender<Request>>> = OnceLock::new();
        let tx = SAMPLER.get_or_init(|| {
            let (tx, rx) = channel::<Request>();
            let _ = std::thread::Builder::new().name("cm-status".into()).spawn(move || {
                let mut sampler = crate::plat::os::status::Sampler::new();
                let mut engine: Option<(cm_core::alerts::AlertSettings, cm_core::alerts::AlertEngine)> = None;
                while let Ok((reply, settings)) = rx.recv() {
                    let mut v = sampler.sample().to_wire();
                    if engine.as_ref().map(|(s, _)| s) != Some(&settings) {
                        engine = Some((settings.clone(), cm_core::alerts::AlertEngine::new(settings)));
                    }
                    let mut raised = Vec::new();
                    if let Some((_, e)) = engine.as_mut()
                        && e.needs_sampling()
                        && let Some(sample) = crate::plat::process_cpu()
                    {
                        let now = cm_core::plan::parse_utc(&now_utc()).unwrap_or(0);
                        raised = e.observe(now, &sample).into_iter().filter_map(|a| serde_json::to_value(a).ok()).collect();
                    }
                    if let Some(o) = v.as_object_mut() {
                        o.insert("alerts".into(), serde_json::Value::Array(raised));
                    }
                    let _ = reply.send(v);
                }
            });
            Mutex::new(tx)
        });
        let (reply, answer) = channel();
        let unavailable = || ipc(ErrorCode::InternalError, "internal", "the status sampler is not running");
        crate::host::lock(tx).send((reply, alerts)).map_err(|_| unavailable())?;
        let mut v = answer.recv().map_err(|_| unavailable())?;
        if let Some(o) = v.as_object_mut() {
            o.insert("supported".into(), serde_json::Value::Bool(true));
        }
        Ok(v)
    }

    /// Show the entry selected in File Explorer (Windows) or the Finder (macOS). The path comes from the published
    /// index; no shell is involved.
    pub fn reveal(s: &AppState, scan_id: &str, entry: u64) -> IpcResult<bool> {
        let item = s.with_index(|idx| Ok(idx.get_entry(scan_id, entry)?))?;
        let Some(path) = os(&item.path) else { return Ok(false) };
        if std::fs::symlink_metadata(&path).is_err() {
            return Ok(false);
        }
        Ok(crate::plat::reveal(&path))
    }
}

// ---------------------------------------------------------------------------------------------
// Manual trash, startup, insights DTOs

#[derive(Debug, Clone, Serialize)]
pub struct PathReasonDto {
    pub path_display: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RefusalDto {
    pub code: String,
    pub reason: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrashInspectionDto {
    pub scan_id: String,
    pub entry_id: String,
    pub name: String,
    pub path_display: String,
    pub parent_display: Option<String>,
    pub object_kind: String,
    pub cloud: String,
    pub logical_bytes: Option<String>,
    pub allocated_bytes: Option<String>,
    pub files: u64,
    pub directories: u64,
    pub protected: Vec<PathReasonDto>,
    pub protected_count: u64,
    pub coverage_gaps: Vec<PathReasonDto>,
    pub gap_count: u64,
    pub trashable: bool,
    pub refusal: Option<RefusalDto>,
    /// WFA-47/63: where a link refused as `protected` leads (the protected system location), for people.
    pub link_target_display: Option<String>,
    /// Processes holding it open; `None` when not determined (or a cloud placeholder, never opened).
    pub holders: Option<Vec<String>>,
    pub recycle_bin_ready: bool,
    pub recycle_bin_problem: Option<String>,
    pub guidance: String,
}

/// Items per bin listing sent to the page (newest first); `total` counts them all.
pub const MAX_BIN_ITEMS: usize = 2000;

/// One object in the current user's Recycle Bin / Trash (`trash list`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BinItemDto {
    pub trash_item_id: String,
    /// The bin (volume) holding it; a permanent-delete plan takes items of one bin only.
    pub bin_id: String,
    /// The original name (from the bin's record), else the recycled object's own name.
    pub name: String,
    /// Where it was deleted from (the bin's record; display only); `None` when not recorded (macOS never claims it).
    pub original_path_display: Option<String>,
    /// Where the recycled object is now.
    pub bin_location_display: String,
    /// UTC (RFC 3339) when recorded.
    pub deleted_at: Option<String>,
    /// `file` or `directory`.
    pub object_kind: String,
    /// Decimal; `None` when unknown.
    pub logical_bytes: Option<String>,
    /// Whether a permanent-delete plan may include it.
    pub purgeable: bool,
    /// Why not (English detail; empty it with Explorer / Finder instead); `None` when purgeable.
    pub refusal: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BinDto {
    pub bin_id: String,
    /// The volume (Windows: `C:\`; macOS: the home folder or `/Volumes/X`).
    pub volume_display: String,
    pub item_count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BinListDto {
    pub supported: bool,
    pub observed_at: String,
    pub bins: Vec<BinDto>,
    /// Newest first, at most [`MAX_BIN_ITEMS`].
    pub items: Vec<BinItemDto>,
    pub total: u64,
    pub truncated: bool,
    /// Bin folders that could not be listed (`bin_unreadable`, `permission_required`), next to the others.
    pub issues: Vec<IssueDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StartupListDto {
    pub supported: bool,
    pub is_admin: bool,
    pub items: Vec<serde_json::Value>,
    pub issues: Vec<IssueDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InsightsDto {
    pub supported: bool,
    pub observed_at: String,
    pub items: Vec<serde_json::Value>,
}

/// Protocol error code of a startup refusal.
pub fn refusal_code(code: &str) -> ErrorCode {
    match code {
        "policy_denied" => ErrorCode::PolicyDenied,
        "unsupported" => ErrorCode::Unsupported,
        "permission_required" => ErrorCode::PermissionRequired,
        "conflict" => ErrorCode::Conflict,
        "internal_error" => ErrorCode::InternalError,
        _ => ErrorCode::InvalidArgument,
    }
}

// ---------------------------------------------------------------------------------------------
// Readiness (GUI-13)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadyOp {
    Scan,
    Cleanup,
    Purge,
    Installer,
    Trash,
    Startup,
    Apply,
    /// Uninstall stage 1 or its leftovers (plan creation).
    Uninstall,
    /// An update plan (plan creation).
    Update,
    /// A maintenance run (application: one UAC prompt for administrator tasks).
    Maintenance,
    /// Ending one process (plan creation).
    Process,
}

impl ReadyOp {
    pub fn parse(s: &str) -> IpcResult<ReadyOp> {
        Ok(match s {
            "scan" => ReadyOp::Scan,
            "cleanup" => ReadyOp::Cleanup,
            "purge" => ReadyOp::Purge,
            "installer" => ReadyOp::Installer,
            "trash" => ReadyOp::Trash,
            "startup" => ReadyOp::Startup,
            "apply" => ReadyOp::Apply,
            "uninstall" => ReadyOp::Uninstall,
            "update" => ReadyOp::Update,
            "maintenance" => ReadyOp::Maintenance,
            "process" => ReadyOp::Process,
            other => return Err(ipc(ErrorCode::InvalidArgument, "invalid_argument", format!("unknown operation {other:?}"))),
        })
    }

    fn writes(self) -> bool {
        !matches!(self, ReadyOp::Scan)
    }

    fn uses_recycle_bin(self) -> bool {
        matches!(self, ReadyOp::Trash | ReadyOp::Cleanup | ReadyOp::Purge | ReadyOp::Installer | ReadyOp::Uninstall)
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ReadinessCheckDto {
    /// `store`, `protection_list`, `administrator`, `bypass_privileges` or `recycle_bin`.
    pub check_id: &'static str,
    /// `ready`, `missing`, `unknown` or `not_applicable`.
    pub state: &'static str,
    pub blocking: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReadinessDto {
    pub platform: String,
    pub operation: String,
    pub is_admin: Option<bool>,
    pub checks: Vec<ReadinessCheckDto>,
}

/// What readiness is decided from (`None`: not determined).
pub struct ReadinessFacts {
    pub store: Result<(), String>,
    pub protection: Option<Result<(), String>>,
    pub is_admin: Option<bool>,
    pub bypass: Option<Result<(), String>>,
    pub recycle_bin: Option<Result<(), String>>,
}

fn check(check_id: &'static str, state: &'static str, blocking: bool, detail: Option<String>) -> ReadinessCheckDto {
    ReadinessCheckDto { check_id, state, blocking, detail }
}

fn tri(id: &'static str, v: &Option<Result<(), String>>, blocking_when_missing: bool) -> ReadinessCheckDto {
    match v {
        Some(Ok(())) => check(id, "ready", false, None),
        Some(Err(e)) => check(id, "missing", blocking_when_missing, Some(e.clone())),
        None => check(id, "unknown", false, None),
    }
}

/// The checks shown before a long operation; only a missing store or (for writes) protection list blocks it.
pub fn assemble(op: ReadyOp, f: &ReadinessFacts) -> Vec<ReadinessCheckDto> {
    let mut out = vec![match &f.store {
        Ok(()) => check("store", "ready", false, None),
        Err(e) => check("store", "missing", true, Some(e.clone())),
    }];
    out.push(tri("protection_list", &f.protection, op.writes()));
    let admin_note = match op {
        ReadyOp::Scan => Some("Some system folders may be unreadable without administrator rights; they are reported as not read"),
        ReadyOp::Startup => Some("System-wide startup items can be listed but not changed without administrator rights"),
        ReadyOp::Apply => Some("Plans that change system-wide items need CleanManager started as administrator (it never elevates itself)"),
        ReadyOp::Uninstall => Some("Leftovers in system-wide locations (HKLM, ProgramData) need CleanManager started as administrator; the rest can be planned"),
        ReadyOp::Process => Some("Processes of other users or the system may only be ended by an administrator; such plans are skipped when applied"),
        ReadyOp::Maintenance => Some("Administrator tasks ask Windows once (UAC) before any task runs; declining skips only those tasks"),
        _ => None,
    };
    out.push(match (admin_note, f.is_admin) {
        (None, _) => check("administrator", "not_applicable", false, None),
        (Some(_), Some(true)) => check("administrator", "ready", false, None),
        (Some(note), Some(false)) => check("administrator", "missing", false, Some(note.into())),
        (Some(_), None) => check("administrator", "unknown", false, None),
    });
    out.push(match &f.bypass {
        None => check("bypass_privileges", "not_applicable", false, None),
        Some(Ok(())) => check("bypass_privileges", "ready", false, None),
        Some(Err(e)) => check("bypass_privileges", "missing", false, Some(e.clone())),
    });
    out.push(if op.uses_recycle_bin() {
        let mut c = tri("recycle_bin", &f.recycle_bin, false);
        if c.state == "missing" {
            c.detail = c.detail.map(|d| format!("{d}; moves to the {} are refused (never turned into permanent deletes)", cm_core::manual_trash::BIN_NAME));
        }
        c
    } else {
        check("recycle_bin", "not_applicable", false, None)
    });
    out
}

fn readiness_of(s: &AppState, op_word: &str, op: ReadyOp) -> ReadinessDto {
    use cm_core::capability::CapabilityProvider;
    let store = s.with_stores(|_| Ok(())).map_err(|e| e.message);
    let protection = s.store_dir.as_deref().map(|d| cm_platform::protection::load(d).map(|_| ()).map_err(|e| e.to_string()));
    let is_admin = crate::v2::is_admin();
    let recycle_bin = op.uses_recycle_bin().then(crate::plat::bin_for_profile);
    let facts = ReadinessFacts { store, protection, is_admin, bypass: s.startup.bypass_privileges.clone(), recycle_bin };
    ReadinessDto { platform: enum_text(&cm_platform::host().platform()), operation: op_word.into(), is_admin, checks: assemble(op, &facts) }
}

// ---------------------------------------------------------------------------------------------
// IPC

#[tauri::command(rename_all = "snake_case")]
pub async fn purge_roots(state: St<'_>) -> IpcResult<PurgeRootsDto> {
    let s = state.inner().clone();
    blocking(move || imp::purge_roots(&s)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn purge_roots_add(app: AppHandle, state: St<'_>) -> IpcResult<Option<PurgeRootsDto>> {
    let s = state.inner().clone();
    blocking(move || imp::purge_roots_add(&app, &s)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn purge_roots_remove(state: St<'_>, root_id: String) -> IpcResult<PurgeRootsDto> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::record_id("root", &root_id)?;
        imp::purge_roots_remove(&s, &id)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn purge_scan(state: St<'_>) -> IpcResult<CatalogPreviewDto> {
    let s = state.inner().clone();
    blocking(move || imp::purge_scan(&s)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn installer_locations() -> IpcResult<InstallerLocationsDto> {
    blocking(move || Ok(imp::installer_locations())).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn installer_scan(state: St<'_>) -> IpcResult<CatalogPreviewDto> {
    let s = state.inner().clone();
    blocking(move || imp::installer_scan(&s)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn create_catalog_plan(state: St<'_>, catalog: String, candidate_set_id: String, candidate_ids: Vec<String>, mode: String) -> IpcResult<CreatedPlanDto> {
    let s = state.inner().clone();
    blocking(move || {
        let catalog = Catalog::parse(&catalog)?;
        let set = args::record_id("candidate set", &candidate_set_id)?;
        let ids = args::record_ids("candidate", &candidate_ids, MAX_SELECTION)?;
        let mode = args::parse_mode(&mode)?;
        s.with_stores(|st| imp::catalog_plan(st, catalog, &set, &ids, mode))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn trash_inspect(state: St<'_>, scan_id: String, entry_id: String) -> IpcResult<TrashInspectionDto> {
    let s = state.inner().clone();
    blocking(move || {
        let scan = args::scan_id(&scan_id)?;
        let entry = args::entry_id(&entry_id)?;
        imp::trash_inspect(&s, &scan, entry)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn trash_plan(state: St<'_>, scan_id: String, entry_ids: Vec<String>) -> IpcResult<CreatedPlanDto> {
    let s = state.inner().clone();
    blocking(move || {
        let scan = args::scan_id(&scan_id)?;
        if entry_ids.is_empty() || entry_ids.len() > MAX_TRASH_SELECTION {
            return Err(ipc(ErrorCode::InvalidArgument, "invalid_argument", format!("select 1..={MAX_TRASH_SELECTION} entries")));
        }
        let mut entries: Vec<u64> = Vec::with_capacity(entry_ids.len());
        for id in &entry_ids {
            let e = args::entry_id(id)?;
            if !entries.contains(&e) {
                entries.push(e);
            }
        }
        imp::trash_plan(&s, &scan, &entries)
    })
    .await
}

/// `trash list` (MTR-03, WFA-65): the current user's bin items; read-only.
#[tauri::command(rename_all = "snake_case")]
pub async fn trash_list() -> IpcResult<BinListDto> {
    blocking(imp::bin_list).await
}

/// `trash purge --item` (MTR-03): listed items of one bin frozen into a separate plan of permanent deletes. Only a
/// terminal review approves it (after `DELETE` is typed); this window never does (`v2::terminal_only`).
#[tauri::command(rename_all = "snake_case")]
pub async fn trash_purge_plan(state: St<'_>, item_ids: Vec<String>) -> IpcResult<CreatedPlanDto> {
    let s = state.inner().clone();
    blocking(move || {
        let ids = args::record_ids("recycle bin item", &item_ids, MAX_TRASH_SELECTION)?;
        s.with_stores(|st| imp::bin_purge_plan(st, &ids))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn startup_list() -> IpcResult<StartupListDto> {
    blocking(imp::startup_list).await
}

pub fn parse_startup_operation(s: &str) -> IpcResult<cm_core::plan::v2::StartupOperation> {
    match s {
        "disable" => Ok(cm_core::plan::v2::StartupOperation::Disable),
        "enable" => Ok(cm_core::plan::v2::StartupOperation::Enable),
        other => Err(ipc(ErrorCode::InvalidArgument, "invalid_argument", format!("unknown operation {other:?}; expected disable or enable"))),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn startup_plan(state: St<'_>, item_id: String, operation: String) -> IpcResult<CreatedPlanDto> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::record_id("startup item", &item_id)?;
        let op = parse_startup_operation(&operation)?;
        s.with_stores(|st| imp::startup_plan(st, &id, op))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn insights_list() -> IpcResult<InsightsDto> {
    blocking(move || Ok(imp::insights())).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn status_snapshot(state: St<'_>) -> IpcResult<serde_json::Value> {
    let s = state.inner().clone();
    blocking(move || {
        let alerts = s.store_dir.as_deref().map(cm_app::processes::load_alerts).unwrap_or_default();
        imp::status(alerts)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn readiness(state: St<'_>, operation: String) -> IpcResult<ReadinessDto> {
    let s = state.inner().clone();
    blocking(move || {
        let op = ReadyOp::parse(&operation)?;
        Ok(readiness_of(&s, &operation, op))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn reveal_entry(state: St<'_>, scan_id: String, entry_id: String) -> IpcResult<bool> {
    let s = state.inner().clone();
    blocking(move || {
        let scan = args::scan_id(&scan_id)?;
        let entry = args::entry_id(&entry_id)?;
        imp::reveal(&s, &scan, entry)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: &str, state: &str, tier: &str, reasons: &[&str], modes: &[&str], (known, unknown): (u64, u64), rule: &str) -> Candidate {
        let path = RawPath::windows("C:\\src\\app\\target".encode_utf16().collect()).unwrap();
        serde_json::from_value(serde_json::json!({
            "candidate_id": id, "entry_id": 7, "path": path, "kind": "directory",
            "rules": [{"rule_id": rule, "version": "1.0.0"}], "category": "build_output",
            "state": state, "tier": tier, "reasons": reasons, "protection": null,
            "logical": {"known": known.to_string(), "unknown_count": unknown},
            "allowed_modes": modes, "probes": [], "impact": "rebuilt", "owner_id": null,
        }))
        .expect("candidate shape")
    }

    #[test]
    fn root_ids_are_stable_opaque_and_distinct() {
        let a = RawPath::windows("C:\\src".encode_utf16().collect()).unwrap();
        let b = RawPath::windows("D:\\src".encode_utf16().collect()).unwrap();
        assert_eq!(root_id(&a), root_id(&a.clone()));
        assert_eq!(root_id(&a).len(), 16);
        assert!(root_id(&a).bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(root_id(&a), root_id(&b));
        assert!(args::record_id("root", &root_id(&a)).is_ok(), "passes IPC id validation");
    }

    #[test]
    fn tiers_and_skips_follow_the_cli_preview() {
        let ok = cand("a", "allowed", "default_selected", &[], &["trash"], (10, 0), "purge.target");
        assert!(plannable(&ok) && selected_by_default(&ok));
        assert_eq!(skip_reason(&ok), None);
        let recent = cand("b", "allowed", "review_only", &["recent_project: 2 days"], &["trash"], (5, 0), "purge.target");
        assert_eq!(skip_reason(&recent), Some("recent"));
        assert!(plannable(&recent) && !selected_by_default(&recent));
        assert_eq!(purge_tier_reason(&recent, None), "recent project: listed, not preselected");
        let tracked = cand("c", "blocked", "blocked", &["vcs_tracked: 3 files"], &[], (5, 0), "purge.target");
        assert_eq!(skip_reason(&tracked), Some("vcs_tracked"));
        assert!(!plannable(&tracked));
        assert_eq!(purge_tier_reason(&tracked, None), "vcs_tracked: 3 files");
        let hard = cand("d", "blocked", "blocked", &["hard_deny: system"], &[], (5, 0), "purge.target");
        assert_eq!(skip_reason(&hard), Some("hard_denied"));
        let pending = cand("e", "pending_evidence", "review_only", &["in_use_unknown"], &["trash"], (5, 0), "installer.exe");
        assert_eq!(skip_reason(&pending), Some("probe_unknown"));
        assert!(!plannable(&pending));
        assert_eq!(installer_tier_reason(&pending), "in_use_unknown", "the blocking reason is shown as recorded");
        assert_eq!(installer_tier_reason(&cand("g", "pending_evidence", "review_only", &["unread"], &[], (1, 0), "installer.exe")), "evidence incomplete");
        assert_eq!(installer_tier_reason(&cand("f", "allowed", "review_only", &[], &["trash"], (1, 0), "installer.exe")), "installers are review-only: choose them one by one or by source");
    }

    #[test]
    fn summary_counts_default_selection_and_keeps_unknown_apart() {
        let rows = [
            cand("a", "allowed", "default_selected", &[], &["trash"], (u64::MAX, 0), "purge.target"),
            cand("b", "allowed", "default_selected", &[], &["trash"], (u64::MAX, 2), "purge.target"),
            cand("c", "allowed", "review_only", &["recent_project"], &["trash"], (1, 0), "purge.target"),
            cand("d", "blocked", "blocked", &["vcs_tracked"], &[], (1, 0), "purge.target"),
        ];
        let s = summarize(&rows);
        assert_eq!(s.default_selected_count, 2);
        assert_eq!(s.default_selected_known_bytes, (2 * u128::from(u64::MAX)).to_string());
        assert_eq!(s.default_selected_unknown_count, 2);
        assert_eq!((s.review_only_count, s.blocked_count, s.skipped_count), (1, 1, 2));
        let item = catalog_item("cs", &rows[3], "x".into(), None, vec![], "g".into());
        assert!(!item.plannable && item.allowed_modes.is_empty());
        assert_eq!((item.tier.as_str(), item.name.as_str()), ("blocked", "target"));
        let unknown_only = cand("u", "allowed", "default_selected", &[], &["trash"], (0, 3), "purge.target");
        assert_eq!(known_bytes(&unknown_only), None, "unknown is never shown as 0");
        assert_eq!(known_bytes(&rows[2]), Some("1".into()));
    }

    #[test]
    fn catalog_ownership_needs_the_current_digest_and_prefix() {
        let set: CandidateSet = serde_json::from_value(serde_json::json!({
            "candidate_set_id": "cs_1", "scan_id": "s", "revision": 1, "rule_set_digest": cm_core::purge::rule_set_digest(7, 5), "protection_version": "v",
            "candidates": [cand("a", "allowed", "default_selected", &[], &["trash"], (1, 0), "purge.target")], "inapplicable": [],
        }))
        .unwrap();
        assert!(Catalog::Purge.owns(&set));
        assert!(!Catalog::Installer.owns(&set));
        assert_eq!(crate::cleanup::catalog_of(&set.rule_set_digest), Some("purge"));
        assert_eq!(crate::cleanup::catalog_of(&"0".repeat(64)), None);
        assert!(Catalog::parse("cleanup").is_err());
    }

    fn facts(store: bool, prot: Option<bool>, admin: Option<bool>, bin: Option<bool>) -> ReadinessFacts {
        let r = |ok: bool| if ok { Ok(()) } else { Err("no".to_string()) };
        ReadinessFacts { store: r(store), protection: prot.map(r), is_admin: admin, bypass: Some(Ok(())), recycle_bin: bin.map(r) }
    }

    fn by_id<'a>(c: &'a [ReadinessCheckDto], id: &str) -> &'a ReadinessCheckDto {
        c.iter().find(|x| x.check_id == id).expect("check present")
    }

    #[test]
    fn readiness_blocks_only_on_store_and_protection_for_writes() {
        let ids: Vec<&str> = assemble(ReadyOp::Scan, &facts(true, Some(true), Some(true), None)).iter().map(|c| c.check_id).collect();
        assert_eq!(ids, ["store", "protection_list", "administrator", "bypass_privileges", "recycle_bin"]);
        let scan = assemble(ReadyOp::Scan, &facts(true, Some(false), Some(false), None));
        assert!(scan.iter().all(|c| !c.blocking), "a scan only reads");
        assert_eq!(by_id(&scan, "administrator").state, "missing");
        assert_eq!(by_id(&scan, "recycle_bin").state, "not_applicable");
        let trash = assemble(ReadyOp::Trash, &facts(true, Some(false), Some(false), Some(false)));
        assert!(by_id(&trash, "protection_list").blocking);
        assert_eq!(by_id(&trash, "administrator").state, "not_applicable");
        let bin = by_id(&trash, "recycle_bin");
        assert_eq!((bin.state, bin.blocking), ("missing", false));
        let none = assemble(ReadyOp::Apply, &facts(false, None, None, None));
        assert!(by_id(&none, "store").blocking);
        assert_eq!(by_id(&none, "protection_list").state, "unknown");
        assert_eq!(by_id(&none, "administrator").state, "unknown");
        assert!(ReadyOp::parse("delete").is_err());
    }

    #[test]
    fn startup_words_parse_exactly() {
        assert!(parse_startup_operation("disable").is_ok());
        assert!(parse_startup_operation("enable").is_ok());
        assert!(parse_startup_operation("remove").is_err(), "stale removal stays in the CLI");
        assert_eq!(refusal_code("permission_required"), ErrorCode::PermissionRequired);
        assert_eq!(refusal_code("whatever"), ErrorCode::InvalidArgument);
    }
}
