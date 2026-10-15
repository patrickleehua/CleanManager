//! Generates the app ACL: only the commands listed here get `allow-*` permissions, and the
//! capability file grants exactly these to the main window. Anything else is rejected by the
//! IPC layer before it reaches Rust code.

fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
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
    ])))
    .expect("tauri-build failed");
}
