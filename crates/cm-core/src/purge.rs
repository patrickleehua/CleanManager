//! Project build-artifact purge (PUR-01..05): project roots from explicit marker files, artifact
//! classes by rebuildability, version-control and secret-file blocking, recent projects never
//! preselected. Produces an ordinary [`CandidateSet`] over a published scan, so plans, review,
//! authorization, execution and recovery are the shared ones (directories become `directory`
//! plan items through the `purge.` rule prefix, see [`crate::plan::removes_directories`]).
//!
//! Names alone never make a candidate: an artifact directory counts only as the direct child of
//! a project root holding one of the artifact's owner markers, and artifact-named directories are
//! never searched for projects. Reading file contents (CACHEDIR.TAG) and version-control state
//! is delegated to a [`PurgeProbe`] supplied by the platform.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::analysis::{AnalysisProtection, Candidate, CandidateSet, CandidateState, IndexView, PROTECTION_VERSION, ProtectionClass, RuleRef, UserList, protection_by_name};
use crate::bytes::{Bytes, KnownTotal};
use crate::c14n::digest;
use crate::model::{CloudState, IdentityQuality, ObjectKind};
use crate::path::RawPath;
use crate::protect::{HARD_DENY_VERSION, ObjectIdentity, hard_deny_within, prefilter};
use crate::catalog::{RegistryStatus, TierInputs, compute_tier};
use crate::rules::{Mode, RebuildClass, Tier};

/// Version of the built-in artifact catalog; bound into rule refs and the rule-set digest.
pub const CATALOG_VERSION: &str = "1.0.0";
/// Projects modified within this many days are not preselected (PUR-04).
pub const DEFAULT_RECENT_DAYS: u32 = 7;
/// Project-marker search depth below each scan root (PUR-01).
pub const DEFAULT_MAX_DEPTH: u32 = 5;
pub const MAX_DEPTH_LIMIT: u32 = 8;
pub const SETTINGS_VERSION: u32 = 1;
/// Settings file name, stored next to the store (shared by GUI and CLI).
pub const SETTINGS_FILE: &str = "purge-settings.json";
/// Rule-id prefix of every purge candidate.
pub const RULE_PREFIX: &str = "purge.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactClass {
    /// Compiler/bundler/test outputs rebuilt locally by the next build.
    Output,
    /// Installed dependencies or environments: reinstalling needs the network (always review-only).
    Dependency,
}

/// One artifact directory name and what makes it one.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Artifact {
    pub name: &'static str,
    pub class: ArtifactClass,
    /// Markers in the project root that own this artifact (`*.ext` matches an extension).
    pub owners: &'static [&'static str],
    /// Best tier when an owner marker is present (outputs may be raised by CACHEDIR.TAG).
    pub tier: Tier,
    /// What it costs to get back.
    pub restore: &'static str,
}

const NODE: &[&str] = &["package.json"];
const PY: &[&str] = &["pyproject.toml", "setup.py", "setup.cfg", "requirements.txt", "pipfile", "tox.ini", "noxfile.py"];
const DOTNET: &[&str] = &["*.csproj", "*.fsproj", "*.vbproj"];
const GRADLE: &[&str] = &["build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts"];
const ZIG: &[&str] = &["build.zig", "build.zig.zon"];

