use super::*;
use cm_core::model::{CloudState, SkipReason};
use crate::audit::{AuditStore, HostRef, Liveness};
use crate::testutil::scratch;
use crate::{Stores, StoreError};

fn host() -> HostRef {
    HostRef { kind: "cli".into(), pid: 4242, started_at: "2026-10-15T00:00:00Z".into() }
}

fn unix(s: &str) -> RawPath {
    RawPath::unix(s.as_bytes().to_vec()).unwrap()
}

fn obj(id: u64, kind: ObjectKind, logical: Option<u64>) -> ObjectRow {
    ObjectRow {
        object_id: id,
        volume_id: "vol".into(),
        file_id: format!("inode64:{id:016x}"),
        generation: None,
        quality: IdentityQuality::Strong,
        kind,
        logical,
        allocated: logical,
        link_count: Some(1),
        modified_unix: Some(1_700_000_000),
        cloud: CloudState::NotCloud,
        shared_blocks: None,
    }
}

fn ent(id: u64, parent: Option<u64>, name: &str, object: u64, kind: ObjectKind) -> EntryRow {
    EntryRow { entry_id: id, parent_entry_id: parent, name: unix(name), object_id: object, kind, boundary: None }
}

fn begin(store: &IndexStore, scan_id: &str) -> u32 {
    store
        .begin_scan(&ScanInit {
            scan_id: scan_id.into(),
            scope_id: "scope".into(),
            root: unix("/r"),
            backend: "test".into(),
            host: host(),
            started_at: "2026-10-15T00:00:00Z".into(),
        })
        .unwrap()
}

/// /r (dir 1) with files a..; sizes given; returns store
fn flat(dir: &std::path::Path, sizes: &[Option<u64>]) -> IndexStore {
    let store = IndexStore::open(&dir.join("index.db")).unwrap();
    let rev = begin(&store, "s1");
    let mut objects = vec![obj(1, ObjectKind::Directory, None)];
    let mut entries = vec![ent(1, None, "/r", 1, ObjectKind::Directory)];
    for (i, s) in sizes.iter().enumerate() {
        let id = 10 + i as u64;
        objects.push(obj(id, ObjectKind::File, *s));
        entries.push(ent(id, Some(1), &format!("f{i:03}"), id, ObjectKind::File));
    }
    store.write_batch("s1", rev, &objects, &entries, &[]).unwrap();
    store.publish("s1", rev, ScanState::Completed, "t").unwrap();
    store
}

fn query(sort: Sort, page: u32, cursor: Option<String>) -> EntryQuery {
    EntryQuery {
        scan_id: "s1".into(),
        sort,
        filter: Filter { kind: KindFilter::File, name_contains: None, parent_entry_id: None },
        page_size: page,
        cursor,
    }
}

fn walk(store: &IndexStore, sort: Sort, page: u32) -> Vec<String> {
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let p = store.list_entries(&query(sort, page, cursor)).unwrap();
        assert!(p.items.len() <= page as usize);
        seen.extend(p.items.iter().map(|i| i.entry_id.clone()));
        match p.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    seen
}

#[test]
fn staging_is_invisible_until_published_and_partial_results_are_queryable() {
    let dir = scratch("staging");
    let store = IndexStore::open(&dir.join("index.db")).unwrap();
    let rev = begin(&store, "s1");
    store
        .write_batch("s1", rev, &[obj(1, ObjectKind::Directory, None), obj(2, ObjectKind::File, Some(5))],
            &[ent(1, None, "/r", 1, ObjectKind::Directory), ent(2, Some(1), "a", 2, ObjectKind::File)], &[])
        .unwrap();
    assert!(matches!(store.list_entries(&query(Sort::LogicalDesc, 10, None)), Err(StoreError::InvalidState(_))));
    assert!(!store.scan_status("s1").unwrap().published);
    let state = store.publish("s1", rev, ScanState::Cancelled, "t").unwrap();
    assert_eq!(state, ScanState::Cancelled);
    let status = store.scan_status("s1").unwrap();
    assert_eq!(status.completeness.as_deref(), Some("partial_scope"));
    assert_eq!(store.list_entries(&query(Sort::LogicalDesc, 10, None)).unwrap().items.len(), 1);
    assert!(store.write_batch("s1", rev, &[], &[], &[]).is_err(), "published revisions are immutable");
}

#[test]
fn completed_with_unread_ranges_is_published_as_partial() {
    let dir = scratch("partial");
    let store = IndexStore::open(&dir.join("index.db")).unwrap();
    let rev = begin(&store, "s1");
    store
        .write_batch("s1", rev, &[obj(1, ObjectKind::Directory, None)], &[ent(1, None, "/r", 1, ObjectKind::Directory)],
            &[SkippedRow { parent_entry_id: Some(1), path: unix("/r/private"), reason: SkipReason::PermissionDenied }])
        .unwrap();
    assert_eq!(store.publish("s1", rev, ScanState::Completed, "t").unwrap(), ScanState::Partial);
}

#[test]
fn every_entry_appears_exactly_once_across_pages_for_every_sort() {
    let dir = scratch("pages");
    let sizes: Vec<Option<u64>> = (0..137).map(|i| if i % 11 == 0 { None } else { Some((i % 7) as u64 * 100) }).collect();
    let store = flat(&dir, &sizes);
    for sort in [Sort::LogicalDesc, Sort::AllocatedDesc, Sort::NameAsc] {
        for page in [1, 7, 50, 500] {
            let seen = walk(&store, sort, page);
            let unique: BTreeSet<_> = seen.iter().collect();
            assert_eq!((seen.len(), unique.len()), (137, 137), "{sort:?} page {page}");
        }
    }
    let order = walk(&store, Sort::LogicalDesc, 13);
    let first_unknown = order.iter().position(|id| sizes[id.parse::<usize>().unwrap() - 10].is_none()).unwrap();
    assert!(order[first_unknown..].iter().all(|id| sizes[id.parse::<usize>().unwrap() - 10].is_none()), "unknown sizes sort last");
}

#[test]
fn exact_u64_ordering_beyond_i64_and_javascript_precision() {
    let dir = scratch("u64");
    // Sum stays below 2^64 so the directory total is representable.
    let big = [(1u64 << 63) + 1, 1u64 << 62, (1u64 << 62) - 1, (1u64 << 53) + 1, 1u64 << 53, 7];
    let store = flat(&dir, &big.iter().map(|v| Some(*v)).collect::<Vec<_>>());
    let page = store.list_entries(&query(Sort::LogicalDesc, 10, None)).unwrap();
    let got: Vec<u64> = page.items.iter().map(|i| i.logical_bytes.unwrap().0).collect();
    let mut want = big.to_vec();
    want.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(got, want);
}

