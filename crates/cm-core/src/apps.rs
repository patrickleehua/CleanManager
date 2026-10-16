//! Application inventory model (MVP APP-01..04).
//!
//! - APP-01: an instance is identified by its source record and location, never by its display
//!   name. Two installs that share a name but differ in source or location stay separate.
//! - APP-02: directory associations carry their basis, sharing and coverage. A location claimed
//!   by more than one instance (same identity, same path, or nested paths) is shared and is
//!   never counted as any instance's exclusive space; shared space is counted once.
//! - APP-03: a directory without a matching installation record stays `unknown`. Neither a
//!   missing record nor a name resemblance ever makes it a cleanup candidate.
//! - APP-04: an instance starts with uninstall `unsupported` (the read-only inventory never pretends that deleting a
//!   folder is an uninstall); `apps list` then reports the verified route of the uninstall module (UNI-01).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bytes::{Bytes, KnownTotal};
use crate::capability::{Capability, CapabilityState};
use crate::error::CoreError;
use crate::model::Timestamp;
use crate::path::RawPath;

/// Default reason of the uninstall capability before a platform resolved the instance's route (APP-04).
pub const READ_ONLY_REASON: &str =
    "Read-only first release: CleanManager does not run uninstallers or delete package folders; use Windows Settings or the vendor uninstaller";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistryHive {
    Hklm,
    Hkcu,
}

/// Registry view the record was read from. HKCU\Software is shared between views on
/// supported Windows versions, so per-user records report `Shared` instead of a bitness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistryView {
    #[serde(rename = "64")]
    View64,
    #[serde(rename = "32")]
    View32,
    Shared,
}

/// Where an installation record came from. The record key is part of the instance identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppSource {
    /// A non-MSI entry under `...\CurrentVersion\Uninstall\{key}`.
    RegistryUninstall { hive: RegistryHive, view: RegistryView, key: String },
    /// An uninstall entry with `WindowsInstaller=1` whose key is the MSI ProductCode. The hive
    /// and view are kept because the same product can be installed per-user and per-machine.
    Msi { hive: RegistryHive, view: RegistryView, product_code: String },
    /// A packaged (MSIX/AppX) application registered for the current user.
    Msix { package_family: String, full_name: String },
    /// A macOS application bundle (`*.app`), identified by its `CFBundleIdentifier` and bundle path.
    AppBundle { bundle_id: String, path: String },
    /// A macOS application bundle with a Mac App Store receipt (`Contents/_MASReceipt/receipt`).
    AppStore { bundle_id: String, path: String },
    /// A Homebrew cask (`Caskroom/<token>`) or formula (`Cellar/<token>`). `kind` is stored as `package_kind` (the
    /// enum's own tag is `kind`).
    Homebrew {
        token: String,
        #[serde(rename = "package_kind")]
        kind: HomebrewKind,
    },
    Unknown,
}

/// Which Homebrew package type an instance was installed as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomebrewKind {
    Cask,
    Formula,
}

impl HomebrewKind {
    pub fn as_str(self) -> &'static str {
        match self {
            HomebrewKind::Cask => "cask",
            HomebrewKind::Formula => "formula",
        }
    }
}

