//! Candidate analysis (ANA-02..ANA-06, IDX-04): turns a published scan revision plus a rule set
//! into reviewable candidates. Analysis never writes targets and never grants authorization;
//! names are data (no file name can change a decision except through literal glob matching).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Serialize;

use crate::bytes::{Bytes, KnownTotal};
use crate::c14n::digest;
use crate::capability::Platform;
use crate::catalog::{CatalogContext, ProbeResult, RegistryStatus, SkipReason, TierInputs, compute_tier, is_dependency_store};
use crate::model::{CloudState, IdentityQuality, ObjectKind};
use crate::path::RawPath;
use crate::protect::{HARD_DENY_VERSION, HardDenySet, ObjectIdentity, UserProtection, hard_deny_within, prefilter};
use crate::rules::{ActionKind, Mode, ObjectType, RebuildClass, Rule, RuleSet, Tier, glob_match};

/// Version of the built-in protection classes; bound into candidate sets and plans.
/// `protection/2`: AI sessions and clean worktree roots are `protected_reviewable` (CAT-04).
pub const PROTECTION_VERSION: &str = "protection/2";

/// What the analyzer needs from an index revision.
pub trait IndexView {
    fn platform(&self) -> Platform;
    fn root(&self) -> u64;
    fn name(&self, entry: u64) -> &RawPath;
    fn parent(&self, entry: u64) -> Option<u64>;
    fn children(&self, entry: u64) -> &[u64];
    fn info(&self, entry: u64) -> &EntryFacts;
    fn path(&self, entry: u64) -> RawPath;
    /// Entry whose object has this protocol file identity (`volume_id`, `file_id`).
    fn find_identity(&self, volume_id: &str, file_id: &str) -> Option<u64>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryFacts {
    pub kind: ObjectKind,
    pub boundary: bool,
    pub object_id: u64,
    pub logical: Option<u64>,
    pub allocated: Option<u64>,
    /// Protocol identity of the entry's object.
    pub volume_id: String,
    pub file_id: String,
    pub quality: IdentityQuality,
    pub cloud: CloudState,
    pub modified_unix: Option<i64>,
    /// Hard-link count when the platform measured it.
    pub link_count: Option<u32>,
    /// Unread ranges directly under this entry.
    pub skipped_here: u64,
}

/// A rule root resolved by the platform layer, identified by file identity rather than text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRoot {
    pub rule_id: String,
    pub path: RawPath,
    pub volume_id: String,
    pub file_id: String,
}

/// Protected locations the platform resolved by identity (documents, system repair stores...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectedRoot {
    pub class: ProtectionClass,
    pub volume_id: String,
    pub file_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectionClass {
    Credentials,
    Database,
    BrowserSiteStorage,
    AiSessionsConfig,
    SourceWorktree,
    Models,
    UserDocuments,
    SystemRepair,
}

impl ProtectionClass {
    pub const ALL: [ProtectionClass; 8] = [
        ProtectionClass::Credentials,
        ProtectionClass::Database,
        ProtectionClass::BrowserSiteStorage,
        ProtectionClass::AiSessionsConfig,
        ProtectionClass::SourceWorktree,
        ProtectionClass::Models,
        ProtectionClass::UserDocuments,
        ProtectionClass::SystemRepair,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ProtectionClass::Credentials => "credentials",
            ProtectionClass::Database => "database",
            ProtectionClass::BrowserSiteStorage => "browser_site_storage",
            ProtectionClass::AiSessionsConfig => "ai_sessions_config",
            ProtectionClass::SourceWorktree => "source_worktree",
            ProtectionClass::Models => "models",
            ProtectionClass::UserDocuments => "user_documents",
            ProtectionClass::SystemRepair => "system_repair",
        }
    }

    /// Classes whose objects may be reviewed one by one (CAT-04, design D5): never automatic
    /// candidates or policy matches, only a per-object human confirmation flow may plan them.
    pub fn is_reviewable(self) -> bool {
        matches!(self, ProtectionClass::AiSessionsConfig | ProtectionClass::SourceWorktree)
    }

    /// What the class covers, for people.
    pub fn describe(self) -> &'static str {
        match self {
            ProtectionClass::Credentials => "Credentials and keys (.ssh, .gnupg, .aws, .azure, .kube, Credentials, Protect, Vault, Keychains, SystemCertificates)",
            ProtectionClass::Database => "Database files and their journals (.db, .sqlite, -wal, -shm, .mdb, .accdb)",
            ProtectionClass::BrowserSiteStorage => "Browser site storage (IndexedDB, Local Storage, Cookies, Login Data, History...)",
            ProtectionClass::AiSessionsConfig => "AI tool sessions and configuration (.claude, .codex); sessions are reviewable one by one",
            ProtectionClass::SourceWorktree => "Git worktrees (directories with .git); a clean worktree is reviewable one by one, a dirty one is blocked",
            ProtectionClass::Models => "Model weights (.gguf, .safetensors, .ckpt, .pt, .onnx)",
            ProtectionClass::UserDocuments => "User documents (resolved by identity)",
            ProtectionClass::SystemRepair => "System repair and installation stores (resolved by identity)",
        }
    }
}

