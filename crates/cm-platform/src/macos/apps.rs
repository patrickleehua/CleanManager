//! Read-only application inventory on macOS (MVP 11.2, APP-01/03/04): app bundles with their
//! `Info.plist`, Mac App Store receipts, Homebrew casks, installer package receipts and the
//! signature's claimed Team ID.
//!
//! Sources, all read-only:
//! - Bundles (`*.app`) in `/Applications` (two levels, so `Utilities/` and vendor folders are
//!   included), `~/Applications` (two levels) and `/System/Applications` (two levels; always the
//!   protected class `os_package`, as is any bundle on the sealed read-only system volume).
//! - `Contents/Info.plist` (XML or binary, through CoreFoundation): `CFBundleIdentifier`,
//!   `CFBundleShortVersionString` / `CFBundleVersion`, `CFBundleDisplayName` / `CFBundleName`.
//!   A missing or unparsable plist is `info_plist_unreadable` evidence; no field is invented and
//!   the name falls back to the bundle's own file name, recorded as such.
//! - Install source: `Contents/_MASReceipt/receipt` makes an instance `app_store`; otherwise a
//!   Homebrew Caskroom (`/opt/homebrew/Caskroom`, `/usr/local/Caskroom`) whose
//!   `<token>/.metadata/**/<token>.json` lists the bundle under an `app` artifact makes it
//!   `homebrew`; otherwise it is `app_bundle`. Installer receipts (`/var/db/receipts/*.plist`)
//!   whose package identifier matches the bundle ID are evidence only, never a source.
//! - Publisher: the code signature's Team ID and leaf certificate summary as the signature claims
//!   them (Security framework signing information, not validated trust); `Apple` only when the
//!   bundle satisfies `anchor apple`.
//!
//! Every bundle path is its own instance: two copies with the same bundle ID stay two instances
//! (APP-01). Nothing here lists data folders, so a folder without a matching bundle stays unknown
//! in `cm_core::apps::assess_directory` (APP-03). No bundle is opened for writing, no installer,
//! `brew` or other program is started, and bundle locations are only opened for their identity.
//!
//! Tests redirect every location with `CM_UNINSTALL_TEST_ROOT` (a fake `/`: `Applications/`,
//! `System/Applications/`, `opt/homebrew/Caskroom/`, `usr/local/Caskroom/`, `var/db/receipts/`,
//! `Library/`) and `CM_UNINSTALL_TEST_HOME` (a fake home), the same pair as on Windows: the root
//! requires the home.

use std::collections::BTreeMap;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use cm_core::apps::{
    AppInstance, AppSource, Evidence, EvidenceKind, HomebrewKind, InventoryIssue, LocationIdentity, ProtectedReason, clean_text, merge_instances,
};
use cm_core::capability::{Capability, CapabilityState};
use cm_core::path::RawPath;

use super::ffi::cf::{self, Plist};
use super::ffi::security::{self, Signature};
use super::ids;
use super::sys::{Fd, describe};

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[(
    "apps.inventory",
    CapabilityState::Unverified,
    "Read-only bundle inventory (/Applications, ~/Applications, /System/Applications, App Store receipts, Homebrew casks) is implemented and tested on hosted runners only; not verified on a real Mac",
)];

/// Uninstall capability of every macOS instance in this build (APP-04): the inventory never moves a bundle.
pub const MAC_READ_ONLY_REASON: &str = "Read-only in this build: CleanManager does not run uninstallers or move app bundles on macOS; quit the app and use its own uninstaller, the App Store, `brew uninstall --cask` or the Finder";

const INFO_PLIST_MAX: u64 = 8 << 20;
const CASK_JSON_MAX: u64 = 4 << 20;
const RECEIPT_MAX: u64 = 1 << 20;
/// `.metadata/<version>/<timestamp>/Casks/<token>.json` is three levels below `.metadata`; allow a little more.
const METADATA_DEPTH: usize = 5;
const CASKROOMS: [&str; 2] = ["/opt/homebrew/Caskroom", "/usr/local/Caskroom"];
const RECEIPTS: &str = "/var/db/receipts";

