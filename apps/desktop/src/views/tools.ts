// Pages of the five-tool IA that are not catalogs: 软件 › 启动项 (STA-01..04), 分析 › 空间洞察
// (DIN-01..04: known objects, overview cards and the old-downloads review), 状态 › 系统状态
// (STS-01..03, STS-05 alerts), and the capability pages of entries that stay command-line only
// (Agent, browsing the recycle bin). Writes only ever create plans for the shared review page.

import type { App } from '../main';
import { formatBytes, groupDigits } from '../lib/bytes';
import { h, icon, keepFocus, replace } from '../lib/dom';
import { formatTime } from '../lib/format';
import { RequestGate } from '../lib/gate';
import { words } from '../lib/platform';
import { DEFAULT_PREFS, PANEL_LABEL, REFRESH_SECONDS, type StatusPanel, type StatusPrefs, layoutRows, loadPrefs, movePanel, sanitize, savePrefs, setShown } from '../lib/statusPrefs';
import type { CpuAlert, Insight, InsightsList, IpcError, NetworkTotals, StartupItem, StartupList, StatusMetric, StatusSnapshot } from '../lib/types';
import type { DownloadItem, InsightCard, InsightOverview } from '../lib/suiteTypes';
import { atLeast, capabilityPage, ensureReady, noticeLine, selectionBar, sumSizes, toIpcError } from './kit';
import { copyField, emptyState, errorLine, exactTitle, pageHead, pill, type Tone } from './parts';

interface PageLike {
  el: HTMLElement;
  update(): void;
  activate?(): void;
}

// ---------------------------------------------------------------------------------------------
// 软件 › 启动项

const SOURCE_WORD: Record<string, string> = {
  run_key: '注册表 Run 项',
  startup_folder: '启动文件夹',
  scheduled_task: '计划任务',
  store_startup_task: '商店应用启动任务',
  sm_login_item: '登录项',
  sm_background_item: '后台项目',
  launchd_user: 'LaunchAgent',
  launchd_system: 'LaunchDaemon',
};

const STATE_WORD: Record<string, [string, Tone]> = {
  enabled: ['已启用', 'good'],
  disabled: ['已停用', 'muted'],
  broken: ['目标缺失', 'warn'],
  target_unreachable: ['目标暂不可达', 'warn'],
};

export class StartupPage implements PageLike {
  el: HTMLElement;
  private body: HTMLElement;
  private data: StartupList | null = null;
  private error: IpcError | null = null;
  private rowError = new Map<string, IpcError>();
  private busy: string | null = null;
  private gate = new RequestGate();
  private loading = false;

  constructor(private app: App) {
    this.body = h('div', { class: 'startup-body' });
    this.el = h('section', { class: 'page page-startup', 'aria-labelledby': 'startup-title' }, this.body);
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
      const r = await this.gate.latest(this.app.backend.startupList());
      if (r.stale) return;
      this.data = r.value;
    } catch (e) {
      this.error = toIpcError(e);
    } finally {
      this.loading = false;
      this.update();
    }
  }

  private async plan(i: StartupItem, op: 'disable' | 'enable'): Promise<void> {
    if (this.busy) return;
    if (!(await ensureReady(this.app, 'startup', op === 'disable' ? '停用启动项' : '启用启动项'))) return;
    this.busy = i.startup_item_id;
    this.rowError.delete(i.startup_item_id);
    this.update();
    try {
      const r = await this.app.backend.startupPlan(i.startup_item_id, op);
      this.app.announce('计划已生成，尚未执行。请审阅后批准');
      this.app.openPlan(r.plan_id, 'startup');
    } catch (e) {
      this.rowError.set(i.startup_item_id, toIpcError(e));
    } finally {
      this.busy = null;
      this.update();
    }
  }

  private render(): void {
    const d = this.data;
    const refresh = h('button', { type: 'button', id: 'startup-refresh', disabled: this.loading }, icon('refresh'), this.loading ? '正在读取…' : '重新读取');
    refresh.addEventListener('click', () => void this.load());
    const w = words();
    if (d && !d.supported) {
      replace(this.body, pageHead({ id: 'startup-title', title: '启动项' }), emptyState('此平台暂不提供', '启动项清单目前只在 Windows 上实现。', null));
      return;
    }
    replace(
      this.body,
      pageHead({
        id: 'startup-title',
        title: '启动项',
        meta: d ? [`${groupDigits(d.items.length)} 项`, d.is_admin ? `以${w.admin}运行` : '以当前用户运行', '只读清单'] : ['只读清单'],
        actions: [refresh],
      }),
      d && !d.is_admin ? noticeLine(`没有${w.admin}：所有用户共用的启动项只能查看。CleanManager 不会自动提权。`, 'warn') : null,
      errorLine(this.error),
      d
        ? h(
            'div',
            { class: 'work-layout' },
            h('div', { class: 'work-main' }, h('section', { class: 'panel', 'aria-labelledby': 'startup-list-title' }, h('h2', { id: 'startup-list-title' }, '登录时启动的程序'), this.table(d.items), this.issues(d))),
            h(
              'aside',
              { class: 'panel work-aside', 'aria-labelledby': 'startup-how' },
              h('h2', { id: 'startup-how' }, '怎样修改'),
              h('p', { class: 'small' }, `停用和启用只改系统的启用记录（与${w.startupSettings}相同），不删除程序或它的设置。`),
              h('p', { class: 'small' }, '每次修改都先生成计划，由你审阅批准后执行；执行前记录原来的状态，可在“活动”中生成还原计划。'),
              h('p', { class: 'muted small' }, '安全软件和系统组件只读显示。'),
            ),
          )
        : !this.error
          ? h('p', { class: 'muted', role: 'status' }, '正在读取启动项…')
          : null,
    );
  }

  private table(items: StartupItem[]): HTMLElement {
    if (!items.length) return h('p', { class: 'muted' }, '没有找到启动项。');
    const rows = items.map((i) => {
      const [word, tone] = STATE_WORD[i.state] ?? [i.state, 'muted' as Tone];
      const id = `st-${i.startup_item_id}`;
      const op: 'disable' | 'enable' = i.state === 'disabled' ? 'enable' : 'disable';
      const why = i.protected
        ? '受保护：只读'
        : i.toggle_capability === 'permission_required'
          ? `需要${words().admin}`
          : i.toggle_capability === 'unsupported'
            ? '没有可验证的修改方式'
            : i.toggle_capability === 'unverified'
              ? '修改方式未经验证'
              : null;
      const act = h('button', { type: 'button', class: 'quiet', id: `${id}-act`, disabled: !!why || this.busy !== null || i.state === 'broken' }, op === 'disable' ? '停用…' : '启用…');
      act.addEventListener('click', () => void this.plan(i, op));
      const err = this.rowError.get(i.startup_item_id);
      return h(
        'tr',
        {},
        h(
          'td',
          { class: 'col-object' },
          h('span', { class: 'obj-name' }, i.name),
          h('span', { class: 'obj-path path' }, i.target?.display ?? i.definition),
          h('span', { class: 'obj-reason muted' }, `${SOURCE_WORD[i.source] ?? i.source} · ${i.scope === 'user' ? '当前用户' : '所有用户'}${i.publisher ? ` · ${i.publisher}` : ' · 发布者未验证'}`),
          i.reason ? h('span', { class: 'why-not' }, i.reason) : null,
          err ? h('span', { class: 'form-error', role: 'alert' }, err.message) : null,
        ),
        h('td', { class: 'col-flags' }, pill(word, tone)),
        h('td', { class: 'col-status' }, act, why ? h('span', { class: 'why-not' }, why) : null),
      );
    });
    return h(
      'div',
      { class: 'table-wrap' },
      h(
        'table',
        { class: 'data-table startup-table' },
        h('caption', { class: 'sr-only' }, '启动项、状态与可用操作'),
        h('thead', {}, h('tr', {}, h('th', { scope: 'col' }, '程序与来源'), h('th', { scope: 'col', class: 'col-flags' }, '状态'), h('th', { scope: 'col', class: 'col-status' }, '操作'))),
        h('tbody', {}, ...rows),
      ),
    );
  }

  private issues(d: StartupList): HTMLElement | null {
    if (!d.issues.length) return null;
    return h(
      'details',
      { class: 'issues-details' },
      h('summary', {}, `未能读取的来源（${d.issues.length}）——清单可能不完整`),
      h('ul', { class: 'issue-list' }, ...d.issues.map((i) => h('li', {}, h('span', { class: 'issue-code' }, i.code), h('span', { class: 'path' }, i.scope_display), h('span', { class: 'muted' }, i.message)))),
    );
  }
}

