import { describe, expect, it } from 'vitest';
import { plannable, shownState } from './updates';
import { allDone, prohibitedWords, taskReason, taskSelectable, taskTitle } from './maintenance';
import { privacyPendingSize, alertText, overviewCard, privacyPending } from './tools';
import { leftoverTier, updateReason, whyNotWord } from '../lib/suiteWords';
import type { MaintenanceTask, UpdateCheck } from '../lib/suiteTypes';

const check = (over: Partial<UpdateCheck>): UpdateCheck => ({
  app_instance_id: 'app_1', name: 'App', current_version: '1', available_version: '2', channel: 'winget', channel_id: 'Vendor.App', state: 'update_available', reason: null, checked_at: null, route: 'package_manager', official_url: null, ...over,
});

describe('UPD-01..03 update rows', () => {
  it('never reads "up to date" without a channel that answered', () => {
    expect(shownState(check({ state: 'up_to_date', channel: null }))).toBe('cannot_check');
    expect(shownState(check({ state: 'up_to_date' }))).toBe('up_to_date');
  });
  it('only package-manager updates with a channel go into a plan', () => {
    expect(plannable(check({}))).toBe(true);
    expect(plannable(check({ route: 'open_store', channel: 'microsoft_store' }))).toBe(false);
    expect(plannable(check({ state: 'cannot_check' }))).toBe(false);
  });
  it('core reasons are shown in words, unknown ones as sent', () => {
    expect(updateReason('No update channel: not installed through winget')).toMatch(/^没有更新通道/);
    expect(updateReason('something new')).toBe('something new');
    expect(updateReason(null)).toBeNull();
  });
});

const task = (over: Partial<MaintenanceTask>): MaintenanceTask => ({
  task_id: 'windows.dns-flush', task_version: '1.0.0', title: 'Flush DNS resolver cache', problem: '', impact: '', requires_admin: false, applies_when: '', estimated_seconds: 1, applicability: 'applicable', applicability_reason: null, user_protected: false, reversible: false, ...over,
});

describe('MNT-01..07 maintenance', () => {
  it('protected or inapplicable tasks cannot be selected', () => {
    expect(taskSelectable(task({}))).toBe(true);
    expect(taskSelectable(task({ applicability: 'unknown' }))).toBe(true);
    expect(taskSelectable(task({ user_protected: true }))).toBe(false);
    expect(taskSelectable(task({ applicability: 'not_needed' }))).toBe(false);
  });
  it('a run with a skipped task is never "all done"', () => {
    const zero = { applied: 0, unchanged: 0, attention: 0, skipped: 0, unavailable: 0, failed: 0 };
    expect(allDone({ counts: { ...zero, applied: 2, unchanged: 1 } })).toBe(true);
    expect(allDone({ counts: { ...zero, applied: 1, skipped: 1 } })).toBe(false);
    expect(allDone({ counts: { ...zero, attention: 1 } })).toBe(false);
  });
  it('tasks, refusals and reasons have words', () => {
    expect(taskTitle('windows.dns-flush', 'x')).toBe('刷新 DNS 解析缓存');
    expect(taskTitle('unknown.task', 'Fallback')).toBe('Fallback');
    expect(prohibitedWords({ id: 'registry-optimizer', title: 'Registry', reason: 'r' }).title).toMatch(/注册表/);
    expect(taskReason('elevation_declined', 'declined')).toMatch(/拒绝/);
    expect(taskReason('flushed', 'DNS resolver cache flushed')).toBe('DNS resolver cache flushed');
  });
});

describe('UNI-05 leftovers and DIN-03/STS-05 words', () => {
  it('kept leftovers use the blocked shape; reasons have words', () => {
    expect(leftoverTier('kept')).toBe('blocked');
    expect(leftoverTier('default_selected')).toBe('default_selected');
    expect(whyNotWord('kept')).toMatch(/保留/);
  });
  it('an overview card without a size shows the count or "unknown", never 0', () => {
    const c = { card_id: 'c', kind: 'snapshots', title: 'Snapshots', logical_bytes: null, count: 3, size_note: 'no admin', observed_at: null };
    expect(overviewCard(c)).toMatchObject({ title: '还原点与快照', size: '3 个', note: 'no admin' });
    expect(overviewCard({ ...c, count: null }).size).toBe('大小未知');
    expect(overviewCard({ ...c, logical_bytes: '1073741824' }).size).toBe('1.0 GiB');
  });
  it('a card waiting for a macOS privacy prompt says so, with no size or count', () => {
    const note = 'Downloads: permission_required (permission_cause=privacy_prompt_pending): macOS did not answer within 5 s';
    const c = { card_id: 'c', kind: 'old_downloads', title: 'Old downloads', logical_bytes: null, count: null, size_note: note, observed_at: null };
    const v = overviewCard(c);
    expect(v).toMatchObject({ size: privacyPendingSize(), pending: true });
    expect(v.note).toMatch(/隐私对话框/);
    expect(v.note).toMatch(/不按 0 计算/);
    expect(privacyPending('permission_required (permission_cause=undetermined): denied')).toBe(false);
    expect(overviewCard({ ...c, logical_bytes: '10' }).pending).toBe(false);
  });
  it('a sustained-CPU alert names the process, window and threshold', () => {
    const text = alertText({ pid: 7, name: 'node.exe', image_path: null, min_cpu_percent: 86.4, threshold_percent: 80, window_seconds: 120, since: '', raised_at: '', link: '' });
    expect(text).toContain('node.exe（PID 7）');
    expect(text).toContain('120 秒');
    expect(text).toContain('80%');
  });
});
