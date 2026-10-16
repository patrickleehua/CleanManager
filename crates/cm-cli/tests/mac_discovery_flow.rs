//! End-to-end `purge` and `installer` flows on macOS (PUR-01..05, INC-01..05, CLI-10/11/12) through the real binary:
//! discovery → preview → plan → authorization receipt → apply, on fixtures under the canonical temporary directory.
//! `CM_RESOLVE_HOME` points the default project roots and the declared installer locations at a fixture home,
//! `CM_UNINSTALL_TEST_ROOT`/`HOME` keep the installed-app hints inside it, and `CM_PURGE_TEST_GIT=none` stands in for
//! a Mac without the Command Line Tools. The gated groups are opted in with `experimental --state on` in the scratch
//! store. The attached-image leg runs only with `CM_ALLOW_DMG_TESTS=1`.
#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    base: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Fixture {
        let tmp = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
        let base = tmp.join(format!("cm-cli-macdisc-{tag}-{}-{}-{nanos:08x}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        for d in ["home", "fakeroot", "store"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        let f = Fixture { base };
        let (code, v) = f.cli(&["experimental", "--state", "on"], &[]);
        assert_eq!(code, 0, "{v}");
        f
    }
    fn home(&self) -> PathBuf {
        self.base.join("home")
    }
    fn store(&self) -> PathBuf {
        self.base.join("store")
    }
    fn cli(&self, args: &[&str], env: &[(&str, &str)]) -> (i32, Value) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cleanmanager"));
        cmd.args(args)
            .arg("--store")
            .arg(self.store())
            .arg("--json")
            .env("CM_RESOLVE_HOME", self.home())
            .env("CM_UNINSTALL_TEST_ROOT", self.base.join("fakeroot"))
            .env("CM_UNINSTALL_TEST_HOME", self.home())
            .env_remove("CM_INSTALLER_LOCATIONS")
            .env_remove("CM_PURGE_TEST_GIT")
            .env_remove("HOMEBREW_CACHE");
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.output().unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        assert_eq!(text.lines().count(), 1, "exactly one record: {text} / {}", String::from_utf8_lossy(&out.stderr));
        (out.status.code().unwrap(), serde_json::from_str(&text).unwrap())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn write(p: &Path, body: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Make every file and directory under `root` a month old (projects are not "recent"); deepest first.
fn age(root: &Path) {
    let then = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400);
    let mut all = vec![root.to_path_buf()];
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().filter_map(Result::ok) {
            let p = e.path();
            if e.file_type().unwrap().is_dir() {
                stack.push(p.clone());
            }
            all.push(p);
        }
    }
    all.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for p in all {
        std::fs::File::open(&p).unwrap().set_modified(then).unwrap();
    }
}

/// Git for building fixtures, when this Mac has a developer directory (`/usr/bin/git` is otherwise an install shim).
fn have_git() -> bool {
    Command::new("/usr/bin/xcode-select").arg("-p").output().is_ok_and(|o| o.status.success())
}

fn git(dir: &Path, args: &[&str]) -> bool {
    Command::new("/usr/bin/git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}

fn item<'a>(preview: &'a Value, suffix: &str) -> Option<&'a Value> {
    preview["items"].as_array().unwrap().iter().find(|i| i["path"]["display"].as_str().unwrap().ends_with(suffix))
}

fn approve(store: &Path, plan: &Value) {
    let stores = cm_store::Stores::open(store).unwrap();
    let mode = serde_json::from_value(plan["body"]["mode"].clone()).unwrap();
    stores
        .audit
        .save_receipt(&cm_core::authz::Receipt {
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
        })
        .unwrap();
}

fn report_codes(report: &Value) -> Vec<(String, String)> {
    report["items"].as_array().unwrap().iter().map(|i| (i["code"].as_str().unwrap_or("").to_owned(), i["message"].as_str().unwrap_or("").to_owned())).collect()
}

// ---------------------------------------------------------------------------------------------
// purge

