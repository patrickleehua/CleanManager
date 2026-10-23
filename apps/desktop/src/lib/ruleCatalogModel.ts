// 清理 › 清理目录 (WFA-49): the view model of the read-only cleanup catalog — filtering by category, risk and
// applicability, text search, grouping by category, and the words for the host's codes. Pure functions over the
// host's `clean_catalog` answer (the same rows as `cleanmanager clean list`); nothing here selects or plans.

import { sizeLabel } from './display';
import type { Applicability, RuleCatalog, RuleCatalogCategory, RuleCatalogEntry, RuleLocation, RuleStatus, Tier } from './types';
import { MODE_LABEL } from './words';
import { t } from '../i18n';

export type RiskFilter = Tier | 'all';
export type ApplicabilityFilter = Applicability | 'all';

export interface CatalogFilter {
  /** A category id, or 'all'. */
  category: string;
  risk: RiskFilter;
  applicability: ApplicabilityFilter;
  query: string;
}

export const NO_FILTER: CatalogFilter = { category: 'all', risk: 'all', applicability: 'all', query: '' };

/** The 15 shared categories (cm-core catalog.rs `CATEGORIES`), in words. */
export const CATEGORY_LABEL: Record<string, string> = {
  get system_data() { return t('ruleCatalog.category.system_data'); },
  get user_essentials() { return t('ruleCatalog.category.user_essentials'); },
  get trash() { return t('ruleCatalog.category.trash'); },
  get app_caches() { return t('ruleCatalog.category.app_caches'); },
  get browsers() { return t('ruleCatalog.category.browsers'); },
  get cloud_office() { return t('ruleCatalog.category.cloud_office'); },
  get developer() { return t('ruleCatalog.category.developer'); },
  get ai_tools() { return t('ruleCatalog.category.ai_tools'); },
  get virtualization() { return t('ruleCatalog.category.virtualization'); },
  get logs() { return t('ruleCatalog.category.logs'); },
  get leftovers() { return t('ruleCatalog.category.leftovers'); },
  get firmware_backups() { return t('ruleCatalog.category.firmware_backups'); },
  get incomplete_downloads() { return t('ruleCatalog.category.incomplete_downloads'); },
  get large_items() { return t('ruleCatalog.category.large_items'); },
  get removable_metadata() { return t('ruleCatalog.category.removable_metadata'); },
};

/** A category's name in words; an id this build does not know reads as the host's label. */
export function categoryLabel(c: Pick<RuleCatalogCategory, 'category' | 'label'>): string {
  return Object.prototype.hasOwnProperty.call(CATEGORY_LABEL, c.category) ? CATEGORY_LABEL[c.category] : c.label;
}

export const APPLICABILITY_LABEL: Record<Applicability, string> = {
  get applicable() { return t('ruleCatalog.applicability.applicable'); },
  get not_present() { return t('ruleCatalog.applicability.not_present'); },
  get not_applicable() { return t('ruleCatalog.applicability.not_applicable'); },
  get unsupported() { return t('ruleCatalog.applicability.unsupported'); },
};

/** Pill tones: only "applies here" is highlighted; tone never carries meaning without the word. */
export const APPLICABILITY_TONE: Record<Applicability, 'good' | 'muted' | 'warn'> = { applicable: 'good', not_present: 'muted', not_applicable: 'muted', unsupported: 'muted' };

/** The reason codes of `cm_app::clean_catalog::applicability` (clean_catalog.rs), in words. */
export const APPLICABILITY_REASON: Record<string, string> = {
  get present() { return t('ruleCatalog.reason.present'); },
  get location_missing() { return t('ruleCatalog.reason.location_missing'); },
  get owner_not_found() { return t('ruleCatalog.reason.owner_not_found'); },
  get not_enabled() { return t('ruleCatalog.reason.not_enabled'); },
  get needs_volume() { return t('ruleCatalog.reason.needs_volume'); },
  get privacy_not_examined() { return t('ruleCatalog.reason.privacy_not_examined'); },
  get no_location() { return t('ruleCatalog.reason.no_location'); },
  get category_unsupported() { return t('ruleCatalog.reason.category_unsupported'); },
};

export function reasonText(code: string): string {
  return Object.prototype.hasOwnProperty.call(APPLICABILITY_REASON, code) ? APPLICABILITY_REASON[code] : t('ruleCatalog.reason.unknown', { code });
}

export const RULE_STATUS_LABEL: Record<RuleStatus, string> = {
  get experimental() { return t('ruleCatalog.status.experimental'); },
  get verified() { return t('ruleCatalog.status.verified'); },
  get enabled() { return t('ruleCatalog.status.enabled'); },
};

export const LOCATION_STATE_LABEL: Record<RuleLocation['state'], string> = {
  get present() { return t('ruleCatalog.location.present'); },
  get missing() { return t('ruleCatalog.location.missing'); },
  get not_examined() { return t('ruleCatalog.location.not_examined'); },
};

