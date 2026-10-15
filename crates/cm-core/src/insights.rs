//! Disk insights (parity DIN-01/02): known large system and virtualization objects, explained.
//!
//! An insight is read-only guidance. It names the object, says what it is and what drives its size, and lists the
//! official system tool that manages it as text. CleanManager never runs those tools, never changes the
//! settings, and offers no trash or delete entry for these objects: `handling` is one of the guidance kinds and
//! `guidance` is display text only. A size that cannot be measured reliably stays unknown with a note, never 0
//! and never a naive sum (WinSxS hard links, snapshot storage).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::path::RawPath;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Handling {
    ReadOnlyGuidance,
    OpenSystemSettings,
    OpenOwnerTool,
    /// Reserved for the old-downloads card (7.3); never used by the known-object catalog.
    ManualTrashReview,
    /// Reserved for installer files (7.3); never used by the known-object catalog.
    InstallerReview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeBasis {
    /// Directory-entry size of a single file (no read, no open for data).
    FileMetadata,
    /// Sum of file sizes from a bounded read-only walk.
    DirectoryWalk,
    /// Reported by a system interface (for example shadow storage used space).
    SystemQuery,
    NotMeasured,
}

/// A step the user can take with a system or owner tool. Text only: never executed by CleanManager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guidance {
    pub tool: String,
    pub steps: String,
    pub requires_admin: bool,
    pub impact: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Insight {
    pub insight_id: String,
    pub kind: String,
    pub title: String,
    pub path: Option<RawPath>,
    pub logical_bytes: Option<u64>,
    pub count: Option<u64>,
    pub size_basis: SizeBasis,
    pub size_note: Option<String>,
    pub observed_at: Option<String>,
    pub explanation: String,
    pub handling: Handling,
    pub guidance: Vec<Guidance>,
}

/// Windows known-object kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsObject {
    HibernationFile,
    PageFile,
    SwapFile,
    ComponentStore,
    ShadowStorage,
    WslDisk,
    DockerDisk,
    HyperVDisks,
    PreviousInstallation,
    DeliveryOptimization,
}

fn g(tool: &str, steps: &str, requires_admin: bool, impact: &str) -> Guidance {
    Guidance { tool: tool.into(), steps: steps.into(), requires_admin, impact: impact.into() }
}

impl WindowsObject {
    pub const ALL: [WindowsObject; 10] = [
        WindowsObject::HibernationFile,
        WindowsObject::PageFile,
        WindowsObject::SwapFile,
        WindowsObject::ComponentStore,
        WindowsObject::ShadowStorage,
        WindowsObject::WslDisk,
        WindowsObject::DockerDisk,
        WindowsObject::HyperVDisks,
        WindowsObject::PreviousInstallation,
        WindowsObject::DeliveryOptimization,
    ];