#[test]
fn purge_discovers_macos_artifacts_in_default_roots_and_removes_planned_ones() {
    let fx = Fixture::new("purge");
    let projects = fx.home().join("Projects");
    let app = projects.join("app");
    write(&app.join("Cargo.toml"), b"[package]\nname = \"app\"");
    write(&app.join("src").join("main.rs"), b"fn main() {}");
    write(&app.join("target").join("CACHEDIR.TAG"), b"Signature: 8a477f597d28d172789f06886806bc55\n");
    write(&app.join("target").join("debug").join("app"), &[7u8; 4096]);
    let web = projects.join("web");
    write(&web.join("package.json"), b"{}");
    write(&web.join("node_modules").join("left-pad").join("index.js"), b"module.exports = 1");
    let swift = projects.join("SwiftTool");
    write(&swift.join("Package.swift"), b"// swift-tools-version:5.9");
    write(&swift.join(".build").join("debug").join("tool.o"), &[1u8; 1024]);
    let xc = projects.join("iOSApp");
    write(&xc.join("iOSApp.xcodeproj").join("project.pbxproj"), b"// !$*UTF8*$!");
    write(&xc.join("DerivedData").join("Build").join("Intermediates.noindex").join("x.o"), &[2u8; 1024]);
    // Dot folders are never searched for projects, and a build folder without a marker above it is not an artifact.
    write(&projects.join(".archive").join("old").join("Cargo.toml"), b"[package]");
    write(&projects.join(".archive").join("old").join("target").join("o"), b"x");
    write(&projects.join("misc").join("build").join("out.o"), b"no marker above me");
    age(&fx.home());

    let (code, roots) = fx.cli(&["purge", "roots"], &[]);
    assert_eq!(code, 0, "{roots}");
    let listed: Vec<(&str, &str)> = roots["roots"].as_array().unwrap().iter().map(|r| (r["path"]["display"].as_str().unwrap(), r["source"].as_str().unwrap())).collect();
    assert_eq!(listed, [(projects.to_str().unwrap(), "default")], "only existing default roots under the resolved home");

    let (code, preview) = fx.cli(&["purge", "--dry-run", "--non-interactive"], &[]);
    assert_eq!(code, 0, "{preview}");
    assert_eq!((preview["command"].as_str(), preview["dry_run"].as_bool()), (Some("purge.preview"), Some(true)));
    for (suffix, tier, selected, rebuild) in [
        ("app/target", "default_selected", true, "local_rebuild"),
        ("web/node_modules", "review_only", false, "network_redownload"),
        ("SwiftTool/.build", "default_selected", true, "local_rebuild"),
        ("iOSApp/DerivedData", "default_selected", true, "local_rebuild"),
    ] {
        let i = item(&preview, suffix).unwrap_or_else(|| panic!("{suffix} listed: {preview}"));
        assert_eq!((i["tier"].as_str(), i["selected_by_default"].as_bool(), i["rebuild_class"].as_str()), (Some(tier), Some(selected), Some(rebuild)), "{suffix}: {i}");
    }
    assert!(item(&preview, "old/target").is_none(), "dot folders are not searched: {preview}");
    assert!(item(&preview, "misc/build").is_none());

    // The same root named explicitly; a root that is not configured is refused.
    let (code, _) = fx.cli(&["purge", "scan", "--root", projects.to_str().unwrap()], &[]);
    assert_eq!(code, 0);
    let (code, _) = fx.cli(&["purge", "scan", "--root", fx.base.to_str().unwrap()], &[]);
    assert_eq!(code, 2, "an unconfigured root is refused");

    let (code, plan) = fx.cli(&["purge", "plan", "create", "--all-default-selected", "--non-interactive", "--mode", "delete"], &[]);
    assert_eq!(code, 0, "{plan}");
    assert_eq!(plan["authorization_status"].as_str(), Some("required"));
    let items = plan["body"]["items"].as_array().unwrap();
    assert!(items.iter().all(|i| !i["path"]["display"].as_str().unwrap().contains("node_modules")), "dependencies never join a non-interactive plan");
    let plan_id = plan["body"]["plan_id"].as_str().unwrap().to_owned();
    let (code, err) = fx.cli(&["plan", "apply", &plan_id], &[]);
    assert_eq!((code, err["code"].as_str()), (3, Some("needs_approval")), "{err}");
    approve(&fx.store(), &plan);
    let (code, report) = fx.cli(&["plan", "apply", &plan_id], &[]);
    assert_eq!((code, report["status"].as_str()), (0, Some("completed")), "{report}");
    assert!(!app.join("target").exists() && !swift.join(".build").exists() && !xc.join("DerivedData").exists(), "{report}");
    assert!(app.join("src").join("main.rs").exists() && web.join("node_modules").join("left-pad").join("index.js").exists());
}

