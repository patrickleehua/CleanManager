//! App update channels on macOS (parity 10.1, UPD-01..04; design D8): file probes, Homebrew, the appcast parser
//! and the `update` plan backend. The decisions live in `cm_core::updates::mac`.
//!
//! - Homebrew: `brew outdated --json=v2` through the safe tool runner (no shell, fixed argv, cleared environment plus
//!   `HOMEBREW_NO_AUTO_UPDATE=1`, `HOMEBREW_NO_ANALYTICS=1`, `HOMEBREW_NO_ENV_HINTS=1`, bounded time and output).
//!   `brew` is `/opt/homebrew/bin/brew` or `/usr/local/bin/brew`. Homebrew's prefix belongs to the user who
//!   installed it, not root, so the runner's user-owned policy applies: the file and every directory above it must
//!   be owned by root or this user and writable by nobody else (group write only for admin/wheel, whose members can
//!   already use `sudo`). Anything else is refused and the Homebrew apps are `cannot_check` with that reason. The
//!   runner executes the resolved path, which for a linked `brew` (the Intel `/usr/local/bin/brew` →
//!   `../Homebrew/bin/brew`) would make Homebrew pick the wrong prefix, so a linked `brew` is refused too.
//! - Formulae have no app bundle: [`formula_instances`] adds the formulae installed on request (Cellar receipts) to
//!   the update list, the way Scoop apps are added on Windows.
//! - Sparkle: `SUFeedURL`, `SUPublicEDKey`, `CFBundleVersion` and `CFBundleShortVersionString` from `Info.plist`
//!   (or `Sparkle.framework` without a feed); [`parse_appcast`] parses a fetched appcast with `NSXMLDocument`
//!   (external entities never loaded). Fetching is the caller's (only on a user-triggered check).
//! - Electron: `Electron Framework.framework` plus `Contents/Resources/app-update.yml` (electron-updater) or
//!   `Squirrel.framework` (Squirrel.Mac): detection only. Homebrew casks marked `auto_updates` or `version :latest`
//!   are flagged from their Caskroom metadata.
//! - Tests: with `CM_UNINSTALL_TEST_ROOT` (see `apps`) every location is under the fake root, `brew` is only
//!   [`TEST_BREW_VAR`] and appcasts come only from the [`TEST_APPCAST_DIR_VAR`] directory: nothing touches the
//!   network or the real Homebrew.

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cm_core::apps::{AppInstance, AppSource, Evidence, EvidenceKind, HomebrewKind};
use cm_core::capability::{Capability, CapabilityState};
use cm_core::path::RawPath;
use cm_core::updates::mac::{MacProbe, SparkleFeed, compare_versions, parse_homebrew_package_id, valid_brew_name};
use cm_core::updates::{Channel, ToolOutput, UpdateBackend, UpdateTarget};

use super::apps::{AppsEnv, MAC_READ_ONLY_REASON};
use super::cmd::Tool;
use super::ffi::cf::{self, Plist};

mod appcast;

pub use appcast::parse_appcast;

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[(
    "apps.updates",
    CapabilityState::Unverified,
    "Update detection (Homebrew brew outdated, Sparkle appcasts, Mac App Store and Electron status) and Homebrew upgrade plans are implemented and tested on hosted runners with fixtures only; not verified on a real Mac",
)];

/// Where Homebrew's `brew` lives (Apple silicon, Intel).
pub const BREW_PATHS: [&str; 2] = ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"];
/// Tests: the stand-in `brew` used while `CM_UNINSTALL_TEST_ROOT` redirects the inventory.
pub const TEST_BREW_VAR: &str = "CM_UPDATES_TEST_BREW";
/// Tests: directory of appcast fixtures (`<host>_<path with / as _>`) used instead of the network while redirected.
pub const TEST_APPCAST_DIR_VAR: &str = "CM_UPDATES_TEST_APPCASTS";

const CELLAR_PREFIXES: [&str; 2] = ["/opt/homebrew", "/usr/local"];
const CASKROOMS: [&str; 2] = ["/opt/homebrew/Caskroom", "/usr/local/Caskroom"];
const INFO_PLIST_MAX: u64 = 8 << 20;
const SMALL_FILE_MAX: u64 = 4 << 20;
const BREW_QUERY_TIMEOUT: Duration = Duration::from_secs(180);

/// A system location under the fake root when redirected.
fn system(env: &AppsEnv, abs: &str) -> PathBuf {
    match &env.root {
        None => PathBuf::from(abs),
        Some(r) => r.join(abs.trim_start_matches('/')),
    }
}

