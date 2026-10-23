// P03 — disk: breadcrumb, one-layer treemap and the equivalent paged table, bound to the same
// scan_id + revision and entry IDs. Every query takes a ticket from a RequestGate; responses for
// an older ticket (left directory, changed sort/filter) are dropped. Selection is a map keyed by
// entry_id, frozen to one revision, and survives paging, filtering and drilling.
//
// The chart and the table are one view of the same layer: the table's current row lights its tile,
// choosing a tile (pointer or arrow keys) makes the same entry current in the table, and the detail
// pane follows both. Hierarchy of the detail pane: what it is → how big → what you can do.

import type { App } from '../main';
import { platform, words } from '../lib/platform';
import { reviewTrash } from './trash';
import { toIpcError } from '../lib/api';
import { formatExactBytes, parseBytes } from '../lib/bytes';
import { h, icon, keepFocus, live, replace } from '../lib/dom';
import { ERROR_HINT, formatTime, isTerminal, STATE_LABEL, stateTone } from '../lib/format';
import { RequestGate } from '../lib/gate';
import { MERGED_ID } from '../lib/treemap';
import { hintLabel, hintAction, hintBasisText, hintOf } from '../lib/cleanable';
import { categoryOf } from '../lib/category';
import { crumbTrail, DISK_TEXT, diskView, entryDetail, entryForTile, protectedBy, selectionBlock, type DiskView } from '../lib/diskModel';
import { sizeLabel } from '../lib/display';
import type { CleanableHint, Entry, EntriesQuery, IpcError, KindFilter, ProtectionList, SortKey, Summary, TreemapLayer } from '../lib/types';
import { canOpen, EntryGrid } from './grid';
import { REST_ID, TreemapView } from './treemapView';
import { refusalText } from '../lib/codes';
import { errorLine } from './parts';
import { emptyBlock, pathText, sizeEl, stateLine, statusBanner, techBlock } from './ui';
import { currentIndexProblem } from '../lib/indexModel';
import { indexProblemBanner } from './indexRepair';
import { t } from '../i18n';

const PAGE_SIZE = 200;
const TREEMAP_LIMIT = 120;
const LOCATE_PAGE_BUDGET = 10;

