// Squarified treemap (Bruls, Huizing, van Wijk) for ONE directory layer. Area is proportional
// to known logical bytes. Items with unknown size are excluded and reported separately (they
// never receive invented area); zero-byte items take no area either. Small tiles below
// `minTileArea` are folded into one "other known entries" tile that keeps the member IDs so the
// list can still locate each of them.

import { ratio } from './bytes';

export interface TreemapInput {
  id: string;
  bytes: bigint | null;
}

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Tile extends Rect {
  id: string;
  bytes: bigint;
  /** Present on the merged tile: the IDs it stands for (in input order). */
  members?: string[];
}

export interface TreemapLayout {
  tiles: Tile[];
  /** Known total represented by tile area (the area denominator). */
  areaTotal: bigint;
  unknownIds: string[];
  zeroIds: string[];
  mergedIds: string[];
}

export interface LayoutOptions {
  /** Minimum tile area in px²; smaller items merge into the "other" tile. */
  minTileArea?: number;
  /** ID used for the merged tile. */
  mergedId?: string;
  /** Extra known bytes (e.g. children beyond the fetched tiles) drawn as one tile. */
  extra?: { id: string; bytes: bigint; members?: string[] } | null;
}

export const MERGED_ID = '__merged__';

interface Weighted {
  id: string;
  bytes: bigint;
  area: number;
  members?: string[];
}

function worst(row: Weighted[], side: number): number {
  let sum = 0;
  let min = Infinity;
  let max = 0;
  for (const r of row) {
    sum += r.area;
    if (r.area < min) min = r.area;
    if (r.area > max) max = r.area;
  }
  if (sum <= 0 || side <= 0) return Infinity;
  const s2 = side * side;
  const sum2 = sum * sum;
  return Math.max((s2 * max) / sum2, sum2 / (s2 * min));
}

function placeRow(row: Weighted[], rect: Rect, out: Tile[]): Rect {
  const sum = row.reduce((a, r) => a + r.area, 0);
  if (rect.w >= rect.h) {
    // Column on the left, full height.
    const colW = rect.h > 0 ? sum / rect.h : 0;
    let y = rect.y;
    for (const r of row) {
      const h = colW > 0 ? r.area / colW : 0;
      out.push({ id: r.id, bytes: r.bytes, members: r.members, x: rect.x, y, w: colW, h });
      y += h;
    }
    return { x: rect.x + colW, y: rect.y, w: Math.max(0, rect.w - colW), h: rect.h };
  }
  const rowH = rect.w > 0 ? sum / rect.w : 0;
  let x = rect.x;
  for (const r of row) {
    const w = rowH > 0 ? r.area / rowH : 0;
    out.push({ id: r.id, bytes: r.bytes, members: r.members, x, y: rect.y, w, h: rowH });
    x += w;
  }
  return { x: rect.x, y: rect.y + rowH, w: rect.w, h: Math.max(0, rect.h - rowH) };
}

function squarify(items: Weighted[], rect: Rect): Tile[] {
  const out: Tile[] = [];
  let r = { ...rect };
  let row: Weighted[] = [];
  for (const item of items) {
    const side = Math.min(r.w, r.h);
    if (row.length === 0 || worst([...row, item], side) <= worst(row, side)) {
      row.push(item);
    } else {
      r = placeRow(row, r, out);
      row = [item];
    }
  }
  if (row.length) placeRow(row, r, out);
  return out;
}

function compareDesc(a: { bytes: bigint; id: string }, b: { bytes: bigint; id: string }): number {
  if (a.bytes !== b.bytes) return a.bytes > b.bytes ? -1 : 1;
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
}

export function layoutTreemap(input: TreemapInput[], rect: Rect, opts: LayoutOptions = {}): TreemapLayout {
  const unknownIds: string[] = [];
  const zeroIds: string[] = [];
  const known: { id: string; bytes: bigint }[] = [];
  for (const it of input) {
    if (it.bytes === null) unknownIds.push(it.id);
    else if (it.bytes === 0n) zeroIds.push(it.id);
    else known.push({ id: it.id, bytes: it.bytes });
  }
  const extra = opts.extra && opts.extra.bytes > 0n ? opts.extra : null;
  let total = known.reduce((a, k) => a + k.bytes, 0n) + (extra ? extra.bytes : 0n);
  const result: TreemapLayout = { tiles: [], areaTotal: total, unknownIds, zeroIds, mergedIds: [] };
  const fullArea = Math.max(0, rect.w) * Math.max(0, rect.h);
  if (total === 0n || fullArea === 0) return result;

  // Deterministic order: size desc, then id (same tie-break idea as the store's entry_id order).
  known.sort(compareDesc);
  const minArea = opts.minTileArea ?? 0;
  const keep: Weighted[] = [];
  const mergedMembers: string[] = [];
  let mergedBytes = 0n;
  for (const k of known) {
    const area = ratio(k.bytes, total) * fullArea;
    if (area < minArea) {
      mergedMembers.push(k.id);
      mergedBytes += k.bytes;
    } else {
      keep.push({ id: k.id, bytes: k.bytes, area });
    }
  }
  if (extra) {
    keep.push({ id: extra.id, bytes: extra.bytes, area: ratio(extra.bytes, total) * fullArea, members: extra.members });
  }
  if (mergedBytes > 0n) {
    keep.push({ id: opts.mergedId ?? MERGED_ID, bytes: mergedBytes, area: ratio(mergedBytes, total) * fullArea, members: mergedMembers });
  }
  // Areas are ratios of BigInt totals; sum may differ from fullArea by float rounding only.
  keep.sort((a, b) => (a.area !== b.area ? b.area - a.area : a.id < b.id ? -1 : 1));
  total = keep.reduce((a, k) => a + k.bytes, 0n);
  result.areaTotal = total;
  result.mergedIds = mergedMembers;
  result.tiles = squarify(keep, rect);
  return result;
}
