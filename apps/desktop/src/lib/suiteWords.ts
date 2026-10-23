// Words for 软件 › 卸载 / 更新, 优化 and 状态 › 进程. The host speaks protocol words (and the core's
// English catalog text); people read these. Unknown values fall back to what the host sent.

import { platform, words } from './platform';
import type { Applicability, LeftoverTier, TaskOutcome, UpdateChannel, UpdateRoute, UpdateState } from './suiteTypes';
import type { Tier } from './types';
import { isResourceText, locale, t, type TextKey } from '../i18n';
import { adapterDisplay } from './uninstallWords';

// ---------------------------------------------------------------------------------------------
// Uninstall

export const SOURCE_WORD: Record<string, string> = {
  get registry_uninstall() { return t('suiteWords.sourceWord.registry_uninstall'); },
  get msi() { return t('suiteWords.sourceWord.msi'); },
  get msix_store() { return t('suiteWords.sourceWord.msix_store'); },
  scoop: 'Scoop',
  get npm_global() { return t('suiteWords.sourceWord.npm_global'); },
  get app_bundle() { return t('suiteWords.sourceWord.app_bundle'); },
  homebrew: 'Homebrew',
  app_store: 'Mac App Store',
  get unknown() { return t('suiteWords.sourceWord.unknown'); },
};

export const UNINSTALL_STATE: Record<string, { label: string; tone: 'good' | 'warn' | 'muted' }> = {
  available: { get label() { return t('suiteWords.uninstallState.available.label'); }, tone: 'good' },
  unverified: { get label() { return t('suiteWords.uninstallState.unverified.label'); }, tone: 'warn' },
  unsupported: { get label() { return t('suiteWords.uninstallState.unsupported.label'); }, tone: 'muted' },
};

/** Leftover groups (UNI-05): name, what they are, and whether anything is preselected. */
export const LEFTOVER_GROUP: Record<string, { title: string; explain: string }> = {
  caches_logs: { get title() { return t('suiteWords.leftoverGroup.caches_logs.title'); }, get explain() { return t('suiteWords.leftoverGroup.caches_logs.explain'); } },
  bundle: { get title() { return t('suiteWords.leftoverGroup.bundle.title'); }, get explain() { return t('suiteWords.leftoverGroup.bundle.explain'); } },
  startup: { get title() { return t('suiteWords.leftoverGroup.startup.title'); }, get explain() { return t('suiteWords.leftoverGroup.startup.explain'); } },
  settings: { get title() { return t('suiteWords.leftoverGroup.settings.title'); }, get explain() { return t('suiteWords.leftoverGroup.settings.explain'); } },
  personal_data: { get title() { return t('suiteWords.leftoverGroup.personal_data.title'); }, get explain() { return t('suiteWords.leftoverGroup.personal_data.explain'); } },
  system: { get title() { return t('suiteWords.leftoverGroup.system.title'); }, get explain() { return t('suiteWords.leftoverGroup.system.explain'); } },
};

export const MARKER_WORD: Record<string, string> = {
  get shared_owner() { return t('suiteWords.markerWord.shared_owner'); },
  get owner_unknown() { return t('suiteWords.markerWord.owner_unknown'); },
  get app_reappeared() { return t('suiteWords.markerWord.app_reappeared'); },
};

export function whyNotWord(k: string): string {
  const table: Record<string, string> = { kept: t('suiteWords.whyNotWord.kept'), blocked: t('suiteWords.whyNotWord.blocked'), manual: t('suiteWords.whyNotWord.manual'), needs_admin: t('suiteWords.whyNotWord.needs_admin', { admin: words().admin }) };
  return table[k] ?? k;
}

/** Leftover tiers on the shared three-tier badge (kept objects are shown as blocked). */
export function leftoverTier(t: LeftoverTier): Tier {
  return t === 'default_selected' ? 'default_selected' : t === 'review_only' ? 'review_only' : 'blocked';
}

export function leftoverAction(k: string): string {
  const table: Record<string, string> = { fs_trash: words().moveToBin, registry_backup_delete: t('suiteWords.leftoverAction.registry_backup_delete'), startup_toggle: t('suiteWords.leftoverAction.startup_toggle'), manual: t('suiteWords.leftoverAction.manual') };
  return table[k] ?? k;
}

