import { afterEach, describe, expect, it } from 'vitest';
import { setPlatform } from '../lib/platform';
import { mockMac } from '../lib/mockMac';
import { uninstallRowWords } from '../lib/suiteWords';
import type { StartupItem, StatusSnapshot } from '../lib/types';
import type { UninstallApp } from '../lib/suiteTypes';
import { metricTiles, startupRow } from './tools';
import { UNINSTALL_STEPS } from './uninstall';

afterEach(() => setPlatform('windows'));

const preview = () => mockMac(() => 'plan_preview');

const item = (over: Partial<StartupItem>): StartupItem => ({
  startup_item_id: 'st_1', name: 'Steam', source: 'run_key', definition: 'HKCU\\…\\Run : Steam', target: { display: 'C:\\Steam\\steam.exe' }, publisher: 'Valve Corp.', publisher_evidence: 'authenticode', scope: 'user', state: 'enabled', toggle_capability: 'available', protected: false, reason: null, ...over,
});

describe('软件 › 启动项 on macOS (STA-01..04)', () => {
  it('a launchd row in state unknown says why, is not guessed, and offers no change button', async () => {
    const list = await preview().startupList!();
    const zoom = list.items.find((i) => i.name === 'us.zoom.updater')!;
    const v = startupRow(zoom, true);
    expect(v.state).toEqual(['状态未知', 'muted']);
    expect(v.stateNote).toContain('不会显示为已启用或已停用');
    expect(v.showAction).toBe(false);
    expect(v.why).toBe('只读：在应用自己的设置或系统设置中修改');
    // The platform-wide read-only sentence is said once by the page, not on every row.
    expect(v.reason).toMatch(/^Not listed as loaded/);
    expect(v.reason).not.toContain('Read-only on macOS');
  });

  it('the Service Management row opens System Settings instead of a plan', async () => {
    const list = await preview().startupList!();
    const sm = list.items.find((i) => i.source === 'sm_background_item')!;
    const v = startupRow(sm, true);
    expect(v.settingsLink).toBe(true);
    expect(v.showAction).toBe(false);
    expect(v.name).toBe('登录项与后台项目（Service Management）');
    expect(v.state[0]).toBe('状态未知');
    expect(v.reason).toContain('系统设置 › 通用 › 登录项与扩展');
  });

  it('toggle_capability unsupported rows stay read-only; Windows rows read as before', () => {
    expect(startupRow(item({ toggle_capability: 'unsupported', reason: 'Read-only on macOS in this build: x' }), true)).toMatchObject({ showAction: false, reason: null });
    const w = startupRow(item({ toggle_capability: 'unsupported', reason: 'Security software: shown read-only' }), false);
    expect(w).toEqual({ name: 'Steam', state: ['已启用', 'good'], why: '没有可验证的修改方式', reason: 'Security software: shown read-only', stateNote: null, settingsLink: false, showAction: true });
    expect(startupRow(item({ toggle_capability: 'permission_required' }), false).why).toBe('需要管理员权限');
  });
});

describe('软件 › 卸载 on macOS (UNI-01)', () => {
  it('permission_required rows say where to remove them: Launchpad or Finder, never Settings', async () => {
    const list = await preview().uninstallList!();
    const pages = list.items.find((a) => a.name === 'Pages')!;
    const word = list.items.find((a) => a.name === 'Microsoft Word')!;
    const p = uninstallRowWords(pages, true);
    expect(p).toMatchObject({ label: '需要管理员', tone: 'warn', settingsButton: false, reason: null });
    expect(p.guidance).toMatch(/启动台中按住 Option/);
    const w = uninstallRowWords(word, true);
    expect(w.guidance).toMatch(/访达中移到废纸篓（访达会要求管理员密码）/);
    expect(w.reason).toBe('Owned by root (installed by an installer package)');
  });

  it('the reason text alone is enough when an older host sends no refusal code', () => {
    const app: Pick<UninstallApp, 'state' | 'source' | 'reason'> = { state: 'unverified', source: 'app_bundle', reason: 'Owned by root: moving it needs administrator rights; this build has no administrator path on macOS' };
    expect(uninstallRowWords(app, true).label).toBe('需要管理员');
    expect(uninstallRowWords({ ...app, reason: 'the bundle identity could not be read' }, true)).toMatchObject({ label: '需要手动卸载', settingsButton: false });
  });

  it('Windows rows keep the Settings button and their words', () => {
    expect(uninstallRowWords({ state: 'unverified', source: 'registry_uninstall', reason: 'script host' }, false)).toEqual({ label: '无法验证', tone: 'warn', settingsButton: true, guidance: null, reason: 'script host' });
    expect(uninstallRowWords({ state: 'unsupported', source: 'registry_uninstall', reason: 'agent' }, false)).toMatchObject({ label: '受保护', settingsButton: false });
  });

  it('the two steps follow the platform and come back unchanged', () => {
    setPlatform('macos');
    expect(UNINSTALL_STEPS[0].note).toContain('废纸篓');
    expect(UNINSTALL_STEPS[1].note).not.toContain('注册表');
    setPlatform('windows');
    expect(UNINSTALL_STEPS[0].note).toBe('先请应用退出，再运行它自己的卸载程序，最后重新检查安装目录和登记。');
    expect(UNINSTALL_STEPS[1].note).toBe('卸载后单独审阅确实留下的文件、注册表项和启动项。');
  });
});

describe('状态 › 系统状态 on macOS (STS-01)', () => {
  it('temperatures and fans are unavailable with the reason in words; zombies are counted', async () => {
    const snap = (await preview().statusSnapshot!()) as StatusSnapshot;
    const tiles = metricTiles(snap, true);
    const by = (label: string) => tiles.find((t) => t.label === label)!;
    expect(by('最高温度')).toMatchObject({ value: '不可用', available: false });
    expect(by('最高温度').note).toContain('系统管理控制器（SMC）');
    expect(by('风扇')).toMatchObject({ value: '不可用', available: false });
    expect(by('进程')).toMatchObject({ value: '512', available: true });
    expect(by('进程').note).toMatch(/^其中 2 个僵尸进程/);
    expect(by('电池健康').note).toBe('相对设计容量，已循环 187 次');
    // Windows shows the shared eight tiles only.
    expect(metricTiles(snap, false).map((t) => t.label)).toEqual(['处理器', '内存已用', '系统盘可用', '磁盘繁忙', '最高温度', '显卡温度', '电量', '电池健康']);
  });
});

describe('优化 › 维护任务 on macOS (MNT-04)', () => {
  it('administrator tasks are skipped elevation_unavailable; the run is not "all clear"', async () => {
    const b = preview();
    const cat = await b.maintenanceCatalog!();
    expect(cat.elevation).toBe('unavailable');
    expect(cat.items.some((t) => t.requires_admin)).toBe(true);
    const h = await b.maintenanceHistory!(10);
    const dns = h.items[0].items.find((i) => i.task_id === 'macos.dns-flush')!;
    expect([dns.outcome, dns.code]).toEqual(['skipped', 'elevation_unavailable']);
    expect(h.items[0].all_clear).toBe(false);
  });
});
