//! Read-only startup inventory on macOS (parity 11.1, STA-01; design D9).
//!
//! Sources, one row per job definition (a plist), never per label, so two plists with the same label stay separate:
//! - `~/Library/LaunchAgents` (user scope, `launchd_user`), loaded into the user's `gui/<uid>` domain;
//! - `/Library/LaunchAgents` (system-wide, loaded per user into `gui/<uid>`) and `/Library/LaunchDaemons` (`system`);
//! - `/System/Library/LaunchAgents` and `/System/Library/LaunchDaemons`: part of macOS, always protected (STA-04).
//!
//! Each plist is parsed with CoreFoundation (XML, binary or OpenStep): `Label`, `Program`, `ProgramArguments`,
//! `RunAtLoad`, `KeepAlive`, `Disabled`. A plist that does not parse, or has no label or program, is still a row, in
//! state `unknown` with the reason. The target (`Program`, else `ProgramArguments[0]`; a bare name is looked up on
//! launchd's default `PATH`) is only stat'ed: missing on a mounted volume is `broken`; under `/Volumes/<name>` with
//! that volume not mounted it is `target_unreachable` (unknown, never broken: STA-03). The publisher is the target's
//! code signature as it claims it (`Apple` for Apple's own code, else the leaf certificate name with its Team ID;
//! basic validation only, not a trust decision).
//!
//! State: launchd's override database (`launchctl print-disabled gui/<uid>` / `system`) wins over the plist's
//! `Disabled` key; a job that is not disabled is `enabled` when its domain lists it as loaded (`launchctl print
//! gui/<uid>` / `system`, one call per domain) and `unknown` otherwise (it may be turned off in Login Items &
//! Extensions, limited to another session type, or unloaded). `launchctl` runs only through the trusted tool runner,
//! only with those read-only subcommands. The Background Task Management database is never read (no `sfltool`).
//!
//! Service Management login and background items have no public enumeration API: the inventory reports one honest
//! `unknown` row that points to System Settings (`x-apple.systempreferences:com.apple.LoginItems-Settings.extension`).
//!
//! Nothing here enables, disables, loads, unloads or deletes anything: every macOS row has `toggle_capability =
//! unsupported` with the reason, and no structured location (so `startup disable|enable|remove-stale` refuse).
//!
//! Tests: `CM_STARTUP_TEST_ROOT` (an absolute directory) is a fake `/` whose `Library/LaunchAgents`,
//! `Library/LaunchDaemons` and `System/Library/Launch*` are read instead; the user folder is `CM_STARTUP_TEST_FOLDER`
//! (default `<root>/home/Library/LaunchAgents`). While redirected `launchctl` is never run: the state comes from the
//! plist's `Disabled` key and the reason says so.

use std::collections::{BTreeMap, HashMap};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use cm_core::apps::{InventoryIssue, clean_text};
use cm_core::capability::CapabilityState;
use cm_core::path::RawPath;
use cm_core::startup::{PublisherEvidence, StartupItem, StartupScope, StartupSource, StartupState, TargetCheck, fold_state, item_id, normalize};

use super::cmd::Tool;
use super::ffi::cf::{self, Plist, PlistFormat};
use super::ffi::security::{self, Signature};

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[
    (
        "startup.toggle",
        CapabilityState::Unsupported,
        "Enabling, disabling or removing login items and launchd jobs is not implemented on macOS in this build; \
         `startup list` is a read-only inventory and System Settings > General > Login Items & Extensions changes them",
    ),
    (
        "startup.inventory",
        CapabilityState::Unverified,
        "Read-only launchd inventory (~/Library/LaunchAgents, /Library/LaunchAgents, /Library/LaunchDaemons, \
         /System/Library/Launch*) with launchctl state and code-signature publishers is implemented and tested on hosted \
         runners only, not on a real Mac; Service Management login/background items have no public listing API and are \
         reported as one unknown row pointing to System Settings",
    ),
];

