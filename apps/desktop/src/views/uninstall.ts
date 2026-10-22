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

import { h, icon, keepFocus, replace } from '../lib/dom';
import { formatTime } from '../lib/format';
import { RequestGate, SubjectGate } from '../lib/gate';
import { notOnThisPlatform, words } from '../lib/platform';
import type { Leftover, Leftovers, UninstallApp, UninstallList } from '../lib/suiteTypes';
import { LEFTOVER_GROUP, MARKER_WORD, SOURCE_WORD, adapterText, leftoverAction, leftoverTier, uninstallRowWords, whyNotWord } from '../lib/suiteWords';
import type { IpcError } from '../lib/types';
import { OPERATION_STATE_LABEL, operationTone } from '../lib/words';
import { LongProgress, atLeast, ensureReady, selectionBar, stageSteps, sumSizes, tierBadge, toIpcError, type Step } from './kit';
import { copyField, emptyState, errorLine, exactTitle, pageHead, pill, sizeText } from './parts';
import { APP_OUTCOME, MAX_BATCH, batchActionText, batchSelectable, batchSelection, leftoverSections, leftoversTitle, type LeftoverSection } from './uninstallBatch';
import { issueList, learnMore } from './ui';
import { t } from '../i18n';

type Filter = 'all' | 'available' | 'settings' | 'protected';

const FILTERS: { key: Filter; label: string }[] = [
  { key: 'all', get label() { return t('uninstall.filters.all.label'); } },
  { key: 'available', get label() { return t('uninstall.filters.available.label'); } },
  { key: 'settings', get label() { return words().uninstallElsewhereFilter; } },
  { key: 'protected', get label() { return t('uninstall.filters.protected.label'); } },
];