    pub fn kind(self) -> &'static str {
        match self {
            WindowsObject::HibernationFile => "windows.hibernation_file",
            WindowsObject::PageFile => "windows.page_file",
            WindowsObject::SwapFile => "windows.swap_file",
            WindowsObject::ComponentStore => "windows.component_store",
            WindowsObject::ShadowStorage => "windows.shadow_storage",
            WindowsObject::WslDisk => "windows.wsl_disk",
            WindowsObject::DockerDisk => "windows.docker_disk",
            WindowsObject::HyperVDisks => "windows.hyperv_disks",
            WindowsObject::PreviousInstallation => "windows.previous_installation",
            WindowsObject::DeliveryOptimization => "windows.delivery_optimization_cache",
        }
    }

    fn title(self) -> &'static str {
        match self {
            WindowsObject::HibernationFile => "Hibernation file",
            WindowsObject::PageFile => "Paging file",
            WindowsObject::SwapFile => "Swap file for packaged apps",
            WindowsObject::ComponentStore => "Windows component store (WinSxS)",
            WindowsObject::ShadowStorage => "System Restore and shadow copy storage",
            WindowsObject::WslDisk => "WSL distribution disk",
            WindowsObject::DockerDisk => "Docker Desktop data disk",
            WindowsObject::HyperVDisks => "Hyper-V virtual disks",
            WindowsObject::PreviousInstallation => "Previous Windows installation (Windows.old)",
            WindowsObject::DeliveryOptimization => "Delivery Optimization cache",
        }
    }

    fn handling(self) -> Handling {
        match self {
            WindowsObject::PageFile | WindowsObject::SwapFile | WindowsObject::ShadowStorage | WindowsObject::PreviousInstallation | WindowsObject::DeliveryOptimization => {
                Handling::OpenSystemSettings
            }
            WindowsObject::WslDisk | WindowsObject::DockerDisk | WindowsObject::HyperVDisks => Handling::OpenOwnerTool,
            WindowsObject::HibernationFile | WindowsObject::ComponentStore => Handling::ReadOnlyGuidance,
        }
    }

    fn explanation(self) -> &'static str {
        match self {
            WindowsObject::HibernationFile => {
                "Holds the memory image Windows writes for hibernate and Fast Startup. Its size follows installed RAM and the hibernation file type (full or reduced). It is in use while hibernation is enabled; Windows removes it itself when hibernation is turned off, and deleting it by hand would break resume."
            }
            WindowsObject::PageFile => {
                "Virtual memory backing store. Windows sizes it from installed RAM, commit demand and crash-dump settings, keeps it open while running and recreates it at boot. It is not junk: removing or shrinking it can cause out-of-memory errors and lost crash dumps."
            }
            WindowsObject::SwapFile => {
                "Used by Windows to page packaged (Store) apps in and out efficiently. It is managed together with the paging file and is normally small."
            }
            WindowsObject::ComponentStore => {
                "WinSxS stores Windows components plus the versions kept for servicing and update rollback. Most of its files are hard links shared with C:\\Windows\\System32, so folder sizes in Explorer count the same data more than once. Never delete files inside WinSxS; superseded versions are removed only by Windows servicing."
            }
            WindowsObject::ShadowStorage => {
                "Volume Shadow Copy storage holds System Restore points and snapshots made by backup software. Windows caps it at the configured maximum and deletes the oldest copies automatically when it needs the space."
            }
            WindowsObject::WslDisk => {
                "Virtual disk that contains a Linux distribution's whole filesystem. It grows when files are added inside Linux and does not shrink by itself after they are deleted. Deleting the file deletes the distribution and everything in it."
            }
            WindowsObject::DockerDisk => {
                "Docker Desktop's data disk with images, containers, volumes and build cache. It grows with use and does not shrink by itself. Deleting it removes all Docker data; manage it from Docker."
            }
            WindowsObject::HyperVDisks => {
                "Virtual hard disks of Hyper-V virtual machines in the default disk folder. Dynamic disks and checkpoint (.avhdx) files grow with use. Deleting a disk destroys the virtual machine's data."
            }
            WindowsObject::PreviousInstallation => {
                "Copy of the previous Windows installation kept after a feature update so you can go back. Windows deletes it automatically when the rollback period ends (10 days by default)."
            }
            WindowsObject::DeliveryOptimization => {
                "Windows Update and Store content cached so it can be shared with other PCs. Windows trims it automatically by age and by the configured size limits."
            }
        }
    }

    fn guidance(self) -> Vec<Guidance> {
        match self {
            WindowsObject::HibernationFile => vec![
                g("powercfg", "powercfg /hibernate /type reduced", true, "Hibernate is no longer offered; Fast Startup keeps working with a smaller file"),
                g("powercfg", "powercfg /hibernate off", true, "Disables hibernate and Fast Startup; Windows deletes hiberfil.sys itself"),
            ],
            WindowsObject::PageFile | WindowsObject::SwapFile => vec![g(
                "System Properties (SystemPropertiesPerformance.exe)",
                "Advanced > Virtual memory > Change. Keep \"Automatically manage paging file size\" unless you have a specific reason",
                true,
                "A paging file that is too small causes out-of-memory errors and prevents crash dumps; changes apply after a restart",
            )],
            WindowsObject::ComponentStore => vec![
                g("DISM", "DISM /Online /Cleanup-Image /AnalyzeComponentStore", true, "Read-only report of the actual store size and whether cleanup is recommended; takes several minutes"),
                g("DISM", "DISM /Online /Cleanup-Image /StartComponentCleanup", true, "Removes superseded component versions; the updates they belonged to can no longer be rolled back"),
                g("Disk Cleanup (cleanmgr.exe)", "Clean up system files > Windows Update Cleanup", true, "Same effect as StartComponentCleanup"),
            ],
            WindowsObject::ShadowStorage => vec![
                g("System Protection (SystemPropertiesProtection.exe)", "Select the drive > Configure > lower Max Usage, or Delete", true, "Deleted restore points can no longer be used to roll the system back"),
                g("Disk Cleanup (cleanmgr.exe)", "Clean up system files > More Options > System Restore and Shadow Copies > Clean up", true, "Deletes all but the most recent restore point"),
            ],
            WindowsObject::WslDisk => vec![
                g("WSL", "Free space inside the distribution first (for example apt clean), then run wsl --shutdown and wsl --manage <distribution> --set-sparse true so freed blocks are returned to Windows", false, "The distribution is stopped while WSL shuts down"),
                g("Hyper-V PowerShell", "After wsl --shutdown: Optimize-VHD -Path <ext4.vhdx> -Mode Full", true, "Requires the Hyper-V module; the distribution must not be running"),
                g("WSL", "wsl --export <distribution> <file> before any removal; wsl --unregister <distribution> deletes it", false, "Unregistering permanently deletes every file in the distribution"),
            ],
            WindowsObject::DockerDisk => vec![
                g("Docker", "docker system df to see usage, then docker system prune (add --volumes only if you no longer need the data)", false, "Pruned images are downloaded again when needed; pruned volumes lose their data"),
                g("Docker Desktop", "Troubleshoot > Clean / Purge data", false, "Removes all images, containers and volumes"),
            ],
            WindowsObject::HyperVDisks => vec![g(
                "Hyper-V Manager (virtmgmt.msc)",
                "Delete unused virtual machines and checkpoints; Edit Disk > Compact for dynamic disks",
                true,
                "Deleting a virtual machine's disk deletes its data; applying or deleting checkpoints cannot be undone",
            )],
            WindowsObject::PreviousInstallation => vec![
                g("Settings", "System > Storage > Temporary files > Previous Windows installation(s)", true, "You can no longer go back to the previous Windows version"),
                g("Disk Cleanup (cleanmgr.exe)", "Clean up system files > Previous Windows installation(s)", true, "You can no longer go back to the previous Windows version"),
            ],
            WindowsObject::DeliveryOptimization => vec![
                g("Settings", "Windows Update > Advanced options > Delivery Optimization > Advanced options: set cache limits", true, "Other PCs can fetch less content from this PC"),
                g("Disk Cleanup (cleanmgr.exe)", "Select Delivery Optimization Files", false, "Content is downloaded again from Microsoft when needed"),
            ],
        }
    }
}