// ---------------------------------------------------------------------------------------------
// 分析 › 空间洞察

const INSIGHT_TITLE: Record<string, [string, string]> = {
  'windows.hibernation_file': ['休眠文件', '保存休眠和快速启动时的内存内容。'],
  'windows.page_file': ['分页文件', '虚拟内存，由系统按需调整大小。'],
  'windows.swap_file': ['应用交换文件', '商店应用挂起时使用的交换空间。'],
  'windows.component_store': ['组件存储（WinSxS）', 'Windows 组件与更新的存储；资源管理器显示的大小包含硬链接，并非实际占用。'],
  'windows.shadow_storage': ['还原点与卷影副本', '系统还原点和卷影复制使用的空间。'],
  'windows.wsl_disk': ['WSL 虚拟磁盘', '在 WSL 中删除文件不会自动缩小这个磁盘文件。'],
  'windows.docker_disk': ['Docker 数据磁盘', '镜像、容器和卷所在的虚拟磁盘。'],
  'windows.hyperv_disks': ['Hyper-V 虚拟磁盘', '虚拟机的磁盘文件。'],
  'windows.previous_installation': ['旧版 Windows（Windows.old）', '升级后保留的旧系统，用于回退。'],
  'windows.delivery_optimization_cache': ['传递优化缓存', 'Windows 更新的点对点分发缓存。'],
  'macos.local_snapshots': ['本地快照', 'Time Machine 等在启动磁盘上保留的 APFS 本地快照；系统不报告它们占用的空间。'],
  'macos.docker_disk': ['Docker 磁盘映像', 'Docker Desktop 的镜像、容器和卷所在的磁盘映像。'],
  'macos.orbstack_disk': ['OrbStack 数据映像', 'OrbStack 的容器与虚拟机数据。'],
  'macos.parallels_vms': ['Parallels 虚拟机', '虚拟机包及其磁盘。'],
  'macos.vmware_vms': ['VMware 虚拟机', '虚拟机包及其磁盘。'],
  'macos.utm_vms': ['UTM 虚拟机', '位于受保护的应用容器中，未读取大小。'],
  'macos.lima_vms': ['Lima 虚拟机', '轻量 Linux 虚拟机的磁盘与状态。'],
  'macos.colima_vms': ['Colima 虚拟机', 'Colima 容器运行时虚拟机的磁盘与状态。'],
  'macos.device_backups': ['设备备份', 'iPhone/iPad 的本地备份；读取需要“完全磁盘访问权限”。'],
  'macos.swap': ['交换文件', '内存不足时系统使用的交换空间，由系统管理。'],
  'macos.purgeable_space': ['可清除空间（估算）', '系统可在需要时自动释放的空间，按“重要用途可用容量”减去可用容量估算。'],
  old_downloads: ['旧下载', '下载文件夹中较久未修改的文件。'],
};

