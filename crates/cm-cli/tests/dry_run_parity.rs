//! Parity 8.1 (CLI-11): `--dry-run` is the plan preview. The same scan previewed and then planned
//! selects the same objects; a preview changes nothing and consumes no authorization (no plan, no
//! receipt, no operation is recorded); its object IDs convert directly into a plan; objects whose
//! state changed after the scan are listed as `stale_items` and left out of the plan, and when
//! every selected object changed the result is `plan_stale` (exit 6). Real NTFS, fixture rules.
#![cfg(windows)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-dryrun-{}-{name}", std::process::id()));
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

/// One enabled, default-selected directory rule under the user cache root (no probes: deterministic).
fn write_pack(dir: &Path) {
    std::fs::create_dir_all(dir.join("windows")).unwrap();
    std::fs::create_dir_all(dir.join("registry")).unwrap();
    let id = "test.d3d.folders";
    let rule = json!({
        "schema_version": "2.0", "rule_id": id, "version": "1.0.0", "status": "enabled", "platforms": ["windows"],
        "category": "system_data", "max_tier": "default_selected", "rebuild_class": "local_rebuild", "owner": {"kind": "system", "id": "windows.directx"},
        "root_resolver": "user_cache",
        "match": {"object_type": "directory", "relative_components": [], "name_glob": "*", "extensions": [], "require_cachedir_tag": false,
                  "archive_payload": false, "max_depth": 0},
        "exclude": [], "probes": [],
        "thresholds": {"min_age_days": null, "age_basis": null, "min_size_bytes": null, "retention_days": null},
        "action_kind": "filesystem", "adapter_ref": null, "allowed_modes": ["trash", "quarantine"], "requires_admin": false,
        "protect_descendants": true, "follow_links": false,
        "impact": {"summary": "fixture", "regeneration": "fixture", "offline_effect": "fixture"},
        "tested_registry_ref": format!("{id}@1.0.0"), "evidence_refs": ["fixture"],
    });
    let registry = json!({
        "schema_version": "1.0", "registry_id": format!("{id}@1.0.0"), "rule_id": id, "rule_version": "1.0.0", "synthetic": false,
        "subject": {"kind": "system", "id": "windows.directx", "name": "fixture"}, "version_range": {"min_inclusive": "10.0", "max_exclusive": "99.0"},
        "platforms": [{"platform": "windows", "os_versions": ["10.0"], "architectures": ["x64"]}], "tested_at": "2026-10-15",
        "result": "passed", "fixtures": {"positive": ["p"], "negative": ["n"]}, "known_effects": [], "evidence_refs": ["fixture"],
        "thresholds": {"min_age_days": null, "age_basis": null, "min_size_bytes": null, "retention_days": null},
    });
    std::fs::write(dir.join("windows").join(format!("{id}.json")), rule.to_string()).unwrap();
    std::fs::write(dir.join("registry").join(format!("{id}@1.0.0.json")), registry.to_string()).unwrap();
}

fn selected(preview: &Value) -> BTreeSet<String> {
    preview["items"].as_array().unwrap().iter().filter(|i| i["selected_by_default"] == true).map(|i| i["object_id"].as_str().unwrap().to_owned()).collect()
}

fn planned(plan: &Value) -> BTreeSet<String> {
    plan["body"]["items"].as_array().unwrap().iter().map(|i| i["candidate_id"].as_str().unwrap().to_owned()).collect()
}

fn audit_counts(store: &Path) -> (usize, usize) {
    let stores = cm_store::Stores::open(store).unwrap();
    (stores.audit.plans_page(500, None).unwrap().0.len(), stores.audit.operations_page(500, None).unwrap().0.len())
}

