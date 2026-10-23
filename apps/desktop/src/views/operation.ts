// P06 — execution and result detail (GUI-01/06, EXE-*, REC-05). Progress is counted over the
// fixed plan total; "stop" only stops scheduling new actions. Results are per item with the
// reason in words. Processed, quarantined and observed free-space change are reported apart:
// a same-volume quarantine frees ~0 and is never celebrated as space reclaimed. Unknown
// outcomes lead to a read-only reconciliation, never to a retry.

import type { App } from '../main';
import { toIpcError } from '../lib/api';
import { formatBytes, parseBytes } from '../lib/bytes';
import { explainResult, refusalText, resultText } from '../lib/codes';
import { h, icon, keepFocus, replace } from '../lib/dom';
import { OPERATION_TEXT, REGISTRATION_RETRIES, REGISTRATION_RETRY_MS, operationAction, operationBanner, registrationPending, resultIssues } from '../lib/operationModel';
import { formatElapsed, formatTime } from '../lib/format';
import type { IpcError, ItemFilter, Operation, OperationItem } from '../lib/types';
import { AVAILABILITY_LABEL } from '../lib/words';
import { copyField, errorLine, pageHead, pill, segBar, statusWord } from './parts';
import { emptyBlock, issueList, statusBanner } from './ui';
import type { MaintenanceReport } from '../lib/suiteTypes';
import { ELEVATION_WORD, TASK_WORDS } from '../lib/suiteWords';
import { platform, words } from '../lib/platform';
import { outcomeMark, outcomeTally, type Outcome } from './kit';
import { allDone, taskReason } from './maintenance';
import { APP_OUTCOME, leftoversReviewable, outcomeReason } from './uninstallBatch';
import { t } from '../i18n';

/** Kinds whose results are not about disk space (no size figures). */
const NO_SPACE = new Set(['startup', 'startup_restore', 'uninstall', 'uninstall_restore', 'update', 'process', 'maintenance']);
const OUTCOMES = new Set(['applied', 'unchanged', 'attention', 'skipped', 'unavailable', 'failed']);

