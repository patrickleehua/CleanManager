// One code → words table for everything the host can report by code: scan issues, the issue lists of
// analysis / catalogs / startup / apps, and refusal reasons (`IpcError.reason`). The main text a person
// reads always comes from the language resources (src/i18n, 简体中文 and English, LIF-05); a code the table does not
// know reads "未识别的问题（代码 x）" / "Unrecognized issue (code x)" and its raw host message is kept for a
// collapsed detail, never shown as the main text.
//
// Refusal reasons extend `ERROR_HINT` (format.ts), which keeps its existing entries unchanged.

import { ERROR_HINT } from './format';
import { words } from './platform';
import type { Issue, PermissionCause } from './types';
import { CODE_EXPLAIN } from './words';
import { t } from '../i18n';

/** Scan issues and the other issue lists (cleanup analysis, catalogs, startup, apps). */
export const ISSUE_TEXT: Record<string, string> = {
  // The scan root itself (the scan ends `failed` before any row is written).
  get not_a_directory() { return t('codes.issueText.not_a_directory'); },
  get root_not_directory() { return t('codes.issueText.root_not_directory'); },
  get root_is_link() { return t('codes.issueText.root_is_link'); },
  get root_unavailable() { return t('codes.issueText.root_unavailable'); },
  get root_missing() { return t('codes.issueText.root_missing'); },
  get unsupported_path() { return t('codes.issueText.unsupported_path'); },
  get scope_requires_volume() { return t('codes.issueText.scope_requires_volume'); },
  get unsupported_filesystem() { return t('codes.issueText.unsupported_filesystem'); },
  get unsupported() { return t('codes.issueText.unsupported'); },
  // Inside the scan.
  get permission_required() { return t('codes.issueText.permission_required'); },
  get permission_denied() { return t('codes.issueText.permission_denied'); },
  get io_error() { return t('codes.issueText.io_error'); },
  get internal_error() { return t('codes.issueText.internal_error'); },
  get own_store_skipped() { return t('codes.issueText.own_store_skipped'); },
  get store_error() { return t('codes.issueText.store_error'); },
  get volume_changed() { return t('codes.issueText.volume_changed'); },
  get identity_changed() { return t('codes.issueText.identity_changed'); },
  get root_changed() { return t('codes.issueText.root_changed'); },
  get volume_offline() { return t('codes.issueText.volume_offline'); },
  get mount_boundary() { return t('codes.issueText.mount_boundary'); },
  get cloud_materialization_avoided() { return t('codes.issueText.cloud_materialization_avoided'); },
  get materialization_policy_unavailable() { return t('codes.issueText.materialization_policy_unavailable'); },
  get entry_vanished() { return t('codes.issueText.entry_vanished'); },
  get bulk_record_malformed() { return t('codes.issueText.bulk_record_malformed'); },
  get supplement_failed() { return t('codes.issueText.supplement_failed'); },
  get timeout() { return t('codes.issueText.timeout'); },
  get host_exited() { return t('codes.issueText.host_exited'); },
  // Cleanup analysis.
  get rule_inapplicable() { return t('codes.issueText.rule_inapplicable'); },
  get undetermined() { return t('codes.issueText.undetermined'); },
  get needs_reanalysis() { return t('codes.issueText.needs_reanalysis'); },
  get protected_reviewable() { return t('codes.issueText.protected_reviewable'); },
  get protection_unavailable() { return t('codes.issueText.protection_unavailable'); },
  // Build artifacts / installers.
  get root_contains_store() { return t('codes.issueText.root_contains_store'); },
  get scan_failed() { return t('codes.issueText.scan_failed'); },
  get cloud_placeholders() { return t('codes.issueText.cloud_placeholders'); },
  // Startup items.
  get registry_open_failed() { return t('codes.issueText.registry_open_failed'); },
  get registry_enum_failed() { return t('codes.issueText.registry_enum_failed'); },
  get registry_entry_unreadable() { return t('codes.issueText.registry_entry_unreadable'); },
  get known_folder_unavailable() { return t('codes.issueText.known_folder_unavailable'); },
  get folder_unreadable() { return t('codes.issueText.folder_unreadable'); },
  get task_scheduler_unavailable() { return t('codes.issueText.task_scheduler_unavailable'); },
  get task_folder_unreadable() { return t('codes.issueText.task_folder_unreadable'); },
  // Apps.
  get unrecognized_package_entry() { return t('codes.issueText.unrecognized_package_entry'); },
  get root_unreadable() { return t('codes.issueText.root_unreadable'); },
  get caskroom_unreadable() { return t('codes.issueText.caskroom_unreadable'); },
  get cask_metadata_unreadable() { return t('codes.issueText.cask_metadata_unreadable'); },
  get receipts_unreadable() { return t('codes.issueText.receipts_unreadable'); },
  get invalid_argument() { return t('codes.issueText.invalid_argument'); },
};

