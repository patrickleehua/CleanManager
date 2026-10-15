//! Protection policy: hard-deny list and user protection list (parity change PRT-01..06,
//! design D5).
//!
//! Three layers, strongest first:
//! 1. **Hard deny** (`HardDeny`): a textual pre-filter for obviously dangerous inputs plus a
//!    platform list of roots compared by volume + file identity. Ancestor links can only add
//!    denials; a link that cannot be resolved denies.
//! 2. **Default classes** (`DefaultClass`): the built-in [`ProtectionClass`]es computed by
//!    analysis (credentials, databases, AI sessions, worktrees...).
//! 3. **User list** (`User`): a versioned file of entries that only *add* protection. There is
//!    no allow entry, so the user list can never remove the first two layers.
//!
//! Everything here is pure: the platform layer supplies identities ([`IdentityChain`],
//! [`HardDenyRoot`]) and does file I/O (atomic save, read errors).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::analysis::{IndexView, ProtectionClass};
use crate::capability::Platform;
use crate::error::ErrorCode;
use crate::path::RawPath;
use crate::rules::glob_match;

/// Version of the hard-deny policy; bump when the textual tables or root classes change.
pub const HARD_DENY_VERSION: &str = "hard-deny/4";
/// Only protection-file schema version this build reads and writes.
pub const PROTECTION_FILE_SCHEMA_VERSION: u32 = 1;

const MAX_WINDOWS_UNITS: usize = 32_767;
const MAX_UNIX_BYTES: usize = 1_024;

// ---------------------------------------------------------------------------------------------
// Decisions
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DenyLayer {
    HardDeny,
    DefaultClass,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum ProtectionDecision {
    Allowed,
    Denied { layer: DenyLayer, reason: String },
}

impl ProtectionDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, ProtectionDecision::Allowed)
    }

    pub fn is_denied(&self) -> bool {
        !self.is_allowed()
    }

    pub fn layer(&self) -> Option<DenyLayer> {
        match self {
            ProtectionDecision::Allowed => None,
            ProtectionDecision::Denied { layer, .. } => Some(*layer),
        }
    }

    fn deny(layer: DenyLayer, reason: impl Into<String>) -> Self {
        ProtectionDecision::Denied { layer, reason: reason.into() }
    }
}

// ---------------------------------------------------------------------------------------------
// Hard-deny roots and identity checks
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardDenyClass {
    /// OS directory (`%SystemRoot%`, `/System`, `/usr`...).
    SystemRoot,
    /// Program installation root (`Program Files`, `/Applications` system apps).
    ProgramFiles,
    /// `%ProgramData%` itself (exact; its children are rule territory).
    ProgramDataRoot,
    /// Parent of all user profiles (exact).
    UsersRoot,
    /// System directory protected itself (exact) whose children are listed separately or are
    /// rule territory (`/usr` minus `/usr/local`, `/Library`, `~/Library`, `/private/var`).
    SystemContainer,
    /// Default/template profile (`C:\Users\Default`).
    DefaultProfile,
    /// Shared profile (`C:\Users\Public`, `/Users/Shared`).
    PublicProfile,
    /// A user's home/profile directory itself (exact).
    UserProfileRoot,
    /// Volume or share root (exact).
    VolumeRoot,
    /// Volume metadata (`System Volume Information`, `.Spotlight-V100`...).
    VolumeMetadata,
    /// Recycle bin / trash root (exact; items inside are manual purge targets).
    RecycleBinRoot,
    /// Recovery environment.
    Recovery,
    /// Installer rollback state (`Config.Msi`).
    InstallerState,
    /// Endpoint security software data.
    SecuritySoftware,
    /// Credential and keychain stores.
    Credentials,
    /// CleanManager's own audit/recovery store.
    ToolStore,
}

impl HardDenyClass {
    /// Whether the root denies its whole subtree (`true`) or only the object itself.
    pub fn covers_descendants(self) -> bool {
        !matches!(
            self,
            HardDenyClass::ProgramDataRoot
                | HardDenyClass::UsersRoot
                | HardDenyClass::SystemContainer
                | HardDenyClass::UserProfileRoot
                | HardDenyClass::VolumeRoot
                | HardDenyClass::RecycleBinRoot
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            HardDenyClass::SystemRoot => "system_root",
            HardDenyClass::ProgramFiles => "program_files",
            HardDenyClass::ProgramDataRoot => "program_data_root",
            HardDenyClass::UsersRoot => "users_root",
            HardDenyClass::SystemContainer => "system_container",
            HardDenyClass::DefaultProfile => "default_profile",
            HardDenyClass::PublicProfile => "public_profile",
            HardDenyClass::UserProfileRoot => "user_profile_root",
            HardDenyClass::VolumeRoot => "volume_root",
            HardDenyClass::VolumeMetadata => "volume_metadata",
            HardDenyClass::RecycleBinRoot => "recycle_bin_root",
            HardDenyClass::Recovery => "recovery",
            HardDenyClass::InstallerState => "installer_state",
            HardDenyClass::SecuritySoftware => "security_software",
            HardDenyClass::Credentials => "credentials",
            HardDenyClass::ToolStore => "tool_store",
        }
    }
}

/// Protocol file identity (`volume_id`, `file_id`), as recorded by the scan backends.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ObjectIdentity {
    pub volume_id: String,
    pub file_id: String,
}

impl ObjectIdentity {
    pub fn new(volume_id: impl Into<String>, file_id: impl Into<String>) -> Self {
        ObjectIdentity { volume_id: volume_id.into(), file_id: file_id.into() }
    }
}

/// A platform hard-deny location resolved to its identity. `display` is for people only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardDenyRoot {
    pub class: HardDenyClass,
    pub volume_id: String,
    pub file_id: String,
    pub display: String,
}

impl HardDenyRoot {
    pub fn identity(&self) -> ObjectIdentity {
        ObjectIdentity::new(self.volume_id.clone(), self.file_id.clone())
    }
}

/// Hard-deny roots indexed by identity. When two roots share an identity (aliases), the one
/// that covers descendants wins, so an alias can never weaken a root.
#[derive(Debug, Clone, Default)]
pub struct HardDenySet {
    by_id: HashMap<ObjectIdentity, HardDenyRoot>,
}