const CREDENTIAL_DIRS: &[&str] = &[".ssh", ".gnupg", ".aws", ".azure", ".kube", "credentials", "protect", "vault", "keychains", "systemcertificates"];
const SITE_STORAGE: &[&str] = &["indexeddb", "local storage", "session storage", "service worker", "cookies", "login data", "web data", "history", "databases"];
const AI_DIRS: &[&str] = &[".claude", ".codex"];
const DB_EXT: &[&str] = &["db", "sqlite", "sqlite3", "db-wal", "db-shm", "sqlite-wal", "sqlite-shm", "db-journal", "mdb", "accdb"];
const MODEL_EXT: &[&str] = &["gguf", "safetensors", "ckpt", "pt", "onnx"];

fn lower_name(view: &dyn IndexView, entry: u64) -> String {
    view.name(entry).lossy().to_lowercase()
}

/// Built-in protection by name or extension of the entry itself.
pub fn protection_by_name(view: &dyn IndexView, entry: u64) -> Option<ProtectionClass> {
    let is_dir = view.info(entry).kind == ObjectKind::Directory;
    let has_git = is_dir && view.children(entry).iter().any(|c| lower_name(view, *c) == ".git");
    class_of_name(&lower_name(view, entry), is_dir, has_git)
}

/// Built-in protection by one lower-case name. `has_git_child`: a directory containing `.git`
/// (a worktree root). Shared by analysis and `protect check` so both classify alike.
pub fn class_of_name(name: &str, is_dir: bool, has_git_child: bool) -> Option<ProtectionClass> {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_owned()).unwrap_or_default();
    if is_dir {
        if CREDENTIAL_DIRS.contains(&name) {
            return Some(ProtectionClass::Credentials);
        }
        if AI_DIRS.contains(&name) {
            return Some(ProtectionClass::AiSessionsConfig);
        }
        if name == ".git" || has_git_child {
            return Some(ProtectionClass::SourceWorktree);
        }
    } else {
        if DB_EXT.contains(&ext.as_str()) {
            return Some(ProtectionClass::Database);
        }
        if MODEL_EXT.contains(&ext.as_str()) {
            return Some(ProtectionClass::Models);
        }
        if name == ".git" {
            return Some(ProtectionClass::SourceWorktree);
        }
    }
    if SITE_STORAGE.contains(&name) {
        return Some(ProtectionClass::BrowserSiteStorage);
    }
    None
}

/// Order is strictness: merging two rules on one entry keeps the larger state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateState {
    /// Evidence is complete; can be put into a plan subject to its tier.
    Allowed,
    /// Default-protected AI session or clean worktree (CAT-04): shown for review only, never an
    /// automatic candidate or policy match; only a per-object human confirmation flow may plan it.
    ProtectedReviewable,
    /// Needed evidence is missing (unread descendants, unknown time); kept for review only.
    PendingEvidence,
    /// Protected or out of policy; cannot be planned.
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct RuleRef {
    pub rule_id: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Candidate {
    pub candidate_id: String,
    pub entry_id: u64,
    pub path: RawPath,
    pub kind: ObjectKind,
    pub rules: Vec<RuleRef>,
    pub category: String,
    pub state: CandidateState,
    pub tier: Tier,
    pub reasons: Vec<String>,
    pub protection: Option<ProtectionClass>,
    pub logical: KnownTotal,
    pub allowed_modes: Vec<Mode>,
    pub probes: Vec<String>,
    pub impact: String,
    /// Owner of the first matching rule (used by the owner-process probe at execution).
    #[serde(default)]
    pub owner_id: Option<String>,
}

/// Catalog facts of a candidate (CAT-02/03/05/09): how its tier was computed and what kind of
/// row it is. Kept beside the candidate set ([`CatalogSet`]) so existing set and candidate
/// records keep their shape; the default's tier inputs are blocked, so a missing fact never widens.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, serde::Deserialize)]
pub struct CatalogFacts {
    pub rebuild_class: Option<RebuildClass>,
    pub action_kind: Option<ActionKind>,
    /// Owner-tool adapter for `tool_adapter` rows (CAT-05).
    pub adapter_ref: Option<String>,
    pub registry_status: RegistryStatus,
    pub installed_version: Option<String>,
    pub tier_inputs: TierInputs,
    /// Set by a live probe pass ([`apply_probes`]): in_use or unknown makes the row unselectable.
    pub skip: Option<SkipReason>,
}

impl CatalogFacts {
    /// Large-item hint rows (CAT-09) are shown with guidance, never planned.
    pub fn is_hint(&self) -> bool {
        self.action_kind == Some(ActionKind::ReviewOnlyRow)
    }

    /// Owner-tool rows (CAT-05) are executed by the owning tool, never as file actions.
    pub fn is_adapter_row(&self) -> bool {
        self.action_kind == Some(ActionKind::ToolAdapter)
    }
}

/// Catalog facts per candidate id plus what could not be judged; produced with the set.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, serde::Deserialize)]
pub struct CatalogSet {
    pub facts: BTreeMap<String, CatalogFacts>,
    /// Matched objects left out because a needed fact was unavailable (e.g. no timestamp).
    pub undetermined: Vec<Undetermined>,
}

/// Could become a file action in a plan right now (state, tier, modes and live probes). Without
/// catalog facts the probes were never run, so the candidate is not plannable through a catalog entry.
pub fn plannable(c: &Candidate, facts: Option<&CatalogFacts>) -> bool {
    c.state == CandidateState::Allowed && c.tier != Tier::Blocked && !c.allowed_modes.is_empty() && facts.is_some_and(|f| f.skip.is_none())
}

/// Part of the non-interactive default selection (CLI-12).
pub fn default_selected(c: &Candidate, facts: Option<&CatalogFacts>) -> bool {
    plannable(c, facts) && c.tier == Tier::DefaultSelected
}

/// Objects a rule matched that could not be judged and therefore are not candidates (CAT-07).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Undetermined {
    pub rule_id: String,
    pub reason: String,
    pub count: u64,
}

