// Byte values cross IPC as u64 decimal strings and stay BigInt in the UI. They are never
// converted to a JS number, which would silently round above 2^53. Unknown is `null`, never 0.

import { formatNumber, t } from '../i18n';

const UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB', 'EiB'] as const;
const DECIMAL_RE = /^(0|[1-9][0-9]*)$/;
const U64_MAX = (1n << 64n) - 1n;

/** Parse a protocol byte string. Rejects numbers, signs, leading zeros and values above u64. */
export function parseBytes(value: string | null | undefined): bigint | null {
  if (value === null || value === undefined) return null;
  if (typeof value !== 'string' || !DECIMAL_RE.test(value)) {
    throw new TypeError(`byte count must be a decimal string, got ${JSON.stringify(value)}`);
  }
  const n = BigInt(value);
  if (n > U64_MAX) throw new RangeError(`byte count exceeds u64: ${value}`);
  return n;
}

/**
 * Human size with binary units and one decimal, rounded half-up using integer arithmetic only.
 * `null` renders as the unknown label, never as "0 B".
 */
export function formatBytes(value: bigint | string | null | undefined, unknownLabel = t('common.unknown')): string {
  const n = typeof value === 'bigint' ? value : parseBytes(value ?? null);
  if (n === null) return unknownLabel;
  if (n < 1024n) return `${n} B`;
  let unit = 0;
  let scale = 1n;
  while (unit < UNITS.length - 1 && n >= scale * 1024n) {
    scale *= 1024n;
    unit += 1;
  }
  // tenths = round(n * 10 / scale)
  let tenths = (n * 10n + scale / 2n) / scale;
  if (tenths >= 10240n && unit < UNITS.length - 1) {
    // Rounding carried into the next unit (e.g. 1023.96 KiB -> 1.0 MiB).
    scale *= 1024n;
    unit += 1;
    tenths = (n * 10n + scale / 2n) / scale;
  }
  const whole = tenths / 10n;
  const frac = tenths % 10n;
  return `${whole}.${frac} ${UNITS[unit]}`;
}

/** Exact count with digit grouping, e.g. "1,610,612,736 字节" / "1,610,612,736 bytes". */
export function formatExactBytes(value: bigint | string | null | undefined, unknownLabel = t('common.unknown')): string {
  const n = typeof value === 'bigint' ? value : parseBytes(value ?? null);
  if (n === null) return unknownLabel;
  return t('bytes.exact', { n });
}

/** A count grouped for the interface language (Intl.NumberFormat: "1,610,612,736" in 简体中文 and in English). */
export function groupDigits(n: bigint | number): string {
  return formatNumber(n);
}

/**
 * part/whole as a float in [0, 1] computed in fixed point (1e9) with BigInt. Only the *ratio*
 * becomes a number (for geometry and percentages); byte values themselves never do.
 */
export function ratio(part: bigint, whole: bigint): number {
  if (whole <= 0n || part <= 0n) return 0;
  const SCALE = 1_000_000_000n;
  const fixed = (part * SCALE) / whole;
  return Number(fixed) / 1e9;
}

/** Share with one decimal, "<0.1%" for tiny non-zero parts. */
export function formatShare(part: bigint | null, whole: bigint | null): string {
  if (part === null || whole === null || whole <= 0n) return '—';
  if (part === 0n) return '0%';
  const permille = (part * 1000n + whole / 2n) / whole;
  if (permille === 0n) return '<0.1%';
  return `${permille / 10n}.${permille % 10n}%`;
}

export function sumBytes(values: Iterable<bigint | null>): { known: bigint; unknown: number } {
  let known = 0n;
  let unknown = 0;
  for (const v of values) {
    if (v === null) unknown += 1;
    else known += v;
  }
  return { known, unknown };
}
