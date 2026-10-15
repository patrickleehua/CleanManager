// Shared selection → review → result components (GUI-09/10/13/14, task 16.3). Cleanup, build
// artifacts, installers, uninstall leftovers, maintenance tasks, updates and old downloads all use
// the same pieces: three-tier badges, the bottom selection bar with "at least X" wording, the plan
// draft with processing modes, long-operation progress, permission readiness, flow steps, the six
// maintenance outcomes and capability pages.

import type { App } from '../main';
import { toIpcError } from '../lib/api';
import { formatBytes, groupDigits, parseBytes } from '../lib/bytes';
import { h, icon } from '../lib/dom';
import { words } from '../lib/platform';
import type { IpcError, Mode, Readiness, ReadinessCheck, ReadinessOp, Tier } from '../lib/types';
import { MODE_EXPLAIN, MODE_LABEL } from '../lib/words';
import { copyField, errorLine, errorText, pill, type Tone } from './parts';

type Child = Node | string | null | undefined | false;

// ---------------------------------------------------------------------------------------------
// Three-tier badge

export const TIER_LABEL: Record<Tier, string> = {
  default_selected: '低风险',
  review_only: '需审阅',
  blocked: '已阻止',
};

const TIER_SHAPE: Record<Tier, string> = { default_selected: 'low', review_only: 'review', blocked: 'blocked' };

export const TIER_EXPLAIN: Record<Tier, string> = {
  default_selected: '规则确认可在本机重新生成。界面仍不会替你预先勾选。',
  review_only: '可以处理，但可能需要联网重新下载、重新登录或丢失手动放入的文件。请逐项确认。',
  blocked: '受保护、被版本控制跟踪、正在使用或证据不足。只能查看，不能放入计划。',
};

/** Shape + word for a rule tier (filled circle, half circle, struck square). */
export function tierBadge(tier: Tier): HTMLElement {
  return h('span', { class: 'gmark tier', 'data-group': TIER_SHAPE[tier], title: TIER_EXPLAIN[tier] }, h('span', { class: 'gmark-shape', 'aria-hidden': 'true' }), h('span', {}, TIER_LABEL[tier]));
}

// ---------------------------------------------------------------------------------------------
// Sizes: never a fake exact total

export interface SizeSum {
  known: bigint;
  /** Objects (or files inside them) whose size is unknown. */
  unknown: number;
}

export function sumSizes(items: { bytes: string | null; unknown?: number }[]): SizeSum {
  let known = 0n;
  let unknown = 0;
  for (const i of items) {
    const v = parseBytes(i.bytes);
    if (v === null) unknown += Math.max(1, i.unknown ?? 0);
    else {
      known += v;
      unknown += i.unknown ?? 0;
    }
  }
  return { known, unknown };
}

/** "1.2 GiB", "至少 1.2 GiB", or "大小未知" — never an exact-looking total over unknown parts. */
export function atLeast(sum: SizeSum): string {
  if (sum.unknown > 0 && sum.known === 0n) return '大小未知';
  if (sum.unknown > 0) return `至少 ${formatBytes(sum.known)}`;
  return formatBytes(sum.known);
}

export function unknownNote(sum: SizeSum): string | null {
  return sum.unknown > 0 ? `${groupDigits(sum.unknown)} 项大小未知，未计入` : null;
}

// ---------------------------------------------------------------------------------------------
// Bottom selection bar

export interface SelectionBarOpts {
  count: number;
  /** Known size of the selection; `null` for selections that are not about space (updates, tasks). */
  sum: SizeSum | null;
  /** Selected objects come from this many roots / sources (one plan each). */
  sources?: number;
  action: { text: string; disabled: boolean; onClick: () => void; id?: string; danger?: boolean };
  onClear: () => void;
  note?: string | null;
}

/**
 * The same bar on every selection page: count, known size ("at least" when part is unknown), the
 * unknown count on its own, and one action that leads into plan review. Hidden when empty.
 */
