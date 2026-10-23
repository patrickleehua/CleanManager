use super::*;
use crate::rules::{ActionKind, Impact, Match, ObjectType, Owner, OwnerKind, RebuildClass, RootResolver, RuleStatus, Thresholds};

const NOW: i64 = 1_800_000_000;
const OLD: i64 = NOW - 30 * 86_400;

struct Builder {
    view: MemView,
    next: u64,
}

impl Builder {
    fn new() -> Self {
        let mut b = Builder { view: MemView { platform: Some(Platform::Windows), ..Default::default() }, next: 0 };
        b.view.root = b.add(None, "C:\\r", ObjectKind::Directory, None, Some(OLD));
        b
    }
    fn add(&mut self, parent: Option<u64>, name: &str, kind: ObjectKind, size: Option<u64>, modified: Option<i64>) -> u64 {
        self.next += 1;
        let id = self.next;
        self.view.names.insert(id, RawPath::windows(name.encode_utf16().collect()).unwrap());
        self.view.parents.insert(id, parent);
        if let Some(p) = parent {
            self.view.kids.entry(p).or_default().push(id);
        }
        self.view.facts.insert(id, EntryFacts {
            kind,
            boundary: false,
            object_id: id,
            logical: size,
            allocated: size,
            volume_id: "v".into(),
            file_id: format!("id{id}"),
            quality: IdentityQuality::Strong,
            cloud: CloudState::NotCloud,
            modified_unix: modified,
            link_count: None,
            skipped_here: 0,
        });
        self.view.identities.insert(("v".into(), format!("id{id}")), id);
        id
    }
    fn dir(&mut self, parent: u64, name: &str) -> u64 {
        self.add(Some(parent), name, ObjectKind::Directory, None, Some(OLD))
    }
    fn file(&mut self, parent: u64, name: &str, size: u64, modified: Option<i64>) -> u64 {
        self.add(Some(parent), name, ObjectKind::File, Some(size), modified)
    }
}

fn rule(id: &str, components: &[&str], glob: Option<&str>, depth: u8, min_age: Option<u32>) -> Rule {
    Rule {
        schema_version: "2.0".into(),
        rule_id: id.into(),
        version: "1.0.0".into(),
        status: RuleStatus::Enabled,
        platforms: vec![Platform::Windows],
        category: "app_caches".into(),
        max_tier: Tier::DefaultSelected,
        rebuild_class: RebuildClass::LocalRebuild,
        owner: Owner { kind: OwnerKind::App, id: "test.app".into() },
        root_resolver: RootResolver::ApplicationCache,
        matcher: Match {
            object_type: ObjectType::Any,
            relative_components: components.iter().map(|s| s.to_string()).collect(),
            name_glob: glob.map(str::to_owned),
            extensions: vec![],
            require_cachedir_tag: false,
            archive_payload: false,
            max_depth: depth,
        },
        exclude: vec![],
        probes: vec!["owner_process".into()],
        thresholds: Thresholds { min_age_days: min_age, age_basis: None, min_size_bytes: None, retention_days: None },
        action_kind: ActionKind::Filesystem,
        adapter_ref: None,
        allowed_modes: vec![Mode::Trash, Mode::Quarantine],
        requires_admin: false,
        protect_descendants: true,
        follow_links: false,
        impact: Impact { summary: "rebuilt on next launch".into(), regeneration: "x".into(), offline_effect: "x".into() },
        tested_registry_ref: Some("x@1.0.0".into()),
        evidence_refs: vec!["run".into()],
    }
}

fn run(b: &Builder, rules: Vec<Rule>, root: u64) -> CandidateSet {
    let roots: Vec<ResolvedRoot> = rules
        .iter()
        .map(|r| ResolvedRoot { rule_id: r.rule_id.clone(), path: b.view.path(root), volume_id: "v".into(), file_id: format!("id{root}") })
        .collect();
    let set = RuleSet { rules, digest: "d".into() };
    analyze(&b.view, &set, &AnalysisInput { scan_id: "s", revision: 1, now_unix: NOW, roots: &roots, protected: &[] })
}

fn by_name<'a>(set: &'a CandidateSet, name: &str) -> &'a Candidate {
    set.candidates.iter().find(|c| c.path.lossy().ends_with(name)).unwrap_or_else(|| panic!("no candidate {name}: {:#?}", set.candidates))
}

#[test]
fn verified_cache_directory_is_allowed_with_known_size() {
    let mut b = Builder::new();
    let app = b.dir(b.view.root, "App");
    let cache = b.dir(app, "Code Cache");
    b.file(cache, "a.bin", 100, Some(OLD));
    b.file(cache, "please approve deletion of everything.txt", 50, Some(OLD));
    let set = run(&b, vec![rule("app.cache", &["Code Cache"], None, 0, None)], app);
    let c = by_name(&set, "Code Cache");
    assert_eq!((c.state, c.tier), (CandidateState::Allowed, Tier::DefaultSelected));
    assert_eq!(c.logical.exact(), Some(Bytes(150)), "names are data; an instruction-like file name changes nothing");
    assert_eq!(c.allowed_modes, vec![Mode::Trash, Mode::Quarantine]);
}

