// Shared selection → review → result components (GUI-09/10/13/14, task 16.3). Cleanup, build
// artifacts, installers, uninstall leftovers, maintenance tasks, updates and old downloads all use
// the same pieces: three-tier badges, the bottom selection bar with "at least X" wording, the plan
// draft with processing modes, long-operation progress, permission readiness, flow steps, the six
// maintenance outcomes and capability pages.

import type { App } from '../main';
import { toIpcError } from '../lib/api';
import { formatBytes, groupDigits, parseBytes } from '../lib/bytes';
import { h, icon, live } from '../lib/dom';
import { words } from '../lib/platform';
import type { IpcError, Mode, Readiness, ReadinessCheck, ReadinessOp, Tier } from '../lib/types';
import { MODE_EXPLAIN, MODE_LABEL } from '../lib/words';
import { MODE_UNDO, UNDO_LABEL, UNDO_SHORT, type Undo } from '../lib/planModel';
import { copyField, errorLine, errorText, pill, type Tone } from './parts';
import { learnMore } from './ui';
import { t } from '../i18n';

type Child = Node | string | null | undefined | false;

// ---------------------------------------------------------------------------------------------
// Three-tier badge

export const TIER_LABEL: Record<Tier, string> = {
  get default_selected() { return t('kit.tierLabel.default_selected'); },
  get review_only() { return t('kit.tierLabel.review_only'); },
  get blocked() { return t('kit.tierLabel.blocked'); },
};

const TIER_SHAPE: Record<Tier, string> = { default_selected: 'low', review_only: 'review', blocked: 'blocked' };

export const TIER_EXPLAIN: Record<Tier, string> = {
  get default_selected() { return t('kit.tierExplain.default_selected'); },
  get review_only() { return t('kit.tierExplain.review_only'); },
  get blocked() { return t('kit.tierExplain.blocked'); },
};

/** Shape + word for a rule tier (filled circle, half circle, struck square). */
export function tierBadge(tier: Tier): HTMLElement {
  return h('span', { class: 'gmark tier', 'data-group': TIER_SHAPE[tier], title: TIER_EXPLAIN[tier] }, h('span', { class: 'gmark-shape', 'aria-hidden': 'true' }), h('span', {}, TIER_LABEL[tier]));
}

// ---------------------------------------------------------------------------------------------
// Sizes: never a fake exact total

export interface SizeSum {
  known: bigint;
  /** Objects (or files inside them) whose size is unknown. */
  unknown: number;
}

export function sumSizes(items: { bytes: string | null; unknown?: number }[]): SizeSum {
  let known = 0n;
  let unknown = 0;
  for (const i of items) {
    const v = parseBytes(i.bytes);
    if (v === null) unknown += Math.max(1, i.unknown ?? 0);
    else {
      known += v;
      unknown += i.unknown ?? 0;
    }
  }
  return { known, unknown };
}

/** "1.2 GiB", "至少 1.2 GiB", or "大小未知" — never an exact-looking total over unknown parts. */
export function atLeast(sum: SizeSum): string {
  if (sum.unknown > 0 && sum.known === 0n) return t('common.sizeUnknown');
  if (sum.unknown > 0) return t('kit.leastKnown', { known: formatBytes(sum.known) });
  return formatBytes(sum.known);
}

export function unknownNote(sum: SizeSum): string | null {
  return sum.unknown > 0 ? t('kit.unknown', { unknown: sum.unknown }) : null;
}

// ---------------------------------------------------------------------------------------------
// Bottom selection bar

export interface SelectionBarOpts {
  count: number;
  /** Known size of the selection; `null` for selections that are not about space (updates, tasks). */
  sum: SizeSum | null;
  /** Selected objects come from this many roots / sources (one plan each). */
  sources?: number;
  action: { text: string; disabled: boolean; onClick: () => void; id?: string; danger?: boolean };
  onClear: () => void;
  note?: string | null;
}

/**
 * The same bar on every selection page: count, known size ("at least" when part is unknown), the
 * unknown count on its own, and one action that leads into plan review. Hidden when empty.
 */
