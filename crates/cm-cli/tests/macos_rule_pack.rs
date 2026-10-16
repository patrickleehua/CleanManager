//! The macOS rule pack (catalog wave 1, parity 3.5), checked on every platform: until a rule is registered from a real
//! Mac run, it is experimental and review-only, so no entry can ever preselect or plan it; the pack still loads as one
//! rule set against the production registry, and every rule has positive and negative fixtures.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use cm_core::capability::Platform;
use cm_core::catalog::{CATEGORIES, CategoryStatus, category_status};
use cm_core::rules::{RegistryEntry, Rule, RuleSet, RuleStatus, Tier};
use serde_json::Value;

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)
}

fn json_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> =
        std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "json")).collect();
    out.sort();
    out
}

fn pack() -> Vec<(PathBuf, Rule)> {
    json_files(&repo("rules/macos")).into_iter().map(|p| {
        let rule = Rule::parse(&std::fs::read_to_string(&p).unwrap()).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        (p, rule)
    }).collect()
}

#[test]
fn every_macos_rule_is_experimental_and_review_only() {
    let rules = pack();
    assert!(rules.len() >= 30, "wave 1 is shipped ({} rules)", rules.len());
    for (path, r) in &rules {
        let id = &r.rule_id;
        assert_eq!(path.file_name().unwrap().to_str().unwrap(), format!("{id}.json"), "file named after the rule");
        assert_eq!(r.status, RuleStatus::Experimental, "{id}: experimental until registered on a real Mac");
        assert_eq!(r.platforms, [Platform::Macos], "{id}");
        assert_eq!(r.max_tier, Tier::ReviewOnly, "{id}: never default_selected before the rebuild is observed on a Mac");
        assert_eq!(r.tested_registry_ref, None, "{id}: no registration exists yet");
        assert!(r.evidence_refs.is_empty(), "{id}: no Mac run evidence exists yet");
        assert!(!r.requires_admin, "{id}: this build has no elevation path on macOS");
    }
    let mut registry = BTreeMap::new();
    for p in json_files(&repo("rules/registry")) {
        let e: RegistryEntry = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        registry.insert(e.registry_id.clone(), e);
    }
    let set = RuleSet::new(rules.into_iter().map(|(_, r)| r).collect(), &registry).expect("the macOS pack loads as one rule set");
    assert_eq!(set.enabled_for(Platform::Macos).count(), 0, "no actionable candidate can come from the macOS pack");
    for info in CATEGORIES {
        assert_ne!(category_status(info, Platform::Macos, &set.rules), CategoryStatus::Available, "{}: not selectable on macOS yet", info.id);
    }
}

#[test]
fn every_macos_rule_has_positive_and_negative_fixtures() {
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(repo("rules/macos/fixtures/wave1.json")).unwrap()).unwrap();
    let entries = manifest["rules"].as_object().unwrap();
    let ids: BTreeSet<String> = pack().into_iter().map(|(_, r)| r.rule_id).collect();
    assert_eq!(entries.keys().cloned().collect::<BTreeSet<_>>(), ids, "fixtures cover exactly the shipped rules");
    for (id, e) in entries {
        for kind in ["positive", "negative"] {
            let list = e["fixtures"][kind].as_array().unwrap_or_else(|| panic!("{id}: {kind}"));
            assert!(!list.is_empty(), "{id}: needs {kind} fixtures");
            for f in list {
                let (base, rel) = f.as_str().unwrap().split_once(':').unwrap_or_else(|| panic!("{id}: {f}"));
                assert!(["home", "temp", "downloads", "volume"].contains(&base), "{id}: {f}");
                assert!(!rel.is_empty() && !rel.starts_with('/') && !rel.split('/').any(|c| c == ".." || c == "."), "{id}: {f}");
            }
        }
        for f in e["fresh"].as_array().into_iter().flatten() {
            let listed = ["positive", "negative"].iter().any(|k| e["fixtures"][k].as_array().unwrap().contains(f));
            assert!(listed, "{id}: fresh fixture {f} is not a fixture");
        }
    }
}
