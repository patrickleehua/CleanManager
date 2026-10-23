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
mod gate;
mod host;
mod ops;
mod plat;
mod resident;
mod roots;
mod rules;
mod settings;
mod software;
mod suite;
mod system;
mod v2;
#[cfg(test)]
mod flow_tests;

use std::sync::Arc;

use tauri::{Manager, RunEvent, WindowEvent};

use crate::host::AppState;

/// D15: every IPC call passes the release-gate check first; a command of a capability that is off here is rejected
/// before it runs (`gate::refusal`), whatever the WebView shows.
fn gated<R: tauri::Runtime>(handler: impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static) -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    move |invoke| {
        let refused = {
            let webview = invoke.message.webview();
            let state = webview.state::<Arc<AppState>>();
            let payload = match invoke.message.payload() {
                tauri::ipc::InvokeBody::Json(v) => Some(v),
                _ => None,
            };
            gate::refusal(state.store_dir.as_deref(), invoke.message.command(), payload)
        };
        match refused {
            Some(e) => {
                invoke.resolver.reject(e);
                true
            }
            None => handler(invoke),
        }
    }
}

/// Test automation hook (debug builds only; see docs/verification-plan.md › 2.2). wry hands WebView2 its own
/// `AdditionalBrowserArguments`, so `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` never reaches the browser process and an
/// acceptance run cannot drive the WebView over CDP. With `CM_WEBVIEW_DEBUG_PORT=<1024..=65535>` a *debug* build
/// starts its windows with `--remote-debugging-port=<port>` (Chromium binds it to the loopback interface). The whole
/// module is compiled only with `debug_assertions`: a release build has no code that reads the variable or opens the
/// port, and the call in `main` is under the same `cfg`, so a release build without it would not compile.
///
/// R3-06: Chromium opens the port only when it can tell the default user-data directory, which it derives from the
/// Local AppData known folder (`%USERPROFILE%\AppData\Local`, expanded with this process's environment). An acceptance
/// run that points `USERPROFILE` at a test home without that folder got a browser process with the switch that never
/// listened and wrote no `DevToolsActivePort`, silently. The hook therefore creates that one empty folder when it is
/// missing (and says so). Only loopback pages may open a CDP WebSocket (`--remote-allow-origins`); clients that send no
/// `Origin` (Playwright, Python `websockets`) are unaffected.
#[cfg(debug_assertions)]
mod webview_debug {
    pub const ENV: &str = "CM_WEBVIEW_DEBUG_PORT";
    /// wry's own arguments (webview2/mod.rs): a window that names its arguments replaces them, so they are repeated.
    pub const WRY_DEFAULT_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection";

    /// The port to open, or `None` when the variable is unset or not a number in 1024..=65535 (never a system port).
    pub fn port(value: Option<&str>) -> Option<u16> {
        let port: u16 = value?.trim().parse().ok()?;
        (port >= 1024).then_some(port)
    }

    /// The browser arguments of a window: its own (or wry's defaults) plus the debugging port, open to loopback pages.
    pub fn browser_args(own: Option<&str>, port: u16) -> String {
        format!(
            "{} --remote-debugging-port={port} --remote-allow-origins=http://127.0.0.1:{port},http://localhost:{port}",
            own.unwrap_or(WRY_DEFAULT_ARGS)
        )
    }

    /// The Local AppData folder Chromium needs to resolve before it opens the port, when it is missing:
    /// `%USERPROFILE%\AppData\Local` (the known folder's default, expanded with this process's `USERPROFILE`).
    pub fn missing_local_app_data(user_profile: Option<&std::ffi::OsStr>) -> Option<std::path::PathBuf> {
        let local = std::path::Path::new(user_profile.filter(|p| !p.is_empty())?).join("AppData").join("Local");
        (!local.is_dir()).then_some(local)
    }

    /// Add the debugging port to every window of the configuration when the variable asks for it.
    pub fn apply(config: &mut tauri::Config) {
        let raw = std::env::var(ENV).ok();
        let Some(port) = port(raw.as_deref()) else {
            if let Some(v) = raw {
                eprintln!("{ENV}={v:?} ignored: expected a port number in 1024..=65535");
            }
            return;
        };
        if cfg!(windows)
            && let Some(local) = missing_local_app_data(std::env::var_os("USERPROFILE").as_deref())
        {
            match std::fs::create_dir_all(&local) {
                Ok(()) => eprintln!("{ENV}: created {} (empty): WebView2 opens its debugging port only when the Local AppData folder of USERPROFILE exists", local.display()),
                Err(e) => eprintln!("{ENV}: {} is missing and could not be created ({e}); WebView2 will not open the debugging port", local.display()),
            }
        }
        for w in &mut config.app.windows {
            w.additional_browser_args = Some(browser_args(w.additional_browser_args.as_deref(), port));
        }
        eprintln!("debug build: WebView remote debugging on 127.0.0.1:{port} ({ENV}); release builds never open it");
    }
}