fn read_small(path: &Path, max: u64) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    (meta.is_file() && meta.len() <= max).then(|| std::fs::read(path).ok()).flatten()
}

fn path_of(raw: &RawPath) -> PathBuf {
    PathBuf::from(std::ffi::OsStr::from_bytes(&raw.raw_bytes()))
}

/// The `brew` to query: the stand-in while redirected, else the first of [`BREW_PATHS`] that exists.
pub fn brew_exe(env: &AppsEnv) -> Option<PathBuf> {
    if env.is_redirected() {
        return std::env::var_os(TEST_BREW_VAR).filter(|v| !v.is_empty()).map(PathBuf::from).filter(|p| p.is_absolute());
    }
    BREW_PATHS.iter().map(PathBuf::from).find(|p| std::fs::symlink_metadata(p).is_ok())
}

/// The user-owned trust policy for `brew` (see the module notes); `Ok` is the path that will run.
pub fn brew_trusted(exe: &Path) -> Result<PathBuf, String> {
    if std::fs::symlink_metadata(exe).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(format!(
            "{} is a symbolic link; CleanManager runs tools by their resolved path, which would make Homebrew use the wrong prefix",
            exe.display()
        ));
    }
    // SAFETY: geteuid has no preconditions and cannot fail.
    super::cmd::trusted_user_program(exe, unsafe { libc::geteuid() }).map_err(|e| e.to_string())
}

fn brew_tool(exe: &Path, env: &AppsEnv, argv: &[String], timeout: Duration) -> Tool {
    let mut t = Tool::new(exe)
        .args(argv)
        .owned_by_user_or_root()
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("HOMEBREW_NO_ANALYTICS", "1")
        .env("HOMEBREW_NO_ENV_HINTS", "1")
        .env("HOMEBREW_NO_COLOR", "1")
        // Homebrew's Ruby reads UTF-8 package metadata; the runner's default `C` locale would be ASCII.
        .env("LC_ALL", "en_US.UTF-8")
        .timeout(timeout)
        .output_caps(8 << 20, 256 << 10);
    if let Some(home) = &env.home {
        t = t.env("HOME", home);
    }
    for k in ["USER", "LOGNAME"] {
        if let Some(v) = std::env::var_os(k).filter(|v| !v.is_empty()) {
            t = t.env(k, v);
        }
    }
    t
}

fn tail(bytes: &[u8], n: usize) -> String {
    let s = String::from_utf8_lossy(bytes);
    let chars: Vec<char> = s.trim().chars().collect();
    chars[chars.len().saturating_sub(n)..].iter().collect::<String>().replace(['\r', '\n'], " ")
}

/// `brew outdated --json=v2` (Homebrew answers from its local package index; auto-update is off).
pub fn brew_outdated(exe: &Path, env: &AppsEnv) -> Result<String, String> {
    brew_trusted(exe)?;
    let argv: Vec<String> = ["outdated", "--json=v2"].map(str::to_owned).to_vec();
    let out = brew_tool(exe, env, &argv, BREW_QUERY_TIMEOUT).run().map_err(|e| e.to_string())?;
    if out.timed_out {
        return Err(format!("brew did not answer within {} s", BREW_QUERY_TIMEOUT.as_secs()));
    }
    if out.stdout_truncated {
        return Err("brew outdated printed more than 8 MiB".into());
    }
    // `brew outdated` exits 1 when it lists outdated packages it was asked about by name; the JSON is the answer.
    match out.exit_code {
        Some(0 | 1) if !out.stdout.is_empty() => String::from_utf8(out.stdout).map_err(|_| "brew outdated output is not UTF-8".into()),
        other => Err(format!("brew outdated exited with {other:?}: {}", tail(&out.stderr, 300))),
    }
}

fn plist_text(p: &Plist, key: &str) -> Option<String> {
    p.get(key).and_then(Plist::as_str).map(str::trim).filter(|s| !s.is_empty() && s.len() <= 2048 && !s.chars().any(char::is_control)).map(str::to_owned)
}

fn info_plist(bundle: &Path) -> Option<Plist> {
    let bytes = read_small(&bundle.join("Contents").join("Info.plist"), INFO_PLIST_MAX)?;
    match cf::plist_from_bytes(&bytes).ok()?.0 {
        p @ Plist::Dict(_) => Some(p),
        _ => None,
    }
}