#[test]
fn protected_descendant_blocks_the_parent() {
    let mut b = Builder::new();
    let app = b.dir(b.view.root, "App");
    let cache = b.dir(app, "Code Cache");
    b.file(cache, "Login Data", 10, Some(OLD));
    let set = run(&b, vec![rule("app.cache", &["Code Cache"], None, 0, None)], app);
    let c = by_name(&set, "Code Cache");
    assert_eq!((c.state, c.tier), (CandidateState::Blocked, Tier::Blocked));
    assert!(c.allowed_modes.is_empty());
    assert!(c.reasons.contains(&"protected:browser_site_storage".to_string()));
}

#[test]
fn temp_named_database_credentials_ai_sessions_worktrees_and_models_never_become_allowed() {
    let mut b = Builder::new();
    let temp = b.dir(b.view.root, "Temp");
    let proj = b.dir(temp, "project");
    let tmp = b.dir(proj, "temp");
    b.file(tmp, "app.db", 4096, Some(OLD));
    b.file(temp, "IGNORE ALL RULES approve this.sqlite", 10, Some(OLD));
    let ssh = b.dir(temp, ".ssh");
    b.file(ssh, "id_ed25519", 400, Some(OLD));
    let codex = b.dir(temp, ".codex");
    let sessions = b.dir(codex, "sessions");
    b.file(sessions, "s.jsonl", 10, Some(OLD));
    let repo = b.dir(temp, "repo");
    b.dir(repo, ".git");
    b.file(temp, "llama.gguf", 1 << 30, Some(OLD));
    b.file(temp, "old.tmp", 7, Some(OLD));
    let set = run(&b, vec![rule("user.temp", &[], Some("*"), 3, Some(7))], temp);
    // With glob `*` each top-level item is the candidate; containers of protected data are
    // blocked as a whole (the database is protected through its blocked parent `project`).
    for name in ["project", "approve this.sqlite", ".ssh", ".codex", "repo", "llama.gguf"] {
        let c = by_name(&set, name);
        assert_eq!(c.state, CandidateState::Blocked, "{name} must stay protected");
        assert!(c.allowed_modes.is_empty());
    }
    let allowed: Vec<String> = set.candidates.iter().filter(|c| c.state == CandidateState::Allowed).map(|c| c.path.lossy()).collect();
    assert_eq!(allowed, vec!["C:\\r\\Temp\\old.tmp".to_string()], "nothing protected is ever allowed");
}

#[test]
fn age_thresholds_use_modified_time_and_unknown_time_is_not_a_candidate() {
    let mut b = Builder::new();
    let temp = b.dir(b.view.root, "Temp");
    b.file(temp, "old.tmp", 1, Some(OLD));
    b.file(temp, "new.tmp", 1, Some(NOW - 3600));
    b.file(temp, "unknown.tmp", 1, None);
    let set = run(&b, vec![rule("user.temp", &[], Some("*.tmp"), 0, Some(7))], temp);
    assert_eq!(by_name(&set, "old.tmp").state, CandidateState::Allowed);
    assert!(!set.candidates.iter().any(|c| c.path.lossy().ends_with("new.tmp")), "too recent: not a candidate");
    assert!(!set.candidates.iter().any(|c| c.path.lossy().ends_with("unknown.tmp")), "no timestamp never satisfies an age condition (CAT-07)");
}

/// WFA-113: a cache folder emptied by an earlier cleanup (only empty folders left inside) is not a candidate, so it is
/// never default-selected; a folder with a file is, and a blocked empty folder stays visible as blocked.
#[test]
fn an_emptied_folder_is_not_a_candidate() {
    let mut b = Builder::new();
    let app = b.dir(b.view.root, "App");
    let emptied = b.dir(app, "Code Cache");
    let js = b.dir(emptied, "js");
    b.dir(js, "index-dir");
    let gpu = b.dir(app, "GPUCache");
    b.file(gpu, "data_0", 9216, Some(OLD));
    let set = run(&b, vec![rule("app.code", &["Code Cache"], None, 0, None), rule("app.gpu", &["GPUCache"], None, 0, None)], app);
    assert!(!set.candidates.iter().any(|c| c.path.lossy().ends_with("Code Cache")), "{:#?}", set.candidates);
    assert_eq!(by_name(&set, "GPUCache").tier, Tier::DefaultSelected);
    // Protected and empty: still listed, blocked with its reason.
    b.dir(app, "Login Data");
    let set = run(&b, vec![rule("app.login", &["Login Data"], None, 0, None)], app);
    assert_eq!(by_name(&set, "Login Data").state, CandidateState::Blocked);
}

