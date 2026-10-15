import { describe, expect, it } from 'vitest';
import { RequestGate, SubjectGate } from './gate';

const later = <T>(v: T, ms: number) => new Promise<T>((r) => setTimeout(() => r(v), ms));

/** A minimal view: shows one subject and the last result applied for it. */
class LeftoversView {
  gate = new SubjectGate<string>();
  shown: string | null = null;
  async select(app: string, ms: number): Promise<void> {
    const r = await this.gate.load(app, later(`leftovers of ${app} (${ms} ms)`, ms));
    if (!r.stale) this.shown = r.value;
  }
}

describe('GUI-14 result generations', () => {
  it('a slow scan for the previous app never replaces the current view', async () => {
    const v = new LeftoversView();
    const slowA = v.select('AppA', 40);
    const fastB = v.select('AppB', 5);
    await Promise.all([slowA, fastB]);
    expect(v.shown).toBe("leftovers of AppB (5 ms)");
  });

  it('switching back to an earlier subject still drops its older in-flight result', async () => {
    const v = new LeftoversView();
    const first = v.select('AppA', 40); // old request for A
    await v.select('AppB', 1);
    const again = v.select('AppA', 5); // new request for A
    await Promise.all([first, again]);
    expect(v.shown).toBe("leftovers of AppA (5 ms)");
    // The first (older) A request resolved last but was dropped: only one apply happened after B.
  });

  it('invalidate drops every pending result (view closed)', async () => {
    const g = new RequestGate();
    const p = g.latest(later('x', 10));
    g.invalidate();
    expect(await p).toEqual({ stale: true });
  });

  it('a refresh of the same subject keeps only the newest response', async () => {
    const g = new SubjectGate<string>();
    const old = g.load('status', later(1, 30));
    const fresh = g.load('status', later(2, 5));
    expect(await fresh).toEqual({ stale: false, value: 2 });
    expect(await old).toEqual({ stale: true });
  });
});
