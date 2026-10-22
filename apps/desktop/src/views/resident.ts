// 状态 › 常驻工具 (RES-01..04, RES-07; add-mole-parity-suite 15.1/15.2). The tray is off until the person turns
// it on; the page shows exactly what the tray menu contains (readings and links, no write actions), how it starts
// and how it is removed. Keep-awake, clean screen, the camera/microphone indicator and battery levels sit below.
// The page reads privacy and battery state only while it is open.
// macOS (GUI-12): the tray is a menu-bar status item; start at login is not offered by this unsigned build (the host
// lists it under `not_here`), keep-awake shows the IOPMAssertion the system lists, the camera is reported without an
// app name (macOS does not give one), and peripherals without a battery reading are listed as unavailable, never 0%.

import type { App } from '../main';
import { formatBytes } from '../lib/bytes';
import { appMark, h, keepFocus, replace } from '../lib/dom';
import { formatTime } from '../lib/format';
import { RequestGate } from '../lib/gate';
import type { IpcError } from '../lib/types';
import { notOnThisPlatform, platform, words } from '../lib/platform';
import type { Batteries, BatteryAlert, CleanScreenExit, DeviceBattery, PrivacySnapshot, ResidentSettings, ResidentStatus, TraySummary } from '../lib/residentTypes';
import { noticeLine, toIpcError } from './kit';
import { emptyState, errorLine, pageHead, pill } from './parts';
import { t, upperFirst } from '../i18n';

const POLL_MS = 3000;

/** A registry path that may wrap after each backslash (never inside a key name). */
function breakable(path: string): (string | HTMLElement)[] {
  return path.split('\\').flatMap((part, i) => (i === 0 ? [part] : [h('wbr', {}), `\\${part}`]));
}

/** The tray's read-only lines, worded exactly like the native menu (unavailable says so, never 0). */
export function trayLines(s: TraySummary | null): [string, string][] {
  const pct = (v: number | null | undefined) => (v === null || v === undefined ? t('common.unavailable') : `${Math.round(v)}%`);
  if (!s) return [[t('common.cpu'), '—'], [t('common.memoryUsed'), '—'], [t('common.systemDiskFree'), '—']];
  return [
    [t('common.cpu'), pct(s.cpu_percent)],
    [t('common.memoryUsed'), pct(s.memory_percent)],
    [t('common.systemDiskFree'), s.disk_free_bytes === null ? t('common.unavailable') : formatBytes(BigInt(Math.round(s.disk_free_bytes)))],
  ];
}

export function privacyLine(p: PrivacySnapshot | null): string {
  if (!p) return t('resident.cameraMicrophone');
  if (!p.supported) return t('resident.cameraMicrophoneUnavailable');
  if (!p.in_use.length) return t('resident.cameraMicrophoneNotUse');
  return t('resident.useJoin', { join: p.in_use.map((a) => a.name).join(t('common.listSep')) });
}

export function remainingText(seconds: number | null): string {
  if (seconds === null) return t('resident.untilEndQuitCleanmanager');
  if (seconds < 60) return t('resident.secondsLeft', { seconds });
  const m = Math.ceil(seconds / 60);
  if (m < 60) return t('resident.mMinLeft', { m });
  return t('resident.floorHPLeft', { floor: Math.floor(m / 60), p: m % 60 ? t('resident.pMin', { p: m % 60 }) : '' });
}

export const EXIT_TEXT: Record<CleanScreenExit, string> = {
  get esc_held() { return t('resident.esc_held'); },
  get timeout() { return t('resident.timeout'); },
  get closed() { return t('resident.closed'); },
};

export const NOT_HERE: Record<string, [string, string]> = {
  get fan_control(): [string, string] { return [t('resident.notHere.fan_control.title'), t('resident.notHere.fan_control.reason')]; },
  get charge_limit(): [string, string] { return [t('resident.notHere.charge_limit.title'), t('resident.notHere.charge_limit.reason')]; },
  get low_latency_network(): [string, string] { return [t('resident.notHere.low_latency_network.title'), t('resident.notHere.low_latency_network.reason')]; },
  get touch_id_sudo(): [string, string] { return [t('resident.notHere.touch_id_sudo.title'), t('resident.notHere.touch_id_sudo.reason')]; },
};

