import { afterEach, describe, expect, it } from 'vitest';
import { crumbTrail, DISK_TEXT, diskView, entryDetail, entryForTile, neighborTile, tileForEntry, type TileRef } from './diskModel';
import { setPlatform } from './platform';
import type { Entry, ScanStatus, Summary } from './types';

afterEach(() => setPlatform('windows'));

const GiB = 1024n ** 3n;
const summary = (over: Partial<Summary> = {}): Summary => ({
  logical_known_bytes: String(66n * GiB),
  logical_unknown_count: 0,
  allocated_exact_bytes: String(64n * GiB),
  allocated_known_bytes: String(64n * GiB),
  allocated_unknown_count: 0,
  covered_entries: 1487,
  overlapping_selected: 0,
  unread_ranges: 0,
  ...over,
});
const status = (over: Partial<ScanStatus> = {}): ScanStatus => ({
  scan_id: 's1',
  scope_id: 'scope',
  state: 'completed',
  revision: 1,
  published: true,
  completeness: 'verified_scope',
  cancel_requested: false,
  host_kind: 'desktop',
  host_pid: 1,
  hosted_here: false,
  heartbeat_at: null,
  entries_seen: 1487,
  skipped_ranges: 0,
  started_at: '2026-10-22T04:41:00Z',
  finished_at: '2026-10-22T04:41:41Z',
  root_display: 'D:\\Work',
  root_entry_id: '1',
  summary: summary(),
  backend: 'win.directory',
  backend_reason: null,
  elevated_read: [],
  issues: [],
  issues_total: 0,
  ...over,
});
const view = (s: ScanStatus | null, scanId: string | null = 's1') => diskView({ scanId, status: s, statusError: null });

describe('P03 disk state matrix', () => {
  it('no scan: an empty state that leads to the scope page', () => {
    const v = view(null, null);
    expect(v).toMatchObject({ layout: 'none', banner: null, action: 'scope' });
  });

  it('a status for another scan, or none yet, is loading — never the old scan’s chart', () => {
    expect(view(null).layout).toBe('loading');
    expect(view(status({ scan_id: 'other' })).layout).toBe('loading');
  });

  it('status that cannot be read: a failed banner in Chinese, next step is the scope page', () => {
    const v = diskView({ scanId: 's1', status: null, statusError: { code: 'internal', reason: 'store_error', message: 'disk I/O error' } });
    expect(v.layout).toBe('error');
    expect(v.banner?.kind).toBe('failed');
    expect(v.banner?.sentence).not.toMatch(/disk I\/O/);
  });

  it('not yet published (running, stopping): a running banner, no chart, the overview shows progress', () => {
    for (const s of [status({ state: 'running', published: false, summary: null }), status({ state: 'created', published: false })]) {
      const v = view(s);
      expect(v.layout).toBe('running');
      expect(v.banner).toMatchObject({ kind: 'running', title: DISK_TEXT.runningTitle });
      expect(v.action).toBe('overview');
    }
    expect(view(status({ state: 'stopping', published: false })).banner?.title).toBe(DISK_TEXT.stoppingTitle);
  });

  it('failed: the reason from the code table; a root refusal leads to choosing another scope', () => {
    const v = view(status({ state: 'failed', entries_seen: 0, summary: null, root_entry_id: null, issues: [{ code: 'not_a_directory', scope_display: 'C:\\a.txt', message: 'root is not a directory' }] }));
    expect(v.layout).toBe('failed');
    expect(v.banner?.kind).toBe('failed');
    expect(v.banner?.sentence).toContain('不是文件夹');
    expect(v.action).toBe('scope');
    const unknown = view(status({ state: 'failed', entries_seen: 0, summary: null, root_entry_id: null, issues: [{ code: 'index_quota_exhausted', scope_display: 'D:\\Work', message: 'index quota exhausted' }] }));
    expect(unknown.banner?.sentence).toBe('未识别的问题（代码 index_quota_exhausted）');
    expect(unknown.action).toBe('overview');
  });

  it('published without rows: says so, never an empty chart', () => {
    expect(view(status({ root_entry_id: null })).layout).toBe('empty');
    expect(view(status({ state: 'cancelled', published: false, summary: null })).layout).toBe('empty');
  });

  it('completed: the chart with no note', () => {
    expect(view(status())).toMatchObject({ layout: 'ready', note: null, action: null });
  });

  it('partial with gaps: the chart plus one line that names the unread places and leads to the reasons', () => {
    const v = view(status({ state: 'partial', skipped_ranges: 1, summary: summary({ unread_ranges: 1, logical_unknown_count: 2 }), issues: [{ code: 'permission_denied', scope_display: 'D:\\Work\\Locked', message: 'Access is denied.' }] }));
    expect(v.layout).toBe('ready');
    expect(v.note).toEqual({ kind: 'partial', text: DISK_TEXT.partial(1) });
    expect(v.note?.text).toContain('大小记为未知');
    expect(v.action).toBe('overview');
  });

  it('partial while a macOS privacy prompt waits: the waiting shape', () => {
    setPlatform('macos');
    const v = view(status({ state: 'partial', summary: summary({ unread_ranges: 1 }), issues: [{ code: 'permission_required', scope_display: '/Users/lin/Documents', message: 'x', permission_cause: 'privacy_prompt_pending' }] }));
    expect(v.note).toEqual({ kind: 'pending-permission', text: DISK_TEXT.pending });
  });

  it('cancelled / interrupted with kept rows: browsable, with a "only what was kept" line', () => {
    expect(view(status({ state: 'cancelled' })).note).toEqual({ kind: 'cancelled', text: DISK_TEXT.cancelled });
    expect(view(status({ state: 'interrupted' })).note).toEqual({ kind: 'cancelled', text: DISK_TEXT.interrupted });
  });
});

