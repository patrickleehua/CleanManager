import { describe, expect, it } from 'vitest';

import { Draft } from './draft';
import type { Candidate } from './types';

function cand(id: string, over: Partial<Candidate> = {}): Candidate {
  return {
    candidate_id: id,
    entry_id: '2',
    name: id,
    path_display: `C:\\x\\${id}`,
    kind: 'directory',
    rules: ['r@1.0.0'],
    category: 'app_cache',
    group: 'low',
    state: 'allowed',
    tier: 'default_selected',
    not_selectable: null,
    reasons: [],
    protection: null,
    logical_known_bytes: '100',
    unknown_size_count: 0,
    logical_exact_bytes: '100',
    allowed_modes: ['trash', 'quarantine'],
    impact: '',
    probes: [],
    row_kind: 'file',
    registry_status: 'covered',
    installed_version: null,
    rebuild_class: null,
    adapter_ref: null,
    probe_results: [],
    catalog_fresh: true,
    ...over,
  };
}

describe('selection draft', () => {
  it('starts empty and refuses candidates that cannot be planned', () => {
    const d = new Draft('cs_1');
    expect(d.size).toBe(0);
    expect(d.add(cand('blocked', { not_selectable: 'blocked', allowed_modes: [] }))).toBe(false);
    expect(d.add(cand('unknown', { not_selectable: 'unknown_size', unknown_size_count: 3, logical_exact_bytes: null }))).toBe(false);
    expect(d.size).toBe(0);
  });

  it('offers only modes every selected candidate allows and totals with BigInt', () => {
    const d = new Draft('cs_1');
    d.add(cand('a', { logical_known_bytes: '18446744073709551615' }));
    d.add(cand('b', { allowed_modes: ['quarantine', 'delete'], logical_known_bytes: '1' }));
    expect(d.modes()).toEqual(['quarantine']);
    expect(d.knownBytes()).toBe(18446744073709551616n);
    expect(d.toggle(cand('a'))).toBe(false);
    expect(d.modes()).toEqual(['quarantine', 'delete']);
    d.clear();
    expect(d.modes()).toEqual([]);
  });
});
