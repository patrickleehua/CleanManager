// Optional AI diagnosis shapes (contract diag/1, mirrored from cm_core::diag). Byte counts are
// decimal strings; unknown is null, never "0". Nothing here carries a credential or a path the
// WebView could send back: objects are named by scan id + entry id, runs by preview/diagnosis id.

export type Origin = 'cleanup' | 'disk' | 'apps';
export type Recommendation = 'reviewable' | 'needs_context' | 'keep' | 'unsupported';
export type DiagState = 'running' | 'completed' | 'cancelled' | 'failed' | 'expired';
export type NextAction = 'review_plan' | 'keep' | 'protect' | 'locate' | 'verify' | 'app_management' | 'archive';
export type Layer = 'observation' | 'rule' | 'protection' | 'ownership' | 'coverage' | 'user_statement';
export type Basis = 'observation' | 'rule' | 'user_statement' | 'inference';
export type ClaimStatus = 'supported' | 'user_statement' | 'inference' | 'unsupported' | 'conflicts_with_facts';
export type ProviderKind = 'anthropic' | 'openai_compatible' | 'claude_code_cli' | 'codex_cli';

/** What the workbench is pointed at (from the cleanup, disk or apps page). */
export interface DiagTarget {
  scanId: string;
  entryId: string;
  origin: Origin;
  /** Display name for the header while nothing is loaded yet (never sent to the host). */
  name: string;
}

export interface Disclosure {
  full_path: boolean;
  child_summary: boolean;
}

export interface UserStatement {
  statement_id: string;
  question_id: string | null;
  text: string;
  at: string;
}

export interface DiagRequest {
  contract: string;
  object: { scan_id: string; revision: number; entry_id: string; origin: Origin; candidate_id: string | null; app_instance_id: string | null };
  observed_at: string;
  observation_fingerprint: string;
  disclosure: Disclosure;
  question: string | null;
  statements: UserStatement[];
  parent_diagnosis_id: string | null;
  prompt_template: string;
}

export interface EvidenceItem {
  evidence_id: string;
  layer: Layer;
  code: string;
  text: string;
}

export interface Gap {
  gap_id: string;
  code: string;
  text: string;
}

export interface Subject {
  kind: string;
  name: string;
  extension: string | null;
  location: string;
  logical_bytes: string | null;
  allocated_bytes: string | null;
  size_basis: string;
  modified_at: string | null;
  link_count: number | null;
  cloud: string;
  identity_quality: string;
}

export interface Evidence {
  version: string;
  subject: Subject;
  children: { name: string; kind: string; logical_bytes: string | null }[] | null;
  children_omitted: number;
  items: EvidenceItem[];
  gaps: Gap[];
  redactions: { field: string; rule: string }[];
  excluded: string[];
}

export interface Destination {
  provider_id: string;
  label: string;
  kind: ProviderKind;
  mode: 'api' | 'cli';
  endpoint: string | null;
  executable: string | null;
  model: string;
  template: string;
}

export interface Payload {
  transport: 'https_post' | 'http_post_loopback' | 'process_stdin';
  target: string;
  headers: string[];
  argv: string[];
  body: string;
}

export interface Preview {
  preview_id: string;
  state: 'preparing';
  request: DiagRequest;
  evidence: Evidence;
  destination: Destination;
  payload: Payload;
  payload_digest: string;
  payload_bytes: number;
  contains_user_additions: boolean;
  created_at: string;
  expires_at: string;
}

export interface Claim {
  text: string;
  basis: Basis;
  refs: string[];
}

export interface Question {
  question_id: string;
  text: string;
  choices: string[];
}

export interface Answer {
  answer_version: string;
  summary: string;
  purpose: string;
  recommendation: Recommendation;
  conditions: string[];
  deletion_impact: string;
  rebuild_cost: string;
  claims: Claim[];
  gaps: string[];
  alternatives: { action: NextAction; text: string }[];
  questions: Question[];
}

export interface DiagResult {
  answer: Answer;
  model_recommendation: Recommendation;
  effective_recommendation: Recommendation;
  grounding: { index: number; status: ClaimStatus; note: string | null }[];
  conflicts: { code: string; text: string }[];
  sanitized: { field: string; reason: string }[];
}

