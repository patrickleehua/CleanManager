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
import { emptyState, errorLine, pageHead, pill, segBar, statusWord } from './parts';

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
      this.items = [];
      this.actionError = null;
      this.filter = 'problems';
    }
    try {
      const op = await this.app.backend.operationDetail(id);
      if (this.opId !== id) return;
      this.op = op;
      this.error = null;
      if (switching && op.counts.succeeded === op.total && op.total > 0) this.filter = 'all';
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
    const action = op.plan_kind === 'cleanup' && op.mode ? MODE_LABEL[op.mode] : op.plan_kind === 'unknown' ? '未知计划' : PLAN_KIND_LABEL[op.plan_kind];
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
    if (state === 'partial') lines.push(h('p', {}, '部分项目被跳过或失败，原因见下表。需要处理剩余对象时，请重新分析并生成新计划；系统不会重做已成功的项目。'));
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
    const rows: (HTMLElement | null)[] = [
      figure('已处理', processed === null ? '—' : formatBytes(processed), `成功项目的逻辑大小，共 ${groupDigits(op.counts.succeeded)} 项`),
      quarantine ? figure('仍在隔离区', quarantined === null ? '—' : formatBytes(quarantined), '这些文件仍占用同一个卷，没有释放空间。') : null,
      op.mode === 'delete' || op.plan_kind === 'purge' ? figure('估计释放', freed === null ? '—' : formatBytes(freed), '按逻辑大小估计；实际以卷空间观测为准。') : null,
      figure(
        '卷可用空间变化',
        deltaText,
        delta === null
          ? '没有可靠的前后读数，保留操作结果，不推测空间变化。'
          : `执行前后的读数差，可能包含其他程序同期的写入。${op.volume_free_after_bytes ? `执行后可用 ${formatBytes(op.volume_free_after_bytes)}。` : ''}`,
      ),
    ];
    const recovery = this.recoveryLine(op);
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

  private recoveryLine(op: Operation): HTMLElement | null {
    if (op.counts.succeeded === 0 && !op.needs_reconcile) return null;
    const key = op.needs_reconcile ? 'unknown' : op.plan_kind === 'cleanup' ? (op.mode === 'quarantine' ? 'in_app_restore' : op.mode === 'trash' ? 'system_recycle_bin' : 'unavailable') : op.plan_kind === 'purge' ? 'unavailable' : 'not_applicable';
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
          void this.loadItems(false);
        });
        return h('label', { for: `op-filter-${f.key}` }, input, h('span', {}, f.label));
      }),
    );
    const rows = this.items.map((i) =>
      h(
        'tr',
        {},
        h('td', { class: 'col-result' }, statusWord(i.status)),
        h('td', { class: 'col-object' }, h('span', { class: 'obj-name' }, i.name), h('span', { class: 'obj-path path' }, i.original_path_display ? `${i.original_path_display}` : i.path_display), i.code ? h('span', { class: 'obj-reason' }, explainCode(i.code, i.message)) : null),
        h('td', { class: 'col-size num' }, formatBytes(i.logical_bytes)),
      ),
    );
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
              h('thead', {}, h('tr', {}, h('th', { scope: 'col', class: 'col-result' }, '结果'), h('th', { scope: 'col' }, '对象与原因'), h('th', { scope: 'col', class: 'col-size' }, '逻辑大小'))),
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
