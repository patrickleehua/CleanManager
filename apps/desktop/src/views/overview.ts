// P02 — overview of the scan this window is bound to. One primary message per state (the status banner),
// then only what that state needs: running → progress and cancel; completed → the space summary;
// partial → what was missed, then the known part; failed → the reason and the next step; cancelled /
// interrupted → how much was kept. Machine facts (codes, backend, raw messages) sit in collapsed details.
// No health score, no "0 B" before a scan, no percentage of an unknown total.

import type { App } from '../main';
import type { InsightsList, IpcError, RecentScan, ScanStatus } from '../lib/types';
import { insightCards } from './tools';
import { toIpcError } from '../lib/api';
import { formatExactBytes, groupDigits } from '../lib/bytes';
import { refusalText } from '../lib/codes';
import { sizeLabel } from '../lib/display';
import { h, keepFocus, replace } from '../lib/dom';
import { COMPLETENESS_LABEL, formatElapsed, formatTime, isTerminal, STATE_LABEL, stateTone } from '../lib/format';
import { overviewPlan, type OverviewPlan } from '../lib/overviewModel';
import { currentIndexProblem } from '../lib/indexModel';
import { indexProblemBanner } from './indexRepair';
import { emptyBlock, issueList, learnMore, pathText, sizeEl, statusBanner, techBlock } from './ui';
import { t } from '../i18n';

type Child = Node | string | null | undefined | false;

export class OverviewPage {
  el: HTMLElement;
  private body: HTMLElement;
  private actionError: IpcError | null = null;
  private timer: number | null = null;
  private insights: InsightsList | null = null;
  private insightsRequested = false;
  private announced: string | null = null;
  private busy = false;

  constructor(private app: App) {
    this.body = h('div', { class: 'overview-body' });
    this.el = h('section', { class: 'page page-overview', 'aria-labelledby': 'overview-title' }, this.body);
  }

  activate(): void {
    void this.app.refreshStatus();
    if (!this.insightsRequested) {
      this.insightsRequested = true;
      this.app.backend
        .insightsList()
        .then((r) => {
          this.insights = r;
          this.update();
        })
        .catch(() => (this.insights = null));
    }
  }

  update(): void {
    keepFocus(this.el, () => this.render());
    // Elapsed time ticks locally while running; the store is not re-read for it.
    const s = this.app.store.state.status;
    const running = s && !isTerminal(s.state);
    if (running && this.timer === null) this.timer = window.setInterval(() => this.tickElapsed(), 1000);
    if (!running && this.timer !== null) {
      window.clearInterval(this.timer);
      this.timer = null;
    }
  }

  private tickElapsed(): void {
    const el = this.el.querySelector('[data-elapsed]');
    const s = this.app.store.state.status;
    if (el && s) el.textContent = t('overview.startedElapsed', { started_at: formatElapsed(s.started_at) });
  }

  private render(): void {
    const st = this.app.store.state;
    // NEW-04: a damaged index is not an empty history. Say what is wrong and offer the rebuild.
    const broken = currentIndexProblem({ caps: st.caps, statusError: st.statusError, repairedAt: st.indexRepairedAt });
    if (broken && (!st.scanId || !st.status)) {
      replace(this.body, h('div', { class: 'ov-single' }, indexProblemBanner(this.app, broken, { titleId: 'overview-title', srPrefix: t('overview.overviewPrefix') })));
      return;
    }
    if (!st.scanId) {
      replace(
        this.body,
        h('div', { class: 'ov-single' }, emptyBlock({ id: 'overview-title', level: 1, icon: 'tool-analyze', title: t('overview.noScanResultsYet'), body: t('overview.pickFolderStartRead'), actions: [this.chooseButton('primary', t('common.chooseWhatScan'))] })),
      );
      return;
    }
    if (st.statusError && !st.status) {
      replace(
        this.body,
        h(
          'div',
          { class: 'ov-single' },
          statusBanner({ kind: 'failed', titleId: 'overview-title', srPrefix: t('overview.overviewPrefix'), title: t('overview.couldntReadScanStatus'), sentence: refusalText(st.statusError), actions: [this.chooseButton('primary', t('common.chooseWhatScan'))] }),
          techBlock(h('p', { class: 'selectable small' }, t('overview.reasonMessage', { reason: st.statusError.reason, message: st.statusError.message }))),
        ),
      );
      return;
    }
    if (!st.status) {
      replace(this.body, h('div', { class: 'ov-single' }, h('h1', { id: 'overview-title', class: 'sb-title' }, t('overview.title')), h('p', { class: 'muted', role: 'status' }, t('overview.readingScanStatus'))));
      return;
    }
    const s = st.status;
    const plan = overviewPlan(s);
    this.announce(s, plan);
    const main = h('div', { class: 'ov-main' }, ...this.mainColumn(s, plan));
    replace(this.body, plan.aside ? h('div', { class: 'ov-layout' }, main, this.aside(plan)) : h('div', { class: 'ov-single' }, main));
  }

