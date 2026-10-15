//! The dangerous-path corpus (`tests/corpus/dangerous-paths.json`) on this machine (PRT-03/04/05):
//! every Windows entry that does not depend on the cm-core model's fixtures goes through the
//! production [`check`] (textual pre-filter + identity chain against [`hard_deny_set`]), with the
//! corpus links created as real junctions, symbolic links and a mount point (a junction to the
//! system volume's GUID root) inside a scratch directory. Read-only: nothing is ever modified
//! outside the scratch directory, and only the links themselves are removed afterwards.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use cm_core::protect::{DenyLayer, IdentityChain, check_path, prefilter};
use cm_fixtures::dangerous_paths::{self as corpus, DenyBy, LinkKind, Vars};

use super::*;

/// Scratch directory with link fixtures; removes the links (never their targets) first and the
/// directory only when every link is gone.
struct Fixture {
    dir: PathBuf,
    links: Vec<(PathBuf, bool)>,
    skipped: Vec<String>,
}

impl Fixture {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let base = std::env::temp_dir().join(format!("cm-corpus-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(base.join("plain").join("x")).unwrap();
        // Long, non-verbatim spelling (GetTempPath may return an 8.3 form).
        let canon = std::fs::canonicalize(&base).unwrap();
        let text = canon.display().to_string();
        let dir = PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text));
        Fixture { dir, links: Vec::new(), skipped: Vec::new() }
    }

    fn mklink_j(link: &Path, target: &str) -> Result<(), String> {
        let out = Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(link).arg(target).output().map_err(|e| e.to_string())?;
        if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_owned()) }
    }

    fn create(&mut self, link: &Path, kind: LinkKind, target: Option<&str>) {
        let n = self.links.len();
        let result = match (kind, target) {
            (LinkKind::Junction | LinkKind::Mount, Some(t)) => Self::mklink_j(link, t),
            (LinkKind::Junction | LinkKind::Mount, None) => {
                let gone = self.dir.join(format!(".gone-{n}"));
                std::fs::create_dir(&gone).map_err(|e| e.to_string()).and_then(|()| {
                    let r = Self::mklink_j(link, &gone.display().to_string());
                    std::fs::remove_dir(&gone).map_err(|e| e.to_string()).and(r)
                })
            }
            (LinkKind::Symlink, t) => std::os::windows::fs::symlink_dir(t.map_or_else(|| self.dir.join(format!(".never-{n}")), PathBuf::from), link).map_err(|e| e.to_string()),
            (LinkKind::FileSymlink, t) => std::os::windows::fs::symlink_file(t.map_or_else(|| self.dir.join(format!(".never-{n}")), PathBuf::from), link).map_err(|e| e.to_string()),
        };
        match result {
            Ok(()) => self.links.push((link.to_path_buf(), kind == LinkKind::FileSymlink)),
            Err(e) => self.skipped.push(format!("{} ({kind:?}): {e}", link.display())),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let mut all_gone = true;
        for (link, file) in self.links.iter().rev() {
            // Removing a junction or symbolic link deletes the reparse point only.
            let _ = if *file { std::fs::remove_file(link) } else { std::fs::remove_dir(link) };
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

/// Host of a UNC path other than this machine: such entries are checked without opening them
/// (they are all textually denied), so the test never contacts the network.
fn remote_unc_host(text: &str) -> Option<String> {
    let t = text.replace('/', "\\");
    let rest = if t.get(..8).is_some_and(|p| p.eq_ignore_ascii_case(r"\\?\UNC\")) {
        &t[8..]
    } else if t.starts_with(r"\\") && !t.starts_with(r"\\?\") && !t.starts_with(r"\\.\") {
        &t[2..]
    } else {
        return None;
    };
    let host = rest.split('\\').next().unwrap_or("").to_ascii_lowercase();
    (!matches!(host.as_str(), "localhost" | "127.0.0.1")).then_some(host)
}

fn system_volume_root() -> String {
    let drive = env_path("SystemDrive").unwrap_or_else(|| PathBuf::from("C:"));
    let root = wide(&PathBuf::from(format!("{}\\", drive.display())));
    let h = Handle::open(&full_path(&root).unwrap().1, FILE_READ_ATTRIBUTES | SYNCHRONIZE).unwrap();
    format!("{}\\", h.volume_guid().expect("system volume has a GUID name"))
}

#[test]
fn hard_deny_corpus_is_denied_on_this_machine() {
    let set = hard_deny_set();
    let mut fx = Fixture::new();
    let home = env_path("USERPROFILE").expect("USERPROFILE");
    let user = home.file_name().unwrap().to_string_lossy().into_owned();
    let tmp = fx.dir.display().to_string();
    let vars = Vars::new(&[("{home}", &home.display().to_string()), ("{user}", &user), ("{tmp}", &tmp), ("{sysvol}", &system_volume_root())]);

    let c = corpus::load().unwrap_or_else(|e| panic!("{e}"));
    let entries: Vec<_> = c.for_platform(corpus::Platform::Windows).filter(|e| !e.model_only).collect();
    for e in &entries {
        if let Some(l) = &e.link {
            let link = e.text(&vars);
            assert!(link.starts_with(&tmp), "real link fixtures must live in {{tmp}}: {}", e.describe(&vars));
            fx.create(Path::new(&link), l.kind, e.link_target(&vars).as_deref());
        }
    }
    let links_wanted = entries.iter().filter(|e| e.link.is_some()).count();
    for s in &fx.skipped {
        eprintln!("link fixture skipped: {s}");
    }
    assert!(fx.links.len() * 10 >= links_wanted * 7, "only {}/{links_wanted} link fixtures could be created", fx.links.len());

    let (mut failures, mut by_identity, mut offline) = (Vec::new(), 0, 0);
    for e in &entries {
        let text = e.text(&vars);
        let raw = RawPath::windows(e.windows_units(&vars)).ok();
        let textual = raw.as_ref().map_or(Err(protect::TextualDeny::Empty), prefilter);
        let decision = match &raw {
            None => ProtectionDecision::Denied { layer: DenyLayer::HardDeny, reason: "not representable as a path".into() },
            Some(p) if remote_unc_host(&text).is_some() => {
                offline += 1;
                check_path(p, &IdentityChain::default(), &set)
            }
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
    assert!(by_identity >= 30, "identity-only denials exercise the real identity chain ({by_identity})");

    // Not vacuous: the scratch area and a junction to an ordinary directory are allowed.
    let plain = fx.dir.join("plain").join("x");
    let ok = fx.dir.join("jok");
    fx.create(&ok, LinkKind::Junction, Some(&fx.dir.join("plain").display().to_string()));
    for p in [plain, ok.clone(), ok.join("x")] {
        let d = check(&RawPath::windows(wide(&p)).unwrap(), &set);
        assert!(d.is_allowed(), "{}: {d:?}", p.display());
    }
    println!(
        "windows corpus on this machine: {} entries denied ({by_identity} by identity only, {offline} remote UNC checked without opening), {}/{links_wanted} link fixtures",
        entries.len(),
        fx.links.len() - 1
    );
}