#[test]
fn largest_file_from_a_late_batch_sorts_first() {
    let dir = scratch("late");
    let store = IndexStore::open(&dir.join("index.db")).unwrap();
    let rev = begin(&store, "s1");
    store.write_batch("s1", rev, &[obj(1, ObjectKind::Directory, None), obj(2, ObjectKind::File, Some(10))],
        &[ent(1, None, "/r", 1, ObjectKind::Directory), ent(2, Some(1), "small", 2, ObjectKind::File)], &[]).unwrap();
    store.write_batch("s1", rev, &[obj(3, ObjectKind::File, Some(10_000))], &[ent(3, Some(1), "huge", 3, ObjectKind::File)], &[]).unwrap();
    store.publish("s1", rev, ScanState::Completed, "t").unwrap();
    let page = store.list_entries(&query(Sort::LogicalDesc, 1, None)).unwrap();
    assert_eq!(page.items[0].path.display(), "/r/huge");
}

#[test]
fn page_size_bounds_and_stale_cursors() {
    let dir = scratch("cursor");
    let store = flat(&dir, &[Some(1), Some(2), Some(3)]);
    assert!(matches!(store.list_entries(&query(Sort::LogicalDesc, 501, None)), Err(StoreError::InvalidArgument(_))));
    assert!(matches!(store.list_entries(&query(Sort::LogicalDesc, 0, None)), Err(StoreError::InvalidArgument(_))));
    let first = store.list_entries(&query(Sort::LogicalDesc, 1, None)).unwrap();
    let cursor = first.next_cursor.clone().unwrap();
    assert!(matches!(store.list_entries(&query(Sort::NameAsc, 1, Some(cursor.clone()))), Err(StoreError::CursorStale(_))));
    assert!(matches!(store.list_entries(&query(Sort::LogicalDesc, 2, Some(cursor.clone()))), Err(StoreError::CursorStale(_))));
    assert!(matches!(store.list_entries(&query(Sort::LogicalDesc, 1, Some("garbage".into()))), Err(StoreError::CursorStale(_))));
    store.drop_scan("s1").unwrap();
    assert!(matches!(store.list_entries(&query(Sort::LogicalDesc, 1, Some(cursor))), Err(StoreError::CursorStale(_))));
}

#[test]
fn hardlinks_and_parent_child_selection_do_not_double_count() {
    let dir = scratch("overlap");
    let store = IndexStore::open(&dir.join("index.db")).unwrap();
    let rev = begin(&store, "s1");
    // /r/{a/,b/}; object 5 hard-linked as a/x and b/y; a/z unknown size
    let objects = [obj(1, ObjectKind::Directory, None), obj(2, ObjectKind::Directory, None), obj(3, ObjectKind::Directory, None),
        obj(5, ObjectKind::File, Some(100)), obj(6, ObjectKind::File, None)];
    let entries = [ent(1, None, "/r", 1, ObjectKind::Directory), ent(2, Some(1), "a", 2, ObjectKind::Directory),
        ent(3, Some(1), "b", 3, ObjectKind::Directory), ent(4, Some(2), "x", 5, ObjectKind::File),
        ent(7, Some(3), "y", 5, ObjectKind::File), ent(8, Some(2), "z", 6, ObjectKind::File)];
    store.write_batch("s1", rev, &objects, &entries, &[]).unwrap();
    store.publish("s1", rev, ScanState::Completed, "t").unwrap();

    let root = store.list_entries(&EntryQuery { scan_id: "s1".into(), sort: Sort::NameAsc,
        filter: Filter { kind: KindFilter::Directory, name_contains: None, parent_entry_id: None }, page_size: 10, cursor: None }).unwrap();
    let by_name: HashMap<String, &EntryItem> = root.items.iter().map(|i| (i.path.display(), i)).collect();
    assert_eq!(by_name["/r"].logical_bytes, Some(Bytes(100)), "hard link counted once in the common ancestor");
    assert_eq!(by_name["/r"].unknown_descendants, Some(1));
    assert_eq!(by_name["/r/a"].logical_bytes, Some(Bytes(100)));
    assert_eq!(by_name["/r/b"].logical_bytes, Some(Bytes(100)));

    let s = store.summarize("s1", &[1, 2, 4]).unwrap();
    assert_eq!(s.overlapping_selected, 2, "a and x are inside the selected root");
    assert_eq!(s.logical.known, Bytes(100));
    assert_eq!(s.logical.unknown_count, 1);
    assert_eq!(s.logical.exact(), None);
    let both_links = store.summarize("s1", &[4, 7]).unwrap();
    assert_eq!(both_links.logical.known, Bytes(100), "two names of one object free at most one object");
}

#[test]
fn unread_subtree_makes_directory_size_partial_not_zero() {
    let dir = scratch("unread");
    let store = IndexStore::open(&dir.join("index.db")).unwrap();
    let rev = begin(&store, "s1");
    store.write_batch("s1", rev, &[obj(1, ObjectKind::Directory, None), obj(2, ObjectKind::Directory, None)],
        &[ent(1, None, "/r", 1, ObjectKind::Directory), ent(2, Some(1), "locked", 2, ObjectKind::Directory)],
        &[SkippedRow { parent_entry_id: Some(2), path: unix("/r/locked"), reason: SkipReason::PermissionDenied }]).unwrap();
    store.publish("s1", rev, ScanState::Partial, "t").unwrap();
    let page = store.list_entries(&EntryQuery { scan_id: "s1".into(), sort: Sort::LogicalDesc,
        filter: Filter { kind: KindFilter::Directory, name_contains: Some("locked".into()), parent_entry_id: None }, page_size: 5, cursor: None }).unwrap();
    assert_eq!(page.items[0].logical_bytes, None);
    assert_eq!(page.items[0].size_basis, SizeBasis::Unknown);
    assert_eq!(page.unknown_size_items, 1);
}

