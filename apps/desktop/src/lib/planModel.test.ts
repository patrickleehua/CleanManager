import { afterEach, describe, expect, it } from 'vitest';
import { AUTH_TEXT, PLAN_KIND_CAPABILITY, authView, kindVerb, planCapability, planStatement, UNDO_LABEL } from './planModel';
import { capabilityOff } from './nav';
import featuresRs from '../../../../crates/cm-app/src/features.rs?raw';
import { setPlatform } from './platform';
import type { Mode, PlanDetail, PlanItem } from './types';
import { MODE_EXPLAIN, STALE_LABEL } from './words';

afterEach(() => setPlatform('windows'));

const MiB = 1024n * 1024n;
const plan = (over: Partial<PlanDetail> = {}): PlanDetail => ({
  plan_id: 'p_1',
  contract: 'plan/v1',
  kind: 'cleanup',
  mode: 'trash',
  digest: 'a'.repeat(64),
  digest_prefix: 'aaaaaaaa',
  created_at: '2026-10-22T08:00:00Z',
  expires_at: '2026-10-22T08:15:00Z',
  expired: false,
  seconds_left: 900,
  scan_id: 's1',
  scan_revision: 1,
  scope_id: 'scope',
  scan_root_display: 'C:\\Users\\Lin',
  rule_refs: ['microsoft.edge.code-cache@1.0.0', 'npm.cache@1.0.0'],
  rule_set_digest: 'r',
  stale_reason: null,
  item_count: 14,
  total_logical_bytes: String(812n * MiB),
  allocated_known_bytes: String(812n * MiB),
  allocated_unknown_count: 0,
  default_selected_items: 6,
  authorization: { status: 'required', source: null, policy_id: null, policy_version: null, principal: null, authorized_at: null, expires_at: null },
  policies: [],
  not_included: null,
  operation: null,
  review_token: 'rv1',
  cli: { show: 'cleanmanager plan show p_1', review: 'cleanmanager plan review p_1', apply: 'cleanmanager plan apply p_1' },
  ...over,
});
const items: PlanItem[] = [];
const cleanup = (mode: Mode) => planStatement(plan({ mode }), items);

describe('P05 plan review: one sentence + counts + reversibility, for each mode', () => {
  it('trash: moves to the Recycle Bin, can be restored, space freed only after emptying it', () => {
    const s = cleanup('trash');
    expect(s.sentence).toBe('把 14 个文件移到回收站');
    expect(s.irreversible).toBe(false);
    expect(s.undo).toBe('restorable');
    expect(s.undoLabel).toBe(UNDO_LABEL.restorable);
    expect(s.undoText).toBe(MODE_EXPLAIN.trash.recovery);
    expect(s.spaceText).toBe(MODE_EXPLAIN.trash.space);
    expect(s.tone).toBe('trash');
    expect(s.counts.map((c) => [c.label, c.value])).toEqual([
      ['对象', '14 项'],
      ['已知大小', '812.0 MiB'],
      ['来源', 'C:\\Users\\Lin'],
      ['规则', '2 条'],
    ]);
    expect(s.counts.find((c) => c.label === '来源')?.path).toBe(true);
  });

  it('quarantine: recoverable, and the statement always says it does not free space now', () => {
    const s = cleanup('quarantine');
    expect(s.sentence).toBe('把 14 个文件移入同卷隔离区');
    expect(s.irreversible).toBe(false);
    expect(s.undo).toBe('recoverable');
    expect(s.undoLabel).toContain('不立即释放空间');
    expect(s.spaceText).toMatch(/^此操作不立即释放空间/);
    expect(s.tone).toBe('quarantine');
  });

  it('delete: marked irreversible with its own word, never by colour alone', () => {
    const s = cleanup('delete');
    expect(s.sentence).toBe('永久删除 14 个文件');
    expect(s.irreversible).toBe(true);
    expect(s.undo).toBe('irreversible');
    expect(s.undoLabel).toBe('无法撤销');
    expect(s.tone).toBe('delete');
    // The badge says 无法撤销; the line under it does not repeat it.
    expect(s.undoText).toBe('没有回收站或隔离副本。');
    expect(kindVerb(plan({ mode: 'delete' }), items).permanent).toBe(true);
  });

  it('unknown total reads 未知, never 0', () => {
    const s = planStatement(plan({ total_logical_bytes: null }), items);
    expect(s.counts.find((c) => c.label === '已知大小')?.value).toBe('未知');
  });

  it('catalog plans name what they hold', () => {
    expect(planStatement(plan({ catalog: 'purge', mode: 'delete' }), items).sentence).toBe('永久删除 14 个构建产物中的文件');
    expect(planStatement(plan({ catalog: 'installer' }), items).sentence).toBe('把 14 个安装包移到回收站');
  });

  it('other kinds: permanent purges and process ends are irreversible; uninstall cannot be undone by CleanManager', () => {
    expect(planStatement(plan({ kind: 'manual_purge', mode: null }), items)).toMatchObject({ irreversible: true, undo: 'irreversible' });
    expect(planStatement(plan({ kind: 'purge', mode: null }), items)).toMatchObject({ sentence: '永久清空 14 个隔离对象', irreversible: true });
    expect(planStatement(plan({ kind: 'restore', mode: null }), items)).toMatchObject({ sentence: '把 14 个隔离对象恢复到原位置', irreversible: false, undo: 'none' });
    const proc = planStatement(plan({ kind: 'process', mode: null, item_count: 1 }), [{ action_id: 'a1', name: 'notepad.exe', path_display: 'notepad.exe', logical_bytes: '0', allocated_bytes: null, candidate_id: null, original_path_display: null, quarantine_id: null, item_kind: 'process' }]);
    expect(proc).toMatchObject({ irreversible: true, undo: 'irreversible' });
    expect(proc.counts.map((c) => c.label)).toEqual(['对象']);
    expect(planStatement(plan({ kind: 'uninstall', mode: null, item_count: 2 }), items)).toMatchObject({ irreversible: false, undo: 'external', tone: 'review' });
    expect(planStatement(plan({ kind: 'startup', mode: null, item_count: 1 }), items).undo).toBe('restorable');
  });

  it('macOS uses its own words for the same plan (GUI-12)', () => {
    setPlatform('macos');
    expect(cleanup('trash').sentence).toBe('把 14 个文件移到废纸篓');
    expect(planStatement(plan({ kind: 'manual_trash', mode: 'trash' }), items).sentence).toBe('把 14 个手选对象移到废纸篓');
  });
});

