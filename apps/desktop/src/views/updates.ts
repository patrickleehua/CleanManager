// 软件 › 更新 (UPD-01..04). Checks run only when you ask and only while checks are on (off = no
// request of any kind; the last results keep their original time). Each result names its channel;
// nothing reads as "up to date" without a channel that answered. Verifiable package-manager updates
// go into one plan reviewed like every other; Store, self-updating and unverifiable apps are routed
// to where they update. Running apps are skipped at execution with "close it first".

import type { App } from '../main';
import { groupDigits } from '../lib/bytes';
import { h, icon, keepFocus, replace } from '../lib/dom';
import { formatTime } from '../lib/format';
import { RequestGate } from '../lib/gate';
import type { UpdateCheck, UpdateList, UpdatePlanCreated, UpdateState } from '../lib/suiteTypes';
import { CHANNEL_WORD, ROUTE_WORD, UPDATE_STATE, updateReason } from '../lib/suiteWords';
import type { IpcError } from '../lib/types';
import { LongProgress, ensureReady, noticeLine, selectionBar, toIpcError } from './kit';
import { copyField, emptyState, errorLine, pageHead, pill } from './parts';
import { notOnThisPlatform, words } from '../lib/platform';
import { learnMore } from './ui';
import { t } from '../i18n';

type Filter = 'all' | 'update_available' | 'cannot_check' | 'up_to_date';

const FILTERS: { key: Filter; label: string }[] = [
  { key: 'all', get label() { return t('updates.filters.all.label'); } },
  { key: 'update_available', get label() { return t('updates.filters.update_available.label'); } },
  { key: 'cannot_check', get label() { return t('updates.filters.cannot_check.label'); } },
  { key: 'up_to_date', get label() { return t('updates.filters.up_to_date.label'); } },
];

/** UPD-01: "up to date" only when a channel actually answered; otherwise it cannot be checked. */
export function shownState(c: Pick<UpdateCheck, 'state' | 'channel'>): UpdateState {
  return c.state === 'up_to_date' && !c.channel ? 'cannot_check' : c.state;
}

/** Only package-manager updates with a channel can be put into a plan from here. */
export function plannable(c: UpdateCheck): boolean {
  return shownState(c) === 'update_available' && c.route === 'package_manager' && !!c.channel;
}

export class UpdatesPage {
  el: HTMLElement;
  private body: HTMLElement;
  private data: UpdateList | null = null;
  private error: IpcError | null = null;
  private checking = false;
  private filter: Filter = 'all';
  private selected = new Set<string>();
  private creating = false;
  private createError: IpcError | null = null;
  private created: UpdatePlanCreated | null = null;
  private savingSetting = false;
  private gate = new RequestGate();
  private progress = new LongProgress(() => t('updates.checkingUpdates'));

  constructor(private app: App) {
    this.body = h('div', { class: 'updates-body' });
    this.el = h('section', { class: 'page page-updates', 'aria-labelledby': 'updates-title' }, this.body);
  }

  activate(): void {
    if (!this.data) void this.load(false);
  }

  update(): void {
    keepFocus(this.el, () => this.render());
  }

  private async load(check: boolean): Promise<void> {
    if (check) {
      if (this.checking) return;
      this.checking = true;
      this.progress.start(words().updatesProgress);
      this.app.announce(t('updates.startedCheckingUpdates'));
    }
    this.error = null;
    this.update();
    try {
      const r = await this.gate.latest(this.app.backend.updatesList(check));
      if (r.stale) return;
      this.data = r.value;
      // A new result list never inherits a selection made on older rows.
      if (check) {
        this.selected.clear();
        this.created = null;
        this.app.announce(t('updates.checkCompleteP', { p: r.value.counts.update_available ?? 0 }));
      }
    } catch (e) {
      this.error = toIpcError(e);
    } finally {
      if (check) {
        this.checking = false;
        this.progress.stop();
      }
      this.update();
    }
  }