export function selectionBar(o: SelectionBarOpts): HTMLElement {
  if (o.count === 0) return h('div', { class: 'selection-bar', hidden: true });
  const unknown = o.sum ? unknownNote(o.sum) : null;
  const go = h('button', { type: 'button', class: o.action.danger ? 'danger' : 'primary', id: o.action.id ?? 'selbar-go', disabled: o.action.disabled }, o.action.text);
  go.addEventListener('click', o.action.onClick);
  return h(
    'div',
    { class: 'selection-bar', role: 'region', 'aria-label': t('kit.selectionBar') },
    h(
      'p',
      { class: 'selbar-sum', 'aria-live': 'polite' },
      h('b', { class: 'num' }, t('kit.countSelected', { count: o.count })),
      o.sum ? h('span', { class: 'num' }, atLeast(o.sum)) : null,
      unknown ? h('span', { class: 'selbar-unknown' }, unknown) : null,
      o.sources && o.sources > 1 ? h('span', { class: 'muted' }, t('kit.sourcesOnePlanPer', { sources: o.sources })) : null,
      o.note ? h('span', { class: 'muted' }, o.note) : null,
    ),
    h('div', { class: 'selbar-actions' }, h('button', { type: 'button', class: 'quiet', onclick: o.onClear }, t('kit.clearSelection')), go),
  );
}

// ---------------------------------------------------------------------------------------------
// Processing mode choice (the plan draft)

/**
 * How reversible something is: a shape and a word, never colour alone. "无法撤销" also carries a
 * hatched ground, so a permanent action reads differently even in grey.
 */
export function undoBadge(undo: Undo, label = UNDO_LABEL[undo]): HTMLElement {
  const mark = undo === 'irreversible' ? icon('warn') : undo === 'restorable' || undo === 'recoverable' ? icon('ext-recovery') : undo === 'external' ? icon('lock') : icon('check');
  return h('span', { class: 'undo-badge', 'data-undo': undo }, h('span', { class: 'undo-mark', 'aria-hidden': 'true' }, mark), h('span', {}, label));
}

/** Words of the mode choice (one table for the language switch). */
export const MODE_CHOICE_TEXT = {
  get legend() { return t('kit.legend'); },
  get notAllowed() { return t('kit.notAllowed'); },
} as const;

export function modeOptions(opts: { name: string; allowed: Mode[]; current: Mode | null; selected: number; onChange: (m: Mode) => void; modes?: Mode[]; id?: (m: Mode) => string }): HTMLElement {
  const modes = opts.modes ?? (['trash', 'quarantine', 'delete'] as Mode[]);
  return h(
    'fieldset',
    { class: 'mode-options', id: `${opts.name}-modes`, disabled: opts.selected === 0 },
    h('legend', {}, MODE_CHOICE_TEXT.legend),
    ...modes.map((m) => {
      const allowed = opts.allowed.includes(m);
      const id = opts.id ? opts.id(m) : `${opts.name}-${m}`;
      const input = h('input', { type: 'radio', name: opts.name, id, value: m, disabled: !allowed, 'aria-describedby': `${id}-desc` }) as HTMLInputElement;
      input.checked = opts.current === m;
      input.addEventListener('change', () => opts.onChange(m));
      return h(
        'label',
        { class: `mode-option${input.checked ? ' checked' : ''}${allowed ? '' : ' disabled'}`, for: id, 'data-mode': m },
        input,
        h(
          'span',
          { class: 'mode-text', id: `${id}-desc` },
          h('span', { class: 'mode-head' }, h('b', {}, MODE_LABEL[m]), undoBadge(MODE_UNDO[m], UNDO_SHORT[MODE_UNDO[m]])),
          h('span', {}, MODE_EXPLAIN[m].space),
          h('span', { class: 'mode-recovery' }, MODE_EXPLAIN[m].recovery),
          opts.selected > 0 && !allowed ? h('span', { class: 'why-not' }, MODE_CHOICE_TEXT.notAllowed) : null,
        ),
      );
    }),
  );
}

