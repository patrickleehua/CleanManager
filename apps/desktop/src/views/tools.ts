// Pages of the five-tool IA that are not catalogs: 软件 › 启动项 (STA-01..04), 分析 › 空间洞察
// (DIN-01..04: known objects, overview cards and the old-downloads review), 状态 › 系统状态
// (STS-01..03, STS-05 alerts), and the capability pages of entries that stay command-line only
// (Agent, browsing the recycle bin). Writes only ever create plans for the shared review page.

import type { App } from '../main';
import { formatBytes } from '../lib/bytes';
import { h, icon, keepFocus, replace } from '../lib/dom';
import { formatTime } from '../lib/format';
import { RequestGate } from '../lib/gate';
import { notOnThisPlatform, platform, words } from '../lib/platform';
import { isRawHostText, metricReason } from '../lib/suiteWords';
import { unknownCodeText } from '../lib/codes';
import { guidanceText, sizeNoteText } from '../lib/insightWords';
import { DEFAULT_PREFS, PANEL_LABEL, REFRESH_SECONDS, type StatusPanel, type StatusPrefs, layoutRows, loadPrefs, movePanel, sanitize, savePrefs, setShown } from '../lib/statusPrefs';
import type { CpuAlert, Insight, InsightsList, IpcError, NetworkTotals, StartupItem, StartupList, StatusMetric, StatusSnapshot } from '../lib/types';
import type { DownloadItem, InsightCard, InsightOverview } from '../lib/suiteTypes';
import { atLeast, capabilityPage, ensureReady, noticeLine, selectionBar, sumSizes, toIpcError } from './kit';
import { copyField, emptyState, errorLine, exactTitle, pageHead, pill, type Tone } from './parts';
import { issueList, learnMore, techBlock } from './ui';
import { t } from '../i18n';

interface PageLike {
  el: HTMLElement;
  update(): void;
  activate?(): void;
}

// ---------------------------------------------------------------------------------------------
// 软件 › 启动项

const SOURCE_WORD: Record<string, string> = {
  get run_key() { return t('tools.sourceWord.run_key'); },
  get startup_folder() { return t('tools.sourceWord.startup_folder'); },
  get scheduled_task() { return t('tools.sourceWord.scheduled_task'); },
  get store_startup_task() { return t('tools.sourceWord.store_startup_task'); },
  get sm_login_item() { return t('tools.sourceWord.sm_login_item'); },
  get sm_background_item() { return t('tools.sourceWord.sm_background_item'); },
  launchd_user: 'LaunchAgent',
  launchd_system: 'LaunchDaemon',
};

const STATE_WORD: Record<string, [string, Tone]> = {
  get enabled(): [string, Tone] { return [t('tools.stateWord.enabled'), 'good']; },
  get disabled(): [string, Tone] { return [t('tools.stateWord.disabled'), 'muted']; },
  get broken(): [string, Tone] { return [t('tools.stateWord.broken'), 'warn']; },
  get target_unreachable(): [string, Tone] { return [t('tools.stateWord.target_unreachable'), 'warn']; },
  get unknown(): [string, Tone] { return [t('tools.stateWord.unknown'), 'muted']; },
};

/** System Settings › General › Login Items & Extensions (macOS 13+): where Service Management items are reviewed. */
export const LOGIN_ITEMS_URI = 'x-apple.systempreferences:com.apple.LoginItems-Settings.extension';

/** How one startup row reads (STA-01..04). macOS rows are read-only in this build; the host says so per row. */
export interface StartupRowView {
  name: string;
  state: [string, Tone];
  /** Why the change button is unavailable (null: it can be planned). */
  why: string | null;
  /** The host's reason without what the page already says. */
  reason: string | null;
  /** macOS `unknown`: the state is not guessed. */
  stateNote: string | null;
  /** The Service Management row: it opens System Settings instead of a change plan. */
  settingsLink: boolean;
  /** Show the disable/enable button (macOS: only for a row the host says can change; none can in this build). */
  showAction: boolean;
}

export function startupRow(i: StartupItem, mac = platform() === 'macos'): StartupRowView {
  const state = STATE_WORD[i.state] ?? [i.state, 'muted' as Tone];
  const why = i.protected
    ? t('tools.protectedReadOnly')
    : i.toggle_capability === 'permission_required'
      ? t('common.needsAdmin', { admin: words().admin })
      : i.toggle_capability === 'unsupported'
        ? t('tools.noVerifiableWayChange')
        : i.toggle_capability === 'unverified'
          ? t('tools.changeMethodNotVerified')
          : null;
  if (!mac) return { name: i.name, state, why, reason: i.reason, stateNote: null, settingsLink: false, showAction: true };
  // Service Management login and background items have no public listing API: one honest row points to System Settings.
  if (!i.target && (i.source === 'sm_background_item' || i.source === 'sm_login_item' || i.definition === LOGIN_ITEMS_URI)) {
    return {
      name: t('tools.loginItemsBackgroundItems'),
      state,
      why: null,
      reason: t('tools.macosNoPublicInterface'),
      stateNote: null,
      settingsLink: true,
      showAction: false,
    };
  }
  // The host appends the platform-wide read-only sentence to every reason; the page says it once instead.
  const rest = (i.reason ?? '')
    .split('. ')
    .filter((x) => x && !x.startsWith('Read-only on macOS in this build'))
    .join('. ');
  return {
    name: i.name,
    state,
    why: i.protected ? t('tools.protectedReadOnly') : i.toggle_capability === 'unsupported' ? t('tools.readOnlyChangeApps') : why,
    reason: rest || null,
    stateNote: i.state === 'unknown' ? t('tools.launchdDoesntListLoaded') : null,
    settingsLink: false,
    showAction: i.toggle_capability !== 'unsupported' && !i.protected,
  };
}

export class StartupPage implements PageLike {
  el: HTMLElement;
  private body: HTMLElement;
  private data: StartupList | null = null;
  private error: IpcError | null = null;
  private rowError = new Map<string, IpcError>();
  private busy: string | null = null;
  private gate = new RequestGate();
  private loading = false;
  private settingsError: string | null = null;

  constructor(private app: App) {
    this.body = h('div', { class: 'startup-body' });
    this.el = h('section', { class: 'page page-startup', 'aria-labelledby': 'startup-title' }, this.body);
  }

  activate(): void {
    void this.load();
  }

  update(): void {
    keepFocus(this.el, () => this.render());
  }

  private async load(): Promise<void> {
    this.loading = true;
    this.error = null;
    this.update();
    try {
      const r = await this.gate.latest(this.app.backend.startupList());
      if (r.stale) return;
      this.data = r.value;
    } catch (e) {
      this.error = toIpcError(e);
    } finally {
      this.loading = false;
      this.update();
    }
  }

  private async plan(i: StartupItem, op: 'disable' | 'enable'): Promise<void> {
    if (this.busy) return;
    if (!(await ensureReady(this.app, 'startup', op === 'disable' ? t('tools.disableStartupItem') : t('tools.enableStartupItem')))) return;
    this.busy = i.startup_item_id;
    this.rowError.delete(i.startup_item_id);
    this.update();
    try {
      const r = await this.app.backend.startupPlan(i.startup_item_id, op);
      this.app.announce(t('tools.planCreatedNotRun'));
      this.app.openPlan(r.plan_id, 'startup');
    } catch (e) {
      this.rowError.set(i.startup_item_id, toIpcError(e));
    } finally {
      this.busy = null;
      this.update();
    }
  }

