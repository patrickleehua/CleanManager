// Information architecture (D14 / GUI-08): five tools, then the extension entries, in one fixed
// order with fixed names and shortcuts on every platform. A feature that is not available here
// keeps its place and shows an explanatory page instead of disappearing.

import type { ActivityTab, Page } from './store';
import type { GateRow } from './types';
import { words } from './platform';
import { t } from '../i18n';

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
  /** Release gate (D15) this tool's capabilities must pass; none has passed yet, so every tool is labelled experimental. */
  gate: string;
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
    gate: 'G1',
    get label() { return t('nav.tools.clean.label'); },
    icon: 'tool-clean',
    key: '1',
    get blurb() { return t('nav.cachesBuildArtifactsInstallers', { bin: words().bin }); },
    sections: [
      { page: 'cleanup', get label() { return t('nav.tools.clean.sections.cleanup.label'); } },
      { page: 'purge', get label() { return t('nav.tools.clean.sections.purge.label'); } },
      { page: 'installer', get label() { return t('nav.tools.clean.sections.installer.label'); } },
      { page: 'recyclebin', get label() { return words().bin; } },
    ],
  },
  {
    id: 'software',
    gate: 'G3',
    get label() { return t('nav.tools.software.label'); },
    icon: 'tool-software',
    key: '2',
    get blurb() { return t('nav.tools.software.blurb'); },
    sections: [
      { page: 'apps', get label() { return t('nav.tools.software.sections.apps.label'); } },
      { page: 'uninstall', get label() { return t('nav.tools.software.sections.uninstall.label'); } },
      { page: 'updates', get label() { return t('nav.tools.software.sections.updates.label'); } },
      { page: 'startup', get label() { return t('nav.tools.software.sections.startup.label'); } },
    ],
  },
  {
    id: 'optimize',
    gate: 'G3',
    get label() { return t('nav.tools.optimize.label'); },
    icon: 'tool-optimize',
    key: '3',
    get blurb() { return t('nav.tools.optimize.blurb'); },
    sections: [
      { page: 'maintenance', get label() { return t('nav.tools.optimize.sections.maintenance.label'); } },
      { page: 'diagnosis', get label() { return t('nav.tools.optimize.sections.diagnosis.label'); } },
    ],
  },
  {
    id: 'analyze',
    gate: 'G1',
    get label() { return t('nav.tools.analyze.label'); },
    icon: 'tool-analyze',
    key: '4',
    get blurb() { return t('nav.diskUsageSpaceInsights', { bin: words().bin }); },
    sections: [
      { page: 'overview', get label() { return t('nav.tools.analyze.sections.overview.label'); } },
      { page: 'disk', get label() { return t('nav.tools.analyze.sections.disk.label'); } },
      { page: 'insights', get label() { return t('nav.tools.analyze.sections.insights.label'); } },
      { page: 'scope', get label() { return t('nav.tools.analyze.sections.scope.label'); } },
    ],
  },
  {
    id: 'status',
    gate: 'G3',
    get label() { return t('nav.tools.status.label'); },
    icon: 'tool-status',
    key: '5',
    get blurb() { return t('nav.tools.status.blurb'); },
    sections: [
      { page: 'status', get label() { return t('nav.tools.status.sections.status.label'); } },
      { page: 'processes', get label() { return t('nav.tools.status.sections.processes.label'); } },
      { page: 'resident', get label() { return t('nav.tools.status.sections.resident.label'); } },
    ],
  },
];

export const EXTENSIONS: Extension[] = [
  { id: 'plans', get label() { return t('nav.extensions.plans.label'); }, icon: 'ext-plans', key: '6', page: 'activity', tab: 'plans' },
  { id: 'activity', get label() { return t('nav.extensions.activity.label'); }, icon: 'ext-activity', key: '7', page: 'activity', tab: 'operations' },
  { id: 'recovery', get label() { return t('nav.extensions.recovery.label'); }, icon: 'ext-recovery', key: '8', page: 'activity', tab: 'quarantine' },
  { id: 'agent', label: 'Agent', icon: 'ext-agent', key: '9', page: 'agent' },
  { id: 'diag', get label() { return t('nav.extensions.diag.label'); }, icon: 'diag', key: '0', page: 'diag' },
  { id: 'settings', get label() { return t('nav.extensions.settings.label'); }, icon: 'settings', key: ',', page: 'settings' },
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

/**
 * Short label for a tool whose gate has not passed (18.5: the UI never presents ungated tools as released). Plain
 * words: the gate id is not shown in the bar (设置 › 实验功能 lists each capability with its gate).
 */
export function gateLabel(_tool: Tool): string {
  return t('nav.experimental');
}

/** A section backed by a capability with a release gate (D15). */
export interface GatedSection {
  capability: string;
  gate: string;
}

/**
 * D15: sections whose capability has not passed its release gate. They are off unless 设置 › 实验功能 is on; an off
 * section keeps its place in the navigation (GUI-08) and opens an explanatory page. Sections not listed are core.
 */
export const GATED_PAGES: Partial<Record<Page, GatedSection>> = {
  purge: { capability: 'purge.artifacts', gate: 'G1' },
  installer: { capability: 'installer.cleanup', gate: 'G1' },
  recyclebin: { capability: 'trash.manual', gate: 'G1' },
  insights: { capability: 'insights.overview', gate: 'G1' },
  uninstall: { capability: 'apps.uninstall', gate: 'G3' },
  updates: { capability: 'apps.updates', gate: 'G3' },
  startup: { capability: 'startup.toggle', gate: 'G3' },
  maintenance: { capability: 'maintenance.run', gate: 'G3' },
  diagnosis: { capability: 'maintenance.run', gate: 'G3' },
  status: { capability: 'status.metrics', gate: 'G3' },
  processes: { capability: 'process.end', gate: 'G3' },
  resident: { capability: 'resident.utilities', gate: 'G4' },
  diag: { capability: 'diag.ai', gate: 'AID' },
};

/**
 * The gate a page waits on when it is off here, or null when it is core or on. The host's rows are authoritative;
 * without them (capabilities unavailable) a gated page is off, as the host would refuse it anyway.
 */
export function gatedOff(page: Page, gates: GateRow[] | null | undefined): GatedSection | null {
  const g = GATED_PAGES[page];
  if (!g) return null;
  const row = gates?.find((r) => r.capability === g.capability);
  if (row?.enabled) return null;
  return row ? { capability: row.capability, gate: row.gate } : g;
}