/// Stable insight ID from the kind and the object's discriminator (path or name).
pub fn insight_id(kind: &str, discriminator: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"cm-insight-v1\0");
    h.update(kind.as_bytes());
    h.update(b"\0");
    h.update(discriminator.to_lowercase().as_bytes());
    let hex: String = h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("i_{hex}")
}

/// What a platform probe measured for one object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Measured {
    Bytes { bytes: u64, basis: SizeBasis },
    /// Not measured, with the reason shown to the user.
    Unknown(String),
}

impl Insight {
    /// Build an insight for a Windows known object. `label` qualifies the title (volume, distribution name).
    pub fn windows(object: WindowsObject, label: Option<&str>, discriminator: &str, path: Option<RawPath>, size: Measured, count: Option<u64>, observed_at: &str) -> Self {
        let (logical_bytes, size_basis, size_note) = match size {
            Measured::Bytes { bytes, basis } => (Some(bytes), basis, None),
            Measured::Unknown(why) => (None, SizeBasis::NotMeasured, Some(why)),
        };
        let title = match label {
            Some(l) => format!("{} ({l})", object.title()),
            None => object.title().to_owned(),
        };
        Insight {
            insight_id: insight_id(object.kind(), discriminator),
            kind: object.kind().into(),
            title,
            path,
            logical_bytes,
            count,
            size_basis,
            size_note,
            observed_at: Some(observed_at.into()),
            explanation: object.explanation().into(),
            handling: object.handling(),
            guidance: object.guidance(),
        }
    }