const CARD_OF: Record<string, string> = {
  'windows.wsl_disk': '虚拟机与容器',
  'windows.docker_disk': '虚拟机与容器',
  'windows.hyperv_disks': '虚拟机与容器',
  'windows.shadow_storage': '还原点',
  'windows.hibernation_file': '系统文件',
  'windows.page_file': '系统文件',
  'windows.swap_file': '系统文件',
  'windows.component_store': '组件存储',
  'windows.previous_installation': '旧版系统',
  'windows.delivery_optimization_cache': '更新缓存',
  'macos.local_snapshots': '还原点',
  'macos.docker_disk': '虚拟机与容器',
  'macos.orbstack_disk': '虚拟机与容器',
  'macos.parallels_vms': '虚拟机与容器',
  'macos.vmware_vms': '虚拟机与容器',
  'macos.utm_vms': '虚拟机与容器',
  'macos.lima_vms': '虚拟机与容器',
  'macos.colima_vms': '虚拟机与容器',
  'macos.device_backups': '设备备份',
  'macos.swap': '系统文件',
  'macos.purgeable_space': '可清除空间',
  old_downloads: '旧下载',
};

/** DIN-03 cards: known size or unknown, and when it was observed. */
export function insightCards(items: Insight[]): { title: string; size: string; observed: string | null; count: number }[] {
  const cards = new Map<string, Insight[]>();
  for (const i of items) {
    const c = CARD_OF[i.kind] ?? '其他';
    cards.set(c, [...(cards.get(c) ?? []), i]);
  }
  return [...cards.entries()].map(([title, members]) => {
    const sum = sumSizes(members.map((m) => ({ bytes: m.logical_bytes })));
    const observed = members.map((m) => m.observed_at).filter((x): x is string => !!x).sort()[0] ?? null;
    return { title, size: sum.known === 0n && sum.unknown ? '大小未知' : atLeast(sum), observed, count: members.length };
  });
}

/** DIN-03 overview card titles by kind (the host's cards: known size or unknown, count, oldest observation). */
export const CARD_TITLE: Record<string, string> = {
  old_downloads: '旧下载',
  virtual_machines: '虚拟机与容器',
  snapshots: '还原点与快照',
  system_files: '系统文件',
  component_store: '组件存储',
  previous_installation: '旧版系统',
  update_cache: '更新缓存',
  developer_tools: '开发工具缓存',
  model_libraries: 'AI 模型库',
  device_backups: '设备备份',
  purgeable_space: '可清除空间（估算）',
};

/** One overview card as shown: never a partial sum presented as the total. */
export function overviewCard(c: InsightCard): { title: string; size: string; note: string | null; count: number | null; observed: string | null } {
  const size = c.logical_bytes === null ? (c.count !== null ? `${c.count} 个` : '大小未知') : formatBytes(c.logical_bytes);
  return { title: CARD_TITLE[c.kind] ?? c.title, size, note: c.logical_bytes === null ? c.size_note : null, count: c.count, observed: c.observed_at };
}

const OLD_DAYS = 90;

export class InsightsPage implements PageLike {
  el: HTMLElement;
  private body: HTMLElement;
  private data: InsightsList | null = null;
  private overview: InsightOverview | null = null;
  private error: IpcError | null = null;
  private ovError: IpcError | null = null;
  private gate = new RequestGate();
  private ovGate = new RequestGate();
  private dlSelected = new Map<string, DownloadItem>();
  private creating = false;
  private createError: IpcError | null = null;

  constructor(private app: App) {
    this.body = h('div', {});
    this.el = h('section', { class: 'page page-insights', 'aria-labelledby': 'insights-title' }, this.body);
  }

  activate(): void {
    if (!this.data) void this.load();
  }

  update(): void {
    keepFocus(this.el, () => this.render());
  }

  private async load(): Promise<void> {
    this.error = null;
    this.ovError = null;
    const list = this.gate.latest(this.app.backend.insightsList());
    const ov = this.ovGate.latest(this.app.backend.insightsOverview(OLD_DAYS));
    try {
      const r = await list;
      if (!r.stale) this.data = r.value;
    } catch (e) {
      this.error = toIpcError(e);
    }
    this.update();
    try {
      const r = await ov;
      if (!r.stale) {
        this.overview = r.value;
        // A new review has new item ids: an old selection is never carried over.
        this.dlSelected.clear();
      }
    } catch (e) {
      this.ovError = toIpcError(e);
    }
    this.update();
  }

  private toggleDownload(d: DownloadItem): void {
    if (this.dlSelected.has(d.item_id)) this.dlSelected.delete(d.item_id);
    else this.dlSelected.set(d.item_id, d);
    this.createError = null;
    this.app.announce(this.dlSelected.has(d.item_id) ? `已选择 ${d.name}` : `已取消选择 ${d.name}`);
    this.update();
  }

  private async trashDownloads(): Promise<void> {
    if (this.creating || !this.dlSelected.size) return;
    if (!(await ensureReady(this.app, 'trash', words().moveToBin))) return;
    this.creating = true;
    this.createError = null;
    this.update();
    try {
      const r = await this.app.backend.downloadsTrashPlan([...this.dlSelected.keys()]);
      this.dlSelected.clear();
      this.app.announce('计划已生成，尚未执行。请审阅后批准');
      this.app.openPlan(r.plan_id, 'insights');
    } catch (e) {
      this.createError = toIpcError(e);
    } finally {
      this.creating = false;
      this.update();
    }
  }

