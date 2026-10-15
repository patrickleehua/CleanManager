// Typed wrappers over the host's explicit commands. Nothing here can reach a path, shell or
// filesystem API: the WebView only knows the commands below and their events. Plans,
// operations, quarantine items and policies are named by store-issued ids; approval needs a
// host-issued review token plus the code the person typed.

import type {
  ActiveApply,
  ApplyFinishedEvent,
  ApplyProgressEvent,
  ApplyStart,
  AppsList,
  Authorization,
  CancelReceipt,
  CandidatePage,
  CandidateSetInfo,
  Capabilities,
  EntriesPage,
  EntriesQuery,
  FinishedEvent,
  Group,
  HistoryPage,
  IpcError,
  ItemFilter,
  Mode,
  Operation,
  OperationItems,
  PlanDetail,
  PlanItems,
  PlansPage,
  Policy,
  PolicyState,
  ProgressEvent,
  ProtectionList,
  ProtectTarget,
  QuarantinePage,
  RecentScan,
  RootOffer,
  ScanStatus,
  StartScan,
  Summary,
  TreemapLayer,
} from './types';

export interface Backend {
  capabilities(): Promise<Capabilities>;
  listScanRoots(): Promise<RootOffer[]>;
  chooseFolder(): Promise<RootOffer | null>;
  recentScans(): Promise<RecentScan[]>;
  startScan(rootToken: string): Promise<StartScan>;
  scanStatus(scanId: string): Promise<ScanStatus>;
  cancelScan(scanId: string): Promise<CancelReceipt>;
  listEntries(query: EntriesQuery): Promise<EntriesPage>;
  treemapLayer(scanId: string, parentEntryId: string | null, limit: number): Promise<TreemapLayer>;
  summarizeSelection(scanId: string, entryIds: string[]): Promise<Summary>;
  // P04/P05
  candidateSummary(scanId: string): Promise<CandidateSetInfo | null>;
  analyzeScan(scanId: string): Promise<CandidateSetInfo>;
  listCandidates(setId: string, group: Group, pageSize: number, cursor: string | null): Promise<CandidatePage>;
  createPlan(setId: string, candidateIds: string[], mode: Mode): Promise<{ plan_id: string }>;
  planDetail(planId: string): Promise<PlanDetail>;
  planItems(planId: string, pageSize: number, cursor: string | null): Promise<PlanItems>;
  approvePlan(planId: string, reviewToken: string, typedCode: string): Promise<Authorization>;
  revokePlan(planId: string): Promise<Authorization>;
  listPolicies(): Promise<Policy[]>;
  setPolicyState(policyId: string, state: PolicyState, confirm: string | null): Promise<Policy[]>;
  createPolicyFromPlan(planId: string): Promise<Policy[]>;
  // P06/P08/P09
  startApply(planId: string): Promise<ApplyStart>;
  cancelApply(operationId: string): Promise<boolean>;
  activeApply(): Promise<ActiveApply | null>;
  operationDetail(operationId: string): Promise<Operation>;
  operationItems(operationId: string, filter: ItemFilter, pageSize: number, cursor: string | null): Promise<OperationItems>;
  reconcileOperation(operationId: string): Promise<Operation>;
  listOperations(pageSize: number, cursor: string | null): Promise<HistoryPage>;
  listPlans(pageSize: number, cursor: string | null): Promise<PlansPage>;
  listQuarantine(pageSize: number, cursor: string | null): Promise<QuarantinePage>;
  createRecoveryPlan(kind: 'restore' | 'purge', quarantineIds: string[]): Promise<{ plan_id: string }>;
  // P07/P10
  listProtection(): Promise<ProtectionList>;
  addProtection(target: ProtectTarget, note: string | null): Promise<ProtectionList>;
  removeProtection(protectionId: string, confirm: string): Promise<ProtectionList>;
  listApps(scanId: string | null): Promise<AppsList>;
  // Events
  onProgress(cb: (e: ProgressEvent) => void): Promise<() => void>;
  onFinished(cb: (e: FinishedEvent) => void): Promise<() => void>;
  onClosing(cb: (e: { waiting_for: string[] }) => void): Promise<() => void>;
  onApplyProgress(cb: (e: ApplyProgressEvent) => void): Promise<() => void>;
  onApplyFinished(cb: (e: ApplyFinishedEvent) => void): Promise<() => void>;
  /** True when the data comes from the development fixture, not a real scan. */
  readonly preview: boolean;
}