  /** Status changes are announced once (navigation to an unchanged scan says nothing). */
  private announce(s: ScanStatus, plan: OverviewPlan): void {
    const key = `${s.scan_id}:${plan.layout}:${plan.banner.title}`;
    if (this.announced !== null && this.announced !== key && this.announced.startsWith(`${s.scan_id}:`)) this.app.announce(t('overview.titleSentence', { title: plan.banner.title, sentence: plan.banner.sentence }));
    this.announced = key;
  }

  // ---------------------------------------------------------------------------------------------
  // Main column

  private mainColumn(s: ScanStatus, plan: OverviewPlan): Child[] {
    const meta: Child[] = [
      s.root_display ? pathText(s.root_display, { max: 64, className: 'sb-path' }) : t('overview.scopePathUnavailable'),
      plan.layout === 'running' ? h('span', { 'data-elapsed': '1' }, t('overview.startedElapsed', { started_at: formatElapsed(s.started_at) })) : s.finished_at ? t('overview.finishedAt', { finished_at: formatTime(s.finished_at) }) : null,
      plan.layout !== 'running' && plan.layout !== 'failed' && s.started_at && s.finished_at ? t('overview.took', { started_at: formatElapsed(s.started_at, s.finished_at) }) : null,
    ];
    const extra: Child[] = [];
    if (plan.layout === 'running') extra.push(h('div', { class: 'indeterminate', role: 'progressbar', 'aria-label': t('overview.scanProgressTotalUnknown'), 'aria-valuetext': t('overview.entriesSeenFound', { entries_seen: s.entries_seen }) }));
    // Completed and stopped scans carry their figure in the banner: the space summary comes first.
    if (plan.space && plan.layout !== 'partial') extra.push(this.figure(s, plan));
    if (this.actionError) extra.push(h('p', { class: 'form-error', role: 'alert' }, refusalText(this.actionError)));

    const banner = statusBanner({ kind: plan.banner.kind, titleId: 'overview-title', srPrefix: t('overview.overviewPrefix'), title: plan.banner.title, sentence: plan.banner.sentence, meta, extra, actions: this.actions(s, plan) });
    const corner = plan.layout !== 'failed' && plan.layout !== 'running';
    const head = h('div', { class: `ov-head${corner ? ' has-corner' : ''}` }, banner, corner ? this.chooseButton('quiet', t('overview.newScan'), 'ov-new') : null);

    const out: Child[] = [head];
    if (s.elevated_read.length && plan.layout !== 'failed') out.push(h('p', { class: 'notice warn' }, t('overview.resultReadJoinEnabled', { join: s.elevated_read.join(t('common.listSep')) })));
    if (plan.layout === 'running') out.push(this.runningMore(s));
    if (plan.gaps && (plan.layout === 'partial' || plan.gaps.unknownSizes > 0)) out.push(this.gapsPanel(s, plan));
    if (plan.layout === 'partial' && plan.space) out.push(this.knownPanel(s, plan));
    if (plan.layout === 'completed' && s.issues.length) out.push(this.notesPanel(s));
    if (isTerminal(s.state)) out.push(this.technical(s, plan));
    return out;
  }

  private figure(s: ScanStatus, plan: OverviewPlan): HTMLElement {
    const sp = plan.space!;
    return h(
      'div',
      { class: 'sb-figure' },
      h('p', { class: 'figure-big num', title: formatExactBytes(sp.bytes) }, sp.text),
      h('p', { class: 'figure-cap' }, sp.label, h('span', { class: 'figure-sep', 'aria-hidden': 'true' }), t('overview.entriesSeen', { entries_seen: s.entries_seen })),
    );
  }

