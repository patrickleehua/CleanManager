// Interface language (LIF-05). Every human sentence the pages show comes from the resources in this folder
// (zh-CN.ts, en.ts); protocol fields, codes and states never do. `t(key, params)` reads the active language and,
// for keys that have one, the macOS reading (`key@macos`) while the platform is macOS (GUI-12).
//
// Message syntax (a small subset of ICU MessageFormat):
//   {name}                                  the value as text (exactly like a template literal would print it)
//   {n, number}                             a number grouped for the active locale (Intl.NumberFormat)
//   {n, plural, one {# file} other {# files}}  a plural form chosen by Intl.PluralRules; `#` prints the number,
//                                           `=0 {…}` matches an exact value
// No new dependency: Intl does the locale work.

import { zhCN, type MessageKey } from './zh-CN';
import { en } from './en';

export type Locale = 'zh-CN' | 'en';
/** What the person chose in Settings: follow the system, or one language. */
export type LanguagePref = 'system' | Locale;
export type { MessageKey };
/** Keys a caller names; `@macos` readings are picked by `t` itself. */
export type TextKey = Exclude<MessageKey, `${string}@macos`>;
export type ParamValue = string | number | bigint | boolean | null | undefined;
export type Params = Record<string, ParamValue>;
export type TextVariant = 'windows' | 'macos';

export const LOCALES: readonly Locale[] = ['zh-CN', 'en'];
export const RESOURCES: Record<Locale, Readonly<Record<MessageKey, string>>> = { 'zh-CN': zhCN, en };

/** Stored next to the theme preference (`cm.theme`); absent = follow the system. */
export const LANGUAGE_KEY = 'cm.lang';

let current: Locale = 'zh-CN';
let variant: TextVariant = 'windows';
const listeners = new Set<(l: Locale) => void>();

/** The language the interface is shown in. */
export function locale(): Locale {
  return current;
}

/** The system UI language mapped onto the two resources: any Chinese → 简体中文, anything else → English. */
export function systemLocale(lang: string | null | undefined = globalThis.navigator?.language): Locale {
  return lang && /^zh\b/i.test(lang) ? 'zh-CN' : 'en';
}

/** The language a preference resolves to on this system. */
export function resolveLocale(pref: LanguagePref, system: string | null | undefined = globalThis.navigator?.language): Locale {
  return pref === 'system' ? systemLocale(system) : pref;
}

/** Switch the interface language; registered listeners (the shell) re-render. Sets `<html lang>`. */
export function setLocale(l: Locale): void {
  const changed = l !== current;
  current = l;
  if (typeof document !== 'undefined') document.documentElement.lang = l;
  if (changed) listeners.forEach((fn) => fn(l));
}

