//! Optional AI cleanup diagnosis (AID-01..13), contract `diag/1`.
//!
//! The extension explains one indexed object; it never adds candidates, rules, protection
//! exceptions or authorization. Its records live in their own versioned contract
//! (`schemas/diag/v1/diagnosis.schema.json`) so the first-version `schemas/v1` protocol is not
//! widened by an un-negotiated extension. Everything here is pure: evidence is built from index
//! metadata ([`evidence`]), provider text is parsed strictly and checked against core facts
//! ([`ground`]), and the allowed next steps come from the core only ([`ground::handoff`]).
//! Network and process adapters live in `cm-app`, credentials in `cm-platform`.

#[path = "ai_prompt.rs"]
pub mod prompt;
#[path = "diag_evidence.rs"]
pub mod evidence;
#[path = "diag_ground.rs"]
pub mod ground;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Contract of every diagnosis record (envelope `contract` field).
pub const DIAG_CONTRACT: &str = "diag/1";
/// Evidence package shape sent to a provider.
pub const EVIDENCE_VERSION: &str = "diag-evidence/1";
/// Answer shape a provider must return.
pub const ANSWER_VERSION: &str = "diag-answer/1";

/// Field budgets (AID-11 bounded output). Text budgets count Unicode scalar values.
pub mod budget {
    pub const NAME_CHARS: usize = 120;
    pub const LOCATION_CHARS: usize = 240;
    pub const EVIDENCE_TEXT_CHARS: usize = 240;
    pub const EVIDENCE_ITEMS: usize = 24;
    pub const GAPS: usize = 12;
    pub const CHILDREN: usize = 5;
    pub const USER_TEXT_CHARS: usize = 280;
    pub const USER_STATEMENTS: usize = 8;
    /// Exact bytes handed to a provider (HTTP body or CLI stdin).
    pub const PAYLOAD_BYTES: usize = 48 * 1024;
    /// Raw provider output accepted before the run fails with `output_too_large`.
    pub const RAW_ANSWER_BYTES: usize = 256 * 1024;
    pub const SUMMARY_CHARS: usize = 280;
    pub const PARAGRAPH_CHARS: usize = 600;
    pub const SHORT_CHARS: usize = 200;
    pub const CLAIMS: usize = 10;
    pub const REFS_PER_CLAIM: usize = 6;
    pub const LIST_ITEMS: usize = 6;
    pub const ALTERNATIVES: usize = 5;
    pub const QUESTIONS: usize = 3;
    pub const CHOICES: usize = 4;
    pub const CHOICE_CHARS: usize = 60;
    pub const MESSAGE_CHARS: usize = 280;
    /// A confirmed preview must be used within this many seconds.
    pub const PREVIEW_TTL_SECONDS: i64 = 600;
    /// A completed diagnosis is shown as expired (read-only, no hand-off) after this long.
    pub const RESULT_TTL_SECONDS: i64 = 86_400;
    /// Session-retained diagnoses are removed after this long unless saved (AID-12).
    pub const SESSION_RETENTION_SECONDS: i64 = 86_400;
    pub const LIST_PAGE: usize = 100;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiagError {
    #[error("invalid diagnosis request: {0}")]
    InvalidRequest(String),
    #[error("the object is not in the indexed scan: {0}")]
    UnknownObject(String),
    #[error("payload exceeds the {0}-byte budget")]
    PayloadTooLarge(usize),
    #[error("invalid answer structure: {0}")]
    InvalidAnswer(String),
    #[error("invalid lifecycle transition {} -> {}", .from.as_str(), .to.as_str())]
    Transition { from: DiagState, to: DiagState },
}

/// Where a diagnosis was started (AID-02).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Cleanup,
    Disk,
    Apps,
}

/// The one object a diagnosis is about, bound to a scan revision (AID-02/03).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectRef {
    pub scan_id: String,
    pub revision: u32,
    /// Index entry id (decimal string, as in the v1 protocol).
    pub entry_id: String,
    pub origin: Origin,
    pub candidate_id: Option<String>,
    pub app_instance_id: Option<String>,
}

