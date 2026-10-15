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
  Catalog,
  CatalogPreview,
  InsightsList,
  InstallerLocations,
  PurgeRoots,
  Readiness,
  ReadinessOp,
  StartupList,
  StatusSnapshot,
  TrashInspection,
} from './types';
import type { DiagBackend, DiagFinishedEvent } from './diagTypes';

export interface Backend extends DiagBackend {
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
  // Five-tool suite: catalogs (build artifacts, installers), manual trash, startup, insights,
  // status, permission readiness. Writes still only create plans for the review page.
  purgeRoots(): Promise<PurgeRoots>;
  /** Opens the native folder picker in the host; `null` when cancelled. */
  purgeRootsAdd(): Promise<PurgeRoots | null>;
  purgeRootsRemove(rootId: string): Promise<PurgeRoots>;
  purgeScan(): Promise<CatalogPreview>;
  installerLocations(): Promise<InstallerLocations>;
  installerScan(): Promise<CatalogPreview>;
  createCatalogPlan(catalog: Catalog, setId: string, candidateIds: string[], mode: Mode): Promise<{ plan_id: string; stale_items?: { candidate_id: string; reason: string; detail: string }[] }>;
  trashInspect(scanId: string, entryId: string): Promise<TrashInspection>;
  trashPlan(scanId: string, entryIds: string[]): Promise<{ plan_id: string }>;
  startupList(): Promise<StartupList>;
  startupPlan(itemId: string, operation: 'disable' | 'enable'): Promise<{ plan_id: string }>;
  insightsList(): Promise<InsightsList>;
  statusSnapshot(): Promise<StatusSnapshot>;
  readiness(operation: ReadinessOp): Promise<Readiness>;
  /** Shows a scanned entry in the platform file manager (path resolved by the host). */
  revealEntry(scanId: string, entryId: string): Promise<boolean>;
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
    purgeRoots: () => call('purge_roots'),
    purgeRootsAdd: () => call('purge_roots_add'),
    purgeRootsRemove: (root_id) => call('purge_roots_remove', { root_id }),
    purgeScan: () => call('purge_scan'),
    installerLocations: () => call('installer_locations'),
    installerScan: () => call('installer_scan'),
    createCatalogPlan: (catalog, candidate_set_id, candidate_ids, mode) => call('create_catalog_plan', { catalog, candidate_set_id, candidate_ids, mode }),
    trashInspect: (scan_id, entry_id) => call('trash_inspect', { scan_id, entry_id }),
    trashPlan: (scan_id, entry_ids) => call('trash_plan', { scan_id, entry_ids }),
    startupList: () => call('startup_list'),
    startupPlan: (item_id, operation) => call('startup_plan', { item_id, operation }),
    insightsList: () => call('insights_list'),
    statusSnapshot: () => call('status_snapshot'),
    readiness: (operation) => call('readiness', { operation }),
    revealEntry: (scan_id, entry_id) => call('reveal_entry', { scan_id, entry_id }),
    onProgress: (cb) => listen<ProgressEvent>('scan-progress', (e) => cb(e.payload)),
    onFinished: (cb) => listen<FinishedEvent>('scan-finished', (e) => cb(e.payload)),
    onClosing: (cb) => listen<{ waiting_for: string[] }>('app-closing', (e) => cb(e.payload)),
    onApplyProgress: (cb) => listen<ApplyProgressEvent>('apply-progress', (e) => cb(e.payload)),
    onApplyFinished: (cb) => listen<ApplyFinishedEvent>('apply-finished', (e) => cb(e.payload)),
    // Optional AI diagnosis: ids only; keys go through the host's system prompt.
    diagStatus: () => call('diag_status'),
    diagSaveProvider: (input) => call('diag_save_provider', { input }),
    diagChooseCli: (kind, model, make_default) => call('diag_choose_cli', { kind, model, make_default }),
    diagRemoveProvider: (provider_id) => call('diag_remove_provider', { provider_id }),
    diagSetKey: (provider_id) => call('diag_set_key', { provider_id }),
    diagForgetKey: (provider_id) => call('diag_forget_key', { provider_id }),
    diagProbe: (provider_id) => call('diag_probe', { provider_id }),
    diagPrepare: (scan_id, entry_id, origin, provider_id, disclosure, question) => call('diag_prepare', { scan_id, entry_id, origin, provider_id, disclosure, question }),
    diagFollowUp: (diagnosis_id, question_id, answer) => call('diag_follow_up', { diagnosis_id, question_id, answer }),
    diagDiscard: (preview_id) => call('diag_discard', { preview_id }),
    diagConfirm: (preview_id, payload_digest) => call('diag_confirm', { preview_id, payload_digest }),
    diagCancel: (diagnosis_id) => call('diag_cancel', { diagnosis_id }),
    diagGet: (diagnosis_id) => call('diag_get', { diagnosis_id }),
    diagList: (cursor) => call('diag_list', { cursor }),
    diagSave: (diagnosis_id) => call('diag_save', { diagnosis_id }),
    diagRemove: (diagnosis_id) => call('diag_remove', { diagnosis_id }),
    diagExport: (diagnosis_id) => call('diag_export', { diagnosis_id }),
    diagHandoff: (diagnosis_id, action, mode) => call('diag_handoff', { diagnosis_id, action, mode }),
    onDiagFinished: (cb) => listen<DiagFinishedEvent>('diag-finished', (e) => cb(e.payload)),
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
