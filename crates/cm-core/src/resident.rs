//! Resident utilities (add-mole-parity-suite group 15, RES-01..04 and RES-07): the platform-neutral part.
//!
//! - RES-01/07: [`ResidentSettings`] (everything off by default), the CleanManager-owned startup value
//!   ([`RUN_VALUE_NAME`] under the per-user `Run` key, written only when the person turns on "start with Windows"),
//!   [`StartupStore`] so the value can be tested against a throwaway key, [`TraySummary`] (the compact status shown
//!   in the tray: CPU, memory, free space on the system disk), the fixed set of [`QuickLink`]s the tray may open,
//!   and [`ResidentLoop`], the only background timer the component runs. [`turn_off`] stops the loop (joining its
//!   thread) and removes the startup value; nothing samples after it returns.
//! - RES-02: [`KeepAwakeRequest`] (optional duration) and [`CleanScreenOptions`] with the long-press-Esc exit
//!   ([`EscHold`]) and the timeout.
//! - RES-03: [`ConsentRecord`] / [`PrivacySnapshot`]: which apps are using the camera or microphone right now
//!   (the system's capability-access records with a start time and no stop time), names only, nothing stored.
//! - RES-04: [`DeviceBattery`] and [`BatteryAlerts`]: one alert when a device falls below the threshold, re-armed
//!   once it recovers.
//!
//! The resident component never performs cleanup, uninstall or maintenance writes; the only system state it
//! changes is its own startup value (opt-in, removed when turned off) and temporary power requests that end with
//! the request or the process.

use std::collections::BTreeMap;
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::status::{Availability, StatusSnapshot, names};

/// Settings file in the store directory (configuration; kept by `self uninstall` unless user data is removed).
pub const SETTINGS_FILE: &str = "resident.json";
/// Per-user startup key (`HKCU\…\Run`). The resident component writes only [`RUN_VALUE_NAME`] there.
pub const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// The value name CleanManager owns under [`RUN_SUBKEY`]; nothing else is ever written or removed there.
pub const RUN_VALUE_NAME: &str = "CleanManager.Resident";
/// Command-line switch: start with the main window hidden and only the tray icon shown.
pub const RESIDENT_ARG: &str = "--resident";
/// How often the tray refreshes its compact status while it is on.
pub const TRAY_INTERVAL: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------------------------
// Settings

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ResidentSettings {
    /// Show the tray component (RES-01). Off by default.
    pub tray: bool,
    /// Start the tray when the person signs in (only meaningful with `tray`). Off by default.
    pub start_at_login: bool,
    /// Low-battery alerts for this computer and devices that report a level (RES-04). Off by default.
    pub battery_alerts: bool,
    /// Alert threshold in percent (5–50).
    pub battery_threshold: u8,
}

impl Default for ResidentSettings {
    fn default() -> Self {
        ResidentSettings { tray: false, start_at_login: false, battery_alerts: false, battery_threshold: 20 }
    }
}

impl ResidentSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !(5..=50).contains(&self.battery_threshold) {
            return Err("battery_threshold must be between 5 and 50".into());
        }
        if self.start_at_login && !self.tray {
            return Err("start_at_login requires the tray component to be on".into());
        }
        Ok(())
    }

    /// Parse a settings file; a missing or unreadable file means the defaults (everything off).
    pub fn parse(text: &str) -> ResidentSettings {
        serde_json::from_str::<ResidentSettings>(text).ok().filter(|s| s.validate().is_ok()).unwrap_or_default()
    }

    /// Whether the startup value should exist for these settings.
    pub fn wants_startup_value(&self) -> bool {
        self.tray && self.start_at_login
    }
}

// ---------------------------------------------------------------------------------------------
// Startup value (RES-01/07)

/// The command stored in the startup value: the desktop program, started tray-only.
pub fn run_command(exe: &str) -> String {
    format!("\"{exe}\" {RESIDENT_ARG}")
}

/// Where the CleanManager-owned startup value lives. The real implementation is `HKCU\…\Run`; tests use a
/// throwaway key. Implementations only ever touch [`RUN_VALUE_NAME`].
pub trait StartupStore: Send + Sync {
    fn read(&self) -> Result<Option<String>, String>;
    fn write(&self, command: &str) -> Result<(), String>;
    /// Remove the value; `Ok(true)` when it existed.
    fn remove(&self) -> Result<bool, String>;
}