/// System Settings page for login items and background items (macOS 13+).
pub const LOGIN_ITEMS_SETTINGS: &str = "x-apple.systempreferences:com.apple.LoginItems-Settings.extension";
/// Why no macOS row toggles in this build.
pub const TOGGLE_UNSUPPORTED: &str = "Read-only on macOS in this build: not enabled, disabled or removed here; use the app's own setting \
     or System Settings > General > Login Items & Extensions";
/// Reason of the Service Management row.
pub const SM_NOT_LISTABLE: &str = "Login items and background items registered through Service Management cannot be listed through a \
     public API (the Background Task Management database is not read); review them in System Settings > General > Login Items & \
     Extensions (x-apple.systempreferences:com.apple.LoginItems-Settings.extension)";
/// State note while redirected.
pub const TEST_STATE_NOTE: &str = "Test redirection: state from the plist's Disabled key; launchctl was not queried";

const LAUNCHCTL: &str = "/bin/launchctl";
const LAUNCHCTL_TIMEOUT: Duration = Duration::from_secs(20);
/// launchd's default `PATH` for jobs (`_PATH_STDPATH`), used for a bare `ProgramArguments[0]`.
const LAUNCHD_PATH: &[&str] = &["/usr/bin", "/bin", "/usr/sbin", "/sbin"];
/// Plists larger than this are not job definitions; reported as unknown instead of read.
const MAX_PLIST: u64 = 1 << 20;

/// Where startup definitions are read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupEnv {
    /// `None`: the real `/Library` and `/System/Library`. Otherwise a test root standing in for `/`.
    pub root: Option<PathBuf>,
    /// The user's LaunchAgents folder (`~/Library/LaunchAgents`); `None` when there is no home.
    pub user_agents: Option<PathBuf>,
    /// The user whose `gui/<uid>` domain is queried.
    pub uid: u32,
}

impl StartupEnv {
    /// The real locations of the current user.
    pub fn real() -> StartupEnv {
        let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from).filter(|h| h.is_absolute());
        // SAFETY: getuid has no preconditions and cannot fail.
        let uid = unsafe { libc::getuid() };
        StartupEnv { root: None, user_agents: home.map(|h| h.join("Library/LaunchAgents")), uid }
    }

    /// The real locations unless `CM_STARTUP_TEST_ROOT` redirects them (see the module documentation).
    pub fn from_process() -> Result<StartupEnv, String> {
        let mut env = StartupEnv::real();
        let Some(root) = std::env::var_os("CM_STARTUP_TEST_ROOT").filter(|r| !r.is_empty()) else { return Ok(env) };
        let root = Self::checked_root(Path::new(&root))?;
        env.user_agents = Some(match std::env::var_os("CM_STARTUP_TEST_FOLDER").filter(|f| !f.is_empty()) {
            Some(f) if Path::new(&f).is_absolute() => PathBuf::from(f),
            Some(_) => return Err("CM_STARTUP_TEST_FOLDER must be an absolute path".into()),
            None => root.join("home/Library/LaunchAgents"),
        });
        env.root = Some(root);
        Ok(env)
    }

    /// A test root: an absolute, existing directory other than `/`.
    pub fn checked_root(root: &Path) -> Result<PathBuf, String> {
        if !root.is_absolute() {
            return Err("CM_STARTUP_TEST_ROOT must be an absolute path".into());
        }
        let c = std::fs::canonicalize(root).map_err(|e| format!("CM_STARTUP_TEST_ROOT {}: {e}", root.display()))?;
        if c == Path::new("/") || !c.is_dir() {
            return Err("CM_STARTUP_TEST_ROOT must be a directory other than /".into());
        }
        Ok(c)
    }

    pub fn is_redirected(&self) -> bool {
        self.root.is_some()
    }

    fn system_path(&self, rel: &str) -> PathBuf {
        match &self.root {
            Some(r) => r.join(rel),
            None => Path::new("/").join(rel),
        }
    }

    /// The folders read, in order, with what they mean.
    fn folders(&self) -> Vec<Folder> {
        let mut out = Vec::new();
        if let Some(u) = &self.user_agents {
            out.push(Folder { dir: u.clone(), tier: Tier::User, domain: Domain::Gui });
        }
        out.push(Folder { dir: self.system_path("Library/LaunchAgents"), tier: Tier::Library, domain: Domain::Gui });
        out.push(Folder { dir: self.system_path("Library/LaunchDaemons"), tier: Tier::Library, domain: Domain::System });
        out.push(Folder { dir: self.system_path("System/Library/LaunchAgents"), tier: Tier::Os, domain: Domain::Gui });
        out.push(Folder { dir: self.system_path("System/Library/LaunchDaemons"), tier: Tier::Os, domain: Domain::System });
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    /// `~/Library/LaunchAgents`.
    User,
    /// `/Library/Launch*`: installed for every user.
    Library,
    /// `/System/Library/Launch*`: part of macOS.
    Os,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Domain {
    Gui,
    System,
}

#[derive(Debug, Clone)]
struct Folder {
    dir: PathBuf,
    tier: Tier,
    domain: Domain,
}

/// `KeepAlive` of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepAlive {
    No,
    Always,
    /// A dictionary of conditions (`SuccessfulExit`, `PathState`, ...).
    Conditional,
}

