// 状态 › 常驻工具 (RES-01..04, RES-07; add-mole-parity-suite 15.1/15.2). The tray is off until the person turns
// it on; the page shows exactly what the tray menu contains (readings and links, no write actions), how it starts
// and how it is removed. Keep-awake, clean screen, the camera/microphone indicator and battery levels sit below.
// The page reads privacy and battery state only while it is open.

import type { App } from '../main';
import { formatBytes } from '../lib/bytes';
import { appMark, h, keepFocus, replace } from '../lib/dom';
import { formatTime } from '../lib/format';
import { RequestGate } from '../lib/gate';
import type { IpcError } from '../lib/types';
import type { Batteries, BatteryAlert, CleanScreenExit, DeviceBattery, PrivacySnapshot, ResidentSettings, ResidentStatus, TraySummary } from '../lib/residentTypes';
import { noticeLine, toIpcError } from './kit';
import { emptyState, errorLine, pageHead, pill } from './parts';

const POLL_MS = 3000;

/** A registry path that may wrap after each backslash (never inside a key name). */
function breakable(path: string): (string | HTMLElement)[] {
  return path.split('\\').flatMap((part, i) => (i === 0 ? [part] : [h('wbr', {}), `\\${part}`]));
}

/** The tray's read-only lines, worded exactly like the native menu (unavailable says so, never 0). */
export function trayLines(s: TraySummary | null): [string, string][] {
  const pct = (v: number | null | undefined) => (v === null || v === undefined ? '不可用' : `${Math.round(v)}%`);
  if (!s) return [['处理器', '—'], ['内存已用', '—'], ['系统盘可用', '—']];
  return [
    ['处理器', pct(s.cpu_percent)],
    ['内存已用', pct(s.memory_percent)],
    ['系统盘可用', s.disk_free_bytes === null ? '不可用' : formatBytes(BigInt(Math.round(s.disk_free_bytes)))],
  ];
}

export function privacyLine(p: PrivacySnapshot | null): string {
  if (!p) return '摄像头和麦克风 —';
  if (!p.supported) return '摄像头和麦克风 不可用';
  if (!p.in_use.length) return '摄像头和麦克风 未在使用';
  return `正在使用：${p.in_use.map((a) => a.name).join('、')}`;
}

export function remainingText(seconds: number | null): string {
  if (seconds === null) return '直到你结束或退出 CleanManager';
  if (seconds < 60) return `剩余 ${seconds} 秒`;
  const m = Math.ceil(seconds / 60);
  if (m < 60) return `剩余 ${m} 分钟`;
  return `剩余 ${Math.floor(m / 60)} 小时${m % 60 ? ` ${m % 60} 分钟` : ''}`;
}

export const EXIT_TEXT: Record<CleanScreenExit, string> = {
  esc_held: '已退出清洁屏幕：按住了 Esc。',
  timeout: '已到时，清洁屏幕自动退出。',
  closed: '清洁屏幕已关闭。',
};

export const NOT_HERE: Record<string, [string, string]> = {
  fan_control: ['风扇控制', 'Windows 没有公开的风扇控制接口。风扇读数见“系统状态”。'],
  charge_limit: ['电池充电上限', 'Windows 没有统一的充电上限接口，请使用电脑厂商的工具或固件设置。'],
  low_latency_network: ['网络低延迟', '需要受限的特权 helper，本版本不安装。'],
  touch_id_sudo: ['Touch ID 管理员认证', '只在 macOS 上提供。'],
};

const QUICK_LINKS = ['打开系统状态', '打开进程', '打开清理', '常驻工具设置'];
const AWAKE_CHOICES: [number | null, string][] = [
  [30, '30 分钟'],
  [60, '1 小时'],
  [120, '2 小时'],
  [null, '直到结束'],
];
const CLEAN_CHOICES: [number, string][] = [
  [30, '30 秒'],
  [60, '1 分钟'],
  [120, '2 分钟'],
];

