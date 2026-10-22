import './styles.css';

import { createBackend, toIpcError, type Backend } from './lib/api';
import { appMark, h, icon, live, refreshLive, replace } from './lib/dom';
import { isTerminal, taskTitle } from './lib/format';
import { EMPTY_COUNTS, Store, type ActivityTab, type Page } from './lib/store';
import type { DiagTarget } from './lib/diagTypes';
import type { ReadinessOp, ScanStatus } from './lib/types';
import { OPERATION_STATE_LABEL } from './lib/words';
import { EXTENSIONS, TOOLS, gatedOff, placeOf, shortcutFor, shortcutLabel, toolForPage, toolOf, type ExtId, type ToolId } from './lib/nav';
import { platform, setPlatform, words } from './lib/platform';
import { ActivityPage } from './views/activity';
import { AppsPage } from './views/apps';
import { CleanupPage } from './views/cleanup';
import { DiagPage } from './views/diag';
import { DiskPage } from './views/disk';
import { OperationPage } from './views/operation';
import { OverviewPage } from './views/overview';
import { PlanPage } from './views/plan';
import { ScopePage } from './views/scope';
import { SettingsPage } from './views/settings';
import { ApplyChip, TaskChip } from './views/task';
import { CatalogPage } from './views/catalog';
import { InsightsPage, StartupPage, StatusPage, capabilityPages } from './views/tools';
import { UninstallPage } from './views/uninstall';
import { UpdatesPage } from './views/updates';
import { DiagnosisPage, MaintenancePage } from './views/maintenance';
import { ProcessesPage } from './views/processes';
import { ResidentPage, batteryName } from './views/resident';
import { ensureReady } from './views/kit';
import { GateOffView } from './views/gateOff';
import { gateBadge } from './views/ui';
import { refusalText } from './lib/codes';
import { locale, onLocale, resolveLocale, setLocale, storedLanguage, storeLanguage, t, type LanguagePref } from './i18n';

export interface App {
  backend: Backend;
  store: Store;
  navigate(page: Page): void;
  openScan(scanId: string, page?: Page): Promise<void>;
  startScan(rootToken: string): Promise<void>;
  cancelScan(scanId: string): Promise<void>;
  refreshStatus(): Promise<void>;
  openPlan(planId: string, from?: Page): void;
  openOperation(operationId: string, from?: Page): void;
  openActivity(tab: ActivityTab): void;
  /** Apply a stored, authorized plan; opens its result page. `readiness` names the permission check (default `apply`). */
  applyPlan(planId: string, readiness?: ReadinessOp): Promise<void>;
  /** Optional AI diagnosis of one object (from the cleanup, disk or apps page). */
  openDiagnosis(target: DiagTarget, from?: Page): void;
  announce(text: string): void;
  /** Interface language (LIF-05): store the choice and show every page in it at once, without a restart. */
  setLanguage(pref: LanguagePref): void;
}

const THEME_KEY = 'cm.theme';

/**
 * The language preference: stored next to the theme (`cm.lang`, absent = follow the system). The browser design preview
 * also takes `?lang=en`, `?lang=zh-CN` or `?lang=system` for one visit (not stored); the desktop host has no query string.
 */
function initialLanguage(search: string = location.search): LanguagePref {
  const asked = new URLSearchParams(search).get('lang');
  if (asked === 'en' || asked === 'zh-CN' || asked === 'system') return asked;
  return storedLanguage();
}

function applyTheme(theme: string | null): void {
  const root = document.documentElement;
  if (theme === 'light' || theme === 'dark') root.dataset.theme = theme;
  else delete root.dataset.theme;
}

function storedTheme(): string | null {
  try {
    return localStorage.getItem(THEME_KEY);
  } catch {
    return null;
  }
}

function cycleTheme(): string {
  const cur = document.documentElement.dataset.theme ?? 'system';
  const next = cur === 'system' ? 'dark' : cur === 'dark' ? 'light' : 'system';
  applyTheme(next === 'system' ? null : next);
  try {
    if (next === 'system') localStorage.removeItem(THEME_KEY);
    else localStorage.setItem(THEME_KEY, next);
  } catch {
    /* per-viewer convenience only */
  }
  return next;
}

