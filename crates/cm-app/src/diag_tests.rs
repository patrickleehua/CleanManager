//! Service tests against a local mock HTTP server (std `TcpListener`) and the test binary as a
//! fake CLI. No real provider is contacted and no real credential is read or printed.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cm_core::analysis::{EntryFacts, MemView};
use cm_core::capability::Platform;
use cm_core::diag::ground::CurrentObservation;
use cm_core::diag::{DiagState, Disclosure, FailureCode, NextAction, Origin, ProviderKind, Recommendation, Retention};
use cm_core::model::{CloudState, IdentityQuality, ObjectKind};
use cm_core::path::RawPath;
use cm_platform::credentials::{MemoryStore, Secret};

use super::*;

const KEY: &str = "sk-test-NOT-A-REAL-KEY-0123456789";

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

/// Minimal HTTP/1.1 server: one response per connection, records every request.
struct Mock {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

type Handler = Arc<dyn Fn(&Seen) -> (u16, String, Duration) + Send + Sync>;

fn mock(handler: Handler) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let (h, s) = (handler.clone(), s2.clone());
            std::thread::spawn(move || serve(stream, h, s));
        }
    });
    Mock { base, seen }
}

fn serve(stream: TcpStream, handler: Handler, seen: Arc<Mutex<Vec<Seen>>>) {
    let mut r = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if r.read_line(&mut line).is_err() || line.is_empty() {
        return;
    }
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or_default().to_owned(), parts.next().unwrap_or_default().to_owned());
    let mut headers = Vec::new();
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        r.read_line(&mut h).unwrap();
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_owned());
            if k == "content-length" {
                len = v.parse().unwrap_or(0);
            }
            headers.push((k, v));
        }
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).unwrap();
    let req = Seen { method, path, headers, body: String::from_utf8(body).unwrap() };
    seen.lock().unwrap().push(req.clone());
    let (status, text, delay) = handler(&req);
    std::thread::sleep(delay);
    let mut out = stream;
    let _ = write!(out, "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", text.len());
    let _ = out.write_all(text.as_bytes());
}

fn header<'a>(s: &'a Seen, k: &str) -> Option<&'a str> {
    s.headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

fn model_answer(rec: &str) -> String {
    serde_json::json!({
        "answer_version": "diag-answer/1", "summary": "这是浏览器缓存，可以进入计划审阅。", "purpose": "加速网页加载。", "recommendation": rec,
        "conditions": [], "deletion_impact": "首次加载变慢。", "rebuild_cost": "自动重建。",
        "claims": [{"text": "大小来自索引", "basis": "observation", "refs": ["ev_2"]}], "gaps": [],
        "alternatives": [{"action": "locate", "text": "先定位"}], "questions": [],
    })
    .to_string()
}

fn anthropic_ok(text: &str) -> String {
    serde_json::json!({"id": "msg_test", "type": "message", "role": "assistant", "model": "claude-opus-5-5", "stop_reason": "end_turn",
        "content": [{"type": "thinking", "thinking": "", "signature": "x"}, {"type": "text", "text": text}], "usage": {"input_tokens": 1, "output_tokens": 1}})
    .to_string()
}

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-app-diag-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// `C:\Users\alice\Videos\clip.mov` in a synthetic published scan.
fn context() -> ScanContext {
    let mut view = MemView { platform: Some(Platform::Windows), ..Default::default() };
    let names = ["C:", "Users", "alice", "Videos", "clip.mov"];
    for (i, n) in names.iter().enumerate() {
        let id = i as u64 + 1;
        view.names.insert(id, RawPath::windows(n.encode_utf16().collect()).unwrap());
        view.parents.insert(id, (id > 1).then_some(id - 1));
        if id > 1 {
            view.kids.entry(id - 1).or_default().push(id);
        }
        let file = id == names.len() as u64;
        view.facts.insert(id, EntryFacts {
            kind: if file { ObjectKind::File } else { ObjectKind::Directory },
            boundary: false,
            object_id: id,
            logical: file.then_some(2_000_000_000),
            allocated: file.then_some(2_000_000_000),
            volume_id: "vol".into(),
            file_id: format!("ntfs64:{id:016x}"),
            quality: IdentityQuality::Strong,
            cloud: CloudState::NotCloud,
            modified_unix: Some(1_780_000_000),
            link_count: file.then_some(1),
            skipped_here: 0,
        });
        view.identities.insert(("vol".into(), format!("ntfs64:{id:016x}")), id);
    }
    view.root = 1;
    ScanContext {
        view,
        scan_id: "s_test".into(),
        revision: 1,
        partial: false,
        observed_at: "2026-10-14T12:00:00Z".into(),
        set: None,
        catalog: None,
        hard: Some(Default::default()),
        user: Ok(cm_core::protect::UserProtection::empty(Platform::Windows)),
        home: Some(RawPath::windows("C:\\Users\\alice".encode_utf16().collect()).unwrap()),
    }
}

