//! Platform access for CleanManager. Each OS module implements `cm_core` traits; the core
//! never calls OS APIs directly.

pub mod credentials;
pub mod host;
pub mod paths;
pub mod protection;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(windows)]
pub mod windows;

use cm_core::capability::{Capability, CapabilityProvider, CapabilityState, Platform};

#[cfg(not(any(windows, target_os = "macos")))]
compile_error!("CleanManager supports Windows and macOS only.");

/// Capabilities that are designed but not yet backed by real-device evidence stay `unverified`.
const NOT_YET_VERIFIED: &str = "Not implemented or not verified on a real device in this build";

pub struct HostPlatform;

impl CapabilityProvider for HostPlatform {
    fn platform(&self) -> Platform {
        if cfg!(windows) { Platform::Windows } else { Platform::Macos }
    }

    fn capabilities(&self) -> Vec<Capability> {
        let names: &[&str] = if cfg!(windows) {
            &["scan.win.directory", "scan.win.ntfs_enum", "execute.trash", "execute.quarantine", "execute.delete", "recovery.restore", "recovery.purge"]
        } else {
            &["scan.mac.posix", "scan.mac.bulk", "execute.trash", "execute.quarantine", "execute.delete", "recovery.restore", "recovery.purge"]
        };
        let mut caps: Vec<Capability> = names.iter().map(|n| Capability::new(n, CapabilityState::Unverified, Some(NOT_YET_VERIFIED))).collect();
        // Mole-parity tools (add-mole-parity-suite 18.5): implemented on Windows but not through their gate, so never
        // `available`; the macOS backends do not exist in this build.
        for (name, gate) in PARITY {
            caps.push(if cfg!(windows) {
                Capability::new(name, CapabilityState::Unverified, Some(&format!("Implemented on Windows; gate {gate} not passed (no release, CI or real-device acceptance yet)")))
            } else {
                Capability::new(name, CapabilityState::Unsupported, Some("Not implemented on macOS in this build"))
            });
        }
        caps
    }
}

/// Parity capabilities and the gate (D15) each must pass before it may report `available`.
const PARITY: &[(&str, &str)] = &[
    ("clean.catalog", "G1"),
    ("purge.artifacts", "G1"),
    ("installer.cleanup", "G1"),
    ("trash.manual", "G1"),
    ("insights.overview", "G1"),
    ("apps.uninstall", "G3"),
    ("apps.updates", "G3"),
    ("startup.toggle", "G3"),
    ("maintenance.run", "G3"),
    ("status.metrics", "G3"),
    ("process.end", "G3"),
    ("resident.utilities", "G4"),
    ("lifecycle.self_update", "G3"),
    ("diag.ai", "AID"),
];

pub fn host() -> HostPlatform {
    HostPlatform
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_platform_matches_target() {
        let p = host().platform();
        assert_eq!(p, if cfg!(windows) { Platform::Windows } else { Platform::Macos });
    }

    #[test]
    fn nothing_claims_availability_without_evidence() {
        assert!(host().capabilities().iter().all(|c| c.state != CapabilityState::Available));
    }
}
