// P08 activity (operations, plans, scans) and P09 quarantine/recovery (GUI-01/06, REC-01..05).
// States are the core's persisted states, whichever entry (desktop or CLI) started the work.
// Unconfirmed operations lead to reconciliation, never to an automatic retry. Quarantine is
// shown as still occupying its volume; restore and purge are new plans reviewed like any other.

import type { App } from '../main';
import { toIpcError } from '../lib/api';
import { formatBytes, groupDigits } from '../lib/bytes';
import { h, keepFocus, replace } from '../lib/dom';
import { formatTime, STATE_LABEL, stateTone } from '../lib/format';
import type { ActivityTab } from '../lib/store';
import type { HistoryItem, IpcError, PlanRow, QuarantineItem, QuarantinePage } from '../lib/types';
import { ACTION_LABEL, AUTH_LABEL, AVAILABILITY_LABEL, MODE_LABEL, OPERATION_STATE_LABEL, PLAN_KIND_LABEL, QUARANTINE_STATE_LABEL, operationTone } from '../lib/words';
import { emptyState, errorLine, exactTitle, pageHead, pill, tabList } from './parts';
import { words } from '../lib/platform';

const PAGE = 50;

interface Listing<T> {
  items: T[];
  next: string | null;
  loading: boolean;
  error: IpcError | null;
  loaded: boolean;
}

function listing<T>(): Listing<T> {
  return { items: [], next: null, loading: false, error: null, loaded: false };
}

export class ActivityPage {
  el: HTMLElement;
  private body: HTMLElement;
  private ops = listing<HistoryItem>();
  private plans = listing<PlanRow>();
  private quarantine = listing<QuarantineItem>();
  private qMeta: Pick<QuarantinePage, 'held_count' | 'held_logical_bytes' | 'metadata_preserved' | 'metadata_limits'> | null = null;
  private qSelected = new Map<string, QuarantineItem>();
  private creating = false;
  private createError: IpcError | null = null;

  constructor(private app: App) {
    this.body = h('div', { class: 'activity-body' });
    this.el = h('section', { class: 'page page-activity', 'aria-labelledby': 'activity-title' }, this.body);
  }

  activate(): void {
    // Always re-read on entry: other processes (the CLI) may have added records.
    this.ops = listing();
    this.plans = listing();
    this.quarantine = listing();
    this.qSelected.clear();
    void this.loadTab(this.app.store.state.activityTab);
    void this.loadQuarantine(false, true);
  }

  update(): void {
    const tab = this.app.store.state.activityTab;
    const l = tab === 'operations' ? this.ops : tab === 'plans' ? this.plans : tab === 'quarantine' ? this.quarantine : null;
    if (l && !l.loaded && !l.loading) void this.loadTab(tab);
    keepFocus(this.el, () => this.render());
  }

  private async loadTab(tab: ActivityTab, more = false): Promise<void> {
    if (tab === 'operations') await this.load(this.ops, (c) => this.app.backend.listOperations(PAGE, c), more);
    else if (tab === 'plans') await this.load(this.plans, (c) => this.app.backend.listPlans(PAGE, c), more);
    else if (tab === 'quarantine') await this.loadQuarantine(more);
    else this.app.store.set({ recent: await this.app.backend.recentScans().catch(() => this.app.store.state.recent) });
  }

  private async load<T>(l: Listing<T>, fetch: (cursor: string | null) => Promise<{ items: T[]; next_cursor: string | null }>, more: boolean): Promise<void> {
    if (l.loading) return;
    l.loading = true;
    l.error = null;
    this.update();
    try {
      const page = await fetch(more ? l.next : null);
      l.items = more ? [...l.items, ...page.items] : page.items;
      l.next = page.next_cursor;
      l.loaded = true;
    } catch (e) {
      // A failed page never clears what is already listed.
      l.error = toIpcError(e);
    } finally {
      l.loading = false;
      this.update();
    }
  }

  private async loadQuarantine(more: boolean, quiet = false): Promise<void> {
    const l = this.quarantine;
    if (l.loading) return;
    l.loading = true;
    l.error = null;
    if (!quiet) this.update();
    try {
      const page = await this.app.backend.listQuarantine(PAGE, more ? l.next : null);
      l.items = more ? [...l.items, ...page.items] : page.items;
      l.next = page.next_cursor;
      l.loaded = true;
      if (!more) this.qMeta = page;
    } catch (e) {
      l.error = toIpcError(e);
    } finally {
      l.loading = false;
      this.update();
    }
  }

