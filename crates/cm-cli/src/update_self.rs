//! Verified CLI self-update (LIF-01, add-mole-parity-suite 17.1).
//!
//! Trust chain, in order — any failure stops before a single installed file changes:
//! 1. The update **envelope** is fetched (≤ 1 MiB) and its Ed25519 signature over the exact payload
//!    bytes must verify with a key **embedded in this build** ([`trusted_keys`]). TLS is transport
//!    only; it is never a substitute for the signature.
//! 2. The signed **manifest** must be for this product and target, newer than the running version
//!    (no downgrades), and declare store schemas this store can be opened with (no update that
//!    could not read the existing history).
//! 3. Every file is downloaded into a staging directory inside the install directory, its size and
//!    SHA-256 must equal the signed manifest (checked in memory and again on disk). A mismatch
//!    deletes the staging directory and reports `verification_failed`; the current version is untouched.
//! 4. The staged program must start and identify as the new version (`capabilities --json`).
//! 5. Swap: each top-level entry is renamed into `.cleanmanager-update-backup/<old version>/` and the
//!    staged one renamed into place (same volume, so each rename is atomic). Any failure renames
//!    everything back (rollback); the old version stays usable.
//!
//! Copies installed by a package manager (winget, Scoop, npm, cargo, Homebrew) are never replaced:
//! the result names the package manager's own command. Network access happens only when a person
//! runs `self update`; nothing checks in the background.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde_json::Value;

use crate::release::{self, RELEASE_FILE, ReleaseFile, ReleaseManifest, exe_name, safe_relative, sha256_bytes};

pub const ENVELOPE_CONTRACT: &str = "cleanmanager-update-envelope/1";
pub const MANIFEST_CONTRACT: &str = "cleanmanager-update/1";
pub const MAX_ENVELOPE_BYTES: u64 = 1 << 20;
pub const MAX_FILE_BYTES: u64 = 256 << 20;
pub const MAX_TOTAL_BYTES: u64 = 512 << 20;
pub const STAGING_PREFIX: &str = ".cleanmanager-update-staging";
pub const BACKUP_DIR: &str = ".cleanmanager-update-backup";

// ---------------------------------------------------------------------------------------------
// Install method / package managers

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMethod {
    /// A release layout this program may update itself.
    Standalone,
    Winget,
    Scoop,
    Npm,
    Cargo,
    Homebrew,
    /// No release manifest: a development build or a loose copy of the binary.
    Unpackaged,
}

impl InstallMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            InstallMethod::Standalone => "standalone",
            InstallMethod::Winget => "winget",
            InstallMethod::Scoop => "scoop",
            InstallMethod::Npm => "npm",
            InstallMethod::Cargo => "cargo",
            InstallMethod::Homebrew => "homebrew",
            InstallMethod::Unpackaged => "unpackaged",
        }
    }

    pub fn managed(self) -> bool {
        !matches!(self, InstallMethod::Standalone | InstallMethod::Unpackaged)
    }
}

/// How this copy was installed, and the package manager's own update / uninstall commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub method: InstallMethod,
    pub update_command: Option<String>,
    pub uninstall_command: Option<String>,
}

fn lower_path(p: &str) -> String {
    let s = p.replace('/', "\\");
    // ASCII folding keeps byte offsets aligned with the original spelling.
    if cfg!(windows) { s.to_ascii_lowercase() } else { s }
}

