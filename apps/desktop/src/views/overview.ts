// P02 — overview of the scan this window is bound to. One primary message per state (the status banner),
// then only what that state needs: running → progress and cancel; completed → the space summary;
// partial → what was missed, then the known part; failed → the reason and the next step; cancelled /
// interrupted → how much was kept. Machine facts (codes, backend, raw messages) sit in collapsed details.
// No health score, no "0 B" before a scan, no percentage of an unknown total.

import type { App } from '../main';
import type { InsightsList, IpcError, RecentScan, ScanStatus } from '../lib/types';
import { insightCards } from './tools';
import { toIpcError } from '../lib/api';
import { formatExactBytes, groupDigits } from '../lib/bytes';
import { refusalText } from '../lib/codes';
import { sizeLabel } from '../lib/display';
import { h, keepFocus, replace } from '../lib/dom';
import { COMPLETENESS_LABEL, formatElapsed, formatTime, isTerminal, STATE_LABEL, stateTone } from '../lib/format';
import { overviewPlan, type OverviewPlan } from '../lib/overviewModel';
import { emptyBlock, issueList, learnMore, pathText, sizeEl, statusBanner, techBlock } from './ui';

type Child = Node | string | null | undefined | false;

export class OverviewPage {
  el: HTMLElement;
  private body: HTMLElement;
  private actionError: IpcError | null = null;
  private timer: number | null = null;
  private insights: InsightsList | null = null;
  private insightsRequested = false;
  private announced: string | null = null;
  private busy = false;

  constructor(private app: App) {
    this.body = h('div', { class: 'overview-body' });
    this.el = h('section', { class: 'page page-overview', 'aria-labelledby': 'overview-title' }, this.body);
  }

  activate(): void {
    void this.app.refreshStatus();
    if (!this.insightsRequested) {
      this.insightsRequested = true;
      this.app.backend
        .insightsList()
        .then((r) => {
          this.insights = r;
          this.update();
        })
        .catch(() => (this.insights = null));
    }
  }

  update(): void {
    keepFocus(this.el, () => this.render());
    // Elapsed time ticks locally while running; the store is not re-read for it.
    const s = this.app.store.state.status;
    const running = s && !isTerminal(s.state);
    if (running && this.timer === null) this.timer = window.setInterval(() => this.tickElapsed(), 1000);
    if (!running && this.timer !== null) {
      window.clearInterval(this.timer);
      this.timer = null;
    }
  }

  private tickElapsed(): void {
    const el = this.el.querySelector('[data-elapsed]');
    const s = this.app.store.state.status;
    if (el && s) el.textContent = `已用 ${formatElapsed(s.started_at)}`;
  }

  private render(): void {
    const st = this.app.store.state;
    if (!st.scanId) {
      replace(
        this.body,
        h('div', { class: 'ov-single' }, emptyBlock({ id: 'overview-title', level: 1, icon: 'tool-analyze', title: '还没有扫描结果', body: '选一个文件夹开始只读扫描，这里会显示它占用了多少空间。', actions: [this.chooseButton('primary', '选择扫描范围')] })),
      );
      return;
    }
    if (st.statusError && !st.status) {
      replace(
        this.body,
        h(
          'div',
          { class: 'ov-single' },
          statusBanner({ kind: 'failed', titleId: 'overview-title', srPrefix: '概览：', title: '无法读取扫描状态', sentence: refusalText(st.statusError), actions: [this.chooseButton('primary', '选择扫描范围')] }),
          techBlock(h('p', { class: 'selectable small' }, `${st.statusError.reason}：${st.statusError.message}`)),
        ),
      );
      return;
    }
    if (!st.status) {
      replace(this.body, h('div', { class: 'ov-single' }, h('h1', { id: 'overview-title', class: 'sb-title' }, '概览'), h('p', { class: 'muted', role: 'status' }, '正在读取扫描状态…')));
      return;
    }
    const s = st.status;
    const plan = overviewPlan(s);
    this.announce(s, plan);
    const main = h('div', { class: 'ov-main' }, ...this.mainColumn(s, plan));
    replace(this.body, plan.aside ? h('div', { class: 'ov-layout' }, main, this.aside(plan)) : h('div', { class: 'ov-single' }, main));
  }

