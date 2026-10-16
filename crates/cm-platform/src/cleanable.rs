//! Live evidence behind the disk view's cleanable-directory hints (DIN-05). Only the directories
//! of the page being shown are probed, read-only: one cache-tag head read each, the parent's
//! names when the directory carries an artifact name, and the protection layers once a
//! signature is found. Nothing is selected, planned or written.

use std::cell::OnceCell;
use std::path::{Path, PathBuf};

use cm_core::cleanable::{CacheLocation, CleanableHint, HintProbe, hint_for};
use cm_core::path::RawPath;
use cm_core::protect::{HardDenySet, Subject, UserProtection};
use cm_core::rules::Rule;

pub fn os_path(raw: &RawPath) -> Option<PathBuf> {
    match raw {
        #[cfg(windows)]
        RawPath::Windows(units) => {
            use std::os::windows::ffi::OsStringExt;
            Some(PathBuf::from(std::ffi::OsString::from_wide(units)))
        }
        #[cfg(unix)]
        RawPath::Unix(bytes) => {
            use std::os::unix::ffi::OsStrExt;
            Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
        }
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

/// Cache locations of the enabled catalog rules, from the platform resolvers' base paths.
#[cfg(windows)]
pub fn cache_locations_with(rules: &[Rule], env: &crate::windows::resolve::ResolverEnv) -> Vec<CacheLocation> {
    let roots: Vec<(String, String)> = rules
        .iter()
        .filter(|r| cm_core::cleanable::is_cache_rule(r, cm_core::capability::Platform::Windows))
        .flat_map(|r| crate::windows::resolve::base_paths(r, env).into_iter().map(move |p| (r.rule_id.clone(), p.display().to_string())))
        .collect();
    cm_core::cleanable::cache_locations(rules, cm_core::capability::Platform::Windows, &roots)
}

/// [`cache_locations_with`] for this machine's own locations. macOS has no rule resolvers in
/// this build, so no catalog location can be confirmed there.
pub fn cache_locations(rules: &[Rule]) -> Vec<CacheLocation> {
    #[cfg(windows)]
    {
        cache_locations_with(rules, &crate::windows::resolve::ResolverEnv::from_os())
    }
    #[cfg(not(windows))]
    {
        let _ = rules;
        Vec::new()
    }
}

/// Probes of the real filesystem and protection state; the hard-deny set and the user list are
/// loaded at most once, and only when some directory carries a signature.
pub struct LiveProbe {
    store_dir: Option<PathBuf>,
    hard: OnceCell<HardDenySet>,
    user: OnceCell<Option<UserProtection>>,
}

impl LiveProbe {
    /// `store_dir` holds the user protection list; without it nothing can be confirmed.
    pub fn new(store_dir: Option<&Path>) -> Self {
        LiveProbe { store_dir: store_dir.map(Path::to_path_buf), hard: OnceCell::new(), user: OnceCell::new() }
    }

    fn user(&self) -> Option<&UserProtection> {
        self.user.get_or_init(|| self.store_dir.as_deref().and_then(|d| crate::protection::load(d).ok())).as_ref()
    }

    fn hard(&self) -> &HardDenySet {
        self.hard.get_or_init(crate::protection::hard_deny_set)
    }
}

/// The directory's tag file is a plain local file whose head is the standard signature.
fn tag_ok(dir: &Path) -> bool {
    let tag = dir.join(cm_core::cleanable::CACHEDIR_TAG);
    #[cfg(windows)]
    {
        use cm_core::purge::PurgeProbe;
        crate::windows::purge::raw(&tag).is_some_and(|raw| crate::windows::purge::FsProbe::default().cachedir_tag(&raw))
    }
    #[cfg(not(windows))]
    {
        use std::io::Read;
        if !std::fs::symlink_metadata(&tag).is_ok_and(|m| m.file_type().is_file()) {
            return false;
        }
        let mut head = [0u8; 43];
        std::fs::File::open(&tag).and_then(|mut f| f.read_exact(&mut head)).is_ok() && cm_core::cleanable::is_cachedir_signature(&head)
    }
}

fn under(path: &str, dir: &str) -> bool {
    let (p, d) = if cfg!(windows) { (path.to_lowercase(), dir.to_lowercase()) } else { (path.to_owned(), dir.to_owned()) };
    let d = d.trim_end_matches(['\\', '/']);
    p.len() > d.len() && p.starts_with(d) && p[d.len()..].starts_with(['\\', '/'])
}

impl HintProbe for LiveProbe {
    fn cachedir_tag(&self, dir: &RawPath) -> bool {
        os_path(dir).is_some_and(|d| tag_ok(&d))
    }

    fn sibling_names(&self, dir: &RawPath) -> Option<Vec<String>> {
        let parent = os_path(dir)?.parent()?.to_path_buf();
        let rd = std::fs::read_dir(parent).ok()?;
        Some(rd.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().to_lowercase()).collect())
    }

    fn path_allowed(&self, dir: &RawPath) -> bool {
        let Some(path) = os_path(dir) else { return false };
        let Some(user) = self.user() else { return false };
        let hard = self.hard();
        let chain = crate::protection::identity_chain(dir);
        let subject = Subject { path: dir, chain: &chain, default_class: crate::protection::default_class(&path), rule_id: None, recursive: true };
        if !cm_core::protect::evaluate(&subject, hard, user).is_allowed() {
            return false;
        }
        // Removing the whole directory would also remove any hard-deny location inside it.
        let display = dir.display();
        !hard.roots().any(|r| under(&r.display, &display))
    }

    fn rule_allowed(&self, rule_id: &str) -> bool {
        self.user().is_some_and(|u| u.protects_rule(rule_id).is_none())
    }
}

/// Hints for one page: `dirs[i]` is the path of a real directory entry (`None` for files, links
/// and boundaries, which never get a hint).
pub fn page_hints(dirs: &[Option<&RawPath>], store_dir: Option<&Path>, caches: &[CacheLocation]) -> Vec<Option<CleanableHint>> {
    let probe = LiveProbe::new(store_dir);
    dirs.iter().map(|d| d.and_then(|d| hint_for(d, &probe, caches))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        // On macOS `$TMPDIR` goes through the `/var` symlink to the hard-deny root `/private/var` (a link on the way only
        // adds denials): use the real `/private/var/...` path there, as a person's own folders are reached.
        let t = std::env::temp_dir();
        let t = if cfg!(windows) { t } else { std::fs::canonicalize(&t).unwrap() };
        let d = t.join(format!("cm-hints-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn raw(p: &Path) -> RawPath {
        crate::paths::raw_path(p.as_os_str()).unwrap()
    }

    #[test]
    fn live_signatures_and_protection() {
        let root = scratch("live");
        let store = root.join("store");
        std::fs::create_dir_all(&store).unwrap();
        let good = root.join("good");
        let bad = root.join("bad");
        let proj = root.join("proj");
        for d in [&good, &bad, &proj.join("node_modules"), &root.join("plain")] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(good.join("CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55\n# a cache").unwrap();
        std::fs::write(bad.join("CACHEDIR.TAG"), "Signature: 0000000000000000000000000000000000").unwrap();
        std::fs::write(proj.join("package.json"), "{}").unwrap();
        let paths = [good.clone(), bad.clone(), proj.join("node_modules"), root.join("plain")].map(|p| raw(&p));
        let dirs: Vec<Option<&RawPath>> = paths.iter().map(Some).chain([None]).collect();
        let hints = page_hints(&dirs, Some(&store), &[]);
        assert_eq!(hints[0].as_ref().map(|h| h.basis), Some(cm_core::cleanable::HintBasis::CachedirTag));
        assert_eq!(hints[1], None, "an invalid tag is not evidence");
        assert_eq!(hints[2].as_ref().and_then(|h| h.rule_id.as_deref()), Some("purge.node-modules"));
        assert_eq!(hints[3], None, "no signature");
        assert_eq!(hints[4], None, "not a directory entry");

        // A user-protected location wins over its signature; an unusable list confirms nothing.
        let mut list = UserProtection::empty(if cfg!(windows) { cm_core::capability::Platform::Windows } else { cm_core::capability::Platform::Macos });
        let entry = cm_core::protect::UserEntry::path("u1", cm_core::protect::MatchKind::Descendants, &good.display().to_string(), "2026-10-16T00:00:00Z");
        list.add(entry).unwrap();
        crate::protection::save(&store, &list).unwrap();
        assert_eq!(page_hints(&[Some(&paths[0])], Some(&store), &[])[0], None);
        assert!(page_hints(&[Some(&paths[2])], Some(&store), &[])[0].is_some(), "only the protected subtree loses its hint");
        assert_eq!(page_hints(&[Some(&paths[2])], None, &[])[0], None, "no protection list, no hint");
        std::fs::remove_dir_all(&root).ok();
    }
}