/// The parsed launchd job definition behind a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchdJob {
    pub plist: PathBuf,
    pub label: Option<String>,
    pub program: Option<String>,
    pub arguments: Vec<String>,
    /// `BundleProgram`: a program path relative to the bundle that registered the job (Service Management).
    pub bundle_program: Option<String>,
    pub run_at_load: bool,
    pub keep_alive: KeepAlive,
    /// The plist's own `Disabled` key (launchd's override database wins over it).
    pub disabled_key: Option<bool>,
    /// `xml`, `binary` or `openstep`.
    pub format: &'static str,
}

impl LaunchdJob {
    /// The program launchd would start: `Program`, else `ProgramArguments[0]`.
    pub fn program_path(&self) -> Option<&str> {
        self.program.as_deref().or(self.arguments.first().map(String::as_str)).filter(|p| !p.is_empty())
    }
}

/// Parse a job plist. `Err` when the bytes are not a property list dictionary.
pub fn parse_job(plist: &Path, bytes: &[u8]) -> Result<LaunchdJob, String> {
    let (value, format) = cf::plist_from_bytes(bytes)?;
    if !matches!(value, Plist::Dict(_)) {
        return Err("the property list is not a dictionary".into());
    }
    let text = |k: &str| value.get(k).and_then(Plist::as_str).and_then(clean_text);
    let flag = |k: &str| match value.get(k) {
        Some(Plist::Bool(b)) => Some(*b),
        Some(Plist::Integer(i)) => Some(*i != 0),
        _ => None,
    };
    let arguments = match value.get("ProgramArguments") {
        Some(Plist::Array(a)) => a.iter().map(|v| v.as_str().map(str::to_owned).unwrap_or_default()).collect(),
        _ => Vec::new(),
    };
    let keep_alive = match value.get("KeepAlive") {
        Some(Plist::Bool(true)) => KeepAlive::Always,
        Some(Plist::Dict(_)) => KeepAlive::Conditional,
        _ => KeepAlive::No,
    };
    Ok(LaunchdJob {
        plist: plist.to_path_buf(),
        label: text("Label"),
        program: value.get("Program").and_then(Plist::as_str).map(str::to_owned),
        arguments,
        bundle_program: text("BundleProgram"),
        run_at_load: flag("RunAtLoad").unwrap_or(false),
        keep_alive,
        disabled_key: flag("Disabled"),
        format: match format {
            PlistFormat::Xml => "xml",
            PlistFormat::Binary => "binary",
            PlistFormat::OpenStep => "openstep",
        },
    })
}

