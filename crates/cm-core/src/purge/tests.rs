use std::collections::BTreeMap;

use super::*;
use crate::analysis::{EntryFacts, MemView};
use crate::plan::{ItemKind, PlanInput, build_plan};

const NOW: i64 = 1_800_000_000;
const OLD: i64 = NOW - 30 * 86_400;

fn wp(s: &str) -> RawPath {
    RawPath::windows(s.encode_utf16().collect()).unwrap()
}

/// A Windows-style tree under `C:\r` built from relative paths; a trailing `/` makes a directory.
struct Tree {
    view: MemView,
    ids: BTreeMap<String, u64>,
}

impl Tree {
    fn new(paths: &[&str], modified: i64) -> Tree {
        let mut t = Tree { view: MemView { platform: Some(crate::capability::Platform::Windows), ..Default::default() }, ids: BTreeMap::new() };
        t.add(0, None, "C:\\r", ObjectKind::Directory, modified);
        for p in paths {
            let dir = p.ends_with('/');
            let parts: Vec<&str> = p.trim_end_matches('/').split('/').collect();
            let mut parent = 0;
            let mut key = String::new();
            for (i, part) in parts.iter().enumerate() {
                key = if key.is_empty() { (*part).to_owned() } else { format!("{key}/{part}") };
                let last = i + 1 == parts.len();
                parent = match t.ids.get(&key) {
                    Some(id) => *id,
                    None => {
                        let id = t.ids.len() as u64 + 1;
                        let kind = if last && !dir { ObjectKind::File } else { ObjectKind::Directory };
                        t.add(id, Some(parent), part, kind, modified);
                        t.ids.insert(key.clone(), id);
                        id
                    }
                };
            }
        }
        t
    }

    fn add(&mut self, id: u64, parent: Option<u64>, name: &str, kind: ObjectKind, modified: i64) {
        let v = &mut self.view;
        v.names.insert(id, wp(name));
        v.parents.insert(id, parent);
        if let Some(p) = parent {
            v.kids.entry(p).or_default().push(id);
        }
        let file_id = format!("fileid128:{id:032x}");
        v.identities.insert(("v".into(), file_id.clone()), id);
        v.facts.insert(id, EntryFacts {
            kind, boundary: false, object_id: id, logical: (kind == ObjectKind::File).then_some(100), allocated: None, volume_id: "v".into(), file_id,
            quality: IdentityQuality::Strong, cloud: CloudState::NotCloud, modified_unix: Some(modified), link_count: Some(1), skipped_here: 0,
        });
    }

    fn touch(&mut self, path: &str, modified: i64) {
        let id = self.ids[path];
        self.view.facts.get_mut(&id).unwrap().modified_unix = Some(modified);
    }
}

#[derive(Default)]
struct Probe {
    tags: Vec<String>,
    vcs: BTreeMap<String, VcsState>,
}

impl PurgeProbe for Probe {
    fn cachedir_tag(&self, path: &RawPath) -> bool {
        self.tags.iter().any(|t| path.lossy().ends_with(t))
    }
    fn vcs(&self, dir: &RawPath) -> VcsState {
        let text = dir.lossy();
        self.vcs.iter().find(|(k, _)| text.ends_with(k.as_str())).map(|(_, v)| v.clone()).unwrap_or(VcsState::NotVersioned)
    }
}

fn run(t: &Tree, probe: &Probe) -> PurgeAnalysis {
    let input = PurgeInput { scan_id: "s", revision: 1, now_unix: NOW, recent_days: DEFAULT_RECENT_DAYS, max_depth: DEFAULT_MAX_DEPTH };
    analyze_purge(&t.view, &input, &AnalysisProtection::default(), probe)
}

fn plannable(c: &Candidate) -> bool {
    c.state == CandidateState::Allowed && c.tier != Tier::Blocked && !c.allowed_modes.is_empty()
}

fn selectable(c: &Candidate) -> bool {
    plannable(c) && c.tier == Tier::DefaultSelected
}

fn by_path<'a>(a: &'a PurgeAnalysis, suffix: &str) -> Option<&'a Candidate> {
    a.set.candidates.iter().find(|c| c.path.lossy().ends_with(suffix))
}

#[test]
fn names_alone_never_make_a_candidate() {
    let t = Tree::new(&["misc/build/out.o", "misc/node_modules/x/index.js", "build/app/Cargo.toml", "build/app/target/debug/a.exe",
        "node_modules/pkg/package.json", "node_modules/pkg/dist/i.js", "proj/package.json", "proj/build/bundle.js"], OLD);
    let a = run(&t, &Probe::default());
    let paths: Vec<String> = a.set.candidates.iter().map(|c| c.path.lossy()).collect();
    assert_eq!(paths, ["C:\\r\\proj\\build"], "only a build dir next to a project marker; artifact dirs are never searched");
    assert_eq!(a.set.candidates[0].tier, Tier::ReviewOnly, "a JavaScript build/ may hold hand-written files");
}

