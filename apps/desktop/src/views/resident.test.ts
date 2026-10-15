import { describe, expect, it } from 'vitest';
import { TOOLS, placeOf } from '../lib/nav';
import { mockResident } from '../lib/mockResident';
import { EXIT_TEXT, NOT_HERE, privacyLine, remainingText, trayLines } from './resident';

describe('状态 › 常驻工具 (RES-01..04)', () => {
  it('sits in the status tool after the processes page', () => {
    const status = TOOLS.find((t) => t.id === 'status')!;
    expect(status.sections.map((s) => s.label)).toEqual(['系统状态', '进程', '常驻工具']);
    expect(placeOf('resident', 'operations', null)).toEqual({ tool: 'status' });
  });

  it('tray lines say unavailable instead of 0 and match the native menu wording', () => {
    expect(trayLines(null)).toEqual([['处理器', '—'], ['内存已用', '—'], ['系统盘可用', '—']]);
    expect(trayLines({ sampled_at: '2026-10-15T01:00:00Z', cpu_percent: 23.4, memory_percent: null, disk_free_bytes: 129_284_829_184 })).toEqual([
      ['处理器', '23%'],
      ['内存已用', '不可用'],
      ['系统盘可用', '120.4 GiB'],
    ]);
  });

  it('the privacy line names apps in use and clears when none are', () => {
    const base = { supported: true, unavailable_reason: null, source: '' };
    expect(privacyLine({ ...base, in_use: [] })).toBe('摄像头和麦克风 未在使用');
    expect(privacyLine({ ...base, in_use: [{ name: 'Zoom.exe', capabilities: ['microphone'], desktop_app: true }] })).toBe('正在使用：Zoom.exe');
    expect(privacyLine({ ...base, supported: false, in_use: [] })).toBe('摄像头和麦克风 不可用');
  });

  it('keep-awake time left reads naturally', () => {
    expect(remainingText(null)).toContain('直到');
    expect(remainingText(42)).toBe('剩余 42 秒');
    expect(remainingText(30 * 60)).toBe('剩余 30 分钟');
    expect(remainingText(65 * 60)).toBe('剩余 1 小时 5 分钟');
    expect(remainingText(120 * 60)).toBe('剩余 2 小时');
  });

  it('every clean-screen exit and every platform gap has words', () => {
    expect(Object.keys(EXIT_TEXT).sort()).toEqual(['closed', 'esc_held', 'timeout']);
    expect(Object.keys(NOT_HERE)).toEqual(['fan_control', 'charge_limit', 'low_latency_network', 'touch_id_sudo']);
  });

  it('the tray is off by default; turning it off removes the startup entry (preview backend)', async () => {
    const b = mockResident();
    const s0 = await b.residentStatus();
    expect(s0.settings.tray).toBe(false);
    expect(s0.tray.sampling).toBe(false);
    await expect(b.residentSave({ ...s0.settings, start_at_login: true })).rejects.toBeTruthy();
    const on = await b.residentSave({ ...s0.settings, tray: true, start_at_login: true });
    expect(on.tray.startup).toBe('at_login');
    const off = await b.residentSave({ ...on.settings, tray: false, start_at_login: false });
    expect(off.tray.sampling).toBe(false);
    expect(off.tray.startup_command).toBeNull();
  });
});
