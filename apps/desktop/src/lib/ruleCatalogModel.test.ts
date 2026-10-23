// 清理 › 清理目录 (WFA-49): filtering, grouping and words of the read-only cleanup catalog.
import { afterEach, describe, expect, it } from 'vitest';
import {
  APPLICABILITY_LABEL,
  APPLICABILITY_REASON,
  CATEGORY_LABEL,
  NO_FILTER,
  canShowCandidates,
  catalogCounts,
  categoryLabel,
  emptyCategories,
  estimateText,
  filterCatalog,
  isFiltered,
  reasonText,
  recoveryText,
} from './ruleCatalogModel';
import { GATED_PAGES, toolOf } from './nav';
import { mockOps } from './mockOps';
import type { RuleCatalog, RuleCatalogEntry } from './types';
import { setLocale } from '../i18n';
import coreCatalog from '../../../../crates/cm-core/src/catalog.rs?raw';
import appCatalog from '../../../../crates/cm-app/src/clean_catalog.rs?raw';

afterEach(() => setLocale('zh-CN'));

function entry(id: string, over: Partial<RuleCatalogEntry> = {}): RuleCatalogEntry {
  return {
    rule_id: id,
    version: '1.0.0',
    status: 'enabled',
    max_tier: 'review_only',
    effective_tier_ceiling: 'review_only',
    rebuild_class: 'local_rebuild',
    action_kind: 'filesystem',
    adapter_ref: null,
    owner_id: id.split('.')[0],
    installed_version: null,
    registry_status: 'covered',
    tested_range: null,
    tested_at: null,
    evidence_refs: [],
    summary: `${id} cache.`,
    pattern: null,
    allowed_modes: ['trash', 'quarantine'],
    requires_admin: false,
    regeneration: '',
    offline_effect: '',
    locations: [{ display: `%USERPROFILE%\\AppData\\Local\\${id}`, state: 'present' }],
    applicability: 'applicable',
    applicability_reason: 'present',
    estimate: null,
    ...over,
  };
}

const CATALOG: RuleCatalog = {
  platform: 'windows',
  catalog_version: 'catalog/1',
  rule_set_digest: 'd',
  estimate_source: { scan_id: 's1', candidate_set_id: 'cs1', analyzed_at: '2026-10-23T08:00:00Z', scan_root_display: 'C:\\Users\\Lin', rules_current: true },
  categories: [
    {
      category: 'browsers',
      label: 'Browsers',
      wave: 1,
      status: 'available',
      rules: [
        entry('google.chrome.code-cache', { effective_tier_ceiling: 'default_selected', estimate: { objects: 2, known_bytes: '1048576', unknown_size_count: 0 } }),
        entry('mozilla.firefox.cache2', { locations: [], applicability: 'not_present', applicability_reason: 'owner_not_found' }),
      ],
    },
    { category: 'developer', label: 'Developer tools', wave: 1, status: 'available', rules: [entry('npm.cache', { locations: [{ display: '%USERPROFILE%\\AppData\\Local\\npm-cache\\_cacache', state: 'present' }] })] },
    { category: 'trash', label: 'Recycle Bin / Trash', wave: 1, status: 'unsupported', rules: [] },
  ],
};

const ids = (c: RuleCatalog, f = NO_FILTER) => filterCatalog(c, f).flatMap((g) => g.rules.map((r) => r.rule_id));