/// The path component right below `root` in `exe`, when `exe` is inside `root`.
fn below(exe: &str, root: &str) -> Option<String> {
    below_lexical(exe, root).or_else(|| {
        // Environment values may use another spelling of the same directory (8.3 short names,
        // junctions); the executable path is canonical, so compare with the canonical root too.
        let canon = std::fs::canonicalize(root).ok()?.display().to_string();
        let canon = canon.strip_prefix(r"\\?\").map(str::to_owned).unwrap_or(canon);
        below_lexical(exe, &canon)
    })
}

fn below_lexical(exe: &str, root: &str) -> Option<String> {
    let (e, r) = (lower_path(exe), lower_path(root));
    let r = r.trim_end_matches('\\').to_owned() + "\\";
    if !e.starts_with(&r) {
        return None;
    }
    // Take the original spelling of the component.
    let orig = exe.replace('/', "\\");
    orig[r.len()..].split('\\').next().filter(|c| !c.is_empty()).map(str::to_owned)
}

/// Detect a package-manager installation from the executable's location (and the release
/// manifest's `install_source`, which a packager may set). `env` reads environment variables.
pub fn detect(exe: &Path, release: Option<&ReleaseManifest>, env: &dyn Fn(&str) -> Option<String>) -> Detected {
    let d = |method, update: String, uninstall: String| Detected { method, update_command: Some(update), uninstall_command: Some(uninstall) };
    let exe_s = exe.display().to_string();
    let home = env("USERPROFILE").or_else(|| env("HOME"));
    // winget: portable packages live under WinGet\Packages\<Id>_<Source>_<hash>\; links under WinGet\Links.
    let mut winget_roots = Vec::new();
    if let Some(l) = env("LOCALAPPDATA") {
        winget_roots.push(format!("{l}\\Microsoft\\WinGet\\Packages"));
        winget_roots.push(format!("{l}\\Microsoft\\WinGet\\Links"));
    }
    for pf in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(p) = env(pf) {
            winget_roots.push(format!("{p}\\WinGet\\Packages"));
            winget_roots.push(format!("{p}\\WinGet\\Links"));
        }
    }
    for root in &winget_roots {
        if let Some(dir) = below(&exe_s, root) {
            let id = if root.ends_with("Packages") { dir.split('_').next().unwrap_or("cleanmanager").to_owned() } else { "cleanmanager".into() };
            return d(InstallMethod::Winget, format!("winget upgrade --id {id} --exact"), format!("winget uninstall --id {id} --exact"));
        }
    }
    let mut scoop_roots: Vec<String> = Vec::new();
    for v in ["SCOOP", "SCOOP_GLOBAL"] {
        if let Some(s) = env(v) {
            scoop_roots.push(format!("{s}\\apps"));
        }
    }
    if let Some(h) = &home {
        scoop_roots.push(format!("{h}\\scoop\\apps"));
    }
    if let Some(p) = env("ProgramData") {
        scoop_roots.push(format!("{p}\\scoop\\apps"));
    }
    for root in &scoop_roots {
        if let Some(app) = below(&exe_s, root) {
            return d(InstallMethod::Scoop, format!("scoop update {app}"), format!("scoop uninstall {app}"));
        }
    }
    let lower = lower_path(&exe_s);
    if let Some(i) = lower.find("\\node_modules\\") {
        let rest = &exe_s.replace('/', "\\")[i + "\\node_modules\\".len()..];
        let mut parts = rest.split('\\');
        let first = parts.next().unwrap_or("cleanmanager");
        let pkg = if first.starts_with('@') { format!("{first}/{}", parts.next().unwrap_or("")) } else { first.to_owned() };
        return d(InstallMethod::Npm, format!("npm update -g {pkg}"), format!("npm uninstall -g {pkg}"));
    }
    if let Some(a) = env("APPDATA")
        && below(&exe_s, &format!("{a}\\npm")).is_some()
    {
        return d(InstallMethod::Npm, "npm update -g cleanmanager".into(), "npm uninstall -g cleanmanager".into());
    }
    let mut cargo_bins: Vec<String> = Vec::new();
    if let Some(c) = env("CARGO_HOME") {
        cargo_bins.push(format!("{c}\\bin"));
    }
    if let Some(h) = &home {
        cargo_bins.push(format!("{h}\\.cargo\\bin"));
    }
    for root in &cargo_bins {
        if below(&exe_s, root).is_some() {
            return d(InstallMethod::Cargo, "cargo install --path crates/cm-cli --locked --force".into(), "cargo uninstall cm-cli".into());
        }
    }
    for root in ["/opt/homebrew", "/usr/local/Cellar", "/home/linuxbrew/.linuxbrew"] {
        if below(&exe_s, root).is_some() {
            return d(InstallMethod::Homebrew, "brew upgrade cleanmanager".into(), "brew uninstall cleanmanager".into());
        }
    }
    match release.map(|r| r.install_source.as_str()) {
        Some("winget") => d(InstallMethod::Winget, "winget upgrade cleanmanager".into(), "winget uninstall cleanmanager".into()),
        Some("scoop") => d(InstallMethod::Scoop, "scoop update cleanmanager".into(), "scoop uninstall cleanmanager".into()),
        Some("npm") => d(InstallMethod::Npm, "npm update -g cleanmanager".into(), "npm uninstall -g cleanmanager".into()),
        Some("cargo") => d(InstallMethod::Cargo, "cargo install --path crates/cm-cli --locked --force".into(), "cargo uninstall cm-cli".into()),
        Some("homebrew") => d(InstallMethod::Homebrew, "brew upgrade cleanmanager".into(), "brew uninstall cleanmanager".into()),
        Some(_) => Detected { method: InstallMethod::Standalone, update_command: None, uninstall_command: None },
        None => Detected { method: InstallMethod::Unpackaged, update_command: None, uninstall_command: None },
    }
}

