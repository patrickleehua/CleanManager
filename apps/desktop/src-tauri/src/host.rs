//! Desktop task host (GUI-06, CLI-09). The scan runs on a background thread of this process
//! through the shared `cm_app::scan::run_scan`; a second thread reads the store at a fixed
//! interval and emits coalesced progress events (bounded rate, no per-file events). Page
//! navigation in the WebView never touches the scan; only `cancel_scan` or closing the window
//! requests a safe stop, and the published terminal state is reported as-is.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use cm_app::scan::{ScanRequest, run_scan};
use cm_core::error::ErrorCode;
use cm_core::host::HostProbe;
use cm_core::scan::ScanOptions;
use cm_core::time::now_utc;
use cm_platform::host::OsHostProbe;
use cm_store::{INDEX_FILE, IndexStore, Stores};
use tauri::{AppHandle, Emitter};

use crate::dto::{IpcError, IpcResult, ScanFinishedEvent, SummaryDto, display_of};

pub const EVENT_PROGRESS: &str = "scan-progress";
pub const EVENT_FINISHED: &str = "scan-finished";
pub const EVENT_CLOSING: &str = "app-closing";
const PROGRESS_INTERVAL: Duration = Duration::from_millis(500);
const SUMMARY_CACHE_CAP: usize = 256;

pub struct ActiveScan {
    pub scan_id: String,
    pub done: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct RootRegistry {
    next: u32,
    roots: HashMap<String, PathBuf>,
}

impl RootRegistry {
    /// Register a root offered to the WebView and return its opaque token. The WebView can
    /// only start scans on roots the host offered; it never supplies a path itself.
    pub fn offer(&mut self, path: PathBuf) -> String {
        if let Some((token, _)) = self.roots.iter().find(|(_, p)| **p == path) {
            return token.clone();
        }
        self.next += 1;
        let token = format!("r{}", self.next);
        self.roots.insert(token.clone(), path);
        token
    }

    pub fn resolve(&self, token: &str) -> Option<PathBuf> {
        self.roots.get(token).cloned()
    }
}

pub struct StartupInfo {
    pub bypass_privileges: Option<Result<(), String>>,
    pub store_problem: Option<IpcError>,
    pub interrupted: Vec<String>,
    /// Operations found without a live host at startup (now `reconciliation_required`).
    pub operations_to_reconcile: Vec<String>,
}

pub struct AppState {
    pub store_dir: Option<PathBuf>,
    pub startup: StartupInfo,
    /// Shared connections for queries and short writes (index + audit). Long-running work
    /// (scans, plan application) opens its own connections.
    stores: Mutex<Option<Stores>>,
    pub roots: Mutex<RootRegistry>,
    pub active: Mutex<Option<ActiveScan>>,
    summaries: Mutex<HashMap<(String, u32, u64), SummaryDto>>,
    /// Plan review sessions handed to the WebView with a plan's detail (see `cleanup`).
    pub reviews: Mutex<crate::cleanup::Reviews>,
    /// Last candidate set read, parsed once for paging.
    pub candidates: Mutex<Option<Arc<crate::cleanup::Loaded>>>,
    /// The plan application hosted by this process, if any (see `ops`).
    pub apply: Mutex<Option<crate::ops::ActiveApply>>,
    /// Where the platform rule pack is read from (see `rules`).
    pub rules_dir: Option<PathBuf>,
    /// Optional AI diagnosis (see `diag`); `None` without a store directory.
    pub diag: Option<Arc<cm_app::diag::Service>>,
    /// Review notes of plans created here and the last old-downloads review (see `software`, `system`).
    pub suite: Mutex<crate::software::SuiteCache>,
}

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while holding a lock must not take the whole UI down; the guarded values are
    // plain caches/registries that stay consistent between statements.
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn no_store() -> IpcError {
    IpcError::new(ErrorCode::InvalidArgument, "no_store_dir", "No per-user store directory is available (LOCALAPPDATA/HOME is not set)")
}

impl AppState {
    /// Drop ACL-bypass privileges, open the stores and mark scans whose host process exited
    /// as `interrupted` (their committed rows are published, never reported as completed).
    pub fn init(rules_dir: Option<PathBuf>) -> Self {
        let bypass_privileges = drop_bypass_privileges();
        Self::with_store(cm_platform::paths::default_store_dir(), rules_dir, bypass_privileges)
    }

