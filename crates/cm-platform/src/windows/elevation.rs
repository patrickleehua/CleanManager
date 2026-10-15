//! Short-lived elevated maintenance worker (parity 13.1, EXE-08, MNT-04; docs/privilege-boundary.md).
//!
//! - The parent (unelevated CLI) creates a single-instance named pipe `\\.\pipe\cleanmanager-maint-<pid>-<128-bit random>`
//!   whose DACL grants only the current user SID, rejecting remote clients, then asks Windows to start **one** worker:
//!   the same executable with the hidden first argument [`WORKER_SUBCOMMAND`], through UAC (`ShellExecuteExW` "runas").
//!   The worker's arguments fix the plan digest and the granted administrator task IDs, so the one UAC consent covers
//!   exactly those. A declined prompt (`ERROR_CANCELLED`) is [`LaunchError::Declined`].
//! - The parent accepts a client only if `GetNamedPipeClientProcessId` is the process it launched; anything else is
//!   disconnected and refused. The worker connects at identification impersonation level only and proceeds only if
//!   `GetNamedPipeServerProcessId` is the parent PID it was given; it refuses pipe names outside the local prefix.
//! - The worker answers newline-delimited JSON: a hello, then one reply per request. [`WorkerSession`] refuses anything
//!   but `run` of a granted task of this digest (arbitrary commands, other tasks, other plans, repeats) and the first
//!   refusal ends it. It exits on `done`, when the parent goes away, or after the idle timeout.
//! - The launcher is injectable: tests use [`DirectLauncher`] (an ordinary unelevated child) so no UAC prompt appears.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cm_core::maintenance::{TaskBackend, TaskRun, TaskSpec, WORKER_MAX_LINE, WORKER_PROTOCOL, WORKER_SUBCOMMAND, WorkerGrant, WorkerReply, WorkerRequest, WorkerSession};
use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_CANCELLED, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows_sys::Win32::Security::{GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ, FILE_GENERIC_WRITE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile,
    SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId, GetNamedPipeServerProcessId, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{CreateEventW, GetCurrentProcess, OpenProcessToken, WaitForMultipleObjects, WaitForSingleObject};

/// Local pipe namespace of workers; the worker refuses any other name (e.g. a remote `\\server\pipe\...`).
pub const PIPE_PREFIX: &str = r"\\.\pipe\cleanmanager-maint-";
/// Longest reply line the parent reads.
const MAX_REPLY: usize = 64 * 1024;
/// How long a worker waits for the next request before it exits.
pub const DEFAULT_IDLE_SECONDS: u32 = 120;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_error() -> u32 {
    // SAFETY: reads the calling thread's last-error value.
    unsafe { GetLastError() }
}

fn owned(h: HANDLE) -> Option<OwnedHandle> {
    // SAFETY: `h` is a fresh, valid handle owned by the caller from here on.
    (!h.is_null() && h != INVALID_HANDLE_VALUE).then(|| unsafe { OwnedHandle::from_raw_handle(h) })
}

/// The current user's SID as a string (`S-1-5-21-...`).
pub fn current_user_sid() -> Result<String, String> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: current-process pseudo handle; valid out-pointer.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(format!("token not readable ({})", last_error()));
    }
    let token = owned(token).ok_or("no token")?;
    let mut needed = 0u32;
    // SAFETY: size query with a null buffer.
    unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, std::ptr::null_mut(), 0, &mut needed) };
    let mut buf = vec![0u64; (needed as usize).div_ceil(8).max(1)];
    // SAFETY: 8-byte aligned buffer of at least `needed` bytes.
    if unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, buf.as_mut_ptr().cast(), needed, &mut needed) } == 0 {
        return Err(format!("token user not readable ({})", last_error()));
    }
    // SAFETY: the API wrote a TOKEN_USER at the start of the buffer.
    let sid = unsafe { (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid };
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: valid SID from the token buffer; the string is freed with LocalFree below.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(format!("SID not convertible ({})", last_error()));
    }
    // SAFETY: NUL-terminated string from ConvertSidToStringSidW.
    let s = unsafe {
        let len = (0..).take_while(|&i| *text.add(i) != 0).count();
        String::from_utf16_lossy(std::slice::from_raw_parts(text, len))
    };
    // SAFETY: allocated by ConvertSidToStringSidW.
    unsafe { LocalFree(text.cast()) };
    Ok(s)
}

