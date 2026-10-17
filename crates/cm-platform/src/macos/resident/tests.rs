//! Resident utilities on macOS. The keep-awake test creates and releases a real power assertion, so it runs only with
//! `CM_ALLOW_MAC_DEVICE_TESTS=1` or on a hosted runner (`GITHUB_ACTIONS=true`) (macos.md §11). The camera, microphone
//! and battery queries are read-only and always run; on a hosted runner (no camera, battery or Bluetooth device) they
//! must answer "not in use" / "none" or say explicitly why they cannot.

use std::time::{Duration, Instant};

use cm_core::capability::CapabilityState;
use cm_core::resident::{Capability, ResidentSettings, StartupStore, sync_startup};

use super::*;

fn device_tests_allowed() -> bool {
    std::env::var("CM_ALLOW_MAC_DEVICE_TESTS").is_ok_and(|v| v == "1") || on_hosted_runner()
}

fn on_hosted_runner() -> bool {
    std::env::var("GITHUB_ACTIONS").is_ok_and(|v| v == "true")
}

fn ours(name: &str) -> Vec<AssertionInfo> {
    process_assertions(std::process::id() as i32).expect("IOPMCopyAssertionsByProcess").into_iter().filter(|a| a.name == name).collect()
}

/// Poll until `f` holds (powerd may apply a create or release a moment later).
fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn keep_awake_assertion_is_listed_while_held_and_gone_after_release() {
    if !device_tests_allowed() {
        eprintln!("skipped: set CM_ALLOW_MAC_DEVICE_TESTS=1 to create a keep-awake power assertion (released at the end)");
        return;
    }
    let name = format!("CleanManager test keep-awake {}-{:?}", std::process::id(), Instant::now());
    let mut k = KeepAwake::start_named(&name, None).expect("assertion created");
    assert!(k.is_active() && k.remaining().is_none());
    assert!(k.describe().contains(ASSERTION_TYPE) && k.describe().contains(&name), "{}", k.describe());
    eventually("the assertion is listed for this process", || !ours(&name).is_empty());
    let listed = ours(&name);
    eprintln!("listed: {listed:?}");
    assert!(listed.iter().all(|a| a.assertion_type == ASSERTION_TYPE || a.assertion_type == "NoDisplaySleepAssertion"), "{listed:?}");
    k.release();
    assert!(!k.is_active());
    eventually("the assertion is gone after release", || ours(&name).is_empty());

    // A timed request ends by itself; dropping one releases it too.
    let timed = format!("{name} timed");
    let t = KeepAwake::start_named(&timed, Some(Duration::from_millis(300))).expect("timed assertion");
    assert!(t.remaining().is_some_and(|r| r <= Duration::from_millis(300)));
    eventually("the timed request ends", || !t.is_active());
    eventually("the timed assertion is gone", || ours(&timed).is_empty());
    let dropped = format!("{name} dropped");
    let d = KeepAwake::start_named(&dropped, None).expect("assertion");
    eventually("the assertion is listed", || !ours(&dropped).is_empty());
    drop(d);
    eventually("the dropped assertion is gone", || ours(&dropped).is_empty());
}

#[test]
fn assertions_of_a_process_without_any_are_empty() {
    // No process has this pid.
    let none = process_assertions(i32::MAX).expect("readable");
    assert!(none.is_empty(), "{none:?}");
}

#[test]
fn camera_and_microphone_answer_definitely_or_explain_why_not() {
    let cams = cameras();
    let mics = input_devices();
    let procs = audio_processes();
    eprintln!("cameras={cams:?}\ninput devices={mics:?}\naudio processes={procs:?}");
    let r = privacy_reading();
    eprintln!("reading={r:?}");
    for c in [&r.camera, &r.microphone] {
        if let UseState::Unknown(why) = &c.state {
            assert!(!why.trim().is_empty(), "an unknown state carries its reason");
        }
    }
    let s = snapshot_of(&r);
    let any_unknown = matches!(r.camera.state, UseState::Unknown(_)) || matches!(r.microphone.state, UseState::Unknown(_));
    assert_eq!(s.supported, !any_unknown);
    assert_eq!(s.unavailable_reason.is_some(), any_unknown);
    assert!(!s.source.is_empty());
    if on_hosted_runner() {
        // macOS 14 runner without a camera; its virtual audio devices are idle and CoreAudio's process objects exist.
        assert!(procs.is_ok(), "CoreAudio process objects are readable on macOS 14: {procs:?}");
        assert_eq!(r.camera.state, UseState::NotInUse, "{r:?}");
        assert_eq!(r.microphone.state, UseState::NotInUse, "{r:?}");
        assert!(s.supported && s.in_use.is_empty(), "{s:?}");
    }
}

