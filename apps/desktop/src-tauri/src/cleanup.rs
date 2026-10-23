//! P04 candidates and P05 plan review/authorization (GUI-03/04, PLN-01..07).
//!
//! Safety properties of this IPC surface:
//! - The WebView never sends a path or a plan body. It names a candidate set, candidate ids
//!   and a mode; the host freezes the plan with the shared core (`build_plan`) from the
//!   published index and stores it. Every later call names the plan by id and the host loads
//!   it from the audit store (whose loader re-verifies the body digest).
//! - Creating a plan authorizes nothing. Approval writes the same `Receipt`
//!   (`InteractiveReview`) the CLI's `plan review` writes, and only when the person typed the
//!   first 8 characters of the digest shown with this plan's full detail. The detail call
//!   issues a single-use review token bound to the plan id and digest; `approve_plan` needs
//!   that token *and* the typed code, so no IPC call approves a plan that was not opened for
//!   review in this window. (As with the CLI's TTY, this is a presentation control; it does not
//!   isolate a same-user process that can drive the UI — PLN-07 threat boundary.)
//! - Policies: listing, pausing and revoking only restrict. Enabling grants future authority,
//!   so it needs the policy id typed by the person. New policies are created paused.

use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasher, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cm_core::analysis::{
    AnalysisInput, AnalysisProtection, Candidate, CandidateSet, CandidateState, CatalogFacts, CatalogSet, IndexView, UserList, analyze_catalog, apply_probes,
    plannable,
};
use cm_core::catalog::{CatalogContext, SkipReason};
use cm_core::authz::{Policy, PolicyState, Receipt, ReceiptSource, current_principal, policy_covers, receipt_valid};
use cm_core::error::ErrorCode;
use cm_core::plan::v2::PLAN_V2_CONTRACT;
use cm_core::plan::{Plan, PlanInput, build_plan, check_plan, parse_utc};
use cm_core::recovery::{RECOVERY_CONTRACT, RecoveryKind, RecoveryPlan, RecoveryReceipt};
use cm_core::rules::{Mode, Tier};
use cm_core::time::{now_utc, utc_from_unix};
use cm_store::Stores;
use serde::{Deserialize, Serialize};

use crate::args;
use crate::commands::{St, blocking};
use crate::dto::{IpcError, IpcResult, IssueDto, last_component};
use crate::host::{AppState, lock};

const REVIEW_TTL: Duration = Duration::from_secs(30 * 60);
const MAX_REVIEW_ATTEMPTS: u8 = 5;
const MAX_REVIEWS: usize = 64;
/// Candidates per plan request (each may expand to many files).
pub const MAX_SELECTION: usize = 2_000;

pub fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub(crate) fn new_id(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{prefix}_desk_{}_{nanos}", std::process::id())
}

fn ipc(code: ErrorCode, reason: &'static str, message: impl Into<String>) -> IpcError {
    IpcError::new(code, reason, message)
}

// ---------------------------------------------------------------------------------------------
// Review sessions

struct Review {
    plan_id: String,
    digest: String,
    issued: Instant,
    attempts: u8,
}

/// Single-use review tokens: issued with a plan's full detail, consumed by its approval.
#[derive(Default)]
pub struct Reviews {
    map: HashMap<String, Review>,
    counter: u64,
}

impl Reviews {
    pub fn issue(&mut self, plan_id: &str, digest: &str) -> String {
        self.map.retain(|_, r| r.issued.elapsed() < REVIEW_TTL);
        if self.map.len() >= MAX_REVIEWS
            && let Some(oldest) = self.map.iter().min_by_key(|(_, r)| r.issued).map(|(k, _)| k.clone())
        {
            self.map.remove(&oldest);
        }
        self.counter += 1;
        // Unpredictable per process (randomly seeded hasher) and unique (counter + time).
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(self.counter);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        h.write(plan_id.as_bytes());
        let token = format!("rv{:016x}{:x}", h.finish(), self.counter);
        self.map.insert(token.clone(), Review { plan_id: plan_id.into(), digest: digest.into(), issued: Instant::now(), attempts: 0 });
        token
    }

    /// Check the typed confirmation against the digest the token was issued for. A wrong code
    /// keeps the token for a few attempts; success consumes it.
    pub fn consume(&mut self, token: &str, plan_id: &str, digest: &str, typed: &str) -> IpcResult<()> {
        let expired = || ipc(ErrorCode::InvalidArgument, "review_expired", "This review session expired; reopen the plan to review it again");
        let Some(r) = self.map.get_mut(token) else { return Err(expired()) };
        if r.issued.elapsed() >= REVIEW_TTL || r.plan_id != plan_id {
            self.map.remove(token);
            return Err(expired());
        }
        if r.digest != digest {
            self.map.remove(token);
            return Err(ipc(ErrorCode::PlanStale, "plan_changed", "The stored plan no longer matches what was reviewed"));
        }
        if !typed.eq_ignore_ascii_case(&digest[..8]) {
            r.attempts += 1;
            if r.attempts >= MAX_REVIEW_ATTEMPTS {
                self.map.remove(token);
            }
            return Err(ipc(ErrorCode::InvalidArgument, "confirmation_mismatch", "The code does not match the first 8 characters of this plan's digest"));
        }
        self.map.remove(token);
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Candidates

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    /// Allowed and default-selected by the rule (still never pre-selected in the UI).
    Low,
    /// Allowed, review-only.
    Review,
    /// Evidence incomplete (unread descendants, unknown time).
    Pending,
    /// Blocked, protected or protected-reviewable: never selectable here.
    Blocked,
}

impl Group {
    fn parse(s: &str) -> IpcResult<Group> {
        match s {
            "low" => Ok(Group::Low),
            "review" => Ok(Group::Review),
            "pending" => Ok(Group::Pending),
            "blocked" => Ok(Group::Blocked),
            other => Err(ipc(ErrorCode::InvalidArgument, "invalid_argument", format!("unknown group {other:?}"))),
        }
    }
}

pub fn group_of(c: &Candidate) -> Group {
    match (c.state, c.tier) {
        (CandidateState::Allowed, Tier::DefaultSelected) => Group::Low,
        (CandidateState::Allowed, Tier::ReviewOnly) => Group::Review,
        (CandidateState::PendingEvidence, _) => Group::Pending,
        _ => Group::Blocked,
    }
}

/// A candidate set with the catalog facts of the analysis that produced it in this process.
/// Catalog facts (registry verdicts, live in-use probes, row kinds) are not stored with the
/// set, exactly as in the CLI: a set read back from the index has none, and nothing from it is
/// plannable until it is analyzed (and probed) again.
pub struct Loaded {
    pub set: CandidateSet,
    pub catalog: Option<CatalogSet>,
}

impl Loaded {
    pub fn facts(&self, c: &Candidate) -> Option<&CatalogFacts> {
        self.catalog.as_ref().and_then(|k| k.facts.get(&c.candidate_id))
    }
}

/// Why a candidate cannot go into a plan from this page, if anything. Mirrors the CLI's
/// `clean` refusal: state and tier, hint and owner-tool rows, live probe skips, then modes and
/// unknown sizes; without fresh catalog facts nothing is plannable (`plannable` in the core).
pub fn not_selectable(c: &Candidate, facts: Option<&CatalogFacts>) -> Option<&'static str> {
    match c.state {
        CandidateState::Blocked => return Some("blocked"),
        CandidateState::ProtectedReviewable => return Some("protected_reviewable"),
        CandidateState::PendingEvidence => return Some("pending_evidence"),
        CandidateState::Allowed if c.tier == Tier::Blocked => return Some("blocked"),
        CandidateState::Allowed => {}
    }
    if let Some(f) = facts {
        if f.is_hint() {
            return Some("hint");
        }
        if f.is_adapter_row() {
            return Some("adapter");
        }
        match f.skip {
            Some(SkipReason::InUse) => return Some("in_use"),
            Some(SkipReason::ProbeUnknown) => return Some("probe_unknown"),
            None => {}
        }
    }
    if c.allowed_modes.is_empty() {
        return Some("no_mode");
    }
    if c.logical.unknown_count > 0 {
        return Some("unknown_size");
    }
    if facts.is_none() {
        return Some("needs_reanalysis");
    }
    debug_assert!(plannable(c, facts));
    None
}