/// DACL text of a worker pipe: protected, one ACE for the given user SID, nothing inherited.
pub fn pipe_sddl(user_sid: &str) -> String {
    format!("D:P(A;;GA;;;{user_sid})")
}

fn random_hex() -> Result<String, String> {
    use windows_sys::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
    let mut b = [0u8; 16];
    // SAFETY: 16-byte buffer, system RNG.
    if unsafe { BCryptGenRandom(std::ptr::null_mut(), b.as_mut_ptr(), b.len() as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG) } != 0 {
        return Err("no random source".into());
    }
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// A fresh, unguessable worker pipe name.
pub fn new_pipe_name() -> Result<String, String> {
    Ok(format!("{PIPE_PREFIX}{}-{}", std::process::id(), random_hex()?))
}

/// Only names this module creates: the local prefix, then lowercase hex digits and dashes.
pub fn valid_pipe_name(name: &str) -> bool {
    name.strip_prefix(PIPE_PREFIX).is_some_and(|rest| !rest.is_empty() && rest.len() <= 64 && rest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || b == b'-'))
}

/// An overlapped pipe handle with a line buffer.
pub struct Pipe {
    h: OwnedHandle,
    event: OwnedHandle,
    buf: Vec<u8>,
}

pub enum ReadLine {
    Line(String),
    Eof,
    Timeout,
    TooLong,
}

impl Pipe {
    fn new(h: OwnedHandle) -> Result<Pipe, String> {
        // SAFETY: unnamed manual-reset event, default security.
        let ev = owned(unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) }).ok_or("no event")?;
        Ok(Pipe { h, event: ev, buf: Vec::new() })
    }

    fn overlapped(&self) -> OVERLAPPED {
        // SAFETY: OVERLAPPED is plain data; zeroed is its documented initial state.
        let mut o: OVERLAPPED = unsafe { std::mem::zeroed() };
        o.hEvent = self.event.as_raw_handle();
        o
    }

    /// Wait for an overlapped operation; `Ok(None)` = timed out (cancelled).
    fn finish(&self, o: &mut OVERLAPPED, started: i32, timeout: Duration) -> Result<Option<u32>, u32> {
        let h = self.h.as_raw_handle();
        let mut n = 0u32;
        if started == 0 {
            let e = last_error();
            if e != ERROR_IO_PENDING {
                return Err(e);
            }
            let ms = timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32;
            // SAFETY: valid event handle.
            if unsafe { WaitForSingleObject(self.event.as_raw_handle(), ms) } == WAIT_TIMEOUT {
                // SAFETY: cancel this handle's pending I/O, then wait for it to settle so `o` outlives it.
                unsafe {
                    CancelIoEx(h, o);
                    GetOverlappedResult(h, o, &mut n, 1);
                }
                return Ok(None);
            }
        }
        // SAFETY: the operation on `o` has completed.
        if unsafe { GetOverlappedResult(h, o, &mut n, 1) } == 0 {
            return Err(last_error());
        }
        Ok(Some(n))
    }

    pub fn write_line(&mut self, line: &str, timeout: Duration) -> Result<(), String> {
        let mut data = line.as_bytes().to_vec();
        data.push(b'\n');
        let mut off = 0;
        while off < data.len() {
            let mut o = self.overlapped();
            let chunk = &data[off..];
            // SAFETY: buffer and OVERLAPPED stay alive until `finish` returns.
            let started = unsafe { WriteFile(self.h.as_raw_handle(), chunk.as_ptr(), chunk.len() as u32, std::ptr::null_mut(), &mut o) };
            match self.finish(&mut o, started, timeout) {
                Ok(Some(0)) | Ok(None) => return Err("pipe write timed out".into()),
                Ok(Some(n)) => off += n as usize,
                Err(e) => return Err(format!("pipe write failed ({e})")),
            }
        }
        Ok(())
    }

    pub fn read_line(&mut self, max: usize, timeout: Duration) -> Result<ReadLine, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=i).collect();
                if line.len() > max + 1 {
                    return Ok(ReadLine::TooLong);
                }
                return Ok(ReadLine::Line(String::from_utf8_lossy(&line[..line.len() - 1]).into_owned()));
            }
            if self.buf.len() > max {
                return Ok(ReadLine::TooLong);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(ReadLine::Timeout);
            }
            let mut chunk = vec![0u8; 4096];
            let mut o = self.overlapped();
            // SAFETY: buffer and OVERLAPPED stay alive until `finish` returns.
            let started = unsafe { ReadFile(self.h.as_raw_handle(), chunk.as_mut_ptr(), chunk.len() as u32, std::ptr::null_mut(), &mut o) };
            match self.finish(&mut o, started, left) {
                Ok(None) => return Ok(ReadLine::Timeout),
                Ok(Some(0)) => return Ok(ReadLine::Eof),
                Ok(Some(n)) => self.buf.extend_from_slice(&chunk[..n as usize]),
                Err(ERROR_BROKEN_PIPE) => return Ok(ReadLine::Eof),
                Err(e) => return Err(format!("pipe read failed ({e})")),
            }
        }
    }

    pub fn raw(&self) -> HANDLE {
        self.h.as_raw_handle()
    }
}

