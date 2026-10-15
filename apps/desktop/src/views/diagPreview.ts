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
    const send = h('button', { type: 'button', class: 'primary', id: 'dp-send' }, '发送并诊断') as HTMLButtonElement;
    const cancel = h('button', { type: 'button', class: 'quiet', id: 'dp-cancel' }, '取消，不发送');

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
          app.announce('发送范围已更新，请重新查看将发送的内容');
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
          h('p', { class: 'dp-dest-label' }, '发送到'),
          h('p', { class: 'dp-dest-name' }, d.label, h('span', { class: 'muted' }, `${d.label === PROVIDER_LABEL[d.kind] ? '' : `（${PROVIDER_LABEL[d.kind]}）`}，模型 ${d.model}`)),
          h('p', { class: 'path small muted selectable' }, where ?? ''),
          d.mode === 'api'
            ? h('p', { class: 'small' }, p.payload.transport === 'http_post_loopback' ? '本机端点（127.0.0.1）：数据不离开这台电脑。' : '数据会通过 HTTPS 发送给这个提供方，按其服务条款处理。')
            : h('p', { class: 'small' }, '本机 CLI：所有工具、文件访问与写入均已关闭，在空的临时目录中运行；资料通过标准输入传入。'),
        ),
        h(
          'fieldset',
          { class: 'dp-scopes' },
          h('legend', {}, '发送范围'),
          h('p', { class: 'small muted' }, '默认只发送这一个对象的脱敏元数据与本机证据。'),
          scope('full_path', '包含完整路径', '名称和上级目录按原样发送（可能含账户名）。'),
          scope('child_summary', '包含子项摘要', '最多 5 个最大的直接子项，名称已脱敏，不含更深的目录树。'),
        ),
        h(
          'section',
          { class: 'dp-section', 'aria-labelledby': 'dp-obj' },
          h('h3', { id: 'dp-obj' }, '对象资料'),
          h(
            'dl',
            { class: 'facts-list small' },
            h('dt', {}, '名称'),
            h('dd', { class: 'selectable' }, s.name),
            h('dt', {}, '位置'),
            h('dd', { class: 'path selectable' }, s.location || '（根）'),
            h('dt', {}, '大小'),
            h('dd', {}, formatBytes(s.logical_bytes)),
            p.evidence.children ? h('dt', {}, '子项摘要') : null,
            p.evidence.children ? h('dd', {}, p.evidence.children.map((c) => `${c.name}（${formatBytes(c.logical_bytes)}）`).join('，')) : null,
            redactions.length ? h('dt', {}, '已脱敏') : null,
            redactions.length ? h('dd', {}, redactions.join('，')) : null,
          ),
          h(
            'ul',
            { class: 'dp-evidence' },
            ...p.evidence.items.map((i) => h('li', {}, h('span', { class: 'lmark', 'data-layer': i.layer, 'aria-hidden': 'true' }), h('span', { class: 'sr-only' }, `${LAYER_LABEL[i.layer]}：`), i.text)),
          ),
          p.contains_user_additions ? h('p', { class: 'small' }, pill('包含你的补充', 'warn'), ' 你的问题或回答会作为“你的陈述”一并发送。') : null,
        ),
        h(
          'section',
          { class: 'dp-section dp-never', 'aria-labelledby': 'dp-never' },
          h('h3', { id: 'dp-never' }, '不会发送'),
          h('ul', {}, ...p.evidence.excluded.map((x) => h('li', {}, EXCLUDED_LABEL[x] ?? x))),
        ),
        h(
          'details',
          { class: 'dp-raw' },
          h('summary', {}, `查看实际发送的 ${payloadSize(p.payload_bytes)}`),
          p.payload.headers.length ? h('pre', { class: 'dp-pre selectable' }, p.payload.headers.join('\n')) : null,
          p.payload.argv.length ? h('pre', { class: 'dp-pre selectable' }, ['参数：', ...p.payload.argv.map((a) => (a === '' ? '""' : a))].join('\n')) : null,
          h('pre', { class: 'dp-pre selectable', tabindex: '0', 'aria-label': '实际载荷（已缩进显示）' }, prettyBody(p.payload.body)),
          h('p', { class: 'small muted' }, '为便于阅读已缩进显示；发送的是同一内容的紧凑形式。摘要 ', h('code', { class: 'selectable' }, p.payload_digest.slice(0, 16)), '…'),
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
        h('p', { id: 'dp-lede', class: 'muted small' }, '确认之前不会发送任何内容。这些资料只用于解释这一个对象；诊断不会删除、移动或批准任何东西。'),
        metaRow(`预览 ${initial.preview_id}`, `${Math.round((Date.parse(initial.expires_at) - Date.now()) / 60_000)} 分钟内有效`),
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
