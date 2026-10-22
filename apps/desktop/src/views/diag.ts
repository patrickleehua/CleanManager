// Optional AI diagnosis workbench (AID-01..13). One object at a time: the person previews what
// would be sent, confirms, and reads a conditional explanation whose every claim links to the
// local evidence it cites. Recommendations come back already checked by the core; next steps
// are the core's (plan with this one candidate, protect, locate, app page, checklist). Nothing
// here deletes, approves or runs a command. Navigation never cancels a run.

import type { App } from '../main';
import { toIpcError } from '../lib/api';
import { formatBytes } from '../lib/bytes';
import { h, icon, keepFocus, replace } from '../lib/dom';
import { formatTime } from '../lib/format';
import { RequestGate } from '../lib/gate';
import type { Diagnosis, DiagStatus, DiagSummary, DiagTarget, Disclosure, Handoff, Preview, Recommendation } from '../lib/diagTypes';
import type { IpcError } from '../lib/types';
import {
  ACTION_BUTTON,
  ACTION_LABEL,
  CLAIM_LABEL,
  FAILURE_HINT,
  LAYER_LABEL,
  PROVIDER_LABEL,
  REC_EXPLAIN,
  REC_LABEL,
  STATE_LABEL,


  stateTone,
} from '../lib/diagWords';
import { emptyState, errorText, pageHead, pill } from './parts';
import { openPreview } from './diagPreview';
import { techBlock } from './ui';
import { words } from '../lib/platform';

const ORIGIN_LABEL: Record<string, string> = { cleanup: '清理', disk: '磁盘', apps: '应用' };
const CLAIM_TONE: Record<string, 'good' | 'warn' | 'bad' | 'busy' | 'muted'> = {
  supported: 'good',
  user_statement: 'busy',
  inference: 'muted',
  unsupported: 'warn',
  conflicts_with_facts: 'bad',
};

/** Recommendation marker: shape + word, never colour alone. */
export function recMark(r: Recommendation): HTMLElement {
  return h('span', { class: 'rmark', 'data-rec': r }, h('span', { class: 'rmark-shape', 'aria-hidden': 'true' }), h('span', {}, REC_LABEL[r]));
}

export class DiagPage {
  el: HTMLElement;
  private body: HTMLElement;
  private status: DiagStatus | null = null;
  private statusError: IpcError | null = null;
  private diag: Diagnosis | null = null;
  private diagError: IpcError | null = null;
  private history: DiagSummary[] | null = null;
  private busy = false;
  private message: { text: string; tone: 'good' | 'warn' } | null = null;
  private question = '';
  private others = new Map<string, string>();
  private planMode: 'trash' | 'quarantine' = 'trash';
  private showChecklist = false;
  private exportText: string | null = null;
  private boundKey = '';
  private gate = new RequestGate();

  constructor(private app: App) {
    this.body = h('div', { class: 'diag-body' });
    this.el = h('section', { class: 'page page-diag', 'aria-labelledby': 'diag-title' }, this.body);
  }

  activate(): void {
    void this.reloadStatus();
    void this.reloadHistory();
    this.bind(true);
  }

  update(): void {
    this.bind(false);
    keepFocus(this.el, () => this.render());
  }

  /** A run finished (any page may be showing). */
  async finished(id: string): Promise<void> {
    void this.reloadHistory();
    if (this.diag?.diagnosis_id === id) await this.loadDiag(id);
  }

  private key(): string {
    const s = this.app.store.state;
    return `${s.diagTarget?.scanId ?? ''}/${s.diagTarget?.entryId ?? ''}/${s.diagId ?? ''}`;
  }

  /** New target or record: drop what belongs to the previous one (late answers never apply). */
  private bind(force: boolean): void {
    const k = this.key();
    if (!force && k === this.boundKey) return;
    this.boundKey = k;
    this.gate.next();
    this.diag = null;
    this.diagError = null;
    this.message = null;
    this.exportText = null;
    this.showChecklist = false;
    this.others.clear();
    const id = this.app.store.state.diagId;
    if (id) void this.loadDiag(id);
  }

