//! Windows resident utilities (add-mole-parity-suite 15.1/15.2, RES-01..04/07). Clean-screen mode is in
//! `resident_screen`.
//!
//! - Startup value: [`RunValue`] reads, writes and removes only `CleanManager.Resident` under a per-user key
//!   (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run` for real; tests pass a throwaway key).
//! - Keep the screen on: [`KeepAwake`] holds `SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED |
//!   ES_SYSTEM_REQUIRED)` on its own thread and clears it (`ES_CONTINUOUS`) when released, when the duration ends
//!   or when it is dropped; the request is per thread, so it also ends if the process exits. No power plan or
//!   timeout setting is changed (`powercfg /requests` lists it while active).
//! - Privacy indicator: [`privacy_snapshot`] reads the capability-access records under
//!   `HKCU\…\CapabilityAccessManager\ConsentStore\{webcam,microphone}` (read-only; an app is in use while its
//!   `LastUsedTimeStop` is 0 after a start). Nothing is stored or sent.
//! - Batteries: [`system_battery`] (`GetSystemPowerStatus`) and [`device_batteries`], which reads the device
//!   property `{104EA319-6EE2-4701-BD47-8DDBF425BBE5},2` that the Windows Bluetooth stack sets on devices that
//!   report a battery level (the value Settings › Bluetooth & devices shows). Devices without it are not guessed.

use std::sync::mpsc::{Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cm_core::resident::{Capability, ConsentRecord, DeviceBattery, PrivacySnapshot, RUN_SUBKEY, RUN_VALUE_NAME, StartupStore, in_use_apps};
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_READ, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_QWORD, RRF_RT_REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegDeleteValueW, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---------------------------------------------------------------------------------------------
// Registry helpers (HKCU only)

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: a handle opened below, closed once.
        unsafe { RegCloseKey(self.0) };
    }
}

fn open(parent: HKEY, subkey: &str, access: u32) -> Result<Option<Key>, u32> {
    let sub = wide(subkey);
    let mut hk: HKEY = std::ptr::null_mut();
    // SAFETY: valid parent handle, NUL-terminated subkey, out-pointer to a local.
    let rc = unsafe { RegOpenKeyExW(parent, sub.as_ptr(), 0, access, &mut hk) };
    match rc {
        0 => Ok(Some(Key(hk))),
        ERROR_FILE_NOT_FOUND => Ok(None),
        e => Err(e),
    }
}

