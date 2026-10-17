//! The macOS rule pack (catalog waves 1 and 2, parity 3.5 and 3.8), checked on every platform: until a rule is
//! registered from a real Mac run, it is experimental and review-only, so no entry can ever preselect or plan it; the
//! pack still loads as one rule set against the production registry, every rule has positive and negative fixtures in
//! exactly one wave's manifest, and only the system-wide log rules need administrator rights.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use cm_core::capability::Platform;
use cm_core::catalog::{CATEGORIES, CategoryStatus, category_status};
use cm_core::rules::{RegistryEntry, RootResolver, Rule, RuleSet, RuleStatus, Tier};
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
    assert!(rules.len() >= 55, "waves 1 and 2 are shipped ({} rules)", rules.len());
    for (path, r) in &rules {
        let id = &r.rule_id;
        assert_eq!(path.file_name().unwrap().to_str().unwrap(), format!("{id}.json"), "file named after the rule");
        assert_eq!(r.status, RuleStatus::Experimental, "{id}: experimental until registered on a real Mac");
        assert_eq!(r.platforms, [Platform::Macos], "{id}");
        assert_eq!(r.max_tier, Tier::ReviewOnly, "{id}: never default_selected before the rebuild is observed on a Mac");
        assert_eq!(r.tested_registry_ref, None, "{id}: no registration exists yet");
        assert!(r.evidence_refs.is_empty(), "{id}: no Mac run evidence exists yet");
        // Administrator-only locations are system-wide logs; without root (no privileged helper in this build) the
        // `elevated_token` probe is unknown and every candidate is skipped.
        assert_eq!(r.requires_admin, r.root_resolver == RootResolver::SystemLogs, "{id}: only system-wide logs need administrator rights");
        if r.requires_admin {
            assert!(r.probes.iter().any(|p| p == "elevated_token"), "{id}: must declare elevated_token");
        }
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
    let mut entries = BTreeMap::new();
    for wave in ["wave1", "wave2"] {
        let manifest: Value = serde_json::from_str(&std::fs::read_to_string(repo(&format!("rules/macos/fixtures/{wave}.json"))).unwrap()).unwrap();
        assert_eq!(manifest["schema"], "cleanmanager.mac-rule-fixtures/1", "{wave}");
        let listed = manifest["rules"].as_object().unwrap();
        assert!(!listed.is_empty(), "{wave}");
        for (id, e) in listed {
            assert!(entries.insert(id.clone(), e.clone()).is_none(), "{id} is listed in two waves");
        }
    }
    let ids: BTreeSet<String> = pack().into_iter().map(|(_, r)| r.rule_id).collect();
    assert_eq!(entries.keys().cloned().collect::<BTreeSet<_>>(), ids, "fixtures of waves 1 and 2 cover exactly the shipped rules");
    for (id, e) in &entries {
        for kind in ["positive", "negative"] {
            let list = e["fixtures"][kind].as_array().unwrap_or_else(|| panic!("{id}: {kind}"));
            assert!(!list.is_empty(), "{id}: needs {kind} fixtures");
            for f in list {
                let (base, rel) = f.as_str().unwrap().split_once(':').unwrap_or_else(|| panic!("{id}: {f}"));
                assert!(["home", "temp", "downloads", "volume", "system"].contains(&base), "{id}: {f}");
                assert!(!rel.is_empty() && !rel.starts_with('/') && !rel.split('/').any(|c| c == ".." || c == "."), "{id}: {f}");
            }
        }
        for f in e["fresh"].as_array().into_iter().flatten() {
            let listed = ["positive", "negative"].iter().any(|k| e["fixtures"][k].as_array().unwrap().contains(f));
            assert!(listed, "{id}: fresh fixture {f} is not a fixture");
        }
    }
}

/// ANA-03: no macOS rule targets a protected category (sessions, message stores, credentials, databases, site
/// storage), and no positive fixture lies at or under a name the default protection classes guard. Package stores that
/// are not caches (Maven's `~/.m2/repository`) are never targeted either.
#[test]
fn no_macos_rule_targets_a_protected_category() {
    use cm_core::analysis::class_of_name;
    for (_, r) in pack() {
        let id = &r.rule_id;
        let named = r.matcher.relative_components.iter().chain(r.matcher.name_glob.iter()).filter(|n| !n.contains(['*', '?', '[']));
        for n in named {
            let lower = n.to_lowercase();
            assert_eq!(class_of_name(&lower, true, false), None, "{id}: matches the protected name {n}");
            assert_eq!(class_of_name(&lower, false, false), None, "{id}: matches the protected name {n}");
            assert!(![".m2", "repository", "messages", "message", "msg"].contains(&lower.as_str()), "{id}: {n}");
        }
        for e in &r.matcher.extensions {
            assert_eq!(class_of_name(&format!("x.{e}"), false, false), None, "{id}: matches protected .{e} files");
        }
        assert!(!r.owner.id.contains("maven"), "{id}: Maven's repository is a package store, not a cache");
    }
    for wave in ["wave1", "wave2"] {
        let manifest: Value = serde_json::from_str(&std::fs::read_to_string(repo(&format!("rules/macos/fixtures/{wave}.json"))).unwrap()).unwrap();
        for (id, e) in manifest["rules"].as_object().unwrap() {
            for f in e["fixtures"]["positive"].as_array().unwrap() {
                let (_, rel) = f.as_str().unwrap().split_once(':').unwrap();
                let comps: Vec<&str> = rel.trim_end_matches('/').split('/').collect();
                for (i, c) in comps.iter().enumerate() {
                    let is_dir = i + 1 < comps.len() || rel.ends_with('/');
                    assert_eq!(class_of_name(&c.to_lowercase(), is_dir, false), None, "{id}: positive fixture {f} is protected at {c}");
                }
            }
        }
    }
}
