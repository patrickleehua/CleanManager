// P03 — disk: breadcrumb, one-layer treemap and the equivalent paged table, bound to the same
// scan_id + revision and entry IDs. Every query takes a ticket from a RequestGate; responses for
// an older ticket (left directory, changed sort/filter) are dropped. Selection is a map keyed by
// entry_id, frozen to one revision, and survives paging, filtering and drilling.
//
// The chart and the table are one view of the same layer: the table's current row lights its tile,
// choosing a tile (pointer or arrow keys) makes the same entry current in the table, and the detail
// pane follows both. Hierarchy of the detail pane: what it is → how big → what you can do.

import type { App } from '../main';
import { words } from '../lib/platform';
import { reviewTrash } from './trash';
import { toIpcError } from '../lib/api';
import { formatExactBytes, groupDigits, parseBytes } from '../lib/bytes';
import { h, icon, keepFocus, replace } from '../lib/dom';
import { ERROR_HINT, formatTime, isTerminal, STATE_LABEL, stateTone } from '../lib/format';
import { RequestGate } from '../lib/gate';
import { MERGED_ID } from '../lib/treemap';
import { HINT_LABEL, hintAction, hintBasisText, hintOf } from '../lib/cleanable';
import { categoryOf } from '../lib/category';
import { crumbTrail, DISK_TEXT, diskView, entryDetail, entryForTile, type DiskView } from '../lib/diskModel';
import { sizeLabel } from '../lib/display';
import type { CleanableHint, Entry, EntriesQuery, IpcError, KindFilter, SortKey, Summary, TreemapLayer } from '../lib/types';
import { canOpen, EntryGrid } from './grid';
import { REST_ID, TreemapView } from './treemapView';
import { refusalText } from '../lib/codes';
import { emptyBlock, pathText, sizeEl, stateLine, statusBanner, techBlock } from './ui';

const PAGE_SIZE = 200;
const TREEMAP_LIMIT = 120;
const LOCATE_PAGE_BUDGET = 10;

/** Words of the working page (the state words are in DISK_TEXT). */
const T = {
  crumbsLabel: '当前位置',
  up: '返回上一级',
  upKey: 'Backspace',
  more: '显示中间的文件夹',
  basis: '面积与排序按逻辑大小',
  known: '此文件夹的已知大小',
  unknownIn: (n: number) => `${groupDigits(n)} 项大小未知`,
  chart: '空间分布图',
  table: '条目表格',
  tableLabel: '当前文件夹的条目',
  side: '详情与选择',
  filters: '表格过滤与排序',
  scope: '范围',
  scopeLayer: '当前文件夹',
  scopeAll: '整个扫描',
  kind: '类型',
  kindAny: '全部',
  kindDir: '文件夹',
  kindFile: '文件',
  filter: '按名称过滤',
  sort: '排序',
  sortLogical: '逻辑大小（大到小）',
  sortAllocated: '分配大小（大到小）',
  sortName: '名称（A–Z）',
  showMore: '显示更多',
  detailTitle: '详情',
  detailEmpty: '在图或表格中选中一项，这里会显示它是什么、有多大。',
  logical: '逻辑大小',
  allocated: (s: string) => `分配 ${s}`,
  openFolder: '打开此文件夹',
  select: '选择',
  unselect: '取消选择',
  diag: 'AI 诊断',
  diagTip: '解释这个对象的用途与删除后果',
  techBasis: '大小依据',
  techIdentity: '身份',
  identityStrong: '卷 + 文件 ID',
  identityWeak: '较弱（仅供浏览，不作执行依据）',
  techExact: '精确逻辑大小',
  techEntry: '条目编号',
  hintNote: '只是提示：不会替你选中，也不改变保护或风险判断。',
  selected: (n: number) => `已选 ${groupDigits(n)} 项`,
  hiddenSelected: (n: number) => `其中 ${groupDigits(n)} 项不在当前表格中（其他文件夹或被过滤）。`,
  dedup: '去重后逻辑大小',
  unknownSel: (n: number) => `${groupDigits(n)} 项，未计入`,
  unknownSelLabel: '大小未知',
  overlap: '包含关系',
  overlapText: (n: number) => `${groupDigits(n)} 项位于另一已选文件夹内，只计一次`,
  summing: '正在汇总…',
  binNote: (moveToBin: string) => `可以把选中的对象${moveToBin}：先逐项检查保护与占用，再生成计划，由你审阅批准后才会移动。`,
  clear: '清除选择',
  showSelected: '查看全部已选',
  hideSelected: '收起已选',
  cleared: '已清除选择',
  opened: (n: string) => `已打开 ${n}`,
  back: (n: string) => `已返回 ${n}`,
  switched: '已切换到另一份扫描结果，之前的选择不会套用到新结果上。',
  revision: '扫描结果的版本已变化，页面已按新版本重新加载。',
  notInTable: '此条目不在表格当前的过滤或已加载范围内；详情已显示在右侧。清除过滤或继续加载可在表格中找到它。',
  restTile: '“其余”图块里的条目没有单独绘制；在表格中继续向下加载可以逐项看到它们。',
  mergedTile: (n: number) => `这个图块合并了 ${groupDigits(n)} 个较小的条目；表格已定位到其中最大的一项。`,
  emptyFiltered: '没有符合过滤条件的条目。清除过滤可查看全部。',
  emptyFolder: '这个文件夹是空的。',
  loadFailed: (r: string) => `无法加载：${r}`,
  chartFailed: (r: string) => `图表无法加载：${r}。表格仍可使用。`,
  count: (n: number) => `共 ${groupDigits(n)} 项`,
  loaded: (n: number) => `已加载 ${groupDigits(n)} 项，还有更多`,
  selectedHidden: (n: number, hidden: number) => `；已选 ${groupDigits(n)} 项${hidden ? `（${groupDigits(hidden)} 项不在当前表格中）` : ''}`,
  revisionTip: (r: number) => `扫描版本 ${r}`,
} as const;

