//! Safe external tool runner for the macOS backends (codesign, launchctl, pkgutil, ...).
//!
//! - The program is an absolute path, resolved once (symlinks included) and executed by that
//!   resolved path. The binary must be a root-owned regular file that only root can write, inside
//!   directories that are root-owned and neither group- nor world-writable all the way up; a
//!   less privileged user could otherwise swap what runs. Only an explicit test override relaxes
//!   this (`cfg(test)`, or a set `CM_*_TEST_*` redirection variable named by the caller).
//! - User-installed package managers (Homebrew: `/opt/homebrew` belongs to the user who installed
//!   it, not root, by Homebrew's design) opt in to [`Tool::owned_by_user_or_root`]: the binary and
//!   every directory above it must be owned by root or by this process's user, and writable by
//!   nobody else, except that group write is accepted for the `admin` group (gid 80) and `wheel`
//!   (gid 0), which Homebrew's installer grants and whose members can already act as root through
//!   `sudo`. Running such a tool is then exactly as trusted as running the user's own files.
//! - A fixed argument vector, no shell. The environment is cleared and rebuilt as `LANG=C`,
//!   `LC_ALL=C`, a minimal system `PATH` plus what the caller adds explicitly; stdin is
//!   `/dev/null`, the working directory `/` unless set.
//! - The tool runs in its own process group. On timeout the group gets `SIGTERM`, then `SIGKILL`;
//!   when the tool ends, whatever it left running in its group is killed too, so pipes close and
//!   nothing outlives the call (the group id cannot be reused before the leader is reaped).
//! - Standard output and error are captured up to a byte cap each; the rest is counted and
//!   dropped, never buffered without bound and never left to block the child.

// Consumers (signing checks, launchctl/pkgutil adapters) land with later macOS work packages.
#![allow(dead_code)]

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::sys::errno;

/// `PATH` for every tool: system binaries only.
pub(crate) const MINIMAL_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_CAP: usize = 1 << 20;
/// Time a tool gets to exit after `SIGTERM` before `SIGKILL`.
const TERM_GRACE: Duration = Duration::from_millis(500);
/// Time the output pipes get to close once the group is gone (a process that left the group may hold them).
const PIPE_GRACE: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Trust {
    /// Root-owned binary in root-only directories (the only production policy).
    RootOnly,
    /// Owned by root or the current user, writable only by them (and the admin/wheel group).
    OwnerOrRoot,
    /// Ownership checks skipped while the named `CM_*_TEST_*` variable is set.
    WhenTestVar(&'static str),
    #[cfg(test)]
    Test,
}

/// One external tool invocation.
#[derive(Debug, Clone)]
pub(crate) struct Tool {
    program: PathBuf,
    args: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
    cwd: PathBuf,
    timeout: Duration,
    stdout_cap: usize,
    stderr_cap: usize,
    trust: Trust,
}

/// What a finished (or timed-out) tool produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolOutput {
    /// Exit code; `None` when ended by a signal.
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// More output was produced than kept (or a pipe was still open after the grace period).
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    /// The deadline passed and the process group was ended.
    pub timed_out: bool,
    pub duration: Duration,
}

impl ToolOutput {
    pub(crate) fn success(&self) -> bool {
        !self.timed_out && self.exit_code == Some(0)
    }

    pub(crate) fn truncated(&self) -> bool {
        self.stdout_truncated || self.stderr_truncated
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolError {
    /// The program is not an absolute path.
    NotAbsolute(PathBuf),
    /// The binary or a directory above it could be replaced by a less privileged user.
    Untrusted(String),
    /// The process could not be started or waited for.
    Spawn(String),
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolError::NotAbsolute(p) => write!(f, "tool path is not absolute: {}", p.display()),
            ToolError::Untrusted(m) => write!(f, "tool refused: {m}"),
            ToolError::Spawn(m) => write!(f, "tool could not run: {m}"),
        }
    }
}

impl std::error::Error for ToolError {}