pub fn os_env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

// ---------------------------------------------------------------------------------------------
// Versions

/// Semantic-version precedence (`1.2.3`, `1.2.3-beta.2`); build metadata is ignored.
pub fn compare_versions(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    fn parse(v: &str) -> Option<([u64; 3], Vec<String>)> {
        let v = v.split('+').next()?;
        let (core, pre) = match v.split_once('-') {
            Some((c, p)) => (c, p.split('.').map(str::to_owned).collect()),
            None => (v, Vec::new()),
        };
        let mut n = [0u64; 3];
        let mut parts = core.split('.');
        for slot in &mut n {
            *slot = parts.next()?.parse().ok()?;
        }
        if parts.next().is_some() {
            return None;
        }
        Some((n, pre))
    }
    let (na, pa) = parse(a)?;
    let (nb, pb) = parse(b)?;
    use std::cmp::Ordering::*;
    Some(na.cmp(&nb).then_with(|| match (pa.is_empty(), pb.is_empty()) {
        (true, true) => Equal,
        (true, false) => Greater,
        (false, true) => Less,
        (false, false) => {
            for (x, y) in pa.iter().zip(&pb) {
                let o = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(i), Ok(j)) => i.cmp(&j),
                    (Ok(_), Err(_)) => Less,
                    (Err(_), Ok(_)) => Greater,
                    _ => x.cmp(y),
                };
                if o != Equal {
                    return o;
                }
            }
            pa.len().cmp(&pb.len())
        }
    }))
}

// ---------------------------------------------------------------------------------------------
// Keys and envelope

#[derive(Clone)]
pub struct TrustedKey {
    pub key_id: String,
    pub key: ed25519_dalek::VerifyingKey,
}

/// `sha256(public key)[..16]` in hex.
pub fn key_id(public: &[u8; 32]) -> String {
    sha256_bytes(public)[..16].to_owned()
}

fn parse_key(hex: &str) -> Option<TrustedKey> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return None;
    }
    let mut b = [0u8; 32];
    for (i, slot) in b.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
    }
    let key = ed25519_dalek::VerifyingKey::from_bytes(&b).ok()?;
    Some(TrustedKey { key_id: key_id(&b), key })
}

/// Release signing keys embedded at build time (`CM_UPDATE_PUBLIC_KEYS`: comma-separated hex
/// Ed25519 public keys, set by the release pipeline). Debug builds additionally trust
/// `CM_UPDATE_TEST_PUBLIC_KEY` from the environment so tests can drive the real binary with test
/// keys; that code is compiled out of release builds (the packager checks the string is absent).
pub fn trusted_keys() -> Vec<TrustedKey> {
    let keys: Vec<TrustedKey> = option_env!("CM_UPDATE_PUBLIC_KEYS").unwrap_or("").split(',').filter_map(parse_key).collect();
    #[cfg(debug_assertions)]
    let keys = {
        let mut keys = keys;
        if let Ok(k) = std::env::var("CM_UPDATE_TEST_PUBLIC_KEY") {
            keys.extend(parse_key(&k));
        }
        keys
    };
    keys
}

