use super::*;
use crate::analysis::{EntryFacts, MemView};
use crate::capability::Platform;
use crate::plan::v2::{PlanKind, validate};
use crate::protect::{HardDenyClass, HardDenyRoot, MatchKind, UserEntry};

const AT: &str = "2026-10-15T00:00:00Z";

struct B {
    view: MemView,
    next: u64,
}

impl B {
    fn new(root: &str) -> Self {
        let mut b = B { view: MemView { platform: Some(Platform::Windows), ..Default::default() }, next: 0 };
        b.view.root = b.add(None, root, ObjectKind::Directory, None);
        b
    }
    fn add(&mut self, parent: Option<u64>, name: &str, kind: ObjectKind, size: Option<u64>) -> u64 {
        self.next += 1;
        let id = self.next;
        self.view.names.insert(id, RawPath::windows(name.encode_utf16().collect()).unwrap());
        self.view.parents.insert(id, parent);
        if let Some(p) = parent {
            self.view.kids.entry(p).or_default().push(id);
        }
        let file_id = format!("fileid128:{id:032x}");
        self.view.facts.insert(id, EntryFacts {
            kind, boundary: false, object_id: id, logical: size, allocated: size, volume_id: "v".into(), file_id: file_id.clone(),
            quality: IdentityQuality::Strong, cloud: CloudState::NotCloud, modified_unix: Some(1_800_000_000), link_count: Some(1), skipped_here: 0,
        });
        self.view.identities.insert(("v".into(), file_id), id);
        id
    }
    fn dir(&mut self, parent: u64, name: &str) -> u64 {
        self.add(Some(parent), name, ObjectKind::Directory, None)
    }
    fn file(&mut self, parent: u64, name: &str, size: u64) -> u64 {
        self.add(Some(parent), name, ObjectKind::File, Some(size))
    }
    fn facts(&mut self, e: u64) -> &mut EntryFacts {
        self.view.facts.get_mut(&e).unwrap()
    }
}

fn input() -> ManualInput<'static> {
    ManualInput {
        plan_id: "mp_1", scope_id: Some("scope_x"), scan_id: Some("s_x"), scan_revision: Some(1), created_unix: 1_800_000_000, observed_at: AT,
        policy_digest: crate::plan::v2::NO_RULES_DIGEST,
    }
}

fn open_guard(user: &UserProtection) -> SelectionGuard<'_> {
    SelectionGuard { hard: None, user: Ok(user) }
}