/// `launchctl print-disabled <domain>`: label → disabled. Older systems print `true`/`false`, newer
/// `disabled`/`enabled`. Only the `disabled services` block is read when present.
pub(crate) fn parse_print_disabled(text: &str) -> BTreeMap<String, bool> {
    let has_block = text.lines().any(|l| l.trim_start().starts_with("disabled services = {"));
    let mut inside = !has_block;
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let t = line.trim();
        if has_block {
            if t.starts_with("disabled services = {") {
                inside = true;
                continue;
            }
            if inside && t == "}" {
                inside = false;
                continue;
            }
        }
        if !inside {
            continue;
        }
        let Some(rest) = t.strip_prefix('"') else { continue };
        let Some((label, value)) = rest.split_once("\" =>") else { continue };
        let disabled = match value.trim() {
            "disabled" | "true" => true,
            "enabled" | "false" => false,
            _ => continue,
        };
        out.insert(label.to_owned(), disabled);
    }
    out
}

/// The `services = {` block of `launchctl print <domain>`: label → pid when running.
pub(crate) fn parse_services(text: &str) -> Option<BTreeMap<String, Option<u32>>> {
    let mut lines = text.lines();
    lines.by_ref().find(|l| l.trim() == "services = {")?;
    let mut out = BTreeMap::new();
    for line in lines {
        let t = line.trim();
        if t == "}" {
            return Some(out);
        }
        let mut parts = t.split_whitespace();
        let (Some(pid), Some(_status)) = (parts.next(), parts.next()) else { continue };
        let label: Vec<&str> = parts.collect();
        if label.is_empty() {
            continue;
        }
        out.insert(label.join(" "), pid.parse::<u32>().ok().filter(|p| *p > 0));
    }
    None
}

/// What launchd says about one domain (`Err` = the query failed, with why).
#[derive(Debug, Clone, Default)]
struct DomainState {
    overrides: Option<Result<BTreeMap<String, bool>, String>>,
    services: Option<Result<BTreeMap<String, Option<u32>>, String>>,
}

fn domain_name(d: Domain, uid: u32) -> String {
    match d {
        Domain::Gui => format!("gui/{uid}"),
        Domain::System => "system".into(),
    }
}

