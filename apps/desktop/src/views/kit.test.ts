import { describe, expect, it } from 'vitest';
import { atLeast, commonModes, sumSizes, TIER_LABEL, unknownNote } from './kit';
import { insightCards } from './tools';
import type { Insight } from '../lib/types';

const GiB = 1024 ** 3;

describe('GUI-09/10 shared selection wording', () => {
  it('known sizes add up; unknown ones are counted apart, never as 0', () => {
    const sum = sumSizes([{ bytes: String(GiB) }, { bytes: null }, { bytes: String(GiB), unknown: 3 }]);
    expect(sum).toEqual({ known: BigInt(2 * GiB), unknown: 4 });
    expect(atLeast(sum)).toBe('至少 2.0 GiB');
    expect(unknownNote(sum)).toBe('4 项大小未知，未计入');
  });

  it('exact only when everything is known; "unknown" when nothing is', () => {
    expect(atLeast(sumSizes([{ bytes: String(GiB) }]))).toBe('1.0 GiB');
    expect(atLeast(sumSizes([{ bytes: null }, { bytes: null }]))).toBe('大小未知');
    expect(unknownNote(sumSizes([{ bytes: '1' }]))).toBeNull();
  });

  it('a plan mode is offered only when every selected object allows it', () => {
    expect(commonModes([{ allowed_modes: ['trash', 'quarantine', 'delete'] }, { allowed_modes: ['trash', 'quarantine'] }])).toEqual(['trash', 'quarantine']);
    expect(commonModes([])).toEqual([]);
  });

  it('three tiers, three words', () => {
    expect(Object.values(TIER_LABEL)).toEqual(['低风险', '需审阅', '已阻止']);
  });
});

describe('DIN-03 insight cards', () => {
  const ins = (kind: string, bytes: string | null, observed: string | null): Insight => ({
    insight_id: kind, kind, title: kind, path: null, logical_bytes: bytes, count: null, size_basis: 'file_metadata', size_note: null, observed_at: observed, explanation: '', handling: 'read_only_guidance', guidance: [],
  });
  it('a card shows a known size, "at least" over unknown members, or unknown; and the oldest observation', () => {
    const cards = insightCards([
      ins('windows.page_file', String(GiB), '2026-10-15T10:00:00Z'),
      ins('windows.hibernation_file', null, '2026-10-15T09:00:00Z'),
      ins('windows.component_store', null, null),
    ]);
    const sys = cards.find((c) => c.title === '系统文件')!;
    expect(sys.size).toBe('至少 1.0 GiB');
    expect(sys.observed).toBe('2026-10-15T09:00:00Z');
    expect(cards.find((c) => c.title === '组件存储')!.size).toBe('大小未知');
  });
});
