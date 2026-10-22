// 软件 › 卸载 (UNI-01..10). Two separately reviewed plans (D7):
//  1. the uninstaller: a close request (never a forced end here), then the app's own verified
//     uninstaller, then a rescan; the result is "partial" when something remains;
//  2. its leftovers: grouped, caches and logs preselected, personal data and settings unselected
//     with warnings, kept / blocked / manual objects shown but not selectable, registry keys backed
//     up first, external configuration references as a read-only hint.
// The forced-termination variant (`--end-running`, typed END at apply time) stays in a terminal.
// UNI-11: several applications can be selected into one plan; each keeps its own close request,
// uninstaller, result and leftover section, and one that fails stops nothing else.

import type { App } from '../main';
import { groupDigits } from '../lib/bytes';
import { h, icon, keepFocus, replace } from '../lib/dom';
import { formatTime } from '../lib/format';
import { RequestGate, SubjectGate } from '../lib/gate';
import { notOnThisPlatform, onPlatform, words } from '../lib/platform';
import type { Leftover, Leftovers, UninstallApp, UninstallList } from '../lib/suiteTypes';
import { LEFTOVER_GROUP, MARKER_WORD, SOURCE_WORD, adapterText, leftoverAction, leftoverTier, uninstallRowWords, whyNotWord } from '../lib/suiteWords';
import type { IpcError } from '../lib/types';
import { OPERATION_STATE_LABEL, operationTone } from '../lib/words';
import { LongProgress, atLeast, ensureReady, selectionBar, stageSteps, sumSizes, tierBadge, toIpcError, type Step } from './kit';
import { copyField, emptyState, errorLine, exactTitle, pageHead, pill, sizeText } from './parts';
import { APP_OUTCOME, MAX_BATCH, batchActionText, batchSelectable, batchSelection, leftoverSections, leftoversTitle, type LeftoverSection } from './uninstallBatch';
import { issueList } from './ui';

type Filter = 'all' | 'available' | 'settings' | 'protected';

const FILTERS: { key: Filter; label: string }[] = [
  { key: 'all', label: '全部' },
  { key: 'available', label: '可卸载' },
  { key: 'settings', label: '需在设置中卸载' },
  { key: 'protected', label: '受保护' },
];

export const UNINSTALL_STEPS: Step[] = [
  { title: '卸载程序', note: '先请应用退出，再运行它自己的卸载程序，最后重新检查安装目录和登记。' },
  { title: '残留', note: '卸载后单独审阅确实留下的文件、注册表项和启动项。' },
];

// GUI-12: the two steps keep their titles; what each does follows the platform (Trash and Homebrew on macOS).
onPlatform(() => {
  const w = words();
  UNINSTALL_STEPS[0].note = w.uninstallStep1;
  UNINSTALL_STEPS[1].note = w.uninstallStep2;
  FILTERS[2].label = w.uninstallElsewhereFilter;
});

function matches(a: UninstallApp, f: Filter, q: string): boolean {
  const inFilter = f === 'all' || (f === 'available' && a.state === 'available') || (f === 'settings' && a.state === 'unverified') || (f === 'protected' && a.state === 'unsupported');
  const text = `${a.name} ${a.publisher ?? ''} ${a.version ?? ''}`.toLowerCase();
  return inFilter && (!q || text.includes(q));
}

export class UninstallPage {
  el: HTMLElement;
  private body: HTMLElement;
  private list: UninstallList | null = null;
  private listError: IpcError | null = null;
  private loading = false;
  private filter: Filter = 'all';
  private query = '';
  /** UNI-11: applications chosen for one uninstall plan. */
  private chosen = new Set<string>();
  private planning = false;
  private batchError: IpcError | null = null;
  private rowError = new Map<string, IpcError>();
  private gate = new RequestGate();
  // Leftovers review of one operation
  private leftovers: Leftovers | null = null;
  private loError: IpcError | null = null;
  private loGate = new SubjectGate<string>();
  private selected = new Map<string, Leftover>();
  private collapsed = new Set<string>();
  private creating = false;
  private createError: IpcError | null = null;
  private progress = new LongProgress('读取残留');

