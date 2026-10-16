//! Read-only application inventory on macOS: app bundles (Info.plist, bundle id, Team ID), Mac App Store receipts and Homebrew casks/formulae.

use cm_core::capability::CapabilityState;

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`. Empty until
/// the module is implemented: its gated capabilities stay `unsupported` on macOS.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[];
