//! Capability reporting (SCN-02): every capability carries an explicit state and reason
//! instead of a single boolean, and unverified platforms never inherit another platform's claim.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Windows,
    Macos,
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

/// Implemented by each platform backend; the core never assumes a capability is present.
pub trait CapabilityProvider {
    fn platform(&self) -> Platform;
    fn capabilities(&self) -> Vec<Capability>;
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