  private async createRecovery(kind: 'restore' | 'purge'): Promise<void> {
    if (!this.qSelected.size || this.creating) return;
    this.creating = true;
    this.createError = null;
    this.update();
    try {
      const r = await this.app.backend.createRecoveryPlan(kind, [...this.qSelected.keys()]);
      this.app.announce(kind === 'restore' ? '恢复计划已生成，尚未执行' : '永久清空计划已生成，尚未执行');
      this.app.openPlan(r.plan_id, 'activity');
    } catch (e) {
      this.createError = toIpcError(e);
    } finally {
      this.creating = false;
      this.update();
    }
  }

  // ---- rendering ---------------------------------------------------------------------------

  private render(): void {
    const s = this.app.store.state;
    const tab = s.activityTab;
    const held = this.qMeta?.held_count;
    const tabs = tabList<ActivityTab>(
      '活动记录类别',
      [
        { key: 'operations', label: '执行' },
        { key: 'plans', label: '计划' },
        { key: 'scans', label: '扫描' },
        { key: 'quarantine', label: '隔离区', count: held ? groupDigits(held) : undefined },
      ],
      tab,
      (k) => this.app.store.set({ activityTab: k }),
    );
    const reconcile = s.caps?.operations_to_reconcile.length
      ? h('div', { class: 'notice warn', role: 'status' }, h('span', {}, `启动时发现 ${s.caps.operations_to_reconcile.length} 个执行在宿主退出时没有完成，结果需要核对。它们标为“需要核对”，不会自动重试。`))
      : null;
    const panel =
      tab === 'operations' ? this.operations() : tab === 'plans' ? this.planList() : tab === 'scans' ? this.scans() : this.quarantinePanel();
    replace(
      this.body,
      pageHead({ id: 'activity-title', title: '活动', meta: ['执行、计划和扫描的持久记录；CLI 与桌面端发起的记录都在这里'] }),
      reconcile,
      tabs,
      h('div', { role: 'tabpanel', id: `panel-${tab}`, 'aria-labelledby': `tab-${tab}`, class: 'tab-panel' }, panel),
    );
  }

  private moreButton<T>(l: Listing<T>, tab: ActivityTab): HTMLElement | null {
    if (!l.next) return null;
    return h('button', { type: 'button', class: 'quiet more', disabled: l.loading, onclick: () => void this.loadTab(tab, true) }, l.loading ? '正在加载…' : '显示更多');
  }

  private operations(): HTMLElement {
    const l = this.ops;
    if (!l.loaded) return h('div', { class: 'panel' }, l.error ? errorLine(l.error) : h('p', { class: 'muted', role: 'status' }, '正在读取执行记录…'));
    if (!l.items.length)
      return h('div', { class: 'panel' }, emptyState('还没有执行记录', '批准并执行一份计划后，每一次执行都会记录在这里，包括逐项结果和恢复方式。', h('button', { type: 'button', onclick: () => this.app.navigate('cleanup') }, '前往清理')));
    return h(
      'div',
      { class: 'panel list-panel' },
      h(
        'ol',
        { class: 'timeline' },
        ...l.items.map((o) => {
          const c = o.counts;
          const counts = c ? `成功 ${c.succeeded}，跳过 ${c.skipped}，失败 ${c.failed}${c.unknown ? `，未知 ${c.unknown}` : ''}${c.not_started ? `，未开始 ${c.not_started}` : ''}` : '结果尚未记录';
          const b = h(
            'button',
            { type: 'button', class: 'tl-row', id: `op-${o.operation_id}` },
            h('span', { class: 'tl-time' }, formatTime(o.started_at)),
            h(
              'span',
              { class: 'tl-main' },
              h('span', { class: 'tl-title' }, ACTION_LABEL[o.action ?? ''] ?? '未知动作', o.processed_logical_bytes && o.processed_logical_bytes !== '0' ? h('span', { class: 'muted num' }, formatBytes(o.processed_logical_bytes)) : null),
              h('span', { class: 'tl-sub' }, counts),
            ),
            h('span', { class: 'tl-side' }, pill(OPERATION_STATE_LABEL[o.state] ?? o.state, operationTone(o.state)), h('span', { class: 'tl-note' }, AVAILABILITY_LABEL[o.availability])),
          );
          b.addEventListener('click', () => this.app.openOperation(o.operation_id, 'activity'));
          return h('li', {}, b);
        }),
      ),
      errorLine(l.error),
      this.moreButton(l, 'operations'),
    );
  }

