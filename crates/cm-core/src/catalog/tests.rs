use super::*;
use crate::rules::{ActionKind, Fixtures, Impact, Match, ObjectType, Owner, OwnerKind, RegistryPlatform, RegistrySubject, Thresholds};

fn rule(id: &str, resolver: RootResolver, rebuild: RebuildClass, max: Tier) -> Rule {
    Rule {
        schema_version: "2.0".into(),
        rule_id: id.into(),
        version: "1.0.0".into(),
        status: RuleStatus::Enabled,
        platforms: vec![Platform::Windows],
        category: "developer".into(),
        max_tier: max,
        rebuild_class: rebuild,
        owner: Owner { kind: OwnerKind::App, id: "google.chrome".into() },
        root_resolver: resolver,
        matcher: Match {
            object_type: ObjectType::Directory,
            relative_components: vec!["Cache".into()],
            name_glob: None,
            extensions: vec![],
            require_cachedir_tag: false,
            archive_payload: false,
            max_depth: 0,
        },
        exclude: vec![],
        probes: vec![],
        thresholds: Thresholds { min_age_days: None, age_basis: None, min_size_bytes: None, retention_days: None },
        action_kind: ActionKind::Filesystem,
        adapter_ref: None,
        allowed_modes: vec![],
        requires_admin: false,
        protect_descendants: true,
        follow_links: false,
        impact: Impact { summary: "x".into(), regeneration: "x".into(), offline_effect: "x".into() },
        tested_registry_ref: Some(format!("{id}@1.0.0")),
        evidence_refs: vec!["run".into()],
    }
}

fn entry(id: &str, min: &str, max: Option<&str>) -> RegistryEntry {
    RegistryEntry {
        schema_version: "1.0".into(),
        registry_id: format!("{id}@1.0.0"),
        rule_id: id.into(),
        rule_version: "1.0.0".into(),
        synthetic: false,
        subject: RegistrySubject { kind: OwnerKind::App, id: "google.chrome".into(), name: "Chrome".into() },
        version_range: VersionRange { min_inclusive: min.into(), max_exclusive: max.map(str::to_owned) },
        platforms: vec![RegistryPlatform { platform: Platform::Windows, os_versions: vec!["10.0.26200".into()], architectures: vec!["x64".into()] }],
        tested_at: "2026-10-15".into(),
        result: "passed".into(),
        fixtures: Fixtures { positive: vec!["p".into()], negative: vec!["n".into()] },
        known_effects: vec![],
        evidence_refs: vec!["run".into()],
        thresholds: None,
    }
}

fn inputs(max: Tier) -> TierInputs {
    TierInputs { rule_max: max, ..TierInputs::default() }
}

#[test]
fn tier_is_the_minimum_of_every_input() {
    let base = inputs(Tier::DefaultSelected);
    assert_eq!(compute_tier(&base).tier, Tier::DefaultSelected);
    for (mutate, expect, reason) in [
        (Box::new(|i: &mut TierInputs| i.registry = RegistryStatus::VersionUntested) as Box<dyn Fn(&mut TierInputs)>, Tier::ReviewOnly, Some("version_untested")),
        (Box::new(|i: &mut TierInputs| i.registry = RegistryStatus::NotRegistered), Tier::ReviewOnly, Some("registry_missing")),
        (Box::new(|i: &mut TierInputs| i.protection = Tier::Blocked), Tier::Blocked, None),
        (Box::new(|i: &mut TierInputs| i.evidence = Tier::ReviewOnly), Tier::ReviewOnly, None),
        (Box::new(|i: &mut TierInputs| i.rule_max = Tier::ReviewOnly), Tier::ReviewOnly, None),
        (Box::new(|i: &mut TierInputs| i.dependency = true), Tier::ReviewOnly, Some("dependency_store_review_only")),
    ] {
        let mut i = base.clone();
        mutate(&mut i);
        let d = compute_tier(&i);
        assert_eq!(d.tier, expect, "{i:?}");
        if let Some(r) = reason {
            assert!(d.reasons.contains(&r.to_string()), "{:?}", d.reasons);
        }
    }
    // Nothing loosens a blocked input.
    let mut i = base.clone();
    i.protection = Tier::Blocked;
    i.registry = RegistryStatus::Covered;
    assert_eq!(compute_tier(&i).tier, Tier::Blocked);
}

#[test]
fn in_use_and_unknown_probes_both_skip() {
    let mut i = inputs(Tier::DefaultSelected);
    i.probes = vec![ProbeResult::new("owner_process", ProbeState::NotInUse, "")];
    assert_eq!(compute_tier(&i).skip, None);
    i.probes.push(ProbeResult::new("open_file_handle", ProbeState::Unknown, "enumeration failed"));
    let d = compute_tier(&i);
    assert_eq!(d.skip, Some(SkipReason::ProbeUnknown), "unknown is never treated as not in use");
    assert!(d.reasons.contains(&"probe:open_file_handle:unknown".to_string()));
    i.probes.push(ProbeResult::new("owner_process", ProbeState::InUse, "chrome.exe"));
    assert_eq!(compute_tier(&i).skip, Some(SkipReason::InUse));
    assert_eq!(compute_tier(&i).tier, Tier::DefaultSelected, "the tier stays explainable; the skip makes it unselectable");
}