fn wire<T: Serialize>(v: &T) -> Option<String> {
    serde_json::to_value(v).ok().and_then(|j| j.as_str().map(str::to_owned))
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeDto {
    pub probe: String,
    /// `in_use`, `not_in_use` or `unknown` (unknown is never treated as not in use).
    pub state: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct GroupCount {
    pub count: u64,
    pub known_logical_bytes: String,
    pub unknown_size_count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Groups {
    pub low: GroupCount,
    pub review: GroupCount,
    pub pending: GroupCount,
    pub blocked: GroupCount,
}

fn groups(set: &CandidateSet) -> Groups {
    let mut acc: BTreeMap<u8, (u64, u128, u64)> = BTreeMap::new();
    for c in &set.candidates {
        let slot = acc.entry(group_of(c) as u8).or_default();
        slot.0 += 1;
        slot.1 += u128::from(c.logical.known.0);
        slot.2 += c.logical.unknown_count;
    }
    let get = |g: Group| {
        let (count, known, unknown) = acc.get(&(g as u8)).copied().unwrap_or_default();
        GroupCount { count, known_logical_bytes: known.to_string(), unknown_size_count: unknown }
    };
    Groups { low: get(Group::Low), review: get(Group::Review), pending: get(Group::Pending), blocked: get(Group::Blocked) }
}

#[derive(Debug, Clone, Serialize)]
pub struct CandidateSetDto {
    pub candidate_set_id: String,
    pub scan_id: String,
    pub revision: u32,
    pub created_at: Option<String>,
    pub rule_set_digest: String,
    /// The set was analyzed with the rule pack currently installed.
    pub rules_current: Option<bool>,
    pub scan_completeness: Option<String>,
    pub scan_root_display: Option<String>,
    pub groups: Groups,
    pub issues: Vec<IssueDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProtectionDto {
    pub class: &'static str,
    pub describe: &'static str,
    pub reviewable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CandidateDto {
    pub candidate_id: String,
    pub entry_id: String,
    pub name: String,
    pub path_display: String,
    pub kind: cm_core::model::ObjectKind,
    pub rules: Vec<String>,
    pub category: String,
    pub group: Group,
    pub state: CandidateState,
    pub tier: Tier,
    /// `None` when it can be planned; otherwise the reason code shown to the person.
    pub not_selectable: Option<&'static str>,
    pub reasons: Vec<String>,
    pub protection: Option<ProtectionDto>,
    pub logical_known_bytes: String,
    pub unknown_size_count: u64,
    /// Exact only when nothing inside is of unknown size; never "0" for unknown.
    pub logical_exact_bytes: Option<String>,
    pub allowed_modes: Vec<Mode>,
    pub impact: String,
    pub probes: Vec<String>,
    /// `file` (ordinary file actions), `hint` (large-item guidance, never planned) or
    /// `adapter` (cleaned by the owning tool; no file modes in this build).
    pub row_kind: &'static str,
    /// `covered`, `version_untested`, `not_registered` or `not_checked`; `None` without facts.
    pub registry_status: Option<String>,
    pub installed_version: Option<String>,
    pub rebuild_class: Option<String>,
    pub adapter_ref: Option<String>,
    /// Live in-use probe results of this analysis (empty when not probed).
    pub probe_results: Vec<ProbeDto>,
    /// Catalog facts from an analysis in this process exist for this row.
    pub catalog_fresh: bool,
}

fn candidate_dto(c: &Candidate, facts: Option<&CatalogFacts>) -> CandidateDto {
    let why = not_selectable(c, facts);
    let display = c.path.display();
    CandidateDto {
        candidate_id: c.candidate_id.clone(),
        entry_id: c.entry_id.to_string(),
        name: last_component(&display).to_owned(),
        path_display: display.clone(),
        kind: c.kind,
        rules: c.rules.iter().map(|r| format!("{}@{}", r.rule_id, r.version)).collect(),
        category: c.category.clone(),
        group: group_of(c),
        state: c.state,
        tier: c.tier,
        not_selectable: why,
        reasons: c.reasons.clone(),
        protection: c.protection.map(|p| ProtectionDto { class: p.as_str(), describe: p.describe(), reviewable: p.is_reviewable() }),
        logical_known_bytes: c.logical.known.to_string(),
        unknown_size_count: c.logical.unknown_count,
        logical_exact_bytes: c.logical.exact().map(|b| b.to_string()),
        // Protected-reviewable objects are planned only by a per-object review flow, never here.
        allowed_modes: if why.is_some() { vec![] } else { c.allowed_modes.clone() },
        impact: c.impact.clone(),
        probes: c.probes.clone(),
        row_kind: match facts {
            Some(f) if f.is_hint() => "hint",
            Some(f) if f.is_adapter_row() => "adapter",
            _ => "file",
        },
        registry_status: facts.and_then(|f| wire(&f.registry_status)),
        installed_version: facts.and_then(|f| f.installed_version.clone()),
        rebuild_class: facts.and_then(|f| f.rebuild_class.as_ref().and_then(wire)),
        adapter_ref: facts.and_then(|f| f.adapter_ref.clone()),
        probe_results: facts
            .map(|f| f.tier_inputs.probes.iter().map(|p| ProbeDto { probe: p.probe.clone(), state: wire(&p.state).unwrap_or_default(), detail: p.detail.clone() }).collect())
            .unwrap_or_default(),
        catalog_fresh: facts.is_some(),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CandidatePageDto {
    pub candidate_set_id: String,
    pub group: Group,
    pub total: u64,
    pub items: Vec<CandidateDto>,
    pub next_cursor: Option<String>,
}

fn load_set(s: &AppState, stores: &Stores, set_id: &str) -> IpcResult<Arc<Loaded>> {
    if let Some(l) = lock(&s.candidates).as_ref().filter(|l| l.set.candidate_set_id == set_id) {
        return Ok(l.clone());
    }
    let value = stores.index.candidate_set(set_id)?;
    let set: CandidateSet = serde_json::from_value(value).map_err(|e| ipc(ErrorCode::InternalError, "index_corrupt", e.to_string()))?;
    let l = Arc::new(Loaded { set, catalog: None });
    *lock(&s.candidates) = Some(l.clone());
    Ok(l)
}

fn set_dto(stores: &Stores, set: &CandidateSet, created_at: Option<String>, issues: Vec<IssueDto>, current_digest: Option<&str>) -> CandidateSetDto {
    let status = stores.index.scan_status(&set.scan_id).ok();
    CandidateSetDto {
        candidate_set_id: set.candidate_set_id.clone(),
        scan_id: set.scan_id.clone(),
        revision: set.revision,
        created_at,
        rule_set_digest: set.rule_set_digest.clone(),
        rules_current: current_digest.map(|d| d == set.rule_set_digest),
        scan_completeness: status.as_ref().and_then(|v| v.completeness.clone()),
        scan_root_display: status.as_ref().map(|v| v.root.display()),
        groups: groups(set),
        issues,
    }
}

fn issue(code: &str, scope: &str, message: impl Into<String>) -> IssueDto {
    IssueDto::new(code, scope, message)
}

fn standing_issues(l: &Loaded) -> Vec<IssueDto> {
    let set = &l.set;
    let mut out: Vec<IssueDto> = set.inapplicable.iter().map(|(rule, why)| issue("rule_inapplicable", rule, why.clone())).collect();
    match &l.catalog {
        Some(k) => out.extend(k.undetermined.iter().map(|u| issue("undetermined", &u.rule_id, format!("{} matched object(s) not judged ({}); they are not candidates", u.count, u.reason)))),
        None => out.push(issue("needs_reanalysis", "catalog", "Live checks (in use, tested versions) come from an analysis in this session; analyze again before planning")),
    }
    if set.candidates.iter().any(|c| c.state == CandidateState::ProtectedReviewable) {
        out.push(issue("protected_reviewable", "AI sessions and worktrees", "Default-protected objects are listed for review only; they are never selected here or matched by a policy"));
    }
    out
}

/// Live in-use probes once per analysis, as `clean` does (CAT-06); the executor probes each
/// item again before acting. Blocked and hint rows are not probed (never selectable anyway).
fn probe_all(candidates: &mut [Candidate], facts: &mut BTreeMap<String, CatalogFacts>) {
    use crate::plat::os::probes::{ProcessSnapshot, probe_candidate};
    let snap = ProcessSnapshot::take();
    for c in candidates.iter_mut() {
        let Some(f) = facts.get_mut(&c.candidate_id) else { continue };
        if c.state == CandidateState::Blocked || f.is_hint() || c.probes.is_empty() {
            continue;
        }
        let results = probe_candidate(&c.path, &c.probes, c.owner_id.as_deref(), &snap);
        apply_probes(c, f, results);
    }
}

/// Run the shared analysis on a published scan and store its candidate set (derived data in
/// the index; nothing on disk is touched, nothing is authorized).
pub fn analyze(s: &AppState, stores: &Stores, scan_id: &str) -> IpcResult<CandidateSetDto> {
    let status = stores.index.scan_status(scan_id)?;
    if !status.published {
        return Err(ipc(ErrorCode::InvalidArgument, "not_published", "The scan has not published results yet"));
    }
    let (rules, registry) = crate::rules::load_with_registry(s.rules_dir.as_deref()).map_err(|e| ipc(ErrorCode::InternalError, "rules_unavailable", e))?;
    let view = stores.index.view(scan_id)?;
    let hard = cm_platform::protection::hard_deny_set();
    let user = cm_platform::protection::load(&stores.dir);
    let problem = user.as_ref().err().map(ToString::to_string);
    let prot = AnalysisProtection {
        hard_deny: Some(&hard),
        user: match (&user, &problem) {
            (Ok(list), _) => UserList::Loaded(list),
            (Err(_), p) => UserList::Unavailable(p.as_deref().unwrap_or_default()),
        },
        worktrees: &[],
    };
    // The same rule-root resolution as `cleanmanager analyze`; problems resolving a root are listed with the analysis.
    let scan_root = view.path(view.root());
    let (roots, resolve_issues) = crate::plat::resolve(&rules.rules, Some(&scan_root));
    let input = AnalysisInput { scan_id, revision: status.revision, now_unix: now_unix(), roots: &roots, protected: &[] };
    // The same catalog analysis as `cleanmanager analyze` / `clean`: tested registry and
    // installed versions decide version verdicts, then live probes may lower tiers.
    let installed = crate::plat::os::resolve::installed_versions(&rules.rules);
    let ctx = CatalogContext { registry: Some(&registry), installed: Some(&installed) };
    let (mut set, mut catalog) = analyze_catalog(&view, &rules, &input, &prot, &ctx);
    probe_all(&mut set.candidates, &mut catalog.facts);
    stores.index.save_candidates(&set)?;
    let loaded = Loaded { set, catalog: Some(catalog) };
    let mut issues = standing_issues(&loaded);
    issues.extend(resolve_issues.into_iter().map(|(code, scope, message)| issue(&code, &scope, message)));
    if let Some(p) = &problem {
        issues.push(issue("protection_unavailable", cm_platform::protection::PROTECTION_FILE, format!("{p}; every candidate stays unplannable and every write is stopped until the list is repaired")));
    }
    let dto = set_dto(stores, &loaded.set, Some(now_utc()), issues, Some(&rules.digest));
    *lock(&s.candidates) = Some(Arc::new(loaded));
    Ok(dto)
}

/// The most recent stored candidate set of a scan, if any.
pub fn latest(s: &AppState, stores: &Stores, scan_id: &str) -> IpcResult<Option<CandidateSetDto>> {
    let Some((set_id, created_at)) = stores.index.latest_candidate_set(scan_id)? else { return Ok(None) };
    let l = load_set(s, stores, &set_id)?;
    let digest = crate::rules::load(s.rules_dir.as_deref()).ok().map(|r| r.digest);
    Ok(Some(set_dto(stores, &l.set, Some(created_at), standing_issues(&l), digest.as_deref())))
}

/// One page of one group: largest known size first, unknown sizes after known ones (they
/// never rank as "frees the most"), then candidate id.
pub fn page(l: &Loaded, group: Group, page_size: u32, cursor: Option<&str>) -> IpcResult<CandidatePageDto> {
    let set = &l.set;
    let size = args::page_size(Some(page_size))? as usize;
    let key = |c: &&Candidate| (c.logical.unknown_count > 0, std::cmp::Reverse(c.logical.known.0), c.candidate_id.clone());
    let mut members: Vec<&Candidate> = set.candidates.iter().filter(|c| group_of(c) == group).collect();
    members.sort_by_key(key);
    let start = match cursor {
        None => 0,
        Some(c) => {
            let (bound, offset) = c.rsplit_once(':').ok_or_else(|| ipc(ErrorCode::CursorStale, "cursor_stale", "malformed cursor"))?;
            let expect = format!("{}.{:?}.{size}", set.candidate_set_id, group);
            if bound != expect {
                return Err(ipc(ErrorCode::CursorStale, "cursor_stale", "cursor belongs to another set, group or page size"));
            }
            args::offset_cursor(Some(offset))?
        }
    };
    let end = (start + size).min(members.len());
    let items = members.get(start..end).unwrap_or_default().iter().map(|c| candidate_dto(c, l.facts(c))).collect();
    let next = (end < members.len()).then(|| format!("{}.{:?}.{size}:o{end}", set.candidate_set_id, group));
    Ok(CandidatePageDto { candidate_set_id: set.candidate_set_id.clone(), group, total: members.len() as u64, items, next_cursor: next })
}

pub(crate) fn policy_digest(stores: &Stores) -> String {
    let policies = stores.audit.policies().unwrap_or_default();
    cm_core::c14n::digest(&serde_json::to_value(&policies).expect("policies serialize")).expect("plain JSON")
}

#[derive(Debug, Clone, Serialize)]
pub struct CreatedPlanDto {
    pub plan_id: String,
    /// Selected rows whose observed state changed since discovery: listed, not planned (CLI-11). Omitted when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stale_items: Vec<StaleItemDto>,
}

impl CreatedPlanDto {
    pub fn new(plan_id: String) -> Self {
        CreatedPlanDto { plan_id, stale_items: vec![] }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StaleItemDto {
    pub candidate_id: String,
    /// `missing` or `changed`.
    pub reason: &'static str,
    pub detail: String,
}

/// Freeze explicitly chosen candidates into a stored plan. The WebView passes ids only; the
/// core refuses blocked, pending, protected, unknown-size and mode-incompatible candidates.
pub fn create(s: &AppState, stores: &Stores, set_id: &str, selected: &[String], mode: Mode) -> IpcResult<CreatedPlanDto> {
    let loaded = load_set(s, stores, set_id)?;
    let set = &loaded.set;
    // PRT-02: no plan while the protection list is unusable.
    if let Err(e) = cm_platform::protection::load(&stores.dir) {
        return Err(ipc(ErrorCode::ProtectionUnavailable, "protection_unavailable", format!("protection list unavailable: {e}")));
    }
    for id in selected {
        match set.candidates.iter().find(|c| &c.candidate_id == id) {
            None => return Err(ipc(ErrorCode::InvalidArgument, "unknown_candidate", format!("candidate {id} is not in this candidate set"))),
            Some(c) => {
                if let Some(why) = not_selectable(c, loaded.facts(c)) {
                    return Err(ipc(ErrorCode::PolicyDenied, "not_selectable", format!("candidate {id} cannot be planned ({why})")));
                }
            }
        }
    }
    let view = stores.index.view(&set.scan_id)?;
    let status = stores.index.scan_status(&set.scan_id)?;
    let plan_id = new_id("p");
    let pdigest = policy_digest(stores);
    let input = PlanInput {
        plan_id: &plan_id,
        scope_id: &status.scope_id,
        created_unix: now_unix(),
        observed_at: status.finished_at.as_deref().unwrap_or(&status.started_at),
        policy_digest: &pdigest,
    };
    let plan = build_plan(&view, set, selected, mode, &input).map_err(plan_error)?;
    bins_ready(set, selected, mode)?;
    stores.audit.save_plan(&plan)?;
    Ok(CreatedPlanDto::new(plan_id))
}

/// The core's refusal of a selection: WFA-113 `empty_selection` when nothing selected holds anything to act on (an
/// emptied folder), otherwise `plan_refused`; never a zero-action plan.
pub(crate) fn plan_error(e: cm_core::plan::PlanError) -> IpcError {
    let reason = if e == cm_core::plan::PlanError::Empty { "empty_selection" } else { "plan_refused" };
    ipc(ErrorCode::PolicyDenied, reason, e.to_string())
}

/// MTR-03 / WFA-35: trash mode needs a usable bin on every selected target's volume, checked before a cleanup plan is
/// stored; the plan is refused (naming the volumes), never switched to another mode or to a delete.
pub(crate) fn bins_ready(set: &CandidateSet, selected: &[String], mode: Mode) -> IpcResult<()> {
    use cm_platform::trash_readiness::{message, next_step, unready};
    if mode != Mode::Trash {
        return Ok(());
    }
    let paths: Vec<cm_core::path::RawPath> = set.candidates.iter().filter(|c| selected.contains(&c.candidate_id)).map(|c| c.path.clone()).collect();
    let unready = unready(&paths);
    if unready.is_empty() {
        return Ok(());
    }
    Err(ipc(ErrorCode::Unsupported, "recycle_bin_unavailable", format!("{} {}", message(&unready), next_step(&unready, Some("quarantine mode")))))
}

// ---------------------------------------------------------------------------------------------
// Plans

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AuthorizationDto {
    /// `authorized`, `required`, `expired` or `revoked`.
    pub status: &'static str,
    pub source: Option<ReceiptSource>,
    pub policy_id: Option<String>,
    pub policy_version: Option<u32>,
    pub principal: Option<String>,
    pub authorized_at: Option<String>,
    pub expires_at: Option<String>,
}

fn auth_status(valid: bool, expired: bool, revoked: bool) -> &'static str {
    match (valid, expired) {
        (true, _) => "authorized",
        (false, true) => "expired",
        (false, false) if revoked => "revoked",
        (false, false) => "required",
    }
}

pub fn cleanup_authorization(stores: &Stores, plan: &Plan, now: i64) -> IpcResult<AuthorizationDto> {
    let principal = current_principal();
    let receipts = stores.audit.receipts_for(&plan.body.plan_id)?;
    let valid = receipts.iter().find(|r| receipt_valid(r, plan, &principal, now)).cloned();
    let expired = parse_utc(&plan.body.expires_at).is_none_or(|e| now >= e);
    Ok(AuthorizationDto {
        status: auth_status(valid.is_some(), expired, receipts.iter().any(|r| r.revoked_at.is_some())),
        source: valid.as_ref().map(|r| r.source),
        policy_id: valid.as_ref().and_then(|r| r.policy_id.clone()),
        policy_version: valid.as_ref().and_then(|r| r.policy_version),
        principal: valid.as_ref().map(|r| r.principal.clone()),
        authorized_at: valid.as_ref().map(|r| r.created_at.clone()),
        expires_at: valid.as_ref().map(|r| r.expires_at.clone()),
    })
}

pub fn recovery_authorization(stores: &Stores, plan: &RecoveryPlan, now: i64) -> IpcResult<AuthorizationDto> {
    let (status, r) = cm_app::recovery::recovery_authorization(stores, plan, &current_principal(), now)?;
    Ok(AuthorizationDto {
        status,
        source: r.as_ref().map(|r| r.source),
        policy_id: None,
        policy_version: None,
        principal: r.as_ref().map(|r| r.principal.clone()),
        authorized_at: r.as_ref().map(|r| r.created_at.clone()),
        expires_at: r.as_ref().map(|r| r.expires_at.clone()),
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct PolicyMatchDto {
    pub policy_id: String,
    pub version: u32,
    pub state: PolicyState,
    /// The policy would authorize this plan now (quota is still reserved atomically at apply).
    pub covers: bool,
    /// Why not (`not_enabled`, `scope_differs`, `rule_not_covered`, `tier_not_default_selected`...).
    pub reason: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct NotIncludedDto {
    /// Candidates of the source set that are not in this plan, by group.
    pub low_unselected: u64,
    pub review_unselected: u64,
    pub pending: u64,
    pub blocked: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct OperationRefDto {
    pub operation_id: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CliDto {
    pub show: String,
    pub review: String,
    pub apply: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanDetailDto {
    pub plan_id: String,
    pub contract: String,
    /// `cleanup`, `restore` or `purge` (recovery/v1), or for plan/v2 `manual_trash`, `manual_purge`, `startup`,
    /// `startup_restore` (other v2 kinds by their raw name).
    pub kind: &'static str,
    pub mode: Option<Mode>,
    pub digest: String,
    pub digest_prefix: String,
    pub created_at: String,
    pub expires_at: String,
    pub expired: bool,
    pub seconds_left: i64,
    pub scan_id: Option<String>,
    pub scan_revision: Option<u32>,
    pub scope_id: Option<String>,
    pub scan_root_display: Option<String>,
    pub rule_refs: Vec<String>,
    pub rule_set_digest: Option<String>,
    /// `expired`, `rule_set_changed` or `digest_mismatch` when the core would refuse it now.
    pub stale_reason: Option<String>,
    pub item_count: u64,
    pub total_logical_bytes: Option<String>,
    pub allocated_known_bytes: Option<String>,
    pub allocated_unknown_count: u64,
    pub default_selected_items: u64,
    pub authorization: AuthorizationDto,
    pub policies: Vec<PolicyMatchDto>,
    pub not_included: Option<NotIncludedDto>,
    pub operation: Option<OperationRefDto>,
    /// Issued only while a human approval is still possible (`required` and not stale).
    pub review_token: Option<String>,
    pub cli: CliDto,
    /// Applying needs an elevated process (system-wide startup items); this tool never elevates itself.
    pub requires_admin: bool,
    /// `purge` or `installer` for plans from a built-in catalog, otherwise `None`.
    pub catalog: Option<&'static str>,
    /// The raw `plan_kind` of a plan/v2 plan.
    pub plan_kind_v2: Option<String>,
    /// Only a terminal review can approve it: `permanent_delete` or `end_processes` (plan/v2), otherwise `None`.
    pub terminal_only: Option<String>,
    /// Kind-specific review facts of plan/v2 plans (uninstall stage and app, maintenance elevation...).
    pub kind_detail: Option<serde_json::Value>,
}

/// Built-in catalog a rule-set digest belongs to (`purge`, `installer`).
pub fn catalog_of(rule_set_digest: &str) -> Option<&'static str> {
    if cm_core::purge::is_current_digest(rule_set_digest) {
        Some("purge")
    } else if cm_core::installer::is_current_digest(rule_set_digest) {
        Some("installer")
    } else {
        None
    }
}

/// The rule-set digest a cleanup plan is checked against: built-in catalogs (purge, installers) ship with this
/// binary and are checked by the core through the plan's own digest, as `cleanmanager plan apply` does; every other
/// plan binds to the installed rule pack.
pub fn rules_digest_for(s: &AppState, plan: &Plan) -> Result<String, String> {
    if cm_core::plan::is_builtin_catalog_digest(&plan.body.rule_set_digest) {
        return Ok(plan.body.rule_set_digest.clone());
    }
    crate::rules::load(s.rules_dir.as_deref()).map(|r| r.digest)
}

pub(crate) fn cli(plan_id: &str) -> CliDto {
    CliDto {
        show: format!("cleanmanager plan show {plan_id}"),
        review: format!("cleanmanager plan review {plan_id}"),
        apply: format!("cleanmanager plan apply {plan_id}"),
    }
}

pub(crate) fn enum_text<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|j| j.as_str().map(str::to_owned)).unwrap_or_default()
}

pub(crate) fn operation_ref(stores: &Stores, plan_id: &str, digest: &str) -> IpcResult<Option<OperationRefDto>> {
    Ok(stores.audit.operation_for_dedupe(&format!("{plan_id}#{digest}"))?.map(|(op, _)| {
        let state = stores.audit.operation_state(&op).unwrap_or_else(|_| "unknown".into());
        OperationRefDto { operation_id: op, state }
    }))
}

fn not_included(s: &AppState, stores: &Stores, plan: &Plan) -> Option<NotIncludedDto> {
    let (set_id, _) = stores.index.latest_candidate_set(&plan.body.scan_id).ok()??;
    let loaded = load_set(s, stores, &set_id).ok()?;
    let set = &loaded.set;
    let planned: std::collections::BTreeSet<&str> = plan.body.items.iter().map(|i| i.candidate_id.as_str()).collect();
    if !planned.iter().all(|id| set.candidates.iter().any(|c| c.candidate_id == *id)) {
        return None; // analyzed again since: the comparison would not be about this plan's set
    }
    let mut out = NotIncludedDto::default();
    for c in set.candidates.iter().filter(|c| !planned.contains(c.candidate_id.as_str())) {
        match group_of(c) {
            Group::Low => out.low_unselected += 1,
            Group::Review => out.review_unselected += 1,
            Group::Pending => out.pending += 1,
            Group::Blocked => out.blocked += 1,
        }
    }
    Some(out)
}

pub fn detail(s: &AppState, stores: &Stores, plan_id: &str) -> IpcResult<PlanDetailDto> {
    let contract = stores.audit.plan_contract(plan_id)?;
    let now = now_unix();
    if contract == RECOVERY_CONTRACT {
        let plan = stores.audit.load_recovery_plan(plan_id)?;
        let authorization = recovery_authorization(stores, &plan, now)?;
        let stale = cm_core::recovery::check_recovery_plan(&plan, now).err().map(|r| enum_text(&r));
        let expires = parse_utc(&plan.body.expires_at).unwrap_or(0);
        let total: u128 = plan.body.items.iter().map(|i| u128::from(i.logical_bytes.0)).sum();
        let review_token = (authorization.status == "required" && stale.is_none()).then(|| lock(&s.reviews).issue(plan_id, &plan.digest));
        return Ok(PlanDetailDto {
            plan_id: plan.body.plan_id.clone(),
            contract,
            kind: match plan.body.recovery_kind {
                RecoveryKind::Restore => "restore",
                RecoveryKind::Purge => "purge",
            },
            mode: None,
            digest_prefix: plan.digest[..8].to_owned(),
            digest: plan.digest.clone(),
            created_at: plan.body.created_at.clone(),
            expires_at: plan.body.expires_at.clone(),
            expired: now >= expires,
            seconds_left: (expires - now).max(0),
            scan_id: None,
            scan_revision: None,
            scope_id: None,
            scan_root_display: None,
            rule_refs: vec![],
            rule_set_digest: None,
            stale_reason: stale,
            item_count: plan.body.items.len() as u64,
            total_logical_bytes: Some(total.to_string()),
            allocated_known_bytes: None,
            allocated_unknown_count: 0,
            default_selected_items: 0,
            authorization,
            policies: vec![],
            not_included: None,
            operation: operation_ref(stores, plan_id, &plan.digest)?,
            review_token,
            cli: cli(plan_id),
            requires_admin: false,
            catalog: None,
            plan_kind_v2: None,
            terminal_only: None,
            kind_detail: None,
        });
    }
    if contract == PLAN_V2_CONTRACT {
        return crate::v2::detail(s, stores, plan_id, contract);
    }
    let plan = stores.audit.load_plan(plan_id)?;
    let authorization = cleanup_authorization(stores, &plan, now)?;
    let rules_digest = rules_digest_for(s, &plan);
    let stale = match &rules_digest {
        Ok(d) => check_plan(&plan, now, d).err().map(|r| enum_text(&r)),
        Err(_) => Some("rules_unavailable".to_string()),
    };
    let principal = current_principal();
    let policies = stores
        .audit
        .policies()?
        .into_iter()
        .map(|p| {
            let m = policy_covers(&p, &plan, &principal, now);
            let v = m.as_ref().err().map(|e| serde_json::to_value(e).unwrap_or_default());
            PolicyMatchDto {
                policy_id: p.policy_id.clone(),
                version: p.version,
                state: p.state,
                covers: m.is_ok(),
                reason: v.as_ref().and_then(|v| v["reason"].as_str().map(str::to_owned)),
                detail: v.as_ref().and_then(|v| v["detail"].as_str().map(str::to_owned)),
            }
        })
        .collect();
    let expires = parse_utc(&plan.body.expires_at).unwrap_or(0);
    let mut alloc_known: u128 = 0;
    let mut alloc_unknown = 0u64;
    for i in &plan.body.items {
        match i.allocated_bytes {
            Some(b) => alloc_known += u128::from(b.0),
            None => alloc_unknown += 1,
        }
    }
    let review_token = (authorization.status == "required" && stale.is_none()).then(|| lock(&s.reviews).issue(plan_id, &plan.digest));
    let scan_root_display = stores.index.scan_status(&plan.body.scan_id).ok().map(|v| v.root.display());
    Ok(PlanDetailDto {
        plan_id: plan.body.plan_id.clone(),
        contract,
        kind: "cleanup",
        mode: Some(plan.body.mode),
        digest_prefix: plan.digest[..8].to_owned(),
        digest: plan.digest.clone(),
        created_at: plan.body.created_at.clone(),
        expires_at: plan.body.expires_at.clone(),
        expired: now >= expires,
        seconds_left: (expires - now).max(0),
        scan_id: Some(plan.body.scan_id.clone()),
        scan_revision: Some(plan.body.scan_revision),
        scope_id: Some(plan.body.scope_id.clone()),
        scan_root_display,
        rule_refs: plan.rule_refs.clone(),
        rule_set_digest: Some(plan.body.rule_set_digest.clone()),
        stale_reason: stale,
        item_count: plan.body.items.len() as u64,
        total_logical_bytes: plan.body.total_logical().map(|b| b.to_string()),
        allocated_known_bytes: Some(alloc_known.to_string()),
        allocated_unknown_count: alloc_unknown,
        default_selected_items: plan.tiers.iter().filter(|t| **t == Tier::DefaultSelected).count() as u64,
        authorization,
        policies,
        not_included: not_included(s, stores, &plan),
        operation: operation_ref(stores, plan_id, &plan.digest)?,
        review_token,
        cli: cli(plan_id),
        requires_admin: false,
        catalog: catalog_of(&plan.body.rule_set_digest),
        plan_kind_v2: None,
        terminal_only: None,
        kind_detail: None,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanItemDto {
    pub action_id: String,
    pub name: String,
    pub path_display: String,
    pub logical_bytes: String,
    pub allocated_bytes: Option<String>,
    pub candidate_id: Option<String>,
    /// Recovery plans: where the object goes back to, and its quarantine record.
    pub original_path_display: Option<String>,
    pub quarantine_id: Option<String>,
    /// `file`, or `directory` (removed only when empty after its planned contents succeeded); plan/v2 also
    /// `startup_item`, `registry_value` or `other`.
    pub item_kind: String,
    /// `fs_trash`, `fs_quarantine`, `fs_delete`, `startup_disable`, `startup_enable`, `registry_restore`...;
    /// `None` for recovery/v1 items.
    pub action_type: Option<String>,
    /// Startup and registry items: `user` or `machine`.
    pub scope: Option<String>,
    /// Kind-specific facts of a plan/v2 action (update target, uninstaller, process identity, maintenance task).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanItemsDto {
    pub plan_id: String,
    pub total: u64,
    pub items: Vec<PlanItemDto>,
    pub next_cursor: Option<String>,
}

/// Every item of a stored plan, paged (the complete review list, never a top-N summary).
pub fn items(stores: &Stores, plan_id: &str, page_size: u32, cursor: Option<&str>) -> IpcResult<PlanItemsDto> {
    let size = args::page_size(Some(page_size))? as usize;
    let start = args::offset_cursor(cursor)?;
    let contract = stores.audit.plan_contract(plan_id)?;
    if contract == PLAN_V2_CONTRACT {
        let all = crate::v2::item_rows(&stores.audit.load_plan_v2(plan_id)?);
        let total = all.len() as u64;
        let mut items: Vec<PlanItemDto> = all.into_iter().skip(start).take(size + 1).collect();
        let next = (items.len() > size).then(|| {
            items.truncate(size);
            format!("o{}", start + size)
        });
        return Ok(PlanItemsDto { plan_id: plan_id.into(), total, items, next_cursor: next });
    }
    let all: Vec<PlanItemDto> = if contract == RECOVERY_CONTRACT {
        let plan = stores.audit.load_recovery_plan(plan_id)?;
        plan.body
            .items
            .iter()
            .skip(start)
            .take(size + 1)
            .map(|i| {
                let d = i.quarantined_path.display();
                let orig = i.original_path.display();
                PlanItemDto {
                    action_id: i.action_id.clone(),
                    name: last_component(&orig).to_owned(),
                    path_display: d,
                    logical_bytes: i.logical_bytes.to_string(),
                    allocated_bytes: None,
                    candidate_id: None,
                    item_kind: "file".into(),
                    original_path_display: Some(orig.clone()),
                    quarantine_id: Some(i.quarantine_id.clone()),
                    action_type: None,
                    scope: None,
                    detail: None,
                }
            })
            .collect()
    } else {
        let plan = stores.audit.load_plan(plan_id)?;
        let action_type = match plan.body.mode {
            Mode::Trash => "fs_trash",
            Mode::Quarantine => "fs_quarantine",
            Mode::Delete => "fs_delete",
        };
        plan.body
            .items
            .iter()
            .skip(start)
            .take(size + 1)
            .map(|i| {
                let d = i.path.display();
                PlanItemDto {
                    action_id: i.action_id.clone(),
                    name: last_component(&d).to_owned(),
                    path_display: d.clone(),
                    logical_bytes: i.logical_bytes.to_string(),
                    allocated_bytes: i.allocated_bytes.map(|b| b.to_string()),
                    candidate_id: Some(i.candidate_id.clone()),
                    item_kind: serde_json::to_value(i).ok().and_then(|v| v["kind"].as_str().map(str::to_owned)).unwrap_or_else(|| "file".into()),
                    original_path_display: None,
                    quarantine_id: None,
                    action_type: Some(action_type.into()),
                    scope: None,
                    detail: None,
                }
            })
            .collect()
    };
    let total = if contract == RECOVERY_CONTRACT {
        stores.audit.load_recovery_plan(plan_id)?.body.items.len()
    } else {
        stores.audit.load_plan(plan_id)?.body.items.len()
    };
    let mut items = all;
    let next = (items.len() > size).then(|| {
        items.truncate(size);
        format!("o{}", start + size)
    });
    Ok(PlanItemsDto { plan_id: plan_id.into(), total: total as u64, items, next_cursor: next })
}

/// Human approval from the review page: the same receipt `plan review` writes in a terminal.
pub fn approve(s: &AppState, stores: &Stores, plan_id: &str, token: &str, typed: &str) -> IpcResult<AuthorizationDto> {
    let now = now_unix();
    if stores.audit.plan_contract(plan_id)? == RECOVERY_CONTRACT {
        let plan = stores.audit.load_recovery_plan(plan_id)?;
        if cm_core::recovery::check_recovery_plan(&plan, now).is_err() {
            return Err(ipc(ErrorCode::PlanStale, "plan_stale", "The recovery plan expired; create a new one"));
        }
        lock(&s.reviews).consume(token, plan_id, &plan.digest, typed)?;
        let receipt = RecoveryReceipt {
            receipt_id: new_id("r"),
            plan_id: plan.body.plan_id.clone(),
            plan_digest: plan.digest.clone(),
            recovery_kind: plan.body.recovery_kind,
            principal: current_principal(),
            source: ReceiptSource::InteractiveReview,
            created_at: now_utc(),
            expires_at: plan.body.expires_at.clone(),
            revoked_at: None,
        };
        stores.audit.save_recovery_receipt(&receipt)?;
        return recovery_authorization(stores, &plan, now);
    }
    if stores.audit.plan_contract(plan_id)? == PLAN_V2_CONTRACT {
        return crate::v2::approve(s, stores, plan_id, token, typed);
    }
    let plan = stores.audit.load_plan(plan_id)?;
    let digest = rules_digest_for(s, &plan).map_err(|e| ipc(ErrorCode::InternalError, "rules_unavailable", e))?;
    if let Err(r) = check_plan(&plan, now, &digest) {
        return Err(ipc(ErrorCode::PlanStale, "plan_stale", format!("The plan can no longer be approved ({}); create a new plan", enum_text(&r))));
    }
    lock(&s.reviews).consume(token, plan_id, &plan.digest, typed)?;
    let receipt = Receipt {
        receipt_id: new_id("r"),
        plan_id: plan.body.plan_id.clone(),
        plan_digest: plan.digest.clone(),
        mode: plan.body.mode,
        principal: current_principal(),
        source: ReceiptSource::InteractiveReview,
        policy_id: None,
        policy_version: None,
        created_at: now_utc(),
        expires_at: plan.body.expires_at.clone(),
        revoked_at: None,
    };
    stores.audit.save_receipt(&receipt)?;
    cleanup_authorization(stores, &plan, now)
}

/// Revoking only restricts: later actions of a running operation are not started.
pub fn revoke(stores: &Stores, plan_id: &str) -> IpcResult<AuthorizationDto> {
    let now = now_unix();
    stores.audit.revoke_receipts(plan_id, &now_utc())?;
    let contract = stores.audit.plan_contract(plan_id)?;
    if contract == RECOVERY_CONTRACT {
        recovery_authorization(stores, &stores.audit.load_recovery_plan(plan_id)?, now)
    } else if contract == PLAN_V2_CONTRACT {
        crate::v2::authorization(stores, &stores.audit.load_plan_v2(plan_id)?, now)
    } else {
        cleanup_authorization(stores, &stores.audit.load_plan(plan_id)?, now)
    }
}

// ---------------------------------------------------------------------------------------------
// Policies

#[derive(Debug, Clone, Serialize)]
pub struct PolicyDto {
    pub policy_id: String,
    pub version: u32,
    pub state: PolicyState,
    pub expired: bool,
    pub principal: String,
    pub scope_id: String,
    pub rules: Vec<String>,
    pub mode: Mode,
    pub created_at: String,
    pub expires_at: String,
    pub single_max_bytes: String,
    pub single_max_items: u64,
    pub window_max_bytes: String,
    pub window_max_items: u64,
    pub window_used_bytes: String,
    pub window_used_items: u64,
}

pub fn policies(stores: &Stores) -> IpcResult<Vec<PolicyDto>> {
    let now = now_unix();
    Ok(stores
        .audit
        .policies()?
        .into_iter()
        .map(|p| {
            let used = stores.audit.policy_usage(&p.policy_id, now).unwrap_or(cm_store::audit::WindowUsage { bytes: 0, items: 0 });
            PolicyDto {
                expired: parse_utc(&p.expires_at).is_none_or(|e| now >= e),
                policy_id: p.policy_id,
                version: p.version,
                state: p.state,
                principal: p.principal,
                scope_id: p.scope_id,
                rules: p.rules,
                mode: p.mode,
                created_at: p.created_at,
                expires_at: p.expires_at,
                single_max_bytes: p.single_max_bytes.to_string(),
                single_max_items: p.single_max_items,
                window_max_bytes: p.window_max_bytes.to_string(),
                window_max_items: p.window_max_items,
                window_used_bytes: used.bytes.to_string(),
                window_used_items: used.items,
            }
        })
        .collect())
}

pub fn parse_policy_state(s: &str) -> IpcResult<PolicyState> {
    match s {
        "enabled" => Ok(PolicyState::Enabled),
        "paused" => Ok(PolicyState::Paused),
        "revoked" => Ok(PolicyState::Revoked),
        other => Err(ipc(ErrorCode::InvalidArgument, "invalid_argument", format!("unknown policy state {other:?}"))),
    }
}

/// Change a policy's state. Enabling grants future authority, so the person must type the
/// policy id; pausing and revoking only restrict and need no confirmation.
pub fn set_policy_state(stores: &Stores, policy_id: &str, state: PolicyState, confirm: Option<&str>) -> IpcResult<Vec<PolicyDto>> {
    let Some(mut policy) = stores.audit.policies()?.into_iter().find(|p| p.policy_id == policy_id) else {
        return Err(ipc(ErrorCode::InvalidArgument, "not_found", "Unknown policy"));
    };
    if policy.state == PolicyState::Revoked {
        return Err(ipc(ErrorCode::Conflict, "policy_revoked", "A revoked policy cannot change state; create a new one"));
    }
    if state == PolicyState::Enabled && confirm != Some(policy_id) {
        return Err(ipc(ErrorCode::InvalidArgument, "confirmation_mismatch", "Type the policy id to enable it"));
    }
    if policy.state == state {
        return policies(stores);
    }
    policy.state = state;
    policy.version += 1;
    stores.audit.save_policy(&policy, &now_utc())?;
    policies(stores)
}

/// A paused policy template covering exactly this cleanup plan's scope, rule versions and mode.
pub fn policy_from_plan(stores: &Stores, plan_id: &str) -> IpcResult<Vec<PolicyDto>> {
    let contract = stores.audit.plan_contract(plan_id)?;
    if contract == RECOVERY_CONTRACT || contract == PLAN_V2_CONTRACT {
        return Err(ipc(ErrorCode::InvalidArgument, "not_cleanup_plan", "Only cleanup plans can be covered by a policy"));
    }
    let plan = stores.audit.load_plan(plan_id)?;
    if plan.rule_refs.is_empty() {
        return Err(ipc(ErrorCode::InvalidArgument, "no_rules", "The plan names no rule versions"));
    }
    let policy = Policy::template(&new_id("pol"), &current_principal(), &plan.body.scope_id, plan.rule_refs.clone(), plan.body.mode, now_unix());
    stores.audit.save_policy(&policy, &utc_from_unix(now_unix()))?;
    policies(stores)
}

// ---------------------------------------------------------------------------------------------
// IPC

#[tauri::command(rename_all = "snake_case")]
pub async fn candidate_summary(state: St<'_>, scan_id: String) -> IpcResult<Option<CandidateSetDto>> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::scan_id(&scan_id)?;
        s.with_stores(|st| latest(&s, st, &id))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn analyze_scan(state: St<'_>, scan_id: String) -> IpcResult<CandidateSetDto> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::scan_id(&scan_id)?;
        s.with_stores(|st| analyze(&s, st, &id))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_candidates(state: St<'_>, candidate_set_id: String, group: String, page_size: Option<u32>, cursor: Option<String>) -> IpcResult<CandidatePageDto> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::record_id("candidate set", &candidate_set_id)?;
        let group = Group::parse(&group)?;
        let size = args::page_size(page_size)?;
        s.with_stores(|st| page(load_set(&s, st, &id)?.as_ref(), group, size, cursor.as_deref()))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn create_plan(state: St<'_>, candidate_set_id: String, candidate_ids: Vec<String>, mode: String) -> IpcResult<CreatedPlanDto> {
    let s = state.inner().clone();
    blocking(move || {
        let set = args::record_id("candidate set", &candidate_set_id)?;
        let ids = args::record_ids("candidate", &candidate_ids, MAX_SELECTION)?;
        let mode = args::parse_mode(&mode)?;
        s.with_stores(|st| create(&s, st, &set, &ids, mode))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn plan_detail(state: St<'_>, plan_id: String) -> IpcResult<PlanDetailDto> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::record_id("plan", &plan_id)?;
        s.with_stores(|st| detail(&s, st, &id))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn plan_items(state: St<'_>, plan_id: String, page_size: Option<u32>, cursor: Option<String>) -> IpcResult<PlanItemsDto> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::record_id("plan", &plan_id)?;
        let size = args::page_size(page_size)?;
        s.with_stores(|st| items(st, &id, size, cursor.as_deref()))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn approve_plan(state: St<'_>, plan_id: String, review_token: String, typed_code: String) -> IpcResult<AuthorizationDto> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::record_id("plan", &plan_id)?;
        let token = args::record_id("review", &review_token)?;
        let typed = args::confirmation(&typed_code)?;
        crate::gate::require_plan(&s, &id)?;
        s.with_stores(|st| approve(&s, st, &id, &token, &typed))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn revoke_plan(state: St<'_>, plan_id: String) -> IpcResult<AuthorizationDto> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::record_id("plan", &plan_id)?;
        s.with_stores(|st| revoke(st, &id))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_policies(state: St<'_>) -> IpcResult<Vec<PolicyDto>> {
    let s = state.inner().clone();
    blocking(move || s.with_stores(policies)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_policy_state_cmd(state: St<'_>, policy_id: String, new_state: String, confirm: Option<String>) -> IpcResult<Vec<PolicyDto>> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::record_id("policy", &policy_id)?;
        let st = parse_policy_state(&new_state)?;
        let confirm = confirm.as_deref().map(args::confirmation).transpose()?;
        s.with_stores(|stores| set_policy_state(stores, &id, st, confirm.as_deref()))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn create_policy_from_plan(state: St<'_>, plan_id: String) -> IpcResult<Vec<PolicyDto>> {
    let s = state.inner().clone();
    blocking(move || {
        let id = args::record_id("plan", &plan_id)?;
        s.with_stores(|st| policy_from_plan(st, &id))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_tokens_are_single_use_bound_and_bounded_in_attempts() {
        let mut r = Reviews::default();
        let digest = "3fa9c2e1".to_string() + &"0".repeat(56);
        let t = r.issue("p1", &digest);
        assert!(crate::args::record_id("review", &t).is_ok(), "{t}");
        assert_eq!(r.consume(&t, "p2", &digest, "3fa9c2e1").unwrap_err().reason, "review_expired", "bound to its plan");
        let t = r.issue("p1", &digest);
        assert_eq!(r.consume(&t, "p1", &digest, "00000000").unwrap_err().reason, "confirmation_mismatch");
        assert!(r.consume(&t, "p1", &digest, "3FA9C2E1").is_ok(), "case of hex digits does not matter");
        assert_eq!(r.consume(&t, "p1", &digest, "3fa9c2e1").unwrap_err().reason, "review_expired", "single use");
        let t = r.issue("p1", &digest);
        assert_eq!(r.consume(&t, "p1", &("f".repeat(64)), "ffffffff").unwrap_err().reason, "plan_changed");
        let t = r.issue("p1", &digest);
        for _ in 0..MAX_REVIEW_ATTEMPTS {
            let _ = r.consume(&t, "p1", &digest, "bad");
        }
        assert_eq!(r.consume(&t, "p1", &digest, "3fa9c2e1").unwrap_err().reason, "review_expired", "guessing is bounded");
        assert_ne!(r.issue("p1", &digest), r.issue("p1", &digest));
    }

    /// Built through serde so fields the core adds with defaults do not break these tests.
    fn cand(id: &str, state: CandidateState, tier: Tier, known: u64, unknown: u64) -> Candidate {
        let path = cm_core::path::RawPath::windows("C:\\Users\\a\\x".encode_utf16().collect()).unwrap();
        serde_json::from_value(serde_json::json!({
            "candidate_id": id, "entry_id": 2, "path": path, "kind": "directory",
            "rules": [{"rule_id": "r", "version": "1.0.0"}], "category": "app_cache",
            "state": state, "tier": tier, "reasons": ["matched"], "protection": null,
            "logical": cm_core::bytes::KnownTotal { known: cm_core::bytes::Bytes(known), unknown_count: unknown },
            "allowed_modes": ["quarantine", "trash"], "probes": [], "impact": "rebuilt", "owner_id": null,
        }))
        .expect("candidate shape")
    }

    fn set_of(candidates: Vec<Candidate>) -> CandidateSet {
        serde_json::from_value(serde_json::json!({
            "candidate_set_id": "cs_1", "scan_id": "s", "revision": 1, "rule_set_digest": "d", "protection_version": "v",
            "candidates": candidates, "inapplicable": [],
        }))
        .expect("set shape")
    }

    #[test]
    fn grouping_selectability_and_order_never_hide_unknown_sizes() {
        let set = set_of(vec![
                cand("c_a", CandidateState::Allowed, Tier::DefaultSelected, 10, 0),
                cand("c_b", CandidateState::Allowed, Tier::DefaultSelected, 500, 0),
                cand("c_c", CandidateState::Allowed, Tier::DefaultSelected, 9_000, 2),
                cand("c_d", CandidateState::Allowed, Tier::ReviewOnly, 7, 0),
                cand("c_e", CandidateState::PendingEvidence, Tier::ReviewOnly, 7, 1),
                cand("c_f", CandidateState::ProtectedReviewable, Tier::ReviewOnly, 7, 0),
                cand("c_g", CandidateState::Blocked, Tier::Blocked, 7, 0),
        ]);
        let g = groups(&set);
        assert_eq!((g.low.count, g.review.count, g.pending.count, g.blocked.count), (3, 1, 1, 2));
        assert_eq!(g.low.known_logical_bytes, "9510");
        let facts = set.candidates.iter().map(|c| (c.candidate_id.clone(), CatalogFacts::default())).collect();
        let set = Loaded { set, catalog: Some(CatalogSet { facts, undetermined: vec![] }) };
        assert_eq!(g.low.unknown_size_count, 2);
        let p = page(&set, Group::Low, 2, None).unwrap();
        let ids: Vec<_> = p.items.iter().map(|c| c.candidate_id.as_str()).collect();
        assert_eq!(ids, ["c_b", "c_a"], "known sizes rank first; a partly unknown total never ranks as largest");
        let p2 = page(&set, Group::Low, 2, p.next_cursor.as_deref()).unwrap();
        assert_eq!(p2.items[0].candidate_id, "c_c");
        assert_eq!(p2.items[0].not_selectable, Some("unknown_size"));
        assert_eq!(p2.items[0].logical_exact_bytes, None, "unknown never becomes an exact total");
        assert!(p2.items[0].allowed_modes.is_empty());
        assert!(page(&set, Group::Review, 2, p.next_cursor.as_deref()).is_err(), "cursor bound to its group");
        let blocked = page(&set, Group::Blocked, 10, None).unwrap();
        assert!(blocked.items.iter().all(|c| c.not_selectable.is_some() && c.allowed_modes.is_empty()));
        assert_eq!(page(&set, Group::Pending, 10, None).unwrap().items[0].not_selectable, Some("pending_evidence"));
    }

    #[test]
    fn auth_status_words_follow_the_cli() {
        assert_eq!(auth_status(true, true, true), "authorized");
        assert_eq!(auth_status(false, true, false), "expired");
        assert_eq!(auth_status(false, false, true), "revoked");
        assert_eq!(auth_status(false, false, false), "required");
    }
}

#[cfg(test)]
mod catalog_tests {
    use super::*;
    use cm_core::catalog::SkipReason;
    use cm_core::rules::ActionKind;

    fn cand(id: &str, modes: &[&str]) -> Candidate {
        let path = cm_core::path::RawPath::windows("C:\\Users\\a\\x".encode_utf16().collect()).unwrap();
        serde_json::from_value(serde_json::json!({
            "candidate_id": id, "entry_id": 2, "path": path, "kind": "directory",
            "rules": [{"rule_id": "r", "version": "1.0.0"}], "category": "app_cache",
            "state": "allowed", "tier": "review_only", "reasons": [], "protection": null,
            "logical": {"known": "5", "unknown_count": 0},
            "allowed_modes": modes, "probes": ["owner_process"], "impact": "", "owner_id": null,
        }))
        .unwrap()
    }

    #[test]
    fn catalog_rows_follow_the_cli_refusals() {
        let c = cand("c", &["trash"]);
        assert_eq!(not_selectable(&c, None), Some("needs_reanalysis"), "no live facts, nothing plannable");
        let ok = CatalogFacts::default();
        assert_eq!(not_selectable(&c, Some(&ok)), None);
        let hint = CatalogFacts { action_kind: Some(ActionKind::ReviewOnlyRow), ..CatalogFacts::default() };
        assert_eq!(not_selectable(&c, Some(&hint)), Some("hint"));
        let adapter = CatalogFacts { action_kind: Some(ActionKind::ToolAdapter), adapter_ref: Some("npm".into()), ..CatalogFacts::default() };
        assert_eq!(not_selectable(&cand("a", &[]), Some(&adapter)), Some("adapter"));
        let busy = CatalogFacts { skip: Some(SkipReason::InUse), ..CatalogFacts::default() };
        assert_eq!(not_selectable(&c, Some(&busy)), Some("in_use"));
        let unsure = CatalogFacts { skip: Some(SkipReason::ProbeUnknown), ..CatalogFacts::default() };
        assert_eq!(not_selectable(&c, Some(&unsure)), Some("probe_unknown"), "unknown is never treated as not in use");
        let d = candidate_dto(&c, Some(&adapter));
        assert_eq!((d.row_kind, d.allowed_modes.len(), d.catalog_fresh), ("adapter", 0, true));
    }
}
