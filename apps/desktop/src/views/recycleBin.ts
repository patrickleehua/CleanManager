// 清理 › 回收站 (WFA-65, MTR-03): what is in the current user's bin — name, where it was deleted from (when the bin
// recorded it), size and when — read-only. Items of one bin can be frozen into a separate plan of permanent deletes;
// the plan review page then only offers the terminal (deleting for good needs the extra irreversibility confirmation
// that only a terminal asks for). Restoring stays the system's job (资源管理器 / 访达); CleanManager does not put
// items back. Only item ids cross IPC.

import type { App } from '../main';
import { formatBytes, formatExactBytes, parseBytes } from '../lib/bytes';
import { h, icon, keepFocus, replace } from '../lib/dom';
import { formatTime } from '../lib/format';
import { RequestGate } from '../lib/gate';
import { words } from '../lib/platform';
import type { BinItem, BinList, IpcError } from '../lib/types';
import { binGroups, binRefusalText, binSelectable, type BinGroup } from '../lib/binModel';
import { atLeast, selectionBar, sumSizes, toIpcError } from './kit';
import { errorLine, pageHead } from './parts';
import { emptyBlock, issueList, learnMore, pathText, stateLine, techBlock } from './ui';
import { t } from '../i18n';

export class RecycleBinPage {
  el: HTMLElement;
  private body: HTMLElement;
  private list: BinList | null = null;
  private error: IpcError | null = null;
  private loading = false;
  private gate = new RequestGate();
  /** Ticked items (all of one bin: a plan takes one bin). */
  private selected = new Map<string, BinItem>();
  private creating = false;
  private createError: IpcError | null = null;

  constructor(private app: App) {
    this.body = h('div', { class: 'bin-body' });
    this.el = h('section', { class: 'page page-recyclebin', 'aria-labelledby': 'bin-title' }, this.body);
  }