  private actions(s: ScanStatus, plan: OverviewPlan): Child[] {
    const a: Child[] = [];
    if (plan.cancel) {
      const cancel = h('button', { type: 'button', id: 'ov-cancel', class: 'danger-quiet', disabled: s.cancel_requested || s.state === 'stopping' }, s.cancel_requested || s.state === 'stopping' ? t('overview.stopRequested') : t('overview.cancelScan'));
      cancel.addEventListener('click', async () => {
        try {
          this.actionError = null;
          await this.app.cancelScan(s.scan_id);
        } catch (e) {
          this.actionError = toIpcError(e);
          this.update();
        }
      });
      a.push(cancel);
    }
    if (plan.rescan === 'primary') a.push(this.rescanButton(s, 'primary'));
    if (plan.chooseRoot === 'primary') a.push(this.chooseButton('primary', t('overview.chooseDifferentScope')));
    if (plan.browse) {
      const disk = h('button', { type: 'button', id: 'ov-disk', class: plan.rescan === 'primary' ? '' : 'primary' }, plan.browse === 'kept' ? t('overview.browseKeptResults') : plan.pendingPermission ? t('overview.browseWhatRead') : t('overview.browseDisk'));
      disk.addEventListener('click', () => this.app.navigate('disk'));
      a.push(disk);
    }
    if (plan.candidates) {
      const clean = h('button', { type: 'button', id: 'ov-cleanup' }, t('overview.viewCleanupCandidates'));
      clean.addEventListener('click', () => this.app.navigate('cleanup'));
      a.push(clean);
    }
    if (plan.rescan === 'secondary') a.push(this.rescanButton(s, ''));
    if (plan.chooseRoot === 'secondary') a.push(this.chooseButton('', t('overview.chooseAnotherScope')));
    if (plan.layout === 'failed') {
      const prev = this.lastGood(s);
      if (prev) a.push(h('button', { type: 'button', class: 'link', id: 'ov-prev', onclick: () => void this.app.openScan(prev.scan_id, 'overview') }, t('overview.viewScanFormattime', { formatTime: formatTime(prev.finished_at ?? prev.started_at) })));
    }
    return a;
  }

  private chooseButton(kind: string, label: string, id = 'ov-choose'): HTMLElement {
    const b = h('button', { type: 'button', id, class: kind }, label);
    b.addEventListener('click', () => this.app.navigate('scope'));
    return b;
  }

  private rescanButton(s: ScanStatus, kind: string): HTMLElement {
    const b = h('button', { type: 'button', id: 'ov-rescan', class: kind, disabled: this.busy }, t('common.scanAgain'));
    b.addEventListener('click', () => void this.rescan(s));
    return b;
  }

  /** Scan the same root again when it is still offered; otherwise choose it on the scope page. */
  private async rescan(s: ScanStatus): Promise<void> {
    this.busy = true;
    this.actionError = null;
    this.update();
    try {
      const offers = await this.app.backend.listScanRoots();
      const o = offers.find((x) => x.path_display === s.root_display && !x.problem);
      if (o) await this.app.startScan(o.token);
      else this.app.navigate('scope');
    } catch (e) {
      this.actionError = toIpcError(e);
    } finally {
      this.busy = false;
      this.update();
    }
  }

  private lastGood(s: ScanStatus): RecentScan | undefined {
    return this.app.store.state.recent.find((r) => r.scan_id !== s.scan_id && r.published && (r.state === 'completed' || r.state === 'partial'));
  }

  private runningMore(s: ScanStatus): HTMLElement {
    return learnMore(
      h('p', {}, t('overview.totalIsntKnownUntil')),
      s.hosted_here
        ? h('p', {}, t('overview.windowRunsScanClosing'))
        : h('p', {}, t('overview.anotherProcessRunsScan', { p: s.host_kind ?? t('common.unknownLower'), p2: s.host_pid ?? '?' })),
    );
  }