interface Crumb {
  entryId: string;
  name: string;
}

export class DiskPage {
  el: HTMLElement;
  private header: HTMLElement;
  private crumbs: HTMLElement;
  private upBtn: HTMLButtonElement;
  private metaRow: HTMLElement;
  private stateNote: HTMLElement;
  private notice: HTMLElement;
  private controls: HTMLElement;
  private chartHead: HTMLElement;
  private details: HTMLElement;
  private selectionBox: HTMLElement;
  private gridStatus: HTMLElement;
  private moreBtn: HTMLButtonElement;
  private content: HTMLElement;
  private blocked: HTMLElement;
  private treemap: TreemapView;
  private grid: EntryGrid;

  // Binding: the scan and revision every query and the selection belong to.
  private scanId: string | null = null;
  private revision: number | null = null;
  private path: Crumb[] = [];
  private crumbsOpen = false;
  private scope: 'layer' | 'all' = 'layer';
  private sort: SortKey = 'logical_desc';
  private kind: KindFilter = 'any';
  private name = '';
  private nameTimer = 0;

  private rows: Entry[] = [];
  private cursor: string | null = null;
  private complete = false;
  private loading = false;
  private listError: IpcError | null = null;
  private layer: TreemapLayer | null = null;
  private selection = new Map<string, Entry>();
  private activeEntry: Entry | null = null;
  private summary: Summary | null = null;
  private summaryError: string | null = null;
  private showSelected = false;


  private listGate = new RequestGate();
  private layerGate = new RequestGate();
  private summaryGate = new RequestGate();

  constructor(private app: App) {
    this.crumbs = h('ol', { class: 'crumbs' });
    this.upBtn = h('button', { type: 'button', id: 'disk-up', class: 'icon-btn crumb-up', 'aria-label': T.up, title: `${T.up}（${T.upKey}）`, 'aria-keyshortcuts': 'Backspace' }, icon('back')) as HTMLButtonElement;
    this.upBtn.addEventListener('click', () => this.up());
    this.metaRow = h('p', { class: 'meta-row disk-meta' });
    this.stateNote = h('div', { class: 'disk-state' });
    this.notice = h('div', { class: 'notice', role: 'status', hidden: true });
    this.header = h(
      'div',
      { class: 'page-head disk-head' },
      h('div', { class: 'page-head-text' }, h('h1', { id: 'disk-title' }, DISK_TEXT.title), h('nav', { class: 'crumb-bar', 'aria-label': T.crumbsLabel }, this.upBtn, this.crumbs), this.metaRow),
    );
    this.treemap = new TreemapView({
      onSelect: (id, members, how) => this.fromTile(id, members, how),
      onOpen: (e) => this.drill(e, 'chart'),
      onUp: () => this.up('chart'),
      onToggle: (e) => this.toggle(e),
    });
    this.grid = new EntryGrid('disk', T.tableLabel, {
      onActive: (e) => this.setActive(e),
      onOpen: (e) => this.drill(e),
      onUp: () => this.up(),
      onToggle: (e) => this.toggle(e),
      onSort: (s) => this.setSort(s),
      onNeedMore: () => void this.loadMore(),
      isSelected: (id) => this.selection.has(id),
      onHover: (e) => this.treemap.setHover(e?.entry_id ?? null),
    });
    this.controls = h('div', { class: 'controls', role: 'group', 'aria-label': T.filters });
    this.gridStatus = h('p', { class: 'grid-status muted', 'aria-live': 'polite' });
    this.moreBtn = h('button', { type: 'button', id: 'disk-more', class: 'quiet' }, T.showMore);
    this.moreBtn.addEventListener('click', () => void this.loadMore());
    this.chartHead = h('div', { class: 'chart-head' });
    this.details = h('div', { class: 'details', 'aria-live': 'polite', 'aria-atomic': 'false' });
    this.selectionBox = h('div', { class: 'selection-box' });
    this.content = h(
      'div',
      { class: 'disk-layout' },
      h('section', { class: 'panel chart-panel', 'aria-label': T.chart }, this.chartHead, this.treemap.el),
      h(
        'section',
        { class: 'panel table-panel', 'aria-labelledby': 'table-title' },
        h('h2', { id: 'table-title', class: 'sr-only' }, T.table),
        this.controls,
        this.grid.el,
        h('div', { class: 'grid-foot' }, this.gridStatus, this.moreBtn),
      ),
      h('aside', { class: 'panel side-panel', 'aria-label': T.side }, this.details, this.selectionBox),
    );
    this.blocked = h('div', { class: 'disk-blocked' });
    this.el = h('section', { class: 'page page-disk', 'aria-labelledby': 'disk-title' }, this.header, this.stateNote, this.notice, this.blocked, this.content);
    this.renderControls();
  }

