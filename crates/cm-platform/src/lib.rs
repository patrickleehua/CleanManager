//! Platform access for CleanManager. Each OS module implements `cm_core` traits; the core
//! never calls OS APIs directly.

pub mod cleanable;
pub mod credentials;
pub mod host;
pub mod paths;
pub mod protection;
pub mod trash_readiness;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(windows)]
pub mod windows;

use cm_core::capability::{Capability, CapabilityProvider, CapabilityState, GATED, Platform};

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
        let declared = module_caps();
        let row = |name: &str| declared.iter().find(|(n, _, _)| *n == name).map(|&(n, state, reason)| Capability::new(n, state, Some(reason)));
        let mut caps: Vec<Capability> =
            names.iter().map(|n| row(n).unwrap_or_else(|| Capability::new(n, CapabilityState::Unverified, Some(NOT_YET_VERIFIED)))).collect();
        // Mole-parity tools (add-mole-parity-suite 18.5): implemented on Windows but not through their gate, so never
        // `available`; on macOS a gated capability stays unsupported until its module declares a row for it.
        // The experimental-features switch (`cm_app::features`) may further report them off on this machine.
        for g in GATED {
            let (name, gate) = (g.capability, g.gate);
            caps.push(if cfg!(windows) {
                Capability::new(name, CapabilityState::Unverified, Some(&format!("Implemented on Windows; gate {gate} not passed (no release, CI or real-device acceptance yet)")))
            } else {
                row(name).unwrap_or_else(|| Capability::new(name, CapabilityState::Unsupported, Some("Not implemented on macOS in this build")))
            });
        }
        // Rows a module declares beyond the base and gated names (for example a host readiness check).
        for &(name, state, reason) in &declared {
            if !caps.iter().any(|c| c.name == name) {
                caps.push(Capability::new(name, state, Some(reason)));
            }
        }
        caps
    }
}

/// Capability rows declared by the platform modules (`CAPS` of each macOS module; none on Windows, whose rows are fixed
/// above). A module adds its rows by filling its own `CAPS`; this list only changes when a module is added.
fn module_caps() -> Vec<(&'static str, CapabilityState, &'static str)> {
    #[cfg(target_os = "macos")]
    let lists: &[&[(&str, CapabilityState, &str)]] = &[
        macos::actions::CAPS,
        macos::actions::recovery::CAPS,
        macos::actions::trash::CAPS,
        macos::resolve::CAPS,
        macos::probes::CAPS,
        macos::apps::CAPS,
        macos::insights::CAPS,
        macos::purge::CAPS,
        macos::installer::CAPS,
        macos::status::CAPS,
        macos::processes::CAPS,
        macos::startup::CAPS,
        macos::maintenance::CAPS,
        macos::updates::CAPS,
        macos::uninstall::CAPS,
        macos::resident::CAPS,
        macos::caps::CAPS,
    ];
    #[cfg(not(target_os = "macos"))]
    let lists: &[&[(&str, CapabilityState, &str)]] = &[];
    lists.iter().flat_map(|l| l.iter().copied()).collect()
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

    #[test]
    fn every_capability_is_reported_once() {
        let caps = host().capabilities();
        let mut names: Vec<&str> = caps.iter().map(|c| c.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), caps.len());
        assert!(GATED.iter().all(|g| caps.iter().any(|c| c.name == g.capability)));
        let declared = module_caps();
        let mut names: Vec<&str> = declared.iter().map(|(n, _, _)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), declared.len(), "a capability row is declared by more than one module");
    }

    #[test]
    fn module_rows_have_reasons() {
        assert!(module_caps().iter().all(|(name, _, reason)| !name.is_empty() && !reason.is_empty()));
    }
}
