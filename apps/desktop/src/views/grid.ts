// Virtualized, keyboard-operable entry table (role=grid). Only rows in the scroll window are in
// the DOM, plus the active row, which is always kept rendered so `aria-activedescendant` never
// points at a recycled node (GUI-05). Selection is by entry_id and lives outside the grid, so
// it survives paging, re-sorting and filtering.

import { formatBytes, formatExactBytes, parseBytes, ratio } from '../lib/bytes';
import { categoryOf } from '../lib/category';
import { hintLabel, hintBasisText, hintOf, hintTitle } from '../lib/cleanable';
import { h, live, replace } from '../lib/dom';
import { kindLabel } from '../lib/format';
import { folderSizeText, removedBadge, removedBeneath, REMOVED_TEXT } from '../lib/removedModel';
import type { Entry, SortKey } from '../lib/types';
import { t } from '../i18n';

export const ROW_HEIGHT = 38;
const OVERSCAN = 8;

export interface GridCallbacks {
  onActive(entry: Entry): void;
  onOpen(entry: Entry): void;
  onUp(): void;
  onToggle(entry: Entry): void;
  onSort(sort: SortKey): void;
  onNeedMore(): void;
  isSelected(id: string): boolean;
  /** The row under the pointer (the chart marks its tile); null when the pointer leaves the table. */
  onHover?(entry: Entry | null): void;
}

export interface GridData {
  rows: Entry[];
  complete: boolean;
  loading: boolean;
  /** Known total of the current layer, for the share bars (null in whole-scan mode). */
  shareBase: bigint | null;
  sort: SortKey;
  emptyText: string;
}

export function canOpen(e: Entry): boolean {
  return e.kind === 'directory' && !e.boundary;
}

export class EntryGrid {
  el: HTMLElement;
  private body: HTMLElement;
  private spacer: HTMLElement;
  private head: HTMLElement;
  private empty: HTMLElement;
  private data: GridData = { rows: [], complete: false, loading: false, shareBase: null, sort: 'logical_desc', emptyText: '' };
  private index = new Map<string, number>();
  private activeId: string | null = null;
  private frame = 0;

  constructor(
    private idPrefix: string,
    label: () => string,
    private cb: GridCallbacks,
  ) {
    this.head = h('div', { class: 'grid-row grid-head', role: 'row', 'aria-rowindex': '1' });
    this.spacer = h('div', { class: 'grid-spacer', role: 'presentation' });
    this.body = h('div', { class: 'grid-body', role: 'rowgroup' }, this.spacer);
    this.empty = h('div', { class: 'grid-empty', hidden: true });
    this.el = live(
      h('div', { class: 'grid', role: 'grid', 'aria-multiselectable': 'true', 'aria-colcount': '5', tabindex: '0' }, h('div', { role: 'rowgroup', class: 'grid-headgroup' }, this.head), this.body, this.empty),
      null,
      { 'aria-label': label },
    );
    this.body.addEventListener('scroll', () => this.schedule());
    this.body.addEventListener('mouseleave', () => this.cb.onHover?.(null));
    this.el.addEventListener('keydown', (e) => this.onKey(e));
    this.el.addEventListener('focus', () => {
      if (!this.activeId && this.data.rows.length) this.setActive(this.data.rows[0].entry_id, true, true);
    });
    new ResizeObserver(() => this.schedule()).observe(this.body);
    this.renderHead();
  }

  setData(data: GridData): void {
    const sortChanged = data.sort !== this.data.sort;
    this.data = data;
    this.index = new Map(data.rows.map((r, i) => [r.entry_id, i]));
    if (this.activeId && !this.index.has(this.activeId)) this.activeId = null;
    const total = data.rows.length;
    // Row count is only stated once known (complete); otherwise -1 = unknown (no invented total).
    this.el.setAttribute('aria-rowcount', data.complete ? String(total + 1) : '-1');
    this.spacer.style.height = `${(total + (data.complete ? 0 : 1)) * ROW_HEIGHT}px`;
    this.empty.hidden = total > 0 || data.loading;
    this.empty.textContent = data.emptyText;
    if (sortChanged) this.renderHead();
    this.syncActiveDescendant();
    this.render();
  }

  get active(): string | null {
    return this.activeId;
  }

  reset(): void {
    this.activeId = null;
    this.body.scrollTop = 0;
  }

  /** Make `id` the active row; returns false when it is not loaded. */
  setActive(id: string, scroll: boolean, notify = false): boolean {
    const i = this.index.get(id);
    if (i === undefined) return false;
    this.activeId = id;
    if (scroll) this.scrollToIndex(i);
    this.syncActiveDescendant();
    this.render();
    if (notify) this.cb.onActive(this.data.rows[i]);
    return true;
  }

  focus(): void {
    this.el.focus({ preventScroll: true });
  }

