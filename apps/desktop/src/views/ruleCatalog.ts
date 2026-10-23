// 清理 › 清理目录 (WFA-49): the cleanup catalog `cleanmanager clean list` prints, for people. Every rule of this
// platform grouped by category, with its own sentence, where it looks on this machine (home written %USERPROFILE% / ~,
// existence checked only), its risk ceiling, how its objects are processed and got back, whether it applies here and
// why, and a size only when a stored analysis of the current scan already found its objects ("未知" otherwise, never 0).
// Filters (category, risk, applicability) and a text search narrow the list. The page is read-only: it never scans,
// analyzes, selects or creates a plan. A rule with candidates in that analysis links to 清理 › 规则清理 limited to it.

import type { App } from '../main';
import { h, icon, keepFocus, replace } from '../lib/dom';
import { formatTime } from '../lib/format';
import { RequestGate } from '../lib/gate';
import type { IpcError, RuleCatalog, RuleCatalogEntry, Tier } from '../lib/types';
import {
  ACTION_KIND_LABEL,
  APPLICABILITY_LABEL,
  APPLICABILITY_TONE,
  LOCATION_STATE_LABEL,
  NO_FILTER,
  REBUILD_LABEL,
  REGISTRY_TEXT,
  RULE_STATUS_LABEL,
  canShowCandidates,
  catalogCounts,
  categoryLabel,
  domId,
  emptyCategories,
  estimateText,
  filterCatalog,
  isFiltered,
  reasonText,
  recoveryText,
  type ApplicabilityFilter,
  type CatalogFilter,
  type CatalogGroup,
  type RiskFilter,
} from '../lib/ruleCatalogModel';
import { TIER_LABEL, tierBadge, toIpcError } from './kit';
import { emptyState, errorLine, pageHead, pill } from './parts';
import { learnMore, pathText, stateLine, techBlock } from './ui';
import { t } from '../i18n';

const TIERS: Tier[] = ['default_selected', 'review_only', 'blocked'];
const APPLICABILITIES = ['applicable', 'not_present', 'not_applicable', 'unsupported'] as const;

export class RuleCatalogPage {
  el: HTMLElement;
  private headEl: HTMLElement;
  private controlsEl: HTMLElement;
  private resultsEl: HTMLElement;
  private statusEl: HTMLElement;
  private catalog: RuleCatalog | null = null;
  private error: IpcError | null = null;
  private loading = false;
  private loadedFor: string | null | undefined = undefined;
  private filter: CatalogFilter = { ...NO_FILTER };
  private collapsed = new Set<string>();
  private expanded = new Set<string>();
  private gate = new RequestGate();
  private searchTimer: number | null = null;

  constructor(private app: App) {
    this.headEl = h('div', {});
    this.controlsEl = h('div', { class: 'rcat-controls', role: 'search' });
    this.statusEl = h('p', { class: 'rcat-count muted small', role: 'status', 'aria-live': 'polite' });
    this.resultsEl = h('div', { class: 'rcat-results' });
    this.el = h('section', { class: 'page page-rulecatalog', 'aria-labelledby': 'rcat-title' }, this.headEl, this.controlsEl, this.statusEl, this.resultsEl);
  }

  activate(): void {
    const scan = this.app.store.state.scanId;
    if (this.loadedFor !== scan && !this.loading) void this.load();
  }

  update(): void {
    const scan = this.app.store.state.scanId;
    if (this.loadedFor !== undefined && this.loadedFor !== scan && !this.loading) void this.load();
    keepFocus(this.el, () => this.render());
  }

  relocalize(): void {
    this.renderControls();
  }

  // ---- data ------------------------------------------------------------------------------

  private async load(): Promise<void> {
    const scan = this.app.store.state.scanId;
    this.loading = true;
    this.error = null;
    this.loadedFor = scan;
    this.update();
    try {
      const r = await this.gate.latest(this.app.backend.cleanCatalog(scan));
      if (r.stale) return;
      this.catalog = r.value;
      // A filter on a category this pack no longer has would hide everything without saying why.
      if (this.filter.category !== 'all' && !r.value.categories.some((c) => c.category === this.filter.category && c.rules.length)) this.filter.category = 'all';
      this.renderControls();
    } catch (e) {
      this.error = toIpcError(e);
    } finally {
      this.loading = false;
      this.update();
    }
  }