  private render(): void {
    const d = this.data;
    const refresh = h('button', { type: 'button', id: 'startup-refresh', disabled: this.loading }, icon('refresh'), this.loading ? t('common.reading') : t('common.reload'));
    refresh.addEventListener('click', () => void this.load());
    const w = words();
    const mac = platform() === 'macos';
    if (d && !d.supported) {
      replace(this.body, pageHead({ id: 'startup-title', title: t('tools.startupItems') }), emptyState(t('common.notAvailablePlatform'), notOnThisPlatform(t('tools.startupItemListCurrently')), null));
      return;
    }
    replace(
      this.body,
      pageHead({
        id: 'startup-title',
        title: t('tools.startupItems'),
        meta: d ? [t('common.itemCount', { length: d.items.length }), d.is_admin ? t('common.runningAdmin', { admin: w.admin }) : t('common.runningCurrentUser'), t('common.readOnlyList')] : [t('common.readOnlyList')],
        actions: [refresh],
      }),
      mac
        ? noticeLine(t('tools.readOnlyMacosVersion', { startupSettings: w.startupSettings }))
        : d && !d.is_admin
          ? noticeLine(t('tools.noAdminStartupItems', { admin: w.admin }), 'warn')
          : null,
      errorLine(this.error),
      d
        ? h(
            'div',
            { class: 'work-layout' },
            h('div', { class: 'work-main' }, h('section', { class: 'panel', 'aria-labelledby': 'startup-list-title' }, h('h2', { id: 'startup-list-title' }, t('tools.programsStartSign')), this.table(d.items), this.issues(d))),
            mac
              ? h(
                  'aside',
                  { class: 'panel work-aside', 'aria-labelledby': 'startup-how' },
                  h('h2', { id: 'startup-how' }, t('tools.howChangeThem')),
                  h('p', { class: 'small' }, t('tools.versionOnlyListsStartup', { startupSettings: w.startupSettings })),
                  this.settingsButton('startup-open-settings'),
                  learnMore(h('p', {}, t('tools.stateComesLaunchdJobs'))),
                )
              : h(
                  'aside',
                  { class: 'panel work-aside', 'aria-labelledby': 'startup-how' },
                  h('h2', { id: 'startup-how' }, t('tools.howChangeThem')),
                  h('p', { class: 'small' }, t('tools.disablingEnablingChangeOnly', { startupSettings: w.startupSettings })),
                  learnMore(h('p', {}, t('tools.everyChangeFirstCreates')), h('p', {}, t('tools.securitySoftwareSystemComponents'))),
                ),
          )
        : !this.error
          ? h('p', { class: 'muted', role: 'status' }, t('tools.readingStartupItems'))
          : null,
    );
  }

  private table(items: StartupItem[]): HTMLElement {
    if (!items.length) return h('p', { class: 'muted' }, t('tools.noStartupItemsFound'));
    const rows = items.map((i) => {
      const v = startupRow(i);
      const [word, tone] = v.state;
      const id = `st-${i.startup_item_id}`;
      const op: 'disable' | 'enable' = i.state === 'disabled' ? 'enable' : 'disable';
      const why = v.why;
      let act: HTMLElement | null = null;
      if (v.settingsLink) act = this.settingsButton(`${id}-settings`);
      else if (v.showAction) {
        act = h('button', { type: 'button', class: 'quiet', id: `${id}-act`, disabled: !!why || this.busy !== null || i.state === 'broken' }, op === 'disable' ? t('tools.disable') : t('common.enable'));
        act.addEventListener('click', () => void this.plan(i, op));
      }
      const err = this.rowError.get(i.startup_item_id);
      return h(
        'tr',
        {},
        h(
          'td',
          { class: 'col-object' },
          h('span', { class: 'obj-name' }, v.name),
          v.settingsLink ? null : h('span', { class: 'obj-path path' }, i.target?.display ?? i.definition),
          h('span', { class: 'obj-reason muted' }, `${SOURCE_WORD[i.source] ?? i.source} · ${i.scope === 'user' ? t('tools.currentUser') : t('tools.allUsers')}${i.publisher ? ` · ${i.publisher}` : t('tools.publisherNotVerified')}`),
          // A host reason without words is a technical detail; the state and the action column say it in Chinese.
          v.reason && !isRawHostText(v.reason) ? h('span', { class: v.settingsLink ? 'obj-reason' : 'why-not' }, v.reason) : null,
          v.reason && isRawHostText(v.reason) ? techBlock(h('p', { class: 'selectable small' }, v.reason)) : null,
          errorLine(err ?? null),
        ),
        h('td', { class: 'col-flags' }, pill(word, tone), v.stateNote ? h('span', { class: 'small muted block' }, v.stateNote) : null),
        h('td', { class: 'col-status' }, act, why ? h('span', { class: 'why-not' }, why) : null),
      );
    });
    return h(
      'div',
      { class: 'table-wrap' },
      h(
        'table',
        { class: 'data-table startup-table' },
        h('caption', { class: 'sr-only' }, t('tools.startupItemsStateAvailable')),
        h('thead', {}, h('tr', {}, h('th', { scope: 'col' }, t('tools.programSource')), h('th', { scope: 'col', class: 'col-flags' }, t('tools.state')), h('th', { scope: 'col', class: 'col-status' }, t('common.action')))),
        h('tbody', {}, ...rows),
      ),
    );
  }

  /** macOS: open System Settings › General › Login Items & Extensions (the host opens only that fixed page). */
  private settingsButton(id: string): HTMLElement {
    const b = h('button', { type: 'button', class: 'quiet', id }, icon('external'), t('tools.openLoginItemsSettings'));
    b.addEventListener('click', () => void this.openLoginItems());
    return h('span', { class: 'block' }, b, this.settingsError ? h('span', { class: 'form-error', role: 'alert' }, this.settingsError) : null);
  }

  private async openLoginItems(): Promise<void> {
    this.settingsError = null;
    try {
      if (!(await this.app.backend.openSystemPage('login_items'))) throw new Error('not opened');
    } catch {
      this.settingsError = t('tools.couldntOpenSystemSettings', { startupSettings: words().startupSettings });
    }
    this.update();
  }

  private issues(d: StartupList): HTMLElement | null {
    if (!d.issues.length) return null;
    return h(
      'details',
      { class: 'issues-details' },
      h('summary', {}, t('tools.sourcesCouldntReadLength', { length: d.issues.length })),
      issueList(d.issues, { label: t('common.recordedIssues') }),
    );
  }
}

// ---------------------------------------------------------------------------------------------
// 分析 › 空间洞察