export interface Handoff {
  action: NextAction;
  available: boolean;
  reason: string;
  suggested_by_model: boolean;
  target: { scan_id: string; entry_id: string; candidate_id: string | null; app_instance_id: string | null } | null;
}

export interface Failure {
  code: string;
  message: string;
  retryable: boolean;
}

export interface Diagnosis {
  diagnosis_id: string;
  state: DiagState;
  request: DiagRequest;
  evidence: Evidence;
  facts: {
    entry_id: string;
    kind: string;
    protection: string | null;
    candidate_id: string | null;
    plannable: boolean;
    app_instance_ids: string[];
    [k: string]: unknown;
  };
  destination: Destination;
  payload_digest: string;
  created_at: string;
  updated_at: string;
  finished_at: string | null;
  expires_at: string;
  retention: 'session' | 'saved';
  retain_until: string | null;
  result: DiagResult | null;
  failure: Failure | null;
  freshness: { state: 'current' | 'stale' | 'unknown'; checked_at: string; reasons: string[] };
  handoff: Handoff[];
}

export interface DiagSummary {
  diagnosis_id: string;
  state: DiagState;
  object: DiagRequest['object'];
  subject_name: string;
  provider_id: string;
  model: string;
  effective_recommendation: Recommendation | null;
  created_at: string;
  retention: 'session' | 'saved';
  parent_diagnosis_id: string | null;
}

export interface ProviderStatus {
  provider_id: string;
  label: string;
  kind: ProviderKind;
  mode: 'api' | 'cli';
  endpoint: string | null;
  executable: string | null;
  model: string;
  credential: 'present' | 'absent' | 'not_required' | 'unavailable';
  support: { state: 'supported' | 'unsupported' | 'unverified'; reason: string | null };
  updated_at: string;
}

export interface DiagStatus {
  configured: boolean;
  credential_store: string;
  providers: ProviderStatus[];
  default_provider_id: string | null;
  key_prompt_available: boolean;
}

export interface ProbeReport {
  provider_id: string;
  ok: boolean;
  detail: string;
  failure: Failure | null;
  checked_at: string;
  sent_file_data: boolean;
}

export interface ApiProviderInput {
  provider_id: string;
  label: string;
  kind: 'anthropic' | 'openai_compatible';
  base_url: string;
  model: string;
  make_default: boolean;
}

export interface HandoffResult {
  action: NextAction;
  outcome: 'plan' | 'protected' | 'disk' | 'apps' | 'none';
  plan_id: string | null;
  scan_id: string | null;
  entry_id: string | null;
  app_instance_id: string | null;
}

export interface DiagFinishedEvent {
  diagnosis_id: string;
  state: DiagState;
}

export interface DiagBackend {
  diagStatus(): Promise<DiagStatus>;
  diagSaveProvider(input: ApiProviderInput): Promise<DiagStatus>;
  diagChooseCli(kind: 'claude_code_cli' | 'codex_cli', model: string, makeDefault: boolean): Promise<DiagStatus | null>;
  diagRemoveProvider(providerId: string): Promise<DiagStatus>;
  /** Opens the system credential prompt in the host; resolves false when cancelled. */
  diagSetKey(providerId: string): Promise<boolean>;
  diagForgetKey(providerId: string): Promise<DiagStatus>;
  diagProbe(providerId: string): Promise<ProbeReport>;
  diagPrepare(scanId: string, entryId: string, origin: Origin, providerId: string | null, disclosure: Disclosure, question: string | null): Promise<Preview>;
  diagFollowUp(diagnosisId: string, questionId: string | null, answer: string): Promise<Preview>;
  diagDiscard(previewId: string): Promise<boolean>;
  diagConfirm(previewId: string, payloadDigest: string): Promise<Diagnosis>;
  diagCancel(diagnosisId: string): Promise<Diagnosis>;
  diagGet(diagnosisId: string): Promise<Diagnosis>;
  diagList(cursor: string | null): Promise<{ items: DiagSummary[]; next_cursor: string | null }>;
  diagSave(diagnosisId: string): Promise<Diagnosis>;
  diagRemove(diagnosisId: string): Promise<boolean>;
  diagExport(diagnosisId: string): Promise<string>;
  diagHandoff(diagnosisId: string, action: NextAction, mode: 'trash' | 'quarantine' | null): Promise<HandoffResult>;
  onDiagFinished(cb: (e: DiagFinishedEvent) => void): Promise<() => void>;
}
