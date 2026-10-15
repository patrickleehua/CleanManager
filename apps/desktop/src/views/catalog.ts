// 清理 › 构建产物 / 安装包 (PUR-01..05, INC-01..05, GUI-09). Read-only discovery over the configured
// project roots or the declared installer locations, then the shared selection pattern: summary →
// groups → expandable rows with three-tier badges, sizes and reasons; a bottom selection bar; a
// plan draft with processing modes. Nothing is pre-selected; creating a plan executes nothing.

import type { App } from '../main';
import { groupDigits } from '../lib/bytes';
import { h, icon, keepFocus, replace } from '../lib/dom';
import { formatTime } from '../lib/format';
import { RequestGate } from '../lib/gate';
import type { Catalog, CatalogItem, CatalogPreview, InstallerLocations, IpcError, Mode, PurgeRoots } from '../lib/types';
import { LongProgress, TIER_EXPLAIN, atLeast, commonModes, ensureReady, modeOptions, noticeLine, selectionBar, sumSizes, tierBadge, toIpcError } from './kit';
import { confirmInline, emptyState, errorLine, exactTitle, pageHead, sizeText } from './parts';

const SOURCE_LABEL: Record<string, string> = {
  downloads: '下载文件夹',
  desktop: '桌面',
  browser_downloads: '浏览器下载位置',
};

const SKIP_LABEL: Record<string, string> = {
  hard_denied: '系统硬拒绝范围',
  protected: '受保护',
  vcs_tracked: '包含被版本控制跟踪的文件',
  vcs_unknown: '无法确认版本控制状态',
  mounted: '映像正在挂载',
  in_use: '正在被使用',
  probe_unknown: '无法确认是否在使用',
  partial_evidence: '证据不完整',
  recent: '项目最近修改过',
};

const REBUILD_LABEL: Record<string, string> = {
  local_rebuild: '本机重新构建即可恢复',
  network_redownload: '需要联网重新下载',
  not_rebuildable: '无法重新生成',
};

interface Copy {
  title: string;
  verb: string;
  op: 'purge' | 'installer';
  lede: string;
  empty: string;
}

const COPY: Record<Catalog, Copy> = {
  purge: {
    title: '构建产物',
    verb: '扫描构建产物',
    op: 'purge',
    lede: '在你的项目文件夹中，按项目标记（Cargo.toml、package.json 等）找出 target、node_modules、.next 这类可重新生成的构建输出。产物名本身不会让一个目录成为候选。',
    empty: '配置的项目文件夹中没有找到构建产物。没有项目标记的 build 目录不会被当作产物。',
  },
  installer: {
    title: '安装包',
    verb: '扫描安装包',
    op: 'installer',
    lede: '在下载文件夹、桌面和浏览器下载位置按文件内容识别安装程序与磁盘映像。安装包一律需要逐项审阅，不会被默认选择。',
    empty: '声明的位置中没有识别出安装包。',
  },
};

export class CatalogPage {
  el: HTMLElement;
  private body: HTMLElement;
  private roots: PurgeRoots | null = null;
  private locations: InstallerLocations | null = null;
  private rootsError: IpcError | null = null;
  private preview: CatalogPreview | null = null;
  private scanError: IpcError | null = null;
  private scanning = false;
  private selected = new Map<string, CatalogItem>();
  private expanded = new Set<string>();
  private collapsed = new Set<string>();
  private mode: Mode | null = null;
  private creating = false;
  private createError: IpcError | null = null;
  private notice: string | null = null;
  private removing: string | null = null;
  private progress: LongProgress;
  private gate = new RequestGate();
  private rootsGate = new RequestGate();
  private loaded = false;

  constructor(
    private app: App,
    private catalog: Catalog,
  ) {
    this.body = h('div', { class: 'catalog-body' });
    this.progress = new LongProgress(COPY[catalog].verb);
    this.el = h('section', { class: `page page-catalog page-${catalog}`, 'aria-labelledby': `${catalog}-title` }, this.body);
  }

  activate(): void {
    if (!this.loaded) {
      this.loaded = true;
      void this.loadRoots();
    }
  }

  update(): void {
    keepFocus(this.el, () => this.render());
  }

  // ---- data ------------------------------------------------------------------------------

