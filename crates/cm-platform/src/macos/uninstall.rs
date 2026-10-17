//! App uninstall on macOS (parity 9.1 macOS half, UNI-01..11; design D7, D16 N3): quit requests, the app bundle to
//! the Trash, the Homebrew cask/formula adapters and `~/Library` leftovers. The decisions live in
//! `cm_core::uninstall::mac`.
//!
//! - Records come from the same inventory as `apps list` (`apps::inventory`, same instance ids) plus the Homebrew
//!   formulae installed on request (`updates::formula_instances`). Each carries its bundle ID, the Team ID its
//!   signature claims, the bundle's location identity and whether this user can move it (owner, write access to the
//!   bundle and its folder). App Store apps and bundles this user cannot change are refused as `permission_required`:
//!   this build has no administrator path on macOS and never asks for one.
//! - Quit (UNI-02): processes whose executable lies inside the bundle (or the formula's keg). A LaunchServices
//!   application gets `-[NSRunningApplication terminate]` (the system's quit request: the app may ask to save or
//!   refuse); a process without an application event loop (a helper or command-line tool inside the bundle) gets
//!   `SIGTERM`, the conventional request it may handle or ignore. Then a bounded wait. Ending
//!   (`forceTerminate` / `SIGKILL`) happens only in a plan reviewed for it, after a person confirmed it, and never for
//!   CleanManager, critical or macOS processes.
//! - The bundle move (UNI-01, N3): before it the record is re-read and must digest as planned, `Contents/Info.plist`
//!   must hash as planned and the signature's publisher must be unchanged. The move itself opens the bundle's folder
//!   without following a link, applies the trusted-namespace gate (owner root or this user, no other writer; group
//!   write is accepted only for `admin`/`wheel`, whose members can already act as root, which is how `/Applications`
//!   is set up), checks the bundle's identity, kind and flags no-follow, and hands over to the verified Trash path of
//!   the file actions (held event-only descriptor, last look at the name, `NSFileManager trashItemAtURL`, the held
//!   descriptor must name the resulting item). It is never a delete.
//! - Homebrew: `brew uninstall --cask|--formula <name>` through the trusted tool runner with Homebrew's user-owned
//!   trust policy, fixed argv (no `--zap`), cleared environment, bounded time; the outcome is the rescan of the bundle
//!   and the Caskroom/Cellar record, never the exit code alone.
//! - Leftovers: names under `~/Library/{Application Support, Caches, Logs, Preferences[/ByHost], Saved Application
//!   State, HTTPStorages, WebKit, LaunchAgents, Containers, Group Containers}` (launchd plists are parsed for their
//!   label and program). Sizes are read only for candidates and never inside Containers or Group Containers
//!   (privacy-protected, App Data). There are no registry steps on macOS.
//! - AI command-line tools (UNI-07, `uninstall/ai.rs`): npm-global Claude Code, Codex CLI and Gemini CLI under the npm prefixes
//!   of Homebrew, `/usr/local`, nvm and the user's npm configuration, removed with `npm uninstall --global` through a
//!   trusted Node.js; their data folders are offered per entry with sessions and settings never preselected.
//! - Orphans (UNI-08, `uninstall/orphans.rs`): `~/Library` entries of apps removed earlier, by bundle-ID evidence only, never
//!   preselected; a person names them for a separately reviewed Trash plan.
//!
//! Tests redirect everything with `CM_UNINSTALL_TEST_ROOT` / `CM_UNINSTALL_TEST_HOME` (see `apps`); `brew` is then
//! only `CM_UPDATES_TEST_BREW` (see `updates`), Node.js only a fixture under the fake root or home, and
//! `CM_UNINSTALL_QUIT_WAIT_MS` shortens the wait.

use std::ffi::{CString, c_int};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use cm_core::apps::{AppInstance, AppSource, EvidenceKind, InventoryIssue, protected_reason};
use cm_core::capability::{Capability, CapabilityState};
use cm_core::execute::ActionOutcome;
use cm_core::uninstall::mac::{Area, LibraryEntry, MacInstalled, MacRecord, team_id_of};
use cm_core::uninstall::{
    AdapterChoice, AdapterEnv, AdapterKind, AppIdentity, Binding, KeyBackup, KeyTree, Leftover, RecordSource, Refusal, RegLocation, Rescan, RunningProcess,
    RunningState, StepStatus, UninstallActions, UninstallRecord, record_source, select_adapter,
};

