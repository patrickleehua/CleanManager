//! Rule-root resolution on macOS (`~/Library/Caches`, `~/Library/Logs`, `~/Library/Application Support`, app
//! containers, browser profiles, package-manager caches, the administrator-only `/Library/Logs` and `/private/var/log`,
//! `$TMPDIR`, Downloads, a selected external volume) by file identity, with the test redirection of
//! docs/platforms/macos.md §11.
//!
//! As on Windows (ANA-01) each resolver kind maps to a fixed, documented location derived from the environment and a
//! built-in owner table; a rule only adds literal components below it and can never supply a path. Resolved roots carry
//! the identity the scan backends record (`ids`), so analysis matches them by identity, not by path text.
//!
//! - A root is identified by `fstatat` in its parent (the root itself is not followed and, unless it is a mount point,
//!   not opened). A root that is a symlink or not a directory is not resolved.
//! - Locations macOS privacy controls (TCC) protect, such as Safari's cache and other apps' containers
//!   (`~/Library/Containers/<bundle id>`), are never opened or listed to find out
//!   whether access was granted (§6: no probing of private directories): when such a location exists the rule reports
//!   `permission_required` (`permission_cause=undetermined`) and the root stays unresolved. Any other root that cannot
//!   be read for lack of permission reports the same issue.
//! - Privacy prompts (§6): a root at or below a privacy-protected location ([`privacy::classify`]: Downloads,
//!   another app's container such as Docker's logs, a selected volume) is looked up through [`privacy::Gate`], as is
//!   the existence check of a TCC location inside a container. When macOS has not answered within
//!   [`privacy::prompt_wait`] the rule reports `permission_required` and the root stays unresolved (never an empty
//!   location): `permission_cause=privacy_prompt_pending` where the prompt is the proven explanation (Downloads, an
//!   app's container), `undetermined` otherwise (a volume, iCloud Drive, File Provider storage).
//! - Scope ([`resolve_in`]): analysis matches roots by identity inside one scan, so a root outside the scan's root can
//!   never yield a candidate. Only roots at or below the scan root (compared in its given, plain `/private`-less and
//!   real spellings) are looked up; nothing outside it is opened, listed or examined (browser profile folders are
//!   listed only when they overlap the scan root).
//! - `CM_RESOLVE_HOME` / `CM_RESOLVE_TEMP` / `CM_RESOLVE_DOWNLOADS` / `CM_RESOLVE_SYSTEM` redirect resolution into
//!   validation fixtures. With a home override the package managers' own variables (`CARGO_HOME`, `GOCACHE`...) are
//!   ignored, since they name the real user's locations, and system-wide locations are not resolved unless
//!   `CM_RESOLVE_SYSTEM` names a stand-in for `/`; Downloads follows the overridden home unless it has its own override.
//! - System-wide logs are administrator-only: their rules declare `requires_admin` and the `elevated_token` probe, so
//!   without root (this build has no privileged helper) every candidate is skipped and never preselected.
//!
//! Also the installed-version facts the tested registry is checked against (CAT-03): app versions come from the
//! `Info.plist` of the owner's bundle in `/Applications` or `~/Applications` (bundle identifier checked), the system
//! version from the kernel; never from files a rule names.

