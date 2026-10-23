//! Checks made when a plan is created, through the real binary on real NTFS (acceptance round WFA, fix package F2a):
//! - MTR-03 / WFA-35: a trash-mode plan (`clean`, `plan create`, manual `trash`) whose targets sit on a volume whose
//!   Recycle Bin is turned off is refused up front with `unsupported` / `trash_unavailable`, naming the volume and the
//!   next step; nothing is stored, and quarantine still works (the mode is never switched). The turned-off setting is
//!   injected through the test-only BitBucket redirect (`CM_TEST_BITBUCKET_ROOT`, a key under
//!   `HKCU\Software\CleanManagerTest\`, read in addition to the real settings); with `CM_ALLOW_VHD_TESTS=1` (elevated)
//!   a disposable VHDX volume gets the real per-volume setting instead.
//! - CLI-11 / WFA-50: a file changed inside a default-selected folder lists that folder as stale; a deleted, explicitly
//!   named file is listed as missing and the rest is planned (its failed live probe no longer refuses the request).
//! - ANA-02 / WFA-17: `candidates list` carries the preview's explanation: tier and why, rebuild and offline cost, the
//!   rule's age threshold by modification time, recovery limits, the rule's own match basis.
//! - WFA-47/63: a junction to System32 is refused as protected (`policy_denied`, exit 3) naming the location.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Command;

use cm_core::analysis::IndexView;
use serde_json::{Value, json};

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-f2a-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    for store in [p.clone(), p.join("store")] {
        // D15: the gated tools under test are off by default; opt in the way a person does.
        let o = Command::new(env!("CARGO_BIN_EXE_cleanmanager")).args(["experimental", "--state", "on", "--json"]).env("CM_STORE_DIR", &store).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stdout));
    }
    p
}

struct Fx {
    root: PathBuf,
    local: PathBuf,
    store: PathBuf,
    rules: PathBuf,
    env: Vec<(String, String)>,
}

impl Fx {
    fn new(name: &str) -> Fx {
        let root = scratch(name);
        let (local, store, rules) = (root.join("local"), root.join("store"), root.join("rules"));
        std::fs::create_dir_all(local.join("Temp")).unwrap();
        write_pack(&rules);
        Fx { root, local, store, rules, env: vec![] }
    }

    fn cli(&self, args: &[&str]) -> (i32, Value) {
        let mut c = Command::new(env!("CARGO_BIN_EXE_cleanmanager"));
        c.args(args)
            .arg("--json")
            .env("CM_STORE_DIR", &self.store)
            .env("CM_RULES_DIR", &self.rules)
            .env("CM_RESOLVE_LOCALAPPDATA", &self.local)
            .env("CM_RESOLVE_TEMP", self.local.join("Temp"))
            .env_remove("CM_TEST_BITBUCKET_ROOT");
        for (k, v) in &self.env {
            c.env(k, v);
        }
        let out = c.output().unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        assert_eq!(text.lines().count(), 1, "exactly one record: {text}");
        (out.status.code().unwrap(), serde_json::from_str(&text).unwrap())
    }

    fn scan(&self, root: &Path) -> String {
        let (code, scan) = self.cli(&["scan", "--root", root.to_str().unwrap()]);
        assert_eq!(code, 0, "{scan}");
        scan["scan_id"].as_str().unwrap().to_owned()
    }

    fn plans(&self) -> usize {
        cm_store::Stores::open(&self.store).unwrap().audit.plans_page(500, None).unwrap().0.len()
    }