/// Parent side: create the single-instance, user-only, local-only server end.
pub fn create_server(name: &str) -> Result<Pipe, String> {
    if !valid_pipe_name(name) {
        return Err("invalid pipe name".into());
    }
    let sddl = wide(&pipe_sddl(&current_user_sid()?));
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: NUL-terminated SDDL; the descriptor is freed with LocalFree below.
    if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut()) } == 0 {
        return Err(format!("pipe security descriptor rejected ({})", last_error()));
    }
    let sa = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd, bInheritHandle: 0 };
    let n = wide(name);
    // SAFETY: NUL-terminated name, valid security attributes; one instance only, remote clients rejected.
    let h = unsafe {
        CreateNamedPipeW(
            n.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            64 * 1024,
            64 * 1024,
            0,
            &sa,
        )
    };
    let err = last_error();
    // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
    unsafe { LocalFree(sd) };
    Pipe::new(owned(h).ok_or_else(|| format!("pipe not created ({err})"))?)
}

/// Why the parent stopped waiting for its worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptError {
    /// Nothing acceptable connected in time; `refused` foreign clients were turned away meanwhile.
    Timeout { refused: u32 },
    /// The launched process exited before connecting.
    WorkerExited,
    Io(String),
}

/// Parent side: wait for the launched process to connect. Clients from any other process are disconnected and refused
/// (counted in the returned value), and waiting continues until `timeout`.
pub fn accept(pipe: &Pipe, expected_pid: u32, child: Option<&OwnedHandle>, timeout: Duration) -> Result<u32, AcceptError> {
    let deadline = Instant::now() + timeout;
    let mut refused = 0u32;
    loop {
        let mut o = pipe.overlapped();
        // SAFETY: OVERLAPPED stays alive until completion is observed below.
        let ok = unsafe { ConnectNamedPipe(pipe.raw(), &mut o) };
        let connected = if ok != 0 || last_error() == ERROR_PIPE_CONNECTED {
            true
        } else if last_error() != ERROR_IO_PENDING {
            return Err(AcceptError::Io(format!("connect failed ({})", last_error())));
        } else {
            let mut handles = vec![pipe.event.as_raw_handle()];
            if let Some(c) = child {
                handles.push(c.as_raw_handle());
            }
            let ms = deadline.saturating_duration_since(Instant::now()).as_millis().min(u128::from(u32::MAX - 1)) as u32;
            // SAFETY: valid handles; wait for any.
            let w = unsafe { WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, ms) };
            let mut n = 0u32;
            if w == WAIT_OBJECT_0 {
                // SAFETY: the connect operation completed.
                unsafe { GetOverlappedResult(pipe.raw(), &o, &mut n, 1) != 0 }
            } else {
                // SAFETY: cancel the pending connect and let it settle before `o` goes away.
                unsafe {
                    CancelIoEx(pipe.raw(), &o);
                    GetOverlappedResult(pipe.raw(), &o, &mut n, 1);
                }
                return Err(if w == WAIT_OBJECT_0 + 1 { AcceptError::WorkerExited } else { AcceptError::Timeout { refused } });
            }
        };
        if !connected {
            return Err(AcceptError::Io(format!("connect failed ({})", last_error())));
        }
        let mut pid = 0u32;
        // SAFETY: connected server handle; valid out-pointer.
        let known = unsafe { GetNamedPipeClientProcessId(pipe.raw(), &mut pid) } != 0;
        if known && pid == expected_pid {
            return Ok(refused);
        }
        // Not the process we launched: refuse it and keep waiting for ours.
        refused += 1;
        // SAFETY: connected server handle.
        unsafe { DisconnectNamedPipe(pipe.raw()) };
        if Instant::now() >= deadline {
            return Err(AcceptError::Timeout { refused });
        }
    }
}