/// Built-in artifact catalog (reference: Mole `purge_shared.sh` target list).
pub const ARTIFACTS: &[Artifact] = &[
    Artifact { name: "node_modules", class: ArtifactClass::Dependency, owners: NODE, tier: Tier::ReviewOnly, restore: "reinstall with the project's package manager (npm/pnpm/yarn install); needs the network" },
    Artifact { name: ".venv", class: ArtifactClass::Dependency, owners: PY, tier: Tier::ReviewOnly, restore: "recreate the virtual environment and reinstall packages; needs the network" },
    Artifact { name: "venv", class: ArtifactClass::Dependency, owners: PY, tier: Tier::ReviewOnly, restore: "recreate the virtual environment and reinstall packages; needs the network" },
    Artifact { name: ".tox", class: ArtifactClass::Dependency, owners: PY, tier: Tier::ReviewOnly, restore: "tox recreates its environments; needs the network" },
    Artifact { name: ".nox", class: ArtifactClass::Dependency, owners: PY, tier: Tier::ReviewOnly, restore: "nox recreates its environments; needs the network" },
    Artifact { name: "vendor", class: ArtifactClass::Dependency, owners: &["composer.json", "gemfile", "go.mod"], tier: Tier::ReviewOnly, restore: "reinstall vendored packages (composer install / bundle install / go mod vendor); needs the network" },
    Artifact { name: "pods", class: ArtifactClass::Dependency, owners: &["podfile"], tier: Tier::ReviewOnly, restore: "pod install; needs the network" },
    Artifact { name: ".terragrunt-cache", class: ArtifactClass::Dependency, owners: &["terragrunt.hcl"], tier: Tier::ReviewOnly, restore: "terragrunt downloads modules and providers again; needs the network" },
    Artifact { name: "target", class: ArtifactClass::Output, owners: &["cargo.toml", "pom.xml"], tier: Tier::DefaultSelected, restore: "rebuilt by the next cargo/maven build (full rebuild time)" },
    Artifact { name: "obj", class: ArtifactClass::Output, owners: DOTNET, tier: Tier::DefaultSelected, restore: "rebuilt by the next dotnet/msbuild build" },
    Artifact { name: "bin", class: ArtifactClass::Output, owners: DOTNET, tier: Tier::DefaultSelected, restore: "rebuilt by the next dotnet/msbuild build" },
    Artifact { name: "build", class: ArtifactClass::Output, owners: GRADLE, tier: Tier::DefaultSelected, restore: "rebuilt by the next Gradle build" },
    Artifact { name: "build", class: ArtifactClass::Output, owners: &["package.json", "pyproject.toml", "setup.py", "cmakelists.txt"], tier: Tier::ReviewOnly, restore: "rebuilt by the project's build script; check it holds no hand-written files" },
    Artifact { name: "dist", class: ArtifactClass::Output, owners: &["package.json", "pyproject.toml", "setup.py"], tier: Tier::ReviewOnly, restore: "rebuilt by the project's build script; check it holds no hand-written files" },
    Artifact { name: "out", class: ArtifactClass::Output, owners: NODE, tier: Tier::ReviewOnly, restore: "rebuilt by the project's build/export script" },
    Artifact { name: "coverage", class: ArtifactClass::Output, owners: NODE, tier: Tier::ReviewOnly, restore: "regenerated by the next coverage run" },
    Artifact { name: ".next", class: ArtifactClass::Output, owners: NODE, tier: Tier::DefaultSelected, restore: "rebuilt by the next Next.js build or dev run" },
    Artifact { name: ".nuxt", class: ArtifactClass::Output, owners: NODE, tier: Tier::DefaultSelected, restore: "rebuilt by the next Nuxt build or dev run" },
    Artifact { name: ".output", class: ArtifactClass::Output, owners: NODE, tier: Tier::ReviewOnly, restore: "rebuilt by the next Nuxt/Nitro build" },
    Artifact { name: ".svelte-kit", class: ArtifactClass::Output, owners: NODE, tier: Tier::DefaultSelected, restore: "regenerated by the next SvelteKit build or dev run" },
    Artifact { name: ".angular", class: ArtifactClass::Output, owners: NODE, tier: Tier::DefaultSelected, restore: "Angular CLI cache; rebuilt on the next build" },
    Artifact { name: ".turbo", class: ArtifactClass::Output, owners: NODE, tier: Tier::DefaultSelected, restore: "Turborepo cache; tasks re-run on the next build" },
    Artifact { name: ".parcel-cache", class: ArtifactClass::Output, owners: NODE, tier: Tier::DefaultSelected, restore: "Parcel cache; rebuilt on the next build" },
    Artifact { name: ".astro", class: ArtifactClass::Output, owners: NODE, tier: Tier::DefaultSelected, restore: "regenerated by the next Astro build or dev run" },
    Artifact { name: ".expo", class: ArtifactClass::Output, owners: NODE, tier: Tier::ReviewOnly, restore: "Expo regenerates it on the next start" },
    Artifact { name: "__pycache__", class: ArtifactClass::Output, owners: PY, tier: Tier::DefaultSelected, restore: "bytecode is recompiled on the next import" },
    Artifact { name: ".pytest_cache", class: ArtifactClass::Output, owners: PY, tier: Tier::DefaultSelected, restore: "pytest recreates it on the next run" },
    Artifact { name: ".mypy_cache", class: ArtifactClass::Output, owners: PY, tier: Tier::DefaultSelected, restore: "mypy recreates it on the next run (slower first check)" },
    Artifact { name: ".ruff_cache", class: ArtifactClass::Output, owners: PY, tier: Tier::DefaultSelected, restore: "ruff recreates it on the next run" },
    Artifact { name: ".gradle", class: ArtifactClass::Output, owners: GRADLE, tier: Tier::ReviewOnly, restore: "Gradle project cache; rebuilt on the next build" },
    Artifact { name: ".cxx", class: ArtifactClass::Output, owners: GRADLE, tier: Tier::ReviewOnly, restore: "Android NDK build cache; rebuilt on the next build" },
    Artifact { name: ".dart_tool", class: ArtifactClass::Output, owners: &["pubspec.yaml"], tier: Tier::DefaultSelected, restore: "regenerated by flutter/dart pub get (may need the network)" },
    Artifact { name: ".build", class: ArtifactClass::Output, owners: &["package.swift"], tier: Tier::DefaultSelected, restore: "rebuilt by the next swift build" },
    Artifact { name: "zig-out", class: ArtifactClass::Output, owners: ZIG, tier: Tier::DefaultSelected, restore: "rebuilt by the next zig build" },
    Artifact { name: ".zig-cache", class: ArtifactClass::Output, owners: ZIG, tier: Tier::DefaultSelected, restore: "rebuilt by the next zig build" },
    Artifact { name: "zig-cache", class: ArtifactClass::Output, owners: ZIG, tier: Tier::DefaultSelected, restore: "rebuilt by the next zig build" },
    Artifact { name: "deriveddata", class: ArtifactClass::Output, owners: &["*.xcodeproj", "*.xcworkspace"], tier: Tier::DefaultSelected, restore: "rebuilt by the next Xcode build" },
    Artifact { name: "cmake-build-debug", class: ArtifactClass::Output, owners: &["cmakelists.txt"], tier: Tier::DefaultSelected, restore: "rebuilt by the next CMake build" },
    Artifact { name: "cmake-build-release", class: ArtifactClass::Output, owners: &["cmakelists.txt"], tier: Tier::DefaultSelected, restore: "rebuilt by the next CMake build" },
];