  constructor(private app: App) {
    this.body = h('div', { class: 'uninstall-body' });
    this.el = h('section', { class: 'page page-uninstall', 'aria-labelledby': 'uninstall-title' }, this.body);
  }

  activate(): void {
    const op = this.app.store.state.leftoversOp;
    if (op) void this.loadLeftovers(op);
    else if (!this.list) void this.load();
  }

  update(): void {
    const op = this.app.store.state.leftoversOp;
    if (op && op !== this.loGate.current) void this.loadLeftovers(op);
    keepFocus(this.el, () => this.render());
  }

  // ---- data --------------------------------------------------------------------------------

  private async load(): Promise<void> {
    this.loading = true;
    this.listError = null;
    this.update();
    try {
      const r = await this.gate.latest(this.app.backend.uninstallList());
      if (!r.stale) this.list = r.value;
    } catch (e) {
      this.listError = toIpcError(e);
    } finally {
      this.loading = false;
      this.update();
    }
  }

  private toggleApp(a: UninstallApp): void {
    if (this.chosen.has(a.app_instance_id)) this.chosen.delete(a.app_instance_id);
    else if (batchSelectable(a) && this.chosen.size < MAX_BATCH) this.chosen.add(a.app_instance_id);
    this.batchError = null;
    this.app.announce(this.chosen.has(a.app_instance_id) ? `已选择 ${a.name}，共 ${this.chosen.size} 个应用` : `已取消选择 ${a.name}，共 ${this.chosen.size} 个应用`);
    this.update();
  }

  /** One plan for every selected application (UNI-11); refused ones are named on their rows and left out. */
  private async planSelected(): Promise<void> {
    const apps = batchSelection(this.list?.items ?? [], this.chosen);
    if (this.planning || !apps.length) return;
    const what = apps.length === 1 ? `卸载 ${apps[0].name}` : `卸载 ${apps.length} 个应用`;
    if (!(await ensureReady(this.app, 'uninstall', what))) return;
    this.planning = true;
    this.batchError = null;
    for (const a of apps) this.rowError.delete(a.app_instance_id);
    this.update();
    try {
      const r = await this.app.backend.uninstallPlan(apps.map((a) => a.app_instance_id));
      for (const x of r.refused) this.rowError.set(x.app_instance_id, { code: x.reason, reason: x.reason, message: x.message });
      this.chosen.clear();
      const names = r.apps.map((a) => a.name).join('、');
      this.app.announce(`已生成卸载计划（${r.apps.length} 个应用：${names}），尚未执行。${r.refused.length ? `另有 ${r.refused.length} 个应用不能卸载，未放入计划。` : ''}请审阅后批准`);
      this.app.openPlan(r.plan_id, 'uninstall');
    } catch (e) {
      // One application: the reason belongs on its row; several: every one was refused, the reasons are in the message.
      if (apps.length === 1) this.rowError.set(apps[0].app_instance_id, toIpcError(e));
      else this.batchError = toIpcError(e);
    } finally {
      this.planning = false;
      this.update();
    }
  }

  private openLeftovers(operationId: string): void {
    this.app.store.set({ leftoversOp: operationId });
  }

  private closeLeftovers(): void {
    this.loGate.show('');
    this.leftovers = null;
    this.app.store.set({ leftoversOp: null });
    if (!this.list) void this.load();
  }

  private async loadLeftovers(op: string): Promise<void> {
    this.leftovers = null;
    this.loError = null;
    this.selected.clear();
    this.createError = null;
    this.progress.start('正在重新检查卸载后留下的对象…');
    this.loGate.show(op);
    this.render();
    try {
      const r = await this.loGate.load(op, this.app.backend.uninstallLeftovers(op));
      if (r.stale) return;
      this.leftovers = r.value;
      // UNI-05: only what the core preselected (caches and logs) starts selected; the person may clear it.
      for (const g of r.value.groups) for (const i of g.items) if (i.selected && i.selectable) this.selected.set(i.object_id, i);
      this.app.announce(`找到 ${groupDigits(r.value.groups.reduce((n, g) => n + g.items.length, 0))} 个残留对象，已选中缓存与日志 ${this.selected.size} 项`);
    } catch (e) {
      if (this.loGate.current === op) this.loError = toIpcError(e);
    } finally {
      if (this.loGate.current === op) this.progress.stop();
      this.update();
    }
  }