/// Resolve `program` and check that only root can change what it names. Returns the resolved path.
pub(crate) fn trusted_program(program: &Path) -> Result<PathBuf, ToolError> {
    if !program.is_absolute() {
        return Err(ToolError::NotAbsolute(program.to_path_buf()));
    }
    let real = std::fs::canonicalize(program).map_err(|e| ToolError::Untrusted(format!("{} cannot be resolved: {e}", program.display())))?;
    let meta = std::fs::symlink_metadata(&real).map_err(|e| ToolError::Untrusted(format!("{}: {e}", real.display())))?;
    if !meta.file_type().is_file() {
        return Err(ToolError::Untrusted(format!("{} is not a regular file", real.display())));
    }
    if meta.permissions().mode() & 0o111 == 0 {
        return Err(ToolError::Untrusted(format!("{} is not executable", real.display())));
    }
    root_only(&real, &meta)?;
    for dir in real.ancestors().skip(1) {
        let m = std::fs::symlink_metadata(dir).map_err(|e| ToolError::Untrusted(format!("{}: {e}", dir.display())))?;
        root_only(dir, &m)?;
    }
    Ok(real)
}

/// `admin` and `wheel`: groups whose members can already become root (`sudo`).
const PRIVILEGED_GROUPS: [u32; 2] = [80, 0];

/// [`trusted_program`] for a tool the current user installed: the binary and its directories must
/// be owned by root or `uid` and not writable by anyone else (group write only for admin/wheel).
pub(crate) fn trusted_user_program(program: &Path, uid: u32) -> Result<PathBuf, ToolError> {
    if !program.is_absolute() {
        return Err(ToolError::NotAbsolute(program.to_path_buf()));
    }
    let real = std::fs::canonicalize(program).map_err(|e| ToolError::Untrusted(format!("{} cannot be resolved: {e}", program.display())))?;
    let meta = std::fs::symlink_metadata(&real).map_err(|e| ToolError::Untrusted(format!("{}: {e}", real.display())))?;
    if !meta.file_type().is_file() {
        return Err(ToolError::Untrusted(format!("{} is not a regular file", real.display())));
    }
    if meta.permissions().mode() & 0o111 == 0 {
        return Err(ToolError::Untrusted(format!("{} is not executable", real.display())));
    }
    owner_or_root(&real, &meta, uid)?;
    for dir in real.ancestors().skip(1) {
        let m = std::fs::symlink_metadata(dir).map_err(|e| ToolError::Untrusted(format!("{}: {e}", dir.display())))?;
        owner_or_root(dir, &m, uid)?;
    }
    Ok(real)
}

fn owner_or_root(path: &Path, meta: &std::fs::Metadata, uid: u32) -> Result<(), ToolError> {
    if meta.uid() != 0 && meta.uid() != uid {
        return Err(ToolError::Untrusted(format!("{} is owned by uid {}, not root or the current user (uid {uid})", path.display(), meta.uid())));
    }
    let mode = meta.mode();
    if mode & 0o002 != 0 || (mode & 0o020 != 0 && !PRIVILEGED_GROUPS.contains(&meta.gid())) {
        return Err(ToolError::Untrusted(format!("{} is writable by others (mode {:o}, group {})", path.display(), mode & 0o7777, meta.gid())));
    }
    Ok(())
}

fn root_only(path: &Path, meta: &std::fs::Metadata) -> Result<(), ToolError> {
    if meta.uid() != 0 {
        return Err(ToolError::Untrusted(format!("{} is owned by uid {}, not root", path.display(), meta.uid())));
    }
    if meta.mode() & 0o022 != 0 {
        return Err(ToolError::Untrusted(format!("{} is writable by group or others (mode {:o})", path.display(), meta.mode() & 0o7777)));
    }
    Ok(())
}

/// Bytes read from one pipe: the first `cap` kept, the rest counted.
#[derive(Default)]
struct Captured {
    kept: Vec<u8>,
    total: usize,
    closed: bool,
}

struct Capture {
    shared: Arc<Mutex<Captured>>,
}