// ---------------------------------------------------------------------------------------------
// Summary strip: the counts of each risk group before the groups themselves (GUI-09 summary → groups)

export interface SummaryCell {
  /** The group's shape + word (tierBadge / groupMark). */
  mark: HTMLElement;
  count: number;
  /** Known size ("至少 X", "未知"); omitted for groups whose size is not the point. */
  size?: string | null;
  /** One short note under the count ("逐项确认", "只能查看"). */
  note?: string | null;
  /** Jump to (and open) the group below. */
  onJump?: () => void;
  id?: string;
}

/** One row of group counts; each cell jumps to its group. Zero groups stay visible but quiet. */
export function summaryStrip(label: string, cells: SummaryCell[]): HTMLElement {
  return h(
    'ul',
    { class: 'summary-strip', 'aria-label': label },
    ...cells.map((c) => {
      const body = [c.mark, h('span', { class: 'ss-count num' }, t('common.itemsCount', { count: c.count })), c.size ? h('span', { class: 'ss-size num' }, c.size) : null, c.note ? h('span', { class: 'ss-note' }, c.note) : null];
      return h(
        'li',
        { 'data-zero': c.count === 0 ? 'yes' : 'no' },
        c.onJump && c.count > 0 ? h('button', { type: 'button', class: 'ss-cell', id: c.id ?? null, onclick: c.onJump }, ...body) : h('span', { class: 'ss-cell' }, ...body),
      );
    }),
  );
}

/** Modes every selected object allows. */
export function commonModes(items: { allowed_modes: Mode[] }[]): Mode[] {
  if (!items.length) return [];
  return (['trash', 'quarantine', 'delete'] as Mode[]).filter((m) => items.every((i) => i.allowed_modes.includes(m)));
}

// ---------------------------------------------------------------------------------------------
// Long operations (GUI-14): stage + elapsed time; no percentage while the denominator is unknown

export class LongProgress {
  el: HTMLElement;
  private started = 0;
  private timer: number | null = null;
  private stageEl: HTMLElement;
  private elapsedEl: HTMLElement;

  constructor(label: () => string) {
    this.stageEl = h('span', { class: 'lp-stage' });
    this.elapsedEl = h('span', { class: 'lp-elapsed num' });
    this.el = h(
      'div',
      { class: 'long-progress', hidden: true },
      live(h('div', { class: 'indeterminate', role: 'progressbar' }), null, { 'aria-label': label, 'aria-valuetext': () => t('kit.progressTotalUnknown') }),
      h('p', { class: 'lp-line', role: 'status', 'aria-live': 'polite' }, this.stageEl, this.elapsedEl),
    );
  }

  start(stage: string): void {
    this.started = Date.now();
    this.el.hidden = false;
    this.stage(stage);
    this.tick();
    if (this.timer === null) this.timer = window.setInterval(() => this.tick(), 1000);
  }

  stage(text: string): void {
    this.stageEl.textContent = text;
  }

  stop(): void {
    this.el.hidden = true;
    if (this.timer !== null) window.clearInterval(this.timer);
    this.timer = null;
  }

  get running(): boolean {
    return this.timer !== null;
  }

  private tick(): void {
    const s = Math.floor((Date.now() - this.started) / 1000);
    this.elapsedEl.textContent = s >= 1 ? t('kit.elapsedP', { p: s >= 60 ? t('kit.floorMinP', { floor: Math.floor(s / 60), p: s % 60 }) : t('common.seconds', { s }) }) : '';
  }
}

// ---------------------------------------------------------------------------------------------
// Permission readiness (GUI-13): explain gaps once before a long operation, then allow continuing

interface CheckWords {
  title: string;
  missing: (op: ReadinessOp) => string;
  fix: string;
}

