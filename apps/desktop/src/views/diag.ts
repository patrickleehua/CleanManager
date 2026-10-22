// Optional AI diagnosis workbench (AID-01..13). One object at a time: the person previews what
// would be sent, confirms, and reads a conditional explanation whose every claim links to the
// local evidence it cites. Recommendations come back already checked by the core; next steps
// are the core's (plan with this one candidate, protect, locate, app page, checklist). Nothing
// here deletes, approves or runs a command. Navigation never cancels a run.

import type { App } from '../main';
import { toIpcError } from '../lib/api';
import { formatBytes } from '../lib/bytes';
import { h, icon, keepFocus, replace } from '../lib/dom';
import { formatTime } from '../lib/format';
import { RequestGate } from '../lib/gate';
import type { Diagnosis, DiagStatus, DiagSummary, DiagTarget, Disclosure, Handoff, Preview, Recommendation } from '../lib/diagTypes';
import type { IpcError } from '../lib/types';
import {
  ACTION_BUTTON,
  ACTION_LABEL,
  CLAIM_LABEL,
  FAILURE_HINT,
  LAYER_LABEL,
  PROVIDER_LABEL,
  REC_EXPLAIN,
  REC_LABEL,
  STATE_LABEL,


  stateTone,
} from '../lib/diagWords';
import { emptyState, errorText, pageHead, pill } from './parts';
import { openPreview } from './diagPreview';
import { techBlock } from './ui';
import { words } from '../lib/platform';
import { t } from '../i18n';

const ORIGIN_LABEL: Record<string, string> = { get cleanup() { return t('diag.originLabel.cleanup'); }, get disk() { return t('diag.originLabel.disk'); }, get apps() { return t('diag.originLabel.apps'); } };
const CLAIM_TONE: Record<string, 'good' | 'warn' | 'bad' | 'busy' | 'muted'> = {
  supported: 'good',
  user_statement: 'busy',
  inference: 'muted',
  unsupported: 'warn',
  conflicts_with_facts: 'bad',
};

/** Recommendation marker: shape + word, never colour alone. */
/** The back link to the page the workbench was opened from (a plain "Back" when that page has no name here). */
function backLabel(from: string): string {
  const names: Record<string, () => string> = { disk: () => t('common.disk'), cleanup: () => t('diag.clean'), apps: () => t('common.apps'), settings: () => t('common.settings'), plan: () => t('common.plan') };
  const name = names[from]?.();
  return name ? t('diag.render.label', { p: name }) : t('diag.back');
}

export function recMark(r: Recommendation): HTMLElement {
  return h('span', { class: 'rmark', 'data-rec': r }, h('span', { class: 'rmark-shape', 'aria-hidden': 'true' }), h('span', {}, REC_LABEL[r]));
}

export class DiagPage {
  el: HTMLElement;
  private body: HTMLElement;
  private status: DiagStatus | null = null;
  private statusError: IpcError | null = null;
  private diag: Diagnosis | null = null;
  private diagError: IpcError | null = null;
  private history: DiagSummary[] | null = null;
  private busy = false;
  private message: { text: string; tone: 'good' | 'warn' } | null = null;
  private question = '';
  private others = new Map<string, string>();
  private planMode: 'trash' | 'quarantine' = 'trash';
  private showChecklist = false;
  private exportText: string | null = null;
  private boundKey = '';
  private gate = new RequestGate();

  constructor(private app: App) {
    this.body = h('div', { class: 'diag-body' });
    this.el = h('section', { class: 'page page-diag', 'aria-labelledby': 'diag-title' }, this.body);
  }

  activate(): void {
    void this.reloadStatus();
    void this.reloadHistory();
    this.bind(true);
  }

  update(): void {
    this.bind(false);
    keepFocus(this.el, () => this.render());
  }

  /** A run finished (any page may be showing). */
  async finished(id: string): Promise<void> {
    void this.reloadHistory();
    if (this.diag?.diagnosis_id === id) await this.loadDiag(id);
  }

