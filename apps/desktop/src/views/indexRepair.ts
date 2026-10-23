// NEW-04: the damaged-index state of the pages that show scans (overview, disk, scope) and its repair. The banner says
// what is wrong in words — never "no scans yet" — and offers the rebuild; the confirmation says exactly what the rebuild
// does: the damaged file is kept under a new name, an empty index replaces it, scans must be run again, and history,
// quarantine and recovery are untouched. The host does the work (`repair_index`); the WebView sends no path.

import type { App } from '../main';
import { toIpcError } from '../lib/api';
import { h, icon } from '../lib/dom';
import { INDEX_TEXT, repairOutcome, type IndexProblem } from '../lib/indexModel';
import { copyField, errorLine } from './parts';
import { statusBanner, techBlock } from './ui';
import { t } from '../i18n';

/** The page's primary message while the index cannot be used, with the rebuild as its one action. */
export function indexProblemBanner(app: App, p: IndexProblem, o: { titleId: string; level?: 1 | 2; srPrefix?: string }): HTMLElement {
  const repair = h('button', { type: 'button', class: 'primary', id: `${o.titleId}-repair` }, INDEX_TEXT.repair);
  repair.addEventListener('click', () => void confirmIndexRepair(app, repair));
  return h(
    'div',
    { class: 'index-problem' },
    statusBanner({ kind: 'failed', level: o.level ?? 1, titleId: o.titleId, srPrefix: o.srPrefix, title: INDEX_TEXT.title(p.reason), sentence: INDEX_TEXT.sentence, actions: [repair] }),
    techBlock(h('p', { class: 'selectable small' }, h('code', {}, p.reason), ` ${p.message}`), copyField(INDEX_TEXT.cli, 'cleanmanager doctor --repair-index')),
  );
}

/**
 * The confirmation before the rebuild. Cancel has the focus; nothing changes until the person confirms. On success
 * the pages forget the old scans (they are no longer in the index) and the scope page opens for a new scan.
 */
export async function confirmIndexRepair(app: App, returnFocus?: HTMLElement | null): Promise<void> {
  let busy = false;
  const dlg = h('dialog', { class: 'cm-dialog index-repair', 'aria-labelledby': 'ir-title', 'aria-describedby': 'ir-lede' }) as HTMLDialogElement;
  const cancel = h('button', { type: 'button', id: 'ir-cancel' }, t('common.cancel')) as HTMLButtonElement;
  const go = h('button', { type: 'button', class: 'primary', id: 'ir-go' }, INDEX_TEXT.confirm) as HTMLButtonElement;
  const errBox = h('div', { class: 'ir-error' });
  const close = () => {
    dlg.close();
    dlg.remove();
    returnFocus?.focus();
  };
  cancel.addEventListener('click', close);
  dlg.addEventListener('cancel', (e) => {
    e.preventDefault();
    if (!busy) close();
  });
  const fact = (shape: string, text: string) => h('li', {}, h('span', { class: 'ir-mark', 'aria-hidden': 'true' }, icon(shape)), h('span', {}, text));
  go.addEventListener('click', async () => {
    if (busy) return;
    busy = true;
    go.disabled = true;
    cancel.disabled = true;
    go.textContent = INDEX_TEXT.repairing;
    errBox.replaceChildren();
    try {
      const r = await app.backend.repairIndex(true);
      const outcome = repairOutcome(r);
      close();
      const caps = await app.backend.capabilities().catch(() => app.store.state.caps);
      const recent = await app.backend.recentScans().catch(() => []);
      // The old scans are gone from the new index: no page stays bound to one of them.
      app.store.set({ caps, recent, indexRepairedAt: outcome.done ? Date.now() : app.store.state.indexRepairedAt ?? null, scanId: null, status: null, statusError: null, task: null, page: 'scope' });
      app.announce(outcome.text);
    } catch (e) {
      busy = false;
      go.disabled = false;
      cancel.disabled = false;
      go.textContent = INDEX_TEXT.confirm;
      const line = errorLine(toIpcError(e));
      if (line) errBox.replaceChildren(line);
    }
  });
  dlg.append(
    h('div', { class: 'dlg-head' }, h('h2', { id: 'ir-title' }, INDEX_TEXT.dialogTitle), h('p', { id: 'ir-lede', class: 'muted' }, INDEX_TEXT.dialogLede)),
    h(
      'div',
      { class: 'dlg-body' },
      h('ul', { class: 'ir-facts' }, fact('shield', INDEX_TEXT.kept), fact('refresh', INDEX_TEXT.rescan), fact('check', INDEX_TEXT.untouched)),
      h('p', { class: 'muted small' }, INDEX_TEXT.busyNote),
      errBox,
    ),
    h('div', { class: 'dlg-actions' }, cancel, go),
  );
  document.body.append(dlg);
  dlg.showModal();
  requestAnimationFrame(() => cancel.focus());
}