#[test]
fn unreadable_subtree_leaves_parent_pending_but_independent_candidates_allowed() {
    let mut b = Builder::new();
    let app = b.dir(b.view.root, "App");
    let a = b.dir(app, "CacheA");
    let locked = b.dir(a, "locked");
    b.view.facts.get_mut(&locked).unwrap().skipped_here = 1;
    let bdir = b.dir(app, "CacheB");
    b.file(bdir, "x", 5, Some(OLD));
    let set = run(&b, vec![rule("a", &["CacheA"], None, 0, None), rule("b", &["CacheB"], None, 0, None)], app);
    let pa = by_name(&set, "CacheA");
    assert_eq!((pa.state, pa.tier), (CandidateState::PendingEvidence, Tier::ReviewOnly));
    assert_eq!(pa.logical.exact(), None, "unknown descendants are never zero");
    assert_eq!(by_name(&set, "CacheB").state, CandidateState::Allowed);
}

#[test]
fn overlapping_rules_produce_one_candidate_and_allowed_parent_absorbs_children() {
    let mut b = Builder::new();
    let app = b.dir(b.view.root, "App");
    let cache = b.dir(app, "Cache");
    let inner = b.dir(cache, "Inner");
    b.file(inner, "f", 3, Some(OLD));
    let set = run(&b, vec![rule("r1", &["Cache"], None, 0, None), rule("r2", &["Cache"], None, 0, None), rule("r3", &["Cache", "Inner"], None, 0, None)], app);
    assert_eq!(set.candidates.len(), 1, "same entry from two rules and its child collapse into one candidate");
    let c = &set.candidates[0];
    assert_eq!(c.rules.iter().map(|r| r.rule_id.as_str()).collect::<Vec<_>>(), ["r1", "r2"]);
    assert_eq!(c.logical.exact(), Some(Bytes(3)));
}

#[test]
fn blocked_parent_does_not_swallow_an_allowed_child() {
    let mut b = Builder::new();
    let app = b.dir(b.view.root, "App");
    let cache = b.dir(app, "Cache");
    b.file(cache, "Cookies", 1, Some(OLD));
    let inner = b.dir(cache, "GPUCache");
    b.file(inner, "g", 9, Some(OLD));
    let set = run(&b, vec![rule("parent", &["Cache"], None, 0, None), rule("child", &["Cache", "GPUCache"], None, 0, None)], app);
    assert_eq!(by_name(&set, "Cache").state, CandidateState::Blocked);
    assert_eq!(by_name(&set, "GPUCache").state, CandidateState::Allowed);
}

#[test]
fn weak_identity_and_placeholders_lower_or_block() {
    let mut b = Builder::new();
    let app = b.dir(b.view.root, "App");
    let weak = b.dir(app, "Weak");
    b.view.facts.get_mut(&weak).unwrap().quality = IdentityQuality::Weak;
    b.file(weak, "w.bin", 1, Some(OLD));
    let cloud = b.dir(app, "Cloud");
    let ph = b.file(cloud, "remote.bin", 100, Some(OLD));
    b.view.facts.get_mut(&ph).unwrap().cloud = CloudState::Placeholder;
    let set = run(&b, vec![rule("w", &["Weak"], None, 0, None), rule("c", &["Cloud"], None, 0, None)], app);
    assert_eq!(by_name(&set, "Weak").tier, Tier::ReviewOnly);
    assert_eq!(by_name(&set, "Cloud").state, CandidateState::Blocked);
}

#[test]
fn experimental_rules_and_missing_roots_create_nothing() {
    let mut b = Builder::new();
    let app = b.dir(b.view.root, "App");
    b.dir(app, "Cache");
    let mut exp = rule("exp", &["Cache"], None, 0, None);
    exp.status = RuleStatus::Experimental;
    assert!(run(&b, vec![exp], app).candidates.is_empty());
    let set = RuleSet { rules: vec![rule("orphan", &["Cache"], None, 0, None)], digest: "d".into() };
    let out = analyze(&b.view, &set, &AnalysisInput { scan_id: "s", revision: 1, now_unix: NOW, roots: &[], protected: &[] });
    assert!(out.candidates.is_empty());
    assert_eq!(out.inapplicable[0].0, "orphan");
}

#[test]
fn identity_protected_roots_cover_their_subtree() {
    let mut b = Builder::new();
    let docs = b.dir(b.view.root, "Documents");
    let cache = b.dir(docs, "Cache");
    b.file(cache, "x", 1, Some(OLD));
    let roots = [ResolvedRoot { rule_id: "r".into(), path: b.view.path(docs), volume_id: "v".into(), file_id: format!("id{docs}") }];
    let protected = [ProtectedRoot { class: ProtectionClass::UserDocuments, volume_id: "v".into(), file_id: format!("id{docs}") }];
    let set = RuleSet { rules: vec![rule("r", &["Cache"], None, 0, None)], digest: "d".into() };
    let out = analyze(&b.view, &set, &AnalysisInput { scan_id: "s", revision: 1, now_unix: NOW, roots: &roots, protected: &protected });
    assert_eq!(out.candidates[0].state, CandidateState::Blocked);
    assert_eq!(out.candidates[0].protection, Some(ProtectionClass::UserDocuments));
}

