// P04 — cleanup candidates and the selection draft (GUI-03). Candidates come from the shared
// analysis of a published scan, grouped by risk with reasons, impact and sizes. Nothing is
// pre-selected; unknown sizes never count as 0 or rank as "largest"; blocked, protected and
// pending candidates explain why and cannot be selected. Creating a plan executes nothing.
//
// Order of the page (GUI-09): the summary — one sentence and every group's count and known size —
// then the groups, then the rows; the draft with the processing mode sits in the aside, and the
// shared selection bar carries the same count to the plan review.

import type { App } from '../main';
import { toIpcError } from '../lib/api';
import { formatExactBytes, parseBytes } from '../lib/bytes';
import { protectionText } from '../lib/codes';
import { sizeLabel } from '../lib/display';
import { h, icon, keepFocus, replace } from '../lib/dom';
import { Draft } from '../lib/draft';
import { COMPLETENESS_LABEL, formatTime } from '../lib/format';
import type { Candidate, CandidateSetInfo, Group, IpcError, Mode } from '../lib/types';
import { GROUP_EXPLAIN, GROUP_LABEL, MODE_LABEL, NOT_SELECTABLE_LABEL, PROBE_LABEL, PROBE_STATE_LABEL, REGISTRY_LABEL } from '../lib/words';
import { errorLine, errorText, exactTitle, groupMark, pageHead } from './parts';
import { atLeast, modeOptions, selectionBar, sumSizes, summaryStrip, tierBadge } from './kit';
import { emptyBlock, issueList, learnMore, pathText, stateLine, statusBanner } from './ui';
import { t } from '../i18n';

const PAGE = 50;
const GROUPS: Group[] = ['low', 'review', 'pending', 'blocked'];

/** Words of the cleanup page that are not data (one table for the language switch). */
export const CLEANUP_TEXT = {
  get title() { return t('cleanup.title'); },
  get candidates() { return t('cleanup.candidates'); },
  get noScanTitle() { return t('cleanup.noScanTitle'); },
  get noScanBody() { return t('cleanup.noScanBody'); },
  get chooseScope() { return t('cleanup.chooseScope'); },
  get unpublishedTitle() { return t('cleanup.unpublishedTitle'); },
  get unpublishedBody() { return t('cleanup.unpublishedBody'); },
  get viewProgress() { return t('cleanup.viewProgress'); },
  get loading() { return t('cleanup.loading'); },
  get noRoot() { return t('cleanup.noRoot'); },
  get analyze() { return t('cleanup.analyze'); },
  get analyzing() { return t('cleanup.analyzing'); },
  get analyzingLabel() { return t('cleanup.analyzingLabel'); },
  get reanalyze() { return t('cleanup.reanalyze'); },
  get notAnalyzedTitle() { return t('cleanup.notAnalyzedTitle'); },
  get notAnalyzedBody() { return t('cleanup.notAnalyzedBody'); },
  get notAnalyzedMore() { return t('cleanup.notAnalyzedMore'); },
  get noneTitle() { return t('cleanup.noneTitle'); },
  get noneBody() { return t('cleanup.noneBody'); },
  get browseDisk() { return t('cleanup.browseDisk'); },
  summary: (n: number) => t('cleanup.summary', { n }),
  summarySize: (size: string) => t('cleanup.summarySize', { size }),
  get summaryNote() { return t('cleanup.summaryNote'); },
  get summaryLabel() { return t('cleanup.summaryLabel'); },
  get dismiss() { return t('cleanup.dismiss'); },
  get rulesChanged() { return t('cleanup.rulesChanged'); },
  get partialScope() { return t('cleanup.partialScope'); },
  issues: (n: number) => t('cleanup.issues', { n }),
  get issuesLabel() { return t('cleanup.issuesLabel'); },
  unknownIn: (n: number) => t('cleanup.unknownIn', { n }),
  get selectLow() { return t('cleanup.selectLow'); },
  get groupEmpty() { return t('cleanup.groupEmpty'); },
  get loadingMore() { return t('cleanup.loadingMore'); },
  showMore: (shown: number, total: number) => t('cleanup.showMore', { shown, total }),
  get loadingRows() { return t('cleanup.loadingRows'); },
  get hideBasis() { return t('cleanup.hideBasis'); },
  get showBasis() { return t('cleanup.showBasis'); },
  get hintTag() { return t('cleanup.hintTag'); },
  get adapterTag() { return t('cleanup.adapterTag'); },
  get draft() { return t('cleanup.draft'); },
  get draftEmpty() { return t('cleanup.draftEmpty'); },
  draftCount: (n: number) => t('cleanup.draftCount', { n }),
  draftMore: (n: number) => t('cleanup.draftMore', { n }),
  get clear() { return t('cleanup.clear'); },
  get freezing() { return t('cleanup.freezing'); },
  get pickFirst() { return t('cleanup.pickFirst'); },
  get pickMode() { return t('cleanup.pickMode'); },
  create: (n: number) => t('cleanup.create', { n }),
  get createNote() { return t('cleanup.createNote'); },
} as const;

