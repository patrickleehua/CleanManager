//! CleanManager desktop host (MVP 13.1/13.2). A thin Tauri shell over the shared services:
//! scan lifecycle from `cm-app`, revision-bound queries from `cm-store`, platform access from
//! `cm-platform`. The WebView talks to the explicit commands in `commands.rs` only (see
//! `build.rs` and `capabilities/main.json`); no plugin APIs are exposed to it.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod args;
mod commands;
mod dto;
mod host;
mod recent;
mod roots;

use std::sync::Arc;

use tauri::{Manager, WindowEvent};

use crate::host::AppState;

fn main() {
    tauri::Builder::default()
        // Used from Rust only (choose_folder); the capability grants the WebView no dialog API.
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let recent = app.path().app_local_data_dir().ok().map(|d| d.join("recent-scans.json"));
            app.manage(Arc::new(AppState::init(recent)));
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running CleanManager desktop");
}