export function readinessWords(forOp?: ReadinessOp): Record<string, CheckWords> {
  const w = words();
  return {
    store: {
      title: t('kit.cleanmanagerDataFolder'),
      missing: () => t('kit.dataFolderUnavailableSo'),
      fix: t('kit.seeReasonSettingsCapabilities'),
    },
    protection_list: {
      title: t('common.protectionList'),
      missing: () => t('kit.protectionListCantRead'),
      fix: t('kit.seeReasonSettingsProtection'),
    },
    administrator: {
      title: w.admin,
      missing: (op) =>
        op === 'scan' || op === 'purge' || op === 'installer'
          ? t('kit.noAdministratorRightsSystem')
          : op === 'startup'
            ? t('kit.noAdministratorRightsStartup')
            : op === 'maintenance'
              ? w.maintAdminMissing
              : op === 'uninstall'
                ? w.uninstallAdminMissing
                : op === 'process'
                  ? t('kit.noAdministratorRightsProcesses')
                  : t('kit.noAdministratorRightsItems'),
      fix: forOp === 'maintenance' ? w.maintAdminFix : t('kit.cleanmanagerNeverElevatesOwn', { admin: w.admin }),
    },
    bypass_privileges: {
      title: t('kit.backupRestorePrivileges'),
      missing: () => t('kit.couldntDropProcesssBackup'),
      fix: t('kit.usuallyNothingSeeSettings'),
    },
    recycle_bin: {
      title: w.bin,
      missing: () => t('kit.volumeNoUsableBin', { bin: w.bin, moveToBin: w.moveToBin }),
      fix: t('kit.useMoveQuarantineSame', { bin: w.bin }),
    },
    full_disk_access: {
      title: t('kit.fullDiskAccess'),
      missing: () => t('kit.fullDiskAccessMissing'),
      fix: t('kit.allowCleanmanagerSystemSettings'),
    },
  };
}

const asked = new Map<string, string>();

/**
 * Check permissions before a long operation. Resolves `true` to continue (all ready, or the person
 * chose to continue with current access), `false` when cancelled or blocked. Explains each gap once
 * per session for the same operation and the same gaps — the task itself never prompts later.
 */
export async function ensureReady(app: App, op: ReadinessOp, label: string): Promise<boolean> {
  let r: Readiness;
  try {
    r = await app.backend.readiness(op);
  } catch {
    return true; // readiness is advisory; the core still refuses anything unsafe
  }
  const gaps = r.checks.filter((c) => c.state === 'missing' || (c.state === 'unknown' && c.blocking));
  if (!gaps.length) return true;
  const key = gaps.map((g) => g.check_id).sort().join(',');
  const blocking = gaps.some((g) => g.blocking);
  if (!blocking && asked.get(op) === key) return true;
  const ok = await readinessDialog(op, label, gaps, blocking);
  if (ok) asked.set(op, key);
  return ok;
}

function readinessDialog(op: ReadinessOp, label: string, gaps: ReadinessCheck[], blocking: boolean): Promise<boolean> {
  return new Promise((resolve) => {
    const table = readinessWords(op);
    const dlg = h('dialog', { class: 'cm-dialog readiness', 'aria-labelledby': 'ready-title', 'aria-describedby': 'ready-lede' }) as HTMLDialogElement;
    const finish = (v: boolean) => {
      dlg.close();
      dlg.remove();
      resolve(v);
    };
    const go = h('button', { type: 'button', class: 'primary', id: 'ready-go', disabled: blocking }, t('kit.continueCurrentPermissions'));
    go.addEventListener('click', () => finish(true));
    const cancel = h('button', { type: 'button', id: 'ready-cancel' }, t('common.cancel'));
    cancel.addEventListener('click', () => finish(false));
    dlg.addEventListener('cancel', (e) => {
      e.preventDefault();
      finish(false);
    });
    dlg.append(
      h('div', { class: 'dlg-head' }, h('h2', { id: 'ready-title' }, t('kit.beforeStartLabel', { label })), h('p', { id: 'ready-lede', class: 'muted' }, blocking ? t('kit.cantStartUntilProblems') : t('kit.continueCurrentPermissionsEffects'))),
      h(
        'ul',
        { class: 'ready-list' },
        ...gaps.map((g) => {
          const row = table[g.check_id];
          return h(
            'li',
            { 'data-blocking': g.blocking ? 'yes' : 'no' },
            h('span', { class: 'ready-mark', 'aria-hidden': 'true' }, icon(g.blocking ? 'lock' : 'warn')),
            h('span', { class: 'ready-text' }, h('b', {}, row?.title ?? g.check_id), h('span', {}, row ? row.missing(op) : (g.detail ?? t('kit.stateUnknown'))), row ? h('span', { class: 'muted small' }, row.fix) : null),
          );
        }),
      ),
      h('div', { class: 'dlg-actions' }, cancel, go),
    );
    document.body.append(dlg);
    dlg.showModal();
    requestAnimationFrame(() => (blocking ? cancel : go).focus());
  });
}

