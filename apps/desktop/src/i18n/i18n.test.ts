// LIF-05 guard: the interface text lives in the two resources, they have the same keys and placeholders, no value is
// empty, and no page writes a Chinese literal of its own. Plus the behaviour of `t`, the platform readings and the
// language switch.
import ts from 'typescript';
import { afterEach, describe, expect, it } from 'vitest';
import { RESOURCES, format, isResourceText, messageParams, onLocale, resolveLocale, setLocale, storedLanguage, storeLanguage, systemLocale, t, translate, type MessageKey } from '.';
import { zhCN } from './zh-CN';
import { en } from './en';
import { live, refreshLive } from '../lib/dom';
import { PLATFORM_WORDS, setPlatform, words } from '../lib/platform';
import { TOOLS, EXTENSIONS } from '../lib/nav';
import { ERROR_HINT, formatTime } from '../lib/format';
import { ISSUE_TEXT, explainIssue, refusalKnown, refusalText } from '../lib/codes';
import { MODE_LABEL, PLAN_KIND_LABEL } from '../lib/words';
import { formatBytes, formatExactBytes, groupDigits } from '../lib/bytes';
import { CLEANUP_TEXT } from '../views/cleanup';

afterEach(() => {
  setLocale('zh-CN');
  setPlatform('windows');
});

/** Sources that must not carry interface text of their own (resources and tests aside; mock data is host data). */
const SOURCES = import.meta.glob(['../main.ts', '../views/**/*.ts', '../lib/**/*.ts', '!../**/*.test.ts', '!../lib/mock*.ts'], { query: '?raw', import: 'default', eager: true }) as Record<string, string>;
/** Every source that may name a key (to find keys nobody uses). */
const ALL_SOURCES = import.meta.glob(['../main.ts', '../views/**/*.ts', '../lib/**/*.ts', '!../**/*.test.ts'], { query: '?raw', import: 'default', eager: true }) as Record<string, string>;

const HAN = /[\u4e00-\u9fff]/;
const FULL_WIDTH = /[\u3000-\u303f\uff00-\uffef]/;

/** Chinese (or full-width punctuation) inside string and template literals; comments and regular expressions are not literals. */
function literalHits(file: string, text: string): string[] {
  const sf = ts.createSourceFile(file, text, ts.ScriptTarget.ES2022, true, ts.ScriptKind.TS);
  const hits: string[] = [];
  const check = (node: ts.Node, s: string) => {
    if (HAN.test(s) || FULL_WIDTH.test(s)) hits.push(`${file}:${sf.getLineAndCharacterOfPosition(node.getStart(sf)).line + 1} ${node.getText(sf).slice(0, 60)}`);
  };
  const visit = (node: ts.Node) => {
    if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) check(node, node.text);
    else if (ts.isTemplateExpression(node)) check(node, [node.head.text, ...node.templateSpans.map((s) => s.literal.text)].join(''));
    ts.forEachChild(node, visit);
  };
  visit(sf);
  return hits;
}

describe('LIF-05 resources', () => {
  const zhKeys = Object.keys(zhCN).sort();
  const enKeys = Object.keys(en).sort();

  it('every key exists in both languages', () => {
    expect(zhKeys.filter((k) => !(k in en)), 'missing in en.ts').toEqual([]);
    expect(enKeys.filter((k) => !(k in zhCN)), 'missing in zh-CN.ts').toEqual([]);
    expect(zhKeys.length).toBeGreaterThan(2500);
  });

  it('no value is empty', () => {
    for (const l of ['zh-CN', 'en'] as const) {
      const empty = Object.entries(RESOURCES[l]).filter(([, v]) => typeof v !== 'string' || v.trim() === '').map(([k]) => k);
      expect(empty, l).toEqual([]);
    }
  });

  it('both languages fill in the same placeholders, and every message parses', () => {
    const differ = zhKeys.filter((k) => messageParams(zhCN[k as MessageKey]).join() !== messageParams(en[k as MessageKey]).join());
    expect(differ).toEqual([]);
  });

  it('English has no Chinese (the language names in the picker excepted), Chinese keeps its own', () => {
    const own = new Set<string>(['settings.language.zhCN']);
    expect(enKeys.filter((k) => !own.has(k) && HAN.test(en[k as MessageKey]))).toEqual([]);
    expect(enKeys.filter((k) => FULL_WIDTH.test(en[k as MessageKey]))).toEqual([]);
  });

  it('a macOS reading always has the key it reads', () => {
    expect(zhKeys.filter((k) => k.endsWith('@macos') && !(k.slice(0, -'@macos'.length) in zhCN))).toEqual([]);
  });

  it('no page, shell or word table writes Chinese text outside the resources', () => {
    const hits = Object.entries(SOURCES).flatMap(([file, text]) => literalHits(file, text));
    expect(Object.keys(SOURCES).length).toBeGreaterThan(50);
    expect(Object.keys(SOURCES).some((f) => f.endsWith('/main.ts'))).toBe(true);
    expect(hits).toEqual([]);
  });

  it('the scan finds a Chinese literal when there is one (and ignores comments)', () => {
    expect(literalHits('x.ts', "// 注释\nconst a = '清理';\nconst b = `共 ${n} 项`;\nconst c = /回收站/;")).toHaveLength(2);
  });

  it('every key is used somewhere', () => {
    const all = Object.values(ALL_SOURCES).join('\n');
    // Keys built from a fixed list in platform.ts.
    const dynamic = ['platform.kindExplain.', 'platform.offDoes.'];
    const unused = zhKeys.filter((k) => !k.endsWith('@macos') && !dynamic.some((p) => k.startsWith(p)) && !all.includes(`'${k}'`));
    expect(unused).toEqual([]);
  });
});