/// Project-root markers (PUR-01). Version-control metadata marks a project boundary as well.
pub const PROJECT_MARKERS: &[&str] = &[
    "package.json", "cargo.toml", "go.mod", "pyproject.toml", "setup.py", "setup.cfg", "requirements.txt", "pipfile", "tox.ini", "noxfile.py",
    "pom.xml", "build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts", "composer.json", "gemfile", "podfile", "pubspec.yaml",
    "package.swift", "build.zig", "build.zig.zon", "terragrunt.hcl", "cmakelists.txt", "*.csproj", "*.fsproj", "*.vbproj", "*.sln",
    "*.xcodeproj", "*.xcworkspace", "lerna.json", "pnpm-workspace.yaml", "nx.json", "rush.json", ".git",
];

/// Names that suggest credentials or keys (never inside an artifact that is purged, PUR-03).
const SECRET_NAMES: &[&str] = &[".env", "id_rsa", "id_dsa", "id_ecdsa", "id_ed25519", ".npmrc", ".pypirc", ".netrc", ".git-credentials", "credentials.json"];
const SECRET_EXT: &[&str] = &["pem", "key", "p12", "pfx", "jks", "keystore", "ppk", "kdbx"];
const ENV_SAMPLES: &[&str] = &[".env.example", ".env.sample", ".env.template", ".env.dist"];

fn marker_match(pattern: &str, name: &str) -> bool {
    match pattern.strip_prefix('*') {
        Some(ext) => name.len() > ext.len() && name.ends_with(ext),
        None => pattern == name,
    }
}

/// The catalog entries for a (lower-case) directory name.
pub fn artifacts_named(name: &str) -> impl Iterator<Item = &'static Artifact> + '_ {
    ARTIFACTS.iter().filter(move |a| a.name == name)
}

/// Whether a directory name is an artifact name (such directories are never project containers).
pub fn is_artifact_name(name: &str) -> bool {
    ARTIFACTS.iter().any(|a| a.name == name)
}

