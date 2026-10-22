// DEVELOPMENT-ONLY fixture for 软件 › 卸载 / 更新, 优化, 状态 › 进程 and the old-downloads review.
// Shapes mirror src-tauri/src/{software,system}.rs; plans go through the shared review fixture
// (`__plan`) with scripted per-item results, so review and result pages behave as on a real host.
// Nothing here can uninstall, update or end anything: it is in-memory preview data.

import type { Backend } from './api';
import type { Scripted } from './mockOps';
import type {
  AlertSettings,
  DownloadItem,
  Leftover,
  Leftovers,
  MaintenanceReport,
  MaintenanceTask,
  ProcessRow,
  ProcessSort,
  Prohibited,
  TaskOutcome,
  UninstallApp,
  UninstallRun,
  UpdateCheck,
  UpdateRouted,
} from './suiteTypes';
import type { IpcError, Mode, OperationItem, PlanDetail, PlanItem } from './types';

type SoftwareBackend = Pick<
  Backend,
  | 'uninstallList'
  | 'uninstallPlan'
  | 'uninstallLeftovers'
  | 'uninstallLeftoversPlan'
  | 'uninstallRestorePlan'
  | 'maintenanceRestorePlan'
  | 'updatesList'
  | 'updatesSetChecks'
  | 'updatesPlan'
  | 'openSystemPage'
  | 'maintenanceCatalog'
  | 'maintenanceSearch'
  | 'maintenancePlan'
  | 'maintenanceHistory'
  | 'maintenanceDiagnose'
  | 'processList'
  | 'processPin'
  | 'processEndPlan'
  | 'alertsSettings'
  | 'alertsSave'
  | 'insightsOverview'
  | 'downloadsTrashPlan'
> & {
  /** Preview hook: sustained-CPU alerts are on (the status fixture raises one sample alert). */
  __alertsOn(): boolean;
};

type MakePlan = (kind: PlanDetail['kind'], mode: Mode | null, items: PlanItem[], extra?: Partial<PlanDetail>, script?: Scripted[], onDone?: (operationId: string, items: OperationItem[]) => void) => string;
type Seed = (planId: string, results: Scripted[], state: string, minutesAgo: number) => string;

const MiB = 1024n * 1024n;
const GiB = 1024n * MiB;
const HOME = 'C:\\Users\\Lin';
const LOCAL = `${HOME}\\AppData\\Local`;
const ROAMING = `${HOME}\\AppData\\Roaming`;
const err = (code: string, reason: string, message: string): IpcError => ({ code, reason, message });
const delay = (ms: number) => new Promise((r) => setTimeout(r, ms));
const iso = (minutes = 0) => new Date(Date.now() + minutes * 60_000).toISOString().replace(/\.\d+Z$/, 'Z');
const b = (n: bigint) => n.toString();
const V2 = { contract: 'plan/v2', scan_id: null, scan_revision: null, scope_id: null, scan_root_display: null, total_logical_bytes: '0', allocated_known_bytes: null } as const;

function row(action_id: string, name: string, path_display: string, item_kind: string, action_type: string, extra: Partial<PlanItem> = {}): PlanItem {
  return { action_id, name, path_display, logical_bytes: '0', allocated_bytes: null, candidate_id: null, original_path_display: null, quarantine_id: null, item_kind, action_type, ...extra };
}

// ---------------------------------------------------------------------------------------------
// Uninstall

function app(id: string, name: string, version: string | null, publisher: string | null, source: string, location: string | null, over: Partial<UninstallApp> = {}): UninstallApp {
  return {
    app_instance_id: id,
    name,
    version,
    publisher,
    source,
    location,
    last_used_at: null,
    protected_class: false,
    state: 'available',
    adapter: { adapter_id: 'windows.registered-uninstaller', display: `${name} 自带的卸载程序（保留它自己的界面）`, package_manager: false },
    reason: null,
    settings_uri: null,
    ...over,
  };
}

const APPS: UninstallApp[] = [
  app('app_7c1e0a9b2f41', 'Visual Studio Code', '1.105.1', 'Microsoft Corporation', 'registry_uninstall', `${LOCAL}\\Programs\\Microsoft VS Code`, {
    adapter: { adapter_id: 'windows.registered-uninstaller', display: 'unins000.exe（Visual Studio Code 自带的卸载程序）', package_manager: false },
  }),
  app('app_2b8d44c10e73', 'Google Chrome', '141.0.7390.66', 'Google LLC', 'registry_uninstall', 'C:\\Program Files\\Google\\Chrome\\Application', {
    adapter: { adapter_id: 'winget.uninstall', display: 'winget uninstall --id Google.Chrome --exact', package_manager: true },
  }),
  app('app_93f0d6e2a118', 'Python 3.12.7 (64-bit)', '3.12.7150.0', 'Python Software Foundation', 'msi', `${LOCAL}\\Programs\\Python\\Python312`, {
    adapter: { adapter_id: 'windows.msiexec-uninstall', display: 'msiexec.exe /x {6C5B3F2E-…}（Windows Installer）', package_manager: false },
  }),
  app('app_5a1f7b3c9d20', 'Claude Code', '2.0.14', 'Anthropic', 'npm_global', `${ROAMING}\\npm\\node_modules\\@anthropic-ai\\claude-code`, {
    last_used_at: iso(-60 * 26),
    adapter: { adapter_id: 'npm.uninstall-global', display: 'npm uninstall --global @anthropic-ai/claude-code', package_manager: true },
  }),
  app('app_e40c2d9f8a55', 'Windows Terminal', '1.22.12111.0', 'Microsoft Corporation', 'msix_store', 'C:\\Program Files\\WindowsApps\\Microsoft.WindowsTerminal_1.22.12111.0_x64__8wekyb3d8bbwe', {
    adapter: { adapter_id: 'windows.msix-remove-package', display: '移除当前用户的应用包（包管理器）', package_manager: true },
  }),
  app('app_0d6b51e7c3a9', '微信', '4.0.3.22', 'Tencent', 'registry_uninstall', null, {
    adapter: { adapter_id: 'windows.registered-uninstaller', display: 'Uninstall.exe（微信自带的卸载程序）', package_manager: false },
  }),
  app('app_b17a3e60f2c4', 'Notepad++', '8.7.9', 'Notepad++ Team', 'registry_uninstall', 'C:\\Program Files\\Notepad++', {
    adapter: { adapter_id: 'windows.registered-uninstaller', display: 'uninstall.exe（Notepad++ 自带的卸载程序）', package_manager: false },
  }),
  app('app_4c9e81a2d7b6', 'PDF Toolbox Lite', '3.1', 'Unknown Vendor', 'registry_uninstall', 'D:\\Tools\\PDFToolbox', {
    state: 'unverified',
    adapter: null,
    reason: '注册的卸载命令是脚本宿主（wscript.exe），无法验证要运行的程序。',
    settings_uri: 'ms-settings:appsfeatures',
  }),
  app('app_88f2c0b4e19d', 'CrowdStrike Falcon Sensor', '7.18.18905.0', 'CrowdStrike, Inc.', 'registry_uninstall', 'C:\\Program Files\\CrowdStrike', {
    state: 'unsupported',
    protected_class: true,
    adapter: null,
    reason: '终端安全软件：只能由管理员或厂商的官方流程卸载。',
    settings_uri: 'ms-settings:appsfeatures',
  }),
  app('app_61d0a7f3b8e2', 'Microsoft Visual C++ 2015-2022 Redistributable (x64)', '14.44.35211.0', 'Microsoft Corporation', 'registry_uninstall', null, {
    state: 'unsupported',
    protected_class: true,
    adapter: null,
    reason: '共享运行库：其他应用依赖它，CleanManager 不提供卸载入口。',
    settings_uri: 'ms-settings:appsfeatures',
  }),
];

