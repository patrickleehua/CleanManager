// P03 disk: which state the page is in and what it shows, plus the pure pieces of the treemap ↔ table
// link (which tile stands for an entry, which entry a tile locates, keyboard movement between tiles)
// and the breadcrumb. Kept free of the DOM so the state matrix and the selection sync are tested.

import { groupDigits, parseBytes } from './bytes';
import { refusalText } from './codes';
import { sizeLabel, type BannerKind } from './display';
import { basisLabel, isTerminal, kindLabel } from './format';
import { overviewPlan } from './overviewModel';
import type { Entry, IpcError, ScanStatus } from './types';

/** Words of the disk page that are not data. One table, so the language switch can replace it whole. */
export const DISK_TEXT = {
  title: '磁盘',
  noneTitle: '还没有可浏览的结果',
  noneBody: '选一个文件夹完成一次只读扫描，这里会按大小画出它的内容。',
  runningTitle: '扫描进行中',
  runningBody: '结果在扫描结束并发布后才能浏览；不会用未发布的数据拼凑排名。',
  stoppingTitle: '正在停止扫描',
  stoppingBody: '停止后会发布已确认的部分，然后就可以浏览。',
  failedTitle: '扫描失败，没有可浏览的结果',
  failedFallback: '扫描没有完成，也没有记录原因。',
  emptyTitle: '这次扫描没有记录任何条目',
  emptyBody: '扫描根目录可能无法读取，原因在概览中。',
  unpublishedTitle: '这次扫描没有发布结果',
  unpublishedBody: '扫描结束前没有确认任何条目，可以重新扫描。',
  errorTitle: '无法读取扫描记录',
  loading: '正在读取扫描记录…',
  partial: (n: number) => (n > 0 ? `这次扫描部分完成：${groupDigits(n)} 处位置没有读到，它们的大小记为未知。` : '这次扫描部分完成：有条目大小未知，它们不计入面积。'),
  pending: '等待 macOS 授权：有位置还没有读取，它们的大小记为未知。',
  cancelled: '扫描已取消：这里只有取消前确认的部分。',
  interrupted: '扫描被中断：这里只有中断前确认的部分。',
  goScope: '选择扫描范围',
  goOverview: '查看扫描进度',
  goReason: '查看原因',
  others: '可以浏览的其他扫描',
} as const;

export type DiskLayout = 'none' | 'loading' | 'error' | 'running' | 'failed' | 'empty' | 'ready';

export interface DiskView {
  layout: DiskLayout;
  /** The one message of a page without results (none uses an empty state, the others a status banner). */
  banner: { kind: BannerKind; title: string; sentence: string } | null;
  /** With results: one line above the chart when the result is not the whole scope. */
  note: { kind: BannerKind; text: string } | null;
  /** The next step: choose a scope, or read the overview (progress, reasons). */
  action: 'scope' | 'overview' | null;
  /** Older scans may be offered when this one cannot be browsed. */
  offerOthers: boolean;
}

export function diskView(o: { scanId: string | null; status: ScanStatus | null; statusError: IpcError | null }): DiskView {
  const t = DISK_TEXT;
  const blocked = (layout: DiskLayout, kind: BannerKind, title: string, sentence: string, action: DiskView['action']): DiskView => ({ layout, banner: { kind, title, sentence }, note: null, action, offerOthers: true });
  if (!o.scanId) return { layout: 'none', banner: null, note: null, action: 'scope', offerOthers: true };
  const s = o.status;
  if (!s) return o.statusError ? blocked('error', 'failed', t.errorTitle, refusalText(o.statusError), 'scope') : { layout: 'loading', banner: null, note: null, action: null, offerOthers: false };
  if (s.scan_id !== o.scanId) return { layout: 'loading', banner: null, note: null, action: null, offerOthers: false };
  if (!isTerminal(s.state)) {
    const stopping = s.state === 'stopping' || s.cancel_requested;
    return blocked('running', 'running', stopping ? t.stoppingTitle : t.runningTitle, stopping ? t.stoppingBody : t.runningBody, 'overview');
  }
  if (s.state === 'failed') {
    const p = overviewPlan(s);
    return blocked('failed', 'failed', t.failedTitle, p.reason?.text ?? t.failedFallback, p.chooseRoot === 'primary' ? 'scope' : 'overview');
  }
  if (!s.published) return blocked('empty', 'cancelled', t.unpublishedTitle, t.unpublishedBody, 'scope');
  if (!s.root_entry_id) return blocked('empty', 'cancelled', t.emptyTitle, t.emptyBody, 'overview');
  // Results exist: say once, above the chart, when they are not the whole scope.
  let note: DiskView['note'] = null;
  if (s.state === 'cancelled') note = { kind: 'cancelled', text: t.cancelled };
  else if (s.state === 'interrupted') note = { kind: 'cancelled', text: t.interrupted };
  else {
    const p = overviewPlan(s);
    if (p.layout === 'partial') note = p.pendingPermission ? { kind: 'pending-permission', text: t.pending } : { kind: 'partial', text: t.partial(p.gaps?.unread ?? 0) };
  }
  return { layout: 'ready', banner: null, note, action: note ? 'overview' : null, offerOthers: false };
}

