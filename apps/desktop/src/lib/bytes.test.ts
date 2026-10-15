import { describe, expect, it } from 'vitest';
import { formatBytes, formatExactBytes, formatShare, groupDigits, parseBytes, ratio } from './bytes';

describe('parseBytes', () => {
  it('keeps u64 values exact as BigInt (beyond 2^53)', () => {
    expect(parseBytes('9007199254740993')).toBe(9007199254740993n);
    expect(parseBytes('18446744073709551615')).toBe(18446744073709551615n);
    expect(parseBytes('0')).toBe(0n);
  });

  it('treats null as unknown, never zero', () => {
    expect(parseBytes(null)).toBeNull();
    expect(formatBytes(null)).toBe('未知');
    expect(formatExactBytes(null)).toBe('未知');
  });

  it('rejects numbers, signs, leading zeros and overflow', () => {
    for (const bad of ['', '-1', '+1', '01', '1.0', '1e3', ' 1', '18446744073709551616']) {
      expect(() => parseBytes(bad), bad).toThrow();
    }
    expect(() => parseBytes(5 as unknown as string)).toThrow();
  });
});

describe('formatBytes', () => {
  it('uses binary units with one decimal', () => {
    expect(formatBytes('0')).toBe('0 B');
    expect(formatBytes('1023')).toBe('1023 B');
    expect(formatBytes('1024')).toBe('1.0 KiB');
    expect(formatBytes('1536')).toBe('1.5 KiB');
    expect(formatBytes(1610612736n)).toBe('1.5 GiB');
    expect(formatBytes('18446744073709551615')).toBe('16.0 EiB');
  });

  it('rounds half up in integer arithmetic and carries into the next unit', () => {
    expect(formatBytes(1024n * 1024n - 1n)).toBe('1.0 MiB');
    expect(formatBytes(1075n)).toBe('1.0 KiB'); // 1.0498 KiB
    expect(formatBytes(1076n)).toBe('1.1 KiB'); // 1.0508 KiB
  });

  it('never loses precision that a JS number would', () => {
    expect(formatExactBytes('9007199254740993')).toBe('9,007,199,254,740,993 字节');
    expect(Number('9007199254740993').toString()).not.toBe('9007199254740993');
  });
});

describe('shares and grouping', () => {
  it('computes ratios from BigInt without overflow', () => {
    expect(ratio(1n, 4n)).toBeCloseTo(0.25, 9);
    expect(ratio(18446744073709551615n, 18446744073709551615n)).toBe(1);
    expect(ratio(0n, 10n)).toBe(0);
    expect(ratio(5n, 0n)).toBe(0);
  });

  it('formats shares with explicit tiny and unknown cases', () => {
    expect(formatShare(1n, 3n)).toBe('33.3%');
    expect(formatShare(1n, 100000n)).toBe('<0.1%');
    expect(formatShare(0n, 5n)).toBe('0%');
    expect(formatShare(null, 5n)).toBe('—');
  });

  it('groups digits', () => {
    expect(groupDigits(1234567n)).toBe('1,234,567');
    expect(groupDigits(12)).toBe('12');
  });
});