/// Project markers (patterns of [`PROJECT_MARKERS`]) present among a directory's lower-case
/// child names; non-empty means the directory is a project root (PUR-01).
pub fn project_markers(child_names: &[String]) -> Vec<String> {
    PROJECT_MARKERS.iter().filter(|m| child_names.iter().any(|n| marker_match(m, n))).map(|m| (*m).to_owned()).collect()
}

/// The artifact a direct child named `name` (lower-case) of a project root with `markers` is:
/// only an artifact one of whose owner markers is present. Shared by the analysis and the disk
/// view's cleanable hints (DIN-05), so both recognize exactly the same directories.
pub fn owned_artifact(name: &str, markers: &[String]) -> Option<&'static Artifact> {
    artifacts_named(name).find(|a| a.owners.iter().any(|o| markers.iter().any(|m| m == o)))
}

pub fn rule_id(a: &Artifact) -> String {
    format!("{RULE_PREFIX}{}", a.name.trim_start_matches('.').replace('_', "-"))
}

/// The artifact behind a purge rule id, for presentation (rebuild class, restore cost).
pub fn artifact_for_rule(rule_id: &str) -> Option<&'static Artifact> {
    ARTIFACTS.iter().find(|a| self::rule_id(a) == rule_id)
}

pub fn rebuild_class(a: &Artifact) -> RebuildClass {
    match a.class {
        ArtifactClass::Output => RebuildClass::LocalRebuild,
        ArtifactClass::Dependency => RebuildClass::NetworkRedownload,
    }
}

pub fn is_secret_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if ENV_SAMPLES.contains(&lower.as_str()) {
        return false;
    }
    if SECRET_NAMES.contains(&lower.as_str()) || lower.starts_with(".env.") || lower.ends_with("-keypair.json") {
        return true;
    }
    lower.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && SECRET_EXT.contains(&ext))
}

// ---------------------------------------------------------------------------------------------
// Settings

/// Purge settings shared by the desktop and the CLI (versioned file next to the store).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PurgeSettings {
    pub schema_version: u32,
    /// Roots the user added.
    pub user_roots: Vec<RawPath>,
    /// Default roots the user removed.
    pub excluded_defaults: Vec<RawPath>,
    pub recent_days: u32,
    pub max_depth: u32,
}

impl Default for PurgeSettings {
    fn default() -> Self {
        PurgeSettings { schema_version: SETTINGS_VERSION, user_roots: vec![], excluded_defaults: vec![], recent_days: DEFAULT_RECENT_DAYS, max_depth: DEFAULT_MAX_DEPTH }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    #[error("purge settings are unreadable: {0}")]
    Invalid(String),
    #[error("purge settings version {0} is not supported")]
    Version(u32),
}

/// Where a configured root came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RootSource {
    Default,
    User,
}

fn same_path(a: &RawPath, b: &RawPath) -> bool {
    match (a, b) {
        (RawPath::Windows(_), RawPath::Windows(_)) => a.lossy().trim_end_matches('\\').eq_ignore_ascii_case(b.lossy().trim_end_matches('\\')),
        _ => a == b,
    }
}