use super::actions::{self, Stop, Verified};
use super::apps::AppsEnv;
use super::ffi::cf::{self, Plist};
use super::ffi::objc::{AutoreleasePool, Bool, Id, Sel, class, sel};
use super::ids;
use super::sys::{Fd, SF_DATALESS, describe};

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[(
    "apps.uninstall",
    CapabilityState::Unverified,
    "Implemented on macOS and tested on hosted runners with fixtures only, not on a real Mac: quit requests (NSRunningApplication terminate, SIGTERM for helpers), the app bundle moved to the Trash through the verified Trash path (never deleted), brew uninstall --cask/--formula (never --zap), and ~/Library leftovers by bundle-ID and Team-ID evidence moved to the Trash in a separately reviewed plan. App Store apps and bundles this user cannot change are refused (no administrator path); Containers and Group Containers are only reported; npm-global AI command-line tools are removed with npm uninstall --global through a trusted Node.js (their sessions and settings never preselected) and orphaned ~/Library leftovers are listed by bundle-ID evidence only, never preselected",
)];

/// Where records, processes and leftovers are read from. Tests redirect `/` and the home folder.
#[derive(Debug, Clone)]
pub struct UninstallEnv {
    pub apps: AppsEnv,
    /// Bounded wait after the quit request (UNI-02).
    pub quit_wait: Duration,
    /// Administrator rights are never used on macOS; kept for the shared leftover planning.
    pub is_admin: bool,
}

impl UninstallEnv {
    pub fn real() -> UninstallEnv {
        UninstallEnv { apps: AppsEnv::real(), quit_wait: Duration::from_secs(cm_core::uninstall::QUIT_WAIT_SECONDS), is_admin: false }
    }

    /// The real environment unless `CM_UNINSTALL_TEST_ROOT` / `CM_UNINSTALL_TEST_HOME` redirect it;
    /// `CM_UNINSTALL_QUIT_WAIT_MS` shortens the wait after a quit request.
    pub fn from_process() -> Result<UninstallEnv, String> {
        let mut env = UninstallEnv { apps: AppsEnv::from_process()?, ..UninstallEnv::real() };
        if let Some(ms) = std::env::var("CM_UNINSTALL_QUIT_WAIT_MS").ok().and_then(|v| v.parse::<u64>().ok()) {
            env.quit_wait = Duration::from_millis(ms.min(60_000));
        }
        Ok(env)
    }

    pub fn is_redirected(&self) -> bool {
        self.apps.is_redirected()
    }

    fn system(&self, abs: &str) -> PathBuf {
        match &self.apps.root {
            None => PathBuf::from(abs),
            Some(r) => r.join(abs.trim_start_matches('/')),
        }
    }

    /// `~/Library` (the test home's while redirected).
    pub fn library(&self) -> Option<PathBuf> {
        self.apps.home.as_ref().map(|h| h.join("Library"))
    }
}

const CASKROOMS: [&str; 2] = ["/opt/homebrew/Caskroom", "/usr/local/Caskroom"];
const CELLARS: [&str; 2] = ["/opt/homebrew/Cellar", "/usr/local/Cellar"];
const INFO_PLIST_MAX: u64 = 8 << 20;
const LAUNCH_PLIST_MAX: u64 = 1 << 20;
/// Entries a leftover size walk visits before it gives up (the size is then unknown).
const SIZE_WALK_MAX: usize = 200_000;

fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

fn path_text(raw: &cm_core::path::RawPath) -> Option<String> {
    String::from_utf8(raw.raw_bytes()).ok()
}

