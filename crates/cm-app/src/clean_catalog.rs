//! The cleanup catalog as one read model for the CLI (`clean list` → `cleanup_catalog`) and the desktop's
//! read-only 清理 › 清理目录 page (WFA-49).
//!
//! - [`categories`] is pure: the shared categories with every rule of this platform, its status, tier ceiling and
//!   tested-registry verdict. `clean list` serializes it unchanged; the desktop shows the same rows.
//! - [`locations`] says where a rule looks on this machine: the fixed resolver locations plus the rule's literal
//!   components, each only checked for existence (`symlink_metadata`, nothing is listed or read; on macOS a
//!   privacy-protected location is never touched and reads `not_examined`). The home folder is shown as
//!   `%USERPROFILE%` (Windows) or `~` (macOS), never spelled out.
//! - [`applicability`] turns those facts into one of four words with a reason code.
//! - [`estimates`] sums a stored candidate set per rule: the only size this read model knows. A rule without
//!   candidates there has no estimate (unknown), never 0, and nothing is scanned to get one.
//!
//! Nothing here selects, plans or changes anything.

use std::collections::BTreeMap;

use cm_core::analysis::CandidateSet;
use cm_core::capability::Platform;
use cm_core::catalog::{CATEGORIES, CatalogContext, CategoryStatus, RegistryStatus, TierInputs, category_status, compute_tier, is_dependency_store};
use cm_core::rules::{ActionKind, Mode, RebuildClass, RegistryEntry, RootResolver, Rule, RuleSet, RuleStatus, Tier};
use serde::Serialize;

/// The platform this build runs on.
pub fn this_platform() -> Platform {
    if cfg!(windows) { Platform::Windows } else { Platform::Macos }
}

/// One rule as `clean list` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalogRule {
    pub rule_id: String,
    pub version: String,
    pub status: RuleStatus,
    pub max_tier: Tier,
    /// The highest tier any object of this rule can reach here (rule ceiling, dependency stores, registry verdict).
    pub effective_tier_ceiling: Tier,
    pub rebuild_class: RebuildClass,
    pub action_kind: ActionKind,
    pub adapter_ref: Option<String>,
    pub owner_id: String,
    pub installed_version: Option<String>,
    /// `covered`, `version_untested` or `not_registered` (every rule that is not enabled reads `not_registered`).
    pub registry_status: &'static str,
    pub tested_range: Option<String>,
    pub tested_at: Option<String>,
    pub evidence_refs: Vec<String>,
    pub summary: String,
}

/// One shared category with this platform's rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalogCategory {
    pub category: &'static str,
    pub label: &'static str,
    pub wave: u8,
    pub status: CategoryStatus,
    pub rules: Vec<CatalogRule>,
}

fn registry_wire(s: RegistryStatus) -> &'static str {
    match s {
        RegistryStatus::Covered => "covered",
        RegistryStatus::VersionUntested => "version_untested",
        RegistryStatus::NotRegistered | RegistryStatus::NotChecked => "not_registered",
    }
}

/// Every shared category, in catalog order, with the rules of `platform` (`clean list`).
pub fn categories(rules: &RuleSet, registry: &BTreeMap<String, RegistryEntry>, installed: &BTreeMap<String, String>, platform: Platform) -> Vec<CatalogCategory> {
    let ctx = CatalogContext { registry: Some(registry), installed: Some(installed) };
    CATEGORIES
        .iter()
        .map(|info| CatalogCategory {
            category: info.id,
            label: info.label,
            wave: info.wave,
            status: category_status(info, platform, &rules.rules),
            rules: rules
                .rules
                .iter()
                .filter(|r| r.category == info.id && r.platforms.contains(&platform))
                .map(|r| {
                    let verdict = ctx.verdict(r);
                    let entry = r.tested_registry_ref.as_ref().and_then(|k| registry.get(k));
                    CatalogRule {
                        rule_id: r.rule_id.clone(),
                        version: r.version.clone(),
                        status: r.status,
                        max_tier: r.max_tier,
                        effective_tier_ceiling: compute_tier(&TierInputs { rule_max: r.max_tier, dependency: is_dependency_store(r), registry: verdict.status, ..Default::default() }).tier,
                        rebuild_class: r.rebuild_class,
                        action_kind: r.action_kind,
                        adapter_ref: r.adapter_ref.clone(),
                        owner_id: r.owner.id.clone(),
                        installed_version: verdict.installed,
                        registry_status: if r.status == RuleStatus::Enabled { registry_wire(verdict.status) } else { "not_registered" },
                        tested_range: entry.map(|e| format!("{}..{}", e.version_range.min_inclusive, e.version_range.max_exclusive.as_deref().unwrap_or(""))),
                        tested_at: entry.map(|e| e.tested_at.clone()),
                        evidence_refs: r.evidence_refs.clone(),
                        summary: r.impact.summary.clone(),
                    }
                })
                .collect(),
        })
        .collect()
}

