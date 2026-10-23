// One vocabulary for cleanup, plans, execution and recovery. Every risk, state and outcome has
// words (never colour alone); the same action keeps the same name through the whole flow.
// The words live in the language resources (src/i18n, LIF-05); the bin and file-manager words come from the platform
// table (GUI-12), while the keys (`trash`, `system_recycle_bin`) mean the same on every platform and in every language.

import { words } from './platform';
import type { Availability, AuthStatus, Counts, Group, ItemStatus, Mode, NotSelectable, PlanKind, PolicyState } from './types';
import { t } from '../i18n';

export const MODE_LABEL: Record<Mode, string> = {
  get trash() { return words().moveToBin; },
  get quarantine() { return t('words.modeLabel.quarantine'); },
  get delete() { return t('words.modeLabel.delete'); },
};

/** What each mode does to the object and to free space, and how it can be undone. */
export const MODE_EXPLAIN: Record<Mode, { effect: string; space: string; recovery: string }> = {
  trash: {
    get effect() { return t('words.sendsSystemBin', { bin: words().bin }); },
    get space() { return t('words.itemsBinStillTake', { bin: words().bin }); },
    get recovery() { return t('words.restorefrombinCleanmanagerCantRestore', { restoreFromBin: words().restoreFromBin }); },
  },
  quarantine: {
    get effect() { return t('words.modeExplain.quarantine.effect'); },
    get space() { return t('words.modeExplain.quarantine.space'); },
    get recovery() { return t('words.modeExplain.quarantine.recovery'); },
  },
  delete: {
    get effect() { return t('words.modeExplain.delete.effect'); },
    get space() { return t('words.modeExplain.delete.space'); },
    get recovery() { return t('words.cantUndoneTheresNo', { bin: words().bin }); },
  },
};

export const PLAN_KIND_LABEL: Record<PlanKind, string> = {
  get cleanup() { return t('words.planKindLabel.cleanup'); },
  get restore() { return t('words.planKindLabel.restore'); },
  get purge() { return t('words.planKindLabel.purge'); },
  get manual_trash() { return t('words.handPickedBinPlan', { bin: words().bin }); },
  get manual_purge() { return t('words.permanentBinDeletionPlan', { bin: words().bin }); },
  get startup() { return t('words.planKindLabel.startup'); },
  get startup_restore() { return t('words.planKindLabel.startup_restore'); },
  get uninstall() { return t('words.planKindLabel.uninstall'); },
  get uninstall_leftovers() { return t('words.planKindLabel.uninstall_leftovers'); },
  get uninstall_restore() { return t('words.planKindLabel.uninstall_restore'); },
  get update() { return t('words.planKindLabel.update'); },
  get process() { return t('words.planKindLabel.process'); },
  get maintenance() { return t('words.planKindLabel.maintenance'); },
};

export const RECOVERY_EXPLAIN: Record<'restore' | 'purge', { effect: string; space: string; recovery: string }> = {
  restore: {
    get effect() { return t('words.recoveryExplain.restore.effect'); },
    get space() { return t('words.recoveryExplain.restore.space'); },
    get recovery() { return t('words.recoveryExplain.restore.recovery'); },
  },
  purge: {
    get effect() { return t('words.recoveryExplain.purge.effect'); },
    get space() { return t('words.recoveryExplain.purge.space'); },
    get recovery() { return t('words.recoveryExplain.purge.recovery'); },
  },
};

export const GROUP_LABEL: Record<Group, string> = {
  get low() { return t('words.groupLabel.low'); },
  get review() { return t('words.groupLabel.review'); },
  get pending() { return t('words.groupLabel.pending'); },
  get blocked() { return t('words.groupLabel.blocked'); },
};

export const GROUP_EXPLAIN: Record<Group, string> = {
  get low() { return t('words.groupExplain.low'); },
  get review() { return t('words.groupExplain.review'); },
  get pending() { return t('words.groupExplain.pending'); },
  get blocked() { return t('words.groupExplain.blocked'); },
};

export const NOT_SELECTABLE_LABEL: Record<NotSelectable, string> = {
  get blocked() { return t('words.notSelectableLabel.blocked'); },
  get protected_reviewable() { return t('words.notSelectableLabel.protected_reviewable'); },
  get pending_evidence() { return t('words.notSelectableLabel.pending_evidence'); },
  get no_mode() { return t('words.notSelectableLabel.no_mode'); },
  get unknown_size() { return t('words.notSelectableLabel.unknown_size'); },
  get hint() { return t('words.notSelectableLabel.hint'); },
  get adapter() { return t('words.notSelectableLabel.adapter'); },
  get in_use() { return t('words.notSelectableLabel.in_use'); },
  get probe_unknown() { return t('words.notSelectableLabel.probe_unknown'); },
  get needs_reanalysis() { return t('words.notSelectableLabel.needs_reanalysis'); },
};

