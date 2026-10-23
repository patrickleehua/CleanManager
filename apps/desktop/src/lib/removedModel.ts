// "Removed since scan": what the disk page (table, chart, search, detail pane) says about entries that operations
// applied after the scan removed. The host derives the facts from the audit store (`removed`, `removed_count`,
// `removed_bytes`, `removed_unknown_count` on each entry; `removed_since_scan` on the status); the scan stays the
// snapshot it was, and nothing here asks the disk. Pure functions, so the hide toggle, the badges and the folder
// sizes are tested without a DOM.

import { sizeLabel } from './display';
import type { Entry, RemovedFact, RemovedTotal } from './types';
import { t } from '../i18n';

/** Words of the removal marks (one table for the language switch). */
export const REMOVED_TEXT = {
  get trash() { return t('removed.badge.trash'); },
  get quarantine() { return t('removed.badge.quarantine'); },
  get quarantinePurged() { return t('removed.badge.quarantinePurged'); },
  get permanent() { return t('removed.badge.permanent'); },
  get inherited() { return t('removed.inherited'); },
  get notSelectable() { return t('removed.notSelectable'); },
  get insideNotSelectable() { return t('removed.insideNotSelectable'); },
  get viewOperation() { return t('removed.viewOperation'); },
  get openRecovery() { return t('removed.openRecovery'); },
  when: (when: string) => t('removed.when', { when }),
  folderSize: (scanned: string, removed: string) => t('removed.folderSize', { scanned, removed }),
  removedShort: (size: string) => t('removed.removedShort', { size }),
  hideToggle: (n: number) => t('removed.hideToggle', { n }),
  sinceLine: (n: number, size: string) => t('removed.sinceLine', { n, size }),
  get tileSuffix() { return t('removed.tileSuffix'); },
  hiddenRows: (n: number) => t('removed.hiddenRows', { n }),
  handled: (n: number, size: string) => t('removed.handled', { n, size }),
  get overviewLabel() { return t('removed.overviewLabel'); },
  overviewValue: (n: number, size: string) => t('removed.overviewValue', { n, size }),
} as const;

/** Tone of a mark: a bin or quarantine move can be put back; a permanent delete cannot. */
export type RemovedTone = 'recoverable' | 'gone';

export interface RemovedBadge {
  text: string;
  tone: RemovedTone;
  /** "随上级文件夹一起移走" when a folder above carries the operation. */
  note: string | null;
}

/** The badge of a removed entry: how it left and whether it can be put back. */
export function removedBadge(r: RemovedFact): RemovedBadge {
  const note = r.inherited ? REMOVED_TEXT.inherited : null;
  if (r.mode === 'trash') return { text: REMOVED_TEXT.trash, tone: 'recoverable', note };
  if (r.mode === 'quarantine') return r.recoverable ? { text: REMOVED_TEXT.quarantine, tone: 'recoverable', note } : { text: REMOVED_TEXT.quarantinePurged, tone: 'gone', note };
  return { text: REMOVED_TEXT.permanent, tone: 'gone', note };
}

/** Why a removed entry — or a folder with removed objects inside, whose snapshot no longer describes it — is not
 *  offered for a selection (the host refuses both: `already_removed`); `null` otherwise. */
export function removedBlock(e: Pick<Entry, 'removed' | 'removed_count'>): string | null {
  if (e.removed) return REMOVED_TEXT.notSelectable;
  return e.removed_count ? REMOVED_TEXT.insideNotSelectable : null;
}

/** The rows the table shows: all of them, or with "hide removed" on, those still there. */
export function visibleRows<T extends Pick<Entry, 'removed'>>(rows: T[], hideRemoved: boolean): { rows: T[]; hidden: number } {
  if (!hideRemoved) return { rows, hidden: 0 };
  const kept = rows.filter((r) => !r.removed);
  return { rows: kept, hidden: rows.length - kept.length };
}

/** What was removed beneath a folder, as a size: the known part ("至少 X" when part is unknown), "未知" when none is
 *  known; `null` when nothing beneath was removed. Unknown is never shown as 0. */
export function removedBeneath(e: Pick<Entry, 'removed_count' | 'removed_bytes' | 'removed_unknown_count'>): string | null {
  if (!e.removed_count) return null;
  return sizeLabel(e.removed_bytes ?? null, { unknownCount: e.removed_unknown_count ?? 0 });
}

/** A folder's size with what left it since the scan: "扫描时 X · 已移走 Y"; the plain size otherwise. */
export function folderSizeText(e: Pick<Entry, 'logical_bytes' | 'unknown_descendants' | 'removed_count' | 'removed_bytes' | 'removed_unknown_count'>): string {
  const scanned = sizeLabel(e.logical_bytes, { unknownCount: e.unknown_descendants ?? 0 });
  const removed = removedBeneath(e);
  return removed === null ? scanned : REMOVED_TEXT.folderSize(scanned, removed);
}

/** The size of a scan's removed total (the same rules). */
export function totalSize(r: Pick<RemovedTotal, 'known_bytes' | 'unknown_count'>): string {
  return sizeLabel(r.known_bytes, { unknownCount: r.unknown_count });
}

/** The quiet line under the scan header: how much left since the scan; `null` when nothing did. */
export function sinceScanLine(r: RemovedTotal | null | undefined): string | null {
  if (!r || r.count <= 0) return null;
  return REMOVED_TEXT.sinceLine(r.count, totalSize(r));
}

/** "Hide removed items" is a per-viewer convenience (`localStorage`); default: show them marked. */
export const HIDE_REMOVED_KEY = 'cm.disk.hideRemoved';

export function loadHideRemoved(storage: Pick<Storage, 'getItem'> | null = safeStorage()): boolean {
  try {
    return storage?.getItem(HIDE_REMOVED_KEY) === '1';
  } catch {
    return false;
  }
}

export function saveHideRemoved(on: boolean, storage: Pick<Storage, 'setItem' | 'removeItem'> | null = safeStorage()): void {
  try {
    if (on) storage?.setItem(HIDE_REMOVED_KEY, '1');
    else storage?.removeItem(HIDE_REMOVED_KEY);
  } catch {
    /* a per-viewer convenience only */
  }
}

function safeStorage(): Storage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}