  activate(): void {
    // Restore keyboard position when returning to the page (filters/path/scroll were kept).
    if (this.grid.active) requestAnimationFrame(() => this.grid.setActive(this.grid.active!, true));
  }

  update(): void {
    const s = this.app.store.state;
    const status = s.status;
    const v = diskView({ scanId: s.scanId, status, statusError: s.statusError });

    if (v.layout !== 'ready' || !status) {
      this.content.hidden = true;
      this.blocked.hidden = false;
      this.header.classList.add('bare');
      replace(this.stateNote);
      this.renderBlocked(v);
      return;
    }
    this.header.classList.remove('bare');
    this.blocked.hidden = true;
    this.content.hidden = false;
    if (status.scan_id !== this.scanId || status.revision !== this.revision) {
      this.bind(status.scan_id, status.revision!, status.root_entry_id!, status.root_display ?? '扫描根目录');
    }
    this.renderStateNote(v);
    this.renderMeta();
    if (s.revealEntry) {
      const id = s.revealEntry;
      queueMicrotask(() => this.app.store.set({ revealEntry: null }));
      void this.reveal(id);
    }
  }

  /** Locate an entry named by another page (AI diagnosis "locate"): this layer first, then the
   *  whole revision by size. Read-only. */
  private async reveal(id: string): Promise<void> {
    await this.locate(id, true);
    if (this.grid.active === id) return;
    this.scope = 'all';
    this.name = '';
    this.renderControls();
    this.reloadAll();
    await this.locate(id);
  }

  // ---- binding and queries -------------------------------------------------------------

  private bind(scanId: string, revision: number, rootId: string, rootName: string): void {
    const hadSelection = this.selection.size > 0 && this.scanId !== null;
    this.scanId = scanId;
    this.revision = revision;
    this.path = [{ entryId: rootId, name: rootName }];
    this.crumbsOpen = false;
    this.scope = 'layer';
    this.selection.clear();
    this.summary = null;
    this.activeEntry = null;
    this.grid.reset();
    this.showNotice(hadSelection ? T.switched : null);
    this.renderControls();
    this.reloadAll();
  }

  private get layerId(): string {
    return this.path[this.path.length - 1].entryId;
  }

  private reloadAll(): void {
    this.renderHeader();
    void this.loadLayer();
    void this.loadFirstPage();
    this.renderSide();
  }

  private query(cursor: string | null): EntriesQuery {
    return {
      scan_id: this.scanId!,
      sort: this.sort,
      kind: this.kind,
      name_contains: this.name.trim() || null,
      parent_entry_id: this.scope === 'layer' ? this.layerId : null,
      page_size: PAGE_SIZE,
      cursor,
    };
  }

  private async loadLayer(): Promise<void> {
    const ticket = this.layerGate.next();
    this.layer = null;
    this.treemap.setLoading();
    this.renderChartHead();
    try {
      const res = await this.layerGate.run(ticket, this.app.backend.treemapLayer(this.scanId!, this.layerId, TREEMAP_LIMIT));
      if (res.stale) return;
      if (res.value.revision !== this.revision) return this.revisionChanged();
      this.layer = res.value;
      this.treemap.setLayer(res.value);
      this.treemap.setActive(this.activeEntry?.entry_id ?? null);
      this.renderChartHead();
      this.pushGrid();
    } catch (e) {
      const err = toIpcError(e);
      this.treemap.setError(T.chartFailed(refusalText(err)));
    }
  }

