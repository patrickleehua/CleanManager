// DEVELOPMENT-ONLY macOS variant of the browser preview (`?platform=macos`). It replaces the Windows fixture's
// answers where the macOS host answers differently: start-up inventory (launchd rows, one Service Management row in
// state `unknown`, every row read-only), uninstall routes (Trash, Homebrew, `permission_required`), update channels
// (Sparkle, Homebrew, Mac App Store, Electron), the maintenance catalog (administrator tasks skipped
// `elevation_unavailable`), status metrics (temperatures and fans unavailable with the reason, zombie count) and the
// macOS insight kinds. Texts the host writes are the core's English, as a real macOS host sends them.
// Nothing here can change anything: it is in-memory preview data.

import type { Backend } from './api';
import type { Scripted } from './mockOps';
import type { MaintenanceReport, MaintenanceTask, Prohibited, TaskOutcome, UninstallApp, UpdateCheck, UpdateRouted } from './suiteTypes';
import type { Insight, IpcError, Mode, OperationItem, PlanDetail, PlanItem, StartupItem, StatusMetric } from './types';

type MakePlan = (kind: PlanDetail['kind'], mode: Mode | null, items: PlanItem[], extra?: Partial<PlanDetail>, script?: Scripted[], onDone?: (operationId: string, items: OperationItem[]) => void) => string;

const err = (code: string, reason: string, message: string): IpcError => ({ code, reason, message });
const delay = (ms: number) => new Promise((r) => setTimeout(r, ms));
const iso = (minutes = 0) => new Date(Date.now() + minutes * 60_000).toISOString().replace(/\.\d+Z$/, 'Z');
const GiB = 1024 ** 3;
const HOME = '/Users/lin';
const V2 = { contract: 'plan/v2', scan_id: null, scan_revision: null, scope_id: null, scan_root_display: null, total_logical_bytes: '0', allocated_known_bytes: null } as const;

function row(action_id: string, name: string, path_display: string, item_kind: string, action_type: string, extra: Partial<PlanItem> = {}): PlanItem {
  return { action_id, name, path_display, logical_bytes: '0', allocated_bytes: null, candidate_id: null, original_path_display: null, quarantine_id: null, item_kind, action_type, ...extra };
}

// ---------------------------------------------------------------------------------------------
// 软件 › 启动项 (cm-platform macos/startup.rs)

const TOGGLE_UNSUPPORTED =
  "Read-only on macOS in this build: not enabled, disabled or removed here; use the app's own setting or System Settings > General > Login Items & Extensions";
const LOGIN_ITEMS = 'x-apple.systempreferences:com.apple.LoginItems-Settings.extension';

function st(id: string, name: string, source: string, definition: string, target: string | null, publisher: string | null, scope: string, state: string, extra: Partial<StartupItem> = {}): StartupItem {
  return {
    startup_item_id: id,
    name,
    source,
    definition,
    target: target ? { display: target } : null,
    publisher,
    publisher_evidence: publisher ? 'code_signature' : 'none',
    scope,
    state,
    toggle_capability: 'unsupported',
    protected: false,
    reason: TOGGLE_UNSUPPORTED,
    ...extra,
  };
}

