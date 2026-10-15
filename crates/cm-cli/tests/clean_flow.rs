//! `clean` command group end to end on real NTFS with a fixture rule pack (CLI-10/11/12, CAT-02,
//! CAT-06): the dry-run preview and the non-interactive plan select the same rows; blocked,
//! in-use and review-only rows never enter a plan without an explicit, allowed ID; nothing is
//! authorized or changed. Rule roots come from CM_RESOLVE_* overrides into a scratch directory.
#![cfg(windows)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-clean-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn cli(args: &[&str], store: &Path, local: &Path, rules: &Path) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_cleanmanager"))
        .args(args)
        .arg("--json")
        .env("CM_STORE_DIR", store)
        .env("CM_RULES_DIR", rules)
        .env("CM_RESOLVE_LOCALAPPDATA", local)
        .env("CM_RESOLVE_TEMP", local.join("Temp"))
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "exactly one record: {text}");
    (out.status.code().unwrap(), serde_json::from_str(&text).unwrap())
}

fn rule(id: &str, owner: &str, resolver: &str, object: &str, max_tier: &str, rebuild: &str, probes: &[&str]) -> Value {
    json!({
        "schema_version": "2.0", "rule_id": id, "version": "1.0.0", "status": "enabled", "platforms": ["windows"],
        "category": "system_data", "max_tier": max_tier, "rebuild_class": rebuild, "owner": {"kind": "system", "id": owner},
        "root_resolver": resolver,
        "match": {"object_type": object, "relative_components": [], "name_glob": "*", "extensions": [], "require_cachedir_tag": false,
                  "archive_payload": false, "max_depth": 0},
        "exclude": [], "probes": probes,
        "thresholds": {"min_age_days": null, "age_basis": null, "min_size_bytes": null, "retention_days": null},
        "action_kind": "filesystem", "adapter_ref": null, "allowed_modes": ["trash", "quarantine"], "requires_admin": false,
        "protect_descendants": true, "follow_links": false,
        "impact": {"summary": "fixture", "regeneration": "fixture", "offline_effect": "fixture"},
        "tested_registry_ref": format!("{id}@1.0.0"), "evidence_refs": ["fixture"],
    })
}

fn registry(id: &str, owner: &str) -> Value {
    json!({
        "schema_version": "1.0", "registry_id": format!("{id}@1.0.0"), "rule_id": id, "rule_version": "1.0.0", "synthetic": false,
        "subject": {"kind": "system", "id": owner, "name": "fixture"}, "version_range": {"min_inclusive": "10.0", "max_exclusive": "99.0"},
        "platforms": [{"platform": "windows", "os_versions": ["10.0"], "architectures": ["x64"]}], "tested_at": "2026-10-15",
        "result": "passed", "fixtures": {"positive": ["p"], "negative": ["n"]}, "known_effects": [], "evidence_refs": ["fixture"],
        "thresholds": {"min_age_days": null, "age_basis": null, "min_size_bytes": null, "retention_days": null},
    })
}

fn write_pack(dir: &Path) {
    std::fs::create_dir_all(dir.join("windows")).unwrap();
    std::fs::create_dir_all(dir.join("registry")).unwrap();
    for (r, owner) in [
        (rule("test.d3d.folders", "windows.directx", "user_cache", "directory", "default_selected", "local_rebuild", &["open_file_handle"]), "windows.directx"),
        (rule("test.temp.files", "windows.temp", "user_temp", "file", "review_only", "not_rebuildable", &["open_file_handle"]), "windows.temp"),
    ] {
        let id = r["rule_id"].as_str().unwrap().to_owned();
        std::fs::write(dir.join("windows").join(format!("{id}.json")), r.to_string()).unwrap();
        std::fs::write(dir.join("registry").join(format!("{id}@1.0.0.json")), registry(&id, owner).to_string()).unwrap();
    }
}

fn by_suffix<'a>(items: &'a [Value], suffix: &str) -> &'a Value {
    items.iter().find(|i| i["path"]["display"].as_str().unwrap().ends_with(suffix)).unwrap_or_else(|| panic!("no item {suffix}: {items:#?}"))
}

