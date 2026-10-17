// 状态 › 进程 (STS-04/05). A sortable, searchable list explained from local facts (path, signer,
// parent, owning app, known purpose); unknown processes are marked and point to the optional AI
// diagnosis. Pins keep names on top. The only handling is ending one process, and that is a plan
// for exactly that process, reviewed like every other; protected processes have no end action.
// Sustained-CPU alerts are opt-in and only sampled while the status pages are open.

import type { App } from '../main';
import { formatBytes, groupDigits } from '../lib/bytes';
import { h, icon, keepFocus, replace } from '../lib/dom';
import { formatTime } from '../lib/format';
import { RequestGate } from '../lib/gate';
import type { AlertSettings, ProcessList, ProcessRow, ProcessSort } from '../lib/suiteTypes';
import { PROTECTION_WORD, SIGNATURE_WORD, SORT_WORD, processNote } from '../lib/suiteWords';
import { PROCESS_SORTS, loadPrefs, updatePrefs } from '../lib/statusPrefs';
import type { IpcError } from '../lib/types';
import { LongProgress, ensureReady, noticeLine, toIpcError } from './kit';
import { emptyState, errorLine, pageHead, pill } from './parts';
import { notOnThisPlatform, words } from '../lib/platform';

const SORTS: readonly ProcessSort[] = PROCESS_SORTS;

export function rate(v: number | null): string {
  if (v === null) return '—';
  return `${formatBytes(BigInt(Math.round(v)))}/s`;
}

export function cpu(v: number | null): string {
  return v === null ? '—' : `${v.toFixed(v < 10 ? 1 : 0)}%`;
}

export class ProcessesPage {
  el: HTMLElement;
  private body: HTMLElement;
  private data: ProcessList | null = null;
  private error: IpcError | null = null;
  private loading = false;
  /** Remembered across sessions with the other status display preferences (STS-07). */
  private sort: ProcessSort = loadPrefs().processSort;
  private query = '';
  private focus: number | null = null;
  private gate = new RequestGate();
  private searchTimer: number | null = null;
  private ending = false;
  private endError: IpcError | null = null;
  private alerts: AlertSettings | null = null;
  private alertDraft: AlertSettings | null = null;
  private alertError: IpcError | null = null;
  private alertNote: string | null = null;
  private progress = new LongProgress('读取进程');

  constructor(private app: App) {
    this.body = h('div', { class: 'processes-body' });
    this.el = h('section', { class: 'page page-processes', 'aria-labelledby': 'proc-title' }, this.body);
  }

  activate(): void {
    const f = this.app.store.state.processFocus;
    if (f !== null) {
      this.focus = f;
      this.app.store.set({ processFocus: null });
    }
    void this.load();
    if (!this.alerts) void this.loadAlerts();
  }

  update(): void {
    keepFocus(this.el, () => this.render());
  }

  private async load(): Promise<void> {
    this.loading = true;
    this.error = null;
    if (!this.data) this.progress.start('正在读取进程，并在 1 秒窗口内计算处理器与磁盘速率…');
    this.update();
    try {
      const r = await this.gate.latest(this.app.backend.processList(this.sort, this.query.trim() || null, 300));
      if (!r.stale) this.data = r.value;
    } catch (e) {
      this.error = toIpcError(e);
    } finally {
      this.loading = false;
      this.progress.stop();
      this.update();
    }
  }

  private async loadAlerts(): Promise<void> {
    try {
      this.alerts = await this.app.backend.alertsSettings();
      this.alertDraft = { ...this.alerts };
    } catch (e) {
      this.alertError = toIpcError(e);
    }
    this.update();
  }

  private async saveAlerts(): Promise<void> {
    if (!this.alertDraft) return;
    this.alertError = null;
    try {
      this.alerts = await this.app.backend.alertsSave(this.alertDraft);
      this.alertDraft = { ...this.alerts };
      this.alertNote = this.alerts.enabled ? '已开启。“系统状态”页打开时随采样检查，满足条件时提醒一次。' : '已关闭：不再为提醒采样进程。';
      this.app.announce(this.alertNote);
    } catch (e) {
      this.alertError = toIpcError(e);
    }
    this.update();
  }