/** Run `fn` after every language switch; returns the unsubscribe function. */
export function onLocale(fn: (l: Locale) => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/** Platform reading of the resources (set by `setPlatform`). */
export function setTextVariant(v: TextVariant): void {
  variant = v;
}

export function textVariant(): TextVariant {
  return variant;
}

/** The stored preference (`system` when nothing or something unknown is stored). */
export function storedLanguage(storage: Pick<Storage, 'getItem'> | null = safeStorage()): LanguagePref {
  try {
    const v = storage?.getItem(LANGUAGE_KEY);
    return v === 'zh-CN' || v === 'en' ? v : 'system';
  } catch {
    return 'system';
  }
}

/** Persist a preference (`system` removes the stored value). */
export function storeLanguage(pref: LanguagePref, storage: Pick<Storage, 'setItem' | 'removeItem'> | null = safeStorage()): void {
  try {
    if (pref === 'system') storage?.removeItem(LANGUAGE_KEY);
    else storage?.setItem(LANGUAGE_KEY, pref);
  } catch {
    /* a per-viewer convenience only */
  }
}

function safeStorage(): Storage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/** Text of `key` in the active language (and platform reading), with `params` filled in. */
export function t(key: TextKey, params?: Params): string {
  return translate(current, variant, key, params);
}

/** Text of `key` in a given language and platform reading (tests, the language picker's own names). */
export function translate(l: Locale, v: TextVariant, key: TextKey, params?: Params): string {
  const res = RESOURCES[l];
  const msg = (v === 'macos' ? res[`${key}@macos` as MessageKey] : undefined) ?? res[key];
  if (msg === undefined) return key;
  return format(msg, params ?? {}, l);
}

/** A platform word used at the start of a heading or sentence ("tray" → "Tray"); Chinese is unchanged. */
export function upperFirst(s: string): string {
  return s.charAt(0).toLocaleUpperCase(current) + s.slice(1);
}

/** Whether a key exists (dynamic lookups such as code tables). */
export function hasText(key: string): key is TextKey {
  return Object.prototype.hasOwnProperty.call(RESOURCES[current], key);
}

// ---- formatting -------------------------------------------------------------------------------

const numberFormats = new Map<Locale, Intl.NumberFormat>();
const pluralRules = new Map<Locale, Intl.PluralRules>();

/** A count or quantity with the active locale's grouping ("1,234"). */
export function formatNumber(n: number | bigint, l: Locale = current): string {
  let f = numberFormats.get(l);
  if (!f) {
    f = new Intl.NumberFormat(l, { maximumFractionDigits: 20 });
    numberFormats.set(l, f);
  }
  return f.format(n);
}

function plural(l: Locale): Intl.PluralRules {
  let r = pluralRules.get(l);
  if (!r) {
    r = new Intl.PluralRules(l);
    pluralRules.set(l, r);
  }
  return r;
}

type Part = string | { name: string; kind: 'text' } | { name: string; kind: 'number' } | { name: string; kind: 'plural'; forms: Record<string, Part[]> };
const compiled = new Map<string, Part[]>();

function compile(msg: string): Part[] {
  let parts = compiled.get(msg);
  if (!parts) {
    parts = parse(msg);
    compiled.set(msg, parts);
  }
  return parts;
}

/** Index just after the brace that closes the one at `open`. */
function closing(s: string, open: number): number {
  let depth = 0;
  for (let i = open; i < s.length; i++) {
    if (s[i] === '{') depth++;
    else if (s[i] === '}' && --depth === 0) return i + 1;
  }
  throw new SyntaxError(`unbalanced brace in message: ${s}`);
}

function parse(s: string): Part[] {
  const out: Part[] = [];
  let text = '';
  for (let i = 0; i < s.length; ) {
    const c = s[i];
    if (c !== '{') {
      text += c;
      i++;
      continue;
    }
    const end = closing(s, i);
    const body = s.slice(i + 1, end - 1);
    const m = /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*(?:,\s*(number|plural)\s*(?:,([\s\S]*))?)?$/.exec(body);
    if (!m) throw new SyntaxError(`bad placeholder {${body}}`);
    if (text) out.push(text);
    text = '';
    if (m[2] === 'plural') {
      const forms: Record<string, Part[]> = {};
      const src = m[3] ?? '';
      const re = /\s*(=\d+|zero|one|two|few|many|other)\s*\{/y;
      let pos = 0;
      while (pos < src.length) {
        re.lastIndex = pos;
        const fm = re.exec(src);
        if (!fm) {
          if (src.slice(pos).trim() === '') break;
          throw new SyntaxError(`bad plural forms in {${body}}`);
        }
        const open = re.lastIndex - 1;
        const close = closing(src, open);
        forms[fm[1]] = parse(src.slice(open + 1, close - 1));
        pos = close;
      }
      if (!forms.other) throw new SyntaxError(`plural without "other" in {${body}}`);
      out.push({ name: m[1], kind: 'plural', forms });
    } else out.push(m[2] === 'number' ? { name: m[1], kind: 'number' } : { name: m[1], kind: 'text' });
    i = end;
  }
  if (text) out.push(text);
  return out;
}

function asNumber(v: ParamValue): number | bigint | null {
  if (typeof v === 'number' || typeof v === 'bigint') return v;
  if (typeof v === 'string' && v.trim() !== '' && !Number.isNaN(Number(v))) return Number(v);
  return null;
}

function render(parts: Part[], params: Params, l: Locale, hash: string | null): string {
  let out = '';
  for (const p of parts) {
    if (typeof p === 'string') {
      out += hash === null ? p : p.replace(/#/g, hash);
      continue;
    }
    const v = params[p.name];
    if (p.kind === 'text') out += String(v);
    else if (p.kind === 'number') {
      const n = asNumber(v);
      out += n === null ? String(v) : formatNumber(n, l);
    } else {
      const n = asNumber(v);
      const exact = n === null ? undefined : p.forms[`=${String(n)}`];
      const form = exact ?? (n === null ? p.forms.other : (p.forms[plural(l).select(Number(n))] ?? p.forms.other));
      out += render(form, params, l, n === null ? String(v) : formatNumber(n, l));
    }
  }
  return out;
}

/** Fill a message in; exported for tests and the resource guard. */
export function format(msg: string, params: Params, l: Locale = current): string {
  return render(compile(msg), params, l, null);
}

const matchers = new Map<Locale, { exact: Set<string>; patterns: RegExp[] }>();

/**
 * Whether `text` is one of the interface texts of a language (any platform reading, placeholders matching anything).
 * The views use it to tell their own words from a host message that was passed through as sent.
 */
export function isResourceText(text: string, l: Locale = current): boolean {
  let m = matchers.get(l);
  if (!m) {
    const exact = new Set<string>();
    const patterns: RegExp[] = [];
    for (const msg of Object.values(RESOURCES[l])) {
      const parts = compile(msg);
      if (parts.every((p) => typeof p === 'string')) exact.add(msg);
      // A message with words of its own (not just placeholders and punctuation) can be recognised with any values.
      else if (parts.every((p) => typeof p === 'string' || p.kind !== 'plural') && parts.filter((p) => typeof p === 'string').join('').replace(/[^\p{L}]/gu, '').length >= 4) {
        const src = parts.map((p) => (typeof p === 'string' ? p.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') : '[\\s\\S]*?')).join('');
        patterns.push(new RegExp(`^${src}$`));
      }
    }
    m = { exact, patterns };
    matchers.set(l, m);
  }
  return m.exact.has(text) || m.patterns.some((re) => re.test(text));
}

/** Parameter names a message uses (the guard compares them between languages). */
export function messageParams(msg: string): string[] {
  const names = new Set<string>();
  const walk = (parts: Part[]) => {
    for (const p of parts) {
      if (typeof p === 'string') continue;
      names.add(p.name);
      if (p.kind === 'plural') Object.values(p.forms).forEach(walk);
    }
  };
  walk(compile(msg));
  return [...names].sort();
}