    pub fn to_wire(&self) -> serde_json::Value {
        serde_json::json!({
            "insight_id": self.insight_id,
            "kind": self.kind,
            "title": self.title,
            "path": self.path,
            "logical_bytes": self.logical_bytes.map(|b| b.to_string()),
            "count": self.count,
            "size_basis": self.size_basis,
            "size_note": self.size_note,
            "observed_at": self.observed_at,
            "explanation": self.explanation,
            "handling": self.handling,
            "guidance": self.guidance,
        })
    }
}

/// Deterministic order: largest known size first, unknown sizes last, then title.
pub fn sort(items: &mut [Insight]) {
    items.sort_by(|a, b| b.logical_bytes.is_some().cmp(&a.logical_bytes.is_some()).then(b.logical_bytes.cmp(&a.logical_bytes)).then(a.title.cmp(&b.title)));
}

// ---------------------------------------------------------------------------------------------------------------
// Overview cards and the old-downloads review (parity DIN-03/04, task 7.3).

/// Overview card categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    OldDownloads,
    VirtualMachines,
    Snapshots,
    SystemFiles,
    ComponentStore,
    PreviousInstallation,
    UpdateCache,
    DeveloperTools,
    ModelLibraries,
    DeviceBackups,
}

impl CardKind {
    fn title(self) -> &'static str {
        match self {
            CardKind::OldDownloads => "Old downloads",
            CardKind::VirtualMachines => "Virtual machines and containers",
            CardKind::Snapshots => "Restore points and snapshots",
            CardKind::SystemFiles => "Hibernation and paging files",
            CardKind::ComponentStore => "Windows component store",
            CardKind::PreviousInstallation => "Previous Windows installation",
            CardKind::UpdateCache => "Update delivery cache",
            CardKind::DeveloperTools => "Developer tool caches",
            CardKind::ModelLibraries => "AI model libraries",
            CardKind::DeviceBackups => "Device backups",
        }
    }

    /// Card a known Windows object belongs to.
    pub fn of_insight(kind: &str) -> Option<CardKind> {
        Some(match kind {
            "windows.wsl_disk" | "windows.docker_disk" | "windows.hyperv_disks" => CardKind::VirtualMachines,
            "windows.shadow_storage" => CardKind::Snapshots,
            "windows.hibernation_file" | "windows.page_file" | "windows.swap_file" => CardKind::SystemFiles,
            "windows.component_store" => CardKind::ComponentStore,
            "windows.previous_installation" => CardKind::PreviousInstallation,
            "windows.delivery_optimization_cache" => CardKind::UpdateCache,
            _ => return None,
        })
    }
}

/// A measured directory for the developer-tool, model-library and device-backup cards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeasuredDir {
    pub card: CardKind,
    pub label: String,
    pub path: RawPath,
    pub size: Measured,
    pub observed_at: String,
}

/// One file in the old-downloads review. Never preselected (DIN-04).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OldDownload {
    pub path: RawPath,
    pub logical_bytes: u64,
    pub modified_at: String,
    pub age_days: u64,
    /// Looks like an installer package by extension; such files can also go through the installer review.
    pub installer_like: bool,
    pub selected: bool,
}

/// A file found in Downloads, as the platform listed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadEntry {
    pub path: RawPath,
    pub logical_bytes: u64,
    pub modified_unix: i64,
}

/// The old-downloads review (DIN-04): a size-sorted list with nothing selected; handling is the manual trash review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OldDownloadsReview {
    pub root: Option<RawPath>,
    pub older_than_days: u32,
    pub observed_at: String,
    pub items: Vec<OldDownload>,
    pub total_logical_bytes: Option<u64>,
    pub item_count: u64,
    /// Why the list may be incomplete (unreadable folders, listing limit), or why there is no list.
    pub note: Option<String>,
    pub handling: Handling,
}

