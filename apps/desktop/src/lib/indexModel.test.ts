import { describe, expect, it } from 'vitest';
import { INDEX_TEXT, currentIndexProblem, indexProblemOf, repairOutcome } from './indexModel';
import { setLocale } from '../i18n';
import hostDto from '../../src-tauri/src/dto.rs?raw';
import hostHost from '../../src-tauri/src/host.rs?raw';

const corrupt = { code: 'index_unavailable', reason: 'index_corrupt', message: 'scan index is damaged; run `cleanmanager doctor --repair-index`' };

describe('NEW-04: a damaged scan index is said as such, never as "no scans yet"', () => {
  it('reads the reasons the host sends for a damaged or incompatible index (dto.rs) and the startup report (host.rs)', () => {
    expect(hostDto).toContain('"index_corrupt"');
    expect(hostDto).toContain('"index_incompatible"');
    expect(hostHost).toContain('store_problem');
    expect(indexProblemOf(corrupt)).toEqual({ reason: 'index_corrupt', message: corrupt.message });
    expect(indexProblemOf({ ...corrupt, reason: 'index_incompatible' })?.reason).toBe('index_incompatible');
    // Only the protocol code: still a damaged index (what the repair handles).
    expect(indexProblemOf({ code: 'index_unavailable', reason: 'store_error', message: 'x' })?.reason).toBe('index_corrupt');
    // Other store problems are not index problems.
    expect(indexProblemOf({ code: 'invalid_argument', reason: 'no_store_dir', message: 'x' })).toBeNull();
    expect(indexProblemOf(null)).toBeNull();
  });

  it('the startup report applies until the index is rebuilt here; a failed status read says so at any time', () => {
    const caps = { store_problem: corrupt };
    expect(currentIndexProblem({ caps })?.reason).toBe('index_corrupt');
    expect(currentIndexProblem({ caps, repairedAt: 1 })).toBeNull();
    expect(currentIndexProblem({ caps: { store_problem: null }, statusError: { ...corrupt, reason: 'index_incompatible' }, repairedAt: 1 })?.reason).toBe('index_incompatible');
    expect(currentIndexProblem({ caps: null })).toBeNull();
    expect(currentIndexProblem({ caps: { store_problem: null }, statusError: { code: 'invalid_argument', reason: 'not_found', message: 'x' } })).toBeNull();
  });

  it('the words: what is wrong, what the rebuild keeps and what must be redone', () => {
    expect(INDEX_TEXT.title('index_corrupt')).toBe('扫描索引已损坏，需要重建');
    expect(INDEX_TEXT.title('index_incompatible')).toContain('不兼容');
    expect(INDEX_TEXT.sentence).not.toContain('还没有扫描');
    expect(INDEX_TEXT.kept).toContain('不会被删除');
    expect(INDEX_TEXT.kept).toContain('index.db.damaged-');
    expect(INDEX_TEXT.rescan).toContain('重新扫描');
    expect(INDEX_TEXT.untouched).toContain('隔离区');
    setLocale('en');
    expect(INDEX_TEXT.title('index_corrupt')).toBe('The scan index is damaged and needs rebuilding');
    expect(INDEX_TEXT.kept).toContain('is not deleted');
  });

  it('after the repair: the kept files by name, or "nothing changed" for a healthy index', () => {
    const done = repairOutcome({ status: 'repaired', kept_files: ['C:\\Users\\Lin\\AppData\\Local\\CleanManager\\store\\index.db.damaged-20261023T061500Z', '/x/index.db.damaged-20261023T061500Z-wal'] });
    expect(done.done).toBe(true);
    expect(done.text).toBe('扫描索引已重建。损坏的文件保留为 index.db.damaged-20261023T061500Z、index.db.damaged-20261023T061500Z-wal。请重新扫描。');
    expect(repairOutcome({ status: 'not_needed', kept_files: [] })).toEqual({ done: false, text: INDEX_TEXT.notNeeded });
  });
});
