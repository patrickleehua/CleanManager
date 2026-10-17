//! Process list and the `process_end` action on macOS (parity 12.3, STS-04).
//!
//! Listing is read-only: the libproc snapshot (`procs::snapshot`: pid, parent, owner, start time, state, executable)
//! plus `proc_pid_rusage(RUSAGE_INFO_V2)` per process for CPU time, physical footprint (Activity Monitor's "Memory")
//! and disk bytes; rates come from two readings `window` apart. Without administrator rights the kernel withholds
//! another user's usage, so those values stay unknown with a note, never 0. Per-process network traffic needs the
//! private NetworkStatistics framework and is never sampled.
//!
//! Publisher is the signature's Team ID as the static signature claims it (`ffi::security`); Apple's own code is
//! reported as Apple only when it satisfies `anchor apple`. Purpose text comes from the local knowledge table
//! ([`KNOWN`]) only when the name, the location and the signer agree.
//!
//! Ending a process re-identifies it by pid + start time (and executable path + SHA-256) right before `kill(SIGTERM)`;
//! macOS has no process handle, so the remaining window between that check and the signal is a few system calls.
//! Never offered, and refused again at execution: pid 0/1, launchd, kernel_task, WindowServer, loginwindow,
//! CleanManager itself, security software, and any process of another user.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use cm_core::capability::CapabilityState;
use cm_core::path::RawPath;
use cm_core::plan::v2::ProcessEndAction;
use cm_core::process::{EndOutcome, ProcessActions, ProcessFacts, ProcessView, Protection, Signature, SignatureState};

use super::ffi::security;
use super::procs::{self, ProcInfo};

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[(
    "process.end",
    CapabilityState::Unverified,
    "Process list (libproc, proc_pid_rusage, Team ID) and SIGTERM after pid + start-time re-identification are implemented and tested on hosted runners only; not verified on a real Mac",
)];

/// Per-process disk figure on macOS.
pub const DISK_NOTE: &str = "disk_bytes_per_sec is the process's storage reads and writes (proc_pid_rusage disk I/O), not network or cache traffic";
/// Why per-process network is unknown.
pub const NETWORK_REASON: &str = "Per-process network traffic needs Apple's private NetworkStatistics interface; it is not sampled";
/// Note on other users' processes.
pub const OTHER_USER_NOTE: &str = "Another user's or a system process: macOS withholds its CPU, memory and disk usage without administrator rights";

/// Names that are always critical on macOS.
pub const CRITICAL_NAMES: &[&str] = &["launchd", "kernel_task", "WindowServer", "loginwindow"];
/// CleanManager executables.
pub const SELF_NAMES: &[&str] = &["cleanmanager", "cleanmanager-desktop", "cm-desktop", "CleanManager"];

/// Who must have signed an entry of [`KNOWN`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signer {
    /// Satisfies `anchor apple`.
    Apple,
    /// A Developer ID Team ID.
    Team(&'static str),
    /// No signer requirement.
    Any,
}

/// Where an entry of [`KNOWN`] must run from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    /// The sealed system volume (`/System`, `/usr` outside `/usr/local`, `/sbin`, `/bin`).
    System,
    /// Inside this directory (an app bundle).
    Under(&'static str),
    Any,
}

/// One entry of the macOS process knowledge table.
pub struct Known {
    pub name: &'static str,
    pub location: Location,
    pub signer: Signer,
    pub purpose: &'static str,
}

const fn k(name: &'static str, location: Location, signer: Signer, purpose: &'static str) -> Known {
    Known { name, location, signer, purpose }
}

const SYS: Location = Location::System;
const APPLE: Signer = Signer::Apple;