export function selectionBar(o: SelectionBarOpts): HTMLElement {
  if (o.count === 0) return h('div', { class: 'selection-bar', hidden: true });
  const unknown = o.sum ? unknownNote(o.sum) : null;
  const go = h('button', { type: 'button', class: o.action.danger ? 'danger' : 'primary', id: o.action.id ?? 'selbar-go', disabled: o.action.disabled }, o.action.text);
  go.addEventListener('click', o.action.onClick);
  return h(
    'div',
    { class: 'selection-bar', role: 'region', 'aria-label': '选择栏' },
    h(
      'p',
      { class: 'selbar-sum', 'aria-live': 'polite' },
      h('b', { class: 'num' }, `已选 ${groupDigits(o.count)} 项`),
      o.sum ? h('span', { class: 'num' }, atLeast(o.sum)) : null,
      unknown ? h('span', { class: 'selbar-unknown' }, unknown) : null,
      o.sources && o.sources > 1 ? h('span', { class: 'muted' }, `来自 ${o.sources} 个位置，每个位置一份计划`) : null,
      o.note ? h('span', { class: 'muted' }, o.note) : null,
    ),
    h('div', { class: 'selbar-actions' }, h('button', { type: 'button', class: 'quiet', onclick: o.onClear }, '清空选择'), go),
  );
}

// ---------------------------------------------------------------------------------------------
// Processing mode choice (the plan draft)

export function modeOptions(opts: { name: string; allowed: Mode[]; current: Mode | null; selected: number; onChange: (m: Mode) => void; modes?: Mode[] }): HTMLElement {
  const modes = opts.modes ?? (['trash', 'quarantine', 'delete'] as Mode[]);
  return h(
    'fieldset',
    { class: 'mode-options', id: `${opts.name}-modes`, disabled: opts.selected === 0 },
    h('legend', {}, '处理方式'),
    ...modes.map((m) => {
      const allowed = opts.allowed.includes(m);
      const id = `${opts.name}-${m}`;
      const input = h('input', { type: 'radio', name: opts.name, id, value: m, disabled: !allowed, 'aria-describedby': `${id}-desc` }) as HTMLInputElement;
      input.checked = opts.current === m;
      input.addEventListener('change', () => opts.onChange(m));
      return h(
        'label',
        { class: `mode-option${input.checked ? ' checked' : ''}${allowed ? '' : ' disabled'}`, for: id, 'data-mode': m },
        input,
        h(
          'span',
          { class: 'mode-text', id: `${id}-desc` },
          h('b', {}, MODE_LABEL[m]),
          h('span', {}, MODE_EXPLAIN[m].space),
          h('span', { class: 'mode-recovery' }, MODE_EXPLAIN[m].recovery),
          opts.selected > 0 && !allowed ? h('span', { class: 'why-not' }, '所选项目中有规则不允许这种方式') : null,
        ),
      );
    }),
  );
}

/** Modes every selected object allows. */
export function commonModes(items: { allowed_modes: Mode[] }[]): Mode[] {
  if (!items.length) return [];
  return (['trash', 'quarantine', 'delete'] as Mode[]).filter((m) => items.every((i) => i.allowed_modes.includes(m)));
}

// ---------------------------------------------------------------------------------------------
// Long operations (GUI-14): stage + elapsed time; no percentage while the denominator is unknown

export class LongProgress {
  el: HTMLElement;
  private started = 0;
  private timer: number | null = null;
  private stageEl: HTMLElement;
  private elapsedEl: HTMLElement;

  constructor(label: string) {
    this.stageEl = h('span', { class: 'lp-stage' });
    this.elapsedEl = h('span', { class: 'lp-elapsed num' });
    this.el = h(
      'div',
      { class: 'long-progress', hidden: true },
      h('div', { class: 'indeterminate', role: 'progressbar', 'aria-label': label, 'aria-valuetext': '进行中，总量未知' }),
      h('p', { class: 'lp-line', role: 'status', 'aria-live': 'polite' }, this.stageEl, this.elapsedEl),
    );
  }

  start(stage: string): void {
    this.started = Date.now();
    this.el.hidden = false;
    this.stage(stage);
    this.tick();
    if (this.timer === null) this.timer = window.setInterval(() => this.tick(), 1000);
  }

