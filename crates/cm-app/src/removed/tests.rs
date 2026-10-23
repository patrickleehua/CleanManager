use std::cell::RefCell;
use std::collections::BTreeMap;

use super::*;
use crate::execute::{ApplyContext, apply_plan};
use crate::recovery::{RecoveryContext, apply_recovery_plan, create_recovery_plan};
use cm_core::analysis::{AnalysisInput, ResolvedRoot, analyze};
use cm_core::authz::{Receipt, ReceiptSource};
use cm_core::bytes::Bytes;
use cm_core::capability::Platform;
use cm_core::execute::{ActionOutcome, ActionTarget, FileActions, LiveCheck, Reconciled, Recovery};
use cm_core::host::HostRef;
use cm_core::model::{CloudState, IdentityQuality, ObjectKind, ScanState};
use cm_core::plan::{PlanInput, build_plan};
use cm_core::recovery::{MetadataCoverage, QuarantineState, QuarantinedObject, RecoveryActions, RecoveryKind, RecoveryOutcome, RecoveryReceipt};
use cm_core::rules::{ActionKind, Impact, Match, ObjectType, Owner, OwnerKind, RebuildClass, RootResolver, Rule, RuleSet, RuleStatus, Thresholds, Tier};
use cm_core::scan::{EntryRow, ObjectRow};
use cm_core::time::utc_from_unix;
use cm_store::index::{ScanInit, SizeBasis};

const T0: i64 = 1_800_000_000;
const PRINCIPAL: &str = "user:alice";

/// In-memory volume keyed by path text (path → file id); quarantine location → file id.
#[derive(Default)]
struct Fake {
    files: RefCell<BTreeMap<String, String>>,
    quarantine: RefCell<BTreeMap<String, String>>,
}

impl FileActions for Fake {
    fn check(&self, t: &ActionTarget<'_>) -> LiveCheck {
        match self.files.borrow().get(&t.path.lossy()) {
            None => LiveCheck::Missing,
            Some(id) if *id != t.identity.file_id => LiveCheck::IdentityChanged,
            Some(_) => LiveCheck::Match,
        }
    }
    fn quarantine(&self, t: &ActionTarget<'_>, dir: &RawPath, name: &str) -> ActionOutcome {
        let id = self.files.borrow_mut().remove(&t.path.lossy()).unwrap();
        let loc = format!("{}/{name}", dir.lossy());
        self.quarantine.borrow_mut().insert(loc.clone(), id);
        ActionOutcome::Succeeded { recovery: Recovery::Quarantine { location: RawPath::unix(loc.into_bytes()).unwrap() } }
    }
    fn trash(&self, t: &ActionTarget<'_>) -> ActionOutcome {
        self.files.borrow_mut().remove(&t.path.lossy());
        ActionOutcome::Succeeded { recovery: Recovery::Trash }
    }
    fn delete(&self, t: &ActionTarget<'_>) -> ActionOutcome {
        self.files.borrow_mut().remove(&t.path.lossy());
        ActionOutcome::Succeeded { recovery: Recovery::Unavailable }
    }
    fn quarantine_dir_for(&self, _: &ActionTarget<'_>, op: &str) -> Option<RawPath> {
        RawPath::unix(format!("/r/.q/{op}").into_bytes()).ok()
    }
    fn reconcile(&self, _: &ActionTarget<'_>, _: Mode, _: Option<&RawPath>) -> Reconciled {
        Reconciled::StillUnknown
    }
    fn free_space(&self, _: &RawPath) -> Option<u64> {
        Some(1_000_000)
    }
}