#[test]
fn purge_recent_projects_need_an_explicit_choice() {
    let fx = Fixture::new("recent");
    let root = fx.home().join("code");
    write(&root.join("fresh").join("pyproject.toml"), b"[project]");
    write(&root.join("fresh").join(".mypy_cache").join("3.12").join("x.json"), b"{}");
    let (_, preview) = fx.cli(&["purge", "scan"], &[]);
    let cache = item(&preview, "fresh/.mypy_cache").unwrap_or_else(|| panic!("listed: {preview}"));
    assert_eq!((cache["tier"].as_str(), cache["skip_reason"].as_str(), cache["selected_by_default"].as_bool()), (Some("review_only"), Some("recent"), Some(false)));
    let (code, err) = fx.cli(&["purge", "plan", "create", "--all-default-selected", "--non-interactive"], &[]);
    assert_eq!((code, err["code"].as_str()), (2, Some("invalid_argument")), "{err}");
    let (code, plan) = fx.cli(&["purge", "plan", "create", "--candidates", cache["object_id"].as_str().unwrap(), "--non-interactive"], &[]);
    assert_eq!((code, plan["body"]["mode"].as_str(), plan["authorization_status"].as_str()), (0, Some("trash"), Some("required")), "{plan}");
    // Removing the default root takes it out of the next discovery; adding it back restores it.
    assert_eq!(fx.cli(&["purge", "roots", "remove", "--path", root.to_str().unwrap()], &[]).0, 0);
    let (code, _) = fx.cli(&["purge", "scan"], &[]);
    assert_eq!(code, 2, "no roots left");
    let (code, roots) = fx.cli(&["purge", "roots", "add", "--path", root.to_str().unwrap()], &[]);
    assert_eq!((code, roots["roots"][0]["source"].as_str()), (0, Some("default")), "{roots}");
}