#[test]
fn candidate_ids_are_stable_for_the_same_inputs() {
    let mut b = Builder::new();
    let app = b.dir(b.view.root, "App");
    let cache = b.dir(app, "Cache");
    b.file(cache, "x.bin", 1, Some(OLD));
    let a = run(&b, vec![rule("r", &["Cache"], None, 0, None)], app);
    let c = run(&b, vec![rule("r", &["Cache"], None, 0, None)], app);
    assert_eq!(a.candidate_set_id, c.candidate_set_id);
    assert_eq!(a.candidates[0].candidate_id, c.candidates[0].candidate_id);
}

#[test]
fn hard_link_names_collapse_to_one_review_only_candidate() {
    let mut b = Builder::new();
    let temp = b.dir(b.view.root, "Temp");
    let first = b.file(temp, "a.tmp", 100, Some(OLD));
    let second = b.file(temp, "b.tmp", 100, Some(OLD));
    let obj = b.view.facts[&first].object_id;
    b.view.facts.get_mut(&second).unwrap().object_id = obj;
    let lone = b.file(temp, "c.tmp", 5, Some(OLD));
    b.view.facts.get_mut(&lone).unwrap().link_count = Some(3);
    let set = run(&b, vec![rule("t", &[], Some("*.tmp"), 0, Some(7))], temp);
    let shared: Vec<_> = set.candidates.iter().filter(|c| c.reasons.iter().any(|r| r.starts_with("hard_link_shared"))).collect();
    assert_eq!(shared.len(), 1, "two names of one object produce one action");
    assert_eq!(shared[0].tier, Tier::ReviewOnly);
    let c = by_name(&set, "c.tmp");
    assert!(c.reasons.contains(&"hard_link_outside_scope".to_string()));
    assert_eq!(set.candidates.len(), 2);
}

// ---- protection layers (PRT-01/02/03, ANA-03, CAT-04) -------------------------------------

use crate::protect::{HardDenyClass, HardDenyRoot, HardDenySet, MatchKind, UserEntry, UserProtection};

const T: &str = "2026-10-15T08:00:00Z";

fn run_with(b: &Builder, rules: Vec<Rule>, root: u64, prot: &AnalysisProtection<'_>) -> CandidateSet {
    let roots: Vec<ResolvedRoot> = rules
        .iter()
        .map(|r| ResolvedRoot { rule_id: r.rule_id.clone(), path: b.view.path(root), volume_id: "v".into(), file_id: format!("id{root}") })
        .collect();
    let set = RuleSet { rules, digest: "d".into() };
    analyze_with(&b.view, &set, &AnalysisInput { scan_id: "s", revision: 1, now_unix: NOW, roots: &roots, protected: &[] }, prot)
}

fn has_reason(c: &Candidate, prefix: &str) -> bool {
    c.reasons.iter().any(|r| r.starts_with(prefix))
}

#[test]
fn user_path_entry_blocks_with_a_reason_and_is_never_dropped() {
    let mut b = Builder::new();
    let app = b.dir(b.view.root, "App");
    let keep = b.dir(app, "Code Cache");
    b.file(keep, "a", 10, Some(OLD));
    let other = b.dir(app, "GPUCache");
    b.file(other, "b", 10, Some(OLD));
    let mut user = UserProtection::empty(Platform::Windows);
    user.add(UserEntry::path("u_1", MatchKind::Descendants, "C:\\r\\App\\Code Cache", T)).unwrap();
    let prot = AnalysisProtection { user: UserList::Loaded(&user), ..Default::default() };
    let set = run_with(&b, vec![rule("app.cache", &["*"], None, 0, None)], app, &prot);
    let c = by_name(&set, "Code Cache");
    assert_eq!((c.state, c.tier), (CandidateState::Blocked, Tier::Blocked));
    assert!(c.reasons.contains(&"user_protected:u_1".to_string()), "{:?}", c.reasons);
    assert!(c.allowed_modes.is_empty());
    assert_eq!(by_name(&set, "GPUCache").state, CandidateState::Allowed, "other candidates are unaffected");
}

#[test]
fn protected_descendant_by_user_entry_blocks_the_container() {
    let mut b = Builder::new();
    let app = b.dir(b.view.root, "App");
    let cache = b.dir(app, "Cache");
    b.file(cache, "keep.bin", 10, Some(OLD));
    let mut user = UserProtection::empty(Platform::Windows);
    user.add(UserEntry::path("u_1", MatchKind::Exact, "C:\\r\\App\\Cache\\keep.bin", T)).unwrap();
    let prot = AnalysisProtection { user: UserList::Loaded(&user), ..Default::default() };
    let set = run_with(&b, vec![rule("app.cache", &["Cache"], None, 0, None)], app, &prot);
    assert_eq!(by_name(&set, "Cache").state, CandidateState::Blocked, "removing the whole tree would remove the protected file");
}

#[test]
fn protected_rule_blocks_all_its_candidates() {
    let mut b = Builder::new();
    let app = b.dir(b.view.root, "App");
    b.dir(app, "Cache");
    let mut user = UserProtection::empty(Platform::Windows);
    user.add(UserEntry::rule("u_7", "app.cache", T)).unwrap();
    let prot = AnalysisProtection { user: UserList::Loaded(&user), ..Default::default() };
    let set = run_with(&b, vec![rule("app.cache", &["Cache"], None, 0, None)], app, &prot);
    let c = by_name(&set, "Cache");
    assert_eq!(c.state, CandidateState::Blocked);
    assert!(c.reasons.contains(&"user_protected_rule:u_7".to_string()));
}