  /** What was not read (partial) or whose size is unknown. */
  private gapsPanel(s: ScanStatus, plan: OverviewPlan): HTMLElement {
    const g = plan.gaps!;
    const title = plan.pendingPermission ? t('overview.locationsWaitingPermission') : g.unread > 0 ? t('overview.locationsNotRead') : t('overview.entriesUnknownSize');
    return h(
      'section',
      { class: 'panel gaps-panel', 'data-tone': plan.pendingPermission ? 'pending' : 'warn', 'aria-labelledby': 'ov-gaps-title' },
      h('h2', { id: 'ov-gaps-title' }, title),
      s.issues.length ? issueList(s.issues, { label: t('overview.issuesRecordedScan') }) : null,
      g.unread > 0 && !s.issues.length ? h('p', {}, t('overview.unreadCouldntReadNo', { unread: g.unread })) : null,
      g.unknownSizes > 0 ? h('p', { class: 'gaps-unknown' }, t('overview.unknownsizesCountedKnownSpace', { unknownSizes: g.unknownSizes })) : null,
      s.issues_total > s.issues.length ? h('p', { class: 'muted small' }, t('overview.showingFirstLengthIssues', { length: s.issues.length, issues_total: s.issues_total })) : null,
      learnMore(h('p', {}, t('overview.locationsSizesRecordedUnknown'))),
    );
  }

  /** Partial: the known part after the gaps. */
  private knownPanel(s: ScanStatus, plan: OverviewPlan): HTMLElement {
    return h('section', { class: 'panel known-panel', 'aria-labelledby': 'ov-known-title' }, h('h2', { id: 'ov-known-title' }, t('overview.whatRead')), this.figure(s, plan));
  }

  /** Completed scans can still note things (e.g. CleanManager's own data was not scanned). */
  private notesPanel(s: ScanStatus): HTMLElement {
    return h('section', { class: 'quiet-section', 'aria-labelledby': 'ov-notes-title' }, h('h2', { id: 'ov-notes-title' }, t('overview.scanNotes')), issueList(s.issues, { label: t('overview.scanNotes') }));
  }

  private technical(s: ScanStatus, plan: OverviewPlan): HTMLElement {
    const sum = s.summary;
    const rows: [string, Child][] = [];
    if (plan.space && sum) {
      rows.push([t('overview.exactValue'), h('span', { class: 'num' }, formatExactBytes(plan.space.bytes))]);
      rows.push([t('common.allocatedSize'), sum.allocated_exact_bytes !== null ? sizeEl(sum.allocated_exact_bytes) : t('overview.unknownKnownPartAllocated', { allocated_known_bytes: sizeLabel(sum.allocated_known_bytes), allocated_unknown_count: sum.allocated_unknown_count })]);
      rows.push([t('overview.basis'), t('overview.logicalSizeTotalContent')]);
    }
    rows.push([t('overview.entriesFound'), h('span', { class: 'num' }, groupDigits(s.entries_seen))]);
    if (plan.layout !== 'failed' && s.completeness) rows.push([t('overview.completeness'), COMPLETENESS_LABEL[s.completeness] ?? s.completeness]);
    if (plan.reason) rows.push([t('common.code'), h('code', { class: 'selectable' }, plan.reason.rawCode)], [t('common.systemMessage'), h('span', { class: 'selectable' }, plan.reason.raw || t('common.none'))]);
    rows.push([t('overview.started'), formatTime(s.started_at)]);
    if (s.finished_at) rows.push([t('overview.finished'), formatTime(s.finished_at)]);
    if (s.backend) rows.push([t('overview.readMethod'), h('span', { class: 'selectable' }, `${s.backend}${s.backend_reason ? t('overview.backendReason', { backend_reason: s.backend_reason }) : ''}`)]);
    rows.push([t('overview.scanId'), h('code', { class: 'selectable' }, s.scan_id)]);
    return techBlock(h('dl', { class: 'facts-list small' }, ...rows.flatMap(([k, v]) => [h('dt', {}, k), h('dd', {}, v)])));
  }

  // ---------------------------------------------------------------------------------------------
  // Aside: other scans, machine-wide objects, capabilities

  private aside(plan: OverviewPlan): HTMLElement {
    return h('aside', { class: 'ov-aside', 'aria-label': t('overview.moreInformation') }, this.recentList(), plan.systemCards ? this.systemObjects() : null, this.hostCard());
  }