impl HardDenySet {
    pub fn new(roots: impl IntoIterator<Item = HardDenyRoot>) -> Self {
        let mut set = HardDenySet::default();
        for r in roots {
            set.insert(r);
        }
        set
    }

    pub fn insert(&mut self, root: HardDenyRoot) {
        let id = root.identity();
        match self.by_id.get(&id) {
            Some(existing) if existing.class.covers_descendants() || !root.class.covers_descendants() => {}
            _ => {
                self.by_id.insert(id, root);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn roots(&self) -> impl Iterator<Item = &HardDenyRoot> {
        self.by_id.values()
    }

    /// The root that *is* this object (exact or subtree root).
    pub fn root_at(&self, id: &ObjectIdentity) -> Option<&HardDenyRoot> {
        self.by_id.get(id)
    }

    /// The subtree root this ancestor identity denies descendants of.
    pub fn covering(&self, ancestor: &ObjectIdentity) -> Option<&HardDenyRoot> {
        self.by_id.get(ancestor).filter(|r| r.class.covers_descendants())
    }
}

/// Real location of a link target, identified after resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    /// Final (real) path of the target.
    pub path: RawPath,
    pub object: ObjectIdentity,
    /// Identities of the target's ancestors up to its volume root (any order).
    pub ancestors: Vec<ObjectIdentity>,
}

/// A symlink, junction or mount point met on the object or one of its ancestors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkResolution {
    pub link: RawPath,
    /// The link is the object itself (not an ancestor). Its target's real path then also goes
    /// through the textual pre-filter; for ancestor links the object's own `real_path` does.
    pub on_object: bool,
    /// `Err` carries why resolution failed; a failure always denies.
    pub target: Result<ResolvedTarget, String>,
}

/// Everything the platform learned about a path's identity, without following the object
/// itself. `None` entries mean "could not be identified" and always deny.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IdentityChain {
    /// Identity of the object itself (links are not followed).
    pub object: Option<ObjectIdentity>,
    /// Real path of the object with ancestor links resolved (the object itself is not
    /// followed), when the platform could determine it. Checked by the textual pre-filter.
    pub real_path: Option<RawPath>,
    /// Identities of every ancestor (textual and real), any order.
    pub ancestors: Vec<Option<ObjectIdentity>>,
    /// Links on the object or its ancestors, with their resolution.
    pub links: Vec<LinkResolution>,
}

/// The hard-deny root this chain hits (object, covering ancestor, or link target), ignoring
/// identification failures. Used to show the denied real location and to reject user entries
/// that point into hard-deny areas.
pub fn hard_deny_hit<'h>(chain: &IdentityChain, hard: &'h HardDenySet) -> Option<&'h HardDenyRoot> {
    if let Some(r) = chain.object.as_ref().and_then(|o| hard.root_at(o)) {
        return Some(r);
    }
    if let Some(r) = chain.ancestors.iter().flatten().find_map(|a| hard.covering(a)) {
        return Some(r);
    }
    chain.links.iter().filter_map(|l| l.target.as_ref().ok()).find_map(|t| {
        hard.root_at(&t.object).or_else(|| t.ancestors.iter().find_map(|a| hard.covering(a)))
    })
}

/// Identity part of the hard-deny layer. Fails closed on any identification failure.
pub fn check_identity(chain: &IdentityChain, hard: &HardDenySet) -> ProtectionDecision {
    let deny = |r: String| ProtectionDecision::deny(DenyLayer::HardDeny, r);
    let Some(object) = &chain.object else {
        return deny("object identity could not be established".into());
    };
    if let Some(r) = hard.root_at(object) {
        return deny(format!("object is the hard-deny location {} ({})", r.display, r.class.as_str()));
    }
    for a in &chain.ancestors {
        match a {
            None => return deny("an ancestor could not be identified".into()),
            Some(a) => {
                if let Some(r) = hard.covering(a) {
                    return deny(format!("object is inside the hard-deny location {} ({})", r.display, r.class.as_str()));
                }
            }
        }
    }
    if let Some((real, r)) = chain.real_path.as_ref().and_then(|p| prefilter(p).err().map(|r| (p, r))) {
        return deny(format!("object's real location {}: {r}", real.display()));
    }
    for l in &chain.links {
        match &l.target {
            Err(e) => return deny(format!("link {} could not be resolved: {e}", l.link.display())),
            Ok(t) => {
                if let Some(r) = l.on_object.then(|| prefilter(&t.path).err()).flatten() {
                    return deny(format!("link {} resolves to {}: {r}", l.link.display(), t.path.display()));
                }
                if let Some(r) = hard.root_at(&t.object) {
                    return deny(format!("link {} resolves to the hard-deny location {} ({})", l.link.display(), r.display, r.class.as_str()));
                }
                if let Some(r) = t.ancestors.iter().find_map(|a| hard.covering(a)) {
                    return deny(format!("link {} resolves inside the hard-deny location {} ({})", l.link.display(), r.display, r.class.as_str()));
                }
            }
        }
    }
    ProtectionDecision::Allowed
}

/// Hard-deny layer: textual pre-filter, then identity comparison of the object, its
/// ancestors and every link target.
pub fn check_path(path: &RawPath, chain: &IdentityChain, hard: &HardDenySet) -> ProtectionDecision {
    if let Err(r) = prefilter(path) {
        return ProtectionDecision::deny(DenyLayer::HardDeny, format!("{}: {r}", path.display()));
    }
    check_identity(chain, hard)
}

