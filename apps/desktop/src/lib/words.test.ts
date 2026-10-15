import { describe, expect, it } from 'vitest';

import { CODE_EXPLAIN, MODE_EXPLAIN, digestGroups, explainCode, formatCountdown, normalizeCode, progressSegments } from './words';

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
