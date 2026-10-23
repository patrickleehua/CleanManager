//! Application services shared by the CLI and the desktop host. Business rules stay in
//! `cm-core`; this layer only wires backends, stores and the hosting process together.

pub mod clean_catalog;
pub mod diag;
pub mod execute;
pub mod features;
pub mod index_repair;
pub mod insights;
pub mod maintenance;
pub mod plans;
pub mod processes;
pub mod recovery;
pub mod removed;
pub mod scan;
pub mod selection;
pub mod trash;
pub mod uninstall;
pub mod updates;
