//! UNI-07 (npm-global AI CLIs) and UNI-08 (orphans) on macOS, on fixtures under a redirected root and home only: a
//! fixture `bin/node` script stands in for Node.js; no real npm, package, `~/.claude` or `~/Library` is touched.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command};

use cm_core::uninstall::{Group, LeftoverKind, Marker};

use super::*;
use crate::macos::ffi::cf::PlistFormat;
use crate::macos::testfs::Fixture;

struct Env {
    fx: Fixture,
    env: UninstallEnv,
}

impl Env {
    fn new(tag: &str) -> Env {
        let fx = Fixture::new(tag);
        let apps = AppsEnv { root: Some(fx.mkdir("root")), home: Some(fx.mkdir("home")) };
        Env { env: UninstallEnv { apps, quit_wait: Duration::from_millis(1500), is_admin: false }, fx }
    }

    fn root(&self) -> PathBuf {
        self.env.apps.root.clone().unwrap()
    }

    fn home(&self) -> PathBuf {
        self.env.apps.home.clone().unwrap()
    }

    fn write(&self, at: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, bytes).unwrap();
    }
}

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A fixture Node.js in `prefix`: `bin/node` logs its arguments and, for the expected argv only, removes the package
/// folder below the fixture as npm would; `@openai/codex` fails.
fn fake_node(e: &Env, prefix: &Path, log: &Path) {
    let node = prefix.join("bin/node");
    let base = e.fx.path().display().to_string();
    let script = format!(
        "#!/bin/sh\necho \"$*\" >> '{log}'\n[ \"$NO_UPDATE_NOTIFIER\" = 1 ] && [ -n \"$HOME\" ] || exit 7\n[ \"$2 $3 $4 $6\" = 'uninstall --global --prefix --ignore-scripts' ] || exit 9\ncase \"$5\" in '{base}'/*) ;; *) exit 6;; esac\n[ \"$7\" = '@openai/codex' ] && {{ echo 'npm ERR! fixture' >&2; exit 1; }}\nrm -rf \"$5/lib/node_modules/$7\"\n",
        log = log.display()
    );
    e.write(&node, script.as_bytes());
    std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o755)).unwrap();
    e.write(&prefix.join("lib/node_modules/npm/bin/npm-cli.js"), b"// npm stand-in\n");
}

