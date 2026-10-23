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
  BinList,
  Authorization,
  CancelReceipt,
  CandidatePage,
  CandidateSetInfo,
  Capabilities,
  EntriesPage,
  EntriesQuery,
  FeaturesChangedEvent,
  FinishedEvent,
  Group,
  HistoryPage,
  IndexRepair,
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
  RuleCatalog,
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
import type { BatteryAlert, ResidentBackend } from './residentTypes';
import type {
  AlertSettings,
  InsightOverview,
  Leftovers,
  MaintenanceCatalog,
  MaintenanceDiagnosis,
  MaintenanceHistory,
  MaintenanceSearch,
  ProcessList,
  ProcessSort,
  UninstallList,
  UninstallPlanCreated,
  UpdateList,
  UpdatePlanCreated,
} from './suiteTypes';

export interface Backend extends DiagBackend, ResidentBackend {
  capabilities(): Promise<Capabilities>;
  /** Turn experimental features on or off (shared with the CLI); off also stops the resident utilities. */
  setExperimentalFeatures(enabled: boolean): Promise<Capabilities>;
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
  /** One page of a group; `ruleId` limits it to one rule's candidates (the catalog page's link). */
  listCandidates(setId: string, group: Group, pageSize: number, cursor: string | null, ruleId?: string | null): Promise<CandidatePage>;
  /** 清理 › 清理目录: every rule of this platform with where it looks, applicability and the stored sizes of `scanId`'s analysis. Read-only. */
  cleanCatalog(scanId: string | null): Promise<RuleCatalog>;
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
  /**
   * NEW-04: rebuild a damaged or incompatible scan index (the desktop's `doctor --repair-index`): the damaged files are
   * kept beside the store under a new name and an empty index is created; earlier scans must be run again. History,
   * quarantine and recovery are not touched. Only called after the person confirmed (the host refuses without it).
   */
  repairIndex(confirmed: true): Promise<IndexRepair>;
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
  /** WFA-65 / MTR-03: the current user's bin items (read-only). */
  trashList(): Promise<BinList>;
  /** Listed items of one bin frozen into a permanent-delete plan; only a terminal review approves it. */
  trashPurgePlan(itemIds: string[]): Promise<{ plan_id: string }>;
  startupList(): Promise<StartupList>;
  startupPlan(itemId: string, operation: 'disable' | 'enable'): Promise<{ plan_id: string }>;
  insightsList(): Promise<InsightsList>;
  statusSnapshot(): Promise<StatusSnapshot>;
  readiness(operation: ReadinessOp): Promise<Readiness>;
  /** Shows a scanned entry in the platform file manager (path resolved by the host). */
  revealEntry(scanId: string, entryId: string): Promise<boolean>;
  // 软件 › 卸载 / 更新, 优化, 状态 › 进程, 空间洞察 overview. Changes are stored plans only.
  uninstallList(): Promise<UninstallList>;
  /** UNI-11: one plan for one or several applications. */
  uninstallPlan(appInstanceIds: string[]): Promise<UninstallPlanCreated>;
  uninstallLeftovers(operationId: string): Promise<Leftovers>;
  uninstallLeftoversPlan(operationId: string, objectIds: string[]): Promise<{ plan_id: string }>;
  uninstallRestorePlan(operationId: string): Promise<{ plan_id: string }>;
  /** Recovery plan putting back the prior values of a maintenance run's setting changes (`optimize restore`). */
  maintenanceRestorePlan(operationId: string): Promise<{ plan_id: string }>;
  /** `check: true` runs the package managers' own query (only while checks are on). */
  updatesList(check: boolean): Promise<UpdateList>;
  updatesSetChecks(enabled: boolean): Promise<{ checks_enabled: boolean; last_checked_at: string | null }>;
  updatesPlan(appInstanceIds: string[]): Promise<UpdatePlanCreated>;
  /** Opens one fixed system page: Settings › Apps / the store's updates; `login_items` (System Settings › General › Login Items & Extensions) and `software_update` exist on macOS only. */
  openSystemPage(target: SystemPage): Promise<boolean>;
  maintenanceCatalog(): Promise<MaintenanceCatalog>;
  maintenanceSearch(query: string): Promise<MaintenanceSearch>;
  maintenancePlan(taskIds: string[]): Promise<{ plan_id: string }>;
  maintenanceHistory(limit: number): Promise<MaintenanceHistory>;
  maintenanceDiagnose(): Promise<MaintenanceDiagnosis>;
  processList(sort: ProcessSort, search: string | null, limit: number): Promise<ProcessList>;
  processPin(name: string, pinned: boolean): Promise<string[]>;
  processEndPlan(pid: number): Promise<{ plan_id: string }>;
  alertsSettings(): Promise<AlertSettings>;
  alertsSave(settings: AlertSettings): Promise<AlertSettings>;
  insightsOverview(olderThanDays: number): Promise<InsightOverview>;
  downloadsTrashPlan(itemIds: string[]): Promise<{ plan_id: string }>;
  // Events
  onProgress(cb: (e: ProgressEvent) => void): Promise<() => void>;
  onFinished(cb: (e: FinishedEvent) => void): Promise<() => void>;
  onClosing(cb: (e: { waiting_for: string[] }) => void): Promise<() => void>;
  /** D15: the experimental-features switch changed outside this window (the CLI); re-read the capabilities. */
  onFeaturesChanged(cb: (e: FeaturesChangedEvent) => void): Promise<() => void>;
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
    setExperimentalFeatures: (enabled) => call('set_experimental_features', { enabled }),
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
    listCandidates: (candidate_set_id, group, page_size, cursor, rule_id = null) => call('list_candidates', { candidate_set_id, group, page_size, cursor, rule_id }),
    cleanCatalog: (scan_id) => call('clean_catalog', { scan_id }),
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
    repairIndex: (confirm) => call('repair_index', { confirm }),
    purgeRoots: () => call('purge_roots'),
    purgeRootsAdd: () => call('purge_roots_add'),
    purgeRootsRemove: (root_id) => call('purge_roots_remove', { root_id }),
    purgeScan: () => call('purge_scan'),
    installerLocations: () => call('installer_locations'),
    installerScan: () => call('installer_scan'),
    createCatalogPlan: (catalog, candidate_set_id, candidate_ids, mode) => call('create_catalog_plan', { catalog, candidate_set_id, candidate_ids, mode }),
    trashInspect: (scan_id, entry_id) => call('trash_inspect', { scan_id, entry_id }),
    trashPlan: (scan_id, entry_ids) => call('trash_plan', { scan_id, entry_ids }),
    trashList: () => call('trash_list'),
    trashPurgePlan: (item_ids) => call('trash_purge_plan', { item_ids }),
    startupList: () => call('startup_list'),
    startupPlan: (item_id, operation) => call('startup_plan', { item_id, operation }),
    insightsList: () => call('insights_list'),
    statusSnapshot: () => call('status_snapshot'),
    readiness: (operation) => call('readiness', { operation }),
    revealEntry: (scan_id, entry_id) => call('reveal_entry', { scan_id, entry_id }),
    uninstallList: () => call('uninstall_list'),
    uninstallPlan: (app_instance_ids) => call('uninstall_plan', { app_instance_ids }),
    uninstallLeftovers: (operation_id) => call('uninstall_leftovers', { operation_id }),
    uninstallLeftoversPlan: (operation_id, object_ids) => call('uninstall_leftovers_plan', { operation_id, object_ids }),
    uninstallRestorePlan: (operation_id) => call('uninstall_restore_plan', { operation_id }),
    maintenanceRestorePlan: (operation_id) => call('maintenance_restore_plan', { operation_id }),
    updatesList: (check) => call('updates_list', { check }),
    updatesSetChecks: (enabled) => call('updates_set_checks', { enabled }),
    updatesPlan: (app_instance_ids) => call('updates_plan', { app_instance_ids }),
    openSystemPage: (target) => call('open_system_page', { target }),
    maintenanceCatalog: () => call('maintenance_catalog'),
    maintenanceSearch: (query) => call('maintenance_search', { query }),
    maintenancePlan: (task_ids) => call('maintenance_plan', { task_ids }),
    maintenanceHistory: (limit) => call('maintenance_history', { limit }),
    maintenanceDiagnose: () => call('maintenance_diagnose'),
    processList: (sort, search, limit) => call('process_list', { sort, search, limit }),
    processPin: (name, pinned) => call('process_pin', { name, pinned }),
    processEndPlan: (pid) => call('process_end_plan', { pid }),
    alertsSettings: () => call('alerts_settings'),
    alertsSave: (settings) => call('alerts_save', { settings }),
    insightsOverview: (older_than_days) => call('insights_overview', { older_than_days }),
    downloadsTrashPlan: (item_ids) => call('downloads_trash_plan', { item_ids }),
    onProgress: (cb) => listen<ProgressEvent>('scan-progress', (e) => cb(e.payload)),
    onFinished: (cb) => listen<FinishedEvent>('scan-finished', (e) => cb(e.payload)),
    onClosing: (cb) => listen<{ waiting_for: string[] }>('app-closing', (e) => cb(e.payload)),
    onFeaturesChanged: (cb) => listen<FeaturesChangedEvent>('features-changed', (e) => cb(e.payload)),
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
    residentStatus: () => call('resident_status'),
    residentSave: (settings) => call('resident_save', { settings }),
    residentKeepAwake: (on, minutes) => call('resident_keep_awake', { on, minutes }),
    residentCleanScreen: (timeout_secs) => call('resident_clean_screen', { timeout_secs }),
    residentPrivacy: () => call('resident_privacy'),
    residentBatteries: () => call('resident_batteries'),
    onResidentNavigate: (cb) => listen<{ page: string }>('resident-navigate', (e) => cb(e.payload.page)),
    onResidentChanged: (cb) => listen('resident-changed', () => cb()),
    onResidentAlert: (cb) => listen<BatteryAlert[]>('resident-alert', (e) => cb(e.payload)),
  };
}

export type SystemPage = 'apps_features' | 'store_updates' | 'login_items' | 'software_update';

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
