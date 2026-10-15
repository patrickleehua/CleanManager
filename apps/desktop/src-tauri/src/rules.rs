//! The platform rule pack, loaded the same way the CLI loads it (`analysis_cmd::load_rules`):
//! the platform directory plus the tested registry, and any invalid rule makes the whole set
//! unavailable — it is never skipped silently. Analysis and plan application both bind to the
//! set's digest, so a rule upgrade makes older plans stale instead of widening them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cm_core::rules::{RegistryEntry, Rule, RuleSet};

/// Where the rule pack is read from: `CM_RULES_DIR`, then `rules` next to the executable (release
/// bundles ship it there as a resource), then — debug builds only — the repository's `rules`
/// directory this build was compiled from. Release builds never embed or read a build-machine path.
pub fn locate() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CM_RULES_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        let beside = exe_dir.join("rules");
        if beside.is_dir() {
            return Some(beside);
        }
    }
    #[cfg(debug_assertions)]
    {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../rules");
        if repo.is_dir() {
            return Some(repo);
        }
    }
    None
}

fn read_json_files(dir: &Path) -> Result<Vec<(PathBuf, String)>, String> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return Ok(out) };
    for e in rd.filter_map(Result::ok) {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("json") {
            out.push((p.clone(), std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?));
        }
    }
    out.sort();
    Ok(out)
}

/// Load the platform rules and the tested registry from `dir`.
pub fn load(dir: Option<&Path>) -> Result<RuleSet, String> {
    load_with_registry(dir).map(|(r, _)| r)
}

/// [`load`] keeping the tested registry for the catalog's version verdicts (CAT-03), as the
/// CLI's `load_rules_and_registry` does.
pub fn load_with_registry(dir: Option<&Path>) -> Result<(RuleSet, BTreeMap<String, RegistryEntry>), String> {
    let dir = dir.ok_or_else(|| "no rule pack found (set CM_RULES_DIR or place `rules` next to the program)".to_string())?;
    let platform_dir = if cfg!(windows) { "windows" } else { "macos" };
    let mut registry = BTreeMap::new();
    for (p, text) in read_json_files(&dir.join("registry"))? {
        let e: RegistryEntry = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))?;
        registry.insert(e.registry_id.clone(), e);
    }
    let rules = read_json_files(&dir.join(platform_dir))?
        .into_iter()
        .map(|(p, text)| Rule::parse(&text).map_err(|e| format!("{}: {e}", p.display())))
        .collect::<Result<Vec<_>, _>>()?;
    let set = RuleSet::new(rules, &registry).map_err(|e| e.to_string())?;
    Ok((set, registry))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_pack_or_a_broken_rule_fails_the_whole_set() {
        // The repository pack is validated by cm-core's own tests; here it must either load
        // with a digest or report why, never come back half-loaded.
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../rules");
        match load(Some(&repo)) {
            Ok(set) => assert!(!set.digest.is_empty()),
            Err(e) => assert!(!e.is_empty()),
        }
        assert!(load(None).is_err());

        let dir = std::env::temp_dir().join(format!("cm-desktop-rules-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let platform_dir = dir.join(if cfg!(windows) { "windows" } else { "macos" });
        std::fs::create_dir_all(&platform_dir).unwrap();
        std::fs::write(platform_dir.join("bad.json"), b"{\"rule_id\": 1}").unwrap();
        assert!(load(Some(&dir)).is_err(), "an invalid rule is never skipped silently");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