const INSIGHT_TITLE: Record<string, [string, string]> = {
  get 'windows.hibernation_file'(): [string, string] { return [t('tools.insightTitle.windows.hibernation_file.title'), t('tools.insightTitle.windows.hibernation_file.note')]; },
  get 'windows.page_file'(): [string, string] { return [t('tools.insightTitle.windows.page_file.title'), t('tools.insightTitle.windows.page_file.note')]; },
  get 'windows.swap_file'(): [string, string] { return [t('tools.insightTitle.windows.swap_file.title'), t('tools.insightTitle.windows.swap_file.note')]; },
  get 'windows.component_store'(): [string, string] { return [t('tools.insightTitle.windows.component_store.title'), t('tools.insightTitle.windows.component_store.note')]; },
  get 'windows.shadow_storage'(): [string, string] { return [t('tools.insightTitle.windows.shadow_storage.title'), t('tools.insightTitle.windows.shadow_storage.note')]; },
  get 'windows.wsl_disk'(): [string, string] { return [t('tools.insightTitle.windows.wsl_disk.title'), t('tools.insightTitle.windows.wsl_disk.note')]; },
  get 'windows.docker_disk'(): [string, string] { return [t('tools.insightTitle.windows.docker_disk.title'), t('tools.insightTitle.windows.docker_disk.note')]; },
  get 'windows.hyperv_disks'(): [string, string] { return [t('tools.insightTitle.windows.hyperv_disks.title'), t('tools.insightTitle.windows.hyperv_disks.note')]; },
  get 'windows.previous_installation'(): [string, string] { return [t('tools.insightTitle.windows.previous_installation.title'), t('tools.insightTitle.windows.previous_installation.note')]; },
  get 'windows.delivery_optimization_cache'(): [string, string] { return [t('tools.insightTitle.windows.delivery_optimization_cache.title'), t('tools.insightTitle.windows.delivery_optimization_cache.note')]; },
  get 'macos.local_snapshots'(): [string, string] { return [t('tools.insightTitle.macos.local_snapshots.title'), t('tools.insightTitle.macos.local_snapshots.note')]; },
  get 'macos.docker_disk'(): [string, string] { return [t('tools.insightTitle.macos.docker_disk.title'), t('tools.insightTitle.macos.docker_disk.note')]; },
  get 'macos.orbstack_disk'(): [string, string] { return [t('tools.insightTitle.macos.orbstack_disk.title'), t('tools.insightTitle.macos.orbstack_disk.note')]; },
  get 'macos.parallels_vms'(): [string, string] { return [t('tools.insightTitle.macos.parallels_vms.title'), t('tools.insightTitle.macos.parallels_vms.note')]; },
  get 'macos.vmware_vms'(): [string, string] { return [t('tools.insightTitle.macos.vmware_vms.title'), t('tools.insightTitle.macos.vmware_vms.note')]; },
  get 'macos.utm_vms'(): [string, string] { return [t('tools.insightTitle.macos.utm_vms.title'), t('tools.insightTitle.macos.utm_vms.note')]; },
  get 'macos.lima_vms'(): [string, string] { return [t('tools.insightTitle.macos.lima_vms.title'), t('tools.insightTitle.macos.lima_vms.note')]; },
  get 'macos.colima_vms'(): [string, string] { return [t('tools.insightTitle.macos.colima_vms.title'), t('tools.insightTitle.macos.colima_vms.note')]; },
  get 'macos.device_backups'(): [string, string] { return [t('tools.insightTitle.macos.device_backups.title'), t('tools.insightTitle.macos.device_backups.note')]; },
  get 'macos.swap'(): [string, string] { return [t('tools.insightTitle.macos.swap.title'), t('tools.insightTitle.macos.swap.note')]; },
  get 'macos.purgeable_space'(): [string, string] { return [t('tools.insightTitle.macos.purgeable_space.title'), t('tools.insightTitle.macos.purgeable_space.note')]; },
  get old_downloads(): [string, string] { return [t('tools.insightTitle.old_downloads.title'), t('tools.insightTitle.old_downloads.note')]; },
};

const CARD_OF: Record<string, string> = {
  get 'windows.wsl_disk'() { return t('tools.cardOf.windows.wsl_disk'); },
  get 'windows.docker_disk'() { return t('tools.cardOf.windows.docker_disk'); },
  get 'windows.hyperv_disks'() { return t('tools.cardOf.windows.hyperv_disks'); },
  get 'windows.shadow_storage'() { return t('tools.cardOf.windows.shadow_storage'); },
  get 'windows.hibernation_file'() { return t('tools.cardOf.windows.hibernation_file'); },
  get 'windows.page_file'() { return t('tools.cardOf.windows.page_file'); },
  get 'windows.swap_file'() { return t('tools.cardOf.windows.swap_file'); },
  get 'windows.component_store'() { return t('tools.cardOf.windows.component_store'); },
  get 'windows.previous_installation'() { return t('tools.cardOf.windows.previous_installation'); },
  get 'windows.delivery_optimization_cache'() { return t('tools.cardOf.windows.delivery_optimization_cache'); },
  get 'macos.local_snapshots'() { return t('tools.cardOf.macos.local_snapshots'); },
  get 'macos.docker_disk'() { return t('tools.cardOf.macos.docker_disk'); },
  get 'macos.orbstack_disk'() { return t('tools.cardOf.macos.orbstack_disk'); },
  get 'macos.parallels_vms'() { return t('tools.cardOf.macos.parallels_vms'); },
  get 'macos.vmware_vms'() { return t('tools.cardOf.macos.vmware_vms'); },
  get 'macos.utm_vms'() { return t('tools.cardOf.macos.utm_vms'); },
  get 'macos.lima_vms'() { return t('tools.cardOf.macos.lima_vms'); },
  get 'macos.colima_vms'() { return t('tools.cardOf.macos.colima_vms'); },
  get 'macos.device_backups'() { return t('tools.cardOf.macos.device_backups'); },
  get 'macos.swap'() { return t('tools.cardOf.macos.swap'); },
  get 'macos.purgeable_space'() { return t('tools.cardOf.macos.purgeable_space'); },
  get old_downloads() { return t('tools.cardOf.old_downloads'); },
};

/** DIN-03 cards: known size or unknown, and when it was observed. */
export function insightCards(items: Insight[]): { title: string; size: string; observed: string | null; count: number }[] {
  const cards = new Map<string, Insight[]>();
  for (const i of items) {
    const c = CARD_OF[i.kind] ?? t('common.other');
    cards.set(c, [...(cards.get(c) ?? []), i]);
  }
  return [...cards.entries()].map(([title, members]) => {
    const sum = sumSizes(members.map((m) => ({ bytes: m.logical_bytes })));
    const observed = members.map((m) => m.observed_at).filter((x): x is string => !!x).sort()[0] ?? null;
    return { title, size: sum.known === 0n && sum.unknown ? t('common.sizeUnknown') : atLeast(sum), observed, count: members.length };
  });
}

/** DIN-03 overview card titles by kind (the host's cards: known size or unknown, count, oldest observation). */
export const CARD_TITLE: Record<string, string> = {
  get old_downloads() { return t('tools.cardTitle.old_downloads'); },
  get virtual_machines() { return t('tools.cardTitle.virtual_machines'); },
  get snapshots() { return t('tools.cardTitle.snapshots'); },
  get system_files() { return t('tools.cardTitle.system_files'); },
  get component_store() { return t('tools.cardTitle.component_store'); },
  get previous_installation() { return t('tools.cardTitle.previous_installation'); },
  get update_cache() { return t('tools.cardTitle.update_cache'); },
  get developer_tools() { return t('tools.cardTitle.developer_tools'); },
  get model_libraries() { return t('tools.cardTitle.model_libraries'); },
  get device_backups() { return t('tools.cardTitle.device_backups'); },
  get purgeable_space() { return t('tools.cardTitle.purgeable_space'); },
};

/** One overview card as shown: never a partial sum presented as the total. */
/** Whether a host note says macOS has not answered its privacy prompt yet (`permission_cause=privacy_prompt_pending`). */
export function privacyPending(note: string | null | undefined): boolean {
  return !!note && note.includes('permission_cause=privacy_prompt_pending');
}