/// The update channel embedded at build time (`CM_UPDATE_MANIFEST_URL`), if any.
pub fn default_channel() -> Option<&'static str> {
    option_env!("CM_UPDATE_MANIFEST_URL").filter(|s| !s.is_empty())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// The envelope or its signature is not valid for any trusted key.
    Signature(String),
    /// Signed, but not an acceptable manifest for this product/target.
    Manifest(String),
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::Signature(s) => write!(f, "signature verification failed: {s}"),
            VerifyError::Manifest(s) => write!(f, "signed manifest rejected: {s}"),
        }
    }
}

/// Verify the envelope and return the signed payload bytes and the key that signed them.
pub fn verify_envelope(bytes: &[u8], keys: &[TrustedKey]) -> Result<(Vec<u8>, String), VerifyError> {
    let sig = |s: &str| VerifyError::Signature(s.to_owned());
    let v: Value = serde_json::from_slice(bytes).map_err(|e| VerifyError::Signature(format!("envelope is not JSON: {e}")))?;
    if v["contract"] != ENVELOPE_CONTRACT {
        return Err(sig("not a CleanManager update envelope"));
    }
    let b64 = base64::engine::general_purpose::STANDARD;
    let payload = b64.decode(v["payload_base64"].as_str().ok_or_else(|| sig("payload missing"))?).map_err(|_| sig("payload is not base64"))?;
    if keys.is_empty() {
        return Err(sig("this build has no trusted update key"));
    }
    for s in v["signatures"].as_array().ok_or_else(|| sig("signatures missing"))? {
        if s["algorithm"] != "ed25519" {
            continue;
        }
        let Some(key) = s["key_id"].as_str().and_then(|id| keys.iter().find(|k| k.key_id == id)) else { continue };
        let Some(raw) = s["signature_base64"].as_str().and_then(|x| b64.decode(x).ok()) else { continue };
        let Ok(arr) = <[u8; 64]>::try_from(raw.as_slice()) else { continue };
        if key.key.verify_strict(&payload, &ed25519_dalek::Signature::from_bytes(&arr)).is_ok() {
            return Ok((payload, key.key_id.clone()));
        }
    }
    Err(sig("no signature from a trusted key verifies this payload"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateFile {
    pub path: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateManifest {
    pub version: String,
    pub published_at: String,
    /// Schema versions the new program writes; it must be able to open (or migrate) this store.
    pub index_schema: u32,
    pub audit_schema: u32,
    pub files: Vec<UpdateFile>,
    /// `version@sha256(payload)[..16]`.
    pub release_id: String,
}

pub fn parse_manifest(payload: &[u8], target: &str) -> Result<UpdateManifest, VerifyError> {
    let bad = |s: String| VerifyError::Manifest(s);
    let v: Value = serde_json::from_slice(payload).map_err(|e| bad(format!("not JSON: {e}")))?;
    if v["contract"] != MANIFEST_CONTRACT || v["product"] != release::PRODUCT {
        return Err(bad("not a CleanManager CLI update manifest".into()));
    }
    let version = v["version"].as_str().filter(|s| compare_versions(s, s).is_some()).ok_or_else(|| bad("version missing or not semantic".into()))?;
    let schema = |k: &str| v["compat"][k].as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| bad(format!("compat.{k} missing")));
    let t = &v["targets"][target];
    let list = t["files"].as_array().ok_or_else(|| bad(format!("no files for target {target}")))?;
    let mut files = Vec::new();
    let mut total = 0u64;
    for f in list {
        let path = f["path"].as_str().ok_or_else(|| bad("file without path".into()))?;
        safe_relative(path).map_err(bad)?;
        if path == RELEASE_FILE || path.starts_with(".cleanmanager-") || files.iter().any(|x: &UpdateFile| x.path.eq_ignore_ascii_case(path)) {
            return Err(bad(format!("reserved or duplicate path {path}")));
        }
        let sha256 = f["sha256"].as_str().filter(|h| release::is_sha256(h)).ok_or_else(|| bad(format!("{path}: no SHA-256")))?;
        let size = f["size"].as_u64().filter(|n| *n <= MAX_FILE_BYTES).ok_or_else(|| bad(format!("{path}: size missing or above the limit")))?;
        let url = f["url"].as_str().filter(|u| !u.is_empty()).ok_or_else(|| bad(format!("{path}: no url")))?;
        total += size;
        files.push(UpdateFile { path: path.into(), url: url.into(), sha256: sha256.into(), size });
    }
    if total > MAX_TOTAL_BYTES {
        return Err(bad("release is larger than the update limit".into()));
    }
    if !files.iter().any(|f| f.path == exe_name()) {
        return Err(bad(format!("the release has no {}", exe_name())));
    }
    Ok(UpdateManifest {
        version: version.into(),
        published_at: v["published_at"].as_str().unwrap_or_default().into(),
        index_schema: schema("index_schema")?,
        audit_schema: schema("audit_schema")?,
        files,
        release_id: format!("{version}@{}", &sha256_bytes(payload)[..16]),
    })
}

// ---------------------------------------------------------------------------------------------
// Transport

/// Fetches update data. Tests inject an in-memory implementation; nothing here talks to the
/// network unless a person runs `self update`.
pub trait Transport {
    fn get(&self, location: &str, max_bytes: u64) -> Result<Vec<u8>, String>;
}

/// HTTPS (OS TLS stack, bounded size and time) or a local file (offline/staged updates).
pub struct SystemTransport;

impl Transport for SystemTransport {
    fn get(&self, location: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
        if location.starts_with("https://") {
            use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
            let tls = TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier).build();
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_global(Some(std::time::Duration::from_secs(600)))
                .timeout_connect(Some(std::time::Duration::from_secs(20)))
                .https_only(true)
                .max_redirects(5)
                .tls_config(tls)
                .user_agent(format!("CleanManager/{} (self-update)", cm_core::ENGINE_VERSION))
                .build()
                .into();
            let resp = agent.get(location).call().map_err(|e| format!("download failed: {e}"))?;
            return resp.into_body().with_config().limit(max_bytes).read_to_vec().map_err(|e| format!("download failed: {e}"));
        }
        if location.starts_with("http://") {
            return Err("plain http is refused; use https".into());
        }
        let path = location.strip_prefix("file:///").or_else(|| location.strip_prefix("file:")).unwrap_or(location);
        let meta = std::fs::metadata(path).map_err(|e| format!("cannot read {path}: {e}"))?;
        if meta.len() > max_bytes {
            return Err(format!("{path} is larger than {max_bytes} bytes"));
        }
        std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))
    }
}