fn dev(name: &str, running: Result<bool, &str>, also_output: bool) -> DeviceUse {
    DeviceUse { name: name.into(), running: running.map_err(str::to_owned), also_output }
}

fn proc_(pid: i32, input: bool) -> AudioProcess {
    AudioProcess { pid, bundle_id: Some(format!("com.example.p{pid}")), running_input: input }
}

fn named(p: &AudioProcess) -> String {
    format!("app{}", p.pid)
}

#[test]
fn camera_state_is_in_use_not_in_use_or_unknown_with_reason() {
    assert_eq!(camera_use(Ok(vec![])).state, UseState::NotInUse, "no camera: definitely not in use");
    assert_eq!(camera_use(Ok(vec![dev("FaceTime HD Camera", Ok(false), false)])).state, UseState::NotInUse);
    assert_eq!(camera_use(Ok(vec![dev("FaceTime HD Camera", Ok(true), false), dev("OBS", Err("x"), false)])).state, UseState::InUse);
    assert_eq!(camera_use(Ok(vec![dev("Cam", Err("'who?'"), false)])).state, UseState::Unknown("Cam: 'who?'".into()));
    assert_eq!(camera_use(Err("list failed".into())).state, UseState::Unknown("list failed".into()));
    assert!(camera_use(Ok(vec![dev("Cam", Ok(true), false)])).apps.is_empty(), "no public API names the camera's user");
}

#[test]
fn microphone_state_uses_devices_and_names_apps_only_from_process_objects() {
    let mic = |r| dev("MacBook Pro Microphone", r, false);
    let headset = |r| dev("USB Headset", r, true);
    // Idle.
    assert_eq!(microphone_use(Ok(vec![mic(Ok(false))]), Ok(vec![proc_(1, false)]), named).state, UseState::NotInUse);
    assert_eq!(microphone_use(Ok(vec![]), Err("old macOS".into()), named).state, UseState::NotInUse, "no input device");
    // An input-only device running: in use, named when the process objects say who.
    let m = microphone_use(Ok(vec![mic(Ok(true))]), Ok(vec![proc_(7, true), proc_(8, false)]), named);
    assert_eq!((m.state, m.apps), (UseState::InUse, vec!["app7".to_owned()]));
    let m = microphone_use(Ok(vec![mic(Ok(true))]), Err("old macOS".into()), named);
    assert_eq!((m.state, m.apps.len()), (UseState::InUse, 0));
    // A headset that also plays sound: the process objects decide; without them it is unknown.
    assert_eq!(microphone_use(Ok(vec![headset(Ok(true))]), Ok(vec![proc_(3, false)]), named).state, UseState::NotInUse);
    assert_eq!(microphone_use(Ok(vec![headset(Ok(true))]), Ok(vec![proc_(3, true)]), named).state, UseState::InUse);
    let u = microphone_use(Ok(vec![headset(Ok(true))]), Err("no process objects".into()), named);
    assert!(matches!(&u.state, UseState::Unknown(w) if w.contains("USB Headset") && w.contains("no process objects")), "{u:?}");
    // An unreadable device: unknown unless the process objects answer.
    assert!(matches!(microphone_use(Ok(vec![mic(Err("'!obj'"))]), Err("old".into()), named).state, UseState::Unknown(_)));
    assert_eq!(microphone_use(Ok(vec![mic(Err("'!obj'"))]), Ok(vec![]), named).state, UseState::NotInUse);
    assert!(matches!(microphone_use(Err("list failed".into()), Ok(vec![]), named).state, UseState::Unknown(_)));
    // Names are sorted and listed once.
    let m = microphone_use(Ok(vec![mic(Ok(true))]), Ok(vec![proc_(9, true), proc_(2, true), proc_(9, true)]), named);
    assert_eq!(m.apps, vec!["app2".to_owned(), "app9".to_owned()]);
}

fn reading(camera: UseState, mic: UseState, apps: &[&str]) -> PrivacyReading {
    PrivacyReading {
        camera: CapabilityUse { state: camera, devices: vec![], apps: vec![] },
        microphone: CapabilityUse { state: mic, devices: vec![], apps: apps.iter().map(|a| (*a).to_owned()).collect() },
    }
}

