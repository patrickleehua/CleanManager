//! App update channels on macOS (Sparkle appcast, Mac App Store, Homebrew; Electron detection only) (D8).

use cm_core::capability::CapabilityState;

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`. Empty until
/// the module is implemented: its gated capabilities stay `unsupported` on macOS.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[];