/** One short note per group in the summary strip. */
const GROUP_NOTE: Record<Group, string> = {
  get low() { return t('cleanup.groupNote.low'); },
  get review() { return t('cleanup.groupNote.review'); },
  get pending() { return t('cleanup.groupNote.pending'); },
  get blocked() { return t('cleanup.groupNote.blocked'); },
};

interface GroupView {
  items: Candidate[];
  next: string | null;
  total: number;
  loading: boolean;
  error: IpcError | null;
}

export class CleanupPage {
  el: HTMLElement;
  private body: HTMLElement;
  private scanId: string | null = null;
  private info: CandidateSetInfo | null = null;
  private loadingInfo = false;
  private analyzing = false;
  private infoError: IpcError | null = null;
  private groups = new Map<Group, GroupView>();
  private collapsed = new Set<Group>(['blocked']);
  private expanded = new Set<string>();
  private draft: Draft | null = null;
  private mode: Mode | null = null;
  private creating = false;
  private createError: IpcError | null = null;
  private notice: string | null = null;
  /** Rule the groups are limited to (opened from the cleanup catalog page); null = every rule. */
  private rule: string | null = null;

  constructor(private app: App) {
    this.body = h('div', { class: 'cleanup-body' });
    this.el = h('section', { class: 'page page-cleanup', 'aria-labelledby': 'cleanup-title' }, this.body);
  }

  activate(): void {
    this.syncRule();
    const id = this.app.store.state.scanId;
    if (id !== this.scanId || (!this.info && !this.loadingInfo)) void this.bind(id);
  }

  update(): void {
    this.syncRule();
    const id = this.app.store.state.scanId;
    if (id !== this.scanId) void this.bind(id);
    keepFocus(this.el, () => this.render());
  }

  // ---- data ------------------------------------------------------------------------------

  /** Follow the store's rule focus: every group is reloaded limited to that rule (or to every rule again). */
  private syncRule(): void {
    const rule = this.app.store.state.cleanupRule ?? null;
    if (rule === this.rule) return;
    this.rule = rule;
    this.groups.clear();
    this.expanded.clear();
    // Limited to one rule, every group is open: its blocked rows are part of what the rule found.
    this.collapsed = rule ? new Set<Group>() : new Set<Group>(['blocked']);
    const info = this.info;
    if (info) for (const g of GROUPS) if (info.groups[g].count > 0 && !this.collapsed.has(g)) void this.loadGroup(g);
  }

  /** A group's count and known size; under a rule focus the host's filtered total once a page arrived (null before). */
  private countOf(g: Group): { count: number | null; known: string | null; unknown: number } {
    const base = this.info!.groups[g];
    if (!this.rule || base.count === 0) return { count: base.count, known: base.known_logical_bytes, unknown: base.unknown_size_count };
    const v = this.groups.get(g);
    if (!v || v.total < 0) return { count: null, known: null, unknown: 0 };
    if (v.next) return { count: v.total, known: null, unknown: 0 };
    const sum = sumSizes(v.items.map((c) => ({ bytes: c.logical_known_bytes, unknown: c.unknown_size_count })));
    return { count: v.total, known: sum.known.toString(), unknown: sum.unknown };
  }