#[test]
fn index_persists_across_restart_and_rebuild_keeps_audit() {
    let dir = scratch("restart");
    {
        let stores = Stores::open(&dir).unwrap();
        let rev = begin(&stores.index, "s1");
        stores.index.write_batch("s1", rev, &[obj(1, ObjectKind::Directory, None)], &[ent(1, None, "/r", 1, ObjectKind::Directory)], &[]).unwrap();
        stores.index.publish("s1", rev, ScanState::Completed, "t").unwrap();
        stores.audit.begin_operation("op1", "p", "d", "k", &host(), "t").unwrap();
    }
    let stores = Stores::open(&dir).unwrap();
    assert_eq!(stores.index.scan_status("s1").unwrap().state, ScanState::Completed);
    drop(stores);
    let (rebuilt, kept) = Stores::rebuild_index(&dir).unwrap();
    assert!(matches!(rebuilt.scan_status("s1"), Err(StoreError::NotFound(_))));
    let audit = AuditStore::open(&dir.join("audit.db")).unwrap();
    assert_eq!(audit.operation_state("op1").unwrap(), "created", "rebuild never touches audit records");
    // The old index is kept under a new name, never deleted, and still holds its scan.
    assert_eq!(kept.len(), 1, "{kept:?}");
    let name = kept[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("index.db.damaged-"), "{name}");
    let old = IndexStore::open(&kept[0]).unwrap();
    assert_eq!(old.scan_status("s1").unwrap().state, ScanState::Completed);
}

#[test]
fn incompatible_or_damaged_index_is_reported_not_silently_reset() {
    let dir = scratch("damaged");
    let path = dir.join("index.db");
    {
        let store = IndexStore::open(&path).unwrap();
        store.conn().execute("UPDATE meta SET value='99' WHERE key='schema_version'", []).unwrap();
    }
    assert!(matches!(IndexStore::open(&path), Err(StoreError::IndexIncompatible { .. })));
    let garbage = dir.join("garbage.db");
    std::fs::write(&garbage, vec![0x42u8; 8192]).unwrap();
    assert!(matches!(IndexStore::open(&garbage), Err(StoreError::IndexCorrupt(_))));
}

#[test]
fn store_inside_scan_root_is_refused() {
    let dir = scratch("inside");
    let stores = Stores::open(&dir.join("store")).unwrap();
    assert!(matches!(stores.ensure_outside(&dir), Err(StoreError::StoreInsideScope { .. })));
    let other = scratch("inside-other");
    stores.ensure_outside(&other).unwrap();
}

#[test]
fn exited_host_interrupts_scan_and_keeps_committed_results() {
    let dir = scratch("host");
    let store = IndexStore::open(&dir.join("index.db")).unwrap();
    let rev = begin(&store, "s1");
    store.write_batch("s1", rev, &[obj(1, ObjectKind::Directory, None), obj(2, ObjectKind::File, Some(9))],
        &[ent(1, None, "/r", 1, ObjectKind::Directory), ent(2, Some(1), "a", 2, ObjectKind::File)], &[]).unwrap();
    assert!(store.reconcile_hosts(|_| Liveness::Unknown, "t").unwrap().is_empty(), "unknown liveness never ends a scan");
    assert_eq!(store.scan_status("s1").unwrap().state, ScanState::Running);
    assert_eq!(store.reconcile_hosts(|_| Liveness::Exited, "t2").unwrap(), vec!["s1".to_string()]);
    let status = store.scan_status("s1").unwrap();
    assert_eq!((status.state, status.completeness.as_deref()), (ScanState::Interrupted, Some("partial_scope")));
    assert_eq!(store.list_entries(&query(Sort::LogicalDesc, 5, None)).unwrap().items.len(), 1);
    let report = store.scan_report("s1").unwrap().expect("interruption is explained");
    assert_eq!(report["issues"][0]["code"], "host_exited");
    assert!(store.reconcile_hosts(|_| Liveness::Exited, "t3").unwrap().is_empty(), "terminal scans are not reconciled twice");
}

#[test]
fn cancel_is_a_receipt_and_terminal_scans_refuse_it() {
    let dir = scratch("cancel");
    let store = IndexStore::open(&dir.join("index.db")).unwrap();
    let rev = begin(&store, "s1");
    let r = store.request_cancel("s1").unwrap();
    assert_eq!((r.accepted, r.state_at_receipt.as_str()), (true, "running"));
    assert!(store.cancel_requested("s1").unwrap());
    assert_eq!(store.scan_status("s1").unwrap().state, ScanState::Running, "receipt does not claim the scan stopped");
    store.set_stopping("s1").unwrap();
    store.publish("s1", rev, ScanState::Cancelled, "t").unwrap();
    let again = store.request_cancel("s1").unwrap();
    assert_eq!((again.accepted, again.state_at_receipt.as_str()), (false, "already_terminal"));
}

#[test]
fn an_accepted_cancel_always_ends_cancelled_even_if_the_backend_finished_first() {
    let dir = scratch("cancel-late");
    let store = IndexStore::open(&dir.join("index.db")).unwrap();
    let other = IndexStore::open(&dir.join("index.db")).unwrap(); // another process's connection
    let rev = begin(&store, "s1");
    store.write_batch("s1", rev, &[obj(1, ObjectKind::Directory, None), obj(2, ObjectKind::File, Some(9))],
        &[ent(1, None, "/r", 1, ObjectKind::Directory), ent(2, Some(1), "a", 2, ObjectKind::File)], &[]).unwrap();
    // The receipt lands after the backend's last poll; the backend still reports `completed`.
    assert!(other.request_cancel("s1").unwrap().accepted);
    let state = store.publish("s1", rev, ScanState::Completed, "t").unwrap();
    assert_eq!(state, ScanState::Cancelled, "an accepted receipt never ends as completed");
    let status = other.scan_status("s1").unwrap();
    assert_eq!((status.state, status.completeness.as_deref()), (ScanState::Cancelled, Some("partial_scope")));
    assert_eq!(store.list_entries(&query(Sort::LogicalDesc, 5, None)).unwrap().items.len(), 1, "confirmed rows stay queryable");
    let late = other.request_cancel("s1").unwrap();
    assert_eq!((late.accepted, late.state_at_receipt.as_str()), (false, "already_terminal"));

    // Without a cancel request the same publish completes.
    let rev = begin(&store, "s2");
    store.write_batch("s2", rev, &[obj(1, ObjectKind::Directory, None)], &[ent(1, None, "/r", 1, ObjectKind::Directory)], &[]).unwrap();
    assert_eq!(store.publish("s2", rev, ScanState::Completed, "t").unwrap(), ScanState::Completed);
}

