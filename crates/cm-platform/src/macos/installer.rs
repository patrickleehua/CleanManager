//! Installer cleanup on macOS (INC-01..05): declared locations with source labels (Downloads, Desktop, the Homebrew
//! download cache), bounded content reads for recognition, `pkgutil` signer evidence for flat packages, attached disk
//! images from `hdiutil info`, and open-file holders. Every read is of a plain local file opened without following a
//! link, with dataless (cloud) materialization off; dataless files are never opened.
//!
//! Recognition is the shared content check of `cm_core::installer` (`.dmg` by its `koly` trailer, `.pkg`/`.xip` by
//! the `xar!` header, ZIPs by an installer payload among their first entries). A ZIP that lists no payload is only
//! taken by its name (`*.app.zip`, or words like `mac`/`installer` in it): still review-only, with evidence saying the
//! name is all there is. An image that is attached now is skipped as mounted and in use, at discovery and again
//! right before acting.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cm_core::capability::CapabilityState;
use cm_core::execute::ActionTarget;
use cm_core::installer::{ByteSource, InstallerKind, InstallerProbe, RULE_PREFIX, Verdict, classify};
use cm_core::path::RawPath;
use cm_core::plan::Plan;

use super::cmd::Tool;
use super::ffi::cf::{Plist, plist_from_bytes};
use super::procs::FdIndex;
use super::purge::{home, open_plain_file, os, raw};
use super::sys::{NoMaterialize, describe};

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[(
    "installer.cleanup",
    CapabilityState::Unverified,
    "macOS: Downloads, Desktop and the Homebrew download cache are searched; .dmg is recognized by its koly trailer, \
     .pkg/.xip by the xar header (signer from pkgutil --check-signature), ZIPs by installer payload or, review-only, by \
     name; images attached now (hdiutil info) are skipped as mounted. Downloads and Desktop are privacy-protected \
     (the first scan may ask for access). Tested on hosted runners only, not on a real Mac; gate G1 not passed",
)];

pub const SOURCE_DOWNLOADS: &str = "downloads";
pub const SOURCE_DESKTOP: &str = "desktop";
pub const SOURCE_HOMEBREW: &str = "homebrew_downloads";
/// Every source label a declared location can carry.
pub const LABELS: &[&str] = &[SOURCE_DOWNLOADS, SOURCE_DESKTOP, SOURCE_HOMEBREW];
/// Preview note for an attached image.
pub const EJECT_NOTE: &str = "attached: eject it in the Finder (Eject) or with `hdiutil detach`, then scan again";
const EJECT_HINT: &str = "Finder > Eject, or hdiutil detach";

const HDIUTIL: &str = "/usr/bin/hdiutil";
const PKGUTIL: &str = "/usr/sbin/pkgutil";
/// Words in a ZIP's name that suggest a macOS app or installer download.
const ZIP_NAME_WORDS: &[&str] = &["mac", "macos", "osx", "darwin", "universal", "installer", "install", "setup"];

fn plain_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

/// Declared locations with their source labels: Downloads, Desktop, then the Homebrew download cache
/// (`$HOMEBREW_CACHE/downloads` when set). Missing folders and links are left out; a folder inside an earlier one is
/// covered by it. `CM_RESOLVE_HOME` / `CM_RESOLVE_DOWNLOADS` redirect them into validation fixtures.
pub fn declared_locations() -> Vec<(RawPath, &'static str)> {
    let (env, used) = super::resolve::ResolverEnv::from_os().with_overrides(|k| std::env::var_os(k));
    let home = home();
    let brew_cache = std::env::var_os("HOMEBREW_CACHE")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && used.is_empty())
        .or_else(|| home.as_ref().map(|h| h.join("Library").join("Caches").join("Homebrew")));
    let all = [
        (env.downloads.clone(), SOURCE_DOWNLOADS),
        (home.as_ref().map(|h| h.join("Desktop")), SOURCE_DESKTOP),
        (brew_cache.map(|c| c.join("downloads")), SOURCE_HOMEBREW),
    ];
    let mut listed: Vec<(PathBuf, &'static str)> = Vec::new();
    for (p, label) in all.into_iter().filter_map(|(p, l)| p.map(|p| (p, l))) {
        if plain_dir(&p) && !listed.iter().any(|(q, _)| p.starts_with(q) || q.starts_with(&p)) {
            listed.push((p, label));
        }
    }
    listed.into_iter().filter_map(|(p, l)| raw(&p).map(|r| (r, l))).collect()
}

/// Bounded random-access reads of a plain local file.
pub struct FileSource {
    file: std::fs::File,
    len: u64,
}

impl FileSource {
    pub fn open(p: &Path) -> Option<FileSource> {
        let file = open_plain_file(p)?;
        let len = file.metadata().ok()?.len();
        Some(FileSource { file, len })
    }
}