// ---------------------------------------------------------------------------------------------
// Updates

export const UPDATE_STATE: Record<UpdateState, { label: string; tone: 'good' | 'warn' | 'muted' | 'busy' }> = {
  update_available: { get label() { return t('suiteWords.updateState.update_available.label'); }, tone: 'busy' },
  up_to_date: { get label() { return t('suiteWords.updateState.up_to_date.label'); }, tone: 'good' },
  cannot_check: { get label() { return t('suiteWords.updateState.cannot_check.label'); }, tone: 'warn' },
  not_checked: { get label() { return t('suiteWords.updateState.not_checked.label'); }, tone: 'muted' },
};

export const CHANNEL_WORD: Record<UpdateChannel, string> = {
  winget: 'winget',
  microsoft_store: 'Microsoft Store',
  get scoop() { return t('suiteWords.channelWord.scoop'); },
  get self_updater() { return t('suiteWords.channelWord.self_updater'); },
  get sparkle() { return t('suiteWords.channelWord.sparkle'); },
  mac_app_store: 'Mac App Store',
  homebrew: 'Homebrew',
  get electron() { return t('suiteWords.channelWord.electron'); },
};

export const ROUTE_WORD: Record<UpdateRoute, string> = {
  get package_manager() { return t('suiteWords.routeWord.package_manager'); },
  get open_store() { return t('suiteWords.routeWord.open_store'); },
  get open_app() { return t('suiteWords.routeWord.open_app'); },
  get open_official_page() { return t('suiteWords.routeWord.open_official_page'); },
  get none() { return t('suiteWords.routeWord.none'); },
};


/** Update check and routing reasons from the core, in words; anything else is shown as sent. The macOS reasons come
 * first: their prefixes are longer than the Windows ones they share a start with. */
const UPDATE_REASONS: [string, TextKey][] = [
  ['No update channel: no Sparkle feed', 'suiteWords.updateReasons.noUpdateChannelInfo'],
  ['The Mac App Store checks its own apps', 'suiteWords.updateReasons.macAppStoreChecks'],
  ['Part of macOS', 'suiteWords.updateReasons.partMacosUpdatedSystem'],
  ['Uses Sparkle but its Info.plist declares no SUFeedURL', 'suiteWords.updateReasons.usesSparkleButInfo'],
  ['The Sparkle feed is not an https URL', 'suiteWords.updateReasons.sparkleFeedNotHttps'],
  ['The Sparkle feed was not fetched', 'suiteWords.updateReasons.sparkleFeedNotRead'],
  ['The Sparkle feed could not be read', 'suiteWords.updateReasons.sparkleFeedUnreadable'],
  ["The app's Info.plist has no CFBundleVersion", 'suiteWords.updateReasons.appInfoPlistHas'],
  ['The Sparkle feed lists no macOS release', 'suiteWords.updateReasons.sparkleFeedListsNo'],
  ['Sparkle feed offers', 'suiteWords.updateReasons.sparkleFeedOffersNewer'],
  ['Compared with the Sparkle feed', 'suiteWords.updateReasons.comparedSparkleFeed'],
  ['Homebrew was not queried', 'suiteWords.updateReasons.homebrewNotQueried'],
  ['Homebrew could not be queried', 'suiteWords.updateReasons.homebrewQueryFailed'],
  ['Pinned in Homebrew', 'suiteWords.updateReasons.pinnedHomebrewRunBrew'],
  ['Homebrew upgrade available', 'suiteWords.updateReasons.homebrewUpgradeAvailableHomebrew'],
  ['Not outdated in Homebrew', 'suiteWords.updateReasons.homebrewReportsUpDate'],
  ['Updates itself through', 'suiteWords.updateReasons.appUpdatesThroughUpdater'],
  ['The cask is marked auto_updates', 'suiteWords.updateReasons.caskMarkedAutoUpdating'],
  ['No update channel', 'suiteWords.updateReasons.noUpdateChannelNot'],
  ['Updates itself', 'suiteWords.updateReasons.appHasUpdaterWhose'],
  ['Microsoft Store checks its own apps', 'suiteWords.updateReasons.microsoftStoreChecksApps'],
  ['Microsoft Store updates its own apps', 'suiteWords.updateReasons.microsoftStoreUpdatesApps'],
  ['Update checks are turned off', 'suiteWords.updateReasons.updateChecksTurnedOff'],
  ['winget could not be queried', 'suiteWords.updateReasons.wingetQueryFailed'],
  ['the installed app has no verifiable signer', 'suiteWords.updateReasons.installedAppHasNo'],
  ['the package manager for this app was not found', 'suiteWords.updateReasons.packageManagerNotFound'],
  ['Compared with the local bucket copy', 'suiteWords.updateReasons.comparedLocalBucketCopy'],
];

