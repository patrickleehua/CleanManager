// Platform idioms (GUI-12): one table of the words each platform uses for the same thing. Plans,
// results and protocol fields never change meaning between platforms; only these labels do.
// The table is applied once at boot (from the host's capabilities) before any page renders.
// Every Windows value is the text the pages showed before macOS existed, byte for byte; views
// take platform-specific sentences from here instead of writing "Windows" or "回收站" themselves.
// The words live in the language resources (src/i18n, LIF-05) as `platform.*` keys: the key is the Windows text and
// `key@macos` the macOS text, in every language.

import { locale, setTextVariant, t, translate, type Params, type TextKey } from '../i18n';

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

/** What is not language: the OS name, key caps and how they are joined. */
const KEYS: Record<PlatformId, { os: string; mod: string; secureKeys: string[]; secureJoin: string }> = {
  windows: { os: 'Windows', mod: 'Ctrl', secureKeys: ['Ctrl', 'Alt', 'Del'], secureJoin: '+' },
  macos: { os: 'macOS', mod: '⌘', secureKeys: ['⌥', '⌘', 'Esc'], secureJoin: '' },
};
/** Plan kinds whose review wording differs on macOS, and gated pages that explain themselves differently there. */
const MAC_KIND_EXPLAIN = ['uninstall', 'uninstall_leftovers', 'update'] as const;
const MAC_OFF_DOES = ['uninstall', 'updates', 'startup', 'maintenance', 'resident'] as const;

/**
 * The words of one platform in the active language (LIF-05). Every entry reads the resources each time, so a language
 * switch shows at once; the Windows text is the resource key itself and the macOS text its `@macos` reading.
 */
function wordsOf(p: PlatformId): PlatformWords {
  const w = (key: TextKey, params?: Params) => translate(locale(), p, key, params);
  const mac = p === 'macos';
  return {
    get os() { return KEYS[p].os; },
    get thisComputer() { return w('platform.thisComputer'); },
    get bin() { return w('platform.bin'); },
    get moveToBin() { return w('platform.moveToBin'); },
    get restoreFromBin() { return w('platform.restoreFromBin'); },
    get fileManager() { return w('platform.fileManager'); },
    get reveal() { return w('platform.reveal'); },
    get mod() { return KEYS[p].mod; },
    get admin() { return w('platform.admin'); },
    get startupSettings() { return w('platform.startupSettings'); },
    get taskManager() { return w('platform.taskManager'); },
    get store() { return w('platform.store'); },

    get binListDoes() { return w('platform.binListDoes'); },
    get binPutBack() { return mac ? w('platform.binPutBack') : null; },

    get uninstallElsewhereFilter() { return w('platform.uninstallElsewhereFilter'); },
    get leftoverKinds() { return w('platform.leftoverKinds'); },
    get leftoverKindsOr() { return w('platform.leftoverKindsOr'); },
    get uninstallStep1() { return w('platform.uninstallStep1'); },
    get uninstallStep2() { return w('platform.uninstallStep2'); },
    get uninstallRuns() { return w('platform.uninstallRuns'); },
    get uninstallManually() { return w('platform.uninstallManually'); },
    get appsUninstallNote() { return w('platform.appsUninstallNote'); },
    get uninstallHowReview() { return w('platform.uninstallHowReview'); },
    get uninstallHowRescan() { return w('platform.uninstallHowRescan'); },
    get uninstallHowRunning() { return w('platform.uninstallHowRunning'); },
    get uninstallerAction() { return w('platform.uninstallerAction'); },
    get uninstallUndo() { return w('platform.uninstallUndo'); },
    get leftoversUndo() { return mac ? '' : w('platform.leftoversUndo'); },
    get kindExplain() {
      if (!mac) return {};
      return Object.fromEntries(
        MAC_KIND_EXPLAIN.map((k) => [k, { effect: w(`platform.kindExplain.${k}.effect` as const), space: w(`platform.kindExplain.${k}.space` as const), recovery: w(`platform.kindExplain.${k}.recovery` as const) }]),
      );
    },

    get updatesProgress() { return w('platform.updatesProgress'); },
    get openStoreUpdates() { return w('platform.openStoreUpdates'); },
    get openStore() { return w('platform.openStore'); },
    get updatesPlanNote() { return w('platform.updatesPlanNote'); },
    get updatesPlanScope() { return w('platform.updatesPlanScope'); },

    get maintAdminMeta() { return w('platform.maintAdminMeta'); },
    maintAdminDraft: (n) => w('platform.maintAdminDraft', { n }),
    get maintAdminReview() { return w('platform.maintAdminReview'); },
    get maintAdminMissing() { return w('platform.maintAdminMissing'); },
    get maintAdminFix() { return w('platform.maintAdminFix'); },
    get maintSearchExample() { return w('platform.maintSearchExample'); },

    get startupEffect() { return w('platform.startupEffect'); },

    get uninstallAdminMissing() { return w('platform.uninstallAdminMissing'); },

    get processesFoot() { return w('platform.processesFoot'); },

    get tray() { return w('platform.tray'); },
    get trayMenu() { return w('platform.trayMenu'); },
    get trayShow() { return w('platform.trayShow'); },
    trayShowNote: (s) => w('platform.trayShowNote', { s }),
    get trayOffSaid() { return w('platform.trayOffSaid'); },
    get loginSwitch() { return w('platform.loginSwitch'); },
    get loginNote() { return w('platform.loginNote'); },
    get trayRemoval() { return w('platform.trayRemoval'); },
    get secureKeys() { return KEYS[p].secureKeys; },
    get secureJoin() { return KEYS[p].secureJoin; },
    get secureNote() { return w('platform.secureNote'); },
    get cleanScreenLimit() { return mac ? w('platform.cleanScreenLimit') : null; },
    get awakeFoot() { return w('platform.awakeFoot'); },
    privacyFoot: (s) => w('platform.privacyFoot', { s }),
    get privacyUnreadable() { return w('platform.privacyUnreadable'); },
    get batteryFoot() { return w('platform.batteryFoot'); },
    get noSystemBattery() { return w('platform.noSystemBattery'); },

    get offDoes() {
      if (!mac) return {};
      return Object.fromEntries(MAC_OFF_DOES.map((k) => [k, [w(`platform.offDoes.${k}.0` as const), w(`platform.offDoes.${k}.1` as const)]]));
    },
    get devBuild() { return w('platform.devBuild'); },
    get keyPrompt() { return w('platform.keyPrompt'); },
    get cliExecutable() { return w('platform.cliExecutable'); },
  };
}

export const PLATFORM_WORDS: Record<PlatformId, PlatformWords> = { windows: wordsOf('windows'), macos: wordsOf('macos') };

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

/** Switch the vocabulary (boot, or tests). Every resource with a `@macos` reading follows (LIF-05 × GUI-12). */
export function setPlatform(p: PlatformId): void {
  current = p;
  setTextVariant(p);
  listeners.forEach((l) => l(p));
}

/** Let a vocabulary module re-derive its platform-dependent labels. Runs once immediately. */
export function onPlatform(l: (p: PlatformId) => void): void {
  listeners.add(l);
  l(current);
}

/** A "not offered here" line: the Windows text as written, one honest line on macOS. */
export function notOnThisPlatform(windowsText: string): string {
  return current === 'macos' ? t('platform.macosHostDoesntOffer') : windowsText;
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
