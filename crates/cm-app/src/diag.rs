//! Optional AI diagnosis service shared by the desktop and the CLI (AID-01..13, task 15). It
//! wires the pure `cm_core::diag` contract to the index, the protection layers, the read-only
//! app inventory, the system credential store and the provider adapters:
//!
//! 1. `prepare` builds the evidence package and the exact payload and returns a preview; nothing
//!    is sent. Missing configuration fails at once (never waits for input).
//! 2. `confirm` accepts exactly that preview (id + digest) and records a `running` diagnosis.
//! 3. `execute` sends it through the configured provider only, then parses, grounds and stores
//!    the result. A cancelled run keeps its state; a late answer is dropped.
//!
//! Diagnoses are stored in `<store>/diagnoses/<id>.json` (session retention unless saved),
//! apart from `audit.db`: removing a diagnosis never touches operation history. Provider
//! settings (no secrets) are in `<store>/diag-providers.json`; keys are only in the OS store.

#[path = "diag_cli.rs"]
pub mod cli;
#[path = "diag_http.rs"]
pub mod http;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use cm_core::analysis::{CandidateSet, CatalogSet, IndexView, MemView, UserList};
use cm_core::diag::evidence::{CandidateFacts, EvidenceInput, OwnershipFacts, ProtectionInput, ScanFacts, build, object_fingerprint};
use cm_core::diag::ground::{CurrentObservation, freshness, ground, handoff, parse_answer};
use cm_core::diag::prompt::{SYSTEM_PROMPT, validate_base_url};
use cm_core::diag::{
    CoreFacts, Destination, DiagState, Diagnosis, DiagnosisSummary, Disclosure, Failure, FailureCode, Freshness, FreshnessState, Origin, Preview, PreviewState,
    ProviderKind, Retention, TransportPayload, UserStatement, budget, valid_diagnosis_id, valid_preview_id,
};
use cm_core::error::ErrorCode;
use cm_core::path::RawPath;
use cm_core::plan::parse_utc;
use cm_core::time::{now_utc, utc_from_unix};
use cm_platform::credentials::{CredentialError, CredentialStore, Secret, target_for};
use cm_store::Stores;
use serde::{Deserialize, Serialize};

use self::http::RunError;

pub const CONFIG_FILE: &str = "diag-providers.json";
pub const DIAG_DIR: &str = "diagnoses";
pub const DEFAULT_ANTHROPIC_BASE: &str = "https://api.anthropic.com";
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
const MAX_PROVIDERS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiagAppError {
    /// No provider, no key, or the store is unavailable: shown with a configuration entry.
    #[error("not configured: {0}")]
    NotConfigured(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("not found: {0}")]
    NotFound(String),
    /// The preview changed or expired; review it again.
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("store: {0}")]
    Store(String),
}

impl DiagAppError {
    pub fn code(&self) -> ErrorCode {
        match self {
            DiagAppError::NotConfigured(_) | DiagAppError::Unsupported(_) => ErrorCode::Unsupported,
            DiagAppError::InvalidArgument(_) | DiagAppError::NotFound(_) => ErrorCode::InvalidArgument,
            DiagAppError::Conflict(_) => ErrorCode::Conflict,
            DiagAppError::Store(_) => ErrorCode::IoError,
        }
    }

    /// Stable machine reason.
    pub fn reason(&self) -> &'static str {
        match self {
            DiagAppError::NotConfigured(_) => "diag_not_configured",
            DiagAppError::Unsupported(_) => "diag_unsupported",
            DiagAppError::InvalidArgument(_) => "invalid_argument",
            DiagAppError::NotFound(_) => "diag_not_found",
            DiagAppError::Conflict(_) => "diag_preview_changed",
            DiagAppError::Store(_) => "diag_store",
        }
    }
}

type Result<T> = std::result::Result<T, DiagAppError>;

fn store_err(e: impl std::fmt::Display) -> DiagAppError {
    DiagAppError::Store(e.to_string())
}

// ---------------------------------------------------------------------------------------------
// Provider configuration (no secrets)

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub provider_id: String,
    pub label: String,
    pub kind: ProviderKind,
    /// API base URL (`https://api.anthropic.com`, `http://127.0.0.1:11434/v1`); CLI: none.
    pub base_url: Option<String>,
    pub model: String,
    /// Absolute path of the CLI executable (chosen in a native dialog or typed in the CLI config).
    pub executable: Option<String>,
    pub updated_at: String,
}

impl ProviderConfig {
    pub fn anthropic_default() -> Self {
        ProviderConfig {
            provider_id: "anthropic-default".into(),
            label: "Anthropic API".into(),
            kind: ProviderKind::Anthropic,
            base_url: Some(DEFAULT_ANTHROPIC_BASE.into()),
            model: DEFAULT_MODEL.into(),
            executable: None,
            updated_at: now_utc(),
        }
    }

