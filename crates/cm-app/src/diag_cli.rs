//! Constrained CLI diagnosis adapter (AID-08). The person picks an executable file; it runs
//! with a fixed argument vector (no shell, no string interpolation), the evidence on stdin, an
//! empty private working directory, a minimal environment, a time limit, an output limit and
//! cancellation. Only a mode that can switch off every tool is supported:
//!
//! - Claude Code (`claude -p`) is supported when its help lists the flags that disable all tools
//!   (`--tools ""`), MCP servers, settings-defined hooks and session persistence. Missing flags
//!   make it unsupported; it is never started "just to try".
//! - Codex (`codex exec`) is always unsupported: its read-only sandbox still lets the model run
//!   shell commands that read any file, and no documented flag disables that tool.
//!
//! On macOS and Linux the CLI starts in its own process group (set at spawn, before it runs). Its exit
//! is detected without reaping it, so the group id stays reserved; then the whole group is killed on
//! timeout, cancellation, the output limit and also after a normal exit, so helpers or grandchildren
//! it started never outlive the call or hold its pipes open.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cm_core::diag::{FailureCode, ProviderKind, budget};

use super::http::{RunError, failure, scrub};

/// Flags that must appear in `--help` before Claude Code is used.
pub const CLAUDE_REQUIRED_FLAGS: &[&str] =
    &["--print", "--output-format", "--tools", "--disallowedTools", "--strict-mcp-config", "--permission-mode", "--no-session-persistence", "--restricted"];
/// Tools named again in the deny list, in case a future default adds one.
pub const CLAUDE_DENIED_TOOLS: &str = "Bash,PowerShell,Edit,Write,MultiEdit,NotebookEdit,Read,Glob,Grep,LS,WebFetch,WebSearch,Task,TodoWrite";
pub const CODEX_UNSUPPORTED: &str = "codex exec cannot disable its shell tool; even the read-only sandbox lets the model read any file, so it is not started";
/// Help/version checks.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_STDERR: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliCheck {
    pub supported: bool,
    pub version: Option<String>,
    pub missing: Vec<String>,
    /// Optional hardening flags this build offers (added to the argument vector when present).
    pub optional: Vec<String>,
    pub reason: String,
}

/// An absolute path to an existing executable file. On Windows only `.exe` is accepted:
/// `.cmd`/`.bat`/`.ps1` would run through a shell interpreter.
pub fn validate_executable(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("the CLI must be an absolute path".into());
    }
    let meta = std::fs::metadata(path).map_err(|_| "the CLI executable does not exist".to_owned())?;
    if !meta.is_file() {
        return Err("the CLI path is not a file".into());
    }
    if cfg!(windows) && !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) {
        return Err("only .exe files are accepted (scripts such as .cmd, .bat or .ps1 would run through a shell)".into());
    }
    Ok(())
}