impl Capture {
    fn start(pipe: Option<impl Read + Send + 'static>, cap: usize) -> Capture {
        let shared = Arc::new(Mutex::new(Captured::default()));
        let Some(mut pipe) = pipe else {
            shared.lock().unwrap_or_else(|p| p.into_inner()).closed = true;
            return Capture { shared };
        };
        let sink = Arc::clone(&shared);
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                let n = match pipe.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let mut c = sink.lock().unwrap_or_else(|p| p.into_inner());
                c.total += n;
                let room = cap.saturating_sub(c.kept.len());
                c.kept.extend_from_slice(&buf[..n.min(room)]);
            }
            sink.lock().unwrap_or_else(|p| p.into_inner()).closed = true;
        });
        Capture { shared }
    }

    fn closed(&self) -> bool {
        self.shared.lock().unwrap_or_else(|p| p.into_inner()).closed
    }

    /// `(kept bytes, truncated)`; a pipe still open counts as truncated.
    fn take(&self) -> (Vec<u8>, bool) {
        let mut c = self.shared.lock().unwrap_or_else(|p| p.into_inner());
        let truncated = !c.closed || c.total > c.kept.len();
        (std::mem::take(&mut c.kept), truncated)
    }
}

/// Whether `pid` (our child) has exited, without reaping it: the zombie keeps its pid and
/// process group id reserved until [`std::process::Child::wait`].
fn exited_unreaped(pid: libc::pid_t) -> Result<bool, i32> {
    // SAFETY: zeroed POD out-parameter (WNOHANG may return without filling it).
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // SAFETY: our own child pid and a valid out-pointer; WNOWAIT leaves the child waitable.
    let rc = unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT) };
    if rc != 0 {
        let e = errno();
        return if e == libc::EINTR { Ok(false) } else { Err(e) };
    }
    Ok(info.si_pid == pid)
}

fn signal_group(pgid: libc::pid_t, sig: libc::c_int) {
    // SAFETY: the group is led by our unreaped child, so `pgid` still names that group.
    unsafe { libc::killpg(pgid, sig) };
}

impl Tool {
    pub(crate) fn new(program: impl Into<PathBuf>) -> Tool {
        Tool {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: PathBuf::from("/"),
            timeout: DEFAULT_TIMEOUT,
            stdout_cap: DEFAULT_CAP,
            stderr_cap: DEFAULT_CAP,
            trust: Trust::RootOnly,
        }
    }

    pub(crate) fn arg(mut self, a: impl AsRef<OsStr>) -> Tool {
        self.args.push(a.as_ref().to_owned());
        self
    }

