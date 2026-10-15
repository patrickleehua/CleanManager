// P01 — first run and scan scope. No sizes are shown before a scan: the page only lists the
// user's folders and volume roots, explains what a read-only scan does, and starts it on an
// explicit click. Roots that cannot be scanned say why; nothing is silently substituted.

import type { App } from '../main';
import { toIpcError } from '../lib/api';
import { h, keepFocus, replace } from '../lib/dom';
import { ERROR_HINT, formatTime, isTerminal, STATE_LABEL } from '../lib/format';
import type { IpcError, RootOffer } from '../lib/types';

export class ScopePage {
  el: HTMLElement;
  private list: HTMLElement;
  private existing: HTMLElement;
  private actions: HTMLElement;
  private message: HTMLElement;
  private offers: RootOffer[] = [];
  private selected: string | null = null;
  private loading = false;
  private loaded = false;
  private starting = false;
  private loadError: IpcError | null = null;

  constructor(private app: App) {
    this.list = h('div', { class: 'root-list', role: 'radiogroup', 'aria-labelledby': 'scope-list-title' });
    this.existing = h('div', { class: 'existing', 'aria-live': 'polite' });
    this.actions = h('div', { class: 'scope-actions' });
    this.message = h('p', { class: 'form-error', role: 'alert', hidden: true });
    this.el = h(
      'section',
      { class: 'page page-scope', 'aria-labelledby': 'scope-title' },
      h(
        'div',
        { class: 'scope-intro' },
        h('h1', { id: 'scope-title' }, '先选一个要了解的文件夹'),
        h(
          'p',
          { class: 'lede' },
          'CleanManager 只读取名称、大小和时间等元数据，不打开文件内容，也不会移动或删除任何东西。扫描完成后，你可以按大小浏览这个范围里的文件夹和文件。',
        ),
        h(
          'ul',
          { class: 'facts' },
          h('li', {}, h('b', {}, '普通权限'), '读不到的位置会记为缺口，不会自动提权，也不会按 0 计算。'),
          h('li', {}, h('b', {}, '只在本机'), '索引保存在本机的用户数据目录里，不上传。'),
          h('li', {}, h('b', {}, '随时可停'), '取消后会保留已确认的结果，并明确标为“已取消”。'),
        ),
      ),
      h(
        'div',
        { class: 'scope-panel panel' },
        h('h2', { id: 'scope-list-title' }, '扫描范围'),
        this.list,
        this.existing,
        this.message,
        this.actions,
      ),
    );
  }

  activate(): void {
    if (!this.loaded && !this.loading) void this.load();
  }

  private async load(): Promise<void> {
    this.loading = true;
    this.loadError = null;
    this.update();
    try {
      this.offers = await this.app.backend.listScanRoots();
      if (!this.selected) {
        // Recommend the first scannable user folder; never pre-select a blocked root.
        const rec = this.offers.find((o) => !o.problem && (o.kind === 'user_folder' || o.kind === 'user_home'));
        this.selected = rec?.token ?? null;
      }
      this.loaded = true;
    } catch (e) {
      this.loadError = toIpcError(e);
    } finally {
      this.loading = false;
      this.update();
    }
  }

  private async choose(): Promise<void> {
    try {
      const o = await this.app.backend.chooseFolder();
      if (!o) return;
      this.offers = [...this.offers.filter((x) => x.token !== o.token), o];
      this.selected = o.token;
      this.showMessage(null);
      this.update();
      (this.list.querySelector(`input[value="${CSS.escape(o.token)}"]`) as HTMLInputElement | null)?.focus();
    } catch (e) {
      this.showMessage(toIpcError(e));
    }
  }

  private async start(): Promise<void> {
    if (!this.selected || this.starting) return;
    this.starting = true;
    this.update();
    try {
      await this.app.startScan(this.selected);
      this.showMessage(null);
    } catch (e) {
      this.showMessage(toIpcError(e));
    } finally {
      this.starting = false;
      this.update();
    }
  }

  private showMessage(e: IpcError | null): void {
    this.message.hidden = !e;
    this.message.textContent = e ? (ERROR_HINT[e.reason] ?? e.message) : '';
  }

  update(): void {
    keepFocus(this.el, () => this.render());
  }

  private render(): void {
    const s = this.app.store.state;
    if (this.loading && !this.loaded) {
      replace(this.list, h('p', { class: 'muted', role: 'status' }, '正在检查可选位置…'));
    } else if (this.loadError) {
      replace(this.list, h('p', { class: 'form-error' }, `无法列出位置：${this.loadError.message}`));
    } else {
      replace(
        this.list,
        ...this.offers.map((o) => {
          const id = `root-${o.token}`;
          const input = h('input', { type: 'radio', name: 'scan-root', id, value: o.token, disabled: !!o.problem, 'aria-describedby': `${id}-desc` });
          input.checked = this.selected === o.token;
          input.addEventListener('change', () => {
            this.selected = o.token;
            this.showMessage(null);
            this.update();
          });
          const kindText = { user_home: '用户目录', user_folder: '用户文件夹', volume: '整个卷', chosen: '你选择的文件夹' }[o.kind];
          return h(
            'label',
            { class: `root-option${o.problem ? ' blocked' : ''}${input.checked ? ' checked' : ''}`, for: id },
            input,
            h(
              'span',
              { class: 'root-text' },
              h('span', { class: 'root-label' }, o.label, h('span', { class: 'root-kind' }, kindText)),
              h('span', { class: 'root-path' }, o.path_display),
              h('span', { class: 'root-problem', id: `${id}-desc` }, o.problem ? (ERROR_HINT[o.problem.reason] ?? o.problem.message) : ''),
            ),
          );
        }),
      );
    }

    // Matching earlier scan for the selected root (P01 "已有匹配扫描").
    const offer = this.offers.find((o) => o.token === this.selected);
    const match = offer ? s.recent.find((r) => r.root_display === offer.path_display && r.state) : undefined;
    if (match && match.state) {
      replace(
        this.existing,
        h(
          'div',
          { class: 'notice' },
          h('span', {}, `这个范围在 ${formatTime(match.started_at)} 扫描过（${STATE_LABEL[match.state]}）。可以先查看已有结果，或重新扫描。`),
          h('button', { type: 'button', class: 'link', onclick: () => void this.app.openScan(match.scan_id, 'overview') }, '查看已有结果'),
        ),
      );
    } else replace(this.existing);

    const running = s.task && !isTerminal(s.task.state);
    const startBtn = h('button', { type: 'button', id: 'scope-start', class: 'primary', disabled: !this.selected || !!offer?.problem || this.starting || !!running }, this.starting ? '正在开始…' : match ? '重新扫描' : '开始只读扫描');
    startBtn.addEventListener('click', () => void this.start());
    const chooseBtn = h('button', { type: 'button', id: 'scope-choose' }, '选择文件夹…');
    chooseBtn.addEventListener('click', () => void this.choose());
    replace(
      this.actions,
      startBtn,
      chooseBtn,
      running ? h('p', { class: 'muted' }, '已有扫描在运行；结束或取消后可以开始新的扫描。') : null,
    );
  }
}