fn rebuild_rank(r: RebuildClass) -> u8 {
    match r {
        RebuildClass::LocalRebuild => 0,
        RebuildClass::NetworkRedownload => 1,
        RebuildClass::NotRebuildable => 2,
    }
}

/// Record live probe results on a candidate and recompute its tier with the shared function
/// (CAT-06: called at preview and plan creation; the executor re-checks before acting).
pub fn apply_probes(c: &mut Candidate, facts: &mut CatalogFacts, results: Vec<ProbeResult>) {
    facts.tier_inputs.probes = results;
    let d = compute_tier(&facts.tier_inputs);
    c.tier = if c.state == CandidateState::Blocked { Tier::Blocked } else { d.tier };
    facts.skip = d.skip;
    c.reasons.retain(|r| !r.starts_with("probe:"));
    c.reasons.extend(d.reasons.into_iter().filter(|r| r.starts_with("probe:")));
    c.reasons.sort();
    c.reasons.dedup();
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct CandidateSet {
    pub candidate_set_id: String,
    pub scan_id: String,
    pub revision: u32,
    pub rule_set_digest: String,
    pub protection_version: String,
    pub candidates: Vec<Candidate>,
    /// Rules that could not be applied (root missing from this scan, wrong platform...).
    pub inapplicable: Vec<(String, String)>,
}

pub struct AnalysisInput<'a> {
    pub scan_id: &'a str,
    pub revision: u32,
    pub now_unix: i64,
    pub roots: &'a [ResolvedRoot],
    pub protected: &'a [ProtectedRoot],
}

/// Git state of a worktree root, supplied by a read-only probe. Absent = unknown = blocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeState {
    /// No uncommitted or untracked changes.
    Clean,
    /// Uncommitted or untracked changes.
    Dirty,
    /// The probe could not decide.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeStatus {
    pub volume_id: String,
    pub file_id: String,
    pub state: WorktreeState,
}

/// The user protection list as the caller could load it.
#[derive(Debug, Clone, Copy, Default)]
pub enum UserList<'a> {
    /// Not consulted (tests, previews); the executor still re-checks before acting.
    #[default]
    NotSupplied,
    Loaded(&'a UserProtection),
    /// Unreadable or invalid (PRT-02): analysis continues read-only, nothing becomes plannable.
    Unavailable(&'a str),
}

/// Protection layers 1 and 3 plus worktree evidence for analysis (layer 2, the default classes,
/// is built in). Hard-deny and user hits make candidates `blocked` with a reason; they are never
/// silently dropped.
#[derive(Debug, Clone, Copy, Default)]
pub struct AnalysisProtection<'a> {
    pub hard_deny: Option<&'a HardDenySet>,
    pub user: UserList<'a>,
    pub worktrees: &'a [WorktreeStatus],
}

struct Subtree {
    logical: KnownTotal,
    /// Class of the entry itself.
    own: Option<ProtectionClass>,
    /// Classes found strictly below the entry.
    inner: BTreeSet<ProtectionClass>,
    unread: u64,
    boundaries: u64,
    placeholders: u64,
    /// Objects other than directories (files, links, special objects) at or under the entry.
    objects: u64,
}

