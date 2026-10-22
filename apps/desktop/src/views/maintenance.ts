// 优化 › 维护任务 (MNT-01..05/07, PRT-06) and 优化 › 性能诊断 (MNT-06).
// The catalog lists fixed tasks with what problem each solves, its impact, whether it needs an
// administrator and a read-only applicability probe. Nothing is preselected. A search for something
// that is not offered (a registry optimizer, a "speed-up") finds no task, only the reason. A run is
// a maintenance plan reviewed like every other; administrator tasks ask Windows once (UAC) when
// the run starts, never silently, and declining skips only those tasks (macOS: this build has no
// privilege path, so they are skipped `elevation_unavailable` and the page says so up front). Every run is recorded
// with six outcomes per task; there is no undo. The diagnosis only reads.

import type { App } from '../main';
import { formatBytes } from '../lib/bytes';
import { h, icon, keepFocus, replace } from '../lib/dom';
import { formatTime } from '../lib/format';
import { RequestGate } from '../lib/gate';
import { notOnThisPlatform, words } from '../lib/platform';
import type { MaintenanceCatalog, MaintenanceDiagnosis, MaintenanceHistory, MaintenanceReport, MaintenanceSearch, MaintenanceTask, Prohibited } from '../lib/suiteTypes';
import { APPLICABILITY, ELEVATION_WORD, FINDING_WORD, PROHIBITED_WORDS, TASK_CODE_WORD, TASK_WORDS, isRawHostText } from '../lib/suiteWords';
import type { IpcError } from '../lib/types';
import { LongProgress, ensureReady, noticeLine, outcomeMark, outcomeTally, selectionBar, toIpcError } from './kit';
import { emptyState, errorLine, pageHead, pill } from './parts';
import { learnMore, techBlock } from './ui';
import { unknownCodeText } from '../lib/codes';
import { t } from '../i18n';

export function taskTitle(id: string, fallback: string): string {
  return TASK_WORDS[id]?.title ?? fallback;
}

export function prohibitedWords(p: Prohibited): { title: string; reason: string } {
  return PROHIBITED_WORDS[p.id] ?? { title: p.title, reason: p.reason };
}

/** A task can be put into a plan unless the user protected it or it is known not to apply. */
export function taskSelectable(t: MaintenanceTask): boolean {
  return !t.user_protected && (t.applicability === 'applicable' || t.applicability === 'unknown');
}

/** MNT-03: "all done" only when nothing was skipped, unavailable, needs attention or failed (never by omission). */
export function allDone(r: Pick<MaintenanceReport, 'counts'>): boolean {
  const c = r.counts;
  return (c.skipped ?? 0) + (c.unavailable ?? 0) + (c.attention ?? 0) + (c.failed ?? 0) === 0;
}

/** The reason text of a task result, in words when the code is one we know. */
/** A reason in words as a line; a raw host reason (no words yet) only as the tooltip of an empty line. */
function rawAware(text: string, cls: string): HTMLElement {
  return isRawHostText(text) ? h('span', { class: `${cls} sr-only`, title: text }, text) : h('span', { class: cls }, text);
}

export function taskReason(code: string, reason: string): string {
  return TASK_CODE_WORD[code] ?? reason;
}

export class MaintenancePage {
  el: HTMLElement;
  private body: HTMLElement;
  private catalog: MaintenanceCatalog | null = null;
  private history: MaintenanceHistory | null = null;
  private error: IpcError | null = null;
  private loading = false;
  private query = '';
  private search: MaintenanceSearch | null = null;
  private searchGate = new RequestGate();
  private gate = new RequestGate();
  private selected = new Set<string>();
  private creating = false;
  private createError: IpcError | null = null;
  private openRuns = new Set<string>();
  private searchTimer: number | null = null;

  constructor(private app: App) {
    this.body = h('div', { class: 'maintenance-body' });
    this.el = h('section', { class: 'page page-maintenance', 'aria-labelledby': 'maint-title' }, this.body);
  }

  activate(): void {
    void this.load();
  }

  update(): void {
    keepFocus(this.el, () => this.render());
  }