  private key(): string {
    const s = this.app.store.state;
    return `${s.diagTarget?.scanId ?? ''}/${s.diagTarget?.entryId ?? ''}/${s.diagId ?? ''}`;
  }

  /** New target or record: drop what belongs to the previous one (late answers never apply). */
  private bind(force: boolean): void {
    const k = this.key();
    if (!force && k === this.boundKey) return;
    this.boundKey = k;
    this.gate.next();
    this.diag = null;
    this.diagError = null;
    this.message = null;
    this.exportText = null;
    this.showChecklist = false;
    this.others.clear();
    const id = this.app.store.state.diagId;
    if (id) void this.loadDiag(id);
  }

  private async reloadStatus(): Promise<void> {
    try {
      this.status = await this.app.backend.diagStatus();
      this.statusError = null;
    } catch (e) {
      this.statusError = toIpcError(e);
    }
    this.update();
  }

  private async reloadHistory(): Promise<void> {
    try {
      this.history = (await this.app.backend.diagList(null)).items;
    } catch {
      this.history = [];
    }
    this.update();
  }

  private async loadDiag(id: string): Promise<void> {
    const ticket = this.gate.current();
    try {
      const r = await this.gate.run(ticket, this.app.backend.diagGet(id));
      if (r.stale) return;
      this.diag = r.value;
      this.diagError = null;
    } catch (e) {
      this.diagError = toIpcError(e);
    }
    this.update();
  }

  // ---- actions ---------------------------------------------------------------------------

  private async start(): Promise<void> {
    const target = this.app.store.state.diagTarget;
    if (!target) return;
    const q = this.question.trim() || null;
    const prepare = (d: Disclosure) => this.app.backend.diagPrepare(target.scanId, target.entryId, target.origin, null, d, q);
    await this.runPreview(prepare, true, t('diag.confirmWhatSent'));
  }

  private async followUp(questionId: string | null, answer: string): Promise<void> {
    const d = this.diag;
    if (!d || !answer.trim()) return;
    const prepare = () => this.app.backend.diagFollowUp(d.diagnosis_id, questionId, answer.trim());
    await this.runPreview(prepare, false, t('diag.confirmWhatSentAdditions'));
  }

  private async runPreview(prepare: (d: Disclosure) => Promise<Preview>, editable: boolean, title: string): Promise<void> {
    this.busy = true;
    this.message = null;
    this.update();
    let first: Preview;
    try {
      first = await prepare({ full_path: false, child_summary: false });
    } catch (e) {
      this.busy = false;
      this.diagError = toIpcError(e);
      this.update();
      return;
    }
    this.busy = false;
    this.update();
    const d = await openPreview(this.app, { prepare, scopeEditable: editable, title }, first);
    if (!d) {
      this.app.announce(t('diag.cancelledNothingSent'));
      this.message = { text: t('diag.cancelledNothingSentFile'), tone: 'good' };
      this.update();
      return;
    }
    this.app.announce(t('diag.sentDiagnosing'));
    // The new record becomes the page's subject (its id survives navigation).
    this.app.store.set({ diagId: d.diagnosis_id });
    this.diag = d;
    void this.reloadHistory();
    this.update();
  }

  private async cancel(): Promise<void> {
    if (!this.diag) return;
    try {
      this.diag = await this.app.backend.diagCancel(this.diag.diagnosis_id);
      this.app.announce(t('diag.diagnosisCancelled'));
    } catch (e) {
      this.diagError = toIpcError(e);
    }
    void this.reloadHistory();
    this.update();
  }

