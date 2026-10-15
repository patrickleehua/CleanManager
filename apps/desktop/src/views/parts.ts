// Shared building blocks of P04–P10, so every page states sizes, risk, progress and errors the
// same way and lines up on the same skeleton (page head, work area, aside).

import { formatBytes, formatExactBytes, groupDigits, parseBytes } from '../lib/bytes';
import { h, icon } from '../lib/dom';
import { ERROR_HINT } from '../lib/format';
import type { Counts, Group, IpcError, ItemStatus } from '../lib/types';
import { GROUP_LABEL, ITEM_STATUS_LABEL, progressSegments } from '../lib/words';

type Child = Node | string | null | undefined | false;

export type Tone = 'good' | 'warn' | 'bad' | 'busy' | 'muted';

/** A state word with a shape marker; tone never carries meaning on its own. */
export function pill(text: string, tone: Tone): HTMLElement {
  return h('span', { class: 'pill', 'data-tone': tone }, h('span', { class: 'pill-mark', 'aria-hidden': 'true' }), text);
}

/** Risk group marker: shape + word (filled, half, dashed, struck). */
export function groupMark(group: Group, withLabel = true): HTMLElement {
  return h(
    'span',
    { class: 'gmark', 'data-group': group },
    h('span', { class: 'gmark-shape', 'aria-hidden': 'true' }),
    withLabel ? h('span', {}, GROUP_LABEL[group]) : h('span', { class: 'sr-only' }, GROUP_LABEL[group]),
  );
}

/** "812.0 MiB", or "至少 1.2 GiB（另有 3 项大小未知）" — unknown is never shown as 0. */
export function sizeText(known: string | null, unknownCount: number): string {
  const k = parseBytes(known);
  if (k === null) return '未知';
  if (unknownCount > 0) return k === 0n ? `未知（${groupDigits(unknownCount)} 项）` : `至少 ${formatBytes(k)}，另有 ${groupDigits(unknownCount)} 项未知`;
  return formatBytes(k);
}

export function exactTitle(known: string | null): string {
  return formatExactBytes(parseBytes(known));
}

export function errorText(e: IpcError): string {
  return ERROR_HINT[e.reason] ?? e.message;
}

export function errorLine(e: IpcError | null, id?: string): HTMLElement | null {
  if (!e) return null;
  return h('p', { class: 'form-error', role: 'alert', id }, errorText(e));
}

/** Page head shared by every page: optional back link, title, a meta line and actions. */
export function pageHead(opts: { id: string; title: string; back?: { label: string; onClick: () => void }; meta?: Child[]; actions?: Child[] }): HTMLElement {
  const back = opts.back
    ? h('button', { type: 'button', class: 'back-link', onclick: opts.back.onClick }, icon('back'), h('span', {}, opts.back.label))
    : null;
  return h(
    'div',
    { class: 'page-head' },
    h('div', { class: 'page-head-text' }, back, h('h1', { id: opts.id }, opts.title), opts.meta?.length ? metaRow(...opts.meta) : null),
    opts.actions?.length ? h('div', { class: 'head-actions' }, ...opts.actions) : null,
  );
}

/** Facts on one line, separated by hairlines (not dots). */
export function metaRow(...parts: Child[]): HTMLElement {
  return h('p', { class: 'meta-row' }, ...parts.filter(Boolean).map((p) => h('span', {}, p as Node | string)));
}

export function emptyState(title: string, body: string, ...actions: Child[]): HTMLElement {
  return h('div', { class: 'empty-block' }, h('h2', {}, title), h('p', { class: 'muted' }, body), actions.length ? h('div', { class: 'card-actions' }, ...actions) : null);
}

/**
 * A progress bar over a fixed plan total: every planned action sits in exactly one segment,
 * with a legend in words. It is the operation's progress, never a "space freed" percentage.
 */
export function segBar(total: number, counts: Counts, label: string): HTMLElement {
  const segs = progressSegments(total, counts);
  const sum = segs.reduce((a, s) => a + s.count, 0) || 1;
  const text = segs.map((s) => `${ITEM_STATUS_LABEL[s.key]} ${s.count}`).join('，');
  return h(
    'div',
    { class: 'segbar-wrap' },
    h(
      'div',
      { class: 'segbar', role: 'img', 'aria-label': `${label}：共 ${total} 项，${text}` },
      ...segs.map((s) => {
        // CSSOM, not a style attribute: the production CSP forbids inline style attributes.
        const part = h('span', { class: 'seg-part', 'data-status': s.key });
        part.style.flexGrow = String(s.count / sum);
        return part;
      }),
    ),
    h(
      'ul',
      { class: 'seg-legend', 'aria-hidden': 'true' },
      ...segs.map((s) => h('li', {}, statusMark(s.key), h('span', {}, ITEM_STATUS_LABEL[s.key]), h('b', { class: 'num' }, groupDigits(s.count)))),
    ),
  );
}

