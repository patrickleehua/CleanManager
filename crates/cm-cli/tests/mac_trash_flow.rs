//! Manual trash on macOS through the real binary (parity 6.2, MTR-01..05): scan → `trash inspect` → `trash` (plan only,
//! refused up front as `trash_unavailable` when the volume has no Trash) → needs_approval → review receipt → apply →
//! `trash list` → `trash purge` (separate plan) → apply. Receipts are written through `cm_store` as the terminal review
//! would (it needs a TTY). Everything lives under the canonical temporary directory; listing tests use a fixture Trash
//! through the `CM_RESOLVE_HOME` redirect (docs/platforms/macos.md §11). The leg that moves a file into the user's real
//! Trash runs only with `CM_ALLOW_MAC_DEVICE_TESTS=1` or on a hosted runner (`GITHUB_ACTIONS=true`) and removes what
//! it put there. Gated commands need experimental features, turned on in the scratch store.
#![cfg(target_os = "macos")]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicUsize, Ordering};

use cm_core::analysis::IndexView;
use serde_json::Value;

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn device_tests_allowed() -> bool {
    std::env::var("CM_ALLOW_MAC_DEVICE_TESTS").is_ok_and(|v| v == "1") || std::env::var("GITHUB_ACTIONS").is_ok_and(|v| v == "true")
}

fn is_root() -> bool {
    Command::new("/usr/bin/id").arg("-u").output().is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
}

/// `data` (scan scope), `store` and `home` (the redirected home whose `.Trash` is listed) under the canonical temp dir.
struct Fixture {
    base: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Fixture {
        let tmp = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
        let base = tmp.join(format!("cm-cli-mactrash-{tag}-{}-{}-{nanos:08x}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(base.join("data")).unwrap();
        std::fs::create_dir_all(base.join("home")).unwrap();
        let fx = Fixture { base };
        let o = Command::new(env!("CARGO_BIN_EXE_cleanmanager")).args(["experimental", "--state", "on", "--json"]).env("CM_STORE_DIR", fx.store()).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stdout));
        fx
    }
    fn data(&self) -> PathBuf {
        self.base.join("data")
    }
    fn store(&self) -> PathBuf {
        self.base.join("store")
    }
    fn home(&self) -> PathBuf {
        self.base.join("home")
    }
    fn bin(&self) -> PathBuf {
        self.home().join(".Trash")
    }

    /// One record from the CLI. `home: false` lists the user's real Trash (device tests only).
    fn run(&self, args: &[&str], home: bool) -> (i32, Value) {
        let mut c = Command::new(env!("CARGO_BIN_EXE_cleanmanager"));
        c.args(args);
        // `trash list` reads only the Trash and takes no store.
        if args != ["trash", "list"] {
            c.arg("--store").arg(self.store());
        }
        c.arg("--json").env("CM_STORE_DIR", self.store());
        if home {
            c.env("CM_RESOLVE_HOME", self.home());
        } else {
            c.env_remove("CM_RESOLVE_HOME");
        }
        let out = c.output().unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        assert_eq!(text.lines().count(), 1, "exactly one record for {args:?}: {text}\n{}", String::from_utf8_lossy(&out.stderr));
        (out.status.code().unwrap(), serde_json::from_str(&text).unwrap())
    }
    fn cli(&self, args: &[&str]) -> (i32, Value) {
        self.run(args, true)
    }

    fn scan(&self) -> String {
        let (code, scan) = self.cli(&["scan", "--root", self.data().to_str().unwrap()]);
        assert_eq!(code, 0, "{scan}");
        scan["scan_id"].as_str().unwrap().to_owned()
    }