impl AppSource {
    /// Protocol `app_instance.source` value.
    pub fn wire_name(&self) -> &'static str {
        match self {
            AppSource::RegistryUninstall { .. } => "registry_uninstall",
            AppSource::Msi { .. } => "msi",
            AppSource::Msix { .. } => "msix_store",
            AppSource::AppBundle { .. } => "app_bundle",
            AppSource::AppStore { .. } => "app_store",
            AppSource::Homebrew { .. } => "homebrew",
            AppSource::Unknown => "unknown",
        }
    }

    fn id_material(&self) -> String {
        let hive = |h: &RegistryHive| match h {
            RegistryHive::Hklm => "hklm",
            RegistryHive::Hkcu => "hkcu",
        };
        let view = |v: &RegistryView| match v {
            RegistryView::View64 => "64",
            RegistryView::View32 => "32",
            RegistryView::Shared => "shared",
        };
        match self {
            AppSource::RegistryUninstall { hive: h, view: v, key } => format!("registry_uninstall\0{}\0{}\0{key}", hive(h), view(v)),
            AppSource::Msi { hive: h, view: v, product_code } => {
                format!("msi\0{}\0{}\0{}", hive(h), view(v), product_code.to_ascii_uppercase())
            }
            AppSource::Msix { package_family, full_name } => format!("msix\0{package_family}\0{full_name}"),
            AppSource::AppBundle { bundle_id, path } => format!("app_bundle\0{bundle_id}\0{path}"),
            AppSource::AppStore { bundle_id, path } => format!("app_store\0{bundle_id}\0{path}"),
            AppSource::Homebrew { token, kind } => format!("homebrew\0{}\0{token}", kind.as_str()),
            AppSource::Unknown => "unknown".to_owned(),
        }
    }
}

/// Protocol identity of an install location (`volume_id`, kind-prefixed `file_id`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LocationIdentity {
    pub volume_id: String,
    pub file_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    RegistryKey,
    DisplayName,
    /// The display name is an indirect resource string; a package name is shown instead.
    IndirectDisplayName,
    InstallLocation,
    LocationIdentity,
    /// A location was recorded but could not be resolved (missing, not absolute, unreadable).
    LocationUnresolved,
    WindowsInstaller,
    SystemComponent,
    PackageFullName,
    PackageRoot,
    Framework,
    ProtectedClass,
    /// `CFBundleIdentifier` of a macOS bundle.
    BundleId,
    /// Team identifier of the bundle's code signature.
    TeamId,
    /// A Mac App Store receipt inside the bundle.
    MasReceipt,
    /// The Homebrew Caskroom/Cellar token that installed it.
    CaskroomToken,
    /// The bundle's `Info.plist` is missing or could not be parsed.
    InfoPlistUnreadable,
    /// A macOS installer package receipt (`/var/db/receipts`) whose identifier matches the bundle; evidence only.
    PkgReceipt,
}