/// Where bundles, Caskrooms and receipts are read from. Tests redirect `/` and the home folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppsEnv {
    /// `None`: the real `/`. Otherwise a fake root that every system location is resolved under.
    pub root: Option<PathBuf>,
    /// The user's home (`~/Applications`); `None` when `HOME` is unset.
    pub home: Option<PathBuf>,
}

impl AppsEnv {
    /// The real locations: `/` and `$HOME`.
    pub fn real() -> AppsEnv {
        AppsEnv { root: None, home: std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from) }
    }

    /// The real environment unless `CM_UNINSTALL_TEST_ROOT` redirects it; the root requires
    /// `CM_UNINSTALL_TEST_HOME`, and both must be absolute.
    pub fn from_process() -> Result<AppsEnv, String> {
        let Some(root) = std::env::var_os("CM_UNINSTALL_TEST_ROOT") else { return Ok(AppsEnv::real()) };
        let home = std::env::var_os("CM_UNINSTALL_TEST_HOME").filter(|h| !h.is_empty()).ok_or("CM_UNINSTALL_TEST_ROOT needs CM_UNINSTALL_TEST_HOME")?;
        let (root, home) = (PathBuf::from(root), PathBuf::from(home));
        if !root.is_absolute() || !home.is_absolute() {
            return Err("CM_UNINSTALL_TEST_ROOT and CM_UNINSTALL_TEST_HOME must be absolute paths".into());
        }
        Ok(AppsEnv { root: Some(root), home: Some(home) })
    }

    pub fn is_redirected(&self) -> bool {
        self.root.is_some()
    }

    /// A system location (`/Applications`, ...) under the fake root when redirected.
    fn system(&self, abs: &str) -> PathBuf {
        match &self.root {
            None => PathBuf::from(abs),
            Some(r) => r.join(abs.trim_start_matches('/')),
        }
    }

    /// The bundle roots in scan order.
    fn roots(&self) -> Vec<(PathBuf, Domain)> {
        let mut out = vec![(self.system("/Applications"), Domain::Applications)];
        if let Some(h) = &self.home {
            out.push((h.join("Applications"), Domain::UserApplications));
        }
        out.push((self.system("/System/Applications"), Domain::System));
        out
    }
}

/// Which bundle root a bundle was found in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Domain {
    Applications,
    UserApplications,
    /// `/System/Applications`: part of the operating system.
    System,
}

#[derive(Debug, Clone)]
struct Found {
    path: PathBuf,
    domain: Domain,
    /// Directly in the root (where Homebrew moves cask apps), not in a sub-folder.
    top_level: bool,
}

/// A Caskroom `app` artifact: the bundle file name it installs and the cask that installs it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CaskClaim {
    token: String,
    metadata: PathBuf,
}

/// One installer receipt: package identifier and version.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Receipt {
    id: String,
    version: Option<String>,
    file: PathBuf,
}

/// The installed-application inventory (APP-01..04). Unreadable roots are reported as issues; a
/// missing root (no `~/Applications`, no Homebrew) is simply empty.
pub fn inventory(env: &AppsEnv) -> (Vec<AppInstance>, Vec<InventoryIssue>) {
    collect(env, &env.roots())
}

fn collect(env: &AppsEnv, roots: &[(PathBuf, Domain)]) -> (Vec<AppInstance>, Vec<InventoryIssue>) {
    let mut issues = Vec::new();
    let mut found = Vec::new();
    for (dir, domain) in roots {
        list_bundles(dir, *domain, &mut found, &mut issues);
    }
    let casks = cask_claims(env, &mut issues);
    let receipts = receipts(env, &mut issues);
    let apps = found.iter().map(|f| bundle_instance(f, &casks, &receipts)).collect();
    (merge_instances(apps), issues)
}