impl ByteSource for FileSource {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, offset: u64, len: usize) -> Option<Vec<u8>> {
        use std::os::unix::fs::FileExt;
        let _guard = NoMaterialize::enter().ok()?;
        let mut buf = vec![0u8; len.min(usize::try_from(self.len.saturating_sub(offset)).unwrap_or(usize::MAX))];
        let mut done = 0;
        while done < buf.len() {
            match self.file.read_at(&mut buf[done..], offset + done as u64) {
                Ok(0) => break,
                Ok(n) => done += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return None,
            }
        }
        buf.truncate(done);
        Some(buf)
    }
}

/// Why a ZIP's name suggests a macOS app or installer download.
pub fn zip_name_hint(name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    if lower.ends_with(".app.zip") {
        return Some("name ends in .app.zip".into());
    }
    let stem = lower.strip_suffix(".zip")?;
    stem.split(|c: char| !c.is_alphanumeric()).find(|w| ZIP_NAME_WORDS.contains(w)).map(|w| format!("\"{w}\" in the name"))
}

/// Recognize an installer by content, refined with what only macOS names carry: `.xip` archives, and ZIPs whose
/// listing shows no payload but whose name looks like an app download (review-only by name).
pub fn classify_file(p: &Path) -> Verdict {
    let Some(src) = FileSource::open(p) else {
        return Verdict::NotInstaller { reason: "not a readable plain local file (link, dataless cloud file or unreadable)".into() };
    };
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match classify(&src) {
        Verdict::Installer { kind: InstallerKind::Pkg, .. } if name.to_lowercase().ends_with(".xip") => {
            Verdict::Installer { kind: InstallerKind::Pkg, evidence: "xip: xar archive (signed Apple archive)".into() }
        }
        Verdict::NotInstaller { reason } if reason.starts_with("archive without installer payload") => match zip_name_hint(&name) {
            Some(why) => Verdict::Installer {
                kind: InstallerKind::Archive,
                evidence: format!("archive by name only ({why}); its first entries list no installer payload, so check it before removing"),
            },
            None => Verdict::NotInstaller { reason },
        },
        v => v,
    }
}

/// The signer `pkgutil --check-signature` reports: `Apple` for Apple software, else the leaf certificate's name
/// without its role prefix (`Developer ID Installer: Name (TEAMID)` gives `Name (TEAMID)`). Unsigned, untrusted or
/// unparseable output gives `None`.
pub fn parse_pkgutil(text: &str) -> Option<String> {
    let status = text.lines().map(str::trim).find_map(|l| l.strip_prefix("Status:"))?.trim();
    if !status.starts_with("signed") || status.contains("untrusted") || status.contains("expired") || status.contains("revoked") {
        return None;
    }
    if status == "signed Apple Software" {
        return Some("Apple".into());
    }
    let leaf = text.lines().map(str::trim).find_map(|l| l.strip_prefix("1. "))?.trim();
    let name = leaf.split_once(": ").map_or(leaf, |(_, rest)| rest.trim());
    (!name.is_empty()).then(|| name.to_owned())
}

/// Signer of a flat package or `.xip` (`pkgutil --check-signature`, read-only).
pub fn pkg_signer(p: &Path) -> Option<String> {
    open_plain_file(p)?; // never let the signature check materialize a cloud file
    let out = Tool::new(PKGUTIL).arg("--check-signature").arg(p).timeout(Duration::from_secs(30)).output_caps(64 << 10, 16 << 10).run().ok()?;
    if !out.success() {
        return None;
    }
    parse_pkgutil(&String::from_utf8_lossy(&out.stdout))
}

/// `(st_dev, st_ino)` of every image file attached now, from `hdiutil info -plist` (read-only).
pub fn attached_images() -> Result<Vec<(u64, u64)>, String> {
    Ok(attached_image_paths()?.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| (m.dev(), m.ino())).collect())
}