fn writable(path: &Path) -> bool {
    let Ok(c) = CString::new(path.as_os_str().as_bytes()) else { return false };
    // SAFETY: NUL-terminated path; access only reads permissions.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// Why this user cannot move the bundle without administrator rights (`None` = it can).
fn admin_reason(bundle: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(bundle).ok()?;
    let me = euid();
    if me == 0 {
        return None;
    }
    if meta.uid() != me {
        let owner = if meta.uid() == 0 { "root".to_owned() } else { format!("another user (uid {})", meta.uid()) };
        return Some(format!("{} is owned by {owner}", bundle.display()));
    }
    // Moving a directory to another folder rewrites its `..` entry: the bundle itself must be writable, and its folder.
    if !writable(bundle) {
        return Some(format!("{} is not writable by you", bundle.display()));
    }
    match bundle.parent() {
        Some(parent) if !writable(parent) => Some(format!("its folder {} can only be changed by an administrator", parent.display())),
        _ => None,
    }
}

fn evidence_of(app: &AppInstance, kind: EvidenceKind) -> Vec<String> {
    app.evidence.iter().filter(|e| e.kind == kind).map(|e| e.detail.clone()).collect()
}

/// One inventory instance as an uninstall record (same instance id as `apps list`).
fn record_of(app: &AppInstance) -> UninstallRecord {
    let location = app.install_location.as_ref().and_then(path_text);
    let bundle_id = match &app.source {
        AppSource::AppBundle { bundle_id, .. } | AppSource::AppStore { bundle_id, .. } => Some(bundle_id.clone()),
        _ => evidence_of(app, EvidenceKind::BundleId).into_iter().find(|d| !d.starts_with("unknown")),
    };
    let (package_name, homebrew_kind) = match &app.source {
        AppSource::Homebrew { token, kind } => (Some(token.clone()), Some(*kind)),
        _ => (None, None),
    };
    let admin = match (&app.source, location.as_deref()) {
        (AppSource::AppBundle { .. }, Some(l)) => admin_reason(Path::new(l)),
        _ => None,
    };
    let protected = app.protected_class.or_else(|| protected_reason(&app.name, app.publisher.as_deref(), false));
    UninstallRecord {
        instance_id: app.instance_id.clone(),
        name: app.name.clone(),
        version: app.version.clone(),
        publisher: app.publisher.clone(),
        source: record_source(&app.source),
        key: None,
        key_name: None,
        product_code: None,
        package_full_name: None,
        package_family: None,
        uninstall_string: None,
        quiet_uninstall_string: None,
        install_location: location,
        system_component: false,
        winget: None,
        package_name,
        ai_tool: None,
        protected_class: protected,
        mac: Some(MacRecord {
            bundle_id,
            team_id: app.publisher.as_deref().and_then(team_id_of),
            homebrew_kind,
            volume_id: app.location_identity.as_ref().map(|i| i.volume_id.clone()),
            file_id: app.location_identity.as_ref().map(|i| i.file_id.clone()),
            admin_reason: admin,
            pkg_receipts: evidence_of(app, EvidenceKind::PkgReceipt),
            npm_prefix: None,
        }),
    }
}

/// Every uninstall record of this environment (bundles and Homebrew formulae), by name.
pub fn records(env: &UninstallEnv) -> (Vec<UninstallRecord>, Vec<InventoryIssue>) {
    let (mut apps, issues) = super::apps::inventory(&env.apps);
    apps.extend(super::updates::formula_instances(&env.apps));
    let mut out: Vec<UninstallRecord> = apps.iter().map(record_of).collect();
    ai::records(env, &mut out);
    let mut seen = std::collections::BTreeSet::new();
    out.retain(|r| seen.insert(r.instance_id.clone()));
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.instance_id.cmp(&b.instance_id)));
    (out, issues)
}

/// Installed instances as the leftover discovery compares them (`shared_owner`, `app_reappeared`).
pub fn installed(records: &[UninstallRecord]) -> Vec<MacInstalled> {
    records
        .iter()
        .map(|r| MacInstalled {
            instance_id: r.instance_id.clone(),
            name: r.name.clone(),
            bundle_id: r.mac.as_ref().and_then(|m| m.bundle_id.clone()),
            location: r.install_location.clone(),
        })
        .collect()
}

/// Trusted tool locations for one record: Homebrew's `brew` when it passes the user-owned policy.
pub fn adapter_env(env: &UninstallEnv, rec: &UninstallRecord) -> AdapterEnv {
    let brew = (rec.source == RecordSource::Homebrew)
        .then(|| super::updates::brew_exe(&env.apps))
        .flatten()
        .filter(|p| super::updates::brew_trusted(p).is_ok())
        .map(|p| p.to_string_lossy().into_owned());
    let node = (rec.source == RecordSource::NpmGlobal).then(|| ai::node_for(env, rec.mac.as_ref().and_then(|m| m.npm_prefix.as_deref()))).flatten();
    AdapterEnv { brew, node, ..AdapterEnv::default() }
}

/// UNI-01: the route `apps uninstall list` reports for one record (adapter, or the refusal with its reason).
pub fn route(env: &UninstallEnv, rec: &UninstallRecord) -> Result<AdapterChoice, Refusal> {
    select_adapter(rec, &adapter_env(env, rec), &|p: &str| Path::new(p).is_file())
}

/// Capability form of a route: `available`; `unsupported` for protected classes and for what needs administrator
/// rights (this build has no such path on macOS); `unverified` with the reason otherwise.
pub fn route_capability(route: &Result<AdapterChoice, Refusal>) -> Capability {
    match route {
        Ok(_) => Capability::new("app.uninstall", CapabilityState::Available, None),
        Err(r) if r.code == "policy_denied" || r.code == "permission_required" => Capability::new("app.uninstall", CapabilityState::Unsupported, Some(&r.message)),
        Err(r) => Capability::new("app.uninstall", CapabilityState::Unverified, Some(&r.message)),
    }
}