  private async reloadStatus(): Promise<void> {
    try {
      this.status = await this.app.backend.diagStatus();
      this.statusError = null;
    } catch (e) {
      this.statusError = toIpcError(e);
    }
    this.update();
  }

  private async reloadHistory(): Promise<void> {
    try {
      this.history = (await this.app.backend.diagList(null)).items;
    } catch {
      this.history = [];
    }
    this.update();
  }

  private async loadDiag(id: string): Promise<void> {
    const ticket = this.gate.current();
    try {
      const r = await this.gate.run(ticket, this.app.backend.diagGet(id));
      if (r.stale) return;
      this.diag = r.value;
      this.diagError = null;
    } catch (e) {
      this.diagError = toIpcError(e);
    }
    this.update();
  }

  // ---- actions ---------------------------------------------------------------------------

  private async start(): Promise<void> {
    const t = this.app.store.state.diagTarget;
    if (!t) return;
    const q = this.question.trim() || null;
    const prepare = (d: Disclosure) => this.app.backend.diagPrepare(t.scanId, t.entryId, t.origin, null, d, q);
    await this.runPreview(prepare, true, '确认要发送的内容');
  }

  private async followUp(questionId: string | null, answer: string): Promise<void> {
    const d = this.diag;
    if (!d || !answer.trim()) return;
    const prepare = () => this.app.backend.diagFollowUp(d.diagnosis_id, questionId, answer.trim());
    await this.runPreview(prepare, false, '确认补充后要发送的内容');
  }

  private async runPreview(prepare: (d: Disclosure) => Promise<Preview>, editable: boolean, title: string): Promise<void> {
    this.busy = true;
    this.message = null;
    this.update();
    let first: Preview;
    try {
      first = await prepare({ full_path: false, child_summary: false });
    } catch (e) {
      this.busy = false;
      this.diagError = toIpcError(e);
      this.update();
      return;
    }
    this.busy = false;
    this.update();
    const d = await openPreview(this.app, { prepare, scopeEditable: editable, title }, first);
    if (!d) {
      this.app.announce('已取消，没有发送任何内容');
      this.message = { text: '已取消：没有发送任何内容，文件保持原状。', tone: 'good' };
      this.update();
      return;
    }
    this.app.announce('已发送，正在诊断');
    // The new record becomes the page's subject (its id survives navigation).
    this.app.store.set({ diagId: d.diagnosis_id });
    this.diag = d;
    void this.reloadHistory();
    this.update();
  }

  private async cancel(): Promise<void> {
    if (!this.diag) return;
    try {
      this.diag = await this.app.backend.diagCancel(this.diag.diagnosis_id);
      this.app.announce('已取消诊断');
    } catch (e) {
      this.diagError = toIpcError(e);
    }
    void this.reloadHistory();
    this.update();
  }

  private async handoff(h0: Handoff): Promise<void> {
    const d = this.diag;
    if (!d) return;
    if (h0.action === 'verify') {
      this.showChecklist = !this.showChecklist;
      this.update();
      return;
    }
    if (h0.action === 'keep') {
      this.message = { text: '好的，保留。CleanManager 不会对它做任何操作。', tone: 'good' };
      this.app.announce('保留，不做任何操作');
      this.update();
      return;
    }
    this.busy = true;
    this.update();
    try {
      const r = await this.app.backend.diagHandoff(d.diagnosis_id, h0.action, h0.action === 'review_plan' ? this.planMode : null);
      if (r.outcome === 'plan' && r.plan_id) {
        this.app.announce('已生成只含这一项的计划，请在计划页审阅');
        this.app.openPlan(r.plan_id, 'diag');
      } else if (r.outcome === 'protected') {
        this.message = { text: '已加入保护名单：以后的规则与策略都不会选中它。', tone: 'good' };
        this.app.announce('已加入保护名单');
      } else if (r.outcome === 'disk' && r.entry_id) {
        this.app.store.set({ revealEntry: r.entry_id });
        this.app.navigate('disk');
      } else if (r.outcome === 'apps') {
        this.app.navigate('apps');
      }
    } catch (e) {
      this.message = { text: errorText(toIpcError(e)), tone: 'warn' };
    } finally {
      this.busy = false;
      this.update();
    }
  }

