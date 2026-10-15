import { describe, expect, it } from 'vitest';
import { EXTENSIONS, GATED_PAGES, TOOLS, gatedOff } from '../lib/nav';
import type { Page } from '../lib/store';
import type { GateRow } from '../lib/types';
import { offDoes, titleOf } from './gateOff';
import capabilityRs from '../../../../crates/cm-core/src/capability.rs?raw';

// The host's gate table (crates/cm-core/src/capability.rs) is the single source; the pages must name its rows.
const HOST_GATES = new Map(
  [...capabilityRs.matchAll(/gated\("([a-z_.]+)", "([A-Z0-9]+)"\)/g)].map((m) => [m[1], m[2]]),
);
const rows = (enabled: boolean): GateRow[] => [...HOST_GATES].map(([capability, gate]) => ({ capability, gate, gate_passed: false, enabled }));
const ALL_PAGES = [...new Set<Page>([...TOOLS.flatMap((t) => t.sections.map((s) => s.page)), ...EXTENSIONS.map((e) => e.page)])];

describe('D15 gated sections', () => {
  it('every gated page names a capability and gate from the host table', () => {
    expect(HOST_GATES.size).toBe(14);
    for (const [page, g] of Object.entries(GATED_PAGES)) {
      expect(HOST_GATES.get(g!.capability), page).toBe(g!.gate);
    }
  });

  it('off by default: every gated page is off, and core pages never are', () => {
    for (const page of ALL_PAGES) {
      const gated = page in GATED_PAGES;
      expect(gatedOff(page, rows(false)) !== null, page).toBe(gated);
      // Without the host's rows (capabilities unavailable) a gated page is off as well.
      expect(gatedOff(page, null) !== null, page).toBe(gated);
    }
    for (const core of ['scope', 'overview', 'disk', 'cleanup', 'apps', 'activity', 'agent', 'settings'] as Page[]) expect(gatedOff(core, rows(false))).toBeNull();
  });

  it('turned on, every section opens its tool; a gate that passed is on without the switch', () => {
    for (const page of ALL_PAGES) expect(gatedOff(page, rows(true)), page).toBeNull();
    const passed = rows(false).map((r) => (r.capability === 'process.end' ? { ...r, gate_passed: true, enabled: true } : r));
    expect(gatedOff('processes', passed)).toBeNull();
    expect(gatedOff('status', passed)).toEqual({ capability: 'status.metrics', gate: 'G3' });
  });

  it('features keep their place: gating changes no tool, section or shortcut', () => {
    expect(TOOLS.map((t) => t.sections.map((s) => s.page))).toEqual([
      ['cleanup', 'purge', 'installer', 'recyclebin'],
      ['apps', 'uninstall', 'updates', 'startup'],
      ['maintenance', 'diagnosis'],
      ['overview', 'disk', 'insights', 'scope'],
      ['status', 'processes', 'resident'],
    ]);
  });

  it('each off page says what the tool does and is titled like its section', () => {
    for (const page of Object.keys(GATED_PAGES) as Page[]) {
      expect(offDoes(page).length, page).toBeGreaterThan(0);
      expect(titleOf(page), page).not.toBe(page);
    }
    expect(titleOf('purge')).toBe('构建产物');
    expect(titleOf('diag')).toBe('AI 诊断');
    expect(offDoes('resident').join('')).toContain('登录启动项');
  });
});
