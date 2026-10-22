// User-facing wording for states, kinds and times. One vocabulary across pages.

import type { Entry, ScanState } from './types';
import { words } from './platform';
import { locale, t, type Locale } from '../i18n';

export const STATE_LABEL: Record<ScanState, string> = {
  get created() { return t('format.stateLabel.created'); },
  get running() { return t('format.stateLabel.running'); },
  get stopping() { return t('format.stateLabel.stopping'); },
  get completed() { return t('format.stateLabel.completed'); },
  get partial() { return t('format.stateLabel.partial'); },
  get cancelled() { return t('format.stateLabel.cancelled'); },
  get failed() { return t('format.stateLabel.failed'); },
  get interrupted() { return t('format.stateLabel.interrupted'); },
};

export const STATE_EXPLAIN: Record<ScanState, string> = {
  get created() { return t('format.stateExplain.created'); },
  get running() { return t('format.stateExplain.running'); },
  get stopping() { return t('format.stateExplain.stopping'); },
  get completed() { return t('format.stateExplain.completed'); },
  get partial() { return t('format.stateExplain.partial'); },
  get cancelled() { return t('format.stateExplain.cancelled'); },
  get failed() { return t('format.stateExplain.failed'); },
  get interrupted() { return t('format.stateExplain.interrupted'); },
};

export function isTerminal(s: ScanState): boolean {
  return !['created', 'running', 'stopping'].includes(s);
}

export function stateTone(s: ScanState): 'good' | 'warn' | 'bad' | 'busy' | 'muted' {
  switch (s) {
    case 'completed':
      return 'good';
    case 'partial':
    case 'interrupted':
      return 'warn';
    case 'failed':
      return 'bad';
    case 'cancelled':
      return 'muted';
    default:
      return 'busy';
  }
}

export const COMPLETENESS_LABEL: Record<string, string> = {
  get verified_scope() { return t('format.completenessLabel.verified_scope'); },
  get partial_scope() { return t('format.completenessLabel.partial_scope'); },
};

export function kindLabel(e: Pick<Entry, 'kind' | 'boundary'>): string {
  if (e.boundary) {
    return (
      {
        symlink: t('format.symbolicLinkNotFollowed'),
        junction: t('format.junctionNotFollowed'),
        mount_point: t('format.mountPointNotFollowed'),
        other_volume: t('format.otherVolumeNotFollowed'),
        cloud_provider: t('format.cloudFolderNotDownloaded'),
        own_store: t('format.cleanmanagersOwnDataNot'),
      } as const
    )[e.boundary];
  }
  return ({ file: t('common.file'), directory: t('format.folder'), symlink: t('format.symbolicLink'), reparse: t('format.reparsePoint'), other: t('common.other') } as const)[e.kind];
}

export function basisLabel(e: Pick<Entry, 'size_basis' | 'kind'>): string {
  switch (e.size_basis) {
    case 'entity':
      return t('format.logicalSizeFileItself');
    case 'subtree_known':
      return t('format.totalKnownLogicalSize');
    default:
      return t('common.sizeUnknown');
  }
}

const timeFormats = new Map<Locale, Intl.DateTimeFormat>();

/**
 * RFC 3339 UTC → local date and time, with Intl in the interface language (LIF-05): "2026-10-22 14:05" in 简体中文
 * (the order the pages and the CLI have always used), "Oct 22, 2026, 2:05 PM" in English.
 */
