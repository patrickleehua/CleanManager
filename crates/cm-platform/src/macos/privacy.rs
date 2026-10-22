//! Bounded first access to privacy-protected locations (TCC), so a pending macOS privacy prompt never stalls a
//! command silently, and an access that does not return is reported with the cause that can be proven.
//!
//! Measured on a real Mac (Mac17,14, macOS 27.0.1; docs/platforms/macos-verification.md): when the responsible app
//! has no decision yet for a protected location (Downloads; Desktop, Documents and another app's data are documented
//! to behave the same), the first `open(2)` of it (what `opendir`/`read_dir` do) blocks in the kernel while tccd
//! shows the "would like to access…" prompt, and returns only once the person answers: there is no timeout and no
//! `EPERM`, also not in a launchd background session. `lstat` of the location itself does not wait. Other threads
//! keep running, and the process can exit while the blocked thread is still waiting. A decision already made returns
//! at once (an allowed one was measured; a denied one returns `EPERM`, which was not measured here).
//!
//! [`Access::run`] therefore runs that first access on a worker thread and waits at most [`prompt_wait`]. Past it the
//! caller reports the location `permission_required`, unknown (never 0 or empty), and moves on; the worker stays
//! parked until the prompt is answered and then ends, its result dropped. The cause written into the report
//! (`permission_cause`, docs/platforms/macos.md §6) is what can be proven from the location alone:
//! - `privacy_prompt_pending` only for a [`Category`] whose prompt was measured or is documented (Desktop, Documents,
//!   Downloads, one app's container), and not when an already-open ancestor shows a network, removable or automounted
//!   volume ([`remote_or_removable`]): a slow share or a sleeping disk does not answer either;
//! - `undetermined` for everything else that did not answer: unmeasured categories (iCloud Drive, File Provider,
//!   Photos, Contacts, Calendars, `/Volumes/<name>`) and any other path (a scan root on a slow volume), with an
//!   honest "did not answer within N s" message.
//!
//! Waits are keyed on the location ([`protected_root`]): while a worker for it is still parked, a further request is
//! answered at once ("not tried") instead of starting another thread, so a desktop refresh does not pile them up.
//! Each app's container is its own location (a container that never prompts, such as Docker Desktop's, is never
//! held up by another app's). At most [`MAX_PARKED`] workers wait at a time; past that a further first access is not
//! tried and is reported as such. Nothing here probes a location that would not be read anyway, retries or changes
//! any privacy setting.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use cm_core::permission::PermissionCause;

use super::sys::Fd;

/// How long a first access may wait for macOS by default before the location is reported unanswered.
pub const DEFAULT_PROMPT_WAIT: Duration = Duration::from_secs(5);
/// Overrides [`DEFAULT_PROMPT_WAIT`] (whole seconds, clamped to [`WAIT_LIMITS`]); docs/platforms/macos.md §11.
pub const WAIT_VAR: &str = "CM_PRIVACY_PROMPT_WAIT_SECS";
/// Accepted range of [`WAIT_VAR`], in seconds.
pub const WAIT_LIMITS: (u64, u64) = (1, 120);
/// Workers that may wait for macOS at the same time; past it a first access is not tried.
pub const MAX_PARKED: usize = 8;
/// After this long without an answer, the `--debug` trace says macOS is probably asking for permission.
const TRACE_AFTER: Duration = Duration::from_millis(250);
/// Test hook (debug builds only): a first access at or below one of these `:`-separated paths never returns, standing
/// in for an unanswered prompt in end-to-end tests. Release builds ignore it.
pub const TEST_HOLD_VAR: &str = "CM_TEST_PRIVACY_HOLD";
/// Prefix of every note about a location whose first access did not return in time where a prompt is the proven
/// explanation (docs/platforms/macos.md §6).
pub const PERMISSION_PENDING: &str = "permission_required (permission_cause=privacy_prompt_pending)";
/// Prefix of every note about a location whose blocking layer cannot be proven.
pub const PERMISSION_UNDETERMINED: &str = "permission_required (permission_cause=undetermined)";

/// The wait for a first access: [`WAIT_VAR`] when set to a whole number of seconds (clamped to [`WAIT_LIMITS`]),
/// else [`DEFAULT_PROMPT_WAIT`]. Read once per process.
pub fn prompt_wait() -> Duration {
    static WAIT: OnceLock<Duration> = OnceLock::new();
    *WAIT.get_or_init(|| wait_from(std::env::var(WAIT_VAR).ok().as_deref()))
}

