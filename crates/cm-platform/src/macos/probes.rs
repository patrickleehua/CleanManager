//! Read-only probes the cleanup catalog asks for (in use, running owner app, cloud/provider state) on macOS.

use cm_core::capability::CapabilityState;

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`. Empty until
/// the module is implemented: its gated capabilities stay `unsupported` on macOS.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[];
