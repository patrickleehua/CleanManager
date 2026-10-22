// IPC shapes (mirrors src-tauri/src/dto.rs). Byte counts are decimal strings or null.

import type { AppOutcome, UninstallPlanCreated, UpdateRouted } from './suiteTypes';

export type ScanState = 'created' | 'running' | 'stopping' | 'completed' | 'partial' | 'cancelled' | 'failed' | 'interrupted';
export type ObjectKind = 'file' | 'directory' | 'symlink' | 'reparse' | 'other';
export type SortKey = 'logical_desc' | 'allocated_desc' | 'name_asc';
export type KindFilter = 'any' | 'file' | 'directory';
export type SizeBasis = 'entity' | 'subtree_known' | 'unknown';

export interface IpcError {
  code: string;
  reason: string;
  message: string;
}

export interface Capability {
  name: string;
  state: 'available' | 'permission_required' | 'unsupported' | 'unverified';
  reason: string | null;
}

export interface Capabilities {
  platform: 'windows' | 'macos';
  engine_version: string;
  schema_version: string;
  capabilities: Capability[];
  store_dir_display: string | null;
  store_problem: IpcError | null;
  bypass_privileges_disabled: boolean | null;
  bypass_privileges_note: string | null;
  interrupted_at_startup: string[];
  operations_to_reconcile: string[];
  write_commands: boolean;
  file_actions_verified: boolean;
  rules_dir_display: string | null;
  rules_problem: string | null;
  rule_count: number | null;
  principal: string;
  /** D15: capabilities whose release gate has not passed are off unless this is on (设置 › 实验功能). */
  experimental_features: boolean;
  /** One row per gated capability: its release gate, whether it passed and whether it is on here. */
  gates: GateRow[];
}

export interface GateRow {
  capability: string;
  gate: string;
  gate_passed: boolean;
  enabled: boolean;
}

export interface RootOffer {
  token: string;
  label: string;
  kind: 'user_home' | 'user_folder' | 'volume' | 'chosen';
  path_display: string;
  problem: IpcError | null;
}

export interface RecentScan {
  scan_id: string;
  root_display: string;
  started_at: string;
  state: ScanState | null;
  published: boolean;
  host_kind: string;
  completeness: string | null;
  finished_at: string | null;
  problem: IpcError | null;
}

export interface Summary {
  logical_known_bytes: string;
  logical_unknown_count: number;
  allocated_exact_bytes: string | null;
  allocated_known_bytes: string;
  allocated_unknown_count: number;
  covered_entries: number;
  overlapping_selected: number;
  unread_ranges: number;
}

/** Why a permission issue or unread range happened: macOS still shows its privacy prompt, or the cause is not known. */
export type PermissionCause = 'privacy_prompt_pending' | 'undetermined';

export interface Issue {
  code: string;
  scope_display: string;
  message: string;
  /** Structured cause of a `permission_required` issue (older hosts put `permission_cause=…` in the message). */
  permission_cause?: PermissionCause | null;
}

export interface ScanStatus {
  scan_id: string;
  scope_id: string | null;
  state: ScanState;
  revision: number | null;
  published: boolean;
  completeness: 'verified_scope' | 'partial_scope' | string | null;
  cancel_requested: boolean;
  host_kind: string | null;
  host_pid: number | null;
  hosted_here: boolean;
  heartbeat_at: string | null;
  entries_seen: number;
  skipped_ranges: number;
  started_at: string | null;
  finished_at: string | null;
  root_display: string | null;
  root_entry_id: string | null;
  /** Null until published, for an empty scan, and always for a `failed` scan (its sizes are unknown, never 0). A
   *  summary that is present always carries decimal strings. */
  summary: Summary | null;
  backend: string | null;
  backend_reason: string | null;
  elevated_read: string[];
  issues: Issue[];
  issues_total: number;
}