  private async loadRoots(): Promise<void> {
    this.rootsError = null;
    try {
      if (this.catalog === 'purge') {
        const r = await this.rootsGate.latest(this.app.backend.purgeRoots());
        if (!r.stale) this.roots = r.value;
      } else {
        const r = await this.rootsGate.latest(this.app.backend.installerLocations());
        if (!r.stale) this.locations = r.value;
      }
    } catch (e) {
      this.rootsError = toIpcError(e);
    }
    this.update();
  }

  private async addRoot(): Promise<void> {
    this.rootsError = null;
    try {
      const r = await this.app.backend.purgeRootsAdd();
      if (r) {
        this.rootsGate.invalidate();
        this.roots = r;
        this.notice = '已添加扫描根目录。重新扫描后，它的项目会出现在下面。';
        this.app.announce('已添加扫描根目录');
      }
    } catch (e) {
      this.rootsError = toIpcError(e);
    }
    this.update();
  }

  private async removeRoot(rootId: string): Promise<void> {
    try {
      this.rootsGate.invalidate();
      this.roots = await this.app.backend.purgeRootsRemove(rootId);
      this.removing = null;
      this.notice = '已移除扫描根目录。磁盘上的文件没有变化。';
      this.app.announce('已移除扫描根目录');
    } catch (e) {
      this.rootsError = toIpcError(e);
    }
    this.update();
  }

  private async scan(): Promise<void> {
    if (this.scanning) return;
    const copy = COPY[this.catalog];
    if (!(await ensureReady(this.app, copy.op, copy.verb))) return;
    this.scanning = true;
    this.scanError = null;
    this.notice = null;
    this.progress.start(this.catalog === 'purge' ? '正在逐个扫描项目文件夹并识别项目标记…' : '正在扫描声明的位置并识别文件内容…');
    this.update();
    this.app.announce(`开始${copy.verb}，只读取，不修改任何文件`);
    try {
      const r = await this.gate.latest(this.catalog === 'purge' ? this.app.backend.purgeScan() : this.app.backend.installerScan());
      if (r.stale) return;
      this.preview = r.value;
      // A new discovery never inherits an old selection (different candidate sets).
      this.selected.clear();
      this.mode = null;
      this.app.announce(`扫描完成：${r.value.items.length} 个对象，未预先勾选任何项目`);
    } catch (e) {
      this.scanError = toIpcError(e);
    } finally {
      this.scanning = false;
      this.progress.stop();
      this.update();
    }
  }

  private toggle(i: CatalogItem): void {
    if (this.selected.has(i.candidate_id)) this.selected.delete(i.candidate_id);
    else if (i.plannable) this.selected.set(i.candidate_id, i);
    this.syncMode();
    this.app.announce(this.selected.has(i.candidate_id) ? `已选择 ${i.name}` : `已取消选择 ${i.name}`);
    this.update();
  }

  private selectGroup(items: CatalogItem[], label: string): void {
    let n = 0;
    for (const i of items) if (i.plannable && !this.selected.has(i.candidate_id)) (this.selected.set(i.candidate_id, i), (n += 1));
    this.syncMode();
    this.app.announce(n ? `已选择${label}中的 ${n} 项` : '没有可添加的项目');
    this.update();
  }

  private selectLowRisk(): void {
    const items = this.preview?.items.filter((i) => i.plannable && i.tier === 'default_selected') ?? [];
    this.selectGroup(items, '低风险项');
  }

  private syncMode(): void {
    const modes = commonModes([...this.selected.values()]);
    if (this.mode && !modes.includes(this.mode)) this.mode = null;
    this.createError = null;
  }