/// Make the startup value match the settings: written when wanted, removed otherwise.
pub fn sync_startup(store: &dyn StartupStore, settings: &ResidentSettings, exe: &str) -> Result<StartupState, String> {
    if settings.wants_startup_value() {
        let want = run_command(exe);
        if store.read()?.as_deref() != Some(want.as_str()) {
            store.write(&want)?;
        }
        Ok(StartupState::AtLogin)
    } else {
        store.remove()?;
        Ok(StartupState::Manual)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupState {
    /// Started only when the person opens CleanManager.
    Manual,
    /// The CleanManager-owned startup value exists.
    AtLogin,
}

// ---------------------------------------------------------------------------------------------
// The only background timer: a sampling loop that can be stopped and joined

/// A background loop that calls `tick` every `interval` until stopped. Dropping it stops it too.
pub struct ResidentLoop {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl ResidentLoop {
    /// Start the loop; the first tick runs immediately.
    pub fn start(name: &str, interval: Duration, mut tick: impl FnMut() + Send + 'static) -> std::io::Result<ResidentLoop> {
        Self::start_with(name, interval, || (), move |_| tick())
    }

    /// Like [`ResidentLoop::start`], with state created on the loop thread (for samplers that hold
    /// thread-affine handles) and dropped there when the loop ends.
    pub fn start_with<S: 'static>(name: &str, interval: Duration, init: impl FnOnce() -> S + Send + 'static, mut tick: impl FnMut(&mut S) + Send + 'static) -> std::io::Result<ResidentLoop> {
        let (tx, rx) = channel::<()>();
        let thread = std::thread::Builder::new().name(name.into()).spawn(move || {
            let mut state = init();
            loop {
                tick(&mut state);
                match rx.recv_timeout(interval) {
                    Err(RecvTimeoutError::Timeout) => continue,
                    // A stop request or the owner went away.
                    _ => break,
                }
            }
        })?;
        Ok(ResidentLoop { stop: Some(tx), thread: Some(thread) })
    }

    pub fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Stop and join: when this returns, the loop thread has exited and `tick` will not run again.
    pub fn stop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for ResidentLoop {
    fn drop(&mut self) {
        self.stop();
    }
}

/// What turning the resident component off did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TurnOffReport {
    /// A sampling loop was running and has been stopped and joined.
    pub sampling_stopped: bool,
    /// The CleanManager-owned startup value existed and was removed.
    pub startup_value_removed: bool,
    /// Removing the startup value failed (the settings are still saved as off; the next start retries).
    pub startup_error: Option<String>,
}

/// RES-01: stop sampling (joined, so no timer remains) and remove the startup value CleanManager created.
pub fn turn_off(sampling: &mut Option<ResidentLoop>, store: &dyn StartupStore) -> TurnOffReport {
    let sampling_stopped = match sampling.take() {
        Some(mut l) => {
            l.stop();
            true
        }
        None => false,
    };
    let (startup_value_removed, startup_error) = match store.remove() {
        Ok(removed) => (removed, None),
        Err(e) => (false, Some(e)),
    };
    TurnOffReport { sampling_stopped, startup_value_removed, startup_error }
}

// ---------------------------------------------------------------------------------------------
// Compact tray status (RES-01)

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TraySummary {
    pub sampled_at: String,
    pub cpu_percent: Option<f64>,
    pub memory_percent: Option<f64>,
    pub disk_free_bytes: Option<f64>,
}

fn available(s: &StatusSnapshot, name: &str) -> Option<f64> {
    s.metrics.iter().find(|m| m.name == name && m.availability == Availability::Available).and_then(|m| m.value)
}

/// Binary units, one decimal (the same wording as the desktop: "120.4 GiB").
pub fn format_bytes(b: f64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if b < 1024.0 {
        return format!("{} B", b.max(0.0).round());
    }
    let mut v = b;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    format!("{v:.1} {}", UNITS[unit])
}

fn pct(v: Option<f64>) -> String {
    v.map(|x| format!("{x:.0}%")).unwrap_or_else(|| "不可用".into())
}

impl TraySummary {
    pub fn of(s: &StatusSnapshot) -> TraySummary {
        TraySummary {
            sampled_at: s.sampled_at.clone(),
            cpu_percent: available(s, names::CPU_TOTAL),
            memory_percent: available(s, names::MEMORY_USED),
            disk_free_bytes: available(s, names::SYSTEM_FREE_BYTES),
        }
    }

    /// Menu lines (read-only, not clickable): one per metric; unavailable values say so instead of showing 0.
    pub fn lines(&self) -> [String; 3] {
        [
            format!("处理器 {}", pct(self.cpu_percent)),
            format!("内存已用 {}", pct(self.memory_percent)),
            format!("系统盘可用 {}", self.disk_free_bytes.map(format_bytes).unwrap_or_else(|| "不可用".into())),
        ]
    }

    /// Tooltip text, within the 127 characters Windows shows.
    pub fn tooltip(&self) -> String {
        let [c, m, d] = self.lines();
        let t = format!("CleanManager\n{c} · {m}\n{d}");
        t.chars().take(127).collect()
    }
}

/// Pages the tray may open. A fixed set: the tray has no other actions besides turning itself off and quitting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuickLink {
    Status,
    Processes,
    Resident,
    Cleanup,
}

impl QuickLink {
    pub const ALL: [QuickLink; 4] = [QuickLink::Status, QuickLink::Processes, QuickLink::Cleanup, QuickLink::Resident];

    /// Menu item id.
    pub fn id(self) -> &'static str {
        match self {
            QuickLink::Status => "open:status",
            QuickLink::Processes => "open:processes",
            QuickLink::Resident => "open:resident",
            QuickLink::Cleanup => "open:cleanup",
        }
    }

    /// The desktop page it opens (the WebView's page id).
    pub fn page(self) -> &'static str {
        match self {
            QuickLink::Status => "status",
            QuickLink::Processes => "processes",
            QuickLink::Resident => "resident",
            QuickLink::Cleanup => "cleanup",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            QuickLink::Status => "打开系统状态",
            QuickLink::Processes => "打开进程",
            QuickLink::Resident => "常驻工具设置",
            QuickLink::Cleanup => "打开清理",
        }
    }

    pub fn from_id(id: &str) -> Option<QuickLink> {
        QuickLink::ALL.into_iter().find(|q| q.id() == id)
    }
}

