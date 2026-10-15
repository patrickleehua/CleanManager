// Information architecture (D14 / GUI-08): five tools, then the extension entries, in one fixed
// order with fixed names and shortcuts on every platform. A feature that is not available here
// keeps its place and shows an explanatory page instead of disappearing.

import type { ActivityTab, Page } from './store';

export type ToolId = 'clean' | 'software' | 'optimize' | 'analyze' | 'status';
export type ExtId = 'plans' | 'activity' | 'recovery' | 'agent' | 'diag' | 'settings';

export interface Section {
  page: Page;
  label: string;
}

export interface Tool {
  id: ToolId;
  label: string;
  icon: string;
  /** Digit used with the platform modifier (Ctrl / ⌘). */
  key: string;
  /** One line under the tool name in the shortcut sheet and on narrow screens. */
  blurb: string;
  sections: Section[];
}

export interface Extension {
  id: ExtId;
  label: string;
  icon: string;
  key: string;
  page: Page;
  tab?: ActivityTab;
}

export const TOOLS: Tool[] = [
  {
    id: 'clean',
    label: '清理',
    icon: 'tool-clean',
    key: '1',
    blurb: '缓存、构建产物、安装包与回收站',
    sections: [
      { page: 'cleanup', label: '规则清理' },
      { page: 'purge', label: '构建产物' },
      { page: 'installer', label: '安装包' },
      { page: 'recyclebin', label: '回收站' },
    ],
  },
  {
    id: 'software',
    label: '软件',
    icon: 'tool-software',
    key: '2',
    blurb: '已安装应用、卸载、更新与启动项',
    sections: [
      { page: 'apps', label: '应用' },
      { page: 'uninstall', label: '卸载' },
      { page: 'updates', label: '更新' },
      { page: 'startup', label: '启动项' },
    ],
  },
  {
    id: 'optimize',
    label: '优化',
    icon: 'tool-optimize',
    key: '3',
    blurb: '系统维护任务',
    sections: [{ page: 'maintenance', label: '维护任务' }],
  },
  {
    id: 'analyze',
    label: '分析',
    icon: 'tool-analyze',
    key: '4',
    blurb: '磁盘占用、空间洞察与手选回收站',
    sections: [
      { page: 'overview', label: '概览' },
      { page: 'disk', label: '磁盘' },
      { page: 'insights', label: '空间洞察' },
      { page: 'scope', label: '扫描范围' },
    ],
  },
  {
    id: 'status',
    label: '状态',
    icon: 'tool-status',
    key: '5',
    blurb: '系统指标与进程',
    sections: [
      { page: 'status', label: '系统状态' },
      { page: 'processes', label: '进程' },
    ],
  },
];

export const EXTENSIONS: Extension[] = [
  { id: 'plans', label: '计划', icon: 'ext-plans', key: '6', page: 'activity', tab: 'plans' },
  { id: 'activity', label: '活动', icon: 'ext-activity', key: '7', page: 'activity', tab: 'operations' },
  { id: 'recovery', label: '恢复', icon: 'ext-recovery', key: '8', page: 'activity', tab: 'quarantine' },
  { id: 'agent', label: 'Agent', icon: 'ext-agent', key: '9', page: 'agent' },
  { id: 'diag', label: 'AI 诊断', icon: 'diag', key: '0', page: 'diag' },
  { id: 'settings', label: '设置', icon: 'settings', key: ',', page: 'settings' },
];

/** Where a page sits in the navigation: its tool, or an extension entry. */
export type NavPlace = { tool: ToolId } | { ext: ExtId } | null;

export function placeOf(page: Page, activityTab: ActivityTab, returnTo: Page | null, depth = 0): NavPlace {
  for (const t of TOOLS) if (t.sections.some((s) => s.page === page)) return { tool: t.id };
  if (page === 'activity') return { ext: activityTab === 'plans' ? 'plans' : activityTab === 'quarantine' ? 'recovery' : 'activity' };
  const ext = EXTENSIONS.find((e) => e.page === page && !e.tab);
  if (ext && page !== 'diag') return { ext: ext.id };
  // Detail pages (plan review, results, diagnosis opened from an object) belong to where they were opened from.
  if ((page === 'plan' || page === 'operation' || page === 'diag') && returnTo && returnTo !== page && depth < 3) {
    const from = placeOf(returnTo, activityTab, null, depth + 1);
    // A result opened from a review belongs to the activity history unless a tool opened it.
    if (from && page === 'operation' && !('tool' in from)) return { ext: 'activity' };
    if (from) return from;
  }
  if (page === 'diag') return { ext: 'diag' };
  if (page === 'plan') return { ext: 'plans' };
  if (page === 'operation') return { ext: 'activity' };
  return null;
}

export function toolOf(id: ToolId): Tool {
  return TOOLS.find((t) => t.id === id)!;
}

/** The tool whose sections include the page, if any. */
export function toolForPage(page: Page): Tool | null {
  return TOOLS.find((t) => t.sections.some((s) => s.page === page)) ?? null;
}

export type ShortcutTarget = { kind: 'tool'; tool: Tool } | { kind: 'ext'; ext: Extension } | { kind: 'section'; step: 1 | -1 } | { kind: 'help' };

/**
 * Keyboard map, identical on both platforms (the modifier is Ctrl on Windows and ⌘ on macOS):
 * mod+1…5 tools, mod+6…9/0 extensions, mod+, settings, mod+PageUp/PageDown previous/next
 * section of the current tool, ? the shortcut sheet (outside text fields).
 */
export function shortcutFor(e: Pick<KeyboardEvent, 'key' | 'ctrlKey' | 'metaKey' | 'altKey' | 'shiftKey'>, mac: boolean): ShortcutTarget | null {
  const mod = mac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  if (!mod && !e.altKey && !e.ctrlKey && !e.metaKey && e.key === '?') return { kind: 'help' };
  if (!mod || e.altKey) return null;
  if (e.key === 'PageDown') return { kind: 'section', step: 1 };
  if (e.key === 'PageUp') return { kind: 'section', step: -1 };
  if (e.shiftKey) return null;
  const tool = TOOLS.find((t) => t.key === e.key);
  if (tool) return { kind: 'tool', tool };
  const ext = EXTENSIONS.find((x) => x.key === e.key);
  if (ext) return { kind: 'ext', ext };
  return null;
}

/** Human-readable shortcut for a key, e.g. "Ctrl+1" or "⌘1". */
export function shortcutLabel(key: string, mod: string): string {
  return mod === '⌘' ? `⌘${key}` : `${mod}+${key}`;
}