    pub fn validate(&self) -> std::result::Result<(), String> {
        target_for(&self.provider_id).map_err(|_| "provider_id must be 1-40 characters of a-z, 0-9 or -".to_owned())?;
        let clean = |s: &str, max: usize| !s.trim().is_empty() && s.chars().count() <= max && !s.chars().any(char::is_control);
        if !clean(&self.label, 80) {
            return Err("label must be 1-80 characters".into());
        }
        if !clean(&self.model, 120) || self.model.contains(char::is_whitespace) {
            return Err("model must be 1-120 characters without spaces".into());
        }
        match self.kind.mode() {
            cm_core::diag::Mode::Api => {
                let base = self.base_url.as_deref().ok_or("an API provider needs an endpoint")?;
                validate_base_url(base)?;
                if self.executable.is_some() {
                    return Err("an API provider has no executable".into());
                }
            }
            cm_core::diag::Mode::Cli => {
                if self.base_url.is_some() {
                    return Err("a CLI provider has no endpoint".into());
                }
                let exe = self.executable.as_deref().ok_or("a CLI provider needs an executable")?;
                if !Path::new(exe).is_absolute() {
                    return Err("the CLI must be an absolute path".into());
                }
            }
        }
        Ok(())
    }

    pub fn endpoint(&self) -> Option<String> {
        self.base_url.as_deref().map(|b| http::endpoint(self.kind, b))
    }

    /// Anthropic always needs a key; OpenAI-compatible endpoints may be keyless (local servers).
    pub fn key_required(&self) -> bool {
        self.kind == ProviderKind::Anthropic
    }

    pub fn destination(&self) -> Destination {
        Destination {
            provider_id: self.provider_id.clone(),
            label: self.label.clone(),
            kind: self.kind,
            mode: self.kind.mode(),
            endpoint: self.endpoint(),
            executable: self.executable.clone(),
            model: self.model.clone(),
            template: cm_core::diag::prompt::TEMPLATE_VERSION.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagConfig {
    pub version: u32,
    pub default_provider_id: Option<String>,
    pub providers: Vec<ProviderConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Support {
    /// `supported`, `unsupported` or `unverified`.
    pub state: &'static str,
    pub reason: Option<String>,
}

/// One provider as shown in settings and `diag config` (contract `provider_status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderStatus {
    pub provider_id: String,
    pub label: String,
    pub kind: ProviderKind,
    pub mode: cm_core::diag::Mode,
    pub endpoint: Option<String>,
    pub executable: Option<String>,
    pub model: String,
    /// `present`, `absent`, `not_required` or `unavailable`; the key itself is never returned.
    pub credential: &'static str,
    pub support: Support,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigStatus {
    pub configured: bool,
    pub credential_store: &'static str,
    pub providers: Vec<ProviderStatus>,
    pub default_provider_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProbeReport {
    pub provider_id: String,
    pub ok: bool,
    pub detail: String,
    pub failure: Option<Failure>,
    pub checked_at: String,
    /// Always true: a probe carries no file information.
    pub sent_file_data: bool,
}

fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, text).map_err(store_err)?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        store_err(e)
    })
}

// ---------------------------------------------------------------------------------------------
// Object context

/// What the evidence builder needs about a scan, loaded once per object.
pub struct ScanContext {
    pub view: MemView,
    pub scan_id: String,
    pub revision: u32,
    pub partial: bool,
    pub observed_at: String,
    pub set: Option<CandidateSet>,
    /// Catalog facts of an analysis made in this process (desktop); `None` from the store.
    pub catalog: Option<CatalogSet>,
    pub hard: Option<cm_core::protect::HardDenySet>,
    pub user: std::result::Result<cm_core::protect::UserProtection, String>,
    pub home: Option<RawPath>,
}

impl ScanContext {
    /// Load a published scan with the latest stored candidate set and every protection layer.
    pub fn load(stores: &Stores, scan_id: &str) -> Result<ScanContext> {
        let status = stores.index.scan_status(scan_id).map_err(|e| DiagAppError::NotFound(format!("scan {scan_id}: {e}")))?;
        if !status.published {
            return Err(DiagAppError::InvalidArgument("the scan is not published yet".into()));
        }
        let view = stores.index.view(scan_id).map_err(store_err)?;
        let set = match stores.index.latest_candidate_set(scan_id).map_err(store_err)? {
            Some((id, _)) => stores.index.candidate_set(&id).ok().and_then(|v| serde_json::from_value::<CandidateSet>(v).ok()),
            None => None,
        };
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).and_then(|h| cm_platform::paths::raw_path(&h).ok());
        Ok(ScanContext {
            view,
            scan_id: scan_id.into(),
            revision: status.revision,
            partial: status.completeness.as_deref() != Some("verified_scope"),
            observed_at: status.finished_at.clone().unwrap_or(status.started_at.clone()),
            set,
            catalog: None,
            hard: Some(cm_platform::protection::hard_deny_set()),
            user: cm_platform::protection::load(&stores.dir).map_err(|e| e.to_string()),
            home,
        })
    }
}

