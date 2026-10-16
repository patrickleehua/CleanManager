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
    let rebuilt = Stores::rebuild_index(&dir).unwrap();
    assert!(matches!(rebuilt.scan_status("s1"), Err(StoreError::NotFound(_))));
    let audit = AuditStore::open(&dir.join("audit.db")).unwrap();
    assert_eq!(audit.operation_state("op1").unwrap(), "created", "rebuild never touches audit records");
}

#[test]
fn incompatible_or_damaged_index_is_reported_not_silently_reset() {
    let dir = scratch("damaged");
    let path = dir.join("index.db");
    {
        let store = IndexStore::open(&path).unwrap();
        store.conn.execute("UPDATE meta SET value='99' WHERE key='schema_version'", []).unwrap();
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
