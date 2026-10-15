//! Declarative cleanup rules (ANA-01; rule 2.0 with rule 1.0 accepted on load).
//!
//! Rules are data: a fixed resolver kind, literal path components, single-segment globs and
//! thresholds. There is no script, regex or command field, and links are never followed.
//! Lifecycle: experimental → verified → enabled. Only enabled rules produce actionable
//! candidates; enabled requires a passed, non-synthetic tested-registry entry for every platform.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::c14n::digest;
use crate::capability::Platform;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleStatus {
    Experimental,
    Verified,
    Enabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    DefaultSelected,
    ReviewOnly,
    Blocked,
}

impl Tier {
    /// The stricter of two tiers.
    pub fn min(self, other: Tier) -> Tier {
        std::cmp::max(self, other)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RebuildClass {
    LocalRebuild,
    NetworkRedownload,
    NotRebuildable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootResolver {
    ApplicationCache,
    AppContainer,
    AppSupport,
    BrowserProfile,
    CrashReports,
    InstallerLocation,
    PackageManagerCache,
    ProjectArtifact,
    RemovableVolumeRoot,
    SystemCache,
    SystemLogs,
    SystemTemp,
    Trash,
    UserCache,
    UserLogs,
    UserTemp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerKind {
    App,
    Tool,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    pub kind: OwnerKind,
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectType {
    File,
    Directory,
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Match {
    pub object_type: ObjectType,
    pub relative_components: Vec<String>,
    pub name_glob: Option<String>,
    pub extensions: Vec<String>,
    pub require_cachedir_tag: bool,
    pub archive_payload: bool,
    pub max_depth: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Thresholds {
    pub min_age_days: Option<u32>,
    pub age_basis: Option<String>,
    pub min_size_bytes: Option<String>,
    pub retention_days: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Filesystem,
    ToolAdapter,
    ReviewOnlyRow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Trash,
    Quarantine,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Impact {
    pub summary: String,
    pub regeneration: String,
    pub offline_effect: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub schema_version: String,
    pub rule_id: String,
    pub version: String,
    pub status: RuleStatus,
    pub platforms: Vec<Platform>,
    pub category: String,
    pub max_tier: Tier,
    pub rebuild_class: RebuildClass,
    pub owner: Owner,
    pub root_resolver: RootResolver,
    #[serde(rename = "match")]
    pub matcher: Match,
    pub exclude: Vec<String>,
    pub probes: Vec<String>,
    pub thresholds: Thresholds,
    pub action_kind: ActionKind,
    pub adapter_ref: Option<String>,
    pub allowed_modes: Vec<Mode>,
    pub requires_admin: bool,
    pub protect_descendants: bool,
    pub follow_links: bool,
    pub impact: Impact,
    pub tested_registry_ref: Option<String>,
    pub evidence_refs: Vec<String>,
}

/// Rule 1.0 as published by the MVP contract draft; converted to the 2.0 model on load.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleV1 {
    #[serde(rename = "schema_version")]
    _schema_version: String,
    rule_id: String,
    version: String,
    status: RuleStatus,
    platforms: Vec<Platform>,
    root_resolver: String,
    owner_app_id: String,
    relative_components: Vec<String>,
    min_age_days: u32,
    risk: String,
    allowed_modes: Vec<Mode>,
    requires_owner_inactive: bool,
    protect_descendants: bool,
    follow_links: bool,
    impact: String,
    evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryPlatform {
    pub platform: Platform,
    pub os_versions: Vec<String>,
    pub architectures: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrySubject {
    pub kind: OwnerKind,
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionRange {
    pub min_inclusive: String,
    pub max_exclusive: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fixtures {
    pub positive: Vec<String>,
    pub negative: Vec<String>,
}

/// One tested-application registry entry (CAT-03).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryEntry {
    pub schema_version: String,
    pub registry_id: String,
    pub rule_id: String,
    pub rule_version: String,
    pub synthetic: bool,
    pub subject: RegistrySubject,
    pub version_range: VersionRange,
    pub platforms: Vec<RegistryPlatform>,
    pub tested_at: String,
    pub result: String,
    pub fixtures: Fixtures,
    pub known_effects: Vec<String>,
    pub evidence_refs: Vec<String>,
    /// Thresholds the run exercised (CAT-07). A rule with thresholds is only covered while they
    /// are unchanged: changing a threshold needs a new rule version and a new test.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thresholds: Option<Thresholds>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuleError {
    #[error("rule {0}: {1}")]
    Invalid(String, String),
}

fn invalid(rule: &str, msg: impl Into<String>) -> RuleError {
    RuleError::Invalid(rule.into(), msg.into())
}

fn check_component(rule: &str, c: &str) -> Result<(), RuleError> {
    if c.is_empty() || c == "." || c == ".." || c.contains(['/', '\\', ':', '\0', '*', '?', '[', ']']) {
        return Err(invalid(rule, format!("path component {c:?} could escape its resolved root")));
    }
    Ok(())
}

fn check_glob(rule: &str, g: &str) -> Result<(), RuleError> {
    if g.is_empty() || g.contains("**") || g.contains("..") || g.contains(['/', '\\', ':', '\0']) {
        return Err(invalid(rule, format!("glob {g:?} must be a single path segment")));
    }
    Ok(())
}

impl Rule {
    /// Parse rule 2.0 or 1.0 JSON. Unknown fields, script resolvers and link following fail.
    pub fn parse(text: &str) -> Result<Rule, RuleError> {
        let value: serde_json::Value = serde_json::from_str(text).map_err(|e| invalid("?", e.to_string()))?;
        let id = value["rule_id"].as_str().unwrap_or("?").to_owned();
        let rule = match value["schema_version"].as_str() {
            Some("2.0") => serde_json::from_value::<Rule>(value).map_err(|e| invalid(&id, e.to_string()))?,
            Some("1.0") => Self::from_v1(serde_json::from_value::<RuleV1>(value).map_err(|e| invalid(&id, e.to_string()))?)?,
            other => return Err(invalid(&id, format!("unsupported schema_version {other:?}"))),
        };
        rule.check_shape()?;
        Ok(rule)
    }

    fn from_v1(v: RuleV1) -> Result<Rule, RuleError> {
        let resolver = match v.root_resolver.as_str() {
            "application_cache" => RootResolver::ApplicationCache,
            "user_temp" => RootResolver::UserTemp,
            "project_build_output" => RootResolver::ProjectArtifact,
            other => return Err(invalid(&v.rule_id, format!("unknown root resolver {other}"))),
        };
        Ok(Rule {
            schema_version: "2.0".into(),
            rule_id: v.rule_id,
            version: v.version,
            status: v.status,
            platforms: v.platforms,
            category: if resolver == RootResolver::UserTemp { "user_essentials".into() } else { "app_caches".into() },
            max_tier: if v.risk == "low" { Tier::DefaultSelected } else { Tier::ReviewOnly },
            rebuild_class: RebuildClass::LocalRebuild,
            owner: Owner { kind: OwnerKind::App, id: v.owner_app_id },
            root_resolver: resolver,
            matcher: Match {
                object_type: ObjectType::Any,
                relative_components: v.relative_components,
                name_glob: None,
                extensions: vec![],
                require_cachedir_tag: false,
                archive_payload: false,
                max_depth: 0,
            },
            exclude: vec![],
            probes: if v.requires_owner_inactive { vec!["owner_process".into()] } else { vec![] },
            thresholds: Thresholds { min_age_days: Some(v.min_age_days), age_basis: Some("modified_time".into()), min_size_bytes: None, retention_days: None },
            action_kind: ActionKind::Filesystem,
            adapter_ref: None,
            allowed_modes: v.allowed_modes,
            requires_admin: false,
            protect_descendants: v.protect_descendants,
            follow_links: v.follow_links,
            impact: Impact { summary: v.impact.clone(), regeneration: v.impact.clone(), offline_effect: v.impact },
            tested_registry_ref: None,
            evidence_refs: v.evidence_refs,
        })
    }

    fn check_shape(&self) -> Result<(), RuleError> {
        let id = &self.rule_id;
        if self.follow_links {
            return Err(invalid(id, "follow_links must be false"));
        }
        if !self.protect_descendants {
            return Err(invalid(id, "protect_descendants must be true"));
        }
        for c in &self.matcher.relative_components {
            check_component(id, c)?;
        }
        for g in self.matcher.name_glob.iter().chain(&self.exclude) {
            check_glob(id, g)?;
        }
        if self.matcher.max_depth > 8 {
            return Err(invalid(id, "max_depth must be at most 8"));
        }
        if self.rebuild_class != RebuildClass::LocalRebuild && self.max_tier == Tier::DefaultSelected {
            return Err(invalid(id, "only locally rebuildable data can be default_selected"));
        }
        if self.max_tier == Tier::Blocked && !self.allowed_modes.is_empty() {
            return Err(invalid(id, "blocked rules have no modes"));
        }
        // Administrator-only locations are skipped without an elevated token (CAT-06 tri-state):
        // the rule must declare the probe that answers it.
        if self.requires_admin && !self.probes.iter().any(|p| p == "elevated_token") {
            return Err(invalid(id, "requires_admin rules must declare the elevated_token probe"));
        }
        if matches!(self.status, RuleStatus::Verified | RuleStatus::Enabled) && self.evidence_refs.is_empty() {
            return Err(invalid(id, "verified/enabled rules need evidence_refs"));
        }
        Ok(())
    }

    /// Promotion check against the tested registry (CAT-03, ANA-01).
    pub fn check_registry(&self, registry: &BTreeMap<String, RegistryEntry>) -> Result<(), RuleError> {
        if self.status != RuleStatus::Enabled {
            return Ok(());
        }
        let id = &self.rule_id;
        let reference = self.tested_registry_ref.as_ref().ok_or_else(|| invalid(id, "enabled rule without tested_registry_ref"))?;
        let entry = registry.get(reference).ok_or_else(|| invalid(id, format!("registry entry {reference} not found")))?;
        if entry.rule_id != self.rule_id || entry.rule_version != self.version {
            return Err(invalid(id, "registry entry is for another rule version"));
        }
        if entry.result != "passed" || entry.synthetic {
            return Err(invalid(id, "enabled rule needs a passed, non-synthetic registry entry"));
        }
        for p in &self.platforms {
            if !entry.platforms.iter().any(|t| t.platform == *p) {
                return Err(invalid(id, format!("platform {p:?} has no tested registry evidence")));
            }
        }
        let has_thresholds = self.thresholds != Thresholds { min_age_days: None, age_basis: None, min_size_bytes: None, retention_days: None };
        match &entry.thresholds {
            Some(t) if *t != self.thresholds => return Err(invalid(id, "thresholds differ from the tested ones; a new rule version must be tested")),
            None if has_thresholds => return Err(invalid(id, "registry entry does not record the tested thresholds")),
            _ => {}
        }
        Ok(())
    }
}

/// A loaded, validated rule set and its digest (bound into plans as rule_set_digest).
#[derive(Debug, Clone)]
pub struct RuleSet {
    pub rules: Vec<Rule>,
    pub digest: String,
}

impl RuleSet {
    pub fn new(mut rules: Vec<Rule>, registry: &BTreeMap<String, RegistryEntry>) -> Result<RuleSet, RuleError> {
        rules.sort_by(|a, b| (a.rule_id.as_str(), a.version.as_str()).cmp(&(b.rule_id.as_str(), b.version.as_str())));
        for w in rules.windows(2) {
            if w[0].rule_id == w[1].rule_id {
                return Err(invalid(&w[0].rule_id, "rule id appears twice"));
            }
        }
        for r in &rules {
            r.check_registry(registry)?;
        }
        let value = serde_json::to_value(&rules).expect("rules serialize");
        let digest = digest(&value).map_err(|e| invalid("rule_set", e.to_string()))?;
        Ok(RuleSet { rules, digest })
    }

    /// Rules that may create actionable candidates on this platform.
    pub fn enabled_for(&self, platform: Platform) -> impl Iterator<Item = &Rule> {
        self.rules.iter().filter(move |r| r.status == RuleStatus::Enabled && r.platforms.contains(&platform))
    }
}

/// Case-insensitive (ASCII) or exact single-segment glob with `*` and `?`.
pub fn glob_match(pattern: &str, name: &str, case_insensitive: bool) -> bool {
    let fold = |c: char| if case_insensitive { c.to_ascii_lowercase() } else { c };
    let p: Vec<char> = pattern.chars().map(fold).collect();
    let n: Vec<char> = name.chars().map(fold).collect();
    let (mut pi, mut ni, mut star, mut mark) = (0usize, 0usize, None::<usize>, 0usize);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            pi += 1;
            mark = ni;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests;