use std::collections::BTreeMap;
use std::ffi::{CString, OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use cm_core::analysis::ResolvedRoot;
use cm_core::capability::CapabilityState;
use cm_core::path::RawPath;
use cm_core::rules::{RootResolver, Rule};

use super::ids;
use super::privacy::{self, GateError};
use super::sys::{Fd, describe};

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[(
    "clean.catalog",
    CapabilityState::Unverified,
    "macOS rule-root resolvers, owner probes and the wave-1 and wave-2 rule packs are implemented, but every macOS rule is experimental \
     (review-only, never preselected) until it is registered on a real Mac; gate G1 not passed",
)];

/// The validation-fixture overrides, in the order they are applied.
pub const OVERRIDE_VARS: [&str; 4] = ["CM_RESOLVE_HOME", "CM_RESOLVE_TEMP", "CM_RESOLVE_DOWNLOADS", "CM_RESOLVE_SYSTEM"];

/// Base locations; defaults come from the environment, overridable for controlled fixtures.
#[derive(Debug, Clone, Default)]
pub struct ResolverEnv {
    /// `$HOME`: every per-user location (`~/Library/...`, `~/.npm`, `~/.cargo`...) is below it.
    pub home: Option<PathBuf>,
    /// `$TMPDIR` (the per-user `/var/folders/.../T`).
    pub temp: Option<PathBuf>,
    pub downloads: Option<PathBuf>,
    /// Package-manager cache overrides the tools themselves honour (`GOCACHE`, `CARGO_HOME`, `XDG_CACHE_HOME`...).
    pub tool_env: BTreeMap<String, PathBuf>,
    /// An external volume root the user explicitly selected (CAT-08); never set implicitly.
    pub removable_volume: Option<PathBuf>,
    /// The root system-wide locations (`/Library/Logs`, `/private/var/log`) are below: `/`, or a fixture stand-in.
    pub system: Option<PathBuf>,
}

impl ResolverEnv {
    pub fn from_os() -> Self {
        Self::from_vars(|k| std::env::var_os(k))
    }

    /// [`ResolverEnv::from_os`] with an explicit variable source.
    pub fn from_vars(var: impl Fn(&str) -> Option<OsString>) -> Self {
        let get = |k: &str| var(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let home = get("HOME");
        let tool_env = TOOL_CACHES.iter().flat_map(|(_, vars, _)| vars.iter()).filter_map(|(v, _)| get(v).map(|p| ((*v).to_owned(), p))).collect();
        ResolverEnv {
            downloads: home.as_ref().map(|h| h.join("Downloads")),
            temp: get("TMPDIR"),
            home,
            tool_env,
            removable_volume: None,
            system: Some(PathBuf::from("/")),
        }
    }

    /// Apply the `CM_RESOLVE_*` fixture overrides (empty values are ignored); returns the variables that were used,
    /// each of which the caller reports as a `resolver_override` issue.
    pub fn with_overrides(mut self, var: impl Fn(&str) -> Option<OsString>) -> (Self, Vec<&'static str>) {
        let mut used = Vec::new();
        for name in OVERRIDE_VARS {
            let Some(p) = var(name).filter(|v| !v.is_empty()).map(PathBuf::from) else { continue };
            used.push(name);
            match name {
                "CM_RESOLVE_HOME" => {
                    self.downloads = Some(p.join("Downloads"));
                    self.home = Some(p);
                    self.tool_env.clear();
                    // A fixture home never reaches the real system logs either.
                    self.system = None;
                }
                "CM_RESOLVE_TEMP" => self.temp = Some(p),
                "CM_RESOLVE_SYSTEM" => self.system = Some(p),
                _ => self.downloads = Some(p),
            }
        }
        (self, used)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Profiles {
    /// Chromium: `Default` and `Profile N` directories.
    Chromium,
    /// Firefox: every profile directory under `Profiles`.
    Any,
}

/// Whether macOS privacy controls (TCC) guard the location's contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Privacy {
    Open,
    /// Readable only with Full Disk Access: never opened or listed by this build.
    Tcc,
}

const AS: &str = "Application Support";

/// Built-in owner table: browser owner id → profile parents below `$HOME`; each profile is a root. Chromium browsers keep
/// their HTTP and code caches in a per-profile folder under `~/Library/Caches` and the GPU cache in the profile itself
/// under `~/Library/Application Support`, so both parents are listed.
const BROWSERS: &[(&str, &[&str], Profiles)] = &[
    ("google.chrome", &["Library", AS, "Google", "Chrome"], Profiles::Chromium),
    ("google.chrome", &["Library", "Caches", "Google", "Chrome"], Profiles::Chromium),
    ("microsoft.edge", &["Library", AS, "Microsoft Edge"], Profiles::Chromium),
    ("microsoft.edge", &["Library", "Caches", "Microsoft Edge"], Profiles::Chromium),
    // Firefox keeps its disposable cache under ~/Library/Caches, apart from the profile.
    ("mozilla.firefox", &["Library", "Caches", "Firefox", "Profiles"], Profiles::Any),
];

/// `$HOME/Library/Containers/<bundle id>/Data/Library/Caches` of a sandboxed app; protected by macOS privacy controls.
macro_rules! container_caches {
    ($bundle:literal) => {
        &["Library", "Containers", $bundle, "Data", "Library", "Caches"]
    };
}

/// Application and system directories below `$HOME` (`application_cache`, `app_container`, `user_logs`,
/// `crash_reports`): owner id → its fixed folders. Rules only add literal components below them.
const APP_DIRS: &[(RootResolver, &str, &[&str], Privacy)] = &[
    (RootResolver::ApplicationCache, "apple.safari", &["Library", "Caches", "com.apple.Safari"], Privacy::Tcc),
    (RootResolver::ApplicationCache, "microsoft.vscode", &["Library", AS, "Code"], Privacy::Open),
    (RootResolver::ApplicationCache, "anysphere.cursor", &["Library", AS, "Cursor"], Privacy::Open),
    (RootResolver::ApplicationCache, "anthropic.claude", &["Library", AS, "Claude"], Privacy::Open),
    (RootResolver::ApplicationCache, "openai.codex", &["Library", AS, "Codex"], Privacy::Open),
    (RootResolver::ApplicationCache, "apple.xcode", &["Library", "Developer", "Xcode"], Privacy::Open),
    (RootResolver::CrashReports, "macos.diagnostic-reports", &["Library", "Logs", "DiagnosticReports"], Privacy::Open),
    // Catalog wave 2: communication, design, media and games (caches only; never message stores or sign-in data).
    (RootResolver::ApplicationCache, "slack", &["Library", AS, "Slack"], Privacy::Open),
    (RootResolver::ApplicationCache, "discord", &["Library", AS, "discord"], Privacy::Open),
    (RootResolver::AppContainer, "microsoft.teams", container_caches!("com.microsoft.teams2"), Privacy::Tcc),
    (RootResolver::UserLogs, "zoom.us", &["Library", "Logs", "zoom.us"], Privacy::Open),
    (RootResolver::ApplicationCache, "bytedance.feishu", &["Library", AS, "LarkShell"], Privacy::Open),
    (RootResolver::UserLogs, "bytedance.feishu", &["Library", AS, "LarkShell", "sdk_storage", "log"], Privacy::Open),
    (RootResolver::AppContainer, "tencent.wechat", container_caches!("com.tencent.xinWeChat"), Privacy::Tcc),
    (RootResolver::ApplicationCache, "figma", &["Library", AS, "Figma"], Privacy::Open),
    (RootResolver::ApplicationCache, "spotify", &["Library", "Caches", "com.spotify.client"], Privacy::Open),
    (RootResolver::ApplicationCache, "valve.steam", &["Library", AS, "Steam"], Privacy::Open),
    (RootResolver::ApplicationCache, "epicgames.launcher", &["Library", AS, "Epic", "EpicGamesLauncher", "Saved"], Privacy::Open),
    // Cloud and office: logs and caches only, never synced content.
    (RootResolver::UserLogs, "microsoft.onedrive", &["Library", "Logs", "OneDrive"], Privacy::Open),
    (RootResolver::UserLogs, "google.drive", &["Library", AS, "Google", "DriveFS", "Logs"], Privacy::Open),
    (RootResolver::AppContainer, "microsoft.office", container_caches!("com.microsoft.Word"), Privacy::Tcc),
    (RootResolver::AppContainer, "microsoft.office", container_caches!("com.microsoft.Excel"), Privacy::Tcc),
    (RootResolver::AppContainer, "microsoft.office", container_caches!("com.microsoft.Powerpoint"), Privacy::Tcc),
    // Virtualization: Docker Desktop's own caches and logs, never the engine disk image (`Data/vms`).
    (RootResolver::ApplicationCache, "docker.desktop", &["Library", AS, "Docker Desktop"], Privacy::Open),
    (RootResolver::UserLogs, "docker.desktop", &["Library", "Containers", "com.docker.docker", "Data", "log"], Privacy::Open),
    // Simulator runtimes: caches only, never simulator devices or their data.
    (RootResolver::ApplicationCache, "apple.coresimulator", &["Library", "Developer", "CoreSimulator"], Privacy::Open),
];

/// Administrator-only system-wide locations (`system_logs`) below the system root: owner id → its fixed folder.
const SYSTEM_DIRS: &[(RootResolver, &str, &[&str])] = &[
    (RootResolver::SystemLogs, "macos.library-logs", &["Library", "Logs"]),
    (RootResolver::SystemLogs, "macos.var-log", &["private", "var", "log"]),
];

/// Package-manager caches: owner → (override variables the tool honours, each with the components it adds), default
/// location below `$HOME`. The first override that is set wins; a custom location the tool reads from elsewhere (a
/// config file) is not followed: the rule then simply does not apply, it never guesses.
type ToolCache = (&'static str, &'static [(&'static str, &'static [&'static str])], &'static [&'static str]);
const TOOL_CACHES: &[ToolCache] = &[
    ("npm", &[("npm_config_cache", &[])], &[".npm"]),
    ("go", &[("GOCACHE", &[])], &["Library", "Caches", "go-build"]),
    ("cargo", &[("CARGO_HOME", &[])], &[".cargo"]),
    ("pnpm", &[("XDG_CACHE_HOME", &["pnpm"])], &["Library", "Caches", "pnpm"]),
    ("yarn", &[], &["Library", "Caches", "Yarn"]),
    ("pip", &[("PIP_CACHE_DIR", &[])], &["Library", "Caches", "pip"]),
    ("uv", &[("UV_CACHE_DIR", &[]), ("XDG_CACHE_HOME", &["uv"])], &[".cache", "uv"]),
    ("gradle", &[("GRADLE_USER_HOME", &[])], &[".gradle"]),
    ("homebrew", &[("HOMEBREW_CACHE", &[])], &["Library", "Caches", "Homebrew"]),
    ("cocoapods", &[], &["Library", "Caches", "CocoaPods"]),
    ("carthage", &[], &["Library", "Caches", "org.carthage.CarthageKit"]),
];

fn join(base: &Path, parts: &[&str]) -> PathBuf {
    parts.iter().fold(base.to_path_buf(), |p, c| p.join(c))
}

fn profiles(parent: &Path, style: Profiles) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(parent) else { return vec![] };
    let mut out: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink()))
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            style == Profiles::Any || n == "Default" || n.starts_with("Profile ")
        })
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// A candidate rule root before identity resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    /// Guarded by macOS privacy controls: reported, never opened.
    pub tcc_protected: bool,
}

/// Every candidate root of a rule, privacy-protected ones included.
pub fn targets(rule: &Rule, env: &ResolverEnv) -> Vec<Target> {
    targets_in(rule, env, None)
}

/// macOS spells some locations two ways (`/private/var` and `/var`, `/tmp`, `/etc`; `/System/Volumes/Data/<p>` and
/// `/<p>`): one spelling for comparisons, from the text alone.
pub(crate) fn plain_spelling(p: &Path) -> PathBuf {
    let p = p.strip_prefix("/System/Volumes/Data").map(|r| Path::new("/").join(r)).unwrap_or_else(|_| p.to_path_buf());
    for link in ["var", "tmp", "etc"] {
        if let Ok(rest) = p.strip_prefix(Path::new("/private").join(link)) {
            return Path::new("/").join(link).join(rest);
        }
    }
    p
}

/// The scan root a resolution is limited to, in each spelling that can be compared: as given (plain) and its real
/// path (looked up once, bounded by the privacy gate; left out when it does not answer).
#[derive(Debug, Clone)]
pub struct Scope(Vec<PathBuf>);

impl Scope {
    pub fn of(root: &Path, gate: &privacy::Gate) -> Scope {
        let mut spellings = vec![plain_spelling(root)];
        let owned = root.to_path_buf();
        if let Ok(Ok(real)) = gate.run(root, move || std::fs::canonicalize(&owned)) {
            spellings.push(plain_spelling(&real));
        }
        spellings.dedup();
        Scope(spellings)
    }

    /// `path` lies at or below the scan root.
    pub fn contains(&self, path: &Path) -> bool {
        let p = plain_spelling(path);
        self.0.iter().any(|s| p.starts_with(s))
    }

    /// `path` and the scan root overlap (one is at or below the other): a listing of `path` can name a root in scope.
    pub fn overlaps(&self, path: &Path) -> bool {
        let p = plain_spelling(path);
        self.0.iter().any(|s| p.starts_with(s) || s.starts_with(&p))
    }
}

/// [`targets`] limited to `scope` (every target when `None`); profile folders are listed only when they overlap it.
pub(crate) fn targets_in(rule: &Rule, env: &ResolverEnv, scope: Option<&Scope>) -> Vec<Target> {
    let all = all_targets(rule, env, scope);
    match scope {
        Some(s) => all.into_iter().filter(|t| s.contains(&t.path)).collect(),
        None => all,
    }
}

fn all_targets(rule: &Rule, env: &ResolverEnv, scope: Option<&Scope>) -> Vec<Target> {
    let owner = rule.owner.id.as_str();
    let open = |path: PathBuf| Target { path, tcc_protected: false };
    let home = env.home.as_deref();
    match rule.root_resolver {
        RootResolver::UserTemp => env.temp.iter().cloned().map(open).collect(),
        RootResolver::InstallerLocation => env.downloads.iter().cloned().map(open).collect(),
        RootResolver::RemovableVolumeRoot => env.removable_volume.iter().cloned().map(open).collect(),
        RootResolver::BrowserProfile => BROWSERS
            .iter()
            .filter(|(id, ..)| *id == owner)
            .filter_map(|(_, parts, style)| home.map(|h| (join(h, parts), *style)))
            .filter(|(parent, _)| scope.is_none_or(|s| s.overlaps(parent)))
            .flat_map(|(parent, style)| profiles(&parent, style))
            .map(open)
            .collect(),
        RootResolver::ApplicationCache | RootResolver::AppContainer | RootResolver::UserLogs | RootResolver::CrashReports => APP_DIRS
            .iter()
            .filter(|(r, id, ..)| *r == rule.root_resolver && *id == owner)
            .filter_map(|(_, _, parts, privacy)| home.map(|h| Target { path: join(h, parts), tcc_protected: *privacy == Privacy::Tcc }))
            .collect(),
        RootResolver::PackageManagerCache => TOOL_CACHES
            .iter()
            .filter(|(id, ..)| *id == owner)
            .filter_map(|(_, vars, parts)| vars.iter().find_map(|(v, add)| env.tool_env.get(*v).map(|p| join(p, add))).or_else(|| home.map(|h| join(h, parts))))
            .map(open)
            .collect(),
        RootResolver::SystemLogs => SYSTEM_DIRS
            .iter()
            .filter(|(r, id, _)| *r == rule.root_resolver && *id == owner)
            .filter_map(|(_, _, parts)| env.system.as_deref().map(|s| open(join(s, parts))))
            .collect(),
        // Other resolver kinds have no macOS mapping yet: the rule stays inapplicable.
        _ => vec![],
    }
}

/// Candidate base paths a rule may be resolved at (privacy-protected locations excluded: they are never read).
pub fn base_paths(rule: &Rule, env: &ResolverEnv) -> Vec<PathBuf> {
    targets(rule, env).into_iter().filter(|t| !t.tcc_protected).map(|t| t.path).collect()
}

/// A problem met while resolving one rule's roots; reported beside the analysis result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveIssue {
    pub code: &'static str,
    /// The rule concerned.
    pub scope: String,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct Resolution {
    pub roots: Vec<ResolvedRoot>,
    pub issues: Vec<ResolveIssue>,
}

fn cstr(p: &Path) -> Result<CString, i32> {
    CString::new(p.as_os_str().as_bytes()).map_err(|_| libc::EINVAL)
}

/// `(volume_id, file_id)` of the directory at `path` as the scan backends record it. The last component is examined
/// with `fstatat` in its parent (not followed; a symlink is refused); only a mount point is opened, to name its volume.
pub(crate) fn identity_of(path: &Path) -> Result<(String, String), i32> {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        let fd = Fd::open_dir(&cstr(path)?)?;
        let st = fd.stat()?;
        return Ok((ids::volume_id(&fd, st.st_dev)?, ids::file_id(st.st_ino)));
    };
    let pfd = Fd::open_dir_following(&cstr(parent)?)?;
    let st = pfd.stat_child(name.as_bytes())?;
    if st.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Err(libc::ENOTDIR);
    }
    let volume = if st.st_dev == pfd.stat()?.st_dev {
        ids::volume_id(&pfd, st.st_dev)?
    } else {
        let child = pfd.open_child_dir(&CString::new(name.as_bytes()).map_err(|_| libc::EINVAL)?)?;
        ids::volume_id(&child, st.st_dev)?
    };
    Ok((volume, ids::file_id(st.st_ino)))
}

fn permission_issue(rule: &str, path: &Path, tcc: bool) -> ResolveIssue {
    let message = if tcc {
        format!(
            "{} is protected by macOS privacy controls (TCC) and readable only with Full Disk Access for the program running \
             cleanmanager; this build neither probes nor requests that access, so the rule is not applied \
             (permission_cause=undetermined). Next step: grant Full Disk Access in System Settings > Privacy & Security and \
             review the location by hand",
            path.display()
        )
    } else {
        format!(
            "{} could not be examined: permission denied (permission_cause=undetermined: POSIX permissions, an ACL or macOS \
             privacy controls); the rule is not applied there. Next step: check the folder's permissions or grant Full Disk \
             Access in System Settings > Privacy & Security, then scan and analyze again",
            path.display()
        )
    };
    ResolveIssue { code: "permission_required", scope: rule.to_owned(), message }
}

/// [`identity_of`] as an injectable function.
pub(crate) type IdentityFn = Arc<dyn Fn(&Path) -> Result<(String, String), i32> + Send + Sync>;

/// The first accesses [`resolve`] makes to a candidate root, injected so tests can stand in for a pending prompt.
#[derive(Clone)]
pub(crate) struct FirstAccess {
    /// [`identity_of`].
    pub identity: IdentityFn,
    /// Whether a TCC location exists (`lstat`, not followed).
    pub exists: Arc<dyn Fn(&Path) -> bool + Send + Sync>,
}

impl FirstAccess {
    fn os() -> Self {
        FirstAccess { identity: Arc::new(identity_of), exists: Arc::new(|p| std::fs::symlink_metadata(p).is_ok()) }
    }
}

/// Run one first access to `path`, through the privacy gate when `path` is at or below a protected location.
fn gated<T: Send + 'static>(path: &Path, gate: &privacy::Gate, op: impl FnOnce(&Path) -> T + Send + 'static) -> Result<T, GateError> {
    let owned = path.to_path_buf();
    gate.run_if_protected(path, move || op(&owned))
}