    pub(crate) fn args<I, S>(mut self, args: I) -> Tool
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args.extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        self
    }

    /// An environment variable on top of the fixed minimal environment.
    pub(crate) fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Tool {
        self.env.push((key.as_ref().to_owned(), value.as_ref().to_owned()));
        self
    }

    pub(crate) fn cwd(mut self, dir: impl Into<PathBuf>) -> Tool {
        self.cwd = dir.into();
        self
    }

    pub(crate) fn timeout(mut self, timeout: Duration) -> Tool {
        self.timeout = timeout;
        self
    }

    pub(crate) fn output_caps(mut self, stdout: usize, stderr: usize) -> Tool {
        self.stdout_cap = stdout;
        self.stderr_cap = stderr;
        self
    }

    /// Accept a binary owned by the current user (a user-installed package manager such as
    /// Homebrew) under the [`trusted_user_program`] rules instead of root-only ownership.
    pub(crate) fn owned_by_user_or_root(mut self) -> Tool {
        self.trust = Trust::OwnerOrRoot;
        self
    }

    /// Skip the ownership checks while `var` is set to a non-empty value. `var` must be a
    /// `CM_*_TEST_*` redirection that placed a stand-in tool; any other name is ignored.
    pub(crate) fn untrusted_when_test_var(mut self, var: &'static str) -> Tool {
        if var.starts_with("CM_") && var.contains("_TEST_") {
            self.trust = Trust::WhenTestVar(var);
        }
        self
    }

    /// Skip the ownership checks (unit tests running fixture binaries).
    #[cfg(test)]
    pub(crate) fn untrusted_for_test(mut self) -> Tool {
        self.trust = Trust::Test;
        self
    }

    fn resolve(&self) -> Result<PathBuf, ToolError> {
        let relaxed = match &self.trust {
            Trust::RootOnly => false,
            // SAFETY: geteuid has no preconditions and cannot fail.
            Trust::OwnerOrRoot => return trusted_user_program(&self.program, unsafe { libc::geteuid() }),
            Trust::WhenTestVar(var) => std::env::var_os(var).is_some_and(|v| !v.is_empty()),
            #[cfg(test)]
            Trust::Test => true,
        };
        if !relaxed {
            return trusted_program(&self.program);
        }
        if !self.program.is_absolute() {
            return Err(ToolError::NotAbsolute(self.program.clone()));
        }
        std::fs::canonicalize(&self.program).map_err(|e| ToolError::Untrusted(format!("{} cannot be resolved: {e}", self.program.display())))
    }

    /// Run to completion or timeout. `Ok` carries timed-out runs too (`timed_out`).
    pub(crate) fn run(&self) -> Result<ToolOutput, ToolError> {
        let program = self.resolve()?;
        let started = Instant::now();
        let mut child = Command::new(&program)
            .args(&self.args)
            .env_clear()
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .env("PATH", MINIMAL_PATH)
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Set at spawn (POSIX_SPAWN_SETPGROUP), before the program runs.
            .process_group(0)
            .spawn()
            .map_err(|e| ToolError::Spawn(format!("{}: {e}", program.display())))?;
        let pid = child.id() as libc::pid_t;
        let out = Capture::start(child.stdout.take(), self.stdout_cap);
        let err = Capture::start(child.stderr.take(), self.stderr_cap);
        let deadline = started + self.timeout;

        let mut timed_out = false;
        loop {
            match exited_unreaped(pid) {
                Ok(true) => break,
                Ok(false) if Instant::now() >= deadline => {
                    timed_out = true;
                    signal_group(pid, libc::SIGTERM);
                    let grace = Instant::now() + TERM_GRACE;
                    while Instant::now() < grace && !exited_unreaped(pid).unwrap_or(true) {
                        std::thread::sleep(POLL);
                    }
                    break;
                }
                Ok(false) => std::thread::sleep(POLL),
                Err(e) => {
                    signal_group(pid, libc::SIGKILL);
                    let _ = child.wait();
                    return Err(ToolError::Spawn(format!("cannot wait for {}: {}", program.display(), super::sys::describe(e))));
                }
            }
        }
        // Ends the leader if it ignored SIGTERM and anything it left in its group.
        signal_group(pid, libc::SIGKILL);
        let status = child.wait().map_err(|e| ToolError::Spawn(format!("cannot reap {}: {e}", program.display())))?;
        let pipes_deadline = Instant::now() + PIPE_GRACE;
        while Instant::now() < pipes_deadline && !(out.closed() && err.closed()) {
            std::thread::sleep(POLL);
        }
        let (stdout, stdout_truncated) = out.take();
        let (stderr, stderr_truncated) = err.take();
        let duration = started.elapsed();
        cm_core::debug_trace!(
            "process",
            "{} {:?} exit={} signal={} timed_out={timed_out} stdout_bytes={} stderr_bytes={} in {}ms",
            program.display(),
            self.args,
            cm_core::trace::exit(status.code()),
            cm_core::trace::exit(status.signal()),
            stdout.len(),
            stderr.len(),
            duration.as_millis()
        );
        Ok(ToolOutput { exit_code: status.code(), signal: status.signal(), stdout, stderr, stdout_truncated, stderr_truncated, timed_out, duration })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macos::testfs::Fixture;

    /// Wait until `pid` no longer exists (reaped by launchd once orphaned).
    fn gone(pid: libc::pid_t) -> bool {
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            // SAFETY: signal 0 only checks for existence.
            if unsafe { libc::kill(pid, 0) } != 0 && errno() == libc::ESRCH {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn first_pid(out: &ToolOutput) -> libc::pid_t {
        String::from_utf8_lossy(&out.stdout).lines().next().and_then(|l| l.trim().parse().ok()).expect("pid printed")
    }

    #[test]
    fn runs_a_system_tool_with_a_minimal_environment() {
        let out = Tool::new("/usr/bin/env").env("CM_EXTRA", "1").run().unwrap();
        assert!(out.success() && !out.truncated() && !out.timed_out, "{out:?}");
        let mut lines: Vec<String> = String::from_utf8(out.stdout).unwrap().lines().map(str::to_owned).collect();
        lines.sort();
        assert_eq!(lines, ["CM_EXTRA=1", "LANG=C", "LC_ALL=C", &format!("PATH={MINIMAL_PATH}")]);
        let fail = Tool::new("/bin/sh").args(["-c", "echo oops >&2; exit 3"]).run().unwrap();
        assert_eq!((fail.exit_code, fail.success(), fail.stderr.as_slice()), (Some(3), false, &b"oops\n"[..]));
    }

    #[test]
    fn refuses_relative_paths_and_user_writable_binaries() {
        assert!(matches!(Tool::new("true").run(), Err(ToolError::NotAbsolute(_))));
        assert!(matches!(Tool::new("usr/bin/true").run(), Err(ToolError::NotAbsolute(_))));
        assert!(matches!(Tool::new("/usr/bin/cm-no-such-tool").run(), Err(ToolError::Untrusted(_))));
        let fx = Fixture::new("cmd-untrusted");
        let copy = fx.path().join("true");
        std::fs::copy("/usr/bin/true", &copy).unwrap();
        match Tool::new(&copy).run() {
            Err(ToolError::Untrusted(m)) => assert!(m.contains("not root"), "{m}"),
            other => panic!("user-owned binary ran: {other:?}"),
        }
        // A symlink in a user directory to a system tool runs the resolved system tool.
        let link = fx.path().join("env-link");
        std::os::unix::fs::symlink("/usr/bin/true", &link).unwrap();
        assert!(Tool::new(&link).run().unwrap().success());
        assert!(Tool::new(&copy).untrusted_for_test().run().unwrap().success());
        // The redirection override only applies while its variable is set.
        assert!(matches!(Tool::new(&copy).untrusted_when_test_var("CM_CMD_TEST_UNSET_VARIABLE").run(), Err(ToolError::Untrusted(_))));
        assert!(matches!(Tool::new(&copy).untrusted_when_test_var("HOME").run(), Err(ToolError::Untrusted(_))));
    }

    #[test]
    fn timeout_ends_the_whole_process_group() {
        let started = Instant::now();
        let out = Tool::new("/bin/sh").args(["-c", "sleep 30 & echo $!; wait"]).timeout(Duration::from_millis(500)).run().unwrap();
        assert!(out.timed_out && !out.success(), "{out:?}");
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(out.signal.is_some(), "{out:?}");
        assert!(gone(first_pid(&out)), "background child survived the timeout");
    }

    #[test]
    fn leftover_group_members_are_ended_when_the_tool_exits() {
        let started = Instant::now();
        let out = Tool::new("/bin/sh").args(["-c", "sleep 30 & echo $!"]).run().unwrap();
        assert!(out.success() && !out.timed_out, "{out:?}");
        assert!(started.elapsed() < Duration::from_secs(10), "did not wait for the background child");
        assert!(gone(first_pid(&out)));
    }

    #[test]
    fn output_is_capped_and_marked_truncated() {
        let out = Tool::new("/bin/dd").args(["if=/dev/zero", "bs=1000", "count=100"]).output_caps(4096, 64).run().unwrap();
        assert!(out.success(), "{out:?}");
        assert_eq!(out.stdout.len(), 4096);
        assert!(out.stdout_truncated && out.truncated());
        assert!(out.stderr.len() <= 64);
    }
}