  stage(text: string): void {
    this.stageEl.textContent = text;
  }

  stop(): void {
    this.el.hidden = true;
    if (this.timer !== null) window.clearInterval(this.timer);
    this.timer = null;
  }

  get running(): boolean {
    return this.timer !== null;
  }

  private tick(): void {
    const s = Math.floor((Date.now() - this.started) / 1000);
    this.elapsedEl.textContent = s >= 1 ? `已用 ${s >= 60 ? `${Math.floor(s / 60)} 分 ${s % 60} 秒` : `${s} 秒`}` : '';
  }
}

// ---------------------------------------------------------------------------------------------
// Permission readiness (GUI-13): explain gaps once before a long operation, then allow continuing

interface CheckWords {
  title: string;
  missing: (op: ReadinessOp) => string;
  fix: string;
}

export function readinessWords(forOp?: ReadinessOp): Record<string, CheckWords> {
  const w = words();
  return {
    store: {
      title: 'CleanManager 数据目录',
      missing: () => '数据目录不可用，扫描结果和计划无法保存，因此不能开始。',
      fix: '在设置 › 能力与权限中查看原因；修复后重新打开应用。',
    },
    protection_list: {
      title: '保护清单',
      missing: () => '保护清单无法读取。为安全起见，所有写操作都会停止，只能浏览。',
      fix: '在设置 › 保护清单中查看原因并修复文件。',
    },
    administrator: {
      title: w.admin,
      missing: (op) =>
        op === 'scan' || op === 'purge' || op === 'installer'
          ? '没有管理员权限：系统保护的文件夹会显示为“未读取”，不会按 0 计算。'
          : op === 'startup'
            ? '没有管理员权限：所有用户共用的启动项只能查看，不能停用或启用。'
            : op === 'maintenance'
              ? '没有管理员权限：需要管理员的任务会在开始前请 Windows 询问你一次；拒绝只跳过这些任务，其余任务照常运行。'
              : op === 'uninstall'
                ? '没有管理员权限：所有用户共用位置（HKLM、ProgramData）中的残留只能查看，不能放入计划。'
                : op === 'process'
                  ? '没有管理员权限：其他用户或系统的进程在执行时会被跳过并说明原因。'
                  : '没有管理员权限：需要管理员的项目会在执行时被跳过并说明原因。',
      fix: forOp === 'maintenance' ? 'Windows 的管理员提示只出现一次，并且只授权这份计划中列出的任务。' : `CleanManager 不会自动提权。需要时请关闭后以${w.admin}重新启动。`,
    },
    bypass_privileges: {
      title: '备份/还原特权',
      missing: () => '未能关闭进程的备份/还原特权。为避免越权读取，部分受限位置会按未读取处理。',
      fix: '通常无需处理；详情见设置 › 能力与权限。',
    },
    recycle_bin: {
      title: w.bin,
      missing: () => `有卷没有可用的${w.bin}。选择“${w.moveToBin}”的项目会失败，不会改为永久删除。`,
      fix: `改用“移入同卷隔离区”，或在系统设置中为该卷启用${w.bin}。`,
    },
    full_disk_access: {
      title: '完全磁盘访问',
      missing: () => '缺少完全磁盘访问：受保护的用户数据位置不可读，相关类目会显示为未读取。',
      fix: '在 系统设置 › 隐私与安全性 › 完全磁盘访问 中允许 CleanManager。',
    },
  };
}

const asked = new Map<string, string>();

/**
 * Check permissions before a long operation. Resolves `true` to continue (all ready, or the person
 * chose to continue with current access), `false` when cancelled or blocked. Explains each gap once
 * per session for the same operation and the same gaps — the task itself never prompts later.
 */