fn subtree(view: &dyn IndexView, entry: u64, protected_ids: &BTreeMap<u64, ProtectionClass>) -> Subtree {
    let mut s = Subtree { logical: KnownTotal::default(), own: None, inner: BTreeSet::new(), unread: 0, boundaries: 0, placeholders: 0, objects: 0 };
    let mut seen = BTreeSet::new();
    let mut stack = vec![entry];
    while let Some(id) = stack.pop() {
        let f = view.info(id);
        let class = protected_ids.get(&id).copied().or_else(|| protection_by_name(view, id));
        if id == entry {
            s.own = class;
        } else if let Some(c) = class {
            s.inner.insert(c);
        }
        s.unread += f.skipped_here;
        if f.boundary {
            s.boundaries += 1;
            continue;
        }
        if f.cloud == CloudState::Placeholder {
            s.placeholders += 1;
        }
        if f.kind == ObjectKind::Directory {
            stack.extend_from_slice(view.children(id));
        } else if seen.insert(f.object_id) {
            s.objects += 1;
            let _ = s.logical.add(f.logical.map(Bytes));
        }
    }
    for _ in 0..s.unread {
        let _ = s.logical.add(None);
    }
    s
}

fn matches_object_type(t: ObjectType, kind: ObjectKind) -> bool {
    match t {
        ObjectType::Any => matches!(kind, ObjectKind::File | ObjectKind::Directory),
        ObjectType::File => kind == ObjectKind::File,
        ObjectType::Directory => kind == ObjectKind::Directory,
    }
}

/// Entries a rule targets under one resolved root.
fn targets(view: &dyn IndexView, rule: &Rule, root: u64, ci: bool) -> Vec<u64> {
    let mut base = vec![root];
    for comp in &rule.matcher.relative_components {
        base = base
            .iter()
            .flat_map(|b| view.children(*b).iter().copied())
            .filter(|c| !view.info(*c).boundary && glob_match(comp, &view.name(*c).lossy(), ci))
            .collect();
    }
    let selective = rule.matcher.name_glob.is_some() || !rule.matcher.extensions.is_empty();
    if !selective {
        return base.into_iter().filter(|b| matches_object_type(rule.matcher.object_type, view.info(*b).kind)).collect();
    }
    let mut out = Vec::new();
    let mut stack: Vec<(u64, u8)> = base.iter().map(|b| (*b, 0)).collect();
    while let Some((dir, depth)) = stack.pop() {
        for &c in view.children(dir) {
            let f = view.info(c);
            if f.boundary {
                continue;
            }
            let name = view.name(c).lossy();
            let glob_ok = rule.matcher.name_glob.as_deref().is_none_or(|g| glob_match(g, &name, ci));
            let ext_ok = rule.matcher.extensions.is_empty()
                || name.rsplit_once('.').is_some_and(|(_, e)| rule.matcher.extensions.iter().any(|x| x.eq_ignore_ascii_case(e)));
            let excluded = rule.exclude.iter().any(|g| glob_match(g, &name, ci));
            if glob_ok && ext_ok && !excluded && matches_object_type(rule.matcher.object_type, f.kind) {
                out.push(c);
            } else if f.kind == ObjectKind::Directory && depth < rule.matcher.max_depth {
                stack.push((c, depth + 1));
            }
        }
    }
    out
}

/// Default-class outcome for one candidate entry (ANA-03, CAT-04).
enum ClassOutcome {
    Clear,
    Blocked(ProtectionClass, String),
    Reviewable(ProtectionClass),
}

/// `ancestors`: classes of the entry's ancestors, nearest first. Any non-reviewable class blocks;
/// a container of a session or worktree is blocked (no recursive bypass); a session inside an AI
/// directory and a clean worktree root are reviewable; anything else under them is blocked.
fn class_outcome(view: &dyn IndexView, entry: u64, sub: &Subtree, ancestors: &[ProtectionClass], worktrees: &[WorktreeStatus]) -> ClassOutcome {
    let block = |c: ProtectionClass| ClassOutcome::Blocked(c, format!("protected:{}", c.as_str()));
    let self_class = sub.own.or_else(|| ancestors.first().copied());
    if let Some(c) = sub.own.iter().chain(ancestors).chain(&sub.inner).copied().find(|c| !c.is_reviewable()) {
        return block(c);
    }
    let Some(class) = self_class else {
        return sub.inner.first().copied().map_or(ClassOutcome::Clear, block);
    };
    match class {
        // The AI directory itself holds configuration as well as sessions: blocked as a whole.
        ProtectionClass::AiSessionsConfig if sub.own.is_some() => block(class),
        // A session: everything around and inside it must be AI data (not inside a worktree).
        ProtectionClass::AiSessionsConfig => match ancestors.iter().chain(&sub.inner).copied().find(|c| *c != class) {
            Some(other) => block(other),
            None => ClassOutcome::Reviewable(class),
        },
        // Only a worktree root that is not nested in another worktree; Git state decides.
        ProtectionClass::SourceWorktree => {
            let f = view.info(entry);
            let is_root = sub.own.is_some() && f.kind == ObjectKind::Directory && lower_name(view, entry) != ".git";
            if !is_root || ancestors.contains(&ProtectionClass::SourceWorktree) {
                return block(class);
            }
            match worktrees.iter().find(|w| w.volume_id == f.volume_id && w.file_id == f.file_id).map(|w| w.state) {
                Some(WorktreeState::Clean) => ClassOutcome::Reviewable(class),
                Some(WorktreeState::Dirty) => ClassOutcome::Blocked(class, "worktree_dirty".into()),
                _ => ClassOutcome::Blocked(class, "worktree_status_unknown".into()),
            }
        }
        _ => block(class),
    }
}