impl RecoveryActions for Fake {
    fn probe(&self, _: &QuarantinedObject<'_>) -> QuarantineState {
        QuarantineState::Recoverable
    }
    fn restore(&self, o: &QuarantinedObject<'_>) -> RecoveryOutcome {
        let id = self.quarantine.borrow_mut().remove(&o.location.lossy()).unwrap();
        self.files.borrow_mut().insert(o.original_path.lossy(), id);
        RecoveryOutcome::Done
    }
    fn purge(&self, o: &QuarantinedObject<'_>) -> RecoveryOutcome {
        self.quarantine.borrow_mut().remove(&o.location.lossy());
        RecoveryOutcome::Done
    }
    fn reconcile(&self, _: &QuarantinedObject<'_>, _: RecoveryKind) -> Reconciled {
        Reconciled::StillUnknown
    }
    fn free_space(&self, _: &RawPath) -> Option<u64> {
        Some(1_000_000)
    }
    fn metadata_coverage(&self) -> MetadataCoverage {
        MetadataCoverage { preserved: vec![], limits: vec![] }
    }
}

fn obj(id: u64, kind: ObjectKind, size: Option<u64>) -> ObjectRow {
    ObjectRow { object_id: id, volume_id: "v".into(), file_id: format!("inode64:{id:016x}"), generation: None, quality: IdentityQuality::Strong,
        kind, logical: size, allocated: size, link_count: Some(1), cloud: CloudState::NotCloud, modified_unix: Some(T0 - 1000), shared_blocks: None }
}

fn ent(id: u64, parent: Option<u64>, name: &str, kind: ObjectKind) -> EntryRow {
    EntryRow { entry_id: id, parent_entry_id: parent, name: RawPath::unix(name.as_bytes().to_vec()).unwrap(), object_id: id, kind, boundary: None }
}

fn rule() -> Rule {
    Rule {
        schema_version: "2.0".into(), rule_id: "cache".into(), version: "1.0.0".into(), status: RuleStatus::Enabled, platforms: vec![Platform::Macos],
        category: "app_caches".into(), max_tier: Tier::DefaultSelected, rebuild_class: RebuildClass::LocalRebuild,
        owner: Owner { kind: OwnerKind::App, id: "t".into() }, root_resolver: RootResolver::ApplicationCache,
        matcher: Match { object_type: ObjectType::Any, relative_components: vec!["Cache".into()], name_glob: None, extensions: vec![],
            require_cachedir_tag: false, archive_payload: false, max_depth: 0 },
        exclude: vec![], probes: vec![], thresholds: Thresholds { min_age_days: None, age_basis: None, min_size_bytes: None, retention_days: None },
        action_kind: ActionKind::Filesystem, adapter_ref: None, allowed_modes: vec![Mode::Trash, Mode::Quarantine, Mode::Delete],
        requires_admin: false, protect_descendants: true, follow_links: false,
        impact: Impact { summary: "s".into(), regeneration: "r".into(), offline_effect: "o".into() },
        tested_registry_ref: None, evidence_refs: vec!["e".into()],
    }
}

fn host() -> HostRef {
    HostRef { kind: "cli".into(), pid: 1, started_at: "t".into() }
}

/// A published scan of `/r`: app/Cache/{a,b,c} (100 bytes each), app/keep (50 bytes), app/odd (size unknown).
fn publish_scan(stores: &Stores, scan_id: &str, finished: i64) {
    stores.index.begin_scan(&ScanInit { scan_id: scan_id.into(), scope_id: "scope".into(), root: RawPath::unix(b"/r".to_vec()).unwrap(),
        backend: "test".into(), host: host(), started_at: utc_from_unix(finished - 5) }).unwrap();
    let objects = [obj(1, ObjectKind::Directory, None), obj(2, ObjectKind::Directory, None), obj(3, ObjectKind::Directory, None),
        obj(4, ObjectKind::File, Some(100)), obj(5, ObjectKind::File, Some(100)), obj(6, ObjectKind::File, Some(100)),
        obj(7, ObjectKind::File, Some(50)), obj(8, ObjectKind::File, None)];
    let entries = [ent(1, None, "/r", ObjectKind::Directory), ent(2, Some(1), "app", ObjectKind::Directory), ent(3, Some(2), "Cache", ObjectKind::Directory),
        ent(4, Some(3), "a", ObjectKind::File), ent(5, Some(3), "b", ObjectKind::File), ent(6, Some(3), "c", ObjectKind::File),
        ent(7, Some(2), "keep", ObjectKind::File), ent(8, Some(2), "odd", ObjectKind::File)];
    stores.index.write_batch(scan_id, 1, &objects, &entries, &[]).unwrap();
    stores.index.publish(scan_id, 1, ScanState::Completed, &utc_from_unix(finished)).unwrap();
}

