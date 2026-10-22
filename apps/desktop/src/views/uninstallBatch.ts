// 软件 › 卸载 — several applications in one plan (UNI-11). Pure helpers shared by the uninstall,
// plan-review and result pages (tested without a DOM): what can be selected, how a batch reads,
// review rows grouped by application, each application's own result and leftover section.
// A batch never relaxes anything: protected apps cannot be selected (and the core refuses them
// anyway), and an application whose uninstaller failed or was skipped keeps all its leftovers.

import type { AppOutcome, AppOutcomeStatus, Leftovers, UninstallApp } from '../lib/suiteTypes';
import type { KindDetail, OperationItem, PlanItem } from '../lib/types';
import { explainCode } from '../lib/words';
import { resultText } from '../lib/codes';
import { words } from '../lib/platform';
import type { Tone } from './parts';

/** Most applications in one uninstall plan (the core's limit). */
export const MAX_BATCH = 20;

/** Only an app with a verified uninstall route can be put into a plan (UNI-01/10). */
export function batchSelectable(a: UninstallApp): boolean {
  return a.state === 'available';
}

/** The selected apps that are still listed and selectable, in list order (a reload can drop or lock some). */
export function batchSelection(items: UninstallApp[], selected: ReadonlySet<string>): UninstallApp[] {
  return items.filter((a) => selected.has(a.app_instance_id) && batchSelectable(a));
}

/** Words of the plan button for `n` selected applications. */
export function batchActionText(n: number): string {
  return n > 1 ? `生成卸载计划并审阅（${n} 个应用，一份计划）` : '生成卸载计划并审阅';
}

/** One application's own result (UNI-11), as a pill. */
export const APP_OUTCOME: Record<AppOutcomeStatus, { label: string; tone: Tone }> = {
  completed: { label: '已卸载', tone: 'good' },
  partial: { label: '已卸载，有残留', tone: 'warn' },
  failed: { label: '卸载失败', tone: 'bad' },
  skipped: { label: '没有卸载', tone: 'warn' },
  not_started: { label: '未开始', tone: 'muted' },
  unknown: { label: '结果未确认', tone: 'warn' },
  pending: { label: '等待执行', tone: 'busy' },
};

/** Why an application ended as it did, in words (a code without words reads as such; the raw message never). */
export function outcomeReason(o: Pick<AppOutcome, 'status' | 'code' | 'message'>): string {
  if (o.status === 'completed') return '卸载程序已运行，重新检查后安装目录与卸载登记都已不在。';
  return o.code ? resultText(o.code) : explainCode(o.code, o.message);
}

/** Review rows of one application of an uninstall plan. */
export interface AppRows {
  /** `null`: rows that belong to no application (not expected in a stage-1 plan). */
  app_instance_id: string | null;
  name: string;
  items: PlanItem[];
}

/** Plan rows grouped by the application they belong to, in plan order (UNI-11: review groups by application). */
export function rowsByApp(items: PlanItem[], apps: NonNullable<KindDetail['apps']>): AppRows[] {
  const groups: AppRows[] = apps.map((a) => ({ app_instance_id: a.app_instance_id, name: a.name, items: [] }));
  const rest: PlanItem[] = [];
  for (const i of items) {
    const bound = typeof i.detail?.app_instance_id === 'string' ? i.detail.app_instance_id : apps.find((a) => a.action_ids.includes(i.action_id))?.app_instance_id;
    const g = groups.find((x) => x.app_instance_id === bound);
    if (g) g.items.push(i);
    else rest.push(i);
  }
  return rest.length ? [...groups, { app_instance_id: null, name: '其他动作', items: rest }] : groups;
}

/** One application's section of a leftover review. */
export interface LeftoverSection {
  app_instance_id: string;
  name: string;
  reviewable: boolean;
  status: AppOutcomeStatus;
  reason: string;
  objects: number;
  object_ids: string[];
}

/** Leftover review sections, one per application in plan order (a single uninstall has one). */
export function leftoverSections(l: Leftovers): LeftoverSection[] {
  return l.apps.map((a) => {
    const ids = a.groups.flatMap((g) => g.items.map((i) => i.object_id));
    return {
      app_instance_id: a.app.app_instance_id,
      name: a.app.name,
      reviewable: a.leftovers_reviewable,
      status: a.status,
      reason: a.leftovers_reviewable ? outcomeReason(a) : `${outcomeReason(a)} 这个应用仍然安装着，它的${words().leftoverKinds}全部保留，不进入残留计划。`,
      objects: ids.length,
      object_ids: ids,
    };
  });
}

/** The title of a leftover review: the application, or how many applications of a batch. */
export function leftoversTitle(l: Leftovers | null): string {
  if (!l) return '审阅残留';
  if (l.app) return `审阅残留：${l.app.name}`;
  const n = l.apps.length;
  const open = l.apps.filter((a) => a.leftovers_reviewable).length;
  return open === n ? `审阅残留：${n} 个应用` : `审阅残留：${n} 个应用中的 ${open} 个`;
}

/**
 * One application's result from its rows (the core's rule, for previews without a host): its uninstaller's status
 * decides; the first step that did not succeed explains it.
 */
export function outcomeFromItems(app: { app_instance_id: string; name: string; action_ids: string[] }, items: OperationItem[]): AppOutcome {
  const mine = app.action_ids.map((id) => items.find((i) => i.action_id === id)).filter((i): i is OperationItem => !!i);
  const run = mine[mine.length - 1];
  const why = mine.find((i) => i.status !== 'succeeded') ?? run;
  const status: AppOutcomeStatus = !run || run.status === 'pending' || run.status === 'in_flight'
    ? 'pending'
    : run.status === 'succeeded'
      ? run.code === 'leftovers_remain' || run.code === 'restart_required' ? 'partial' : 'completed'
      : run.status;
  return {
    app_instance_id: app.app_instance_id,
    name: app.name,
    status,
    code: why?.code ?? null,
    message: why?.message ?? null,
    action_ids: app.action_ids,
    leftovers_reviewable: run?.status === 'succeeded',
  };
}

/** Whether a finished stage-1 result has leftovers to review: some application's own uninstaller succeeded. */
export function leftoversReviewable(outcomes: AppOutcome[] | undefined, state: string): boolean {
  if (outcomes && outcomes.length) return outcomes.some((o) => o.leftovers_reviewable);
  return state === 'completed' || state === 'partial';
}
