//! `clean` command group (CLI-10/11/12, CAT-01..CAT-09).
//!
//! - `clean list`: the platform catalog — categories, every rule with its status, tier ceiling,
//!   tested-registry coverage of the installed version, and the owner-tool adapters' read-only
//!   previews (`cleanup_catalog`).
//! - `clean --scan ID --dry-run`: the same analysis and live in-use probes as plan creation,
//!   returned as a `selection_preview`; nothing is changed and no authorization is consumed.
//! - `clean --scan ID --mode MODE [--candidates ID,...]`: a fixed cleanup plan. Without explicit
//!   IDs only unskipped `default_selected` rows are included (CLI-12); explicit IDs may add
//!   review-only rows but never blocked, skipped, hint or owner-tool rows. The plan still needs
//!   authorization (`plan review` or a policy) before `plan apply`.

use std::path::PathBuf;
use std::process::ExitCode;

use cm_core::adapter::{ADAPTERS, AdapterPreview, adapter};
use cm_core::analysis::{Candidate, CandidateState, CatalogFacts, apply_probes, default_selected, plannable};
use cm_core::capability::Platform;
use cm_core::catalog::{CATALOG_VERSION, CATEGORIES, CatalogContext, RegistryStatus, SkipReason, category_status};
use cm_core::error::ErrorCode;
use cm_core::plan::{PlanInput, build_plan};
use cm_core::protocol::record;
use cm_core::rules::{ActionKind, Mode, RebuildClass, Tier};
use serde_json::{Value, json};

use crate::analysis_cmd::{installed_versions, load_rules_and_registry, now_unix, run_analysis};
use crate::args::Parsed;
use crate::commands::{fields, open_stores, store_error};
use crate::{emit, fail};

const MAX_PAGE: usize = 500;

fn platform() -> Platform {
    if cfg!(windows) { Platform::Windows } else { Platform::Macos }
}

/// Wire form of a snake_case enum or plain record (this crate has no direct serde dependency).
macro_rules! snake {
    ($v:expr) => {
        serde_json::to_value($v).unwrap_or(Value::Null)
    };
}

fn registry_wire(s: RegistryStatus) -> &'static str {
    match s {
        RegistryStatus::Covered => "covered",
        RegistryStatus::VersionUntested => "version_untested",
        RegistryStatus::NotRegistered | RegistryStatus::NotChecked => "not_registered",
    }
}

#[cfg(windows)]
fn adapter_previews() -> Vec<AdapterPreview> {
    ADAPTERS.iter().map(cm_platform::windows::adapters::run_preview).collect()
}

#[cfg(not(windows))]
fn adapter_previews() -> Vec<AdapterPreview> {
    ADAPTERS.iter().map(|a| AdapterPreview::unavailable(a, "owner-tool adapters are not available on this platform in this build")).collect()
}

/// `clean list`: categories, rules and adapters as this build and machine see them.
pub fn clean_list(req: &str, p: &Parsed) -> ExitCode {
    let dir = p.raw_option("rules").map(PathBuf::from).unwrap_or_else(crate::analysis_cmd::default_rules_dir);
    let (rules, registry) = match load_rules_and_registry(&dir) {
        Ok(v) => v,
        Err(e) => return fail(req, ErrorCode::InternalError, &format!("rule set unavailable: {e}"), p.format),
    };
    let platform = platform();
    let installed = installed_versions(&rules.rules);
    let ctx = CatalogContext { registry: Some(&registry), installed: Some(&installed) };
    let categories: Vec<Value> = CATEGORIES
        .iter()
        .map(|info| {
            let rows: Vec<Value> = rules
                .rules
                .iter()
                .filter(|r| r.category == info.id && r.platforms.contains(&platform))
                .map(|r| {
                    let verdict = ctx.verdict(r);
                    let entry = r.tested_registry_ref.as_ref().and_then(|k| registry.get(k));
                    json!({
                        "rule_id": r.rule_id, "version": r.version, "status": snake!(r.status), "max_tier": snake!(r.max_tier),
                        "effective_tier_ceiling": snake!(cm_core::catalog::compute_tier(&cm_core::catalog::TierInputs {
                            rule_max: r.max_tier, dependency: cm_core::catalog::is_dependency_store(r), registry: verdict.status,
                            ..Default::default()
                        }).tier),
                        "rebuild_class": snake!(r.rebuild_class), "action_kind": snake!(r.action_kind), "adapter_ref": r.adapter_ref,
                        "owner_id": r.owner.id, "installed_version": verdict.installed,
                        "registry_status": if r.status == cm_core::rules::RuleStatus::Enabled { registry_wire(verdict.status) } else { "not_registered" },
                        "tested_range": entry.map(|e| format!("{}..{}", e.version_range.min_inclusive, e.version_range.max_exclusive.as_deref().unwrap_or(""))),
                        "tested_at": entry.map(|e| e.tested_at.clone()),
                        "evidence_refs": r.evidence_refs,
                        "summary": r.impact.summary,
                    })
                })
                .collect();
            json!({
                "category": info.id, "label": info.label, "wave": info.wave,
                "status": snake!(category_status(info, platform, &rules.rules)), "rules": rows,
            })
        })
        .collect();
    let adapters: Vec<Value> = adapter_previews().into_iter().map(|a| snake!(a)).collect();
    emit(
        &record("cleanup_catalog", req, fields(json!({
            "platform": snake!(platform),
            "catalog_version": CATALOG_VERSION,
            "rule_set_digest": rules.digest,
            "categories": categories,
            "adapters": adapters,
        }))),
        p.format,
    );
    ExitCode::SUCCESS
}