/// Paths of every image file attached now, from `hdiutil info -plist` (read-only).
pub fn attached_image_paths() -> Result<Vec<PathBuf>, String> {
    let out = Tool::new(HDIUTIL).args(["info", "-plist"]).timeout(Duration::from_secs(20)).output_caps(4 << 20, 16 << 10).run().map_err(|e| e.to_string())?;
    if !out.success() || out.stdout_truncated {
        return Err(format!("hdiutil info failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let (plist, _) = plist_from_bytes(&out.stdout)?;
    Ok(image_paths(&plist))
}

/// `images[].image-path` of an `hdiutil info -plist` reply.
pub(crate) fn image_paths(info: &Plist) -> Vec<PathBuf> {
    match info.get("images") {
        Some(Plist::Array(images)) => images.iter().filter_map(|i| i.get("image-path").and_then(Plist::as_str)).map(PathBuf::from).collect(),
        _ => vec![],
    }
}

/// Installed applications as (display name, publisher) for installer hints (the read-only app inventory; its test
/// redirection `CM_UNINSTALL_TEST_ROOT` applies).
pub fn installed_apps() -> Vec<(String, Option<String>)> {
    match super::apps::AppsEnv::from_process() {
        Ok(env) => super::apps::inventory(&env).0.into_iter().map(|a| (a.name, a.publisher)).collect(),
        Err(_) => vec![],
    }
}

/// Preview note about a candidate's signer evidence.
pub fn signer_note(kind: InstallerKind, signer: Option<&str>) -> String {
    match (signer, kind) {
        (Some(s), _) => format!("signed by {s} (pkgutil --check-signature)"),
        (None, InstallerKind::Pkg) => "no trusted installer signature (pkgutil --check-signature)".into(),
        (None, _) => "disk images and archives carry no installer signature that is checked here".into(),
    }
}

fn identity(p: &Path) -> Option<(u64, u64)> {
    std::fs::symlink_metadata(p).ok().filter(|m| m.is_file()).map(|m| (m.dev(), m.ino()))
}

/// The platform evidence used by `cm_core::installer::analyze_installers`. The attached-image list and the open-file
/// index are taken once per probe (one discovery run).
#[derive(Default)]
pub struct MacInstallerProbe {
    attached: OnceCell<Result<Vec<(u64, u64)>, String>>,
    fds: OnceCell<Result<FdIndex, String>>,
}

impl MacInstallerProbe {
    fn attached(&self) -> &Result<Vec<(u64, u64)>, String> {
        self.attached.get_or_init(attached_images)
    }

    fn fds(&self) -> &Result<FdIndex, String> {
        self.fds.get_or_init(|| FdIndex::build().map_err(|e| format!("open-file listing failed: {}", describe(e))))
    }
}

impl InstallerProbe for MacInstallerProbe {
    fn classify(&self, path: &RawPath) -> Verdict {
        match os(path) {
            Some(p) => classify_file(&p),
            None => Verdict::NotInstaller { reason: "not a POSIX path".into() },
        }
    }

    fn signer(&self, path: &RawPath) -> Option<String> {
        pkg_signer(&os(path)?)
    }

    fn mounted(&self, path: &RawPath) -> Option<bool> {
        let id = identity(&os(path)?)?;
        self.attached().as_ref().ok().map(|list| list.contains(&id))
    }

    /// An attached image is in use by the disk-image driver; otherwise any descriptor of an inspectable process
    /// (other users' processes cannot be inspected without privileges: a limit, as for the executor's own check).
    fn in_use(&self, path: &RawPath) -> Option<bool> {
        let p = os(path)?;
        let (dev, ino) = identity(&p)?;
        if self.attached().as_ref().is_ok_and(|l| l.contains(&(dev, ino))) {
            return Some(true);
        }
        let index = self.fds().as_ref().ok()?;
        Some(!index.holders(dev as i32, ino).holders.is_empty())
    }

    fn signs(&self, kind: InstallerKind) -> bool {
        kind == InstallerKind::Pkg
    }

    fn eject_hint(&self) -> &'static str {
        EJECT_HINT
    }
}

// ---------------------------------------------------------------------------------------------
// Pre-execution gate (INC-04: an image attached after planning is skipped, never removed)

/// Whether `plan` was built from the installer catalog.
pub fn is_installer_plan(plan: &Plan) -> bool {
    !plan.rule_refs.is_empty() && plan.rule_refs.iter().all(|r| r.starts_with(RULE_PREFIX))
}

/// For installer plans, a protection check that re-reads the attached images (once per operation) and stops an item
/// whose file is attached now. When the list cannot be read, items that are disk images by content are stopped.
/// Other plans pass through.
pub fn mounted_gate(plan: &Plan) -> impl Fn(&ActionTarget<'_>) -> Result<(), String> + use<> {
    let installer = is_installer_plan(plan);
    let attached: OnceCell<Result<Vec<(u64, u64)>, String>> = OnceCell::new();
    let verdicts: std::cell::RefCell<HashMap<String, Result<(), String>>> = std::cell::RefCell::new(HashMap::new());
    move |t: &ActionTarget<'_>| {
        if !installer {
            return Ok(());
        }
        if let Some(v) = verdicts.borrow().get(t.action_id) {
            return v.clone();
        }
        let v = match os(t.path).and_then(|p| identity(&p).map(|id| (p, id))) {
            // Gone or replaced: the executor's identity check reports it.
            None => Ok(()),
            Some((p, id)) => match attached.get_or_init(attached_images) {
                Ok(list) if list.contains(&id) => Err(format!("mounted_image: {} is attached; eject it first ({EJECT_HINT})", p.display())),
                Ok(_) => Ok(()),
                Err(e) => match classify_file(&p) {
                    Verdict::Installer { kind, .. } if kind.is_image() => Err(format!("mounted_state_unknown: {e}")),
                    _ => Ok(()),
                },
            },
        };
        verdicts.borrow_mut().insert(t.action_id.to_owned(), v.clone());
        v
    }
}

#[cfg(test)]
mod tests;
