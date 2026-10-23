// 清理 › 构建产物 / 安装包 (PUR-01..05, INC-01..05, GUI-09). Read-only discovery over the configured
// project roots or the declared installer locations, then the shared selection pattern: summary →
// groups → expandable rows with three-tier badges, sizes and reasons; a bottom selection bar; a
// plan draft with processing modes. Nothing is pre-selected; creating a plan executes nothing.

import type { App } from '../main';

import { h, icon, keepFocus, replace } from '../lib/dom';
import { formatTime } from '../lib/format';
import { RequestGate } from '../lib/gate';
import type { Catalog, CatalogItem, CatalogPreview, InstallerLocations, IpcError, Mode, PurgeRoots } from '../lib/types';
import { LongProgress, TIER_EXPLAIN, atLeast, commonModes, ensureReady, modeOptions, noticeLine, selectionBar, sumSizes, summaryStrip, tierBadge, toIpcError } from './kit';
import { confirmInline, emptyState, errorLine, exactTitle, handledLine, pageHead, sizeText } from './parts';
import { notOnThisPlatform } from '../lib/platform';
import { issueList, learnMore, pathText } from './ui';
import { t } from '../i18n';

const SOURCE_LABEL: Record<string, string> = {
  get downloads() { return t('catalog.sourceLabel.downloads'); },
  get desktop() { return t('catalog.sourceLabel.desktop'); },
  get browser_downloads() { return t('catalog.sourceLabel.browser_downloads'); },
};

const SKIP_LABEL: Record<string, string> = {
  get hard_denied() { return t('catalog.skipLabel.hard_denied'); },
  get protected() { return t('catalog.skipLabel.protected'); },
  get vcs_tracked() { return t('catalog.skipLabel.vcs_tracked'); },
  get vcs_unknown() { return t('catalog.skipLabel.vcs_unknown'); },
  get mounted() { return t('catalog.skipLabel.mounted'); },
  get in_use() { return t('catalog.skipLabel.in_use'); },
  get probe_unknown() { return t('catalog.skipLabel.probe_unknown'); },
  get partial_evidence() { return t('catalog.skipLabel.partial_evidence'); },
  get recent() { return t('catalog.skipLabel.recent'); },
};

const REBUILD_LABEL: Record<string, string> = {
  get local_rebuild() { return t('catalog.rebuildLabel.local_rebuild'); },
  get network_redownload() { return t('catalog.rebuildLabel.network_redownload'); },
  get not_rebuildable() { return t('catalog.rebuildLabel.not_rebuildable'); },
};

interface Copy {
  title: string;
  verb: string;
  op: 'purge' | 'installer';
  /** One short sentence; the rest goes under "了解更多". */
  lede: string;
  more: string;
  empty: string;
}

/** Words of the summary (one table for the language switch). */
const CATALOG_TEXT = {
  found: (n: number) => t('catalog.found', { n }),
  plannable: (size: string) => t('catalog.plannable', { size }),
  get note() { return t('catalog.note'); },
  get summaryLabel() { return t('catalog.summaryLabel'); },
  get lowNote() { return t('catalog.lowNote'); },
  get reviewNote() { return t('catalog.reviewNote'); },
  get blockedNote() { return t('catalog.blockedNote'); },
} as const;

const COPY: Record<Catalog, Copy> = {
  purge: {
    get title() { return t('catalog.copy.purge.title'); },
    get verb() { return t('catalog.copy.purge.verb'); },
    op: 'purge',
    get lede() { return t('catalog.copy.purge.lede'); },
    get more() { return t('catalog.copy.purge.more'); },
    get empty() { return t('catalog.copy.purge.empty'); },
  },
  installer: {
    get title() { return t('catalog.copy.installer.title'); },
    get verb() { return t('catalog.copy.installer.verb'); },
    op: 'installer',
    get lede() { return t('catalog.copy.installer.lede'); },
    get more() { return t('catalog.copy.installer.more'); },
    get empty() { return t('catalog.copy.installer.empty'); },
  },
};

