// NEW-04 (WFA-16 on the desktop): a damaged or incompatible scan index. The host keeps history, quarantine and
// recovery usable (they live in the audit store) and reports the index problem at startup as `store_problem`
// (code `index_unavailable`, reason `index_corrupt` / `index_incompatible`), and again from every call that needs the
// index. Pages that show scans must say "the index needs rebuilding", never "no scans yet". Pure so the cases are
// tested without a DOM.

import type { Capabilities, IpcError } from './types';
import { t } from '../i18n';

export type IndexProblemReason = 'index_corrupt' | 'index_incompatible';

export interface IndexProblem {
  reason: IndexProblemReason;
  /** The host's own message (技术详情 only). */
  message: string;
}

const REASONS: readonly string[] = ['index_corrupt', 'index_incompatible'];

/** The index problem an IPC error reports, if it is one. */
export function indexProblemOf(e: Pick<IpcError, 'code' | 'reason' | 'message'> | null | undefined): IndexProblem | null {
  if (!e) return null;
  if (REASONS.includes(e.reason)) return { reason: e.reason as IndexProblemReason, message: e.message };
  // The protocol code alone (an older host): treat it as damage, which is what the repair handles.
  if (e.code === 'index_unavailable') return { reason: 'index_corrupt', message: e.message };
  return null;
}

/**
 * The index problem this window knows of: a failed status read says so directly; otherwise the startup report, unless
 * the index was rebuilt in this window since (the startup report is a snapshot taken when the host started).
 */
export function currentIndexProblem(o: { caps: Pick<Capabilities, 'store_problem'> | null; statusError?: IpcError | null; repairedAt?: number | null }): IndexProblem | null {
  const fromStatus = indexProblemOf(o.statusError);
  if (fromStatus) return fromStatus;
  if (o.repairedAt) return null;
  return indexProblemOf(o.caps?.store_problem);
}

/** Words of the index problem and its repair (one table for the language switch). */
export const INDEX_TEXT = {
  title: (r: IndexProblemReason) => (r === 'index_incompatible' ? t('indexRepair.titleIncompatible') : t('indexRepair.titleCorrupt')),
  get sentence() { return t('indexRepair.sentence'); },
  get repair() { return t('indexRepair.repair'); },
  get dialogTitle() { return t('indexRepair.dialogTitle'); },
  get dialogLede() { return t('indexRepair.dialogLede'); },
  get kept() { return t('indexRepair.kept'); },
  get rescan() { return t('indexRepair.rescan'); },
  get untouched() { return t('indexRepair.untouched'); },
  get busyNote() { return t('indexRepair.busyNote'); },
  get confirm() { return t('indexRepair.confirm'); },
  get repairing() { return t('indexRepair.repairing'); },
  repaired: (files: string) => t('indexRepair.repaired', { files }),
  get notNeeded() { return t('indexRepair.notNeeded'); },
  get cli() { return t('indexRepair.cli'); },
} as const;

/** What the person is told after the host answered the repair. */
export function repairOutcome(r: { status: string; kept_files: string[] }): { done: boolean; text: string } {
  if (r.status === 'repaired') {
    const names = r.kept_files.map((f) => f.split(/[\\/]/).pop() ?? f).join(t('common.listSep'));
    return { done: true, text: INDEX_TEXT.repaired(names || 'index.db.damaged-…') };
  }
  return { done: false, text: INDEX_TEXT.notNeeded };
}
