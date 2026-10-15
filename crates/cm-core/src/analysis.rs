//! Candidate analysis (ANA-02..ANA-06, IDX-04): turns a published scan revision plus a rule set
//! into reviewable candidates. Analysis never writes targets and never grants authorization;
//! names are data (no file name can change a decision except through literal glob matching).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Serialize;

use crate::bytes::{Bytes, KnownTotal};
use crate::c14n::digest;
use crate::capability::Platform;
use crate::model::{CloudState, IdentityQuality, ObjectKind};
use crate::path::RawPath;
use crate::rules::{Mode, ObjectType, Rule, RuleSet, Tier, glob_match};

/// Version of the built-in protection classes; bound into candidate sets and plans.
pub const PROTECTION_VERSION: &str = "protection/1";

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
    let name = lower_name(view, entry);
    let facts = view.info(entry);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_owned()).unwrap_or_default();
    if facts.kind == ObjectKind::Directory {
        if CREDENTIAL_DIRS.contains(&name.as_str()) {
            return Some(ProtectionClass::Credentials);
        }
        if AI_DIRS.contains(&name.as_str()) {
            return Some(ProtectionClass::AiSessionsConfig);
        }
        if name == ".git" || view.children(entry).iter().any(|c| lower_name(view, *c) == ".git") {
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
    if SITE_STORAGE.contains(&name.as_str()) {
        return Some(ProtectionClass::BrowserSiteStorage);
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateState {
    /// Evidence is complete; can be put into a plan subject to its tier.
    Allowed,
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

struct Subtree {
    logical: KnownTotal,
    protection: Option<ProtectionClass>,
    unread: u64,
    boundaries: u64,
    placeholders: u64,
}

fn subtree(view: &dyn IndexView, entry: u64, protected_ids: &BTreeMap<u64, ProtectionClass>) -> Subtree {
    let mut s = Subtree { logical: KnownTotal::default(), protection: None, unread: 0, boundaries: 0, placeholders: 0 };
    let mut seen = BTreeSet::new();
    let mut stack = vec![entry];
    while let Some(id) = stack.pop() {
        let f = view.info(id);
        s.protection = s.protection.or_else(|| protected_ids.get(&id).copied()).or_else(|| protection_by_name(view, id));
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

/// Evaluate all enabled rules for the view's platform.
pub fn analyze(view: &dyn IndexView, rules: &RuleSet, input: &AnalysisInput<'_>) -> CandidateSet {
    let platform = view.platform();
    let ci = platform == Platform::Windows;
    let mut protected_ids = BTreeMap::new();
    for p in input.protected {
        if let Some(e) = view.find_identity(&p.volume_id, &p.file_id) {
            protected_ids.insert(e, p.class);
        }
    }
    let mut inapplicable = Vec::new();
    let mut by_entry: BTreeMap<u64, Candidate> = BTreeMap::new();
    for rule in rules.enabled_for(platform) {
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
                let sub = subtree(view, entry, &protected_ids);
                // Protection of an ancestor also covers this entry.
                let ancestor_protection = std::iter::successors(view.parent(entry), |p| view.parent(*p))
                    .find_map(|a| protected_ids.get(&a).copied().or_else(|| protection_by_name(view, a)));
                let mut reasons = Vec::new();
                let mut state = CandidateState::Allowed;
                let mut tier = rule.max_tier;
                let protection = sub.protection.or(ancestor_protection);
                if let Some(class) = protection {
                    reasons.push(format!("protected:{}", serde_json::to_value(class).unwrap().as_str().unwrap()));
                    state = CandidateState::Blocked;
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
                if let Some(days) = rule.thresholds.min_age_days {
                    match f.modified_unix {
                        None if state == CandidateState::Allowed => {
                            reasons.push("modified_time_unknown".into());
                            state = CandidateState::PendingEvidence;
                        }
                        Some(t) if input.now_unix - t < i64::from(days) * 86_400 => continue, // too recent: not a candidate
                        _ => {}
                    }
                }
                if f.quality == IdentityQuality::Weak {
                    reasons.push("weak_identity".into());
                    tier = tier.min(Tier::ReviewOnly);
                }
                if state == CandidateState::Blocked {
                    tier = Tier::Blocked;
                } else if state == CandidateState::PendingEvidence {
                    tier = tier.min(Tier::ReviewOnly);
                }
                let modes = if state == CandidateState::Blocked { vec![] } else { rule.allowed_modes.clone() };
                let rref = RuleRef { rule_id: rule.rule_id.clone(), version: rule.version.clone() };
                match by_entry.get_mut(&entry) {
                    // Two rules on one entry: one candidate, both reasons, stricter outcome (ANA-04).
                    Some(existing) => {
                        existing.rules.push(rref);
                        existing.reasons.extend(reasons);
                        existing.state = existing.state.max(state);
                        existing.tier = existing.tier.min(tier);
                        existing.allowed_modes.retain(|m| modes.contains(m));
                        existing.probes.extend(rule.probes.iter().cloned());
                    }
                    None => {
                        by_entry.insert(entry, Candidate {
                            candidate_id: String::new(),
                            entry_id: entry,
                            path: view.path(entry),
                            kind: f.kind,
                            rules: vec![rref],
                            category: rule.category.clone(),
                            state,
                            tier,
                            reasons,
                            protection,
                            logical: sub.logical,
                            allowed_modes: modes,
                            probes: rule.probes.clone(),
                            impact: rule.impact.summary.clone(),
                            owner_id: Some(rule.owner.id.clone()),
                        });
                    }
                }
            }
        }
    }
    // Parent/child overlap: an allowed ancestor absorbs its candidate descendants; a blocked or
    // pending ancestor never covers them (they stay separate, ANA-04).
    let allowed: BTreeSet<u64> = by_entry.iter().filter(|(_, c)| c.state == CandidateState::Allowed).map(|(e, _)| *e).collect();
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
    let mut candidates: Vec<Candidate> = Vec::new();
    for mut c in by_entry.into_values() {
        let f = view.info(c.entry_id);
        if f.kind != ObjectKind::Directory {
            if !seen_objects.insert(f.object_id) {
                continue;
            }
            let in_scope = names_per_object[&f.object_id];
            if in_scope > 1 {
                c.reasons.push(format!("hard_link_shared:{in_scope}_names"));
                c.tier = c.tier.min(Tier::ReviewOnly);
            } else if f.link_count.is_some_and(|n| n > 1) {
                c.reasons.push("hard_link_outside_scope".into());
                c.tier = c.tier.min(Tier::ReviewOnly);
            }
        }
        candidates.push(c);
    }
    for c in &mut candidates {
        c.rules.sort_by(|a, b| a.rule_id.cmp(&b.rule_id));
        c.rules.dedup();
        c.reasons.sort();
        c.reasons.dedup();
        c.probes.sort();
        c.probes.dedup();
        let id = serde_json::json!({"scan": input.scan_id, "revision": input.revision, "entry": c.entry_id, "rules": c.rules});
        c.candidate_id = format!("c_{}", &digest(&id).expect("plain JSON")[..24]);
    }
    let set_key = serde_json::json!({
        "scan": input.scan_id, "revision": input.revision, "rules": rules.digest, "protection": PROTECTION_VERSION,
        "candidates": candidates.iter().map(|c| c.candidate_id.clone()).collect::<Vec<_>>(),
    });
    CandidateSet {
        candidate_set_id: format!("cs_{}", &digest(&set_key).expect("plain JSON")[..24]),
        scan_id: input.scan_id.into(),
        revision: input.revision,
        rule_set_digest: rules.digest.clone(),
        protection_version: PROTECTION_VERSION.into(),
        candidates,
        inapplicable,
    }
}

/// Evaluate one not-yet-enabled rule without making anything actionable: candidates keep their
/// computed state and tier for review, carry `experimental_preview`, and have no allowed modes,
/// so no plan can be built from them.
pub fn analyze_preview(view: &dyn IndexView, rule: &Rule, input: &AnalysisInput<'_>) -> CandidateSet {
    let mut forced = rule.clone();
    forced.status = crate::rules::RuleStatus::Enabled;
    let set = RuleSet { rules: vec![forced], digest: format!("preview:{}@{}", rule.rule_id, rule.version) };
    let mut out = analyze(view, &set, input);
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
