//! Installer and disk-image cleanup (INC-01..05): leftover installers in declared locations,
//! recognized by content, labelled by source and signer, always review-only. Produces an
//! ordinary [`CandidateSet`]; plans are identity-bound file plans of the shared pipeline.
//!
//! Recognition never trusts a name: a PE file needs an installer-engine signature, an OLE file
//! the MSI/MSP class id, a ZIP an `AppxManifest.xml` or installer payload among its first
//! entries (encrypted or unlistable archives are never candidates), images their volume or
//! footer signatures. Only bounded byte ranges are read, through a [`ByteSource`] supplied by
//! the platform, which never opens cloud placeholders.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::analysis::{AnalysisProtection, Candidate, CandidateSet, CandidateState, IndexView, PROTECTION_VERSION, RuleRef, UserList};
use crate::bytes::{Bytes, KnownTotal};
use crate::c14n::digest;
use crate::catalog::{RegistryStatus, TierInputs, compute_tier};
use crate::model::{CloudState, ObjectKind};
use crate::path::RawPath;
use crate::protect::{HARD_DENY_VERSION, ObjectIdentity, prefilter};
use crate::rules::{Mode, Tier};

pub const CATALOG_VERSION: &str = "1.0.0";
pub const RULE_PREFIX: &str = "installer.";
/// Depth below each declared location (1 = files directly inside it).
pub const DEFAULT_MAX_DEPTH: u32 = 2;
pub const MAX_DEPTH_LIMIT: u32 = 4;
/// Archive entries inspected for installer payload (INC-02).
pub const ARCHIVE_ENTRY_LIMIT: usize = 256;
const PE_SCAN_BYTES: usize = 2 << 20;
const RSRC_SCAN_BYTES: usize = 4 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallerKind {
    Exe,
    Msi,
    Msp,
    Msix,
    MsixBundle,
    Archive,
    Iso,
    Vhd,
    Vhdx,
    Dmg,
    Pkg,
}

