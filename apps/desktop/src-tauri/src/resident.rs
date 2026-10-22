//! 状态 › 常驻工具 (add-mole-parity-suite 15.1/15.2; RES-01..04, RES-07). Why each command is safe:
//!
//! - `resident_status`, `resident_privacy`, `resident_batteries` are read-only.
//! - `resident_save` changes only CleanManager's own `resident.json` and, when "start at login" is on, the one
//!   startup value CleanManager owns (`HKCU\…\Run\CleanManager.Resident`). Turning the tray off stops and joins its
//!   sampling thread, removes the tray icon and removes that value ([`cm_core::resident::turn_off`]).
//! - `resident_keep_awake` holds or releases a per-thread power request (no power plan or timeout is changed; the
//!   request ends when released, when its duration passes, when CleanManager exits).
//! - `resident_clean_screen` shows the clean-screen window until Esc is held 2 s or the timeout (5–300 s) passes.
//!
//! The tray only shows the compact status (CPU, memory, free space on the system disk, camera/microphone use)
//! and opens app pages from a fixed list; it never cleans, uninstalls or runs maintenance.
//!
//! D15 (the experimental-features switch, shared with the CLI): besides the tray's sampling loop the app runs one
//! more timer, [`watch_features`], which re-reads `<store>/features.json` every [`FEATURES_POLL`]. Turning the switch
//! off from the CLI (`cleanmanager experimental --state off`) therefore stops the tray, sampling, battery alerts and
//! keep-awake and removes the startup value within that time, as 设置 › 实验功能 does; turning it on only makes the
//! gated pages available again (nothing is started). Either way the page hears [`EVENT_FEATURES`].
//!
//! macOS (same commands; `cm_platform::macos::resident`): the tray is a menu-bar status item; keep-awake holds an
//! `IOPMAssertion` (PreventUserIdleDisplaySleep) released the same way; clean screen is one borderless, always-on-top
//! black webview window per display plus this app's presentation options (Dock and menu bar hidden, Command-Tab and
//! Command-H disabled), see `mac_screen`; start at login is refused as unsupported (SMAppService.mainApp needs a
//! signed bundle) and nothing is written instead.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cm_app::features::FeatureSettings;
use cm_core::error::ErrorCode;
use cm_core::resident::{BatteryAlert, DeviceBattery, PrivacySnapshot, ResidentLoop, ResidentSettings, StartupState, StartupStore, TraySummary, TurnOffReport};
#[cfg(target_os = "macos")]
use cm_platform::macos::{resident as plat, status as plat_status};
#[cfg(windows)]
use cm_platform::windows::{resident as plat, status as plat_status};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::blocking;
use crate::dto::{IpcError, IpcResult};
use crate::host::lock;

pub const TRAY_ID: &str = "cm-resident";
pub const EVENT_NAVIGATE: &str = "resident-navigate";
pub const EVENT_CHANGED: &str = "resident-changed";
pub const EVENT_ALERT: &str = "resident-alert";
/// The experimental-features switch changed outside this window (payload [`FeaturesEvent`]).
pub const EVENT_FEATURES: &str = "features-changed";
/// How often the running app re-reads the experimental-features switch (D15): a change made from the CLI takes
/// effect here within this time.
pub const FEATURES_POLL: Duration = Duration::from_secs(3);

fn bad(message: impl Into<String>) -> IpcError {
    IpcError::new(ErrorCode::InvalidArgument, "invalid_argument", message)
}

fn internal(message: impl Into<String>) -> IpcError {
    IpcError::new(ErrorCode::InternalError, "internal", message)
}

// ---------------------------------------------------------------------------------------------
// State and controller (testable without a window)

/// What the tray loop last saw; read by `resident_status` (never re-sampled for the page).
#[derive(Debug, Default, Clone, Serialize)]
pub struct Seen {
    pub summary: Option<TraySummary>,
    pub privacy: Option<PrivacySnapshot>,
    /// Battery alerts raised by the tray loop (newest first, this session only).
    pub alerts: Vec<BatteryAlert>,
}

pub struct Resident {
    /// The shared store: settings live here, and the experimental-features switch (D15) is read from it.
    store_dir: Option<PathBuf>,
    settings_path: Option<PathBuf>,
    pub settings: Mutex<ResidentSettings>,
    sampling: Mutex<Option<ResidentLoop>>,
    store: Box<dyn StartupStore>,
    /// The command written to the startup value.
    exe: String,
    pub seen: Arc<Mutex<Seen>>,
    /// "Quit" was chosen: the next window close really exits instead of hiding to the tray.
    pub quitting: AtomicBool,
    keep_awake: Mutex<Option<plat::KeepAwake>>,
    alerts: Mutex<cm_core::resident::BatteryAlerts>,
    /// The experimental-features switch as this app last saw it ([`Resident::recheck_features`]).
    features_seen: Mutex<FeatureSettings>,
    /// Stops the [`watch_features`] thread when dropped.
    features_watch: Mutex<Option<Sender<()>>>,
}

/// A change of the experimental-features switch found by [`Resident::recheck_features`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeaturesChange {
    pub experimental_features: bool,
    /// The resident utilities are no longer allowed, so [`Resident::stop_all`] ran (its result).
    pub stopped: Option<Result<TurnOffReport, String>>,
}

fn load(path: Option<&Path>) -> ResidentSettings {
    path.and_then(|p| std::fs::read_to_string(p).ok()).map(|t| ResidentSettings::parse(&t)).unwrap_or_default()
}

