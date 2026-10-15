// Platform idioms (GUI-12): one table of the words each platform uses for the same thing. Plans,
// results and protocol fields never change meaning between platforms; only these labels do.
// The table is applied once at boot (from the host's capabilities) before any page renders.

export type PlatformId = 'windows' | 'macos';

export interface PlatformWords {
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
}

export const PLATFORM_WORDS: Record<PlatformId, PlatformWords> = {
  windows: {
    bin: '回收站',
    moveToBin: '移到回收站',
    restoreFromBin: '在资源管理器的回收站中还原',
    fileManager: '资源管理器',
    reveal: '在资源管理器中显示',
    mod: 'Ctrl',
    admin: '管理员权限',
    startupSettings: '任务管理器 › 启动应用',
  },
  macos: {
    bin: '废纸篓',
    moveToBin: '移到废纸篓',
    restoreFromBin: '在访达的废纸篓中“放回原处”',
    fileManager: '访达',
    reveal: '在访达中显示',
    mod: '⌘',
    admin: '管理员密码',
    startupSettings: '系统设置 › 通用 › 登录项',
  },
};

let current: PlatformId = 'windows';
const listeners = new Set<(p: PlatformId) => void>();

/** The words of the platform the app runs on. */
export function words(): PlatformWords {
  return PLATFORM_WORDS[current];
}

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