/// Hard-deny layer for an indexed entry: textual pre-filter, then identity of the entry, its
/// indexed ancestors and (for whole-tree actions) any deny root inside it.
fn hard_deny_reason(view: &dyn IndexView, entry: u64, path: &RawPath, recursive: bool, hard: Option<&HardDenySet>) -> Option<String> {
    if let Err(r) = prefilter(path) {
        return Some(r.to_string());
    }
    let hard = hard?;
    let id = |e: u64| {
        let f = view.info(e);
        ObjectIdentity::new(f.volume_id.clone(), f.file_id.clone())
    };
    if let Some(r) = hard.root_at(&id(entry)) {
        return Some(format!("{} ({})", r.display, r.class.as_str()));
    }
    if let Some(r) = std::iter::successors(view.parent(entry), |p| view.parent(*p)).find_map(|a| hard.covering(&id(a))) {
        return Some(format!("inside {} ({})", r.display, r.class.as_str()));
    }
    if recursive && let Some(r) = hard_deny_within(view, entry, hard) {
        return Some(format!("contains {} ({})", r.display, r.class.as_str()));
    }
    None
}

/// Evaluate all enabled rules for the view's platform with only the built-in protection classes
/// and the textual hard-deny pre-filter (no hard-deny roots, no user list).
pub fn analyze(view: &dyn IndexView, rules: &RuleSet, input: &AnalysisInput<'_>) -> CandidateSet {
    analyze_with(view, rules, input, &AnalysisProtection::default())
}

/// Evaluate all enabled rules with every protection layer the caller could supply, without the
/// tested registry (tiers are not lowered for versions). Entries use [`analyze_catalog`].
pub fn analyze_with(view: &dyn IndexView, rules: &RuleSet, input: &AnalysisInput<'_>, prot: &AnalysisProtection<'_>) -> CandidateSet {
    analyze_catalog(view, rules, input, prot, &CatalogContext::default()).0
}

