// P05 — plan review and authorization (GUI-03/04, PLN-01..07). Shows the complete stored
// plan: mode and what it does to space and recovery, every item (paged, never a top-N
// summary), rule versions, digest, expiry, what is not included, and where authorization
// comes from. Approval is the person typing the digest's first 8 characters; it writes the
// same receipt the CLI's `plan review` writes. The same plan id works in the CLI.

import type { App } from '../main';
import { toIpcError } from '../lib/api';
import { formatBytes, formatExactBytes, parseBytes } from '../lib/bytes';
import { refusalText, unknownCodeText } from '../lib/codes';
import { h, icon, keepFocus, replace } from '../lib/dom';
import { formatTime } from '../lib/format';
import { authView, kindVerb, NO_SPACE, planCapability, planExplain, planStatement } from '../lib/planModel';
import type { IpcError, PlanDetail, PlanExcluded, PlanItem } from '../lib/types';
import { AUTH_LABEL, OPERATION_STATE_LABEL, PLAN_KIND_LABEL, POLICY_MISMATCH_LABEL, POLICY_STATE_LABEL, digestGroups, formatCountdown, normalizeCode } from '../lib/words';
import { copyField, errorLine, pageHead, pill, type Tone } from './parts';
import { EXTENSIONS, TOOLS, capabilityOff, type GatedSection } from '../lib/nav';
import { words } from '../lib/platform';
import type { Page } from '../lib/store';
import { LEFTOVER_GROUP, ROUTE_WORD, TASK_WORDS } from '../lib/suiteWords';
import { noticeLine, stageSteps, undoBadge } from './kit';
import { UNINSTALL_STEPS } from './uninstall';
import { rowsByApp } from './uninstallBatch';
import { emptyBlock, gateBadge, pathText, stateLine, statusBanner } from './ui';
import { openExperimentalSettings } from './gateOff';
import { capabilityLabel } from './settings';
import { processPurpose, unidentifiedReason } from '../lib/processWords';
import { adapterDisplay, rowDisplay } from '../lib/uninstallWords';
import { t } from '../i18n';

/** Words of the plan page that are not data (one table for the language switch). */
export const PLAN_PAGE_TEXT = {
  get title() { return t('plan.title'); },
  get review() { return t('plan.review'); },
  get noneTitle() { return t('plan.noneTitle'); },
  get noneBody() { return t('plan.noneBody'); },
  get toCleanup() { return t('plan.toCleanup'); },
  get loadFailed() { return t('plan.loadFailed'); },
  get loading() { return t('plan.loading'); },
  get countsLabel() { return t('plan.countsLabel'); },
  get irreversibleSr() { return t('plan.irreversibleSr'); },
  get everyItemIrreversible() { return t('plan.everyItemIrreversible'); },
  get approvePermanent() { return t('plan.approvePermanent'); },
  get ledger() { return t('plan.ledger'); },
} as const;

/** Human name of the page a plan was opened from (its back link). */
function pageName(p: Page): string {
  for (const t of TOOLS) for (const sec of t.sections) if (sec.page === p) return sec.label;
  const ext = EXTENSIONS.find((e) => e.page === p);
  return ext ? ext.label : p === 'plan' ? t('common.plan') : t('plan.previousPage');
}

/** Item kinds that have a size (files and folders); registry keys and startup entries do not. */
const SIZED = new Set(['file', 'directory']);

const ITEM_PAGE = 100;

/** Why a selected row was left out of a plan when it was frozen (R3-07), in words. */
export function excludedReason(reason: string): string {
  return reason === 'changed' ? t('plan.excludedChanged') : reason === 'missing' ? t('plan.excludedMissing') : t('plan.excludedOther');
}

/** R3-07: the selected rows this plan left out (changed or gone since discovery), each with its reason. */
function excludedList(excluded: PlanExcluded[]): HTMLElement | null {
  if (!excluded.length) return null;
  return h(
    'div',
    { class: 'notice warn plan-excluded', id: 'plan-excluded', role: 'note' },
    h('h3', {}, t('plan.excludedTitle', { n: excluded.length })),
    h(
      'ul',
      {},
      ...excluded.map((x) => h('li', {}, h('span', { class: 'obj-name' }, x.name), ' ', pathText(x.path_display, { max: 64, className: 'obj-path' }), ' ', h('span', { class: 'obj-reason', title: x.detail }, excludedReason(x.reason)))),
    ),
  );
}

export class PlanPage {
  el: HTMLElement;
  private body: HTMLElement;
  private planId: string | null = null;
  private detail: PlanDetail | null = null;
  private error: IpcError | null = null;
  private items: PlanItem[] = [];
  private itemsNext: string | null = null;
  private itemsTotal = 0;
  private itemsLoading = false;
  private itemsError: IpcError | null = null;
  private typed = '';
  private busy = false;
  private actionError: IpcError | null = null;
  private policyNote: string | null = null;
  private timer: number | null = null;
  private loadedAt = 0;