  private async loadFirstPage(): Promise<void> {
    const ticket = this.listGate.next();
    this.rows = [];
    this.cursor = null;
    this.complete = false;
    this.loading = true;
    this.listError = null;
    this.pushGrid();
    await this.fetchPage(ticket, null);
  }

  private async loadMore(): Promise<void> {
    if (this.loading || this.complete || !this.cursor) return;
    this.loading = true;
    this.pushGrid();
    await this.fetchPage(this.listGate.current(), this.cursor);
  }

  private async fetchPage(ticket: number, cursor: string | null): Promise<boolean> {
    try {
      const res = await this.listGate.run(ticket, this.app.backend.listEntries(this.query(cursor)));
      if (res.stale) return false;
      const page = res.value;
      if (page.revision !== this.revision) {
        this.revisionChanged();
        return false;
      }
      this.rows = cursor ? [...this.rows, ...page.items] : page.items;
      this.cursor = page.next_cursor;
      this.complete = page.next_cursor === null;
      this.loading = false;
      // Keep selected entries' details fresh when they come back into view.
      for (const it of page.items) if (this.selection.has(it.entry_id)) this.selection.set(it.entry_id, it);
      this.pushGrid();
      return true;
    } catch (e) {
      if (!this.listGate.isCurrent(ticket)) return false;
      const err = toIpcError(e);
      this.loading = false;
      if (err.reason === 'cursor_stale' && cursor) {
        this.showNotice(ERROR_HINT.cursor_stale);
        void this.loadFirstPage();
        return false;
      }
      this.listError = err;
      this.complete = true;
      this.pushGrid();
      return false;
    }
  }

  private revisionChanged(): void {
    this.showNotice(T.revision);
    void this.app.refreshStatus();
  }

  // ---- navigation ------------------------------------------------------------------------

  /** Open a folder. Focus stays in the widget the person used (the chart or the table). */
  private drill(e: Entry, from: 'chart' | 'table' = 'table'): void {
    if (!canOpen(e)) return;
    this.path = [...this.path, { entryId: e.entry_id, name: e.name }];
    this.scope = 'layer';
    this.grid.reset();
    this.activeEntry = null;
    this.renderControls();
    this.reloadAll();
    this.app.announce(T.opened(e.name));
    if (from === 'chart') this.treemap.focus();
    else this.grid.focus();
  }

  private goTo(depth: number, from: 'chart' | 'table' | 'crumb' = 'crumb'): void {
    if (depth >= this.path.length - 1) return;
    const left = this.path[depth + 1];
    this.path = this.path.slice(0, depth + 1);
    this.crumbsOpen = false;
    this.scope = 'layer';
    this.grid.reset();
    this.activeEntry = null;
    this.renderControls();
    this.reloadAll();
    this.app.announce(T.back(this.path[depth].name));
    if (from === 'chart') this.treemap.focus();
    else if (from === 'crumb') requestAnimationFrame(() => (document.getElementById(`crumb-${depth}`) ?? this.upBtn).focus());
    // Put the cursor back on the folder we came from once the page arrives.
    void this.locate(left.entryId, true);
  }

  private up(from: 'chart' | 'table' | 'crumb' = 'table'): void {
    if (this.path.length > 1) this.goTo(this.path.length - 2, from === 'crumb' ? 'table' : from);
  }

  private setSort(s: SortKey): void {
    if (s === this.sort) return;
    this.sort = s;
    this.renderControls();
    const keep = this.activeEntry?.entry_id;
    this.grid.reset();
    void this.loadFirstPage().then(() => (keep ? this.locate(keep, true) : undefined));
  }

  /** A tile became current: the table locates the same entry (never a look-alike). */
  private fromTile(id: string, members: string[] | undefined, how: 'pointer' | 'keyboard'): void {
    const target = entryForTile({ id, members }, REST_ID);
    if (!target.entryId) {
      this.showNotice(T.restTile);
      this.app.announce(T.restTile);
      return;
    }
    if (id === MERGED_ID && members?.length) {
      this.showNotice(T.mergedTile(members.length));
      if (how === 'keyboard') this.app.announce(T.mergedTile(members.length));
    } else this.showNotice(null);
    void this.locate(target.entryId, true);
  }

