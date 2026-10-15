// Typed wrappers over the host's explicit commands. Nothing here can reach a path, shell or
// filesystem API: the WebView only knows these ten commands and two events.

import type {
  CancelReceipt,
  Capabilities,
  EntriesPage,
  EntriesQuery,
  FinishedEvent,
  IpcError,
  ProgressEvent,
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
  onProgress(cb: (e: ProgressEvent) => void): Promise<() => void>;
  onFinished(cb: (e: FinishedEvent) => void): Promise<() => void>;
  onClosing(cb: (e: { scan_id: string }) => void): Promise<() => void>;
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
    onProgress: (cb) => listen<ProgressEvent>('scan-progress', (e) => cb(e.payload)),
    onFinished: (cb) => listen<FinishedEvent>('scan-finished', (e) => cb(e.payload)),
    onClosing: (cb) => listen<{ scan_id: string }>('app-closing', (e) => cb(e.payload)),
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