    pub fn with_store(store_dir: Option<PathBuf>, rules_dir: Option<PathBuf>, bypass_privileges: Option<Result<(), String>>) -> Self {
        let mut store_problem = None;
        let mut interrupted = Vec::new();
        let mut operations_to_reconcile = Vec::new();
        let mut opened = None;
        match &store_dir {
            None => store_problem = Some(no_store()),
            Some(dir) => match Stores::open(dir) {
                // The audit store opens here, an existing index on first use (BUG-WFA-01): a damaged or incompatible
                // index becomes the startup store problem (`index_corrupt`/`index_incompatible`, code
                // `index_unavailable`, the message names `cleanmanager doctor --repair-index`) while history,
                // quarantine and recovery stay usable; only an audit-store problem leaves the stores closed.
                Ok(stores) => {
                    match stores.index.reconcile_hosts(|h| OsHostProbe.liveness(h), &now_utc()) {
                        Ok(ids) => interrupted = ids,
                        Err(e) => store_problem = Some(e.into()),
                    }
                    // Operations whose host exited need a person to reconcile them (GUI-06).
                    if let Ok(ids) = stores.audit.reconcile_hosts(|h| OsHostProbe.liveness(h)) {
                        operations_to_reconcile = ids;
                    }
                    opened = Some(stores);
                }
                Err(e) => store_problem = Some(e.into()),
            },
        }
        let diag = store_dir.as_deref().map(|d| Arc::new(cm_app::diag::Service::new(d, cm_platform::credentials::os_store())));
        AppState {
            store_dir,
            startup: StartupInfo { bypass_privileges, store_problem, interrupted, operations_to_reconcile },
            stores: Mutex::new(opened),
            roots: Mutex::new(RootRegistry::default()),
            active: Mutex::new(None),
            summaries: Mutex::new(HashMap::new()),
            reviews: Mutex::new(crate::cleanup::Reviews::default()),
            candidates: Mutex::new(None),
            apply: Mutex::new(None),
            diag,
            rules_dir,
            suite: Mutex::new(crate::software::SuiteCache::default()),
        }
    }

    /// Run `f` with the shared connections, opening them on first use.
    pub fn with_stores<T>(&self, f: impl FnOnce(&Stores) -> IpcResult<T>) -> IpcResult<T> {
        let mut guard = lock(&self.stores);
        if guard.is_none() {
            let dir = self.store_dir.as_ref().ok_or_else(no_store)?;
            *guard = Some(Stores::open(dir).map_err(IpcError::from)?);
        }
        f(guard.as_ref().expect("opened above"))
    }

    /// Run `f` with the shared index connection.
    pub fn with_index<T>(&self, f: impl FnOnce(&IndexStore) -> IpcResult<T>) -> IpcResult<T> {
        self.with_stores(|s| f(&s.index))
    }

    pub fn store_dir(&self) -> IpcResult<&Path> {
        self.store_dir.as_deref().ok_or_else(no_store)
    }

    pub fn cached_summary(&self, key: &(String, u32, u64)) -> Option<SummaryDto> {
        lock(&self.summaries).get(key).cloned()
    }

    pub fn store_summary(&self, key: (String, u32, u64), value: SummaryDto) {
        let mut cache = lock(&self.summaries);
        if cache.len() >= SUMMARY_CACHE_CAP {
            cache.clear();
        }
        cache.insert(key, value);
    }