#[test]
fn snapshot_never_names_an_app_the_system_did_not_name() {
    let idle = snapshot_of(&reading(UseState::NotInUse, UseState::NotInUse, &[]));
    assert!(idle.supported && idle.in_use.is_empty() && idle.unavailable_reason.is_none());
    let cam = snapshot_of(&reading(UseState::InUse, UseState::NotInUse, &[]));
    assert_eq!(cam.in_use.len(), 1);
    assert_eq!((cam.in_use[0].name.as_str(), cam.in_use[0].capabilities.as_slice()), (UNNAMED_APP, &[Capability::Camera][..]));
    let both = snapshot_of(&reading(UseState::InUse, UseState::InUse, &["zoom.us"]));
    let names: Vec<(&str, &[Capability])> = both.in_use.iter().map(|a| (a.name.as_str(), a.capabilities.as_slice())).collect();
    assert_eq!(names, vec![("zoom.us", &[Capability::Microphone][..]), (UNNAMED_APP, &[Capability::Camera][..])]);
    let unnamed_mic = snapshot_of(&reading(UseState::InUse, UseState::InUse, &[]));
    assert_eq!(unnamed_mic.in_use.len(), 1);
    assert_eq!(unnamed_mic.in_use[0].capabilities, vec![Capability::Camera, Capability::Microphone]);
    // Unknown on one side: unavailable, with both states in the reason (never "not in use").
    let unknown = snapshot_of(&reading(UseState::Unknown("kCMIO… 'who?'".into()), UseState::InUse, &["Teams"]));
    assert!(!unknown.supported && unknown.in_use.is_empty());
    let why = unknown.unavailable_reason.unwrap();
    assert!(why.contains("camera: unknown: kCMIO… 'who?'") && why.contains("microphone: in use by Teams"), "{why}");
}

#[test]
fn os_status_codes_read_as_four_character_codes() {
    assert_eq!(status_text(fourcc(b"who?") as i32), format!("OSStatus 'who?' ({})", fourcc(b"who?") as i32));
    assert_eq!(status_text(-50), "OSStatus -50");
}

fn ps(pairs: &[(&str, Plist)]) -> Plist {
    Plist::Dict(pairs.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect())
}

#[test]
fn internal_battery_comes_from_the_power_source_snapshot() {
    let batt = |cur: i64, charging: bool, state: &str| {
        ps(&[
            ("Type", Plist::String("InternalBattery".into())),
            ("Current Capacity", Plist::Integer(cur)),
            ("Max Capacity", Plist::Integer(100)),
            ("Is Charging", Plist::Bool(charging)),
            ("Power Source State", Plist::String(state.into())),
            ("Is Present", Plist::Bool(true)),
        ])
    };
    let b = system_battery_from(&[batt(42, false, "Battery Power")]).unwrap();
    assert_eq!((b.device_id.as_str(), b.percent, b.kind, b.charging), ("system", 42, "system", Some(false)));
    assert_eq!(system_battery_from(&[batt(80, false, "AC Power")]).unwrap().charging, Some(true));
    assert_eq!(system_battery_from(&[batt(80, true, "Battery Power")]).unwrap().charging, Some(true));
    let ups = ps(&[("Type", Plist::String("UPS".into())), ("Current Capacity", Plist::Integer(50)), ("Max Capacity", Plist::Integer(100))]);
    assert_eq!(system_battery_from(&[ups]), None, "only the internal battery");
    let absent = ps(&[("Type", Plist::String("InternalBattery".into())), ("Is Present", Plist::Bool(false)), ("Current Capacity", Plist::Integer(0)), ("Max Capacity", Plist::Integer(100))]);
    assert_eq!(system_battery_from(&[absent]), None, "a removed battery is not 0%");
    let no_max = ps(&[("Type", Plist::String("InternalBattery".into())), ("Current Capacity", Plist::Integer(50))]);
    assert_eq!(system_battery_from(&[no_max]), None);
}

fn hid(id: u64, product: &str, transport: &str, address: Option<&str>, battery: Option<Option<i64>>) -> HidEntry {
    HidEntry {
        registry_id: id,
        product: Some(product.into()),
        transport: Some(transport.into()),
        address: address.map(str::to_owned),
        serial: None,
        has_battery_key: battery.is_some(),
        battery: battery.flatten(),
    }
}