fn request() -> PrepareRequest {
    PrepareRequest {
        scan_id: "s_test".into(),
        entry_id: 5,
        origin: Origin::Disk,
        provider_id: None,
        disclosure: Disclosure::default(),
        question: Some("这个视频能删吗？".into()),
        statements: vec![],
        parent_diagnosis_id: None,
    }
}

fn service(dir: &Path) -> Service {
    let mut s = Service::new(dir, Box::new(MemoryStore::default()));
    s.limits = Limits { http_timeout: Duration::from_secs(5), cli_timeout: Duration::from_secs(20) };
    s
}

fn anthropic(base: &str) -> ProviderConfig {
    ProviderConfig { base_url: Some(base.into()), ..ProviderConfig::anthropic_default() }
}

#[test]
fn unconfigured_fails_immediately_and_local_facts_need_nothing() {
    let dir = scratch("unconfigured");
    let s = service(&dir);
    assert!(!s.status().unwrap().configured);
    let e = s.prepare(&context(), request()).unwrap_err();
    assert!(matches!(e, DiagAppError::NotConfigured(_)), "{e}");
    assert_eq!(e.code(), ErrorCode::Unsupported);
    // A configured Anthropic provider without a key is still not usable, and says so.
    let m = mock(Arc::new(|_| (200, "{}".into(), Duration::ZERO)));
    s.save_provider(anthropic(&m.base), true).unwrap();
    assert!(matches!(s.prepare(&context(), request()), Err(DiagAppError::NotConfigured(_))));
    assert!(m.seen.lock().unwrap().is_empty(), "nothing is sent without configuration");
}

#[test]
fn preview_is_exactly_what_is_sent_and_ids_are_shared_across_entries() {
    let dir = scratch("happy");
    let m = mock(Arc::new(|_| (200, anthropic_ok(&model_answer("reviewable")), Duration::ZERO)));
    let s = service(&dir);
    s.save_provider(anthropic(&m.base), true).unwrap();
    s.store_key("anthropic-default", &Secret::new(KEY.into()).unwrap()).unwrap();
    let st = s.status().unwrap();
    assert!(st.configured);
    assert_eq!(st.providers[0].credential, "present");
    assert!(!serde_json::to_string(&st).unwrap().contains(KEY), "status never returns the key");

    let p = s.prepare(&context(), request()).unwrap();
    let preview_text = serde_json::to_string(&p).unwrap();
    assert!(!preview_text.contains(KEY) && !preview_text.contains("alice"), "{preview_text}");
    assert!(p.contains_user_additions);
    assert!(m.seen.lock().unwrap().is_empty(), "preparing sends nothing");
    assert!(matches!(s.confirm(&p.preview_id, &"0".repeat(64)), Err(DiagAppError::Conflict(_))));
    // A wrong digest consumed nothing irreversible: the preview must be prepared again.
    let p = s.prepare(&context(), request()).unwrap();
    let d = s.confirm(&p.preview_id, &p.payload_digest).unwrap();
    assert_eq!(d.state, DiagState::Running);
    assert!(matches!(s.confirm(&p.preview_id, &p.payload_digest), Err(DiagAppError::Conflict(_))), "single use");
    let done = s.execute(&d.diagnosis_id).unwrap();
    assert_eq!(done.state, DiagState::Completed, "{:?}", done.failure);
    let seen = m.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!((seen[0].method.as_str(), seen[0].path.as_str()), ("POST", "/v1/messages"));
    assert_eq!(header(&seen[0], "x-api-key"), Some(KEY));
    assert_eq!(header(&seen[0], "anthropic-version"), Some("2023-06-01"));
    assert_eq!(seen[0].body, p.payload.body, "the body sent is byte-for-byte the previewed body");
    assert_eq!((p.payload_bytes, header(&seen[0], "content-length")), (seen[0].body.len() as u64, Some(p.payload_bytes.to_string().as_str())), "WFA-104");
    let r = done.result.as_ref().unwrap();
    // No rule matched this video: reviewable is not available from the model alone.
    assert_eq!(r.effective_recommendation, Recommendation::NeedsContext);
    assert!(!done.handoff.iter().any(|h| h.action == NextAction::ReviewPlan && h.available));
    // Same id from another process (CLI) sees the same record.
    let other = service(&dir);
    let again = other.get(&done.diagnosis_id, CurrentObservation::Same).unwrap();
    assert_eq!((again.diagnosis_id.as_str(), again.payload_digest.as_str()), (done.diagnosis_id.as_str(), done.payload_digest.as_str()));
    assert_eq!(other.list(10, None).unwrap().0[0].diagnosis_id, done.diagnosis_id);
    let export = serde_json::to_string(&other.export(&done.diagnosis_id).unwrap()).unwrap();
    assert!(!export.contains(KEY));
    // The object changed in a newer scan: the old explanation stays readable, marked expired.
    let stale = other.get(&done.diagnosis_id, CurrentObservation::Changed).unwrap();
    assert_eq!(stale.state, DiagState::Expired);
    assert!(stale.result.is_some());
}