/// Local process knowledge (macOS). Purpose text is shown only when name, location and signer all agree.
pub const KNOWN: &[Known] = &[
    k("launchd", SYS, APPLE, "System and session manager: starts, supervises and restarts every other process"),
    k("WindowServer", SYS, APPLE, "Draws every window, the menu bar and the cursor on screen"),
    k("loginwindow", SYS, APPLE, "Login session: login and lock screen, logging out and restarting apps at login"),
    k("Finder", SYS, APPLE, "Finder: desktop, file windows and file operations"),
    k("Dock", SYS, APPLE, "Dock, Mission Control, Launchpad and the desktop picture"),
    k("SystemUIServer", SYS, APPLE, "Menu bar extras (status items)"),
    k("ControlCenter", SYS, APPLE, "Control Center and the menu bar's system modules"),
    k("NotificationCenter", SYS, APPLE, "Notification Center and notification banners"),
    k("WindowManager", SYS, APPLE, "Stage Manager and window tiling"),
    k("Spotlight", SYS, APPLE, "Spotlight search interface"),
    k("mds", SYS, APPLE, "Spotlight metadata server: keeps the search index"),
    k("mds_stores", SYS, APPLE, "Spotlight index storage: writes the per-volume search index"),
    k("mdworker", SYS, APPLE, "Spotlight importer: reads files to index their contents"),
    k("mdworker_shared", SYS, APPLE, "Spotlight importer: reads files to index their contents"),
    k("corespotlightd", SYS, APPLE, "Indexes app content (mail, messages, notes) for Spotlight"),
    k("kernelmanagerd", SYS, APPLE, "Loads and manages kernel and system extensions"),
    k("logd", SYS, APPLE, "Unified logging daemon: collects the system log"),
    k("syslogd", SYS, APPLE, "Legacy system log daemon"),
    k("configd", SYS, APPLE, "System configuration: network settings and computer name"),
    k("notifyd", SYS, APPLE, "System notification bus between processes"),
    k("distnoted", SYS, APPLE, "Distributed notifications between apps"),
    k("cfprefsd", SYS, APPLE, "Preferences daemon: reads and writes app and system settings"),
    k("UserEventAgent", SYS, APPLE, "Runs plug-ins that react to system events (network, power, devices)"),
    k("coreservicesd", SYS, APPLE, "Core Services: file types, app registration and Launch Services"),
    k("launchservicesd", SYS, APPLE, "Launch Services: which app opens which document"),
    k("opendirectoryd", SYS, APPLE, "Directory services: user accounts and authentication lookups"),
    k("securityd", SYS, APPLE, "Keychain and code-signing security services"),
    k("trustd", SYS, APPLE, "Certificate trust evaluation"),
    k("powerd", SYS, APPLE, "Power management: sleep, wake and battery"),
    k("coreaudiod", SYS, APPLE, "Core Audio: sound input and output for every app"),
    k("bluetoothd", SYS, APPLE, "Bluetooth"),
    k("airportd", SYS, APPLE, "Wi-Fi"),
    k("mDNSResponder", SYS, APPLE, "DNS resolution and Bonjour discovery"),
    k("fseventsd", SYS, APPLE, "File-system change journal used by Time Machine, Spotlight and apps"),
    k("hidd", SYS, APPLE, "Keyboard, mouse and trackpad input"),
    k("backupd", SYS, APPLE, "Time Machine backups"),
    k("cloudd", SYS, APPLE, "iCloud sync service"),
    k("bird", SYS, APPLE, "iCloud Drive file sync"),
    k("nsurlsessiond", SYS, APPLE, "Background downloads and uploads on behalf of apps"),
    k("softwareupdated", SYS, APPLE, "Software Update: checks for and installs macOS updates"),
    k("photoanalysisd", SYS, APPLE, "Photos library analysis (faces, scenes, memories)"),
    k("mediaanalysisd", SYS, APPLE, "On-device image and video analysis"),
    k("sharingd", SYS, APPLE, "AirDrop, Handoff and sharing services"),
    k("rapportd", SYS, APPLE, "Device-to-device communication for Continuity"),
    k("locationd", SYS, APPLE, "Location services"),
    k("symptomsd", SYS, APPLE, "Network diagnostics and usage statistics"),
    k("apsd", SYS, APPLE, "Apple Push Notification service connection"),
    k("Terminal", SYS, APPLE, "Terminal"),
    k("Activity Monitor", SYS, APPLE, "Activity Monitor"),
    k("Safari", SYS, APPLE, "Safari web browser"),
    k("Google Chrome", Location::Under("/Applications/Google Chrome.app/"), Signer::Team("EQHXZ8M8AV"), "Google Chrome web browser"),
    k("Google Chrome Helper", Location::Under("/Applications/Google Chrome.app/"), Signer::Team("EQHXZ8M8AV"), "Google Chrome helper (tab, extension or GPU process)"),
    k("firefox", Location::Under("/Applications/Firefox.app/"), Signer::Team("43AQ936H96"), "Mozilla Firefox web browser"),
    k("Code Helper", Location::Under("/Applications/Visual Studio Code.app/"), Signer::Team("UBF8T346G9"), "Visual Studio Code helper process"),
    k("Electron", Location::Under("/Applications/Visual Studio Code.app/"), Signer::Team("UBF8T346G9"), "Visual Studio Code editor"),
    k("cleanmanager", Location::Any, Signer::Any, "CleanManager command line"),
];

