import { describe, expect, it } from 'vitest';
import { RequestGate } from './gate';

const later = <T>(v: T, ms: number) => new Promise<T>((r) => setTimeout(() => r(v), ms));
const failLater = (msg: string, ms: number) => new Promise<never>((_, rej) => setTimeout(() => rej(new Error(msg)), ms));

describe('RequestGate', () => {
  it('drops a slow response once a newer request started', async () => {
    const g = new RequestGate();
    const slow = g.run(g.next(), later('old', 30));
    const fast = g.run(g.next(), later('new', 5));
    expect(await fast).toEqual({ stale: false, value: 'new' });
    expect(await slow).toEqual({ stale: true });
  });

  it('rethrows errors of the current request and swallows stale ones', async () => {
    const g = new RequestGate();
    await expect(g.run(g.next(), failLater('boom', 1))).rejects.toThrow('boom');
    const p = g.run(g.next(), failLater('late', 5));
    g.next();
    expect(await p).toEqual({ stale: true });
  });

  it('follow-up pages share the current ticket', async () => {
    const g = new RequestGate();
    const t = g.next();
    expect(g.current()).toBe(t);
    expect(await g.run(g.current(), later(1, 1))).toEqual({ stale: false, value: 1 });
  });
});