function lo(id: string, group: string, display: string, tier: Leftover['tier'], over: Partial<Leftover> = {}): Leftover {
  const selectable = tier === 'default_selected' || tier === 'review_only';
  return {
    object_id: id,
    kind: 'directory',
    group,
    display,
    evidence: ['与产品名完全相同的目录'],
    marker: null,
    tier,
    selected: tier === 'default_selected',
    selectable,
    why_not: tier === 'kept' ? 'kept' : tier === 'blocked' ? 'blocked' : null,
    warnings: [],
    logical_bytes: null,
    note: null,
    requires_admin: false,
    action: 'fs_trash',
    guidance: null,
    ...over,
  };
}

function nppLeftovers(): Leftovers {
  return {
    operation_id: '',
    app: { app_instance_id: 'app_b17a3e60f2c4', name: 'Notepad++', publisher: 'Notepad++ Team', source: 'registry_uninstall' },
    is_admin: false,
    default_selected: 2,
    apps: [],
    external_references: [`${HOME}\\.gitconfig`, `${HOME}\\Documents\\PowerShell\\Microsoft.PowerShell_profile.ps1`],
    groups: [
      {
        group: 'caches_logs',
        items: [
          lo('lo_3e1a90c2d7f4b615', 'caches_logs', `${LOCAL}\\Notepad++\\cache`, 'default_selected', { logical_bytes: b(212n * MiB) }),
          lo('lo_8b20f4e1a3c9d702', 'caches_logs', `${LOCAL}\\Notepad++\\logs`, 'default_selected', { logical_bytes: b(18n * MiB) }),
        ],
      },
      {
        group: 'bundle',
        items: [
          lo('lo_c4d1e08f2b7a3960', 'bundle', 'C:\\Program Files\\Notepad++\\plugins', 'review_only', {
            logical_bytes: b(41n * MiB),
            evidence: ['位于注册的安装位置下', '卸载程序运行后仍在'],
            requires_admin: true,
            selectable: false,
            why_not: 'needs_admin',
          }),
        ],
      },
      {
        group: 'startup',
        items: [
          lo('lo_71a3b9c0e2d4f815', 'startup', 'Notepad++ Updater（启动项，目标已不存在）', 'review_only', {
            kind: 'startup_entry',
            action: 'startup_toggle',
            evidence: ['Run 值指向安装目录中的 GUP.exe', '目标文件已不存在'],
          }),
        ],
      },
      {
        group: 'settings',
        items: [
          lo('lo_2f8e6a1b0c3d9e47', 'settings', `${ROAMING}\\Notepad++`, 'review_only', { logical_bytes: b(3n * MiB + 412_000n), evidence: ['%APPDATA% 下与产品名完全相同的目录'] }),
          lo('lo_9d4c2b7e1f0a3856', 'settings', 'HKCU\\Software\\Notepad++', 'review_only', {
            kind: 'registry_key',
            action: 'registry_backup_delete',
            evidence: ['HKCU\\Software\\<产品> 键'],
          }),
          lo('lo_5e7a1c3b9d2f0864', 'settings', `${ROAMING}\\Notepad++ Beta`, 'kept', {
            marker: 'shared_owner',
            evidence: ['另一个已安装实例 Notepad++ Beta 可能使用它'],
          }),
        ],
      },
      {
        group: 'personal_data',
        items: [
          lo('lo_0a9e3d7c1b5f2468', 'personal_data', `${ROAMING}\\Notepad++\\backup`, 'review_only', { logical_bytes: b(6n * MiB), note: '未保存的会话备份：删除后无法找回未保存的编辑。' }),
          lo('lo_6c2f8a0e4d1b9357', 'personal_data', `${ROAMING}\\Notepad++\\plugins\\config\\NppFTP\\credentials`, 'review_only', {
            kind: 'file',
            logical_bytes: '2048',
            warnings: ['敏感：名称包含“credential”（凭据、密钥或文档），选择前请确认'],
          }),
          lo('lo_d3b7e1f9c0a24685', 'personal_data', `${HOME}\\source\\npp-macros\\.git`, 'blocked', {
            evidence: ['数据目录中的 git 工作区：无法排除未提交的修改'],
          }),
        ],
      },
      {
        group: 'system',
        items: [
          lo('lo_1f6d9b3a7e0c2548', 'system', 'HKLM\\SOFTWARE\\Notepad++', 'review_only', {
            kind: 'registry_key',
            action: 'registry_backup_delete',
            requires_admin: true,
            selectable: false,
            why_not: 'needs_admin',
            evidence: ['HKLM\\SOFTWARE\\<产品> 键'],
          }),
          lo('lo_8e0c4a2d6f1b3579', 'system', 'NppUpdateService（服务）', 'review_only', {
            kind: 'service',
            action: 'manual',
            selectable: false,
            why_not: 'manual',
            guidance: '服务指向安装目录。CleanManager 不删除服务：在“服务”管理工具中停止并禁用它，或使用厂商的修复工具。',
          }),
        ],
      },
    ],
  };
}

const ESTIMATE: Record<string, Record<string, number>> = {
  default: { caches_logs: 3, settings: 2, personal_data: 1 },
};

// ---------------------------------------------------------------------------------------------
// Updates

function upd(id: string, name: string, current: string | null, available: string | null, channel: UpdateCheck['channel'], state: UpdateCheck['state'], route: UpdateCheck['route'], over: Partial<UpdateCheck> = {}): UpdateCheck {
  return { app_instance_id: id, name, current_version: current, available_version: available, channel, channel_id: null, state, reason: null, checked_at: null, route, official_url: null, ...over };
}

