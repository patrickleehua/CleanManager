import { describe, expect, it } from 'vitest';
import { EXTENSIONS, TOOLS, placeOf, shortcutFor } from './nav';
import { PLATFORM_WORDS, setPlatform } from './platform';
import { ACTION_LABEL, AVAILABILITY_LABEL, MODE_LABEL } from './words';

const key = (k: string, mods: Partial<Record<'ctrlKey' | 'metaKey' | 'altKey' | 'shiftKey', boolean>> = {}) => ({ key: k, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...mods });

describe('GUI-08 navigation', () => {
  it('five tools then the extensions, in a fixed order with fixed names', () => {
    expect(TOOLS.map((t) => t.label)).toEqual(['清理', '软件', '优化', '分析', '状态']);
    expect(EXTENSIONS.map((e) => e.label)).toEqual(['计划', '活动', '恢复', 'Agent', 'AI 诊断', '设置']);
  });

  it('every entry has a unique shortcut key', () => {
    const keys = [...TOOLS.map((t) => t.key), ...EXTENSIONS.map((e) => e.key)];
    expect(new Set(keys).size).toBe(keys.length);
  });

  it('the same keys reach the same entries on both platforms (Ctrl on Windows, ⌘ on macOS)', () => {
    for (const t of TOOLS) {
      const win = shortcutFor(key(t.key, { ctrlKey: true }), false);
      const mac = shortcutFor(key(t.key, { metaKey: true }), true);
      expect(win).toEqual({ kind: 'tool', tool: t });
      expect(mac).toEqual(win);
    }
    expect(shortcutFor(key(',', { ctrlKey: true }), false)).toMatchObject({ kind: 'ext', ext: { id: 'settings' } });
    expect(shortcutFor(key('PageDown', { ctrlKey: true }), false)).toEqual({ kind: 'section', step: 1 });
    expect(shortcutFor(key('?'), false)).toEqual({ kind: 'help' });
    // The other platform's modifier does nothing (no surprise actions while typing).
    expect(shortcutFor(key('1', { metaKey: true }), false)).toBeNull();
    expect(shortcutFor(key('1', { ctrlKey: true }), true)).toBeNull();
  });

  it('every page has a place; detail pages belong to where they were opened', () => {
    for (const t of TOOLS) for (const s of t.sections) expect(placeOf(s.page, 'operations', null)).toEqual({ tool: t.id });
    expect(placeOf('activity', 'quarantine', null)).toEqual({ ext: 'recovery' });
    expect(placeOf('activity', 'scans', null)).toEqual({ ext: 'activity' });
    expect(placeOf('plan', 'operations', 'purge')).toEqual({ tool: 'clean' });
    expect(placeOf('plan', 'operations', 'startup')).toEqual({ tool: 'software' });
    expect(placeOf('operation', 'operations', 'plan')).toEqual({ ext: 'activity' });
    expect(placeOf('diag', 'operations', 'disk')).toEqual({ tool: 'analyze' });
    expect(placeOf('agent', 'operations', null)).toEqual({ ext: 'agent' });
  });
});

describe('GUI-12 platform idioms', () => {
  it('the same trash mode reads as the platform bin, keys unchanged', () => {
    setPlatform('macos');
    expect(MODE_LABEL.trash).toBe('移到废纸篓');
    expect(ACTION_LABEL.trash).toBe('移到废纸篓');
    expect(AVAILABILITY_LABEL.system_recycle_bin).toContain('废纸篓');
    expect(PLATFORM_WORDS.macos.reveal).toBe('在访达中显示');
    setPlatform('windows');
    expect(MODE_LABEL.trash).toBe('移到回收站');
    expect(PLATFORM_WORDS.windows.reveal).toBe('在资源管理器中显示');
    expect(Object.keys(MODE_LABEL)).toEqual(['trash', 'quarantine', 'delete']);
  });
});