    fn entry(&self, scan: &str, path: &Path) -> String {
        let stores = cm_store::Stores::open(&self.store()).unwrap();
        let view = stores.index.view(scan).unwrap();
        let want = path.file_name().unwrap().to_string_lossy().into_owned();
        let parent = path.parent().unwrap().file_name().unwrap().to_string_lossy().into_owned();
        let found = view.facts.keys().copied().find(|&e| view.name(e).lossy() == want && view.parent(e).is_some_and(|p| view.name(p).lossy().ends_with(&parent)));
        found.unwrap_or_else(|| panic!("{} indexed", path.display())).to_string()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(self.bin(), std::fs::Permissions::from_mode(0o700));
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn write(p: &Path, bytes: usize) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, vec![7u8; bytes]).unwrap();
}

fn approve(store: &Path, plan: &Value) {
    let stores = cm_store::Stores::open(store).unwrap();
    let kind: cm_core::plan::v2::PlanKind = serde_json::from_value(plan["body"]["plan_kind"].clone()).unwrap();
    stores
        .audit
        .save_kind_receipt(&cm_core::authz::KindReceipt {
            receipt_id: format!("r_{}", plan["body"]["plan_id"].as_str().unwrap()),
            plan_id: plan["body"]["plan_id"].as_str().unwrap().into(),
            plan_digest: plan["plan_digest"].as_str().unwrap().into(),
            plan_kind: kind,
            principal: cm_core::authz::current_principal(),
            source: cm_core::authz::ReceiptSource::InteractiveReview,
            created_at: cm_core::time::now_utc(),
            expires_at: plan["body"]["expires_at"].as_str().unwrap().into(),
            revoked_at: None,
        })
        .unwrap();
}

/// A trash plan either exists (the volume has a Trash) or was refused up front as `trash_unavailable`.
fn plan_or_unavailable(code: i32, plan: &Value) -> bool {
    if code == 0 {
        assert_eq!((plan["body"]["plan_kind"].as_str(), plan["body"]["policy_matchable"].as_bool()), (Some("manual_selection"), Some(false)), "{plan}");
        assert_eq!(plan["authorization_status"], "required");
        let items = plan["body"]["items"].as_array().unwrap();
        assert!(items.iter().all(|a| a["action_type"] == "fs_trash" && a["target_location"] == "original"), "{plan}");
        return true;
    }
    assert_eq!((code, plan["code"].as_str()), (7, Some("unsupported")), "{plan}");
    let msg = plan["message"].as_str().unwrap();
    assert!(msg.starts_with("trash_unavailable: ") && msg.contains("no permanent-delete alternative"), "{msg}");
    false
}

