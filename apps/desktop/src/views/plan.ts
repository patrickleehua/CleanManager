// P05 — plan review and authorization (GUI-03/04, PLN-01..07). Shows the complete stored
// plan: mode and what it does to space and recovery, every item (paged, never a top-N
// summary), rule versions, digest, expiry, what is not included, and where authorization
// comes from. Approval is the person typing the digest's first 8 characters; it writes the
// same receipt the CLI's `plan review` writes. The same plan id works in the CLI.

import type { App } from '../main';
import { toIpcError } from '../lib/api';
import { formatBytes, formatExactBytes, groupDigits, parseBytes } from '../lib/bytes';
import { h, keepFocus, replace } from '../lib/dom';
import { formatTime } from '../lib/format';
import type { IpcError, PlanDetail, PlanItem } from '../lib/types';
import {
  AUTH_LABEL,
  MODE_EXPLAIN,
  MODE_LABEL,
  OPERATION_STATE_LABEL,
  PLAN_KIND_LABEL,
  POLICY_MISMATCH_LABEL,
  POLICY_STATE_LABEL,
  RECOVERY_EXPLAIN,
  STALE_LABEL,
  digestGroups,
  formatCountdown,
  normalizeCode,
} from '../lib/words';
import { copyField, emptyState, errorLine, pageHead, pill, type Tone } from './parts';

const ITEM_PAGE = 100;

export class PlanPage {
  el: HTMLElement;
  private body: HTMLElement;
  private planId: string | null = null;
  private detail: PlanDetail | null = null;
  private error: IpcError | null = null;
  private items: PlanItem[] = [];
  private itemsNext: string | null = null;
  private itemsTotal = 0;
  private itemsLoading = false;
  private itemsError: IpcError | null = null;
  private typed = '';
  private busy = false;
  private actionError: IpcError | null = null;
  private policyNote: string | null = null;
  private timer: number | null = null;
  private loadedAt = 0;

  constructor(private app: App) {
    this.body = h('div', { class: 'plan-body' });
    this.el = h('section', { class: 'page page-plan', 'aria-labelledby': 'plan-title' }, this.body);
  }

  activate(): void {
    const id = this.app.store.state.planId;
    if (id) void this.load(id, true);
    if (this.timer === null) this.timer = window.setInterval(() => this.tick(), 1000);
  }

  update(): void {
    const id = this.app.store.state.planId;
    if (id !== this.planId && id) void this.load(id, true);
    if (this.app.store.state.page !== 'plan' && this.timer !== null) {
      window.clearInterval(this.timer);
      this.timer = null;
    }
    keepFocus(this.el, () => this.render());
  }

  private tick(): void {
    if (this.app.store.state.page !== 'plan' || !this.detail) return;
    const left = this.secondsLeft();
    const el = this.el.querySelector('[data-countdown]');
    if (el) el.textContent = left > 0 ? `剩余 ${formatCountdown(left)}` : '已过期';
    if (left === 0 && !this.detail.expired && this.planId) void this.load(this.planId, false);
  }

  private secondsLeft(): number {
    if (!this.detail) return 0;
    return Math.max(0, this.detail.seconds_left - Math.floor((Date.now() - this.loadedAt) / 1000));
  }

  private async load(planId: string, resetItems: boolean): Promise<void> {
    const switching = planId !== this.planId;
    this.planId = planId;
    if (switching) {
      this.detail = null;
      this.typed = '';
      this.actionError = null;
      this.policyNote = null;
    }
    this.error = null;
    try {
      const d = await this.app.backend.planDetail(planId);
      if (this.planId !== planId) return;
      this.detail = d;
      this.loadedAt = Date.now();
    } catch (e) {
      if (this.planId === planId) this.error = toIpcError(e);
    }
    if (resetItems) await this.loadItems(false);
    this.update();
  }