fn parse_mode(s: Option<&str>) -> Result<Option<Mode>, String> {
    match s {
        None => Ok(None),
        Some("trash") => Ok(Some(Mode::Trash)),
        Some("quarantine") => Ok(Some(Mode::Quarantine)),
        Some("delete") => Ok(Some(Mode::Delete)),
        Some(other) => Err(format!("unknown mode {other}")),
    }
}

/// Why a row cannot be selected, in the preview vocabulary (`skip_reason`).
fn skip_reason(c: &Candidate, f: Option<&CatalogFacts>) -> Option<&'static str> {
    if let Some(s) = f.and_then(|f| f.skip) {
        return Some(match s {
            SkipReason::InUse => "in_use",
            SkipReason::ProbeUnknown => "probe_unknown",
        });
    }
    match c.state {
        CandidateState::Blocked if c.reasons.iter().any(|r| r.starts_with("hard_deny:")) => Some("hard_denied"),
        CandidateState::Blocked | CandidateState::ProtectedReviewable => Some("protected"),
        CandidateState::PendingEvidence => Some("partial_evidence"),
        CandidateState::Allowed if c.tier == Tier::Blocked => Some("protected"),
        CandidateState::Allowed => None,
    }
}

/// Explicit selection refusal (CAT-02: blocked → policy_denied with the reason).
fn refusal(c: &Candidate, f: Option<&CatalogFacts>, mode: Mode) -> Option<String> {
    if f.is_some_and(|f| f.is_hint()) {
        return Some("large-item hint rows are guidance only and are never planned".into());
    }
    if f.is_some_and(|f| f.is_adapter_row()) {
        return Some("owner-tool cleanup is preview-only in this build; the directory is never deleted directly".into());
    }
    if let Some(r) = skip_reason(c, f) {
        return Some(format!("{r}: {}", c.reasons.join(", ")));
    }
    if !plannable(c, f) {
        return Some(format!("not plannable: {}", c.reasons.join(", ")));
    }
    (!c.allowed_modes.contains(&mode)).then(|| format!("mode {} is not allowed for this row", mode.as_str()))
}