  constructor(private app: App) {
    this.body = h('div', { class: 'plan-body' });
    this.el = h('section', { class: 'page page-plan', 'aria-labelledby': 'plan-title' }, this.body);
  }

  activate(): void {
    const id = this.app.store.state.planId;
    if (id) void this.load(id, true);
    if (this.timer === null) this.timer = window.setInterval(() => this.tick(), 1000);
  }

  update(): void {
    const id = this.app.store.state.planId;
    if (id !== this.planId && id) void this.load(id, true);
    if (this.app.store.state.page !== 'plan' && this.timer !== null) {
      window.clearInterval(this.timer);
      this.timer = null;
    }
    keepFocus(this.el, () => this.render());
  }

  private tick(): void {
    if (this.app.store.state.page !== 'plan' || !this.detail) return;
    const left = this.secondsLeft();
    const el = this.el.querySelector('[data-countdown]');
    if (el) el.textContent = left > 0 ? t('plan.leftLeft', { left: formatCountdown(left) }) : t('plan.expired');
    if (left === 0 && !this.detail.expired && this.planId) void this.load(this.planId, false);
  }

  private secondsLeft(): number {
    if (!this.detail) return 0;
    return Math.max(0, this.detail.seconds_left - Math.floor((Date.now() - this.loadedAt) / 1000));
  }

  private async load(planId: string, resetItems: boolean): Promise<void> {
    const switching = planId !== this.planId;
    this.planId = planId;
    if (switching) {
      this.detail = null;
      this.typed = '';
      this.actionError = null;
      this.policyNote = null;
    }
    this.error = null;
    try {
      const d = await this.app.backend.planDetail(planId);
      if (this.planId !== planId) return;
      this.detail = d;
      this.loadedAt = Date.now();
    } catch (e) {
      if (this.planId === planId) this.error = toIpcError(e);
    }
    if (resetItems) await this.loadItems(false);
    this.update();
  }

  private async loadItems(more: boolean): Promise<void> {
    const id = this.planId;
    if (!id || this.itemsLoading) return;
    this.itemsLoading = true;
    this.itemsError = null;
    if (!more) {
      this.items = [];
      this.itemsNext = null;
    }
    this.update();
    try {
      const page = await this.app.backend.planItems(id, ITEM_PAGE, more ? this.itemsNext : null);
      if (this.planId !== id) return;
      this.items = more ? [...this.items, ...page.items] : page.items;
      this.itemsNext = page.next_cursor;
      this.itemsTotal = page.total;
    } catch (e) {
      this.itemsError = toIpcError(e);
    } finally {
      this.itemsLoading = false;
      this.update();
    }
  }

  private async approve(): Promise<void> {
    const d = this.detail;
    if (!d || !d.review_token || this.busy) return;
    this.busy = true;
    this.actionError = null;
    this.update();
    try {
      await this.app.backend.approvePlan(d.plan_id, d.review_token, normalizeCode(this.typed));
      this.typed = '';
      this.app.announce(t('plan.planApprovedHasntRun'));
      await this.load(d.plan_id, false);
      // Permanent actions never take the default focus; the safe path stays first.
      const permanent = kindVerb(d, this.items).permanent;
      requestAnimationFrame(() => document.getElementById(permanent ? 'auth-title' : 'plan-apply')?.focus());
    } catch (e) {
      this.actionError = toIpcError(e);
      if (this.actionError.reason === 'review_expired' || this.actionError.reason === 'plan_changed') await this.load(d.plan_id, false);
      // Turned off elsewhere (the CLI) since the page was drawn: re-read the switch so the panel shows the off state.
      if (this.actionError.reason === 'experimental_off') await this.refreshGates();
    } finally {
      this.busy = false;
      this.update();
    }
  }

  private async refreshGates(): Promise<void> {
    const caps = await this.app.backend.capabilities().catch(() => null);
    if (caps) this.app.store.set({ caps });
  }

  private async revoke(): Promise<void> {
    const d = this.detail;
    if (!d || this.busy) return;
    this.busy = true;
    try {
      await this.app.backend.revokePlan(d.plan_id);
      this.app.announce(t('plan.approvalRevoked'));
      await this.load(d.plan_id, false);
    } catch (e) {
      this.actionError = toIpcError(e);
    } finally {
      this.busy = false;
      this.update();
    }
  }

  private async apply(): Promise<void> {
    const d = this.detail;
    if (!d || this.busy) return;
    this.busy = true;
    this.actionError = null;
    this.update();
    try {
      await this.app.applyPlan(d.plan_id, d.kind === 'maintenance' ? 'maintenance' : 'apply');
    } catch (e) {
      this.actionError = toIpcError(e);
      if (this.actionError.reason === 'experimental_off') await this.refreshGates();
    } finally {
      this.busy = false;
      this.update();
    }
  }