  private async bind(scanId: string | null): Promise<void> {
    this.scanId = scanId;
    this.info = null;
    this.infoError = null;
    this.groups.clear();
    this.draft = null;
    this.mode = null;
    this.expanded.clear();
    if (!scanId) return;
    this.loadingInfo = true;
    this.update();
    try {
      const info = await this.app.backend.candidateSummary(scanId);
      if (this.scanId !== scanId) return;
      this.setInfo(info);
    } catch (e) {
      if (this.scanId === scanId) this.infoError = toIpcError(e);
    } finally {
      this.loadingInfo = false;
      this.update();
    }
  }

  private setInfo(info: CandidateSetInfo | null): void {
    this.info = info;
    this.groups.clear();
    // A new candidate set never inherits an old selection (it may be a different index).
    this.draft = info ? new Draft(info.candidate_set_id) : null;
    this.mode = null;
    if (info) for (const g of GROUPS) if (info.groups[g].count > 0 && !this.collapsed.has(g)) void this.loadGroup(g);
  }

  private async analyze(): Promise<void> {
    if (!this.scanId || this.analyzing) return;
    const scanId = this.scanId;
    this.analyzing = true;
    this.infoError = null;
    this.update();
    this.app.announce(t('cleanup.analyzingScanResultsRules'));
    try {
      const info = await this.app.backend.analyzeScan(scanId);
      if (this.scanId !== scanId) return;
      this.setInfo(info);
      const total = GROUPS.reduce((a, g) => a + info.groups[g].count, 0);
      this.app.announce(t('cleanup.analysisCompleteTotalNothing', { total }));
    } catch (e) {
      this.infoError = toIpcError(e);
    } finally {
      this.analyzing = false;
      this.update();
    }
  }

  private async loadGroup(g: Group, more = false): Promise<void> {
    const info = this.info;
    if (!info) return;
    const rule = this.rule;
    // Under a rule focus the total is unknown (-1) until the host answers with the filtered one.
    const cur = this.groups.get(g) ?? { items: [], next: null, total: rule ? -1 : info.groups[g].count, loading: false, error: null };
    if (cur.loading) return;
    cur.loading = true;
    cur.error = null;
    this.groups.set(g, cur);
    this.update();
    try {
      const page = await this.app.backend.listCandidates(info.candidate_set_id, g, PAGE, more ? cur.next : null, rule);
      if (this.info !== info || this.rule !== rule) return;
      cur.items = more ? [...cur.items, ...page.items] : page.items;
      cur.next = page.next_cursor;
      cur.total = page.total;
    } catch (e) {
      cur.error = toIpcError(e);
    } finally {
      cur.loading = false;
      this.update();
    }
  }

  /** Explicit action: add every selectable low-risk candidate (all pages, known sizes only). */
  private async selectAllLow(): Promise<void> {
    const g = this.groups.get('low');
    while (g && g.next && !g.loading) await this.loadGroup('low', true);
    const items = this.groups.get('low')?.items ?? [];
    let added = 0;
    for (const c of items) if (this.draft && !this.draft.has(c.candidate_id) && this.draft.add(c)) added += 1;
    this.syncMode();
    this.app.announce(added ? t('cleanup.selectedAdded', { added }) : t('cleanup.noLowRiskItems'));
    this.update();
  }

  private toggle(c: Candidate): void {
    if (!this.draft) return;
    const on = this.draft.toggle(c);
    this.syncMode();
    this.app.announce(on ? t('common.selectedName', { name: c.name }) : t('common.deselectedName', { name: c.name }));
    this.update();
  }

  private syncMode(): void {
    const modes = this.draft?.modes() ?? [];
    if (this.mode && !modes.includes(this.mode)) this.mode = null;
    this.createError = null;
  }

  private async createPlan(): Promise<void> {
    if (!this.draft || !this.mode || this.creating) return;
    this.creating = true;
    this.createError = null;
    this.update();
    try {
      const r = await this.app.backend.createPlan(this.draft.setId, this.draft.items().map((c) => c.candidate_id), this.mode);
      this.app.announce(t('common.planCreatedNotRun'));
      this.app.openPlan(r.plan_id, 'cleanup');
    } catch (e) {
      this.createError = toIpcError(e);
    } finally {
      this.creating = false;
      this.update();
    }
  }

