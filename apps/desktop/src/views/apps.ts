// P07 — read-only application inventory (APP-01..04). Names, versions, sources and the
// evidence each identity rests on. Install locations are matched to the current scan by file
// identity; sizes come only from that scan (never a recursive rescan), shared locations are
// marked and not attributed to one app, and an unrecorded location is a gap, not zero.
// There is no uninstall, update or launch action in this release.

import type { App } from '../main';
import { toIpcError } from '../lib/api';
import { formatBytes, groupDigits } from '../lib/bytes';
import { h, icon, keepFocus, replace } from '../lib/dom';
import type { AppItem, AppsList, IpcError } from '../lib/types';
import { emptyState, errorLine, exactTitle, metaRow, pageHead, pill } from './parts';
import { words } from '../lib/platform';
import { issueList, learnMore } from './ui';
import { t } from '../i18n';

const SOURCE_LABEL: Record<AppItem['source'], string> = {
  get registry_uninstall() { return t('apps.sourceLabel.registry_uninstall'); },
  msi: 'Windows Installer',
  get msix_store() { return t('apps.sourceLabel.msix_store'); },
  get app_bundle() { return t('apps.sourceLabel.app_bundle'); },
  app_store: 'Mac App Store',
  homebrew: 'Homebrew',
  get unknown() { return t('apps.sourceLabel.unknown'); },
};

const PROTECTED_LABEL: Record<string, string> = {
  get system_component() { return t('apps.protectedLabel.system_component'); },
  get shared_runtime() { return t('apps.protectedLabel.shared_runtime'); },
  get endpoint_security_or_management() { return t('apps.protectedLabel.endpoint_security_or_management'); },
  get os_package() { return t('apps.protectedLabel.os_package'); },
};

function sizeCell(a: AppItem): { text: string; note: string } {
  switch (a.size.state) {
    case 'known':
      return {
        text: formatBytes(a.size.logical_known_bytes),
        note: a.size.unknown_descendants ? t('apps.unknownDescendants', { unknown_descendants: a.size.unknown_descendants }) : a.shared_with.length ? t('apps.sharedOtherApps') : '',
      };
    case 'not_in_scan':
      return { text: t('apps.notScan'), note: '' };
    case 'no_location':
      return { text: t('apps.locationNotRegistered'), note: '' };
    default:
      return { text: t('apps.locationCantResolved'), note: '' };
  }
}

export class AppsPage {
  el: HTMLElement;
  private body: HTMLElement;
  private data: AppsList | null = null;
  private loading = false;
  private error: IpcError | null = null;
  private boundScan: string | null | undefined = undefined;
  private filter = '';
  private active: string | null = null;

  constructor(private app: App) {
    this.body = h('div', { class: 'apps-body' });
    this.el = h('section', { class: 'page page-apps', 'aria-labelledby': 'apps-title' }, this.body);
  }

  activate(): void {
    const scan = this.app.store.state.status?.published ? this.app.store.state.scanId : null;
    if (scan !== this.boundScan || !this.data) void this.load(scan);
  }

  update(): void {
    keepFocus(this.el, () => this.render());
  }

  private async load(scan: string | null): Promise<void> {
    this.boundScan = scan;
    this.loading = true;
    this.error = null;
    this.update();
    try {
      this.data = await this.app.backend.listApps(scan);
    } catch (e) {
      this.error = toIpcError(e);
    } finally {
      this.loading = false;
      this.update();
    }
  }

