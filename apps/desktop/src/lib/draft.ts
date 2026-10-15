// The P04 selection draft. It holds explicitly chosen candidates of one candidate set only:
// nothing is pre-selected, unselectable candidates can never enter it, and a new candidate set
// starts an empty draft (an old selection is never applied to a new index).

import type { Candidate, Mode } from './types';

const ALL_MODES: Mode[] = ['trash', 'quarantine', 'delete'];

export class Draft {
  private chosen = new Map<string, Candidate>();
  constructor(public readonly setId: string) {}

  get size(): number {
    return this.chosen.size;
  }

  has(id: string): boolean {
    return this.chosen.has(id);
  }

  items(): Candidate[] {
    return [...this.chosen.values()];
  }

  /** Add one candidate. Returns false (and changes nothing) when it cannot be planned. */
  add(c: Candidate): boolean {
    if (c.not_selectable !== null || c.allowed_modes.length === 0) return false;
    this.chosen.set(c.candidate_id, c);
    return true;
  }

  remove(id: string): void {
    this.chosen.delete(id);
  }

  toggle(c: Candidate): boolean {
    if (this.chosen.has(c.candidate_id)) {
      this.chosen.delete(c.candidate_id);
      return false;
    }
    return this.add(c);
  }

  clear(): void {
    this.chosen.clear();
  }

  /** Modes every selected candidate allows (a plan has exactly one mode). */
  modes(): Mode[] {
    const items = this.items();
    if (!items.length) return [];
    return ALL_MODES.filter((m) => items.every((c) => c.allowed_modes.includes(m)));
  }

  /** Known logical total; candidates in a draft never contain unknown sizes. */
  knownBytes(): bigint {
    let t = 0n;
    for (const c of this.chosen.values()) t += BigInt(c.logical_known_bytes);
    return t;
  }
}
