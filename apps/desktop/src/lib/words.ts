// One vocabulary for cleanup, plans, execution and recovery. Every risk, state and outcome has
// words (never colour alone); the same action keeps the same name through the whole flow.

import { onPlatform, words } from './platform';
import type { Availability, AuthStatus, Counts, Group, ItemStatus, Mode, NotSelectable, PlanKind, PolicyState } from './types';

export const MODE_LABEL: Record<Mode, string> = {
  trash: '移到回收站',
  quarantine: '移入同卷隔离区',
  delete: '永久删除',
};

/** What each mode does to the object and to free space, and how it can be undone. */
export const MODE_EXPLAIN: Record<Mode, { effect: string; space: string; recovery: string }> = {
  trash: {
    effect: '交给系统回收站。',
    space: '回收站仍占用这个卷的空间，清空回收站后才释放。',
    recovery: '在资源管理器的回收站中还原；CleanManager 无法替你还原。',
  },
  quarantine: {
    effect: '移动到同一个卷上的 CleanManager 隔离区，原位置不再有这些文件。',
    space: '此操作不立即释放空间：文件仍在同一个卷上，直到你另行创建并批准“永久清空”计划。',
    recovery: '在“活动 › 隔离区”为它们创建恢复计划；原位置被占用时跳过，不会覆盖。',
  },
  delete: {
    effect: '直接从磁盘删除。',
    space: '删除成功的文件立即释放其占用（实际变化以卷空间观测为准）。',
    recovery: '无法撤销，没有回收站或隔离副本。',
  },
};

export const PLAN_KIND_LABEL: Record<PlanKind, string> = {
  cleanup: '清理计划',
  restore: '恢复计划',
  purge: '永久清空计划',
  manual_trash: '手选回收站计划',
  manual_purge: '永久删除回收站项目计划',
  startup: '启动项计划',
  startup_restore: '启动项还原计划',
  uninstall: '卸载计划',
  uninstall_leftovers: '卸载残留计划',
  uninstall_restore: '注册表还原计划',
  update: '应用更新计划',
  process: '结束进程计划',
  maintenance: '维护计划',
};

export const RECOVERY_EXPLAIN: Record<'restore' | 'purge', { effect: string; space: string; recovery: string }> = {
  restore: {
    effect: '把每一项移回原来的文件夹。',
    space: '隔离区与原位置在同一个卷上，恢复不改变可用空间。',
    recovery: '原位置已有同名对象时跳过该项，从不覆盖；被跳过的项目仍留在隔离区。',
  },
  purge: {
    effect: '永久删除隔离区中的这些对象。',
    space: '清空成功后释放这些对象占用的空间。',
    recovery: '无法撤销。清空授权不继承自当初的隔离授权，需要单独批准。',
  },
};

export const GROUP_LABEL: Record<Group, string> = {
  low: '低风险',
  review: '需要审阅',
  pending: '待判断',
  blocked: '不可选择',
};

export const GROUP_EXPLAIN: Record<Group, string> = {
  low: '规则已验证可再生成。即使如此也不会预先勾选，请确认后再选择。',
  review: '可以处理，但删除后可能需要重新下载、重新登录或丢失本地状态。请逐项确认影响。',
  pending: '证据不完整（有未读取的子项或时间未知），在补齐证据前不能放入计划。',
  blocked: '受保护、属于硬拒绝范围，或只允许逐项审阅的对象。这里只说明原因，不能选择。',
};

export const NOT_SELECTABLE_LABEL: Record<NotSelectable, string> = {
  blocked: '受保护或被规则拒绝',
  protected_reviewable: '默认保护：仅可逐项审阅，不进入批量计划',
  pending_evidence: '证据不完整，待判断',
  no_mode: '此规则不提供文件操作',
  unknown_size: '含大小未知的文件，无法冻结进计划',
  hint: '大项提示：只给出处理建议，不进入计划',
  adapter: '由所属工具清理；此版本只预览，不提供文件操作',
  in_use: '正在使用中，本次跳过',
  probe_unknown: '无法确认是否在使用，按占用处理',
  needs_reanalysis: '需要重新分析：占用与版本检查只在本次运行的分析中有效',
};

export const REGISTRY_LABEL: Record<string, string> = {
  covered: '已安装版本在实测范围内',
  version_untested: '已安装版本未经实测，因此只能审阅',
  not_registered: '没有对应的实测登记',
  not_checked: '未检查实测登记',
};

export const PROBE_STATE_LABEL: Record<string, string> = {
  in_use: '正在使用',
  not_in_use: '未在使用',
  unknown: '无法确认（按占用处理）',
};