export const ACTION_KIND_LABEL: Record<RuleCatalogEntry['action_kind'], string> = {
  get filesystem() { return t('ruleCatalog.action.filesystem'); },
  get tool_adapter() { return t('ruleCatalog.action.tool_adapter'); },
  get review_only_row() { return t('ruleCatalog.action.review_only_row'); },
};

export const REBUILD_LABEL: Record<string, string> = {
  get local_rebuild() { return t('catalog.rebuildLabel.local_rebuild'); },
  get network_redownload() { return t('catalog.rebuildLabel.network_redownload'); },
  get not_rebuildable() { return t('catalog.rebuildLabel.not_rebuildable'); },
};

export const REGISTRY_TEXT: Record<RuleCatalogEntry['registry_status'], string> = {
  get covered() { return t('ruleCatalog.registry.covered'); },
  get version_untested() { return t('ruleCatalog.registry.version_untested'); },
  get not_registered() { return t('ruleCatalog.registry.not_registered'); },
};

/** How an object of this rule would be processed and got back: the modes it allows, or the owning tool / review only. */
export function recoveryText(e: Pick<RuleCatalogEntry, 'action_kind' | 'allowed_modes' | 'adapter_ref'>): string {
  if (e.action_kind === 'tool_adapter') return t('ruleCatalog.recovery.adapter', { adapter: e.adapter_ref ?? t('common.unknownLower') });
  if (e.action_kind === 'review_only_row' || !e.allowed_modes.length) return t('ruleCatalog.recovery.none');
  return e.allowed_modes.map((m) => MODE_LABEL[m]).join(t('common.listSep'));
}

/** Size from the stored analysis: "1.2 GiB", "至少 …" when part is unknown, "未知" without an estimate (never 0). */
export function estimateText(e: Pick<RuleCatalogEntry, 'estimate'>): string {
  if (!e.estimate) return t('common.unknown');
  return sizeLabel(e.estimate.known_bytes, { unknownCount: e.estimate.unknown_size_count });
}

const fold = (s: string) => s.toLocaleLowerCase();

/** Whether an entry of `category` passes the filter. The text search reads the rule id, owner, the rule's own sentence, the locations and pattern, and the category in words. */
export function matches(e: RuleCatalogEntry, category: RuleCatalogCategory, f: CatalogFilter): boolean {
  if (f.category !== 'all' && category.category !== f.category) return false;
  if (f.risk !== 'all' && e.effective_tier_ceiling !== f.risk) return false;
  if (f.applicability !== 'all' && e.applicability !== f.applicability) return false;
  const q = fold(f.query.trim());
  if (!q) return true;
  const hay = [e.rule_id, e.owner_id, e.summary, e.pattern ?? '', categoryLabel(category), category.category, ...e.locations.map((l) => l.display)].map(fold);
  return q.split(/\s+/).every((word) => hay.some((h) => h.includes(word)));
}

export interface CatalogGroup {
  category: RuleCatalogCategory;
  rules: RuleCatalogEntry[];
}

/** Categories with at least one matching rule, in catalog order; rules keep the host's order (by rule id). */
export function filterCatalog(c: RuleCatalog, f: CatalogFilter): CatalogGroup[] {
  return c.categories.map((category) => ({ category, rules: category.rules.filter((e) => matches(e, category, f)) })).filter((g) => g.rules.length > 0);
}

/** Categories that have no rule on this platform (listed once, quietly). */
export function emptyCategories(c: RuleCatalog): RuleCatalogCategory[] {
  return c.categories.filter((x) => x.rules.length === 0);
}

export function isFiltered(f: CatalogFilter): boolean {
  return f.category !== 'all' || f.risk !== 'all' || f.applicability !== 'all' || f.query.trim() !== '';
}

export interface CatalogCounts {
  rules: number;
  applicable: number;
  categoriesWithRules: number;
  withEstimate: number;
}

export function catalogCounts(c: RuleCatalog): CatalogCounts {
  const all = c.categories.flatMap((x) => x.rules);
  return {
    rules: all.length,
    applicable: all.filter((e) => e.applicability === 'applicable').length,
    categoriesWithRules: c.categories.filter((x) => x.rules.length > 0).length,
    withEstimate: all.filter((e) => e.estimate !== null).length,
  };
}

/**
 * The cleanup candidates can be opened focused on this rule only when the stored analysis the sizes come from belongs
 * to the scan the cleanup page shows and it found candidates of this rule.
 */
export function canShowCandidates(e: Pick<RuleCatalogEntry, 'estimate'>, c: Pick<RuleCatalog, 'estimate_source'>, scanId: string | null): boolean {
  return !!scanId && !!e.estimate && e.estimate.objects > 0 && c.estimate_source?.scan_id === scanId;
}

/** A DOM id fragment for a rule id (`google.chrome.code-cache` → `google-chrome-code-cache`). */
export function domId(ruleId: string): string {
  return ruleId.replace(/[^a-z0-9]+/gi, '-');
}