/// Resolve a file `url` against the manifest location: absolute https stays; anything else is
/// relative to the manifest's directory. A remote manifest can never point at a local file.
pub fn resolve(base: &str, url: &str) -> Result<String, String> {
    if url.starts_with("https://") {
        return Ok(url.to_owned());
    }
    if url.contains("://") || url.starts_with("file:") || Path::new(url).is_absolute() || url.split('/').any(|c| c == "..") {
        return Err(format!("file url {url:?} must be https or relative to the manifest"));
    }
    if base.starts_with("https://") {
        let dir = &base[..base.rfind('/').map(|i| i + 1).unwrap_or(base.len())];
        return Ok(format!("{dir}{url}"));
    }
    let base_path = Path::new(base.strip_prefix("file:///").or_else(|| base.strip_prefix("file:")).unwrap_or(base));
    Ok(base_path.parent().unwrap_or(Path::new(".")).join(url).display().to_string())
}

// ---------------------------------------------------------------------------------------------
// Check and apply

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Updated,
    UpToDate,
    UpdateAvailable,
    UsePackageManager,
    CannotCheck,
    VerificationFailed,
    Incompatible,
    Locked,
    RolledBack,
    RollbackIncomplete,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Updated => "updated",
            Status::UpToDate => "up_to_date",
            Status::UpdateAvailable => "update_available",
            Status::UsePackageManager => "use_package_manager",
            Status::CannotCheck => "cannot_check",
            Status::VerificationFailed => "verification_failed",
            Status::Incompatible => "incompatible",
            Status::Locked => "locked",
            Status::RolledBack => "rolled_back",
            Status::RollbackIncomplete => "rollback_incomplete",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub status: Status,
    pub to_version: Option<String>,
    pub release_id: Option<String>,
    pub signature_verified: bool,
    pub checksum_verified: bool,
    pub reason: Option<String>,
    pub backup: Option<PathBuf>,
}