fn main() {
    // The short-lived elevated maintenance worker (MNT-04): this executable started by Windows after one UAC prompt,
    // with the hidden worker argument. It serves the granted tasks of one plan to its parent and exits; no window.
    #[cfg(windows)]
    if let Some(code) = system::worker_main() {
        std::process::exit(code);
    }
    #[cfg_attr(not(debug_assertions), allow(unused_mut))]
    let mut context = tauri::generate_context!();
    #[cfg(debug_assertions)]
    webview_debug::apply(context.config_mut());
    tauri::Builder::default()
        // Used from Rust only (choose_folder); the capability grants the WebView no dialog API.
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let state = Arc::new(AppState::init(rules::locate()));
            // Optional tray (RES-01): off by default; the window starts hidden and is shown here unless the
            // tray is on and this start came from the CleanManager-owned startup value (`--resident`).
            let resident = Arc::new(resident::Resident::for_this_platform(state.store_dir.as_deref()));
            app.manage(state);
            app.manage(resident.clone());
            resident::setup(app.handle(), &resident);
            // D15: the experimental-features switch is shared with the CLI; a change made there takes effect here.
            resident::watch_features(app.handle(), &resident);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // A macOS clean-screen window closes normally (that ends clean-screen mode).
                if resident::is_clean_screen_window(window.label()) {
                    return;
                }
                // Tray on: closing the window hides it; the tray's "退出" really quits.
                if resident::hide_on_close(window.app_handle()) {
                    api.prevent_close();
                    let _ = window.hide();
                    return;
                }
                let state = window.state::<Arc<AppState>>();
                if host::close_requested(window.app_handle(), state.inner()) {
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(gated(tauri::generate_handler![
            commands::capabilities,
            gate::set_experimental_features,
            commands::list_scan_roots,
            commands::choose_folder,
            commands::recent_scans,
            commands::start_scan,
            commands::scan_status,
            commands::cancel_scan,
            commands::list_entries,
            commands::treemap_layer,
            commands::summarize_selection,
            commands::repair_index,
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
            suite::trash_list,
            suite::trash_purge_plan,
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
            system::maintenance_restore_plan,
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
            resident::resident_status,
            resident::resident_save,
            resident::resident_keep_awake,
            resident::resident_clean_screen,
            resident::resident_privacy,
            resident::resident_batteries,
        ]))
        .build(context)
        .expect("error while building CleanManager desktop")
        .run(|app, event| {
            // Exit: stop the tray's sampling and release any keep-awake request (RES-02).
            if let RunEvent::Exit = event {
                app.state::<Arc<resident::Resident>>().shutdown();
            }
            // macOS: clicking the Dock icon brings back a window hidden to the menu bar.
            #[cfg(target_os = "macos")]
            if let RunEvent::Reopen { has_visible_windows: false, .. } = event {
                resident::show_main(app);
            }
        });
}

#[cfg(test)]
mod webview_debug_tests {
    use super::webview_debug::{ENV, WRY_DEFAULT_ARGS, browser_args, missing_local_app_data, port};

    #[test]
    fn the_debug_port_is_a_non_system_port_number_or_nothing() {
        assert_eq!(port(Some("9222")), Some(9222));
        assert_eq!(port(Some(" 65535 ")), Some(65535));
        assert_eq!(port(Some("1024")), Some(1024));
        for bad in ["1023", "0", "65536", "-1", "9222x", "", "port"] {
            assert_eq!(port(Some(bad)), None, "{bad}");
        }
        assert_eq!(port(None), None);
        assert_eq!(ENV, "CM_WEBVIEW_DEBUG_PORT");
    }

    #[test]
    fn wry_defaults_are_kept_and_the_port_is_added() {
        let loopback = |p: u16| format!("--remote-allow-origins=http://127.0.0.1:{p},http://localhost:{p}");
        assert_eq!(browser_args(None, 9222), format!("{WRY_DEFAULT_ARGS} --remote-debugging-port=9222 {}", loopback(9222)));
        assert_eq!(browser_args(Some("--foo"), 9300), format!("--foo --remote-debugging-port=9300 {}", loopback(9300)));
        assert!(!browser_args(None, 9222).contains('*'), "never any origin");
    }

    /// R3-06: a redirected USERPROFILE without `AppData\Local` is the case where WebView2 silently does not listen.
    #[test]
    fn a_test_home_without_local_app_data_is_found() {
        let home = std::env::temp_dir().join(format!("cm-webview-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        assert_eq!(missing_local_app_data(Some(home.as_os_str())), Some(home.join("AppData").join("Local")));
        std::fs::create_dir_all(home.join("AppData").join("Local")).unwrap();
        assert_eq!(missing_local_app_data(Some(home.as_os_str())), None);
        assert_eq!(missing_local_app_data(None), None);
        assert_eq!(missing_local_app_data(Some(std::ffi::OsStr::new(""))), None);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Release builds never contain the hook: every line that names the variable or the switch is inside the
    /// `#[cfg(debug_assertions)]` module (or this test module), and its one call in `main` is under the same `cfg`.
    #[test]
    fn the_hook_exists_only_in_debug_builds() {
        let src = include_str!("main.rs");
        // The module with its doc comment: only `///` lines between the comment's start and the cfg-gated `mod`.
        let module = src.find("#[cfg(debug_assertions)]\nmod webview_debug {").expect("the hook module is cfg(debug_assertions)");
        let start = src.find("/// Test automation hook").expect("doc comment");
        assert!(src[start..module].lines().all(|l| l.starts_with("///")), "nothing but its doc comment precedes the module");
        let end = module + src[module..].find("\n}\n").expect("module end");
        let tests = src.find("#[cfg(test)]\nmod webview_debug_tests {").expect("tests");
        for needle in ["CM_WEBVIEW_DEBUG_PORT", "remote-debugging-port", "additional_browser_args"] {
            for (at, _) in src.match_indices(needle) {
                assert!((start..end).contains(&at) || at > tests, "{needle} outside the debug-only module at byte {at}");
            }
        }
        assert!(src.contains("    #[cfg(debug_assertions)]\n    webview_debug::apply(context.config_mut());"), "the call is debug-only too");
        // Tests run with debug assertions, so the module is compiled (and these tests run) here.
        const { assert!(cfg!(debug_assertions)) };
    }
}
