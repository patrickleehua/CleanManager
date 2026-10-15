//! End-to-end `purge` flow on Windows (PUR-01..05, CLI-10/11/12): roots → read-only discovery →
//! non-interactive plan → authorization gate → apply with directory items. Fixtures live in a
//! scratch directory; Git is only used to build fixtures (the product never runs it).
#![cfg(windows)]

use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-purge-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    experimental_on(&p);
    experimental_on(&p.join("store"));
    p
}

/// D15: the gated tools under test are off by default. Opt in to experimental features in the scratch stores (the
/// directory itself and its `store` child) the way a person does, through the CLI.
fn experimental_on(store: &Path) {
    let o = Command::new(env!("CARGO_BIN_EXE_cleanmanager")).args(["experimental", "--state", "on", "--json"]).env("CM_STORE_DIR", store).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stdout));
}

fn cli(args: &[&str], store: &Path) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_cleanmanager")).args(args).arg("--store").arg(store).arg("--json").output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "exactly one record: {text}");
    (out.status.code().unwrap(), serde_json::from_str(&text).unwrap())
}

fn write(p: &Path, body: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn git(dir: &Path, args: &[&str]) -> bool {
    Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"]).args(args).output().is_ok_and(|o| o.status.success())
}

/// Make every file and directory under `root` a month old (projects are not "recent").
fn age(root: &Path) {
    let then = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400);
    let mut stack = vec![root.to_path_buf()];
    let mut all = Vec::new();
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p.clone());
            }
            all.push(p);
        }
    }
    all.push(root.to_path_buf());
    // Deepest first so a parent's time is set after its children are touched.
    all.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for p in all {
        let f = std::fs::OpenOptions::new().access_mode(0x100).custom_flags(0x0200_0000).open(&p).unwrap();
        f.set_modified(then).unwrap();
    }
}

fn item<'a>(preview: &'a Value, suffix: &str) -> Option<&'a Value> {
    preview["items"].as_array().unwrap().iter().find(|i| i["path"]["display"].as_str().unwrap().ends_with(suffix))
}

fn approve(store: &Path, plan: &Value) {
    let stores = cm_store::Stores::open(store).unwrap();
    let mode = serde_json::from_value(plan["body"]["mode"].clone()).unwrap();
    stores.audit.save_receipt(&cm_core::authz::Receipt {
        receipt_id: format!("r_{}", plan["body"]["plan_id"].as_str().unwrap()),
        plan_id: plan["body"]["plan_id"].as_str().unwrap().into(),
        plan_digest: plan["plan_digest"].as_str().unwrap().into(),
        mode,
        principal: cm_core::authz::current_principal(),
        source: cm_core::authz::ReceiptSource::InteractiveReview,
        policy_id: None,
        policy_version: None,
        created_at: cm_core::time::now_utc(),
        expires_at: plan["body"]["expires_at"].as_str().unwrap().into(),
        revoked_at: None,
    }).unwrap();
}

