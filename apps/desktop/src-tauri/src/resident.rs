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

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cm_core::error::ErrorCode;
use cm_core::resident::{BatteryAlert, DeviceBattery, PrivacySnapshot, ResidentLoop, ResidentSettings, StartupState, StartupStore, TraySummary, TurnOffReport};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::blocking;
use crate::dto::{IpcError, IpcResult};
use crate::host::lock;

pub const TRAY_ID: &str = "cm-resident";
pub const EVENT_NAVIGATE: &str = "resident-navigate";
pub const EVENT_CHANGED: &str = "resident-changed";
pub const EVENT_ALERT: &str = "resident-alert";

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
    settings_path: Option<PathBuf>,
    pub settings: Mutex<ResidentSettings>,
    sampling: Mutex<Option<ResidentLoop>>,
    store: Box<dyn StartupStore>,
    /// The command written to the startup value.
    exe: String,
    pub seen: Arc<Mutex<Seen>>,
    /// "Quit" was chosen: the next window close really exits instead of hiding to the tray.
    pub quitting: AtomicBool,
    #[cfg(windows)]
    keep_awake: Mutex<Option<cm_platform::windows::resident::KeepAwake>>,
    #[cfg(windows)]
    alerts: Mutex<cm_core::resident::BatteryAlerts>,
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
            settings_path,
            settings: Mutex::new(settings),
            sampling: Mutex::new(None),
            store,
            exe,
            seen: Arc::new(Mutex::new(Seen::default())),
            quitting: AtomicBool::new(false),
            #[cfg(windows)]
            keep_awake: Mutex::new(None),
            #[cfg(windows)]
            alerts: Mutex::new(Default::default()),
        }
    }

    /// The real per-user store, or none off Windows.
    pub fn for_this_platform(store_dir: Option<&Path>) -> Resident {
        let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
        #[cfg(windows)]
        let store: Box<dyn StartupStore> = Box::new(cm_platform::windows::resident::RunValue::real());
        #[cfg(not(windows))]
        let store: Box<dyn StartupStore> = Box::new(NoStartup);
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
    /// and settings stay as the person chose.
    pub fn shutdown(&self) {
        if let Some(mut l) = lock(&self.sampling).take() {
            l.stop();
        }
        #[cfg(windows)]
        if let Some(mut k) = lock(&self.keep_awake).take() {
            k.release();
        }
    }

    pub fn startup_entry(&self) -> Option<String> {
        self.store.read().ok().flatten()
    }
}

#[cfg(not(windows))]
struct NoStartup;

#[cfg(not(windows))]
impl StartupStore for NoStartup {
    fn read(&self) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn write(&self, _: &str) -> Result<(), String> {
        Err("start at login is implemented on Windows only in this build".into())
    }
    fn remove(&self) -> Result<bool, String> {
        Ok(false)
    }
}

pub(crate) type Rs<'a> = State<'a, Arc<Resident>>;

// ---------------------------------------------------------------------------------------------
// Tray (Windows notification area; the same code serves the macOS menu bar once verified there)

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
    ResidentLoop::start_with(
            "cm-resident",
            cm_core::resident::TRAY_INTERVAL,
            || {
                #[cfg(windows)]
                return cm_platform::windows::status::Sampler::new();
            },
            move |_sampler| {
                #[cfg(windows)]
                {
                    let summary = TraySummary::of(&_sampler.sample());
                    let privacy = cm_platform::windows::resident::privacy_snapshot();
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
                }
            },
        )
        .map_err(|e| e.to_string())
}