  private async protect(c: Candidate, what: 'entry' | 'rule'): Promise<void> {
    if (!this.scanId) return;
    try {
      const target =
        what === 'entry'
          ? ({ source: 'entry', scan_id: this.scanId, entry_id: c.entry_id, recursive: c.kind === 'directory' } as const)
          : ({ source: 'rule', rule_id: c.rules[0].split('@')[0] } as const);
      await this.app.backend.addProtection(target, null);
      this.draft?.remove(c.candidate_id);
      this.notice = what === 'entry' ? t('cleanup.addedNameProtectionList', { name: c.name }) : t('cleanup.protectedRulePAll', { p: c.rules[0].split('@')[0] });
    } catch (e) {
      this.notice = t('cleanup.couldntAddProtectionList', { errorText: errorText(toIpcError(e)) });
    }
    this.update();
  }

  // ---- rendering ---------------------------------------------------------------------------

  private render(): void {
    const st = this.app.store.state;
    const T = CLEANUP_TEXT;
    if (!st.scanId) {
      replace(
        this.body,
        h('h1', { id: 'cleanup-title' }, T.title),
        emptyBlock({ icon: 'tool-clean', title: T.noScanTitle, body: T.noScanBody, actions: [h('button', { type: 'button', class: 'primary', onclick: () => this.app.navigate('scope') }, T.chooseScope)] }),
      );
      return;
    }
    const status = st.status;
    if (status && !status.published) {
      replace(
        this.body,
        this.head(),
        h(
          'div',
          { class: 'ov-single' },
          statusBanner({ kind: 'running', level: 2, titleId: 'cleanup-state-title', title: T.unpublishedTitle, sentence: T.unpublishedBody, actions: [h('button', { type: 'button', onclick: () => this.app.navigate('overview') }, T.viewProgress)] }),
        ),
      );
      return;
    }
    if (this.loadingInfo && !this.info) {
      replace(this.body, this.head(), h('p', { class: 'muted', role: 'status' }, T.loading));
      return;
    }
    if (!this.info) {
      replace(this.body, this.head(), this.notAnalyzed());
      return;
    }
    const total = GROUPS.reduce((a, g) => a + this.info!.groups[g].count, 0);
    replace(
      this.body,
      this.head(),
      total === 0
        ? h(
            'div',
            { class: 'ov-single' },
            this.notices(),
            emptyBlock({ icon: 'tool-clean', title: T.noneTitle, body: T.noneBody, actions: [h('button', { type: 'button', onclick: () => this.app.navigate('disk') }, T.browseDisk)] }),
          )
        : h('div', { class: 'work-layout' }, h('div', { class: 'work-main' }, this.rule ? this.ruleFocus(this.rule) : this.summary(total), this.notices(), ...GROUPS.map((g) => this.groupSection(g))), this.draftPanel()),
      this.bar(),
    );
  }

  private head(): HTMLElement {
    const info = this.info;
    const status = this.app.store.state.status;
    const root = info?.scan_root_display ?? status?.root_display ?? null;
    const completeness = info?.scan_completeness ?? status?.completeness;
    const meta = [
      root ? pathText(root, { max: 48 }) : CLEANUP_TEXT.noRoot,
      info?.created_at ? t('cleanup.analyzedAt', { created_at: formatTime(info.created_at) }) : null,
      completeness ? (COMPLETENESS_LABEL[completeness] ?? '') : null,
    ];
    const reanalyze = info ? h('button', { type: 'button', id: 'cleanup-analyze', disabled: this.analyzing }, this.analyzing ? CLEANUP_TEXT.analyzing : CLEANUP_TEXT.reanalyze) : null;
    reanalyze?.addEventListener('click', () => void this.analyze());
    return pageHead({ id: 'cleanup-title', title: CLEANUP_TEXT.candidates, meta, actions: [reanalyze] });
  }

  private notAnalyzed(): HTMLElement {
    const T = CLEANUP_TEXT;
    const btn = h('button', { type: 'button', id: 'cleanup-analyze', class: 'primary', disabled: this.analyzing }, this.analyzing ? T.analyzing : T.analyze);
    btn.addEventListener('click', () => void this.analyze());
    return h(
      'section',
      { class: 'panel analyze-panel', 'aria-labelledby': 'analyze-title' },
      h('h2', { id: 'analyze-title' }, T.notAnalyzedTitle),
      h('p', {}, T.notAnalyzedBody),
      learnMore(h('p', {}, T.notAnalyzedMore)),
      this.analyzing ? h('div', { class: 'indeterminate', role: 'progressbar', 'aria-label': T.analyzingLabel, 'aria-valuetext': T.analyzingLabel }) : null,
      errorLine(this.infoError),
      h('div', { class: 'card-actions' }, btn),
    );
  }