export class ResidentPage {
  el: HTMLElement;
  private body: HTMLElement;
  private status: ResidentStatus | null = null;
  private privacy: PrivacySnapshot | null = null;
  private batteries: Batteries | null = null;
  private error: IpcError | null = null;
  private saveError: IpcError | null = null;
  private awakeError: IpcError | null = null;
  private cleanError: IpcError | null = null;
  private busy: 'save' | 'awake' | 'clean' | null = null;
  private awakeChoice: number | null = 60;
  private cleanChoice = 60;
  private cleanResult: CleanScreenExit | null = null;
  private alerts: BatteryAlert[] = [];
  private threshold: number | null = null;
  private gate = new RequestGate();
  private timer: number | null = null;
  private ticks = 0;

  constructor(private app: App) {
    this.body = h('div', {});
    this.el = h('section', { class: 'page page-resident', 'aria-labelledby': 'resident-title' }, this.body);
  }

  activate(): void {
    void this.load(true);
    this.stop();
    this.timer = window.setInterval(() => void this.load(false), POLL_MS);
  }

  update(): void {
    if (this.app.store.state.page !== 'resident') this.stop();
    keepFocus(this.el, () => this.render());
  }

  /** Alerts raised by the tray loop (event from the host). */
  alerted(alerts: BatteryAlert[]): void {
    this.alerts = [...alerts, ...this.alerts].slice(0, 4);
    if (this.app.store.state.page === 'resident') this.update();
  }

  /** The tray changed from its own menu (turned off there). */
  changed(): void {
    if (this.app.store.state.page === 'resident') void this.load(true);
  }

  private stop(): void {
    if (this.timer !== null) window.clearInterval(this.timer);
    this.timer = null;
  }

  private async load(full: boolean): Promise<void> {
    if (this.app.store.state.page !== 'resident') return this.stop();
    const b = this.app.backend;
    try {
      const r = await this.gate.latest(Promise.all([b.residentStatus(), b.residentPrivacy(), full || this.ticks % 5 === 0 ? b.residentBatteries() : Promise.resolve(this.batteries)]));
      this.ticks += 1;
      if (r.stale) return;
      [this.status, this.privacy, this.batteries] = r.value;
      this.error = null;
    } catch (e) {
      this.error = toIpcError(e);
    }
    if (this.app.store.state.page === 'resident') this.update();
  }

  private async save(next: ResidentSettings, said: string): Promise<void> {
    if (this.busy) return;
    this.busy = 'save';
    this.saveError = null;
    this.update();
    try {
      this.status = await this.app.backend.residentSave(next);
      this.app.announce(said);
    } catch (e) {
      this.saveError = toIpcError(e);
    } finally {
      this.busy = null;
      this.update();
    }
  }

  private async keepAwake(on: boolean): Promise<void> {
    if (this.busy) return;
    this.busy = 'awake';
    this.awakeError = null;
    this.update();
    try {
      const k = await this.app.backend.residentKeepAwake(on, on ? this.awakeChoice : null);
      if (this.status) this.status = { ...this.status, keep_awake: k };
      this.app.announce(on ? `已开始保持亮屏，${remainingText(k.remaining_seconds)}` : '已结束保持亮屏，恢复系统的息屏与睡眠设置');
    } catch (e) {
      this.awakeError = toIpcError(e);
    } finally {
      this.busy = null;
      this.update();
    }
  }

  private async cleanScreen(): Promise<void> {
    if (this.busy) return;
    this.busy = 'clean';
    this.cleanError = null;
    this.cleanResult = null;
    this.update();
    try {
      this.cleanResult = await this.app.backend.residentCleanScreen(this.cleanChoice);
      this.app.announce(EXIT_TEXT[this.cleanResult]);
    } catch (e) {
      this.cleanError = toIpcError(e);
    } finally {
      this.busy = null;
      this.update();
    }
  }

