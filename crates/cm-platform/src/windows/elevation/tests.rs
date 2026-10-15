//! EXE-08 / MNT-04 boundary tests of the worker pipe, in-process and without UAC: the "worker" runs on a thread (or is
//! never started), tasks are scripted, so nothing on the machine changes and no prompt appears.

use std::sync::mpsc;

use super::*;
use cm_core::maintenance::{Execution, Probe, TaskOutcome, TaskRef, ids};

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

struct Scripted;

impl TaskBackend for Scripted {
    fn probe(&self, _: &TaskSpec) -> Probe {
        Probe::Applicable { detail: String::new() }
    }
    fn execute(&self, t: &TaskSpec) -> Execution {
        Execution::new(TaskOutcome::Unchanged, "scripted", format!("scripted {}", t.id))
    }
}

fn grant() -> WorkerGrant {
    WorkerGrant { plan_digest: DIGEST.into(), tasks: vec![TaskRef { task_id: ids::TRIM.into(), task_version: "1.0.0".into() }] }
}

fn self_handle() -> OwnedHandle {
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
    // SAFETY: synchronize-only handle to this process (never signaled while the test runs).
    owned(unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, std::process::id()) }).unwrap()
}

/// Runs the worker on a thread of this process: its pipe client PID is this process, which the parent expects.
struct ThreadLauncher(mpsc::Sender<Result<u32, String>>);

impl WorkerLauncher for ThreadLauncher {
    fn launch(&self, _: &Path, args: &[String]) -> Result<LaunchedWorker, LaunchError> {
        assert_eq!(args[0], WORKER_SUBCOMMAND);
        let wa = WorkerArgs::parse(&args[1..]).map_err(LaunchError::Failed)?;
        let tx = self.0.clone();
        std::thread::spawn(move || {
            let _ = tx.send(serve(&wa, &Scripted, &|| 1_800_000_000));
        });
        Ok(LaunchedWorker { pid: std::process::id(), process: self_handle() })
    }
    fn elevates(&self) -> bool {
        false
    }
}

fn cfg() -> WorkerConfig {
    WorkerConfig { exe: PathBuf::from("unused.exe"), connect_timeout: Duration::from_secs(10), idle_seconds: 5 }
}

#[test]
fn pipe_names_and_arguments_are_closed_sets() {
    let n = new_pipe_name().unwrap();
    assert!(valid_pipe_name(&n) && n.len() > PIPE_PREFIX.len() + 32);
    assert_ne!(n, new_pipe_name().unwrap(), "unguessable per run");
    for bad in [r"\\server\pipe\cleanmanager-maint-1-ab", r"\\.\pipe\other", r"\\.\pipe\cleanmanager-maint-", r"\\.\pipe\cleanmanager-maint-1-AB", r"\\.\pipe\cleanmanager-maint-..\x"] {
        assert!(!valid_pipe_name(bad), "{bad}");
    }
    assert!(quote("--tasks").is_ok());
    for bad in ["a b", "a\"b", "", "x\ny"] {
        assert!(quote(bad).is_err(), "{bad:?}");
    }
    let args = worker_args(&n, 42, &grant(), 7);
    let parsed = WorkerArgs::parse(&args[1..]).unwrap();
    assert_eq!((parsed.parent_pid, parsed.idle_seconds, parsed.grant.clone()), (42, 7, grant()));
    // Extra or repeated options, foreign pipes and non-admin tasks are refused before any connection.
    let mut extra = args[1..].to_vec();
    extra.extend(["--command".into(), "cmd.exe".into()]);
    assert!(WorkerArgs::parse(&extra).is_err());
    let mut twice = args[1..].to_vec();
    twice.extend(["--tasks".into(), "windows.dns-flush@1.0.0".into()]);
    assert!(WorkerArgs::parse(&twice).is_err());
    let remote: Vec<String> = args[1..].iter().map(|a| if a == &n { r"\\evil\pipe\cleanmanager-maint-1-ab".to_owned() } else { a.clone() }).collect();
    assert!(WorkerArgs::parse(&remote).is_err());
    let user_task: Vec<String> = args[1..].iter().map(|a| if a.starts_with("windows.") { "windows.dns-flush@1.0.0".to_owned() } else { a.clone() }).collect();
    assert!(WorkerArgs::parse(&user_task).is_err());
}

#[test]
fn the_pipe_dacl_grants_only_the_current_user() {
    use windows_sys::Win32::Security::Authorization::{ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SE_KERNEL_OBJECT};
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;
    let sid = current_user_sid().unwrap();
    assert!(sid.starts_with("S-1-"));
    let name = new_pipe_name().unwrap();
    let pipe = create_server(&name).unwrap();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: valid pipe handle; the descriptor is freed below.
    let rc = unsafe { GetSecurityInfo(pipe.raw(), SE_KERNEL_OBJECT, DACL_SECURITY_INFORMATION, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), &mut sd) };
    assert_eq!(rc, 0);
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: valid descriptor; string freed below.
    assert_ne!(unsafe { ConvertSecurityDescriptorToStringSecurityDescriptorW(sd, SDDL_REVISION_1, DACL_SECURITY_INFORMATION, &mut text, std::ptr::null_mut()) }, 0);
    // SAFETY: NUL-terminated string from the conversion.
    let s = unsafe { String::from_utf16_lossy(std::slice::from_raw_parts(text, (0..).take_while(|&i| *text.add(i) != 0).count())) };
    // SAFETY: both allocated by the APIs above.
    unsafe {
        LocalFree(text.cast());
        LocalFree(sd);
    }
    assert!(s.starts_with("D:P"), "protected DACL: {s}");
    assert_eq!(s.matches("(A;").count(), 1, "exactly one allow ACE: {s}");
    assert!(s.ends_with(&format!(";;;{sid})")), "for the current user only: {s}");
    // A second server instance of the same name is refused (single instance, first-instance flag).
    assert!(create_server(&name).is_err());
}