fn issue(code: &str, source: &Path, message: String) -> InventoryIssue {
    InventoryIssue { code: code.into(), source: source.display().to_string(), message }
}

fn is_app_name(name: &[u8]) -> bool {
    name.len() > 4 && name[name.len() - 4..].eq_ignore_ascii_case(b".app") && name[0] != b'.'
}

/// Bundles in `dir` and in its plain sub-folders (two levels). Symbolic links to bundles are
/// listed (their identity is not recorded); linked folders are not entered.
fn list_bundles(dir: &Path, domain: Domain, out: &mut Vec<Found>, issues: &mut Vec<InventoryIssue>) {
    fn walk(dir: &Path, domain: Domain, top: bool, out: &mut Vec<Found>, issues: &mut Vec<InventoryIssue>) {
        let rd = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && top => return,
            Err(e) => return issues.push(issue("root_unreadable", dir, format!("folder not listed: {e}"))),
        };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name();
            let bytes = name.as_bytes();
            if bytes.first() == Some(&b'.') {
                continue;
            }
            let Ok(ty) = e.file_type() else { continue };
            let path = e.path();
            if is_app_name(bytes) {
                if ty.is_dir() || (ty.is_symlink() && path.is_dir()) {
                    out.push(Found { path, domain, top_level: top });
                }
            } else if top && ty.is_dir() {
                walk(&path, domain, false, out, issues);
            }
        }
    }
    walk(dir, domain, true, out, issues);
}

/// Text value of `key` in a plist dictionary, cleaned for display; empty is unknown.
fn text(plist: &Plist, key: &str) -> Option<String> {
    plist.get(key).and_then(Plist::as_str).and_then(clean_text)
}

/// `Contents/Info.plist` (or `WrappedBundle/Info.plist` of an iOS app on a Mac) as a dictionary.
fn read_info_plist(bundle: &Path) -> Result<Plist, String> {
    let mut file = bundle.join("Contents").join("Info.plist");
    if std::fs::symlink_metadata(&file).is_err() {
        let wrapped = bundle.join("WrappedBundle").join("Info.plist");
        if wrapped.is_file() {
            file = wrapped;
        }
    }
    let meta = std::fs::metadata(&file).map_err(|e| format!("Contents/Info.plist: {e}"))?;
    if !meta.is_file() {
        return Err("Contents/Info.plist is not a file".into());
    }
    if meta.len() > INFO_PLIST_MAX {
        return Err(format!("Contents/Info.plist is larger than {INFO_PLIST_MAX} bytes"));
    }
    let bytes = std::fs::read(&file).map_err(|e| format!("Contents/Info.plist: {e}"))?;
    let (plist, _) = cf::plist_from_bytes(&bytes)?;
    match plist {
        Plist::Dict(_) => Ok(plist),
        _ => Err("the property list is not a dictionary".into()),
    }
}

/// Location identity of a bundle directory, opened without following a link.
fn identity_of(path: &Path) -> Result<LocationIdentity, String> {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("the bundle path is a symbolic link".into());
    }
    let c = CString::new(path.as_os_str().as_bytes()).map_err(|_| "path contains NUL".to_owned())?;
    let fd = Fd::open_dir(&c).map_err(|e| if e == libc::ELOOP { "the bundle path is a symbolic link".to_owned() } else { describe(e) })?;
    let st = fd.stat().map_err(describe)?;
    Ok(LocationIdentity { volume_id: ids::volume_id(&fd, st.st_dev).map_err(describe)?, file_id: ids::file_id(st.st_ino) })
}

