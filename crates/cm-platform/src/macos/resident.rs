//! macOS resident utilities (add-mole-parity-suite 15.2 on macOS; RES-02..04, RES-07; D12). Everything here is
//! read-only except the keep-awake power assertion and, during clean-screen mode, this app's own presentation options.
//!
//! - Keep the screen on: [`KeepAwake`] holds one `IOPMAssertionCreateWithName(kIOPMAssertPreventUserIdleDisplaySleep)`
//!   assertion (while the display is kept on the system does not idle-sleep either) and releases it with
//!   `IOPMAssertionRelease` when released, when its duration ends or when dropped; powerd also drops it when the process
//!   exits. No energy setting is changed. [`process_assertions`] reads `IOPMCopyAssertionsByProcess` (what
//!   `pmset -g assertions` lists under "Listed by owning process").
//! - Privacy indicator: [`privacy_reading`] asks CoreMediaIO whether any camera is running somewhere
//!   (`kCMIODevicePropertyDeviceIsRunningSomewhere` over `kCMIOHardwarePropertyDevices`) and CoreAudio whether any input
//!   device is (`kAudioDevicePropertyDeviceIsRunningSomewhere` over the devices with input streams). Apps are named only
//!   where a public API names them: CoreAudio's process objects (`kAudioHardwarePropertyProcessObjectList`,
//!   `kAudioProcessPropertyIsRunningInput`, macOS 14 and later) for the microphone. No public API names the app using a
//!   camera, so a running camera is shown as "app unknown". A property that cannot be read makes the state unknown with
//!   the reason, never "not in use". Nothing is stored or sent.
//! - Batteries: [`system_battery`] from the IOPS power-source snapshot (internal battery only) and
//!   [`device_batteries`] from the I/O Registry `BatteryPercent` property that Apple's HID peripheral drivers publish
//!   (`AppleDeviceManagementHIDEventService` and the older Bluetooth HID classes). Bluetooth HID devices without that
//!   property are listed as unavailable, never as 0%.
//! - Start at login: [`LoginItem`] reports it unsupported. `SMAppService.mainApp` registers only a signed app bundle;
//!   this build is unsigned, and no hidden `~/Library/LaunchAgents` job is written instead.
//! - Clean screen: [`begin_clean_presentation`] / [`end_clean_presentation`] set and clear this app's
//!   `NSApplicationPresentationOptions` (Dock and menu bar hidden, app switching and hiding disabled) on the main
//!   thread; the desktop host draws the black windows. Force Quit (Option-Command-Esc) stays available.

use std::collections::BTreeSet;
use std::ffi::{CStr, c_void};
use std::ptr;
use std::sync::mpsc::{Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cm_core::capability::CapabilityState;
use cm_core::resident::{Capability, DeviceBattery, InUseApp, PrivacySnapshot, StartupStore};

use super::ffi::cf::{self, CFTypeRef, CfRef, Plist};
use super::ffi::iokit::{self, Service};
use super::ffi::objc::{self, Id, Sel};
use super::procs;

/// Why the menu-bar component cannot start at login in this build.
pub const LOGIN_ITEM_REASON: &str = "Starting the menu-bar component at login needs SMAppService.mainApp, which registers only a signed and \
     notarized app bundle; this build is unsigned, and CleanManager does not write a hidden ~/Library/LaunchAgents job instead";

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[
    (
        "resident.utilities",
        CapabilityState::Unverified,
        "Menu-bar status item, keep the screen on (IOPMAssertion PreventUserIdleDisplaySleep), camera and microphone in use \
         (CoreMediaIO and CoreAudio DeviceIsRunningSomewhere; app names only for the microphone, from CoreAudio process \
         objects on macOS 14 and later) and battery levels (IOPS internal battery, I/O Registry BatteryPercent of HID \
         peripherals) are implemented and tested on hosted runners only (no camera, battery or Bluetooth device there), not on \
         a real Mac; clean-screen mode (borderless always-on-top windows plus presentation options) was run once on a hosted \
         runner (display covered, Dock and menu bar hidden, timeout and a synthetic Esc hold end it), not with a real keyboard \
         or on a real Mac; fan control, charge limit, low-latency networking and Touch ID for sudo are not implemented",
    ),
    ("resident.login_item", CapabilityState::Unsupported, LOGIN_ITEM_REASON),
];

// ---------------------------------------------------------------------------------------------
// Keep the screen on (RES-02)

/// `kIOPMAssertPreventUserIdleDisplaySleep` (`IOKit/pwr_mgt/IOPMLib.h`).
pub const ASSERTION_TYPE: &str = "PreventUserIdleDisplaySleep";
/// The name shown in `pmset -g assertions` and Activity Monitor.
pub const ASSERTION_NAME: &str = "CleanManager: keep the screen on";
/// `kIOPMAssertionLevelOn`
const ASSERTION_LEVEL_ON: u32 = 255;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    /// `IOReturn IOPMAssertionCreateWithName(CFStringRef AssertionType, IOPMAssertionLevel AssertionLevel, CFStringRef AssertionName, IOPMAssertionID *AssertionID);`
    fn IOPMAssertionCreateWithName(assertion_type: CFTypeRef, level: u32, name: CFTypeRef, id: *mut u32) -> i32;
    /// `IOReturn IOPMAssertionRelease(IOPMAssertionID AssertionID);`
    fn IOPMAssertionRelease(id: u32) -> i32;
    /// `IOReturn IOPMCopyAssertionsByProcess(CFDictionaryRef *AssertionsByPID);` (+1 out)
    fn IOPMCopyAssertionsByProcess(by_pid: *mut CFTypeRef) -> i32;
}

