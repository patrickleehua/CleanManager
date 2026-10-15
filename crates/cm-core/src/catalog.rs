//! Cleanup catalog (CAT-01..CAT-09): shared categories, the one tier computation every entry uses,
//! tested-registry version verdicts and tri-state in-use probe results.
//!
//! Pure data and decisions. Probe results and installed versions are gathered by the platform
//! layer and passed in; nothing here touches the file system or runs a process. GUI, CLI and
//! agents all receive tiers from [`compute_tier`] through analysis, so no entry can be wider.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::capability::Platform;
use crate::rules::{RebuildClass, RegistryEntry, RootResolver, Rule, RuleStatus, Tier, VersionRange};

/// Version of the category table and tier computation; reported by `clean list`.
pub const CATALOG_VERSION: &str = "catalog/1";

/// One shared category (design D2). `wave` is the first catalog wave that targets it (D4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CategoryInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub wave: u8,
    pub windows: bool,
    pub macos: bool,
}

pub const CATEGORIES: &[CategoryInfo] = &[
    CategoryInfo { id: "system_data", label: "System data", wave: 1, windows: true, macos: true },
    CategoryInfo { id: "user_essentials", label: "User temporary files and logs", wave: 1, windows: true, macos: true },
    CategoryInfo { id: "trash", label: "Recycle Bin / Trash", wave: 1, windows: true, macos: true },
    CategoryInfo { id: "app_caches", label: "Application caches", wave: 2, windows: true, macos: true },
    CategoryInfo { id: "browsers", label: "Browsers", wave: 1, windows: true, macos: true },
    CategoryInfo { id: "cloud_office", label: "Cloud and office", wave: 2, windows: true, macos: true },
    CategoryInfo { id: "developer", label: "Developer tools", wave: 1, windows: true, macos: true },
    CategoryInfo { id: "ai_tools", label: "AI tools", wave: 1, windows: true, macos: true },
    CategoryInfo { id: "virtualization", label: "Virtualization", wave: 2, windows: true, macos: true },
    CategoryInfo { id: "logs", label: "Logs and crash reports", wave: 1, windows: true, macos: true },
    CategoryInfo { id: "leftovers", label: "Application leftovers", wave: 2, windows: true, macos: true },
    CategoryInfo { id: "firmware_backups", label: "Device firmware and backups", wave: 2, windows: true, macos: true },
    CategoryInfo { id: "incomplete_downloads", label: "Incomplete downloads", wave: 1, windows: true, macos: true },
    CategoryInfo { id: "large_items", label: "Large items (review hints only)", wave: 1, windows: true, macos: true },
    CategoryInfo { id: "removable_metadata", label: "External volume metadata", wave: 2, windows: true, macos: true },
];

pub fn category(id: &str) -> Option<&'static CategoryInfo> {
    CATEGORIES.iter().find(|c| c.id == id)
}

/// Category availability on one platform (CAT-01): a category with only experimental rules is
/// `unverified` (not selectable), one without rules for the platform is `unsupported`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CategoryStatus {
    Available,
    Unverified,
    Unsupported,
}

pub fn category_status(info: &CategoryInfo, platform: Platform, rules: &[Rule]) -> CategoryStatus {
    let defined = match platform {
        Platform::Windows => info.windows,
        Platform::Macos => info.macos,
    };
    let mine: Vec<&Rule> = rules.iter().filter(|r| r.category == info.id && r.platforms.contains(&platform)).collect();
    if !defined || mine.is_empty() {
        CategoryStatus::Unsupported
    } else if mine.iter().any(|r| r.status == RuleStatus::Enabled) {
        CategoryStatus::Available
    } else {
        CategoryStatus::Unverified
    }
}

/// Dependency and package stores are review-only whatever a rule declares (design D3): removing
/// them costs a network re-download and breaks offline work.
pub fn is_dependency_store(rule: &Rule) -> bool {
    rule.root_resolver == RootResolver::PackageManagerCache
        || (matches!(rule.category.as_str(), "developer" | "ai_tools") && rule.rebuild_class == RebuildClass::NetworkRedownload)
        || (rule.root_resolver == RootResolver::ProjectArtifact && rule.rebuild_class != RebuildClass::LocalRebuild)
}

// ---------------------------------------------------------------------------------------------
// In-use probes (CAT-06)