export function updateReason(text: string | null): string | null {
  if (!text) return null;
  const hit = UPDATE_REASONS.find(([prefix]) => text.startsWith(prefix));
  return hit ? t(hit[1]) : text;
}

// ---------------------------------------------------------------------------------------------
// Maintenance

export const TASK_WORDS: Record<string, { title: string; problem: string; impact: string; applies: string }> = {
  'windows.shell-restart': {
    get title() { return t('suiteWords.taskWords.windows.shell-restart.title'); },
    get problem() { return t('suiteWords.taskWords.windows.shell-restart.problem'); },
    get impact() { return t('suiteWords.taskWords.windows.shell-restart.impact'); },
    get applies() { return t('suiteWords.taskWords.windows.shell-restart.applies'); },
  },
  'windows.icon-cache-rebuild': {
    get title() { return t('suiteWords.taskWords.windows.icon-cache-rebuild.title'); },
    get problem() { return t('suiteWords.taskWords.windows.icon-cache-rebuild.problem'); },
    get impact() { return t('suiteWords.taskWords.windows.icon-cache-rebuild.impact'); },
    get applies() { return t('suiteWords.taskWords.windows.icon-cache-rebuild.applies'); },
  },
  'windows.dns-flush': {
    get title() { return t('suiteWords.taskWords.windows.dns-flush.title'); },
    get problem() { return t('suiteWords.taskWords.windows.dns-flush.problem'); },
    get impact() { return t('suiteWords.taskWords.windows.dns-flush.impact'); },
    get applies() { return t('suiteWords.taskWords.windows.dns-flush.applies'); },
  },
  'windows.trim-when-due': {
    get title() { return t('suiteWords.taskWords.windows.trim-when-due.title'); },
    get problem() { return t('suiteWords.taskWords.windows.trim-when-due.problem'); },
    get impact() { return t('suiteWords.taskWords.windows.trim-when-due.impact'); },
    get applies() { return t('suiteWords.taskWords.windows.trim-when-due.applies'); },
  },
  'windows.image-health-check': {
    get title() { return t('suiteWords.taskWords.windows.image-health-check.title'); },
    get problem() { return t('suiteWords.taskWords.windows.image-health-check.problem'); },
    get impact() { return t('suiteWords.taskWords.windows.image-health-check.impact'); },
    get applies() { return t('suiteWords.taskWords.windows.image-health-check.applies'); },
  },
};

export const PROHIBITED_WORDS: Record<string, { title: string; reason: string }> = {
  'registry-optimizer': { get title() { return t('suiteWords.prohibitedWords.registry-optimizer.title'); }, get reason() { return t('suiteWords.prohibitedWords.registry-optimizer.reason'); } },
  'performance-privacy-tweaks': { get title() { return t('suiteWords.prohibitedWords.performance-privacy-tweaks.title'); }, get reason() { return t('suiteWords.prohibitedWords.performance-privacy-tweaks.reason'); } },
  'service-disable': { get title() { return t('suiteWords.prohibitedWords.service-disable.title'); }, get reason() { return t('suiteWords.prohibitedWords.service-disable.reason'); } },
  'search-indexing-disable': { get title() { return t('suiteWords.prohibitedWords.search-indexing-disable.title'); }, get reason() { return t('suiteWords.prohibitedWords.search-indexing-disable.reason'); } },
  'system-debloat': { get title() { return t('suiteWords.prohibitedWords.system-debloat.title'); }, get reason() { return t('suiteWords.prohibitedWords.system-debloat.reason'); } },
  'memory-release': { get title() { return t('suiteWords.prohibitedWords.memory-release.title'); }, get reason() { return t('suiteWords.prohibitedWords.memory-release.reason'); } },
  'pagefile-tweak': { get title() { return t('suiteWords.prohibitedWords.pagefile-tweak.title'); }, get reason() { return t('suiteWords.prohibitedWords.pagefile-tweak.reason'); } },
  'rebuild-app-registrations': { get title() { return t('suiteWords.prohibitedWords.rebuild-app-registrations.title'); }, get reason() { return t('suiteWords.prohibitedWords.rebuild-app-registrations.reason'); } },
  'ssd-full-defrag': { get title() { return t('suiteWords.prohibitedWords.ssd-full-defrag.title'); }, get reason() { return t('suiteWords.prohibitedWords.ssd-full-defrag.reason'); } },
  'one-click-speedup': { get title() { return t('suiteWords.prohibitedWords.one-click-speedup.title'); }, get reason() { return t('suiteWords.prohibitedWords.one-click-speedup.reason'); } },
};