  private async createPlans(): Promise<void> {
    if (!this.mode || this.creating || !this.selected.size) return;
    this.creating = true;
    this.createError = null;
    this.update();
    const bySet = new Map<string, string[]>();
    for (const i of this.selected.values()) bySet.set(i.candidate_set_id, [...(bySet.get(i.candidate_set_id) ?? []), i.candidate_id]);
    const created: string[] = [];
    try {
      let stale = 0;
      for (const [set, ids] of bySet) {
        const r = await this.app.backend.createCatalogPlan(this.catalog, set, ids, this.mode);
        created.push(r.plan_id);
        stale += r.stale_items?.length ?? 0;
      }
      // Rows that changed since discovery are left out of the plan, never silently replaced.
      if (stale) this.app.announce(`${stale} 项在扫描后已变化，没有放入计划`);
      if (created.length === 1) {
        this.app.announce('计划已生成，尚未执行。请审阅完整清单');
        this.app.openPlan(created[0], this.catalog);
      } else {
        this.app.announce(`已生成 ${created.length} 份计划，每个位置一份，都尚未执行`);
        this.app.openActivity('plans');
      }
      this.selected.clear();
      this.mode = null;
    } catch (e) {
      this.createError = toIpcError(e);
      if (created.length) this.notice = `已生成 ${created.length} 份计划，其余未能生成。已生成的计划在“计划”中。`;
    } finally {
      this.creating = false;
      this.update();
    }
  }

  // ---- rendering ---------------------------------------------------------------------------

  private render(): void {
    const copy = COPY[this.catalog];
    const scanBtn = h('button', { type: 'button', class: this.preview ? '' : 'primary', id: `${this.catalog}-scan`, disabled: this.scanning }, this.scanning ? '正在扫描…' : this.preview ? '重新扫描' : copy.verb);
    scanBtn.addEventListener('click', () => void this.scan());
    const p = this.preview;
    const unsupported = (this.catalog === 'purge' ? this.roots?.supported : this.locations?.supported) === false || p?.supported === false;
    replace(
      this.body,
      pageHead({
        id: `${this.catalog}-title`,
        title: copy.title,
        meta: p ? [`扫描于 ${formatTime(p.observed_at)}`, `${groupDigits(p.items.length)} 个对象`, '只读发现，没有预先勾选'] : ['只读发现，没有预先勾选'],
        actions: unsupported ? [] : [scanBtn],
      }),
      this.notice ? noticeLine(this.notice, 'info', () => ((this.notice = null), this.update())) : null,
      unsupported
        ? emptyState('此平台暂不提供', '这项发现目前只在 Windows 上实现并经过验证。', null)
        : h(
            'div',
            { class: 'work-layout' },
            h('div', { class: 'work-main' }, this.progress.el, errorLine(this.scanError), p ? this.results(p) : this.intro(), this.rootsPanel()),
            this.draft(),
          ),
      this.bar(),
    );
  }

  private intro(): HTMLElement {
    const copy = COPY[this.catalog];
    return h(
      'section',
      { class: 'panel catalog-intro', 'aria-labelledby': `${this.catalog}-intro` },
      h('h2', { id: `${this.catalog}-intro` }, this.scanning ? '正在扫描' : '还没有扫描'),
      h('p', { class: 'lede' }, copy.lede),
      h('p', { class: 'muted small' }, '扫描只读取文件元数据并把结果存进 CleanManager 自己的索引；不会打开、移动或删除任何文件。'),
    );
  }

  private results(p: CatalogPreview): HTMLElement {
    if (!p.items.length) return h('div', { class: 'panel' }, emptyState('没有找到', COPY[this.catalog].empty, null), this.issues(p));
    const groups = new Map<string, CatalogItem[]>();
    for (const i of p.items) groups.set(i.group_label, [...(groups.get(i.group_label) ?? []), i]);
    const s = p.summary;
    const lowCount = p.items.filter((i) => i.plannable && i.tier === 'default_selected').length;
    const summary = h(
      'section',
      { class: 'panel catalog-summary', 'aria-labelledby': `${this.catalog}-sum` },
      h('h2', { id: `${this.catalog}-sum`, class: 'sr-only' }, '摘要'),
      h(
        'dl',
        { class: 'tier-summary' },
        h('div', {}, h('dt', {}, tierBadge('default_selected')), h('dd', { class: 'num' }, `${groupDigits(s.default_selected_count)} 项`, h('small', {}, s.default_selected_count ? atLeast({ known: BigInt(s.default_selected_known_bytes), unknown: s.default_selected_unknown_count }) : '—'))),
        h('div', {}, h('dt', {}, tierBadge('review_only')), h('dd', { class: 'num' }, `${groupDigits(s.review_only_count)} 项`, h('small', {}, '逐项确认'))),
        h('div', {}, h('dt', {}, tierBadge('blocked')), h('dd', { class: 'num' }, `${groupDigits(s.blocked_count)} 项`, h('small', {}, '只能查看'))),
      ),
      lowCount && this.catalog === 'purge' ? h('button', { type: 'button', class: 'quiet', id: 'select-low', onclick: () => this.selectLowRisk() }, `选择全部低风险项（${lowCount}）`) : null,
    );
    return h('div', { class: 'catalog-results' }, summary, this.issues(p), ...[...groups.entries()].map(([label, items], gi) => this.group(label, items, gi)));
  }