// ---------------------------------------------------------------------------------------------
// Capability pages (GUI-08): a feature that is not here yet keeps its place and explains itself

export type GateState = 'in_progress' | 'not_applicable' | 'experimental' | 'cli_only' | 'off';

export const GATE_LABEL: Record<GateState, string> = {
  get in_progress() { return t('kit.gateLabel.in_progress'); },
  get not_applicable() { return t('kit.gateLabel.not_applicable'); },
  get experimental() { return t('kit.gateLabel.experimental'); },
  get cli_only() { return t('kit.gateLabel.cli_only'); },
  get off() { return t('kit.gateLabel.off'); },
};

const GATE_TONE: Record<GateState, Tone> = { in_progress: 'busy', not_applicable: 'muted', experimental: 'warn', cli_only: 'good', off: 'muted' };

export interface CapabilityOpts {
  id: string;
  title: string;
  gate: GateState;
  /** What the feature will do, in the user's words. */
  does: string[];
  /** Why it is not available here right now: one short sentence. */
  why: string;
  /** Longer explanation, shown under "了解更多". */
  whyMore?: string[];
  /** Requirement ids and the release gate it waits on. */
  spec: string;
  gateName: string;
  /** What the safety model guarantees once it ships. */
  safety?: string[];
  cli?: { label: string; command: string }[];
  actions?: Child[];
}

export function capabilityPage(o: CapabilityOpts): HTMLElement {
  return h(
    'div',
    { class: 'capability' },
    h(
      'div',
      { class: 'page-head' },
      h('div', { class: 'page-head-text' }, h('h1', { id: `${o.id}-title` }, o.title), h('p', { class: 'meta-row' }, h('span', {}, pill(GATE_LABEL[o.gate], GATE_TONE[o.gate])))),
    ),
    h(
      'div',
      { class: 'cap-grid' },
      h('section', { class: 'panel cap-does', 'aria-labelledby': `${o.id}-does` }, h('h2', { id: `${o.id}-does` }, t('kit.what')), h('ul', { class: 'cap-list-plain' }, ...o.does.map((d) => h('li', {}, d)))),
      h(
        'section',
        { class: 'panel cap-why', 'aria-labelledby': `${o.id}-why` },
        h('h2', { id: `${o.id}-why` }, o.gate === 'not_applicable' ? t('kit.whyIsntOfferedHere') : t('kit.currentStatus')),
        h('p', {}, o.why),
        learnMore(...(o.whyMore ?? []).map((t) => h('p', {}, t)), h('p', { class: 'small' }, t('kit.specSpecReleaseGate', { spec: o.spec, gateName: o.gateName }))),
        o.safety?.length ? h('h3', {}, t('kit.stillAppliesOnceReleased')) : null,
        o.safety?.length ? h('ul', { class: 'cap-list-plain small' }, ...o.safety.map((s) => h('li', {}, s))) : null,
        o.cli?.length ? h('h3', {}, o.gate === 'off' ? t('kit.turnTerminal') : t('kit.availableTerminalNow')) : null,
        ...(o.cli ?? []).map((c) => copyField(c.label, c.command)),
        o.actions?.length ? h('div', { class: 'card-actions' }, ...o.actions) : null,
      ),
    ),
  );
}

export interface GateOffOpts {
  /** The page id; the explanatory page uses `${id}-title`. */
  id: string;
  title: string;
  capability: string;
  gate: string;
  does: string[];
  /** Opens 设置 › 实验功能. */
  openSettings: () => void;
}

