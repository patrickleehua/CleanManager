// IPC shapes (mirrors src-tauri/src/dto.rs). Byte counts are decimal strings or null.

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
  write_commands: boolean;
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

export interface Issue {
  code: string;
  scope_display: string;
  message: string;
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
  boundary: 'symlink' | 'junction' | 'mount_point' | 'other_volume' | 'cloud_provider' | null;
  identity_quality: 'strong' | 'weak';
  link_count: number | null;
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