  private async pin(p: ProcessRow): Promise<void> {
    try {
      await this.app.backend.processPin(p.name, !p.pinned);
      this.app.announce(p.pinned ? `已取消置顶 ${p.name}` : `已置顶 ${p.name}`);
      await this.load();
    } catch (e) {
      this.error = toIpcError(e);
      this.update();
    }
  }

  private async end(p: ProcessRow): Promise<void> {
    if (this.ending) return;
    if (!(await ensureReady(this.app, 'process', `结束 ${p.name}`))) return;
    this.ending = true;
    this.endError = null;
    this.update();
    try {
      const r = await this.app.backend.processEndPlan(p.pid);
      this.app.announce(`已生成结束 ${p.name} 的计划，尚未执行。请审阅后批准`);
      this.app.openPlan(r.plan_id, 'processes');
    } catch (e) {
      this.endError = toIpcError(e);
    } finally {
      this.ending = false;
      this.update();
    }
  }

  // ---- rendering ---------------------------------------------------------------------------

  private render(): void {
    const d = this.data;
    const refresh = h('button', { type: 'button', id: 'proc-refresh', disabled: this.loading }, icon('refresh'), this.loading ? '正在读取…' : '刷新');
    refresh.addEventListener('click', () => void this.load());
    if (d && !d.supported) {
      replace(this.body, pageHead({ id: 'proc-title', title: '进程' }), emptyState('此平台暂不提供', notOnThisPlatform('进程列表目前只在 Windows 上实现。'), null));
      return;
    }
    replace(
      this.body,
      pageHead({
        id: 'proc-title',
        title: '进程',
        meta: d ? [`${groupDigits(d.total ?? d.items.length)} 个进程`, `未识别 ${groupDigits(d.unidentified ?? 0)} 个`, d.sampled_at ? `采样于 ${formatTime(d.sampled_at)}` : null, '只读清单'] : ['只读清单'],
        actions: [refresh],
      }),
      this.progress.el,
      errorLine(this.error),
      d ? h('div', { class: 'work-layout' }, h('div', { class: 'work-main' }, this.listPanel(d)), h('div', { class: 'work-aside proc-aside' }, this.detail(d), this.alertsPanel())) : null,
    );
  }

