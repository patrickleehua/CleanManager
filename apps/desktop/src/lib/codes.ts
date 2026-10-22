// One code → 中文 table for everything the host can report by code: scan issues, the issue lists of
// analysis / catalogs / startup / apps, and refusal reasons (`IpcError.reason`). The main text a person
// reads is always Chinese; a code the table does not know reads "未识别的问题（代码 x）" and its raw host
// message is kept for a collapsed detail, never shown as the main text.
//
// Refusal reasons extend `ERROR_HINT` (format.ts), which keeps its existing entries unchanged.

import { ERROR_HINT } from './format';
import { macOverlay, onPlatform, words } from './platform';
import type { Issue, PermissionCause } from './types';

/** Scan issues and the other issue lists (cleanup analysis, catalogs, startup, apps). */
export const ISSUE_TEXT: Record<string, string> = {
  // The scan root itself (the scan ends `failed` before any row is written).
  not_a_directory: '所选路径是文件，不是文件夹，请选择一个文件夹。',
  root_not_directory: '所选路径是文件，不是文件夹，请选择一个文件夹。',
  root_is_link: '所选路径是链接（目录联接或符号链接），请直接选择它指向的文件夹。',
  root_unavailable: '找不到所选文件夹，或它所在的卷已离线。请连接后重新扫描，或选择其他文件夹。',
  root_missing: '文件夹不存在或所在的卷已离线。',
  unsupported_path: '这个路径不是本机可以扫描的位置，请选择这台电脑上的文件夹。',
  scope_requires_volume: '这种读取方式只能扫描整个卷，请选择卷的根目录。',
  unsupported_filesystem: '这个卷的文件系统不支持这种读取方式。',
  unsupported: '这个平台还没有可用的扫描方式。',
  // Inside the scan.
  permission_required: '以普通权限无法读取这个位置。CleanManager 不会自动提权，它的大小记为未知。',
  permission_denied: '没有权限读取这个位置，其中的项目没有列出。',
  io_error: '读取这个位置时出错，它没有计入结果。',
  internal_error: 'CleanManager 处理这个位置时出错，它没有计入结果。',
  own_store_skipped: '这是 CleanManager 自己的数据目录，按设计不扫描。',
  store_error: '扫描结果没能完整写入索引，本次结果可能缺少部分条目。',
  volume_changed: '扫描期间卷发生了变化，扫描已停止。请重新扫描。',
  identity_changed: '这个文件夹在列出后被替换，为安全起见没有进入。',
  root_changed: '所选文件夹在扫描期间被移动或替换，扫描已停止。请重新扫描。',
  volume_offline: '扫描期间卷不再响应（可能已拔出或断开），扫描已停止。',
  mount_boundary: '这里挂载了另一个卷，没有进入。',
  cloud_materialization_avoided: '这是只在云端的项目，为避免触发下载没有进入。',
  materialization_policy_unavailable: '无法阻止云端文件被下载，为安全起见云端项目没有读取。',
  entry_vanished: '这个条目在扫描期间被删除或移走，没有计入。',
  bulk_record_malformed: '系统返回的目录记录格式异常，这部分没有计入。',
  supplement_failed: '补充读取这个文件的详细信息时出错，它的大小可能未知。',
  timeout: '扫描超过了等待时间，已保留到此为止确认的结果。',
  host_exited: '运行扫描的程序在完成前退出，已保留确认的部分。',
  // Cleanup analysis.
  rule_inapplicable: '有规则不适用于这台电脑，已跳过。',
  undetermined: '有匹配的对象暂时无法判断，没有列为候选。',
  needs_reanalysis: '检查结果已过时，请重新分析。',
  protected_reviewable: '有对象默认受保护，只能审阅。',
  protection_unavailable: '保护清单无法读取。为安全起见，所有写操作已停止。',
  // Build artifacts / installers.
  root_contains_store: '这个文件夹包含 CleanManager 自己的数据，没有扫描。',
  scan_failed: '扫描这个位置时失败。',
  cloud_placeholders: '有只在云端的文件，没有打开或识别。',
  // Startup items.
  registry_open_failed: '无法打开一个注册表位置，其中的项目没有列出。',
  registry_enum_failed: '无法读取一个注册表位置中的全部项目。',
  registry_entry_unreadable: '有一条登记无法读取，已跳过。',
  known_folder_unavailable: '找不到一个系统文件夹（例如“启动”文件夹）。',
  folder_unreadable: '无法读取一个启动文件夹。',
  task_scheduler_unavailable: '无法连接任务计划程序，计划任务类启动项没有列出。',
  task_folder_unreadable: '无法读取任务计划程序中的一个文件夹。',
  // Apps.
  unrecognized_package_entry: '有一条应用包登记无法识别，已跳过。',
  root_unreadable: '无法读取应用文件夹。',
  caskroom_unreadable: '无法读取 Homebrew 的 Caskroom 目录。',
  cask_metadata_unreadable: '有一个 Homebrew 应用的信息无法读取。',
  receipts_unreadable: '无法读取安装收据。',
  invalid_argument: '读取应用清单的条件无效，清单可能不完整。',
};

/** Why a permission issue happened (`permission_cause`). */
export const PERMISSION_CAUSE_TEXT: Record<PermissionCause, string> = {
  privacy_prompt_pending: '正在等待 macOS 授权：请在弹出的对话框中选择，然后重新扫描。',
  undetermined: ISSUE_TEXT.permission_required,
};