/// Default age threshold of the old-downloads card.
pub const OLD_DOWNLOADS_DAYS: u32 = 90;

const INSTALLER_EXTENSIONS: &[&str] = &["exe", "msi", "msix", "msixbundle", "appx", "appxbundle", "iso", "dmg", "pkg", "zip", "7z"];

/// Filter by age, sort (largest first, then path) and wrap the listing. `root: None` = no Downloads folder (unknown).
pub fn old_downloads(root: Option<RawPath>, entries: Vec<DownloadEntry>, now_unix: i64, days: u32, observed_at: &str, note: Option<String>) -> OldDownloadsReview {
    let cutoff = now_unix - i64::from(days) * 86_400;
    let mut items: Vec<OldDownload> = entries
        .into_iter()
        .filter(|e| e.modified_unix <= cutoff)
        .map(|e| {
            let ext = e.path.display().rsplit_once('.').map(|(_, x)| x.to_lowercase()).unwrap_or_default();
            OldDownload {
                logical_bytes: e.logical_bytes,
                modified_at: crate::time::utc_from_unix(e.modified_unix),
                age_days: ((now_unix - e.modified_unix).max(0) / 86_400) as u64,
                installer_like: INSTALLER_EXTENSIONS.contains(&ext.as_str()),
                selected: false,
                path: e.path,
            }
        })
        .collect();
    items.sort_by(|a, b| b.logical_bytes.cmp(&a.logical_bytes).then_with(|| a.path.display().cmp(&b.path.display())));
    let total = items.iter().map(|i| i.logical_bytes).fold(0u64, u64::saturating_add);
    OldDownloadsReview {
        item_count: items.len() as u64,
        total_logical_bytes: root.is_some().then_some(total),
        root,
        older_than_days: days,
        observed_at: observed_at.into(),
        items,
        note,
        handling: Handling::ManualTrashReview,
    }
}

/// Where a card opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardLink {
    /// `analyze` (disk analysis at `path`), `insights.downloads` (old-downloads review) or `insights` (known objects).
    pub view: String,
    pub path: Option<RawPath>,
    pub insight_ids: Vec<String>,
}

/// One overview card (DIN-03): known size or unknown, with the last observation time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsightCard {
    pub card_id: String,
    pub kind: CardKind,
    pub title: String,
    /// `None` = unknown (never 0 and never a partial sum presented as the total).
    pub logical_bytes: Option<u64>,
    /// Object count (snapshots: the number of restore points, shown even when sizes are unavailable).
    pub count: Option<u64>,
    pub size_note: Option<String>,
    /// Oldest observation among the card's members.
    pub observed_at: Option<String>,
    pub link: CardLink,
}

struct Member {
    bytes: Option<u64>,
    count: Option<u64>,
    note: Option<String>,
    observed_at: Option<String>,
    path: Option<RawPath>,
    insight_id: Option<String>,
}

fn card(kind: CardKind, members: &[Member]) -> InsightCard {
    let known: Vec<u64> = members.iter().filter_map(|m| m.bytes).collect();
    let unknown = members.len() - known.len();
    let sum = known.iter().copied().fold(0u64, u64::saturating_add);
    let (logical_bytes, size_note) = if unknown == 0 {
        (Some(sum), None)
    } else {
        let why = members.iter().filter(|m| m.bytes.is_none()).find_map(|m| m.note.clone()).unwrap_or_else(|| "size not available".into());
        let known_part = if known.is_empty() { String::new() } else { format!("; {} of {} measured: {sum} bytes", known.len(), members.len()) };
        (None, Some(format!("Size unavailable: {why}{known_part}")))
    };
    let count = match kind {
        CardKind::Snapshots => members.iter().filter_map(|m| m.count).reduce(u64::saturating_add),
        _ => Some(members.len() as u64),
    };
    let observed_at = members.iter().filter_map(|m| m.observed_at.clone()).min();
    let insight_ids: Vec<String> = members.iter().filter_map(|m| m.insight_id.clone()).collect();
    let single_path = if members.len() == 1 { members[0].path.clone() } else { None };
    let link = match kind {
        CardKind::OldDownloads => CardLink { view: "insights.downloads".into(), path: single_path, insight_ids },
        _ if single_path.is_some() => CardLink { view: "analyze".into(), path: single_path, insight_ids },
        _ => CardLink { view: "insights".into(), path: None, insight_ids },
    };
    InsightCard { card_id: insight_id("card", &format!("{kind:?}")), kind, title: kind.title().into(), logical_bytes, count, size_note, observed_at, link }
}

