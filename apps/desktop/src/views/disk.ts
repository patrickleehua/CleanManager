// P03 — disk: breadcrumb, one-layer treemap and the equivalent paged table, bound to the same
// scan_id + revision and entry IDs. Every query takes a ticket from a RequestGate; responses for
// an older ticket (left directory, changed sort/filter) are dropped. Selection is a map keyed by
// entry_id, frozen to one revision, and survives paging, filtering and drilling.

import type { App } from '../main';
import { words } from '../lib/platform';
import { reviewTrash } from './trash';
import { toIpcError } from '../lib/api';
import { formatBytes, formatExactBytes, groupDigits, parseBytes } from '../lib/bytes';
import { h, icon, keepFocus, replace } from '../lib/dom';
import { basisLabel, ERROR_HINT, formatTime, isTerminal, kindLabel, STATE_LABEL, taskTitle } from '../lib/format';
import { RequestGate } from '../lib/gate';
import { MERGED_ID } from '../lib/treemap';
import { HINT_LABEL, hintAction, hintBasisText, hintOf } from '../lib/cleanable';
import type { CleanableHint, Entry, EntriesQuery, IpcError, KindFilter, SortKey, Summary, TreemapLayer } from '../lib/types';
import { canOpen, EntryGrid } from './grid';
import { REST_ID, TreemapView } from './treemapView';

const PAGE_SIZE = 200;
const TREEMAP_LIMIT = 120;
const LOCATE_PAGE_BUDGET = 10;

interface Crumb {
  entryId: string;
  name: string;
}