/// What the browse page found at one location (existence only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationState {
    Present,
    Missing,
    /// A macOS privacy-protected location: never opened here, so whether it exists is not known.
    NotExamined,
}

/// One place a rule looks on this machine, as shown to people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleLocation {
    /// The folder, with the home folder written `%USERPROFILE%` (Windows) or `~` (macOS).
    pub display: String,
    pub state: LocationState,
}

/// Where the rule's objects are matched below a location: the name pattern, the extensions or everything.
pub fn object_pattern(rule: &Rule) -> Option<String> {
    if let Some(g) = &rule.matcher.name_glob {
        return Some(g.clone());
    }
    if !rule.matcher.extensions.is_empty() {
        return Some(rule.matcher.extensions.iter().map(|e| format!("*.{}", e.trim_start_matches('.'))).collect::<Vec<_>>().join(" "));
    }
    None
}

/// `path` with `home` written as `marker` (case-insensitively on Windows), only at a component boundary.
pub fn redact_home(path: &str, home: Option<&str>, marker: &str, case_insensitive: bool) -> String {
    let Some(home) = home.map(|h| h.trim_end_matches(['\\', '/'])).filter(|h| !h.is_empty()) else { return path.to_owned() };
    let (Some(head), Some(rest)) = (path.get(..home.len()), path.get(home.len()..)) else { return path.to_owned() };
    let same = if case_insensitive { head.eq_ignore_ascii_case(home) } else { head == home };
    if same && (rest.is_empty() || rest.starts_with(['\\', '/'])) { format!("{marker}{rest}") } else { path.to_owned() }
}

fn with_components(base: &std::path::Path, rule: &Rule) -> std::path::PathBuf {
    rule.matcher.relative_components.iter().fold(base.to_path_buf(), |p, c| p.join(c))
}

fn exists(p: &std::path::Path) -> LocationState {
    if std::fs::symlink_metadata(p).is_ok() { LocationState::Present } else { LocationState::Missing }
}

/// Where `rule` looks on this machine (Windows): `None` when this build has no location for its resolver here.
#[cfg(windows)]
pub fn locations(rule: &Rule) -> Option<Vec<RuleLocation>> {
    locations_in(rule, &cm_platform::windows::resolve::ResolverEnv::from_os())
}

/// [`locations`] with an explicit resolver environment (fixtures).
#[cfg(windows)]
pub fn locations_in(rule: &Rule, env: &cm_platform::windows::resolve::ResolverEnv) -> Option<Vec<RuleLocation>> {
    let bases = cm_platform::windows::resolve::base_paths(rule, env);
    if bases.is_empty() {
        // A browser without profiles or a volume nobody chose: known places, nothing there.
        return matches!(rule.root_resolver, RootResolver::BrowserProfile | RootResolver::RemovableVolumeRoot).then(Vec::new);
    }
    let home = env.user_profile.as_ref().map(|h| h.to_string_lossy().into_owned());
    Some(
        bases
            .iter()
            .map(|b| {
                let p = with_components(b, rule);
                RuleLocation { display: redact_home(&p.to_string_lossy(), home.as_deref(), "%USERPROFILE%", true), state: exists(&p) }
            })
            .collect(),
    )
}

/// Where `rule` looks on this Mac: `None` when this build has no location for its resolver here. A location that
/// macOS privacy controls guard is `not_examined`: this page never opens it, so it never raises a prompt.
#[cfg(target_os = "macos")]
pub fn locations(rule: &Rule) -> Option<Vec<RuleLocation>> {
    locations_in(rule, &cm_platform::macos::resolve::ResolverEnv::from_os())
}

/// [`locations`] with an explicit resolver environment (fixtures).
#[cfg(target_os = "macos")]
pub fn locations_in(rule: &Rule, env: &cm_platform::macos::resolve::ResolverEnv) -> Option<Vec<RuleLocation>> {
    use cm_platform::macos::privacy;
    let targets = cm_platform::macos::resolve::targets(rule, env);
    if targets.is_empty() {
        return matches!(rule.root_resolver, RootResolver::BrowserProfile | RootResolver::RemovableVolumeRoot).then(Vec::new);
    }
    let home = env.home.as_deref();
    let home_text = home.map(|h| h.to_string_lossy().into_owned());
    Some(
        targets
            .iter()
            .map(|t| {
                let p = with_components(&t.path, rule);
                let state = if t.tcc_protected || privacy::classify(&p, home).is_some() { LocationState::NotExamined } else { exists(&p) };
                RuleLocation { display: redact_home(&p.to_string_lossy(), home_text.as_deref(), "~", false), state }
            })
            .collect(),
    )
}