/// The analysis every entry (GUI, CLI, agent) uses: all protection layers plus the tested
/// registry and installed versions. Every tier comes from [`compute_tier`].
pub fn analyze_catalog(
    view: &dyn IndexView, rules: &RuleSet, input: &AnalysisInput<'_>, prot: &AnalysisProtection<'_>, ctx: &CatalogContext<'_>,
) -> (CandidateSet, CatalogSet) {
    let platform = view.platform();
    let mut facts_by_entry: BTreeMap<u64, CatalogFacts> = BTreeMap::new();
    let ci = platform == Platform::Windows;
    let mut protected_ids = BTreeMap::new();
    for p in input.protected {
        if let Some(e) = view.find_identity(&p.volume_id, &p.file_id) {
            protected_ids.insert(e, p.class);
        }
    }
    let mut inapplicable = Vec::new();
    let mut undetermined: BTreeMap<(String, String), u64> = BTreeMap::new();
    let mut by_entry: BTreeMap<u64, Candidate> = BTreeMap::new();
    for rule in rules.enabled_for(platform) {
        let verdict = ctx.verdict(rule);
        let dependency = is_dependency_store(rule);
        let roots: Vec<u64> = input
            .roots
            .iter()
            .filter(|r| r.rule_id == rule.rule_id)
            .filter_map(|r| view.find_identity(&r.volume_id, &r.file_id))
            .collect();
        if roots.is_empty() {
            inapplicable.push((rule.rule_id.clone(), "no resolved root is inside this scan".into()));
            continue;
        }
        for root in roots {
            for entry in targets(view, rule, root, ci) {
                let f = view.info(entry);
                let path = view.path(entry);
                let sub = subtree(view, entry, &protected_ids);
                // Protection of an ancestor also covers this entry (nearest first).
                let ancestor_classes: Vec<ProtectionClass> = std::iter::successors(view.parent(entry), |p| view.parent(*p))
                    .filter_map(|a| protected_ids.get(&a).copied().or_else(|| protection_by_name(view, a)))
                    .collect();
                let mut reasons = Vec::new();
                let mut state = CandidateState::Allowed;
                let mut evidence = Tier::DefaultSelected;
                let mut protection = None;
                match class_outcome(view, entry, &sub, &ancestor_classes, prot.worktrees) {
                    ClassOutcome::Clear => {}
                    ClassOutcome::Blocked(class, why) => {
                        protection = Some(class);
                        reasons.push(why);
                        state = CandidateState::Blocked;
                    }
                    ClassOutcome::Reviewable(class) => {
                        protection = Some(class);
                        reasons.push(format!("protected_reviewable:{}", class.as_str()));
                        state = CandidateState::ProtectedReviewable;
                    }
                }
                // Layer 1 outranks everything; it is reported even when another reason blocks.
                if let Some(why) = hard_deny_reason(view, entry, &path, f.kind == ObjectKind::Directory, prot.hard_deny) {
                    reasons.push(format!("hard_deny:{why}"));
                    state = CandidateState::Blocked;
                }
                match prot.user {
                    UserList::NotSupplied => {}
                    UserList::Loaded(user) => {
                        if let Some(e) = user.match_path(&path, f.kind == ObjectKind::Directory) {
                            reasons.push(format!("user_protected:{}", e.id));
                            state = CandidateState::Blocked;
                        }
                        if let Some(e) = user.protects_rule(&rule.rule_id) {
                            reasons.push(format!("user_protected_rule:{}", e.id));
                            state = CandidateState::Blocked;
                        }
                    }
                    UserList::Unavailable(_) => {
                        reasons.push("user_protection_unavailable".into());
                        state = state.max(CandidateState::PendingEvidence);
                    }
                }
                if sub.placeholders > 0 {
                    reasons.push("cloud_placeholder".into());
                    state = CandidateState::Blocked;
                }
                if f.boundary {
                    reasons.push("link_or_mount_boundary".into());
                    state = CandidateState::Blocked;
                }
                if sub.unread > 0 && state != CandidateState::Blocked {
                    reasons.push("unread_descendants".into());
                    state = CandidateState::PendingEvidence;
                }
                // WFA-113: a folder the index shows empty (for example emptied by an earlier cleanup) holds nothing to
                // clean, so it is not a candidate (and never default-selected). Blocked rows stay visible as such.
                if f.kind == ObjectKind::Directory && sub.objects == 0 && sub.unread == 0 && sub.boundaries == 0 && state != CandidateState::Blocked {
                    continue;
                }
                if let Some(days) = rule.thresholds.min_age_days {
                    match f.modified_unix {
                        // An unavailable timestamp never satisfies an age condition (CAT-07): not a
                        // candidate, counted as undetermined. Blocked rows stay visible as such.
                        None if state != CandidateState::Blocked => {
                            *undetermined.entry((rule.rule_id.clone(), "modified_time_unknown".into())).or_default() += 1;
                            continue;
                        }
                        None => reasons.push("modified_time_unknown".into()),
                        Some(t) if input.now_unix - t < i64::from(days) * 86_400 => continue, // too recent: not a candidate
                        _ => {}
                    }
                }
                if f.quality == IdentityQuality::Weak {
                    reasons.push("weak_identity".into());
                    evidence = Tier::ReviewOnly;
                }
                match rule.action_kind {
                    ActionKind::ReviewOnlyRow => reasons.push("large_item_hint:not_selectable".into()),
                    ActionKind::ToolAdapter => reasons.push(format!("owner_tool:{}", rule.adapter_ref.as_deref().unwrap_or("none"))),
                    ActionKind::Filesystem => {}
                }
                if let Some(why) = &verdict.reason {
                    reasons.push(format!("registry:{why}"));
                }
                let inputs = TierInputs {
                    rule_max: rule.max_tier,
                    dependency,
                    registry: verdict.status,
                    protection: match state {
                        CandidateState::Blocked => Tier::Blocked,
                        CandidateState::ProtectedReviewable => Tier::ReviewOnly,
                        _ => Tier::DefaultSelected,
                    },
                    evidence: if state == CandidateState::PendingEvidence { Tier::ReviewOnly } else { evidence },
                    probes: vec![],
                };
                // Only filesystem rows carry file modes; hint and owner-tool rows are never file actions.
                let modes = if state == CandidateState::Blocked || rule.action_kind != ActionKind::Filesystem { vec![] } else { rule.allowed_modes.clone() };
                let rref = RuleRef { rule_id: rule.rule_id.clone(), version: rule.version.clone() };
                match by_entry.get_mut(&entry) {
                    // Two rules on one entry: one candidate, both reasons, stricter outcome (ANA-04).
                    Some(existing) => {
                        existing.rules.push(rref);
                        existing.reasons.extend(reasons);
                        existing.state = existing.state.max(state);
                        existing.protection = existing.protection.or(protection);
                        existing.allowed_modes.retain(|m| modes.contains(m));
                        existing.probes.extend(rule.probes.iter().cloned());
                        let facts = facts_by_entry.entry(entry).or_default();
                        facts.tier_inputs.merge(&inputs);
                        facts.registry_status = facts.registry_status.worse(verdict.status);
                        if facts.rebuild_class.is_none_or(|r| rebuild_rank(rule.rebuild_class) > rebuild_rank(r)) {
                            facts.rebuild_class = Some(rule.rebuild_class);
                        }
                        if facts.action_kind != Some(rule.action_kind) {
                            // Mixed row kinds on one object: shown for review only, never a file action.
                            facts.action_kind = Some(ActionKind::ReviewOnlyRow);
                            existing.allowed_modes.clear();
                        }
                    }
                    None => {
                        by_entry.insert(entry, Candidate {
                            candidate_id: String::new(),
                            entry_id: entry,
                            path,
                            kind: f.kind,
                            rules: vec![rref],
                            category: rule.category.clone(),
                            state,
                            tier: Tier::Blocked, // computed below from the stored inputs
                            reasons,
                            protection,
                            logical: sub.logical,
                            allowed_modes: modes,
                            probes: rule.probes.clone(),
                            impact: rule.impact.summary.clone(),
                            owner_id: Some(rule.owner.id.clone()),
                        });
                        facts_by_entry.insert(entry, CatalogFacts {
                            rebuild_class: Some(rule.rebuild_class),
                            action_kind: Some(rule.action_kind),
                            adapter_ref: rule.adapter_ref.clone(),
                            registry_status: verdict.status,
                            installed_version: verdict.installed.clone(),
                            tier_inputs: inputs,
                            skip: None,
                        });
                    }
                }
            }
        }
    }
    // Parent/child overlap: an allowed file-action ancestor absorbs its candidate descendants; a
    // blocked, pending, hint or owner-tool ancestor never covers them (they stay separate, ANA-04).
    let allowed: BTreeSet<u64> =
        by_entry.iter().filter(|(_, c)| c.state == CandidateState::Allowed && !c.allowed_modes.is_empty()).map(|(e, _)| *e).collect();
    let covered: Vec<u64> = by_entry
        .keys()
        .copied()
        .filter(|e| std::iter::successors(view.parent(*e), |p| view.parent(*p)).any(|a| allowed.contains(&a)))
        .collect();
    for e in covered {
        by_entry.remove(&e);
    }
    // One object, one action (IDX-04): several names of a hard-linked file collapse to one
    // review-only candidate; removing one name would not free the object's space.
    let mut names_per_object: BTreeMap<u64, u32> = BTreeMap::new();
    for c in by_entry.values() {
        *names_per_object.entry(view.info(c.entry_id).object_id).or_default() += 1;
    }
    let mut seen_objects = BTreeSet::new();
    let mut candidates: Vec<(Candidate, CatalogFacts)> = Vec::new();
    for mut c in by_entry.into_values() {
        let mut facts = facts_by_entry.remove(&c.entry_id).unwrap_or_default();
        let f = view.info(c.entry_id);
        if f.kind != ObjectKind::Directory {
            if !seen_objects.insert(f.object_id) {
                continue;
            }
            let in_scope = names_per_object[&f.object_id];
            if in_scope > 1 {
                c.reasons.push(format!("hard_link_shared:{in_scope}_names"));
                facts.tier_inputs.evidence = Tier::ReviewOnly;
            } else if f.link_count.is_some_and(|n| n > 1) {
                c.reasons.push("hard_link_outside_scope".into());
                facts.tier_inputs.evidence = Tier::ReviewOnly;
            }
        }
        // One shared computation for every tier (CAT-02, design D3).
        let d = compute_tier(&facts.tier_inputs);
        c.tier = if c.state == CandidateState::Blocked { Tier::Blocked } else { d.tier };
        c.reasons.extend(d.reasons);
        candidates.push((c, facts));
    }
    for (c, _) in &mut candidates {
        c.rules.sort_by(|a, b| a.rule_id.cmp(&b.rule_id));
        c.rules.dedup();
        c.reasons.sort();
        c.reasons.dedup();
        c.probes.sort();
        c.probes.dedup();
        let id = serde_json::json!({"scan": input.scan_id, "revision": input.revision, "entry": c.entry_id, "rules": c.rules});
        c.candidate_id = format!("c_{}", &digest(&id).expect("plain JSON")[..24]);
    }
    // The protection inputs are part of the set identity: a changed user list or worktree state
    // yields a new set rather than silently replacing the stored one.
    let user_key = match prot.user {
        UserList::NotSupplied => serde_json::Value::Null,
        UserList::Loaded(u) => serde_json::to_value(u.entries()).expect("entries serialize"),
        UserList::Unavailable(_) => "unavailable".into(),
    };
    let worktree_key: Vec<_> = prot.worktrees.iter().map(|w| (&w.volume_id, &w.file_id, w.state)).collect();
    // Registry verdicts change tiers, so the installed versions they used are part of the identity too.
    let catalog_key = ctx.registry.map(|_| serde_json::json!({"catalog": crate::catalog::CATALOG_VERSION, "installed": ctx.installed}));
    let mut set_key = serde_json::json!({
        "scan": input.scan_id, "revision": input.revision, "rules": rules.digest, "protection": PROTECTION_VERSION,
        "hard_deny": prot.hard_deny.map(|_| HARD_DENY_VERSION), "user": user_key, "worktrees": worktree_key,
        "candidates": candidates.iter().map(|(c, _)| c.candidate_id.clone()).collect::<Vec<_>>(),
    });
    if let Some(k) = catalog_key {
        set_key["catalog"] = k;
    }
    let catalog = CatalogSet {
        facts: candidates.iter().map(|(c, f)| (c.candidate_id.clone(), f.clone())).collect(),
        undetermined: undetermined.into_iter().map(|((rule_id, reason), count)| Undetermined { rule_id, reason, count }).collect(),
    };
    let set = CandidateSet {
        candidate_set_id: format!("cs_{}", &digest(&set_key).expect("plain JSON")[..24]),
        scan_id: input.scan_id.into(),
        revision: input.revision,
        rule_set_digest: rules.digest.clone(),
        protection_version: PROTECTION_VERSION.into(),
        candidates: candidates.into_iter().map(|(c, _)| c).collect(),
        inapplicable,
    };
    (set, catalog)
}

