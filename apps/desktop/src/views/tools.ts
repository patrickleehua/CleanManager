// Pages of the five-tool IA that are not catalogs: 软件 › 启动项 (STA-01..04), 分析 › 空间洞察
// (DIN-01..03), 状态 › 系统状态 (STS-01..03), and the capability pages of features that keep
// their place while another gate is open (uninstall, updates, maintenance, processes, Agent,
// recycle bin). Writes only ever create plans for the shared review page.

import type { App } from '../main';
import { formatBytes, groupDigits } from '../lib/bytes';
import { h, icon, keepFocus, replace } from '../lib/dom';
import { formatTime } from '../lib/format';
import { RequestGate } from '../lib/gate';
import { words } from '../lib/platform';
import type { Insight, InsightsList, IpcError, StartupItem, StartupList, StatusMetric, StatusSnapshot } from '../lib/types';
import { atLeast, capabilityPage, ensureReady, noticeLine, sumSizes, toIpcError } from './kit';
import { copyField, emptyState, errorLine, pageHead, pill, type Tone } from './parts';

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

export class InsightsPage implements PageLike {
  el: HTMLElement;
  private body: HTMLElement;
  private data: InsightsList | null = null;
  private error: IpcError | null = null;
  private gate = new RequestGate();

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
    try {
      const r = await this.gate.latest(this.app.backend.insightsList());
      if (!r.stale) this.data = r.value;
    } catch (e) {
      this.error = toIpcError(e);
    }
    this.update();
  }

  private render(): void {
    const d = this.data;
    const refresh = h('button', { type: 'button', id: 'insights-refresh' }, icon('refresh'), '重新测量');
    refresh.addEventListener('click', () => ((this.data = null), this.update(), void this.load()));
    replace(
      this.body,
      pageHead({ id: 'insights-title', title: '空间洞察', meta: d ? [`观测于 ${formatTime(d.observed_at)}`, '只读说明，没有删除入口'] : ['只读说明，没有删除入口'], actions: [refresh] }),
      errorLine(this.error),
      d && !d.supported ? emptyState('此平台暂不提供', '空间洞察目前只在 Windows 上实现。', null) : null,
      d && d.supported
        ? h(
            'div',
            { class: 'insights' },
            h(
              'ul',
              { class: 'insight-cards', 'aria-label': '按类别汇总' },
              ...insightCards(d.items).map((c) =>
                h('li', { class: 'icard' }, h('span', { class: 'icard-title' }, c.title), h('span', { class: 'icard-size num' }, c.size), h('span', { class: 'icard-meta muted' }, `${c.count} 个对象${c.observed ? ` · ${formatTime(c.observed)}` : ''}`)),
              ),
            ),
            h('p', { class: 'muted small' }, '这些对象由系统或其所属工具管理。CleanManager 只解释它们是什么、由谁管理，并给出官方工具的用法；不会运行这些命令，也不提供删除。'),
            ...d.items.map((i) => this.item(i)),
          )
        : !this.error
          ? h('p', { class: 'muted', role: 'status' }, '正在测量已知的大型系统对象…')
          : null,
    );
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

// ---------------------------------------------------------------------------------------------
// 状态 › 系统状态

const REFRESH_MS = 3000;

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

export class StatusPage implements PageLike {
  el: HTMLElement;
  private body: HTMLElement;
  private snap: StatusSnapshot | null = null;
  private error: IpcError | null = null;
  private gate = new RequestGate();
  private timer: number | null = null;
  private paused = false;

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
    if (!this.paused) this.timer = window.setInterval(() => void this.sample(), REFRESH_MS);
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
      this.error = null;
    } catch (e) {
      this.error = toIpcError(e);
    }
    if (this.app.store.state.page === 'status') this.update();
  }

  private render(): void {
    const s = this.snap;
    const pause = h('button', { type: 'button', id: 'status-pause', 'aria-pressed': this.paused ? 'true' : 'false' }, this.paused ? '继续刷新' : '暂停刷新');
    pause.addEventListener('click', () => {
      this.paused = !this.paused;
      this.schedule();
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
        meta: s?.sampled_at ? [`采样于 ${formatTime(s.sampled_at)}`, s.stale ? '数据已过期' : this.paused ? '已暂停' : `每 ${REFRESH_MS / 1000} 秒刷新`] : [],
        actions: [pause],
      }),
      errorLine(this.error),
      s ? this.view(s) : !this.error ? h('p', { class: 'muted', role: 'status' }, '正在采样…') : null,
    );
  }

  private view(s: StatusSnapshot): HTMLElement {
    const hl = s.health;
    const tile = (label: string, name: string, extra?: string | null) => {
      const m = metric(s, name);
      return h('li', { class: 'mtile', 'data-available': m && m.availability === 'available' ? 'yes' : 'no' }, h('span', { class: 'mtile-label' }, label), h('span', { class: 'mtile-value num' }, valueText(m)), h('span', { class: 'mtile-note muted' }, m && m.availability !== 'available' ? (m.reason ?? '不可用') : (extra ?? '')));
    };
    const memTotal = metric(s, 'memory.total_bytes');
    const diskTotal = metric(s, 'disk.system.total_bytes');
    const net = (s.metrics ?? []).filter((m) => m.name.startsWith('network.if.') && m.name.endsWith('received_bytes_per_sec'));
    const diag = s.diagnosis;
    return h(
      'div',
      { class: 'status-layout' },
      h(
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
      ),
      h(
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
          ...net.slice(0, 2).map((m) => {
            const name = m.name.slice('network.if.'.length, -'.received_bytes_per_sec'.length);
            return tile(`网络 ${name}`, m.name, `上传 ${valueText(metric(s, `network.if.${name}.sent_bytes_per_sec`))}`);
          }),
        ),
        h('p', { class: 'muted small' }, '不可用的指标会写明原因，从不显示为 0。'),
      ),
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

export function capabilityPages(app: App): Record<'recyclebin' | 'uninstall' | 'updates' | 'maintenance' | 'processes' | 'agent', CapabilityPageView> {
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
    uninstall: new CapabilityPageView('uninstall', () =>
      capabilityPage({
        id: 'uninstall',
        title: '卸载',
        gate: 'in_progress',
        spec: 'APP-05..09（app-uninstall）',
        gateName: 'G3',
        does: ['运行应用自己的卸载程序，保留它的界面与退出码。', '卸载后重新扫描，只把确实留下的文件、注册表项和启动项列为“残留”。', '残留用同一套三档徽标和选择栏审阅；注册表项先备份再删除。'],
        why: '卸载的两段计划（卸载程序 → 残留审阅）和注册表备份正在开发和实测，尚未通过发布门禁。在此之前，这里不会启动任何卸载程序。',
        safety: ['共享运行库与系统组件只读。', '残留只在卸载后的重新扫描中确认，不凭名称猜测。', '每一步都单独审阅、单独批准。'],
        cli: [{ label: '列出可卸载的应用（实验）', command: 'cleanmanager apps uninstall list' }],
        actions: [h('button', { type: 'button', onclick: () => app.navigate('apps') }, '查看已安装的应用')],
      }),
    ),
    updates: new CapabilityPageView('updates', () =>
      capabilityPage({
        id: 'updates',
        title: '更新',
        gate: 'in_progress',
        spec: 'UPD-01..04（app-updates）',
        gateName: 'G3',
        does: ['按应用的官方渠道（winget、商店、自带更新器）检查可用版本。', '列出版本差异与来源，更新同样先生成计划再执行。'],
        why: '更新渠道的探测与验证正在开发，尚未通过发布门禁；这里不会联网检查或下载任何东西。',
        safety: ['只使用应用的官方更新渠道。', '不自动更新，不在后台常驻。'],
        cli: [{ label: '检查更新（实验）', command: 'cleanmanager apps updates' }],
        actions: [h('button', { type: 'button', onclick: () => app.navigate('apps') }, '查看已安装的应用')],
      }),
    ),
    maintenance: new CapabilityPageView('maintenance', () =>
      capabilityPage({
        id: 'maintenance',
        title: '维护任务',
        gate: 'in_progress',
        spec: 'MNT-01..06（system-maintenance）',
        gateName: 'G3',
        does: ['列出系统维护任务（如清理组件存储、重建图标缓存、刷新 DNS），说明每项做什么、是否需要管理员、是否可撤销。', '按适用性探测：不适用的任务显示为“跳过”并说明原因，其他任务继续。', '需要管理员的任务集中在一次提权中执行，拒绝提权时只跳过这些任务。'],
        why: '维护任务目录与集中提权的工作进程正在开发，尚未通过发布门禁。这里不会运行任何系统命令。',
        safety: ['结果逐项显示：成功、跳过、失败或未知，并附原因。', '用户保护的任务一律跳过。', '不宣称“加速”，只说明实际做了什么。'],
        cli: [
          { label: '查看维护任务（实验）', command: 'cleanmanager optimize list' },
          { label: '只读诊断', command: 'cleanmanager optimize diagnose' },
        ],
      }),
    ),
    processes: new CapabilityPageView('processes', () =>
      capabilityPage({
        id: 'processes',
        title: '进程',
        gate: 'in_progress',
        spec: 'STS-04/05（system-status）',
        gateName: 'G3',
        does: ['按处理器、内存和磁盘占用列出进程，并标出所属应用。', '结束进程同样先生成计划；系统进程和安全软件只读。'],
        why: '进程清单与结束进程的计划正在开发，尚未通过发布门禁。系统指标已经可以在“系统状态”中查看。',
        cli: [{ label: '列出进程（实验）', command: 'cleanmanager process list --by cpu' }],
        actions: [h('button', { type: 'button', class: 'primary', onclick: () => app.navigate('status') }, '查看系统状态')],
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
