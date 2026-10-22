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

const SOURCE_LABEL: Record<AppItem['source'], string> = {
  registry_uninstall: '安装程序登记',
  msi: 'Windows Installer',
  msix_store: 'MSIX / 应用商店',
  app_bundle: '应用包（.app）',
  app_store: 'Mac App Store',
  homebrew: 'Homebrew',
  unknown: '来源未知',
};

const PROTECTED_LABEL: Record<string, string> = {
  system_component: '系统组件',
  shared_runtime: '共享运行库',
  endpoint_security_or_management: '安全或设备管理软件',
  os_package: '系统包',
};

function sizeCell(a: AppItem): { text: string; note: string } {
  switch (a.size.state) {
    case 'known':
      return {
        text: formatBytes(a.size.logical_known_bytes),
        note: a.size.unknown_descendants ? `另有 ${a.size.unknown_descendants} 项未知` : a.shared_with.length ? '与其他应用共享' : '',
      };
    case 'not_in_scan':
      return { text: '不在本次扫描内', note: '' };
    case 'no_location':
      return { text: '位置未登记', note: '' };
    default:
      return { text: '位置无法解析', note: '' };
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
    const refresh = h('button', { type: 'button', id: 'apps-refresh', disabled: this.loading }, this.loading ? '正在读取…' : '重新读取');
    refresh.addEventListener('click', () => void this.load(this.boundScan ?? null));
    const head = pageHead({
      id: 'apps-title',
      title: '应用',
      meta: [
        '只读清单：不卸载、不更新、不启动',
        d?.scan_root_display ? h('span', {}, '大小来自扫描 ', h('span', { class: 'path' }, d.scan_root_display)) : '没有已发布的扫描，大小不显示',
      ],
      actions: [refresh],
    });
    if (!d) {
      replace(this.body, head, this.error ? h('div', { class: 'panel' }, errorLine(this.error)) : h('p', { class: 'muted', role: 'status' }, '正在读取已安装应用…'));
      return;
    }
    if (!d.supported) {
      replace(this.body, head, h('div', { class: 'panel' }, emptyState('此平台尚未支持应用清单', '这不代表没有安装应用；只是这个版本还没有经过验证的读取方式。')));
      return;
    }
    const q = this.filter.trim().toLowerCase();
    const items = q ? d.items.filter((a) => `${a.name} ${a.publisher ?? ''}`.toLowerCase().includes(q)) : d.items;
    const search = h('input', { type: 'search', id: 'apps-search', placeholder: '按名称或发布者筛选', 'aria-label': '按名称或发布者筛选', value: this.filter }) as HTMLInputElement;
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
        a.shared_with.length ? pill('共享位置', 'warn') : null,
        a.protected_reason ? pill(PROTECTED_LABEL[a.protected_reason] ?? '受保护', 'muted') : null,
      ].filter(Boolean) as HTMLElement[];
      return h(
        'tr',
        { class: a.instance_id === this.active ? 'selected' : '' },
        h('td', { class: 'col-object' }, btn, h('span', { class: 'obj-path' }, [a.publisher, a.version].filter(Boolean).join('，') || '发布者未知')),
        h('td', { class: 'col-source' }, SOURCE_LABEL[a.source]),
        h('td', { class: `col-size num${a.size.state === 'known' ? '' : ' muted'}`, title: a.size.logical_known_bytes ? exactTitle(a.size.logical_known_bytes) : '' }, s.text, s.note ? h('small', {}, s.note) : null),
        h('td', { class: 'col-flags' }, ...flags),
      );
    });
    replace(
      this.body,
      head,
      d.issues.length ? h('div', { class: 'notice warn' }, h('span', {}, '部分来源没有读完，缺失的记录不代表应用已卸载。'), learnMore(issueList(d.issues, { label: '没有读完的来源' }))) : null,
      h(
        'div',
        { class: 'work-layout' },
        h(
          'div',
          { class: 'work-main' },
          h(
            'section',
            { class: 'panel items-panel', 'aria-labelledby': 'apps-list-title' },
            h('div', { class: 'panel-head' }, h('h2', { id: 'apps-list-title' }, `已安装应用（${groupDigits(items.length)}${q ? ` / ${groupDigits(d.items.length)}` : ''}）`), search),
            items.length
              ? h(
                  'div',
                  { class: 'table-wrap' },
                  h(
                    'table',
                    { class: 'data-table apps-table' },
                    h('caption', { class: 'sr-only' }, '已安装应用，选择一行查看归属依据'),
                    h('thead', {}, h('tr', {}, h('th', { scope: 'col' }, '应用'), h('th', { scope: 'col', class: 'col-source' }, '来源'), h('th', { scope: 'col', class: 'col-size' }, '安装位置大小'), h('th', { scope: 'col', class: 'col-flags' }, '标记'))),
                    h('tbody', {}, ...rows),
                  ),
                )
              : h('p', { class: 'muted small' }, q ? '没有匹配的应用。' : '没有读取到应用记录。'),
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
        h('h2', { id: 'app-detail-title' }, '应用详情'),
        h('p', { class: 'muted small' }, '选择一个应用，查看它的身份依据、安装位置与共享情况。同名应用按来源分开列出，不按名称合并。'),
      );
    const s = sizeCell(a);
    return h(
      'aside',
      { class: 'panel work-aside', 'aria-labelledby': 'app-detail-title' },
      h('h2', { id: 'app-detail-title', class: 'detail-name' }, a.name),
      metaRow(SOURCE_LABEL[a.source], a.version ? `版本 ${a.version}` : '版本未知'),
      h(
        'dl',
        { class: 'facts-list' },
        h('dt', {}, '发布者'),
        h('dd', {}, a.publisher ?? '未知'),
        h('dt', {}, '安装位置'),
        h('dd', { class: 'path' }, a.location_display ?? '未登记（存储情况未知，不是 0）'),
        h('dt', {}, '大小'),
        h('dd', {}, s.text, s.note ? h('small', {}, s.note) : null),
        a.shared_with.length ? h('dt', {}, '共享') : null,
        a.shared_with.length ? h('dd', {}, `与 ${a.shared_with.join('、')} 使用相同或嵌套的位置，空间不归属单个应用。`) : null,
        h('dt', {}, '卸载'),
        h('dd', {}, words().appsUninstallNote),
      ),
      h('h3', {}, '身份依据'),
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
      h('button', { type: 'button', class: 'quiet', id: 'app-diag', onclick: () => this.app.openDiagnosis({ scanId, entryId, origin: 'apps', name: a.name }, 'apps') }, icon('diag'), 'AI 诊断安装位置'),
    );
  }
}