/// [`prompt_wait`] from the variable's value.
pub(crate) fn wait_from(value: Option<&str>) -> Duration {
    match value.map(str::trim).and_then(|v| v.parse::<u64>().ok()) {
        Some(s) => Duration::from_secs(s.clamp(WAIT_LIMITS.0, WAIT_LIMITS.1)),
        None => DEFAULT_PROMPT_WAIT,
    }
}

/// The prefix of a note with this cause.
pub fn prefix(cause: PermissionCause) -> &'static str {
    match cause {
        PermissionCause::PrivacyPromptPending => PERMISSION_PENDING,
        PermissionCause::Undetermined => PERMISSION_UNDETERMINED,
    }
}

/// Whether `reason` (a note or an unknown-size reason) reports a pending privacy prompt.
pub fn is_pending(reason: &str) -> bool {
    reason.contains(PERMISSION_PENDING)
}

// ---------------------------------------------------------------------------------------------------------------
// Which locations are protected.

/// What guards a privacy-protected location, as far as this build knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// Desktop, Documents, Downloads (Files and Folders): the first open waits for the prompt (measured for
    /// Downloads, §6).
    FilesAndFolders,
    /// One app's `Library/Containers/<id>` or `Library/Group Containers/<id>` (App Data, macOS 14+): Apple documents
    /// a prompt; a container of an app that is not sandboxed is not checked at all.
    AppData,
    /// iCloud Drive (`Library/Mobile Documents`). Prompt behaviour not measured.
    ICloudDrive,
    /// File Provider storage (`Library/CloudStorage`, one provider's folder). Not measured.
    FileProvider,
    /// The Photos library (`Pictures/Photos Library.photoslibrary`). Not measured.
    Photos,
    /// Contacts data (`Library/Application Support/AddressBook`). Not measured.
    Contacts,
    /// Calendar data (`Library/Calendars`). Not measured.
    Calendars,
    /// A mounted volume `/Volumes/<name>` (Removable / Network Volumes). Not measured; it may also be a slow disk.
    Volume,
}

impl Category {
    /// Whether a first access that does not return is proven to be the privacy prompt (measured or documented); for
    /// the others the cause is `undetermined`.
    pub fn prompt_known(self) -> bool {
        matches!(self, Category::FilesAndFolders | Category::AppData)
    }
}

/// A privacy-protected location: the folder whose first access may wait, and what guards it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Protected {
    pub root: PathBuf,
    pub category: Category,
}

/// Folders below a home directory whose first open macOS holds for a Files and Folders prompt.
const FILES_AND_FOLDERS: &[&str] = &["Desktop", "Documents", "Downloads"];
/// Other single protected folders below a home directory (their own root).
const HOME_SINGLE: &[(&str, Category)] = &[
    ("Library/Mobile Documents", Category::ICloudDrive),
    ("Pictures/Photos Library.photoslibrary", Category::Photos),
    ("Library/Application Support/AddressBook", Category::Contacts),
    ("Library/Calendars", Category::Calendars),
];
/// Folders below a home directory each child of which is another app's data (App Data protection); listing the folder
/// itself is not guarded.
const HOME_APP_DATA: &[&str] = &["Library/Containers", "Library/Group Containers"];
/// File Provider storage: the folder itself and each provider's folder in it are their own locations.
const FILE_PROVIDER: &str = "Library/CloudStorage";

