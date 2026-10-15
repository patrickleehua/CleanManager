import { describe, expect, it } from 'vitest';
import { mockSoftware } from '../lib/mockSoftware';
import type { Leftovers, UninstallApp } from '../lib/suiteTypes';
import type { OperationItem, PlanItem } from '../lib/types';
import {
  APP_OUTCOME,
  MAX_BATCH,
  batchActionText,
  batchSelectable,
  batchSelection,
  leftoverSections,
  leftoversReviewable,
  leftoversTitle,
  outcomeFromItems,
  rowsByApp,
} from './uninstallBatch';

const app = (id: string, state: UninstallApp['state']): UninstallApp => ({
  app_instance_id: id, name: id.toUpperCase(), version: null, publisher: null, source: 'registry_uninstall', location: null, last_used_at: null,
  protected_class: state === 'unsupported', state, adapter: null, reason: null, settings_uri: null,
});

const planRow = (action_id: string, name: string, item_kind: string, appId?: string): PlanItem => ({
  action_id, name, path_display: '', logical_bytes: '0', allocated_bytes: null, candidate_id: null, original_path_display: null, quarantine_id: null, item_kind,
  action_type: item_kind === 'app_quit' ? 'app_quit' : 'tool_adapter', detail: appId ? { app_instance_id: appId } : {},
});

const opRow = (action_id: string, status: OperationItem['status'], code: string): OperationItem => ({
  action_id, name: '', path_display: '', original_path_display: null, logical_bytes: null, status, code, message: `${code} message`, recovery: 'not_applicable',
});

describe('UNI-11 multi-select', () => {
  it('only apps with a verified route can be chosen; protected and Settings-only apps never', () => {
    expect(batchSelectable(app('a', 'available'))).toBe(true);
    expect(batchSelectable(app('agent', 'unsupported'))).toBe(false);
    expect(batchSelectable(app('tool', 'unverified'))).toBe(false);
  });
  it('the selection is read in list order and drops apps that disappeared or became locked', () => {
    const items = [app('a', 'available'), app('agent', 'unsupported'), app('b', 'available')];
    const picked = batchSelection(items, new Set(['b', 'gone', 'agent', 'a']));
    expect(picked.map((a) => a.app_instance_id)).toEqual(['a', 'b']);
  });
  it('the plan button says how many applications go into the one plan', () => {
    expect(batchActionText(1)).toBe('生成卸载计划并审阅');
    expect(batchActionText(3)).toContain('3 个应用，一份计划');
    expect(MAX_BATCH).toBe(20);
  });
});

describe('UNI-11 grouped review', () => {
  it('plan rows are grouped by application in plan order', () => {
    const apps = [
      { app_instance_id: 'app_a', name: 'A', publisher: null, version: null, source: 'registry_uninstall', adapter: 'u.exe', signer: null, action_ids: ['q0000000', 'u0000001'] },
      { app_instance_id: 'app_b', name: 'B', publisher: null, version: null, source: 'msi', adapter: 'msiexec', signer: null, action_ids: ['u0010000', 'u0010001'] },
    ];
    const rows = [planRow('q0000000', 'A', 'app_quit', 'app_a'), planRow('u0000001', 'A', 'uninstaller'), planRow('u0010000', 'B', 'app_quit', 'app_b'), planRow('u0010001', 'B', 'uninstaller', 'app_b')];
    const groups = rowsByApp(rows, apps);
    expect(groups.map((g) => [g.name, g.items.map((i) => i.action_id)])).toEqual([
      ['A', ['q0000000', 'u0000001']],
      ['B', ['u0010000', 'u0010001']],
    ]);
  });
  it('each application has its own result; leftovers only where its own uninstaller succeeded', () => {
    const items = [opRow('q0000000', 'succeeded', 'not_running'), opRow('u0000001', 'failed', 'uninstaller_failed'), opRow('u0010000', 'succeeded', 'closed'), opRow('u0010001', 'succeeded', 'leftovers_remain')];
    const a = outcomeFromItems({ app_instance_id: 'app_a', name: 'A', action_ids: ['q0000000', 'u0000001'] }, items);
    const b = outcomeFromItems({ app_instance_id: 'app_b', name: 'B', action_ids: ['u0010000', 'u0010001'] }, items);
    expect([a.status, a.code, a.leftovers_reviewable]).toEqual(['failed', 'uninstaller_failed', false]);
    expect([b.status, b.leftovers_reviewable]).toEqual(['partial', true]);
    expect(APP_OUTCOME[a.status].tone).toBe('bad');
    expect(leftoversReviewable([a, b], 'partial')).toBe(true);
    expect(leftoversReviewable([a], 'partial')).toBe(false);
    // An app that did not quit is not uninstalled; the close request explains why.
    const stuck = outcomeFromItems({ app_instance_id: 'app_c', name: 'C', action_ids: ['q0', 'u0'] }, [opRow('q0', 'failed', 'still_running'), opRow('u0', 'skipped', 'app_still_running')]);
    expect([stuck.status, stuck.code]).toEqual(['skipped', 'still_running']);
    expect(outcomeFromItems({ app_instance_id: 'x', name: 'X', action_ids: ['q9', 'u9'] }, []).status).toBe('pending');
  });
});

describe('UNI-11 through the preview backend', () => {
  it('a protected app in a batch is refused as on its own; the others are planned and reviewed per application', async () => {
    let done: ((op: string) => void) | undefined;
    const plans: { items: PlanItem[]; extra?: unknown }[] = [];
    const sw = mockSoftware(
      (_kind, _mode, items, extra, _script, onDone) => {
        plans.push({ items, extra });
        done = onDone ? (op) => onDone(op, []) : undefined;
        return `up_${plans.length}`;
      },
      () => 'op_seed',
    );
    const npp = 'app_b17a3e60f2c4';
    const code = 'app_7c1e0a9b2f41'; // the preview's app that does not quit
    const agent = 'app_88f2c0b4e19d';
    const r = await sw.uninstallPlan([npp, agent, code]);
    expect(r.apps.map((a) => a.app_instance_id)).toEqual([npp, code]);
    expect(r.refused).toEqual([expect.objectContaining({ app_instance_id: agent, reason: 'policy_denied' })]);
    expect(plans[plans.length - 1].items.map((i) => i.action_id)).toEqual(['q0000000', 'u0000001', 'u0010000', 'u0010001']);
    await expect(sw.uninstallPlan([agent])).rejects.toMatchObject({ code: 'policy_denied' });
    done?.('op_batch');
    const l: Leftovers = await sw.uninstallLeftovers('op_batch');
    expect(l.app).toBeNull();
    expect(leftoversTitle(l)).toBe('审阅残留：2 个应用中的 1 个');
    const sections = leftoverSections(l);
    expect(sections.map((s) => [s.name, s.reviewable])).toEqual([
      ['Notepad++', true],
      ['Visual Studio Code', false],
    ]);
    expect(sections[1].objects).toBe(0);
    expect(sections[1].reason).toContain('全部保留');
    // UNI-05 holds in a batch: only caches and logs start selected.
    const all = l.groups.flatMap((g) => g.items);
    expect(all.filter((i) => i.selected).every((i) => i.group === 'caches_logs')).toBe(true);
  });
});
