// P06 — execution and result detail (GUI-01/06, EXE-*, REC-05). Progress is counted over the
// fixed plan total; "stop" only stops scheduling new actions. Results are per item with the
// reason in words. Processed, quarantined and observed free-space change are reported apart:
// a same-volume quarantine frees ~0 and is never celebrated as space reclaimed. Unknown
// outcomes lead to a read-only reconciliation, never to a retry.

import type { App } from '../main';
import { toIpcError } from '../lib/api';
import { formatBytes, groupDigits, parseBytes } from '../lib/bytes';
import { h, keepFocus, replace } from '../lib/dom';
import { formatElapsed, formatTime } from '../lib/format';
import type { IpcError, ItemFilter, Operation, OperationItem } from '../lib/types';
import { AVAILABILITY_LABEL, MODE_LABEL, OPERATION_STATE_LABEL, PLAN_KIND_LABEL, explainCode, operationTone } from '../lib/words';
import { copyField, emptyState, errorLine, pageHead, pill, segBar, statusWord } from './parts';
import type { MaintenanceReport } from '../lib/suiteTypes';
import { ELEVATION_WORD, TASK_WORDS } from '../lib/suiteWords';
import { outcomeMark, outcomeTally, type Outcome } from './kit';
import { allDone, taskReason } from './maintenance';

/** Kinds whose results are not about disk space (no size figures). */
const NO_SPACE = new Set(['startup', 'startup_restore', 'uninstall', 'uninstall_restore', 'update', 'process', 'maintenance']);
const OUTCOMES = new Set(['applied', 'unchanged', 'attention', 'skipped', 'unavailable', 'failed']);

const ITEM_PAGE = 100;
const FILTERS: { key: ItemFilter; label: string }[] = [
  { key: 'problems', label: '未成功' },
  { key: 'all', label: '全部' },
  { key: 'succeeded', label: '成功' },
];

export class OperationPage {
  el: HTMLElement;
  private body: HTMLElement;
  private opId: string | null = null;
  private op: Operation | null = null;
  private error: IpcError | null = null;
  private filter: ItemFilter = 'problems';
  private items: OperationItem[] = [];
  private itemsNext: string | null = null;
  private itemsTotal = 0;
  private itemsLoading = false;
  private busy = false;
  private actionError: IpcError | null = null;
  private refreshTimer: number | null = null;
  private lastApplyKey = '';
  private userFiltered = false;
  /** MNT-07: the recorded maintenance report of this operation (six outcomes, elevation). */
  private maint: MaintenanceReport | null = null;
  private restoreError: IpcError | null = null;

  constructor(private app: App) {
    this.body = h('div', { class: 'operation-body' });
    this.el = h('section', { class: 'page page-operation', 'aria-labelledby': 'op-title' }, this.body);
  }

  activate(): void {
    const id = this.app.store.state.operationId;
    if (id) void this.load(id);
  }

  update(): void {
    const s = this.app.store.state;
    if (s.operationId && s.operationId !== this.opId) void this.load(s.operationId);
    // Follow the running operation at the event rate, re-reading the store at most every 1.2 s.
    const a = s.apply;
    if (a && a.operationId === this.opId) {
      const key = JSON.stringify([a.counts, a.running, a.cancelRequested]);
      if (key !== this.lastApplyKey) {
        this.lastApplyKey = key;
        this.scheduleRefresh(!a.running);
      }
    }
    keepFocus(this.el, () => this.render());
  }

  private scheduleRefresh(final: boolean): void {
    if (this.refreshTimer !== null && !final) return;
    if (this.refreshTimer !== null) window.clearTimeout(this.refreshTimer);
    this.refreshTimer = window.setTimeout(
      () => {
        this.refreshTimer = null;
        if (this.opId) void this.load(this.opId, true);
      },
      final ? 50 : 1200,
    );
  }