// ---------------------------------------------------------------------------------------------
// Treemap ↔ table

export interface TileRef {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
  /** The merged "其他 N 项" tile stands for these entries. */
  members?: string[];
}

/** The tile that shows `entryId`: its own tile, or the merged tile it was folded into. */
export function tileForEntry(tiles: Pick<TileRef, 'id' | 'members'>[], entryId: string | null): string | null {
  if (!entryId) return null;
  for (const t of tiles) if (t.id === entryId) return t.id;
  for (const t of tiles) if (t.members?.includes(entryId)) return t.id;
  return null;
}

/**
 * The table row a tile locates: the entry itself; for the merged tile its largest member (the first);
 * for the "rest" tile nothing (its children are not loaded — the table pages on to them).
 */
export function entryForTile(tile: Pick<TileRef, 'id' | 'members'>, restId: string): { entryId: string | null; folded: boolean } {
  if (tile.id === restId) return { entryId: null, folded: true };
  if (tile.members?.length) return { entryId: tile.members[0], folded: true };
  return { entryId: tile.id, folded: false };
}

export type TileMove = 'left' | 'right' | 'up' | 'down' | 'first' | 'last';

/**
 * Keyboard movement between tiles: the nearest tile in that direction (its edge beyond ours, overlap
 * along the other axis preferred); first / last follow the size order. Returns the current tile when
 * nothing lies that way, and the first tile when nothing is current.
 */
export function neighborTile(tiles: TileRef[], fromId: string | null, move: TileMove): string | null {
  if (!tiles.length) return null;
  if (move === 'first') return tiles[0].id;
  if (move === 'last') return tiles[tiles.length - 1].id;
  const from = tiles.find((t) => t.id === fromId);
  if (!from) return tiles[0].id;
  const eps = 0.5;
  const cx = from.x + from.w / 2;
  const cy = from.y + from.h / 2;
  let best: { id: string; score: number } | null = null;
  for (const t of tiles) {
    if (t.id === from.id) continue;
    let gap: number;
    let overlap: number;
    let off: number;
    if (move === 'right' || move === 'left') {
      gap = move === 'right' ? t.x - (from.x + from.w) : from.x - (t.x + t.w);
      overlap = Math.min(from.y + from.h, t.y + t.h) - Math.max(from.y, t.y);
      off = Math.abs(t.y + t.h / 2 - cy);
    } else {
      gap = move === 'down' ? t.y - (from.y + from.h) : from.y - (t.y + t.h);
      overlap = Math.min(from.x + from.w, t.x + t.w) - Math.max(from.x, t.x);
      off = Math.abs(t.x + t.w / 2 - cx);
    }
    if (gap < -eps) continue;
    // Tiles that share an edge segment win over ones that only lie diagonally that way.
    const score = Math.max(0, gap) * 4 + off + (overlap > eps ? 0 : 10_000);
    if (!best || score < best.score) best = { id: t.id, score };
  }
  return best ? best.id : from.id;
}

// ---------------------------------------------------------------------------------------------
// Breadcrumb

export type Crumb = { kind: 'crumb'; depth: number; name: string } | { kind: 'more'; depths: number[] };

/** Long trails keep the root, an ellipsis for the middle (it opens a list), and the last `tail` levels. */
export function crumbTrail(names: string[], tail = 3): Crumb[] {
  const all: Crumb[] = names.map((name, depth) => ({ kind: 'crumb', depth, name }));
  if (names.length <= tail + 2) return all;
  const hidden = names.slice(1, names.length - tail).map((_, i) => i + 1);
  return [all[0], { kind: 'more', depths: hidden }, ...all.slice(names.length - tail)];
}

// ---------------------------------------------------------------------------------------------
// Detail pane: what it is → its size → (actions); the evidence behind the size stays secondary.

export interface EntryDetail {
  /** "文件夹", "文件", "目录联接（未跟随）"… */
  kind: string;
  /** Logical size; "未知" when unknown, "至少 X" when children are unknown. */
  size: string;
  sizeBytes: bigint | null;
  allocated: string;
  /** Children whose size is unknown, in words; null when there are none. */
  unknownNote: string | null;
  basis: string;
  /** Not entered (link, mount, cloud, own data): why its size is what it is. */
  boundaryNote: string | null;
  weakIdentity: boolean;
}

function safeBytes(v: string | null): bigint | null {
  try {
    return parseBytes(v);
  } catch {
    return null;
  }
}

export function entryDetail(e: Entry): EntryDetail {
  const logical = safeBytes(e.logical_bytes);
  const unknown = e.unknown_descendants ?? 0;
  return {
    kind: kindLabel(e),
    size: sizeLabel(logical, { unknownCount: unknown }),
    sizeBytes: logical,
    allocated: sizeLabel(safeBytes(e.allocated_bytes)),
    unknownNote: unknown > 0 ? `其中 ${groupDigits(unknown)} 项大小未知，没有计入` : null,
    basis: basisLabel(e),
    boundaryNote: e.boundary ? '这是链接或云端边界：扫描记录了它，但没有进入，也不把目标的大小算进来。' : null,
    weakIdentity: e.identity_quality === 'weak',
  };
}