  private async policyFromPlan(): Promise<void> {
    const d = this.detail;
    if (!d) return;
    try {
      const list = await this.app.backend.createPolicyFromPlan(d.plan_id);
      const p = list[list.length - 1];
      this.policyNote = t('plan.createdPolicyPPaused', { p: p?.policy_id ?? '' });
    } catch (e) {
      this.policyNote = null;
      this.actionError = toIpcError(e);
    }
    this.update();
  }

  /** D15 / WFA-02: the plan's capability, when it is off here (the host refuses approving and running it). */
  private gateOff(d: PlanDetail): GatedSection | null {
    const cap = planCapability(d);
    return cap ? capabilityOff(cap, this.app.store.state.caps?.gates) : null;
  }

  // ---- rendering ---------------------------------------------------------------------------

  private back(): { label: string; onClick: () => void } {
    const from = this.app.store.state.returnTo;
    if (from === 'activity') return { label: t('common.backActivity'), onClick: () => this.app.navigate('activity') };
    if (from && from !== 'plan' && from !== 'operation') return { label: t('plan.back', { from: pageName(from) }), onClick: () => this.app.navigate(from) };
    return { label: t('plan.backCleanupCandidates'), onClick: () => this.app.navigate('cleanup') };
  }

  private render(): void {
    const P = PLAN_PAGE_TEXT;
    if (!this.planId) {
      replace(this.body, h('h1', { id: 'plan-title' }, P.title), emptyBlock({ title: P.noneTitle, body: P.noneBody, actions: [h('button', { type: 'button', class: 'primary', onclick: () => this.app.navigate('cleanup') }, P.toCleanup)] }));
      return;
    }
    if (this.error && !this.detail) {
      replace(this.body, pageHead({ id: 'plan-title', title: P.title, back: this.back() }), statusBanner({ kind: 'failed', level: 2, titleId: 'plan-load-title', title: P.loadFailed, sentence: refusalText(this.error) }));
      return;
    }
    const d = this.detail;
    if (!d) {
      replace(this.body, pageHead({ id: 'plan-title', title: P.review, back: this.back() }), h('p', { class: 'muted', role: 'status' }, P.loading));
      return;
    }
    const tone: Tone = d.authorization.status === 'authorized' ? 'good' : d.authorization.status === 'required' ? 'busy' : 'muted';
    const off = !d.operation && !d.stale_reason && (d.authorization.status === 'required' || d.authorization.status === 'authorized') ? this.gateOff(d) : null;
    const left = this.secondsLeft();
    replace(
      this.body,
      pageHead({
        id: 'plan-title',
        title: t('plan.reviewP', { p: d.catalog === 'purge' ? t('plan.buildArtifactCleanupPlan') : d.catalog === 'installer' ? t('plan.installerCleanupPlan') : PLAN_KIND_LABEL[d.kind] }),
        back: this.back(),
        meta: [
          // Off by default (D15): the plain badge, not "waiting for your approval" — there is nothing to approve here now.
          off ? gateBadge('off') : pill(d.operation ? t('plan.runP', { p: OPERATION_STATE_LABEL[d.operation.state] ?? d.operation.state }) : AUTH_LABEL[d.authorization.status], d.operation ? 'muted' : tone),
          t('plan.frozenCreated', { created_at: formatTime(d.created_at) }),
          h('span', { 'data-countdown': '' }, left > 0 ? t('plan.leftLeft', { left: formatCountdown(left) }) : t('plan.expired')),
        ],
      }),
      d.kind === 'uninstall' || d.kind === 'uninstall_leftovers' ? stageSteps(t('plan.uninstallingTwoStepsEach'), UNINSTALL_STEPS, d.kind === 'uninstall' ? 0 : 1) : null,
      this.statementEl(d),
      h('div', { class: 'work-layout' }, h('div', { class: 'work-main' }, this.ledger(d), this.itemsPanel(d)), this.authPanel(d)),
    );
  }

  /**
   * The page's one primary element: what will happen in one sentence, the counts, and whether it
   * can be undone — before any table. A permanent plan carries a hatched edge, a warning shape and
   * the word 无法撤销, so it never depends on colour.
   */
  private statementEl(d: PlanDetail): HTMLElement {
    const s = planStatement(d, this.items);
    return h(
      'section',
      { class: 'plan-statement', 'data-mode': s.tone, 'data-irreversible': s.irreversible ? 'yes' : 'no', 'aria-labelledby': 'ps-sentence' },
      h('h2', { id: 'ps-sentence', class: 'ps-sentence' }, s.irreversible ? h('span', { class: 'sr-only' }, t('plan.irreversiblesr', { irreversibleSr: PLAN_PAGE_TEXT.irreversibleSr })) : null, s.sentence),
      h(
        'ul',
        { class: 'ps-counts', 'aria-label': PLAN_PAGE_TEXT.countsLabel },
        ...s.counts.map((c) => h('li', {}, h('span', { class: 'ps-count-label' }, c.label), c.path ? pathText(c.value, { max: 40, className: 'ps-count-value' }) : h('span', { class: 'ps-count-value num' }, c.value))),
      ),
      h(
        'div',
        { class: 'ps-undo' },
        undoBadge(s.undo, s.undoLabel),
        s.undoText ? h('p', { class: 'ps-undo-text' }, s.undoText) : null,
      ),
      s.spaceText ? h('p', { class: `ps-space${s.tone === 'quarantine' ? ' strong-note' : ''}` }, s.spaceText) : null,
    );
  }