/// One power assertion as powerd lists it for a process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertionInfo {
    /// `AssertType` (for example `PreventUserIdleDisplaySleep`; some types are listed under legacy names).
    pub assertion_type: String,
    /// `AssertName`
    pub name: String,
}

/// The power assertions powerd currently holds for `pid` (empty when it holds none).
pub fn process_assertions(pid: i32) -> Result<Vec<AssertionInfo>, String> {
    let mut out: CFTypeRef = ptr::null();
    // SAFETY: valid out-pointer; on success it receives a +1 dictionary (or NULL).
    let status = unsafe { IOPMCopyAssertionsByProcess(&mut out) };
    if status != 0 {
        return Err(format!("IOPMCopyAssertionsByProcess failed (IOReturn {status:#x})"));
    }
    // SAFETY: Create rule.
    let Some(by_pid) = (unsafe { CfRef::from_create(out) }) else { return Ok(Vec::new()) };
    if by_pid.type_id() != cf::CFDictionaryGetTypeID() {
        return Err("IOPMCopyAssertionsByProcess returned something other than a dictionary".into());
    }
    // The dictionary is keyed by CFNumber pids; CFNumber equality and hashing compare values, not storage types.
    let key = cf::from_plist(&Plist::Integer(i64::from(pid)))?;
    // SAFETY: live dictionary and key; the value is borrowed from `by_pid` and converted before it drops.
    let list = unsafe { cf::CFDictionaryGetValue(by_pid.as_ptr(), key.as_ptr()) };
    if list.is_null() {
        return Ok(Vec::new());
    }
    // SAFETY: live CF object owned by `by_pid`.
    let Plist::Array(items) = (unsafe { cf::to_plist(list) })? else { return Err("the assertions of a process are not an array".into()) };
    Ok(items
        .iter()
        .map(|a| AssertionInfo {
            assertion_type: a.get("AssertType").and_then(Plist::as_str).unwrap_or_default().to_owned(),
            name: a.get("AssertName").and_then(Plist::as_str).unwrap_or_default().to_owned(),
        })
        .collect())
}

/// An active "keep the screen on" assertion. Released by [`KeepAwake::release`], when its duration ends, on drop, or
/// by powerd when the process exits.
pub struct KeepAwake {
    id: u32,
    name: String,
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
    until: Option<Instant>,
}

impl KeepAwake {
    pub fn start(duration: Option<Duration>) -> Result<KeepAwake, String> {
        Self::start_named(ASSERTION_NAME, duration)
    }

    /// [`KeepAwake::start`] with another assertion name (tests use a unique one to find their own assertion).
    pub fn start_named(name: &str, duration: Option<Duration>) -> Result<KeepAwake, String> {
        let ty = cf::string(ASSERTION_TYPE).ok_or("cannot create the assertion type string")?;
        let cf_name = cf::string(name).ok_or("cannot create the assertion name string")?;
        let mut id = 0u32;
        // SAFETY: live CFStrings and a valid out-pointer; the strings are not consumed.
        let status = unsafe { IOPMAssertionCreateWithName(ty.as_ptr(), ASSERTION_LEVEL_ON, cf_name.as_ptr(), &mut id) };
        if status != 0 {
            return Err(format!("IOPMAssertionCreateWithName refused the request (IOReturn {status:#x})"));
        }
        let (stop_tx, stop_rx) = channel::<()>();
        let spawned = std::thread::Builder::new().name("cm-keep-awake".into()).spawn(move || {
            let _ = match duration {
                Some(d) => stop_rx.recv_timeout(d).map_err(|_| ()),
                None => stop_rx.recv().map_err(|_| ()),
            };
            // SAFETY: the assertion this thread owns, released exactly once (here).
            unsafe { IOPMAssertionRelease(id) };
        });
        match spawned {
            Ok(thread) => Ok(KeepAwake { id, name: name.to_owned(), stop: Some(stop_tx), thread: Some(thread), until: duration.map(|d| Instant::now() + d) }),
            Err(e) => {
                // SAFETY: no thread took ownership; released once here.
                unsafe { IOPMAssertionRelease(id) };
                Err(format!("cannot start the keep-awake timer: {e}"))
            }
        }
    }