/** D15: a section whose capability has not passed its release gate is off here; it keeps its place and says why. */
export function gateOffPage(o: GateOffOpts): HTMLElement {
  return capabilityPage({
    id: o.id,
    title: o.title,
    gate: 'off',
    does: o.does,
    why: t('kit.featureHasntPassedRelease'),
    whyMore: [
      t('kit.releaseGateRequiresEvidence'),
      t('kit.evaluateComputerOnlyTurn'),
    ],
    spec: o.capability,
    gateName: o.gate,
    cli: [{ label: t('kit.turnExperimentalFeaturesShared'), command: 'cleanmanager experimental --state on' }],
    actions: [h('button', { type: 'button', class: 'primary', 'data-open-experimental': '', onclick: o.openSettings }, t('kit.openExperimentalFeaturesSettings'))],
  });
}

// ---------------------------------------------------------------------------------------------
// Steps of a flow that really is a sequence (an uninstall: the uninstaller, then its leftovers)

export interface Step {
  title: string;
  note: string;
}

/** Numbered steps; `current` is highlighted, earlier ones read as done. */
export function stageSteps(label: string, steps: Step[], current: number): HTMLElement {
  return h(
    'ol',
    { class: 'stage-steps', 'aria-label': label },
    ...steps.map((s, i) =>
      h(
        'li',
        { 'data-state': i < current ? 'done' : i === current ? 'current' : 'next', 'aria-current': i === current ? 'step' : null },
        h('span', { class: 'step-num', 'aria-hidden': 'true' }, String(i + 1)),
        h('span', { class: 'step-text' }, h('b', {}, s.title), h('span', {}, s.note)),
      ),
    ),
  );
}

// ---------------------------------------------------------------------------------------------
// Six maintenance outcomes (MNT-03): every outcome has its own shape and word

export type Outcome = 'applied' | 'unchanged' | 'attention' | 'skipped' | 'unavailable' | 'failed';

export const OUTCOME_LABEL: Record<Outcome, string> = {
  get applied() { return t('kit.outcomeLabel.applied'); },
  get unchanged() { return t('kit.outcomeLabel.unchanged'); },
  get attention() { return t('kit.outcomeLabel.attention'); },
  get skipped() { return t('kit.outcomeLabel.skipped'); },
  get unavailable() { return t('kit.outcomeLabel.unavailable'); },
  get failed() { return t('kit.outcomeLabel.failed'); },
};

export function outcomeMark(o: Outcome, withLabel = true): HTMLElement {
  return h('span', { class: 'omark', 'data-outcome': o }, h('span', { class: 'omark-shape', 'aria-hidden': 'true' }), withLabel ? h('span', {}, OUTCOME_LABEL[o]) : h('span', { class: 'sr-only' }, OUTCOME_LABEL[o]));
}

/** All six counts in a fixed order; zero counts stay visible but quiet, so nothing reads as "all done" by omission. */
export function outcomeTally(counts: Partial<Record<Outcome, number>>, label = t('kit.results')): HTMLElement {
  const order: Outcome[] = ['applied', 'unchanged', 'attention', 'skipped', 'unavailable', 'failed'];
  return h(
    'ul',
    { class: 'outcome-tally', 'aria-label': label },
    ...order.map((o) => h('li', { 'data-zero': (counts[o] ?? 0) === 0 ? 'yes' : 'no' }, outcomeMark(o), h('b', { class: 'num' }, groupDigits(counts[o] ?? 0)))),
  );
}

// ---------------------------------------------------------------------------------------------
// Misc

/** A notice line with an optional close button. */
export function noticeLine(text: string, tone: 'info' | 'warn' = 'info', onClose?: () => void): HTMLElement {
  return h(
    'div',
    { class: `notice${tone === 'warn' ? ' warn' : ''}`, role: 'status' },
    h('span', {}, text),
    onClose ? h('button', { type: 'button', class: 'quiet', onclick: onClose }, t('common.got')) : null,
  );
}

export function errorBlock(e: IpcError | null): HTMLElement | null {
  return errorLine(e);
}

export { errorText, toIpcError };
