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
        names.iter().map(|n| Capability::new(n, CapabilityState::Unverified, Some(NOT_YET_VERIFIED))).collect()
    }
}

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