#[test]
fn outputs_and_dependencies_are_separate_with_their_costs() {
    let t = Tree::new(&["app/Cargo.toml", "app/package.json", "app/target/debug/app.exe", "app/node_modules/left-pad/index.js"], OLD);
    let a = run(&t, &Probe::default());
    let target = by_path(&a, "\\target").unwrap();
    let deps = by_path(&a, "\\node_modules").unwrap();
    assert_eq!((target.tier, target.category.as_str()), (Tier::DefaultSelected, "project_build_output"));
    assert_eq!((deps.tier, deps.category.as_str()), (Tier::ReviewOnly, "project_dependencies"));
    assert!(deps.impact.contains("needs the network"), "{}", deps.impact);
    assert!(deps.reasons.iter().any(|r| r == "dependency_reinstall_needs_network"));
    assert_eq!(a.facts.iter().find(|f| f.candidate_id == deps.candidate_id).unwrap().rebuild_class, RebuildClass::NetworkRedownload);
    assert!(!selectable(deps) && selectable(target));
    assert_eq!(target.rules[0].rule_id, "purge.target");
    assert_eq!(a.projects, 1);
}

#[test]
fn recent_projects_are_listed_but_not_preselected() {
    let mut t = Tree::new(&["svc/Cargo.toml", "svc/src/main.rs", "svc/target/x.o"], OLD);
    t.touch("svc/src/main.rs", NOW - 86_400);
    let a = run(&t, &Probe::default());
    let c = by_path(&a, "\\target").unwrap();
    assert_eq!(c.tier, Tier::ReviewOnly);
    assert!(c.reasons.iter().any(|r| r == "recent_project:7d"), "{:?}", c.reasons);
    assert!(a.facts[0].recent);
    assert!(plannable(c), "still plannable by explicit choice");
}

#[test]
fn tracked_files_block_and_unknown_tracking_keeps_unselected() {
    let t = Tree::new(&["web/package.json", "web/dist/app.js", "web2/package.json", "web2/.next/cache/x", "web3/Cargo.toml", "web3/target/y"], OLD);
    let mut probe = Probe::default();
    probe.vcs.insert("web\\dist".into(), VcsState::Tracked(3));
    probe.vcs.insert("web2\\.next".into(), VcsState::Unknown("index unreadable".into()));
    probe.vcs.insert("web3\\target".into(), VcsState::Untracked);
    let a = run(&t, &probe);
    let dist = by_path(&a, "web\\dist").unwrap();
    assert_eq!((dist.state, dist.tier), (CandidateState::Blocked, Tier::Blocked));
    assert!(dist.reasons.iter().any(|r| r == "vcs_tracked:3_files"));
    assert!(dist.allowed_modes.is_empty());
    let next = by_path(&a, ".next").unwrap();
    assert_eq!(next.state, CandidateState::PendingEvidence);
    assert!(!plannable(next) && next.reasons.iter().any(|r| r.starts_with("vcs_unknown:")));
    assert!(selectable(by_path(&a, "web3\\target").unwrap()));
}

#[test]
fn secrets_and_repositories_inside_block_the_artifact() {
    let t = Tree::new(&["a/package.json", "a/dist/.env", "b/package.json", "b/node_modules/pkg/test/server.pem", "c/package.json", "c/node_modules/.npmrc",
        "d/Cargo.toml", "d/target/vendored/.git/HEAD", "e/package.json", "e/dist/.env.example"], OLD);
    let a = run(&t, &Probe::default());
    let reason = |p: &str| by_path(&a, p).unwrap().reasons.clone();
    assert!(reason("a\\dist").iter().any(|r| r.starts_with("secret_file:")));
    assert_eq!(by_path(&a, "a\\dist").unwrap().tier, Tier::Blocked);
    assert_ne!(by_path(&a, "b\\node_modules").unwrap().tier, Tier::Blocked, "package test fixtures deep inside dependencies do not block");
    assert!(reason("c\\node_modules").iter().any(|r| r.starts_with("secret_file:")));
    assert!(reason("d\\target").iter().any(|r| r == "vcs_metadata_inside"));
    assert_ne!(by_path(&a, "e\\dist").unwrap().tier, Tier::Blocked, ".env.example is a template");
    assert!(is_secret_name("id_rsa") && is_secret_name("Prod.PFX") && is_secret_name(".env.local") && !is_secret_name("key") && !is_secret_name("index.js"));
}