    /// Entry id of `path` in a published scan (by its name under its parent's name).
    fn entry(&self, scan: &str, path: &Path) -> String {
        let stores = cm_store::Stores::open(&self.store).unwrap();
        let view = stores.index.view(scan).unwrap();
        let want = path.file_name().unwrap().to_string_lossy().to_ascii_lowercase();
        let parent = path.parent().unwrap().file_name().unwrap().to_string_lossy().to_ascii_lowercase();
        let found = view.facts.keys().copied().find(|&e| {
            view.name(e).lossy().to_ascii_lowercase() == want && view.parent(e).is_some_and(|p| view.name(p).lossy().to_ascii_lowercase().ends_with(&parent))
        });
        found.unwrap_or_else(|| panic!("{} indexed", path.display())).to_string()
    }
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Two enabled rules: a default-selected folder rule under the user cache root (no probes) and a review-only rule for
/// week-old files directly in the user temp folder (in-use probe, modification-time age).
fn write_pack(dir: &Path) {
    std::fs::create_dir_all(dir.join("windows")).unwrap();
    std::fs::create_dir_all(dir.join("registry")).unwrap();
    let none = json!({"min_age_days": null, "age_basis": null, "min_size_bytes": null, "retention_days": null});
    let week = json!({"min_age_days": 7, "age_basis": "modified_time", "min_size_bytes": null, "retention_days": null});
    let rules = [
        ("test.d3d.folders", "windows.directx", "user_cache", "directory", "default_selected", "local_rebuild", json!([]), none.clone(),
         ["Shader cache folders.", "Rebuilt by DirectX when games next run.", "None: rebuilt from local data."]),
        ("test.temp.week-old", "windows", "user_temp", "file", "review_only", "not_rebuildable", json!(["open_file_handle"]), week,
         ["Files directly in the user's temporary folder whose last modification is at least 7 days old.", "Not regenerated: review before removing.",
          "A program that still expects the file may fail."]),
    ];
    for (id, owner, resolver, object, tier, rebuild, probes, thresholds, [summary, regeneration, offline]) in rules {
        let rule = json!({
            "schema_version": "2.0", "rule_id": id, "version": "1.0.0", "status": "enabled", "platforms": ["windows"],
            "category": "system_data", "max_tier": tier, "rebuild_class": rebuild, "owner": {"kind": "system", "id": owner},
            "root_resolver": resolver,
            "match": {"object_type": object, "relative_components": [], "name_glob": "*", "extensions": [], "require_cachedir_tag": false,
                      "archive_payload": false, "max_depth": 0},
            "exclude": [], "probes": probes, "thresholds": thresholds,
            "action_kind": "filesystem", "adapter_ref": null, "allowed_modes": ["trash", "quarantine"], "requires_admin": false,
            "protect_descendants": true, "follow_links": false,
            "impact": {"summary": summary, "regeneration": regeneration, "offline_effect": offline},
            "tested_registry_ref": format!("{id}@1.0.0"), "evidence_refs": [format!("fixture_{id}")],
        });
        let registry = json!({
            "schema_version": "1.0", "registry_id": format!("{id}@1.0.0"), "rule_id": id, "rule_version": "1.0.0", "synthetic": false,
            "subject": {"kind": "system", "id": owner, "name": "fixture"}, "version_range": {"min_inclusive": "10.0", "max_exclusive": "99.0"},
            "platforms": [{"platform": "windows", "os_versions": ["10.0"], "architectures": ["x64"]}], "tested_at": "2026-10-15",
            "result": "passed", "fixtures": {"positive": ["p"], "negative": ["n"]}, "known_effects": [], "evidence_refs": ["fixture"],
            "thresholds": thresholds,
        });
        std::fs::write(dir.join("windows").join(format!("{id}.json")), rule.to_string()).unwrap();
        std::fs::write(dir.join("registry").join(format!("{id}@1.0.0.json")), registry.to_string()).unwrap();
    }
}

fn write(p: &Path, bytes: usize) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, vec![7u8; bytes]).unwrap();
}

/// A file last modified a month ago (old enough for the week-old rule).
fn write_old(p: &Path, bytes: usize) {
    write(p, bytes);
    let month_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400);
    std::fs::File::options().write(true).open(p).unwrap().set_modified(month_ago).unwrap();
}

fn reg(args: &[&str]) -> bool {
    Command::new("reg").args(args).output().is_ok_and(|o| o.status.success())
}

/// A test-only BitBucket key under `HKCU\Software\CleanManagerTest\`, deleted on drop.
struct TestBitBucket(String);