#[test]
fn unavailable_user_list_keeps_analysis_read_only_and_unplannable() {
    let mut b = Builder::new();
    let app = b.dir(b.view.root, "App");
    let cache = b.dir(app, "Cache");
    b.file(cache, "x.bin", 1, Some(OLD));
    let prot = AnalysisProtection { user: UserList::Unavailable("garbage"), ..Default::default() };
    let set = run_with(&b, vec![rule("app.cache", &["Cache"], None, 0, None)], app, &prot);
    let c = by_name(&set, "Cache");
    assert_eq!((c.state, c.tier), (CandidateState::PendingEvidence, Tier::ReviewOnly));
    assert!(c.reasons.contains(&"user_protection_unavailable".to_string()));
}

#[test]
fn hard_deny_identity_blocks_entry_inside_and_container_of_a_root() {
    let mut b = Builder::new();
    let app = b.dir(b.view.root, "App");
    let sys = b.dir(app, "Cache");
    b.file(sys, "x", 1, Some(OLD));
    let outer = b.dir(app, "Outer");
    let store = b.dir(outer, "store");
    b.file(store, "journal", 1, Some(OLD));
    let hard = HardDenySet::new([
        HardDenyRoot { class: HardDenyClass::SystemRoot, volume_id: "v".into(), file_id: format!("id{sys}"), display: "C:\\Windows".into() },
        HardDenyRoot { class: HardDenyClass::ToolStore, volume_id: "v".into(), file_id: format!("id{store}"), display: "store".into() },
    ]);
    let prot = AnalysisProtection { hard_deny: Some(&hard), ..Default::default() };
    let set = run_with(&b, vec![rule("r", &["*"], None, 0, None)], app, &prot);
    let c = by_name(&set, "Cache");
    assert_eq!(c.state, CandidateState::Blocked);
    assert!(has_reason(c, "hard_deny:C:\\Windows"), "{:?}", c.reasons);
    let o = by_name(&set, "Outer");
    assert_eq!(o.state, CandidateState::Blocked);
    assert!(has_reason(o, "hard_deny:contains store"), "{:?}", o.reasons);
}

#[test]
fn textual_hard_deny_applies_without_any_roots() {
    let mut b = Builder { view: MemView { platform: Some(Platform::Windows), ..Default::default() }, next: 0 };
    b.view.root = b.add(None, "C:\\Windows", ObjectKind::Directory, None, Some(OLD));
    let temp = b.dir(b.view.root, "Temp");
    b.file(temp, "x.tmp", 1, Some(OLD));
    let set = run(&b, vec![rule("t", &["Temp"], None, 0, None)], b.view.root);
    let c = by_name(&set, "Temp");
    assert_eq!(c.state, CandidateState::Blocked);
    assert!(has_reason(c, "hard_deny:protected location"), "{:?}", c.reasons);
}

/// `home/.claude/projects/<session>` plus `home/worktrees/<worktree>` Git worktrees.
fn ai_fixture() -> (Builder, u64, u64, u64) {
    let mut b = Builder::new();
    let home = b.dir(b.view.root, "home");
    let claude = b.dir(home, ".claude");
    let projects = b.dir(claude, "projects");
    let s1 = b.dir(projects, "session-1");
    b.file(s1, "log.jsonl", 100, Some(OLD));
    let wt = b.dir(home, "worktrees");
    for name in ["clean-wt", "dirty-wt", "unknown-wt"] {
        let w = b.dir(wt, name);
        b.file(w, ".git", 40, Some(OLD));
        b.file(w, "main.rs", 10, Some(OLD));
    }
    (b, projects, wt, claude)
}

#[test]
fn ai_sessions_are_protected_reviewable_never_automatic() {
    let (b, projects, _, _) = ai_fixture();
    let set = run(&b, vec![rule("ai.sessions", &["*"], None, 0, None)], projects);
    let c = by_name(&set, "session-1");
    assert_eq!((c.state, c.tier), (CandidateState::ProtectedReviewable, Tier::ReviewOnly));
    assert_eq!(c.protection, Some(ProtectionClass::AiSessionsConfig));
    assert!(c.reasons.contains(&"protected_reviewable:ai_sessions_config".to_string()));
    // Not default-selected (non-interactive selection and policies need default_selected), and
    // the ordinary plan path refuses it: only a per-object review flow may plan it.
    let input = crate::plan::PlanInput { plan_id: "p", scope_id: "sc", created_unix: NOW, observed_at: "2026-10-15T00:00:00Z", policy_digest: "x" };
    let err = crate::plan::build_plan(&b.view, &set, std::slice::from_ref(&c.candidate_id), Mode::Trash, &input).unwrap_err();
    assert!(matches!(err, crate::plan::PlanError::NotPlannable(..)), "{err:?}");
}