impl PurgeSettings {
    pub fn parse(text: &str) -> Result<Self, SettingsError> {
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| SettingsError::Invalid(e.to_string()))?;
        match v.get("schema_version").and_then(serde_json::Value::as_u64) {
            Some(n) if n == u64::from(SETTINGS_VERSION) => {}
            Some(n) => return Err(SettingsError::Version(n.min(u64::from(u32::MAX)) as u32)),
            None => return Err(SettingsError::Invalid("missing schema_version".into())),
        }
        let s: PurgeSettings = serde_json::from_value(v).map_err(|e| SettingsError::Invalid(e.to_string()))?;
        if s.recent_days > 365 || !(1..=MAX_DEPTH_LIMIT).contains(&s.max_depth) {
            return Err(SettingsError::Invalid("recent_days must be 0..=365 and max_depth 1..=8".into()));
        }
        Ok(s)
    }

    pub fn serialize(&self) -> String {
        serde_json::to_string_pretty(self).expect("settings serialize")
    }

    /// Effective roots: defaults the user did not remove, then user roots (deduplicated).
    pub fn roots(&self, defaults: &[RawPath]) -> Vec<(RawPath, RootSource)> {
        let mut out: Vec<(RawPath, RootSource)> = Vec::new();
        let candidates = defaults.iter().filter(|d| !self.excluded_defaults.iter().any(|x| same_path(x, d))).map(|d| (d, RootSource::Default))
            .chain(self.user_roots.iter().map(|u| (u, RootSource::User)));
        for (p, src) in candidates {
            if !out.iter().any(|(q, _)| same_path(q, p)) {
                out.push((p.clone(), src));
            }
        }
        out
    }

    /// Add a root (re-enabling a removed default instead of duplicating it). False if present.
    pub fn add_root(&mut self, path: RawPath, defaults: &[RawPath]) -> bool {
        if let Some(i) = self.excluded_defaults.iter().position(|x| same_path(x, &path)) {
            self.excluded_defaults.remove(i);
            return true;
        }
        if defaults.iter().chain(&self.user_roots).any(|x| same_path(x, &path)) {
            return false;
        }
        self.user_roots.push(path);
        true
    }

    /// Remove a root (a default is recorded as excluded). False if it was not configured.
    pub fn remove_root(&mut self, path: &RawPath, defaults: &[RawPath]) -> bool {
        if let Some(i) = self.user_roots.iter().position(|x| same_path(x, path)) {
            self.user_roots.remove(i);
            return true;
        }
        if defaults.iter().any(|d| same_path(d, path)) && !self.excluded_defaults.iter().any(|x| same_path(x, path)) {
            self.excluded_defaults.push(path.clone());
            return true;
        }
        false
    }
}

// ---------------------------------------------------------------------------------------------
// Rule-set digest (catalog + thresholds)

fn catalog_base() -> String {
    let table = serde_json::json!({"catalog": CATALOG_VERSION, "artifacts": ARTIFACTS, "markers": PROJECT_MARKERS, "protection": PROTECTION_VERSION});
    digest(&table).expect("plain JSON")
}

