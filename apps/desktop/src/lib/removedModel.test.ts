import { afterEach, describe, expect, it } from 'vitest';
import { HIDE_REMOVED_KEY, folderSizeText, loadHideRemoved, removedBadge, removedBeneath, removedBlock, saveHideRemoved, sinceScanLine, visibleRows } from './removedModel';
import { selectionBlock } from './diskModel';
import { setPlatform } from './platform';
import { setLocale } from '../i18n';
import type { Entry, RemovedFact } from './types';

afterEach(() => {
  setLocale('zh-CN');
  setPlatform('windows');
});

const fact = (over: Partial<RemovedFact> = {}): RemovedFact => ({ mode: 'trash', operation_id: 'op_1', at: '2026-10-23T08:00:00Z', recoverable: true, inherited: false, ...over });

function entry(id: string, over: Partial<Entry> = {}): Entry {
  return {
    entry_id: id, parent_entry_id: '1', name: id, path_display: `D:\\Work\\${id}`, kind: 'file', logical_bytes: '1024', allocated_bytes: '1024', size_basis: 'entity',
    unknown_descendants: null, boundary: null, identity_quality: 'strong', link_count: 1, removed: null, removed_count: 0, removed_bytes: null, removed_unknown_count: 0, ...over,
  };
}

describe('removal badges', () => {
  it('say how it left and whether it can be put back, in both languages and on both platforms', () => {
    expect(removedBadge(fact())).toEqual({ text: '已移到回收站 · 可恢复', tone: 'recoverable', note: null });
    expect(removedBadge(fact({ mode: 'quarantine' })).text).toBe('已移入隔离区 · 可恢复');
    expect(removedBadge(fact({ mode: 'quarantine', recoverable: false }))).toMatchObject({ text: '已移入隔离区 · 已永久清空', tone: 'gone' });
    expect(removedBadge(fact({ mode: 'permanent', recoverable: false }))).toMatchObject({ text: '已永久删除', tone: 'gone' });
    expect(removedBadge(fact({ inherited: true })).note).toBe('随上级文件夹一起移走');
    setPlatform('macos');
    expect(removedBadge(fact()).text).toBe('已移到废纸篓 · 可恢复');
    setLocale('en');
    expect(removedBadge(fact()).text).toBe('Moved to the Trash · recoverable');
    setPlatform('windows');
    expect(removedBadge(fact()).text).toBe('Moved to the Recycle Bin · recoverable');
    expect(removedBadge(fact({ mode: 'permanent' })).text).toBe('Permanently deleted');
  });

  it('a removed entry is never offered for a selection, and says why', () => {
    expect(removedBlock(entry('a', { removed: fact() }))).toContain('不能再选择');
    expect(removedBlock(entry('a'))).toBeNull();
    // A folder with removed objects inside: the host refuses it too (its snapshot no longer describes it).
    expect(removedBlock(entry('d', { kind: 'directory', removed_count: 2 }))).toContain('不能整体选择');
    // The disk page's one selection check refuses it too (before the link checks).
    expect(selectionBlock(entry('a', { removed: fact({ inherited: true }) }))).toContain('不能再选择');
    expect(selectionBlock(entry('a'))).toBeNull();
  });
});

describe('hide removed items', () => {
  const rows = [entry('a'), entry('b', { removed: fact() }), entry('c', { removed: fact({ mode: 'permanent' }) }), entry('d')];

  it('shows every row marked by default and leaves removed rows out when on', () => {
    expect(visibleRows(rows, false)).toEqual({ rows, hidden: 0 });
    const on = visibleRows(rows, true);
    expect(on.rows.map((r) => r.entry_id)).toEqual(['a', 'd']);
    expect(on.hidden).toBe(2);
  });

  it('remembers the choice per viewer and survives storage that throws', () => {
    const store = new Map<string, string>();
    const s = { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => void store.set(k, v), removeItem: (k: string) => void store.delete(k) };
    expect(loadHideRemoved(s)).toBe(false);
    saveHideRemoved(true, s);
    expect(store.get(HIDE_REMOVED_KEY)).toBe('1');
    expect(loadHideRemoved(s)).toBe(true);
    saveHideRemoved(false, s);
    expect(loadHideRemoved(s)).toBe(false);
    const broken = { getItem: () => { throw new Error('denied'); }, setItem: () => { throw new Error('denied'); }, removeItem: () => { throw new Error('denied'); } };
    expect(loadHideRemoved(broken)).toBe(false);
    expect(() => saveHideRemoved(true, broken)).not.toThrow();
    expect(loadHideRemoved(null)).toBe(false);
  });
});

describe('folder sizes with what left them', () => {
  it('reads "扫描时 X · 已移走 Y" from the stored sizes', () => {
    const dir = entry('Videos', { kind: 'directory', size_basis: 'subtree_known', logical_bytes: String(10n * 1024n ** 3n), unknown_descendants: 0, removed_count: 2, removed_bytes: String(4n * 1024n ** 3n) });
    expect(folderSizeText(dir)).toBe('扫描时 10.0 GiB · 已移走 4.0 GiB');
    setLocale('en');
    expect(folderSizeText(dir)).toBe('At scan 10.0 GiB · removed 4.0 GiB');
  });

  it('keeps unknown sizes unknown, never 0', () => {
    const base = { kind: 'directory' as const, size_basis: 'subtree_known' as const, logical_bytes: '5000', unknown_descendants: 0 };
    expect(removedBeneath(entry('x', { ...base, removed_count: 1, removed_bytes: null, removed_unknown_count: 1 }))).toBe('未知');
    expect(removedBeneath(entry('x', { ...base, removed_count: 3, removed_bytes: '2048', removed_unknown_count: 1 }))).toBe('至少 2.0 KiB');
    expect(removedBeneath(entry('x', { ...base, removed_count: 2, removed_bytes: '0', removed_unknown_count: 2 }))).toBe('未知');
    expect(removedBeneath(entry('x', { ...base, removed_count: 1, removed_bytes: '0', removed_unknown_count: 0 }))).toBe('0 B');
    expect(removedBeneath(entry('x', base))).toBeNull();
    expect(folderSizeText(entry('x', base))).toBe('4.9 KiB');
    expect(folderSizeText(entry('x', { ...base, logical_bytes: null, removed_count: 1, removed_bytes: null, removed_unknown_count: 1 }))).toBe('扫描时 未知 · 已移走 未知');
  });

  it('the line under the header counts the scan total; nothing removed, no line', () => {
    expect(sinceScanLine({ count: 3, known_bytes: '3145728', unknown_count: 0, latest_at: null, signature: 's' })).toBe('这次扫描之后已移走 3 项（3.0 MiB）；重新扫描可得到最新结果');
    expect(sinceScanLine({ count: 1, known_bytes: null, unknown_count: 1, latest_at: null, signature: 's' })).toBe('这次扫描之后已移走 1 项（未知）；重新扫描可得到最新结果');
    expect(sinceScanLine({ count: 0, known_bytes: null, unknown_count: 0, latest_at: null, signature: '0' })).toBeNull();
    expect(sinceScanLine(null)).toBeNull();
    setLocale('en');
    expect(sinceScanLine({ count: 1, known_bytes: '1024', unknown_count: 0, latest_at: null, signature: 's' })).toBe('1 item was removed after this scan (1.0 KiB). Scan again for the current state');
  });
});
