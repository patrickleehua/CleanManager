import { afterEach, describe, expect, it } from 'vitest';
import { explainResult, groupIssues } from './codes';
import { OPERATION_TEXT, operationBanner, resultIssues, type OperationFacts } from './operationModel';
import { setPlatform } from './platform';
import type { Counts, OperationItem } from './types';

afterEach(() => setPlatform('windows'));

const counts = (over: Partial<Counts> = {}): Counts => ({ succeeded: 0, skipped: 0, failed: 0, not_started: 0, unknown: 0, in_flight: 0, ...over });
const facts = (over: Partial<OperationFacts> = {}): OperationFacts => ({ state: 'completed', plan_kind: 'cleanup', mode: 'trash', counts: counts({ succeeded: 14 }), total: 14, needs_reconcile: false, live: false, ...over });

describe('P06 operation result: the outcome banner', () => {
  it('completed: success, with the action and what happens to space for each mode', () => {
    expect(operationBanner(facts())).toEqual({ kind: 'success', title: '全部完成：移到回收站 14 项', sentence: OPERATION_TEXT.doneTrash('回收站', '在资源管理器的回收站中还原') });
    // A same-volume quarantine is never celebrated as space reclaimed.
    expect(operationBanner(facts({ mode: 'quarantine' })).sentence).toBe(OPERATION_TEXT.doneQuarantine);
    expect(operationBanner(facts({ mode: 'delete' })).sentence).toBe(OPERATION_TEXT.doneDelete);
  });

  it('partial: half shape; the title counts what did not finish', () => {
    const b = operationBanner(facts({ state: 'partial', counts: counts({ succeeded: 12, skipped: 2 }) }));
    expect(b.kind).toBe('partial');
    expect(b.title).toBe('部分完成：12 项成功，2 项没有完成');
    expect(b.sentence).toBe(OPERATION_TEXT.partialSentence);
    expect(operationBanner(facts({ state: 'partial', plan_kind: 'uninstall', mode: null, counts: counts({ succeeded: 2 }) })).sentence).toBe(OPERATION_TEXT.partialUninstall);
  });

  it('unconfirmed outcomes lead to a read-only check, never to "done"', () => {
    const b = operationBanner(facts({ state: 'reconciliation_required', needs_reconcile: true, counts: counts({ succeeded: 2, unknown: 1, not_started: 3 }) }));
    expect(b.kind).toBe('partial');
    expect(b.title).toBe(OPERATION_TEXT.reconcile);
  });

  it('running, stopping, cancelled and failed each have their own shape and words', () => {
    expect(operationBanner(facts({ state: 'running', live: true, counts: counts({ succeeded: 3, in_flight: 1 }) }))).toMatchObject({ kind: 'running', title: OPERATION_TEXT.running, sentence: OPERATION_TEXT.runningSentence(3, 14) });
    expect(operationBanner(facts({ state: 'stopping', live: true })).title).toBe(OPERATION_TEXT.stopping);
    expect(operationBanner(facts({ state: 'cancelled', counts: counts({ succeeded: 4, not_started: 10 }) })).kind).toBe('cancelled');
    expect(operationBanner(facts({ state: 'failed', counts: counts({ failed: 14 }) })).kind).toBe('failed');
  });

  it('macOS says 废纸篓 and 访达', () => {
    setPlatform('macos');
    expect(operationBanner(facts()).title).toBe('全部完成：移到废纸篓 14 项');
    expect(operationBanner(facts()).sentence).toContain('废纸篓');
  });
});

describe('P06 per-item issues through the shared issue lines', () => {
  const item = (over: Partial<OperationItem>): OperationItem => ({ action_id: 'a', name: 'x', path_display: 'C:\\x', original_path_display: null, logical_bytes: '1', status: 'succeeded', code: 'done', message: '', recovery: null, ...over });

  it('lists only what did not succeed, at its path', () => {
    const issues = resultIssues([item({}), item({ status: 'skipped', code: 'precondition_in_use', path_display: 'C:\\a' }), item({ status: 'pending', code: null }), item({ status: 'failed', code: 'access_denied', message: 'Access is denied. (os error 5)', path_display: 'C:\\b' })]);
    expect(issues).toEqual([
      { code: 'precondition_in_use', scope_display: 'C:\\a', message: '' },
      { code: 'access_denied', scope_display: 'C:\\b', message: 'Access is denied. (os error 5)' },
    ]);
  });

  it('result codes read in Chinese; the host message stays raw only for 技术详情; unknown codes say so', () => {
    const g = groupIssues(
      resultIssues([item({ status: 'skipped', code: 'precondition_in_use' }), item({ status: 'skipped', code: 'precondition_in_use', path_display: 'C:\\y' }), item({ status: 'failed', code: 'access_denied', message: 'Access is denied.' }), item({ status: 'failed', code: 'brand_new_code', message: 'raw' })]),
      explainResult,
    );
    expect(g).toHaveLength(3);
    expect(g[0]).toMatchObject({ count: 2, issue: { text: '文件正被占用，未操作。', known: true } });
    expect(g[1].issue.text).not.toMatch(/Access is denied/);
    expect(g[1].raws[0].message).toBe('Access is denied.');
    expect(g[2].issue).toMatchObject({ known: false, text: '未识别的问题（代码 brand_new_code）' });
  });
});