/// `provider:` of an electron-updater `app-update.yml` (first top-level occurrence).
fn yml_provider(text: &str) -> Option<String> {
    text.lines().find_map(|l| l.strip_prefix("provider:")).map(|v| v.trim().trim_matches(['"', '\'']).to_owned()).filter(|v| !v.is_empty() && v.len() <= 64)
}

/// `<token>.json` files below a cask's `.metadata` folder (bounded depth, links not followed).
fn cask_metadata(dir: &Path, token: &str, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() && depth > 0 {
            cask_metadata(&e.path(), token, depth - 1, out);
        } else if ft.is_file() && e.file_name().as_bytes() == format!("{token}.json").as_bytes() {
            out.push(e.path());
        }
    }
}

/// The cask declares `auto_updates true` or `version :latest` in its installed metadata.
fn cask_self_updating(env: &AppsEnv, token: &str) -> bool {
    if !valid_brew_name(token) || token.contains('/') {
        return false;
    }
    CASKROOMS.iter().any(|room| {
        let mut files = Vec::new();
        cask_metadata(&system(env, room).join(token).join(".metadata"), token, 5, &mut files);
        files.iter().filter_map(|f| read_small(f, SMALL_FILE_MAX)).filter_map(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()).any(|v| {
            v.get("auto_updates").and_then(serde_json::Value::as_bool) == Some(true) || v.get("version").and_then(serde_json::Value::as_str) == Some("latest")
        })
    })
}

/// File-level update facts of one app (no network, nothing executed).
pub fn probe(app: &AppInstance, env: &AppsEnv) -> MacProbe {
    let mut p = MacProbe::default();
    if let AppSource::Homebrew { token, kind: HomebrewKind::Cask } = &app.source {
        p.cask_self_updating = cask_self_updating(env, token);
    }
    let Some(bundle) = app.install_location.as_ref().map(path_of).filter(|b| b.extension().is_some_and(|x| x == "app")) else { return p };
    let frameworks = bundle.join("Contents").join("Frameworks");
    let info = info_plist(&bundle);
    let feed_url = info.as_ref().and_then(|i| plist_text(i, "SUFeedURL"));
    if feed_url.is_some() || frameworks.join("Sparkle.framework").is_dir() {
        p.sparkle = Some(SparkleFeed {
            feed_url,
            public_ed_key: info.as_ref().and_then(|i| plist_text(i, "SUPublicEDKey")).is_some(),
            bundle_version: info.as_ref().and_then(|i| plist_text(i, "CFBundleVersion")),
            short_version: info.as_ref().and_then(|i| plist_text(i, "CFBundleShortVersionString")),
        });
    }
    if frameworks.join("Electron Framework.framework").is_dir() {
        let yml = bundle.join("Contents").join("Resources").join("app-update.yml");
        p.electron = match read_small(&yml, 64 << 10) {
            Some(b) => Some(match yml_provider(&String::from_utf8_lossy(&b)) {
                Some(provider) => format!("electron-updater (Contents/Resources/app-update.yml, provider {provider})"),
                None => "electron-updater (Contents/Resources/app-update.yml)".into(),
            }),
            None if frameworks.join("Squirrel.framework").is_dir() => Some("Squirrel.Mac (Electron autoUpdater, Contents/Frameworks/Squirrel.framework)".into()),
            None => None,
        };
    }
    p
}

fn version_dirs(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .filter(|n| !n.starts_with('.'))
        .collect();
    v.sort_by(|a, b| compare_versions(a, b));
    v
}

/// Formulae installed on request (`INSTALL_RECEIPT.json`), as update-list entries (`homebrew` / `formula`).
pub fn formula_instances(env: &AppsEnv) -> Vec<AppInstance> {
    let mut out = Vec::new();
    for prefix in CELLAR_PREFIXES {
        let cellar = system(env, prefix).join("Cellar");
        let Ok(rd) = std::fs::read_dir(&cellar) else { continue };
        for e in rd.flatten() {
            let Some(name) = e.file_name().to_str().map(str::to_owned).filter(|n| valid_brew_name(n) && !n.contains('/')) else { continue };
            if !e.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            // The linked keg (`opt/<name>` → `Cellar/<name>/<version>`) is the installed version; else the newest keg.
            let linked = std::fs::read_link(system(env, prefix).join("opt").join(&name)).ok().and_then(|l| l.file_name().and_then(|n| n.to_str()).map(str::to_owned));
            let versions = version_dirs(&e.path());
            let Some(version) = linked.filter(|l| versions.contains(l)).or_else(|| versions.last().cloned()) else { continue };
            let receipt = read_small(&e.path().join(&version).join("INSTALL_RECEIPT.json"), SMALL_FILE_MAX).and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
            if receipt.as_ref().and_then(|r| r.get("installed_on_request")).and_then(serde_json::Value::as_bool) != Some(true) {
                continue;
            }
            let Ok(location) = RawPath::unix(e.path().as_os_str().as_bytes().to_vec()) else { continue };
            let mut a = AppInstance::new(name.clone(), AppSource::Homebrew { token: name, kind: HomebrewKind::Formula }, Some(location));
            a.version = Some(version);
            a.uninstall_capability = Capability::new("app.uninstall", CapabilityState::Unsupported, Some(MAC_READ_ONLY_REASON));
            a.evidence.push(Evidence::new(EvidenceKind::InstallLocation, format!("Homebrew Cellar keg {}", e.path().display())));
            out.push(a);
        }
    }
    out
}