/** What the macOS host does not offer here (`*_macos` ids: their reasons differ from the Windows ones). */
export const NOT_HERE_MACOS: Record<string, [string, string]> = {
  get start_at_login_macos(): [string, string] { return [t('resident.notHereMacos.start_at_login_macos.title'), t('resident.notHereMacos.start_at_login_macos.reason')]; },
  get fan_control_macos(): [string, string] { return [t('resident.notHereMacos.fan_control_macos.title'), t('resident.notHereMacos.fan_control_macos.reason')]; },
  get charge_limit_macos(): [string, string] { return [t('resident.notHereMacos.charge_limit_macos.title'), t('resident.notHereMacos.charge_limit_macos.reason')]; },
  get low_latency_network_macos(): [string, string] { return [t('resident.notHereMacos.low_latency_network_macos.title'), t('resident.notHereMacos.low_latency_network_macos.reason')]; },
  get touch_id_sudo_macos(): [string, string] { return [t('resident.notHereMacos.touch_id_sudo_macos.title'), t('resident.notHereMacos.touch_id_sudo_macos.reason')]; },
  get bluetooth_headphones_macos(): [string, string] { return [t('resident.notHereMacos.bluetooth_headphones_macos.title'), t('resident.notHereMacos.bluetooth_headphones_macos.reason')]; },
};

/** A platform gap in words: the known ids of either platform, else the host's id and reason as sent. */
export function notHereWords(id: string, reason: string): [string, string] {
  return NOT_HERE[id] ?? NOT_HERE_MACOS[id] ?? [id, reason];
}

/** Start at login cannot be turned on here (macOS: the host lists it as not offered). */
export function loginUnsupported(s: Pick<ResidentStatus, 'not_here'>): boolean {
  return s.not_here.some((n) => n.id === 'start_at_login_macos');
}

/** Why a device has no level, in words; anything else is shown as sent. */
export function unavailableReason(reason: string): string {
  if (reason.startsWith('the device publishes no BatteryPercent')) return t('resident.deviceDoesntPublishBattery');
  if (/^BatteryPercent .* is outside 0–100/.test(reason)) return t('resident.deviceReportedLevelOutside');
  if (reason.startsWith('BatteryPercent is not a number')) return t('resident.deviceReportedLevelIsnt');
  return reason;
}

const KIND_NOTE: Record<string, string> = { get bluetooth() { return t('resident.kindNote.bluetooth'); }, get peripheral() { return t('resident.kindNote.peripheral'); } };

/** A battery's name: the built-in battery in the interface language (the host names it in Chinese), others as reported. */
export function batteryName(d: { device_id: string; name: string }): string {
  return d.device_id === 'system' ? t('resident.systemBattery') : d.name;
}

/** The battery panel's lists: devices with a level, devices without one (never 0%), and the note under them. */
export function batteryLists(b: Batteries): { devices: DeviceBattery[]; unavailable: { device_id: string; name: string; note: string; reason: string }[]; note: string | null } {
  const devices = [...(b.system ? [b.system] : []), ...b.devices];
  const unavailable = (b.unavailable_devices ?? []).map((d) => ({ device_id: d.device_id, name: d.name, note: KIND_NOTE[d.kind] ?? '', reason: unavailableReason(d.reason) }));
  const note = b.devices_unavailable
    ? t('resident.deviceBatteryLevelsCant', { devices_unavailable: b.devices_unavailable })
    : b.devices.length || unavailable.length
      ? null
      : b.bluetooth_devices === 0
        ? t('resident.noBluetoothDevicesConnected')
        : t('resident.bluetoothDevicesReportedBattery', { bluetooth_devices: b.bluetooth_devices, os: words().os });
  return { devices, unavailable, note };
}

