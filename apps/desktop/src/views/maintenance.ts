// 优化 › 维护任务 (MNT-01..05/07, PRT-06) and 优化 › 性能诊断 (MNT-06).
// The catalog lists fixed tasks with what problem each solves, its impact, whether it needs an
// administrator and a read-only applicability probe. Nothing is preselected. A search for something
// that is not offered (a registry optimizer, a "speed-up") finds no task, only the reason. A run is
// a maintenance plan reviewed like every other; administrator tasks ask Windows once (UAC) when
// the run starts, never silently, and declining skips only those tasks. Every run is recorded
// with six outcomes per task; there is no undo. The diagnosis only reads.

import type { App } from '../main';
import { formatBytes, groupDigits } from '../lib/bytes';
import { h, icon, keepFocus, replace } from '../lib/dom';
import { formatTime } from '../lib/format';
import { RequestGate } from '../lib/gate';
import { words } from '../lib/platform';
import type { MaintenanceCatalog, MaintenanceDiagnosis, MaintenanceHistory, MaintenanceReport, MaintenanceSearch, MaintenanceTask, Prohibited } from '../lib/suiteTypes';
import { APPLICABILITY, ELEVATION_WORD, FINDING_WORD, PROHIBITED_WORDS, TASK_CODE_WORD, TASK_WORDS } from '../lib/suiteWords';
import type { IpcError } from '../lib/types';
import { LongProgress, ensureReady, noticeLine, outcomeMark, outcomeTally, selectionBar, toIpcError } from './kit';
import { emptyState, errorLine, pageHead, pill } from './parts';

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
        if (!r.value.task_ids.length && r.value.prohibited.length) this.app.announce(`没有这项任务：${prohibitedWords(r.value.prohibited[0]).title}，原因见下方`);
      } catch (e) {
        this.error = toIpcError(e);
      }
      this.update();
    }, 180);
  }

  private async createPlan(): Promise<void> {
    if (this.creating || !this.selected.size) return;
    if (!(await ensureReady(this.app, 'maintenance', '维护任务'))) return;
    this.creating = true;
    this.createError = null;
    this.update();
    try {
      const order = this.catalog?.items.map((t) => t.task_id) ?? [];
      const ids = [...this.selected].sort((a, b) => order.indexOf(a) - order.indexOf(b));
      const r = await this.app.backend.maintenancePlan(ids);
      this.selected.clear();
      this.app.announce('维护计划已生成，尚未执行。请审阅后批准');
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
    const refresh = h('button', { type: 'button', id: 'maint-refresh', disabled: this.loading }, icon('refresh'), this.loading ? '正在探测…' : '重新探测');
    refresh.addEventListener('click', () => void this.load());
    if (c && !c.supported) {
      replace(this.body, pageHead({ id: 'maint-title', title: '维护任务' }), emptyState('此平台暂不提供', '这一版的维护任务目录只包含 Windows 任务。', null));
      return;
    }
    replace(
      this.body,
      pageHead({
        id: 'maint-title',
        title: '维护任务',
        meta: c ? [`${groupDigits(c.items.length)} 项任务`, c.is_admin ? `以${words().admin}运行` : '需要管理员的任务会在执行时请求一次授权', '执行前只读探测'] : ['执行前只读探测'],
        actions: [refresh],
      }),
      errorLine(this.error),
      c
        ? h('div', { class: 'work-layout' }, h('div', { class: 'work-main' }, this.searchPanel(c), this.tasksPanel(c), this.prohibitedPanel(c), this.historyPanel()), this.draft(c))
        : !this.error
          ? h('p', { class: 'muted', role: 'status' }, '正在读取任务目录并探测适用性…')
          : null,
      c ? this.bar() : null,
    );
  }

  private searchPanel(c: MaintenanceCatalog): HTMLElement {
    const input = h('input', { type: 'search', id: 'maint-search', placeholder: '想做什么？例如：DNS、图标、注册表优化', 'aria-label': '搜索维护任务', 'aria-describedby': 'maint-search-help', value: this.query }) as HTMLInputElement;
    input.addEventListener('input', () => this.onSearch(input.value));
    const s = this.search;
    const hits = s ? c.items.filter((t) => s.task_ids.includes(t.task_id)) : [];
    return h(
      'div',
      { class: 'maint-search' },
      input,
      h('p', { id: 'maint-search-help', class: 'sr-only' }, '搜索任务；没有对应任务的操作会显示不提供的原因'),
      s && s.query.trim()
        ? h(
            'div',
            { class: 'search-result', role: 'status' },
            hits.length ? h('p', { class: 'small' }, `匹配的任务：${hits.map((t) => taskTitle(t.task_id, t.title)).join('、')}`) : s.prohibited.length ? null : h('p', { class: 'small' }, '没有匹配的任务。'),
            ...s.prohibited.map((p) => {
              const w = prohibitedWords(p);
              return h('div', { class: 'notice warn prohibited-hit' }, h('span', {}, h('b', {}, `不提供：${w.title}`), h('span', { class: 'block' }, w.reason)));
            }),
          )
        : null,
    );
  }

  private tasksPanel(c: MaintenanceCatalog): HTMLElement {
    const s = this.search;
    const items = s && s.query.trim() ? c.items.filter((t) => s.task_ids.includes(t.task_id)) : c.items;
    const selectable = c.items.filter(taskSelectable);
    const pickAll = h('button', { type: 'button', class: 'quiet', id: 'maint-select-applicable', disabled: !selectable.length }, `选择全部适用的任务（${selectable.length}）`);
    pickAll.addEventListener('click', () => {
      selectable.forEach((t) => this.selected.add(t.task_id));
      this.app.announce(`已选择 ${selectable.length} 项适用的任务`);
      this.update();
    });
    const rows = items.map((t) => {
      const id = `mt-${t.task_id.replace(/[^a-z0-9]+/gi, '-')}`;
      const can = taskSelectable(t);
      const check = h('input', { type: 'checkbox', id: `${id}-check`, disabled: !can, 'aria-label': `选择 ${taskTitle(t.task_id, t.title)}` }) as HTMLInputElement;
      check.checked = this.selected.has(t.task_id);
      check.addEventListener('change', () => {
        if (check.checked) this.selected.add(t.task_id);
        else this.selected.delete(t.task_id);
        this.createError = null;
        this.update();
      });
      const w = TASK_WORDS[t.task_id];
      const ap = t.user_protected ? { label: '受保护', tone: 'muted' as const } : APPLICABILITY[t.applicability];
      return h(
        'tr',
        { class: `crow${check.checked ? ' selected' : ''}${can ? '' : ' locked'}` },
        h('td', { class: 'col-check' }, check),
        h(
          'td',
          { class: 'col-object' },
          h('label', { for: `${id}-check`, class: 'obj-name' }, taskTitle(t.task_id, t.title)),
          h('span', { class: 'obj-reason' }, h('span', { class: 'muted' }, '解决：'), w?.problem ?? t.problem),
          h('span', { class: 'obj-reason' }, h('span', { class: 'muted' }, '影响：'), w?.impact ?? t.impact),
          h('span', { class: 'obj-path' }, `${t.task_id}@${t.task_version} · 约 ${t.estimated_seconds} 秒 · 不可撤销`),
        ),
        h(
          'td',
          { class: 'col-status' },
          pill(ap.label, ap.tone),
          t.requires_admin ? h('span', { class: 'row-tag block-tag' }, '需要管理员') : null,
          h('span', { class: 'small muted block' }, t.user_protected ? '你把它加入了保护，执行时一律跳过' : (t.applicability_reason ?? w?.applies ?? t.applies_when)),
        ),
      );
    });
    return h(
      'section',
      { class: 'panel', 'aria-labelledby': 'maint-tasks-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'maint-tasks-title' }, '任务目录'), pickAll),
      h('p', { class: 'muted small' }, '没有预先勾选任何任务。适用性是刚才的只读探测结果，执行前每项还会再探测一次。'),
      items.length
        ? h(
            'div',
            { class: 'table-wrap' },
            h(
              'table',
              { class: 'data-table ctable maint-table' },
              h('caption', { class: 'sr-only' }, '维护任务、解决的问题、影响与适用性'),
              h('thead', {}, h('tr', {}, h('th', { scope: 'col', class: 'col-check' }, h('span', { class: 'sr-only' }, '选择')), h('th', { scope: 'col' }, '任务'), h('th', { scope: 'col', class: 'col-status' }, '适用性'))),
              h('tbody', {}, ...rows),
            ),
          )
        : h('p', { class: 'small' }, s?.prohibited.length ? `“${s.query.trim()}”没有对应的任务，原因见上方。` : '没有匹配的任务。'),
    );
  }

  private prohibitedPanel(c: MaintenanceCatalog): HTMLElement {
    return h(
      'details',
      { class: 'panel prohibited-panel' },
      h('summary', {}, h('h2', { class: 'inline-h2' }, `不提供的操作（${c.prohibited.length}）`), h('span', { class: 'muted small' }, '这些不是任务，以后也不会成为任务')),
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
      h('div', { class: 'panel-head' }, h('h2', { id: 'maint-history-title' }, '运行记录'), h('span', { class: 'muted small' }, '维护操作没有撤销；每次运行逐项记录结果与原因')),
      runs.length ? h('ol', { class: 'run-history' }, ...runs.map((r) => this.run(r))) : h('p', { class: 'muted small' }, '还没有运行记录。'),
    );
  }

  private run(r: MaintenanceReport): HTMLElement {
    const open = this.openRuns.has(r.operation_id);
    const toggle = h('button', { type: 'button', class: 'link-btn', 'aria-expanded': open ? 'true' : 'false' }, open ? '收起逐项结果' : '逐项结果');
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
        h('span', { class: 'run-main' }, h('b', {}, formatTime(r.started_at)), h('span', { class: 'muted small' }, `${r.entry === 'desktop' ? '桌面' : '命令行'} · ${ELEVATION_WORD[r.elevation] ?? r.elevation}`)),
        allDone(r) ? pill('全部执行或无需改变', 'good') : pill('有需要跟进或没有执行的', 'warn'),
      ),
      outcomeTally(r.counts, '这次运行的结果'),
      toggle,
      open
        ? h(
            'ul',
            { class: 'task-results' },
            ...r.items.map((i) => h('li', {}, outcomeMark(i.outcome), h('span', { class: 'task-result-text' }, h('b', {}, taskTitle(i.task_id, i.task_id)), h('span', { class: 'muted small' }, taskReason(i.code, i.reason))))),
          )
        : null,
    );
  }

  private draft(c: MaintenanceCatalog): HTMLElement {
    const chosen = c.items.filter((t) => this.selected.has(t.task_id));
    const admin = chosen.filter((t) => t.requires_admin).length;
    const create = h('button', { type: 'button', class: 'primary wide', id: 'maint-create', disabled: !chosen.length || this.creating }, this.creating ? '正在冻结计划…' : chosen.length ? `生成维护计划（${chosen.length} 项）` : '先选择任务');
    create.addEventListener('click', () => void this.createPlan());
    return h(
      'aside',
      { class: 'panel work-aside draft', 'aria-labelledby': 'maint-draft-title' },
      h('h2', { id: 'maint-draft-title' }, '运行草稿'),
      chosen.length
        ? h('ul', { class: 'draft-list' }, ...chosen.map((t) => h('li', {}, h('span', {}, taskTitle(t.task_id, t.title)), t.requires_admin ? h('span', { class: 'row-tag' }, '管理员') : null)))
        : h('p', { class: 'muted small' }, '勾选要运行的任务。生成计划只记录这份清单，不运行任何东西。'),
      admin && !c.is_admin
        ? noticeLine(`其中 ${admin} 项需要管理员：执行开始前 Windows 会询问一次，只授权这份计划中的这些任务。拒绝只跳过这些任务，其余照常运行。CleanManager 不会在后台静默提权。`, 'warn')
        : null,
      errorLine(this.createError),
      create,
      h('p', { class: 'muted small aside-note' }, '维护不是“一键加速”：每项任务只说明它解决的问题，并在结束后逐项报告六种结果之一。维护操作没有通用撤销。'),
    );
  }

  private bar(): HTMLElement {
    return selectionBar({
      count: this.selected.size,
      sum: null,
      onClear: () => {
        this.selected.clear();
        this.app.announce('已清空选择');
        this.update();
      },
      action: { id: 'maint-bar-go', text: '生成维护计划并审阅', disabled: this.creating, onClick: () => void this.createPlan() },
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
  private progress = new LongProgress('性能诊断');

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
    this.progress.start('正在两次读取每个进程的处理器时间（间隔 2 秒）以及内存与磁盘读数…');
    this.update();
    try {
      const r = await this.gate.latest(this.app.backend.maintenanceDiagnose());
      if (!r.stale) {
        this.data = r.value;
        this.app.announce(`诊断完成：${r.value.findings.length} 项发现`);
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
    const go = h('button', { type: 'button', class: d ? '' : 'primary', id: 'mdiag-run', disabled: this.running }, this.running ? '正在采样…' : d ? '重新诊断' : '开始诊断');
    go.addEventListener('click', () => void this.run());
    replace(
      this.body,
      pageHead({ id: 'mdiag-title', title: '性能诊断', meta: [d?.sampled_at ? `采样于 ${formatTime(d.sampled_at)}` : '还没有诊断', '只读：不结束进程，不改任何设置'], actions: [go] }),
      this.progress.el,
      errorLine(this.error),
      d && !d.supported ? emptyState('此平台暂不提供', '性能诊断目前只在 Windows 上实现。', null) : null,
      d && d.supported
        ? h('div', { class: 'work-layout' }, h('div', { class: 'work-main' }, this.findings(d), this.top(d)), this.aside(d))
        : !d && !this.running
          ? h(
              'section',
              { class: 'panel catalog-intro', 'aria-labelledby': 'mdiag-intro' },
              h('h2', { id: 'mdiag-intro' }, '电脑变慢时先看看原因'),
              h('p', { class: 'lede' }, '用 2 秒的窗口读取每个进程的处理器占用、内存负载、提交量、磁盘可用空间、启动项数量和仍挂载的磁盘映像，列出值得注意的发现，并指向可以查看的地方。'),
              h('p', { class: 'muted small' }, '诊断只读取，不结束任何进程，也不更改任何设置。'),
            )
          : null,
    );
  }

  private findings(d: MaintenanceDiagnosis): HTMLElement {
    const next = (step: string): HTMLElement | null => {
      const go = (label: string, fn: () => void) => h('button', { type: 'button', class: 'quiet', onclick: fn }, label);
      if (step === 'status.processes') return go('在“进程”中查看', () => this.app.navigate('processes'));
      if (step === 'analyze') return go('去分析磁盘', () => this.app.navigate('overview'));
      if (step === 'startup.list') return go('查看启动项', () => this.app.navigate('startup'));
      if (step === 'status.memory') return go('查看系统状态', () => this.app.navigate('status'));
      return null;
    };
    return h(
      'section',
      { class: 'panel', 'aria-labelledby': 'mdiag-findings' },
      h('h2', { id: 'mdiag-findings' }, `发现（${d.findings.length}）`),
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
                h('span', { class: 'finding-text' }, h('b', {}, w?.title ?? f.code), h('span', { class: 'path small' }, f.subject), h('span', { class: 'muted small' }, f.detail)),
                next(f.next_step),
              );
            }),
          )
        : h('p', { class: 'small' }, '这次采样没有值得注意的发现。'),
    );
  }

  private top(d: MaintenanceDiagnosis): HTMLElement | null {
    if (!d.top_processes.length) return null;
    return h(
      'section',
      { class: 'panel', 'aria-labelledby': 'mdiag-top' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'mdiag-top' }, '采样窗口内最忙的进程'), h('button', { type: 'button', class: 'quiet', onclick: () => this.app.navigate('processes') }, '打开进程列表')),
      h(
        'div',
        { class: 'table-wrap' },
        h(
          'table',
          { class: 'data-table diag-top' },
          h('caption', { class: 'sr-only' }, '处理器占用最高的进程'),
          h('thead', {}, h('tr', {}, h('th', { scope: 'col' }, '进程'), h('th', { scope: 'col', class: 'col-size' }, '处理器（单核 %）'), h('th', { scope: 'col', class: 'col-size' }, '内存'))),
          h(
            'tbody',
            {},
            ...d.top_processes.map((p) =>
              h('tr', {}, h('td', { class: 'col-object' }, h('span', { class: 'obj-name' }, `${p.name}（PID ${p.process_id}）`), p.image ? h('span', { class: 'obj-path path' }, p.image) : null), h('td', { class: 'col-size num' }, `${p.cpu_core_percent.toFixed(1)}%`), h('td', { class: 'col-size num' }, formatBytes(p.working_set_bytes))),
            ),
          ),
        ),
      ),
    );
  }

  private aside(d: MaintenanceDiagnosis): HTMLElement {
    const pct = (v: number | null | undefined) => (v === null || v === undefined ? '不可用' : `${v.toFixed(0)}%`);
    return h(
      'aside',
      { class: 'panel work-aside', 'aria-labelledby': 'mdiag-aside' },
      h('h2', { id: 'mdiag-aside' }, '采样'),
      h('dl', { class: 'facts-list' }, h('dt', {}, '窗口'), h('dd', {}, `${d.window_seconds ?? 2} 秒`), h('dt', {}, '内存负载'), h('dd', { class: 'num' }, pct(d.memory_load_percent)), h('dt', {}, '提交量'), h('dd', { class: 'num' }, pct(d.commit_used_percent))),
      h('p', { class: 'muted small' }, '处理器占用按单个核心计算（4 核满载为 400%）。阈值是本产品的设计值，诊断从不承诺“加速”百分比。'),
      h('p', { class: 'muted small' }, '要结束一个进程，请在“状态 › 进程”中为它生成单独的计划并审阅。'),
    );
  }
}
