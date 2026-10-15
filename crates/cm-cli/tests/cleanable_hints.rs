//! DIN-05 end to end: `entries list` marks directories "cleanable by hand" only on a verified
//! signature (valid CACHEDIR.TAG, a build artifact next to its project marker, a location of an
//! enabled catalog cache rule), never on protected locations or unconfirmed evidence, and the
//! hint changes nothing (no candidates, no plan, no tier). Catalog roots come from
//! CM_RESOLVE_* overrides into a scratch directory with a fixture rule pack.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

const TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag.\n";

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-hints-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn cli(args: &[&str], root: &Path) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_cleanmanager"))
        .args(args)
        .arg("--json")
        .env("CM_STORE_DIR", root.join("store"))
        .env("CM_RULES_DIR", root.join("rules"))
        .env("CM_RESOLVE_LOCALAPPDATA", root.join("tree").join("local"))
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "exactly one record: {text}");
    (out.status.code().unwrap(), serde_json::from_str(&text).unwrap())
}

fn write(p: &Path, body: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// One enabled whole-directory cache rule (every folder in `D3DSCache`) with its registry entry.
fn write_pack(dir: &Path) {
    let rule = json!({
        "schema_version": "2.0", "rule_id": "test.d3d.folders", "version": "1.0.0", "status": "enabled", "platforms": ["windows"],
        "category": "system_data", "max_tier": "review_only", "rebuild_class": "local_rebuild", "owner": {"kind": "system", "id": "windows.directx"},
        "root_resolver": "user_cache",
        "match": {"object_type": "directory", "relative_components": [], "name_glob": "*", "extensions": [], "require_cachedir_tag": false,
                  "archive_payload": false, "max_depth": 0},
        "exclude": [], "probes": [], "thresholds": {"min_age_days": null, "age_basis": null, "min_size_bytes": null, "retention_days": null},
        "action_kind": "filesystem", "adapter_ref": null, "allowed_modes": ["trash"], "requires_admin": false,
        "protect_descendants": true, "follow_links": false,
        "impact": {"summary": "fixture", "regeneration": "fixture", "offline_effect": "fixture"},
        "tested_registry_ref": "test.d3d.folders@1.0.0", "evidence_refs": ["fixture"],
    });
    let registry = json!({
        "schema_version": "1.0", "registry_id": "test.d3d.folders@1.0.0", "rule_id": "test.d3d.folders", "rule_version": "1.0.0", "synthetic": false,
        "subject": {"kind": "system", "id": "windows.directx", "name": "fixture"}, "version_range": {"min_inclusive": "10.0", "max_exclusive": "99.0"},
        "platforms": [{"platform": "windows", "os_versions": ["10.0"], "architectures": ["x64"]}], "tested_at": "2026-10-16",
        "result": "passed", "fixtures": {"positive": ["p"], "negative": ["n"]}, "known_effects": [], "evidence_refs": ["fixture"],
        "thresholds": {"min_age_days": null, "age_basis": null, "min_size_bytes": null, "retention_days": null},
    });
    write(&dir.join("windows").join("test.d3d.folders.json"), &rule.to_string());
    write(&dir.join("registry").join("test.d3d.folders@1.0.0.json"), &registry.to_string());
}

fn entries(root: &Path, sid: &str, parent: &str) -> Vec<Value> {
    let (code, page) = cli(&["entries", "list", "--scan", sid, "--parent", parent, "--sort", "name_asc"], root);
    assert_eq!(code, 0, "{page}");
    assert_eq!(page["kind"], "entries_page");
    page["items"].as_array().unwrap().clone()
}

fn named<'a>(items: &'a [Value], name: &str) -> &'a Value {
    items.iter().find(|i| i["path"]["display"].as_str().unwrap().ends_with(&format!("\\{name}"))).unwrap_or_else(|| panic!("no {name}: {items:#?}"))
}

#[test]
fn entries_carry_hints_only_on_verified_unprotected_signatures() {
    let root = scratch("flow");
    write_pack(&root.join("rules"));
    let tree = root.join("tree");
    write(&tree.join("proj").join("package.json"), "{}");
    write(&tree.join("proj").join("node_modules").join("left-pad").join("index.js"), "module.exports = 1;");
    write(&tree.join("proj").join("src").join("main.js"), "1");
    write(&tree.join("tagged").join("CACHEDIR.TAG"), TAG);
    write(&tree.join("tagged").join("blob.bin"), "x");
    write(&tree.join("badtag").join("CACHEDIR.TAG"), "Signature: this is not the cachedir signature\n");
    write(&tree.join("kept").join("CACHEDIR.TAG"), TAG);
    write(&tree.join("plain").join("notes.txt"), "x");
    write(&tree.join("loose").join("target").join("out.o"), "x");
    write(&tree.join("local").join("D3DSCache").join("shader1").join("a.bin"), "x");

    // The user protects one tagged folder: protection wins over its signature.
    let (code, list) = cli(&["protect", "add", "--path", tree.join("kept").to_str().unwrap()], &root);
    assert_eq!(code, 0, "{list}");

    let (code, scan) = cli(&["scan", "--root", tree.to_str().unwrap()], &root);
    assert_eq!(code, 0, "{scan}");
    let sid = scan["scan_id"].as_str().unwrap().to_owned();
    let top = entries(&root, &sid, "1");

    let tagged = named(&top, "tagged");
    assert_eq!(tagged["cleanable_hint"], json!({"basis": "cachedir_tag", "rule_id": null, "detail": "CACHEDIR.TAG", "flow": "manual_trash"}));
    for name in ["badtag", "kept", "plain", "loose", "proj", "local"] {
        assert!(named(&top, name).get("cleanable_hint").is_none(), "{name} must not carry a hint: {}", named(&top, name));
    }

    let proj_id = named(&top, "proj")["entry_id"].as_str().unwrap().to_owned();
    let proj = entries(&root, &sid, &proj_id);
    assert_eq!(named(&proj, "node_modules")["cleanable_hint"], json!({"basis": "build_artifact", "rule_id": "purge.node-modules", "detail": "package.json", "flow": "purge"}));
    assert!(named(&proj, "src").get("cleanable_hint").is_none());
    assert!(named(&proj, "package.json").get("cleanable_hint").is_none(), "files never carry a hint");

    let loose_id = named(&top, "loose")["entry_id"].as_str().unwrap().to_owned();
    assert!(named(&entries(&root, &sid, &loose_id), "target").get("cleanable_hint").is_none(), "an artifact name without its project marker is not a signature");

    let local_id = named(&top, "local")["entry_id"].as_str().unwrap().to_owned();
    let d3d_id = named(&entries(&root, &sid, &local_id), "D3DSCache")["entry_id"].as_str().unwrap().to_owned();
    let shader = named(&entries(&root, &sid, &d3d_id), "shader1").clone();
    assert_eq!(shader["cleanable_hint"]["basis"], "catalog_rule");
    assert_eq!(shader["cleanable_hint"]["rule_id"], "test.d3d.folders");
    assert_eq!(shader["cleanable_hint"]["flow"], "manual_trash");

    // Read-only: listing hints ran no operation and left every file in place.
    let (code, ops) = cli(&["operations", "list"], &root);
    assert_eq!(code, 0, "{ops}");
    assert_eq!(ops["items"].as_array().map_or(0, Vec::len), 0, "{ops}");
    assert!(tree.join("tagged").join("blob.bin").exists() && tree.join("proj").join("node_modules").join("left-pad").join("index.js").exists());
    std::fs::remove_dir_all(&root).ok();
}