/// Empty per-run working directory; nothing of the person's is inside it.
fn sandbox_dir() -> std::io::Result<PathBuf> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("cm-diag-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Environment passed to the CLI: what it needs to find its own sign-in, nothing else.
fn environment(sandbox: &Path) -> Vec<(String, String)> {
    const KEEP: &[&str] = &[
        "SystemRoot", "SystemDrive", "windir", "USERPROFILE", "HOMEDRIVE", "HOMEPATH", "APPDATA", "LOCALAPPDATA", "ProgramData", "ProgramFiles",
        "PROCESSOR_ARCHITECTURE", "NUMBER_OF_PROCESSORS", "OS", "COMPUTERNAME", "USERNAME", "HOME", "USER", "LOGNAME", "LANG", "LC_ALL",
        "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL", "CLAUDE_CONFIG_DIR", "HTTPS_PROXY", "HTTP_PROXY", "NO_PROXY",
    ];
    let mut env: Vec<(String, String)> = std::env::vars().filter(|(k, _)| KEEP.iter().any(|x| x.eq_ignore_ascii_case(k))).collect();
    let tmp = sandbox.to_string_lossy().into_owned();
    for k in ["TEMP", "TMP", "TMPDIR"] {
        env.push((k.into(), tmp.clone()));
    }
    env.push(("PATH".into(), if cfg!(windows) { std::env::var("SystemRoot").map(|r| format!("{r}\\System32")).unwrap_or_default() } else { "/usr/bin:/bin".into() }));
    env.push(("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(), "1".into()));
    env
}

pub struct Output {
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

/// Run `exe argv…` with `stdin`, bounded by `timeout` and `max_stdout`. Never through a shell.
pub fn run(exe: &Path, argv: &[String], stdin: &[u8], timeout: Duration, max_stdout: usize, cancel: &Arc<AtomicBool>, extra_env: &[(String, String)]) -> Result<Output, RunError> {
    let sandbox = sandbox_dir().map_err(|e| RunError::Failed(failure(FailureCode::ProcessFailed, format!("无法创建隔离工作目录：{e}"))))?;
    let result = run_in(exe, argv, stdin, timeout, max_stdout, cancel, extra_env, &sandbox);
    let _ = std::fs::remove_dir_all(&sandbox);
    result
}

#[allow(clippy::too_many_arguments)]
fn run_in(
    exe: &Path, argv: &[String], stdin: &[u8], timeout: Duration, max_stdout: usize, cancel: &Arc<AtomicBool>, extra_env: &[(String, String)], sandbox: &Path,
) -> Result<Output, RunError> {
    let mut cmd = Command::new(exe);
    cmd.args(argv).current_dir(sandbox).env_clear().envs(environment(sandbox)).envs(extra_env.iter().cloned());
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::os::unix::process::CommandExt;
        // Own process group, led by the CLI: `group::kill` reaches everything it starts.
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|e| RunError::Failed(failure(FailureCode::ProcessFailed, format!("无法启动 CLI：{e}"))))?;
    let mut sin = child.stdin.take().expect("piped");
    let data = stdin.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = sin.write_all(&data);
    });
    let over = Arc::new(AtomicBool::new(false));
    let mut sout = child.stdout.take().expect("piped");
    let over_r = over.clone();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match sout.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if buf.len() + n > max_stdout {
                        over_r.store(true, Ordering::Release);
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
            }
        }
        buf
    });
    let mut serr = child.stderr.take().expect("piped");
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = serr.by_ref().take(MAX_STDERR as u64).read_to_end(&mut buf);
        let _ = std::io::copy(&mut serr, &mut std::io::sink());
        String::from_utf8_lossy(&buf).into_owned()
    });
    let start = Instant::now();
    let status = loop {
        if cancel.load(Ordering::Acquire) {
            terminate(&mut child);
            return Err(RunError::Cancelled);
        }
        if over.load(Ordering::Acquire) {
            terminate(&mut child);
            return Err(RunError::Failed(failure(FailureCode::OutputTooLarge, "CLI 输出超过上限，进程已终止，结果已丢弃。")));
        }
        if start.elapsed() > timeout {
            terminate(&mut child);
            return Err(RunError::Failed(failure(FailureCode::Timeout, "CLI 在时限内没有结束，进程已终止。")));
        }
        match poll_exit(&mut child) {
            Ok(Some(s)) => break s,
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => return Err(RunError::Failed(failure(FailureCode::ProcessFailed, format!("无法等待 CLI：{e}")))),
        }
    };
    let _ = writer.join();
    let stdout = reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    // Exit code and sizes only: the CLI's output can echo the prompt, and its environment may carry credentials.
    cm_core::debug_trace!("process", "\"{}\" exit={} in {}ms stdout_bytes={}", exe.display(), cm_core::trace::exit(status.code()), start.elapsed().as_millis(), stdout.len());
    if over.load(Ordering::Acquire) {
        return Err(RunError::Failed(failure(FailureCode::OutputTooLarge, "CLI 输出超过上限，结果已丢弃。")));
    }
    Ok(Output { status: status.code(), stdout, stderr })
}

/// End the CLI (and on macOS/Linux its whole process group) and reap it.
fn terminate(child: &mut Child) {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    group::kill(child);
    let _ = child.kill();
    let _ = child.wait();
}

/// The CLI's exit status once it has ended. On macOS/Linux the exit is seen before the CLI is reaped, so its group
/// id cannot be reused yet: whatever it left running in the group is killed, then it is reaped.
fn poll_exit(child: &mut Child) -> std::io::Result<Option<ExitStatus>> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        if !group::exited_unreaped(child)? {
            return Ok(None);
        }
        group::kill(child);
        child.wait().map(Some)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    child.try_wait()
}