export const APPLICABILITY: Record<Applicability, { label: string; tone: 'good' | 'warn' | 'muted' }> = {
  applicable: { get label() { return t('suiteWords.applicability.applicable.label'); }, tone: 'good' },
  not_needed: { get label() { return t('suiteWords.applicability.not_needed.label'); }, tone: 'muted' },
  not_applicable: { get label() { return t('suiteWords.applicability.not_applicable.label'); }, tone: 'muted' },
  unknown: { get label() { return t('suiteWords.applicability.unknown.label'); }, tone: 'warn' },
};

/** The six maintenance outcomes (MNT-03), each with its own shape. */
export const OUTCOME: Record<TaskOutcome, { label: string; explain: string }> = {
  applied: { get label() { return t('suiteWords.outcome.applied.label'); }, get explain() { return t('suiteWords.outcome.applied.explain'); } },
  unchanged: { get label() { return t('suiteWords.outcome.unchanged.label'); }, get explain() { return t('suiteWords.outcome.unchanged.explain'); } },
  attention: { get label() { return t('suiteWords.outcome.attention.label'); }, get explain() { return t('suiteWords.outcome.attention.explain'); } },
  skipped: { get label() { return t('suiteWords.outcome.skipped.label'); }, get explain() { return t('suiteWords.outcome.skipped.explain'); } },
  unavailable: { get label() { return t('suiteWords.outcome.unavailable.label'); }, get explain() { return t('suiteWords.outcome.unavailable.explain'); } },
  failed: { get label() { return t('suiteWords.outcome.failed.label'); }, get explain() { return t('suiteWords.outcome.failed.explain'); } },
};

export const OUTCOME_ORDER: TaskOutcome[] = ['applied', 'unchanged', 'attention', 'skipped', 'unavailable', 'failed'];

export const ELEVATION_WORD: Record<string, string> = {
  get not_needed() { return t('suiteWords.elevationWord.not_needed'); },
  get already_elevated() { return t('suiteWords.elevationWord.already_elevated'); },
  get granted() { return t('suiteWords.elevationWord.granted'); },
  get declined() { return t('suiteWords.elevationWord.declined'); },
  get unavailable() { return t('suiteWords.elevationWord.unavailable'); },
};

/** Chinese for the reason codes of skipped tasks. */
export const TASK_CODE_WORD: Record<string, string> = {
  get elevation_declined() { return t('suiteWords.taskCodeWord.elevation_declined'); },
  get elevation_unavailable() { return t('suiteWords.taskCodeWord.elevation_unavailable'); },
  get protected() { return t('suiteWords.taskCodeWord.protected'); },
  get task_version_changed() { return t('suiteWords.taskCodeWord.task_version_changed'); },
  get state_unknown() { return t('suiteWords.taskCodeWord.state_unknown'); },
  get cancelled() { return t('suiteWords.taskCodeWord.cancelled'); },
};