  activate(): void {
    // Opened again: read the bin afresh (items move in and out of it outside CleanManager).
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
      const r = await this.gate.latest(this.app.backend.trashList());
      if (r.stale) return;
      this.list = r.value;
      // A fresh listing: keep only ticks on items that are still there and still purgeable.
      const now = new Map(r.value.items.map((i) => [i.trash_item_id, i]));
      for (const id of [...this.selected.keys()]) {
        const still = now.get(id);
        if (still?.purgeable) this.selected.set(id, still);
        else this.selected.delete(id);
      }
    } catch (e) {
      this.error = toIpcError(e);
    } finally {
      this.loading = false;
      this.update();
    }
  }

  private selectedBin(): string | null {
    const first = this.selected.values().next();
    return first.done ? null : first.value.bin_id;
  }

  private toggle(i: BinItem): void {
    if (this.selected.has(i.trash_item_id)) this.selected.delete(i.trash_item_id);
    else if (binSelectable(i, this.selectedBin())) this.selected.set(i.trash_item_id, i);
    this.createError = null;
    this.app.announce(this.selected.has(i.trash_item_id) ? t('common.selectedName', { name: i.name }) : t('common.deselectedName', { name: i.name }));
    this.update();
  }

  private async createPlan(): Promise<void> {
    if (this.creating || !this.selected.size) return;
    this.creating = true;
    this.createError = null;
    this.update();
    try {
      const r = await this.app.backend.trashPurgePlan([...this.selected.keys()]);
      this.selected.clear();
      this.app.announce(t('common.planCreatedNotRun'));
      this.app.openPlan(r.plan_id, 'recyclebin');
    } catch (e) {
      this.createError = toIpcError(e);
      // The bin changed under the selection: read it again so the list matches what is there.
      if (this.createError.reason === 'bin_item_gone') void this.load();
    } finally {
      this.creating = false;
      this.update();
    }
  }

  private render(): void {
    const w = words();
    const l = this.list;
    const reload = h('button', { type: 'button', id: 'bin-reload', disabled: this.loading }, icon('refresh'), t('common.reload'));
    reload.addEventListener('click', () => void this.load());
    const meta = l ? [t('recycleBin.observed', { time: formatTime(l.observed_at) }), t('recycleBin.readOnly')] : [t('recycleBin.readOnly')];
    const head = pageHead({ id: 'bin-title', title: w.bin, meta, actions: [reload] });
    if (!l) {
      replace(this.body, head, this.error ? errorLine(this.error) : h('p', { class: 'muted', role: 'status' }, t('recycleBin.reading', { bin: w.bin })));
      return;
    }
    if (!l.supported) {
      replace(this.body, head, emptyBlock({ icon: 'bin', title: t('common.notAvailablePlatform'), body: t('recycleBin.unsupported', { bin: w.bin }) }));
      return;
    }
    const groups = binGroups(l);
    const lede = h('p', { class: 'lede bin-lede' }, t('recycleBin.lede', { bin: w.bin, restoreFromBin: w.restoreFromBin }));
    const more = learnMore(
      h('p', {}, w.binListDoes),
      h('p', {}, t('recycleBin.permanentHow')),
      h('p', {}, t('recycleBin.oneBinPerPlan')),
      w.binPutBack ? h('p', {}, w.binPutBack) : null,
    );
    const issues = l.issues.length ? h('div', { class: 'bin-issues' }, stateLine({ kind: 'partial', text: t('recycleBin.someUnread', { bin: w.bin }) }), issueList(l.issues, { label: t('recycleBin.issuesLabel') })) : null;
    replace(
      this.body,
      head,
      lede,
      more,
      errorLine(this.error),
      issues,
      groups.length
        ? h('div', { class: 'bin-groups' }, ...groups.map((g, n) => this.groupPanel(g, n, groups.length)))
        : emptyBlock({ icon: 'bin', title: t('recycleBin.emptyTitle', { bin: w.bin }), body: l.issues.length ? t('recycleBin.emptyButUnread') : null }),
      l.truncated ? h('p', { class: 'muted small' }, t('recycleBin.truncated', { shown: l.items.length, total: l.total })) : null,
      errorLine(this.createError),
      this.bar(),
    );
  }

  private groupPanel(g: BinGroup, n: number, of: number): HTMLElement {
    const sel = this.selectedBin();
    const other = sel !== null && sel !== g.bin_id;
    const sum = sumSizes(g.items.map((i) => ({ bytes: i.logical_bytes })));
    const titleId = `bin-group-${n}`;
    const rows = g.items.map((i) => this.row(i, sel));
    return h(
      'section',
      { class: 'panel bin-group', 'aria-labelledby': titleId, 'data-locked': other ? 'yes' : 'no' },
      h(
        'div',
        { class: 'panel-head' },
        h('h2', { id: titleId }, of > 1 && g.volume_display ? t('recycleBin.groupTitle', { volume: g.volume_display, bin: words().bin }) : t('recycleBin.groupTitleOne', { bin: words().bin })),
        h('span', { class: 'small muted num' }, t('recycleBin.groupCount', { n: g.item_count || g.items.length, size: atLeast(sum) })),
      ),
      other ? h('p', { class: 'muted small' }, t('recycleBin.otherBinLocked')) : null,
      h(
        'div',
        { class: 'table-wrap' },
        h(
          'table',
          { class: 'data-table ctable bin-table' },
          h('caption', { class: 'sr-only' }, t('recycleBin.tableCaption', { bin: words().bin })),
          h(
            'thead',
            {},
            h(
              'tr',
              {},
              h('th', { scope: 'col', class: 'col-check' }, h('span', { class: 'sr-only' }, t('common.select'))),
              h('th', { scope: 'col' }, t('recycleBin.colItem')),
              h('th', { scope: 'col', class: 'col-size' }, t('common.logicalSize')),
              h('th', { scope: 'col', class: 'col-when' }, t('recycleBin.colDeleted')),
            ),
          ),
          h('tbody', {}, ...rows),
        ),
      ),
    );
  }

  private row(i: BinItem, selectedBin: string | null): HTMLElement {
    const id = `bin-${i.trash_item_id}`;
    const can = binSelectable(i, selectedBin);
    const checked = this.selected.has(i.trash_item_id);
    const check = h('input', { type: 'checkbox', id: `${id}-check`, disabled: !checked && (!can || this.creating), 'aria-label': t('common.selectName', { name: i.name }), 'aria-describedby': i.purgeable ? null : `${id}-why` }) as HTMLInputElement;
    check.checked = checked;
    check.addEventListener('change', () => this.toggle(i));
    const why = binRefusalText(i.purgeable ? null : (i.refusal ?? ''));
    const bytes = parseBytes(i.logical_bytes);
    return h(
      'tr',
      { class: `crow${checked ? ' selected' : ''}${i.purgeable ? '' : ' locked'}` },
      h('td', { class: 'col-check' }, check),
      h(
        'td',
        { class: 'col-object' },
        i.purgeable ? h('label', { for: `${id}-check`, class: 'obj-name' }, i.name) : h('span', { class: 'obj-name' }, i.name),
        i.object_kind === 'directory' ? h('span', { class: 'obj-kind muted small' }, t('recycleBin.folder')) : null,
        // Where it came from, when the bin recorded it; macOS never claims it.
        i.original_path_display
          ? h('span', { class: 'obj-path' }, h('span', { class: 'muted' }, t('recycleBin.from')), pathText(i.original_path_display, { max: 64 }))
          : h('span', { class: 'obj-path muted' }, t('recycleBin.fromUnknown')),
        why ? h('span', { class: 'why-not', id: `${id}-why` }, icon('lock'), why) : null,
        !i.purgeable && i.refusal ? techBlock(h('p', { class: 'selectable small' }, i.refusal)) : null,
      ),
      h('td', { class: 'col-size num', title: bytes === null ? null : formatExactBytes(bytes) }, bytes === null ? t('common.sizeUnknown') : formatBytes(bytes)),
      h('td', { class: 'col-when' }, i.deleted_at ? formatTime(i.deleted_at) : h('span', { class: 'muted' }, t('recycleBin.whenUnknown'))),
    );
  }

  private bar(): HTMLElement {
    const items = [...this.selected.values()];
    const bar = selectionBar({
      count: items.length,
      sum: sumSizes(items.map((i) => ({ bytes: i.logical_bytes }))),
      note: t('recycleBin.barNote'),
      onClear: () => {
        this.selected.clear();
        this.createError = null;
        this.app.announce(t('common.selectionCleared'));
        this.update();
      },
      action: { id: 'bin-purge-plan', danger: true, text: this.creating ? t('common.freezingPlan') : t('recycleBin.createPurgePlan', { n: items.length }), disabled: this.creating, onClick: () => void this.createPlan() },
    });
    return bar;
  }
}