impl Outcome {
    fn new(status: Status, reason: impl Into<String>) -> Outcome {
        Outcome { status, to_version: None, release_id: None, signature_verified: false, checksum_verified: false, reason: Some(reason.into()), backup: None }
    }
}

/// Renames used by the swap; tests inject failures.
pub trait FsOps {
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()>;
}

pub struct RealFs;

impl FsOps for RealFs {
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::rename(from, to)
    }
}

/// Everything an update needs, injected so tests use scratch directories and test keys.
pub struct Context<'a> {
    pub install_dir: &'a Path,
    pub current_version: &'a str,
    pub target: &'a str,
    pub manifest_location: &'a str,
    pub keys: &'a [TrustedKey],
    pub transport: &'a dyn Transport,
    /// Schema version of the user's audit store, if one exists (the update must be able to read it).
    pub store_audit_schema: Option<u32>,
    pub store_index_schema: Option<u32>,
    /// Starts the staged program and checks it reports `version`.
    pub smoke: &'a dyn Fn(&Path, &str) -> Result<(), String>,
    pub fs: &'a dyn FsOps,
}

/// Fetch + verify the envelope and manifest, and compare versions. No file is written.
fn verified(ctx: &Context<'_>) -> Result<UpdateManifest, Outcome> {
    let bytes = ctx.transport.get(ctx.manifest_location, MAX_ENVELOPE_BYTES).map_err(|e| Outcome::new(Status::CannotCheck, e))?;
    let (payload, _key) = verify_envelope(&bytes, ctx.keys).map_err(|e| Outcome::new(Status::VerificationFailed, e.to_string()))?;
    let m = parse_manifest(&payload, ctx.target).map_err(|e| Outcome { signature_verified: true, ..Outcome::new(Status::VerificationFailed, e.to_string()) })?;
    Ok(m)
}

fn base_outcome(m: &UpdateManifest, status: Status, reason: Option<String>) -> Outcome {
    Outcome { status, to_version: Some(m.version.clone()), release_id: Some(m.release_id.clone()), signature_verified: true, checksum_verified: false, reason, backup: None }
}

/// The verified newest release compared with the running version (`self update --check`).
pub fn check(ctx: &Context<'_>) -> Outcome {
    let m = match verified(ctx) {
        Ok(m) => m,
        Err(o) => return o,
    };
    match compare_versions(&m.version, ctx.current_version) {
        Some(std::cmp::Ordering::Greater) => match compat(ctx, &m) {
            Some(why) => base_outcome(&m, Status::Incompatible, Some(why)),
            None => base_outcome(&m, Status::UpdateAvailable, None),
        },
        Some(std::cmp::Ordering::Equal) => base_outcome(&m, Status::UpToDate, None),
        _ => base_outcome(&m, Status::UpToDate, Some(format!("the channel offers {}, not newer than {}; downgrades are never installed", m.version, ctx.current_version))),
    }
}

fn compat(ctx: &Context<'_>, m: &UpdateManifest) -> Option<String> {
    if let Some(a) = ctx.store_audit_schema.filter(|a| *a > m.audit_schema) {
        return Some(format!("the operation/recovery store is at schema {a} but {} only reads up to {}; updating would make the history unreadable", m.version, m.audit_schema));
    }
    if let Some(i) = ctx.store_index_schema.filter(|i| *i > m.index_schema) {
        return Some(format!("the scan index is at schema {i} but {} only reads up to {}", m.version, m.index_schema));
    }
    None
}

