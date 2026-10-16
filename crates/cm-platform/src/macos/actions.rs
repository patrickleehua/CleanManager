//! Executor file actions on macOS (`execute.trash`, `execute.quarantine`, `execute.delete`). Until they are verified,
//! [`file_actions`] reports every live check as unsupported, so plans are skipped item by item and nothing is touched.

pub mod recovery;
pub mod trash;

use cm_core::capability::CapabilityState;
use cm_core::execute::{ActionOutcome, ActionTarget, FileActions, LiveCheck, Reconciled};
use cm_core::path::RawPath;
use cm_core::rules::Mode;

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[];

const UNSUPPORTED: &str = "file actions are not verified on this platform";

/// The executor's file actions for this store (`plan apply`, `operations reconcile`, plan/v2 backends).
pub fn file_actions(_store_dir: &std::path::Path) -> impl FileActions {
    Unsupported
}

struct Unsupported;

fn unsupported() -> ActionOutcome {
    ActionOutcome::Skipped { code: "unsupported".into(), message: UNSUPPORTED.into() }
}

impl FileActions for Unsupported {
    fn check(&self, _: &ActionTarget<'_>) -> LiveCheck {
        LiveCheck::Unsupported(UNSUPPORTED.into())
    }
    fn quarantine(&self, _: &ActionTarget<'_>, _: &RawPath, _: &str) -> ActionOutcome {
        unsupported()
    }
    fn trash(&self, _: &ActionTarget<'_>) -> ActionOutcome {
        unsupported()
    }
    fn delete(&self, _: &ActionTarget<'_>) -> ActionOutcome {
        unsupported()
    }
    fn quarantine_dir_for(&self, _: &ActionTarget<'_>, _: &str) -> Option<RawPath> {
        None
    }
    fn reconcile(&self, _: &ActionTarget<'_>, _: Mode, _: Option<&RawPath>) -> Reconciled {
        Reconciled::StillUnknown
    }
    fn free_space(&self, _: &RawPath) -> Option<u64> {
        None
    }
}
