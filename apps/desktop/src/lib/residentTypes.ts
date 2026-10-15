// 状态 › 常驻工具 (RES-01..04, RES-07): wire types of the resident_* host commands.

export interface ResidentSettings {
  tray: boolean;
  start_at_login: boolean;
  battery_alerts: boolean;
  battery_threshold: number;
}

export interface TraySummary {
  sampled_at: string;
  cpu_percent: number | null;
  memory_percent: number | null;
  disk_free_bytes: number | null;
}

export type Capability = 'camera' | 'microphone';

export interface InUseApp {
  name: string;
  capabilities: Capability[];
  desktop_app: boolean;
}

export interface PrivacySnapshot {
  supported: boolean;
  in_use: InUseApp[];
  unavailable_reason: string | null;
  source: string;
}

export interface DeviceBattery {
  device_id: string;
  name: string;
  percent: number;
  kind: 'system' | 'bluetooth';
  charging: boolean | null;
}

export interface BatteryAlert {
  device_id: string;
  name: string;
  percent: number;
  threshold: number;
}

export interface ResidentStatus {
  supported: boolean;
  platform: 'windows' | 'macos' | 'other';
  settings: ResidentSettings;
  tray: {
    enabled: boolean;
    icon_shown: boolean;
    sampling: boolean;
    interval_seconds: number;
    startup: 'manual' | 'at_login';
    startup_location: string;
    startup_command: string | null;
    version: string;
  };
  seen: { summary: TraySummary | null; privacy: PrivacySnapshot | null; alerts: BatteryAlert[] };
  keep_awake: KeepAwake;
  clean_screen: { running: boolean; min_seconds: number; max_seconds: number; default_seconds: number; esc_hold_ms: number };
  not_here: { id: string; reason: string }[];
}

export interface KeepAwake {
  active: boolean;
  remaining_seconds: number | null;
}

export type CleanScreenExit = 'esc_held' | 'timeout' | 'closed';

export interface Batteries {
  supported: boolean;
  system: DeviceBattery | null;
  devices: DeviceBattery[];
  bluetooth_devices: number;
  devices_unavailable: string | null;
  source: string;
}

export interface ResidentBackend {
  residentStatus(): Promise<ResidentStatus>;
  residentSave(settings: ResidentSettings): Promise<ResidentStatus>;
  residentKeepAwake(on: boolean, minutes: number | null): Promise<KeepAwake>;
  residentCleanScreen(timeoutSecs: number): Promise<CleanScreenExit>;
  residentPrivacy(): Promise<PrivacySnapshot>;
  residentBatteries(): Promise<Batteries>;
  onResidentNavigate(cb: (page: string) => void): Promise<() => void>;
  onResidentChanged(cb: () => void): Promise<() => void>;
  onResidentAlert(cb: (alerts: BatteryAlert[]) => void): Promise<() => void>;
}