  private async load(id: string, keepItems = false): Promise<void> {
    const switching = id !== this.opId;
    this.opId = id;
    if (switching) {
      this.op = null;
      this.maint = null;
      this.restoreError = null;
      this.userFiltered = false;
      this.items = [];
      this.actionError = null;
      this.filter = 'problems';
    }
    try {
      const op = await this.app.backend.operationDetail(id);
      if (this.opId !== id) return;
      this.op = op;
      this.error = null;
      if (op.plan_kind === 'maintenance' && op.report_available && this.maint?.operation_id !== id) {
        const hist = await this.app.backend.maintenanceHistory(50).catch(() => null);
        if (this.opId === id) this.maint = hist?.items.find((r) => r.operation_id === id) ?? null;
      }
      // Everything succeeded: show all rows (an empty "not succeeded" list says nothing about what happened).
      if ((switching || !this.userFiltered) && op.counts.succeeded === op.total && op.total > 0 && !op.running_here) this.filter = 'all';
    } catch (e) {
      if (this.opId === id) this.error = toIpcError(e);
    }
    await this.loadItems(false, keepItems);
  }

  private async loadItems(more: boolean, quiet = false): Promise<void> {
    const id = this.opId;
    if (!id || this.itemsLoading) return;
    this.itemsLoading = true;
    if (!quiet) this.update();
    try {
      const want = more ? ITEM_PAGE : Math.max(ITEM_PAGE, quiet ? this.items.length : 0);
      const page = await this.app.backend.operationItems(id, this.filter, Math.min(500, want), more ? this.itemsNext : null);
      if (this.opId !== id) return;
      this.items = more ? [...this.items, ...page.items] : page.items;
      this.itemsNext = page.next_cursor;
      this.itemsTotal = page.total;
    } catch (e) {
      this.actionError = toIpcError(e);
    } finally {
      this.itemsLoading = false;
      this.update();
    }
  }

  private async stop(): Promise<void> {
    if (!this.opId) return;
    const ok = await this.app.backend.cancelApply(this.opId).catch(() => false);
    const a = this.app.store.state.apply;
    if (ok && a) this.app.store.set({ apply: { ...a, cancelRequested: true } });
    this.app.announce(ok ? '已请求停止：当前动作完成后不再开始新的动作' : '操作已经结束');
  }

  private async reconcile(): Promise<void> {
    if (!this.opId || this.busy) return;
    this.busy = true;
    this.actionError = null;
    this.update();
    try {
      this.op = await this.app.backend.reconcileOperation(this.opId);
      this.app.announce('核对完成');
      await this.loadItems(false);
    } catch (e) {
      this.actionError = toIpcError(e);
    } finally {
      this.busy = false;
      this.update();
    }
  }

  private back(): { label: string; onClick: () => void } {
    const from = this.app.store.state.returnTo;
    if (from === 'plan') return { label: '返回计划', onClick: () => this.app.navigate('plan') };
    if (from === 'uninstall') return { label: '返回卸载', onClick: () => this.app.navigate('uninstall') };
    return { label: '返回活动', onClick: () => this.app.navigate('activity') };
  }

  private render(): void {
    if (!this.opId) {
      replace(this.body, h('h1', { id: 'op-title' }, '执行结果'), emptyState('没有打开的执行记录', '执行一份已批准的计划后，这里显示逐项结果。', h('button', { type: 'button', onclick: () => this.app.navigate('activity') }, '查看活动记录')));
      return;
    }
    if (this.error && !this.op) {
      replace(this.body, pageHead({ id: 'op-title', title: '执行结果', back: this.back() }), h('div', { class: 'panel' }, errorLine(this.error)));
      return;
    }
    const op = this.op;
    if (!op) {
      replace(this.body, pageHead({ id: 'op-title', title: '执行结果', back: this.back() }), h('p', { class: 'muted', role: 'status' }, '正在读取执行记录…'));
      return;
    }
    const a = this.app.store.state.apply;
    const live = a && a.operationId === op.operation_id && a.running ? a : null;
    const counts = live ? live.counts : op.counts;
    const total = live ? live.total : op.total;
    const state = live ? (live.cancelRequested ? 'stopping' : 'running') : op.state;
    const action = (op.plan_kind === 'cleanup' || op.plan_kind === 'manual_trash') && op.mode ? MODE_LABEL[op.mode] : op.plan_kind === 'unknown' ? '未知计划' : (PLAN_KIND_LABEL[op.plan_kind] ?? op.plan_kind);
    replace(
      this.body,
      pageHead({
        id: 'op-title',
        title: '执行结果',
        back: this.back(),
        meta: [
          h('code', { class: 'selectable' }, op.operation_id),
          pill(OPERATION_STATE_LABEL[state] ?? state, operationTone(state)),
          action,
          op.started_at ? `开始于 ${formatTime(op.started_at)}` : null,
          op.finished_at ? `用时 ${formatElapsed(op.started_at, op.finished_at)}` : null,
        ],
        actions: [h('button', { type: 'button', class: 'quiet', onclick: () => this.app.openPlan(op.plan_id, 'activity') }, '查看计划')],
      }),
      h('div', { class: 'work-layout' }, h('div', { class: 'work-main' }, this.progressPanel(op, state, total, counts, !!live), this.itemsPanel()), this.summaryPanel(op, state)),
    );
  }