  /** GUI-09 summary first: every group's count and known size, each jumping to its group. */
  private summary(total: number): HTMLElement {
    const info = this.info!;
    const T = CLEANUP_TEXT;
    const known = GROUPS.filter((g) => g === 'low' || g === 'review').reduce((a, g) => a + (parseBytes(info.groups[g].known_logical_bytes) ?? 0n), 0n);
    const unknown = GROUPS.filter((g) => g === 'low' || g === 'review').reduce((a, g) => a + info.groups[g].unknown_size_count, 0);
    return h(
      'section',
      { class: 'cand-summary', 'aria-labelledby': 'cand-sum-title' },
      h('h2', { id: 'cand-sum-title', class: 'cs-sentence' }, T.summary(total), h('span', { class: 'cs-size num' }, T.summarySize(sizeLabel(known, { unknownCount: unknown })))),
      h('p', { class: 'cs-note' }, T.summaryNote),
      summaryStrip(
        T.summaryLabel,
        GROUPS.map((g) => {
          const c = info.groups[g];
          return {
            mark: groupMark(g),
            count: c.count,
            size: c.count && (g === 'low' || g === 'review') ? sizeLabel(c.known_logical_bytes, { unknownCount: c.unknown_size_count }) : null,
            note: GROUP_NOTE[g],
            id: `jump-${g}`,
            onJump: () => this.jumpTo(g),
          };
        }),
      ),
    );
  }

  /** In place of the summary while the groups are limited to one rule: which rule, and the way back to every rule. */
  private ruleFocus(rule: string): HTMLElement {
    const all = h('button', { type: 'button', class: 'quiet', id: 'rule-focus-clear', onclick: () => this.app.store.set({ cleanupRule: null }) }, t('cleanup.ruleFocusAll'));
    const back = h('button', { type: 'button', class: 'quiet', id: 'rule-focus-catalog', onclick: () => this.app.navigate('rulecatalog') }, t('cleanup.ruleFocusCatalog'));
    return h(
      'section',
      { class: 'cand-summary rule-focus', 'aria-labelledby': 'rule-focus-title' },
      h('h2', { id: 'rule-focus-title', class: 'cs-sentence' }, t('cleanup.ruleFocus'), h('code', { class: 'selectable rule-focus-id' }, rule)),
      h('p', { class: 'cs-note' }, t('cleanup.ruleFocusNote')),
      h('div', { class: 'card-actions' }, all, back),
    );
  }

  private jumpTo(g: Group): void {
    if (this.collapsed.has(g)) {
      this.collapsed.delete(g);
      if (!this.groups.get(g) && this.info?.groups[g].count) void this.loadGroup(g);
      this.update();
    }
    requestAnimationFrame(() => {
      const t = document.getElementById(`group-toggle-${g}`);
      t?.scrollIntoView({ block: 'start', behavior: 'smooth' });
      t?.focus({ preventScroll: true });
    });
  }

  private notices(): HTMLElement {
    const info = this.info!;
    const T = CLEANUP_TEXT;
    const parts: (HTMLElement | null)[] = [];
    if (this.notice) {
      const close = h('button', { type: 'button', class: 'quiet', onclick: () => ((this.notice = null), this.update()) }, T.dismiss);
      parts.push(h('div', { class: 'notice', role: 'status' }, h('span', {}, this.notice), close));
    }
    if (info.rules_current === false) parts.push(stateLine({ kind: 'failed', text: T.rulesChanged }));
    if (info.scan_completeness === 'partial_scope') parts.push(stateLine({ kind: 'partial', text: T.partialScope }));
    if (this.infoError) parts.push(errorLine(this.infoError));
    if (info.issues.length) parts.push(h('details', { class: 'issues-details' }, h('summary', {}, T.issues(info.issues.length)), issueList(info.issues, { label: T.issuesLabel })));
    return h('div', { class: 'notices' }, ...parts);
  }