/// Probe kinds a rule may declare (rule-v2 `probes`). `elevated_token` answers whether this
/// process holds an administrator token; a rule with `requires_admin` must declare it, so its
/// objects are skipped (never attempted) when elevation was refused or not requested.
pub const PROBE_KINDS: &[&str] = &[
    "owner_process",
    "open_database_handle",
    "open_file_handle",
    "incomplete_download_handle",
    "endpoint_security_path",
    "mounted_image",
    "vcs_tracked",
    "elevated_token",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeState {
    NotInUse,
    /// The probe could not decide (enumeration failed, access denied, too much to check...).
    /// Never treated as not in use.
    Unknown,
    InUse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeResult {
    pub probe: String,
    pub state: ProbeState,
    pub detail: String,
}

impl ProbeResult {
    pub fn new(probe: &str, state: ProbeState, detail: impl Into<String>) -> Self {
        ProbeResult { probe: probe.into(), state, detail: detail.into() }
    }
}

/// Why an object is skipped although its tier allows it (in_use and unknown both skip).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    InUse,
    ProbeUnknown,
}

impl SkipReason {
    pub fn as_str(self) -> &'static str {
        match self {
            SkipReason::InUse => "in_use",
            SkipReason::ProbeUnknown => "probe_unknown",
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Tested registry versions (CAT-03, CAT-07)

/// Whether the installed owner version is covered by the rule's tested-registry entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistryStatus {
    /// The caller did not consult the registry (tests, previews of experimental rules).
    #[default]
    NotChecked,
    Covered,
    /// Installed version outside the tested range, or not determinable.
    VersionUntested,
    NotRegistered,
}

impl RegistryStatus {
    fn rank(self) -> u8 {
        match self {
            RegistryStatus::NotChecked => 0,
            RegistryStatus::Covered => 1,
            RegistryStatus::VersionUntested => 2,
            RegistryStatus::NotRegistered => 3,
        }
    }

    /// The less trustworthy of two verdicts (merging two rules on one object).
    pub fn worse(self, other: RegistryStatus) -> RegistryStatus {
        if other.rank() > self.rank() { other } else { self }
    }
}

/// Numeric dotted version (`155.0.8059.41`, `10.0.26200`). Pre-release or non-numeric text
/// returns `None`: an unparseable version is never inside a tested range.
pub fn parse_version(s: &str) -> Option<Vec<u64>> {
    let s = s.trim();
    let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
    if s.is_empty() || s.split('.').count() > 6 {
        return None;
    }
    s.split('.').map(|p| if p.is_empty() || p.len() > 12 || !p.bytes().all(|b| b.is_ascii_digit()) { None } else { p.parse().ok() }).collect()
}

/// Compare numeric versions; missing trailing parts count as zero (`1.2` == `1.2.0`).
pub fn compare_versions(a: &[u64], b: &[u64]) -> Ordering {
    let n = a.len().max(b.len());
    (0..n).map(|i| a.get(i).copied().unwrap_or(0).cmp(&b.get(i).copied().unwrap_or(0))).find(|o| o.is_ne()).unwrap_or(Ordering::Equal)
}

/// `Some(true)` inside `[min_inclusive, max_exclusive)`, `None` if any version is unparseable.
pub fn version_in_range(version: &str, range: &VersionRange) -> Option<bool> {
    let v = parse_version(version)?;
    let lo = parse_version(&range.min_inclusive)?;
    if compare_versions(&v, &lo) == Ordering::Less {
        return Some(false);
    }
    match &range.max_exclusive {
        None => Some(true),
        Some(hi) => Some(compare_versions(&v, &parse_version(hi)?) == Ordering::Less),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryVerdict {
    pub status: RegistryStatus,
    pub installed: Option<String>,
    pub reason: Option<String>,
}

/// Registry verdict for one rule given the installed owner version (`None` = not determinable).
pub fn registry_verdict(rule: &Rule, registry: &BTreeMap<String, RegistryEntry>, installed: Option<&str>) -> RegistryVerdict {
    let verdict = |status, reason: Option<String>| RegistryVerdict { status, installed: installed.map(str::to_owned), reason };
    let Some(entry) = rule.tested_registry_ref.as_ref().and_then(|r| registry.get(r)) else {
        return verdict(RegistryStatus::NotRegistered, Some("no tested-registry entry for this rule version".into()));
    };
    if entry.rule_id != rule.rule_id || entry.rule_version != rule.version || entry.result != "passed" || entry.synthetic {
        return verdict(RegistryStatus::NotRegistered, Some("registry entry does not cover this rule version".into()));
    }
    let range = &entry.version_range;
    let shown = format!("{}..{}", range.min_inclusive, range.max_exclusive.as_deref().unwrap_or(""));
    match installed.map(|v| (v, version_in_range(v, range))) {
        Some((_, Some(true))) => verdict(RegistryStatus::Covered, None),
        Some((v, Some(false))) => verdict(RegistryStatus::VersionUntested, Some(format!("installed {v} is outside the tested range {shown}"))),
        Some((v, None)) => verdict(RegistryStatus::VersionUntested, Some(format!("installed version {v:?} cannot be compared with {shown}"))),
        None => verdict(RegistryStatus::VersionUntested, Some(format!("installed version unknown; tested range {shown}"))),
    }
}

/// What the caller could supply about the tested registry and installed versions.
#[derive(Debug, Clone, Copy, Default)]
pub struct CatalogContext<'a> {
    /// `None`: registry not consulted (tests, single-rule previews); tiers are not lowered for it.
    pub registry: Option<&'a BTreeMap<String, RegistryEntry>>,
    /// Owner id (`google.chrome`, `windows`, `npm`...) → installed version. Absent = unknown.
    pub installed: Option<&'a BTreeMap<String, String>>,
}

impl CatalogContext<'_> {
    pub fn verdict(&self, rule: &Rule) -> RegistryVerdict {
        match self.registry {
            None => RegistryVerdict { status: RegistryStatus::NotChecked, installed: None, reason: None },
            Some(reg) => registry_verdict(rule, reg, self.installed.and_then(|m| m.get(&rule.owner.id)).map(String::as_str)),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Tier computation (CAT-02, design D3)

/// Everything the tier depends on; stored with each candidate so a later probe pass recomputes
/// the tier with the same function instead of patching it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierInputs {
    pub rule_max: Tier,
    pub dependency: bool,
    pub registry: RegistryStatus,
    /// Protection layers: `blocked` (hard deny, default class, user list), `review_only`
    /// (protected-reviewable), otherwise `default_selected`.
    pub protection: Tier,
    /// Evidence completeness: unread descendants, weak identity or shared hard links lower it.
    pub evidence: Tier,
    /// Live probe results; empty until probed.
    #[serde(default)]
    pub probes: Vec<ProbeResult>,
}

impl Default for TierInputs {
    fn default() -> Self {
        TierInputs {
            rule_max: Tier::Blocked,
            dependency: false,
            registry: RegistryStatus::NotChecked,
            protection: Tier::DefaultSelected,
            evidence: Tier::DefaultSelected,
            probes: vec![],
        }
    }
}

impl TierInputs {
    /// Two rules on one object: the stricter of everything.
    pub fn merge(&mut self, other: &TierInputs) {
        self.rule_max = self.rule_max.min(other.rule_max);
        self.dependency |= other.dependency;
        self.registry = self.registry.worse(other.registry);
        self.protection = self.protection.min(other.protection);
        self.evidence = self.evidence.min(other.evidence);
        self.probes.extend(other.probes.iter().cloned());
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TierDecision {
    pub tier: Tier,
    pub skip: Option<SkipReason>,
    /// Machine reasons for every input that lowered the tier or caused a skip.
    pub reasons: Vec<String>,
}

/// `tier = min(rule max, registry, probe, protection, evidence)`. Dependency stores are capped at
/// review_only; a version outside the tested range is at most review_only; any probe that is
/// in_use or unknown skips the object (the tier is kept for explanation, the object is not
/// selectable).
pub fn compute_tier(i: &TierInputs) -> TierDecision {
    let mut reasons = Vec::new();
    let mut tier = i.rule_max;
    if i.dependency && tier < Tier::ReviewOnly {
        tier = Tier::ReviewOnly;
        reasons.push("dependency_store_review_only".to_string());
    }
    match i.registry {
        RegistryStatus::NotChecked | RegistryStatus::Covered => {}
        RegistryStatus::VersionUntested => {
            tier = tier.min(Tier::ReviewOnly);
            reasons.push("version_untested".into());
        }
        RegistryStatus::NotRegistered => {
            tier = tier.min(Tier::ReviewOnly);
            reasons.push("registry_missing".into());
        }
    }
    tier = tier.min(i.protection).min(i.evidence);
    let worst = i.probes.iter().map(|p| p.state).max();
    let skip = match worst {
        Some(ProbeState::InUse) => Some(SkipReason::InUse),
        Some(ProbeState::Unknown) => Some(SkipReason::ProbeUnknown),
        _ => None,
    };
    for p in i.probes.iter().filter(|p| p.state != ProbeState::NotInUse) {
        let state = if p.state == ProbeState::InUse { "in_use" } else { "unknown" };
        reasons.push(format!("probe:{}:{state}", p.probe));
    }
    TierDecision { tier, skip, reasons }
}

#[cfg(test)]
mod tests;