#[test]
fn inspect_and_plan_on_fixtures_without_touching_anything() {
    let fx = Fixture::new("plan");
    let data = fx.data();
    write(&data.join("big.bin"), 64 * 1024);
    write(&data.join("renders").join("a.png"), 1000);
    write(&data.join("home").join(".ssh").join("id_ed25519"), 10);
    write(&data.join("home").join("notes.txt"), 10);
    let scan = fx.scan();
    let e = |p: &Path| fx.entry(&scan, p);

    // MTR-02/05: read-only inspection; a folder with protected contents is view-only.
    let (code, ins) = fx.cli(&["trash", "inspect", "--scan", &scan, "--entry", &e(&data.join("home"))]);
    assert_eq!(code, 0, "{ins}");
    assert_eq!((ins["kind"].as_str(), ins["trashable"].as_bool()), (Some("trash_inspection"), Some(false)));
    assert_eq!(ins["refusal"]["reason"], "protected_descendants", "{ins}");
    assert_eq!(ins["available_actions"], serde_json::json!(["show_path", "copy_path"]));
    let (code, err) = fx.cli(&["trash", "--scan", &scan, "--entry", &e(&data.join("home"))]);
    // Readiness is checked first: without a Trash on the volume every plan is refused as trash_unavailable instead.
    if !err["message"].as_str().is_some_and(|m| m.starts_with("trash_unavailable: ")) {
        assert_eq!((code, err["code"].as_str()), (3, Some("policy_denied")), "{err}");
    }
    let (code, err) = fx.cli(&["trash", "--scan", &scan, "--entry", "999999999"]);
    assert_eq!((code, err["code"].as_str()), (2, Some("invalid_argument")), "{err}");

    // Whole folders are not moved on macOS in this build: view-only with a reason, and the plan is refused.
    let (_, ins) = fx.cli(&["trash", "inspect", "--scan", &scan, "--entry", &e(&data.join("renders"))]);
    assert_eq!((ins["trashable"].as_bool(), ins["refusal"]["reason"].as_str(), ins["files"].as_u64()), (Some(false), Some("folder_move_unavailable"), Some(1)), "{ins}");
    let (code, err) = fx.cli(&["trash", "--scan", &scan, "--entry", &e(&data.join("renders"))]);
    assert_eq!((code, err["code"].as_str()), (7, Some("unsupported")), "{err}");

    // A plain file: trashable, guidance names Finder's Put Back, holders were queried (none here).
    let big = e(&data.join("big.bin"));
    let (_, ins) = fx.cli(&["trash", "inspect", "--scan", &scan, "--entry", &big]);
    assert_eq!((ins["trashable"].as_bool(), ins["logical_bytes"].as_str()), (Some(true), Some("65536")), "{ins}");
    assert_eq!(ins["available_actions"], serde_json::json!(["show_path", "copy_path", "move_to_trash"]));
    assert!(ins["guidance"].as_str().unwrap().contains("Put Back"), "{ins}");
    assert!(ins["holders"].as_array().is_some_and(|h| h.is_empty()), "{ins}");

    // MTR-01: only a plan, and it needs a review.
    let (code, plan) = fx.cli(&["trash", "--scan", &scan, "--entry", &big]);
    if plan_or_unavailable(code, &plan) {
        let (code, err) = fx.cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()]);
        assert_eq!((code, err["code"].as_str()), (3, Some("needs_approval")), "{err}");
    }
    assert!(data.join("big.bin").exists() && data.join("renders").join("a.png").exists(), "nothing moved without a review");
}

struct Kill(Child);

impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// MTR-04: a held file names its holding process; applying fails as `in_use` with that name and nothing moves.
#[test]
fn a_held_file_reports_its_holder_by_name() {
    let fx = Fixture::new("held");
    let log = fx.data().join("app.log");
    write(&log, 50);
    let scan = fx.scan();
    let entry = fx.entry(&scan, &log);
    let _tail = Kill(Command::new("/usr/bin/tail").arg("-f").arg(&log).stdout(std::process::Stdio::null()).spawn().unwrap());
    let mut ins = Value::Null;
    for _ in 0..50 {
        ins = fx.cli(&["trash", "inspect", "--scan", &scan, "--entry", &entry]).1;
        if ins["holders"].as_array().is_some_and(|h| !h.is_empty()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(ins["holders"].as_array().unwrap().iter().any(|h| h.as_str().unwrap().starts_with("tail (pid ")), "{ins}");
    let (code, plan) = fx.cli(&["trash", "--scan", &scan, "--entry", &entry]);
    if !plan_or_unavailable(code, &plan) {
        return;
    }
    approve(&fx.store(), &plan);
    let (code, report) = fx.cli(&["plan", "apply", plan["body"]["plan_id"].as_str().unwrap()]);
    assert_eq!(code, 8, "failed actions make a failed operation: {report}");
    let item = &report["items"][0];
    assert_eq!((item["status"].as_str(), item["code"].as_str()), (Some("failed"), Some("in_use")), "{report}");
    assert!(item["message"].as_str().unwrap().contains("tail (pid "), "{item}");
    assert!(log.exists());
}

/// A Trash that cannot be listed is `permission_required`, never an empty Trash (TCC simulated with mode 000).
#[test]
fn an_unlistable_trash_is_permission_required() {
    let fx = Fixture::new("perm");
    write(&fx.bin().join("x.bin"), 3);
    std::fs::set_permissions(fx.bin(), std::fs::Permissions::from_mode(0o000)).unwrap();
    if is_root() {
        eprintln!("skipped: root lists a mode-000 folder");
        return;
    }
    let (code, err) = fx.cli(&["trash", "list"]);
    assert_eq!((code, err["kind"].as_str(), err["code"].as_str()), (4, Some("error"), Some("permission_required")), "{err}");
    // Mode 000 is the folder's own permissions (EACCES); privacy protection (EPERM) points to Full Disk Access instead.
    assert!(err["required_action"].as_str().is_some_and(|a| a.contains("permissions")), "{err}");
    assert!(err["message"].as_str().unwrap().contains(".Trash"), "{err}");
    let (code, err) = fx.cli(&["trash", "purge", "--item", "tb_0000000000000000"]);
    assert_eq!((code, err["code"].as_str()), (4, Some("permission_required")), "{err}");
}

/// MTR-03 on a fixture Trash: listed without original locations, Finder's record skipped; a separate purge plan needs
/// its own approval and deletes through the verified delete; links are refused.
#[test]
fn a_fixture_trash_lists_and_purges_through_a_separate_plan() {
    let fx = Fixture::new("purge");
    let bin = fx.bin();
    write(&bin.join(".DS_Store"), 12);
    write(&bin.join("old.mov"), 900);
    write(&bin.join("renders").join("a.png"), 3);
    write(&bin.join("renders").join("sub").join("b.png"), 4);
    write(&fx.data().join("outside.txt"), 5);
    std::os::unix::fs::symlink(fx.data().join("outside.txt"), bin.join("link")).unwrap();

    let (code, list) = fx.cli(&["trash", "list"]);
    assert_eq!(code, 0, "{list}");
    let items = list["items"].as_array().unwrap();
    let loc = |i: &Value| i["bin_location"].as_str().unwrap().rsplit('/').next().unwrap().to_owned();
    let mut names: Vec<String> = items.iter().map(loc).collect();
    names.sort();
    assert_eq!(names, ["link", "old.mov", "renders"], "{list}");
    assert!(items.iter().all(|i| i["original_path"].is_null() && i["deleted_at"].is_null()), "no original location is claimed: {list}");
    let by = |n: &str| items.iter().find(|i| loc(i) == n).unwrap().clone();
    assert_eq!((by("link")["purge_capability"].as_str(), by("renders")["logical_bytes"].as_str()), (Some("unsupported"), Some("7")), "{list}");

    let (code, err) = fx.cli(&["trash", "purge", "--item", by("link")["trash_item_id"].as_str().unwrap()]);
    assert_eq!((code, err["code"].as_str()), (7, Some("unsupported")), "{err}");
    let ids = format!("{},{}", by("old.mov")["trash_item_id"].as_str().unwrap(), by("renders")["trash_item_id"].as_str().unwrap());
    let (code, purge) = fx.cli(&["trash", "purge", "--item", &ids]);
    assert_eq!(code, 0, "{purge}");
    let acts = purge["body"]["items"].as_array().unwrap();
    assert_eq!(acts.len(), 5, "old.mov; renders: two files, sub and itself: {purge}");
    assert!(acts.iter().all(|a| a["action_type"] == "fs_delete" && a["target_location"] == "platform_trash"), "{purge}");
    let purge_id = purge["body"]["plan_id"].as_str().unwrap().to_owned();
    let (code, err) = fx.cli(&["plan", "apply", &purge_id]);
    assert_eq!((code, err["code"].as_str()), (3, Some("needs_approval")), "{err}");
    assert!(bin.join("old.mov").exists());
    approve(&fx.store(), &purge);
    let (code, report) = fx.cli(&["plan", "apply", &purge_id]);
    assert_eq!(code, 0, "{report}");
    assert!(report["items"].as_array().unwrap().iter().all(|i| i["status"] == "succeeded" && i["recovery"] == "unavailable"), "{report}");
    assert!(report["estimated_freed_bytes"].as_str().unwrap().parse::<u64>().unwrap() >= 907, "{report}");
    assert!(!bin.join("old.mov").exists() && !bin.join("renders").exists());
    assert!(bin.join(".DS_Store").exists() && bin.join("link").symlink_metadata().is_ok() && fx.data().join("outside.txt").exists(), "nothing else touched");
    let (_, history) = fx.cli(&["operations", "list"]);
    assert_eq!((history["items"][0]["action"].as_str(), history["items"][0]["recovery"]["availability"].as_str()), (Some("delete"), Some("unavailable")), "{history}");
    let (_, list) = fx.cli(&["trash", "list"]);
    let mut names: Vec<String> = list["items"].as_array().unwrap().iter().map(loc).collect();
    names.sort();
    assert_eq!(names, ["link"], "{list}");
}

/// Removes from the user's real Trash what this test moved there (by its unique name; never anything else).
struct RealTrashGuard(PathBuf);

impl Drop for RealTrashGuard {
    fn drop(&mut self) {
        if self.0.symlink_metadata().is_ok()
            && let Err(e) = std::fs::remove_file(&self.0)
        {
            eprintln!("could not remove {} from the Trash: {e}", self.0.display());
        }
    }
}

/// Parity 6.2 end to end on the user's real Trash: the outcome is succeeded with post-verification (or the plan was
/// refused as trash_unavailable); then the real Trash is listed (or permission_required) and the item purged.
#[test]
fn a_reviewed_plan_moves_a_file_to_the_real_trash_and_purges_it() {
    if !device_tests_allowed() {
        eprintln!("skipped: set CM_ALLOW_MAC_DEVICE_TESTS=1 to move a test file to your Trash");
        return;
    }
    let fx = Fixture::new("real");
    let name = format!("{}.bin", fx.base.file_name().unwrap().to_string_lossy());
    let file = fx.data().join(&name);
    write(&file, 4096);
    write(&fx.data().join("keep.bin"), 10);
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
    let _guard = RealTrashGuard(home.join(".Trash").join(&name));
    let scan = fx.scan();
    let entry = fx.entry(&scan, &file);
    let (code, plan) = fx.run(&["trash", "--scan", &scan, "--entry", &entry], false);
    if !plan_or_unavailable(code, &plan) {
        eprintln!("the runner's volume has no Trash: refused up front, as required");
        return;
    }
    let plan_id = plan["body"]["plan_id"].as_str().unwrap().to_owned();
    approve(&fx.store(), &plan);
    let (code, report) = fx.run(&["plan", "apply", &plan_id], false);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["status"], "completed", "{report}");
    let item = &report["items"][0];
    assert_eq!((item["status"].as_str(), item["recovery"].as_str()), (Some("succeeded"), Some("trash")), "{report}");
    assert!(!file.exists() && fx.data().join("keep.bin").exists());
    let (_, history) = fx.run(&["operations", "list"], false);
    assert_eq!((history["items"][0]["action"].as_str(), history["items"][0]["recovery"]["availability"].as_str()), (Some("trash"), Some("system_recycle_bin")));

    let (code, list) = fx.run(&["trash", "list"], false);
    if code == 4 {
        assert_eq!(list["code"], "permission_required", "{list}");
        eprintln!("the real Trash is protected for this process: permission_required, as required");
        return;
    }
    assert!(code == 0 || code == 5, "{list}");
    let ours: Vec<&Value> = list["items"].as_array().unwrap().iter().filter(|i| i["bin_location"].as_str().is_some_and(|l| l.ends_with(&format!("/{name}")))).collect();
    assert_eq!(ours.len(), 1, "{list}");
    assert!(ours[0]["original_path"].is_null());
    let (code, purge) = fx.run(&["trash", "purge", "--item", ours[0]["trash_item_id"].as_str().unwrap()], false);
    assert_eq!(code, 0, "{purge}");
    approve(&fx.store(), &purge);
    let (code, report) = fx.run(&["plan", "apply", purge["body"]["plan_id"].as_str().unwrap()], false);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["items"][0]["status"], "succeeded", "{report}");
    assert!(home.join(".Trash").join(&name).symlink_metadata().is_err(), "purged");
}
