// One-layer treemap. Area = known logical bytes of the layer's children; unknown sizes are listed in
// the gap list and never drawn. The chart is linked to the table both ways: choosing a tile (pointer or
// arrow keys) locates the same entry in the table, and the table's current row lights its tile (or the
// merged tile it was folded into). The chart is one tab stop — a listbox whose options are the tiles —
// and every tile has an equivalent table row (GUI-02/05). Rendering failures fall back to a message
// while the table keeps working.

import { formatBytes, formatShare, parseBytes } from '../lib/bytes';
import { CATEGORY_LABEL, categoryOf, type Category } from '../lib/category';
import { neighborTile, tileForEntry, type TileMove, type TileRef } from '../lib/diskModel';
import { h, live, replace } from '../lib/dom';
import { layoutTreemap, MERGED_ID, type TreemapLayout } from '../lib/treemap';
import type { Entry, TreemapLayer } from '../lib/types';
import { canOpen } from './grid';
import { removedBadge, REMOVED_TEXT } from '../lib/removedModel';
import { t } from '../i18n';

export const REST_ID = '__rest__';
const GAP = 3;
const MIN_TILE_AREA = 260;

/** Words of the chart (one table for the language switch). */
export const TREEMAP_TEXT = {
  get label() { return t('treemapView.label'); },
  get legend() { return t('treemapView.legend'); },
  merged: (n: number) => t('treemapView.merged', { n }),
  rest: (n: number) => t('treemapView.rest', { n }),
  share: (s: string) => t('treemapView.share', { s }),
  get restNote() { return t('treemapView.restNote'); },
  get folder() { return t('treemapView.folder'); },
  aria: (n: number, total: string) => t('treemapView.aria', { n, total }),
  get noTiles() { return t('treemapView.noTiles'); },
  get emptyFolder() { return t('treemapView.emptyFolder'); },
  failed: (e: string) => t('treemapView.failed', { e }),
  get gapsTitle() { return t('treemapView.gapsTitle'); },
  unknown: (n: number) => t('treemapView.unknown', { n }),
  get unknownNote() { return t('treemapView.unknownNote'); },
  unread: (n: number) => t('treemapView.unread', { n }),
  get unreadNote() { return t('treemapView.unreadNote'); },
  zero: (n: number) => t('treemapView.zero', { n }),
  get restPartial() { return t('treemapView.restPartial'); },
  more: (n: number) => t('treemapView.more', { n }),
  get removedLegend() { return t('treemapView.removedLegend'); },
} as const;

export interface TreemapCallbacks {
  /** A tile became current (click or arrow keys): the table locates the same entry. */
  onSelect(id: string, members: string[] | undefined, how: 'pointer' | 'keyboard'): void;
  onOpen(entry: Entry): void;
  onUp(): void;
  onToggle(entry: Entry): void;
}

const KEY_MOVE: Record<string, TileMove> = { ArrowLeft: 'left', ArrowRight: 'right', ArrowUp: 'up', ArrowDown: 'down', Home: 'first', End: 'last' };

export class TreemapView {
  el: HTMLElement;
  private canvas: HTMLElement;
  private legend: HTMLElement;
  private gaps: HTMLElement;
  private layer: TreemapLayer | null = null;
  private byId = new Map<string, Entry>();
  private tiles: TileRef[] = [];
  /** The entry the table has current. */
  private activeEntry: string | null = null;
  /** The tile the keyboard is on (may be the merged or the "rest" tile). */
  private currentTile: string | null = null;
  private hoverEntry: string | null = null;
  private frame = 0;