    pub fn active_scan_id(&self) -> Option<(String, bool)> {
        lock(&self.active).as_ref().map(|a| (a.scan_id.clone(), a.done.load(Ordering::Acquire)))
    }

    pub fn active_running(&self) -> Option<(String, Arc<AtomicBool>)> {
        lock(&self.active).as_ref().filter(|a| !a.done.load(Ordering::Acquire)).map(|a| (a.scan_id.clone(), a.done.clone()))
    }
}

#[cfg(windows)]
fn drop_bypass_privileges() -> Option<Result<(), String>> {
    Some(cm_platform::windows::privileges::disable_bypass_privileges().map_err(|e| e.to_string()))
}

#[cfg(not(windows))]
fn drop_bypass_privileges() -> Option<Result<(), String>> {
    None
}

/// Why a root cannot be scanned, if anything (checked when offering and again when starting).
/// A root that *contains* CleanManager's store is fine: the scanner records the store as an
/// `own_store` boundary and reports it. A root at or inside the store is refused. The root itself
/// is inspected, never a link's target: a symbolic link or junction is refused like the scanner
/// refuses it (`root_is_link`), which also rechecks the opened handle when the scan starts.
pub fn root_problem(root: &Path, store_dir: Option<&Path>) -> Option<IpcError> {
    match std::fs::symlink_metadata(root) {
        // On Windows `is_symlink` covers every name-surrogate reparse point: symlinks and junctions.
        Ok(m) if m.file_type().is_symlink() => {
            return Some(IpcError::new(
                ErrorCode::InvalidArgument,
                cm_core::scan::issue::ROOT_IS_LINK,
                "This location is a link (symbolic link or junction); CleanManager never follows links. Choose the folder it points to instead",
            ));
        }
        Ok(m) if m.is_dir() => {}
        Ok(_) => return Some(IpcError::new(ErrorCode::InvalidArgument, cm_core::scan::issue::NOT_A_DIRECTORY, "This location is not a folder")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Some(IpcError::new(ErrorCode::InvalidArgument, "root_missing", "This folder does not exist or its volume is offline"));
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            return Some(IpcError::new(ErrorCode::PermissionRequired, "permission_required", "This folder cannot be read with ordinary permissions"));
        }
        Err(e) => return Some(IpcError::new(ErrorCode::IoError, "io_error", e.to_string())),
    }
    if let Some(store) = store_dir
        && let (Ok(s), Ok(r)) = (std::fs::canonicalize(store), std::fs::canonicalize(root))
        && r.starts_with(&s)
    {
        return Some(IpcError::new(
            ErrorCode::InvalidArgument,
            "root_inside_store",
            format!("This folder is part of CleanManager's own data ({}); choose another folder", display_of(store)),
        ));
    }
    None
}

fn new_scan_id() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("s_desk_{}_{}", std::process::id(), nanos)
}

fn scope_id_for(root: &Path) -> String {
    // Stable per root within this host; the CLI uses a digest, the desktop a display-based id.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in display_of(root).bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("scope_desk_{h:016x}")
}