#[test]
fn purge_git_gate_with_and_without_git() {
    if !have_git() {
        eprintln!("skipped: no developer directory (xcode-select -p), so git cannot build the fixture");
        return;
    }
    let fx = Fixture::new("git");
    let root = fx.home().join("workspace");
    let lib = root.join("lib");
    write(&lib.join("Cargo.toml"), b"[package]");
    write(&lib.join(".gitignore"), b"/target/\n");
    write(&lib.join("target").join("out.bin"), &[1u8; 512]);
    let site = root.join("site");
    write(&site.join("package.json"), b"{}");
    write(&site.join("dist").join("app.js"), b"committed bundle");
    write(&site.join("build").join("hand.js"), b"untracked and not ignored");
    let plain = root.join("plain");
    write(&plain.join("Cargo.toml"), b"[package]");
    write(&plain.join("target").join("o"), b"x");
    assert!(git(&lib, &["init", "-q"]) && git(&lib, &["add", "Cargo.toml", ".gitignore"]));
    assert!(git(&site, &["init", "-q"]) && git(&site, &["add", "package.json", "dist/app.js"]));
    age(&fx.home());

    let (_, preview) = fx.cli(&["purge", "scan"], &[]);
    let target = item(&preview, "lib/target").unwrap_or_else(|| panic!("{preview}"));
    assert_eq!((target["tier"].as_str(), target["selected_by_default"].as_bool()), (Some("default_selected"), Some(true)), "ignored output: {target}");
    let dist = item(&preview, "site/dist").unwrap();
    assert_eq!((dist["tier"].as_str(), dist["skip_reason"].as_str()), (Some("blocked"), Some("vcs_tracked")), "{dist}");
    assert!(dist["tier_reason"].as_str().unwrap().starts_with("vcs_tracked:1_files"), "{dist}");
    let build = item(&preview, "site/build").unwrap();
    assert_eq!((build["skip_reason"].as_str(), build["selected_by_default"].as_bool()), (Some("vcs_unknown"), Some(false)), "unignored untracked files may be source: {build}");

    // Without Git, artifacts inside working copies are unknown and never selected; plain folders are unaffected.
    let (_, blind) = fx.cli(&["purge", "scan"], &[("CM_PURGE_TEST_GIT", "none")]);
    let t = item(&blind, "lib/target").unwrap();
    assert_eq!((t["skip_reason"].as_str(), t["selected_by_default"].as_bool()), (Some("vcs_unknown"), Some(false)), "{t}");
    assert!(t["tier_reason"].as_str().unwrap().contains("Git"), "{t}");
    assert_eq!(item(&blind, "plain/target").unwrap()["selected_by_default"], true);

    // Plan with Git, then: (1) Git unavailable at apply time stops the item; (2) committed after planning stops it.
    let (_, preview) = fx.cli(&["purge", "scan", "--root", root.to_str().unwrap()], &[]);
    let id = item(&preview, "lib/target").unwrap()["object_id"].as_str().unwrap().to_owned();
    for (env, expect) in [(&[("CM_PURGE_TEST_GIT", "none")][..], "vcs_unknown"), (&[][..], "vcs_tracked")] {
        let (code, plan) = fx.cli(&["purge", "plan", "create", "--candidates", &id, "--non-interactive", "--mode", "delete"], &[]);
        assert_eq!(code, 0, "{plan}");
        approve(&fx.store(), &plan);
        if expect == "vcs_tracked" {
            assert!(git(&lib, &["add", "-f", "target/out.bin"]), "someone commits the output after planning");
        }
        let (_, report) = fx.cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()], env);
        let codes = report_codes(&report);
        assert!(codes.iter().any(|(c, m)| c == "protected" && m.starts_with(expect)), "{expect}: {report}");
        assert_eq!(report["succeeded"], 0, "{report}");
        assert!(lib.join("target").join("out.bin").exists());
    }
}

// ---------------------------------------------------------------------------------------------
// installer

fn synthetic_dmg(tag: u8) -> Vec<u8> {
    let mut b = vec![tag; 2048];
    let mut trailer = vec![0u8; 512];
    trailer[..4].copy_from_slice(b"koly");
    b.extend_from_slice(&trailer);
    b
}

fn synthetic_pkg() -> Vec<u8> {
    let mut b = b"xar!".to_vec();
    b.resize(512, 0x5a);
    b
}

fn names(preview: &Value) -> Vec<String> {
    let mut v: Vec<String> = preview["items"].as_array().unwrap().iter().map(|i| i["path"]["display"].as_str().unwrap().rsplit('/').next().unwrap().to_owned()).collect();
    v.sort();
    v
}

