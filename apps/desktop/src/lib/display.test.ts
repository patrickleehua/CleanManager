import { describe, expect, it } from 'vitest';
import { middleEllipsis, sizeLabel, sizeTitle, textWidth } from './display';

describe('path display: middle ellipsis', () => {
  const long = 'C:\\Users\\Lin\\AppData\\Local\\Temp\\CleanManagerAcceptance-20261022-b4ef3aa9\\input\\a.txt';

  it('short paths are unchanged', () => {
    expect(middleEllipsis('D:\\Work', 40)).toBe('D:\\Work');
    expect(middleEllipsis('', 10)).toBe('');
  });

  it('long paths keep the start and the last name, with one ellipsis in the middle, within the budget', () => {
    const s = middleEllipsis(long, 48);
    expect(textWidth(s)).toBeLessThanOrEqual(48);
    expect(s.startsWith('C:\\Users\\')).toBe(true);
    expect(s.endsWith('\\input\\a.txt') || s.endsWith('\\a.txt')).toBe(true);
    expect(s.split('…')).toHaveLength(2);
  });

  it('CJK counts double, and a long last name is cut at its start instead of losing the drive', () => {
    const cjk = 'C:\\Users\\Lin\\Downloads\\季度报告-2026-Q3-最终版-给财务部门和法务部门审阅用的副本.pdf';
    const s = middleEllipsis(cjk, 40);
    expect(textWidth(s)).toBeLessThanOrEqual(40);
    expect(s.startsWith('C:\\')).toBe(true);
    expect(s.endsWith('.pdf')).toBe(true);
    expect(textWidth('季度')).toBe(4);
  });

  it('macOS paths keep their slashes', () => {
    const s = middleEllipsis('/Users/lin/Library/Application Support/CleanManager/store/index.sqlite', 36);
    expect(s.startsWith('/Users/')).toBe(true);
    expect(s.endsWith('/index.sqlite')).toBe(true);
    expect(textWidth(s)).toBeLessThanOrEqual(36);
  });

  it('tiny budgets still produce something with an ellipsis', () => {
    expect(middleEllipsis(long, 4)).toBe('C:\\…');
  });
});

describe('size display: unknown is 未知, never 0', () => {
  it('null, undefined and malformed values read 未知', () => {
    expect(sizeLabel(null)).toBe('未知');
    expect(sizeLabel(undefined)).toBe('未知');
    expect(sizeLabel('-1')).toBe('未知');
    expect(sizeTitle(null)).toBe('大小未知');
  });

  it('known sizes are formatted; partial knowledge reads "至少"; nothing known with unknown parts reads 未知', () => {
    expect(sizeLabel('0')).toBe('0 B');
    expect(sizeLabel(String(1024 ** 3))).toBe('1.0 GiB');
    expect(sizeLabel(String(1024 ** 3), { unknownCount: 3 })).toBe('至少 1.0 GiB');
    expect(sizeLabel(String(1024 ** 3), { atLeast: true })).toBe('至少 1.0 GiB');
    expect(sizeLabel('0', { unknownCount: 2 })).toBe('未知');
    expect(sizeTitle('71393594448')).toBe('71,393,594,448 字节');
  });
});
