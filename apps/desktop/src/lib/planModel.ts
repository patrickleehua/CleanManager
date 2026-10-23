// P05 plan review: what a stored plan will do, said once before the item table — one sentence, the
// counts, and how (or whether) it can be undone — plus which authorization state the plan is in. Pure
// so every mode's sentence and the irreversible marking are tested without a DOM.

import { groupDigits, parseBytes } from './bytes';
import { sizeLabel, type BannerKind } from './display';
import { words } from './platform';
import type { Mode, PlanDetail, PlanItem } from './types';
import { MODE_EXPLAIN, MODE_LABEL, RECOVERY_EXPLAIN, STALE_LABEL } from './words';
import { t, upperFirst } from '../i18n';

type Explain = { effect: string; space: string; recovery: string };

const STARTUP_EXPLAIN = {
  get effect(): string {
    return words().startupEffect;
  },
  get space() { return t('planModel.startupExplain.space'); },
  get recovery() { return t('planModel.startupExplain.recovery'); },
};

/** What plans of the newer kinds do, in the same three lines as the file modes. */
const KIND_EXPLAIN: Partial<Record<PlanDetail['kind'], Explain>> = {
  uninstall: {
    get effect() { return t('planModel.kindExplain.uninstall.effect'); },
    get space() { return t('planModel.kindExplain.uninstall.space'); },
    get recovery() { return t('planModel.kindExplain.uninstall.recovery'); },
  },
  uninstall_leftovers: {
    get effect() { return t('planModel.kindExplain.uninstall_leftovers.effect'); },
    get space() { return t('planModel.kindExplain.uninstall_leftovers.space'); },
    get recovery() { return t('planModel.kindExplain.uninstall_leftovers.recovery'); },
  },
  uninstall_restore: {
    get effect() { return t('planModel.kindExplain.uninstall_restore.effect'); },
    get space() { return t('planModel.kindExplain.uninstall_restore.space'); },
    get recovery() { return t('planModel.kindExplain.uninstall_restore.recovery'); },
  },
  update: {
    get effect() { return t('planModel.kindExplain.update.effect'); },
    get space() { return t('planModel.kindExplain.update.space'); },
    get recovery() { return t('planModel.kindExplain.update.recovery'); },
  },
  process: {
    get effect() { return t('planModel.kindExplain.process.effect'); },
    get space() { return t('planModel.kindExplain.process.space'); },
    get recovery() { return t('planModel.kindExplain.process.recovery'); },
  },
  maintenance: {
    get effect() { return t('planModel.kindExplain.maintenance.effect'); },
    get space() { return t('planModel.kindExplain.maintenance.space'); },
    get recovery() { return t('planModel.kindExplain.maintenance.recovery'); },
  },
};

/** Kinds whose actions are not about space: no size column or figure. */
export const NO_SPACE = new Set<PlanDetail['kind']>(['startup', 'startup_restore', 'uninstall', 'uninstall_restore', 'update', 'process', 'maintenance']);

