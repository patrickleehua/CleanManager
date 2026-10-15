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
mod dto;
mod host;
mod ops;
mod roots;
mod rules;
mod settings;
#[cfg(all(test, windows))]
mod flow_tests;

use std::sync::Arc;

use tauri::{Manager, WindowEvent};

use crate::host::AppState;

fn main() {
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
            settings::list_protection,
            settings::add_protection,
            settings::remove_protection,
            settings::list_apps,
        ])
        .run(tauri::generate_context!())
        .expect("error while running CleanManager desktop");
}