fn preview_item(c: &Candidate, f: Option<&CatalogFacts>, mode: Option<Mode>, rules: &[cm_core::rules::Rule]) -> Value {
    let rule = c.rules.first();
    let probes: Vec<Value> = f
        .map(|f| f.tier_inputs.probes.iter().map(|p| json!({"probe": p.probe, "result": snake!(p.state)})).collect())
        .unwrap_or_default();
    let selected = default_selected(c, f) && mode.is_none_or(|m| c.allowed_modes.contains(&m));
    let mut impact = c.impact.clone();
    if let Some(spec) = f.and_then(|f| f.adapter_ref.as_deref()).and_then(adapter) {
        impact = format!("{impact} Cleanup is delegated to {} ({}): {}", spec.tool_name, spec.cleanup_argv.join(" "), spec.cleanup_effect);
    }
    // ANA-02: the same explanation as `candidates list` (tier reason, rebuild and offline cost, age basis, recovery).
    let explained = crate::analysis_cmd::explanation(c, rules);
    let mut item = json!({
        "object_id": c.candidate_id,
        "entry_id": c.entry_id.to_string(),
        "path": c.path,
        "category": c.category,
        "rule_id": rule.map(|r| r.rule_id.clone()),
        "rule_version": rule.map(|r| r.version.clone()),
        "tier": snake!(c.tier),
        "tier_reason": explained["tier_reason"],
        "rebuild_class": snake!(f.and_then(|f| f.rebuild_class).unwrap_or(RebuildClass::NotRebuildable)),
        "registry_status": registry_wire(f.map(|f| f.registry_status).unwrap_or(RegistryStatus::NotRegistered)),
        "selected_by_default": selected,
        "skip_reason": skip_reason(c, f),
        "probes": probes,
        "logical_bytes": c.logical.exact().map(|b| b.to_string()),
        "reclaimable_estimate_bytes": null,
        "impact": impact,
    });
    for key in ["regeneration", "offline_effect", "age_threshold", "recovery_limits"] {
        item[key] = explained[key].clone();
    }
    item
}

