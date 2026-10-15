// IPC shapes of 软件 › 卸载 / 更新, 优化, 状态 › 进程 and the space-insight overview (mirrors
// src-tauri/src/{software,system}.rs). Byte counts are decimal strings or null.

import type { Issue } from './types';

// ---------------------------------------------------------------------------------------------
// 软件 › 卸载 (UNI-01..10)

export interface UninstallApp {
  app_instance_id: string;
  name: string;
  version: string | null;
  publisher: string | null;
  source: string;
  location: string | null;
  last_used_at: string | null;
  /** Security agents, system components, shared runtimes, CleanManager: no uninstall entry at all. */
  protected_class: boolean;
  /** `available`, `unverified` (no verifiable route: Settings) or `unsupported` (protected). */
  state: 'available' | 'unverified' | 'unsupported';
  adapter: { adapter_id: string; display: string; package_manager: boolean } | null;
  reason: string | null;
  settings_uri: string | null;
}

export interface UninstallRun {
  operation_id: string;
  plan_id: string;
  app_name: string;
  stage: 'uninstall' | 'leftovers';
  state: string;
  started_at: string;
  finished_at: string | null;
}

export interface UninstallList {
  supported: boolean;
  is_admin: boolean;
  items: UninstallApp[];
  issues: Issue[];
  recent: UninstallRun[];
}

export interface UninstallPlanCreated {
  plan_id: string;
  leftover_estimate: Record<string, number>;
}

export type LeftoverTier = 'default_selected' | 'review_only' | 'blocked' | 'kept';

export interface Leftover {
  object_id: string;
  kind: string;
  group: string;
  display: string;
  evidence: string[];
  marker: 'shared_owner' | 'owner_unknown' | 'app_reappeared' | null;
  tier: LeftoverTier;
  /** Preselected by the core (caches and logs only). */
  selected: boolean;
  selectable: boolean;
  why_not: 'kept' | 'blocked' | 'manual' | 'needs_admin' | null;
  warnings: string[];
  logical_bytes: string | null;
  note: string | null;
  requires_admin: boolean;
  action: 'fs_trash' | 'registry_backup_delete' | 'startup_toggle' | 'manual';
  guidance: string | null;
}

export interface Leftovers {
  operation_id: string;
  app: { app_instance_id: string; name: string; publisher: string | null; source: string };
  is_admin: boolean;
  groups: { group: string; items: Leftover[] }[];
  default_selected: number;
  /** Home-folder configuration files that mention the install folder: read-only hint. */
  external_references: string[];
}

// ---------------------------------------------------------------------------------------------
// 软件 › 更新 (UPD-01..04)

export type UpdateState = 'update_available' | 'up_to_date' | 'cannot_check' | 'not_checked';
export type UpdateRoute = 'package_manager' | 'open_store' | 'open_app' | 'open_official_page' | 'none';
export type UpdateChannel = 'winget' | 'microsoft_store' | 'scoop' | 'self_updater';

export interface UpdateCheck {
  app_instance_id: string;
  name: string;
  current_version: string | null;
  available_version: string | null;
  channel: UpdateChannel | null;
  channel_id: string | null;
  state: UpdateState;
  reason: string | null;
  checked_at: string | null;
  route: UpdateRoute;
  official_url: string | null;
}

export interface UpdateList {
  supported: boolean;
  checks_enabled: boolean;
  /** A detection pass ran now (false: cached results or not checked). */
  checked: boolean;
  checked_at: string | null;
  counts: Partial<Record<UpdateState, number>>;
  items: UpdateCheck[];
}

export interface UpdateRouted {
  app_instance_id: string;
  name: string;
  route: UpdateRoute;
  reason: string;
  url: string | null;
}

export interface UpdatePlanCreated {
  plan_id: string | null;
  planned: number;
  routed: UpdateRouted[];
}

// ---------------------------------------------------------------------------------------------
// 优化 (MNT-01..07)

export type Applicability = 'applicable' | 'not_needed' | 'not_applicable' | 'unknown';

export interface MaintenanceTask {
  task_id: string;
  task_version: string;
  title: string;
  problem: string;
  impact: string;
  requires_admin: boolean;
  applies_when: string;
  estimated_seconds: number;
  applicability: Applicability;
  applicability_reason: string | null;
  user_protected: boolean;
  reversible: boolean;
}

