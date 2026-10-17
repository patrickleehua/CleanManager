// Words for 软件 › 卸载 / 更新, 优化 and 状态 › 进程. The host speaks protocol words (and the core's
// English catalog text); people read these. Unknown values fall back to what the host sent.

import { words } from './platform';
import type { Applicability, LeftoverTier, TaskOutcome, UpdateChannel, UpdateRoute, UpdateState } from './suiteTypes';
import type { Tier } from './types';

// ---------------------------------------------------------------------------------------------
// Uninstall

export const SOURCE_WORD: Record<string, string> = {
  registry_uninstall: '注册的卸载项',
  msi: 'Windows Installer（MSI）',
  msix_store: '商店 / MSIX 包',
  scoop: 'Scoop',
  npm_global: 'npm 全局包',
  app_bundle: '应用包（.app）',
  homebrew: 'Homebrew',
  app_store: 'Mac App Store',
  unknown: '来源未知',
};

export const UNINSTALL_STATE: Record<string, { label: string; tone: 'good' | 'warn' | 'muted' }> = {
  available: { label: '可卸载', tone: 'good' },
  unverified: { label: '无法验证', tone: 'warn' },
  unsupported: { label: '受保护', tone: 'muted' },
};

/** Leftover groups (UNI-05): name, what they are, and whether anything is preselected. */
export const LEFTOVER_GROUP: Record<string, { title: string; explain: string }> = {
  caches_logs: { title: '缓存与日志', explain: '应用运行时生成，可以重新生成。这一组默认选中，你可以取消。' },
  bundle: { title: '应用本体', explain: '安装目录中卸载程序没有删除的文件。' },
  startup: { title: '启动与后台项', explain: '指向已卸载程序的启动项。移除前先备份，可以还原。' },
  settings: { title: '设置与偏好', explain: '重新安装后会用到的设置。默认不选。' },
  personal_data: { title: '个人数据与模型', explain: '会话、备份、模型或文档。删除后可能无法找回，默认不选，请逐项确认。' },
  system: { title: '系统级位置', explain: '所有用户共用的位置与服务。需要管理员；服务只给出手动处理方法。' },
};

export const MARKER_WORD: Record<string, string> = {
  shared_owner: '另一个已安装的实例可能也在使用',
  owner_unknown: '只按名称相近匹配，无法证明属于这个应用',
  app_reappeared: '这个应用已经重新安装',
};

export function whyNotWord(k: string): string {
  const table: Record<string, string> = { kept: '保留：不能放入计划', blocked: '已阻止：只能查看', manual: '需要你手动处理', needs_admin: `需要${words().admin}` };
  return table[k] ?? k;
}

/** Leftover tiers on the shared three-tier badge (kept objects are shown as blocked). */
export function leftoverTier(t: LeftoverTier): Tier {
  return t === 'default_selected' ? 'default_selected' : t === 'review_only' ? 'review_only' : 'blocked';
}

export function leftoverAction(k: string): string {
  const table: Record<string, string> = { fs_trash: words().moveToBin, registry_backup_delete: '先备份，再删除注册表项', startup_toggle: '先备份，再移除启动项', manual: '只给出处理方法' };
  return table[k] ?? k;
}

// ---------------------------------------------------------------------------------------------
// Updates

export const UPDATE_STATE: Record<UpdateState, { label: string; tone: 'good' | 'warn' | 'muted' | 'busy' }> = {
  update_available: { label: '有可用更新', tone: 'busy' },
  up_to_date: { label: '已是最新', tone: 'good' },
  cannot_check: { label: '无法检查', tone: 'warn' },
  not_checked: { label: '未检查', tone: 'muted' },
};

export const CHANNEL_WORD: Record<UpdateChannel, string> = {
  winget: 'winget',
  microsoft_store: 'Microsoft Store',
  scoop: 'Scoop（本地 bucket）',
  self_updater: '应用自带更新器',
  sparkle: 'Sparkle 更新源',
  mac_app_store: 'Mac App Store',
  homebrew: 'Homebrew',
  electron: 'Electron 更新源（仅检测）',
};

export const ROUTE_WORD: Record<UpdateRoute, string> = {
  package_manager: '经审阅的更新计划',
  open_store: '在商店中更新',
  open_app: '在应用内更新',
  open_official_page: '前往官方页面',
  none: '无需操作',
};