export const privacyPendingSize = () => t('tools.privacyPendingSize');
export const privacyPendingText = () => t('tools.privacyPendingText');

export function overviewCard(c: InsightCard): { title: string; size: string; note: string | null; noteRaw: string | null; count: number | null; observed: string | null; pending: boolean } {
  const pending = c.logical_bytes === null && privacyPending(c.size_note);
  const size = pending ? privacyPendingSize() : c.logical_bytes === null ? (c.count !== null ? t('tools.countItems', { count: c.count }) : t('common.sizeUnknown')) : formatBytes(c.logical_bytes);
  // NEW-13: the host's English note is the tooltip; the card says it in words.
  const note = !pending && c.logical_bytes === null ? sizeNoteText(c.size_note) : null;
  return { title: CARD_TITLE[c.kind] ?? c.title, size, note: pending ? privacyPendingText() : (note?.text ?? null), noteRaw: note?.raw ?? null, count: c.count, observed: c.observed_at, pending };
}

const OLD_DAYS = 90;

export class InsightsPage implements PageLike {
  el: HTMLElement;
  private body: HTMLElement;
  private data: InsightsList | null = null;
  private overview: InsightOverview | null = null;
  private error: IpcError | null = null;
  private ovError: IpcError | null = null;
  private gate = new RequestGate();
  private ovGate = new RequestGate();
  private dlSelected = new Map<string, DownloadItem>();
  private creating = false;
  private createError: IpcError | null = null;

  constructor(private app: App) {
    this.body = h('div', {});
    this.el = h('section', { class: 'page page-insights', 'aria-labelledby': 'insights-title' }, this.body);
  }

  activate(): void {
    if (!this.data) void this.load();
  }

  update(): void {
    keepFocus(this.el, () => this.render());
  }

  private async load(): Promise<void> {
    this.error = null;
    this.ovError = null;
    const list = this.gate.latest(this.app.backend.insightsList());
    const ov = this.ovGate.latest(this.app.backend.insightsOverview(OLD_DAYS));
    try {
      const r = await list;
      if (!r.stale) this.data = r.value;
    } catch (e) {
      this.error = toIpcError(e);
    }
    this.update();
    try {
      const r = await ov;
      if (!r.stale) {
        this.overview = r.value;
        // A new review has new item ids: an old selection is never carried over.
        this.dlSelected.clear();
      }
    } catch (e) {
      this.ovError = toIpcError(e);
    }
    this.update();
  }

  private toggleDownload(d: DownloadItem): void {
    if (this.dlSelected.has(d.item_id)) this.dlSelected.delete(d.item_id);
    else this.dlSelected.set(d.item_id, d);
    this.createError = null;
    this.app.announce(this.dlSelected.has(d.item_id) ? t('common.selectedName', { name: d.name }) : t('common.deselectedName', { name: d.name }));
    this.update();
  }

  private async trashDownloads(): Promise<void> {
    if (this.creating || !this.dlSelected.size) return;
    if (!(await ensureReady(this.app, 'trash', words().moveToBin))) return;
    this.creating = true;
    this.createError = null;
    this.update();
    try {
      const r = await this.app.backend.downloadsTrashPlan([...this.dlSelected.keys()]);
      this.dlSelected.clear();
      this.app.announce(t('tools.planCreatedNotRun'));
      this.app.openPlan(r.plan_id, 'insights');
    } catch (e) {
      this.createError = toIpcError(e);
    } finally {
      this.creating = false;
      this.update();
    }
  }

  private render(): void {
    const d = this.data;
    const refresh = h('button', { type: 'button', id: 'insights-refresh' }, icon('refresh'), t('tools.measureAgain'));
    refresh.addEventListener('click', () => ((this.data = null), (this.overview = null), this.update(), void this.load()));
    const ov = this.overview;
    // Some location is still waiting for a macOS privacy prompt: say so once, above the cards.
    const waiting = !!ov?.cards.some((c) => c.logical_bytes === null && privacyPending(c.size_note)) || !!d?.items.some((i) => i.logical_bytes === null && privacyPending(i.size_note)) || privacyPending(ov?.downloads?.note);
    replace(
      this.body,
      pageHead({ id: 'insights-title', title: t('tools.spaceInsights'), meta: d ? [t('tools.measuredObserved', { observed_at: formatTime(d.observed_at) }), t('tools.systemObjectsReadOnly')] : [t('tools.systemObjectsReadOnly')], actions: [refresh] }),
      errorLine(this.error),
      waiting ? noticeLine(t('tools.waitingMacosPermissionPrivacy', { text: privacyPendingText() }), 'warn') : null,
      d && !d.supported ? emptyState(t('common.notAvailablePlatform'), notOnThisPlatform(t('tools.spaceInsightsCurrentlyImplemented')), null) : null,
      d && d.supported
        ? h(
            'div',
            { class: 'insights' },
            ov && ov.cards.length
              ? h(
                  'ul',
                  { class: 'insight-cards', 'aria-label': t('tools.totalsCategory') },
                  ...ov.cards.map((c) => {
                    const v = overviewCard(c);
                    return h(
                      'li',
                      { class: `icard${v.pending ? ' pending' : ''}`, 'data-kind': c.kind },
                      h('span', { class: 'icard-title' }, v.title),
                      h('span', { class: 'icard-size num' }, v.size),
                      h('span', { class: 'icard-meta muted' }, [v.count !== null && c.logical_bytes !== null ? t('tools.countObjects', { count: v.count }) : null, v.observed ? formatTime(v.observed) : null].filter(Boolean).join(' · ')),
                      v.note ? h('span', { class: 'icard-meta muted', title: v.noteRaw }, v.note) : null,
                    );
                  }),
                )
              : h(
                  'ul',
                  { class: 'insight-cards', 'aria-label': t('tools.totalsCategory') },
                  ...insightCards(d.items).map((c) =>
                    h('li', { class: 'icard' }, h('span', { class: 'icard-title' }, c.title), h('span', { class: 'icard-size num' }, c.size), h('span', { class: 'icard-meta muted' }, t('tools.countP', { count: c.count, p: c.observed ? ` · ${formatTime(c.observed)}` : '' }))),
                  ),
                ),
            errorLine(this.ovError),
            this.downloadsPanel(),
            h('p', { class: 'muted small' }, t('tools.objectsBelowManagedSystem')),
            ...d.items.map((i) => this.item(i)),
          )
        : !this.error
          ? h('p', { class: 'muted', role: 'status' }, t('tools.measuringKnownLargeSystem'))
          : null,
      this.dlBar(),
    );
  }