/// `apps list`: each instance's uninstall capability from the same decision `apps uninstall list` makes for it.
pub fn apply_uninstall_capability(env: &UninstallEnv, apps: &mut [AppInstance]) {
    let (recs, _) = records(env);
    for a in apps {
        a.uninstall_capability = match recs.iter().find(|r| r.instance_id == a.instance_id) {
            Some(r) => route_capability(&route(env, r)),
            None => Capability::new("app.uninstall", CapabilityState::Unverified, Some("no uninstall record matches this instance; see `cleanmanager apps uninstall list`")),
        };
    }
}

/// UNI-07: when an AI CLI was last used, from the newest entry of its data folder (Unix seconds); `None` = unknown.
pub fn ai_last_used(env: &UninstallEnv, tool_id: &str) -> Option<i64> {
    ai::last_used(env, tool_id)
}

fn sha256_file(path: &Path, max: u64) -> Result<String, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.file_type().is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if meta.len() > max {
        return Err(format!("{} is larger than {max} bytes", path.display()));
    }
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    cm_core::adapter::sha256_hex(f).map_err(|e| format!("{}: {e}", path.display()))
}

/// Plan-time (and pre-run) binding of a route: (SHA-256, signer). The bundle move binds `Contents/Info.plist` and the
/// signature's publisher; Homebrew binds the resolved `brew` it runs, npm the resolved `node` (and `npm-cli.js` must pass
/// the same trust policy).
pub fn verify_program(choice: &AdapterChoice) -> Result<(String, Option<String>), (&'static str, String)> {
    match choice.kind {
        AdapterKind::MacTrashAppBundle => {
            let bundle = Path::new(&choice.program);
            let sha = sha256_file(&bundle.join("Contents").join("Info.plist"), INFO_PLIST_MAX).map_err(|e| ("unsupported", format!("the bundle cannot be bound: {e}")))?;
            let signer = super::ffi::security::signature(bundle).ok().as_ref().and_then(super::apps::publisher_of);
            Ok((sha, signer))
        }
        AdapterKind::HomebrewCask | AdapterKind::HomebrewFormula => {
            let real = super::updates::brew_trusted(Path::new(&choice.program)).map_err(|e| ("policy_denied", e))?;
            let sha = sha256_file(&real, 64 << 20).map_err(|e| ("io_error", e))?;
            Ok((sha, None))
        }
        AdapterKind::NpmGlobal => {
            let real = super::cmd::trusted_user_program(Path::new(&choice.program), euid()).map_err(|e| ("policy_denied", e.to_string()))?;
            let npm = choice.argv.first().ok_or(("unsupported", "the npm program is missing".to_owned()))?;
            ai::trusted_script(Path::new(npm)).map_err(|e| ("policy_denied", format!("npm refused: {e}")))?;
            let sha = sha256_file(&real, ai::NODE_MAX).map_err(|e| ("io_error", e))?;
            Ok((sha, None))
        }
        _ => Err(("unsupported", "not a macOS uninstall route".into())),
    }
}

// ---------------------------------------------------------------------------------------------
// Leftovers

/// Logical bytes below `path` (no link followed); `None` when unreadable or larger than the walk allows.
fn logical_size(path: &Path) -> Option<u64> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_dir() {
        return Some(meta.len());
    }
    let mut total = 0u64;
    let mut seen = 0usize;
    let mut stack = vec![path.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).ok()?.flatten() {
            seen += 1;
            if seen > SIZE_WALK_MAX {
                return None;
            }
            let Ok(m) = e.path().symlink_metadata() else { continue };
            if m.is_dir() {
                stack.push(e.path());
            } else if m.is_file() {
                total += m.len();
            }
        }
    }
    Some(total)
}

/// `Label` and program of a launchd plist (bounded read, any plist format).
fn launch_facts(path: &Path) -> (Option<String>, Option<String>) {
    let plist = std::fs::metadata(path)
        .ok()
        .filter(|m| m.is_file() && m.len() <= LAUNCH_PLIST_MAX)
        .and_then(|_| std::fs::read(path).ok())
        .and_then(|b| cf::plist_from_bytes(&b).ok())
        .map(|(p, _)| p);
    let Some(p) = plist else { return (None, None) };
    let label = p.get("Label").and_then(Plist::as_str).map(str::to_owned);
    let program = p.get("Program").and_then(Plist::as_str).map(str::to_owned).or_else(|| match p.get("ProgramArguments") {
        Some(Plist::Array(a)) => a.first().and_then(Plist::as_str).map(str::to_owned),
        _ => None,
    });
    (label, program)
}