/// Worker side: open the parent's pipe (identification level only) and check that its server is `parent_pid`.
pub fn connect_client(name: &str, parent_pid: u32) -> Result<Pipe, String> {
    if !valid_pipe_name(name) {
        return Err("refused: not a local CleanManager worker pipe".into());
    }
    let n = wide(name);
    // SAFETY: NUL-terminated name; the server may only identify, never impersonate, this (elevated) client.
    let h = unsafe {
        CreateFileW(n.as_ptr(), FILE_GENERIC_READ | FILE_GENERIC_WRITE, 0, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION, std::ptr::null_mut())
    };
    let pipe = Pipe::new(owned(h).ok_or_else(|| format!("pipe not opened ({})", last_error()))?)?;
    let mut pid = 0u32;
    // SAFETY: connected client handle; valid out-pointer.
    if unsafe { GetNamedPipeServerProcessId(pipe.raw(), &mut pid) } == 0 || pid != parent_pid {
        return Err(format!("refused: the pipe server is process {pid}, not the parent {parent_pid}"));
    }
    Ok(pipe)
}

// ---------------------------------------------------------------------------------------------------------------------
// Launchers

/// A started worker process.
pub struct LaunchedWorker {
    pub pid: u32,
    pub process: OwnedHandle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    /// The user declined the UAC prompt.
    Declined,
    Failed(String),
}

pub trait WorkerLauncher {
    fn launch(&self, exe: &Path, args: &[String]) -> Result<LaunchedWorker, LaunchError>;
    /// Whether the started worker must report an elevated token.
    fn elevates(&self) -> bool;
}

/// Production launcher: one UAC prompt (`runas`), hidden window.
pub struct UacLauncher;

fn quote(arg: &str) -> Result<String, LaunchError> {
    // Worker arguments are fixed tokens; anything that would need escaping is refused rather than escaped.
    if arg.is_empty() || arg.chars().any(|c| c == '"' || c.is_whitespace() || c.is_control()) {
        return Err(LaunchError::Failed(format!("refusing worker argument {arg:?}")));
    }
    Ok(format!("\"{arg}\""))
}

impl WorkerLauncher for UacLauncher {
    fn launch(&self, exe: &Path, args: &[String]) -> Result<LaunchedWorker, LaunchError> {
        use windows_sys::Win32::UI::Shell::{SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
        let params = args.iter().map(|a| quote(a)).collect::<Result<Vec<_>, _>>()?.join(" ");
        let verb = wide("runas");
        let file: Vec<u16> = exe.as_os_str().encode_wide_z();
        let params_w = wide(&params);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            lpVerb: verb.as_ptr(),
            lpFile: file.as_ptr(),
            lpParameters: params_w.as_ptr(),
            nShow: 0, // SW_HIDE
            ..Default::default()
        };
        // SAFETY: all strings are NUL-terminated and outlive the call; hProcess is returned to us.
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            let e = last_error();
            return Err(if e == ERROR_CANCELLED { LaunchError::Declined } else { LaunchError::Failed(format!("elevation failed ({e})")) });
        }
        let process = owned(info.hProcess).ok_or_else(|| LaunchError::Failed("no worker process handle".into()))?;
        // SAFETY: valid process handle.
        let pid = unsafe { windows_sys::Win32::System::Threading::GetProcessId(process.as_raw_handle()) };
        Ok(LaunchedWorker { pid, process })
    }

    fn elevates(&self) -> bool {
        true
    }
}

trait EncodeWideZ {
    fn encode_wide_z(&self) -> Vec<u16>;
}

impl EncodeWideZ for std::ffi::OsStr {
    fn encode_wide_z(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(std::iter::once(0)).collect()
    }
}

/// Test launcher: an ordinary child process with this process' token (never elevated, never prompts).
pub struct DirectLauncher;

impl WorkerLauncher for DirectLauncher {
    fn launch(&self, exe: &Path, args: &[String]) -> Result<LaunchedWorker, LaunchError> {
        let child = std::process::Command::new(exe)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| LaunchError::Failed(e.to_string()))?;
        let pid = child.id();
        Ok(LaunchedWorker { pid, process: OwnedHandle::from(child) })
    }

    fn elevates(&self) -> bool {
        false
    }
}