export interface Entry {
  entry_id: string;
  parent_entry_id: string | null;
  name: string;
  path_display: string;
  kind: ObjectKind;
  logical_bytes: string | null;
  allocated_bytes: string | null;
  size_basis: SizeBasis;
  unknown_descendants: number | null;
  boundary: 'symlink' | 'junction' | 'mount_point' | 'other_volume' | 'cloud_provider' | 'own_store' | null;
  identity_quality: 'strong' | 'weak';
  link_count: number | null;
  /** DIN-05: read-only "cleanable by hand" hint on a listed directory; absent or null = none confirmed. */
  cleanable_hint?: CleanableHint | null;
}

/** The verified evidence behind a cleanable hint and the review flow it leads to (never a selection). */
export interface CleanableHint {
  basis: 'build_artifact' | 'catalog_rule' | 'cachedir_tag';
  /** `purge.<artifact>` or the catalog rule; null for a cache tag. */
  rule_id: string | null;
  /** Owner markers found, the rule's location, or the tag file. */
  detail: string;
  flow: 'purge' | 'manual_trash';
}

export interface EntriesPage {
  scan_id: string;
  scope_id: string;
  revision: number;
  sort: SortKey;
  kind: KindFilter;
  name_contains: string | null;
  parent_entry_id: string | null;
  page_size: number;
  items: Entry[];
  next_cursor: string | null;
  unknown_size_items: number;
}

export interface EntriesQuery {
  scan_id: string;
  sort: SortKey;
  kind: KindFilter;
  name_contains: string | null;
  parent_entry_id: string | null;
  page_size: number;
  cursor: string | null;
}

export interface TreemapLayer {
  scan_id: string;
  revision: number;
  parent_entry_id: string;
  parent_summary: Summary;
  items: Entry[];
  rest: { count: number; known_logical_bytes: string; unknown_count: number; complete: boolean };
}

export interface StartScan {
  scan_id: string;
  root_display: string;
}

export interface CancelReceipt {
  scan_id: string;
  accepted: boolean;
  state_at_receipt: string;
}

export interface ProgressEvent {
  scan_id: string;
  state: ScanState;
  entries_seen: number;
  skipped_ranges: number;
  cancel_requested: boolean;
  heartbeat_at: string;
}

export interface FinishedEvent {
  scan_id: string;
  state: ScanState | null;
  error: IpcError | null;
}

/** `features-changed`: the experimental-features switch changed outside this window (the CLI). */
export interface FeaturesChangedEvent {
  experimental_features: boolean;
  /** The resident utilities were stopped because the switch was turned off. */
  resident_stopped: boolean;
  /** They stopped, but the startup value or the saved settings could not be updated (retried at the next start). */
  error: string | null;
}

// ---------------------------------------------------------------------------------------------
// P04–P10 (mirrors src-tauri/src/{cleanup,ops,settings}.rs)

export type Mode = 'trash' | 'quarantine' | 'delete';
export type Tier = 'default_selected' | 'review_only' | 'blocked';
export type CandidateState = 'allowed' | 'protected_reviewable' | 'pending_evidence' | 'blocked';
export type Group = 'low' | 'review' | 'pending' | 'blocked';
export type NotSelectable =
  | 'blocked'
  | 'protected_reviewable'
  | 'pending_evidence'
  | 'no_mode'
  | 'unknown_size'
  | 'hint'
  | 'adapter'
  | 'in_use'
  | 'probe_unknown'
  | 'needs_reanalysis';

export interface GroupCount {
  count: number;
  known_logical_bytes: string;
  unknown_size_count: number;
}

export interface CandidateSetInfo {
  candidate_set_id: string;
  scan_id: string;
  revision: number;
  created_at: string | null;
  rule_set_digest: string;
  rules_current: boolean | null;
  scan_completeness: string | null;
  scan_root_display: string | null;
  groups: Record<Group, GroupCount>;
  issues: Issue[];
}