  constructor(private cb: TreemapCallbacks) {
    this.canvas = live(h('div', { class: 'treemap', role: 'listbox', tabindex: '0', 'aria-orientation': 'horizontal' }), null, { 'aria-label': () => TREEMAP_TEXT.label });
    this.legend = live(h('ul', { class: 'legend' }), null, { 'aria-label': () => TREEMAP_TEXT.legend });
    this.gaps = h('div', { class: 'gaps' });
    this.el = h('div', { class: 'treemap-wrap' }, this.canvas, this.legend, this.gaps);
    this.canvas.addEventListener('keydown', (e) => this.onKey(e));
    this.canvas.addEventListener('focus', () => {
      // Arriving with the keyboard starts on the tile of the table's current row, else the largest tile.
      if (!this.currentTile || !this.tiles.some((t) => t.id === this.currentTile)) {
        const start = tileForEntry(this.tiles, this.activeEntry) ?? this.tiles[0]?.id ?? null;
        if (start) this.choose(start, 'keyboard');
      }
    });
    new ResizeObserver(() => this.schedule()).observe(this.canvas);
  }

  setLoading(): void {
    this.canvas.classList.add('loading');
    this.canvas.setAttribute('aria-busy', 'true');
  }

  setError(message: string): void {
    this.canvas.classList.remove('loading');
    this.canvas.removeAttribute('aria-busy');
    replace(this.canvas, h('p', { class: 'treemap-msg', role: 'note' }, message));
    replace(this.legend);
    replace(this.gaps);
    this.layer = null;
    this.tiles = [];
  }

  setLayer(layer: TreemapLayer): void {
    this.layer = layer;
    this.byId = new Map(layer.items.map((e) => [e.entry_id, e]));
    this.currentTile = null;
    this.activeEntry = null;
    this.canvas.classList.remove('loading');
    this.canvas.removeAttribute('aria-busy');
    this.render();
  }

  /** The table's current row: light its tile (or the merged tile holding it). */
  setActive(entryId: string | null): void {
    this.activeEntry = entryId;
    // An entry without a tile (unknown size, not drawn) lights nothing rather than an old tile.
    this.currentTile = tileForEntry(this.tiles, entryId);
    this.paint();
  }

  /** The row under the pointer in the table: a lighter mark on its tile. */
  setHover(entryId: string | null): void {
    this.hoverEntry = entryId;
    const tile = tileForEntry(this.tiles, entryId);
    for (const t of this.canvas.querySelectorAll<HTMLElement>('.tile')) t.classList.toggle('hover', t.dataset.id === tile);
  }

  focus(): void {
    this.canvas.focus({ preventScroll: true });
  }

  private paint(): void {
    const lit = this.currentTile && this.tiles.some((t) => t.id === this.currentTile) ? this.currentTile : null;
    for (const t of this.canvas.querySelectorAll<HTMLElement>('.tile')) {
      const on = t.dataset.id === lit;
      t.classList.toggle('active', on);
      t.setAttribute('aria-selected', on ? 'true' : 'false');
    }
    this.canvas.toggleAttribute('data-has-active', !!lit);
    if (lit) this.canvas.setAttribute('aria-activedescendant', this.tileDomId(lit));
    else this.canvas.removeAttribute('aria-activedescendant');
  }

  private tileDomId(id: string): string {
    return `tm-tile-${id}`;
  }

  private choose(tileId: string, how: 'pointer' | 'keyboard'): void {
    const t = this.tiles.find((x) => x.id === tileId);
    if (!t) return;
    this.currentTile = tileId;
    this.paint();
    this.cb.onSelect(tileId, t.members, how);
  }

  private onKey(e: KeyboardEvent): void {
    if (!this.tiles.length) return;
    const move = KEY_MOVE[e.key];
    const entry = this.currentTile ? this.byId.get(this.currentTile) : undefined;
    if (move && !(e.key === 'ArrowUp' && e.altKey)) {
      const next = neighborTile(this.tiles, this.currentTile, move);
      if (next) this.choose(next, 'keyboard');
    } else if (e.key === 'Enter') {
      if (entry && canOpen(entry)) this.cb.onOpen(entry);
      else if (this.currentTile) this.choose(this.currentTile, 'keyboard');
    } else if (e.key === ' ') {
      if (entry) this.cb.onToggle(entry);
    } else if (e.key === 'Backspace' || (e.key === 'ArrowUp' && e.altKey)) {
      this.cb.onUp();
    } else return;
    e.preventDefault();
  }

