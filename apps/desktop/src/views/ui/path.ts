// A path shown shortened in the middle. The full path stays available: as the tooltip, to screen readers,
// in what the clipboard receives when the text is copied, and through an optional copy button.

import { middleEllipsis } from '../../lib/display';
import { h, icon } from '../../lib/dom';
import { t } from '../../i18n';

export interface PathOpts {
  /** Width budget in units (CJK counts 2). */
  max?: number;
  className?: string;
  /** Show a copy button next to the path. */
  copy?: boolean;
}

export function pathText(full: string, opts: PathOpts = {}): HTMLElement {
  const shown = middleEllipsis(full, opts.max ?? 56);
  const cut = shown !== full;
  const visible = h('span', { class: 'pt-shown', 'aria-hidden': cut ? 'true' : null }, shown);
  const el = h(
    'span',
    { class: `path-text${opts.className ? ` ${opts.className}` : ''}`, title: full, 'data-full': full },
    visible,
    cut ? h('span', { class: 'sr-only' }, full) : null,
  );
  // Copying the shortened text copies the whole path.
  el.addEventListener('copy', (e: ClipboardEvent) => {
    if (!cut || !e.clipboardData) return;
    e.clipboardData.setData('text/plain', full);
    e.preventDefault();
  });
  if (opts.copy) {
    const done = h('span', { class: 'pt-done', 'aria-live': 'polite' });
    const btn = h('button', { type: 'button', class: 'icon-btn pt-copy', title: t('ui.path.copyFullPath'), 'aria-label': t('ui.path.copyFullPathFull', { full }) }, icon('copy'));
    btn.addEventListener('click', async () => {
      try {
        await navigator.clipboard.writeText(full);
        done.textContent = t('common.copied');
      } catch {
        done.textContent = t('ui.path.cantAccessClipboardSelect');
      }
      window.setTimeout(() => (done.textContent = ''), 2200);
    });
    el.append(btn, done);
  }
  return el;
}