impl InstallerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            InstallerKind::Exe => "exe",
            InstallerKind::Msi => "msi",
            InstallerKind::Msp => "msp",
            InstallerKind::Msix => "msix",
            InstallerKind::MsixBundle => "msix_bundle",
            InstallerKind::Archive => "archive",
            InstallerKind::Iso => "iso",
            InstallerKind::Vhd => "vhd",
            InstallerKind::Vhdx => "vhdx",
            InstallerKind::Dmg => "dmg",
            InstallerKind::Pkg => "pkg",
        }
    }

    /// Disk images can be attached (mounted); those are skipped with eject guidance (INC-04).
    pub fn is_image(self) -> bool {
        matches!(self, InstallerKind::Iso | InstallerKind::Vhd | InstallerKind::Vhdx | InstallerKind::Dmg)
    }

    /// Formats that carry an Authenticode signature on Windows.
    pub fn signable(self) -> bool {
        matches!(self, InstallerKind::Exe | InstallerKind::Msi | InstallerKind::Msp | InstallerKind::Msix | InstallerKind::MsixBundle)
    }

    pub fn rule_id(self) -> String {
        format!("{RULE_PREFIX}{}", self.as_str().replace('_', "-"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "verdict")]
pub enum Verdict {
    Installer { kind: InstallerKind, evidence: String },
    NotInstaller { reason: String },
}

fn not(reason: impl Into<String>) -> Verdict {
    Verdict::NotInstaller { reason: reason.into() }
}

fn yes(kind: InstallerKind, evidence: impl Into<String>) -> Verdict {
    Verdict::Installer { kind, evidence: evidence.into() }
}

/// Random-access reads of one file (bounded; may return fewer bytes at the end of the file).
pub trait ByteSource {
    fn len(&self) -> u64;
    fn read_at(&self, offset: u64, len: usize) -> Option<Vec<u8>>;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl ByteSource for Vec<u8> {
    fn len(&self) -> u64 {
        Vec::len(self) as u64
    }
    fn read_at(&self, offset: u64, len: usize) -> Option<Vec<u8>> {
        let start = usize::try_from(offset).ok().filter(|s| *s <= Vec::len(self))?;
        Some(self[start..start.saturating_add(len).min(Vec::len(self))].to_vec())
    }
}

fn u16le(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32le(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64le(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn find(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

/// Installer-engine markers searched (ASCII and UTF-16LE) in the PE head and resources.
const PE_MARKERS: &[(&str, &str)] = &[
    ("Inno Setup", "inno_setup"),
    ("Nullsoft Install System", "nsis"),
    ("NullsoftInst", "nsis"),
    ("InstallShield", "installshield"),
    ("Advanced Installer", "advanced_installer"),
    ("Setup Factory", "setup_factory"),
    ("Wise Installation", "wise"),
    ("WixBundleManifest", "wix_burn"),
];

fn classify_pe(src: &dyn ByteSource, head: &[u8]) -> Verdict {
    let Some(lfanew) = u32le(head, 0x3c).map(u64::from) else { return not("truncated executable header") };
    let Some(hdr) = src.read_at(lfanew, 24) else { return not("truncated executable header") };
    if hdr.get(..4) != Some(b"PE\0\0") {
        return not("MZ file without a PE header");
    }
    let (Some(nsec), Some(opt)) = (u16le(&hdr, 6), u16le(&hdr, 20)) else { return not("truncated PE header") };
    let table = lfanew + 24 + u64::from(opt);
    let Some(sections) = src.read_at(table, usize::from(nsec.min(96)) * 40) else { return not("truncated section table") };
    let mut overlay = 0u64;
    let mut rsrc = None;
    for s in sections.chunks_exact(40) {
        let name = &s[..8];
        let (size, ptr) = (u64::from(u32le(s, 16).unwrap_or(0)), u64::from(u32le(s, 20).unwrap_or(0)));
        overlay = overlay.max(ptr + size);
        if name.starts_with(b".wixburn") {
            return yes(InstallerKind::Exe, "wix_burn: .wixburn section");
        }
        if name.starts_with(b".rsrc") {
            rsrc = Some((ptr, size));
        }
    }
    if overlay > 0 && overlay < src.len() {
        if let Some(o) = src.read_at(overlay, 32) {
            if o.get(4..8) == Some(&[0xef, 0xbe, 0xad, 0xde]) && o.get(8..20) == Some(b"NullsoftInst") {
                return yes(InstallerKind::Exe, "nsis: installer data header after the image");
            }
            if o.starts_with(&[0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c]) {
                return yes(InstallerKind::Exe, "7zip_sfx: self-extracting setup archive after the image");
            }
        }
    }
    let mut regions = vec![src.read_at(0, PE_SCAN_BYTES).unwrap_or_default()];
    if let Some((ptr, size)) = rsrc {
        regions.push(src.read_at(ptr, usize::try_from(size).unwrap_or(usize::MAX).min(RSRC_SCAN_BYTES)).unwrap_or_default());
    }
    for (marker, engine) in PE_MARKERS {
        let (ascii, wide) = (marker.as_bytes(), utf16(marker));
        if regions.iter().any(|r| find(r, ascii) || find(r, &wide)) {
            return yes(InstallerKind::Exe, format!("{engine}: \"{marker}\" marker"));
        }
    }
    not("executable without installer-engine markers")
}

const CFB_MAGIC: [u8; 8] = [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1];
/// Root-storage class ids: {000C1084-...} installer package, {000C1086-...} patch package.
const CLSID_MSI: [u8; 16] = [0x84, 0x10, 0x0c, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46];
const CLSID_MSP: [u8; 16] = [0x86, 0x10, 0x0c, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46];

fn classify_cfb(src: &dyn ByteSource, head: &[u8]) -> Verdict {
    let (Some(shift), Some(first_dir)) = (u16le(head, 0x1e), u32le(head, 0x30)) else { return not("truncated compound file") };
    if !(7..=16).contains(&shift) {
        return not("malformed compound file");
    }
    let Some(root) = src.read_at((u64::from(first_dir) + 1) << shift, 128) else { return not("truncated compound file") };
    match root.get(0x50..0x60) {
        Some(c) if c == CLSID_MSI => yes(InstallerKind::Msi, "msi: Windows Installer package class id"),
        Some(c) if c == CLSID_MSP => yes(InstallerKind::Msp, "msp: Windows Installer patch class id"),
        _ => not("compound document that is not a Windows Installer package"),
    }
}

const PAYLOAD_EXT: &[&str] = &["msi", "msix", "msixbundle", "appx", "appxbundle", "iso", "vhd", "vhdx", "dmg", "pkg", "mpkg"];

fn classify_zip(src: &dyn ByteSource) -> Verdict {
    let len = src.len();
    let tail_len = len.min(65_557);
    let Some(tail) = src.read_at(len - tail_len, tail_len as usize) else { return not("unreadable archive") };
    let Some(eocd) = tail.windows(4).rposition(|w| w == b"PK\x05\x06") else { return not("archive directory not found") };
    let (mut entries, mut cd_size, mut cd_off) = (
        u64::from(u16le(&tail, eocd + 10).unwrap_or(0)),
        u64::from(u32le(&tail, eocd + 12).unwrap_or(0)),
        u64::from(u32le(&tail, eocd + 16).unwrap_or(0)),
    );
    if (entries == 0xffff || cd_off == 0xffff_ffff) && eocd >= 20 && tail.get(eocd - 20..eocd - 16) == Some(b"PK\x06\x07") {
        let Some(z64) = u64le(&tail, eocd - 12).and_then(|o| src.read_at(o, 56)) else { return not("unreadable zip64 directory") };
        if z64.get(..4) != Some(b"PK\x06\x06") {
            return not("malformed zip64 directory");
        }
        (entries, cd_size, cd_off) = (u64le(&z64, 32).unwrap_or(0), u64le(&z64, 40).unwrap_or(0), u64le(&z64, 48).unwrap_or(0));
    }
    let Some(cd) = src.read_at(cd_off, usize::try_from(cd_size).unwrap_or(usize::MAX).min(4 << 20)) else { return not("unreadable archive directory") };
    let mut at = 0usize;
    let mut payload = Vec::new();
    let (mut appx, mut bundle) = (false, false);
    for _ in 0..entries.min(ARCHIVE_ENTRY_LIMIT as u64) {
        if cd.get(at..at + 4) != Some(b"PK\x01\x02") {
            return not("malformed archive directory");
        }
        let (Some(flags), Some(n), Some(x), Some(c)) = (u16le(&cd, at + 8), u16le(&cd, at + 28), u16le(&cd, at + 30), u16le(&cd, at + 32)) else {
            return not("truncated archive directory");
        };
        if flags & 1 != 0 {
            return not("encrypted archive: its contents cannot be verified");
        }
        let Some(name) = cd.get(at + 46..at + 46 + usize::from(n)) else { return not("truncated archive directory") };
        let name = String::from_utf8_lossy(name).replace('\\', "/");
        let lower = name.to_ascii_lowercase();
        if lower == "appxmanifest.xml" {
            appx = true;
        } else if lower == "appxmetadata/appxbundlemanifest.xml" {
            bundle = true;
        } else if !lower.starts_with("__macosx/") && lower.rsplit_once('.').is_some_and(|(_, e)| PAYLOAD_EXT.contains(&e)) {
            payload.push(name);
        }
        at += 46 + usize::from(n) + usize::from(x) + usize::from(c);
    }
    if bundle {
        yes(InstallerKind::MsixBundle, "msix_bundle: AppxBundleManifest.xml")
    } else if appx {
        yes(InstallerKind::Msix, "msix: AppxManifest.xml")
    } else if let Some(first) = payload.first() {
        yes(InstallerKind::Archive, format!("archive with installer payload ({} item(s), e.g. {first})", payload.len()))
    } else {
        not(format!("archive without installer payload in its first {} entries", entries.min(ARCHIVE_ENTRY_LIMIT as u64)))
    }
}

/// Recognize an installer or disk image by content only.
pub fn classify(src: &dyn ByteSource) -> Verdict {
    let len = src.len();
    if len < 64 {
        return not("too small");
    }
    let Some(head) = src.read_at(0, 4096) else { return not("unreadable") };
    if head.starts_with(b"MZ") {
        return classify_pe(src, &head);
    }
    if head.starts_with(&CFB_MAGIC) {
        return classify_cfb(src, &head);
    }
    if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        return classify_zip(src);
    }
    if head.starts_with(b"vhdxfile") {
        return yes(InstallerKind::Vhdx, "vhdx: file signature");
    }
    if head.starts_with(b"xar!") {
        return yes(InstallerKind::Pkg, "pkg: xar archive");
    }
    if head.starts_with(b"Rar!\x1a\x07") || head.starts_with(&[0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c]) {
        return not("archive format whose contents are not listed");
    }
    let footer = if len >= 512 { src.read_at(len - 512, 8).unwrap_or_default() } else { vec![] };
    if head.starts_with(b"conectix") || footer.starts_with(b"conectix") {
        return yes(InstallerKind::Vhd, "vhd: conectix footer");
    }
    if footer.starts_with(b"koly") {
        return yes(InstallerKind::Dmg, "dmg: koly trailer");
    }
    if len > 0x9006 {
        for at in [0x8001u64, 0x8801, 0x9001] {
            let id = src.read_at(at, 5).unwrap_or_default();
            if id == b"CD001" || id == b"BEA01" || id == b"NSR02" || id == b"NSR03" {
                return yes(InstallerKind::Iso, format!("iso: {} volume descriptor", String::from_utf8_lossy(&id)));
            }
        }
    }
    not("no installer or image signature")
}

// ---------------------------------------------------------------------------------------------
// Declared locations and hints

/// Chromium-family `Preferences`: the custom download directory, if one is set.
pub fn chromium_download_dir(prefs_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(prefs_json).ok()?;
    v["download"]["default_directory"].as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

/// Firefox `prefs.js`: `browser.download.dir` when downloads go to a custom folder.
pub fn firefox_download_dir(prefs_js: &str) -> Option<String> {
    let custom = prefs_js.lines().any(|l| l.trim_start().starts_with("user_pref(\"browser.download.folderList\", 2)"));
    let line = prefs_js.lines().find(|l| l.trim_start().starts_with("user_pref(\"browser.download.dir\","))?;
    let value = line.trim_start().strip_prefix("user_pref(\"browser.download.dir\",")?.trim().strip_suffix(");")?.trim();
    custom.then(|| serde_json::from_str::<String>(value).ok()).flatten().filter(|s| !s.is_empty())
}

const ORG_SUFFIXES: &[&str] = &["inc", "llc", "ltd", "limited", "corp", "corporation", "co", "company", "gmbh", "ag", "sa", "bv", "pty", "the", "srl", "sas"];
const GENERIC: &[&str] = &[
    "setup", "installer", "install", "x64", "x86", "amd64", "arm64", "win", "windows", "win64", "win32", "release", "stable", "latest", "user", "online",
    "offline", "full", "version", "update", "client", "desktop", "portable", "community", "edition", "free", "app",
];

fn org(s: &str) -> String {
    let words: Vec<String> = s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_owned).collect();
    let mut keep: Vec<&str> = words.iter().map(String::as_str).collect();
    while keep.last().is_some_and(|w| ORG_SUFFIXES.contains(w)) {
        keep.pop();
    }
    keep.join(" ")
}

fn tokens(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 4 && !GENERIC.contains(w) && !w.chars().all(|c| c.is_ascii_digit())).map(str::to_owned).collect()
}

/// An installed app this installer most likely belongs to: the Authenticode signer must equal
/// the app's publisher (normalized) and a distinctive word of the app name (4+ letters, not a
/// generic word like `setup`) must appear in the file name. A hint only; it never changes the tier.
pub fn installed_hint(file_name: &str, signer: Option<&str>, apps: &[(String, Option<String>)]) -> Option<String> {
    let signer = org(signer?);
    if signer.is_empty() {
        return None;
    }
    let stem: String = file_name.rsplit_once('.').map_or(file_name, |(s, _)| s).to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
    apps.iter()
        .find(|(name, publisher)| publisher.as_deref().is_some_and(|p| org(p) == signer) && tokens(name).iter().any(|t| stem.contains(t.as_str())))
        .map(|(name, publisher)| format!("{name} is installed (publisher {})", publisher.as_deref().unwrap_or_default()))
}

// ---------------------------------------------------------------------------------------------
// Analysis

/// Content, signer and live-state evidence from the platform.
pub trait InstallerProbe {
    fn classify(&self, path: &RawPath) -> Verdict;
    fn signer(&self, path: &RawPath) -> Option<String>;
    /// Whether a disk image is attached now (`None`: cannot tell).
    fn mounted(&self, path: &RawPath) -> Option<bool>;
    /// Whether another process holds the file open now (`None`: cannot tell).
    fn in_use(&self, path: &RawPath) -> Option<bool>;
}

pub struct InstallerInput<'a> {
    pub scan_id: &'a str,
    pub revision: u32,
    /// Source label of the declared location (`downloads`, `desktop`, `browser_downloads`...).
    pub source_label: &'a str,
    pub max_depth: u32,
    /// Installed applications: (display name, publisher).
    pub apps: &'a [(String, Option<String>)],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstallerFacts {
    pub candidate_id: String,
    pub kind: InstallerKind,
    pub evidence: String,
    pub source_label: String,
    pub signer: Option<String>,
    pub installed_hint: Option<String>,
    pub mounted: Option<bool>,
    pub in_use: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstallerAnalysis {
    pub set: CandidateSet,
    pub facts: Vec<InstallerFacts>,
    /// Cloud placeholders in the location: never opened, so never classified.
    pub unclassified_placeholders: u64,
}

fn digest_for(max_depth: u32) -> String {
    let markers: Vec<&str> = PE_MARKERS.iter().map(|m| m.0).collect();
    let text = format!("cm-installer-rules-v1\0{CATALOG_VERSION}\0{PROTECTION_VERSION}\0{}\0{}\0{max_depth}", markers.join("|"), PAYLOAD_EXT.join("|"));
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

pub fn rule_set_digest(max_depth: u32) -> String {
    digest_for(max_depth)
}

pub fn is_current_digest(d: &str) -> bool {
    d.len() == 64 && (1..=MAX_DEPTH_LIMIT).any(|m| digest_for(m) == d)
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
    std::iter::successors(view.parent(entry), |p| view.parent(*p)).find_map(|a| hard.covering(&id(a))).map(|r| format!("inside {} ({})", r.display, r.class.as_str()))
}

/// Installers and images within `max_depth` of the scanned location (the view's root).
pub fn analyze_installers(view: &dyn IndexView, input: &InstallerInput<'_>, prot: &AnalysisProtection<'_>, probe: &dyn InstallerProbe) -> InstallerAnalysis {
    let mut found: BTreeMap<u64, (Candidate, InstallerFacts)> = BTreeMap::new();
    let mut placeholders = 0;
    let mut stack = vec![(view.root(), 0u32)];
    let max_depth = input.max_depth.clamp(1, MAX_DEPTH_LIMIT);
    while let Some((dir, d)) = stack.pop() {
        for &e in view.children(dir) {
            let f = view.info(e);
            if f.boundary {
                continue; // links and mounts are recorded, never followed or classified
            }
            if f.kind == ObjectKind::Directory {
                if d + 1 < max_depth {
                    stack.push((e, d + 1));
                }
                continue;
            }
            if f.kind != ObjectKind::File || f.logical.is_none_or(|n| n < 64) {
                continue;
            }
            if f.cloud == CloudState::Placeholder {
                placeholders += 1;
                continue;
            }
            let path = view.path(e);
            let Verdict::Installer { kind, evidence } = probe.classify(&path) else { continue };
            let name = view.name(e).lossy();
            let signer = if kind.signable() { probe.signer(&path) } else { None };
            let hint = installed_hint(&name, signer.as_deref(), input.apps);
            let mounted = if kind.is_image() { probe.mounted(&path) } else { Some(false) };
            let in_use = probe.in_use(&path);
            let mut reasons = vec![format!("installer:{}", kind.as_str()), format!("source:{}", input.source_label)];
            let (mut blocks, mut pending) = (Vec::new(), Vec::new());
            if let Some(why) = hard_deny(view, e, &path, prot) {
                blocks.push(format!("hard_deny:{why}"));
            }
            match prot.user {
                UserList::NotSupplied => {}
                UserList::Loaded(user) => {
                    if let Some(u) = user.match_path(&path, false) {
                        blocks.push(format!("user_protected:{}", u.id));
                    }
                    if let Some(u) = user.protects_rule(&kind.rule_id()) {
                        blocks.push(format!("user_protected_rule:{}", u.id));
                    }
                }
                UserList::Unavailable(_) => pending.push("user_protection_unavailable".to_string()),
            }
            match mounted {
                Some(true) => pending.push("mounted_image: eject it first (File Explorer > Eject, or Dismount-DiskImage)".into()),
                None => pending.push("mounted_state_unknown".into()),
                Some(false) => {}
            }
            match in_use {
                Some(true) => pending.push("in_use: another process has the file open".into()),
                None => pending.push("in_use_unknown".into()),
                Some(false) => {}
            }
            if f.link_count.is_some_and(|n| n > 1) {
                reasons.push("hard_link_outside_scope".into());
            }
            // INC-03: always review-only; protection and live state can only lower it.
            let inputs = TierInputs {
                rule_max: Tier::ReviewOnly,
                dependency: false,
                registry: RegistryStatus::NotChecked,
                protection: if blocks.is_empty() { Tier::DefaultSelected } else { Tier::Blocked },
                evidence: Tier::DefaultSelected,
                probes: vec![],
            };
            let state = if !blocks.is_empty() {
                CandidateState::Blocked
            } else if !pending.is_empty() {
                CandidateState::PendingEvidence
            } else {
                CandidateState::Allowed
            };
            reasons.extend(blocks);
            reasons.extend(pending);
            if let Some(s) = &signer {
                reasons.push(format!("signer:{s}"));
            }
            reasons.sort();
            reasons.dedup();
            let tier = compute_tier(&inputs).tier;
            let rules = vec![RuleRef { rule_id: kind.rule_id(), version: CATALOG_VERSION.into() }];
            let id = serde_json::json!({"scan": input.scan_id, "revision": input.revision, "entry": e, "rules": rules});
            let candidate_id = format!("c_{}", &digest(&id).expect("plain JSON")[..24]);
            let mut logical = KnownTotal::default();
            let _ = logical.add(f.logical.map(Bytes));
            let facts = InstallerFacts {
                candidate_id: candidate_id.clone(), kind, evidence, source_label: input.source_label.into(), signer, installed_hint: hint, mounted, in_use,
            };
            let candidate = Candidate {
                candidate_id, entry_id: e, path, kind: f.kind, rules, category: "installers".into(), state, tier, reasons, protection: None, logical,
                allowed_modes: if state == CandidateState::Blocked { vec![] } else { vec![Mode::Trash, Mode::Quarantine, Mode::Delete] }, probes: vec![],
                impact: format!("{} installer or image from {}; getting it back means downloading it again from its publisher", kind.as_str(), input.source_label),
                owner_id: None,
            };
            found.insert(e, (candidate, facts));
        }
    }
    let (candidates, facts): (Vec<Candidate>, Vec<InstallerFacts>) = found.into_values().unzip();
    let user_key = match prot.user {
        UserList::NotSupplied => serde_json::Value::Null,
        UserList::Loaded(u) => serde_json::to_value(u.entries()).expect("entries serialize"),
        UserList::Unavailable(_) => "unavailable".into(),
    };
    let rule_set_digest = rule_set_digest(max_depth);
    let key = serde_json::json!({"scan": input.scan_id, "revision": input.revision, "rules": rule_set_digest, "hard_deny": prot.hard_deny.map(|_| HARD_DENY_VERSION),
        "user": user_key, "facts": facts});
    let set = CandidateSet {
        candidate_set_id: format!("cs_{}", &digest(&key).expect("plain JSON")[..24]),
        scan_id: input.scan_id.into(),
        revision: input.revision,
        rule_set_digest,
        protection_version: PROTECTION_VERSION.into(),
        candidates,
        inapplicable: vec![],
    };
    InstallerAnalysis { set, facts, unclassified_placeholders: placeholders }
}

#[cfg(test)]
mod tests;