export async function ensureReady(app: App, op: ReadinessOp, label: string): Promise<boolean> {
  let r: Readiness;
  try {
    r = await app.backend.readiness(op);
  } catch {
    return true; // readiness is advisory; the core still refuses anything unsafe
  }
  const gaps = r.checks.filter((c) => c.state === 'missing' || (c.state === 'unknown' && c.blocking));
  if (!gaps.length) return true;
  const key = gaps.map((g) => g.check_id).sort().join(',');
  const blocking = gaps.some((g) => g.blocking);
  if (!blocking && asked.get(op) === key) return true;
  const ok = await readinessDialog(op, label, gaps, blocking);
  if (ok) asked.set(op, key);
  return ok;
}

function readinessDialog(op: ReadinessOp, label: string, gaps: ReadinessCheck[], blocking: boolean): Promise<boolean> {
  return new Promise((resolve) => {
    const table = readinessWords(op);
    const dlg = h('dialog', { class: 'cm-dialog readiness', 'aria-labelledby': 'ready-title', 'aria-describedby': 'ready-lede' }) as HTMLDialogElement;
    const finish = (v: boolean) => {
      dlg.close();
      dlg.remove();
      resolve(v);
    };
    const go = h('button', { type: 'button', class: 'primary', id: 'ready-go', disabled: blocking }, '以当前权限继续');
    go.addEventListener('click', () => finish(true));
    const cancel = h('button', { type: 'button', id: 'ready-cancel' }, '取消');
    cancel.addEventListener('click', () => finish(false));
    dlg.addEventListener('cancel', (e) => {
      e.preventDefault();
      finish(false);
    });
    dlg.append(
      h('div', { class: 'dlg-head' }, h('h2', { id: 'ready-title' }, `开始${label}之前`), h('p', { id: 'ready-lede', class: 'muted' }, blocking ? '下面的问题解决前不能开始。' : '当前权限下可以继续，但有以下影响。任务开始后不会再弹出权限请求。')),
      h(
        'ul',
        { class: 'ready-list' },
        ...gaps.map((g) => {
          const t = table[g.check_id];
          return h(
            'li',
            { 'data-blocking': g.blocking ? 'yes' : 'no' },
            h('span', { class: 'ready-mark', 'aria-hidden': 'true' }, icon(g.blocking ? 'lock' : 'warn')),
            h('span', { class: 'ready-text' }, h('b', {}, t?.title ?? g.check_id), h('span', {}, t ? t.missing(op) : (g.detail ?? '状态未知')), t ? h('span', { class: 'muted small' }, t.fix) : null),
          );
        }),
      ),
      h('div', { class: 'dlg-actions' }, cancel, go),
    );
    document.body.append(dlg);
    dlg.showModal();
    requestAnimationFrame(() => (blocking ? cancel : go).focus());
  });
}

// ---------------------------------------------------------------------------------------------
// Capability pages (GUI-08): a feature that is not here yet keeps its place and explains itself

export type GateState = 'in_progress' | 'not_applicable' | 'experimental' | 'cli_only';

export const GATE_LABEL: Record<GateState, string> = {
  in_progress: '已规格 · 开发中',
  not_applicable: '此平台不适用',
  experimental: '实验',
  cli_only: '仅命令行',
};

const GATE_TONE: Record<GateState, Tone> = { in_progress: 'busy', not_applicable: 'muted', experimental: 'warn', cli_only: 'good' };

export interface CapabilityOpts {
  id: string;
  title: string;
  gate: GateState;
  /** What the feature will do, in the user's words. */
  does: string[];
  /** Why it is not available here right now. */
  why: string;
  /** Requirement ids and the release gate it waits on. */
  spec: string;
  gateName: string;
  /** What the safety model guarantees once it ships. */
  safety?: string[];
  cli?: { label: string; command: string }[];
  actions?: Child[];
}

