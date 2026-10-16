//! Tri-state in-use probes for catalog candidates (CAT-06) on macOS.
//!
//! Every probe answers `in_use`, `not_in_use` or `unknown`, and `unknown` is never read as "not in use". Probes only
//! list processes (`procs::snapshot`), stat entries without following links and read which vnodes processes hold open
//! (`procs::FdIndex`); no file is opened, read or changed, so nothing is materialized from a cloud provider. They run at
//! preview/plan time; the executor checks each item again before acting ([`execution_check`], [`owner_running_check`]).
//!
//! - `owner_process`: the owner's application bundle (`Google Chrome.app`...) appears in a running process's
//!   executable path (helpers inside the bundle included), or one of the owner's command-line tools runs.
//! - `open_file_handle` / `open_database_handle` / `incomplete_download_handle`: a process holds an object of the
//!   candidate open. Descriptors of other users' and protected processes are not readable without privileges; while any
//!   process could not be inspected, "nobody holds it" is not a fact and the answer is `unknown`.
//! - `endpoint_security_path`: the path is inside an endpoint-security or device-management product's data.
//! - `elevated_token`: this build has no elevation path on macOS (platform doc §11); only a process already running as
//!   root answers `not_in_use`.

use std::cell::OnceCell;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cm_core::capability::CapabilityState;
use cm_core::catalog::{ProbeResult, ProbeState};
use cm_core::execute::{ActionTarget, LiveCheck};
use cm_core::path::RawPath;

