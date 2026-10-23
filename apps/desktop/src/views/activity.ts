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
import { ACTION_LABEL, AUTH_LABEL, MODE_LABEL, OPERATION_STATE_LABEL, PLAN_KIND_LABEL, QUARANTINE_STATE_LABEL, historyRecoveryLabel, operationTone } from '../lib/words';
import { emptyState, errorLine, exactTitle, pageHead, pill, tabList } from './parts';
import { words } from '../lib/platform';
import { pathText } from './ui';
import { t } from '../i18n';

/** The page heading per entry (one table for the language switch). */
const ACTIVITY_HEAD: Record<ActivityTab, { title: string; meta: string }> = {
  operations: { get title() { return t('activity.head.operations.title'); }, get meta() { return t('activity.head.operations.meta'); } },
  scans: { get title() { return t('activity.head.scans.title'); }, get meta() { return t('activity.head.scans.meta'); } },
  plans: { get title() { return t('activity.head.plans.title'); }, get meta() { return t('activity.head.plans.meta'); } },
  quarantine: { get title() { return t('activity.head.quarantine.title'); }, get meta() { return t('activity.head.quarantine.meta'); } },
};

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

/**
 * R3-05: the control that takes focus after "显示更多" loaded a page: the first newly listed row (a quarantine row's
 * checkbox, an operation's or plan's row button), so a keyboard user goes on from where the list grew instead of from
 * the top of the page. `null` when nothing new was listed.
 */