#[test]
fn purge_discovers_plans_and_removes_emptied_artifacts() {
    let base = scratch("flow");
    let (store, root) = (base.join("store"), base.join("projects"));
    let app = root.join("app");
    write(&app.join("Cargo.toml"), b"[package]\nname = \"app\"");
    write(&app.join("src").join("main.rs"), b"fn main() {}");
    write(&app.join("target").join("CACHEDIR.TAG"), b"Signature: 8a477f597d28d172789f06886806bc55\n");
    write(&app.join("target").join("debug").join("app.exe"), &[7u8; 4096]);
    write(&app.join("target").join("debug").join("deps").join("app.d"), b"deps");
    let web = root.join("web");
    write(&web.join("package.json"), b"{}");
    write(&web.join("node_modules").join("left-pad").join("index.js"), b"module.exports = 1");
    write(&web.join("dist").join("app.js"), b"committed bundle");
    write(&root.join("misc").join("build").join("out.o"), b"no marker above me");
    let have_git = git(&web, &["init", "-q"]) && git(&web, &["add", "package.json", "dist/app.js"]);
    age(&root);

    let (code, roots) = cli(&["purge", "roots", "add", "--path", root.to_str().unwrap()], &store);
    assert_eq!(code, 0, "{roots}");
    let ours = roots["roots"].as_array().unwrap().iter().find(|r| r["path"]["display"] == root.to_str().unwrap()).expect("root listed");
    assert_eq!((ours["source"].as_str(), ours["exists"].as_bool()), (Some("user"), Some(true)));
    let (_, again) = cli(&["purge", "roots"], &store);
    assert_eq!(again["roots"], roots["roots"], "CLI listing is the stored configuration");

    let (code, preview) = cli(&["purge", "--dry-run", "--root", root.to_str().unwrap(), "--non-interactive"], &store);
    assert_eq!(code, 0, "{preview}");
    assert_eq!((preview["kind"].as_str(), preview["command"].as_str(), preview["dry_run"].as_bool()), (Some("selection_preview"), Some("purge.preview"), Some(true)));
    let target = item(&preview, "app\\target").expect("target listed");
    assert_eq!((target["tier"].as_str(), target["selected_by_default"].as_bool(), target["rebuild_class"].as_str()), (Some("default_selected"), Some(true), Some("local_rebuild")));
    let deps = item(&preview, "web\\node_modules").expect("dependencies listed");
    assert_eq!((deps["tier"].as_str(), deps["selected_by_default"].as_bool(), deps["rebuild_class"].as_str()), (Some("review_only"), Some(false), Some("network_redownload")));
    assert!(deps["tier_reason"].as_str().unwrap().contains("network"), "{deps}");
    if have_git {
        let dist = item(&preview, "web\\dist").expect("dist listed");
        assert_eq!((dist["tier"].as_str(), dist["skip_reason"].as_str()), (Some("blocked"), Some("vcs_tracked")), "{dist}");
        assert!(dist["tier_reason"].as_str().unwrap().starts_with("vcs_tracked:1_files"), "{dist}");
    }
    assert!(item(&preview, "misc\\build").is_none(), "a build dir without a project marker is never a candidate");

    // Non-interactive: only default-selected artifacts; recoverable trash unless delete is named.
    let (code, plan) = cli(&["purge", "plan", "create", "--all-default-selected", "--non-interactive"], &store);
    assert_eq!(code, 0, "{plan}");
    assert_eq!((plan["body"]["mode"].as_str(), plan["authorization_status"].as_str()), (Some("trash"), Some("required")));
    let items = plan["body"]["items"].as_array().unwrap();
    assert!(items.iter().all(|i| !i["path"]["display"].as_str().unwrap().contains("node_modules")), "dependencies never join a non-interactive plan");
    let dirs: Vec<&str> = items.iter().filter(|i| i["kind"] == "directory").map(|i| i["path"]["display"].as_str().unwrap()).collect();
    assert_eq!(dirs.len(), 3, "target, debug and deps are directory items: {dirs:?}");
    assert!(dirs.last().unwrap().ends_with("app\\target"), "bottom-up: the artifact root is last");

    let (code, plan) = cli(&["purge", "plan", "create", "--all-default-selected", "--non-interactive", "--mode", "delete"], &store);
    assert_eq!((code, plan["body"]["mode"].as_str()), (0, Some("delete")), "{plan}");
    let plan_id = plan["body"]["plan_id"].as_str().unwrap().to_owned();
    let (code, err) = cli(&["plan", "apply", &plan_id], &store);
    assert_eq!((code, err["code"].as_str()), (3, Some("needs_approval")));
    assert!(app.join("target").join("debug").join("app.exe").exists());

    approve(&store, &plan);
    let late = app.join("target").join("debug").join("late.log");
    write(&late, b"written by a build that started after planning");
    let (code, report) = cli(&["plan", "apply", &plan_id], &store);
    assert_eq!((code, report["status"].as_str()), (5, Some("partial")), "{report}");
    let by_code = |c: &str| report["items"].as_array().unwrap().iter().filter(|i| i["code"] == c).count();
    assert_eq!(by_code("precondition_new_descendant"), 1, "{report}");
    assert_eq!(by_code("precondition_descendant_kept"), 1, "{report}");
    assert!(!app.join("target").join("debug").join("app.exe").exists() && !app.join("target").join("debug").join("deps").exists());
    assert!(!app.join("target").join("CACHEDIR.TAG").exists());
    assert_eq!(std::fs::read(&late).unwrap(), b"written by a build that started after planning", "the new file is never touched");
    assert!(web.join("node_modules").join("left-pad").join("index.js").exists() && web.join("dist").join("app.js").exists());
    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn files_tracked_after_planning_stop_the_purge() {
    let base = scratch("tracked");
    let (store, root) = (base.join("store"), base.join("projects"));
    let lib = root.join("lib");
    write(&lib.join("Cargo.toml"), b"[package]");
    write(&lib.join("target").join("out.bin"), &[1u8; 512]);
    if !git(&lib, &["init", "-q"]) || !git(&lib, &["add", "Cargo.toml"]) {
        eprintln!("skipped: git is not available to build the fixture");
        return;
    }
    age(&root);
    assert_eq!(cli(&["purge", "roots", "add", "--path", root.to_str().unwrap()], &store).0, 0);
    let (_, preview) = cli(&["purge", "scan", "--root", root.to_str().unwrap()], &store);
    assert_eq!(item(&preview, "lib\\target").unwrap()["tier"], "default_selected", "{preview}");
    let (code, plan) = cli(&["purge", "plan", "create", "--all-default-selected", "--non-interactive", "--mode", "delete"], &store);
    assert_eq!(code, 0, "{plan}");
    approve(&store, &plan);
    assert!(git(&lib, &["add", "-f", "target/out.bin"]), "someone commits the output after planning");
    let (_, report) = cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()], &store);
    let items = report["items"].as_array().unwrap();
    assert!(items.iter().any(|i| i["code"] == "protected" && i["message"].as_str().unwrap().starts_with("vcs_tracked")), "{report}");
    assert_eq!(report["succeeded"], 0, "{report}");
    assert!(lib.join("target").join("out.bin").exists());
    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn recent_projects_need_an_explicit_choice() {
    let base = scratch("recent");
    let (store, root) = (base.join("store"), base.join("projects"));
    write(&root.join("fresh").join("pyproject.toml"), b"[project]");
    write(&root.join("fresh").join(".mypy_cache").join("3.12").join("x.json"), b"{}");
    assert_eq!(cli(&["purge", "roots", "add", "--path", root.to_str().unwrap()], &store).0, 0);
    let (_, preview) = cli(&["purge", "scan"], &store);
    let cache = item(&preview, "fresh\\.mypy_cache").expect("listed");
    assert_eq!((cache["tier"].as_str(), cache["skip_reason"].as_str(), cache["selected_by_default"].as_bool()), (Some("review_only"), Some("recent"), Some(false)));
    let (code, err) = cli(&["purge", "plan", "create", "--all-default-selected", "--non-interactive"], &store);
    assert_eq!((code, err["code"].as_str()), (2, Some("invalid_argument")), "{err}");
    let id = cache["object_id"].as_str().unwrap();
    let (code, plan) = cli(&["purge", "plan", "create", "--candidates", id, "--non-interactive"], &store);
    assert_eq!((code, plan["authorization_status"].as_str()), (0, Some("required")), "explicit ids widen the selection, still needing approval");
    // Removing the root takes it out of the next discovery.
    assert_eq!(cli(&["purge", "roots", "remove", "--path", root.to_str().unwrap()], &store).0, 0);
    let (code, _) = cli(&["purge", "scan", "--root", root.to_str().unwrap()], &store);
    assert_eq!(code, 2, "an unconfigured root is refused");
    std::fs::remove_dir_all(&base).ok();
}
