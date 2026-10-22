// P05 — plan review and authorization (GUI-03/04, PLN-01..07). Shows the complete stored
// plan: mode and what it does to space and recovery, every item (paged, never a top-N
// summary), rule versions, digest, expiry, what is not included, and where authorization
// comes from. Approval is the person typing the digest's first 8 characters; it writes the
// same receipt the CLI's `plan review` writes. The same plan id works in the CLI.

import type { App } from '../main';
import { toIpcError } from '../lib/api';
import { formatBytes, formatExactBytes, groupDigits, parseBytes } from '../lib/bytes';
import { refusalText, unknownCodeText } from '../lib/codes';
import { h, icon, keepFocus, replace } from '../lib/dom';
import { formatTime } from '../lib/format';
import { authView, kindVerb, NO_SPACE, planExplain, planStatement } from '../lib/planModel';
import type { IpcError, PlanDetail, PlanItem } from '../lib/types';
import { AUTH_LABEL, OPERATION_STATE_LABEL, PLAN_KIND_LABEL, POLICY_MISMATCH_LABEL, POLICY_STATE_LABEL, digestGroups, formatCountdown, normalizeCode } from '../lib/words';
import { copyField, errorLine, pageHead, pill, type Tone } from './parts';
import { EXTENSIONS, TOOLS } from '../lib/nav';
import { words } from '../lib/platform';
import type { Page } from '../lib/store';
import { LEFTOVER_GROUP, ROUTE_WORD, TASK_WORDS } from '../lib/suiteWords';
import { noticeLine, stageSteps, undoBadge } from './kit';
import { UNINSTALL_STEPS } from './uninstall';
import { rowsByApp } from './uninstallBatch';
import { emptyBlock, pathText, stateLine, statusBanner } from './ui';

/** Words of the plan page that are not data (one table for the language switch). */
export const PLAN_PAGE_TEXT = {
  title: '计划',
  review: '审阅计划',
  noneTitle: '没有打开的计划',
  noneBody: '在清理页选择候选并生成计划，或在活动记录中打开一份已有计划。',
  toCleanup: '前往清理',
  loadFailed: '无法读取这份计划',
  loading: '正在读取计划…',
  countsLabel: '这份计划的范围',
  irreversibleSr: '无法撤销',
  everyItemIrreversible: '每一项都无法撤销',
  approvePermanent: '批准这份永久删除计划',
  ledger: '范围与依据',
} as const;

/** Human name of the page a plan was opened from (its back link). */
function pageName(p: Page): string {
  for (const t of TOOLS) for (const sec of t.sections) if (sec.page === p) return sec.label;
  const ext = EXTENSIONS.find((e) => e.page === p);
  return ext ? ext.label : p === 'plan' ? '计划' : '上一页';
}