/// `clean --scan ID [--dry-run]`.
pub fn clean(req: &str, p: &Parsed) -> ExitCode {
    let Some(scan_id) = p.option("scan") else { return fail(req, ErrorCode::InvalidArgument, "clean needs --scan ID (scan the profile first)", p.format) };
    let mode = match parse_mode(p.option("mode")) {
        Ok(m) => m,
        Err(e) => return fail(req, ErrorCode::InvalidArgument, &e, p.format),
    };
    let dry_run = p.flag("dry-run");
    if !dry_run && mode.is_none() {
        return fail(req, ErrorCode::InvalidArgument, "clean needs --mode trash|quarantine|delete (or --dry-run for a preview)", p.format);
    }
    if dry_run && p.option("candidates").is_some() {
        return fail(req, ErrorCode::InvalidArgument, "--candidates selects plan contents; a preview always lists every row", p.format);
    }
    let volume = match p.raw_option("volume") {
        None => None,
        #[cfg(windows)]
        Some(v) => match cm_platform::windows::resolve::removable_volume_root(std::path::Path::new(v)) {
            Ok(root) => Some(root),
            Err(e) => return fail(req, ErrorCode::InvalidArgument, &format!("--volume refused: {e}"), p.format),
        },
        #[cfg(target_os = "macos")]
        Some(v) => match cm_platform::macos::resolve::removable_volume_root(std::path::Path::new(v)) {
            Ok(root) => Some(root),
            Err(e) => return fail(req, ErrorCode::InvalidArgument, &format!("--volume refused: {e}"), p.format),
        },
    };
    let page_size: usize = match p.option("limit").map(str::parse::<usize>).unwrap_or(Ok(MAX_PAGE)) {
        Ok(n) if (1..=MAX_PAGE).contains(&n) => n,
        _ => return fail(req, ErrorCode::InvalidArgument, "--limit must be 1..500", p.format),
    };
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let analysis = cm_core::trace::span("clean.analysis");
    let mut a = match run_analysis(req, p, &stores, scan_id, volume) {
        Ok(a) => a,
        Err(code) => return code,
    };
    drop(analysis);
    // Live in-use probes (CAT-06), once here; the executor probes each item again before acting.
    let probes = cm_core::trace::span("clean.probes");
    probe_all(&mut a.set.candidates, &mut a.catalog.facts);
    drop(probes);
    cm_core::debug_trace!("clean", "scan={scan_id} set={} candidates={} dry_run={dry_run} mode={mode:?}", a.set.candidate_set_id, a.set.candidates.len());
    let mut order: Vec<usize> = (0..a.set.candidates.len()).collect();
    order.sort_by(|x, y| {
        let (cx, cy) = (&a.set.candidates[*x], &a.set.candidates[*y]);
        (cx.category.as_str(), cx.path.display()).cmp(&(cy.category.as_str(), cy.path.display()))
    });
    if dry_run {
        let offset = match p.option("cursor").map(|c| c.strip_prefix(&format!("{}:", a.set.candidate_set_id)).and_then(|o| o.parse::<usize>().ok())) {
            None => 0,
            Some(Some(o)) if o <= order.len() => o,
            Some(_) => return fail(req, ErrorCode::CursorStale, "cursor does not belong to this preview; run the preview again", p.format),
        };
        let page: Vec<&Candidate> = order.iter().skip(offset).take(page_size).map(|i| &a.set.candidates[*i]).collect();
        let next = (offset + page.len() < order.len()).then(|| format!("{}:{}", a.set.candidate_set_id, offset + page.len()));
        let items: Vec<Value> = page.iter().map(|c| preview_item(c, a.catalog.facts.get(&c.candidate_id), mode, &a.rules)).collect();
        for i in items.iter().filter(|i| !i["skip_reason"].is_null()) {
            cm_core::debug_trace!("clean.skip", "{} tier={} skip_reason={}", i["object_id"], i["tier"], i["skip_reason"]);
        }
        let chosen: Vec<&Value> = items.iter().filter(|i| i["selected_by_default"] == true).collect();
        let known: u64 = chosen.iter().filter_map(|i| i["logical_bytes"].as_str()).filter_map(|b| b.parse::<u64>().ok()).sum();
        let summary = json!({
            "selected_count": chosen.len(),
            "selected_known_bytes": known.to_string(),
            "selected_unknown_size_count": chosen.iter().filter(|i| i["logical_bytes"].is_null()).count(),
            "review_only_count": items.iter().filter(|i| i["tier"] == "review_only").count(),
            "blocked_count": items.iter().filter(|i| i["tier"] == "blocked").count(),
            "skipped_count": items.iter().filter(|i| !i["skip_reason"].is_null()).count(),
        });
        let convertible = a.set.candidates.iter().any(|c| plannable(c, a.catalog.facts.get(&c.candidate_id)));
        emit(
            &record("selection_preview", req, fields(json!({
                "command": "clean.preview", "scan_id": scan_id, "scan_revision": a.status.revision, "dry_run": true,
                "non_interactive": p.flag("non-interactive"), "items": items, "next_cursor": next, "page_size": page_size,
                "summary": summary, "convertible_to_plan": convertible,
            }))),
            p.format,
        );
        return ExitCode::SUCCESS;
    }
    let mode = mode.expect("checked above");
    // PRT-02: no plan while the protection list is unusable.
    if let Some(problem) = &a.user_problem {
        return fail(req, ErrorCode::ProtectionUnavailable, &format!("protection list unavailable: {problem}"), p.format);
    }
    let facts = |c: &Candidate| a.catalog.facts.get(&c.candidate_id);
    // The live probes decide only after the re-check below: a probe of an object that changed or vanished since the
    // scan says nothing about it (WFA-50), so selection first ignores their skips.
    let unprobed = |c: &Candidate| facts(c).map(|f| CatalogFacts { skip: None, ..f.clone() });
    let candidate = |id: &str| a.set.candidates.iter().find(|c| c.candidate_id == id);
    let explicit = p.option("candidates").is_some();
    let wanted: Vec<String> = match p.option("candidates") {
        Some(list) => {
            let ids: Vec<&str> = list.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
            for id in &ids {
                let Some(c) = candidate(id) else {
                    return fail(req, ErrorCode::InvalidArgument, &format!("candidate {id} is not in this scan's catalog analysis"), p.format);
                };
                if let Some(why) = refusal(c, unprobed(c).as_ref(), mode) {
                    return fail(req, ErrorCode::PolicyDenied, &format!("candidate {id} cannot be planned: {why}"), p.format);
                }
            }
            ids.into_iter().map(str::to_owned).collect()
        }
        // CLI-12: without explicit IDs only default_selected rows, never review-only ones.
        None => order.iter().map(|i| &a.set.candidates[*i]).filter(|c| default_selected(c, unprobed(c).as_ref()) && c.allowed_modes.contains(&mode)).map(|c| c.candidate_id.clone()).collect(),
    };
    if wanted.is_empty() {
        return fail(req, ErrorCode::PolicyDenied, "nothing to plan: no row is default-selected, unskipped and allowed for this mode", p.format);
    }
    let view = match stores.index.view(scan_id) {
        Ok(v) => v,
        Err(e) => return store_error(req, &e, p.format),
    };
    // CLI-11: rows whose observed state changed since the scan (a folder: any file in it) are listed as stale, never
    // planned; the rest are planned.
    let (unchanged, stale) = split_stale(&a.set.candidates, &view, wanted);
    for s in &stale {
        cm_core::debug_trace!("clean.skip", "{} stale reason={} detail=\"{}\"", s.object_id, s.reason, s.detail);
    }
    // Live probes of the unchanged rows (CAT-06): an explicitly named row in use is refused with its reason, as before;
    // a default row in use is left out, as the preview shows it skipped.
    let mut selected = Vec::new();
    for id in unchanged {
        let Some(c) = candidate(&id) else { continue };
        match refusal(c, facts(c), mode) {
            None => selected.push(id),
            Some(why) if explicit => return fail(req, ErrorCode::PolicyDenied, &format!("candidate {id} cannot be planned: {why}"), p.format),
            Some(why) => cm_core::debug_trace!("clean.skip", "{id} {why}"),
        }
    }
    cm_core::debug_trace!("clean", "selected={} stale={} explicit_ids={explicit}", selected.len(), stale.len());
    if selected.is_empty() && !stale.is_empty() {
        return fail(req, ErrorCode::PlanStale, &format!("every selected row changed since the scan ({}); scan again and preview", stale_summary(&stale)), p.format);
    }
    if selected.is_empty() {
        return fail(req, ErrorCode::PolicyDenied, "nothing to plan: no row is default-selected, unskipped and allowed for this mode", p.format);
    }
    let policies = stores.audit.policies().unwrap_or_default();
    let pdigest = cm_core::c14n::digest(&serde_json::to_value(&policies).unwrap_or(Value::Null)).expect("plain JSON");
    let plan_id = format!("p_{}", &req[4..]);
    let input = PlanInput {
        plan_id: &plan_id,
        scope_id: &a.status.scope_id,
        created_unix: now_unix(),
        observed_at: a.status.finished_at.as_deref().unwrap_or(&a.status.started_at),
        policy_digest: &pdigest,
    };
    let mut plan = match build_plan(&view, &a.set, &selected, mode, &input) {
        Ok(plan) => plan,
        Err(e) => return fail(req, ErrorCode::PolicyDenied, &e.to_string(), p.format),
    };
    // R3-07: the rows left out stay with the plan, so `plan show` and the desktop review list them too.
    plan.excluded = stale.iter().map(Stale::excluded).collect();
    // MTR-03 / WFA-35: trash mode needs a usable bin on every target's volume; refused before the plan is stored.
    if let Err(c) = crate::trash_cmd::plan_bins_ready(req, p, &a.set, &selected, mode) {
        return c;
    }
    if let Err(e) = stores.audit.save_plan(&plan) {
        return store_error(req, &e, p.format);
    }
    emit(&with_stale(crate::plan_cmd::plan_record(req, &stores, &plan), &stale), p.format);
    ExitCode::SUCCESS
}