const STARTUP: StartupItem[] = [
  st('st_m_dropbox', 'com.dropbox.DropboxMacUpdate.agent', 'launchd_user', `${HOME}/Library/LaunchAgents/com.dropbox.DropboxMacUpdate.agent.plist`, '/Applications/Dropbox.app/Contents/Resources/DropboxMacUpdate.app/Contents/MacOS/DropboxMacUpdate', 'Developer ID Application: Dropbox, Inc. (G7HH3F8CAK)', 'user', 'enabled'),
  st('st_m_zoom', 'us.zoom.updater', 'launchd_system', '/Library/LaunchAgents/us.zoom.updater.plist', '/Library/Application Support/zoom.us/ZoomUpdater.app/Contents/MacOS/ZoomUpdater', 'Developer ID Application: Zoom Video Communications, Inc. (BJ4HAAB9B3)', 'system', 'unknown', {
    reason: `Not listed as loaded in gui/501: it may be turned off in Login Items & Extensions, limited to another session type, or not loaded yet. ${TOGGLE_UNSUPPORTED}`,
  }),
  st('st_m_docker', 'com.docker.vmnetd', 'launchd_system', '/Library/LaunchDaemons/com.docker.vmnetd.plist', '/Library/PrivilegedHelperTools/com.docker.vmnetd', 'Developer ID Application: Docker Inc (9BNSXJN65R)', 'system', 'enabled'),
  st('st_m_old', 'com.example.removed-helper', 'launchd_user', `${HOME}/Library/LaunchAgents/com.example.removed-helper.plist`, '/Applications/Removed Tool.app/Contents/MacOS/helper', null, 'user', 'broken', {
    publisher_evidence: 'none',
    reason: `The program is missing on a mounted volume. ${TOGGLE_UNSUPPORTED}`,
  }),
  st('st_m_apple', 'com.apple.Safari.History', 'launchd_system', '/System/Library/LaunchAgents/com.apple.Safari.History.plist', '/System/Library/PrivateFrameworks/SafariShared.framework/Versions/A/XPCServices/com.apple.Safari.History.xpc', 'Apple', 'system', 'enabled', {
    protected: true,
    reason: `Part of macOS: shown read-only. ${TOGGLE_UNSUPPORTED}`,
  }),
  st('st_m_sm', 'Login items and background items (Service Management)', 'sm_background_item', LOGIN_ITEMS, null, null, 'user', 'unknown', {
    reason:
      'Login items and background items registered through Service Management cannot be listed through a public API (the Background Task Management database is not read); review them in System Settings > General > Login Items & Extensions (x-apple.systempreferences:com.apple.LoginItems-Settings.extension)',
  }),
];

// ---------------------------------------------------------------------------------------------
// 分析 › 空间洞察 (cm-core insights.rs, macOS objects)

function ins(id: string, kind: string, title: string, path: string | null, size: number | null, basis: string, explanation: string, guidance: Insight['guidance'], note: string | null = null, count: number | null = null): Insight {
  return { insight_id: id, kind, title, path: path ? { display: path } : null, logical_bytes: size === null ? null : String(Math.round(size)), count, size_basis: basis, size_note: note, observed_at: iso(-2), explanation, handling: 'read_only_guidance', guidance };
}

const INSIGHTS: Insight[] = [
  ins('i_m_snap', 'macos.local_snapshots', 'Local snapshots', null, null, 'not_measured', 'APFS local snapshots kept on the startup disk.', [
    { tool: 'tmutil', steps: 'tmutil listlocalsnapshots /', requires_admin: false, impact: 'Lists them; macOS removes them on its own when space is needed.' },
  ], 'macOS does not report the space local snapshots use', 3),
  ins('i_m_docker', 'macos.docker_disk', 'Docker disk image', `${HOME}/Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw`, 34.2 * GiB, 'allocated', 'Images, containers and volumes of Docker Desktop.', [
    { tool: 'docker', steps: 'docker system df', requires_admin: false, impact: 'Shows what the disk image holds; `docker system prune` removes unused data.' },
  ]),
  ins('i_m_utm', 'macos.utm_vms', 'UTM virtual machines', `${HOME}/Library/Containers/com.utmapp.UTM/Data/Documents`, null, 'not_measured', 'UTM virtual machines.', [], 'Inside a privacy-protected app container; its size was not read'),
  ins('i_m_backups', 'macos.device_backups', 'Device backups', `${HOME}/Library/Application Support/MobileSync/Backup`, null, 'not_measured', 'Local iPhone and iPad backups.', [
    { tool: 'Finder', steps: 'Finder > (device) > Manage Backups', requires_admin: false, impact: 'Delete old backups you no longer need.' },
  ], 'Reading it needs Full Disk Access'),
  ins('i_m_swap', 'macos.swap', 'Swap files', '/System/Volumes/VM', 2 * GiB, 'system_query', 'Swap space macOS manages on its own.', []),
  ins('i_m_purge', 'macos.purgeable_space', 'Purgeable space (estimate)', null, 18.6 * GiB, 'system_query', 'Space the system can free on demand.', [], 'Estimate: available for important usage minus free space'),
];

