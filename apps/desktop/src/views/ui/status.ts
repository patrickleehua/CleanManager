// The status banner: the one primary message of a page — a shape, a headline with the state word in it,
// one sentence and the next step. The shape repeats the word; colour only reinforces it.

import type { BannerKind } from '../../lib/display';
import { h } from '../../lib/dom';

type Child = Node | string | null | undefined | false;

export type { BannerKind };

/** The state word a banner kind stands for (read by screen readers before the headline when the headline omits it). */
export const BANNER_WORD: Record<BannerKind, string> = {
  success: '完成',
  partial: '部分完成',
  failed: '失败',
  cancelled: '已停止',
  running: '进行中',
  'pending-permission': '等待授权',
};

export interface BannerOpts {
  kind: BannerKind;
  title: string;
  /** One short sentence: what it means or what to do. */
  sentence?: string | null;
  /** Small facts under the sentence (path, time). */
  meta?: Child[];
  /** Extra content between the sentence and the actions (a progress bar). */
  extra?: Child[];
  actions?: Child[];
  /** The headline's id (the page section is labelled by it). */
  titleId: string;
  /** `h1` when the banner is the page's own heading. */
  level?: 1 | 2;
  /** Screen-reader prefix of the headline, e.g. "概览：". */
  srPrefix?: string;
}

export function statusBanner(o: BannerOpts): HTMLElement {
  const tag = o.level === 2 ? 'h2' : 'h1';
  return h(
    'div',
    { class: 'status-banner', 'data-kind': o.kind },
    h('span', { class: 'sb-mark', 'aria-hidden': 'true' }),
    h(
      'div',
      { class: 'sb-body' },
      h(tag, { id: o.titleId, class: 'sb-title' }, o.srPrefix ? h('span', { class: 'sr-only' }, o.srPrefix) : null, o.title),
      o.sentence ? h('p', { class: 'sb-sentence' }, o.sentence) : null,
      o.meta?.filter(Boolean).length ? h('p', { class: 'sb-meta' }, ...o.meta.filter(Boolean).map((m) => h('span', {}, m as Node | string))) : null,
      ...(o.extra ?? []),
      o.actions?.filter(Boolean).length ? h('div', { class: 'sb-actions' }, ...o.actions) : null,
    ),
  );
}