  private planList(): HTMLElement {
    const l = this.plans;
    if (!l.loaded) return h('div', { class: 'panel' }, l.error ? errorLine(l.error) : h('p', { class: 'muted', role: 'status' }, '正在读取计划…'));
    if (!l.items.length) return h('div', { class: 'panel' }, emptyState('还没有计划', '在清理页或 CLI 生成的计划都会列在这里，可以按编号打开审阅。'));
    return h(
      'div',
      { class: 'panel list-panel' },
      h(
        'ol',
        { class: 'timeline' },
        ...l.items.map((p) => {
          const what = p.kind === 'cleanup' ? (p.mode ? MODE_LABEL[p.mode] : PLAN_KIND_LABEL.cleanup) : PLAN_KIND_LABEL[p.kind];
          const status = p.operation_id ? pill('已执行', 'muted') : p.authorization ? pill(p.expired && p.authorization !== 'authorized' ? AUTH_LABEL.expired : AUTH_LABEL[p.authorization], p.authorization === 'authorized' ? 'good' : p.expired ? 'muted' : 'busy') : pill('无法读取', 'bad');
          const b = h(
            'button',
            { type: 'button', class: 'tl-row', id: `plan-${p.plan_id}` },
            h('span', { class: 'tl-time' }, formatTime(p.created_at)),
            h('span', { class: 'tl-main' }, h('span', { class: 'tl-title' }, what, p.item_count !== null ? h('span', { class: 'muted num' }, `${groupDigits(p.item_count)} 项`) : null), h('code', { class: 'tl-sub' }, p.plan_id)),
            h('span', { class: 'tl-side' }, status, p.problem ? h('span', { class: 'tl-note' }, p.problem) : null),
          );
          b.addEventListener('click', () => this.app.openPlan(p.plan_id, 'activity'));
          return h('li', {}, b);
        }),
      ),
      errorLine(l.error),
      this.moreButton(l, 'plans'),
    );
  }

  private scans(): HTMLElement {
    const recent = this.app.store.state.recent;
    if (!recent.length) return h('div', { class: 'panel' }, emptyState('还没有扫描', '开始一次只读扫描后，它会出现在这里。', h('button', { type: 'button', class: 'primary', onclick: () => this.app.navigate('scope') }, '选择扫描范围')));
    return h(
      'div',
      { class: 'panel list-panel' },
      h(
        'ol',
        { class: 'timeline' },
        ...recent.map((r) => {
          const b = h(
            'button',
            { type: 'button', class: 'tl-row', id: `scan-${r.scan_id}` },
            h('span', { class: 'tl-time' }, formatTime(r.started_at)),
            h('span', { class: 'tl-main' }, h('span', { class: 'tl-title path' }, r.root_display), h('span', { class: 'tl-sub' }, r.host_kind === 'cli' ? '由 CLI 运行' : '由桌面端运行')),
            h('span', { class: 'tl-side' }, r.state ? pill(STATE_LABEL[r.state], stateTone(r.state) === 'good' ? 'good' : stateTone(r.state)) : pill('记录不可用', 'bad')),
          );
          b.addEventListener('click', () => void this.app.openScan(r.scan_id, 'overview'));
          return h('li', {}, b);
        }),
      ),
    );
  }