// ---------------------------------------------------------------------------------------------
// 软件 › 卸载 (cm-core uninstall/mac.rs routes)

const APP_STORE_GUIDANCE =
  'Mac App Store apps belong to the system installer and need administrator rights to remove; this build has no administrator path on macOS. Remove it in Launchpad (hold Option and click the delete button) or with Finder › Move to Trash';
const FINDER_GUIDANCE = "this build has no administrator path on macOS and never asks for one; quit the app and use Finder › Move to Trash (Finder asks for an administrator password), or the vendor's own uninstaller";

function app(id: string, name: string, version: string | null, publisher: string | null, source: string, location: string | null, over: Partial<UninstallApp> = {}): UninstallApp {
  return {
    app_instance_id: id,
    name,
    version,
    publisher,
    source,
    location,
    last_used_at: null,
    protected_class: false,
    state: 'available',
    adapter: { adapter_id: 'macos.trash-app-bundle', display: `move ${location} to the Trash (recoverable with Finder › Put Back; never deleted)`, package_manager: false },
    reason: null,
    settings_uri: null,
    ...over,
  };
}

const APPS: UninstallApp[] = [
  app('app_m_vlc', 'VLC', '3.0.21', 'Developer ID Application: VideoLAN (75GAHG3SZQ)', 'app_bundle', '/Applications/VLC.app'),
  app('app_m_zoom', 'zoom.us', '6.2.5', 'Developer ID Application: Zoom Video Communications, Inc. (BJ4HAAB9B3)', 'app_bundle', '/Applications/zoom.us.app', {
    adapter: {
      adapter_id: 'macos.trash-app-bundle',
      display: 'move /Applications/zoom.us.app to the Trash (recoverable with Finder › Put Back; never deleted); it came with an installer package, whose other components are not removed',
      package_manager: false,
    },
  }),
  app('app_m_firefox', 'Firefox', '131.0.3', 'Developer ID Application: Mozilla Corporation (43AQ936H96)', 'homebrew', '/Applications/Firefox.app', {
    adapter: { adapter_id: 'homebrew.uninstall-cask', display: "brew uninstall --cask firefox (package manager; the cask's zap stanza is never run)", package_manager: true },
  }),
  app('app_m_ffmpeg', 'ffmpeg', '7.1', null, 'homebrew', '/opt/homebrew/Cellar/ffmpeg/7.1', {
    adapter: { adapter_id: 'homebrew.uninstall-formula', display: "brew uninstall --formula ffmpeg (package manager; the formula's zap stanza is never run)", package_manager: true },
  }),
  app('app_m_pages', 'Pages', '14.2', 'Apple Mac OS Application Signing', 'app_store', '/Applications/Pages.app', {
    state: 'unverified',
    adapter: null,
    reason: APP_STORE_GUIDANCE,
    refusal_code: 'permission_required',
  }),
  app('app_m_office', 'Microsoft Word', '16.89', 'Developer ID Application: Microsoft Corporation (UBF8T346G9)', 'app_bundle', '/Applications/Microsoft Word.app', {
    state: 'unverified',
    adapter: null,
    reason: `Owned by root (installed by an installer package): moving it needs administrator rights; ${FINDER_GUIDANCE}`,
    refusal_code: 'permission_required',
  }),
  app('app_m_safari', 'Safari', '18.1', 'Apple', 'app_bundle', '/System/Applications/Safari.app', {
    state: 'unsupported',
    protected_class: true,
    adapter: null,
    reason: 'Part of macOS (in /System/Applications or on the sealed system volume): it cannot be removed',
    refusal_code: 'policy_denied',
  }),
];

// ---------------------------------------------------------------------------------------------
// 软件 › 更新 (cm-core updates/mac.rs)