/// The `~/Library` entries leftovers are looked for in (names only; launchd plists parsed).
fn library_entries(env: &UninstallEnv) -> Vec<LibraryEntry> {
    let Some(lib) = env.library() else { return Vec::new() };
    let mut out = Vec::new();
    for area in Area::ALL {
        let dir = lib.join(area.rel());
        // A missing or privacy-blocked folder has nothing to offer; it is never read any further.
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Some(name) = e.file_name().to_str().map(str::to_owned) else { continue };
            let Ok(ft) = e.file_type() else { continue };
            if name.starts_with('.') || ft.is_symlink() {
                continue;
            }
            let path = e.path();
            let (launch_label, launch_program) =
                if area == Area::LaunchAgents && ft.is_file() && name.ends_with(".plist") { launch_facts(&path) } else { (None, None) };
            out.push(LibraryEntry { area, name, path: path.to_string_lossy().into_owned(), is_dir: ft.is_dir(), launch_label, launch_program });
        }
    }
    out
}

/// UNI-04/05/09: the grouped leftovers of one removed app. `sizes`: read logical sizes of the candidates (never
/// inside Containers or Group Containers).
pub fn discover(env: &UninstallEnv, identity: &AppIdentity, mac: Option<&MacRecord>, installed: &[MacInstalled], sizes: bool) -> Vec<Leftover> {
    // UNI-07: an AI CLI leaves its data folder, not ~/Library entries.
    if let Some(tool) = identity.ai_tool.as_deref().and_then(cm_core::uninstall::ai_cli) {
        return ai::leftovers(env, identity, tool, installed, sizes);
    }
    let entries = library_entries(env);
    let bundle_present = identity
        .install_location
        .as_deref()
        .filter(|l| l.to_ascii_lowercase().ends_with(".app") && std::fs::symlink_metadata(l).is_ok());
    let mut items = cm_core::uninstall::mac::leftovers(identity, mac, &entries, installed, bundle_present);
    if sizes {
        for l in &mut items {
            if let Binding::Path { path } = &l.binding {
                l.logical_bytes = logical_size(Path::new(path));
            }
        }
    }
    items
}

// ---------------------------------------------------------------------------------------------
// Processes (UNI-02)

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    /// `void objc_msgSend(void);` (`objc/message.h`): cast to the method's exact type before calling.
    fn objc_msgSend();
}

/// `objc_msgSend` as `unsafe extern "C" fn(id, SEL, args...) -> ret` (on arm64 it must be called with the method's
/// exact C type; it is not variadic).
macro_rules! msg_fn {
    (($($arg:ty),*) -> $ret:ty) => {
        // SAFETY: function pointers share one size and representation; the caller sends only selectors whose
        // implementation has exactly this C signature.
        unsafe {
            std::mem::transmute::<unsafe extern "C" fn(), unsafe extern "C" fn(Id, Sel $(, $arg)*) -> $ret>(objc_msgSend as unsafe extern "C" fn())
        }
    };
}

#[cfg(target_arch = "aarch64")]
fn truthy(b: Bool) -> bool {
    b
}
#[cfg(not(target_arch = "aarch64"))]
fn truthy(b: Bool) -> bool {
    b != 0
}

/// Send `selector` (`terminate` / `forceTerminate`, both `- (BOOL)…;`) to the LaunchServices application with this
/// pid. `None`: no such application (not registered with LaunchServices); `Some(delivered)` otherwise.
fn running_application(pid: i32, selector: &std::ffi::CStr) -> Option<bool> {
    let _pool = AutoreleasePool::new();
    let cls = class(c"NSRunningApplication")?;
    // `+ (nullable instancetype)runningApplicationWithProcessIdentifier:(pid_t)pid;`
    let lookup = msg_fn!((libc::pid_t) -> Id);
    // SAFETY: class object receiver; the result is autoreleased (+0) and lives until `_pool` drops.
    let app = unsafe { lookup(cls, sel(c"runningApplicationWithProcessIdentifier:"), pid) };
    if app.is_null() {
        return None;
    }
    let send = msg_fn!(() -> Bool);
    // SAFETY: live NSRunningApplication (kept by the pool); `terminate`/`forceTerminate` return BOOL and take no arguments.
    Some(truthy(unsafe { send(app, sel(selector)) }))
}

fn signal(pid: i32, sig: c_int) -> Result<(), String> {
    // SAFETY: plain syscall with integer arguments; it only signals the process named by `pid`.
    if unsafe { libc::kill(pid, sig) } == 0 { Ok(()) } else { Err(describe(super::sys::errno())) }
}

/// The process still runs the planned executable (not exited, not a zombie, not a reused pid).
fn still_running(p: &RunningProcess) -> bool {
    match super::procs::process(p.pid as i32) {
        Ok(i) => !i.zombie && i.path.as_deref().map(|b| String::from_utf8_lossy(b).into_owned()) == p.path,
        Err(_) => false,
    }
}