  /** Status changes are announced once (navigation to an unchanged scan says nothing). */
  private announce(s: ScanStatus, plan: OverviewPlan): void {
    const key = `${s.scan_id}:${plan.layout}:${plan.banner.title}`;
    if (this.announced !== null && this.announced !== key && this.announced.startsWith(`${s.scan_id}:`)) this.app.announce(`${plan.banner.title}。${plan.banner.sentence}`);
    this.announced = key;
  }

  // ---------------------------------------------------------------------------------------------
  // Main column

  private mainColumn(s: ScanStatus, plan: OverviewPlan): Child[] {
    const meta: Child[] = [
      s.root_display ? pathText(s.root_display, { max: 64, className: 'sb-path' }) : '（范围路径不可用）',
      plan.layout === 'running' ? h('span', { 'data-elapsed': '1' }, `已用 ${formatElapsed(s.started_at)}`) : s.finished_at ? `${formatTime(s.finished_at)} 结束` : null,
      plan.layout !== 'running' && plan.layout !== 'failed' && s.started_at && s.finished_at ? `用时 ${formatElapsed(s.started_at, s.finished_at)}` : null,
    ];
    const extra: Child[] = [];
    if (plan.layout === 'running') extra.push(h('div', { class: 'indeterminate', role: 'progressbar', 'aria-label': '扫描进度（总量未知）', 'aria-valuetext': `已发现 ${groupDigits(s.entries_seen)} 个条目` }));
    // Completed and stopped scans carry their figure in the banner: the space summary comes first.
    if (plan.space && plan.layout !== 'partial') extra.push(this.figure(s, plan));
    if (this.actionError) extra.push(h('p', { class: 'form-error', role: 'alert' }, refusalText(this.actionError)));

    const banner = statusBanner({ kind: plan.banner.kind, titleId: 'overview-title', srPrefix: '概览：', title: plan.banner.title, sentence: plan.banner.sentence, meta, extra, actions: this.actions(s, plan) });
    const corner = plan.layout !== 'failed' && plan.layout !== 'running';
    const head = h('div', { class: `ov-head${corner ? ' has-corner' : ''}` }, banner, corner ? this.chooseButton('quiet', '新建扫描', 'ov-new') : null);

    const out: Child[] = [head];
    if (s.elevated_read.length && plan.layout !== 'failed') out.push(h('p', { class: 'notice warn' }, `此结果在启用 ${s.elevated_read.join('、')} 时读取，不代表普通权限下可见的内容。`));
    if (plan.layout === 'running') out.push(this.runningMore(s));
    if (plan.gaps && (plan.layout === 'partial' || plan.gaps.unknownSizes > 0)) out.push(this.gapsPanel(s, plan));
    if (plan.layout === 'partial' && plan.space) out.push(this.knownPanel(s, plan));
    if (plan.layout === 'completed' && s.issues.length) out.push(this.notesPanel(s));
    if (isTerminal(s.state)) out.push(this.technical(s, plan));
    return out;
  }

  private figure(s: ScanStatus, plan: OverviewPlan): HTMLElement {
    const sp = plan.space!;
    return h(
      'div',
      { class: 'sb-figure' },
      h('p', { class: 'figure-big num', title: formatExactBytes(sp.bytes) }, sp.text),
      h('p', { class: 'figure-cap' }, sp.label, h('span', { class: 'figure-sep', 'aria-hidden': 'true' }), `${groupDigits(s.entries_seen)} 个条目`),
    );
  }