#[test]
fn dependency_categories_are_always_review_only() {
    let pkg = rule("npm.cache", RootResolver::PackageManagerCache, RebuildClass::LocalRebuild, Tier::DefaultSelected);
    assert!(is_dependency_store(&pkg));
    let mut dev = rule("x.deps", RootResolver::ApplicationCache, RebuildClass::NetworkRedownload, Tier::ReviewOnly);
    assert!(is_dependency_store(&dev));
    dev.category = "browsers".into();
    assert!(!is_dependency_store(&dev));
    let i = TierInputs { rule_max: Tier::DefaultSelected, dependency: is_dependency_store(&pkg), ..TierInputs::default() };
    assert_eq!(compute_tier(&i).tier, Tier::ReviewOnly);
}

#[test]
fn versions_compare_numerically_and_unparseable_is_never_covered() {
    assert_eq!(parse_version("155.0.8059.41"), Some(vec![155, 0, 8059, 41]));
    assert_eq!(parse_version("v1.2"), Some(vec![1, 2]));
    for bad in ["", "1.2.3-beta", "abc", "1..2", "1.2.3.4.5.6.7"] {
        assert_eq!(parse_version(bad), None, "{bad}");
    }
    assert_eq!(compare_versions(&[1, 2], &[1, 2, 0]), Ordering::Equal);
    assert_eq!(compare_versions(&[10, 0], &[9, 9, 9]), Ordering::Greater);
    let r = VersionRange { min_inclusive: "155.0".into(), max_exclusive: Some("156.0".into()) };
    assert_eq!(version_in_range("155.0.8059.41", &r), Some(true));
    assert_eq!(version_in_range("156.0.1", &r), Some(false));
    assert_eq!(version_in_range("154.9", &r), Some(false));
    assert_eq!(version_in_range("155.0-dev", &r), None);
    let open = VersionRange { min_inclusive: "10.0.22000".into(), max_exclusive: None };
    assert_eq!(version_in_range("10.0.26200", &open), Some(true));
}

#[test]
fn installed_version_outside_the_registry_range_is_review_only() {
    let r = rule("google.chrome.gpu-cache", RootResolver::BrowserProfile, RebuildClass::LocalRebuild, Tier::DefaultSelected);
    let mut reg = BTreeMap::new();
    reg.insert("google.chrome.gpu-cache@1.0.0".to_string(), entry("google.chrome.gpu-cache", "155.0", Some("156.0")));
    let covered = registry_verdict(&r, &reg, Some("155.0.8059.41"));
    assert_eq!(covered.status, RegistryStatus::Covered);
    let newer = registry_verdict(&r, &reg, Some("156.0.1000.1"));
    assert_eq!(newer.status, RegistryStatus::VersionUntested);
    assert!(newer.reason.unwrap().contains("outside the tested range"));
    assert_eq!(registry_verdict(&r, &reg, None).status, RegistryStatus::VersionUntested, "unknown version is not covered");
    assert_eq!(registry_verdict(&r, &BTreeMap::new(), Some("155.0")).status, RegistryStatus::NotRegistered);
    let mut other = reg.clone();
    other.get_mut("google.chrome.gpu-cache@1.0.0").unwrap().rule_version = "0.9.0".into();
    assert_eq!(registry_verdict(&r, &other, Some("155.0")).status, RegistryStatus::NotRegistered);
    // Through the context, as analysis uses it.
    let installed: BTreeMap<String, String> = [("google.chrome".to_string(), "157.0".to_string())].into();
    let ctx = CatalogContext { registry: Some(&reg), installed: Some(&installed) };
    let d = compute_tier(&TierInputs { rule_max: Tier::DefaultSelected, registry: ctx.verdict(&r).status, ..TierInputs::default() });
    assert_eq!(d.tier, Tier::ReviewOnly);
    assert_eq!(CatalogContext::default().verdict(&r).status, RegistryStatus::NotChecked);
}

#[test]
fn category_status_reflects_enabled_rules_per_platform() {
    let info = category("developer").unwrap();
    let mut r = rule("npm.cache", RootResolver::PackageManagerCache, RebuildClass::NetworkRedownload, Tier::ReviewOnly);
    assert_eq!(category_status(info, Platform::Windows, std::slice::from_ref(&r)), CategoryStatus::Available);
    assert_eq!(category_status(info, Platform::Macos, std::slice::from_ref(&r)), CategoryStatus::Unsupported);
    r.status = RuleStatus::Experimental;
    assert_eq!(category_status(info, Platform::Windows, std::slice::from_ref(&r)), CategoryStatus::Unverified);
    assert_eq!(CATEGORIES.len(), 15);
    assert!(category("shell_scripts").is_none());
}

#[test]
fn merging_keeps_the_stricter_inputs() {
    let mut a = inputs(Tier::DefaultSelected);
    a.registry = RegistryStatus::Covered;
    let mut b = inputs(Tier::ReviewOnly);
    b.registry = RegistryStatus::VersionUntested;
    b.dependency = true;
    a.merge(&b);
    assert_eq!((a.rule_max, a.registry, a.dependency), (Tier::ReviewOnly, RegistryStatus::VersionUntested, true));
}

