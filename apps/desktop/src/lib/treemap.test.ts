import { describe, expect, it } from 'vitest';
import { layoutTreemap, MERGED_ID, type Tile } from './treemap';

const rect = { x: 0, y: 0, w: 800, h: 500 };
const full = rect.w * rect.h;
const area = (t: Tile) => t.w * t.h;

function inside(t: Tile): boolean {
  const eps = 1e-6;
  return t.x >= -eps && t.y >= -eps && t.x + t.w <= rect.w + eps && t.y + t.h <= rect.h + eps;
}

describe('layoutTreemap', () => {
  it('gives each known item an area proportional to its bytes', () => {
    const { tiles, areaTotal } = layoutTreemap(
      [
        { id: 'a', bytes: 600n },
        { id: 'b', bytes: 300n },
        { id: 'c', bytes: 100n },
      ],
      rect,
    );
    expect(areaTotal).toBe(1000n);
    expect(tiles).toHaveLength(3);
    const byId = Object.fromEntries(tiles.map((t) => [t.id, t]));
    expect(area(byId.a) / full).toBeCloseTo(0.6, 6);
    expect(area(byId.b) / full).toBeCloseTo(0.3, 6);
    expect(area(byId.c) / full).toBeCloseTo(0.1, 6);
    expect(tiles.reduce((s, t) => s + area(t), 0)).toBeCloseTo(full, 3);
    tiles.forEach((t) => expect(inside(t), t.id).toBe(true));
  });

  it('keeps tiles from overlapping', () => {
    const input = Array.from({ length: 30 }, (_, i) => ({ id: `e${i}`, bytes: BigInt((i + 1) * 997) }));
    const { tiles } = layoutTreemap(input, rect);
    for (let i = 0; i < tiles.length; i++) {
      for (let j = i + 1; j < tiles.length; j++) {
        const a = tiles[i];
        const b = tiles[j];
        const ox = Math.min(a.x + a.w, b.x + b.w) - Math.max(a.x, b.x);
        const oy = Math.min(a.y + a.h, b.y + b.h) - Math.max(a.y, b.y);
        expect(ox <= 1e-6 || oy <= 1e-6, `${a.id}/${b.id}`).toBe(true);
      }
    }
  });

  it('excludes unknown sizes (no invented area) and reports them separately', () => {
    const { tiles, unknownIds, areaTotal } = layoutTreemap(
      [
        { id: 'known', bytes: 10n },
        { id: 'unknown', bytes: null },
      ],
      rect,
    );
    expect(unknownIds).toEqual(['unknown']);
    expect(tiles.map((t) => t.id)).toEqual(['known']);
    expect(areaTotal).toBe(10n);
    expect(area(tiles[0])).toBeCloseTo(full, 3);
  });

  it('draws nothing when every size is unknown or zero', () => {
    const r = layoutTreemap(
      [
        { id: 'u', bytes: null },
        { id: 'z', bytes: 0n },
      ],
      rect,
    );
    expect(r.tiles).toEqual([]);
    expect(r.unknownIds).toEqual(['u']);
    expect(r.zeroIds).toEqual(['z']);
  });

  it('keeps proportions exact for sizes beyond 2^53', () => {
    const big = 1n << 62n;
    const { tiles } = layoutTreemap(
      [
        { id: 'x', bytes: big * 3n },
        { id: 'y', bytes: big },
      ],
      rect,
    );
    expect(area(tiles.find((t) => t.id === 'x')!) / full).toBeCloseTo(0.75, 6);
  });

  it('folds tiny items into one merged tile that keeps member ids', () => {
    const input = [{ id: 'big', bytes: 1_000_000n }, ...Array.from({ length: 50 }, (_, i) => ({ id: `t${i}`, bytes: 1n }))];
    const { tiles, mergedIds } = layoutTreemap(input, rect, { minTileArea: 100 });
    const merged = tiles.find((t) => t.id === MERGED_ID)!;
    expect(merged.members).toHaveLength(50);
    expect(mergedIds).toHaveLength(50);
    expect(merged.bytes).toBe(50n);
    expect(tiles).toHaveLength(2);
  });

  it('adds the remainder of a layer as its own tile', () => {
    const { tiles, areaTotal } = layoutTreemap([{ id: 'a', bytes: 300n }], rect, { extra: { id: 'rest', bytes: 100n } });
    expect(areaTotal).toBe(400n);
    expect(area(tiles.find((t) => t.id === 'rest')!) / full).toBeCloseTo(0.25, 6);
  });

  it('is deterministic for equal sizes (id tie-break)', () => {
    const input = [
      { id: 'b', bytes: 5n },
      { id: 'a', bytes: 5n },
    ];
    const ids = (xs: typeof input) => layoutTreemap(xs, rect).tiles.map((t) => `${t.id}@${t.x},${t.y}`);
    expect(ids(input)).toEqual(ids([...input].reverse()));
  });
});
