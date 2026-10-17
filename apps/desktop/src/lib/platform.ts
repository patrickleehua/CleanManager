// Platform idioms (GUI-12): one table of the words each platform uses for the same thing. Plans,
// results and protocol fields never change meaning between platforms; only these labels do.
// The table is applied once at boot (from the host's capabilities) before any page renders.
// Every Windows value is the text the pages showed before macOS existed, byte for byte; views
// take platform-specific sentences from here instead of writing "Windows" or "回收站" themselves.

export type PlatformId = 'windows' | 'macos';

export interface PlatformWords {
  /** The operating system's name. */
  os: string;
  /** "this computer" as a person calls it. */
  thisComputer: string;
  /** The recycle bin's name. */
  bin: string;
  /** "Move to <bin>" (the trash mode and every button that runs it). */
  moveToBin: string;
  /** Where a person restores from the bin themselves. */
  restoreFromBin: string;
  /** The file manager. */
  fileManager: string;
  /** "Show in <file manager>". */
  reveal: string;
  /** Modifier key name used in shortcut hints. */
  mod: string;
  /** Administrator / admin privileges wording. */
  admin: string;
  /** Where startup items are managed by the system. */
  startupSettings: string;
  /** The system's own process viewer. */
  taskManager: string;
  /** The app store's short name. */
  store: string;

  // 清理 › 回收站 (list is command-line only)
  /** What the bin list shows. */
  binListDoes: string;
  /** How a person puts an item back themselves (null: the list shows the original location). */
  binPutBack: string | null;

  // 软件 › 卸载
  /** Filter label for apps this page cannot uninstall (no verifiable route here). */
  uninstallElsewhereFilter: string;
  /** Kinds of leftovers ("A, B and C"). */
  leftoverKinds: string;
  /** Kinds of leftovers ("A, B or C"). */
  leftoverKindsOr: string;
  /** Stage 1 in one line (steps strip and plan review). */
  uninstallStep1: string;
  /** Stage 2 in one line. */
  uninstallStep2: string;
  /** How stage 1 runs, after "先请它退出，". */
  uninstallRuns: string;
  /** Where to remove an app this page will not uninstall. */
  uninstallManually: string;
  /** 软件 › 应用 detail: the uninstall line. */
  appsUninstallNote: string;
  /** 卸载 aside: review, rescan and still-running paragraphs. */
  uninstallHowReview: string;
  uninstallHowRescan: string;
  uninstallHowRunning: string;
  /** Plan review: what the uninstaller step does. */
  uninstallerAction: string;
  /** Result page: how an uninstall is undone. */
  uninstallUndo: string;
  /** Result page: how leftovers come back, after the bin sentence ('' when only the bin restores them). */
  leftoversUndo: string;
  /** Plan review: what plans of these kinds do, where it differs from the shared wording. */
  kindExplain: Record<string, { effect: string; space: string; recovery: string }>;

  // 软件 › 更新
  /** Progress line while channels are asked. */
  updatesProgress: string;
  /** Button that opens the store's updates page. */
  openStoreUpdates: string;
  /** Short button that opens the store. */
  openStore: string;
  /** How an update plan runs, in the draft panel. */
  updatesPlanNote: string;
  /** What an update plan contains (review page). */
  updatesPlanScope: string;

  // 优化 › 维护任务
  /** Catalog meta when CleanManager is not elevated. */
  maintAdminMeta: string;
  /** Draft notice for `n` administrator tasks. */
  maintAdminDraft: (n: number) => string;
  /** Plan review: how administrator tasks are authorized. */
  maintAdminReview: string;
  /** Readiness: missing administrator rights before a maintenance run. */
  maintAdminMissing: string;
  /** Readiness: what to do about it. */
  maintAdminFix: string;
  /** Search box example. */
  maintSearchExample: string;

  // 软件 › 启动项
  /** Plan review: what a startup change does. */
  startupEffect: string;

  // Readiness (uninstall)
  /** Missing administrator rights before an uninstall. */
  uninstallAdminMissing: string;

  // 状态 › 进程
  /** Footnote under the process table. */
  processesFoot: string;