  private async save(): Promise<void> {
    if (!this.diag) return;
    this.diag = await this.app.backend.diagSave(this.diag.diagnosis_id);
    this.message = { text: '已保存这条诊断；在你移除之前会一直保留在本机。', tone: 'good' };
    void this.reloadHistory();
    this.update();
  }

  private async remove(): Promise<void> {
    if (!this.diag) return;
    await this.app.backend.diagRemove(this.diag.diagnosis_id);
    this.app.announce('已移除这条诊断记录；文件操作记录不受影响');
    this.app.store.set({ diagId: null });
    void this.reloadHistory();
  }

  private async exportPreview(): Promise<void> {
    if (!this.diag) return;
    this.exportText = this.exportText ? null : await this.app.backend.diagExport(this.diag.diagnosis_id);
    this.update();
  }

  // ---- rendering -------------------------------------------------------------------------

  private render(): void {
    const s = this.app.store.state;
    const back = s.returnTo && s.returnTo !== 'diag' ? { label: `返回${({ disk: '磁盘', cleanup: '清理', apps: '应用', settings: '设置', plan: '计划' } as Record<string, string>)[s.returnTo] ?? ''}`, onClick: () => this.app.navigate(s.returnTo!) } : undefined;
    const t = s.diagTarget;
    const d = this.diag;
    const subject = d?.evidence.subject;
    const name = subject?.name ?? t?.name ?? null;
    const head = pageHead({
      id: 'diag-title',
      title: 'AI 诊断',
      back,
      meta: name
        ? [
            h('b', { class: 'diag-subject' }, name),
            subject ? formatBytes(subject.logical_bytes) : null,
            `来自${ORIGIN_LABEL[d?.request.object.origin ?? t?.origin ?? 'disk']}`,
            d ? pill(STATE_LABEL[d.state], stateTone(d.state)) : null,
          ]
        : ['解释单个文件或文件夹的用途与删除后果；可选扩展，不影响本机功能'],
    });
    const notices = h(
      'div',
      { class: 'notices' },
      this.message ? h('div', { class: `notice${this.message.tone === 'warn' ? ' warn' : ''}`, role: 'status' }, h('span', {}, this.message.text)) : null,
      this.diagError ? h('div', { class: 'notice warn', role: 'alert' }, h('span', {}, errorText(this.diagError))) : null,
    );
    if (!t && !s.diagId) {
      replace(this.body, head, notices, this.historyPage());
      return;
    }
    if (this.status && !this.status.configured && !d) {
      replace(this.body, head, notices, this.unconfigured());
      return;
    }
    replace(
      this.body,
      head,
      notices,
      h('div', { class: 'work-layout diag-layout' }, h('div', { class: 'work-main' }, ...this.mainColumn()), this.aside()),
    );
  }

  private unconfigured(): HTMLElement {
    return h(
      'div',
      { class: 'panel' },
      emptyState(
        '还没有配置 AI 诊断',
        '这是可选扩展：在设置里填写你自己的 API（含本机端点）或选择已安装的 CLI 后才能使用。扫描、规则说明、计划、执行与恢复都不依赖它。',
        h('button', { type: 'button', class: 'primary', id: 'diag-configure', onclick: () => this.app.navigate('settings') }, '去设置 AI 诊断'),
      ),
      this.statusError ? h('p', { class: 'form-error' }, errorText(this.statusError)) : null,
    );
  }