/// The bundle's contents live on the sealed, read-only system volume mounted at `/`.
fn on_system_volume(path: &Path) -> bool {
    let Ok(c) = CString::new(path.as_os_str().as_bytes()) else { return false };
    // SAFETY: zeroed POD out-parameter of the declared type.
    let mut sfs: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: NUL-terminated path and valid out-pointer.
    if unsafe { libc::statfs(c.as_ptr(), &mut sfs) } != 0 {
        return false;
    }
    let mount: Vec<u8> = sfs.f_mntonname.iter().take_while(|&&b| b != 0).map(|&b| b as u8).collect();
    sfs.f_flags & (libc::MNT_RDONLY as u32) != 0 && mount == b"/"
}

/// Publisher text from a signature: `Apple` for Apple's own code, else the leaf certificate's
/// name when it carries the Team ID (`Developer ID Application: Name (TEAMID)` gives
/// `Name (TEAMID)`), else the Team ID alone.
fn publisher_of(sig: &Signature) -> Option<String> {
    if sig.apple {
        return Some("Apple".into());
    }
    let team = sig.team_id.as_deref().and_then(clean_text)?;
    let leaf = sig.authorities.first().map(String::as_str).unwrap_or_default();
    let named = leaf.split_once(": ").map(|(_, rest)| rest.trim()).filter(|rest| rest.ends_with(&format!("({team})")) && rest.len() > team.len() + 3);
    Some(named.and_then(clean_text).unwrap_or(team))
}

/// Team ID evidence as the signature claims it (not validated trust).
fn signature_evidence(sig: &Result<Signature, String>) -> Evidence {
    let detail = match sig {
        Ok(s) if s.apple => "none; signed by Apple (satisfies anchor apple, basic validation)".to_owned(),
        Ok(s) => match (&s.team_id, s.signed, s.adhoc) {
            (Some(t), _, _) => match s.authorities.first() {
                Some(leaf) => format!("{t} (signature claimed, not validated; leaf certificate: {leaf})"),
                None => format!("{t} (signature claimed, not validated)"),
            },
            (None, true, true) => "none; ad-hoc signature".to_owned(),
            (None, true, false) => "none; signature carries no Team ID".to_owned(),
            (None, false, _) => "none; not signed".to_owned(),
        },
        Err(e) => format!("unknown; code signature unreadable: {e}"),
    };
    Evidence::new(EvidenceKind::TeamId, detail)
}

/// Bundle names installed by `app` artifacts of a cask's metadata JSON (renames via `target`
/// included; absolute targets are not matched).
fn cask_app_names(v: &serde_json::Value) -> Vec<String> {
    fn base(s: &str) -> Option<String> {
        if s.starts_with('/') || s.starts_with('~') {
            return None;
        }
        s.trim_end_matches('/').rsplit('/').next().filter(|n| is_app_name(n.as_bytes())).map(str::to_owned)
    }
    let mut out = Vec::new();
    for art in v.get("artifacts").and_then(serde_json::Value::as_array).into_iter().flatten() {
        let Some(app) = art.get("app").and_then(serde_json::Value::as_array) else { continue };
        let mut last: Option<Option<String>> = None;
        for item in app {
            if let Some(s) = item.as_str() {
                if let Some(Some(prev)) = last.take() {
                    out.push(prev);
                }
                last = Some(base(s));
            } else if let Some(t) = item.get("target").and_then(serde_json::Value::as_str) {
                last = Some(base(t));
            }
        }
        if let Some(Some(prev)) = last {
            out.push(prev);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `<token>.json` files below a cask's `.metadata` folder (bounded depth, links not followed).
fn metadata_files(dir: &Path, token: &str, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let want = format!("{token}.json");
    for e in rd.flatten() {
        let Ok(ty) = e.file_type() else { continue };
        if ty.is_file() && e.file_name().as_bytes() == want.as_bytes() {
            out.push(e.path());
        } else if ty.is_dir() && depth > 0 {
            metadata_files(&e.path(), token, depth - 1, out);
        }
    }
}

/// Bundle file name -> casks whose metadata installs it, from every Caskroom.
fn cask_claims(env: &AppsEnv, issues: &mut Vec<InventoryIssue>) -> BTreeMap<Vec<u8>, Vec<CaskClaim>> {
    let mut claims: BTreeMap<Vec<u8>, Vec<CaskClaim>> = BTreeMap::new();
    for room in CASKROOMS.map(|r| env.system(r)) {
        let rd = match std::fs::read_dir(&room) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                issues.push(issue("caskroom_unreadable", &room, format!("Caskroom not listed: {e}")));
                continue;
            }
        };
        let mut tokens: Vec<_> = rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).collect();
        tokens.sort_by_key(|e| e.file_name());
        for t in tokens {
            let Some(token) = t.file_name().to_str().filter(|s| !s.starts_with('.')).map(str::to_owned) else { continue };
            let mut files = Vec::new();
            metadata_files(&t.path().join(".metadata"), &token, METADATA_DEPTH, &mut files);
            files.sort();
            for file in files {
                let parsed = std::fs::metadata(&file)
                    .ok()
                    .filter(|m| m.len() <= CASK_JSON_MAX)
                    .and_then(|_| std::fs::read(&file).ok())
                    .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
                let Some(v) = parsed else {
                    issues.push(issue("cask_metadata_unreadable", &file, "cask metadata is unreadable or not JSON".into()));
                    continue;
                };
                for name in cask_app_names(&v) {
                    let list = claims.entry(name.into_bytes()).or_default();
                    if !list.iter().any(|c| c.token == token) {
                        list.push(CaskClaim { token: token.clone(), metadata: file.clone() });
                    }
                }
            }
        }
    }
    claims
}

