// NEW-13 (WFA-76): 软件 › 卸载 in the interface language. The host writes why an application has no uninstall entry
// (cm-core uninstall.rs `protected_refusal` / `select_adapter`), how it would be uninstalled (an adapter's display:
// the command plus a parenthetical kind) and why a leftover belongs to the application (cm-platform windows
// uninstall.rs, cm-core ownership markers) as fixed English sentences. Each known sentence has its words here;
// `hostWords.test.ts` reads the Rust sources so a new sentence cannot ship without them. Anything unknown keeps
// the host's text as the tooltip, never as the main text. Commands, paths and names are data and stay as sent.

import { t } from '../i18n';
import { isRawHostText } from './suiteWords';

type Words = (m: RegExpExecArray) => string;

/** Why an application has no uninstall entry here, by the start of the host's sentence. */
const REASONS: [RegExp, Words][] = [
  [/^CleanManager is removed by its own installer/, () => t('uninstallWords.reason.self')],
  [/^Security or device-management agent/, () => t('uninstallWords.reason.security')],
  [/^System component: part of Windows or of another product/, () => t('uninstallWords.reason.systemComponent')],
  [/^Windows package: part of the operating system/, () => t('uninstallWords.reason.osPackage')],
  [/^Shared runtime other applications depend on/, () => t('uninstallWords.reason.sharedRuntime')],
  [/^npm package name is not recognized/, () => t('uninstallWords.reason.npmName')],
  [/^Node\.js\/npm was not found/, () => t('uninstallWords.reason.npmMissing')],
  [/^Scoop was not found or the app name is not recognized/, () => t('uninstallWords.reason.scoop')],
  [/^package full name unknown/, () => t('uninstallWords.reason.msixName')],
  [/^the package manifest could not be found/, () => t('uninstallWords.reason.msixManifest')],
  [/^winget package identifier is not recognized/, () => t('uninstallWords.reason.wingetId')],
  [/^installed by winget, but winget was not found/, () => t('uninstallWords.reason.wingetMissing')],
  [/^MSI product code is not valid/, () => t('uninstallWords.reason.msiCode')],
  [/^no uninstall command is registered for this application/, () => t('uninstallWords.reason.noCommand')],
  [/^the registered uninstall command could not be read/, () => t('uninstallWords.reason.unreadable')],
  [/^msiexec command without a product code/, () => t('uninstallWords.reason.msiexecNoCode')],
  [/^the registered command names a program without its full path/, () => t('uninstallWords.reason.barePath')],
  [/^the registered command is a script or host program, not a verifiable uninstaller/, () => t('uninstallWords.reason.scriptHost')],
  [/^the registered uninstall command is unusually long/, () => t('uninstallWords.reason.tooLong')],
  [/^no publisher is recorded, so the uninstaller's signature cannot be matched/, () => t('uninstallWords.reason.noPublisher')],
];

/** The reason line of an uninstall row: words for a known sentence, else a plain line with the host's text as tooltip. */
export function uninstallReason(reason: string | null): { text: string; raw: string | null } | null {
  if (!reason) return null;
  for (const [re, words] of REASONS) {
    const m = re.exec(reason);
    if (m) return { text: words(m), raw: null };
  }
  return isRawHostText(reason) ? { text: t('uninstallWords.reason.other'), raw: reason } : { text: reason, raw: null };
}

/** The parenthetical kinds of the host's Windows adapter displays (`command (kind)`). */
const ADAPTER_KIND: Record<string, () => string> = {
  'vendor uninstaller, its own UI': () => t('uninstallWords.adapter.vendor'),
  'Windows package manager, current user': () => t('uninstallWords.adapter.msix'),
  'package manager': () => t('uninstallWords.adapter.packageManager'),
  'Windows Installer': () => t('uninstallWords.adapter.msi'),
};

/** A Windows adapter display with its kind in words; the command itself is shown exactly as it will run. */
export function adapterDisplay(display: string): string {
  const m = /^(.*) \(([^()]+)\)$/.exec(display);
  const kind = m ? ADAPTER_KIND[m[2]] : undefined;
  return m && kind ? t('uninstallWords.adapter.commandKind', { command: m[1], kind: kind() }) : display;
}

/**
 * R3-04: the object line of a plan-review or result row. An uninstaller row's line is the host's route display
 * (`"…\uninst.exe" /c … (vendor uninstaller, its own UI)`), so its kind is put in words; any other row as sent.
 */
export function rowDisplay(display: string, uninstaller: boolean): string {
  return uninstaller ? adapterDisplay(display) : display;
}

/** Why a leftover belongs to the application (its evidence lines), by their fixed shapes. */
const EVIDENCE: [RegExp, Words][] = [
  [/^contains only caches and logs$/, () => t('uninstallWords.evidence.cachesOnly')],
  [/^(.+) matches the product name$/, (m) => t('uninstallWords.evidence.productName', { what: m[1] })],
  [/^(.+) matches publisher\\product$/, (m) => t('uninstallWords.evidence.publisherProduct', { what: m[1] })],
  [/^starts a program in (.+)$/, (m) => t('uninstallWords.evidence.startsProgram', { dir: m[1] })],
  [/^service program lives in (.+)$/, (m) => t('uninstallWords.evidence.serviceIn', { dir: m[1] })],
  [/^App Paths registration of (.+) points into (.+)$/, (m) => t('uninstallWords.evidence.appPaths', { name: m[1], dir: m[2] })],
  [/^(.+) mentions (.+) \(read-only hint; CleanManager does not edit it\)$/, (m) => t('uninstallWords.evidence.mentions', { file: m[1], dir: m[2] })],
  [/^(.+) is installed again$/, (m) => t('uninstallWords.evidence.reinstalled', { name: m[1] })],
  [/^(.+) \(installed\) may use it too$/, (m) => t('uninstallWords.evidence.shared', { name: m[1] })],
  [/^only the family name "(.+)" matches; another edition may own it$/, (m) => t('uninstallWords.evidence.familyName', { name: m[1] })],
  [/^the short name "(.+)" alone does not prove ownership$/, (m) => t('uninstallWords.evidence.shortName', { name: m[1] })],
  // A leftover's note (cm-platform windows uninstall.rs, cm-core uninstall.rs startup orphans).
  [/^backed up before it is deleted; restorable from history$/, () => t('uninstallWords.note.backedUp')],
  [/^orphan: the program it starts no longer exists$/, () => t('uninstallWords.note.orphanMissing')],
  [/^orphan: idle for at least (\d+) days$/, (m) => t('uninstallWords.note.orphanIdle', { n: Number(m[1]) })],
  [/^used within the last (\d+) days; kept$/, (m) => t('uninstallWords.note.recentlyUsed', { n: Number(m[1]) })],
];

/** One evidence line in words; an unknown one reads plainly with the host's text as the tooltip. */
export function evidenceText(line: string): { text: string; raw: string | null } {
  for (const [re, words] of EVIDENCE) {
    const m = re.exec(line);
    if (m) return { text: words(m), raw: null };
  }
  return isRawHostText(line) ? { text: t('uninstallWords.evidence.other'), raw: line } : { text: line, raw: null };
}