#[test]
fn windows_paths_are_rebuilt_from_raw_units() {
    let dir = scratch("winpath");
    let store = IndexStore::open(&dir.join("index.db")).unwrap();
    let root = RawPath::windows("C:\\数据".encode_utf16().collect()).unwrap();
    store.begin_scan(&ScanInit { scan_id: "w".into(), scope_id: "s".into(), root: root.clone(), backend: "t".into(),
        host: host(), started_at: "t".into() }).unwrap();
    let mut name_units: Vec<u16> = "a b".encode_utf16().collect();
    name_units.push(0xDC00); // unpaired low surrogate survives
    let name = RawPath::windows(name_units).unwrap();
    store.write_batch("w", 1, &[obj(1, ObjectKind::Directory, None), obj(2, ObjectKind::File, Some(1))],
        &[EntryRow { entry_id: 1, parent_entry_id: None, name: root, object_id: 1, kind: ObjectKind::Directory, boundary: None },
          EntryRow { entry_id: 2, parent_entry_id: Some(1), name, object_id: 2, kind: ObjectKind::File, boundary: None }], &[]).unwrap();
    store.publish("w", 1, ScanState::Completed, "t").unwrap();
    let page = store.list_entries(&EntryQuery { scan_id: "w".into(), sort: Sort::LogicalDesc,
        filter: Filter { kind: KindFilter::File, name_contains: None, parent_entry_id: Some(1) }, page_size: 5, cursor: None }).unwrap();
    let RawPath::Windows(units) = &page.items[0].path else { panic!("windows path") };
    let mut want: Vec<u16> = "C:\\数据\\a b".encode_utf16().collect();
    want.push(0xDC00);
    assert_eq!(units, &want);
}

#[test]
fn unrepresentable_directory_total_is_unknown_not_wrapped() {
    let dir = scratch("overflow");
    let store = flat(&dir, &[Some(u64::MAX), Some(2)]);
    let files = store.list_entries(&query(Sort::LogicalDesc, 5, None)).unwrap();
    assert_eq!(files.items[0].logical_bytes, Some(Bytes(u64::MAX)), "single values keep full u64 precision");
    let dirs = store.list_entries(&EntryQuery { scan_id: "s1".into(), sort: Sort::LogicalDesc,
        filter: Filter { kind: KindFilter::Directory, name_contains: None, parent_entry_id: None }, page_size: 5, cursor: None }).unwrap();
    assert_eq!(dirs.items[0].logical_bytes, None);
    assert_eq!(dirs.items[0].size_basis, SizeBasis::Unknown);
    let s = store.summarize("s1", &[1]);
    assert!(s.is_err(), "selection summaries use checked addition and report overflow");
    assert!(store.summarize_full("s1", &[1]).is_err());
    // No stored total for a directory whose total does not fit: its folder total reports the overflow too.
    let stored: i64 = store.conn().query_row("SELECT count(*) FROM subtree_totals WHERE scan_id='s1'", [], |r| r.get(0)).unwrap();
    assert_eq!(stored, 0);
    assert!(matches!(store.folder_total("s1", 1), Err(StoreError::Core(_))));
    assert_eq!(store.scan_status("s1").unwrap().entries_seen, 3, "the counts are still stored");
}

/// /r{a/{x,z?},b/{y=x},locked/(unread)} with a hard link and an unknown size.
fn tree_fixture(dir: &std::path::Path) -> IndexStore {
    let store = IndexStore::open(&dir.join("index.db")).unwrap();
    let rev = begin(&store, "s1");
    let objects = [obj(1, ObjectKind::Directory, None), obj(2, ObjectKind::Directory, None), obj(3, ObjectKind::Directory, None),
        obj(5, ObjectKind::File, Some(100)), obj(6, ObjectKind::File, None), obj(9, ObjectKind::Directory, None), obj(10, ObjectKind::File, Some(7))];
    let entries = [ent(1, None, "/r", 1, ObjectKind::Directory), ent(2, Some(1), "a", 2, ObjectKind::Directory),
        ent(3, Some(1), "b", 3, ObjectKind::Directory), ent(4, Some(2), "x", 5, ObjectKind::File),
        ent(7, Some(3), "y", 5, ObjectKind::File), ent(8, Some(2), "z", 6, ObjectKind::File),
        ent(9, Some(1), "locked", 9, ObjectKind::Directory), ent(10, Some(3), "w", 10, ObjectKind::File)];
    store.write_batch("s1", rev, &objects, &entries,
        &[SkippedRow { parent_entry_id: Some(9), path: unix("/r/locked/deep"), reason: SkipReason::PermissionDenied }]).unwrap();
    store.publish("s1", rev, ScanState::Completed, "t").unwrap();
    store
}

#[test]
fn folder_total_matches_the_full_tree_summary_for_every_entry() {
    let dir = scratch("folder-total");
    let store = tree_fixture(&dir);
    for id in [1, 2, 3, 4, 7, 8, 9, 10] {
        assert_eq!(store.folder_total("s1", id).unwrap(), store.summarize("s1", &[id]).unwrap(), "entry {id}");
    }
    let root = store.folder_total("s1", 1).unwrap();
    assert_eq!(root.logical.known, Bytes(107), "hard link counted once");
    assert_eq!(root.logical.unknown_count, 2, "one unknown size plus one unread range, never zero");
    assert_eq!(root.unread_ranges, 1);
    assert_eq!(root.covered_entries, 8);
    assert!(matches!(store.folder_total("s1", 99), Err(StoreError::NotFound(_))));
    // Staging revisions stay invisible.
    let rev = begin(&store, "s2");
    store.write_batch("s2", rev, &[obj(1, ObjectKind::Directory, None)], &[ent(1, None, "/r", 1, ObjectKind::Directory)], &[]).unwrap();
    assert!(matches!(store.folder_total("s2", 1), Err(StoreError::InvalidState(_))));
}

#[test]
fn get_entry_and_identity_lookup_return_published_rows_only() {
    let dir = scratch("get-entry");
    let store = tree_fixture(&dir);
    let e = store.get_entry("s1", 8).unwrap();
    assert_eq!((e.path.display(), e.logical_bytes, e.size_basis), ("/r/a/z".to_string(), None, SizeBasis::Unknown));
    let a = store.get_entry("s1", 2).unwrap();
    assert_eq!((a.logical_bytes, a.unknown_descendants), (Some(Bytes(100)), Some(1)));
    assert!(matches!(store.get_entry("s1", 77), Err(StoreError::NotFound(_))));
    let found = store
        .entries_by_identity("s1", &[("vol".into(), format!("inode64:{:016x}", 3)), ("vol".into(), "inode64:ffff".into())])
        .unwrap();
    assert_eq!(found.iter().map(|(id, i)| (id.1.clone(), i.path.display())).collect::<Vec<_>>(), [(format!("inode64:{:016x}", 3), "/r/b".to_string())], "absent identities are not invented");
    assert!(store.entries_by_identity("s1", &[("vol".into(), format!("inode64:{:016x}", 5))]).unwrap().is_empty(), "directories only");
}

