use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::process::Command;
use std::time::{Duration, SystemTime};

use cm_core::analysis::{AnalysisInput, EntryFacts, MemView, analyze_preview};
use cm_core::capability::Platform;
use cm_core::model::ScanState;
use cm_core::rules::Tier;
use cm_core::scan::{EntryRow, ObjectRow, ScanBackend, ScanOptions, ScanSink, SkippedRow};
use serde_json::Value;

use super::*;
use crate::macos::posix::PosixBackend;
use crate::macos::testfs::Fixture;

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)
}

fn mac_rules() -> Vec<Rule> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(repo("rules/macos"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    paths.iter().map(|p| Rule::parse(&std::fs::read_to_string(p).unwrap()).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).collect()
}

fn vars(pairs: &[(&str, &Path)]) -> impl Fn(&str) -> Option<OsString> {
    let map: HashMap<String, OsString> = pairs.iter().map(|(k, v)| ((*k).to_owned(), v.as_os_str().to_owned())).collect();
    move |k| map.get(k).cloned()
}

fn rule_like(resolver: RootResolver, owner: &str) -> Rule {
    let mut rule = mac_rules().into_iter().next().unwrap();
    rule.root_resolver = resolver;
    rule.owner.id = owner.into();
    rule
}

#[test]
fn resolvers_only_map_fixed_locations() {
    let env = ResolverEnv {
        home: Some(PathBuf::from("/U")),
        temp: Some(PathBuf::from("/T")),
        downloads: Some(PathBuf::from("/U/Downloads")),
        tool_env: [("GOCACHE".to_string(), PathBuf::from("/gocache")), ("XDG_CACHE_HOME".to_string(), PathBuf::from("/xdg"))].into(),
        removable_volume: None,
        system: Some(PathBuf::from("/")),
    };
    let r = |resolver, owner: &str| targets(&rule_like(resolver, owner), &env);
    let p = |s: &str| vec![Target { path: PathBuf::from(s), tcc_protected: false }];
    assert_eq!(r(RootResolver::ApplicationCache, "microsoft.vscode"), p("/U/Library/Application Support/Code"));
    assert_eq!(r(RootResolver::ApplicationCache, "apple.xcode"), p("/U/Library/Developer/Xcode"));
    assert_eq!(r(RootResolver::CrashReports, "macos.diagnostic-reports"), p("/U/Library/Logs/DiagnosticReports"));
    assert_eq!(r(RootResolver::PackageManagerCache, "go"), p("/gocache"), "the tool's own override wins");
    assert_eq!(r(RootResolver::PackageManagerCache, "uv"), p("/xdg/uv"), "XDG_CACHE_HOME/uv when UV_CACHE_DIR is not set");
    assert_eq!(r(RootResolver::PackageManagerCache, "pnpm"), p("/xdg/pnpm"));
    assert_eq!(r(RootResolver::PackageManagerCache, "npm"), p("/U/.npm"));
    assert_eq!(r(RootResolver::PackageManagerCache, "pip"), p("/U/Library/Caches/pip"));
    assert_eq!(r(RootResolver::PackageManagerCache, "yarn"), p("/U/Library/Caches/Yarn"));
    assert_eq!(r(RootResolver::PackageManagerCache, "gradle"), p("/U/.gradle"));
    assert_eq!(r(RootResolver::UserTemp, "macos"), p("/T"));
    assert_eq!(r(RootResolver::InstallerLocation, "macos.downloads"), p("/U/Downloads"));
    assert!(r(RootResolver::RemovableVolumeRoot, "apple.metadata").is_empty(), "no volume unless the user selected one");
    assert_eq!(r(RootResolver::ApplicationCache, "apple.safari"), vec![Target { path: PathBuf::from("/U/Library/Caches/com.apple.Safari"), tcc_protected: true }]);
    assert!(base_paths(&rule_like(RootResolver::ApplicationCache, "apple.safari"), &env).is_empty(), "a TCC location is never a base path");
    assert!(r(RootResolver::ApplicationCache, "unknown.owner").is_empty(), "no mapping, no root");
    assert!(r(RootResolver::UserLogs, "microsoft.vscode").is_empty(), "a mapping is per resolver kind");
    assert!(r(RootResolver::SystemLogs, "macos").is_empty());
    // Wave 2.
    assert_eq!(r(RootResolver::ApplicationCache, "slack"), p("/U/Library/Application Support/Slack"));
    assert_eq!(r(RootResolver::ApplicationCache, "spotify"), p("/U/Library/Caches/com.spotify.client"));
    assert_eq!(r(RootResolver::ApplicationCache, "apple.coresimulator"), p("/U/Library/Developer/CoreSimulator"));
    assert_eq!(r(RootResolver::UserLogs, "docker.desktop"), p("/U/Library/Containers/com.docker.docker/Data/log"));
    assert_eq!(r(RootResolver::ApplicationCache, "docker.desktop"), p("/U/Library/Application Support/Docker Desktop"));
    assert_eq!(r(RootResolver::UserLogs, "bytedance.feishu"), p("/U/Library/Application Support/LarkShell/sdk_storage/log"));
    assert_eq!(r(RootResolver::UserLogs, "google.drive"), p("/U/Library/Application Support/Google/DriveFS/Logs"));
    assert!(r(RootResolver::ApplicationCache, "google.drive").is_empty(), "never Drive's content cache");
    assert_eq!(r(RootResolver::PackageManagerCache, "homebrew"), p("/U/Library/Caches/Homebrew"));
    assert_eq!(r(RootResolver::PackageManagerCache, "cocoapods"), p("/U/Library/Caches/CocoaPods"));
    assert_eq!(r(RootResolver::PackageManagerCache, "carthage"), p("/U/Library/Caches/org.carthage.CarthageKit"));
    assert!(r(RootResolver::PackageManagerCache, "maven").is_empty(), "~/.m2/repository is a package store, not a cache");
    let tcc = |s: &str| Target { path: PathBuf::from(s), tcc_protected: true };
    assert_eq!(r(RootResolver::AppContainer, "tencent.wechat"), vec![tcc("/U/Library/Containers/com.tencent.xinWeChat/Data/Library/Caches")]);
    assert_eq!(r(RootResolver::AppContainer, "microsoft.teams"), vec![tcc("/U/Library/Containers/com.microsoft.teams2/Data/Library/Caches")]);
    assert_eq!(r(RootResolver::AppContainer, "microsoft.office").len(), 3, "Word, Excel and PowerPoint");
    assert!(r(RootResolver::AppContainer, "microsoft.office").iter().all(|t| t.tcc_protected && !t.path.display().to_string().contains("Outlook")));
    assert_eq!(r(RootResolver::SystemLogs, "macos.library-logs"), p("/Library/Logs"));
    assert_eq!(r(RootResolver::SystemLogs, "macos.var-log"), p("/private/var/log"));
    let brew = ResolverEnv { tool_env: [("HOMEBREW_CACHE".to_string(), PathBuf::from("/brewcache"))].into(), ..env.clone() };
    assert_eq!(targets(&rule_like(RootResolver::PackageManagerCache, "homebrew"), &brew), p("/brewcache"));
    let systemless = ResolverEnv { system: None, ..env.clone() };
    assert!(targets(&rule_like(RootResolver::SystemLogs, "macos.var-log"), &systemless).is_empty());
    let homeless = ResolverEnv { home: None, ..env.clone() };
    assert!(targets(&rule_like(RootResolver::PackageManagerCache, "npm"), &homeless).is_empty());
    assert_eq!(targets(&rule_like(RootResolver::PackageManagerCache, "go"), &homeless), p("/gocache"));
}

#[test]
fn fixture_overrides_replace_the_real_locations_and_are_reported() {
    let real = ResolverEnv::from_vars(vars(&[("HOME", Path::new("/Users/a")), ("TMPDIR", Path::new("/var/folders/x/T/")), ("CARGO_HOME", Path::new("/opt/cargo"))]));
    assert_eq!(real.downloads.as_deref(), Some(Path::new("/Users/a/Downloads")));
    assert_eq!(real.tool_env.get("CARGO_HOME").map(PathBuf::as_path), Some(Path::new("/opt/cargo")));
    let (none, used) = real.clone().with_overrides(|_| None);
    assert!(used.is_empty() && none.home == real.home);
    let (env, used) = real.clone().with_overrides(vars(&[("CM_RESOLVE_HOME", Path::new("/fx/home")), ("CM_RESOLVE_TEMP", Path::new(""))]));
    assert_eq!(used, ["CM_RESOLVE_HOME"], "an empty override is not an override");
    assert_eq!(env.home.as_deref(), Some(Path::new("/fx/home")));
    assert_eq!(env.downloads.as_deref(), Some(Path::new("/fx/home/Downloads")), "Downloads follows the fixture home");
    assert_eq!(env.temp, real.temp);
    assert!(env.tool_env.is_empty(), "a fixture home never reaches the real user's tool caches");
    assert_eq!(real.system.as_deref(), Some(Path::new("/")));
    assert_eq!(env.system, None, "a fixture home never reaches the real system logs");
    let (env, used) = real.clone().with_overrides(vars(&[("CM_RESOLVE_HOME", Path::new("/fx/home")), ("CM_RESOLVE_SYSTEM", Path::new("/fx/root"))]));
    assert_eq!(used, ["CM_RESOLVE_HOME", "CM_RESOLVE_SYSTEM"]);
    assert_eq!(env.system.as_deref(), Some(Path::new("/fx/root")), "the system stand-in applies after the home override");
    let (env, used) = real.with_overrides(vars(&[("CM_RESOLVE_DOWNLOADS", Path::new("/fx/dl")), ("CM_RESOLVE_TEMP", Path::new("/fx/t"))]));
    assert_eq!(used, ["CM_RESOLVE_TEMP", "CM_RESOLVE_DOWNLOADS"]);
    assert_eq!((env.temp.as_deref(), env.downloads.as_deref()), (Some(Path::new("/fx/t")), Some(Path::new("/fx/dl"))));
    assert!(!env.tool_env.is_empty(), "without a home override the tools' own variables still apply");
    assert_eq!(env.system.as_deref(), Some(Path::new("/")));
}

/// Every rule of the shipped macOS pack has a fixed location for its owner (otherwise it could never apply).
#[test]
fn every_shipped_macos_rule_has_a_resolver_mapping() {
    let env = ResolverEnv {
        home: Some(PathBuf::from("/U")),
        temp: Some(PathBuf::from("/T")),
        downloads: Some(PathBuf::from("/U/Downloads")),
        tool_env: BTreeMap::new(),
        removable_volume: Some(PathBuf::from("/Volumes/X")),
        system: Some(PathBuf::from("/")),
    };
    let rules = mac_rules();
    for rule in &rules {
        if rule.root_resolver == RootResolver::BrowserProfile {
            assert!(BROWSERS.iter().any(|(id, ..)| *id == rule.owner.id), "{} has no browser mapping", rule.rule_id);
            continue; // profiles are listed from disk; covered by the fixture test
        }
        assert!(!targets(rule, &env).is_empty(), "{} ({:?}, owner {}) has no location", rule.rule_id, rule.root_resolver, rule.owner.id);
    }
    assert!(rules.len() >= 55, "the pack (waves 1 and 2) was read ({} rules)", rules.len());
}

// ---------------------------------------------------------------------------------------------
// Fixture tree: every rule resolves to its positive fixtures, never to its negative ones.

#[derive(Default)]
struct Rows(Vec<ObjectRow>, Vec<EntryRow>);

impl ScanSink for Rows {
    fn write(&mut self, o: &[ObjectRow], e: &[EntryRow], _: &[SkippedRow]) -> Result<(), String> {
        self.0.extend_from_slice(o);
        self.1.extend_from_slice(e);
        Ok(())
    }
    fn cancel_requested(&mut self) -> bool {
        false
    }
}

/// The real `mac.posix` scan of `root` as an analysis view.
fn scanned(root: &Path) -> MemView {
    let raw = RawPath::unix(root.as_os_str().as_bytes().to_vec()).unwrap();
    let mut rows = Rows::default();
    let report = PosixBackend.scan(&raw, &mut rows, &ScanOptions::default());
    assert_eq!(report.state, ScanState::Completed, "{:?}", report.issues);
    let objects: HashMap<u64, &ObjectRow> = rows.0.iter().map(|o| (o.object_id, o)).collect();
    let mut v = MemView { platform: Some(Platform::Macos), ..MemView::default() };
    for e in &rows.1 {
        let o = objects[&e.object_id];
        if e.parent_entry_id.is_none() {
            v.root = e.entry_id;
        }
        v.names.insert(e.entry_id, e.name.clone());
        v.parents.insert(e.entry_id, e.parent_entry_id);
        if let Some(p) = e.parent_entry_id {
            v.kids.entry(p).or_default().push(e.entry_id);
        }
        v.identities.insert((o.volume_id.clone(), o.file_id.clone()), e.entry_id);
        v.facts.insert(e.entry_id, EntryFacts {
            kind: e.kind,
            boundary: e.boundary.is_some(),
            object_id: e.object_id,
            logical: o.logical,
            allocated: o.allocated,
            volume_id: o.volume_id.clone(),
            file_id: o.file_id.clone(),
            quality: o.quality,
            cloud: o.cloud,
            modified_unix: o.modified_unix,
            link_count: o.link_count,
            skipped_here: 0,
        });
    }
    v
}

/// Fixture bases: `CM_RESOLVE_HOME`, `CM_RESOLVE_TEMP`, `CM_RESOLVE_DOWNLOADS`, the selected volume and
/// `CM_RESOLVE_SYSTEM` (the stand-in for `/`).
const BASES: [&str; 5] = ["home", "temp", "downloads", "volume", "system"];

/// The fixture manifests of every wave, merged (a rule is listed in exactly one).
fn manifest() -> Value {
    let mut rules = serde_json::Map::new();
    for wave in ["wave1", "wave2"] {
        let m: Value = serde_json::from_str(&std::fs::read_to_string(repo(&format!("rules/macos/fixtures/{wave}.json"))).unwrap()).unwrap();
        assert_eq!(m["schema"], "cleanmanager.mac-rule-fixtures/1", "{wave}");
        for (id, e) in m["rules"].as_object().unwrap() {
            assert!(rules.insert(id.clone(), e.clone()).is_none(), "{id} is listed in two waves");
        }
    }
    serde_json::json!({ "rules": rules })
}

/// `<base>:<relative>` of the fixture manifest → absolute path (trailing `/` dropped) and whether it is a directory.
fn fixture_path(root: &Path, spec: &str) -> (PathBuf, bool) {
    let (base, rel) = spec.split_once(':').unwrap_or_else(|| panic!("fixture {spec} has no base"));
    assert!(BASES.contains(&base), "unknown fixture base in {spec}");
    assert!(!rel.split('/').any(|c| c == ".."), "{spec} escapes its base");
    (root.join(base).join(rel.trim_end_matches('/')), rel.ends_with('/'))
}

fn set_mtime(p: &Path, t: SystemTime) {
    File::open(p).and_then(|f| f.set_modified(t)).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
}

/// Write every rule's fixtures (directories get one payload file); all are 30 days old except the rule's `fresh` ones.
fn build_fixtures(root: &Path, manifest: &Value) {
    for base in BASES {
        std::fs::create_dir_all(root.join(base)).unwrap();
    }
    let mut aged = Vec::new();
    for (_, entry) in manifest["rules"].as_object().unwrap() {
        let fresh: BTreeSet<&str> = entry["fresh"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        for spec in ["positive", "negative"].iter().flat_map(|k| entry["fixtures"][k].as_array().unwrap()).filter_map(Value::as_str) {
            let (p, dir) = fixture_path(root, spec);
            if dir {
                std::fs::create_dir_all(&p).unwrap();
                std::fs::write(p.join("fixture.bin"), b"cached bytes").unwrap();
            } else {
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, b"fixture").unwrap();
            }
            if !fresh.contains(spec) {
                aged.push(p);
            }
        }
    }
    // After everything is written, so later writes never refresh a directory's time.
    let old = SystemTime::now() - Duration::from_secs(30 * 86_400);
    for p in aged {
        set_mtime(&p, old);
    }
}

#[test]
fn with_cm_resolve_home_every_rule_resolves_to_its_fixtures_and_negatives_do_not_match() {
    let manifest = manifest();
    let rules = mac_rules();
    let fx = Fixture::new("catalog");
    let root = fx.path();
    build_fixtures(root, &manifest);
    // The real environment (tool variables such as CARGO_HOME included), redirected the way the CLI does it.
    let (mut env, used) = ResolverEnv::from_os().with_overrides(vars(&[
        ("CM_RESOLVE_HOME", root.join("home").as_path()),
        ("CM_RESOLVE_TEMP", root.join("temp").as_path()),
        ("CM_RESOLVE_DOWNLOADS", root.join("downloads").as_path()),
        ("CM_RESOLVE_SYSTEM", root.join("system").as_path()),
    ]));
    assert_eq!(used, OVERRIDE_VARS);
    env.removable_volume = Some(root.join("volume"));
    let view = scanned(root);
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs() as i64;
    let mut checked = 0;
    for rule in &rules {
        let entry = &manifest["rules"][&rule.rule_id];
        assert!(entry.is_object(), "{} has no fixtures", rule.rule_id);
        let positives: BTreeSet<PathBuf> = entry["fixtures"]["positive"].as_array().unwrap().iter().map(|s| fixture_path(root, s.as_str().unwrap()).0).collect();
        let negatives: Vec<PathBuf> = entry["fixtures"]["negative"].as_array().unwrap().iter().map(|s| fixture_path(root, s.as_str().unwrap()).0).collect();
        assert!(!positives.is_empty() && !negatives.is_empty(), "{}: positive and negative fixtures", rule.rule_id);
        let res = resolve(std::slice::from_ref(rule), &env);
        if entry["expect"] == "permission_required" {
            assert!(res.roots.is_empty(), "{}: a privacy-protected location is never resolved", rule.rule_id);
            assert!(res.issues.iter().any(|i| i.code == "permission_required" && i.scope == rule.rule_id && i.message.contains("permission_cause=undetermined")), "{}: {:?}", rule.rule_id, res.issues);
            checked += 1;
            continue;
        }
        assert!(res.issues.is_empty(), "{}: {:?}", rule.rule_id, res.issues);
        assert!(!res.roots.is_empty(), "{}: no root resolved", rule.rule_id);
        for r in &res.roots {
            assert!(r.path.display().starts_with(&root.display().to_string()), "{}: {} is outside the fixture tree", rule.rule_id, r.path.display());
        }
        let input = AnalysisInput { scan_id: "scan_fixture", revision: 1, now_unix: now, roots: &res.roots, protected: &[] };
        let set = analyze_preview(&view, rule, &input);
        let got: BTreeSet<PathBuf> = set.candidates.iter().map(|c| PathBuf::from(c.path.display())).collect();
        assert_eq!(got, positives, "{}: candidates must be exactly the positive fixtures ({:?})", rule.rule_id, set.inapplicable);
        for n in &negatives {
            assert!(!got.iter().any(|c| n.starts_with(c)), "{}: negative fixture {} matched", rule.rule_id, n.display());
        }
        for c in &set.candidates {
            assert_ne!(c.tier, Tier::DefaultSelected, "{}: never preselected", rule.rule_id);
            assert!(c.allowed_modes.is_empty() && c.reasons.iter().any(|r| r == "experimental_preview"), "{}: preview only", rule.rule_id);
        }
        checked += 1;
    }
    assert_eq!(checked, rules.len());
    let ids: BTreeSet<&str> = rules.iter().map(|r| r.rule_id.as_str()).collect();
    let listed: BTreeSet<&str> = manifest["rules"].as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(ids, listed, "fixtures cover exactly the shipped rules");
}

#[test]
fn unreadable_roots_are_permission_required_and_links_are_not_roots() {
    let fx = Fixture::new("resolve-perm");
    let home = fx.mkdir("home");
    let code = fx.mkdir("home/Library/Application Support/Code");
    let rule = rule_like(RootResolver::ApplicationCache, "microsoft.vscode");
    let env = ResolverEnv { home: Some(home.clone()), ..ResolverEnv::default() };
    assert_eq!(resolve(std::slice::from_ref(&rule), &env).roots.len(), 1);
    // The parent cannot be searched: the root cannot be identified, and that is reported, not skipped silently.
    let support = code.parent().unwrap().to_path_buf();
    std::fs::set_permissions(&support, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
    let res = resolve(std::slice::from_ref(&rule), &env);
    std::fs::set_permissions(&support, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    // SAFETY: plain query of this process's effective user.
    if unsafe { libc::geteuid() } != 0 {
        assert!(res.roots.is_empty());
        assert_eq!(res.issues.len(), 1, "{:?}", res.issues);
        assert_eq!((res.issues[0].code, res.issues[0].scope.as_str()), ("permission_required", rule.rule_id.as_str()));
    }
    // A symlinked root is never followed.
    std::fs::remove_dir_all(&code).unwrap();
    let elsewhere = fx.mkdir("elsewhere");
    std::os::unix::fs::symlink(&elsewhere, &code).unwrap();
    let res = resolve(std::slice::from_ref(&rule), &env);
    assert!(res.roots.is_empty() && res.issues.is_empty(), "{res:?}");
}

#[test]
fn resolved_identity_is_the_scanned_identity() {
    let fx = Fixture::new("resolve-id");
    let dir = fx.mkdir("a/b");
    let view = scanned(fx.path());
    let (volume, file) = identity_of(&dir).unwrap();
    assert!(cm_core::analysis::IndexView::find_identity(&view, &volume, &file).is_some(), "{volume} {file}");
    assert_eq!(identity_of(&fx.path().join("missing")), Err(libc::ENOENT));
    let f = fx.write("a/file", b"x");
    assert_eq!(identity_of(&f), Err(libc::ENOTDIR), "roots are directories");
}

#[test]
fn the_root_volume_and_non_volume_paths_are_never_selected() {
    for bad in ["/", "/Volumes", "/Volumes/", "/System/Volumes/Data", "/Volumes/../System", "Volumes/X", "relative", "/Volumes/a/b", "/private/var"] {
        assert!(removable_volume_root(Path::new(bad)).is_err(), "{bad}");
    }
    // The startup volume's entry in /Volumes is a link to `/`.
    for entry in std::fs::read_dir("/Volumes").unwrap().flatten() {
        let p = entry.path();
        let is_root = std::fs::canonicalize(&p).is_ok_and(|c| c == Path::new("/"));
        if is_root {
            let e = removable_volume_root(&p).unwrap_err();
            assert!(e.contains("link") || e.contains("startup"), "{e}");
        }
    }
    let e = removable_volume_root(Path::new("/Volumes/cm-no-such-volume-for-tests")).unwrap_err();
    assert!(e.contains("cm-no-such-volume-for-tests"), "{e}");
}

#[test]
fn an_attached_disk_image_is_a_selectable_volume() {
    if std::env::var_os("CM_ALLOW_DMG_TESTS").is_none() && std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        eprintln!("skipped: set CM_ALLOW_DMG_TESTS=1 to attach a disposable disk image");
        return;
    }
    let fx = Fixture::new("volume");
    let image = fx.path().join("vol.dmg");
    let name = format!("cmvol{}", std::process::id());
    let ok = Command::new("hdiutil").args(["create", "-quiet", "-size", "4m", "-fs", "HFS+", "-volname", &name, "-o"]).arg(&image).status().unwrap();
    assert!(ok.success());
    let ok = Command::new("hdiutil").args(["attach", "-quiet", "-nobrowse"]).arg(&image).status().unwrap();
    assert!(ok.success());
    let mount = PathBuf::from("/Volumes").join(&name);
    let accepted = removable_volume_root(&mount);
    let with_slash = removable_volume_root(Path::new(&format!("/Volumes/{name}/")));
    let inside = removable_volume_root(&mount.join("sub"));
    let rule = rule_like(RootResolver::RemovableVolumeRoot, "apple.metadata");
    let env = ResolverEnv { removable_volume: accepted.clone().ok(), ..ResolverEnv::default() };
    let roots = resolve(std::slice::from_ref(&rule), &env).roots;
    let _ = Command::new("hdiutil").args(["detach", "-quiet", "-force"]).arg(&mount).status();
    assert_eq!(accepted, Ok(mount.clone()));
    assert_eq!(with_slash, Ok(mount));
    assert!(inside.is_err(), "only the volume root");
    assert_eq!(roots.len(), 1, "the selected volume resolves as the removable-volume root");
}

#[test]
fn installed_versions_come_from_the_os_and_bundles() {
    let v = os_version().expect("kern.osproductversion is readable");
    assert!(cm_core::catalog::parse_version(&v).is_some_and(|p| p[0] >= 11), "{v}");
    let rules = mac_rules();
    let found = installed_versions(&rules);
    assert_eq!(found.get("macos").map(String::as_str), Some(v.as_str()));
    assert!(found.keys().all(|o| rules.iter().any(|r| r.owner.id == *o)), "{found:?}");
    for (_, version) in found {
        assert!(cm_core::catalog::parse_version(&version).is_some());
    }
}

#[test]
fn bundle_versions_need_the_owner_identifier() {
    let fx = Fixture::new("bundle");
    let app = fx.mkdir("Fake.app/Contents");
    let plist = |id: &str, v: &str| {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>{id}</string>\
             <key>CFBundleShortVersionString</key><string>{v}</string></dict></plist>"
        )
    };
    std::fs::write(app.join("Info.plist"), plist("com.google.Chrome", "131.0.6778.86")).unwrap();
    let bundle = fx.path().join("Fake.app");
    assert_eq!(bundle_version(&bundle, "com.google.Chrome").as_deref(), Some("131.0.6778.86"));
    assert_eq!(bundle_version(&bundle, "com.microsoft.edgemac"), None, "another app's bundle never answers");
    std::fs::write(app.join("Info.plist"), plist("com.google.Chrome", "beta build")).unwrap();
    assert_eq!(bundle_version(&bundle, "com.google.Chrome"), None, "unparseable versions are unknown");
}

/// Administrator-only system logs (catalog wave 2): only `system_logs` rules are `requires_admin`, they declare
/// `elevated_token`, and without root (this build has no privileged helper on macOS) every candidate is skipped as
/// `probe_unknown`, so nothing of theirs is ever preselected or planned.
#[test]
fn administrator_rules_are_skipped_without_root() {
    use cm_core::catalog::{ProbeState, SkipReason, TierInputs, compute_tier};
    use crate::macos::probes::{ProcessSnapshot, probe_candidate};
    let rules = mac_rules();
    let admin: Vec<&Rule> = rules.iter().filter(|r| r.requires_admin).collect();
    assert_eq!(admin.len(), 2, "/Library/Logs and /private/var/log");
    for r in &rules {
        assert_eq!(r.requires_admin, r.root_resolver == RootResolver::SystemLogs, "{}: system-wide locations, and only they, need administrator rights", r.rule_id);
    }
    let fx = Fixture::new("admin");
    let dir = fx.mkdir("system/private/var/log");
    let path = RawPath::unix(dir.as_os_str().as_bytes().to_vec()).unwrap();
    // SAFETY: plain query of this process's effective user.
    let root = unsafe { libc::geteuid() } == 0;
    for r in admin {
        assert!(r.probes.iter().any(|p| p == "elevated_token"), "{}", r.rule_id);
        assert_ne!(r.max_tier, Tier::DefaultSelected, "{}", r.rule_id);
        let probes = probe_candidate(&path, &r.probes, Some(&r.owner.id), &ProcessSnapshot::fixed(Ok(&[])));
        let elevated = probes.iter().find(|p| p.probe == "elevated_token").unwrap();
        assert_eq!(elevated.state, if root { ProbeState::NotInUse } else { ProbeState::Unknown }, "{}: {}", r.rule_id, elevated.detail);
        let decision = compute_tier(&TierInputs { rule_max: r.max_tier, probes: vec![elevated.clone()], ..TierInputs::default() });
        assert_ne!(decision.tier, Tier::DefaultSelected, "{}: never preselected", r.rule_id);
        if !root {
            assert_eq!(decision.skip, Some(SkipReason::ProbeUnknown), "{}: skipped without a helper", r.rule_id);
            assert!(decision.reasons.iter().any(|x| x == "probe:elevated_token:unknown"), "{}: {:?}", r.rule_id, decision.reasons);
        }
    }
}

/// Catalog roots inside privacy-protected locations while macOS shows its prompt: the lookup of Docker's logs (inside
/// its container) and the existence check of Teams' container caches do not return, so after the deadline each rule
/// reports `permission_required (permission_cause=privacy_prompt_pending)` and stays unresolved; Downloads (answered)
/// and an unprotected root resolve. Only lookups at or below a protected location run through the gate.
#[test]
fn roots_held_by_a_privacy_prompt_are_pending_not_absent() {
    use std::sync::{Arc, Mutex, mpsc};
    let fx = Fixture::new("resolve-prompt");
    let home = fx.mkdir("home");
    fx.mkdir("home/Library/Containers/com.docker.docker/Data/log");
    fx.mkdir("home/Library/Containers/com.microsoft.teams2/Data/Library/Caches");
    fx.mkdir("home/Library/Application Support/Code");
    fx.mkdir("home/Downloads");
    let rules = [
        rule_like(RootResolver::UserLogs, "docker.desktop"),
        rule_like(RootResolver::AppContainer, "microsoft.teams"),
        rule_like(RootResolver::InstallerLocation, "macos.downloads"),
        rule_like(RootResolver::ApplicationCache, "microsoft.vscode"),
    ];
    let env = ResolverEnv { home: Some(home.clone()), downloads: Some(home.join("Downloads")), ..ResolverEnv::default() };

    let (answer, prompt) = mpsc::channel::<()>();
    let prompt = Arc::new(Mutex::new(prompt));
    let calls: Arc<Mutex<Vec<(PathBuf, bool)>>> = Arc::default();
    let held = |p: &Path| p.components().any(|c| c.as_os_str() == "com.docker.docker" || c.as_os_str() == "com.microsoft.teams2");
    let (seen, wait_for) = (Arc::clone(&calls), Arc::clone(&prompt));
    let identity = Arc::new(move |p: &Path| {
        seen.lock().unwrap().push((p.to_path_buf(), std::thread::current().name() == Some("cm-privacy-gate")));
        if held(p) {
            let _ = wait_for.lock().unwrap().recv(); // held in the kernel until the person answers
        }
        identity_of(p)
    });
    let wait_for = Arc::clone(&prompt);
    let exists = Arc::new(move |p: &Path| {
        if held(p) {
            let _ = wait_for.lock().unwrap().recv();
        }
        std::fs::symlink_metadata(p).is_ok()
    });
    let t = std::time::Instant::now();
    let res = resolve_gated(&rules, &env, Duration::from_millis(200), &FirstAccess { identity, exists });
    assert!(t.elapsed() < Duration::from_secs(3), "resolution is not held by the prompt: {:?}", t.elapsed());

    let resolved: BTreeSet<&str> = res.roots.iter().map(|r| r.rule_id.as_str()).collect();
    assert_eq!(resolved, BTreeSet::from([rules[2].rule_id.as_str(), rules[3].rule_id.as_str()]), "{res:?}");
    assert_eq!(res.issues.len(), 2, "{:?}", res.issues);
    for (issue, rule, what) in [(&res.issues[0], &rules[0], "com.docker.docker/Data/log"), (&res.issues[1], &rules[1], "com.microsoft.teams2")] {
        assert_eq!((issue.code, issue.scope.as_str()), ("permission_required", rule.rule_id.as_str()));
        assert!(issue.message.starts_with(privacy::PERMISSION_PENDING) && issue.message.contains(what), "{}", issue.message);
    }
    let gated: BTreeMap<String, bool> = calls.lock().unwrap().iter().map(|(p, g)| (p.strip_prefix(&home).unwrap().display().to_string(), *g)).collect();
    assert_eq!(
        gated,
        BTreeMap::from([
            ("Library/Containers/com.docker.docker/Data/log".to_owned(), true),
            ("Downloads".to_owned(), true),
            ("Library/Application Support/Code".to_owned(), false),
        ])
    );
    drop(answer); // the parked lookups end
}