  private downloadsPanel(): HTMLElement {
    const ov = this.overview;
    const dl = ov?.downloads ?? null;
    if (!ov) return h('section', { class: 'panel dl-panel', 'aria-labelledby': 'dl-title' }, h('h2', { id: 'dl-title' }, t('tools.oldDownloads')), h('p', { class: 'muted small', role: 'status' }, this.ovError ? '' : t('tools.listingFilesDownloadsFolder')));
    if (!dl) return h('section', { class: 'panel dl-panel', 'aria-labelledby': 'dl-title' }, h('h2', { id: 'dl-title' }, t('tools.oldDownloads')), h('p', { class: 'muted small' }, t('tools.noDownloadsFolderFound')));
    // An unreadable or pending folder: neither the file count nor the size is known (never "0 files").
    const known = dl.total_logical_bytes !== null;
    const pending = !known && privacyPending(dl.note);
    const files = dl.item_count === null ? t('tools.fileCountUnknown') : t('tools.itemCount', { item_count: dl.item_count });
    const summary = known ? t('tools.filesFormatbytesTotal', { files, formatBytes: formatBytes(dl.total_logical_bytes as string) }) : pending ? privacyPendingSize() : t('tools.fileCountSizeUnknown');
    const rows = dl.items.map((x) => {
      const id = `dl-${x.item_id}`;
      const check = h('input', { type: 'checkbox', id: `${id}-check`, 'aria-label': t('common.selectName', { name: x.name }) }) as HTMLInputElement;
      check.checked = this.dlSelected.has(x.item_id);
      check.addEventListener('change', () => this.toggleDownload(x));
      return h(
        'tr',
        { class: `crow${check.checked ? ' selected' : ''}` },
        h('td', { class: 'col-check' }, check),
        h(
          'td',
          { class: 'col-object' },
          h('label', { for: `${id}-check`, class: 'obj-name' }, x.name),
          h('span', { class: 'obj-path path' }, x.path_display),
          x.installer_like ? h('span', { class: 'obj-reason' }, h('span', { class: 'row-tag' }, t('tools.looksLikeInstaller')), t('tools.alsoReviewContentClean')) : null,
        ),
        h('td', { class: 'col-size num', title: exactTitle(x.logical_bytes) }, formatBytes(x.logical_bytes)),
        h('td', { class: 'col-when' }, t('tools.ageDays', { age_days: x.age_days }), h('span', { class: 'why-not' }, formatTime(x.modified_at))),
      );
    });
    return h(
      'section',
      { class: 'panel dl-panel', 'aria-labelledby': 'dl-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'dl-title' }, t('tools.oldDownloadsNotModified', { older_than_days: dl.older_than_days })), h('span', { class: 'muted small num' }, summary)),
      h('p', { class: 'muted small' }, t('tools.reviewOnlyNoFiles', { moveToBin: words().moveToBin })),
      dl.root_display ? h('p', { class: 'path small muted' }, dl.root_display) : null,
      dl.note ? noticeLine(pending ? privacyPendingText() : dl.note, 'warn') : null,
      dl.items.length
        ? h(
            'div',
            { class: 'table-wrap' },
            h(
              'table',
              { class: 'data-table ctable dl-table' },
              h('caption', { class: 'sr-only' }, t('tools.filesDownloadsFolderNot')),
              h('thead', {}, h('tr', {}, h('th', { scope: 'col', class: 'col-check' }, h('span', { class: 'sr-only' }, t('common.select'))), h('th', { scope: 'col' }, t('common.file')), h('th', { scope: 'col', class: 'col-size' }, t('common.logicalSize')), h('th', { scope: 'col', class: 'col-when' }, t('tools.lastModified')))),
              h('tbody', {}, ...rows),
            ),
          )
        : known
          ? h('p', { class: 'small' }, t('tools.noFilesUnmodifiedMore', { older_than_days: dl.older_than_days }))
          : null,
      dl.item_count !== null && dl.items.length < dl.item_count ? h('p', { class: 'muted small' }, t('tools.showingLargestLengthItem', { length: dl.items.length, item_count: dl.item_count })) : null,
      errorLine(this.createError),
    );
  }

  private dlBar(): HTMLElement {
    const items = [...this.dlSelected.values()];
    return selectionBar({
      count: items.length,
      sum: sumSizes(items.map((i) => ({ bytes: i.logical_bytes }))),
      onClear: () => {
        this.dlSelected.clear();
        this.app.announce(t('common.selectionCleared'));
        this.update();
      },
      action: { id: 'dl-bar-go', text: this.creating ? t('tools.creatingPlan') : `${words().moveToBin}…`, disabled: this.creating, onClick: () => void this.trashDownloads() },
    });
  }

  private item(i: Insight): HTMLElement {
    const [title, explain] = INSIGHT_TITLE[i.kind] ?? [i.title, i.explanation];
    const pending = i.logical_bytes === null && privacyPending(i.size_note);
    const size = pending ? privacyPendingSize() : i.logical_bytes === null ? t('common.sizeUnknown') : formatBytes(i.logical_bytes);
    const sized = pending ? null : sizeNoteText(i.size_note);
    const note = pending ? privacyPendingText() : (sized?.text ?? null);
    return h(
      'section',
      { class: 'panel insight', 'aria-labelledby': `ins-${i.insight_id}` },
      h(
        'div',
        { class: 'insight-head' },
        h('div', {}, h('h2', { id: `ins-${i.insight_id}` }, title), i.path ? h('p', { class: 'path muted small' }, i.path.display) : null),
        h('p', { class: 'insight-size num' }, size, i.count !== null ? h('small', {}, t('tools.countItems', { count: i.count })) : null, note ? h('small', { title: sized?.raw ?? null }, note) : null),
      ),
      h('p', {}, explain),
      ...i.guidance.map((g, n) =>
        h(
          'div',
          { class: 'guidance' },
          h('p', { class: 'small', title: guidanceText(i.kind, n, g).raw }, h('b', {}, guidanceText(i.kind, n, g).tool), g.requires_admin ? t('tools.needsAdmin', { admin: words().admin }) : '', t('tools.colon'), guidanceText(i.kind, n, g).impact),
          copyField(t('tools.officialUsageCleanmanagerWont'), guidanceText(i.kind, n, g).steps),
        ),
      ),
    );
  }
}

/** One sustained-CPU alert in words (STS-05). */
export function alertText(a: CpuAlert): string {
  return t('tools.sustainedHighCpuName', { name: a.name, pid: a.pid, window_seconds: a.window_seconds, threshold_percent: a.threshold_percent, toFixed: a.min_cpu_percent.toFixed(0) });
}

// ---------------------------------------------------------------------------------------------
// 状态 › 系统状态

function metric(s: StatusSnapshot, name: string): StatusMetric | undefined {
  return s.metrics?.find((m) => m.name === name);
}

function valueText(m: StatusMetric | undefined): string {
  if (!m || m.value === null || m.availability !== 'available') return t('common.unavailable');
  switch (m.unit) {
    case 'percent':
      return `${m.value.toFixed(m.value < 10 ? 1 : 0)}%`;
    case 'bytes':
      return formatBytes(BigInt(Math.round(m.value)));
    case 'bytes_per_second':
      return `${formatBytes(BigInt(Math.round(m.value)))}/s`;
    case 'celsius':
      return `${m.value.toFixed(0)} °C`;
    case 'rpm':
      return t('tools.valueRpm', { value: Math.round(m.value) });
    default:
      return String(m.value);
  }
}

/** One tile of the metrics panel: a value, or "不可用" with the reason (never 0). */
export interface MetricTile {
  label: string;
  value: string;
  note: string;
  available: boolean;
  /** The host's own reason when it has no words yet (shown as a tooltip, never as the note). */
  raw?: string | null;
}

/** Words of the status page that are not data (one table for the language switch). */
export const STATUS_TEXT = {
  get noReading() { return t('tools.noReading'); },
} as const;

/**
 * STS-01 tiles. Windows: the eight shared tiles. macOS adds fans (unavailable with the reason: SMC is private) and the
 * process count with zombies, and the battery's cycle count; its reasons are put in words.
 */
export function metricTiles(s: StatusSnapshot, mac = platform() === 'macos'): MetricTile[] {
  const tile = (label: string, name: string, extra?: string | null): MetricTile => {
    const m = metric(s, name);
    const available = !!m && m.availability === 'available';
    const reason = m && !available ? metricReason(m.reason ?? t('common.unavailable')) : null;
    const raw = reason !== null && isRawHostText(reason) ? reason : null;
    return { label, value: valueText(m), note: reason === null ? (extra ?? '') : raw ? STATUS_TEXT.noReading : reason, available, raw };
  };
  const memTotal = metric(s, 'memory.total_bytes');
  const diskTotal = metric(s, 'disk.system.total_bytes');
  const cycles = metric(s, 'battery.cycle_count');
  const tiles = [
    tile(t('common.cpu'), 'cpu.total_percent', metric(s, 'cpu.logical_count') ? t('tools.logicalProcessors', { value: metric(s, 'cpu.logical_count')!.value }) : null),
    tile(t('common.memoryUsed'), 'memory.used_percent', memTotal?.value ? t('tools.memtotalTotalValuetextAvailable', { memTotal: valueText(memTotal), valueText: valueText(metric(s, 'memory.available_bytes')) }) : null),
    tile(t('common.systemDiskFree'), 'disk.system.free_bytes', diskTotal?.value ? t('tools.valuetextDisktotalTotal', { valueText: valueText(metric(s, 'disk.system.free_percent')), diskTotal: valueText(diskTotal) }) : null),
    tile(t('tools.diskBusy'), 'disk.busy_percent', t('tools.readValuetextWriteValuetext2', { valueText: valueText(metric(s, 'disk.read_bytes_per_sec')), valueText2: valueText(metric(s, 'disk.write_bytes_per_sec')) })),
    tile(t('tools.highestTemperature'), 'thermal.max_zone_celsius'),
    tile(t('tools.gpuTemperature'), 'gpu.temperature_celsius'),
    tile(t('common.batteryLevel'), 'battery.charge_percent', metric(s, 'battery.on_ac_power')?.value ? t('tools.powerAdapter') : t('common.battery')),
    tile(t('tools.batteryHealth'), 'battery.health_percent', mac && cycles?.availability === 'available' && cycles.value !== null ? t('tools.designCapacityValue', { value: cycles.value }) : t('tools.designCapacity')),
  ];
  if (!mac) return tiles;
  const zombies = metric(s, 'process.zombie_count');
  const z = zombies?.availability === 'available' ? zombies.value : null;
  tiles.push(
    tile(t('tools.fans'), 'fan.rpm'),
    tile(t('common.processes'), 'process.count', z === null ? (zombies?.reason ? metricReason(zombies.reason) : null) : z > 0 ? t('tools.zExitedWaitingParent', { z }) : t('tools.noZombieProcesses')),
  );
  return tiles;
}

const DIMENSION_LABEL: Record<string, string> = { get cpu() { return t('tools.dimensionLabel.cpu'); }, get memory() { return t('tools.dimensionLabel.memory'); }, get disk() { return t('tools.dimensionLabel.disk'); }, get thermal() { return t('tools.dimensionLabel.thermal'); }, get io() { return t('tools.dimensionLabel.io'); } };

const DIAGNOSIS_LABEL: Record<string, string> = {
  get low_free_space() { return t('tools.diagnosisLabel.low_free_space'); },
  get disk_health_warning() { return t('tools.diagnosisLabel.disk_health_warning'); },
  get high_cpu() { return t('tools.diagnosisLabel.high_cpu'); },
  get memory_pressure() { return t('tools.diagnosisLabel.memory_pressure'); },
  get battery_low() { return t('tools.diagnosisLabel.battery_low'); },
  get battery_worn() { return t('tools.diagnosisLabel.battery_worn'); },
  get high_temperature() { return t('tools.diagnosisLabel.high_temperature'); },
  get heavy_io() { return t('tools.diagnosisLabel.heavy_io'); },
};

/**
 * The status page's one diagnosis line (STS-03): `normal` when no metric crosses a threshold (cm-core status.rs), one
 * of the problem codes, or a code this build does not know (its host message only as the tooltip).
 */
export function diagnosisLine(d: { code: string; message: string }): { normal: boolean; text: string; raw: string | null } {
  if (d.code === 'normal') return { normal: true, text: t('tools.diagnosisNormal'), raw: null };
  const known = DIAGNOSIS_LABEL[d.code];
  return { normal: false, text: known ?? unknownCodeText(d.code), raw: known ? null : d.message };
}

const NET_CLASS_WORD: Record<string, string> = { get physical() { return t('tools.netClassWord.physical'); }, get tunnel() { return t('tools.netClassWord.tunnel'); }, get virtual() { return t('tools.netClassWord.virtual'); }, get unknown() { return t('tools.netClassWord.unknown'); } };

/** What the network total covers, in words (STS-07). */
export function networkScope(n: NetworkTotals): string {
  if (n.classification === 'unavailable') return t('tools.adapterTypesCantTold');
  if (n.may_double_count) return t('tools.totalIncludesUnclassifiedAdapters');
  const loop = n.loopback_excluded ? t('tools.excludingLoopback') : '';
  return t('tools.totalCountsPhysicalAdapters', { loop });
}

export class StatusPage implements PageLike {
  el: HTMLElement;
  private body: HTMLElement;
  private snap: StatusSnapshot | null = null;
  /** STS-05 alerts raised while this page sampled (newest first, this session only). */
  private alerts: CpuAlert[] = [];
  private error: IpcError | null = null;
  private gate = new RequestGate();
  private timer: number | null = null;
  private paused = false;
  /** STS-07: panel visibility and order and the refresh interval, kept across sessions. */
  private prefs: StatusPrefs = loadPrefs();
  private prefsOpen = false;
  private prefsNote: string | null = null;