function updateResults(at: string | null): UpdateCheck[] {
  const t = { checked_at: at };
  return [
    upd('app_2b8d44c10e73', 'Google Chrome', '141.0.7390.66', '141.0.7390.108', 'winget', 'update_available', 'package_manager', { ...t, channel_id: 'Google.Chrome', official_url: 'https://www.google.com/chrome/' }),
    upd('app_93f0d6e2a118', 'Python 3.12.7 (64-bit)', '3.12.7150.0', '3.12.10150.0', 'winget', 'update_available', 'package_manager', { ...t, channel_id: 'Python.Python.3.12', official_url: 'https://www.python.org/downloads/' }),
    upd('app_b17a3e60f2c4', 'Notepad++', '8.7.9', '8.8.1', 'winget', 'update_available', 'package_manager', { ...t, channel_id: 'Notepad++.Notepad++' }),
    upd('app_7zip', '7-Zip 24.08 (x64)', '24.08', '25.01', 'scoop', 'update_available', 'package_manager', { ...t, channel_id: '7zip', reason: '与本地 bucket 副本比较（副本更新于 3 天前）' }),
    upd('app_7c1e0a9b2f41', 'Visual Studio Code', '1.105.1', null, 'self_updater', 'cannot_check', 'open_app', { ...t, reason: '应用自带更新器（Squirrel/Electron），其更新源无法验证：请在应用内更新。' }),
    upd('app_e40c2d9f8a55', 'Windows Terminal', '1.22.12111.0', null, 'microsoft_store', 'cannot_check', 'open_store', { ...t, channel_id: '9N0DX20HK701', reason: '商店自行检查其应用：打开商店的“下载和更新”。' }),
    upd('app_0d6b51e7c3a9', '微信', '4.0.3.22', null, null, 'cannot_check', 'none', { ...t, reason: '没有更新通道：不是通过 winget、商店或 Scoop 安装，也没有可验证的更新源。' }),
    upd('app_nodejs', 'Node.js', '22.11.0', '22.11.0', 'winget', 'up_to_date', 'none', { ...t, channel_id: 'OpenJS.NodeJS.LTS' }),
    upd('app_git', 'Git', '2.51.0', '2.51.0', 'winget', 'up_to_date', 'none', { ...t, channel_id: 'Git.Git' }),
  ];
}

// ---------------------------------------------------------------------------------------------
// Maintenance

function task(id: string, title: string, problem: string, impact: string, admin: boolean, applies: string, seconds: number, applicability: MaintenanceTask['applicability'], reason: string | null, over: Partial<MaintenanceTask> = {}): MaintenanceTask {
  return { task_id: id, task_version: '1.0.0', title, problem, impact, requires_admin: admin, applies_when: applies, estimated_seconds: seconds, applicability, applicability_reason: reason, user_protected: false, reversible: false, ...over };
}

const TASKS: MaintenanceTask[] = [
  task('windows.shell-restart', 'Restart Start menu and Search', 'Start menu or Search does not open, shows stale results or is slow to respond', 'Start and Search close and start again on next use; open windows and other apps are not touched', false, 'StartMenuExperienceHost or SearchHost runs in your session from the Windows system apps folder', 10, 'applicable', '2 hosts running in this session'),
  task('windows.icon-cache-rebuild', 'Rebuild icon and thumbnail cache', 'Icons or thumbnails are blank, wrong or outdated', 'Icons and thumbnails are drawn again on first view; only cache files not in use are removed', false, 'iconcache_*/thumbcache_* files exist in your Explorer cache folder', 5, 'applicable', '30 cache files, 11 in use by Explorer', { user_protected: true }),
  task('windows.dns-flush', 'Flush DNS resolver cache', 'Sites resolve to old addresses after a network or DNS change', 'The first lookup of each name is slightly slower once', false, 'The Windows DNS client interface is present', 1, 'applicable', null),
  task('windows.trim-when-due', 'Retrim solid-state drives when due', 'An SSD has not been retrimmed for a long time (scheduled optimization did not run)', 'Short disk activity; data is not moved. Runs only on SSD volumes whose last optimization is overdue', true, 'A fixed SSD volume with TRIM enabled was last optimized more than 30 days ago (or never)', 60, 'not_needed', 'C: was optimized 6 days ago'),
  task('windows.image-health-check', 'Check system image health (read-only)', 'Windows updates or features fail and component store corruption is suspected', 'Read-only check of the component store; nothing is repaired automatically', true, 'The Windows servicing (DISM) interface is present', 20, 'applicable', null),
];

const PROHIBITED: Prohibited[] = [
  { id: 'registry-optimizer', title: 'Registry cleaning or "optimization"', reason: 'Removing or rewriting registry data has no measurable benefit and can break applications or Windows; there is no task for it' },
  { id: 'performance-privacy-tweaks', title: 'Registry performance or privacy tweaks', reason: 'Undocumented setting changes have unprovable effects and are hard to undo; change settings in Windows Settings instead' },
  { id: 'service-disable', title: 'Disabling or deleting system services', reason: 'Disabled services break updates, security and features in ways that show up much later' },
  { id: 'search-indexing-disable', title: 'Turning off Windows Search indexing', reason: 'Search, Outlook and Settings depend on the index; manage indexed locations in Settings > Privacy & security > Searching Windows' },
  { id: 'system-debloat', title: 'System slimming or component removal', reason: 'Removing Windows components or built-in apps breaks servicing and updates; uninstall apps you chose to install from the Apps page' },
  { id: 'memory-release', title: 'Memory "release" or RAM boosting', reason: 'Windows manages memory itself; forcing pages out only makes the next use slower. The diagnosis shows real memory pressure instead' },
  { id: 'pagefile-tweak', title: 'Page file or virtual memory tweaks', reason: 'Wrong page file sizes cause crashes and lost crash dumps; Windows sizes it automatically' },
  { id: 'rebuild-app-registrations', title: 'Re-registering all apps or rebuilding app registration databases', reason: 'Mass re-registration can leave apps unusable and cannot be undone; repair one app from Settings > Apps instead' },
  { id: 'ssd-full-defrag', title: 'Full defragmentation of SSDs', reason: 'SSDs only need retrim, which the TRIM task does when due; full defragmentation adds wear without benefit' },
  { id: 'one-click-speedup', title: 'One-click speed-up', reason: 'There is no honest percentage to promise; each task here says what problem it solves and reports what actually happened' },
];

const KEYWORDS: Record<string, string[]> = {
  'registry-optimizer': ['registry', '注册表', 'regedit', '注册表优化', '注册表清理'],
  'performance-privacy-tweaks': ['tweak', '调优', 'telemetry', '遥测', '隐私', '性能调优', '组策略'],
  'service-disable': ['service', '服务', '禁用服务', 'sysmain'],
  'search-indexing-disable': ['indexing', '索引', '搜索索引', '关闭搜索'],
  'system-debloat': ['debloat', '精简', '组件移除', '删除自带'],
  'memory-release': ['memory', '内存', 'ram', '内存释放', '清理内存'],
  'pagefile-tweak': ['pagefile', '页面文件', '虚拟内存', 'swap'],
  'rebuild-app-registrations': ['re-register', '重新注册', '应用注册', '重建数据库'],
  'ssd-full-defrag': ['defrag', '碎片整理'],
  'one-click-speedup': ['speed up', '一键加速', '加速', 'boost', '提速'],
  'windows.shell-restart': ['start menu', '开始菜单', 'search', '搜索', 'shell'],
  'windows.icon-cache-rebuild': ['icon', '图标', 'thumbnail', '缩略图', 'cache', '缓存'],
  'windows.dns-flush': ['dns', '域名', '解析', 'network', '网络'],
  'windows.trim-when-due': ['trim', 'ssd', '固态', '优化驱动器'],
  'windows.image-health-check': ['dism', 'checkhealth', '系统映像', '组件存储', 'sfc', '损坏'],
};

