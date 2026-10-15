//! Application services shared by the CLI and the desktop host. Business rules stay in
//! `cm-core`; this layer only wires backends, stores and the hosting process together.

pub mod diag;
pub mod execute;
pub mod features;
pub mod insights;
pub mod maintenance;
pub mod plans;
pub mod processes;
pub mod recovery;
pub mod scan;
pub mod uninstall;
pub mod updates;