/// Extra disclosure the person chose in the preview; both default to off (AID-07).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Disclosure {
    /// Send the full display path instead of the redacted location and name.
    pub full_path: bool,
    /// Send a summary of at most [`budget::CHILDREN`] largest direct children (names redacted).
    pub child_summary: bool,
}

/// Something the person said; always a statement, never a verified fact (AID-05).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserStatement {
    pub statement_id: String,
    pub question_id: Option<String>,
    pub text: String,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosisRequest {
    pub contract: String,
    pub object: ObjectRef,
    /// When the bound scan observed the object (scan finish time, not "now").
    pub observed_at: String,
    /// SHA-256 of the object's index facts; local only, never sent to a provider.
    pub observation_fingerprint: String,
    pub disclosure: Disclosure,
    pub question: Option<String>,
    pub statements: Vec<UserStatement>,
    pub parent_diagnosis_id: Option<String>,
    pub prompt_template: String,
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

pub(crate) fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `dg_` + 20 lower-case hex digits.
pub fn valid_diagnosis_id(s: &str) -> bool {
    s.strip_prefix("dg_").is_some_and(|h| h.len() == 20 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

/// `pv_` + 20 lower-case hex digits.
pub fn valid_preview_id(s: &str) -> bool {
    s.strip_prefix("pv_").is_some_and(|h| h.len() == 20 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

impl DiagnosisRequest {
    /// Version, reference and budget checks shared by every entry point.
    pub fn validate(&self) -> Result<(), DiagError> {
        let bad = |m: &str| Err(DiagError::InvalidRequest(m.into()));
        if self.contract != DIAG_CONTRACT {
            return bad("unsupported contract version");
        }
        if self.prompt_template != prompt::TEMPLATE_VERSION {
            return bad("unsupported prompt template");
        }
        if self.object.scan_id.is_empty() || self.object.entry_id.parse::<u64>().is_err() {
            return bad("object needs a scan_id and a numeric entry_id");
        }
        if !is_hex64(&self.observation_fingerprint) {
            return bad("observation_fingerprint must be 64 lower-case hex digits");
        }
        if crate::plan::parse_utc(&self.observed_at).is_none() {
            return bad("observed_at must be an RFC 3339 UTC time");
        }
        if self.question.as_deref().is_some_and(|q| q.trim().is_empty() || chars(q) > budget::USER_TEXT_CHARS) {
            return bad("question must be 1..=280 characters");
        }
        if self.statements.len() > budget::USER_STATEMENTS {
            return bad("too many user statements");
        }
        let mut ids = std::collections::BTreeSet::new();
        for s in &self.statements {
            if s.text.trim().is_empty() || chars(&s.text) > budget::USER_TEXT_CHARS || !ids.insert(s.statement_id.as_str()) {
                return bad("each statement needs a unique id and 1..=280 characters");
            }
        }
        if self.parent_diagnosis_id.as_deref().is_some_and(|p| !valid_diagnosis_id(p)) {
            return bad("parent_diagnosis_id is malformed");
        }
        Ok(())
    }
}

/// Which layer a piece of evidence comes from (AID-03): local observation, rule/catalog facts,
/// protection, ownership, coverage, or something the person said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    Observation,
    Rule,
    Protection,
    Ownership,
    Coverage,
    UserStatement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceItem {
    pub evidence_id: String,
    pub layer: Layer,
    pub code: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gap {
    pub gap_id: String,
    pub code: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactionRule {
    UserProfile,
    Email,
    LongNumber,
    SecretLike,
    ControlCharacters,
    InstructionLike,
    Truncated,
    ParentComponents,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Redaction {
    pub field: String,
    pub rule: RedactionRule,
}

/// What a package never contains, stated in every package (AID-07).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Excluded {
    FileContents,
    DirectoryTree,
    Credentials,
    Sessions,
    CloudHydration,
}

pub const ALWAYS_EXCLUDED: [Excluded; 5] = [Excluded::FileContents, Excluded::DirectoryTree, Excluded::Credentials, Excluded::Sessions, Excluded::CloudHydration];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subject {
    pub kind: String,
    pub name: String,
    pub extension: Option<String>,
    pub location: String,
    pub logical_bytes: Option<String>,
    pub allocated_bytes: Option<String>,
    pub size_basis: String,
    pub modified_at: Option<String>,
    pub link_count: Option<u32>,
    pub cloud: String,
    pub identity_quality: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildSummary {
    pub name: String,
    pub kind: String,
    pub logical_bytes: Option<String>,
}

/// The bounded, redacted facts about one object that a provider may see (AID-03/07).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidencePackage {
    pub version: String,
    pub subject: Subject,
    /// Present only when the person disclosed a child summary.
    pub children: Option<Vec<ChildSummary>>,
    pub children_omitted: u64,
    pub items: Vec<EvidenceItem>,
    pub gaps: Vec<Gap>,
    pub redactions: Vec<Redaction>,
    pub excluded: Vec<Excluded>,
}

impl EvidencePackage {
    pub fn has_ref(&self, id: &str) -> bool {
        self.items.iter().any(|i| i.evidence_id == id) || self.gaps.iter().any(|g| g.gap_id == id)
    }

    pub fn item(&self, id: &str) -> Option<&EvidenceItem> {
        self.items.iter().find(|i| i.evidence_id == id)
    }

    pub fn has_code(&self, code: &str) -> bool {
        self.items.iter().any(|i| i.code == code) || self.gaps.iter().any(|g| g.code == code)
    }

    /// Structural checks used when reading stored or exchanged packages.
    pub fn validate(&self) -> Result<(), DiagError> {
        let bad = |m: &str| Err(DiagError::InvalidRequest(m.into()));
        if self.version != EVIDENCE_VERSION {
            return bad("unsupported evidence version");
        }
        if self.items.len() > budget::EVIDENCE_ITEMS || self.gaps.len() > budget::GAPS {
            return bad("too many evidence items or gaps");
        }
        if chars(&self.subject.name) > budget::NAME_CHARS || chars(&self.subject.location) > budget::LOCATION_CHARS {
            return bad("subject text over budget");
        }
        if self.children.as_ref().is_some_and(|c| c.len() > budget::CHILDREN) {
            return bad("too many children");
        }
        let mut ids = std::collections::BTreeSet::new();
        for (id, text) in self.items.iter().map(|i| (&i.evidence_id, &i.text)).chain(self.gaps.iter().map(|g| (&g.gap_id, &g.text))) {
            if !ids.insert(id.as_str()) || chars(text) > budget::EVIDENCE_TEXT_CHARS {
                return bad("evidence ids must be unique and texts within budget");
            }
        }
        if ALWAYS_EXCLUDED.iter().any(|e| !self.excluded.contains(e)) {
            return bad("package must state every excluded data class");
        }
        Ok(())
    }
}

/// Conditional recommendation states (AID-04); never a percentage or a "safe" verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Recommendation {
    /// May be taken to the core's own plan review (which verifies again).
    Reviewable,
    NeedsContext,
    Keep,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    Observation,
    Rule,
    UserStatement,
    Inference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claim {
    pub text: String,
    pub basis: Basis,
    pub refs: Vec<String>,
}

/// Next steps a diagnosis may point at. Only the core decides which are available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NextAction {
    ReviewPlan,
    Keep,
    Protect,
    Locate,
    Verify,
    AppManagement,
    Archive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Alternative {
    pub action: NextAction,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub question_id: String,
    pub text: String,
    pub choices: Vec<String>,
}

/// The only answer shape accepted from a provider (`diag-answer/1`). Unknown fields, wrong
/// versions, duplicate keys and numbers are rejected; texts are bounded by [`ground`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelAnswer {
    pub answer_version: String,
    pub summary: String,
    pub purpose: String,
    pub recommendation: Recommendation,
    pub conditions: Vec<String>,
    pub deletion_impact: String,
    pub rebuild_cost: String,
    pub claims: Vec<Claim>,
    pub gaps: Vec<String>,
    pub alternatives: Vec<Alternative>,
    pub questions: Vec<Question>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    /// Cites existing observation/rule evidence and conflicts with nothing the core knows.
    Supported,
    /// The person's own statement, shown as such (not verified).
    UserStatement,
    /// Model inference, shown as such.
    Inference,
    /// Cites nothing that supports it (e.g. "already backed up" without verified evidence).
    Unsupported,
    ConflictsWithFacts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimCheck {
    pub index: u32,
    pub status: ClaimStatus,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictCode {
    ProtectedObject,
    NotACandidate,
    CandidateNotPlannable,
    PartialEvidence,
    UnverifiedBackupClaim,
    UnverifiedUsageClaim,
    UnverifiedReclaimClaim,
    SafeToDeleteClaim,
    RuleClaimWithoutRule,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Conflict {
    pub code: ConflictCode,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SanitizeReason {
    CommandText,
    ApprovalInstruction,
    UnknownReference,
    /// "Safe to delete" style wording without a hedge; CleanManager never states safety.
    UnverifiedSafetyClaim,
    Truncated,
    DroppedItem,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sanitized {
    pub field: String,
    pub reason: SanitizeReason,
}

/// What a hand-off acts on: always an id the core issued, never a path or model text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffTarget {
    pub scan_id: String,
    pub entry_id: String,
    pub candidate_id: Option<String>,
    pub app_instance_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Handoff {
    pub action: NextAction,
    pub available: bool,
    pub reason: String,
    /// Model text that suggested this step, when the model did (display only).
    pub suggested_by_model: bool,
    pub target: Option<HandoffTarget>,
}

/// Provider answer after strict parsing, sanitizing and grounding (AID-04/05/08/09).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosisResult {
    pub answer: ModelAnswer,
    pub model_recommendation: Recommendation,
    pub effective_recommendation: Recommendation,
    pub grounding: Vec<ClaimCheck>,
    pub conflicts: Vec<Conflict>,
    pub sanitized: Vec<Sanitized>,
}

/// Core facts kept beside a diagnosis (local only): the basis for grounding and hand-off.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreFacts {
    pub entry_id: String,
    pub kind: String,
    /// Object identity and object-only fingerprint, to recognise the object in a later scan.
    pub volume_id: String,
    pub file_id: String,
    pub object_fingerprint: String,
    /// Reason code of the strongest protection layer that matched, if any.
    pub protection: Option<String>,
    pub protection_checked: bool,
    pub candidate_id: Option<String>,
    pub candidate_state: Option<String>,
    pub plannable: bool,
    pub partial_evidence: bool,
    pub cloud_placeholder: bool,
    pub hard_links: bool,
    pub app_instance_ids: Vec<String>,
    pub rule_refs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Api,
    Cli,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Anthropic Messages API (`POST /v1/messages`).
    Anthropic,
    /// An OpenAI-compatible `chat/completions` endpoint, local ones included.
    OpenaiCompatible,
    /// Claude Code in print mode with every tool disabled.
    ClaudeCodeCli,
    /// Codex `exec`: its shell tool cannot be disabled, so it is always unsupported (AID-08).
    CodexCli,
}

impl ProviderKind {
    pub fn mode(self) -> Mode {
        match self {
            ProviderKind::Anthropic | ProviderKind::OpenaiCompatible => Mode::Api,
            ProviderKind::ClaudeCodeCli | ProviderKind::CodexCli => Mode::Cli,
        }
    }
}

/// Where a diagnosis is sent, as shown in the preview. Changing any field needs a new
/// disclosure confirmation (AID-06).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Destination {
    pub provider_id: String,
    pub label: String,
    pub kind: ProviderKind,
    pub mode: Mode,
    pub endpoint: Option<String>,
    pub executable: Option<String>,
    pub model: String,
    pub template: String,
}

/// The exact bytes that would leave CleanManager, with credentials shown only as a reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransportPayload {
    /// `https_post`, `http_post_loopback` or `process_stdin`.
    pub transport: String,
    pub target: String,
    /// Header lines (`name: value`); the credential header shows a store reference, never a key.
    pub headers: Vec<String>,
    /// CLI argument vector (no shell); empty for HTTP.
    pub argv: Vec<String>,
    pub body: String,
}

impl TransportPayload {
    pub fn digest(&self, destination: &Destination) -> String {
        let v = serde_json::json!({"destination": destination, "payload": self});
        crate::c14n::digest(&v).expect("plain JSON")
    }
}

/// A disclosure waiting for confirmation (the lifecycle's `preparing` step). Cancelling it
/// sends nothing (AID-07).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preview {
    pub preview_id: String,
    pub state: PreviewState,
    pub request: DiagnosisRequest,
    pub evidence: EvidencePackage,
    pub destination: Destination,
    pub payload: TransportPayload,
    pub payload_digest: String,
    pub payload_bytes: u64,
    pub contains_user_additions: bool,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewState {
    Preparing,
}

/// Diagnosis lifecycle (AID-10). `preparing` is a [`Preview`]; a diagnosis record exists once a
/// disclosure was confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagState {
    Running,
    Completed,
    Cancelled,
    Failed,
    /// Completed, but the observation or the result's validity window ran out: readable, no hand-off.
    Expired,
}

impl DiagState {
    pub fn is_terminal(self) -> bool {
        !matches!(self, DiagState::Running)
    }

    /// The protocol word (`running`, `completed`, ...), also used in messages.
    pub fn as_str(self) -> &'static str {
        match self {
            DiagState::Running => "running",
            DiagState::Completed => "completed",
            DiagState::Cancelled => "cancelled",
            DiagState::Failed => "failed",
            DiagState::Expired => "expired",
        }
    }

    /// Allowed transitions; a late provider answer for a cancelled run is refused here.
    pub fn check(self, to: DiagState) -> Result<(), DiagError> {
        let ok = matches!(
            (self, to),
            (DiagState::Running, DiagState::Completed | DiagState::Cancelled | DiagState::Failed) | (DiagState::Completed, DiagState::Expired)
        );
        if ok { Ok(()) } else { Err(DiagError::Transition { from: self, to }) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    NotConfigured,
    CredentialMissing,
    CredentialStoreUnavailable,
    AuthFailed,
    ModelUnavailable,
    RateLimited,
    ProviderError,
    Refused,
    Timeout,
    Network,
    OutputTooLarge,
    InvalidStructure,
    UnsupportedMode,
    ProcessFailed,
}

impl FailureCode {
    pub fn retryable(self) -> bool {
        matches!(self, FailureCode::RateLimited | FailureCode::ProviderError | FailureCode::Timeout | FailureCode::Network | FailureCode::InvalidStructure)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    pub code: FailureCode,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FreshnessState {
    Current,
    Stale,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Freshness {
    pub state: FreshnessState,
    pub checked_at: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Retention {
    /// Removed after [`budget::SESSION_RETENTION_SECONDS`] or when the person removes it.
    Session,
    Saved,
}

/// One diagnosis as stored and as returned to the GUI and the CLI (same id everywhere, AID-11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnosis {
    pub diagnosis_id: String,
    pub state: DiagState,
    pub request: DiagnosisRequest,
    pub evidence: EvidencePackage,
    pub facts: CoreFacts,
    pub destination: Destination,
    pub payload_digest: String,
    pub created_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
    pub expires_at: String,
    pub retention: Retention,
    pub retain_until: Option<String>,
    pub result: Option<DiagnosisResult>,
    pub failure: Option<Failure>,
    pub freshness: Freshness,
    pub handoff: Vec<Handoff>,
}

impl Diagnosis {
    /// Record-level invariants: a result only when completed/expired, a failure only when
    /// failed, ids well formed, everything within budget.
    pub fn validate(&self) -> Result<(), DiagError> {
        let bad = |m: &str| Err(DiagError::InvalidRequest(m.into()));
        if !valid_diagnosis_id(&self.diagnosis_id) || !is_hex64(&self.payload_digest) {
            return bad("malformed diagnosis id or payload digest");
        }
        self.request.validate()?;
        self.evidence.validate()?;
        let has_result = matches!(self.state, DiagState::Completed | DiagState::Expired);
        if self.result.is_some() != has_result {
            return bad("a result exists exactly when the diagnosis completed");
        }
        if self.failure.is_some() != (self.state == DiagState::Failed) {
            return bad("a failure exists exactly when the diagnosis failed");
        }
        if self.state.is_terminal() != self.finished_at.is_some() {
            return bad("finished_at is set exactly for terminal states");
        }
        if (self.retention == Retention::Session) != self.retain_until.is_some() {
            return bad("session retention needs retain_until; saved has none");
        }
        if let Some(r) = &self.result {
            ground::check_bounds(&r.answer).map_err(DiagError::InvalidAnswer)?;
            for c in &r.answer.claims {
                if c.refs.iter().any(|id| !self.evidence.has_ref(id)) {
                    return bad("a claim cites evidence that is not in the package");
                }
            }
            if r.effective_recommendation < r.model_recommendation {
                return bad("grounding can only make a recommendation more conservative");
            }
            if self.facts.protection.is_some() && r.effective_recommendation < Recommendation::Keep {
                return bad("a protected object is never less than keep");
            }
            if r.effective_recommendation == Recommendation::Reviewable && (!self.facts.plannable || self.facts.partial_evidence) {
                return bad("reviewable needs a plannable candidate with complete evidence");
            }
            if (self.state != DiagState::Completed || self.freshness.state != FreshnessState::Current)
                && self.handoff.iter().any(|h| h.available && h.action == NextAction::ReviewPlan)
            {
                return bad("an expired or stale diagnosis cannot hand off to a plan");
            }
        }
        if self.handoff.iter().any(|h| h.action == NextAction::ReviewPlan && h.available && (self.facts.protection.is_some() || !self.facts.plannable)) {
            return bad("plan hand-off needs a plannable, unprotected candidate");
        }
        Ok(())
    }
}

/// One row of a diagnosis listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosisSummary {
    pub diagnosis_id: String,
    pub state: DiagState,
    pub object: ObjectRef,
    pub subject_name: String,
    pub provider_id: String,
    pub model: String,
    pub effective_recommendation: Option<Recommendation>,
    pub created_at: String,
    pub retention: Retention,
    pub parent_diagnosis_id: Option<String>,
}

impl From<&Diagnosis> for DiagnosisSummary {
    fn from(d: &Diagnosis) -> Self {
        DiagnosisSummary {
            diagnosis_id: d.diagnosis_id.clone(),
            state: d.state,
            object: d.request.object.clone(),
            subject_name: d.evidence.subject.name.clone(),
            provider_id: d.destination.provider_id.clone(),
            model: d.destination.model.clone(),
            effective_recommendation: d.result.as_ref().map(|r| r.effective_recommendation),
            created_at: d.created_at.clone(),
            retention: d.retention,
            parent_diagnosis_id: d.request.parent_diagnosis_id.clone(),
        }
    }
}

/// Wrap record fields in the diagnosis envelope (not the v1 protocol envelope).
pub fn record(kind: &str, request_id: &str, fields: Map<String, Value>) -> Value {
    let mut out = Map::new();
    out.insert("contract".into(), Value::from(DIAG_CONTRACT));
    out.insert("kind".into(), Value::from(kind));
    out.insert("request_id".into(), Value::from(request_id));
    out.extend(fields);
    Value::Object(out)
}

#[cfg(test)]
#[path = "diag_tests.rs"]
mod tests;
