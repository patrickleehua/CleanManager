//! 清理 › 清理目录 (WFA-49): the cleanup catalog `cleanmanager clean list` prints, read-only, for people.
//!
//! Safety properties of this IPC surface:
//! - One command, `clean_catalog`, takes at most a scan id. It loads the installed rule pack exactly as analysis does
//!   (`rules::load_with_registry`) and builds the categories with the shared `cm_app::clean_catalog` read model the
//!   CLI uses, so the two entries cannot drift.
//! - Where a rule looks is checked for existence only (`symlink_metadata`); nothing is listed, measured or read, and on
//!   macOS a privacy-protected location is never touched. The home folder never crosses IPC: it is written
//!   `%USERPROFILE%` / `~`.
//! - Sizes are only what a stored analysis of the named scan already found (its candidate set); a rule without
//!   candidates there has no size ("unknown"), never 0. No scan or analysis is started here.
//! - Nothing is selected, planned, authorized or changed: there is no write path behind this page. Like `clean list`,
//!   the command belongs to the gated capability `clean.catalog` (D15, `gate::capability_of`).

use std::collections::BTreeMap;

use cm_app::clean_catalog::{self, Applicability, CatalogRule, RuleEstimate, RuleLocation};
use cm_core::analysis::CandidateSet;
use cm_core::catalog::{CATALOG_VERSION, CategoryStatus};
use cm_core::error::ErrorCode;
use cm_core::rules::{Mode, Rule};
use cm_store::Stores;
use serde::Serialize;

use crate::args;
use crate::commands::{St, blocking};
use crate::dto::{IpcError, IpcResult};
use crate::host::{AppState, lock};