export const REGISTRY_LABEL: Record<string, string> = {
  get covered() { return t('words.registryLabel.covered'); },
  get version_untested() { return t('words.registryLabel.version_untested'); },
  get not_registered() { return t('words.registryLabel.not_registered'); },
  get not_checked() { return t('words.registryLabel.not_checked'); },
};

export const PROBE_STATE_LABEL: Record<string, string> = {
  get in_use() { return t('words.probeStateLabel.in_use'); },
  get not_in_use() { return t('words.probeStateLabel.not_in_use'); },
  get unknown() { return t('words.probeStateLabel.unknown'); },
};

export const PROBE_LABEL: Record<string, string> = {
  get owner_process() { return t('words.probeLabel.owner_process'); },
  get open_handles() { return t('words.probeLabel.open_handles'); },
};

export const AUTH_LABEL: Record<AuthStatus, string> = {
  get authorized() { return t('words.authLabel.authorized'); },
  get required() { return t('words.authLabel.required'); },
  get expired() { return t('words.authLabel.expired'); },
  get revoked() { return t('words.authLabel.revoked'); },
};

export const STALE_LABEL: Record<string, string> = {
  get expired() { return t('words.staleLabel.expired'); },
  get rule_set_changed() { return t('words.staleLabel.rule_set_changed'); },
  get digest_mismatch() { return t('words.staleLabel.digest_mismatch'); },
  get rules_unavailable() { return t('words.staleLabel.rules_unavailable'); },
};

export const OPERATION_STATE_LABEL: Record<string, string> = {
  get created() { return t('words.operationStateLabel.created'); },
  get validating() { return t('words.operationStateLabel.validating'); },
  get running() { return t('words.operationStateLabel.running'); },
  get stopping() { return t('words.operationStateLabel.stopping'); },
  get completed() { return t('words.operationStateLabel.completed'); },
  get partial() { return t('words.operationStateLabel.partial'); },
  get cancelled() { return t('words.operationStateLabel.cancelled'); },
  get failed() { return t('words.operationStateLabel.failed'); },
  get reconciliation_required() { return t('words.operationStateLabel.reconciliation_required'); },
};

export function operationTone(state: string): 'good' | 'warn' | 'bad' | 'busy' | 'muted' {
  switch (state) {
    case 'completed':
      return 'good';
    case 'partial':
    case 'reconciliation_required':
      return 'warn';
    case 'failed':
      return 'bad';
    case 'cancelled':
      return 'muted';
    default:
      return 'busy';
  }
}

export const ITEM_STATUS_LABEL: Record<ItemStatus, string> = {
  get succeeded() { return t('words.itemStatusLabel.succeeded'); },
  get skipped() { return t('words.itemStatusLabel.skipped'); },
  get failed() { return t('words.itemStatusLabel.failed'); },
  get not_started() { return t('words.itemStatusLabel.not_started'); },
  get unknown() { return t('words.itemStatusLabel.unknown'); },
  get in_flight() { return t('words.itemStatusLabel.in_flight'); },
  get pending() { return t('words.itemStatusLabel.pending'); },
};