  constructor(private app: App) {
    this.body = h('div', {});
    this.el = h('section', { class: 'page page-status', 'aria-labelledby': 'status-title' }, this.body);
  }

  activate(): void {
    void this.sample();
    this.schedule();
  }

  update(): void {
    if (this.app.store.state.page !== 'status') this.stop();
    keepFocus(this.el, () => this.render());
  }

  private schedule(): void {
    this.stop();
    if (!this.paused) this.timer = window.setInterval(() => void this.sample(), this.prefs.refreshSeconds * 1000);
  }

  private stop(): void {
    if (this.timer !== null) window.clearInterval(this.timer);
    this.timer = null;
  }

  private async sample(): Promise<void> {
    if (this.app.store.state.page !== 'status') return this.stop();
    try {
      // Sampling runs at a fixed rate; a slow sample that returns after a newer one is dropped.
      const r = await this.gate.latest(this.app.backend.statusSnapshot());
      if (r.stale) return;
      this.snap = r.value;
      if (r.value.alerts?.length) {
        this.alerts = [...r.value.alerts, ...this.alerts].slice(0, 5);
        this.app.announce(r.value.alerts.map((x) => alertText(x)).join(t('common.clauseSep')));
      }
      this.error = null;
    } catch (e) {
      this.error = toIpcError(e);
    }
    if (this.app.store.state.page === 'status') this.update();
  }

