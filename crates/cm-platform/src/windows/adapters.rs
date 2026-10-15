//! Owner-tool adapter runner (CAT-05), Windows.
//!
//! The executable comes only from an installer-written location (the Node.js install key or the
//! Program Files known folder), never `PATH` or the working directory; it must be a regular file
//! (no link), and its SHA-256 is recorded before launch. The process gets the adapter's fixed argv
//! as separate arguments (no shell, no batch file), no console window, a null stdin, the install
//! directory as working directory (no project config is picked up), a time limit and bounded
//! output capture. Only the read-only preview argv is ever run by this build.

use std::ffi::OsString;
use std::io::Read;
use std::os::windows::fs::MetadataExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cm_core::adapter::{AdapterPreview, AdapterSpec, AdapterStatus, Locator, PreviewKind, reported_path, sha256_hex, summarize_output};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const ATTR_REPARSE: u32 = 0x400;

fn program_files() -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_ProgramFiles, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    // SAFETY: documented known-folder query; the string is freed once with CoTaskMemFree.
    let p = unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramFiles, KF_FLAG_DEFAULT, None) }.ok()?;
    // SAFETY: valid NUL-terminated string returned above.
    let s = unsafe { p.to_string() }.ok();
    // SAFETY: allocated by the shell with CoTaskMemAlloc.
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s.map(PathBuf::from)
}

/// A regular, non-link file at an absolute path.
fn regular_file(p: &Path) -> Result<(), String> {
    if !p.is_absolute() {
        return Err(format!("{} is not absolute", p.display()));
    }
    let m = std::fs::symlink_metadata(p).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("{} does not exist", p.display()),
        _ => format!("{} cannot be inspected (os error {})", p.display(), e.raw_os_error().unwrap_or(0)),
    })?;
    if m.file_attributes() & ATTR_REPARSE != 0 || !m.is_file() {
        return Err(format!("{} is not a regular file", p.display()));
    }
    Ok(())
}

/// Executable (and script, for interpreter locators) from trusted install locations only.
pub fn locate(spec: &AdapterSpec) -> Result<(PathBuf, Option<PathBuf>), String> {
    let (exe, script) = match spec.locator {
        Locator::NodeJsScript { script } => {
            let dir = super::resolve::nodejs_install_dir().or_else(|| program_files().map(|p| p.join("nodejs"))).ok_or("Node.js is not installed")?;
            (dir.join("node.exe"), Some(script.iter().fold(dir, |p, c| p.join(c))))
        }
        Locator::ProgramFiles { path } => (path.iter().fold(program_files().ok_or("Program Files folder unknown")?, |p, c| p.join(c)), None),
    };
    regular_file(&exe)?;
    if let Some(s) = &script {
        regular_file(s)?;
    }
    Ok((exe, script))
}

fn sha_of(p: &Path) -> Option<String> {
    std::fs::File::open(p).ok().and_then(|f| sha256_hex(f).ok())
}

#[derive(Debug)]
pub(crate) enum RunError {
    Spawn(String),
    Timeout { output: Vec<u8>, total: usize },
}

pub(crate) struct Ran {
    pub exit: Option<i32>,
    pub stdout: Vec<u8>,
    pub stdout_total: usize,
    pub stderr: Vec<u8>,
    pub stderr_total: usize,
}

/// Keep the first `limit` bytes, count the rest, never block the child on a full pipe.
fn drain(mut r: impl Read + Send + 'static, limit: usize) -> std::thread::JoinHandle<(Vec<u8>, usize)> {
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut total = 0usize;
        let mut buf = [0u8; 8192];
        while let Ok(n) = r.read(&mut buf) {
            if n == 0 {
                break;
            }
            total += n;
            let room = limit.saturating_sub(kept.len());
            kept.extend_from_slice(&buf[..n.min(room)]);
        }
        (kept, total)
    })
}

/// Run `exe args` without a shell, with a timeout and bounded output.
pub(crate) fn run_bounded(exe: &Path, args: &[OsString], cwd: &Path, timeout: Duration, limit: usize) -> Result<Ran, RunError> {
    let mut child = Command::new(exe)
        .args(args)
        .current_dir(cwd)
        .env("NO_UPDATE_NOTIFIER", "1")
        .env("npm_config_update_notifier", "false")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| RunError::Spawn(e.to_string()))?;
    let out = drain(child.stdout.take().expect("piped"), limit);
    let err = drain(child.stderr.take().expect("piped"), limit);
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() >= deadline => break None,
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RunError::Spawn(format!("cannot wait for the tool: {e}")));
            }
        }
    };
    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        let (output, total) = out.join().unwrap_or_default();
        return Err(RunError::Timeout { output, total });
    };
    let (stdout, stdout_total) = out.join().unwrap_or_default();
    let (stderr, stderr_total) = err.join().unwrap_or_default();
    Ok(Ran { exit: status.code(), stdout, stdout_total, stderr, stderr_total })
}