fn save(path: Option<&Path>, s: &ResidentSettings) -> Result<(), String> {
    let Some(path) = path else { return Err("no per-user store directory; settings cannot be saved".into()) };
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

impl Resident {
    pub fn new(store_dir: Option<&Path>, store: Box<dyn StartupStore>, exe: String) -> Resident {
        let settings_path = store_dir.map(|d| d.join(cm_core::resident::SETTINGS_FILE));
        let settings = load(settings_path.as_deref());
        Resident {
            store_dir: store_dir.map(Path::to_path_buf),
            settings_path,
            settings: Mutex::new(settings),
            sampling: Mutex::new(None),
            store,
            exe,
            seen: Arc::new(Mutex::new(Seen::default())),
            quitting: AtomicBool::new(false),
            keep_awake: Mutex::new(None),
            alerts: Mutex::new(Default::default()),
            features_seen: Mutex::new(cm_app::features::load(store_dir)),
            features_watch: Mutex::new(None),
        }
    }

    /// The real per-user store: the `Run` value on Windows; on macOS the login-item store, which reports start at
    /// login unsupported in this unsigned build and writes nothing.
    pub fn for_this_platform(store_dir: Option<&Path>) -> Resident {
        let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
        #[cfg(windows)]
        let store: Box<dyn StartupStore> = Box::new(plat::RunValue::real());
        #[cfg(target_os = "macos")]
        let store: Box<dyn StartupStore> = Box::new(plat::LoginItem);
        Resident::new(store_dir, store, exe)
    }

    pub fn sampling_running(&self) -> bool {
        lock(&self.sampling).as_ref().is_some_and(ResidentLoop::is_running)
    }

    /// Startup: make the startup value match the saved settings (a stale value from an earlier run is removed).
    pub fn reconcile_startup(&self) -> Result<StartupState, String> {
        let s = lock(&self.settings).clone();
        cm_core::resident::sync_startup(self.store.as_ref(), &s, &self.exe)
    }

    /// Apply new settings: save them, start sampling (`start`) when the tray turns on, and when it turns off stop
    /// and join sampling and remove the startup value. Returns the turn-off report when the tray was turned off.
    pub fn apply(&self, new: ResidentSettings, start: impl FnOnce() -> Result<ResidentLoop, String>) -> Result<Option<TurnOffReport>, String> {
        new.validate()?;
        save(self.settings_path.as_deref(), &new)?;
        *lock(&self.settings) = new.clone();
        if !new.tray {
            let mut sampling = lock(&self.sampling);
            let report = cm_core::resident::turn_off(&mut sampling, self.store.as_ref());
            *lock(&self.seen) = Seen::default();
            return Ok(Some(report));
        }
        {
            let mut sampling = lock(&self.sampling);
            if !sampling.as_ref().is_some_and(ResidentLoop::is_running) {
                *sampling = Some(start()?);
            }
        }
        cm_core::resident::sync_startup(self.store.as_ref(), &new, &self.exe)?;
        Ok(None)
    }

    /// Stop everything this component holds (app exit): sampling and any keep-awake request. The startup value
    /// and settings stay as the person chose. The switch watcher is told to end but not joined (a check in flight
    /// may be waiting for the main thread, which is running this).
    pub fn shutdown(&self) {
        drop(lock(&self.features_watch).take());
        if let Some(mut l) = lock(&self.sampling).take() {
            l.stop();
        }
        if let Some(mut k) = lock(&self.keep_awake).take() {
            k.release();
        }
    }

    pub fn startup_entry(&self) -> Option<String> {
        self.store.read().ok().flatten()
    }

    /// D15: the resident utilities run only while their capability is on (gate G4 passed or experimental features on).
    pub fn allowed(&self) -> bool {
        cm_app::features::enabled(self.store_dir.as_deref(), "resident.utilities")
    }

    /// Experimental features turned off (here or from the CLI): stop everything the resident utilities hold, as
    /// turning the tray off does — sampling, battery alerts, the startup value — and release any keep-awake request.
    /// Settings are saved off. Everything is stopped even when they cannot be saved; that error is returned after.
    pub fn stop_all(&self) -> Result<TurnOffReport, String> {
        let off = ResidentSettings { tray: false, start_at_login: false, battery_alerts: false, ..lock(&self.settings).clone() };
        let saved = save(self.settings_path.as_deref(), &off);
        *lock(&self.settings) = off;
        let report = cm_core::resident::turn_off(&mut lock(&self.sampling), self.store.as_ref());
        *lock(&self.seen) = Seen::default();
        if let Some(mut k) = lock(&self.keep_awake).take() {
            k.release();
        }
        saved.map(|()| report)
    }

    /// The switch this app wrote itself (设置 › 实验功能), so the watcher does not handle that change again.
    pub fn note_features(&self, now: FeatureSettings) {
        *lock(&self.features_seen) = now;
    }

    /// D15 from outside this window: re-read the shared switch. `None` when it is as this app last saw it. When it
    /// changed and the resident utilities are no longer allowed, everything they hold is stopped first
    /// ([`Resident::stop_all`]); turning it on starts nothing (the tray stays the person's choice).
    pub fn recheck_features(&self) -> Option<FeaturesChange> {
        let now = cm_app::features::load(self.store_dir.as_deref());
        {
            let mut seen = lock(&self.features_seen);
            if *seen == now {
                return None;
            }
            *seen = now;
        }
        let stopped = (!self.allowed()).then(|| self.stop_all());
        Some(FeaturesChange { experimental_features: now.experimental_features, stopped })
    }
}

/// Run [`Resident::recheck_features`] every `interval` on a thread of its own and call `changed` after each change
/// was handled. The thread ends when the Resident is gone or [`Resident::shutdown`] drops its stop handle.
pub fn start_features_watch(r: &Arc<Resident>, interval: Duration, changed: impl Fn(&FeaturesChange) + Send + 'static) -> std::io::Result<()> {
    let res = Arc::downgrade(r);
    let (tx, rx) = channel::<()>();
    std::thread::Builder::new().name("cm-features".into()).spawn(move || {
        while let Err(RecvTimeoutError::Timeout) = rx.recv_timeout(interval) {
            let Some(r) = res.upgrade() else { break };
            if let Some(change) = r.recheck_features() {
                changed(&change);
            }
        }
    })?;
    *lock(&r.features_watch) = Some(tx);
    Ok(())
}

/// Payload of [`EVENT_FEATURES`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeaturesEvent {
    pub experimental_features: bool,
    /// The resident utilities were stopped because the switch was turned off.
    pub resident_stopped: bool,
    /// They stopped, but something could not be undone (the startup value or the saved settings); the next start
    /// retries.
    pub error: Option<String>,
}

impl From<&FeaturesChange> for FeaturesEvent {
    fn from(c: &FeaturesChange) -> FeaturesEvent {
        let error = match &c.stopped {
            Some(Ok(report)) => report.startup_error.as_ref().map(|e| format!("the resident utilities stopped, but their startup value could not be removed: {e}")),
            Some(Err(e)) => Some(format!("the resident utilities stopped, but their settings could not be saved: {e}")),
            None => None,
        };
        FeaturesEvent { experimental_features: c.experimental_features, resident_stopped: c.stopped.is_some(), error }
    }
}

/// D15: watch the switch every [`FEATURES_POLL`]. Turned off elsewhere (the CLI), the same stop path as
/// 设置 › 实验功能 runs ([`Resident::stop_all`] and the tray icon), and a window hidden to the tray is shown again
/// (nothing else would lead back to it). The page hears [`EVENT_FEATURES`] and re-reads the capabilities, so gated
/// tools show or leave their "默认关闭" page.
pub fn watch_features(app: &AppHandle, r: &Arc<Resident>) {
    let app = app.clone();
    let _ = start_features_watch(r, FEATURES_POLL, move |change| {
        if change.stopped.is_some() {
            // On the main thread, which created the tray icon; not waited for.
            let a = app.clone();
            let _ = app.run_on_main_thread(move || {
                remove_tray(&a);
                if a.get_webview_window("main").is_some_and(|w| !w.is_visible().unwrap_or(true)) {
                    show_main(&a);
                }
            });
            let _ = app.emit(EVENT_CHANGED, ());
        }
        let _ = app.emit(EVENT_FEATURES, FeaturesEvent::from(change));
    });
}