export interface Candidate {
  candidate_id: string;
  entry_id: string;
  name: string;
  path_display: string;
  kind: ObjectKind;
  rules: string[];
  category: string;
  group: Group;
  state: CandidateState;
  tier: Tier;
  not_selectable: NotSelectable | null;
  reasons: string[];
  protection: { class: string; describe: string; reviewable: boolean } | null;
  logical_known_bytes: string;
  unknown_size_count: number;
  logical_exact_bytes: string | null;
  allowed_modes: Mode[];
  impact: string;
  probes: string[];
  /** `file` rows get file actions; `hint` rows are guidance only; `adapter` rows belong to the owning tool. */
  row_kind: 'file' | 'hint' | 'adapter';
  registry_status: 'covered' | 'version_untested' | 'not_registered' | 'not_checked' | null;
  installed_version: string | null;
  rebuild_class: string | null;
  adapter_ref: string | null;
  probe_results: { probe: string; state: 'in_use' | 'not_in_use' | 'unknown'; detail: string }[];
  catalog_fresh: boolean;
}

export interface CandidatePage {
  candidate_set_id: string;
  group: Group;
  total: number;
  items: Candidate[];
  next_cursor: string | null;
}

export type AuthStatus = 'authorized' | 'required' | 'expired' | 'revoked';

export interface Authorization {
  status: AuthStatus;
  source: 'interactive_review' | 'policy' | null;
  policy_id: string | null;
  policy_version: number | null;
  principal: string | null;
  authorized_at: string | null;
  expires_at: string | null;
}

export interface PolicyMatch {
  policy_id: string;
  version: number;
  state: PolicyState;
  covers: boolean;
  reason: string | null;
  detail: string | null;
}

export type PlanKind =
  | 'cleanup'
  | 'restore'
  | 'purge'
  | 'manual_trash'
  | 'manual_purge'
  | 'startup'
  | 'startup_restore'
  | 'uninstall'
  | 'uninstall_leftovers'
  | 'uninstall_restore'
  | 'update'
  | 'process'
  | 'maintenance';

export interface PlanDetail {
  plan_id: string;
  contract: string;
  kind: PlanKind;
  mode: Mode | null;
  digest: string;
  digest_prefix: string;
  created_at: string;
  expires_at: string;
  expired: boolean;
  seconds_left: number;
  scan_id: string | null;
  scan_revision: number | null;
  scope_id: string | null;
  scan_root_display: string | null;
  rule_refs: string[];
  rule_set_digest: string | null;
  stale_reason: string | null;
  item_count: number;
  total_logical_bytes: string | null;
  allocated_known_bytes: string | null;
  allocated_unknown_count: number;
  default_selected_items: number;
  authorization: Authorization;
  policies: PolicyMatch[];
  not_included: { low_unselected: number; review_unselected: number; pending: number; blocked: number } | null;
  operation: { operation_id: string; state: string } | null;
  review_token: string | null;
  cli: { show: string; review: string; apply: string };
  /** System-wide changes: applying needs an elevated host (the app never elevates itself). */
  requires_admin?: boolean;
  /** Built-in catalog of a cleanup plan: build artifacts or installer packages. */
  catalog?: Catalog | null;
  plan_kind_v2?: string | null;
  /** Only a terminal review can approve it: `permanent_delete` or `end_processes`. */
  terminal_only?: 'permanent_delete' | 'end_processes' | null;
  /** Kind-specific review facts (uninstall stage and app, maintenance elevation, notes from plan creation). */
  kind_detail?: KindDetail | null;
}

export interface KindDetail {
  stage?: 'uninstall' | 'leftovers';
  app?: { name: string; publisher: string | null; version: string | null; source: string; adapter: string; signer: string | null } | null;
  /** UNI-11: every application of an uninstall plan, in plan order, with the actions bound to it. */
  apps?: { app_instance_id: string; name: string; publisher: string | null; version: string | null; source: string; adapter: string; signer: string | null; action_ids: string[] }[];
  may_end_processes?: boolean;
  registry_keys?: number;
  admin_tasks?: number;
  is_admin?: boolean | null;
  noted?: {
    app_name?: string;
    leftover_estimate?: Record<string, number>;
    /** UNI-11: per application, and the applications refused (not in the plan). */
    apps?: UninstallPlanCreated['apps'];
    refused?: UninstallPlanCreated['refused'];
    operation_id?: string;
    routed?: UpdateRouted[];
    pid?: number;
    name?: string;
    purpose?: string | null;
    unidentified_reason?: string | null;
  } | null;
}

