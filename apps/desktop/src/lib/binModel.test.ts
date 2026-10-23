import { afterEach, describe, expect, it } from 'vitest';
import { binGroups, binRefusalText, binSelectable } from './binModel';
import { setPlatform } from './platform';
import type { BinItem } from './types';
import { t } from '../i18n';
import winBin from '../../../../crates/cm-platform/src/windows/recycle_bin.rs?raw';
import macTrash from '../../../../crates/cm-platform/src/macos/actions/trash.rs?raw';
import hostSuite from '../../src-tauri/src/suite.rs?raw';

afterEach(() => setPlatform('windows'));

const item = (over: Partial<BinItem>): BinItem => ({ trash_item_id: 'tb_1', bin_id: 'bin_c', name: 'a.bin', original_path_display: 'C:\\a.bin', bin_location_display: 'C:\\$Recycle.Bin\\x', deleted_at: null, object_kind: 'file', logical_bytes: '1', purgeable: true, refusal: null, ...over });

describe('WFA-65: the bin page', () => {
  it('lists the host command the page reads and plans with (suite.rs)', () => {
    expect(hostSuite).toContain('pub async fn trash_list(');
    expect(hostSuite).toContain('pub async fn trash_purge_plan(');
  });

  it('groups items by bin in the host order; a bin without items on the page but with a count stays', () => {
    const g = binGroups({
      bins: [{ bin_id: 'bin_c', volume_display: 'C:\\', item_count: 2 }, { bin_id: 'bin_d', volume_display: 'D:\\', item_count: 0 }],
      items: [item({ trash_item_id: '1' }), item({ trash_item_id: '2', bin_id: 'bin_e' }), item({ trash_item_id: '3' })],
    });
    expect(g.map((x) => [x.bin_id, x.items.map((i) => i.trash_item_id)])).toEqual([['bin_c', ['1', '3']], ['bin_e', ['2']]]);
  });

  it('one plan, one bin: once an item is ticked only its bin stays open; refused items never', () => {
    expect(binSelectable(item({}), null)).toBe(true);
    expect(binSelectable(item({}), 'bin_c')).toBe(true);
    expect(binSelectable(item({}), 'bin_d')).toBe(false);
    expect(binSelectable(item({ purgeable: false }), null)).toBe(false);
  });

  it('every "cannot purge" detail the hosts write reads in words, never as the English detail', () => {
    const details = [...winBin.matchAll(/Some\("([^"]+)"\.to_string\(\)\)/g), ...macTrash.matchAll(/return Some\((?:format!\()?"([^"]+)"/g)].map((m) => m[1]);
    expect(details.length).toBeGreaterThanOrEqual(6);
    const other = t('binModel.refusal.other', { fileManager: '资源管理器', bin: '回收站' });
    for (const d of details) {
      const text = binRefusalText(d)!;
      expect(text, d).not.toBe(other);
      expect(/[\u4e00-\u9fff]/.test(text), d).toBe(true);
    }
    // Each detail reads as its own reason (a file with hard links is not "a link").
    expect(binRefusalText('contains a file with several hard links; deleting one name would not free its data')).toBe(t('binModel.refusal.hardLinks'));
    expect(binRefusalText('contains a link or special object; empty it with Explorer (Recycle Bin, Delete) instead')).toContain('链接');
    expect(binRefusalText('contains a cloud placeholder (dataless), which is never downloaded or touched; empty the Trash in Finder instead')).toContain('云端');
    expect(binRefusalText('something new')).toBe(other);
    expect(binRefusalText(null)).toBeNull();
    setPlatform('macos');
    expect(binRefusalText('part of it could not be read; empty the Trash in Finder instead')).toContain('访达');
  });
});
