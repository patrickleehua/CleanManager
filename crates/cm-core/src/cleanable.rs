//! Cleanable-directory hints for the disk view (DIN-05, design D16 N9).
//!
//! A hint is read-only advice that a listed directory can be cleaned by hand, named with the
//! evidence that confirmed it and the review flow it leads to. It never selects anything, never
//! changes a tier and never overrides protection: a protected directory gets no hint, and a
//! directory whose evidence cannot be confirmed gets none either. Bases, strongest first:
//!
//! - `build_artifact`: the purge recognizer's own test ([`purge::project_markers`] and
//!   [`purge::owned_artifact`]): an artifact name directly inside a project root that holds one of
//!   its owner markers, outside artifact and dot directories (which the purge search never enters);
//! - `catalog_rule`: inside a location an enabled cache rule of the catalog removes as a whole;
//! - `cachedir_tag`: a `CACHEDIR.TAG` with the standard signature directly in the directory.
//!
//! The evidence is read live by a [`HintProbe`] for the listed directories only.

use serde::Serialize;

use crate::capability::Platform;
use crate::path::RawPath;
use crate::purge;
use crate::rules::{ActionKind, ObjectType, Rule, RuleStatus, glob_match};

/// Name of the cache tag file (Cache Directory Tagging Specification).
pub const CACHEDIR_TAG: &str = "CACHEDIR.TAG";
/// The header a valid `CACHEDIR.TAG` starts with.
pub const CACHEDIR_SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";

/// Whether the first bytes of a file are a valid cache-tag header.
pub fn is_cachedir_signature(head: &[u8]) -> bool {
    head.starts_with(CACHEDIR_SIGNATURE)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HintBasis {
    BuildArtifact,
    CatalogRule,
    CachedirTag,
}

/// The review flow a hint offers; nothing is preselected there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HintFlow {
    /// Project build-artifact cleanup (PUR).
    Purge,
    /// Manual selection moved to the platform trash after review (MTR).
    ManualTrash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CleanableHint {
    pub basis: HintBasis,
    /// `purge.<artifact>` or the catalog rule; `None` for a cache tag.
    pub rule_id: Option<String>,
    /// The verified evidence: the owner markers found, the rule's location, or the tag file.
    pub detail: String,
    pub flow: HintFlow,
}

/// A location an enabled catalog cache rule removes as a whole: the resolved root joined with
/// the rule's literal components, then (for a selective rule) a child whose name matches the
/// rule's glob within its depth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheLocation {
    pub rule_id: String,
    pub platform: Platform,
    pub base: String,
    pub name_glob: Option<String>,
    pub exclude: Vec<String>,
    pub max_depth: u8,
}

/// Rules whose targets are whole cache directories: enabled for the platform, removed by a file
/// action, directory targets, no age threshold (every matching directory qualifies), no extra
/// evidence requirement and no administrator token.
pub fn is_cache_rule(rule: &Rule, platform: Platform) -> bool {
    rule.status == RuleStatus::Enabled
        && rule.platforms.contains(&platform)
        && rule.action_kind == ActionKind::Filesystem
        && rule.matcher.object_type == ObjectType::Directory
        && rule.matcher.extensions.is_empty()
        && !rule.matcher.require_cachedir_tag
        && !rule.matcher.archive_payload
        && rule.thresholds.min_age_days.is_none()
        && rule.thresholds.min_size_bytes.is_none()
        && !rule.requires_admin
}

fn separator(platform: Platform) -> char {
    if platform == Platform::Windows { '\\' } else { '/' }
}

fn components(path: &str, platform: Platform) -> Vec<&str> {
    match platform {
        Platform::Windows => path.split(['\\', '/']).filter(|c| !c.is_empty()).collect(),
        _ => path.split('/').filter(|c| !c.is_empty()).collect(),
    }
}

fn same(a: &str, b: &str, ci: bool) -> bool {
    if ci { a.to_lowercase() == b.to_lowercase() } else { a == b }
}