export interface PlanItem {
  action_id: string;
  name: string;
  path_display: string;
  logical_bytes: string;
  allocated_bytes: string | null;
  candidate_id: string | null;
  original_path_display: string | null;
  quarantine_id: string | null;
  item_kind: string;
  /** plan/v2 action, e.g. `fs_trash`, `startup_disable`, `startup_enable`. */
  action_type?: string | null;
  /** Startup items: `user` or `machine`. */
  scope?: string | null;
  /** Kind-specific facts (update target, uninstaller, process identity, maintenance task, leftover group). */
  detail?: Record<string, unknown> | null;
}

export interface PlanItems {
  plan_id: string;
  total: number;
  items: PlanItem[];
  next_cursor: string | null;
}

export type PolicyState = 'enabled' | 'paused' | 'revoked';

export interface Policy {
  policy_id: string;
  version: number;
  state: PolicyState;
  expired: boolean;
  principal: string;
  scope_id: string;
  rules: string[];
  mode: Mode;
  created_at: string;
  expires_at: string;
  single_max_bytes: string;
  single_max_items: number;
  window_max_bytes: string;
  window_max_items: number;
  window_used_bytes: string;
  window_used_items: number;
}

export interface Counts {
  succeeded: number;
  skipped: number;
  failed: number;
  not_started: number;
  unknown: number;
  in_flight: number;
}

export interface ApplyStart {
  plan_id: string;
  operation_id: string;
  total: number;
  existing: boolean;
}

export interface ActiveApply {
  plan_id: string;
  operation_id: string;
  total: number;
  running: boolean;
  cancel_requested: boolean;
}

export interface Operation {
  operation_id: string;
  plan_id: string;
  plan_kind: PlanKind | 'unknown';
  mode: Mode | null;
  state: string;
  running_here: boolean;
  cancel_requested: boolean;
  started_at: string | null;
  finished_at: string | null;
  authorization_source: string | null;
  total: number;
  counts: Counts;
  report_available: boolean;
  processed_logical_bytes: string | null;
  quarantined_logical_bytes: string | null;
  estimated_freed_bytes: string | null;
  volume_free_before_bytes: string | null;
  volume_free_after_bytes: string | null;
  observed_free_delta_bytes: string | null;
  needs_reconcile: boolean;
  /** UNI-11: each application's own result of a stage-1 uninstall (empty for other plans). */
  uninstall_apps?: AppOutcome[];
}

export type ItemStatus = 'succeeded' | 'skipped' | 'failed' | 'not_started' | 'unknown' | 'in_flight' | 'pending';
export type ItemFilter = 'all' | 'problems' | 'succeeded' | 'skipped' | 'failed' | 'not_started' | 'unknown';

export interface OperationItem {
  action_id: string;
  name: string;
  path_display: string;
  original_path_display: string | null;
  logical_bytes: string | null;
  status: ItemStatus;
  code: string | null;
  message: string | null;
  recovery: string | null;
}

export interface OperationItems {
  operation_id: string;
  total: number;
  items: OperationItem[];
  next_cursor: string | null;
}

export interface ApplyProgressEvent {
  plan_id: string;
  operation_id: string;
  total: number;
  counts: Counts;
  cancel_requested: boolean;
}

export interface ApplyFinishedEvent {
  plan_id: string;
  operation_id: string;
  status: string | null;
  error: IpcError | null;
}

export type Availability = 'in_app_restore' | 'system_recycle_bin' | 'unavailable' | 'not_applicable' | 'unknown';

export interface HistoryItem {
  operation_id: string;
  plan_id: string;
  contract: string | null;
  action: 'trash' | 'quarantine' | 'delete' | 'restore' | 'purge' | null;
  state: string;
  started_at: string;
  finished_at: string | null;
  authorization_source: string | null;
  rule_refs: string[];
  counts: Counts | null;
  processed_logical_bytes: string | null;
  availability: Availability;
}