  // 状态 › 常驻工具
  /** The resident component's name. */
  tray: string;
  /** Its menu. */
  trayMenu: string;
  /** The switch that turns it on. */
  trayShow: string;
  /** What turning it on and off does. */
  trayShowNote: (seconds: number) => string;
  /** Announced when it is turned off. */
  trayOffSaid: string;
  /** The start-at-login switch. */
  loginSwitch: string;
  /** Its note when the tray is on. */
  loginNote: string;
  /** How it is removed. */
  trayRemoval: string;
  /** The keys that always work during clean-screen mode, and how they are joined. */
  secureKeys: string[];
  secureJoin: string;
  secureNote: string;
  /** Extra limit of clean-screen mode on this platform (null: none). */
  cleanScreenLimit: string | null;
  /** Keep-awake footnote. */
  awakeFoot: string;
  /** Privacy footnote (`seconds`: page polling interval). */
  privacyFoot: (seconds: number) => string;
  /** Lead of the line when camera/microphone state cannot be read. */
  privacyUnreadable: string;
  /** Battery footnote. */
  batteryFoot: string;
  /** No system battery. */
  noSystemBattery: string;

  // Shell
  /** What an off (gated) section does, where it differs from the shared wording (keyed by page). */
  offDoes: Record<string, string[]>;
  /** Gate pill title of an unreleased tool. */
  devBuild: string;
  /** Diagnosis: where the API key is pasted. */
  keyPrompt: string;
  /** Diagnosis: what a CLI provider must be. */
  cliExecutable: string;
}