/// Installer receipts (`*.plist` in `/var/db/receipts`).
fn receipts(env: &AppsEnv, issues: &mut Vec<InventoryIssue>) -> Vec<Receipt> {
    let dir = env.system(RECEIPTS);
    let rd = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            issues.push(issue("receipts_unreadable", &dir, format!("receipts not listed: {e}")));
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let file = e.path();
        if !e.file_type().is_ok_and(|t| t.is_file()) || file.extension().is_none_or(|x| x != "plist") {
            continue;
        }
        let plist = std::fs::metadata(&file)
            .ok()
            .filter(|m| m.len() <= RECEIPT_MAX)
            .and_then(|_| std::fs::read(&file).ok())
            .and_then(|b| cf::plist_from_bytes(&b).ok())
            .map(|(p, _)| p);
        if let Some(id) = plist.as_ref().and_then(|p| text(p, "PackageIdentifier")) {
            out.push(Receipt { id, version: plist.as_ref().and_then(|p| text(p, "PackageVersion")), file });
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Receipts whose identifier is the bundle ID or starts with it (`<bundle id>.<suffix>`).
fn matching_receipts<'a>(receipts: &'a [Receipt], bundle_id: &str) -> impl Iterator<Item = &'a Receipt> {
    let lower = bundle_id.to_ascii_lowercase();
    receipts.iter().filter(move |r| {
        let id = r.id.to_ascii_lowercase();
        id == lower || id.strip_prefix(&lower).is_some_and(|rest| rest.starts_with('.'))
    })
}

