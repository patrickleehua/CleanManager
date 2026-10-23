// 分析 › 磁盘 → 手选移到回收站 (MTR-01..05, GUI-09/12/13). The person picks scanned entries;
// each is inspected read-only (size, protected descendants, unread parts, processes holding it),
// then the trashable ones are frozen into one manual-selection plan for the shared review page.
// Only entry ids cross IPC; the plan moves nothing until it is approved and applied.

import type { App } from '../main';
import { formatBytes } from '../lib/bytes';
import { h, icon, replace } from '../lib/dom';
import { RequestGate } from '../lib/gate';
import { words } from '../lib/platform';
import type { Entry, TrashInspection } from '../lib/types';
import { atLeast, ensureReady, sumSizes, toIpcError } from './kit';
import { errorText } from './parts';
import { protectionText, refusalText as codeRefusalText } from '../lib/codes';
import { plainPath } from '../lib/diskModel';
import { t } from '../i18n';

const MAX_ENTRIES = 50;

const REFUSAL: Record<string, string> = {
  get protected() { return t('trash.refusal.protected'); },
  get protected_descendants() { return t('trash.refusal.protected_descendants'); },
  get coverage_gap() { return t('trash.refusal.coverage_gap'); },
  get cloud_placeholder() { return t('trash.refusal.cloud_placeholder'); },
  get hard_denied() { return t('trash.refusal.hard_denied'); },
  get no_recycle_bin() { return t('trash.refusal.no_recycle_bin', { bin: words().bin }); },
  get mounted() { return t('trash.refusal.mounted'); },
  get root() { return t('trash.refusal.root'); },
};

function refusalText(i: TrashInspection): string {
  const r = i.refusal;
  if (!r) return '';
  // WFA-47/63: a link into a protected system location names where it leads.
  if (r.reason === 'protected' && i.link_target_display) return t('trash.refusal.linkProtected', { target: plainPath(i.link_target_display) });
  // A reason without words here reads through the shared code table (never the host's raw message).
  return REFUSAL[r.reason] ?? codeRefusalText({ reason: r.reason });
}

/**
 * NEW-13: why one protected object inside the selection is protected, in words (cm-core manual_trash.rs writes it in
 * English: a default category, a hard-deny location or check, or one of the person's own entries).
 */
export function protectedWhy(reason: string): string {
  const cls = /^default protection class ([a-z_]+)/.exec(reason);
  if (cls) return protectionText(cls[1]);
  if (/^hard-deny/.test(reason)) return t('trash.why.hardDeny');
  const user = /protected by user entry (\S+)/.exec(reason);
  if (user) return t('trash.why.userEntry', { id: user[1] });
  return t('trash.why.other');
}

