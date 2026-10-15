//! CleanManager desktop host (MVP 13.1–13.5). A thin Tauri shell over the shared services:
//! scan lifecycle, plan application and recovery from `cm-app`, plans/authorization/rules from
//! `cm-core`, revision-bound queries and the audit log from `cm-store`, platform access from
//! `cm-platform`. The WebView talks to the explicit commands registered below only (see
//! `build.rs` and `capabilities/main.json`); no plugin APIs are exposed to it, and no command
//! accepts a filesystem path or a plan body from it.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod args;
mod cleanup;
mod commands;
mod diag;
mod dto;
mod host;
mod ops;
mod roots;
mod rules;
mod settings;
mod software;
mod suite;
mod system;
mod v2;
#[cfg(all(test, windows))]
mod flow_tests;

use std::sync::Arc;

use tauri::{Manager, WindowEvent};

use crate::host::AppState;

fn main() {
    // The short-lived elevated maintenance worker (MNT-04): this executable started by Windows after one UAC prompt,
    // with the hidden worker argument. It serves the granted tasks of one plan to its parent and exits; no window.
    #[cfg(windows)]
    if let Some(code) = system::worker_main() {
        std::process::exit(code);
    }
    tauri::Builder::default()
        // Used from Rust only (choose_folder); the capability grants the WebView no dialog API.
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            app.manage(Arc::new(AppState::init(rules::locate())));
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let state = window.state::<Arc<AppState>>();
                if host::close_requested(window.app_handle(), state.inner()) {
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::capabilities,
            commands::list_scan_roots,
            commands::choose_folder,
            commands::recent_scans,
            commands::start_scan,
            commands::scan_status,
            commands::cancel_scan,
            commands::list_entries,
            commands::treemap_layer,
            commands::summarize_selection,
            cleanup::candidate_summary,
            cleanup::analyze_scan,
            cleanup::list_candidates,
            cleanup::create_plan,
            cleanup::plan_detail,
            cleanup::plan_items,
            cleanup::approve_plan,
            cleanup::revoke_plan,
            cleanup::list_policies,
            cleanup::set_policy_state_cmd,
            cleanup::create_policy_from_plan,
            ops::start_apply,
            ops::cancel_apply,
            ops::active_apply,
            ops::operation_detail,
            ops::list_operation_items,
            ops::reconcile_operation,
            ops::list_operations,
            ops::list_plans,
            ops::list_quarantine,
            ops::create_recovery_plan,
            suite::purge_roots,
            suite::purge_roots_add,
            suite::purge_roots_remove,
            suite::purge_scan,
            suite::installer_locations,
            suite::installer_scan,
            suite::create_catalog_plan,
            suite::trash_inspect,
            suite::trash_plan,
            suite::startup_list,
            suite::startup_plan,
            suite::insights_list,
            suite::status_snapshot,
            suite::readiness,
            suite::reveal_entry,
            software::uninstall_list,
            software::uninstall_plan,
            software::uninstall_leftovers,
            software::uninstall_leftovers_plan,
            software::uninstall_restore_plan,
            software::updates_list,
            software::updates_set_checks,
            software::updates_plan,
            software::open_system_page,
            system::insights_overview,
            system::downloads_trash_plan,
            system::maintenance_catalog,
            system::maintenance_search,
            system::maintenance_plan,
            system::maintenance_history,
            system::maintenance_diagnose,
            system::process_list,
            system::process_pin,
            system::process_end_plan,
            system::alerts_settings,
            system::alerts_save,
            settings::list_protection,
            settings::add_protection,
            settings::remove_protection,
            settings::list_apps,
            diag::diag_status,
            diag::diag_save_provider,
            diag::diag_choose_cli,
            diag::diag_remove_provider,
            diag::diag_set_key,
            diag::diag_forget_key,
            diag::diag_probe,
            diag::diag_prepare,
            diag::diag_follow_up,
            diag::diag_discard,
            diag::diag_confirm,
            diag::diag_cancel,
            diag::diag_get,
            diag::diag_list,
            diag::diag_save,
            diag::diag_remove,
            diag::diag_export,
            diag::diag_handoff,
        ])
        .run(tauri::generate_context!())
        .expect("error while running CleanManager desktop");
}