  private groupSection(g: Group): HTMLElement {
    const info = this.info!;
    const count = this.countOf(g);
    const view = this.groups.get(g);
    const open = !this.collapsed.has(g);
    const T = CLEANUP_TEXT;
    const toggle = h(
      'button',
      { type: 'button', class: 'group-toggle', id: `group-toggle-${g}`, 'aria-expanded': open ? 'true' : 'false', 'aria-controls': `group-body-${g}` },
      h('span', { class: 'chev-icon', 'aria-hidden': 'true' }, icon('chevron')),
      groupMark(g),
      h('span', { class: 'group-count num' }, count.count === null ? T.loadingRows : t('common.itemsCount', { count: count.count })),
      count.count && count.known !== null
        ? h(
            'span',
            { class: 'group-size num', title: exactTitle(count.known) },
            sizeLabel(count.known, { unknownCount: count.unknown }),
            count.unknown ? h('span', { class: 'group-unknown' }, T.unknownIn(count.unknown)) : null,
          )
        : null,
    );
    toggle.addEventListener('click', () => {
      if (open) this.collapsed.add(g);
      else {
        this.collapsed.delete(g);
        if (!this.groups.get(g) && info.groups[g].count) void this.loadGroup(g);
      }
      this.update();
    });
    const extra = g === 'low' && (count.count ?? 0) > 0 ? h('button', { type: 'button', class: 'quiet', id: 'select-low', onclick: () => void this.selectAllLow() }, T.selectLow) : null;
    const bodyEl = h(
      'div',
      { class: 'group-body', id: `group-body-${g}`, hidden: !open },
      h('p', { class: 'group-explain' }, GROUP_EXPLAIN[g]),
      count.count === 0 ? h('p', { class: 'muted small' }, T.groupEmpty) : this.table(g, view),
      view?.error ? errorLine(view.error) : null,
      view?.next
        ? h('button', { type: 'button', class: 'quiet more', id: `more-${g}`, disabled: view.loading, onclick: () => void this.loadGroup(g, true) }, view.loading ? T.loadingMore : T.showMore(view.items.length, view.total))
        : null,
    );
    return h('section', { class: 'panel cgroup', 'data-group': g, 'aria-labelledby': `group-toggle-${g}` }, h('div', { class: 'cgroup-head' }, h('h2', {}, toggle), extra), bodyEl);
  }

  private table(g: Group, view: GroupView | undefined): HTMLElement {
    const T = CLEANUP_TEXT;
    if (!view || (view.loading && !view.items.length)) return h('p', { class: 'muted small', role: 'status' }, T.loadingRows);
    const rows: HTMLElement[] = [];
    for (const c of view.items) {
      const id = `cand-${c.candidate_id}`;
      const selectable = c.not_selectable === null && c.allowed_modes.length > 0;
      const check = h('input', {
        type: 'checkbox',
        id: `${id}-check`,
        disabled: !selectable,
        'aria-describedby': `${id}-why`,
        'aria-label': t('common.selectName', { name: c.name }),
      }) as HTMLInputElement;
      check.checked = !!this.draft?.has(c.candidate_id);
      check.addEventListener('change', () => this.toggle(c));
      const open = this.expanded.has(c.candidate_id);
      const more = h('button', { type: 'button', class: 'link-btn', id: `${id}-more`, 'aria-expanded': open ? 'true' : 'false', 'aria-controls': `${id}-detail` }, open ? T.hideBasis : T.showBasis);
      more.addEventListener('click', () => {
        if (open) this.expanded.delete(c.candidate_id);
        else this.expanded.add(c.candidate_id);
        this.update();
      });
      const size = h(
        'td',
        { class: 'col-size num', title: exactTitle(c.logical_known_bytes) },
        sizeLabel(c.logical_known_bytes, { unknownCount: c.unknown_size_count }),
        c.unknown_size_count ? h('small', {}, T.unknownIn(c.unknown_size_count)) : null,
      );
      const why = selectable
        ? h('span', { id: `${id}-why`, class: 'modes-line' }, c.allowed_modes.map((m) => MODE_LABEL[m]).join(t('common.listSep')))
        : h('span', { id: `${id}-why`, class: 'why-not' }, NOT_SELECTABLE_LABEL[c.not_selectable ?? 'no_mode']);
      rows.push(
        h(
          'tr',
          { class: `crow${check.checked ? ' selected' : ''}${selectable ? '' : ' locked'}` },
          h('td', { class: 'col-check' }, check),
          h(
            'td',
            { class: 'col-object' },
            h('label', { for: `${id}-check`, class: 'obj-name' }, c.name),
            pathText(c.path_display, { max: 64, className: 'obj-path' }),
            h('span', { class: 'obj-reason' }, c.row_kind === 'hint' ? h('span', { class: 'row-tag' }, T.hintTag) : null, c.row_kind === 'adapter' ? h('span', { class: 'row-tag' }, T.adapterTag) : null, c.reasons[0] ?? c.rules[0]),
          ),
          size,
          h('td', { class: 'col-status' }, tierBadge(c.tier), why, more),
        ),
      );
      if (open) rows.push(h('tr', { class: 'cdetail', id: `${id}-detail` }, h('td', { colspan: '4' }, this.detail(c))));
    }
    return h(
      'div',
      { class: 'table-wrap' },
      h(
        'table',
        { class: 'data-table ctable' },
        h('caption', { class: 'sr-only' }, t('cleanup.pCandidatesSortedKnown', { p: GROUP_LABEL[g] })),
        h('thead', {}, h('tr', {}, h('th', { scope: 'col', class: 'col-check' }, h('span', { class: 'sr-only' }, t('common.select'))), h('th', { scope: 'col' }, t('common.objectBasis')), h('th', { scope: 'col', class: 'col-size' }, t('common.logicalSize')), h('th', { scope: 'col' }, t('cleanup.availableActions')))),
        h('tbody', {}, ...rows),
      ),
    );
  }