  private setPrefs(next: StatusPrefs, said: string): void {
    const refreshChanged = next.refreshSeconds !== this.prefs.refreshSeconds;
    // The process sort belongs to the process page: keep whatever it stored meanwhile.
    this.prefs = sanitize({ ...next, processSort: loadPrefs().processSort });
    this.prefsNote = savePrefs(this.prefs) ? null : t('tools.computerDoesntAllowSaving');
    if (refreshChanged) this.schedule();
    this.app.announce(said);
    this.update();
  }

  private render(): void {
    const s = this.snap;
    const pause = h('button', { type: 'button', id: 'status-pause', 'aria-pressed': this.paused ? 'true' : 'false' }, this.paused ? t('tools.resumeRefresh') : t('tools.pauseRefresh'));
    pause.addEventListener('click', () => {
      this.paused = !this.paused;
      this.schedule();
      this.update();
    });
    const customize = h('button', { type: 'button', id: 'status-prefs-toggle', 'aria-expanded': this.prefsOpen ? 'true' : 'false', 'aria-controls': 'status-prefs' }, icon('settings'), t('tools.displaySettings'));
    customize.addEventListener('click', () => {
      this.prefsOpen = !this.prefsOpen;
      this.update();
    });
    if (s && !s.supported) {
      replace(this.body, pageHead({ id: 'status-title', title: t('tools.systemStatus') }), emptyState(t('common.notAvailablePlatform'), notOnThisPlatform(t('tools.statusSamplingCurrentlyImplemented')), null));
      return;
    }
    replace(
      this.body,
      pageHead({
        id: 'status-title',
        title: t('tools.systemStatus'),
        meta: s?.sampled_at ? [t('common.sampledSampled', { sampled_at: formatTime(s.sampled_at) }), s.stale ? t('tools.dataStale') : this.paused ? t('tools.paused') : t('tools.refreshesEveryRefreshseconds', { refreshSeconds: this.prefs.refreshSeconds })] : [],
        actions: [customize, pause],
      }),
      this.prefsOpen ? this.prefsPanel() : null,
      errorLine(this.error),
      ...this.alerts.map((x, i) =>
        h(
          'div',
          { class: 'notice warn alert-line', role: 'status' },
          h('span', {}, alertText(x)),
          h('span', { class: 'alert-actions' }, h('button', { type: 'button', class: 'quiet', onclick: () => (this.app.store.set({ processFocus: x.pid }), this.app.navigate('processes')) }, t('tools.viewProcesses')), h('button', { type: 'button', class: 'quiet', onclick: () => ((this.alerts = this.alerts.filter((_, j) => j !== i)), this.update()) }, t('common.got'))),
        ),
      ),
      s ? this.view(s) : !this.error ? h('p', { class: 'muted', role: 'status' }, t('common.sampling')) : null,
    );
  }