async function boot(): Promise<void> {
  applyTheme(storedTheme());
  setLocale(resolveLocale(initialLanguage()));
  const root = document.getElementById('app')!;
  const booting = root.querySelector('.booting');
  if (booting) booting.textContent = t('main.starting');
  let backend: Backend;
  try {
    backend = await createBackend();
  } catch (e) {
    replace(root, h('div', { class: 'fatal', role: 'alert' }, h('h1', {}, t('main.cleanmanagerCouldntStart')), h('p', {}, String(e))));
    return;
  }

  // Platform words (GUI-12) are settled before the first page renders.
  const earlyCaps = await backend.capabilities().catch(() => null);
  if (earlyCaps?.platform === 'macos' || earlyCaps?.platform === 'windows') setPlatform(earlyCaps.platform);
  document.documentElement.dataset.platform = platform();

  const store = new Store({
    page: 'scope',
    caps: null,
    recent: [],
    scanId: null,
    status: null,
    statusError: null,
    task: null,
    apply: null,
    planId: null,
    operationId: null,
    returnTo: null,
    activityTab: 'operations',
    closing: false,
    preview: backend.preview,
    diagTarget: null,
    diagId: null,
    revealEntry: null,
    leftoversOp: null,
    processFocus: null,
  });

  const announcer = h('div', { class: 'sr-only', role: 'status', 'aria-live': 'polite', 'aria-atomic': 'true' });
  const announce = (text: string) => {
    announcer.textContent = '';
    window.setTimeout(() => (announcer.textContent = text), 60);
  };

  let statusTicket = 0;
  const app: App = {
    backend,
    store,
    announce,
    setLanguage(pref) {
      storeLanguage(pref);
      const before = locale();
      setLocale(resolveLocale(pref));
      // Same language (e.g. following a system that already matched): the settings page still shows the new choice.
      if (locale() === before) render();
      announce(t('main.languageNow'));
    },
    navigate(page) {
      // Navigation only changes the view; it never cancels or pauses a running scan or operation.
      store.set({ page });
    },
    async openScan(scanId, page) {
      store.set({ scanId, status: null, statusError: null, ...(page ? { page } : {}) });
      await app.refreshStatus();
    },
    async refreshStatus() {
      const id = store.state.scanId;
      if (!id) return;
      const ticket = ++statusTicket;
      try {
        const status = await backend.scanStatus(id);
        if (ticket !== statusTicket || store.state.scanId !== id) return;
        store.set({ status, statusError: null });
        syncTaskFromStatus(status);
      } catch (e) {
        if (ticket !== statusTicket) return;
        store.set({ statusError: toIpcError(e) });
      }
    },
    async startScan(rootToken) {
      // GUI-13: explain missing access once before the long task; it never prompts mid-scan.
      if (!(await ensureReady(app, 'scan', t('main.scan')))) return;
      const res = await backend.startScan(rootToken);
      store.set({
        task: {
          scanId: res.scan_id,
          rootDisplay: res.root_display,
          state: 'created',
          entriesSeen: 0,
          skippedRanges: 0,
          cancelRequested: false,
          startedAt: new Date().toISOString(),
          error: null,
          dismissed: false,
        },
      });
      announce(t('main.readOnlyScanStarted', { root_display: res.root_display }));
      await app.openScan(res.scan_id, 'overview');
    },
    async cancelScan(scanId) {
      const r = await backend.cancelScan(scanId);
      const task = store.state.task;
      if (task && task.scanId === scanId && r.accepted) {
        store.set({ task: { ...task, cancelRequested: true, state: task.state === 'running' ? 'stopping' : task.state } });
        announce(t('main.stopRequestedScanStopping'));
      }
      await app.refreshStatus();
    },
    openPlan(planId, from) {
      store.set({ planId, page: 'plan', returnTo: from ?? store.state.page });
    },
    openOperation(operationId, from) {
      store.set({ operationId, page: 'operation', returnTo: from ?? store.state.page });
    },
    openDiagnosis(target, from) {
      // A new object: the workbench starts from its preview; a run elsewhere keeps running.
      store.set({ diagTarget: target, diagId: null, page: 'diag', returnTo: from ?? store.state.page });
    },
    openActivity(tab) {
      store.set({ activityTab: tab, page: 'activity' });
    },
    async applyPlan(planId, readiness = 'apply') {
      if (!(await ensureReady(app, readiness, t('main.runPlan')))) return;
      const r = await backend.startApply(planId);
      if (!r.existing) {
        store.set({
          apply: { planId, operationId: r.operation_id, total: r.total, counts: { ...EMPTY_COUNTS }, cancelRequested: false, running: true, status: null, error: null, dismissed: false },
        });
        announce(t('main.planRunStartedTotal', { total: r.total }));
      } else {
        announce(t('main.planAlreadyRunShowing'));
      }
      app.openOperation(r.operation_id, 'plan');
    },
  };

  function syncTaskFromStatus(s: ScanStatus): void {
    const t = store.state.task;
    if (t && t.scanId === s.scan_id) {
      store.set({ task: { ...t, state: s.state, entriesSeen: s.entries_seen, skippedRanges: s.skipped_ranges, cancelRequested: s.cancel_requested } });
    }
  }

  // Events from the host: coalesced progress and one terminal notice per task.
  await backend.onProgress((e) => {
    const task = store.state.task;
    if (!task || task.scanId !== e.scan_id) return;
    const changedState = task.state !== e.state;
    store.set({ task: { ...task, state: e.state, entriesSeen: e.entries_seen, skippedRanges: e.skipped_ranges, cancelRequested: e.cancel_requested } });
    if (changedState && e.state === 'stopping') announce(t('main.stoppingScan'));
    if (store.state.scanId === e.scan_id && store.state.status && !isTerminal(store.state.status.state)) {
      store.set({ status: { ...store.state.status, state: e.state, entries_seen: e.entries_seen, skipped_ranges: e.skipped_ranges, cancel_requested: e.cancel_requested } });
    }
  });
  await backend.onFinished(async (e) => {
    const task = store.state.task;
    if (task && task.scanId === e.scan_id) {
      store.set({ task: { ...task, state: e.state ?? 'failed', error: e.error, dismissed: false } });
    }
    announce(e.state ? taskTitle(e.state) : t('main.scanCouldntStartP', { p: e.error ? refusalText(e.error) : '' }));
    if (store.state.scanId === e.scan_id) await app.refreshStatus();
    store.set({ recent: await backend.recentScans().catch(() => store.state.recent) });
  });
  await backend.onApplyProgress((e) => {
    const a = store.state.apply;
    if (!a || a.operationId !== e.operation_id) return;
    store.set({ apply: { ...a, total: e.total, counts: e.counts, cancelRequested: e.cancel_requested } });
  });
  await backend.onApplyFinished((e) => {
    const a = store.state.apply;
    if (a && a.operationId === e.operation_id) {
      store.set({ apply: { ...a, running: false, status: e.status, error: e.error, dismissed: false } });
    }
    announce(e.status ? t('main.runFinishedP', { p: OPERATION_STATE_LABEL[e.status] ?? e.status }) : t('main.runCouldntStartP', { p: e.error ? refusalText(e.error) : '' }));
  });
  await backend.onDiagFinished((e) => {
    const word = e.state === 'completed' ? t('main.completed') : e.state === 'cancelled' ? t('main.cancelled') : e.state === 'failed' ? t('main.failed') : e.state;
    announce(t('main.aiDiagnosisWord', { word }));
    void (pages.diag as DiagPage).finished(e.diagnosis_id);
  });
  // Tray (RES-01): quick links open one of a fixed set of pages; alerts and changes refresh 常驻工具.
  await backend.onResidentNavigate((page) => {
    if (['status', 'processes', 'resident', 'cleanup'].includes(page)) app.navigate(page as Page);
  });
  await backend.onResidentChanged(() => (pages.resident as ResidentPage).changed());
  await backend.onResidentAlert((alerts) => {
    announce(alerts.map((a) => t('main.batteryLowNamePercent', { name: batteryName(a), percent: a.percent })).join(t('common.clauseSep')));
    (pages.resident as ResidentPage).alerted(alerts);
  });
  // D15: the switch changed from the CLI; the host already stopped the resident utilities when it went off. Fresh
  // capabilities make open gated pages show (or leave) their "默认关闭" page.
  await backend.onFeaturesChanged((e) => {
    void backend.capabilities().then((caps) => store.set({ caps }), () => undefined);
    const stopped = e.resident_stopped ? (e.error ? t('main.residentToolsStoppedBut') : t('main.toolsHaventPassedGate')) : '';
    announce(`${e.experimental_features ? t('main.experimentalFeaturesTurnedCommand') : t('main.experimentalFeaturesTurnedOff')}${stopped}`);
  });
  await backend.onClosing((e) => {
    store.set({ closing: true });
    announce(e.waiting_for.includes('operation') ? t('main.stoppingFurtherActionsWindow') : t('main.stoppingScanSafelyWindow'));
  });

  // Shell: five tools, then the extensions (D14). Every entry keeps its place on every platform.
  const caps = capabilityPages(app);
  const pages: Record<Page, { el: HTMLElement; update(): void; activate?(): void; relocalize?(): void }> = {
    scope: new ScopePage(app),
    overview: new OverviewPage(app),
    disk: new DiskPage(app),
    insights: new InsightsPage(app),
    cleanup: new CleanupPage(app),
    purge: new CatalogPage(app, 'purge'),
    installer: new CatalogPage(app, 'installer'),
    recyclebin: caps.recyclebin,
    plan: new PlanPage(app),
    operation: new OperationPage(app),
    apps: new AppsPage(app),
    uninstall: new UninstallPage(app),
    updates: new UpdatesPage(app),
    startup: new StartupPage(app),
    maintenance: new MaintenancePage(app),
    diagnosis: new DiagnosisPage(app),
    status: new StatusPage(app),
    processes: new ProcessesPage(app),
    resident: new ResidentPage(app),
    activity: new ActivityPage(app),
    agent: caps.agent,
    settings: new SettingsPage(app),
    diag: new DiagPage(app),
  };
  const w = words();
  const lastSection = new Map<ToolId, Page>();
  const openTool = (id: ToolId) => {
    const t = toolOf(id);
    app.navigate(lastSection.get(id) ?? t.sections[0].page);
  };
  const openExt = (id: ExtId) => {
    const e = EXTENSIONS.find((x) => x.id === id)!;
    if (e.tab) app.openActivity(e.tab);
    else if (e.page === 'diag') {
      store.set({ diagTarget: null, diagId: null, returnTo: store.state.page === 'diag' ? store.state.returnTo : store.state.page });
      app.navigate('diag');
    } else app.navigate(e.page);
  };
  const keyHint = (key: string) => shortcutLabel(key, w.mod);
  const ariaKeys = (key: string) => `${platform() === 'macos' ? 'Meta' : 'Control'}+${key}`;
  // Shell text is bound with `live`: a language switch (LIF-05) re-applies it without rebuilding the shell.
  const toolButtons = TOOLS.map((tool) =>
    live(
      h(
        'button',
        { class: 'nav-item tool-item', type: 'button', 'data-tool': tool.id, 'aria-keyshortcuts': ariaKeys(tool.key), onclick: () => openTool(tool.id) },
        icon(tool.icon),
        live(h('span', { class: 'nav-label' }), () => tool.label),
        h('kbd', { class: 'nav-key', 'aria-hidden': 'true' }, keyHint(tool.key)),
      ),
      null,
      { title: () => t('main.labelBlurbKey', { label: tool.label, blurb: tool.blurb, key: keyHint(tool.key) }) },
    ),
  );
  const extNav = live(h('nav', { class: 'ext-nav', id: 'ext-nav' }), null, { 'aria-label': () => t('main.more') });
  const moreBtn = live(
    h('button', { class: 'nav-item more-item', type: 'button', 'aria-expanded': 'false', 'aria-controls': 'ext-nav' }, icon('ext-plans'), live(h('span', { class: 'nav-label' }), () => t('main.moreButton'))),
    null,
    { title: () => t('main.morePlansActivityRecovery') },
  );
  const closeMore = () => {
    extNav.classList.remove('open');
    moreBtn.setAttribute('aria-expanded', 'false');
  };
  const extButtons = EXTENSIONS.map((e) =>
    live(
      h(
        'button',
        { class: 'nav-item ext-item', type: 'button', 'data-ext': e.id, 'aria-keyshortcuts': ariaKeys(e.key), onclick: () => (closeMore(), openExt(e.id)) },
        icon(e.icon),
        live(h('span', { class: 'nav-label' }), () => e.label),
        h('kbd', { class: 'nav-key', 'aria-hidden': 'true' }, keyHint(e.key)),
      ),
      null,
      { title: () => t('main.labelKey', { label: e.label, key: keyHint(e.key) }) },
    ),
  );
  replace(extNav, live(h('p', { class: 'nav-group-label', 'aria-hidden': 'true' }), () => t('main.more')), ...extButtons);
  moreBtn.addEventListener('click', () => {
    const open = !extNav.classList.contains('open');
    extNav.classList.toggle('open', open);
    moreBtn.setAttribute('aria-expanded', open ? 'true' : 'false');
    if (open) requestAnimationFrame(() => extButtons[0].focus());
  });
  extNav.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && extNav.classList.contains('open')) {
      closeMore();
      moreBtn.focus();
    }
  });
  const themeBtn = h('button', { class: 'icon-btn', type: 'button' }, icon('theme'));
  const syncThemeLabel = (theme: string) => {
    const label = theme === 'system' ? t('main.system') : theme === 'dark' ? t('main.dark') : t('main.light');
    themeBtn.title = t('main.themeLabel', { label });
    themeBtn.setAttribute('aria-label', t('main.switchThemeNowLabel', { label }));
  };
  syncThemeLabel(document.documentElement.dataset.theme ?? 'system');
  themeBtn.addEventListener('click', () => syncThemeLabel(cycleTheme()));
  const keysBtn = live(h('button', { class: 'icon-btn', type: 'button', 'aria-keyshortcuts': 'Shift+?' }, icon('keyboard')), null, { title: () => t('main.keyboardShortcutsHint'), 'aria-label': () => t('main.keyboardShortcuts') });
  const sidebar = h(
    'aside',
    { class: 'sidebar', 'aria-label': 'CleanManager' },
    h('div', { class: 'brand' }, appMark(28), h('span', { class: 'brand-name' }, 'CleanManager')),
    live(h('nav', { class: 'tool-nav' }, ...toolButtons, moreBtn), null, { 'aria-label': () => t('main.tools') }),
    extNav,
  );
  // Theme and shortcut sheet sit at the foot of the extension group (inside the "更多" menu on narrow windows).
  extNav.append(h('div', { class: 'side-foot' }, keysBtn, themeBtn));
  const sectionNav = h('nav', { class: 'section-nav', 'aria-label': t('main.sections') }); // labelled per tool in render()
  const chip = new TaskChip(app);
  const applyChip = new ApplyChip(app);
  const main = h('main', { id: 'main', tabindex: '-1' });
  const previewBanner = backend.preview
    ? live(h('div', { class: 'preview-banner', role: 'note' }), () => t('main.browserPreviewDataBelow'))
    : null;
  const closingBanner = live(h('div', { class: 'closing-banner', role: 'alert', hidden: true }), () => t('main.stoppingSafelyScanPublishes'));
  const skip = live(h('a', { class: 'skip-link', href: '#main' }), () => t('main.skipMainContent'));
  const header = h('header', { class: 'topbar' }, sectionNav, h('div', { class: 'topbar-tools' }, applyChip.el, chip.el));
  replace(root, skip, h('div', { class: 'shell' }, sidebar, h('div', { class: 'main-col' }, previewBanner, header, closingBanner, main)), announcer);

  const showShortcuts = () => {
    if (document.querySelector('dialog.shortcuts[open]')) return;
    const row = (keys: string, what: string) => h('tr', {}, h('td', {}, h('kbd', {}, keys)), h('td', {}, what));
    const dlg = h(
      'dialog',
      { class: 'cm-dialog shortcuts', 'aria-labelledby': 'keys-title' },
      h('div', { class: 'dlg-head' }, h('h2', { id: 'keys-title' }, t('main.keyboardShortcuts')), h('p', { class: 'muted' }, t('main.sameBothPlatformsCtrl'))),
      h(
        'table',
        { class: 'keys-table' },
        h('caption', { class: 'sr-only' }, t('main.shortcutsWhereTheyGo')),
        h(
          'tbody',
          {},
          ...TOOLS.map((tool) => row(keyHint(tool.key), t('main.labelBlurb', { label: tool.label, blurb: tool.blurb }))),
          ...EXTENSIONS.map((e) => row(keyHint(e.key), e.label)),
          row(`${keyHint('PageUp')} / ${keyHint('PageDown')}`, t('main.previousNextSectionCurrent')),
          row('?', t('main.showTable')),
          row('Esc', t('main.closeDialogMenu')),
        ),
      ),
      h('div', { class: 'dlg-actions' }, h('button', { type: 'button', class: 'primary', onclick: () => dlg.close() }, t('main.close'))),
    ) as HTMLDialogElement;
    dlg.addEventListener('close', () => dlg.remove());
    document.body.append(dlg);
    dlg.showModal();
  };
  keysBtn.addEventListener('click', () => showShortcuts());

  // Keyboard: one map on both platforms (Ctrl on Windows, ⌘ on macOS); see docs/visual-identity.md.
  const inField = (t: EventTarget | null) => t instanceof HTMLElement && (t.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(t.tagName));
  document.addEventListener('keydown', (e) => {
    const target = shortcutFor(e, platform() === 'macos');
    if (!target) return;
    if (target.kind === 'help') {
      if (inField(e.target) || document.querySelector('dialog[open]')) return;
      e.preventDefault();
      showShortcuts();
      return;
    }
    if (document.querySelector('dialog[open]')) return;
    e.preventDefault();
    if (target.kind === 'tool') openTool(target.tool.id);
    else if (target.kind === 'ext') openExt(target.ext.id);
    else {
      const t = toolForPage(store.state.page);
      if (!t || t.sections.length < 2) return;
      const i = t.sections.findIndex((x) => x.page === store.state.page);
      const next = t.sections[(i + target.step + t.sections.length) % t.sections.length];
      app.navigate(next.page);
      requestAnimationFrame(() => (sectionNav.querySelector('[aria-current="page"]') as HTMLElement | null)?.focus());
    }
  });

  // D15: a gated section that is off here shows its explanatory page in its place (one view per page, made on demand).
  const offViews = new Map<Page, GateOffView>();
  let current: string | null = null;
  const render = () => {
    const s = store.state;
    const off = gatedOff(s.page, s.caps?.gates);
    const place = placeOf(s.page, s.activityTab, s.returnTo);
    const toolId = place && 'tool' in place ? place.tool : null;
    const extId = place && 'ext' in place ? place.ext : null;
    if (toolId && toolForPage(s.page)) lastSection.set(toolId, s.page);
    for (const b of toolButtons) {
      const on = b.dataset.tool === toolId;
      b.classList.toggle('active', on);
      if (on) b.setAttribute('aria-current', 'page');
      else b.removeAttribute('aria-current');
    }
    for (const b of extButtons) {
      const on = b.dataset.ext === extId;
      b.classList.toggle('active', on);
      if (on) b.setAttribute('aria-current', 'page');
      else b.removeAttribute('aria-current');
    }
    moreBtn.classList.toggle('active', !!extId);
    // Section tabs of the current tool (detail pages keep their tool's sections visible).
    const tool = toolId ? toolOf(toolId) : null;
    const ext = extId ? EXTENSIONS.find((x) => x.id === extId) : null;
    const sectionKey = `${tool ? `${tool.id}:${s.page}` : `ext:${extId ?? ''}`}:${off ? 'off' : 'on'}:${locale()}`;
    if (sectionNav.dataset.key !== sectionKey) {
      sectionNav.dataset.key = sectionKey;
      sectionNav.setAttribute('aria-label', tool ? t('main.labelSections', { label: tool.label }) : t('main.currentLocation'));
      replace(
        sectionNav,
        tool ? h('span', { class: 'section-tool' }, icon(tool.icon), tool.label) : ext ? h('span', { class: 'section-tool' }, icon(ext.icon), ext.label) : null,
        ...(tool && tool.sections.length > 1
          ? tool.sections.map((sec) =>
              h(
                'button',
                {
                  type: 'button',
                  class: 'section-tab',
                  'aria-current': sec.page === s.page ? 'page' : null,
                  title: gatedOff(sec.page, s.caps?.gates) ? t('main.labelOffDefaultTurn', { label: sec.label }) : null,
                  onclick: () => app.navigate(sec.page),
                },
                sec.label,
              ),
            )
          : []),
        // Plain words in the bar ("实验功能" / "默认关闭"); the gate id stays in the settings list.
        tool ? gateBadge(off ? 'off' : 'experimental') : null,
      );
    }
    closingBanner.hidden = !s.closing;
    let view: { el: HTMLElement; update(): void; activate?(): void } = pages[s.page];
    if (off) {
      const v = offViews.get(s.page) ?? new GateOffView(app, s.page, off);
      offViews.set(s.page, v);
      v.show(off);
      view = v;
    }
    const key = `${s.page}:${off ? 'off' : 'on'}`;
    if (current !== key) {
      replace(main, view.el);
      current = key;
      closeMore();
      view.activate?.();
      window.scrollTo({ top: 0 });
    }
    view.update();
    chip.update();
    applyChip.update();
  };
  store.subscribe(render);
  // LIF-05: a language switch re-applies the shell's bound text and re-renders the current page in place; other pages
  // read their words afresh when they are shown. Machine fields, codes and states never change with the language.
  onLocale(() => {
    refreshLive();
    for (const p of Object.values(pages)) p.relocalize?.();
    syncThemeLabel(document.documentElement.dataset.theme ?? 'system');
    render();
  });

  // Startup: capabilities, scans recorded in the store, an operation still running here.
  const [recent, active] = await Promise.all([backend.recentScans().catch(() => []), backend.activeApply().catch(() => null)]);
  store.set({ caps: earlyCaps, recent });
  if (active?.running) {
    store.set({
      apply: { planId: active.plan_id, operationId: active.operation_id, total: active.total, counts: { ...EMPTY_COUNTS }, cancelRequested: active.cancel_requested, running: true, status: null, error: null, dismissed: false },
    });
  }
  const latest = recent.find((r) => r.state !== null);
  if (latest) await app.openScan(latest.scan_id, 'overview');
  else store.set({ page: 'scope' });
  if (backend.preview) {
    routeFromHash(app);
    window.addEventListener('hashchange', () => routeFromHash(app));
  }
  render();

  // Safety net while a task runs: refresh status at a low rate in case an event was missed.
  window.setInterval(() => {
    const t = store.state.task;
    if (t && !isTerminal(t.state) && store.state.scanId === t.scanId) void app.refreshStatus();
  }, 3000);
}

