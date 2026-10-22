// Small shared pieces: size text (unknown is "未知"), the experimental / off badge in plain words,
// "了解更多" and "技术详情" disclosures, and empty states.

import { sizeLabel, sizeTitle } from '../../lib/display';
import { h, icon } from '../../lib/dom';
import { t } from '../../i18n';

type Child = Node | string | null | undefined | false;

/** A size; `null` reads "未知". The exact byte count is the tooltip. */
export function sizeEl(bytes: string | bigint | null | undefined, opts: { unknownCount?: number; atLeast?: boolean; className?: string } = {}): HTMLElement {
  return h('span', { class: `num${opts.className ? ` ${opts.className}` : ''}`, title: sizeTitle(bytes) }, sizeLabel(bytes, opts));
}

export type GateBadge = 'experimental' | 'off';

export const GATE_BADGE: Record<GateBadge, { label: string; tip: string }> = {
  experimental: { get label() { return t('ui.bits.gateBadge.experimental.label'); }, get tip() { return t('ui.bits.gateBadge.experimental.tip'); } },
  off: { get label() { return t('ui.bits.gateBadge.off.label'); }, get tip() { return t('ui.bits.gateBadge.off.tip'); } },
};

/** "实验功能" / "默认关闭" with the explanation as tooltip and for screen readers (no gate ids in the main text). */
export function gateBadge(kind: GateBadge, extraClass = ''): HTMLElement {
  const b = GATE_BADGE[kind];
  return h(
    'span',
    { class: `pill gate-pill${extraClass ? ` ${extraClass}` : ''}`, 'data-tone': kind === 'off' ? 'muted' : 'warn', title: b.tip },
    h('span', { class: 'pill-mark', 'aria-hidden': 'true' }),
    b.label,
    h('span', { class: 'sr-only' }, t('ui.bits.tip', { tip: b.tip })),
  );
}

/** Explanations that are not the page's main message. */
export function learnMore(...content: Child[]): HTMLDetailsElement {
  return h('details', { class: 'learn-more' }, h('summary', {}, t('ui.bits.learnMore')), h('div', { class: 'lm-body' }, ...content)) as HTMLDetailsElement;
}

/** Machine facts (codes, backend names, raw messages) for people who need them. */
export function techBlock(...content: Child[]): HTMLDetailsElement {
  return h('details', { class: 'tech' }, h('summary', {}, t('common.technicalDetails')), h('div', { class: 'tech-body' }, ...content)) as HTMLDetailsElement;
}

/** An empty state: what is missing and the one action that fills it. */
export function emptyBlock(o: { title: string; body?: string | null; actions?: Child[]; icon?: string; level?: 1 | 2; id?: string }): HTMLElement {
  const tag = o.level === 1 ? 'h1' : 'h2';
  return h(
    'div',
    { class: 'empty-state' },
    o.icon ? h('span', { class: 'es-icon', 'aria-hidden': 'true' }, icon(o.icon)) : null,
    h(tag, { id: o.id ?? null }, o.title),
    o.body ? h('p', { class: 'muted' }, o.body) : null,
    o.actions?.filter(Boolean).length ? h('div', { class: 'card-actions' }, ...o.actions) : null,
  );
}