function upd(id: string, name: string, current: string | null, available: string | null, channel: UpdateCheck['channel'], state: UpdateCheck['state'], route: UpdateCheck['route'], over: Partial<UpdateCheck> = {}): UpdateCheck {
  return { app_instance_id: id, name, current_version: current, available_version: available, channel, channel_id: null, state, reason: null, checked_at: null, route, official_url: null, ...over };
}

const SPARKLE_OWN = 'the app installs its own updates (open it and choose Check for Updates); CleanManager only reports and never downloads or installs them';

function updateResults(at: string | null): UpdateCheck[] {
  const t = { checked_at: at };
  return [
    upd('app_m_firefox', 'Firefox', '131.0.3', '132.0', 'homebrew', 'update_available', 'package_manager', { ...t, channel_id: 'cask:firefox', reason: "Homebrew upgrade available; compared with Homebrew's local index; Homebrew verifies the download's checksum before installing" }),
    upd('app_m_ffmpeg', 'ffmpeg', '7.1', '7.1', 'homebrew', 'up_to_date', 'none', { ...t, channel_id: 'formula:ffmpeg', reason: "Not outdated in Homebrew; compared with Homebrew's local index" }),
    upd('app_m_vlc', 'VLC', '3.0.21', '3.0.22', 'sparkle', 'update_available', 'open_app', {
      ...t,
      channel_id: 'https://update.videolan.org/vlc/sparkle/vlc-arm64.xml',
      reason: `Sparkle feed offers 3.0.22 (sparkle:version 3.0.22); ${SPARKLE_OWN}; updates are verified by the app against the EdDSA key (SUPublicEDKey) in its Info.plist`,
    }),
    upd('app_m_zoom', 'zoom.us', '6.2.5', null, 'sparkle', 'cannot_check', 'open_app', { ...t, reason: `Uses Sparkle but its Info.plist declares no SUFeedURL (the feed is chosen at run time), so it cannot be checked here; ${SPARKLE_OWN}` }),
    upd('app_m_pages', 'Pages', '14.2', null, 'mac_app_store', 'cannot_check', 'open_store', { ...t, channel_id: 'com.apple.iWork.Pages', reason: 'The Mac App Store checks its own apps; open its Updates page', official_url: 'macappstore://showUpdatesPage' }),
    upd('app_m_slack', 'Slack', '4.41.97', null, 'electron', 'cannot_check', 'open_app', { ...t, reason: 'Updates itself through Electron autoUpdater (Squirrel.Mac); CleanManager does not read or contact that feed, so open the app and use its own update command' }),
    upd('app_m_wechat', '微信', '3.8.9', null, null, 'cannot_check', 'none', { ...t, reason: 'No update channel: no Sparkle feed in Info.plist, not from the Mac App Store or Homebrew, and no recognizable update framework' }),
  ];
}

// ---------------------------------------------------------------------------------------------
// 优化 › 维护任务 (cm-core maintenance.rs MACOS_TASKS)

function task(id: string, title: string, problem: string, impact: string, admin: boolean, applies: string, seconds: number, applicability: MaintenanceTask['applicability'], reason: string | null, over: Partial<MaintenanceTask> = {}): MaintenanceTask {
  return { task_id: id, task_version: '1.0.0', title, problem, impact, requires_admin: admin, applies_when: applies, estimated_seconds: seconds, applicability, applicability_reason: reason, user_protected: false, reversible: false, ...over };
}