export const FINDING_WORD: Record<string, { title: string; next: string | null }> = {
  runaway_process: { get title() { return t('suiteWords.findingWord.runaway_process.title'); }, get next() { return t('suiteWords.findingWord.runaway_process.next'); } },
  busy_process: { get title() { return t('suiteWords.findingWord.busy_process.title'); }, get next() { return t('suiteWords.findingWord.busy_process.next'); } },
  memory_pressure: { get title() { return t('suiteWords.findingWord.memory_pressure.title'); }, next: null },
  commit_pressure: { get title() { return t('suiteWords.findingWord.commit_pressure.title'); }, next: null },
  low_disk_space: { get title() { return t('suiteWords.findingWord.low_disk_space.title'); }, get next() { return t('suiteWords.findingWord.low_disk_space.next'); } },
  idle_virtual_machine: { get title() { return t('suiteWords.findingWord.idle_virtual_machine.title'); }, next: null },
  attached_disk_image: { get title() { return t('suiteWords.findingWord.attached_disk_image.title'); }, next: null },
  many_startup_items: { get title() { return t('suiteWords.findingWord.many_startup_items.title'); }, get next() { return t('suiteWords.findingWord.many_startup_items.next'); } },
};

// ---------------------------------------------------------------------------------------------
// Processes

export const SIGNATURE_WORD: Record<string, string> = {
  get signed() { return t('suiteWords.signatureWord.signed'); },
  get catalog_signed() { return t('suiteWords.signatureWord.catalog_signed'); },
  get unsigned() { return t('suiteWords.signatureWord.unsigned'); },
  get unknown() { return t('suiteWords.signatureWord.unknown'); },
};

export const PROTECTION_WORD: Record<string, string> = {
  get critical_system() { return t('suiteWords.protectionWord.critical_system'); },
  get os_critical() { return t('suiteWords.protectionWord.os_critical'); },
  get clean_manager() { return t('suiteWords.protectionWord.clean_manager'); },
  get security_software() { return t('suiteWords.protectionWord.security_software'); },
  get other_user() { return t('suiteWords.protectionWord.other_user'); },
};

export const SORT_WORD: Record<string, string> = { get cpu() { return t('suiteWords.sortWord.cpu'); }, get memory() { return t('suiteWords.sortWord.memory'); }, get disk() { return t('suiteWords.sortWord.disk'); }, get network() { return t('suiteWords.sortWord.network'); }, get name() { return t('suiteWords.sortWord.name'); } };

// ---------------------------------------------------------------------------------------------
// macOS (GUI-12): words for what only the macOS host reports. macOS readings of shared entries are the `@macos` keys
// of the resources (src/i18n): the Windows entries above stay exactly as written, and `t` picks the macOS reading
// while the platform is macOS.