  private toggle(i: Leftover): void {
    if (this.selected.has(i.object_id)) this.selected.delete(i.object_id);
    else if (i.selectable) this.selected.set(i.object_id, i);
    this.createError = null;
    this.app.announce(this.selected.has(i.object_id) ? `已选择 ${tail(i.display)}` : `已取消选择 ${tail(i.display)}`);
    this.update();
  }

  private async createLeftoversPlan(): Promise<void> {
    const l = this.leftovers;
    if (!l || this.creating || !this.selected.size) return;
    if (!(await ensureReady(this.app, 'uninstall', '处理卸载残留'))) return;
    this.creating = true;
    this.createError = null;
    this.update();
    try {
      const r = await this.app.backend.uninstallLeftoversPlan(l.operation_id, [...this.selected.keys()]);
      this.app.announce('残留计划已生成，尚未执行。请审阅完整清单');
      this.app.openPlan(r.plan_id, 'uninstall');
    } catch (e) {
      this.createError = toIpcError(e);
    } finally {
      this.creating = false;
      this.update();
    }
  }

  // ---- rendering ---------------------------------------------------------------------------

  private render(): void {
    if (this.app.store.state.leftoversOp) this.renderLeftovers();
    else this.renderList();
  }

  private renderList(): void {
    const d = this.list;
    const refresh = h('button', { type: 'button', id: 'uninstall-refresh', disabled: this.loading }, icon('refresh'), this.loading ? '正在读取…' : '重新读取');
    refresh.addEventListener('click', () => void this.load());
    if (d && !d.supported) {
      replace(this.body, pageHead({ id: 'uninstall-title', title: '卸载' }), emptyState('此平台暂不提供', notOnThisPlatform('应用卸载目前只在 Windows 上实现。'), null));
      return;
    }
    const available = d?.items.filter((a) => a.state === 'available').length ?? 0;
    replace(
      this.body,
      pageHead({
        id: 'uninstall-title',
        title: '卸载',
        meta: d ? [`${groupDigits(d.items.length)} 个应用`, `可卸载 ${groupDigits(available)} 个`, d.is_admin ? `以${words().admin}运行` : '以当前用户运行', '只读清单'] : ['只读清单'],
        actions: [refresh],
      }),
      stageSteps('卸载分两步，每步单独审阅', UNINSTALL_STEPS, 0),
      errorLine(this.listError),
      d
        ? h('div', { class: 'work-layout' }, h('div', { class: 'work-main' }, this.recentPanel(d), this.appsPanel(d)), this.howPanel())
        : !this.listError
          ? h('p', { class: 'muted', role: 'status' }, '正在读取已安装的应用…')
          : null,
      d ? this.appsBar(d) : null,
    );
  }

  private appsBar(d: UninstallList): HTMLElement {
    const n = batchSelection(d.items, this.chosen).length;
    return selectionBar({
      count: n,
      sum: null,
      onClear: () => {
        this.chosen.clear();
        this.batchError = null;
        this.app.announce('已清空选择');
        this.update();
      },
      action: { id: 'uninstall-bar-go', text: this.planning ? '正在核验…' : batchActionText(n), disabled: this.planning, onClick: () => void this.planSelected() },
      note: n > 1 ? '每个应用单独退出、单独卸载、单独报告结果；一个失败不影响其他' : null,
    });
  }