  private render(): void {
    const s = this.status;
    if (s && !s.supported) {
      replace(this.body, pageHead({ id: 'resident-title', title: '常驻工具' }), emptyState('此平台暂不提供', '托盘、保持亮屏、清洁屏幕、隐私提示与电量目前只在 Windows 上实现。'));
      return;
    }
    replace(
      this.body,
      pageHead({
        id: 'resident-title',
        title: '常驻工具',
        meta: [s ? (s.tray.enabled ? '托盘已开启' : '托盘已关闭') : '正在读取…', '只显示读数和打开页面，不执行清理、卸载或维护'],
      }),
      errorLine(this.error),
      ...this.alerts.map((a, i) =>
        h(
          'div',
          { class: 'notice warn alert-line', role: 'status' },
          h('span', {}, `电量低：${a.name} ${a.percent}%，低于提醒阈值 ${a.threshold}%`),
          h('button', { type: 'button', class: 'quiet', onclick: () => ((this.alerts = this.alerts.filter((_, j) => j !== i)), this.update()) }, '知道了'),
        ),
      ),
      s ? this.trayPanel(s) : !this.error ? h('p', { class: 'muted', role: 'status' }, '正在读取常驻工具状态…') : null,
      s ? h('div', { class: 'res-grid' }, this.awakePanel(s), this.cleanPanel(s), this.privacyPanel(), this.batteryPanel(s)) : null,
      s?.not_here.length ? this.notHerePanel(s) : null,
    );
  }

  private switchRow(id: string, checked: boolean, disabled: boolean, title: string, note: string, onChange: (v: boolean) => void): HTMLElement {
    const input = h('input', { type: 'checkbox', role: 'switch', id, disabled }) as HTMLInputElement;
    input.checked = checked;
    input.addEventListener('change', () => onChange(input.checked));
    return h('label', { class: 'switch-row', for: id }, input, h('span', {}, h('b', {}, title), h('span', { class: 'muted small' }, note)));
  }

  private trayPanel(s: ResidentStatus): HTMLElement {
    const set = s.settings;
    const busy = this.busy === 'save';
    const tray = this.switchRow('res-tray', set.tray, busy, '在通知区域显示 CleanManager', `默认关闭。开启后每 ${s.tray.interval_seconds} 秒采样一次处理器、内存和系统盘可用空间；关闭即停止采样、移除图标和登录启动项。`, (v) =>
      void this.save(v ? { ...set, tray: true } : { ...set, tray: false, start_at_login: false }, v ? '托盘已开启' : '托盘已关闭：已停止采样并移除图标与登录启动项'),
    );
    const login = this.switchRow('res-login', set.start_at_login, busy || !set.tray, '登录 Windows 时只启动托盘', set.tray ? '在当前用户的启动项中写入 CleanManager.Resident，关闭这个开关或托盘时删除。' : '需要先开启托盘。', (v) =>
      void this.save({ ...set, start_at_login: v }, v ? '已添加登录启动项' : '已删除登录启动项'),
    );
    const t = s.tray;
    return h(
      'section',
      { class: 'panel tray-panel', 'aria-labelledby': 'tray-title' },
      h(
        'div',
        { class: 'tray-main' },
        h('h2', { id: 'tray-title' }, '托盘'),
        h('div', { class: 'switches' }, tray, login),
        errorLine(this.saveError),
        h(
          'dl',
          { class: 'facts-list tray-facts' },
          h('dt', {}, '状态'),
          h('dd', {}, t.enabled ? pill(t.sampling ? '已开启，正在采样' : '已开启', 'good') : pill('已关闭，不采样', 'muted')),
          h('dt', {}, '启动方式'),
          h('dd', {}, t.startup === 'at_login' ? '登录时启动（只显示托盘）' : '打开 CleanManager 时'),
          h('dt', {}, '登录启动项'),
          h('dd', {}, h('span', { class: 'path reg-path' }, ...breakable(t.startup_location)), h('small', {}, t.startup_command ? `已创建：${t.startup_command}` : '未创建')),
          h('dt', {}, '版本'),
          h('dd', { class: 'num' }, t.version),
          h('dt', {}, '移除'),
          h('dd', {}, '关闭托盘即移除图标与登录启动项。卸载 CleanManager（self uninstall）时也会列出并删除这个启动项。'),
        ),
      ),
      this.trayPreview(s),
    );
  }

