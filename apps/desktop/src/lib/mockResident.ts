// Browser preview of 状态 › 常驻工具. Development fixture only: no tray, power request or window is created.

import type { BatteryAlert, Batteries, KeepAwake, PrivacySnapshot, ResidentBackend, ResidentSettings, ResidentStatus } from './residentTypes';

const RUN = 'HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run\\CleanManager.Resident';

export function mockResident(): ResidentBackend {
  let settings: ResidentSettings = { tray: false, start_at_login: false, battery_alerts: false, battery_threshold: 20 };
  let awakeUntil: number | null = null;
  let awakeOn = false;
  const changed = new Set<() => void>();
  const privacy = (): PrivacySnapshot => ({
    supported: true,
    in_use: [{ name: 'Zoom.exe', capabilities: ['camera', 'microphone'], desktop_app: true }],
    unavailable_reason: null,
    source: 'HKCU CapabilityAccessManager ConsentStore (webcam, microphone): LastUsedTimeStart set and LastUsedTimeStop = 0',
  });
  const keepAwake = (): KeepAwake => {
    if (awakeOn && awakeUntil !== null && Date.now() >= awakeUntil) {
      awakeOn = false;
      awakeUntil = null;
    }
    return { active: awakeOn, remaining_seconds: awakeOn && awakeUntil !== null ? Math.ceil((awakeUntil - Date.now()) / 1000) : null };
  };
  const status = (): ResidentStatus => ({
    supported: true,
    platform: 'windows',
    settings: { ...settings },
    tray: {
      enabled: settings.tray,
      icon_shown: settings.tray,
      sampling: settings.tray,
      interval_seconds: 5,
      startup: settings.tray && settings.start_at_login ? 'at_login' : 'manual',
      startup_location: RUN,
      startup_command: settings.tray && settings.start_at_login ? '"C:\\Program Files\\CleanManager\\cleanmanager-desktop.exe" --resident' : null,
      version: '0.1.0',
    },
    seen: settings.tray
      ? { summary: { sampled_at: new Date().toISOString(), cpu_percent: 18 + Math.round(Math.random() * 6), memory_percent: 61, disk_free_bytes: 129_284_829_184 }, privacy: privacy(), alerts: [] }
      : { summary: null, privacy: null, alerts: [] },
    keep_awake: keepAwake(),
    clean_screen: { running: false, min_seconds: 5, max_seconds: 300, default_seconds: 60, esc_hold_ms: 2000 },
    not_here: [
      { id: 'fan_control', reason: 'Windows has no documented interface for fan control' },
      { id: 'charge_limit', reason: 'Windows has no common interface for a battery charge limit' },
      { id: 'low_latency_network', reason: 'needs the privileged helper, which this build does not install' },
      { id: 'touch_id_sudo', reason: 'Touch ID for administrator prompts exists on macOS only' },
    ],
  });
  return {
    async residentStatus() {
      return status();
    },
    async residentSave(s) {
      if (s.start_at_login && !s.tray) throw { code: 'invalid_argument', reason: 'invalid_argument', message: 'start_at_login requires the tray component to be on' };
      settings = { ...s };
      changed.forEach((cb) => cb());
      return status();
    },
    async residentKeepAwake(on, minutes) {
      awakeOn = on;
      awakeUntil = on && minutes ? Date.now() + minutes * 60_000 : null;
      return keepAwake();
    },
    async residentCleanScreen() {
      // The preview cannot blank the screen; it reports the timeout exit after a short pause.
      await new Promise((r) => setTimeout(r, 1200));
      return 'timeout';
    },
    async residentPrivacy() {
      return privacy();
    },
    async residentBatteries(): Promise<Batteries> {
      return {
        supported: true,
        system: { device_id: 'system', name: '本机电池', percent: 76, kind: 'system', charging: false },
        devices: [
          { device_id: 'BTHENUM\\DEV_1', name: 'MX Keys', percent: 18, kind: 'bluetooth', charging: null },
          { device_id: 'BTHENUM\\DEV_2', name: 'WH-1000XM4', percent: 64, kind: 'bluetooth', charging: null },
        ],
        bluetooth_devices: 3,
        devices_unavailable: null,
        source: 'GetSystemPowerStatus; device property {104EA319-6EE2-4701-BD47-8DDBF425BBE5},2 set by the Windows Bluetooth stack',
      };
    },
    async onResidentNavigate() {
      return () => {};
    },
    async onResidentChanged(cb) {
      changed.add(cb);
      return () => changed.delete(cb);
    },
    async onResidentAlert(_cb: (a: BatteryAlert[]) => void) {
      return () => {};
    },
  };
}