  private progressPanel(op: Operation, state: string, total: number, counts: Operation['counts'], live: boolean): HTMLElement {
    const lines: (HTMLElement | null)[] = [];
    if (state === 'running')
      lines.push(h('p', { class: 'muted' }, '每一项在执行前都会重新检查对象、占用和保护。计划条目数是进度的分母，不代表释放空间的比例。'));
    if (state === 'stopping') lines.push(h('p', {}, '已请求停止：正在进行的动作结束后不再开始新的动作。已处理的项目不会自动撤销。'));
    if (op.needs_reconcile && !live)
      lines.push(
        h(
          'div',
          { class: 'notice warn', role: 'status' },
          h('span', {}, '有动作的结果无法确认，可能已经生效。先进行只读核对；核对不会重新执行任何动作，也不会自动重试。'),
          h('button', { type: 'button', id: 'op-reconcile', class: 'primary', disabled: this.busy, onclick: () => void this.reconcile() }, this.busy ? '正在核对…' : '核对结果'),
        ),
      );
    if (state === 'cancelled') lines.push(h('p', {}, '你停止了这次执行。停止前完成的项目保留其结果；未开始的项目没有被触碰。'));
    if (state === 'partial' && op.plan_kind === 'uninstall' && counts.failed + counts.skipped === 0)
      lines.push(h('p', {}, '卸载程序已运行，但重新检查发现安装目录或卸载登记仍在，所以记为“部分完成”。剩下的内容在“审阅残留”中单独处理。'));
    else if (state === 'partial' && op.plan_kind === 'maintenance' && counts.failed + counts.skipped === 0)
      lines.push(h('p', {}, '有任务需要你跟进（见下表），所以记为“部分完成”。'));
    else if (state === 'partial') lines.push(h('p', {}, '部分项目被跳过或失败，原因见下表。需要处理剩余对象时，请重新分析并生成新计划；系统不会重做已成功的项目。'));
    const stop = live && state === 'running' ? h('button', { type: 'button', id: 'op-stop', class: 'danger-quiet', onclick: () => void this.stop() }, '停止后续操作') : null;
    return h(
      'section',
      { class: 'panel progress-panel', 'aria-labelledby': 'progress-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'progress-title' }, live ? '进度' : '结果'), stop),
      segBar(total, counts, live ? '执行进度' : '执行结果'),
      ...lines,
      errorLine(this.actionError),
    );
  }

  private summaryPanel(op: Operation, state: string): HTMLElement {
    const processed = parseBytes(op.processed_logical_bytes);
    const quarantined = parseBytes(op.quarantined_logical_bytes);
    const freed = parseBytes(op.estimated_freed_bytes);
    const delta = op.observed_free_delta_bytes === null ? null : BigInt(op.observed_free_delta_bytes);
    const deltaText = delta === null ? '空间变化未知' : delta >= 0n ? `+${formatBytes(delta)}` : `−${formatBytes(-delta)}`;
    const quarantine = op.plan_kind === 'cleanup' && op.mode === 'quarantine';
    // GUI-10: when some outcomes are unconfirmed the processed amount is a lower bound, never exact.
    const partial = op.counts.unknown > 0 || op.counts.in_flight > 0;
    const toggles = NO_SPACE.has(op.plan_kind);
    const rows: (HTMLElement | null)[] = [
      toggles
        ? figure('已处理', `${groupDigits(op.counts.succeeded)} 项`, op.plan_kind === 'startup' || op.plan_kind === 'startup_restore' ? '启动项只改启用记录，不涉及磁盘空间。' : '这类操作不按空间计算结果，逐项结果见左侧。')
        : figure(
            '已处理',
            processed === null ? '—' : partial ? `至少 ${formatBytes(processed)}` : formatBytes(processed),
            `成功项目的逻辑大小，共 ${groupDigits(op.counts.succeeded)} 项${partial ? `；另有 ${groupDigits(op.counts.unknown + op.counts.in_flight)} 项结果未确认，未计入` : ''}`,
          ),
      quarantine ? figure('仍在隔离区', quarantined === null ? '—' : formatBytes(quarantined), '这些文件仍占用同一个卷，没有释放空间。') : null,
      op.mode === 'delete' || op.plan_kind === 'purge' ? figure('估计释放', freed === null ? '—' : formatBytes(freed), '按逻辑大小估计；实际以卷空间观测为准。') : null,
      toggles
        ? null
        : figure(
        '卷可用空间变化',
        deltaText,
        delta === null
          ? '没有可靠的前后读数，保留操作结果，不推测空间变化。'
          : `执行前后的读数差，可能包含其他程序同期的写入。${op.volume_free_after_bytes ? `执行后可用 ${formatBytes(op.volume_free_after_bytes)}。` : ''}`,
      ),
    ];
    const recovery = this.recoveryLine(op);
    const next = this.nextStep(op, state);
    if (state === 'running' || state === 'stopping')
      return h(
        'aside',
        { class: 'panel work-aside op-summary', 'aria-labelledby': 'op-summary-title' },
        h('h2', { id: 'op-summary-title' }, '执行中'),
        h('p', { class: 'small' }, '空间与恢复情况在执行结束后按实际结果给出；进行中不显示估算。'),
        h('p', { class: 'small muted' }, '切换页面不会中断执行。关闭窗口会先停止派发后续动作，等当前动作完成并记录后再退出。'),
        h('dl', { class: 'facts-list small' }, h('dt', {}, '计划'), h('dd', {}, h('code', { class: 'selectable' }, op.plan_id))),
      );
    return h(
      'aside',
      { class: 'panel work-aside op-summary', 'aria-labelledby': 'op-summary-title' },
      h('h2', { id: 'op-summary-title' }, '空间与恢复'),
      next,
      ...rows,
      recovery,
      h(
        'dl',
        { class: 'facts-list small' },
        h('dt', {}, '授权来源'),
        h('dd', {}, op.authorization_source === 'interactive_review' ? '人工审阅' : op.authorization_source?.startsWith('policy:') ? `策略 ${op.authorization_source.slice(7)}` : (op.authorization_source ?? '—')),
        h('dt', {}, '计划'),
        h('dd', {}, h('code', { class: 'selectable' }, op.plan_id)),
        h('dt', {}, '执行宿主'),
        h('dd', {}, op.running_here ? '本窗口（关闭窗口会先停止后续操作）' : '已结束或由其他进程执行'),
      ),
    );
  }

  /** What comes after this result: the leftovers review of an uninstall, the six outcomes of a maintenance run. */
  private nextStep(op: Operation, state: string): HTMLElement | null {
    if (op.plan_kind === 'uninstall' && (state === 'completed' || state === 'partial')) {
      return h(
        'div',
        { class: 'next-block' },
        h('h3', {}, '下一步：审阅残留'),
        h('p', { class: 'small' }, '卸载程序已经运行。重新检查后，确实留下的文件、注册表项和启动项在单独的第 2 步审阅；什么都不会自动删除。'),
        h('button', { type: 'button', class: 'primary wide', id: 'op-leftovers', onclick: () => this.app.store.set({ leftoversOp: op.operation_id, page: 'uninstall' }) }, '审阅残留'),
      );
    }
    if (op.plan_kind === 'uninstall' && state === 'failed')
      return h('div', { class: 'next-block' }, h('h3', {}, '没有卸载'), h('p', { class: 'small' }, '应用没有退出或卸载程序没有运行成功，原因见左侧。关闭应用后可以重新生成卸载计划。'));
    if (op.plan_kind === 'update' && op.counts.skipped > 0)
      return h('div', { class: 'next-block' }, h('h3', {}, '跳过的应用'), h('p', { class: 'small' }, '正在运行的应用被跳过了。请先关闭它们，再生成一份新的更新计划。'), h('button', { type: 'button', class: 'quiet', onclick: () => this.app.navigate('updates') }, '返回更新'));
    const m = this.maint;
    if (op.plan_kind === 'maintenance' && m)
      return h(
        'div',
        { class: 'next-block' },
        h('h3', {}, allDone(m) ? '每项任务都已执行或无需改变' : '有需要跟进或没有执行的任务'),
        outcomeTally(m.counts, '六种结果'),
        h('p', { class: 'small muted' }, `提权：${ELEVATION_WORD[m.elevation] ?? m.elevation}`),
      );
    return null;
  }

  private async restore(op: Operation): Promise<void> {
    this.restoreError = null;
    try {
      const r = await this.app.backend.uninstallRestorePlan(op.operation_id);
      this.app.announce('还原计划已生成，尚未执行。请审阅后批准');
      this.app.openPlan(r.plan_id, 'operation');
    } catch (e) {
      this.restoreError = toIpcError(e);
      this.update();
    }
  }

  private recoveryLine(op: Operation): HTMLElement | null {
    if (op.counts.succeeded === 0 && !op.needs_reconcile) return null;
    const plain = (text: string, ...extra: (HTMLElement | null)[]) => h('div', { class: 'recovery-line' }, h('h3', {}, '如何恢复'), h('p', { class: 'small' }, text), ...extra);
    if (!op.needs_reconcile) {
      if (op.plan_kind === 'uninstall') return plain('卸载无法由 CleanManager 撤销；需要时重新安装这个应用。');
      if (op.plan_kind === 'update') return plain('更新无法由 CleanManager 撤销；需要旧版本时请从发布者处获取。');
      if (op.plan_kind === 'process') return plain('结束进程无法撤销；需要时重新打开该程序。');
      if (op.plan_kind === 'maintenance') return plain('维护操作没有通用撤销；每项结果和原因都记录在“优化 › 维护任务”的运行记录中。');
      if (op.plan_kind === 'uninstall_restore') return plain('这是一次还原；它本身不再生成新的还原。');
      if (op.plan_kind === 'uninstall_leftovers')
        return plain(
          `文件与文件夹${AVAILABILITY_LABEL.system_recycle_bin}。注册表项与启动项可以从执行时保存的备份放回，同样先审阅再执行。`,
          h('button', { type: 'button', class: 'quiet', id: 'op-restore', onclick: () => void this.restore(op) }, '生成还原计划'),
          errorLine(this.restoreError),
        );
    }
    if ((op.plan_kind === 'startup' || op.plan_kind === 'startup_restore') && !op.needs_reconcile)
      return h('div', { class: 'recovery-line' }, h('h3', {}, '如何恢复'), h('p', { class: 'small' }, op.plan_kind === 'startup' ? '执行前记录了每一项原来的状态。需要时用下面的命令生成还原计划，同样先审阅再执行。' : '这是一次还原；它本身不再生成新的还原。'), op.plan_kind === 'startup' ? copyField('生成还原计划', `cleanmanager startup restore --operation ${op.operation_id}`) : null);
    const files = op.plan_kind === 'cleanup' || op.plan_kind === 'manual_trash';
    const key = op.needs_reconcile ? 'unknown' : files ? (op.mode === 'quarantine' ? 'in_app_restore' : op.mode === 'trash' ? 'system_recycle_bin' : 'unavailable') : op.plan_kind === 'purge' || op.plan_kind === 'manual_purge' ? 'unavailable' : 'not_applicable';
    const action = key === 'in_app_restore' ? h('button', { type: 'button', class: 'quiet', onclick: () => this.app.openActivity('quarantine') }, '打开隔离区') : null;
    return h('div', { class: 'recovery-line' }, h('h3', {}, '如何恢复'), h('p', { class: 'small' }, AVAILABILITY_LABEL[key as keyof typeof AVAILABILITY_LABEL]), action);
  }

  private itemsPanel(): HTMLElement {
    const seg = h(
      'fieldset',
      { class: 'seg' },
      h('legend', { class: 'sr-only' }, '筛选结果'),
      ...FILTERS.map((f) => {
        const input = h('input', { type: 'radio', name: 'op-filter', id: `op-filter-${f.key}`, value: f.key }) as HTMLInputElement;
        input.checked = this.filter === f.key;
        input.addEventListener('change', () => {
          this.filter = f.key;
          this.userFiltered = true;
          void this.loadItems(false);
        });
        return h('label', { for: `op-filter-${f.key}` }, input, h('span', {}, f.label));
      }),
    );
    const kind = this.op?.plan_kind ?? 'unknown';
    const maintenance = kind === 'maintenance';
    const sizes = !NO_SPACE.has(kind);
    const rows = this.items.map((i) => {
      // MNT-03: a maintenance task reports one of six outcomes; its code and reason come from the task.
      const outcome = maintenance && i.code && OUTCOMES.has(i.code) ? (i.code as Outcome) : null;
      const taskId = maintenance ? i.path_display.split('@')[0] : '';
      const [code, ...rest] = (i.message ?? '').split(': ');
      const reason = outcome ? taskReason(code, rest.join(': ') || (i.message ?? '')) : i.code ? explainCode(i.code, i.message) : null;
      return h(
        'tr',
        {},
        h('td', { class: 'col-result' }, outcome ? outcomeMark(outcome) : statusWord(i.status)),
        h(
          'td',
          { class: 'col-object' },
          h('span', { class: 'obj-name' }, maintenance ? (TASK_WORDS[taskId]?.title ?? i.name) : i.name),
          h('span', { class: 'obj-path path' }, i.original_path_display ? `${i.original_path_display}` : i.path_display),
          reason ? h('span', { class: 'obj-reason' }, reason) : null,
        ),
        sizes ? h('td', { class: 'col-size num' }, formatBytes(i.logical_bytes)) : null,
      );
    });
    return h(
      'section',
      { class: 'panel items-panel', 'aria-labelledby': 'op-items-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'op-items-title' }, '逐项结果'), seg),
      this.items.length
        ? h(
            'div',
            { class: 'table-wrap' },
            h(
              'table',
              { class: 'data-table' },
              h('caption', { class: 'sr-only' }, '每一个计划动作的结果与原因'),
              h('thead', {}, h('tr', {}, h('th', { scope: 'col', class: 'col-result' }, '结果'), h('th', { scope: 'col' }, '对象与原因'), sizes ? h('th', { scope: 'col', class: 'col-size' }, '逻辑大小') : null)),
              h('tbody', {}, ...rows),
            ),
          )
        : h('p', { class: 'muted small', role: 'status' }, this.itemsLoading ? '正在加载…' : this.filter === 'problems' ? '没有未成功的项目。' : '没有符合条件的项目。'),
      this.itemsNext
        ? h('button', { type: 'button', class: 'quiet more', disabled: this.itemsLoading, onclick: () => void this.loadItems(true) }, `显示更多（已显示 ${this.items.length} / ${this.itemsTotal}）`)
        : null,
    );
  }
}

function figure(label: string, value: string, note: string): HTMLElement {
  return h('div', { class: 'figure' }, h('p', { class: 'figure-label' }, label), h('p', { class: 'figure-value num' }, value), h('p', { class: 'figure-note' }, note));
}
