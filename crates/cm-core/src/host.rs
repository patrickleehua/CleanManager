//! Task hosting (CLI-09): tasks run inside the CLI or desktop process that started them.
//! A host is identified by PID plus process creation time, so a reused PID is never mistaken
//! for the original host.

use serde::Serialize;

/// Whether the process that hosted a task is still alive. `Unknown` never ends a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Liveness {
    Alive,
    Exited,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostRef {
    /// `cli` or `desktop`.
    pub kind: String,
    pub pid: u32,
    /// Process creation time as reported by the OS (UTC RFC 3339, second precision).
    pub started_at: String,
}

/// Implemented by the platform layer.
pub trait HostProbe {
    fn current(&self, kind: &str) -> HostRef;
    fn liveness(&self, host: &HostRef) -> Liveness;
}