  private appsPanel(d: UninstallList): HTMLElement {
    const search = h('input', { type: 'search', id: 'uninstall-search', placeholder: '按名称或发布者查找', 'aria-label': '查找应用', value: this.query }) as HTMLInputElement;
    search.addEventListener('input', () => {
      this.query = search.value;
      this.update();
    });
    const seg = h(
      'fieldset',
      { class: 'seg' },
      h('legend', { class: 'sr-only' }, '筛选应用'),
      ...FILTERS.map((f) => {
        const input = h('input', { type: 'radio', name: 'uninstall-filter', id: `uninstall-filter-${f.key}`, value: f.key }) as HTMLInputElement;
        input.checked = this.filter === f.key;
        input.addEventListener('change', () => {
          this.filter = f.key;
          this.update();
        });
        return h('label', { for: `uninstall-filter-${f.key}` }, input, h('span', {}, f.label));
      }),
    );
    const q = this.query.trim().toLowerCase();
    const items = d.items.filter((a) => matches(a, this.filter, q));
    const n = batchSelection(d.items, this.chosen).length;
    return h(
      'section',
      { class: 'panel', 'aria-labelledby': 'uninstall-apps-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'uninstall-apps-title' }, '已安装的应用'), h('div', { class: 'list-tools' }, seg, search)),
      h(
        'p',
        { class: 'muted small' },
        n ? `已选 ${n} 个应用。` : '',
        `勾选一个或多个可卸载的应用，生成一份卸载计划（最多 ${MAX_BATCH} 个）。每个应用单独审阅、单独执行；受保护的应用不能勾选。`,
      ),
      errorLine(this.batchError),
      items.length ? this.appsTable(items) : h('p', { class: 'muted small' }, '没有符合条件的应用。'),
      d.issues.length
        ? h(
            'details',
            { class: 'issues-details' },
            h('summary', {}, `未能读取的登记（${d.issues.length}）——清单可能不完整`),
            issueList(d.issues, { label: '记录的问题' }),
          )
        : null,
    );
  }

  private appsTable(items: UninstallApp[]): HTMLElement {
    const rows = items.map((a) => {
      const id = `un-${a.app_instance_id}`;
      const st = uninstallRowWords(a);
      const err = this.rowError.get(a.app_instance_id);
      const can = batchSelectable(a);
      const check = h('input', {
        type: 'checkbox',
        id: `${id}-check`,
        disabled: !can || this.planning || (!this.chosen.has(a.app_instance_id) && this.chosen.size >= MAX_BATCH),
        'aria-label': `选择 ${a.name}`,
      }) as HTMLInputElement;
      check.checked = can && this.chosen.has(a.app_instance_id);
      check.addEventListener('change', () => this.toggleApp(a));
      let action: HTMLElement;
      if (a.state === 'available') {
        action = h('span', { class: 'muted small' }, check.checked ? (this.planning ? '正在核验…' : '已选择') : '勾选后卸载');
      } else if (st.settingsButton) {
        const btn = h('button', { type: 'button', class: 'quiet', id: `${id}-settings` }, icon('external'), '在设置中卸载');
        btn.addEventListener('click', () => void this.app.backend.openSystemPage('apps_features'));
        action = btn;
      } else if (st.guidance) {
        action = h('span', { class: 'muted small' }, st.tone === 'warn' && st.label === '需要管理员' ? '在启动台或访达中删除' : '在访达中处理');
      } else {
        action = h('span', { class: 'muted small' }, '没有卸载入口');
      }
      const facts = [a.publisher ?? '发布者未知', a.version ? `版本 ${a.version}` : null, SOURCE_WORD[a.source] ?? a.source, a.last_used_at ? `上次使用 ${formatTime(a.last_used_at)}` : null].filter(Boolean).join(' · ');
      return h(
        'tr',
        { class: `crow${check.checked ? ' selected' : ''}${can ? '' : ' locked'}` },
        h('td', { class: 'col-check' }, check),
        h(
          'td',
          { class: 'col-object' },
          can ? h('label', { for: `${id}-check`, class: 'obj-name' }, a.name) : h('span', { class: 'obj-name' }, a.name),
          h('span', { class: 'obj-path' }, facts),
          a.adapter ? h('span', { class: 'obj-reason muted' }, '方式：', adapterText(a.adapter)) : null,
          st.reason ? h('span', { class: a.state === 'unsupported' ? 'obj-reason muted' : 'why-not' }, st.reason) : null,
          st.guidance ? h('span', { class: 'obj-reason' }, st.guidance) : null,
          err ? h('span', { class: 'form-error', role: 'alert' }, err.message) : null,
        ),
        h('td', { class: 'col-flags' }, pill(st.label, st.tone)),
        h('td', { class: 'col-status' }, action),
      );
    });
    return h(
      'div',
      { class: 'table-wrap' },
      h(
        'table',
        { class: 'data-table uninstall-table' },
        h('caption', { class: 'sr-only' }, '已安装的应用、卸载方式与可用操作；勾选可卸载的应用放入同一份计划'),
        h('thead', {}, h('tr', {}, h('th', { scope: 'col', class: 'col-check' }, h('span', { class: 'sr-only' }, '选择')), h('th', { scope: 'col' }, '应用与卸载方式'), h('th', { scope: 'col', class: 'col-flags' }, '状态'), h('th', { scope: 'col', class: 'col-status' }, '操作'))),
        h('tbody', {}, ...rows),
      ),
    );
  }

  private recentPanel(d: UninstallList): HTMLElement | null {
    if (!d.recent.length) return null;
    const rows = d.recent.map((r) => {
      // UNI-11: leftovers exist only for applications whose own uninstaller succeeded.
      const reviewable = r.stage === 'uninstall' && r.leftovers_reviewable;
      const go = reviewable
        ? h('button', { type: 'button', class: 'quiet', onclick: () => this.openLeftovers(r.operation_id) }, '审阅残留')
        : h('button', { type: 'button', class: 'quiet', onclick: () => this.app.openOperation(r.operation_id, 'uninstall') }, '查看结果');
      return h(
        'li',
        { class: 'run-row' },
        h('span', { class: 'run-main' }, h('b', {}, r.app_name), h('span', { class: 'muted small' }, `${r.stage === 'uninstall' ? `第 1 步 · 卸载程序${r.app_count > 1 ? `（${r.app_count} 个应用）` : ''}` : '第 2 步 · 残留'} · ${formatTime(r.started_at)}`)),
        pill(OPERATION_STATE_LABEL[r.state] ?? r.state, operationTone(r.state)),
        go,
      );
    });
    return h('section', { class: 'panel', 'aria-labelledby': 'uninstall-recent-title' }, h('h2', { id: 'uninstall-recent-title' }, '最近的卸载'), h('ul', { class: 'run-list' }, ...rows));
  }

  private howPanel(): HTMLElement {
    return h(
      'aside',
      { class: 'panel work-aside', 'aria-labelledby': 'uninstall-how' },
      h('h2', { id: 'uninstall-how' }, '卸载怎样进行'),
      h('p', { class: 'small' }, words().uninstallHowReview),
      h('p', { class: 'small' }, words().uninstallHowRescan),
      h('p', { class: 'small' }, '一次选择多个应用时，它们在同一份计划中依次执行：每个应用单独退出、单独卸载、单独报告结果。一个应用没有退出或卸载失败，不影响其他应用；它的残留也不会进入第 2 步。'),
      h(
        'div',
        { class: 'aside-section' },
        h('h3', {}, '应用还在运行时'),
        h('p', { class: 'small' }, words().uninstallHowRunning),
        h('p', { class: 'muted small' }, '确实需要结束仍在运行的进程时，只能在终端中创建一份单独审阅的计划，执行时还要输入 END 确认：'),
        copyField('在终端中（需要人工确认）', 'cleanmanager apps uninstall plan create --instance <应用编号> --end-running'),
      ),
      h(
        'div',
        { class: 'aside-section' },
        h('h3', {}, '没有卸载入口的应用'),
        h('p', { class: 'small' }, `安全软件、系统组件、共享运行库和 CleanManager 自己不能在这里卸载。${words().uninstallManually}`),
      ),
    );
  }

  // ---- leftovers -----------------------------------------------------------------------------

  private renderLeftovers(): void {
    const l = this.leftovers;
    const back = { label: '返回卸载', onClick: () => this.closeLeftovers() };
    const all = l ? l.groups.flatMap((g) => g.items) : [];
    // UNI-11: a batch is reviewed application by application; a single uninstall keeps one flat review.
    const sections = l && l.apps.length > 1 ? leftoverSections(l) : null;
    const groupsOf = (appId: string) => l?.apps.find((a) => a.app.app_instance_id === appId)?.groups ?? [];
    replace(
      this.body,
      pageHead({
        id: 'uninstall-title',
        title: leftoversTitle(l),
        back,
        meta: l
          ? [h('code', { class: 'selectable' }, l.operation_id), `${groupDigits(all.length)} 个对象`, `默认选中 ${groupDigits(l.default_selected)} 项（缓存与日志）`, l.is_admin ? `以${words().admin}运行` : '以当前用户运行']
          : [],
      }),
      stageSteps('卸载分两步，每步单独审阅', UNINSTALL_STEPS, 1),
      this.progress.el,
      errorLine(this.loError),
      l
        ? all.length
          ? h(
              'div',
              { class: 'work-layout' },
              h(
                'div',
                { class: 'work-main' },
                this.loSummary(l),
                ...(sections
                  ? sections.flatMap((sec, ai) => [this.loAppHead(sec, ai), ...groupsOf(sec.app_instance_id).map((g, gi) => this.loGroup(g.group, g.items, gi, `a${ai}`, sec.name))])
                  : l.groups.map((g, gi) => this.loGroup(g.group, g.items, gi))),
                this.refsPanel(l),
              ),
              this.loDraft(l),
            )
          : h('div', { class: 'panel' }, emptyState('没有残留', `重新检查没有找到与这个应用相关的${words().leftoverKindsOr}。`, h('button', { type: 'button', onclick: back.onClick }, back.label)))
        : null,
      l ? this.loBar() : null,
    );
  }

  private loSummary(l: Leftovers): HTMLElement {
    const all = l.groups.flatMap((g) => g.items);
    const count = (f: (i: Leftover) => boolean) => all.filter(f).length;
    const keys = count((i) => i.action === 'registry_backup_delete');
    return h(
      'section',
      { class: 'panel catalog-summary', 'aria-labelledby': 'lo-sum' },
      h('h2', { id: 'lo-sum', class: 'sr-only' }, '摘要'),
      h(
        'dl',
        { class: 'tier-summary' },
        h('div', {}, h('dt', {}, tierBadge('default_selected')), h('dd', { class: 'num' }, `${groupDigits(count((i) => i.tier === 'default_selected'))} 项`, h('small', {}, '缓存与日志，默认选中'))),
        h('div', {}, h('dt', {}, tierBadge('review_only')), h('dd', { class: 'num' }, `${groupDigits(count((i) => i.tier === 'review_only'))} 项`, h('small', {}, '设置、个人数据等，逐项确认'))),
        h('div', {}, h('dt', {}, tierBadge('blocked')), h('dd', { class: 'num' }, `${groupDigits(count((i) => !i.selectable))} 项`, h('small', {}, '保留、已阻止或需手动处理'))),
      ),
      keys ? h('p', { class: 'muted small lo-keys' }, `其中 ${keys} 个注册表项：执行时先把整棵键备份到 CleanManager 的记录中，备份失败就不删除；之后可以生成还原计划。`) : null,
    );
  }

  /** UNI-11: one application's heading in a batch review: its own result and whether its leftovers are reviewed. */
  private loAppHead(sec: LeftoverSection, ai: number): HTMLElement {
    const st = APP_OUTCOME[sec.status] ?? { label: sec.status, tone: 'muted' as const };
    return h(
      'section',
      { class: 'panel', 'aria-labelledby': `lo-app${ai}`, 'data-app': sec.app_instance_id },
      h(
        'div',
        { class: 'panel-head' },
        h('h2', { id: `lo-app${ai}` }, sec.name),
        h('span', {}, pill(st.label, st.tone), ' ', h('span', { class: 'small muted num' }, sec.reviewable ? `${groupDigits(sec.objects)} 个残留对象` : '全部保留')),
      ),
      h('p', { class: sec.reviewable ? 'small muted' : 'small' }, sec.reason),
    );
  }

  private loGroup(group: string, items: Leftover[], gi: number, scope = '', appName = ''): HTMLElement {
    const id = `lo-${scope}g${gi}`;
    const key = scope ? `${scope}:${group}` : group;
    const open = !this.collapsed.has(key);
    const meta = LEFTOVER_GROUP[group] ?? { title: group, explain: '' };
    // Registry keys, startup entries and services have no size; only files and folders count.
    const files = items.filter((i) => i.action === 'fs_trash' || (i.action === 'manual' && i.kind !== 'service' && i.kind !== 'registry_key'));
    const sum = sumSizes(files.map((i) => ({ bytes: i.logical_bytes })));
    const toggle = h(
      'button',
      { type: 'button', class: 'group-toggle', id: `${id}-toggle`, 'aria-expanded': open ? 'true' : 'false', 'aria-controls': `${id}-body` },
      h('span', { class: 'chev-icon', 'aria-hidden': 'true' }, icon('chevron')),
      h('span', { class: 'group-title' }, meta.title),
      h('span', { class: 'group-count num' }, `${groupDigits(items.length)} 项`),
      h('span', { class: 'group-size num' }, files.length ? atLeast(sum) : '—'),
    );
    toggle.addEventListener('click', () => {
      if (open) this.collapsed.add(key);
      else this.collapsed.delete(key);
      this.update();
    });
    return h(
      'section',
      { class: 'panel cgroup', 'data-group': group, 'aria-labelledby': `${id}-toggle` },
      h('div', { class: 'cgroup-head' }, h('h2', {}, toggle)),
      h('div', { class: 'group-body', id: `${id}-body`, hidden: !open }, h('p', { class: 'group-explain' }, meta.explain), this.loTable(items, appName ? `${appName} · ${meta.title}` : meta.title)),
    );
  }

  private loTable(items: Leftover[], caption: string): HTMLElement {
    const rows = items.map((i) => {
      const id = `lo-${i.object_id}`;
      const check = h('input', { type: 'checkbox', id: `${id}-check`, disabled: !i.selectable, 'aria-label': `选择 ${tail(i.display)}`, 'aria-describedby': `${id}-why` }) as HTMLInputElement;
      check.checked = this.selected.has(i.object_id);
      check.addEventListener('change', () => this.toggle(i));
      const tags: HTMLElement[] = [];
      if (i.action === 'registry_backup_delete') tags.push(h('span', { class: 'row-tag' }, '先备份'));
      if (i.action === 'startup_toggle') tags.push(h('span', { class: 'row-tag' }, '先备份'));
      if (i.requires_admin) tags.push(h('span', { class: 'row-tag' }, `需要${words().admin}`));
      return h(
        'tr',
        { class: `crow${check.checked ? ' selected' : ''}${i.selectable ? '' : ' locked'}` },
        h('td', { class: 'col-check' }, check),
        h(
          'td',
          { class: 'col-object' },
          h('label', { for: `${id}-check`, class: 'obj-name' }, tail(i.display)),
          h('span', { class: 'obj-path path' }, i.display),
          h('span', { class: 'obj-reason', id: `${id}-why` }, ...tags, i.evidence.join('；')),
          ...i.warnings.map((w) => h('span', { class: 'why-not' }, w)),
          i.note ? h('span', { class: 'obj-reason muted' }, i.note) : null,
          i.marker ? h('span', { class: 'obj-reason muted' }, `保留：${MARKER_WORD[i.marker] ?? i.marker}。`) : null,
          i.guidance ? h('span', { class: 'obj-reason muted' }, i.guidance) : null,
        ),
        h('td', { class: 'col-size num', title: exactTitle(i.logical_bytes) }, i.kind === 'registry_key' || i.kind === 'startup_entry' || i.kind === 'service' ? '—' : sizeText(i.logical_bytes, i.logical_bytes === null ? 1 : 0)),
        h('td', { class: 'col-status' }, tierBadge(leftoverTier(i.tier)), h('span', { class: 'small muted' }, i.selectable ? leftoverAction(i.action) : whyNotWord(i.why_not ?? i.tier))),
      );
    });
    return h(
      'div',
      { class: 'table-wrap' },
      h(
        'table',
        { class: 'data-table ctable' },
        h('caption', { class: 'sr-only' }, `${caption}：对象、依据、大小与处理方式`),
        h('thead', {}, h('tr', {}, h('th', { scope: 'col', class: 'col-check' }, h('span', { class: 'sr-only' }, '选择')), h('th', { scope: 'col' }, '对象与依据'), h('th', { scope: 'col', class: 'col-size' }, '逻辑大小'), h('th', { scope: 'col' }, '档位与处理'))),
        h('tbody', {}, ...rows),
      ),
    );
  }

  private refsPanel(l: Leftovers): HTMLElement | null {
    if (!l.external_references.length) return null;
    return h(
      'section',
      { class: 'panel refs-panel', 'aria-labelledby': 'lo-refs' },
      h('h2', { id: 'lo-refs' }, '只读提示：这些配置文件提到了安装目录'),
      h('p', { class: 'muted small' }, '它们属于你自己或其他工具，不会被修改，也不能放入计划。卸载后其中的路径可能失效，请自行检查。'),
      h('ul', { class: 'plain-list path small' }, ...l.external_references.map((r) => h('li', {}, r))),
    );
  }

  private loDraft(l: Leftovers): HTMLElement {
    const items = [...this.selected.values()];
    const sum = sumSizes(items.filter((i) => i.action === 'fs_trash').map((i) => ({ bytes: i.logical_bytes })));
    const keys = items.filter((i) => i.action === 'registry_backup_delete').length;
    const startup = items.filter((i) => i.action === 'startup_toggle').length;
    const files = items.length - keys - startup;
    const create = h('button', { type: 'button', class: 'primary wide', id: 'lo-create', disabled: !items.length || this.creating }, this.creating ? '正在冻结计划…' : items.length ? `生成残留计划（${items.length} 项）` : '先选择对象');
    create.addEventListener('click', () => void this.createLeftoversPlan());
    return h(
      'aside',
      { class: 'panel work-aside draft', 'aria-labelledby': 'lo-draft-title' },
      h('h2', { id: 'lo-draft-title' }, '残留计划草稿'),
      items.length
        ? h(
            'div',
            { class: 'draft-sum' },
            h('p', { class: 'draft-figure num' }, files ? atLeast(sum) : `${items.length} 项`),
            h(
              'ul',
              { class: 'draft-list' },
              files ? h('li', {}, h('span', {}, `文件与文件夹 ${files} 项`), h('span', { class: 'muted' }, words().moveToBin)) : null,
              keys ? h('li', {}, h('span', {}, `注册表项 ${keys} 个`), h('span', { class: 'muted' }, '先备份再删除')) : null,
              startup ? h('li', {}, h('span', {}, `启动项 ${startup} 个`), h('span', { class: 'muted' }, '先备份再移除')) : null,
            ),
          )
        : h('p', { class: 'muted small' }, '没有选择任何对象。缓存与日志默认选中，其他对象需要你逐项勾选。'),
      errorLine(this.createError),
      create,
      h('p', { class: 'muted small aside-note' }, `这是与卸载计划分开的第二份计划，单独审阅、单独批准。文件夹只移走其中的内容；所有移动都可以在${words().bin}中还原。`),
      l.apps.some((a) => !a.leftovers_reviewable)
        ? h('p', { class: 'muted small aside-note' }, `卸载没有完成的应用（${l.apps.filter((a) => !a.leftovers_reviewable).map((a) => a.app.name).join('、')}）仍然安装着，它们的任何对象都不会进入这份计划。`)
        : null,
      l.groups.some((g) => g.items.some((i) => i.why_not === 'needs_admin'))
        ? h('p', { class: 'muted small aside-note' }, `所有用户共用位置中的残留需要${words().admin}。CleanManager 不会自动提权：需要时请以${words().admin}重新启动后再审阅。`)
        : null,
    );
  }

  private loBar(): HTMLElement {
    const items = [...this.selected.values()];
    return selectionBar({
      count: items.length,
      sum: sumSizes(items.filter((i) => i.action === 'fs_trash').map((i) => ({ bytes: i.logical_bytes }))),
      onClear: () => {
        this.selected.clear();
        this.app.announce('已清空选择');
        this.update();
      },
      action: { id: 'lo-bar-go', text: '生成残留计划并审阅', disabled: this.creating, onClick: () => void this.createLeftoversPlan() },
      note: items.some((i) => i.action !== 'fs_trash') ? '注册表项与启动项不计大小' : null,
    });
  }
}

function tail(p: string): string {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? p;
}