  private async loadItems(more: boolean): Promise<void> {
    const id = this.planId;
    if (!id || this.itemsLoading) return;
    this.itemsLoading = true;
    this.itemsError = null;
    if (!more) {
      this.items = [];
      this.itemsNext = null;
    }
    this.update();
    try {
      const page = await this.app.backend.planItems(id, ITEM_PAGE, more ? this.itemsNext : null);
      if (this.planId !== id) return;
      this.items = more ? [...this.items, ...page.items] : page.items;
      this.itemsNext = page.next_cursor;
      this.itemsTotal = page.total;
    } catch (e) {
      this.itemsError = toIpcError(e);
    } finally {
      this.itemsLoading = false;
      this.update();
    }
  }

  private async approve(): Promise<void> {
    const d = this.detail;
    if (!d || !d.review_token || this.busy) return;
    this.busy = true;
    this.actionError = null;
    this.update();
    try {
      await this.app.backend.approvePlan(d.plan_id, d.review_token, normalizeCode(this.typed));
      this.typed = '';
      this.app.announce('已批准这份计划。它还没有执行');
      await this.load(d.plan_id, false);
      // Permanent actions never take the default focus; the safe path stays first.
      const permanent = d.mode === 'delete' || d.kind === 'purge';
      requestAnimationFrame(() => document.getElementById(permanent ? 'auth-title' : 'plan-apply')?.focus());
    } catch (e) {
      this.actionError = toIpcError(e);
      if (this.actionError.reason === 'review_expired' || this.actionError.reason === 'plan_changed') await this.load(d.plan_id, false);
    } finally {
      this.busy = false;
      this.update();
    }
  }

  private async revoke(): Promise<void> {
    const d = this.detail;
    if (!d || this.busy) return;
    this.busy = true;
    try {
      await this.app.backend.revokePlan(d.plan_id);
      this.app.announce('已撤销批准');
      await this.load(d.plan_id, false);
    } catch (e) {
      this.actionError = toIpcError(e);
    } finally {
      this.busy = false;
      this.update();
    }
  }

  private async apply(): Promise<void> {
    const d = this.detail;
    if (!d || this.busy) return;
    this.busy = true;
    this.actionError = null;
    this.update();
    try {
      await this.app.applyPlan(d.plan_id);
    } catch (e) {
      this.actionError = toIpcError(e);
    } finally {
      this.busy = false;
      this.update();
    }
  }

  private async policyFromPlan(): Promise<void> {
    const d = this.detail;
    if (!d) return;
    try {
      const list = await this.app.backend.createPolicyFromPlan(d.plan_id);
      const p = list[list.length - 1];
      this.policyNote = `已创建暂停状态的策略 ${p?.policy_id ?? ''}。它不会自动生效；在设置中输入策略编号才能启用。`;
    } catch (e) {
      this.policyNote = null;
      this.actionError = toIpcError(e);
    }
    this.update();
  }

  // ---- rendering ---------------------------------------------------------------------------

  private back(): { label: string; onClick: () => void } {
    const from = this.app.store.state.returnTo;
    if (from === 'activity') return { label: '返回活动', onClick: () => this.app.navigate('activity') };
    return { label: '返回清理候选', onClick: () => this.app.navigate('cleanup') };
  }