/// Process-group control without a libc dependency; declarations follow `<signal.h>` and `<sys/wait.h>`.
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod group {
    use std::process::Child;

    const SIGKILL: i32 = 9;
    /// `idtype_t` `P_PID`.
    const P_PID: i32 = 1;
    const WNOHANG: i32 = 0x1;
    const WEXITED: i32 = 0x4;
    #[cfg(target_os = "macos")]
    const WNOWAIT: i32 = 0x20;
    #[cfg(target_os = "linux")]
    const WNOWAIT: i32 = 0x0100_0000;
    const EINTR: i32 = 4;

    /// `siginfo_t` with `si_signo` first on both systems (104 bytes on macOS, 128 on Linux); oversized and
    /// zero-initialized, only `si_signo` is read.
    #[repr(C, align(8))]
    struct SigInfo {
        si_signo: i32,
        _rest: [u8; 252],
    }

    unsafe extern "C" {
        /// `int killpg(pid_t pgrp, int sig);`
        fn killpg(pgrp: i32, sig: i32) -> i32;
        /// `int waitid(idtype_t idtype, id_t id, siginfo_t *infop, int options);`
        fn waitid(idtype: i32, id: u32, infop: *mut SigInfo, options: i32) -> i32;
    }

    /// Whether the CLI has exited, without reaping it (the zombie keeps its pid and group id reserved).
    pub(super) fn exited_unreaped(child: &Child) -> std::io::Result<bool> {
        let mut info = SigInfo { si_signo: 0, _rest: [0; 252] };
        // SAFETY: our own unreaped child's pid and a valid, zeroed out-buffer larger than `siginfo_t`; WNOWAIT leaves
        // the child waitable, WNOHANG returns at once (with `si_signo` left 0 when it is still running).
        let rc = unsafe { waitid(P_PID, child.id(), &mut info, WEXITED | WNOHANG | WNOWAIT) };
        if rc != 0 {
            let e = std::io::Error::last_os_error();
            return if e.raw_os_error() == Some(EINTR) { Ok(false) } else { Err(e) };
        }
        Ok(info.si_signo != 0)
    }

    /// SIGKILL to the CLI's process group. Only called while the leader is not yet reaped, so the group id still
    /// names that group.
    pub(super) fn kill(child: &Child) {
        let Ok(pgid) = i32::try_from(child.id()) else { return };
        // SAFETY: plain syscall; the group is led by our unreaped child.
        unsafe { killpg(pgid, SIGKILL) };
    }
}

