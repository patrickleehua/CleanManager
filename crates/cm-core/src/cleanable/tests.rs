use std::collections::BTreeMap;

use serde_json::json;

use super::*;

fn win(s: &str) -> RawPath {
    RawPath::windows(s.encode_utf16().collect()).unwrap()
}

/// In-memory evidence: tag files, parent listings and protected paths/rules.
#[derive(Default)]
struct Fake {
    tags: Vec<&'static str>,
    listings: BTreeMap<&'static str, Vec<&'static str>>,
    protected: Vec<&'static str>,
    protected_rules: Vec<&'static str>,
    /// Protection list unreadable: nothing can be confirmed.
    unavailable: bool,
}

impl HintProbe for Fake {
    fn cachedir_tag(&self, dir: &RawPath) -> bool {
        self.tags.contains(&dir.display().as_str())
    }
    fn sibling_names(&self, dir: &RawPath) -> Option<Vec<String>> {
        let d = dir.display();
        let parent = &d[..d.rfind('\\')?];
        self.listings.get(parent).map(|names| names.iter().map(|n| n.to_lowercase()).collect())
    }
    fn path_allowed(&self, dir: &RawPath) -> bool {
        let d = dir.display().to_lowercase();
        !self.unavailable && !self.protected.iter().any(|p| d == p.to_lowercase() || d.starts_with(&format!("{}\\", p.to_lowercase())))
    }
    fn rule_allowed(&self, rule_id: &str) -> bool {
        !self.unavailable && !self.protected_rules.contains(&rule_id)
    }
}

fn rule(id: &str, components: &[&str], glob: Option<&str>, depth: u8, extra: serde_json::Value) -> Rule {
    let mut v = json!({
        "schema_version": "2.0", "rule_id": id, "version": "1.0.0", "status": "enabled", "platforms": ["windows"],
        "category": "app_caches", "max_tier": "review_only", "rebuild_class": "local_rebuild",
        "owner": {"kind": "app", "id": "x"}, "root_resolver": "application_cache",
        "match": {"object_type": "directory", "relative_components": components, "name_glob": glob, "extensions": [],
            "require_cachedir_tag": false, "archive_payload": false, "max_depth": depth},
        "exclude": [], "probes": [], "thresholds": {"min_age_days": null, "age_basis": null, "min_size_bytes": null, "retention_days": null},
        "action_kind": "filesystem", "adapter_ref": null, "allowed_modes": ["trash"], "requires_admin": false,
        "protect_descendants": true, "follow_links": false,
        "impact": {"summary": "s", "regeneration": "r", "offline_effect": "o"}, "tested_registry_ref": null, "evidence_refs": ["e"]
    });
    if let (Some(obj), Some(extra)) = (v.as_object_mut(), extra.as_object()) {
        for (k, x) in extra {
            obj.insert(k.clone(), x.clone());
        }
    }
    Rule::parse(&v.to_string()).unwrap()
}

fn caches() -> Vec<CacheLocation> {
    let rules = vec![
        rule("app.code-cache", &["Code Cache"], None, 0, json!({})),
        rule("gpu.shader-cache", &[], Some("*Cache"), 0, json!({})),
        rule("app.logs", &["logs"], None, 0, json!({"thresholds": {"min_age_days": 7, "age_basis": "modified_time", "min_size_bytes": null, "retention_days": null}})),
        rule("app.experimental", &["Exp"], None, 0, json!({"status": "experimental", "evidence_refs": []})),
    ];
    let roots = [
        ("app.code-cache", r"C:\Users\u\AppData\Roaming\App"),
        ("gpu.shader-cache", r"C:\Users\u\AppData\Local\GPU"),
        ("app.logs", r"C:\Users\u\AppData\Roaming\App"),
        ("app.experimental", r"C:\Users\u\AppData\Roaming\App"),
    ]
    .map(|(a, b)| (a.to_owned(), b.to_owned()));
    cache_locations(&rules, Platform::Windows, &roots)
}

#[test]
fn cachedir_tag_signature_is_checked_byte_for_byte() {
    assert!(is_cachedir_signature(b"Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag."));
    assert!(is_cachedir_signature(CACHEDIR_SIGNATURE));
    assert!(!is_cachedir_signature(b"Signature: 8a477f597d28d172789f06886806bc5"), "truncated header");
    assert!(!is_cachedir_signature(b"signature: 8a477f597d28d172789f06886806bc55"), "case matters");
    assert!(!is_cachedir_signature(b"Signature: 00000000000000000000000000000000"));
    assert!(!is_cachedir_signature(b""));
}

#[test]
fn a_valid_cache_tag_gives_a_manual_trash_hint() {
    let probe = Fake { tags: vec![r"D:\data\cache"], ..Default::default() };
    let h = hint_for(&win(r"D:\data\cache"), &probe, &[]).unwrap();
    assert_eq!((h.basis, h.flow, h.rule_id.as_deref(), h.detail.as_str()), (HintBasis::CachedirTag, HintFlow::ManualTrash, None, CACHEDIR_TAG));
    // An invalid tag is not evidence (the probe answers false): no hint.
    assert_eq!(hint_for(&win(r"D:\data\other"), &probe, &[]), None);
}