  /** Secondary: where the plan comes from and what binds it; then what is not in it. */
  private ledger(d: PlanDetail): HTMLElement {
    const total = parseBytes(d.total_logical_bytes);
    const facts: (HTMLElement | null)[] = [];
    const fact = (k: string, ...v: (Node | string | null)[]) => facts.push(h('dt', {}, k), h('dd', {}, ...v));
    const explain = planExplain(d);
    if (explain) fact(t('plan.what'), explain.effect);
    if (d.kind === 'manual_trash') {
      fact(t('plan.sourceScan'), pathText(d.scan_root_display ?? t('plan.scopeUnavailable'), { max: 56 }), h('small', {}, t('plan.scanPRevisionP2', { p: d.scan_id ?? '—', p2: d.scan_revision ?? '—' })));
    }
    if (d.kind === 'maintenance' && d.kind_detail?.admin_tasks)
      fact(t('plan.permissions'), t('plan.adminTasksAdmin', { admin_tasks: d.kind_detail.admin_tasks, admin: words().admin }), h('small', {}, d.kind_detail.is_admin ? t('plan.cleanmanagerRunningAdministratorTasks') : words().maintAdminReview));
    else if (d.requires_admin) fact(t('plan.permissions'), t('plan.requiresAdmin', { admin: words().admin }), h('small', {}, t('plan.cleanmanagerNeverElevatesOwn')));
    const kd = d.kind_detail;
    const batch = d.kind === 'uninstall' && (kd?.apps?.length ?? 0) > 1 ? kd!.apps! : null;
    if (batch) {
      // UNI-11: several applications, each with its own close request, uninstaller and result.
      fact(t('common.apps'), t('plan.appCount', { length: batch.length }), h('small', {}, batch.map((a) => a.name).join(t('common.listSep'))));
      fact(t('plan.uninstallMethod'), t('plan.eachAppsOwnUninstaller'), h('small', {}, t('plan.beforeEachAppRuns')));
      const est = kd?.noted?.leftover_estimate;
      if (est && Object.keys(est).length)
        fact(t('plan.mayLeaveBehind'), Object.entries(est).map(([g, n]) => `${LEFTOVER_GROUP[g]?.title ?? g} ${n}`).join(t('common.commaSep')), h('small', {}, t('plan.estimateAllAppsTogether')));
    } else if (d.kind === 'uninstall' && kd?.app) {
      fact(t('common.app'), kd.app.name, h('small', {}, [kd.app.publisher, kd.app.version ? t('common.versionVersion', { version: kd.app.version }) : null].filter(Boolean).join(' · ')));
      fact(t('plan.uninstallMethod'), adapterDisplay(kd.app.adapter), h('small', {}, kd.app.signer ? t('plan.signerSignerProgramFiles', { signer: kd.app.signer }) : t('plan.programFilesDigestChecked')));
      const est = kd.noted?.leftover_estimate;
      if (est && Object.keys(est).length)
        fact(t('plan.mayLeaveBehind'), Object.entries(est).map(([g, n]) => `${LEFTOVER_GROUP[g]?.title ?? g} ${n}`).join(t('common.commaSep')), h('small', {}, t('plan.estimateIsntPlanIsnt')));
    }
    if (d.kind === 'uninstall_leftovers' && kd?.registry_keys) fact(t('plan.registry'), t('plan.registryKeys', { registry_keys: kd.registry_keys }), h('small', {}, t('plan.eachWholeKeyBacked')));
    if (d.kind === 'process' && kd?.noted) {
      fact(t('common.process'), t('plan.pPidP2', { p: kd.noted.name ?? '', p2: kd.noted.pid ?? '—' }), h('small', {}, processPurpose({ name: kd.noted.name ?? '', identified: !!kd.noted.purpose, purpose: kd.noted.purpose ?? null })?.text ?? (kd.noted.unidentified_reason ? t('plan.unidentifiedUnidentifiedReason', { unidentified_reason: unidentifiedReason(kd.noted.unidentified_reason).text }) : '')));
    }
    if (d.kind === 'cleanup') {
      fact(t('plan.sourceScan'), pathText(d.scan_root_display ?? t('plan.scopeUnavailable'), { max: 56 }), h('small', {}, t('plan.scanPRevisionP22', { p: d.scan_id ?? '—', p2: d.scan_revision ?? '—' })));
      fact(d.catalog ? t('plan.builtCatalogRules') : t('plan.ruleVersion'), d.rule_refs.length ? h('span', { class: 'rule-list' }, ...d.rule_refs.map((r) => h('code', { class: 'selectable' }, r))) : '—');
      fact(t('plan.objects'), t('common.groupdigits', { groupDigits: d.item_count ?? 0 }), h('small', {}, t('plan.defaultSelectedItemsLow', { default_selected_items: d.default_selected_items })));
    }
    if (!NO_SPACE.has(d.kind)) fact(t('plan.knownLogicalSize'), total === null ? t('common.unknown') : formatBytes(total), h('small', {}, total === null ? t('plan.filesUnknownSizeArent') : formatExactBytes(total)));
    if (d.allocated_known_bytes !== null && !NO_SPACE.has(d.kind))
      fact(t('common.allocatedSize'), d.allocated_unknown_count ? t('plan.leastAllocatedKnownBytes', { allocated_known_bytes: formatBytes(d.allocated_known_bytes), allocated_unknown_count: d.allocated_unknown_count }) : formatBytes(d.allocated_known_bytes));
    fact(t('plan.validUntil'), formatTime(d.expires_at), h('small', {}, t('plan.planValid15Minutes')));
    return h(
      'section',
      { class: 'panel ledger', 'aria-labelledby': 'ledger-title' },
      h('h2', { id: 'ledger-title' }, PLAN_PAGE_TEXT.ledger),
      h('dl', { class: 'fact-grid' }, ...facts),
      this.notIncluded(d),
    );
  }