  private detail(c: Candidate): HTMLElement {
    const protectEntry = h('button', { type: 'button', class: 'quiet', onclick: () => void this.protect(c, 'entry') }, icon('shield'), t('cleanup.protectLocation'));
    const protectRule = h('button', { type: 'button', class: 'quiet', onclick: () => void this.protect(c, 'rule') }, icon('shield'), t('cleanup.protectWholeRule'));
    return h(
      'div',
      { class: 'cdetail-body' },
      h(
        'dl',
        { class: 'facts-list' },
        h('dt', {}, t('common.rules')),
        h('dd', {}, h('code', { class: 'selectable' }, c.rules.join(t('common.commaSep')))),
        h('dt', {}, t('cleanup.reasons')),
        h('dd', {}, c.reasons.length ? c.reasons.join(t('common.clauseSep')) : t('cleanup.ruleMatch')),
        h('dt', {}, t('common.impact')),
        h('dd', {}, c.impact || t('cleanup.ruleGivesNoImpact')),
        c.protection ? h('dt', {}, t('cleanup.protection')) : null,
        c.protection ? h('dd', {}, protectionText(c.protection.class), c.protection.reviewable ? t('cleanup.reviewItemItemOnly') : '') : null,
        c.unknown_size_count ? h('dt', {}, t('common.unknownSize')) : null,
        c.unknown_size_count ? h('dd', {}, t('cleanup.unknownSizeCount', { unknown_size_count: c.unknown_size_count })) : null,
        c.registry_status ? h('dt', {}, t('cleanup.testedVersions')) : null,
        c.registry_status ? h('dd', {}, REGISTRY_LABEL[c.registry_status] ?? c.registry_status, c.installed_version ? h('small', {}, t('cleanup.installedVersionInstalledVersion', { installed_version: c.installed_version })) : null) : null,
        c.probes.length ? h('dt', {}, t('cleanup.useCheck')) : null,
        c.probes.length
          ? h(
              'dd',
              {},
              c.probe_results.length
                ? h('ul', { class: 'probe-list' }, ...c.probe_results.map((r) => h('li', {}, h('span', { class: 'probe-state', 'data-state': r.state }, PROBE_STATE_LABEL[r.state] ?? r.state), ` ${PROBE_LABEL[r.probe] ?? r.probe}`)))
                : c.catalog_fresh
                  ? t('cleanup.analysisDidntRunCheck')
                  : t('cleanup.analyzeAgainGetRuns'),
              h('small', {}, t('cleanup.eachItemCheckedAgain')),
            )
          : null,
        c.row_kind === 'adapter' ? h('dt', {}, t('cleanup.howCleaned')) : null,
        c.row_kind === 'adapter' ? h('dd', {}, t('cleanup.cleanedOwnToolP', { p: c.adapter_ref ?? t('common.unknownLower') })) : null,
        c.row_kind === 'hint' ? h('dt', {}, t('cleanup.hint')) : null,
        c.row_kind === 'hint' ? h('dd', {}, t('cleanup.largeItemHintHelp')) : null,
      ),
      h('div', { class: 'card-actions' }, protectEntry, c.rules.length ? protectRule : null, this.diagButton(c)),
    );
  }