#[derive(Debug, Clone, Serialize)]
pub struct CatalogEntryDto {
    /// The `clean list` row, field for field.
    #[serde(flatten)]
    pub rule: CatalogRule,
    /// Objects matched below each location (`*.log`, `Code Cache`); `None` = the location's entries.
    pub pattern: Option<String>,
    pub allowed_modes: Vec<Mode>,
    pub requires_admin: bool,
    pub regeneration: String,
    pub offline_effect: String,
    /// Where it looks on this machine (home redacted), each with whether it exists; empty when nothing is known.
    pub locations: Vec<RuleLocation>,
    pub applicability: Applicability,
    /// `present`, `location_missing`, `owner_not_found`, `not_enabled`, `needs_volume`, `privacy_not_examined`,
    /// `no_location` or `category_unsupported`.
    pub applicability_reason: &'static str,
    /// What the stored analysis of the scan found for this rule; `None` = unknown (never 0).
    pub estimate: Option<RuleEstimate>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogCategoryDto {
    pub category: &'static str,
    pub label: &'static str,
    pub wave: u8,
    pub status: CategoryStatus,
    pub rules: Vec<CatalogEntryDto>,
}

/// The stored analysis sizes come from.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EstimateSourceDto {
    pub scan_id: String,
    pub candidate_set_id: String,
    pub analyzed_at: String,
    pub scan_root_display: Option<String>,
    /// The analysis used the rule pack installed now.
    pub rules_current: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CleanCatalogDto {
    pub platform: &'static str,
    pub catalog_version: &'static str,
    pub rule_set_digest: String,
    pub categories: Vec<CatalogCategoryDto>,
    /// `None`: no scan named, or it has no stored analysis; every size is then unknown.
    pub estimate_source: Option<EstimateSourceDto>,
}

/// The latest stored candidate set of `scan_id` without disturbing the cleanup page's cached analysis: the cached
/// set (with its live facts) is reused when it is that set; otherwise the stored copy is read and not cached.
fn stored_set(s: &AppState, stores: &Stores, scan_id: &str) -> IpcResult<Option<(CandidateSet, String)>> {
    let Some((set_id, created_at)) = stores.index.latest_candidate_set(scan_id)? else { return Ok(None) };
    if let Some(l) = lock(&s.candidates).as_ref().filter(|l| l.set.candidate_set_id == set_id) {
        return Ok(Some((l.set.clone(), created_at)));
    }
    let value = stores.index.candidate_set(&set_id)?;
    let set: CandidateSet = serde_json::from_value(value).map_err(|e| IpcError::new(ErrorCode::InternalError, "index_corrupt", e.to_string()))?;
    Ok(Some((set, created_at)))
}

/// Build the page's catalog. `installed` (owner versions) and `locate` (where a rule looks) are injected so tests use
/// fixtures; the command passes the platform's own.
pub fn build(
    s: &AppState, scan_id: Option<&str>, installed: &BTreeMap<String, String>, locate: &dyn Fn(&Rule) -> Option<Vec<RuleLocation>>,
) -> IpcResult<CleanCatalogDto> {
    let (rules, registry) = crate::rules::load_with_registry(s.rules_dir.as_deref()).map_err(|e| IpcError::new(ErrorCode::InternalError, "rules_unavailable", e))?;
    let platform = clean_catalog::this_platform();
    let (estimates, estimate_source) = match scan_id {
        Some(id) if s.store_dir.is_some() => s.with_stores(|st| {
            Ok(match stored_set(s, st, id)? {
                Some((set, analyzed_at)) => {
                    let source = EstimateSourceDto {
                        scan_id: set.scan_id.clone(),
                        candidate_set_id: set.candidate_set_id.clone(),
                        analyzed_at,
                        scan_root_display: st.index.scan_status(&set.scan_id).ok().map(|v| v.root.display()),
                        rules_current: set.rule_set_digest == rules.digest,
                    };
                    (clean_catalog::estimates(&set), Some(source))
                }
                None => (BTreeMap::new(), None),
            })
        })?,
        _ => (BTreeMap::new(), None),
    };
    let by_id: BTreeMap<&str, &Rule> = rules.rules.iter().map(|r| (r.rule_id.as_str(), r)).collect();
    let categories = clean_catalog::categories(&rules, &registry, installed, platform)
        .into_iter()
        .map(|c| CatalogCategoryDto {
            category: c.category,
            label: c.label,
            wave: c.wave,
            status: c.status,
            rules: c
                .rules
                .into_iter()
                .filter_map(|row| {
                    let rule = by_id.get(row.rule_id.as_str())?;
                    let locations = locate(rule);
                    let (applicability, reason) = clean_catalog::applicability(rule, c.status, locations.as_deref());
                    Some(CatalogEntryDto {
                        pattern: clean_catalog::object_pattern(rule),
                        allowed_modes: clean_catalog::modes(rule),
                        requires_admin: rule.requires_admin,
                        regeneration: rule.impact.regeneration.clone(),
                        offline_effect: rule.impact.offline_effect.clone(),
                        locations: locations.unwrap_or_default(),
                        applicability,
                        applicability_reason: reason,
                        estimate: estimates.get(&row.rule_id).cloned(),
                        rule: row,
                    })
                })
                .collect(),
        })
        .collect();
    Ok(CleanCatalogDto {
        platform: if platform == cm_core::capability::Platform::Windows { "windows" } else { "macos" },
        catalog_version: CATALOG_VERSION,
        rule_set_digest: rules.digest.clone(),
        categories,
        estimate_source,
    })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn clean_catalog(state: St<'_>, scan_id: Option<String>) -> IpcResult<CleanCatalogDto> {
    let s = state.inner().clone();
    blocking(move || {
        let id = scan_id.as_deref().map(args::scan_id).transpose()?;
        let installed = crate::rules::load(s.rules_dir.as_deref()).map(|r| crate::plat::os::resolve::installed_versions(&r.rules)).unwrap_or_default();
        build(&s, id.as_deref(), &installed, &clean_catalog::locations)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use cm_app::clean_catalog::LocationState;

    fn temp(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("cm-desk-catalog-{tag}-{}-{nanos}", std::process::id()))
    }

    const PLATFORM: &str = if cfg!(windows) { "windows" } else { "macos" };

    fn write_rule(dir: &std::path::Path, id: &str, category: &str, glob: Option<&str>) {
        let rule = serde_json::json!({
            "schema_version": "2.0", "rule_id": id, "version": "1.0.0", "status": "experimental", "platforms": [PLATFORM],
            "category": category, "max_tier": "review_only", "rebuild_class": "local_rebuild",
            "owner": {"kind": "app", "id": "slack"}, "root_resolver": "application_cache",
            "match": {"object_type": "directory", "relative_components": ["Cache"], "name_glob": glob, "extensions": [],
                      "require_cachedir_tag": false, "archive_payload": false, "max_depth": 0},
            "exclude": [], "probes": ["owner_process"],
            "thresholds": {"min_age_days": null, "age_basis": null, "min_size_bytes": null, "retention_days": null},
            "action_kind": "filesystem", "adapter_ref": null, "allowed_modes": ["trash", "quarantine"],
            "requires_admin": false, "protect_descendants": true, "follow_links": false,
            "impact": {"summary": "Fixture cache.", "regeneration": "Rebuilt on next start.", "offline_effect": "None."},
            "tested_registry_ref": null, "evidence_refs": ["run_fixture"]
        });
        std::fs::write(dir.join(format!("{id}.json")), rule.to_string()).unwrap();
    }

    fn candidate(id: &str, rule: &str, known: u64, unknown: u64) -> serde_json::Value {
        let path = if cfg!(windows) {
            serde_json::json!({"platform": "windows", "display": "C:\\a", "raw_encoding": "windows_utf16le_base64", "raw_base64": "QwA6AFwAYQA="})
        } else {
            serde_json::json!({"platform": "macos", "display": "/a", "raw_encoding": "unix_bytes_base64", "raw_base64": "L2E="})
        };
        serde_json::json!({
            "candidate_id": id, "entry_id": 1, "path": path, "kind": "directory", "rules": [{"rule_id": rule, "version": "1.0.0"}],
            "category": "app_caches", "state": "allowed", "tier": "review_only", "reasons": [], "protection": null,
            "logical": {"known": known.to_string(), "unknown_count": unknown}, "allowed_modes": ["trash"], "probes": [], "impact": "", "owner_id": null,
        })
    }

    /// Fixture rules, a fixture store and fixture locations only: nothing outside the temporary directory is read.
    #[test]
    fn catalog_lists_rules_with_locations_applicability_and_stored_sizes_only() {
        let base = temp("build");
        let rules = base.join("rules");
        let platform_dir = rules.join(PLATFORM);
        std::fs::create_dir_all(&platform_dir).unwrap();
        write_rule(&platform_dir, "fixture.cache", "app_caches", None);
        write_rule(&platform_dir, "fixture.logs", "logs", Some("*.log"));
        let store = base.join("store");
        let digest = crate::rules::load(Some(&rules)).unwrap().digest;
        let stores = Stores::open(&store).unwrap();
        let set: CandidateSet = serde_json::from_value(serde_json::json!({
            "candidate_set_id": "cs_cat", "scan_id": "s_cat", "revision": 1, "rule_set_digest": digest, "protection_version": "v",
            "candidates": [candidate("c1", "fixture.cache", 4096, 0), candidate("c2", "fixture.cache", 1000, 1)], "inapplicable": [],
        }))
        .unwrap();
        stores.index.save_candidates(&set).unwrap();
        drop(stores);

        let s = AppState::with_store(Some(store.clone()), Some(rules.clone()), None);
        let locate = |r: &Rule| -> Option<Vec<RuleLocation>> {
            match r.rule_id.as_str() {
                "fixture.cache" => Some(vec![RuleLocation { display: "%USERPROFILE%\\AppData\\Roaming\\Slack\\Cache".into(), state: LocationState::Present }]),
                _ => None,
            }
        };
        let none = BTreeMap::new();

        // Without a scan: every size is unknown.
        let plain = build(&s, None, &none, &locate).unwrap();
        assert_eq!((plain.platform, plain.catalog_version, plain.rule_set_digest.as_str()), (PLATFORM, CATALOG_VERSION, digest.as_str()));
        assert_eq!(plain.categories.len(), cm_core::catalog::CATEGORIES.len(), "every shared category, even empty ones");
        assert!(plain.estimate_source.is_none());
        let entries: Vec<&CatalogEntryDto> = plain.categories.iter().flat_map(|c| c.rules.iter()).collect();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.estimate.is_none()));

        // With the scan whose analysis is stored: sizes come from it, rules without candidates stay unknown.
        let d = build(&s, Some("s_cat"), &none, &locate).unwrap();
        let src = d.estimate_source.clone().expect("stored analysis");
        assert_eq!((src.scan_id.as_str(), src.candidate_set_id.as_str(), src.rules_current), ("s_cat", "cs_cat", true));
        let cache = d.categories.iter().find(|c| c.category == "app_caches").unwrap();
        let e = &cache.rules[0];
        assert_eq!(e.rule.rule_id, "fixture.cache");
        assert_eq!(e.estimate, Some(RuleEstimate { objects: 2, known_bytes: "5096".into(), unknown_size_count: 1 }));
        assert_eq!((e.applicability, e.applicability_reason), (Applicability::NotApplicable, "not_enabled"), "experimental rules are not used by analysis");
        assert_eq!((e.locations.len(), e.locations[0].state), (1, LocationState::Present));
        assert_eq!(e.allowed_modes, vec![Mode::Trash, Mode::Quarantine]);
        let logs = d.categories.iter().find(|c| c.category == "logs").unwrap();
        let l = &logs.rules[0];
        assert_eq!((l.pattern.as_deref(), l.estimate.as_ref()), (Some("*.log"), None), "no candidates: unknown, never 0");
        assert_eq!((l.applicability, l.applicability_reason), (Applicability::Unsupported, "no_location"));
        let empty = d.categories.iter().find(|c| c.category == "trash").unwrap();
        assert_eq!((empty.status, empty.rules.len()), (CategoryStatus::Unsupported, 0));
        // The wire form flattens the `clean list` row next to the page's facts.
        let wire = serde_json::to_value(e).unwrap();
        assert_eq!((wire["rule_id"].as_str(), wire["registry_status"].as_str(), wire["applicability"].as_str()), (Some("fixture.cache"), Some("not_registered"), Some("not_applicable")));
        assert_eq!(wire["estimate"]["known_bytes"].as_str(), Some("5096"));

        // The cleanup page's cached analysis is left alone (no facts-less copy replaces it).
        assert!(lock(&s.candidates).is_none());
        // A scan with no stored analysis, or a malformed id: unknown sizes / refused before any work.
        assert!(build(&s, Some("s_other"), &none, &locate).unwrap().estimate_source.is_none());
        assert!(args::scan_id("../x").is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_missing_rule_pack_is_an_error_not_an_empty_catalog() {
        let s = AppState::with_store(None, None, None);
        let e = build(&s, None, &BTreeMap::new(), &|_| None).unwrap_err();
        assert_eq!(e.reason, "rules_unavailable");
    }
}