describe('t()', () => {
  it('fills in text, numbers and plural forms for the active language', () => {
    expect(format('扫描 {name}', { name: 'D:\\Downloads' }, 'zh-CN')).toBe('扫描 D:\\Downloads');
    expect(format('{n, number} 个对象', { n: 1234567 }, 'zh-CN')).toBe('1,234,567 个对象');
    expect(format('{n, number} 个对象', { n: 18446744073709551615n }, 'zh-CN')).toBe('18,446,744,073,709,551,615 个对象');
    const files = '{n, plural, =0 {No files} one {# file} other {# files}} in {dir}';
    expect(format(files, { n: 0, dir: 'Temp' }, 'en')).toBe('No files in Temp');
    expect(format(files, { n: 1, dir: 'Temp' }, 'en')).toBe('1 file in Temp');
    expect(format(files, { n: 1234, dir: 'Temp' }, 'en')).toBe('1,234 files in Temp');
    // A count that arrives as text still picks its form; anything else falls back to "other".
    expect(format(files, { n: '1', dir: 'x' }, 'en')).toBe('1 file in x');
    expect(format(files, { n: 'many', dir: 'x' }, 'en')).toBe('many files in x');
    // A value prints exactly like a template literal would.
    expect(format('{a}/{b}', { a: null, b: 3 }, 'en')).toBe('null/3');
  });

  it('reads the active language and switches without a restart', () => {
    expect(t('cleanup.title')).toBe('清理');
    expect(CLEANUP_TEXT.summary(1234)).toBe('按规则找到 1,234 个候选');
    setLocale('en');
    expect(t('cleanup.title')).toBe('Clean');
    expect(CLEANUP_TEXT.summary(1)).toBe('The rules found 1 candidate');
    expect(CLEANUP_TEXT.summary(1234)).toBe('The rules found 1,234 candidates');
    expect(TOOLS.map((x) => x.label)).toEqual(['Clean', 'Software', 'Optimize', 'Analyze', 'Status']);
    expect(EXTENSIONS.find((e) => e.id === 'settings')!.label).toBe('Settings');
    setLocale('zh-CN');
    expect(TOOLS.map((x) => x.label)).toEqual(['清理', '软件', '优化', '分析', '状态']);
  });

  it('tells its own words from a host message passed through as sent', () => {
    setLocale('en');
    expect(isResourceText(t('cleanup.noneTitle'))).toBe(true);
    expect(isResourceText(t('cleanup.summarySize', { size: '1.0 GiB' }))).toBe(true);
    expect(isResourceText('Access is denied. (os error 5)')).toBe(false);
  });
});

describe('platform × language', () => {
  it('every combination reads its own words', () => {
    const bins = (['zh-CN', 'en'] as const).flatMap((l) => (['windows', 'macos'] as const).map((p) => translate(l, p, 'platform.bin')));
    expect(bins).toEqual(['回收站', '废纸篓', 'Recycle Bin', 'Trash']);
    setLocale('en');
    expect(PLATFORM_WORDS.windows.fileManager).toBe('File Explorer');
    expect(PLATFORM_WORDS.macos.fileManager).toBe('Finder');
    setPlatform('macos');
    expect(words().moveToBin).toBe('Move to Trash');
    expect(MODE_LABEL.trash).toBe('Move to Trash');
    expect(PLAN_KIND_LABEL.manual_trash).toContain('Trash');
    expect(TOOLS[0].sections.find((s) => s.page === 'recyclebin')!.label).toBe('Trash');
    expect(ERROR_HINT.recycle_bin_unavailable).toContain('Trash');
    expect(refusalText({ reason: 'root_is_link' })).toContain('alias');
    setLocale('zh-CN');
    expect(words().moveToBin).toBe('移到废纸篓');
    expect(refusalText({ reason: 'root_is_link' })).toBe('所选路径是替身或符号链接，请直接选择它指向的文件夹。');
    setPlatform('windows');
    expect(words().moveToBin).toBe('移到回收站');
  });

  it('macOS English never borrows a Windows idiom', () => {
    setLocale('en');
    const windowsOnly = /Windows|Recycle Bin|File Explorer|Ctrl|Task Manager|notification area|\btray\b|Microsoft Store|Settings › Apps/;
    const texts: string[] = [];
    const walk = (v: unknown) => {
      if (typeof v === 'string') texts.push(v);
      else if (typeof v === 'function') texts.push(String((v as (n: number) => string)(3)));
      else if (Array.isArray(v)) v.forEach(walk);
      else if (v && typeof v === 'object') Object.values(v).forEach(walk);
    };
    Object.values(PLATFORM_WORDS.macos).forEach(walk);
    for (const s of texts) expect(s, s).not.toMatch(windowsOnly);
  });
});