    /// Still holding the assertion (false once the duration ended).
    pub fn is_active(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    pub fn remaining(&self) -> Option<Duration> {
        self.until.map(|u| u.saturating_duration_since(Instant::now()))
    }

    /// The assertion as `pmset -g assertions` would show it.
    pub fn describe(&self) -> String {
        format!("IOPMAssertion {ASSERTION_TYPE} #{} named \"{}\"", self.id, self.name)
    }

    /// Release the assertion and wait until it is released.
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
// CoreAudio / CoreMediaIO hardware objects (read-only property gets)

const fn fourcc(c: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*c)
}

/// `kAudioObjectSystemObject` / `kCMIOObjectSystemObject`
const SYSTEM_OBJECT: u32 = 1;
/// `kAudioObjectPropertyScopeGlobal` / `kCMIOObjectPropertyScopeGlobal`
const SCOPE_GLOBAL: u32 = fourcc(b"glob");
/// `kAudioObjectPropertyScopeInput`
const SCOPE_INPUT: u32 = fourcc(b"inpt");
/// `kAudioObjectPropertyScopeOutput`
const SCOPE_OUTPUT: u32 = fourcc(b"outp");
/// `kAudioObjectPropertyElementMain` / `kCMIOObjectPropertyElementMain`
const ELEMENT_MAIN: u32 = 0;
/// `kAudioHardwarePropertyDevices` / `kCMIOHardwarePropertyDevices`
const PROP_DEVICES: u32 = fourcc(b"dev#");
/// `kAudioObjectPropertyName` / `kCMIOObjectPropertyName` (CFStringRef, +1)
const PROP_NAME: u32 = fourcc(b"lnam");
/// `kAudioDevicePropertyStreams`
const PROP_STREAMS: u32 = fourcc(b"stm#");
/// `kAudioDevicePropertyDeviceIsRunningSomewhere` / `kCMIODevicePropertyDeviceIsRunningSomewhere` (UInt32)
const PROP_RUNNING_SOMEWHERE: u32 = fourcc(b"gone");
/// `kAudioHardwarePropertyProcessObjectList` (macOS 14)
const PROP_PROCESS_LIST: u32 = fourcc(b"prs#");
/// `kAudioProcessPropertyPID` (pid_t)
const PROP_PROCESS_PID: u32 = fourcc(b"ppid");
/// `kAudioProcessPropertyBundleID` (CFStringRef, +1)
const PROP_PROCESS_BUNDLE_ID: u32 = fourcc(b"pbid");
/// `kAudioProcessPropertyIsRunningInput` (UInt32)
const PROP_PROCESS_RUNNING_INPUT: u32 = fourcc(b"piri");
/// Upper bound on objects taken from one list (defends against a runaway HAL).
const MAX_OBJECTS: usize = 512;

/// `AudioObjectPropertyAddress` / `CMIOObjectPropertyAddress`: three `UInt32`s.
#[repr(C)]
struct PropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    /// `OSStatus AudioObjectGetPropertyDataSize(AudioObjectID inObjectID, const AudioObjectPropertyAddress *inAddress, UInt32 inQualifierDataSize, const void *inQualifierData, UInt32 *outDataSize);`
    fn AudioObjectGetPropertyDataSize(object: u32, address: *const PropertyAddress, qualifier_size: u32, qualifier: *const c_void, size: *mut u32) -> i32;
    /// `OSStatus AudioObjectGetPropertyData(AudioObjectID inObjectID, const AudioObjectPropertyAddress *inAddress, UInt32 inQualifierDataSize, const void *inQualifierData, UInt32 *ioDataSize, void *outData);`
    fn AudioObjectGetPropertyData(object: u32, address: *const PropertyAddress, qualifier_size: u32, qualifier: *const c_void, io_size: *mut u32, data: *mut c_void) -> i32;
}

#[link(name = "CoreMediaIO", kind = "framework")]
unsafe extern "C" {
    /// `OSStatus CMIOObjectGetPropertyDataSize(CMIOObjectID objectID, const CMIOObjectPropertyAddress *address, UInt32 qualifierDataSize, const void *qualifierData, UInt32 *dataSize);`
    fn CMIOObjectGetPropertyDataSize(object: u32, address: *const PropertyAddress, qualifier_size: u32, qualifier: *const c_void, size: *mut u32) -> i32;
    /// `OSStatus CMIOObjectGetPropertyData(CMIOObjectID objectID, const CMIOObjectPropertyAddress *address, UInt32 qualifierDataSize, const void *qualifierData, UInt32 dataSize, UInt32 *dataUsed, void *data);`
    fn CMIOObjectGetPropertyData(object: u32, address: *const PropertyAddress, qualifier_size: u32, qualifier: *const c_void, size: u32, used: *mut u32, data: *mut c_void) -> i32;
}

/// `OSStatus` as text: the four-character code when it is one (`'who?'`), otherwise the number.
pub(crate) fn status_text(status: i32) -> String {
    let b = status.to_be_bytes();
    if b.iter().all(|c| (0x20..0x7f).contains(c)) { format!("OSStatus '{}' ({status})", String::from_utf8_lossy(&b)) } else { format!("OSStatus {status}") }
}

/// Which hardware abstraction layer a property get goes to.
#[derive(Debug, Clone, Copy)]
enum Hal {
    Audio,
    MediaIo,
}

impl Hal {
    fn size(self, object: u32, selector: u32, scope: u32) -> Result<u32, i32> {
        let address = PropertyAddress { selector, scope, element: ELEMENT_MAIN };
        let mut size = 0u32;
        // SAFETY: valid address, no qualifier, valid out-pointer.
        let status = unsafe {
            match self {
                Hal::Audio => AudioObjectGetPropertyDataSize(object, &address, 0, ptr::null(), &mut size),
                Hal::MediaIo => CMIOObjectGetPropertyDataSize(object, &address, 0, ptr::null(), &mut size),
            }
        };
        if status == 0 { Ok(size) } else { Err(status) }
    }

