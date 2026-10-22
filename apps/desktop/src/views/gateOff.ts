// D15 — a section whose capability has not passed its release gate is off unless 设置 › 实验功能 is on. It keeps
// its place in the navigation (GUI-08) and shows this explanatory page instead of the tool; the host refuses the
// tool's commands as well, so nothing here depends on the page being hidden.

import type { App } from '../main';
import { h, replace } from '../lib/dom';
import { EXTENSIONS, toolForPage, type GatedSection } from '../lib/nav';
import { words } from '../lib/platform';
import type { Page } from '../lib/store';
import { gateOffPage } from './kit';
import { t } from '../i18n';

/** What each gated section does, in the person's words (shown while it is off). */
const DOES: Partial<Record<Page, () => string[]>> = {
  purge: () => [t('gateOff.does.purge.0'), t('gateOff.does.purge.1')],
  installer: () => [t('gateOff.does.installer.0'), t('gateOff.does.installer.1')],
  recyclebin: () => [t('gateOff.does.recyclebin.0', { bin: words().bin }), t('gateOff.does.recyclebin.1', { moveToBin: words().moveToBin })],
  insights: () => [t('gateOff.does.insights.0'), t('gateOff.does.insights.1')],
  uninstall: () => [t('gateOff.does.uninstall.0'), t('gateOff.does.uninstall.1')],
  updates: () => [t('gateOff.does.updates.0'), t('gateOff.does.updates.1')],
  startup: () => [t('gateOff.does.startup.0'), t('gateOff.does.startup.1')],
  maintenance: () => [t('gateOff.does.maintenance.0'), t('gateOff.does.maintenance.1')],
  diagnosis: () => [t('gateOff.does.diagnosis.0'), t('gateOff.does.diagnosis.1')],
  status: () => [t('gateOff.does.status.0'), t('gateOff.does.status.1')],
  processes: () => [t('gateOff.does.processes.0'), t('gateOff.does.processes.1')],
  resident: () => [t('gateOff.does.resident.0'), t('gateOff.does.resident.1')],
  diag: () => [t('gateOff.does.diag.0'), t('gateOff.does.diag.1')],
};

export function titleOf(page: Page): string {
  if (page === 'recyclebin') return words().bin;
  return toolForPage(page)?.sections.find((s) => s.page === page)?.label ?? EXTENSIONS.find((e) => e.page === page)?.label ?? page;
}

/** What an off section says it does (empty for a page that is not gated). */
export function offDoes(page: Page): string[] {
  return words().offDoes[page] ?? DOES[page]?.() ?? [];
}

/** Bring 设置 › 实验功能 into view with focus on its switch. */
export function openExperimentalSettings(app: App): void {
  app.navigate('settings');
  requestAnimationFrame(() => {
    document.getElementById('set-experimental-title')?.scrollIntoView({ block: 'start' });
    (document.getElementById('experimental-switch') as HTMLElement | null)?.focus({ preventScroll: true });
  });
}

export class GateOffView {
  el: HTMLElement;
  private gate: GatedSection;

  constructor(
    private app: App,
    private page: Page,
    gate: GatedSection,
  ) {
    this.gate = gate;
    this.el = h('section', { class: `page page-${page} page-gate-off`, 'aria-labelledby': `off-${page}-title` });
  }

  /** The host's row may name a different gate than the local fallback; keep the page in step. */
  show(gate: GatedSection): void {
    this.gate = gate;
  }

  update(): void {
    replace(
      this.el,
      gateOffPage({
        id: `off-${this.page}`,
        title: titleOf(this.page),
        capability: this.gate.capability,
        gate: this.gate.gate,
        does: offDoes(this.page),
        openSettings: () => openExperimentalSettings(this.app),
      }),
    );
  }
}
