// Display preferences of the status tool (STS-07): which panels the status page shows and in
// what order, how often it refreshes, and how the process list is sorted. They are view
// preferences of this window, so they live where the theme lives (localStorage, per viewer) and
// never in the shared store: nothing outside the desktop window reads them. Every read is
// validated; a missing, corrupt or older record falls back to the defaults field by field.

import type { ProcessSort } from './suiteTypes';

export type StatusPanel = 'health' | 'metrics' | 'network';

export const PANELS: readonly StatusPanel[] = ['health', 'metrics', 'network'];

export const PANEL_LABEL: Record<StatusPanel, string> = { health: '健康分', metrics: '指标', network: '网络' };

/** Narrow panels share a row with the wide panel next to them. */
const NARROW: ReadonlySet<StatusPanel> = new Set(['health']);

export const REFRESH_SECONDS: readonly number[] = [2, 3, 5, 10, 30];

export const PROCESS_SORTS: readonly ProcessSort[] = ['cpu', 'memory', 'disk', 'name'];

export interface StatusPrefs {
  /** Every panel exactly once, in display order (hidden ones keep their place). */
  order: StatusPanel[];
  hidden: StatusPanel[];
  refreshSeconds: number;
  processSort: ProcessSort;
}

export const DEFAULT_PREFS: StatusPrefs = { order: [...PANELS], hidden: [], refreshSeconds: 3, processSort: 'cpu' };

export const PREFS_KEY = 'cm.status.prefs';

type PrefsStorage = Pick<Storage, 'getItem' | 'setItem'>;

function browserStorage(): PrefsStorage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

const isPanel = (v: unknown): v is StatusPanel => typeof v === 'string' && (PANELS as readonly string[]).includes(v);

/** Keep what is valid in a stored record; unknown panels are dropped and new ones appended. */
export function sanitize(raw: unknown): StatusPrefs {
  const r = (raw && typeof raw === 'object' ? raw : {}) as Record<string, unknown>;
  const order: StatusPanel[] = [];
  for (const p of Array.isArray(r.order) ? r.order : []) if (isPanel(p) && !order.includes(p)) order.push(p);
  for (const p of PANELS) if (!order.includes(p)) order.push(p);
  const hidden = PANELS.filter((p) => Array.isArray(r.hidden) && r.hidden.includes(p));
  const refreshSeconds = REFRESH_SECONDS.includes(r.refreshSeconds as number) ? (r.refreshSeconds as number) : DEFAULT_PREFS.refreshSeconds;
  const processSort = PROCESS_SORTS.includes(r.processSort as ProcessSort) ? (r.processSort as ProcessSort) : DEFAULT_PREFS.processSort;
  return { order, hidden, refreshSeconds, processSort };
}

export function loadPrefs(storage: PrefsStorage | null = browserStorage()): StatusPrefs {
  try {
    const text = storage?.getItem(PREFS_KEY);
    return sanitize(text ? JSON.parse(text) : null);
  } catch {
    return sanitize(null);
  }
}

/** False when the preferences could not be kept (storage blocked); the page still uses them. */
export function savePrefs(p: StatusPrefs, storage: PrefsStorage | null = browserStorage()): boolean {
  try {
    if (!storage) return false;
    storage.setItem(PREFS_KEY, JSON.stringify({ v: 1, ...sanitize(p) }));
    return true;
  } catch {
    return false;
  }
}

/** Read, change and write back, so two pages never overwrite each other's fields. */
export function updatePrefs(change: (p: StatusPrefs) => StatusPrefs, storage: PrefsStorage | null = browserStorage()): StatusPrefs {
  const next = sanitize(change(loadPrefs(storage)));
  savePrefs(next, storage);
  return next;
}

export function movePanel(p: StatusPrefs, panel: StatusPanel, delta: -1 | 1): StatusPrefs {
  const order = [...p.order];
  const i = order.indexOf(panel);
  const j = i + delta;
  if (i < 0 || j < 0 || j >= order.length) return p;
  [order[i], order[j]] = [order[j], order[i]];
  return { ...p, order };
}

export function setShown(p: StatusPrefs, panel: StatusPanel, shown: boolean): StatusPrefs {
  const hidden = p.hidden.filter((x) => x !== panel);
  if (!shown) hidden.push(panel);
  return { ...p, hidden: PANELS.filter((x) => hidden.includes(x)) };
}

export type RowShape = 'full' | 'narrow-wide' | 'wide-narrow';

/** Rows of visible panels in order: a narrow panel pairs with the wide panel beside it. */
export function layoutRows(p: StatusPrefs): { shape: RowShape; panels: StatusPanel[] }[] {
  const shown = p.order.filter((x) => !p.hidden.includes(x));
  const rows: { shape: RowShape; panels: StatusPanel[] }[] = [];
  for (let i = 0; i < shown.length; i++) {
    const a = shown[i];
    const b = shown[i + 1];
    if (b !== undefined && NARROW.has(a) !== NARROW.has(b)) {
      rows.push({ shape: NARROW.has(a) ? 'narrow-wide' : 'wide-narrow', panels: [a, b] });
      i++;
    } else rows.push({ shape: 'full', panels: [a] });
  }
  return rows;
}