    /// Read the property into `out` (`capacity` bytes); returns the bytes written.
    ///
    /// # Safety
    /// `out` must be valid for writes of `capacity` bytes and suitably aligned for the property's type.
    unsafe fn read(self, object: u32, selector: u32, scope: u32, out: *mut c_void, capacity: u32) -> Result<u32, i32> {
        let address = PropertyAddress { selector, scope, element: ELEMENT_MAIN };
        let mut used = capacity;
        // SAFETY: valid address, no qualifier; `out` holds `capacity` bytes per the caller.
        let status = unsafe {
            match self {
                Hal::Audio => AudioObjectGetPropertyData(object, &address, 0, ptr::null(), &mut used, out),
                Hal::MediaIo => CMIOObjectGetPropertyData(object, &address, 0, ptr::null(), capacity, &mut used, out),
            }
        };
        if status == 0 { Ok(used.min(capacity)) } else { Err(status) }
    }

    /// An array of object IDs (`UInt32`).
    fn ids(self, object: u32, selector: u32) -> Result<Vec<u32>, i32> {
        let n = (self.size(object, selector, SCOPE_GLOBAL)? as usize / 4).min(MAX_OBJECTS);
        if n == 0 {
            return Ok(Vec::new());
        }
        let mut ids = vec![0u32; n];
        // SAFETY: `ids` holds `n` UInt32s.
        let used = unsafe { self.read(object, selector, SCOPE_GLOBAL, ids.as_mut_ptr().cast(), (n * 4) as u32) }?;
        ids.truncate(used as usize / 4);
        Ok(ids)
    }

    fn u32_of(self, object: u32, selector: u32) -> Result<u32, i32> {
        let mut v = 0u32;
        // SAFETY: one UInt32.
        let used = unsafe { self.read(object, selector, SCOPE_GLOBAL, (&mut v as *mut u32).cast(), 4) }?;
        if used == 4 { Ok(v) } else { Err(fourcc(b"!siz") as i32) }
    }

    /// A CFString property (returned +1 by the HAL); `None` when absent or unreadable.
    fn string_of(self, object: u32, selector: u32) -> Option<String> {
        let mut s: CFTypeRef = ptr::null();
        // SAFETY: one pointer-sized slot for the CFStringRef.
        let used = unsafe { self.read(object, selector, SCOPE_GLOBAL, (&mut s as *mut CFTypeRef).cast(), size_of::<CFTypeRef>() as u32) }.ok()?;
        // SAFETY: on success the HAL hands over a +1 CFString (or NULL); released by the wrapper.
        let s = unsafe { CfRef::from_create(s) }?;
        if used as usize != size_of::<CFTypeRef>() || s.type_id() != cf::CFStringGetTypeID() {
            return None;
        }
        // SAFETY: live CFString.
        unsafe { cf::string_value(s.as_ptr()) }.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty())
    }

    /// Whether the device has streams in `scope` (input or output).
    fn has_streams(self, object: u32, scope: u32) -> bool {
        self.size(object, PROP_STREAMS, scope).is_ok_and(|s| s > 0)
    }
}

// ---------------------------------------------------------------------------------------------
// Privacy indicator (RES-03)

/// Shown in place of an app name when the system says a device is in use but no public API says by whom.
pub const UNNAMED_APP: &str = "未知应用（macOS 只报告设备正在使用，不提供应用名称）";
const SOURCE: &str = "CoreMediaIO kCMIODevicePropertyDeviceIsRunningSomewhere (cameras); CoreAudio kAudioDevicePropertyDeviceIsRunningSomewhere \
     (input devices) and, on macOS 14 and later, kAudioProcessPropertyIsRunningInput of CoreAudio process objects (microphone app names)";

/// A camera or input device and whether it is running somewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceUse {
    pub name: String,
    /// `Err` with the reason when the property could not be read.
    pub running: Result<bool, String>,
    /// An audio device that also has output streams: "running" may mean playback only.
    pub also_output: bool,
}

/// A CoreAudio process object (macOS 14 and later).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioProcess {
    pub pid: i32,
    pub bundle_id: Option<String>,
    pub running_input: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UseState {
    InUse,
    NotInUse,
    /// The state could not be determined, with the reason.
    Unknown(String),
}

/// What is known about one capability (camera or microphone).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityUse {
    pub state: UseState,
    pub devices: Vec<DeviceUse>,
    /// Apps a public API names as using it (microphone on macOS 14 and later); empty otherwise.
    pub apps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyReading {
    pub camera: CapabilityUse,
    pub microphone: CapabilityUse,
}