fn pending_issue(rule: &str, why: GateError) -> ResolveIssue {
    ResolveIssue { code: why.issue_code(), scope: rule.to_owned(), message: format!("{why}; the rule is not applied there") }
}

/// Resolve every rule's roots to identities. Missing locations are simply absent; privacy-protected or unreadable
/// ones are reported as `permission_required` issues.
pub fn resolve(rules: &[Rule], env: &ResolverEnv) -> Resolution {
    resolve_in(rules, env, None)
}

/// [`resolve`] limited to the roots at or below `within` (a scan's root): the others cannot match anything in that
/// scan, so they are neither opened nor examined.
pub fn resolve_in(rules: &[Rule], env: &ResolverEnv, within: Option<&RawPath>) -> Resolution {
    let gate = privacy::Gate::for_home(env.home.as_deref());
    let scope = within.map(|w| match w {
        RawPath::Unix(b) => Scope::of(Path::new(OsStr::from_bytes(b)), &gate),
        // Not a macOS path: nothing of this platform's lies inside it.
        RawPath::Windows(_) => Scope(Vec::new()),
    });
    resolve_gated(rules, env, &gate, &FirstAccess::os(), scope.as_ref())
}

/// [`resolve_in`] with the privacy gate and the first accesses injected.
pub(crate) fn resolve_gated(rules: &[Rule], env: &ResolverEnv, gate: &privacy::Gate, access: &FirstAccess, scope: Option<&Scope>) -> Resolution {
    let mut out = Resolution::default();
    for rule in rules {
        for t in targets_in(rule, env, scope) {
            if t.tcc_protected {
                // Existence only (lstat in the unprotected parent): the contents are never touched.
                let exists = Arc::clone(&access.exists);
                match gated(&t.path, gate, move |p| exists(p)) {
                    Ok(true) => out.issues.push(permission_issue(&rule.rule_id, &t.path, true)),
                    Ok(false) => {}
                    Err(why) => out.issues.push(pending_issue(&rule.rule_id, why)),
                }
                continue;
            }
            let identity = Arc::clone(&access.identity);
            let found = match gated(&t.path, gate, move |p| identity(p)) {
                Ok(found) => found,
                Err(why) => {
                    out.issues.push(pending_issue(&rule.rule_id, why));
                    continue;
                }
            };
            match (found, RawPath::unix(t.path.as_os_str().as_bytes().to_vec())) {
                (Ok((volume_id, file_id)), Ok(path)) => out.roots.push(ResolvedRoot { rule_id: rule.rule_id.clone(), path, volume_id, file_id }),
                (Err(libc::EACCES | libc::EPERM), _) => out.issues.push(permission_issue(&rule.rule_id, &t.path, false)),
                _ => {}
            }
        }
    }
    out
}