impl TestBitBucket {
    /// "Don't move files to the Recycle Bin" for the volume holding `path`, in the redirected settings only.
    fn turned_off_for(path: &Path) -> TestBitBucket {
        let raw = cm_platform::paths::raw_path(path.as_os_str()).unwrap();
        let guid = cm_platform::windows::actions::recycle_bin_volume_key(&raw).expect("the scratch volume has a GUID name");
        let key = format!(r"Software\CleanManagerTest\cli{}_bin", std::process::id());
        assert!(reg(&["add", &format!(r"HKCU\{key}\Volume\{guid}"), "/v", "NukeOnDelete", "/t", "REG_DWORD", "/d", "1", "/f"]));
        TestBitBucket(key)
    }
}

impl Drop for TestBitBucket {
    fn drop(&mut self) {
        let _ = reg(&["delete", &format!(r"HKCU\{}", self.0), "/f"]);
        // The shared parent goes too once no other test key is under it.
        let rest = Command::new("reg").args(["query", r"HKCU\Software\CleanManagerTest"]).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        if !rest.lines().any(|l| l.to_ascii_lowercase().contains(r"cleanmanagertest\")) {
            let _ = reg(&["delete", r"HKCU\Software\CleanManagerTest", "/f"]);
        }
    }
}

fn object_id(preview: &Value, ends_with: &str) -> String {
    let items = preview["items"].as_array().unwrap();
    let item = items.iter().find(|i| i["path"]["display"].as_str().unwrap().ends_with(ends_with)).unwrap_or_else(|| panic!("{ends_with} in {preview}"));
    item["object_id"].as_str().unwrap().to_owned()
}

fn planned(plan: &Value) -> std::collections::BTreeSet<String> {
    plan["body"]["items"].as_array().unwrap().iter().map(|i| i["candidate_id"].as_str().unwrap().to_owned()).collect()
}

fn assert_bin_refusal(code: i32, err: &Value, quarantine_hint: bool) {
    assert_eq!((code, err["kind"].as_str(), err["code"].as_str()), (7, Some("error"), Some("unsupported")), "{err}");
    let msg = err["message"].as_str().unwrap();
    assert!(msg.starts_with("trash_unavailable: no usable Recycle Bin on ") && msg.contains("turned off for this volume"), "{msg}");
    assert!(msg.contains("no permanent-delete alternative"), "{msg}");
    let next = err["required_action"].as_str().unwrap();
    assert_eq!(next.contains("--mode quarantine"), quarantine_hint, "{next}");
    assert!(next.contains("Recycle Bin"), "{next}");
}

#[test]
fn a_volume_whose_recycle_bin_is_off_refuses_trash_plans_up_front() {
    let mut fx = Fx::new("bin");
    write(&fx.local.join("D3DSCache").join("one").join("blob.bin"), 4096);
    let file = fx.local.join("data").join("keep.bin");
    write(&file, 100);
    let sid = fx.scan(&fx.local);
    let real_bin = cm_platform::windows::actions::recycle_bin_ready(&cm_platform::paths::raw_path(fx.local.as_os_str()).unwrap()).is_ok();
    if real_bin {
        // Control: with the bin on, the same request makes a plan.
        let (code, plan) = fx.cli(&["clean", "--scan", &sid, "--mode", "trash", "--non-interactive"]);
        assert_eq!(code, 0, "{plan}");
    }
    let before = fx.plans();

    let off = TestBitBucket::turned_off_for(&fx.local);
    fx.env.push(("CM_TEST_BITBUCKET_ROOT".into(), off.0.clone()));
    let (code, err) = fx.cli(&["clean", "--scan", &sid, "--mode", "trash", "--non-interactive"]);
    assert_bin_refusal(code, &err, true);
    let (_, analysis) = fx.cli(&["analyze", "--scan", &sid]);
    let set = analysis["candidate_set_id"].as_str().unwrap().to_owned();
    let (code, err) = fx.cli(&["plan", "create", "--set", &set, "--mode", "trash", "--all-default-selected"]);
    assert_bin_refusal(code, &err, true);
    let entry = fx.entry(&sid, &file);
    let (code, err) = fx.cli(&["trash", "--scan", &sid, "--entry", &entry]);
    assert_bin_refusal(code, &err, false);
    assert_eq!(fx.plans(), before, "nothing was stored");
    // The read-only inspection says so before a plan is attempted.
    let (code, ins) = fx.cli(&["trash", "inspect", "--scan", &sid, "--entry", &entry]);
    assert_eq!(code, 0, "{ins}");
    assert_eq!((ins["trashable"].as_bool(), ins["refusal"]["reason"].as_str()), (Some(false), Some("trash_unavailable")), "{ins}");
    assert_eq!(ins["available_actions"], json!(["show_path", "copy_path"]));
    // The mode is never switched for the person: quarantine is their explicit choice, and it still works.
    let (code, plan) = fx.cli(&["clean", "--scan", &sid, "--mode", "quarantine", "--non-interactive"]);
    assert_eq!((code, plan["body"]["mode"].as_str()), (0, Some("quarantine")), "{plan}");
    assert!(file.exists() && fx.local.join("D3DSCache").join("one").join("blob.bin").exists());
}

#[test]
fn plan_creation_lists_changed_folders_and_missing_named_files_as_stale() {
    let fx = Fx::new("stale");
    let d3d = fx.local.join("D3DSCache");
    for dir in ["one", "two"] {
        write(&d3d.join(dir).join("blob.bin"), 4000);
        write(&d3d.join(dir).join("sub").join("deep.bin"), 10);
    }
    write_old(&fx.local.join("Temp").join("old-a.log"), 50);
    write_old(&fx.local.join("Temp").join("old-b.log"), 60);
    let sid = fx.scan(&fx.local);
    let (code, preview) = fx.cli(&["clean", "--scan", &sid, "--dry-run", "--non-interactive"]);
    assert_eq!(code, 0, "{preview}");
    let (one, two) = (object_id(&preview, "\\one"), object_id(&preview, "\\two"));
    let (a, b) = (object_id(&preview, "\\old-a.log"), object_id(&preview, "\\old-b.log"));

    // (a) A file inside a default-selected folder grew after the preview: that folder is stale, the other planned.
    write(&d3d.join("one").join("sub").join("deep.bin"), 4500);
    let (code, plan) = fx.cli(&["clean", "--scan", &sid, "--mode", "quarantine", "--non-interactive"]);
    assert_eq!(code, 0, "{plan}");
    assert_eq!(planned(&plan), [two.clone()].into_iter().collect(), "{plan}");
    let stale = plan["stale_items"].as_array().unwrap();
    assert_eq!(stale.len(), 1, "{plan}");
    assert_eq!((stale[0]["object_id"].as_str(), stale[0]["reason"].as_str()), (Some(one.as_str()), Some("changed")));
    let detail = stale[0]["detail"].as_str().unwrap();
    assert!(detail.contains("deep.bin") && detail.contains("4500 bytes now, 10 when scanned"), "{detail}");
    // A deleted file inside a folder counts as a change of that folder too.
    std::fs::remove_file(d3d.join("two").join("blob.bin")).unwrap();
    let (code, err) = fx.cli(&["clean", "--scan", &sid, "--mode", "quarantine", "--non-interactive"]);
    assert_eq!((code, err["code"].as_str()), (6, Some("plan_stale")), "every selected folder changed: {err}");

    // (b) A deleted, explicitly named file is listed as missing; the other named file is planned.
    std::fs::remove_file(fx.local.join("Temp").join("old-a.log")).unwrap();
    let (code, plan) = fx.cli(&["clean", "--scan", &sid, "--mode", "quarantine", "--candidates", &format!("{a},{b}")]);
    assert_eq!(code, 0, "a missing object no longer refuses the request: {plan}");
    assert_eq!(planned(&plan), [b.clone()].into_iter().collect(), "{plan}");
    assert_eq!(plan["stale_items"], json!([{"object_id": a, "reason": "missing",
        "detail": format!("{} no longer exists", fx.local.join("Temp").join("old-a.log").display())}]), "{plan}");
    assert_eq!(plan["authorization_status"], "required", "explicitly named review-only rows still need authorization");
}

#[test]
fn candidates_list_explains_like_the_preview() {
    let fx = Fx::new("explain");
    write(&fx.local.join("D3DSCache").join("one").join("blob.bin"), 4096);
    write_old(&fx.local.join("Temp").join("old.log"), 50);
    write(&fx.local.join("Temp").join("fresh.log"), 50);
    let sid = fx.scan(&fx.local);
    let (code, analysis) = fx.cli(&["analyze", "--scan", &sid]);
    assert_eq!(code, 0, "{analysis}");
    let (code, page) = fx.cli(&["candidates", "list", "--set", analysis["candidate_set_id"].as_str().unwrap()]);
    assert_eq!(code, 0, "{page}");
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "the fresh file is not a candidate: {page}");
    let schema: Value = serde_json::from_str(include_str!("../../../schemas/v1/protocol.schema.json")).unwrap();
    let def = &schema["$defs"]["candidate_item"];
    for i in items {
        for k in i.as_object().unwrap().keys() {
            assert!(def["properties"].get(k).is_some(), "candidate_item field {k} is not in the schema");
        }
        for r in def["required"].as_array().unwrap() {
            assert!(i.get(r.as_str().unwrap()).is_some(), "required field {r} missing: {i}");
        }
    }
    let by = |suffix: &str| items.iter().find(|i| i["path"]["display"].as_str().unwrap().ends_with(suffix)).unwrap();
    let temp = by("\\old.log");
    assert_eq!((temp["tier"].as_str(), temp["risk"].as_str(), temp["rebuild_class"].as_str()), (Some("review_only"), Some("medium"), Some("not_rebuildable")), "{temp}");
    let reason = temp["reason"].as_str().unwrap();
    assert!(reason.starts_with("test.temp.week-old@1.0.0: ") && reason.contains("at least 7 day(s)") && reason.contains("modification time"), "{reason}");
    assert!(!reason.contains("last used ") || reason.contains("when it was last used is not known"), "modification time is never called last use: {reason}");
    assert_ne!(temp["reason"], "matched rule");
    assert!(temp["tier_reason"].as_str().unwrap().starts_with("rule test.temp.week-old@1.0.0 lists it for review only"), "{temp}");
    assert_eq!(temp["age_threshold"], json!({"min_age_days": 7, "basis": "modified_time"}));
    assert_eq!((temp["regeneration"].as_str(), temp["offline_effect"].as_str()), (Some("Not regenerated: review before removing."), Some("A program that still expects the file may fail.")));
    let limits = temp["recovery_limits"].as_array().unwrap();
    assert_eq!(limits.iter().map(|l| (l["mode"].as_str().unwrap(), l["recovery"].as_str().unwrap())).collect::<Vec<_>>(), vec![("trash", "system_recycle_bin"), ("quarantine", "in_app_restore")]);
    assert!(limits.iter().all(|l| !l["limit"].as_str().unwrap().is_empty()));
    assert_eq!(temp["evidence_refs"], json!(["fixture_test.temp.week-old"]));
    let cache = by("\\one");
    assert_eq!((cache["rebuild_class"].as_str(), cache["age_threshold"].clone()), (Some("local_rebuild"), Value::Null), "{cache}");
    assert!(cache["tier_reason"].as_str().unwrap().starts_with("rule test.d3d.folders@1.0.0 allows default selection (rebuilt locally"), "{cache}");