  private notIncluded(d: PlanDetail): HTMLElement {
    const lines: string[] = [];
    if (d.kind === 'cleanup') {
      lines.push(t('plan.foldersThemselvesArentList'));
      const n = d.not_included;
      if (n) {
        if (n.low_unselected || n.review_unselected) lines.push(t('plan.candidatesDidntSelectLow', { low_unselected: n.low_unselected, review_unselected: n.review_unselected }));
        if (n.pending) lines.push(t('plan.pendingIncompleteEvidence', { pending: n.pending }));
        if (n.blocked) lines.push(t('plan.blocked', { blocked: n.blocked }));
      } else {
        lines.push(t('plan.newerAnalysisReplacedSource'));
      }
      lines.push(t('plan.eachItemCheckedAgain'));
    } else if (d.kind === 'manual_trash' || d.kind === 'manual_purge') {
      // WFA-65: a permanent-delete plan holds items picked on the bin page, never scanned objects.
      lines.push(d.kind === 'manual_purge' ? t('plan.containsOnlyBinItemsPicked', { bin: words().bin }) : t('plan.containsOnlyObjectsPicked'));
      lines.push(t('plan.eachObjectsIdentityChecked'));
    } else if (d.kind === 'startup' || d.kind === 'startup_restore') {
      lines.push(t('plan.doesntIncludeProgramFiles'));
      lines.push(t('plan.eachItemReadAgain'));
    } else if (d.kind === 'uninstall') {
      lines.push(t('plan.containsNoLeftoversLeftoverkinds', { leftoverKinds: words().leftoverKinds }));
      lines.push(t('plan.appsNeverForceQuit'));
      lines.push(t('plan.uninstallRegistrationProgramFile'));
      if ((d.kind_detail?.apps?.length ?? 0) > 1) lines.push(t('plan.appDoesntQuitFails'));
      const refused = d.kind_detail?.noted?.refused ?? [];
      if (refused.length) lines.push(t('plan.lengthPlanRefusedJust', { length: refused.length, join: refused.map((r) => t('plan.pRefusaltext', { p: r.name ?? r.app_instance_id, refusalText: refusalText({ reason: r.reason }) })).join(t('common.clauseSep')) }));
    } else if (d.kind === 'uninstall_leftovers') {
      lines.push(t('plan.containsOnlyObjectsSelected'));
      lines.push(t('plan.foldersOnlyContentsMoved'));
    } else if (d.kind === 'update') {
      const routed = d.kind_detail?.noted?.routed ?? [];
      lines.push(words().updatesPlanScope);
      if (routed.length) lines.push(t('plan.lengthPlanJoin', { length: routed.length, join: routed.map((r) => t('common.nameP', { name: r.name, p: ROUTE_WORD[r.route] })).join(t('common.listSep')) }));
      lines.push(t('plan.runningAppsSkippedClose'));
    } else if (d.kind === 'process') {
      lines.push(t('plan.containsOnlyOneProcess'));
      lines.push(t('plan.skippedIfPidReused'));
    } else if (d.kind === 'maintenance') {
      lines.push(t('plan.containsOnlyListedTasks'));
      lines.push(t('plan.tasksProtectedTasksWhose'));
    } else {
      lines.push(t('plan.containsOnlySelectedQuarantine'));
      lines.push(d.kind === 'restore' ? t('plan.ifObjectSameName') : t('plan.emptyingRecoversNothingCreate'));
    }
    return h('div', { class: 'not-included' }, excludedList(d.excluded ?? []), h('h3', {}, t('plan.notIncluded')), h('ul', {}, ...lines.map((l) => h('li', {}, l))));
  }