export function statusMark(s: ItemStatus): HTMLElement {
  return h('span', { class: 'smark', 'data-status': s, 'aria-hidden': 'true' });
}

export function statusWord(s: ItemStatus): HTMLElement {
  return h('span', { class: 'status-word', 'data-status': s }, statusMark(s), ITEM_STATUS_LABEL[s]);
}

/** A machine string (id, command) people may copy to the CLI. */
export function copyField(label: string, text: string): HTMLElement {
  const code = h('code', { class: 'copy-text selectable' }, text);
  const btn = h('button', { type: 'button', class: 'icon-btn copy-btn', 'aria-label': `复制：${label}`, title: '复制' }, icon('copy'));
  const done = h('span', { class: 'copy-done', 'aria-live': 'polite' });
  btn.addEventListener('click', async () => {
    try {
      await navigator.clipboard.writeText(text);
      done.textContent = '已复制';
    } catch {
      const range = document.createRange();
      range.selectNodeContents(code);
      const sel = window.getSelection();
      sel?.removeAllRanges();
      sel?.addRange(range);
      done.textContent = '已选中，可按 Ctrl+C 复制';
    }
    window.setTimeout(() => (done.textContent = ''), 2200);
  });
  return h('div', { class: 'copy-field' }, h('span', { class: 'copy-label' }, label), h('div', { class: 'copy-row' }, code, btn), done);
}

/**
 * Inline typed confirmation (enable a policy, remove a protection). The action stays disabled
 * until the typed text equals `expected`; Escape cancels and returns focus to the trigger.
 */
export function confirmInline(opts: {
  id: string;
  prompt: string;
  expected: string;
  action: string;
  danger?: boolean;
  onConfirm: (typed: string) => Promise<void>;
  onCancel: () => void;
}): HTMLElement {
  const input = h('input', {
    id: `${opts.id}-input`,
    type: 'text',
    class: 'confirm-input',
    autocomplete: 'off',
    spellcheck: 'false',
    'aria-describedby': `${opts.id}-prompt`,
  }) as HTMLInputElement;
  const go = h('button', { type: 'button', class: opts.danger ? 'danger' : 'primary', disabled: true }, opts.action) as HTMLButtonElement;
  const cancel = h('button', { type: 'button', class: 'quiet' }, '取消');
  const error = h('p', { class: 'form-error', role: 'alert', hidden: true });
  input.addEventListener('input', () => (go.disabled = input.value.trim() !== opts.expected));
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') opts.onCancel();
    if (e.key === 'Enter' && !go.disabled) go.click();
  });
  cancel.addEventListener('click', () => opts.onCancel());
  go.addEventListener('click', async () => {
    go.disabled = true;
    error.hidden = true;
    try {
      await opts.onConfirm(input.value.trim());
    } catch (e) {
      error.hidden = false;
      error.textContent = errorText(e as IpcError);
      go.disabled = false;
    }
  });
  const el = h(
    'div',
    { class: 'confirm-inline', role: 'group', 'aria-labelledby': `${opts.id}-prompt` },
    h('label', { id: `${opts.id}-prompt`, for: `${opts.id}-input` }, opts.prompt, ' ', h('code', { class: 'selectable' }, opts.expected)),
    h('div', { class: 'confirm-row' }, input, go, cancel),
    error,
  );
  requestAnimationFrame(() => input.focus());
  return el;
}

/** Tab list with arrow-key movement (WAI-ARIA tabs, automatic activation). */
export function tabList<T extends string>(label: string, tabs: { key: T; label: string; count?: string }[], current: T, onSelect: (k: T) => void): HTMLElement {
  const buttons = tabs.map((t) =>
    h(
      'button',
      {
        type: 'button',
        role: 'tab',
        id: `tab-${t.key}`,
        class: 'tab',
        'aria-selected': t.key === current ? 'true' : 'false',
        tabindex: t.key === current ? '0' : '-1',
        'aria-controls': `panel-${t.key}`,
      },
      t.label,
      t.count ? h('span', { class: 'tab-count num' }, t.count) : null,
    ),
  );
  buttons.forEach((b, i) => {
    b.addEventListener('click', () => onSelect(tabs[i].key));
    b.addEventListener('keydown', (e) => {
      const d = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : e.key === 'Home' ? -i : e.key === 'End' ? tabs.length - 1 - i : 0;
      if (!d) return;
      e.preventDefault();
      const n = (i + d + tabs.length) % tabs.length;
      onSelect(tabs[n].key);
      requestAnimationFrame(() => document.getElementById(`tab-${tabs[n].key}`)?.focus());
    });
  });
  return h('div', { class: 'tabbar', role: 'tablist', 'aria-label': label }, ...buttons);
}

export function bytesOrDash(v: string | null): string {
  return v === null ? '—' : formatBytes(v);
}
