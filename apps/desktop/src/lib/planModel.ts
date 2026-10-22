// P05 plan review: what a stored plan will do, said once before the item table — one sentence, the
// counts, and how (or whether) it can be undone — plus which authorization state the plan is in. Pure
// so every mode's sentence and the irreversible marking are tested without a DOM.

import { groupDigits, parseBytes } from './bytes';
import { sizeLabel, type BannerKind } from './display';
import { words } from './platform';
import type { Mode, PlanDetail, PlanItem } from './types';
import { MODE_EXPLAIN, MODE_LABEL, RECOVERY_EXPLAIN, STALE_LABEL } from './words';

type Explain = { effect: string; space: string; recovery: string };

const STARTUP_EXPLAIN = {
  get effect(): string {
    return words().startupEffect;
  },
  space: '不影响磁盘空间。',
  recovery: '执行前记录每一项原来的状态；之后可以在“活动”中为这次执行生成还原计划。',
};

/** What plans of the newer kinds do, in the same three lines as the file modes. */
const KIND_EXPLAIN: Partial<Record<PlanDetail['kind'], Explain>> = {
  uninstall: {
    effect: '先请应用退出并等待；应用没有退出时什么都不做。然后运行它自己的卸载程序（保留它的界面），记录退出码和耗时，再重新检查安装目录和卸载登记。',
    space: '卸载程序删除的内容不经过 CleanManager；留下的文件在下一步单独审阅。',
    recovery: '卸载无法由 CleanManager 撤销；需要时重新安装这个应用。',
  },
  uninstall_leftovers: {
    effect: '文件与文件夹移到回收站；注册表项先把整棵键备份到 CleanManager 的记录中再删除；启动项先备份再移除。',
    space: '回收站仍占用这个卷的空间，清空回收站后才释放。',
    recovery: '文件在回收站中还原；注册表项与启动项可以用“生成还原计划”从备份放回，目标已被重新创建时跳过，不覆盖。',
  },
  uninstall_restore: {
    effect: '从执行时保存的备份重新创建注册表项与启动值。',
    space: '不影响磁盘空间。',
    recovery: '目标已被重新创建时跳过该项，从不覆盖。',
  },
  update: {
    effect: '逐个应用执行：下载这个确切版本，验证安装包签名者与已安装应用一致，然后升级并回读版本。',
    space: '新版本可能比旧版本略大；下载的安装包放在临时目录。',
    recovery: '更新不能由 CleanManager 撤销；签名不一致或无法验证时不安装，原应用不变。',
  },
  process: {
    effect: '执行前按 PID、启动时间和程序文件重新核对这个进程，相符才结束它。',
    space: '不影响磁盘空间。',
    recovery: '无法撤销：进程中未保存的工作会丢失。需要时重新打开该程序。',
  },
  maintenance: {
    effect: '按顺序运行列出的固定任务；每项运行前再次只读探测，不适用的跳过并说明原因。',
    space: '维护不承诺释放空间，也不承诺“加速”。',
    recovery: '维护操作没有通用撤销；每项结果和原因都记录在运行记录中。',
  },
};

/** Kinds whose actions are not about space: no size column or figure. */
export const NO_SPACE = new Set<PlanDetail['kind']>(['startup', 'startup_restore', 'uninstall', 'uninstall_restore', 'update', 'process', 'maintenance']);