  private render(): void {
    const d = this.data;
    const refresh = h('button', { type: 'button', id: 'insights-refresh' }, icon('refresh'), '重新测量');
    refresh.addEventListener('click', () => ((this.data = null), (this.overview = null), this.update(), void this.load()));
    const ov = this.overview;
    replace(
      this.body,
      pageHead({ id: 'insights-title', title: '空间洞察', meta: d ? [`观测于 ${formatTime(d.observed_at)}`, '系统对象只读说明，没有删除入口'] : ['系统对象只读说明，没有删除入口'], actions: [refresh] }),
      errorLine(this.error),
      d && !d.supported ? emptyState('此平台暂不提供', '空间洞察目前只在 Windows 上实现。', null) : null,
      d && d.supported
        ? h(
            'div',
            { class: 'insights' },
            ov && ov.cards.length
              ? h(
                  'ul',
                  { class: 'insight-cards', 'aria-label': '按类别汇总' },
                  ...ov.cards.map((c) => {
                    const v = overviewCard(c);
                    return h(
                      'li',
                      { class: 'icard', 'data-kind': c.kind },
                      h('span', { class: 'icard-title' }, v.title),
                      h('span', { class: 'icard-size num' }, v.size),
                      h('span', { class: 'icard-meta muted' }, [v.count !== null && c.logical_bytes !== null ? `${v.count} 个对象` : null, v.observed ? formatTime(v.observed) : null].filter(Boolean).join(' · ')),
                      v.note ? h('span', { class: 'icard-meta muted' }, v.note) : null,
                    );
                  }),
                )
              : h(
                  'ul',
                  { class: 'insight-cards', 'aria-label': '按类别汇总' },
                  ...insightCards(d.items).map((c) =>
                    h('li', { class: 'icard' }, h('span', { class: 'icard-title' }, c.title), h('span', { class: 'icard-size num' }, c.size), h('span', { class: 'icard-meta muted' }, `${c.count} 个对象${c.observed ? ` · ${formatTime(c.observed)}` : ''}`)),
                  ),
                ),
            errorLine(this.ovError),
            this.downloadsPanel(),
            h('p', { class: 'muted small' }, '下面这些对象由系统或其所属工具管理。CleanManager 只解释它们是什么、由谁管理，并给出官方工具的用法；不会运行这些命令，也不提供删除。'),
            ...d.items.map((i) => this.item(i)),
          )
        : !this.error
          ? h('p', { class: 'muted', role: 'status' }, '正在测量已知的大型系统对象…')
          : null,
      this.dlBar(),
    );
  }