  private render(): void {
    if (!this.planId) {
      replace(this.body, h('h1', { id: 'plan-title' }, '计划'), emptyState('没有打开的计划', '在清理页选择候选并生成计划，或在活动记录中打开一份已有计划。', h('button', { type: 'button', class: 'primary', onclick: () => this.app.navigate('cleanup') }, '前往清理')));
      return;
    }
    if (this.error && !this.detail) {
      replace(this.body, pageHead({ id: 'plan-title', title: '计划', back: this.back() }), h('div', { class: 'panel' }, errorLine(this.error)));
      return;
    }
    const d = this.detail;
    if (!d) {
      replace(this.body, pageHead({ id: 'plan-title', title: '审阅计划', back: this.back() }), h('p', { class: 'muted', role: 'status' }, '正在读取计划…'));
      return;
    }
    const tone: Tone = d.authorization.status === 'authorized' ? 'good' : d.authorization.status === 'required' ? 'busy' : 'muted';
    const left = this.secondsLeft();
    replace(
      this.body,
      pageHead({
        id: 'plan-title',
        title: `审阅${PLAN_KIND_LABEL[d.kind]}`,
        back: this.back(),
        meta: [
          h('code', { class: 'selectable' }, d.plan_id),
          pill(d.operation ? `已执行：${OPERATION_STATE_LABEL[d.operation.state] ?? d.operation.state}` : AUTH_LABEL[d.authorization.status], d.operation ? 'muted' : tone),
          `冻结于 ${formatTime(d.created_at)}`,
          h('span', { 'data-countdown': '' }, left > 0 ? `剩余 ${formatCountdown(left)}` : '已过期'),
        ],
      }),
      this.notices(d),
      h('div', { class: 'work-layout' }, h('div', { class: 'work-main' }, this.ledger(d), this.itemsPanel(d)), this.authPanel(d)),
    );
  }

  private notices(d: PlanDetail): HTMLElement | null {
    const parts: HTMLElement[] = [];
    if (d.operation) {
      parts.push(
        h(
          'div',
          { class: 'notice', role: 'status' },
          h('span', {}, '这份计划已经执行过。同一份计划不会再次执行；需要处理剩余对象时，请重新分析并生成新计划。'),
          h('button', { type: 'button', class: 'link', onclick: () => this.app.openOperation(d.operation!.operation_id, 'plan') }, '查看执行结果'),
        ),
      );
    } else if (d.stale_reason) {
      parts.push(h('div', { class: 'notice warn', role: 'status' }, h('span', {}, STALE_LABEL[d.stale_reason] ?? d.stale_reason)));
    }
    return parts.length ? h('div', { class: 'notices' }, ...parts) : null;
  }

  private statement(d: PlanDetail): HTMLElement {
    const n = groupDigits(d.item_count);
    const explain = d.kind === 'cleanup' && d.mode ? MODE_EXPLAIN[d.mode] : d.kind === 'cleanup' ? null : RECOVERY_EXPLAIN[d.kind];
    const verb = d.kind === 'cleanup' && d.mode ? `${MODE_LABEL[d.mode]}：${n} 个文件` : d.kind === 'restore' ? `恢复到原位置：${n} 个对象` : `永久清空：${n} 个隔离对象`;
    const kind = d.kind === 'cleanup' ? (d.mode ?? 'trash') : d.kind === 'purge' ? 'delete' : 'restore';
    return h(
      'div',
      { class: 'statement', 'data-mode': kind },
      h('p', { class: 'statement-verb' }, verb),
      explain
        ? h(
            'dl',
            { class: 'statement-lines' },
            h('dt', {}, '做什么'),
            h('dd', {}, explain.effect),
            h('dt', {}, '空间'),
            h('dd', { class: kind === 'quarantine' ? 'strong-note' : '' }, explain.space),
            h('dt', {}, '恢复'),
            h('dd', {}, explain.recovery),
          )
        : null,
    );
  }

