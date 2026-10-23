// WFA-65 / MTR-03: the bin page (清理 › 回收站). The host lists the current user's bin items read-only; some of them
// can be frozen into a separate plan of permanent deletes, which only a terminal review approves. Pure so the grouping,
// the one-bin rule and the refusal words are tested without a DOM.

import { words } from './platform';
import type { BinItem, BinList } from './types';
import { t } from '../i18n';

export interface BinGroup {
  bin_id: string;
  volume_display: string;
  /** Items of this bin on the page (newest first, as the host sent them). */
  items: BinItem[];
  /** Items of this bin the host counted (the page may show fewer). */
  item_count: number;
}

/** The items by bin (volume), in the host's bin order; items of a bin the host did not describe still get a group. */
export function binGroups(list: Pick<BinList, 'bins' | 'items'>): BinGroup[] {
  const groups = new Map<string, BinGroup>(list.bins.map((b) => [b.bin_id, { bin_id: b.bin_id, volume_display: b.volume_display, items: [], item_count: b.item_count }]));
  for (const i of list.items) {
    let g = groups.get(i.bin_id);
    if (!g) {
      g = { bin_id: i.bin_id, volume_display: '', items: [], item_count: 0 };
      groups.set(i.bin_id, g);
    }
    g.items.push(i);
  }
  return [...groups.values()].filter((g) => g.items.length || g.item_count);
}

/**
 * Whether an item can be ticked now: it must be purgeable, and a permanent-delete plan takes the items of one bin only
 * (the host refuses a mix with `bin_items_mixed`), so once something is ticked only its bin stays open.
 */
export function binSelectable(item: Pick<BinItem, 'purgeable' | 'bin_id'>, selectedBin: string | null): boolean {
  return item.purgeable && (selectedBin === null || selectedBin === item.bin_id);
}

/** Known wordings of the host's "cannot purge" detail (Windows recycle_bin.rs, macOS actions/trash.rs) → words. */
const REFUSALS: [RegExp, () => string][] = [
  // "hard links" first: it also contains the word "link".
  [/hard links/i, () => t('binModel.refusal.hardLinks')],
  [/cloud placeholder/i, () => t('binModel.refusal.cloud', { fileManager: words().fileManager, bin: words().bin })],
  [/could not be read/i, () => t('binModel.refusal.unread', { fileManager: words().fileManager, bin: words().bin })],
  [/unlock it/i, () => t('binModel.refusal.locked', { fileManager: words().fileManager })],
  [/\ba link\b|mount point|special object/i, () => t('binModel.refusal.special', { fileManager: words().fileManager, bin: words().bin })],
];

/** Why an item cannot be deleted permanently from here, in words (the host's detail stays for 技术详情). */
export function binRefusalText(refusal: string | null): string | null {
  if (!refusal) return null;
  const hit = REFUSALS.find(([re]) => re.test(refusal));
  return hit ? hit[1]() : t('binModel.refusal.other', { fileManager: words().fileManager, bin: words().bin });
}