/// One read-only `launchctl` query through the trusted runner.
fn launchctl(args: &[&str]) -> Result<String, String> {
    let out = Tool::new(LAUNCHCTL).args(args).timeout(LAUNCHCTL_TIMEOUT).output_caps(16 << 20, 64 << 10).run().map_err(|e| e.to_string())?;
    if out.timed_out {
        return Err(format!("launchctl {} timed out", args.join(" ")));
    }
    if !out.success() {
        let (err, std) = (String::from_utf8_lossy(&out.stderr), String::from_utf8_lossy(&out.stdout));
        let first = err.lines().chain(std.lines()).map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_owned();
        return Err(format!("launchctl {} exited {:?}{}", args.join(" "), out.exit_code, if first.is_empty() { String::new() } else { format!(": {first}") }));
    }
    if out.stdout_truncated {
        return Err(format!("launchctl {} printed more than the output cap", args.join(" ")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn query_domain(d: Domain, uid: u32) -> DomainState {
    let name = domain_name(d, uid);
    DomainState {
        overrides: Some(launchctl(&["print-disabled", &name]).map(|t| parse_print_disabled(&t))),
        services: Some(launchctl(&["print", &name]).and_then(|t| parse_services(&t).ok_or_else(|| format!("launchctl print {name}: no services block")))),
    }
}

/// Read-only check of a target path.
fn check_target(p: &Path) -> TargetCheck {
    match std::fs::metadata(p) {
        Ok(_) => TargetCheck::Exists,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ENOTDIR) => {
            if volume_offline(p) { TargetCheck::VolumeOffline } else { TargetCheck::Missing }
        }
        Err(_) => TargetCheck::Unchecked,
    }
}

/// `/Volumes/<name>/...` whose volume is not mounted: the mount point is missing, or is a plain directory on the same
/// device as `/Volumes` (a stale mount point). `/Volumes/Macintosh HD` is a link to `/` and counts as mounted.
fn volume_offline(p: &Path) -> bool {
    let mut c = p.components();
    if !(c.next() == Some(Component::RootDir) && c.next() == Some(Component::Normal("Volumes".as_ref()))) {
        return false;
    }
    let Some(Component::Normal(name)) = c.next() else { return false };
    let volumes = Path::new("/Volumes");
    let mount = volumes.join(name);
    match std::fs::symlink_metadata(&mount) {
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        Ok(m) => m.is_dir() && std::fs::metadata(volumes).is_ok_and(|v| v.dev() == m.dev()),
    }
}

/// The target path of a program string: absolute as is, a bare name looked up on launchd's default `PATH`.
fn resolve_program(program: &str) -> Result<PathBuf, String> {
    if program.starts_with('/') {
        return Ok(PathBuf::from(program));
    }
    if !program.contains('/') {
        if let Some(found) = LAUNCHD_PATH.iter().map(|d| Path::new(d).join(program)).find(|p| p.is_file()) {
            return Ok(found);
        }
        return Err(format!("program {program} is looked up on the job's PATH; not found on launchd's default PATH"));
    }
    Err(format!("program {program} is a relative path; not checked"))
}

fn raw(p: &Path) -> Option<RawPath> {
    RawPath::unix(p.as_os_str().as_bytes().to_vec()).ok()
}

fn display(p: &Path) -> String {
    String::from_utf8_lossy(p.as_os_str().as_bytes()).into_owned()
}

/// Why a row is read-only beyond the platform-wide reason (STA-04).
fn protection(tier: Tier, name: &str, publisher: Option<&str>, sig: Option<&Signature>, target: Option<&Path>) -> Option<&'static str> {
    if tier == Tier::Os {
        return Some("Part of macOS (/System/Library, on the sealed system volume)");
    }
    if sig.is_some_and(|s| s.apple) {
        return Some("Signed by Apple: macOS component");
    }
    if target.is_some_and(|t| display(t).to_lowercase().contains("cleanmanager")) {
        return Some("CleanManager's own entry");
    }
    cm_core::startup::protected_reason(name, publisher, None, None)
}

/// A complete macOS inventory: the rows, the parsed job behind each launchd row (by `item_id`) and the issues.
#[derive(Debug, Clone, Default)]
pub struct Inventory {
    pub items: Vec<StartupItem>,
    pub jobs: BTreeMap<String, LaunchdJob>,
    pub issues: Vec<InventoryIssue>,
}

struct Builder<'a> {
    env: &'a StartupEnv,
    domains: HashMap<Domain, DomainState>,
    signatures: HashMap<PathBuf, Result<Signature, String>>,
    out: Inventory,
}

impl Builder<'_> {
    fn domain(&mut self, d: Domain) -> &DomainState {
        let uid = self.env.uid;
        self.domains.entry(d).or_insert_with(|| query_domain(d, uid))
    }

    fn signature(&mut self, p: &Path) -> Result<Signature, String> {
        self.signatures.entry(p.to_path_buf()).or_insert_with(|| security::signature(p)).clone()
    }

    fn folder(&mut self, f: &Folder) {
        let entries = match std::fs::read_dir(&f.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                let code = if e.kind() == std::io::ErrorKind::PermissionDenied { "permission_denied" } else { "io_error" };
                self.out.issues.push(InventoryIssue { code: code.into(), source: display(&f.dir), message: format!("folder could not be listed: {e}") });
                return;
            }
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "plist") && !p.file_name().is_some_and(|n| n.as_bytes().starts_with(b".")))
            .collect();
        paths.sort();
        for p in paths {
            self.plist(f, &p);
        }
    }

    fn plist(&mut self, f: &Folder, path: &Path) {
        let (source, scope) = match f.tier {
            Tier::User => (StartupSource::LaunchdUser, StartupScope::User),
            Tier::Library | Tier::Os => (StartupSource::LaunchdSystem, StartupScope::System),
        };
        let definition = display(path);
        let id = item_id(source, scope, &definition);
        let stem = path.file_stem().map(|s| String::from_utf8_lossy(s.as_bytes()).into_owned()).unwrap_or_else(|| definition.clone());
        let mut item = StartupItem {
            item_id: id.clone(),
            name: stem,
            source,
            definition,
            target: None,
            publisher: None,
            publisher_evidence: PublisherEvidence::None,
            scope,
            state: StartupState::Unknown,
            toggle_capability: CapabilityState::Unsupported,
            protected: f.tier == Tier::Os,
            reason: None,
            location: None,
            removable_when_stale: false,
        };
        let mut notes: Vec<String> = Vec::new();
        let job = match read_plist(path).and_then(|b| parse_job(path, &b)) {
            Ok(j) => j,
            Err(e) => {
                if let Some(p) = protection(f.tier, &item.name, None, None, None) {
                    notes.push(p.into());
                }
                notes.push(format!("The plist could not be read as a launchd job: {e}"));
                item.reason = Some(finish(notes));
                self.out.items.push(item);
                return;
            }
        };
        if let Some(l) = &job.label {
            item.name = l.clone();
        }
        // Target and publisher.
        let mut check = TargetCheck::Unchecked;
        let mut sig = None;
        let mut target_path = None;
        match job.program_path().map(resolve_program) {
            None if job.bundle_program.is_some() => {
                notes.push(format!("BundleProgram {} is relative to the bundle that registered the job; not checked", job.bundle_program.as_deref().unwrap_or("")))
            }
            None => notes.push("No Program or ProgramArguments: launchd does not start it".into()),
            Some(Err(why)) => notes.push(why),
            Some(Ok(t)) => {
                check = check_target(&t);
                if check == TargetCheck::Exists {
                    sig = self.signature(&t).ok();
                }
                item.target = raw(&t);
                target_path = Some(t);
            }
        }
        if let Some(s) = &sig
            && let Some(p) = super::apps::publisher_of(s)
        {
            item.publisher = Some(p);
            item.publisher_evidence = PublisherEvidence::CodeSignature;
        }
        if let Some(p) = protection(f.tier, &item.name, item.publisher.as_deref(), sig.as_ref(), target_path.as_deref()) {
            item.protected = true;
            notes.insert(0, p.into());
        }
        // Enabled state.
        let enabled: Result<bool, String> = match &job.label {
            None => Err("No Label key: launchd does not load it".into()),
            Some(_) if job.program_path().is_none() && job.bundle_program.is_none() => Err("launchd does not load a job without a program".into()),
            Some(_) if self.env.is_redirected() => {
                notes.push(TEST_STATE_NOTE.into());
                Ok(!job.disabled_key.unwrap_or(false))
            }
            Some(label) => self.live_state(f.domain, label, job.disabled_key, &mut notes),
        };
        let (state, why) = match enabled {
            Ok(e) => fold_state(e, check),
            Err(why) => {
                notes.push(why);
                match check {
                    TargetCheck::Missing | TargetCheck::VolumeOffline => fold_state(true, check),
                    _ => (StartupState::Unknown, None),
                }
            }
        };
        item.state = state;
        if let Some(w) = why {
            notes.push(w.into());
        }
        let launch: Vec<&str> = [
            job.run_at_load.then_some("RunAtLoad"),
            match job.keep_alive {
                KeepAlive::Always => Some("KeepAlive"),
                KeepAlive::Conditional => Some("KeepAlive (conditional)"),
                KeepAlive::No => None,
            },
        ]
        .into_iter()
        .flatten()
        .collect();
        if !launch.is_empty() {
            notes.push(format!("Launch: {}", launch.join(", ")));
        }
        item.reason = Some(finish(notes));
        self.out.jobs.insert(id, job);
        self.out.items.push(item);
    }

    /// Enabled per launchd (override database over the plist key; loaded in its domain), or why it is unknown.
    fn live_state(&mut self, d: Domain, label: &str, disabled_key: Option<bool>, notes: &mut Vec<String>) -> Result<bool, String> {
        let name = domain_name(d, self.env.uid);
        let st = self.domain(d).clone();
        let overrides = st.overrides.unwrap_or_else(|| Err("not queried".into()));
        let services = st.services.unwrap_or_else(|| Err("not queried".into()));
        match (&overrides, disabled_key) {
            (Ok(o), _) if o.get(label) == Some(&true) => {
                notes.push(format!("Disabled in launchd's override database ({name})"));
                return Ok(false);
            }
            (Ok(o), _) if o.get(label) == Some(&false) => {}
            (_, Some(true)) => {
                notes.push(format!("Disabled key set in the plist{}", if overrides.is_err() { "; launchd's override database could not be read" } else { "" }));
                return Ok(false);
            }
            _ => {}
        }
        match services {
            Ok(s) => match s.get(label) {
                Some(Some(pid)) => {
                    notes.push(format!("Loaded in {name}, running (pid {pid})"));
                    Ok(true)
                }
                Some(None) => {
                    notes.push(format!("Loaded in {name}"));
                    Ok(true)
                }
                None => Err(format!(
                    "Not disabled but not loaded in {name}: it may be turned off in Login Items & Extensions, limited to another session type, or unloaded"
                )),
            },
            Err(e) => Err(format!("launchd state unavailable: {e}")),
        }
    }
}