/// Every camera CoreMediaIO knows and whether it is running somewhere.
pub fn cameras() -> Result<Vec<DeviceUse>, String> {
    let hal = Hal::MediaIo;
    let ids = hal.ids(SYSTEM_OBJECT, PROP_DEVICES).map_err(|s| format!("the camera list (kCMIOHardwarePropertyDevices) cannot be read: {}", status_text(s)))?;
    Ok(ids
        .into_iter()
        .map(|id| DeviceUse {
            name: hal.string_of(id, PROP_NAME).unwrap_or_else(|| format!("camera {id}")),
            running: hal.u32_of(id, PROP_RUNNING_SOMEWHERE).map(|v| v != 0).map_err(|s| format!("kCMIODevicePropertyDeviceIsRunningSomewhere: {}", status_text(s))),
            also_output: false,
        })
        .collect())
}

/// Every CoreAudio device with input streams and whether it is running somewhere.
pub fn input_devices() -> Result<Vec<DeviceUse>, String> {
    let hal = Hal::Audio;
    let ids = hal.ids(SYSTEM_OBJECT, PROP_DEVICES).map_err(|s| format!("the audio device list (kAudioHardwarePropertyDevices) cannot be read: {}", status_text(s)))?;
    Ok(ids
        .into_iter()
        .filter(|&id| hal.has_streams(id, SCOPE_INPUT))
        .map(|id| DeviceUse {
            name: hal.string_of(id, PROP_NAME).unwrap_or_else(|| format!("audio device {id}")),
            running: hal.u32_of(id, PROP_RUNNING_SOMEWHERE).map(|v| v != 0).map_err(|s| format!("kAudioDevicePropertyDeviceIsRunningSomewhere: {}", status_text(s))),
            also_output: hal.has_streams(id, SCOPE_OUTPUT),
        })
        .collect())
}

/// CoreAudio's process objects with their input state. `Err` before macOS 14 (the properties do not exist there).
pub fn audio_processes() -> Result<Vec<AudioProcess>, String> {
    let hal = Hal::Audio;
    let ids = hal.ids(SYSTEM_OBJECT, PROP_PROCESS_LIST).map_err(|s| format!("CoreAudio process objects (kAudioHardwarePropertyProcessObjectList, macOS 14) cannot be read: {}", status_text(s)))?;
    ids.into_iter()
        .map(|id| {
            let running_input = hal.u32_of(id, PROP_PROCESS_RUNNING_INPUT).map_err(|s| format!("kAudioProcessPropertyIsRunningInput cannot be read: {}", status_text(s)))? != 0;
            let pid = hal.u32_of(id, PROP_PROCESS_PID).map_err(|s| format!("kAudioProcessPropertyPID cannot be read: {}", status_text(s)))? as i32;
            Ok(AudioProcess { pid, bundle_id: hal.string_of(id, PROP_PROCESS_BUNDLE_ID), running_input })
        })
        .collect()
}

/// A display name for a process: its executable name (the `.app` name when it runs from a bundle), else its bundle ID.
fn process_name(p: &AudioProcess) -> String {
    let from_bundle_path = procs::exe_path(p.pid).and_then(|path| {
        let text = String::from_utf8_lossy(&path).into_owned();
        let app = text.split('/').find(|c| c.ends_with(".app"))?.trim_end_matches(".app").to_owned();
        (!app.is_empty()).then_some(app)
    });
    from_bundle_path
        .or_else(|| procs::process(p.pid).ok().map(|i| i.name).filter(|n| !n.is_empty()))
        .or_else(|| p.bundle_id.clone())
        .unwrap_or_else(|| format!("pid {}", p.pid))
}

/// Camera state from the device list: in use when any camera runs; unknown when a camera's state cannot be read.
pub fn camera_use(devices: Result<Vec<DeviceUse>, String>) -> CapabilityUse {
    let devices = match devices {
        Ok(d) => d,
        Err(e) => return CapabilityUse { state: UseState::Unknown(e), devices: Vec::new(), apps: Vec::new() },
    };
    let state = if devices.iter().any(|d| d.running == Ok(true)) {
        UseState::InUse
    } else if let Some((d, e)) = devices.iter().find_map(|d| d.running.as_ref().err().map(|e| (d, e))) {
        UseState::Unknown(format!("{}: {e}", d.name))
    } else {
        UseState::NotInUse
    };
    CapabilityUse { state, devices, apps: Vec::new() }
}

/// Microphone state from the input devices and, when available, CoreAudio's process objects.
///
/// - An input-only device running somewhere, or a process reading input, means in use.
/// - A device with both input and output running somewhere may only be playing sound: the process objects decide; without
///   them the state is unknown.
/// - A device whose state cannot be read makes the state unknown unless the process objects answer for every app.
pub fn microphone_use(devices: Result<Vec<DeviceUse>, String>, processes: Result<Vec<AudioProcess>, String>, name_of: impl Fn(&AudioProcess) -> String) -> CapabilityUse {
    let devices = match devices {
        Ok(d) => d,
        Err(e) => return CapabilityUse { state: UseState::Unknown(e), devices: Vec::new(), apps: Vec::new() },
    };
    let mut apps: Vec<String> = processes.as_ref().map(|ps| ps.iter().filter(|p| p.running_input).map(&name_of).collect()).unwrap_or_default();
    apps.sort_by_key(|a| a.to_lowercase());
    apps.dedup();
    let input_only_running = devices.iter().any(|d| d.running == Ok(true) && !d.also_output);
    let shared_running: Vec<&str> = devices.iter().filter(|d| d.running == Ok(true) && d.also_output).map(|d| d.name.as_str()).collect();
    let unreadable = devices.iter().find_map(|d| d.running.as_ref().err().map(|e| format!("{}: {e}", d.name)));
    let state = if input_only_running || !apps.is_empty() {
        UseState::InUse
    } else if let Err(why) = &processes {
        if !shared_running.is_empty() {
            UseState::Unknown(format!("{} is running, but it also plays sound and this macOS does not say whether its input is in use ({why})", shared_running.join(", ")))
        } else if let Some(e) = unreadable {
            UseState::Unknown(e)
        } else {
            UseState::NotInUse
        }
    } else {
        // The process objects answer for every app: none is reading input.
        UseState::NotInUse
    };
    CapabilityUse { state, devices, apps }
}