  private setFilter(patch: Partial<CatalogFilter>): void {
    this.filter = { ...this.filter, ...patch };
    this.renderResults();
  }

  // ---- rendering ---------------------------------------------------------------------------

  private render(): void {
    this.renderHead();
    if (!this.controlsEl.childElementCount && this.catalog) this.renderControls();
    this.renderResults();
  }

  private renderHead(): void {
    const c = this.catalog;
    const n = c ? catalogCounts(c) : null;
    const refresh = h('button', { type: 'button', id: 'rcat-refresh', disabled: this.loading }, icon('refresh'), this.loading ? t('ruleCatalog.reading') : t('ruleCatalog.readAgain'));
    refresh.addEventListener('click', () => void this.load());
    replace(
      this.headEl,
      pageHead({
        id: 'rcat-title',
        title: t('ruleCatalog.title'),
        meta: n
          ? [t('ruleCatalog.metaRules', { rules: n.rules, categories: n.categoriesWithRules }), t('ruleCatalog.metaApplicable', { n: n.applicable }), t('ruleCatalog.readOnly')]
          : [t('ruleCatalog.readOnly')],
        actions: [refresh],
      }),
      h('p', { class: 'lede' }, t('ruleCatalog.lede')),
      learnMore(h('p', {}, t('ruleCatalog.more.locations')), h('p', {}, t('ruleCatalog.more.sizes')), h('p', {}, t('ruleCatalog.more.cli'))),
      errorLine(this.error),
      c ? this.sizeSource(c) : null,
    );
  }

  /** Where the sizes come from: the stored analysis of the current scan, or nowhere (every size unknown). */
  private sizeSource(c: RuleCatalog): HTMLElement {
    const src = c.estimate_source;
    if (!src) {
      const go = h('button', { type: 'button', class: 'quiet', id: 'rcat-to-cleanup', onclick: () => this.app.navigate('cleanup') }, t('ruleCatalog.toCleanup'));
      return h('div', { class: 'notice rcat-source' }, h('span', {}, this.app.store.state.scanId ? t('ruleCatalog.sizesNoAnalysis') : t('ruleCatalog.sizesNoScan')), go);
    }
    const text = src.scan_root_display
      ? t('ruleCatalog.sizesFrom', { time: formatTime(src.analyzed_at), root: src.scan_root_display })
      : t('ruleCatalog.sizesFromNoRoot', { time: formatTime(src.analyzed_at) });
    return h(
      'div',
      { class: 'rcat-source' },
      h('p', { class: 'muted small' }, text),
      src.rules_current ? null : stateLine({ kind: 'partial', text: t('ruleCatalog.sizesOldRules') }),
    );
  }