export const PLATFORM_WORDS: Record<PlatformId, PlatformWords> = {
  windows: {
    os: 'Windows',
    thisComputer: '这台电脑',
    bin: '回收站',
    moveToBin: '移到回收站',
    restoreFromBin: '在资源管理器的回收站中还原',
    fileManager: '资源管理器',
    reveal: '在资源管理器中显示',
    mod: 'Ctrl',
    admin: '管理员权限',
    startupSettings: '任务管理器 › 启动应用',
    taskManager: '任务管理器',
    store: '商店',

    binListDoes: '列出你的回收站中的项目，显示原位置、删除时间和大小。',
    binPutBack: null,

    uninstallElsewhereFilter: '需在设置中卸载',
    leftoverKinds: '文件、注册表项和启动项',
    leftoverKindsOr: '文件、注册表项或启动项',
    uninstallStep1: '先请应用退出，再运行它自己的卸载程序，最后重新检查安装目录和登记。',
    uninstallStep2: '卸载后单独审阅确实留下的文件、注册表项和启动项。',
    uninstallRuns: '再运行它自己的卸载程序',
    uninstallManually: '无法验证卸载方式的应用请在“设置 › 应用 › 已安装的应用”中卸载；CleanManager 从不把删除安装目录当作卸载。',
    appsUninstallNote: '此版本不提供。请使用 Windows 设置或厂商的卸载程序。',
    uninstallHowReview: '每一步都先生成计划，由你审阅并输入摘要前 8 位批准后才执行。卸载程序保留它自己的界面，CleanManager 不添加静默参数。',
    uninstallHowRescan: '执行后重新检查安装目录和卸载登记；仍有剩余时结果记为“部分完成”，残留在第 2 步单独审阅。',
    uninstallHowRunning: '只发出退出请求并等待。应用没有退出时，卸载程序不会运行，也不会删除任何东西；这里永远不会强制结束进程。',
    uninstallerAction: '运行它自己的卸载程序，记录退出码，再重新检查',
    uninstallUndo: '卸载无法由 CleanManager 撤销；需要时重新安装这个应用。',
    leftoversUndo: '注册表项与启动项可以从执行时保存的备份放回，同样先审阅再执行。',
    kindExplain: {},

    updatesProgress: '正在询问各更新通道（winget 访问它自己配置的源；Scoop 只读本地目录）…',
    openStoreUpdates: '打开商店的更新页',
    openStore: '打开商店',
    updatesPlanNote: '计划中的每个应用会单独执行：先下载这个确切版本，验证安装包签名者与已安装的应用一致，再升级并回读版本。',
    updatesPlanScope: '只包含能用包管理器验证的更新；商店、自带更新器和无法验证签名的应用不在其中。',

    maintAdminMeta: '需要管理员的任务会在执行时请求一次授权',
    maintAdminDraft: (n) => `其中 ${n} 项需要管理员：执行开始前 Windows 会询问一次，只授权这份计划中的这些任务。拒绝只跳过这些任务，其余照常运行。CleanManager 不会在后台静默提权。`,
    maintAdminReview: '执行开始、任何任务运行之前，Windows 会询问一次；只授权这份计划中的这些任务。拒绝只跳过它们，其余照常运行。',
    maintAdminMissing: '没有管理员权限：需要管理员的任务会在开始前请 Windows 询问你一次；拒绝只跳过这些任务，其余任务照常运行。',
    maintAdminFix: 'Windows 的管理员提示只出现一次，并且只授权这份计划中列出的任务。',
    maintSearchExample: '想做什么？例如：DNS、图标、注册表优化',

    startupEffect: '只修改系统的启用记录（与任务管理器中的“启动应用”相同），不删除程序、快捷方式或它的设置。',

    uninstallAdminMissing: '没有管理员权限：所有用户共用位置（HKLM、ProgramData）中的残留只能查看，不能放入计划。',

    processesFoot: '读不到的值显示为“—”，不会写成 0。磁盘是进程的读写传输速率，可能与任务管理器不同；每个进程的网络速率需要管理员跟踪，本版本不采样。',

    tray: '托盘',
    trayMenu: '托盘菜单',
    trayShow: '在通知区域显示 CleanManager',
    trayShowNote: (s) => `默认关闭。开启后每 ${s} 秒采样一次处理器、内存和系统盘可用空间；关闭即停止采样、移除图标和登录启动项。`,
    trayOffSaid: '托盘已关闭：已停止采样并移除图标与登录启动项',
    loginSwitch: '登录 Windows 时只启动托盘',
    loginNote: '在当前用户的启动项中写入 CleanManager.Resident，关闭这个开关或托盘时删除。',
    trayRemoval: '关闭托盘即移除图标与登录启动项。卸载 CleanManager（self uninstall）时也会列出并删除这个启动项。',
    secureKeys: ['Ctrl', 'Alt', 'Del'],
    secureJoin: '+',
    secureNote: ' 始终可用',
    cleanScreenLimit: null,
    awakeFoot: '向系统提出临时请求（与播放视频时相同），不修改电源计划、息屏或睡眠设置。到时、点“结束”或退出 CleanManager 时自动撤销。',
    privacyFoot: (s) => `本页打开时每 ${s} 秒读取一次 Windows 的使用记录；托盘开启时随托盘读取。只在本机显示，不保存、不上传。`,
    privacyUnreadable: '无法读取系统的使用记录：',
    batteryFoot: '蓝牙设备电量来自 Windows 蓝牙协议栈为设备记录的电量值（与“设置 › 蓝牙和其他设备”相同）；没有记录的设备不显示数字。',
    noSystemBattery: '这台电脑没有系统电池。',

    offDoes: {},
    devBuild: '仅 Windows 开发构建，未发布',
    keyPrompt: ' 密钥不在这里填写：保存后点「保存密钥…」，在 Windows 安全窗口的“密码”栏粘贴。',
    cliExecutable: '只接受 .exe；以固定参数运行、不经过 shell、在空的临时目录里、关闭全部工具与文件访问。无法确认能关闭工具的 CLI 不会被启动。是否复用你的订阅额度由 CLI 自己的规则决定。',
  },
  macos: {
    os: 'macOS',
    thisComputer: '这台 Mac',
    bin: '废纸篓',
    moveToBin: '移到废纸篓',
    restoreFromBin: '在访达的废纸篓中“放回原处”',
    fileManager: '访达',
    reveal: '在访达中显示',
    mod: '⌘',
    admin: '管理员密码',
    startupSettings: '系统设置 › 通用 › 登录项与扩展',
    taskManager: '活动监视器',
    store: 'App Store',

    binListDoes: '列出你的废纸篓中的项目和大小。macOS 不公开原位置和删除时间（访达的“放回原处”记录是私有的），所以列表中没有原位置。',
    binPutBack: '要把项目放回原处，请在访达中打开废纸篓，选中项目后选择“文件 › 放回原处”；CleanManager 不替你放回。',

    uninstallElsewhereFilter: '需要手动卸载',
    leftoverKinds: '文件和启动项',
    leftoverKindsOr: '文件或启动项',
    uninstallStep1: '先请应用退出，再把应用移到废纸篓或用 Homebrew 卸载，最后重新检查应用是否还在。',
    uninstallStep2: '卸载后单独审阅确实留下的文件和启动项，它们会移到废纸篓。',
    uninstallRuns: '再移到废纸篓或用 Homebrew 卸载',
    uninstallManually:
      '需要管理员的应用（App Store 应用、属于 root 的应用）请在启动台中删除，或在访达中移到废纸篓（访达会要求管理员密码）；这一版在 macOS 上没有提权方式，也不会询问密码。',
    appsUninstallNote: '请在“软件 › 卸载”中卸载，或在访达中把应用移到废纸篓。',
    uninstallHowReview: '每一步都先生成计划，由你审阅并输入摘要前 8 位批准后才执行。普通应用包移到废纸篓（可以“放回原处”），Homebrew 应用用 brew uninstall 卸载，从不使用 --zap。',
    uninstallHowRescan: '执行后重新检查应用包（Homebrew 应用还检查它的安装记录）；仍有剩余时结果记为“部分完成”，残留在第 2 步单独审阅。',
    uninstallHowRunning: '只发出退出请求并等待（应用可以询问是否保存，也可以拒绝）。应用没有退出时，什么都不会移动或卸载；这里永远不会强制结束进程。',
    uninstallerAction: '移到废纸篓或运行 brew uninstall，再重新检查',
    uninstallUndo: '移到废纸篓的应用可以在访达中“放回原处”；用 Homebrew 卸载的应用需要重新安装。',
    leftoversUndo: '',
    kindExplain: {
      uninstall: {
        effect: '先请应用退出并等待；应用没有退出时什么都不做。然后把应用包移到废纸篓（Homebrew 应用运行 brew uninstall，从不使用 --zap），再重新检查应用是否还在。',
        space: '移到废纸篓的应用包仍占用空间，清空废纸篓后才释放；留下的文件在下一步单独审阅。',
        recovery: '应用包可以在访达的废纸篓中“放回原处”；用 Homebrew 卸载的应用需要重新安装。',
      },
      uninstall_leftovers: {
        effect: '文件逐个移到废纸篓；macOS 上没有注册表步骤，也从不永久删除。',
        space: '废纸篓仍占用这个卷的空间，清空废纸篓后才释放。',
        recovery: '在访达的废纸篓中选中文件，用“放回原处”还原。',
      },
      update: {
        effect: '逐个应用由 Homebrew 升级（安装前它会校验下载的校验和），然后回读版本。',
        space: '新版本可能比旧版本略大；下载缓存由 Homebrew 自己管理。',
        recovery: '更新不能由 CleanManager 撤销；校验失败时不安装，原应用不变。',
      },
    },

    updatesProgress: '正在询问各更新通道（Homebrew 读取它自己的索引；Sparkle 更新源只读取 https 地址）…',
    openStoreUpdates: '打开 App Store 的更新页',
    openStore: '打开 App Store',
    updatesPlanNote: '计划中的每个应用会单独执行：由 Homebrew 升级（安装前它会校验下载的校验和），再回读版本。',
    updatesPlanScope: '只包含 Homebrew 能升级的应用；App Store、Sparkle 和其他应用自带的更新器不在其中。',

    maintAdminMeta: '需要管理员的任务会被跳过（这一版没有提权方式）',
    maintAdminDraft: (n) => `其中 ${n} 项需要管理员：这一版在 macOS 上没有提权方式，执行时这些任务会记为“已跳过”，其余照常运行。CleanManager 不会弹出密码提示，也不会在后台提权。`,
    maintAdminReview: '这一版在 macOS 上没有提权方式：这些任务在执行时跳过并记录原因，不会弹出密码提示；其余任务照常运行。',
    maintAdminMissing: '需要管理员的任务会被跳过：这一版在 macOS 上没有提权方式，其余任务照常运行。',
    maintAdminFix: '需要这些任务时，请按任务说明中的系统命令在终端中自行运行。',
    maintSearchExample: '想做什么？例如：DNS、快速预览、聚焦',

    startupEffect: '只修改系统的启用记录（与“系统设置 › 通用 › 登录项与扩展”相同），不删除程序或它的设置。',

    uninstallAdminMissing: '没有管理员权限：所有用户共用位置（/Library）中的残留只能查看，不能放入计划。',

    processesFoot: '读不到的值显示为“—”，不会写成 0。磁盘是进程的读写传输速率，可能与活动监视器不同；本版本不采样每个进程的网络速率。',

    tray: '菜单栏图标',
    trayMenu: '菜单栏菜单',
    trayShow: '在菜单栏显示 CleanManager',
    trayShowNote: (s) => `默认关闭。开启后每 ${s} 秒采样一次处理器、内存和系统盘可用空间；关闭即停止采样并移除图标。`,
    trayOffSaid: '菜单栏图标已关闭：已停止采样并移除图标',
    loginSwitch: '登录 Mac 时只启动菜单栏图标',
    loginNote: '这一版不提供：注册登录项（SMAppService）需要签名的应用包。',
    trayRemoval: '关闭菜单栏图标即移除图标并停止采样。这一版不创建登录项，也不写入 ~/Library/LaunchAgents，没有需要删除的启动项。',
    secureKeys: ['⌥', '⌘', 'Esc'],
    secureJoin: '',
    secureNote: '（强制退出）始终可用',
    cleanScreenLimit: '只在 CleanManager 位于前台时暂停输入；亮度、音量、调度中心和截屏等系统按键仍然有效。',
    awakeFoot: '向系统提出临时请求（与播放视频时相同），不修改“系统设置”中的显示器、锁定屏幕或电池设置。到时、点“结束”或退出 CleanManager 时自动撤销。',
    privacyFoot: (s) =>
      `本页打开时每 ${s} 秒读取一次 macOS 报告的设备状态；菜单栏图标开启时随它读取。摄像头只知道正在使用，macOS 不说明是哪个应用；麦克风在 macOS 14 及以上可以显示应用名称。只在本机显示，不保存、不上传。`,
    privacyUnreadable: '无法确定摄像头或麦克风的状态（不会显示为“未在使用”）：',
    batteryFoot: '外设电量来自 Apple 外设驱动在 I/O Registry 中公布的电量值；没有公布电量的设备列为“不可用”，不显示数字，也不估计。',
    noSystemBattery: '这台 Mac 没有内置电池。',

    offDoes: {
      uninstall: ['把应用移到废纸篓（可以“放回原处”），或用 Homebrew 卸载，再单独审阅残留。', '残留同样移到废纸篓；macOS 上没有注册表步骤。'],
      updates: ['只在你要求时检查 Homebrew、App Store 和 Sparkle 更新源。', 'Homebrew 更新会生成需要审阅的计划，其他应用指向它自己的更新入口。'],
      startup: ['列出 LaunchAgent、LaunchDaemon 和登录项，显示来源和签名。', '这一版在 macOS 上只读：登录项请在“系统设置 › 通用 › 登录项与扩展”中修改。'],
      maintenance: ['运行系统维护任务，例如重置快速预览缓存、检查聚焦索引和查找损坏的偏好设置。', '需要管理员的任务在这一版中会被跳过：macOS 上没有提权方式。'],
      resident: ['在菜单栏显示简要状态，并提供亮屏、清洁屏幕、隐私指示和电量提醒。', '关闭实验功能会停止这些常驻工具；这一版不创建登录项。'],
    },
    devBuild: '仅 macOS 开发构建，未发布',
    keyPrompt: ' 密钥不在这里填写：保存后点「保存密钥…」，在弹出的安全输入窗口中粘贴，密钥保存在钥匙串中。',
    cliExecutable: '只接受可执行文件；以固定参数运行、不经过 shell、在空的临时目录里、关闭全部工具与文件访问。无法确认能关闭工具的 CLI 不会被启动。是否复用你的订阅额度由 CLI 自己的规则决定。',
  },
};