/// Check a CLI without sending any data: `--version` and `--help` only.
pub fn verify(kind: ProviderKind, exe: &Path) -> CliCheck {
    let unsupported = |reason: String| CliCheck { supported: false, version: None, missing: vec![], optional: vec![], reason };
    if kind == ProviderKind::CodexCli {
        return unsupported(CODEX_UNSUPPORTED.into());
    }
    if kind != ProviderKind::ClaudeCodeCli {
        return unsupported("not a CLI provider".into());
    }
    if let Err(e) = validate_executable(exe) {
        return unsupported(e);
    }
    let never = Arc::new(AtomicBool::new(false));
    let text = |args: &[&str]| -> Option<String> {
        let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        run(exe, &argv, b"", CHECK_TIMEOUT, 512 * 1024, &never, &[]).ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    let version = text(&["--version"]).map(|v| v.lines().next().unwrap_or_default().chars().take(80).collect::<String>());
    let Some(help) = text(&["--help"]) else {
        return CliCheck { version, ..unsupported("could not read the CLI's --help".into()) };
    };
    let present = |flag: &str| help.split(|c: char| c.is_whitespace() || c == ',').any(|w| w == flag);
    let missing: Vec<String> = CLAUDE_REQUIRED_FLAGS.iter().filter(|f| !present(f)).map(|f| f.to_string()).collect();
    let tools_off = help.contains("\"\" to disable all tools") || help.contains("\"\" to disable all");
    let optional: Vec<String> = ["--safe-mode", "--disable-slash-commands"].iter().filter(|f| present(f)).map(|f| f.to_string()).collect();
    if !missing.is_empty() || !tools_off {
        let mut why = format!("this CLI build cannot be constrained (missing: {})", if missing.is_empty() { "--tools \"\"".into() } else { missing.join(", ") });
        if !tools_off && missing.is_empty() {
            why = "this CLI build does not document disabling all tools with --tools \"\"".into();
        }
        return CliCheck { supported: false, version, missing, optional, reason: why };
    }
    CliCheck { supported: true, version, missing, optional, reason: "every built-in tool, MCP server, settings file and session write is disabled".into() }
}

/// Fixed argument vector for Claude Code print mode. The evidence goes on stdin.
pub fn claude_argv(model: &str, system: &str, check: &CliCheck) -> Vec<String> {
    let mut a: Vec<String> = [
        "-p", "--output-format", "json", "--tools", "", "--disallowedTools", CLAUDE_DENIED_TOOLS, "--strict-mcp-config", "--permission-mode", "dontAsk",
        "--restricted", "--no-session-persistence",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    a.extend(check.optional.iter().cloned());
    if !model.is_empty() && model != "default" {
        a.push("--model".into());
        a.push(model.into());
    }
    a.push("--system-prompt".into());
    a.push(system.into());
    a
}

/// The answer text from Claude Code's `--output-format json` result object.
pub fn claude_result(out: &Output) -> Result<String, cm_core::diag::Failure> {
    if out.status != Some(0) {
        let excerpt: String = scrub(&out.stderr, None).chars().filter(|c| !c.is_control()).take(160).collect();
        let code = out.status.map_or_else(|| "无（进程未正常结束）".to_owned(), |c| c.to_string());
        return Err(failure(FailureCode::ProcessFailed, format!("CLI 退出码 {code}：{excerpt}")));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|_| failure(FailureCode::InvalidStructure, "CLI 输出不是预期的 JSON 结果。"))?;
    if v["type"] != "result" || v["is_error"].as_bool() != Some(false) {
        let msg: String = v["result"].as_str().unwrap_or("").chars().take(120).collect();
        return Err(failure(FailureCode::ProviderError, format!("CLI 报告错误：{}", scrub(&msg, None))));
    }
    match v["result"].as_str() {
        Some(t) if !t.trim().is_empty() && t.len() <= budget::RAW_ANSWER_BYTES => Ok(t.to_owned()),
        Some(t) if t.len() > budget::RAW_ANSWER_BYTES => Err(failure(FailureCode::OutputTooLarge, "回答超过长度上限，已丢弃。")),
        _ => Err(failure(FailureCode::InvalidStructure, "CLI 没有返回回答文本。")),
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod group_tests {
    use super::*;

    /// Unique scratch directory for the pid file (the CLI's own working directory is removed after the run).
    fn scratch(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let d = std::env::temp_dir().join(format!("cm-diag-group-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The grandchild's pid, written by the fixture shell before it blocks.
    fn grandchild(pid_file: &Path) -> i32 {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(pid) = std::fs::read_to_string(pid_file).ok().and_then(|t| t.trim().parse().ok()) {
                return pid;
            }
            assert!(Instant::now() < until, "fixture did not write its grandchild pid");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    unsafe extern "C" {
        /// `int kill(pid_t pid, int sig);`
        fn kill(pid: i32, sig: i32) -> i32;
    }

    /// The process is gone (killed and reaped by init/launchd once orphaned).
    fn gone(pid: i32) -> bool {
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            // SAFETY: signal 0 only checks for existence.
            if unsafe { kill(pid, 0) } != 0 && std::io::Error::last_os_error().raw_os_error() == Some(3) {
                return true; // ESRCH
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// A shell that starts a long-running grandchild, records its pid and then `script`s; no shell is involved in
    /// starting the fixture itself (fixed argument vector).
    fn fixture(pid_file: &Path, then: &str) -> Vec<String> {
        vec!["-c".into(), format!("sleep 60 & echo $! > '{}'; {then}", pid_file.display())]
    }

    #[test]
    fn timeout_kills_the_grandchild_too() {
        let dir = scratch("timeout");
        let pid_file = dir.join("pid");
        let never = Arc::new(AtomicBool::new(false));
        let started = Instant::now();
        let r = run(Path::new("/bin/sh"), &fixture(&pid_file, "wait"), b"", Duration::from_millis(700), 1 << 20, &never, &[]);
        assert!(matches!(&r, Err(RunError::Failed(f)) if f.code == FailureCode::Timeout), "{:?}", r.as_ref().err());
        assert!(started.elapsed() < Duration::from_secs(15), "returned without waiting for the grandchild");
        assert!(gone(grandchild(&pid_file)), "the grandchild survived the timeout");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_limit_and_cancel_kill_the_grandchild_too() {
        let dir = scratch("limit");
        let pid_file = dir.join("pid");
        let never = Arc::new(AtomicBool::new(false));
        let r = run(Path::new("/bin/sh"), &fixture(&pid_file, "yes"), b"", Duration::from_secs(30), 64 * 1024, &never, &[]);
        assert!(matches!(&r, Err(RunError::Failed(f)) if f.code == FailureCode::OutputTooLarge), "{:?}", r.as_ref().err());
        assert!(gone(grandchild(&pid_file)), "the grandchild survived the output limit");

        let pid_file = dir.join("pid2");
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            flag.store(true, Ordering::Release);
        });
        let r = run(Path::new("/bin/sh"), &fixture(&pid_file, "wait"), b"", Duration::from_secs(30), 1 << 20, &cancel, &[]);
        t.join().unwrap();
        assert!(matches!(r, Err(RunError::Cancelled)));
        assert!(gone(grandchild(&pid_file)), "the grandchild survived cancellation");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A CLI that exits normally while a grandchild still holds its output pipe: the call returns at once, with the
    /// output, and the grandchild is ended.
    #[test]
    fn leftover_grandchild_is_ended_after_a_normal_exit() {
        let dir = scratch("exit");
        let pid_file = dir.join("pid");
        let never = Arc::new(AtomicBool::new(false));
        let started = Instant::now();
        let out = run(Path::new("/bin/sh"), &fixture(&pid_file, "echo done"), b"", Duration::from_secs(30), 1 << 20, &never, &[]).unwrap();
        assert_eq!((out.status, String::from_utf8_lossy(&out.stdout).trim()), (Some(0), "done"));
        assert!(started.elapsed() < Duration::from_secs(15), "waited for the grandchild");
        assert!(gone(grandchild(&pid_file)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