  /** What the tray menu holds, drawn with the live readings: the whole menu, nothing more. */
  private trayPreview(s: ResidentStatus): HTMLElement {
    const on = s.tray.enabled;
    const lines = trayLines(on ? s.seen.summary : null);
    const sampled = on && s.seen.summary ? formatTime(s.seen.summary.sampled_at) : null;
    return h(
      'figure',
      { class: 'tray-preview', 'data-on': on ? 'yes' : 'no', 'aria-labelledby': 'tray-preview-cap' },
      h(
        'div',
        { class: 'tp-menu', 'aria-hidden': 'true' },
        h('p', { class: 'tp-head' }, 'CleanManager 状态'),
        h('ul', { class: 'tp-read' }, ...lines.map(([k, v]) => h('li', {}, h('span', {}, k), h('b', { class: 'num' }, v))), h('li', { class: 'tp-privacy' }, h('span', {}, privacyLine(on ? (s.seen.privacy ?? this.privacy) : null)))),
        h('ul', { class: 'tp-links' }, ...QUICK_LINKS.map((l) => h('li', {}, l))),
        h('ul', { class: 'tp-links' }, h('li', {}, '关闭常驻组件'), h('li', {}, '退出 CleanManager')),
      ),
      h('div', { class: 'tp-bar', 'aria-hidden': 'true' }, h('span', { class: 'tp-chevron' }), h('span', { class: 'tp-icon' }, appMark(16)), h('span', { class: 'tp-clock num' }, sampled ? sampled.slice(-5) : '')),
      h('figcaption', { id: 'tray-preview-cap', class: 'muted small' }, on ? `托盘菜单的全部内容：读数和打开页面，没有清理入口。${sampled ? `读数采样于 ${sampled}。` : ''}` : '开启后，托盘菜单只会显示这些内容：读数和打开页面，没有清理入口。'),
    );
  }

  private seg<T>(name: string, label: string, choices: [T, string][], current: T, disabled: boolean, onPick: (v: T) => void): HTMLElement {
    return h(
      'fieldset',
      { class: 'seg', 'aria-label': label, disabled },
      ...choices.map(([v, text], i) => {
        const input = h('input', { type: 'radio', name, id: `${name}-${i}` }) as HTMLInputElement;
        input.checked = v === current;
        input.addEventListener('change', () => {
          onPick(v);
          this.update();
        });
        return h('label', { for: `${name}-${i}` }, input, h('span', {}, text));
      }),
    );
  }