fn shipped(dir: &str) -> Vec<String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(dir);
    let mut paths: Vec<_> = std::fs::read_dir(root).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    paths.sort();
    paths.into_iter().map(|p| std::fs::read_to_string(p).unwrap()).collect()
}

/// The shipped Windows pack (waves 1 and 2) loads with its tested registry, and the catalog
/// invariants hold for every rule: probes are known kinds, administrator rules can be skipped,
/// nothing that needs the network or holds records is default-selected.
#[test]
fn shipped_windows_pack_loads_with_its_registry() {
    let registry: BTreeMap<String, RegistryEntry> =
        shipped("rules/registry").iter().map(|t| serde_json::from_str::<RegistryEntry>(t).unwrap()).map(|e| (e.registry_id.clone(), e)).collect();
    let rules: Vec<Rule> = shipped("rules/windows").iter().map(|t| Rule::parse(t).unwrap()).collect();
    let set = crate::rules::RuleSet::new(rules.clone(), &registry).expect("pack and registry agree");
    for r in &rules {
        assert!(r.probes.iter().all(|p| PROBE_KINDS.contains(&p.as_str())), "{}: unknown probe kind", r.rule_id);
        assert!(category(&r.category).is_some(), "{}: unknown category", r.rule_id);
        if r.requires_admin {
            assert!(r.probes.iter().any(|p| p == "elevated_token"), "{}", r.rule_id);
        }
        if r.max_tier == Tier::DefaultSelected {
            assert_eq!(r.rebuild_class, RebuildClass::LocalRebuild, "{}", r.rule_id);
        }
        if r.action_kind != ActionKind::Filesystem {
            assert!(r.allowed_modes.is_empty(), "{}: hint and owner-tool rows carry no file modes", r.rule_id);
        }
    }
    // Wave 2 (D4) has enabled rules in each of its Windows categories on the test device.
    for cat in ["app_caches", "cloud_office", "virtualization"] {
        assert_eq!(category_status(category(cat).unwrap(), Platform::Windows, &set.rules), CategoryStatus::Available, "{cat}");
    }
    let admin: Vec<&Rule> = set.enabled_for(Platform::Windows).filter(|r| r.requires_admin).collect();
    assert!(!admin.is_empty() && admin.iter().all(|r| r.max_tier == Tier::ReviewOnly), "administrator logs are enabled and review-only");
}

/// ANA-02 / WFA-17: the explanation names the rule ceiling and every lowering code, states the age basis as a
/// modification time (never as a last use), and gives each offered mode's recovery limit.
#[test]
fn explanations_name_the_rule_basis_and_recovery_limits() {
    let mut temp = rule("windows.user-temp.stale-files", RootResolver::UserTemp, RebuildClass::NotRebuildable, Tier::ReviewOnly);
    temp.impact.summary = "Files directly in the user's temporary folder.".into();
    temp.thresholds = Thresholds { min_age_days: Some(7), age_basis: Some("modified_time".into()), min_size_bytes: None, retention_days: None };
    let basis = match_reason(&temp);
    assert!(basis.starts_with("windows.user-temp.stale-files@1.0.0: Files directly in the user's temporary folder. Age threshold: modified at least 7 day(s)"), "{basis}");
    assert!(basis.contains("by its modification time") && basis.contains("when it was last used is not known"), "{basis}");
    assert_eq!(age_threshold(&temp), Some((7, "modified_time")));
    assert_eq!(tier_reason(&[], Some(&temp)), "rule windows.user-temp.stale-files@1.0.0 lists it for review only (its owner does not rebuild it)");
    let lowered = tier_reason(&["registry_missing".into(), "probe:open_file_handle:unknown".into()], Some(&temp));
    assert!(lowered.ends_with("; registry_missing, probe:open_file_handle:unknown"), "{lowered}");
    assert_eq!(tier_reason(&["protected:database".into()], None), "matched rule; protected:database");
    let cache = rule("google.chrome.code-cache", RootResolver::BrowserProfile, RebuildClass::LocalRebuild, Tier::DefaultSelected);
    assert!(tier_reason(&[], Some(&cache)).contains("allows default selection (rebuilt locally"));
    assert_eq!(age_threshold(&cache), None);
    assert_eq!(worst_rebuild([&cache, &temp]), Some(RebuildClass::NotRebuildable));
    let limits = recovery_limits(&[crate::rules::Mode::Trash, crate::rules::Mode::Quarantine, crate::rules::Mode::Delete]);
    let kinds: Vec<&str> = limits.iter().map(|l| l.recovery).collect();
    assert_eq!(kinds, ["system_recycle_bin", "in_app_restore", "unavailable"]);
    assert!(limits[0].limit.contains(crate::manual_trash::BIN_NAME) && limits[1].limit.contains("frees no space") && limits[2].limit.contains("no undo"));
    assert!(recovery_limits(&[]).is_empty(), "a row without offered modes has no recovery to describe");
}