#[test]
fn installers_in_declared_locations_are_review_only_and_identity_bound() {
    let fx = Fixture::new("inst");
    let (dl, desk, brew) = (fx.home().join("Downloads"), fx.home().join("Desktop"), fx.home().join("Library/Caches/Homebrew/downloads"));
    write(&dl.join("Tool.dmg"), &synthetic_dmg(1));
    write(&dl.join("Tool.pkg"), &synthetic_pkg());
    write(&dl.join("notes.pdf"), b"%PDF-1.7 just a document, nothing to install here at all......................");
    write(&dl.join("setup.dmg"), b"plain text renamed to setup.dmg, which proves nothing about it at all........");
    write(&dl.join("report.txt"), b"quarterly numbers");
    write(&dl.join("Fixture.app").join("Contents").join("Info.plist"), b"<plist/>");
    let zip = |out: &str, what: &str| Command::new("/usr/bin/zip").current_dir(&dl).args(["-q", "-r", out, what]).output().is_ok_and(|o| o.status.success());
    let zipped = zip("docs.zip", "report.txt") && zip("Fixture.app.zip", "Fixture.app");
    std::fs::remove_dir_all(dl.join("Fixture.app")).unwrap();
    write(&desk.join("Other.pkg"), &synthetic_pkg());
    write(&brew.join("0123abcd--tool-1.0.dmg"), &synthetic_dmg(2));

    let (code, locs) = fx.cli(&["installer", "locations"], &[]);
    assert_eq!(code, 0, "{locs}");
    let labels: Vec<&str> = locs["locations"].as_array().unwrap().iter().map(|l| l["source_label"].as_str().unwrap()).collect();
    assert_eq!(labels, ["downloads", "desktop", "homebrew_downloads"], "{locs}");

    let (code, preview) = fx.cli(&["installer", "--dry-run", "--non-interactive"], &[]);
    assert_eq!((code, preview["command"].as_str()), (0, Some("installer.preview")), "{preview}");
    let mut expect = vec!["0123abcd--tool-1.0.dmg", "Other.pkg", "Tool.dmg", "Tool.pkg"];
    if zipped {
        expect.push("Fixture.app.zip");
        expect.sort();
    }
    assert_eq!(names(&preview), expect, "documents, archives of documents and renamed files are not candidates: {preview}");
    for i in preview["items"].as_array().unwrap() {
        assert_eq!((i["tier"].as_str(), i["selected_by_default"].as_bool(), i["skip_reason"].as_str()), (Some("review_only"), Some(false), None), "{i}");
    }
    assert_eq!(item(&preview, "Other.pkg").unwrap()["source_label"], "desktop");
    assert_eq!(item(&preview, "tool-1.0.dmg").unwrap()["source_label"], "homebrew_downloads");
    let pkg_notes = item(&preview, "Tool.pkg").unwrap()["notes"].to_string();
    assert!(pkg_notes.contains("pkgutil"), "{pkg_notes}");
    if zipped {
        assert!(item(&preview, "Fixture.app.zip").unwrap()["notes"].to_string().contains("archive by name only"), "{preview}");
    }
    let (_, only) = fx.cli(&["installer", "scan", "--source", "desktop"], &[]);
    assert_eq!(names(&only), ["Other.pkg"], "a source filter leaves the other sources out");

    // Machines never get an automatic or by-source selection.
    assert_eq!(fx.cli(&["installer", "plan", "create", "--all-default-selected", "--non-interactive"], &[]).0, 2);
    let (_, preview) = fx.cli(&["installer", "scan"], &[]);
    let dmg = item(&preview, "Downloads/Tool.dmg").unwrap()["object_id"].as_str().unwrap().to_owned();
    let (code, plan) = fx.cli(&["installer", "plan", "create", "--candidates", &dmg, "--mode", "delete", "--non-interactive"], &[]);
    assert_eq!((code, plan["authorization_status"].as_str()), (0, Some("required")), "{plan}");
    // A new download replaces the planned file under the same name after planning.
    std::fs::remove_file(dl.join("Tool.dmg")).unwrap();
    write(&dl.join("Tool.dmg"), &synthetic_dmg(9));
    approve(&fx.store(), &plan);
    let (_, report) = fx.cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()], &[]);
    assert_eq!((report["status"].as_str(), report["items"][0]["code"].as_str()), (Some("partial"), Some("precondition_identity_changed")), "{report}");
    assert_eq!(std::fs::read(dl.join("Tool.dmg")).unwrap(), synthetic_dmg(9), "the new download survives");

    // An unchanged installer is removed once approved.
    let (_, preview) = fx.cli(&["installer", "scan"], &[]);
    let pkg = item(&preview, "Downloads/Tool.pkg").unwrap()["object_id"].as_str().unwrap().to_owned();
    let (_, plan) = fx.cli(&["installer", "plan", "create", "--candidates", &pkg, "--mode", "delete", "--non-interactive"], &[]);
    approve(&fx.store(), &plan);
    let (code, report) = fx.cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()], &[]);
    assert_eq!((code, report["status"].as_str()), (0, Some("completed")), "{report}");
    assert!(!dl.join("Tool.pkg").exists());
}