  private prefsPanel(): HTMLElement {
    const p = this.prefs;
    const refresh = h(
      'fieldset',
      { class: 'seg' },
      h('legend', { class: 'sr-only' }, t('tools.refreshInterval')),
      ...REFRESH_SECONDS.map((sec) => {
        const input = h('input', { type: 'radio', name: 'status-refresh', id: `status-refresh-${sec}`, value: String(sec) }) as HTMLInputElement;
        input.checked = p.refreshSeconds === sec;
        input.addEventListener('change', () => this.setPrefs({ ...p, refreshSeconds: sec }, t('tools.refreshingEverySec', { sec })));
        return h('label', { for: `status-refresh-${sec}` }, input, h('span', {}, t('tools.sec', { sec })));
      }),
    );
    const shownCount = p.order.length - p.hidden.length;
    const rows = p.order.map((panel, i) => {
      const shown = !p.hidden.includes(panel);
      const box = h('input', { type: 'checkbox', id: `status-show-${panel}` }) as HTMLInputElement;
      box.checked = shown;
      // At least one panel stays visible, so the page never turns blank.
      box.disabled = shown && shownCount === 1;
      box.addEventListener('change', () => this.setPrefs(setShown(p, panel, box.checked), box.checked ? t('tools.panelShown', { panel: PANEL_LABEL[panel] }) : t('tools.panelHidden', { panel: PANEL_LABEL[panel] })));
      const move = (delta: -1 | 1) => {
        const id = `status-${delta < 0 ? 'up' : 'down'}-${panel}`;
        const edge = delta < 0 ? i === 0 : i === p.order.length - 1;
        const b = h('button', { type: 'button', class: 'icon-btn small-btn', id, disabled: edge, 'aria-label': delta < 0 ? t('tools.panelMoveUp', { panel: PANEL_LABEL[panel] }) : t('tools.panelMoveDown', { panel: PANEL_LABEL[panel] }), title: delta < 0 ? t('tools.moveUp2') : t('tools.moveDown2') }, icon(delta < 0 ? 'up' : 'down'));
        b.addEventListener('click', () => {
          const next = movePanel(p, panel, delta);
          this.setPrefs(next, t('tools.pMovedPositionP2', { p: PANEL_LABEL[panel], p2: next.order.indexOf(panel) + 1 }));
          // At the new edge this button is disabled; keep focus on the row's other move button.
          const now = document.getElementById(id) as HTMLButtonElement | null;
          if (now?.disabled) document.getElementById(`status-${delta < 0 ? 'down' : 'up'}-${panel}`)?.focus();
        });
        return b;
      };
      return h(
        'li',
        { 'data-shown': shown ? 'yes' : 'no' },
        h('label', { class: 'pref-panel', for: `status-show-${panel}` }, box, h('span', {}, PANEL_LABEL[panel])),
        h('span', { class: 'pref-move' }, move(-1), move(1)),
      );
    });
    const reset = h('button', { type: 'button', class: 'quiet', id: 'status-prefs-reset' }, t('tools.resetDefaults'));
    reset.addEventListener('click', () => this.setPrefs({ ...DEFAULT_PREFS }, t('tools.displaySettingsResetDefaults')));
    return h(
      'section',
      { class: 'panel status-prefs', id: 'status-prefs', 'aria-labelledby': 'status-prefs-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'status-prefs-title' }, t('tools.displaySettings')), reset),
      h('div', { class: 'pref-grid' }, h('div', {}, h('h3', {}, t('tools.panelsOrder')), h('ol', { class: 'pref-panels' }, ...rows)), h('div', {}, h('h3', {}, t('tools.refreshInterval')), refresh, h('p', { class: 'muted small' }, t('tools.processListsSortOrder')))),
      this.prefsNote ? noticeLine(this.prefsNote, 'warn') : h('p', { class: 'muted small' }, t('tools.settingsSavedOnlyApp')),
    );
  }

  private view(s: StatusSnapshot): HTMLElement {
    const build: Record<StatusPanel, () => HTMLElement> = { health: () => this.healthPanel(s), metrics: () => this.metricsPanel(s), network: () => this.networkPanel(s) };
    return h('div', { class: 'status-layout' }, ...layoutRows(this.prefs).map((r) => h('div', { class: 'status-row', 'data-shape': r.shape }, ...r.panels.map((x) => build[x]()))));
  }

  private healthPanel(s: StatusSnapshot): HTMLElement {
    const hl = s.health;
    const diag = s.diagnosis;
    return h(
      'section',
      { class: 'panel health', 'aria-labelledby': 'health-title' },
      h('h2', { id: 'health-title' }, t('tools.healthScore')),
      h('p', { class: 'health-score num' }, hl?.score === null || hl?.score === undefined ? '—' : String(hl.score), h('small', {}, '/ 100')),
      h('p', { class: 'muted small' }, t('tools.reflectsOnlyCurrentLoad')),
      hl
        ? h(
            'ul',
            { class: 'dims' },
            ...hl.dimensions.map((d) => {
              const bar = h('span', { class: 'dim-bar' }, h('span', { class: 'dim-fill' }));
              (bar.firstChild as HTMLElement).style.width = `${d.score ?? 0}%`;
              return h(
                'li',
                { 'data-low': d.score !== null && d.score < 60 ? 'yes' : 'no' },
                h('span', { class: 'dim-label' }, DIMENSION_LABEL[d.dimension] ?? d.dimension, h('small', { class: 'muted' }, t('tools.weightP', { p: hl.weights[d.dimension] ?? '—' }))),
                bar,
                h('span', { class: 'dim-score num' }, d.score === null ? t('tools.noData') : String(d.score)),
              );
            }),
          )
        : null,
      diag
        ? diagnosisLine(diag).normal
          ? // NEW-13 (WFA-91): the healthy state is a quiet line of its own, never an "unrecognized issue".
            h('p', { class: 'diag-line diag-normal small' }, h('span', { class: 'state-word', 'data-tone': 'good' }, h('span', { class: 'sw-mark', 'aria-hidden': 'true' }), diagnosisLine(diag).text))
          : h(
              'div',
              { class: 'notice warn diag-line' },
              h('span', { title: diagnosisLine(diag).raw }, diagnosisLine(diag).text),
              diag.link === 'analyze' ? h('button', { type: 'button', class: 'quiet', onclick: () => this.app.navigate('overview') }, t('common.analyzeDisk')) : null,
            )
        : null,
    );
  }

  private metricsPanel(s: StatusSnapshot): HTMLElement {
    return h(
      'section',
      { class: 'panel metrics', 'aria-labelledby': 'metrics-title' },
      h('h2', { id: 'metrics-title' }, t('tools.metrics')),
      h(
        'ul',
        { class: 'mtiles' },
        ...metricTiles(s).map((t) =>
          h('li', { class: 'mtile', 'data-available': t.available ? 'yes' : 'no' }, h('span', { class: 'mtile-label' }, t.label), h('span', { class: 'mtile-value num' }, t.value), h('span', { class: 'mtile-note muted', title: t.raw ?? null }, t.note)),
        ),
      ),
      h('p', { class: 'muted small' }, t('tools.unavailableMetricsStateReason')),
    );
  }

  private networkPanel(s: StatusSnapshot): HTMLElement {
    const n = s.network;
    const head = h('h2', { id: 'network-title' }, t('tools.network'));
    if (!n) {
      const m = metric(s, 'network.received_bytes_per_sec');
      return h('section', { class: 'panel network', 'aria-labelledby': 'network-title' }, head, h('p', { class: 'muted small' }, m?.reason ?? t('tools.noNetworkDataSample')));
    }
    const rate = (v: number) => `${formatBytes(BigInt(Math.round(v)))}/s`;
    const row = (i: { name: string; class: string; received_bytes_per_sec: number; sent_bytes_per_sec: number }) =>
      h('tr', {}, h('td', { class: 'col-object' }, h('span', { class: 'obj-name' }, i.name)), h('td', { class: 'small' }, NET_CLASS_WORD[i.class] ?? i.class), h('td', { class: 'col-num-s num' }, rate(i.received_bytes_per_sec)), h('td', { class: 'col-num-s num' }, rate(i.sent_bytes_per_sec)));
    const table = (caption: string, items: typeof n.counted) =>
      h(
        'div',
        { class: 'table-wrap' },
        h(
          'table',
          { class: 'data-table net-table' },
          h('caption', { class: 'sr-only' }, caption),
          h('thead', {}, h('tr', {}, h('th', { scope: 'col' }, t('tools.adapter')), h('th', { scope: 'col' }, t('common.type')), h('th', { scope: 'col', class: 'col-num-s' }, t('tools.download')), h('th', { scope: 'col', class: 'col-num-s' }, t('tools.upload')))),
          h('tbody', {}, ...items.map(row)),
        ),
      );
    const active = n.separate.filter((i) => i.received_bytes_per_sec > 0 || i.sent_bytes_per_sec > 0);
    const idle = n.separate.filter((i) => !active.includes(i));
    return h(
      'section',
      { class: 'panel network', 'aria-labelledby': 'network-title' },
      head,
      h(
        'ul',
        { class: 'mtiles' },
        h('li', { class: 'mtile' }, h('span', { class: 'mtile-label' }, n.may_double_count ? t('tools.totalDownloadMayDouble') : t('tools.totalDownload')), h('span', { class: 'mtile-value num' }, rate(n.received_bytes_per_sec))),
        h('li', { class: 'mtile' }, h('span', { class: 'mtile-label' }, n.may_double_count ? t('tools.totalUploadMayDouble') : t('tools.totalUpload')), h('span', { class: 'mtile-value num' }, rate(n.sent_bytes_per_sec))),
      ),
      n.may_double_count ? noticeLine(networkScope(n), 'warn') : h('p', { class: 'muted small' }, networkScope(n)),
      n.counted.length ? table(t('tools.adaptersTotal'), n.counted) : h('p', { class: 'muted small' }, t('tools.noActivePhysicalAdapters')),
      n.separate.length ? h('h3', { class: 'net-sub' }, t('tools.listedSeparatelyNotTotal')) : null,
      active.length ? table(t('tools.tunnelsVirtualAdapters'), active) : null,
      idle.length ? h('p', { class: 'muted small' }, t('tools.lengthNoTrafficNow', { length: idle.length, join: idle.map((i) => t('common.nameP', { name: i.name, p: NET_CLASS_WORD[i.class] ?? i.class })).join(t('common.listSep')) })) : null,
    );
  }
}

// ---------------------------------------------------------------------------------------------
// Capability pages

export class CapabilityPageView implements PageLike {
  el: HTMLElement;
  constructor(
    id: string,
    private build: () => HTMLElement,
  ) {
    this.el = h('section', { class: `page page-${id}`, 'aria-labelledby': `${id}-title` });
  }
  update(): void {
    replace(this.el, this.build());
  }
}

/** Pages that describe a capability instead of a tool. 清理 › 回收站 is a tool now (`views/recycleBin.ts`, WFA-65). */
export function capabilityPages(_app: App): Record<'agent', CapabilityPageView> {
  return {
    agent: new CapabilityPageView('agent', () =>
      capabilityPage({
        id: 'agent',
        title: 'Agent',
        gate: 'cli_only',
        spec: t('tools.cli0112Agent'),
        gateName: 'G1',
        does: [t('tools.letsScriptsAiAgents'), t('tools.planIdsDigestsWork'), t('tools.automatedCallsCantApprove')],
        why: t('tools.agentInterfaceCommandLines'),
        whyMore: [t('tools.desktopNoSeparateAgent')],
        cli: [
          { label: t('tools.showCapabilities'), command: 'cleanmanager capabilities --json' },
          { label: t('tools.previewCleanupCandidatesRead'), command: 'cleanmanager clean --dry-run --json' },
          { label: t('tools.findBuildArtifactsRead'), command: 'cleanmanager purge --dry-run --json' },
          { label: t('tools.reviewPlanPerson'), command: 'cleanmanager plan review PLAN_ID' },
        ],
      }),
    ),
  };
}
