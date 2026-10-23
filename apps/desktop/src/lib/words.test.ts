import { describe, expect, it } from 'vitest';

import { setLocale } from '../i18n';
import { HISTORY_ACTIONS, type Availability } from './types';
import { AVAILABILITY_LABEL, CODE_EXPLAIN, MODE_EXPLAIN, digestGroups, explainCode, formatCountdown, historyRecoveryLabel, normalizeCode, progressSegments } from './words';

describe('progress over a fixed plan total', () => {
  it('puts every planned action in exactly one segment and keeps pending visible', () => {
    const segs = progressSegments(10, { succeeded: 3, skipped: 1, failed: 0, not_started: 0, unknown: 1, in_flight: 1 });
    expect(segs).toEqual([
      { key: 'succeeded', count: 3 },
      { key: 'skipped', count: 1 },
      { key: 'unknown', count: 1 },
      { key: 'in_flight', count: 1 },
      { key: 'pending', count: 4 },
    ]);
    expect(segs.reduce((a, s) => a + s.count, 0)).toBe(10);
  });

  it('never reports negative pending when the journal is ahead of a stale total', () => {
    const segs = progressSegments(1, { succeeded: 2, skipped: 0, failed: 0, not_started: 0, unknown: 0, in_flight: 0 });
    expect(segs).toEqual([{ key: 'succeeded', count: 2 }]);
  });
});

describe('wording', () => {
  it('states that a same-volume quarantine does not free space', () => {
    expect(MODE_EXPLAIN.quarantine.space).toContain('不立即释放空间');
    expect(MODE_EXPLAIN.delete.recovery).toContain('无法撤销');
  });

  it('explains known codes and falls back to the host message, never to nothing', () => {
    expect(explainCode('outcome_unconfirmed', 'x')).toBe(CODE_EXPLAIN.outcome_unconfirmed);
    expect(explainCode('some_new_code', 'host says why')).toBe('host says why');
    expect(explainCode(null, null)).toBe('');
  });

  it('groups digests and normalizes typed codes', () => {
    expect(digestGroups('a'.repeat(64))).toHaveLength(8);
    expect(normalizeCode(' 3FA9-C2E1 ')).toBe('3fa9c2e1');
    expect(normalizeCode('3fa9c2e1ff')).toBe('3fa9c2e1');
    expect(formatCountdown(65)).toBe('1 分 05 秒');
    expect(formatCountdown(-3)).toBe('0 秒');
  });
});

describe('R3-02: the Activity recovery note follows the host facts of each kind', () => {
  const label = (action: string | null, availability: Availability, recycle_bin_items = false) => historyRecoveryLabel({ action, availability, recycle_bin_items });

  it('names the quarantine only for a quarantine, and backups and the bin for uninstall leftovers', () => {
    expect(label('quarantine', 'in_app_restore')).toBe('可在隔离区恢复');
    expect(label('uninstall_leftovers', 'in_app_restore', true)).toBe('注册表项与启动值可从备份还原；文件在回收站中');
    expect(label('uninstall_leftovers', 'in_app_restore')).toBe('注册表项与启动值可从备份还原');
    expect(label('uninstall_leftovers', 'system_recycle_bin', true)).toBe('在系统回收站中还原');
    expect(label('startup', 'in_app_restore')).toBe('可在本应用恢复原来的启动设置');
    expect(label('maintenance', 'in_app_restore')).toBe('设置更改可在本应用恢复');
    expect(label('maintenance', 'unavailable')).toBe('无法撤销');
    expect(label('uninstall', 'unavailable')).toBe('无法撤销');
    expect(label('process', 'unavailable')).toBe('无法撤销');
    expect(label('uninstall_restore', 'not_applicable')).toBe('无需恢复');
    expect(label('trash', 'system_recycle_bin', true)).toBe('在系统回收站中还原');
    expect(label(null, 'unknown')).toBe(AVAILABILITY_LABEL.unknown);
  });

  it('never says "quarantine" for any other kind, in either language', () => {
    for (const lang of ['zh-CN', 'en'] as const) {
      setLocale(lang);
      const quarantine = historyRecoveryLabel({ action: 'quarantine', availability: 'in_app_restore' });
      for (const action of HISTORY_ACTIONS.filter((a) => a !== 'quarantine')) {
        for (const availability of ['in_app_restore', 'system_recycle_bin', 'unavailable', 'not_applicable', 'unknown'] as const) {
          for (const bin of [false, true]) {
            const text = label(action, availability, bin);
            expect(text, `${lang} ${action} ${availability}`).not.toBe(quarantine);
            expect(text, `${lang} ${action} ${availability}`).not.toMatch(/words\./);
          }
        }
      }
    }
    setLocale('en');
    expect(label('uninstall_leftovers', 'in_app_restore', true)).toBe('Registry keys and startup values can be restored from their backups; files are in the Recycle Bin');
    setLocale('zh-CN');
  });
});