export const PROBE_LABEL: Record<string, string> = {
  owner_process: '所属应用进程',
  open_handles: '打开的文件句柄',
};

export const AUTH_LABEL: Record<AuthStatus, string> = {
  authorized: '已授权',
  required: '等待你批准',
  expired: '已过期',
  revoked: '已撤销',
};

export const STALE_LABEL: Record<string, string> = {
  expired: '计划已过有效期（15 分钟）。请从候选重新生成计划。',
  rule_set_changed: '规则包已更新，旧计划不会按新规则执行。请重新分析并生成新计划。',
  digest_mismatch: '存储的计划内容与摘要不符，拒绝使用。',
  rules_unavailable: '规则包当前无法加载，计划不能执行。',
};

export const OPERATION_STATE_LABEL: Record<string, string> = {
  created: '准备中',
  validating: '复核中',
  running: '执行中',
  stopping: '正在停止',
  completed: '全部完成',
  partial: '部分完成',
  cancelled: '已停止',
  failed: '失败',
  reconciliation_required: '需要核对',
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
  succeeded: '成功',
  skipped: '已跳过',
  failed: '失败',
  not_started: '未开始',
  unknown: '结果未知',
  in_flight: '进行中',
  pending: '等待',
};

/** Result codes explained for people. Unknown codes fall back to the host's own message. */
export const CODE_EXPLAIN: Record<string, string> = {
  done: '已按计划完成。',
  protected: '执行前复查发现它受保护，未触碰。',
  precondition_missing: '对象已不在原位置，未做任何操作。',
  precondition_link_or_reparse: '对象变成了链接或重分析点，未跟随也未操作。',
  precondition_identity_changed: '同名位置上已是另一个文件，未操作。',
  precondition_parent_changed: '上级文件夹被替换，未操作。',
  precondition_metadata_changed: '文件在计划后被修改过，未操作。',
  precondition_in_use: '文件正被占用，未操作。',
  precondition_owner_running: '所属应用正在运行，未操作。关闭应用后可重新分析。',
  precondition_unsupported: '此平台或文件系统不支持该操作，未操作。',
  quarantine_unavailable: '同一卷上没有可用的隔离位置，未操作。',
  cancelled: '你停止了后续操作，此项未开始。',
  plan_expired: '执行途中计划到期，此项未开始。',
  authorization_revoked: '授权已撤销，此项未开始。',
  outcome_unconfirmed: '系统调用的结果无法确认，可能已经生效。请先核对，不会自动重试。',
  reconciled_performed: '核对确认：该动作已经生效。',
  reconciled_not_performed: '核对确认：对象仍在原处，动作没有发生。',
  never_started: '没有开始过该动作。',
  restored: '已移回原位置。',
  purged: '已永久删除。',
  not_recoverable: '该隔离项已被其他计划恢复或清空，跳过。',
  target_conflict: '原位置已有同名对象，按规则跳过，不覆盖。',
  missing: '隔离区中找不到该对象。',
  // Uninstall (UNI-02/03/06/09)
  not_running: '应用没有在运行。',
  closed: '收到退出请求后，应用已自行关闭。',
  still_running: '发出退出请求后应用仍在运行：卸载程序没有运行，也没有删除任何东西。请先关闭它再执行；强制结束只能在终端中单独审阅。',
  app_still_running: '应用没有退出，卸载程序没有运行，也没有删除任何东西。',
  running_state_unknown: '无法判断应用是否仍在运行，为安全起见没有卸载。',
  leftovers_remain: '卸载程序已运行，但重新扫描发现安装目录或卸载登记仍在，结果记为“部分完成”。下一步：审阅残留。',
  restart_required: '卸载程序要求重启后才能完成；重启后再审阅残留。',
  cancelled_by_user: '你在卸载程序中取消了卸载。',
  uninstaller_failed: '卸载程序报告失败（见退出码），应用可能仍然安装着。',
  record_changed: '卸载登记在计划之后发生了变化，没有运行卸载程序。',
  program_changed: '卸载程序文件在计划之后被替换或修改，没有运行。',
  start_failed: '卸载程序无法启动，没有运行任何东西。',
  backup_failed: '注册表项的备份未能保存，因此没有删除该项。',
  precondition_changed: '对象在计划之后发生了变化，没有改动它。',
  permission_required: '需要管理员权限，已跳过。',
  destination_exists: '目标已被重新创建，按规则跳过，不覆盖。',
  // Updates (UPD-02/03)
  updated: '已更新，并回读确认了新版本。',
  app_running: '应用正在运行，已跳过。请先关闭它，再执行一份新的更新计划；CleanManager 不会为了更新而关闭应用。',
  running_unknown: '无法判断应用是否在运行，已跳过。请先关闭它再执行。',
  signature_mismatch: '下载包的签名者与已安装应用不一致：拒绝更新，原应用没有改变。',
  unverifiable: '下载包无法验证签名：没有安装，原应用没有改变。请前往官方页面更新。',
  download_failed: '安装包未能下载以供验证，没有更新。',
  tool_changed: '包管理器程序在计划之后发生了变化，没有运行。请重新生成计划。',
  target_changed: '更新目标与审阅的计划不一致，没有运行。',
  adapter_mismatch: '更新命令不是审阅时的固定形式，没有运行。',
  tool_failed: '包管理器报告失败。',
  // Processes (STS-04)
  ended: '进程已结束；其中未保存的工作已丢失。',
  precondition_gone: '进程在执行前已经退出，没有结束任何东西。',
  end_failed: '结束进程失败。',
};