/// Accept a user-selected volume for external-volume metadata cleanup (CAT-08): exactly `/Volumes/NAME`, a mount point
/// (not a link such as `/Volumes/Macintosh HD` → `/`) of a local file system (`MNT_LOCAL`) that is not the startup
/// volume (`MNT_ROOTFS`). Anything else is refused.
pub fn removable_volume_root(arg: &Path) -> Result<PathBuf, String> {
    let shown = arg.display();
    let mut comps = arg.components();
    let name = match (comps.next(), comps.next(), comps.next(), comps.next()) {
        (Some(Component::RootDir), Some(Component::Normal(v)), Some(Component::Normal(n)), None) if v == OsStr::new("Volumes") => n.to_owned(),
        _ => return Err(format!("{shown} is not a volume root such as /Volumes/NAME")),
    };
    let root = Path::new("/Volumes").join(&name);
    let meta = std::fs::symlink_metadata(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    if meta.file_type().is_symlink() {
        let to = std::fs::read_link(&root).map(|t| t.display().to_string()).unwrap_or_default();
        return Err(format!("{} is a link (to {to}), not a mounted volume", root.display()));
    }
    if !meta.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    let c = cstr(&root).map_err(describe)?;
    // Opening a volume can wait for the Removable / Network Volumes prompt, or for a slow disk (cause undetermined).
    let fd = privacy::Gate::os().run(&root, move || Fd::open_dir(&c)).map_err(GateError::into_message)?.map_err(|e| format!("{}: {}", root.display(), describe(e)))?;
    let sfs = fd.statfs().map_err(|e| format!("{}: {}", root.display(), describe(e)))?;
    if sfs.f_flags & libc::MNT_ROOTFS as u32 != 0 {
        return Err(format!("{} is on the startup volume, not a separate mounted volume", root.display()));
    }
    let parent_dev = std::fs::metadata("/Volumes").map(|m| std::os::unix::fs::MetadataExt::dev(&m)).map_err(|e| format!("/Volumes: {e}"))?;
    if fd.stat().map_err(describe)?.st_dev as u64 == parent_dev {
        return Err(format!("{} is not a mount point", root.display()));
    }
    if sfs.f_flags & libc::MNT_LOCAL as u32 == 0 {
        return Err(format!("{} is not a local volume (network or remote file system)", root.display()));
    }
    Ok(root)
}

// ---------------------------------------------------------------------------------------------
// Installed versions (CAT-03)

/// Owner id → application bundle name and the bundle identifier its `Info.plist` must carry.
const OWNER_BUNDLES: &[(&str, &str, &str)] = &[
    ("google.chrome", "Google Chrome.app", "com.google.Chrome"),
    ("microsoft.edge", "Microsoft Edge.app", "com.microsoft.edgemac"),
    ("mozilla.firefox", "Firefox.app", "org.mozilla.firefox"),
    ("apple.safari", "Safari.app", "com.apple.Safari"),
    ("microsoft.vscode", "Visual Studio Code.app", "com.microsoft.VSCode"),
    ("anysphere.cursor", "Cursor.app", "com.todesktop.230313mzl4w4u92"),
    ("anthropic.claude", "Claude.app", "com.anthropic.claudefordesktop"),
    ("openai.codex", "Codex.app", "com.openai.codex"),
    ("apple.xcode", "Xcode.app", "com.apple.dt.Xcode"),
    ("slack", "Slack.app", "com.tinyspeck.slackmacgap"),
    ("discord", "Discord.app", "com.hnc.Discord"),
    ("microsoft.teams", "Microsoft Teams.app", "com.microsoft.teams2"),
    ("zoom.us", "zoom.us.app", "us.zoom.xos"),
    ("tencent.wechat", "WeChat.app", "com.tencent.xinWeChat"),
    ("figma", "Figma.app", "com.figma.Desktop"),
    ("spotify", "Spotify.app", "com.spotify.client"),
    ("google.drive", "Google Drive.app", "com.google.drivefs"),
    ("docker.desktop", "Docker.app", "com.docker.docker"),
];

/// Largest `Info.plist` read; real ones are a few KiB.
const MAX_INFO_PLIST: u64 = 1 << 20;

/// macOS product version (`kern.osproductversion`, e.g. `15.6.1`).
pub fn os_version() -> Option<String> {
    let mut buf = [0u8; 64];
    let mut len = buf.len();
    // SAFETY: NUL-terminated name, writable buffer of `len` bytes; no new value is set.
    let rc = unsafe { libc::sysctlbyname(c"kern.osproductversion".as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) };
    if rc != 0 {
        return None;
    }
    let text = std::str::from_utf8(&buf[..len.min(buf.len())]).ok()?.trim_end_matches('\0').trim().to_owned();
    cm_core::catalog::parse_version(&text).map(|_| text)
}

/// `CFBundleShortVersionString` of the bundle at `app` when its identifier is `bundle_id`.
fn bundle_version(app: &Path, bundle_id: &str) -> Option<String> {
    let plist = app.join("Contents").join("Info.plist");
    if std::fs::metadata(&plist).ok()?.len() > MAX_INFO_PLIST {
        return None;
    }
    let (info, _) = super::ffi::cf::plist_from_bytes(&std::fs::read(&plist).ok()?).ok()?;
    if info.get("CFBundleIdentifier")?.as_str()? != bundle_id {
        return None;
    }
    let v = info.get("CFBundleShortVersionString")?.as_str()?.trim().to_owned();
    cm_core::catalog::parse_version(&v).map(|_| v)
}

/// Installed version per rule owner. An owner whose installs disagree, or that has no bundle record, is absent
/// (unknown), which keeps its rules at most review_only.
pub fn installed_versions(rules: &[Rule]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let owners: std::collections::BTreeSet<&str> = rules.iter().map(|r| r.owner.id.as_str()).collect();
    let mut app_dirs = vec![PathBuf::from("/Applications")];
    app_dirs.extend(std::env::var_os("HOME").filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join("Applications")));
    for owner in owners {
        let found = if owner == "macos" || owner.starts_with("macos.") {
            os_version()
        } else if let Some((_, bundle, id)) = OWNER_BUNDLES.iter().find(|(o, ..)| *o == owner) {
            let mut versions: Vec<String> = app_dirs.iter().filter_map(|d| bundle_version(&d.join(bundle), id)).collect();
            versions.sort_unstable();
            versions.dedup();
            (versions.len() == 1).then(|| versions.remove(0))
        } else {
            None
        };
        if let Some(v) = found {
            out.insert(owner.to_owned(), v);
        }
    }
    out
}

#[cfg(test)]
mod tests;