  private renderControls(): void {
    const c = this.catalog;
    if (!c) {
      replace(this.controlsEl);
      return;
    }
    const search = h('input', {
      type: 'search',
      id: 'rcat-search',
      placeholder: t('ruleCatalog.searchPlaceholder'),
      'aria-label': t('ruleCatalog.searchLabel'),
      'aria-controls': 'rcat-results',
      value: this.filter.query,
    }) as HTMLInputElement;
    search.addEventListener('input', () => {
      if (this.searchTimer !== null) window.clearTimeout(this.searchTimer);
      this.searchTimer = window.setTimeout(() => this.setFilter({ query: search.value }), 120);
    });
    search.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && search.value) {
        e.preventDefault();
        search.value = '';
        this.setFilter({ query: '' });
      }
    });
    const select = (id: string, label: string, options: [string, string][], value: string, on: (v: string) => void) => {
      const el = h('select', { id, 'aria-controls': 'rcat-results' }, ...options.map(([v, text]) => h('option', { value: v }, text))) as HTMLSelectElement;
      el.value = value;
      el.addEventListener('change', () => on(el.value));
      return h('label', { class: 'rcat-field', for: id }, h('span', { class: 'rcat-field-label' }, label), el);
    };
    const withRules = c.categories.filter((x) => x.rules.length);
    replace(
      this.controlsEl,
      h('label', { class: 'rcat-field rcat-search', for: 'rcat-search' }, h('span', { class: 'rcat-field-label' }, t('ruleCatalog.searchField')), search),
      select('rcat-category', t('ruleCatalog.filterCategory'), [['all', t('ruleCatalog.allCategories')], ...withRules.map((x) => [x.category, categoryLabel(x)] as [string, string])], this.filter.category, (v) => this.setFilter({ category: v })),
      select('rcat-risk', t('ruleCatalog.filterRisk'), [['all', t('ruleCatalog.allRisks')], ...TIERS.map((x) => [x, tierWord(x)] as [string, string])], this.filter.risk, (v) =>
        this.setFilter({ risk: v as RiskFilter }),
      ),
      select('rcat-applies', t('ruleCatalog.filterApplicability'), [['all', t('ruleCatalog.allApplicability')], ...APPLICABILITIES.map((x) => [x, APPLICABILITY_LABEL[x]] as [string, string])], this.filter.applicability, (v) =>
        this.setFilter({ applicability: v as ApplicabilityFilter }),
      ),
    );
  }

  private renderResults(): void {
    const c = this.catalog;
    if (!c) {
      replace(this.statusEl, this.loading ? t('ruleCatalog.readingCatalog') : '');
      replace(this.resultsEl);
      return;
    }
    const groups = filterCatalog(c, this.filter);
    const shown = groups.reduce((a, g) => a + g.rules.length, 0);
    const total = catalogCounts(c).rules;
    replace(this.statusEl, isFiltered(this.filter) ? t('ruleCatalog.shownOf', { shown, total }) : t('ruleCatalog.shownAll', { total }));
    const clear = h('button', { type: 'button', class: 'quiet', id: 'rcat-clear', onclick: () => this.clearFilters() }, t('ruleCatalog.clearFilters'));
    const empty = emptyCategories(c);
    replace(
      this.resultsEl,
      h(
        'div',
        { id: 'rcat-results' },
        isFiltered(this.filter) && shown ? h('div', { class: 'rcat-filtered' }, clear) : null,
        shown ? null : h('div', { class: 'panel' }, emptyState(t('ruleCatalog.noMatchTitle'), total ? t('ruleCatalog.noMatchBody') : t('ruleCatalog.noRulesBody'), total ? clear : null)),
        ...groups.map((g) => this.group(g)),
        empty.length && !isFiltered(this.filter) ? h('p', { class: 'muted small rcat-empty-cats' }, t('ruleCatalog.emptyCategories', { list: empty.map((x) => categoryLabel(x)).join(t('common.listSep')) })) : null,
      ),
    );
  }

  private clearFilters(): void {
    this.filter = { ...NO_FILTER };
    this.renderControls();
    this.renderResults();
    this.app.announce(t('ruleCatalog.filtersCleared'));
    (document.getElementById('rcat-search') as HTMLInputElement | null)?.focus();
  }

  private group(g: CatalogGroup): HTMLElement {
    const id = `rcat-g-${g.category.category}`;
    const open = !this.collapsed.has(g.category.category);
    const toggle = h(
      'button',
      { type: 'button', class: 'group-toggle', id: `${id}-toggle`, 'aria-expanded': open ? 'true' : 'false', 'aria-controls': `${id}-body` },
      h('span', { class: 'chev-icon', 'aria-hidden': 'true' }, icon('chevron')),
      h('span', { class: 'group-title' }, categoryLabel(g.category)),
      h('span', { class: 'group-count num' }, t('ruleCatalog.ruleCount', { n: g.rules.length })),
      g.category.status === 'unverified' ? pill(t('ruleCatalog.categoryUnverified'), 'warn') : null,
    );
    toggle.addEventListener('click', () => {
      if (open) this.collapsed.add(g.category.category);
      else this.collapsed.delete(g.category.category);
      keepFocus(this.el, () => this.renderResults());
    });
    return h(
      'section',
      { class: 'panel cgroup rcat-group', 'aria-labelledby': `${id}-toggle` },
      h('div', { class: 'cgroup-head' }, h('h2', {}, toggle)),
      h('div', { class: 'group-body', id: `${id}-body`, hidden: !open }, this.table(g)),
    );
  }

  private table(g: CatalogGroup): HTMLElement {
    const rows: HTMLElement[] = [];
    for (const e of g.rules) {
      const id = `rcat-${domId(e.rule_id)}`;
      const open = this.expanded.has(e.rule_id);
      const more = h('button', { type: 'button', class: 'link-btn', id: `${id}-more`, 'aria-expanded': open ? 'true' : 'false', 'aria-controls': `${id}-detail` }, open ? t('ruleCatalog.hideDetails') : t('ruleCatalog.details'));
      more.addEventListener('click', () => {
        if (open) this.expanded.delete(e.rule_id);
        else this.expanded.add(e.rule_id);
        keepFocus(this.el, () => this.renderResults());
      });
      rows.push(
        h(
          'tr',
          { class: `crow rcat-row${e.applicability === 'applicable' ? '' : ' locked'}` },
          h(
            'td',
            { class: 'col-object' },
            h('span', { class: 'obj-name' }, h('code', { class: 'rcat-id selectable' }, e.rule_id)),
            h('span', { class: 'obj-reason', lang: 'en' }, e.summary),
            this.locations(e),
          ),
          h(
            'td',
            { class: 'col-status rcat-applies' },
            pill(APPLICABILITY_LABEL[e.applicability], APPLICABILITY_TONE[e.applicability]),
            h('span', { class: 'small muted block' }, reasonText(e.applicability_reason)),
          ),
          h('td', { class: 'col-status rcat-risk' }, tierBadge(e.effective_tier_ceiling), h('span', { class: 'small muted block' }, recoveryText(e))),
          h('td', { class: 'col-size num', title: e.estimate ? null : t('ruleCatalog.sizeUnknownTip') }, estimateText(e), e.estimate ? h('small', { class: 'block muted' }, t('ruleCatalog.objects', { n: e.estimate.objects })) : null, more),
        ),
      );
      if (open) rows.push(h('tr', { class: 'cdetail', id: `${id}-detail` }, h('td', { colspan: '4' }, this.detail(e, id))));
    }
    return h(
      'div',
      { class: 'table-wrap' },
      h(
        'table',
        { class: 'data-table ctable rcat-table' },
        h('caption', { class: 'sr-only' }, t('ruleCatalog.caption', { category: categoryLabel(g.category) })),
        h(
          'thead',
          {},
          h(
            'tr',
            {},
            h('th', { scope: 'col' }, t('ruleCatalog.colRule')),
            h('th', { scope: 'col', class: 'col-status' }, t('ruleCatalog.colApplies')),
            h('th', { scope: 'col', class: 'col-status' }, t('ruleCatalog.colRisk')),
            h('th', { scope: 'col', class: 'col-size' }, t('ruleCatalog.colSize')),
          ),
        ),
        h('tbody', {}, ...rows),
      ),
    );
  }

  private locations(e: RuleCatalogEntry): HTMLElement {
    if (!e.locations.length) return h('span', { class: 'obj-path' }, t('ruleCatalog.noLocations'));
    return h(
      'ul',
      { class: 'rcat-locs', 'aria-label': t('ruleCatalog.locationsLabel') },
      ...e.locations.slice(0, 3).map((l) =>
        h('li', { 'data-state': l.state }, h('span', { class: 'rcat-loc-state' }, LOCATION_STATE_LABEL[l.state]), pathText(e.pattern ? `${l.display}${sep(l.display)}${e.pattern}` : l.display, { max: 72, className: 'obj-path' })),
      ),
      e.locations.length > 3 ? h('li', { class: 'muted small' }, t('ruleCatalog.moreLocations', { n: e.locations.length - 3 })) : null,
    );
  }

  private detail(e: RuleCatalogEntry, id: string): HTMLElement {
    const scanId = this.app.store.state.scanId;
    const link = this.catalog && canShowCandidates(e, this.catalog, scanId)
      ? h('button', { type: 'button', class: 'quiet', id: `${id}-cands`, onclick: () => this.openCandidates(e) }, icon('tool-clean'), t('ruleCatalog.showCandidates', { n: e.estimate!.objects }))
      : null;
    return h(
      'div',
      { class: 'cdetail-body' },
      h(
        'dl',
        { class: 'facts-list' },
        h('dt', {}, t('ruleCatalog.factStatus')),
        h('dd', {}, RULE_STATUS_LABEL[e.status], h('small', {}, t('ruleCatalog.factVersion', { version: e.version }))),
        h('dt', {}, t('ruleCatalog.factRisk')),
        h('dd', {}, tierBadge(e.effective_tier_ceiling), e.effective_tier_ceiling !== e.max_tier ? h('small', {}, t('ruleCatalog.factRuleMax', { tier: tierWord(e.max_tier) })) : null),
        h('dt', {}, t('ruleCatalog.factHow')),
        h('dd', {}, ACTION_KIND_LABEL[e.action_kind], h('small', {}, recoveryText(e))),
        h('dt', {}, t('ruleCatalog.factRebuild')),
        h('dd', {}, REBUILD_LABEL[e.rebuild_class] ?? e.rebuild_class, e.regeneration ? h('small', { lang: 'en' }, e.regeneration) : null),
        e.offline_effect ? h('dt', {}, t('ruleCatalog.factOffline')) : null,
        e.offline_effect ? h('dd', { lang: 'en' }, e.offline_effect) : null,
        h('dt', {}, t('ruleCatalog.factOwner')),
        h('dd', {}, h('code', { class: 'selectable' }, e.owner_id), e.installed_version ? h('small', {}, t('ruleCatalog.factInstalled', { version: e.installed_version })) : null),
        h('dt', {}, t('ruleCatalog.factRegistry')),
        h('dd', {}, REGISTRY_TEXT[e.registry_status], e.tested_range || e.tested_at ? h('small', {}, t('ruleCatalog.factTested', { range: e.tested_range ?? '—', at: e.tested_at ?? '—' })) : null),
        e.requires_admin ? h('dt', {}, t('ruleCatalog.factAdmin')) : null,
        e.requires_admin ? h('dd', {}, t('ruleCatalog.factAdminText')) : null,
        e.locations.length ? h('dt', {}, t('ruleCatalog.factLocations')) : null,
        e.locations.length
          ? h('dd', {}, h('ul', { class: 'plain-list' }, ...e.locations.map((l) => h('li', {}, pathText(l.display, { max: 80, copy: true }), h('small', {}, LOCATION_STATE_LABEL[l.state])))), e.pattern ? h('small', {}, t('ruleCatalog.factPattern', { pattern: e.pattern })) : null)
          : null,
        h('dt', {}, t('ruleCatalog.factSize')),
        h('dd', {}, estimateText(e), h('small', {}, e.estimate ? t('ruleCatalog.factSizeFrom') : t('ruleCatalog.sizeUnknownTip'))),
      ),
      techBlock(h('p', { class: 'selectable small' }, t('ruleCatalog.tech', { rule: `${e.rule_id}@${e.version}`, evidence: e.evidence_refs.join(t('common.commaSep')) || '—' }))),
      link ? h('div', { class: 'card-actions' }, link) : null,
    );
  }

  /** Navigation only: the cleanup page shows the same scan's candidates limited to this rule. Nothing is selected. */
  private openCandidates(e: RuleCatalogEntry): void {
    this.app.store.set({ cleanupRule: e.rule_id, page: 'cleanup' });
    this.app.announce(t('ruleCatalog.openedCandidates', { rule: e.rule_id }));
  }
}

function tierWord(tier: Tier): string {
  return TIER_LABEL[tier];
}

/** The separator a display path uses (Windows `\`, macOS `/`). */
function sep(display: string): string {
  return display.includes('\\') && !display.startsWith('/') ? '\\' : '/';
}