/// Read the camera and microphone state now.
pub fn privacy_reading() -> PrivacyReading {
    PrivacyReading { camera: camera_use(cameras()), microphone: microphone_use(input_devices(), audio_processes(), process_name) }
}

fn state_text(c: &CapabilityUse) -> String {
    match &c.state {
        UseState::InUse if c.apps.is_empty() => "in use (app not named by the system)".into(),
        UseState::InUse => format!("in use by {}", c.apps.join(", ")),
        UseState::NotInUse => "not in use".into(),
        UseState::Unknown(why) => format!("unknown: {why}"),
    }
}

/// The platform-neutral snapshot. Unknown on either side makes the indicator unavailable with both states in the reason
/// (it never says "not in use" for something it could not read).
pub fn snapshot_of(r: &PrivacyReading) -> PrivacySnapshot {
    let unknown = matches!(r.camera.state, UseState::Unknown(_)) || matches!(r.microphone.state, UseState::Unknown(_));
    if unknown {
        return PrivacySnapshot {
            supported: false,
            in_use: Vec::new(),
            unavailable_reason: Some(format!("camera: {}; microphone: {}", state_text(&r.camera), state_text(&r.microphone))),
            source: SOURCE,
        };
    }
    let mut in_use: Vec<InUseApp> = r.microphone.apps.iter().map(|a| InUseApp { name: a.clone(), capabilities: vec![Capability::Microphone], desktop_app: true }).collect();
    let mut unnamed = Vec::new();
    if r.camera.state == UseState::InUse {
        unnamed.push(Capability::Camera);
    }
    if r.microphone.state == UseState::InUse && r.microphone.apps.is_empty() {
        unnamed.push(Capability::Microphone);
    }
    if !unnamed.is_empty() {
        in_use.push(InUseApp { name: UNNAMED_APP.into(), capabilities: unnamed, desktop_app: false });
    }
    PrivacySnapshot { supported: true, in_use, unavailable_reason: None, source: SOURCE }
}

pub fn privacy_snapshot() -> PrivacySnapshot {
    snapshot_of(&privacy_reading())
}

// ---------------------------------------------------------------------------------------------
// Batteries (RES-04)

fn int_of(p: &Plist, key: &str) -> Option<i64> {
    match p.get(key)? {
        Plist::Integer(n) => Some(*n),
        _ => None,
    }
}

/// This Mac's internal battery from the IOPS snapshot; `None` without one (desktop Macs, virtual machines).
pub fn system_battery() -> Option<DeviceBattery> {
    let (_, sources) = iokit::power_sources()?;
    system_battery_from(&sources)
}

/// [`system_battery`] over the IOPS source descriptions (`kIOPSTypeKey`, `kIOPSCurrentCapacityKey`, `kIOPSMaxCapacityKey`,
/// `kIOPSIsChargingKey`, `kIOPSPowerSourceStateKey`, `kIOPSIsPresentKey`).
pub(crate) fn system_battery_from(sources: &[Plist]) -> Option<DeviceBattery> {
    let b = sources.iter().find(|s| s.get("Type").and_then(Plist::as_str) == Some("InternalBattery"))?;
    if b.get("Is Present") == Some(&Plist::Bool(false)) {
        return None;
    }
    let (current, max) = (int_of(b, "Current Capacity")?, int_of(b, "Max Capacity")?);
    if max <= 0 || current < 0 {
        return None;
    }
    let percent = (current as f64 * 100.0 / max as f64).round().min(100.0) as u8;
    let charging = match (b.get("Is Charging"), b.get("Power Source State").and_then(Plist::as_str)) {
        (Some(Plist::Bool(true)), _) | (_, Some("AC Power")) => Some(true),
        (_, Some("Battery Power")) => Some(false),
        _ => None,
    };
    Some(DeviceBattery { device_id: "system".into(), name: "本机电池".into(), percent, kind: "system", charging })
}

/// Registry classes of Apple's HID peripheral drivers that publish `BatteryPercent` (current Magic devices use the first;
/// the others are the pre-Monterey Bluetooth HID drivers).
const BATTERY_CLASSES: [&CStr; 4] = [c"AppleDeviceManagementHIDEventService", c"AppleBluetoothHIDKeyboard", c"BNBMouseDevice", c"BNBTrackpadDevice"];
/// Why a Bluetooth HID device is listed without a level.
pub const NO_LEVEL_REASON: &str = "the device publishes no BatteryPercent in the I/O Registry; its level is not estimated";