  private async handoff(h0: Handoff): Promise<void> {
    const d = this.diag;
    if (!d) return;
    if (h0.action === 'verify') {
      this.showChecklist = !this.showChecklist;
      this.update();
      return;
    }
    if (h0.action === 'keep') {
      this.message = { text: t('diag.okKeepingCleanmanagerWont'), tone: 'good' };
      this.app.announce(t('diag.keptNoActionTaken'));
      this.update();
      return;
    }
    this.busy = true;
    this.update();
    try {
      const r = await this.app.backend.diagHandoff(d.diagnosis_id, h0.action, h0.action === 'review_plan' ? this.planMode : null);
      if (r.outcome === 'plan' && r.plan_id) {
        this.app.announce(t('diag.createdPlanJustItem'));
        this.app.openPlan(r.plan_id, 'diag');
      } else if (r.outcome === 'protected') {
        this.message = { text: t('diag.addedProtectionListFuture'), tone: 'good' };
        this.app.announce(t('diag.addedProtectionList'));
      } else if (r.outcome === 'disk' && r.entry_id) {
        this.app.store.set({ revealEntry: r.entry_id });
        this.app.navigate('disk');
      } else if (r.outcome === 'apps') {
        this.app.navigate('apps');
      }
    } catch (e) {
      this.message = { text: errorText(toIpcError(e)), tone: 'warn' };
    } finally {
      this.busy = false;
      this.update();
    }
  }

  private async save(): Promise<void> {
    if (!this.diag) return;
    this.diag = await this.app.backend.diagSave(this.diag.diagnosis_id);
    this.message = { text: t('diag.diagnosisSavedStaysComputer'), tone: 'good' };
    void this.reloadHistory();
    this.update();
  }

  private async remove(): Promise<void> {
    if (!this.diag) return;
    await this.app.backend.diagRemove(this.diag.diagnosis_id);
    this.app.announce(t('diag.diagnosisRecordRemovedFile'));
    this.app.store.set({ diagId: null });
    void this.reloadHistory();
  }

  private async exportPreview(): Promise<void> {
    if (!this.diag) return;
    this.exportText = this.exportText ? null : await this.app.backend.diagExport(this.diag.diagnosis_id);
    this.update();
  }

  // ---- rendering -------------------------------------------------------------------------

  private render(): void {
    const s = this.app.store.state;
    const back = s.returnTo && s.returnTo !== 'diag' ? { label: backLabel(s.returnTo), onClick: () => this.app.navigate(s.returnTo!) } : undefined;
    const target = s.diagTarget;
    const d = this.diag;
    const subject = d?.evidence.subject;
    const name = subject?.name ?? target?.name ?? null;
    const head = pageHead({
      id: 'diag-title',
      title: t('common.aiDiagnosis'),
      back,
      meta: name
        ? [
            h('b', { class: 'diag-subject' }, name),
            subject ? formatBytes(subject.logical_bytes) : null,
            t('diag.fromOrigin', { p: ORIGIN_LABEL[d?.request.object.origin ?? target?.origin ?? 'disk'] }),
            d ? pill(STATE_LABEL[d.state], stateTone(d.state)) : null,
          ]
        : [t('diag.explainsWhatSingleFile')],
    });
    const notices = h(
      'div',
      { class: 'notices' },
      this.message ? h('div', { class: `notice${this.message.tone === 'warn' ? ' warn' : ''}`, role: 'status' }, h('span', {}, this.message.text)) : null,
      this.diagError ? h('div', { class: 'notice warn', role: 'alert' }, h('span', {}, errorText(this.diagError))) : null,
    );
    if (!target && !s.diagId) {
      replace(this.body, head, notices, this.historyPage());
      return;
    }
    if (this.status && !this.status.configured && !d) {
      replace(this.body, head, notices, this.unconfigured());
      return;
    }
    replace(
      this.body,
      head,
      notices,
      h('div', { class: 'work-layout diag-layout' }, h('div', { class: 'work-main' }, ...this.mainColumn()), this.aside()),
    );
  }

  private unconfigured(): HTMLElement {
    return h(
      'div',
      { class: 'panel' },
      emptyState(
        t('diag.aiDiagnosisIsntSet'),
        t('diag.optionalExtensionUseAdd'),
        h('button', { type: 'button', class: 'primary', id: 'diag-configure', onclick: () => this.app.navigate('settings') }, t('diag.setUpAiDiagnosis')),
      ),
      this.statusError ? h('p', { class: 'form-error' }, errorText(this.statusError)) : null,
    );
  }