const TASKS: MaintenanceTask[] = [
  task('macos.quicklook-reset', 'Reset Quick Look thumbnail cache', 'Quick Look previews or Finder thumbnails are blank, wrong or outdated', 'Thumbnails are generated again on next view (`qlmanage -r cache`); files are not touched', false, '/usr/bin/qlmanage is present and system-owned', 3, 'applicable', null),
  task('macos.spotlight-status', 'Check Spotlight indexing (read-only)', 'Spotlight finds nothing or misses files', "Read-only status of the startup volume's index (`mdutil -s /`); nothing is changed", false, '/usr/bin/mdutil is present and system-owned', 2, 'applicable', null),
  task('macos.broken-preferences', 'Find broken preference files (report only)', 'An app keeps losing its settings or fails to start because its preference file is damaged', 'Reads ~/Library/Preferences (and ByHost) and lists files that do not parse as property lists; nothing is deleted or moved', false, 'Your ~/Library/Preferences folder exists', 4, 'applicable', null),
  task('macos.network-dsstore-off', 'Stop writing .DS_Store files on network volumes', 'Finder leaves .DS_Store files on shared network folders', 'Sets com.apple.desktopservices DSDontWriteNetworkStores to true after backing up the prior value (restorable with `optimize restore`); Finder picks it up after you log out and back in', false, 'The setting is not already true', 1, 'applicable', null, { reversible: true }),
  task('macos.icon-cache-rebuild', 'Rebuild icon services cache', 'App or file icons are blank, generic or outdated in Finder and the Dock', 'Icons are drawn again after the cache is rebuilt; needs administrator authorization (system cache)', true, 'The system icon services store exists (/Library/Caches/com.apple.iconservices.store)', 20, 'applicable', null),
  task('macos.dns-flush', 'Flush DNS resolver cache', 'Sites resolve to old addresses after a network or DNS change', 'The first lookup of each name is slightly slower once; mDNSResponder must be signalled, which needs administrator authorization', true, '/usr/bin/dscacheutil and /usr/bin/killall are present and system-owned', 1, 'applicable', null),
];

const PROHIBITED: Prohibited[] = [
  { id: 'memory-release', title: 'Memory "release" or RAM boosting', reason: 'Windows manages memory itself; forcing pages out only makes the next use slower. The diagnosis shows real memory pressure instead' },
  { id: 'search-indexing-disable', title: 'Turning off Windows Search indexing', reason: 'Search, Outlook and Settings depend on the index' },
  { id: 'one-click-speedup', title: 'One-click speed-up', reason: 'There is no honest percentage to promise' },
];

const NO_PRIVILEGE = 'needs administrator authorization; no privilege path exists on macOS in this build; not run';

// ---------------------------------------------------------------------------------------------
// 状态 › 系统状态 (cm-platform macos/status.rs)

const SENSOR_REASON =
  'macOS publishes temperatures and fan speeds only through the System Management Controller (SMC) and HID sensor services, which are private interfaces CleanManager does not use';