  private downloadsPanel(): HTMLElement {
    const ov = this.overview;
    const dl = ov?.downloads ?? null;
    if (!ov) return h('section', { class: 'panel dl-panel', 'aria-labelledby': 'dl-title' }, h('h2', { id: 'dl-title' }, '旧下载'), h('p', { class: 'muted small', role: 'status' }, this.ovError ? '' : '正在列出下载文件夹中较久未修改的文件…'));
    if (!dl) return h('section', { class: 'panel dl-panel', 'aria-labelledby': 'dl-title' }, h('h2', { id: 'dl-title' }, '旧下载'), h('p', { class: 'muted small' }, '没有找到下载文件夹。'));
    const total = dl.total_logical_bytes === null ? '大小未知' : formatBytes(dl.total_logical_bytes);
    const rows = dl.items.map((x) => {
      const id = `dl-${x.item_id}`;
      const check = h('input', { type: 'checkbox', id: `${id}-check`, 'aria-label': `选择 ${x.name}` }) as HTMLInputElement;
      check.checked = this.dlSelected.has(x.item_id);
      check.addEventListener('change', () => this.toggleDownload(x));
      return h(
        'tr',
        { class: `crow${check.checked ? ' selected' : ''}` },
        h('td', { class: 'col-check' }, check),
        h(
          'td',
          { class: 'col-object' },
          h('label', { for: `${id}-check`, class: 'obj-name' }, x.name),
          h('span', { class: 'obj-path path' }, x.path_display),
          x.installer_like ? h('span', { class: 'obj-reason' }, h('span', { class: 'row-tag' }, '像安装包'), '也可以在“清理 › 安装包”中按内容审阅') : null,
        ),
        h('td', { class: 'col-size num', title: exactTitle(x.logical_bytes) }, formatBytes(x.logical_bytes)),
        h('td', { class: 'col-when' }, `${groupDigits(x.age_days)} 天前`, h('span', { class: 'why-not' }, formatTime(x.modified_at))),
      );
    });
    return h(
      'section',
      { class: 'panel dl-panel', 'aria-labelledby': 'dl-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'dl-title' }, `旧下载：超过 ${dl.older_than_days} 天没有修改`), h('span', { class: 'muted small num' }, `${groupDigits(dl.item_count)} 个文件，共 ${total}`)),
      h('p', { class: 'muted small' }, `只供审阅：没有预先勾选任何文件，按大小从大到小排列。勾选的文件会先生成“${words().moveToBin}”计划，由你审阅批准后才移动。`),
      dl.root_display ? h('p', { class: 'path small muted' }, dl.root_display) : null,
      dl.note ? noticeLine(dl.note, 'warn') : null,
      dl.items.length
        ? h(
            'div',
            { class: 'table-wrap' },
            h(
              'table',
              { class: 'data-table ctable dl-table' },
              h('caption', { class: 'sr-only' }, '下载文件夹中较久未修改的文件，按大小排序'),
              h('thead', {}, h('tr', {}, h('th', { scope: 'col', class: 'col-check' }, h('span', { class: 'sr-only' }, '选择')), h('th', { scope: 'col' }, '文件'), h('th', { scope: 'col', class: 'col-size' }, '逻辑大小'), h('th', { scope: 'col', class: 'col-when' }, '上次修改'))),
              h('tbody', {}, ...rows),
            ),
          )
        : h('p', { class: 'small' }, `没有超过 ${dl.older_than_days} 天没有修改的文件。`),
      dl.items.length < dl.item_count ? h('p', { class: 'muted small' }, `显示最大的 ${groupDigits(dl.items.length)} 个，共 ${groupDigits(dl.item_count)} 个；合计包括全部文件。`) : null,
      errorLine(this.createError),
    );
  }

  private dlBar(): HTMLElement {
    const items = [...this.dlSelected.values()];
    return selectionBar({
      count: items.length,
      sum: sumSizes(items.map((i) => ({ bytes: i.logical_bytes }))),
      onClear: () => {
        this.dlSelected.clear();
        this.app.announce('已清空选择');
        this.update();
      },
      action: { id: 'dl-bar-go', text: this.creating ? '正在冻结计划…' : `${words().moveToBin}…`, disabled: this.creating, onClick: () => void this.trashDownloads() },
    });
  }

  private item(i: Insight): HTMLElement {
    const [title, explain] = INSIGHT_TITLE[i.kind] ?? [i.title, i.explanation];
    const size = i.logical_bytes === null ? '大小未知' : formatBytes(i.logical_bytes);
    return h(
      'section',
      { class: 'panel insight', 'aria-labelledby': `ins-${i.insight_id}` },
      h(
        'div',
        { class: 'insight-head' },
        h('div', {}, h('h2', { id: `ins-${i.insight_id}` }, title), i.path ? h('p', { class: 'path muted small' }, i.path.display) : null),
        h('p', { class: 'insight-size num' }, size, i.count !== null ? h('small', {}, `${i.count} 个`) : null, i.size_note ? h('small', {}, i.size_note) : null),
      ),
      h('p', {}, explain),
      ...i.guidance.map((g) =>
        h(
          'div',
          { class: 'guidance' },
          h('p', { class: 'small' }, h('b', {}, g.tool), g.requires_admin ? `（需要${words().admin}）` : '', '：', g.impact),
          copyField('官方用法（CleanManager 不会替你运行）', g.steps),
        ),
      ),
    );
  }
}

/** One sustained-CPU alert in words (STS-05). */
export function alertText(a: CpuAlert): string {
  return `持续高占用：${a.name}（PID ${a.pid}）在 ${a.window_seconds} 秒内一直不低于整机处理器的 ${a.threshold_percent}%（最低 ${a.min_cpu_percent.toFixed(0)}%）`;
}

// ---------------------------------------------------------------------------------------------
// 状态 › 系统状态

function metric(s: StatusSnapshot, name: string): StatusMetric | undefined {
  return s.metrics?.find((m) => m.name === name);
}

function valueText(m: StatusMetric | undefined): string {
  if (!m || m.value === null || m.availability !== 'available') return '不可用';
  switch (m.unit) {
    case 'percent':
      return `${m.value.toFixed(m.value < 10 ? 1 : 0)}%`;
    case 'bytes':
      return formatBytes(BigInt(Math.round(m.value)));
    case 'bytes_per_second':
      return `${formatBytes(BigInt(Math.round(m.value)))}/s`;
    case 'celsius':
      return `${m.value.toFixed(0)} °C`;
    default:
      return String(m.value);
  }
}

const DIMENSION_LABEL: Record<string, string> = { cpu: '处理器', memory: '内存', disk: '系统盘空间', thermal: '温度', io: '磁盘繁忙' };

const DIAGNOSIS_LABEL: Record<string, string> = {
  low_free_space: '系统盘可用空间偏低',
  disk_health_warning: '系统盘报告可能故障，请立即备份',
  high_cpu: '处理器占用很高',
  memory_pressure: '内存紧张',
  battery_low: '电量低且未充电',
  battery_worn: '电池容量明显衰减',
  high_temperature: '温度偏高',
  heavy_io: '磁盘持续繁忙',
};

const NET_CLASS_WORD: Record<string, string> = { physical: '物理网卡', tunnel: '隧道', virtual: '虚拟网卡', unknown: '无法分类' };

/** What the network total covers, in words (STS-07). */
export function networkScope(n: NetworkTotals): string {
  if (n.classification === 'unavailable') return '无法区分网卡类型：合计是所有网卡之和，隧道或虚拟交换机的流量可能被重复计算。';
  if (n.may_double_count) return '合计包含无法分类的网卡，隧道或虚拟交换机的流量可能被重复计算。';
  const loop = n.loopback_excluded ? '，已排除回环' : '';
  return `合计只计物理网卡${loop}；隧道和虚拟网卡单独列出，不计入合计。`;
}

export class StatusPage implements PageLike {
  el: HTMLElement;
  private body: HTMLElement;
  private snap: StatusSnapshot | null = null;
  /** STS-05 alerts raised while this page sampled (newest first, this session only). */
  private alerts: CpuAlert[] = [];
  private error: IpcError | null = null;
  private gate = new RequestGate();
  private timer: number | null = null;
  private paused = false;
  /** STS-07: panel visibility and order and the refresh interval, kept across sessions. */
  private prefs: StatusPrefs = loadPrefs();
  private prefsOpen = false;
  private prefsNote: string | null = null;

  constructor(private app: App) {
    this.body = h('div', {});
    this.el = h('section', { class: 'page page-status', 'aria-labelledby': 'status-title' }, this.body);
  }

  activate(): void {
    void this.sample();
    this.schedule();
  }

  update(): void {
    if (this.app.store.state.page !== 'status') this.stop();
    keepFocus(this.el, () => this.render());
  }

  private schedule(): void {
    this.stop();
    if (!this.paused) this.timer = window.setInterval(() => void this.sample(), this.prefs.refreshSeconds * 1000);
  }

  private stop(): void {
    if (this.timer !== null) window.clearInterval(this.timer);
    this.timer = null;
  }