export class DiskPage {
  el: HTMLElement;
  private header: HTMLElement;
  private crumbs: HTMLElement;
  private layerInfo: HTMLElement;
  private notice: HTMLElement;
  private controls: HTMLElement;
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
    this.layerInfo = h('p', { class: 'layer-info muted' });
    this.notice = h('div', { class: 'notice', role: 'status', hidden: true });
    this.header = h(
      'div',
      { class: 'page-head disk-head' },
      h('div', {}, h('h1', { id: 'disk-title' }, '磁盘'), h('nav', { 'aria-label': '当前位置' }, this.crumbs), this.layerInfo),
    );
    this.treemap = new TreemapView({
      onSelect: (id, members) => void this.locate(id === MERGED_ID || id === REST_ID ? (members?.[0] ?? id) : id),
      onOpen: (e) => this.drill(e),
    });
    this.grid = new EntryGrid('disk', '当前文件夹的条目', {
      onActive: (e) => this.setActive(e),
      onOpen: (e) => this.drill(e),
      onUp: () => this.up(),
      onToggle: (e) => this.toggle(e),
      onSort: (s) => this.setSort(s),
      onNeedMore: () => void this.loadMore(),
      isSelected: (id) => this.selection.has(id),
    });
    this.controls = h('div', { class: 'controls', role: 'group', 'aria-label': '表格过滤与排序' });
    this.gridStatus = h('p', { class: 'grid-status muted', 'aria-live': 'polite' });
    this.moreBtn = h('button', { type: 'button', id: 'disk-more', class: 'quiet' }, '显示更多');
    this.moreBtn.addEventListener('click', () => void this.loadMore());
    this.details = h('div', { class: 'details' });
    this.selectionBox = h('div', { class: 'selection-box' });
    this.content = h(
      'div',
      { class: 'disk-layout' },
      h('section', { class: 'panel chart-panel', 'aria-label': '空间分布图' }, this.treemap.el),
      h(
        'section',
        { class: 'panel table-panel', 'aria-labelledby': 'table-title' },
        h('h2', { id: 'table-title', class: 'sr-only' }, '条目表格'),
        this.controls,
        this.grid.el,
        h('div', { class: 'grid-foot' }, this.gridStatus, this.moreBtn),
      ),
      h('aside', { class: 'panel side-panel', 'aria-label': '详情与选择' }, this.details, this.selectionBox),
    );
    this.blocked = h('div', { class: 'disk-blocked' });
    this.el = h('section', { class: 'page page-disk', 'aria-labelledby': 'disk-title' }, this.header, this.notice, this.blocked, this.content);
    this.renderControls();
  }

  activate(): void {
    // Restore keyboard position when returning to the page (filters/path/scroll were kept).
    if (this.grid.active) requestAnimationFrame(() => this.grid.setActive(this.grid.active!, true));
  }

  update(): void {
    const s = this.app.store.state;
    const status = s.status;
    const ready = !!status && status.published && !!status.root_entry_id && status.scan_id === s.scanId;
    if (!s.scanId || !status || !ready) {
      this.content.hidden = true;
      this.blocked.hidden = false;
      this.renderBlocked();
      return;
    }
    this.blocked.hidden = true;
    this.content.hidden = false;
    if (status.scan_id !== this.scanId || status.revision !== this.revision) {
      this.bind(status.scan_id, status.revision!, status.root_entry_id!, status.root_display ?? '扫描根目录');
    }
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
    this.scope = 'layer';
    this.selection.clear();
    this.summary = null;
    this.activeEntry = null;
    this.grid.reset();
    this.showNotice(hadSelection ? '已切换到另一份扫描结果，之前的选择不会套用到新结果上。' : null);
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
    this.renderHeader();
    try {
      const res = await this.layerGate.run(ticket, this.app.backend.treemapLayer(this.scanId!, this.layerId, TREEMAP_LIMIT));
      if (res.stale) return;
      if (res.value.revision !== this.revision) return this.revisionChanged();
      this.layer = res.value;
      this.treemap.setLayer(res.value);
      this.treemap.setActive(this.activeEntry?.entry_id ?? null);
      this.renderHeader();
      this.pushGrid();
    } catch (e) {
      const err = toIpcError(e);
      this.treemap.setError(`图表无法加载：${ERROR_HINT[err.reason] ?? err.message}。表格仍可使用。`);
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
    this.showNotice('扫描结果的版本已变化，页面已按新版本重新加载。');
    void this.app.refreshStatus();
  }

  // ---- navigation ------------------------------------------------------------------------

  private drill(e: Entry): void {
    if (!canOpen(e)) return;
    this.path = [...this.path, { entryId: e.entry_id, name: e.name }];
    this.scope = 'layer';
    this.grid.reset();
    this.activeEntry = null;
    this.renderControls();
    this.reloadAll();
    this.app.announce(`已打开 ${e.name}`);
    this.grid.focus();
  }

  private goTo(depth: number): void {
    if (depth >= this.path.length - 1) return;
    const left = this.path[depth + 1];
    this.path = this.path.slice(0, depth + 1);
    this.scope = 'layer';
    this.grid.reset();
    this.activeEntry = null;
    this.renderControls();
    this.reloadAll();
    this.app.announce(`已返回 ${this.path[depth].name}`);
    // Put the cursor back on the folder we came from once the page arrives.
    void this.locate(left.entryId, true);
  }

  private up(): void {
    if (this.path.length > 1) this.goTo(this.path.length - 2);
  }

  private setSort(s: SortKey): void {
    if (s === this.sort) return;
    this.sort = s;
    this.renderControls();
    const keep = this.activeEntry?.entry_id;
    this.grid.reset();
    void this.loadFirstPage().then(() => (keep ? this.locate(keep, true) : undefined));
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
    if (!quiet) this.showNotice('此条目不在表格当前的过滤或已加载范围内；详情已显示在右侧。清除过滤或继续加载可在表格中找到它。');
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
    this.app.announce('已清除选择');
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
      this.summaryError = toIpcError(e).message;
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
    const emptyText = this.listError
      ? `无法加载：${ERROR_HINT[this.listError.reason] ?? this.listError.message}`
      : filtered
        ? '没有符合过滤条件的条目。清除过滤可查看全部。'
        : '这个文件夹是空的。';
    const base = this.scope === 'layer' && this.layer ? parseBytes(this.layer.parent_summary.logical_known_bytes) : null;
    this.grid.setData({ rows: this.rows, complete: this.complete, loading: this.loading, shareBase: base, sort: this.sort, emptyText });
    const visibleSelected = this.rows.filter((r) => this.selection.has(r.entry_id)).length;
    this.gridStatus.textContent = this.listError
      ? ''
      : `${this.complete ? `共 ${groupDigits(this.rows.length)} 项` : `已加载 ${groupDigits(this.rows.length)} 项，还有更多`}${
          this.selection.size ? `；已选 ${groupDigits(this.selection.size)} 项${this.selection.size > visibleSelected ? `（${groupDigits(this.selection.size - visibleSelected)} 项不在当前表格中）` : ''}` : ''
        }`;
    this.moreBtn.hidden = this.complete;
    this.moreBtn.disabled = this.loading;
  }

  private renderHeader(): void {
    keepFocus(this.crumbs, () =>
      replace(
        this.crumbs,
        ...this.path.map((c, i) => {
          const last = i === this.path.length - 1;
          const b = h('button', { type: 'button', id: `crumb-${i}`, class: 'crumb', 'aria-current': last ? 'location' : null, disabled: last }, c.name);
          b.addEventListener('click', () => this.goTo(i));
          return h('li', {}, b);
        }),
      ),
    );
    const st = this.app.store.state.status;
    const sum = this.layer?.parent_summary;
    const parts: string[] = [];
    if (sum) {
      parts.push(`此文件夹已知 ${formatBytes(sum.logical_known_bytes)}`);
      if (sum.logical_unknown_count) parts.push(`${groupDigits(sum.logical_unknown_count)} 项大小未知`);
    }
    if (st) parts.push(`${STATE_LABEL[st.state]} · ${formatTime(st.finished_at ?? st.started_at)} · 修订 ${st.revision}`);
    parts.push('面积与排序按逻辑大小');
    this.layerInfo.textContent = parts.join('　');
  }

  private renderControls(): void {
    const seg = <T extends string>(label: string, name: string, value: T, options: [T, string][], on: (v: T) => void) =>
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
    const search = h('input', { type: 'search', id: 'disk-filter', placeholder: '按名称过滤', 'aria-label': '按名称过滤', value: this.name, maxlength: '256' });
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
      { id: 'disk-sort', 'aria-label': '排序' },
      h('option', { value: 'logical_desc' }, '逻辑大小（大到小）'),
      h('option', { value: 'allocated_desc' }, '分配大小（大到小）'),
      h('option', { value: 'name_asc' }, '名称（A–Z）'),
    );
    sortSel.value = this.sort;
    sortSel.addEventListener('change', () => this.setSort(sortSel.value as SortKey));
    keepFocus(this.controls, () =>
      replace(
        this.controls,
        seg('范围', 'disk-scope', this.scope, [['layer', '当前文件夹'], ['all', '整个扫描']], (v) => {
          this.scope = v;
          this.grid.reset();
          void this.loadFirstPage();
        }),
        seg('类型', 'disk-kind', this.kind, [['any', '全部'], ['directory', '文件夹'], ['file', '文件']], (v) => {
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

  private renderDetails(): void {
    const e = this.activeEntry;
    if (!e) {
      replace(this.details, h('h2', {}, '详情'), h('p', { class: 'muted' }, '在图或表格中选中一项，这里会显示它的大小依据和覆盖情况。'));
      return;
    }
    const logical = parseBytes(e.logical_bytes);
    const open = canOpen(e) ? h('button', { type: 'button', id: 'detail-open', class: 'primary' }, '打开此文件夹') : null;
    open?.addEventListener('click', () => this.drill(e));
    const sel = h('button', { type: 'button', id: 'detail-select' }, this.selection.has(e.entry_id) ? '取消选择' : '选择');
    sel.addEventListener('click', () => this.toggle(e));
    // Optional AI diagnosis of this one object (no rule needed). Sends nothing until previewed.
    const diag = this.scanId ? h('button', { type: 'button', id: 'detail-diag', class: 'quiet', title: '解释这个对象的用途与删除后果' }, icon('diag'), 'AI 诊断') : null;
    diag?.addEventListener('click', () => this.app.openDiagnosis({ scanId: this.scanId!, entryId: e.entry_id, origin: 'disk', name: e.name }, 'disk'));
    // Platform idiom (GUI-12): show the object in the file manager; the host resolves the path from the index.
    const reveal = this.scanId ? h('button', { type: 'button', id: 'detail-reveal', class: 'quiet' }, icon('reveal'), words().reveal) : null;
    reveal?.addEventListener('click', () => void this.app.backend.revealEntry(this.scanId!, e.entry_id).catch(() => this.app.announce(`无法${words().reveal}`)));
    const hint = hintOf(e);
    replace(
      this.details,
      h('h2', { class: 'detail-name' }, e.name),
      h('p', { class: 'path selectable' }, e.path_display),
      h(
        'dl',
        { class: 'facts-list' },
        h('dt', {}, '逻辑大小'),
        h('dd', { class: 'num' }, formatBytes(logical), logical !== null ? h('small', {}, formatExactBytes(logical)) : null),
        h('dt', {}, '分配大小'),
        h('dd', { class: 'num' }, formatBytes(e.allocated_bytes)),
        h('dt', {}, '大小依据'),
        h('dd', {}, basisLabel(e)),
        h('dt', {}, '类型'),
        h('dd', {}, kindLabel(e)),
        e.unknown_descendants ? h('dt', {}, '未知子项') : null,
        e.unknown_descendants ? h('dd', {}, `${groupDigits(e.unknown_descendants)} 项（未计入大小）`) : null,
        h('dt', {}, '身份'),
        h('dd', {}, e.identity_quality === 'strong' ? '卷 + 文件 ID' : '较弱（仅供浏览，不作执行依据）'),
        hint ? h('dt', {}, HINT_LABEL) : null,
        hint ? h('dd', { id: 'detail-hint-basis' }, hintBasisText(hint)) : null,
      ),
      e.boundary ? h('p', { class: 'notice' }, '这是一个链接或云端边界。扫描记录了它，但没有进入其中，也不会把目标的大小算进来。') : null,
      hint ? this.hintEntry(e, hint) : null,
      h('div', { class: 'card-actions' }, open, sel, diag, reveal),
    );
  }

  /** DIN-05: the review flow a cleanable hint leads to. Read-only here: the selection is not touched. */
  private hintEntry(e: Entry, hint: CleanableHint): HTMLElement {
    const action = hintAction(hint);
    const go = h('button', { type: 'button', id: 'detail-hint-go', 'aria-describedby': 'detail-hint-basis detail-hint-note' }, action.label);
    go.addEventListener('click', () => {
      if (action.flow === 'purge') this.app.navigate('purge');
      else void reviewTrash(this.app, this.scanId!, [e]);
    });
    return h(
      'div',
      { class: 'hint-entry' },
      h('p', { class: 'muted small', id: 'detail-hint-note' }, `只是提示：不会替你选中，也不改变保护或风险判断。${action.note}`),
      h('div', { class: 'card-actions' }, go),
    );
  }

  private renderSelection(): void {
    if (!this.selection.size) {
      replace(this.selectionBox);
      return;
    }
    const visible = new Set(this.rows.map((r) => r.entry_id));
    const hidden = [...this.selection.keys()].filter((id) => !visible.has(id)).length;
    const clear = h('button', { type: 'button', id: 'sel-clear', class: 'quiet' }, '清除选择');
    const trash = h('button', { type: 'button', id: 'sel-trash', class: 'primary' }, `${words().moveToBin}…`);
    trash.addEventListener('click', () => void reviewTrash(this.app, this.scanId!, [...this.selection.values()]));
    clear.addEventListener('click', () => this.clearSelection());
    const toggleList = h('button', { type: 'button', id: 'sel-list', class: 'quiet', 'aria-expanded': this.showSelected ? 'true' : 'false' }, this.showSelected ? '收起已选' : '查看全部已选');
    toggleList.addEventListener('click', () => {
      this.showSelected = !this.showSelected;
      this.renderSide();
    });
    const s = this.summary;
    replace(
      this.selectionBox,
      h('h2', {}, `已选 ${groupDigits(this.selection.size)} 项`),
      hidden ? h('p', { class: 'notice warn' }, `其中 ${groupDigits(hidden)} 项不在当前表格中（其他文件夹或被过滤）。`) : null,
      s
        ? h(
            'dl',
            { class: 'facts-list' },
            h('dt', {}, '去重后逻辑大小'),
            h('dd', { class: 'num' }, formatBytes(s.logical_known_bytes)),
            s.logical_unknown_count ? h('dt', {}, '大小未知') : null,
            s.logical_unknown_count ? h('dd', {}, `${groupDigits(s.logical_unknown_count)} 项，未计入`) : null,
            s.overlapping_selected ? h('dt', {}, '包含关系') : null,
            s.overlapping_selected ? h('dd', {}, `${groupDigits(s.overlapping_selected)} 项位于另一已选文件夹内，只计一次`) : null,
          )
        : this.summaryError
          ? h('p', { class: 'form-error' }, this.summaryError)
          : h('p', { class: 'muted' }, '正在汇总…'),
      h('p', { class: 'muted small' }, `可以把选中的对象${words().moveToBin}：先逐项检查保护与占用，再生成计划，由你审阅批准后才会移动。`),
      this.showSelected
        ? h(
            'ul',
            { class: 'selected-list' },
            ...[...this.selection.values()].map((e) => h('li', {}, h('span', { class: 'path' }, e.path_display), h('span', { class: 'num' }, formatBytes(e.logical_bytes)))),
          )
        : null,
      h('div', { class: 'card-actions' }, trash, toggleList, clear),
    );
  }

  private renderBlocked(): void {
    const s = this.app.store.state;
    const st = s.status;
    let title = '还没有可浏览的结果';
    let text = '先选择一个范围并完成一次扫描。';
    if (st && !isTerminal(st.state)) {
      title = taskTitle(st.state);
      text = '结果在扫描结束并发布后才能浏览。CleanManager 不读取未发布的暂存数据来拼凑实时排名。你可以留在这里或切换页面，扫描会继续进行。';
    } else if (st && st.published && !st.root_entry_id) {
      title = '这次扫描没有记录任何条目';
      text = '扫描根目录可能无法读取。请在概览中查看原因，或重新选择范围。';
    } else if (s.statusError) {
      title = '无法读取扫描记录';
      text = ERROR_HINT[s.statusError.reason] ?? s.statusError.message;
    }
    const others = s.recent.filter((r) => r.scan_id !== s.scanId && r.state && isTerminal(r.state));
    const go = h('button', { type: 'button', id: 'disk-go-scope' }, '选择扫描范围');
    go.addEventListener('click', () => this.app.navigate('scope'));
    replace(
      this.blocked,
      h('div', { class: 'panel empty' }, icon('disk'), h('h2', {}, title), h('p', { class: 'lede' }, text), h('div', { class: 'card-actions' }, go)),
      others.length
        ? h(
            'div',
            { class: 'panel' },
            h('h2', {}, '可以浏览的其他扫描'),
            h(
              'ul',
              { class: 'recent-list' },
              ...others.slice(0, 5).map((r) => {
                const b = h('button', { type: 'button', class: 'recent-item' }, h('span', { class: 'path' }, r.root_display), h('span', { class: 'muted' }, `${formatTime(r.started_at)} · ${STATE_LABEL[r.state!]}`));
                b.addEventListener('click', () => void this.app.openScan(r.scan_id, 'disk'));
                return h('li', {}, b);
              }),
            ),
          )
        : null,
    );
  }
}