let current: PlatformId = 'windows';
const listeners = new Set<(p: PlatformId) => void>();

/** The words of the platform the app runs on. */
export function words(): PlatformWords {
  return PLATFORM_WORDS[current];
}

/** The platform the app runs on: the host's capabilities record decides (set once at boot). */
export function platform(): PlatformId {
  return current;
}

/** Switch the vocabulary (boot, or tests). Registered vocabularies (e.g. MODE_LABEL) follow. */
export function setPlatform(p: PlatformId): void {
  current = p;
  listeners.forEach((l) => l(p));
}

/** Let a vocabulary module re-derive its platform-dependent labels. Runs once immediately. */
export function onPlatform(l: (p: PlatformId) => void): void {
  listeners.add(l);
  l(current);
}

/**
 * Keep a word table equal to its Windows entries on Windows and to those entries plus `mac` on macOS. The Windows
 * entries are taken from the table as written, so their text and order never change.
 */
export function macOverlay<V>(table: Record<string, V>, mac: Record<string, V>): void {
  const base = { ...table };
  onPlatform((p) => {
    for (const k of Object.keys(table)) delete table[k];
    Object.assign(table, base, p === 'macos' ? mac : {});
  });
}

/** A "not offered here" line: the Windows text as written, one honest line on macOS. */
export function notOnThisPlatform(windowsText: string): string {
  return current === 'macos' ? '这一版的 macOS 宿主没有提供这项功能。' : windowsText;
}

/**
 * Browser design preview only (never the desktop host, whose capabilities decide): `?platform=macos` or
 * `?platform=windows` in the URL, otherwise the browser's own system.
 */
export function previewPlatform(search: string = globalThis.location?.search ?? '', userAgent: string = globalThis.navigator?.userAgent ?? ''): PlatformId {
  const asked = new URLSearchParams(search).get('platform');
  if (asked === 'macos' || asked === 'mac') return 'macos';
  if (asked === 'windows' || asked === 'win') return 'windows';
  return /Macintosh|Mac OS X/.test(userAgent) ? 'macos' : 'windows';
}
