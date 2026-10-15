// DIN-05 "cleanable by hand" hints in the disk view. The host attaches a hint only to a listed
// directory whose signature it verified (valid CACHEDIR.TAG, a build artifact next to its project
// marker, a location of an enabled catalog cache rule) and that no protection covers. The hint is
// advice with an entry into a review flow: it never selects, never changes a tier and never
// overrides protection.

import { words } from './platform';
import type { CleanableHint, Entry } from './types';

/** Short word on the row marker. */
export const HINT_LABEL = '可手动清理';

/** The verified evidence, in words. */
export function hintBasisText(h: CleanableHint): string {
  switch (h.basis) {
    case 'build_artifact':
      return `构建产物：所在项目有 ${h.detail}，可由项目重新生成`;
    case 'catalog_rule':
      return h.rule_id ? `清理规则 ${h.rule_id} 覆盖的缓存位置` : '清理规则覆盖的缓存位置';
    case 'cachedir_tag':
      return '文件夹内有有效的 CACHEDIR.TAG 缓存标记';
  }
}

/** Tooltip of the row marker. */
export function hintTitle(h: CleanableHint): string {
  return `${HINT_LABEL}（依据：${hintBasisText(h)}）。只是提示，不会替你选中。`;
}

/** The review flow the detail pane offers. */
export function hintAction(h: CleanableHint): { flow: CleanableHint['flow']; label: string; note: string } {
  if (h.flow === 'purge') {
    return { flow: 'purge', label: '打开构建产物清理', note: '在构建产物页扫描项目后逐项审阅，那里会再检查版本控制、保护与最近修改。' };
  }
  const w = words();
  return { flow: 'manual_trash', label: `将此文件夹${w.moveToBin}…`, note: `只针对这一项打开${w.moveToBin}的审阅，先逐项检查保护与占用，批准后才会移动。` };
}

/** The hint a row shows: only real directories (never files, links or boundaries). */
export function hintOf(e: Entry): CleanableHint | null {
  if (e.kind !== 'directory' || e.boundary) return null;
  return e.cleanable_hint ?? null;
}
