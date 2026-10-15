//! Windows side of project-artifact purge (PUR-01/03/05): default scan roots, the shared
//! settings file next to the store, read-only CACHEDIR.TAG and Git evidence, and the
//! pre-execution version-control gate. No process is started: Git state comes from reading
//! `<gitdir>/index` (see `cm_core::purge_git`); anything unreadable is "unknown", never "clean".

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Read;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use cm_core::cleanable::CACHEDIR_SIGNATURE;
use cm_core::execute::ActionTarget;
use cm_core::path::RawPath;
use cm_core::plan::Plan;
use cm_core::purge::{PurgeProbe, PurgeSettings, RULE_PREFIX, SETTINGS_FILE, SettingsError, VcsState};
use cm_core::purge_git::{HashKind, tracked_under};

const FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const ATTR_REPARSE_POINT: u32 = 0x400;
const ATTR_OFFLINE: u32 = 0x1000;
const ATTR_RECALL_ON_OPEN: u32 = 0x0004_0000;
const ATTR_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
/// Common project folder names under the profile and at the root of fixed drives.
const PROFILE_DIRS: &[&str] = &["source\\repos", "Projects", "dev", "code", "src", "repos", "workspace", "GitHub", "Development", "www"];
const DRIVE_DIRS: &[&str] = &["Projects", "dev", "code", "src", "repos", "workspace", "workplace", "GitHub"];

pub fn raw(p: &Path) -> Option<RawPath> {
    RawPath::windows(p.as_os_str().encode_wide().collect()).ok()
}

pub fn os(p: &RawPath) -> Option<PathBuf> {
    match p {
        RawPath::Windows(u) => Some(PathBuf::from(OsString::from_wide(u))),
        RawPath::Unix(_) => None,
    }
}

fn plain_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && m.file_attributes() & ATTR_REPARSE_POINT == 0)
}

