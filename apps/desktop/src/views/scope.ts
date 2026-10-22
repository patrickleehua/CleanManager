// P01 — first run and scan scope. One message: choose a folder, then start the read-only scan. No sizes
// are shown before a scan. Roots that cannot be scanned say why in Chinese before anything is submitted
// (a file, a link, a missing folder); nothing is silently substituted.

import type { App } from '../main';
import { toIpcError } from '../lib/api';
import { refusalText } from '../lib/codes';
import { h, icon, keepFocus, replace } from '../lib/dom';
import { formatTime, isTerminal, STATE_LABEL, stateTone } from '../lib/format';
import type { IpcError, RecentScan, RootOffer } from '../lib/types';
import { learnMore, pathText } from './ui';

const GROUPS: { kinds: RootOffer['kind'][]; label: string }[] = [
  { kinds: ['user_folder', 'user_home'], label: '你的文件夹' },
  { kinds: ['volume'], label: '整个卷' },
  { kinds: ['chosen'], label: '你选择的文件夹' },
];

export class ScopePage {
  el: HTMLElement;
  private list: HTMLElement;
  private feedback: HTMLElement;
  private actions: HTMLElement;
  private aside: HTMLElement;
  private offers: RootOffer[] = [];
  private selected: string | null = null;
  private loading = false;
  private loaded = false;
  private starting = false;
  private loadError: IpcError | null = null;
  private startError: IpcError | null = null;
  /** The last pick from the folder picker, when the host refused it (a file, a link…). */
  private refused: RootOffer | null = null;

  constructor(private app: App) {
    this.list = h('div', { class: 'root-groups' });
    this.feedback = h('div', { class: 'scope-feedback' });
    this.actions = h('div', { class: 'scope-actions' });
    this.aside = h('aside', { class: 'scope-aside', 'aria-labelledby': 'scope-recent-title' });
    this.el = h(
      'section',
      { class: 'page page-scope', 'aria-labelledby': 'scope-title' },
      h(
        'div',
        { class: 'scope-main' },
        h(
          'div',
          { class: 'scope-head' },
          h('h1', { id: 'scope-title' }, '选择要扫描的文件夹'),
          h('p', { class: 'lede' }, '只读取名称、大小和时间，不会打开、移动或删除文件。'),
          learnMore(
            h(
              'ul',
              { class: 'facts' },
              h('li', {}, h('b', {}, '普通权限'), '读不到的位置记为缺口，不会自动提权，也不会按 0 计算。'),
              h('li', {}, h('b', {}, '只在本机'), '索引保存在本机的用户数据目录里，不上传。'),
              h('li', {}, h('b', {}, '随时可停'), '取消后保留已确认的结果，并明确标为“已取消”。'),
            ),
          ),
        ),
        h('fieldset', { class: 'scope-panel panel' }, h('legend', { class: 'sr-only' }, '扫描范围'), this.list, this.feedback, this.actions),
      ),
      this.aside,
    );
  }

  activate(): void {
    if (!this.loaded && !this.loading) void this.load();
    else this.update();
  }

