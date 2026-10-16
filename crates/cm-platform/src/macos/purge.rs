//! Project build-artifact discovery (`purge`, PUR-01..05) on macOS: default project roots, the settings file next to
//! the store, read-only `CACHEDIR.TAG` evidence and the version-control check run at discovery and again before acting.
//!
//! The recognizer itself is the platform-neutral `cm_core::purge`. Version control: the nearest `.git` above an
//! artifact decides. Git is asked through the trusted tool runner (`/usr/bin/git ls-files`), and only when
//! `xcode-select -p` reports a developer directory: without Xcode or the Command Line Tools `/usr/bin/git` is a shim
//! that opens an install dialog. An artifact is fine to purge only when Git says nothing in it is tracked *and*
//! everything in it is ignored; untracked files that are not ignored may be uncommitted source, so that (like an
//! unavailable Git, a timeout or any Git error) is "unknown": kept, never selected, and refused again at execution.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Read;
use std::os::macos::fs::MetadataExt as _;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use cm_core::capability::CapabilityState;
use cm_core::cleanable::CACHEDIR_SIGNATURE;
use cm_core::execute::ActionTarget;
use cm_core::path::RawPath;
use cm_core::plan::Plan;
use cm_core::purge::{PurgeProbe, PurgeSettings, RULE_PREFIX, SETTINGS_FILE, SettingsError, VcsState};

use super::cmd::Tool;
use super::sys::{NoMaterialize, SF_DATALESS};

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[(
    "purge.artifacts",
    CapabilityState::Unverified,
    "macOS: project roots (~/Developer, ~/Projects, ~/code, ~/src, ~/workspace and configured roots), the shared artifact \
     recognizer, CACHEDIR.TAG evidence and a Git check at discovery and before acting are implemented. Git runs only when \
     Xcode or the Command Line Tools are installed; otherwise tracking is unknown and artifacts inside Git working copies \
     are never selected. Tested on hosted runners only, not on a real Mac; gate G1 not passed",
)];

/// Common project folder names under the home folder.
const HOME_DIRS: &[&str] = &["Developer", "Projects", "code", "src", "workspace"];
const GIT: &str = "/usr/bin/git";
const XCODE_SELECT: &str = "/usr/bin/xcode-select";
/// Validation fixtures only: `none` makes Git unavailable, as on a Mac without the Command Line Tools.
pub const TEST_GIT_VAR: &str = "CM_PURGE_TEST_GIT";
const GIT_TIMEOUT: Duration = Duration::from_secs(30);
const GIT_STDOUT_CAP: usize = 4 << 20;

pub fn raw(p: &Path) -> Option<RawPath> {
    RawPath::unix(p.as_os_str().as_bytes().to_vec()).ok()
}

pub fn os(p: &RawPath) -> Option<PathBuf> {
    match p {
        RawPath::Unix(b) => Some(PathBuf::from(OsStr::from_bytes(b))),
        RawPath::Windows(_) => None,
    }
}

fn plain_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

/// The home folder default roots and declared locations hang off: `CM_RESOLVE_HOME` (validation fixtures) or `$HOME`.
pub(crate) fn home() -> Option<PathBuf> {
    super::resolve::ResolverEnv::from_os().with_overrides(|k| std::env::var_os(k)).0.home
}

/// Whether two directory paths name the same directory (same identity when both exist, else the same spelling).
pub fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => a.as_os_str().as_bytes().strip_suffix(b"/").unwrap_or(a.as_os_str().as_bytes()) == b.as_os_str().as_bytes().strip_suffix(b"/").unwrap_or(b.as_os_str().as_bytes()),
    }
}

/// Default project roots that exist on this machine (plain directories, never links).
pub fn default_roots() -> Vec<RawPath> {
    let Some(home) = home() else { return vec![] };
    let mut seen: Vec<(u64, u64)> = Vec::new();
    HOME_DIRS
        .iter()
        .map(|d| home.join(d))
        .filter(|p| plain_dir(p))
        .filter(|p| {
            // `code` and `Code` are one folder on a case-insensitive volume.
            let Ok(m) = std::fs::symlink_metadata(p) else { return false };
            let key = (m.dev(), m.ino());
            let fresh = !seen.contains(&key);
            seen.push(key);
            fresh
        })
        .filter_map(|p| raw(&p))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Settings file (versioned, atomic replace; shared with the desktop)

pub fn settings_path(store_dir: &Path) -> PathBuf {
    store_dir.join(SETTINGS_FILE)
}

/// Missing file = defaults; an unreadable or unknown-version file is an error (never silently reset).
pub fn load_settings(store_dir: &Path) -> Result<PurgeSettings, SettingsError> {
    match std::fs::read_to_string(settings_path(store_dir)) {
        Ok(text) => PurgeSettings::parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PurgeSettings::default()),
        Err(e) => Err(SettingsError::Invalid(e.to_string())),
    }
}