  private actions(s: ScanStatus, plan: OverviewPlan): Child[] {
    const a: Child[] = [];
    if (plan.cancel) {
      const cancel = h('button', { type: 'button', id: 'ov-cancel', class: 'danger-quiet', disabled: s.cancel_requested || s.state === 'stopping' }, s.cancel_requested || s.state === 'stopping' ? '已请求停止' : '取消扫描');
      cancel.addEventListener('click', async () => {
        try {
          this.actionError = null;
          await this.app.cancelScan(s.scan_id);
        } catch (e) {
          this.actionError = toIpcError(e);
          this.update();
        }
      });
      a.push(cancel);
    }
    if (plan.rescan === 'primary') a.push(this.rescanButton(s, 'primary'));
    if (plan.chooseRoot === 'primary') a.push(this.chooseButton('primary', '重新选择扫描范围'));
    if (plan.browse) {
      const disk = h('button', { type: 'button', id: 'ov-disk', class: plan.rescan === 'primary' ? '' : 'primary' }, plan.browse === 'kept' ? '浏览已保留的结果' : plan.pendingPermission ? '浏览已读到的部分' : '浏览磁盘');
      disk.addEventListener('click', () => this.app.navigate('disk'));
      a.push(disk);
    }
    if (plan.candidates) {
      const clean = h('button', { type: 'button', id: 'ov-cleanup' }, '查看清理候选');
      clean.addEventListener('click', () => this.app.navigate('cleanup'));
      a.push(clean);
    }
    if (plan.rescan === 'secondary') a.push(this.rescanButton(s, ''));
    if (plan.chooseRoot === 'secondary') a.push(this.chooseButton('', '选择其他范围'));
    if (plan.layout === 'failed') {
      const prev = this.lastGood(s);
      if (prev) a.push(h('button', { type: 'button', class: 'link', id: 'ov-prev', onclick: () => void this.app.openScan(prev.scan_id, 'overview') }, `查看 ${formatTime(prev.finished_at ?? prev.started_at)} 的扫描结果`));
    }
    return a;
  }

  private chooseButton(kind: string, label: string, id = 'ov-choose'): HTMLElement {
    const b = h('button', { type: 'button', id, class: kind }, label);
    b.addEventListener('click', () => this.app.navigate('scope'));
    return b;
  }

  private rescanButton(s: ScanStatus, kind: string): HTMLElement {
    const b = h('button', { type: 'button', id: 'ov-rescan', class: kind, disabled: this.busy }, '重新扫描');
    b.addEventListener('click', () => void this.rescan(s));
    return b;
  }

  /** Scan the same root again when it is still offered; otherwise choose it on the scope page. */
  private async rescan(s: ScanStatus): Promise<void> {
    this.busy = true;
    this.actionError = null;
    this.update();
    try {
      const offers = await this.app.backend.listScanRoots();
      const o = offers.find((x) => x.path_display === s.root_display && !x.problem);
      if (o) await this.app.startScan(o.token);
      else this.app.navigate('scope');
    } catch (e) {
      this.actionError = toIpcError(e);
    } finally {
      this.busy = false;
      this.update();
    }
  }

  private lastGood(s: ScanStatus): RecentScan | undefined {
    return this.app.store.state.recent.find((r) => r.scan_id !== s.scan_id && r.published && (r.state === 'completed' || r.state === 'partial'));
  }

  private runningMore(s: ScanStatus): HTMLElement {
    return learnMore(
      h('p', {}, '总量在读完之前无法得知，所以这里不显示完成百分比，也不显示大小。'),
      s.hosted_here
        ? h('p', {}, '扫描由本窗口运行。关闭窗口会先请求安全停止，并保留已确认的结果。')
        : h('p', {}, `此扫描由另一个进程运行（${s.host_kind ?? '未知'}，PID ${s.host_pid ?? '?'}），关闭本窗口不会影响它。`),
    );
  }