/// The privacy-protected location `path` is at or below, if any: `<home>/Desktop`, `Documents` and `Downloads`;
/// `Library/Mobile Documents`, the Photos library, `Library/Application Support/AddressBook` and `Library/Calendars`;
/// one app's `<home>/Library/Containers/<id>` or `Library/Group Containers/<id>`; `Library/CloudStorage` or one
/// provider's folder in it; or a mounted volume `/Volumes/<name>`. Homes are `home` (`$HOME`) and every
/// `/Users/<name>` (also spelled through `/System/Volumes/Data`). Decided from the path text alone: nothing is
/// examined.
pub fn classify(path: &Path, home: Option<&Path>) -> Option<Protected> {
    use std::path::Component;
    let comps: Vec<Component<'_>> = path.components().collect();
    let normal = |i: usize| match comps.get(i) {
        Some(Component::Normal(n)) => Some(*n),
        _ => None,
    };
    if comps.first() != Some(&Component::RootDir) {
        return None;
    }
    if normal(1).is_some_and(|c| c == "Volumes")
        && let Some(name) = normal(2)
    {
        return Some(Protected { root: Path::new("/Volumes").join(name), category: Category::Volume });
    }
    let mut homes: Vec<PathBuf> = home.filter(|h| h.is_absolute()).map(Path::to_path_buf).into_iter().collect();
    let users_at = if [1, 2, 3].iter().zip(["System", "Volumes", "Data"]).all(|(&i, n)| normal(i).is_some_and(|c| c == n)) { 4 } else { 1 };
    if normal(users_at).is_some_and(|c| c == "Users")
        && let Some(user) = normal(users_at + 1)
    {
        homes.push(comps[..users_at + 1].iter().collect::<PathBuf>().join(user));
    }
    let found = |root: PathBuf, category| Some(Protected { root, category });
    for h in &homes {
        let Ok(rel) = path.strip_prefix(h) else { continue };
        if let Some(f) = FILES_AND_FOLDERS.iter().find(|f| rel.starts_with(f)) {
            return found(h.join(f), Category::FilesAndFolders);
        }
        if let Some((f, category)) = HOME_SINGLE.iter().find(|(f, _)| rel.starts_with(f)) {
            return found(h.join(f), *category);
        }
        for d in HOME_APP_DATA {
            if let Ok(inner) = rel.strip_prefix(d)
                && let Some(Component::Normal(app)) = inner.components().next()
            {
                return found(h.join(d).join(app), Category::AppData);
            }
        }
        if let Ok(inner) = rel.strip_prefix(FILE_PROVIDER) {
            let root = match inner.components().next() {
                Some(Component::Normal(provider)) => h.join(FILE_PROVIDER).join(provider),
                _ => h.join(FILE_PROVIDER),
            };
            return found(root, Category::FileProvider);
        }
    }
    None
}

/// The privacy-protected location `path` is at or below, if any ([`classify`]).
pub fn protected_root(path: &Path, home: Option<&Path>) -> Option<PathBuf> {
    classify(path, home).map(|p| p.root)
}

/// The location a first access to `path` waits for: its [`protected_root`]. Each app's container is its own key, so
/// one that never prompts is never reported pending because another one waits.
pub fn prompt_key(path: &Path, home: Option<&Path>) -> Option<PathBuf> {
    protected_root(path, home)
}

/// `MNT_REMOVABLE` from `<sys/mount.h>` (not exported by `libc`).
const MNT_REMOVABLE: u32 = 0x0000_0200;

/// Whether the volume `fd` is on is a network, removable or automounted one (`fstatfs`: no `MNT_LOCAL`, or
/// `MNT_REMOVABLE` / `MNT_AUTOMOUNTED`); such a volume can be slow to answer by itself. `false` when it cannot be told.
pub(crate) fn remote_or_removable(fd: &Fd) -> bool {
    fd.statfs().is_ok_and(|s| s.f_flags & libc::MNT_LOCAL as u32 == 0 || s.f_flags & (MNT_REMOVABLE | libc::MNT_AUTOMOUNTED as u32) != 0)
}

// ---------------------------------------------------------------------------------------------------------------
// The bounded first access.

/// Why a first access gave no result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateError {
    /// It did not return within the wait, or was not tried because earlier ones still wait: unknown, never empty.
    /// `message` starts with [`prefix`] of `cause`.
    Unanswered { cause: PermissionCause, message: String },
    /// It could not be run (no worker thread, or the worker ended without a result).
    Failed(String),
}

impl GateError {
    pub fn message(&self) -> &str {
        match self {
            GateError::Unanswered { message, .. } | GateError::Failed(message) => message,
        }
    }

    pub fn into_message(self) -> String {
        match self {
            GateError::Unanswered { message, .. } | GateError::Failed(message) => message,
        }
    }

    pub fn cause(&self) -> Option<PermissionCause> {
        match self {
            GateError::Unanswered { cause, .. } => Some(*cause),
            GateError::Failed(_) => None,
        }
    }

    /// The issue code: `permission_required` for an unanswered access, `io_error` when it could not be run.
    pub fn issue_code(&self) -> &'static str {
        match self {
            GateError::Unanswered { .. } => "permission_required",
            GateError::Failed(_) => "io_error",
        }
    }

    /// The precondition code of an action skipped because of it: `privacy_prompt_pending` when the prompt is the
    /// proven cause, `access_timeout` when it is undetermined; `None` when the access could not be run.
    pub fn skip_code(&self) -> Option<&'static str> {
        match self {
            GateError::Unanswered { cause: PermissionCause::PrivacyPromptPending, .. } => Some(SKIP_PROMPT_PENDING),
            GateError::Unanswered { cause: PermissionCause::Undetermined, .. } => Some(SKIP_ACCESS_TIMEOUT),
            GateError::Failed(_) => None,
        }
    }
}

impl std::fmt::Display for GateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