fn stores(name: &str) -> Stores {
    let dir = std::env::temp_dir().join(format!("cm-removed-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Stores::open(&dir).unwrap()
}

/// Plan the Cache candidate of scan `s` in `mode`, approve it and apply it at `now` on a volume holding the scanned files.
fn apply_cache_plan(s: &Stores, mode: Mode, plan_id: &str, now: i64, fake: &Fake) {
    let view = s.index.view("s").unwrap();
    let rules = RuleSet { rules: vec![rule()], digest: "rs".into() };
    let roots = [ResolvedRoot { rule_id: "cache".into(), path: RawPath::unix(b"/r/app".to_vec()).unwrap(), volume_id: "v".into(), file_id: "inode64:0000000000000002".into() }];
    let set = analyze(&view, &rules, &AnalysisInput { scan_id: "s", revision: 1, now_unix: T0, roots: &roots, protected: &[] });
    let cid = set.candidates.iter().find(|c| c.path.lossy() == "/r/app/Cache").unwrap().candidate_id.clone();
    let plan = build_plan(&view, &set, &[cid], mode, &PlanInput { plan_id, scope_id: "scope", created_unix: now - 5, observed_at: "t", policy_digest: "0" }).unwrap();
    s.audit.save_plan(&plan).unwrap();
    s.audit.save_receipt(&Receipt { receipt_id: format!("r-{plan_id}"), plan_id: plan_id.into(), plan_digest: plan.digest.clone(), mode,
        principal: PRINCIPAL.into(), source: ReceiptSource::InteractiveReview, policy_id: None, policy_version: None,
        created_at: plan.body.created_at.clone(), expires_at: plan.body.expires_at.clone(), revoked_at: None }).unwrap();
    for (i, n) in ["a", "b", "c"].iter().enumerate() {
        fake.files.borrow_mut().insert(format!("/r/app/Cache/{n}"), format!("inode64:{:016x}", i + 4));
    }
    let h = host();
    let ctx = ApplyContext { principal: PRINCIPAL, host: &h, current_rule_set_digest: "rs", now: &move || now, cancel: &|| false, fault: &|_, _| false, protect: &|_| Ok(()) };
    let r = apply_plan(s, fake, plan_id, &ctx).unwrap();
    assert_eq!((r.status.as_str(), r.succeeded), ("completed", 3), "{r:?}");
}

fn entry(s: &Stores, scan: &str, id: u64) -> EntryItem {
    s.index.get_entry(scan, id).unwrap()
}

#[test]
fn a_trash_plan_applied_after_the_scan_marks_its_entries_and_their_folders() {
    let s = stores("trash");
    publish_scan(&s, "s", T0);
    assert!(removed_overlay(&s, "s").unwrap().is_empty(), "nothing was applied yet");
    let fake = Fake::default();
    apply_cache_plan(&s, Mode::Trash, "p1", T0 + 60, &fake);
    let o = removed_overlay(&s, "s").unwrap();
    for id in [4, 5, 6] {
        let m = o.mark(id).expect("each planned file is marked");
        assert_eq!((m.mode, m.recoverable, m.operation_id.starts_with("op_")), (RemovedMode::Trash, true, true));
        assert_eq!(m.at, utc_from_unix(T0 + 60));
    }
    assert!(o.mark(3).is_none() && o.mark(7).is_none(), "the folder itself and its siblings stay");
    // The folder and every ancestor carry what went beneath them, from the snapshot's stored sizes.
    for dir in [3, 2, 1] {
        assert_eq!(o.beneath(dir), Some(RemovedTotal { count: 3, known_bytes: 300, unknown_count: 0 }), "dir {dir}");
    }
    assert_eq!(o.total, RemovedTotal { count: 3, known_bytes: 300, unknown_count: 0 });
    assert_eq!(o.total.known_text().as_deref(), Some("300"));
    assert_eq!(o.of_entry(&entry(&s, "s", 4)).map(|r| r.inherited), Some(false));
    assert!(o.of_entry(&entry(&s, "s", 7)).is_none());
    // A candidate view asks whether anything of an entry was touched: the file itself, or (the Cache folder) its contents.
    assert!(matches!(o.touched(4, &entry(&s, "s", 4).path), Some(Touched::Removed(_))));
    assert!(matches!(o.touched(3, &entry(&s, "s", 3).path), Some(Touched::Inside(t)) if t.count == 3));
    assert!(o.touched(7, &entry(&s, "s", 7).path).is_none());
    // Cached under the audit revision: nothing changed, the same key.
    let again = removed_overlay(&s, "s").unwrap();
    assert_eq!((again.audit_revision.clone(), again.signature.clone()), (o.audit_revision.clone(), o.signature.clone()));

    // A newer scan of the same root finished after the operation: it shows the volume as it is, no overlay from it,
    // even where (as here) it still lists the same names.
    publish_scan(&s, "s2", T0 + 120);
    let newer = removed_overlay(&s, "s2").unwrap();
    assert!(newer.is_empty() && newer.total == RemovedTotal::default());
    assert!(!removed_overlay(&s, "s").unwrap().is_empty(), "the older snapshot keeps saying what left it");
}

#[test]
fn a_restore_from_the_quarantine_clears_the_mark_and_a_purge_makes_it_permanent() {
    let s = stores("restore");
    publish_scan(&s, "s", T0);
    let fake = Fake::default();
    apply_cache_plan(&s, Mode::Quarantine, "p1", T0 + 60, &fake);
    let o = removed_overlay(&s, "s").unwrap();
    assert!([4, 5, 6].iter().all(|id| o.mark(*id).is_some_and(|m| m.mode == RemovedMode::Quarantine && m.recoverable)));
    let q: Vec<(String, String)> = s.audit.quarantine_items().unwrap().into_iter().map(|r| (r.0, r.1)).collect();
    let id_of = |name: &str| q.iter().find(|(_, record)| record.contains(&format!("/r/app/Cache/{name}\""))).unwrap().0.clone();
    let (qa, qb) = (id_of("a"), id_of("b"));

    let h = host();
    let recover = |kind: RecoveryKind, ids: Vec<String>, plan_id: &str, now: i64| {
        let p = create_recovery_plan(&s, kind, &ids, plan_id, now).unwrap();
        s.audit.save_recovery_receipt(&RecoveryReceipt {
            receipt_id: format!("r-{plan_id}"), plan_id: plan_id.into(), plan_digest: p.digest.clone(), recovery_kind: kind, principal: PRINCIPAL.into(),
            source: ReceiptSource::InteractiveReview, created_at: p.body.created_at.clone(), expires_at: p.body.expires_at.clone(), revoked_at: None,
        }).unwrap();
        let ctx = RecoveryContext { principal: PRINCIPAL, host: &h, now: &move || now + 1, cancel: &|| false, fault: &|_, _| false };
        let r = apply_recovery_plan(&s, &fake, plan_id, &ctx).unwrap();
        assert_eq!(r.status, "completed", "{r:?}");
    };
    recover(RecoveryKind::Restore, vec![qa], "rp1", T0 + 100);
    recover(RecoveryKind::Purge, vec![qb], "rp2", T0 + 110);
    let after = removed_overlay(&s, "s").unwrap();
    assert_ne!(after.audit_revision, o.audit_revision, "the restore changed the audit revision");
    assert_ne!(after.signature, o.signature);
    let marked: Vec<u64> = {
        let mut v: Vec<u64> = after.marks().map(|(id, _)| id).collect();
        v.sort_unstable();
        v
    };
    let (a, b, c) = (entry(&s, "s", 4), entry(&s, "s", 5), entry(&s, "s", 6));
    assert!(after.of_entry(&a).is_none(), "the restored file is no longer marked: {marked:?}");
    assert_eq!(after.of_entry(&b).map(|r| (r.mark.mode, r.mark.recoverable)), Some((RemovedMode::Permanent, false)), "purged from the quarantine");
    assert_eq!(after.of_entry(&c).map(|r| (r.mark.mode, r.mark.recoverable)), Some((RemovedMode::Quarantine, true)));
    assert_eq!(after.beneath(3), Some(RemovedTotal { count: 2, known_bytes: 200, unknown_count: 0 }));
}

fn item(path: &str, kind: ObjectKind, logical: Option<u64>, unknown_descendants: Option<u64>) -> EntryItem {
    EntryItem {
        entry_id: "9".into(), parent_entry_id: Some("1".into()), object_id: "9".into(), path: RawPath::unix(path.as_bytes().to_vec()).unwrap(), kind,
        logical_bytes: logical.map(Bytes), allocated_bytes: None, size_basis: if logical.is_some() { SizeBasis::Entity } else { SizeBasis::Unknown },
        unknown_descendants, link_count: None, boundary: None, identity_quality: IdentityQuality::Strong,
    }
}

#[test]
fn unknown_sizes_stay_unknown_never_zero() {
    let mut t = RemovedTotal::default();
    t.add(&item("/r/x", ObjectKind::File, None, None));
    assert_eq!((t.count, t.known_bytes, t.unknown_count, t.known_text()), (1, 0, 1, None), "nothing known: no number at all");
    t.add(&item("/r/d", ObjectKind::Directory, Some(40), Some(2)));
    assert_eq!((t.count, t.known_bytes, t.unknown_count, t.known_text().as_deref()), (2, 40, 2, Some("40")), "a partly known folder counts as unknown too");
    let mut z = RemovedTotal::default();
    z.add(&item("/r/empty", ObjectKind::File, Some(0), None));
    assert_eq!(z.known_text().as_deref(), Some("0"), "a known empty file is a known 0");
}

#[test]
fn paths_resolve_below_the_root_with_windows_case_and_separators_folded() {
    let w = |s: &str| RawPath::windows(s.encode_utf16().collect()).unwrap();
    let names = |v: Option<Vec<Vec<u16>>>| v.map(|v| v.into_iter().map(|n| String::from_utf16(&n).unwrap()).collect::<Vec<_>>());
    assert_eq!(names(components_below(&w(r"C:\"), &w(r"c:\Users\Lin"))), Some(vec!["users".into(), "lin".into()]));
    assert_eq!(names(components_below(&w(r"D:\Work"), &w("D:/work/Videos/A.mov"))), Some(vec!["videos".into(), "a.mov".into()]));
    assert_eq!(names(components_below(&w(r"D:\Work"), &w(r"D:\Work\"))), Some(vec![]));
    assert_eq!(components_below(&w(r"D:\Work"), &w(r"D:\Workshop\a")), None, "a longer sibling name is not inside");
    assert_eq!(components_below(&w(r"D:\Work"), &RawPath::unix(b"/r".to_vec()).unwrap()), None);
    let u = |s: &str| RawPath::unix(s.as_bytes().to_vec()).unwrap();
    assert_eq!(components_below(&u("/r"), &u("/R/a")), None, "macOS paths are compared as they are");
    // An inherited mark: a removed folder marks what the snapshot lists inside it.
    let mut o = RemovedOverlay::empty("s", "x".into());
    o.marks.insert(3, RemovedMark { mode: RemovedMode::Trash, operation_id: "op".into(), at: "t".into(), recoverable: true });
    o.by_path.insert(path_key(&u("/r/app/Cache")), 3);
    assert_eq!(o.of_path(&u("/r/app/Cache/a/b")).map(|r| r.inherited), Some(true));
    assert_eq!(o.of_path(&u("/r/app/Cache")).map(|r| r.inherited), Some(false));
    assert!(o.of_path(&u("/r/app/Cachex")).is_none() && o.of_path(&u("/r/app")).is_none());
}