/// [`Resident::stop_all`] plus the tray icon. Fails when the startup value could not be removed.
pub fn turn_off_all(app: &AppHandle, r: &Resident) -> Result<(), String> {
    let report = r.stop_all()?;
    remove_tray(app);
    match report.startup_error {
        Some(e) => Err(format!("the resident utilities stopped, but their startup value could not be removed: {e}")),
        None => Ok(()),
    }
}

pub(crate) type Rs<'a> = State<'a, Arc<Resident>>;

// ---------------------------------------------------------------------------------------------
// Tray (Windows notification area; macOS menu-bar status item)

/// Create (or re-create) the tray icon and return its sampling loop. Called through [`Resident::apply`], which
/// starts it only when no loop is running.
pub fn start_tray(app: &AppHandle, r: &Arc<Resident>) -> Result<ResidentLoop, String> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};

    remove_tray(app);
    let e = |x: tauri::Error| x.to_string();
    let header = MenuItem::with_id(app, "info:header", "CleanManager 状态", false, None::<&str>).map_err(e)?;
    let lines = [
        MenuItem::with_id(app, "info:cpu", "处理器 正在采样…", false, None::<&str>).map_err(e)?,
        MenuItem::with_id(app, "info:memory", "内存已用 正在采样…", false, None::<&str>).map_err(e)?,
        MenuItem::with_id(app, "info:disk", "系统盘可用 正在采样…", false, None::<&str>).map_err(e)?,
        MenuItem::with_id(app, "info:privacy", "摄像头和麦克风 正在读取…", false, None::<&str>).map_err(e)?,
    ];
    let links: Vec<MenuItem<tauri::Wry>> = cm_core::resident::QuickLink::ALL.iter().map(|q| MenuItem::with_id(app, q.id(), q.label(), true, None::<&str>)).collect::<Result<_, _>>().map_err(e)?;
    let off = MenuItem::with_id(app, "resident:off", "关闭常驻组件", true, None::<&str>).map_err(e)?;
    let quit = MenuItem::with_id(app, "app:quit", "退出 CleanManager", true, None::<&str>).map_err(e)?;
    let sep1 = PredefinedMenuItem::separator(app).map_err(e)?;
    let sep2 = PredefinedMenuItem::separator(app).map_err(e)?;
    let mut items: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = vec![&header];
    items.extend(lines.iter().map(|m| m as &dyn tauri::menu::IsMenuItem<tauri::Wry>));
    items.push(&sep1);
    items.extend(links.iter().map(|m| m as &dyn tauri::menu::IsMenuItem<tauri::Wry>));
    items.push(&sep2);
    items.push(&off);
    items.push(&quit);
    let menu = Menu::with_items(app, &items).map_err(e)?;
    {
        let mut b = TrayIconBuilder::with_id(TRAY_ID).tooltip("CleanManager").menu(&menu).show_menu_on_left_click(true);
        if let Some(icon) = app.default_window_icon() {
            b = b.icon(icon.clone());
        }
        b.on_menu_event(|app, ev| menu_event(app, ev.id().as_ref()))
            .on_tray_icon_event(|tray, ev| {
                if let TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } = ev {
                    show_main(tray.app_handle());
                }
            })
            .build(app)
            .map_err(e)?;
    }
    let app2 = app.clone();
    let seen = r.seen.clone();
    // Weak: the loop is owned by the Resident it reports to.
    let res = Arc::downgrade(r);
    ResidentLoop::start_with("cm-resident", cm_core::resident::TRAY_INTERVAL, plat_status::Sampler::new, move |sampler| {
        let summary = TraySummary::of(&sampler.sample());
        let privacy = plat::privacy_snapshot();
        let [c, m, d] = summary.lines();
        for (item, text) in lines.iter().zip([c, m, d, privacy_line(&privacy)]) {
            let _ = item.set_text(text);
        }
        if let Some(t) = app2.tray_by_id(TRAY_ID) {
            let _ = t.set_tooltip(Some(summary.tooltip()));
        }
        let raised = res.upgrade().map(|r| battery_check(&r)).unwrap_or_default();
        let mut s = lock(&seen);
        s.summary = Some(summary);
        s.privacy = Some(privacy);
        if !raised.is_empty() {
            s.alerts.splice(0..0, raised.iter().cloned());
            s.alerts.truncate(10);
            let _ = app2.emit(EVENT_ALERT, &raised);
        }
    })
    .map_err(|e| e.to_string())
}

fn privacy_line(p: &PrivacySnapshot) -> String {
    if !p.supported {
        return "摄像头和麦克风 不可用".into();
    }
    if p.in_use.is_empty() {
        return "摄像头和麦克风 未在使用".into();
    }
    let names: Vec<&str> = p.in_use.iter().map(|a| a.name.as_str()).collect();
    format!("正在使用摄像头或麦克风：{}", names.join("、")).chars().take(80).collect()
}

/// RES-04: one alert per device below the threshold (only when the person turned alerts on).
fn battery_check(r: &Resident) -> Vec<BatteryAlert> {
    let s = lock(&r.settings).clone();
    if !s.battery_alerts {
        return Vec::new();
    }
    let mut devices: Vec<DeviceBattery> = plat::system_battery().into_iter().collect();
    if let Ok(scan) = plat::device_batteries() {
        devices.extend(scan.devices);
    }
    lock(&r.alerts).observe(s.battery_threshold, &devices)
}

