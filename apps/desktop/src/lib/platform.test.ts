import { afterEach, describe, expect, it } from 'vitest';
import { PLATFORM_WORDS, notOnThisPlatform, previewPlatform, setPlatform, words, type PlatformWords } from './platform';
import { TOOLS } from './nav';
import { ERROR_HINT } from './format';
import { PLAN_KIND_LABEL } from './words';
import { ELEVATION_WORD, LEFTOVER_GROUP, PROHIBITED_WORDS, PROTECTION_WORD, ROUTE_WORD, TASK_CODE_WORD, TASK_WORDS, adapterText, metricReason, processNote, updateReason } from './suiteWords';

afterEach(() => setPlatform('windows'));

/** Every text a word entry can produce (functions are called with a sample number). */
function texts(w: PlatformWords): string[] {
  const out: string[] = [];
  const walk = (v: unknown) => {
    if (typeof v === 'string') out.push(v);
    else if (typeof v === 'function') out.push(String((v as (n: number) => string)(3)));
    else if (Array.isArray(v)) v.forEach(walk);
    else if (v && typeof v === 'object') Object.values(v).forEach(walk);
  };
  Object.values(w).forEach(walk);
  return out;
}

describe('GUI-12 platform wording table', () => {
  it('both platforms define every entry', () => {
    expect(Object.keys(PLATFORM_WORDS.macos).sort()).toEqual(Object.keys(PLATFORM_WORDS.windows).sort());
  });

  it('Windows keeps the text the pages showed before macOS, byte for byte', () => {
    const w = PLATFORM_WORDS.windows;
    expect(w.bin).toBe('回收站');
    expect(w.reveal).toBe('在资源管理器中显示');
    expect(w.loginSwitch).toBe('登录 Windows 时只启动托盘');
    expect(w.trayShow).toBe('在通知区域显示 CleanManager');
    expect(w.trayShowNote(5)).toBe('默认关闭。开启后每 5 秒采样一次处理器、内存和系统盘可用空间；关闭即停止采样、移除图标和登录启动项。');
    expect(w.secureKeys.join(w.secureJoin) + w.secureNote).toBe('Ctrl+Alt+Del 始终可用');
    expect(w.privacyFoot(3)).toBe('本页打开时每 3 秒读取一次 Windows 的使用记录；托盘开启时随托盘读取。只在本机显示，不保存、不上传。');
    expect(w.batteryFoot).toBe('蓝牙设备电量来自 Windows 蓝牙协议栈为设备记录的电量值（与“设置 › 蓝牙和其他设备”相同）；没有记录的设备不显示数字。');
    expect(w.maintAdminDraft(2)).toBe('其中 2 项需要管理员：执行开始前 Windows 会询问一次，只授权这份计划中的这些任务。拒绝只跳过这些任务，其余照常运行。CleanManager 不会在后台静默提权。');
    expect(w.updatesProgress).toBe('正在询问各更新通道（winget 访问它自己配置的源；Scoop 只读本地目录）…');
    expect(w.openStoreUpdates).toBe('打开商店的更新页');
    expect(w.leftoverKinds).toBe('文件、注册表项和启动项');
    expect(w.uninstallManually).toBe('无法验证卸载方式的应用请在“设置 › 应用 › 已安装的应用”中卸载；CleanManager 从不把删除安装目录当作卸载。');
    expect(w.processesFoot).toContain('可能与任务管理器不同');
    expect(w.devBuild).toBe('仅 Windows 开发构建，未发布');
    expect(w.offDoes).toEqual({});
    expect(w.kindExplain).toEqual({});
    expect(w.binPutBack).toBeNull();
    expect(w.cleanScreenLimit).toBeNull();
  });

  it('macOS never borrows a Windows idiom', () => {
    const windowsOnly = /Windows|回收站|资源管理器|Ctrl|(?<!没有)注册表|UAC|winget|Scoop|任务管理器|通知区域|托盘|商店的|设置 › 应用/;
    for (const t of texts(PLATFORM_WORDS.macos)) expect(t, t).not.toMatch(windowsOnly);
  });

  it('macOS says the platform things plainly', () => {
    const m = PLATFORM_WORDS.macos;
    expect(m.bin).toBe('废纸篓');
    expect(m.mod).toBe('⌘');
    expect(m.tray).toBe('菜单栏图标');
    expect(m.secureKeys.join(m.secureJoin) + m.secureNote).toBe('⌥⌘Esc（强制退出）始终可用');
    expect(m.startupSettings).toBe('系统设置 › 通用 › 登录项与扩展');
    expect(m.loginNote).toContain('SMAppService');
    expect(m.privacyFoot(3)).toMatch(/摄像头只知道正在使用.*macOS 14/);
    expect(m.maintAdminDraft(2)).toMatch(/^其中 2 项需要管理员：这一版在 macOS 上没有提权方式/);
    expect(m.binPutBack).toContain('放回原处');
    expect(m.updatesProgress).toContain('Homebrew');
    expect(m.uninstallManually).toMatch(/启动台.*访达/);
  });

  it('switching follows everywhere and switching back restores Windows exactly', () => {
    const before = { elevation: { ...ELEVATION_WORD }, codes: { ...TASK_CODE_WORD }, prohibited: JSON.stringify(PROHIBITED_WORDS), route: { ...ROUTE_WORD }, protection: { ...PROTECTION_WORD }, group: JSON.stringify(LEFTOVER_GROUP) };
    setPlatform('macos');
    expect(words().bin).toBe('废纸篓');
    expect(TOOLS[0].sections.find((s) => s.page === 'recyclebin')!.label).toBe('废纸篓');
    expect(TOOLS[0].blurb).toBe('缓存、构建产物、安装包与废纸篓');
    expect(PLAN_KIND_LABEL.manual_trash).toBe('手选废纸篓计划');
    expect(ERROR_HINT.recycle_bin_unavailable).toBe('所在的卷没有可用的废纸篓。不会改为永久删除。');
    expect(ELEVATION_WORD.unavailable).toContain('macOS 上没有提权方式');
    expect(TASK_CODE_WORD.elevation_unavailable).toContain('不会弹出密码提示');
    expect(PROHIBITED_WORDS['memory-release'].reason).toMatch(/^macOS 自行管理内存/);
    expect(ROUTE_WORD.open_store).toBe('在 App Store 中更新');
    expect(LEFTOVER_GROUP.system.explain).toContain('/Library');
    expect(notOnThisPlatform('目前只在 Windows 上实现。')).not.toContain('Windows');
    setPlatform('windows');
    expect(TOOLS[0].sections.find((s) => s.page === 'recyclebin')!.label).toBe('回收站');
    expect(TOOLS[0].blurb).toBe('缓存、构建产物、安装包与回收站');
    expect(TOOLS[3].blurb).toBe('磁盘占用、空间洞察与手选回收站');
    expect(PLAN_KIND_LABEL.manual_trash).toBe('手选回收站计划');
    expect(PLAN_KIND_LABEL.manual_purge).toBe('永久删除回收站项目计划');
    expect(ERROR_HINT.recycle_bin_unavailable).toBe('所在的卷没有可用的回收站。不会改为永久删除。');
    expect(ELEVATION_WORD).toEqual(before.elevation);
    expect(Object.keys(ELEVATION_WORD)).toEqual(Object.keys(before.elevation));
    expect(TASK_CODE_WORD).toEqual(before.codes);
    expect(JSON.stringify(PROHIBITED_WORDS)).toBe(before.prohibited);
    expect(ROUTE_WORD).toEqual(before.route);
    expect(PROTECTION_WORD).toEqual(before.protection);
    expect(JSON.stringify(LEFTOVER_GROUP)).toBe(before.group);
    expect(notOnThisPlatform('目前只在 Windows 上实现。')).toBe('目前只在 Windows 上实现。');
  });

  it('macOS host texts are put in words; Windows texts pass through unchanged', () => {
    expect(updateReason('No update channel: no Sparkle feed in Info.plist, not from the Mac App Store or Homebrew')).toMatch(/^没有更新通道：Info.plist/);
    expect(updateReason('No update channel: not installed through winget, Microsoft Store or Scoop')).toBe('没有更新通道：不是通过 winget、商店或 Scoop 安装，也没有可验证的更新源。');
    expect(updateReason('Updates itself (Squirrel/Electron update feed); CleanManager cannot verify that feed')).toMatch(/^应用自带更新器，其更新源无法验证/);
    expect(updateReason('Updates itself through Electron autoUpdater; CleanManager does not read')).toMatch(/^应用通过自带的更新器更新/);
    expect(updateReason('Sparkle feed offers 3.0.22; the app installs its own updates')).toContain('不下载也不安装');
    expect(metricReason('macOS publishes temperatures and fan speeds only through the System Management Controller (SMC)')).toContain('私有接口');
    expect(metricReason('the sensor service is not running')).toBe('the sensor service is not running');
    expect(processNote('Exited and waiting for its parent to collect its status (zombie)')).toContain('僵尸进程');
    expect(adapterText({ adapter_id: 'homebrew.uninstall-cask', display: "brew uninstall --cask firefox (package manager; the cask's zap stanza is never run)" })).toBe('Homebrew：brew uninstall --cask firefox（从不使用 --zap）');
    expect(adapterText({ adapter_id: 'winget.uninstall', display: 'winget uninstall --id Google.Chrome --exact' })).toBe('winget uninstall --id Google.Chrome --exact');
    expect(TASK_WORDS['macos.quicklook-reset'].title).toBe('重置快速预览缩略图缓存');
    expect(TASK_WORDS['windows.dns-flush'].title).toBe('刷新 DNS 解析缓存');
  });

  it('the browser preview picks the platform from ?platform=, else from the browser', () => {
    const mac = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 14_6) AppleWebKit/605.1.15';
    const win = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36';
    expect(previewPlatform('?platform=macos', win)).toBe('macos');
    expect(previewPlatform('?x=1&platform=windows', mac)).toBe('windows');
    expect(previewPlatform('', mac)).toBe('macos');
    expect(previewPlatform('', win)).toBe('windows');
    expect(previewPlatform('?platform=linux', win)).toBe('windows');
  });
});