/** What a plan of this kind does, in one line, and how it reads on the execute button. */
export function kindVerb(d: PlanDetail, items: PlanItem[]): { verb: string; run: string; permanent: boolean } {
  const n = groupDigits(d.item_count ?? 0);
  switch (d.kind) {
    case 'cleanup':
      return {
        verb: t('planModel.pNP2', { p: d.mode ? MODE_LABEL[d.mode] : t('planModel.process'), objects: objectCount(d) }),
        run: t('planModel.runPN', { p: d.mode ? MODE_LABEL[d.mode] : t('planModel.process'), n }),
        permanent: d.mode === 'delete',
      };
    case 'manual_trash':
      return { verb: t('planModel.movetobinN', { moveToBin: words().moveToBin, n }), run: t('planModel.runMovetobinN', { moveToBin: words().moveToBin, n }), permanent: false };
    case 'manual_purge':
      return { verb: t('planModel.deletePermanentlyBinN', { bin: words().bin, n }), run: t('planModel.runDeleteNPermanently', { n }), permanent: true };
    case 'startup': {
      const disable = items.every((i) => i.action_type === 'startup_disable');
      const enable = items.every((i) => i.action_type === 'startup_enable');
      const what = disable ? t('planModel.disable') : enable ? t('planModel.enable') : t('planModel.change');
      const name = d.item_count === 1 && items[0] ? `“${items[0].name}”` : t('planModel.n', { n });
      return { verb: t('planModel.whatName', { what, name }), run: t('planModel.runWhatP', { what, p: d.item_count === 1 ? t('planModel.startupItem') : t('planModel.n2', { n }) }), permanent: false };
    }
    case 'startup_restore':
      return { verb: t('planModel.restoreStartupItemsN', { n }), run: t('planModel.runRestoreN', { n }), permanent: false };
    case 'uninstall': {
      const apps = d.kind_detail?.apps ?? [];
      if (apps.length > 1)
        return { verb: t('planModel.uninstallLengthAskEach', { length: apps.length, uninstallRuns: words().uninstallRuns }), run: t('planModel.runUninstallLength', { length: apps.length }), permanent: false };
      const name = d.kind_detail?.app?.name ?? items[0]?.name ?? t('planModel.app');
      return { verb: t('planModel.uninstallNameAskQuit', { name, uninstallRuns: words().uninstallRuns }), run: t('planModel.runUninstallName', { name }), permanent: false };
    }
    case 'uninstall_leftovers': {
      const name = d.kind_detail?.noted?.app_name;
      return { verb: t('planModel.cleanUpLeftoversP', { p: name ? t('planModel.name', { name }) : '', n }), run: t('planModel.runCleanUpN', { n }), permanent: false };
    }
    case 'uninstall_restore':
      return { verb: t('planModel.restoreRegistryKeysBackup', { n }), run: t('planModel.runRestoreN', { n }), permanent: false };
    case 'update':
      return { verb: t('planModel.updateN', { n }), run: t('planModel.runUpdateN', { n }), permanent: false };
    case 'process': {
      const name = d.kind_detail?.noted?.name ?? items[0]?.name ?? t('planModel.process2');
      return { verb: t('planModel.endProcessP', { p: items[0]?.name ?? name }), run: t('planModel.runEndProcess'), permanent: true };
    }
    case 'maintenance':
      return { verb: t('planModel.runN', { n }), run: t('planModel.runN2', { n }), permanent: false };
    case 'restore':
      return { verb: t('planModel.restoreOriginalLocationN', { n }), run: t('planModel.runRestoreN2', { n }), permanent: false };
    default:
      return { verb: t('planModel.emptyPermanentlyN', { n }), run: t('planModel.runEmptyNPermanently', { n }), permanent: true };
  }
}

/** What the plan does, to space, and how it is undone (null for kinds without the three lines). */
export function planExplain(d: PlanDetail): Explain | null {
  if ((d.kind === 'cleanup' || d.kind === 'manual_trash') && d.mode) return MODE_EXPLAIN[d.mode];
  if (d.kind === 'startup' || d.kind === 'startup_restore') return STARTUP_EXPLAIN;
  if (d.kind === 'manual_purge') return MODE_EXPLAIN.delete;
  if (d.kind === 'restore' || d.kind === 'purge') return RECOVERY_EXPLAIN[d.kind];
  return words().kindExplain[d.kind] ?? KIND_EXPLAIN[d.kind] ?? null;
}

// ---------------------------------------------------------------------------------------------
// Reversibility: a shape and a word for every plan (never colour alone)

export type Undo = 'restorable' | 'recoverable' | 'irreversible' | 'external' | 'none';

export const UNDO_LABEL: Record<Undo, string> = {
  get restorable() { return t('planModel.undoLabel.restorable'); },
  get recoverable() { return t('planModel.undoLabel.recoverable'); },
  get irreversible() { return t('planModel.undoLabel.irreversible'); },
  get external() { return t('planModel.undoLabel.external'); },
  get none() { return t('planModel.undoLabel.none'); },
};

/** The short form beside a mode choice, where the next line already says what happens to space. */
export const UNDO_SHORT: Record<Undo, string> = {
  get restorable() { return UNDO_LABEL.restorable; },
  get recoverable() { return t('planModel.undoShort.recoverable'); },
  get irreversible() { return UNDO_LABEL.irreversible; },
  get external() { return UNDO_LABEL.external; },
  get none() { return UNDO_LABEL.none; },
};

export const MODE_UNDO: Record<Mode, Undo> = { trash: 'restorable', quarantine: 'recoverable', delete: 'irreversible' };

