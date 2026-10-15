//! Windows backends.

pub mod actions;
pub mod adapters;
pub mod apps;
pub mod directory;
pub mod hard_deny;
mod handle;
pub mod insights;
pub mod installer;
pub mod ntfs;
pub mod privileges;
pub mod probes;
pub mod purge;
pub mod resolve;
pub mod startup;
pub mod status;
#[cfg(test)]
pub(crate) mod testcloud;
#[cfg(test)]
pub(crate) mod testvhd;