pub(crate) use cm_app::selection::{Stale, split_stale, stale_summary};

/// The plan record plus `stale_items` (outside the signed body, so the digest is unaffected).
pub(crate) fn with_stale(mut plan: Value, stale: &[Stale]) -> Value {
    plan["stale_items"] = Value::Array(stale.iter().map(|s| json!({"object_id": s.object_id, "reason": s.reason, "detail": s.detail})).collect());
    plan
}

/// Run each row's declared probes and recompute its tier. Blocked and hint rows are not probed
/// (they cannot be selected anyway).
fn probe_all(candidates: &mut [Candidate], facts: &mut std::collections::BTreeMap<String, CatalogFacts>) {
    #[cfg(windows)]
    let snap = cm_platform::windows::probes::ProcessSnapshot::take();
    #[cfg(target_os = "macos")]
    let snap = cm_platform::macos::probes::ProcessSnapshot::take();
    for c in candidates.iter_mut() {
        let Some(f) = facts.get_mut(&c.candidate_id) else { continue };
        if c.state == CandidateState::Blocked || f.action_kind == Some(ActionKind::ReviewOnlyRow) || c.probes.is_empty() {
            continue;
        }
        #[cfg(windows)]
        let results = cm_platform::windows::probes::probe_candidate(&c.path, &c.probes, c.owner_id.as_deref(), &snap);
        #[cfg(target_os = "macos")]
        let results = cm_platform::macos::probes::probe_candidate(&c.path, &c.probes, c.owner_id.as_deref(), &snap);
        apply_probes(c, f, results);
    }
}
