//! System status sampling on macOS (host/process statistics, memory pressure, battery, network totals, zombie count).

use cm_core::capability::CapabilityState;

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`. Empty until
/// the module is implemented: its gated capabilities stay `unsupported` on macOS.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[];