fn subkeys(key: &Key) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0.. {
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        // SAFETY: buffer of `len` UTF-16 units; optional outputs are null.
        let rc = unsafe { RegEnumKeyExW(key.0, i, buf.as_mut_ptr(), &mut len, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
        if rc == ERROR_NO_MORE_ITEMS {
            break;
        }
        if rc != 0 {
            continue;
        }
        out.push(String::from_utf16_lossy(&buf[..len as usize]));
    }
    out
}

fn qword(key: &Key, subkey: &str, name: &str) -> u64 {
    let (sub, n) = (wide(subkey), wide(name));
    let mut v = 0u64;
    let mut size = 8u32;
    // SAFETY: 8-byte output for a REG_QWORD; other values are refused by the type filter.
    let rc = unsafe { RegGetValueW(key.0, sub.as_ptr(), n.as_ptr(), RRF_RT_REG_QWORD, std::ptr::null_mut(), (&mut v as *mut u64).cast(), &mut size) };
    if rc == 0 { v } else { 0 }
}

// ---------------------------------------------------------------------------------------------
// Startup value (RES-01/07)

/// The CleanManager-owned startup value under a per-user key. Only [`RUN_VALUE_NAME`] is ever touched.
pub struct RunValue {
    /// Under HKCU. [`RunValue::real`] uses the per-user `Run` key; tests pass a throwaway key.
    pub subkey: String,
}

impl RunValue {
    pub fn real() -> RunValue {
        RunValue { subkey: RUN_SUBKEY.into() }
    }

    /// Display form for lifecycle views and `self uninstall`.
    pub fn display(&self) -> String {
        format!("HKCU\\{}\\{}", self.subkey, RUN_VALUE_NAME)
    }
}

impl StartupStore for RunValue {
    fn read(&self) -> Result<Option<String>, String> {
        let Some(key) = open(HKEY_CURRENT_USER, &self.subkey, KEY_QUERY_VALUE).map_err(|e| format!("cannot open HKCU\\{} (error {e})", self.subkey))? else { return Ok(None) };
        let name = wide(RUN_VALUE_NAME);
        let mut size = 0u32;
        // SAFETY: size query with a null buffer.
        let rc = unsafe { RegGetValueW(key.0, std::ptr::null(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut size) };
        if rc == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if rc != 0 {
            return Err(format!("cannot read {} (error {rc})", self.display()));
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut got = (buf.len() * 2) as u32;
        // SAFETY: buffer of `got` bytes.
        let rc = unsafe { RegGetValueW(key.0, std::ptr::null(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut got) };
        if rc != 0 {
            return Err(format!("cannot read {} (error {rc})", self.display()));
        }
        let end = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
        Ok(Some(String::from_utf16_lossy(&buf[..end])))
    }

    fn write(&self, command: &str) -> Result<(), String> {
        let sub = wide(&self.subkey);
        let mut hk: HKEY = std::ptr::null_mut();
        // SAFETY: valid NUL-terminated subkey, out-pointer to a local handle.
        let rc = unsafe {
            RegCreateKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, std::ptr::null(), &mut hk, std::ptr::null_mut())
        };
        if rc != 0 {
            return Err(format!("cannot open HKCU\\{} (error {rc})", self.subkey));
        }
        let key = Key(hk);
        let (name, data) = (wide(RUN_VALUE_NAME), wide(command));
        // SAFETY: NUL-terminated UTF-16 data of the given byte length.
        let rc = unsafe { RegSetValueExW(key.0, name.as_ptr(), 0, REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32) };
        if rc != 0 {
            return Err(format!("cannot write {} (error {rc})", self.display()));
        }
        Ok(())
    }

    fn remove(&self) -> Result<bool, String> {
        let Some(key) = open(HKEY_CURRENT_USER, &self.subkey, KEY_SET_VALUE).map_err(|e| format!("cannot open HKCU\\{} (error {e})", self.subkey))? else { return Ok(false) };
        let name = wide(RUN_VALUE_NAME);
        // SAFETY: valid key handle and NUL-terminated value name.
        match unsafe { RegDeleteValueW(key.0, name.as_ptr()) } {
            0 => Ok(true),
            ERROR_FILE_NOT_FOUND => Ok(false),
            e => Err(format!("cannot remove {} (error {e})", self.display())),
        }
    }
}

/// Delete a throwaway test key (`HKCU\Software\CleanManagerTest\…` only).
pub fn delete_test_key(subkey: &str) {
    assert!(subkey.starts_with(r"Software\CleanManagerTest\"), "only throwaway test keys may be deleted");
    let sub = wide(subkey);
    // SAFETY: valid NUL-terminated subkey under HKCU.
    unsafe { windows_sys::Win32::System::Registry::RegDeleteTreeW(HKEY_CURRENT_USER, sub.as_ptr()) };
    // The shared parent `Software\CleanManagerTest` is left in place: deleting it while a test running in parallel
    // opens a key below it fails that test with ERROR_KEY_DELETED (1018). The CLI uninstall tests remove the tree.
}

// ---------------------------------------------------------------------------------------------
// Keep the screen on (RES-02)

/// An active "keep the screen on" request. Released by [`KeepAwake::release`], when its duration ends, on drop,
/// or with the process.
pub struct KeepAwake {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
    until: Option<Instant>,
}

impl KeepAwake {
    pub fn start(duration: Option<Duration>) -> Result<KeepAwake, String> {
        use windows_sys::Win32::System::Power::{ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState};
        let (stop_tx, stop_rx) = channel::<()>();
        let (ready_tx, ready_rx) = channel::<bool>();
        let thread = std::thread::Builder::new()
            .name("cm-keep-awake".into())
            .spawn(move || {
                // SAFETY: plain flag call; the state belongs to this thread.
                let ok = unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED) } != 0;
                let _ = ready_tx.send(ok);
                if !ok {
                    return;
                }
                let _ = match duration {
                    Some(d) => stop_rx.recv_timeout(d).map_err(|_| ()),
                    None => stop_rx.recv().map_err(|_| ()),
                };
                // SAFETY: clears this thread's request (back to the system's own idle timers).
                unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
            })
            .map_err(|e| e.to_string())?;
        match ready_rx.recv() {
            Ok(true) => Ok(KeepAwake { stop: Some(stop_tx), thread: Some(thread), until: duration.map(|d| Instant::now() + d) }),
            _ => {
                let _ = thread.join();
                Err("SetThreadExecutionState refused the request".into())
            }
        }
    }

    /// Still holding the request (false once the duration ended).
    pub fn is_active(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    pub fn remaining(&self) -> Option<Duration> {
        self.until.map(|u| u.saturating_duration_since(Instant::now()))
    }

    /// Clear the request and wait until it is cleared.
    pub fn release(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        self.release();
    }
}

// ---------------------------------------------------------------------------------------------
// Privacy indicator (RES-03)

/// Real location of the capability-access records.
pub const CONSENT_STORE: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore";
const SOURCE: &str = "HKCU CapabilityAccessManager ConsentStore (webcam, microphone): LastUsedTimeStart set and LastUsedTimeStop = 0";

/// Read every camera/microphone record under `base` (an HKCU subkey). Read-only.
pub fn consent_records(base: &str) -> Result<Vec<ConsentRecord>, String> {
    let mut out = Vec::new();
    let mut readable = false;
    for (cap, sub) in [(Capability::Camera, "webcam"), (Capability::Microphone, "microphone")] {
        let path = format!("{base}\\{sub}");
        let key = match open(HKEY_CURRENT_USER, &path, KEY_READ) {
            Ok(Some(k)) => k,
            Ok(None) => continue,
            Err(e) => return Err(format!("cannot read HKCU\\{path} (error {e})")),
        };
        readable = true;
        for name in subkeys(&key) {
            if name.eq_ignore_ascii_case("NonPackaged") {
                let Ok(Some(np)) = open(key.0, &name, KEY_READ) else { continue };
                for app in subkeys(&np) {
                    out.push(ConsentRecord { capability: cap, last_used_start: qword(&np, &app, "LastUsedTimeStart"), last_used_stop: qword(&np, &app, "LastUsedTimeStop"), key: app, desktop_app: true });
                }
            } else {
                out.push(ConsentRecord { capability: cap, last_used_start: qword(&key, &name, "LastUsedTimeStart"), last_used_stop: qword(&key, &name, "LastUsedTimeStop"), key: name, desktop_app: false });
            }
        }
    }
    if readable { Ok(out) } else { Err("the capability-access records are not present on this system".into()) }
}

pub fn privacy_snapshot_at(base: &str) -> PrivacySnapshot {
    match consent_records(base) {
        Ok(records) => PrivacySnapshot { supported: true, in_use: in_use_apps(&records), unavailable_reason: None, source: SOURCE },
        Err(e) => PrivacySnapshot { supported: false, in_use: Vec::new(), unavailable_reason: Some(e), source: SOURCE },
    }
}

pub fn privacy_snapshot() -> PrivacySnapshot {
    privacy_snapshot_at(CONSENT_STORE)
}

// ---------------------------------------------------------------------------------------------
// Batteries (RES-04)

/// This computer's battery from `GetSystemPowerStatus`; `None` when there is no system battery or it is unknown.
pub fn system_battery() -> Option<DeviceBattery> {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut s = SYSTEM_POWER_STATUS { ACLineStatus: 0, BatteryFlag: 0, BatteryLifePercent: 0, SystemStatusFlag: 0, BatteryLifeTime: 0, BatteryFullLifeTime: 0 };
    // SAFETY: valid out-pointer.
    if unsafe { GetSystemPowerStatus(&mut s) } == 0 || s.BatteryFlag == 128 || s.BatteryFlag == 255 || s.BatteryLifePercent > 100 {
        return None;
    }
    let charging = if s.BatteryFlag & 8 != 0 || s.ACLineStatus == 1 { Some(true) } else if s.ACLineStatus == 0 { Some(false) } else { None };
    Some(DeviceBattery { device_id: "system".into(), name: "本机电池".into(), percent: s.BatteryLifePercent, kind: "system", charging })
}

/// Result of reading device batteries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceScan {
    pub devices: Vec<DeviceBattery>,
    /// Present Bluetooth devices (Classic and LE device nodes), whether or not they report a level.
    pub bluetooth_devices: usize,
}

/// Property set by the Windows Bluetooth stack on devices that report their battery level (byte, 0–100).
const BATTERY_FMTID: u128 = 0x104E_A319_6EE2_4701_BD47_8DDB_F425_BBE5;

pub fn device_batteries() -> Result<DeviceScan, String> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        DIGCF_ALLCLASSES, DIGCF_PRESENT, SP_DEVINFO_DATA, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDeviceInstanceIdW,
        SetupDiGetDevicePropertyW,
    };
    use windows_sys::Win32::Devices::Properties::{DEVPKEY_Device_DeviceDesc, DEVPKEY_Device_FriendlyName, DEVPROP_TYPE_BYTE, DEVPROP_TYPE_STRING, DEVPROPTYPE};
    use windows_sys::Win32::Foundation::{DEVPROPKEY, INVALID_HANDLE_VALUE};
    use windows_sys::core::GUID;

    // SAFETY: enumerate all present device nodes; the set is destroyed below.
    let set = unsafe { SetupDiGetClassDevsW(std::ptr::null(), std::ptr::null(), std::ptr::null_mut(), DIGCF_ALLCLASSES | DIGCF_PRESENT) };
    if set as isize == INVALID_HANDLE_VALUE as isize {
        return Err("the device list could not be read".into());
    }
    struct Set(windows_sys::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO);
    impl Drop for Set {
        fn drop(&mut self) {
            // SAFETY: the set from SetupDiGetClassDevsW, destroyed once.
            unsafe { SetupDiDestroyDeviceInfoList(self.0) };
        }
    }
    let set = Set(set);
    let battery_key = DEVPROPKEY { fmtid: GUID::from_u128(BATTERY_FMTID), pid: 2 };
    let string_prop = |info: &SP_DEVINFO_DATA, key: &DEVPROPKEY| -> Option<String> {
        let mut ty: DEVPROPTYPE = 0;
        let mut buf = [0u16; 260];
        let mut need = 0u32;
        // SAFETY: buffer of the given byte size; output type checked below.
        let ok = unsafe { SetupDiGetDevicePropertyW(set.0, info, key, &mut ty, buf.as_mut_ptr().cast(), (buf.len() * 2) as u32, &mut need, 0) } != 0;
        if !ok || ty != DEVPROP_TYPE_STRING {
            return None;
        }
        let end = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end])).filter(|s| !s.trim().is_empty())
    };
    let mut devices = Vec::new();
    let mut bluetooth_devices = 0;
    for i in 0.. {
        let mut info = SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ClassGuid: GUID::from_u128(0), DevInst: 0, Reserved: 0 };
        // SAFETY: valid set and a sized SP_DEVINFO_DATA.
        if unsafe { SetupDiEnumDeviceInfo(set.0, i, &mut info) } == 0 {
            break;
        }
        let mut id = [0u16; 512];
        let mut need = 0u32;
        // SAFETY: buffer of `id.len()` UTF-16 units.
        let id = if unsafe { SetupDiGetDeviceInstanceIdW(set.0, &info, id.as_mut_ptr(), id.len() as u32, &mut need) } != 0 {
            let end = id.iter().position(|&u| u == 0).unwrap_or(id.len());
            String::from_utf16_lossy(&id[..end])
        } else {
            continue;
        };
        let upper = id.to_ascii_uppercase();
        if upper.starts_with("BTHENUM\\DEV_") || upper.starts_with("BTHLE\\DEV_") {
            bluetooth_devices += 1;
        }
        let mut ty: DEVPROPTYPE = 0;
        let mut level = 0u8;
        // SAFETY: one-byte output; the type is checked below.
        let ok = unsafe { SetupDiGetDevicePropertyW(set.0, &info, &battery_key, &mut ty, &mut level, 1, &mut need, 0) } != 0;
        if !ok || ty != DEVPROP_TYPE_BYTE || level > 100 {
            continue;
        }
        let name = string_prop(&info, &DEVPKEY_Device_FriendlyName).or_else(|| string_prop(&info, &DEVPKEY_Device_DeviceDesc)).unwrap_or_else(|| id.clone());
        devices.push(DeviceBattery { device_id: id, name, percent: level, kind: "bluetooth", charging: None });
    }
    devices.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.device_id.cmp(&b.device_id)));
    // One physical device can expose the level on several nodes (hands-free and audio sink): keep one per name.
    devices.dedup_by(|a, b| a.name == b.name && a.percent == b.percent);
    Ok(DeviceScan { devices, bluetooth_devices })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key(tag: &str) -> String {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        format!(r"Software\CleanManagerTest\{tag}-{}-{nanos}", std::process::id())
    }

    #[test]
    fn run_value_is_written_read_and_removed_in_a_throwaway_key() {
        let key = test_key("run");
        let rv = RunValue { subkey: key.clone() };
        assert_eq!(rv.read().unwrap(), None, "a missing key reads as no value");
        assert!(!rv.remove().unwrap());
        rv.write(r#""C:\x\cleanmanager-desktop.exe" --resident"#).unwrap();
        assert_eq!(rv.read().unwrap().as_deref(), Some(r#""C:\x\cleanmanager-desktop.exe" --resident"#));
        let mut sampling = None;
        let report = cm_core::resident::turn_off(&mut sampling, &rv);
        assert!(report.startup_value_removed && report.startup_error.is_none());
        assert_eq!(rv.read().unwrap(), None, "no Run value remains after turning off");
        delete_test_key(&key);
        assert!(RunValue::real().display().ends_with(r"CurrentVersion\Run\CleanManager.Resident"));
    }

    #[test]
    fn consent_records_are_read_from_an_injected_root() {
        use windows_sys::Win32::System::Registry::{REG_QWORD, RegCreateKeyExW, RegSetValueExW};
        let base = test_key("consent");
        let make = |sub: &str, start: u64, stop: u64| {
            let w = wide(&format!("{base}\\{sub}"));
            let mut hk: HKEY = std::ptr::null_mut();
            // SAFETY: throwaway test key under HKCU.
            unsafe {
                assert_eq!(RegCreateKeyExW(HKEY_CURRENT_USER, w.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, std::ptr::null(), &mut hk, std::ptr::null_mut()), 0);
                for (n, v) in [("LastUsedTimeStart", start), ("LastUsedTimeStop", stop)] {
                    let n = wide(n);
                    RegSetValueExW(hk, n.as_ptr(), 0, REG_QWORD, (&v as *const u64).cast(), 8);
                }
                RegCloseKey(hk);
            }
        };
        make(r"microphone\NonPackaged\C:#Tools#Rec#rec.exe", 100, 0);
        make(r"microphone\Microsoft.WindowsCamera_8wekyb3d8bbwe", 100, 200);
        make(r"webcam\Contoso.Meet_abcdefghjkmnp", 100, 0);
        let snap = privacy_snapshot_at(&base);
        assert!(snap.supported);
        let names: Vec<&str> = snap.in_use.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["Contoso.Meet", "rec.exe"]);
        // When the app stops (LastUsedTimeStop set), the indicator clears.
        make(r"microphone\NonPackaged\C:#Tools#Rec#rec.exe", 100, 300);
        make(r"webcam\Contoso.Meet_abcdefghjkmnp", 100, 300);
        assert!(privacy_snapshot_at(&base).in_use.is_empty());
        delete_test_key(&base);
        let gone = privacy_snapshot_at(&base);
        assert!(!gone.supported && gone.unavailable_reason.is_some());
    }

    #[test]
    fn keep_awake_holds_and_releases_on_its_own_thread() {
        let mut k = KeepAwake::start(Some(Duration::from_secs(30))).unwrap();
        assert!(k.is_active());
        assert!(k.remaining().unwrap() <= Duration::from_secs(30));
        k.release();
        assert!(!k.is_active());
        let short = KeepAwake::start(Some(Duration::from_millis(20))).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(!short.is_active(), "the request ends with its duration");
    }

    #[test]
    fn device_and_system_battery_reads_do_not_fail() {
        let scan = device_batteries().unwrap();
        assert!(scan.devices.iter().all(|d| d.percent <= 100));
        if let Some(b) = system_battery() {
            assert!(b.percent <= 100);
        }
    }

    /// Real-device read-only snapshot of the privacy indicator and batteries (prints JSON for the evidence file):
    /// `cargo test -p cm-platform real_device_snapshot -- --ignored --nocapture`
    #[test]
    #[ignore = "real device: prints what this computer reports"]
    fn real_device_snapshot() {
        let privacy = privacy_snapshot();
        let records = consent_records(CONSENT_STORE).unwrap_or_default();
        let devices = device_batteries().unwrap();
        let system = system_battery();
        let count = |cap: Capability| records.iter().filter(|r| r.capability == cap).count();
        println!("privacy: {privacy:?}");
        println!(
            "consent records: total={} camera={} microphone={} in_use={}",
            records.len(),
            count(Capability::Camera),
            count(Capability::Microphone),
            records.iter().filter(|r| r.in_use()).count()
        );
        println!("bluetooth devices present: {}; device batteries: {:?}", devices.bluetooth_devices, devices.devices);
        println!("system battery: {system:?}");
        assert!(privacy.supported, "{:?}", privacy.unavailable_reason);
    }

    /// Real-device check: the keep-awake request appears in `powercfg /requests` (needs an elevated shell) and is
    /// gone after release. `cargo test -p cm-platform keep_awake_shows_in_powercfg -- --ignored --nocapture`
    #[test]
    #[ignore = "real device; needs an elevated shell for powercfg /requests"]
    fn keep_awake_shows_in_powercfg() {
        let requests = || {
            let out = std::process::Command::new("powercfg").arg("/requests").output().unwrap();
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let me = std::env::current_exe().unwrap().file_name().unwrap().to_string_lossy().to_lowercase();
        // Sections are a header line ("DISPLAY:") followed by "[PROCESS] …" lines.
        let has_me = |text: &str| -> (bool, bool) {
            let mut current = String::new();
            let (mut display, mut system) = (false, false);
            for line in text.lines().map(str::trim) {
                if !line.starts_with('[') && line.ends_with(':') {
                    current = line.trim_end_matches(':').to_owned();
                } else if line.to_lowercase().contains(&me) {
                    display |= current == "DISPLAY";
                    system |= current == "SYSTEM";
                }
            }
            (display, system)
        };
        assert_eq!(has_me(&requests()), (false, false));
        let mut k = KeepAwake::start(None).unwrap();
        let during = requests();
        println!("during:\n{during}");
        assert_eq!(has_me(&during), (true, true));
        k.release();
        let after = requests();
        println!("after:\n{after}");
        assert_eq!(has_me(&after), (false, false));
    }
}
