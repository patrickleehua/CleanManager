import { describe, expect, it } from 'vitest';
import { TOOLS, placeOf } from '../lib/nav';
import { mockResident } from '../lib/mockResident';
import { EXIT_TEXT, NOT_HERE, NOT_HERE_MACOS, batteryLists, loginUnsupported, notHereWords, privacyLine, remainingText, trayLines, unavailableReason } from './resident';
import { setPlatform } from '../lib/platform';

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

describe('状态 › 常驻工具 on macOS', () => {
  it('devices without a reading are listed as unavailable with the reason, never as 0%', async () => {
    setPlatform('macos');
    try {
      const b = await mockResident(true).residentBatteries();
      const l = batteryLists(b);
      expect(l.devices.map((d) => [d.name, d.percent])).toEqual([
        ['本机电池', 81],
        ['Magic Keyboard', 17],
      ]);
      expect(l.unavailable).toEqual([
        { device_id: 'bt:f0-b3-ec-65-43-21', name: 'AirPods Pro', note: '蓝牙设备', reason: '设备没有公布电量，这里不估计。' },
        { device_id: 'ioreg:0x100000a2f', name: 'USB Receiver', note: '外设', reason: '设备报告的电量不是数字，不显示。' },
      ]);
      // Listed devices explain themselves: no "N devices did not report" line on top of the list.
      expect(l.note).toBeNull();
      for (const u of l.unavailable) expect(Object.keys(u)).not.toContain('percent');
      expect(unavailableReason('BatteryPercent 140 is outside 0–100; not shown')).toBe('设备报告的电量超出 0–100，不显示。');
      expect(unavailableReason('something new')).toBe('something new');
    } finally {
      setPlatform('windows');
    }
  });

  it('Windows battery lists read exactly as before', () => {
    const base = { supported: true, system: null, devices: [], devices_unavailable: null, source: '' };
    expect(batteryLists({ ...base, bluetooth_devices: 2 }).note).toBe('2 个蓝牙设备没有向 Windows 报告电量，这里不做估计。');
    expect(batteryLists({ ...base, bluetooth_devices: 0 }).note).toBe('没有已连接的蓝牙设备。');
    expect(batteryLists({ ...base, bluetooth_devices: 0, devices_unavailable: 'boom' }).note).toBe('设备电量不可读：boom');
    expect(batteryLists({ ...base, bluetooth_devices: 2 }).unavailable).toEqual([]);
  });

  it('the macOS not-here ids have their own labels and reasons', async () => {
    const s = await mockResident(true).residentStatus();
    expect(s.platform).toBe('macos');
    expect(s.not_here.every((n) => n.id.endsWith('_macos'))).toBe(true);
    for (const n of s.not_here) expect(NOT_HERE_MACOS[n.id], n.id).toBeDefined();
    expect(notHereWords('start_at_login_macos', 'x')).toEqual(NOT_HERE_MACOS.start_at_login_macos);
    expect(notHereWords('start_at_login_macos', 'x')[1]).toContain('SMAppService');
    expect(notHereWords('fan_control', 'x')).toEqual(NOT_HERE.fan_control);
    expect(notHereWords('new_gap_macos', 'host reason')).toEqual(['new_gap_macos', 'host reason']);
    expect(loginUnsupported(s)).toBe(true);
    expect(loginUnsupported(await mockResident().residentStatus())).toBe(false);
  });

  it('start at login is refused, keep-awake names its assertion, the camera has no app name', async () => {
    const b = mockResident(true);
    const s = await b.residentStatus();
    await expect(b.residentSave({ ...s.settings, tray: true, start_at_login: true })).rejects.toMatchObject({ code: 'unsupported' });
    const k = await b.residentKeepAwake(true, 30);
    expect(k.assertion).toMatch(/^IOPMAssertion PreventUserIdleDisplaySleep/);
    expect((await b.residentKeepAwake(false, null)).assertion).toBeNull();
    const p = await b.residentPrivacy();
    expect(p.in_use.find((a) => a.capabilities.includes('camera'))!.name).toContain('macOS 只报告设备正在使用');
  });
});
