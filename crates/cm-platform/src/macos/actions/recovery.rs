//! Restore of quarantined objects on macOS (recovery/v1): put back by identity, never over a recreated name. Until it
//! is verified, [`recovery_actions`] touches nothing and every restore or purge is skipped as unsupported.

use cm_core::capability::CapabilityState;
use cm_core::execute::Reconciled;
use cm_core::path::RawPath;
use cm_core::recovery::{MetadataCoverage, QuarantineState, QuarantinedObject, RecoveryActions, RecoveryKind, RecoveryOutcome};

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[];

/// Whether restore/purge of quarantined objects is verified on macOS in this build (quarantine details report
/// `complete` only then).
pub const RESTORE_VERIFIED: bool = false;

const UNSUPPORTED: &str = "recovery actions are not verified on this platform";

/// The recovery actions for `recovery/v1` plans and their reconciliation.
pub fn recovery_actions() -> impl RecoveryActions {
    Unsupported
}

struct Unsupported;

impl RecoveryActions for Unsupported {
    fn probe(&self, _: &QuarantinedObject<'_>) -> QuarantineState {
        QuarantineState::Unknown
    }
    fn restore(&self, _: &QuarantinedObject<'_>) -> RecoveryOutcome {
        RecoveryOutcome::Skipped { code: "unsupported".into(), message: UNSUPPORTED.into() }
    }
    fn purge(&self, o: &QuarantinedObject<'_>) -> RecoveryOutcome {
        self.restore(o)
    }
    fn reconcile(&self, _: &QuarantinedObject<'_>, _: RecoveryKind) -> Reconciled {
        Reconciled::StillUnknown
    }
    fn free_space(&self, _: &RawPath) -> Option<u64> {
        None
    }
    fn metadata_coverage(&self) -> MetadataCoverage {
        MetadataCoverage { preserved: vec![], limits: vec![UNSUPPORTED] }
    }
}