  /** LIF-05: draw the tiles, legend and gaps again in the new language. */
  relocalize(): void {
    this.schedule();
  }

  private schedule(): void {
    if (this.frame) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      if (this.layer) this.render();
    });
  }

  private render(): void {
    const layer = this.layer;
    if (!layer) return;
    try {
      this.draw(layer);
    } catch (e) {
      this.setError(TREEMAP_TEXT.failed(String(e)));
    }
  }

  private draw(layer: TreemapLayer): void {
    const T = TREEMAP_TEXT;
    const w = this.canvas.clientWidth;
    const hgt = this.canvas.clientHeight;
    const restBytes = parseBytes(layer.rest.known_logical_bytes) ?? 0n;
    const layout = layoutTreemap(
      layer.items.map((e) => ({ id: e.entry_id, bytes: parseBytes(e.logical_bytes) })),
      { x: 0, y: 0, w, h: hgt },
      { minTileArea: MIN_TILE_AREA, extra: restBytes > 0n ? { id: REST_ID, bytes: restBytes } : null },
    );
    const total = layout.areaTotal;
    const cats = new Set<Category>();
    let anyRemoved = false;
    this.tiles = layout.tiles.map((t) => ({ id: t.id, x: t.x, y: t.y, w: t.w, h: t.h, members: t.members }));
    const tiles = layout.tiles.map((tile, i) => {
      const entry = this.byId.get(tile.id);
      const isMerged = tile.id === MERGED_ID;
      const isRest = tile.id === REST_ID;
      const cat: Category | 'merged' = entry ? categoryOf(entry) : 'merged';
      if (entry) cats.add(cat as Category);
      const name = entry ? entry.name : isMerged ? T.merged(tile.members?.length ?? 0) : T.rest(layer.rest.count);
      const size = formatBytes(tile.bytes);
      const share = formatShare(tile.bytes, total);
      const folder = !!entry && canOpen(entry);
      const big = tile.w > 74 && tile.h > 36;
      // Removed after the scan: the tile keeps its place (the snapshot's layout), drawn as a grey hatch.
      const removed = entry?.removed ? removedBadge(entry.removed) : null;
      if (removed) anyRemoved = true;
      const el = h(
        'div',
        {
          class: 'tile',
          id: this.tileDomId(tile.id),
          role: 'option',
          'aria-selected': 'false',
          'aria-label': `${t('treemapView.nameSizeShareP', { name, size, share: T.share(share), p: folder ? t('treemapView.folderSuffix', { folder: T.folder }) : '', p2: isRest ? t('treemapView.restnote', { restNote: T.restNote }) : '' })}${removed ? `${REMOVED_TEXT.tileSuffix}${t('common.labelColon', { p: removed.text })}` : ''}`,
          'data-id': tile.id,
          'data-cat': cat,
          'data-shade': String(i % 3),
          'data-members': tile.members ? tile.members.join(',') : null,
          'data-removed': entry?.removed ? entry.removed.mode : null,
          title: `${name}\n${size} · ${T.share(share)}${isRest ? t('treemapView.restNoteLine', { restNote: T.restNote }) : ''}${removed ? `\n${removed.text}` : ''}`,
        },
        big ? h('span', { class: 'tile-name', 'aria-hidden': 'true' }, name) : null,
        big && tile.h > 52 ? h('span', { class: 'tile-size', 'aria-hidden': 'true' }, size, tile.h > 70 && tile.w > 110 ? h('span', { class: 'tile-share' }, share) : null) : null,
      );
      el.style.left = `${tile.x + GAP / 2}px`;
      el.style.top = `${tile.y + GAP / 2}px`;
      el.style.width = `${Math.max(0, tile.w - GAP)}px`;
      el.style.height = `${Math.max(0, tile.h - GAP)}px`;
      el.addEventListener('click', () => {
        this.choose(tile.id, 'pointer');
        this.focus();
      });
      el.addEventListener('dblclick', () => {
        if (entry && canOpen(entry)) this.cb.onOpen(entry);
      });
      return el;
    });
    if (!tiles.length) {
      replace(this.canvas, h('p', { class: 'treemap-msg' }, layer.items.length ? T.noTiles : T.emptyFolder));
      this.canvas.setAttribute('tabindex', '-1');
    } else {
      replace(this.canvas, ...tiles);
      this.canvas.setAttribute('tabindex', '0');
    }
    this.canvas.setAttribute('aria-label', T.aria(tiles.length, formatBytes(total)));
    replace(
      this.legend,
      ...[...cats].map((c) => h('li', {}, h('span', { class: 'swatch', 'data-cat': c, 'aria-hidden': 'true' }), CATEGORY_LABEL[c])),
      anyRemoved ? h('li', {}, h('span', { class: 'swatch swatch-removed', 'aria-hidden': 'true' }), T.removedLegend) : null,
    );
    this.renderGaps(layer, layout);
    if (!this.currentTile || !this.tiles.some((t) => t.id === this.currentTile)) this.currentTile = tileForEntry(this.tiles, this.activeEntry);
    this.paint();
    if (this.hoverEntry) this.setHover(this.hoverEntry);
  }

  private renderGaps(layer: TreemapLayer, layout: TreemapLayout): void {
    const T = TREEMAP_TEXT;
    const unknown = layout.unknownIds.map((id) => this.byId.get(id)!).filter(Boolean);
    const unread = layer.parent_summary.unread_ranges;
    const restUnknown = layer.rest.unknown_count;
    const items: HTMLElement[] = [];
    if (unknown.length || restUnknown) {
      items.push(
        h(
          'li',
          { 'data-mark': 'unknown' },
          h('span', { class: 'gap-mark', 'aria-hidden': 'true' }),
          h(
            'span',
            { class: 'gap-text' },
            h('b', {}, T.unknown(unknown.length + restUnknown)),
            h('span', { class: 'muted' }, t('treemapView.unknownnote', { unknownNote: T.unknownNote })),
            unknown.length
              ? h(
                  'span',
                  { class: 'gap-names' },
                  ...unknown.slice(0, 6).map((e) => {
                    const b = h('button', { type: 'button', class: 'link' }, e.name);
                    b.addEventListener('click', () => this.cb.onSelect(e.entry_id, undefined, 'pointer'));
                    return b;
                  }),
                  unknown.length > 6 ? h('span', { class: 'muted' }, T.more(unknown.length)) : null,
                )
              : null,
          ),
        ),
      );
    }
    if (unread) items.push(h('li', { 'data-mark': 'unread' }, h('span', { class: 'gap-mark', 'aria-hidden': 'true' }), h('span', { class: 'gap-text' }, h('b', {}, T.unread(unread)), h('span', { class: 'muted' }, t('treemapView.unreadnote', { unreadNote: T.unreadNote })))));
    if (layout.zeroIds.length) items.push(h('li', { 'data-mark': 'zero' }, h('span', { class: 'gap-mark', 'aria-hidden': 'true' }), h('span', { class: 'gap-text muted' }, T.zero(layout.zeroIds.length))));
    if (!layer.rest.complete) items.push(h('li', { 'data-mark': 'zero' }, h('span', { class: 'gap-mark', 'aria-hidden': 'true' }), h('span', { class: 'gap-text muted' }, T.restPartial)));
    replace(this.gaps, items.length ? h('h3', { class: 'gaps-title' }, T.gapsTitle) : null, items.length ? h('ul', {}, ...items) : null);
  }
}