#[test]
fn a_worker_runs_only_granted_tasks_over_the_pipe() {
    let (tx, rx) = mpsc::channel();
    let mut w = start_worker(&ThreadLauncher(tx), &cfg(), &grant()).unwrap();
    assert_eq!((w.worker_pid(), w.refused_clients), (std::process::id(), 0));
    let run = w.run(cm_core::maintenance::find(ids::TRIM).unwrap()).unwrap();
    assert_eq!((run.task_id.as_str(), run.outcome, run.code.as_str()), (ids::TRIM, TaskOutcome::Unchanged, "scripted"));
    // A task that is not granted: the worker refuses and ends the session.
    let err = w.run(cm_core::maintenance::find(ids::IMAGE_HEALTH).unwrap()).unwrap_err();
    assert!(err.contains("task_not_granted"), "{err}");
    assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), Ok(1), "exactly one task ran");
}

#[test]
fn a_worker_refuses_arbitrary_commands_and_other_plans() {
    let cases = [
        (r#"{"op":"exec","command":"cmd.exe /c whoami"}"#.to_owned(), "malformed_request"),
        ("powershell -NoProfile -Command Remove-Item C:\\ -Recurse".to_owned(), "malformed_request"),
        (WorkerRequest::Run { plan_digest: "f".repeat(64), task_id: ids::TRIM.into(), task_version: "1.0.0".into() }.to_line(), "wrong_plan"),
        (WorkerRequest::Run { plan_digest: DIGEST.into(), task_id: ids::DNS_FLUSH.into(), task_version: "1.0.0".into() }.to_line(), "task_not_granted"),
        (WorkerRequest::Run { plan_digest: DIGEST.into(), task_id: "windows.registry-clean".into(), task_version: "1.0.0".into() }.to_line(), "task_not_granted"),
    ];
    for (line, want) in cases {
        let (tx, rx) = mpsc::channel();
        let mut w = start_worker(&ThreadLauncher(tx), &cfg(), &grant()).unwrap();
        match w.send_raw(&line).unwrap() {
            WorkerReply::Refused { code, .. } => assert_eq!(code, want, "{line}"),
            other => panic!("{line}: {other:?}"),
        }
        assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), Ok(0), "{line}: nothing ran and the worker exited");
    }
}

#[test]
fn the_parent_refuses_a_client_that_is_not_its_worker() {
    let name = new_pipe_name().unwrap();
    let server = create_server(&name).unwrap();
    // The launched worker would be process 4 (never connects here); this process connects instead.
    let n2 = name.clone();
    let client = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        let n = wide(&n2);
        // SAFETY: NUL-terminated name; plain client open (this is the foreign process' view; no server check).
        let h = unsafe { CreateFileW(n.as_ptr(), FILE_GENERIC_READ | FILE_GENERIC_WRITE, 0, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_OVERLAPPED, std::ptr::null_mut()) };
        let mut p = Pipe::new(owned(h).expect("the DACL admits the same user")).unwrap();
        let _ = p.write_line(r#"{"op":"run","plan_digest":"x","task_id":"windows.trim-when-due","task_version":"1.0.0"}"#, Duration::from_secs(2));
        matches!(p.read_line(1024, Duration::from_secs(3)), Ok(ReadLine::Eof) | Err(_))
    });
    let r = accept(&server, 4, None, Duration::from_secs(2));
    assert!(matches!(r, Err(AcceptError::Timeout { refused }) if refused >= 1), "{r:?}");
    assert!(client.join().unwrap(), "the foreign client was disconnected without a reply");
}

#[test]
fn a_worker_refuses_a_server_that_is_not_its_parent() {
    let name = new_pipe_name().unwrap();
    let _server = create_server(&name).unwrap();
    let e = connect_client(&name, 4).err().unwrap();
    assert!(e.contains("not the parent"), "{e}");
    assert!(connect_client(r"\\server\pipe\cleanmanager-maint-1-ab", std::process::id()).is_err());
}

#[test]
fn a_declined_prompt_is_reported_as_declined() {
    assert_eq!(start_worker(&DecliningLauncher, &cfg(), &grant()).err(), Some(StartError::Declined));
    // A grant with a user-level task never reaches any launcher.
    let bad = WorkerGrant { tasks: vec![TaskRef { task_id: ids::DNS_FLUSH.into(), task_version: "1.0.0".into() }], ..grant() };
    assert!(matches!(start_worker(&DecliningLauncher, &cfg(), &bad), Err(StartError::Unavailable(_))));
}