    // The preview explains the same rows with the same words.
    let (code, preview) = fx.cli(&["clean", "--scan", &sid, "--dry-run"]);
    assert_eq!(code, 0, "{preview}");
    for listed in [temp, cache] {
        let row = preview["items"].as_array().unwrap().iter().find(|r| r["object_id"] == listed["candidate_id"]).unwrap();
        for key in ["tier", "tier_reason", "regeneration", "offline_effect", "age_threshold", "recovery_limits", "rebuild_class"] {
            assert_eq!(row[key], listed[key], "{key}: preview {row} vs list {listed}");
        }
    }
}

/// `mklink /J` (no administrator needed); removed with `rmdir` semantics (the link only, never its target).
fn junction(link: &Path, target: &Path) -> bool {
    Command::new("cmd").args(["/c", "mklink", "/J"]).arg(link).arg(target).output().is_ok_and(|o| o.status.success())
}

#[test]
fn a_link_into_system32_is_refused_as_protected_and_named() {
    let fx = Fx::new("link");
    let links = fx.local.join("links");
    std::fs::create_dir_all(&links).unwrap();
    let system32 = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32");
    let (sys, plain) = (links.join("link-sys"), links.join("link-plain"));
    write(&fx.local.join("target").join("x.bin"), 10);
    if !junction(&sys, &system32) || !junction(&plain, &fx.local.join("target")) {
        eprintln!("skipped: junctions cannot be created here");
        return;
    }
    let sid = fx.scan(&fx.local);
    let entry = fx.entry(&sid, &sys);
    let (code, err) = fx.cli(&["trash", "--scan", &sid, "--entry", &entry]);
    assert_eq!((code, err["code"].as_str()), (3, Some("policy_denied")), "{err}");
    let msg = err["message"].as_str().unwrap().to_ascii_lowercase();
    assert!(msg.contains("link-sys") && msg.contains("resolves into a protected system location") && msg.contains("system32") && msg.contains("hard-deny"), "{msg}");
    let (code, ins) = fx.cli(&["trash", "inspect", "--scan", &sid, "--entry", &entry]);
    assert_eq!(code, 0, "{ins}");
    assert_eq!((ins["trashable"].as_bool(), ins["refusal"]["code"].as_str(), ins["refusal"]["reason"].as_str()), (Some(false), Some("policy_denied"), Some("protected")), "{ins}");
    // A link to an ordinary folder stays the plain link refusal: links are never moved or followed.
    let (code, err) = fx.cli(&["trash", "--scan", &sid, "--entry", &fx.entry(&sid, &plain)]);
    assert_eq!((code, err["code"].as_str()), (7, Some("unsupported")), "{err}");
    assert!(err["message"].as_str().unwrap().contains("never moved or followed"), "{err}");
    // Remove the links themselves (never their targets) before the scratch folder goes.
    std::fs::remove_dir(&sys).unwrap();
    std::fs::remove_dir(&plain).unwrap();
    assert!(system32.join("kernel32.dll").exists() && fx.local.join("target").join("x.bin").exists());
}