  /**
   * Bring `id` into the table: if it is not loaded yet, keep paging the *same* query (bounded),
   * so the row shown is the same entry the chart showed, never a look-alike.
   */
  private async locate(id: string, quiet = false): Promise<void> {
    const ticket = this.listGate.current();
    const deadline = Date.now() + 8000;
    let pages = 0;
    while (Date.now() < deadline && this.listGate.isCurrent(ticket)) {
      if (this.grid.setActive(id, true, true)) {
        if (!quiet) this.grid.focus();
        return;
      }
      if (this.loading) {
        await new Promise((r) => setTimeout(r, 50));
        continue;
      }
      if (this.complete || !this.cursor || pages >= LOCATE_PAGE_BUDGET) break;
      pages += 1;
      this.loading = true;
      const ok = await this.fetchPage(ticket, this.cursor);
      if (!ok) return;
    }
    if (!this.listGate.isCurrent(ticket)) return;
    const layerEntry = this.layer?.items.find((e) => e.entry_id === id);
    if (layerEntry) this.setActive(layerEntry);
    if (!quiet || layerEntry) this.showNotice(T.notInTable);
  }

  // ---- selection and details ---------------------------------------------------------------

  private setActive(e: Entry): void {
    this.activeEntry = e;
    this.treemap.setActive(e.entry_id);
    this.renderSide();
  }

  private toggle(e: Entry): void {
    if (this.selection.has(e.entry_id)) this.selection.delete(e.entry_id);
    else this.selection.set(e.entry_id, e);
    this.grid.refreshSelection();
    void this.loadSummary();
    this.renderSide();
  }

  private clearSelection(): void {
    this.selection.clear();
    this.summary = null;
    this.summaryGate.next();
    this.showSelected = false;
    this.grid.refreshSelection();
    this.renderSide();
    this.app.announce(T.cleared);
  }

  private async loadSummary(): Promise<void> {
    const ticket = this.summaryGate.next();
    this.summary = null;
    this.summaryError = null;
    if (!this.selection.size) return this.renderSide();
    try {
      const res = await this.summaryGate.run(ticket, this.app.backend.summarizeSelection(this.scanId!, [...this.selection.keys()].slice(0, 500)));
      if (res.stale) return;
      this.summary = res.value;
    } catch (e) {
      if (!this.summaryGate.isCurrent(ticket)) return;
      this.summaryError = refusalText(toIpcError(e));
    }
    this.renderSide();
  }

  // ---- rendering ---------------------------------------------------------------------------

  private showNotice(text: string | null): void {
    this.notice.hidden = !text;
    this.notice.textContent = text ?? '';
  }

  private pushGrid(): void {
    const filtered = this.kind !== 'any' || this.name.trim() !== '';
    const emptyText = this.listError ? T.loadFailed(refusalText(this.listError)) : filtered ? T.emptyFiltered : T.emptyFolder;
    const base = this.scope === 'layer' && this.layer ? parseBytes(this.layer.parent_summary.logical_known_bytes) : null;
    this.grid.setData({ rows: this.rows, complete: this.complete, loading: this.loading, shareBase: base, sort: this.sort, emptyText });
    const visibleSelected = this.rows.filter((r) => this.selection.has(r.entry_id)).length;
    this.gridStatus.textContent = this.listError
      ? ''
      : `${this.complete ? T.count(this.rows.length) : T.loaded(this.rows.length)}${this.selection.size ? T.selectedHidden(this.selection.size, this.selection.size - visibleSelected) : ''}`;
    this.moreBtn.hidden = this.complete;
    this.moreBtn.disabled = this.loading;
  }

  /** The trail of folders; long trails fold their middle into "…" (it opens the whole trail). */
  private renderHeader(): void {
    const names = this.path.map((c) => c.name);
    const trail = this.crumbsOpen ? crumbTrail(names, names.length) : crumbTrail(names);
    keepFocus(this.crumbs, () =>
      replace(
        this.crumbs,
        ...trail.map((c) => {
          if (c.kind === 'more') {
            const b = h('button', { type: 'button', id: 'crumb-more', class: 'crumb crumb-more', 'aria-label': `${T.more}（${groupDigits(c.depths.length)} 层）`, title: c.depths.map((d) => names[d]).join(' › ') }, '…');
            b.addEventListener('click', () => {
              this.crumbsOpen = true;
              this.renderHeader();
              requestAnimationFrame(() => document.getElementById(`crumb-${c.depths[0]}`)?.focus());
            });
            return h('li', {}, b);
          }
          const last = c.depth === this.path.length - 1;
          const label = c.depth === 0 ? pathText(c.name, { max: 36 }) : pathText(c.name, { max: 28 });
          const b = h('button', { type: 'button', id: `crumb-${c.depth}`, class: 'crumb', 'aria-current': last ? 'location' : null, disabled: last }, label);
          b.addEventListener('click', () => this.goTo(c.depth));
          return h('li', {}, b);
        }),
      ),
    );
    // At the root there is nowhere to go up to: the button leaves the trail rather than sit disabled.
    this.upBtn.hidden = this.path.length <= 1;
    this.renderChartHead();
  }