export interface HistoryPage {
  items: HistoryItem[];
  next_cursor: string | null;
}

export interface PlanRow {
  plan_id: string;
  contract: string;
  kind: PlanKind;
  mode: Mode | null;
  item_count: number | null;
  created_at: string;
  expires_at: string;
  expired: boolean;
  authorization: AuthStatus | null;
  operation_id: string | null;
  problem: string | null;
}

export interface PlansPage {
  items: PlanRow[];
  next_cursor: string | null;
}

export interface QuarantineItem {
  quarantine_id: string;
  plan_id: string;
  operation_id: string;
  name: string;
  original_path_display: string;
  location_display: string;
  logical_bytes: string;
  quarantined_at: string;
  reminder_at: string;
  reminder_due: boolean;
  state: string;
  actionable: boolean;
}

export interface QuarantinePage {
  items: QuarantineItem[];
  next_cursor: string | null;
  held_count: number | null;
  held_logical_bytes: string | null;
  metadata_preserved: string[];
  metadata_limits: string[];
}

export interface ProtectionItem {
  protection_id: string;
  layer: 'hard_deny' | 'default_category' | 'user';
  match: string;
  display: string;
  removable: boolean;
  note: string | null;
  added_at: string | null;
}

export interface ProtectionList {
  items: ProtectionItem[];
  problem: string | null;
  file_display: string | null;
}

export type ProtectTarget =
  | { source: 'entry'; scan_id: string; entry_id: string; recursive: boolean }
  | { source: 'folder'; root_token: string }
  | { source: 'rule'; rule_id: string };

export interface AppItem {
  instance_id: string;
  name: string;
  version: string | null;
  publisher: string | null;
  source: 'registry_uninstall' | 'msi' | 'msix_store' | 'app_bundle' | 'app_store' | 'homebrew' | 'unknown';
  location_display: string | null;
  evidence: string[];
  protected_reason: string | null;
  uninstall_state: string;
  uninstall_reason: string | null;
  update_status: string;
  shared_with: string[];
  size: { state: 'known' | 'not_in_scan' | 'no_location' | 'unresolved'; logical_known_bytes: string | null; unknown_descendants: number | null; entry_id: string | null };
}

export interface AppsList {
  items: AppItem[];
  issues: Issue[];
  scan_id: string | null;
  scan_root_display: string | null;
  supported: boolean;
}

// ---------------------------------------------------------------------------------------------
// Five-tool suite (D14): catalogs, manual trash, startup, insights, status, readiness.

/** A path as the protocol carries it; the UI only ever shows `display`. */
export interface PathWire {
  display: string;
  [k: string]: unknown;
}

export interface PurgeRoot {
  root_id: string;
  display: string;
  source: 'default' | 'user';
  exists: boolean;
}

export interface PurgeRoots {
  supported: boolean;
  roots: PurgeRoot[];
  recent_days: number;
  max_depth: number;
}

export type Catalog = 'purge' | 'installer';

export interface CatalogItem {
  candidate_set_id: string;
  candidate_id: string;
  entry_id: string;
  name: string;
  path_display: string;
  tier: Tier;
  tier_reason: string;
  skip_reason: string | null;
  plannable: boolean;
  selected_by_default: boolean;
  allowed_modes: Mode[];
  logical_bytes: string | null;
  unknown_size_count: number;
  rebuild_class: string | null;
  impact: string;
  notes: string[];
  group_label: string;
}

export interface CatalogPreview {
  supported: boolean;
  catalog: Catalog;
  observed_at: string;
  sets: { candidate_set_id: string; scan_id: string; root_display: string; source_label: string | null }[];
  items: CatalogItem[];
  summary: {
    default_selected_count: number;
    default_selected_known_bytes: string;
    default_selected_unknown_count: number;
    review_only_count: number;
    blocked_count: number;
    skipped_count: number;
  };
  issues: Issue[];
}