/** Result codes explained for people. Unknown codes fall back to the host's own message. */
export const CODE_EXPLAIN: Record<string, string> = {
  get done() { return t('words.codeExplain.done'); },
  get protected() { return t('words.codeExplain.protected'); },
  get precondition_missing() { return t('words.codeExplain.precondition_missing'); },
  get precondition_link_or_reparse() { return t('words.codeExplain.precondition_link_or_reparse'); },
  get precondition_identity_changed() { return t('words.codeExplain.precondition_identity_changed'); },
  get precondition_parent_changed() { return t('words.codeExplain.precondition_parent_changed'); },
  get precondition_metadata_changed() { return t('words.codeExplain.precondition_metadata_changed'); },
  get precondition_in_use() { return t('words.codeExplain.precondition_in_use'); },
  get precondition_owner_running() { return t('words.codeExplain.precondition_owner_running'); },
  get precondition_unsupported() { return t('words.codeExplain.precondition_unsupported'); },
  get quarantine_unavailable() { return t('words.codeExplain.quarantine_unavailable'); },
  get cancelled() { return t('words.codeExplain.cancelled'); },
  get plan_expired() { return t('words.codeExplain.plan_expired'); },
  get authorization_revoked() { return t('words.codeExplain.authorization_revoked'); },
  get outcome_unconfirmed() { return t('words.codeExplain.outcome_unconfirmed'); },
  get reconciled_performed() { return t('words.codeExplain.reconciled_performed'); },
  get reconciled_not_performed() { return t('words.codeExplain.reconciled_not_performed'); },
  get never_started() { return t('words.codeExplain.never_started'); },
  get restored() { return t('words.codeExplain.restored'); },
  get purged() { return t('words.codeExplain.purged'); },
  get not_recoverable() { return t('words.codeExplain.not_recoverable'); },
  get target_conflict() { return t('words.codeExplain.target_conflict'); },
  get missing() { return t('words.codeExplain.missing'); },
  // Uninstall (UNI-02/03/06/09)
  get not_running() { return t('words.codeExplain.not_running'); },
  get closed() { return t('words.codeExplain.closed'); },
  get still_running() { return t('words.codeExplain.still_running'); },
  get app_still_running() { return t('words.codeExplain.app_still_running'); },
  get running_state_unknown() { return t('words.codeExplain.running_state_unknown'); },
  get leftovers_remain() { return t('words.codeExplain.leftovers_remain'); },
  get restart_required() { return t('words.codeExplain.restart_required'); },
  get cancelled_by_user() { return t('words.codeExplain.cancelled_by_user'); },
  get uninstaller_failed() { return t('words.codeExplain.uninstaller_failed'); },
  get record_changed() { return t('words.codeExplain.record_changed'); },
  get program_changed() { return t('words.codeExplain.program_changed'); },
  get start_failed() { return t('words.codeExplain.start_failed'); },
  get backup_failed() { return t('words.codeExplain.backup_failed'); },
  get precondition_changed() { return t('words.codeExplain.precondition_changed'); },
  get permission_required() { return t('words.codeExplain.permission_required'); },
  get destination_exists() { return t('words.codeExplain.destination_exists'); },
  // Updates (UPD-02/03)
  get updated() { return t('words.codeExplain.updated'); },
  get app_running() { return t('words.codeExplain.app_running'); },
  get running_unknown() { return t('words.codeExplain.running_unknown'); },
  get signature_mismatch() { return t('words.codeExplain.signature_mismatch'); },
  get unverifiable() { return t('words.codeExplain.unverifiable'); },
  get download_failed() { return t('words.codeExplain.download_failed'); },
  get tool_changed() { return t('words.codeExplain.tool_changed'); },
  get target_changed() { return t('words.codeExplain.target_changed'); },
  get adapter_mismatch() { return t('words.codeExplain.adapter_mismatch'); },
  get tool_failed() { return t('words.codeExplain.tool_failed'); },
  // File actions reported by the platform layer (a per-item result is never shown as the host's raw message).
  get access_denied() { return t('words.codeExplain.access_denied'); },
  get trash_unavailable() { return t('words.codeExplain.trash_unavailable'); },
  get rename_failed() { return t('words.codeExplain.rename_failed'); },
  get delete_failed() { return t('words.codeExplain.delete_failed'); },
  get new_descendant() { return t('words.codeExplain.new_descendant'); },
  get precondition_descendant_kept() { return t('words.codeExplain.precondition_descendant_kept'); },
  get invalid_target() { return t('words.codeExplain.invalid_target'); },
  get in_use() { return t('words.codeExplain.in_use'); },
  get changed_since_plan() { return t('words.codeExplain.changed_since_plan'); },
  get identity_changed() { return t('words.codeExplain.identity_changed'); },
  get tree_mismatch() { return t('words.codeExplain.tree_mismatch'); },
  get race_safety_unavailable() { return t('words.codeExplain.race_safety_unavailable'); },
  get unsupported() { return t('words.codeExplain.unsupported'); },
  // Processes (STS-04)
  get ended() { return t('words.codeExplain.ended'); },
  get precondition_gone() { return t('words.codeExplain.precondition_gone'); },
  get end_failed() { return t('words.codeExplain.end_failed'); },
  // NEW-13: the rest of the per-item results the executors write (cm-app plans.rs / uninstall.rs, cm-platform
  // actions, startup and recovery); `codes.test.ts` reads those sources so a new code cannot ship without words.
  get already_prior() { return t('words.codeExplain.already_prior'); },
  get already_restored() { return t('words.codeExplain.already_restored'); },
  get backup_changed() { return t('words.codeExplain.backup_changed'); },
  get backup_missing() { return t('words.codeExplain.backup_missing'); },
  get backup_unavailable() { return t('words.codeExplain.backup_unavailable'); },
  get backup_incomplete() { return t('words.codeExplain.backup_incomplete'); },
  get changed_since() { return t('words.codeExplain.changed_since'); },
  get ended_on_request() { return t('words.codeExplain.ended_on_request'); },
  get not_in_trash() { return t('words.codeExplain.not_in_trash', { bin: words().bin }); },
  get setting_unknown() { return t('words.codeExplain.setting_unknown'); },
  get state_unknown() { return t('words.codeExplain.state_unknown'); },
  get write_failed() { return t('words.codeExplain.write_failed'); },
  get check_failed() { return t('words.codeExplain.check_failed'); },
  get privilege_state() { return t('words.codeExplain.privilege_state'); },
  get parent_missing() { return t('words.codeExplain.parent_missing'); },
  get quarantine_unsafe() { return t('words.codeExplain.quarantine_unsafe'); },
  get cross_volume() { return t('words.codeExplain.cross_volume'); },
  get invalid_name() { return t('words.codeExplain.invalid_name'); },
  get not_stale() { return t('words.codeExplain.not_stale'); },
  get root_mismatch() { return t('words.codeExplain.root_mismatch'); },
  get target_unreachable() { return t('words.codeExplain.target_unreachable'); },
  // Restore checks report the bare live-check codes (cm-platform windows/recovery.rs): same meaning as the precondition.
  get parent_changed() { return t('words.codeExplain.precondition_parent_changed'); },
  get metadata_changed() { return t('words.codeExplain.precondition_metadata_changed'); },
};