#[test]
fn scans_are_listed_newest_first_with_their_root() {
    let dir = scratch("list-scans");
    let store = IndexStore::open(&dir.join("index.db")).unwrap();
    for i in 0..5 {
        let id = format!("s{i}");
        let rev = store
            .begin_scan(&ScanInit { scan_id: id.clone(), scope_id: "scope".into(), root: unix(&format!("/root{i}")), backend: "t".into(),
                host: host(), started_at: format!("2026-10-15T00:00:0{i}Z") })
            .unwrap();
        if i != 3 {
            store.publish(&id, rev, ScanState::Completed, "t").unwrap();
        }
    }
    assert_eq!(store.scan_status("s2").unwrap().root.display(), "/root2");
    let (first, next) = store.list_scans(2, None).unwrap();
    assert_eq!(first.iter().map(|s| s.scan_id.as_str()).collect::<Vec<_>>(), ["s4", "s3"]);
    assert_eq!((first[1].state, first[1].published), (ScanState::Running, false));
    assert_eq!(first[0].root.display(), "/root4");
    let (second, next) = store.list_scans(2, next.as_deref()).unwrap();
    assert_eq!(second.iter().map(|s| s.scan_id.as_str()).collect::<Vec<_>>(), ["s2", "s1"]);
    let (last, end) = store.list_scans(2, next.as_deref()).unwrap();
    assert_eq!((last.len(), end), (1, None));
    assert!(matches!(store.list_scans(3, next.as_deref()), Err(StoreError::CursorStale(_))), "cursor bound to page size");
    assert!(matches!(store.list_scans(0, None), Err(StoreError::InvalidArgument(_))));
    assert_eq!(store.latest_candidate_set("s1").unwrap(), None);
}

/// Overwrite `len` bytes at `offset` of a closed database file.
fn garble(path: &std::path::Path, offset: u64, len: usize) {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.seek(SeekFrom::Start(offset)).unwrap();
    f.write_all(&vec![0xA5u8; len]).unwrap();
}

/// One small published scan in a closed database whose WAL was checkpointed into the file; returns
/// the root page of `entries` (the whole table fits in it).
fn small_index(path: &std::path::Path) -> u64 {
    let store = IndexStore::open(path).unwrap();
    let rev = begin(&store, "s1");
    store.write_batch("s1", rev, &[obj(1, ObjectKind::Directory, None), obj(2, ObjectKind::File, Some(9))],
        &[ent(1, None, "/r", 1, ObjectKind::Directory), ent(2, Some(1), "a", 2, ObjectKind::File)], &[]).unwrap();
    store.publish("s1", rev, ScanState::Completed, "t").unwrap();
    assert_eq!(store.conn().query_row("PRAGMA page_size", [], |r| r.get::<_, i64>(0)).unwrap(), 4096);
    store.conn().query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())).unwrap();
    store.conn().query_row("SELECT rootpage FROM sqlite_master WHERE name='entries'", [], |r| r.get::<_, i64>(0)).unwrap() as u64
}

#[test]
fn a_damaged_index_leaves_history_and_recovery_usable_and_is_repaired_by_moving_it_aside() {
    let dir = scratch("damaged-header");
    {
        let stores = Stores::open(&dir).unwrap();
        let rev = begin(&stores.index, "s1");
        stores.index.publish("s1", rev, ScanState::Completed, "t").unwrap();
        stores.audit.begin_operation("op1", "p", "d", "k", &host(), "t").unwrap();
    }
    // The acceptance repro (WFA-16): the first 92 bytes of index.db overwritten, audit.db intact.
    garble(&dir.join("index.db"), 0, 92);
    // A WAL left by a crash belongs to the damaged file: no open may delete it, and it moves with the file
    // (it must never be replayed into the new index).
    std::fs::write(dir.join("index.db-wal"), b"stale wal").unwrap();
    let stores = Stores::open(&dir).expect("the audit store opens without the index");
    assert_eq!(stores.audit.operation_state("op1").unwrap(), "created", "history stays readable");
    let e = stores.index.scan_status("s1").unwrap_err();
    assert!(matches!(e, StoreError::IndexCorrupt(_)), "{e:?}");
    assert_eq!(e.code(), cm_core::error::ErrorCode::IndexUnavailable);
    assert_eq!(e.required_action(), Some(crate::REPAIR_INDEX_COMMAND));
    // Every later call reports the same typed error.
    assert!(matches!(stores.index.list_scans(5, None), Err(StoreError::IndexCorrupt(_))));
    assert!(matches!(stores.index.begin_scan(&ScanInit { scan_id: "s2".into(), scope_id: "x".into(), root: unix("/r"), backend: "t".into(),
        host: host(), started_at: "t".into() }), Err(StoreError::IndexCorrupt(_))), "a new scan needs the index");

    assert_eq!(std::fs::read(dir.join("index.db-wal")).unwrap(), b"stale wal", "a failed open leaves the WAL alone");
    let before = std::fs::read(dir.join("index.db")).unwrap();
    let repair = Stores::repair_index(&dir).unwrap();
    assert_eq!((repair.previous_state, repair.repaired), ("corrupt", true), "{repair:?}");
    assert!(repair.problem.as_deref().is_some_and(|p| p.contains("damaged")), "{repair:?}");
    let names: Vec<String> = repair.kept.iter().map(|k| k.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names[0].starts_with("index.db.damaged-") && names[1] == format!("{}-wal", names[0]), "{names:?}");
    assert_eq!(std::fs::read(&repair.kept[0]).unwrap(), before, "moved, never rewritten or deleted");
    assert_eq!(std::fs::read(&repair.kept[1]).unwrap(), b"stale wal");
    // The same handle sees the new, empty index (the file changed): scans must be run again.
    assert!(matches!(stores.index.scan_status("s1"), Err(StoreError::NotFound(_))));
    assert_eq!(stores.audit.operation_state("op1").unwrap(), "created", "the repair never touches the audit store");
    // A healthy index is left alone; nothing is moved.
    let again = Stores::repair_index(&dir).unwrap();
    assert_eq!((again.previous_state, again.repaired, again.kept.len()), ("healthy", false, 0));
    let empty = scratch("damaged-header-none");
    assert_eq!(Stores::repair_index(&empty).unwrap().previous_state, "missing");
}

