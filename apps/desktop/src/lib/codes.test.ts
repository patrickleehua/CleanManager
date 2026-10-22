import { afterEach, describe, expect, it } from 'vitest';
import { explainIssue, groupIssues, ISSUE_TEXT, permissionCause, refusalKnown, refusalText, unknownCodeText } from './codes';
import { ERROR_HINT } from './format';
import { setPlatform } from './platform';
import mockTs from './mock.ts?raw';
// The host's sources are the single source of the codes it can send; the table must name every one.
import coreScan from '../../../../crates/cm-core/src/scan.rs?raw';
import winDirectory from '../../../../crates/cm-platform/src/windows/directory.rs?raw';
import winNtfs from '../../../../crates/cm-platform/src/windows/ntfs.rs?raw';
import winStartup from '../../../../crates/cm-platform/src/windows/startup.rs?raw';
import winApps from '../../../../crates/cm-platform/src/windows/apps.rs?raw';
import winUninstall from '../../../../crates/cm-platform/src/windows/uninstall.rs?raw';
import macWalk from '../../../../crates/cm-platform/src/macos/walk.rs?raw';
import macBulk from '../../../../crates/cm-platform/src/macos/bulk.rs?raw';
import macApps from '../../../../crates/cm-platform/src/macos/apps.rs?raw';
import macStartup from '../../../../crates/cm-platform/src/macos/startup.rs?raw';
import appScan from '../../../../crates/cm-app/src/scan.rs?raw';
import appDiag from '../../../../crates/cm-app/src/diag.rs?raw';
import storeIndex from '../../../../crates/cm-store/src/index.rs?raw';
import cliScan from '../../../../crates/cm-cli/src/scan_cmd.rs?raw';
import manualTrash from '../../../../crates/cm-core/src/manual_trash.rs?raw';
import hostDto from '../../src-tauri/src/dto.rs?raw';
import hostHost from '../../src-tauri/src/host.rs?raw';
import hostCommands from '../../src-tauri/src/commands.rs?raw';
import hostCleanup from '../../src-tauri/src/cleanup.rs?raw';
import hostOps from '../../src-tauri/src/ops.rs?raw';
import hostV2 from '../../src-tauri/src/v2.rs?raw';
import hostSettings from '../../src-tauri/src/settings.rs?raw';
import hostSoftware from '../../src-tauri/src/software.rs?raw';
import hostSuite from '../../src-tauri/src/suite.rs?raw';
import hostSystem from '../../src-tauri/src/system.rs?raw';
import hostResident from '../../src-tauri/src/resident.rs?raw';
import hostGate from '../../src-tauri/src/gate.rs?raw';
import hostDiag from '../../src-tauri/src/diag.rs?raw';

afterEach(() => setPlatform('windows'));

/**
 * Matches in a source's production code: its inline `#[cfg(test)] mod … {` module is left out (a test-only item
 * elsewhere in the file, e.g. a `#[cfg(test)]` helper method, does not hide the code after it).
 */