/// Run an adapter's read-only preview. Every failure is reported, none falls back to anything.
pub fn run_preview(spec: &AdapterSpec) -> AdapterPreview {
    if let Err(e) = spec.validate() {
        return AdapterPreview::unavailable(spec, format!("adapter rejected before launch: {e}"));
    }
    let (exe, script) = match locate(spec) {
        Ok(v) => v,
        Err(e) => return AdapterPreview::unavailable(spec, format!("{} not found in a trusted location: {e}", spec.tool_name)),
    };
    let mut p = AdapterPreview::unavailable(spec, "");
    p.reason = None;
    p.executable = Some(exe.display().to_string());
    p.executable_sha256 = sha_of(&exe);
    p.script = script.as_ref().map(|s| s.display().to_string());
    p.script_sha256 = script.as_deref().and_then(sha_of);
    if p.executable_sha256.is_none() || (script.is_some() && p.script_sha256.is_none()) {
        p.reason = Some("cannot read the executable to record its SHA-256".into());
        return p;
    }
    let mut args: Vec<OsString> = script.iter().map(|s| s.as_os_str().to_owned()).collect();
    args.extend(spec.preview_argv.iter().map(OsString::from));
    let cwd = exe.parent().unwrap_or(Path::new("\\")).to_path_buf();
    let started = Instant::now();
    let result = run_bounded(&exe, &args, &cwd, Duration::from_millis(spec.timeout_ms), spec.max_output_bytes);
    p.duration_ms = started.elapsed().as_millis() as u64;
    match result {
        Err(RunError::Spawn(e)) => p.reason = Some(format!("could not start {}: {e}", spec.tool_name)),
        Err(RunError::Timeout { output, total }) => {
            p.status = AdapterStatus::TimedOut;
            p.reason = Some(format!("no result within {} ms; the tool was stopped", spec.timeout_ms));
            p.output_summary = summarize_output(&output, total, spec.max_output_bytes);
        }
        Ok(ran) => {
            p.exit_code = ran.exit;
            let mut summary = summarize_output(&ran.stdout, ran.stdout_total, spec.max_output_bytes);
            if ran.stderr_total > 0 {
                summary.push_str("\nstderr: ");
                summary.push_str(&summarize_output(&ran.stderr, ran.stderr_total, spec.max_output_bytes / 4));
            }
            if ran.exit == Some(0) {
                p.status = AdapterStatus::Available;
                if spec.preview_kind == PreviewKind::CachePath {
                    p.reported_path = reported_path(&summarize_output(&ran.stdout, ran.stdout_total, spec.max_output_bytes));
                    if p.reported_path.is_none() {
                        p.status = AdapterStatus::Failed;
                        p.reason = Some("the tool did not report an absolute cache path".into());
                    }
                }
            } else {
                p.status = AdapterStatus::Failed;
                p.reason = Some(format!("exit code {:?}", ran.exit));
            }
            p.output_summary = summary.trim().to_owned();
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use cm_core::adapter::ADAPTERS;

    fn system32(exe: &str) -> PathBuf {
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32").join(exe)
    }

    #[test]
    fn a_missing_tool_is_unavailable_and_nothing_runs() {
        let spec = AdapterSpec { locator: Locator::ProgramFiles { path: &["CleanManagerNoSuchTool", "tool.exe"] }, ..ADAPTERS[1].clone() };
        let p = run_preview(&spec);
        assert_eq!(p.status, AdapterStatus::Unavailable);
        assert!(p.reason.unwrap().contains("not found in a trusted location"));
        assert!(p.exit_code.is_none() && p.executable.is_none());
    }

    #[test]
    fn an_injected_argument_is_rejected_before_launch() {
        let spec = AdapterSpec { preview_argv: &["config", "get", "cache&calc"], ..ADAPTERS[0].clone() };
        let p = run_preview(&spec);
        assert_eq!(p.status, AdapterStatus::Unavailable);
        assert!(p.reason.unwrap().contains("rejected before launch"));
        assert!(p.executable.is_none(), "no executable is even located for a rejected spec");
    }

    #[test]
    fn a_slow_tool_times_out_and_is_stopped() {
        let args: Vec<OsString> = ["-n", "30", "127.0.0.1"].iter().map(OsString::from).collect();
        let started = Instant::now();
        let r = run_bounded(&system32("PING.EXE"), &args, &std::env::temp_dir(), Duration::from_millis(300), 1024);
        assert!(matches!(r, Err(RunError::Timeout { .. })), "{:?}", r.err());
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn output_capture_is_bounded() {
        let args: Vec<OsString> = ["/all"].iter().map(OsString::from).collect();
        let r = run_bounded(&system32("whoami.exe"), &args, &std::env::temp_dir(), Duration::from_secs(20), 64).expect("whoami runs");
        assert_eq!(r.exit, Some(0));
        assert!(r.stdout.len() <= 64 && r.stdout_total > 64, "kept {} of {}", r.stdout.len(), r.stdout_total);
    }

    #[test]
    fn arguments_reach_the_tool_verbatim_without_a_shell() {
        // A shell would run the second command; without one `&` is just a hostname character.
        let args: Vec<OsString> = ["-n", "1", "-w", "1", "127.0.0.1&whoami"].iter().map(OsString::from).collect();
        let r = run_bounded(&system32("PING.EXE"), &args, &std::env::temp_dir(), Duration::from_secs(20), 4096).expect("ping runs");
        let user = std::env::var("USERNAME").unwrap_or_default().to_ascii_lowercase();
        let text = String::from_utf8_lossy(&r.stdout).to_ascii_lowercase();
        assert!(user.is_empty() || !text.contains(&format!("\\{user}")), "whoami must not have run: {text}");
    }

    #[test]
    fn real_npm_preview_reports_its_cache_or_is_unavailable() {
        let p = run_preview(&ADAPTERS[0]);
        match p.status {
            AdapterStatus::Available => {
                assert!(p.executable.as_deref().is_some_and(|e| e.to_ascii_lowercase().ends_with("node.exe")));
                assert_eq!(p.executable_sha256.as_ref().map(String::len), Some(64));
                assert!(p.reported_path.is_some(), "{p:?}");
                assert_eq!(p.exit_code, Some(0));
            }
            _ => assert!(p.reported_path.is_none(), "a failed preview never reports a path: {p:?}"),
        }
    }
}