/// DIN-03: overview cards from the known objects, the old-downloads review and measured tool directories. Cards
/// without members are left out, except old downloads (an empty review is a measured result).
pub fn overview_cards(known: &[Insight], downloads: &OldDownloadsReview, dirs: &[MeasuredDir]) -> Vec<InsightCard> {
    let mut groups: std::collections::BTreeMap<CardKind, Vec<Member>> = std::collections::BTreeMap::new();
    groups.insert(
        CardKind::OldDownloads,
        vec![Member {
            bytes: downloads.total_logical_bytes,
            count: Some(downloads.item_count),
            note: downloads.note.clone(),
            observed_at: Some(downloads.observed_at.clone()),
            path: downloads.root.clone(),
            insight_id: None,
        }],
    );
    for i in known {
        if let Some(kind) = CardKind::of_insight(&i.kind) {
            groups.entry(kind).or_default().push(Member {
                bytes: i.logical_bytes,
                count: i.count,
                note: i.size_note.clone(),
                observed_at: i.observed_at.clone(),
                path: i.path.clone(),
                insight_id: Some(i.insight_id.clone()),
            });
        }
    }
    for d in dirs {
        let (bytes, note) = match &d.size {
            Measured::Bytes { bytes, .. } => (Some(*bytes), None),
            Measured::Unknown(why) => (None, Some(format!("{}: {why}", d.label))),
        };
        groups.entry(d.card).or_default().push(Member { bytes, count: None, note, observed_at: Some(d.observed_at.clone()), path: Some(d.path.clone()), insight_id: None });
    }
    let mut cards: Vec<InsightCard> = groups.iter().map(|(k, m)| card(*k, m)).collect();
    if let Some(c) = cards.iter_mut().find(|c| c.kind == CardKind::OldDownloads) {
        c.count = Some(downloads.item_count);
        c.title = format!("Downloads older than {} days", downloads.older_than_days);
    }
    // Largest known size first, unknown after, then by kind.
    cards.sort_by(|a, b| b.logical_bytes.is_some().cmp(&a.logical_bytes.is_some()).then(b.logical_bytes.cmp(&a.logical_bytes)).then(a.kind.cmp(&b.kind)));
    cards
}

impl InsightCard {
    pub fn to_wire(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).expect("card serializes");
        v["logical_bytes"] = self.logical_bytes.map_or(serde_json::Value::Null, |b| serde_json::Value::String(b.to_string()));
        v
    }
}

/// Review text of the old-downloads list: how to act on a file without CleanManager choosing for the user.
pub const OLD_DOWNLOADS_GUIDANCE: &str =
    "Review only: nothing is selected. Pick files yourself and move them to the Recycle Bin with the trash command, or review installer packages with installer scan.";