#[test]
fn integrity_is_checked_when_due_and_damage_found_later_is_still_the_typed_error() {
    let dir = scratch("integrity-due");
    let path = dir.join("index.db");
    let entries_root = small_index(&path);
    assert!(!dir.join("index.db-wal").exists() || std::fs::metadata(dir.join("index.db-wal")).unwrap().len() == 0);
    garble(&path, (entries_root - 1) * 4096, 4096);
    // The fresh index recorded its check, so this open reads only the header and `meta`...
    let store = IndexStore::open(&path).expect("no check is due");
    // ...and the damage surfaces on the first query that reads the page, as the typed index error.
    let e = store.list_entries(&query(Sort::LogicalDesc, 5, None)).unwrap_err();
    assert!(matches!(e, StoreError::IndexCorrupt(_)), "{e:?}");
    assert_eq!(e.code(), cm_core::error::ErrorCode::IndexUnavailable);
    assert!(store.integrity_status().unwrap().1.is_some(), "the damage marks the index for a full check");
    drop(store);
    // The next open checks every page and refuses.
    assert!(matches!(IndexStore::open(&path), Err(StoreError::IndexCorrupt(m)) if m.contains("integrity check")));
}

#[test]
fn a_scheduled_check_runs_at_most_once_per_interval_and_doctor_always_checks() {
    let dir = scratch("integrity-interval");
    let path = dir.join("index.db");
    drop(IndexStore::open(&path).unwrap());
    let checked = |s: &IndexStore| s.integrity_status().unwrap().0.expect("a check was recorded");
    let set = |s: &IndexStore, at: i64| s.conn().execute("UPDATE meta SET value=?1 WHERE key='integrity_checked_unix'", [at.to_string()]).unwrap();
    let store = IndexStore::open(&path).unwrap();
    let first = checked(&store);
    set(&store, first - 60);
    drop(store);
    let store = IndexStore::open(&path).unwrap();
    assert_eq!(checked(&store), first - 60, "checked less than RECHECK_DAYS ago: an open does not check again");
    set(&store, first - crate::integrity::RECHECK_DAYS * 86_400 - 1);
    drop(store);
    let store = IndexStore::open(&path).unwrap();
    assert!(checked(&store) >= first, "an older check runs again at the next open and is recorded");
    set(&store, first - 60);
    store.verify().unwrap();
    assert!(checked(&store) >= first, "doctor's check always runs and is recorded");
}

#[test]
fn a_host_killed_after_recording_its_own_report_is_still_explained_as_host_exited() {
    let dir = scratch("host-late");
    let store = IndexStore::open(&dir.join("index.db")).unwrap();
    let rev = begin(&store, "s1");
    store.write_batch("s1", rev, &[obj(1, ObjectKind::Directory, None), obj(2, ObjectKind::File, Some(9))],
        &[ent(1, None, "/r", 1, ObjectKind::Directory), ent(2, Some(1), "a", 2, ObjectKind::File)], &[]).unwrap();
    // BUG-WFA-06: every row was confirmed and the host recorded its report, then it was killed before publishing.
    let own = cm_core::scan::ScanReport { state: ScanState::Completed, backend: "win.directory".into(), backend_reason: "NTFS volume".into(),
        entries: 2, skipped_ranges: 0, boundaries: 0, elevated_read: vec![], issues: vec![] };
    store.record_report("s1", &own).unwrap();
    assert_eq!(store.reconcile_hosts(|_| Liveness::Exited, "t2").unwrap(), vec!["s1".to_string()]);
    let report = store.scan_report("s1").unwrap().unwrap();
    assert_eq!(report["state"], "interrupted", "{report}");
    assert_eq!((report["backend"].as_str(), report["entries"].as_u64()), (Some("win.directory"), Some(2)), "the host's own facts are kept: {report}");
    let codes: Vec<&str> = report["issues"].as_array().unwrap().iter().map(|i| i["code"].as_str().unwrap()).collect();
    assert_eq!(codes, ["host_exited"]);
    let status = store.scan_status("s1").unwrap();
    assert_eq!((status.state, status.completeness.as_deref()), (ScanState::Interrupted, Some("partial_scope")));
    assert!(store.integrity_status().unwrap().1.is_some_and(|m| m.contains("s1")), "an unclean shutdown marks the index for a full check");
    assert!(store.reconcile_hosts(|_| Liveness::Exited, "t3").unwrap().is_empty());
    assert_eq!(store.scan_report("s1").unwrap().unwrap()["issues"].as_array().unwrap().len(), 1, "the issue is added once");
}

/// SQLite on Windows opens its files without FILE_SHARE_DELETE, so a damaged index another process
/// still holds cannot be moved: the repair reports it and changes nothing.
#[cfg(windows)]
#[test]
fn a_held_index_is_not_moved_and_nothing_changes() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = scratch("repair-held");
    let path = dir.join("index.db");
    drop(IndexStore::open(&path).unwrap());
    garble(&path, 0, 92);
    let holder = std::fs::OpenOptions::new().read(true).share_mode(0x1 | 0x2).open(&path).unwrap();
    let err = Stores::repair_index(&dir).unwrap_err();
    assert!(matches!(err, StoreError::Io(_)), "{err:?}");
    drop(holder);
    let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(names.iter().all(|n| !n.contains("damaged")), "{names:?}");
    assert!(matches!(IndexStore::open(&path), Err(StoreError::IndexCorrupt(_))), "the damaged file is still in place");
}