export function explainCode(code: string | null, message: string | null): string {
  if (code && CODE_EXPLAIN[code]) return CODE_EXPLAIN[code];
  return message ?? '';
}

export const AVAILABILITY_LABEL: Record<Availability, string> = {
  get in_app_restore() { return t('words.availabilityLabel.in_app_restore'); },
  get system_recycle_bin() { return t('words.restoreSystemBin', { bin: words().bin }); },
  get unavailable() { return t('words.availabilityLabel.unavailable'); },
  get not_applicable() { return t('words.availabilityLabel.not_applicable'); },
  get unknown() { return t('words.availabilityLabel.unknown'); },
};

/**
 * R3-02: the Activity list's recovery note for one operation, from the host's facts (`availability` and the history
 * `action`, plus whether a succeeded action went to the system bin), never from the plan's mode. "In-app restore"
 * means a different thing per kind: the quarantine, a recorded startup state, a recorded setting value, or the
 * registry backups of uninstall leftovers (whose files, when any were removed, are in the system bin).
 */
export function historyRecoveryLabel(o: { action: string | null; availability: Availability; recycle_bin_items?: boolean }): string {
  if (o.availability !== 'in_app_restore') return AVAILABILITY_LABEL[o.availability];
  switch (o.action) {
    case 'quarantine':
      return AVAILABILITY_LABEL.in_app_restore;
    case 'startup':
      return t('words.recoveryLabel.startup');
    case 'maintenance':
      return t('words.recoveryLabel.maintenance');
    case 'uninstall_leftovers':
      return o.recycle_bin_items ? t('words.recoveryLabel.leftoversAndBin', { bin: words().bin }) : t('words.recoveryLabel.leftovers');
    default:
      return t('words.recoveryLabel.inApp');
  }
}

export const ACTION_LABEL: Record<string, string> = {
  get trash() { return words().moveToBin; },
  get quarantine() { return t('words.actionLabel.quarantine'); },
  get delete() { return t('words.actionLabel.delete'); },
  get restore() { return t('words.actionLabel.restore'); },
  get purge() { return t('words.actionLabel.purge'); },
  get startup() { return t('words.actionLabel.startup'); },
  get maintenance() { return t('words.actionLabel.maintenance'); },
  get uninstall() { return t('words.actionLabel.uninstall'); },
  get uninstall_leftovers() { return t('words.actionLabel.uninstall_leftovers'); },
  get uninstall_restore() { return t('words.actionLabel.uninstall_restore'); },
  get update() { return t('words.actionLabel.update'); },
  get process() { return t('words.actionLabel.process'); },
  // Restores of recorded prior states (history `action`, WFA-40); `restore` stays the quarantine recovery.
  get startup_restore() { return t('words.actionLabel.startup_restore'); },
  get maintenance_restore() { return t('words.actionLabel.maintenance_restore'); },
};

