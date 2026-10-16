//! Purge evidence on a real macOS file system (fixtures under the canonical temporary directory). Git builds the
//! fixtures through `std::process::Command`; the probe asks it through the trusted tool runner.

use std::process::Command;

use super::*;
use crate::macos::testfs::Fixture;

fn r(p: &Path) -> RawPath {
    raw(p).unwrap()
}

fn fixture_git(dir: &Path, args: &[&str]) -> bool {
    Command::new(GIT)
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Git when this Mac has a developer directory (the hosted runner has Xcode); `None` prints why a test is skipped.
fn real_git() -> Option<Git> {
    match Git::detect() {
        g @ Git::Available(_) => Some(g),
        Git::Unavailable(why) => {
            eprintln!("skipped: {why}");
            None
        }
    }
}

#[test]
fn raw_paths_round_trip_bytes() {
    let p = Path::new(OsStr::from_bytes(b"/tmp/caf\xc3\xa9/\xff-not-utf8"));
    assert_eq!(os(&r(p)).unwrap(), p);
    assert_eq!(os(&RawPath::windows("C:\\x".encode_utf16().collect()).unwrap()), None);
}

#[test]
fn cachedir_tag_needs_the_signature() {
    let fx = Fixture::new("purge-tag");
    let good = fx.write("a/CACHEDIR.TAG", b"Signature: 8a477f597d28d172789f06886806bc55\n# created by cargo");
    let bad = fx.write("b/CACHEDIR.TAG", b"Signature: not the right one at all, sorry");
    let p = FsProbe::with_git(Git::Unavailable("not needed".into()));
    assert!(p.cachedir_tag(&r(&good)));
    assert!(!p.cachedir_tag(&r(&bad)));
    assert!(!p.cachedir_tag(&r(&fx.path().join("missing"))));
    // A link to a valid tag is never followed.
    let link = fx.path().join("c-CACHEDIR.TAG");
    std::os::unix::fs::symlink(&good, &link).unwrap();
    assert!(!p.cachedir_tag(&r(&link)));
    assert!(open_plain_file(&link).is_none() && open_plain_file(fx.path()).is_none());
}

#[test]
fn settings_are_versioned_and_never_silently_reset() {
    let fx = Fixture::new("purge-settings");
    let store = fx.path();
    assert_eq!(load_settings(store).unwrap(), PurgeSettings::default());
    let mut s = PurgeSettings::default();
    assert!(s.add_root(r(&store.join("code")), &[]));
    s.recent_days = 3;
    save_settings(store, &s).unwrap();
    assert_eq!(load_settings(store).unwrap(), s);
    std::fs::write(settings_path(store), "{\"schema_version\": 1, \"oops\": true}").unwrap();
    assert!(load_settings(store).is_err());
}

#[test]
fn default_roots_are_plain_existing_directories_and_same_dir_uses_identity() {
    for root in default_roots() {
        let p = os(&root).unwrap();
        assert!(plain_dir(&p), "{}", p.display());
    }
    let fx = Fixture::new("purge-same");
    let a = fx.mkdir("Projects");
    assert!(same_dir(&a, &fx.path().join("Projects/")));
    assert!(same_dir(&a, &fx.path().join("projects")) || !fx.path().join("projects").exists(), "case-insensitive volumes: one folder");
    assert!(!same_dir(&a, fx.path()));
    assert!(same_dir(Path::new("/no/such/x/"), Path::new("/no/such/x")));
}

#[test]
fn git_decides_tracked_untracked_unignored_and_unknown() {
    let Some(git) = real_git() else { return };
    let fx = Fixture::new("purge-git");
    let repo = fx.mkdir("repo");
    fx.write("repo/package.json", b"{}");
    fx.write("repo/.gitignore", b"node_modules/\n/target/\n");
    fx.write("repo/dist/app.js", b"committed bundle");
    fx.write("repo/node_modules/x/i.js", b"installed");
    fx.write("repo/target/debug/out.bin", b"ignored output");
    fx.write("repo/build/hand-written.js", b"not committed, not ignored");
    assert!(fixture_git(&repo, &["init", "-q"]) && fixture_git(&repo, &["add", "package.json", ".gitignore", "dist/app.js"]));
    let p = FsProbe::with_git(git.clone());
    assert_eq!(p.vcs(&r(&repo.join("dist"))), VcsState::Tracked(1));
    assert_eq!(p.vcs(&r(&repo.join("node_modules"))), VcsState::Untracked);
    assert_eq!(p.vcs(&r(&repo.join("target"))), VcsState::Untracked);
    match p.vcs(&r(&repo.join("build"))) {
        VcsState::Unknown(why) => assert!(why.contains("not ignored"), "{why}"),
        other => panic!("unignored untracked files are not 'untracked': {other:?}"),
    }
    assert_eq!(p.vcs(&r(&fx.path().join("elsewhere").join("target"))), VcsState::NotVersioned);

    // Committed later: the next probe (a new operation) sees it.
    assert!(fixture_git(&repo, &["add", "-f", "target/debug/out.bin"]));
    assert_eq!(FsProbe::with_git(git.clone()).vcs(&r(&repo.join("target"))), VcsState::Tracked(1));

    // A linked worktree: `.git` is a file pointing at the real git directory.
    assert!(fixture_git(&repo, &["commit", "-q", "-m", "c"]));
    let wt = fx.path().join("wt");
    assert!(fixture_git(&repo, &["worktree", "add", "-q", wt.to_str().unwrap()]));
    std::fs::create_dir_all(wt.join("node_modules").join("y")).unwrap();
    std::fs::write(wt.join("node_modules").join("y").join("i.js"), b"x").unwrap();
    assert!(std::fs::symlink_metadata(wt.join(".git")).unwrap().is_file());
    let p = FsProbe::with_git(git.clone());
    assert_eq!(p.vcs(&r(&wt.join("dist"))), VcsState::Tracked(1));
    assert_eq!(p.vcs(&r(&wt.join("node_modules"))), VcsState::Untracked);

    // Not a repository Git accepts: unknown, never "untracked".
    let broken = fx.mkdir("broken/.git");
    assert!(matches!(p.vcs(&r(&broken.parent().unwrap().join("target"))), VcsState::Unknown(_)));
    // Another version-control system is not read.
    fx.mkdir("hgrepo/.hg");
    assert!(matches!(p.vcs(&r(&fx.path().join("hgrepo").join("build"))), VcsState::Unknown(_)));
}

#[test]
fn without_git_working_copies_are_unknown_and_plain_folders_are_not_versioned() {
    let fx = Fixture::new("purge-nogit");
    fx.mkdir("repo/.git");
    fx.write("repo/target/out.bin", b"x");
    fx.write("plain/target/out.bin", b"x");
    let p = FsProbe::with_git(Git::Unavailable("Git is not available (fixture)".into()));
    assert_eq!(p.vcs(&r(&fx.path().join("repo").join("target"))), VcsState::Unknown("Git is not available (fixture)".into()));
    assert_eq!(p.vcs(&r(&fx.path().join("plain").join("target"))), VcsState::NotVersioned);
    // A `.git` link is never followed (whatever Git would say).
    fx.mkdir("real-git-dir");
    fx.mkdir("linked");
    std::os::unix::fs::symlink(fx.path().join("real-git-dir"), fx.path().join("linked").join(".git")).unwrap();
    let p = FsProbe::with_git(Git::Available(PathBuf::from(GIT)));
    match p.vcs(&r(&fx.path().join("linked").join("target"))) {
        VcsState::Unknown(why) => assert!(why.contains("link"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn git_detection_reports_a_reason_when_unavailable() {
    match Git::detect() {
        Git::Available(p) => assert_eq!(p, Path::new(GIT)),
        Git::Unavailable(why) => assert!(why.contains("Git"), "{why}"),
    }
    assert_eq!(entries(b"a\0b/c\0\0"), 2);
    assert_eq!(entries(b""), 0);
}

#[test]
fn the_capability_row_is_unverified_with_a_reason() {
    assert_eq!(CAPS.len(), 1);
    assert_eq!((CAPS[0].0, CAPS[0].1), ("purge.artifacts", CapabilityState::Unverified));
    assert!(CAPS[0].2.contains("gate G1 not passed"));
}