/** Words of the working page (the state words are in DISK_TEXT). */
const T = {
  get crumbsLabel() { return t('disk.t.crumbsLabel'); },
  get up() { return t('disk.t.up'); },
  upKey: 'Backspace',
  get more() { return t('disk.t.more'); },
  get basis() { return t('disk.t.basis'); },
  get known() { return t('disk.t.known'); },
  unknownIn: (n: number) => t('disk.t.unknownIn', { n }),
  get chart() { return t('disk.t.chart'); },
  get table() { return t('disk.t.table'); },
  get tableLabel() { return t('disk.t.tableLabel'); },
  get side() { return t('disk.t.side'); },
  get filters() { return t('disk.t.filters'); },
  get scope() { return t('disk.t.scope'); },
  get scopeLayer() { return t('disk.t.scopeLayer'); },
  get scopeAll() { return t('disk.t.scopeAll'); },
  get kind() { return t('disk.t.kind'); },
  get kindAny() { return t('disk.t.kindAny'); },
  get kindDir() { return t('disk.t.kindDir'); },
  get kindFile() { return t('disk.t.kindFile'); },
  get filter() { return t('disk.t.filter'); },
  get sort() { return t('disk.t.sort'); },
  get sortLogical() { return t('disk.t.sortLogical'); },
  get sortAllocated() { return t('disk.t.sortAllocated'); },
  get sortName() { return t('disk.t.sortName'); },
  get showMore() { return t('disk.t.showMore'); },
  get detailTitle() { return t('disk.t.detailTitle'); },
  get detailEmpty() { return t('disk.t.detailEmpty'); },
  get logical() { return t('disk.t.logical'); },
  allocated: (s: string) => t('disk.t.allocated', { s }),
  get openFolder() { return t('disk.t.openFolder'); },
  get select() { return t('disk.t.select'); },
  get unselect() { return t('disk.t.unselect'); },
  get diag() { return t('disk.t.diag'); },
  get diagTip() { return t('disk.t.diagTip'); },
  get techBasis() { return t('disk.t.techBasis'); },
  get techIdentity() { return t('disk.t.techIdentity'); },
  get identityStrong() { return t('disk.t.identityStrong'); },
  get identityWeak() { return t('disk.t.identityWeak'); },
  get techExact() { return t('disk.t.techExact'); },
  get techEntry() { return t('disk.t.techEntry'); },
  get hintNote() { return t('disk.t.hintNote'); },
  selected: (n: number) => t('disk.t.selected', { n }),
  hiddenSelected: (n: number) => t('disk.t.hiddenSelected', { n }),
  get dedup() { return t('disk.t.dedup'); },
  unknownSel: (n: number) => t('disk.t.unknownSel', { n }),
  get unknownSelLabel() { return t('disk.t.unknownSelLabel'); },
  get overlap() { return t('disk.t.overlap'); },
  overlapText: (n: number) => t('disk.t.overlapText', { n }),
  get summing() { return t('disk.t.summing'); },
  binNote: (moveToBin: string) => t('disk.t.binNote', { moveToBin }),
  get clear() { return t('disk.t.clear'); },
  get showSelected() { return t('disk.t.showSelected'); },
  get hideSelected() { return t('disk.t.hideSelected'); },
  get cleared() { return t('disk.t.cleared'); },
  opened: (n: string) => t('disk.t.opened', { n }),
  back: (n: string) => t('disk.t.back', { n }),
  get switched() { return t('disk.t.switched'); },
  get revision() { return t('disk.t.revision'); },
  get notInTable() { return t('disk.t.notInTable'); },
  get restTile() { return t('disk.t.restTile'); },
  mergedTile: (n: number) => t('disk.t.mergedTile', { n }),
  get emptyFiltered() { return t('disk.t.emptyFiltered'); },
  get emptyFolder() { return t('disk.t.emptyFolder'); },
  loadFailed: (r: string) => t('disk.t.loadFailed', { r }),
  chartFailed: (r: string) => t('disk.t.chartFailed', { r }),
  count: (n: number) => t('disk.t.count', { n }),
  loaded: (n: number) => t('disk.t.loaded', { n }),
  selectedHidden: (n: number, hidden: number) => t('disk.t.selectedHidden', { n, p: hidden ? t('disk.hiddenNotCurrentTable', { hidden }) : '' }),
  revisionTip: (r: number) => t('disk.t.revisionTip', { r }),
  get protectFolder() { return t('disk.t.protectFolder'); },
  get protectFile() { return t('disk.t.protectFile'); },
  get protecting() { return t('disk.t.protecting'); },
  get protectedNow() { return t('disk.t.protectedNow'); },
  protectedAlready: (byParent: boolean) => (byParent ? t('disk.t.protectedByParent') : t('disk.t.protectedAlready')),
  get protectedHow() { return t('disk.t.protectedHow'); },
  get openProtectionList() { return t('disk.t.openProtectionList'); },
  get protectListBroken() { return t('disk.t.protectListBroken'); },
  get protectWhat() { return t('disk.t.protectWhat'); },
  protectedAnnounce: (name: string) => t('disk.t.protectedAnnounce', { name }),
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
  /** NEW-06: the protection list (to say "protected" instead of offering it twice) and the last protect result. */
  private protection: ProtectionList | null = null;
  private protectionAsked = false;
  private protecting = false;
  private protectDone: { entryId: string; name: string } | null = null;
  private protectError: IpcError | null = null;


  private listGate = new RequestGate();
  private layerGate = new RequestGate();
  private summaryGate = new RequestGate();

  constructor(private app: App) {
    this.crumbs = h('ol', { class: 'crumbs' });
    this.upBtn = live(h('button', { type: 'button', id: 'disk-up', class: 'icon-btn crumb-up', 'aria-keyshortcuts': 'Backspace' }, icon('back')), null, { 'aria-label': () => T.up, title: () => t('disk.upUpkey', { up: T.up, upKey: T.upKey }) });
    this.upBtn.addEventListener('click', () => this.up());
    this.metaRow = h('p', { class: 'meta-row disk-meta' });
    this.stateNote = h('div', { class: 'disk-state' });
    this.notice = h('div', { class: 'notice', role: 'status', hidden: true });
    this.header = h(
      'div',
      { class: 'page-head disk-head' },
      h('div', { class: 'page-head-text' }, live(h('h1', { id: 'disk-title' }), () => DISK_TEXT.title), live(h('nav', { class: 'crumb-bar' }, this.upBtn, this.crumbs), null, { 'aria-label': () => T.crumbsLabel }), this.metaRow),
    );
    this.treemap = new TreemapView({
      onSelect: (id, members, how) => this.fromTile(id, members, how),
      onOpen: (e) => this.drill(e, 'chart'),
      onUp: () => this.up('chart'),
      onToggle: (e) => this.toggle(e),
    });
    this.grid = new EntryGrid('disk', () => T.tableLabel, {
      onActive: (e) => this.setActive(e),
      onOpen: (e) => this.drill(e),
      onUp: () => this.up(),
      onToggle: (e) => this.toggle(e),
      onSort: (s) => this.setSort(s),
      onNeedMore: () => void this.loadMore(),
      isSelected: (id) => this.selection.has(id),
      onHover: (e) => this.treemap.setHover(e?.entry_id ?? null),
    });
    this.controls = live(h('div', { class: 'controls', role: 'group' }), null, { 'aria-label': () => T.filters });
    this.gridStatus = h('p', { class: 'grid-status muted', 'aria-live': 'polite' });
    this.moreBtn = live(h('button', { type: 'button', id: 'disk-more', class: 'quiet' }), () => T.showMore);
    this.moreBtn.addEventListener('click', () => void this.loadMore());
    this.chartHead = h('div', { class: 'chart-head' });
    this.details = h('div', { class: 'details', 'aria-live': 'polite', 'aria-atomic': 'false' });
    this.selectionBox = h('div', { class: 'selection-box' });
    this.content = h(
      'div',
      { class: 'disk-layout' },
      live(h('section', { class: 'panel chart-panel' }, this.chartHead, this.treemap.el), null, { 'aria-label': () => T.chart }),
      h(
        'section',
        { class: 'panel table-panel', 'aria-labelledby': 'table-title' },
        live(h('h2', { id: 'table-title', class: 'sr-only' }), () => T.table),
        this.controls,
        this.grid.el,
        h('div', { class: 'grid-foot' }, this.gridStatus, this.moreBtn),
      ),
      live(h('aside', { class: 'panel side-panel' }, this.details, this.selectionBox), null, { 'aria-label': () => T.side }),
    );
    this.blocked = h('div', { class: 'disk-blocked' });
    this.el = h('section', { class: 'page page-disk', 'aria-labelledby': 'disk-title' }, this.header, this.stateNote, this.notice, this.blocked, this.content);
    this.renderControls();
  }

  activate(): void {
    // The protection list may have changed in 设置 › 保护清单 meanwhile.
    if (this.protectionAsked) void this.loadProtection();
    // Restore keyboard position when returning to the page (filters/path/scroll were kept).
    if (this.grid.active) requestAnimationFrame(() => this.grid.setActive(this.grid.active!, true));
  }

  /** LIF-05: the parts drawn only when their data changes are drawn again in the new language. */
  relocalize(): void {
    this.renderHeader();
    this.renderChartHead();
    this.renderControls();
    this.renderSide();
    if (this.scanId) this.pushGrid();
    this.grid.relocalize();
    this.treemap.relocalize();
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
      this.bind(status.scan_id, status.revision!, status.root_entry_id!, status.root_display ?? t('disk.scanRoot'));
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
    this.protectDone = null;
    this.protectError = null;
    if (!this.protectionAsked) {
      this.protectionAsked = true;
      void this.loadProtection();
    }
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

  private async loadProtection(): Promise<void> {
    try {
      this.protection = await this.app.backend.listProtection();
    } catch {
      this.protection = null;
    }
    this.renderSide();
  }

  /**
   * NEW-06: protect the current entry (a folder with everything in it). Adding protection only narrows what may be
   * cleaned, so it needs no typed confirmation; removing it later does (设置 › 保护清单). The host reads the path from the
   * index: only the scan and entry ids cross IPC.
   */
  private async protect(e: Entry): Promise<void> {
    if (!this.scanId || this.protecting) return;
    this.protecting = true;
    this.protectError = null;
    this.protectDone = null;
    this.renderSide();
    try {
      this.protection = await this.app.backend.addProtection({ source: 'entry', scan_id: this.scanId, entry_id: e.entry_id, recursive: e.kind === 'directory' }, null);
      this.protectDone = { entryId: e.entry_id, name: e.name };
      // A protected object is never part of a plan: it leaves the selection now rather than at the review step.
      if (this.selection.delete(e.entry_id)) {
        this.grid.refreshSelection();
        void this.loadSummary();
      }
      this.app.announce(T.protectedAnnounce(e.name));
    } catch (err) {
      this.protectError = toIpcError(err);
    } finally {
      this.protecting = false;
      this.renderSide();
    }
  }

  private setActive(e: Entry): void {
    if (this.activeEntry?.entry_id !== e.entry_id) {
      this.protectDone = null;
      this.protectError = null;
    }
    this.activeEntry = e;
    this.treemap.setActive(e.entry_id);
    this.renderSide();
  }

  private toggle(e: Entry): void {
    if (this.selection.has(e.entry_id)) this.selection.delete(e.entry_id);
    else {
      // R3-03: a link (or one into a protected system location) is never offered, from the table or chart either.
      const blocked = selectionBlock(e);
      if (blocked) return this.app.announce(blocked);
      this.selection.set(e.entry_id, e);
    }
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
            const b = h('button', { type: 'button', id: 'crumb-more', class: 'crumb crumb-more', 'aria-label': t('disk.moreLength', { more: T.more, length: c.depths.length }), title: c.depths.map((d) => names[d]).join(' › ') }, '…');
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
    // Covered by the list as read, or protected from here just now (the host's display of the entry may differ in form).
    const covered =
      (this.protection ? protectedBy(e.path_display, this.protection.items, platform() === 'windows') : null) ??
      (this.protectDone?.entryId === e.entry_id ? { protection_id: '', display: e.path_display } : null);
    // A protected object cannot be moved: it is not offered for a selection (one already made can still be undone).
    // R3-03: nor is a link: one into a protected system location says where it leads (the host's check), here already.
    const blocked = covered ? null : selectionBlock(e);
    const sel = h(
      'button',
      { type: 'button', id: 'detail-select', 'aria-pressed': this.selection.has(e.entry_id) ? 'true' : 'false', disabled: (!!covered || !!blocked) && !this.selection.has(e.entry_id), 'aria-describedby': blocked ? 'detail-not-selectable' : null },
      this.selection.has(e.entry_id) ? T.unselect : T.select,
    );
    sel.addEventListener('click', () => this.toggle(e));
    // NEW-06: protect this object from here. Links and mount points are never entered, so they are not offered.
    const listBroken = !!this.protection?.problem;
    const protect =
      this.scanId && !covered && !e.boundary && e.kind !== 'symlink' && e.kind !== 'reparse'
        ? h('button', { type: 'button', id: 'detail-protect', class: 'quiet', disabled: this.protecting || listBroken, title: listBroken ? T.protectListBroken : null }, icon('shield'), this.protecting ? T.protecting : e.kind === 'directory' ? T.protectFolder : T.protectFile)
        : null;
    protect?.addEventListener('click', () => void this.protect(e));
    // Optional AI diagnosis of this one object (no rule needed). Sends nothing until previewed.
    const diag = this.scanId ? h('button', { type: 'button', id: 'detail-diag', class: 'quiet', title: T.diagTip }, icon('diag'), T.diag) : null;
    diag?.addEventListener('click', () => this.app.openDiagnosis({ scanId: this.scanId!, entryId: e.entry_id, origin: 'disk', name: e.name }, 'disk'));
    // Platform idiom (GUI-12): show the object in the file manager; the host resolves the path from the index.
    const reveal = this.scanId ? h('button', { type: 'button', id: 'detail-reveal', class: 'quiet' }, icon('reveal'), words().reveal) : null;
    reveal?.addEventListener('click', () => void this.app.backend.revealEntry(this.scanId!, e.entry_id).catch(() => this.app.announce(t('disk.revealFailed'))));
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
      blocked ? h('p', { class: 'small muted', id: 'detail-not-selectable', 'data-state': e.protected_link ? 'protected' : 'link' }, blocked) : null,
      this.protectLine(e, covered, protect),
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

  /** Whether this object is protected, the result of protecting it, or why it could not be. */
  private protectLine(e: Entry, covered: { protection_id: string; display: string } | null, protect: HTMLElement | null): HTMLElement | null {
    const link = h('button', { type: 'button', class: 'link', id: 'detail-protection-list' }, T.openProtectionList);
    link.addEventListener('click', () => {
      this.app.navigate('settings');
      requestAnimationFrame(() => {
        const target = document.getElementById('set-protection-title');
        target?.scrollIntoView({ block: 'start' });
        target?.focus({ preventScroll: true });
      });
    });
    if (this.protectError) return h('div', { class: 'protect-line', 'data-state': 'error' }, errorLine(this.protectError));
    if (covered) {
      const done = this.protectDone?.entryId === e.entry_id;
      return h(
        'div',
        { class: 'protect-line', 'data-state': 'protected', role: done ? 'status' : null },
        h('p', { class: 'pl-head' }, h('span', { class: 'pl-mark', 'aria-hidden': 'true' }, icon('shield')), done ? T.protectedNow : T.protectedAlready(covered.display !== e.path_display)),
        h('p', { class: 'small muted' }, T.protectedHow, ' ', link),
      );
    }
    if (this.protection?.problem) return h('div', { class: 'protect-line' }, protect, h('p', { class: 'small muted' }, T.protectListBroken, ' ', link));
    // Protection is its own topic under the actions: the shield button and what it means.
    return protect ? h('div', { class: 'protect-line', 'data-state': 'open' }, protect, h('p', { class: 'small muted' }, T.protectWhat)) : null;
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
      h('p', { class: 'hint-head' }, h('span', { class: 'hint-tag' }, h('span', { class: 'hint-mark', 'aria-hidden': 'true' }), hintLabel()), h('span', { id: 'detail-hint-basis', class: 'small' }, hintBasisText(hint))),
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
      s?.overlapping_selected ? h('p', { class: 'muted small' }, t('disk.overlapOverlappingSelected', { overlap: T.overlap, overlapping_selected: T.overlapText(s.overlapping_selected) })) : null,
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
    // NEW-04: with a damaged index there is nothing to browse because it cannot be read, not because nothing was scanned.
    const broken = v.layout === 'none' || v.layout === 'error' ? currentIndexProblem({ caps: s.caps, statusError: s.statusError, repairedAt: s.indexRepairedAt }) : null;
    if (broken) {
      replace(this.blocked, indexProblemBanner(this.app, broken, { titleId: 'disk-state-title', level: 2 }));
      return;
    }
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
        extra: v.layout === 'running' ? [h('div', { class: 'indeterminate', role: 'progressbar', 'aria-label': DISK_TEXT.runningTitle, 'aria-valuetext': t('disk.groupdigitsFound', { groupDigits: st?.entries_seen ?? 0 }) })] : [],
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
