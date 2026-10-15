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

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
            let _ = child.kill();
            let _ = child.wait();
            return Err(RunError::Cancelled);
        }
        if over.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RunError::Failed(failure(FailureCode::OutputTooLarge, "CLI 输出超过上限，进程已终止，结果已丢弃。")));
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RunError::Failed(failure(FailureCode::Timeout, "CLI 在时限内没有结束，进程已终止。")));
        }
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => return Err(RunError::Failed(failure(FailureCode::ProcessFailed, format!("无法等待 CLI：{e}")))),
        }
    };
    let _ = writer.join();
    let stdout = reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    // Exit code and sizes only: the CLI's output can echo the prompt, and its environment may carry credentials.
    cm_core::debug_trace!("process", "\"{}\" exit={:?} in {}ms stdout_bytes={}", exe.display(), status.code(), start.elapsed().as_millis(), stdout.len());
    if over.load(Ordering::Acquire) {
        return Err(RunError::Failed(failure(FailureCode::OutputTooLarge, "CLI 输出超过上限，结果已丢弃。")));
    }
    Ok(Output { status: status.code(), stdout, stderr })
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
        return Err(failure(FailureCode::ProcessFailed, format!("CLI 退出码 {:?}：{excerpt}", out.status)));
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