  private ledger(d: PlanDetail): HTMLElement {
    const total = parseBytes(d.total_logical_bytes);
    const facts: (HTMLElement | null)[] = [];
    const fact = (k: string, ...v: (Node | string | null)[]) => facts.push(h('dt', {}, k), h('dd', {}, ...v));
    if (d.kind === 'cleanup') {
      fact('来源扫描', h('span', { class: 'path' }, d.scan_root_display ?? '（范围不可用）'), h('small', {}, `扫描 ${d.scan_id ?? '—'}，版本 ${d.scan_revision ?? '—'}`));
      fact('规则版本', d.rule_refs.length ? h('span', { class: 'rule-list' }, ...d.rule_refs.map((r) => h('code', { class: 'selectable' }, r))) : '—');
    }
    fact('对象', `${groupDigits(d.item_count)} 项`, d.kind === 'cleanup' ? h('small', {}, `其中 ${groupDigits(d.default_selected_items)} 项来自低风险规则`) : null);
    fact('已知逻辑大小', total === null ? '未知' : formatBytes(total), h('small', {}, total === null ? '计划中不允许大小未知的文件' : formatExactBytes(total)));
    if (d.allocated_known_bytes !== null)
      fact('分配大小', d.allocated_unknown_count ? `至少 ${formatBytes(d.allocated_known_bytes)}，${d.allocated_unknown_count} 项未知` : formatBytes(d.allocated_known_bytes));
    fact('冻结于', formatTime(d.created_at));
    fact('有效至', formatTime(d.expires_at), h('small', {}, '计划有效 15 分钟；过期后需要重新生成，不会在旧计划上修补'));
    return h(
      'section',
      { class: 'panel ledger', 'aria-labelledby': 'ledger-title' },
      h('h2', { id: 'ledger-title' }, '这份计划会做什么'),
      this.statement(d),
      h('dl', { class: 'fact-grid' }, ...facts),
      this.notIncluded(d),
    );
  }

  private notIncluded(d: PlanDetail): HTMLElement {
    const lines: string[] = [];
    if (d.kind === 'cleanup') {
      lines.push('文件夹本身不在清单中，只处理冻结时观察到的文件；计划之后新出现的文件不会被处理。');
      const n = d.not_included;
      if (n) {
        if (n.low_unselected || n.review_unselected) lines.push(`你没有选择的候选：低风险 ${n.low_unselected} 个，需审阅 ${n.review_unselected} 个。`);
        if (n.pending) lines.push(`待判断的候选 ${n.pending} 个（证据不完整）。`);
        if (n.blocked) lines.push(`受保护或被拒绝的候选 ${n.blocked} 个。`);
      } else {
        lines.push('来源候选集已被新的分析替换，无法逐类对照未包含的候选；清单本身不受影响。');
      }
      lines.push('执行时逐项复查：已变化、正被占用、所属应用在运行或已受保护的对象会被跳过，不会替换成别的文件。');
    } else {
      lines.push('只包含所选的隔离项；隔离区中的其他对象不受影响。');
      lines.push(d.kind === 'restore' ? '原位置已有同名对象时跳过，不覆盖、不改名到其他位置。' : '清空不会恢复任何对象；需要恢复的项目请另建恢复计划。');
    }
    return h('div', { class: 'not-included' }, h('h3', {}, '不包含的内容'), h('ul', {}, ...lines.map((l) => h('li', {}, l))));
  }

  private itemsPanel(d: PlanDetail): HTMLElement {
    const recovery = d.kind !== 'cleanup';
    const rows = this.items.map((i) =>
      h(
        'tr',
        {},
        h('td', { class: 'col-id' }, h('code', {}, i.action_id)),
        h(
          'td',
          { class: 'col-object' },
          h('span', { class: 'obj-name' }, i.name, i.item_kind === 'directory' ? h('span', { class: 'muted small' }, '（空文件夹，内容成功后才移除）') : null),
          h('span', { class: 'obj-path path' }, recovery ? `回到 ${i.original_path_display ?? '—'}` : i.path_display),
        ),
        h('td', { class: 'col-size num', title: formatExactBytes(parseBytes(i.logical_bytes)) }, formatBytes(i.logical_bytes)),
      ),
    );
    return h(
      'section',
      { class: 'panel items-panel', 'aria-labelledby': 'items-title' },
      h('h2', { id: 'items-title' }, `完整清单（${groupDigits(d.item_count)} 项）`),
      h('p', { class: 'muted small' }, '按计划顺序列出每一个动作。这里就是将被处理的全部对象，没有隐藏项。'),
      this.items.length
        ? h(
            'div',
            { class: 'table-wrap' },
            h(
              'table',
              { class: 'data-table' },
              h('caption', { class: 'sr-only' }, '计划中的全部动作'),
              h('thead', {}, h('tr', {}, h('th', { scope: 'col', class: 'col-id' }, '动作'), h('th', { scope: 'col' }, '对象'), h('th', { scope: 'col', class: 'col-size' }, '逻辑大小'))),
              h('tbody', {}, ...rows),
            ),
          )
        : h('p', { class: 'muted small', role: 'status' }, this.itemsLoading ? '正在加载清单…' : '清单为空。'),
      errorLine(this.itemsError),
      this.itemsNext
        ? h('button', { type: 'button', class: 'quiet more', id: 'plan-items-more', disabled: this.itemsLoading, onclick: () => void this.loadItems(true) }, this.itemsLoading ? '正在加载…' : `显示更多（已显示 ${this.items.length} / ${this.itemsTotal}）`)
        : null,
    );
  }