export function toIpcError(e: unknown): IpcError {
  if (e && typeof e === 'object' && 'code' in e && 'message' in e) {
    const o = e as Record<string, unknown>;
    return { code: String(o.code), reason: String(o.reason ?? o.code), message: String(o.message) };
  }
  return { code: 'invalid_argument', reason: 'ipc', message: typeof e === 'string' ? e : String(e) };
}

async function tauriBackend(): Promise<Backend> {
  const { invoke } = await import('@tauri-apps/api/core');
  const { listen } = await import('@tauri-apps/api/event');
  const call = async <T>(cmd: string, args?: Record<string, unknown>): Promise<T> => {
    try {
      return await invoke<T>(cmd, args);
    } catch (e) {
      throw toIpcError(e);
    }
  };
  return {
    preview: false,
    capabilities: () => call('capabilities'),
    listScanRoots: () => call('list_scan_roots'),
    chooseFolder: () => call('choose_folder'),
    recentScans: () => call('recent_scans'),
    startScan: (root_token) => call('start_scan', { root_token }),
    scanStatus: (scan_id) => call('scan_status', { scan_id }),
    cancelScan: (scan_id) => call('cancel_scan', { scan_id }),
    listEntries: (query) => call('list_entries', { query }),
    treemapLayer: (scan_id, parent_entry_id, limit) => call('treemap_layer', { scan_id, parent_entry_id, limit }),
    summarizeSelection: (scan_id, entry_ids) => call('summarize_selection', { scan_id, entry_ids }),
    candidateSummary: (scan_id) => call('candidate_summary', { scan_id }),
    analyzeScan: (scan_id) => call('analyze_scan', { scan_id }),
    listCandidates: (candidate_set_id, group, page_size, cursor) => call('list_candidates', { candidate_set_id, group, page_size, cursor }),
    createPlan: (candidate_set_id, candidate_ids, mode) => call('create_plan', { candidate_set_id, candidate_ids, mode }),
    planDetail: (plan_id) => call('plan_detail', { plan_id }),
    planItems: (plan_id, page_size, cursor) => call('plan_items', { plan_id, page_size, cursor }),
    approvePlan: (plan_id, review_token, typed_code) => call('approve_plan', { plan_id, review_token, typed_code }),
    revokePlan: (plan_id) => call('revoke_plan', { plan_id }),
    listPolicies: () => call('list_policies'),
    setPolicyState: (policy_id, new_state, confirm) => call('set_policy_state_cmd', { policy_id, new_state, confirm }),
    createPolicyFromPlan: (plan_id) => call('create_policy_from_plan', { plan_id }),
    startApply: (plan_id) => call('start_apply', { plan_id }),
    cancelApply: (operation_id) => call('cancel_apply', { operation_id }),
    activeApply: () => call('active_apply'),
    operationDetail: (operation_id) => call('operation_detail', { operation_id }),
    operationItems: (operation_id, filter, page_size, cursor) => call('list_operation_items', { operation_id, filter, page_size, cursor }),
    reconcileOperation: (operation_id) => call('reconcile_operation', { operation_id }),
    listOperations: (page_size, cursor) => call('list_operations', { page_size, cursor }),
    listPlans: (page_size, cursor) => call('list_plans', { page_size, cursor }),
    listQuarantine: (page_size, cursor) => call('list_quarantine', { page_size, cursor }),
    createRecoveryPlan: (kind, quarantine_ids) => call('create_recovery_plan', { kind, quarantine_ids }),
    listProtection: () => call('list_protection'),
    addProtection: (target, note) => call('add_protection', { target, note }),
    removeProtection: (protection_id, confirm) => call('remove_protection', { protection_id, confirm }),
    listApps: (scan_id) => call('list_apps', { scan_id }),
    onProgress: (cb) => listen<ProgressEvent>('scan-progress', (e) => cb(e.payload)),
    onFinished: (cb) => listen<FinishedEvent>('scan-finished', (e) => cb(e.payload)),
    onClosing: (cb) => listen<{ waiting_for: string[] }>('app-closing', (e) => cb(e.payload)),
    onApplyProgress: (cb) => listen<ApplyProgressEvent>('apply-progress', (e) => cb(e.payload)),
    onApplyFinished: (cb) => listen<ApplyFinishedEvent>('apply-finished', (e) => cb(e.payload)),
  };
}

export async function createBackend(): Promise<Backend> {
  const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
  if (inTauri) return tauriBackend();
  if (import.meta.env.DEV) {
    // Browser-only design preview. Never part of a production build.
    const { mockBackend } = await import('./mock');
    return mockBackend();
  }
  throw new Error('CleanManager must run inside its desktop host.');
}