export function formatTime(iso: string | null | undefined): string {
  if (!iso) return '—';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const l = locale();
  let f = timeFormats.get(l);
  if (!f) {
    f =
      l === 'zh-CN'
        ? new Intl.DateTimeFormat(l, { year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' })
        : new Intl.DateTimeFormat(l, { dateStyle: 'medium', timeStyle: 'short' });
    timeFormats.set(l, f);
  }
  if (l !== 'zh-CN') return f.format(d);
  const parts = f.formatToParts(d);
  const part = (type: Intl.DateTimeFormatPartTypes) => parts.find((p) => p.type === type)?.value ?? '';
  return `${part('year')}-${part('month')}-${part('day')} ${part('hour')}:${part('minute')}`;
}

export function formatElapsed(fromIso: string | null | undefined, toIso?: string | null): string {
  if (!fromIso) return '—';
  const a = new Date(fromIso).getTime();
  const b = toIso ? new Date(toIso).getTime() : Date.now();
  if (Number.isNaN(a) || Number.isNaN(b)) return '—';
  const s = Math.max(0, Math.round((b - a) / 1000));
  if (s < 60) return t('common.seconds', { s });
  const m = Math.floor(s / 60);
  if (m < 60) return t('format.mMinP', { m, p: s % 60 });
  return t('format.floorHPMin', { floor: Math.floor(m / 60), p: m % 60 });
}

export const ERROR_HINT: Record<string, string> = {
  get store_inside_scope() { return t('format.errorHint.store_inside_scope'); },
  get root_missing() { return t('format.errorHint.root_missing'); },
  get not_a_directory() { return t('format.errorHint.not_a_directory'); },
  get root_is_link() { return t('format.errorHint.root_is_link'); },
  get permission_required() { return t('format.errorHint.permission_required'); },
  get scan_running() { return t('format.errorHint.scan_running'); },
  get unknown_root() { return t('format.errorHint.unknown_root'); },
  get not_published() { return t('format.errorHint.not_published'); },
  get cursor_stale() { return t('format.errorHint.cursor_stale'); },
  get index_corrupt() { return t('format.errorHint.index_corrupt'); },
  get index_incompatible() { return t('format.errorHint.index_incompatible'); },
  get not_found() { return t('format.errorHint.not_found'); },
  get root_inside_store() { return t('format.errorHint.root_inside_store'); },
  get rules_unavailable() { return t('format.errorHint.rules_unavailable'); },
  get not_selectable() { return t('format.errorHint.not_selectable'); },
  get plan_refused() { return t('format.errorHint.plan_refused'); },
  get needs_approval() { return t('format.errorHint.needs_approval'); },
  get plan_stale() { return t('format.errorHint.plan_stale'); },
  get plan_changed() { return t('format.errorHint.plan_changed'); },
  get review_expired() { return t('format.errorHint.review_expired'); },
  get confirmation_mismatch() { return t('format.errorHint.confirmation_mismatch'); },
  get operation_running() { return t('format.errorHint.operation_running'); },
  get operation_conflict() { return t('format.errorHint.operation_conflict'); },
  get policy_denied() { return t('format.errorHint.policy_denied'); },
  get policy_revoked() { return t('format.errorHint.policy_revoked'); },
  get not_recoverable() { return t('format.errorHint.not_recoverable'); },
  get manifest_incomplete() { return t('format.errorHint.manifest_incomplete'); },
  get protection_unavailable() { return t('format.errorHint.protection_unavailable'); },
  get protection_rejected() { return t('format.errorHint.protection_rejected'); },
  get protection_busy() { return t('format.errorHint.protection_busy'); },
  get not_removable() { return t('format.errorHint.not_removable'); },
  get unsupported() { return t('format.errorHint.unsupported'); },
  get not_directory() { return t('format.errorHint.not_directory'); },
  get not_project_root() { return t('format.errorHint.not_project_root'); },
  get already_configured() { return t('format.errorHint.already_configured'); },
  get purge_settings_invalid() { return t('format.errorHint.purge_settings_invalid'); },
  get io_error() { return t('format.errorHint.io_error'); },
  get wrong_catalog() { return t('format.errorHint.wrong_catalog'); },
  get unknown_candidate() { return t('format.errorHint.unknown_candidate'); },
  get selection_stale() { return t('format.errorHint.selection_stale'); },
  get recycle_bin_unavailable() { return t('format.volumeNoBinAvailable', { bin: words().bin }); },
  get unknown_entry() { return t('format.errorHint.unknown_entry'); },
  get overlap() { return t('format.errorHint.overlap'); },
  get incomplete_subtree() { return t('format.errorHint.incomplete_subtree'); },
  get unknown_item() { return t('format.errorHint.unknown_item'); },
  get needs_cli_confirmation() { return t('format.errorHint.needs_cli_confirmation'); },
  get conflict() { return t('format.errorHint.conflict'); },
  get protected() { return t('format.errorHint.protected'); },
};

/** Short task title: "正在扫描", "正在停止扫描", "扫描已完成"… */
export function taskTitle(s: ScanState): string {
  switch (s) {
    case 'created':
      return t('format.preparingScan');
    case 'running':
      return t('common.scanning');
    case 'stopping':
      return t('common.stoppingScan');
    case 'completed':
      return t('format.taskTitle.completed');
    case 'partial':
      return t('format.taskTitle.partial');
    case 'cancelled':
      return t('format.taskTitle.cancelled');
    case 'failed':
      return t('format.taskTitle.failed');
    case 'interrupted':
      return t('format.taskTitle.interrupted');
  }
}