/// A launcher that always reports a declined prompt (tests of MNT-04).
pub struct DecliningLauncher;

impl WorkerLauncher for DecliningLauncher {
    fn launch(&self, _: &Path, _: &[String]) -> Result<LaunchedWorker, LaunchError> {
        Err(LaunchError::Declined)
    }

    fn elevates(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Parent: one connected worker

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    Declined,
    Unavailable(String),
}

pub struct WorkerConfig {
    pub exe: PathBuf,
    pub connect_timeout: Duration,
    pub idle_seconds: u32,
}

/// A connected, verified worker for one grant.
pub struct WorkerConnection {
    pipe: Pipe,
    child: LaunchedWorker,
    plan_digest: String,
    pub elevated: bool,
    /// Foreign clients refused while waiting.
    pub refused_clients: u32,
}

/// Worker arguments after [`WORKER_SUBCOMMAND`].
pub fn worker_args(pipe: &str, parent_pid: u32, grant: &WorkerGrant, idle_seconds: u32) -> Vec<String> {
    let mut a = vec![WORKER_SUBCOMMAND.to_owned(), "--pipe".into(), pipe.into(), "--parent-pid".into(), parent_pid.to_string()];
    a.extend(grant.to_args());
    a.extend(["--idle-seconds".into(), idle_seconds.to_string()]);
    a
}

/// Start, connect and verify one worker (one UAC prompt with [`UacLauncher`]).
pub fn start_worker(launcher: &dyn WorkerLauncher, cfg: &WorkerConfig, grant: &WorkerGrant) -> Result<WorkerConnection, StartError> {
    grant.validate(cm_core::maintenance::TaskPlatform::Windows).map_err(StartError::Unavailable)?;
    let name = new_pipe_name().map_err(StartError::Unavailable)?;
    let mut pipe = create_server(&name).map_err(StartError::Unavailable)?;
    let child = match launcher.launch(&cfg.exe, &worker_args(&name, std::process::id(), grant, cfg.idle_seconds)) {
        Ok(c) => c,
        Err(LaunchError::Declined) => return Err(StartError::Declined),
        Err(LaunchError::Failed(e)) => return Err(StartError::Unavailable(e)),
    };
    let refused_clients = accept(&pipe, child.pid, Some(&child.process), cfg.connect_timeout).map_err(|e| StartError::Unavailable(format!("worker did not connect: {e:?}")))?;
    let hello = match pipe.read_line(MAX_REPLY, cfg.connect_timeout) {
        Ok(ReadLine::Line(l)) => WorkerReply::parse(&l).ok_or_else(|| StartError::Unavailable("worker hello not understood".into()))?,
        _ => return Err(StartError::Unavailable("worker sent no hello".into())),
    };
    let elevated = match hello {
        WorkerReply::Hello { protocol, pid, elevated } if protocol == WORKER_PROTOCOL && pid == child.pid => elevated,
        _ => return Err(StartError::Unavailable("worker hello does not match the launched process".into())),
    };
    if launcher.elevates() && !elevated {
        return Err(StartError::Unavailable("the worker is not elevated".into()));
    }
    Ok(WorkerConnection { pipe, child, plan_digest: grant.plan_digest.clone(), elevated, refused_clients })
}

impl WorkerConnection {
    /// Ask the worker to run one granted task; the reply must be that task's result.
    pub fn run(&mut self, task: &TaskSpec) -> Result<TaskRun, String> {
        let req = WorkerRequest::Run { plan_digest: self.plan_digest.clone(), task_id: task.id.into(), task_version: task.version.into() };
        self.pipe.write_line(&req.to_line(), Duration::from_secs(10))?;
        self.reply_for(task)
    }

    /// Send a raw line (tests of the worker's refusals).
    pub fn send_raw(&mut self, line: &str) -> Result<WorkerReply, String> {
        self.pipe.write_line(line, Duration::from_secs(10))?;
        match self.pipe.read_line(MAX_REPLY, Duration::from_secs(30))? {
            ReadLine::Line(l) => WorkerReply::parse(&l).ok_or_else(|| "reply not understood".to_string()),
            _ => Err("no reply".into()),
        }
    }