/// Verify, stage, smoke-test and swap (`self update`). The caller holds the lifecycle lock.
pub fn apply(ctx: &Context<'_>, old: &ReleaseManifest) -> Outcome {
    let m = match verified(ctx) {
        Ok(m) => m,
        Err(o) => return o,
    };
    match compare_versions(&m.version, ctx.current_version) {
        Some(std::cmp::Ordering::Greater) => {}
        Some(std::cmp::Ordering::Equal) => return base_outcome(&m, Status::UpToDate, None),
        _ => return base_outcome(&m, Status::UpToDate, Some(format!("the channel offers {}, not newer than {}; downgrades are never installed", m.version, ctx.current_version))),
    }
    if let Some(why) = compat(ctx, &m) {
        return base_outcome(&m, Status::Incompatible, Some(why));
    }
    let staging = ctx.install_dir.join(format!("{STAGING_PREFIX}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let result = stage(ctx, &m, &staging).and_then(|()| {
        // Every checksum matched; the program itself is what failed.
        (ctx.smoke)(&staging.join(exe_name()), &m.version).map_err(|e| Outcome {
            checksum_verified: true,
            ..base_outcome(&m, Status::VerificationFailed, Some(format!("the downloaded program did not start as {}: {e}; nothing was replaced", m.version)))
        })
    });
    if let Err(o) = result {
        let _ = std::fs::remove_dir_all(&staging);
        return o;
    }
    // The new release manifest goes in with the files it describes.
    let new_release = ReleaseManifest {
        version: m.version.clone(),
        target: ctx.target.to_owned(),
        install_source: old.install_source.clone(),
        files: m.files.iter().map(|f| ReleaseFile { path: f.path.clone(), sha256: f.sha256.clone(), size: f.size }).collect(),
    };
    if let Err(e) = std::fs::write(staging.join(RELEASE_FILE), new_release.to_json()) {
        let _ = std::fs::remove_dir_all(&staging);
        return base_outcome(&m, Status::RolledBack, Some(format!("cannot stage {RELEASE_FILE}: {e}")));
    }
    let outcome = swap(ctx, &m, old, &staging);
    let _ = std::fs::remove_dir_all(&staging);
    outcome
}

fn stage(ctx: &Context<'_>, m: &UpdateManifest, staging: &Path) -> Result<(), Outcome> {
    let fail = |status: Status, why: String, checksum: bool| Err(Outcome { checksum_verified: checksum, ..base_outcome(m, status, Some(why)) });
    for f in &m.files {
        let url = match resolve(ctx.manifest_location, &f.url) {
            Ok(u) => u,
            Err(e) => return fail(Status::VerificationFailed, e, false),
        };
        let bytes = match ctx.transport.get(&url, f.size) {
            Ok(b) => b,
            Err(e) => return fail(Status::CannotCheck, format!("{}: {e}", f.path), false),
        };
        if bytes.len() as u64 != f.size || sha256_bytes(&bytes) != f.sha256 {
            return fail(Status::VerificationFailed, format!("checksum of {} does not match the signed manifest; nothing was replaced", f.path), false);
        }
        let dest = staging.join(safe_relative(&f.path).expect("validated"));
        let written = dest.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&dest, &bytes));
        if let Err(e) = written {
            return fail(Status::RolledBack, format!("cannot stage {}: {e}", f.path), false);
        }
        // Read back what reached the disk.
        match release::sha256_file(&dest) {
            Ok((h, n)) if h == f.sha256 && n == f.size => {}
            _ => return fail(Status::VerificationFailed, format!("checksum of the staged {} does not match the signed manifest; nothing was replaced", f.path), false),
        }
    }
    Ok(())
}

fn top_level(path: &str) -> String {
    path.split('/').next().unwrap_or(path).to_owned()
}