/// Evaluate one not-yet-enabled rule without making anything actionable: candidates keep their
/// computed state and tier for review, carry `experimental_preview`, and have no allowed modes,
/// so no plan can be built from them.
pub fn analyze_preview(view: &dyn IndexView, rule: &Rule, input: &AnalysisInput<'_>) -> CandidateSet {
    analyze_preview_with(view, rule, input, &AnalysisProtection::default())
}

/// [`analyze_preview`] with the caller's protection layers.
pub fn analyze_preview_with(view: &dyn IndexView, rule: &Rule, input: &AnalysisInput<'_>, prot: &AnalysisProtection<'_>) -> CandidateSet {
    let mut forced = rule.clone();
    forced.status = crate::rules::RuleStatus::Enabled;
    let set = RuleSet { rules: vec![forced], digest: format!("preview:{}@{}", rule.rule_id, rule.version) };
    let mut out = analyze_with(view, &set, input, prot);
    for c in &mut out.candidates {
        c.reasons.push("experimental_preview".into());
        c.allowed_modes.clear();
    }
    out
}

/// Simple in-memory view used by tests and by small callers.
#[derive(Debug, Default)]
pub struct MemView {
    pub platform: Option<Platform>,
    pub root: u64,
    pub names: HashMap<u64, RawPath>,
    pub parents: HashMap<u64, Option<u64>>,
    pub kids: HashMap<u64, Vec<u64>>,
    pub facts: HashMap<u64, EntryFacts>,
    pub identities: HashMap<(String, String), u64>,
}

