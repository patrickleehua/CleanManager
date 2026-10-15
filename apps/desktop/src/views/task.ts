// Global task area: the scan hosted by this window, visible on every page. Counts update at the
// host's coalesced rate; screen readers hear only state changes (announced by the app).

import type { App } from '../main';
import { groupDigits } from '../lib/bytes';
import { h, icon, replace } from '../lib/dom';
import { isTerminal, STATE_LABEL, stateTone, taskTitle } from '../lib/format';

export class TaskChip {
  el: HTMLElement;
  private main: HTMLButtonElement;
  private dismiss: HTMLButtonElement;

  constructor(private app: App) {
    this.main = h('button', { type: 'button', class: 'task-main' });
    this.main.addEventListener('click', () => {
      const t = app.store.state.task;
      if (t) void app.openScan(t.scanId, 'overview');
    });
    this.dismiss = h('button', { type: 'button', class: 'task-dismiss', 'aria-label': '隐藏任务结果' }, icon('close'));
    this.dismiss.addEventListener('click', () => {
      const t = app.store.state.task;
      if (t) app.store.set({ task: { ...t, dismissed: true } });
    });
    this.el = h('div', { class: 'task-chip', hidden: true }, this.main, this.dismiss);
  }

  update(): void {
    const t = this.app.store.state.task;
    if (!t || (t.dismissed && isTerminal(t.state))) {
      this.el.hidden = true;
      return;
    }
    this.el.hidden = false;
    const running = !isTerminal(t.state);
    this.el.dataset.tone = stateTone(t.state);
    this.dismiss.hidden = running;
    const detail = running
      ? t.state === 'stopping' || t.cancelRequested
        ? '等待当前目录结束'
        : `已发现 ${groupDigits(t.entriesSeen)} 项`
      : t.error
        ? '未能开始'
        : '查看结果';
    replace(
      this.main,
      h('span', { class: running ? 'pulse' : 'dot', 'aria-hidden': 'true' }),
      h('span', { class: 'task-state' }, taskTitle(t.state)),
      h('span', { class: 'task-detail' }, detail),
    );
    this.main.setAttribute('aria-label', `扫描任务：${STATE_LABEL[t.state]}，${detail}。打开概览`);
  }
}