  private async setChecks(enabled: boolean): Promise<void> {
    this.savingSetting = true;
    this.update();
    try {
      await this.app.backend.updatesSetChecks(enabled);
      this.app.announce(enabled ? t('updates.updateChecksAllowed') : t('updates.updateChecksTurnedOff'));
      await this.load(false);
    } catch (e) {
      this.error = toIpcError(e);
    } finally {
      this.savingSetting = false;
      this.update();
    }
  }

  private async createPlan(): Promise<void> {
    if (this.creating || !this.selected.size) return;
    if (!(await ensureReady(this.app, 'update', t('updates.updateApps')))) return;
    this.creating = true;
    this.createError = null;
    this.update();
    try {
      const r = await this.app.backend.updatesPlan([...this.selected]);
      this.selected.clear();
      if (r.plan_id && !r.routed.length) {
        this.app.announce(t('updates.updatePlanCreatedNothing'));
        this.app.openPlan(r.plan_id, 'updates');
        return;
      }
      this.created = r;
      this.app.announce(r.plan_id ? t('updates.createdUpdatePlanPlanned', { planned: r.planned, length: r.routed.length }) : t('updates.noneSelectedAppsUpdated'));
    } catch (e) {
      this.createError = toIpcError(e);
    } finally {
      this.creating = false;
      this.update();
    }
  }

  // ---- rendering ---------------------------------------------------------------------------

  private render(): void {
    const d = this.data;
    if (d && !d.supported) {
      replace(this.body, pageHead({ id: 'updates-title', title: t('updates.updates') }), emptyState(t('common.notAvailablePlatformYet'), notOnThisPlatform(t('updates.updateChecksOnlyImplemented')), null));
      return;
    }
    const check = h('button', { type: 'button', class: 'primary', id: 'updates-check', disabled: this.checking || !d?.checks_enabled }, icon('refresh'), this.checking ? t('common.checking') : t('updates.checkUpdates'));
    check.addEventListener('click', () => void this.load(true));
    replace(
      this.body,
      pageHead({
        id: 'updates-title',
        title: t('updates.updates'),
        meta: d ? [d.checked_at ? t('updates.lastCheckedChecked', { checked_at: formatTime(d.checked_at) }) : t('updates.notCheckedYet'), d.checks_enabled ? t('updates.checksOnlyWhenClick') : t('updates.checksOff'), t('updates.officialChannelsOnly')] : [],
        actions: [check],
      }),
      d && !d.checks_enabled ? noticeLine(t('updates.updateChecksOffNo'), 'warn') : null,
      this.progress.el,
      errorLine(this.error),
      d ? h('div', { class: 'work-layout' }, h('div', { class: 'work-main' }, this.routedPanel(), this.resultsPanel(d)), this.aside(d)) : !this.error ? h('p', { class: 'muted', role: 'status' }, t('updates.readingLastResults')) : null,
      d ? this.bar() : null,
    );
  }