  private authPanel(d: PlanDetail): HTMLElement {
    const a = d.authorization;
    const danger = d.mode === 'delete' || d.kind === 'purge';
    const blocks: (HTMLElement | null)[] = [];
    blocks.push(
      h(
        'div',
        { class: 'auth-state' },
        pill(AUTH_LABEL[a.status], a.status === 'authorized' ? 'good' : a.status === 'required' ? 'busy' : 'muted'),
        a.status === 'authorized'
          ? h('p', { class: 'small' }, a.source === 'policy' ? `来源：策略 ${a.policy_id}（版本 ${a.policy_version}）` : `来源：人工审阅（${a.principal ?? ''}）于 ${formatTime(a.authorized_at)}`)
          : null,
      ),
    );
    if (d.operation) {
      blocks.push(h('button', { type: 'button', class: 'primary wide', onclick: () => this.app.openOperation(d.operation!.operation_id, 'plan') }, '查看执行结果'));
    } else if (a.status === 'required' && d.review_token) {
      blocks.push(this.seal(d, danger));
    } else if (a.status === 'authorized' && !d.stale_reason) {
      const label = d.kind === 'cleanup' && d.mode ? `执行：${MODE_LABEL[d.mode]}（${groupDigits(d.item_count)} 项）` : d.kind === 'restore' ? `执行：恢复 ${groupDigits(d.item_count)} 项` : `执行：永久清空 ${groupDigits(d.item_count)} 项`;
      const apply = h('button', { type: 'button', id: 'plan-apply', class: `${danger ? 'danger' : 'primary'} wide`, disabled: this.busy }, label);
      apply.addEventListener('click', () => void this.apply());
      const revoke = h('button', { type: 'button', class: 'quiet', id: 'plan-revoke', disabled: this.busy }, '撤销批准');
      revoke.addEventListener('click', () => void this.revoke());
      blocks.push(
        apply,
        h('p', { class: 'muted small' }, d.mode === 'quarantine' ? '此操作不立即释放空间。执行中可以停止后续操作；已完成的项目不会自动撤销。' : danger ? '永久删除无法撤销。执行中可以停止后续操作；已删除的不会恢复。' : '执行中可以停止后续操作；已完成的项目不会自动撤销。'),
        revoke,
      );
    } else {
      blocks.push(
        h('p', { class: 'small' }, a.status === 'revoked' ? '批准已撤销，这份计划不能执行。' : d.stale_reason ? (STALE_LABEL[d.stale_reason] ?? d.stale_reason) : '这份计划不能再批准。'),
        h('button', { type: 'button', class: 'wide', onclick: () => this.app.navigate(d.kind === 'cleanup' ? 'cleanup' : 'activity') }, d.kind === 'cleanup' ? '回到清理候选' : '回到隔离区'),
      );
    }
    blocks.push(errorLine(this.actionError));
    return h(
      'aside',
      { class: 'panel work-aside auth-panel', 'aria-labelledby': 'auth-title' },
      h('h2', { id: 'auth-title', tabindex: '-1' }, '授权'),
      ...blocks,
      d.kind === 'cleanup' ? this.policies(d) : h('p', { class: 'muted small' }, '恢复与清空计划只能由人工审阅授权，任何策略都不会匹配它们。'),
      this.handoff(d),
    );
  }