/** The host's `selection_preview` summary for the rows still offered (`suite::summarize`: same rules). */
function summaryOf(items: CatalogItem[]): CatalogPreview['summary'] {
  const low = items.filter((i) => i.plannable && i.selected_by_default);
  const known = sumSizes(low.map((i) => ({ bytes: i.logical_bytes, unknown: i.unknown_size_count })));
  return {
    default_selected_count: low.length,
    default_selected_known_bytes: known.known.toString(),
    default_selected_unknown_count: known.unknown,
    review_only_count: items.filter((i) => i.tier === 'review_only').length,
    blocked_count: items.filter((i) => i.tier === 'blocked').length,
    skipped_count: items.filter((i) => i.skip_reason !== null).length,
  };
}

/** Removed rows counted together (count, known size, unknown sizes). */
function handledCount(items: CatalogItem[]): { count: number; known_logical_bytes: string; unknown_size_count: number } {
  const sum = sumSizes(items.map((i) => ({ bytes: i.logical_bytes, unknown: i.unknown_size_count })));
  return { count: items.length, known_logical_bytes: sum.known.toString(), unknown_size_count: sum.unknown };
}

export class CatalogPage {
  el: HTMLElement;
  private body: HTMLElement;
  private roots: PurgeRoots | null = null;
  private locations: InstallerLocations | null = null;
  private rootsError: IpcError | null = null;
  private preview: CatalogPreview | null = null;
  private scanError: IpcError | null = null;
  private scanning = false;
  private selected = new Map<string, CatalogItem>();
  private expanded = new Set<string>();
  private collapsed = new Set<string>();
  private mode: Mode | null = null;
  private creating = false;
  private createError: IpcError | null = null;
  private notice: string | null = null;
  private removing: string | null = null;
  private progress: LongProgress;
  private gate = new RequestGate();
  private rootsGate = new RequestGate();
  private loaded = false;
  /** Rows (set + entry) a plan run after the discovery already removed or emptied: left out with a count. */
  private handled = new Set<string>();
  private opsSeen = 0;

  constructor(
    private app: App,
    private catalog: Catalog,
  ) {
    this.body = h('div', { class: 'catalog-body' });
    this.progress = new LongProgress(() => COPY[catalog].verb);
    this.el = h('section', { class: `page page-catalog page-${catalog}`, 'aria-labelledby': `${catalog}-title` }, this.body);
  }

  activate(): void {
    if (!this.loaded) {
      this.loaded = true;
      void this.loadRoots();
    }
    // Back after a plan ran: rows it removed are left out (the discovery is a snapshot; nothing is scanned again).
    const ops = this.app.store.state.opsRevision ?? 0;
    if (this.preview && ops !== this.opsSeen) void this.refreshHandled();
  }

  private async refreshHandled(): Promise<void> {
    const p = this.preview;
    if (!p) return;
    this.opsSeen = this.app.store.state.opsRevision ?? 0;
    const handled = new Set<string>();
    try {
      for (const set of p.sets) {
        const ids = p.items.filter((i) => i.candidate_set_id === set.candidate_set_id).map((i) => i.entry_id);
        if (!ids.length) continue;
        const r = await this.app.backend.removedEntries(set.scan_id, ids);
        for (const e of r.entries) handled.add(`${set.candidate_set_id}/${e.entry_id}`);
      }
    } catch {
      return; // the rows stay as discovered; plan creation still refuses removed objects
    }
    if (this.preview !== p) return;
    this.handled = handled;
    for (const [id, i] of this.selected) if (this.isHandled(i)) this.selected.delete(id);
    this.syncMode();
    this.update();
  }

  private isHandled(i: CatalogItem): boolean {
    return this.handled.has(`${i.candidate_set_id}/${i.entry_id}`);
  }

  update(): void {
    keepFocus(this.el, () => this.render());
  }

  // ---- data ------------------------------------------------------------------------------

  private async loadRoots(): Promise<void> {
    this.rootsError = null;
    try {
      if (this.catalog === 'purge') {
        const r = await this.rootsGate.latest(this.app.backend.purgeRoots());
        if (!r.stale) this.roots = r.value;
      } else {
        const r = await this.rootsGate.latest(this.app.backend.installerLocations());
        if (!r.stale) this.locations = r.value;
      }
    } catch (e) {
      this.rootsError = toIpcError(e);
    }
    this.update();
  }

