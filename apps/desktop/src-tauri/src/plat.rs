//! The platform backends of the desktop host, chosen once here. Each function mirrors the CLI's per-platform arm of the
//! same operation (`crates/cm-cli/src/*_cmd.rs`), so a desktop command and its CLI command reach the same
//! `cm_platform` backend: Windows (`cm_platform::windows`) or macOS (`cm_platform::macos`). Modules whose API is the same
//! on both platforms are reached through [`os`]; everything that differs in shape or wording is a function below.
//!
//! macOS specifics this build keeps (as the CLI does): there is no privilege path (administrator maintenance tasks are
//! skipped `elevation_unavailable`, `requires_admin` plans are refused), startup items are listed only (no toggle
//! backend), and the Trash takes single verified files (a folder is refused before planning).

#[cfg(not(any(windows, target_os = "macos")))]
compile_error!("the CleanManager desktop host is built for Windows and macOS only");

pub use imp::*;

/// A per-action gate consulted before every action of a plan/v1 apply (purge version control, attached images).
pub type ActionGate = Box<dyn Fn(&cm_core::execute::ActionTarget<'_>) -> Result<(), String>>;

/// A rule-root resolution problem shown with the analysis (code, rule, message).
pub type ResolveIssue = (String, String, String);

#[cfg(windows)]
mod imp {
    use std::path::Path;
    use std::time::Duration;

    use cm_core::path::RawPath;
    pub use cm_platform::windows as os;

    use super::{ActionGate, ResolveIssue};

    // ---- scan, file actions, recovery ------------------------------------------------------------

    pub fn scan_backend() -> Box<dyn cm_core::scan::ScanBackend + Send> {
        Box::new(os::directory::DirectoryBackend)
    }

    /// Verified Windows actions with the store's quarantine (`cleanmanager plan apply`).
    pub fn file_actions(store_dir: &Path) -> impl cm_core::execute::FileActions {
        os::actions::WindowsActions::with_store(store_dir.to_path_buf())
    }

    pub fn recovery_actions() -> impl cm_core::recovery::RecoveryActions {
        os::actions::recovery::WindowsRecovery
    }

    /// Restore/purge of quarantined objects is implemented and verified here.
    pub const RESTORE_VERIFIED: bool = true;

    /// PUR-03: purge plans re-read version-control state per artifact before acting (other plans pass through).
    pub fn apply_gate(plan: &cm_core::plan::Plan) -> Option<ActionGate> {
        Some(Box::new(os::purge::vcs_gate(plan)))
    }

    // ---- analysis --------------------------------------------------------------------------------

    /// Rule roots (`within`, the scan's root, only narrows the lookup on macOS).
    pub fn resolve(rules: &[cm_core::rules::Rule], within: Option<&cm_core::path::RawPath>) -> (Vec<cm_core::analysis::ResolvedRoot>, Vec<ResolveIssue>) {
        let _ = within;
        (os::resolve::resolve(rules, &os::resolve::ResolverEnv::from_os()), Vec::new())
    }

    // ---- plan/v2 backends --------------------------------------------------------------------------

    /// Whether this process may change machine-wide entries (`None` where not determined).
    pub fn is_admin() -> Option<bool> {
        os::startup::StartupEnv::from_process().ok().map(|e| e.is_admin)
    }

    /// Run `f` with the platform backends of plan/v2 kinds, as `cleanmanager plan apply` builds them.
    pub fn with_backends<T>(stores: &cm_store::Stores, f: impl FnOnce(&cm_app::plans::V2Backends<'_>) -> T) -> Result<T, String> {
        use os::startup_actions::WindowsStartup;
        let env = os::startup::StartupEnv::from_process()?;
        let startup = WindowsStartup { env };
        let files = file_actions(&stores.dir);
        let user = cm_platform::protection::load(&stores.dir);
        let holders = |paths: &[RawPath]| -> Option<Vec<String>> { os::restart_manager::holders(paths).ok().map(|h| h.iter().map(|x| x.describe()).collect()) };
        let protect_item = |item_id: &str| -> Result<(), String> {
            let user = user.as_ref().map_err(|e| format!("protection_unavailable: {e}"))?;
            match cm_core::protect::evaluate_task(item_id, user) {
                cm_core::protect::ProtectionDecision::Allowed => Ok(()),
                denied => Err(format!("{denied:?}")),
            }
        };
        let processes = os::processes::WindowsProcesses;
        let updates = os::updates::WindowsUpdates;
        // UNI-02: this window never confirms ending a process; such plans are refused before they start (`precheck`), and
        // if one got here anyway the close request is all that happens.
        let uninstall = os::uninstall::WindowsUninstall {
            env: os::uninstall::UninstallEnv::from_process()?,
            confirm: Box::new(|_: &[cm_core::uninstall::RunningProcess]| false),
        };
        let b = cm_app::plans::V2Backends {
            files: &files,
            startup: Some(&startup),
            holders: &holders,
            protect_item: &protect_item,
            processes: Some(&processes),
            updates: Some(&updates),
            uninstall: Some(&uninstall),
        };
        Ok(f(&b))
    }

    // ---- maintenance -------------------------------------------------------------------------------

    /// The fixed maintenance tasks of this platform and whether this process is already elevated.
    pub fn maintenance_tasks() -> (Box<dyn cm_core::maintenance::TaskBackend>, bool) {
        let tasks = os::maintenance::WindowsTasks::real();
        let is_admin = tasks.is_admin;
        (Box::new(tasks), is_admin)
    }

    /// Reads and writes the settings of setting tasks (14.5); none on Windows.
    pub fn settings_backend() -> Option<Box<dyn cm_core::maintenance::settings::SettingsActions>> {
        None
    }

    /// The catalog's `elevation` field: how administrator tasks of a run would be authorized here.
    pub fn elevation_word(is_admin: bool) -> &'static str {
        if is_admin { "already_elevated" } else { "one_prompt_per_run" }
    }

    /// One short-lived worker per run, started through one UAC prompt (the CLI's `WorkerElevation`): this executable
    /// with the hidden worker argument (see `main.rs`).
    pub fn maintenance_elevation() -> Result<Box<dyn cm_app::maintenance::Elevation>, String> {
        let exe = std::env::current_exe().map_err(|e| format!("own executable unknown: {e}"))?;
        Ok(Box::new(elevation::WorkerElevation { exe }))
    }

    mod elevation {
        use std::path::PathBuf;
        use std::time::Duration;

        use cm_app::maintenance::{ElevatedSession, Elevation, ElevationRefused};
        use cm_core::maintenance::{TaskRun, TaskSpec, WorkerGrant};
        use cm_platform::windows::elevation::{StartError, UacLauncher, WorkerConfig, WorkerConnection, start_worker};

        pub struct WorkerElevation {
            pub exe: PathBuf,
        }

        struct Session(WorkerConnection);

        impl ElevatedSession for Session {
            fn run(&mut self, task: &TaskSpec) -> Result<TaskRun, String> {
                self.0.run(task)
            }
            fn close(self: Box<Self>) {
                self.0.close();
            }
        }

        impl Elevation for WorkerElevation {
            fn start(&self, grant: &WorkerGrant) -> Result<Box<dyn ElevatedSession + '_>, ElevationRefused> {
                let cfg = WorkerConfig { exe: self.exe.clone(), connect_timeout: Duration::from_secs(60), idle_seconds: cm_platform::windows::elevation::DEFAULT_IDLE_SECONDS };
                match start_worker(&UacLauncher, &cfg, grant) {
                    Ok(c) => Ok(Box::new(Session(c))),
                    Err(StartError::Declined) => Err(ElevationRefused::Declined("the Windows administrator prompt was declined".into())),
                    Err(StartError::Unavailable(e)) => Err(ElevationRefused::Unavailable(e)),
                }
            }
        }
    }

    // ---- processes ---------------------------------------------------------------------------------

    fn apps_map() -> Vec<(String, String)> {
        os::apps::inventory().0.into_iter().filter_map(|a| a.install_location.map(|l| (a.name, l.display()))).collect()
    }

    /// Every process explained from local facts (path, signer, parent, owning app, known purpose).
    pub fn process_views(window: Duration, pins: &[String]) -> Result<Vec<cm_core::process::ProcessView>, String> {
        let facts = os::processes::list_with(window, true)?;
        Ok(cm_core::process::explain(facts, std::process::id(), &os::processes::windows_dir(), &apps_map(), pins))
    }

    /// One running process re-read for an end plan, with the SHA-256 of its executable when it can be read.
    pub fn process_for_end(pid: u32) -> Result<Option<(cm_core::process::ProcessView, Option<String>)>, String> {
        use os::processes::{file_sha256, list_with, signature_of, windows_dir};
        let facts = list_with(Duration::from_millis(200), false)?;
        let Some(mut f) = facts.into_iter().find(|f| f.pid == pid) else { return Ok(None) };
        let image = f.image_path.as_ref().map(RawPath::display);
        if let Some(i) = &image {
            f.signature = signature_of(i);
        }
        let row = cm_core::process::explain(vec![f], std::process::id(), &windows_dir(), &apps_map(), &[]).remove(0);
        Ok(Some((row, image.as_deref().and_then(file_sha256))))
    }

    /// Per-process CPU (share of the whole machine) for sustained-CPU alerts; `None` when the list cannot be read.
    pub fn process_cpu() -> Option<Vec<cm_core::alerts::ProcessCpu>> {
        let procs = os::processes::list_with(Duration::from_millis(500), false).ok()?;
        Some(
            procs
                .into_iter()
                .filter(|f| f.pid != 0)
                .map(|f| cm_core::alerts::ProcessCpu { pid: f.pid, image_path: f.image_path.as_ref().map(|i| i.display()), name: f.name, cpu_percent: f.cpu_percent })
                .collect(),
        )
    }

    // ---- manual trash ------------------------------------------------------------------------------

    /// MTR-03: whether the volume of `path` has a usable recycle bin (otherwise a plan is refused; never a delete).
    pub fn bin_ready(path: &RawPath) -> Result<(), String> {
        os::actions::recycle_bin_ready(path)
    }

    /// The recycle bin of the user profile's volume (readiness of trash-like operations).
    pub fn bin_for_profile() -> Result<(), String> {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| {
                std::env::var_os("SystemDrive").map(|d| {
                    let mut d = d;
                    d.push("\\");
                    d
                })
            })
            .ok_or("no user profile or system drive")?;
        let raw = cm_platform::paths::raw_path(&home).map_err(|e| e.to_string())?;
        os::actions::recycle_bin_ready(&raw)
    }

    /// Processes holding these objects open (Restart Manager) and a note on what could not be seen (none here).
    pub fn holders(paths: &[RawPath]) -> (Option<Vec<String>>, Option<String>) {
        (os::restart_manager::holders(paths).ok().map(|h| h.iter().map(|x| x.describe()).collect()), None)
    }

    /// Objects this platform's trash action cannot take beyond the core refusals (none on Windows).
    pub fn trash_refusal(_: &cm_core::manual_trash::Inspection) -> Option<cm_core::manual_trash::TrashRefusal> {
        None
    }

    /// Whether objects of this kind may meet [`trash_refusal`] (so planning inspects them first): never on Windows.
    pub fn trash_may_refuse(_: cm_core::model::ObjectKind) -> bool {
        false
    }

    /// Guidance added after the "plan it" line of a trashable object (none on Windows).
    pub const PUT_BACK_HINT: Option<&str> = None;

    // ---- installer discovery -----------------------------------------------------------------------

    pub const INSTALLER_LABELS: &[&str] = &[os::installer::SOURCE_DOWNLOADS, os::installer::SOURCE_DESKTOP, os::installer::SOURCE_BROWSER];
    pub const EJECT_NOTE: &str = "attached: eject it in File Explorer (right-click > Eject) or with Dismount-DiskImage, then scan again";

    pub fn installer_probe() -> os::installer::WinInstallerProbe {
        os::installer::WinInstallerProbe
    }

    pub fn signer_note(kind: cm_core::installer::InstallerKind, signer: Option<&str>) -> String {
        match signer {
            Some(s) => format!("signed by {s}"),
            None if kind.signable() => "no verified Authenticode signer".into(),
            None => "images carry no Authenticode signer".into(),
        }
    }

    // ---- inventories -------------------------------------------------------------------------------

    /// The installed-application inventory (`apps list`).
    pub fn apps_inventory() -> (Vec<cm_core::apps::AppInstance>, Vec<cm_core::apps::InventoryIssue>) {
        os::apps::inventory()
    }

    // ---- shell -------------------------------------------------------------------------------------

    /// Start File Explorer with the object selected (no shell is involved).
    pub fn reveal(path: &Path) -> bool {
        use std::os::windows::process::CommandExt;
        let text = path.as_os_str();
        // Paths cannot contain quotes on Windows; refuse anything that could break out of the quoted argument.
        if text.to_string_lossy().contains('"') {
            return false;
        }
        let mut arg = std::ffi::OsString::from("/select,\"");
        arg.push(text);
        arg.push("\"");
        let windir = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Windows".into());
        std::process::Command::new(windir.join("explorer.exe")).raw_arg(arg).spawn().is_ok()
    }

    /// The fixed system page a target names: Settings › Apps or the Store's updates page; nothing else.
    pub fn system_page(target: &str) -> Option<&'static str> {
        match target {
            "apps_features" => Some(cm_core::uninstall::SETTINGS_URI),
            "store_updates" => Some(cm_core::updates::STORE_UPDATES_URI),
            _ => None,
        }
    }

    /// Open a fixed system page with File Explorer (no shell, no other target).
    pub fn open_uri(uri: &str) -> bool {
        let windir = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Windows".into());
        std::process::Command::new(windir.join("explorer.exe")).arg(uri).spawn().is_ok()
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::path::Path;
    use std::time::Duration;

    use cm_core::path::RawPath;
    pub use cm_platform::macos as os;

    use super::{ActionGate, ResolveIssue};

    // ---- scan, file actions, recovery ------------------------------------------------------------

    /// The portable POSIX walker, the CLI's default until the G0 decision between it and `mac.bulk`.
    pub fn scan_backend() -> Box<dyn cm_core::scan::ScanBackend + Send> {
        Box::new(os::posix::PosixBackend)
    }

    /// Descriptor-verified quarantine/delete behind the trusted-namespace gate (otherwise `race_safety_unavailable`) and
    /// identity-confirmed Trash moves; quarantine under this store when it is on the target's volume.
    pub fn file_actions(store_dir: &Path) -> impl cm_core::execute::FileActions {
        os::actions::file_actions(store_dir)
    }

    /// Descriptor-verified restore (exclusive rename, identity confirmed afterwards) and purge inside the quarantine.
    pub fn recovery_actions() -> impl cm_core::recovery::RecoveryActions {
        os::actions::recovery::recovery_actions()
    }

    pub const RESTORE_VERIFIED: bool = os::actions::recovery::RESTORE_VERIFIED;

    /// The execution-time gates of discovery plans (the CLI's `mac_vcs_gate`): purge plans (PUR-03) ask Git again for
    /// each artifact; installer plans (INC-04) re-read the attached disk images. `None` for every other plan.
    pub fn apply_gate(plan: &cm_core::plan::Plan) -> Option<ActionGate> {
        if os::purge::is_purge_plan(plan) {
            Some(Box::new(os::purge::vcs_gate(plan)))
        } else if os::installer::is_installer_plan(plan) {
            Some(Box::new(os::installer::mounted_gate(plan)))
        } else {
            None
        }
    }

    // ---- analysis --------------------------------------------------------------------------------

    /// Roots of the enabled rules at or below `within` (the scan's root), as `cleanmanager analyze` resolves them:
    /// analysis matches roots by identity inside the scan, so no other root is looked up (a privacy-protected location
    /// such as another app's container is never opened for nothing, docs/platforms/macos.md §6).
    pub fn resolve(rules: &[cm_core::rules::Rule], within: Option<&cm_core::path::RawPath>) -> (Vec<cm_core::analysis::ResolvedRoot>, Vec<ResolveIssue>) {
        let enabled: Vec<cm_core::rules::Rule> = rules.iter().filter(|r| r.status == cm_core::rules::RuleStatus::Enabled).cloned().collect();
        let r = os::resolve::resolve_in(&enabled, &os::resolve::ResolverEnv::from_os(), within);
        (r.roots, r.issues.into_iter().map(|i| (i.code.to_owned(), i.scope, i.message)).collect())
    }

    // ---- plan/v2 backends --------------------------------------------------------------------------

    /// Not determined: this build has no privilege path on macOS, so plans that need one are refused.
    pub fn is_admin() -> Option<bool> {
        None
    }

    /// Run `f` with the macOS backends of plan/v2 kinds, as `cleanmanager plan apply` builds them (startup changes have
    /// no backend in this build: such plans are refused as unsupported).
    pub fn with_backends<T>(stores: &cm_store::Stores, f: impl FnOnce(&cm_app::plans::V2Backends<'_>) -> T) -> Result<T, String> {
        let files = file_actions(&stores.dir);
        let user = cm_platform::protection::load(&stores.dir);
        let protect_item = |item_id: &str| -> Result<(), String> {
            let user = user.as_ref().map_err(|e| format!("protection_unavailable: {e}"))?;
            match cm_core::protect::evaluate_task(item_id, user) {
                cm_core::protect::ProtectionDecision::Allowed => Ok(()),
                denied => Err(format!("{denied:?}")),
            }
        };
        let holders = |paths: &[RawPath]| os::actions::trash::apply_holders(paths);
        let processes = os::processes::MacProcesses;
        let updates = os::updates::MacUpdates::from_process().ok();
        // UNI-02: this window never confirms ending a process (`precheck` refuses such plans first).
        let uninstall = os::uninstall::UninstallEnv::from_process().ok().map(|env| os::uninstall::MacUninstall::new(env, Box::new(|_: &[cm_core::uninstall::RunningProcess]| false)));
        let b = cm_app::plans::V2Backends {
            files: &files,
            startup: None,
            holders: &holders,
            protect_item: &protect_item,
            processes: Some(&processes),
            updates: updates.as_ref().map(|u| u as &dyn cm_core::updates::UpdateBackend),
            uninstall: uninstall.as_ref().map(|u| u as &dyn cm_core::uninstall::UninstallActions),
        };
        Ok(f(&b))
    }

    // ---- maintenance -------------------------------------------------------------------------------

    /// The fixed macOS tasks; never "already elevated", even when run as root (no privilege path in this build).
    pub fn maintenance_tasks() -> (Box<dyn cm_core::maintenance::TaskBackend>, bool) {
        (Box::new(os::maintenance::MacTasks::from_process()), false)
    }

    /// Reads and writes the preference values of setting tasks (14.5).
    pub fn settings_backend() -> Option<Box<dyn cm_core::maintenance::settings::SettingsActions>> {
        Some(Box::new(os::maintenance::MacSettings::from_process()))
    }

    pub fn elevation_word(_is_admin: bool) -> &'static str {
        "unavailable"
    }

    /// No privileged helper (never osascript or AuthorizationExecuteWithPrivileges): administrator tasks are skipped.
    pub fn maintenance_elevation() -> Result<Box<dyn cm_app::maintenance::Elevation>, String> {
        Ok(Box::new(cm_app::maintenance::NoElevation(
            "this build has no privilege path on macOS (no privileged helper; never osascript or AuthorizationExecuteWithPrivileges)",
        )))
    }

    // ---- processes ---------------------------------------------------------------------------------

    pub fn process_views(window: Duration, pins: &[String]) -> Result<Vec<cm_core::process::ProcessView>, String> {
        os::processes::views(window, true, pins)
    }

    /// One running process for an end plan; the plan binds PID, start time, executable and its SHA-256.
    pub fn process_for_end(pid: u32) -> Result<Option<(cm_core::process::ProcessView, Option<String>)>, String> {
        Ok(os::processes::view(pid)?.map(|row| {
            let digest = row.facts.image_path.as_ref().and_then(os::processes::file_sha256);
            (row, digest)
        }))
    }

    pub fn process_cpu() -> Option<Vec<cm_core::alerts::ProcessCpu>> {
        let procs = os::processes::list_with(Duration::from_millis(500), false).ok()?;
        Some(
            procs
                .into_iter()
                .filter(|p| !p.zombie)
                .map(|p| p.facts)
                .map(|f| cm_core::alerts::ProcessCpu { pid: f.pid, image_path: f.image_path.as_ref().map(|i| i.display()), name: f.name, cpu_percent: f.cpu_percent })
                .collect(),
        )
    }

    // ---- manual trash ------------------------------------------------------------------------------

    /// MTR-03: the Trash the system would use for `path`, or why there is none (the plan is refused; never a delete).
    pub fn bin_ready(path: &RawPath) -> Result<(), String> {
        os::actions::trash::trash_ready(path).map(|_| ())
    }

    /// The Trash of the home folder's volume (readiness of trash-like operations).
    pub fn bin_for_profile() -> Result<(), String> {
        let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).ok_or("no home folder")?;
        let raw = cm_platform::paths::raw_path(&home).map_err(|e| e.to_string())?;
        bin_ready(&raw)
    }

    /// Processes holding these objects open (open descriptors) and a note on the other users' processes not inspected.
    pub fn holders(paths: &[RawPath]) -> (Option<Vec<String>>, Option<String>) {
        match os::actions::trash::open_holders(paths) {
            Ok(h) => {
                let note = (h.uninspected > 0).then(|| format!("{} process(es) of other users could not be inspected; a holder among them would not be listed", h.uninspected));
                (Some(h.holders), note)
            }
            Err(e) => (None, Some(e)),
        }
    }

    /// A folder is refused: the macOS Trash action moves single verified files.
    pub fn trash_refusal(ins: &cm_core::manual_trash::Inspection) -> Option<cm_core::manual_trash::TrashRefusal> {
        os::actions::trash::folder_refusal(ins)
    }

    /// Whether objects of this kind may meet [`trash_refusal`] (so planning inspects them first): folders.
    pub fn trash_may_refuse(kind: cm_core::model::ObjectKind) -> bool {
        kind == cm_core::model::ObjectKind::Directory
    }

    /// Finder keeps the original location privately: where to look, and how to put it back.
    pub const PUT_BACK_HINT: Option<&str> = Some(cm_core::manual_trash::PUT_BACK_GUIDANCE);

    // ---- installer discovery -----------------------------------------------------------------------

    pub const INSTALLER_LABELS: &[&str] = os::installer::LABELS;
    pub const EJECT_NOTE: &str = os::installer::EJECT_NOTE;

    pub fn installer_probe() -> os::installer::MacInstallerProbe {
        os::installer::MacInstallerProbe::default()
    }

    pub fn signer_note(kind: cm_core::installer::InstallerKind, signer: Option<&str>) -> String {
        os::installer::signer_note(kind, signer)
    }

    // ---- inventories -------------------------------------------------------------------------------

    /// The read-only bundle inventory (`CM_UNINSTALL_TEST_ROOT` + `CM_UNINSTALL_TEST_HOME` redirect it for tests).
    pub fn apps_inventory() -> (Vec<cm_core::apps::AppInstance>, Vec<cm_core::apps::InventoryIssue>) {
        match os::apps::AppsEnv::from_process() {
            Ok(env) => os::apps::inventory(&env),
            Err(e) => (Vec::new(), vec![cm_core::apps::InventoryIssue { code: "invalid_argument".into(), source: "apps".into(), message: e }]),
        }
    }

    // ---- shell -------------------------------------------------------------------------------------

    /// `open -R`: Finder shows the object selected. The path is one argument, no shell is involved.
    pub fn reveal(path: &Path) -> bool {
        std::process::Command::new("/usr/bin/open").arg("-R").arg(path).spawn().is_ok()
    }

    /// The fixed system page a target names: Storage settings (where apps are listed and removed), the App Store's
    /// updates, Login Items or Software Update; nothing else.
    pub fn system_page(target: &str) -> Option<&'static str> {
        match target {
            "apps_features" => Some("x-apple.systempreferences:com.apple.settings.Storage"),
            "store_updates" => Some(cm_core::updates::mac::MAC_APP_STORE_UPDATES_URI),
            "login_items" => Some(os::startup::LOGIN_ITEMS_SETTINGS),
            "software_update" => Some(cm_core::updates::mac::SOFTWARE_UPDATE_URI),
            _ => None,
        }
    }

    /// `open URI` for one of the fixed pages (no shell, no other target).
    pub fn open_uri(uri: &str) -> bool {
        std::process::Command::new("/usr/bin/open").arg(uri).spawn().is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_pages_are_a_fixed_list() {
        assert!(system_page("apps_features").is_some() && system_page("store_updates").is_some());
        assert_eq!(system_page("file:///etc/passwd"), None);
        assert_eq!(system_page(""), None);
        #[cfg(windows)]
        assert_eq!((system_page("apps_features"), system_page("login_items")), (Some("ms-settings:appsfeatures"), None));
        #[cfg(target_os = "macos")]
        assert!(["apps_features", "store_updates", "login_items", "software_update"].iter().all(|t| system_page(t).is_some_and(|u| u.starts_with("x-apple.systempreferences:") || u.starts_with("macappstore:"))));
    }

    /// The trash seams: Windows takes folders as they are; macOS refuses a folder before planning and never reports
    /// "nobody holds it" for a path that cannot be inspected.
    #[test]
    fn trash_seams_follow_the_platform() {
        let ins = |kind: cm_core::model::ObjectKind| cm_core::manual_trash::Inspection {
            entry_id: 1, path: cm_core::path::RawPath::Unix(b"/tmp/x".to_vec()), kind, cloud: cm_core::model::CloudState::NotCloud, logical_bytes: Some(0), allocated_bytes: Some(0),
            files: 0, directories: 0, protected: vec![], protected_count: 0, gaps: vec![], gap_count: 0, refusal: None,
        };
        assert!(trash_refusal(&ins(cm_core::model::ObjectKind::File)).is_none());
        let folder = trash_refusal(&ins(cm_core::model::ObjectKind::Directory));
        assert_eq!(folder.map(|r| r.reason), if cfg!(windows) { None } else { Some("folder_move_unavailable") });
        assert_eq!(PUT_BACK_HINT.is_some(), cfg!(target_os = "macos"));
    }
}