  private async sample(): Promise<void> {
    if (this.app.store.state.page !== 'status') return this.stop();
    try {
      // Sampling runs at a fixed rate; a slow sample that returns after a newer one is dropped.
      const r = await this.gate.latest(this.app.backend.statusSnapshot());
      if (r.stale) return;
      this.snap = r.value;
      if (r.value.alerts?.length) {
        this.alerts = [...r.value.alerts, ...this.alerts].slice(0, 5);
        this.app.announce(r.value.alerts.map((x) => alertText(x)).join('；'));
      }
      this.error = null;
    } catch (e) {
      this.error = toIpcError(e);
    }
    if (this.app.store.state.page === 'status') this.update();
  }

  private setPrefs(next: StatusPrefs, said: string): void {
    const refreshChanged = next.refreshSeconds !== this.prefs.refreshSeconds;
    // The process sort belongs to the process page: keep whatever it stored meanwhile.
    this.prefs = sanitize({ ...next, processSort: loadPrefs().processSort });
    this.prefsNote = savePrefs(this.prefs) ? null : '这台电脑不允许保存显示设置；本次打开期间仍然生效。';
    if (refreshChanged) this.schedule();
    this.app.announce(said);
    this.update();
  }

  private render(): void {
    const s = this.snap;
    const pause = h('button', { type: 'button', id: 'status-pause', 'aria-pressed': this.paused ? 'true' : 'false' }, this.paused ? '继续刷新' : '暂停刷新');
    pause.addEventListener('click', () => {
      this.paused = !this.paused;
      this.schedule();
      this.update();
    });
    const customize = h('button', { type: 'button', id: 'status-prefs-toggle', 'aria-expanded': this.prefsOpen ? 'true' : 'false', 'aria-controls': 'status-prefs' }, icon('settings'), '显示设置');
    customize.addEventListener('click', () => {
      this.prefsOpen = !this.prefsOpen;
      this.update();
    });
    if (s && !s.supported) {
      replace(this.body, pageHead({ id: 'status-title', title: '系统状态' }), emptyState('此平台暂不提供', '状态采样目前只在 Windows 上实现。', null));
      return;
    }
    replace(
      this.body,
      pageHead({
        id: 'status-title',
        title: '系统状态',
        meta: s?.sampled_at ? [`采样于 ${formatTime(s.sampled_at)}`, s.stale ? '数据已过期' : this.paused ? '已暂停' : `每 ${this.prefs.refreshSeconds} 秒刷新`] : [],
        actions: [customize, pause],
      }),
      this.prefsOpen ? this.prefsPanel() : null,
      errorLine(this.error),
      ...this.alerts.map((x, i) =>
        h(
          'div',
          { class: 'notice warn alert-line', role: 'status' },
          h('span', {}, alertText(x)),
          h('span', { class: 'alert-actions' }, h('button', { type: 'button', class: 'quiet', onclick: () => (this.app.store.set({ processFocus: x.pid }), this.app.navigate('processes')) }, '查看进程'), h('button', { type: 'button', class: 'quiet', onclick: () => ((this.alerts = this.alerts.filter((_, j) => j !== i)), this.update()) }, '知道了')),
        ),
      ),
      s ? this.view(s) : !this.error ? h('p', { class: 'muted', role: 'status' }, '正在采样…') : null,
    );
  }

