import { describe, expect, it } from 'vitest';
import { checkStylesheet, ratio } from '../../scripts/contrast.mjs';

describe('GUI-11 design tokens meet WCAG contrast in both themes', () => {
  it('computes known ratios', () => {
    expect(ratio('#000000', '#ffffff')).toBeCloseTo(21, 5);
    expect(ratio('#ffffff', '#ffffff')).toBeCloseTo(1, 5);
  });

  it('every text pair is at least 4.5:1 and every control boundary at least 3:1', () => {
    const rows = checkStylesheet();
    expect(rows.length).toBeGreaterThan(40);
    const failing = rows.filter((r) => !r.ok).map((r) => `${r.theme} --${r.fg}/--${r.bg} ${r.ratio.toFixed(2)}`);
    expect(failing).toEqual([]);
  });
});
