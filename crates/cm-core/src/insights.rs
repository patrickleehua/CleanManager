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

#[cfg(test)]
mod tests {
    use super::*;

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
