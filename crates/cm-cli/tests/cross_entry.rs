//! MVP 12.5 (CLI-01, PLN-01/02): the CLI and the desktop host freeze plans with the same core.
//! A plan created by the CLI is rebuilt here exactly the way the desktop's `cleanup::create`
//! does (stored candidate set, published scan view, scan status, policy digest, `build_plan`),
//! with the same fixed inputs (plan id, creation time, selection order): the body and its digest
//! are byte-identical. An independently created plan (new id, new time, other click order) gets
//! a different digest - as it must - and the same action semantics. Real NTFS, real rule pack.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Command;

use cm_core::analysis::CandidateSet;
use cm_core::plan::{PlanInput, build_plan, parse_utc};
use cm_core::rules::Mode;
use serde_json::Value;

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-cross-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn cli(args: &[&str], store: &Path, local: &Path) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_cleanmanager"))
        .args(args)
        .arg("--store")
        .arg(store)
        .arg("--json")
        .env("CM_RESOLVE_LOCALAPPDATA", local)
        .env("CM_RESOLVE_TEMP", local.join("Temp"))
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "exactly one record: {text}");
    (out.status.code().unwrap(), serde_json::from_str(&text).unwrap())
}

/// The desktop host's plan creation (apps/desktop/src-tauri/src/cleanup.rs `create`), minus the IPC layer.
fn desktop_plan(store: &Path, set_id: &str, selected: &[String], mode: Mode, plan_id: &str, created_unix: i64) -> cm_core::plan::Plan {
    let stores = cm_store::Stores::open(store).unwrap();
    let set: CandidateSet = serde_json::from_value(stores.index.candidate_set(set_id).unwrap()).unwrap();
    let view = stores.index.view(&set.scan_id).unwrap();
    let status = stores.index.scan_status(&set.scan_id).unwrap();
    let policies = stores.audit.policies().unwrap_or_default();
    let pdigest = cm_core::c14n::digest(&serde_json::to_value(&policies).expect("policies serialize")).expect("plain JSON");
    let input = PlanInput {
        plan_id,
        scope_id: &status.scope_id,
        created_unix,
        observed_at: status.finished_at.as_deref().unwrap_or(&status.started_at),
        policy_digest: &pdigest,
    };
    build_plan(&view, &set, selected, mode, &input).unwrap()
}

/// What the plan does, independent of its id, time and action numbering.
fn semantics(body: &Value) -> (Value, Vec<Value>) {
    let mut head = body.clone();
    for k in ["plan_id", "created_at", "expires_at", "items"] {
        head.as_object_mut().unwrap().remove(k);
    }
    let mut items: Vec<Value> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            let mut i = i.clone();
            i.as_object_mut().unwrap().remove("action_id");
            i
        })
        .collect();
    items.sort_by_key(|i| i.to_string());
    (head, items)
}

#[test]
fn cli_and_desktop_core_freeze_the_same_plan_body() {
    // The desktop must still build plans the way this test mirrors.
    let desktop = include_str!("../../../apps/desktop/src-tauri/src/cleanup.rs");
    for expected in [
        "build_plan(&view, set, selected, mode, &input)",
        "scope_id: &status.scope_id,",
        "observed_at: status.finished_at.as_deref().unwrap_or(&status.started_at),",
        "cm_core::c14n::digest(&serde_json::to_value(&policies)",
        "let view = stores.index.view(&set.scan_id)?;",
    ] {
        assert!(desktop.contains(expected), "desktop plan creation changed: {expected}");
    }

    let base = scratch("plan");
    let (local, store) = (base.join("local"), base.join("store"));
    for profile in ["Default", "Profile 1"] {
        let cache = local.join("Microsoft").join("Edge").join("User Data").join(profile).join("Code Cache").join("js");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("index"), vec![1u8; 2048]).unwrap();
        std::fs::write(cache.join("f_0001"), vec![2u8; 4096]).unwrap();
    }
    let udd = local.join("Microsoft").join("Edge").join("User Data");
    let (code, scan) = cli(&["scan", "--root", udd.to_str().unwrap()], &store, &local);
    assert_eq!(code, 0, "{scan}");
    let (_, analysis) = cli(&["analyze", "--scan", scan["scan_id"].as_str().unwrap()], &store, &local);
    let set = analysis["candidate_set_id"].as_str().unwrap().to_owned();
    assert_eq!(analysis["candidate_count"], 2, "{analysis}");
    // Select both candidates explicitly: whether a rule is selected by default depends on the host's OS and Edge
    // versions matching its real-device registration, which a CI runner need not do.
    let (_, listed) = cli(&["candidates", "list", "--set", &set], &store, &local);
    let ids: Vec<&str> = listed["items"].as_array().unwrap().iter().map(|c| c["candidate_id"].as_str().unwrap()).collect();
    let (code, plan) = cli(&["plan", "create", "--set", &set, "--mode", "quarantine", "--candidates", &ids.join(",")], &store, &local);
    assert_eq!(code, 0, "{plan}");
    let body = &plan["body"];
    let mut order: Vec<String> = Vec::new();
    for i in body["items"].as_array().unwrap() {
        let c = i["candidate_id"].as_str().unwrap().to_owned();
        if !order.contains(&c) {
            order.push(c);
        }
    }
    assert_eq!(order.len(), 2);

    // Same fixed inputs → byte-identical body and digest.
    let created = parse_utc(body["created_at"].as_str().unwrap()).unwrap();
    let rebuilt = desktop_plan(&store, &set, &order, Mode::Quarantine, body["plan_id"].as_str().unwrap(), created);
    assert_eq!(serde_json::to_value(&rebuilt.body).unwrap(), *body, "same inputs, same body");
    assert_eq!(rebuilt.digest, plan["plan_digest"].as_str().unwrap(), "same body, same CM-C14N-v1 digest");

    // An independent plan from the desktop: new id, later time, other click order.
    let reversed: Vec<String> = order.iter().rev().cloned().collect();
    let independent = desktop_plan(&store, &set, &reversed, Mode::Quarantine, "p_desk_independent", created + 60);
    let independent_body = serde_json::to_value(&independent.body).unwrap();
    assert_ne!(independent.digest, rebuilt.digest, "different ids/times must not share a digest");
    assert_eq!(semantics(&independent_body), semantics(body), "same objects, identities, mode and bindings");

    // And the CLI reads the desktop's stored plan with the same digest and authorization gate.
    let stores = cm_store::Stores::open(&store).unwrap();
    stores.audit.save_plan(&independent).unwrap();
    drop(stores);
    let (_, shown) = cli(&["plan", "show", "p_desk_independent"], &store, &local);
    assert_eq!(shown["plan_digest"].as_str(), Some(independent.digest.as_str()));
    let (code, err) = cli(&["plan", "apply", "p_desk_independent"], &store, &local);
    assert_eq!((code, err["code"].as_str()), (3, Some("needs_approval")));
    let _ = std::fs::remove_dir_all(&base);
}