export interface Prohibited {
  id: string;
  title: string;
  reason: string;
}

export interface MaintenanceCatalog {
  supported: boolean;
  items: MaintenanceTask[];
  default_selection: string[];
  prohibited: Prohibited[];
  is_admin: boolean;
  elevation: 'already_elevated' | 'one_prompt_per_run' | 'unavailable';
  observed_at?: string;
}

export interface MaintenanceSearch {
  query: string;
  task_ids: string[];
  prohibited: Prohibited[];
}

export type TaskOutcome = 'applied' | 'unchanged' | 'skipped' | 'unavailable' | 'attention' | 'failed';

export interface TaskRun {
  task_id: string;
  task_version: string;
  requires_admin: boolean;
  outcome: TaskOutcome;
  code: string;
  reason: string;
  elevated: boolean;
  started_at: string;
  finished_at: string;
}

export interface MaintenanceReport {
  operation_id: string;
  plan_id: string;
  plan_digest: string;
  entry: string;
  principal: string;
  elevation: 'not_needed' | 'already_elevated' | 'granted' | 'declined' | 'unavailable';
  started_at: string;
  finished_at: string;
  items: TaskRun[];
  counts: Record<TaskOutcome, number>;
  all_clear: boolean;
  undo: string;
}

export interface MaintenanceHistory {
  items: MaintenanceReport[];
  undo: string;
}

export interface DiagnosisFinding {
  code: string;
  severity: string;
  subject: string;
  detail: string;
  process_id: number | null;
  duration_seconds: number | null;
  next_step: string;
}

export interface MaintenanceDiagnosis {
  supported: boolean;
  sampled_at?: string;
  window_seconds?: number;
  read_only: boolean;
  findings: DiagnosisFinding[];
  top_processes: { process_id: number; name: string; image: string | null; cpu_core_percent: number; working_set_bytes: string }[];
  memory_load_percent?: number | null;
  commit_used_percent?: number | null;
}

// ---------------------------------------------------------------------------------------------
// 状态 › 进程 (STS-04/05)

export type ProcessSort = 'cpu' | 'memory' | 'disk' | 'network' | 'name';

export interface ProcessRow {
  pid: number;
  parent_pid: number | null;
  parent_name: string | null;
  name: string;
  image_path: string | null;
  started_at: string | null;
  cpu_percent: number | null;
  memory_bytes: string | null;
  disk_bytes_per_sec: number | null;
  network_bytes_per_sec: number | null;
  signature: { state: 'signed' | 'catalog_signed' | 'unsigned' | 'unknown'; publisher: string | null; reason: string | null } | null;
  owner_app: string | null;
  identified: boolean;
  purpose: string | null;
  unidentified_reason: string | null;
  ai_diagnosis: { available: boolean; hint: string } | null;
  pinned: boolean;
  end_capability: { state: 'available' | 'unsupported'; protection: string | null; reason: string };
  notes: string[];
}

export interface ProcessList {
  supported: boolean;
  sampled_at?: string;
  window_ms?: number;
  sort?: ProcessSort;
  search?: string | null;
  total?: number;
  matched?: number;
  unidentified?: number;
  items: ProcessRow[];
  notes?: string[];
  pins: string[];
}

export interface AlertSettings {
  enabled: boolean;
  cpu_threshold_percent: number;
  window_seconds: number;
  cooldown_seconds: number;
}

// ---------------------------------------------------------------------------------------------
// 分析 › 空间洞察: overview cards and old downloads (DIN-03/04)

export interface InsightCard {
  card_id: string;
  kind: string;
  title: string;
  logical_bytes: string | null;
  count: number | null;
  size_note: string | null;
  observed_at: string | null;
}

export interface DownloadItem {
  item_id: string;
  name: string;
  path_display: string;
  logical_bytes: string;
  modified_at: string;
  age_days: number;
  installer_like: boolean;
}

export interface OldDownloads {
  root_display: string | null;
  older_than_days: number;
  observed_at: string;
  item_count: number;
  total_logical_bytes: string | null;
  note: string | null;
  /** Largest first; nothing is selected. */
  items: DownloadItem[];
}

export interface InsightOverview {
  supported: boolean;
  generated_at: string;
  cards: InsightCard[];
  downloads: OldDownloads | null;
}
