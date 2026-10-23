import { describe, expect, it } from 'vitest';
import { firstNewRowId } from './activity';
import { mockBackend } from '../lib/mock';
import type { HistoryItem, PlanRow, QuarantineItem } from '../lib/types';
import auditRs from '../../../../crates/cm-store/src/audit.rs?raw';

describe('R3-05 recovery list: newest first, and "显示更多" keeps the keyboard where the list grew', () => {
  it('focus goes to the first newly loaded row of the tab, never to the top of the page', () => {
    const q = (id: string) => ({ quarantine_id: id }) as QuarantineItem;
    expect(firstNewRowId('quarantine', [q('a'), q('b'), q('c')], 2)).toBe('q-c');
    expect(firstNewRowId('operations', [{ operation_id: 'op_1' } as HistoryItem, { operation_id: 'op_2' } as HistoryItem], 1)).toBe('op-op_2');
    expect(firstNewRowId('plans', [{ plan_id: 'p_1' } as PlanRow], 0)).toBe('plan-p_1');
    expect(firstNewRowId('quarantine', [q('a')], 1), 'nothing new: focus is left alone').toBeNull();
  });

  it('the host lists the quarantine most recently quarantined first (CLI and desktop share the store query)', () => {
    const query = auditRs.slice(auditRs.indexOf('pub fn quarantine_page'), auditRs.indexOf('pub fn quarantine_for_operation'));
    expect(query).toContain('ORDER BY rowid DESC');
  });

  it('the preview backend lists the same order', async () => {
    const page = await mockBackend().listQuarantine(500, null);
    const times = page.items.map((i) => i.quarantined_at);
    expect(times.length).toBeGreaterThan(1);
    expect(times).toEqual([...times].sort().reverse());
  });
});