fn refused(r: Result<PlanV2, ManualError>) -> &'static str {
    match r {
        Err(ManualError::Refused { refusal, .. }) => refusal.reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

struct Tree {
    b: B,
    docs: u64,
    a_txt: u64,
    b_txt: u64,
    video: u64,
    proj: u64,
    creds: u64,
    partial: u64,
    with_link: u64,
    db: u64,
    cloud: u64,
}

fn tree() -> Tree {
    let mut b = B::new("C:\\r");
    let r = b.view.root;
    let docs = b.dir(r, "docs");
    let a_txt = b.file(docs, "a.txt", 10);
    let b_txt = b.file(docs, "b.txt", 20);
    let video = b.file(r, "video.mov", 1 << 30);
    let proj = b.dir(r, "proj");
    b.dir(proj, ".git");
    b.file(proj, "main.c", 5);
    let creds = b.dir(r, "old-home");
    let ssh = b.dir(creds, ".ssh");
    b.file(ssh, "id_ed25519", 1);
    let partial = b.dir(r, "partial");
    b.file(partial, "c.txt", 1);
    b.facts(partial).skipped_here = 1;
    let with_link = b.dir(r, "withlink");
    let link = b.add(Some(with_link), "junction", ObjectKind::Reparse, None);
    b.facts(link).boundary = true;
    let db = b.file(r, "data.db", 4096);
    let cloud = b.file(r, "cloud.txt", 100);
    b.facts(cloud).cloud = CloudState::Placeholder;
    Tree { b, docs, a_txt, b_txt, video, proj, creds, partial, with_link, db, cloud }
}

/// MTR-01: one manual plan of fs_trash actions, never policy-matchable, valid for its kind.
#[test]
fn a_file_and_a_folder_become_a_trash_only_plan() {
    let t = tree();
    let user = UserProtection::empty(Platform::Windows);
    let plan = build_trash_plan(&t.b.view, &[t.video, t.docs], &open_guard(&user), &input()).unwrap();
    assert_eq!(plan.body.plan_kind, PlanKind::ManualSelection);
    assert!(!plan.body.policy_matchable && !plan.body.requires_admin);
    assert_eq!(validate(&plan.body), Ok(()));
    let PlanAction::FsTrash(file) = &plan.body.items[0] else { panic!("{:?}", plan.body.items[0]) };
    assert_eq!(file.target_location, TargetLocation::Original);
    assert_eq!(file.target.logical_bytes, Bytes(1 << 30));
    assert!(file.subtree.is_none() && plan.meta[0].members.is_empty());
    let PlanAction::FsTrash(dir) = &plan.body.items[1] else { panic!() };
    assert_eq!(dir.target.kind, ItemKind::Directory);
    let sub = dir.subtree.as_ref().unwrap();
    assert_eq!((sub.items, sub.logical_bytes), (2, Bytes(30)));
    assert_eq!(plan.meta[1].members.len(), 2);
    assert_eq!(members_digest(&plan.meta[1].members).unwrap(), sub.members_sha256);
    crate::plan::v2::check_meta(&plan).unwrap();
}

/// MTR-02: protected objects, protected descendants and protected ancestors are view-only.
#[test]
fn protected_objects_and_their_relatives_are_refused() {
    let t = tree();
    let user = UserProtection::empty(Platform::Windows);
    let g = open_guard(&user);
    assert_eq!(refused(build_trash_plan(&t.b.view, &[t.proj], &g, &input())), "protected", "a Git worktree root");
    assert_eq!(refused(build_trash_plan(&t.b.view, &[t.db], &g, &input())), "protected", "a database file");
    let ins = inspect(&t.b.view, t.creds, &g);
    assert_eq!(ins.refusal.as_ref().unwrap().reason, "protected_descendants");
    assert!(ins.protected.iter().any(|h| h.path.display().ends_with(".ssh")), "{:?}", ins.protected);
    assert!(ins.refusal.unwrap().message.contains("Select unprotected descendants"));
    // A file inside a worktree is protected through its ancestor.
    let main_c = t.b.view.children(t.proj)[1];
    assert_eq!(refused(build_trash_plan(&t.b.view, &[main_c], &g, &input())), "protected");

    // User list: an exact entry on a descendant refuses the folder, and names the descendant.
    let mut user = UserProtection::empty(Platform::Windows);
    user.add(UserEntry::path("u1", MatchKind::Exact, "C:\\r\\docs\\b.txt", AT)).unwrap();
    let ins = inspect(&t.b.view, t.docs, &open_guard(&user));
    assert_eq!(ins.refusal.unwrap().reason, "protected_descendants");
    assert_eq!(ins.protected_count, 1);
    assert!(build_trash_plan(&t.b.view, &[t.a_txt], &open_guard(&user), &input()).is_ok(), "an unprotected sibling stays selectable");
    assert_eq!(refused(build_trash_plan(&t.b.view, &[t.b_txt], &open_guard(&user), &input())), "protected");

    // Hard deny by identity, on the object and as a covering ancestor.
    let hard = HardDenySet::new([HardDenyRoot { class: HardDenyClass::SystemRoot, volume_id: "v".into(), file_id: t.b.view.facts[&t.docs].file_id.clone(), display: "C:\\r\\docs".into() }]);
    let g = SelectionGuard { hard: Some(&hard), user: Ok(&user) };
    assert_eq!(refused(build_trash_plan(&t.b.view, &[t.a_txt], &g, &input())), "protected");
    assert_eq!(refused(build_trash_plan(&t.b.view, &[t.video], &SelectionGuard { hard: Some(&hard), user: Err("damaged list") }, &input())), "protection_unavailable");
}

/// MTR-02: a folder whose subtree was not read completely is refused as a whole; its known parts stay selectable.
#[test]
fn incomplete_subtrees_are_refused() {
    let t = tree();
    let user = UserProtection::empty(Platform::Windows);
    let g = open_guard(&user);
    assert_eq!(refused(build_trash_plan(&t.b.view, &[t.partial], &g, &input())), "incomplete_subtree");
    assert_eq!(refused(build_trash_plan(&t.b.view, &[t.with_link], &g, &input())), "incomplete_subtree");
    let c_txt = t.b.view.children(t.partial)[0];
    assert!(build_trash_plan(&t.b.view, &[c_txt], &g, &input()).is_ok(), "known descendant of a partial folder");
    let link = t.b.view.children(t.with_link)[0];
    assert_eq!(refused(build_trash_plan(&t.b.view, &[link], &g, &input())), "link_or_reparse");
    assert_eq!(refused(build_trash_plan(&t.b.view, &[t.cloud], &g, &input())), "cloud_placeholder");
    assert_eq!(refused(build_trash_plan(&t.b.view, &[t.b.view.root], &g, &input())), "scan_root");
    let mut t2 = tree();
    t2.b.facts(t2.a_txt).logical = None;
    assert_eq!(refused(build_trash_plan(&t2.b.view, &[t2.docs], &g, &input())), "incomplete_subtree");
    t2.b.facts(t2.video).link_count = Some(2);
    assert_eq!(refused(build_trash_plan(&t2.b.view, &[t2.video], &g, &input())), "hard_links");
    assert!(matches!(build_trash_plan(&t.b.view, &[t.docs, t.a_txt], &g, &input()), Err(ManualError::Overlap { .. })));
    assert_eq!(build_trash_plan(&t.b.view, &[], &g, &input()).unwrap_err(), ManualError::Empty);
}

/// MTR-03: the purge plan deletes only objects in the bin, contents first, the bin record last.
#[test]
fn purge_plans_delete_only_recycled_objects_in_order() {
    let mut b = B::new("C:\\$Recycle.Bin\\S-1-5-21-1");
    let r = b.view.root;
    let rdir = b.dir(r, "$RAB12CD");
    let f1 = b.file(rdir, "one.txt", 3);
    let sub = b.dir(rdir, "sub");
    let f2 = b.file(sub, "two.txt", 4);
    let idir = b.file(r, "$IAB12CD", 60);
    let rfile = b.file(r, "$RXY.mov", 900);
    let ifile = b.file(r, "$IXY.mov", 60);
    let items = [
        BinItem { trash_item_id: "tb_1".into(), original_path: Some("D:\\old".into()), deleted_at: None, kind: ObjectKind::Directory, logical_bytes: Some(7), data_entry: rdir, info_entry: Some(idir), refusal: None },
        BinItem { trash_item_id: "tb_2".into(), original_path: Some("D:\\x.mov".into()), deleted_at: None, kind: ObjectKind::File, logical_bytes: Some(900), data_entry: rfile, info_entry: Some(ifile), refusal: None },
    ];
    let plan = build_purge_plan(&b.view, &items.iter().collect::<Vec<_>>(), &input()).unwrap();
    assert_eq!(validate(&plan.body), Ok(()));
    let order: Vec<(u64, &str)> = plan
        .body
        .items
        .iter()
        .map(|a| {
            let PlanAction::FsDelete(d) = a else { panic!("only deletes: {a:?}") };
            assert_eq!(d.target_location, TargetLocation::PlatformTrash);
            (d.target.entry_id.rsplit('/').next().unwrap().parse().unwrap(), d.trash_entry_id.as_deref().unwrap())
        })
        .collect();
    assert_eq!(order, [(f1, "tb_1"), (f2, "tb_1"), (sub, "tb_1"), (rdir, "tb_1"), (idir, "tb_1"), (rfile, "tb_2"), (ifile, "tb_2")]);
    assert_eq!(plan.body.total_logical(), 3 + 4 + 60 + 900 + 60);
    let refused_item = BinItem { refusal: Some("contains a link".into()), ..items[1].clone() };
    assert!(matches!(build_purge_plan(&b.view, &[&refused_item], &input()), Err(ManualError::Refused { .. })));
}

#[test]
fn recycle_bin_paths_are_recognized() {
    let w = |s: &str| RawPath::windows(s.encode_utf16().collect()).unwrap();
    assert!(in_recycle_bin(&w("D:\\$Recycle.Bin\\S-1\\$RX.txt")));
    assert!(!in_recycle_bin(&w("D:\\Recycle\\x.txt")));
    let t = tree();
    let user = UserProtection::empty(Platform::Windows);
    let mut b = B::new("D:\\$Recycle.Bin\\S-1");
    let r = b.view.root;
    let x = b.file(r, "$RX.txt", 1);
    assert_eq!(refused(build_trash_plan(&b.view, &[x], &open_guard(&user), &input())), "in_recycle_bin");
    drop(t);
}