Object.assign(TASK_WORDS, {
  'macos.quicklook-reset': {
    get title() { return t('suiteWords.taskWords.macos.quicklook-reset.title'); },
    get problem() { return t('suiteWords.taskWords.macos.quicklook-reset.problem'); },
    get impact() { return t('suiteWords.taskWords.macos.quicklook-reset.impact'); },
    get applies() { return t('suiteWords.taskWords.macos.quicklook-reset.applies'); },
  },
  'macos.icon-cache-rebuild': {
    get title() { return t('suiteWords.taskWords.macos.icon-cache-rebuild.title'); },
    get problem() { return t('suiteWords.taskWords.macos.icon-cache-rebuild.problem'); },
    get impact() { return t('suiteWords.taskWords.macos.icon-cache-rebuild.impact'); },
    get applies() { return t('suiteWords.taskWords.macos.icon-cache-rebuild.applies'); },
  },
  'macos.dns-flush': {
    get title() { return t('suiteWords.taskWords.macos.dns-flush.title'); },
    get problem() { return t('suiteWords.taskWords.macos.dns-flush.problem'); },
    get impact() { return t('suiteWords.taskWords.macos.dns-flush.impact'); },
    get applies() { return t('suiteWords.taskWords.macos.dns-flush.applies'); },
  },
  'macos.spotlight-status': {
    get title() { return t('suiteWords.taskWords.macos.spotlight-status.title'); },
    get problem() { return t('suiteWords.taskWords.macos.spotlight-status.problem'); },
    get impact() { return t('suiteWords.taskWords.macos.spotlight-status.impact'); },
    get applies() { return t('suiteWords.taskWords.macos.spotlight-status.applies'); },
  },
  'macos.broken-preferences': {
    get title() { return t('suiteWords.taskWords.macos.broken-preferences.title'); },
    get problem() { return t('suiteWords.taskWords.macos.broken-preferences.problem'); },
    get impact() { return t('suiteWords.taskWords.macos.broken-preferences.impact'); },
    get applies() { return t('suiteWords.taskWords.macos.broken-preferences.applies'); },
  },
  'macos.stale-saved-state': {
    get title() { return t('suiteWords.taskWords.macos.stale-saved-state.title'); },
    get problem() { return t('suiteWords.taskWords.macos.stale-saved-state.problem'); },
    get impact() { return t('suiteWords.taskWords.macos.stale-saved-state.impact'); },
    get applies() { return t('suiteWords.taskWords.macos.stale-saved-state.applies'); },
  },
  'macos.disk-verify': {
    get title() { return t('suiteWords.taskWords.macos.disk-verify.title'); },
    get problem() { return t('suiteWords.taskWords.macos.disk-verify.problem'); },
    get impact() { return t('suiteWords.taskWords.macos.disk-verify.impact'); },
    get applies() { return t('suiteWords.taskWords.macos.disk-verify.applies'); },
  },
  'macos.launch-agents-report': {
    get title() { return t('suiteWords.taskWords.macos.launch-agents-report.title'); },
    get problem() { return t('suiteWords.taskWords.macos.launch-agents-report.problem'); },
    get impact() { return t('suiteWords.taskWords.macos.launch-agents-report.impact'); },
    get applies() { return t('suiteWords.taskWords.macos.launch-agents-report.applies'); },
  },
  'macos.network-dsstore-off': {
    get title() { return t('suiteWords.taskWords.macos.network-dsstore-off.title'); },
    get problem() { return t('suiteWords.taskWords.macos.network-dsstore-off.problem'); },
    get impact() { return t('suiteWords.taskWords.macos.network-dsstore-off.impact'); },
    get applies() { return t('suiteWords.taskWords.macos.network-dsstore-off.applies'); },
  },
  'macos.spotlight-rebuild': {
    get title() { return t('suiteWords.taskWords.macos.spotlight-rebuild.title'); },
    get problem() { return t('suiteWords.taskWords.macos.spotlight-rebuild.problem'); },
    get impact() { return t('suiteWords.taskWords.macos.spotlight-rebuild.impact'); },
    get applies() { return t('suiteWords.taskWords.macos.spotlight-rebuild.applies'); },
  },
});






/** Process notes only the macOS host writes, in words; anything else is shown as sent. */
const PROCESS_NOTES: [string, TextKey][] = [
  ['Exited and waiting for its parent to collect its status (zombie)', 'suiteWords.processNotes.exitedWaitingParentCollect'],
  ['The executable path is withheld or unavailable', 'suiteWords.processNotes.executablePathWithheld'],
];

export function processNote(text: string): string {
  const hit = PROCESS_NOTES.find(([prefix]) => text.startsWith(prefix));
  return hit ? t(hit[1]) : text;
}

/** Status reasons only the macOS host writes, in words; anything else is shown as sent. */
const METRIC_REASONS: [string, TextKey][] = [
  ['macOS publishes temperatures and fan speeds only through the System Management Controller', 'suiteWords.metricReasons.macosPublishesTemperaturesFan'],
  ['No IOAccelerator service is registered', 'suiteWords.metricReasons.noGraphicsServiceRegistered'],
  ['The GPU driver publishes no utilization', 'suiteWords.metricReasons.gpuDriverPublishesNo'],
  ['the battery does not report a cycle count', 'suiteWords.metricReasons.noCycleCount'],
  ['the battery does not report its design and full-charge capacity', 'suiteWords.metricReasons.noDesignCapacity'],
  ['IOPSCopyPowerSourcesInfo returned no power-source snapshot', 'suiteWords.metricReasons.systemReturnedNoPower'],
  // Windows host (cm-platform windows/status.rs).
  ['No documented Windows interface reports GPU temperature', 'suiteWords.metricReasons.windowsHasNoDocumented'],
  ['GPU engine counters are not published', 'suiteWords.metricReasons.gpuDriverPublishesNo2'],
  ['Windows does not publish the memory compression store size', 'suiteWords.metricReasons.noMemoryCompressionSize'],
  ['no paging file is configured or the counter is not available', 'suiteWords.metricReasons.noPagingFileConfigured'],
  ['neither the interface table nor the Network Interface performance counters could be read', 'suiteWords.metricReasons.neitherNetworkAdapterList'],
  ["the battery reports 0 cycles, which its driver uses for 'not supported'", 'suiteWords.metricReasons.batteryReports0Cycles'],
];