describe('treemap ↔ table selection sync', () => {
  const tiles: TileRef[] = [
    { id: 'a', x: 0, y: 0, w: 60, h: 100 },
    { id: 'b', x: 60, y: 0, w: 40, h: 50 },
    { id: 'c', x: 60, y: 50, w: 20, h: 50 },
    { id: '__merged__', x: 80, y: 50, w: 20, h: 50, members: ['m1', 'm2', 'm3'] },
  ];

  it('the table row lights its own tile, or the merged tile it was folded into; undrawn rows light nothing', () => {
    expect(tileForEntry(tiles, 'b')).toBe('b');
    expect(tileForEntry(tiles, 'm2')).toBe('__merged__');
    expect(tileForEntry(tiles, 'unknown-size')).toBeNull();
    expect(tileForEntry(tiles, null)).toBeNull();
  });

  it('a tile locates the same entry; the merged tile its largest member; the rest tile nothing', () => {
    expect(entryForTile(tiles[1], '__rest__')).toEqual({ entryId: 'b', folded: false });
    expect(entryForTile(tiles[3], '__rest__')).toEqual({ entryId: 'm1', folded: true });
    expect(entryForTile({ id: '__rest__' }, '__rest__')).toEqual({ entryId: null, folded: true });
  });

  it('round trip: locating a tile’s entry lights that same tile', () => {
    for (const t of tiles) {
      const e = entryForTile(t, '__rest__').entryId!;
      expect(tileForEntry(tiles, e)).toBe(t.id);
    }
  });

  it('arrow keys move to the adjacent tile in that direction and stop at the edge', () => {
    expect(neighborTile(tiles, 'a', 'right')).toBe('b');
    expect(neighborTile(tiles, 'b', 'down')).toBe('c');
    expect(neighborTile(tiles, 'c', 'right')).toBe('__merged__');
    expect(neighborTile(tiles, 'c', 'left')).toBe('a');
    expect(neighborTile(tiles, '__merged__', 'up')).toBe('b');
    expect(neighborTile(tiles, 'a', 'left')).toBe('a');
    expect(neighborTile(tiles, 'b', 'up')).toBe('b');
  });

  it('Home / End follow the size order; no current tile starts at the largest', () => {
    expect(neighborTile(tiles, 'c', 'first')).toBe('a');
    expect(neighborTile(tiles, 'a', 'last')).toBe('__merged__');
    expect(neighborTile(tiles, null, 'right')).toBe('a');
    expect(neighborTile([], null, 'down')).toBeNull();
  });
});

describe('breadcrumb and detail pane', () => {
  it('short trails show every level; long ones keep the root and the last three', () => {
    expect(crumbTrail(['D:\\Work', 'Projects', 'app'])).toHaveLength(3);
    const t = crumbTrail(['D:\\Work', 'a', 'b', 'c', 'd', 'e', 'f']);
    expect(t.map((c) => (c.kind === 'crumb' ? c.name : '…'))).toEqual(['D:\\Work', '…', 'd', 'e', 'f']);
    expect(t[1]).toEqual({ kind: 'more', depths: [1, 2, 3] });
  });

  const entry = (over: Partial<Entry> = {}): Entry => ({
    entry_id: '7',
    parent_entry_id: '1',
    name: 'Locked',
    path_display: 'D:\\Work\\Locked',
    kind: 'directory',
    logical_bytes: null,
    allocated_bytes: null,
    size_basis: 'unknown',
    unknown_descendants: null,
    boundary: null,
    identity_quality: 'strong',
    link_count: null,
    ...over,
  });

  it('unknown sizes read 未知 (never 0); unknown children make the size a lower bound', () => {
    const d = entryDetail(entry());
    expect(d.size).toBe('未知');
    expect(d.allocated).toBe('未知');
    const partial = entryDetail(entry({ logical_bytes: String(2n * GiB), allocated_bytes: String(2n * GiB), size_basis: 'subtree_known', unknown_descendants: 3 }));
    expect(partial.size).toBe('至少 2.0 GiB');
    expect(partial.unknownNote).toContain('3 项大小未知');
  });

  it('a boundary explains why it was not entered', () => {
    expect(entryDetail(entry({ boundary: 'cloud_provider', logical_bytes: '0' })).boundaryNote).toContain('没有进入');
    expect(entryDetail(entry()).boundaryNote).toBeNull();
  });
});