  /** What was not read (partial) or whose size is unknown. */
  private gapsPanel(s: ScanStatus, plan: OverviewPlan): HTMLElement {
    const g = plan.gaps!;
    const title = plan.pendingPermission ? '等待授权的位置' : g.unread > 0 ? '没有读到的位置' : '大小未知的条目';
    return h(
      'section',
      { class: 'panel gaps-panel', 'data-tone': plan.pendingPermission ? 'pending' : 'warn', 'aria-labelledby': 'ov-gaps-title' },
      h('h2', { id: 'ov-gaps-title' }, title),
      s.issues.length ? issueList(s.issues, { label: '扫描记录的问题' }) : null,
      g.unread > 0 && !s.issues.length ? h('p', {}, `${groupDigits(g.unread)} 处位置无法读取，原因没有记录。`) : null,
      g.unknownSizes > 0 ? h('p', { class: 'gaps-unknown' }, `另有 ${groupDigits(g.unknownSizes)} 个条目大小未知，没有计入已知空间。`) : null,
      s.issues_total > s.issues.length ? h('p', { class: 'muted small' }, `只显示了前 ${groupDigits(s.issues.length)} 条记录（共 ${groupDigits(s.issues_total)} 条）。`) : null,
      learnMore(h('p', {}, '这些位置的大小记为未知，而不是 0：它们不计入已知空间，也不会在磁盘图中占面积。常见原因是权限不足、文件正被占用，或云端文件尚未下载。')),
    );
  }

  /** Partial: the known part after the gaps. */
  private knownPanel(s: ScanStatus, plan: OverviewPlan): HTMLElement {
    return h('section', { class: 'panel known-panel', 'aria-labelledby': 'ov-known-title' }, h('h2', { id: 'ov-known-title' }, '已读到的部分'), this.figure(s, plan));
  }

  /** Completed scans can still note things (e.g. CleanManager's own data was not scanned). */
  private notesPanel(s: ScanStatus): HTMLElement {
    return h('section', { class: 'quiet-section', 'aria-labelledby': 'ov-notes-title' }, h('h2', { id: 'ov-notes-title' }, '扫描备注'), issueList(s.issues, { label: '扫描备注' }));
  }

  private technical(s: ScanStatus, plan: OverviewPlan): HTMLElement {
    const sum = s.summary;
    const rows: [string, Child][] = [];
    if (plan.space && sum) {
      rows.push(['精确值', h('span', { class: 'num' }, formatExactBytes(plan.space.bytes))]);
      rows.push(['分配大小', sum.allocated_exact_bytes !== null ? sizeEl(sum.allocated_exact_bytes) : `未知（已知部分 ${sizeLabel(sum.allocated_known_bytes)}，${groupDigits(sum.allocated_unknown_count)} 项缺少分配信息）`]);
      rows.push(['口径', '逻辑大小是已读取文件的内容长度之和，硬链接按对象只计一次。']);
    }
    rows.push(['已发现条目', h('span', { class: 'num' }, groupDigits(s.entries_seen))]);
    if (plan.layout !== 'failed' && s.completeness) rows.push(['完整性', COMPLETENESS_LABEL[s.completeness] ?? s.completeness]);
    if (plan.reason) rows.push(['代码', h('code', { class: 'selectable' }, plan.reason.rawCode)], ['系统消息', h('span', { class: 'selectable' }, plan.reason.raw || '（无）')]);
    rows.push(['开始于', formatTime(s.started_at)]);
    if (s.finished_at) rows.push(['结束于', formatTime(s.finished_at)]);
    if (s.backend) rows.push(['读取方式', h('span', { class: 'selectable' }, `${s.backend}${s.backend_reason ? `（${s.backend_reason}）` : ''}`)]);
    rows.push(['扫描编号', h('code', { class: 'selectable' }, s.scan_id)]);
    return techBlock(h('dl', { class: 'facts-list small' }, ...rows.flatMap(([k, v]) => [h('dt', {}, k), h('dd', {}, v)])));
  }

  // ---------------------------------------------------------------------------------------------
  // Aside: other scans, machine-wide objects, capabilities

  private aside(plan: OverviewPlan): HTMLElement {
    return h('aside', { class: 'ov-aside', 'aria-label': '其他信息' }, this.recentList(), plan.systemCards ? this.systemObjects() : null, this.hostCard());
  }