/// On the sealed system volume (or an Apple cryptex mounted below `/System`).
fn on_system_volume(path: &str) -> bool {
    ["/System/", "/usr/libexec/", "/usr/sbin/", "/usr/bin/", "/usr/lib/", "/sbin/", "/bin/"].iter().any(|p| path.starts_with(p))
}

/// One process as listed on macOS: the shared facts plus what macOS-only checks need.
#[derive(Debug, Clone, PartialEq)]
pub struct MacProcess {
    pub facts: ProcessFacts,
    /// Effective user.
    pub uid: u32,
    pub zombie: bool,
    /// Leaf certificate subject of the signature, as claimed (e.g. `Developer ID Application: Name (TEAMID)`).
    pub authority: Option<String>,
    /// Satisfies `anchor apple`.
    pub apple: bool,
}

/// Purpose from [`KNOWN`] when name, location and signer agree; otherwise why not.
pub fn identify(p: &MacProcess) -> Result<&'static str, String> {
    let f = &p.facts;
    let Some(entry) = KNOWN.iter().find(|e| e.name == f.name) else {
        return Err("Not in CleanManager's local process knowledge".into());
    };
    let path = f.image_path.as_ref().map(RawPath::display);
    let at = |ok: bool, p: &str| if ok { Ok(()) } else { Err(format!("Named like a known process but runs from an unexpected location: {p}")) };
    match (entry.location, path.as_deref()) {
        (Location::Any, _) => {}
        (_, None) => return Err("Named like a known process but its location could not be read".into()),
        (Location::System, Some(p)) => at(on_system_volume(p), p)?,
        (Location::Under(dir), Some(p)) => at(p.starts_with(dir), p)?,
    }
    match entry.signer {
        Signer::Any => {}
        Signer::Apple if p.apple => {}
        Signer::Team(team) if f.signature.publisher.as_deref() == Some(team) && f.signature.state == SignatureState::Signed => {}
        _ if f.signature.state == SignatureState::Unknown => return Err("Named like a known process but its signature could not be checked".into()),
        _ => return Err(format!("Named like a known process but not signed by its vendor (signer: {})", f.signature.publisher.as_deref().unwrap_or("none"))),
    }
    Ok(entry.purpose)
}

/// Protection decision (macOS). `own_uid` is the caller's effective user.
pub fn protection(p: &MacProcess, own_pid: u32, own_uid: u32) -> Option<Protection> {
    let f = &p.facts;
    if f.pid == own_pid || SELF_NAMES.contains(&f.name.as_str()) {
        return Some(Protection::CleanManager);
    }
    if f.pid <= 1 || CRITICAL_NAMES.contains(&f.name.as_str()) {
        return Some(Protection::CriticalSystem);
    }
    if p.uid != own_uid {
        return Some(Protection::OtherUser);
    }
    let claimed = p.authority.as_deref().or(f.signature.publisher.as_deref());
    if cm_core::apps::protected_reason(&f.name, claimed, false) == Some(cm_core::apps::ProtectedReason::EndpointSecurityOrManagement) {
        return Some(Protection::SecuritySoftware);
    }
    None
}

/// The app bundle an executable belongs to: the outermost `*.app` directory on its path.
pub fn owner_app(path: &str) -> Option<String> {
    path.split('/').find_map(|c| c.strip_suffix(".app").filter(|n| !n.is_empty()).map(str::to_owned))
}

/// Explain listed processes: purpose, owner app, parent name, protection and pins.
pub fn explain(procs: Vec<MacProcess>, own_pid: u32, own_uid: u32, pins: &[String]) -> Vec<ProcessView> {
    let names: HashMap<u32, String> = procs.iter().map(|p| (p.facts.pid, p.facts.name.clone())).collect();
    procs
        .into_iter()
        .map(|p| {
            let (purpose, unidentified_reason) = match identify(&p) {
                Ok(x) => (Some(x), None),
                Err(why) => (None, Some(why)),
            };
            let protection = protection(&p, own_pid, own_uid);
            let f = p.facts;
            ProcessView {
                parent_name: f.parent_pid.and_then(|pp| names.get(&pp).cloned()),
                owner_app: f.image_path.as_ref().and_then(|i| owner_app(&i.display())),
                protection,
                pinned: pins.iter().any(|x| x.eq_ignore_ascii_case(&f.name)),
                purpose,
                unidentified_reason,
                facts: f,
            }
        })
        .collect()
}