/// The registry properties read from one HID entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HidEntry {
    pub registry_id: u64,
    /// `Product`
    pub product: Option<String>,
    /// `Transport` (`Bluetooth`, `BluetoothLowEnergy`, `USB`, …)
    pub transport: Option<String>,
    /// `DeviceAddress`
    pub address: Option<String>,
    /// `SerialNumber`
    pub serial: Option<String>,
    /// The entry has a `BatteryPercent` property at all.
    pub has_battery_key: bool,
    /// `BatteryPercent` when it is an integer.
    pub battery: Option<i64>,
}

fn hid_entry(s: &Service) -> HidEntry {
    let text = |k: &str| {
        s.property(k).and_then(Result::ok).and_then(|p| match p {
            Plist::String(x) => Some(x.trim().to_owned()).filter(|x| !x.is_empty()),
            _ => None,
        })
    };
    let battery = s.property("BatteryPercent");
    HidEntry {
        registry_id: s.registry_id().unwrap_or(0),
        product: text("Product"),
        transport: text("Transport"),
        address: text("DeviceAddress"),
        serial: text("SerialNumber"),
        has_battery_key: battery.is_some(),
        battery: battery.and_then(Result::ok).and_then(|p| if let Plist::Integer(n) = p { Some(n) } else { None }),
    }
}

fn is_bluetooth(transport: Option<&str>) -> bool {
    transport.is_some_and(|t| t.to_ascii_lowercase().contains("bluetooth"))
}

/// A stable id for alerts: the Bluetooth address, else the serial number, else the registry entry.
fn identity(e: &HidEntry) -> String {
    if let Some(a) = &e.address {
        return format!("bt:{}", a.to_ascii_lowercase().replace([':', '-'], ""));
    }
    if let Some(s) = &e.serial {
        return format!("sn:{s}");
    }
    format!("ioreg:{:#x}", e.registry_id)
}

/// A peripheral the system lists without a usable level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnavailableDevice {
    pub device_id: String,
    pub name: String,
    /// `bluetooth` or `peripheral`
    pub kind: &'static str,
    pub reason: String,
}

/// Result of reading peripheral batteries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceScan {
    pub devices: Vec<DeviceBattery>,
    /// Bluetooth devices seen (with or without a level).
    pub bluetooth_devices: usize,
    /// Devices without a usable level (never shown as 0%).
    pub unavailable: Vec<UnavailableDevice>,
}

/// Build the scan from the entries that carry `BatteryPercent` and the Bluetooth HID devices.
pub fn scan_from(batteries: &[HidEntry], bluetooth: &[HidEntry]) -> DeviceScan {
    let mut scan = DeviceScan::default();
    let mut seen = BTreeSet::new();
    for e in batteries.iter().filter(|e| e.has_battery_key) {
        let id = identity(e);
        if !seen.insert(id.clone()) {
            continue;
        }
        let name = e.product.clone().unwrap_or_else(|| "未命名设备".into());
        let kind = if is_bluetooth(e.transport.as_deref()) { "bluetooth" } else { "peripheral" };
        match e.battery {
            Some(p) if (0..=100).contains(&p) => scan.devices.push(DeviceBattery { device_id: id, name, percent: p as u8, kind, charging: None }),
            Some(p) => scan.unavailable.push(UnavailableDevice { device_id: id, name, kind, reason: format!("BatteryPercent {p} is outside 0–100; not shown") }),
            None => scan.unavailable.push(UnavailableDevice { device_id: id, name, kind, reason: "BatteryPercent is not a number; not shown".into() }),
        }
    }
    let named: BTreeSet<&str> = batteries.iter().filter(|e| e.has_battery_key).filter_map(|e| e.product.as_deref()).collect();
    let mut unnamed_seen = BTreeSet::new();
    for e in bluetooth.iter().filter(|e| is_bluetooth(e.transport.as_deref())) {
        let id = identity(e);
        // The same device seen through its battery entry (by address/serial, or by product name when the battery entry
        // carries no address).
        if seen.contains(&id) || e.product.as_deref().is_some_and(|p| named.contains(p)) {
            continue;
        }
        let name = e.product.clone().unwrap_or_else(|| "未命名蓝牙设备".into());
        // Several HID nodes of one device without an address or serial: list it once by name.
        if id.starts_with("ioreg:") && !unnamed_seen.insert(name.clone()) {
            continue;
        }
        if !seen.insert(id.clone()) {
            continue;
        }
        scan.unavailable.push(UnavailableDevice { device_id: id, name, kind: "bluetooth", reason: NO_LEVEL_REASON.into() });
    }
    scan.devices.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.device_id.cmp(&b.device_id)));
    scan.unavailable.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.device_id.cmp(&b.device_id)));
    scan.bluetooth_devices = scan.devices.iter().filter(|d| d.kind == "bluetooth").count() + scan.unavailable.iter().filter(|d| d.kind == "bluetooth").count();
    scan
}