  private async load(): Promise<void> {
    this.loading = true;
    this.loadError = null;
    this.update();
    try {
      this.offers = await this.app.backend.listScanRoots();
      if (!this.selected) {
        // The scan being looked at, when it is still offered; else the first scannable user folder. Never a blocked root.
        const current = this.app.store.state.status?.root_display;
        const same = this.offers.find((o) => !o.problem && o.path_display === current);
        const rec = this.offers.find((o) => !o.problem && (o.kind === 'user_folder' || o.kind === 'user_home'));
        this.selected = (same ?? rec)?.token ?? null;
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
      this.startError = null;
      this.refused = o.problem ? o : null;
      // A refused pick is reported next to the start button and never listed or selected; the previous selection is
      // cleared too, so starting never scans something other than what was just chosen.
      if (o.problem) this.selected = null;
      else {
        this.offers = [...this.offers.filter((x) => x.token !== o.token), o];
        this.selected = o.token;
      }
      this.update();
      if (!o.problem) (this.list.querySelector(`input[value="${CSS.escape(o.token)}"]`) as HTMLInputElement | null)?.focus();
    } catch (e) {
      this.startError = toIpcError(e);
      this.update();
    }
  }

  private async start(): Promise<void> {
    if (!this.selected || this.starting) return;
    this.starting = true;
    this.startError = null;
    this.update();
    try {
      await this.app.startScan(this.selected);
    } catch (e) {
      this.startError = toIpcError(e);
    } finally {
      this.starting = false;
      this.update();
    }
  }

  update(): void {
    keepFocus(this.el, () => this.render());
  }

  private lastScanOf(o: RootOffer): RecentScan | undefined {
    return this.app.store.state.recent.find((r) => r.root_display === o.path_display && r.state);
  }

  private option(o: RootOffer): HTMLElement {
    const id = `root-${o.token}`;
    const input = h('input', { type: 'radio', name: 'scan-root', id, value: o.token, disabled: !!o.problem, 'aria-describedby': `${id}-desc` });
    input.checked = this.selected === o.token;
    input.addEventListener('change', () => {
      this.selected = o.token;
      this.startError = null;
      this.refused = null;
      this.update();
    });
    const last = this.lastScanOf(o);
    const kind = o.kind === 'user_home' ? '用户目录' : null;
    return h(
      'label',
      { class: `root-option${o.problem ? ' blocked' : ''}${input.checked ? ' checked' : ''}`, for: id },
      input,
      h(
        'span',
        { class: 'root-text' },
        h('span', { class: 'root-label' }, o.label, kind ? h('span', { class: 'root-kind' }, kind) : null),
        pathText(o.path_display, { max: 58, className: 'root-path' }),
        h(
          'span',
          { class: 'root-desc', id: `${id}-desc` },
          o.problem
            ? h('span', { class: 'root-problem' }, icon('warn'), refusalText(o.problem))
            : last?.state
              ? h('span', { class: 'root-last' }, `上次扫描 ${formatTime(last.started_at)}，${STATE_LABEL[last.state]}`)
              : null,
        ),
      ),
    );
  }

  private render(): void {
    const s = this.app.store.state;
    if (this.loading && !this.loaded) {
      replace(this.list, h('p', { class: 'muted', role: 'status' }, '正在检查可选位置…'));
    } else if (this.loadError) {
      replace(this.list, h('p', { class: 'form-error', role: 'alert' }, `无法列出位置：${refusalText(this.loadError)}`));
    } else {
      replace(
        this.list,
        ...GROUPS.map((g) => {
          const items = this.offers.filter((o) => g.kinds.includes(o.kind));
          if (!items.length) return null;
          const gid = `scope-group-${g.kinds[0]}`;
          return h('div', { class: 'root-group', role: 'group', 'aria-labelledby': gid }, h('h2', { class: 'group-label', id: gid }, g.label), h('div', { class: 'root-list' }, ...items.map((o) => this.option(o))));
        }),
      );
    }

    // Feedback before anything is submitted: a refused folder from the picker, or a start that was refused.
    const refused = this.refused;
    const offer = this.offers.find((o) => o.token === this.selected);
    const match = offer ? this.lastScanOf(offer) : undefined;
    const problem = this.startError ?? refused?.problem ?? null;
    replace(
      this.feedback,
      problem
        ? h(
            'div',
            { class: 'field-error', role: 'alert' },
            icon('warn'),
            h('div', {}, h('p', { class: 'fe-text' }, refusalText(problem)), refused && !this.startError ? pathText(refused.path_display, { max: 56, className: 'fe-path' }) : null),
          )
        : match?.state
          ? h(
              'div',
              { class: 'notice' },
              h('span', {}, `这个范围在 ${formatTime(match.started_at)} 扫描过，结果为“${STATE_LABEL[match.state]}”。`),
              h('button', { type: 'button', class: 'link', id: 'scope-open-existing', onclick: () => void this.app.openScan(match.scan_id, 'overview') }, '查看结果'),
            )
          : null,
    );

    const running = s.task && !isTerminal(s.task.state);
    const startBtn = h('button', { type: 'button', id: 'scope-start', class: 'primary', disabled: !this.selected || !!offer?.problem || this.starting || !!running }, this.starting ? '正在开始…' : match ? '重新扫描' : '开始只读扫描');
    startBtn.addEventListener('click', () => void this.start());
    const chooseBtn = h('button', { type: 'button', id: 'scope-choose' }, this.refused ? '重新选择文件夹…' : '选择其他文件夹…');
    chooseBtn.addEventListener('click', () => void this.choose());
    replace(this.actions, startBtn, chooseBtn, running ? h('p', { class: 'muted' }, '已有扫描在运行；结束或取消后可以开始新的扫描。') : null);

    this.renderAside();
  }

  /** Recent scans: open a result again (starting another scan needs its folder chosen above). */
  private renderAside(): void {
    const recent = this.app.store.state.recent.filter((r) => r.state);
    this.aside.hidden = !recent.length;
    if (!recent.length) return;
    replace(
      this.aside,
      h('h2', { id: 'scope-recent-title' }, '最近的扫描'),
      h(
        'ul',
        { class: 'recent-list' },
        ...recent.slice(0, 6).map((r) =>
          h(
            'li',
            {},
            h(
              'button',
              { type: 'button', class: 'recent-item', id: `scope-recent-${r.scan_id}`, onclick: () => void this.app.openScan(r.scan_id, 'overview') },
              pathText(r.root_display, { max: 34, className: 'ri-path' }),
              h('span', { class: 'ri-meta' }, h('span', { class: 'state-word', 'data-tone': stateTone(r.state!) }, h('span', { class: 'sw-mark', 'aria-hidden': 'true' }), STATE_LABEL[r.state!]), formatTime(r.started_at)),
            ),
          ),
        ),
      ),
    );
  }
}