pub fn save_settings(store_dir: &Path, s: &PurgeSettings) -> std::io::Result<()> {
    std::fs::create_dir_all(store_dir)?;
    let tmp = store_dir.join(format!("{SETTINGS_FILE}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, s.serialize())?;
    std::fs::rename(&tmp, settings_path(store_dir)).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

// ---------------------------------------------------------------------------------------------
// Plain local files

/// Open a regular local file for reading without following a link or materializing a dataless (cloud) file. The
/// opened descriptor is checked to be the object that was inspected.
pub(crate) fn open_plain_file(p: &Path) -> Option<std::fs::File> {
    let m = std::fs::symlink_metadata(p).ok()?;
    if !m.file_type().is_file() || m.st_flags() & SF_DATALESS != 0 {
        return None;
    }
    let _guard = NoMaterialize::enter().ok()?;
    let f = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(p).ok()?;
    let live = f.metadata().ok()?;
    (live.file_type().is_file() && live.dev() == m.dev() && live.ino() == m.ino() && live.st_flags() & SF_DATALESS == 0).then_some(f)
}

/// Up to `limit` bytes from the start of a plain local file (dataless materialization off while reading).
pub(crate) fn read_head(p: &Path, limit: u64) -> Option<Vec<u8>> {
    let f = open_plain_file(p)?;
    let _guard = NoMaterialize::enter().ok()?;
    let mut out = Vec::new();
    f.take(limit).read_to_end(&mut out).ok()?;
    Some(out)
}

// ---------------------------------------------------------------------------------------------
// Git

/// Whether Git can be asked, decided once per process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Git {
    /// The program to run.
    Available(PathBuf),
    /// Why Git is not run.
    Unavailable(String),
}

impl Git {
    /// `/usr/bin/git` when `xcode-select -p` names an existing developer directory; otherwise unavailable (the shim
    /// would only offer to install the Command Line Tools). `CM_PURGE_TEST_GIT=none` forces "unavailable".
    pub fn detect() -> Git {
        if std::env::var_os(TEST_GIT_VAR).is_some_and(|v| v == "none") {
            return Git::Unavailable(format!("Git is disabled by {TEST_GIT_VAR} (validation fixture)"));
        }
        let missing = "Git is not available: no Xcode or Command Line Tools developer directory (`xcode-select -p` failed), \
                       and /usr/bin/git would only offer to install them";
        match Tool::new(XCODE_SELECT).arg("-p").timeout(Duration::from_secs(10)).output_caps(4096, 4096).run() {
            Ok(out) if out.success() => {
                let dir = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                if !dir.is_empty() && Path::new(&dir).is_dir() { Git::Available(PathBuf::from(GIT)) } else { Git::Unavailable(missing.into()) }
            }
            Ok(_) => Git::Unavailable(missing.into()),
            Err(e) => Git::Unavailable(format!("Git is not available: {e}")),
        }
    }

    /// [`Git::detect`], once per process.
    pub fn cached() -> Git {
        static GIT_STATE: OnceLock<Git> = OnceLock::new();
        GIT_STATE.get_or_init(Git::detect).clone()
    }
}

fn first_line(b: &[u8]) -> String {
    String::from_utf8_lossy(b).lines().find(|l| !l.trim().is_empty()).unwrap_or("no message").trim().chars().take(300).collect()
}

/// NUL-separated entries `git ls-files` printed (a lower bound when the output was capped).
fn entries(out: &[u8]) -> usize {
    out.split(|b| *b == 0).filter(|e| !e.is_empty()).count()
}

/// `git ls-files -z <extra> -- <rel>` in `worktree`, with repository-configured helpers that would run programs
/// switched off and no optional lock taken. `Err` carries the reason the state is unknown.
fn ls_files(git: &Path, worktree: &Path, rel: &OsStr, extra: &[&str]) -> Result<usize, String> {
    let mut tool = Tool::new(git)
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(worktree)
        .args(["-c", "core.fsmonitor=false", "-c", "core.untrackedCache=false", "ls-files", "-z"])
        .args(extra)
        .arg("--")
        .arg(rel)
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .timeout(GIT_TIMEOUT)
        .output_caps(GIT_STDOUT_CAP, 16 << 10);
    // Never answer from a repository above this working copy when its own `.git` is not one Git accepts.
    if let Some(parent) = worktree.parent() {
        tool = tool.env("GIT_CEILING_DIRECTORIES", parent);
    }
    // The user's global excludes file decides what is ignored, as in their own Git.
    if let Some(h) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        tool = tool.env("HOME", h);
    }
    if let Some(t) = std::env::var_os("TMPDIR").filter(|t| !t.is_empty()) {
        tool = tool.env("TMPDIR", t);
    }
    let out = tool.run().map_err(|e| format!("git could not run: {e}"))?;
    if out.timed_out {
        return Err(format!("git ls-files timed out after {}s", GIT_TIMEOUT.as_secs()));
    }
    if !out.success() {
        return Err(format!("git ls-files failed: {}", first_line(&out.stderr)));
    }
    Ok(entries(&out.stdout))
}

/// Version-control state of `target` inside the Git working copy at `worktree`.
pub fn git_state(git: &Git, worktree: &Path, target: &Path) -> VcsState {
    let program = match git {
        Git::Available(p) => p,
        Git::Unavailable(why) => return VcsState::Unknown(why.clone()),
    };
    let Ok(rel) = target.strip_prefix(worktree) else { return VcsState::Unknown("path is outside its working copy".into()) };
    if rel.as_os_str().is_empty() {
        return VcsState::Unknown("the artifact is the working copy itself".into());
    }
    match ls_files(program, worktree, rel.as_os_str(), &[]) {
        Err(e) => return VcsState::Unknown(e),
        Ok(n) if n > 0 => return VcsState::Tracked(n as u64),
        Ok(_) => {}
    }
    match ls_files(program, worktree, rel.as_os_str(), &["--others", "--exclude-standard", "--directory", "--no-empty-directory"]) {
        Err(e) => VcsState::Unknown(e),
        Ok(0) => VcsState::Untracked,
        Ok(n) => VcsState::Unknown(format!(
            "{n} untracked item(s) under {} are not ignored by Git and may be uncommitted source; ignore the artifact (.gitignore) to purge it",
            rel.display()
        )),
    }
}

// ---------------------------------------------------------------------------------------------
// Evidence probe

/// Reads `CACHEDIR.TAG` heads and asks Git (verdicts cached per directory for one run).
pub struct FsProbe {
    git: Git,
    verdicts: RefCell<HashMap<PathBuf, VcsState>>,
}

impl Default for FsProbe {
    fn default() -> Self {
        FsProbe::with_git(Git::cached())
    }
}

impl FsProbe {
    pub fn with_git(git: Git) -> FsProbe {
        FsProbe { git, verdicts: RefCell::new(HashMap::new()) }
    }

    fn decide(&self, path: &Path) -> VcsState {
        let mut cur = path.parent();
        while let Some(anc) = cur {
            let dotgit = anc.join(".git");
            match std::fs::symlink_metadata(&dotgit) {
                Ok(m) if m.file_type().is_symlink() => return VcsState::Unknown(format!("{} is a link; not followed", dotgit.display())),
                Ok(_) => return git_state(&self.git, anc, path),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return VcsState::Unknown(format!("cannot inspect {}: {e}", dotgit.display())),
            }
            for other in [".hg", ".svn"] {
                if std::fs::symlink_metadata(anc.join(other)).is_ok() {
                    return VcsState::Unknown(format!("{other} working copy at {}: its tracking is not read", anc.display()));
                }
            }
            cur = anc.parent();
        }
        VcsState::NotVersioned
    }
}

impl PurgeProbe for FsProbe {
    fn cachedir_tag(&self, path: &RawPath) -> bool {
        os(path).and_then(|p| read_head(&p, CACHEDIR_SIGNATURE.len() as u64)).is_some_and(|h| h == CACHEDIR_SIGNATURE)
    }

    fn vcs(&self, dir: &RawPath) -> VcsState {
        let Some(path) = os(dir) else { return VcsState::Unknown("not a POSIX path".into()) };
        if let Some(v) = self.verdicts.borrow().get(&path) {
            return v.clone();
        }
        let v = self.decide(&path);
        self.verdicts.borrow_mut().insert(path, v.clone());
        v
    }
}

// ---------------------------------------------------------------------------------------------
// Pre-execution gate (PUR-03: version control checked again before acting)

/// Whether `plan` was built from the purge catalog.
pub fn is_purge_plan(plan: &Plan) -> bool {
    !plan.rule_refs.is_empty() && plan.rule_refs.iter().all(|r| r.starts_with(RULE_PREFIX))
}

/// For purge plans, a protection check that asks Git again per candidate (once per operation) and stops every item
/// of a candidate that is now tracked, holds unignored untracked files, or is undecidable. Other plans pass through.
pub fn vcs_gate(plan: &Plan) -> impl Fn(&ActionTarget<'_>) -> Result<(), String> + use<> {
    let purge = is_purge_plan(plan);
    let roots: HashMap<String, RawPath> = if purge {
        plan.body.items.iter().zip(&plan.item_meta).filter_map(|(i, m)| m.candidate_root.clone().map(|r| (i.action_id.clone(), r))).collect()
    } else {
        HashMap::new()
    };
    let probe = FsProbe::default();
    move |t: &ActionTarget<'_>| {
        if !purge {
            return Ok(());
        }
        let Some(root) = roots.get(t.action_id) else { return Err("vcs_unknown: the plan does not record this item's project artifact".into()) };
        match probe.vcs(root) {
            VcsState::NotVersioned | VcsState::Untracked => Ok(()),
            VcsState::Tracked(n) => Err(format!("vcs_tracked: {n} files under {} are tracked by Git now", root.display())),
            VcsState::Unknown(why) => Err(format!("vcs_unknown: {why}")),
        }
    }
}

#[cfg(test)]
mod tests;