  private async addRoot(): Promise<void> {
    this.rootsError = null;
    try {
      const r = await this.app.backend.purgeRootsAdd();
      if (r) {
        this.rootsGate.invalidate();
        this.roots = r;
        this.notice = t('catalog.scanRootAddedProjects');
        this.app.announce(t('catalog.scanRootAdded'));
      }
    } catch (e) {
      this.rootsError = toIpcError(e);
    }
    this.update();
  }

  private async removeRoot(rootId: string): Promise<void> {
    try {
      this.rootsGate.invalidate();
      this.roots = await this.app.backend.purgeRootsRemove(rootId);
      this.removing = null;
      this.notice = t('catalog.scanRootRemovedFiles');
      this.app.announce(t('catalog.scanRootRemoved'));
    } catch (e) {
      this.rootsError = toIpcError(e);
    }
    this.update();
  }

  private async scan(): Promise<void> {
    if (this.scanning) return;
    const copy = COPY[this.catalog];
    if (!(await ensureReady(this.app, copy.op, copy.verb))) return;
    this.scanning = true;
    this.scanError = null;
    this.notice = null;
    this.progress.start(this.catalog === 'purge' ? t('catalog.scanningEachProjectFolder') : t('catalog.scanningListedLocationsChecking'));
    this.update();
    this.app.announce(t('catalog.startedVerbReadOnly', { verb: copy.verb }));
    try {
      const r = await this.gate.latest(this.catalog === 'purge' ? this.app.backend.purgeScan() : this.app.backend.installerScan());
      if (r.stale) return;
      this.preview = r.value;
      this.handled = new Set();
      this.opsSeen = this.app.store.state.opsRevision ?? 0;
      // A new discovery never inherits an old selection (different candidate sets).
      this.selected.clear();
      this.mode = null;
      this.app.announce(t('catalog.scanCompleteLengthNothing', { length: r.value.items.length }));
    } catch (e) {
      this.scanError = toIpcError(e);
    } finally {
      this.scanning = false;
      this.progress.stop();
      this.update();
    }
  }

  private toggle(i: CatalogItem): void {
    if (this.selected.has(i.candidate_id)) this.selected.delete(i.candidate_id);
    else if (i.plannable) this.selected.set(i.candidate_id, i);
    this.syncMode();
    this.app.announce(this.selected.has(i.candidate_id) ? t('common.selectedName', { name: i.name }) : t('common.deselectedName', { name: i.name }));
    this.update();
  }

  private selectGroup(items: CatalogItem[], label: string): void {
    let n = 0;
    for (const i of items) if (i.plannable && !this.selected.has(i.candidate_id)) (this.selected.set(i.candidate_id, i), (n += 1));
    this.syncMode();
    this.app.announce(n ? t('catalog.selectedNLabel', { label, n }) : t('catalog.nothingAdd'));
    this.update();
  }

  private selectLowRisk(): void {
    const items = this.preview?.items.filter((i) => i.plannable && i.tier === 'default_selected' && !this.isHandled(i)) ?? [];
    this.selectGroup(items, t('catalog.lowRiskGroup'));
  }

  private syncMode(): void {
    const modes = commonModes([...this.selected.values()]);
    if (this.mode && !modes.includes(this.mode)) this.mode = null;
    this.createError = null;
  }

  private async createPlans(): Promise<void> {
    if (!this.mode || this.creating || !this.selected.size) return;
    this.creating = true;
    this.createError = null;
    this.update();
    const bySet = new Map<string, string[]>();
    for (const i of this.selected.values()) bySet.set(i.candidate_set_id, [...(bySet.get(i.candidate_set_id) ?? []), i.candidate_id]);
    const created: string[] = [];
    try {
      let stale = 0;
      for (const [set, ids] of bySet) {
        const r = await this.app.backend.createCatalogPlan(this.catalog, set, ids, this.mode);
        created.push(r.plan_id);
        stale += r.stale_items?.length ?? 0;
      }
      // Rows that changed since discovery are left out of the plan, never silently replaced.
      if (stale) this.app.announce(t('catalog.stale', { stale }));
      if (created.length === 1) {
        this.app.announce(t('common.planCreatedNotRun'));
        this.app.openPlan(created[0], this.catalog);
      } else {
        this.app.announce(t('catalog.createdLengthOnePer', { length: created.length }));
        this.app.openActivity('plans');
      }
      this.selected.clear();
      this.mode = null;
    } catch (e) {
      this.createError = toIpcError(e);
      if (created.length) this.notice = t('catalog.createdLengthRestCouldnt', { length: created.length });
    } finally {
      this.creating = false;
      this.update();
    }
  }

