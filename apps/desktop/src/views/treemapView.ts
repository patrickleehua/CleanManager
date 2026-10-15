// One-layer treemap. Area = known logical bytes of the layer's children; unknown sizes are
// listed in the gap panel and never drawn. The chart is an enhancement: tiles are not tab
// stops, and every tile has an equivalent row in the table (GUI-02/05). Rendering failures
// fall back to a message while the table keeps working.

import { formatBytes, formatShare, groupDigits, parseBytes } from '../lib/bytes';
import { CATEGORY_LABEL, categoryOf, type Category } from '../lib/category';
import { h, replace } from '../lib/dom';
import { layoutTreemap, MERGED_ID, type TreemapLayout } from '../lib/treemap';
import type { Entry, TreemapLayer } from '../lib/types';
import { canOpen } from './grid';

export const REST_ID = '__rest__';
const GAP = 3;
const MIN_TILE_AREA = 260;

export interface TreemapCallbacks {
  onSelect(id: string, members?: string[]): void;
  onOpen(entry: Entry): void;
}

export class TreemapView {
  el: HTMLElement;
  private canvas: HTMLElement;
  private legend: HTMLElement;
  private gaps: HTMLElement;
  private layer: TreemapLayer | null = null;
  private byId = new Map<string, Entry>();
  private activeId: string | null = null;
  private frame = 0;

  constructor(private cb: TreemapCallbacks) {
    this.canvas = h('div', { class: 'treemap', role: 'img', 'aria-label': '空间分布图' });
    this.legend = h('ul', { class: 'legend', 'aria-label': '颜色表示文件类别（不表示是否可以删除）' });
    this.gaps = h('div', { class: 'gaps' });
    this.el = h('div', { class: 'treemap-wrap' }, this.canvas, this.legend, this.gaps);
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
  }

  setLayer(layer: TreemapLayer): void {
    this.layer = layer;
    this.byId = new Map(layer.items.map((e) => [e.entry_id, e]));
    this.canvas.classList.remove('loading');
    this.canvas.removeAttribute('aria-busy');
    this.render();
  }

  setActive(id: string | null): void {
    this.activeId = id;
    for (const t of this.canvas.querySelectorAll<HTMLElement>('.tile')) {
      const members = t.dataset.members ? t.dataset.members.split(',') : [];
      t.classList.toggle('active', t.dataset.id === id || (id !== null && members.includes(id)));
    }
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
      this.setError(`图表无法显示（${String(e)}）。下方表格仍可排序、过滤和分页浏览全部结果。`);
    }
  }

  private draw(layer: TreemapLayer): void {
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
    const tiles = layout.tiles.map((t, i) => {
      const entry = this.byId.get(t.id);
      const isMerged = t.id === MERGED_ID;
      const isRest = t.id === REST_ID;
      const cat: Category | 'merged' = entry ? categoryOf(entry) : 'merged';
      if (entry) cats.add(cat as Category);
      const name = entry ? entry.name : isMerged ? `其他 ${groupDigits(t.members?.length ?? 0)} 项` : `其余 ${groupDigits(layer.rest.count)} 项`;
      const size = formatBytes(t.bytes);
      const share = formatShare(t.bytes, total);
      const el = h(
        'div',
        {
          class: 'tile',
          'data-id': t.id,
          'data-cat': cat,
          'data-shade': String(i % 3),
          'data-members': t.members ? t.members.join(',') : null,
          title: `${name}\n${size} · 占此层已知合计 ${share}${isRest ? '\n（未单独绘制，可在表格中继续加载）' : ''}`,
          'aria-hidden': 'true',
        },
        t.w > 74 && t.h > 36 ? h('span', { class: 'tile-name' }, name) : null,
        t.w > 74 && t.h > 52 ? h('span', { class: 'tile-size' }, size) : null,
      );
      el.style.left = `${t.x + GAP / 2}px`;
      el.style.top = `${t.y + GAP / 2}px`;
      el.style.width = `${Math.max(0, t.w - GAP)}px`;
      el.style.height = `${Math.max(0, t.h - GAP)}px`;
      el.addEventListener('click', () => {
        this.cb.onSelect(isRest ? REST_ID : t.id, t.members);
      });
      el.addEventListener('dblclick', () => {
        if (entry && canOpen(entry)) this.cb.onOpen(entry);
      });
      return el;
    });
    if (!tiles.length) {
      replace(this.canvas, h('p', { class: 'treemap-msg' }, layer.items.length ? '此文件夹中没有已知大小大于 0 的条目，无法按面积绘制。' : '此文件夹是空的。'));
    } else {
      replace(this.canvas, ...tiles);
    }
    this.canvas.setAttribute(
      'aria-label',
      `空间分布图：${tiles.length} 个图块，面积按已知逻辑大小，合计 ${formatBytes(total)}。同样的条目可在下方表格中逐项浏览。`,
    );
    replace(this.legend, ...[...cats].map((c) => h('li', {}, h('span', { class: 'swatch', 'data-cat': c, 'aria-hidden': 'true' }), CATEGORY_LABEL[c])));
    this.renderGaps(layer, layout);
    this.setActive(this.activeId);
  }

  private renderGaps(layer: TreemapLayer, layout: TreemapLayout): void {
    const unknown = layout.unknownIds.map((id) => this.byId.get(id)!).filter(Boolean);
    const unread = layer.parent_summary.unread_ranges;
    const restUnknown = layer.rest.unknown_count;
    const items: HTMLElement[] = [];
    if (unknown.length || restUnknown) {
      items.push(
        h(
          'li',
          {},
          h('b', {}, `${groupDigits(unknown.length + restUnknown)} 个条目大小未知`),
          h('span', { class: 'muted' }, '：未计入面积，也不按 0 计算。'),
          unknown.length
            ? h(
                'span',
                { class: 'gap-names' },
                ...unknown.slice(0, 6).map((e) => {
                  const b = h('button', { type: 'button', class: 'link' }, e.name);
                  b.addEventListener('click', () => this.cb.onSelect(e.entry_id));
                  return b;
                }),
                unknown.length > 6 ? `等 ${unknown.length} 项` : null,
              )
            : null,
        ),
      );
    }
    if (unread) {
      items.push(h('li', {}, h('b', {}, `此文件夹下 ${groupDigits(unread)} 个范围无法读取`), h('span', { class: 'muted' }, '：其中内容的大小未知，详见概览中的问题列表。')));
    }
    if (layout.zeroIds.length) {
      items.push(h('li', {}, h('span', { class: 'muted' }, `${groupDigits(layout.zeroIds.length)} 个条目已知大小为 0，未绘制。`)));
    }
    if (!layer.rest.complete) {
      items.push(h('li', {}, h('span', { class: 'muted' }, '此文件夹的直接子项过多，“其余”图块只汇总了已读取的部分。')));
    }
    replace(this.gaps, items.length ? h('h3', { class: 'gaps-title' }, '未绘制的部分') : null, items.length ? h('ul', {}, ...items) : null);
  }
}