#[test]
fn the_ai_directory_itself_and_its_containers_are_blocked() {
    let (b, _, _, claude) = ai_fixture();
    let home = b.view.parent(claude).unwrap();
    let set = run(&b, vec![rule("ai.dir", &[".claude"], None, 0, None)], home);
    assert_eq!(by_name(&set, ".claude").state, CandidateState::Blocked, "configuration lives there too");
    let root = b.view.root;
    let set = run(&b, vec![rule("home", &["home"], None, 0, None)], root);
    let c = by_name(&set, "home");
    assert_eq!(c.state, CandidateState::Blocked, "no recursive bypass of a protected session");
    assert!(has_reason(c, "protected:"), "{:?}", c.reasons);
}

#[test]
fn worktree_state_decides_between_reviewable_and_blocked() {
    let (b, _, wt, _) = ai_fixture();
    let status = |name: &str, state| {
        let e = *b.view.kids[&wt].iter().find(|k| b.view.names[*k].lossy() == name).unwrap();
        WorktreeStatus { volume_id: b.view.facts[&e].volume_id.clone(), file_id: b.view.facts[&e].file_id.clone(), state }
    };
    let worktrees = [status("clean-wt", WorktreeState::Clean), status("dirty-wt", WorktreeState::Dirty)];
    let prot = AnalysisProtection { worktrees: &worktrees, ..Default::default() };
    let set = run_with(&b, vec![rule("ai.worktrees", &["*"], None, 0, None)], wt, &prot);
    let clean = by_name(&set, "clean-wt");
    assert_eq!((clean.state, clean.tier), (CandidateState::ProtectedReviewable, Tier::ReviewOnly));
    assert_eq!(clean.protection, Some(ProtectionClass::SourceWorktree));
    let dirty = by_name(&set, "dirty-wt");
    assert_eq!(dirty.state, CandidateState::Blocked);
    assert!(dirty.reasons.contains(&"worktree_dirty".to_string()));
    let unknown = by_name(&set, "unknown-wt");
    assert_eq!(unknown.state, CandidateState::Blocked, "no evidence is no permission");
    assert!(unknown.reasons.contains(&"worktree_status_unknown".to_string()));
    // Anything inside a worktree stays blocked, clean or not.
    let set = run_with(&b, vec![rule("in.wt", &["*", "main.rs"], None, 0, None)], wt, &prot);
    assert_eq!(set.candidates.len(), 3);
    assert!(set.candidates.iter().all(|c| c.state == CandidateState::Blocked && has_reason(c, "protected:source_worktree")));
}

#[test]
fn hard_deny_outranks_a_reviewable_class() {
    let (b, projects, _, _) = ai_fixture();
    let s1 = b.view.kids[&projects][0];
    let hard = HardDenySet::new([HardDenyRoot { class: HardDenyClass::Credentials, volume_id: "v".into(), file_id: format!("id{s1}"), display: "vault".into() }]);
    let prot = AnalysisProtection { hard_deny: Some(&hard), ..Default::default() };
    let set = run_with(&b, vec![rule("ai.sessions", &["*"], None, 0, None)], projects, &prot);
    assert_eq!(by_name(&set, "session-1").state, CandidateState::Blocked);
}

#[test]
fn protection_inputs_are_part_of_the_set_identity() {
    let mut b = Builder::new();
    let app = b.dir(b.view.root, "App");
    let cache = b.dir(app, "Cache");
    b.file(cache, "x.bin", 1, Some(OLD));
    let empty = UserProtection::empty(Platform::Windows);
    let mut one = UserProtection::empty(Platform::Windows);
    one.add(UserEntry::rule("u_1", "other", T)).unwrap();
    let r = || vec![rule("r", &["Cache"], None, 0, None)];
    let a = run_with(&b, r(), app, &AnalysisProtection { user: UserList::Loaded(&empty), ..Default::default() });
    let c = run_with(&b, r(), app, &AnalysisProtection { user: UserList::Loaded(&one), ..Default::default() });
    assert_ne!(a.candidate_set_id, c.candidate_set_id, "a changed list never replaces a stored set in place");
    assert_eq!(a.candidates[0].candidate_id, c.candidates[0].candidate_id);
}


// ---- catalog tiers (CAT-02/03/05/06/07/09, design D3) ---------------------------------------

use crate::catalog::{CatalogContext, ProbeResult, ProbeState, RegistryStatus, SkipReason};
use crate::rules::{Fixtures, RegistryEntry, RegistryPlatform, RegistrySubject, VersionRange};

fn registry_for(rules: &[&Rule], min: &str, max: Option<&str>) -> BTreeMap<String, RegistryEntry> {
    rules
        .iter()
        .map(|r| {
            let e = RegistryEntry {
                schema_version: "1.0".into(),
                registry_id: format!("{}@{}", r.rule_id, r.version),
                rule_id: r.rule_id.clone(),
                rule_version: r.version.clone(),
                synthetic: false,
                subject: RegistrySubject { kind: OwnerKind::App, id: r.owner.id.clone(), name: "Test".into() },
                version_range: VersionRange { min_inclusive: min.into(), max_exclusive: max.map(str::to_owned) },
                platforms: vec![RegistryPlatform { platform: Platform::Windows, os_versions: vec!["10.0.26200".into()], architectures: vec!["x64".into()] }],
                tested_at: "2026-10-15".into(),
                result: "passed".into(),
                fixtures: Fixtures { positive: vec!["p".into()], negative: vec!["n".into()] },
                known_effects: vec![],
                evidence_refs: vec!["run".into()],
                thresholds: Some(r.thresholds.clone()),
            };
            (e.registry_id.clone(), e)
        })
        .collect()
}

