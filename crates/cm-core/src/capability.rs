//! Capability reporting (SCN-02): every capability carries an explicit state and reason
//! instead of a single boolean, and unverified platforms never inherit another platform's claim.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Windows,
    Macos,
}

impl Platform {
    /// The protocol word (the serde name), for human-readable text.
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Windows => "windows",
            Platform::Macos => "macos",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Available,
    PermissionRequired,
    Unsupported,
    Unverified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    pub name: String,
    pub state: CapabilityState,
    pub reason: Option<String>,
}

impl Capability {
    pub fn new(name: &str, state: CapabilityState, reason: Option<&str>) -> Self {
        Self { name: name.to_owned(), state, reason: reason.map(str::to_owned) }
    }
}

/// A parity capability and the release gate (D15) it must pass before it may be on by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GatedCapability {
    pub capability: &'static str,
    pub gate: &'static str,
    /// The gate has passed with real-device evidence on both platforms. Passing a gate is this one field.
    pub gate_passed: bool,
}

const fn gated(capability: &'static str, gate: &'static str) -> GatedCapability {
    GatedCapability { capability, gate, gate_passed: false }
}

/// D15: "单项未通过则该能力关闭". Every capability here is off unless its gate passed or the person turned on
/// experimental features (`cm_app::features`). None has passed.
pub const GATED: &[GatedCapability] = &[
    gated("clean.catalog", "G1"),
    gated("purge.artifacts", "G1"),
    gated("installer.cleanup", "G1"),
    gated("trash.manual", "G1"),
    gated("insights.overview", "G1"),
    gated("apps.uninstall", "G3"),
    gated("apps.updates", "G3"),
    gated("startup.toggle", "G3"),
    gated("maintenance.run", "G3"),
    gated("status.metrics", "G3"),
    gated("process.end", "G3"),
    gated("resident.utilities", "G4"),
    gated("lifecycle.self_update", "G3"),
    gated("diag.ai", "AID"),
];

/// The gate entry of a capability, if it is gated.
pub fn gate_of(capability: &str) -> Option<&'static GatedCapability> {
    GATED.iter().find(|g| g.capability == capability)
}

/// Implemented by each platform backend; the core never assumes a capability is present.
pub trait CapabilityProvider {
    fn platform(&self) -> Platform;
    fn capabilities(&self) -> Vec<Capability>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_words_are_the_protocol_names() {
        for p in [Platform::Windows, Platform::Macos] {
            assert_eq!(serde_json::to_value(p).unwrap(), p.as_str());
        }
    }

    #[test]
    fn states_serialize_to_protocol_names() {
        let names: Vec<String> = [
            CapabilityState::Available,
            CapabilityState::PermissionRequired,
            CapabilityState::Unsupported,
            CapabilityState::Unverified,
        ]
        .iter()
        .map(|s| serde_json::to_value(s).unwrap().as_str().unwrap().to_owned())
        .collect();
        assert_eq!(names, ["available", "permission_required", "unsupported", "unverified"]);
    }

    #[test]
    fn no_gate_has_passed_and_names_are_unique() {
        assert!(GATED.iter().all(|g| !g.gate_passed));
        let mut names: Vec<&str> = GATED.iter().map(|g| g.capability).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), GATED.len());
        assert_eq!(gate_of("process.end").map(|g| g.gate), Some("G3"));
        assert!(gate_of("scan.win.directory").is_none());
    }
}
