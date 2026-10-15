//! Windows backends.

pub mod actions;
pub mod apps;
pub mod directory;
pub mod hard_deny;
mod handle;
pub mod ntfs;
pub mod privileges;
pub mod resolve;
#[cfg(test)]
pub(crate) mod testcloud;
#[cfg(test)]
pub(crate) mod testvhd;