/// A disposable VHDX volume (diskpart; elevated shell), detached and deleted on drop.
struct Vhd(PathBuf);

impl Vhd {
    fn create(file: PathBuf, letter: char) -> Vhd {
        let script = file.with_extension("dp");
        let f = file.display();
        std::fs::write(&script, format!("create vdisk file=\"{f}\" maximum=64 type=expandable\nselect vdisk file=\"{f}\"\nattach vdisk\ncreate partition primary\nformat fs=ntfs quick label=CMF2A\nassign letter={letter}\n")).unwrap();
        let out = Command::new("diskpart").arg("/s").arg(&script).output().unwrap();
        let _ = std::fs::remove_file(&script);
        assert!(out.status.success(), "diskpart: {}", String::from_utf8_lossy(&out.stdout));
        Vhd(file)
    }
}

impl Drop for Vhd {
    fn drop(&mut self) {
        let cmd = format!("Dismount-DiskImage -ImagePath \"{}\" -ErrorAction SilentlyContinue | Out-Null", self.0.display());
        let _ = Command::new("powershell").args(["-NoProfile", "-Command", &cmd]).output();
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The real per-volume setting of a disposable volume, restored (removed) on drop.
struct RealBitBucket(String);

impl Drop for RealBitBucket {
    fn drop(&mut self) {
        let _ = reg(&["delete", &self.0, "/f"]);
    }
}

#[test]
fn a_real_volume_with_its_recycle_bin_turned_off_refuses_trash_plans() {
    if std::env::var("CM_ALLOW_VHD_TESTS").as_deref() != Ok("1") {
        eprintln!("skipped: set CM_ALLOW_VHD_TESTS=1 (elevated) to mount a scratch volume");
        return;
    }
    let mut fx = Fx::new("vhd");
    let letter = ('M'..='Z').rev().find(|c| !Path::new(&format!("{c}:\\")).exists()).expect("a free drive letter");
    let _vhd = Vhd::create(fx.root.join("bin.vhdx"), letter);
    let vol = PathBuf::from(format!("{letter}:\\"));
    let data = vol.join("data").join("x.bin");
    write(&data, 100);
    let raw = cm_platform::paths::raw_path(data.as_os_str()).unwrap();
    let guid = cm_platform::windows::actions::recycle_bin_volume_key(&raw).expect("volume GUID");
    // The volume is new, so its BitBucket key is ours to create and remove.
    let key = format!(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume\{guid}");
    let _setting = RealBitBucket(key.clone());
    assert!(reg(&["add", &key, "/v", "NukeOnDelete", "/t", "REG_DWORD", "/d", "1", "/f"]));
    fx.local = vol.clone();
    std::fs::create_dir_all(vol.join("Temp")).unwrap();
    write_old(&vol.join("Temp").join("old.log"), 10);
    let sid = fx.scan(&vol.join("data"));
    let (code, err) = fx.cli(&["trash", "--scan", &sid, "--entry", &fx.entry(&sid, &data)]);
    assert_bin_refusal(code, &err, false);
    assert!(err["message"].as_str().unwrap().contains(&format!("{letter}:\\")), "names the volume: {err}");
    let tsid = fx.scan(&vol.join("Temp"));
    let (code, preview) = fx.cli(&["clean", "--scan", &tsid, "--dry-run"]);
    assert_eq!(code, 0, "{preview}");
    let id = object_id(&preview, "\\old.log");
    let (code, err) = fx.cli(&["clean", "--scan", &tsid, "--mode", "trash", "--candidates", &id]);
    assert_bin_refusal(code, &err, true);
    let (code, plan) = fx.cli(&["clean", "--scan", &tsid, "--mode", "quarantine", "--candidates", &id]);
    assert_eq!(code, 0, "{plan}");
    assert!(data.exists() && vol.join("Temp").join("old.log").exists());
}

/// WFA-113 / NEW-11: a cache folder emptied by an earlier cleanup (only an empty folder left inside) is not a candidate,
/// so it is never default-selected; a selection that is left with nothing to act on is refused as an empty selection
/// (`policy_denied`, exit 3) and no zero-action plan is stored.
#[test]
fn an_emptied_folder_is_not_planned_and_an_empty_selection_makes_no_plan() {
    let fx = Fx::new("emptied");
    let d3d = fx.local.join("D3DSCache");
    std::fs::create_dir_all(d3d.join("emptied").join("sub")).unwrap();
    write(&d3d.join("full").join("blob.bin"), 4096);
    let sid = fx.scan(&fx.local);
    let (code, analysis) = fx.cli(&["analyze", "--scan", &sid]);
    assert_eq!(code, 0, "{analysis}");
    let set = analysis["candidate_set_id"].as_str().unwrap().to_owned();
    let (_, page) = fx.cli(&["candidates", "list", "--set", &set]);
    let paths: Vec<&str> = page["items"].as_array().unwrap().iter().map(|i| i["path"]["display"].as_str().unwrap()).collect();
    assert!(paths.iter().any(|p| p.ends_with(r"\full")) && !paths.iter().any(|p| p.ends_with(r"\emptied")), "{paths:?}");
    let (code, plan) = fx.cli(&["plan", "create", "--set", &set, "--mode", "quarantine", "--all-default-selected"]);
    assert_eq!(code, 0, "{plan}");
    assert!(!plan["body"]["items"].as_array().unwrap().is_empty() && planned(&plan).len() == 1, "{plan}");

    // Only emptied folders left: nothing to plan, and nothing is stored.
    std::fs::remove_dir_all(d3d.join("full")).unwrap();
    let sid = fx.scan(&fx.local);
    let (_, analysis) = fx.cli(&["analyze", "--scan", &sid]);
    let set = analysis["candidate_set_id"].as_str().unwrap().to_owned();
    let before = fx.plans();
    let (code, err) = fx.cli(&["plan", "create", "--set", &set, "--mode", "quarantine", "--all-default-selected"]);
    assert_eq!((code, err["code"].as_str()), (3, Some("policy_denied")), "{err}");
    assert!(err["message"].as_str().unwrap().starts_with("empty selection"), "{err}");
    let (code, err) = fx.cli(&["clean", "--scan", &sid, "--mode", "quarantine", "--non-interactive"]);
    assert_eq!((code, err["code"].as_str()), (3, Some("policy_denied")), "{err}");
    assert_eq!(fx.plans(), before, "no zero-action plan was stored");
}