export function firstNewRowId(tab: ActivityTab, items: readonly (HistoryItem | PlanRow | QuarantineItem)[], before: number): string | null {
  const row = items[before];
  if (!row) return null;
  if (tab === 'quarantine') return `q-${(row as QuarantineItem).quarantine_id}`;
  if (tab === 'operations') return `op-${(row as HistoryItem).operation_id}`;
  if (tab === 'plans') return `plan-${(row as PlanRow).plan_id}`;
  return null;
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
      this.app.announce(kind === 'restore' ? t('activity.restorePlanCreatedNothing') : t('activity.purgePlanCreatedNothing'));
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
      t('activity.activityRecordTypes'),
      [
        { key: 'operations', label: t('activity.runs') },
        { key: 'plans', label: t('activity.plans') },
        { key: 'scans', label: t('activity.scans') },
        { key: 'quarantine', label: t('activity.quarantine'), count: held ? groupDigits(held) : undefined },
      ],
      tab,
      (k) => this.app.store.set({ activityTab: k }),
    );
    const reconcile = s.caps?.operations_to_reconcile.length
      ? h('div', { class: 'notice warn', role: 'status' }, h('span', {}, t('activity.startupLengthFoundUnfinished', { length: s.caps.operations_to_reconcile.length })))
      : null;
    const panel =
      tab === 'operations' ? this.operations() : tab === 'plans' ? this.planList() : tab === 'scans' ? this.scans() : this.quarantinePanel();
    replace(
      this.body,
      // The heading follows the entry the person used (计划 / 活动 / 恢复 open tabs of this one page).
      pageHead({ id: 'activity-title', title: ACTIVITY_HEAD[tab].title, meta: [ACTIVITY_HEAD[tab].meta] }),
      reconcile,
      tabs,
      h('div', { role: 'tabpanel', id: `panel-${tab}`, 'aria-labelledby': `tab-${tab}`, class: 'tab-panel' }, panel),
    );
  }

  private moreButton<T>(l: Listing<T>, tab: ActivityTab): HTMLElement | null {
    if (!l.next) return null;
    return h('button', { type: 'button', class: 'quiet more', id: `more-${tab}`, disabled: l.loading, onclick: () => void this.showMore(tab) }, l.loading ? t('common.loading') : t('activity.showMore'));
  }

  /** Load the next page and put the focus on its first row (R3-05); the new rows are announced. */
  private async showMore(tab: ActivityTab): Promise<void> {
    const l: Listing<HistoryItem | PlanRow | QuarantineItem> = tab === 'operations' ? this.ops : tab === 'plans' ? this.plans : this.quarantine;
    const before = l.items.length;
    await this.loadTab(tab, true);
    const id = firstNewRowId(tab, l.items, before);
    if (!id) return;
    const el = document.getElementById(id);
    // A row whose checkbox is not available (it cannot be restored now) is focused as a row.
    const target = el instanceof HTMLInputElement && el.disabled ? el.closest('tr') : el;
    if (target instanceof HTMLElement) {
      if (target.tagName === 'TR') target.tabIndex = -1;
      target.focus();
    }
    this.app.announce(t('activity.moreShown', { n: l.items.length - before }));
  }

  private operations(): HTMLElement {
    const l = this.ops;
    if (!l.loaded) return h('div', { class: 'panel' }, l.error ? errorLine(l.error) : h('p', { class: 'muted', role: 'status' }, t('activity.readingRunRecords')));
    if (!l.items.length)
      return h('div', { class: 'panel' }, emptyState(t('activity.noRunsYet'), t('activity.eachTimeApproveRun'), h('button', { type: 'button', onclick: () => this.app.navigate('cleanup') }, t('activity.goClean'))));
    return h(
      'div',
      { class: 'panel list-panel' },
      h(
        'ol',
        { class: 'timeline' },
        ...l.items.map((o) => {
          const c = o.counts;
          const counts = c ? t('activity.succeededSucceededSkippedSkipped', { succeeded: c.succeeded, skipped: c.skipped, failed: c.failed, p: c.unknown ? t('activity.unknownUnknown', { unknown: c.unknown }) : '', p2: c.not_started ? t('activity.notStartedNotStarted', { not_started: c.not_started }) : '' }) : t('activity.noResultsRecordedYet');
          const b = h(
            'button',
            { type: 'button', class: 'tl-row', id: `op-${o.operation_id}` },
            h('span', { class: 'tl-time' }, formatTime(o.started_at)),
            h(
              'span',
              { class: 'tl-main' },
              h('span', { class: 'tl-title' }, ACTION_LABEL[o.action ?? ''] ?? t('activity.unknownAction'), o.processed_logical_bytes && o.processed_logical_bytes !== '0' ? h('span', { class: 'muted num' }, formatBytes(o.processed_logical_bytes)) : null),
              h('span', { class: 'tl-sub' }, counts),
            ),
            h('span', { class: 'tl-side' }, pill(OPERATION_STATE_LABEL[o.state] ?? o.state, operationTone(o.state)), h('span', { class: 'tl-note' }, historyRecoveryLabel(o))),
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
    if (!l.loaded) return h('div', { class: 'panel' }, l.error ? errorLine(l.error) : h('p', { class: 'muted', role: 'status' }, t('activity.readingPlans')));
    if (!l.items.length) return h('div', { class: 'panel' }, emptyState(t('activity.noPlansYet'), t('activity.plansCreatedCleanPage')));
    return h(
      'div',
      { class: 'panel list-panel' },
      h(
        'ol',
        { class: 'timeline' },
        ...l.items.map((p) => {
          const what = p.kind === 'cleanup' ? (p.mode ? MODE_LABEL[p.mode] : PLAN_KIND_LABEL.cleanup) : PLAN_KIND_LABEL[p.kind];
          const status = p.operation_id ? pill(t('activity.executed'), 'muted') : p.authorization ? pill(p.expired && p.authorization !== 'authorized' ? AUTH_LABEL.expired : AUTH_LABEL[p.authorization], p.authorization === 'authorized' ? 'good' : p.expired ? 'muted' : 'busy') : pill(t('activity.unreadable'), 'bad');
          const b = h(
            'button',
            { type: 'button', class: 'tl-row', id: `plan-${p.plan_id}` },
            h('span', { class: 'tl-time' }, formatTime(p.created_at)),
            h('span', { class: 'tl-main' }, h('span', { class: 'tl-title' }, what, p.item_count !== null ? h('span', { class: 'muted num' }, t('activity.itemCount', { item_count: p.item_count })) : null), h('code', { class: 'tl-sub' }, p.plan_id)),
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
    if (!recent.length) return h('div', { class: 'panel' }, emptyState(t('activity.noScansYet'), t('activity.startReadOnlyScan'), h('button', { type: 'button', class: 'primary', onclick: () => this.app.navigate('scope') }, t('common.chooseWhatScan'))));
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
            h('span', { class: 'tl-main' }, h('span', { class: 'tl-title path' }, r.root_display), h('span', { class: 'tl-sub' }, r.host_kind === 'cli' ? t('activity.runCli') : t('activity.runDesktop'))),
            h('span', { class: 'tl-side' }, r.state ? pill(STATE_LABEL[r.state], stateTone(r.state) === 'good' ? 'good' : stateTone(r.state)) : pill(t('common.recordUnavailable'), 'bad')),
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
        h('b', {}, t('activity.sameVolumeQuarantineStill')),
        meta?.held_count ? t('activity.heldCountQuarantineHeld', { held_count: meta.held_count, held_logical_bytes: formatBytes(meta.held_logical_bytes) }) : ' ',
        t('activity.quarantineNotBackupDevice'),
      ),
    );
    if (!l.loaded) return h('div', {}, head, h('div', { class: 'panel' }, l.error ? errorLine(l.error) : h('p', { class: 'muted', role: 'status' }, t('activity.readingQuarantine'))));
    if (!l.items.length)
      return h('div', {}, h('div', { class: 'panel' }, emptyState(t('activity.quarantineEmpty'), t('activity.filesProcessedMoveQuarantine', { bin: words().bin }))));
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
        h('td', { class: 'col-object' }, h('label', { for: `q-${q.quarantine_id}`, class: 'obj-name' }, q.name), pathText(q.original_path_display, { max: 64, className: 'obj-path' })),
        h('td', { class: 'col-size num', title: exactTitle(q.logical_bytes) }, formatBytes(q.logical_bytes)),
        h('td', { class: 'col-when' }, formatTime(q.quarantined_at), q.reminder_due ? h('span', { class: 'why-not' }, t('activity.reminderDateReached')) : null),
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
            { class: 'panel items-panel', 'aria-label': t('activity.quarantinedItems') },
            h(
              'div',
              { class: 'table-wrap' },
              h(
                'table',
                { class: 'data-table' },
                h('caption', { class: 'sr-only' }, t('activity.objectsQuarantineSortedQuarantine')),
                h('thead', {}, h('tr', {}, h('th', { scope: 'col', class: 'col-check' }, h('span', { class: 'sr-only' }, t('common.select'))), h('th', { scope: 'col' }, t('activity.originalLocation')), h('th', { scope: 'col', class: 'col-size' }, t('common.logicalSize')), h('th', { scope: 'col', class: 'col-when' }, t('activity.quarantined')), h('th', { scope: 'col', class: 'col-status' }, t('common.status')))),
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
    const restore = h('button', { type: 'button', id: 'q-restore', class: 'primary wide', disabled: !n || this.creating }, n ? t('activity.createRestorePlanN', { n }) : t('activity.selectQuarantinedItemsFirst'));
    restore.addEventListener('click', () => void this.createRecovery('restore'));
    const purge = h('button', { type: 'button', id: 'q-purge', class: 'danger-quiet wide', disabled: !n || this.creating }, n ? t('activity.createPurgePlanN', { n }) : t('activity.emptySelectedPermanently'));
    purge.addEventListener('click', () => void this.createRecovery('purge'));
    const meta = this.qMeta;
    return h(
      'aside',
      { class: 'panel work-aside', 'aria-labelledby': 'q-aside-title' },
      h('h2', { id: 'q-aside-title' }, t('activity.restoreEmpty')),
      n ? h('p', { class: 'draft-figure num' }, formatBytes(total)) : null,
      h('p', { class: 'muted small' }, n ? t('activity.nSelected', { n }) : t('activity.onlyItemsMarkedRestorable')),
      restore,
      h('p', { class: 'muted small' }, t('activity.restoringMovesObjectsBack')),
      purge,
      h('p', { class: 'muted small' }, t('activity.emptyingPermanentlyFreesSpace')),
      errorLine(this.createError),
      meta && (meta.metadata_preserved.length || meta.metadata_limits.length)
        ? h(
            'details',
            { class: 'aside-section' },
            h('summary', {}, t('activity.whatRestoreKeeps')),
            meta.metadata_preserved.length ? h('p', { class: 'small' }, t('activity.keptJoin', { join: meta.metadata_preserved.join(t('common.listSep')) })) : null,
            meta.metadata_limits.length ? h('p', { class: 'small muted' }, t('activity.limitsJoin', { join: meta.metadata_limits.join(t('common.clauseSep')) })) : null,
          )
        : null,
    );
  }
}