fn read_plist(path: &Path) -> Result<Vec<u8>, String> {
    let m = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !m.is_file() {
        return Err("not a regular file".into());
    }
    if m.len() > MAX_PLIST {
        return Err(format!("{} bytes is too large for a job definition", m.len()));
    }
    std::fs::read(path).map_err(|e| e.to_string())
}

/// Row reason: the notes, then the platform-wide toggle reason.
fn finish(mut notes: Vec<String>) -> String {
    notes.push(TOGGLE_UNSUPPORTED.into());
    notes.iter().map(|n| n.trim_end_matches('.')).collect::<Vec<_>>().join(". ")
}

/// The one Service Management row: no public API lists these items.
fn sm_row() -> StartupItem {
    let definition = LOGIN_ITEMS_SETTINGS.to_owned();
    StartupItem {
        item_id: item_id(StartupSource::SmBackgroundItem, StartupScope::User, &definition),
        name: "Login items and background items (Service Management)".into(),
        source: StartupSource::SmBackgroundItem,
        definition,
        target: None,
        publisher: None,
        publisher_evidence: PublisherEvidence::None,
        scope: StartupScope::User,
        state: StartupState::Unknown,
        toggle_capability: CapabilityState::Unsupported,
        protected: false,
        reason: Some(SM_NOT_LISTABLE.into()),
        location: None,
        removable_when_stale: false,
    }
}

/// Read-only inventory of `env` with the parsed job behind each launchd row. Unreadable folders are issues (the list
/// is then incomplete); a plist that does not parse is a row in state `unknown`. Nothing is changed or started.
pub fn inventory_detailed(env: &StartupEnv) -> Inventory {
    let mut b = Builder { env, domains: HashMap::new(), signatures: HashMap::new(), out: Inventory::default() };
    for f in env.folders() {
        b.folder(&f);
    }
    b.out.items.push(sm_row());
    let mut out = b.out;
    out.items = normalize(out.items);
    out
}

/// Read-only inventory of `env`: rows and issues (as `windows::startup::inventory_with`).
pub fn inventory_with(env: &StartupEnv) -> (Vec<StartupItem>, Vec<InventoryIssue>) {
    let inv = inventory_detailed(env);
    (inv.items, inv.issues)
}

/// Read-only inventory of this user's real locations.
pub fn inventory() -> (Vec<StartupItem>, Vec<InventoryIssue>) {
    inventory_with(&StartupEnv::real())
}

#[cfg(test)]
mod tests;