#[test]
fn cachedir_tag_is_extra_evidence_for_outputs_only() {
    let t = Tree::new(&["p/pyproject.toml", "p/build/CACHEDIR.TAG", "p/build/lib/x.pyc", "q/package.json", "q/node_modules/CACHEDIR.TAG"], OLD);
    let probe = Probe { tags: vec!["p\\build\\CACHEDIR.TAG".into(), "q\\node_modules\\CACHEDIR.TAG".into()], ..Default::default() };
    let a = run(&t, &probe);
    let build = by_path(&a, "p\\build").unwrap();
    assert_eq!(build.tier, Tier::DefaultSelected);
    assert!(build.reasons.iter().any(|r| r == "cachedir_tag"));
    assert_eq!(by_path(&a, "q\\node_modules").unwrap().tier, Tier::ReviewOnly, "dependencies stay review-only");
}

#[test]
fn project_search_depth_is_bounded() {
    let t = Tree::new(&["a/b/c/d/e/f/Cargo.toml", "a/b/c/d/e/f/target/x", "a/b/Cargo.toml", "a/b/target/y"], OLD);
    let input = PurgeInput { scan_id: "s", revision: 1, now_unix: NOW, recent_days: 7, max_depth: 3 };
    let a = analyze_purge(&t.view, &input, &AnalysisProtection::default(), &Probe::default());
    assert_eq!(a.set.candidates.len(), 1);
    assert!(a.set.candidates[0].path.lossy().ends_with("a\\b\\target"));
    assert_ne!(rule_set_digest(7, 3), rule_set_digest(8, 3), "thresholds are bound into the digest");
    assert!(is_current_digest(&a.set.rule_set_digest));
    assert!(!is_current_digest(&"0".repeat(64)));
}

#[test]
fn purge_plans_remove_emptied_directories() {
    let t = Tree::new(&["w/Cargo.toml", "w/target/debug/a.exe", "w/target/b.d"], OLD);
    let a = run(&t, &Probe::default());
    let id = a.set.candidates[0].candidate_id.clone();
    let input = PlanInput { plan_id: "p", scope_id: "sc", created_unix: NOW, observed_at: "2027-01-15T08:00:00Z", policy_digest: "0" };
    let plan = build_plan(&t.view, &a.set, std::slice::from_ref(&id), Mode::Trash, &input).unwrap();
    let kinds: Vec<(ItemKind, String)> = plan.body.items.iter().map(|i| (i.kind, i.path.lossy())).collect();
    assert_eq!(kinds, [
        (ItemKind::File, "C:\\r\\w\\target\\debug\\a.exe".into()),
        (ItemKind::File, "C:\\r\\w\\target\\b.d".into()),
        (ItemKind::Directory, "C:\\r\\w\\target\\debug".into()),
        (ItemKind::Directory, "C:\\r\\w\\target".into()),
    ]);
    assert_eq!(plan.item_meta[0].candidate_root.as_ref().map(RawPath::lossy).as_deref(), Some("C:\\r\\w\\target"));
    let q = build_plan(&t.view, &a.set, std::slice::from_ref(&id), Mode::Quarantine, &input).unwrap();
    assert!(q.body.items.iter().all(|i| i.kind == ItemKind::File), "quarantine keeps parents for restore");
}

#[test]
fn settings_roundtrip_and_root_edits() {
    let defaults = [wp("C:\\Users\\me\\source\\repos")];
    let mut s = PurgeSettings::default();
    assert!(s.add_root(wp("D:\\code"), &defaults));
    assert!(!s.add_root(wp("d:\\CODE\\"), &defaults), "same root, different spelling");
    assert!(!s.add_root(defaults[0].clone(), &defaults));
    assert!(s.remove_root(&defaults[0], &defaults));
    let roots = s.roots(&defaults);
    assert_eq!(roots.iter().map(|(p, src)| (p.lossy(), *src)).collect::<Vec<_>>(), [("D:\\code".to_string(), RootSource::User)]);
    assert!(s.add_root(defaults[0].clone(), &defaults), "re-adding a removed default re-enables it");
    let back = PurgeSettings::parse(&s.serialize()).unwrap();
    assert_eq!(back, s);
    assert!(matches!(PurgeSettings::parse("{\"schema_version\": 2}"), Err(SettingsError::Version(2))));
    assert!(matches!(PurgeSettings::parse("not json"), Err(SettingsError::Invalid(_))));
    let bad = PurgeSettings { max_depth: 99, ..PurgeSettings::default() };
    assert!(PurgeSettings::parse(&bad.serialize()).is_err());
}
