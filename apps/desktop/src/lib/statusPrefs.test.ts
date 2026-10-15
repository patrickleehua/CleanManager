import { describe, expect, it } from 'vitest';
import { networkScope } from '../views/tools';
import { DEFAULT_PREFS, PREFS_KEY, layoutRows, loadPrefs, movePanel, savePrefs, sanitize, setShown, updatePrefs } from './statusPrefs';
import type { NetworkTotals } from './types';

/** An in-memory stand-in for the window's localStorage; a new `loadPrefs` on it is a new session. */
function memoryStorage(): Storage & { data: Map<string, string> } {
  const data = new Map<string, string>();
  return {
    data,
    get length() {
      return data.size;
    },
    clear: () => data.clear(),
    getItem: (k) => data.get(k) ?? null,
    key: (i) => [...data.keys()][i] ?? null,
    removeItem: (k) => void data.delete(k),
    setItem: (k, v) => void data.set(k, String(v)),
  };
}

describe('状态显示偏好 (STS-07)', () => {
  it('starts from the defaults when nothing is stored', () => {
    expect(loadPrefs(memoryStorage())).toEqual(DEFAULT_PREFS);
    expect(loadPrefs(null)).toEqual(DEFAULT_PREFS);
  });

  it('a hidden panel, new order, process sort and refresh interval survive a restart', () => {
    const storage = memoryStorage();
    let p = loadPrefs(storage);
    p = setShown(p, 'metrics', false);
    p = movePanel(p, 'network', -1);
    p = { ...p, refreshSeconds: 10 };
    expect(savePrefs(p, storage)).toBe(true);
    updatePrefs((x) => ({ ...x, processSort: 'memory' }), storage);
    // Next session: only what was stored is read back.
    const restored = loadPrefs(storage);
    expect(restored).toEqual({ order: ['health', 'network', 'metrics'], hidden: ['metrics'], refreshSeconds: 10, processSort: 'memory' });
    expect(layoutRows(restored)).toEqual([{ shape: 'narrow-wide', panels: ['health', 'network'] }]);
  });

  it('the process page and the status page do not overwrite each other', () => {
    const storage = memoryStorage();
    updatePrefs((x) => setShown(x, 'health', false), storage);
    updatePrefs((x) => ({ ...x, processSort: 'name' }), storage);
    expect(loadPrefs(storage)).toMatchObject({ hidden: ['health'], processSort: 'name' });
  });

  it('corrupt or foreign records fall back field by field', () => {
    const storage = memoryStorage();
    storage.setItem(PREFS_KEY, '{not json');
    expect(loadPrefs(storage)).toEqual(DEFAULT_PREFS);
    storage.setItem(PREFS_KEY, JSON.stringify({ order: ['network', 'gone', 'network'], hidden: ['metrics', 'gone'], refreshSeconds: 7, processSort: 'network' }));
    // Unknown and duplicate panels are dropped, missing ones appended; unsupported values reset.
    expect(loadPrefs(storage)).toEqual({ order: ['network', 'health', 'metrics'], hidden: ['metrics'], refreshSeconds: 3, processSort: 'cpu' });
    expect(sanitize([1, 2])).toEqual(DEFAULT_PREFS);
  });

  it('blocked storage keeps the page working and says the choice was not kept', () => {
    const blocked = { getItem: () => null, setItem: () => { throw new Error('QuotaExceededError'); } };
    expect(savePrefs(DEFAULT_PREFS, blocked)).toBe(false);
    expect(savePrefs(DEFAULT_PREFS, null)).toBe(false);
  });

  it('moving stops at the ends and rows follow the saved order', () => {
    expect(movePanel(DEFAULT_PREFS, 'health', -1)).toBe(DEFAULT_PREFS);
    expect(movePanel(DEFAULT_PREFS, 'network', 1)).toBe(DEFAULT_PREFS);
    expect(layoutRows(DEFAULT_PREFS)).toEqual([
      { shape: 'narrow-wide', panels: ['health', 'metrics'] },
      { shape: 'full', panels: ['network'] },
    ]);
    const healthLast = movePanel(movePanel(DEFAULT_PREFS, 'health', 1), 'health', 1);
    expect(healthLast.order).toEqual(['metrics', 'network', 'health']);
    expect(layoutRows(healthLast)).toEqual([
      { shape: 'full', panels: ['metrics'] },
      { shape: 'wide-narrow', panels: ['network', 'health'] },
    ]);
  });
});

describe('网络合计说明 (STS-07)', () => {
  const base: NetworkTotals = {
    classification: 'interface_type',
    received_bytes_per_sec: 1,
    sent_bytes_per_sec: 1,
    may_double_count: false,
    counted: [],
    separate: [],
    loopback_excluded: 1,
    note: '',
  };
  it('says what the total covers, and when it may double count', () => {
    expect(networkScope(base)).toContain('只计物理网卡');
    expect(networkScope(base)).toContain('已排除回环');
    expect(networkScope({ ...base, may_double_count: true })).toContain('可能被重复计算');
    expect(networkScope({ ...base, classification: 'unavailable', may_double_count: true })).toContain('无法区分网卡类型');
  });
});