/// Cache locations of `rules` from the platform's resolved root paths `(rule_id, root)`.
pub fn cache_locations(rules: &[Rule], platform: Platform, roots: &[(String, String)]) -> Vec<CacheLocation> {
    let sep = separator(platform);
    let mut out = Vec::new();
    for rule in rules.iter().filter(|r| is_cache_rule(r, platform)) {
        for (_, root) in roots.iter().filter(|(id, _)| *id == rule.rule_id) {
            let mut base = root.trim_end_matches(['\\', '/']).to_owned();
            for c in &rule.matcher.relative_components {
                base.push(sep);
                base.push_str(c);
            }
            out.push(CacheLocation {
                rule_id: rule.rule_id.clone(),
                platform,
                base,
                name_glob: rule.matcher.name_glob.clone(),
                exclude: rule.exclude.clone(),
                max_depth: rule.matcher.max_depth,
            });
        }
    }
    out
}

impl CacheLocation {
    /// Whether the directory at `path` (display form) is this location's target or inside one.
    pub fn covers(&self, path: &str) -> bool {
        let ci = self.platform == Platform::Windows;
        let base = components(&self.base, self.platform);
        let dir = components(path, self.platform);
        if base.is_empty() || dir.len() < base.len() || !base.iter().zip(&dir).all(|(a, b)| same(a, b, ci)) {
            return false;
        }
        let rel = &dir[base.len()..];
        match &self.name_glob {
            // The literal location itself is the target.
            None => true,
            // The first matching (non-excluded) name within the rule's depth is the target.
            Some(g) => rel.iter().take(usize::from(self.max_depth) + 1).any(|n| glob_match(g, n, ci) && !self.exclude.iter().any(|x| glob_match(x, n, ci))),
        }
    }
}

/// Live, read-only evidence for one listed directory.
pub trait HintProbe {
    /// The directory directly holds a `CACHEDIR.TAG` regular file with the standard signature.
    fn cachedir_tag(&self, dir: &RawPath) -> bool;
    /// Lower-case names in the directory's parent; `None` when it cannot be listed.
    fn sibling_names(&self, dir: &RawPath) -> Option<Vec<String>>;
    /// Every path protection layer (hard deny, default classes, user list) allows offering this
    /// directory as a whole. Anything unknown answers `false`.
    fn path_allowed(&self, dir: &RawPath) -> bool;
    /// The user did not protect this rule; `false` when the list is unavailable.
    fn rule_allowed(&self, rule_id: &str) -> bool;
}

fn build_artifact(dir: &RawPath, probe: &dyn HintProbe) -> Option<CleanableHint> {
    let display = dir.display();
    let parts = components(&display, dir.platform());
    let (name, ancestors) = parts.split_last()?;
    let name = name.to_lowercase();
    if !purge::is_artifact_name(&name) {
        return None;
    }
    // The purge search never enters artifact or dot directories: nested dependencies and tool
    // data (`node_modules\pkg\node_modules`, `.vscode\extensions\x\node_modules`) are not projects.
    if ancestors.iter().skip(1).any(|a| a.starts_with('.') || purge::is_artifact_name(&a.to_lowercase())) {
        return None;
    }
    let markers = purge::project_markers(&probe.sibling_names(dir)?);
    let artifact = purge::owned_artifact(&name, &markers)?;
    let owners: Vec<&str> = markers.iter().map(String::as_str).filter(|m| artifact.owners.contains(m)).collect();
    Some(CleanableHint { basis: HintBasis::BuildArtifact, rule_id: Some(purge::rule_id(artifact)), detail: owners.join(", "), flow: HintFlow::Purge })
}

/// The hint for one listed directory (the caller passes only real directories, never links or
/// mount points), or `None` when no basis is confirmed or protection does not allow it.
pub fn hint_for(dir: &RawPath, probe: &dyn HintProbe, caches: &[CacheLocation]) -> Option<CleanableHint> {
    let display = dir.display();
    let mut found: Vec<CleanableHint> = Vec::new();
    found.extend(build_artifact(dir, probe));
    found.extend(caches.iter().filter(|c| c.covers(&display)).map(|c| CleanableHint {
        basis: HintBasis::CatalogRule,
        rule_id: Some(c.rule_id.clone()),
        detail: c.base.clone(),
        flow: HintFlow::ManualTrash,
    }));
    if probe.cachedir_tag(dir) {
        found.push(CleanableHint { basis: HintBasis::CachedirTag, rule_id: None, detail: CACHEDIR_TAG.into(), flow: HintFlow::ManualTrash });
    }
    // Protection wins: checked once for the path, then per rule.
    if found.is_empty() || !probe.path_allowed(dir) {
        return None;
    }
    found.into_iter().find(|h| h.rule_id.as_deref().is_none_or(|r| probe.rule_allowed(r)))
}

#[cfg(test)]
mod tests;