/// Precondition code of an action skipped while macOS shows a privacy prompt for its location.
pub const SKIP_PROMPT_PENDING: &str = "privacy_prompt_pending";
/// Precondition code of an action skipped because a first access did not return, cause undetermined.
pub const SKIP_ACCESS_TIMEOUT: &str = "access_timeout";

/// Locations whose worker is still parked in the kernel, with a cap.
pub struct Registry {
    parked: Mutex<BTreeSet<PathBuf>>,
    cap: usize,
}

impl Registry {
    pub const fn new(cap: usize) -> Registry {
        Registry { parked: Mutex::new(BTreeSet::new()), cap }
    }

    fn lock(&self) -> MutexGuard<'_, BTreeSet<PathBuf>> {
        self.parked.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Locations waiting now.
    pub fn parked(&self) -> usize {
        self.lock().len()
    }
}

static GLOBAL: Registry = Registry::new(MAX_PARKED);

/// The process-wide registry every product access uses.
pub fn global() -> &'static Registry {
    &GLOBAL
}

/// Marks a location as no longer waiting when dropped, also when the reader panics.
struct Parked {
    registry: &'static Registry,
    key: PathBuf,
}

impl Drop for Parked {
    fn drop(&mut self) {
        self.registry.lock().remove(&self.key);
    }
}

/// Runs on the worker just before a first access (tests stand in for an unanswered prompt by blocking in it).
pub type Hold = std::sync::Arc<dyn Fn(&Path) + Send + Sync>;

/// How first accesses are bounded: the home [`classify`] uses besides every `/Users/<name>`, the wait, where waiting
/// workers are counted and an optional test [`Hold`].
#[derive(Clone)]
pub struct Gate {
    pub home: Option<PathBuf>,
    pub wait: Duration,
    pub registry: &'static Registry,
    pub hold: Option<Hold>,
}

impl std::fmt::Debug for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gate").field("home", &self.home).field("wait", &self.wait).field("hold", &self.hold.is_some()).finish()
    }
}

impl Default for Gate {
    fn default() -> Self {
        Gate::os()
    }
}

impl Gate {
    /// `$HOME`, [`prompt_wait`] and the process-wide registry.
    pub fn os() -> Gate {
        Gate { home: os_home(), wait: prompt_wait(), registry: global(), hold: None }
    }

    /// [`Gate::os`] classifying below `home` (a resolver's home, which tests redirect) instead of `$HOME`.
    pub fn for_home(home: Option<&Path>) -> Gate {
        Gate { home: home.map(Path::to_path_buf), ..Gate::os() }
    }

    /// The first access to `path` under this gate.
    pub fn access(&self, path: &Path) -> Access {
        let mut a = Access::of(path, self.home.as_deref()).wait(self.wait).in_registry(self.registry);
        a.hold = self.hold.clone();
        a
    }

    /// Run `op`, the first access to `path`, bounded ([`Access::run`]).
    pub fn run<T: Send + 'static>(&self, path: &Path, op: impl FnOnce() -> T + Send + 'static) -> Result<T, GateError> {
        self.access(path).run(op)
    }

    /// Run `op` bounded only when `path` is at or below a protected location; otherwise directly.
    pub fn run_if_protected<T: Send + 'static>(&self, path: &Path, op: impl FnOnce() -> T + Send + 'static) -> Result<T, GateError> {
        if classify(path, self.home.as_deref()).is_some() { self.run(path, op) } else { Ok(op()) }
    }
}

/// One first access to run bounded: the path it opens, the location it waits for and what a missing answer is
/// reported as.
#[derive(Clone)]
pub struct Access {
    pub path: PathBuf,
    /// Accesses waiting for the same location share this key.
    pub key: PathBuf,
    /// The cause a missing answer is reported with.
    pub cause: PermissionCause,
    pub wait: Duration,
    pub registry: &'static Registry,
    pub hold: Option<Hold>,
}

impl std::fmt::Debug for Access {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Access").field("path", &self.path).field("key", &self.key).field("cause", &self.cause).field("wait", &self.wait).finish()
    }
}

impl Access {
    /// The first access to `path`: keyed on its protected location (or the path itself), `privacy_prompt_pending`
    /// only when that location's prompt is known ([`Category::prompt_known`]).
    pub fn of(path: &Path, home: Option<&Path>) -> Access {
        let found = classify(path, home);
        let cause = match &found {
            Some(p) if p.category.prompt_known() => PermissionCause::PrivacyPromptPending,
            _ => PermissionCause::Undetermined,
        };
        let key = found.map_or_else(|| path.to_path_buf(), |p| p.root);
        Access { path: path.to_path_buf(), key, cause, wait: prompt_wait(), registry: global(), hold: None }
    }