/// Start one scan hosted by this process. Only one desktop-hosted scan runs at a time.
pub fn start(app: &AppHandle, state: &Arc<AppState>, root: PathBuf) -> IpcResult<(String, String)> {
    let dir = state.store_dir.clone().ok_or_else(no_store)?;
    if let Some(p) = root_problem(&root, Some(&dir)) {
        return Err(p);
    }
    let backend = crate::plat::scan_backend();
    let raw = cm_platform::paths::raw_path(root.as_os_str())
        .map_err(|e| IpcError::new(ErrorCode::InvalidArgument, "invalid_argument", format!("invalid root: {e}")))?;
    let root_display = raw.display();

    let mut active = lock(&state.active);
    if let Some(a) = active.as_ref()
        && !a.done.load(Ordering::Acquire)
    {
        return Err(IpcError::new(ErrorCode::Conflict, "scan_running", format!("Scan {} is still running; wait for it or cancel it first", a.scan_id)));
    }
    // The scan thread gets its own connection. The store directory is handed to the scanner,
    // which records it as an `own_store` boundary instead of reading its own writes.
    let stores = Stores::open(&dir).map_err(IpcError::from)?;
    let own_store: Vec<_> = cm_platform::paths::raw_path(dir.as_os_str()).ok().into_iter().collect();

    let scan_id = new_scan_id();
    let request = ScanRequest { scan_id: scan_id.clone(), scope_id: scope_id_for(&root), root: raw, host: OsHostProbe.current("desktop") };
    let done = Arc::new(AtomicBool::new(false));
    *active = Some(ActiveScan { scan_id: scan_id.clone(), done: done.clone() });
    drop(active);

    let scan_app = app.clone();
    let scan_done = done.clone();
    let scan_id_t = scan_id.clone();
    let spawned = std::thread::Builder::new().name("cm-scan".into()).spawn(move || {
        let options = ScanOptions { own_store_dirs: own_store, ..ScanOptions::default() };
        let outcome = run_scan(&stores.index, backend.as_ref(), &request, &options);
        scan_done.store(true, Ordering::Release);
        let event = match outcome {
            Ok(o) => ScanFinishedEvent { scan_id: scan_id_t, state: Some(o.state), error: None },
            Err(e) => ScanFinishedEvent { scan_id: scan_id_t, state: None, error: Some(e.into()) },
        };
        let _ = scan_app.emit(EVENT_FINISHED, event);
    });
    if let Err(e) = spawned {
        done.store(true, Ordering::Release);
        return Err(IpcError::new(ErrorCode::InternalError, "internal", format!("could not start scan thread: {e}")));
    }
    spawn_progress(app.clone(), dir, scan_id.clone(), done);
    Ok((scan_id, root_display))
}

/// Coalesced progress: one store read and at most one event per interval while the scan runs.
fn spawn_progress(app: AppHandle, dir: PathBuf, scan_id: String, done: Arc<AtomicBool>) {
    let _ = std::thread::Builder::new().name("cm-scan-progress".into()).spawn(move || {
        let Ok(index) = IndexStore::open(&dir.join(INDEX_FILE)) else { return };
        let mut last: Option<(String, u64, u64, bool)> = None;
        while !done.load(Ordering::Acquire) {
            std::thread::sleep(PROGRESS_INTERVAL);
            if let Ok(s) = index.scan_status(&scan_id) {
                let state = serde_json::to_value(s.state).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
                let key = (state.clone(), s.entries_seen, s.skipped_ranges, s.cancel_requested);
                if last.as_ref() != Some(&key) {
                    let _ = app.emit(
                        EVENT_PROGRESS,
                        serde_json::json!({
                            "scan_id": scan_id,
                            "state": state,
                            "entries_seen": s.entries_seen,
                            "skipped_ranges": s.skipped_ranges,
                            "cancel_requested": s.cancel_requested,
                            "heartbeat_at": s.heartbeat_at,
                        }),
                    );
                    last = Some(key);
                }
            }
        }
    });
}