/// Peripheral batteries from the I/O Registry. Read-only; the registry walk itself cannot fail (an unreadable class is
/// simply empty), so the result is always `Ok` and mirrors the Windows signature.
pub fn device_batteries() -> Result<DeviceScan, String> {
    let mut batteries = Vec::new();
    for class in BATTERY_CLASSES {
        batteries.extend(Service::all_matching(class).iter().map(hid_entry).filter(|e| e.has_battery_key));
    }
    let bluetooth: Vec<HidEntry> = Service::all_matching(c"IOHIDDevice").iter().map(hid_entry).filter(|e| is_bluetooth(e.transport.as_deref())).collect();
    Ok(scan_from(&batteries, &bluetooth))
}

// ---------------------------------------------------------------------------------------------
// Start at login (RES-01/07): unsupported in this build

/// Where a login item would be registered (shown on the page whether or not it is available).
pub const LOGIN_ITEM_LOCATION: &str = "SMAppService.mainApp (login item of the signed CleanManager.app; unavailable in this unsigned build)";

/// The login-item store on macOS: nothing exists, nothing can be created, removing is a no-op.
pub struct LoginItem;

impl StartupStore for LoginItem {
    fn read(&self) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn write(&self, _: &str) -> Result<(), String> {
        Err(LOGIN_ITEM_REASON.into())
    }
    fn remove(&self) -> Result<bool, String> {
        Ok(false)
    }
}

// ---------------------------------------------------------------------------------------------
// Clean-screen presentation options (RES-02)

/// `NSApplicationPresentationHideDock`
pub const PRESENTATION_HIDE_DOCK: usize = 1 << 1;
/// `NSApplicationPresentationHideMenuBar` (requires `HideDock`)
pub const PRESENTATION_HIDE_MENU_BAR: usize = 1 << 3;
/// `NSApplicationPresentationDisableProcessSwitching` (Command-Tab; requires `HideDock`)
pub const PRESENTATION_DISABLE_PROCESS_SWITCHING: usize = 1 << 5;
/// `NSApplicationPresentationDisableHideApplication` (Command-H)
pub const PRESENTATION_DISABLE_HIDE_APPLICATION: usize = 1 << 8;
/// The options used while clean-screen mode is on. Force Quit and session termination are deliberately left enabled.
pub const CLEAN_SCREEN_PRESENTATION: usize = PRESENTATION_HIDE_DOCK | PRESENTATION_HIDE_MENU_BAR | PRESENTATION_DISABLE_PROCESS_SWITCHING | PRESENTATION_DISABLE_HIDE_APPLICATION;

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    /// `void objc_msgSend(void);` (`objc/message.h`): cast to the method's exact type before calling.
    fn objc_msgSend();
}

fn shared_application() -> Result<Id, String> {
    // SAFETY: no arguments.
    if unsafe { libc::pthread_main_np() } != 1 {
        return Err("presentation options can only be changed on the main thread".into());
    }
    let cls = objc::class(c"NSApplication").ok_or("AppKit's NSApplication class is not loaded")?;
    // SAFETY: `+ (NSApplication *)sharedApplication;` has exactly this C type.
    let f = unsafe { std::mem::transmute::<unsafe extern "C" fn(), unsafe extern "C" fn(Id, Sel) -> Id>(objc_msgSend as unsafe extern "C" fn()) };
    // SAFETY: class receiver, main thread (checked above).
    let app = unsafe { f(cls, objc::sel(c"sharedApplication")) };
    if app.is_null() { Err("NSApplication.sharedApplication is nil".into()) } else { Ok(app) }
}

fn presentation_options(app: Id) -> usize {
    // SAFETY: `@property NSApplicationPresentationOptions presentationOptions;` getter type.
    let f = unsafe { std::mem::transmute::<unsafe extern "C" fn(), unsafe extern "C" fn(Id, Sel) -> usize>(objc_msgSend as unsafe extern "C" fn()) };
    // SAFETY: live shared application on the main thread.
    unsafe { f(app, objc::sel(c"presentationOptions")) }
}

fn set_presentation_options(app: Id, options: usize) {
    // SAFETY: `presentationOptions` setter type (NSUInteger argument).
    let f = unsafe { std::mem::transmute::<unsafe extern "C" fn(), unsafe extern "C" fn(Id, Sel, usize)>(objc_msgSend as unsafe extern "C" fn()) };
    // SAFETY: live shared application on the main thread; callers pass only combinations AppKit accepts (Default, or
    // HideDock with HideMenuBar / DisableProcessSwitching), so it raises no exception.
    unsafe { f(app, objc::sel(c"setPresentationOptions:"), options) }
}

/// Main thread only. Hide the Dock and menu bar and disable app switching and hiding while this app is active. Returns
/// `Ok(false)` without changing anything when the app already uses other presentation options (for example a window in
/// full screen), so [`end_clean_presentation`] never clobbers them.
pub fn begin_clean_presentation() -> Result<bool, String> {
    let app = shared_application()?;
    if presentation_options(app) != 0 {
        return Ok(false);
    }
    set_presentation_options(app, CLEAN_SCREEN_PRESENTATION);
    Ok(true)
}

/// Main thread only. Back to the default presentation (only when clean-screen mode set it).
pub fn end_clean_presentation() -> Result<(), String> {
    let app = shared_application()?;
    if presentation_options(app) == CLEAN_SCREEN_PRESENTATION {
        set_presentation_options(app, 0);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
