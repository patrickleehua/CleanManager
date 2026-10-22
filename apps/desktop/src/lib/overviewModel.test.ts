import { describe, expect, it } from 'vitest';
import { overviewPlan } from './overviewModel';
import type { Issue, ScanStatus, Summary } from './types';

const GiB = 1024n ** 3n;

const summary = (over: Partial<Summary> = {}): Summary => ({
  logical_known_bytes: String(66n * GiB),
  logical_unknown_count: 0,
  allocated_exact_bytes: String(64n * GiB),
  allocated_known_bytes: String(64n * GiB),
  allocated_unknown_count: 0,
  covered_entries: 1487,
  overlapping_selected: 0,
  unread_ranges: 0,
  ...over,
});

const status = (over: Partial<ScanStatus> = {}): ScanStatus => ({
  scan_id: 's1',
  scope_id: 'scope',
  state: 'completed',
  revision: 1,
  published: true,
  completeness: 'verified_scope',
  cancel_requested: false,
  host_kind: 'desktop',
  host_pid: 1,
  hosted_here: false,
  heartbeat_at: null,
  entries_seen: 1487,
  skipped_ranges: 0,
  started_at: '2026-10-22T04:41:00Z',
  finished_at: '2026-10-22T04:41:41Z',
  root_display: 'D:\\Work',
  root_entry_id: '1',
  summary: summary(),
  backend: 'win.directory',
  backend_reason: null,
  elevated_read: [],
  issues: [],
  issues_total: 0,
  ...over,
});

/** The acceptance run's failed file root: failed, 0 entries, published, no sizes, one root issue. */
const failedFile = (issue: Issue = { code: 'not_a_directory', scope_display: 'C:\\in\\a.txt', message: 'root is not a directory' }) =>
  status({ state: 'failed', entries_seen: 0, completeness: 'partial_scope', summary: null, root_entry_id: null, root_display: 'C:\\in\\a.txt', issues: [issue], issues_total: 1 });

