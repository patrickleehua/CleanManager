//! Rule-root resolution on macOS (`~/Library/Caches`, `~/Library/Logs`, browser profiles, containers, package-manager caches, Trash, removable volumes) by file identity, with the test redirection of docs/platforms/macos.md §11.

use cm_core::capability::CapabilityState;

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`. Empty until
/// the module is implemented: its gated capabilities stay `unsupported` on macOS.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[];