/// Default project roots that exist on this machine (plain directories, never links).
pub fn default_roots() -> Vec<RawPath> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        out.extend(PROFILE_DIRS.iter().map(|d| home.join(d)));
    }
    // SAFETY: no arguments; returns a bit mask of logical drives.
    let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
    for (i, letter) in ('A'..='Z').enumerate() {
        if mask & (1 << i) == 0 {
            continue;
        }
        let root: Vec<u16> = format!("{letter}:\\").encode_utf16().chain([0]).collect();
        // SAFETY: NUL-terminated root path.
        if unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(root.as_ptr()) } != 3 {
            continue; // DRIVE_FIXED only: never wake removable or network volumes
        }
        out.extend(DRIVE_DIRS.iter().map(|d| PathBuf::from(format!("{letter}:\\{d}"))));
    }
    let mut seen: Vec<String> = Vec::new();
    out.into_iter()
        .filter(|p| plain_dir(p))
        .filter(|p| {
            let key = p.to_string_lossy().to_lowercase();
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
// Evidence probe

/// Open a regular local file for reading without following links or recalling cloud content.
pub fn open_plain_file(p: &Path) -> Option<std::fs::File> {
    let m = std::fs::symlink_metadata(p).ok()?;
    let a = m.file_attributes();
    if !m.is_file() || a & (ATTR_REPARSE_POINT | ATTR_OFFLINE | ATTR_RECALL_ON_OPEN | ATTR_RECALL_ON_DATA_ACCESS) != 0 {
        return None;
    }
    std::fs::OpenOptions::new().read(true).custom_flags(FLAG_OPEN_REPARSE_POINT).open(p).ok()
}

/// A loaded Git index (empty bytes: no index file) or why it could not be read.
type LoadedIndex = Rc<Result<(Vec<u8>, HashKind), String>>;

/// Reads `CACHEDIR.TAG` heads and Git indexes (cached per git directory for one run).
#[derive(Default)]
pub struct FsProbe {
    indexes: RefCell<HashMap<PathBuf, LoadedIndex>>,
}

impl FsProbe {
    fn index(&self, gitdir: &Path) -> LoadedIndex {
        if let Some(i) = self.indexes.borrow().get(gitdir) {
            return Rc::clone(i);
        }
        let loaded = Rc::new(read_index(gitdir));
        self.indexes.borrow_mut().insert(gitdir.to_path_buf(), Rc::clone(&loaded));
        loaded
    }

    fn git(&self, worktree: &Path, dotgit: &Path, target: &Path) -> VcsState {
        let gitdir = match git_dir(worktree, dotgit) {
            Ok(g) => g,
            Err(e) => return VcsState::Unknown(e),
        };
        let rel = match target.strip_prefix(worktree) {
            Ok(r) => r.components().map(|c| c.as_os_str().to_str()).collect::<Option<Vec<_>>>().map(|v| v.join("/")),
            Err(_) => None,
        };
        let Some(rel) = rel else { return VcsState::Unknown("path is not valid Unicode inside the working copy".into()) };
        match self.index(&gitdir).as_ref() {
            Ok((bytes, hash)) if bytes.is_empty() => {
                let _ = hash;
                VcsState::Untracked
            }
            Ok((bytes, hash)) => match tracked_under(bytes, &rel, *hash, true) {
                Ok(0) => VcsState::Untracked,
                Ok(n) => VcsState::Tracked(n),
                Err(e) => VcsState::Unknown(format!("git index: {e}")),
            },
            Err(e) => VcsState::Unknown(e.clone()),
        }
    }
}

/// `.git` directory, or the directory a `.git` file (worktree/submodule) points to.
fn git_dir(worktree: &Path, dotgit: &Path) -> Result<PathBuf, String> {
    let m = std::fs::symlink_metadata(dotgit).map_err(|e| format!("cannot inspect .git: {e}"))?;
    if m.file_attributes() & ATTR_REPARSE_POINT != 0 {
        return Err(".git is a link; not followed".into());
    }
    if m.is_dir() {
        return Ok(dotgit.to_path_buf());
    }
    let mut text = String::new();
    open_plain_file(dotgit).ok_or(".git file is unreadable")?.take(4096).read_to_string(&mut text).map_err(|e| format!("cannot read .git file: {e}"))?;
    let target = text.lines().find_map(|l| l.strip_prefix("gitdir:")).map(str::trim).filter(|t| !t.is_empty()).ok_or(".git file has no gitdir line")?;
    let p = PathBuf::from(target.replace('/', "\\"));
    Ok(if p.is_absolute() { p } else { worktree.join(p) })
}

fn read_index(gitdir: &Path) -> Result<(Vec<u8>, HashKind), String> {
    // Linked worktrees keep their config in the common directory.
    let common = match std::fs::read_to_string(gitdir.join("commondir")) {
        Ok(c) => {
            let p = PathBuf::from(c.trim().replace('/', "\\"));
            if p.is_absolute() { p } else { gitdir.join(p) }
        }
        Err(_) => gitdir.to_path_buf(),
    };
    let config = std::fs::read_to_string(common.join("config")).unwrap_or_default().to_ascii_lowercase();
    let sha256 = config.lines().any(|l| {
        let l: String = l.chars().filter(|c| !c.is_whitespace()).collect();
        l == "objectformat=sha256"
    });
    let hash = if sha256 { HashKind::Sha256 } else { HashKind::Sha1 };
    let index = gitdir.join("index");
    match std::fs::symlink_metadata(&index) {
        // A repository with nothing staged has no index: nothing is tracked.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), hash)),
        Err(e) => return Err(format!("cannot inspect the git index: {e}")),
        Ok(_) => {}
    }
    let mut f = open_plain_file(&index).ok_or("the git index is not a readable local file")?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes).map_err(|e| format!("cannot read the git index: {e}"))?;
    Ok((bytes, hash))
}

impl PurgeProbe for FsProbe {
    fn cachedir_tag(&self, path: &RawPath) -> bool {
        let Some(mut f) = os(path).as_deref().and_then(open_plain_file) else { return false };
        let mut head = [0u8; 43];
        f.read_exact(&mut head).is_ok() && head == CACHEDIR_SIGNATURE
    }

    fn vcs(&self, dir: &RawPath) -> VcsState {
        let Some(path) = os(dir) else { return VcsState::Unknown("not a Windows path".into()) };
        let mut cur = path.parent();
        while let Some(anc) = cur {
            let dotgit = anc.join(".git");
            match std::fs::symlink_metadata(&dotgit) {
                Ok(_) => return self.git(anc, &dotgit, &path),
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

// ---------------------------------------------------------------------------------------------
// Pre-execution gate (PUR-03: version control checked again before acting)

/// For purge plans, a protection check that re-reads version-control state per candidate (once
/// per operation) and stops every item of a candidate that is now tracked or undecidable.
/// Other plans pass through unchanged.
pub fn vcs_gate(plan: &Plan) -> impl Fn(&ActionTarget<'_>) -> Result<(), String> + use<> {
    let purge = !plan.rule_refs.is_empty() && plan.rule_refs.iter().all(|r| r.starts_with(RULE_PREFIX));
    let roots: HashMap<String, RawPath> = if purge {
        plan.body.items.iter().zip(&plan.item_meta).filter_map(|(i, m)| m.candidate_root.clone().map(|r| (i.action_id.clone(), r))).collect()
    } else {
        HashMap::new()
    };
    let probe = FsProbe::default();
    let verdicts: RefCell<HashMap<RawPath, Result<(), String>>> = RefCell::new(HashMap::new());
    move |t: &ActionTarget<'_>| {
        if !purge {
            return Ok(());
        }
        let Some(root) = roots.get(t.action_id) else { return Err("vcs_unknown: the plan does not record this item's project artifact".into()) };
        if let Some(v) = verdicts.borrow().get(root) {
            return v.clone();
        }
        let v = match probe.vcs(root) {
            VcsState::NotVersioned | VcsState::Untracked => Ok(()),
            VcsState::Tracked(n) => Err(format!("vcs_tracked: {n} files under {} are tracked by Git now", root.display())),
            VcsState::Unknown(why) => Err(format!("vcs_unknown: {why}")),
        };
        verdicts.borrow_mut().insert(root.clone(), v.clone());
        v
    }
}

#[cfg(test)]
mod tests;