export function planUndo(d: PlanDetail, items: PlanItem[]): Undo {
  if (kindVerb(d, items).permanent) return 'irreversible';
  if ((d.kind === 'cleanup' || d.kind === 'manual_trash') && d.mode) return MODE_UNDO[d.mode];
  switch (d.kind) {
    case 'startup':
    case 'uninstall_leftovers':
      return 'restorable';
    case 'uninstall':
    case 'update':
    case 'maintenance':
      return 'external';
    default:
      return 'none';
  }
}

// ---------------------------------------------------------------------------------------------
// The statement: one sentence + counts + reversibility, before the item table

/** Words of plan review that are not data (one table for the language switch). */
export const PLAN_TEXT = {
  get objects() { return t('planModel.planText.objects'); },
  get knownSize() { return t('planModel.planText.knownSize'); },
  get source() { return t('planModel.planText.source'); },
  get rules() { return t('planModel.planText.rules'); },
  count: (n: number) => t('planModel.planText.count', { n }),
  ruleCount: (n: number) => t('planModel.planText.ruleCount', { n }),
  get notFreed() { return t('planModel.planText.notFreed'); },
} as const;

export interface PlanCount {
  label: string;
  value: string;
  /** The value is a path (shown shortened in the middle). */
  path?: boolean;
}

export interface PlanStatement {
  /** What will happen, as one sentence (no trailing full stop: it is the headline). */
  sentence: string;
  counts: PlanCount[];
  undo: Undo;
  undoLabel: string;
  /** How it is undone, or why it cannot be. */
  undoText: string | null;
  /** What it does to free space. */
  spaceText: string | null;
  /** The plan deletes something for good: the page marks it with a shape, a word and a pattern. */
  irreversible: boolean;
  /** The CSS tone of the statement edge. */
  tone: 'delete' | 'trash' | 'quarantine' | 'review' | 'restore';
}

/** "6 个文件" / "6 files": the count with the word for what the plan holds (a plural form in English). */
function objectCount(d: PlanDetail): string {
  const n = d.item_count ?? 0;
  return d.catalog === 'purge' ? t('planModel.countArtifactFiles', { n }) : d.catalog === 'installer' ? t('planModel.countInstallers', { n }) : t('planModel.countFiles', { n });
}

/** The headline sentence for each kind; kinds without a file mode keep their review line. */
export function planSentence(d: PlanDetail, items: PlanItem[]): string {
  const n = groupDigits(d.item_count ?? 0);
  const w = words();
  if (d.kind === 'cleanup' && d.mode) return d.mode === 'delete' ? t('planModel.deleteNPermanentlyD', { objects: objectCount(d) }) : t('planModel.pND', { objects: objectCount(d), p: MODE_LABEL[d.mode] });
  if (d.kind === 'manual_trash') return t('planModel.movetobinN2', { n, moveToBin: w.moveToBin });
  if (d.kind === 'manual_purge') return t('planModel.deleteNPermanentlyBin', { bin: w.bin, n });
  if (d.kind === 'restore') return t('planModel.restoreNOriginalLocation', { n });
  if (d.kind === 'purge') return t('planModel.emptyNPermanently', { n });
  return kindVerb(d, items).verb;
}

export function planStatement(d: PlanDetail, items: PlanItem[]): PlanStatement {
  const T = PLAN_TEXT;
  const permanent = kindVerb(d, items).permanent;
  const explain = planExplain(d);
  const undo = planUndo(d, items);
  const counts: PlanCount[] = [{ label: T.objects, value: T.count(d.item_count ?? 0) }];
  if (!NO_SPACE.has(d.kind)) {
    let total: bigint | null = null;
    try {
      total = parseBytes(d.total_logical_bytes);
    } catch {
      total = null;
    }
    counts.push({ label: T.knownSize, value: sizeLabel(total) });
  }
  if ((d.kind === 'cleanup' || d.kind === 'manual_trash') && d.scan_root_display) counts.push({ label: T.source, value: d.scan_root_display, path: true });
  if (d.kind === 'cleanup' && d.rule_refs.length) counts.push({ label: T.rules, value: T.ruleCount(d.rule_refs.length) });
  const tone: PlanStatement['tone'] = permanent
    ? 'delete'
    : d.kind === 'uninstall_leftovers'
      ? 'trash'
      : d.kind === 'uninstall' || d.kind === 'update' || d.kind === 'maintenance'
        ? 'review'
        : d.mode === 'trash' || d.mode === 'quarantine'
          ? d.mode
          : 'restore';
  return {
    sentence: planSentence(d, items),
    counts,
    undo,
    undoLabel: UNDO_LABEL[undo],
    // The badge already says 无法撤销; the line under it starts where the badge ends.
    undoText: explain ? (undo === 'irreversible' && explain.recovery.startsWith(UNDO_LABEL.irreversible) ? upperFirst(explain.recovery.slice(UNDO_LABEL.irreversible.length).replace(/^[，：:、；;,.\s]+/, '')) || explain.recovery : explain.recovery) : null,
    spaceText: explain?.space ?? null,
    irreversible: permanent,
    tone,
  };
}

