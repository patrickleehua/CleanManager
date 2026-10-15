//! Restricted root resolvers (ANA-01): each resolver kind maps to a fixed, documented location
//! derived from the OS environment and a built-in owner table. There is no way for a rule to
//! supply its own path or script. Resolved roots carry file identity so analysis matches them by
//! identity, not by path text (8.3 names and case differences do not matter).

use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use cm_core::analysis::ResolvedRoot;
use cm_core::rules::{Rule, RootResolver};

use super::directory::identity_of;

/// Base locations; defaults come from the environment, overridable for controlled fixtures.
#[derive(Debug, Clone)]
pub struct ResolverEnv {
    pub local_app_data: Option<PathBuf>,
    pub temp: Option<PathBuf>,
}

impl ResolverEnv {
    pub fn from_os() -> Self {
        ResolverEnv { local_app_data: std::env::var_os("LOCALAPPDATA").map(PathBuf::from), temp: std::env::var_os("TEMP").map(PathBuf::from) }
    }
}

/// Built-in owner table: browser owner id → user-data directory under LOCALAPPDATA.
const BROWSERS: &[(&str, &[&str])] = &[
    ("microsoft.edge", &["Microsoft", "Edge", "User Data"]),
    ("google.chrome", &["Google", "Chrome", "User Data"]),
];

fn profiles(user_data: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(user_data) else { return vec![] };
    let mut out: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink()))
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n == "Default" || n.starts_with("Profile ")
        })
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// Candidate base paths for a rule (before identity resolution).
pub fn base_paths(rule: &Rule, env: &ResolverEnv) -> Vec<PathBuf> {
    match rule.root_resolver {
        RootResolver::UserTemp => env.temp.iter().cloned().collect(),
        RootResolver::BrowserProfile => {
            let Some(lad) = &env.local_app_data else { return vec![] };
            BROWSERS
                .iter()
                .filter(|(id, _)| *id == rule.owner.id)
                .flat_map(|(_, parts)| profiles(&parts.iter().fold(lad.clone(), |p, c| p.join(c))))
                .collect()
        }
        // Other resolver kinds have no Windows mapping yet: the rule stays inapplicable.
        _ => vec![],
    }
}

/// Resolve every enabled rule's roots to identities. Missing locations are simply absent.
pub fn resolve(rules: &[Rule], env: &ResolverEnv) -> Vec<ResolvedRoot> {
    let mut out = Vec::new();
    for rule in rules {
        for path in base_paths(rule, env) {
            let wide: Vec<u16> = OsString::from(path.as_os_str()).encode_wide().collect();
            if let (Ok((volume_id, file_id)), Ok(raw)) = (identity_of(&wide), cm_core::path::RawPath::windows(wide.clone())) {
                out.push(ResolvedRoot { rule_id: rule.rule_id.clone(), path: raw, volume_id, file_id });
            }
        }
    }
    out
}
