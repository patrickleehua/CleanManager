import { afterEach, describe, expect, it } from 'vitest';
import { hintLabel, hintAction, hintBasisText, hintOf, hintTitle } from './cleanable';
import { setPlatform } from './platform';
import type { CleanableHint, Entry } from './types';

const entry = (over: Partial<Entry> = {}): Entry => ({
  entry_id: '7',
  parent_entry_id: '1',
  name: 'node_modules',
  path_display: 'D:\\Work\\app\\node_modules',
  kind: 'directory',
  logical_bytes: '1024',
  allocated_bytes: '1024',
  size_basis: 'subtree_known',
  unknown_descendants: 0,
  boundary: null,
  identity_quality: 'strong',
  link_count: null,
  ...over,
});

const artifact: CleanableHint = { basis: 'build_artifact', rule_id: 'purge.node-modules', detail: 'package.json', flow: 'purge' };
const tag: CleanableHint = { basis: 'cachedir_tag', rule_id: null, detail: 'CACHEDIR.TAG', flow: 'manual_trash' };
const rule: CleanableHint = { basis: 'catalog_rule', rule_id: 'google.chrome.code-cache', detail: 'C:\\Users\\u\\AppData\\Local\\Google\\Chrome\\User Data\\Default\\Code Cache', flow: 'manual_trash' };

afterEach(() => setPlatform('windows'));

describe('DIN-05 cleanable directory hints', () => {
  it('every hint names its basis', () => {
    expect(hintBasisText(artifact)).toContain('package.json');
    expect(hintBasisText(tag)).toContain('CACHEDIR.TAG');
    expect(hintBasisText(rule)).toContain('google.chrome.code-cache');
    expect(hintTitle(tag)).toContain(hintLabel());
    expect(hintTitle(tag)).toContain('不会替你选中');
  });

  it('a build artifact leads to the build-artifact flow, a cache to the manual-bin review of this one folder', () => {
    expect(hintAction(artifact)).toMatchObject({ flow: 'purge', label: '打开构建产物清理' });
    expect(hintAction(tag)).toMatchObject({ flow: 'manual_trash', label: '将此文件夹移到回收站…' });
    expect(hintAction(rule).flow).toBe('manual_trash');
    setPlatform('macos');
    expect(hintAction(tag).label).toBe('将此文件夹移到废纸篓…');
  });

  it('only real directories carry a hint; absent, null and unconfirmed mean none', () => {
    expect(hintOf(entry({ cleanable_hint: artifact }))).toEqual(artifact);
    expect(hintOf(entry())).toBeNull();
    expect(hintOf(entry({ cleanable_hint: null }))).toBeNull();
    expect(hintOf(entry({ kind: 'file', cleanable_hint: tag }))).toBeNull();
    expect(hintOf(entry({ boundary: 'junction', cleanable_hint: tag }))).toBeNull();
  });

  it('a hint is advice only: it carries no selection, tier or protection field', () => {
    for (const h of [artifact, tag, rule]) expect(Object.keys(h).sort()).toEqual(['basis', 'detail', 'flow', 'rule_id']);
  });
});