/// A tree with every case a total has to get right: a hard-linked object in two directories (`with_links`),
/// unknown sizes, unread ranges (one under a directory, one deep in a large directory, one with no parent),
/// a directory large enough for a stored total, a small one that is walked, and an empty one.
///
/// /r (1): a/ (2) {x (4) = obj 5, z (8) unknown}; b/ (3) {y (7) = obj 5 when linked, w (10)}; locked/ (9, unread);
/// big/ (11) {300 files 100..=399, every 37th unknown; sub/ (12) {400..=409, unread; 410 = obj 5 when linked}};
/// empty/ (13).
fn rich_fixture(store: &IndexStore, scan: &str, state: ScanState, with_links: bool) {
    let rev = begin(store, scan);
    let mut objects = vec![obj(1, ObjectKind::Directory, None), obj(2, ObjectKind::Directory, None), obj(3, ObjectKind::Directory, None),
        obj(5, ObjectKind::File, Some(100)), obj(6, ObjectKind::File, None), obj(9, ObjectKind::Directory, None), obj(10, ObjectKind::File, Some(7)),
        obj(11, ObjectKind::Directory, None), obj(12, ObjectKind::Directory, None), obj(13, ObjectKind::Directory, None)];
    let mut entries = vec![ent(1, None, "/r", 1, ObjectKind::Directory), ent(2, Some(1), "a", 2, ObjectKind::Directory),
        ent(3, Some(1), "b", 3, ObjectKind::Directory), ent(4, Some(2), "x", 5, ObjectKind::File), ent(8, Some(2), "z", 6, ObjectKind::File),
        ent(9, Some(1), "locked", 9, ObjectKind::Directory), ent(10, Some(3), "w", 10, ObjectKind::File),
        ent(11, Some(1), "big", 11, ObjectKind::Directory), ent(12, Some(11), "sub", 12, ObjectKind::Directory),
        ent(13, Some(1), "empty", 13, ObjectKind::Directory)];
    if with_links {
        entries.push(ent(7, Some(3), "y", 5, ObjectKind::File));
        entries.push(ent(410, Some(12), "x-again", 5, ObjectKind::File));
    } else {
        objects.push(obj(7, ObjectKind::File, Some(100)));
        entries.push(ent(7, Some(3), "y", 7, ObjectKind::File));
    }
    for id in 100..=409u64 {
        let size = (id % 37 != 0).then_some(id * 1000);
        objects.push(obj(id, ObjectKind::File, size));
        entries.push(ent(id, Some(if id < 400 { 11 } else { 12 }), &format!("f{id}"), id, ObjectKind::File));
    }
    let skipped = [SkippedRow { parent_entry_id: Some(9), path: unix("/r/locked/deep"), reason: SkipReason::PermissionDenied },
        SkippedRow { parent_entry_id: Some(12), path: unix("/r/big/sub/private"), reason: SkipReason::PermissionDenied },
        SkippedRow { parent_entry_id: None, path: unix("/elsewhere"), reason: SkipReason::VolumeChanged }];
    // Two batches, as a backend writes them.
    store.write_batch(scan, rev, &objects[..8], &entries[..6], &skipped[..1]).unwrap();
    store.write_batch(scan, rev, &objects[8..], &entries[6..], &skipped[1..]).unwrap();
    if state == ScanState::Interrupted {
        assert_eq!(store.reconcile_hosts(|_| Liveness::Exited, "t").unwrap(), vec![scan.to_string()]);
    } else {
        store.publish(scan, rev, state, "t").unwrap();
    }
}

const RICH_IDS: [u64; 15] = [1, 2, 3, 4, 7, 8, 9, 10, 11, 12, 13, 100, 111, 405, 409];

fn rich_selections(with_links: bool) -> Vec<Vec<u64>> {
    let mut s: Vec<Vec<u64>> = RICH_IDS.iter().map(|id| vec![*id]).collect();
    s.extend([vec![], vec![2, 3], vec![4, 7], vec![2, 4], vec![1, 2, 11], vec![11, 3], vec![12, 2, 9], vec![100, 101, 4, 405], vec![9, 13, 12], vec![3, 12]]);
    if with_links {
        s.push(vec![4, 410]);
        s.push(vec![2, 12, 410]);
    }
    s
}

fn counted(store: &IndexStore, scan: &str) -> (u64, u64) {
    let count = |table: &str| store.conn().query_row(&format!("SELECT count(*) FROM {table} WHERE scan_id=?1"), [scan], |r| r.get::<_, i64>(0)).unwrap() as u64;
    (count("entries"), count("skipped"))
}