  private issues(p: CatalogPreview): HTMLElement | null {
    if (!p.issues.length) return null;
    return h(
      'details',
      { class: 'issues-details' },
      h('summary', {}, `扫描说明（${p.issues.length}）`),
      h('ul', { class: 'issue-list' }, ...p.issues.map((i) => h('li', {}, h('span', { class: 'issue-code' }, i.code), h('span', { class: 'path' }, i.scope_display), h('span', { class: 'muted' }, i.message)))),
    );
  }

  private group(label: string, items: CatalogItem[], gi: number): HTMLElement {
    const id = `${this.catalog}-g${gi}`;
    const open = !this.collapsed.has(label);
    const sum = sumSizes(items.map((i) => ({ bytes: i.logical_bytes, unknown: i.unknown_size_count })));
    const title = this.catalog === 'installer' ? (SOURCE_LABEL[label] ?? label) : label;
    const toggle = h(
      'button',
      { type: 'button', class: 'group-toggle', id: `${id}-toggle`, 'aria-expanded': open ? 'true' : 'false', 'aria-controls': `${id}-body` },
      h('span', { class: 'chev-icon', 'aria-hidden': 'true' }, icon('chevron')),
      h('span', { class: this.catalog === 'purge' ? 'path group-title' : 'group-title' }, title),
      h('span', { class: 'group-count num' }, `${groupDigits(items.length)} 项`),
      h('span', { class: 'group-size num' }, atLeast(sum)),
    );
    toggle.addEventListener('click', () => {
      if (open) this.collapsed.add(label);
      else this.collapsed.delete(label);
      this.update();
    });
    const plannable = items.filter((i) => i.plannable);
    const pick =
      this.catalog === 'installer' && plannable.length > 1
        ? h('button', { type: 'button', class: 'quiet', onclick: () => this.selectGroup(plannable, title) }, `选择此来源的全部（${plannable.length}）`)
        : null;
    return h(
      'section',
      { class: 'panel cgroup', 'aria-labelledby': `${id}-toggle` },
      h('div', { class: 'cgroup-head' }, h('h2', {}, toggle), pick),
      h('div', { class: 'group-body', id: `${id}-body`, hidden: !open }, this.table(items, title)),
    );
  }

  private table(items: CatalogItem[], caption: string): HTMLElement {
    const rows: HTMLElement[] = [];
    for (const i of items) {
      const id = `${this.catalog}-${i.candidate_id}`;
      const check = h('input', { type: 'checkbox', id: `${id}-check`, disabled: !i.plannable, 'aria-label': `选择 ${i.name}`, 'aria-describedby': `${id}-why` }) as HTMLInputElement;
      check.checked = this.selected.has(i.candidate_id);
      check.addEventListener('change', () => this.toggle(i));
      const open = this.expanded.has(i.candidate_id);
      const more = h('button', { type: 'button', class: 'link-btn', id: `${id}-more`, 'aria-expanded': open ? 'true' : 'false', 'aria-controls': `${id}-detail` }, open ? '收起依据' : '依据与影响');
      more.addEventListener('click', () => {
        if (open) this.expanded.delete(i.candidate_id);
        else this.expanded.add(i.candidate_id);
        this.update();
      });
      const reason = i.skip_reason ? (SKIP_LABEL[i.skip_reason] ?? i.skip_reason) : i.rebuild_class ? (REBUILD_LABEL[i.rebuild_class] ?? i.rebuild_class) : '';
      rows.push(
        h(
          'tr',
          { class: `crow${check.checked ? ' selected' : ''}${i.plannable ? '' : ' locked'}` },
          h('td', { class: 'col-check' }, check),
          h('td', { class: 'col-object' }, h('label', { for: `${id}-check`, class: 'obj-name' }, i.name), h('span', { class: 'obj-path path' }, i.path_display), h('span', { class: 'obj-reason', id: `${id}-why` }, reason)),
          h('td', { class: 'col-size num', title: exactTitle(i.logical_bytes) }, sizeText(i.logical_bytes, i.unknown_size_count)),
          h('td', { class: 'col-status' }, tierBadge(i.tier), more),
        ),
      );
      if (open) rows.push(h('tr', { class: 'cdetail', id: `${id}-detail` }, h('td', { colspan: '4' }, this.detail(i))));
    }
    return h(
      'div',
      { class: 'table-wrap' },
      h(
        'table',
        { class: 'data-table ctable' },
        h('caption', { class: 'sr-only' }, `${caption}：对象、大小与风险档位`),
        h('thead', {}, h('tr', {}, h('th', { scope: 'col', class: 'col-check' }, h('span', { class: 'sr-only' }, '选择')), h('th', { scope: 'col' }, '对象与依据'), h('th', { scope: 'col', class: 'col-size' }, '逻辑大小'), h('th', { scope: 'col' }, '档位'))),
        h('tbody', {}, ...rows),
      ),
    );
  }

