// P06 execution result: the outcome banner (one state word, one sentence) and the per-item issues the
// result page lists through the shared issue lines. Pure so every outcome is tested without a DOM.


import type { BannerKind } from './display';
import { words } from './platform';
import type { Counts, Issue, Mode, OperationItem, PlanKind } from './types';
import { MODE_LABEL, PLAN_KIND_LABEL } from './words';
import { t } from '../i18n';

/** Words of the result page that are not data (one table for the language switch). */
export const OPERATION_TEXT = {
  get running() { return t('operationModel.operationText.running'); },
  runningSentence: (done: number, total: number) => t('operationModel.operationText.runningSentence', { done, total }),
  get stopping() { return t('operationModel.operationText.stopping'); },
  get stoppingSentence() { return t('operationModel.operationText.stoppingSentence'); },
  get reconcile() { return t('operationModel.operationText.reconcile'); },
  get reconcileSentence() { return t('operationModel.operationText.reconcileSentence'); },
  completed: (action: string, n: number) => t('operationModel.operationText.completed', { action, n }),
  partial: (ok: number, notOk: number) => t('operationModel.operationText.partial', { ok, notOk }),
  get partialSentence() { return t('operationModel.operationText.partialSentence'); },
  get partialUninstall() { return t('operationModel.operationText.partialUninstall'); },
  get partialMaintenance() { return t('operationModel.operationText.partialMaintenance'); },
  get partialBatch() { return t('operationModel.operationText.partialBatch'); },
  get partialTitle() { return t('operationModel.operationText.partialTitle'); },
  get cancelled() { return t('operationModel.operationText.cancelled'); },
  get cancelledSentence() { return t('operationModel.operationText.cancelledSentence'); },
  get failed() { return t('operationModel.operationText.failed'); },
  get failedSentence() { return t('operationModel.operationText.failedSentence'); },
  get doneQuarantine() { return t('operationModel.operationText.doneQuarantine'); },
  doneTrash: (bin: string, restore: string) => t('operationModel.operationText.doneTrash', { bin, bin2: bin, restore }),
  get doneDelete() { return t('operationModel.operationText.doneDelete'); },
  get doneOther() { return t('operationModel.operationText.doneOther'); },
  get issuesTitle() { return t('operationModel.operationText.issuesTitle'); },
  get issuesLabel() { return t('operationModel.operationText.issuesLabel'); },
} as const;

export interface OperationFacts {
  state: string;
  plan_kind: PlanKind | 'unknown';
  mode: Mode | null;
  counts: Counts;
  total: number;
  needs_reconcile: boolean;
  /** Followed live in this window (running or stopping). */
  live: boolean;
  /** UNI-11: applications in a batch uninstall. */
  apps?: number;
}

/** The action a result is about, in the plan's own words. */
export function operationAction(o: Pick<OperationFacts, 'plan_kind' | 'mode'>): string {
  if ((o.plan_kind === 'cleanup' || o.plan_kind === 'manual_trash') && o.mode) return MODE_LABEL[o.mode];
  if (o.plan_kind === 'unknown') return t('operationModel.unknownPlan');
  return PLAN_KIND_LABEL[o.plan_kind] ?? o.plan_kind;
}

export function operationBanner(o: OperationFacts): { kind: BannerKind; title: string; sentence: string } {
  const T = OPERATION_TEXT;
  const c = o.counts;
  const notOk = c.skipped + c.failed + c.unknown + c.not_started;
  if (o.state === 'running' || o.state === 'created' || o.state === 'validating')
    return { kind: 'running', title: T.running, sentence: T.runningSentence(c.succeeded + c.skipped + c.failed + c.unknown + c.not_started, o.total) };
  if (o.state === 'stopping') return { kind: 'running', title: T.stopping, sentence: T.stoppingSentence };
  if (o.needs_reconcile || o.state === 'reconciliation_required') return { kind: 'partial', title: T.reconcile, sentence: T.reconcileSentence };
  if (o.state === 'cancelled') return { kind: 'cancelled', title: T.cancelled, sentence: T.cancelledSentence };
  if (o.state === 'failed' || (o.total > 0 && c.succeeded === 0 && notOk > 0 && o.state !== 'partial'))
    return { kind: 'failed', title: T.failed, sentence: T.failedSentence };
  if (o.state === 'partial') {
    let sentence: string = T.partialSentence;
    if (o.plan_kind === 'uninstall' && (o.apps ?? 0) > 1) sentence = T.partialBatch;
    else if (o.plan_kind === 'uninstall' && c.failed + c.skipped === 0) sentence = T.partialUninstall;
    else if (o.plan_kind === 'maintenance' && c.failed + c.skipped === 0) sentence = T.partialMaintenance;
    return { kind: 'partial', title: notOk > 0 ? T.partial(c.succeeded, notOk) : T.partialTitle, sentence };
  }
  // completed
  const w = words();
  const files = o.plan_kind === 'cleanup' || o.plan_kind === 'manual_trash';
  const sentence = files && o.mode === 'quarantine' ? T.doneQuarantine : files && o.mode === 'trash' ? T.doneTrash(w.bin, w.restoreFromBin) : (files && o.mode === 'delete') || o.plan_kind === 'purge' || o.plan_kind === 'manual_purge' ? T.doneDelete : T.doneOther;
  return { kind: 'success', title: T.completed(operationAction(o), c.succeeded), sentence };
}

/**
 * NEW-02: `start_apply` answers with the operation id before the worker thread has registered the operation, so the
 * first reads of a run this window just started can say `not_found`. That is "starting", not an error: the page shows
 * the loading line and reads again. Any other failure, or `not_found` for a run that is not this window's, is real.
 */
export function registrationPending(error: { reason?: string | null } | null, operationId: string, liveRun: { operationId: string; error?: unknown } | null): boolean {
  // A run that could not start at all (its finish event carries an error) is never registered: that is the error.
  return !!error && error.reason === 'not_found' && !!liveRun && liveRun.operationId === operationId && !liveRun.error;
}

/** How many times a read of a run that is still being registered is tried again (every `REGISTRATION_RETRY_MS`). */
export const REGISTRATION_RETRIES = 40;
export const REGISTRATION_RETRY_MS = 250;

/** The items that did not succeed, as issues for the shared issue lines (grouped by meaning, path as place). */
export function resultIssues(items: OperationItem[]): Issue[] {
  return items
    .filter((i) => i.status !== 'succeeded' && i.status !== 'pending' && i.status !== 'in_flight')
    .map((i) => ({ code: i.code ?? i.status, scope_display: i.original_path_display ?? i.path_display, message: i.message ?? '' }));
}