/// Processes whose executable lies inside `location` (the bundle or keg), this process excluded.
fn running_under(location: &str) -> RunningState {
    let prefix = format!("{}/", location.trim_end_matches('/'));
    let me = std::process::id() as i32;
    match super::procs::snapshot() {
        Err(e) => RunningState::Unknown { reason: format!("the process list cannot be read: {}", describe(e)) },
        Ok(snap) => RunningState::Known {
            processes: snap
                .procs
                .into_iter()
                .filter(|p| p.pid != me && !p.zombie)
                .filter_map(|p| {
                    let path = String::from_utf8(p.path?).ok()?;
                    path.starts_with(&prefix).then_some(RunningProcess { pid: p.pid as u32, name: p.name, path: Some(path) })
                })
                .collect(),
        },
    }
}

/// A person's answer to "end these still-running processes now?" (UNI-02).
pub type ConfirmEnd = Box<dyn Fn(&[RunningProcess]) -> bool>;

/// The planned bundle verified right before its move: (path, volume id, file id).
type VerifiedBundle = (String, String, String);

/// Platform uninstall backend for the shared executor.
pub struct MacUninstall {
    pub env: UninstallEnv,
    /// A person confirms ending still-running processes (interactive apply only; otherwise always false).
    pub confirm: ConfirmEnd,
    /// Set by `verify`, taken by the bundle move that follows it in the same action.
    verified: Mutex<Option<VerifiedBundle>>,
}

impl MacUninstall {
    pub fn new(env: UninstallEnv, confirm: ConfirmEnd) -> MacUninstall {
        MacUninstall { env, confirm, verified: Mutex::new(None) }
    }

    pub fn non_interactive(env: UninstallEnv) -> MacUninstall {
        MacUninstall::new(env, Box::new(|_| false))
    }

    fn record(&self, instance_id: &str) -> Option<UninstallRecord> {
        records(&self.env).0.into_iter().find(|r| r.instance_id == instance_id)
    }

    fn brew_run(&self, choice: &AdapterChoice, timeout_seconds: u32) -> Result<(Option<i32>, u64), String> {
        let started = Instant::now();
        let out = super::updates::brew_tool(Path::new(&choice.program), &self.env.apps, &choice.argv, Duration::from_secs(u64::from(timeout_seconds)))
            .run()
            .map_err(|e| e.to_string())?;
        let ms = started.elapsed().as_millis() as u64;
        Ok((if out.timed_out { None } else { out.exit_code }, ms))
    }
}

fn unsupported_registry() -> (String, String) {
    ("unsupported".into(), "there are no registry steps on macOS".into())
}

impl UninstallActions for MacUninstall {
    fn is_admin(&self) -> bool {
        euid() == 0
    }

    fn running(&self, identity: &AppIdentity) -> RunningState {
        if identity.source == RecordSource::NpmGlobal {
            // Node runs these tools: look at executables and script arguments inside the package.
            return match identity.install_location.as_deref() {
                Some(dir) => ai::running(dir),
                None => RunningState::Unknown { reason: "the package location is not recorded, so its processes cannot be identified".into() },
            };
        }
        match identity.install_location.as_deref() {
            Some(dir) if Path::new(dir).is_dir() => running_under(dir),
            Some(_) => RunningState::Known { processes: vec![] },
            None => RunningState::Unknown { reason: "the bundle location is not recorded, so its processes cannot be identified".into() },
        }
    }