/// Window close while a scan or an operation runs: request a safe stop (a scan publishes what
/// it confirmed; an operation stops scheduling new actions and journals the one in flight),
/// tell the WebView, and exit only afterwards — never claim work continues without a host.
pub fn close_requested(app: &AppHandle, state: &Arc<AppState>) -> bool {
    let scan = state.active_running();
    let apply = crate::ops::running_apply(state);
    if scan.is_none() && apply.is_none() {
        return false;
    }
    let mut waits: Vec<Arc<AtomicBool>> = Vec::new();
    let mut what = Vec::new();
    if let Some((scan_id, done)) = scan {
        let _ = state.with_index(|idx| idx.request_cancel(&scan_id).map_err(IpcError::from));
        what.push("scan");
        waits.push(done);
    }
    if let Some((cancel, done)) = apply {
        cancel.store(true, Ordering::Release);
        what.push("operation");
        waits.push(done);
    }
    let _ = app.emit(EVENT_CLOSING, serde_json::json!({ "waiting_for": what }));
    let app = app.clone();
    let _ = std::thread::Builder::new().name("cm-exit".into()).spawn(move || {
        while waits.iter().any(|d| !d.load(Ordering::Acquire)) {
            std::thread::sleep(Duration::from_millis(100));
        }
        app.exit(0);
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_hands_out_stable_opaque_tokens() {
        let mut r = RootRegistry::default();
        let a = r.offer(PathBuf::from("C:\\A"));
        let b = r.offer(PathBuf::from("C:\\B"));
        assert_ne!(a, b);
        assert_eq!(r.offer(PathBuf::from("C:\\A")), a);
        assert_eq!(r.resolve(&a), Some(PathBuf::from("C:\\A")));
        assert_eq!(r.resolve("r999"), None);
    }

    #[test]
    fn missing_root_is_reported_not_substituted() {
        let p = std::env::temp_dir().join("cm-desktop-test-definitely-missing-root");
        let e = root_problem(&p, None).expect("problem");
        assert_eq!(e.reason, "root_missing");
    }

    #[test]
    fn root_containing_the_store_is_allowed_but_not_one_inside_it() {
        let base = std::env::temp_dir().join(format!("cm-desktop-test-{}", std::process::id()));
        let store = base.join("store");
        std::fs::create_dir_all(store.join("quarantine")).unwrap();
        assert!(root_problem(&base, Some(&store)).is_none(), "the scanner skips the store as a boundary");
        assert_eq!(root_problem(&store, Some(&store)).map(|e| e.reason), Some("root_inside_store"));
        assert_eq!(root_problem(&store.join("quarantine"), Some(&store)).map(|e| e.reason), Some("root_inside_store"));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// WFA-16: a damaged index is reported at startup; the audit store (history, quarantine, recovery) stays open.
    #[test]
    fn a_damaged_index_is_a_startup_problem_and_history_stays_usable() {
        let base = std::env::temp_dir().join(format!("cm-desktop-damaged-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let store = base.join("store");
        {
            let stores = Stores::open(&store).unwrap();
            // A pid no process has (pid 1 is launchd on macOS: probing it unprivileged gives EPERM, i.e. "unknown").
            let host = cm_core::host::HostRef { kind: "cli".into(), pid: 0x7FFF_FFF0, started_at: "2026-10-22T00:00:00Z".into() };
            stores.audit.begin_operation("op1", "p1", "d", "k", &host, "2026-10-22T00:00:00Z").unwrap();
        }
        std::fs::write(store.join(INDEX_FILE), [0x42u8; 8192]).unwrap();
        let s = AppState::with_store(Some(store.clone()), None, None);
        let problem = s.startup.store_problem.clone().expect("the index problem is reported");
        assert_eq!((problem.code, problem.reason), (ErrorCode::IndexUnavailable, "index_corrupt"), "{problem:?}");
        assert!(problem.message.contains("doctor --repair-index"), "{}", problem.message);
        // The audit side of startup still ran: the operation whose host exited awaits reconciliation (GUI-06).
        assert_eq!(s.startup.operations_to_reconcile, ["op1".to_string()]);
        let state = s.with_stores(|st| st.audit.operation_state("op1").map_err(IpcError::from)).unwrap();
        assert_eq!(state, "reconciliation_required", "history is readable while the index is damaged");
        let e = s.with_index(|idx| idx.list_scans(5, None).map_err(IpcError::from)).unwrap_err();
        assert_eq!(e.reason, "index_corrupt");
        drop(s);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn scan_ids_pass_ipc_validation() {
        let id = new_scan_id();
        assert!(crate::args::scan_id(&id).is_ok(), "{id}");
    }
}