    fn reply_for(&mut self, task: &TaskSpec) -> Result<TaskRun, String> {
        let timeout = Duration::from_secs(u64::from(task.timeout_seconds) + 30);
        let line = match self.pipe.read_line(MAX_REPLY, timeout)? {
            ReadLine::Line(l) => l,
            ReadLine::Timeout => return Err(format!("worker did not answer within {} s", timeout.as_secs())),
            ReadLine::Eof => return Err("worker ended the session".into()),
            ReadLine::TooLong => return Err("worker reply too long".into()),
        };
        match WorkerReply::parse(&line) {
            Some(WorkerReply::Result { run }) if run.task_id == task.id && run.task_version == task.version => Ok(run),
            Some(WorkerReply::Refused { code, message }) => Err(format!("worker refused ({code}): {message}")),
            _ => Err("worker reply does not match the request".into()),
        }
    }

    pub fn worker_pid(&self) -> u32 {
        self.child.pid
    }

    /// Tell the worker to exit and wait briefly for it.
    pub fn close(mut self) {
        let _ = self.pipe.write_line(r#"{"op":"done"}"#, Duration::from_secs(5));
        let _ = self.pipe.read_line(MAX_REPLY, Duration::from_secs(5));
        // SAFETY: valid process handle.
        unsafe { WaitForSingleObject(self.child.process.as_raw_handle(), 5_000) };
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Worker

pub struct WorkerArgs {
    pub pipe: String,
    pub parent_pid: u32,
    pub grant: WorkerGrant,
    pub idle_seconds: u32,
}

impl WorkerArgs {
    /// Parse the arguments after [`WORKER_SUBCOMMAND`]; every option exactly once, nothing else.
    pub fn parse(args: &[String]) -> Result<WorkerArgs, String> {
        let mut m = std::collections::BTreeMap::new();
        let mut it = args.iter();
        while let Some(k) = it.next() {
            let key = k.strip_prefix("--").ok_or_else(|| format!("unexpected argument {k}"))?;
            if !matches!(key, "pipe" | "parent-pid" | "plan-digest" | "tasks" | "idle-seconds") {
                return Err(format!("unknown worker option --{key}"));
            }
            let v = it.next().ok_or_else(|| format!("--{key} needs a value"))?;
            if m.insert(key, v.clone()).is_some() {
                return Err(format!("--{key} given twice"));
            }
        }
        let get = |k: &str| m.get(k).cloned().ok_or_else(|| format!("missing --{k}"));
        let pipe = get("pipe")?;
        if !valid_pipe_name(&pipe) {
            return Err("refused: not a local CleanManager worker pipe".into());
        }
        let parent_pid = get("parent-pid")?.parse().map_err(|_| "bad --parent-pid")?;
        let grant = WorkerGrant::parse(&get("plan-digest")?, &get("tasks")?)?;
        let idle_seconds = m.get("idle-seconds").map(|v| v.parse::<u32>().map_err(|_| "bad --idle-seconds")).transpose()?.unwrap_or(DEFAULT_IDLE_SECONDS).clamp(1, 3600);
        Ok(WorkerArgs { pipe, parent_pid, grant, idle_seconds })
    }
}

/// Run the worker: connect to the verified parent, say hello, then serve granted tasks until done, refusal, parent
/// exit or idle timeout. Returns the number of tasks run.
pub fn serve(args: &WorkerArgs, backend: &dyn TaskBackend, now: &dyn Fn() -> i64) -> Result<u32, String> {
    let mut session = WorkerSession::new(args.grant.clone(), cm_core::maintenance::TaskPlatform::Windows)?;
    let mut pipe = connect_client(&args.pipe, args.parent_pid)?;
    let elevated = super::startup_actions::is_elevated();
    let hello = WorkerReply::Hello { protocol: WORKER_PROTOCOL.into(), pid: std::process::id(), elevated };
    pipe.write_line(&hello.to_line(), Duration::from_secs(10))?;
    let mut ran = 0u32;
    loop {
        let line = match pipe.read_line(WORKER_MAX_LINE, Duration::from_secs(u64::from(args.idle_seconds)))? {
            ReadLine::Line(l) => l,
            ReadLine::Eof | ReadLine::Timeout => return Ok(ran),
            ReadLine::TooLong => "x".repeat(WORKER_MAX_LINE + 1),
        };
        let (reply, go_on) = session.handle(&line, &mut |spec| {
            ran += 1;
            cm_core::maintenance::run_task(backend, spec, elevated, now)
        });
        pipe.write_line(&reply.to_line(), Duration::from_secs(10))?;
        if !go_on {
            return Ok(ran);
        }
    }
}

#[cfg(test)]
mod tests;