export function explainCode(code: string | null, message: string | null): string {
  if (code && CODE_EXPLAIN[code]) return CODE_EXPLAIN[code];
  return message ?? '';
}

export const AVAILABILITY_LABEL: Record<Availability, string> = {
  in_app_restore: '可在隔离区恢复',
  system_recycle_bin: '在系统回收站中还原',
  unavailable: '无法撤销',
  not_applicable: '无需恢复',
  unknown: '恢复情况未知，需要核对',
};

export const ACTION_LABEL: Record<string, string> = {
  trash: '移到回收站',
  quarantine: '移入隔离区',
  delete: '永久删除',
  restore: '从隔离区恢复',
  purge: '永久清空隔离项',
  startup: '修改启动项',
  maintenance: '运行维护任务',
  uninstall: '卸载应用',
  uninstall_leftovers: '处理卸载残留',
  uninstall_restore: '还原注册表项',
  update: '更新应用',
  process: '结束进程',
};

export const QUARANTINE_STATE_LABEL: Record<string, string> = {
  recoverable: '可恢复',
  target_conflict: '原位置已被占用',
  volume_offline: '所在卷离线',
  volume_changed: '卷已更换',
  identity_changed: '身份不符',
  missing: '内容缺失',
  parent_changed: '原上级文件夹已变化',
  manifest_incomplete: '记录不完整，无法验证恢复',
  unknown: '状态未知',
};

export const POLICY_STATE_LABEL: Record<PolicyState, string> = {
  enabled: '已启用',
  paused: '已暂停',
  revoked: '已撤销',
};

export const POLICY_MISMATCH_LABEL: Record<string, string> = {
  not_enabled: '策略未启用',
  expired: '策略已到期',
  other_principal: '策略属于另一个用户',
  scope_differs: '扫描范围不同',
  mode_differs: '处理方式不同',
  rule_not_covered: '包含策略未列出的规则版本',
  tier_not_default_selected: '包含需审阅的项目（策略只覆盖低风险项）',
  size_unknown: '总大小未知',
  single_limit: '超过单次上限',
  quota_exceeded: '超过 24 小时额度',
};

export const LAYER_LABEL: Record<string, string> = {
  hard_deny: '硬拒绝（程序内置）',
  default_category: '默认保护类目',
  user: '你的保护项',
};

export const MATCH_LABEL: Record<string, string> = {
  exact: '仅此对象',
  descendants: '此位置及其内容',
  ancestors: '此位置及上级',
  glob: '名称模式',
  rule: '整条规则',
  maintenance_task: '维护任务',
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
  if (s < 60) return `${s} 秒`;
  return `${Math.floor(s / 60)} 分 ${String(s % 60).padStart(2, '0')} 秒`;
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

// Platform idioms: the bin and file-manager words follow the platform table (GUI-12). The keys
// (`trash`, `system_recycle_bin`) and what they mean stay identical on every platform.
onPlatform(() => {
  const w = words();
  MODE_LABEL.trash = w.moveToBin;
  ACTION_LABEL.trash = w.moveToBin;
  MODE_EXPLAIN.trash = {
    effect: `交给系统${w.bin}。`,
    space: `${w.bin}仍占用这个卷的空间，清空${w.bin}后才释放。`,
    recovery: `${w.restoreFromBin}；CleanManager 无法替你还原。`,
  };
  MODE_EXPLAIN.delete = { ...MODE_EXPLAIN.delete, recovery: `无法撤销，没有${w.bin}或隔离副本。` };
  AVAILABILITY_LABEL.system_recycle_bin = `在系统${w.bin}中还原`;
});