/** Opens the review dialog for the selected entries; resolves when it closes. */
export async function reviewTrash(app: App, scanId: string, entries: Entry[]): Promise<void> {
  if (!entries.length) return;
  const w = words();
  if (!(await ensureReady(app, 'trash', w.moveToBin))) return;
  const picked = entries.slice(0, MAX_ENTRIES);
  const gate = new RequestGate();
  const ticket = gate.next();
  const results = new Map<string, TrashInspection | string>();
  let creating = false;
  let error: string | null = null;

  const dlg = h('dialog', { class: 'cm-dialog trash-review', 'aria-labelledby': 'trash-title' }) as HTMLDialogElement;
  const list = h('ul', { class: 'trash-list', 'aria-live': 'polite' });
  const status = h('p', { class: 'muted small', role: 'status' });
  const go = h('button', { type: 'button', class: 'primary', id: 'trash-create', disabled: true }, t('trash.createPlan'));
  const cancel = h('button', { type: 'button', id: 'trash-cancel' }, t('common.cancel'));
  const errEl = h('p', { class: 'form-error', role: 'alert', hidden: true });
  const close = () => {
    gate.invalidate(); // late inspections never write into a closed dialog
    dlg.close();
    dlg.remove();
  };
  cancel.addEventListener('click', close);
  dlg.addEventListener('cancel', (e) => {
    e.preventDefault();
    close();
  });

  const ok = () => [...results.values()].filter((r): r is TrashInspection => typeof r !== 'string' && r.trashable);
  const render = () => {
    replace(
      list,
      ...picked.map((e) => {
        const r = results.get(e.entry_id);
        const ins = typeof r === 'string' ? null : (r ?? null);
        const state = r === undefined ? 'pending' : typeof r === 'string' ? 'error' : r.trashable ? 'ok' : 'no';
        return h(
          'li',
          { 'data-state': state },
          h('span', { class: 'tr-mark', 'aria-hidden': 'true' }, state === 'ok' ? icon('check') : state === 'pending' ? null : icon(state === 'no' ? 'lock' : 'warn')),
          h(
            'span',
            { class: 'tr-main' },
            h('b', { class: 'tr-name' }, e.name),
            h('span', { class: 'path muted small' }, e.path_display),
            state === 'pending' ? h('span', { class: 'small muted' }, t('common.checking')) : null,
            typeof r === 'string' ? h('span', { class: 'why-not' }, r) : null,
            ins && !ins.trashable ? h('span', { class: 'why-not' }, refusalText(ins)) : null,
            ins && ins.protected.length
              ? h(
                  'span',
                  { class: 'tr-sub small' },
                  t('trash.protectedCount', { protected_count: ins.protected_count }),
                  ins.protected
                    .slice(0, 3)
                    .map((p) => t('trash.pathDisplayReason', { path_display: p.path_display, reason: protectedWhy(p.reason) }))
                    .join(t('common.clauseSep')),
                  ins.protected_count > 3 ? t('trash.more') : '',
                )
              : null,
            ins && ins.gap_count ? h('span', { class: 'tr-sub small' }, t('trash.gapCountJoin', { gap_count: ins.gap_count, join: ins.coverage_gaps.slice(0, 2).map((g) => g.path_display).join(t('common.clauseSep')) })) : null,
            ins && ins.holders && ins.holders.length ? h('span', { class: 'tr-sub small' }, t('trash.useJoinSkippedWhen', { join: ins.holders.join(t('common.listSep')) })) : null,
            // WFA-35: the host refuses such an object (`recycle_bin_unavailable`); its own wording stays in the details.
            ins && !ins.recycle_bin_ready && ins.refusal?.reason !== 'recycle_bin_unavailable' ? h('span', { class: 'why-not' }, t('trash.volumeNoUsableBin', { bin: w.bin })) : null,
          ),
          h('span', { class: 'tr-size num' }, ins ? (ins.logical_bytes === null ? t('common.sizeUnknown') : formatBytes(ins.logical_bytes)) : formatBytes(e.logical_bytes)),
        );
      }),
    );
    const good = ok();
    const done = results.size === picked.length;
    const sum = sumSizes(good.map((g) => ({ bytes: g.logical_bytes })));
    status.textContent = done
      ? t('trash.lengthLength2UseMovetobin', { length: good.length, length2: picked.length, moveToBin: w.moveToBin, sum: atLeast(sum), p: entries.length > MAX_ENTRIES ? t('trash.mostMaxEntriesItems', { MAX_ENTRIES }) : '' })
      : t('trash.checkingSizeLengthRead', { size: results.size, length: picked.length });
    go.disabled = !done || good.length === 0 || creating;
    go.textContent = creating ? t('common.freezingPlan') : good.length ? t('trash.createPlanLength', { length: good.length }) : t('trash.nothingProcessed');
    errEl.hidden = !error;
    errEl.textContent = error ?? '';
  };

  go.addEventListener('click', async () => {
    const good = ok();
    if (!good.length || creating) return;
    creating = true;
    error = null;
    render();
    try {
      const r = await app.backend.trashPlan(scanId, good.map((g) => g.entry_id));
      close();
      app.announce(t('common.planCreatedNotRun'));
      app.openPlan(r.plan_id, 'disk');
    } catch (e) {
      error = errorText(toIpcError(e));
      creating = false;
      render();
    }
  });

  dlg.append(
    h(
      'div',
      { class: 'dlg-head' },
      h('h2', { id: 'trash-title' }, t('trash.movetobinLength', { moveToBin: w.moveToBin, length: picked.length })),
      h('p', { class: 'muted' }, t('trash.eachItemGetsRead', { restoreFromBin: w.restoreFromBin })),
    ),
    h('div', { class: 'dlg-body' }, list, status, errEl),
    h('div', { class: 'dlg-actions' }, cancel, go),
  );
  document.body.append(dlg);
  render();
  dlg.showModal();

  for (const e of picked) {
    try {
      const r = await gate.run(ticket, app.backend.trashInspect(scanId, e.entry_id));
      if (r.stale) return;
      results.set(e.entry_id, r.value);
    } catch (err) {
      if (!gate.isCurrent(ticket)) return;
      results.set(e.entry_id, errorText(toIpcError(err)));
    }
    render();
  }
  requestAnimationFrame(() => (go.disabled ? cancel : go).focus());
}