export function capabilityPage(o: CapabilityOpts): HTMLElement {
  return h(
    'div',
    { class: 'capability' },
    h(
      'div',
      { class: 'page-head' },
      h('div', { class: 'page-head-text' }, h('h1', { id: `${o.id}-title` }, o.title), h('p', { class: 'meta-row' }, h('span', {}, pill(GATE_LABEL[o.gate], GATE_TONE[o.gate])), h('span', {}, o.spec), h('span', {}, `门禁 ${o.gateName}`))),
    ),
    h(
      'div',
      { class: 'cap-grid' },
      h('section', { class: 'panel cap-does', 'aria-labelledby': `${o.id}-does` }, h('h2', { id: `${o.id}-does` }, '它会做什么'), h('ul', { class: 'cap-list-plain' }, ...o.does.map((d) => h('li', {}, d)))),
      h(
        'section',
        { class: 'panel cap-why', 'aria-labelledby': `${o.id}-why` },
        h('h2', { id: `${o.id}-why` }, o.gate === 'not_applicable' ? '为什么这里不提供' : '现在的状态'),
        h('p', {}, o.why),
        o.safety?.length ? h('h3', {}, '上线后仍然遵守') : null,
        o.safety?.length ? h('ul', { class: 'cap-list-plain small' }, ...o.safety.map((s) => h('li', {}, s))) : null,
        o.cli?.length ? h('h3', {}, '现在可以在终端中使用') : null,
        ...(o.cli ?? []).map((c) => copyField(c.label, c.command)),
        o.actions?.length ? h('div', { class: 'card-actions' }, ...o.actions) : null,
      ),
    ),
  );
}

// ---------------------------------------------------------------------------------------------
// Steps of a flow that really is a sequence (an uninstall: the uninstaller, then its leftovers)

export interface Step {
  title: string;
  note: string;
}

/** Numbered steps; `current` is highlighted, earlier ones read as done. */
export function stageSteps(label: string, steps: Step[], current: number): HTMLElement {
  return h(
    'ol',
    { class: 'stage-steps', 'aria-label': label },
    ...steps.map((s, i) =>
      h(
        'li',
        { 'data-state': i < current ? 'done' : i === current ? 'current' : 'next', 'aria-current': i === current ? 'step' : null },
        h('span', { class: 'step-num', 'aria-hidden': 'true' }, String(i + 1)),
        h('span', { class: 'step-text' }, h('b', {}, s.title), h('span', {}, s.note)),
      ),
    ),
  );
}

// ---------------------------------------------------------------------------------------------
// Six maintenance outcomes (MNT-03): every outcome has its own shape and word

export type Outcome = 'applied' | 'unchanged' | 'attention' | 'skipped' | 'unavailable' | 'failed';

export const OUTCOME_LABEL: Record<Outcome, string> = {
  applied: '已执行',
  unchanged: '无需改变',
  attention: '需要跟进',
  skipped: '已跳过',
  unavailable: '不可用',
  failed: '失败',
};

export function outcomeMark(o: Outcome, withLabel = true): HTMLElement {
  return h('span', { class: 'omark', 'data-outcome': o }, h('span', { class: 'omark-shape', 'aria-hidden': 'true' }), withLabel ? h('span', {}, OUTCOME_LABEL[o]) : h('span', { class: 'sr-only' }, OUTCOME_LABEL[o]));
}

/** All six counts in a fixed order; zero counts stay visible but quiet, so nothing reads as "all done" by omission. */
export function outcomeTally(counts: Partial<Record<Outcome, number>>, label = '结果'): HTMLElement {
  const order: Outcome[] = ['applied', 'unchanged', 'attention', 'skipped', 'unavailable', 'failed'];
  return h(
    'ul',
    { class: 'outcome-tally', 'aria-label': label },
    ...order.map((o) => h('li', { 'data-zero': (counts[o] ?? 0) === 0 ? 'yes' : 'no' }, outcomeMark(o), h('b', { class: 'num' }, groupDigits(counts[o] ?? 0)))),
  );
}

// ---------------------------------------------------------------------------------------------
// Misc

/** A notice line with an optional close button. */
export function noticeLine(text: string, tone: 'info' | 'warn' = 'info', onClose?: () => void): HTMLElement {
  return h(
    'div',
    { class: `notice${tone === 'warn' ? ' warn' : ''}`, role: 'status' },
    h('span', {}, text),
    onClose ? h('button', { type: 'button', class: 'quiet', onclick: onClose }, '知道了') : null,
  );
}

export function errorBlock(e: IpcError | null): HTMLElement | null {
  return errorLine(e);
}

export { errorText, toIpcError };
