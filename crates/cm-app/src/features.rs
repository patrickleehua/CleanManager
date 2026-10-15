//! The experimental-features switch (design D15: "单项未通过则该能力关闭"), shared by the CLI and the desktop.
//!
//! - The gate table is [`cm_core::capability::GATED`]. A capability there is on only when its gate passed or the
//!   person turned on experimental features on this machine; no gate has passed, so every one is off by default.
//! - The switch lives in `<store>/features.json` (this tool's own file, like `app-updates.json`), so the CLI and the
//!   desktop read the same value. A missing or unreadable file means off.
//! - [`check`] is the one place both entries ask "is capability X enabled"; callers refuse before doing any work.

use std::path::Path;

use cm_core::capability::{Capability, CapabilityState, GATED, GatedCapability, gate_of};
use cm_core::plan::v2::PlanKind;
use serde::{Deserialize, Serialize};

pub const FEATURES_FILE: &str = "features.json";

/// How to opt in from a terminal; named in every refusal.
pub const OPT_IN_COMMAND: &str = "cleanmanager experimental --state on";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureSettings {
    /// Capabilities whose release gate has not passed may be used on this machine. Off by default.
    #[serde(default)]
    pub experimental_features: bool,
}

pub fn load(store: Option<&Path>) -> FeatureSettings {
    store
        .and_then(|d| std::fs::read_to_string(d.join(FEATURES_FILE)).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Turn experimental features on or off; returns the stored settings.
pub fn set_experimental(store: &Path, on: bool) -> std::io::Result<FeatureSettings> {
    let s = FeatureSettings { experimental_features: on };
    std::fs::create_dir_all(store)?;
    let tmp = store.join(format!("{FEATURES_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&s).expect("feature settings serialize"))?;
    std::fs::rename(&tmp, store.join(FEATURES_FILE))?;
    Ok(s)
}

/// A gated capability that is off on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disabled {
    pub capability: &'static str,
    pub gate: &'static str,
}

impl std::fmt::Display for Disabled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} is off until release gate {} passes; experimental features are disabled. To evaluate it on this machine, turn them on with `{OPT_IN_COMMAND}` or 设置 › 实验功能 in the desktop app",
            self.capability, self.gate
        )
    }
}

fn enabled_with(g: &GatedCapability, settings: FeatureSettings) -> bool {
    g.gate_passed || settings.experimental_features
}

/// `Ok` when the capability is not gated, its gate passed, or experimental features are on.
pub fn check(store: Option<&Path>, capability: &str) -> Result<(), Disabled> {
    match gate_of(capability) {
        Some(g) if !enabled_with(g, load(store)) => Err(Disabled { capability: g.capability, gate: g.gate }),
        _ => Ok(()),
    }
}

pub fn enabled(store: Option<&Path>, capability: &str) -> bool {
    check(store, capability).is_ok()
}

/// The gated capability a plan/v2 kind belongs to. Cleanup plans (core, or catalog/purge/installer built through
/// gated commands) and recovery plans are core and stay usable.
pub fn plan_kind_capability(kind: PlanKind) -> Option<&'static str> {
    match kind {
        PlanKind::ManualSelection => Some("trash.manual"),
        PlanKind::Uninstall => Some("apps.uninstall"),
        PlanKind::Maintenance => Some("maintenance.run"),
        PlanKind::Startup => Some("startup.toggle"),
        PlanKind::Update => Some("apps.updates"),
        PlanKind::Process => Some("process.end"),
        PlanKind::Cleanup | PlanKind::Recovery => None,
    }
}

/// Reason a gated capability reports while it is off here.
pub fn off_reason(gate: &str) -> String {
    format!("off until gate {gate} passes; experimental features are disabled")
}

/// Report gated capabilities that are off on this machine as `unsupported` with [`off_reason`]. With the switch on
/// they keep the platform's report (`unverified` on Windows); nothing becomes `available` here.
pub fn apply_to(store: Option<&Path>, caps: &mut [Capability]) {
    let settings = load(store);
    for c in caps.iter_mut() {
        if let Some(g) = gate_of(&c.name)
            && !enabled_with(g, settings)
            && c.state != CapabilityState::Unsupported
        {
            c.state = CapabilityState::Unsupported;
            c.reason = Some(off_reason(g.gate));
        }
    }
}

/// One row per gated capability: its gate, whether it passed and whether it is on here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GateRow {
    pub capability: &'static str,
    pub gate: &'static str,
    pub gate_passed: bool,
    pub enabled: bool,
}

pub fn rows(store: Option<&Path>) -> Vec<GateRow> {
    let s = load(store);
    GATED.iter().map(|g| GateRow { capability: g.capability, gate: g.gate, gate_passed: g.gate_passed, enabled: enabled_with(g, s) }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let d = std::env::temp_dir().join(format!("cm-features-{tag}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn off_by_default_and_refusal_names_capability_gate_and_opt_in() {
        let d = temp("default");
        assert!(!load(Some(&d)).experimental_features);
        assert!(!load(None).experimental_features);
        let e = check(Some(&d), "maintenance.run").unwrap_err();
        assert_eq!((e.capability, e.gate), ("maintenance.run", "G3"));
        let msg = e.to_string();
        assert!(msg.contains("maintenance.run") && msg.contains("G3") && msg.contains(OPT_IN_COMMAND), "{msg}");
        // Core capabilities are never gated.
        assert!(check(Some(&d), "scan.win.directory").is_ok());
    }

    #[test]
    fn the_switch_persists_and_turns_every_gated_capability_on() {
        let d = temp("persist");
        set_experimental(&d, true).unwrap();
        assert!(load(Some(&d)).experimental_features);
        assert!(GATED.iter().all(|g| enabled(Some(&d), g.capability)));
        assert!(rows(Some(&d)).iter().all(|r| r.enabled && !r.gate_passed));
        set_experimental(&d, false).unwrap();
        assert!(GATED.iter().all(|g| !enabled(Some(&d), g.capability)));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_unreadable_settings_file_means_off() {
        let d = temp("corrupt");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(FEATURES_FILE), b"{not json").unwrap();
        assert!(!enabled(Some(&d), "diag.ai"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn capabilities_report_off_but_never_available() {
        let d = temp("caps");
        let mut caps = vec![
            Capability::new("process.end", CapabilityState::Unverified, Some("x")),
            Capability::new("execute.trash", CapabilityState::Unverified, Some("y")),
        ];
        apply_to(Some(&d), &mut caps);
        assert_eq!(caps[0].state, CapabilityState::Unsupported);
        assert_eq!(caps[0].reason.as_deref(), Some("off until gate G3 passes; experimental features are disabled"));
        assert_eq!(caps[1].state, CapabilityState::Unverified);
        set_experimental(&d, true).unwrap();
        let mut on = vec![Capability::new("process.end", CapabilityState::Unverified, Some("x"))];
        apply_to(Some(&d), &mut on);
        assert_eq!(on[0].state, CapabilityState::Unverified);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn every_gated_plan_kind_maps_to_a_gated_capability() {
        for k in [PlanKind::ManualSelection, PlanKind::Uninstall, PlanKind::Maintenance, PlanKind::Startup, PlanKind::Update, PlanKind::Process] {
            assert!(plan_kind_capability(k).and_then(gate_of).is_some(), "{k:?}");
        }
        assert!(plan_kind_capability(PlanKind::Cleanup).is_none() && plan_kind_capability(PlanKind::Recovery).is_none());
    }
}