/// Other platforms: no resolver, nothing applies.
#[cfg(not(any(windows, target_os = "macos")))]
pub fn locations(_rule: &Rule) -> Option<Vec<RuleLocation>> {
    None
}

/// Whether a rule takes part in cleanup on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Applicability {
    /// Enabled and at least one of its locations exists here.
    Applicable,
    /// None of its locations exists here (the owning app or tool is not installed, or nothing was written yet).
    NotPresent,
    /// Not used by cleanup analysis here (not enabled yet, needs a volume you choose, or privacy-protected).
    NotApplicable,
    /// This build has no rule location for it on this platform.
    Unsupported,
}

/// The verdict and its reason code: `category_unsupported`, `no_location`, `needs_volume`, `not_enabled`, `present`,
/// `privacy_not_examined`, `owner_not_found` or `location_missing`.
pub fn applicability(rule: &Rule, category: CategoryStatus, locations: Option<&[RuleLocation]>) -> (Applicability, &'static str) {
    if category == CategoryStatus::Unsupported {
        return (Applicability::Unsupported, "category_unsupported");
    }
    if rule.root_resolver == RootResolver::RemovableVolumeRoot {
        return (Applicability::NotApplicable, "needs_volume");
    }
    let Some(locs) = locations else { return (Applicability::Unsupported, "no_location") };
    if rule.status != RuleStatus::Enabled {
        return (Applicability::NotApplicable, "not_enabled");
    }
    if locs.iter().any(|l| l.state == LocationState::Present) {
        (Applicability::Applicable, "present")
    } else if locs.iter().any(|l| l.state == LocationState::NotExamined) {
        (Applicability::NotApplicable, "privacy_not_examined")
    } else if locs.is_empty() {
        (Applicability::NotPresent, "owner_not_found")
    } else {
        (Applicability::NotPresent, "location_missing")
    }
}

/// The stored size estimate of one rule: what its candidates in a saved analysis add up to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RuleEstimate {
    pub objects: u64,
    /// Decimal bytes of the objects whose size is known.
    pub known_bytes: String,
    /// Objects (or parts of them) of unknown size; the known bytes are then "at least".
    pub unknown_size_count: u64,
}

/// Per-rule totals of a stored candidate set. Rules without candidates are absent (unknown, never 0).
pub fn estimates(set: &CandidateSet) -> BTreeMap<String, RuleEstimate> {
    let mut acc: BTreeMap<String, (u64, u128, u64)> = BTreeMap::new();
    for c in &set.candidates {
        let mut seen: Vec<&str> = Vec::new();
        for r in &c.rules {
            if seen.contains(&r.rule_id.as_str()) {
                continue;
            }
            seen.push(&r.rule_id);
            let slot = acc.entry(r.rule_id.clone()).or_default();
            slot.0 += 1;
            slot.1 += u128::from(c.logical.known.0);
            slot.2 += c.logical.unknown_count;
        }
    }
    acc.into_iter().map(|(id, (objects, known, unknown))| (id, RuleEstimate { objects, known_bytes: known.to_string(), unknown_size_count: unknown })).collect()
}

