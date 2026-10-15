// Global task area: the scan and the operation hosted by this window, visible on every page.
// Counts update at the host's coalesced rate; screen readers hear only state changes
// (announced by the app).

import type { App } from '../main';
import { groupDigits } from '../lib/bytes';
import { h, icon, replace } from '../lib/dom';
import { isTerminal, STATE_LABEL, stateTone, taskTitle } from '../lib/format';
import { OPERATION_STATE_LABEL, operationTone } from '../lib/words';

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

/** The plan application running in (or just finished by) this window. */
export class ApplyChip {
  el: HTMLElement;
  private main: HTMLButtonElement;
  private dismiss: HTMLButtonElement;

  constructor(private app: App) {
    this.main = h('button', { type: 'button', class: 'task-main' });
    this.main.addEventListener('click', () => {
      const a = app.store.state.apply;
      if (a) app.openOperation(a.operationId);
    });
    this.dismiss = h('button', { type: 'button', class: 'task-dismiss', 'aria-label': '隐藏执行结果' }, icon('close'));
    this.dismiss.addEventListener('click', () => {
      const a = app.store.state.apply;
      if (a) app.store.set({ apply: { ...a, dismissed: true } });
    });
    this.el = h('div', { class: 'task-chip', hidden: true }, this.main, this.dismiss);
  }

  update(): void {
    const a = this.app.store.state.apply;
    if (!a || (a.dismissed && !a.running)) {
      this.el.hidden = true;
      return;
    }
    this.el.hidden = false;
    const c = a.counts;
    const done = c.succeeded + c.skipped + c.failed + c.not_started + c.unknown;
    const state = a.running ? (a.cancelRequested ? 'stopping' : 'running') : (a.status ?? 'failed');
    this.el.dataset.tone = a.error ? 'bad' : operationTone(state);
    this.dismiss.hidden = a.running;
    const title = a.running ? (a.cancelRequested ? '正在停止执行' : '正在执行计划') : a.error ? '执行未能开始' : `执行${OPERATION_STATE_LABEL[state] ?? state}`;
    const detail = a.running ? `${groupDigits(done)} / ${groupDigits(a.total)} 项` : '查看结果';
    replace(
      this.main,
      h('span', { class: a.running ? 'pulse' : 'dot', 'aria-hidden': 'true' }),
      h('span', { class: 'task-state' }, title),
      h('span', { class: 'task-detail num' }, detail),
    );
    this.main.setAttribute('aria-label', `执行任务：${title}，${detail}。打开执行结果`);
  }
}
