//! Release gates in the desktop host (design D15: "单项未通过则该能力关闭"). Why this is safe:
//!
//! - Every IPC call passes [`refusal`] before its command runs (see the invoke handler in `main.rs`): a command that
//!   belongs to a capability whose gate has not passed is rejected unless the person turned on experimental features.
//!   The WebView also shows an explanatory page in place of such a tool, but nothing depends on that.
//! - `approve_plan` and `start_apply` check the stored plan/v2's kind ([`require_plan`]): a plan of a gated kind made
//!   while the switch was on cannot be approved or run after it is turned off.
//! - The switch is `cm_app::features` in the shared store, so the CLI and the desktop see one value.
//! - `set_experimental_features` changes only CleanManager's own `features.json`; turning it off also stops the
//!   resident utilities and removes their startup value, as turning the tray off does.
//! - A change made from the CLI is noticed by the running app within `resident::FEATURES_POLL`
//!   (`resident::watch_features`): off runs the same stop path, and the page re-reads the capabilities.

use std::sync::Arc;

use cm_core::error::ErrorCode;
use tauri::{AppHandle, Emitter, State};

use crate::commands::{St, blocking};
use crate::dto::{CapabilitiesDto, IpcError, IpcResult};
use crate::host::AppState;
use crate::resident::Resident;

/// The gated capability an IPC command belongs to. `catalog` is `create_catalog_plan`'s argument.
pub fn capability_of(command: &str, catalog: Option<&str>) -> Option<&'static str> {
    Some(match command {
        "clean_catalog" => "clean.catalog",
        "purge_roots" | "purge_roots_add" | "purge_roots_remove" | "purge_scan" => "purge.artifacts",
        "installer_locations" | "installer_scan" => "installer.cleanup",
        "create_catalog_plan" if catalog == Some("installer") => "installer.cleanup",
        "create_catalog_plan" => "purge.artifacts",
        "trash_inspect" | "trash_plan" | "trash_list" | "trash_purge_plan" => "trash.manual",
        "insights_list" | "insights_overview" | "downloads_trash_plan" => "insights.overview",
        "startup_list" | "startup_plan" => "startup.toggle",
        "status_snapshot" | "alerts_settings" | "alerts_save" => "status.metrics",
        "process_list" | "process_pin" | "process_end_plan" => "process.end",
        "maintenance_catalog" | "maintenance_search" | "maintenance_plan" | "maintenance_history" | "maintenance_diagnose" | "maintenance_restore_plan" => {
            "maintenance.run"
        }
        c if c.starts_with("uninstall_") => "apps.uninstall",
        "updates_list" | "updates_set_checks" | "updates_plan" => "apps.updates",
        c if c.starts_with("diag_") => "diag.ai",
        c if c.starts_with("resident_") => "resident.utilities",
        _ => return None,
    })
}

fn off_error(off: cm_app::features::Disabled) -> IpcError {
    IpcError::new(ErrorCode::Unsupported, "experimental_off", off.to_string())
}

/// The error an IPC call gets when its capability is off here, or `None` when it may run.
pub fn refusal(store_dir: Option<&std::path::Path>, command: &str, payload: Option<&serde_json::Value>) -> Option<IpcError> {
    let catalog = payload.and_then(|p| p.get("catalog")).and_then(serde_json::Value::as_str);
    let cap = capability_of(command, catalog)?;
    cm_app::features::check(store_dir, cap).err().map(off_error)
}

/// A stored plan/v2 of a gated kind is approved or applied only while its capability is on.
pub fn require_plan(s: &AppState, plan_id: &str) -> IpcResult<()> {
    let kind = s.with_stores(|st| Ok(st.audit.load_plan_v2(plan_id).ok().map(|p| p.body.plan_kind)))?;
    match kind.and_then(cm_app::features::plan_kind_capability) {
        Some(cap) => cm_app::features::check(s.store_dir.as_deref(), cap).map_err(off_error),
        None => Ok(()),
    }
}