export interface InstallerLocations {
  supported: boolean;
  locations: { display: string; source_label: string; exists: boolean }[];
  max_depth: number;
}

export interface TrashInspection {
  scan_id: string;
  entry_id: string;
  name: string;
  path_display: string;
  parent_display: string | null;
  object_kind: string;
  cloud: string;
  logical_bytes: string | null;
  allocated_bytes: string | null;
  files: number;
  directories: number;
  protected: { path_display: string; reason: string }[];
  protected_count: number;
  coverage_gaps: { path_display: string; reason: string }[];
  gap_count: number;
  trashable: boolean;
  refusal: { code: string; reason: string; message: string } | null;
  holders: string[] | null;
  recycle_bin_ready: boolean;
  recycle_bin_problem: string | null;
  guidance: string;
}

export interface StartupItem {
  startup_item_id: string;
  name: string;
  source: string;
  definition: string;
  target: PathWire | null;
  publisher: string | null;
  publisher_evidence: string;
  scope: string;
  state: string;
  toggle_capability: string;
  protected: boolean;
  reason: string | null;
}

export interface StartupList {
  supported: boolean;
  is_admin: boolean;
  items: StartupItem[];
  issues: Issue[];
}

export interface Insight {
  insight_id: string;
  kind: string;
  title: string;
  path: PathWire | null;
  logical_bytes: string | null;
  count: number | null;
  size_basis: string;
  size_note: string | null;
  observed_at: string | null;
  /** Structured cause named by `size_note` (absent when it names none). */
  permission_cause?: PermissionCause | null;
  explanation: string;
  handling: string;
  guidance: { tool: string; steps: string; requires_admin: boolean; impact: string }[];
}

export interface InsightsList {
  supported: boolean;
  observed_at: string;
  items: Insight[];
}

export interface StatusMetric {
  name: string;
  availability: string;
  value: number | null;
  unit: string;
  reason: string | null;
}

export interface StatusSnapshot {
  supported: boolean;
  sampled_at?: string;
  stale?: boolean;
  metrics?: StatusMetric[];
  health?: {
    score: number | null;
    weights: Record<string, number>;
    lowered_by: string[];
    dimensions: { dimension: string; metric: string; value: number | null; full_score_at: number; zero_score_at: number; score: number | null }[];
  };
  diagnosis?: { code: string; message: string; link: string | null } | null;
  /** STS-05: sustained-CPU alerts raised with this sample (only while alerts are on). */
  alerts?: CpuAlert[];
  /** STS-07: totals over physical interfaces; tunnel and virtual adapters listed apart. */
  network?: NetworkTotals;
}

export type InterfaceClass = 'physical' | 'tunnel' | 'virtual' | 'unknown';

export interface InterfaceRate {
  name: string;
  class: InterfaceClass;
  received_bytes_per_sec: number;
  sent_bytes_per_sec: number;
}

export interface NetworkTotals {
  classification: 'interface_type' | 'unavailable';
  received_bytes_per_sec: number;
  sent_bytes_per_sec: number;
  may_double_count: boolean;
  counted: InterfaceRate[];
  separate: InterfaceRate[];
  loopback_excluded: number;
  note: string;
}

export interface CpuAlert {
  pid: number;
  name: string;
  image_path: string | null;
  min_cpu_percent: number;
  threshold_percent: number;
  window_seconds: number;
  since: string;
  raised_at: string;
  link: string;
}

export type ReadinessOp = 'scan' | 'cleanup' | 'purge' | 'installer' | 'trash' | 'startup' | 'apply' | 'uninstall' | 'update' | 'maintenance' | 'process';

export interface ReadinessCheck {
  check_id: string;
  state: 'ready' | 'missing' | 'unknown' | 'not_applicable';
  blocking: boolean;
  detail: string | null;
}

export interface Readiness {
  platform: string;
  operation: string;
  is_admin: boolean | null;
  checks: ReadinessCheck[];
}
