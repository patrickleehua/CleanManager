// User-facing wording for states, kinds and times. One vocabulary across pages.

import type { Entry, ScanState } from './types';

export const STATE_LABEL: Record<ScanState, string> = {
  created: '准备中',
  running: '正在扫描',
  stopping: '正在停止',
  completed: '已完成',
  partial: '部分完成',
  cancelled: '已取消',
  failed: '失败',
  interrupted: '已中断',
};

export const STATE_EXPLAIN: Record<ScanState, string> = {
  created: '扫描任务已创建，正在打开扫描根目录。',
  running: '正在读取文件元数据。总量在枚举结束前无法得知，因此不显示完成百分比。',
  stopping: '已请求停止：正在处理完当前目录后安全停止，已确认的结果会保留。',
  completed: '在本次声明的范围内读取完成。这只代表所选文件夹，不代表整台设备。',
  partial: '扫描结束，但有部分范围无法读取。已知结果可以浏览，缺口单独列出，不按 0 计算。',
  cancelled: '扫描在完成前被取消。取消前已确认的结果已保留，但不视为完整结果。',
  failed: '扫描未能完成。已确认的结果（如有）仍可查看，原因见下方问题列表。',
  interrupted: '运行扫描的进程在完成前退出。已提交的部分已保留并标为中断，不视为完成。',
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
  verified_scope: '声明范围内完整',
  partial_scope: '仅部分覆盖',
};

export function kindLabel(e: Pick<Entry, 'kind' | 'boundary'>): string {
  if (e.boundary) {
    return (
      {
        symlink: '符号链接（未跟随）',
        junction: '目录联接（未跟随）',
        mount_point: '挂载点（未跟随）',
        other_volume: '其他卷（未跟随）',
        cloud_provider: '云端文件夹（未下载）',
        own_store: 'CleanManager 自身数据（未扫描）',
      } as const
    )[e.boundary];
  }
  return ({ file: '文件', directory: '文件夹', symlink: '符号链接', reparse: '重分析点', other: '其他' } as const)[e.kind];
}

export function basisLabel(e: Pick<Entry, 'size_basis' | 'kind'>): string {
  switch (e.size_basis) {
    case 'entity':
      return '文件自身逻辑大小';
    case 'subtree_known':
      return '子项已知逻辑大小合计（硬链接按对象去重）';
    default:
      return '大小未知';
  }
}

const pad = (n: number) => String(n).padStart(2, '0');

/** RFC 3339 UTC → local "YYYY-MM-DD HH:MM". */
export function formatTime(iso: string | null | undefined): string {
  if (!iso) return '—';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

export function formatElapsed(fromIso: string | null | undefined, toIso?: string | null): string {
  if (!fromIso) return '—';
  const a = new Date(fromIso).getTime();
  const b = toIso ? new Date(toIso).getTime() : Date.now();
  if (Number.isNaN(a) || Number.isNaN(b)) return '—';
  const s = Math.max(0, Math.round((b - a) / 1000));
  if (s < 60) return `${s} 秒`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} 分 ${s % 60} 秒`;
  return `${Math.floor(m / 60)} 小时 ${m % 60} 分`;
}

export const ERROR_HINT: Record<string, string> = {
  store_inside_scope: '此文件夹包含 CleanManager 自己的索引。请改选其中的子文件夹（如“下载”“文档”）。',
  root_missing: '文件夹不存在或所在的卷已离线。请重新选择或连接后重新检查。',
  not_a_directory: '这不是文件夹。',
  permission_required: '以普通权限无法读取此位置。CleanManager 不会自动提权。',
  scan_running: '已有扫描在运行。等待它结束或先取消它。',
  unknown_root: '此位置已失效，请重新选择。',
  not_published: '扫描尚未发布结果。扫描结束后才能浏览，不读取未发布的暂存数据。',
  cursor_stale: '结果版本已变化，已从第一页重新加载。',
  index_corrupt: '扫描索引已损坏，需要重建。操作与恢复记录不受影响。',
  index_incompatible: '扫描索引版本不兼容，需要重建。操作与恢复记录不受影响。',
  not_found: '找不到这条记录。它可能来自已被清理的旧索引。',
  root_inside_store: '这是 CleanManager 自己的数据目录，不能作为扫描范围。',
  rules_unavailable: '规则包无法加载，暂时不能分析或执行。设置页列出了规则包位置和原因。',
  not_selectable: '所选项目中有不能放入计划的对象，请刷新候选后重新选择。',
  plan_refused: '核心拒绝生成这份计划。请重新分析后再选择。',
  needs_approval: '这份计划还没有授权。请先在本页批准，或在终端运行 plan review。',
  plan_stale: '计划已失效（过期或规则已变化），不能执行。请从候选重新生成。',
  plan_changed: '存储的计划与审阅时的内容不一致，已拒绝批准。请重新打开计划。',
  review_expired: '审阅会话已失效，请重新打开计划后再确认。',
  confirmation_mismatch: '输入的内容不匹配。请对照上方显示的内容重新输入。',
  operation_running: '另一个操作仍在执行。等待它结束或先停止它。',
  operation_conflict: '计划中的对象正被另一个操作占用（可能有待核对的结果）。',
  policy_denied: '策略额度不足或不再覆盖此计划。',
  policy_revoked: '已撤销的策略不能再改变状态；需要时请新建。',
  not_recoverable: '所选项中有已恢复、已清空或当前不可恢复的对象。',
  manifest_incomplete: '隔离记录不完整，无法验证恢复条件。',
  protection_unavailable: '保护清单无法读取。为安全起见，所有写操作已停止，原文件保持不变以便修复。',
  protection_rejected: '这个位置不能作为保护项（例如属于系统硬拒绝范围）。',
  protection_busy: '另一个程序正在写保护清单，请稍后重试。',
  not_removable: '内置的硬拒绝与默认保护类目不能移除。',
  unsupported: '此平台的这项能力尚未经过验证，未执行任何操作。',
};

/** Short task title: "正在扫描", "正在停止扫描", "扫描已完成"… */
export function taskTitle(s: ScanState): string {
  switch (s) {
    case 'created':
      return '扫描准备中';
    case 'running':
      return '正在扫描';
    case 'stopping':
      return '正在停止扫描';
    default:
      return `扫描${STATE_LABEL[s]}`;
  }
}