#[test]
fn installers_open_elsewhere_are_skipped() {
    let fx = Fixture::new("busy");
    let dl = fx.home().join("Downloads");
    write(&dl.join("busy.pkg"), &synthetic_pkg());
    let holder = std::fs::File::open(dl.join("busy.pkg")).unwrap();
    let (_, preview) = fx.cli(&["installer", "scan"], &[]);
    let i = &preview["items"][0];
    assert_eq!((i["skip_reason"].as_str(), i["selected_by_default"].as_bool()), (Some("in_use"), Some(false)), "{preview}");
    let (code, err) = fx.cli(&["installer", "plan", "create", "--candidates", i["object_id"].as_str().unwrap(), "--non-interactive"], &[]);
    assert_eq!((code, err["code"].as_str()), (3, Some("policy_denied")), "an in-use installer cannot be planned now: {err}");
    drop(holder);
}

#[test]
fn attached_images_are_skipped_at_discovery_and_at_execution() {
    if std::env::var_os("CM_ALLOW_DMG_TESTS").is_none() {
        eprintln!("skipped: set CM_ALLOW_DMG_TESTS=1 to attach a disposable disk image");
        return;
    }
    let fx = Fixture::new("dmg");
    let dl = fx.home().join("Downloads");
    std::fs::create_dir_all(&dl).unwrap();
    let image = dl.join("Fixture.dmg");
    let mnt = fx.base.join("mnt");
    std::fs::create_dir_all(&mnt).unwrap();
    let hdiutil = |args: &[&str], path: &Path| Command::new("/usr/bin/hdiutil").args(args).arg(path).status().unwrap().success();
    let src = fx.base.join("src");
    write(&src.join("ReadMe.txt"), b"disposable image content");
    // A compressed UDIF image, the format downloads come in.
    assert!(Command::new("/usr/bin/hdiutil").args(["create", "-quiet", "-volname", "cmdisc", "-format", "UDZO", "-srcfolder"]).arg(&src).arg("-o").arg(&image).status().unwrap().success());
    let attach = || assert!(Command::new("/usr/bin/hdiutil").args(["attach", "-quiet", "-nobrowse", "-mountpoint"]).arg(&mnt).arg(&image).status().unwrap().success());

    let listing = Command::new("/bin/ls").arg("-la").arg(&dl).output().unwrap();
    let (_, detached) = fx.cli(&["installer", "scan"], &[]);
    let d = item(&detached, "Fixture.dmg").unwrap_or_else(|| panic!("detached image listed: {detached}
{}", String::from_utf8_lossy(&listing.stdout)));
    assert_eq!(d["skip_reason"], Value::Null, "{d}");

    attach();
    let (_, preview) = fx.cli(&["installer", "scan"], &[]);
    hdiutil(&["detach", "-quiet", "-force"], &mnt);
    let i = item(&preview, "Fixture.dmg").unwrap_or_else(|| panic!("attached image listed: {preview}"));
    assert_eq!((i["skip_reason"].as_str(), i["selected_by_default"].as_bool()), (Some("mounted"), Some(false)), "{i}");
    assert!(i["notes"].to_string().contains("hdiutil detach"), "{i}");

    // Planned while detached, attached again before apply: skipped, never removed.
    let (_, preview) = fx.cli(&["installer", "scan"], &[]);
    let id = item(&preview, "Fixture.dmg").unwrap()["object_id"].as_str().unwrap().to_owned();
    let (code, plan) = fx.cli(&["installer", "plan", "create", "--candidates", &id, "--mode", "delete", "--non-interactive"], &[]);
    assert_eq!(code, 0, "{plan}");
    approve(&fx.store(), &plan);
    attach();
    let (_, report) = fx.cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()], &[]);
    hdiutil(&["detach", "-quiet", "-force"], &mnt);
    assert!(report_codes(&report).iter().any(|(c, m)| c == "protected" && m.starts_with("mounted_image")), "{report}");
    assert!(image.exists());
}