describe('codes in English', () => {
  it('every issue and refusal code that has Chinese words has English words too', () => {
    setLocale('en');
    for (const [k, v] of [...Object.entries(ISSUE_TEXT), ...Object.entries(ERROR_HINT)]) {
      expect(v.length, k).toBeGreaterThan(4);
      expect(HAN.test(v), `${k}: ${v}`).toBe(false);
    }
    expect(refusalKnown('plan_stale')).toBe(true);
    const e = explainIssue({ code: 'quota_x', scope_display: 'D:\\', message: 'disk quota exceeded' });
    expect(e.text).toBe('Unrecognized issue (code quota_x)');
    expect(e.raw).toBe('disk quota exceeded');
  });
});

describe('formatting', () => {
  it('sizes keep binary units in both languages; counts and dates follow the language', () => {
    expect(formatBytes('1610612736')).toBe('1.5 GiB');
    expect(formatExactBytes('1610612736')).toBe('1,610,612,736 字节');
    expect(groupDigits(1234567n)).toBe('1,234,567');
    expect(formatTime('2026-10-22T06:05:00Z')).toMatch(/^2026-10-22 \d\d:05$/);
    setLocale('en');
    expect(formatBytes('1610612736')).toBe('1.5 GiB');
    expect(formatBytes(null)).toBe('Unknown');
    expect(formatExactBytes('1610612736')).toBe('1,610,612,736 bytes');
    expect(formatTime('2026-10-22T06:05:00Z')).toMatch(/^Oct 2[12], 2026, \d{1,2}:05\s?[AP]M$/);
  });
});

describe('the language setting', () => {
  it('follows the system by default: any Chinese → 简体中文, anything else → English', () => {
    expect(systemLocale('zh-CN')).toBe('zh-CN');
    expect(systemLocale('zh-Hans-SG')).toBe('zh-CN');
    expect(systemLocale('zh-TW')).toBe('zh-CN');
    expect(systemLocale('en-GB')).toBe('en');
    expect(systemLocale('fr-FR')).toBe('en');
    expect(systemLocale('')).toBe('en');
    expect(systemLocale(null)).toBe('en');
    expect(resolveLocale('system', 'zh-CN')).toBe('zh-CN');
    expect(resolveLocale('en', 'zh-CN')).toBe('en');
  });

  it('is kept next to the theme (cm.lang) and "system" removes it', () => {
    const mem = new Map<string, string>();
    const storage = { getItem: (k: string) => mem.get(k) ?? null, setItem: (k: string, v: string) => void mem.set(k, v), removeItem: (k: string) => void mem.delete(k) };
    expect(storedLanguage(storage)).toBe('system');
    storeLanguage('en', storage);
    expect(mem.get('cm.lang')).toBe('en');
    expect(storedLanguage(storage)).toBe('en');
    storeLanguage('system', storage);
    expect(mem.has('cm.lang')).toBe(false);
    mem.set('cm.lang', 'fr');
    expect(storedLanguage(storage)).toBe('system');
  });

  it('switching notifies the shell once and re-applies bound text; machine values stay as they are', () => {
    // A stand-in for an element built once (the shell, a page head): `live` keeps its text in the active language.
    const attrs = new Map<string, string>();
    const el = { textContent: '', setAttribute: (k: string, v: string) => void attrs.set(k, v), removeAttribute: (k: string) => void attrs.delete(k) } as unknown as Element;
    live(el, () => t('cleanup.title'), { 'aria-label': () => t('cleanup.candidates'), 'data-code': () => 'not_selectable' });
    expect(el.textContent).toBe('清理');
    const seen: string[] = [];
    const off = onLocale((l) => {
      seen.push(l);
      refreshLive();
    });
    setLocale('en');
    setLocale('en');
    expect(seen).toEqual(['en']);
    expect(el.textContent).toBe('Clean');
    expect(attrs.get('aria-label')).toBe('Cleanup candidates');
    expect(attrs.get('data-code')).toBe('not_selectable');
    setLocale('zh-CN');
    expect(el.textContent).toBe('清理');
    off();
    setLocale('en');
    expect(seen).toEqual(['en', 'zh-CN']);
  });
});