  /** The digest as the plan's seal; approving means typing its first group. */
  private seal(d: PlanDetail, danger: boolean): HTMLElement {
    const groups = digestGroups(d.digest);
    const input = h('input', {
      id: 'plan-code',
      class: 'code-input',
      type: 'text',
      inputmode: 'latin',
      autocomplete: 'off',
      autocapitalize: 'off',
      spellcheck: 'false',
      maxlength: '9',
      placeholder: '········',
      'aria-describedby': 'seal-help',
    }) as HTMLInputElement;
    input.value = this.typed;
    const approve = h('button', { type: 'button', id: 'plan-approve', class: `${danger ? 'danger' : 'primary'} wide`, disabled: normalizeCode(this.typed) !== d.digest_prefix || this.busy }, this.busy ? '正在批准…' : '批准这份计划') as HTMLButtonElement;
    input.addEventListener('input', () => {
      this.typed = input.value;
      approve.disabled = normalizeCode(this.typed) !== d.digest_prefix || this.busy;
      const ok = normalizeCode(this.typed) === d.digest_prefix;
      input.classList.toggle('match', ok);
    });
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && !approve.disabled) void this.approve();
    });
    approve.addEventListener('click', () => void this.approve());
    return h(
      'div',
      { class: 'seal' },
      h('p', { class: 'seal-label' }, '计划摘要'),
      h('p', { class: 'seal-digest', 'aria-label': `计划摘要，前 8 位是 ${d.digest_prefix.split('').join(' ')}` }, ...groups.map((g, i) => h('span', { class: i === 0 ? 'grp first' : 'grp' }, g))),
      h('label', { for: 'plan-code', class: 'seal-ask' }, '输入摘要的前 8 位，批准这份确切的计划'),
      input,
      h('p', { id: 'seal-help', class: 'muted small' }, `摘要绑定清单、模式和有效期；内容有任何变化都会得到不同的摘要。批准只授权这份计划，${d.mode === 'delete' ? '永久删除无法撤销。' : '执行仍需你在下一步点击。'}`),
      approve,
    );
  }

  private policies(d: PlanDetail): HTMLElement {
    const rows = d.policies.map((p) =>
      h(
        'li',
        {},
        h('code', { class: 'selectable' }, p.policy_id),
        h('span', { class: 'small' }, `${POLICY_STATE_LABEL[p.state]}，${p.covers ? '覆盖这份计划（仍按额度检查）' : `不覆盖：${POLICY_MISMATCH_LABEL[p.reason ?? ''] ?? p.reason ?? ''}`}`),
      ),
    );
    const create = h('button', { type: 'button', class: 'quiet', id: 'policy-from-plan' }, '为同类计划创建有限策略');
    create.addEventListener('click', () => void this.policyFromPlan());
    return h(
      'div',
      { class: 'aside-section' },
      h('h3', {}, '预授权策略'),
      rows.length ? h('ul', { class: 'policy-mini' }, ...rows) : h('p', { class: 'muted small' }, '没有策略。默认所有计划都需要人工批准。'),
      this.policyNote ? h('p', { class: 'small', role: 'status' }, this.policyNote) : null,
      d.default_selected_items === d.item_count ? create : h('p', { class: 'muted small' }, '策略只覆盖全部来自低风险规则的计划，这份计划不适用。'),
    );
  }

  private handoff(d: PlanDetail): HTMLElement {
    return h(
      'div',
      { class: 'aside-section' },
      h('h3', {}, '在终端中使用同一份计划'),
      h('p', { class: 'muted small' }, 'CLI 读取同一个计划编号与摘要；在任一入口批准都对另一入口有效。'),
      copyField('查看', d.cli.show),
      copyField('审阅并批准', d.cli.review),
      copyField('执行', d.cli.apply),
      h('p', { class: 'muted small' }, '自动化调用不能批准计划，只能等待人工批准或已启用的策略。'),
    );
  }
}
