// NEW-13 (WFA-93): a process's purpose and the reason it is not identified, in the interface language. The host's
// local process knowledge (cm-core process.rs `KNOWN` on Windows, cm-platform macos/processes.rs on macOS) names each
// known executable and writes its purpose in English; the host shows a purpose only when the name, the location and
// the signer all agree. Here each known name has its purpose in both resources (`hostWords.test.ts` reads the two
// Rust tables), and the few fixed "not identified" reasons are put in words. Anything else keeps the host's text in a
// tooltip, never as the main text.

import { hasText, t, type TextKey } from '../i18n';
import { isRawHostText } from './suiteWords';

/** The resource key of a known executable's purpose (Windows names are matched without case, like the host). */
export function purposeKey(name: string): TextKey | null {
  for (const k of [`processWords.purpose.${name}`, `processWords.purpose.${name.toLowerCase()}`]) if (hasText(k)) return k;
  return null;
}

/**
 * The purpose line of an identified process: its words when the name is in the table; the host's text when it is
 * already in the interface language; otherwise a plain "identified" line with the host's text as the tooltip.
 */
export function processPurpose(p: { name: string; identified: boolean; purpose: string | null }): { text: string; raw: string | null } | null {
  if (!p.identified) return null;
  const key = purposeKey(p.name);
  if (key) return { text: t(key), raw: null };
  if (!p.purpose) return null;
  if (!isRawHostText(p.purpose)) return { text: p.purpose, raw: null };
  return { text: t('processWords.identified'), raw: p.purpose };
}

/** The host's fixed reasons for "not identified" (both platforms), by their English start. */
const UNIDENTIFIED: [RegExp, (m: RegExpExecArray) => string][] = [
  [/^Not in CleanManager's local process knowledge/, () => t('processWords.unid.notKnown')],
  [/^Named like a known (?:Windows )?process but runs from an unexpected location: (.+)$/, (m) => t('processWords.unid.location', { path: m[1] })],
  [/^Named like a known process but its location could not be read/, () => t('processWords.unid.noLocation')],
  [/^Named like a known process but its signature could not be checked/, () => t('processWords.unid.signatureUnknown')],
  [/^Named like a known process but not signed by its vendor(?: \(signer: (.+)\))?/, (m) => (m[1] && m[1] !== 'none' ? t('processWords.unid.signedBy', { signer: m[1] }) : t('processWords.unid.notVendorSigned'))],
  [/^Named like a known process but signed by (.+)$/, (m) => t('processWords.unid.signedBy', { signer: m[1] })],
];

/** Why a process is not identified, in words; an unknown reason keeps the host's text as the tooltip. */
export function unidentifiedReason(reason: string | null): { text: string; raw: string | null } {
  if (!reason) return { text: t('processWords.unid.notKnown'), raw: null };
  for (const [re, words] of UNIDENTIFIED) {
    const m = re.exec(reason);
    if (m) return { text: words(m), raw: null };
  }
  return isRawHostText(reason) ? { text: t('processWords.unid.other'), raw: reason } : { text: reason, raw: null };
}