/// Applications whose recorded location is or contains `path` (read-only inventory).
pub fn ownership_for(path: &RawPath) -> OwnershipFacts {
    #[cfg(windows)]
    {
        let (instances, _issues) = cm_platform::windows::apps::inventory();
        let report = cm_core::apps::associate(&instances, &cm_core::model::Timestamp(now_utc()));
        let a = cm_core::apps::assess_directory(path, None, cm_core::apps::InstallRecordStatus::NotFound, &instances, &report.associations);
        match a.ownership {
            cm_core::apps::Ownership::Associated { instance_ids, shared } => OwnershipFacts {
                checked: true,
                apps: instance_ids
                    .iter()
                    .map(|id| (id.clone(), instances.iter().find(|i| &i.instance_id == id).map(|i| i.name.clone()).unwrap_or_default()))
                    .collect(),
                shared,
            },
            cm_core::apps::Ownership::Unknown { .. } => OwnershipFacts { checked: true, apps: vec![], shared: false },
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        OwnershipFacts::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareRequest {
    pub scan_id: String,
    pub entry_id: u64,
    pub origin: Origin,
    pub provider_id: Option<String>,
    pub disclosure: Disclosure,
    pub question: Option<String>,
    pub statements: Vec<UserStatement>,
    pub parent_diagnosis_id: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Service

struct Pending {
    preview: Preview,
    facts: CoreFacts,
    config: ProviderConfig,
    created_unix: i64,
}

struct Ready {
    payload: TransportPayload,
    config: ProviderConfig,
}

/// Limits for one run. Tests shorten them.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub http_timeout: Duration,
    pub cli_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { http_timeout: Duration::from_secs(180), cli_timeout: Duration::from_secs(240) }
    }
}

pub struct Service {
    store_dir: PathBuf,
    creds: Box<dyn CredentialStore>,
    pending: Mutex<HashMap<String, Pending>>,
    ready: Mutex<HashMap<String, Ready>>,
    cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
    write: Mutex<()>,
    pub limits: Limits,
    /// Test hook: extra environment for CLI runs.
    pub cli_env: Vec<(String, String)>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn new_id(prefix: &str) -> String {
    use std::hash::{BuildHasher, Hasher};
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(nanos);
    h.write_u32(std::process::id());
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    let a = h.finish();
    h.write_u64(a);
    format!("{prefix}_{:016x}{:04x}", a, h.finish() & 0xffff)
}

fn credential_failure(e: &CredentialError) -> Failure {
    match e {
        CredentialError::Unsupported(why) => http::failure(FailureCode::CredentialStoreUnavailable, format!("系统凭据存储不可用：{why}")),
        other => http::failure(FailureCode::CredentialStoreUnavailable, format!("系统凭据存储错误：{other}")),
    }
}

impl Service {
    pub fn new(store_dir: &Path, creds: Box<dyn CredentialStore>) -> Self {
        let s = Service {
            store_dir: store_dir.to_path_buf(),
            creds,
            pending: Mutex::new(HashMap::new()),
            ready: Mutex::new(HashMap::new()),
            cancels: Mutex::new(HashMap::new()),
            write: Mutex::new(()),
            limits: Limits::default(),
            cli_env: Vec::new(),
        };
        let _ = s.purge_session(now_unix());
        s
    }

    fn diag_dir(&self) -> Result<PathBuf> {
        let d = self.store_dir.join(DIAG_DIR);
        std::fs::create_dir_all(&d).map_err(store_err)?;
        Ok(d)
    }

    fn record_path(&self, id: &str) -> Result<PathBuf> {
        if !valid_diagnosis_id(id) {
            return Err(DiagAppError::InvalidArgument("malformed diagnosis id".into()));
        }
        Ok(self.diag_dir()?.join(format!("{id}.json")))
    }

    // -- configuration --------------------------------------------------------------------

    pub fn config(&self) -> Result<DiagConfig> {
        match std::fs::read_to_string(self.store_dir.join(CONFIG_FILE)) {
            Ok(text) => {
                let c: DiagConfig = serde_json::from_str(&text).map_err(|e| DiagAppError::Store(format!("{CONFIG_FILE} is invalid: {e}")))?;
                if c.version != 1 {
                    return Err(DiagAppError::Store(format!("{CONFIG_FILE} has an unsupported version")));
                }
                Ok(c)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DiagConfig { version: 1, default_provider_id: None, providers: vec![] }),
            Err(e) => Err(store_err(e)),
        }
    }

    fn save_config(&self, c: &DiagConfig) -> Result<()> {
        std::fs::create_dir_all(&self.store_dir).map_err(store_err)?;
        write_atomic(&self.store_dir.join(CONFIG_FILE), &serde_json::to_string_pretty(c).expect("serializable"))
    }

    fn credential_state(&self, p: &ProviderConfig) -> &'static str {
        if p.kind.mode() == cm_core::diag::Mode::Cli {
            return "not_required";
        }
        match target_for(&p.provider_id).and_then(|t| self.creds.exists(&t)) {
            Ok(true) => "present",
            Ok(false) => "absent",
            Err(_) => "unavailable",
        }
    }

    fn support(&self, p: &ProviderConfig) -> Support {
        match p.kind {
            ProviderKind::CodexCli => Support { state: "unsupported", reason: Some(cli::CODEX_UNSUPPORTED.into()) },
            ProviderKind::ClaudeCodeCli => match p.executable.as_deref().map(Path::new).map(cli::validate_executable) {
                Some(Ok(())) => Support { state: "unverified", reason: Some("checked with --help when a diagnosis is prepared".into()) },
                Some(Err(e)) => Support { state: "unsupported", reason: Some(e) },
                None => Support { state: "unsupported", reason: Some("no executable".into()) },
            },
            _ if self.credential_state(p) == "unavailable" => Support { state: "unsupported", reason: Some("system credential store unavailable".into()) },
            _ => Support { state: "supported", reason: None },
        }
    }

    pub fn status(&self) -> Result<ConfigStatus> {
        let c = self.config()?;
        let providers: Vec<ProviderStatus> = c
            .providers
            .iter()
            .map(|p| ProviderStatus {
                provider_id: p.provider_id.clone(),
                label: p.label.clone(),
                kind: p.kind,
                mode: p.kind.mode(),
                endpoint: p.endpoint(),
                executable: p.executable.clone(),
                model: p.model.clone(),
                credential: self.credential_state(p),
                support: self.support(p),
                updated_at: p.updated_at.clone(),
            })
            .collect();
        let configured = providers
            .iter()
            .any(|p| p.support.state != "unsupported" && (p.credential != "absent" || p.kind == ProviderKind::OpenaiCompatible) && p.credential != "unavailable");
        Ok(ConfigStatus { configured, credential_store: self.creds.backend(), providers, default_provider_id: c.default_provider_id })
    }

    /// Add or replace a provider's settings. Changing a provider invalidates its open previews.
    pub fn save_provider(&self, mut p: ProviderConfig, make_default: bool) -> Result<ConfigStatus> {
        p.updated_at = now_utc();
        if let Some(b) = &p.base_url {
            p.base_url = Some(b.trim_end_matches('/').to_owned());
        }
        p.validate().map_err(DiagAppError::InvalidArgument)?;
        let mut c = self.config()?;
        c.version = 1;
        let count = c.providers.len();
        match c.providers.iter_mut().find(|x| x.provider_id == p.provider_id) {
            Some(slot) => *slot = p.clone(),
            None if count >= MAX_PROVIDERS => return Err(DiagAppError::InvalidArgument("too many providers".into())),
            None => c.providers.push(p.clone()),
        }
        if make_default || c.default_provider_id.is_none() {
            c.default_provider_id = Some(p.provider_id.clone());
        }
        self.save_config(&c)?;
        lock(&self.pending).retain(|_, x| x.config.provider_id != p.provider_id);
        self.status()
    }

    /// Remove a provider and revoke its stored key.
    pub fn remove_provider(&self, provider_id: &str) -> Result<ConfigStatus> {
        let mut c = self.config()?;
        let before = c.providers.len();
        c.providers.retain(|p| p.provider_id != provider_id);
        if c.providers.len() == before {
            return Err(DiagAppError::NotFound(format!("provider {provider_id}")));
        }
        if c.default_provider_id.as_deref() == Some(provider_id) {
            c.default_provider_id = c.providers.first().map(|p| p.provider_id.clone());
        }
        self.save_config(&c)?;
        let _ = self.forget_key(provider_id);
        lock(&self.pending).retain(|_, x| x.config.provider_id != provider_id);
        self.status()
    }

    pub fn store_key(&self, provider_id: &str, secret: &Secret) -> Result<()> {
        let t = target_for(provider_id).map_err(|e| DiagAppError::InvalidArgument(e.to_string()))?;
        self.creds.write(&t, secret).map_err(|e| DiagAppError::NotConfigured(format!("could not store the key: {e}")))
    }

    pub fn forget_key(&self, provider_id: &str) -> Result<bool> {
        let t = target_for(provider_id).map_err(|e| DiagAppError::InvalidArgument(e.to_string()))?;
        self.creds.delete(&t).map_err(|e| DiagAppError::NotConfigured(format!("could not remove the key: {e}")))
    }

    /// The named provider, or the default one; `NotConfigured` at once when there is none.
    pub fn provider(&self, id: Option<&str>) -> Result<ProviderConfig> {
        let c = self.config()?;
        let id = id.map(str::to_owned).or(c.default_provider_id.clone());
        let Some(id) = id else {
            return Err(DiagAppError::NotConfigured("no AI provider is configured; add one in Settings > AI 诊断 (local features stay available)".into()));
        };
        c.providers.into_iter().find(|p| p.provider_id == id).ok_or_else(|| DiagAppError::NotConfigured(format!("provider {id} is not configured")))
    }

    fn secret_for(&self, p: &ProviderConfig) -> std::result::Result<Option<Secret>, Failure> {
        if p.kind.mode() == cm_core::diag::Mode::Cli {
            return Ok(None);
        }
        let t = target_for(&p.provider_id).map_err(|e| credential_failure(&e))?;
        match self.creds.read(&t) {
            Ok(Some(s)) => Ok(Some(s)),
            Ok(None) if p.key_required() => Err(http::failure(FailureCode::CredentialMissing, "没有保存 API Key；请在设置中添加。")),
            Ok(None) => Ok(None),
            Err(e) => Err(credential_failure(&e)),
        }
    }

    /// No-data connectivity check of one provider.
    pub fn probe(&self, provider_id: &str) -> Result<ProbeReport> {
        let p = self.provider(Some(provider_id))?;
        let checked_at = now_utc();
        let report = |ok: bool, detail: String, failure: Option<Failure>| ProbeReport {
            provider_id: provider_id.into(),
            ok,
            detail,
            failure,
            checked_at: checked_at.clone(),
            sent_file_data: false,
        };
        match p.kind.mode() {
            cm_core::diag::Mode::Cli => {
                let c = cli::verify(p.kind, Path::new(p.executable.as_deref().unwrap_or_default()));
                let detail = format!("{}{}", c.version.as_deref().map(|v| format!("{v}：")).unwrap_or_default(), c.reason);
                Ok(if c.supported { report(true, detail, None) } else { report(false, detail.clone(), Some(http::failure(FailureCode::UnsupportedMode, detail))) })
            }
            cm_core::diag::Mode::Api => {
                let secret = match self.secret_for(&p) {
                    Ok(s) => s,
                    Err(f) => return Ok(report(false, f.message.clone(), Some(f))),
                };
                Ok(match http::probe(p.kind, p.base_url.as_deref().unwrap_or_default(), &p.model, secret.as_ref(), Duration::from_secs(20)) {
                    Ok(d) => report(true, d, None),
                    Err(f) => report(false, f.message.clone(), Some(f)),
                })
            }
        }
    }

    // -- prepare / confirm / execute ------------------------------------------------------

    /// Build the evidence and the exact payload for one object and return the preview.
    pub fn prepare(&self, ctx: &ScanContext, req: PrepareRequest) -> Result<Preview> {
        let p = self.provider(req.provider_id.as_deref())?;
        p.validate().map_err(DiagAppError::NotConfigured)?;
        if !ctx.view.facts.contains_key(&req.entry_id) {
            return Err(DiagAppError::NotFound(format!("entry {} is not in scan {}", req.entry_id, ctx.scan_id)));
        }
        // Mode checks happen before anything is built: an unconstrained CLI is never prepared.
        let (argv, has_key) = match p.kind {
            ProviderKind::CodexCli => return Err(DiagAppError::Unsupported(cli::CODEX_UNSUPPORTED.into())),
            ProviderKind::ClaudeCodeCli => {
                let check = cli::verify(p.kind, Path::new(p.executable.as_deref().unwrap_or_default()));
                if !check.supported {
                    return Err(DiagAppError::Unsupported(check.reason));
                }
                (cli::claude_argv(&p.model, SYSTEM_PROMPT, &check), false)
            }
            _ => {
                let t = target_for(&p.provider_id).map_err(|e| DiagAppError::InvalidArgument(e.to_string()))?;
                match self.creds.exists(&t) {
                    Ok(true) => (vec![], true),
                    Ok(false) if p.key_required() => return Err(DiagAppError::NotConfigured(format!("no API key is stored for {}", p.provider_id))),
                    Ok(false) => (vec![], false),
                    Err(e) => return Err(DiagAppError::NotConfigured(format!("system credential store unavailable: {e}"))),
                }
            }
        };
        let path = ctx.view.path(req.entry_id);
        let cands: Vec<CandidateFacts<'_>> = ctx
            .set
            .as_ref()
            .filter(|s| s.scan_id == ctx.scan_id && s.revision == ctx.revision)
            .map(|s| s.candidates.iter().map(|c| CandidateFacts { candidate: c, catalog: ctx.catalog.as_ref().and_then(|k| k.facts.get(&c.candidate_id)) }).collect())
            .unwrap_or_default();
        let have_set = ctx.set.as_ref().is_some_and(|s| s.scan_id == ctx.scan_id && s.revision == ctx.revision);
        let user = match &ctx.user {
            Ok(u) => UserList::Loaded(u),
            Err(e) => UserList::Unavailable(e),
        };
        let built = build(&EvidenceInput {
            view: &ctx.view,
            entry: req.entry_id,
            origin: req.origin,
            scan: ScanFacts { scan_id: &ctx.scan_id, revision: ctx.revision, partial: ctx.partial, observed_at: &ctx.observed_at },
            candidates: have_set.then_some(cands.as_slice()),
            protection: ProtectionInput { hard_deny: ctx.hard.as_ref(), user },
            ownership: ownership_for(&path),
            home: ctx.home.as_ref(),
            disclosure: req.disclosure,
            question: req.question.clone(),
            statements: req.statements.clone(),
            parent_diagnosis_id: req.parent_diagnosis_id.clone(),
        })
        .map_err(|e| DiagAppError::InvalidArgument(e.to_string()))?;
        let dest = p.destination();
        let target = (p.kind.mode() == cm_core::diag::Mode::Api && (has_key || p.key_required())).then(|| target_for(&p.provider_id).expect("validated"));
        let payload = cm_core::diag::prompt::payload(
            p.kind,
            &p.model,
            dest.endpoint.as_deref().unwrap_or_default(),
            target.as_deref(),
            p.executable.as_deref(),
            &argv,
            &built.request,
            &built.package,
        )
        .map_err(|e| DiagAppError::InvalidArgument(e.to_string()))?;
        let now = now_unix();
        let preview = Preview {
            preview_id: new_id("pv"),
            state: PreviewState::Preparing,
            contains_user_additions: built.request.question.is_some() || !built.request.statements.is_empty(),
            payload_digest: payload.digest(&dest),
            payload_bytes: payload.body.len() as u64,
            request: built.request,
            evidence: built.package,
            destination: dest,
            payload,
            created_at: utc_from_unix(now),
            expires_at: utc_from_unix(now + budget::PREVIEW_TTL_SECONDS),
        };
        let mut pending = lock(&self.pending);
        pending.retain(|_, x| now - x.created_unix < budget::PREVIEW_TTL_SECONDS);
        if pending.len() >= 32 {
            let oldest = pending.iter().min_by_key(|(_, x)| x.created_unix).map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                pending.remove(&k);
            }
        }
        pending.insert(preview.preview_id.clone(), Pending { preview: preview.clone(), facts: built.facts, config: p, created_unix: now });
        Ok(preview)
    }

    /// Drop a preview without sending anything (AID-07 cancel).
    pub fn discard(&self, preview_id: &str) -> bool {
        lock(&self.pending).remove(preview_id).is_some()
    }

    /// Accept exactly the previewed payload (id + digest) and record a running diagnosis.
    pub fn confirm(&self, preview_id: &str, digest: &str) -> Result<Diagnosis> {
        if !valid_preview_id(preview_id) {
            return Err(DiagAppError::InvalidArgument("malformed preview id".into()));
        }
        let pending = lock(&self.pending).remove(preview_id).ok_or_else(|| DiagAppError::Conflict("the preview expired or was already used; prepare it again".into()))?;
        let now = now_unix();
        if now - pending.created_unix >= budget::PREVIEW_TTL_SECONDS {
            return Err(DiagAppError::Conflict("the preview expired; review the payload again".into()));
        }
        if pending.preview.payload_digest != digest {
            return Err(DiagAppError::Conflict("the confirmed digest does not match this preview".into()));
        }
        // The destination must still be what the person saw (AID-06).
        let current = self.provider(Some(&pending.config.provider_id))?;
        if current.destination() != pending.preview.destination {
            return Err(DiagAppError::Conflict("the provider settings changed after the preview; review it again".into()));
        }
        let created = utc_from_unix(now);
        let d = Diagnosis {
            diagnosis_id: new_id("dg"),
            state: DiagState::Running,
            handoff: handoff(&pending.preview.request, &pending.facts, None, false),
            request: pending.preview.request.clone(),
            evidence: pending.preview.evidence.clone(),
            facts: pending.facts.clone(),
            destination: pending.preview.destination.clone(),
            payload_digest: pending.preview.payload_digest.clone(),
            created_at: created.clone(),
            updated_at: created.clone(),
            finished_at: None,
            expires_at: utc_from_unix(now + budget::RESULT_TTL_SECONDS),
            retention: Retention::Session,
            retain_until: Some(utc_from_unix(now + budget::SESSION_RETENTION_SECONDS)),
            result: None,
            failure: None,
            freshness: Freshness { state: FreshnessState::Unknown, checked_at: created, reasons: vec![] },
        };
        d.validate().map_err(|e| DiagAppError::Store(e.to_string()))?;
        self.persist(&d)?;
        lock(&self.cancels).insert(d.diagnosis_id.clone(), Arc::new(AtomicBool::new(false)));
        lock(&self.ready).insert(d.diagnosis_id.clone(), Ready { payload: pending.preview.payload, config: pending.config });
        Ok(d)
    }

    /// Send a confirmed diagnosis and record its terminal state. Blocking; the desktop calls it
    /// on a worker thread. A cancel during the call wins; the late answer is discarded.
    pub fn execute(&self, diagnosis_id: &str) -> Result<Diagnosis> {
        let ready = lock(&self.ready).remove(diagnosis_id).ok_or_else(|| DiagAppError::Conflict("this diagnosis is not waiting to run".into()))?;
        let cancel = lock(&self.cancels).get(diagnosis_id).cloned().unwrap_or_default();
        let outcome = self.dispatch(&ready, &cancel);
        lock(&self.cancels).remove(diagnosis_id);
        self.finish(diagnosis_id, outcome)
    }

    /// The configured provider only; no fallback (AID-06).
    fn dispatch(&self, ready: &Ready, cancel: &Arc<AtomicBool>) -> std::result::Result<String, RunError> {
        let p = &ready.config;
        match p.kind {
            ProviderKind::Anthropic | ProviderKind::OpenaiCompatible => {
                let secret = self.secret_for(p).map_err(RunError::Failed)?;
                http::send(p.kind, &ready.payload.target, &ready.payload.body, secret, self.limits.http_timeout, cancel)
            }
            ProviderKind::ClaudeCodeCli => {
                let exe = PathBuf::from(p.executable.as_deref().unwrap_or_default());
                let check = cli::verify(p.kind, &exe);
                if !check.supported {
                    return Err(RunError::Failed(http::failure(FailureCode::UnsupportedMode, check.reason)));
                }
                // The argument vector must be the one previewed (a CLI update could change flags).
                if cli::claude_argv(&p.model, SYSTEM_PROMPT, &check) != ready.payload.argv {
                    return Err(RunError::Failed(http::failure(FailureCode::UnsupportedMode, "CLI 版本变化导致参数与预览不一致；请重新预览。")));
                }
                let out = cli::run(&exe, &ready.payload.argv, ready.payload.body.as_bytes(), self.limits.cli_timeout, budget::RAW_ANSWER_BYTES * 2, cancel, &self.cli_env)?;
                cli::claude_result(&out).map_err(RunError::Failed)
            }
            ProviderKind::CodexCli => Err(RunError::Failed(http::failure(FailureCode::UnsupportedMode, cli::CODEX_UNSUPPORTED))),
        }
    }

    fn finish(&self, id: &str, outcome: std::result::Result<String, RunError>) -> Result<Diagnosis> {
        let _w = lock(&self.write);
        let mut d = self.load(id)?;
        if d.state != DiagState::Running {
            return Ok(d); // cancelled meanwhile: the late outcome is dropped
        }
        let now = now_utc();
        let next = match &outcome {
            Ok(_) => DiagState::Completed,
            Err(RunError::Cancelled) => DiagState::Cancelled,
            Err(RunError::Failed(_)) => DiagState::Failed,
        };
        d.state.check(next).map_err(|e| DiagAppError::Store(e.to_string()))?;
        match outcome {
            Ok(text) => match parse_answer(&text) {
                Ok(answer) => {
                    let r = ground(answer, &d.evidence, &d.facts);
                    d.freshness = freshness(&d.expires_at, now_unix(), CurrentObservation::Same, &now);
                    d.handoff = handoff(&d.request, &d.facts, Some(&r.answer), d.freshness.state == FreshnessState::Current);
                    d.result = Some(r);
                    d.state = DiagState::Completed;
                }
                Err(e) => {
                    d.state = DiagState::Failed;
                    d.failure = Some(http::failure(FailureCode::InvalidStructure, format!("回答结构无效，未采用其中任何内容：{e}")));
                }
            },
            Err(RunError::Cancelled) => d.state = DiagState::Cancelled,
            Err(RunError::Failed(f)) => {
                d.state = DiagState::Failed;
                d.failure = Some(f);
            }
        }
        d.finished_at = Some(now.clone());
        d.updated_at = now;
        d.validate().map_err(|e| DiagAppError::Store(e.to_string()))?;
        self.persist(&d)?;
        Ok(d)
    }

    /// Request cancellation. The record turns `cancelled` at once; the adapter stops waiting
    /// (HTTP) or is killed (CLI). Cancelling a finished diagnosis changes nothing.
    pub fn cancel(&self, id: &str) -> Result<Diagnosis> {
        if let Some(flag) = lock(&self.cancels).get(id) {
            flag.store(true, Ordering::Release);
        }
        lock(&self.ready).remove(id);
        let _w = lock(&self.write);
        let mut d = self.load(id)?;
        if d.state == DiagState::Running {
            let now = now_utc();
            d.state = DiagState::Cancelled;
            d.finished_at = Some(now.clone());
            d.updated_at = now;
            self.persist(&d)?;
        }
        Ok(d)
    }

    // -- storage --------------------------------------------------------------------------

    fn persist(&self, d: &Diagnosis) -> Result<()> {
        write_atomic(&self.record_path(&d.diagnosis_id)?, &serde_json::to_string(d).expect("serializable"))
    }

    fn load(&self, id: &str) -> Result<Diagnosis> {
        let path = self.record_path(id)?;
        let text = std::fs::read_to_string(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound { DiagAppError::NotFound(format!("diagnosis {id}")) } else { store_err(e) }
        })?;
        let d: Diagnosis = serde_json::from_str(&text).map_err(|e| DiagAppError::Store(format!("diagnosis {id} is invalid: {e}")))?;
        d.validate().map_err(|e| DiagAppError::Store(format!("diagnosis {id} is invalid: {e}")))?;
        Ok(d)
    }

    /// One diagnosis with freshness re-evaluated against `current` (AID-10/11). A completed
    /// result whose window ran out or whose object changed becomes `expired`: still readable,
    /// hand-off closed.
    pub fn get(&self, id: &str, current: CurrentObservation) -> Result<Diagnosis> {
        let _w = lock(&self.write);
        let mut d = self.load(id)?;
        let process_lost = d.state == DiagState::Running && !lock(&self.cancels).contains_key(id) && !lock(&self.ready).contains_key(id);
        if process_lost && parse_utc(&d.updated_at).is_some_and(|t| now_unix() - t > self.limits.cli_timeout.max(self.limits.http_timeout).as_secs() as i64 + 60) {
            // The hosting process ended without a terminal state: report it, never "running" forever.
            d.state = DiagState::Failed;
            d.failure = Some(http::failure(FailureCode::ProcessFailed, "运行诊断的进程在完成前结束。"));
            d.finished_at = Some(now_utc());
        }
        if matches!(d.state, DiagState::Completed | DiagState::Expired) {
            d.freshness = freshness(&d.expires_at, now_unix(), current, &now_utc());
            if d.freshness.state == FreshnessState::Stale && d.state == DiagState::Completed {
                d.state = DiagState::Expired;
            }
        }
        let fresh = d.state == DiagState::Completed && d.freshness.state == FreshnessState::Current;
        d.handoff = handoff(&d.request, &d.facts, d.result.as_ref().map(|r| &r.answer), fresh);
        d.validate().map_err(|e| DiagAppError::Store(e.to_string()))?;
        let stored = self.load(id)?;
        if stored.state != d.state {
            d.updated_at = now_utc();
            self.persist(&d)?;
        }
        Ok(d)
    }

    /// Newest first; `cursor` is the offset returned by the previous page.
    pub fn list(&self, page_size: usize, cursor: Option<&str>) -> Result<(Vec<DiagnosisSummary>, Option<String>)> {
        if page_size == 0 || page_size > budget::LIST_PAGE {
            return Err(DiagAppError::InvalidArgument(format!("page size must be 1..={}", budget::LIST_PAGE)));
        }
        let start: usize = match cursor {
            None => 0,
            Some(c) => c.strip_prefix("o:").and_then(|n| n.parse().ok()).ok_or_else(|| DiagAppError::InvalidArgument("malformed cursor".into()))?,
        };
        self.purge_session(now_unix())?;
        let mut all: Vec<DiagnosisSummary> = Vec::new();
        for e in std::fs::read_dir(self.diag_dir()?).map_err(store_err)?.filter_map(|e| e.ok()) {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(id) = name.strip_suffix(".json")
                && let Ok(d) = self.load(id)
            {
                all.push(DiagnosisSummary::from(&d));
            }
        }
        all.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.diagnosis_id.cmp(&a.diagnosis_id)));
        let end = (start + page_size).min(all.len());
        let page = if start < all.len() { all[start..end].to_vec() } else { vec![] };
        Ok((page, (end < all.len()).then(|| format!("o:{end}"))))
    }

    /// Keep a diagnosis beyond the session (AID-12).
    pub fn save(&self, id: &str) -> Result<Diagnosis> {
        let _w = lock(&self.write);
        let mut d = self.load(id)?;
        d.retention = Retention::Saved;
        d.retain_until = None;
        d.updated_at = now_utc();
        self.persist(&d)?;
        Ok(d)
    }

    /// Remove a diagnosis record. Operation history (`audit.db`) is a different store and is
    /// never touched.
    pub fn remove(&self, id: &str) -> Result<bool> {
        let _ = self.cancel(id);
        let _w = lock(&self.write);
        match std::fs::remove_file(self.record_path(id)?) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(store_err(e)),
        }
    }

    /// Session-retained diagnoses past `retain_until` are removed.
    pub fn purge_session(&self, now: i64) -> Result<usize> {
        let dir = self.store_dir.join(DIAG_DIR);
        let Ok(rd) = std::fs::read_dir(&dir) else { return Ok(0) };
        let mut n = 0;
        for e in rd.filter_map(|e| e.ok()) {
            let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
            let Ok(d) = serde_json::from_str::<Diagnosis>(&text) else { continue };
            if d.retention == Retention::Session
                && d.state != DiagState::Running
                && d.retain_until.as_deref().and_then(parse_utc).is_some_and(|t| now >= t)
                && std::fs::remove_file(e.path()).is_ok()
            {
                n += 1;
            }
        }
        Ok(n)
    }

    /// Export view of a diagnosis: the record as stored (it never holds a credential) plus
    /// what is and is not included.
    pub fn export(&self, id: &str) -> Result<serde_json::Value> {
        let d = self.load(id)?;
        Ok(serde_json::json!({
            "contract": cm_core::diag::DIAG_CONTRACT,
            "kind": "diagnosis_export",
            "includes": ["object_reference", "redacted_evidence", "provider_and_model", "explanation", "grounding", "handoff"],
            "excludes": ["credentials", "file_contents", "operation_audit"],
            "diagnosis": d,
        }))
    }

    /// Follow-up answer: a new preview for the same object and destination with the person's
    /// statement added. It must be confirmed again (AID-05).
    pub fn follow_up(&self, ctx: &ScanContext, parent_id: &str, question_id: Option<&str>, text: &str) -> Result<Preview> {
        let parent = self.load(parent_id)?;
        let text = text.trim();
        if text.is_empty() || text.chars().count() > budget::USER_TEXT_CHARS {
            return Err(DiagAppError::InvalidArgument("the answer must be 1-280 characters".into()));
        }
        let mut statements = parent.request.statements.clone();
        if statements.len() >= budget::USER_STATEMENTS {
            return Err(DiagAppError::InvalidArgument("too many follow-up answers for one object; start a new diagnosis".into()));
        }
        statements.push(UserStatement {
            statement_id: format!("u{}", statements.len() + 1),
            question_id: question_id.map(str::to_owned),
            text: text.into(),
            at: now_utc(),
        });
        let entry_id: u64 = parent.request.object.entry_id.parse().map_err(|_| DiagAppError::Store("bad entry id".into()))?;
        if ctx.scan_id != parent.request.object.scan_id {
            return Err(DiagAppError::InvalidArgument("the follow-up must use the diagnosis' own scan".into()));
        }
        self.prepare(ctx, PrepareRequest {
            scan_id: parent.request.object.scan_id.clone(),
            entry_id,
            origin: parent.request.object.origin,
            provider_id: Some(parent.destination.provider_id.clone()),
            disclosure: parent.request.disclosure,
            question: parent.request.question.clone(),
            statements,
            parent_diagnosis_id: Some(parent.diagnosis_id.clone()),
        })
    }
}

/// What the latest index says about a diagnosis' object: the bound scan is immutable, so only
/// a newer published scan of the same scope can show a change.
pub fn current_observation(stores: &Stores, d: &Diagnosis) -> CurrentObservation {
    let Ok(s) = stores.index.scan_status(&d.request.object.scan_id) else { return CurrentObservation::Missing };
    if !s.published {
        return CurrentObservation::NotChecked;
    }
    let Ok((scans, _)) = stores.index.list_scans(50, None) else { return CurrentObservation::NotChecked };
    let newer = scans.iter().filter(|x| x.scope_id == s.scope_id && x.published && x.started_at > s.started_at).max_by(|a, b| a.started_at.cmp(&b.started_at));
    let Some(newer) = newer else { return CurrentObservation::Same };
    let Ok(view) = stores.index.view(&newer.scan_id) else { return CurrentObservation::NotChecked };
    match view.find_identity(&d.facts.volume_id, &d.facts.file_id) {
        None => CurrentObservation::Missing,
        Some(e) if object_fingerprint(&view, e) == d.facts.object_fingerprint => CurrentObservation::Same,
        Some(_) => CurrentObservation::Changed,
    }
}

#[cfg(test)]
#[path = "diag_tests.rs"]
mod tests;