// ---------------------------------------------------------------------------------------------
// Authorization state

export type AuthView = 'executed' | 'gated' | 'terminal' | 'review' | 'authorized' | 'stale' | 'revoked' | 'expired' | 'closed';

/**
 * D15 / WFA-02: the gated capability a stored plan/v2 belongs to. Mirrors `cm_app::features::plan_kind_capability`
 * (the host's `gate::require_plan` refuses approving and running such a plan while the capability is off); cleanup and
 * recovery plans are core. `planModel.test.ts` reads the Rust table, so the two cannot drift apart.
 */
export const PLAN_KIND_CAPABILITY: Readonly<Record<string, string | null>> = {
  manual_selection: 'trash.manual',
  uninstall: 'apps.uninstall',
  maintenance: 'maintenance.run',
  startup: 'startup.toggle',
  update: 'apps.updates',
  process: 'process.end',
  cleanup: null,
  recovery: null,
};

/** The capability a plan needs, or null for a core plan (v1 cleanup/recovery plans have no plan/v2 kind). */
export function planCapability(d: Pick<PlanDetail, 'plan_kind_v2'>): string | null {
  return d.plan_kind_v2 ? (PLAN_KIND_CAPABILITY[d.plan_kind_v2] ?? null) : null;
}

export const AUTH_TEXT = {
  get gated() { return t('planModel.authText.gated'); },
  get executed() { return t('planModel.authText.executed'); },
  get terminal() { return t('planModel.authText.terminal'); },
  get authorized() { return t('planModel.authText.authorized'); },
  get revoked() { return t('planModel.authText.revoked'); },
  get expired() { return t('planModel.authText.expired'); },
  get closed() { return t('planModel.authText.closed'); },
} as const;

/**
 * The authorization panel's state. `gateOff`: the plan's capability is off here (D15), so it can be neither approved
 * nor run — the panel says so and links to the setting instead of offering controls the host would refuse. A plan that
 * already ran keeps its result, and a stale plan stays stale (turning the feature on would not make it usable).
 */
export function authView(d: Pick<PlanDetail, 'operation' | 'authorization' | 'terminal_only' | 'review_token' | 'stale_reason'>, gateOff = false): { view: AuthView; line: { kind: BannerKind; text: string } | null } {
  const a = d.authorization.status;
  if (d.operation) return { view: 'executed', line: { kind: 'success', text: AUTH_TEXT.executed } };
  if (d.stale_reason) return { view: 'stale', line: { kind: 'failed', text: STALE_LABEL[d.stale_reason] ?? AUTH_TEXT.closed } };
  if (gateOff && (a === 'required' || a === 'authorized')) return { view: 'gated', line: { kind: 'cancelled', text: AUTH_TEXT.gated } };
  if (a === 'required' && d.terminal_only) return { view: 'terminal', line: { kind: 'pending-permission', text: AUTH_TEXT.terminal } };
  if (a === 'required' && d.review_token) return { view: 'review', line: null };
  if (a === 'authorized') return { view: 'authorized', line: { kind: 'success', text: AUTH_TEXT.authorized } };
  if (a === 'revoked') return { view: 'revoked', line: { kind: 'cancelled', text: AUTH_TEXT.revoked } };
  if (a === 'expired') return { view: 'expired', line: { kind: 'cancelled', text: AUTH_TEXT.expired } };
  return { view: 'closed', line: { kind: 'cancelled', text: AUTH_TEXT.closed } };
}