impl OldDownloadsReview {
    pub fn to_wire(&self) -> serde_json::Value {
        let items: Vec<serde_json::Value> = self
            .items
            .iter()
            .map(|i| {
                serde_json::json!({"path": i.path, "logical_bytes": i.logical_bytes.to_string(), "modified_at": i.modified_at, "age_days": i.age_days,
                    "installer_like": i.installer_like, "selected": i.selected})
            })
            .collect();
        serde_json::json!({
            "root": self.root, "older_than_days": self.older_than_days, "observed_at": self.observed_at, "items": items,
            "total_logical_bytes": self.total_logical_bytes.map(|b| b.to_string()), "item_count": self.item_count, "note": self.note,
            "handling": self.handling, "guidance": OLD_DOWNLOADS_GUIDANCE,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(p: &str) -> RawPath {
        RawPath::windows(p.encode_utf16().collect()).unwrap()
    }

    /// DIN-04: the old-downloads review is size-sorted, age-filtered and has nothing selected.
    #[test]
    fn old_downloads_are_reviewed_not_preselected() {
        let now = 1_800_000_000;
        let day = 86_400;
        let e = |p: &str, size: u64, age: i64| DownloadEntry { path: raw(p), logical_bytes: size, modified_unix: now - age * day };
        let r = old_downloads(
            Some(raw(r"C:\Users\A\Downloads")),
            vec![e(r"C:\Users\A\Downloads\new.zip", 9_000, 3), e(r"C:\Users\A\Downloads\a.pdf", 10, 200), e(r"C:\Users\A\Downloads\setup.EXE", 5_000, 91), e(r"C:\Users\A\Downloads\b.mov", 7_000, 400)],
            now,
            90,
            "2026-10-15T00:00:00Z",
            None,
        );
        assert_eq!(r.items.iter().map(|i| i.logical_bytes).collect::<Vec<_>>(), [7_000, 5_000, 10]);
        assert!(r.items.iter().all(|i| !i.selected), "nothing preselected");
        assert!(r.items[1].installer_like && !r.items[0].installer_like);
        assert_eq!((r.total_logical_bytes, r.item_count, r.handling), (Some(12_010), 3, Handling::ManualTrashReview));
        assert_eq!(r.items[2].age_days, 200);
        let w = r.to_wire();
        assert!(w["items"].as_array().unwrap().iter().all(|i| i["selected"] == false));
        assert!(w.get("allowed_modes").is_none() && w["items"][0].get("default_selected").is_none());
    }

    /// DIN-03: each card shows a known size or unknown plus the observation time; snapshots without sizes show a count.
    #[test]
    fn cards_show_known_size_or_unknown_with_time() {
        let downloads = old_downloads(Some(raw(r"C:\D")), vec![], 1_800_000_000, 90, "2026-10-15T08:00:00Z", None);
        let known = vec![
            Insight::windows(WindowsObject::WslDisk, Some("Ubuntu"), "u", Some(raw(r"C:\wsl\ext4.vhdx")), Measured::Bytes { bytes: 40, basis: SizeBasis::FileMetadata }, None, "2026-10-15T08:00:03Z"),
            Insight::windows(WindowsObject::DockerDisk, None, "d", None, Measured::Unknown("access denied".into()), None, "2026-10-15T08:00:02Z"),
            Insight::windows(WindowsObject::ShadowStorage, Some("C:"), "c", None, Measured::Unknown("WMI unavailable".into()), Some(3), "2026-10-15T08:00:01Z"),
            Insight::windows(WindowsObject::HibernationFile, Some("C:"), "h", Some(raw(r"C:\hiberfil.sys")), Measured::Bytes { bytes: 7, basis: SizeBasis::FileMetadata }, None, "2026-10-15T08:00:04Z"),
        ];
        let dirs = vec![MeasuredDir {
            card: CardKind::DeveloperTools,
            label: "Cargo".into(),
            path: raw(r"C:\Users\A\.cargo"),
            size: Measured::Bytes { bytes: 100, basis: SizeBasis::DirectoryWalk },
            observed_at: "2026-10-15T08:00:05Z".into(),
        }];
        let cards = overview_cards(&known, &downloads, &dirs);
        let by = |k: CardKind| cards.iter().find(|c| c.kind == k).unwrap();
        let vm = by(CardKind::VirtualMachines);
        assert_eq!(vm.logical_bytes, None, "a partial sum is never shown as the total");
        let note = vm.size_note.as_deref().unwrap();
        assert!(note.contains("access denied") && note.contains("1 of 2 measured: 40"), "{note}");
        assert_eq!(vm.observed_at.as_deref(), Some("2026-10-15T08:00:02Z"), "oldest observation");
        let snap = by(CardKind::Snapshots);
        assert_eq!((snap.logical_bytes, snap.count), (None, Some(3)));
        assert!(snap.to_wire()["logical_bytes"].is_null());
        let dl = by(CardKind::OldDownloads);
        assert_eq!((dl.logical_bytes, dl.count, dl.link.view.as_str()), (Some(0), Some(0), "insights.downloads"));
        assert_eq!(dl.observed_at.as_deref(), Some("2026-10-15T08:00:00Z"));
        let dev = by(CardKind::DeveloperTools);
        assert_eq!((dev.logical_bytes, dev.link.view.as_str()), (Some(100), "analyze"));
        assert_eq!(dev.to_wire()["logical_bytes"], "100");
        assert!(cards.iter().all(|c| c.observed_at.is_some()));
        assert!(cards.iter().all(|c| c.logical_bytes.is_some() || c.size_note.is_some()));
        assert!(!cards.iter().any(|c| c.kind == CardKind::ModelLibraries), "no members, no card");
        // No Downloads folder: unknown, not 0.
        let none = old_downloads(None, vec![], 0, 90, "t", Some("no Downloads folder".into()));
        let c = overview_cards(&[], &none, &[]);
        assert_eq!(c[0].logical_bytes, None);
    }

    /// DIN-01/02: no known object offers a trash/delete route, every one explains itself, names a system or owner
    /// tool, and marks administrator steps.
    #[test]
    fn catalog_is_guidance_only() {
        for o in WindowsObject::ALL {
            let i = Insight::windows(o, None, "x", None, Measured::Unknown("n/a".into()), None, "2026-01-01T00:00:00Z");
            assert!(matches!(i.handling, Handling::ReadOnlyGuidance | Handling::OpenSystemSettings | Handling::OpenOwnerTool), "{:?}", o);
            assert!(!i.guidance.is_empty(), "{o:?} needs a system tool route");
            assert!(i.explanation.len() > 60);
            for step in &i.guidance {
                assert!(!step.tool.is_empty() && !step.steps.is_empty() && !step.impact.is_empty());
                let l = step.steps.to_lowercase();
                assert!(!l.contains("recycle") && !l.contains("del ") && !l.contains("rm "), "{o:?}: guidance must not be a file delete");
            }
            let w = i.to_wire();
            assert!(w.get("allowed_modes").is_none());
            assert!(w["logical_bytes"].is_null() && w["size_basis"] == "not_measured" && w["size_note"] == "n/a");
        }
        assert!(WindowsObject::ComponentStore.guidance().iter().any(|g| g.steps.contains("StartComponentCleanup") && g.requires_admin));
        assert!(WindowsObject::HibernationFile.guidance().iter().any(|g| g.steps == "powercfg /hibernate off" && g.requires_admin));
        assert!(WindowsObject::WslDisk.explanation().contains("deletes the distribution"));
    }

    #[test]
    fn sizes_and_ids() {
        let a = Insight::windows(WindowsObject::PageFile, Some("C:"), r"C:\pagefile.sys", None, Measured::Bytes { bytes: 42, basis: SizeBasis::FileMetadata }, None, "t");
        assert_eq!(a.to_wire()["logical_bytes"], "42");
        assert_eq!(a.title, "Paging file (C:)");
        assert_eq!(a.insight_id, insight_id("windows.page_file", r"c:\PAGEFILE.SYS"));
        let b = Insight::windows(WindowsObject::PageFile, Some("D:"), r"D:\pagefile.sys", None, Measured::Unknown("denied".into()), None, "t");
        assert_ne!(a.insight_id, b.insight_id);
        let c = Insight::windows(WindowsObject::SwapFile, None, "s", None, Measured::Bytes { bytes: 7, basis: SizeBasis::FileMetadata }, None, "t");
        let mut v = vec![b.clone(), c.clone(), a.clone()];
        sort(&mut v);
        assert_eq!(v.iter().map(|i| i.logical_bytes).collect::<Vec<_>>(), [Some(42), Some(7), None]);
    }
}