#[test]
fn build_artifacts_reuse_the_purge_recognizer() {
    let mut listings = BTreeMap::new();
    listings.insert(r"D:\src\web", vec!["package.json", "node_modules", "dist"]);
    listings.insert(r"D:\src\rs", vec!["Cargo.toml", "target"]);
    listings.insert(r"D:\src\loose", vec!["target", "notes.txt"]);
    listings.insert(r"D:\src\web\node_modules\pkg", vec!["package.json", "node_modules"]);
    listings.insert(r"C:\Users\u\.vscode\extensions\ext", vec!["package.json", "node_modules"]);
    let probe = Fake { listings, ..Default::default() };
    let h = hint_for(&win(r"D:\src\web\node_modules"), &probe, &[]).unwrap();
    assert_eq!((h.basis, h.flow, h.rule_id.as_deref(), h.detail.as_str()), (HintBasis::BuildArtifact, HintFlow::Purge, Some("purge.node-modules"), "package.json"));
    let h = hint_for(&win(r"D:\src\rs\target"), &probe, &[]).unwrap();
    assert_eq!((h.rule_id.as_deref(), h.detail.as_str()), (Some("purge.target"), "cargo.toml"));
    // dist is an artifact of package.json projects too.
    assert_eq!(hint_for(&win(r"D:\src\web\dist"), &probe, &[]).unwrap().rule_id.as_deref(), Some("purge.dist"));
    // A name alone is not a signature: no owner marker next to it.
    assert_eq!(hint_for(&win(r"D:\src\loose\target"), &probe, &[]), None);
    // Nested dependencies and dot-directory tool data are never projects for the purge search.
    assert_eq!(hint_for(&win(r"D:\src\web\node_modules\pkg\node_modules"), &probe, &[]), None);
    assert_eq!(hint_for(&win(r"C:\Users\u\.vscode\extensions\ext\node_modules"), &probe, &[]), None);
    // An unlistable parent cannot confirm the signature.
    assert_eq!(hint_for(&win(r"E:\gone\node_modules"), &probe, &[]), None);
}

#[test]
fn catalog_cache_locations_cover_their_targets_only() {
    let c = caches();
    assert_eq!(c.len(), 2, "age-threshold and experimental rules are not whole-cache locations: {c:?}");
    let probe = Fake::default();
    let at = |p: &str| hint_for(&win(p), &probe, &c).map(|h| (h.basis, h.rule_id.unwrap(), h.flow));
    let code = Some((HintBasis::CatalogRule, "app.code-cache".to_owned(), HintFlow::ManualTrash));
    assert_eq!(at(r"C:\Users\u\AppData\Roaming\App\Code Cache"), code);
    assert_eq!(at(r"c:\users\U\appdata\roaming\app\code cache\js"), code, "case-insensitive on Windows; inside the target");
    assert_eq!(at(r"C:\Users\u\AppData\Roaming\App"), None, "the application folder itself is not a cache");
    assert_eq!(at(r"C:\Users\u\AppData\Roaming\App\Code Cache Old"), None);
    assert_eq!(at(r"C:\Users\u\AppData\Roaming\App\logs"), None, "a dated log rule removes old files, not the folder");
    let gpu = Some((HintBasis::CatalogRule, "gpu.shader-cache".to_owned(), HintFlow::ManualTrash));
    assert_eq!(at(r"C:\Users\u\AppData\Local\GPU\DxCache"), gpu);
    assert_eq!(at(r"C:\Users\u\AppData\Local\GPU"), None);
    assert_eq!(at(r"C:\Users\u\AppData\Local\GPU\Settings"), None, "a non-matching sibling is not covered");
}

#[test]
fn protection_wins_over_every_signature() {
    let mut listings = BTreeMap::new();
    listings.insert(r"D:\src\web", vec!["package.json", "node_modules"]);
    let probe = Fake {
        tags: vec![r"D:\src\web\node_modules", r"C:\Users\u\.ssh\cache"],
        listings,
        protected: vec![r"D:\src\web", r"C:\Users\u\.ssh"],
        ..Default::default()
    };
    assert_eq!(hint_for(&win(r"D:\src\web\node_modules"), &probe, &[]), None, "inside a user-protected folder");
    assert_eq!(hint_for(&win(r"C:\Users\u\.ssh\cache"), &probe, &[]), None, "a cache tag inside a protected location");
    // A protected rule falls back to another confirmed basis, never to an unconfirmed one.
    let c = caches();
    let tagged = Fake { tags: vec![r"C:\Users\u\AppData\Roaming\App\Code Cache"], protected_rules: vec!["app.code-cache"], ..Default::default() };
    assert_eq!(hint_for(&win(r"C:\Users\u\AppData\Roaming\App\Code Cache"), &tagged, &c).unwrap().basis, HintBasis::CachedirTag);
    let untagged = Fake { protected_rules: vec!["app.code-cache"], ..Default::default() };
    assert_eq!(hint_for(&win(r"C:\Users\u\AppData\Roaming\App\Code Cache"), &untagged, &c), None);
    // An unreadable protection list confirms nothing.
    let unavailable = Fake { tags: vec![r"D:\data\cache"], unavailable: true, ..Default::default() };
    assert_eq!(hint_for(&win(r"D:\data\cache"), &unavailable, &[]), None);
}

#[test]
fn no_signature_no_hint() {
    let mut listings = BTreeMap::new();
    listings.insert(r"D:\data", vec!["photos", "readme.txt"]);
    let probe = Fake { listings, ..Default::default() };
    assert_eq!(hint_for(&win(r"D:\data\photos"), &probe, &caches()), None);
}

#[test]
fn the_build_artifact_basis_is_preferred_and_serializes_flat() {
    let mut listings = BTreeMap::new();
    listings.insert(r"D:\src\rs", vec!["cargo.toml", "target"]);
    let probe = Fake { tags: vec![r"D:\src\rs\target"], listings, ..Default::default() };
    let h = hint_for(&win(r"D:\src\rs\target"), &probe, &[]).unwrap();
    assert_eq!(h.basis, HintBasis::BuildArtifact);
    assert_eq!(
        serde_json::to_value(&h).unwrap(),
        json!({"basis": "build_artifact", "rule_id": "purge.target", "detail": "cargo.toml", "flow": "purge"})
    );
}