  private awakePanel(s: ResidentStatus): HTMLElement {
    const k = s.keep_awake;
    const busy = this.busy === 'awake';
    const go = h('button', { type: 'button', class: k.active ? '' : 'primary', id: 'awake-go', disabled: busy }, k.active ? '结束保持亮屏' : '开始保持亮屏');
    go.addEventListener('click', () => void this.keepAwake(!k.active));
    return h(
      'section',
      { class: 'panel res-card', 'aria-labelledby': 'awake-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'awake-title' }, '保持亮屏'), k.active ? pill('保持中', 'busy') : null),
      k.active
        ? h('p', { class: 'res-live', role: 'status' }, h('b', { class: 'num' }, remainingText(k.remaining_seconds)))
        : h('div', { class: 'res-row' }, h('span', { class: 'res-label' }, '时长'), this.seg('awake-len', '保持亮屏的时长', AWAKE_CHOICES, this.awakeChoice, busy, (v) => (this.awakeChoice = v))),
      h('div', { class: 'res-actions' }, go),
      errorLine(this.awakeError),
      h('p', { class: 'muted small res-foot' }, '向系统提出临时请求（与播放视频时相同），不修改电源计划、息屏或睡眠设置。到时、点“结束”或退出 CleanManager 时自动撤销。'),
    );
  }

  private cleanPanel(s: ResidentStatus): HTMLElement {
    const running = this.busy === 'clean' || s.clean_screen.running;
    const go = h('button', { type: 'button', class: 'primary', id: 'clean-go', disabled: running || this.busy !== null }, running ? '清洁屏幕进行中…' : '开始清洁屏幕');
    go.addEventListener('click', () => void this.cleanScreen());
    const hold = Math.round(s.clean_screen.esc_hold_ms / 1000);
    return h(
      'section',
      { class: 'panel res-card', 'aria-labelledby': 'clean-title' },
      h('h2', { id: 'clean-title' }, '清洁屏幕'),
      h('p', { class: 'small' }, '所有显示器变黑，键盘按键和鼠标点击暂停，方便擦拭屏幕和键盘。'),
      h('div', { class: 'res-row' }, h('span', { class: 'res-label' }, '自动退出'), this.seg('clean-len', '多久后自动退出', CLEAN_CHOICES, this.cleanChoice, running, (v) => (this.cleanChoice = v))),
      h(
        'ul',
        { class: 'res-exits' },
        h('li', {}, '按住 ', h('kbd', {}, 'Esc'), ` ${hold} 秒退出`),
        h('li', {}, '到时自动退出并恢复输入'),
        h('li', {}, h('kbd', {}, 'Ctrl'), '+', h('kbd', {}, 'Alt'), '+', h('kbd', {}, 'Del'), ' 始终可用'),
      ),
      h('div', { class: 'res-actions' }, go),
      errorLine(this.cleanError),
      this.cleanResult ? noticeLine(EXIT_TEXT[this.cleanResult]) : null,
    );
  }

  private privacyPanel(): HTMLElement {
    const p = this.privacy;
    const CAP: Record<string, string> = { camera: '摄像头', microphone: '麦克风' };
    return h(
      'section',
      { class: 'panel res-card', 'aria-labelledby': 'privacy-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'privacy-title' }, '摄像头与麦克风'), p && !p.supported ? pill('不可用', 'muted') : null),
      !p
        ? h('p', { class: 'muted small', role: 'status' }, '正在读取…')
        : !p.supported
          ? h('p', { class: 'small' }, `无法读取系统的使用记录：${p.unavailable_reason ?? '原因未知'}`)
          : p.in_use.length
            ? h(
                'ul',
                { class: 'res-apps', 'aria-live': 'polite' },
                ...p.in_use.map((a) => h('li', {}, h('span', { class: 'res-app-name' }, a.name), h('span', { class: 'res-app-caps' }, ...a.capabilities.map((c) => pill(`正在使用${CAP[c] ?? c}`, 'warn'))))),
              )
            : h('p', { class: 'res-quiet', 'aria-live': 'polite' }, '现在没有应用在使用摄像头或麦克风。'),
      h('p', { class: 'muted small res-foot' }, `本页打开时每 ${POLL_MS / 1000} 秒读取一次 Windows 的使用记录；托盘开启时随托盘读取。只在本机显示，不保存、不上传。`),
    );
  }

  private batteryRow(d: DeviceBattery, threshold: number | null): HTMLElement {
    const bar = h('span', { class: 'res-meter' }, h('span', { class: 'res-meter-fill' }));
    (bar.firstChild as HTMLElement).style.width = `${Math.max(0, Math.min(100, d.percent))}%`;
    const low = threshold !== null && d.percent < threshold && d.charging !== true;
    const note = d.charging === true ? '正在充电或使用电源' : d.charging === false ? '使用电池' : d.kind === 'bluetooth' ? '蓝牙设备' : '';
    return h('li', { 'data-low': low ? 'yes' : 'no' }, h('span', { class: 'res-batt-name' }, d.name, note ? h('small', { class: 'muted' }, note) : null), bar, h('b', { class: 'num res-batt-pct' }, `${d.percent}%`));
  }

  private batteryPanel(s: ResidentStatus): HTMLElement {
    const b = this.batteries;
    const set = s.settings;
    const threshold = set.battery_alerts && set.tray ? set.battery_threshold : null;
    const alerts = this.switchRow('res-batt-alerts', set.battery_alerts && set.tray, this.busy === 'save' || !set.tray, '低电量提醒', set.tray ? `低于阈值时提醒一次，回升 5 个百分点后重新计算；由托盘每 ${s.tray.interval_seconds} 秒检查。` : '需要先开启托盘：关闭托盘时不在后台检查电量。', (v) =>
      void this.save({ ...set, battery_alerts: v }, v ? '已开启低电量提醒' : '已关闭低电量提醒'),
    );
    const input = h('input', { type: 'number', id: 'res-threshold', min: '5', max: '50', step: '1', value: String(this.threshold ?? set.battery_threshold), class: 'num-input', disabled: !set.battery_alerts || !set.tray || this.busy === 'save' }) as HTMLInputElement;
    input.addEventListener('change', () => {
      const v = Math.round(Number(input.value));
      if (!(v >= 5 && v <= 50)) {
        this.threshold = null;
        this.update();
        return;
      }
      this.threshold = v;
      void this.save({ ...set, battery_threshold: v }, `提醒阈值已设为 ${v}%`).then(() => (this.threshold = null));
    });
    const devices = b ? [...(b.system ? [b.system] : []), ...b.devices] : [];
    const btNote = !b
      ? null
      : b.devices_unavailable
        ? `设备电量不可读：${b.devices_unavailable}`
        : b.devices.length
          ? null
          : b.bluetooth_devices === 0
            ? '没有已连接的蓝牙设备。'
            : `${b.bluetooth_devices} 个蓝牙设备没有向 Windows 报告电量，这里不做估计。`;
    return h(
      'section',
      { class: 'panel res-card', 'aria-labelledby': 'battery-title' },
      h('h2', { id: 'battery-title' }, '电量'),
      !b ? h('p', { class: 'muted small', role: 'status' }, '正在读取…') : devices.length ? h('ul', { class: 'res-batt' }, ...devices.map((d) => this.batteryRow(d, threshold))) : h('p', { class: 'small' }, '这台电脑没有系统电池。'),
      btNote ? h('p', { class: 'small res-bt-note' }, btNote) : null,
      h('div', { class: 'res-alerts' }, alerts, h('label', { class: 'num-field', for: 'res-threshold' }, h('span', {}, '提醒阈值'), h('span', { class: 'num-wrap' }, input, h('span', { class: 'muted small' }, '%')))),
      h('p', { class: 'muted small res-foot' }, '蓝牙设备电量来自 Windows 蓝牙协议栈为设备记录的电量值（与“设置 › 蓝牙和其他设备”相同）；没有记录的设备不显示数字。'),
    );
  }

  private notHerePanel(s: ResidentStatus): HTMLElement {
    return h(
      'section',
      { class: 'panel res-nothere', 'aria-labelledby': 'nothere-title' },
      h('h2', { id: 'nothere-title' }, '此平台不提供'),
      h(
        'ul',
        { class: 'res-nothere-list' },
        ...s.not_here.map((n) => {
          const [label, why] = NOT_HERE[n.id] ?? [n.id, n.reason];
          return h('li', {}, h('span', { class: 'res-nh-name' }, label, pill('此平台不适用', 'muted')), h('span', { class: 'muted small' }, why));
        }),
      ),
    );
  }
}