/** Update check and routing reasons from the core, in words; anything else is shown as sent. */
const UPDATE_REASONS: [string, string][] = [
  ['No update channel', '没有更新通道：不是通过 winget、商店或 Scoop 安装，也没有可验证的更新源。'],
  ['Updates itself', '应用自带更新器，其更新源无法验证：请打开应用，用它自己的更新功能。'],
  ['Microsoft Store checks its own apps', '商店自行检查它的应用：请打开商店的“下载和更新”。'],
  ['Microsoft Store updates its own apps', '商店自行更新它的应用。'],
  ['Update checks are turned off', '更新检查已关闭，没有发出任何请求。'],
  ['winget could not be queried', '无法查询 winget，这次没有结果（不会显示为“已是最新”）。'],
  ['the installed app has no verifiable signer', '已安装的应用没有可验证的签名者，无法核对下载的安装包。'],
  ['the package manager for this app was not found', '这台电脑上没有找到这个应用的包管理器。'],
  ['Compared with the local bucket copy', '与本地 bucket 副本比较（不联网）。'],
];

export function updateReason(text: string | null): string | null {
  if (!text) return null;
  const hit = UPDATE_REASONS.find(([prefix]) => text.startsWith(prefix));
  return hit ? hit[1] : text;
}

// ---------------------------------------------------------------------------------------------
// Maintenance

export const TASK_WORDS: Record<string, { title: string; problem: string; impact: string; applies: string }> = {
  'windows.shell-restart': {
    title: '重启开始菜单与搜索',
    problem: '开始菜单或搜索打不开、结果陈旧或响应很慢。',
    impact: '开始菜单与搜索会关闭，并在下次使用时重新启动；打开的窗口和其他应用不受影响。',
    applies: '本会话中有来自系统应用目录的 StartMenuExperienceHost 或 SearchHost 在运行。',
  },
  'windows.icon-cache-rebuild': {
    title: '重建图标与缩略图缓存',
    problem: '图标或缩略图显示空白、错误或过期。',
    impact: '图标和缩略图在首次查看时重新绘制；只删除没有被占用的缓存文件。',
    applies: '资源管理器缓存文件夹中存在 iconcache_* / thumbcache_* 文件。',
  },
  'windows.dns-flush': {
    title: '刷新 DNS 解析缓存',
    problem: '网络或 DNS 变更后，网站仍解析到旧地址。',
    impact: '每个域名的第一次解析会稍慢一次。',
    applies: '系统的 DNS 客户端接口存在。',
  },
  'windows.trim-when-due': {
    title: '到期时对固态盘执行 retrim',
    problem: '固态盘很久没有 retrim（计划优化没有运行）。',
    impact: '短暂的磁盘活动，不移动数据；只在上次优化已超期的固态卷上运行。',
    applies: '已启用 TRIM 的固定固态卷，上次优化超过 30 天（或从未优化）。',
  },
  'windows.image-health-check': {
    title: '检查系统映像健康（只读）',
    problem: 'Windows 更新或功能安装失败，怀疑组件存储损坏。',
    impact: '只读检查组件存储，不会自动修复任何东西。',
    applies: 'Windows 维护（DISM）接口存在。',
  },
};

export const PROHIBITED_WORDS: Record<string, { title: string; reason: string }> = {
  'registry-optimizer': { title: '注册表清理 / “优化”', reason: '删除或改写注册表数据没有可测量的收益，还可能破坏应用或系统，所以没有这项任务。' },
  'performance-privacy-tweaks': { title: '注册表性能 / 隐私调优', reason: '未公开的设置改动效果无法证明、难以撤销；请在 Windows 设置中修改。' },
  'service-disable': { title: '禁用或删除系统服务', reason: '被禁用的服务会让更新、安全和功能在之后出问题。' },
  'search-indexing-disable': { title: '关闭 Windows 搜索索引', reason: '搜索、Outlook 和设置依赖索引；请在“设置 › 隐私和安全性 › 搜索 Windows”中管理索引位置。' },
  'system-debloat': { title: '系统精简 / 移除组件', reason: '移除系统组件或内置应用会破坏维护与更新；卸载你自己安装的应用请用“软件 › 卸载”。' },
  'memory-release': { title: '内存“释放” / 加速', reason: 'Windows 自行管理内存，强行换出只会让下次使用更慢；诊断会显示真实的内存压力。' },
  'pagefile-tweak': { title: '调整页面文件 / 虚拟内存', reason: '大小设置不当会导致崩溃并丢失转储；Windows 会自动调整。' },
  'rebuild-app-registrations': { title: '重新注册全部应用 / 重建应用注册数据库', reason: '批量重新注册可能让应用无法使用且无法撤销；请在“设置 › 应用”中修复单个应用。' },
  'ssd-full-defrag': { title: '对固态盘完整碎片整理', reason: '固态盘只需要 retrim（到期时由 TRIM 任务执行），完整整理只会增加磨损。' },
  'one-click-speedup': { title: '一键加速', reason: '没有可以诚实承诺的百分比；这里每项任务都说明它解决什么问题，并报告实际发生了什么。' },
};