  private listPanel(d: ProcessList): HTMLElement {
    const seg = h(
      'fieldset',
      { class: 'seg' },
      h('legend', { class: 'sr-only' }, '排序'),
      ...SORTS.map((k) => {
        const input = h('input', { type: 'radio', name: 'proc-sort', id: `proc-sort-${k}`, value: k }) as HTMLInputElement;
        input.checked = this.sort === k;
        input.addEventListener('change', () => {
          this.sort = k;
          updatePrefs((p) => ({ ...p, processSort: k }));
          void this.load();
        });
        return h('label', { for: `proc-sort-${k}` }, input, h('span', {}, SORT_WORD[k]));
      }),
    );
    const search = h('input', { type: 'search', id: 'proc-search', placeholder: '名称、PID、路径、签名者或用途', 'aria-label': '查找进程', value: this.query }) as HTMLInputElement;
    search.addEventListener('input', () => {
      this.query = search.value;
      if (this.searchTimer !== null) window.clearTimeout(this.searchTimer);
      this.searchTimer = window.setTimeout(() => void this.load(), 250);
    });
    const rows = d.items.map((p) => {
      const pinBtn = h('button', { type: 'button', class: `icon-btn small-btn pin-btn${p.pinned ? ' active' : ''}`, 'aria-pressed': p.pinned ? 'true' : 'false', 'aria-label': p.pinned ? `取消置顶 ${p.name}` : `置顶 ${p.name}`, title: p.pinned ? '取消置顶' : '置顶（按名称）' }, icon('pin'));
      pinBtn.addEventListener('click', () => void this.pin(p));
      const name = h('button', { type: 'button', class: 'row-link', id: `proc-${p.pid}`, 'aria-pressed': this.focus === p.pid ? 'true' : 'false' }, p.name);
      name.addEventListener('click', () => {
        this.focus = p.pid;
        this.endError = null;
        this.update();
        // One column: the detail sits below the list; bring it into view.
        if (window.matchMedia('(max-width: 1180px)').matches) requestAnimationFrame(() => document.getElementById('proc-detail-title')?.scrollIntoView({ block: 'start' }));
      });
      const sub = p.identified ? (p.purpose ?? p.owner_app ?? '') : '';
      return h(
        'tr',
        { class: this.focus === p.pid ? 'selected' : '' },
        h('td', { class: 'col-pin' }, pinBtn),
        h(
          'td',
          { class: 'col-object' },
          h('span', { class: 'obj-name proc-name' }, name, h('span', { class: 'muted small num' }, `PID ${p.pid}`)),
          sub ? h('span', { class: 'obj-path' }, sub) : null,
          !p.identified ? h('span', { class: 'unid-tag' }, h('span', { class: 'unid-mark', 'aria-hidden': 'true' }), '未识别') : null,
        ),
        h('td', { class: 'col-num-s num' }, cpu(p.cpu_percent)),
        h('td', { class: 'col-num-s num' }, p.memory_bytes === null ? '—' : formatBytes(p.memory_bytes)),
        h('td', { class: 'col-num-s num' }, rate(p.disk_bytes_per_sec)),
        h('td', { class: 'col-sig small' }, p.signature ? SIGNATURE_WORD[p.signature.state] : '—', p.end_capability.state !== 'available' ? h('span', { class: 'block muted' }, '受保护') : null),
      );
    });
    return h(
      'section',
      { class: 'panel', 'aria-labelledby': 'proc-list-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'proc-list-title' }, '正在运行'), h('div', { class: 'list-tools' }, seg, search)),
      d.items.length
        ? h(
            'div',
            { class: 'table-wrap' },
            h(
              'table',
              { class: 'data-table proc-table' },
              h('caption', { class: 'sr-only' }, `进程，按${SORT_WORD[this.sort]}排序，置顶的在最前`),
              h(
                'thead',
                {},
                h(
                  'tr',
                  {},
                  h('th', { scope: 'col', class: 'col-pin' }, h('span', { class: 'sr-only' }, '置顶')),
                  h('th', { scope: 'col' }, '进程与用途'),
                  h('th', { scope: 'col', class: 'col-num-s' }, '处理器'),
                  h('th', { scope: 'col', class: 'col-num-s' }, '内存'),
                  h('th', { scope: 'col', class: 'col-num-s' }, '磁盘'),
                  h('th', { scope: 'col', class: 'col-sig' }, '签名'),
                ),
              ),
              h('tbody', {}, ...rows),
            ),
          )
        : h('p', { class: 'muted small' }, '没有符合条件的进程。'),
      h('p', { class: 'muted small' }, words().processesFoot),
    );
  }

  private detail(d: ProcessList): HTMLElement {
    const p = d.items.find((x) => x.pid === this.focus) ?? null;
    if (!p)
      return h('section', { class: 'panel', 'aria-labelledby': 'proc-detail-title' }, h('h2', { id: 'proc-detail-title' }, '进程详情'), h('p', { class: 'muted small' }, '点一个进程的名称，查看它的路径、签名、父进程和用途。'));
    const end =
      p.end_capability.state === 'available'
        ? h('button', { type: 'button', class: 'danger-quiet wide', id: 'proc-end', disabled: this.ending }, this.ending ? '正在生成计划…' : '结束这个进程…')
        : null;
    end?.addEventListener('click', () => void this.end(p));
    return h(
      'section',
      { class: 'panel', 'aria-labelledby': 'proc-detail-title' },
      h('h2', { id: 'proc-detail-title' }, p.name),
      p.identified ? h('p', { class: 'small' }, p.purpose ?? '用途来自本地知识表。') : null,
      !p.identified
        ? h(
            'div',
            { class: 'unid-box' },
            h('p', { class: 'small' }, h('b', {}, '未识别：'), p.unidentified_reason ?? '本地知识表中没有它。'),
            h('p', { class: 'muted small' }, '可以用可选的 AI 诊断询问它是什么。发送前会完整显示将要发送的内容（名称、路径、签名者、父进程），列表本身不发送任何数据。'),
            h('button', { type: 'button', class: 'quiet', id: 'proc-diag', onclick: () => (this.app.store.set({ diagTarget: null, diagId: null, returnTo: 'processes' }), this.app.navigate('diag')) }, icon('diag'), '打开 AI 诊断'),
          )
        : null,
      h(
        'dl',
        { class: 'facts-list small' },
        h('dt', {}, 'PID'),
        h('dd', { class: 'num' }, String(p.pid)),
        h('dt', {}, '父进程'),
        h('dd', {}, p.parent_name ? `${p.parent_name}（${p.parent_pid}）` : p.parent_pid ? String(p.parent_pid) : '—'),
        h('dt', {}, '路径'),
        h('dd', { class: 'path' }, p.image_path ?? '读不到（受保护或属于其他用户）'),
        h('dt', {}, '签名'),
        h('dd', {}, p.signature ? `${SIGNATURE_WORD[p.signature.state]}${p.signature.publisher ? `：${p.signature.publisher}` : ''}` : '—', p.signature?.reason ? h('small', {}, p.signature.reason) : null),
        h('dt', {}, '所属应用'),
        h('dd', {}, p.owner_app ?? '—'),
        h('dt', {}, '处理器 / 内存'),
        h('dd', { class: 'num' }, `${cpu(p.cpu_percent)} / ${p.memory_bytes === null ? '—' : formatBytes(p.memory_bytes)}`),
      ),
      ...p.notes.map((n) => h('p', { class: 'muted small' }, processNote(n))),
      end
        ? h('div', { class: 'aside-section' }, end, h('p', { class: 'muted small' }, '只生成一份针对这一个进程的计划：执行前按 PID、启动时间和程序文件重新核对，不符就跳过。其中未保存的工作会丢失。'), errorLine(this.endError))
        : h('div', { class: 'aside-section' }, pill('没有结束入口', 'muted'), h('p', { class: 'small' }, PROTECTION_WORD[p.end_capability.protection ?? ''] ?? p.end_capability.reason)),
    );
  }

  private alertsPanel(): HTMLElement {
    const a = this.alertDraft;
    if (!a) return h('section', { class: 'panel' }, h('h2', {}, '持续高占用提醒'), errorLine(this.alertError), h('p', { class: 'muted small' }, this.alertError ? '' : '正在读取设置…'));
    const toggle = h('input', { type: 'checkbox', id: 'alert-on', role: 'switch' }) as HTMLInputElement;
    toggle.checked = a.enabled;
    toggle.addEventListener('change', () => {
      this.alertDraft = { ...a, enabled: toggle.checked };
      this.alertNote = null;
      this.update();
    });
    const num = (id: string, label: string, value: number, min: number, max: number, unit: string, set: (v: number) => void) => {
      const input = h('input', { type: 'number', id, min: String(min), max: String(max), step: '1', value: String(value), disabled: !a.enabled, class: 'num-input' }) as HTMLInputElement;
      input.addEventListener('change', () => {
        set(Number(input.value));
        this.alertNote = null;
      });
      return h('label', { class: 'num-field', for: id }, h('span', {}, label), h('span', { class: 'num-wrap' }, input, h('span', { class: 'muted small' }, unit)));
    };
    const changed = JSON.stringify(a) !== JSON.stringify(this.alerts);
    const save = h('button', { type: 'button', class: changed ? 'primary' : '', id: 'alert-save', disabled: !changed }, '保存提醒设置');
    save.addEventListener('click', () => void this.saveAlerts());
    return h(
      'section',
      { class: 'panel alerts-panel', 'aria-labelledby': 'alerts-title' },
      h('h2', { id: 'alerts-title' }, '持续高占用提醒'),
      h('label', { class: 'switch-row', for: 'alert-on' }, toggle, h('span', {}, h('b', {}, '开启提醒'), h('span', { class: 'muted small' }, '默认关闭。关闭时不为提醒采样任何进程。'))),
      h(
        'div',
        { class: 'num-fields' },
        num('alert-threshold', '整机处理器占用不低于', a.cpu_threshold_percent, 1, 100, '%', (v) => (this.alertDraft = { ...this.alertDraft!, cpu_threshold_percent: v })),
        num('alert-window', '持续', a.window_seconds, 10, 3600, '秒', (v) => (this.alertDraft = { ...this.alertDraft!, window_seconds: v })),
        num('alert-cooldown', '同一程序再次提醒间隔', a.cooldown_seconds, 60, 86400, '秒', (v) => (this.alertDraft = { ...this.alertDraft!, cooldown_seconds: v })),
      ),
      errorLine(this.alertError),
      this.alertNote ? noticeLine(this.alertNote) : null,
      save,
      h('p', { class: 'muted small' }, '提醒只在“状态 › 系统状态”页打开时检查（随每次采样），没有后台常驻；提醒只指出进程，不会结束它。'),
    );
  }
}