  private detail(i: CatalogItem): HTMLElement {
    return h(
      'div',
      { class: 'cdetail-body' },
      h(
        'dl',
        { class: 'facts-list' },
        h('dt', {}, '档位'),
        h('dd', {}, TIER_EXPLAIN[i.tier], h('small', {}, i.tier_reason)),
        h('dt', {}, '影响'),
        h('dd', {}, i.impact || '没有额外说明'),
        i.rebuild_class ? h('dt', {}, '重新获取') : null,
        i.rebuild_class ? h('dd', {}, REBUILD_LABEL[i.rebuild_class] ?? i.rebuild_class) : null,
        i.unknown_size_count ? h('dt', {}, '未知大小') : null,
        i.unknown_size_count ? h('dd', {}, `${groupDigits(i.unknown_size_count)} 个文件大小未知，未计入上面的数字。`) : null,
        i.notes.length ? h('dt', {}, '依据') : null,
        i.notes.length ? h('dd', {}, h('ul', { class: 'plain-list' }, ...i.notes.map((n) => h('li', {}, n)))) : null,
      ),
    );
  }

  private rootsPanel(): HTMLElement {
    if (this.catalog === 'installer') {
      const l = this.locations;
      return h(
        'section',
        { class: 'panel roots-panel', 'aria-labelledby': 'inst-loc-title' },
        h('h2', { id: 'inst-loc-title' }, '查找位置'),
        h('p', { class: 'muted small' }, `只查找下面这些位置，向下最多 ${l?.max_depth ?? '—'} 层。与命令行 installer locations 一致。`),
        l
          ? h(
              'ul',
              { class: 'root-rows' },
              ...l.locations.map((x) => h('li', {}, h('span', { class: 'root-main' }, h('b', {}, SOURCE_LABEL[x.source_label] ?? x.source_label), h('span', { class: 'path muted' }, x.display)), x.exists ? null : h('span', { class: 'why-not' }, '不存在'))),
            )
          : h('p', { class: 'muted small', role: 'status' }, this.rootsError ? '' : '正在读取…'),
        errorLine(this.rootsError),
      );
    }
    const r = this.roots;
    const add = h('button', { type: 'button', id: 'purge-root-add' }, icon('reveal'), '添加文件夹…');
    add.addEventListener('click', () => void this.addRoot());
    return h(
      'section',
      { class: 'panel roots-panel', 'aria-labelledby': 'purge-roots-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'purge-roots-title' }, '项目扫描根目录'), add),
      h('p', { class: 'muted small' }, `只在这些文件夹里找项目，向下最多 ${r?.max_depth ?? '—'} 层；${r?.recent_days ?? '—'} 天内修改过的项目只列出、不归入低风险。与命令行 purge roots 使用同一份设置。`),
      r
        ? h(
            'ul',
            { class: 'root-rows' },
            ...r.roots.map((x) => {
              const rm = h('button', { type: 'button', class: 'quiet', id: `root-rm-${x.root_id}`, 'aria-label': `移除 ${x.display}` }, '移除');
              rm.addEventListener('click', () => {
                this.removing = x.root_id;
                this.update();
              });
              return h(
                'li',
                {},
                h('span', { class: 'root-main' }, h('span', { class: 'path' }, x.display), h('span', { class: 'muted small' }, x.source === 'default' ? '默认' : '你添加的', x.exists ? '' : '　·　当前不存在或卷离线，扫描时跳过')),
                this.removing === x.root_id
                  ? confirmInline({
                      id: `root-rm-${x.root_id}-c`,
                      prompt: '只从扫描设置中移除，不会改动磁盘上的文件。输入',
                      expected: '移除',
                      action: '确认移除',
                      onConfirm: () => this.removeRoot(x.root_id),
                      onCancel: () => ((this.removing = null), this.update()),
                    })
                  : rm,
              );
            }),
          )
        : h('p', { class: 'muted small', role: 'status' }, this.rootsError ? '' : '正在读取…'),
      r && !r.roots.length ? h('p', { class: 'small' }, '还没有扫描根目录。添加一个包含项目的文件夹后再扫描。') : null,
      errorLine(this.rootsError),
    );
  }

  private draft(): HTMLElement {
    const items = [...this.selected.values()];
    const n = items.length;
    const sum = sumSizes(items.map((i) => ({ bytes: i.logical_bytes, unknown: i.unknown_size_count })));
    const sets = new Set(items.map((i) => i.candidate_set_id)).size;
    const create = h(
      'button',
      { type: 'button', class: 'primary wide', id: `${this.catalog}-create`, disabled: n === 0 || !this.mode || this.creating },
      this.creating ? '正在冻结计划…' : n === 0 ? '先选择对象' : !this.mode ? '再选择处理方式' : sets > 1 ? `生成 ${sets} 份计划（${n} 项）` : `生成计划（${n} 项）`,
    );
    create.addEventListener('click', () => void this.createPlans());
    return h(
      'aside',
      { class: 'panel work-aside draft', 'aria-labelledby': `${this.catalog}-draft-title` },
      h('h2', { id: `${this.catalog}-draft-title` }, '选择草稿'),
      n === 0
        ? h('p', { class: 'muted small' }, '没有预先勾选任何项目。勾选对象后，在这里选择处理方式并生成计划。')
        : h(
            'div',
            { class: 'draft-sum' },
            h('p', { class: 'draft-figure num' }, atLeast(sum)),
            h('p', { class: 'muted small' }, `${groupDigits(n)} 项的已知逻辑大小${sum.unknown ? `；另有 ${groupDigits(sum.unknown)} 项大小未知，未计入` : ''}。`),
            sets > 1 ? h('p', { class: 'small' }, `来自 ${sets} 个位置：每个位置生成一份计划，分别审阅。`) : null,
          ),
      modeOptions({ name: `${this.catalog}-mode`, allowed: commonModes(items), current: this.mode, selected: n, onChange: (m) => ((this.mode = m), (this.createError = null), this.update()) }),
      errorLine(this.createError),
      create,
      h('p', { class: 'muted small aside-note' }, '生成计划只冻结清单，不会执行。下一步显示每一个文件、摘要和有效期，由你批准后才可执行。'),
    );
  }

  private bar(): HTMLElement {
    const items = [...this.selected.values()];
    const sum = sumSizes(items.map((i) => ({ bytes: i.logical_bytes, unknown: i.unknown_size_count })));
    return selectionBar({
      count: items.length,
      sum,
      sources: new Set(items.map((i) => i.candidate_set_id)).size,
      onClear: () => {
        this.selected.clear();
        this.syncMode();
        this.app.announce('已清空选择');
        this.update();
      },
      action: {
        id: `${this.catalog}-bar-go`,
        text: this.mode ? '生成计划并审阅' : '选择处理方式',
        disabled: this.creating,
        onClick: () => {
          if (this.mode) void this.createPlans();
          else document.querySelector<HTMLInputElement>(`#${this.catalog}-mode-modes input:not(:disabled)`)?.focus();
        },
      },
    });
  }
}
