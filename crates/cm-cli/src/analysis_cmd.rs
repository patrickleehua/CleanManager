//! `analyze` and `candidates list` (ANA-*, CLI-02/06). Analysis writes only this tool's derived
//! results; it never touches targets or grants authorization. `analyze` and `clean` share
//! [`run_analysis`], so both see the same candidates and tiers for one scan revision (CAT-02).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cm_core::analysis::{
    AnalysisInput, AnalysisProtection, Candidate, CandidateSet, CandidateState, CatalogSet, Undetermined, UserList, analyze_catalog, analyze_preview_with,
};
use cm_core::catalog::CatalogContext;
use cm_core::error::ErrorCode;
use cm_core::protocol::record;
use cm_core::rules::{RegistryEntry, Rule, RuleSet, Tier};
use cm_store::Stores;
use cm_store::index::ScanStatusView;
use serde_json::{Value, json};

use crate::args::Parsed;
use crate::commands::{fields, open_stores, store_error};
use crate::{emit, fail};

/// `CM_RULES_DIR`, then `rules` next to the program; only debug builds fall back to the source tree (14.1).
pub fn default_rules_dir() -> PathBuf {
    crate::release::rules_dir()
}

fn read_json_files(dir: &Path) -> Result<Vec<(PathBuf, String)>, String> {
    // The rule pack compiled into the program (standalone installs, MVP 14.1).
    if let Some(files) = crate::release::embedded_json_files(dir) {
        return Ok(files);
    }
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return Ok(out) };
    for e in rd.filter_map(Result::ok) {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("json") {
            out.push((p.clone(), std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?));
        }
    }
    out.sort();
    Ok(out)
}

/// Platform rule pack plus tested registry; any invalid rule makes the whole set unavailable.
pub fn load_rules(dir: &Path) -> Result<RuleSet, String> {
    load_rules_and_registry(dir).map(|(r, _)| r)
}

/// [`load_rules`] keeping the tested registry for version verdicts (CAT-03).
pub fn load_rules_and_registry(dir: &Path) -> Result<(RuleSet, BTreeMap<String, RegistryEntry>), String> {
    let platform_dir = if cfg!(windows) { "windows" } else { "macos" };
    let mut registry = BTreeMap::new();
    for (p, text) in read_json_files(&dir.join("registry"))? {
        let e: RegistryEntry = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))?;
        registry.insert(e.registry_id.clone(), e);
    }
    let rules = read_json_files(&dir.join(platform_dir))?
        .into_iter()
        .map(|(p, text)| Rule::parse(&text).map_err(|e| format!("{}: {e}", p.display())))
        .collect::<Result<Vec<_>, _>>()?;
    let set = RuleSet::new(rules, &registry).map_err(|e| e.to_string())?;
    Ok((set, registry))
}

#[cfg(windows)]
fn resolver_env(volume: Option<PathBuf>) -> (cm_platform::windows::resolve::ResolverEnv, Vec<Value>) {
    let mut env = cm_platform::windows::resolve::ResolverEnv::from_os();
    let mut issues = Vec::new();
    for (var, slot) in [
        ("CM_RESOLVE_LOCALAPPDATA", &mut env.local_app_data),
        ("CM_RESOLVE_TEMP", &mut env.temp),
        ("CM_RESOLVE_APPDATA", &mut env.app_data),
        ("CM_RESOLVE_USERPROFILE", &mut env.user_profile),
        ("CM_RESOLVE_DOWNLOADS", &mut env.downloads),
        ("CM_RESOLVE_PROGRAMDATA", &mut env.program_data),
    ] {
        if let Some(v) = std::env::var_os(var) {
            issues.push(json!({"code": "resolver_override", "scope_display": var,
                "message": "rule roots resolved from an override (validation fixture), not the user's real location"}));
            *slot = Some(PathBuf::from(v));
        }
    }
    env.removable_volume = volume;
    (env, issues)
}

fn roots_for(rules: &[Rule], volume: Option<PathBuf>) -> (Vec<cm_core::analysis::ResolvedRoot>, Vec<Value>) {
    #[cfg(windows)]
    {
        let (env, issues) = resolver_env(volume);
        (cm_platform::windows::resolve::resolve(rules, &env), issues)
    }
    #[cfg(not(windows))]
    {
        let _ = (rules, volume);
        (Vec::new(), Vec::new())
    }
}