/// A hard-deny root inside the subtree of `entry` (or `entry` itself), for whole-tree
/// removals: deleting a directory that contains a deny root is denied too.
pub fn hard_deny_within<'h>(view: &dyn IndexView, entry: u64, hard: &'h HardDenySet) -> Option<&'h HardDenyRoot> {
    hard.roots().find(|root| {
        let Some(mut e) = view.find_identity(&root.volume_id, &root.file_id) else { return false };
        loop {
            if e == entry {
                return true;
            }
            match view.parent(e) {
                Some(p) => e = p,
                None => return false,
            }
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Textual pre-filter
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TextualDeny {
    #[error("path is empty")]
    Empty,
    #[error("path contains NUL")]
    Nul,
    #[error("path contains a control character")]
    ControlCharacter,
    #[error("path contains an invisible or bidirectional formatting character")]
    FormatCharacter,
    #[error("path is not valid in its platform encoding")]
    EncodingAnomaly,
    #[error("path is too long")]
    TooLong,
    #[error("path is not absolute")]
    Relative,
    #[error("path contains a '.' or '..' component")]
    Traversal,
    #[error("path is not canonical: {0}")]
    NonCanonical(&'static str),
    #[error("device or object-manager path")]
    DevicePath,
    #[error("path contains a reserved device name")]
    ReservedName,
    #[error("path names an alternate data stream")]
    AlternateDataStream,
    #[error("path contains a character that is invalid in file names")]
    InvalidCharacter,
    #[error("administrative share")]
    AdminShare,
    #[error("network host or share root")]
    ShareRoot,
    #[error("volume root")]
    VolumeRoot,
    #[error("protected location: {0}")]
    ProtectedLocation(&'static str),
}

/// Reject obviously dangerous inputs before any OS call. Passing the pre-filter grants
/// nothing: the identity check still runs.
pub fn prefilter(path: &RawPath) -> Result<(), TextualDeny> {
    match path {
        RawPath::Windows(units) => prefilter_windows(units),
        RawPath::Unix(bytes) => prefilter_unix(bytes),
    }
}

fn is_format_char(c: char) -> bool {
    matches!(c as u32,
        0x00AD | 0x061C | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF)
}

fn check_chars(s: &str) -> Result<(), TextualDeny> {
    for c in s.chars() {
        if c == '\0' {
            return Err(TextualDeny::Nul);
        }
        if c.is_control() {
            return Err(TextualDeny::ControlCharacter);
        }
        if is_format_char(c) {
            return Err(TextualDeny::FormatCharacter);
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Exact,
    Subtree,
}

type Table = &'static [(&'static [&'static str], Scope, &'static str)];

fn table_hit(table: Table, comps: &[String]) -> Option<&'static str> {
    table.iter().find_map(|(pat, scope, reason)| {
        let fits = match scope {
            Scope::Exact => comps.len() == pat.len(),
            Scope::Subtree => comps.len() >= pat.len(),
        };
        (fits && pat.iter().zip(comps).all(|(p, c)| *p == "*" || p == c)).then_some(*reason)
    })
}

use Scope::{Exact, Subtree};

/// Volume-relative Windows locations (lower case). Applied to drive, volume-GUID and UNC paths.
const WINDOWS_LOCATIONS: Table = &[
    (&["windows"], Subtree, "Windows system directory"),
    (&["program files"], Subtree, "program installation root"),
    (&["program files (x86)"], Subtree, "program installation root"),
    (&["programdata"], Exact, "ProgramData root"),
    (&["programdata", "microsoft", "windows defender"], Subtree, "security software data"),
    (&["programdata", "microsoft", "windows defender advanced threat protection"], Subtree, "security software data"),
    (&["programdata", "microsoft", "crypto"], Subtree, "machine credentials"),
    (&["users"], Exact, "user profiles root"),
    (&["users", "default"], Subtree, "default user profile"),
    (&["users", "default user"], Subtree, "default user profile"),
    (&["users", "public"], Subtree, "public profile"),
    (&["users", "all users"], Subtree, "ProgramData alias"),
    (&["users", "*"], Exact, "user profile root"),
    (&["users", "*", "appdata"], Exact, "application data root"),
    (&["users", "*", "appdata", "*"], Exact, "application data root"),
    (&["users", "*", "appdata", "local", "cleanmanager"], Subtree, "CleanManager store"),
    (&["users", "*", "appdata", "roaming", "microsoft", "protect"], Subtree, "user credentials"),
    (&["users", "*", "appdata", "roaming", "microsoft", "credentials"], Subtree, "user credentials"),
    (&["users", "*", "appdata", "roaming", "microsoft", "systemcertificates"], Subtree, "user credentials"),
    (&["users", "*", "appdata", "roaming", "microsoft", "crypto"], Subtree, "user credentials"),
    (&["users", "*", "appdata", "roaming", "microsoft", "vault"], Subtree, "user credentials"),
    (&["users", "*", "appdata", "local", "microsoft", "credentials"], Subtree, "user credentials"),
    (&["users", "*", "appdata", "local", "microsoft", "vault"], Subtree, "user credentials"),
    (&["documents and settings"], Subtree, "legacy profiles alias"),
    (&["system volume information"], Subtree, "volume metadata"),
    (&["$recycle.bin"], Exact, "recycle bin root"),
    (&["$recycle.bin", "*"], Exact, "recycle bin root"),
    (&["recovery"], Subtree, "recovery environment"),
    (&["config.msi"], Subtree, "installer rollback data"),
    (&[".cleanmanager-quarantine"], Subtree, "CleanManager quarantine"),
    (&["boot"], Subtree, "boot files"),
    (&["efi"], Subtree, "boot files"),
    (&["pagefile.sys"], Exact, "system paging file"),
    (&["hiberfil.sys"], Exact, "system hibernation file"),
    (&["swapfile.sys"], Exact, "system swap file"),
    (&["bootmgr"], Exact, "boot files"),
    (&["bootnxt"], Exact, "boot files"),
];

const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "conin$", "conout$", "clock$", "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9",
    "com¹", "com²", "com³", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9", "lpt¹", "lpt²", "lpt³",
];

fn is_win_sep(c: char) -> bool {
    c == '\\' || c == '/'
}

/// Split a Windows path remainder into components. Non-verbatim paths accept both separators
/// and collapse empty components (as Win32 does); verbatim paths accept only `\` and reject
/// empty inner components.
fn win_components(rest: &str, verbatim: bool) -> Result<Vec<String>, TextualDeny> {
    let parts: Vec<&str> = if verbatim { rest.split('\\').collect() } else { rest.split(is_win_sep).collect() };
    let mut out = Vec::new();
    let last = parts.len().saturating_sub(1);
    for (i, p) in parts.into_iter().enumerate() {
        if p.is_empty() {
            if verbatim && i != 0 && i != last {
                return Err(TextualDeny::NonCanonical("empty component in a verbatim path"));
            }
            continue;
        }
        out.push(p.to_owned());
    }
    Ok(out)
}

fn check_win_component(c: &str) -> Result<(), TextualDeny> {
    if c == "." || c == ".." {
        return Err(TextualDeny::Traversal);
    }
    if c.contains(':') {
        return Err(TextualDeny::AlternateDataStream);
    }
    if c.chars().any(|ch| matches!(ch, '<' | '>' | '"' | '|' | '?' | '*')) {
        return Err(TextualDeny::InvalidCharacter);
    }
    if c.ends_with('.') || c.ends_with(' ') {
        return Err(TextualDeny::NonCanonical("trailing dot or space in a component"));
    }
    let lower = c.to_lowercase();
    let base = lower.split('.').next().unwrap_or("").trim_end_matches(' ');
    if RESERVED.contains(&base) {
        return Err(TextualDeny::ReservedName);
    }
    if lower.starts_with("ntuser.") || lower.starts_with("usrclass.dat") {
        return Err(TextualDeny::ProtectedLocation("user registry hive"));
    }
    Ok(())
}

enum WinRoot {
    Volume,
    Share,
}

/// Parse a Windows path into its root kind and volume-relative components.
fn parse_windows(s: &str) -> Result<(WinRoot, Vec<String>), TextualDeny> {
    let chars: Vec<char> = s.chars().take(4).collect();
    if s.starts_with(r"\??\") || s.starts_with("/??/") {
        return Err(TextualDeny::DevicePath);
    }
    if chars.len() >= 2 && is_win_sep(chars[0]) && is_win_sep(chars[1]) {
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            if rest.contains('/') {
                return Err(TextualDeny::NonCanonical("forward slash in a verbatim path"));
            }
            let lower = rest.to_ascii_lowercase();
            let rb = rest.as_bytes();
            if rb.len() >= 2 && rb[0].is_ascii_alphabetic() && rb[1] == b':' {
                let tail = &rest[2..];
                if !(tail.is_empty() || tail.starts_with('\\')) {
                    return Err(TextualDeny::Relative);
                }
                return Ok((WinRoot::Volume, win_components(tail, true)?));
            }
            if lower.starts_with(r"unc\") {
                return parse_unc(&rest[4..], true);
            }
            if lower.starts_with("volume{") {
                let end = rest.find('}').ok_or(TextualDeny::DevicePath)?;
                if !rest[7..end].chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
                    return Err(TextualDeny::DevicePath);
                }
                let tail = &rest[end + 1..];
                if !(tail.is_empty() || tail.starts_with('\\')) {
                    return Err(TextualDeny::DevicePath);
                }
                return Ok((WinRoot::Volume, win_components(tail, true)?));
            }
            return Err(TextualDeny::DevicePath);
        }
        if chars.len() >= 3 && (chars[2] == '.' || chars[2] == '?') && (chars.len() == 3 || is_win_sep(chars[3])) {
            return Err(TextualDeny::DevicePath);
        }
        return parse_unc(&s[2..], false);
    }
    let b = s.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        if b.len() == 2 || !is_win_sep(b[2] as char) {
            return Err(TextualDeny::Relative);
        }
        return Ok((WinRoot::Volume, win_components(&s[2..], false)?));
    }
    Err(TextualDeny::Relative)
}

fn parse_unc(rest: &str, verbatim: bool) -> Result<(WinRoot, Vec<String>), TextualDeny> {
    let comps = win_components(rest, verbatim)?;
    if comps.len() < 2 {
        return Err(TextualDeny::ShareRoot);
    }
    for c in &comps[..2] {
        if c == "." || c == ".." {
            return Err(TextualDeny::Traversal);
        }
        if c.chars().any(|ch| matches!(ch, ':' | '<' | '>' | '"' | '|' | '?' | '*')) {
            return Err(TextualDeny::InvalidCharacter);
        }
    }
    if comps[1].ends_with('$') {
        return Err(TextualDeny::AdminShare);
    }
    Ok((WinRoot::Share, comps[2..].to_vec()))
}

pub fn prefilter_windows(units: &[u16]) -> Result<(), TextualDeny> {
    if units.is_empty() {
        return Err(TextualDeny::Empty);
    }
    if units.len() > MAX_WINDOWS_UNITS {
        return Err(TextualDeny::TooLong);
    }
    if units.contains(&0) {
        return Err(TextualDeny::Nul);
    }
    let s = String::from_utf16(units).map_err(|_| TextualDeny::EncodingAnomaly)?;
    check_chars(&s)?;
    let (root, comps) = parse_windows(&s)?;
    for c in &comps {
        check_win_component(c)?;
    }
    if comps.is_empty() {
        return Err(match root {
            WinRoot::Volume => TextualDeny::VolumeRoot,
            WinRoot::Share => TextualDeny::ShareRoot,
        });
    }
    let lower: Vec<String> = comps.iter().map(|c| c.to_lowercase()).collect();
    if lower[0].starts_with('$') && lower[0] != "$recycle.bin" {
        return Err(TextualDeny::ProtectedLocation("NTFS metadata or system staging area"));
    }
    match table_hit(WINDOWS_LOCATIONS, &lower) {
        Some(reason) => Err(TextualDeny::ProtectedLocation(reason)),
        None => Ok(()),
    }
}

/// Unix (macOS first) locations below `/`, lower case (APFS is case-insensitive by default).
const UNIX_LOCATIONS: Table = &[
    (&["system"], Subtree, "operating system"),
    (&["bin"], Subtree, "system binaries"),
    (&["sbin"], Subtree, "system binaries"),
    (&["etc"], Subtree, "system configuration"),
    (&["dev"], Subtree, "devices"),
    (&["boot"], Subtree, "boot files"),
    (&["proc"], Subtree, "kernel interface"),
    (&["sys"], Subtree, "kernel interface"),
    (&["lib"], Subtree, "system libraries"),
    (&["lib64"], Subtree, "system libraries"),
    (&["network"], Subtree, "network volumes"),
    // `/.vol/<device>/<inode>[/name]` opens any object by file ID (volfs), so no other entry of
    // this table would ever see `/System` or `/private/var/db` in such a path, and its identity
    // chain lists volfs directories instead of the object's real ancestors.
    (&[".vol"], Subtree, "file-ID (volfs) namespace"),
    // `/.nofollow/...` and `/.resolve/<flags>/...` re-enter path resolution with options.
    (&[".nofollow"], Subtree, "path-resolution prefix"),
    (&[".resolve"], Subtree, "path-resolution prefix"),
    (&["private", "etc"], Subtree, "system configuration"),
    (&["private", "var"], Exact, "system data root"),
    (&["private", "var", "db"], Subtree, "system databases"),
    (&["private", "var", "root"], Subtree, "root home"),
    (&["private", "var", "vm"], Subtree, "virtual memory"),
    (&["private", "var", "folders"], Exact, "per-user temporary root"),
    (&["private", "var", "tmp"], Exact, "temporary root"),
    (&["private", "tmp"], Exact, "temporary root"),
    (&["var", "db"], Subtree, "system databases"),
    (&["var", "root"], Subtree, "root home"),
    (&["var", "vm"], Subtree, "virtual memory"),
    (&["var", "folders"], Exact, "per-user temporary root"),
    (&["var", "tmp"], Exact, "temporary root"),
    (&["library", "*"], Exact, "system library container"),
    (&["library", "keychains"], Subtree, "system keychains"),
    (&["library", "apple"], Subtree, "operating system"),
    (&["library", "application support", "com.apple.tcc"], Subtree, "privacy database"),
    (&["users", "*"], Exact, "user home"),
    (&["users", "*", "library"], Exact, "user library root"),
    (&["users", "*", "library", "*"], Exact, "user library container"),
    (&["users", "*", "library", "keychains"], Subtree, "user keychains"),
    (&["users", "*", "library", "application support", "com.apple.tcc"], Subtree, "privacy database"),
    (&["users", "*", "library", "application support", "cleanmanager"], Subtree, "CleanManager store"),
    (&["users", "*", ".trash"], Exact, "trash root"),
    (&["volumes", "*"], Exact, "volume root"),
    (&["volumes", "*", ".spotlight-v100"], Subtree, "volume metadata"),
    (&["volumes", "*", ".fseventsd"], Subtree, "volume metadata"),
    (&["volumes", "*", ".documentrevisions-v100"], Subtree, "volume metadata"),
    (&["volumes", "*", ".trashes"], Exact, "trash root"),
    (&["home", "*"], Exact, "user home"),
    (&["opt", "homebrew"], Exact, "package manager root"),
];

pub fn prefilter_unix(bytes: &[u8]) -> Result<(), TextualDeny> {
    if bytes.is_empty() {
        return Err(TextualDeny::Empty);
    }
    if bytes.len() > MAX_UNIX_BYTES {
        return Err(TextualDeny::TooLong);
    }
    if bytes.contains(&0) {
        return Err(TextualDeny::Nul);
    }
    if bytes.iter().any(|b| *b < 0x20 || *b == 0x7F) {
        return Err(TextualDeny::ControlCharacter);
    }
    let s = std::str::from_utf8(bytes).map_err(|_| TextualDeny::EncodingAnomaly)?;
    check_chars(s)?;
    let Some(body) = s.strip_prefix('/') else { return Err(TextualDeny::Relative) };
    let body = body.strip_suffix('/').unwrap_or(body);
    if body.is_empty() {
        return Err(TextualDeny::VolumeRoot);
    }
    let mut comps = Vec::new();
    for c in body.split('/') {
        if c.is_empty() {
            return Err(TextualDeny::NonCanonical("repeated separator"));
        }
        if c == "." || c == ".." {
            return Err(TextualDeny::Traversal);
        }
        comps.push(c.to_lowercase());
    }
    if comps.len() == 1 {
        return Err(TextualDeny::ProtectedLocation("root-level entry"));
    }
    if comps[0] == "usr" {
        let local = comps[1] == "local";
        if !local || comps.len() <= 3 {
            return Err(TextualDeny::ProtectedLocation(if local { "package root" } else { "system binaries" }));
        }
    }
    match table_hit(UNIX_LOCATIONS, &comps) {
        Some(reason) => Err(TextualDeny::ProtectedLocation(reason)),
        None => Ok(()),
    }
}

// ---------------------------------------------------------------------------------------------
// User protection list
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    /// The object at `path` only (and, for whole-tree removals, any container holding it).
    Exact,
    /// Objects whose path matches `glob`, and their subtrees.
    Glob,
    /// `path` and everything below it.
    Descendants,
    /// `path` and every directory above it.
    Ancestors,
    /// Every action proposed by rule `rule_id`.
    Rule,
    /// Maintenance task or startup item `task_id`.
    MaintenanceTask,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserEntry {
    pub id: String,
    #[serde(rename = "match")]
    pub kind: MatchKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// RFC 3339 UTC (`YYYY-MM-DDTHH:MM:SS[.fff]Z`).
    pub added_at: String,
}

impl UserEntry {
    fn bare(id: &str, kind: MatchKind, added_at: &str) -> Self {
        UserEntry { id: id.into(), kind, path: None, glob: None, rule_id: None, task_id: None, note: None, added_at: added_at.into() }
    }

    /// `kind` must be `Exact`, `Descendants` or `Ancestors`.
    pub fn path(id: &str, kind: MatchKind, path: &str, added_at: &str) -> Self {
        UserEntry { path: Some(path.into()), ..Self::bare(id, kind, added_at) }
    }

    pub fn glob(id: &str, glob: &str, added_at: &str) -> Self {
        UserEntry { glob: Some(glob.into()), ..Self::bare(id, MatchKind::Glob, added_at) }
    }

    pub fn rule(id: &str, rule_id: &str, added_at: &str) -> Self {
        UserEntry { rule_id: Some(rule_id.into()), ..Self::bare(id, MatchKind::Rule, added_at) }
    }

    pub fn task(id: &str, task_id: &str, added_at: &str) -> Self {
        UserEntry { task_id: Some(task_id.into()), ..Self::bare(id, MatchKind::MaintenanceTask, added_at) }
    }

    pub fn with_note(mut self, note: &str) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// Why a single entry is rejected (argument error when adding; protection_unavailable when
/// found in the stored file).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EntryError {
    #[error("entry id must be 1-64 characters of [A-Za-z0-9._-]")]
    InvalidId,
    #[error("entry id {0:?} is used twice")]
    DuplicateId(String),
    #[error("match kind requires field {0}")]
    MissingField(&'static str),
    #[error("field {0} is not allowed for this match kind")]
    UnexpectedField(&'static str),
    #[error("path is rejected: {0}")]
    PathRejected(TextualDeny),
    #[error("glob is invalid: {0}")]
    InvalidGlob(&'static str),
    #[error("{0} must be a non-empty identifier without whitespace or control characters")]
    InvalidIdentifier(&'static str),
    #[error("note is too long or contains control characters")]
    InvalidNote,
    #[error("added_at must be an RFC 3339 UTC timestamp")]
    InvalidTimestamp,
    #[error("entry points into the hard-deny location {0}; it is already protected and cannot be changed")]
    TargetsHardDeny(String),
}

impl EntryError {
    pub fn code(&self) -> ErrorCode {
        ErrorCode::InvalidArgument
    }
}

/// The protection list could not be used. Callers MUST stop every write; reads continue.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtectionUnavailable {
    #[error("protection file could not be read: {0}")]
    Unreadable(String),
    #[error("protection file is not valid: {0}")]
    Malformed(String),
    #[error("protection file schema_version {found} is not supported (this build reads {expected})")]
    UnsupportedVersion { found: String, expected: u32 },
    #[error("protection entry {id:?} is invalid: {problem}")]
    InvalidEntry { id: String, problem: EntryError },
}

impl ProtectionUnavailable {
    pub fn code(&self) -> ErrorCode {
        ErrorCode::ProtectionUnavailable
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    root: String,
    comps: Vec<String>,
}

impl Key {
    fn is_prefix_of(&self, other: &Key) -> bool {
        self.root == other.root && self.comps.len() <= other.comps.len() && self.comps[..] == other.comps[..self.comps.len()]
    }
}

/// Case-folded comparison key. Folding only ever adds matches, i.e. adds protection.
fn key_of(text: &str, platform: Platform) -> Option<Key> {
    match platform {
        Platform::Windows => {
            let mut s = text.replace('/', "\\");
            if let Some(rest) = s.strip_prefix(r"\\?\") {
                s = if rest.len() >= 4 && rest[..4].eq_ignore_ascii_case(r"unc\") { format!(r"\\{}", &rest[4..]) } else { rest.to_owned() };
            }
            let s = s.to_lowercase();
            if let Some(rest) = s.strip_prefix(r"\\") {
                let mut parts = rest.split('\\').filter(|p| !p.is_empty());
                let (host, share) = (parts.next()?, parts.next()?);
                return Some(Key { root: format!(r"\\{host}\{share}"), comps: parts.map(str::to_owned).collect() });
            }
            let b = s.as_bytes();
            if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b.len() == 2 || b[2] == b'\\') {
                return Some(Key { root: s[..2].to_owned(), comps: s[2..].split('\\').filter(|p| !p.is_empty()).map(str::to_owned).collect() });
            }
            if s.starts_with("volume{") {
                let end = s.find('}')?;
                return Some(Key { root: s[..=end].to_owned(), comps: s[end + 1..].split('\\').filter(|p| !p.is_empty()).map(str::to_owned).collect() });
            }
            None
        }
        Platform::Macos => {
            let rest = text.strip_prefix('/')?;
            Some(Key { root: "/".into(), comps: rest.split('/').filter(|p| !p.is_empty()).map(str::to_lowercase).collect() })
        }
    }
}

fn raw_of(text: &str, platform: Platform) -> Option<RawPath> {
    match platform {
        Platform::Windows => RawPath::windows(text.encode_utf16().collect()).ok(),
        Platform::Macos => RawPath::unix(text.as_bytes().to_vec()).ok(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Compiled {
    Path(MatchKind, Key),
    Glob(Key),
    Rule(String),
    Task(String),
}

fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn valid_identifier(s: &str) -> bool {
    (1..=128).contains(&s.chars().count()) && !s.chars().any(|c| c.is_whitespace() || c.is_control() || is_format_char(c))
}

fn valid_timestamp(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 20 || b[b.len() - 1] != b'Z' {
        return false;
    }
    let shape = b[..19].iter().enumerate().all(|(i, c)| match i {
        4 | 7 => *c == b'-',
        10 => *c == b'T',
        13 | 16 => *c == b':',
        _ => c.is_ascii_digit(),
    });
    let frac = &b[19..b.len() - 1];
    shape && (frac.is_empty() || (frac.len() >= 2 && frac[0] == b'.' && frac[1..].iter().all(u8::is_ascii_digit)))
}

fn compile_glob(glob: &str, platform: Platform) -> Result<Key, EntryError> {
    check_chars(glob).map_err(|_| EntryError::InvalidGlob("control or formatting character"))?;
    let key = key_of(glob, platform).ok_or(EntryError::InvalidGlob("must start with an absolute root"))?;
    if key.root.contains(['*', '?']) {
        return Err(EntryError::InvalidGlob("the root (drive, share, '/') must be literal"));
    }
    if key.comps.len() > 64 {
        return Err(EntryError::InvalidGlob("too many segments"));
    }
    let mut doubles = 0;
    for seg in &key.comps {
        if seg == "." || seg == ".." {
            return Err(EntryError::InvalidGlob("'.' and '..' segments are not allowed"));
        }
        if seg == "**" {
            doubles += 1;
        } else if seg.contains("**") {
            return Err(EntryError::InvalidGlob("'**' must be a whole segment"));
        }
        if platform == Platform::Windows && seg.chars().any(|c| matches!(c, ':' | '<' | '>' | '"' | '|')) {
            return Err(EntryError::InvalidGlob("character not allowed in Windows names"));
        }
    }
    if doubles > 4 {
        return Err(EntryError::InvalidGlob("at most four '**' segments"));
    }
    Ok(key)
}

/// Validate one entry and compile its matcher.
fn compile(entry: &UserEntry, platform: Platform) -> Result<Compiled, EntryError> {
    if !valid_id(&entry.id) {
        return Err(EntryError::InvalidId);
    }
    if !valid_timestamp(&entry.added_at) {
        return Err(EntryError::InvalidTimestamp);
    }
    if entry.note.as_deref().is_some_and(|n| n.chars().count() > 1024 || n.chars().any(|c| c.is_control() || is_format_char(c))) {
        return Err(EntryError::InvalidNote);
    }
    let wants = match entry.kind {
        MatchKind::Exact | MatchKind::Descendants | MatchKind::Ancestors => "path",
        MatchKind::Glob => "glob",
        MatchKind::Rule => "rule_id",
        MatchKind::MaintenanceTask => "task_id",
    };
    for (name, present) in
        [("path", entry.path.is_some()), ("glob", entry.glob.is_some()), ("rule_id", entry.rule_id.is_some()), ("task_id", entry.task_id.is_some())]
    {
        if present && name != wants {
            return Err(EntryError::UnexpectedField(name));
        }
    }
    match entry.kind {
        MatchKind::Exact | MatchKind::Descendants | MatchKind::Ancestors => {
            let path = entry.path.as_deref().ok_or(EntryError::MissingField("path"))?;
            let raw = raw_of(path, platform).ok_or(EntryError::PathRejected(TextualDeny::Empty))?;
            prefilter(&raw).map_err(EntryError::PathRejected)?;
            let key = key_of(path, platform).ok_or(EntryError::PathRejected(TextualDeny::Relative))?;
            Ok(Compiled::Path(entry.kind, key))
        }
        MatchKind::Glob => Ok(Compiled::Glob(compile_glob(entry.glob.as_deref().ok_or(EntryError::MissingField("glob"))?, platform)?)),
        MatchKind::Rule => {
            let r = entry.rule_id.as_deref().ok_or(EntryError::MissingField("rule_id"))?;
            if !valid_identifier(r) {
                return Err(EntryError::InvalidIdentifier("rule_id"));
            }
            Ok(Compiled::Rule(r.to_owned()))
        }
        MatchKind::MaintenanceTask => {
            let t = entry.task_id.as_deref().ok_or(EntryError::MissingField("task_id"))?;
            if !valid_identifier(t) {
                return Err(EntryError::InvalidIdentifier("task_id"));
            }
            Ok(Compiled::Task(t.to_owned()))
        }
    }
}

fn glob_full(segs: &[String], comps: &[String]) -> bool {
    match segs.split_first() {
        None => comps.is_empty(),
        Some((s, rest)) if s == "**" => (0..=comps.len()).any(|k| glob_full(rest, &comps[k..])),
        Some((s, rest)) => comps.split_first().is_some_and(|(c, more)| glob_match(s, c, false) && glob_full(rest, more)),
    }
}

/// Whether some descendant of `comps` could match (used for whole-tree removals).
fn glob_partial(segs: &[String], comps: &[String]) -> bool {
    match (segs.split_first(), comps.split_first()) {
        (_, None) => true,
        (None, Some(_)) => false,
        (Some((s, _)), Some(_)) if s == "**" => true,
        (Some((s, rest)), Some((c, more))) => glob_match(s, c, false) && glob_partial(rest, more),
    }
}

fn compiled_matches(c: &Compiled, subj: &Key, recursive: bool) -> bool {
    match c {
        Compiled::Path(MatchKind::Exact, k) => subj == k || (recursive && subj.is_prefix_of(k)),
        Compiled::Path(MatchKind::Descendants, k) => k.is_prefix_of(subj) || (recursive && subj.is_prefix_of(k)),
        Compiled::Path(_, k) => subj.is_prefix_of(k),
        Compiled::Glob(g) => {
            g.root == subj.root
                && ((0..=subj.comps.len()).any(|i| glob_full(&g.comps, &subj.comps[..i])) || (recursive && glob_partial(&g.comps, &subj.comps)))
        }
        Compiled::Rule(_) | Compiled::Task(_) => false,
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtectionFile {
    schema_version: u32,
    entries: Vec<UserEntry>,
}

#[derive(Deserialize)]
struct VersionProbe {
    schema_version: Option<serde_json::Value>,
}

/// Validated user protection list for one platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserProtection {
    platform: Platform,
    entries: Vec<UserEntry>,
    compiled: Vec<Compiled>,
}

fn host_platform() -> Platform {
    if cfg!(windows) { Platform::Windows } else { Platform::Macos }
}

/// Parse and validate the stored protection file for the host platform.
pub fn load_protection_file(text: &str) -> Result<UserProtection, ProtectionUnavailable> {
    load_protection_file_for(text, host_platform())
}

/// Parse and validate a protection file. Any syntax, version or entry error makes the whole
/// list unavailable (never silently ignored).
pub fn load_protection_file_for(text: &str, platform: Platform) -> Result<UserProtection, ProtectionUnavailable> {
    let probe: VersionProbe = serde_json::from_str(text).map_err(|e| ProtectionUnavailable::Malformed(e.to_string()))?;
    match probe.schema_version {
        Some(v) if v.as_u64() == Some(u64::from(PROTECTION_FILE_SCHEMA_VERSION)) => {}
        other => {
            return Err(ProtectionUnavailable::UnsupportedVersion {
                found: other.map_or_else(|| "missing".to_owned(), |v| v.to_string()),
                expected: PROTECTION_FILE_SCHEMA_VERSION,
            });
        }
    }
    let file: ProtectionFile = serde_json::from_str(text).map_err(|e| ProtectionUnavailable::Malformed(e.to_string()))?;
    let mut list = UserProtection::empty(platform);
    for entry in file.entries {
        let id = entry.id.clone();
        list.add(entry).map_err(|problem| ProtectionUnavailable::InvalidEntry { id, problem })?;
    }
    Ok(list)
}

impl UserProtection {
    pub fn empty(platform: Platform) -> Self {
        UserProtection { platform, entries: Vec::new(), compiled: Vec::new() }
    }

    pub fn platform(&self) -> Platform {
        self.platform
    }

    pub fn entries(&self) -> &[UserEntry] {
        &self.entries
    }

    /// Validate an entry against this list's platform without adding it.
    pub fn validate(&self, entry: &UserEntry) -> Result<(), EntryError> {
        if self.entries.iter().any(|e| e.id == entry.id) {
            return Err(EntryError::DuplicateId(entry.id.clone()));
        }
        compile(entry, self.platform).map(|_| ())
    }

    /// Add a validated entry. Entries only ever add protection.
    pub fn add(&mut self, entry: UserEntry) -> Result<(), EntryError> {
        if self.entries.iter().any(|e| e.id == entry.id) {
            return Err(EntryError::DuplicateId(entry.id));
        }
        let c = compile(&entry, self.platform)?;
        self.entries.push(entry);
        self.compiled.push(c);
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> Option<UserEntry> {
        let i = self.entries.iter().position(|e| e.id == id)?;
        self.compiled.remove(i);
        Some(self.entries.remove(i))
    }

    /// Canonical file text (pretty JSON + newline). The platform writes it atomically
    /// (temporary file + rename).
    pub fn serialize(&self) -> String {
        let file = ProtectionFile { schema_version: PROTECTION_FILE_SCHEMA_VERSION, entries: self.entries.clone() };
        let mut s = serde_json::to_string_pretty(&file).expect("protection file serializes");
        s.push('\n');
        s
    }

    /// First path or glob entry protecting `path`. `recursive` = the action removes the whole
    /// subtree, so containers of protected objects are protected too. Callers run the hard-deny
    /// check first; a path that cannot be keyed here is denied there.
    pub fn match_path(&self, path: &RawPath, recursive: bool) -> Option<&UserEntry> {
        let subj = key_of(&path.lossy(), self.platform)?;
        self.compiled.iter().position(|c| compiled_matches(c, &subj, recursive)).map(|i| &self.entries[i])
    }

    pub fn protects_rule(&self, rule_id: &str) -> Option<&UserEntry> {
        self.compiled.iter().position(|c| matches!(c, Compiled::Rule(r) if r == rule_id)).map(|i| &self.entries[i])
    }

    pub fn protects_task(&self, task_id: &str) -> Option<&UserEntry> {
        self.compiled.iter().position(|c| matches!(c, Compiled::Task(t) if t == task_id)).map(|i| &self.entries[i])
    }
}

/// Reject a new path entry that resolves into a hard-deny location (PRT-02): it would be
/// redundant and suggests the user expects to change hard-deny behavior. Entries for paths
/// that do not exist yet are accepted.
pub fn check_entry_identity(chain: &IdentityChain, hard: &HardDenySet) -> Result<(), EntryError> {
    match hard_deny_hit(chain, hard) {
        Some(r) => Err(EntryError::TargetsHardDeny(r.display.clone())),
        None => Ok(()),
    }
}

// ---------------------------------------------------------------------------------------------
// Layered evaluation
// ---------------------------------------------------------------------------------------------

/// One object an action would touch.
#[derive(Debug, Clone)]
pub struct Subject<'a> {
    pub path: &'a RawPath,
    pub chain: &'a IdentityChain,
    /// Built-in class from analysis (`protection_by_name` / protected roots), if any.
    pub default_class: Option<ProtectionClass>,
    /// Rule that proposed the action, if any.
    pub rule_id: Option<&'a str>,
    /// The action removes the whole subtree.
    pub recursive: bool,
}

/// Default-class layer alone.
pub fn check_default_class(subject: &Subject<'_>) -> ProtectionDecision {
    match subject.default_class {
        Some(c) => ProtectionDecision::deny(DenyLayer::DefaultClass, format!("default protection class {}", c.as_str())),
        None => ProtectionDecision::Allowed,
    }
}

/// User-list layer alone, with the entry that matched (path, link target, then rule).
pub fn check_user<'u>(subject: &Subject<'_>, user: &'u UserProtection) -> (ProtectionDecision, Option<&'u UserEntry>) {
    let user_deny = |e: &'u UserEntry, what: String| (ProtectionDecision::deny(DenyLayer::User, format!("{what} by user entry {}", e.id)), Some(e));
    if let Some(e) = user.match_path(subject.path, subject.recursive) {
        return user_deny(e, "protected".into());
    }
    for t in subject.chain.links.iter().filter_map(|l| l.target.as_ref().ok()) {
        if let Some(e) = user.match_path(&t.path, subject.recursive) {
            return user_deny(e, format!("link target {} is protected", t.path.display()));
        }
    }
    if let Some(e) = subject.rule_id.and_then(|r| user.protects_rule(r)) {
        return user_deny(e, "rule is protected".into());
    }
    (ProtectionDecision::Allowed, None)
}

/// Hard deny > default classes > user list. Nothing in a lower layer can lift a higher one.
pub fn evaluate(subject: &Subject<'_>, hard: &HardDenySet, user: &UserProtection) -> ProtectionDecision {
    explain(subject, hard, user).into_iter().map(|l| l.decision).find(ProtectionDecision::is_denied).unwrap_or(ProtectionDecision::Allowed)
}

/// One layer's own verdict, for explanations (`protect check`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerVerdict {
    pub layer: DenyLayer,
    pub decision: ProtectionDecision,
    /// User entry that matched (user layer only).
    pub entry_id: Option<String>,
}

/// Every layer's verdict, strongest first. The overall decision is the first denial; every
/// layer is still evaluated so the explanation shows all reasons that apply.
pub fn explain(subject: &Subject<'_>, hard: &HardDenySet, user: &UserProtection) -> Vec<LayerVerdict> {
    let (decision, entry) = check_user(subject, user);
    vec![
        LayerVerdict { layer: DenyLayer::HardDeny, decision: check_path(subject.path, subject.chain, hard), entry_id: None },
        LayerVerdict { layer: DenyLayer::DefaultClass, decision: check_default_class(subject), entry_id: None },
        LayerVerdict { layer: DenyLayer::User, decision, entry_id: entry.map(|e| e.id.clone()) },
    ]
}

/// Maintenance tasks and startup items (PRT-06): a protected task is reported as skipped.
pub fn evaluate_task(task_id: &str, user: &UserProtection) -> ProtectionDecision {
    match user.protects_task(task_id) {
        Some(e) => ProtectionDecision::deny(DenyLayer::User, format!("task protected by user entry {}", e.id)),
        None => ProtectionDecision::Allowed,
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod corpus_tests;
