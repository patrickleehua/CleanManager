//! macOS backends (MVP task group 5): read-only scan backends `mac.posix` (readdir + fstatat)
//! and `mac.bulk` (getattrlistbulk with per-directory POSIX fallback). Both share one
//! fd-relative walker so their rows can be reconciled field by field. `hard_deny` resolves the
//! protection policy's platform hard-deny roots by identity.
//!
//! The parity backends (add-mole-parity-suite) each live in their own module and declare the capability rows they
//! back in a `CAPS` constant, which `HostPlatform::capabilities` concatenates; a module with an empty `CAPS` leaves its
//! gated capabilities `unsupported` on macOS.

pub mod actions;
pub mod apps;
pub mod bulk;
pub mod caps;
pub mod hard_deny;
pub mod insights;
pub mod installer;
pub mod maintenance;
pub mod maintenance_diag;
pub mod posix;
pub mod probes;
pub mod processes;
pub mod privacy;
pub mod purge;
pub mod resident;
pub mod resolve;
pub mod startup;
pub mod status;
mod sys;
#[cfg(test)]
mod tests;
pub mod uninstall;
pub mod updates;
mod walk;

// macOS foundations (work package 0B): FFI, identity text, processes, tool runner, test fixtures.
pub(crate) mod cmd;
pub(crate) mod ffi;
pub(crate) mod ids;
pub(crate) mod procs;
#[cfg(test)]
mod testfs;
