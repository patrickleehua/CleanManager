//! Windows backends.

pub mod actions;
pub mod adapters;
pub mod apps;
pub mod directory;
pub mod elevation;
pub mod hard_deny;
mod handle;
pub mod insights;
pub mod installer;
pub mod maintenance;
pub mod maintenance_diag;
pub mod ntfs;
pub mod privileges;
pub mod probes;
pub mod processes;
pub mod purge;
pub mod resident;
pub mod resident_screen;
pub mod resolve;
pub mod restart_manager;
pub mod startup;
pub mod startup_actions;
pub mod status;
pub mod uninstall;
pub mod updates;
#[cfg(test)]
pub(crate) mod testcloud;
#[cfg(test)]
pub(crate) mod testvhd;