    /// [`Access::of`] with `$HOME` as the home.
    pub fn of_os(path: &Path) -> Access {
        Access::of(path, os_home().as_deref())
    }

    pub fn wait(mut self, wait: Duration) -> Access {
        self.wait = wait;
        self
    }

    pub fn in_registry(mut self, registry: &'static Registry) -> Access {
        self.registry = registry;
        self
    }

    /// The location lies on a network, removable or automounted volume ([`remote_or_removable`]): a missing answer is
    /// then `undetermined`.
    pub fn remote(mut self, remote: bool) -> Access {
        if remote {
            self.cause = PermissionCause::Undetermined;
        }
        self
    }

    fn unanswered(&self, detail: String) -> GateError {
        GateError::Unanswered { cause: self.cause, message: format!("{}: {detail}", prefix(self.cause)) }
    }

    fn hint(&self) -> &'static str {
        match self.cause {
            PermissionCause::PrivacyPromptPending => "macOS is most likely showing a privacy prompt (\"would like to access…\"). Answer it, then measure again",
            PermissionCause::Undetermined => {
                "this can be a macOS privacy prompt (look for a \"would like to access…\" dialog) or a slow or unresponsive volume (a network share, a sleeping disk); which one cannot be told here"
            }
        }
    }

    fn late(&self) -> GateError {
        let what = self.path.display();
        let secs = self.wait.as_secs_f32();
        let detail = match self.cause {
            PermissionCause::PrivacyPromptPending => format!("macOS did not answer within {secs} s when opening {what}; {}", self.hint()),
            PermissionCause::Undetermined => format!("opening {what} did not answer within {secs} s; {}", self.hint()),
        };
        self.unanswered(format!("{detail}. Nothing was read and the size is unknown"))
    }

    fn still_waiting(&self) -> GateError {
        let earlier = if self.key == self.path { "an earlier first access to it".to_owned() } else { format!("an earlier first access to {}", self.key.display()) };
        self.unanswered(format!(
            "{} was not tried: {earlier} has not returned yet ({}). Nothing was read and the size is unknown",
            self.path.display(),
            self.hint()
        ))
    }

    fn over_cap(&self, waiting: usize) -> GateError {
        self.unanswered(format!(
            "{} was not tried: {waiting} other first accesses are already waiting for macOS (privacy prompts or slow volumes). Answer any \"would like to access…\" prompt, then measure again. Nothing was read and the size is unknown",
            self.path.display()
        ))
    }

    /// Run `op`, this first access, on a worker thread and wait at most [`Access::wait`] for it.
    pub fn run<T: Send + 'static>(&self, op: impl FnOnce() -> T + Send + 'static) -> Result<T, GateError> {
        {
            let mut parked = self.registry.lock();
            if parked.contains(&self.key) {
                cm_core::debug_trace!("privacy", "{} not tried: {} is still waiting for macOS", self.path.display(), self.key.display());
                return Err(self.still_waiting());
            }
            if parked.len() >= self.registry.cap {
                cm_core::debug_trace!("privacy", "{} not tried: {} first accesses are waiting already", self.path.display(), parked.len());
                return Err(self.over_cap(parked.len()));
            }
            parked.insert(self.key.clone());
        }
        let (tx, rx) = mpsc::sync_channel(1);
        let (registry, key, path, hold) = (self.registry, self.key.clone(), self.path.clone(), self.hold.clone());
        let started = Instant::now();
        let spawned = std::thread::Builder::new().name("cm-privacy-gate".into()).spawn(move || {
            let parked = Parked { registry, key };
            if held_for_test(&path) {
                loop {
                    std::thread::park(); // stands in for an unanswered prompt; never returns
                }
            }
            if let Some(hold) = hold {
                hold(&path);
            }
            let out = op();
            drop(parked); // before the result is sent: a caller that has it may open the path again at once
            if tx.send(out).is_err() {
                cm_core::debug_trace!("privacy", "{} answered after {} ms, after it was reported unanswered", path.display(), started.elapsed().as_millis());
            }
        });
        if let Err(e) = spawned {
            drop(Parked { registry: self.registry, key: self.key.clone() });
            return Err(GateError::Failed(format!("{} was not read: no reader thread could be started ({e})", self.path.display())));
        }
        let ended = || GateError::Failed(format!("{} was not read: the reader thread ended without a result", self.path.display()));
        let first = self.wait.min(TRACE_AFTER);
        match rx.recv_timeout(first) {
            Ok(v) => return Ok(v),
            Err(RecvTimeoutError::Disconnected) => return Err(ended()),
            Err(RecvTimeoutError::Timeout) => {}
        }
        cm_core::debug_trace!(
            "privacy",
            "opening {} has waited {} ms; macOS may be showing a privacy prompt (or the volume is slow). Waiting up to {} s",
            self.path.display(),
            started.elapsed().as_millis(),
            self.wait.as_secs_f32()
        );
        match rx.recv_timeout(self.wait.saturating_sub(first)) {
            Ok(v) => {
                cm_core::debug_trace!("privacy", "{} answered after {} ms", self.path.display(), started.elapsed().as_millis());
                Ok(v)
            }
            Err(RecvTimeoutError::Timeout) => {
                cm_core::debug_trace!("privacy", "no answer for {} after {} s; reported {}", self.path.display(), self.wait.as_secs_f32(), prefix(self.cause));
                Err(self.late())
            }
            Err(RecvTimeoutError::Disconnected) => Err(ended()),
        }
    }
}