/** Refusal reasons the hint table did not cover (IPC `reason`, trash refusals, readiness). */
export const EXTRA_REFUSAL_TEXT: Record<string, string> = {
  root_not_directory: '所选路径是文件，不是文件夹，请选择一个文件夹。',
  root_unavailable: '找不到所选文件夹，或它所在的卷已离线。',
  ipc: '与 CleanManager 宿主通信失败，未做任何更改。',
  invalid_argument: '请求的内容不符合要求，未做任何更改。',
  schema_mismatch: '界面与核心的版本不一致，未做任何更改。请重新启动 CleanManager。',
  internal: 'CleanManager 内部出错，未做任何更改。',
  internal_error: 'CleanManager 内部出错，未做任何更改。',
  cancelled: '已取消，未做任何更改。',
  audit_unavailable: '操作记录无法写入。为安全起见没有执行。',
  no_store_dir: 'CleanManager 的数据目录不可用，扫描结果和计划无法保存。',
  no_store: 'CleanManager 的数据目录不可用，扫描结果和计划无法保存。',
  store_error: '写入 CleanManager 自己的记录时出错，未做任何更改。',
  experimental_off: '这项功能默认关闭，可以在“设置 › 实验功能”中开启。',
  diag_not_configured: '还没有配置 AI 诊断的提供方。',
  diag_unsupported: '这个对象不支持 AI 诊断。',
  diag_not_found: '找不到这条诊断记录。',
  diag_preview_changed: '要发送的内容在确认前发生了变化，请重新预览。',
  diag_store: '诊断记录无法保存。',
  cli_rejected: '这个命令行工具不符合要求，不会被启动。',
  credential_store: '无法访问系统的凭据存储，密钥没有保存。',
  handoff_unavailable: '这条建议不能转为计划。',
  needs_reanalysis: '所选对象不在当前的分析结果中，请重新分析。',
  plan_kind_violation: '这种计划不能执行这个动作。',
  not_cleanup_plan: '只有清理计划可以由策略覆盖。',
  no_rules: '这份计划没有引用规则版本，不能据此创建策略。',
  unsupported_name: '这个名称无法准确保存到保护清单中。',
  unknown_rule: '找不到这条规则，请刷新后重试。',
  not_checked: '还没有检查过更新。',
  unknown_app: '找不到这个应用，请刷新列表。',
  no_purge_roots: '还没有添加项目文件夹。',
  not_reviewed: '请先逐项审阅这些下载。',
  no_downloads: '没有可处理的旧下载。',
  scan_failed: '扫描失败，未做任何更改。',
  refused: '核心拒绝了这个请求，未做任何更改。',
  not_running: '这个进程已经不在运行。',
  scan_root: '不能把整个扫描范围移到回收站。',
  in_recycle_bin: '这个对象已经在回收站中。',
  link_or_reparse: '这是链接或重分析点，不会被跟随，也不会被移动。',
  protected_descendants: '文件夹里有受保护的项目，不能整体移动。',
  cloud_placeholder: '这是只在云端的文件，不能移动。',
  unknown_size: '大小未知，不能放入计划。',
  hard_links: '这个文件有多个硬链接，移动它不会释放空间。',
  weak_identity: '无法可靠地识别这个对象，为安全起见不能放入计划。',
  folder_move_unavailable: '这一版不能把文件夹移到废纸篓。',
};

// macOS words: a link root is a symbolic link or an alias; read permission is granted in System Settings.
macOverlay(ISSUE_TEXT, {
  root_is_link: '所选路径是替身或符号链接，请直接选择它指向的文件夹。',
  permission_required: '以普通权限无法读取这个位置，它的大小记为未知。可以在“系统设置 › 隐私与安全性”中允许 CleanManager 访问，然后重新扫描。',
});
const ROOT_IS_LINK_WINDOWS = ERROR_HINT.root_is_link;
onPlatform((p) => {
  PERMISSION_CAUSE_TEXT.undetermined = ISSUE_TEXT.permission_required;
  ERROR_HINT.root_is_link = p === 'macos' ? ISSUE_TEXT.root_is_link : ROOT_IS_LINK_WINDOWS;
  EXTRA_REFUSAL_TEXT.scan_root = `不能把整个扫描范围移到${words().bin}。`;
  EXTRA_REFUSAL_TEXT.in_recycle_bin = `这个对象已经在${words().bin}中。`;
});

/** Main text for a code nobody has put in words yet; the raw message goes into a collapsed detail. */
export function unknownCodeText(code: string): string {
  return `未识别的问题（代码 ${code || '空'}）`;
}

/** Chinese text of a refusal reason (IPC error), or the unknown-code line. */
export function refusalText(e: { reason?: string | null; code?: string | null }): string {
  const reason = e.reason ?? '';
  return ERROR_HINT[reason] ?? EXTRA_REFUSAL_TEXT[reason] ?? ISSUE_TEXT[reason] ?? (e.code ? (ERROR_HINT[e.code] ?? EXTRA_REFUSAL_TEXT[e.code]) : undefined) ?? unknownCodeText(reason || (e.code ?? ''));
}

/** Whether a refusal reason has Chinese words. */
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
  /** The Chinese main text: always set. */
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

export function groupIssues(issues: Issue[]): IssueGroup[] {
  const groups = new Map<string, IssueGroup>();
  for (const i of issues) {
    const e = explainIssue(i);
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

/** Codes a root refusal reports: the scan ends before any row and the next step is choosing another root. */
export const ROOT_REFUSALS = ['not_a_directory', 'root_not_directory', 'root_is_link', 'root_unavailable', 'root_missing', 'unsupported_path', 'scope_requires_volume', 'unsupported_filesystem', 'root_inside_store', 'store_inside_scope'];