const all = (src: string, re: RegExp): string[] => [...src.split(/#\[cfg\(test\)\]\s*(?:pub(?:\([a-z]+\))?\s+)?mod\s+\w+\s*\{/)[0].matchAll(re)].map((m) => m[1]);

/** Issue codes: `ScanIssue { code: "x"`, `issue("x", …)`, `(SkipReason::…, "x")`, and `pub const X: &str = "x"` in issue modules. */
function issueCodes(): Map<string, string> {
  const sources: Record<string, string> = { coreScan, winDirectory, winNtfs, winStartup, winApps, winUninstall, macWalk, macBulk, macApps, macStartup, appScan, storeIndex, cliScan, hostCleanup, hostSuite };
  const out = new Map<string, string>();
  for (const [name, src] of Object.entries(sources)) {
    for (const re of [/ScanIssue\s*\{\s*code:\s*"([a-z][a-z0-9_]*)"/g, /\bissue\(\s*"([a-z][a-z0-9_]*)"/g, /\(\s*SkipReason::\w+,\s*"([a-z][a-z0-9_]*)"/g]) for (const c of all(src, re)) out.set(c, name);
  }
  // Named codes: the shared `scan::issue` module and the CLI timeout code.
  for (const c of all(coreScan.slice(coreScan.indexOf('pub mod issue')), /pub const [A-Z_]+: &str = "([a-z_]+)"/g)) out.set(c, 'coreScan');
  for (const c of all(appScan, /pub const [A-Z_]*ISSUE[A-Z_]*: &str = "([a-z_]+)"/g)) out.set(c, 'appScan');
  return out;
}

/** Refusal reasons: `IpcError::new(ErrorCode::…, "x"`, the per-module `ipc(ErrorCode::…, "x"` helpers and the error→reason maps. */
function refusalCodes(): Map<string, string> {
  const sources: Record<string, string> = { hostDto, hostHost, hostCommands, hostCleanup, hostOps, hostV2, hostSettings, hostSoftware, hostSuite, hostSystem, hostResident, hostGate, hostDiag };
  const out = new Map<string, string>();
  for (const [name, src] of Object.entries(sources)) {
    for (const re of [/IpcError::new\(\s*ErrorCode::\w+,\s*"([a-z][a-z0-9_]*)"/g, /\bipc\(\s*ErrorCode::\w+,\s*"([a-z][a-z0-9_]*)"/g, /StoreError::\w+[^=\n]*=>\s*"([a-z][a-z0-9_]*)"/g]) for (const c of all(src, re)) out.set(c, name);
  }
  for (const c of all(appDiag, /DiagAppError::\w+[^=\n]*=>\s*"([a-z][a-z0-9_]*)"/g)) out.set(c, 'appDiag');
  // Manual-trash refusals carry their reason through as the IPC reason.
  for (const c of all(manualTrash, /refusal\(\s*"[a-z_]+",\s*"([a-z][a-z0-9_]*)"/g)) out.set(c, 'manualTrash');
  for (const c of all(manualTrash, /\(\s*"([a-z][a-z0-9_]*)",\s*format!/g)) out.set(c, 'manualTrash');
  return out;
}

describe('code → 中文 table covers every code the host can send', () => {
  it('finds the codes it is meant to find (the scan does not silently match nothing)', () => {
    const issues = issueCodes();
    for (const c of ['not_a_directory', 'root_is_link', 'root_unavailable', 'permission_required', 'identity_changed', 'own_store_skipped', 'store_error', 'timeout', 'host_exited', 'registry_open_failed', 'rule_inapplicable', 'cloud_placeholders', 'caskroom_unreadable']) {
      expect(issues.has(c), c).toBe(true);
    }
    expect(issues.size).toBeGreaterThan(40);
    const refusals = refusalCodes();
    for (const c of ['scan_running', 'unknown_root', 'root_missing', 'not_published', 'plan_stale', 'experimental_off', 'diag_not_configured', 'link_or_reparse']) expect(refusals.has(c), c).toBe(true);
    expect(refusals.size).toBeGreaterThan(60);
  });

  it('every issue code has Chinese words', () => {
    const missing = [...issueCodes()].filter(([code]) => !explainIssue({ code, scope_display: '', message: '' }).known).map(([c, src]) => `${c} (${src})`);
    expect(missing).toEqual([]);
  });

  it('every refusal reason has Chinese words', () => {
    const missing = [...refusalCodes()].filter(([code]) => !refusalKnown(code)).map(([c, src]) => `${c} (${src})`);
    expect(missing).toEqual([]);
  });

  it('every code the preview mock reports has words, except the one meant to show the unknown case', () => {
    const codes = new Set([...all(mockTs, /code: '([a-z][a-z0-9_]*)'/g), ...all(mockTs, /failed\('([a-z][a-z0-9_]*)'/g)]);
    const reasons = new Set(all(mockTs, /err\('[a-z_]+', '([a-z][a-z0-9_]*)'/g));
    expect(codes.has('not_a_directory') && codes.has('root_is_link') && codes.has('root_unavailable')).toBe(true);
    for (const c of codes) {
      if (c === 'index_quota_exhausted') continue;
      expect(explainIssue({ code: c, scope_display: '', message: '' }).known, c).toBe(true);
    }
    for (const r of reasons) expect(refusalKnown(r), r).toBe(true);
    expect(explainIssue({ code: 'index_quota_exhausted', scope_display: '', message: 'x' }).known).toBe(false);
  });

  it('no entry is English or empty', () => {
    for (const [k, v] of [...Object.entries(ISSUE_TEXT), ...Object.entries(ERROR_HINT)]) {
      expect(v.length, k).toBeGreaterThan(4);
      expect(/[\u4e00-\u9fff]/.test(v), k).toBe(true);
    }
  });
});

describe('issue lines', () => {
  it('the root refusals read as the plain sentences', () => {
    expect(explainIssue({ code: 'not_a_directory', scope_display: 'C:\\a.txt', message: 'root is not a directory' }).text).toBe('所选路径是文件，不是文件夹，请选择一个文件夹。');
    expect(explainIssue({ code: 'root_not_directory', scope_display: 'C:\\a.txt', message: 'root is not a directory' }).text).toBe('所选路径是文件，不是文件夹，请选择一个文件夹。');
    expect(explainIssue({ code: 'root_is_link', scope_display: 'C:\\l', message: 'x' }).text).toBe('所选路径是链接（目录联接或符号链接），请直接选择它指向的文件夹。');
    expect(refusalText({ reason: 'not_a_directory' })).toBe('所选路径是文件，不是文件夹，请选择一个文件夹。');
    expect(refusalText({ reason: 'root_is_link' })).toBe('所选路径是链接（目录联接或符号链接），请直接选择它指向的文件夹。');
  });

  it('a macOS link root reported as root_unavailable reads as a link; macOS has its own link words', () => {
    setPlatform('macos');
    const e = explainIssue({ code: 'root_unavailable', scope_display: '/Users/lin/x', message: 'root is a symbolic link; links are not followed' });
    expect(e.code).toBe('root_is_link');
    expect(e.rawCode).toBe('root_unavailable');
    expect(e.text).toBe('所选路径是替身或符号链接，请直接选择它指向的文件夹。');
    expect(refusalText({ reason: 'root_is_link' })).not.toContain('快捷方式');
    setPlatform('windows');
    expect(refusalText({ reason: 'root_is_link' })).toBe('所选路径是链接（目录联接或符号链接），请直接选择它指向的文件夹。');
  });

  it('an unknown code never shows the raw message as its text', () => {
    const e = explainIssue({ code: 'quota_x', scope_display: 'D:\\', message: 'disk quota exceeded' });
    expect(e.known).toBe(false);
    expect(e.text).toBe('未识别的问题（代码 quota_x）');
    expect(e.text).not.toContain('quota exceeded');
    expect(e.raw).toBe('disk quota exceeded');
    expect(refusalText({ reason: 'brand_new_reason', code: 'internal_error' })).toBe(ERROR_HINT.internal_error ?? 'CleanManager 内部出错，未做任何更改。');
    expect(refusalText({ reason: 'brand_new_reason' })).toBe(unknownCodeText('brand_new_reason'));
  });

  it('permission_cause: the structured field wins, the message marker is the fallback', () => {
    expect(permissionCause({ code: 'permission_required', message: 'x', permission_cause: 'privacy_prompt_pending' })).toBe('privacy_prompt_pending');
    expect(permissionCause({ code: 'permission_required', message: 'permission_required (permission_cause=undetermined)' })).toBe('undetermined');
    expect(permissionCause({ code: 'permission_required', message: 'Access is denied. (os error 5)' })).toBeNull();
    expect(permissionCause({ code: 'permission_required', message: 'permission_cause=guessed' })).toBeNull();
    const pending = explainIssue({ code: 'permission_required', scope_display: '/Users/lin/Documents', message: 'permission_required (permission_cause=privacy_prompt_pending)' });
    expect(pending.text).toBe('正在等待 macOS 授权：请在弹出的对话框中选择，然后重新扫描。');
    const generic = explainIssue({ code: 'permission_required', scope_display: 'D:\\x', message: 'm', permission_cause: 'undetermined' });
    expect(generic.text).toBe(ISSUE_TEXT.permission_required);
  });

  it('issues of one meaning are one line with a count; waiting for a prompt comes first', () => {
    const g = groupIssues([
      { code: 'permission_required', scope_display: 'D:\\a', message: 'denied' },
      { code: 'permission_required', scope_display: 'D:\\b', message: 'denied' },
      { code: 'permission_required', scope_display: '/Users/lin/Desktop', message: 'm', permission_cause: 'privacy_prompt_pending' },
      { code: 'quota_x', scope_display: 'D:\\c', message: 'raw' },
    ]);
    expect(g.map((x) => [x.issue.code, x.issue.cause, x.count])).toEqual([
      ['permission_required', 'privacy_prompt_pending', 1],
      ['permission_required', null, 2],
      ['quota_x', null, 1],
    ]);
    expect(g[1].scopes).toEqual(['D:\\a', 'D:\\b']);
  });
});
