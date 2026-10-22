// P02 overview: which layout a scan state gets and what each layout shows. Kept pure so the state
// matrix is tested without a DOM. Rules:
// - running: progress and cancel only (sizes are not published yet);
// - completed: the space summary first;
// - partial: what was missed first, then the known part ("至少");
// - failed: the reason and the next step only — no coverage, no browse/candidates, no machine-wide cards;
// - cancelled / interrupted: how much was kept.

import { groupDigits, parseBytes } from './bytes';
import { explainIssue, ROOT_REFUSALS, type ExplainedIssue } from './codes';
import { sizeLabel, type BannerKind } from './display';
import { isTerminal } from './format';
import type { ScanStatus } from './types';

export type OverviewLayout = 'running' | 'completed' | 'partial' | 'failed' | 'cancelled' | 'interrupted';

export interface OverviewPlan {
  layout: OverviewLayout;
  banner: { kind: BannerKind; title: string; sentence: string };
  /** The known-space figure, or null when the layout does not show one. */
  space: { text: string; bytes: bigint | null; atLeast: boolean; label: string } | null;
  /** Places that were not read and sizes that are unknown (partial layouts lead with these). */
  gaps: { unread: number; unknownSizes: number } | null;
  /** The reason a failed scan stopped. */
  reason: ExplainedIssue | null;
  /** Next steps. `browse` names the button: browse the results, or what was kept before a stop. */
  browse: 'results' | 'kept' | null;
  candidates: boolean;
  cancel: boolean;
  /** Scan the same root again. */
  rescan: 'primary' | 'secondary' | null;
  /** Choose another root on the scope page (failed scans; other layouts keep "新建扫描" in the page head). */
  chooseRoot: 'primary' | 'secondary' | null;
  /** Machine-wide system and virtualization objects (not part of the scanned folder). */
  systemCards: boolean;
  /** Other scans and the capability details in the aside. */
  aside: boolean;
  /** A privacy prompt is still waiting for an answer (macOS). */
  pendingPermission: boolean;
}

function knownBytes(s: ScanStatus): bigint | null {
  const v = s.summary?.logical_known_bytes ?? null;
  try {
    return parseBytes(v);
  } catch {
    return null;
  }
}

/** The first issue that explains a failure: a root refusal if there is one, else the first issue. */
function failureReason(s: ScanStatus): ExplainedIssue | null {
  const all = s.issues.map(explainIssue);
  return all.find((e) => ROOT_REFUSALS.includes(e.code)) ?? all[0] ?? null;
}

export function overviewPlan(s: ScanStatus): OverviewPlan {
  const n = s.entries_seen;
  const base: OverviewPlan = {
    layout: 'running',
    banner: { kind: 'running', title: '', sentence: '' },
    space: null,
    gaps: null,
    reason: null,
    browse: null,
    candidates: false,
    cancel: false,
    rescan: null,
    chooseRoot: null,
    systemCards: false,
    aside: true,
    pendingPermission: false,
  };

  if (!isTerminal(s.state)) {
    const title = s.state === 'created' ? '正在准备扫描' : s.state === 'stopping' || s.cancel_requested ? '正在停止扫描' : '正在扫描';
    const sentence =
      s.state === 'created'
        ? '正在打开所选文件夹。'
        : s.state === 'stopping' || s.cancel_requested
          ? '处理完当前目录后停止，已确认的结果会保留。'
          : `已发现 ${groupDigits(n)} 个条目，切换页面不会中断扫描。`;
    return { ...base, layout: 'running', banner: { kind: 'running', title, sentence }, cancel: true };
  }

  const known = s.published ? knownBytes(s) : null;
  const unread = s.summary?.unread_ranges ?? s.skipped_ranges;
  const unknownTotal = s.summary?.logical_unknown_count ?? 0;
  const unknownSizes = Math.max(0, unknownTotal - unread);
  const hasResults = s.published && n > 0 && s.summary !== null;

  if (s.state === 'failed') {
    const reason = failureReason(s);
    const rootRefused = !!reason && (ROOT_REFUSALS.includes(reason.code) || (reason.code === 'permission_required' && n === 0));
    return {
      ...base,
      layout: 'failed',
      banner: { kind: 'failed', title: '扫描失败', sentence: reason?.text ?? '扫描没有完成，也没有记录原因。请重新扫描。' },
      reason,
      chooseRoot: rootRefused ? 'primary' : 'secondary',
      rescan: rootRefused ? null : 'primary',
      aside: false,
    };
  }

  if (s.state === 'cancelled' || s.state === 'interrupted') {
    const kept = hasResults ? n : 0;
    // The size is the banner's figure; the sentence says how many entries were kept.
    const keptText = kept > 0 ? `已保留 ${groupDigits(kept)} 个条目，结果不完整。` : '没有保留任何条目。';
    const cancelled = s.state === 'cancelled';
    return {
      ...base,
      layout: cancelled ? 'cancelled' : 'interrupted',
      banner: { kind: 'cancelled', title: cancelled ? '扫描已取消' : '扫描已中断', sentence: cancelled ? `取消前${keptText}` : `运行扫描的程序在完成前退出，${keptText}` },
      space: kept > 0 && known !== null ? { text: sizeLabel(known, { atLeast: true }), bytes: known, atLeast: true, label: '已保留的部分' } : null,
      browse: kept > 0 ? 'kept' : null,
      rescan: 'primary',
    };
  }

  // completed / partial
  const pending = s.issues.some((i) => explainIssue(i).cause === 'privacy_prompt_pending');
  const atLeast = unknownTotal > 0;
  const space = hasResults ? { text: sizeLabel(known, { atLeast }), bytes: known, atLeast, label: atLeast ? '已知部分的逻辑大小' : '逻辑大小' } : null;
  const results = {
    space,
    browse: hasResults ? ('results' as const) : null,
    candidates: hasResults,
    systemCards: true,
  };
  if (s.state === 'completed' && !pending && unread === 0) {
    return {
      ...base,
      ...results,
      layout: 'completed',
      banner: {
        kind: 'success',
        title: '扫描完成',
        sentence: unknownSizes > 0 ? `所选范围已读取完，其中 ${groupDigits(unknownSizes)} 个条目大小未知。` : `已读取所选范围内的 ${groupDigits(n)} 个条目。`,
      },
      gaps: unknownSizes > 0 ? { unread: 0, unknownSizes } : null,
    };
  }
  const title = pending ? '等待 macOS 授权' : unread > 0 ? `部分完成：${groupDigits(unread)} 处位置无法读取` : unknownSizes > 0 ? `部分完成：${groupDigits(unknownSizes)} 个条目大小未知` : '部分完成';
  return {
    ...base,
    ...results,
    layout: 'partial',
    banner: {
      kind: pending ? 'pending-permission' : 'partial',
      title,
      sentence: pending ? '在 macOS 弹出的对话框中选择后重新扫描；已读到的部分可以先浏览。' : '已读到的部分可以浏览；没读到的位置不按 0 计算。',
    },
    gaps: { unread, unknownSizes },
    rescan: pending ? 'primary' : 'secondary',
    pendingPermission: pending,
  };
}
