import './styles.css';

import { createBackend, toIpcError, type Backend } from './lib/api';
import { h, icon, replace } from './lib/dom';
import { isTerminal, taskTitle } from './lib/format';
import { EMPTY_COUNTS, Store, type ActivityTab, type Page } from './lib/store';
import type { ScanStatus } from './lib/types';
import { OPERATION_STATE_LABEL } from './lib/words';
import { ActivityPage } from './views/activity';
import { AppsPage } from './views/apps';
import { CleanupPage } from './views/cleanup';
import { DiskPage } from './views/disk';
import { OperationPage } from './views/operation';
import { OverviewPage } from './views/overview';
import { PlanPage } from './views/plan';
import { ScopePage } from './views/scope';
import { SettingsPage } from './views/settings';
import { ApplyChip, TaskChip } from './views/task';

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
  /** Apply a stored, authorized plan; opens its result page. */
  applyPlan(planId: string): Promise<void>;
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

/** Pages whose nav highlight is another page's (detail pages inside a section). */
const SECTION_OF: Partial<Record<Page, Page>> = { plan: 'cleanup', operation: 'activity', scope: 'overview' };

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
    openActivity(tab) {
      store.set({ activityTab: tab, page: 'activity' });
    },
    async applyPlan(planId) {
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
  await backend.onClosing((e) => {
    store.set({ closing: true });
    announce(e.waiting_for.includes('operation') ? '正在停止后续操作，当前动作结束后关闭窗口' : '正在安全停止扫描，完成后关闭窗口');
  });

  // Shell.
  const pages: Record<Page, { el: HTMLElement; update(): void; activate?(): void }> = {
    scope: new ScopePage(app),
    overview: new OverviewPage(app),
    disk: new DiskPage(app),
    cleanup: new CleanupPage(app),
    plan: new PlanPage(app),
    operation: new OperationPage(app),
    apps: new AppsPage(app),
    activity: new ActivityPage(app),
    settings: new SettingsPage(app),
  };
  const navItems: { page: Page; label: string; icon: string }[] = [
    { page: 'overview', label: '概览', icon: 'overview' },
    { page: 'disk', label: '磁盘', icon: 'disk' },
    { page: 'cleanup', label: '清理', icon: 'cleanup' },
    { page: 'apps', label: '应用', icon: 'apps' },
    { page: 'activity', label: '活动', icon: 'activity' },
  ];
  const navButtons = navItems.map((n) =>
    h('button', { class: 'nav-item', type: 'button', 'data-page': n.page, onclick: () => app.navigate(n.page) }, icon(n.icon), h('span', {}, n.label)),
  );
  const settingsBtn = h('button', { class: 'icon-btn', type: 'button', 'data-page': 'settings', title: '设置', 'aria-label': '设置' }, icon('settings'));
  settingsBtn.addEventListener('click', () => app.navigate('settings'));
  const themeBtn = h('button', { class: 'icon-btn', type: 'button', title: '主题：跟随系统', 'aria-label': '切换主题（当前跟随系统）' }, icon('theme'));
  themeBtn.addEventListener('click', () => {
    const t = cycleTheme();
    const label = t === 'system' ? '跟随系统' : t === 'dark' ? '深色' : '浅色';
    themeBtn.title = `主题：${label}`;
    themeBtn.setAttribute('aria-label', `切换主题（当前${label}）`);
  });
  const chip = new TaskChip(app);
  const applyChip = new ApplyChip(app);
  const main = h('main', { id: 'main', tabindex: '-1' });
  const previewBanner = backend.preview
    ? h('div', { class: 'preview-banner', role: 'note' }, '浏览器预览：以下为开发用示例数据，不是本机扫描结果，也不会对任何文件执行操作。')
    : null;
  const closingBanner = h('div', { class: 'closing-banner', role: 'alert', hidden: true }, '正在安全停止。扫描会发布已确认的结果，执行会停止派发后续动作；完成后窗口关闭。');
  const skip = h('a', { class: 'skip-link', href: '#main' }, '跳到主要内容');
  const header = h(
    'header',
    { class: 'topbar' },
    h('div', { class: 'brand' }, h('span', { class: 'brand-mark', 'aria-hidden': 'true' }), h('span', { class: 'brand-name' }, 'CleanManager')),
    h('nav', { class: 'nav', 'aria-label': '主要页面' }, ...navButtons),
    h('div', { class: 'topbar-tools' }, applyChip.el, chip.el, settingsBtn, themeBtn),
  );
  replace(root, skip, previewBanner, header, closingBanner, main, announcer);

  let current: Page | null = null;
  const render = () => {
    const s = store.state;
    const section = s.page === 'plan' && s.returnTo === 'activity' ? 'activity' : (SECTION_OF[s.page] ?? s.page);
    for (const b of [...navButtons, settingsBtn]) {
      const active = b.dataset.page === section;
      b.classList.toggle('active', active);
      if (active) b.setAttribute('aria-current', 'page');
      else b.removeAttribute('aria-current');
    }
    closingBanner.hidden = !s.closing;
    if (current !== s.page) {
      replace(main, pages[s.page].el);
      current = s.page;
      pages[s.page].activate?.();
      window.scrollTo({ top: 0 });
    }
    pages[s.page].update();
    chip.update();
    applyChip.update();
  };
  store.subscribe(render);

  // Startup: capabilities, scans recorded in the store, an operation still running here.
  const [caps, recent, active] = await Promise.all([
    backend.capabilities().catch(() => null),
    backend.recentScans().catch(() => []),
    backend.activeApply().catch(() => null),
  ]);
  store.set({ caps, recent });
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
  else if (['scope', 'overview', 'disk', 'cleanup', 'apps', 'settings'].includes(page)) app.navigate(page as Page);
}

void boot();