  private mainColumn(): (HTMLElement | null)[] {
    const d = this.diag;
    if (!d) return [this.intro()];
    if (d.state === 'running') return [this.running(d)];
    if (d.state === 'failed' || d.state === 'cancelled') return [this.stopped(d)];
    return [this.verdict(d), this.explanation(d), this.ledger(d), this.questions(d), this.nextSteps(d)];
  }

  private intro(): HTMLElement {
    const ta = h('textarea', { id: 'diag-question', class: 'diag-question', rows: 2, maxlength: 280, placeholder: t('diag.exampleFileDeletedWhat') }) as HTMLTextAreaElement;
    ta.value = this.question;
    ta.addEventListener('input', () => (this.question = ta.value));
    const go = h('button', { type: 'button', class: 'primary', id: 'diag-start', disabled: this.busy }, t('diag.previewWhatSent'));
    go.addEventListener('click', () => void this.start());
    return h(
      'div',
      { class: 'panel diag-intro' },
      h('h2', {}, t('diag.seeExactlyWhatSent')),
      h(
        'ol',
        { class: 'diag-steps' },
        h('li', {}, h('b', {}, t('diag.preview')), h('span', {}, t('diag.onlyRedactedMetadataLocal'))),
        h('li', {}, h('b', {}, t('diag.diagnose')), h('span', {}, t('diag.sentOnlyProviderSet'))),
        h('li', {}, h('b', {}, t('diag.decide')), h('span', {}, t('diag.answerFirstCheckedAgainst'))),
      ),
      h('label', { for: 'diag-question', class: 'diag-label' }, t('diag.whatWantAskOptional')),
      ta,
      h('div', { class: 'card-actions' }, go),
      h('p', { class: 'small muted' }, t('diag.aiDiagnosisHereExplains')),
    );
  }

  private running(d: Diagnosis): HTMLElement {
    const stop = h('button', { type: 'button', id: 'diag-cancel' }, t('diag.cancelDiagnosis'));
    stop.addEventListener('click', () => void this.cancel());
    return h(
      'div',
      { class: 'panel progress-panel', role: 'status' },
      h('h2', {}, t('diag.waitingAnswerLabel', { label: d.destination.label })),
      h('div', { class: 'indeterminate', 'aria-hidden': 'true' }),
      h('p', {}, t('diag.leavingPageDoesntCancel')),
      h('div', { class: 'card-actions' }, stop),
    );
  }

  private stopped(d: Diagnosis): HTMLElement {
    const retry = h('button', { type: 'button', class: 'primary', id: 'diag-retry', disabled: this.busy }, t('diag.previewDiagnoseAgain'));
    retry.addEventListener('click', () => void this.start());
    const failed = d.state === 'failed';
    return h(
      'div',
      { class: 'panel diag-stopped' },
      h('h2', {}, failed ? t('diag.diagnosisFailed') : t('diag.diagnosisCancelledTitle')),
      failed && d.failure
        ? h('div', {}, h('p', {}, h('b', {}, FAILURE_HINT[d.failure.code] ?? t('diag.diagnosisFailedSentence'))), d.failure.message ? techBlock(h('p', { class: 'selectable small' }, h('code', {}, d.failure.code), ` ${d.failure.message}`)) : null)
        : h('p', {}, t('diag.noAnswerUsedFile')),
      h('p', { class: 'small muted' }, t('diag.localEvidenceRightStill')),
      failed && d.failure?.code && ['auth_failed', 'credential_missing', 'model_unavailable', 'unsupported_mode'].includes(d.failure.code)
        ? h('div', { class: 'card-actions' }, h('button', { type: 'button', onclick: () => this.app.navigate('settings') }, t('diag.checkAiDiagnosisSettings')), this.app.store.state.diagTarget ? retry : null)
        : this.app.store.state.diagTarget
          ? h('div', { class: 'card-actions' }, retry)
          : null,
    );
  }