const OUTCOME_SCRIPT: Record<string, { outcome: TaskOutcome; code: string; reason: string }> = {
  'windows.shell-restart': { outcome: 'applied', code: 'restarted', reason: 'StartMenuExperienceHost and SearchHost closed; Windows starts them on next use' },
  'windows.icon-cache-rebuild': { outcome: 'attention', code: 'files_in_use', reason: '19 cache files removed; 11 in use by Explorer were kept (sign out and run again)' },
  'windows.dns-flush': { outcome: 'applied', code: 'flushed', reason: 'DNS resolver cache flushed' },
  'windows.trim-when-due': { outcome: 'unchanged', code: 'not_due', reason: 'C: was optimized 6 days ago; retrim is not due' },
  'windows.image-health-check': { outcome: 'skipped', code: 'elevation_declined', reason: 'administrator authorization was declined (the Windows administrator prompt was declined); not run' },
};

// ---------------------------------------------------------------------------------------------
// Processes

function proc(pid: number, name: string, image: string | null, cpu: number | null, mem: bigint | null, over: Partial<ProcessRow> = {}): ProcessRow {
  return {
    pid,
    parent_pid: 1180,
    parent_name: 'explorer.exe',
    name,
    image_path: image,
    started_at: '133730000000000000',
    started_at_utc: '2024-10-10T02:13:20Z',
    cpu_percent: cpu,
    memory_bytes: mem === null ? null : b(mem),
    disk_bytes_per_sec: cpu === null ? null : Math.round((cpu * 40_000) % 900_000),
    network_bytes_per_sec: null,
    signature: { state: 'signed', publisher: 'Microsoft Corporation', reason: null },
    owner_app: null,
    identified: true,
    purpose: null,
    unidentified_reason: null,
    ai_diagnosis: null,
    pinned: false,
    end_capability: { state: 'available', protection: null, reason: 'Ends only through a reviewed plan for this one process' },
    notes: [],
    ...over,
  };
}

const UNIDENTIFIED_HINT = 'Purpose not identified locally. You can ask the optional AI diagnosis about this process; it shows exactly what would be sent (name, path, signer, parent) before sending';

const PROCS: ProcessRow[] = [
  proc(14820, 'Code.exe', `${LOCAL}\\Programs\\Microsoft VS Code\\Code.exe`, 18.4, 1n * GiB + 210n * MiB, { owner_app: 'Visual Studio Code', purpose: 'Visual Studio Code 编辑器窗口' }),
  proc(9324, 'chrome.exe', 'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe', 6.2, 812n * MiB, { owner_app: 'Google Chrome', purpose: 'Chrome 浏览器进程', signature: { state: 'signed', publisher: 'Google LLC', reason: null } }),
  proc(2040, 'MsMpEng.exe', 'C:\\ProgramData\\Microsoft\\Windows Defender\\Platform\\4.18.25080.5-0\\MsMpEng.exe', 3.1, 296n * MiB, {
    parent_name: 'services.exe',
    purpose: 'Microsoft Defender 防病毒服务',
    end_capability: { state: 'unsupported', protection: 'security_software', reason: "Security software is never ended from CleanManager; use the product's own console" },
  }),
  proc(1180, 'explorer.exe', 'C:\\Windows\\explorer.exe', 1.4, 214n * MiB, { parent_name: 'userinit.exe', purpose: 'Windows 资源管理器（任务栏、桌面与文件窗口）', signature: { state: 'catalog_signed', publisher: 'Microsoft Windows', reason: null } }),
  proc(712, 'csrss.exe', null, null, null, {
    parent_name: null,
    parent_pid: null,
    purpose: 'Windows 客户端/服务器运行时（系统关键进程）',
    signature: { state: 'unknown', publisher: null, reason: 'access denied (protected process)' },
    end_capability: { state: 'unsupported', protection: 'critical_system', reason: 'Critical Windows process: ending it stops or restarts the system' },
    notes: ['受保护进程：路径、内存与 CPU 读取被拒绝，显示为不可用而不是 0'],
  }),
  proc(18872, 'node.exe', 'C:\\Program Files\\nodejs\\node.exe', 42.7, 640n * MiB, { parent_name: 'Code.exe', parent_pid: 14820, owner_app: 'Node.js', purpose: 'Node.js 运行时（由 Code.exe 启动）', signature: { state: 'signed', publisher: 'OpenJS Foundation', reason: null } }),
  proc(21544, 'helperd.exe', `${LOCAL}\\Temp\\hx41\\helperd.exe`, 11.9, 88n * MiB, {
    identified: false,
    purpose: null,
    unidentified_reason: '位于临时文件夹，未签名，没有所属应用',
    signature: { state: 'unsigned', publisher: null, reason: null },
    ai_diagnosis: { available: true, hint: UNIDENTIFIED_HINT },
  }),
  proc(17260, 'OneDrive.exe', `${LOCAL}\\Microsoft\\OneDrive\\OneDrive.exe`, 0.4, 120n * MiB, { owner_app: 'Microsoft OneDrive', purpose: 'OneDrive 同步客户端' }),
  proc(6120, 'svchost.exe', 'C:\\Windows\\System32\\svchost.exe', 0.9, 42n * MiB, { parent_name: 'services.exe', purpose: 'Windows 服务宿主', signature: { state: 'catalog_signed', publisher: 'Microsoft Windows', reason: null } }),
  proc(25510, 'Weixin.exe', 'C:\\Program Files\\Tencent\\Weixin\\Weixin.exe', 0.7, 356n * MiB, { owner_app: '微信', purpose: '微信客户端', signature: { state: 'signed', publisher: 'Tencent Technology (Shenzhen) Company Limited', reason: null } }),
  proc(30122, 'svc_updater.exe', 'D:\\Tools\\PDFToolbox\\svc_updater.exe', 0.2, 24n * MiB, {
    identified: false,
    unidentified_reason: '名称像系统组件，但位置和签名者不符',
    signature: { state: 'unsigned', publisher: null, reason: null },
    ai_diagnosis: { available: true, hint: UNIDENTIFIED_HINT },
  }),
];

// ---------------------------------------------------------------------------------------------
// Old downloads

const DOWNLOADS: DownloadItem[] = [
  ['ubuntu-24.04.1-desktop-amd64.iso', 6n * GiB + 120n * MiB, 214, true],
  ['DaVinci_Resolve_19.0_Windows.zip', 3n * GiB + 410n * MiB, 160, true],
  ['会议录像-2025-03-18.mp4', 1n * GiB + 900n * MiB, 210, false],
  ['VSCodeUserSetup-x64-1.96.2.exe', 98n * MiB, 280, true],
  ['dataset-train-v2.parquet', 870n * MiB, 133, false],
  ['WeChatSetup.exe', 214n * MiB, 101, true],
  ['旧版合同扫描件.pdf', 46n * MiB, 365, false],
  ['screenshots-2024.7z', 412n * MiB, 300, true],
].map(([name, size, age, inst], i) => ({
  item_id: `dl_${i}`,
  name: name as string,
  path_display: `${HOME}\\Downloads\\${name as string}`,
  logical_bytes: b(size as bigint),
  modified_at: iso(-(age as number) * 24 * 60),
  age_days: age as number,
  installer_like: inst as boolean,
}))
  .sort((a, c) => (BigInt(c.logical_bytes) > BigInt(a.logical_bytes) ? 1 : -1))
  .map((x, i) => ({ ...x, item_id: `dl_${i}` }));

