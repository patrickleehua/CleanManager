import './styles.css';

import { createBackend, toIpcError, type Backend } from './lib/api';
import { appMark, h, icon, replace } from './lib/dom';
import { isTerminal, taskTitle } from './lib/format';
import { EMPTY_COUNTS, Store, type ActivityTab, type Page } from './lib/store';
import type { DiagTarget } from './lib/diagTypes';
import type { ReadinessOp, ScanStatus } from './lib/types';
import { OPERATION_STATE_LABEL } from './lib/words';
import { EXTENSIONS, TOOLS, gateLabel, gatedOff, placeOf, shortcutFor, shortcutLabel, toolForPage, toolOf, type ExtId, type ToolId } from './lib/nav';
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
import { ResidentPage } from './views/resident';
import { ensureReady } from './views/kit';
import { GateOffView } from './views/gateOff';

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
}

const THEME_KEY = 'cm.theme';

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
  const root = document.getElementById('app')!;
  let backend: Backend;
  try {
    backend = await createBackend();
  } catch (e) {
    replace(root, h('div', { class: 'fatal', role: 'alert' }, h('h1', {}, 'CleanManager 无法启动'), h('p', {}, String(e))));
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
      if (!(await ensureReady(app, 'scan', '扫描'))) return;
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
      announce(`已开始只读扫描：${res.root_display}`);
      await app.openScan(res.scan_id, 'overview');
    },
    async cancelScan(scanId) {
      const r = await backend.cancelScan(scanId);
      const t = store.state.task;
      if (t && t.scanId === scanId && r.accepted) {
        store.set({ task: { ...t, cancelRequested: true, state: t.state === 'running' ? 'stopping' : t.state } });
        announce('已请求停止扫描，正在安全停止');
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
      if (!(await ensureReady(app, readiness, '执行'))) return;
      const r = await backend.startApply(planId);
      if (!r.existing) {
        store.set({
          apply: { planId, operationId: r.operation_id, total: r.total, counts: { ...EMPTY_COUNTS }, cancelRequested: false, running: true, status: null, error: null, dismissed: false },
        });
        announce(`开始执行计划，共 ${r.total} 项`);
      } else {
        announce('这份计划已经执行过，显示已有的执行记录');
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
    const t = store.state.task;
    if (!t || t.scanId !== e.scan_id) return;
    const changedState = t.state !== e.state;
    store.set({ task: { ...t, state: e.state, entriesSeen: e.entries_seen, skippedRanges: e.skipped_ranges, cancelRequested: e.cancel_requested } });
    if (changedState && e.state === 'stopping') announce('正在停止扫描');
    if (store.state.scanId === e.scan_id && store.state.status && !isTerminal(store.state.status.state)) {
      store.set({ status: { ...store.state.status, state: e.state, entries_seen: e.entries_seen, skipped_ranges: e.skipped_ranges, cancel_requested: e.cancel_requested } });
    }
  });
  await backend.onFinished(async (e) => {
    const t = store.state.task;
    if (t && t.scanId === e.scan_id) {
      store.set({ task: { ...t, state: e.state ?? 'failed', error: e.error, dismissed: false } });
    }
    announce(e.state ? taskTitle(e.state) : `扫描未能开始：${e.error?.message ?? ''}`);
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
    announce(e.status ? `执行结束：${OPERATION_STATE_LABEL[e.status] ?? e.status}` : `执行未能开始：${e.error?.message ?? ''}`);
  });
  await backend.onDiagFinished((e) => {
    const word = e.state === 'completed' ? '已完成' : e.state === 'cancelled' ? '已取消' : e.state === 'failed' ? '失败' : e.state;
    announce(`AI 诊断${word}`);
    void (pages.diag as DiagPage).finished(e.diagnosis_id);
  });
  // Tray (RES-01): quick links open one of a fixed set of pages; alerts and changes refresh 常驻工具.
  await backend.onResidentNavigate((page) => {
    if (['status', 'processes', 'resident', 'cleanup'].includes(page)) app.navigate(page as Page);
  });
  await backend.onResidentChanged(() => (pages.resident as ResidentPage).changed());
  await backend.onResidentAlert((alerts) => {
    announce(alerts.map((a) => `电量低：${a.name} ${a.percent}%`).join('；'));
    (pages.resident as ResidentPage).alerted(alerts);
  });
  await backend.onClosing((e) => {
    store.set({ closing: true });
    announce(e.waiting_for.includes('operation') ? '正在停止后续操作，当前动作结束后关闭窗口' : '正在安全停止扫描，完成后关闭窗口');
  });

  // Shell: five tools, then the extensions (D14). Every entry keeps its place on every platform.
  const caps = capabilityPages(app);
  const pages: Record<Page, { el: HTMLElement; update(): void; activate?(): void }> = {
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
  const toolButtons = TOOLS.map((t) =>
    h(
      'button',
      { class: 'nav-item tool-item', type: 'button', 'data-tool': t.id, 'aria-keyshortcuts': ariaKeys(t.key), title: `${t.label}：${t.blurb}（${keyHint(t.key)}）`, onclick: () => openTool(t.id) },
      icon(t.icon),
      h('span', { class: 'nav-label' }, t.label),
      h('kbd', { class: 'nav-key', 'aria-hidden': 'true' }, keyHint(t.key)),
    ),
  );
  const extNav = h('nav', { class: 'ext-nav', id: 'ext-nav', 'aria-label': '扩展' });
  const moreBtn = h('button', { class: 'nav-item more-item', type: 'button', 'aria-expanded': 'false', 'aria-controls': 'ext-nav', title: '扩展：计划、活动、恢复、Agent、AI 诊断、设置' }, icon('ext-plans'), h('span', { class: 'nav-label' }, '更多'));
  const closeMore = () => {
    extNav.classList.remove('open');
    moreBtn.setAttribute('aria-expanded', 'false');
  };
  const extButtons = EXTENSIONS.map((e) =>
    h(
      'button',
      { class: 'nav-item ext-item', type: 'button', 'data-ext': e.id, 'aria-keyshortcuts': ariaKeys(e.key), title: `${e.label}（${keyHint(e.key)}）`, onclick: () => (closeMore(), openExt(e.id)) },
      icon(e.icon),
      h('span', { class: 'nav-label' }, e.label),
      h('kbd', { class: 'nav-key', 'aria-hidden': 'true' }, keyHint(e.key)),
    ),
  );
  replace(extNav, h('p', { class: 'nav-group-label', 'aria-hidden': 'true' }, '扩展'), ...extButtons);
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
  const themeBtn = h('button', { class: 'icon-btn', type: 'button', title: '主题：跟随系统', 'aria-label': '切换主题（当前跟随系统）' }, icon('theme'));
  const syncThemeLabel = (t: string) => {
    const label = t === 'system' ? '跟随系统' : t === 'dark' ? '深色' : '浅色';
    themeBtn.title = `主题：${label}`;
    themeBtn.setAttribute('aria-label', `切换主题（当前${label}）`);
  };
  syncThemeLabel(document.documentElement.dataset.theme ?? 'system');
  themeBtn.addEventListener('click', () => syncThemeLabel(cycleTheme()));
  const keysBtn = h('button', { class: 'icon-btn', type: 'button', title: '键盘快捷键（?）', 'aria-label': '键盘快捷键', 'aria-keyshortcuts': 'Shift+?' }, icon('keyboard'));
  const sidebar = h(
    'aside',
    { class: 'sidebar', 'aria-label': 'CleanManager' },
    h('div', { class: 'brand' }, appMark(28), h('span', { class: 'brand-name' }, 'CleanManager')),
    h('nav', { class: 'tool-nav', 'aria-label': '工具' }, ...toolButtons, moreBtn),
    extNav,
  );
  // Theme and shortcut sheet sit at the foot of the extension group (inside the "更多" menu on narrow windows).
  extNav.append(h('div', { class: 'side-foot' }, keysBtn, themeBtn));
  const sectionNav = h('nav', { class: 'section-nav', 'aria-label': '分区' });
  const chip = new TaskChip(app);
  const applyChip = new ApplyChip(app);
  const main = h('main', { id: 'main', tabindex: '-1' });
  const previewBanner = backend.preview
    ? h('div', { class: 'preview-banner', role: 'note' }, '浏览器预览：以下为开发用示例数据，不是本机扫描结果，也不会对任何文件执行操作。')
    : null;
  const closingBanner = h('div', { class: 'closing-banner', role: 'alert', hidden: true }, '正在安全停止。扫描会发布已确认的结果，执行会停止派发后续动作；完成后窗口关闭。');
  const skip = h('a', { class: 'skip-link', href: '#main' }, '跳到主要内容');
  const header = h('header', { class: 'topbar' }, sectionNav, h('div', { class: 'topbar-tools' }, applyChip.el, chip.el));
  replace(root, skip, h('div', { class: 'shell' }, sidebar, h('div', { class: 'main-col' }, previewBanner, header, closingBanner, main)), announcer);

  const showShortcuts = () => {
    if (document.querySelector('dialog.shortcuts[open]')) return;
    const row = (keys: string, what: string) => h('tr', {}, h('td', {}, h('kbd', {}, keys)), h('td', {}, what));
    const dlg = h(
      'dialog',
      { class: 'cm-dialog shortcuts', 'aria-labelledby': 'keys-title' },
      h('div', { class: 'dlg-head' }, h('h2', { id: 'keys-title' }, '键盘快捷键'), h('p', { class: 'muted' }, '两个平台相同：Windows 用 Ctrl，macOS 用 ⌘。')),
      h(
        'table',
        { class: 'keys-table' },
        h('caption', { class: 'sr-only' }, '快捷键与对应入口'),
        h(
          'tbody',
          {},
          ...TOOLS.map((t) => row(keyHint(t.key), `${t.label}：${t.blurb}`)),
          ...EXTENSIONS.map((e) => row(keyHint(e.key), e.label)),
          row(`${keyHint('PageUp')} / ${keyHint('PageDown')}`, '当前工具的上一个 / 下一个分区'),
          row('?', '显示这张表'),
          row('Esc', '关闭对话框或菜单'),
        ),
      ),
      h('div', { class: 'dlg-actions' }, h('button', { type: 'button', class: 'primary', onclick: () => dlg.close() }, '关闭')),
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
    const sectionKey = `${tool ? `${tool.id}:${s.page}` : `ext:${extId ?? ''}`}:${off ? 'off' : 'on'}`;
    if (sectionNav.dataset.key !== sectionKey) {
      sectionNav.dataset.key = sectionKey;
      sectionNav.setAttribute('aria-label', tool ? `${tool.label}分区` : '当前位置');
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
                  title: gatedOff(sec.page, s.caps?.gates) ? `${sec.label}：默认关闭，可在 设置 › 实验功能 中开启` : null,
                  onclick: () => app.navigate(sec.page),
                },
                sec.label,
              ),
            )
          : []),
        tool
          ? off
            ? h('span', { class: 'pill gate-pill', title: `这个分区尚未通过发布门禁 ${off.gate}，默认关闭；可在 设置 › 实验功能 中开启`, 'data-tone': 'muted' }, h('span', { class: 'pill-mark' }), `默认关闭 · ${off.gate} 未通过`)
            : h('span', { class: 'pill gate-pill', title: `${tool.label}的能力尚未通过发布门禁 ${tool.gate}：仅 Windows 开发构建，未发布`, 'data-tone': 'warn' }, h('span', { class: 'pill-mark' }), gateLabel(tool))
          : null,
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