/// NEW-01: status and summary reads come from stored totals or a subtree walk, never from the loaded
/// revision, and give exactly what the full in-memory computation gives: for every entry, for overlapping
/// and hard-linked selections, in every terminal state, and for a revision an older program published
/// (no stored rows), which is then recorded so its next status read is a keyed lookup.
#[test]
fn stored_and_walked_totals_equal_the_full_tree_computation() {
    let dir = scratch("stored-totals");
    let store = IndexStore::open(&dir.join("index.db")).unwrap();
    let mut n = 0;
    for state in [ScanState::Completed, ScanState::Partial, ScanState::Cancelled, ScanState::Interrupted, ScanState::Failed] {
        for with_links in [true, false] {
            n += 1;
            let scan = format!("s{n}");
            rich_fixture(&store, &scan, state, with_links);
            let check = |label: &str| {
                for id in RICH_IDS {
                    let full = store.summarize_full(&scan, &[id]).unwrap();
                    assert_eq!(store.folder_total(&scan, id).unwrap(), full, "{label} {state:?} links={with_links} folder_total({id})");
                    assert_eq!(store.summarize(&scan, &[id]).unwrap(), full, "{label} {state:?} links={with_links} summarize([{id}])");
                }
                for sel in rich_selections(with_links) {
                    assert_eq!(store.summarize(&scan, &sel).unwrap(), store.summarize_full(&scan, &sel).unwrap(), "{label} {state:?} links={with_links} {sel:?}");
                }
                let status = store.scan_status(&scan).unwrap();
                assert_eq!((status.entries_seen, status.skipped_ranges), counted(&store, &scan), "{label} {state:?}");
            };
            check("stored");
            let stored: Vec<i64> = {
                let mut stmt = store.conn().prepare("SELECT entry_id FROM subtree_totals WHERE scan_id=?1 ORDER BY entry_id").unwrap();
                stmt.query_map([&scan], |r| r.get(0)).unwrap().map(std::result::Result::unwrap).collect()
            };
            assert_eq!(stored, [1, 11], "{state:?}: the root and the one directory with at least {STORED_TOTAL_MIN_ENTRIES} entries");
            let shared: Option<i64> = store.conn().query_row("SELECT shared_objects FROM revision_summaries WHERE scan_id=?1", [&scan], |r| r.get(0)).unwrap();
            assert_eq!(shared, Some(if with_links { 1 } else { 0 }));

            // A revision published by an older program: nothing stored, same answers.
            store.conn().execute("DELETE FROM subtree_totals WHERE scan_id=?1", [&scan]).unwrap();
            store.conn().execute("DELETE FROM revision_summaries WHERE scan_id=?1", [&scan]).unwrap();
            check("walked");
            // Reading its root recorded the root's total and the counts (not the hard-link fact, which is unknown).
            let roots: Vec<i64> = {
                let mut stmt = store.conn().prepare("SELECT entry_id FROM subtree_totals WHERE scan_id=?1").unwrap();
                stmt.query_map([&scan], |r| r.get(0)).unwrap().map(std::result::Result::unwrap).collect()
            };
            assert_eq!(roots, [1]);
            let row: (i64, i64, Option<i64>) =
                store.conn().query_row("SELECT entries, skipped, shared_objects FROM revision_summaries WHERE scan_id=?1", [&scan], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
            let (entries, skipped) = counted(&store, &scan);
            assert_eq!(row, (entries as i64, skipped as i64, None));
            check("recorded");
        }
    }
    // Errors stay what they were.
    assert!(matches!(store.folder_total("s1", 999), Err(StoreError::NotFound(_))));
    assert!(matches!(store.summarize("s1", &[1, 999]), Err(StoreError::NotFound(_))));
    let rev = begin(&store, "staging");
    store.write_batch("staging", rev, &[obj(1, ObjectKind::Directory, None)], &[ent(1, None, "/r", 1, ObjectKind::Directory)], &[]).unwrap();
    assert!(matches!(store.folder_total("staging", 1), Err(StoreError::InvalidState(_))));
    assert!(matches!(store.summarize("staging", &[1]), Err(StoreError::InvalidState(_))));
    // Dropping a scan drops what was stored for it.
    store.drop_scan("s1").unwrap();
    for table in ["subtree_totals", "revision_summaries"] {
        let left: i64 = store.conn().query_row(&format!("SELECT count(*) FROM {table} WHERE scan_id='s1'"), [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0, "{table}");
    }
}

/// The counts a running scan's status shows are kept per batch (no row counting while it runs), and a
/// revision begun by an older program (no counter row) is still counted correctly.
#[test]
fn status_counts_are_kept_per_batch_while_staging() {
    let dir = scratch("staging-counts");
    let store = IndexStore::open(&dir.join("index.db")).unwrap();
    let rev = begin(&store, "s1");
    assert_eq!((store.scan_status("s1").unwrap().entries_seen, store.scan_status("s1").unwrap().skipped_ranges), (0, 0));
    store.write_batch("s1", rev, &[obj(1, ObjectKind::Directory, None)], &[ent(1, None, "/r", 1, ObjectKind::Directory)],
        &[SkippedRow { parent_entry_id: Some(1), path: unix("/r/x"), reason: SkipReason::PermissionDenied }]).unwrap();
    store.write_batch("s1", rev, &[obj(2, ObjectKind::File, Some(3)), obj(3, ObjectKind::File, None)],
        &[ent(2, Some(1), "a", 2, ObjectKind::File), ent(3, Some(1), "b", 3, ObjectKind::File)], &[]).unwrap();
    let s = store.scan_status("s1").unwrap();
    assert_eq!((s.published, s.entries_seen, s.skipped_ranges), (false, 3, 1));
    let stored: (i64, i64) = store.conn().query_row("SELECT entries, skipped FROM revision_summaries WHERE scan_id='s1'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(stored, (3, 1));
    store.conn().execute("DELETE FROM revision_summaries WHERE scan_id='s1'", []).unwrap();
    store.write_batch("s1", rev, &[obj(4, ObjectKind::File, Some(1))], &[ent(4, Some(1), "c", 4, ObjectKind::File)], &[]).unwrap();
    let s = store.scan_status("s1").unwrap();
    assert_eq!((s.entries_seen, s.skipped_ranges), (4, 1), "no counter row: counted");
    assert_eq!(store.publish("s1", rev, ScanState::Completed, "t").unwrap(), ScanState::Partial);
    let s = store.scan_status("s1").unwrap();
    assert_eq!((s.published, s.entries_seen, s.skipped_ranges), (true, 4, 1), "recounted at publish");
    let root = store.folder_total("s1", 1).unwrap();
    assert_eq!((root.covered_entries, root.logical.known, root.logical.unknown_count, root.unread_ranges), (4, Bytes(4), 2, 1));
}
/// The treemap's "rest" tile: the other children of a folder, added plainly from one unsorted read.
#[test]
fn children_rest_adds_the_other_children_exactly_and_within_its_budget() {
    let dir = scratch("children-rest");
    let sizes: Vec<Option<u64>> = (0..137).map(|i| if i % 11 == 0 { None } else { Some(u64::MAX / 2 + i) }).collect();
    let store = flat(&dir, &sizes);
    let top: BTreeSet<u64> = [10, 11, 12].into();
    let rest = store.children_rest("s1", 1, 1, &top, 1000).unwrap();
    let others: Vec<Option<u64>> = sizes.iter().enumerate().filter(|(i, _)| !top.contains(&(10 + *i as u64))).map(|(_, s)| *s).collect();
    let known: u128 = others.iter().flatten().map(|v| u128::from(*v)).sum();
    assert_eq!(rest, ChildrenRest { count: 134, known_logical: known, unknown_count: others.iter().filter(|s| s.is_none()).count() as u64, complete: true });
    assert!(known > u128::from(u64::MAX), "the sum is exact beyond u64");
    let bounded = store.children_rest("s1", 1, 1, &top, 50).unwrap();
    assert_eq!((bounded.count, bounded.complete), (50, false));
    assert!(store.children_rest("s1", 1, 1, &top, 134).unwrap().complete, "exactly the budget is complete");
    assert_eq!(store.children_rest("s1", 1, 10, &BTreeSet::new(), 10).unwrap(), ChildrenRest { complete: true, ..ChildrenRest::default() }, "a file has no children");
    assert!(matches!(store.children_rest("s1", 2, 1, &top, 10), Err(StoreError::CursorStale(_))), "bound to the revision of the first page");
}
/// NEW-01 regression: each step of the subtree walk is one lookup by parent, never a pass over the revision
/// (SQLite's planner picked that for a plain join, which made folder totals quadratic in the scan's size).
#[test]
fn the_subtree_walk_looks_children_up_by_parent_and_reads_only_the_subtree() {
    let dir = scratch("walk-plan");
    let store = flat(&dir, &vec![Some(1); 3000]);
    let plan: Vec<String> = {
        let mut stmt = store.conn().prepare(&format!("EXPLAIN QUERY PLAN {WALK_SQL}")).unwrap();
        stmt.query_map(params!["s1", 1, 10], |r| r.get::<_, String>(3)).unwrap().map(std::result::Result::unwrap).collect()
    };
    let step = plan.iter().position(|l| l == "RECURSIVE STEP").expect("a recursive walk");
    assert!(plan[step + 1..].iter().any(|l| l.starts_with("SEARCH e USING INDEX entries_parent")), "{plan:?}");
    assert!(!plan.iter().any(|l| l.starts_with("SCAN e") || l.contains("entries_logical")), "{plan:?}");
    // Walking one file of a 3,001-entry revision takes a handful of VM steps, not one pass per queued row.
    let mut stmt = store.conn().prepare(WALK_SQL).unwrap();
    let rows = stmt.query_map(params!["s1", 1, 10], |r| r.get::<_, i64>(0)).unwrap().count();
    assert_eq!(rows, 1);
    let steps = stmt.get_status(rusqlite::StatementStatus::VmStep);
    assert!(steps < 500, "{steps} VM steps for a one-entry subtree");
}