#[test]
fn dry_run_equals_the_plan_consumes_nothing_and_marks_changed_rows_stale() {
    let root = scratch("parity");
    let (local, store, rules) = (root.join("local"), root.join("store"), root.join("rules"));
    write_pack(&rules);
    let d3d = local.join("D3DSCache");
    for dir in ["one", "two", "three"] {
        std::fs::create_dir_all(d3d.join(dir)).unwrap();
        std::fs::write(d3d.join(dir).join("blob.bin"), vec![7u8; 4096]).unwrap();
    }
    let (code, scan) = cli(&["scan", "--root", local.to_str().unwrap()], &store, &local, &rules);
    assert_eq!(code, 0, "{scan}");
    let sid = scan["scan_id"].as_str().unwrap().to_owned();

    // The preview: same analysis as planning, nothing recorded or authorized.
    let (code, preview) = cli(&["clean", "--scan", &sid, "--dry-run", "--non-interactive"], &store, &local, &rules);
    assert_eq!(code, 0, "{preview}");
    assert_eq!((preview["dry_run"].as_bool(), preview["convertible_to_plan"].as_bool()), (Some(true), Some(true)));
    let previewed = selected(&preview);
    assert_eq!(previewed.len(), 3, "{preview}");
    assert_eq!(audit_counts(&store), (0, 0), "a dry run creates no plan, receipt or operation");
    let (code, again) = cli(&["clean", "--scan", &sid, "--dry-run", "--non-interactive"], &store, &local, &rules);
    assert_eq!((code, selected(&again)), (0, previewed.clone()), "previews are repeatable");

    // Same filter logic: the non-interactive plan holds exactly the preselected rows.
    let (code, plan) = cli(&["clean", "--scan", &sid, "--mode", "quarantine", "--non-interactive"], &store, &local, &rules);
    assert_eq!(code, 0, "{plan}");
    assert_eq!(planned(&plan), previewed);
    assert_eq!(plan["stale_items"], json!([]));
    assert_eq!(plan["authorization_status"], "required", "a plan from a preview is not authorized");

    // Convertible: the preview's object IDs become a plan as they are.
    let ids: Vec<&str> = previewed.iter().map(String::as_str).collect();
    let (code, explicit) = cli(&["clean", "--scan", &sid, "--mode", "quarantine", "--candidates", &ids.join(","), "--non-interactive"], &store, &local, &rules);
    assert_eq!(code, 0, "{explicit}");
    assert_eq!(planned(&explicit), previewed);

    // State changed after the scan: one previewed row was deleted.
    let by_dir = |name: &str| {
        preview["items"].as_array().unwrap().iter().find(|i| i["path"]["display"].as_str().unwrap().ends_with(&format!("\\{name}"))).unwrap()["object_id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let (two, three) = (by_dir("two"), by_dir("three"));
    std::fs::remove_dir_all(d3d.join("two")).unwrap();
    let (code, plan) = cli(&["clean", "--scan", &sid, "--mode", "quarantine", "--non-interactive"], &store, &local, &rules);
    assert_eq!(code, 0, "{plan}");
    let expected: BTreeSet<String> = previewed.iter().filter(|i| **i != two).cloned().collect();
    assert_eq!(planned(&plan), expected, "the changed row is not in the plan");
    let stale = plan["stale_items"].as_array().unwrap();
    assert_eq!(stale.len(), 1, "{plan}");
    assert_eq!((stale[0]["object_id"].as_str(), stale[0]["reason"].as_str()), (Some(two.as_str()), Some("missing")));

    // Every requested row changed: plan_stale, exit 6, nothing created.
    let plans_before = audit_counts(&store).0;
    let (code, err) = cli(&["clean", "--scan", &sid, "--mode", "trash", "--candidates", &two, "--non-interactive"], &store, &local, &rules);
    assert_eq!((code, err["code"].as_str()), (6, Some("plan_stale")), "{err}");
    assert_eq!(audit_counts(&store).0, plans_before);
    // An unchanged row still converts.
    let (code, one_plan) = cli(&["clean", "--scan", &sid, "--mode", "trash", "--candidates", &three, "--non-interactive"], &store, &local, &rules);
    assert_eq!((code, planned(&one_plan).len()), (0, 1), "{one_plan}");
    assert_eq!(audit_counts(&store).1, 0, "planning never applies anything");
    assert!(d3d.join("one").join("blob.bin").exists() && d3d.join("three").join("blob.bin").exists());
    let _ = std::fs::remove_dir_all(&root);
}
