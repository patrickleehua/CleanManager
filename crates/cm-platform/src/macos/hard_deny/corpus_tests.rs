//! The dangerous-path corpus (`tests/corpus/dangerous-paths.json`) on this Mac (PRT-03/04/05):
//! every macOS entry that does not depend on the cm-core model's fixtures goes through the
//! production [`check`] (textual pre-filter + identity chain against [`hard_deny_set`]), with
//! the corpus links created as real symbolic links inside a scratch directory. Read-only:
//! nothing outside the scratch directory is touched, and only the links are removed afterwards.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use cm_core::protect::{DenyLayer, IdentityChain, TextualDeny, check_path, prefilter};
use cm_fixtures::dangerous_paths::{self as corpus, DenyBy, Vars};

use super::*;

struct Fixture {
    dir: PathBuf,
    links: Vec<PathBuf>,
    skipped: Vec<String>,
}

impl Fixture {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let base = std::env::temp_dir().join(format!("cm-corpus-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(base.join("plain").join("x")).unwrap();
        // `$TMPDIR` is reached through the `/var` symlink; use the real `/private/var/...` path.
        let dir = std::fs::canonicalize(&base).unwrap();
        Fixture { dir, links: Vec::new(), skipped: Vec::new() }
    }

    fn create(&mut self, link: &Path, target: Option<&str>) {
        let target = target.map_or_else(|| self.dir.join(format!(".never-{}", self.links.len())), PathBuf::from);
        match std::os::unix::fs::symlink(&target, link) {
            Ok(()) => self.links.push(link.to_path_buf()),
            Err(e) => self.skipped.push(format!("{} -> {}: {e}", link.display(), target.display())),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let mut all_gone = true;
        for link in self.links.iter().rev() {
            // unlink(2) removes the symbolic link itself, never its target.
            let _ = std::fs::remove_file(link);
            if std::fs::symlink_metadata(link).is_ok() {
                all_gone = false;
                eprintln!("could not remove link fixture {}; leaving {} in place", link.display(), self.dir.display());
            }
        }
        if all_gone {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// Automounted namespaces are checked without opening them (they are all textually denied), so
/// the test never waits for autofs or the network.
fn automounted(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    ["/home", "/network", "/net"].iter().any(|p| lower == *p || lower.starts_with(&format!("{p}/")))
}

#[test]
fn hard_deny_corpus_is_denied_on_this_machine() {
    let set = hard_deny_set();
    let mut fx = Fixture::new();
    let home = std::env::var("HOME").expect("HOME");
    let user = Path::new(&home).file_name().unwrap().to_string_lossy().into_owned();
    let tmp = fx.dir.display().to_string();
    let vars = Vars::new(&[("{home}", &home), ("{user}", &user), ("{tmp}", &tmp)]);

    let c = corpus::load().unwrap_or_else(|e| panic!("{e}"));
    let entries: Vec<_> = c.for_platform(corpus::Platform::Macos).filter(|e| !e.model_only).collect();
    for e in &entries {
        if e.link.is_some() {
            let link = e.text(&vars);
            assert!(link.starts_with(&tmp), "real link fixtures must live in {{tmp}}: {}", e.describe(&vars));
            fx.create(Path::new(&link), e.link_target(&vars).as_deref());
        }
    }
    let links_wanted = entries.iter().filter(|e| e.link.is_some()).count();
    assert!(fx.skipped.is_empty(), "symlink fixtures failed: {:?}", fx.skipped);

    let (mut failures, mut by_identity) = (Vec::new(), 0);
    for e in &entries {
        let text = e.text(&vars);
        let raw = RawPath::unix(e.unix_bytes(&vars)).ok();
        let textual = raw.as_ref().map_or(Err(TextualDeny::Empty), prefilter);
        let decision = match &raw {
            None => ProtectionDecision::Denied { layer: DenyLayer::HardDeny, reason: "not representable as a path".into() },
            Some(p) if automounted(&text) => check_path(p, &IdentityChain::default(), &set),
            Some(p) => check(p, &set),
        };
        if decision.is_allowed() {
            failures.push(format!("ALLOWED {}", e.describe(&vars)));
            continue;
        }
        match (e.deny_by, &textual) {
            (DenyBy::Prefilter, Ok(())) => failures.push(format!("expected the textual filter to deny {}", e.describe(&vars))),
            (DenyBy::Identity, Err(t)) => failures.push(format!("expected only identity to deny {}, but the textual filter did: {t}", e.describe(&vars))),
            _ => {}
        }
        if textual.is_ok() {
            by_identity += 1;
        }
    }
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    assert!(by_identity >= 15, "identity-only denials exercise the real identity chain ({by_identity})");

    // Not vacuous: the scratch area and a symlink to an ordinary directory are allowed.
    let ok = fx.dir.join("lok");
    fx.create(&ok, Some(&fx.dir.join("plain").display().to_string()));
    for p in [fx.dir.join("plain").join("x"), ok.clone(), ok.join("x")] {
        let d = check(&RawPath::unix(p.as_os_str().as_bytes().to_vec()).unwrap(), &set);
        assert!(d.is_allowed(), "{}: {d:?}", p.display());
    }
    println!("macos corpus on this machine: {} entries denied ({by_identity} by identity only), {links_wanted} symlink fixtures", entries.len());
}