fn digest_for(base: &str, recent_days: u32, max_depth: u32) -> String {
    Sha256::digest(format!("cm-purge-rules-v1\0{base}\0{recent_days}\0{max_depth}").as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Rule-set digest of purge candidates: the catalog plus the thresholds in force, so a plan
/// records the recent-project threshold and a catalog upgrade makes old plans stale.
pub fn rule_set_digest(recent_days: u32, max_depth: u32) -> String {
    digest_for(&catalog_base(), recent_days, max_depth)
}

/// Whether `d` is the purge digest of this build's catalog for some allowed threshold pair
/// (the executor's "rules unchanged" check for purge plans).
pub fn is_current_digest(d: &str) -> bool {
    if d.len() != 64 {
        return false;
    }
    let base = catalog_base();
    (0..=365).any(|r| (1..=MAX_DEPTH_LIMIT).any(|m| digest_for(&base, r, m) == d))
}

// ---------------------------------------------------------------------------------------------
// Analysis

/// Version-control state of a candidate directory, from a read-only probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "detail")]
pub enum VcsState {
    /// Not inside any working copy.
    NotVersioned,
    /// Inside a Git working copy; nothing at or below the directory is tracked.
    Untracked,
    /// This many index entries are at or below the directory.
    Tracked(u64),
    /// The probe could not decide (other VCS, unreadable index...): kept, never selected.
    Unknown(String),
}

/// File-content and version-control evidence the analysis needs from the platform.
pub trait PurgeProbe {
    /// Whether the file is a CACHEDIR.TAG carrying the standard signature (reads only its head).
    fn cachedir_tag(&self, path: &RawPath) -> bool;
    fn vcs(&self, dir: &RawPath) -> VcsState;
}

pub struct PurgeInput<'a> {
    pub scan_id: &'a str,
    pub revision: u32,
    pub now_unix: i64,
    pub recent_days: u32,
    pub max_depth: u32,
}

/// Per-candidate facts that do not fit [`Candidate`] (shown by previews).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PurgeFacts {
    pub candidate_id: String,
    pub project: RawPath,
    pub markers: Vec<String>,
    pub class: ArtifactClass,
    pub recent: bool,
    pub newest_activity_unix: Option<i64>,
    pub vcs: VcsState,
    pub cachedir_tag: bool,
    pub rebuild_class: RebuildClass,
    /// Inputs of the shared tier function, for explanations.
    pub tier_inputs: TierInputs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PurgeAnalysis {
    pub set: CandidateSet,
    pub facts: Vec<PurgeFacts>,
    /// Project roots found (for summaries).
    pub projects: usize,
}

fn lower(view: &dyn IndexView, e: u64) -> String {
    view.name(e).lossy().to_lowercase()
}

/// Project roots below `root` within `max_depth`, with their (lower-case) marker names.
fn find_projects(view: &dyn IndexView, root: u64, max_depth: u32) -> Vec<(u64, Vec<String>)> {
    let mut out = Vec::new();
    let mut stack = vec![(root, 0u32)];
    while let Some((dir, d)) = stack.pop() {
        let names: Vec<String> = view.children(dir).iter().map(|c| lower(view, *c)).collect();
        let markers = project_markers(&names);
        if !markers.is_empty() {
            out.push((dir, markers));
        }
        if d >= max_depth {
            continue;
        }
        for &c in view.children(dir) {
            let f = view.info(c);
            let name = lower(view, c);
            // Artifact names are never containers; dot directories and links are not searched.
            if f.kind != ObjectKind::Directory || f.boundary || name.starts_with('.') || is_artifact_name(&name) {
                continue;
            }
            stack.push((c, d + 1));
        }
    }
    out.sort_unstable();
    out
}

struct Walk {
    logical: KnownTotal,
    newest: Option<i64>,
    unknown_time: bool,
    unread: u64,
    boundaries: u64,
    placeholders: u64,
    secrets: Vec<String>,
    vcs_inside: bool,
    blocked_class: Option<ProtectionClass>,
    review_class: Option<ProtectionClass>,
}

/// Facts about everything under `entry`. Secrets are searched in the whole subtree of outputs
/// and in the top two levels of dependency directories (packages ship test fixtures).
fn walk(view: &dyn IndexView, entry: u64, secret_depth: Option<u32>) -> Walk {
    let mut w = Walk {
        logical: KnownTotal::default(), newest: None, unknown_time: false, unread: 0, boundaries: 0, placeholders: 0, secrets: vec![],
        vcs_inside: false, blocked_class: None, review_class: None,
    };
    let mut seen = BTreeSet::new();
    let mut stack = vec![(entry, 0u32)];
    while let Some((id, d)) = stack.pop() {
        let f = view.info(id);
        w.unread += f.skipped_here;
        match f.modified_unix {
            Some(t) => w.newest = Some(w.newest.map_or(t, |n| n.max(t))),
            None => w.unknown_time = true,
        }
        if id != entry {
            let name = view.name(id).lossy();
            if matches!(name.to_ascii_lowercase().as_str(), ".git" | ".hg" | ".svn") {
                w.vcs_inside = true;
            }
            if f.kind != ObjectKind::Directory && secret_depth.is_none_or(|max| d <= max) && is_secret_name(&name) && w.secrets.len() < 5 {
                w.secrets.push(view.path(id).display());
            }
            match protection_by_name(view, id) {
                Some(c @ (ProtectionClass::Credentials | ProtectionClass::AiSessionsConfig)) => w.blocked_class = w.blocked_class.or(Some(c)),
                Some(ProtectionClass::SourceWorktree) => w.vcs_inside = true,
                Some(c) => w.review_class = w.review_class.or(Some(c)),
                None => {}
            }
        }
        if f.boundary {
            w.boundaries += 1;
            continue;
        }
        if f.cloud == CloudState::Placeholder {
            w.placeholders += 1;
        }
        if f.kind == ObjectKind::Directory {
            stack.extend(view.children(id).iter().map(|c| (*c, d + 1)));
        } else if seen.insert(f.object_id) {
            let _ = w.logical.add(f.logical.map(Bytes));
        }
    }
    for _ in 0..w.unread {
        let _ = w.logical.add(None);
    }
    w
}

fn hard_deny(view: &dyn IndexView, entry: u64, path: &RawPath, prot: &AnalysisProtection<'_>) -> Option<String> {
    if let Err(r) = prefilter(path) {
        return Some(r.to_string());
    }
    let hard = prot.hard_deny?;
    let id = |e: u64| {
        let f = view.info(e);
        ObjectIdentity::new(f.volume_id.clone(), f.file_id.clone())
    };
    if let Some(r) = hard.root_at(&id(entry)) {
        return Some(format!("{} ({})", r.display, r.class.as_str()));
    }
    if let Some(r) = std::iter::successors(view.parent(entry), |p| view.parent(*p)).find_map(|a| hard.covering(&id(a))) {
        return Some(format!("inside {} ({})", r.display, r.class.as_str()));
    }
    hard_deny_within(view, entry, hard).map(|r| format!("contains {} ({})", r.display, r.class.as_str()))
}

/// Find project artifacts in a published scan. `scan_root` is the configured purge root entry
/// (usually the view's root).
pub fn analyze_purge(view: &dyn IndexView, input: &PurgeInput<'_>, prot: &AnalysisProtection<'_>, probe: &dyn PurgeProbe) -> PurgeAnalysis {
    let projects = find_projects(view, view.root(), input.max_depth.clamp(1, MAX_DEPTH_LIMIT));
    let mut candidates: BTreeMap<u64, (Candidate, PurgeFacts)> = BTreeMap::new();
    for (project, markers) in &projects {
        let project_walk = walk(view, *project, Some(0));
        let recent_cut = input.now_unix - i64::from(input.recent_days) * 86_400;
        let recent = project_walk.unknown_time && project_walk.newest.is_none() || project_walk.newest.is_some_and(|t| t >= recent_cut);
        for &child in view.children(*project) {
            let f = view.info(child);
            let name = lower(view, child);
            if f.kind != ObjectKind::Directory || f.boundary {
                continue;
            }
            let Some(artifact) = owned_artifact(&name, markers) else {
                continue;
            };
            let path = view.path(child);
            let secret_depth = match artifact.class {
                ArtifactClass::Output => None,
                ArtifactClass::Dependency => Some(1),
            };
            let w = walk(view, child, secret_depth);
            let tag = view.children(child).iter().find(|c| lower(view, **c) == "cachedir.tag" && view.info(**c).kind == ObjectKind::File).is_some_and(|c| probe.cachedir_tag(&view.path(*c)));
            let mut reasons = vec![format!("project_marker:{}", markers.join("+"))];
            let mut tier = artifact.tier;
            if tag && artifact.class == ArtifactClass::Output {
                reasons.push("cachedir_tag".into());
                tier = Tier::DefaultSelected;
            }
            if artifact.class == ArtifactClass::Dependency {
                reasons.push("dependency_reinstall_needs_network".into());
            }
            let mut blocks: Vec<String> = Vec::new();
            let mut pending: Vec<String> = Vec::new();
            if let Some(why) = hard_deny(view, child, &path, prot) {
                blocks.push(format!("hard_deny:{why}"));
            }
            match prot.user {
                UserList::NotSupplied => {}
                UserList::Loaded(user) => {
                    if let Some(e) = user.match_path(&path, true) {
                        blocks.push(format!("user_protected:{}", e.id));
                    }
                    if let Some(e) = user.protects_rule(&rule_id(artifact)) {
                        blocks.push(format!("user_protected_rule:{}", e.id));
                    }
                }
                UserList::Unavailable(_) => pending.push("user_protection_unavailable".into()),
            }
            if !w.secrets.is_empty() {
                blocks.push(format!("secret_file:{}", w.secrets.join("|")));
            }
            if let Some(c) = w.blocked_class {
                blocks.push(format!("protected:{}", c.as_str()));
            }
            if w.vcs_inside {
                blocks.push("vcs_metadata_inside".into());
            }
            if w.placeholders > 0 {
                blocks.push("cloud_placeholder".into());
            }
            // Version control is consulted for every remaining candidate (and again before acting).
            let vcs = if blocks.is_empty() { probe.vcs(&path) } else { VcsState::Unknown("not checked (already blocked)".into()) };
            match &vcs {
                VcsState::Tracked(n) => blocks.push(format!("vcs_tracked:{n}_files")),
                VcsState::Unknown(why) if blocks.is_empty() => pending.push(format!("vcs_unknown:{why}")),
                _ => {}
            }
            if w.unread > 0 || w.logical.unknown_count > 0 {
                pending.push("unread_or_unknown_size".into());
            }
            // The shared tier function (design D3): rule cap (artifact tier, recent projects not
            // preselected), dependency cap, protection and evidence layers.
            let mut inputs = TierInputs {
                rule_max: if recent { tier.min(Tier::ReviewOnly) } else { tier },
                dependency: artifact.class == ArtifactClass::Dependency,
                registry: RegistryStatus::NotChecked,
                protection: Tier::DefaultSelected,
                evidence: Tier::DefaultSelected,
                probes: vec![],
            };
            if recent {
                reasons.push(format!("recent_project:{}d", input.recent_days));
            }
            if let Some(c) = w.review_class {
                reasons.push(format!("contains:{}", c.as_str()));
                inputs.protection = Tier::ReviewOnly;
            }
            for (cond, why) in [(w.boundaries > 0, format!("contains_links_or_mounts:{}", w.boundaries)), (f.quality == IdentityQuality::Weak, "weak_identity".into()),
                (!pending.is_empty(), "evidence_incomplete".into())] {
                if cond {
                    reasons.push(why);
                    inputs.evidence = Tier::ReviewOnly;
                }
            }
            let state = if !blocks.is_empty() {
                inputs.protection = Tier::Blocked;
                CandidateState::Blocked
            } else if !pending.is_empty() {
                CandidateState::PendingEvidence
            } else {
                CandidateState::Allowed
            };
            reasons.extend(blocks);
            reasons.extend(pending);
            let decision = compute_tier(&inputs);
            reasons.extend(decision.reasons);
            let tier = decision.tier;
            let modes = if state == CandidateState::Blocked { vec![] } else { vec![Mode::Trash, Mode::Quarantine, Mode::Delete] };
            let rules = vec![RuleRef { rule_id: rule_id(artifact), version: CATALOG_VERSION.into() }];
            let category = match artifact.class {
                ArtifactClass::Output => "project_build_output",
                ArtifactClass::Dependency => "project_dependencies",
            };
            reasons.sort();
            reasons.dedup();
            let id = serde_json::json!({"scan": input.scan_id, "revision": input.revision, "entry": child, "rules": rules});
            let candidate_id = format!("c_{}", &digest(&id).expect("plain JSON")[..24]);
            let impact = format!("{} directory of the project at {}; {}", artifact.name, view.path(*project).display(), artifact.restore);
            let facts = PurgeFacts {
                candidate_id: candidate_id.clone(), project: view.path(*project), markers: markers.clone(), class: artifact.class, recent,
                newest_activity_unix: project_walk.newest, vcs, cachedir_tag: tag, rebuild_class: rebuild_class(artifact), tier_inputs: inputs,
            };
            let c = Candidate {
                candidate_id, entry_id: child, path, kind: f.kind, rules, category: category.into(), state, tier, reasons, protection: None,
                logical: w.logical, allowed_modes: modes, probes: vec![], impact, owner_id: None,
            };
            candidates.insert(child, (c, facts));
        }
    }
    // Nested projects (monorepo packages) may sit inside an artifact of an outer project only if
    // the search entered it, which it never does; an artifact inside another artifact cannot occur.
    let (candidates, facts): (Vec<Candidate>, Vec<PurgeFacts>) = candidates.into_values().unzip();
    let user_key = match prot.user {
        UserList::NotSupplied => serde_json::Value::Null,
        UserList::Loaded(u) => serde_json::to_value(u.entries()).expect("entries serialize"),
        UserList::Unavailable(_) => "unavailable".into(),
    };
    let rule_set_digest = rule_set_digest(input.recent_days, input.max_depth);
    let set_key = serde_json::json!({
        "scan": input.scan_id, "revision": input.revision, "rules": rule_set_digest, "hard_deny": prot.hard_deny.map(|_| HARD_DENY_VERSION),
        "user": user_key, "now_day": input.now_unix.div_euclid(86_400), "facts": facts,
    });
    let set = CandidateSet {
        candidate_set_id: format!("cs_{}", &digest(&serde_json::to_value(&set_key).expect("json")).expect("plain JSON")[..24]),
        scan_id: input.scan_id.into(),
        revision: input.revision,
        rule_set_digest,
        protection_version: PROTECTION_VERSION.into(),
        candidates,
        inapplicable: vec![],
    };
    PurgeAnalysis { set, facts, projects: projects.len() }
}

#[cfg(test)]
mod tests;