  private quarantinePanel(): HTMLElement {
    const l = this.quarantine;
    const meta = this.qMeta;
    const head = h(
      'div',
      { class: 'notice warn q-notice', role: 'note' },
      h(
        'span',
        {},
        h('b', {}, '同卷隔离仍占用此卷空间。'),
        meta?.held_count ? ` 当前隔离 ${groupDigits(meta.held_count)} 项，共 ${formatBytes(meta.held_logical_bytes)}。` : ' ',
        '隔离区不是备份：设备损坏、外部删除或权限变化都可能影响恢复。到达提醒时间只会提示你查看，不会自动删除。',
      ),
    );
    if (!l.loaded) return h('div', {}, head, h('div', { class: 'panel' }, l.error ? errorLine(l.error) : h('p', { class: 'muted', role: 'status' }, '正在读取隔离区…')));
    if (!l.items.length)
      return h('div', {}, h('div', { class: 'panel' }, emptyState('隔离区是空的', `选择“移入同卷隔离区”执行的文件会出现在这里，可以创建恢复计划送回原位，或创建永久清空计划释放空间。系统${words().bin}中的内容不会被导入。`)));
    const rows = l.items.map((q) => {
      const check = h('input', { type: 'checkbox', id: `q-${q.quarantine_id}`, disabled: !q.actionable || q.state !== 'recoverable', 'aria-describedby': `q-${q.quarantine_id}-state` }) as HTMLInputElement;
      check.checked = this.qSelected.has(q.quarantine_id);
      check.addEventListener('change', () => {
        if (check.checked) this.qSelected.set(q.quarantine_id, q);
        else this.qSelected.delete(q.quarantine_id);
        this.createError = null;
        this.update();
      });
      const ok = q.state === 'recoverable';
      return h(
        'tr',
        { class: check.checked ? 'selected' : '' },
        h('td', { class: 'col-check' }, check),
        h('td', { class: 'col-object' }, h('label', { for: `q-${q.quarantine_id}`, class: 'obj-name' }, q.name), h('span', { class: 'obj-path path' }, q.original_path_display)),
        h('td', { class: 'col-size num', title: exactTitle(q.logical_bytes) }, formatBytes(q.logical_bytes)),
        h('td', { class: 'col-when' }, formatTime(q.quarantined_at), q.reminder_due ? h('span', { class: 'why-not' }, '已到提醒时间') : null),
        h('td', { class: 'col-status', id: `q-${q.quarantine_id}-state` }, pill(QUARANTINE_STATE_LABEL[q.state] ?? q.state, ok ? 'good' : q.state === 'target_conflict' ? 'warn' : 'bad')),
      );
    });
    return h(
      'div',
      {},
      head,
      h(
        'div',
        { class: 'work-layout' },
        h(
          'div',
          { class: 'work-main' },
          h(
            'section',
            { class: 'panel items-panel', 'aria-label': '隔离项' },
            h(
              'div',
              { class: 'table-wrap' },
              h(
                'table',
                { class: 'data-table' },
                h('caption', { class: 'sr-only' }, '隔离区中的对象，按隔离时间排序'),
                h('thead', {}, h('tr', {}, h('th', { scope: 'col', class: 'col-check' }, h('span', { class: 'sr-only' }, '选择')), h('th', { scope: 'col' }, '原位置'), h('th', { scope: 'col', class: 'col-size' }, '逻辑大小'), h('th', { scope: 'col', class: 'col-when' }, '隔离于'), h('th', { scope: 'col', class: 'col-status' }, '状态'))),
                h('tbody', {}, ...rows),
              ),
            ),
            errorLine(l.error),
            this.moreButton(l, 'quarantine'),
          ),
        ),
        this.quarantineAside(),
      ),
    );
  }

  private quarantineAside(): HTMLElement {
    const n = this.qSelected.size;
    let total = 0n;
    for (const q of this.qSelected.values()) total += BigInt(q.logical_bytes);
    const restore = h('button', { type: 'button', id: 'q-restore', class: 'primary wide', disabled: !n || this.creating }, n ? `创建恢复计划（${n} 项）` : '先选择隔离项');
    restore.addEventListener('click', () => void this.createRecovery('restore'));
    const purge = h('button', { type: 'button', id: 'q-purge', class: 'danger-quiet wide', disabled: !n || this.creating }, n ? `创建永久清空计划（${n} 项）` : '永久清空所选');
    purge.addEventListener('click', () => void this.createRecovery('purge'));
    const meta = this.qMeta;
    return h(
      'aside',
      { class: 'panel work-aside', 'aria-labelledby': 'q-aside-title' },
      h('h2', { id: 'q-aside-title' }, '恢复或清空'),
      n ? h('p', { class: 'draft-figure num' }, formatBytes(total)) : null,
      h('p', { class: 'muted small' }, n ? `已选 ${n} 项。` : '只有状态为“可恢复”的项目可以选择。'),
      restore,
      h('p', { class: 'muted small' }, '恢复把对象移回原文件夹。原位置已有同名对象时跳过该项，从不覆盖，也不改名到别处。'),
      purge,
      h('p', { class: 'muted small' }, '永久清空释放这些对象占用的空间，无法撤销。它需要单独审阅批准，不继承当初的隔离授权。'),
      errorLine(this.createError),
      meta && (meta.metadata_preserved.length || meta.metadata_limits.length)
        ? h(
            'details',
            { class: 'aside-section' },
            h('summary', {}, '恢复会保留什么'),
            meta.metadata_preserved.length ? h('p', { class: 'small' }, `保留：${meta.metadata_preserved.join('、')}`) : null,
            meta.metadata_limits.length ? h('p', { class: 'small muted' }, `限制：${meta.metadata_limits.join('；')}`) : null,
          )
        : null,
    );
  }
}