export function mockSoftware(makePlan: MakePlan, seed: Seed): SoftwareBackend {
  let checksOn = true;
  let lastChecked: string | null = iso(-60 * 30);
  let results = updateResults(lastChecked);
  const pins = new Set<string>(['Code.exe']);
  let alerts: AlertSettings = { enabled: false, cpu_threshold_percent: 80, window_seconds: 120, cooldown_seconds: 1800 };
  const history: MaintenanceReport[] = [];
  const recent: UninstallRun[] = [];

  // A finished stage-1 uninstall whose rescan found leftovers: its leftovers can be reviewed now.
  const nppPlan = makePlan(
    'uninstall',
    null,
    [
      row('q0000000', 'Notepad++', 'C:\\Program Files\\Notepad++', 'app_quit', 'app_quit', { detail: { force: 'never' } }),
      row('u0000001', 'Notepad++', 'uninstall.exe（Notepad++ 自带的卸载程序）', 'uninstaller', 'tool_adapter', { detail: { adapter_id: 'windows.registered-uninstaller', program: 'C:\\Program Files\\Notepad++\\uninstall.exe', timeout_seconds: 3600 } }),
    ],
    { ...V2, plan_kind_v2: 'uninstall', kind_detail: { stage: 'uninstall', app: { name: 'Notepad++', publisher: 'Notepad++ Team', version: '8.7.9', source: 'registry_uninstall', adapter: 'uninstall.exe', signer: 'Notepad++ Team' }, may_end_processes: false } },
  );
  const nppOp = seed(
    nppPlan,
    [
      { status: 'succeeded', code: 'not_running', message: 'Notepad++ is not running' },
      { status: 'succeeded', code: 'leftovers_remain', message: 'uninstaller exited with code 0 after 8214 ms; rescan: the install folder still present: partial' },
    ],
    'partial',
    60 * 3,
  );
  recent.push({ operation_id: nppOp, plan_id: nppPlan, app_name: 'Notepad++', app_count: 1, stage: 'uninstall', state: 'partial', leftovers_reviewable: true, started_at: iso(-60 * 3), finished_at: iso(-60 * 3 + 2) });
  /** Which applications each stage-1 preview run held, and whether each one's uninstaller succeeded (UNI-11). */
  const runApps = new Map<string, { app: UninstallApp; ok: boolean }[]>([[nppOp, [{ app: APPS.find((a) => a.app_instance_id === 'app_b17a3e60f2c4')!, ok: true }]]]);

  const uninstallScript = (a: UninstallApp): Scripted[] =>
    a.app_instance_id === 'app_7c1e0a9b2f41'
      ? [
          { status: 'failed', code: 'still_running', message: 'still running after the close request: Code.exe (pid 14820); nothing was uninstalled' },
          { status: 'skipped', code: 'app_still_running', message: 'the application did not quit; nothing was uninstalled or deleted' },
        ]
      : [
          { status: 'succeeded', code: 'closed', message: `${a.name} closed after a close request` },
          { status: 'succeeded', code: 'leftovers_remain', message: 'uninstaller exited with code 0 after 12040 ms; rescan: the install folder still present: partial' },
        ];

  return {
    __alertsOn: () => alerts.enabled,

    async uninstallList() {
      await delay(260);
      return { supported: true, is_admin: false, items: APPS.map((a) => ({ ...a })), issues: [{ code: 'unreadable_key', scope_display: 'HKLM\\SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{0F…}', message: '1 registration could not be read; it is not listed' }], recent: recent.map((r) => ({ ...r })) };
    },
    async uninstallPlan(ids) {
      await delay(420);
      // UNI-11: one plan for every selected app; refused ones (protected, no verifiable route) stay out, as on their own.
      const chosen: UninstallApp[] = [];
      const refused: { app_instance_id: string; name: string | null; reason: string; message: string }[] = [];
      for (const id of ids) {
        const a = APPS.find((x) => x.app_instance_id === id);
        if (!a) refused.push({ app_instance_id: id, name: null, reason: 'invalid_argument', message: `no installed application ${id}` });
        else if (a.state !== 'available') refused.push({ app_instance_id: id, name: a.name, reason: a.state === 'unsupported' ? 'policy_denied' : 'unsupported', message: `${a.reason ?? ''} (ms-settings:appsfeatures)` });
        else chosen.push(a);
      }
      if (!chosen.length) {
        if (refused.length === 1) throw err(refused[0].reason, refused[0].reason, refused[0].message);
        throw err('invalid_argument', 'invalid_argument', `none of the selected applications can be uninstalled here: ${refused.map((r) => `${r.name ?? r.app_instance_id}: ${r.message}`).join('; ')}`);
      }
      const estimate = ESTIMATE.default;
      const total: Record<string, number> = {};
      for (const _ of chosen) for (const [g, n] of Object.entries(estimate)) total[g] = (total[g] ?? 0) + n;
      const items: PlanItem[] = [];
      const script: Scripted[] = [];
      const apps = chosen.map((a, n) => {
        // Ids sort in plan order: the first app keeps q0000000/u0000001, the n-th uses u{n:03}0000/u{n:03}0001.
        const q = n === 0 ? 'q0000000' : `u${String(n).padStart(3, '0')}0000`;
        const u = `u${String(n).padStart(3, '0')}0001`;
        items.push(row(q, a.name, a.location ?? a.name, 'app_quit', 'app_quit', { detail: { force: 'never', app_instance_id: a.app_instance_id } }));
        items.push(row(u, a.name, a.adapter!.display, 'uninstaller', 'tool_adapter', { detail: { adapter_id: a.adapter!.adapter_id, program: a.location ?? '', timeout_seconds: 3600, app_instance_id: a.app_instance_id } }));
        script.push(...uninstallScript(a));
        return { app_instance_id: a.app_instance_id, name: a.name, publisher: a.publisher, version: a.version, source: a.source, adapter: a.adapter!.display, signer: a.publisher, action_ids: [q, u] };
      });
      const names = chosen.map((a) => a.name).join('、');
      const batchApps = chosen.map((a) => ({ app_instance_id: a.app_instance_id, name: a.name, leftover_estimate: estimate }));
      const first = apps[0];
      const planId = makePlan(
        'uninstall',
        null,
        items,
        {
          ...V2,
          plan_kind_v2: 'uninstall',
          kind_detail: { stage: 'uninstall', app: first, apps, may_end_processes: false, noted: { app_name: names, leftover_estimate: total, apps: batchApps, refused } },
        },
        script,
        (op) => {
          const ran = chosen.map((a) => ({ app: a, ok: a.app_instance_id !== 'app_7c1e0a9b2f41' }));
          runApps.set(op, ran);
          const ok = ran.filter((r) => r.ok).length;
          recent.unshift({ operation_id: op, plan_id: planId, app_name: names, app_count: chosen.length, stage: 'uninstall', state: ok ? 'partial' : 'failed', leftovers_reviewable: ok > 0, started_at: iso(0), finished_at: iso(0) });
        },
      );
      return { plan_id: planId, leftover_estimate: total, apps: batchApps, refused };
    },
    async uninstallLeftovers(opId) {
      await delay(520);
      if (!recent.some((r) => r.operation_id === opId && r.stage === 'uninstall')) throw err('invalid_argument', 'invalid_argument', `operation ${opId} is not an application uninstall`);
      const ran = runApps.get(opId) ?? [];
      if (!ran.some((r) => r.ok)) throw err('invalid_argument', 'invalid_argument', `the uninstall in operation ${opId} did not complete; its leftovers are not reviewed or removed while the application is still installed`);
      // The preview has one set of leftover objects: it belongs to the first application whose uninstaller succeeded.
      const l = nppLeftovers();
      const owner = ran.find((r) => r.ok)!.app;
      const apps = ran.map((r) => {
        const app = { app_instance_id: r.app.app_instance_id, name: r.app.name, publisher: r.app.publisher, source: r.app.source };
        const mine = r.app === owner;
        return r.ok
          ? { app, status: 'partial' as const, code: 'leftovers_remain', message: 'uninstaller exited with code 0; rescan: the install folder still present: partial', leftovers_reviewable: true, groups: mine ? l.groups : [], default_selected: mine ? l.default_selected : 0, external_references: mine ? l.external_references : [] }
          : { app, status: 'skipped' as const, code: 'still_running', message: 'still running after the close request; nothing was uninstalled', leftovers_reviewable: false, groups: [], default_selected: 0, external_references: [] };
      });
      return { ...l, operation_id: opId, app: ran.length === 1 ? apps[0].app : null, apps };
    },
    async uninstallLeftoversPlan(opId, ids) {
      await delay(640);
      const all = nppLeftovers().groups.flatMap((g) => g.items);
      const chosen = ids.map((id) => all.find((x) => x.object_id === id));
      if (chosen.some((c) => !c)) throw err('invalid_argument', 'invalid_argument', 'not a leftover of this operation');
      const bad = chosen.find((c) => !c!.selectable);
      if (bad) throw err('policy_denied', 'policy_denied', `${bad.display} is ${bad.tier}`);
      let n = 0;
      const items: PlanItem[] = chosen.map((c) => {
        n += 1;
        if (c!.action === 'registry_backup_delete') return row(`k${String(n).padStart(7, '0')}`, c!.display.split('\\').pop()!, c!.display, 'registry_key', 'registry_backup_delete', { scope: c!.display.startsWith('HKLM') ? 'machine' : 'user', detail: { owner_evidence: c!.evidence.join('; '), backup_required: true } });
        if (c!.action === 'startup_toggle') return row(`s${String(n).padStart(7, '0')}`, 'Notepad++ Updater', 'C:\\Program Files\\Notepad++\\updater\\GUP.exe', 'startup_item', 'startup_remove_with_backup', { scope: 'user' });
        return row(`f${String(n).padStart(4, '0')}0000`, c!.display.split('\\').pop()!, c!.display, 'directory', 'fs_trash', { logical_bytes: c!.logical_bytes ?? '0', detail: { leftover: `${c!.group}: ${c!.display}` } });
      });
      const run = recent.find((r) => r.operation_id === opId);
      const owner = (runApps.get(opId) ?? []).filter((r) => r.ok).map((r) => r.app.name).join('、');
      const keys = items.filter((i) => i.action_type === 'registry_backup_delete').length;
      const planId = makePlan(
        'uninstall_leftovers',
        null,
        items,
        { ...V2, plan_kind_v2: 'uninstall', total_logical_bytes: items.reduce((a, i) => a + BigInt(i.logical_bytes), 0n).toString(), kind_detail: { stage: 'leftovers', app: null, registry_keys: keys, noted: { app_name: owner || run?.app_name || 'Notepad++', operation_id: opId } } },
        items.map((i) => (i.action_type === 'registry_backup_delete' ? { status: 'succeeded', code: 'done', message: 'deleted after its backup was stored' } : { status: 'succeeded', code: 'done', message: '' })),
      );
      return { plan_id: planId };
    },
    async uninstallRestorePlan() {
      await delay(200);
      throw err('invalid_argument', 'invalid_argument', 'the operation recorded no registry backup that can still be restored');
    },
    async maintenanceRestorePlan() {
      await delay(200);
      throw err('invalid_argument', 'invalid_argument', 'the operation recorded no setting change that can still be restored');
    },

    async updatesList(check) {
      await delay(check ? 1800 : 160);
      if (check && checksOn) {
        lastChecked = iso(0);
        results = updateResults(lastChecked);
      }
      const items = checksOn || lastChecked ? results.map((r) => ({ ...r })) : results.map((r) => ({ ...r, state: 'not_checked' as const, checked_at: null }));
      const count = (s: UpdateCheck['state']) => items.filter((i) => i.state === s).length;
      return {
        supported: true,
        checks_enabled: checksOn,
        checked: check && checksOn,
        checked_at: lastChecked,
        counts: { update_available: count('update_available'), up_to_date: count('up_to_date'), cannot_check: count('cannot_check'), not_checked: count('not_checked') },
        items,
      };
    },
    async updatesSetChecks(enabled) {
      await delay(120);
      checksOn = enabled;
      return { checks_enabled: checksOn, last_checked_at: lastChecked };
    },
    async updatesPlan(ids) {
      await delay(500);
      const routed: UpdateRouted[] = [];
      const planned: PlanItem[] = [];
      for (const id of ids) {
        const r = results.find((x) => x.app_instance_id === id);
        if (!r) throw err('invalid_argument', 'unknown_app', `unknown app instance ${id}`);
        if (r.state !== 'update_available') {
          routed.push({ app_instance_id: id, name: r.name, route: r.route === 'package_manager' ? 'none' : r.route, reason: r.reason ?? 'no update is available', url: r.route === 'open_store' ? 'ms-windows-store://downloadsandupdates' : r.official_url });
          continue;
        }
        if (r.channel === 'scoop') {
          routed.push({ app_instance_id: id, name: r.name, route: 'open_official_page', reason: 'the installed app has no verifiable signer, so a downloaded package cannot be checked against it', url: 'https://www.7-zip.org/' });
          continue;
        }
        planned.push(
          row(`a${String(planned.length + 1).padStart(2, '0')}`, r.name, `${r.current_version} → ${r.available_version}`, 'app_update', 'tool_adapter', {
            detail: { channel: r.channel, package_id: r.channel_id, command: `winget.exe upgrade --id ${r.channel_id} --exact --version ${r.available_version}`, installed_signer: r.name === 'Google Chrome' ? 'Google LLC' : r.name.startsWith('Python') ? 'Python Software Foundation' : 'Notepad++ Team', official_url: r.official_url, timeout_seconds: 900 },
          }),
        );
      }
      if (!planned.length) return { plan_id: null, planned: 0, routed };
      const script: Scripted[] = planned.map((p) =>
        p.name === 'Google Chrome'
          ? { status: 'skipped', code: 'app_running', message: 'Google Chrome is running; close it and apply the plan again (CleanManager does not close apps to update them)' }
          : { status: 'succeeded', code: 'updated', message: `updated to ${p.path_display.split('→ ')[1]}` },
      );
      const planId = makePlan('update', null, planned, { ...V2, plan_kind_v2: 'update', kind_detail: { noted: { routed } } }, script);
      return { plan_id: planId, planned: planned.length, routed };
    },
    async openSystemPage() {
      await delay(60);
      return true;
    },

    async maintenanceCatalog() {
      await delay(380);
      return {
        supported: true,
        items: TASKS.map((t) => ({ ...t })),
        default_selection: TASKS.filter((t) => !t.user_protected && (t.applicability === 'applicable' || t.applicability === 'unknown')).map((t) => t.task_id),
        prohibited: PROHIBITED,
        is_admin: false,
        elevation: 'one_prompt_per_run',
        observed_at: iso(0),
      };
    },
    async maintenanceSearch(query) {
      await delay(90);
      const q = query.trim().toLowerCase();
      if ([...q].length < 2) return { query, task_ids: TASKS.map((t) => t.task_id), prohibited: [] };
      const hit = (id: string, words: string[]) => [id, ...words].some((w) => w.toLowerCase().includes(q) || q.includes(w.toLowerCase()));
      return {
        query,
        task_ids: TASKS.filter((t) => hit(t.title, KEYWORDS[t.task_id] ?? [])).map((t) => t.task_id),
        prohibited: PROHIBITED.filter((p) => hit(p.title, KEYWORDS[p.id] ?? [])),
      };
    },
    async maintenancePlan(ids) {
      await delay(300);
      const chosen = ids.map((id) => TASKS.find((t) => t.task_id === id));
      if (chosen.some((c) => !c)) throw err('invalid_argument', 'refused', 'unknown task');
      const prot = chosen.find((c) => c!.user_protected);
      if (prot) throw err('policy_denied', 'protected', `task ${prot.task_id} is protected; remove the protection entry first`);
      const items = chosen.map((t, i) =>
        row(`m${String(i + 1).padStart(2, '0')}`, t!.title, `${t!.task_id}@${t!.task_version}`, 'maintenance_task', 'maintenance_task', { scope: t!.requires_admin ? 'machine' : 'user', detail: { task_id: t!.task_id, task_version: t!.task_version, requires_admin: t!.requires_admin } }),
      );
      const runs = chosen.map((t) => ({ t: t!, r: OUTCOME_SCRIPT[t!.task_id] }));
      const status = (o: TaskOutcome): Scripted['status'] => (o === 'applied' || o === 'unchanged' || o === 'attention' ? 'succeeded' : o === 'failed' ? 'failed' : 'skipped');
      const script: Scripted[] = runs.map(({ r }) => ({ status: status(r.outcome), code: r.outcome, message: `${r.code}: ${r.reason}` }));
      const admin = chosen.filter((c) => c!.requires_admin).length;
      const planId = makePlan('maintenance', null, items, { ...V2, plan_kind_v2: 'maintenance', requires_admin: admin > 0, kind_detail: { admin_tasks: admin, is_admin: false } }, script, (op) => {
        const counts = { applied: 0, unchanged: 0, skipped: 0, unavailable: 0, attention: 0, failed: 0 } as Record<TaskOutcome, number>;
        runs.forEach(({ r }) => (counts[r.outcome] += 1));
        history.unshift({
          operation_id: op,
          plan_id: planId,
          plan_digest: '',
          entry: 'desktop',
          principal: 'user:DESKTOP\\Lin',
          elevation: admin ? 'declined' : 'not_needed',
          started_at: iso(0),
          finished_at: iso(0),
          items: runs.map(({ t, r }) => ({ task_id: t.task_id, task_version: t.task_version, requires_admin: t.requires_admin, outcome: r.outcome, code: r.code, reason: r.reason, elevated: false, started_at: iso(0), finished_at: iso(0) })),
          counts,
          all_clear: counts.failed === 0 && counts.attention === 0,
          undo: 'unavailable',
        });
      });
      return { plan_id: planId };
    },
    async maintenanceHistory() {
      await delay(120);
      if (!history.length) {
        history.push({
          operation_id: 'op_5d1c0e7a92b3f4c1',
          plan_id: 'mt_desk_4242_1759990000000',
          plan_digest: '',
          entry: 'cli',
          principal: 'user:DESKTOP\\Lin',
          elevation: 'granted',
          started_at: iso(-60 * 24 * 6),
          finished_at: iso(-60 * 24 * 6 + 1),
          items: [
            { task_id: 'windows.dns-flush', task_version: '1.0.0', requires_admin: false, outcome: 'applied', code: 'flushed', reason: 'DNS resolver cache flushed', elevated: false, started_at: iso(-60 * 24 * 6), finished_at: iso(-60 * 24 * 6) },
            { task_id: 'windows.image-health-check', task_version: '1.0.0', requires_admin: true, outcome: 'unchanged', code: 'healthy', reason: 'No component store corruption detected', elevated: true, started_at: iso(-60 * 24 * 6), finished_at: iso(-60 * 24 * 6 + 1) },
          ],
          counts: { applied: 1, unchanged: 1, skipped: 0, unavailable: 0, attention: 0, failed: 0 },
          all_clear: true,
          undo: 'unavailable',
        });
      }
      return { items: history.map((h) => ({ ...h })), undo: 'unavailable' };
    },
    async maintenanceDiagnose() {
      await delay(2100);
      return {
        supported: true,
        sampled_at: iso(0),
        window_seconds: 2,
        read_only: true,
        findings: [
          { code: 'runaway_process', severity: 'warning', subject: 'node.exe', detail: 'node.exe (PID 18872) used 97% of one CPU in the window and averaged 64% since it started 42 minutes ago', process_id: 18872, duration_seconds: 2520, next_step: 'status.processes' },
          { code: 'low_disk_space', severity: 'warning', subject: 'C:', detail: 'C: has 56.2 GiB free (11.8%)', process_id: null, duration_seconds: null, next_step: 'analyze' },
          { code: 'attached_disk_image', severity: 'info', subject: `${HOME}\\Downloads\\ubuntu-24.04.1-desktop-amd64.iso`, detail: 'a disk image is still attached; eject it in Explorer when done', process_id: null, duration_seconds: null, next_step: 'explorer' },
        ],
        top_processes: [
          { process_id: 18872, name: 'node.exe', image: 'C:\\Program Files\\nodejs\\node.exe', cpu_core_percent: 97.2, working_set_bytes: b(640n * MiB) },
          { process_id: 14820, name: 'Code.exe', image: `${LOCAL}\\Programs\\Microsoft VS Code\\Code.exe`, cpu_core_percent: 21.5, working_set_bytes: b(1n * GiB) },
          { process_id: 21544, name: 'helperd.exe', image: `${LOCAL}\\Temp\\hx41\\helperd.exe`, cpu_core_percent: 12.0, working_set_bytes: b(88n * MiB) },
        ],
        memory_load_percent: 71,
        commit_used_percent: 58.4,
      };
    },

    async processList(sort: ProcessSort, search, limit) {
      await delay(900);
      const q = search?.trim().toLowerCase() ?? '';
      const match = (p: ProcessRow) =>
        !q || [p.name, String(p.pid), p.image_path ?? '', p.signature?.publisher ?? '', p.owner_app ?? '', p.purpose ?? ''].some((x) => x.toLowerCase().includes(q));
      const key = (p: ProcessRow): number | string | null =>
        sort === 'cpu' ? p.cpu_percent : sort === 'memory' ? (p.memory_bytes === null ? null : Number(p.memory_bytes)) : sort === 'disk' ? p.disk_bytes_per_sec : sort === 'network' ? p.network_bytes_per_sec : p.name.toLowerCase();
      const rows = PROCS.map((p) => ({ ...p, pinned: pins.has(p.name) }));
      const pinned = rows.filter((p) => p.pinned);
      const rest = rows.filter((p) => !p.pinned && match(p));
      const cmp = (a: ProcessRow, c: ProcessRow) => {
        const x = key(a);
        const y = key(c);
        if (x === null && y === null) return a.pid - c.pid;
        if (x === null) return 1;
        if (y === null) return -1;
        if (typeof x === 'string') return x < (y as string) ? -1 : 1;
        return (y as number) - x;
      };
      const items = [...pinned.sort(cmp), ...rest.sort(cmp)].slice(0, limit);
      return {
        supported: true,
        sampled_at: iso(0),
        window_ms: 1000,
        sort,
        search,
        total: PROCS.length,
        matched: pinned.length + rest.length,
        unidentified: items.filter((i) => !i.identified).length,
        items,
        notes: ["disk_bytes_per_sec is the process's read+write transfer rate (file, device and network I/O counters); Task Manager's Disk column uses ETW tracing and can differ", 'Per-process network rates need ETW kernel tracing (administrator); not sampled'],
        pins: [...pins],
      };
    },
    async processPin(name, pinned) {
      await delay(80);
      if (pinned) pins.add(name);
      else pins.delete(name);
      return [...pins].sort();
    },
    async processEndPlan(pid) {
      await delay(300);
      const p = PROCS.find((x) => x.pid === pid);
      if (!p) throw err('invalid_argument', 'not_running', `process ${pid} is no longer running; refresh the list`);
      if (p.end_capability.state !== 'available') throw err('policy_denied', 'protected', p.end_capability.reason);
      const planId = makePlan(
        'process',
        null,
        [row('e01', `${p.name} (PID ${p.pid})`, p.image_path ?? '', 'process', 'process_end', { detail: { pid: p.pid, started_at: p.started_at, image_bound: true } })],
        { ...V2, plan_kind_v2: 'process', kind_detail: { noted: { pid: p.pid, name: p.name, purpose: p.purpose, unidentified_reason: p.unidentified_reason } } },
        [{ status: 'succeeded', code: 'ended', message: `${p.name} (PID ${p.pid}) was ended; unsaved work in it is lost` }],
      );
      return { plan_id: planId };
    },
    async alertsSettings() {
      await delay(60);
      return { ...alerts };
    },
    async alertsSave(s) {
      await delay(120);
      if (!(s.cpu_threshold_percent >= 1 && s.cpu_threshold_percent <= 100)) throw err('invalid_argument', 'invalid_argument', 'cpu threshold must be 1..=100 percent');
      if (!(s.window_seconds >= 10 && s.window_seconds <= 3600)) throw err('invalid_argument', 'invalid_argument', 'window must be 10..=3600 seconds');
      if (!(s.cooldown_seconds >= 60 && s.cooldown_seconds <= 86400)) throw err('invalid_argument', 'invalid_argument', 'cooldown must be 60..=86400 seconds');
      alerts = { ...s };
      return { ...alerts };
    },

    async insightsOverview(days) {
      await delay(700);
      const total = DOWNLOADS.reduce((a, d) => a + BigInt(d.logical_bytes), 0n);
      const card = (id: string, kind: string, title: string, size: bigint | null, count: number | null, note: string | null = null) => ({ card_id: id, kind, title, logical_bytes: size === null ? null : b(size), count, size_note: note, observed_at: iso(-2) });
      return {
        supported: true,
        generated_at: iso(0),
        cards: [
          card('c_vm', 'virtual_machines', 'Virtual machines and containers', 64n * GiB + 512n * MiB, 2),
          card('c_old', 'old_downloads', `Downloads older than ${days} days`, total, DOWNLOADS.length),
          card('c_sys', 'system_files', 'System files', 19n * GiB, 3),
          card('c_models', 'model_libraries', 'Model libraries', 11n * GiB + 300n * MiB, 4),
          card('c_dev', 'developer_tools', 'Developer tool caches', 7n * GiB + 220n * MiB, 5),
          card('c_store', 'component_store', 'Component store', null, null, 'Component store: reported by DISM only for administrators'),
          card('c_snap', 'snapshots', 'Restore points and snapshots', null, 3, 'Shadow storage: size not available without an administrator'),
        ],
        downloads: {
          root_display: `${HOME}\\Downloads`,
          older_than_days: days,
          observed_at: iso(-2),
          item_count: DOWNLOADS.length,
          total_logical_bytes: b(total),
          note: null,
          items: DOWNLOADS.map((d) => ({ ...d })),
        },
      };
    },
    async downloadsTrashPlan(ids) {
      await delay(900);
      const chosen = ids.map((id) => DOWNLOADS.find((d) => d.item_id === id));
      if (chosen.some((c) => !c)) throw err('invalid_argument', 'unknown_item', 'review old downloads again');
      const items = chosen.map((c, i) => row(`a${String(i + 1).padStart(2, '0')}`, c!.name, c!.path_display, 'file', 'fs_trash', { logical_bytes: c!.logical_bytes, allocated_bytes: c!.logical_bytes }));
      const planId = makePlan('manual_trash', 'trash', items, {
        ...V2,
        plan_kind_v2: 'manual_selection',
        scan_id: 's_desk_dl_4242_1',
        scan_revision: 1,
        scan_root_display: `${HOME}\\Downloads`,
        total_logical_bytes: items.reduce((a, i) => a + BigInt(i.logical_bytes), 0n).toString(),
        allocated_known_bytes: items.reduce((a, i) => a + BigInt(i.logical_bytes), 0n).toString(),
      });
      return { plan_id: planId };
    },
  };
}