/** Browser preview only: `#cleanup`, `#plan=ID`, `#operation=ID`, `#activity/quarantine`... */
function routeFromHash(app: App): void {
  const hash = location.hash.replace(/^#/, '');
  if (!hash) return;
  const [page, arg] = hash.split(/[=/]/);
  if (page === 'plan' && arg) app.openPlan(arg, 'cleanup');
  else if (page === 'operation' && arg) app.openOperation(arg, 'activity');
  else if (page === 'activity') app.openActivity((arg as ActivityTab) || 'operations');
  else if (page === 'leftovers' && arg) app.store.set({ leftoversOp: arg, page: 'uninstall' });
  else if (page === 'diag') {
    // `#diag` history, `#diag=unconfigured`, `#diag=completed|failed|cancelled|expired|running`,
    // `#diag=video` (a target ready to preview). Fixture states only.
    const seed = (app.backend as unknown as { seed?: (s: string, sc?: string) => string | null }).seed;
    const entry = location.hash.match(/entry:(\d+)/)?.[1];
    if (entry) app.openDiagnosis({ scanId: 's_preview_1', entryId: entry, origin: 'disk', name: '' }, 'disk');
    else if (arg === 'video' || arg === 'cache') app.openDiagnosis({ scanId: 's_preview_1', entryId: arg === 'video' ? '5' : 'c', origin: arg === 'video' ? 'disk' : 'cleanup', name: arg === 'video' ? 'drone_raw_0412.mov' : 'Code Cache' }, arg === 'video' ? 'disk' : 'cleanup');
    else if (arg && seed) {
      const id = seed(arg, arg === 'expired' ? 'cache' : 'cache');
      if (id) app.store.set({ diagId: id, diagTarget: { scanId: 's_preview_1', entryId: 'c', origin: 'cleanup', name: 'Code Cache' }, page: 'diag', returnTo: 'cleanup' });
      else app.store.set({ diagTarget: { scanId: 's_preview_1', entryId: '5', origin: 'disk', name: 'drone_raw_0412.mov' }, diagId: null, page: 'diag', returnTo: 'disk' });
    } else app.navigate('diag');
  }
  else if (
    ['scope', 'overview', 'disk', 'insights', 'cleanup', 'purge', 'installer', 'recyclebin', 'apps', 'uninstall', 'updates', 'startup', 'maintenance', 'diagnosis', 'status', 'processes', 'resident', 'agent', 'settings'].includes(page)
  )
    app.navigate(page as Page);
}

void boot();