  /** One quiet line under the title: the scan's state word, when it ended, what the area means. */
  private renderMeta(): void {
    const st = this.app.store.state.status;
    if (!st) return replace(this.metaRow);
    const when = st.finished_at ?? st.started_at;
    replace(
      this.metaRow,
      // The state word only when no state line says it already.
      this.stateNote.childElementCount ? null : h('span', {}, h('span', { class: 'state-word', 'data-tone': stateTone(st.state) }, h('span', { class: 'sw-mark', 'aria-hidden': 'true' }), STATE_LABEL[st.state])),
      when ? h('span', { title: st.revision !== null ? T.revisionTip(st.revision) : null }, formatTime(when)) : null,
      h('span', {}, T.basis),
    );
  }

  private renderStateNote(v: DiskView): void {
    if (!v.note) return replace(this.stateNote);
    const go = h('button', { type: 'button', class: 'link', id: 'disk-reason' }, DISK_TEXT.goReason);
    go.addEventListener('click', () => this.app.navigate('overview'));
    replace(this.stateNote, stateLine({ kind: v.note.kind, text: v.note.text, actions: [v.action === 'overview' && v.note.kind !== 'cancelled' ? go : null] }));
  }

  /** The chart's own heading: the folder's known size (the page's one figure) and what is unknown. */
  private renderChartHead(): void {
    const sum = this.layer?.parent_summary;
    if (!sum) return replace(this.chartHead, h('p', { class: 'chart-figure muted', role: 'status' }, '…'));
    const unknown = sum.logical_unknown_count;
    replace(
      this.chartHead,
      h(
        'p',
        { class: 'chart-figure' },
        sizeEl(sum.logical_known_bytes, { className: 'cf-num' }),
        h('span', { class: 'cf-cap' }, T.known),
        unknown ? h('span', { class: 'cf-unknown' }, T.unknownIn(unknown)) : null,
      ),
    );
  }

  private renderControls(): void {
    const seg = <V extends string>(label: string, name: string, value: V, options: [V, string][], on: (v: V) => void) =>
      h(
        'fieldset',
        { class: 'seg' },
        h('legend', { class: 'sr-only' }, label),
        ...options.map(([v, text]) => {
          const id = `${name}-${v}`;
          const input = h('input', { type: 'radio', name, id, value: v });
          input.checked = v === value;
          input.addEventListener('change', () => on(v));
          return h('label', { for: id }, input, h('span', {}, text));
        }),
      );
    const search = h('input', { type: 'search', id: 'disk-filter', placeholder: T.filter, 'aria-label': T.filter, value: this.name, maxlength: '256' });
    search.addEventListener('input', () => {
      window.clearTimeout(this.nameTimer);
      this.nameTimer = window.setTimeout(() => {
        this.name = search.value;
        this.grid.reset();
        void this.loadFirstPage();
      }, 250);
    });
    const sortSel = h(
      'select',
      { id: 'disk-sort', 'aria-label': T.sort },
      h('option', { value: 'logical_desc' }, T.sortLogical),
      h('option', { value: 'allocated_desc' }, T.sortAllocated),
      h('option', { value: 'name_asc' }, T.sortName),
    );
    sortSel.value = this.sort;
    sortSel.addEventListener('change', () => this.setSort(sortSel.value as SortKey));
    keepFocus(this.controls, () =>
      replace(
        this.controls,
        seg(T.scope, 'disk-scope', this.scope, [['layer', T.scopeLayer], ['all', T.scopeAll]], (v) => {
          this.scope = v;
          this.grid.reset();
          void this.loadFirstPage();
        }),
        seg(T.kind, 'disk-kind', this.kind, [['any', T.kindAny], ['directory', T.kindDir], ['file', T.kindFile]], (v) => {
          this.kind = v;
          this.grid.reset();
          void this.loadFirstPage();
        }),
        search,
        sortSel,
      ),
    );
  }

  private renderSide(): void {
    keepFocus(this.el.querySelector('.side-panel') ?? this.details, () => {
      this.renderDetails();
      this.renderSelection();
    });
  }