/** Item kinds that have a size (files and folders); registry keys and startup entries do not. */
const SIZED = new Set(['file', 'directory']);

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
      const permanent = kindVerb(d, this.items).permanent;
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
      await this.app.applyPlan(d.plan_id, d.kind === 'maintenance' ? 'maintenance' : 'apply');
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
    if (from && from !== 'plan' && from !== 'operation') return { label: `返回${pageName(from)}`, onClick: () => this.app.navigate(from) };
    return { label: '返回清理候选', onClick: () => this.app.navigate('cleanup') };
  }

  private render(): void {
    const P = PLAN_PAGE_TEXT;
    if (!this.planId) {
      replace(this.body, h('h1', { id: 'plan-title' }, P.title), emptyBlock({ title: P.noneTitle, body: P.noneBody, actions: [h('button', { type: 'button', class: 'primary', onclick: () => this.app.navigate('cleanup') }, P.toCleanup)] }));
      return;
    }
    if (this.error && !this.detail) {
      replace(this.body, pageHead({ id: 'plan-title', title: P.title, back: this.back() }), statusBanner({ kind: 'failed', level: 2, titleId: 'plan-load-title', title: P.loadFailed, sentence: refusalText(this.error) }));
      return;
    }
    const d = this.detail;
    if (!d) {
      replace(this.body, pageHead({ id: 'plan-title', title: P.review, back: this.back() }), h('p', { class: 'muted', role: 'status' }, P.loading));
      return;
    }
    const tone: Tone = d.authorization.status === 'authorized' ? 'good' : d.authorization.status === 'required' ? 'busy' : 'muted';
    const left = this.secondsLeft();
    replace(
      this.body,
      pageHead({
        id: 'plan-title',
        title: `审阅${d.catalog === 'purge' ? '构建产物清理计划' : d.catalog === 'installer' ? '安装包清理计划' : PLAN_KIND_LABEL[d.kind]}`,
        back: this.back(),
        meta: [
          pill(d.operation ? `已执行：${OPERATION_STATE_LABEL[d.operation.state] ?? d.operation.state}` : AUTH_LABEL[d.authorization.status], d.operation ? 'muted' : tone),
          `冻结于 ${formatTime(d.created_at)}`,
          h('span', { 'data-countdown': '' }, left > 0 ? `剩余 ${formatCountdown(left)}` : '已过期'),
        ],
      }),
      d.kind === 'uninstall' || d.kind === 'uninstall_leftovers' ? stageSteps('卸载分两步，每步单独审阅', UNINSTALL_STEPS, d.kind === 'uninstall' ? 0 : 1) : null,
      this.statementEl(d),
      h('div', { class: 'work-layout' }, h('div', { class: 'work-main' }, this.ledger(d), this.itemsPanel(d)), this.authPanel(d)),
    );
  }

  /**
   * The page's one primary element: what will happen in one sentence, the counts, and whether it
   * can be undone — before any table. A permanent plan carries a hatched edge, a warning shape and
   * the word 无法撤销, so it never depends on colour.
   */
  private statementEl(d: PlanDetail): HTMLElement {
    const s = planStatement(d, this.items);
    return h(
      'section',
      { class: 'plan-statement', 'data-mode': s.tone, 'data-irreversible': s.irreversible ? 'yes' : 'no', 'aria-labelledby': 'ps-sentence' },
      h('h2', { id: 'ps-sentence', class: 'ps-sentence' }, s.irreversible ? h('span', { class: 'sr-only' }, `${PLAN_PAGE_TEXT.irreversibleSr}：`) : null, s.sentence),
      h(
        'ul',
        { class: 'ps-counts', 'aria-label': PLAN_PAGE_TEXT.countsLabel },
        ...s.counts.map((c) => h('li', {}, h('span', { class: 'ps-count-label' }, c.label), c.path ? pathText(c.value, { max: 40, className: 'ps-count-value' }) : h('span', { class: 'ps-count-value num' }, c.value))),
      ),
      h(
        'div',
        { class: 'ps-undo' },
        undoBadge(s.undo, s.undoLabel),
        s.undoText ? h('p', { class: 'ps-undo-text' }, s.undoText) : null,
      ),
      s.spaceText ? h('p', { class: `ps-space${s.tone === 'quarantine' ? ' strong-note' : ''}` }, s.spaceText) : null,
    );
  }

  /** Secondary: where the plan comes from and what binds it; then what is not in it. */
  private ledger(d: PlanDetail): HTMLElement {
    const total = parseBytes(d.total_logical_bytes);
    const facts: (HTMLElement | null)[] = [];
    const fact = (k: string, ...v: (Node | string | null)[]) => facts.push(h('dt', {}, k), h('dd', {}, ...v));
    const explain = planExplain(d);
    if (explain) fact('做什么', explain.effect);
    if (d.kind === 'manual_trash') {
      fact('来源扫描', pathText(d.scan_root_display ?? '（范围不可用）', { max: 56 }), h('small', {}, `扫描 ${d.scan_id ?? '—'}，版本 ${d.scan_revision ?? '—'}；按条目编号选择，不按路径`));
    }
    if (d.kind === 'maintenance' && d.kind_detail?.admin_tasks)
      fact('权限', `${d.kind_detail.admin_tasks} 项需要${words().admin}`, h('small', {}, d.kind_detail.is_admin ? 'CleanManager 已以管理员身份运行，这些任务直接执行。' : words().maintAdminReview));
    else if (d.requires_admin) fact('权限', `需要${words().admin}`, h('small', {}, 'CleanManager 不会自动提权；没有管理员权限时执行会被拒绝，不会部分修改。'));
    const kd = d.kind_detail;
    const batch = d.kind === 'uninstall' && (kd?.apps?.length ?? 0) > 1 ? kd!.apps! : null;
    if (batch) {
      // UNI-11: several applications, each with its own close request, uninstaller and result.
      fact('应用', `${batch.length} 个`, h('small', {}, batch.map((a) => a.name).join('、')));
      fact('卸载方式', '各自的卸载程序', h('small', {}, '每个应用执行前再核验一次它的卸载登记、程序文件摘要与签名者；一个应用失败或被跳过，不影响其他应用。'));
      const est = kd?.noted?.leftover_estimate;
      if (est && Object.keys(est).length)
        fact('可能留下', Object.entries(est).map(([g, n]) => `${LEFTOVER_GROUP[g]?.title ?? g} ${n}`).join('，'), h('small', {}, '这是所有应用合计的估计，不在这份计划里，也不被授权；卸载后在第 2 步按应用单独审阅。'));
    } else if (d.kind === 'uninstall' && kd?.app) {
      fact('应用', kd.app.name, h('small', {}, [kd.app.publisher, kd.app.version ? `版本 ${kd.app.version}` : null].filter(Boolean).join(' · ')));
      fact('卸载方式', kd.app.adapter, h('small', {}, kd.app.signer ? `签名者 ${kd.app.signer}，执行前再核验一次程序文件的摘要与签名` : '执行前再核验一次程序文件的摘要'));
      const est = kd.noted?.leftover_estimate;
      if (est && Object.keys(est).length)
        fact('可能留下', Object.entries(est).map(([g, n]) => `${LEFTOVER_GROUP[g]?.title ?? g} ${n}`).join('，'), h('small', {}, '这是估计，不在这份计划里，也不被授权；卸载后在第 2 步单独审阅。'));
    }
    if (d.kind === 'uninstall_leftovers' && kd?.registry_keys) fact('注册表', `${kd.registry_keys} 个键`, h('small', {}, '执行时先备份整棵键，备份失败就不删除；之后可以生成还原计划。'));
    if (d.kind === 'process' && kd?.noted) {
      fact('进程', `${kd.noted.name ?? ''}（PID ${kd.noted.pid ?? '—'}）`, h('small', {}, kd.noted.purpose ?? (kd.noted.unidentified_reason ? `未识别：${kd.noted.unidentified_reason}` : '')));
    }
    if (d.kind === 'cleanup') {
      fact('来源扫描', pathText(d.scan_root_display ?? '（范围不可用）', { max: 56 }), h('small', {}, `扫描 ${d.scan_id ?? '—'}，版本 ${d.scan_revision ?? '—'}`));
      fact(d.catalog ? '内置目录规则' : '规则版本', d.rule_refs.length ? h('span', { class: 'rule-list' }, ...d.rule_refs.map((r) => h('code', { class: 'selectable' }, r))) : '—');
      fact('对象', `${groupDigits(d.item_count ?? 0)} 项`, h('small', {}, `其中 ${groupDigits(d.default_selected_items)} 项来自低风险规则`));
    }
    if (!NO_SPACE.has(d.kind)) fact('已知逻辑大小', total === null ? '未知' : formatBytes(total), h('small', {}, total === null ? '计划中不允许大小未知的文件' : formatExactBytes(total)));
    if (d.allocated_known_bytes !== null && !NO_SPACE.has(d.kind))
      fact('分配大小', d.allocated_unknown_count ? `至少 ${formatBytes(d.allocated_known_bytes)}，${d.allocated_unknown_count} 项未知` : formatBytes(d.allocated_known_bytes));
    fact('有效至', formatTime(d.expires_at), h('small', {}, '计划有效 15 分钟；过期后需要重新生成，不会在旧计划上修补'));
    return h(
      'section',
      { class: 'panel ledger', 'aria-labelledby': 'ledger-title' },
      h('h2', { id: 'ledger-title' }, PLAN_PAGE_TEXT.ledger),
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
    } else if (d.kind === 'manual_trash' || d.kind === 'manual_purge') {
      lines.push('只包含你在磁盘页手动选中的对象；没有规则会自动扩大这份清单，也没有策略会匹配它。');
      lines.push('执行前逐项核对身份；对象已变化、正被占用或其中出现受保护对象时跳过，不会替换成别的文件。');
    } else if (d.kind === 'startup' || d.kind === 'startup_restore') {
      lines.push('不包含程序文件、快捷方式、计划任务本身或应用设置；只改启用记录。');
      lines.push('执行前再次读取该项；状态在此期间被改变时跳过，不覆盖。');
    } else if (d.kind === 'uninstall') {
      lines.push(`不包含任何残留：卸载后留下的${words().leftoverKinds}在第 2 步单独审阅，单独批准。`);
      lines.push('不会强制结束应用。应用没有退出时，卸载程序不运行，也不删除任何东西。');
      lines.push('执行前再次核对卸载登记、程序文件的摘要和签名者；有任何变化就跳过。');
      if ((d.kind_detail?.apps?.length ?? 0) > 1) lines.push('一个应用没有退出、卸载失败或已变化时只跳过它自己；其余应用照常执行，结果逐个报告。卸载没有完成的应用，它的残留不会进入第 2 步。');
      const refused = d.kind_detail?.noted?.refused ?? [];
      if (refused.length) lines.push(`你选择的 ${refused.length} 个应用不在计划中，与单独卸载时一样被拒绝：${refused.map((r) => `${r.name ?? r.app_instance_id}（${refusalText({ reason: r.reason })}）`).join('；')}。`);
    } else if (d.kind === 'uninstall_leftovers') {
      lines.push('只包含你在残留审阅中选中的对象；保留、已阻止和需要手动处理的对象不在其中。');
      lines.push('文件夹只移走其中的内容，空文件夹保留；对象已变化、正被占用或受保护时跳过。');
    } else if (d.kind === 'update') {
      const routed = d.kind_detail?.noted?.routed ?? [];
      lines.push(words().updatesPlanScope);
      if (routed.length) lines.push(`另有 ${routed.length} 个应用不在计划中：${routed.map((r) => `${r.name}（${ROUTE_WORD[r.route]}）`).join('、')}。`);
      lines.push('正在运行的应用会被跳过并提示“请先关闭”；CleanManager 不会为了更新而关闭应用。');
    } else if (d.kind === 'process') {
      lines.push('只包含这一个进程；它启动的其他进程不受影响。');
      lines.push('PID 被复用或程序被替换时跳过，不会结束别的进程。');
    } else if (d.kind === 'maintenance') {
      lines.push('只包含列出的任务；搜索时出现的“不提供的操作”（如注册表优化）永远不是任务。');
      lines.push('你保护的任务、与审阅时版本不同的任务都会被跳过并说明原因。');
    } else {
      lines.push('只包含所选的隔离项；隔离区中的其他对象不受影响。');
      lines.push(d.kind === 'restore' ? '原位置已有同名对象时跳过，不覆盖、不改名到其他位置。' : '清空不会恢复任何对象；需要恢复的项目请另建恢复计划。');
    }
    return h('div', { class: 'not-included' }, h('h3', {}, '不包含的内容'), h('ul', {}, ...lines.map((l) => h('li', {}, l))));
  }

  private itemsPanel(d: PlanDetail): HTMLElement {
    const recovery = d.kind === 'restore' || d.kind === 'purge';
    const toggles = NO_SPACE.has(d.kind);
    const irreversible = kindVerb(d, this.items).permanent;
    const ACTION: Record<string, string> = {
      startup_disable: '停用',
      startup_enable: '启用',
      startup_remove_with_backup: '先备份，再移除启动项',
      registry_restore: '从备份还原',
      registry_backup_delete: '先备份，再删除注册表项',
      fs_trash: words().moveToBin,
      fs_delete: '永久删除',
      app_quit: '请应用退出（不强制结束）',
      process_end: '结束这个进程',
    };
    const actionText = (i: PlanItem): string | null => {
      if (i.item_kind === 'uninstaller') return words().uninstallerAction;
      if (i.item_kind === 'app_update') {
        const det = i.detail ?? {};
        if (det.channel === 'homebrew') return '经 Homebrew 更新；安装前由 Homebrew 校验下载的校验和';
        return `经 ${String(det.channel ?? '包管理器')} 更新；安装包签名者必须是 ${String(det.installed_signer ?? '已安装应用的签名者')}`;
      }
      if (i.item_kind === 'maintenance_task') return i.scope === 'machine' ? `需要${words().admin}` : '以当前用户运行';
      if (i.action_type && ACTION[i.action_type]) return ACTION[i.action_type];
      return null;
    };
    const nameOf = (i: PlanItem): string => (i.item_kind === 'maintenance_task' ? (TASK_WORDS[String(i.detail?.task_id ?? '')]?.title ?? i.name) : i.name);
    const rowOf = (i: PlanItem) =>
      h(
        'tr',
        {},
        h('td', { class: 'col-id' }, h('code', {}, i.action_id)),
        h(
          'td',
          { class: 'col-object' },
          h('span', { class: 'obj-name' }, nameOf(i), i.item_kind === 'directory' && d.kind !== 'uninstall_leftovers' ? h('span', { class: 'muted small' }, '（空文件夹，内容成功后才移除）') : null),
          recovery ? h('span', { class: 'obj-path' }, '回到 ', pathText(i.original_path_display ?? '—', { max: 72 })) : pathText(i.path_display, { max: 76, className: 'obj-path' }),
          d.kind !== 'cleanup' && actionText(i) ? h('span', { class: 'obj-reason' }, actionText(i), i.scope && i.item_kind !== 'maintenance_task' ? `（${i.scope === 'user' ? '当前用户' : '所有用户'}）` : '') : null,
          i.detail?.command ? h('span', { class: 'obj-reason muted' }, h('code', {}, String(i.detail.command))) : null,
        ),
        toggles ? null : h('td', { class: 'col-size num', title: SIZED.has(i.item_kind) ? formatExactBytes(parseBytes(i.logical_bytes)) : '' }, SIZED.has(i.item_kind) ? formatBytes(i.logical_bytes) : '—'),
      );
    const table = (items: PlanItem[], caption: string) =>
      h(
        'div',
        { class: 'table-wrap table-scroll' },
        h(
          'table',
          { class: 'data-table' },
          h('caption', { class: 'sr-only' }, caption),
          h('thead', {}, h('tr', {}, h('th', { scope: 'col', class: 'col-id' }, '动作'), h('th', { scope: 'col' }, '对象'), toggles ? null : h('th', { scope: 'col', class: 'col-size' }, '逻辑大小'))),
          h('tbody', {}, ...items.map(rowOf)),
        ),
      );
    // UNI-11: an uninstall plan for several applications is reviewed application by application.
    const apps = d.kind === 'uninstall' ? (d.kind_detail?.apps ?? []) : [];
    const body =
      apps.length > 1
        ? rowsByApp(this.items, apps).flatMap((g, n) =>
            g.items.length ? [h('h3', { id: `items-app-${n}` }, `${n + 1}. ${g.name}`, h('span', { class: 'muted small' }, `（${g.items.length} 个动作）`)), table(g.items, `${g.name} 的动作`)] : [],
          )
        : [table(this.items, irreversible ? `计划中的全部动作；每一项都${PLAN_PAGE_TEXT.irreversibleSr}` : '计划中的全部动作')];
    return h(
      'section',
      { class: 'panel items-panel', 'aria-labelledby': 'items-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'items-title' }, `完整清单（${groupDigits(d.item_count ?? 0)} 项）`), irreversible ? undoBadge('irreversible', PLAN_PAGE_TEXT.everyItemIrreversible) : null),
      h('p', { class: 'muted small' }, apps.length > 1 ? `按应用分组，按计划顺序执行：${apps.length} 个应用各自退出、各自卸载。这里就是将被处理的全部对象，没有隐藏项。` : '按计划顺序列出每一个动作。这里就是将被处理的全部对象，没有隐藏项。'),
      this.items.length ? h('div', {}, ...body) : h('p', { class: 'muted small', role: 'status' }, this.itemsLoading ? '正在加载清单…' : '清单为空。'),
      errorLine(this.itemsError),
      this.itemsNext
        ? h('button', { type: 'button', class: 'quiet more', id: 'plan-items-more', disabled: this.itemsLoading, onclick: () => void this.loadItems(true) }, this.itemsLoading ? '正在加载…' : `显示更多（已显示 ${this.items.length} / ${this.itemsTotal}）`)
        : null,
    );
  }

  /** The decision: one state line, then only the action that state allows. */
  private authPanel(d: PlanDetail): HTMLElement {
    const a = d.authorization;
    const { run, permanent: danger } = kindVerb(d, this.items);
    const v = authView(d);
    const blocks: (HTMLElement | null)[] = [];
    if (v.line) blocks.push(stateLine({ kind: v.line.kind, text: v.line.text, className: 'auth-line' }));
    if (a.status === 'authorized' && !d.operation)
      blocks.push(h('p', { class: 'small muted auth-source' }, a.source === 'policy' ? `来源：策略 ${a.policy_id}（版本 ${a.policy_version}）` : `来源：人工审阅（${a.principal ?? ''}）于 ${formatTime(a.authorized_at)}`));
    if (v.view === 'executed') {
      blocks.push(h('button', { type: 'button', class: 'primary wide', id: 'plan-view-result', onclick: () => this.app.openOperation(d.operation!.operation_id, 'plan') }, '查看执行结果'));
    } else if (v.view === 'terminal') {
      blocks.push(
        noticeLine(
          d.terminal_only === 'end_processes'
            ? '这份计划可能在执行时结束仍在运行的进程，需要你在终端中输入 END 确认。这个窗口不批准、也不执行它。'
            : '这份计划会永久删除对象，需要在终端中额外确认不可恢复。这个窗口不批准、也不执行它。',
          'warn',
        ),
        copyField('在终端中审阅并批准', d.cli.review),
        copyField('在终端中执行', d.cli.apply),
      );
    } else if (v.view === 'review') {
      blocks.push(this.seal(d, danger));
    } else if (v.view === 'authorized') {
      const apply = h('button', { type: 'button', id: 'plan-apply', class: `${danger ? 'danger' : 'primary'} wide`, disabled: this.busy }, danger ? icon('warn') : null, run);
      apply.addEventListener('click', () => void this.apply());
      const revoke = h('button', { type: 'button', class: 'quiet', id: 'plan-revoke', disabled: this.busy }, '撤销批准');
      revoke.addEventListener('click', () => void this.revoke());
      blocks.push(
        apply,
        h('p', { class: 'muted small' }, d.mode === 'quarantine' ? '此操作不立即释放空间。执行中可以停止后续操作；已完成的项目不会自动撤销。' : danger ? (d.kind === 'process' ? '结束进程无法撤销，其中未保存的工作会丢失。' : '永久删除无法撤销。执行中可以停止后续操作；已删除的不会恢复。') : '执行中可以停止后续操作；已完成的项目不会自动撤销。'),
        revoke,
      );
    } else {
      blocks.push(h('button', { type: 'button', class: 'wide', onclick: () => this.back().onClick() }, this.back().label));
    }
    blocks.push(errorLine(this.actionError));
    return h(
      'aside',
      { class: 'panel work-aside auth-panel', 'aria-labelledby': 'auth-title' },
      h('h2', { id: 'auth-title', tabindex: '-1' }, '授权'),
      ...blocks,
      d.kind === 'cleanup' && !d.catalog
        ? this.policies(d)
        : h('p', { class: 'muted small' }, d.kind === 'cleanup' ? '构建产物与安装包计划只能由人工审阅授权。' : '这类计划只能由人工审阅授权，任何策略都不会匹配它们。'),
      d.cli.show ? this.handoff(d) : null,
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
    const approve = h('button', { type: 'button', id: 'plan-approve', class: `${danger ? 'danger' : 'primary'} wide`, disabled: normalizeCode(this.typed) !== d.digest_prefix || this.busy }, danger ? icon('warn') : null, this.busy ? '正在批准…' : danger ? PLAN_PAGE_TEXT.approvePermanent : '批准这份计划') as HTMLButtonElement;
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
      { class: 'seal', 'data-irreversible': danger ? 'yes' : 'no' },
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
        h('span', { class: 'small' }, `${POLICY_STATE_LABEL[p.state]}，${p.covers ? '覆盖这份计划（仍按额度检查）' : `不覆盖：${POLICY_MISMATCH_LABEL[p.reason ?? ''] ?? (p.reason ? unknownCodeText(p.reason) : '')}`}`),
      ),
    );
    const create = h('button', { type: 'button', class: 'quiet', id: 'policy-from-plan' }, '为同类计划创建有限策略');
    create.addEventListener('click', () => void this.policyFromPlan());
    return h(
      'details',
      { class: 'aside-section', open: d.policies.some((p) => p.covers) ? true : null },
      h('summary', {}, `预授权策略（${rows.length}）`),
      rows.length ? h('ul', { class: 'policy-mini' }, ...rows) : h('p', { class: 'muted small' }, '没有策略。默认所有计划都需要人工批准。'),
      this.policyNote ? h('p', { class: 'small', role: 'status' }, this.policyNote) : null,
      d.default_selected_items === d.item_count ? create : h('p', { class: 'muted small' }, '策略只覆盖全部来自低风险规则的计划，这份计划不适用。'),
    );
  }

  private handoff(d: PlanDetail): HTMLElement {
    return h(
      'details',
      { class: 'aside-section' },
      h('summary', {}, '在终端中使用同一份计划'),
      h('p', { class: 'muted small' }, 'CLI 读取同一个计划编号与摘要；在任一入口批准都对另一入口有效。'),
      h('p', { class: 'small' }, '计划编号 ', h('code', { class: 'selectable' }, d.plan_id)),
      copyField('查看', d.cli.show),
      copyField('审阅并批准', d.cli.review),
      copyField('执行', d.cli.apply),
      h('p', { class: 'muted small' }, '自动化调用不能批准计划，只能等待人工批准或已启用的策略。'),
    );
  }
}