describe('P02 overview state matrix', () => {
  it('failed: the reason and the next step only — no coverage, no browse/candidates, no machine-wide cards', () => {
    for (const issue of [
      { code: 'not_a_directory', scope_display: 'C:\\in\\a.txt', message: 'root is not a directory' },
      { code: 'root_not_directory', scope_display: 'C:\\in\\a.txt', message: 'root is not a directory' },
      { code: 'root_is_link', scope_display: 'C:\\in\\link', message: 'x' },
      { code: 'root_unavailable', scope_display: 'E:\\gone', message: 'The system cannot find the path specified. (os error 3)' },
    ]) {
      const p = overviewPlan(failedFile(issue));
      expect(p.layout).toBe('failed');
      expect(p.banner.kind).toBe('failed');
      expect(p.gaps, issue.code).toBeNull();
      expect(p.space, issue.code).toBeNull();
      expect(p.browse, issue.code).toBeNull();
      expect(p.candidates, issue.code).toBe(false);
      expect(p.systemCards, issue.code).toBe(false);
      expect(p.aside, issue.code).toBe(false);
      // A refused root: the next step is choosing another one, not scanning the same file again.
      expect(p.chooseRoot, issue.code).toBe('primary');
      expect(p.rescan, issue.code).toBeNull();
      // The main text is Chinese, never the raw code or English message.
      expect(p.banner.sentence).not.toMatch(/[a-z_]{4,}/i);
    }
    expect(overviewPlan(failedFile()).banner.sentence).toBe('所选路径是文件，不是文件夹，请选择一个文件夹。');
    expect(overviewPlan(failedFile({ code: 'root_is_link', scope_display: 'C:\\l', message: '' })).banner.sentence).toBe('所选路径是链接（目录联接或符号链接），请直接选择它指向的文件夹。');
  });

  it('failed without a root refusal offers scanning again first; an unknown code reads as unknown, never raw', () => {
    const p = overviewPlan(status({ state: 'failed', summary: null, issues: [{ code: 'volume_changed', scope_display: 'D:\\Work', message: 'volume identity changed' }], issues_total: 1 }));
    expect(p.rescan).toBe('primary');
    expect(p.chooseRoot).toBe('secondary');
    expect(p.browse).toBeNull();
    const u = overviewPlan(status({ state: 'failed', summary: null, issues: [{ code: 'quota_x', scope_display: 'D:\\', message: 'disk quota exceeded' }], issues_total: 1 }));
    expect(u.banner.sentence).toBe('未识别的问题（代码 quota_x）');
    expect(u.reason?.raw).toBe('disk quota exceeded');
    expect(overviewPlan(status({ state: 'failed', summary: null })).banner.sentence).toContain('没有记录原因');
  });

  it('completed: the space summary first, browse and candidates, no gaps panel', () => {
    const p = overviewPlan(status());
    expect(p.layout).toBe('completed');
    expect(p.banner).toMatchObject({ kind: 'success', title: '扫描完成' });
    expect(p.space).toMatchObject({ text: '66.0 GiB', atLeast: false });
    expect(p.gaps).toBeNull();
    expect(p.browse).toBe('results');
    expect(p.candidates).toBe(true);
    expect(p.systemCards).toBe(true);
    expect(p.cancel).toBe(false);
  });

  it('completed with unknown sizes says "at least" and lists the unknown part', () => {
    const p = overviewPlan(status({ summary: summary({ logical_unknown_count: 2 }) }));
    expect(p.layout).toBe('completed');
    expect(p.space?.text).toBe('至少 66.0 GiB');
    expect(p.gaps).toEqual({ unread: 0, unknownSizes: 2 });
  });

  it('partial: what was missed first, the known part as "at least", never the "nothing missing" sentence', () => {
    const p = overviewPlan(
      status({ state: 'partial', completeness: 'partial_scope', skipped_ranges: 1, summary: summary({ unread_ranges: 1, logical_unknown_count: 2 }), issues: [{ code: 'permission_required', scope_display: 'D:\\Work\\Locked', message: 'Access is denied. (os error 5)' }], issues_total: 1 }),
    );
    expect(p.layout).toBe('partial');
    expect(p.banner.kind).toBe('partial');
    expect(p.banner.title).toBe('部分完成：1 处位置无法读取');
    expect(p.gaps).toEqual({ unread: 1, unknownSizes: 1 });
    expect(p.space).toMatchObject({ text: '至少 66.0 GiB', atLeast: true });
    expect(p.browse).toBe('results');
    expect(p.pendingPermission).toBe(false);
  });

  it('partial with a macOS privacy prompt pending: wait for the answer, then scan again', () => {
    const pending: Issue = { code: 'permission_required', scope_display: '/Users/lin/Documents', message: 'permission_required (permission_cause=privacy_prompt_pending)', permission_cause: 'privacy_prompt_pending' };
    const p = overviewPlan(status({ state: 'partial', summary: summary({ unread_ranges: 1, logical_unknown_count: 1 }), issues: [pending], issues_total: 1, root_display: '/Users/lin' }));
    expect(p.banner).toMatchObject({ kind: 'pending-permission', title: '等待 macOS 授权' });
    expect(p.pendingPermission).toBe(true);
    expect(p.rescan).toBe('primary');
    // Also when only the message carries the marker (older hosts).
    const old = overviewPlan(status({ state: 'partial', summary: summary({ unread_ranges: 1 }), issues: [{ ...pending, permission_cause: undefined }], issues_total: 1 }));
    expect(old.pendingPermission).toBe(true);
  });

  it('cancelled and interrupted: how much was kept, browse what was kept, scan again', () => {
    const kept = overviewPlan(status({ state: 'cancelled', completeness: 'partial_scope', entries_seen: 812, summary: summary({ logical_known_bytes: String(45n * GiB) }) }));
    expect(kept.layout).toBe('cancelled');
    expect(kept.banner.kind).toBe('cancelled');
    expect(kept.banner.sentence).toBe('取消前已保留 812 个条目，结果不完整。');
    expect(kept.space).toMatchObject({ text: '至少 45.0 GiB', label: '已保留的部分' });
    expect(kept.browse).toBe('kept');
    expect(kept.candidates).toBe(false);
    expect(kept.systemCards).toBe(false);
    expect(kept.rescan).toBe('primary');
    const none = overviewPlan(status({ state: 'cancelled', entries_seen: 0, summary: null }));
    expect(none.banner.sentence).toBe('取消前没有保留任何条目。');
    expect(none.space).toBeNull();
    expect(none.browse).toBeNull();
    const intr = overviewPlan(status({ state: 'interrupted', entries_seen: 812 }));
    expect(intr.layout).toBe('interrupted');
    expect(intr.banner.title).toBe('扫描已中断');
    expect(intr.browse).toBe('kept');
  });

  it('running: progress and cancel only — no sizes, no browse, no candidates', () => {
    for (const state of ['created', 'running', 'stopping'] as const) {
      const p = overviewPlan(status({ state, published: false, summary: null, entries_seen: 4318 }));
      expect(p.layout).toBe('running');
      expect(p.banner.kind).toBe('running');
      expect(p.cancel).toBe(true);
      expect(p.space).toBeNull();
      expect(p.browse).toBeNull();
      expect(p.candidates).toBe(false);
      expect(p.systemCards).toBe(false);
    }
    expect(overviewPlan(status({ state: 'running', published: false, summary: null, entries_seen: 4318 })).banner.sentence).toBe('已发现 4,318 个条目，切换页面不会中断扫描。');
    expect(overviewPlan(status({ state: 'running', published: false, summary: null, cancel_requested: true })).banner.title).toBe('正在停止扫描');
  });

  it('each layout has its own banner kind (shape), so states never share one look', () => {
    const kinds = new Set(
      [
        status(),
        status({ state: 'partial', summary: summary({ unread_ranges: 1 }) }),
        failedFile(),
        status({ state: 'cancelled' }),
        status({ state: 'running', published: false, summary: null }),
        status({ state: 'partial', summary: summary({ unread_ranges: 1 }), issues: [{ code: 'permission_required', scope_display: '/x', message: '', permission_cause: 'privacy_prompt_pending' }] }),
      ].map((s) => overviewPlan(s).banner.kind),
    );
    expect([...kinds].sort()).toEqual(['cancelled', 'failed', 'partial', 'pending-permission', 'running', 'success']);
  });
});