/// Turn experimental features on or off. Off stops every resident utility and removes their startup value first.
#[tauri::command(rename_all = "snake_case")]
pub async fn set_experimental_features(app: AppHandle, state: St<'_>, resident: State<'_, Arc<Resident>>, enabled: bool) -> IpcResult<CapabilitiesDto> {
    let s = state.inner().clone();
    let r = resident.inner().clone();
    let a = app.clone();
    blocking(move || {
        let dir = s.store_dir.clone().ok_or_else(|| IpcError::new(ErrorCode::InvalidArgument, "no_store", "no per-user store directory; the setting cannot be saved"))?;
        if !enabled {
            crate::resident::turn_off_all(&a, &r).map_err(|e| IpcError::new(ErrorCode::InternalError, "internal", e))?;
        }
        let saved = cm_app::features::set_experimental(&dir, enabled).map_err(|e| IpcError::new(ErrorCode::IoError, "io_error", e.to_string()))?;
        r.note_features(saved);
        let _ = a.emit(crate::resident::EVENT_CHANGED, ());
        Ok(())
    })
    .await?;
    crate::commands::capabilities(state).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("cm-desk-gate-{tag}-{}-{nanos}", std::process::id()))
    }

    /// Every command the app registers (build.rs) is classified: gated with a known capability, or core on purpose.
    #[test]
    fn every_registered_command_is_gated_or_core() {
        const CORE: &[&str] = &[
            "capabilities", "list_scan_roots", "choose_folder", "recent_scans", "start_scan", "scan_status", "cancel_scan", "list_entries", "treemap_layer",
            "summarize_selection", "repair_index", "removed_entries", "candidate_summary", "analyze_scan", "list_candidates", "create_plan", "plan_detail", "plan_items", "approve_plan",
            "revoke_plan", "list_policies", "set_policy_state_cmd", "create_policy_from_plan", "start_apply", "cancel_apply", "active_apply",
            "operation_detail", "list_operation_items", "reconcile_operation", "list_operations", "list_plans", "list_quarantine",
            "create_recovery_plan", "readiness", "reveal_entry", "open_system_page", "list_protection", "add_protection", "remove_protection",
            "list_apps", "set_experimental_features",
        ];
        let build = include_str!("../build.rs");
        let start = build.find(".commands(&[").expect("command list");
        let names: Vec<&str> = build[start..].lines().filter_map(|l| l.trim().strip_prefix('"')).filter_map(|l| l.strip_suffix("\",")).collect();
        assert!(names.len() > 90, "{}", names.len());
        let acl = include_str!("../capabilities/main.json");
        for n in &names {
            match capability_of(n, None) {
                Some(cap) => assert!(cm_core::capability::gate_of(cap).is_some() && !CORE.contains(n), "{n} -> {cap}"),
                None => assert!(CORE.contains(n), "{n} is neither gated nor listed as core"),
            }
            assert!(acl.contains(&format!("\"allow-{}\"", n.replace('_', "-"))), "{n} missing from capabilities/main.json");
        }
        assert_eq!(capability_of("create_catalog_plan", Some("installer")), Some("installer.cleanup"));
        assert_eq!(capability_of("resident_keep_awake", None), Some("resident.utilities"));
    }

    #[test]
    fn gated_commands_are_refused_until_experimental_features_are_on() {
        let d = temp("refuse");
        let e = refusal(Some(&d), "process_end_plan", None).expect("off by default");
        assert_eq!((e.code, e.reason), (ErrorCode::Unsupported, "experimental_off"));
        assert!(e.message.contains("process.end") && e.message.contains("G3") && e.message.contains("cleanmanager experimental --state on"), "{}", e.message);
        let catalog = serde_json::json!({ "catalog": "installer" });
        assert!(refusal(Some(&d), "create_catalog_plan", Some(&catalog)).unwrap().message.contains("installer.cleanup"));
        assert!(refusal(Some(&d), "resident_keep_awake", None).is_some());
        assert!(refusal(None, "diag_status", None).is_some(), "no store: off");
        // Core commands are never refused.
        assert!(refusal(Some(&d), "start_scan", None).is_none() && refusal(Some(&d), "list_apps", None).is_none());

        cm_app::features::set_experimental(&d, true).unwrap();
        assert!(refusal(Some(&d), "process_end_plan", None).is_none());
        assert!(refusal(Some(&d), "resident_keep_awake", None).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }
}