  refreshSelection(): void {
    this.render();
  }

  private rowId(id: string): string {
    return `${this.idPrefix}-row-${id}`;
  }

  private syncActiveDescendant(): void {
    if (this.activeId) this.el.setAttribute('aria-activedescendant', this.rowId(this.activeId));
    else this.el.removeAttribute('aria-activedescendant');
  }

  private scrollToIndex(i: number): void {
    const top = i * ROW_HEIGHT;
    const view = this.body.clientHeight;
    if (top < this.body.scrollTop) this.body.scrollTop = top;
    else if (top + ROW_HEIGHT > this.body.scrollTop + view) this.body.scrollTop = top + ROW_HEIGHT - view;
  }

  /** LIF-05: draw the head and the visible rows again in the new language. */
  relocalize(): void {
    this.renderHead();
    this.schedule();
  }

  private schedule(): void {
    if (this.frame) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      this.render();
    });
  }

  private renderHead(): void {
    const sortBtn = (label: string, key: SortKey | null) => {
      if (!key) return h('div', { role: 'columnheader', class: 'col' }, label);
      const active = this.data.sort === key;
      const b = h('button', { type: 'button', class: `sort${active ? ' active' : ''}`, tabindex: '-1' }, label, active ? h('span', { 'aria-hidden': 'true', class: 'sort-mark' }, key === 'name_asc' ? '↑' : '↓') : null);
      b.addEventListener('click', () => this.cb.onSort(key));
      return h('div', { role: 'columnheader', class: 'col', 'aria-sort': active ? (key === 'name_asc' ? 'ascending' : 'descending') : 'none' }, b);
    };
    replace(
      this.head,
      sortBtn(t('common.name'), 'name_asc'),
      sortBtn(t('common.logicalSize'), 'logical_desc'),
      sortBtn(t('grid.allocated'), 'allocated_desc'),
      sortBtn(t('common.type'), null),
      sortBtn(t('common.notes'), null),
    );
    this.head.firstElementChild?.classList.add('col-name');
    (this.head.children[1] as HTMLElement).classList.add('col-num');
    (this.head.children[2] as HTMLElement).classList.add('col-num', 'col-alloc');
    (this.head.children[3] as HTMLElement).classList.add('col-kind');
    (this.head.children[4] as HTMLElement).classList.add('col-note');
  }

  private render(): void {
    const { rows } = this.data;
    const viewH = this.body.clientHeight || 400;
    const first = Math.max(0, Math.floor(this.body.scrollTop / ROW_HEIGHT) - OVERSCAN);
    const last = Math.min(rows.length - 1, Math.ceil((this.body.scrollTop + viewH) / ROW_HEIGHT) + OVERSCAN);
    const wanted = new Set<number>();
    for (let i = first; i <= last; i++) wanted.add(i);
    const activeIndex = this.activeId ? this.index.get(this.activeId) : undefined;
    if (activeIndex !== undefined) wanted.add(activeIndex);

    const nodes: HTMLElement[] = [];
    for (const i of [...wanted].sort((a, b) => a - b)) nodes.push(this.renderRow(rows[i], i));
    if (!this.data.complete) {
      const loader = h('div', { class: 'grid-loader', role: 'row', 'aria-rowindex': String(rows.length + 2) }, h('div', { role: 'gridcell' }, this.data.loading ? t('grid.loadingMore') : t('grid.scrollDownLoadMore')));
      loader.style.top = `${rows.length * ROW_HEIGHT}px`;
      nodes.push(loader);
    }
    replace(this.spacer, ...nodes);

    // Prefetch when the window nears the end of what is loaded.
    if (!this.data.complete && !this.data.loading && last >= rows.length - OVERSCAN) this.cb.onNeedMore();
  }

  private renderRow(e: Entry, i: number): HTMLElement {
    const selected = this.cb.isSelected(e.entry_id);
    const active = e.entry_id === this.activeId;
    const logical = parseBytes(e.logical_bytes);
    const share = this.data.shareBase && logical !== null ? ratio(logical, this.data.shareBase) : 0;
    const note: string[] = [];
    if (e.size_basis === 'unknown') note.push(t('common.sizeUnknown'));
    else if (e.unknown_descendants) note.push(t('grid.unknownDescendants', { unknown_descendants: e.unknown_descendants }));
    if (e.identity_quality === 'weak') note.push(t('grid.weakIdentity'));
    // A folder the scan saw whole: what later operations removed beneath it (its own size stays the snapshot's).
    const beneath = removedBeneath(e);
    if (beneath !== null) note.unshift(REMOVED_TEXT.removedShort(beneath));
    const bar = h('span', { class: 'share', 'aria-hidden': 'true' });
    bar.style.setProperty('--share', String(share));
    // DIN-05: read-only marker; the basis is in the tooltip and for screen readers. It never selects.
    const hint = hintOf(e);
    const hintTag = hint
      ? h('span', { class: 'hint-tag', title: hintTitle(hint) }, h('span', { class: 'hint-mark', 'aria-hidden': 'true' }), hintLabel(), h('span', { class: 'sr-only' }, t('grid.basisHint', { hint: hintBasisText(hint) })))
      : null;
    // Removed after the scan: dimmed, struck through, with how it left; never selectable (the page says why).
    const badge = e.removed ? removedBadge(e.removed) : null;
    const removedTag = badge
      ? h('span', { class: 'removed-tag', 'data-tone': badge.tone, title: badge.note ? `${badge.text}${t('common.commaSep')}${badge.note}` : badge.text }, h('span', { class: 'removed-mark', 'aria-hidden': 'true' }), badge.text)
      : null;
    const row = h(
      'div',
      {
        class: `grid-row${active ? ' active' : ''}${selected ? ' selected' : ''}${e.removed ? ' removed' : ''}`,
        role: 'row',
        id: this.rowId(e.entry_id),
        'aria-rowindex': String(i + 2),
        'aria-selected': selected ? 'true' : 'false',
        'aria-disabled': e.removed && !selected ? 'true' : null,
        'data-id': e.entry_id,
      },
      h(
        'div',
        { role: 'gridcell', class: 'col col-name' },
        h('span', { class: 'check', 'aria-hidden': 'true' }),
        h('span', { class: 'swatch', 'data-cat': categoryOf(e), 'aria-hidden': 'true' }),
        h('span', { class: 'name', title: e.path_display }, e.name),
        removedTag,
        removedTag ? null : hintTag,
        canOpen(e) ? h('span', { class: 'chev', 'aria-hidden': 'true' }, '›') : null,
      ),
      h('div', { role: 'gridcell', class: 'col col-num', title: beneath !== null ? `${folderSizeText(e)}\n${formatExactBytes(logical)}` : formatExactBytes(logical) }, h('span', {}, formatBytes(logical))),
      h('div', { role: 'gridcell', class: 'col col-num col-alloc', title: formatExactBytes(e.allocated_bytes) }, formatBytes(e.allocated_bytes)),
      h('div', { role: 'gridcell', class: 'col col-kind', title: kindLabel(e) }, kindLabel(e)),
      h('div', { role: 'gridcell', class: 'col col-note', title: note.join(t('common.commaSep')) || null }, note.join(t('common.commaSep'))),
    );
    row.style.top = `${i * ROW_HEIGHT}px`;
    // The share bar runs along the row's bottom edge (a fill of the divider), so it never reads as an underline.
    row.append(bar);
    row.addEventListener('mousedown', (ev) => {
      if ((ev.target as HTMLElement).classList.contains('check') || ev.ctrlKey || ev.metaKey) {
        ev.preventDefault();
        this.setActive(e.entry_id, false, true);
        this.cb.onToggle(e);
        this.focus();
        return;
      }
      this.setActive(e.entry_id, false, true);
      requestAnimationFrame(() => this.focus());
    });
    row.addEventListener('dblclick', () => {
      if (canOpen(e)) this.cb.onOpen(e);
    });
    row.addEventListener('mouseenter', () => this.cb.onHover?.(e));
    return row;
  }

  private move(delta: number): void {
    const rows = this.data.rows;
    if (!rows.length) return;
    const cur = this.activeId ? (this.index.get(this.activeId) ?? -1) : -1;
    const next = Math.max(0, Math.min(rows.length - 1, cur + delta));
    this.setActive(rows[next].entry_id, true, true);
    if (next >= rows.length - 2 && !this.data.complete && !this.data.loading) this.cb.onNeedMore();
  }

  private onKey(e: KeyboardEvent): void {
    if ((e.target as HTMLElement).tagName === 'BUTTON') return;
    const page = Math.max(1, Math.floor(this.body.clientHeight / ROW_HEIGHT) - 1);
    const active = this.activeId ? this.data.rows[this.index.get(this.activeId) ?? -1] : undefined;
    switch (e.key) {
      case 'ArrowDown':
        this.move(1);
        break;
      case 'ArrowUp':
        if (e.altKey) this.cb.onUp();
        else this.move(-1);
        break;
      case 'PageDown':
        this.move(page);
        break;
      case 'PageUp':
        this.move(-page);
        break;
      case 'Home':
        this.move(-Infinity);
        break;
      case 'End':
        this.move(Infinity);
        break;
      case ' ':
        if (active) this.cb.onToggle(active);
        break;
      case 'Enter':
        if (active && canOpen(active)) this.cb.onOpen(active);
        break;
      case 'Backspace':
        this.cb.onUp();
        break;
      default:
        return;
    }
    e.preventDefault();
  }
}
