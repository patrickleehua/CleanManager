//! Generates the app ACL: only the commands listed here get `allow-*` permissions, and the
//! capability file grants exactly these to the main window. Anything else is rejected by the
//! IPC layer before it reaches Rust code. Why each command is safe is documented on its module
//! (`commands`, `cleanup`, `ops`, `settings`).

fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
        // Scan and index (read-only for user files).
        "capabilities",
        "list_scan_roots",
        "choose_folder",
        "recent_scans",
        "start_scan",
        "scan_status",
        "cancel_scan",
        "list_entries",
        "treemap_layer",
        "summarize_selection",
        // Candidates, plans and authorization (ids only; approval needs a review token + typed digest).
        "candidate_summary",
        "analyze_scan",
        "list_candidates",
        "create_plan",
        "plan_detail",
        "plan_items",
        "approve_plan",
        "revoke_plan",
        "list_policies",
        "set_policy_state_cmd",
        "create_policy_from_plan",
        // Execution, history and recovery (stored, authorized plans only).
        "start_apply",
        "cancel_apply",
        "active_apply",
        "operation_detail",
        "list_operation_items",
        "reconcile_operation",
        "list_operations",
        "list_plans",
        "list_quarantine",
        "create_recovery_plan",
        // Settings and read-only inventory.
        "list_protection",
        "add_protection",
        "remove_protection",
        "list_apps",
    ])))
    .expect("tauri-build failed");
}
