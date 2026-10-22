// DIN-05 "cleanable by hand" hints in the disk view. The host attaches a hint only to a listed
// directory whose signature it verified (valid CACHEDIR.TAG, a build artifact next to its project
// marker, a location of an enabled catalog cache rule) and that no protection covers. The hint is
// advice with an entry into a review flow: it never selects, never changes a tier and never
// overrides protection.

import { words } from './platform';
import type { CleanableHint, Entry } from './types';
import { t } from '../i18n';

/** Short word on the row marker. */
export const hintLabel = () => t('cleanable.hintLabel');

/** The verified evidence, in words. */
export function hintBasisText(h: CleanableHint): string {
  switch (h.basis) {
    case 'build_artifact':
      return t('cleanable.buildArtifactProjectDetail', { detail: h.detail });
    case 'catalog_rule':
      return h.rule_id ? t('cleanable.cacheLocationCoveredCleanup', { rule_id: h.rule_id }) : t('cleanable.cacheLocationCoveredCleanup2');
    case 'cachedir_tag':
      return t('cleanable.folderValidCachedirTag');
  }
}

/** Tooltip of the row marker. */
export function hintTitle(h: CleanableHint): string {
  return t('cleanable.hintLabelBasisH', { label: hintLabel(), h: hintBasisText(h) });
}

/** The review flow the detail pane offers. */
export function hintAction(h: CleanableHint): { flow: CleanableHint['flow']; label: string; note: string } {
  if (h.flow === 'purge') {
    return { flow: 'purge', label: t('cleanable.openBuildArtifactCleanup'), note: t('cleanable.scanProjectsBuildArtifacts') };
  }
  const w = words();
  return { flow: 'manual_trash', label: t('cleanable.movetobinFolder'), note: t('cleanable.opensMovetobinReviewItem', { moveToBin: w.moveToBin }) };
}

/** The hint a row shows: only real directories (never files, links or boundaries). */
export function hintOf(e: Entry): CleanableHint | null {
  if (e.kind !== 'directory' || e.boundary) return null;
  return e.cleanable_hint ?? null;
}