/** Default protection categories (`protection.class` of a candidate, built-in protection rows `dc_<class>`). */
export const PROTECTION_CLASS_TEXT: Record<string, string> = {
  get credentials() { return t('codes.protectionClassText.credentials'); },
  get database() { return t('codes.protectionClassText.database'); },
  get browser_site_storage() { return t('codes.protectionClassText.browser_site_storage'); },
  get ai_sessions_config() { return t('codes.protectionClassText.ai_sessions_config'); },
  get source_worktree() { return t('codes.protectionClassText.source_worktree'); },
  get models() { return t('codes.protectionClassText.models'); },
  get user_documents() { return t('codes.protectionClassText.user_documents'); },
  get system_repair() { return t('codes.protectionClassText.system_repair'); },
};

/** A protection class in words; a class the table does not know reads as the unknown-code line. */
export function protectionText(cls: string): string {
  return PROTECTION_CLASS_TEXT[cls] ?? unknownCodeText(cls);
}

/** Why a permission issue happened (`permission_cause`). */
export const PERMISSION_CAUSE_TEXT: Record<PermissionCause, string> = {
  get privacy_prompt_pending() { return t('codes.permissionCauseText.privacy_prompt_pending'); },
  get undetermined() { return ISSUE_TEXT.permission_required; },
};