export const QUARANTINE_STATE_LABEL: Record<string, string> = {
  get recoverable() { return t('words.quarantineStateLabel.recoverable'); },
  get target_conflict() { return t('words.quarantineStateLabel.target_conflict'); },
  get volume_offline() { return t('words.quarantineStateLabel.volume_offline'); },
  get volume_changed() { return t('words.quarantineStateLabel.volume_changed'); },
  get identity_changed() { return t('words.quarantineStateLabel.identity_changed'); },
  get missing() { return t('words.quarantineStateLabel.missing'); },
  get parent_changed() { return t('words.quarantineStateLabel.parent_changed'); },
  get manifest_incomplete() { return t('words.quarantineStateLabel.manifest_incomplete'); },
  get unknown() { return t('words.quarantineStateLabel.unknown'); },
};

export const POLICY_STATE_LABEL: Record<PolicyState, string> = {
  get enabled() { return t('words.policyStateLabel.enabled'); },
  get paused() { return t('words.policyStateLabel.paused'); },
  get revoked() { return t('words.policyStateLabel.revoked'); },
};

export const POLICY_MISMATCH_LABEL: Record<string, string> = {
  get not_enabled() { return t('words.policyMismatchLabel.not_enabled'); },
  get expired() { return t('words.policyMismatchLabel.expired'); },
  get other_principal() { return t('words.policyMismatchLabel.other_principal'); },
  get scope_differs() { return t('words.policyMismatchLabel.scope_differs'); },
  get mode_differs() { return t('words.policyMismatchLabel.mode_differs'); },
  get rule_not_covered() { return t('words.policyMismatchLabel.rule_not_covered'); },
  get tier_not_default_selected() { return t('words.policyMismatchLabel.tier_not_default_selected'); },
  get size_unknown() { return t('words.policyMismatchLabel.size_unknown'); },
  get single_limit() { return t('words.policyMismatchLabel.single_limit'); },
  get quota_exceeded() { return t('words.policyMismatchLabel.quota_exceeded'); },
};

export const LAYER_LABEL: Record<string, string> = {
  get hard_deny() { return t('words.layerLabel.hard_deny'); },
  get default_category() { return t('words.layerLabel.default_category'); },
  get user() { return t('words.layerLabel.user'); },
};

export const MATCH_LABEL: Record<string, string> = {
  get exact() { return t('words.matchLabel.exact'); },
  get descendants() { return t('words.matchLabel.descendants'); },
  get ancestors() { return t('words.matchLabel.ancestors'); },
  get glob() { return t('words.matchLabel.glob'); },
  get rule() { return t('words.matchLabel.rule'); },
  get maintenance_task() { return t('words.matchLabel.maintenance_task'); },
};

/** Segments of a fixed-total progress bar: every planned action is in exactly one segment. */
export function progressSegments(total: number, c: Counts): { key: ItemStatus; count: number }[] {
  const done = c.succeeded + c.skipped + c.failed + c.not_started + c.unknown + c.in_flight;
  const pending = Math.max(0, total - done);
  const order: [ItemStatus, number][] = [
    ['succeeded', c.succeeded],
    ['skipped', c.skipped],
    ['failed', c.failed],
    ['unknown', c.unknown],
    ['not_started', c.not_started],
    ['in_flight', c.in_flight],
    ['pending', pending],
  ];
  return order.filter(([, n]) => n > 0).map(([key, count]) => ({ key, count }));
}

/** Seconds as "12 分 05 秒" / "45 秒". */
export function formatCountdown(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  if (s < 60) return t('common.seconds', { s });
  return t('words.floorMinPadstart', { floor: Math.floor(s / 60), padStart: String(s % 60).padStart(2, '0') });
}

/** The digest in groups of 8 for reading aloud and comparing with the CLI. */
export function digestGroups(digest: string): string[] {
  const out: string[] = [];
  for (let i = 0; i < digest.length; i += 8) out.push(digest.slice(i, i + 8));
  return out;
}

/** Normalize what a person typed for the 8-character confirmation code. */
export function normalizeCode(typed: string): string {
  return typed.replace(/[\s-]/g, '').toLowerCase().slice(0, 8);
}