#[test]
fn peripheral_levels_come_only_from_battery_percent_and_others_are_unavailable() {
    let batteries = [
        hid(10, "Magic Keyboard", "Bluetooth", Some("AA-BB-CC-DD-EE-FF"), Some(Some(77))),
        hid(11, "Magic Mouse", "BluetoothLowEnergy", None, Some(Some(5))),
        hid(12, "Magic Trackpad", "USB", Some("11:22:33:44:55:66"), Some(Some(100))),
        hid(13, "Odd Device", "Bluetooth", Some("99-99-99-99-99-99"), Some(Some(140))),
        hid(14, "Magic Keyboard", "Bluetooth", Some("aa:bb:cc:dd:ee:ff"), Some(Some(77))),
    ];
    let bluetooth = [
        hid(20, "Magic Keyboard", "Bluetooth", Some("aa-bb-cc-dd-ee-ff"), None),
        hid(21, "Magic Mouse", "BluetoothLowEnergy", Some("01-02-03-04-05-06"), None),
        hid(22, "Game Controller", "Bluetooth", Some("0A-0B-0C-0D-0E-0F"), None),
        hid(23, "Pen", "Bluetooth Low Energy", None, None),
        hid(24, "Pen", "Bluetooth Low Energy", None, None),
        hid(25, "Built-in Keyboard", "SPI", None, None),
    ];
    let scan = scan_from(&batteries, &bluetooth);
    let levels: Vec<(&str, u8, &str)> = scan.devices.iter().map(|d| (d.name.as_str(), d.percent, d.kind)).collect();
    assert_eq!(levels, vec![("Magic Keyboard", 77, "bluetooth"), ("Magic Mouse", 5, "bluetooth"), ("Magic Trackpad", 100, "peripheral")]);
    assert!(scan.devices.iter().all(|d| d.charging.is_none()));
    assert_eq!(scan.devices[0].device_id, "bt:aabbccddeeff", "one id per address however it is written");
    let missing: Vec<(&str, &str)> = scan.unavailable.iter().map(|u| (u.name.as_str(), u.kind)).collect();
    assert_eq!(missing, vec![("Game Controller", "bluetooth"), ("Odd Device", "bluetooth"), ("Pen", "bluetooth")]);
    assert!(scan.unavailable.iter().any(|u| u.name == "Game Controller" && u.reason == NO_LEVEL_REASON));
    assert!(scan.unavailable.iter().any(|u| u.name == "Odd Device" && u.reason.contains("140")));
    assert_eq!(scan.bluetooth_devices, 5);
    assert_eq!(scan_from(&[], &[]), DeviceScan::default());
}

#[test]
fn batteries_on_this_machine_never_report_a_made_up_level() {
    let system = system_battery();
    let scan = device_batteries().expect("registry walk");
    eprintln!("system={system:?} scan={scan:?}");
    assert!(scan.devices.iter().all(|d| d.percent <= 100));
    if on_hosted_runner() {
        // The hosted runner is a virtual machine: no internal battery, no Bluetooth peripherals.
        assert_eq!(system, None);
        assert!(scan.devices.is_empty() && scan.unavailable.is_empty() && scan.bluetooth_devices == 0, "{scan:?}");
    }
}

#[test]
fn start_at_login_is_unsupported_and_writes_nothing() {
    let store = LoginItem;
    assert_eq!(store.read(), Ok(None));
    assert_eq!(store.write("anything"), Err(LOGIN_ITEM_REASON.to_owned()));
    assert_eq!(store.remove(), Ok(false));
    let on = ResidentSettings { tray: true, start_at_login: true, ..Default::default() };
    assert!(sync_startup(&store, &on, "/Applications/CleanManager.app").unwrap_err().contains("SMAppService.mainApp"));
    let tray_only = ResidentSettings { tray: true, ..Default::default() };
    assert!(sync_startup(&store, &tray_only, "x").is_ok(), "turning start-at-login off always succeeds");
}

#[test]
fn presentation_options_refuse_a_background_thread() {
    // The test harness runs tests off the main thread: nothing is sent to AppKit.
    assert_eq!(begin_clean_presentation(), Err("presentation options can only be changed on the main thread".into()));
    assert!(end_clean_presentation().is_err());
    assert_eq!(CLEAN_SCREEN_PRESENTATION & PRESENTATION_HIDE_DOCK, PRESENTATION_HIDE_DOCK, "HideMenuBar and DisableProcessSwitching require HideDock");
}

#[test]
fn capability_rows_say_unverified_and_login_item_unsupported() {
    let row = |n: &str| CAPS.iter().find(|(name, _, _)| *name == n).map(|&(_, s, r)| (s, r));
    let (state, reason) = row("resident.utilities").unwrap();
    assert_eq!(state, CapabilityState::Unverified);
    assert!(reason.contains("not on a real Mac"), "{reason}");
    assert_eq!(row("resident.login_item").map(|r| r.0), Some(CapabilityState::Unsupported));
    let caps = cm_core::capability::CapabilityProvider::capabilities(&crate::host());
    let gated = caps.iter().find(|c| c.name == "resident.utilities").unwrap();
    assert_eq!(gated.state, CapabilityState::Unverified);
}