  private resultsPanel(d: UpdateList): HTMLElement {
    const seg = h(
      'fieldset',
      { class: 'seg' },
      h('legend', { class: 'sr-only' }, t('common.filterResults')),
      ...FILTERS.map((f) => {
        const input = h('input', { type: 'radio', name: 'updates-filter', id: `updates-filter-${f.key}`, value: f.key }) as HTMLInputElement;
        input.checked = this.filter === f.key;
        input.addEventListener('change', () => {
          this.filter = f.key;
          this.update();
        });
        const n = f.key === 'all' ? d.items.length : d.items.filter((i) => shownState(i) === f.key).length;
        return h('label', { for: `updates-filter-${f.key}` }, input, h('span', {}, `${f.label} ${n}`));
      }),
    );
    const items = d.items.filter((i) => this.filter === 'all' || shownState(i) === this.filter);
    const order: UpdateState[] = ['update_available', 'cannot_check', 'not_checked', 'up_to_date'];
    items.sort((a, b) => order.indexOf(shownState(a)) - order.indexOf(shownState(b)) || a.name.localeCompare(b.name));
    return h(
      'section',
      { class: 'panel', 'aria-labelledby': 'updates-list-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'updates-list-title' }, t('updates.checkResults')), seg),
      items.length ? this.table(items) : h('p', { class: 'muted small' }, d.items.length ? t('common.noAppsMatch') : t('updates.noResultsYetClick')),
    );
  }

  private table(items: UpdateCheck[]): HTMLElement {
    const rows = items.map((c) => {
      const id = `upd-${c.app_instance_id}`;
      const state = shownState(c);
      const st = UPDATE_STATE[state];
      const can = plannable(c);
      const check = h('input', { type: 'checkbox', id: `${id}-check`, disabled: !can, 'aria-label': t('common.selectName', { name: c.name }) }) as HTMLInputElement;
      check.checked = this.selected.has(c.app_instance_id);
      check.addEventListener('change', () => {
        if (check.checked) this.selected.add(c.app_instance_id);
        else this.selected.delete(c.app_instance_id);
        this.createError = null;
        this.update();
      });
      const versions =
        state === 'update_available'
          ? `${c.current_version ?? t('updates.currentVersionUnknown')} → ${c.available_version ?? '?'}`
          : c.current_version
            ? t('updates.currentCurrentVersion', { current_version: c.current_version })
            : t('updates.currentVersionUnknown');
      const channel = c.channel ? `${CHANNEL_WORD[c.channel]}${c.channel_id ? t('updates.channelId', { channel_id: c.channel_id }) : ''}` : t('updates.noUpdateChannel');
      let go: HTMLElement | null = null;
      if (c.route === 'open_store') go = h('button', { type: 'button', class: 'quiet', onclick: () => void this.app.backend.openSystemPage('store_updates') }, icon('external'), words().openStoreUpdates);
      else if (!can && state === 'update_available') go = h('span', { class: 'small muted' }, ROUTE_WORD[c.route]);
      else if (c.route === 'open_app') go = h('span', { class: 'small muted' }, ROUTE_WORD.open_app);
      return h(
        'tr',
        { class: `crow${check.checked ? ' selected' : ''}${can ? '' : ' locked'}` },
        h('td', { class: 'col-check' }, check),
        h(
          'td',
          { class: 'col-object' },
          h('label', { for: `${id}-check`, class: 'obj-name' }, c.name),
          h('span', { class: 'obj-path num' }, versions),
          c.reason ? h('span', { class: state === 'cannot_check' ? 'why-not' : 'obj-reason muted' }, updateReason(c.reason)) : null,
          c.official_url && !can && !c.official_url.startsWith('macappstore:') ? h('span', { class: 'obj-reason muted path' }, t('updates.officialPageOfficialUrl', { official_url: c.official_url })) : null,
        ),
        h('td', { class: 'col-source' }, h('span', { class: 'small' }, channel), c.checked_at ? h('span', { class: 'small muted block' }, formatTime(c.checked_at)) : null),
        h('td', { class: 'col-status' }, pill(st.label, st.tone), go),
      );
    });
    return h(
      'div',
      { class: 'table-wrap' },
      h(
        'table',
        { class: 'data-table updates-table' },
        h('caption', { class: 'sr-only' }, t('updates.appsVersionsUpdateChannels')),
        h('thead', {}, h('tr', {}, h('th', { scope: 'col', class: 'col-check' }, h('span', { class: 'sr-only' }, t('common.select'))), h('th', { scope: 'col' }, t('updates.appVersion')), h('th', { scope: 'col', class: 'col-source' }, t('updates.channel')), h('th', { scope: 'col', class: 'col-status' }, t('common.result')))),
        h('tbody', {}, ...rows),
      ),
    );
  }

  private routedPanel(): HTMLElement | null {
    const c = this.created;
    if (!c) return null;
    const review = c.plan_id ? h('button', { type: 'button', class: 'primary', id: 'updates-review', onclick: () => this.app.openPlan(c.plan_id!, 'updates') }, t('updates.reviewUpdatePlanPlanned', { planned: c.planned })) : null;
    return h(
      'section',
      { class: 'panel routed-panel', 'aria-labelledby': 'updates-routed' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'updates-routed' }, c.plan_id ? t('updates.planCreatedAppsBelow') : t('updates.selectedAppsCantUpdated')), review),
      h(
        'ul',
        { class: 'run-list' },
        ...c.routed.map((r) =>
          h(
            'li',
            { class: 'run-row' },
            h('span', { class: 'run-main' }, h('b', {}, r.name), h('span', { class: 'muted small' }, updateReason(r.reason))),
            h('span', { class: 'small' }, ROUTE_WORD[r.route]),
            r.route === 'open_store'
              ? h('button', { type: 'button', class: 'quiet', onclick: () => void this.app.backend.openSystemPage('store_updates') }, icon('external'), words().openStore)
              : r.url
                ? copyField(t('updates.officialPage'), r.url)
                : h('span', {}),
          ),
        ),
      ),
    );
  }

  private aside(d: UpdateList): HTMLElement {
    const toggle = h('input', { type: 'checkbox', id: 'updates-checks', role: 'switch', disabled: this.savingSetting }) as HTMLInputElement;
    toggle.checked = d.checks_enabled;
    toggle.addEventListener('change', () => void this.setChecks(toggle.checked));
    const n = this.selected.size;
    const create = h('button', { type: 'button', class: 'primary wide', id: 'updates-create', disabled: !n || this.creating }, this.creating ? t('common.freezingPlan') : n ? t('updates.createUpdatePlanN', { n }) : t('updates.selectAppsUpdatesFirst'));
    create.addEventListener('click', () => void this.createPlan());
    const counts = d.counts;
    return h(
      'aside',
      { class: 'panel work-aside draft', 'aria-labelledby': 'updates-aside-title' },
      h('h2', { id: 'updates-aside-title' }, t('updates.updateDraft')),
      h(
        'ul',
        { class: 'draft-list' },
        h('li', {}, h('span', {}, UPDATE_STATE.update_available.label), h('b', { class: 'num' }, groupDigits(counts.update_available ?? 0))),
        h('li', {}, h('span', {}, UPDATE_STATE.cannot_check.label), h('b', { class: 'num' }, groupDigits(counts.cannot_check ?? 0))),
        h('li', {}, h('span', {}, UPDATE_STATE.up_to_date.label), h('b', { class: 'num' }, groupDigits(counts.up_to_date ?? 0))),
        counts.not_checked ? h('li', {}, h('span', {}, UPDATE_STATE.not_checked.label), h('b', { class: 'num' }, groupDigits(counts.not_checked))) : null,
      ),
      h('p', { class: 'small' }, words().updatesPlanNote),
      learnMore(h('p', {}, t('updates.runningAppsSkippedClose'))),
      errorLine(this.createError),
      create,
      h(
        'div',
        { class: 'aside-section' },
        h('label', { class: 'switch-row', for: 'updates-checks' }, toggle, h('span', {}, h('b', {}, t('updates.allowUpdateChecks')), h('span', { class: 'muted small' }, t('updates.whenOffNoChannel')))),
      ),
    );
  }

  private bar(): HTMLElement {
    return selectionBar({
      count: this.selected.size,
      sum: null,
      onClear: () => {
        this.selected.clear();
        this.app.announce(t('common.selectionCleared'));
        this.update();
      },
      action: { id: 'updates-bar-go', text: t('updates.createUpdatePlanReview'), disabled: this.creating, onClick: () => void this.createPlan() },
    });
  }
}