fn package(e: &Env, prefix: &Path, name: &str, version: &str) -> PathBuf {
    let dir = name.split('/').fold(prefix.join("lib/node_modules"), |p, c| p.join(c));
    e.write(&dir.join("package.json"), format!(r#"{{"name":"{name}","version":"{version}"}}"#).as_bytes());
    e.write(&dir.join("cli.js"), b"while :; do sleep 1; done\n");
    dir
}

fn record(e: &Env, name: &str, prefix: &Path) -> UninstallRecord {
    let p = prefix.to_string_lossy().into_owned();
    records(&e.env).0.into_iter().find(|r| r.name == name && r.mac.as_ref().and_then(|m| m.npm_prefix.as_deref()) == Some(p.as_str())).unwrap_or_else(|| panic!("no {name} in {p}"))
}

#[test]
fn npm_ai_clis_are_listed_per_prefix_and_removed_through_a_trusted_node() {
    let e = Env::new("unin-ai");
    let log = e.fx.path().join("node.log");
    let brew = e.root().join("opt/homebrew");
    fake_node(&e, &brew, &log);
    let claude = package(&e, &brew, "@anthropic-ai/claude-code", "1.0.5");
    // nvm keeps its own prefix per version; without a Node.js of its own there, the Homebrew one removes it.
    let nvm = e.home().join(".nvm/versions/node/v20.11.0");
    let codex = package(&e, &nvm, "@openai/codex", "0.9.0");
    // A linked package is a developer checkout: not listed.
    std::fs::create_dir_all(e.home().join(".npm-global/lib/node_modules/@google")).unwrap();
    std::os::unix::fs::symlink(&codex, e.home().join(".npm-global/lib/node_modules/@google/gemini-cli")).unwrap();

    let (recs, _) = records(&e.env);
    let ai: Vec<&UninstallRecord> = recs.iter().filter(|r| r.source == RecordSource::NpmGlobal).collect();
    assert_eq!(ai.len(), 2, "{ai:#?}");
    let rec = record(&e, "Claude Code", &brew);
    assert_eq!((rec.version.as_deref(), rec.ai_tool.as_deref(), rec.install_location.as_deref()), (Some("1.0.5"), Some("claude-code"), claude.to_str()));
    let choice = route(&e.env, &rec).unwrap();
    assert_eq!((choice.kind, choice.program.as_str()), (AdapterKind::NpmGlobal, brew.join("bin/node").to_str().unwrap()));
    assert_eq!(choice.argv[1..], ["uninstall", "--global", "--prefix", brew.to_str().unwrap(), "--ignore-scripts", "@anthropic-ai/claude-code"]);
    let codex_rec = record(&e, "Codex CLI", &nvm);
    let codex_choice = route(&e.env, &codex_rec).unwrap();
    assert_eq!((codex_choice.program.as_str(), codex_choice.argv[4].as_str()), (brew.join("bin/node").to_str().unwrap(), nvm.to_str().unwrap()));

    let u = MacUninstall::non_interactive(e.env.clone());
    let (sha, signer) = verify_program(&choice).unwrap();
    assert!(sha.len() == 64 && signer.is_none());
    u.verify(&rec, &choice, &sha, None).unwrap();
    assert_eq!(u.run_step(&choice, 60).unwrap().0, Some(0));
    assert!(!claude.exists(), "removed by the (fixture) package manager");
    let r = u.rescan(&AppIdentity::of(&rec));
    assert_eq!((r.install_dir_present, r.registration_present), (Some(false), Some(false)));
    assert_eq!(u.run_step(&codex_choice, 60).unwrap().0, Some(1), "a failing npm is reported by its exit code");
    assert!(codex.exists());
    let runs = std::fs::read_to_string(&log).unwrap();
    assert!(runs.lines().next().unwrap().ends_with(&format!("uninstall --global --prefix {} --ignore-scripts @anthropic-ai/claude-code", brew.display())), "{runs}");

    // npm others may change is refused before anything runs, and no route is offered with it.
    let npm = brew.join("lib/node_modules/npm/bin/npm-cli.js");
    std::fs::set_permissions(&npm, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(verify_program(&codex_choice).is_err_and(|(c, m)| c == "policy_denied" && m.contains("writable by others")));
    let err = route(&e.env, &codex_rec).unwrap_err();
    assert!(err.code == "unsupported" && err.message.contains("npm uninstall -g @openai/codex"), "{err:?}");

    // Last used: the newest entry of the data folder.
    assert_eq!(ai_last_used(&e.env, "codex"), None);
    e.write(&e.home().join(".codex/history.jsonl"), b"{}\n");
    assert!(ai_last_used(&e.env, "codex").is_some());
}

#[test]
fn ai_data_folder_leftovers_never_preselect_sessions_settings_or_worktrees() {
    let e = Env::new("unin-ai-left");
    let brew = e.root().join("opt/homebrew");
    package(&e, &brew, "@anthropic-ai/claude-code", "1.0.5");
    let rec = record(&e, "Claude Code", &brew);
    let d = e.home().join(".claude");
    e.write(&d.join("shell-snapshots/snap.sh"), &[1u8; 40]);
    e.write(&d.join("projects/p1/session.jsonl"), b"{}");
    e.write(&d.join("settings.json"), b"{}");
    e.write(&d.join(".credentials.json"), b"{}");
    e.write(&d.join("statsig/chats/c.json"), b"{}");
    e.write(&d.join("worktrees/w1/.git/HEAD"), b"ref");
    e.write(&e.home().join(".claude.json"), b"{}");
    let items = discover(&e.env, &AppIdentity::of(&rec), rec.mac.as_ref(), &[], true);
    let by = |suffix: &str| items.iter().find(|l| l.display.ends_with(suffix)).unwrap_or_else(|| panic!("{suffix}: {items:#?}"));
    assert_eq!((by("shell-snapshots").selected, by("shell-snapshots").logical_bytes), (true, Some(40)));
    for s in ["projects", "settings.json", ".credentials.json", "statsig"] {
        assert!(!by(s).selected, "{s}: {:?}", by(s));
    }
    assert_eq!(by("statsig").group, Group::PersonalData, "a cache holding chats is session data");
    assert_eq!(by("worktrees").tier, "blocked");
    assert!(matches!(by(".claude.json").binding, Binding::Manual { .. }));
    assert!(items.iter().all(|l| !l.selected || l.group == Group::CachesLogs), "{items:#?}");
    // The package folder itself is still there (nothing ran): offered, never preselected.
    let pkg = by("@anthropic-ai/claude-code");
    assert_eq!((pkg.group, pkg.selected), (Group::Bundle, false));
    // Another installation of the tool remains: everything is kept.
    let other = MacInstalled { instance_id: "app_other".into(), name: "Claude Code".into(), bundle_id: None, location: Some("/x".into()) };
    let kept = discover(&e.env, &AppIdentity::of(&rec), rec.mac.as_ref(), &[other], false);
    assert!(kept.iter().filter(|l| l.group != Group::Bundle).all(|l| !l.selected && l.tier != "review_only"), "{kept:#?}");
}

#[test]
fn a_node_script_inside_the_package_counts_as_running() {
    let e = Env::new("unin-ai-run");
    let brew = e.root().join("opt/homebrew");
    let dir = package(&e, &brew, "@google/gemini-cli", "0.1.0");
    let rec = record(&e, "Gemini CLI", &brew);
    let u = MacUninstall::non_interactive(e.env.clone());
    let id = AppIdentity::of(&rec);
    assert_eq!(u.running(&id), RunningState::Known { processes: vec![] });
    // `/bin/sh <package>/cli.js`: the executable is outside the package, the script argument inside it.
    let child = Running(Command::new("/bin/sh").arg(dir.join("cli.js")).spawn().unwrap());
    let mut found = vec![];
    for _ in 0..50 {
        if let RunningState::Known { processes } = u.running(&id)
            && !processes.is_empty()
        {
            found = processes;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(found.iter().any(|p| p.pid == child.0.id()), "{found:?}");
    assert!(u.request_quit(&found).is_empty(), "SIGTERM ends the script");
}

fn touch_old(paths: &[PathBuf]) {
    for p in paths {
        let ok = Command::new("/usr/bin/touch").args(["-h", "-t", "202001010000"]).arg(p).status().unwrap().success();
        assert!(ok, "touch {}", p.display());
    }
}

#[test]
fn orphans_need_a_bundle_id_no_installed_app_has_and_stay_unselected() {
    let e = Env::new("unin-orphans");
    let apps = e.root().join("Applications");
    let exe = apps.join("Quill.app/Contents/MacOS/Quill");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::copy("/usr/bin/true", &exe).unwrap();
    let info = |pairs: &[(&str, &str)]| {
        let map: BTreeMap<String, Plist> = pairs.iter().map(|(k, v)| ((*k).to_owned(), Plist::String((*v).to_owned()))).collect();
        cf::plist_to_bytes(&Plist::Dict(map), PlistFormat::Xml).unwrap()
    };
    e.write(&apps.join("Quill.app/Contents/Info.plist"), &info(&[("CFBundleIdentifier", "com.example.quill"), ("CFBundleName", "Quill"), ("CFBundleExecutable", "Quill")]));
    e.write(&e.root().join("var/db/receipts/com.pkgvendor.tool.plist"), b"r");
    let lib = e.home().join("Library");
    let files = [
        "Application Support/com.gone.app/data.db", "Caches/com.gone.app/blob", "Preferences/com.gone.app.plist", "Caches/com.example.quill/c",
        "Caches/com.apple.Safari/c", "Caches/Decoy Tool/c", "Caches/com.pkgvendor.tool/c", "Containers/com.gone.app/Data/x",
    ];
    let mut old = Vec::new();
    for f in files {
        let p = lib.join(f);
        e.write(&p, &[3u8; 64]);
        old.push(p.clone());
        old.push(p.parent().unwrap().to_path_buf());
    }
    let agent = cf::plist_to_bytes(
        &Plist::Dict(BTreeMap::from([
            ("Label".to_owned(), Plist::String("com.gone.updater".into())),
            ("ProgramArguments".to_owned(), Plist::Array(vec![Plist::String("/Applications/Gone.app/Contents/MacOS/upd".into())])),
        ])),
        PlistFormat::Binary,
    )
    .unwrap();
    e.write(&lib.join("LaunchAgents/com.gone.updater.plist"), &agent);
    old.push(lib.join("LaunchAgents/com.gone.updater.plist"));
    touch_old(&old);
    // Recently used: kept, owner unknown.
    e.write(&lib.join("Caches/com.recent.app/c"), b"new");

    let items = orphans(&e.env, true);
    let find = |suffix: &str| items.iter().find(|l| l.display.ends_with(suffix));
    let listed = |suffix: &str| find(suffix).unwrap_or_else(|| panic!("{suffix}: {items:#?}"));
    let support = listed("Application Support/com.gone.app");
    assert_eq!((support.tier.as_str(), support.selected, support.marker, support.logical_bytes), ("review_only", false, None, Some(64)));
    let cache = listed("Caches/com.gone.app");
    assert_eq!((cache.group, cache.selected, cache.tier.as_str()), (Group::CachesLogs, false, "review_only"), "orphan caches are never preselected");
    assert_eq!(listed("com.gone.app.plist").kind, LeftoverKind::File);
    let container = listed("Containers/com.gone.app");
    assert_eq!((container.tier.as_str(), container.logical_bytes), ("blocked", None));
    assert!(listed("com.gone.updater.plist").evidence.iter().any(|x| x.contains("Gone.app no longer exists")));
    assert_eq!(listed("com.recent.app").marker, Some(Marker::OwnerUnknown));
    for s in ["com.example.quill", "com.apple.Safari", "Decoy Tool", "com.pkgvendor.tool"] {
        assert!(find(s).is_none(), "{s} is not an orphan: {items:#?}");
    }
    assert!(items.iter().all(|l| !l.selected));
}