#[cfg(windows)]
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
#[cfg(windows)]
fn battery_check(r: &Resident) -> Vec<BatteryAlert> {
    let s = lock(&r.settings).clone();
    if !s.battery_alerts {
        return Vec::new();
    }
    let mut devices: Vec<DeviceBattery> = cm_platform::windows::resident::system_battery().into_iter().collect();
    if let Ok(scan) = cm_platform::windows::resident::device_batteries() {
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
    let _ = r.reconcile_startup();
    let settings = lock(&r.settings).clone();
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

fn keep_awake_dto(_r: &Resident) -> KeepAwakeDto {
    #[cfg(windows)]
    {
        let mut k = lock(&_r.keep_awake);
        if k.as_ref().is_some_and(|x| !x.is_active()) {
            *k = None;
        }
        if let Some(x) = k.as_ref() {
            return KeepAwakeDto { active: true, remaining_seconds: x.remaining().map(|d| d.as_secs() + u64::from(d.subsec_nanos() > 0)) };
        }
    }
    KeepAwakeDto { active: false, remaining_seconds: None }
}

pub fn status_of(app: Option<&AppHandle>, r: &Resident) -> ResidentStatusDto {
    let settings = lock(&r.settings).clone();
    let command = r.startup_entry();
    #[cfg(windows)]
    let location = cm_platform::windows::resident::RunValue::real().display();
    #[cfg(not(windows))]
    let location = String::new();
    #[cfg(windows)]
    let running = cm_platform::windows::resident_screen::is_running();
    #[cfg(not(windows))]
    let running = false;
    ResidentStatusDto {
        supported: cfg!(windows),
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
        not_here: if cfg!(windows) {
            vec![
                NotHereDto { id: "fan_control", reason: "Windows has no documented interface for fan control; vendor tools write the embedded controller directly" },
                NotHereDto { id: "charge_limit", reason: "Windows has no common interface for a battery charge limit; it is set in vendor tools or firmware" },
                NotHereDto { id: "low_latency_network", reason: "needs the privileged helper, which this build does not install" },
                NotHereDto { id: "touch_id_sudo", reason: "Touch ID for administrator prompts exists on macOS only" },
            ]
        } else {
            Vec::new()
        },
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
        if !cfg!(windows) && settings.tray {
            return Err(IpcError::new(ErrorCode::Unsupported, "unsupported", "the tray component is implemented on Windows only in this build"));
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
        #[cfg(windows)]
        {
            let mut k = lock(&r.keep_awake);
            if let Some(mut old) = k.take() {
                old.release();
            }
            if on {
                *k = Some(cm_platform::windows::resident::KeepAwake::start(duration).map_err(internal)?);
            }
        }
        #[cfg(not(windows))]
        if on {
            let _ = duration;
            return Err(IpcError::new(ErrorCode::Unsupported, "unsupported", "keeping the screen on is implemented on Windows only in this build"));
        }
        Ok(keep_awake_dto(&r))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn resident_clean_screen(timeout_secs: u32) -> IpcResult<cm_core::resident::CleanScreenExit> {
    blocking(move || {
        let timeout = cm_core::resident::CleanScreenOptions { timeout_secs }.validate().map_err(bad)?;
        #[cfg(windows)]
        return cm_platform::windows::resident_screen::run(timeout).map_err(|e| IpcError::new(ErrorCode::Conflict, "conflict", e));
        #[cfg(not(windows))]
        {
            let _ = timeout;
            Err(IpcError::new(ErrorCode::Unsupported, "unsupported", "clean-screen mode is implemented on Windows only in this build"))
        }
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn resident_privacy() -> IpcResult<PrivacySnapshot> {
    blocking(|| {
        #[cfg(windows)]
        return Ok(cm_platform::windows::resident::privacy_snapshot());
        #[cfg(not(windows))]
        Ok(PrivacySnapshot { supported: false, in_use: Vec::new(), unavailable_reason: Some("implemented on Windows only in this build".into()), source: "" })
    })
    .await
}

#[derive(Debug, Serialize)]
pub struct BatteriesDto {
    pub supported: bool,
    pub system: Option<DeviceBattery>,
    pub devices: Vec<DeviceBattery>,
    pub bluetooth_devices: usize,
    /// Why device levels cannot be read (the list could not be read at all).
    pub devices_unavailable: Option<String>,
    pub source: &'static str,
}

#[cfg(windows)]
fn batteries_now() -> BatteriesDto {
    let source = "GetSystemPowerStatus; device property {104EA319-6EE2-4701-BD47-8DDBF425BBE5},2 set by the Windows Bluetooth stack";
    let system = cm_platform::windows::resident::system_battery();
    match cm_platform::windows::resident::device_batteries() {
        Ok(scan) => BatteriesDto { supported: true, system, devices: scan.devices, bluetooth_devices: scan.bluetooth_devices, devices_unavailable: None, source },
        Err(e) => BatteriesDto { supported: true, system, devices: Vec::new(), bluetooth_devices: 0, devices_unavailable: Some(e), source },
    }
}

#[cfg(not(windows))]
fn batteries_now() -> BatteriesDto {
    BatteriesDto { supported: false, system: None, devices: Vec::new(), bluetooth_devices: 0, devices_unavailable: Some("implemented on Windows only in this build".into()), source: "" }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn resident_batteries() -> IpcResult<BatteriesDto> {
    blocking(|| Ok(batteries_now())).await
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