// ---------------------------------------------------------------------------------------------
// Keep the screen on and clean screen (RES-02)

/// Longest keep-awake duration that can be chosen (24 h); `None` means "until turned off or CleanManager exits".
pub const KEEP_AWAKE_MAX_MINUTES: u32 = 24 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeepAwakeRequest {
    pub minutes: Option<u32>,
}

impl KeepAwakeRequest {
    pub fn validate(&self) -> Result<Option<Duration>, String> {
        match self.minutes {
            None => Ok(None),
            Some(m) if (1..=KEEP_AWAKE_MAX_MINUTES).contains(&m) => Ok(Some(Duration::from_secs(u64::from(m) * 60))),
            Some(_) => Err(format!("minutes must be between 1 and {KEEP_AWAKE_MAX_MINUTES}")),
        }
    }
}

/// How long Esc must be held to leave clean-screen mode.
pub const ESC_HOLD: Duration = Duration::from_millis(2000);
/// Clean-screen timeout bounds and default.
pub const CLEAN_SCREEN_MIN_SECS: u32 = 5;
pub const CLEAN_SCREEN_MAX_SECS: u32 = 300;
pub const CLEAN_SCREEN_DEFAULT_SECS: u32 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanScreenOptions {
    pub timeout_secs: u32,
}

impl CleanScreenOptions {
    pub fn validate(&self) -> Result<Duration, String> {
        if (CLEAN_SCREEN_MIN_SECS..=CLEAN_SCREEN_MAX_SECS).contains(&self.timeout_secs) {
            Ok(Duration::from_secs(u64::from(self.timeout_secs)))
        } else {
            Err(format!("timeout_secs must be between {CLEAN_SCREEN_MIN_SECS} and {CLEAN_SCREEN_MAX_SECS}"))
        }
    }
}

/// Why clean-screen mode ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanScreenExit {
    /// Esc was held for [`ESC_HOLD`].
    EscHeld,
    /// The timeout was reached.
    Timeout,
    /// The window was closed some other way (session switch, Ctrl+Alt+Del, shutdown).
    Closed,
}

/// Tracks a long press of Esc from key-down/key-up events (auto-repeat key-downs are ignored).
#[derive(Debug, Default, Clone, Copy)]
pub struct EscHold {
    down_since_ms: Option<u64>,
}

impl EscHold {
    pub fn key_down(&mut self, now_ms: u64) {
        if self.down_since_ms.is_none() {
            self.down_since_ms = Some(now_ms);
        }
    }