  private prefsPanel(): HTMLElement {
    const p = this.prefs;
    const refresh = h(
      'fieldset',
      { class: 'seg' },
      h('legend', { class: 'sr-only' }, '刷新间隔'),
      ...REFRESH_SECONDS.map((sec) => {
        const input = h('input', { type: 'radio', name: 'status-refresh', id: `status-refresh-${sec}`, value: String(sec) }) as HTMLInputElement;
        input.checked = p.refreshSeconds === sec;
        input.addEventListener('change', () => this.setPrefs({ ...p, refreshSeconds: sec }, `改为每 ${sec} 秒刷新`));
        return h('label', { for: `status-refresh-${sec}` }, input, h('span', {}, `${sec} 秒`));
      }),
    );
    const shownCount = p.order.length - p.hidden.length;
    const rows = p.order.map((panel, i) => {
      const shown = !p.hidden.includes(panel);
      const box = h('input', { type: 'checkbox', id: `status-show-${panel}` }) as HTMLInputElement;
      box.checked = shown;
      // At least one panel stays visible, so the page never turns blank.
      box.disabled = shown && shownCount === 1;
      box.addEventListener('change', () => this.setPrefs(setShown(p, panel, box.checked), `${box.checked ? '显示' : '隐藏'}${PANEL_LABEL[panel]}`));
      const move = (delta: -1 | 1) => {
        const id = `status-${delta < 0 ? 'up' : 'down'}-${panel}`;
        const edge = delta < 0 ? i === 0 : i === p.order.length - 1;
        const b = h('button', { type: 'button', class: 'icon-btn small-btn', id, disabled: edge, 'aria-label': `${PANEL_LABEL[panel]}${delta < 0 ? '上移' : '下移'}`, title: delta < 0 ? '上移' : '下移' }, icon(delta < 0 ? 'up' : 'down'));
        b.addEventListener('click', () => {
          const next = movePanel(p, panel, delta);
          this.setPrefs(next, `${PANEL_LABEL[panel]}移到第 ${next.order.indexOf(panel) + 1} 位`);
          // At the new edge this button is disabled; keep focus on the row's other move button.
          const now = document.getElementById(id) as HTMLButtonElement | null;
          if (now?.disabled) document.getElementById(`status-${delta < 0 ? 'down' : 'up'}-${panel}`)?.focus();
        });
        return b;
      };
      return h(
        'li',
        { 'data-shown': shown ? 'yes' : 'no' },
        h('label', { class: 'pref-panel', for: `status-show-${panel}` }, box, h('span', {}, PANEL_LABEL[panel])),
        h('span', { class: 'pref-move' }, move(-1), move(1)),
      );
    });
    const reset = h('button', { type: 'button', class: 'quiet', id: 'status-prefs-reset' }, '恢复默认');
    reset.addEventListener('click', () => this.setPrefs({ ...DEFAULT_PREFS }, '显示设置已恢复默认'));
    return h(
      'section',
      { class: 'panel status-prefs', id: 'status-prefs', 'aria-labelledby': 'status-prefs-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'status-prefs-title' }, '显示设置'), reset),
      h('div', { class: 'pref-grid' }, h('div', {}, h('h3', {}, '面板与顺序'), h('ol', { class: 'pref-panels' }, ...rows)), h('div', {}, h('h3', {}, '刷新间隔'), refresh, h('p', { class: 'muted small' }, '进程列表的排序在“进程”页选择，也会记住。'))),
      this.prefsNote ? noticeLine(this.prefsNote, 'warn') : h('p', { class: 'muted small' }, '设置只保存在这台电脑的本应用中，下次打开时按上次的样子显示。'),
    );
  }

  private view(s: StatusSnapshot): HTMLElement {
    const build: Record<StatusPanel, () => HTMLElement> = { health: () => this.healthPanel(s), metrics: () => this.metricsPanel(s), network: () => this.networkPanel(s) };
    return h('div', { class: 'status-layout' }, ...layoutRows(this.prefs).map((r) => h('div', { class: 'status-row', 'data-shape': r.shape }, ...r.panels.map((x) => build[x]()))));
  }

  private healthPanel(s: StatusSnapshot): HTMLElement {
    const hl = s.health;
    const diag = s.diagnosis;
    return h(
      'section',
      { class: 'panel health', 'aria-labelledby': 'health-title' },
      h('h2', { id: 'health-title' }, '健康分'),
      h('p', { class: 'health-score num' }, hl?.score === null || hl?.score === undefined ? '—' : String(hl.score), h('small', {}, '/ 100')),
      h('p', { class: 'muted small' }, '只反映当前的负载、空间与温度，不代表需要清理。'),
      hl
        ? h(
            'ul',
            { class: 'dims' },
            ...hl.dimensions.map((d) => {
              const bar = h('span', { class: 'dim-bar' }, h('span', { class: 'dim-fill' }));
              (bar.firstChild as HTMLElement).style.width = `${d.score ?? 0}%`;
              return h(
                'li',
                { 'data-low': d.score !== null && d.score < 60 ? 'yes' : 'no' },
                h('span', { class: 'dim-label' }, DIMENSION_LABEL[d.dimension] ?? d.dimension, h('small', { class: 'muted' }, `权重 ${hl.weights[d.dimension] ?? '—'}`)),
                bar,
                h('span', { class: 'dim-score num' }, d.score === null ? '无数据' : String(d.score)),
              );
            }),
          )
        : null,
      diag
        ? h(
            'div',
            { class: 'notice warn diag-line' },
            h('span', {}, DIAGNOSIS_LABEL[diag.code] ?? diag.message),
            diag.link === 'analyze' ? h('button', { type: 'button', class: 'quiet', onclick: () => this.app.navigate('overview') }, '去分析磁盘') : null,
          )
        : null,
    );
  }

  private metricsPanel(s: StatusSnapshot): HTMLElement {
    const tile = (label: string, name: string, extra?: string | null) => {
      const m = metric(s, name);
      return h('li', { class: 'mtile', 'data-available': m && m.availability === 'available' ? 'yes' : 'no' }, h('span', { class: 'mtile-label' }, label), h('span', { class: 'mtile-value num' }, valueText(m)), h('span', { class: 'mtile-note muted' }, m && m.availability !== 'available' ? (m.reason ?? '不可用') : (extra ?? '')));
    };
    const memTotal = metric(s, 'memory.total_bytes');
    const diskTotal = metric(s, 'disk.system.total_bytes');
    return h(
      'section',
      { class: 'panel metrics', 'aria-labelledby': 'metrics-title' },
      h('h2', { id: 'metrics-title' }, '指标'),
      h(
        'ul',
        { class: 'mtiles' },
        tile('处理器', 'cpu.total_percent', metric(s, 'cpu.logical_count') ? `${metric(s, 'cpu.logical_count')!.value} 个逻辑处理器` : null),
        tile('内存已用', 'memory.used_percent', memTotal?.value ? `共 ${valueText(memTotal)}，可用 ${valueText(metric(s, 'memory.available_bytes'))}` : null),
        tile('系统盘可用', 'disk.system.free_bytes', diskTotal?.value ? `${valueText(metric(s, 'disk.system.free_percent'))}，共 ${valueText(diskTotal)}` : null),
        tile('磁盘繁忙', 'disk.busy_percent', `读 ${valueText(metric(s, 'disk.read_bytes_per_sec'))} · 写 ${valueText(metric(s, 'disk.write_bytes_per_sec'))}`),
        tile('最高温度', 'thermal.max_zone_celsius'),
        tile('显卡温度', 'gpu.temperature_celsius'),
        tile('电量', 'battery.charge_percent', metric(s, 'battery.on_ac_power')?.value ? '正在使用电源' : '使用电池'),
        tile('电池健康', 'battery.health_percent', '相对设计容量'),
      ),
      h('p', { class: 'muted small' }, '不可用的指标会写明原因，从不显示为 0。'),
    );
  }

  private networkPanel(s: StatusSnapshot): HTMLElement {
    const n = s.network;
    const head = h('h2', { id: 'network-title' }, '网络');
    if (!n) {
      const m = metric(s, 'network.received_bytes_per_sec');
      return h('section', { class: 'panel network', 'aria-labelledby': 'network-title' }, head, h('p', { class: 'muted small' }, m?.reason ?? '这次采样没有网络数据。'));
    }
    const rate = (v: number) => `${formatBytes(BigInt(Math.round(v)))}/s`;
    const row = (i: { name: string; class: string; received_bytes_per_sec: number; sent_bytes_per_sec: number }) =>
      h('tr', {}, h('td', { class: 'col-object' }, h('span', { class: 'obj-name' }, i.name)), h('td', { class: 'small' }, NET_CLASS_WORD[i.class] ?? i.class), h('td', { class: 'col-num-s num' }, rate(i.received_bytes_per_sec)), h('td', { class: 'col-num-s num' }, rate(i.sent_bytes_per_sec)));
    const table = (caption: string, items: typeof n.counted) =>
      h(
        'div',
        { class: 'table-wrap' },
        h(
          'table',
          { class: 'data-table net-table' },
          h('caption', { class: 'sr-only' }, caption),
          h('thead', {}, h('tr', {}, h('th', { scope: 'col' }, '网卡'), h('th', { scope: 'col' }, '类型'), h('th', { scope: 'col', class: 'col-num-s' }, '下载'), h('th', { scope: 'col', class: 'col-num-s' }, '上传'))),
          h('tbody', {}, ...items.map(row)),
        ),
      );
    const active = n.separate.filter((i) => i.received_bytes_per_sec > 0 || i.sent_bytes_per_sec > 0);
    const idle = n.separate.filter((i) => !active.includes(i));
    return h(
      'section',
      { class: 'panel network', 'aria-labelledby': 'network-title' },
      head,
      h(
        'ul',
        { class: 'mtiles' },
        h('li', { class: 'mtile' }, h('span', { class: 'mtile-label' }, n.may_double_count ? '下载合计（可能重复）' : '下载合计'), h('span', { class: 'mtile-value num' }, rate(n.received_bytes_per_sec))),
        h('li', { class: 'mtile' }, h('span', { class: 'mtile-label' }, n.may_double_count ? '上传合计（可能重复）' : '上传合计'), h('span', { class: 'mtile-value num' }, rate(n.sent_bytes_per_sec))),
      ),
      n.may_double_count ? noticeLine(networkScope(n), 'warn') : h('p', { class: 'muted small' }, networkScope(n)),
      n.counted.length ? table('计入合计的网卡', n.counted) : h('p', { class: 'muted small' }, '没有正在工作的物理网卡。'),
      n.separate.length ? h('h3', { class: 'net-sub' }, '单独列出，不计入合计') : null,
      active.length ? table('隧道与虚拟网卡', active) : null,
      idle.length ? h('p', { class: 'muted small' }, `另有 ${idle.length} 个当前无流量：${idle.map((i) => `${i.name}（${NET_CLASS_WORD[i.class] ?? i.class}）`).join('、')}`) : null,
    );
  }
}

// ---------------------------------------------------------------------------------------------
// Capability pages

export class CapabilityPageView implements PageLike {
  el: HTMLElement;
  constructor(
    id: string,
    private build: () => HTMLElement,
  ) {
    this.el = h('section', { class: `page page-${id}`, 'aria-labelledby': `${id}-title` });
  }
  update(): void {
    replace(this.el, this.build());
  }
}

export function capabilityPages(app: App): Record<'recyclebin' | 'agent', CapabilityPageView> {
  const w = () => words();
  return {
    recyclebin: new CapabilityPageView('recyclebin', () =>
      capabilityPage({
        id: 'recyclebin',
        title: w().bin,
        gate: 'cli_only',
        spec: 'MTR-01..05',
        gateName: 'G1',
        does: [`列出你的${w().bin}中的项目，显示原位置、删除时间和大小。`, `把选中的项目冻结成“永久删除”计划；审阅时需要额外确认不可恢复。`, `在“分析 › 磁盘”中选择对象，生成“${w().moveToBin}”计划。`],
        why: `手选${w().moveToBin}已经可用：在“分析 › 磁盘”选中对象后点“${w().moveToBin}…”。浏览和永久删除${w().bin}项目目前只在命令行提供，因为它需要一次单独的不可恢复确认，桌面版的确认界面还在开发。`,
        safety: ['只按扫描中的条目编号选择对象，界面从不传递路径。', `目标卷没有可用的${w().bin}时计划失败，绝不改为永久删除。`, '受保护的子项会被列出并阻止整个文件夹进入计划。'],
        cli: [
          { label: `列出${w().bin}`, command: 'cleanmanager trash list' },
          { label: '生成永久删除计划', command: 'cleanmanager trash purge --item ID' },
        ],
        actions: [h('button', { type: 'button', class: 'primary', onclick: () => app.navigate('disk') }, `去磁盘中选择要${w().moveToBin}的对象`)],
      }),
    ),
    agent: new CapabilityPageView('agent', () =>
      capabilityPage({
        id: 'agent',
        title: 'Agent',
        gate: 'cli_only',
        spec: 'CLI-01..12（agent-cli）',
        gateName: 'G1',
        does: ['让脚本和 AI Agent 用 JSON 调用同样的扫描、分析、计划与执行。', '计划编号与摘要在桌面和终端之间通用：在任一入口批准都对另一入口有效。', '自动化调用不能批准计划，只能等待人工批准或已启用的有限策略。'],
        why: 'Agent 接口就是命令行的 JSON 协议，现在可以直接使用。桌面里没有单独的 Agent 运行界面，以免绕过人工审阅。',
        cli: [
          { label: '查看能力', command: 'cleanmanager capabilities --json' },
          { label: '只读预览清理候选', command: 'cleanmanager clean --dry-run --json' },
          { label: '只读发现构建产物', command: 'cleanmanager purge --dry-run --json' },
          { label: '审阅一份计划（人工）', command: 'cleanmanager plan review PLAN_ID' },
        ],
      }),
    ),
  };
}