static NO_KIDS: [u64; 0] = [];

impl IndexView for MemView {
    fn platform(&self) -> Platform {
        self.platform.unwrap_or(Platform::Windows)
    }
    fn root(&self) -> u64 {
        self.root
    }
    fn name(&self, entry: u64) -> &RawPath {
        &self.names[&entry]
    }
    fn parent(&self, entry: u64) -> Option<u64> {
        self.parents.get(&entry).copied().flatten()
    }
    fn children(&self, entry: u64) -> &[u64] {
        self.kids.get(&entry).map(Vec::as_slice).unwrap_or(&NO_KIDS)
    }
    fn info(&self, entry: u64) -> &EntryFacts {
        &self.facts[&entry]
    }
    fn path(&self, entry: u64) -> RawPath {
        let mut parts = vec![self.names[&entry].clone()];
        let mut cur = self.parent(entry);
        while let Some(p) = cur {
            parts.push(self.names[&p].clone());
            cur = self.parent(p);
        }
        parts.reverse();
        let mut path = parts[0].clone();
        for p in &parts[1..] {
            path = path.join(p).expect("same platform");
        }
        path
    }
    fn find_identity(&self, volume_id: &str, file_id: &str) -> Option<u64> {
        self.identities.get(&(volume_id.to_owned(), file_id.to_owned())).copied()
    }
}

#[cfg(test)]
mod tests;