// GUI-12: the two steps keep their titles; what each does follows the platform (Trash and Homebrew on macOS).
export const UNINSTALL_STEPS: Step[] = [
  { get title() { return t('uninstall.uninstallSteps.0.title'); }, get note() { return words().uninstallStep1; } },
  { get title() { return t('uninstall.uninstallSteps.1.title'); }, get note() { return words().uninstallStep2; } },
];

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
  private progress = new LongProgress(() => t('uninstall.readingLeftovers'));

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
    this.app.announce(this.chosen.has(a.app_instance_id) ? t('uninstall.selectedNameSizeTotal', { name: a.name, size: this.chosen.size }) : t('uninstall.deselectedNameSizeTotal', { name: a.name, size: this.chosen.size }));
    this.update();
  }

  /** One plan for every selected application (UNI-11); refused ones are named on their rows and left out. */
  private async planSelected(): Promise<void> {
    const apps = batchSelection(this.list?.items ?? [], this.chosen);
    if (this.planning || !apps.length) return;
    const what = apps.length === 1 ? t('uninstall.uninstallName', { name: apps[0].name }) : t('uninstall.uninstallLength', { length: apps.length });
    if (!(await ensureReady(this.app, 'uninstall', what))) return;
    this.planning = true;
    this.batchError = null;
    for (const a of apps) this.rowError.delete(a.app_instance_id);
    this.update();
    try {
      const r = await this.app.backend.uninstallPlan(apps.map((a) => a.app_instance_id));
      for (const x of r.refused) this.rowError.set(x.app_instance_id, { code: x.reason, reason: x.reason, message: x.message });
      this.chosen.clear();
      const names = r.apps.map((a) => a.name).join(t('common.listSep'));
      this.app.announce(t('uninstall.createdUninstallPlanLength', { length: r.apps.length, names, p: r.refused.length ? t('uninstall.othersNotUninstallable', { length: r.refused.length }) : '' }));
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
    this.progress.start(t('uninstall.recheckingWhatLeftBehind'));
    this.loGate.show(op);
    this.render();
    try {
      const r = await this.loGate.load(op, this.app.backend.uninstallLeftovers(op));
      if (r.stale) return;
      this.leftovers = r.value;
      // UNI-05: only what the core preselected (caches and logs) starts selected; the person may clear it.
      for (const g of r.value.groups) for (const i of g.items) if (i.selected && i.selectable) this.selected.set(i.object_id, i);
      this.app.announce(t('uninstall.foundGroupdigitsSizeSelected', { groupDigits: r.value.groups.reduce((n, g) => n + g.items.length, 0), size: this.selected.size }));
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
    this.app.announce(this.selected.has(i.object_id) ? t('uninstall.selectedDisplay', { display: tail(i.display) }) : t('uninstall.deselectedDisplay', { display: tail(i.display) }));
    this.update();
  }

  private async createLeftoversPlan(): Promise<void> {
    const l = this.leftovers;
    if (!l || this.creating || !this.selected.size) return;
    if (!(await ensureReady(this.app, 'uninstall', t('uninstall.handleUninstallLeftovers')))) return;
    this.creating = true;
    this.createError = null;
    this.update();
    try {
      const r = await this.app.backend.uninstallLeftoversPlan(l.operation_id, [...this.selected.keys()]);
      this.app.announce(t('uninstall.leftoverPlanCreatedNothing'));
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
    const refresh = h('button', { type: 'button', id: 'uninstall-refresh', disabled: this.loading }, icon('refresh'), this.loading ? t('common.reading') : t('common.reload'));
    refresh.addEventListener('click', () => void this.load());
    if (d && !d.supported) {
      replace(this.body, pageHead({ id: 'uninstall-title', title: t('common.uninstall') }), emptyState(t('common.notAvailablePlatformYet'), notOnThisPlatform(t('uninstall.appUninstallOnlyImplemented')), null));
      return;
    }
    const available = d?.items.filter((a) => a.state === 'available').length ?? 0;
    replace(
      this.body,
      pageHead({
        id: 'uninstall-title',
        title: t('common.uninstall'),
        meta: d ? [t('uninstall.appsCount', { length: d.items.length }), t('uninstall.availableUninstalled', { available }), d.is_admin ? t('common.runningAdmin', { admin: words().admin }) : t('common.runningCurrentUser'), t('common.readOnlyList')] : [t('common.readOnlyList')],
        actions: [refresh],
      }),
      stageSteps(t('uninstall.uninstallingTakesTwoSteps'), UNINSTALL_STEPS, 0),
      errorLine(this.listError),
      d
        ? h('div', { class: 'work-layout' }, h('div', { class: 'work-main' }, this.recentPanel(d), this.appsPanel(d)), this.howPanel())
        : !this.listError
          ? h('p', { class: 'muted', role: 'status' }, t('uninstall.readingInstalledApps'))
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
        this.app.announce(t('common.selectionCleared'));
        this.update();
      },
      action: { id: 'uninstall-bar-go', text: this.planning ? t('uninstall.verifying') : batchActionText(n), disabled: this.planning, onClick: () => void this.planSelected() },
      note: n > 1 ? t('uninstall.eachAppQuitsUninstalls') : null,
    });
  }

  private appsPanel(d: UninstallList): HTMLElement {
    const search = h('input', { type: 'search', id: 'uninstall-search', placeholder: t('uninstall.findNamePublisher'), 'aria-label': t('uninstall.findApps'), value: this.query }) as HTMLInputElement;
    search.addEventListener('input', () => {
      this.query = search.value;
      this.update();
    });
    const seg = h(
      'fieldset',
      { class: 'seg' },
      h('legend', { class: 'sr-only' }, t('uninstall.filterApps')),
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
      h('div', { class: 'panel-head' }, h('h2', { id: 'uninstall-apps-title' }, t('uninstall.installedApps')), h('div', { class: 'list-tools' }, seg, search)),
      h(
        'p',
        { class: 'muted small' },
        n ? t('uninstall.appsSelected', { n }) : '',
        t('uninstall.selectOneMoreApps', { MAX_BATCH }),
      ),
      errorLine(this.batchError),
      items.length ? this.appsTable(items) : h('p', { class: 'muted small' }, t('common.noAppsMatch')),
      d.issues.length
        ? h(
            'details',
            { class: 'issues-details' },
            h('summary', {}, t('uninstall.registrationsCouldntReadLength', { length: d.issues.length })),
            issueList(d.issues, { label: t('common.recordedIssues') }),
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
        'aria-label': t('common.selectName', { name: a.name }),
      }) as HTMLInputElement;
      check.checked = can && this.chosen.has(a.app_instance_id);
      check.addEventListener('change', () => this.toggleApp(a));
      let action: HTMLElement;
      if (a.state === 'available') {
        action = h('span', { class: 'muted small' }, check.checked ? (this.planning ? t('uninstall.verifying') : t('uninstall.selected')) : t('uninstall.selectUninstall'));
      } else if (st.settingsButton) {
        const btn = h('button', { type: 'button', class: 'quiet', id: `${id}-settings` }, icon('external'), t('uninstall.uninstallSettings'));
        btn.addEventListener('click', () => void this.app.backend.openSystemPage('apps_features'));
        action = btn;
      } else if (st.guidance) {
        action = h('span', { class: 'muted small' }, st.needsAdmin ? t('uninstall.deleteLaunchpadFinder') : t('uninstall.handleFinder'));
      } else {
        action = h('span', { class: 'muted small' }, t('uninstall.noUninstallRoute'));
      }
      const facts = [a.publisher ?? t('common.unknownPublisher'), a.version ? t('common.versionVersion', { version: a.version }) : null, SOURCE_WORD[a.source] ?? a.source, a.last_used_at ? t('uninstall.lastUsedLastUsed', { last_used_at: formatTime(a.last_used_at) }) : null].filter(Boolean).join(' · ');
      return h(
        'tr',
        { class: `crow${check.checked ? ' selected' : ''}${can ? '' : ' locked'}` },
        h('td', { class: 'col-check' }, check),
        h(
          'td',
          { class: 'col-object' },
          can ? h('label', { for: `${id}-check`, class: 'obj-name' }, a.name) : h('span', { class: 'obj-name' }, a.name),
          h('span', { class: 'obj-path' }, facts),
          a.adapter ? h('span', { class: 'obj-reason muted' }, t('uninstall.method'), adapterText(a.adapter)) : null,
          st.reason ? h('span', { class: a.state === 'unsupported' ? 'obj-reason muted' : 'why-not' }, st.reason) : null,
          st.guidance ? h('span', { class: 'obj-reason' }, st.guidance) : null,
          errorLine(err ?? null),
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
        h('caption', { class: 'sr-only' }, t('uninstall.installedAppsHowThey')),
        h('thead', {}, h('tr', {}, h('th', { scope: 'col', class: 'col-check' }, h('span', { class: 'sr-only' }, t('common.select'))), h('th', { scope: 'col' }, t('uninstall.appUninstallMethod')), h('th', { scope: 'col', class: 'col-flags' }, t('common.status')), h('th', { scope: 'col', class: 'col-status' }, t('common.action')))),
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
        ? h('button', { type: 'button', class: 'quiet', onclick: () => this.openLeftovers(r.operation_id) }, t('common.reviewLeftovers'))
        : h('button', { type: 'button', class: 'quiet', onclick: () => this.app.openOperation(r.operation_id, 'uninstall') }, t('common.viewResult'));
      return h(
        'li',
        { class: 'run-row' },
        h('span', { class: 'run-main' }, h('b', {}, r.app_name), h('span', { class: 'muted small' }, `${r.stage === 'uninstall' ? t('uninstall.step1UninstallerP', { p: r.app_count > 1 ? t('uninstall.appCount', { app_count: r.app_count }) : '' }) : t('uninstall.step2Leftovers')} · ${formatTime(r.started_at)}`)),
        pill(OPERATION_STATE_LABEL[r.state] ?? r.state, operationTone(r.state)),
        go,
      );
    });
    return h('section', { class: 'panel', 'aria-labelledby': 'uninstall-recent-title' }, h('h2', { id: 'uninstall-recent-title' }, t('uninstall.recentUninstalls')), h('ul', { class: 'run-list' }, ...rows));
  }

  private howPanel(): HTMLElement {
    return h(
      'aside',
      { class: 'panel work-aside', 'aria-labelledby': 'uninstall-how' },
      h('h2', { id: 'uninstall-how' }, t('uninstall.howUninstallingWorks')),
      h('p', { class: 'small' }, words().uninstallHowReview),
      learnMore(
        h('p', {}, words().uninstallHowRescan),
        h('p', {}, t('uninstall.whenSelectSeveralApps')),
      ),
      h(
        'div',
        { class: 'aside-section' },
        h('h3', {}, t('uninstall.ifAppStillRunning')),
        h('p', { class: 'small' }, words().uninstallHowRunning),
        learnMore(
          h('p', {}, t('uninstall.ifReallyNeedEnd')),
          copyField(t('uninstall.terminalNeedsManualConfirmation'), t('uninstall.cleanmanagerAppsUninstallPlan')),
        ),
      ),
      h(
        'div',
        { class: 'aside-section' },
        h('h3', {}, t('uninstall.appsWithoutUninstallRoute')),
        h('p', { class: 'small' }, t('uninstall.securitySoftwareSystemComponents')),
        learnMore(h('p', {}, words().uninstallManually)),
      ),
    );
  }

  // ---- leftovers -----------------------------------------------------------------------------

  private renderLeftovers(): void {
    const l = this.leftovers;
    const back = { label: t('uninstall.renderLeftovers.label'), onClick: () => this.closeLeftovers() };
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
          ? [h('code', { class: 'selectable' }, l.operation_id), t('common.objectCount', { length: all.length }), t('uninstall.defaultSelectedSelectedDefault', { default_selected: l.default_selected }), l.is_admin ? t('common.runningAdmin', { admin: words().admin }) : t('common.runningCurrentUser')]
          : [],
      }),
      stageSteps(t('uninstall.uninstallingTakesTwoSteps'), UNINSTALL_STEPS, 1),
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
          : h('div', { class: 'panel' }, emptyState(t('uninstall.noLeftovers'), t('uninstall.recheckFoundNoLeftoverkindsor', { leftoverKindsOr: words().leftoverKindsOr }), h('button', { type: 'button', onclick: back.onClick }, back.label)))
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
      h('h2', { id: 'lo-sum', class: 'sr-only' }, t('uninstall.summary')),
      h(
        'dl',
        { class: 'tier-summary' },
        h('div', {}, h('dt', {}, tierBadge('default_selected')), h('dd', { class: 'num' }, t('common.groupdigits', { groupDigits: count((i) => i.tier === 'default_selected') }), h('small', {}, t('uninstall.cachesLogsSelectedDefault')))),
        h('div', {}, h('dt', {}, tierBadge('review_only')), h('dd', { class: 'num' }, t('common.groupdigits', { groupDigits: count((i) => i.tier === 'review_only') }), h('small', {}, t('uninstall.settingsPersonalDataMore')))),
        h('div', {}, h('dt', {}, tierBadge('blocked')), h('dd', { class: 'num' }, t('common.groupdigits', { groupDigits: count((i) => !i.selectable) }), h('small', {}, t('uninstall.keptBlockedNeedsManual')))),
      ),
      keys ? h('p', { class: 'muted small lo-keys' }, t('uninstall.keysBeforeDeletionWhole', { keys })) : null,
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
        h('span', {}, pill(st.label, st.tone), ' ', h('span', { class: 'small muted num' }, sec.reviewable ? t('uninstall.objects', { objects: sec.objects }) : t('uninstall.allKept'))),
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
      h('span', { class: 'group-count num' }, t('common.itemCount', { length: items.length })),
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
      const check = h('input', { type: 'checkbox', id: `${id}-check`, disabled: !i.selectable, 'aria-label': t('uninstall.selectDisplay', { display: tail(i.display) }), 'aria-describedby': `${id}-why` }) as HTMLInputElement;
      check.checked = this.selected.has(i.object_id);
      check.addEventListener('change', () => this.toggle(i));
      const tags: HTMLElement[] = [];
      if (i.action === 'registry_backup_delete') tags.push(h('span', { class: 'row-tag' }, t('uninstall.backedUpFirst')));
      if (i.action === 'startup_toggle') tags.push(h('span', { class: 'row-tag' }, t('uninstall.backedUpFirst')));
      if (i.requires_admin) tags.push(h('span', { class: 'row-tag' }, t('common.needsAdmin', { admin: words().admin })));
      return h(
        'tr',
        { class: `crow${check.checked ? ' selected' : ''}${i.selectable ? '' : ' locked'}` },
        h('td', { class: 'col-check' }, check),
        h(
          'td',
          { class: 'col-object' },
          h('label', { for: `${id}-check`, class: 'obj-name' }, tail(i.display)),
          h('span', { class: 'obj-path path' }, i.display),
          h('span', { class: 'obj-reason', id: `${id}-why` }, ...tags, i.evidence.join(t('common.clauseSep'))),
          ...i.warnings.map((w) => h('span', { class: 'why-not' }, w)),
          i.note ? h('span', { class: 'obj-reason muted' }, i.note) : null,
          i.marker ? h('span', { class: 'obj-reason muted' }, t('uninstall.keptP', { p: MARKER_WORD[i.marker] ?? i.marker })) : null,
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
        h('caption', { class: 'sr-only' }, t('uninstall.captionObjectsEvidenceSize', { caption })),
        h('thead', {}, h('tr', {}, h('th', { scope: 'col', class: 'col-check' }, h('span', { class: 'sr-only' }, t('common.select'))), h('th', { scope: 'col' }, t('uninstall.objectEvidence')), h('th', { scope: 'col', class: 'col-size' }, t('common.logicalSize')), h('th', { scope: 'col' }, t('uninstall.tierAction')))),
        h('tbody', {}, ...rows),
      ),
    );
  }

  private refsPanel(l: Leftovers): HTMLElement | null {
    if (!l.external_references.length) return null;
    return h(
      'section',
      { class: 'panel refs-panel', 'aria-labelledby': 'lo-refs' },
      h('h2', { id: 'lo-refs' }, t('uninstall.readOnlyNoticeConfig')),
      h('p', { class: 'muted small' }, t('uninstall.theyBelongOtherTools')),
      h('ul', { class: 'plain-list path small' }, ...l.external_references.map((r) => h('li', {}, r))),
    );
  }

  private loDraft(l: Leftovers): HTMLElement {
    const items = [...this.selected.values()];
    const sum = sumSizes(items.filter((i) => i.action === 'fs_trash').map((i) => ({ bytes: i.logical_bytes })));
    const keys = items.filter((i) => i.action === 'registry_backup_delete').length;
    const startup = items.filter((i) => i.action === 'startup_toggle').length;
    const files = items.length - keys - startup;
    const create = h('button', { type: 'button', class: 'primary wide', id: 'lo-create', disabled: !items.length || this.creating }, this.creating ? t('common.freezingPlan') : items.length ? t('uninstall.createLeftoverPlanLength', { length: items.length }) : t('common.selectObjectsFirst'));
    create.addEventListener('click', () => void this.createLeftoversPlan());
    return h(
      'aside',
      { class: 'panel work-aside draft', 'aria-labelledby': 'lo-draft-title' },
      h('h2', { id: 'lo-draft-title' }, t('uninstall.leftoverPlanDraft')),
      items.length
        ? h(
            'div',
            { class: 'draft-sum' },
            h('p', { class: 'draft-figure num' }, files ? atLeast(sum) : t('uninstall.itemCount', { length: items.length })),
            h(
              'ul',
              { class: 'draft-list' },
              files ? h('li', {}, h('span', {}, t('uninstall.files', { files })), h('span', { class: 'muted' }, words().moveToBin)) : null,
              keys ? h('li', {}, h('span', {}, t('uninstall.keys', { keys })), h('span', { class: 'muted' }, t('uninstall.backUpDelete'))) : null,
              startup ? h('li', {}, h('span', {}, t('uninstall.startup', { startup })), h('span', { class: 'muted' }, t('uninstall.backUpRemove'))) : null,
            ),
          )
        : h('p', { class: 'muted small' }, t('uninstall.nothingSelectedCachesLogs')),
      errorLine(this.createError),
      create,
      h('p', { class: 'muted small aside-note' }, t('uninstall.secondPlanSeparateUninstall', { bin: words().bin })),
      l.apps.some((a) => !a.leftovers_reviewable)
        ? h('p', { class: 'muted small aside-note' }, t('uninstall.appsWhoseUninstallDidnt', { join: l.apps.filter((a) => !a.leftovers_reviewable).map((a) => a.app.name).join(t('common.listSep')) }))
        : null,
      l.groups.some((g) => g.items.some((i) => i.why_not === 'needs_admin'))
        ? h('p', { class: 'muted small aside-note' }, t('uninstall.leftoversLocationsSharedAll', { admin: words().admin, admin2: words().admin }))
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
        this.app.announce(t('common.selectionCleared'));
        this.update();
      },
      action: { id: 'lo-bar-go', text: t('uninstall.createLeftoverPlanReview'), disabled: this.creating, onClick: () => void this.createLeftoversPlan() },
      note: items.some((i) => i.action !== 'fs_trash') ? t('uninstall.registryKeysStartupItems') : null,
    });
  }
}

function tail(p: string): string {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? p;
}