/** What a plan of this kind does, in one line, and how it reads on the execute button. */
export function kindVerb(d: PlanDetail, items: PlanItem[]): { verb: string; run: string; permanent: boolean } {
  const n = groupDigits(d.item_count ?? 0);
  switch (d.kind) {
    case 'cleanup':
      return {
        verb: `${d.mode ? MODE_LABEL[d.mode] : '处理'}：${n} 个${d.catalog === 'purge' ? '构建产物中的文件' : d.catalog === 'installer' ? '安装包' : '文件'}`,
        run: `执行：${d.mode ? MODE_LABEL[d.mode] : '处理'}（${n} 项）`,
        permanent: d.mode === 'delete',
      };
    case 'manual_trash':
      return { verb: `${words().moveToBin}：${n} 个手选对象`, run: `执行：${words().moveToBin}（${n} 项）`, permanent: false };
    case 'manual_purge':
      return { verb: `从${words().bin}永久删除：${n} 个对象`, run: `执行：永久删除 ${n} 项`, permanent: true };
    case 'startup': {
      const disable = items.every((i) => i.action_type === 'startup_disable');
      const enable = items.every((i) => i.action_type === 'startup_enable');
      const what = disable ? '停用' : enable ? '启用' : '修改';
      const name = d.item_count === 1 && items[0] ? `“${items[0].name}”` : `${n} 个启动项`;
      return { verb: `${what}启动项：${name}`, run: `执行：${what}${d.item_count === 1 ? '这个启动项' : ` ${n} 个启动项`}`, permanent: false };
    }
    case 'startup_restore':
      return { verb: `还原启动项：${n} 项`, run: `执行：还原 ${n} 项`, permanent: false };
    case 'uninstall': {
      const apps = d.kind_detail?.apps ?? [];
      if (apps.length > 1)
        return { verb: `卸载 ${apps.length} 个应用：逐个先请它退出，${words().uninstallRuns}`, run: `执行：卸载 ${apps.length} 个应用`, permanent: false };
      const name = d.kind_detail?.app?.name ?? items[0]?.name ?? '应用';
      return { verb: `卸载“${name}”：先请它退出，${words().uninstallRuns}`, run: `执行：卸载 ${name}`, permanent: false };
    }
    case 'uninstall_leftovers': {
      const name = d.kind_detail?.noted?.app_name;
      return { verb: `处理${name ? `“${name}”的` : ''}卸载残留：${n} 个动作`, run: `执行：处理 ${n} 项残留`, permanent: false };
    }
    case 'uninstall_restore':
      return { verb: `从备份还原注册表项：${n} 项`, run: `执行：还原 ${n} 项`, permanent: false };
    case 'update':
      return { verb: `更新 ${n} 个应用`, run: `执行：更新 ${n} 个应用`, permanent: false };
    case 'process': {
      const name = d.kind_detail?.noted?.name ?? items[0]?.name ?? '进程';
      return { verb: `结束进程：${items[0]?.name ?? name}`, run: '执行：结束这个进程', permanent: true };
    }
    case 'maintenance':
      return { verb: `运行 ${n} 项维护任务`, run: `执行：运行 ${n} 项维护任务`, permanent: false };
    case 'restore':
      return { verb: `恢复到原位置：${n} 个对象`, run: `执行：恢复 ${n} 项`, permanent: false };
    default:
      return { verb: `永久清空：${n} 个隔离对象`, run: `执行：永久清空 ${n} 项`, permanent: true };
  }
}

/** What the plan does, to space, and how it is undone (null for kinds without the three lines). */
export function planExplain(d: PlanDetail): Explain | null {
  if ((d.kind === 'cleanup' || d.kind === 'manual_trash') && d.mode) return MODE_EXPLAIN[d.mode];
  if (d.kind === 'startup' || d.kind === 'startup_restore') return STARTUP_EXPLAIN;
  if (d.kind === 'manual_purge') return MODE_EXPLAIN.delete;
  if (d.kind === 'restore' || d.kind === 'purge') return RECOVERY_EXPLAIN[d.kind];
  return words().kindExplain[d.kind] ?? KIND_EXPLAIN[d.kind] ?? null;
}

// ---------------------------------------------------------------------------------------------
// Reversibility: a shape and a word for every plan (never colour alone)

export type Undo = 'restorable' | 'recoverable' | 'irreversible' | 'external' | 'none';

export const UNDO_LABEL: Record<Undo, string> = {
  restorable: '可以还原',
  recoverable: '可以恢复，但不立即释放空间',
  irreversible: '无法撤销',
  external: 'CleanManager 无法撤销',
  none: '不需要撤销',
};

/** The short form beside a mode choice, where the next line already says what happens to space. */
export const UNDO_SHORT: Record<Undo, string> = { ...UNDO_LABEL, recoverable: '可以恢复' };

export const MODE_UNDO: Record<Mode, Undo> = { trash: 'restorable', quarantine: 'recoverable', delete: 'irreversible' };

export function planUndo(d: PlanDetail, items: PlanItem[]): Undo {
  if (kindVerb(d, items).permanent) return 'irreversible';
  if ((d.kind === 'cleanup' || d.kind === 'manual_trash') && d.mode) return MODE_UNDO[d.mode];
  switch (d.kind) {
    case 'startup':
    case 'uninstall_leftovers':
      return 'restorable';
    case 'uninstall':
    case 'update':
    case 'maintenance':
      return 'external';
    default:
      return 'none';
  }
}

// ---------------------------------------------------------------------------------------------
// The statement: one sentence + counts + reversibility, before the item table

/** Words of plan review that are not data (one table for the language switch). */
export const PLAN_TEXT = {
  objects: '对象',
  knownSize: '已知大小',
  source: '来源',
  rules: '规则',
  count: (n: number) => `${groupDigits(n)} 项`,
  ruleCount: (n: number) => `${groupDigits(n)} 条`,
  notFreed: '此操作不立即释放空间',
} as const;

export interface PlanCount {
  label: string;
  value: string;
  /** The value is a path (shown shortened in the middle). */
  path?: boolean;
}

export interface PlanStatement {
  /** What will happen, as one sentence (no trailing full stop: it is the headline). */
  sentence: string;
  counts: PlanCount[];
  undo: Undo;
  undoLabel: string;
  /** How it is undone, or why it cannot be. */
  undoText: string | null;
  /** What it does to free space. */
  spaceText: string | null;
  /** The plan deletes something for good: the page marks it with a shape, a word and a pattern. */
  irreversible: boolean;
  /** The CSS tone of the statement edge. */
  tone: 'delete' | 'trash' | 'quarantine' | 'review' | 'restore';
}