/// Installed owner versions for the registry verdicts; unknown owners are simply absent.
pub fn installed_versions(rules: &[Rule]) -> BTreeMap<String, String> {
    #[cfg(windows)]
    {
        cm_platform::windows::resolve::installed_versions(rules)
    }
    #[cfg(not(windows))]
    {
        let _ = rules;
        BTreeMap::new()
    }
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// A published scan's catalog analysis with every protection layer, the tested registry and the
/// installed versions; the candidate set is stored so a plan can be built from it.
pub struct Analysis {
    pub set: CandidateSet,
    pub catalog: CatalogSet,
    pub issues: Vec<Value>,
    pub status: ScanStatusView,
    pub user_problem: Option<String>,
}

pub fn run_analysis(req: &str, p: &Parsed, stores: &Stores, scan_id: &str, volume: Option<PathBuf>) -> Result<Analysis, ExitCode> {
    let status = match stores.index.scan_status(scan_id) {
        Ok(s) if s.published => s,
        Ok(_) => return Err(fail(req, ErrorCode::InvalidArgument, "scan is not published yet", p.format)),
        Err(e) => return Err(store_error(req, &e, p.format)),
    };
    let view = stores.index.view(scan_id).map_err(|e| store_error(req, &e, p.format))?;
    let dir = p.raw_option("rules").map(PathBuf::from).unwrap_or_else(default_rules_dir);
    let (rules, registry) = load_rules_and_registry(&dir).map_err(|e| fail(req, ErrorCode::InternalError, &format!("rule set unavailable: {e}"), p.format))?;
    // Protection layers 1 and 3 (PRT-01/02/03). An unusable user list does not stop this
    // read-only analysis; it makes every candidate unplannable and is reported as an issue.
    let hard = cm_platform::protection::hard_deny_set();
    let user = cm_platform::protection::load(&stores.dir);
    let user_problem = user.as_ref().err().map(ToString::to_string);
    let prot = AnalysisProtection {
        hard_deny: Some(&hard),
        user: match (&user, &user_problem) {
            (Ok(list), _) => UserList::Loaded(list),
            (Err(_), problem) => UserList::Unavailable(problem.as_deref().unwrap_or_default()),
        },
        // No read-only Git probe yet: every worktree root is `worktree_status_unknown` (blocked).
        worktrees: &[],
    };
    let (roots, issues) = roots_for(&rules.rules, volume);
    let installed = installed_versions(&rules.rules);
    let ctx = CatalogContext { registry: Some(&registry), installed: Some(&installed) };
    let input = AnalysisInput { scan_id, revision: status.revision, now_unix: now_unix(), roots: &roots, protected: &[] };
    let (set, catalog) = analyze_catalog(&view, &rules, &input, &prot, &ctx);
    stores.index.save_candidates(&set).map_err(|e| store_error(req, &e, p.format))?;
    Ok(Analysis { set, catalog, issues, status, user_problem })
}

/// Issues every analysis reports alongside its result.
pub fn standing_issues(set: &CandidateSet, undetermined: &[Undetermined], user_problem: Option<&str>, issues: &mut Vec<Value>) {
    for (rule, why) in &set.inapplicable {
        issues.push(json!({"code": "rule_inapplicable", "scope_display": rule, "message": why}));
    }
    for u in undetermined {
        issues.push(json!({"code": "undetermined", "scope_display": u.rule_id,
            "message": format!("{} matched object(s) not judged ({}); they are not candidates", u.count, u.reason)}));
    }
    if let Some(problem) = user_problem {
        issues.push(json!({"code": "protection_unavailable", "scope_display": cm_platform::protection::PROTECTION_FILE,
            "message": format!("{problem}; candidates stay pending and every write is stopped until the list is repaired")}));
    }
    if set.candidates.iter().any(|c| c.state == CandidateState::ProtectedReviewable) {
        issues.push(json!({"code": "protected_reviewable", "scope_display": "AI sessions and worktrees",
            "message": "default-protected objects listed for per-object review only; they are never selected automatically or matched by a policy"}));
    }
}

pub fn analyze_cmd(req: &str, p: &Parsed) -> ExitCode {
    let Some(scan_id) = p.option("scan") else { return fail(req, ErrorCode::InvalidArgument, "analyze needs --scan ID", p.format) };
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let (set, undetermined, status, mut issues, user_problem) = if let Some(preview) = p.raw_option("preview-rule") {
        let status = match stores.index.scan_status(scan_id) {
            Ok(s) if s.published => s,
            Ok(_) => return fail(req, ErrorCode::InvalidArgument, "scan is not published yet", p.format),
            Err(e) => return store_error(req, &e, p.format),
        };
        let view = match stores.index.view(scan_id) {
            Ok(v) => v,
            Err(e) => return store_error(req, &e, p.format),
        };
        let rule = match std::fs::read_to_string(preview).map_err(|e| e.to_string()).and_then(|t| Rule::parse(&t).map_err(|e| e.to_string())) {
            Ok(r) => r,
            Err(e) => return fail(req, ErrorCode::InvalidArgument, &format!("preview rule: {e}"), p.format),
        };
        let hard = cm_platform::protection::hard_deny_set();
        let user = cm_platform::protection::load(&stores.dir);
        let user_problem = user.as_ref().err().map(ToString::to_string);
        let prot = AnalysisProtection {
            hard_deny: Some(&hard),
            user: match (&user, &user_problem) {
                (Ok(list), _) => UserList::Loaded(list),
                (Err(_), problem) => UserList::Unavailable(problem.as_deref().unwrap_or_default()),
            },
            worktrees: &[],
        };
        let (roots, mut issues) = roots_for(std::slice::from_ref(&rule), None);
        issues.push(json!({"code": "experimental_preview", "scope_display": rule.rule_id,
            "message": "preview of a rule that is not enabled; results cannot be planned"}));
        let input = AnalysisInput { scan_id, revision: status.revision, now_unix: now_unix(), roots: &roots, protected: &[] };
        let set = analyze_preview_with(&view, &rule, &input, &prot);
        if let Err(e) = stores.index.save_candidates(&set) {
            return store_error(req, &e, p.format);
        }
        (set, vec![], status, issues, user_problem)
    } else {
        match run_analysis(req, p, &stores, scan_id, None) {
            Ok(a) => (a.set, a.catalog.undetermined, a.status, a.issues, a.user_problem),
            Err(code) => return code,
        }
    };
    standing_issues(&set, &undetermined, user_problem.as_deref(), &mut issues);
    let known = set.candidates.iter().fold(0u64, |a, c| a.saturating_add(c.logical.known.0));
    let unknown: u64 = set.candidates.iter().map(|c| c.logical.unknown_count).sum();
    let rule_digest = if set.rule_set_digest.len() == 64 {
        set.rule_set_digest.clone()
    } else {
        cm_core::c14n::digest(&json!(set.rule_set_digest)).expect("string digests")
    };
    emit(
        &record("analysis_result", req, fields(json!({
            "candidate_set_id": set.candidate_set_id,
            "scan_id": scan_id,
            "scan_revision": status.revision,
            "rule_set_digest": rule_digest,
            "scan_completeness": status.completeness.unwrap_or_else(|| "partial_scope".into()),
            "candidate_count": set.candidates.len(),
            "blocked_count": set.candidates.iter().filter(|c| c.state == CandidateState::Blocked).count(),
            "pending_evidence_count": set.candidates.iter().filter(|c| c.state == CandidateState::PendingEvidence).count(),
            "known_logical_bytes": known.to_string(),
            "unknown_size_count": unknown,
            "issues": issues,
        }))),
        p.format,
    );
    ExitCode::SUCCESS
}

/// Map a core candidate to the protocol `candidate_item`.
pub fn candidate_item(c: &Candidate, scan_id: &str, revision: u32) -> Value {
    let risk = match c.tier {
        Tier::DefaultSelected => "low",
        Tier::ReviewOnly => "medium",
        Tier::Blocked => "blocked",
    };
    let rule = c.rules.first();
    json!({
        "candidate_id": c.candidate_id,
        "scan_id": scan_id,
        "scan_revision": revision,
        "entry_id": c.entry_id.to_string(),
        "rule_id": rule.map(|r| r.rule_id.clone()).unwrap_or_default(),
        "rule_version": rule.map(|r| r.version.clone()).unwrap_or_default(),
        "risk": risk,
        "path": c.path,
        "reason": if c.reasons.is_empty() { "matched rule".to_string() } else { c.reasons.join(", ") },
        "impact": c.impact,
        "logical_bytes": c.logical.known.to_string(),
        "allocated_bytes": null,
        "reclaimable_estimate_bytes": null,
        "estimate_basis": if c.logical.unknown_count > 0 { "not_computed" } else { "shared_blocks_unknown" },
        // Protected-reviewable objects are planned only by a per-object review flow, never here.
        "allowed_modes": if c.tier == Tier::Blocked || c.state == CandidateState::ProtectedReviewable { vec![] } else { c.allowed_modes.clone() },
    })
}

pub fn candidates_list(req: &str, p: &Parsed) -> ExitCode {
    let Some(set_id) = p.option("set") else { return fail(req, ErrorCode::InvalidArgument, "candidates list needs --set ID", p.format) };
    let limit: u32 = match p.option("limit").map(str::parse).unwrap_or(Ok(cm_store::index::DEFAULT_PAGE)) {
        Ok(n) => n,
        Err(_) => return fail(req, ErrorCode::InvalidArgument, "--limit must be a number", p.format),
    };
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let set: CandidateSet = match stores
        .index
        .candidate_set(set_id)
        .and_then(|v| serde_json::from_value(v).map_err(|e| cm_store::StoreError::IndexCorrupt(e.to_string())))
    {
        Ok(s) => s,
        Err(e) => return store_error(req, &e, p.format),
    };
    let (page, next) = match stores.index.list_candidates(set_id, limit, p.option("cursor")) {
        Ok(v) => v,
        Err(e) => return store_error(req, &e, p.format),
    };
    let by_id: BTreeMap<&str, &Candidate> = set.candidates.iter().map(|c| (c.candidate_id.as_str(), c)).collect();
    let items: Vec<Value> = page
        .iter()
        .filter_map(|v| v["candidate_id"].as_str())
        .filter_map(|id| by_id.get(id))
        .map(|c| candidate_item(c, &set.scan_id, set.revision))
        .collect();
    emit(
        &record("candidates_page", req, fields(json!({
            "candidate_set_id": set_id, "scan_id": set.scan_id, "revision": set.revision,
            "page_size": limit, "items": items, "next_cursor": next,
        }))),
        p.format,
    );
    ExitCode::SUCCESS
}