const quickLinks = () => [t('resident.quickLinks.status'), t('resident.quickLinks.processes'), t('resident.quickLinks.cleanup'), t('resident.quickLinks.settings')];
const awakeChoices = (): [number | null, string][] => [
  [30, t('resident.awakeChoices.30min')],
  [60, t('resident.awakeChoices.1h')],
  [120, t('resident.awakeChoices.2h')],
  [null, t('resident.awakeChoices.untilStopped')],
];
const cleanChoices = (): [number, string][] => [
  [30, t('resident.cleanChoices.30s')],
  [60, t('resident.cleanChoices.1min')],
  [120, t('resident.cleanChoices.2min')],
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
      this.app.announce(on ? t('resident.keepAwakeStartedRemaining', { remaining_seconds: remainingText(k.remaining_seconds) }) : t('resident.keepAwakeEndedSystems'));
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
      replace(this.body, pageHead({ id: 'resident-title', title: t('resident.residentTools') }), emptyState(t('common.notAvailablePlatformYet'), notOnThisPlatform(t('resident.trayKeepAwakeClean'))));
      return;
    }
    const tray = words().tray;
    replace(
      this.body,
      pageHead({
        id: 'resident-title',
        title: t('resident.residentTools'),
        meta: [s ? (s.tray.enabled ? t('resident.tray', { tray }) : t('resident.trayOff', { tray })) : t('common.reading'), t('resident.onlyShowsReadingsOpens')],
      }),
      errorLine(this.error),
      ...this.alerts.map((a, i) =>
        h(
          'div',
          { class: 'notice warn alert-line', role: 'status' },
          h('span', {}, t('resident.batteryLowNamePercent', { name: batteryName(a), percent: a.percent, threshold: a.threshold })),
          h('button', { type: 'button', class: 'quiet', onclick: () => ((this.alerts = this.alerts.filter((_, j) => j !== i)), this.update()) }, t('common.got')),
        ),
      ),
      s ? this.trayPanel(s) : !this.error ? h('p', { class: 'muted', role: 'status' }, t('resident.readingResidentToolsStatus')) : null,
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
    const w = words();
    // macOS: start at login is not offered by this build; the switch stays visible, off and disabled, with the reason.
    const noLogin = loginUnsupported(s);
    const tray = this.switchRow('res-tray', set.tray, busy, w.trayShow, w.trayShowNote(s.tray.interval_seconds), (v) =>
      void this.save(v ? { ...set, tray: true } : { ...set, tray: false, start_at_login: false }, v ? t('resident.turnedTray', { tray: w.tray }) : w.trayOffSaid),
    );
    const login = this.switchRow('res-login', set.start_at_login && !noLogin, busy || !set.tray || noLogin, w.loginSwitch, noLogin ? w.loginNote : set.tray ? w.loginNote : t('resident.turnTrayFirst', { tray: w.tray }), (v) =>
      void this.save({ ...set, start_at_login: v }, v ? t('resident.loginStartupItemAdded') : t('resident.loginStartupItemDeleted')),
    );
    const tray2 = s.tray;
    return h(
      'section',
      { class: 'panel tray-panel', 'aria-labelledby': 'tray-title' },
      h(
        'div',
        { class: 'tray-main' },
        h('h2', { id: 'tray-title' }, upperFirst(w.tray)),
        h('div', { class: 'switches' }, tray, login),
        errorLine(this.saveError),
        h(
          'dl',
          { class: 'facts-list tray-facts' },
          h('dt', {}, t('common.status')),
          h('dd', {}, tray2.enabled ? pill(tray2.sampling ? t('resident.sampling') : t('resident.on'), 'good') : pill(t('resident.offNotSampling'), 'muted')),
          h('dt', {}, t('resident.launch')),
          h('dd', {}, tray2.startup === 'at_login' ? t('resident.loginShowsOnlyTray', { tray: w.tray }) : t('resident.whenCleanmanagerOpens')),
          h('dt', {}, noLogin ? t('resident.loginItem') : t('resident.loginStartupItem')),
          noLogin
            ? h('dd', {}, t('resident.unavailableVersionUnsignedCant'), h('small', {}, t('resident.noStartupItemCreated')))
            : h('dd', {}, h('span', { class: 'path reg-path' }, ...breakable(tray2.startup_location)), h('small', {}, tray2.startup_command ? t('resident.createdStartupCommand', { startup_command: tray2.startup_command }) : t('resident.notCreated'))),
          h('dt', {}, t('resident.version')),
          h('dd', { class: 'num' }, tray2.version),
          h('dt', {}, t('resident.removal')),
          h('dd', {}, w.trayRemoval),
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
    const menu = words().trayMenu;
    // The menu bar has no overflow chevron; the notification area does.
    const mac = platform() === 'macos';
    return h(
      'figure',
      { class: 'tray-preview', 'data-on': on ? 'yes' : 'no', 'aria-labelledby': 'tray-preview-cap' },
      h(
        'div',
        { class: 'tp-menu', 'aria-hidden': 'true' },
        h('p', { class: 'tp-head' }, t('resident.cleanmanagerStatus')),
        h('ul', { class: 'tp-read' }, ...lines.map(([k, v]) => h('li', {}, h('span', {}, k), h('b', { class: 'num' }, v))), h('li', { class: 'tp-privacy' }, h('span', {}, privacyLine(on ? (s.seen.privacy ?? this.privacy) : null)))),
        h('ul', { class: 'tp-links' }, ...quickLinks().map((l) => h('li', {}, l))),
        h('ul', { class: 'tp-links' }, h('li', {}, t('resident.turnOffResidentComponent')), h('li', {}, t('resident.quitCleanmanager'))),
      ),
      h('div', { class: 'tp-bar', 'aria-hidden': 'true' }, mac ? null : h('span', { class: 'tp-chevron' }), h('span', { class: 'tp-icon' }, appMark(16)), h('span', { class: 'tp-clock num' }, sampled ? sampled.slice(-5) : '')),
      h('figcaption', { id: 'tray-preview-cap', class: 'muted small' }, on ? t('resident.everythingMenuShowsReadings', { menu, p: sampled ? t('resident.readingsSampledSampled', { sampled }) : '' }) : t('resident.whenMenuShowsOnly', { menu })),
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
    const go = h('button', { type: 'button', class: k.active ? '' : 'primary', id: 'awake-go', disabled: busy }, k.active ? t('resident.stopKeepingAwake') : t('resident.startKeepingAwake'));
    go.addEventListener('click', () => void this.keepAwake(!k.active));
    return h(
      'section',
      { class: 'panel res-card', 'aria-labelledby': 'awake-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'awake-title' }, t('resident.keepAwake')), k.active ? pill(t('resident.active'), 'busy') : null),
      k.active
        ? h('p', { class: 'res-live', role: 'status' }, h('b', { class: 'num' }, remainingText(k.remaining_seconds)))
        : h('div', { class: 'res-row' }, h('span', { class: 'res-label' }, t('resident.duration')), this.seg('awake-len', t('resident.howLongKeepAwake'), awakeChoices(), this.awakeChoice, busy, (v) => (this.awakeChoice = v))),
      // macOS names the request it holds; `pmset -g assertions` lists the same one.
      k.active && k.assertion ? h('p', { class: 'small muted res-assertion' }, t('resident.requestRecordedSystem'), h('code', { class: 'selectable' }, k.assertion), h('small', { class: 'block' }, t('resident.runPmsetGAssertions'))) : null,
      h('div', { class: 'res-actions' }, go),
      errorLine(this.awakeError),
      h('p', { class: 'muted small res-foot' }, words().awakeFoot),
    );
  }

  private cleanPanel(s: ResidentStatus): HTMLElement {
    const running = this.busy === 'clean' || s.clean_screen.running;
    const go = h('button', { type: 'button', class: 'primary', id: 'clean-go', disabled: running || this.busy !== null }, running ? t('resident.cleanScreenProgress') : t('resident.startCleanScreen'));
    go.addEventListener('click', () => void this.cleanScreen());
    const hold = Math.round(s.clean_screen.esc_hold_ms / 1000);
    const w = words();
    const keys = w.secureKeys.flatMap((k, i) => (i === 0 ? [h('kbd', {}, k)] : [w.secureJoin, h('kbd', {}, k)]));
    return h(
      'section',
      { class: 'panel res-card', 'aria-labelledby': 'clean-title' },
      h('h2', { id: 'clean-title' }, t('resident.cleanScreen')),
      h('p', { class: 'small' }, t('resident.allDisplaysGoBlack')),
      w.cleanScreenLimit ? h('p', { class: 'muted small' }, w.cleanScreenLimit) : null,
      h('div', { class: 'res-row' }, h('span', { class: 'res-label' }, t('resident.autoExit')), this.seg('clean-len', t('resident.whenExitAutomatically'), cleanChoices(), this.cleanChoice, running, (v) => (this.cleanChoice = v))),
      h(
        'ul',
        { class: 'res-exits' },
        h('li', {}, t('resident.hold'), h('kbd', {}, 'Esc'), t('resident.holdExit', { hold })),
        h('li', {}, t('resident.exitsAutomaticallyWhenTime')),
        h('li', {}, ...keys, w.secureNote),
      ),
      h('div', { class: 'res-actions' }, go),
      errorLine(this.cleanError),
      this.cleanResult ? noticeLine(EXIT_TEXT[this.cleanResult]) : null,
    );
  }

  private privacyPanel(): HTMLElement {
    const p = this.privacy;
    const CAP: Record<string, string> = { camera: t('resident.privacyPanel.camera'), microphone: t('resident.privacyPanel.microphone') };
    return h(
      'section',
      { class: 'panel res-card', 'aria-labelledby': 'privacy-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'privacy-title' }, t('resident.cameraMicrophone2')), p && !p.supported ? pill(t('common.unavailable'), 'muted') : null),
      !p
        ? h('p', { class: 'muted small', role: 'status' }, t('common.reading'))
        : !p.supported
          ? h('p', { class: 'small' }, `${words().privacyUnreadable}${p.unavailable_reason ?? t('resident.reasonUnknown')}`)
          : p.in_use.length
            ? h(
                'ul',
                { class: 'res-apps', 'aria-live': 'polite' },
                ...p.in_use.map((a) => h('li', {}, h('span', { class: 'res-app-name' }, a.name), h('span', { class: 'res-app-caps' }, ...a.capabilities.map((c) => pill(t('resident.pUse', { p: CAP[c] ?? c }), 'warn'))))),
              )
            : h('p', { class: 'res-quiet', 'aria-live': 'polite' }, t('resident.noAppUsingCamera')),
      h('p', { class: 'muted small res-foot' }, words().privacyFoot(POLL_MS / 1000)),
    );
  }

  private batteryRow(d: DeviceBattery, threshold: number | null): HTMLElement {
    const bar = h('span', { class: 'res-meter' }, h('span', { class: 'res-meter-fill' }));
    (bar.firstChild as HTMLElement).style.width = `${Math.max(0, Math.min(100, d.percent))}%`;
    const low = threshold !== null && d.percent < threshold && d.charging !== true;
    const note = d.charging === true ? t('resident.chargingPower') : d.charging === false ? t('common.battery') : (KIND_NOTE[d.kind] ?? '');
    return h('li', { 'data-low': low ? 'yes' : 'no' }, h('span', { class: 'res-batt-name' }, batteryName(d), note ? h('small', { class: 'muted' }, note) : null), bar, h('b', { class: 'num res-batt-pct' }, `${d.percent}%`));
  }

  private batteryPanel(s: ResidentStatus): HTMLElement {
    const b = this.batteries;
    const set = s.settings;
    const threshold = set.battery_alerts && set.tray ? set.battery_threshold : null;
    const tray = words().tray;
    const alerts = this.switchRow('res-batt-alerts', set.battery_alerts && set.tray, this.busy === 'save' || !set.tray, t('resident.lowBatteryAlert'), set.tray ? t('resident.alertsOnceWhenLevel', { tray, interval_seconds: s.tray.interval_seconds }) : t('resident.turnTrayFirstWhile', { tray, tray2: tray }), (v) =>
      void this.save({ ...set, battery_alerts: v }, v ? t('resident.lowBatteryAlertTurned') : t('resident.lowBatteryAlertTurned2')),
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
      void this.save({ ...set, battery_threshold: v }, t('resident.alertThresholdSetV', { v })).then(() => (this.threshold = null));
    });
    const lists = b ? batteryLists(b) : null;
    const devices = lists?.devices ?? [];
    const btNote = lists?.note ?? null;
    return h(
      'section',
      { class: 'panel res-card', 'aria-labelledby': 'battery-title' },
      h('h2', { id: 'battery-title' }, t('common.batteryLevel')),
      !b ? h('p', { class: 'muted small', role: 'status' }, t('common.reading')) : devices.length ? h('ul', { class: 'res-batt' }, ...devices.map((d) => this.batteryRow(d, threshold))) : h('p', { class: 'small' }, words().noSystemBattery),
      // Devices without a reading: named, with the reason, and no meter or number (never 0%).
      lists?.unavailable.length
        ? h(
            'ul',
            { class: 'res-batt res-batt-unavailable', 'aria-label': t('resident.devicesWithoutBatteryReading') },
            ...lists.unavailable.map((d) =>
              h(
                'li',
                { 'data-low': 'no', 'data-available': 'no' },
                h('span', { class: 'res-batt-name' }, d.name, h('small', { class: 'muted' }, [d.note, d.reason].filter(Boolean).join(' · '))),
                h('span', { class: 'res-meter', 'aria-hidden': 'true' }),
                h('b', { class: 'res-batt-pct muted' }, t('common.unavailable')),
              ),
            ),
          )
        : null,
      btNote ? h('p', { class: 'small res-bt-note' }, btNote) : null,
      h('div', { class: 'res-alerts' }, alerts, h('label', { class: 'num-field', for: 'res-threshold' }, h('span', {}, t('resident.alertThreshold')), h('span', { class: 'num-wrap' }, input, h('span', { class: 'muted small' }, '%')))),
      h('p', { class: 'muted small res-foot' }, words().batteryFoot),
    );
  }

  private notHerePanel(s: ResidentStatus): HTMLElement {
    return h(
      'section',
      { class: 'panel res-nothere', 'aria-labelledby': 'nothere-title' },
      h('h2', { id: 'nothere-title' }, t('resident.notOfferedPlatform')),
      h(
        'ul',
        { class: 'res-nothere-list' },
        ...s.not_here.map((n) => {
          const [label, why] = notHereWords(n.id, n.reason);
          return h('li', {}, h('span', { class: 'res-nh-name' }, label, pill(t('resident.notApplicableHere'), 'muted')), h('span', { class: 'muted small' }, why));
        }),
      ),
    );
  }
}