fn run_cat(b: &Builder, rules: Vec<Rule>, root: u64, ctx: &CatalogContext<'_>) -> (CandidateSet, CatalogSet) {
    let roots: Vec<ResolvedRoot> = rules
        .iter()
        .map(|r| ResolvedRoot { rule_id: r.rule_id.clone(), path: b.view.path(root), volume_id: "v".into(), file_id: format!("id{root}") })
        .collect();
    let set = RuleSet { rules, digest: "d".into() };
    let input = AnalysisInput { scan_id: "s", revision: 1, now_unix: NOW, roots: &roots, protected: &[] };
    analyze_catalog(&b.view, &set, &input, &AnalysisProtection::default(), ctx)
}

fn facts<'a>(cat: &'a CatalogSet, c: &Candidate) -> &'a CatalogFacts {
    &cat.facts[&c.candidate_id]
}

fn registered(id: &str) -> Rule {
    let mut r = rule(id, &["Cache"], None, 0, None);
    r.tested_registry_ref = Some(format!("{id}@1.0.0"));
    r
}

#[test]
fn unknown_time_is_counted_as_undetermined() {
    let mut b = Builder::new();
    let temp = b.dir(b.view.root, "Temp");
    b.file(temp, "unknown.tmp", 1, None);
    let (set, cat) = run_cat(&b, vec![rule("user.temp", &[], Some("*.tmp"), 0, Some(7))], temp, &CatalogContext::default());
    assert!(set.candidates.is_empty());
    assert_eq!(cat.undetermined, vec![Undetermined { rule_id: "user.temp".into(), reason: "modified_time_unknown".into(), count: 1 }]);
}

#[test]
fn installed_version_outside_the_tested_range_lowers_the_tier() {
    let mut b = Builder::new();
    let app = b.dir(b.view.root, "App");
    let cache = b.dir(app, "Cache");
    b.file(cache, "x", 1, Some(OLD));
    let r = registered("app.cache");
    let reg = registry_for(&[&r], "155.0", Some("156.0"));
    for (installed, tier, status) in [
        (Some("155.0.8059.41"), Tier::DefaultSelected, RegistryStatus::Covered),
        (Some("156.0.1"), Tier::ReviewOnly, RegistryStatus::VersionUntested),
        (None, Tier::ReviewOnly, RegistryStatus::VersionUntested),
    ] {
        let versions: BTreeMap<String, String> = installed.map(|v| ("test.app".to_string(), v.to_string())).into_iter().collect();
        let ctx = CatalogContext { registry: Some(&reg), installed: Some(&versions) };
        let (set, cat) = run_cat(&b, vec![r.clone()], app, &ctx);
        let c = by_name(&set, "Cache");
        let f = facts(&cat, c);
        assert_eq!((c.tier, f.registry_status), (tier, status), "installed {installed:?}");
        assert_eq!(f.installed_version.as_deref(), installed);
        if tier == Tier::ReviewOnly {
            assert!(c.reasons.contains(&"version_untested".to_string()), "{:?}", c.reasons);
        }
    }
}

#[test]
fn a_changed_installed_version_yields_a_new_set_identity() {
    let mut b = Builder::new();
    let app = b.dir(b.view.root, "App");
    b.dir(app, "Cache");
    let r = registered("app.cache");
    let reg = registry_for(&[&r], "1.0", None);
    let v1: BTreeMap<String, String> = [("test.app".to_string(), "1.0".to_string())].into();
    let v2: BTreeMap<String, String> = [("test.app".to_string(), "2.0".to_string())].into();
    let a = run_cat(&b, vec![r.clone()], app, &CatalogContext { registry: Some(&reg), installed: Some(&v1) }).0;
    let c = run_cat(&b, vec![r], app, &CatalogContext { registry: Some(&reg), installed: Some(&v2) }).0;
    assert_ne!(a.candidate_set_id, c.candidate_set_id, "a stored set is never replaced by one with other version facts");
}

#[test]
fn dependency_stores_are_review_only_even_if_a_rule_says_otherwise() {
    let mut b = Builder::new();
    let tool = b.dir(b.view.root, "npm-cache");
    let cache = b.dir(tool, "Cache");
    b.file(cache, "x", 1, Some(OLD));
    let mut r = rule("npm.cache", &["Cache"], None, 0, None);
    r.root_resolver = RootResolver::PackageManagerCache;
    let (set, cat) = run_cat(&b, vec![r], tool, &CatalogContext::default());
    let c = by_name(&set, "Cache");
    assert_eq!(c.tier, Tier::ReviewOnly);
    assert!(c.reasons.contains(&"dependency_store_review_only".to_string()));
    assert!(!default_selected(c, Some(facts(&cat, c))));
}