#[test]
fn auth_failure_is_reported_without_trying_another_provider() {
    let dir = scratch("auth");
    let bad = mock(Arc::new(|_| (401, r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key sk-test-NOT-A-REAL-KEY-0123456789"}}"#.into(), Duration::ZERO)));
    let other = mock(Arc::new(|_| (200, "{}".into(), Duration::ZERO)));
    let s = service(&dir);
    s.save_provider(anthropic(&bad.base), true).unwrap();
    s.save_provider(
        ProviderConfig {
            provider_id: "local".into(),
            label: "local".into(),
            kind: ProviderKind::OpenaiCompatible,
            base_url: Some(format!("{}/v1", other.base)),
            model: "m".into(),
            executable: None,
            updated_at: String::new(),
        },
        false,
    )
    .unwrap();
    s.store_key("anthropic-default", &Secret::new(KEY.into()).unwrap()).unwrap();
    // The connectivity probe sends no body and no file data.
    let probe = s.probe("anthropic-default").unwrap();
    assert!(!probe.ok && !probe.sent_file_data);
    assert_eq!(probe.failure.as_ref().unwrap().code, FailureCode::AuthFailed);
    let p0 = bad.seen.lock().unwrap()[0].clone();
    assert_eq!((p0.method.as_str(), p0.path.as_str(), p0.body.as_str()), ("GET", "/v1/models/claude-opus-5-5", ""));
    let p = s.prepare(&context(), request()).unwrap();
    let d = s.confirm(&p.preview_id, &p.payload_digest).unwrap();
    let done = s.execute(&d.diagnosis_id).unwrap();
    assert_eq!(done.state, DiagState::Failed);
    let f = done.failure.unwrap();
    assert_eq!(f.code, FailureCode::AuthFailed);
    assert!(!f.message.contains(KEY), "keys are scrubbed from messages: {}", f.message);
    assert!(other.seen.lock().unwrap().is_empty(), "no automatic fallback");
    assert_eq!(bad.seen.lock().unwrap().len(), 2, "one probe and one run, no retry");
    // Revocation: the key is gone and preparing fails until a new key is stored.
    assert!(s.forget_key("anthropic-default").unwrap());
    assert!(matches!(s.prepare(&context(), request()), Err(DiagAppError::NotConfigured(_))));
}

#[test]
fn timeouts_oversize_invalid_and_refused_answers_fail_without_results() {
    let dir = scratch("failures");
    let cases: Vec<(&str, Handler, FailureCode)> = vec![
        ("slow", Arc::new(|_| (200, anthropic_ok(&model_answer("keep")), Duration::from_secs(3))), FailureCode::Timeout),
        ("huge", Arc::new(|_| (200, "x".repeat(2 * 1024 * 1024), Duration::ZERO)), FailureCode::OutputTooLarge),
        ("prose", Arc::new(|_| (200, anthropic_ok("Sure! It is safe to delete."), Duration::ZERO)), FailureCode::InvalidStructure),
        ("refusal", Arc::new(|_| (200, r#"{"content":[],"stop_reason":"refusal"}"#.into(), Duration::ZERO)), FailureCode::Refused),
        ("cut", Arc::new(|_| (200, r#"{"content":[{"type":"text","text":"{\"answer_version\""}],"stop_reason":"max_tokens"}"#.into(), Duration::ZERO)), FailureCode::InvalidStructure),
        ("rate", Arc::new(|_| (429, r#"{"error":{"message":"slow down"}}"#.into(), Duration::ZERO)), FailureCode::RateLimited),
    ];
    for (name, handler, code) in cases {
        let m = mock(handler);
        let mut s = service(&dir);
        s.limits.http_timeout = Duration::from_secs(1);
        s.save_provider(anthropic(&m.base), true).unwrap();
        s.store_key("anthropic-default", &Secret::new(KEY.into()).unwrap()).unwrap();
        let p = s.prepare(&context(), request()).unwrap();
        let d = s.confirm(&p.preview_id, &p.payload_digest).unwrap();
        let done = s.execute(&d.diagnosis_id).unwrap();
        assert_eq!(done.state, DiagState::Failed, "{name}");
        assert_eq!(done.failure.as_ref().unwrap().code, code, "{name}: {:?}", done.failure);
        assert!(done.result.is_none(), "{name}: a partial or invalid answer is never a result");
    }
}

#[test]
fn cancel_wins_and_late_answers_are_dropped() {
    let dir = scratch("cancel");
    let m = mock(Arc::new(|_| (200, anthropic_ok(&model_answer("keep")), Duration::from_millis(1500))));
    let s = Arc::new(service(&dir));
    s.save_provider(anthropic(&m.base), true).unwrap();
    s.store_key("anthropic-default", &Secret::new(KEY.into()).unwrap()).unwrap();
    let p = s.prepare(&context(), request()).unwrap();
    let d = s.confirm(&p.preview_id, &p.payload_digest).unwrap();
    let (s2, id) = (s.clone(), d.diagnosis_id.clone());
    let worker = std::thread::spawn(move || s2.execute(&id).unwrap());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(s.cancel(&d.diagnosis_id).unwrap().state, DiagState::Cancelled);
    assert_eq!(worker.join().unwrap().state, DiagState::Cancelled);
    std::thread::sleep(Duration::from_millis(1500));
    let after = s.get(&d.diagnosis_id, CurrentObservation::Same).unwrap();
    assert_eq!(after.state, DiagState::Cancelled, "the late answer did not overwrite the cancellation");
    assert!(after.result.is_none());
    // A discarded preview sends nothing.
    let before = m.seen.lock().unwrap().len();
    let p = s.prepare(&context(), request()).unwrap();
    assert!(s.discard(&p.preview_id));
    assert_eq!(m.seen.lock().unwrap().len(), before);
}

#[test]
fn changing_the_destination_needs_a_new_disclosure() {
    let dir = scratch("dest");
    let m = mock(Arc::new(|_| (200, anthropic_ok(&model_answer("keep")), Duration::ZERO)));
    let s = service(&dir);
    s.save_provider(anthropic(&m.base), true).unwrap();
    s.store_key("anthropic-default", &Secret::new(KEY.into()).unwrap()).unwrap();
    let p = s.prepare(&context(), request()).unwrap();
    s.save_provider(ProviderConfig { model: "claude-sonnet-5-5".into(), ..anthropic(&m.base) }, true).unwrap();
    assert!(matches!(s.confirm(&p.preview_id, &p.payload_digest), Err(DiagAppError::Conflict(_))));
    assert!(m.seen.lock().unwrap().is_empty());
}

#[test]
fn local_openai_compatible_endpoint_without_a_key() {
    let dir = scratch("local");
    let m = mock(Arc::new(|r| {
        if r.path.ends_with("/models") {
            (200, r#"{"data":[{"id":"local-model"}]}"#.into(), Duration::ZERO)
        } else {
            (200, serde_json::json!({"choices": [{"message": {"content": model_answer("keep")}, "finish_reason": "stop"}]}).to_string(), Duration::ZERO)
        }
    }));
    let s = service(&dir);
    let cfg = ProviderConfig {
        provider_id: "local".into(),
        label: "本机模型".into(),
        kind: ProviderKind::OpenaiCompatible,
        base_url: Some(format!("{}/v1", m.base)),
        model: "local-model".into(),
        executable: None,
        updated_at: String::new(),
    };
    s.save_provider(cfg, true).unwrap();
    assert!(s.status().unwrap().configured);
    assert!(s.probe("local").unwrap().ok);
    let p = s.prepare(&context(), request()).unwrap();
    assert_eq!(p.payload.transport, "http_post_loopback");
    let d = s.confirm(&p.preview_id, &p.payload_digest).unwrap();
    let done = s.execute(&d.diagnosis_id).unwrap();
    assert_eq!(done.state, DiagState::Completed, "{:?}", done.failure);
    let seen = m.seen.lock().unwrap().clone();
    let post = seen.iter().find(|x| x.method == "POST").unwrap();
    assert_eq!(post.path, "/v1/chat/completions");
    assert!(header(post, "authorization").is_none(), "no key configured, none sent");
    // WFA-104: the size the preview shows is the request body's size on the wire, byte for byte (UTF-8 bytes, not
    // characters: the question and the evidence text are Chinese, so the two differ).
    assert_eq!(post.body, p.payload.body, "the body sent is the previewed body");
    assert_eq!(p.payload_bytes, post.body.len() as u64, "preview size = bytes received");
    assert_eq!(header(post, "content-length"), Some(p.payload_bytes.to_string().as_str()), "preview size = Content-Length");
    assert!(p.payload.body.chars().count() < post.body.len(), "multi-byte text: characters are not bytes");
    // Plain http to a non-loopback host is refused at configuration time.
    let bad = ProviderConfig { base_url: Some("http://example.com/v1".into()), provider_id: "remote".into(), ..ProviderConfig::anthropic_default() };
    assert!(matches!(s.save_provider(bad, false), Err(DiagAppError::InvalidArgument(_))));
}

#[test]
fn retention_save_remove_and_audit_independence() {
    let dir = scratch("retention");
    let stores = cm_store::Stores::open(&dir).unwrap();
    drop(stores);
    let audit_before = std::fs::read(dir.join(cm_store::AUDIT_FILE)).unwrap();
    let m = mock(Arc::new(|_| (200, anthropic_ok(&model_answer("keep")), Duration::ZERO)));
    let s = service(&dir);
    s.save_provider(anthropic(&m.base), true).unwrap();
    s.store_key("anthropic-default", &Secret::new(KEY.into()).unwrap()).unwrap();
    let mut ids = vec![];
    for _ in 0..2 {
        let p = s.prepare(&context(), request()).unwrap();
        let d = s.confirm(&p.preview_id, &p.payload_digest).unwrap();
        ids.push(s.execute(&d.diagnosis_id).unwrap().diagnosis_id);
    }
    assert_eq!(s.save(&ids[0]).unwrap().retention, Retention::Saved);
    // Session records go after their window; saved ones stay.
    assert_eq!(s.purge_session(now_unix() + budget::SESSION_RETENTION_SECONDS + 10).unwrap(), 1);
    assert!(s.get(&ids[0], CurrentObservation::Same).is_ok());
    assert!(matches!(s.get(&ids[1], CurrentObservation::Same), Err(DiagAppError::NotFound(_))));
    assert!(s.remove(&ids[0]).unwrap());
    assert_eq!(std::fs::read(dir.join(cm_store::AUDIT_FILE)).unwrap(), audit_before, "removing diagnoses never touches the audit store");
    assert!(!std::fs::read_to_string(dir.join(CONFIG_FILE)).unwrap().contains(KEY), "settings file never holds the key");
}

// ---------------------------------------------------------------------------------------------
// CLI adapter

/// Fake CLI used by the runner tests: the test binary re-invoked with `CM_FAKE_CLI_MODE`.
#[test]
fn fake_cli_child() {
    let Ok(mode) = std::env::var("CM_FAKE_CLI_MODE") else { return };
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    match mode.as_str() {
        "echo" => {
            let cwd = std::env::current_dir().unwrap();
            let entries = std::fs::read_dir(&cwd).map(|r| r.count()).unwrap_or(99);
            let out = serde_json::json!({"argv": std::env::args().skip(1).collect::<Vec<_>>(), "stdin": input, "cwd_entries": entries,
                "has_secret_env": std::env::var("CM_TEST_SECRET").is_ok()});
            println!("FAKE:{out}");
        }
        "sleep" => std::thread::sleep(Duration::from_secs(30)),
        "flood" => {
            let chunk = "y".repeat(64 * 1024);
            for _ in 0..200 {
                print!("{chunk}");
            }
        }
        _ => {
            eprintln!("boom sk-ant-api03-SHOULD-NOT-APPEAR-1234");
            std::process::exit(3);
        }
    }
    let _ = std::io::stdout().flush();
    std::process::exit(0);
}

fn fake(mode: &str, extra: &[&str], timeout: Duration, max: usize, cancel: &Arc<AtomicBool>) -> std::result::Result<cli::Output, RunError> {
    let exe = std::env::current_exe().unwrap();
    let mut argv: Vec<String> = vec!["--exact".into(), "diag::tests::fake_cli_child".into(), "--nocapture".into(), "--test-threads=1".into()];
    argv.extend(extra.iter().map(|s| s.to_string()));
    cli::run(&exe, &argv, "证据 JSON 在 stdin".as_bytes(), timeout, max, cancel, &[("CM_FAKE_CLI_MODE".into(), mode.into())])
}

#[test]
fn cli_runner_passes_argv_verbatim_without_a_shell() {
    // SAFETY-relevant: a secret in the parent environment is not inherited (env is cleared).
    let never = Arc::new(AtomicBool::new(false));
    let marker = std::env::temp_dir().join(format!("cm-diag-pwned-{}", std::process::id()));
    let evil = format!("a b & echo pwned > {} | rm -rf $HOME `whoami`", marker.display());
    let out = fake("echo", &[evil.as_str(), ""], Duration::from_secs(30), 1 << 20, &never).unwrap();
    assert_eq!(out.status, Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find_map(|l| l.split_once("FAKE:").map(|(_, j)| j)).unwrap_or_else(|| panic!("fake output: {text} {}", out.stderr));
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    let argv: Vec<String> = serde_json::from_value(v["argv"].clone()).unwrap();
    assert!(argv.contains(&evil), "metacharacters arrive as one literal argument");
    assert!(argv.contains(&String::new()), "an empty argument stays an empty argument");
    assert_eq!(v["stdin"], "证据 JSON 在 stdin");
    assert_eq!(v["cwd_entries"], 0, "the working directory is an empty sandbox");
    assert!(!marker.exists(), "nothing was interpreted by a shell");
}

#[test]
fn cli_runner_enforces_timeout_output_limit_cancel_and_exit_codes() {
    let never = Arc::new(AtomicBool::new(false));
    let t = std::time::Instant::now();
    assert!(matches!(fake("sleep", &[], Duration::from_millis(800), 1 << 20, &never), Err(RunError::Failed(f)) if f.code == FailureCode::Timeout));
    assert!(t.elapsed() < Duration::from_secs(15), "the process is killed at the limit");
    assert!(matches!(fake("flood", &[], Duration::from_secs(30), 256 * 1024, &never), Err(RunError::Failed(f)) if f.code == FailureCode::OutputTooLarge));
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        c2.store(true, Ordering::Release);
    });
    assert!(matches!(fake("sleep", &[], Duration::from_secs(30), 1 << 20, &cancel), Err(RunError::Cancelled)));
    let out = fake("fail", &[], Duration::from_secs(30), 1 << 20, &never).unwrap();
    assert_eq!(out.status, Some(3));
    let f = cli::claude_result(&out).unwrap_err();
    assert_eq!(f.code, FailureCode::ProcessFailed);
    assert!(!f.message.contains("SHOULD-NOT-APPEAR"), "{}", f.message);
}

#[test]
fn unconstrainable_clis_are_never_started() {
    let dir = scratch("cli");
    let s = service(&dir);
    let exe = std::env::current_exe().unwrap().display().to_string();
    let codex = ProviderConfig {
        provider_id: "codex".into(),
        label: "Codex".into(),
        kind: ProviderKind::CodexCli,
        base_url: None,
        model: "default".into(),
        executable: Some(exe.clone()),
        updated_at: String::new(),
    };
    s.save_provider(codex, true).unwrap();
    assert_eq!(s.status().unwrap().providers[0].support.state, "unsupported");
    assert!(matches!(s.prepare(&context(), request()), Err(DiagAppError::Unsupported(_))));
    // A Claude Code entry pointing at something without the constraint flags is unsupported.
    let fake_claude = ProviderConfig { provider_id: "claude".into(), kind: ProviderKind::ClaudeCodeCli, ..s.config().unwrap().providers[0].clone() };
    s.save_provider(fake_claude, true).unwrap();
    assert!(matches!(s.prepare(&context(), request()), Err(DiagAppError::Unsupported(_))));
    // Scripts would run through a shell interpreter.
    let script = dir.join("claude.cmd");
    std::fs::write(&script, "@echo off\r\n").unwrap();
    if cfg!(windows) {
        assert!(cli::validate_executable(&script).is_err());
    }
    assert!(cli::validate_executable(Path::new("claude.exe")).is_err(), "relative paths are refused");
}

#[test]
fn claude_argv_disables_every_tool() {
    let check = cli::CliCheck { supported: true, version: None, missing: vec![], optional: vec!["--safe-mode".into()], reason: String::new() };
    let a = cli::claude_argv("claude-opus-5-5", "SYS", &check);
    let at = |flag: &str| a.iter().position(|x| x == flag).map(|i| a[i + 1].clone());
    assert_eq!(at("--tools").as_deref(), Some(""));
    assert_eq!(at("--permission-mode").as_deref(), Some("dontAsk"));
    assert!(a.contains(&"--strict-mcp-config".to_owned()) && a.contains(&"--restricted".to_owned()) && a.contains(&"--no-session-persistence".to_owned()));
    assert!(at("--disallowedTools").unwrap().contains("Bash"));
    assert_eq!(a.last().unwrap(), "SYS");
    let ok = cli::Output { status: Some(0), stdout: br#"{"type":"result","subtype":"success","is_error":false,"result":"{}"}"#.to_vec(), stderr: String::new() };
    assert_eq!(cli::claude_result(&ok).unwrap(), "{}");
    let err = cli::Output { status: Some(0), stdout: br#"{"type":"result","is_error":true,"result":"Not logged in"}"#.to_vec(), stderr: String::new() };
    assert_eq!(cli::claude_result(&err).unwrap_err().code, FailureCode::ProviderError);
}

#[test]
fn scrub_removes_keys() {
    let s = Secret::new(KEY.into()).unwrap();
    let t = http::scrub(&format!("bad key {KEY}, other sk-ant-api03-zzzzzzzzzzzz"), Some(&s));
    assert!(!t.contains(KEY) && !t.contains("sk-ant-api03-zzzz"), "{t}");
}

/// Machine check (no data sent): `CM_DIAG_CLI=<path to claude.exe or codex.exe> cargo test -p cm-app
/// installed_cli_verification -- --ignored --nocapture` prints what the adapter decides.
#[test]
#[ignore]
fn installed_cli_verification() {
    let Some(path) = std::env::var_os("CM_DIAG_CLI") else { return };
    let path = PathBuf::from(path);
    let kind = if path.to_string_lossy().to_lowercase().contains("codex") { ProviderKind::CodexCli } else { ProviderKind::ClaudeCodeCli };
    let c = cli::verify(kind, &path);
    println!("CLI {} -> {:?}", path.display(), c);
    if c.supported {
        println!("argv: {:?}", cli::claude_argv("default", "<system prompt>", &c));
    }
}