const ITEM_PAGE = 100;
const FILTERS: { key: ItemFilter; label: string }[] = [
  { key: 'problems', get label() { return t('operation.filters.problems.label'); } },
  { key: 'all', get label() { return t('operation.filters.all.label'); } },
  { key: 'succeeded', get label() { return t('operation.filters.succeeded.label'); } },
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
  /** A reload asked for while a page of items was still loading (the newest read must win). */
  private itemsReload: boolean | null = null;
  /** The last items read failed; cleared by the next read that succeeds (NEW-02: never a stale error). */
  private itemsError: IpcError | null = null;
  private busy = false;
  /** The last action the person started here (reconcile) failed. */
  private actionError: IpcError | null = null;
  private refreshTimer: number | null = null;
  private retryTimer: number | null = null;
  private retries = 0;
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
    // Reopening a result reads it afresh; a failure of an earlier read does not follow it (NEW-02).
    if (id) {
      this.actionError = null;
      void this.load(id);
    }
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
      this.itemsError = null;
      this.actionError = null;
      this.filter = 'problems';
      this.retries = 0;
    }
    let pending = false;
    try {
      const op = await this.app.backend.operationDetail(id);
      if (this.opId !== id) return;
      this.op = op;
      this.error = null;
      this.retries = 0;
      if (op.plan_kind === 'maintenance' && op.report_available && this.maint?.operation_id !== id) {
        const hist = await this.app.backend.maintenanceHistory(50).catch(() => null);
        if (this.opId === id) this.maint = hist?.items.find((r) => r.operation_id === id) ?? null;
      }
      // Everything succeeded: show all rows (an empty "not succeeded" list says nothing about what happened).
      if ((switching || !this.userFiltered) && op.counts.succeeded === op.total && op.total > 0 && !op.running_here) this.filter = 'all';
    } catch (e) {
      if (this.opId !== id) return;
      const err = toIpcError(e);
      // NEW-02: the run this window just started is not registered yet; keep "loading" and read again shortly.
      pending = registrationPending(err, id, this.app.store.state.apply) && this.retries < REGISTRATION_RETRIES;
      this.error = pending ? null : err;
    }
    if (pending) {
      this.retryRegistration(id, keepItems);
      this.update();
      return;
    }
    await this.loadItems(false, keepItems);
  }

  private retryRegistration(id: string, keepItems: boolean): void {
    if (this.retryTimer !== null) return;
    this.retries += 1;
    this.retryTimer = window.setTimeout(() => {
      this.retryTimer = null;
      if (this.opId === id) void this.load(id, keepItems);
    }, REGISTRATION_RETRY_MS);
  }

  private async loadItems(more: boolean, quiet = false): Promise<void> {
    const id = this.opId;
    if (!id) return;
    if (this.itemsLoading) {
      // A refresh while a page is in flight: run it once that page is in, so the last read reflects the end of the run.
      if (!more) this.itemsReload = (this.itemsReload ?? true) && quiet;
      return;
    }
    this.itemsLoading = true;
    if (!quiet) this.update();
    try {
      const want = more ? ITEM_PAGE : Math.max(ITEM_PAGE, quiet ? this.items.length : 0);
      const page = await this.app.backend.operationItems(id, this.filter, Math.min(500, want), more ? this.itemsNext : null);
      if (this.opId !== id) return;
      this.items = more ? [...this.items, ...page.items] : page.items;
      this.itemsNext = page.next_cursor;
      this.itemsTotal = page.total;
      this.itemsError = null;
    } catch (e) {
      if (this.opId !== id) return;
      const err = toIpcError(e);
      // Still registering (see `load`): not an error; the next refresh reads the items.
      this.itemsError = registrationPending(err, id, this.app.store.state.apply) ? null : err;
    } finally {
      this.itemsLoading = false;
      const again = this.itemsReload;
      this.itemsReload = null;
      if (again !== null && this.opId === id) void this.loadItems(false, again);
      else this.update();
    }
  }

  private async stop(): Promise<void> {
    if (!this.opId) return;
    const ok = await this.app.backend.cancelApply(this.opId).catch(() => false);
    const a = this.app.store.state.apply;
    if (ok && a) this.app.store.set({ apply: { ...a, cancelRequested: true } });
    this.app.announce(ok ? t('operation.stopRequestedNoNew') : t('operation.operationAlreadyEnded'));
  }

  private async reconcile(): Promise<void> {
    if (!this.opId || this.busy) return;
    this.busy = true;
    this.actionError = null;
    this.update();
    try {
      const op = await this.app.backend.reconcileOperation(this.opId);
      this.op = op;
      // The end of a run is announced globally (main.ts); a reconciliation says its new outcome here, once.
      const b = operationBanner({ state: op.state, plan_kind: op.plan_kind, mode: op.mode, counts: op.counts, total: op.total, needs_reconcile: op.needs_reconcile, live: false, apps: op.uninstall_apps?.length ?? 0 });
      this.app.announce(t('operation.reconciliationCompleteTitleSentence', { title: b.title, sentence: b.sentence }));
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
    if (from === 'plan') return { label: t('operation.backPlan'), onClick: () => this.app.navigate('plan') };
    if (from === 'uninstall') return { label: t('operation.backUninstall'), onClick: () => this.app.navigate('uninstall') };
    return { label: t('common.backActivity'), onClick: () => this.app.navigate('activity') };
  }

  private render(): void {
    if (!this.opId) {
      replace(this.body, h('h1', { id: 'op-title' }, t('operation.result')), emptyBlock({ title: t('operation.noRunRecordOpen'), body: t('operation.runApprovedPlanSee'), actions: [h('button', { type: 'button', onclick: () => this.app.navigate('activity') }, t('operation.viewActivity'))] }));
      return;
    }
    if (this.error && !this.op) {
      replace(this.body, pageHead({ id: 'op-title', title: t('operation.result'), back: this.back() }), statusBanner({ kind: 'failed', level: 2, titleId: 'op-load-title', title: t('operation.cantReadRunRecord'), sentence: refusalText(this.error) }));
      return;
    }
    const op = this.op;
    if (!op) {
      replace(this.body, pageHead({ id: 'op-title', title: t('operation.result'), back: this.back() }), h('p', { class: 'muted', role: 'status' }, t('operation.loadingRunRecord')));
      return;
    }
    const a = this.app.store.state.apply;
    const live = a && a.operationId === op.operation_id && a.running ? a : null;
    const counts = live ? live.counts : op.counts;
    const total = live ? live.total : op.total;
    const state = live ? (live.cancelRequested ? 'stopping' : 'running') : op.state;
    const banner = operationBanner({ state, plan_kind: op.plan_kind, mode: op.mode, counts, total, needs_reconcile: op.needs_reconcile && !live, live: !!live, apps: op.uninstall_apps?.length ?? 0 });
    replace(
      this.body,
      // The page head keeps the way back and the plan; the outcome banner is the page's heading.
      h('div', { class: 'page-head op-head' }, h('div', { class: 'page-head-text' }, h('button', { type: 'button', class: 'back-link', onclick: this.back().onClick }, icon('back'), h('span', {}, this.back().label))), h('div', { class: 'head-actions' }, h('button', { type: 'button', class: 'quiet', onclick: () => this.app.openPlan(op.plan_id, 'activity') }, t('operation.viewPlan')))),
      statusBanner({
        kind: banner.kind,
        titleId: 'op-title',
        srPrefix: t('operation.resultLabel'),
        title: banner.title,
        sentence: banner.sentence,
        meta: [
          operationAction(op),
          op.started_at ? t('operation.startedStarted', { started_at: formatTime(op.started_at) }) : null,
          op.finished_at ? t('operation.tookStarted', { started_at: formatElapsed(op.started_at, op.finished_at) }) : null,
        ],
        extra: [segBar(total, counts, live ? t('operation.runProgress') : t('operation.result'))],
        actions: this.bannerActions(op, state, !!live),
      }),
      errorLine(this.actionError),
      h('div', { class: 'work-layout' }, h('div', { class: 'work-main' }, this.issuesPanel(op, !!live), this.appsPanel(op, !!live), this.itemsPanel()), this.summaryPanel(op, state)),
    );
  }

  private bannerActions(op: Operation, state: string, live: boolean): HTMLElement[] {
    const out: HTMLElement[] = [];
    if (live && state === 'running') out.push(h('button', { type: 'button', id: 'op-stop', class: 'danger-quiet', onclick: () => void this.stop() }, t('operation.stopRemainingActions')));
    if (op.needs_reconcile && !live) out.push(h('button', { type: 'button', id: 'op-reconcile', class: 'primary', disabled: this.busy, onclick: () => void this.reconcile() }, this.busy ? t('operation.reconciling') : t('operation.reconcileResults')));
    return out;
  }

  /** What did not succeed, grouped by meaning through the shared issue lines (codes in 技术详情). */
  private issuesPanel(op: Operation, live: boolean): HTMLElement | null {
    if (live || op.plan_kind === 'maintenance') return null;
    const issues = resultIssues(this.items);
    if (!issues.length) return null;
    const notOk = op.counts.skipped + op.counts.failed + op.counts.unknown + op.counts.not_started;
    return h(
      'section',
      { class: 'panel gaps-panel op-issues', 'data-tone': 'warn', 'aria-labelledby': 'op-issues-title' },
      h('h2', { id: 'op-issues-title' }, OPERATION_TEXT.issuesTitle, h('span', { class: 'il-count num' }, t('operation.notok', { notOk }))),
      issueList(issues, { label: OPERATION_TEXT.issuesLabel, explain: explainResult, pathMax: 72 }),
      this.itemsNext && this.filter !== 'succeeded' ? h('p', { class: 'muted small' }, t('operation.lengthGroupedReasonHere', { length: this.items.length })) : null,
    );
  }

  /** UNI-11: each application of a batch uninstall with its own result (a single uninstall needs no extra panel). */
  private appsPanel(op: Operation, live: boolean): HTMLElement | null {
    const apps = op.uninstall_apps ?? [];
    if (op.plan_kind !== 'uninstall' || apps.length < 2 || live) return null;
    const done = apps.filter((a) => a.status === 'completed' || a.status === 'partial').length;
    return h(
      'section',
      { class: 'panel', 'aria-labelledby': 'op-apps-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'op-apps-title' }, t('operation.perApp')), h('span', { class: 'small muted num' }, t('operation.doneLengthUninstalled', { length: apps.length, done }))),
      h(
        'ul',
        { class: 'run-list' },
        ...apps.map((a) => {
          const st = APP_OUTCOME[a.status] ?? { label: a.status, tone: 'muted' as const };
          return h(
            'li',
            { class: 'run-row', 'data-app': a.app_instance_id },
            h('span', { class: 'run-main' }, h('b', {}, a.name), h('span', { class: 'muted small' }, outcomeReason(a), a.leftovers_reviewable || a.status === 'pending' ? '' : t('operation.leftoversWontGoStep'))),
            pill(st.label, st.tone),
          );
        }),
      ),
    );
  }

  private summaryPanel(op: Operation, state: string): HTMLElement {
    const processed = parseBytes(op.processed_logical_bytes);
    const quarantined = parseBytes(op.quarantined_logical_bytes);
    const freed = parseBytes(op.estimated_freed_bytes);
    const delta = op.observed_free_delta_bytes === null ? null : BigInt(op.observed_free_delta_bytes);
    const deltaText = delta === null ? t('operation.spaceChangeUnknown') : delta >= 0n ? `+${formatBytes(delta)}` : `−${formatBytes(-delta)}`;
    const quarantine = op.plan_kind === 'cleanup' && op.mode === 'quarantine';
    // GUI-10: when some outcomes are unconfirmed the processed amount is a lower bound, never exact.
    const partial = op.counts.unknown > 0 || op.counts.in_flight > 0;
    const toggles = NO_SPACE.has(op.plan_kind);
    const rows: (HTMLElement | null)[] = [
      toggles
        ? figure(t('operation.processed'), t('operation.succeeded', { succeeded: op.counts.succeeded }), op.plan_kind === 'startup' || op.plan_kind === 'startup_restore' ? t('operation.startupChangesOnlyEdit') : t('operation.kindOperationIsntMeasured'))
        : figure(
            t('operation.processed'),
            processed === null ? '—' : partial ? t('operation.leastProcessed', { processed: formatBytes(processed) }) : formatBytes(processed),
            t('operation.logicalSizeSucceededItems', { succeeded: op.counts.succeeded, p: partial ? t('operation.groupdigits', { groupDigits: op.counts.unknown + op.counts.in_flight }) : '' }),
          ),
      quarantine ? figure(t('operation.stillQuarantine'), quarantined === null ? '—' : formatBytes(quarantined), t('operation.filesStillOccupySame')) : null,
      op.mode === 'delete' || op.plan_kind === 'purge' ? figure(t('operation.estimatedFreed'), freed === null ? '—' : formatBytes(freed), t('operation.estimatedLogicalSizeVolumes')) : null,
      toggles
        ? null
        : figure(
        t('operation.volumeFreeSpaceChange'),
        deltaText,
        delta === null
          ? t('operation.noReliableBeforeAfter')
          : t('operation.differenceBetweenReadingsBefore', { p: op.volume_free_after_bytes ? t('operation.freeAfterRunVolume', { volume_free_after_bytes: formatBytes(op.volume_free_after_bytes) }) : '' }),
      ),
    ];
    const recovery = this.recoveryLine(op);
    const next = this.nextStep(op, state);
    if (state === 'running' || state === 'stopping')
      return h(
        'aside',
        { class: 'panel work-aside op-summary', 'aria-labelledby': 'op-summary-title' },
        h('h2', { id: 'op-summary-title' }, t('operation.running')),
        h('p', { class: 'small' }, t('operation.spaceRecoveryReportedActual')),
        h('p', { class: 'small muted' }, t('operation.switchingPagesDoesntInterrupt')),
        h('dl', { class: 'facts-list small' }, h('dt', {}, t('common.plan')), h('dd', {}, h('code', { class: 'selectable' }, op.plan_id))),
      );
    return h(
      'aside',
      { class: 'panel work-aside op-summary', 'aria-labelledby': 'op-summary-title' },
      h('h2', { id: 'op-summary-title' }, t('operation.spaceRecovery')),
      next,
      ...rows,
      recovery,
      h(
        'dl',
        { class: 'facts-list small' },
        h('dt', {}, t('operation.authorized')),
        h('dd', {}, op.authorization_source === 'interactive_review' ? t('operation.manualReview') : op.authorization_source?.startsWith('policy:') ? t('operation.policySlice', { slice: op.authorization_source.slice(7) }) : (op.authorization_source ?? '—')),
        h('dt', {}, t('common.plan')),
        h('dd', {}, h('code', { class: 'selectable' }, op.plan_id)),
        h('dt', {}, t('operation.run')),
        h('dd', {}, op.running_here ? t('operation.windowClosingFirstStops') : t('operation.endedRunAnotherProcess')),
      ),
    );
  }

  /** What comes after this result: the leftovers review of an uninstall, the six outcomes of a maintenance run. */
  private nextStep(op: Operation, state: string): HTMLElement | null {
    // UNI-11: leftovers are reviewed only for applications whose own uninstaller succeeded.
    const batch = (op.uninstall_apps?.length ?? 0) > 1;
    if (op.plan_kind === 'uninstall' && (state === 'completed' || state === 'partial') && leftoversReviewable(op.uninstall_apps, state)) {
      return h(
        'div',
        { class: 'next-block' },
        h('h3', {}, t('operation.nextReviewLeftovers')),
        h('p', { class: 'small' }, t('operation.uninstallerRunAfterRecheck', { leftoverKinds: words().leftoverKinds })),
        batch ? h('p', { class: 'small muted' }, t('operation.leftoversReviewedPerApp')) : null,
        h('button', { type: 'button', class: 'primary wide', id: 'op-leftovers', onclick: () => this.app.store.set({ leftoversOp: op.operation_id, page: 'uninstall' }) }, t('common.reviewLeftovers')),
      );
    }
    if (op.plan_kind === 'uninstall' && (state === 'failed' || state === 'partial'))
      return h('div', { class: 'next-block' }, h('h3', {}, t('operation.notUninstalled')), h('p', { class: 'small' }, t('operation.appDidntQuitUninstaller')));
    if (op.plan_kind === 'update' && op.counts.skipped > 0)
      return h('div', { class: 'next-block' }, h('h3', {}, t('operation.skippedApps')), h('p', { class: 'small' }, t('operation.runningAppsSkippedClose')), h('button', { type: 'button', class: 'quiet', onclick: () => this.app.navigate('updates') }, t('operation.backUpdates')));
    const m = this.maint;
    if (op.plan_kind === 'maintenance' && m)
      return h(
        'div',
        { class: 'next-block' },
        h('h3', {}, allDone(m) ? t('operation.everyTaskRanNeeded') : t('operation.someTasksNeedFollow')),
        outcomeTally(m.counts, t('operation.sixOutcomes')),
        h('p', { class: 'small muted' }, t('operation.elevationP', { p: ELEVATION_WORD[m.elevation] ?? m.elevation })),
      );
    return null;
  }

  private async restore(op: Operation): Promise<void> {
    this.restoreError = null;
    try {
      const r = op.plan_kind === 'maintenance' ? await this.app.backend.maintenanceRestorePlan(op.operation_id) : await this.app.backend.uninstallRestorePlan(op.operation_id);
      this.app.announce(t('operation.restorePlanCreatedNot'));
      this.app.openPlan(r.plan_id, 'operation');
    } catch (e) {
      this.restoreError = toIpcError(e);
      this.update();
    }
  }

  private recoveryLine(op: Operation): HTMLElement | null {
    if (op.counts.succeeded === 0 && !op.needs_reconcile) return null;
    const plain = (text: string, ...extra: (HTMLElement | null)[]) => h('div', { class: 'recovery-line' }, h('h3', {}, t('operation.howRecover')), h('p', { class: 'small' }, text), ...extra);
    if (!op.needs_reconcile) {
      if (op.plan_kind === 'uninstall') return plain(words().uninstallUndo);
      if (op.plan_kind === 'update') return plain(t('operation.cleanmanagerCantUndoUpdate'));
      if (op.plan_kind === 'process') return plain(t('operation.endingProcessCantUndone'));
      if (op.plan_kind === 'maintenance')
        return platform() === 'macos'
          ? plain(
              t('operation.maintenanceNoGeneralUndo'),
              h('button', { type: 'button', class: 'quiet', id: 'op-restore', onclick: () => void this.restore(op) }, t('operation.createRestorePlan')),
              errorLine(this.restoreError),
            )
          : plain(t('operation.maintenanceNoGeneralUndo2'));
      if (op.plan_kind === 'uninstall_restore') return plain(t('operation.restoreDoesntCreateAnother'));
      // macOS leftovers only ever go to the Trash: Put Back restores them, there is no backup to restore from.
      if (op.plan_kind === 'uninstall_leftovers')
        return words().leftoversUndo
          ? plain(
              t('operation.filesFoldersSystemRecycle', { system_recycle_bin: AVAILABILITY_LABEL.system_recycle_bin, leftoversUndo: words().leftoversUndo }),
              h('button', { type: 'button', class: 'quiet', id: 'op-restore', onclick: () => void this.restore(op) }, t('operation.createRestorePlan')),
              errorLine(this.restoreError),
            )
          : plain(t('operation.filesFoldersSystemRecycle2', { system_recycle_bin: AVAILABILITY_LABEL.system_recycle_bin, restoreFromBin: words().restoreFromBin }));
    }
    if ((op.plan_kind === 'startup' || op.plan_kind === 'startup_restore') && !op.needs_reconcile)
      return h('div', { class: 'recovery-line' }, h('h3', {}, t('operation.howRecover')), h('p', { class: 'small' }, op.plan_kind === 'startup' ? t('operation.eachItemsOriginalState') : t('operation.restoreDoesntCreateAnother')), op.plan_kind === 'startup' ? copyField(t('operation.createRestorePlan'), `cleanmanager startup restore --operation ${op.operation_id}`) : null);
    const files = op.plan_kind === 'cleanup' || op.plan_kind === 'manual_trash';
    const key = op.needs_reconcile ? 'unknown' : files ? (op.mode === 'quarantine' ? 'in_app_restore' : op.mode === 'trash' ? 'system_recycle_bin' : 'unavailable') : op.plan_kind === 'purge' || op.plan_kind === 'manual_purge' ? 'unavailable' : 'not_applicable';
    const action = key === 'in_app_restore' ? h('button', { type: 'button', class: 'quiet', onclick: () => this.app.openActivity('quarantine') }, t('operation.openQuarantine')) : null;
    return h('div', { class: 'recovery-line' }, h('h3', {}, t('operation.howRecover')), h('p', { class: 'small' }, AVAILABILITY_LABEL[key as keyof typeof AVAILABILITY_LABEL]), action);
  }

  private itemsPanel(): HTMLElement {
    const seg = h(
      'fieldset',
      { class: 'seg' },
      h('legend', { class: 'sr-only' }, t('common.filterResults')),
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
      const reason = outcome ? taskReason(code, rest.join(': ') || (i.message ?? '')) : i.code ? resultText(i.code) : null;
      return h(
        'tr',
        {},
        h('td', { class: 'col-result' }, outcome ? outcomeMark(outcome) : statusWord(i.status)),
        h(
          'td',
          { class: 'col-object' },
          h('span', { class: 'obj-name' }, maintenance ? (TASK_WORDS[taskId]?.title ?? i.name) : i.name),
          h('span', { class: 'obj-path path' }, i.original_path_display ? `${i.original_path_display}` : i.path_display),
          // The code and the host's message are in the grouped issue lines' 技术详情 above; here only as a tooltip.
          reason ? h('span', { class: 'obj-reason', title: i.code ? `${i.code}${i.message ? t('operation.message', { message: i.message }) : ''}` : null }, reason) : null,
        ),
        sizes ? h('td', { class: 'col-size num' }, formatBytes(i.logical_bytes)) : null,
      );
    });
    return h(
      'section',
      { class: 'panel items-panel', 'aria-labelledby': 'op-items-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'op-items-title' }, t('operation.itemResults')), seg),
      this.items.length
        ? h(
            'div',
            { class: 'table-wrap' },
            h(
              'table',
              { class: 'data-table' },
              h('caption', { class: 'sr-only' }, t('operation.resultReasonEveryPlanned')),
              h('thead', {}, h('tr', {}, h('th', { scope: 'col', class: 'col-result' }, t('common.result')), h('th', { scope: 'col' }, t('operation.objectReason')), sizes ? h('th', { scope: 'col', class: 'col-size' }, t('common.logicalSize')) : null)),
              h('tbody', {}, ...rows),
            ),
          )
        : this.itemsError
          ? null
          : h('p', { class: 'muted small', role: 'status' }, this.itemsLoading ? t('common.loading') : this.filter === 'problems' ? t('operation.noUnsuccessfulItems') : t('operation.noMatchingItems')),
      // The list could not be read: said here, beside the list it is about, with a way to read it again.
      this.itemsError
        ? h(
            'div',
            { class: 'items-error' },
            errorLine(this.itemsError),
            h('button', { type: 'button', class: 'quiet', id: 'op-items-retry', disabled: this.itemsLoading, onclick: () => void this.loadItems(false) }, t('common.reload')),
          )
        : null,
      this.itemsNext
        ? h('button', { type: 'button', class: 'quiet more', disabled: this.itemsLoading, onclick: () => void this.loadItems(true) }, t('common.showMoreLengthItemstotal', { length: this.items.length, itemsTotal: this.itemsTotal }))
        : null,
    );
  }
}

function figure(label: string, value: string, note: string): HTMLElement {
  return h('div', { class: 'figure' }, h('p', { class: 'figure-label' }, label), h('p', { class: 'figure-value num' }, value), h('p', { class: 'figure-note' }, note));
}