  private recentList(): HTMLElement | null {
    const { recent, scanId } = this.app.store.state;
    const others = recent.filter((r) => r.scan_id !== scanId);
    if (!others.length) return null;
    return h(
      'section',
      { class: 'aside-block', 'aria-labelledby': 'recent-title' },
      h('h2', { id: 'recent-title' }, '其他扫描'),
      h(
        'ul',
        { class: 'recent-list' },
        ...others.slice(0, 6).map((r) => {
          const b = h(
            'button',
            { type: 'button', id: `recent-${r.scan_id}`, class: 'recent-item' },
            pathText(r.root_display, { max: 34, className: 'ri-path' }),
            h('span', { class: 'ri-meta' }, h('span', { class: 'state-word', 'data-tone': r.state ? stateTone(r.state) : 'muted' }, h('span', { class: 'sw-mark', 'aria-hidden': 'true' }), r.state ? STATE_LABEL[r.state] : '记录不可用'), formatTime(r.started_at)),
          );
          b.addEventListener('click', () => void this.app.openScan(r.scan_id, 'overview'));
          return h('li', {}, b);
        }),
      ),
      others.length > 6 ? h('button', { type: 'button', class: 'link small', onclick: () => this.app.openActivity('scans') }, `全部 ${groupDigits(others.length)} 次扫描`) : null,
    );
  }

  /** DIN-03: machine-wide objects — clearly apart from the scanned folder. */
  private systemObjects(): HTMLElement | null {
    const d = this.insights;
    if (!d || !d.supported || !d.items.length) return null;
    const open = h('button', { type: 'button', class: 'link small', id: 'ov-insights' }, '查看空间洞察');
    open.addEventListener('click', () => this.app.navigate('insights'));
    return h(
      'section',
      { class: 'aside-block', 'aria-labelledby': 'ov-ins-title' },
      h('h2', { id: 'ov-ins-title' }, '这台电脑的系统对象'),
      h('p', { class: 'muted small' }, '不属于本次扫描范围，由系统或所属工具管理。'),
      h(
        'ul',
        { class: 'sys-list' },
        ...insightCards(d.items).map((c) => h('li', { title: c.observed ? `观测于 ${formatTime(c.observed)}` : '观测时间未知' }, h('span', {}, c.title), h('span', { class: 'num' }, c.size))),
      ),
      open,
    );
  }

  private hostCard(): HTMLElement | null {
    const caps = this.app.store.state.caps;
    if (!caps) return null;
    return h(
      'details',
      { class: 'aside-block host-card' },
      h('summary', {}, '权限与能力'),
      h(
        'dl',
        { class: 'facts-list small' },
        h('dt', {}, '写入操作'),
        h('dd', {}, caps.write_commands ? '只执行你批准（或有限策略覆盖）的已存储计划，每一项执行前重新检查' : '此版本桌面端没有任何删除、移动或写入用户文件的命令'),
        h('dt', {}, '绕过权限读取'),
        h(
          'dd',
          {},
          caps.bypass_privileges_disabled === null
            ? '不适用'
            : caps.bypass_privileges_disabled
              ? '已在启动时关闭（备份/还原特权），扫描以普通权限运行'
              : `未能关闭：${caps.bypass_privileges_note ?? ''}`,
        ),
        h('dt', {}, '索引位置'),
        h('dd', {}, caps.store_dir_display ? pathText(caps.store_dir_display, { max: 30, copy: true }) : '不可用'),
        h('dt', {}, '引擎'),
        h('dd', {}, `${caps.engine_version}（协议 ${caps.schema_version}）`),
      ),
      caps.store_problem ? h('p', { class: 'form-error' }, refusalText(caps.store_problem)) : null,
      h(
        'ul',
        { class: 'cap-list' },
        ...caps.capabilities.map((c) => h('li', {}, h('code', {}, c.name), h('span', { class: 'muted' }, c.state === 'unverified' ? '尚未在真机验证' : c.state))),
      ),
    );
  }
}
