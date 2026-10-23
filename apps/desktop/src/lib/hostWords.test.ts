// NEW-13: host sentences the pages show are put in words (zh-CN and en); the host's English text stays a tooltip. Each
// table is read against the Rust source that writes the sentences, so a new or changed sentence fails here.
import { describe, expect, it } from 'vitest';
import { processPurpose, purposeKey, unidentifiedReason } from './processWords';
import { guidanceText, sizeNoteText } from './insightWords';
import { adapterDisplay, evidenceText, uninstallReason } from './uninstallWords';
import { diagnosisLine } from '../views/tools';
import { RESOURCES, setLocale } from '../i18n';
import winProcess from '../../../../crates/cm-core/src/process.rs?raw';
import macProcess from '../../../../crates/cm-platform/src/macos/processes.rs?raw';
import coreInsights from '../../../../crates/cm-core/src/insights.rs?raw';
import winInsights from '../../../../crates/cm-platform/src/windows/insights.rs?raw';
import coreUninstall from '../../../../crates/cm-core/src/uninstall.rs?raw';
import winUninstall from '../../../../crates/cm-platform/src/windows/uninstall.rs?raw';
import coreStatus from '../../../../crates/cm-core/src/status.rs?raw';
import manualTrash from '../../../../crates/cm-core/src/manual_trash.rs?raw';
import { protectedWhy } from '../views/trash';
import { protectionText } from './codes';