export function mockMac(makePlan: MakePlan): Partial<Backend> {
  let cpu = 22;
  let checksOn = true;
  let lastChecked: string | null = iso(-60 * 5);
  let results = updateResults(lastChecked);
  const history: MaintenanceReport[] = [];
  return {
    async startupList() {
      await delay(220);
      return { supported: true, is_admin: false, items: STARTUP.map((s) => ({ ...s })), issues: [] };
    },
    async startupPlan() {
      await delay(120);
      throw err('unsupported', 'unsupported', 'Enabling, disabling or removing login items and launchd jobs is not implemented on macOS in this build');
    },
    async insightsList() {
      await delay(300);
      return { supported: true, observed_at: iso(-2), items: INSIGHTS };
    },
    async statusSnapshot() {
      await delay(140);
      cpu = Math.max(4, Math.min(96, cpu + Math.round((Math.random() - 0.45) * 14)));
      const m = (name: string, value: number | null, unit: string, reason: string | null = null): StatusMetric => ({ name, availability: value === null ? 'unavailable' : 'available', value, unit, reason });
      return {
        supported: true,
        sampled_at: iso(0),
        stale: false,
        metrics: [
          m('cpu.total_percent', cpu, 'percent'),
          m('cpu.logical_count', 10, 'count'),
          m('memory.used_percent', 63.2, 'percent', 'App memory + wired + compressed, as Activity Monitor\'s "Memory Used"'),
          m('memory.total_bytes', 16 * GiB, 'bytes'),
          m('memory.available_bytes', 5.9 * GiB, 'bytes'),
          m('disk.system.free_percent', 24.5, 'percent'),
          m('disk.system.free_bytes', 121.7 * GiB, 'bytes'),
          m('disk.system.total_bytes', 494 * GiB, 'bytes'),
          m('disk.busy_percent', 3.1, 'percent'),
          m('disk.read_bytes_per_sec', 1.2 * 1024 ** 2, 'bytes_per_second'),
          m('disk.write_bytes_per_sec', 0.4 * 1024 ** 2, 'bytes_per_second'),
          m('thermal.max_zone_celsius', null, 'celsius', SENSOR_REASON),
          m('gpu.temperature_celsius', null, 'celsius', SENSOR_REASON),
          m('fan.rpm', null, 'rpm', SENSOR_REASON),
          m('battery.charge_percent', 81, 'percent'),
          m('battery.on_ac_power', 0, 'boolean'),
          m('battery.health_percent', 92.4, 'percent', 'Full-charge capacity over design capacity (AppleSmartBattery)'),
          m('battery.cycle_count', 187, 'count'),
          m('process.count', 512, 'count'),
          m('process.zombie_count', 2, 'count', 'Processes that exited but whose parent has not collected them yet (SZOMB); they use no CPU or memory'),
        ],
        health: {
          score: 84,
          weights: { cpu: 30, memory: 30, disk: 25, thermal: 0, io: 15 },
          lowered_by: [],
          dimensions: [
            { dimension: 'cpu', metric: 'cpu.total_percent', value: cpu, full_score_at: 30, zero_score_at: 95, score: Math.max(0, Math.min(100, Math.round(((95 - cpu) / 65) * 100))) },
            { dimension: 'memory', metric: 'memory.used_percent', value: 63.2, full_score_at: 60, zero_score_at: 95, score: 91 },
            { dimension: 'disk', metric: 'disk.system.free_percent', value: 24.5, full_score_at: 20, zero_score_at: 5, score: 100 },
            { dimension: 'thermal', metric: 'thermal.max_zone_celsius', value: null, full_score_at: 70, zero_score_at: 95, score: null },
            { dimension: 'io', metric: 'disk.busy_percent', value: 3.1, full_score_at: 20, zero_score_at: 90, score: 100 },
          ],
        },
        diagnosis: null,
      };
    },

    async uninstallList() {
      await delay(260);
      return { supported: true, is_admin: false, items: APPS.map((a) => ({ ...a })), issues: [], recent: [] };
    },
    async uninstallPlan(ids) {
      await delay(380);
      const chosen = ids.map((id) => APPS.find((a) => a.app_instance_id === id)).filter((a): a is UninstallApp => !!a && a.state === 'available');
      const refused = ids
        .map((id) => APPS.find((a) => a.app_instance_id === id))
        .filter((a): a is UninstallApp => !!a && a.state !== 'available')
        .map((a) => ({ app_instance_id: a.app_instance_id, name: a.name, reason: a.refusal_code ?? 'unsupported', message: a.reason ?? '' }));
      if (!chosen.length) {
        const r = refused[0];
        throw err(r?.reason ?? 'invalid_argument', r?.reason ?? 'invalid_argument', r?.message ?? 'nothing to uninstall');
      }
      const items: PlanItem[] = [];
      const script: Scripted[] = [];
      const actionIds: string[][] = [];
      chosen.forEach((a, n) => {
        const q = `u${String(n).padStart(3, '0')}0000`;
        const u = `u${String(n).padStart(3, '0')}0001`;
        actionIds.push([q, u]);
        items.push(row(q, a.name, a.location ?? a.name, 'app_quit', 'app_quit', { detail: { force: 'never', app_instance_id: a.app_instance_id } }));
        items.push(row(u, a.name, a.adapter!.display, 'uninstaller', 'tool_adapter', { detail: { adapter_id: a.adapter!.adapter_id, app_instance_id: a.app_instance_id } }));
        script.push({ status: 'succeeded', code: 'closed', message: `${a.name} quit after a close request` });
        script.push(
          a.adapter!.adapter_id === 'macos.trash-app-bundle'
            ? { status: 'succeeded', code: 'completed', message: 'moved to the Trash in 412 ms (Finder › Put Back restores it); rescan: the app bundle is gone' }
            : { status: 'succeeded', code: 'completed', message: 'brew exited with code 0 after 5210 ms; rescan: the app bundle and the Homebrew Caskroom record are gone' },
        );
      });
      const apps = chosen.map((a, n) => ({ app_instance_id: a.app_instance_id, name: a.name, publisher: a.publisher, version: a.version, source: a.source, adapter: a.adapter!.display, signer: a.publisher, action_ids: actionIds[n] }));
      const names = chosen.map((a) => a.name).join('、');
      const batch = chosen.map((a) => ({ app_instance_id: a.app_instance_id, name: a.name, leftover_estimate: { caches_logs: 2, settings: 1 } }));
      const planId = makePlan('uninstall', null, items, { ...V2, plan_kind_v2: 'uninstall', kind_detail: { stage: 'uninstall', app: apps[0], apps, may_end_processes: false, noted: { app_name: names, leftover_estimate: { caches_logs: 2, settings: 1 }, apps: batch, refused } } }, script);
      return { plan_id: planId, leftover_estimate: { caches_logs: 2, settings: 1 }, apps: batch, refused };
    },

    async updatesList(check) {
      await delay(check ? 1600 : 160);
      if (check && checksOn) {
        lastChecked = iso(0);
        results = updateResults(lastChecked);
      }
      const items = checksOn || lastChecked ? results.map((r) => ({ ...r })) : results.map((r) => ({ ...r, state: 'not_checked' as const, checked_at: null }));
      const count = (s: UpdateCheck['state']) => items.filter((i) => i.state === s).length;
      return {
        supported: true,
        checks_enabled: checksOn,
        checked: check && checksOn,
        checked_at: lastChecked,
        counts: { update_available: count('update_available'), up_to_date: count('up_to_date'), cannot_check: count('cannot_check'), not_checked: count('not_checked') },
        items,
      };
    },
    async updatesSetChecks(enabled) {
      await delay(120);
      checksOn = enabled;
      return { checks_enabled: checksOn, last_checked_at: lastChecked };
    },
    async updatesPlan(ids) {
      await delay(420);
      const routed: UpdateRouted[] = [];
      const planned: PlanItem[] = [];
      for (const id of ids) {
        const r = results.find((x) => x.app_instance_id === id);
        if (!r) throw err('invalid_argument', 'unknown_app', `unknown app instance ${id}`);
        if (r.state !== 'update_available' || r.route !== 'package_manager') {
          routed.push({ app_instance_id: id, name: r.name, route: r.route === 'package_manager' ? 'none' : r.route, reason: r.reason ?? 'no update is available', url: r.official_url });
          continue;
        }
        planned.push(
          row(`a${String(planned.length + 1).padStart(2, '0')}`, r.name, `${r.current_version} → ${r.available_version}`, 'app_update', 'tool_adapter', {
            detail: { channel: 'homebrew', package_id: r.channel_id, command: `brew upgrade --cask ${String(r.channel_id).replace(/^cask:/, '')}`, official_url: null, timeout_seconds: 900 },
          }),
        );
      }
      if (!planned.length) return { plan_id: null, planned: 0, routed };
      const planId = makePlan('update', null, planned, { ...V2, plan_kind_v2: 'update', kind_detail: { noted: { routed } } }, planned.map((p) => ({ status: 'succeeded', code: 'updated', message: `updated to ${p.path_display.split('→ ')[1]}` })));
      return { plan_id: planId, planned: planned.length, routed };
    },
    async openSystemPage() {
      await delay(60);
      return true;
    },

    async maintenanceCatalog() {
      await delay(320);
      return {
        supported: true,
        items: TASKS.map((t) => ({ ...t })),
        default_selection: [],
        prohibited: PROHIBITED,
        is_admin: false,
        elevation: 'unavailable',
        observed_at: iso(0),
      };
    },
    async maintenanceSearch(query) {
      await delay(90);
      const q = query.trim().toLowerCase();
      if ([...q].length < 2) return { query, task_ids: TASKS.map((t) => t.task_id), prohibited: [] };
      return {
        query,
        task_ids: TASKS.filter((t) => `${t.task_id} ${t.title}`.toLowerCase().includes(q)).map((t) => t.task_id),
        prohibited: PROHIBITED.filter((p) => `${p.id} ${p.title}`.toLowerCase().includes(q) || (q.includes('内存') && p.id === 'memory-release') || (q.includes('聚焦') && p.id === 'search-indexing-disable')),
      };
    },
    async maintenancePlan(ids) {
      await delay(280);
      const chosen = ids.map((id) => TASKS.find((t) => t.task_id === id));
      if (chosen.some((c) => !c)) throw err('invalid_argument', 'refused', 'unknown task');
      const items = chosen.map((t, i) =>
        row(`m${String(i + 1).padStart(2, '0')}`, t!.title, `${t!.task_id}@${t!.task_version}`, 'maintenance_task', 'maintenance_task', { scope: t!.requires_admin ? 'machine' : 'user', detail: { task_id: t!.task_id, task_version: t!.task_version, requires_admin: t!.requires_admin } }),
      );
      // MNT-04 on macOS: no privilege path, so every administrator task is skipped `elevation_unavailable`.
      const runs = chosen.map((t) => ({ t: t!, outcome: (t!.requires_admin ? 'skipped' : 'applied') as TaskOutcome, code: t!.requires_admin ? 'elevation_unavailable' : 'done', reason: t!.requires_admin ? NO_PRIVILEGE : 'completed' }));
      const script: Scripted[] = runs.map((r) => ({ status: r.outcome === 'skipped' ? 'skipped' : 'succeeded', code: r.outcome, message: `${r.code}: ${r.reason}` }));
      const admin = chosen.filter((c) => c!.requires_admin).length;
      const planId = makePlan('maintenance', null, items, { ...V2, plan_kind_v2: 'maintenance', requires_admin: admin > 0, kind_detail: { admin_tasks: admin, is_admin: false } }, script, (op) => {
        const counts = { applied: 0, unchanged: 0, skipped: 0, unavailable: 0, attention: 0, failed: 0 } as Record<TaskOutcome, number>;
        runs.forEach((r) => (counts[r.outcome] += 1));
        history.unshift({
          operation_id: op,
          plan_id: planId,
          plan_digest: '',
          entry: 'desktop',
          principal: 'user:lin',
          elevation: admin ? 'unavailable' : 'not_needed',
          started_at: iso(0),
          finished_at: iso(0),
          items: runs.map((r) => ({ task_id: r.t.task_id, task_version: r.t.task_version, requires_admin: r.t.requires_admin, outcome: r.outcome, code: r.code, reason: r.reason, elevated: false, started_at: iso(0), finished_at: iso(0) })),
          counts,
          all_clear: counts.skipped === 0,
          undo: 'unavailable',
        });
      });
      return { plan_id: planId };
    },
    async maintenanceHistory() {
      await delay(120);
      if (!history.length) {
        history.push({
          operation_id: 'op_mac_7a1c0e5d92b3',
          plan_id: 'mt_mac_1',
          plan_digest: '',
          entry: 'cli',
          principal: 'user:lin',
          elevation: 'unavailable',
          started_at: iso(-60 * 24 * 2),
          finished_at: iso(-60 * 24 * 2 + 1),
          items: [
            { task_id: 'macos.quicklook-reset', task_version: '1.0.0', requires_admin: false, outcome: 'applied', code: 'reset', reason: 'Quick Look cache reset', elevated: false, started_at: iso(-60 * 24 * 2), finished_at: iso(-60 * 24 * 2) },
            { task_id: 'macos.dns-flush', task_version: '1.0.0', requires_admin: true, outcome: 'skipped', code: 'elevation_unavailable', reason: NO_PRIVILEGE, elevated: false, started_at: iso(-60 * 24 * 2), finished_at: iso(-60 * 24 * 2) },
          ],
          counts: { applied: 1, unchanged: 0, skipped: 1, unavailable: 0, attention: 0, failed: 0 },
          all_clear: false,
          undo: 'unavailable',
        });
      }
      return { items: history.map((x) => ({ ...x })), undo: 'unavailable' };
    },
  };
}