  // ---- rendering ---------------------------------------------------------------------------

  private render(): void {
    const copy = COPY[this.catalog];
    const scanBtn = h('button', { type: 'button', class: this.preview ? '' : 'primary', id: `${this.catalog}-scan`, disabled: this.scanning }, this.scanning ? t('catalog.scanning') : this.preview ? t('common.scanAgain') : copy.verb);
    scanBtn.addEventListener('click', () => void this.scan());
    const p = this.preview;
    const unsupported = (this.catalog === 'purge' ? this.roots?.supported : this.locations?.supported) === false || p?.supported === false;
    replace(
      this.body,
      pageHead({
        id: `${this.catalog}-title`,
        title: copy.title,
        meta: p ? [t('catalog.scannedObserved', { observed_at: formatTime(p.observed_at) }), t('common.objectCount', { length: p.items.length }), t('catalog.readOnlyDiscoveryNothing')] : [t('catalog.readOnlyDiscoveryNothing')],
        actions: unsupported ? [] : [scanBtn],
      }),
      this.notice ? noticeLine(this.notice, 'info', () => ((this.notice = null), this.update())) : null,
      unsupported
        ? emptyState(t('common.notAvailablePlatform'), notOnThisPlatform(t('catalog.discoveryImplementedVerifiedOnly')), null)
        : h(
            'div',
            { class: 'work-layout' },
            h('div', { class: 'work-main' }, this.progress.el, errorLine(this.scanError), p ? this.results(p) : this.intro(), this.rootsPanel()),
            this.draft(),
          ),
      this.bar(),
    );
  }

  private intro(): HTMLElement {
    const copy = COPY[this.catalog];
    return h(
      'section',
      { class: 'panel catalog-intro', 'aria-labelledby': `${this.catalog}-intro` },
      h('h2', { id: `${this.catalog}-intro` }, this.scanning ? t('common.scanning') : t('catalog.notScannedYet')),
      h('p', { class: 'lede' }, copy.lede),
      learnMore(h('p', {}, copy.more), h('p', {}, t('catalog.scanReadsOnlyFile'))),
    );
  }

  private results(p: CatalogPreview): HTMLElement {
    // Rows a later plan run already removed are not offered again: left out, counted in one quiet line.
    const items = this.handled.size ? p.items.filter((i) => !this.isHandled(i)) : p.items;
    const gone = this.handled.size ? p.items.filter((i) => this.isHandled(i)) : [];
    const goneLine = gone.length ? handledLine(this.app, handledCount(gone), null) : null;
    if (!items.length) return h('div', { class: 'panel' }, goneLine, emptyState(t('catalog.nothingFound'), COPY[this.catalog].empty, null), this.issues(p));
    const groups = new Map<string, CatalogItem[]>();
    for (const i of items) groups.set(i.group_label, [...(groups.get(i.group_label) ?? []), i]);
    const s = gone.length ? summaryOf(items) : p.summary;
    const lowCount = items.filter((i) => i.plannable && i.tier === 'default_selected').length;
    const known = sumSizes(items.filter((i) => i.plannable).map((i) => ({ bytes: i.logical_bytes, unknown: i.unknown_size_count })));
    const summary = h(
      'section',
      { class: 'cand-summary catalog-summary', 'aria-labelledby': `${this.catalog}-sum` },
      h('h2', { id: `${this.catalog}-sum`, class: 'cs-sentence' }, CATALOG_TEXT.found(items.length), h('span', { class: 'cs-size num' }, CATALOG_TEXT.plannable(atLeast(known)))),
      h('p', { class: 'cs-note' }, CATALOG_TEXT.note),
      summaryStrip(CATALOG_TEXT.summaryLabel, [
        { mark: tierBadge('default_selected'), count: s.default_selected_count, size: s.default_selected_count ? atLeast({ known: BigInt(s.default_selected_known_bytes), unknown: s.default_selected_unknown_count }) : null, note: CATALOG_TEXT.lowNote },
        { mark: tierBadge('review_only'), count: s.review_only_count, note: CATALOG_TEXT.reviewNote },
        { mark: tierBadge('blocked'), count: s.blocked_count, note: CATALOG_TEXT.blockedNote },
      ]),
      lowCount && this.catalog === 'purge' ? h('button', { type: 'button', class: 'quiet', id: 'select-low', onclick: () => this.selectLowRisk() }, t('catalog.selectAllLowRisk', { lowCount })) : null,
    );
    return h('div', { class: 'catalog-results' }, summary, goneLine, this.issues(p), ...[...groups.entries()].map(([label, rows], gi) => this.group(label, rows, gi)));
  }