  private verdict(d: Diagnosis): HTMLElement {
    const r = d.result!;
    const eff = r.effective_recommendation;
    const changed = eff !== r.model_recommendation;
    const expired = d.state === 'expired';
    return h(
      'div',
      { class: 'panel diag-verdict-panel' },
      expired
        ? h(
            'div',
            { class: 'notice warn', role: 'status' },
            h('span', {}, t('diag.evidenceChangedResultExpired')),
            this.app.store.state.diagTarget ? h('button', { type: 'button', id: 'diag-redo', onclick: () => void this.start() }, t('common.diagnoseAgain')) : null,
          )
        : null,
      h(
        'div',
        { class: 'diag-verdict', 'data-rec': eff },
        recMark(eff),
        h('p', { class: 'diag-summary' }, r.answer.summary),
        r.answer.conditions.length ? h('ul', { class: 'diag-conditions', 'aria-label': t('diag.preconditions') }, ...r.answer.conditions.map((c) => h('li', {}, c))) : null,
        h('p', { class: 'small diag-rec-explain' }, expired ? t('diag.verdictPredatesChangeEvidence') : REC_EXPLAIN[eff]),
      ),
      changed
        ? h('p', { class: 'diag-adjusted small' }, t('diag.modelSaidPCleanmanager', { p: REC_LABEL[r.model_recommendation], p2: REC_LABEL[eff] }))
        : null,
      r.conflicts.length
        ? h('ul', { class: 'diag-conflicts', 'aria-label': t('diag.conflictsLocalFacts') }, ...r.conflicts.map((c) => h('li', {}, h('span', { class: 'cmark', 'aria-hidden': 'true' }), c.text)))
        : null,
      r.sanitized.length ? h('p', { class: 'small muted' }, t('diag.lengthRemovedTruncatedCommands', { length: r.sanitized.length })) : null,
    );
  }

  private explanation(d: Diagnosis): HTMLElement {
    const a = d.result!.answer;
    return h(
      'section',
      { class: 'panel', 'aria-labelledby': 'diag-explain' },
      h('h2', { id: 'diag-explain' }, t('diag.purposeConsequences')),
      h(
        'dl',
        { class: 'fact-grid diag-facts' },
        h('dt', {}, t('diag.what')),
        h('dd', {}, a.purpose),
        h('dt', {}, t('diag.whatHappensIfDeleted')),
        h('dd', {}, a.deletion_impact || t('diag.notCoveredAnswer')),
        h('dt', {}, t('diag.rebuildOfflineCost')),
        h('dd', {}, a.rebuild_cost || t('diag.notCoveredAnswer')),
        a.gaps.length ? h('dt', {}, t('diag.uncertaintiesModelMentioned')) : null,
        a.gaps.length ? h('dd', {}, h('ul', { class: 'plain-list' }, ...a.gaps.map((g) => h('li', {}, g)))) : null,
      ),
    );
  }