function objectWord(d: PlanDetail): string {
  return d.catalog === 'purge' ? '构建产物中的文件' : d.catalog === 'installer' ? '安装包' : '文件';
}

/** The headline sentence for each kind; kinds without a file mode keep their review line. */
export function planSentence(d: PlanDetail, items: PlanItem[]): string {
  const n = groupDigits(d.item_count ?? 0);
  const w = words();
  if (d.kind === 'cleanup' && d.mode) return d.mode === 'delete' ? `永久删除 ${n} 个${objectWord(d)}` : `把 ${n} 个${objectWord(d)}${MODE_LABEL[d.mode]}`;
  if (d.kind === 'manual_trash') return `把 ${n} 个手选对象${w.moveToBin}`;
  if (d.kind === 'manual_purge') return `从${w.bin}永久删除 ${n} 个对象`;
  if (d.kind === 'restore') return `把 ${n} 个隔离对象恢复到原位置`;
  if (d.kind === 'purge') return `永久清空 ${n} 个隔离对象`;
  return kindVerb(d, items).verb;
}

export function planStatement(d: PlanDetail, items: PlanItem[]): PlanStatement {
  const T = PLAN_TEXT;
  const permanent = kindVerb(d, items).permanent;
  const explain = planExplain(d);
  const undo = planUndo(d, items);
  const counts: PlanCount[] = [{ label: T.objects, value: T.count(d.item_count ?? 0) }];
  if (!NO_SPACE.has(d.kind)) {
    let total: bigint | null = null;
    try {
      total = parseBytes(d.total_logical_bytes);
    } catch {
      total = null;
    }
    counts.push({ label: T.knownSize, value: sizeLabel(total) });
  }
  if ((d.kind === 'cleanup' || d.kind === 'manual_trash') && d.scan_root_display) counts.push({ label: T.source, value: d.scan_root_display, path: true });
  if (d.kind === 'cleanup' && d.rule_refs.length) counts.push({ label: T.rules, value: T.ruleCount(d.rule_refs.length) });
  const tone: PlanStatement['tone'] = permanent
    ? 'delete'
    : d.kind === 'uninstall_leftovers'
      ? 'trash'
      : d.kind === 'uninstall' || d.kind === 'update' || d.kind === 'maintenance'
        ? 'review'
        : d.mode === 'trash' || d.mode === 'quarantine'
          ? d.mode
          : 'restore';
  return {
    sentence: planSentence(d, items),
    counts,
    undo,
    undoLabel: UNDO_LABEL[undo],
    // The badge already says 无法撤销; the line under it starts where the badge ends.
    undoText: explain ? (undo === 'irreversible' && explain.recovery.startsWith(UNDO_LABEL.irreversible) ? explain.recovery.slice(UNDO_LABEL.irreversible.length).replace(/^[，：:、\s]+/, '') || explain.recovery : explain.recovery) : null,
    spaceText: explain?.space ?? null,
    irreversible: permanent,
    tone,
  };
}

// ---------------------------------------------------------------------------------------------
// Authorization state

export type AuthView = 'executed' | 'terminal' | 'review' | 'authorized' | 'stale' | 'revoked' | 'expired' | 'closed';

export const AUTH_TEXT = {
  executed: '这份计划已经执行过；同一份计划不会再次执行。',
  terminal: '这份计划需要在终端中确认；这个窗口不批准、也不执行它。',
  authorized: '已批准，尚未执行。',
  revoked: '批准已撤销，这份计划不能执行。',
  expired: '计划已过有效期，不能再批准；请从候选重新生成。',
  closed: '这份计划不能再批准。',
} as const;

export function authView(d: Pick<PlanDetail, 'operation' | 'authorization' | 'terminal_only' | 'review_token' | 'stale_reason'>): { view: AuthView; line: { kind: BannerKind; text: string } | null } {
  const a = d.authorization.status;
  if (d.operation) return { view: 'executed', line: { kind: 'success', text: AUTH_TEXT.executed } };
  if (d.stale_reason) return { view: 'stale', line: { kind: 'failed', text: STALE_LABEL[d.stale_reason] ?? AUTH_TEXT.closed } };
  if (a === 'required' && d.terminal_only) return { view: 'terminal', line: { kind: 'pending-permission', text: AUTH_TEXT.terminal } };
  if (a === 'required' && d.review_token) return { view: 'review', line: null };
  if (a === 'authorized') return { view: 'authorized', line: { kind: 'success', text: AUTH_TEXT.authorized } };
  if (a === 'revoked') return { view: 'revoked', line: { kind: 'cancelled', text: AUTH_TEXT.revoked } };
  if (a === 'expired') return { view: 'expired', line: { kind: 'cancelled', text: AUTH_TEXT.expired } };
  return { view: 'closed', line: { kind: 'cancelled', text: AUTH_TEXT.closed } };
}
