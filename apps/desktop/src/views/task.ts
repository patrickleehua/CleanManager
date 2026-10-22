// Global task area: the scan and the operation hosted by this window, visible on every page.
// Counts update at the host's coalesced rate; screen readers hear only state changes
// (announced by the app).

import type { App } from '../main';

import { h, icon, live, replace } from '../lib/dom';
import { isTerminal, STATE_LABEL, stateTone, taskTitle } from '../lib/format';
import { OPERATION_STATE_LABEL, operationTone } from '../lib/words';
import { t } from '../i18n';

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
    this.dismiss = live(h('button', { type: 'button', class: 'task-dismiss' }, icon('close')), null, { 'aria-label': () => t('task.hideTaskResult') });
    this.dismiss.addEventListener('click', () => {
      const t = app.store.state.task;
      if (t) app.store.set({ task: { ...t, dismissed: true } });
    });
    this.el = h('div', { class: 'task-chip', hidden: true }, this.main, this.dismiss);
  }

  update(): void {
    const task = this.app.store.state.task;
    if (!task || (task.dismissed && isTerminal(task.state))) {
      this.el.hidden = true;
      return;
    }
    this.el.hidden = false;
    const running = !isTerminal(task.state);
    this.el.dataset.tone = stateTone(task.state);
    this.dismiss.hidden = running;
    const detail = running
      ? task.state === 'stopping' || task.cancelRequested
        ? t('task.waitingCurrentFolderFinish')
        : t('task.entriesseen', { entriesSeen: task.entriesSeen })
      : task.error
        ? t('task.couldntStart')
        : t('common.viewResult');
    replace(
      this.main,
      h('span', { class: running ? 'pulse' : 'dot', 'aria-hidden': 'true' }),
      h('span', { class: 'task-state' }, taskTitle(task.state)),
      h('span', { class: 'task-detail' }, detail),
    );
    this.main.setAttribute('aria-label', t('task.scanTaskPDetail', { p: STATE_LABEL[task.state], detail }));
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
    this.dismiss = live(h('button', { type: 'button', class: 'task-dismiss' }, icon('close')), null, { 'aria-label': () => t('task.hideRunResult') });
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
    const title = a.running ? (a.cancelRequested ? t('task.stoppingRun') : t('task.runningPlan')) : a.error ? t('task.runCouldntStart') : t('task.runP', { p: OPERATION_STATE_LABEL[state] ?? state });
    const detail = a.running ? t('task.doneTotal', { done, total: a.total }) : t('common.viewResult');
    replace(
      this.main,
      h('span', { class: a.running ? 'pulse' : 'dot', 'aria-hidden': 'true' }),
      h('span', { class: 'task-state' }, title),
      h('span', { class: 'task-detail num' }, detail),
    );
    this.main.setAttribute('aria-label', t('task.runTaskTitleDetail', { title, detail }));
  }
}