fn bundle_instance(f: &Found, casks: &BTreeMap<Vec<u8>, Vec<CaskClaim>>, receipts: &[Receipt]) -> AppInstance {
    let path_text = f.path.display().to_string();
    let file_name = f.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path_text.clone());
    let stem = file_name.get(..file_name.len().saturating_sub(4)).filter(|s| !s.is_empty()).unwrap_or(&file_name).to_owned();
    let mut evidence = vec![Evidence::new(EvidenceKind::InstallLocation, path_text.clone())];

    let info = read_info_plist(&f.path);
    let (bundle_id, version, name) = match &info {
        Ok(p) => {
            let id = text(p, "CFBundleIdentifier");
            match &id {
                Some(id) => evidence.push(Evidence::new(EvidenceKind::BundleId, id.clone())),
                None => evidence.push(Evidence::new(EvidenceKind::BundleId, "unknown; Info.plist has no CFBundleIdentifier")),
            }
            let version = text(p, "CFBundleShortVersionString").or_else(|| text(p, "CFBundleVersion"));
            let name = text(p, "CFBundleDisplayName")
                .map(|n| (n, "CFBundleDisplayName"))
                .or_else(|| text(p, "CFBundleName").map(|n| (n, "CFBundleName")))
                .unwrap_or_else(|| (stem.clone(), "bundle file name"));
            (id, version, name)
        }
        Err(e) => {
            evidence.push(Evidence::new(EvidenceKind::InfoPlistUnreadable, e.clone()));
            (None, None, (stem.clone(), "bundle file name; Info.plist unreadable"))
        }
    };
    evidence.push(Evidence::new(EvidenceKind::DisplayName, format!("{} ({})", name.0, name.1)));

    let identity = match identity_of(&f.path) {
        Ok(id) => {
            evidence.push(Evidence::new(EvidenceKind::LocationIdentity, format!("{} {}", id.volume_id, id.file_id)));
            Some(id)
        }
        Err(e) => {
            evidence.push(Evidence::new(EvidenceKind::LocationUnresolved, format!("identity unavailable: {e}")));
            None
        }
    };

    let mas = f.path.join("Contents").join("_MASReceipt").join("receipt").is_file();
    if mas {
        evidence.push(Evidence::new(EvidenceKind::MasReceipt, "Contents/_MASReceipt/receipt present (receipt not validated)"));
    }
    let cask = if f.top_level && f.domain != Domain::System { casks.get(file_name.as_bytes()) } else { None };
    for c in cask.into_iter().flatten() {
        evidence.push(Evidence::new(EvidenceKind::CaskroomToken, format!("{} (app artifact {file_name} in {})", c.token, c.metadata.display())));
    }
    if let Some(id) = &bundle_id {
        for r in matching_receipts(receipts, id) {
            let version = r.version.as_deref().map(|v| format!(" {v}")).unwrap_or_default();
            evidence.push(Evidence::new(
                EvidenceKind::PkgReceipt,
                format!("{}{version} ({}; identifier match only, not a file list)", r.id, r.file.display()),
            ));
        }
    }

    let sig = security::signature(&f.path);
    evidence.push(signature_evidence(&sig));

    let bundle_path = path_text.clone();
    // Only one cask may claim the bundle for it to be attributed to Homebrew; several claims stay evidence.
    let single_cask = cask.filter(|c| c.len() == 1).and_then(|c| c.first());
    let source = match (&bundle_id, mas, single_cask) {
        (Some(id), true, _) => AppSource::AppStore { bundle_id: id.clone(), path: bundle_path },
        (_, false, Some(c)) => AppSource::Homebrew { token: c.token.clone(), kind: HomebrewKind::Cask },
        (Some(id), false, None) => AppSource::AppBundle { bundle_id: id.clone(), path: bundle_path },
        (None, _, _) => AppSource::Unknown,
    };
    let location = RawPath::unix(f.path.as_os_str().as_bytes().to_vec()).ok();
    let mut app = AppInstance::new(name.0, source, location);
    app.location_identity = identity;
    app.version = version;
    app.publisher = sig.as_ref().ok().and_then(publisher_of);
    app.uninstall_capability = Capability::new("app.uninstall", CapabilityState::Unsupported, Some(MAC_READ_ONLY_REASON));
    if f.domain == Domain::System || on_system_volume(&f.path) {
        app.protected_class = Some(ProtectedReason::OsPackage);
        evidence.push(Evidence::new(EvidenceKind::ProtectedClass, format!("{:?}", ProtectedReason::OsPackage)));
    }
    app.evidence = evidence;
    app
}

#[cfg(test)]
mod tests;