describe('P05 approval and refusal states', () => {
  const auth = (status: PlanDetail['authorization']['status']) => ({ ...plan().authorization, status });

  it('waiting for review: the seal, no state line', () => {
    expect(authView(plan())).toEqual({ view: 'review', line: null });
  });

  it('approved: a success line; executed plans point to their result', () => {
    expect(authView(plan({ authorization: auth('authorized'), review_token: null }))).toEqual({ view: 'authorized', line: { kind: 'success', text: AUTH_TEXT.authorized } });
    expect(authView(plan({ authorization: auth('authorized'), operation: { operation_id: 'op1', state: 'completed' } })).view).toBe('executed');
  });

  it('refused: stale, revoked and expired plans say why and cannot be approved', () => {
    expect(authView(plan({ stale_reason: 'rule_set_changed', review_token: null }))).toEqual({ view: 'stale', line: { kind: 'failed', text: STALE_LABEL.rule_set_changed } });
    expect(authView(plan({ authorization: auth('revoked'), review_token: null })).line?.kind).toBe('cancelled');
    expect(authView(plan({ authorization: auth('expired'), review_token: null })).view).toBe('expired');
  });

  it('terminal-only plans are approved in the terminal, never here', () => {
    expect(authView(plan({ terminal_only: 'permanent_delete' })).view).toBe('terminal');
  });
});

describe('D15 / WFA-02: a stored plan of a capability that is off', () => {
  const auth = (status: PlanDetail['authorization']['status']) => ({ ...plan().authorization, status });
  const manual = (over: Partial<PlanDetail> = {}) => plan({ kind: 'manual_trash', contract: 'plan/v2', plan_kind_v2: 'manual_selection', ...over });

  it('names the same capability for every plan/v2 kind as the host (cm_app::features::plan_kind_capability)', () => {
    const rows = [...featuresRs.slice(featuresRs.indexOf('pub fn plan_kind_capability')).matchAll(/PlanKind::(\w+)(?:\s*\|\s*PlanKind::(\w+))?\s*=>\s*(Some\("([a-z_.]+)"\)|None)/g)];
    const snake = (s: string) => s.replace(/[A-Z]/g, (c, i) => (i ? '_' : '') + c.toLowerCase());
    const host: Record<string, string | null> = {};
    for (const m of rows) for (const k of [m[1], m[2]].filter(Boolean)) host[snake(k)] = m[4] ?? null;
    expect(Object.keys(host).sort()).toEqual(['cleanup', 'maintenance', 'manual_selection', 'process', 'recovery', 'startup', 'uninstall', 'update']);
    expect(PLAN_KIND_CAPABILITY).toEqual(host);
    expect(planCapability(manual())).toBe('trash.manual');
    // v1 cleanup and recovery plans carry no plan/v2 kind and are core.
    expect(planCapability(plan())).toBeNull();
    expect(planCapability(plan({ plan_kind_v2: 'cleanup' }))).toBeNull();
  });

  it('off: no seal and no approve or run control, the off line instead', () => {
    expect(authView(manual(), true)).toEqual({ view: 'gated', line: { kind: 'cancelled', text: AUTH_TEXT.gated } });
    expect(authView(manual({ authorization: auth('authorized'), review_token: null }), true).view).toBe('gated');
    expect(AUTH_TEXT.gated).toContain('默认关闭');
  });

  it('on, or nothing left to decide: the usual states', () => {
    expect(authView(manual(), false).view).toBe('review');
    // A plan that already ran keeps its result; a stale one stays stale; revoked and expired stay closed.
    expect(authView(manual({ operation: { operation_id: 'op1', state: 'completed' } }), true).view).toBe('executed');
    expect(authView(manual({ stale_reason: 'rule_set_changed' }), true).view).toBe('stale');
    expect(authView(manual({ authorization: auth('expired'), review_token: null }), true).view).toBe('expired');
    expect(authView(manual({ authorization: auth('revoked'), review_token: null }), true).view).toBe('revoked');
  });

  it('the capability is off unless the host row says it is on (no rows: off, as the host would refuse)', () => {
    const row = (enabled: boolean) => [{ capability: 'trash.manual', gate: 'G1', gate_passed: false, enabled }];
    expect(capabilityOff('trash.manual', row(false))).toEqual({ capability: 'trash.manual', gate: 'G1' });
    expect(capabilityOff('trash.manual', row(true))).toBeNull();
    expect(capabilityOff('trash.manual', null)).not.toBeNull();
  });
});