  private async load(): Promise<void> {
    this.loading = true;
    this.error = null;
    this.update();
    try {
      const [c, hist] = await Promise.all([this.gate.latest(this.app.backend.maintenanceCatalog()), this.app.backend.maintenanceHistory(20).catch(() => null)]);
      if (!c.stale) {
        this.catalog = c.value;
        // Selections of tasks that are now protected or not applicable are dropped, never kept silently.
        for (const id of [...this.selected]) {
          const t = c.value.items.find((x) => x.task_id === id);
          if (!t || !taskSelectable(t)) this.selected.delete(id);
        }
      }
      this.history = hist;
    } catch (e) {
      this.error = toIpcError(e);
    } finally {
      this.loading = false;
      this.update();
    }
  }

  private onSearch(text: string): void {
    this.query = text;
    if (this.searchTimer !== null) window.clearTimeout(this.searchTimer);
    if (!text.trim()) {
      this.searchGate.invalidate();
      this.search = null;
      this.update();
      return;
    }
    this.searchTimer = window.setTimeout(async () => {
      try {
        const r = await this.searchGate.latest(this.app.backend.maintenanceSearch(text));
        if (r.stale) return;
        this.search = r.value;
        if (!r.value.task_ids.length && r.value.prohibited.length) this.app.announce(t('maintenance.noTaskTitleSee', { title: prohibitedWords(r.value.prohibited[0]).title }));
      } catch (e) {
        this.error = toIpcError(e);
      }
      this.update();
    }, 180);
  }

  private async createPlan(): Promise<void> {
    if (this.creating || !this.selected.size) return;
    if (!(await ensureReady(this.app, 'maintenance', t('maintenance.maintenanceTasks')))) return;
    this.creating = true;
    this.createError = null;
    this.update();
    try {
      const order = this.catalog?.items.map((t) => t.task_id) ?? [];
      const ids = [...this.selected].sort((a, b) => order.indexOf(a) - order.indexOf(b));
      const r = await this.app.backend.maintenancePlan(ids);
      this.selected.clear();
      this.app.announce(t('maintenance.maintenancePlanCreatedNothing'));
      this.app.openPlan(r.plan_id, 'maintenance');
    } catch (e) {
      this.createError = toIpcError(e);
    } finally {
      this.creating = false;
      this.update();
    }
  }

  // ---- rendering ---------------------------------------------------------------------------

  private render(): void {
    const c = this.catalog;
    const refresh = h('button', { type: 'button', id: 'maint-refresh', disabled: this.loading }, icon('refresh'), this.loading ? t('maintenance.probing') : t('maintenance.probeAgain'));
    refresh.addEventListener('click', () => void this.load());
    if (c && !c.supported) {
      replace(this.body, pageHead({ id: 'maint-title', title: t('maintenance.maintenanceTasks') }), emptyState(t('common.notAvailablePlatformYet'), notOnThisPlatform(t('maintenance.versionMaintenanceTaskCatalog')), null));
      return;
    }
    replace(
      this.body,
      pageHead({
        id: 'maint-title',
        title: t('maintenance.maintenanceTasks'),
        meta: c ? [t('maintenance.taskCount', { length: c.items.length }), c.is_admin ? t('maintenance.runsAdmin', { admin: words().admin }) : words().maintAdminMeta, t('maintenance.readOnlyProbeBefore')] : [t('maintenance.readOnlyProbeBefore')],
        actions: [refresh],
      }),
      errorLine(this.error),
      c
        ? h('div', { class: 'work-layout' }, h('div', { class: 'work-main' }, this.searchPanel(c), this.tasksPanel(c), this.prohibitedPanel(c), this.historyPanel()), this.draft(c))
        : !this.error
          ? h('p', { class: 'muted', role: 'status' }, t('maintenance.readingTaskCatalogProbing'))
          : null,
      c ? this.bar() : null,
    );
  }