impl EvidenceKind {
    fn wire(self) -> &'static str {
        match self {
            EvidenceKind::RegistryKey => "registry_key",
            EvidenceKind::DisplayName => "display_name",
            EvidenceKind::IndirectDisplayName => "indirect_display_name",
            EvidenceKind::InstallLocation => "install_location",
            EvidenceKind::LocationIdentity => "location_identity",
            EvidenceKind::LocationUnresolved => "location_unresolved",
            EvidenceKind::WindowsInstaller => "windows_installer",
            EvidenceKind::SystemComponent => "system_component",
            EvidenceKind::PackageFullName => "package_full_name",
            EvidenceKind::PackageRoot => "package_root",
            EvidenceKind::Framework => "framework",
            EvidenceKind::ProtectedClass => "protected_class",
            EvidenceKind::BundleId => "bundle_id",
            EvidenceKind::TeamId => "team_id",
            EvidenceKind::MasReceipt => "mas_receipt",
            EvidenceKind::CaskroomToken => "caskroom_token",
            EvidenceKind::InfoPlistUnreadable => "info_plist_unreadable",
            EvidenceKind::PkgReceipt => "pkg_receipt",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Evidence {
    pub kind: EvidenceKind,
    pub detail: String,
}

impl Evidence {
    pub fn new(kind: EvidenceKind, detail: impl Into<String>) -> Self {
        Evidence { kind, detail: detail.into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStatus {
    UpToDate,
    UpdateAvailable,
    CannotCheck,
    NotChecked,
}

/// Why an instance is in a protected class (shown, never offered for removal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectedReason {
    SystemComponent,
    SharedRuntime,
    EndpointSecurityOrManagement,
    OsPackage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInstance {
    pub instance_id: String,
    pub name: String,
    pub version: Option<String>,
    pub publisher: Option<String>,
    pub source: AppSource,
    pub install_location: Option<RawPath>,
    /// `None` when the location is unrecorded, missing or unreadable: unknown, not absent.
    pub location_identity: Option<LocationIdentity>,
    pub evidence: Vec<Evidence>,
    pub uninstall_capability: Capability,
    pub update_status: UpdateStatus,
    pub protected_class: Option<ProtectedReason>,
}

/// Stable instance ID from the source record and the recorded location (never the name).
pub fn instance_id_of(source: &AppSource, location: Option<&RawPath>) -> String {
    let mut h = Sha256::new();
    h.update(b"cm-app-instance-v1\0");
    h.update(source.id_material().as_bytes());
    h.update(b"\0");
    match location {
        Some(p) => {
            h.update(p.raw_encoding().as_bytes());
            h.update(b"\0");
            h.update(p.raw_bytes());
        }
        None => h.update(b"-"),
    }
    let hex: String = h.finalize().iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!("app_{hex}")
}

/// The read-only uninstall capability of the first release (APP-04).
pub fn read_only_uninstall() -> Capability {
    Capability::new("app.uninstall", CapabilityState::Unsupported, Some(READ_ONLY_REASON))
}

impl AppInstance {
    pub fn new(name: String, source: AppSource, install_location: Option<RawPath>) -> Self {
        AppInstance {
            instance_id: instance_id_of(&source, install_location.as_ref()),
            name,
            version: None,
            publisher: None,
            source,
            install_location,
            location_identity: None,
            evidence: Vec::new(),
            uninstall_capability: read_only_uninstall(),
            update_status: UpdateStatus::NotChecked,
            protected_class: None,
        }
    }

    /// Protocol `app_instance` record (schemas/v1 `$defs.app_instance`).
    pub fn to_wire(&self) -> serde_json::Value {
        let evidence: Vec<String> = self.evidence.iter().map(|e| format!("{}: {}", e.kind.wire(), e.detail)).collect();
        serde_json::json!({
            "app_instance_id": self.instance_id,
            "name": self.name,
            "version": self.version,
            "source": self.source.wire_name(),
            "location": self.install_location,
            "publisher": self.publisher,
            "identity_evidence": evidence,
            "uninstall_capability": {
                "state": self.uninstall_capability.state,
                "reason": self.uninstall_capability.reason,
            },
            "update_status": self.update_status,
            "protected_class": self.protected_class.is_some(),
        })
    }
}

/// A problem met while building the inventory; the affected source is incomplete, not empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryIssue {
    pub code: String,
    pub source: String,
    pub message: String,
}

/// Registry/manifest text for display: trimmed, NULs removed, control characters escaped.
/// Empty text is unknown (`None`), never an empty string.
pub fn clean_text(s: &str) -> Option<String> {
    let out: String = s
        .chars()
        .filter(|c| *c != '\0')
        .flat_map(|c| -> Box<dyn Iterator<Item = char>> {
            if c.is_control() { Box::new(c.escape_unicode()) } else { Box::new(std::iter::once(c)) }
        })
        .collect();
    let t = out.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

const RUNTIME_MARKERS: &[&str] = &[
    "microsoft visual c++",
    "visual c++ redistributable",
    "vc_redist",
    "microsoft .net",
    ".net runtime",
    ".net desktop runtime",
    "windows desktop runtime",
    "asp.net core",
    ".net host",
    "webview2",
    "microsoft edge",
    "microsoft.vclibs",
    "microsoft.net.native",
    "microsoft.ui.xaml",
    "microsoft.windowsappruntime",
    "windows app runtime",
    "windows app sdk",
    "directx",
    "microsoft update health tools",
];

const SECURITY_MARKERS: &[&str] = &[
    "defender",
    "crowdstrike",
    "falcon sensor",
    "sentinelone",
    "sentinel agent",
    "carbon black",
    "cortex xdr",
    "symantec",
    "norton",
    "mcafee",
    "trellix",
    "sophos",
    "eset ",
    "kaspersky",
    "trend micro",
    "bitdefender",
    "cylance",
    "cybereason",
    "tanium",
    "qualys",
    "rapid7",
    "intune",
    "configuration manager client",
    "workspace one",
    "airwatch",
    "bigfix",
    "ivanti",
    "zscaler",
    "netskope",
    "forcepoint",
    "forticlient",
    "globalprotect",
    "cisco secure",
    "anyconnect",
    "microsoft monitoring agent",
    "azure connected machine agent",
    "elastic agent",
    "wazuh",
    "火绒",
    "360安全",
    "360 total security",
    "电脑管家",
];

fn contains_marker(text: &str, markers: &[&str]) -> bool {
    let lower = text.to_lowercase();
    markers.iter().any(|m| lower.contains(m))
}

/// Protected-class decision for a registry or package record. System components, shared
/// runtimes (VC++ redistributables, .NET, WebView2, ...) and endpoint security/management
/// agents are inventoried but never treated as removable.
pub fn protected_reason(name: &str, publisher: Option<&str>, system_component: bool) -> Option<ProtectedReason> {
    if system_component {
        return Some(ProtectedReason::SystemComponent);
    }
    if contains_marker(name, SECURITY_MARKERS) || publisher.is_some_and(|p| contains_marker(p, SECURITY_MARKERS)) {
        return Some(ProtectedReason::EndpointSecurityOrManagement);
    }
    if contains_marker(name, RUNTIME_MARKERS) {
        return Some(ProtectedReason::SharedRuntime);
    }
    None
}

/// Merge records into instances. Only records with the same instance ID (same source record
/// and location) are combined; display names never merge instances (APP-01). Missing fields
/// are filled from the duplicate, evidence is unioned, and protection is never dropped.
pub fn merge_instances(records: Vec<AppInstance>) -> Vec<AppInstance> {
    let mut by_id: BTreeMap<String, AppInstance> = BTreeMap::new();
    for r in records {
        match by_id.get_mut(&r.instance_id) {
            None => {
                by_id.insert(r.instance_id.clone(), r);
            }
            Some(a) => {
                a.version = a.version.take().or(r.version);
                a.publisher = a.publisher.take().or(r.publisher);
                a.location_identity = a.location_identity.take().or(r.location_identity);
                a.protected_class = a.protected_class.or(r.protected_class);
                for e in r.evidence {
                    if !a.evidence.contains(&e) {
                        a.evidence.push(e);
                    }
                }
            }
        }
    }
    let mut out: Vec<AppInstance> = by_id.into_values().collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.instance_id.cmp(&b.instance_id)));
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssociationBasis {
    /// The installer recorded this folder as the install location.
    InstallLocation,
    /// The package manager recorded this folder as the package root.
    PackageRoot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageGap {
    /// The recorded location could not be opened or identified.
    LocationUnresolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum Coverage {
    Known,
    Unknown { reason: CoverageGap },
}

/// One directory associated with one instance, with its evidence basis and sharing (APP-02).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Association {
    pub instance_id: String,
    pub path: RawPath,
    pub identity: Option<LocationIdentity>,
    pub basis: AssociationBasis,
    /// Other instances whose location is the same object or overlaps by nesting.
    pub shared_with: Vec<String>,
    pub coverage: Coverage,
    pub observed_at: Timestamp,
}

impl Association {
    /// Only an unshared, identified location can be attributed to a single instance.
    pub fn is_exclusive(&self) -> bool {
        self.shared_with.is_empty() && self.coverage == Coverage::Known
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssociationReport {
    pub associations: Vec<Association>,
    /// Instances without a recorded location: their storage is a coverage gap, not zero.
    pub without_location: Vec<String>,
}

/// Comparable path text: Windows ASCII case folded and `/` read as `\`, trailing separators
/// removed. Used only to detect overlap, never to address files.
fn path_key(p: &RawPath) -> Vec<u16> {
    let mut units: Vec<u16> = match p {
        RawPath::Windows(u) => u
            .iter()
            .map(|&c| match c {
                0x61..=0x7a => c - 0x20,
                0x2f => 0x5c,
                _ => c,
            })
            .collect(),
        RawPath::Unix(b) => b.iter().map(|&c| u16::from(c)).collect(),
    };
    let sep = if matches!(p, RawPath::Windows(_)) { 0x5c } else { 0x2f };
    while units.len() > 1 && units.last() == Some(&sep) {
        units.pop();
    }
    units
}

fn separator(p: &RawPath) -> u16 {
    if matches!(p, RawPath::Windows(_)) { 0x5c } else { 0x2f }
}

/// `inner` lies strictly below `outer`.
fn is_nested(outer: &RawPath, inner: &RawPath) -> bool {
    if outer.platform() != inner.platform() {
        return false;
    }
    let (o, i) = (path_key(outer), path_key(inner));
    let sep = separator(outer);
    if i.len() <= o.len() || !i.starts_with(&o) {
        return false;
    }
    // Keys drop trailing separators (`C:\` becomes `C:`); only a POSIX root `/` keeps one.
    o.last() == Some(&sep) || i[o.len()] == sep
}

fn same_location(a: &Association, b: &Association) -> bool {
    match (&a.identity, &b.identity) {
        (Some(x), Some(y)) if x == y => return true,
        _ => {}
    }
    a.path.platform() == b.path.platform() && path_key(&a.path) == path_key(&b.path)
}

fn overlaps(a: &Association, b: &Association) -> bool {
    same_location(a, b) || is_nested(&a.path, &b.path) || is_nested(&b.path, &a.path)
}

/// Build directory associations and mark shared locations (APP-02).
pub fn associate(instances: &[AppInstance], observed_at: &Timestamp) -> AssociationReport {
    let mut associations = Vec::new();
    let mut without_location = Vec::new();
    for inst in instances {
        let Some(path) = &inst.install_location else {
            without_location.push(inst.instance_id.clone());
            continue;
        };
        let basis = if matches!(inst.source, AppSource::Msix { .. }) { AssociationBasis::PackageRoot } else { AssociationBasis::InstallLocation };
        let coverage = match inst.location_identity {
            Some(_) => Coverage::Known,
            None => Coverage::Unknown { reason: CoverageGap::LocationUnresolved },
        };
        associations.push(Association {
            instance_id: inst.instance_id.clone(),
            path: path.clone(),
            identity: inst.location_identity.clone(),
            basis,
            shared_with: Vec::new(),
            coverage,
            observed_at: observed_at.clone(),
        });
    }
    let n = associations.len();
    for i in 0..n {
        let mut shared: Vec<String> = (0..n)
            .filter(|&j| j != i && associations[j].instance_id != associations[i].instance_id && overlaps(&associations[i], &associations[j]))
            .map(|j| associations[j].instance_id.clone())
            .collect();
        shared.sort();
        shared.dedup();
        associations[i].shared_with = shared;
    }
    AssociationReport { associations, without_location }
}

/// Space attribution where every location is counted at most once (APP-02).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SpaceAttribution {
    /// Per instance: only identified, unshared locations.
    pub exclusive: BTreeMap<String, KnownTotal>,
    /// Shared locations, each counted once; nested locations are inside their outer location.
    pub shared: KnownTotal,
    pub shared_locations: u64,
    /// Locations with unknown coverage; never attributed to anyone.
    pub unresolved_locations: u64,
}

/// Attribute measured location sizes without double counting shared or nested locations.
/// `size_of` returns `None` when a size is unknown; unknown is counted, never zero.
pub fn exclusive_space(associations: &[Association], size_of: impl Fn(&Association) -> Option<Bytes>) -> Result<SpaceAttribution, CoreError> {
    let mut out = SpaceAttribution::default();
    for (i, a) in associations.iter().enumerate() {
        if a.coverage != Coverage::Known {
            out.unresolved_locations += 1;
            continue;
        }
        if a.is_exclusive() {
            out.exclusive.entry(a.instance_id.clone()).or_default().add(size_of(a))?;
            continue;
        }
        // Count a shared location once: skip it when an earlier association is the same
        // location, or when any other identified association contains it.
        let duplicate = associations[..i].iter().any(|b| b.coverage == Coverage::Known && same_location(a, b));
        let contained = associations.iter().any(|b| b.coverage == Coverage::Known && is_nested(&b.path, &a.path));
        if duplicate || contained {
            continue;
        }
        out.shared.add(size_of(a))?;
        out.shared_locations += 1;
    }
    Ok(out)
}

/// What is known about installation records for a directory (APP-03).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallRecordStatus {
    /// No supported source listed an installation for this directory.
    NotFound,
    /// The directory belongs to a source this platform build cannot read.
    SourceUnsupported,
    /// A source confirmed an uninstall; ownership of leftover data is still unproven.
    ConfirmedUninstalled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownOwnership {
    NoInstallationRecord,
    NameResemblanceOnly,
    SourceUnsupported,
    UninstalledOwnershipUnproven,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum Ownership {
    /// Recorded as (part of) the location of these instances.
    Associated { instance_ids: Vec<String>, shared: bool },
    Unknown { reason: UnknownOwnership, detail: String },
}

/// Read-only explanation of a directory's ownership. The inventory never produces cleanup
/// candidates: `cleanup_candidate` is always false and `auto_select` is always false.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryAssessment {
    pub path: RawPath,
    pub ownership: Ownership,
    cleanup_candidate: bool,
    auto_select: bool,
}

impl DirectoryAssessment {
    pub fn cleanup_candidate(&self) -> bool {
        self.cleanup_candidate
    }

    pub fn auto_select(&self) -> bool {
        self.auto_select
    }
}

fn last_component(p: &RawPath) -> String {
    let text = p.lossy();
    text.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().unwrap_or_default().to_lowercase()
}

/// Explain a directory that may look like application data (APP-03). A missing installation
/// record or a folder name resembling an app name leaves the directory unknown and read-only.
pub fn assess_directory(
    path: &RawPath,
    identity: Option<&LocationIdentity>,
    record: InstallRecordStatus,
    instances: &[AppInstance],
    associations: &[Association],
) -> DirectoryAssessment {
    let mut ids: Vec<String> = associations
        .iter()
        .filter(|a| {
            identity.is_some_and(|id| a.identity.as_ref() == Some(id))
                || (a.path.platform() == path.platform() && path_key(&a.path) == path_key(path))
                || is_nested(&a.path, path)
        })
        .map(|a| a.instance_id.clone())
        .collect();
    ids.sort();
    ids.dedup();
    let ownership = if !ids.is_empty() {
        let shared = ids.len() > 1;
        Ownership::Associated { instance_ids: ids, shared }
    } else {
        let name = last_component(path);
        let resembles = name.chars().count() >= 3
            && instances.iter().any(|i| {
                let n = i.name.to_lowercase();
                n.contains(&name) || name.contains(&n)
            });
        let (reason, detail) = match record {
            InstallRecordStatus::SourceUnsupported => {
                (UnknownOwnership::SourceUnsupported, "The installation source for this location is not supported; ownership is unknown")
            }
            _ if resembles => {
                (UnknownOwnership::NameResemblanceOnly, "The folder name resembles an installed application, which does not prove ownership")
            }
            InstallRecordStatus::ConfirmedUninstalled => (
                UnknownOwnership::UninstalledOwnershipUnproven,
                "An uninstall was recorded, but no record proves this directory belonged to that application",
            ),
            InstallRecordStatus::NotFound => {
                (UnknownOwnership::NoInstallationRecord, "No installation record was found; a missing record is not proof of residue")
            }
        };
        Ownership::Unknown { reason, detail: detail.to_owned() }
    };
    DirectoryAssessment { path: path.clone(), ownership, cleanup_candidate: false, auto_select: false }
}

#[cfg(test)]
mod tests;