/// `$HOME`, when set to an absolute path.
pub fn os_home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from).filter(|h| h.is_absolute())
}

/// [`TEST_HOLD_VAR`] (debug builds only).
fn held_for_test(path: &Path) -> bool {
    if !cfg!(debug_assertions) {
        return false;
    }
    static HOLD: OnceLock<Vec<PathBuf>> = OnceLock::new();
    let held = HOLD.get_or_init(|| {
        std::env::var_os(TEST_HOLD_VAR)
            .map(|v| std::env::split_paths(&v).filter(|p| p.is_absolute()).collect())
            .unwrap_or_default()
    });
    held.iter().any(|h| path.starts_with(h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Receiver;

    /// A registry of its own per test: tests run in parallel and must not share the process-wide cap.
    fn registry(cap: usize) -> &'static Registry {
        Box::leak(Box::new(Registry::new(cap)))
    }

    fn access(path: &str, home: Option<&str>, wait: Duration, reg: &'static Registry) -> Access {
        Access::of(Path::new(path), home.map(Path::new)).wait(wait).in_registry(reg)
    }

    /// A reader that blocks until the returned sender is used or dropped (an unanswered prompt).
    fn blocking_reader() -> (mpsc::Sender<()>, impl FnOnce() -> u32 + Send + 'static) {
        let (release, gate): (mpsc::Sender<()>, Receiver<()>) = mpsc::channel();
        (release, move || {
            let _ = gate.recv();
            7
        })
    }

    #[test]
    fn a_protected_location_that_does_not_answer_is_reported_pending_after_the_deadline() {
        let reg = registry(MAX_PARKED);
        let a = access("/Users/ann/Downloads", None, Duration::from_millis(150), reg);
        let (release, reader) = blocking_reader();
        let t = Instant::now();
        let why = a.run(reader).expect_err("an unanswered first access is not waited for");
        let took = t.elapsed();
        assert_eq!(why.cause(), Some(PermissionCause::PrivacyPromptPending));
        assert_eq!((why.issue_code(), why.skip_code()), ("permission_required", Some("privacy_prompt_pending")));
        let msg = why.message();
        assert!(is_pending(msg) && msg.contains("/Users/ann/Downloads") && msg.contains("within 0.15 s") && msg.contains("size is unknown"), "{msg}");
        assert_eq!(cm_core::permission::cause_in(msg), Some(PermissionCause::PrivacyPromptPending));
        assert!(took >= Duration::from_millis(150) && took < Duration::from_secs(3), "waited {took:?}");

        // Still parked: a second request for the same location is answered at once, says it was not tried.
        let t = Instant::now();
        let again = access("/Users/ann/Downloads/sub", None, Duration::from_secs(30), reg).run(|| 1u32).unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(1), "a parked location is not waited for again");
        assert!(is_pending(again.message()) && again.message().contains("/Users/ann/Downloads/sub was not tried"), "{again}");
        assert!(!again.message().contains("was opened"), "nothing was opened: {again}");

        // Answered later: the worker ends, and the location is read normally from then on.
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match a.clone().wait(Duration::from_secs(1)).run(|| 2u32) {
                Ok(v) => break assert_eq!(v, 2),
                Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
                Err(w) => panic!("the location stayed waiting after the answer: {w}"),
            }
        }
    }

    #[test]
    fn a_slow_path_that_is_not_a_measured_prompt_is_undetermined() {
        let reg = registry(MAX_PARKED);
        for path in ["/private/tmp/cm-slow-share/root", "/Volumes/NAS/Projects", "/Users/ann/Library/CloudStorage/OneDrive-Personal", "/Users/ann/Library/Mobile Documents/x"] {
            let (release, reader) = blocking_reader();
            let why = access(path, None, Duration::from_millis(100), reg).run(reader).unwrap_err();
            assert_eq!(why.cause(), Some(PermissionCause::Undetermined), "{path}");
            assert_eq!(why.skip_code(), Some("access_timeout"));
            let msg = why.message();
            assert!(msg.starts_with(PERMISSION_UNDETERMINED) && !is_pending(msg), "{msg}");
            assert!(msg.contains(&format!("opening {path} did not answer within 0.1 s")) && msg.contains("slow or unresponsive volume"), "{msg}");
            drop(release);
        }
        // A measured category on a network, removable or automounted volume is undetermined too.
        let (release, reader) = blocking_reader();
        let why = access("/Users/ann/Documents", None, Duration::from_millis(100), reg).remote(true).run(reader).unwrap_err();
        assert_eq!(why.cause(), Some(PermissionCause::Undetermined), "{why}");
        drop(release);
    }

    #[test]
    fn each_container_waits_on_its_own_and_parked_workers_are_capped() {
        let reg = registry(3);
        let home = Some("/Users/ann");
        let mut releases = Vec::new();
        for app in ["app.a", "app.b", "app.c"] {
            let (release, reader) = blocking_reader();
            releases.push(release);
            let why = access(&format!("/Users/ann/Library/Containers/{app}/Data"), home, Duration::from_millis(80), reg).run(reader).unwrap_err();
            assert!(is_pending(why.message()) && why.message().contains("did not answer"), "{app} is tried on its own: {why}");
        }
        assert_eq!(reg.parked(), 3);
        // Another container is not held up by them … until the cap is reached: then it is not tried, honestly.
        let t = Instant::now();
        let capped = access("/Users/ann/Library/Containers/com.docker.docker/Data/vms", home, Duration::from_secs(30), reg).run(|| 9u32).unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(1));
        assert!(capped.message().contains("was not tried: 3 other first accesses are already waiting"), "{capped}");
        assert_eq!(capped.cause(), Some(PermissionCause::PrivacyPromptPending), "its own category decides the cause");
        drop(releases.remove(0)); // app.a answered
        let deadline = Instant::now() + Duration::from_secs(5);
        while reg.parked() > 2 {
            assert!(Instant::now() < deadline, "the answered worker ends");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(access("/Users/ann/Library/Containers/com.docker.docker/Data/vms", home, Duration::from_secs(5), reg).run(|| 9u32), Ok(9), "a container that answers is read");
        drop(releases);
    }

    #[test]
    fn an_answer_within_the_deadline_returns_the_result() {
        let reg = registry(MAX_PARKED);
        let a = access("/nonexistent/cm-privacy-test/answered", None, Duration::from_secs(5), reg);
        let got = a.run(|| {
            std::thread::sleep(Duration::from_millis(300)); // past the trace point, well within the deadline
            Err::<(), _>(std::io::Error::from_raw_os_error(libc::EPERM))
        });
        let inner = got.expect("answered in time");
        assert_eq!(inner.unwrap_err().raw_os_error(), Some(libc::EPERM), "a denial is passed through as the read's own error");
        assert_eq!(a.run(|| 3u32), Ok(3), "nothing stays parked");
        assert_eq!(reg.parked(), 0);
    }

    #[test]
    fn protected_locations_are_named_from_the_path_alone() {
        let home = Some(Path::new("/opt/home/kim"));
        let root = |p: &str| protected_root(Path::new(p), home).map(|r| r.display().to_string());
        let cat = |p: &str| classify(Path::new(p), home).map(|r| r.category);
        assert_eq!(root("/Users/ann/Downloads").as_deref(), Some("/Users/ann/Downloads"));
        assert_eq!(root("/Users/ann/Documents/a/b").as_deref(), Some("/Users/ann/Documents"));
        assert_eq!(root("/Users/ann/Library/Mobile Documents/com~apple~CloudDocs").as_deref(), Some("/Users/ann/Library/Mobile Documents"));
        assert_eq!(root("/Users/ann/Library/Containers/com.docker.docker/Data/log").as_deref(), Some("/Users/ann/Library/Containers/com.docker.docker"));
        assert_eq!(root("/Users/ann/Library/Group Containers/group.x").as_deref(), Some("/Users/ann/Library/Group Containers/group.x"));
        assert_eq!(root("/System/Volumes/Data/Users/ann/Desktop/x").as_deref(), Some("/System/Volumes/Data/Users/ann/Desktop"));
        assert_eq!(root("/opt/home/kim/Desktop").as_deref(), Some("/opt/home/kim/Desktop"));
        assert_eq!(root("/Volumes/Stick/.Trashes/501").as_deref(), Some("/Volumes/Stick"));
        assert_eq!(root("/Users/ann/Library/CloudStorage").as_deref(), Some("/Users/ann/Library/CloudStorage"));
        assert_eq!(root("/Users/ann/Library/CloudStorage/GoogleDrive-a/My Drive").as_deref(), Some("/Users/ann/Library/CloudStorage/GoogleDrive-a"));
        assert_eq!(root("/Users/ann/Pictures/Photos Library.photoslibrary/database").as_deref(), Some("/Users/ann/Pictures/Photos Library.photoslibrary"));
        assert_eq!(root("/Users/ann/Library/Application Support/AddressBook/Sources").as_deref(), Some("/Users/ann/Library/Application Support/AddressBook"));
        assert_eq!(root("/Users/ann/Library/Calendars/x.calendar").as_deref(), Some("/Users/ann/Library/Calendars"));
        assert_eq!(cat("/Users/ann/Downloads/x"), Some(Category::FilesAndFolders));
        assert_eq!(cat("/Users/ann/Library/Containers/a"), Some(Category::AppData));
        for (p, c) in [
            ("/Users/ann/Library/Mobile Documents", Category::ICloudDrive),
            ("/Users/ann/Library/CloudStorage/x", Category::FileProvider),
            ("/Users/ann/Pictures/Photos Library.photoslibrary", Category::Photos),
            ("/Users/ann/Library/Application Support/AddressBook", Category::Contacts),
            ("/Users/ann/Library/Calendars", Category::Calendars),
            ("/Volumes/Stick", Category::Volume),
        ] {
            assert_eq!(cat(p), Some(c), "{p}");
            assert!(!c.prompt_known(), "{p}: never measured, cause undetermined");
        }
        let key = |p: &str| prompt_key(Path::new(p), home).map(|r| r.display().to_string());
        assert_eq!(key("/Users/ann/Library/Containers/com.a/Data").as_deref(), Some("/Users/ann/Library/Containers/com.a"), "one key per container");
        assert_eq!(key("/Users/ann/Library/Group Containers/group.x/y").as_deref(), Some("/Users/ann/Library/Group Containers/group.x"));
        assert_eq!(key("/Users/ann/Downloads/x").as_deref(), Some("/Users/ann/Downloads"));
        assert_eq!(key("/Users/ann/Projects"), None);
        for open in [
            "/Users/ann",
            "/Users/ann/Library/Containers",
            "/Users/ann/Library/Caches/x",
            "/Users/ann/Library/Application Support/Code",
            "/Users/ann/Pictures/Other",
            "/Users/ann/DownloadsOld",
            "/Users/ann/Projects/Downloads",
            "/Volumes",
            "/private/tmp/Downloads",
            "relative/Downloads",
        ] {
            assert_eq!(root(open), None, "{open}");
        }
    }

    #[test]
    fn the_wait_can_be_set_within_limits() {
        assert_eq!(wait_from(None), DEFAULT_PROMPT_WAIT);
        assert_eq!(wait_from(Some("12")), Duration::from_secs(12));
        assert_eq!(wait_from(Some(" 0 ")), Duration::from_secs(1), "clamped up");
        assert_eq!(wait_from(Some("9999")), Duration::from_secs(120), "clamped down");
        assert_eq!(wait_from(Some("soon")), DEFAULT_PROMPT_WAIT);
        assert_eq!(wait_from(Some("-3")), DEFAULT_PROMPT_WAIT);
    }

    #[test]
    fn a_reader_that_panics_is_not_reported_as_a_result() {
        let reg = registry(MAX_PARKED);
        let got: Result<u32, GateError> = access("/nonexistent/cm-privacy-test/panics", None, Duration::from_secs(5), reg).run(|| panic!("reader failed"));
        let why = got.unwrap_err();
        assert!(matches!(&why, GateError::Failed(m) if m.contains("without a result")), "{why:?}");
        assert_eq!((why.issue_code(), why.skip_code()), ("io_error", None));
        assert_eq!(reg.parked(), 0);
    }

    #[test]
    fn the_local_temp_volume_is_neither_remote_nor_removable() {
        let tmp = std::ffi::CString::new(std::env::temp_dir().as_os_str().as_encoded_bytes()).unwrap();
        let fd = Fd::open_dir_following(&tmp).unwrap();
        assert!(!remote_or_removable(&fd), "the runner's own disk is local");
    }
}