  private itemsPanel(d: PlanDetail): HTMLElement {
    const recovery = d.kind === 'restore' || d.kind === 'purge';
    const toggles = NO_SPACE.has(d.kind);
    const irreversible = kindVerb(d, this.items).permanent;
    const ACTION: Record<string, string> = {
      startup_disable: t('plan.itemsPanel.startup_disable'),
      startup_enable: t('plan.itemsPanel.startup_enable'),
      startup_remove_with_backup: t('plan.itemsPanel.startup_remove_with_backup'),
      registry_restore: t('plan.itemsPanel.registry_restore'),
      registry_backup_delete: t('plan.itemsPanel.registry_backup_delete'),
      fs_trash: words().moveToBin,
      fs_delete: t('plan.itemsPanel.fs_delete'),
      app_quit: t('plan.itemsPanel.app_quit'),
      process_end: t('plan.itemsPanel.process_end'),
    };
    const actionText = (i: PlanItem): string | null => {
      if (i.item_kind === 'uninstaller') return words().uninstallerAction;
      if (i.item_kind === 'app_update') {
        const det = i.detail ?? {};
        if (det.channel === 'homebrew') return t('plan.updatedThroughHomebrewHomebrew');
        return t('plan.updatedThroughStringInstallers', { String: String(det.channel ?? t('plan.packageManager')), String2: String(det.installed_signer ?? t('plan.installedAppsSigner')) });
      }
      if (i.item_kind === 'maintenance_task') return i.scope === 'machine' ? t('plan.requiresAdmin', { admin: words().admin }) : t('plan.runsCurrentUser');
      if (i.action_type && ACTION[i.action_type]) return ACTION[i.action_type];
      return null;
    };
    const nameOf = (i: PlanItem): string => (i.item_kind === 'maintenance_task' ? (TASK_WORDS[String(i.detail?.task_id ?? '')]?.title ?? i.name) : i.name);
    const rowOf = (i: PlanItem) =>
      h(
        'tr',
        {},
        h('td', { class: 'col-id' }, h('code', {}, i.action_id)),
        h(
          'td',
          { class: 'col-object' },
          h('span', { class: 'obj-name' }, nameOf(i), i.item_kind === 'directory' && d.kind !== 'uninstall_leftovers' ? h('span', { class: 'muted small' }, t('plan.emptyFolderRemovedOnly')) : null),
          recovery ? h('span', { class: 'obj-path' }, t('plan.returnsTo'), pathText(i.original_path_display ?? '—', { max: 72 })) : pathText(rowDisplay(i.path_display, i.item_kind === 'uninstaller'), { max: 76, className: 'obj-path' }),
          d.kind !== 'cleanup' && actionText(i) ? h('span', { class: 'obj-reason' }, actionText(i), i.scope && i.item_kind !== 'maintenance_task' ? t('common.parenthetical', { p: i.scope === 'user' ? t('plan.currentUser') : t('plan.allUsers') }) : '') : null,
          i.detail?.command ? h('span', { class: 'obj-reason muted' }, h('code', {}, String(i.detail.command))) : null,
        ),
        toggles ? null : h('td', { class: 'col-size num', title: SIZED.has(i.item_kind) ? formatExactBytes(parseBytes(i.logical_bytes)) : '' }, SIZED.has(i.item_kind) ? formatBytes(i.logical_bytes) : '—'),
      );
    const table = (items: PlanItem[], caption: string) =>
      h(
        'div',
        { class: 'table-wrap table-scroll' },
        h(
          'table',
          { class: 'data-table' },
          h('caption', { class: 'sr-only' }, caption),
          h('thead', {}, h('tr', {}, h('th', { scope: 'col', class: 'col-id' }, t('plan.action')), h('th', { scope: 'col' }, t('plan.object')), toggles ? null : h('th', { scope: 'col', class: 'col-size' }, t('common.logicalSize')))),
          h('tbody', {}, ...items.map(rowOf)),
        ),
      );
    // UNI-11: an uninstall plan for several applications is reviewed application by application.
    const apps = d.kind === 'uninstall' ? (d.kind_detail?.apps ?? []) : [];
    const body =
      apps.length > 1
        ? rowsByApp(this.items, apps).flatMap((g, n) =>
            g.items.length ? [h('h3', { id: `items-app-${n}` }, `${n + 1}. ${g.name}`, h('span', { class: 'muted small' }, t('plan.actionCount', { length: g.items.length }))), table(g.items, t('plan.actionsName', { name: g.name }))] : [],
          )
        : [table(this.items, irreversible ? t('plan.everyActionPlanIrreversiblesr', { irreversibleSr: PLAN_PAGE_TEXT.irreversibleSr }) : t('plan.everyActionPlan'))];
    return h(
      'section',
      { class: 'panel items-panel', 'aria-labelledby': 'items-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'items-title' }, t('plan.fullListGroupdigits', { groupDigits: d.item_count ?? 0 })), irreversible ? undoBadge('irreversible', PLAN_PAGE_TEXT.everyItemIrreversible) : null),
      h('p', { class: 'muted small' }, apps.length > 1 ? t('plan.groupedAppRunPlan', { length: apps.length }) : t('plan.everyActionPlanOrder')),
      this.items.length ? h('div', {}, ...body) : h('p', { class: 'muted small', role: 'status' }, this.itemsLoading ? t('plan.loadingList') : t('plan.listEmpty')),
      errorLine(this.itemsError),
      this.itemsNext
        ? h('button', { type: 'button', class: 'quiet more', id: 'plan-items-more', disabled: this.itemsLoading, onclick: () => void this.loadItems(true) }, this.itemsLoading ? t('common.loading') : t('common.showMoreLengthItemstotal', { length: this.items.length, itemsTotal: this.itemsTotal }))
        : null,
    );
  }

  /** The decision: one state line, then only the action that state allows. */
  private authPanel(d: PlanDetail): HTMLElement {
    const a = d.authorization;
    const { run, permanent: danger } = kindVerb(d, this.items);
    const off = this.gateOff(d);
    const v = authView(d, !!off);
    const blocks: (HTMLElement | null)[] = [];
    if (v.line) blocks.push(stateLine({ kind: v.line.kind, text: v.line.text, className: 'auth-line' }));
    if (a.status === 'authorized' && !d.operation)
      blocks.push(h('p', { class: 'small muted auth-source' }, a.source === 'policy' ? t('plan.sourcePolicyPolicyId', { policy_id: a.policy_id, policy_version: a.policy_version }) : t('plan.sourceManualReviewP', { p: a.principal ?? '', authorized_at: formatTime(a.authorized_at) })));
    if (v.view === 'executed') {
      blocks.push(h('button', { type: 'button', class: 'primary wide', id: 'plan-view-result', onclick: () => this.app.openOperation(d.operation!.operation_id, 'plan') }, t('plan.viewResult')));
    } else if (v.view === 'gated' && off) {
      // The capability is off: no seal, no approve or run button (the host would refuse them); the way to turn it on.
      const open = h('button', { type: 'button', class: 'primary wide', id: 'plan-open-experimental', 'data-open-experimental': '' }, t('kit.openExperimentalFeaturesSettings'));
      open.addEventListener('click', () => openExperimentalSettings(this.app));
      blocks.push(
        h('p', { class: 'small' }, t('plan.gatedFeature', { feature: capabilityLabel(off.capability) })),
        h('p', { class: 'small muted' }, t('plan.gatedKeepsPlan')),
        open,
      );
    } else if (v.view === 'terminal') {
      blocks.push(
        noticeLine(
          d.terminal_only === 'end_processes'
            ? t('plan.planMayEndProcesses')
            : t('plan.planPermanentlyDeletesObjects'),
          'warn',
        ),
        copyField(t('plan.reviewApproveTerminal'), d.cli.review),
        copyField(t('plan.runTerminal'), d.cli.apply),
      );
    } else if (v.view === 'review') {
      blocks.push(this.seal(d, danger));
    } else if (v.view === 'authorized') {
      const apply = h('button', { type: 'button', id: 'plan-apply', class: `${danger ? 'danger' : 'primary'} wide`, disabled: this.busy }, danger ? icon('warn') : null, run);
      apply.addEventListener('click', () => void this.apply());
      const revoke = h('button', { type: 'button', class: 'quiet', id: 'plan-revoke', disabled: this.busy }, t('plan.revokeApproval'));
      revoke.addEventListener('click', () => void this.revoke());
      blocks.push(
        apply,
        h('p', { class: 'muted small' }, d.mode === 'quarantine' ? t('plan.doesntFreeSpaceRight') : danger ? (d.kind === 'process' ? t('plan.endingProcessesCantUndone') : t('plan.permanentDeletionCantUndone')) : t('plan.stopRemainingActionsDuring')),
        revoke,
      );
    } else {
      blocks.push(h('button', { type: 'button', class: 'wide', onclick: () => this.back().onClick() }, this.back().label));
    }
    // The off state already says why nothing can be approved; the host's refusal of an earlier click would repeat it.
    blocks.push(v.view === 'gated' && this.actionError?.reason === 'experimental_off' ? null : errorLine(this.actionError));
    return h(
      'aside',
      { class: 'panel work-aside auth-panel', 'aria-labelledby': 'auth-title' },
      h('h2', { id: 'auth-title', tabindex: '-1' }, t('plan.authorization')),
      ...blocks,
      d.kind === 'cleanup' && !d.catalog
        ? this.policies(d)
        : h('p', { class: 'muted small' }, d.kind === 'cleanup' ? t('plan.buildArtifactInstallerPlans') : t('plan.plansKindOnlyAuthorized')),
      d.cli.show ? this.handoff(d) : null,
    );
  }

  /** The digest as the plan's seal; approving means typing its first group. */
  private seal(d: PlanDetail, danger: boolean): HTMLElement {
    const groups = digestGroups(d.digest);
    const input = h('input', {
      id: 'plan-code',
      class: 'code-input',
      type: 'text',
      inputmode: 'latin',
      autocomplete: 'off',
      autocapitalize: 'off',
      spellcheck: 'false',
      maxlength: '9',
      placeholder: '········',
      'aria-describedby': 'seal-help',
    }) as HTMLInputElement;
    input.value = this.typed;
    const approve = h('button', { type: 'button', id: 'plan-approve', class: `${danger ? 'danger' : 'primary'} wide`, disabled: normalizeCode(this.typed) !== d.digest_prefix || this.busy }, danger ? icon('warn') : null, this.busy ? t('plan.approving') : danger ? PLAN_PAGE_TEXT.approvePermanent : t('plan.approvePlan')) as HTMLButtonElement;
    input.addEventListener('input', () => {
      this.typed = input.value;
      approve.disabled = normalizeCode(this.typed) !== d.digest_prefix || this.busy;
      const ok = normalizeCode(this.typed) === d.digest_prefix;
      input.classList.toggle('match', ok);
    });
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && !approve.disabled) void this.approve();
    });
    approve.addEventListener('click', () => void this.approve());
    return h(
      'div',
      { class: 'seal', 'data-irreversible': danger ? 'yes' : 'no' },
      h('p', { class: 'seal-label' }, t('plan.planDigest')),
      h('p', { class: 'seal-digest', 'aria-label': t('plan.planDigestFirst8', { join: d.digest_prefix.split('').join(' ') }) }, ...groups.map((g, i) => h('span', { class: i === 0 ? 'grp first' : 'grp' }, g))),
      h('label', { for: 'plan-code', class: 'seal-ask' }, t('plan.typeFirst8Characters')),
      input,
      h('p', { id: 'seal-help', class: 'muted small' }, t('plan.digestBindsListMode', { p: d.mode === 'delete' ? t('plan.permanentDeletionCantUndone2') : t('plan.stillStartRunNext') })),
      approve,
    );
  }

  private policies(d: PlanDetail): HTMLElement {
    const rows = d.policies.map((p) =>
      h(
        'li',
        {},
        h('code', { class: 'selectable' }, p.policy_id),
        h('span', { class: 'small' }, t('plan.pP2', { p: POLICY_STATE_LABEL[p.state], p2: p.covers ? t('plan.coversPlanLimitsStill') : t('plan.doesntCoverP', { p: POLICY_MISMATCH_LABEL[p.reason ?? ''] ?? (p.reason ? unknownCodeText(p.reason) : '') }) })),
      ),
    );
    const create = h('button', { type: 'button', class: 'quiet', id: 'policy-from-plan' }, t('plan.createLimitedPolicyPlans'));
    create.addEventListener('click', () => void this.policyFromPlan());
    return h(
      'details',
      { class: 'aside-section', open: d.policies.some((p) => p.covers) ? true : null },
      h('summary', {}, t('plan.preAuthorizationPoliciesLength', { length: rows.length })),
      rows.length ? h('ul', { class: 'policy-mini' }, ...rows) : h('p', { class: 'muted small' }, t('plan.noPoliciesDefaultEvery')),
      this.policyNote ? h('p', { class: 'small', role: 'status' }, this.policyNote) : null,
      d.default_selected_items === d.item_count ? create : h('p', { class: 'muted small' }, t('plan.policiesOnlyCoverPlans')),
    );
  }

  private handoff(d: PlanDetail): HTMLElement {
    return h(
      'details',
      { class: 'aside-section' },
      h('summary', {}, t('plan.usePlanTerminal')),
      h('p', { class: 'muted small' }, t('plan.cliReadsSamePlan')),
      h('p', { class: 'small' }, t('plan.planId'), h('code', { class: 'selectable' }, d.plan_id)),
      copyField(t('plan.show'), d.cli.show),
      copyField(t('plan.reviewApprove'), d.cli.review),
      copyField(t('plan.run'), d.cli.apply),
      h('p', { class: 'muted small' }, t('plan.automationCantApprovePlans')),
    );
  }
}