/// Remove the tray icon (its sampling has already been stopped and joined).
pub fn remove_tray(app: &AppHandle) {
    if let Some(t) = app.remove_tray_by_id(TRAY_ID) {
        let _ = t.set_visible(false);
    }
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn menu_event(app: &AppHandle, id: &str) {
    if let Some(q) = cm_core::resident::QuickLink::from_id(id) {
        show_main(app);
        let _ = app.emit(EVENT_NAVIGATE, serde_json::json!({ "page": q.page() }));
        return;
    }
    let r = app.state::<Arc<Resident>>().inner().clone();
    match id {
        "resident:off" => {
            let off = ResidentSettings { tray: false, start_at_login: false, ..lock(&r.settings).clone() };
            let _ = r.apply(off, || Err("not starting".into()));
            remove_tray(app);
            let _ = app.emit(EVENT_CHANGED, ());
            // With the tray gone, a hidden window would leave nothing to return to.
            show_main(app);
        }
        "app:quit" => {
            r.quitting.store(true, Ordering::SeqCst);
            match app.get_webview_window("main") {
                Some(w) => {
                    let _ = w.close();
                }
                None => app.exit(0),
            }
        }
        _ => {}
    }
}

/// Tray on: closing the window hides it (the tray stays); "退出" in the tray really closes it.
pub fn hide_on_close(app: &AppHandle) -> bool {
    let r = app.state::<Arc<Resident>>();
    r.settings.lock().map(|s| s.tray).unwrap_or(false) && !r.quitting.load(Ordering::SeqCst) && app.tray_by_id(TRAY_ID).is_some()
}

/// Setup: reconcile the startup value, start the tray if it is on, and hide the window when started tray-only.
pub fn setup(app: &AppHandle, r: &Arc<Resident>) {
    // D15: with the capability off (e.g. turned off from the CLI), nothing starts and a leftover startup value goes.
    if !r.allowed() {
        let s = lock(&r.settings).clone();
        if s.tray || s.battery_alerts || r.startup_entry().is_some() {
            let _ = r.stop_all();
        }
        show_main(app);
        return;
    }
    let settings = lock(&r.settings).clone();
    // macOS: start at login cannot be on in this build (a hand-edited settings file is corrected and saved by `apply`).
    #[cfg(target_os = "macos")]
    let settings = ResidentSettings { start_at_login: false, ..settings };
    *lock(&r.settings) = settings.clone();
    let _ = r.reconcile_startup();
    let tray_ok = settings.tray && r.apply(settings, || start_tray(app, r)).is_ok() && r.sampling_running();
    let tray_only = std::env::args().any(|a| a == cm_core::resident::RESIDENT_ARG);
    if !(tray_only && tray_ok) {
        show_main(app);
    }
}

// ---------------------------------------------------------------------------------------------
// Commands

#[derive(Debug, Serialize)]
pub struct TrayDto {
    pub enabled: bool,
    pub icon_shown: bool,
    pub sampling: bool,
    pub interval_seconds: u64,
    /// `manual` or `at_login`.
    pub startup: StartupState,
    /// Where the startup value lives (shown whether or not it exists).
    pub startup_location: String,
    /// The startup value's command when it exists.
    pub startup_command: Option<String>,
    pub version: &'static str,
}

#[derive(Debug, Serialize)]
pub struct KeepAwakeDto {
    pub active: bool,
    /// Seconds left; `None` while it lasts until turned off or CleanManager exits.
    pub remaining_seconds: Option<u64>,
    /// The active system request as the system lists it (macOS: the IOPMAssertion type, id and name; `None` on
    /// Windows, whose per-thread request has no id).
    pub assertion: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CleanScreenDto {
    pub running: bool,
    pub min_seconds: u32,
    pub max_seconds: u32,
    pub default_seconds: u32,
    pub esc_hold_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct NotHereDto {
    pub id: &'static str,
    pub reason: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ResidentStatusDto {
    pub supported: bool,
    pub platform: &'static str,
    pub settings: ResidentSettings,
    pub tray: TrayDto,
    pub seen: Seen,
    pub keep_awake: KeepAwakeDto,
    pub clean_screen: CleanScreenDto,
    /// Resident features this platform does not offer, with the reason (RES-05/06 on Windows).
    pub not_here: Vec<NotHereDto>,
}

fn keep_awake_dto(r: &Resident) -> KeepAwakeDto {
    let mut k = lock(&r.keep_awake);
    if k.as_ref().is_some_and(|x| !x.is_active()) {
        *k = None;
    }
    match k.as_ref() {
        Some(x) => KeepAwakeDto {
            active: true,
            remaining_seconds: x.remaining().map(|d| d.as_secs() + u64::from(d.subsec_nanos() > 0)),
            #[cfg(target_os = "macos")]
            assertion: Some(x.describe()),
            #[cfg(windows)]
            assertion: None,
        },
        None => KeepAwakeDto { active: false, remaining_seconds: None, assertion: None },
    }
}

/// Resident features this platform does not offer, with the reason (RES-05/06; on macOS also start at login).
fn not_here() -> Vec<NotHereDto> {
    #[cfg(windows)]
    return vec![
        NotHereDto { id: "fan_control", reason: "Windows has no documented interface for fan control; vendor tools write the embedded controller directly" },
        NotHereDto { id: "charge_limit", reason: "Windows has no common interface for a battery charge limit; it is set in vendor tools or firmware" },
        NotHereDto { id: "low_latency_network", reason: "needs the privileged helper, which this build does not install" },
        NotHereDto { id: "touch_id_sudo", reason: "Touch ID for administrator prompts exists on macOS only" },
    ];
    // Ids differ from the Windows ones because the reasons differ (the page words the Windows ids itself).
    #[cfg(target_os = "macos")]
    return vec![
        NotHereDto { id: "start_at_login_macos", reason: plat::LOGIN_ITEM_REASON },
        NotHereDto { id: "fan_control_macos", reason: "fan control needs the SMC through the signed privileged helper (D12), which this build does not install; fan speeds are not read either" },
        NotHereDto { id: "charge_limit_macos", reason: "a battery charge limit needs the SMC through the signed privileged helper (D12), which this build does not install; macOS's own Optimized Battery Charging is unaffected" },
        NotHereDto { id: "low_latency_network_macos", reason: "needs the signed privileged helper (D12), which this build does not install" },
        NotHereDto { id: "touch_id_sudo_macos", reason: "Touch ID for sudo (RES-06, task 15.4) is not implemented in this build; /etc/pam.d is never changed" },
    ];
}

pub fn status_of(app: Option<&AppHandle>, r: &Resident) -> ResidentStatusDto {
    let settings = lock(&r.settings).clone();
    let command = r.startup_entry();
    #[cfg(windows)]
    let location = plat::RunValue::real().display();
    #[cfg(target_os = "macos")]
    let location = plat::LOGIN_ITEM_LOCATION.to_owned();
    #[cfg(windows)]
    let running = cm_platform::windows::resident_screen::is_running();
    #[cfg(target_os = "macos")]
    let running = mac_screen::is_running();
    ResidentStatusDto {
        supported: true,
        platform: if cfg!(windows) { "windows" } else if cfg!(target_os = "macos") { "macos" } else { "other" },
        tray: TrayDto {
            enabled: settings.tray,
            icon_shown: app.is_some_and(|a| a.tray_by_id(TRAY_ID).is_some()),
            sampling: r.sampling_running(),
            interval_seconds: cm_core::resident::TRAY_INTERVAL.as_secs(),
            startup: if command.is_some() { StartupState::AtLogin } else { StartupState::Manual },
            startup_location: location,
            startup_command: command,
            version: env!("CARGO_PKG_VERSION"),
        },
        seen: lock(&r.seen).clone(),
        keep_awake: keep_awake_dto(r),
        clean_screen: CleanScreenDto {
            running,
            min_seconds: cm_core::resident::CLEAN_SCREEN_MIN_SECS,
            max_seconds: cm_core::resident::CLEAN_SCREEN_MAX_SECS,
            default_seconds: cm_core::resident::CLEAN_SCREEN_DEFAULT_SECS,
            esc_hold_ms: cm_core::resident::ESC_HOLD.as_millis() as u64,
        },
        not_here: not_here(),
        settings,
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn resident_status(app: AppHandle, state: Rs<'_>) -> IpcResult<ResidentStatusDto> {
    let r = state.inner().clone();
    blocking(move || Ok(status_of(Some(&app), &r))).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn resident_save(app: AppHandle, state: Rs<'_>, settings: ResidentSettings) -> IpcResult<ResidentStatusDto> {
    let r = state.inner().clone();
    blocking(move || {
        settings.validate().map_err(bad)?;
        // Refused before anything is saved, so the settings never claim a login item that cannot exist.
        #[cfg(target_os = "macos")]
        if settings.start_at_login {
            return Err(IpcError::new(ErrorCode::Unsupported, "unsupported", plat::LOGIN_ITEM_REASON));
        }
        let off = r.apply(settings, || start_tray(&app, &r)).map_err(internal)?;
        if let Some(report) = off {
            remove_tray(&app);
            if let Some(e) = report.startup_error {
                return Err(internal(format!("the tray is off, but its startup value could not be removed: {e}")));
            }
        }
        let _ = app.emit(EVENT_CHANGED, ());
        Ok(status_of(Some(&app), &r))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn resident_keep_awake(state: Rs<'_>, on: bool, minutes: Option<u32>) -> IpcResult<KeepAwakeDto> {
    let r = state.inner().clone();
    blocking(move || {
        let duration = cm_core::resident::KeepAwakeRequest { minutes }.validate().map_err(bad)?;
        {
            let mut k = lock(&r.keep_awake);
            if let Some(mut old) = k.take() {
                old.release();
            }
            if on {
                *k = Some(plat::KeepAwake::start(duration).map_err(internal)?);
            }
        }
        Ok(keep_awake_dto(&r))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn resident_clean_screen(app: AppHandle, timeout_secs: u32) -> IpcResult<cm_core::resident::CleanScreenExit> {
    blocking(move || {
        let timeout = cm_core::resident::CleanScreenOptions { timeout_secs }.validate().map_err(bad)?;
        #[cfg(windows)]
        {
            let _ = app;
            cm_platform::windows::resident_screen::run(timeout).map_err(|e| IpcError::new(ErrorCode::Conflict, "conflict", e))
        }
        #[cfg(target_os = "macos")]
        mac_screen::run(&app, timeout).map_err(|e| IpcError::new(ErrorCode::Conflict, "conflict", e))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn resident_privacy() -> IpcResult<PrivacySnapshot> {
    blocking(|| Ok(plat::privacy_snapshot())).await
}

/// A device the system lists without a usable battery level (shown as unavailable, never as 0%).
#[derive(Debug, Serialize)]
pub struct UnavailableDeviceDto {
    pub device_id: String,
    pub name: String,
    pub kind: &'static str,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct BatteriesDto {
    pub supported: bool,
    pub system: Option<DeviceBattery>,
    pub devices: Vec<DeviceBattery>,
    pub bluetooth_devices: usize,
    /// Why device levels cannot be read (the list could not be read at all).
    pub devices_unavailable: Option<String>,
    /// Devices listed without a level (macOS: Bluetooth HID devices without `BatteryPercent`; empty on Windows, which
    /// only counts them in `bluetooth_devices`).
    pub unavailable_devices: Vec<UnavailableDeviceDto>,
    pub source: &'static str,
}

#[cfg(windows)]
const BATTERY_SOURCE: &str = "GetSystemPowerStatus; device property {104EA319-6EE2-4701-BD47-8DDBF425BBE5},2 set by the Windows Bluetooth stack";
#[cfg(target_os = "macos")]
const BATTERY_SOURCE: &str = "IOPSCopyPowerSourcesInfo (internal battery); I/O Registry BatteryPercent of Apple HID peripheral drivers \
     (AppleDeviceManagementHIDEventService and the older Bluetooth HID classes)";

fn batteries_now() -> BatteriesDto {
    let system = plat::system_battery();
    match plat::device_batteries() {
        Ok(scan) => {
            #[cfg(target_os = "macos")]
            let unavailable_devices = scan.unavailable.into_iter().map(|u| UnavailableDeviceDto { device_id: u.device_id, name: u.name, kind: u.kind, reason: u.reason }).collect();
            #[cfg(windows)]
            let unavailable_devices = Vec::new();
            BatteriesDto { supported: true, system, devices: scan.devices, bluetooth_devices: scan.bluetooth_devices, devices_unavailable: None, unavailable_devices, source: BATTERY_SOURCE }
        }
        Err(e) => BatteriesDto { supported: true, system, devices: Vec::new(), bluetooth_devices: 0, devices_unavailable: Some(e), unavailable_devices: Vec::new(), source: BATTERY_SOURCE },
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn resident_batteries() -> IpcResult<BatteriesDto> {
    blocking(|| Ok(batteries_now())).await
}

/// Label prefix of the macOS clean-screen windows (one per display). Windows draws its own native window instead.
pub const CLEAN_SCREEN_LABEL_PREFIX: &str = "cm-clean-screen-";

/// A clean-screen window: the main window's close handling (hide to the tray, safe exit) does not apply to it.
pub fn is_clean_screen_window(label: &str) -> bool {
    label.starts_with(CLEAN_SCREEN_LABEL_PREFIX)
}

/// Clean-screen mode on macOS (RES-02): one borderless, always-on-top, black webview window per display, showing
/// a blank page (`about:blank#cm-clean-screen`) with a small script that swallows keys, clicks and scrolling, hides the pointer and reports a 2-second
/// Esc hold by navigating to the `cm-clean-screen:` scheme, which the window's navigation handler
/// intercepts (the windows get no IPC permission). While it runs this app hides the Dock and menu bar and disables
/// Command-Tab and Command-H (`NSApplicationPresentationOptions`); Force Quit (Option-Command-Esc), the power button and
/// screen lock stay available. It ends when Esc is held, at the timeout, or when a window is closed (Command-W).
///
/// Limits: input is captured only while CleanManager is the active app (macOS gives an app no way to block other apps'
/// input without Accessibility permission); system keys handled before any app (brightness, volume, Mission Control,
/// Spotlight, screenshots) still work; a key already held when the mode starts is ignored until it is released.
#[cfg(target_os = "macos")]
pub mod mac_screen {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc::{RecvTimeoutError, channel};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use cm_core::resident::{CleanScreenExit, ESC_HOLD};
    use cm_platform::macos::resident::{begin_clean_presentation, end_clean_presentation};
    use tauri::webview::PageLoadEvent;
    use tauri::{AppHandle, LogicalPosition, LogicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

    use super::CLEAN_SCREEN_LABEL_PREFIX;

    /// The page's only way to report: a navigation to this scheme, cancelled by the navigation handler.
    pub const SIGNAL_SCHEME: &str = "cm-clean-screen";
    static RUNNING: AtomicBool = AtomicBool::new(false);
    /// The page each window shows. Not plain `about:blank`: Tauri leaves a webview with exactly that URL unloaded, and
    /// WebKit runs evaluated scripts only after a navigation commits.
    const BLANK_PAGE: &str = "about:blank#cm-clean-screen";
    /// Session number: each session's windows get fresh labels (a destroyed window may linger briefly under its label).
    static SESSION: AtomicU64 = AtomicU64::new(0);
    /// How often the page script is re-applied while the mode runs (it is idempotent; this covers a document that was
    /// replaced after the first evaluation, for example when `about:blank` finished loading).
    const REAPPLY: Duration = Duration::from_millis(250);

    pub fn is_running() -> bool {
        RUNNING.load(Ordering::SeqCst)
    }

    struct Running;
    impl Drop for Running {
        fn drop(&mut self) {
            RUNNING.store(false, Ordering::SeqCst);
        }
    }

    /// The page script: idempotent per document (it may run before and after `about:blank` finishes loading).
    pub fn script(hold_ms: u64, deadline_epoch_ms: u64, hint: bool) -> String {
        format!(
            r#"(function () {{
  var root = document.documentElement;
  if (!root || root.getAttribute('data-cm-clean')) return;
  root.setAttribute('data-cm-clean', '1');
  root.style.cssText = 'background:#000;cursor:none;height:100%;margin:0;overflow:hidden';
  if (document.body) document.body.style.cssText = 'background:#000;cursor:none;margin:0;height:100%';
  var HOLD = {hold_ms}, END = {deadline_epoch_ms}, timer = null, done = false;
  function eat(e) {{ e.preventDefault(); e.stopImmediatePropagation(); }}
  function isEsc(e) {{ return e.key === 'Escape' || e.keyCode === 27; }}
  document.addEventListener('keydown', function (e) {{
    eat(e);
    if (isEsc(e) && !e.repeat && timer === null) {{
      timer = setTimeout(function () {{ if (!done) {{ done = true; location.href = '{scheme}://esc-held'; }} }}, HOLD);
    }}
  }}, true);
  document.addEventListener('keyup', function (e) {{ eat(e); if (isEsc(e) && timer !== null) {{ clearTimeout(timer); timer = null; }} }}, true);
  ['keypress', 'mousedown', 'mouseup', 'click', 'dblclick', 'auxclick', 'contextmenu', 'wheel', 'dragstart', 'selectstart'].forEach(function (t) {{
    document.addEventListener(t, eat, {{ capture: true, passive: false }});
  }});
  if ({hint}) {{
    var p = document.createElement('div');
    p.style.cssText = 'position:fixed;left:0;right:0;bottom:24px;text-align:center;color:#4a4a4a;font:13px -apple-system,sans-serif;user-select:none';
    (document.body || root).appendChild(p);
    var tick = function () {{
      var left = Math.max(0, Math.ceil((END - Date.now()) / 1000));
      p.textContent = '清洁屏幕：按住 Esc ' + Math.round(HOLD / 1000) + ' 秒退出 · ' + left + ' 秒后自动退出 · 强制退出（Option-Command-Esc）始终可用';
    }};
    tick();
    setInterval(tick, 1000);
  }}
}})();"#,
            scheme = SIGNAL_SCHEME,
        )
    }

    /// Run `f` on the main thread and wait for its result.
    fn on_main<T: Send + 'static>(app: &AppHandle, f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
        let (tx, rx) = channel();
        app.run_on_main_thread(move || {
            let _ = tx.send(f());
        })
        .map_err(|e| e.to_string())?;
        rx.recv_timeout(Duration::from_secs(5)).map_err(|_| "the main thread did not answer".to_string())
    }

    fn close_all(windows: &mut Vec<(WebviewWindow, String)>) {
        for (w, _) in windows.drain(..) {
            let _ = w.destroy();
        }
    }

    /// Show the clean-screen windows until Esc is held, the timeout passes or a window is closed. Blocks the caller
    /// (a worker thread; window creation is dispatched to the main thread).
    pub fn run(app: &AppHandle, timeout: Duration) -> Result<CleanScreenExit, String> {
        if RUNNING.swap(true, Ordering::SeqCst) {
            return Err("clean-screen mode is already running".into());
        }
        let _running = Running;
        let monitors = app.available_monitors().map_err(|e| e.to_string())?;
        if monitors.is_empty() {
            return Err("no display was found".into());
        }
        let primary = app.primary_monitor().ok().flatten();
        let deadline = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| (d + timeout).as_millis() as u64).unwrap_or(0);
        let hold_ms = ESC_HOLD.as_millis() as u64;
        // Hide the Dock and menu bar first, so the windows can cover each whole display.
        let presented = on_main(app, begin_clean_presentation).and_then(|r| r);
        let session = SESSION.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = channel::<CleanScreenExit>();
        let mut windows: Vec<(WebviewWindow, String)> = Vec::new();
        let mut focus: Option<usize> = None;
        for (i, m) in monitors.iter().enumerate() {
            let scale = m.scale_factor();
            let pos: LogicalPosition<f64> = m.position().to_logical(scale);
            let size: LogicalSize<f64> = m.size().to_logical(scale);
            let is_primary = primary.as_ref().map_or(i == 0, |p| p.position() == m.position() && p.size() == m.size());
            let js = script(hold_ms, deadline, is_primary);
            let on_load = js.clone();
            let nav_tx = tx.clone();
            let built = WebviewWindowBuilder::new(app, format!("{CLEAN_SCREEN_LABEL_PREFIX}{session}-{i}"), WebviewUrl::External(BLANK_PAGE.parse().map_err(|e| format!("{e}"))?))
                .title("CleanManager 清洁屏幕")
                .decorations(false)
                .resizable(false)
                .shadow(false)
                .always_on_top(true)
                .visible_on_all_workspaces(true)
                .skip_taskbar(true)
                .position(pos.x, pos.y)
                .inner_size(size.width, size.height)
                .background_color(tauri::window::Color(0, 0, 0, 255))
                .focused(is_primary)
                .on_navigation(move |url| {
                    if url.scheme() == SIGNAL_SCHEME {
                        let _ = nav_tx.send(CleanScreenExit::EscHeld);
                        return false;
                    }
                    true
                })
                .on_page_load(move |w, payload| {
                    if payload.event() == PageLoadEvent::Finished {
                        let _ = w.eval(on_load.as_str());
                    }
                })
                .build();
            match built {
                Ok(w) => {
                    let closed = tx.clone();
                    w.on_window_event(move |e| {
                        if let WindowEvent::Destroyed = e {
                            let _ = closed.send(CleanScreenExit::Closed);
                        }
                    });
                    // Re-apply the frame in case AppKit constrained it while the window was created.
                    let _ = w.set_position(pos);
                    let _ = w.set_size(size);
                    let _ = w.eval(js.as_str());
                    if is_primary {
                        focus = Some(windows.len());
                    }
                    windows.push((w, js));
                }
                Err(e) => {
                    close_all(&mut windows);
                    if matches!(presented, Ok(true)) {
                        let _ = on_main(app, end_clean_presentation);
                    }
                    return Err(format!("cannot open the clean-screen window: {e}"));
                }
            }
        }
        drop(tx);
        if let Some((w, _)) = windows.get(focus.unwrap_or(0)) {
            let _ = w.set_focus();
        }
        let end = Instant::now() + timeout;
        let exit = loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break CleanScreenExit::Timeout;
            }
            match rx.recv_timeout(left.min(REAPPLY)) {
                Ok(e) => break e,
                Err(RecvTimeoutError::Disconnected) => break CleanScreenExit::Closed,
                Err(RecvTimeoutError::Timeout) => {
                    for (w, js) in &windows {
                        let _ = w.eval(js.as_str());
                    }
                }
            }
        };
        close_all(&mut windows);
        if matches!(presented, Ok(true)) {
            let _ = on_main(app, end_clean_presentation);
        }
        Ok(exit)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cm-resident-{tag}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    struct Mem(Mutex<Option<String>>);
    impl StartupStore for Mem {
        fn read(&self) -> Result<Option<String>, String> {
            Ok(lock(&self.0).clone())
        }
        fn write(&self, c: &str) -> Result<(), String> {
            *lock(&self.0) = Some(c.into());
            Ok(())
        }
        fn remove(&self) -> Result<bool, String> {
            Ok(lock(&self.0).take().is_some())
        }
    }

    fn counting(calls: &Arc<AtomicUsize>) -> impl FnOnce() -> Result<ResidentLoop, String> + use<> {
        let c = calls.clone();
        move || {
            ResidentLoop::start("test-tray", Duration::from_millis(10), move || {
                c.fetch_add(1, Ordering::SeqCst);
            })
            .map_err(|e| e.to_string())
        }
    }

    fn turn_on_then_off(store: Box<dyn StartupStore>, read: impl Fn() -> Option<String>) {
        let dir = temp("ctl");
        let r = Resident::new(Some(&dir), store, r"C:\Apps\cleanmanager-desktop.exe".into());
        assert_eq!(*lock(&r.settings), ResidentSettings::default(), "off by default");
        assert!(!r.sampling_running());
        let calls = Arc::new(AtomicUsize::new(0));
        let on = ResidentSettings { tray: true, start_at_login: true, ..Default::default() };
        assert_eq!(r.apply(on.clone(), counting(&calls)).unwrap(), None);
        assert!(r.sampling_running());
        assert_eq!(read().as_deref(), Some(r#""C:\Apps\cleanmanager-desktop.exe" --resident"#));
        assert_eq!(ResidentSettings::parse(&std::fs::read_to_string(dir.join("resident.json")).unwrap()), on);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while calls.load(Ordering::SeqCst) < 2 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let off = r.apply(ResidentSettings::default(), counting(&calls)).unwrap().unwrap();
        assert!(off.sampling_stopped && off.startup_value_removed && off.startup_error.is_none());
        assert!(!r.sampling_running());
        assert_eq!(read(), None, "no startup value remains");
        let after = calls.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(calls.load(Ordering::SeqCst), after, "no sampling after turning off");
        // Restarting with saved "off" settings removes a stale value left by someone else's earlier run.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn turning_the_tray_off_stops_sampling_and_removes_the_startup_value() {
        let mem = Arc::new(Mem(Mutex::new(None)));
        struct Shared(Arc<Mem>);
        impl StartupStore for Shared {
            fn read(&self) -> Result<Option<String>, String> {
                self.0.read()
            }
            fn write(&self, c: &str) -> Result<(), String> {
                self.0.write(c)
            }
            fn remove(&self) -> Result<bool, String> {
                self.0.remove()
            }
        }
        let m = mem.clone();
        turn_on_then_off(Box::new(Shared(mem)), move || m.read().unwrap());
    }

    /// The same through the real registry code, against a throwaway `HKCU\Software\CleanManagerTest\…` key.
    #[cfg(windows)]
    #[test]
    fn turning_off_removes_the_value_from_a_throwaway_registry_key() {
        use cm_platform::windows::resident::{RunValue, delete_test_key};
        let key = format!(r"Software\CleanManagerTest\desktop-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let probe = RunValue { subkey: key.clone() };
        turn_on_then_off(Box::new(RunValue { subkey: key.clone() }), move || probe.read().unwrap());
        delete_test_key(&key);
    }

    #[test]
    fn a_stale_startup_value_is_removed_at_startup_when_the_tray_is_off() {
        let dir = temp("stale");
        let r = Resident::new(Some(&dir), Box::new(Mem(Mutex::new(Some("\"old.exe\" --resident".into())))), "new.exe".into());
        assert_eq!(r.reconcile_startup().unwrap(), StartupState::Manual);
        assert_eq!(r.startup_entry(), None);
        let s = status_of(None, &r);
        assert!(!s.tray.enabled && !s.tray.sampling && !s.tray.icon_shown && s.tray.startup_command.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// D15: the resident utilities are off until experimental features are on; turning them off stops everything,
    /// removes the startup value and saves the settings off, as turning the tray off does.
    #[test]
    fn turning_experimental_features_off_stops_every_resident_utility() {
        let dir = temp("experimental");
        let r = Resident::new(Some(&dir), Box::new(Mem(Mutex::new(None))), "cm.exe".into());
        assert!(!r.allowed(), "off by default");
        cm_app::features::set_experimental(&dir, true).unwrap();
        assert!(r.allowed());
        let calls = Arc::new(AtomicUsize::new(0));
        let on = ResidentSettings { tray: true, start_at_login: true, battery_alerts: true, battery_threshold: 30 };
        r.apply(on, counting(&calls)).unwrap();
        assert!(r.sampling_running() && r.startup_entry().is_some());

        cm_app::features::set_experimental(&dir, false).unwrap();
        assert!(!r.allowed());
        let report = r.stop_all().unwrap();
        assert!(report.sampling_stopped && report.startup_value_removed && report.startup_error.is_none());
        assert!(!r.sampling_running() && r.startup_entry().is_none());
        let saved = ResidentSettings::parse(&std::fs::read_to_string(dir.join("resident.json")).unwrap());
        assert_eq!(saved, ResidentSettings { tray: false, start_at_login: false, battery_alerts: false, battery_threshold: 30 });
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// D15 from the CLI: `cleanmanager experimental --state off` rewrites `<store>/features.json` while the app runs. The
    /// next check notices it once and runs the stop path; turning it back on is noticed and starts nothing.
    fn cli_flips_the_switch(store: Box<dyn StartupStore>, read: impl Fn() -> Option<String>) {
        let dir = temp("cli-flip");
        cm_app::features::set_experimental(&dir, true).unwrap();
        let r = Resident::new(Some(&dir), store, r"C:\Apps\cleanmanager-desktop.exe".into());
        assert_eq!(r.recheck_features(), None, "unchanged since start");
        let calls = Arc::new(AtomicUsize::new(0));
        let on = ResidentSettings { tray: true, start_at_login: true, battery_alerts: true, battery_threshold: 25 };
        r.apply(on, counting(&calls)).unwrap();
        assert!(r.sampling_running() && read().is_some());

        cm_app::features::set_experimental(&dir, false).unwrap();
        let change = r.recheck_features().expect("the change is noticed");
        assert!(!change.experimental_features);
        let report = change.stopped.clone().expect("stop path ran").unwrap();
        assert!(report.sampling_stopped && report.startup_value_removed && report.startup_error.is_none(), "{report:?}");
        assert_eq!(FeaturesEvent::from(&change), FeaturesEvent { experimental_features: false, resident_stopped: true, error: None });
        assert!(!r.sampling_running());
        assert_eq!(read(), None, "no startup value remains");
        assert_eq!(r.startup_entry(), None);
        let after = calls.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(calls.load(Ordering::SeqCst), after, "no sampling after the switch went off");
        let saved = ResidentSettings::parse(&std::fs::read_to_string(dir.join("resident.json")).unwrap());
        assert_eq!(saved, ResidentSettings { tray: false, start_at_login: false, battery_alerts: false, battery_threshold: 25 });
        assert_eq!(r.recheck_features(), None, "handled once");

        cm_app::features::set_experimental(&dir, true).unwrap();
        let back = r.recheck_features().expect("turned on again is noticed");
        assert_eq!(back, FeaturesChange { experimental_features: true, stopped: None });
        assert!(!r.sampling_running() && read().is_none(), "turning it on starts nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn experimental_features_turned_off_from_the_cli_stop_the_resident_utilities() {
        let mem = Arc::new(Mem(Mutex::new(None)));
        struct Shared(Arc<Mem>);
        impl StartupStore for Shared {
            fn read(&self) -> Result<Option<String>, String> {
                self.0.read()
            }
            fn write(&self, c: &str) -> Result<(), String> {
                self.0.write(c)
            }
            fn remove(&self) -> Result<bool, String> {
                self.0.remove()
            }
        }
        let m = mem.clone();
        cli_flips_the_switch(Box::new(Shared(mem)), move || m.read().unwrap());
    }

    /// The same through the real `Run` value code, against a throwaway `HKCU\Software\CleanManagerTest\…` key.
    #[cfg(windows)]
    #[test]
    fn experimental_features_turned_off_from_the_cli_leave_no_run_value() {
        use cm_platform::windows::resident::{RunValue, delete_test_key};
        let key = format!(r"Software\CleanManagerTest\desktop-cli-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let probe = RunValue { subkey: key.clone() };
        cli_flips_the_switch(Box::new(RunValue { subkey: key.clone() }), move || probe.read().unwrap());
        delete_test_key(&key);
    }

    /// The app's own change (设置 › 实验功能) is noted, so the watcher does not handle it again.
    #[test]
    fn the_apps_own_switch_change_is_not_handled_twice() {
        let dir = temp("own");
        let r = Resident::new(Some(&dir), Box::new(Mem(Mutex::new(None))), "cm.exe".into());
        r.note_features(cm_app::features::set_experimental(&dir, true).unwrap());
        assert_eq!(r.recheck_features(), None);
        r.note_features(cm_app::features::set_experimental(&dir, false).unwrap());
        assert_eq!(r.recheck_features(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The watcher thread notices a change made elsewhere within a bounded time and ends at shutdown.
    #[test]
    fn the_switch_watcher_reports_changes_and_ends_at_shutdown() {
        let dir = temp("watch");
        cm_app::features::set_experimental(&dir, true).unwrap();
        let r = Arc::new(Resident::new(Some(&dir), Box::new(Mem(Mutex::new(None))), "cm.exe".into()));
        let (tx, rx) = std::sync::mpsc::channel::<FeaturesChange>();
        let tx = Mutex::new(tx);
        start_features_watch(&r, Duration::from_millis(20), move |c| {
            let _ = lock(&tx).send(c.clone());
        })
        .unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err(), "nothing changed yet");
        cm_app::features::set_experimental(&dir, false).unwrap();
        let c = rx.recv_timeout(Duration::from_secs(5)).expect("noticed within the bound");
        assert!(!c.experimental_features && c.stopped.as_ref().is_some_and(Result::is_ok), "{c:?}");
        r.shutdown();
        std::thread::sleep(Duration::from_millis(100));
        cm_app::features::set_experimental(&dir, true).unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(), "nothing is checked after shutdown");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_are_validated_before_anything_changes() {
        let dir = temp("invalid");
        let r = Resident::new(Some(&dir), Box::new(Mem(Mutex::new(None))), "x".into());
        let calls = Arc::new(AtomicUsize::new(0));
        assert!(r.apply(ResidentSettings { start_at_login: true, ..Default::default() }, counting(&calls)).is_err());
        assert!(r.apply(ResidentSettings { battery_threshold: 80, ..Default::default() }, counting(&calls)).is_err());
        assert!(!dir.join("resident.json").exists());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