/**
 * Host text that was not put in words: it belongs in 技术详情 or a tooltip, never as the main text. In Chinese that is any
 * text without Chinese characters (the host's English diagnostics); in English it is any text that is not one of the
 * interface texts (LIF-05), so a host message never passes for the page's own words.
 */
export function isRawHostText(text: string | null | undefined): boolean {
  if (!text) return false;
  return locale() === 'zh-CN' ? !/[㐀-鿿]/.test(text) : !isResourceText(text);
}

export function metricReason(text: string): string {
  const hit = METRIC_REASONS.find(([prefix]) => text.startsWith(prefix));
  return hit ? t(hit[1]) : text;
}

/** Uninstall routes of the macOS host (adapter ids), in words; Windows adapters keep the host's description. */
export function adapterText(a: { adapter_id: string; display: string }): string {
  const cmd = a.display.split(' (')[0];
  switch (a.adapter_id) {
    case 'macos.trash-app-bundle':
      return t('suiteWords.moveTrashRestorePut', { p: a.display.includes('installer package') ? t('suiteWords.installedInstallerPackageWhose') : '' });
    case 'homebrew.uninstall-cask':
      return t('suiteWords.homebrewCmdNeverUses', { cmd });
    case 'homebrew.uninstall-formula':
      return t('suiteWords.homebrewCmd', { cmd });
    default:
      // Windows: the command exactly as it will run, its kind in words (NEW-13).
      return adapterDisplay(a.display);
  }
}

/** How one row of 软件 › 卸载 reads: its badge, whether the Settings button applies, and what to do instead. */
export interface UninstallRowWords {
  label: string;
  tone: 'good' | 'warn' | 'muted';
  /** Windows: "在设置中卸载" opens Settings › Apps. */
  settingsButton: boolean;
  /** Where to remove it instead (macOS routes this page does not take). */
  guidance: string | null;
  /** The host's reason, shortened when the guidance already says the rest. */
  reason: string | null;
  /** macOS: the app needs an administrator to remove (the row says where to remove it instead). */
  needsAdmin: boolean;
}

/**
 * macOS `permission_required` (UNI-01): App Store apps and bundles this user cannot move need an administrator, and
 * this build has no privilege path, so the row says where to remove them instead (Launchpad or Finder). The host
 * names the code in `refusal_code` when it sends one; otherwise its reason text or the App Store source says so.
 */
export function uninstallRowWords(a: { state: string; source: string; reason: string | null; refusal_code?: string | null }, mac = platform() === 'macos'): UninstallRowWords {
  const st = UNINSTALL_STATE[a.state] ?? { label: a.state, tone: 'muted' as const };
  if (!mac) return { label: st.label, tone: st.tone, settingsButton: a.state === 'unverified', guidance: null, reason: a.reason, needsAdmin: false };
  const permission =
    a.state !== 'available' && a.state !== 'unsupported' && (a.refusal_code === 'permission_required' || a.source === 'app_store' || /administrator (rights|path)/.test(a.reason ?? ''));
  if (permission) {
    const why = a.reason?.split(': moving it needs')[0];
    return {
      label: t('common.needsAdministrator'),
      tone: 'warn',
      settingsButton: false,
      guidance:
        a.source === 'app_store'
          ? t('suiteWords.appStoreAppDeleting')
          : t('suiteWords.movingNeedsAdministratorRights'),
      reason: a.source === 'app_store' || !why || why === a.reason ? null : why,
      needsAdmin: true,
    };
  }
  if (a.state === 'unverified') return { label: t('suiteWords.uninstallManually'), tone: 'warn', settingsButton: false, guidance: t('suiteWords.quitAppMoveTrash'), reason: a.reason, needsAdmin: false };
  return { label: st.label, tone: st.tone, settingsButton: false, guidance: null, reason: a.reason, needsAdmin: false };
}
