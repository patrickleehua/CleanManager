//! `analyze` and `candidates list` (ANA-*, CLI-02/06). Analysis writes only this tool's derived
//! results; it never touches targets or grants authorization.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cm_core::analysis::{AnalysisInput, Candidate, CandidateSet, CandidateState, analyze, analyze_preview};
use cm_core::error::ErrorCode;
use cm_core::protocol::record;
use cm_core::rules::{RegistryEntry, Rule, RuleSet, Tier};
use serde_json::{Value, json};

use crate::args::Parsed;
use crate::commands::{fields, open_stores, store_error};
use crate::{emit, fail};

pub fn default_rules_dir() -> PathBuf {
    std::env::var_os("CM_RULES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rules"))
}

fn read_json_files(dir: &Path) -> Result<Vec<(PathBuf, String)>, String> {
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
    RuleSet::new(rules, &registry).map_err(|e| e.to_string())
}

#[cfg(windows)]
fn resolver_env() -> (cm_platform::windows::resolve::ResolverEnv, Vec<Value>) {
    let mut env = cm_platform::windows::resolve::ResolverEnv::from_os();
    let mut issues = Vec::new();
    for (var, slot) in [("CM_RESOLVE_LOCALAPPDATA", &mut env.local_app_data), ("CM_RESOLVE_TEMP", &mut env.temp)] {
        if let Some(v) = std::env::var_os(var) {
            issues.push(json!({"code": "resolver_override", "scope_display": var,
                "message": "rule roots resolved from an override (validation fixture), not the user's real location"}));
            *slot = Some(PathBuf::from(v));
        }
    }
    (env, issues)
}

fn roots_for(rules: &[Rule]) -> (Vec<cm_core::analysis::ResolvedRoot>, Vec<Value>) {
    #[cfg(windows)]
    {
        let (env, issues) = resolver_env();
        (cm_platform::windows::resolve::resolve(rules, &env), issues)
    }
    #[cfg(not(windows))]
    {
        let _ = rules;
        (Vec::new(), Vec::new())
    }
}

pub fn analyze_cmd(req: &str, p: &Parsed) -> ExitCode {
    let Some(scan_id) = p.option("scan") else { return fail(req, ErrorCode::InvalidArgument, "analyze needs --scan ID", p.format) };
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let status = match stores.index.scan_status(scan_id) {
        Ok(s) if s.published => s,
        Ok(_) => return fail(req, ErrorCode::InvalidArgument, "scan is not published yet", p.format),
        Err(e) => return store_error(req, &e, p.format),
    };
    let view = match stores.index.view(scan_id) {
        Ok(v) => v,
        Err(e) => return store_error(req, &e, p.format),
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (set, mut issues): (CandidateSet, Vec<Value>) = if let Some(preview) = p.raw_option("preview-rule") {
        let rule = match std::fs::read_to_string(preview).map_err(|e| e.to_string()).and_then(|t| Rule::parse(&t).map_err(|e| e.to_string())) {
            Ok(r) => r,
            Err(e) => return fail(req, ErrorCode::InvalidArgument, &format!("preview rule: {e}"), p.format),
        };
        let (roots, mut issues) = roots_for(std::slice::from_ref(&rule));
        issues.push(json!({"code": "experimental_preview", "scope_display": rule.rule_id,
            "message": "preview of a rule that is not enabled; results cannot be planned"}));
        let input = AnalysisInput { scan_id, revision: status.revision, now_unix: now, roots: &roots, protected: &[] };
        (analyze_preview(&view, &rule, &input), issues)
    } else {
        let dir = p.raw_option("rules").map(PathBuf::from).unwrap_or_else(default_rules_dir);
        let rules = match load_rules(&dir) {
            Ok(r) => r,
            Err(e) => return fail(req, ErrorCode::InternalError, &format!("rule set unavailable: {e}"), p.format),
        };
        let (roots, issues) = roots_for(&rules.rules);
        let input = AnalysisInput { scan_id, revision: status.revision, now_unix: now, roots: &roots, protected: &[] };
        (analyze(&view, &rules, &input), issues)
    };
    if let Err(e) = stores.index.save_candidates(&set) {
        return store_error(req, &e, p.format);
    }
    for (rule, why) in &set.inapplicable {
        issues.push(json!({"code": "rule_inapplicable", "scope_display": rule, "message": why}));
    }
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
        "allowed_modes": if c.tier == Tier::Blocked { vec![] } else { c.allowed_modes.clone() },
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