  /** What it is → how big → what you can do; the evidence behind the size is in 技术详情. */
  private renderDetails(): void {
    const e = this.activeEntry;
    if (!e) {
      replace(this.details, h('h2', { class: 'detail-empty-title' }, T.detailTitle), h('p', { class: 'muted small' }, T.detailEmpty));
      return;
    }
    const d = entryDetail(e);
    const open = canOpen(e) ? h('button', { type: 'button', id: 'detail-open', class: 'primary' }, T.openFolder) : null;
    open?.addEventListener('click', () => this.drill(e));
    const sel = h('button', { type: 'button', id: 'detail-select', 'aria-pressed': this.selection.has(e.entry_id) ? 'true' : 'false' }, this.selection.has(e.entry_id) ? T.unselect : T.select);
    sel.addEventListener('click', () => this.toggle(e));
    // Optional AI diagnosis of this one object (no rule needed). Sends nothing until previewed.
    const diag = this.scanId ? h('button', { type: 'button', id: 'detail-diag', class: 'quiet', title: T.diagTip }, icon('diag'), T.diag) : null;
    diag?.addEventListener('click', () => this.app.openDiagnosis({ scanId: this.scanId!, entryId: e.entry_id, origin: 'disk', name: e.name }, 'disk'));
    // Platform idiom (GUI-12): show the object in the file manager; the host resolves the path from the index.
    const reveal = this.scanId ? h('button', { type: 'button', id: 'detail-reveal', class: 'quiet' }, icon('reveal'), words().reveal) : null;
    reveal?.addEventListener('click', () => void this.app.backend.revealEntry(this.scanId!, e.entry_id).catch(() => this.app.announce(`无法${words().reveal}`)));
    const hint = hintOf(e);
    replace(
      this.details,
      // 1. What it is.
      h('p', { class: 'detail-kind' }, h('span', { class: 'swatch', 'data-cat': categoryOf(e), 'aria-hidden': 'true' }), d.kind),
      h('h2', { class: 'detail-name' }, e.name),
      pathText(e.path_display, { max: 44, copy: true, className: 'detail-path' }),
      // 2. How big.
      h(
        'div',
        { class: 'detail-size' },
        h('p', { class: 'ds-figure num', title: formatExactBytes(d.sizeBytes) }, d.size),
        h('p', { class: 'ds-cap' }, h('span', {}, T.logical), h('span', { class: 'num' }, T.allocated(d.allocated)), d.unknownNote ? h('span', { class: 'ds-unknown' }, d.unknownNote) : null),
      ),
      d.boundaryNote ? h('p', { class: 'notice' }, d.boundaryNote) : null,
      // 3. What you can do.
      h('div', { class: 'card-actions detail-actions' }, open, sel, reveal, diag),
      hint ? this.hintEntry(e, hint) : null,
      techBlock(
        h(
          'dl',
          { class: 'facts-list small' },
          h('dt', {}, T.techBasis),
          h('dd', {}, d.basis),
          h('dt', {}, T.techExact),
          h('dd', { class: 'num' }, formatExactBytes(d.sizeBytes)),
          h('dt', {}, T.techIdentity),
          h('dd', {}, d.weakIdentity ? T.identityWeak : T.identityStrong),
          h('dt', {}, T.techEntry),
          h('dd', {}, h('code', { class: 'selectable' }, e.entry_id)),
        ),
      ),
    );
  }

  /** DIN-05: the review flow a cleanable hint leads to. Read-only here: the selection is not touched. */
  private hintEntry(e: Entry, hint: CleanableHint): HTMLElement {
    const action = hintAction(hint);
    const go = h('button', { type: 'button', id: 'detail-hint-go', class: 'quiet', 'aria-describedby': 'detail-hint-basis detail-hint-note' }, action.label);
    go.addEventListener('click', () => {
      if (action.flow === 'purge') this.app.navigate('purge');
      else void reviewTrash(this.app, this.scanId!, [e]);
    });
    return h(
      'div',
      { class: 'hint-entry' },
      h('p', { class: 'hint-head' }, h('span', { class: 'hint-tag' }, h('span', { class: 'hint-mark', 'aria-hidden': 'true' }), HINT_LABEL), h('span', { id: 'detail-hint-basis', class: 'small' }, hintBasisText(hint))),
      h('p', { class: 'muted small', id: 'detail-hint-note' }, `${T.hintNote}${action.note}`),
      go,
    );
  }