  private issues(p: CatalogPreview): HTMLElement | null {
    if (!p.issues.length) return null;
    return h(
      'details',
      { class: 'issues-details' },
      h('summary', {}, t('catalog.scanNotesLength', { length: p.issues.length })),
      issueList(p.issues, { label: t('common.recordedIssues') }),
    );
  }

  private group(label: string, items: CatalogItem[], gi: number): HTMLElement {
    const id = `${this.catalog}-g${gi}`;
    const open = !this.collapsed.has(label);
    const sum = sumSizes(items.map((i) => ({ bytes: i.logical_bytes, unknown: i.unknown_size_count })));
    const title = this.catalog === 'installer' ? (SOURCE_LABEL[label] ?? label) : label;
    const toggle = h(
      'button',
      { type: 'button', class: 'group-toggle', id: `${id}-toggle`, 'aria-expanded': open ? 'true' : 'false', 'aria-controls': `${id}-body` },
      h('span', { class: 'chev-icon', 'aria-hidden': 'true' }, icon('chevron')),
      h('span', { class: this.catalog === 'purge' ? 'path group-title' : 'group-title' }, title),
      h('span', { class: 'group-count num' }, t('common.itemCount', { length: items.length })),
      h('span', { class: 'group-size num' }, atLeast(sum)),
    );
    toggle.addEventListener('click', () => {
      if (open) this.collapsed.add(label);
      else this.collapsed.delete(label);
      this.update();
    });
    const plannable = items.filter((i) => i.plannable);
    const pick =
      this.catalog === 'installer' && plannable.length > 1
        ? h('button', { type: 'button', class: 'quiet', onclick: () => this.selectGroup(plannable, title) }, t('catalog.selectAllSourceLength', { length: plannable.length }))
        : null;
    return h(
      'section',
      { class: 'panel cgroup', 'aria-labelledby': `${id}-toggle` },
      h('div', { class: 'cgroup-head' }, h('h2', {}, toggle), pick),
      h('div', { class: 'group-body', id: `${id}-body`, hidden: !open }, this.table(items, title)),
    );
  }

  private table(items: CatalogItem[], caption: string): HTMLElement {
    const rows: HTMLElement[] = [];
    for (const i of items) {
      const id = `${this.catalog}-${i.candidate_id}`;
      const check = h('input', { type: 'checkbox', id: `${id}-check`, disabled: !i.plannable, 'aria-label': t('common.selectName', { name: i.name }), 'aria-describedby': `${id}-why` }) as HTMLInputElement;
      check.checked = this.selected.has(i.candidate_id);
      check.addEventListener('change', () => this.toggle(i));
      const open = this.expanded.has(i.candidate_id);
      const more = h('button', { type: 'button', class: 'link-btn', id: `${id}-more`, 'aria-expanded': open ? 'true' : 'false', 'aria-controls': `${id}-detail` }, open ? t('catalog.hideBasis') : t('catalog.basisImpact'));
      more.addEventListener('click', () => {
        if (open) this.expanded.delete(i.candidate_id);
        else this.expanded.add(i.candidate_id);
        this.update();
      });
      const reason = i.skip_reason ? (SKIP_LABEL[i.skip_reason] ?? i.skip_reason) : i.rebuild_class ? (REBUILD_LABEL[i.rebuild_class] ?? i.rebuild_class) : '';
      rows.push(
        h(
          'tr',
          { class: `crow${check.checked ? ' selected' : ''}${i.plannable ? '' : ' locked'}` },
          h('td', { class: 'col-check' }, check),
          h('td', { class: 'col-object' }, h('label', { for: `${id}-check`, class: 'obj-name' }, i.name), pathText(i.path_display, { max: 64, className: 'obj-path' }), h('span', { class: 'obj-reason', id: `${id}-why` }, reason)),
          h('td', { class: 'col-size num', title: exactTitle(i.logical_bytes) }, sizeText(i.logical_bytes, i.unknown_size_count)),
          h('td', { class: 'col-status' }, tierBadge(i.tier), more),
        ),
      );
      if (open) rows.push(h('tr', { class: 'cdetail', id: `${id}-detail` }, h('td', { colspan: '4' }, this.detail(i))));
    }
    return h(
      'div',
      { class: 'table-wrap' },
      h(
        'table',
        { class: 'data-table ctable' },
        h('caption', { class: 'sr-only' }, t('catalog.captionObjectsSizesRisk', { caption })),
        h('thead', {}, h('tr', {}, h('th', { scope: 'col', class: 'col-check' }, h('span', { class: 'sr-only' }, t('common.select'))), h('th', { scope: 'col' }, t('common.objectBasis')), h('th', { scope: 'col', class: 'col-size' }, t('common.logicalSize')), h('th', { scope: 'col' }, t('catalog.tier')))),
        h('tbody', {}, ...rows),
      ),
    );
  }