fn swap(ctx: &Context<'_>, m: &UpdateManifest, old: &ReleaseManifest, staging: &Path) -> Outcome {
    let dir = ctx.install_dir;
    let backup_root = dir.join(BACKUP_DIR);
    // Earlier backups are best-effort removed; the one written now is kept for manual rollback.
    let _ = std::fs::remove_dir_all(&backup_root);
    let backup = backup_root.join(ctx.current_version.replace(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-', "_"));
    if let Err(e) = std::fs::create_dir_all(&backup) {
        return Outcome { checksum_verified: true, ..base_outcome(m, Status::RolledBack, Some(format!("cannot create the backup directory: {e}"))) };
    }
    let mut new_top: Vec<String> = m.files.iter().map(|f| top_level(&f.path)).collect();
    new_top.push(RELEASE_FILE.to_owned());
    new_top.sort();
    new_top.dedup();
    // Old entries the new release no longer has are moved out too, so the layout matches its manifest.
    let mut old_only: Vec<String> = old.files.iter().map(|f| top_level(&f.path)).filter(|t| !new_top.iter().any(|n| n.eq_ignore_ascii_case(t))).collect();
    old_only.sort();
    old_only.dedup();
    enum Step {
        Moved(String),
        Placed(String),
    }
    let mut done: Vec<Step> = Vec::new();
    let mut failure = None;
    for name in old_only.iter().chain(new_top.iter()) {
        let live = dir.join(name);
        if live.exists() {
            if let Err(e) = ctx.fs.rename(&live, &backup.join(name)) {
                failure = Some(format!("cannot move {name} aside: {e}"));
                break;
            }
            done.push(Step::Moved(name.clone()));
        }
        if new_top.contains(name) {
            if let Err(e) = ctx.fs.rename(&staging.join(name), &live) {
                failure = Some(format!("cannot move the new {name} into place: {e}"));
                break;
            }
            done.push(Step::Placed(name.clone()));
        }
    }
    let Some(why) = failure else {
        return Outcome { checksum_verified: true, backup: Some(backup), ..base_outcome(m, Status::Updated, None) };
    };
    // Roll back in reverse order.
    let mut problems = Vec::new();
    for step in done.iter().rev() {
        let r = match step {
            Step::Placed(name) => ctx.fs.rename(&dir.join(name), &staging.join(name)),
            Step::Moved(name) => ctx.fs.rename(&backup.join(name), &dir.join(name)),
        };
        if let Err(e) = r {
            problems.push(match step {
                Step::Placed(n) | Step::Moved(n) => format!("{n}: {e}"),
            });
        }
    }
    if problems.is_empty() {
        let _ = std::fs::remove_dir_all(&backup_root);
        Outcome { checksum_verified: true, ..base_outcome(m, Status::RolledBack, Some(format!("{why}; every file was put back, version {} is unchanged", ctx.current_version))) }
    } else {
        Outcome {
            checksum_verified: true,
            backup: Some(backup.clone()),
            ..base_outcome(m, Status::RollbackIncomplete, Some(format!("{why}; rollback could not restore {} — the previous files are in {}", problems.join(", "), release::display(&backup))))
        }
    }
}

/// Production smoke test: run the staged program's `capabilities --json` (30 s limit) and check
/// it reports the manifest version.
pub fn smoke_test(exe: &Path, version: &str) -> Result<(), String> {
    use std::io::Read;
    let mut child = std::process::Command::new(exe)
        .args(["capabilities", "--json"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start it: {e}"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("it did not finish within 30 s".into());
            }
        }
    }
    let mut out = String::new();
    child.stdout.take().map(|mut s| s.read_to_string(&mut out));
    let v: Value = serde_json::from_str(out.trim()).map_err(|_| "it did not print a capabilities record".to_string())?;
    match v["engine_version"].as_str() {
        Some(found) if v["kind"] == "capabilities" && found == version => Ok(()),
        Some(found) => Err(format!("it reports version {found}")),
        None => Err("it did not report a version".into()),
    }
}

#[cfg(test)]
mod tests;