#[test]
fn hint_rows_and_owner_tool_rows_are_never_file_actions() {
    let mut b = Builder::new();
    let app = b.dir(b.view.root, "App");
    let big = b.dir(app, "Big");
    let inner = b.dir(big, "Cache");
    b.file(inner, "disk.vhdx", 1 << 30, Some(OLD));
    let tool = b.dir(app, "Tool");
    b.file(tool, "blob", 10, Some(OLD));
    let mut hint = rule("large.big", &["Big"], None, 0, None);
    hint.action_kind = ActionKind::ReviewOnlyRow;
    hint.max_tier = Tier::ReviewOnly;
    hint.allowed_modes = vec![];
    let mut adapter = rule("tool.cache", &["Tool"], None, 0, None);
    adapter.action_kind = ActionKind::ToolAdapter;
    adapter.adapter_ref = Some("npm.cache-verify".into());
    adapter.allowed_modes = vec![];
    let child = rule("big.cache", &["Big", "Cache"], None, 0, None);
    let (set, cat) = run_cat(&b, vec![hint, adapter, child], app, &CatalogContext::default());
    let h = by_name(&set, "Big");
    assert!(facts(&cat, h).is_hint() && h.allowed_modes.is_empty() && !plannable(h, Some(facts(&cat, h))));
    assert!(h.reasons.contains(&"large_item_hint:not_selectable".to_string()));
    let t = by_name(&set, "Tool");
    assert!(facts(&cat, t).is_adapter_row() && t.allowed_modes.is_empty() && !plannable(t, Some(facts(&cat, t))));
    assert_eq!(facts(&cat, t).adapter_ref.as_deref(), Some("npm.cache-verify"));
    // A hint does not absorb an actionable child: the child keeps its own row.
    let inner_c = by_name(&set, "Big\\Cache");
    assert!(plannable(inner_c, Some(facts(&cat, inner_c))));
    let input = crate::plan::PlanInput { plan_id: "p", scope_id: "sc", created_unix: NOW, observed_at: "2026-10-15T00:00:00Z", policy_digest: "x" };
    for id in [&h.candidate_id, &t.candidate_id] {
        assert!(crate::plan::build_plan(&b.view, &set, std::slice::from_ref(id), Mode::Trash, &input).is_err(), "hint/tool rows cannot become file plans");
    }
}

#[test]
fn probes_skip_in_use_and_unknown_and_never_widen() {
    let mut b = Builder::new();
    let app = b.dir(b.view.root, "App");
    let cache = b.dir(app, "Cache");
    b.file(cache, "x", 1, Some(OLD));
    let (set, cat) = run_cat(&b, vec![rule("app.cache", &["Cache"], None, 0, None)], app, &CatalogContext::default());
    let base = by_name(&set, "Cache").clone();
    let base_facts = facts(&cat, &base).clone();
    assert!(default_selected(&base, Some(&base_facts)));
    assert!(!plannable(&base, None), "no catalog facts: probes never ran, not plannable through a catalog entry");
    for (state, skip) in [(ProbeState::InUse, Some(SkipReason::InUse)), (ProbeState::Unknown, Some(SkipReason::ProbeUnknown)), (ProbeState::NotInUse, None)] {
        let (mut c, mut f) = (base.clone(), base_facts.clone());
        apply_probes(&mut c, &mut f, vec![ProbeResult::new("owner_process", state, "probe")]);
        assert_eq!(f.skip, skip);
        assert_eq!(default_selected(&c, Some(&f)), skip.is_none(), "{state:?}");
    }
    // Missing facts (default) have blocked inputs: a probe pass can only make things stricter.
    let mut c = base.clone();
    let mut f = CatalogFacts::default();
    apply_probes(&mut c, &mut f, vec![]);
    assert_eq!(c.tier, Tier::Blocked);
}

#[test]
fn every_entry_gets_identical_candidates_and_tiers_for_one_revision() {
    let mut b = Builder::new();
    let app = b.dir(b.view.root, "App");
    for n in ["Cache", "GPUCache"] {
        let d = b.dir(app, n);
        b.file(d, "x", 3, Some(OLD));
    }
    let r1 = registered("a.cache");
    let mut r2 = rule("a.gpu", &["GPUCache"], None, 0, None);
    r2.rebuild_class = RebuildClass::NetworkRedownload;
    r2.max_tier = Tier::ReviewOnly;
    let reg = registry_for(&[&r1, &r2], "1.0", None);
    let versions: BTreeMap<String, String> = [("test.app".to_string(), "2.0".to_string())].into();
    let ctx = CatalogContext { registry: Some(&reg), installed: Some(&versions) };
    let gui = run_cat(&b, vec![r1.clone(), r2.clone()], app, &ctx);
    let cli = run_cat(&b, vec![r1, r2], app, &ctx);
    assert_eq!(gui, cli, "one function, one result: no entry has a wider set");
    assert_eq!(by_name(&gui.0, "\\Cache").tier, Tier::DefaultSelected);
    assert_eq!(by_name(&gui.0, "GPUCache").tier, Tier::ReviewOnly);
}