describe('WFA-49 cleanup catalog view model', () => {
  it('groups by category in catalog order and lists categories without rules separately', () => {
    expect(filterCatalog(CATALOG, NO_FILTER).map((g) => g.category.category)).toEqual(['browsers', 'developer']);
    expect(emptyCategories(CATALOG).map((c) => c.category)).toEqual(['trash']);
    expect(catalogCounts(CATALOG)).toEqual({ rules: 3, applicable: 2, categoriesWithRules: 2, withEstimate: 1 });
    expect(isFiltered(NO_FILTER)).toBe(false);
  });

  it('filters by category, risk ceiling and applicability; every filter combines', () => {
    expect(ids(CATALOG, { ...NO_FILTER, category: 'developer' })).toEqual(['npm.cache']);
    expect(ids(CATALOG, { ...NO_FILTER, risk: 'default_selected' })).toEqual(['google.chrome.code-cache']);
    expect(ids(CATALOG, { ...NO_FILTER, applicability: 'not_present' })).toEqual(['mozilla.firefox.cache2']);
    expect(ids(CATALOG, { ...NO_FILTER, category: 'developer', risk: 'default_selected' })).toEqual([]);
    expect(isFiltered({ ...NO_FILTER, risk: 'blocked' })).toBe(true);
  });

  it('searches rule ids, owners, sentences, locations and the category in words, every word must match', () => {
    expect(ids(CATALOG, { ...NO_FILTER, query: 'npm-cache' })).toEqual(['npm.cache']);
    expect(ids(CATALOG, { ...NO_FILTER, query: 'FIREFOX' })).toEqual(['mozilla.firefox.cache2']);
    expect(ids(CATALOG, { ...NO_FILTER, query: '浏览器' })).toEqual(['google.chrome.code-cache', 'mozilla.firefox.cache2']);
    expect(ids(CATALOG, { ...NO_FILTER, query: 'chrome  cache' })).toEqual(['google.chrome.code-cache']);
    expect(ids(CATALOG, { ...NO_FILTER, query: 'chrome npm' })).toEqual([]);
    setLocale('en');
    expect(ids(CATALOG, { ...NO_FILTER, query: 'developer tools' })).toEqual(['npm.cache']);
  });

  it('a size is the stored estimate or "unknown", never 0', () => {
    const [chrome, firefox] = CATALOG.categories[0].rules;
    expect(estimateText(chrome)).toBe('1.0 MiB');
    expect(estimateText(firefox)).toBe('未知');
    expect(estimateText({ estimate: { objects: 1, known_bytes: '0', unknown_size_count: 2 } })).toBe('未知');
    expect(estimateText({ estimate: { objects: 1, known_bytes: '2048', unknown_size_count: 1 } })).toContain('至少');
    setLocale('en');
    expect(estimateText(firefox)).toBe('Unknown');
  });

  it('links to the candidates only for the same scan and only when the stored analysis found some', () => {
    const [chrome, firefox] = CATALOG.categories[0].rules;
    expect(canShowCandidates(chrome, CATALOG, 's1')).toBe(true);
    expect(canShowCandidates(chrome, CATALOG, 's2')).toBe(false);
    expect(canShowCandidates(chrome, CATALOG, null)).toBe(false);
    expect(canShowCandidates(firefox, CATALOG, 's1')).toBe(false);
    expect(canShowCandidates(chrome, { estimate_source: null }, 's1')).toBe(false);
  });

  it('recovery reads the allowed modes, or the owning tool, or that nothing is offered', () => {
    expect(recoveryText(entry('a'))).toBe('移到回收站、移入同卷隔离区');
    expect(recoveryText(entry('a', { action_kind: 'tool_adapter', adapter_ref: 'go.build-cache-clean', allowed_modes: [] }))).toContain('go.build-cache-clean');
    expect(recoveryText(entry('a', { action_kind: 'review_only_row', allowed_modes: [] }))).toBe('不提供处理方式');
  });

  it('every applicability reason the shared service can give reads in words in both languages', () => {
    const body = appCatalog.slice(appCatalog.indexOf('pub fn applicability('), appCatalog.indexOf('/// The stored size estimate'));
    const codes = [...body.matchAll(/Applicability::\w+,\s*"([a-z_]+)"/g)].map((m) => m[1]);
    expect(codes.length).toBeGreaterThanOrEqual(8);
    for (const lang of ['zh-CN', 'en'] as const) {
      setLocale(lang);
      for (const c of codes) expect(Object.prototype.hasOwnProperty.call(APPLICABILITY_REASON, c), c).toBe(true);
      for (const c of codes) expect(reasonText(c), c).not.toContain(c);
      expect(Object.values(APPLICABILITY_LABEL).every((v) => v.length > 0)).toBe(true);
    }
    expect(reasonText('brand_new')).toContain('brand_new');
  });

  it('every shared category of the core has a name in words', () => {
    const core = [...coreCatalog.matchAll(/CategoryInfo \{ id: "([a-z_]+)"/g)].map((m) => m[1]);
    expect(core.length).toBe(15);
    for (const id of core) expect(Object.prototype.hasOwnProperty.call(CATEGORY_LABEL, id), id).toBe(true);
    expect(categoryLabel({ category: 'browsers', label: 'Browsers' })).toBe('浏览器');
    expect(categoryLabel({ category: 'future', label: 'Future things' })).toBe('Future things');
  });

  it('is a section of the Clean tool behind the same gate as the CLI catalog', () => {
    expect(toolOf('clean').sections.map((s) => s.page)).toContain('rulecatalog');
    expect(GATED_PAGES.rulecatalog).toEqual({ capability: 'clean.catalog', gate: 'G1' });
  });

  it('the preview fixture keeps the host semantics: no size without candidates, sizes only for the analyzed scan', async () => {
    const ops = mockOps();
    const sized = await ops.cleanCatalog('s_preview_1');
    const all = sized.categories.flatMap((c) => c.rules);
    expect(all.length).toBeGreaterThan(10);
    expect(sized.estimate_source?.scan_id).toBe('s_preview_1');
    expect(all.some((e) => e.estimate)).toBe(true);
    expect(all.every((e) => !e.estimate || e.estimate.objects > 0)).toBe(true);
    expect(all.every((e) => e.locations.every((l) => !/Users\\Lin|\/Users\/lin/.test(l.display))), 'home is never spelled out').toBe(true);
    const other = await ops.cleanCatalog(null);
    expect(other.estimate_source).toBeNull();
    expect(other.categories.flatMap((c) => c.rules).every((e) => e.estimate === null)).toBe(true);
    // The link's filter: only that rule's candidates.
    const page = await ops.listCandidates('cs_preview', 'low', 50, null, 'google.chrome.code-cache');
    expect(page.items.map((c) => c.rules[0].split('@')[0])).toEqual(['google.chrome.code-cache']);
  });
});
