// Application state shared by the shell and pages. Pages keep their own view state (filters,
// path, scroll) in long-lived view objects; this store carries what several pages show.

import type { DiagTarget } from './diagTypes';
import type { Capabilities, Counts, IpcError, RecentScan, ScanState, ScanStatus } from './types';

export type Page =
  | 'scope'
  | 'overview'
  | 'disk'
  | 'insights'
  | 'cleanup'
  | 'rulecatalog'
  | 'purge'
  | 'installer'
  | 'recyclebin'
  | 'plan'
  | 'operation'
  | 'apps'
  | 'uninstall'
  | 'updates'
  | 'startup'
  | 'maintenance'
  | 'diagnosis'
  | 'status'
  | 'processes'
  | 'resident'
  | 'activity'
  | 'agent'
  | 'settings'
  | 'diag';
export type ActivityTab = 'operations' | 'plans' | 'scans' | 'quarantine';

/** The scan hosted by this desktop process (global task area). */
export interface Task {
  scanId: string;
  rootDisplay: string;
  state: ScanState;
  entriesSeen: number;
  skippedRanges: number;
  cancelRequested: boolean;
  startedAt: string;
  error: IpcError | null;
  /** The user dismissed the finished-task chip. */
  dismissed: boolean;
}

/** The plan application hosted by this desktop process (global task area). */
export interface ApplyTask {
  planId: string;
  operationId: string;
  total: number;
  counts: Counts;
  cancelRequested: boolean;
  running: boolean;
  status: string | null;
  error: IpcError | null;
  dismissed: boolean;
}

export interface AppState {
  page: Page;
  caps: Capabilities | null;
  recent: RecentScan[];
  /** The scan the overview, disk, cleanup and apps pages are bound to. */
  scanId: string | null;
  status: ScanStatus | null;
  statusError: IpcError | null;
  task: Task | null;
  apply: ApplyTask | null;
  /** The plan shown on the review page and the operation shown on the result page. */
  planId: string | null;
  operationId: string | null;
  /** Where the plan / operation page was opened from (its back link). */
  returnTo: Page | null;
  activityTab: ActivityTab;
  closing: boolean;
  preview: boolean;
  /** Optional AI diagnosis: the object the workbench is about and the record shown. */
  diagTarget: DiagTarget | null;
  diagId: string | null;
  /** Entry the disk page should locate when it is shown next (diagnosis "locate"). */
  revealEntry: string | null;
  /** The finished uninstall operation whose leftovers the uninstall page reviews (null: the app list). */
  leftoversOp: string | null;
  /** Process the process page should select when it is shown next (an alert's link). */
  processFocus: number | null;
  /** Rule the cleanup candidates are limited to (the catalog page's "show its candidates"); null = every rule. */
  cleanupRule?: string | null;
  /** NEW-04: when the scan index was rebuilt from this window (the startup store problem no longer applies). */
  indexRepairedAt?: number | null;
  /** Counts plan runs that finished in this window: pages holding candidates from a stored scan re-read them. */
  opsRevision?: number;
}

type Listener = (s: AppState, prev: AppState) => void;

export class Store {
  private listeners = new Set<Listener>();
  constructor(public state: AppState) {}

  set(patch: Partial<AppState>): void {
    const prev = this.state;
    this.state = { ...prev, ...patch };
    this.listeners.forEach((l) => l(this.state, prev));
  }

  subscribe(l: Listener): () => void {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  }
}

export const EMPTY_COUNTS: Counts = { succeeded: 0, skipped: 0, failed: 0, not_started: 0, unknown: 0, in_flight: 0 };