use super::procs::{self, FdIndex};
use super::sys::describe;

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`. The catalog
/// capability these probes serve is declared once, by `resolve`.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[];

/// Entries examined per candidate and time budget; beyond either the answer is `unknown`.
pub const MAX_PROBE_FILES: usize = 20_000;
pub const PROBE_BUDGET: Duration = Duration::from_secs(10);

/// How an owner's processes are recognized: its application bundles (a path component of the executable) and its
/// command-line tools (the executable's file name).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerMatch {
    pub bundles: &'static [&'static str],
    pub executables: &'static [&'static str],
}

const fn app(bundles: &'static [&'static str]) -> OwnerMatch {
    OwnerMatch { bundles, executables: &[] }
}

const fn tool(executables: &'static [&'static str]) -> OwnerMatch {
    OwnerMatch { bundles: &[], executables }
}

/// Owner id → how its running processes are recognized. Interpreters that run many tools (`node`, `python`, `java`)
/// are never an owner's process.
const OWNERS: &[(&str, OwnerMatch)] = &[
    ("google.chrome", app(&["Google Chrome.app"])),
    ("microsoft.edge", app(&["Microsoft Edge.app"])),
    ("mozilla.firefox", app(&["Firefox.app"])),
    ("apple.safari", app(&["Safari.app"])),
    ("microsoft.vscode", app(&["Visual Studio Code.app"])),
    ("anysphere.cursor", app(&["Cursor.app"])),
    ("anthropic.claude", app(&["Claude.app"])),
    ("openai.codex", app(&["Codex.app"])),
    ("apple.xcode", app(&["Xcode.app"])),
    ("cargo", tool(&["cargo"])),
    ("go", tool(&["go"])),
    ("pnpm", tool(&["pnpm"])),
    ("uv", tool(&["uv"])),
];

pub fn owner_match(owner: &str) -> Option<&'static OwnerMatch> {
    OWNERS.iter().find(|(id, _)| *id == owner).map(|(_, m)| m)
}

/// Directory names of endpoint-security / device-management products whose data is never a cleanup target (PRT-03
/// complement; compared lower-case, per path component).
const ENDPOINT_SECURITY: &[&str] = &[
    "crowdstrike", "com.crowdstrike.falcon", "sentinelone", "sentinel", "com.sentinelone", "jamf", "jamfprotect", "com.jamf.protect",
    "kandji", "io.kandji", "microsoft defender", "com.microsoft.wdav", "sophos", "com.sophos", "carbon black", "carbonblack",
    "com.vmware.carbonblack", "tanium", "qualys", "malwarebytes", "bitdefender", "eset", "kaspersky lab", "trend micro", "cylance",
    "palo alto networks", "cortex xdr", "com.paloaltonetworks", "intune", "com.microsoft.intune", "mosyle", "addigy", "fleetsmith",
];

/// Extensions of in-progress downloads (Chromium, Firefox, generic, Safari's `.download` bundles).
const DOWNLOAD_MARKERS: &[&str] = &["crdownload", "part", "partial", "download", "opdownload"];
const DB_EXT: &[&str] = &["db", "sqlite", "sqlite3", "db-wal", "db-shm", "sqlite-wal", "sqlite-shm", "db-journal", "ldb", "log", "mdb", "accdb"];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Proc {
    pid: i32,
    name: String,
    path: Option<Vec<u8>>,
}

/// One process listing shared by every probe of a preview; the open-file index is built on first use, once.
#[derive(Debug)]
pub struct ProcessSnapshot {
    procs: Result<Vec<Proc>, String>,
    fds: OnceCell<Result<FdIndex, String>>,
}

impl ProcessSnapshot {
    pub fn take() -> Self {
        let procs = match procs::snapshot() {
            Ok(s) if s.procs.is_empty() => Err("process snapshot returned no processes".to_owned()),
            Ok(s) => Ok(s.procs.into_iter().map(|p| Proc { pid: p.pid, name: p.name, path: p.path }).collect()),
            Err(e) => Err(format!("process snapshot failed: {}", describe(e))),
        };
        ProcessSnapshot { procs, fds: OnceCell::new() }
    }

    /// For tests: a fixed listing of `(name, executable path)` or a failed enumeration.
    pub fn fixed(procs: Result<&[(&str, Option<&str>)], &str>) -> Self {
        let procs = procs
            .map(|l| l.iter().enumerate().map(|(i, (n, p))| Proc { pid: 1000 + i as i32, name: (*n).to_owned(), path: p.map(|p| p.as_bytes().to_vec()) }).collect())
            .map_err(str::to_owned);
        ProcessSnapshot { procs, fds: OnceCell::new() }
    }

    fn fd_index(&self) -> &Result<FdIndex, String> {
        self.fds.get_or_init(|| FdIndex::build().map_err(|e| format!("open-file listing failed: {}", describe(e))))
    }
}

fn eq_ci(a: &[u8], b: &str) -> bool {
    a.eq_ignore_ascii_case(b.as_bytes())
}

/// The process belongs to `m`: a bundle is a component of its executable path, or its executable is one of the tools.
/// Without a readable path, the process name is compared with the bundle name (`Google Chrome`) and the tools.
fn belongs(p: &Proc, m: &OwnerMatch) -> bool {
    match &p.path {
        Some(path) => {
            let mut comps = path.split(|&b| b == b'/').filter(|c| !c.is_empty()).peekable();
            let mut last: &[u8] = &[];
            let mut in_bundle = false;
            while let Some(c) = comps.next() {
                in_bundle |= m.bundles.iter().any(|b| eq_ci(c, b));
                if comps.peek().is_none() {
                    last = c;
                }
            }
            in_bundle || m.executables.iter().any(|e| eq_ci(last, e))
        }
        None => m.bundles.iter().any(|b| eq_ci(p.name.as_bytes(), b.trim_end_matches(".app"))) || m.executables.iter().any(|e| eq_ci(p.name.as_bytes(), e)),
    }
}

/// `owner_process` against an explicit matcher (the owner table, or a test fixture).
pub fn probe_match(m: &OwnerMatch, snap: &ProcessSnapshot) -> ProbeResult {
    const P: &str = "owner_process";
    match &snap.procs {
        Err(e) => ProbeResult::new(P, ProbeState::Unknown, e.clone()),
        Ok(procs) => match procs.iter().find(|p| belongs(p, m)) {
            Some(p) => {
                let what = p.path.as_deref().map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_else(|| p.name.clone());
                ProbeResult::new(P, ProbeState::InUse, format!("{what} (pid {}) is running", p.pid))
            }
            None => {
                let names: Vec<&str> = m.bundles.iter().chain(m.executables).copied().collect();
                ProbeResult::new(P, ProbeState::NotInUse, format!("{} not running", names.join(", ")))
            }
        },
    }
}

/// `owner_process`: in_use if any of the owner's processes runs; unknown if the owner has no mapping or the listing
/// failed.
pub fn probe_owner(owner: Option<&str>, snap: &ProcessSnapshot) -> ProbeResult {
    const P: &str = "owner_process";
    let Some(owner) = owner else { return ProbeResult::new(P, ProbeState::Unknown, "rule has no owner") };
    match owner_match(owner) {
        Some(m) => probe_match(m, snap),
        None => ProbeResult::new(P, ProbeState::Unknown, format!("no process mapping for owner {owner}")),
    }
}

/// `elevated_token`: administrator-only locations are attempted only by a process already running as root; this build
/// never elevates itself on macOS.
pub fn probe_elevation(is_root: bool) -> ProbeResult {
    const P: &str = "elevated_token";
    if is_root {
        ProbeResult::new(P, ProbeState::NotInUse, "this process runs as root")
    } else {
        ProbeResult::new(P, ProbeState::Unknown, "administrator rights required: this build has no elevation path on macOS; skipped")
    }
}

fn is_root() -> bool {
    // SAFETY: plain query of this process's effective user.
    unsafe { libc::geteuid() == 0 }
}

/// One entry at or under a candidate: its path and `(st_dev, st_ino)`.
type Seen = (PathBuf, i32, u64);

/// Entries at or under `root` (directories included, links not followed), bounded by count and time.
fn walk(root: &Path, mut keep: impl FnMut(&Path, bool) -> bool) -> Result<Vec<Seen>, String> {
    let started = Instant::now();
    let meta = std::fs::symlink_metadata(root).map_err(|e| format!("{}: {e}", root.display()))?;
    if meta.file_type().is_symlink() {
        return Err(format!("{} is a link", root.display()));
    }
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), meta)];
    let mut seen = 0usize;
    while let Some((p, m)) = stack.pop() {
        if m.file_type().is_symlink() {
            continue; // links below are boundaries; analysis already blocks candidates containing them
        }
        seen += 1;
        if seen > MAX_PROBE_FILES || started.elapsed() > PROBE_BUDGET {
            return Err(format!("more than {MAX_PROBE_FILES} entries or {}s; not every entry could be probed", PROBE_BUDGET.as_secs()));
        }
        if keep(&p, m.is_dir()) {
            out.push((p.clone(), m.dev() as i32, m.ino()));
        }
        if m.is_dir() {
            let rd = std::fs::read_dir(&p).map_err(|e| format!("cannot list {}: {e}", p.display()))?;
            for e in rd {
                let e = e.map_err(|e| format!("cannot list {}: {e}", p.display()))?;
                // DirEntry::metadata does not follow a symlink.
                let m = e.metadata().map_err(|err| format!("{}: {err}", e.path().display()))?;
                stack.push((e.path(), m));
            }
        }
    }
    Ok(out)
}

fn ext_in(p: &Path, set: &[&str]) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| set.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Whether any listed object is held open; `unknown` when one could be held by a process whose descriptors are not
/// readable.
fn held(name: &str, objects: &[Seen], snap: &ProcessSnapshot, none_detail: String) -> ProbeResult {
    let index = match snap.fd_index() {
        Ok(i) => i,
        Err(e) => return ProbeResult::new(name, ProbeState::Unknown, e.clone()),
    };
    for (p, dev, ino) in objects {
        let report = index.holders(*dev, *ino);
        if let Some(h) = report.holders.first() {
            return ProbeResult::new(name, ProbeState::InUse, format!("{} is open in {} (pid {})", p.display(), h.name, h.pid));
        }
    }
    if !objects.is_empty() && !index.uninspected.is_empty() {
        return ProbeResult::new(
            name,
            ProbeState::Unknown,
            format!("{} process(es) could not be inspected (other users' or protected processes); a holder among them would be invisible", index.uninspected.len()),
        );
    }
    ProbeResult::new(name, ProbeState::NotInUse, none_detail)
}

/// `open_file_handle` (every entry) or `open_database_handle` (database/journal/log files only).
pub fn probe_open_files(root: &Path, databases_only: bool, snap: &ProcessSnapshot) -> ProbeResult {
    let name = if databases_only { "open_database_handle" } else { "open_file_handle" };
    match walk(root, |p, dir| if databases_only { !dir && ext_in(p, DB_EXT) } else { true }) {
        Ok(objects) => {
            let n = objects.len();
            held(name, &objects, snap, format!("{n} entr(ies) not open elsewhere"))
        }
        Err(e) => ProbeResult::new(name, ProbeState::Unknown, e),
    }
}

/// Whether `p` (relative to the candidate's parent) is a download marker or inside one (Safari `.download` bundles).
fn in_marker(root: &Path, p: &Path) -> bool {
    let base = root.parent().unwrap_or(root);
    p.strip_prefix(base).unwrap_or(p).ancestors().any(|a| !a.as_os_str().is_empty() && ext_in(a, DOWNLOAD_MARKERS))
}

/// `incomplete_download_handle`: an in-progress download marker (or anything inside a marker bundle) that a process
/// holds open.
pub fn probe_downloads(root: &Path, snap: &ProcessSnapshot) -> ProbeResult {
    const P: &str = "incomplete_download_handle";
    match walk(root, |p, _| in_marker(root, p)) {
        Ok(objects) => {
            let n = objects.len();
            held(P, &objects, snap, format!("{n} download marker entr(ies), none held open"))
        }
        Err(e) => ProbeResult::new(P, ProbeState::Unknown, e),
    }
}

/// `endpoint_security_path`: the path is inside an endpoint-security or management product's data.
pub fn probe_endpoint(path: &Path) -> ProbeResult {
    const P: &str = "endpoint_security_path";
    let hit = path.components().filter_map(|c| c.as_os_str().to_str()).map(str::to_ascii_lowercase).find(|c| ENDPOINT_SECURITY.contains(&c.as_str()));
    match hit {
        Some(c) => ProbeResult::new(P, ProbeState::InUse, format!("inside endpoint-security data ({c})")),
        None => ProbeResult::new(P, ProbeState::NotInUse, "not an endpoint-security location"),
    }
}

fn os_path(p: &RawPath) -> Option<PathBuf> {
    match p {
        RawPath::Unix(b) => Some(PathBuf::from(std::ffi::OsStr::from_bytes(b))),
        RawPath::Windows(_) => None,
    }
}

/// Run every probe a candidate's rules declared. Kinds this platform cannot answer are `unknown`.
pub fn probe_candidate(path: &RawPath, probes: &[String], owner: Option<&str>, snap: &ProcessSnapshot) -> Vec<ProbeResult> {
    let Some(p) = os_path(path) else { return probes.iter().map(|k| ProbeResult::new(k, ProbeState::Unknown, "not a macOS path")).collect() };
    let mut kinds: Vec<&str> = probes.iter().map(String::as_str).collect();
    kinds.sort_unstable();
    kinds.dedup();
    kinds
        .into_iter()
        .map(|k| match k {
            "owner_process" => probe_owner(owner, snap),
            "open_file_handle" => probe_open_files(&p, false, snap),
            "open_database_handle" => probe_open_files(&p, true, snap),
            "incomplete_download_handle" => probe_downloads(&p, snap),
            "endpoint_security_path" => probe_endpoint(&p),
            "elevated_token" => probe_elevation(is_root()),
            other => ProbeResult::new(other, ProbeState::Unknown, "probe not available on macOS in this build"),
        })
        .collect()
}

/// Execution-time re-check for the catalog probe kinds an executor's own check does not cover. `None` means nothing
/// to add.
pub fn execution_check(t: &ActionTarget<'_>) -> Option<LiveCheck> {
    let path = os_path(t.path)?;
    for p in t.probes {
        let r = match p.as_str() {
            "endpoint_security_path" => probe_endpoint(&path),
            "elevated_token" => probe_elevation(is_root()),
            "incomplete_download_handle" => probe_downloads(&path, &ProcessSnapshot::take()),
            _ => continue,
        };
        match (p.as_str(), r.state) {
            (_, ProbeState::NotInUse) => {}
            ("endpoint_security_path", _) => return Some(LiveCheck::Protected(r.detail)),
            (_, ProbeState::InUse) => return Some(LiveCheck::InUse),
            (_, ProbeState::Unknown) => return Some(LiveCheck::Unknown(r.detail)),
        }
    }
    None
}

/// Owner-process re-check for an executor.
pub fn owner_running_check(owner: &str) -> Option<LiveCheck> {
    let r = probe_owner(Some(owner), &ProcessSnapshot::take());
    match r.state {
        ProbeState::InUse => Some(LiveCheck::OwnerRunning),
        ProbeState::Unknown => Some(LiveCheck::Unknown(r.detail)),
        ProbeState::NotInUse => None,
    }
}

#[cfg(test)]
mod tests;
