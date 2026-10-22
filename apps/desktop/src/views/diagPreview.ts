// Disclosure preview (AID-07): before anything is sent, show where it goes and the exact bytes.
// Changing the scope (full path, child summary) prepares a new preview; closing the dialog
// discards it and sends nothing. Confirmation names the preview id and the digest shown.

import type { App } from '../main';
import { toIpcError } from '../lib/api';
import { formatBytes } from '../lib/bytes';
import { h, replace } from '../lib/dom';
import type { Diagnosis, Disclosure, Preview } from '../lib/diagTypes';
import { EXCLUDED_LABEL, LAYER_LABEL, PROVIDER_LABEL, REDACTION_LABEL, payloadSize, prettyBody } from '../lib/diagWords';
import { errorText, metaRow, pill } from './parts';
import { t } from '../i18n';

export interface PreviewRequest {
  /** Prepare (or re-prepare with another scope) and return the preview. */
  prepare(d: Disclosure): Promise<Preview>;
  /** The scope can be changed (false for follow-ups, which keep the parent's scope). */
  scopeEditable: boolean;
  title: string;
}

/** Open the dialog; resolves with the running diagnosis, or null when cancelled. */
export function openPreview(app: App, req: PreviewRequest, initial: Preview): Promise<Diagnosis | null> {
  return new Promise((resolve) => {
    let preview = initial;
    let busy = false;
    let done = false;
    const dialog = h('dialog', { class: 'diag-dialog', 'aria-labelledby': 'dp-title', 'aria-describedby': 'dp-lede' }) as HTMLDialogElement;
    const body = h('div', { class: 'dp-body' });
    const error = h('p', { class: 'form-error', role: 'alert', hidden: true });
    const send = h('button', { type: 'button', class: 'primary', id: 'dp-send' }, t('diagPreview.sendDiagnose')) as HTMLButtonElement;
    const cancel = h('button', { type: 'button', class: 'quiet', id: 'dp-cancel' }, t('diagPreview.cancelDontSend'));

    const close = (result: Diagnosis | null) => {
      if (done) return;
      done = true;
      if (!result) void app.backend.diagDiscard(preview.preview_id).catch(() => {});
      dialog.close();
      dialog.remove();
      resolve(result);
    };

    const scope = (key: keyof Disclosure, label: string, note: string) => {
      const input = h('input', { type: 'checkbox', id: `dp-${key}`, checked: preview.request.disclosure[key], disabled: !req.scopeEditable || busy }) as HTMLInputElement;
      input.addEventListener('change', async () => {
        busy = true;
        render();
        error.hidden = true;
        const next = { ...preview.request.disclosure, [key]: input.checked };
        try {
          const old = preview.preview_id;
          preview = await req.prepare(next);
          void app.backend.diagDiscard(old).catch(() => {});
          app.announce(t('diagPreview.whatSentChangedReview'));
        } catch (e) {
          error.hidden = false;
          error.textContent = errorText(toIpcError(e));
        } finally {
          busy = false;
          render();
          document.getElementById(`dp-${key}`)?.focus();
        }
      });
      return h('label', { class: 'dp-scope' }, input, h('span', { class: 'dp-scope-text' }, h('b', {}, label), h('span', {}, note)));
    };

    const render = () => {
      const p = preview;
      const d = p.destination;
      const s = p.evidence.subject;
      const where = d.mode === 'cli' ? d.executable : d.endpoint;
      const redactions = [...new Set(p.evidence.redactions.map((r) => REDACTION_LABEL[r.rule] ?? r.rule))];
      replace(
        body,
        h(
          'div',
          { class: 'dp-dest' },
          h('p', { class: 'dp-dest-label' }, t('diagPreview.send')),
          h('p', { class: 'dp-dest-name' }, d.label, h('span', { class: 'muted' }, t('diagPreview.pModelModel', { p: d.label === PROVIDER_LABEL[d.kind] ? '' : t('common.parenthetical', { p: PROVIDER_LABEL[d.kind] }), model: d.model }))),
          h('p', { class: 'path small muted selectable' }, where ?? ''),
          d.mode === 'api'
            ? h('p', { class: 'small' }, p.payload.transport === 'http_post_loopback' ? t('diagPreview.localEndpoint') : t('diagPreview.dataSentProviderOver'))
            : h('p', { class: 'small' }, t('diagPreview.localCliAllTools')),
        ),
        h(
          'fieldset',
          { class: 'dp-scopes' },
          h('legend', {}, t('diagPreview.whatsSent')),
          h('p', { class: 'small muted' }, t('diagPreview.defaultOnlyRedactedMetadata')),
          scope('full_path', t('diagPreview.includeFullPath'), t('diagPreview.nameParentFoldersSent')),
          scope('child_summary', t('diagPreview.includeChildSummary'), t('diagPreview.up5LargestDirect')),
        ),
        h(
          'section',
          { class: 'dp-section', 'aria-labelledby': 'dp-obj' },
          h('h3', { id: 'dp-obj' }, t('diagPreview.objectDetails')),
          h(
            'dl',
            { class: 'facts-list small' },
            h('dt', {}, t('common.name')),
            h('dd', { class: 'selectable' }, s.name),
            h('dt', {}, t('diagPreview.location')),
            h('dd', { class: 'path selectable' }, s.location || t('diagPreview.root')),
            h('dt', {}, t('common.size')),
            h('dd', {}, formatBytes(s.logical_bytes)),
            p.evidence.children ? h('dt', {}, t('diagPreview.childSummary')) : null,
            p.evidence.children ? h('dd', {}, p.evidence.children.map((c) => t('diagPreview.nameLogicalBytes', { name: c.name, logical_bytes: formatBytes(c.logical_bytes) })).join(t('common.commaSep'))) : null,
            redactions.length ? h('dt', {}, t('diagPreview.redacted')) : null,
            redactions.length ? h('dd', {}, redactions.join(t('common.commaSep'))) : null,
          ),
          h(
            'ul',
            { class: 'dp-evidence' },
            ...p.evidence.items.map((i) => h('li', {}, h('span', { class: 'lmark', 'data-layer': i.layer, 'aria-hidden': 'true' }), h('span', { class: 'sr-only' }, t('common.labelColon', { p: LAYER_LABEL[i.layer] })), i.text)),
          ),
          p.contains_user_additions ? h('p', { class: 'small' }, pill(t('diagPreview.includesAdditions'), 'warn'), t('diagPreview.questionAnswersSentAlong')) : null,
        ),
        h(
          'section',
          { class: 'dp-section dp-never', 'aria-labelledby': 'dp-never' },
          h('h3', { id: 'dp-never' }, t('diagPreview.neverSent')),
          h('ul', {}, ...p.evidence.excluded.map((x) => h('li', {}, EXCLUDED_LABEL[x] ?? x))),
        ),
        h(
          'details',
          { class: 'dp-raw' },
          h('summary', {}, t('diagPreview.viewExactlyWhatSent', { payload_bytes: payloadSize(p.payload_bytes) })),
          p.payload.headers.length ? h('pre', { class: 'dp-pre selectable' }, p.payload.headers.join('\n')) : null,
          p.payload.argv.length ? h('pre', { class: 'dp-pre selectable' }, [t('diagPreview.arguments'), ...p.payload.argv.map((a) => (a === '' ? '""' : a))].join('\n')) : null,
          h('pre', { class: 'dp-pre selectable', tabindex: '0', 'aria-label': t('diagPreview.actualPayloadIndented') }, prettyBody(p.payload.body)),
          h('p', { class: 'small muted' }, t('diagPreview.indentedReadabilitySameContent'), h('code', { class: 'selectable' }, p.payload_digest.slice(0, 16)), '…'),
        ),
      );
      send.disabled = busy;
    };

    send.addEventListener('click', async () => {
      busy = true;
      render();
      error.hidden = true;
      try {
        const d = await app.backend.diagConfirm(preview.preview_id, preview.payload_digest);
        done = true;
        dialog.close();
        dialog.remove();
        resolve(d);
      } catch (e) {
        busy = false;
        error.hidden = false;
        error.textContent = errorText(toIpcError(e));
        render();
      }
    });
    cancel.addEventListener('click', () => close(null));
    dialog.addEventListener('cancel', (e) => {
      e.preventDefault();
      close(null);
    });

    dialog.append(
      h(
        'div',
        { class: 'dp-head' },
        h('h2', { id: 'dp-title' }, req.title),
        h('p', { id: 'dp-lede', class: 'muted small' }, t('diagPreview.nothingSentUntilConfirm')),
        metaRow(t('diagPreview.previewPreviewId', { preview_id: initial.preview_id }), t('diagPreview.validRound', { round: Math.round((Date.parse(initial.expires_at) - Date.now()) / 60_000) })),
      ),
      body,
      error,
      h('div', { class: 'dp-actions' }, cancel, send),
    );
    render();
    document.body.append(dialog);
    dialog.showModal();
    requestAnimationFrame(() => send.focus());
  });
}