    pub fn key_up(&mut self) {
        self.down_since_ms = None;
    }

    /// Esc has been held continuously for [`ESC_HOLD`].
    pub fn held(&self, now_ms: u64) -> bool {
        self.down_since_ms.is_some_and(|t| now_ms.saturating_sub(t) >= ESC_HOLD.as_millis() as u64)
    }
}

// ---------------------------------------------------------------------------------------------
// Privacy indicator (RES-03)

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Camera,
    Microphone,
}

/// One app's capability-access record (Windows: a ConsentStore key with its last-used times as FILETIMEs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentRecord {
    pub capability: Capability,
    /// Package family name, or the program path for desktop apps.
    pub key: String,
    /// Desktop (non-packaged) app: `key` is a path.
    pub desktop_app: bool,
    pub last_used_start: u64,
    pub last_used_stop: u64,
}

impl ConsentRecord {
    /// In use now: it started and has no stop time yet.
    pub fn in_use(&self) -> bool {
        self.last_used_start != 0 && self.last_used_stop == 0
    }
}

/// A readable name: the program file name for desktop apps (`C:#Program Files#App#app.exe` → `app.exe`),
/// the package name without the publisher id for packaged apps (`Microsoft.WindowsCamera_8wekyb3d8bbwe` →
/// `Microsoft.WindowsCamera`).
pub fn app_display_name(key: &str, desktop_app: bool) -> String {
    if desktop_app {
        let path = key.replace('#', "\\");
        path.rsplit(['\\', '/']).find(|s| !s.is_empty()).unwrap_or(&path).to_owned()
    } else {
        match key.rsplit_once('_') {
            Some((name, publisher)) if !name.is_empty() && publisher.len() == 13 && publisher.chars().all(|c| c.is_ascii_alphanumeric()) => name.to_owned(),
            _ => key.to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InUseApp {
    pub name: String,
    pub capabilities: Vec<Capability>,
    pub desktop_app: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrivacySnapshot {
    pub supported: bool,
    /// Apps using the camera or microphone right now (empty when none).
    pub in_use: Vec<InUseApp>,
    /// Why the indicator is unavailable (no reliable source or not readable).
    pub unavailable_reason: Option<String>,
    /// Where the information comes from.
    pub source: &'static str,
}

/// Group in-use records by app name; nothing is kept between snapshots.
pub fn in_use_apps(records: &[ConsentRecord]) -> Vec<InUseApp> {
    let mut by_name: BTreeMap<String, InUseApp> = BTreeMap::new();
    for r in records.iter().filter(|r| r.in_use()) {
        let name = app_display_name(&r.key, r.desktop_app);
        let e = by_name.entry(name.to_lowercase()).or_insert_with(|| InUseApp { name: name.clone(), capabilities: Vec::new(), desktop_app: r.desktop_app });
        if !e.capabilities.contains(&r.capability) {
            e.capabilities.push(r.capability);
            e.capabilities.sort();
        }
    }
    by_name.into_values().collect()
}

// ---------------------------------------------------------------------------------------------
// Device battery (RES-04)

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceBattery {
    /// Stable id for alerts (`system` for this computer's battery, otherwise the device instance id).
    pub device_id: String,
    pub name: String,
    pub percent: u8,
    /// `system` (this computer) or `bluetooth`.
    pub kind: &'static str,
    /// Charging or on external power, when known.
    pub charging: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BatteryAlert {
    pub device_id: String,
    pub name: String,
    pub percent: u8,
    pub threshold: u8,
}

/// Points above the threshold a device must recover to before it can alert again.
pub const BATTERY_REARM_MARGIN: u8 = 5;

/// One alert per device each time it falls below the threshold; re-armed when it recovers (or charges).
#[derive(Debug, Default, Clone)]
pub struct BatteryAlerts {
    alerted: BTreeMap<String, ()>,
}

impl BatteryAlerts {
    pub fn observe(&mut self, threshold: u8, devices: &[DeviceBattery]) -> Vec<BatteryAlert> {
        let mut out = Vec::new();
        for d in devices {
            let low = d.percent < threshold && d.charging != Some(true);
            if low {
                if self.alerted.insert(d.device_id.clone(), ()).is_none() {
                    out.push(BatteryAlert { device_id: d.device_id.clone(), name: d.name.clone(), percent: d.percent, threshold });
                }
            } else if d.charging == Some(true) || d.percent >= threshold.saturating_add(BATTERY_REARM_MARGIN) {
                self.alerted.remove(&d.device_id);
            }
        }
        // Devices that disappeared are forgotten, so a reconnect below the threshold alerts again.
        self.alerted.retain(|id, _| devices.iter().any(|d| &d.device_id == id));
        out
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::status::Metric;

    struct MemStore(Mutex<Option<String>>);

    impl StartupStore for MemStore {
        fn read(&self) -> Result<Option<String>, String> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn write(&self, command: &str) -> Result<(), String> {
            *self.0.lock().unwrap() = Some(command.to_owned());
            Ok(())
        }
        fn remove(&self) -> Result<bool, String> {
            Ok(self.0.lock().unwrap().take().is_some())
        }
    }

    #[test]
    fn everything_is_off_by_default_and_bad_files_mean_off() {
        let d = ResidentSettings::default();
        assert!(!d.tray && !d.start_at_login && !d.battery_alerts);
        assert_eq!(ResidentSettings::parse("not json"), d);
        assert_eq!(ResidentSettings::parse(r#"{"tray":false,"start_at_login":true}"#), d, "start at login without the tray is refused");
        assert_eq!(ResidentSettings::parse(r#"{"battery_threshold":90}"#), d);
        assert!(ResidentSettings::parse(r#"{"tray":true,"start_at_login":true}"#).wants_startup_value());
    }

    #[test]
    fn startup_value_follows_the_settings_and_turning_off_removes_it() {
        let store = MemStore(Mutex::new(None));
        let on = ResidentSettings { tray: true, start_at_login: true, ..Default::default() };
        assert_eq!(sync_startup(&store, &on, r"C:\Apps\cleanmanager-desktop.exe").unwrap(), StartupState::AtLogin);
        assert_eq!(store.read().unwrap().as_deref(), Some(r#""C:\Apps\cleanmanager-desktop.exe" --resident"#));
        let tray_only = ResidentSettings { tray: true, ..Default::default() };
        assert_eq!(sync_startup(&store, &tray_only, "x").unwrap(), StartupState::Manual);
        assert_eq!(store.read().unwrap(), None);
        store.write("y").unwrap();
        let mut none = None;
        let r = turn_off(&mut none, &store);
        assert!(r.startup_value_removed && !r.sampling_stopped && r.startup_error.is_none());
        assert_eq!(store.read().unwrap(), None);
    }

    #[test]
    fn turning_off_stops_and_joins_the_sampling_loop() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let mut sampling = Some(ResidentLoop::start("test-resident", Duration::from_millis(10), move || {
            c.fetch_add(1, Ordering::SeqCst);
        }).unwrap());
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while calls.load(Ordering::SeqCst) < 3 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(sampling.as_ref().unwrap().is_running());
        let store = MemStore(Mutex::new(Some("cmd".into())));
        let report = turn_off(&mut sampling, &store);
        assert!(report.sampling_stopped && report.startup_value_removed);
        assert!(sampling.is_none());
        let after = calls.load(Ordering::SeqCst);
        assert!(after >= 3);
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(calls.load(Ordering::SeqCst), after, "no tick after turning off");
    }

    #[test]
    fn dropping_the_loop_stops_it() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let l = ResidentLoop::start("test-drop", Duration::from_millis(5), move || {
            c.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(30));
        drop(l);
        let after = calls.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(calls.load(Ordering::SeqCst), after);
    }

    #[test]
    fn tray_summary_says_unavailable_instead_of_zero() {
        let s = StatusSnapshot::new(
            "2026-10-15T01:00:00Z".into(),
            vec![
                Metric::value(names::CPU_TOTAL, 23.4, "percent"),
                Metric::unavailable(names::MEMORY_USED, "percent", "x"),
                Metric::value(names::SYSTEM_FREE_BYTES, 129_284_829_184.0, "bytes"),
            ],
            None,
        );
        let t = TraySummary::of(&s);
        assert_eq!(t.lines(), ["处理器 23%".to_owned(), "内存已用 不可用".to_owned(), "系统盘可用 120.4 GiB".to_owned()]);
        assert!(t.tooltip().chars().count() <= 127);
        assert!(t.tooltip().starts_with("CleanManager\n"));
    }

    #[test]
    fn quick_links_are_a_fixed_set_of_pages() {
        for q in QuickLink::ALL {
            assert_eq!(QuickLink::from_id(q.id()), Some(q));
        }
        assert_eq!(QuickLink::from_id("clean:run"), None);
        assert_eq!(QuickLink::from_id("open:settings"), None);
    }

    #[test]
    fn keep_awake_and_clean_screen_bounds() {
        assert_eq!(KeepAwakeRequest { minutes: None }.validate().unwrap(), None);
        assert_eq!(KeepAwakeRequest { minutes: Some(30) }.validate().unwrap(), Some(Duration::from_secs(1800)));
        assert!(KeepAwakeRequest { minutes: Some(0) }.validate().is_err());
        assert!(KeepAwakeRequest { minutes: Some(KEEP_AWAKE_MAX_MINUTES + 1) }.validate().is_err());
        assert!(CleanScreenOptions { timeout_secs: 4 }.validate().is_err());
        assert!(CleanScreenOptions { timeout_secs: 301 }.validate().is_err());
        assert_eq!(CleanScreenOptions { timeout_secs: 60 }.validate().unwrap(), Duration::from_secs(60));
    }

    #[test]
    fn esc_must_be_held_continuously() {
        let mut e = EscHold::default();
        e.key_down(1000);
        e.key_down(1500); // auto-repeat does not restart the hold
        assert!(!e.held(2999));
        assert!(e.held(3000));
        e.key_up();
        assert!(!e.held(9000));
        e.key_down(9000);
        e.key_up();
        e.key_down(9500);
        assert!(!e.held(11_000));
        assert!(e.held(11_500));
    }

    #[test]
    fn privacy_lists_apps_in_use_by_name() {
        let r = |cap, key: &str, desktop, start, stop| ConsentRecord { capability: cap, key: key.into(), desktop_app: desktop, last_used_start: start, last_used_stop: stop };
        let records = vec![
            r(Capability::Microphone, "C:#Program Files#Zoom#bin#Zoom.exe", true, 10, 0),
            r(Capability::Camera, "C:#Program Files#Zoom#bin#Zoom.exe", true, 11, 0),
            r(Capability::Camera, "Microsoft.WindowsCamera_8wekyb3d8bbwe", false, 5, 9),
            r(Capability::Microphone, "E:#Microsoft VS Code#Code.exe", true, 134_361_051_770_989_569, 134_361_051_771_035_299),
            r(Capability::Microphone, "Never.Used_8wekyb3d8bbwe", false, 0, 0),
        ];
        let apps = in_use_apps(&records);
        assert_eq!(apps, vec![InUseApp { name: "Zoom.exe".into(), capabilities: vec![Capability::Camera, Capability::Microphone], desktop_app: true }]);
        assert_eq!(app_display_name("Microsoft.WindowsCamera_8wekyb3d8bbwe", false), "Microsoft.WindowsCamera");
        assert_eq!(app_display_name("Some_Name", false), "Some_Name");
        // When it stops, the indicator clears.
        let stopped = vec![r(Capability::Microphone, "C:#Program Files#Zoom#bin#Zoom.exe", true, 10, 20)];
        assert!(in_use_apps(&stopped).is_empty());
    }

    #[test]
    fn battery_alerts_once_and_rearms_after_recovery() {
        let kb = |p, charging| DeviceBattery { device_id: "BTHENUM\\kb".into(), name: "Keyboard".into(), percent: p, kind: "bluetooth", charging };
        let mut a = BatteryAlerts::default();
        assert!(a.observe(20, &[kb(40, None)]).is_empty());
        assert_eq!(a.observe(20, &[kb(19, None)]).len(), 1);
        assert!(a.observe(20, &[kb(15, None)]).is_empty(), "only once while low");
        assert!(a.observe(20, &[kb(22, None)]).is_empty(), "within the margin: still armed off");
        assert!(a.observe(20, &[kb(18, None)]).is_empty());
        assert!(a.observe(20, &[kb(25, None)]).is_empty());
        assert_eq!(a.observe(20, &[kb(18, None)]).len(), 1, "re-armed after recovering");
        assert!(a.observe(20, &[kb(10, Some(true))]).is_empty(), "charging never alerts");
        assert_eq!(a.observe(20, &[kb(10, None)]).len(), 1);
        assert!(a.observe(20, &[]).is_empty());
        assert_eq!(a.observe(20, &[kb(10, None)]).len(), 1, "a reconnected device below the threshold alerts again");
    }
}