  /** Optional AI diagnosis of this candidate's object; the plan path stays on this page's rules. */
  private diagButton(c: Candidate): HTMLElement | null {
    const scanId = this.app.store.state.scanId;
    if (!scanId) return null;
    const name = c.name;
    return h('button', { type: 'button', class: 'quiet', id: `diag-${c.candidate_id}`, onclick: () => this.app.openDiagnosis({ scanId, entryId: c.entry_id, origin: 'cleanup', name }, 'cleanup') }, icon('diag'), t('common.aiDiagnosis'));
  }

  /** The shared bottom selection bar (GUI-09): count, known size, unknown count, next step. */
  private bar(): HTMLElement {
    const items = this.draft?.items() ?? [];
    return selectionBar({
      count: items.length,
      sum: sumSizes(items.map((c) => ({ bytes: c.logical_known_bytes, unknown: c.unknown_size_count }))),
      onClear: () => {
        this.draft?.clear();
        this.syncMode();
        this.app.announce(t('common.selectionCleared'));
        this.update();
      },
      action: {
        id: 'cleanup-bar-go',
        text: this.mode ? t('common.createPlanReview') : t('common.chooseAction'),
        disabled: this.creating,
        danger: this.mode === 'delete',
        onClick: () => {
          if (this.mode) void this.createPlan();
          else document.querySelector<HTMLInputElement>('input[name="plan-mode"]:not(:disabled)')?.focus();
        },
      },
    });
  }

  private draftPanel(): HTMLElement {
    const T = CLEANUP_TEXT;
    const d = this.draft;
    const n = d?.size ?? 0;
    const modes = d?.modes() ?? [];
    const create = h(
      'button',
      { type: 'button', class: `${this.mode === 'delete' ? 'danger' : 'primary'} wide`, id: 'create-plan', disabled: n === 0 || !this.mode || this.creating },
      this.creating ? T.freezing : n === 0 ? T.pickFirst : !this.mode ? T.pickMode : T.create(n),
    );
    create.addEventListener('click', () => void this.createPlan());
    const list = d?.items() ?? [];
    const sum = sumSizes(list.map((c) => ({ bytes: c.logical_known_bytes, unknown: c.unknown_size_count })));
    return h(
      'aside',
      { class: 'panel work-aside draft', 'aria-labelledby': 'draft-title' },
      h('h2', { id: 'draft-title' }, T.draft),
      n === 0
        ? h('p', { class: 'muted small' }, T.draftEmpty)
        : h(
            'div',
            { class: 'draft-sum' },
            h('p', { class: 'draft-figure num', title: formatExactBytes(sum.known) }, atLeast(sum)),
            h('p', { class: 'muted small' }, T.draftCount(n)),
            h(
              'ul',
              { class: 'draft-list' },
              ...list.slice(0, 5).map((c) =>
                h('li', {}, h('span', { class: 'path' }, c.name), h('button', { type: 'button', class: 'icon-btn small-btn', 'aria-label': t('cleanup.removeName', { name: c.name }), onclick: () => this.toggle(c) }, icon('close'))),
              ),
              list.length > 5 ? h('li', { class: 'muted small' }, T.draftMore(list.length - 5)) : null,
            ),
            h('button', { type: 'button', class: 'quiet', onclick: () => (this.draft?.clear(), this.syncMode(), this.update()) }, T.clear),
          ),
      modeOptions({
        name: 'plan-mode',
        id: (m) => `mode-${m}`,
        allowed: modes,
        current: this.mode,
        selected: n,
        onChange: (m) => {
          this.mode = m;
          this.createError = null;
          this.update();
        },
      }),
      errorLine(this.createError),
      create,
      h('p', { class: 'muted small aside-note' }, T.createNote),
    );
  }
}