/// Mach absolute-time units → nanoseconds (`ri_user_time`/`ri_system_time` are in these units on Apple silicon).
fn timebase() -> (u64, u64) {
    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        /// `kern_return_t mach_timebase_info(mach_timebase_info_t info);` (`<mach/mach_time.h>`)
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }
    static TB: OnceLock<(u64, u64)> = OnceLock::new();
    *TB.get_or_init(|| {
        let mut tb = Timebase { numer: 0, denom: 0 };
        // SAFETY: valid out-pointer.
        let ok = unsafe { mach_timebase_info(&mut tb) } == 0 && tb.numer != 0 && tb.denom != 0;
        if ok { (u64::from(tb.numer), u64::from(tb.denom)) } else { (1, 1) }
    })
}

/// Resource usage of one process at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Usage {
    /// User + system CPU time, nanoseconds.
    pub cpu_ns: u64,
    /// Physical footprint (Activity Monitor's "Memory"), bytes.
    pub footprint: u64,
    /// Storage bytes read + written since the process started.
    pub disk_bytes: u64,
}

/// `proc_pid_rusage(RUSAGE_INFO_V2)`. `Err(EPERM)` for another user's process without privileges, `Err(ESRCH)` when gone.
pub(crate) fn usage(pid: i32) -> Result<Usage, i32> {
    // SAFETY: zeroed POD out-parameter of the declared type.
    let mut ri: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    // SAFETY: the buffer is a rusage_info_v2, the flavor asked for; the kernel writes exactly that struct.
    let rc = unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V2, (&mut ri as *mut libc::rusage_info_v2).cast()) };
    if rc != 0 {
        return Err(super::sys::errno());
    }
    let (numer, denom) = timebase();
    let ticks = u128::from(ri.ri_user_time) + u128::from(ri.ri_system_time);
    let cpu_ns = u64::try_from(ticks * u128::from(numer) / u128::from(denom)).unwrap_or(u64::MAX);
    Ok(Usage { cpu_ns, footprint: ri.ri_phys_footprint, disk_bytes: ri.ri_diskio_bytesread.saturating_add(ri.ri_diskio_byteswritten) })
}

/// The caller's effective user.
pub fn own_uid() -> u32 {
    // SAFETY: plain getter.
    unsafe { libc::geteuid() }
}

/// Start time in the `started_at` wire form: microseconds since the Unix epoch (decimal).
pub(crate) fn start_key(p: &ProcInfo) -> Option<String> {
    p.start.map(|(s, us)| s.saturating_mul(1_000_000).saturating_add(us).to_string())
}

fn raw(path: &[u8]) -> Option<RawPath> {
    RawPath::unix(path.to_vec()).ok()
}

/// Signature facts of an executable: (shared signature, leaf authority, Apple anchor).
pub fn signature_of(path: &[u8]) -> (Signature, Option<String>, bool) {
    match security::signature(Path::new(OsStr::from_bytes(path))) {
        Err(e) => (Signature::unknown(format!("the code signature could not be read: {e}")), None, false),
        Ok(s) if !s.signed => (Signature { state: SignatureState::Unsigned, publisher: None, reason: Some("no code signature".into()) }, None, false),
        Ok(s) if s.adhoc => (Signature { state: SignatureState::Unsigned, publisher: None, reason: Some("ad-hoc signature: no signing identity".into()) }, None, false),
        Ok(s) if s.apple => (
            Signature { state: SignatureState::Signed, publisher: Some("Apple".into()), reason: Some("satisfies `anchor apple` (code directory signature and certificate chain checked)".into()) },
            s.authorities.first().cloned(),
            true,
        ),
        Ok(s) => {
            let leaf = s.authorities.first().cloned();
            let reason = format!(
                "Team ID as claimed by the static code signature{}; page hashes are not verified",
                leaf.as_deref().map(|l| format!(" ({l})")).unwrap_or_default()
            );
            (Signature { state: SignatureState::Signed, publisher: s.team_id.clone().or_else(|| leaf.clone()), reason: Some(reason) }, leaf, false)
        }
    }
}