  private searchPanel(c: MaintenanceCatalog): HTMLElement {
    const input = h('input', { type: 'search', id: 'maint-search', placeholder: words().maintSearchExample, 'aria-label': t('maintenance.searchMaintenanceTasks'), 'aria-describedby': 'maint-search-help', value: this.query }) as HTMLInputElement;
    input.addEventListener('input', () => this.onSearch(input.value));
    const s = this.search;
    const hits = s ? c.items.filter((t) => s.task_ids.includes(t.task_id)) : [];
    return h(
      'div',
      { class: 'maint-search' },
      input,
      h('p', { id: 'maint-search-help', class: 'sr-only' }, t('maintenance.searchTasksThingsNo')),
      s && s.query.trim()
        ? h(
            'div',
            { class: 'search-result', role: 'status' },
            hits.length ? h('p', { class: 'small' }, t('maintenance.matchingTasksJoin', { join: hits.map((t) => taskTitle(t.task_id, t.title)).join(t('common.listSep')) })) : s.prohibited.length ? null : h('p', { class: 'small' }, t('maintenance.noMatchingTasks')),
            ...s.prohibited.map((p) => {
              const w = prohibitedWords(p);
              return h('div', { class: 'notice warn prohibited-hit' }, h('span', {}, h('b', {}, t('maintenance.notOfferedTitle', { title: w.title })), h('span', { class: 'block' }, w.reason)));
            }),
          )
        : null,
    );
  }