export const APPLICABILITY: Record<Applicability, { label: string; tone: 'good' | 'warn' | 'muted' }> = {
  applicable: { label: '适用', tone: 'good' },
  not_needed: { label: '无需执行', tone: 'muted' },
  not_applicable: { label: '不适用', tone: 'muted' },
  unknown: { label: '状态未知', tone: 'warn' },
};

/** The six maintenance outcomes (MNT-03), each with its own shape. */
export const OUTCOME: Record<TaskOutcome, { label: string; explain: string }> = {
  applied: { label: '已执行', explain: '执行了，并且有改变。' },
  unchanged: { label: '无需改变', explain: '检查过，没有需要改变的。' },
  attention: { label: '需要跟进', explain: '执行或检查后，需要你再处理一步。' },
  skipped: { label: '已跳过', explain: '没有执行：不适用、受保护、拒绝提权或状态不明。' },
  unavailable: { label: '不可用', explain: '这台电脑缺少这项任务需要的系统接口。' },
  failed: { label: '失败', explain: '尝试了，但没有成功。' },
};

export const OUTCOME_ORDER: TaskOutcome[] = ['applied', 'unchanged', 'attention', 'skipped', 'unavailable', 'failed'];

export const ELEVATION_WORD: Record<string, string> = {
  not_needed: '没有需要管理员的任务',
  already_elevated: 'CleanManager 已以管理员身份运行',
  granted: '你在 Windows 提示中同意了一次',
  declined: '你拒绝了 Windows 的管理员提示，这些任务已跳过',
  unavailable: '没有可用的提权工作进程，这些任务已跳过',
};

/** Chinese for the reason codes of skipped tasks. */
export const TASK_CODE_WORD: Record<string, string> = {
  elevation_declined: '你拒绝了 Windows 的管理员提示，此任务没有运行。',
  elevation_unavailable: '没有可用的提权工作进程，此任务没有运行。',
  protected: '你把这项任务加入了保护，执行时跳过。',
  task_version_changed: '这项任务与审阅时的版本不同，没有运行。',
  state_unknown: '状态不明，没有猜测着执行。',
  cancelled: '你停止了后续任务，此项没有开始。',
};

export const FINDING_WORD: Record<string, { title: string; next: string | null }> = {
  runaway_process: { title: '进程持续高占用', next: '在“状态 › 进程”中查看' },
  busy_process: { title: '进程在采样期间繁忙', next: '在“状态 › 进程”中查看' },
  memory_pressure: { title: '内存紧张', next: null },
  commit_pressure: { title: '提交内存接近上限', next: null },
  low_disk_space: { title: '磁盘可用空间偏低', next: '去分析磁盘' },
  idle_virtual_machine: { title: '空闲的虚拟机仍在运行', next: null },
  attached_disk_image: { title: '磁盘映像仍处于挂载状态', next: null },
  many_startup_items: { title: '启用的启动项较多', next: '查看启动项' },
};

// ---------------------------------------------------------------------------------------------
// Processes

export const SIGNATURE_WORD: Record<string, string> = {
  signed: '已签名',
  catalog_signed: '系统目录签名',
  unsigned: '未签名',
  unknown: '签名未知',
};

export const PROTECTION_WORD: Record<string, string> = {
  critical_system: '系统关键进程：结束它会让系统停止或重启',
  os_critical: 'Windows 将它标记为关键进程',
  clean_manager: '这是 CleanManager 自己',
  security_software: '安全软件从不在 CleanManager 中结束；请使用产品自己的控制台',
  other_user: '属于其他用户或系统的进程；CleanManager 只结束你自己的进程',
};

export const SORT_WORD: Record<string, string> = { cpu: '处理器', memory: '内存', disk: '磁盘', network: '网络', name: '名称' };
