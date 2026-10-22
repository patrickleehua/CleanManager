// P06 execution result: the outcome banner (one state word, one sentence) and the per-item issues the
// result page lists through the shared issue lines. Pure so every outcome is tested without a DOM.

import { groupDigits } from './bytes';
import type { BannerKind } from './display';
import { words } from './platform';
import type { Counts, Issue, Mode, OperationItem, PlanKind } from './types';
import { MODE_LABEL, PLAN_KIND_LABEL } from './words';

/** Words of the result page that are not data (one table for the language switch). */
export const OPERATION_TEXT = {
  running: '正在执行',
  runningSentence: (done: number, total: number) => `已处理 ${groupDigits(done)} / ${groupDigits(total)} 项；每一项执行前都会重新检查对象、占用和保护。`,
  stopping: '正在停止',
  stoppingSentence: '正在进行的动作结束后不再开始新的动作；已处理的项目不会自动撤销。',
  reconcile: '需要核对：有动作的结果无法确认',
  reconcileSentence: '它们可能已经生效。先进行只读核对；核对不会重新执行任何动作，也不会自动重试。',
  completed: (action: string, n: number) => `全部完成：${action} ${groupDigits(n)} 项`,
  partial: (ok: number, notOk: number) => `部分完成：${groupDigits(ok)} 项成功，${groupDigits(notOk)} 项没有完成`,
  partialSentence: '没有完成的项目和原因见下方。需要处理它们时，请重新分析并生成新计划；已成功的项目不会重做。',
  partialUninstall: '卸载程序已运行，但重新检查发现安装目录或卸载登记仍在，所以记为“部分完成”。剩下的内容在“审阅残留”中单独处理。',
  partialMaintenance: '有任务需要你跟进（见下表），所以记为“部分完成”。',
  partialBatch: '有应用没有卸载或卸载后仍有剩余，所以整体记为“部分完成”。每个应用的结果见“逐个应用”；一个应用失败没有影响其他应用。',
  partialTitle: '部分完成',
  cancelled: '已停止',
  cancelledSentence: '你停止了这次执行。停止前完成的项目保留其结果；未开始的项目没有被触碰。',
  failed: '执行失败',
  failedSentence: '没有项目成功完成，原因见下方。',
  doneQuarantine: '文件仍在同一个卷上，空间没有释放；需要时可以在隔离区恢复。',
  doneTrash: (bin: string, restore: string) => `${bin}仍占用这些空间，清空${bin}后才释放；${restore}。`,
  doneDelete: '已永久删除，无法撤销；实际释放的空间以卷的读数为准。',
  doneOther: '逐项结果见下方。',
  issuesTitle: '没有完成的项目',
  issuesLabel: '没有完成的项目，按原因合并',
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
  if (o.plan_kind === 'unknown') return '未知计划';
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

/** The items that did not succeed, as issues for the shared issue lines (grouped by meaning, path as place). */
export function resultIssues(items: OperationItem[]): Issue[] {
  return items
    .filter((i) => i.status !== 'succeeded' && i.status !== 'pending' && i.status !== 'in_flight')
    .map((i) => ({ code: i.code ?? i.status, scope_display: i.original_path_display ?? i.path_display, message: i.message ?? '' }));
}