#[test]
fn preview_and_plan_select_the_same_rows_and_refuse_the_rest() {
    let root = scratch("flow");
    let (local, store, rules) = (root.join("local"), root.join("store"), root.join("rules"));
    write_pack(&rules);
    let d3d = local.join("D3DSCache");
    for (dir, file) in [("free", "a.bin"), ("site", "Login Data"), ("held", "busy.bin")] {
        std::fs::create_dir_all(d3d.join(dir)).unwrap();
        std::fs::write(d3d.join(dir).join(file), b"0123456789").unwrap();
    }
    std::fs::create_dir_all(local.join("Temp")).unwrap();
    std::fs::write(local.join("Temp").join("leftover.tmp"), b"x").unwrap();

    let (code, scan) = cli(&["scan", "--root", local.to_str().unwrap()], &store, &local, &rules);
    assert_eq!(code, 0, "{scan}");
    let sid = scan["scan_id"].as_str().unwrap();
    let busy = std::fs::File::open(d3d.join("held").join("busy.bin")).unwrap(); // another process still uses it

    let (code, preview) = cli(&["clean", "--scan", sid, "--dry-run", "--non-interactive"], &store, &local, &rules);
    assert_eq!(code, 0, "{preview}");
    assert_eq!((preview["kind"].as_str(), preview["dry_run"].as_bool()), (Some("selection_preview"), Some(true)));
    let items = preview["items"].as_array().unwrap();
    let free = by_suffix(items, "\\D3DSCache\\free");
    assert_eq!((free["tier"].as_str(), free["selected_by_default"].as_bool(), free["registry_status"].as_str()), (Some("default_selected"), Some(true), Some("covered")));
    let site = by_suffix(items, "\\D3DSCache\\site");
    assert_eq!((site["tier"].as_str(), site["skip_reason"].as_str()), (Some("blocked"), Some("protected")));
    let held = by_suffix(items, "\\D3DSCache\\held");
    assert_eq!((held["skip_reason"].as_str(), held["selected_by_default"].as_bool()), (Some("in_use"), Some(false)), "{held}");
    assert_eq!(held["probes"][0]["result"].as_str(), Some("in_use"));
    let tmp = by_suffix(items, "\\Temp\\leftover.tmp");
    assert_eq!((tmp["tier"].as_str(), tmp["selected_by_default"].as_bool()), (Some("review_only"), Some(false)));
    let previewed: BTreeSet<&str> = items.iter().filter(|i| i["selected_by_default"] == true).map(|i| i["object_id"].as_str().unwrap()).collect();

    // CLI-11/12: the non-interactive plan contains exactly the preselected rows.
    let (code, plan) = cli(&["clean", "--scan", sid, "--mode", "quarantine", "--non-interactive"], &store, &local, &rules);
    assert_eq!(code, 0, "{plan}");
    assert_eq!(plan["kind"], "plan");
    assert_eq!(plan["authorization_status"], "required", "creating a plan never authorizes it");
    let planned: BTreeSet<&str> = plan["body"]["items"].as_array().unwrap().iter().map(|i| i["candidate_id"].as_str().unwrap()).collect();
    assert_eq!(planned, previewed);

    // CAT-02/06: blocked and in-use rows are refused even by explicit ID.
    for id in [site["object_id"].as_str().unwrap(), held["object_id"].as_str().unwrap()] {
        let (code, err) = cli(&["clean", "--scan", sid, "--mode", "trash", "--candidates", id], &store, &local, &rules);
        assert_ne!(code, 0);
        assert_eq!((err["kind"].as_str(), err["code"].as_str()), (Some("error"), Some("policy_denied")), "{err}");
    }
    drop(busy);
    // Widening needs an explicit ID; a review-only row may then be planned (still unauthorized).
    let (code, plan) = cli(&["clean", "--scan", sid, "--mode", "trash", "--candidates", tmp["object_id"].as_str().unwrap()], &store, &local, &rules);
    assert_eq!(code, 0, "{plan}");
    assert_eq!(plan["body"]["items"].as_array().unwrap().len(), 1);
    // Previews take no selection and plans need a mode: nothing is guessed.
    let (code, _) = cli(&["clean", "--scan", sid, "--dry-run", "--candidates", "x"], &store, &local, &rules);
    assert_ne!(code, 0);
    let (code, _) = cli(&["clean", "--scan", sid], &store, &local, &rules);
    assert_ne!(code, 0);
    // Nothing was moved.
    assert!(d3d.join("free").join("a.bin").exists() && local.join("Temp").join("leftover.tmp").exists());

    let (code, cat) = cli(&["clean", "list"], &store, &local, &rules);
    assert_eq!(code, 0, "{cat}");
    let sys = cat["categories"].as_array().unwrap().iter().find(|c| c["category"] == "system_data").unwrap();
    assert_eq!(sys["status"], "available");
    let trash = cat["categories"].as_array().unwrap().iter().find(|c| c["category"] == "trash").unwrap();
    assert_eq!(trash["status"], "unsupported", "no rule: unsupported, never an empty 0-byte category");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_volume_that_is_not_removable_is_refused() {
    let root = scratch("volume");
    let sys = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let (code, err) = cli(&["clean", "--scan", "s_x", "--dry-run", "--volume", &sys], &root.join("store"), &root, &root.join("rules"));
    assert_ne!(code, 0);
    assert!(err["message"].as_str().unwrap().contains("system drive"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}
