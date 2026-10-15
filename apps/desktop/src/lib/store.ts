// Application state shared by the shell and pages. Pages keep their own view state (filters,
// path, scroll) in long-lived view objects; this store carries what several pages show.

import type { Capabilities, IpcError, RecentScan, ScanState, ScanStatus } from './types';

export type Page = 'scope' | 'overview' | 'disk';

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

export interface AppState {
  page: Page;
  caps: Capabilities | null;
  recent: RecentScan[];
  /** The scan the overview and disk pages are bound to. */
  scanId: string | null;
  status: ScanStatus | null;
  statusError: IpcError | null;
  task: Task | null;
  closing: boolean;
  preview: boolean;
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