const HAN = /[\u4e00-\u9fff]/;
const known = (src: string) => {
  const body = src.slice(src.indexOf('pub const KNOWN'));
  return [...body.slice(0, body.indexOf('];')).matchAll(/k\(\s*"([^"]+)",[\s\S]*?,\s*"((?:[^"\\]|\\.)*)"\s*\)/g)].map((m) => ({ name: m[1], purpose: m[2] }));
};

describe('processes: purposes and "not identified" reasons', () => {
  it('every known executable of both hosts has its purpose in both languages (English = the host text)', () => {
    const rows = [...known(winProcess), ...known(macProcess)];
    expect(rows.length).toBeGreaterThan(100);
    for (const r of rows) {
      const key = purposeKey(r.name);
      expect(key, r.name).not.toBeNull();
      expect(RESOURCES.en[key!], r.name).toBe(r.purpose);
      expect(HAN.test(RESOURCES['zh-CN'][key!]), r.name).toBe(true);
    }
  });

  it('an identified process reads its purpose in words; the host text never is the main text', () => {
    expect(processPurpose({ name: 'dwm.exe', identified: true, purpose: 'Desktop Window Manager: draws windows, animations and transparency' })).toEqual({ text: '桌面窗口管理器：绘制窗口、动画与透明效果', raw: null });
    expect(processPurpose({ name: 'DWM.EXE', identified: true, purpose: 'x' })?.text).toContain('桌面窗口管理器');
    expect(processPurpose({ name: 'newthing.exe', identified: true, purpose: 'Does new things' })).toEqual({ text: expect.stringContaining('已通过'), raw: 'Does new things' });
    expect(processPurpose({ name: 'dwm.exe', identified: false, purpose: null })).toBeNull();
    setLocale('en');
    expect(processPurpose({ name: 'dwm.exe', identified: true, purpose: 'x' })?.text).toBe('Desktop Window Manager: draws windows, animations and transparency');
  });

  it('every reason the hosts give for "not identified" reads in words', () => {
    const reasons = [...winProcess.matchAll(/Err\((?:format!\()?"(Named like[^"]+|Not in[^"]+)"/g), ...macProcess.matchAll(/Err\((?:format!\()?"(Named like[^"]+|Not in[^"]+)"/g)].map((m) => m[1].replace('{p}', 'C:\\x.exe').replace('{}', 'none'));
    expect(reasons.length).toBeGreaterThanOrEqual(8);
    for (const r of reasons) {
      const w = unidentifiedReason(r);
      expect(w.raw, r).toBeNull();
      expect(HAN.test(w.text), r).toBe(true);
    }
    expect(unidentifiedReason('Named like a known process but signed by Evil Corp').text).toBe('名称像已知进程，但签名者是 Evil Corp');
    expect(unidentifiedReason('something new').raw).toBe('something new');
  });
});

describe('space insights: official-tool guidance and size notes', () => {
  const guides = (() => {
    const out: { kind: string; i: number; tool: string; steps: string; impact: string }[] = [];
    for (const en of ['WindowsObject', 'MacObject']) {
      const body = coreInsights.slice(coreInsights.indexOf(`impl ${en}`));
      const kb = body.slice(body.indexOf('fn kind(self)'));
      const kinds = new Map([...kb.slice(0, kb.indexOf('\n    }\n')).matchAll(new RegExp(`${en}::(\\w+) => "([a-z_.]+)"`, 'g'))].map((m) => [m[1], m[2]]));
      const gb = body.slice(body.indexOf('fn guidance(self)'));
      for (const arm of gb.slice(0, gb.indexOf('\n    }\n')).split(new RegExp(`\\n            (?=${en}::)`)).slice(1)) {
        const [head, rest] = [arm.slice(0, arm.indexOf('=>')), arm.slice(arm.indexOf('=>'))];
        const gs = [...rest.matchAll(/g\(\s*"((?:[^"\\]|\\.)*)",\s*"((?:[^"\\]|\\.)*)",\s*(?:true|false),\s*"((?:[^"\\]|\\.)*)",?\s*\)/g)];
        for (const v of [...head.matchAll(new RegExp(`${en}::(\\w+)`, 'g'))].map((m) => m[1]))
          gs.forEach((g, i) => out.push({ kind: kinds.get(v)!, i, tool: g[1], steps: g[2].replace(/\\"/g, '"'), impact: g[3] }));
      }
    }
    return out;
  })();

  it('every guidance line of the host has its effect in words, in the host order', () => {
    expect(guides.length).toBeGreaterThan(30);
    for (const g of guides) {
      setLocale('en');
      expect(guidanceText(g.kind, g.i, g), `${g.kind}#${g.i}`).toEqual({ tool: g.tool, steps: g.steps, impact: g.impact, raw: null });
      setLocale('zh-CN');
      const zh = guidanceText(g.kind, g.i, g);
      expect(HAN.test(zh.impact), `${g.kind}#${g.i}`).toBe(true);
    }
    // A pure command is shown (and copied) exactly as the host wrote it.
    expect(guidanceText('windows.hibernation_file', 1, { tool: 'powercfg', steps: 'powercfg /hibernate off', impact: 'x' }).steps).toBe('powercfg /hibernate off');
  });

  it('the size notes the Windows host writes read in words; others keep the host text as tooltip', () => {
    const notes = [...winInsights.matchAll(/let why = "([^"]+)"/g)].map((m) => m[1]);
    expect(notes.length).toBeGreaterThanOrEqual(3);
    for (const n of notes) {
      expect(sizeNoteText(n)?.raw, n).toBeNull();
      expect(sizeNoteText(`Size unavailable: ${n}`)?.text, n).toMatch(/^大小不可用：/);
    }
    expect(sizeNoteText('no admin')).toEqual({ text: '没有测量（原因见提示）', raw: 'no admin' });
    expect(sizeNoteText(null)).toBeNull();
  });
});

describe('uninstall: reasons, routes and leftover evidence', () => {
  it('every sentence the core writes for "no uninstall entry" reads in words', () => {
    const body = coreUninstall.slice(coreUninstall.indexOf('pub fn protected_refusal'), coreUninstall.indexOf('fn command_tail'));
    const sentences = [...body.matchAll(/refuse\(\s*"[a-z_]+",\s*"([^"]+)"/g), ...body.matchAll(/=> "([A-Z][^"]+)",?\n/g)].map((m) => m[1]);
    expect(sentences.length).toBeGreaterThanOrEqual(18);
    for (const s of sentences) {
      const w = uninstallReason(s)!;
      expect(w.raw, s).toBeNull();
      expect(HAN.test(w.text), s).toBe(true);
    }
    expect(uninstallReason('Windows package: part of the operating system')?.text).toBe('Windows 包：属于操作系统的一部分');
    expect(uninstallReason('brand new')).toEqual({ text: expect.stringContaining('原因见提示'), raw: 'brand new' });
  });

  it('a Windows route keeps its command exactly and says its kind in words', () => {
    expect(adapterDisplay('"C:\\Apps\\Bar\\uninst.exe" /c exit 3 (vendor uninstaller, its own UI)')).toBe('"C:\\Apps\\Bar\\uninst.exe" /c exit 3（厂商的卸载程序，使用它自己的界面）');
    expect(adapterDisplay('remove package Pkg_1.0_x64__abc (Windows package manager, current user)')).toBe('remove package Pkg_1.0_x64__abc（Windows 包管理器，当前用户）');
    expect(adapterDisplay('msiexec /x {X} (Windows Installer)')).toContain('msiexec /x {X}');
    expect(adapterDisplay('something else')).toBe('something else');
    // Every kind the core writes is in the table.
    for (const m of coreUninstall.matchAll(/\(([^()"]+)\)"\)\)?;?/g)) if (/uninstaller|package manager|Installer/.test(m[1])) expect(adapterDisplay(`x (${m[1]})`), m[1]).not.toBe(`x (${m[1]})`);
  });

  it('leftover evidence and notes read in words; paths and names stay as sent', () => {
    expect(evidenceText('%LOCALAPPDATA%\\CMWFA-BazApp matches the product name')).toEqual({ text: '%LOCALAPPDATA%\\CMWFA-BazApp 与产品名称一致', raw: null });
    expect(evidenceText('contains only caches and logs').text).toBe('只含缓存和日志');
    expect(evidenceText('orphan: idle for at least 90 days').text).toBe('孤立项：至少 90 天没有使用');
    const shapes = [...winUninstall.matchAll(/format!\(r?"\{[^"]*\} (matches the product name|matches publisher\\\\product)"/g)].length;
    expect(shapes).toBeGreaterThanOrEqual(2);
    expect(evidenceText('brand new evidence').raw).toBe('brand new evidence');
  });
});

describe('move to the bin: why something inside is protected', () => {
  it('every reason the core writes reads in words', () => {
    expect(protectedWhy('default protection class credentials')).toBe(protectionText('credentials'));
    expect(protectedWhy('hard-deny location C:\\Windows (system)')).toBe('系统硬拒绝位置');
    expect(protectedWhy('hard-deny: refused by the path check')).toBe('系统硬拒绝位置');
    expect(protectedWhy('protected by user entry u_3')).toBe('你的保护项 u_3');
    expect(protectedWhy('contains an object protected by user entry u_4')).toBe('你的保护项 u_4');
    for (const m of manualTrash.matchAll(/format!\("((?:default protection class|hard-deny|protected by user|contains an object protected)[^"]*)"/g)) expect(protectedWhy(m[1].replace(/\{[^}]*\}/g, 'credentials')), m[1]).not.toBe('受保护');
  });
});

describe('status: the diagnosis line', () => {
  it('normal is a healthy line of its own, never "未识别的问题（代码 normal）"; every problem code has words', () => {
    expect(coreStatus).toContain('code: "normal".into()');
    expect(diagnosisLine({ code: 'normal', message: 'No metric crosses a diagnosis threshold' })).toEqual({ normal: true, text: '没有读数超过诊断阈值。', raw: null });
    for (const m of coreStatus.matchAll(/line\("([a-z_]+)"/g)) {
      const d = diagnosisLine({ code: m[1], message: 'raw' });
      expect(d.raw, m[1]).toBeNull();
      expect(d.text, m[1]).not.toContain('未识别');
    }
    expect(diagnosisLine({ code: 'brand_new', message: 'raw' })).toEqual({ normal: false, text: '未识别的问题（代码 brand_new）', raw: 'raw' });
  });
});
