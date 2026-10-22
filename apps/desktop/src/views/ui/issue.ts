// Issue lines: the Chinese explanation is always the main text; the code and the host's raw (English)
// message sit in a collapsed "技术详情". Issues with the same meaning are one line with a count.

import { groupIssues, type ExplainedIssue, type IssueGroup } from '../../lib/codes';
import { groupDigits } from '../../lib/bytes';
import { h, icon } from '../../lib/dom';
import type { Issue } from '../../lib/types';
import { pathText } from './path';

const SHOWN_SCOPES = 3;

function markOf(g: IssueGroup): string {
  if (g.issue.cause === 'privacy_prompt_pending') return 'pending';
  if (g.issue.code === 'permission_required' || g.issue.code === 'permission_denied') return 'lock';
  if (!g.issue.known) return 'unknown';
  return 'warn';
}

/** One group of issues as a list item. */
export function issueLine(g: IssueGroup, opts: { pathMax?: number } = {}): HTMLElement {
  const mark = markOf(g);
  const scopes = g.scopes.slice(0, SHOWN_SCOPES);
  const more = g.scopes.length - scopes.length;
  return h(
    'li',
    { class: 'issue-line', 'data-mark': mark, 'data-code': g.issue.code },
    h('span', { class: 'il-mark', 'aria-hidden': 'true' }, mark === 'lock' ? icon('lock') : mark === 'pending' ? icon('hourglass') : icon('warn')),
    h(
      'div',
      { class: 'il-body' },
      h('p', { class: 'il-text' }, g.issue.text, g.count > 1 ? h('span', { class: 'il-count num' }, `${groupDigits(g.count)} 处`) : null),
      scopes.length
        ? h(
            'ul',
            { class: 'il-scopes', 'aria-label': '涉及的位置' },
            ...scopes.map((s) => h('li', {}, pathText(s, { max: opts.pathMax ?? 64 }))),
            more > 0 ? h('li', { class: 'muted' }, `另有 ${groupDigits(more)} 处`) : null,
          )
        : null,
      techDetails(g),
    ),
  );
}

function techDetails(g: IssueGroup): HTMLElement {
  const raws = g.raws.slice(0, 5);
  return h(
    'details',
    { class: 'tech' },
    h('summary', {}, '技术详情'),
    h(
      'dl',
      { class: 'tech-list' },
      h('dt', {}, '代码'),
      h('dd', {}, h('code', { class: 'selectable' }, g.issue.rawCode), g.issue.cause ? h('code', { class: 'selectable' }, ` permission_cause=${g.issue.cause}`) : null),
      ...raws.flatMap((r) => [h('dt', {}, '系统消息'), h('dd', { class: 'selectable' }, r.message || '（无）')]),
    ),
    g.raws.length > raws.length ? h('p', { class: 'muted small' }, `另有 ${groupDigits(g.raws.length - raws.length)} 条相同代码的消息。`) : null,
  );
}

/**
 * A list of issues, grouped by meaning. `explain` picks the code table: scan and inventory issues by
 * default, `explainResult` for the per-item results of an execution.
 */
export function issueList(issues: Issue[], opts: { label: string; pathMax?: number; explain?: (i: Issue) => ExplainedIssue }): HTMLElement {
  return h('ul', { class: 'issue-lines', 'aria-label': opts.label }, ...groupIssues(issues, opts.explain).map((g) => issueLine(g, opts)));
}