  private mainColumn(): (HTMLElement | null)[] {
    const d = this.diag;
    if (!d) return [this.intro()];
    if (d.state === 'running') return [this.running(d)];
    if (d.state === 'failed' || d.state === 'cancelled') return [this.stopped(d)];
    return [this.verdict(d), this.explanation(d), this.ledger(d), this.questions(d), this.nextSteps(d)];
  }

  private intro(): HTMLElement {
    const ta = h('textarea', { id: 'diag-question', class: 'diag-question', rows: 2, maxlength: 280, placeholder: '例如：这个文件还能删吗？删了会影响什么？' }) as HTMLTextAreaElement;
    ta.value = this.question;
    ta.addEventListener('input', () => (this.question = ta.value));
    const go = h('button', { type: 'button', class: 'primary', id: 'diag-start', disabled: this.busy }, '预览将发送的内容');
    go.addEventListener('click', () => void this.start());
    return h(
      'div',
      { class: 'panel diag-intro' },
      h('h2', {}, '先看清楚会发送什么，再决定'),
      h(
        'ol',
        { class: 'diag-steps' },
        h('li', {}, h('b', {}, '预览'), h('span', {}, '只含这个对象的脱敏元数据与本机证据；不读文件内容，不发送整盘目录。')),
        h('li', {}, h('b', {}, '诊断'), h('span', {}, '只发送到你配置的提供方，失败时不会改用其他服务。')),
        h('li', {}, h('b', {}, '你决定'), h('span', {}, '回答会先经过本机事实核对；任何处理都走原有计划审阅与授权。')),
      ),
      h('label', { for: 'diag-question', class: 'diag-label' }, '你想问什么（可选）'),
      ta,
      h('div', { class: 'card-actions' }, go),
      h('p', { class: 'small muted' }, '这里的 AI 诊断解释具体对象；清理页的「AI 与开发者」类目管理的是 AI 工具自己的缓存与会话，两者无关。'),
    );
  }

  private running(d: Diagnosis): HTMLElement {
    const stop = h('button', { type: 'button', id: 'diag-cancel' }, '取消诊断');
    stop.addEventListener('click', () => void this.cancel());
    return h(
      'div',
      { class: 'panel progress-panel', role: 'status' },
      h('h2', {}, `正在等待 ${d.destination.label} 的回答`),
      h('div', { class: 'indeterminate', 'aria-hidden': 'true' }),
      h('p', {}, '离开这个页面不会取消诊断；完成后会提示你。取消后迟到的回答会被丢弃。'),
      h('div', { class: 'card-actions' }, stop),
    );
  }

  private stopped(d: Diagnosis): HTMLElement {
    const retry = h('button', { type: 'button', class: 'primary', id: 'diag-retry', disabled: this.busy }, '重新预览并诊断');
    retry.addEventListener('click', () => void this.start());
    const failed = d.state === 'failed';
    return h(
      'div',
      { class: 'panel diag-stopped' },
      h('h2', {}, failed ? '诊断失败' : '诊断已取消'),
      failed && d.failure
        ? h('div', {}, h('p', {}, h('b', {}, FAILURE_HINT[d.failure.code] ?? '诊断失败。')), d.failure.message ? techBlock(h('p', { class: 'selectable small' }, h('code', {}, d.failure.code), ` ${d.failure.message}`)) : null)
        : h('p', {}, '没有采用任何回答；文件保持原状。'),
      h('p', { class: 'small muted' }, '右侧的本机依据仍然有效。没有尝试其他提供方或模型。'),
      failed && d.failure?.code && ['auth_failed', 'credential_missing', 'model_unavailable', 'unsupported_mode'].includes(d.failure.code)
        ? h('div', { class: 'card-actions' }, h('button', { type: 'button', onclick: () => this.app.navigate('settings') }, '检查 AI 诊断设置'), this.app.store.state.diagTarget ? retry : null)
        : this.app.store.state.diagTarget
          ? h('div', { class: 'card-actions' }, retry)
          : null,
    );
  }