    fn request_quit(&self, processes: &[RunningProcess]) -> Vec<RunningProcess> {
        for p in processes {
            let pid = p.pid as i32;
            if !still_running(p) {
                continue;
            }
            // A LaunchServices application gets the system's quit request; anything else the conventional SIGTERM.
            if running_application(pid, c"terminate").is_none() {
                let _ = signal(pid, libc::SIGTERM);
            }
        }
        let deadline = Instant::now() + self.env.quit_wait;
        loop {
            let still: Vec<RunningProcess> = processes.iter().filter(|p| still_running(p)).cloned().collect();
            if still.is_empty() || Instant::now() >= deadline {
                return still;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn confirm_end(&self, processes: &[RunningProcess]) -> bool {
        (self.confirm)(processes)
    }

    fn end_process(&self, process: &RunningProcess) -> Result<(), String> {
        if let Some(why) = cm_core::uninstall::mac::end_refusal(process, std::process::id()) {
            return Err(format!("{} is never ended: {why}", process.describe()));
        }
        if !still_running(process) {
            return Ok(());
        }
        let pid = process.pid as i32;
        if running_application(pid, c"forceTerminate") != Some(true) {
            signal(pid, libc::SIGKILL).map_err(|e| format!("{} could not be ended: {e}", process.describe()))?;
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while still_running(process) {
            if Instant::now() >= deadline {
                return Err(format!("{} is still running after it was ended", process.describe()));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }

    fn verify(&self, rec: &UninstallRecord, choice: &AdapterChoice, sha256: &str, signer: Option<&str>) -> Result<(), (String, String)> {
        *self.verified.lock().unwrap_or_else(|p| p.into_inner()) = None;
        let live = self.record(&rec.instance_id).ok_or(("record_changed".to_owned(), "the application is no longer installed there; nothing was run".to_owned()))?;
        if live.digest() != rec.digest() {
            return Err(("record_changed".into(), "the application changed since planning (version, signature, location or permissions); create a new plan".into()));
        }
        let (sha, live_signer) = verify_program(choice).map_err(|(c, m)| (c.to_owned(), m))?;
        if sha != sha256 {
            let what = match choice.kind {
                AdapterKind::MacTrashAppBundle => "its Info.plist",
                AdapterKind::NpmGlobal => "the Node.js program",
                _ => "the brew program",
            };
            return Err(("program_changed".into(), format!("{what} changed since planning (SHA-256 differs); nothing was run")));
        }
        if live_signer.as_deref() != signer {
            return Err(("policy_denied".into(), format!("the code signature of {} changed since planning", choice.program)));
        }
        if choice.kind == AdapterKind::MacTrashAppBundle {
            let mac = rec.mac.as_ref();
            let (Some(v), Some(f)) = (mac.and_then(|m| m.volume_id.clone()), mac.and_then(|m| m.file_id.clone())) else {
                return Err(("unsupported".into(), "the bundle's identity was not recorded; nothing was moved".into()));
            };
            *self.verified.lock().unwrap_or_else(|p| p.into_inner()) = Some((choice.program.clone(), v, f));
        }
        Ok(())
    }

    fn run(&self, choice: &AdapterChoice, timeout_seconds: u32) -> Result<(Option<i32>, u64), String> {
        self.run_step(choice, timeout_seconds).map_err(|(_, code, message)| format!("{code}: {message}"))
    }

    fn run_step(&self, choice: &AdapterChoice, timeout_seconds: u32) -> Result<(Option<i32>, u64), (StepStatus, String, String)> {
        match choice.kind {
            AdapterKind::HomebrewCask | AdapterKind::HomebrewFormula => {
                self.brew_run(choice, timeout_seconds).map_err(|e| (StepStatus::Failed, "start_failed".to_owned(), format!("{e}; nothing was run")))
            }
            AdapterKind::NpmGlobal => {
                ai::run(&self.env, choice, timeout_seconds).map_err(|e| (StepStatus::Failed, "start_failed".to_owned(), format!("{e}; nothing was run")))
            }
            AdapterKind::MacTrashAppBundle => {
                let planned = self.verified.lock().unwrap_or_else(|p| p.into_inner()).take().filter(|(p, _, _)| p == &choice.program);
                let Some((path, volume_id, file_id)) = planned else {
                    return Err((StepStatus::Skipped, "precondition_missing".into(), "the bundle was not verified right before the move; nothing was moved".into()));
                };
                let started = Instant::now();
                match trash_bundle(&path, &volume_id, &file_id) {
                    ActionOutcome::Succeeded { .. } => Ok((Some(0), started.elapsed().as_millis() as u64)),
                    ActionOutcome::Skipped { code, message } => Err((StepStatus::Skipped, code, format!("{message}; the bundle was not moved"))),
                    ActionOutcome::Failed { code, message } => Err((StepStatus::Failed, code, format!("{message}; the bundle was not moved"))),
                    ActionOutcome::Unknown { message } => Err((StepStatus::Unknown, "unknown".into(), message)),
                }
            }
            _ => Err((StepStatus::Skipped, "unsupported".into(), "not a macOS uninstall route; nothing was run".into())),
        }
    }

    fn rescan(&self, identity: &AppIdentity) -> Rescan {
        let present = |p: &Path| std::fs::symlink_metadata(p).is_ok();
        let install_dir_present = identity.install_location.as_deref().map(|l| present(Path::new(l)));
        let registration_present = match (identity.source, identity.package_name.as_deref()) {
            (RecordSource::Homebrew, Some(name)) => Some(CASKROOMS.iter().chain(CELLARS.iter()).any(|room| present(&self.env.system(room).join(name)))),
            (RecordSource::Homebrew, None) => None,
            // npm keeps no registration beyond the package folder.
            (RecordSource::NpmGlobal, _) => install_dir_present,
            // A bundle has no registration of its own (LaunchServices follows the bundle).
            _ => Some(false),
        };
        Rescan { install_dir_present, registration_present }
    }

    fn read_key(&self, _loc: &RegLocation) -> Result<Option<KeyTree>, String> {
        Err(unsupported_registry().1)
    }

    fn delete_key(&self, _loc: &RegLocation) -> Result<(), (String, String)> {
        Err(unsupported_registry())
    }

    fn restore_key(&self, _backup: &KeyBackup) -> Result<(), (String, String)> {
        Err(unsupported_registry())
    }
}

// ---------------------------------------------------------------------------------------------
// The bundle move (N3: Trash only)

/// `admin` and `wheel`: groups whose members can already act as root (`sudo`); `/Applications` is `root:admin 0775`.
const PRIVILEGED_GROUPS: [u32; 2] = [80, 0];

/// Move the planned bundle to the Trash: its folder opened without following a link and gated, the bundle examined
/// no-follow against its planned identity, then the verified Trash move of the file actions. Never a delete.
pub fn trash_bundle(path: &str, volume_id: &str, file_id: &str) -> ActionOutcome {
    let _guard = match actions::no_materialize() {
        Ok(g) => g,
        Err(stop) => return stop.outcome(),
    };
    let skip = |code: &str, message: String| ActionOutcome::Skipped { code: code.into(), message };
    let Some((parent, name)) = path.trim_end_matches('/').rsplit_once('/') else { return skip("unsupported", "not an absolute bundle path".into()) };
    let parent = if parent.is_empty() { "/" } else { parent };
    if !actions::valid_component(name.as_bytes()) || !name.to_ascii_lowercase().ends_with(".app") {
        return skip("unsupported", format!("{path} is not an app bundle path"));
    }
    let (Ok(cparent), Ok(cname)) = (CString::new(parent), CString::new(name)) else { return skip("unsupported", "path contains NUL".into()) };
    let dir = match Fd::open_dir(&cparent) {
        Ok(d) => d,
        Err(libc::ENOENT) => return Stop::Missing.outcome(),
        Err(libc::ELOOP | libc::ENOTDIR) => return Stop::Parent.outcome(),
        Err(e @ (libc::EACCES | libc::EPERM)) => return Stop::Denied(format!("access denied: {}", describe(e))).outcome(),
        Err(e) => return Stop::Error(format!("cannot open {parent}: {}", describe(e))).outcome(),
    };
    let (pst, sfs) = match (dir.stat(), dir.statfs()) {
        (Ok(st), Ok(fs)) => (st, fs),
        (Err(e), _) | (_, Err(e)) => return Stop::Error(format!("cannot query {parent}: {}", describe(e))).outcome(),
    };
    // Trusted namespace (§7), with group write accepted only for admin/wheel.
    let mut gated = pst;
    if PRIVILEGED_GROUPS.contains(&pst.st_gid) {
        gated.st_mode &= !libc::S_IWGRP;
    }
    if let Err(why) = actions::namespace_gate(&gated, sfs.f_flags, euid(), || actions::acl_grants_change(&dir)) {
        return Stop::Gate(why).outcome();
    }
    if let Some(why) = actions::protected_flags(pst.st_flags) {
        return Stop::Protected(format!("{parent} is {why}")).outcome();
    }
    let live = match dir.stat_child(name.as_bytes()) {
        Ok(st) => st,
        Err(libc::ENOENT) => return Stop::Missing.outcome(),
        Err(e) => return Stop::Error(format!("cannot examine the bundle: {}", describe(e))).outcome(),
    };
    match live.st_mode & libc::S_IFMT {
        libc::S_IFLNK => return Stop::Reparse.outcome(),
        libc::S_IFDIR => {}
        _ => return Stop::Metadata("kind: no longer a directory".into()).outcome(),
    }
    let live_volume = match ids::volume_id(&dir, live.st_dev) {
        Ok(v) => v,
        Err(e) => return Stop::Error(format!("cannot identify the volume: {}", describe(e))).outcome(),
    };
    if live.st_dev != pst.st_dev || live_volume != volume_id || ids::file_id(live.st_ino) != file_id {
        return Stop::Identity.outcome();
    }
    if live.st_flags & SF_DATALESS != 0 {
        return Stop::Unsupported("cloud placeholder (dataless): the bundle is not on this Mac".into()).outcome();
    }
    if let Some(why) = actions::protected_flags(live.st_flags) {
        return Stop::Protected(format!("the bundle is {why}")).outcome();
    }
    let v = Verified { parent: dir, name: cname, live };
    actions::trash::move_verified(&v, path.as_bytes())
}

mod ai;
mod orphans;

pub use orphans::orphans;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_ai;