  private tasksPanel(c: MaintenanceCatalog): HTMLElement {
    const s = this.search;
    const items = s && s.query.trim() ? c.items.filter((t) => s.task_ids.includes(t.task_id)) : c.items;
    const selectable = c.items.filter(taskSelectable);
    const pickAll = h('button', { type: 'button', class: 'quiet', id: 'maint-select-applicable', disabled: !selectable.length }, t('maintenance.selectAllApplicableTasks', { length: selectable.length }));
    pickAll.addEventListener('click', () => {
      selectable.forEach((t) => this.selected.add(t.task_id));
      this.app.announce(t('maintenance.selectedLength', { length: selectable.length }));
      this.update();
    });
    const rows = items.map((task) => {
      const id = `mt-${task.task_id.replace(/[^a-z0-9]+/gi, '-')}`;
      const can = taskSelectable(task);
      const check = h('input', { type: 'checkbox', id: `${id}-check`, disabled: !can, 'aria-label': t('maintenance.selectTaskId', { task_id: taskTitle(task.task_id, task.title) }) }) as HTMLInputElement;
      check.checked = this.selected.has(task.task_id);
      check.addEventListener('change', () => {
        if (check.checked) this.selected.add(task.task_id);
        else this.selected.delete(task.task_id);
        this.createError = null;
        this.update();
      });
      const w = TASK_WORDS[task.task_id];
      const ap = task.user_protected ? { label: t('maintenance.tasksPanel.label'), tone: 'muted' as const } : APPLICABILITY[task.applicability];
      return h(
        'tr',
        { class: `crow${check.checked ? ' selected' : ''}${can ? '' : ' locked'}` },
        h('td', { class: 'col-check' }, check),
        h(
          'td',
          { class: 'col-object' },
          h('label', { for: `${id}-check`, class: 'obj-name' }, taskTitle(task.task_id, task.title)),
          h('span', { class: 'obj-reason' }, h('span', { class: 'muted' }, t('maintenance.fixes')), w?.problem ?? task.problem),
          h('span', { class: 'obj-reason' }, h('span', { class: 'muted' }, t('maintenance.impact')), w?.impact ?? task.impact),
          // The task id is a machine fact: a tooltip here, the full id in the plan and the run record.
          h('span', { class: 'obj-path', title: `${task.task_id}@${task.task_version}` }, t('maintenance.aboutEstimatedSeconds', { estimated_seconds: task.estimated_seconds }), h('span', { class: 'meta-sep', 'aria-hidden': 'true' }), task.reversible ? t('maintenance.originalSettingsRestored') : t('maintenance.cantUndone')),
        ),
        h(
          'td',
          { class: 'col-status' },
          pill(ap.label, ap.tone),
          task.requires_admin ? h('span', { class: 'row-tag block-tag' }, t('common.needsAdministrator')) : null,
          // The probe's own sentence is shown only when it is in words; a raw host reason is the tooltip.
          h(
            'span',
            { class: 'small muted block', title: isRawHostText(task.applicability_reason) ? task.applicability_reason : null },
            task.user_protected ? t('maintenance.addedProtectionListSo') : task.applicability_reason && !isRawHostText(task.applicability_reason) ? task.applicability_reason : (w?.applies ?? task.applies_when),
          ),
        ),
      );
    });
    return h(
      'section',
      { class: 'panel', 'aria-labelledby': 'maint-tasks-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'maint-tasks-title' }, t('maintenance.taskCatalog')), pickAll),
      h('p', { class: 'muted small' }, t('maintenance.noTasksPreselectedApplicability')),
      items.length
        ? h(
            'div',
            { class: 'table-wrap' },
            h(
              'table',
              { class: 'data-table ctable maint-table' },
              h('caption', { class: 'sr-only' }, t('maintenance.maintenanceTasksProblemsThey')),
              h('thead', {}, h('tr', {}, h('th', { scope: 'col', class: 'col-check' }, h('span', { class: 'sr-only' }, t('common.select'))), h('th', { scope: 'col' }, t('maintenance.task')), h('th', { scope: 'col', class: 'col-status' }, t('maintenance.applicability')))),
              h('tbody', {}, ...rows),
            ),
          )
        : h('p', { class: 'small' }, s?.prohibited.length ? t('maintenance.noTaskTrimSee', { trim: s.query.trim() }) : t('maintenance.noMatchingTasks')),
    );
  }

  private prohibitedPanel(c: MaintenanceCatalog): HTMLElement {
    return h(
      'details',
      { class: 'panel prohibited-panel' },
      h('summary', {}, h('h2', { class: 'inline-h2' }, t('maintenance.notOfferedLength', { length: c.prohibited.length })), h('span', { class: 'muted small' }, t('maintenance.arentTasksNever'))),
      h(
        'ul',
        { class: 'prohibited-list' },
        ...c.prohibited.map((p) => {
          const w = prohibitedWords(p);
          return h('li', {}, h('b', {}, w.title), h('span', { class: 'muted small' }, w.reason));
        }),
      ),
    );
  }

  private historyPanel(): HTMLElement {
    const runs = this.history?.items ?? [];
    return h(
      'section',
      { class: 'panel', 'aria-labelledby': 'maint-history-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'maint-history-title' }, t('maintenance.runHistory')), h('span', { class: 'muted small' }, t('maintenance.maintenanceCantUndoneEvery'))),
      runs.length ? h('ol', { class: 'run-history' }, ...runs.map((r) => this.run(r))) : h('p', { class: 'muted small' }, t('maintenance.noRunsYet')),
    );
  }

  private run(r: MaintenanceReport): HTMLElement {
    const open = this.openRuns.has(r.operation_id);
    const toggle = h('button', { type: 'button', class: 'link-btn', 'aria-expanded': open ? 'true' : 'false' }, open ? t('maintenance.hidePerTaskResults') : t('maintenance.perTaskResults'));
    toggle.addEventListener('click', () => {
      if (open) this.openRuns.delete(r.operation_id);
      else this.openRuns.add(r.operation_id);
      this.update();
    });
    return h(
      'li',
      { class: 'run-entry' },
      h(
        'div',
        { class: 'run-entry-head' },
        h('span', { class: 'run-main' }, h('b', {}, formatTime(r.started_at)), h('span', { class: 'muted small' }, `${r.entry === 'desktop' ? t('maintenance.desktop') : t('maintenance.commandLine')} · ${ELEVATION_WORD[r.elevation] ?? r.elevation}`)),
        allDone(r) ? pill(t('maintenance.allRanNothingChange'), 'good') : pill(t('maintenance.someNeedFollowUp'), 'warn'),
      ),
      outcomeTally(r.counts, t('maintenance.resultsRun')),
      toggle,
      open
        ? h(
            'ul',
            { class: 'task-results' },
            ...r.items.map((i) => h('li', {}, outcomeMark(i.outcome), h('span', { class: 'task-result-text' }, h('b', {}, taskTitle(i.task_id, i.task_id)), rawAware(taskReason(i.code, i.reason), 'muted small')))),
          )
        : null,
    );
  }

  private draft(c: MaintenanceCatalog): HTMLElement {
    const chosen = c.items.filter((t) => this.selected.has(t.task_id));
    const admin = chosen.filter((t) => t.requires_admin).length;
    const create = h('button', { type: 'button', class: 'primary wide', id: 'maint-create', disabled: !chosen.length || this.creating }, this.creating ? t('common.freezingPlan') : chosen.length ? t('maintenance.createMaintenancePlanLength', { length: chosen.length }) : t('maintenance.selectTasksFirst'));
    create.addEventListener('click', () => void this.createPlan());
    return h(
      'aside',
      { class: 'panel work-aside draft', 'aria-labelledby': 'maint-draft-title' },
      h('h2', { id: 'maint-draft-title' }, t('maintenance.runSelection')),
      chosen.length
        ? h('ul', { class: 'draft-list' }, ...chosen.map((task) => h('li', {}, h('span', {}, taskTitle(task.task_id, task.title)), task.requires_admin ? h('span', { class: 'row-tag' }, t('maintenance.admin')) : null)))
        : h('p', { class: 'muted small' }, t('maintenance.checkTasksRunCreating')),
      admin && !c.is_admin
        ? noticeLine(words().maintAdminDraft(admin), 'warn')
        : null,
      errorLine(this.createError),
      create,
      h('p', { class: 'muted small aside-note' }, t('maintenance.maintenanceNotOneClick')),
    );
  }

  private bar(): HTMLElement {
    return selectionBar({
      count: this.selected.size,
      sum: null,
      onClear: () => {
        this.selected.clear();
        this.app.announce(t('common.selectionCleared'));
        this.update();
      },
      action: { id: 'maint-bar-go', text: t('maintenance.createMaintenancePlanReview'), disabled: this.creating, onClick: () => void this.createPlan() },
    });
  }
}

// ---------------------------------------------------------------------------------------------
// 优化 › 性能诊断 (MNT-06): read-only

export class DiagnosisPage {
  el: HTMLElement;
  private body: HTMLElement;
  private data: MaintenanceDiagnosis | null = null;
  private error: IpcError | null = null;
  private running = false;
  private gate = new RequestGate();
  private progress = new LongProgress(() => t('maintenance.performanceDiagnosis'));

  constructor(private app: App) {
    this.body = h('div', { class: 'diagnosis-body' });
    this.el = h('section', { class: 'page page-diagnosis', 'aria-labelledby': 'mdiag-title' }, this.body);
  }

  update(): void {
    keepFocus(this.el, () => this.render());
  }

  private async run(): Promise<void> {
    if (this.running) return;
    this.running = true;
    this.error = null;
    this.progress.start(t('maintenance.readingEachProcesssCpu'));
    this.update();
    try {
      const r = await this.gate.latest(this.app.backend.maintenanceDiagnose());
      if (!r.stale) {
        this.data = r.value;
        this.app.announce(t('maintenance.diagnosisCompleteLength', { length: r.value.findings.length }));
      }
    } catch (e) {
      this.error = toIpcError(e);
    } finally {
      this.running = false;
      this.progress.stop();
      this.update();
    }
  }

  private render(): void {
    const d = this.data;
    const go = h('button', { type: 'button', class: d ? '' : 'primary', id: 'mdiag-run', disabled: this.running }, this.running ? t('common.sampling') : d ? t('common.diagnoseAgain') : t('maintenance.startDiagnosis'));
    go.addEventListener('click', () => void this.run());
    replace(
      this.body,
      pageHead({ id: 'mdiag-title', title: t('maintenance.performanceDiagnosis'), meta: [d?.sampled_at ? t('common.sampledSampled', { sampled_at: formatTime(d.sampled_at) }) : t('maintenance.noDiagnosisYet'), t('maintenance.readOnlyEndsNo')], actions: [go] }),
      this.progress.el,
      errorLine(this.error),
      d && !d.supported ? emptyState(t('common.notAvailablePlatformYet'), notOnThisPlatform(t('maintenance.performanceDiagnosisCurrentlyOnly')), null) : null,
      d && d.supported
        ? h('div', { class: 'work-layout' }, h('div', { class: 'work-main' }, this.findings(d), this.top(d)), this.aside(d))
        : !d && !this.running
          ? h(
              'section',
              { class: 'panel catalog-intro', 'aria-labelledby': 'mdiag-intro' },
              h('h2', { id: 'mdiag-intro' }, t('maintenance.whenComputerSlowsDown')),
              h('p', { class: 'lede' }, t('maintenance.readsLoadSpaceOver')),
              learnMore(h('p', {}, t('maintenance.readsPerProcessCpu'))),
            )
          : null,
    );
  }

  private findings(d: MaintenanceDiagnosis): HTMLElement {
    const next = (step: string): HTMLElement | null => {
      const go = (label: string, fn: () => void) => h('button', { type: 'button', class: 'quiet', onclick: fn }, label);
      if (step === 'status.processes') return go(t('maintenance.viewProcesses'), () => this.app.navigate('processes'));
      if (step === 'analyze') return go(t('common.analyzeDisk'), () => this.app.navigate('overview'));
      if (step === 'startup.list') return go(t('maintenance.viewStartupItems'), () => this.app.navigate('startup'));
      if (step === 'status.memory') return go(t('maintenance.viewSystemStatus'), () => this.app.navigate('status'));
      return null;
    };
    return h(
      'section',
      { class: 'panel', 'aria-labelledby': 'mdiag-findings' },
      h('h2', { id: 'mdiag-findings' }, t('maintenance.findingsLength', { length: d.findings.length })),
      d.findings.length
        ? h(
            'ul',
            { class: 'finding-list' },
            ...d.findings.map((f) => {
              const w = FINDING_WORD[f.code];
              return h(
                'li',
                { 'data-severity': f.severity },
                h('span', { class: 'finding-mark', 'aria-hidden': 'true' }, icon(f.severity === 'info' ? 'info' : 'warn')),
                h(
                  'span',
                  { class: 'finding-text' },
                  h('b', {}, w?.title ?? unknownCodeText(f.code)),
                  h('span', { class: 'path small' }, f.subject),
                  f.detail && !isRawHostText(f.detail) ? h('span', { class: 'muted small' }, f.detail) : null,
                  f.detail && isRawHostText(f.detail) ? techBlock(h('p', { class: 'selectable small' }, h('code', {}, f.code), ` ${f.detail}`)) : null,
                ),
                next(f.next_step),
              );
            }),
          )
        : h('p', { class: 'small' }, t('maintenance.nothingWorthNotingSample')),
    );
  }

  private top(d: MaintenanceDiagnosis): HTMLElement | null {
    if (!d.top_processes.length) return null;
    return h(
      'section',
      { class: 'panel', 'aria-labelledby': 'mdiag-top' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'mdiag-top' }, t('maintenance.busiestProcessesSampleWindow')), h('button', { type: 'button', class: 'quiet', onclick: () => this.app.navigate('processes') }, t('maintenance.openProcessList'))),
      h(
        'div',
        { class: 'table-wrap' },
        h(
          'table',
          { class: 'data-table diag-top' },
          h('caption', { class: 'sr-only' }, t('maintenance.processesHighestCpuUse')),
          h('thead', {}, h('tr', {}, h('th', { scope: 'col' }, t('common.process')), h('th', { scope: 'col', class: 'col-size' }, t('maintenance.cpuOneCore')), h('th', { scope: 'col', class: 'col-size' }, t('common.memory')))),
          h(
            'tbody',
            {},
            ...d.top_processes.map((p) =>
              h('tr', {}, h('td', { class: 'col-object' }, h('span', { class: 'obj-name' }, t('maintenance.namePidProcessId', { name: p.name, process_id: p.process_id })), p.image ? h('span', { class: 'obj-path path' }, p.image) : null), h('td', { class: 'col-size num' }, `${p.cpu_core_percent.toFixed(1)}%`), h('td', { class: 'col-size num' }, formatBytes(p.working_set_bytes))),
            ),
          ),
        ),
      ),
    );
  }

  private aside(d: MaintenanceDiagnosis): HTMLElement {
    const pct = (v: number | null | undefined) => (v === null || v === undefined ? t('common.unavailable') : `${v.toFixed(0)}%`);
    return h(
      'aside',
      { class: 'panel work-aside', 'aria-labelledby': 'mdiag-aside' },
      h('h2', { id: 'mdiag-aside' }, t('maintenance.sample')),
      h('dl', { class: 'facts-list' }, h('dt', {}, t('maintenance.window')), h('dd', {}, t('maintenance.seconds', { p: d.window_seconds ?? 2 })), h('dt', {}, t('maintenance.memoryLoad')), h('dd', { class: 'num' }, pct(d.memory_load_percent)), h('dt', {}, t('maintenance.commitCharge')), h('dd', { class: 'num' }, pct(d.commit_used_percent))),
      h('p', { class: 'muted small' }, t('maintenance.cpuUseMeasuredPer')),
      h('p', { class: 'muted small' }, t('maintenance.endProcessCreateSeparate')),
    );
  }
}