  private verdict(d: Diagnosis): HTMLElement {
    const r = d.result!;
    const eff = r.effective_recommendation;
    const changed = eff !== r.model_recommendation;
    const expired = d.state === 'expired';
    return h(
      'div',
      { class: 'panel diag-verdict-panel' },
      expired
        ? h(
            'div',
            { class: 'notice warn', role: 'status' },
            h('span', {}, '依据已变化或结果超过有效期：解释仍可阅读，处理入口已关闭。'),
            this.app.store.state.diagTarget ? h('button', { type: 'button', id: 'diag-redo', onclick: () => void this.start() }, '重新诊断') : null,
          )
        : null,
      h(
        'div',
        { class: 'diag-verdict', 'data-rec': eff },
        recMark(eff),
        h('p', { class: 'diag-summary' }, r.answer.summary),
        r.answer.conditions.length ? h('ul', { class: 'diag-conditions', 'aria-label': '前提条件' }, ...r.answer.conditions.map((c) => h('li', {}, c))) : null,
        h('p', { class: 'small diag-rec-explain' }, expired ? '这是依据变化前的判断，只供阅读；处理入口要在重新诊断后才会按当前事实打开。' : REC_EXPLAIN[eff]),
      ),
      changed
        ? h('p', { class: 'diag-adjusted small' }, `模型给出的是「${REC_LABEL[r.model_recommendation]}」，CleanManager 按本机事实调整为「${REC_LABEL[eff]}」。`)
        : null,
      r.conflicts.length
        ? h('ul', { class: 'diag-conflicts', 'aria-label': '与本机事实的冲突' }, ...r.conflicts.map((c) => h('li', {}, h('span', { class: 'cmark', 'aria-hidden': 'true' }), c.text)))
        : null,
      r.sanitized.length ? h('p', { class: 'small muted' }, `回答中有 ${r.sanitized.length} 处被移除或截断（命令、授权说法或无效引用）；CleanManager 不执行也不复制模型给出的命令。`) : null,
    );
  }

  private explanation(d: Diagnosis): HTMLElement {
    const a = d.result!.answer;
    return h(
      'section',
      { class: 'panel', 'aria-labelledby': 'diag-explain' },
      h('h2', { id: 'diag-explain' }, '用途与后果'),
      h(
        'dl',
        { class: 'fact-grid diag-facts' },
        h('dt', {}, '它是什么'),
        h('dd', {}, a.purpose),
        h('dt', {}, '删除后会怎样'),
        h('dd', {}, a.deletion_impact || '回答没有说明'),
        h('dt', {}, '重建与离线成本'),
        h('dd', {}, a.rebuild_cost || '回答没有说明'),
        a.gaps.length ? h('dt', {}, '模型提到的不确定') : null,
        a.gaps.length ? h('dd', {}, h('ul', { class: 'plain-list' }, ...a.gaps.map((g) => h('li', {}, g)))) : null,
      ),
    );
  }