  private renderSelection(): void {
    if (!this.selection.size) {
      replace(this.selectionBox);
      return;
    }
    const visible = new Set(this.rows.map((r) => r.entry_id));
    const hidden = [...this.selection.keys()].filter((id) => !visible.has(id)).length;
    const clear = h('button', { type: 'button', id: 'sel-clear', class: 'quiet' }, T.clear);
    const trash = h('button', { type: 'button', id: 'sel-trash', class: 'primary' }, `${words().moveToBin}…`);
    trash.addEventListener('click', () => void reviewTrash(this.app, this.scanId!, [...this.selection.values()]));
    clear.addEventListener('click', () => this.clearSelection());
    const toggleList = h('button', { type: 'button', id: 'sel-list', class: 'quiet', 'aria-expanded': this.showSelected ? 'true' : 'false' }, this.showSelected ? T.hideSelected : T.showSelected);
    toggleList.addEventListener('click', () => {
      this.showSelected = !this.showSelected;
      this.renderSide();
    });
    const s = this.summary;
    replace(
      this.selectionBox,
      h('h2', {}, T.selected(this.selection.size)),
      s
        ? h(
            'p',
            { class: 'sel-figure' },
            sizeEl(s.logical_known_bytes, { unknownCount: s.logical_unknown_count, className: 'ds-figure' }),
            h('span', { class: 'ds-cap' }, h('span', {}, T.dedup), s.logical_unknown_count ? h('span', { class: 'ds-unknown' }, `${T.unknownSelLabel} ${T.unknownSel(s.logical_unknown_count)}`) : null),
          )
        : this.summaryError
          ? h('p', { class: 'form-error' }, this.summaryError)
          : h('p', { class: 'muted small', role: 'status' }, T.summing),
      hidden ? h('p', { class: 'notice warn' }, T.hiddenSelected(hidden)) : null,
      s?.overlapping_selected ? h('p', { class: 'muted small' }, `${T.overlap}：${T.overlapText(s.overlapping_selected)}`) : null,
      h('div', { class: 'card-actions' }, trash, toggleList, clear),
      h('p', { class: 'muted small' }, T.binNote(words().moveToBin)),
      this.showSelected
        ? h('ul', { class: 'selected-list' }, ...[...this.selection.values()].map((e) => h('li', {}, pathText(e.path_display, { max: 34 }), h('span', { class: 'num' }, sizeLabel(e.logical_bytes)))))
        : null,
    );
  }

  private renderBlocked(v: DiskView): void {
    const s = this.app.store.state;
    const st = s.status;
    const go = (kind: string) => {
      const toScope = v.action === 'scope';
      const b = h('button', { type: 'button', id: toScope ? 'disk-go-scope' : 'disk-go-overview', class: kind }, toScope ? DISK_TEXT.goScope : v.layout === 'running' ? DISK_TEXT.goOverview : DISK_TEXT.goReason);
      b.addEventListener('click', () => this.app.navigate(toScope ? 'scope' : 'overview'));
      return b;
    };
    let main: HTMLElement;
    if (v.layout === 'loading') main = h('p', { class: 'muted', role: 'status' }, DISK_TEXT.loading);
    else if (v.layout === 'none' || !v.banner) main = emptyBlock({ icon: 'disk', title: DISK_TEXT.noneTitle, body: DISK_TEXT.noneBody, actions: [go('primary')] });
    else
      main = statusBanner({
        kind: v.banner.kind,
        level: 2,
        titleId: 'disk-state-title',
        title: v.banner.title,
        sentence: v.banner.sentence,
        meta: st?.root_display ? [pathText(st.root_display, { max: 64, className: 'sb-path' })] : [],
        extra: v.layout === 'running' ? [h('div', { class: 'indeterminate', role: 'progressbar', 'aria-label': DISK_TEXT.runningTitle, 'aria-valuetext': `已发现 ${groupDigits(st?.entries_seen ?? 0)} 个条目` })] : [],
        actions: [go(v.layout === 'running' ? '' : 'primary')],
      });
    const others = v.offerOthers ? s.recent.filter((r) => r.scan_id !== s.scanId && r.state && isTerminal(r.state) && r.published && r.state !== 'failed') : [];
    replace(
      this.blocked,
      main,
      others.length
        ? h(
            'section',
            { class: 'quiet-section disk-others', 'aria-labelledby': 'disk-others-title' },
            h('h2', { id: 'disk-others-title' }, DISK_TEXT.others),
            h(
              'ul',
              { class: 'recent-list' },
              ...others.slice(0, 5).map((r) => {
                const b = h(
                  'button',
                  { type: 'button', class: 'recent-item' },
                  pathText(r.root_display, { max: 48, className: 'ri-path' }),
                  h('span', { class: 'ri-meta' }, h('span', { class: 'state-word', 'data-tone': stateTone(r.state!) }, h('span', { class: 'sw-mark', 'aria-hidden': 'true' }), STATE_LABEL[r.state!]), formatTime(r.started_at)),
                );
                b.addEventListener('click', () => void this.app.openScan(r.scan_id, 'disk'));
                return h('li', {}, b);
              }),
            ),
          )
        : null,
    );
  }
}