  private detail(i: CatalogItem): HTMLElement {
    return h(
      'div',
      { class: 'cdetail-body' },
      h(
        'dl',
        { class: 'facts-list' },
        h('dt', {}, t('catalog.tier')),
        h('dd', {}, TIER_EXPLAIN[i.tier], h('small', {}, i.tier_reason)),
        h('dt', {}, t('common.impact')),
        h('dd', {}, i.impact || t('catalog.noFurtherNotes')),
        i.rebuild_class ? h('dt', {}, t('catalog.gettingBack')) : null,
        i.rebuild_class ? h('dd', {}, REBUILD_LABEL[i.rebuild_class] ?? i.rebuild_class) : null,
        i.unknown_size_count ? h('dt', {}, t('common.unknownSize')) : null,
        i.unknown_size_count ? h('dd', {}, t('catalog.unknownSizeCount', { unknown_size_count: i.unknown_size_count })) : null,
        i.notes.length ? h('dt', {}, t('catalog.basis')) : null,
        i.notes.length ? h('dd', {}, h('ul', { class: 'plain-list' }, ...i.notes.map((n) => h('li', {}, n)))) : null,
      ),
    );
  }

  private rootsPanel(): HTMLElement {
    if (this.catalog === 'installer') {
      const l = this.locations;
      return h(
        'section',
        { class: 'panel roots-panel', 'aria-labelledby': 'inst-loc-title' },
        h('h2', { id: 'inst-loc-title' }, t('catalog.searchLocations')),
        h('p', { class: 'muted small' }, t('catalog.onlyLocationsSearchedUp', { p: l?.max_depth ?? '—' })),
        l
          ? h(
              'ul',
              { class: 'root-rows' },
              ...l.locations.map((x) => h('li', {}, h('span', { class: 'root-main' }, h('b', {}, SOURCE_LABEL[x.source_label] ?? x.source_label), h('span', { class: 'path muted' }, x.display)), x.exists ? null : h('span', { class: 'why-not' }, t('catalog.missing')))),
            )
          : h('p', { class: 'muted small', role: 'status' }, this.rootsError ? '' : t('catalog.loading')),
        errorLine(this.rootsError),
      );
    }
    const r = this.roots;
    const add = h('button', { type: 'button', id: 'purge-root-add' }, icon('reveal'), t('catalog.addFolder'));
    add.addEventListener('click', () => void this.addRoot());
    return h(
      'section',
      { class: 'panel roots-panel', 'aria-labelledby': 'purge-roots-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'purge-roots-title' }, t('catalog.projectScanRoots')), add),
      h('p', { class: 'muted small' }, t('catalog.projectsSearchedOnlyFolders', { p: r?.max_depth ?? '—' })),
      learnMore(h('p', {}, t('catalog.projectsChangedWithinP', { p: r?.recent_days ?? '—' }))),
      r
        ? h(
            'ul',
            { class: 'root-rows' },
            ...r.roots.map((x) => {
              const rm = h('button', { type: 'button', class: 'quiet', id: `root-rm-${x.root_id}`, 'aria-label': t('catalog.removeDisplay', { display: x.display }) }, t('common.remove'));
              rm.addEventListener('click', () => {
                this.removing = x.root_id;
                this.update();
              });
              return h(
                'li',
                {},
                h('span', { class: 'root-main' }, h('span', { class: 'path' }, x.display), h('span', { class: 'muted small' }, x.source === 'default' ? t('common.default') : t('catalog.added'), x.exists ? '' : t('catalog.missingVolumeOfflineSkipped'))),
                this.removing === x.root_id
                  ? confirmInline({
                      id: `root-rm-${x.root_id}-c`,
                      prompt: t('catalog.removesScanSettingsOnly'),
                      expected: t('catalog.remove'),
                      action: t('catalog.confirmRemoval'),
                      onConfirm: () => this.removeRoot(x.root_id),
                      onCancel: () => ((this.removing = null), this.update()),
                    })
                  : rm,
              );
            }),
          )
        : h('p', { class: 'muted small', role: 'status' }, this.rootsError ? '' : t('catalog.loading')),
      r && !r.roots.length ? h('p', { class: 'small' }, t('catalog.noScanRootsYet')) : null,
      errorLine(this.rootsError),
    );
  }

  private draft(): HTMLElement {
    const items = [...this.selected.values()];
    const n = items.length;
    const sum = sumSizes(items.map((i) => ({ bytes: i.logical_bytes, unknown: i.unknown_size_count })));
    const sets = new Set(items.map((i) => i.candidate_set_id)).size;
    const create = h(
      'button',
      { type: 'button', class: 'primary wide', id: `${this.catalog}-create`, disabled: n === 0 || !this.mode || this.creating },
      this.creating ? t('common.freezingPlan') : n === 0 ? t('common.selectObjectsFirst') : !this.mode ? t('catalog.chooseAction') : sets > 1 ? t('catalog.createSetsPlansN', { sets, n }) : t('catalog.createPlanN', { n }),
    );
    create.addEventListener('click', () => void this.createPlans());
    return h(
      'aside',
      { class: 'panel work-aside draft', 'aria-labelledby': `${this.catalog}-draft-title` },
      h('h2', { id: `${this.catalog}-draft-title` }, t('catalog.selection')),
      n === 0
        ? h('p', { class: 'muted small' }, t('catalog.nothingPreselectedCheckObjects'))
        : h(
            'div',
            { class: 'draft-sum' },
            h('p', { class: 'draft-figure num' }, atLeast(sum)),
            h('p', { class: 'muted small' }, t('catalog.knownLogicalSizeN', { n, p: sum.unknown ? t('catalog.unknown', { unknown: sum.unknown }) : '' })),
            sets > 1 ? h('p', { class: 'small' }, t('catalog.setsLocationsOnePlan', { sets })) : null,
          ),
      modeOptions({ name: `${this.catalog}-mode`, allowed: commonModes(items), current: this.mode, selected: n, onChange: (m) => ((this.mode = m), (this.createError = null), this.update()) }),
      errorLine(this.createError),
      create,
      h('p', { class: 'muted small aside-note' }, t('catalog.creatingPlanOnlyFreezes')),
    );
  }

  private bar(): HTMLElement {
    const items = [...this.selected.values()];
    const sum = sumSizes(items.map((i) => ({ bytes: i.logical_bytes, unknown: i.unknown_size_count })));
    return selectionBar({
      count: items.length,
      sum,
      sources: new Set(items.map((i) => i.candidate_set_id)).size,
      onClear: () => {
        this.selected.clear();
        this.syncMode();
        this.app.announce(t('common.selectionCleared'));
        this.update();
      },
      action: {
        id: `${this.catalog}-bar-go`,
        text: this.mode ? t('common.createPlanReview') : t('common.chooseAction'),
        disabled: this.creating,
        onClick: () => {
          if (this.mode) void this.createPlans();
          else document.querySelector<HTMLInputElement>(`#${this.catalog}-mode-modes input:not(:disabled)`)?.focus();
        },
      },
    });
  }
}