/// Modes a rule allows, in protocol order (for display next to the recovery words).
pub fn modes(rule: &Rule) -> Vec<Mode> {
    let mut m = rule.allowed_modes.clone();
    m.sort();
    m.dedup();
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str, status: &str, resolver: &str, owner: &str, components: &[&str], glob: Option<&str>) -> Rule {
        let platform = if cfg!(windows) { "windows" } else { "macos" };
        Rule::parse(
            &serde_json::json!({
                "schema_version": "2.0", "rule_id": id, "version": "1.0.0", "status": status, "platforms": [platform],
                "category": "app_caches", "max_tier": "review_only", "rebuild_class": "local_rebuild",
                "owner": {"kind": "app", "id": owner}, "root_resolver": resolver,
                "match": {"object_type": "directory", "relative_components": components, "name_glob": glob, "extensions": [],
                          "require_cachedir_tag": false, "archive_payload": false, "max_depth": 0},
                "exclude": [], "probes": ["owner_process"],
                "thresholds": {"min_age_days": null, "age_basis": null, "min_size_bytes": null, "retention_days": null},
                "action_kind": "filesystem", "adapter_ref": null, "allowed_modes": ["quarantine", "trash"],
                "requires_admin": false, "protect_descendants": true, "follow_links": false,
                "impact": {"summary": "Test cache.", "regeneration": "Rebuilt.", "offline_effect": "None."},
                "tested_registry_ref": null, "evidence_refs": ["run_test"]
            })
            .to_string(),
        )
        .expect("test rule parses")
    }

    #[test]
    fn categories_list_every_shared_category_with_this_platforms_rules() {
        let set = RuleSet::new(vec![rule("test.cache", "experimental", "application_cache", "slack", &["Cache"], None)], &BTreeMap::new()).unwrap();
        let cats = categories(&set, &BTreeMap::new(), &BTreeMap::new(), this_platform());
        assert_eq!(cats.len(), CATEGORIES.len());
        let app = cats.iter().find(|c| c.category == "app_caches").unwrap();
        assert_eq!(app.status, CategoryStatus::Unverified, "only experimental rules: unverified");
        assert_eq!(app.rules.len(), 1);
        let r = &app.rules[0];
        assert_eq!((r.registry_status, r.effective_tier_ceiling, r.summary.as_str()), ("not_registered", Tier::ReviewOnly, "Test cache."));
        assert!(cats.iter().filter(|c| c.category != "app_caches").all(|c| c.rules.is_empty() && c.status == CategoryStatus::Unsupported));
        // The wire form `clean list` emits.
        let wire = serde_json::to_value(r).unwrap();
        assert_eq!((wire["status"].as_str(), wire["max_tier"].as_str(), wire["rebuild_class"].as_str()), (Some("experimental"), Some("review_only"), Some("local_rebuild")));
        let other = categories(&set, &BTreeMap::new(), &BTreeMap::new(), if cfg!(windows) { Platform::Macos } else { Platform::Windows });
        assert!(other.iter().all(|c| c.rules.is_empty()), "another platform's rules are not listed");
    }

    #[test]
    fn applicability_follows_category_resolver_status_and_presence() {
        let on = rule("test.on", "enabled", "application_cache", "slack", &[], None);
        let off = rule("test.off", "experimental", "application_cache", "slack", &[], None);
        let vol = rule("test.vol", "enabled", "removable_volume_root", "apple", &[], Some(".DS_Store"));
        let loc = |s| vec![RuleLocation { display: "%USERPROFILE%\\x".into(), state: s }];
        let present = loc(LocationState::Present);
        let missing = loc(LocationState::Missing);
        let hidden = loc(LocationState::NotExamined);
        assert_eq!(applicability(&on, CategoryStatus::Unsupported, Some(&present)), (Applicability::Unsupported, "category_unsupported"));
        assert_eq!(applicability(&vol, CategoryStatus::Available, Some(&[])), (Applicability::NotApplicable, "needs_volume"));
        assert_eq!(applicability(&on, CategoryStatus::Available, None), (Applicability::Unsupported, "no_location"));
        assert_eq!(applicability(&off, CategoryStatus::Unverified, Some(&present)), (Applicability::NotApplicable, "not_enabled"));
        assert_eq!(applicability(&on, CategoryStatus::Available, Some(&present)), (Applicability::Applicable, "present"));
        assert_eq!(applicability(&on, CategoryStatus::Available, Some(&missing)), (Applicability::NotPresent, "location_missing"));
        assert_eq!(applicability(&on, CategoryStatus::Available, Some(&[])), (Applicability::NotPresent, "owner_not_found"));
        assert_eq!(applicability(&on, CategoryStatus::Available, Some(&hidden)), (Applicability::NotApplicable, "privacy_not_examined"));
    }

    #[test]
    fn home_is_never_spelled_out() {
        assert_eq!(redact_home("C:\\Users\\Ann\\AppData\\Local\\x", Some("c:\\users\\ann"), "%USERPROFILE%", true), "%USERPROFILE%\\AppData\\Local\\x");
        assert_eq!(redact_home("C:\\Users\\Anna\\x", Some("C:\\Users\\Ann"), "%USERPROFILE%", true), "C:\\Users\\Anna\\x", "component boundary only");
        assert_eq!(redact_home("/Users/ann/Library/Caches", Some("/Users/ann/"), "~", false), "~/Library/Caches");
        assert_eq!(redact_home("/Users/Ann/x", Some("/Users/ann"), "~", false), "/Users/Ann/x", "case matters on macOS");
        assert_eq!(redact_home("/private/var/log", None, "~", false), "/private/var/log");
    }

    #[test]
    fn estimates_sum_stored_candidates_per_rule_and_leave_others_unknown() {
        let set: CandidateSet = serde_json::from_value(serde_json::json!({
            "candidate_set_id": "cs", "scan_id": "s", "revision": 1, "rule_set_digest": "d", "protection_version": "v",
            "candidates": [
                {"candidate_id": "a", "entry_id": 1, "path": {"platform": "windows", "display": "C:\\a", "raw_encoding": "windows_utf16le_base64", "raw_base64": "QwA6AFwAYQA="},
                 "kind": "directory", "rules": [{"rule_id": "r.one", "version": "1"}, {"rule_id": "r.one", "version": "1"}], "category": "app_caches",
                 "state": "allowed", "tier": "review_only", "reasons": [], "protection": null, "logical": {"known": "1000", "unknown_count": 0},
                 "allowed_modes": ["trash"], "probes": [], "impact": "", "owner_id": null},
                {"candidate_id": "b", "entry_id": 2, "path": {"platform": "windows", "display": "C:\\b", "raw_encoding": "windows_utf16le_base64", "raw_base64": "QwA6AFwAYgA="},
                 "kind": "directory", "rules": [{"rule_id": "r.one", "version": "1"}, {"rule_id": "r.two", "version": "1"}], "category": "app_caches",
                 "state": "blocked", "tier": "blocked", "reasons": [], "protection": null, "logical": {"known": "24", "unknown_count": 2},
                 "allowed_modes": [], "probes": [], "impact": "", "owner_id": null}
            ],
            "inapplicable": [["r.three", "no resolved root is inside this scan"]]
        }))
        .unwrap();
        let e = estimates(&set);
        assert_eq!(e["r.one"], RuleEstimate { objects: 2, known_bytes: "1024".into(), unknown_size_count: 2 });
        assert_eq!(e["r.two"], RuleEstimate { objects: 1, known_bytes: "24".into(), unknown_size_count: 2 });
        assert!(!e.contains_key("r.three"), "no candidates: no estimate (unknown), never 0");
    }

    #[cfg(windows)]
    #[test]
    fn windows_locations_check_existence_below_fixture_bases_only() {
        let base = std::env::temp_dir().join(format!("cm-app-catalog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let local = base.join("profile").join("AppData").join("Local");
        std::fs::create_dir_all(local.join("Python").join("pip")).unwrap();
        let env = cm_platform::windows::resolve::ResolverEnv {
            local_app_data: Some(local.clone()),
            temp: None,
            app_data: Some(base.join("profile").join("AppData").join("Roaming")),
            user_profile: Some(base.join("profile")),
            downloads: None,
            program_data: None,
            tool_env: BTreeMap::new(),
            removable_volume: None,
        };
        let there = rule("t.py", "enabled", "application_cache", "python.install-manager", &["pip"], None);
        let l = locations_in(&there, &env).expect("mapped");
        assert_eq!(l, vec![RuleLocation { display: "%USERPROFILE%\\AppData\\Local\\Python\\pip".into(), state: LocationState::Present }]);
        let absent = rule("t.slack", "enabled", "application_cache", "slack", &["Cache"], None);
        assert_eq!(locations_in(&absent, &env).unwrap()[0].state, LocationState::Missing);
        let unmapped = rule("t.none", "enabled", "application_cache", "nobody.knows", &[], None);
        assert_eq!(locations_in(&unmapped, &env), None, "no location for this owner in this build");
        let browser = rule("t.browser", "enabled", "browser_profile", "google.chrome", &["Code Cache"], None);
        assert_eq!(locations_in(&browser, &env), Some(vec![]), "no profiles: nothing there");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_locations_never_touch_privacy_protected_places() {
        let base = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("cm-app-catalog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let home = base.join("home");
        std::fs::create_dir_all(home.join("Library").join("Application Support").join("Slack").join("Cache")).unwrap();
        let env = cm_platform::macos::resolve::ResolverEnv { home: Some(home.clone()), downloads: Some(home.join("Downloads")), ..Default::default() };
        let there = rule("t.slack", "enabled", "application_cache", "slack", &["Cache"], None);
        assert_eq!(locations_in(&there, &env).unwrap(), vec![RuleLocation { display: "~/Library/Application Support/Slack/Cache".into(), state: LocationState::Present }]);
        let tcc = rule("t.teams", "enabled", "app_container", "microsoft.teams", &[], None);
        assert!(locations_in(&tcc, &env).unwrap().iter().all(|l| l.state == LocationState::NotExamined));
        let downloads = rule("t.dl", "enabled", "installer_location", "apple", &[], None);
        assert!(locations_in(&downloads, &env).unwrap().iter().all(|l| l.state == LocationState::NotExamined), "Downloads is a Files and Folders location");
        let _ = std::fs::remove_dir_all(&base);
    }
}
