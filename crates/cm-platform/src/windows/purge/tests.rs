//! Real-NTFS tests for purge evidence (temp directories only). Git is used only to build
//! fixtures; the probe itself never runs it.

use std::process::Command;

use super::*;

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-purge-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn write(p: &Path, body: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn git(dir: &Path, args: &[&str]) -> bool {
    Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"]).args(args).output().is_ok_and(|o| o.status.success())
}

fn r(p: &Path) -> RawPath {
    raw(p).unwrap()
}

#[test]
fn git_index_decides_tracked_untracked_and_unknown() {
    let root = scratch("git");
    let repo = root.join("repo");
    write(&repo.join("package.json"), "{}");
    write(&repo.join("dist").join("app.js"), "committed");
    write(&repo.join("node_modules").join("x").join("i.js"), "installed");
    if !git(&repo, &["init", "-q"]) || !git(&repo, &["add", "package.json", "dist/app.js"]) {
        eprintln!("skipped: git is not available to build the fixture");
        return;
    }
    let p = FsProbe::default();
    assert_eq!(p.vcs(&r(&repo.join("dist"))), VcsState::Tracked(1));
    assert_eq!(p.vcs(&r(&repo.join("node_modules"))), VcsState::Untracked);
    assert_eq!(p.vcs(&r(&root.join("elsewhere"))), VcsState::NotVersioned);
    // A linked worktree: `.git` is a file pointing at the real git directory.
    assert!(git(&repo, &["commit", "-q", "-m", "c"]));
    let wt = root.join("wt");
    assert!(git(&repo, &["worktree", "add", "-q", wt.to_str().unwrap()]));
    write(&wt.join("target").join("o"), "x");
    assert!(std::fs::metadata(wt.join(".git")).unwrap().is_file());
    assert_eq!(FsProbe::default().vcs(&r(&wt.join("dist"))), VcsState::Tracked(1));
    assert_eq!(FsProbe::default().vcs(&r(&wt.join("target"))), VcsState::Untracked);
    // A corrupt index is "unknown", never "untracked".
    std::fs::write(repo.join(".git").join("index"), b"DIRC\0\0\0\x02\0\0\0\x05junk").unwrap();
    assert!(matches!(FsProbe::default().vcs(&r(&repo.join("node_modules"))), VcsState::Unknown(_)));
    // Another version-control system is not read: unknown.
    let hg = root.join("hgrepo");
    std::fs::create_dir_all(hg.join(".hg")).unwrap();
    assert!(matches!(FsProbe::default().vcs(&r(&hg.join("build"))), VcsState::Unknown(_)));
}

#[test]
fn repository_without_an_index_tracks_nothing() {
    let root = scratch("noindex");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    assert_eq!(FsProbe::default().vcs(&r(&root.join("target"))), VcsState::Untracked);
}

#[test]
fn cachedir_tag_needs_the_signature() {
    let root = scratch("tag");
    write(&root.join("a").join("CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55\n# created by cargo");
    write(&root.join("b").join("CACHEDIR.TAG"), "Signature: not the right one at all, sorry");
    let p = FsProbe::default();
    assert!(p.cachedir_tag(&r(&root.join("a").join("CACHEDIR.TAG"))));
    assert!(!p.cachedir_tag(&r(&root.join("b").join("CACHEDIR.TAG"))));
    assert!(!p.cachedir_tag(&r(&root.join("missing"))));
}

#[test]
fn settings_are_versioned_and_never_silently_reset() {
    let store = scratch("settings");
    assert_eq!(load_settings(&store).unwrap(), PurgeSettings::default());
    let mut s = PurgeSettings::default();
    assert!(s.add_root(r(&store.join("code")), &[]));
    s.recent_days = 3;
    save_settings(&store, &s).unwrap();
    assert_eq!(load_settings(&store).unwrap(), s);
    std::fs::write(settings_path(&store), "{\"schema_version\": 1, \"oops\": true}").unwrap();
    assert!(load_settings(&store).is_err());
}

#[test]
fn default_roots_are_plain_existing_directories() {
    for root in default_roots() {
        let p = os(&root).unwrap();
        assert!(plain_dir(&p), "{}", p.display());
    }
}