  private ledger(d: Diagnosis): HTMLElement | null {
    const r = d.result!;
    if (!r.answer.claims.length) return null;
    return h(
      'section',
      { class: 'panel', 'aria-labelledby': 'diag-ledger' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'diag-ledger' }, '每句话的依据'), h('span', { class: 'small muted' }, '引用指向右侧的本机证据')),
      h(
        'ul',
        { class: 'claim-ledger' },
        ...r.answer.claims.map((c, i) => {
          const g = r.grounding.find((x) => x.index === i);
          const status = g?.status ?? 'unsupported';
          return h(
            'li',
            { class: 'claim-row', 'data-status': status },
            h('p', { class: 'claim-text' }, c.text),
            h(
              'div',
              { class: 'claim-meta' },
              pill(CLAIM_LABEL[status], CLAIM_TONE[status]),
              g?.note && g.note !== CLAIM_LABEL[status] ? h('span', { class: 'small muted' }, g.note) : null,
              ...c.refs.map((ref) => {
                const item = d.evidence.items.find((x) => x.evidence_id === ref);
                const gap = d.evidence.gaps.find((x) => x.gap_id === ref);
                const label = item ? LAYER_LABEL[item.layer] : gap ? '缺口' : '';
                return h(
                  'button',
                  { type: 'button', class: 'ref-chip', 'aria-label': `查看依据 ${ref}（${label}）`, onclick: () => this.focusEvidence(ref) },
                  item ? h('span', { class: 'lmark', 'data-layer': item.layer, 'aria-hidden': 'true' }) : null,
                  `${ref} ${label}`,
                );
              }),
            ),
          );
        }),
      ),
    );
  }

  private focusEvidence(ref: string): void {
    const el = document.getElementById(`ev-${ref}`);
    if (!el) return;
    el.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    el.focus({ preventScroll: true });
    el.classList.remove('flash');
    void el.offsetWidth;
    el.classList.add('flash');
  }

  private questions(d: Diagnosis): HTMLElement | null {
    const qs = d.result!.answer.questions;
    if (!qs.length || d.state !== 'completed') return null;
    return h(
      'section',
      { class: 'panel', 'aria-labelledby': 'diag-q' },
      h('h2', { id: 'diag-q' }, '回答这些问题能缩小不确定'),
      h('p', { class: 'small muted' }, '你的回答会作为“你的陈述”发送（先预览再确认），CleanManager 不会把它当作已核验的事实。'),
      ...qs.map((q) => {
        const id = `diag-other-${q.question_id}`;
        const input = h('input', { type: 'text', id, class: 'diag-other', maxlength: 280, placeholder: '其他回答' }) as HTMLInputElement;
        input.value = this.others.get(q.question_id) ?? '';
        input.addEventListener('input', () => this.others.set(q.question_id, input.value));
        input.addEventListener('keydown', (e) => {
          if (e.key === 'Enter' && input.value.trim()) void this.followUp(q.question_id, input.value);
        });
        return h(
          'fieldset',
          { class: 'diag-qa' },
          h('legend', {}, q.text),
          h(
            'div',
            { class: 'diag-choices' },
            ...q.choices.map((c) => h('button', { type: 'button', class: 'choice', disabled: this.busy, onclick: () => void this.followUp(q.question_id, c) }, c)),
          ),
          h('div', { class: 'diag-other-row' }, h('label', { class: 'sr-only', for: id }, `${q.text} 的其他回答`), input, h('button', { type: 'button', disabled: this.busy, onclick: () => void this.followUp(q.question_id, input.value) }, '补充')),
        );
      }),
    );
  }

  private nextSteps(d: Diagnosis): HTMLElement {
    const checklist = [...(d.result?.answer.conditions ?? []), ...d.evidence.gaps.map((g) => g.text)];
    return h(
      'section',
      { class: 'panel', 'aria-labelledby': 'diag-next' },
      h('h2', { id: 'diag-next' }, '下一步'),
      h('p', { class: 'small muted' }, '这些入口由 CleanManager 按本机事实决定；模型的建议只会被标注出来，不能打开新的处理方式。'),
      h(
        'ul',
        { class: 'next-steps' },
        ...d.handoff.map((x) => {
          const btn = x.available
            ? h('button', { type: 'button', id: `next-${x.action}`, class: x.action === 'review_plan' ? 'primary' : '', disabled: this.busy, onclick: () => void this.handoff(x), 'aria-label': ACTION_LABEL[x.action] }, ACTION_BUTTON[x.action])
            : null;
          const mode =
            x.action === 'review_plan' && x.available
              ? (() => {
                  const sel = h('select', { id: 'next-mode', 'aria-label': '计划的处理方式' }, h('option', { value: 'trash' }, `移入${words().bin}`), h('option', { value: 'quarantine' }, '移入隔离区')) as HTMLSelectElement;
                  sel.value = this.planMode;
                  sel.addEventListener('change', () => (this.planMode = sel.value as 'trash' | 'quarantine'));
                  return sel;
                })()
              : null;
          return h(
            'li',
            { class: 'next-row', 'data-available': x.available ? 'yes' : 'no' },
            h(
              'div',
              { class: 'next-text' },
              h('b', {}, ACTION_LABEL[x.action]),
              x.suggested_by_model ? h('span', { class: 'row-tag' }, '模型也建议') : null,
              h('span', { class: 'small muted' }, x.available ? x.reason : `不可用：${x.reason}`),
            ),
            h('div', { class: 'next-act' }, mode, btn),
          );
        }),
      ),
      this.showChecklist
        ? h('div', { class: 'gaps' }, h('p', { class: 'gaps-title' }, '核实清单'), h('ul', {}, ...checklist.map((c) => h('li', {}, c))))
        : null,
    );
  }

  private aside(): HTMLElement {
    const d = this.diag;
    const t = this.app.store.state.diagTarget;
    return h(
      'aside',
      { class: 'panel work-aside diag-aside', 'aria-label': '本机依据与记录' },
      d ? this.evidenceList(d) : h('div', {}, h('h2', {}, '本机依据'), h('p', { class: 'small muted' }, t ? '预览时会整理这个对象的本机证据：扫描观察、规则、保护与应用归属。' : '')),
      d ? this.sentInfo(d) : null,
      d ? this.record(d) : null,
      this.historyMini(),
    );
  }

  private evidenceList(d: Diagnosis): HTMLElement {
    return h(
      'div',
      { class: 'aside-section diag-evidence' },
      h('h2', {}, '本机依据'),
      h(
        'ul',
        { class: 'ev-list' },
        ...d.evidence.items.map((i) =>
          h(
            'li',
            { id: `ev-${i.evidence_id}`, tabindex: '-1', class: 'ev-item' },
            h('span', { class: 'ev-head' }, h('span', { class: 'lmark', 'data-layer': i.layer, 'aria-hidden': 'true' }), h('span', { class: 'ev-layer' }, LAYER_LABEL[i.layer]), h('code', {}, i.evidence_id)),
            h('span', { class: 'ev-text' }, i.text),
          ),
        ),
      ),
      d.evidence.gaps.length
        ? h(
            'div',
            { class: 'gaps' },
            h('p', { class: 'gaps-title' }, '仍然未知'),
            h('ul', {}, ...d.evidence.gaps.map((g) => h('li', { id: `ev-${g.gap_id}`, tabindex: '-1' }, g.text))),
          )
        : null,
    );
  }

  private sentInfo(d: Diagnosis): HTMLElement {
    return h(
      'details',
      { class: 'aside-section' },
      h('summary', {}, '发送了什么'),
      h(
        'dl',
        { class: 'facts-list small' },
        h('dt', {}, '提供方'),
        h('dd', {}, `${d.destination.label}（${PROVIDER_LABEL[d.destination.kind]}）`),
        h('dt', {}, '模型'),
        h('dd', {}, d.destination.model),
        h('dt', {}, '目的地'),
        h('dd', { class: 'path' }, d.destination.endpoint ?? d.destination.executable ?? ''),
        h('dt', {}, '范围'),
        h('dd', {}, [d.request.disclosure.full_path ? '含完整路径' : '路径已脱敏', d.request.disclosure.child_summary ? '含子项摘要' : '无子项', d.request.statements.length ? `含 ${d.request.statements.length} 条你的陈述` : null].filter(Boolean).join('，')),
        h('dt', {}, '载荷摘要'),
        h('dd', {}, h('code', { class: 'selectable' }, `${d.payload_digest.slice(0, 16)}…`)),
        h('dt', {}, '模板'),
        h('dd', {}, d.destination.template),
      ),
    );
  }

  private record(d: Diagnosis): HTMLElement {
    const save = d.retention === 'session' ? h('button', { type: 'button', class: 'quiet', id: 'diag-save', onclick: () => void this.save() }, '保存这条诊断') : null;
    const remove = h('button', { type: 'button', class: 'danger-quiet', id: 'diag-remove', onclick: () => void this.remove() }, '移除记录');
    const exp = h('button', { type: 'button', class: 'quiet', id: 'diag-export', 'aria-expanded': this.exportText ? 'true' : 'false', onclick: () => void this.exportPreview() }, this.exportText ? '收起导出预览' : '导出预览');
    return h(
      'div',
      { class: 'aside-section' },
      h('h3', {}, '记录'),
      h(
        'p',
        { class: 'small muted' },
        d.retention === 'saved' ? '已保存在本机，直到你移除。' : `仅本次会话保留，${formatTime(d.retain_until)} 后自动移除。`,
        ' 移除诊断不会影响文件操作记录。',
      ),
      h('p', { class: 'small' }, '编号 ', h('code', { class: 'selectable' }, d.diagnosis_id), '（CLI：', h('code', { class: 'selectable' }, `cleanmanager diag show ${d.diagnosis_id}`), '）'),
      h('div', { class: 'card-actions' }, save, exp, remove),
      this.exportText ? h('pre', { class: 'dp-pre selectable', tabindex: '0', 'aria-label': '导出内容（不含密钥）' }, this.exportText) : null,
    );
  }

  private historyMini(): HTMLElement | null {
    const list = (this.history ?? []).slice(0, 6);
    if (!list.length) return null;
    return h(
      'div',
      { class: 'aside-section' },
      h('h3', {}, '最近的诊断'),
      h('ul', { class: 'diag-history' }, ...list.map((x) => this.historyRow(x, true))),
    );
  }

  private historyRow(x: DiagSummary, compact: boolean): HTMLElement {
    const open = () => this.app.store.set({ diagId: x.diagnosis_id, diagTarget: { scanId: x.object.scan_id, entryId: x.object.entry_id, origin: x.object.origin, name: x.subject_name } as DiagTarget });
    return h(
      'li',
      {},
      h(
        'button',
        { type: 'button', class: compact ? 'recent-item' : 'tl-row', 'aria-current': this.diag?.diagnosis_id === x.diagnosis_id ? 'true' : null, onclick: open },
        compact ? null : h('span', { class: 'tl-time' }, formatTime(x.created_at)),
        h('span', { class: compact ? '' : 'tl-main' }, h('span', { class: 'tl-title' }, x.subject_name), compact ? null : h('span', { class: 'tl-sub' }, `${x.model}，${x.retention === 'saved' ? '已保存' : '本次会话'}`)),
        h('span', { class: compact ? 'muted' : 'tl-side' }, x.effective_recommendation ? REC_LABEL[x.effective_recommendation] : STATE_LABEL[x.state]),
      ),
    );
  }

  private historyPage(): HTMLElement {
    const list = this.history;
    return h(
      'div',
      { class: 'diag-home' },
      h(
        'div',
        { class: 'panel' },
        h('h2', {}, '从对象开始'),
        h('p', { class: 'muted' }, '在磁盘页的文件详情、清理页的候选详情或应用页的应用详情里选择「AI 诊断」。没有清理规则的大文件也可以诊断。'),
        this.status && !this.status.configured ? h('div', { class: 'card-actions' }, h('button', { type: 'button', onclick: () => this.app.navigate('settings') }, '配置 AI 诊断')) : null,
      ),
      h(
        'section',
        { class: 'panel list-panel', 'aria-labelledby': 'diag-hist' },
        h('h2', { id: 'diag-hist', class: 'list-title' }, '诊断记录'),
        !list ? h('p', { class: 'muted', role: 'status' }, '正在读取…') : list.length ? h('ul', { class: 'timeline' }, ...list.map((x) => this.historyRow(x, false))) : h('p', { class: 'muted small list-empty' }, '还没有诊断记录。记录默认只保留到本次会话结束，可以逐条保存。'),
      ),
      h('p', { class: 'small muted' }, icon('info'), ' 诊断只解释对象，不删除、不批准；处理始终走原有的计划审阅与授权。'),
    );
  }
}