  private ledger(d: Diagnosis): HTMLElement | null {
    const r = d.result!;
    if (!r.answer.claims.length) return null;
    return h(
      'section',
      { class: 'panel', 'aria-labelledby': 'diag-ledger' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'diag-ledger' }, t('diag.evidenceEachStatement')), h('span', { class: 'small muted' }, t('diag.referencesPointLocalEvidence'))),
      h(
        'ul',
        { class: 'claim-ledger' },
        ...r.answer.claims.map((c, i) => {
          const g = r.grounding.find((x) => x.index === i);
          const status = g?.status ?? 'unsupported';
          return h(
            'li',
            { class: 'claim-row', 'data-status': status },
            h('p', { class: 'claim-text' }, c.text),
            h(
              'div',
              { class: 'claim-meta' },
              pill(CLAIM_LABEL[status], CLAIM_TONE[status]),
              g?.note && g.note !== CLAIM_LABEL[status] ? h('span', { class: 'small muted' }, g.note) : null,
              ...c.refs.map((ref) => {
                const item = d.evidence.items.find((x) => x.evidence_id === ref);
                const gap = d.evidence.gaps.find((x) => x.gap_id === ref);
                const label = item ? LAYER_LABEL[item.layer] : gap ? t('diag.gap') : '';
                return h(
                  'button',
                  { type: 'button', class: 'ref-chip', 'aria-label': t('diag.viewEvidenceRefLabel', { ref, label }), onclick: () => this.focusEvidence(ref) },
                  item ? h('span', { class: 'lmark', 'data-layer': item.layer, 'aria-hidden': 'true' }) : null,
                  `${ref} ${label}`,
                );
              }),
            ),
          );
        }),
      ),
    );
  }

  private focusEvidence(ref: string): void {
    const el = document.getElementById(`ev-${ref}`);
    if (!el) return;
    el.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    el.focus({ preventScroll: true });
    el.classList.remove('flash');
    void el.offsetWidth;
    el.classList.add('flash');
  }

  private questions(d: Diagnosis): HTMLElement | null {
    const qs = d.result!.answer.questions;
    if (!qs.length || d.state !== 'completed') return null;
    return h(
      'section',
      { class: 'panel', 'aria-labelledby': 'diag-q' },
      h('h2', { id: 'diag-q' }, t('diag.answeringQuestionsNarrowsUncertainty')),
      h('p', { class: 'small muted' }, t('diag.answersSentStatementPreviewed')),
      ...qs.map((q) => {
        const id = `diag-other-${q.question_id}`;
        const input = h('input', { type: 'text', id, class: 'diag-other', maxlength: 280, placeholder: t('diag.otherAnswer') }) as HTMLInputElement;
        input.value = this.others.get(q.question_id) ?? '';
        input.addEventListener('input', () => this.others.set(q.question_id, input.value));
        input.addEventListener('keydown', (e) => {
          if (e.key === 'Enter' && input.value.trim()) void this.followUp(q.question_id, input.value);
        });
        return h(
          'fieldset',
          { class: 'diag-qa' },
          h('legend', {}, q.text),
          h(
            'div',
            { class: 'diag-choices' },
            ...q.choices.map((c) => h('button', { type: 'button', class: 'choice', disabled: this.busy, onclick: () => void this.followUp(q.question_id, c) }, c)),
          ),
          h('div', { class: 'diag-other-row' }, h('label', { class: 'sr-only', for: id }, t('diag.otherAnswerText', { text: q.text })), input, h('button', { type: 'button', disabled: this.busy, onclick: () => void this.followUp(q.question_id, input.value) }, t('diag.add'))),
        );
      }),
    );
  }

  private nextSteps(d: Diagnosis): HTMLElement {
    const checklist = [...(d.result?.answer.conditions ?? []), ...d.evidence.gaps.map((g) => g.text)];
    return h(
      'section',
      { class: 'panel', 'aria-labelledby': 'diag-next' },
      h('h2', { id: 'diag-next' }, t('diag.nextSteps')),
      h('p', { class: 'small muted' }, t('diag.cleanmanagerDecidesActionsFacts')),
      h(
        'ul',
        { class: 'next-steps' },
        ...d.handoff.map((x) => {
          const btn = x.available
            ? h('button', { type: 'button', id: `next-${x.action}`, class: x.action === 'review_plan' ? 'primary' : '', disabled: this.busy, onclick: () => void this.handoff(x), 'aria-label': ACTION_LABEL[x.action] }, ACTION_BUTTON[x.action])
            : null;
          const mode =
            x.action === 'review_plan' && x.available
              ? (() => {
                  const sel = h('select', { id: 'next-mode', 'aria-label': t('diag.planAction') }, h('option', { value: 'trash' }, t('diag.moveBin', { bin: words().bin })), h('option', { value: 'quarantine' }, t('diag.moveQuarantine'))) as HTMLSelectElement;
                  sel.value = this.planMode;
                  sel.addEventListener('change', () => (this.planMode = sel.value as 'trash' | 'quarantine'));
                  return sel;
                })()
              : null;
          return h(
            'li',
            { class: 'next-row', 'data-available': x.available ? 'yes' : 'no' },
            h(
              'div',
              { class: 'next-text' },
              h('b', {}, ACTION_LABEL[x.action]),
              x.suggested_by_model ? h('span', { class: 'row-tag' }, t('diag.alsoSuggestedModel')) : null,
              h('span', { class: 'small muted' }, x.available ? x.reason : t('diag.unavailableReason', { reason: x.reason })),
            ),
            h('div', { class: 'next-act' }, mode, btn),
          );
        }),
      ),
      this.showChecklist
        ? h('div', { class: 'gaps' }, h('p', { class: 'gaps-title' }, t('diag.checklist')), h('ul', {}, ...checklist.map((c) => h('li', {}, c))))
        : null,
    );
  }

  private aside(): HTMLElement {
    const d = this.diag;
    const target = this.app.store.state.diagTarget;
    return h(
      'aside',
      { class: 'panel work-aside diag-aside', 'aria-label': t('diag.localEvidenceRecord') },
      d ? this.evidenceList(d) : h('div', {}, h('h2', {}, t('diag.localEvidence')), h('p', { class: 'small muted' }, target ? t('diag.previewGathersLocalEvidence') : '')),
      d ? this.sentInfo(d) : null,
      d ? this.record(d) : null,
      this.historyMini(),
    );
  }

  private evidenceList(d: Diagnosis): HTMLElement {
    return h(
      'div',
      { class: 'aside-section diag-evidence' },
      h('h2', {}, t('diag.localEvidence')),
      h(
        'ul',
        { class: 'ev-list' },
        ...d.evidence.items.map((i) =>
          h(
            'li',
            { id: `ev-${i.evidence_id}`, tabindex: '-1', class: 'ev-item' },
            h('span', { class: 'ev-head' }, h('span', { class: 'lmark', 'data-layer': i.layer, 'aria-hidden': 'true' }), h('span', { class: 'ev-layer' }, LAYER_LABEL[i.layer]), h('code', {}, i.evidence_id)),
            h('span', { class: 'ev-text' }, i.text),
          ),
        ),
      ),
      d.evidence.gaps.length
        ? h(
            'div',
            { class: 'gaps' },
            h('p', { class: 'gaps-title' }, t('diag.stillUnknown')),
            h('ul', {}, ...d.evidence.gaps.map((g) => h('li', { id: `ev-${g.gap_id}`, tabindex: '-1' }, g.text))),
          )
        : null,
    );
  }

  private sentInfo(d: Diagnosis): HTMLElement {
    return h(
      'details',
      { class: 'aside-section' },
      h('summary', {}, t('diag.whatSent')),
      h(
        'dl',
        { class: 'facts-list small' },
        h('dt', {}, t('diag.provider')),
        h('dd', {}, t('diag.labelP', { label: d.destination.label, p: PROVIDER_LABEL[d.destination.kind] })),
        h('dt', {}, t('common.model')),
        h('dd', {}, d.destination.model),
        h('dt', {}, t('diag.destination')),
        h('dd', { class: 'path' }, d.destination.endpoint ?? d.destination.executable ?? ''),
        h('dt', {}, t('diag.scope')),
        h('dd', {}, [d.request.disclosure.full_path ? t('diag.fullPathIncluded') : t('diag.pathRedacted'), d.request.disclosure.child_summary ? t('diag.childSummaryIncluded') : t('diag.noChildren'), d.request.statements.length ? t('diag.statementCount', { length: d.request.statements.length }) : null].filter(Boolean).join(t('common.commaSep'))),
        h('dt', {}, t('diag.payloadDigest')),
        h('dd', {}, h('code', { class: 'selectable' }, `${d.payload_digest.slice(0, 16)}…`)),
        h('dt', {}, t('diag.template')),
        h('dd', {}, d.destination.template),
      ),
    );
  }

  private record(d: Diagnosis): HTMLElement {
    const save = d.retention === 'session' ? h('button', { type: 'button', class: 'quiet', id: 'diag-save', onclick: () => void this.save() }, t('diag.saveDiagnosis')) : null;
    const remove = h('button', { type: 'button', class: 'danger-quiet', id: 'diag-remove', onclick: () => void this.remove() }, t('diag.removeRecord'));
    const exp = h('button', { type: 'button', class: 'quiet', id: 'diag-export', 'aria-expanded': this.exportText ? 'true' : 'false', onclick: () => void this.exportPreview() }, this.exportText ? t('diag.hideExportPreview') : t('diag.exportPreview'));
    return h(
      'div',
      { class: 'aside-section' },
      h('h3', {}, t('diag.record')),
      h(
        'p',
        { class: 'small muted' },
        d.retention === 'saved' ? t('diag.savedComputerUntilRemove') : t('diag.keptSessionOnlyRemoved', { retain_until: formatTime(d.retain_until) }),
        t('diag.removingDiagnosisDoesntAffect'),
      ),
      h('p', { class: 'small' }, t('diag.id'), h('code', { class: 'selectable' }, d.diagnosis_id), t('diag.cli'), h('code', { class: 'selectable' }, `cleanmanager diag show ${d.diagnosis_id}`), t('diag.closeParen')),
      h('div', { class: 'card-actions' }, save, exp, remove),
      this.exportText ? h('pre', { class: 'dp-pre selectable', tabindex: '0', 'aria-label': t('diag.exportContentsNoKeys') }, this.exportText) : null,
    );
  }

  private historyMini(): HTMLElement | null {
    const list = (this.history ?? []).slice(0, 6);
    if (!list.length) return null;
    return h(
      'div',
      { class: 'aside-section' },
      h('h3', {}, t('diag.recentDiagnoses')),
      h('ul', { class: 'diag-history' }, ...list.map((x) => this.historyRow(x, true))),
    );
  }

  private historyRow(x: DiagSummary, compact: boolean): HTMLElement {
    const open = () => this.app.store.set({ diagId: x.diagnosis_id, diagTarget: { scanId: x.object.scan_id, entryId: x.object.entry_id, origin: x.object.origin, name: x.subject_name } as DiagTarget });
    return h(
      'li',
      {},
      h(
        'button',
        { type: 'button', class: compact ? 'recent-item' : 'tl-row', 'aria-current': this.diag?.diagnosis_id === x.diagnosis_id ? 'true' : null, onclick: open },
        compact ? null : h('span', { class: 'tl-time' }, formatTime(x.created_at)),
        h('span', { class: compact ? '' : 'tl-main' }, h('span', { class: 'tl-title' }, x.subject_name), compact ? null : h('span', { class: 'tl-sub' }, t('diag.modelP', { model: x.model, p: x.retention === 'saved' ? t('diag.saved') : t('diag.session') }))),
        h('span', { class: compact ? 'muted' : 'tl-side' }, x.effective_recommendation ? REC_LABEL[x.effective_recommendation] : STATE_LABEL[x.state]),
      ),
    );
  }

  private historyPage(): HTMLElement {
    const list = this.history;
    return h(
      'div',
      { class: 'diag-home' },
      h(
        'div',
        { class: 'panel' },
        h('h2', {}, t('diag.startObject')),
        h('p', { class: 'muted' }, t('diag.chooseAiDiagnosisFiles')),
        this.status && !this.status.configured ? h('div', { class: 'card-actions' }, h('button', { type: 'button', onclick: () => this.app.navigate('settings') }, t('diag.setUpAiDiagnosisButton'))) : null,
      ),
      h(
        'section',
        { class: 'panel list-panel', 'aria-labelledby': 'diag-hist' },
        h('h2', { id: 'diag-hist', class: 'list-title' }, t('diag.diagnosisRecords')),
        !list ? h('p', { class: 'muted', role: 'status' }, t('common.reading')) : list.length ? h('ul', { class: 'timeline' }, ...list.map((x) => this.historyRow(x, false))) : h('p', { class: 'muted small list-empty' }, t('diag.noDiagnosisRecordsYet')),
      ),
      h('p', { class: 'small muted' }, icon('info'), t('diag.diagnosisOnlyExplainsObjects')),
    );
  }
}

