// Display helpers shared by every page: paths shortened in the middle (the drive and the last name stay
// readable) and sizes where unknown is always "未知", never 0.

import { formatBytes, formatExactBytes, parseBytes } from './bytes';
import { t } from '../i18n';

/** The six states a status banner shows, each with its own shape (views/ui/status.ts). */
export type BannerKind = 'success' | 'partial' | 'failed' | 'cancelled' | 'running' | 'pending-permission';

/** Visual width: CJK and full-width characters count 2, everything else 1. */
export function textWidth(s: string): number {
  let w = 0;
  for (const ch of s) w += charWidth(ch);
  return w;
}

function charWidth(ch: string): number {
  const c = ch.codePointAt(0) ?? 0;
  return (c >= 0x1100 && c <= 0x115f) || (c >= 0x2e80 && c <= 0xa4cf) || (c >= 0xac00 && c <= 0xd7a3) || (c >= 0xf900 && c <= 0xfaff) || (c >= 0xfe30 && c <= 0xfe4f) || (c >= 0xff00 && c <= 0xff60) || (c >= 0xffe0 && c <= 0xffe6) || c >= 0x20000 ? 2 : 1;
}

function takeWidth(chars: string[], budget: number, fromEnd: boolean): string[] {
  const out: string[] = [];
  let w = 0;
  const seq = fromEnd ? [...chars].reverse() : chars;
  for (const ch of seq) {
    const cw = charWidth(ch);
    if (w + cw > budget) break;
    out.push(ch);
    w += cw;
  }
  return fromEnd ? out.reverse() : out;
}

/**
 * Shorten a path in the middle to at most `max` width units: the start (drive, first folders) and the
 * last name stay, the middle becomes "…". The full text is never lost — callers keep it in a title and
 * in what is copied.
 */
export function middleEllipsis(text: string, max = 56): string {
  if (textWidth(text) <= max) return text;
  const chars = [...text];
  if (max <= 4) return takeWidth(chars, Math.max(1, max - 1), false).join('') + '…';
  const budget = max - 1;
  const sep = Math.max(text.lastIndexOf('\\'), text.lastIndexOf('/'));
  // The last segment (with its separator) is the part people recognise; keep it whole when it fits in 60 %.
  const lastSeg = sep >= 0 ? [...text.slice(sep)] : [];
  const lastW = textWidth(lastSeg.join(''));
  const tailBudget = lastSeg.length && lastW <= Math.ceil(budget * 0.6) ? lastW : Math.ceil(budget / 2);
  const tail = takeWidth(chars, tailBudget, true);
  const head = takeWidth(chars, budget - textWidth(tail.join('')), false);
  return `${head.join('')}…${tail.join('')}`;
}

/** Size text: "1.2 GiB", "至少 1.2 GiB" when part is unknown, "未知" when nothing is known. Never "0 B" for unknown. */
export function sizeLabel(bytes: string | bigint | null | undefined, opts: { unknownCount?: number; atLeast?: boolean } = {}): string {
  const n = typeof bytes === 'bigint' ? bytes : safeParse(bytes);
  if (n === null) return t('common.unknown');
  const partial = opts.atLeast || (opts.unknownCount ?? 0) > 0;
  if (partial && n === 0n) return t('common.unknown');
  return partial ? t('display.leastN', { n: formatBytes(n) }) : formatBytes(n);
}

/** Exact bytes for a tooltip ("71,393,594,448 字节"), or "大小未知". */
export function sizeTitle(bytes: string | bigint | null | undefined): string {
  const n = typeof bytes === 'bigint' ? bytes : safeParse(bytes);
  return n === null ? t('common.sizeUnknown') : formatExactBytes(n);
}

/** Byte strings that are malformed read as unknown here (the grid and tests still use the strict parser). */
function safeParse(v: string | null | undefined): bigint | null {
  try {
    return parseBytes(v ?? null);
  } catch {
    return null;
  }
}

/** "1,487 个条目" / "1,487 entries" (a plural form in English). */
export function countLabel(n: number): string {
  return t('display.entries', { n });
}
