//! Application services shared by the CLI and the desktop host. Business rules stay in
//! `cm-core`; this layer only wires backends, stores and the hosting process together.

pub mod scan;