fn facts_of(p: &ProcInfo, now: Option<&Usage>, before: Option<&Usage>, secs: f64, ncpu: f64, verify: bool, sigs: &mut HashMap<Vec<u8>, (Signature, Option<String>, bool)>) -> MacProcess {
    let mut notes = Vec::new();
    let (signature, authority, apple) = match (&p.path, verify) {
        (Some(path), true) => sigs.entry(path.clone()).or_insert_with(|| signature_of(path)).clone(),
        (Some(_), false) => (Signature::unknown("not checked in this view"), None, false),
        (None, _) => (Signature::unknown("the executable path could not be read"), None, false),
    };
    if p.zombie {
        notes.push("Exited and waiting for its parent to collect its status (zombie)".to_owned());
    } else if now.is_none() {
        notes.push(OTHER_USER_NOTE.to_owned());
    }
    if p.path.is_none() && !p.zombie {
        notes.push("The executable path is withheld or unavailable".to_owned());
    }
    let rate = |f: fn(&Usage) -> u64| -> Option<f64> {
        let (a, b) = (before?, now?);
        (secs > 0.0).then(|| f(b).saturating_sub(f(a)) as f64 / secs)
    };
    let cpu_percent = rate(|u| u.cpu_ns).map(|ns_per_s| ((ns_per_s / 1e9 / ncpu * 100.0).min(100.0) * 10.0).round() / 10.0);
    MacProcess {
        facts: ProcessFacts {
            pid: p.pid as u32,
            parent_pid: u32::try_from(p.ppid).ok(),
            name: p.name.clone(),
            image_path: p.path.as_deref().and_then(raw),
            started_at: start_key(p),
            cpu_percent,
            memory_bytes: now.map(|u| u.footprint),
            disk_bytes_per_sec: rate(|u| u.disk_bytes),
            network_bytes_per_sec: None,
            signature,
            os_critical: None,
            notes,
        },
        uid: p.uid,
        zombie: p.zombie,
        authority,
        apple,
    }
}

/// Every process with CPU, memory and disk rates over `window`. `verify` also reads each executable's signature.
pub fn list_with(window: Duration, verify: bool) -> Result<Vec<MacProcess>, String> {
    let first = procs::snapshot().map_err(|e| format!("the process list could not be read: {}", super::sys::describe(e)))?;
    let key = |p: &ProcInfo| (p.pid, p.start);
    let before: HashMap<(i32, Option<(u64, u64)>), Usage> = first.procs.iter().filter_map(|p| usage(p.pid).ok().map(|u| (key(p), u))).collect();
    let t0 = Instant::now();
    std::thread::sleep(window);
    let snap = procs::snapshot().map_err(|e| format!("the process list could not be read: {}", super::sys::describe(e)))?;
    let secs = t0.elapsed().as_secs_f64();
    let ncpu = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
    let mut sigs = HashMap::new();
    Ok(snap
        .procs
        .iter()
        .map(|p| {
            let now = if p.zombie { None } else { usage(p.pid).ok() };
            // Only the same instance (pid + start) gives a rate; a reused pid has none for this window.
            let prev = before.get(&key(p)).filter(|_| p.start.is_some());
            facts_of(p, now.as_ref(), prev, secs, ncpu, verify, &mut sigs)
        })
        .collect())
}

/// The explained process list.
pub fn views(window: Duration, verify: bool, pins: &[String]) -> Result<Vec<ProcessView>, String> {
    Ok(explain(list_with(window, verify)?, std::process::id(), own_uid(), pins))
}

/// One explained process (signature checked, no rates), or `None` when no process `pid` runs.
pub fn view(pid: u32) -> Result<Option<ProcessView>, String> {
    let snap = procs::snapshot().map_err(|e| format!("the process list could not be read: {}", super::sys::describe(e)))?;
    let mut sigs = HashMap::new();
    let all: Vec<MacProcess> = snap
        .procs
        .iter()
        .map(|p| {
            let u = if p.pid as u32 == pid && !p.zombie { usage(p.pid).ok() } else { None };
            facts_of(p, u.as_ref(), None, 0.0, 1.0, p.pid as u32 == pid, &mut sigs)
        })
        .collect();
    Ok(explain(all, std::process::id(), own_uid(), &[]).into_iter().find(|v| v.facts.pid == pid))
}

