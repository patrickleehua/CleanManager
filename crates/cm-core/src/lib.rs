//! CleanManager core: domain types, protocol records and state machines.
//!
//! This crate must not depend on a GUI toolkit, terminal styling or a specific agent.
//! Platform access lives behind traits implemented in `cm-platform`.

pub mod analysis;
pub mod apps;
pub mod authz;
pub mod bytes;
pub mod c14n;
pub mod capability;
pub mod error;
pub mod execute;
pub mod host;
pub mod insights;
pub mod model;
pub mod obs;
pub mod path;
pub mod plan;
pub mod protect;
pub mod protocol;
pub mod recovery;
pub mod rules;
pub mod scan;
pub mod startup;
pub mod status;
pub mod time;

/// Version of the engine reported in machine records.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Machine protocol schema version (see `schemas/v1/protocol.schema.json`).
pub const SCHEMA_VERSION: &str = "1.0";