/** Refusal reasons the hint table did not cover (IPC `reason`, trash refusals, readiness). */
export const EXTRA_REFUSAL_TEXT: Record<string, string> = {
  get root_not_directory() { return t('codes.extraRefusalText.root_not_directory'); },
  get root_unavailable() { return t('codes.extraRefusalText.root_unavailable'); },
  get ipc() { return t('codes.extraRefusalText.ipc'); },
  get invalid_argument() { return t('codes.extraRefusalText.invalid_argument'); },
  get schema_mismatch() { return t('codes.extraRefusalText.schema_mismatch'); },
  get internal() { return t('codes.extraRefusalText.internal'); },
  get internal_error() { return t('codes.extraRefusalText.internal_error'); },
  get cancelled() { return t('codes.extraRefusalText.cancelled'); },
  get audit_unavailable() { return t('codes.extraRefusalText.audit_unavailable'); },
  // Protocol code of a damaged or incompatible scan index (the reasons index_corrupt/index_incompatible say which).
  get index_unavailable() { return t('codes.extraRefusalText.index_unavailable'); },
  get no_store_dir() { return t('codes.extraRefusalText.no_store_dir'); },
  get no_store() { return t('codes.extraRefusalText.no_store'); },
  get store_error() { return t('codes.extraRefusalText.store_error'); },
  get experimental_off() { return t('codes.extraRefusalText.experimental_off'); },
  get diag_not_configured() { return t('codes.extraRefusalText.diag_not_configured'); },
  get diag_unsupported() { return t('codes.extraRefusalText.diag_unsupported'); },
  get diag_not_found() { return t('codes.extraRefusalText.diag_not_found'); },
  get diag_preview_changed() { return t('codes.extraRefusalText.diag_preview_changed'); },
  get diag_store() { return t('codes.extraRefusalText.diag_store'); },
  get cli_rejected() { return t('codes.extraRefusalText.cli_rejected'); },
  get credential_store() { return t('codes.extraRefusalText.credential_store'); },
  get handoff_unavailable() { return t('codes.extraRefusalText.handoff_unavailable'); },
  get needs_reanalysis() { return t('codes.extraRefusalText.needs_reanalysis'); },
  get plan_kind_violation() { return t('codes.extraRefusalText.plan_kind_violation'); },
  get not_cleanup_plan() { return t('codes.extraRefusalText.not_cleanup_plan'); },
  get no_rules() { return t('codes.extraRefusalText.no_rules'); },
  get unsupported_name() { return t('codes.extraRefusalText.unsupported_name'); },
  get unknown_rule() { return t('codes.extraRefusalText.unknown_rule'); },
  get not_checked() { return t('codes.extraRefusalText.not_checked'); },
  get unknown_app() { return t('codes.extraRefusalText.unknown_app'); },
  get no_purge_roots() { return t('codes.extraRefusalText.no_purge_roots'); },
  get not_reviewed() { return t('codes.extraRefusalText.not_reviewed'); },
  get no_downloads() { return t('codes.extraRefusalText.no_downloads'); },
  get scan_failed() { return t('codes.extraRefusalText.scan_failed'); },
  get refused() { return t('codes.extraRefusalText.refused'); },
  get not_running() { return t('codes.extraRefusalText.not_running'); },
  get scan_root() { return t('codes.wholeScanScopeCant', { bin: words().bin }); },
  get in_recycle_bin() { return t('codes.objectAlreadyBin', { bin: words().bin }); },
  get link_or_reparse() { return t('codes.extraRefusalText.link_or_reparse'); },
  get protected_descendants() { return t('codes.extraRefusalText.protected_descendants'); },
  get cloud_placeholder() { return t('codes.extraRefusalText.cloud_placeholder'); },
  get unknown_size() { return t('codes.extraRefusalText.unknown_size'); },
  get hard_links() { return t('codes.extraRefusalText.hard_links'); },
  get weak_identity() { return t('codes.extraRefusalText.weak_identity'); },
  get folder_move_unavailable() { return t('codes.extraRefusalText.folder_move_unavailable'); },
};

// macOS readings (a link root is a symbolic link or an alias; read permission is granted in System Settings) are the
// `@macos` keys of the resources (src/i18n); they follow the platform without any code here.

/** Main text for a code nobody has put in words yet; the raw message goes into a collapsed detail. */
export function unknownCodeText(code: string): string {
  return t('codes.unrecognizedIssueCodeP', { p: code || t('codes.empty') });
}

/** Interface text of a refusal reason (IPC error), or the unknown-code line. */
export function refusalText(e: { reason?: string | null; code?: string | null }): string {
  const reason = e.reason ?? '';
  return ERROR_HINT[reason] ?? EXTRA_REFUSAL_TEXT[reason] ?? ISSUE_TEXT[reason] ?? (e.code ? (ERROR_HINT[e.code] ?? EXTRA_REFUSAL_TEXT[e.code]) : undefined) ?? unknownCodeText(reason || (e.code ?? ''));
}

/** Whether a refusal reason has words (in every language: the resources share their keys). */
export function refusalKnown(reason: string): boolean {
  return reason in ERROR_HINT || reason in EXTRA_REFUSAL_TEXT || reason in ISSUE_TEXT;
}

const CAUSE_RE = /permission_cause=([a-z_]+)/;