  private render(): void {
    const d = this.data;
    const refresh = h('button', { type: 'button', id: 'apps-refresh', disabled: this.loading }, this.loading ? t('common.reading') : t('common.reload'));
    refresh.addEventListener('click', () => void this.load(this.boundScan ?? null));
    const head = pageHead({
      id: 'apps-title',
      title: t('common.apps'),
      meta: [
        t('apps.readOnlyListNothing'),
        d?.scan_root_display ? h('span', {}, t('apps.sizesScan'), h('span', { class: 'path' }, d.scan_root_display)) : t('apps.noPublishedScanSo'),
      ],
      actions: [refresh],
    });
    if (!d) {
      replace(this.body, head, this.error ? h('div', { class: 'panel' }, errorLine(this.error)) : h('p', { class: 'muted', role: 'status' }, t('apps.readingInstalledApps')));
      return;
    }
    if (!d.supported) {
      replace(this.body, head, h('div', { class: 'panel' }, emptyState(t('apps.appInventoryIsntSupported'), t('apps.doesntMeanNoApps'))));
      return;
    }
    const q = this.filter.trim().toLowerCase();
    const items = q ? d.items.filter((a) => `${a.name} ${a.publisher ?? ''}`.toLowerCase().includes(q)) : d.items;
    const search = h('input', { type: 'search', id: 'apps-search', placeholder: t('apps.filterNamePublisher'), 'aria-label': t('apps.filterNamePublisher'), value: this.filter }) as HTMLInputElement;
    search.addEventListener('input', () => {
      this.filter = search.value;
      const caret = search.selectionStart ?? search.value.length;
      this.update();
      const again = document.getElementById('apps-search') as HTMLInputElement | null;
      again?.setSelectionRange(caret, caret);
    });
    const active = d.items.find((a) => a.instance_id === this.active) ?? null;
    const rows = items.map((a) => {
      const s = sizeCell(a);
      const btn = h('button', { type: 'button', class: 'row-link', id: `app-${a.instance_id}`, 'aria-pressed': a.instance_id === this.active ? 'true' : 'false' }, a.name);
      btn.addEventListener('click', () => {
        this.active = a.instance_id;
        this.update();
      });
      const flags = [
        a.shared_with.length ? pill(t('apps.sharedLocation'), 'warn') : null,
        a.protected_reason ? pill(PROTECTED_LABEL[a.protected_reason] ?? t('common.protected'), 'muted') : null,
      ].filter(Boolean) as HTMLElement[];
      return h(
        'tr',
        { class: a.instance_id === this.active ? 'selected' : '' },
        h('td', { class: 'col-object' }, btn, h('span', { class: 'obj-path' }, [a.publisher, a.version].filter(Boolean).join(t('common.commaSep')) || t('common.unknownPublisher'))),
        h('td', { class: 'col-source' }, SOURCE_LABEL[a.source]),
        h('td', { class: `col-size num${a.size.state === 'known' ? '' : ' muted'}`, title: a.size.logical_known_bytes ? exactTitle(a.size.logical_known_bytes) : '' }, s.text, s.note ? h('small', {}, s.note) : null),
        h('td', { class: 'col-flags' }, ...flags),
      );
    });
    replace(
      this.body,
      head,
      d.issues.length ? h('div', { class: 'notice warn' }, h('span', {}, t('apps.someSourcesWerentRead')), learnMore(issueList(d.issues, { label: t('apps.sourcesNotFullyRead') }))) : null,
      h(
        'div',
        { class: 'work-layout' },
        h(
          'div',
          { class: 'work-main' },
          h(
            'section',
            { class: 'panel items-panel', 'aria-labelledby': 'apps-list-title' },
            h('div', { class: 'panel-head' }, h('h2', { id: 'apps-list-title' }, t('apps.installedAppsLengthP', { length: items.length, p: q ? ` / ${groupDigits(d.items.length)}` : '' })), search),
            items.length
              ? h(
                  'div',
                  { class: 'table-wrap' },
                  h(
                    'table',
                    { class: 'data-table apps-table' },
                    h('caption', { class: 'sr-only' }, t('apps.installedAppsSelectRow')),
                    h('thead', {}, h('tr', {}, h('th', { scope: 'col' }, t('common.app')), h('th', { scope: 'col', class: 'col-source' }, t('apps.source')), h('th', { scope: 'col', class: 'col-size' }, t('apps.installLocationSize')), h('th', { scope: 'col', class: 'col-flags' }, t('apps.flags')))),
                    h('tbody', {}, ...rows),
                  ),
                )
              : h('p', { class: 'muted small' }, q ? t('apps.noMatchingApps') : t('apps.noAppRecordsRead')),
          ),
        ),
        this.detail(active),
      ),
    );
  }

  private detail(a: AppItem | null): HTMLElement {
    if (!a)
      return h(
        'aside',
        { class: 'panel work-aside', 'aria-labelledby': 'app-detail-title' },
        h('h2', { id: 'app-detail-title' }, t('apps.appDetails')),
        h('p', { class: 'muted small' }, t('apps.selectAppSeeIdentity')),
      );
    const s = sizeCell(a);
    return h(
      'aside',
      { class: 'panel work-aside', 'aria-labelledby': 'app-detail-title' },
      h('h2', { id: 'app-detail-title', class: 'detail-name' }, a.name),
      metaRow(SOURCE_LABEL[a.source], a.version ? t('common.versionVersion', { version: a.version }) : t('apps.versionUnknown')),
      h(
        'dl',
        { class: 'facts-list' },
        h('dt', {}, t('apps.publisher')),
        h('dd', {}, a.publisher ?? t('common.unknown')),
        h('dt', {}, t('apps.installLocation')),
        h('dd', { class: 'path' }, a.location_display ?? t('apps.notRegisteredStorageUnknown')),
        h('dt', {}, t('common.size')),
        h('dd', {}, s.text, s.note ? h('small', {}, s.note) : null),
        a.shared_with.length ? h('dt', {}, t('apps.shared')) : null,
        a.shared_with.length ? h('dd', {}, t('apps.usesSameNestedLocations', { join: a.shared_with.join(t('common.listSep')) })) : null,
        h('dt', {}, t('common.uninstall')),
        h('dd', {}, words().appsUninstallNote),
      ),
      h('h3', {}, t('apps.identityEvidence')),
      h('ul', { class: 'evidence' }, ...a.evidence.map((e) => h('li', { class: 'path' }, e))),
      this.diagButton(a),
    );
  }

  /** Optional AI diagnosis of the app's install location (only when it is in the bound scan). */
  private diagButton(a: AppItem): HTMLElement | null {
    const scanId = this.app.store.state.scanId;
    const entryId = a.size.entry_id;
    if (!scanId || !entryId) return null;
    return h(
      'div',
      { class: 'card-actions' },
      h('button', { type: 'button', class: 'quiet', id: 'app-diag', onclick: () => this.app.openDiagnosis({ scanId, entryId, origin: 'apps', name: a.name }, 'apps') }, icon('diag'), t('apps.diagnoseInstallLocationAi')),
    );
  }
}
