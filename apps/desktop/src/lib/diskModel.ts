// P03 disk: which state the page is in and what it shows, plus the pure pieces of the treemap ↔ table
// link (which tile stands for an entry, which entry a tile locates, keyboard movement between tiles)
// and the breadcrumb. Kept free of the DOM so the state matrix and the selection sync are tested.

import { parseBytes } from './bytes';
import { refusalText } from './codes';
import { sizeLabel, type BannerKind } from './display';
import { basisLabel, isTerminal, kindLabel } from './format';
import { overviewPlan } from './overviewModel';
import type { Entry, IpcError, ProtectionItem, ScanStatus } from './types';
import { t } from '../i18n';

/** Words of the disk page that are not data. One table, so the language switch can replace it whole. */
export const DISK_TEXT = {
  get title() { return t('diskModel.diskText.title'); },
  get noneTitle() { return t('diskModel.diskText.noneTitle'); },
  get noneBody() { return t('diskModel.diskText.noneBody'); },
  get runningTitle() { return t('diskModel.diskText.runningTitle'); },
  get runningBody() { return t('diskModel.diskText.runningBody'); },
  get stoppingTitle() { return t('diskModel.diskText.stoppingTitle'); },
  get stoppingBody() { return t('diskModel.diskText.stoppingBody'); },
  get failedTitle() { return t('diskModel.diskText.failedTitle'); },
  get failedFallback() { return t('diskModel.diskText.failedFallback'); },
  get emptyTitle() { return t('diskModel.diskText.emptyTitle'); },
  get emptyBody() { return t('diskModel.diskText.emptyBody'); },
  get unpublishedTitle() { return t('diskModel.diskText.unpublishedTitle'); },
  get unpublishedBody() { return t('diskModel.diskText.unpublishedBody'); },
  get errorTitle() { return t('diskModel.diskText.errorTitle'); },
  get loading() { return t('diskModel.diskText.loading'); },
  partial: (n: number) => (n > 0 ? t('diskModel.diskText.partial.scanPartiallyCompleteN', { n }) : t('diskModel.diskText.partial.scanPartiallyCompleteSome')),
  get pending() { return t('diskModel.diskText.pending'); },
  get cancelled() { return t('diskModel.diskText.cancelled'); },
  get interrupted() { return t('diskModel.diskText.interrupted'); },
  get goScope() { return t('diskModel.diskText.goScope'); },
  get goOverview() { return t('diskModel.diskText.goOverview'); },
  get goReason() { return t('diskModel.diskText.goReason'); },
  get others() { return t('diskModel.diskText.others'); },
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
    unknownNote: unknown > 0 ? t('diskModel.unknown', { unknown }) : null,
    basis: basisLabel(e),
    boundaryNote: e.boundary ? t('diskModel.linkCloudBoundaryScan') : null,
    weakIdentity: e.identity_quality === 'weak',
  };
}

// ---------------------------------------------------------------------------------------------
// R3-03 (WFA-63): what is never offered for a selection. The bin flow refuses every link, junction, mount point and
// other scan boundary (none is moved or followed), and a link into a protected system location is refused as
// protected: the host names where it leads (`protected_link`). The page says why instead of offering "选择" and
// refusing only in the check dialog.

/** A resolved Windows path as people write it: without the `\\?\` (or `\\?\UNC\`) prefix of the host's resolution. */
export function plainPath(p: string): string {
  if (p.startsWith('\\\\?\\UNC\\')) return `\\\\${p.slice(8)}`;
  return p.startsWith('\\\\?\\') ? p.slice(4) : p;
}

/** Why this entry cannot be selected for the bin, or `null` when it can be offered. */
export function selectionBlock(e: Pick<Entry, 'kind' | 'boundary' | 'protected_link'>): string | null {
  if (e.protected_link) return t('trash.refusal.linkProtected', { target: plainPath(e.protected_link) });
  const link = e.kind === 'symlink' || e.kind === 'reparse' || e.boundary === 'symlink' || e.boundary === 'junction' || e.boundary === 'mount_point';
  if (link) return refusalText({ reason: 'link_or_reparse' });
  if (e.boundary || (e.kind !== 'file' && e.kind !== 'directory')) return t('diskModel.notMovableBoundary');
  return null;
}

// ---------------------------------------------------------------------------------------------
// NEW-06 (WFA-44): the detail pane's "protect" action. Which of the person's own protection entries already covers an
// entry (an exact entry on it, or a folder entry above it), so the pane says "protected" instead of offering it again.

export function protectedBy(path: string, items: Pick<ProtectionItem, 'protection_id' | 'layer' | 'match' | 'display'>[], caseInsensitive: boolean): Pick<ProtectionItem, 'protection_id' | 'display'> | null {
  const norm = (p: string) => {
    const trimmed = p.length > 1 ? p.replace(/[\\/]+$/, '') : p;
    return caseInsensitive ? trimmed.toLowerCase() : trimmed;
  };
  const target = norm(path);
  for (const i of items) {
    if (i.layer !== 'user' || !i.display) continue;
    const base = norm(i.display);
    if (i.match === 'exact' && target === base) return i;
    if (i.match === 'descendants' && (target === base || target.startsWith(`${base}\\`) || target.startsWith(`${base}/`))) return i;
  }
  return null;
}
