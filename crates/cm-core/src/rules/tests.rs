use super::*;

fn repo(rel: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)).unwrap()
}

fn example_v2() -> serde_json::Value {
    serde_json::from_str(&repo("rules/examples/v2/example-browser-code-cache.json")).unwrap()
}

fn parse(v: &serde_json::Value) -> Result<Rule, RuleError> {
    Rule::parse(&v.to_string())
}

#[test]
fn shipped_examples_parse_and_stay_experimental() {
    for rel in [
        "rules/examples/example-cache.json",
        "rules/examples/v2/example-browser-code-cache.json",
        "rules/examples/v2/example-package-cache-adapter.json",
        "rules/examples/v2/example-installer-files.json",
    ] {
        let rule = Rule::parse(&repo(rel)).unwrap();
        assert_eq!(rule.status, RuleStatus::Experimental, "{rel}");
    }
    let set = RuleSet::new(vec![Rule::parse(&repo("rules/examples/example-cache.json")).unwrap()], &BTreeMap::new()).unwrap();
    assert_eq!(set.enabled_for(Platform::Windows).count(), 0, "examples never produce actionable candidates");
}

#[test]
fn scripts_regexes_links_and_escapes_are_rejected() {
    let mut v = example_v2();
    v["root_resolver"] = "script".into();
    assert!(parse(&v).is_err(), "unknown resolver kind");
    let mut v = example_v2();
    v["match"]["path_regex"] = ".*".into();
    assert!(parse(&v).is_err(), "unknown field");
    let mut v = example_v2();
    v["follow_links"] = true.into();
    assert!(parse(&v).is_err());
    let mut v = example_v2();
    v["match"]["relative_components"] = serde_json::json!([".."]);
    assert!(parse(&v).is_err());
    let mut v = example_v2();
    v["exclude"] = serde_json::json!(["**"]);
    assert!(parse(&v).is_err());
}

#[test]
fn promotion_requires_real_registry_evidence_for_every_platform() {
    let mut v = example_v2();
    v["status"] = "verified".into();
    assert!(parse(&v).is_err(), "verified without evidence_refs");
    v["evidence_refs"] = serde_json::json!(["run_x"]);
    v["status"] = "enabled".into();
    let rule = parse(&v).unwrap();
    assert!(rule.check_registry(&BTreeMap::new()).is_err(), "enabled without registry ref");

    let mut entry: RegistryEntry = serde_json::from_str(&repo("rules/registry/examples/example-browser-code-cache.registry.json")).unwrap();
    let mut rule = rule;
    rule.tested_registry_ref = Some(entry.registry_id.clone());
    let reg = |e: &RegistryEntry| BTreeMap::from([(e.registry_id.clone(), e.clone())]);
    assert!(rule.check_registry(&reg(&entry)).is_err(), "synthetic registry entries never back enabled rules");
    entry.synthetic = false;
    assert!(rule.check_registry(&reg(&entry)).is_err(), "rule claims macOS but registry covers only Windows");
    rule.platforms = vec![Platform::Windows];
    rule.check_registry(&reg(&entry)).unwrap();
    entry.result = "failed".into();
    assert!(rule.check_registry(&reg(&entry)).is_err());
}

#[test]
fn network_rebuild_cannot_be_default_selected() {
    let mut v: serde_json::Value = serde_json::from_str(&repo("rules/examples/v2/example-package-cache-adapter.json")).unwrap();
    v["max_tier"] = "default_selected".into();
    assert!(parse(&v).is_err());
}

#[test]
fn rule_set_digest_is_stable_and_order_independent() {
    let a = Rule::parse(&repo("rules/examples/v2/example-browser-code-cache.json")).unwrap();
    let b = Rule::parse(&repo("rules/examples/v2/example-installer-files.json")).unwrap();
    let x = RuleSet::new(vec![a.clone(), b.clone()], &BTreeMap::new()).unwrap();
    let y = RuleSet::new(vec![b, a.clone()], &BTreeMap::new()).unwrap();
    assert_eq!(x.digest, y.digest);
    assert!(RuleSet::new(vec![a.clone(), a], &BTreeMap::new()).is_err(), "duplicate rule ids");
}

#[test]
fn globs_are_single_segment_and_case_aware() {
    assert!(glob_match("*.tmp", "a.TMP", true));
    assert!(!glob_match("*.tmp", "a.TMP", false));
    assert!(glob_match("f?le*", "file-1", false));
    assert!(!glob_match("*.log", "x.log.bak", false));
    assert!(glob_match("*", "", false));
}

#[test]
fn tier_min_is_the_stricter_tier() {
    assert_eq!(Tier::DefaultSelected.min(Tier::ReviewOnly), Tier::ReviewOnly);
    assert_eq!(Tier::Blocked.min(Tier::DefaultSelected), Tier::Blocked);
}

#[test]
fn changing_a_threshold_needs_a_newly_tested_rule_version() {
    let mut v = example_v2();
    v["status"] = "enabled".into();
    v["platforms"] = serde_json::json!(["windows"]);
    v["evidence_refs"] = serde_json::json!(["run_x"]);
    v["thresholds"]["min_age_days"] = 7.into();
    v["thresholds"]["age_basis"] = "modified_time".into();
    let mut entry: RegistryEntry = serde_json::from_str(&repo("rules/registry/examples/example-browser-code-cache.registry.json")).unwrap();
    entry.synthetic = false;
    v["tested_registry_ref"] = entry.registry_id.clone().into();
    let mut rule = parse(&v).unwrap();
    let reg = |e: &RegistryEntry| BTreeMap::from([(e.registry_id.clone(), e.clone())]);
    assert!(rule.check_registry(&reg(&entry)).is_err(), "a rule with thresholds needs the tested thresholds recorded");
    entry.thresholds = Some(rule.thresholds.clone());
    rule.check_registry(&reg(&entry)).unwrap();
    rule.thresholds.min_age_days = Some(1);
    let err = rule.check_registry(&reg(&entry)).unwrap_err().to_string();
    assert!(err.contains("thresholds differ"), "{err}");
}

#[test]
fn administrator_rules_must_declare_the_elevation_probe() {
    let mut v = example_v2();
    v["requires_admin"] = true.into();
    assert!(parse(&v).unwrap_err().to_string().contains("elevated_token"), "requires_admin without the probe");
    v["probes"] = serde_json::json!(["open_file_handle", "elevated_token"]);
    assert!(parse(&v).is_ok());
}
