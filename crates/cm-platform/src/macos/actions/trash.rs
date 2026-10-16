//! Moves to the user's Trash on macOS (NSFileManager trashItem / FSMoveObjectToTrash) with identity checks before and after (D6).

use cm_core::capability::CapabilityState;

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`. Empty until
/// the module is implemented: its gated capabilities stay `unsupported` on macOS.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[];
