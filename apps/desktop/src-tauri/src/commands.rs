//! Scan and index commands (P01–P03). Every command here is read-only with respect to user
//! files; `start_scan` and `cancel_scan` only write CleanManager's own index. The WebView never
//! supplies a filesystem path: scan roots are opaque tokens issued by `list_scan_roots` /
//! `choose_folder`. Cleanup, execution, recovery and settings commands live in `cleanup`,
//! `ops` and `settings`; the full allow-list is `build.rs` + `capabilities/main.json`.

use std::sync::Arc;

use cm_core::capability::CapabilityProvider;
use cm_core::error::ErrorCode;
use cm_store::StoreError;
use cm_store::index::{EntryQuery, Filter, KindFilter, MAX_PAGE, Sort};
use serde::Deserialize;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::args;
use crate::dto::*;
use crate::host::{self, AppState, root_problem};

pub(crate) type St<'a> = State<'a, Arc<AppState>>;

/// The scope root is the first entry the backend writes.
const ROOT_ENTRY_ID: u64 = 1;
/// Pages read (of `MAX_PAGE`) to total the children beyond the treemap tiles.
const REST_PAGE_BUDGET: u32 = 40;
const MAX_ISSUES: usize = 50;
const RECENT_SCANS: u32 = 30;

pub(crate) async fn blocking<T: Send + 'static>(f: impl FnOnce() -> IpcResult<T> + Send + 'static) -> IpcResult<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| IpcError::new(ErrorCode::InternalError, "internal", format!("worker failed: {e}")))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn capabilities(state: St<'_>) -> IpcResult<CapabilitiesDto> {
    let s = state.inner().clone();
    let host = cm_platform::host();
    let (disabled, note) = match &s.startup.bypass_privileges {
        None => (None, None),
        Some(Ok(())) => (Some(true), None),
        Some(Err(e)) => (Some(false), Some(e.clone())),
    };
    let rules = crate::rules::load(s.rules_dir.as_deref());
    let store = s.store_dir.as_deref();
    let mut capabilities = host.capabilities();
    cm_app::features::apply_to(store, &mut capabilities);
    Ok(CapabilitiesDto {
        platform: host.platform(),
        engine_version: cm_core::ENGINE_VERSION,
        schema_version: cm_core::SCHEMA_VERSION,
        capabilities,
        store_dir_display: s.store_dir.as_deref().map(display_of),
        store_problem: s.startup.store_problem.clone(),
        bypass_privileges_disabled: disabled,
        bypass_privileges_note: note,
        interrupted_at_startup: s.startup.interrupted.clone(),
        operations_to_reconcile: s.startup.operations_to_reconcile.clone(),
        write_commands: true,
        file_actions_verified: true,
        rules_dir_display: s.rules_dir.as_deref().map(display_of),
        rules_problem: rules.as_ref().err().cloned(),
        rule_count: rules.as_ref().ok().map(|r| r.rules.len()),
        principal: cm_core::authz::current_principal(),
        experimental_features: cm_app::features::load(store).experimental_features,
        gates: cm_app::features::rows(store),
    })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_scan_roots(state: St<'_>) -> IpcResult<Vec<RootOfferDto>> {
    let s = state.inner().clone();
    blocking(move || {
        let store = s.store_dir.clone();
        let offers = crate::roots::suggestions()
            .into_iter()
            .map(|sug| {
                let problem = root_problem(&sug.path, store.as_deref());
                let path_display = display_of(&sug.path);
                let token = s.roots.lock().unwrap_or_else(std::sync::PoisonError::into_inner).offer(sug.path);
                RootOfferDto { token, label: sug.label, kind: sug.kind, path_display, problem }
            })
            .collect();
        Ok(offers)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn choose_folder(app: AppHandle, state: St<'_>) -> IpcResult<Option<RootOfferDto>> {
    let s = state.inner().clone();
    blocking(move || {
        let Some(picked) = app.dialog().file().set_title("选择要扫描的文件夹").blocking_pick_folder() else {
            return Ok(None);
        };
        let path = picked
            .into_path()
            .map_err(|e| IpcError::new(ErrorCode::InvalidArgument, "invalid_argument", format!("unsupported folder location: {e}")))?;
        let problem = root_problem(&path, s.store_dir.as_deref());
        let path_display = display_of(&path);
        let label = last_component(&path_display).to_owned();
        let token = s.roots.lock().unwrap_or_else(std::sync::PoisonError::into_inner).offer(path);
        Ok(Some(RootOfferDto { token, label, kind: "chosen", path_display, problem }))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn start_scan(app: AppHandle, state: St<'_>, root_token: String) -> IpcResult<StartScanDto> {
    let s = state.inner().clone();
    blocking(move || {
        let token = args::root_token(&root_token)?;
        let root = s
            .roots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .resolve(&token)
            .ok_or_else(|| IpcError::new(ErrorCode::InvalidArgument, "unknown_root", "This scan location is no longer offered; choose it again"))?;
        let (scan_id, root_display) = host::start(&app, &s, root)?;
        Ok(StartScanDto { scan_id, root_display })
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn cancel_scan(state: St<'_>, scan_id: String) -> IpcResult<CancelDto> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::scan_id(&scan_id)?;
        s.with_index(|idx| {
            let r = idx.request_cancel(&id)?;
            Ok(CancelDto { scan_id: r.scan_id, accepted: r.accepted, state_at_receipt: r.state_at_receipt })
        })
    })
    .await
}

fn root_summary(s: &AppState, idx: &cm_store::IndexStore, scan_id: &str, revision: u32, entry: u64) -> IpcResult<SummaryDto> {
    let key = (scan_id.to_owned(), revision, entry);
    if let Some(v) = s.cached_summary(&key) {
        return Ok(v);
    }
    // One subtree, walked through the parent index; the same result as `summarize(&[entry])`.
    let v = SummaryDto::from(idx.folder_total(scan_id, entry)?);
    s.store_summary(key, v.clone());
    Ok(v)
}

pub fn status_of(s: &AppState, idx: &cm_store::IndexStore, id: &str) -> IpcResult<ScanStatusDto> {
    let active = s.active_scan_id();
    let hosted_here = matches!(&active, Some((a, false)) if a == id);
    let v = match idx.scan_status(id) {
        Ok(v) => v,
        // The scan thread has not created its record yet.
        Err(StoreError::NotFound(_)) if hosted_here => {
            return Ok(ScanStatusDto {
                scan_id: id.to_owned(),
                scope_id: None,
                state: cm_core::model::ScanState::Created,
                revision: None,
                published: false,
                completeness: None,
                cancel_requested: false,
                host_kind: Some("desktop".into()),
                host_pid: Some(std::process::id()),
                hosted_here,
                heartbeat_at: None,
                entries_seen: 0,
                skipped_ranges: 0,
                started_at: None,
                finished_at: None,
                root_display: None,
                root_entry_id: None,
                summary: None,
                backend: None,
                backend_reason: None,
                elevated_read: vec![],
                issues: vec![],
                issues_total: 0,
            });
        }
        Err(e) => return Err(e.into()),
    };
    let root_display = Some(v.root.display());
    let mut summary = None;
    let mut root_entry_id = None;
    // A failed scan reports no sizes (unknown, never 0), as its `scan_result` does (SCN-03).
    if v.published && v.entries_seen > 0 && v.state != cm_core::model::ScanState::Failed {
        summary = Some(root_summary(s, idx, id, v.revision, ROOT_ENTRY_ID)?);
        root_entry_id = Some(ROOT_ENTRY_ID.to_string());
    }
    let report = idx.scan_report(id)?.unwrap_or(serde_json::Value::Null);
    let all_issues = report["issues"].as_array().cloned().unwrap_or_default();
    let issues = all_issues
        .iter()
        .take(MAX_ISSUES)
        .map(|i| IssueDto {
            code: i["code"].as_str().unwrap_or("").to_owned(),
            scope_display: i["scope_display"].as_str().unwrap_or("").to_owned(),
            message: i["message"].as_str().unwrap_or("").to_owned(),
        })
        .collect();
    Ok(ScanStatusDto {
        scan_id: id.to_owned(),
        scope_id: Some(v.scope_id),
        state: v.state,
        revision: Some(v.revision),
        published: v.published,
        completeness: v.completeness,
        cancel_requested: v.cancel_requested,
        host_kind: Some(v.host.kind),
        host_pid: Some(v.host.pid),
        hosted_here,
        heartbeat_at: Some(v.heartbeat_at),
        entries_seen: v.entries_seen,
        skipped_ranges: v.skipped_ranges,
        started_at: Some(v.started_at),
        finished_at: v.finished_at,
        root_display,
        root_entry_id,
        summary,
        backend: report["backend"].as_str().map(str::to_owned),
        backend_reason: report["backend_reason"].as_str().map(str::to_owned),
        elevated_read: report["elevated_read"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default(),
        issues,
        issues_total: all_issues.len() as u64,
    })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn scan_status(state: St<'_>, scan_id: String) -> IpcResult<ScanStatusDto> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::scan_id(&scan_id)?;
        s.with_index(|idx| status_of(&s, idx, &id))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn recent_scans(state: St<'_>) -> IpcResult<Vec<RecentScanDto>> {
    let s = state.inner().clone();
    blocking(move || s.with_index(recent_of)).await
}

/// Scans recorded in the store (from this desktop or the CLI), newest first. The store is
/// the only source: there is no desktop-private list to drift from it.
pub fn recent_of(idx: &cm_store::IndexStore) -> IpcResult<Vec<RecentScanDto>> {
    let (items, _) = idx.list_scans(RECENT_SCANS, None)?;
    Ok(items
        .into_iter()
        .map(|r| RecentScanDto {
            scan_id: r.scan_id,
            root_display: r.root.display(),
            started_at: r.started_at,
            state: Some(r.state),
            published: r.published,
            host_kind: r.host_kind,
            completeness: r.completeness,
            finished_at: r.finished_at,
            problem: None,
        })
        .collect())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListEntriesArgs {
    pub scan_id: String,
    pub sort: String,
    pub kind: String,
    pub name_contains: Option<String>,
    pub parent_entry_id: Option<String>,
    pub page_size: Option<u32>,
    pub cursor: Option<String>,
}

/// Validate and convert WebView arguments into a store query (no store access).
pub fn entry_query(a: &ListEntriesArgs) -> IpcResult<EntryQuery> {
    Ok(EntryQuery {
        scan_id: args::scan_id(&a.scan_id)?,
        sort: args::parse_sort(&a.sort)?,
        filter: Filter {
            kind: args::parse_kind(&a.kind)?,
            name_contains: args::name_filter(a.name_contains.as_deref())?,
            parent_entry_id: a.parent_entry_id.as_deref().map(args::entry_id).transpose()?,
        },
        page_size: args::page_size(a.page_size)?,
        cursor: args::cursor(a.cursor.as_deref())?,
    })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_entries(state: St<'_>, query: ListEntriesArgs) -> IpcResult<EntriesPageDto> {
    let s = state.inner().clone();
    blocking(move || {
        let q = entry_query(&query)?;
        let page = s.with_index(|idx| Ok(idx.list_entries(&q)?))?;
        // Probed after the index is released: the page's directories only, never the whole scan.
        let hints = page_hints(&s, &page.items);
        Ok(EntriesPageDto {
            scan_id: page.scan_id,
            scope_id: page.scope_id,
            revision: page.revision,
            sort: page.sort,
            kind: page.filter.kind,
            name_contains: page.filter.name_contains,
            parent_entry_id: page.filter.parent_entry_id.map(|p| p.to_string()),
            page_size: page.page_size,
            items: page.items.iter().zip(hints).map(|(i, cleanable_hint)| EntryDto { cleanable_hint, ..EntryDto::from_item(i) }).collect(),
            next_cursor: page.next_cursor,
            unknown_size_items: page.unknown_size_items,
        })
    })
    .await
}

/// DIN-05 read-only "cleanable by hand" hints for one page: real directories only (links and
/// boundaries never), probed live; protection and unconfirmed evidence suppress them.
pub fn page_hints(s: &AppState, items: &[cm_store::index::EntryItem]) -> Vec<Option<cm_core::cleanable::CleanableHint>> {
    let dirs: Vec<Option<&cm_core::path::RawPath>> =
        items.iter().map(|i| (i.kind == cm_core::model::ObjectKind::Directory && i.boundary.is_none()).then_some(&i.path)).collect();
    if dirs.iter().all(Option::is_none) {
        return vec![None; items.len()];
    }
    let caches = crate::rules::load(s.rules_dir.as_deref()).map(|r| cm_platform::cleanable::cache_locations(&r.rules)).unwrap_or_default();
    cm_platform::cleanable::page_hints(&dirs, s.store_dir.as_deref(), &caches)
}

/// Running total of the children not drawn as tiles. u128 cannot overflow for any realistic
/// count of u64 values; the result is still checked.
#[derive(Default)]
struct RestAcc {
    count: u64,
    known: u128,
    unknown: u64,
}

impl RestAcc {
    fn add(&mut self, logical: Option<cm_core::bytes::Bytes>) {
        self.count += 1;
        match logical {
            Some(b) => self.known += u128::from(b.0),
            None => self.unknown += 1,
        }
    }

    fn finish(self, complete: bool) -> RestDto {
        RestDto { count: self.count, known_logical_bytes: self.known.to_string(), unknown_count: self.unknown, complete }
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn treemap_layer(state: St<'_>, scan_id: String, parent_entry_id: Option<String>, limit: Option<u32>) -> IpcResult<TreemapLayerDto> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::scan_id(&scan_id)?;
        let parent = parent_entry_id.as_deref().map(args::entry_id).transpose()?.unwrap_or(ROOT_ENTRY_ID);
        let limit = args::treemap_limit(limit)? as usize;
        s.with_index(|idx| layer_of(&s, idx, &id, parent, limit))
    })
    .await
}

/// One treemap layer: the largest `limit` children plus an exact (or budget-bounded) total of
/// the rest, all from one revision-bound query shape.
pub fn layer_of(s: &AppState, idx: &cm_store::IndexStore, id: &str, parent: u64, limit: usize) -> IpcResult<TreemapLayerDto> {
    let mut q = EntryQuery {
        scan_id: id.to_owned(),
        sort: Sort::LogicalDesc,
        filter: Filter { kind: KindFilter::Any, name_contains: None, parent_entry_id: Some(parent) },
        page_size: MAX_PAGE,
        cursor: None,
    };
    let first = idx.list_entries(&q)?;
    let revision = first.revision;
    let parent_summary = root_summary(s, idx, id, revision, parent)?;
    let mut rest = RestAcc::default();
    let items: Vec<EntryDto> = first.items.iter().take(limit).map(EntryDto::from_item).collect();
    for item in first.items.iter().skip(limit) {
        rest.add(item.logical_bytes);
    }
    let mut cursor = first.next_cursor;
    let mut pages = 1;
    while let Some(c) = cursor.take() {
        if pages >= REST_PAGE_BUDGET {
            cursor = Some(c);
            break;
        }
        q.cursor = Some(c);
        let page = idx.list_entries(&q)?;
        if page.revision != revision {
            return Err(IpcError::new(ErrorCode::CursorStale, "cursor_stale", "the scan revision changed while reading this layer"));
        }
        for item in &page.items {
            rest.add(item.logical_bytes);
        }
        cursor = page.next_cursor;
        pages += 1;
    }
    Ok(TreemapLayerDto { scan_id: id.to_owned(), revision, parent_entry_id: parent.to_string(), parent_summary, items, rest: rest.finish(cursor.is_none()) })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn summarize_selection(state: St<'_>, scan_id: String, entry_ids: Vec<String>) -> IpcResult<SummaryDto> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::scan_id(&scan_id)?;
        let ids = args::summary_ids(&entry_ids)?;
        s.with_index(|idx| Ok(SummaryDto::from(idx.summarize(&id, &ids)?)))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(sort: &str, kind: &str, page: Option<u32>) -> ListEntriesArgs {
        ListEntriesArgs {
            scan_id: "s_desk_1_2".into(),
            sort: sort.into(),
            kind: kind.into(),
            name_contains: Some("  cache ".into()),
            parent_entry_id: Some("12".into()),
            page_size: page,
            cursor: None,
        }
    }

    #[test]
    fn list_entries_args_become_a_bounded_query() {
        let q = entry_query(&a("logical_desc", "directory", None)).unwrap();
        assert_eq!(q.page_size, 50);
        assert_eq!(q.sort, Sort::LogicalDesc);
        assert_eq!(q.filter.kind, KindFilter::Directory);
        assert_eq!(q.filter.parent_entry_id, Some(12));
        assert_eq!(q.filter.name_contains.as_deref(), Some("cache"));
        assert_eq!(entry_query(&a("name_asc", "file", Some(500))).unwrap().page_size, 500);
    }

    #[test]
    fn list_entries_rejects_bad_arguments() {
        for bad in [a("logical_desc", "any", Some(501)), a("logical_desc", "any", Some(0)), a("size", "any", None), a("name_asc", "folder", None)] {
            let e = entry_query(&bad).unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidArgument);
        }
        let mut b = a("logical_desc", "any", None);
        b.parent_entry_id = Some("-3".into());
        assert!(entry_query(&b).is_err());
        let mut b = a("logical_desc", "any", None);
        b.cursor = Some("not a cursor!".into());
        assert!(entry_query(&b).is_err());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let v = serde_json::json!({"scan_id":"s_1","sort":"name_asc","kind":"any","path":"C:\\"});
        assert!(serde_json::from_value::<ListEntriesArgs>(v).is_err());
        let v = serde_json::json!({"scan_id":"s_1","sort":"name_asc","kind":"any","page_size":-1});
        assert!(serde_json::from_value::<ListEntriesArgs>(v).is_err());
    }

    /// SCN-03: a failed scan shows no sizes (the WebView renders unknown), even when rows were written.
    #[test]
    fn failed_scan_status_has_no_size_summary() {
        use cm_core::model::{CloudState, IdentityQuality, ObjectKind, ScanState};
        use cm_core::path::RawPath;
        use cm_core::scan::{EntryRow, ObjectRow, RootProbe, ScanBackend, ScanIssue, ScanOptions, ScanReport, ScanSink};
        use cm_core::host::HostProbe;

        struct FailsAfterOneBatch;
        impl ScanBackend for FailsAfterOneBatch {
            fn name(&self) -> &'static str {
                "test.fails"
            }
            fn probe(&self, _: &RawPath) -> RootProbe {
                RootProbe { filesystem: None, volume_id: None, backend: "test.fails".into(), state: cm_core::capability::CapabilityState::Unverified, reason: "test".into() }
            }
            fn scan(&self, root: &RawPath, sink: &mut dyn ScanSink, _: &ScanOptions) -> ScanReport {
                let obj = |id: u64, kind, logical| ObjectRow { object_id: id, volume_id: "v".into(), file_id: format!("inode64:{id:016x}"), generation: None,
                    quality: IdentityQuality::Strong, kind, logical, allocated: logical, link_count: None, cloud: CloudState::NotCloud, modified_unix: None, shared_blocks: None };
                let file = RawPath::unix(b"a".to_vec()).unwrap();
                sink.write(&[obj(1, ObjectKind::Directory, None), obj(2, ObjectKind::File, Some(700))],
                    &[EntryRow { entry_id: 1, parent_entry_id: None, name: root.clone(), object_id: 1, kind: ObjectKind::Directory, boundary: None },
                      EntryRow { entry_id: 2, parent_entry_id: Some(1), name: file, object_id: 2, kind: ObjectKind::File, boundary: None }], &[]).unwrap();
                ScanReport { state: ScanState::Failed, backend: "test.fails".into(), backend_reason: "test".into(), entries: 2, skipped_ranges: 0, boundaries: 0,
                    elevated_read: vec![], issues: vec![ScanIssue { code: "store_error".into(), scope_display: String::new(), message: "disk full".into() }] }
            }
        }
        let base = std::env::temp_dir().join(format!("cm-desktop-failed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let store = base.join("store");
        let stores = cm_store::Stores::open(&store).unwrap();
        let request = cm_app::scan::ScanRequest { scan_id: "s_failed".into(), scope_id: "scope".into(), root: RawPath::unix(b"/r".to_vec()).unwrap(),
            host: cm_platform::host::OsHostProbe.current("desktop") };
        let outcome = cm_app::scan::run_scan(&stores.index, &FailsAfterOneBatch, &request, &ScanOptions::default()).unwrap();
        assert_eq!(outcome.state, ScanState::Failed);
        let s = AppState::with_store(Some(store.clone()), None, None);
        s.with_index(|idx| {
            let st = status_of(&s, idx, "s_failed")?;
            assert_eq!((st.state, st.published, st.entries_seen), (ScanState::Failed, true, 2));
            assert!(st.summary.is_none(), "a failed scan reports no sizes, never 0 or a partial total");
            Ok(())
        })
        .unwrap();
        drop((s, stores));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn root_pre_check_refuses_files_and_links_with_the_scanner_codes() {
        let base = std::env::temp_dir().join(format!("cm-desktop-roots-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let target = base.join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(base.join("file.txt"), b"x").unwrap();
        assert!(root_problem(&target, None).is_none());
        let reason = |p: &std::path::Path| root_problem(p, None).map(|e| e.reason);
        assert_eq!(reason(&base.join("file.txt")), Some(cm_core::scan::issue::NOT_A_DIRECTORY));
        #[cfg(windows)]
        {
            let jn = base.join("jn");
            assert!(std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&jn).arg(&target).output().unwrap().status.success());
            assert_eq!(reason(&jn), Some(cm_core::scan::issue::ROOT_IS_LINK), "a junction is refused, not followed");
            match std::os::windows::fs::symlink_dir(&target, base.join("link")) {
                Ok(()) => assert_eq!(reason(&base.join("link")), Some(cm_core::scan::issue::ROOT_IS_LINK)),
                Err(e) => eprintln!("skipped directory symlink: cannot create a symbolic link here ({e})"),
            }
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&target, base.join("link")).unwrap();
            assert_eq!(reason(&base.join("link")), Some(cm_core::scan::issue::ROOT_IS_LINK), "a symlink is refused, not followed");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rest_totals_keep_unknown_apart() {
        let mut r = RestAcc::default();
        r.add(Some(cm_core::bytes::Bytes(u64::MAX)));
        r.add(Some(cm_core::bytes::Bytes(u64::MAX)));
        r.add(None);
        let d = r.finish(true);
        assert_eq!(d.count, 3);
        assert_eq!(d.unknown_count, 1);
        assert_eq!(d.known_logical_bytes, (2 * u128::from(u64::MAX)).to_string());
    }

    /// End to end over the real platform backend and index: what the WebView receives for a
    /// scanned tree (status, treemap layer with remainder, revision-bound paging).
    #[test]
    fn real_scan_feeds_status_treemap_and_pages() {
        use cm_app::scan::{ScanRequest, run_scan};
        use cm_core::host::HostProbe;

        let base = std::env::temp_dir().join(format!("cm-desktop-it-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("root");
        let store = base.join("store");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("big.bin"), vec![0u8; 3000]).unwrap();
        std::fs::write(root.join("sub").join("a.txt"), vec![1u8; 1000]).unwrap();
        std::fs::write(root.join("sub").join("b.txt"), vec![2u8; 500]).unwrap();
        for i in 0..10 {
            std::fs::write(root.join(format!("small-{i:02}.log")), vec![3u8; 10]).unwrap();
        }
        let stores = cm_store::Stores::open(&store).unwrap();
        let request = ScanRequest {
            scan_id: "s_it_1".into(),
            scope_id: "scope_it".into(),
            root: cm_platform::paths::raw_path(root.as_os_str()).unwrap(),
            host: cm_platform::host::OsHostProbe.current("desktop"),
        };
        let backend = crate::plat::scan_backend();
        let outcome = run_scan(&stores.index, backend.as_ref(), &request, &cm_core::scan::ScanOptions::default()).unwrap();
        assert_eq!(outcome.state, cm_core::model::ScanState::Completed);

        let s = AppState::with_store(Some(store.clone()), None, None);
        s.with_index(|idx| {
            let st = status_of(&s, idx, "s_it_1")?;
            assert!(st.published && !st.hosted_here);
            assert_eq!(st.root_entry_id.as_deref(), Some("1"));
            assert!(st.root_display.as_deref().unwrap_or("").ends_with("root"), "{:?}", st.root_display);
            let sum = st.summary.clone().unwrap();
            assert_eq!(sum.logical_known_bytes, "4600");
            assert_eq!(sum.logical_unknown_count, 0);

            let layer = layer_of(&s, idx, "s_it_1", 1, 2)?;
            let names: Vec<&str> = layer.items.iter().map(|e| e.name.as_str()).collect();
            assert_eq!(names, ["big.bin", "sub"]);
            assert_eq!(layer.items[1].logical_bytes.as_deref(), Some("1500"));
            assert_eq!(layer.rest, RestDto { count: 10, known_logical_bytes: "100".into(), unknown_count: 0, complete: true });
            assert_eq!(layer.parent_summary.logical_known_bytes, "4600");

            // Walk every page of the same revision-bound query: each child exactly once.
            let mut args = ListEntriesArgs {
                scan_id: "s_it_1".into(),
                sort: "name_asc".into(),
                kind: "any".into(),
                name_contains: None,
                parent_entry_id: Some("1".into()),
                page_size: Some(3),
                cursor: None,
            };
            let mut seen = Vec::new();
            let mut first_cursor = None;
            loop {
                let page = idx.list_entries(&entry_query(&args)?)?;
                seen.extend(page.items.iter().map(|i| i.entry_id.clone()));
                if first_cursor.is_none() {
                    first_cursor = page.next_cursor.clone();
                }
                match page.next_cursor {
                    Some(c) => args.cursor = Some(c),
                    None => break,
                }
            }
            let unique: std::collections::BTreeSet<_> = seen.iter().collect();
            assert_eq!((seen.len(), unique.len()), (12, 12));

            // A cursor is bound to its sort: reusing it under another sort is cursor_stale.
            args.sort = "logical_desc".into();
            args.cursor = first_cursor;
            let err: IpcError = idx.list_entries(&entry_query(&args)?).unwrap_err().into();
            assert_eq!(err.reason, "cursor_stale");
            Ok(())
        })
        .unwrap();
        drop(stores);
        drop(s);
        let _ = std::fs::remove_dir_all(&base);
    }
}