/// The installed app's signer as its signature claims it (Team ID; recorded in Homebrew cask plans and compared
/// with the upgraded bundle afterwards). Formulae have none.
pub fn installed_signer(app: &AppInstance) -> Option<String> {
    app.publisher.clone()
}

/// This Mac's product version (`kern.osproductversion`).
pub fn os_version() -> Option<String> {
    super::resolve::os_version()
}

/// The macOS backend for `update` plans: only Homebrew's fixed `brew upgrade` ever runs through it.
pub struct MacUpdates {
    pub env: AppsEnv,
}

impl MacUpdates {
    pub fn from_process() -> Result<MacUpdates, String> {
        Ok(MacUpdates { env: AppsEnv::from_process()? })
    }
}

impl UpdateBackend for MacUpdates {
    fn run(&self, exe: &str, argv: &[String], timeout_seconds: u32) -> Result<ToolOutput, String> {
        let out = brew_tool(Path::new(exe), &self.env, argv, Duration::from_secs(u64::from(timeout_seconds))).run().map_err(|e| e.to_string())?;
        let mut output = String::from_utf8_lossy(&out.stdout).into_owned();
        output.push_str(&String::from_utf8_lossy(&out.stderr));
        Ok(ToolOutput { exit_code: out.exit_code, output, timed_out: out.timed_out })
    }

    fn file_sha256(&self, path: &str) -> Option<String> {
        cm_core::adapter::sha256_hex(std::fs::File::open(path).ok()?).ok()
    }

    fn signer(&self, path: &str) -> Option<String> {
        super::ffi::security::signature(Path::new(path)).ok().as_ref().and_then(super::apps::publisher_of)
    }

    /// Homebrew verifies its own downloads; there is no separate download step on macOS.
    fn downloaded(&self, _target: &UpdateTarget, _dir: &str) -> Vec<String> {
        Vec::new()
    }

    fn scratch_dir(&self, action_id: &str) -> Result<String, String> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("cleanmanager-update-{}-{action_id}-{nanos:x}", std::process::id()));
        std::fs::create_dir(&dir).map_err(|e| format!("cannot create a download directory: {e}"))?;
        Ok(dir.to_string_lossy().into_owned())
    }

    /// Any process whose executable lies inside the app bundle (or formula keg directory).
    fn running(&self, t: &UpdateTarget) -> Option<bool> {
        let loc = t.install_location.as_deref()?;
        let prefix = format!("{}/", loc.trim_end_matches('/'));
        let snap = super::procs::snapshot().ok()?;
        Some(snap.procs.iter().any(|p| p.path.as_deref().is_some_and(|b| b.starts_with(prefix.as_bytes()))))
    }

    /// The installed version as Homebrew records it (Caskroom/Cellar version folders).
    fn installed_version(&self, t: &UpdateTarget) -> Option<String> {
        if t.channel != Channel::Homebrew {
            return None;
        }
        let (kind, name) = parse_homebrew_package_id(&t.package_id)?;
        let short = name.rsplit('/').next()?;
        let dirs: Vec<PathBuf> = match kind {
            HomebrewKind::Cask => CASKROOMS.iter().map(|r| system(&self.env, r).join(short)).collect(),
            HomebrewKind::Formula => CELLAR_PREFIXES.iter().map(|p| system(&self.env, p).join("Cellar").join(short)).collect(),
        };
        let versions: Vec<String> = dirs.iter().flat_map(|d| version_dirs(d)).collect();
        if versions.contains(&t.to_version) {
            return Some(t.to_version.clone());
        }
        versions.into_iter().max_by(|a, b| compare_versions(a, b))
    }
}

#[cfg(test)]
mod tests;