  private recentList(): HTMLElement | null {
    const { recent, scanId } = this.app.store.state;
    const others = recent.filter((r) => r.scan_id !== scanId);
    if (!others.length) return null;
    return h(
      'section',
      { class: 'aside-block', 'aria-labelledby': 'recent-title' },
      h('h2', { id: 'recent-title' }, t('overview.otherScans')),
      h(
        'ul',
        { class: 'recent-list' },
        ...others.slice(0, 6).map((r) => {
          const b = h(
            'button',
            { type: 'button', id: `recent-${r.scan_id}`, class: 'recent-item' },
            pathText(r.root_display, { max: 34, className: 'ri-path' }),
            h('span', { class: 'ri-meta' }, h('span', { class: 'state-word', 'data-tone': r.state ? stateTone(r.state) : 'muted' }, h('span', { class: 'sw-mark', 'aria-hidden': 'true' }), r.state ? STATE_LABEL[r.state] : t('common.recordUnavailable')), formatTime(r.started_at)),
          );
          b.addEventListener('click', () => void this.app.openScan(r.scan_id, 'overview'));
          return h('li', {}, b);
        }),
      ),
      others.length > 6 ? h('button', { type: 'button', class: 'link small', onclick: () => this.app.openActivity('scans') }, t('overview.allScans', { length: others.length })) : null,
    );
  }

  /** DIN-03: machine-wide objects — clearly apart from the scanned folder. */
  private systemObjects(): HTMLElement | null {
    const d = this.insights;
    if (!d || !d.supported || !d.items.length) return null;
    const open = h('button', { type: 'button', class: 'link small', id: 'ov-insights' }, t('overview.viewSpaceInsights'));
    open.addEventListener('click', () => this.app.navigate('insights'));
    return h(
      'section',
      { class: 'aside-block', 'aria-labelledby': 'ov-ins-title' },
      h('h2', { id: 'ov-ins-title' }, t('overview.systemObjectsComputer')),
      h('p', { class: 'muted small' }, t('overview.outsideScansScopeManaged')),
      h(
        'ul',
        { class: 'sys-list' },
        ...insightCards(d.items).map((c) => h('li', { title: c.observed ? t('overview.observedObserved', { observed: formatTime(c.observed) }) : t('overview.observationTimeUnknown') }, h('span', {}, c.title), h('span', { class: 'num' }, c.size))),
      ),
      open,
    );
  }

  private hostCard(): HTMLElement | null {
    const caps = this.app.store.state.caps;
    if (!caps) return null;
    return h(
      'details',
      { class: 'aside-block host-card' },
      h('summary', {}, t('overview.permissionsCapabilities')),
      h(
        'dl',
        { class: 'facts-list small' },
        h('dt', {}, t('common.writeOperations')),
        h('dd', {}, caps.write_commands ? t('overview.runsOnlyStoredPlans') : t('overview.desktopVersionNoCommand')),
        h('dt', {}, t('overview.readingPastPermissions')),
        h(
          'dd',
          {},
          caps.bypass_privileges_disabled === null
            ? t('overview.notApplicable')
            : caps.bypass_privileges_disabled
              ? t('overview.turnedOffStartupBackup')
              : t('overview.couldntTurnOffP', { p: caps.bypass_privileges_note ?? '' }),
        ),
        h('dt', {}, t('overview.indexLocation')),
        h('dd', {}, caps.store_dir_display ? pathText(caps.store_dir_display, { max: 30, copy: true }) : t('common.unavailable')),
        h('dt', {}, t('common.engine')),
        h('dd', {}, t('common.engineVersionProtocolSchema', { engine_version: caps.engine_version, schema_version: caps.schema_version })),
      ),
      caps.store_problem ? h('p', { class: 'form-error' }, refusalText(caps.store_problem)) : null,
      h(
        'ul',
        { class: 'cap-list' },
        ...caps.capabilities.map((c) => h('li', {}, h('code', {}, c.name), h('span', { class: 'muted' }, c.state === 'unverified' ? t('overview.notYetVerifiedReal') : c.state))),
      ),
    );
  }
}