/// SHA-256 of an executable (hex), when readable.
pub fn file_sha256(path: &RawPath) -> Option<String> {
    let bytes = path.raw_bytes();
    std::fs::File::open(Path::new(OsStr::from_bytes(&bytes))).ok().and_then(|f| cm_core::adapter::sha256_hex(f).ok())
}

/// Live process facts for re-identification (no usage, signature only when `verify`).
fn live(pid: i32, verify: bool) -> Result<MacProcess, i32> {
    let p = procs::process(pid)?;
    let mut sigs = HashMap::new();
    Ok(facts_of(&p, None, None, 0.0, 1.0, verify, &mut sigs))
}

/// How long `end` waits for the process to exit after SIGTERM.
const EXIT_WAIT: Duration = Duration::from_secs(5);

/// The planned instance has exited: gone, a zombie, or its pid now names another process.
fn exited(a: &ProcessEndAction) -> Option<bool> {
    match procs::process(a.pid as i32) {
        Err(libc::ESRCH) => Some(true),
        Err(_) => None,
        Ok(p) if p.zombie => Some(true),
        Ok(p) => start_key(&p).map(|s| s != a.started_at),
    }
}

/// The macOS `process_end` backend.
pub struct MacProcesses;

impl ProcessActions for MacProcesses {
    fn end(&self, a: &ProcessEndAction, own_pid: u32) -> EndOutcome {
        if a.pid == own_pid {
            return EndOutcome::Protected(Protection::CleanManager);
        }
        if a.pid <= 1 || CRITICAL_NAMES.contains(&a.name.as_str()) {
            return EndOutcome::Protected(Protection::CriticalSystem);
        }
        let Ok(pid) = i32::try_from(a.pid) else { return EndOutcome::AlreadyExited };
        let p = match live(pid, true) {
            Ok(p) => p,
            Err(libc::ESRCH) => return EndOutcome::AlreadyExited,
            Err(e) => return EndOutcome::Failed(format!("the process could not be inspected: {}", super::sys::describe(e))),
        };
        if p.zombie {
            return EndOutcome::AlreadyExited;
        }
        if let Some(pr) = protection(&p, own_pid, own_uid()) {
            return EndOutcome::Protected(pr);
        }
        if p.facts.started_at.as_deref() != Some(a.started_at.as_str()) {
            return EndOutcome::Changed("the PID now belongs to a different process; the planned process already exited".into());
        }
        if a.image_path.as_ref().map(RawPath::raw_bytes) != p.facts.image_path.as_ref().map(RawPath::raw_bytes) {
            return EndOutcome::Changed("the process's executable path differs from the reviewed plan".into());
        }
        if let (Some(expected), Some(image)) = (&a.image_sha256, &p.facts.image_path)
            && file_sha256(image).as_deref() != Some(expected.as_str())
        {
            return EndOutcome::Changed("the executable changed since the plan was reviewed".into());
        }
        // Last re-identification immediately before the signal (no process handle exists on macOS).
        match procs::process(pid) {
            Ok(q) if !q.zombie && start_key(&q).as_deref() == Some(a.started_at.as_str()) => {}
            Ok(q) if q.zombie => return EndOutcome::AlreadyExited,
            Ok(_) => return EndOutcome::Changed("the PID now belongs to a different process; the planned process already exited".into()),
            Err(_) => return EndOutcome::AlreadyExited,
        }
        // SAFETY: plain syscall on a pid re-identified just above.
        if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
            return match super::sys::errno() {
                libc::ESRCH => EndOutcome::AlreadyExited,
                libc::EPERM => EndOutcome::PermissionRequired("macOS refused to signal the process (it runs as another user or is protected)".into()),
                e => EndOutcome::Failed(format!("kill(SIGTERM) failed: {}", super::sys::describe(e))),
            };
        }
        let deadline = Instant::now() + EXIT_WAIT;
        loop {
            if exited(a) == Some(true) {
                return EndOutcome::Ended;
            }
            if Instant::now() >= deadline {
                return EndOutcome::Unknown("SIGTERM was sent but the process had not exited after 5 s (it may be saving work or ignoring the request)".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn running(&self, a: &ProcessEndAction) -> Option<bool> {
        exited(a).map(|gone| !gone)
    }
}

#[cfg(test)]
mod tests;