/** The structured cause of a permission issue: the field when the host sends it, else the marker in its message. */
export function permissionCause(i: Pick<Issue, 'code' | 'message'> & { permission_cause?: PermissionCause | null }): PermissionCause | null {
  if (i.permission_cause) return i.permission_cause;
  const m = CAUSE_RE.exec(i.message ?? '');
  if (m && (m[1] === 'privacy_prompt_pending' || m[1] === 'undetermined')) return m[1];
  return null;
}

export interface ExplainedIssue {
  /** The code the text is keyed by (a symbolic-link root reported as `root_unavailable` reads as `root_is_link`). */
  code: string;
  /** The host's own code. */
  rawCode: string;
  known: boolean;
  cause: PermissionCause | null;
  /** The main text in the interface language: always set. */
  text: string;
  /** The host's raw message, for a collapsed detail only. */
  raw: string;
  scope: string;
}

/** A scan / inventory issue in words. Unknown codes read "未识别的问题（代码 x）". */
export function explainIssue(i: Issue): ExplainedIssue {
  const cause = permissionCause(i);
  let code = i.code;
  // The macOS backend reports a link root as root_unavailable with this message; it means the same as root_is_link.
  if (code === 'root_unavailable' && /symbolic link|links are not followed/i.test(i.message ?? '')) code = 'root_is_link';
  const text = cause ? PERMISSION_CAUSE_TEXT[cause] : (ISSUE_TEXT[code] ?? ERROR_HINT[code] ?? EXTRA_REFUSAL_TEXT[code]);
  return { code, rawCode: i.code, known: text !== undefined, cause, text: text ?? unknownCodeText(i.code), raw: i.message ?? '', scope: i.scope_display ?? '' };
}

/** Issues of one meaning together: one sentence, how many places, a few of them. */
export interface IssueGroup {
  key: string;
  issue: ExplainedIssue;
  count: number;
  scopes: string[];
  raws: { code: string; scope: string; message: string }[];
}

export function groupIssues(issues: Issue[], explain: (i: Issue) => ExplainedIssue = explainIssue): IssueGroup[] {
  const groups = new Map<string, IssueGroup>();
  for (const i of issues) {
    const e = explain(i);
    // Unknown codes stay one per code; known ones group by meaning (code + cause).
    const key = `${e.code}|${e.cause ?? ''}`;
    let g = groups.get(key);
    if (!g) {
      g = { key, issue: e, count: 0, scopes: [], raws: [] };
      groups.set(key, g);
    }
    g.count += 1;
    if (e.scope && !g.scopes.includes(e.scope)) g.scopes.push(e.scope);
    g.raws.push({ code: i.code, scope: e.scope, message: e.raw });
  }
  // Waiting for an answer comes first: it is the one thing the person can act on right now.
  return [...groups.values()].sort((a, b) => Number(b.issue.cause === 'privacy_prompt_pending') - Number(a.issue.cause === 'privacy_prompt_pending'));
}

/**
 * A per-item execution result (operation items: skipped, failed, unknown…) in words. The result table
 * (`CODE_EXPLAIN`) comes first, then the issue and refusal tables; an unknown code reads
 * "未识别的问题（代码 x）" and the host's message stays for the collapsed detail.
 */
export function explainResult(i: Issue): ExplainedIssue {
  const known = CODE_EXPLAIN[i.code];
  if (known) return { code: i.code, rawCode: i.code, known: true, cause: null, text: known, raw: i.message ?? '', scope: i.scope_display ?? '' };
  return explainIssue(i);
}

/** The main text of a result code: its words, or the unknown-code line — never the host's raw message. */
export function resultText(code: string | null): string {
  if (!code) return '';
  return explainResult({ code, scope_display: '', message: '' }).text;
}

/** Codes a root refusal reports: the scan ends before any row and the next step is choosing another root. */
export const ROOT_REFUSALS = ['not_a_directory', 'root_not_directory', 'root_is_link', 'root_unavailable', 'root_missing', 'unsupported_path', 'scope_requires_volume', 'unsupported_filesystem', 'root_inside_store', 'store_inside_scope'];
