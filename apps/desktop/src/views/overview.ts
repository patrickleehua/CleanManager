// P02 — overview of the scan this window is bound to: lifecycle state in words, what is known,
// what is not (unread ranges, unknown sizes) and where the numbers come from. No health score,
// no "0 B junk" before a scan, no percentage of an unknown total.

import type { App } from '../main';
import type { InsightsList } from '../lib/types';
import { insightCards } from './tools';
import { toIpcError } from '../lib/api';
import { formatBytes, formatExactBytes, groupDigits, parseBytes } from '../lib/bytes';
import { h, keepFocus, replace } from '../lib/dom';
import { COMPLETENESS_LABEL, ERROR_HINT, formatElapsed, formatTime, isTerminal, STATE_EXPLAIN, STATE_LABEL, stateTone } from '../lib/format';
import type { ScanStatus } from '../lib/types';

export class OverviewPage {
  el: HTMLElement;
  private body: HTMLElement;
  private cancelError: string | null = null;
  private timer: number | null = null;
  private insights: InsightsList | null = null;
  private insightsRequested = false;

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

  /** DIN-03: known large system objects as cards — known size or "unknown", with observation time. */
  private insightStrip(): HTMLElement | null {
    const d = this.insights;
    if (!d || !d.supported || !d.items.length) return null;
    const open = h('button', { type: 'button', class: 'quiet', id: 'ov-insights' }, '查看空间洞察');
    open.addEventListener('click', () => this.app.navigate('insights'));
    return h(
      'section',
      { class: 'panel insight-strip', 'aria-labelledby': 'ov-ins-title' },
      h('div', { class: 'panel-head' }, h('h2', { id: 'ov-ins-title' }, '系统与虚拟化对象'), open),
      h(
        'ul',
        { class: 'insight-cards' },
        ...insightCards(d.items).map((c) => h('li', { class: 'icard' }, h('span', { class: 'icard-title' }, c.title), h('span', { class: 'icard-size num' }, c.size), h('span', { class: 'icard-meta muted' }, c.observed ? `观测于 ${formatTime(c.observed)}` : '观测时间未知'))),
      ),
      h('p', { class: 'muted small' }, '由系统或所属工具管理，只提供说明与官方工具用法，不提供删除。'),
    );
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
    if (el && s) el.textContent = formatElapsed(s.started_at);
  }

  private render(): void {
    const st = this.app.store.state;
    if (!st.scanId) {
      replace(this.body, this.emptyState());
      return;
    }
    if (st.statusError && !st.status) {
      replace(
        this.body,
        h('h1', { id: 'overview-title' }, '概览'),
        h('div', { class: 'panel' }, h('p', { class: 'form-error', role: 'alert' }, ERROR_HINT[st.statusError.reason] ?? st.statusError.message), this.newScanButton()),
      );
      return;
    }
    if (!st.status) {
      replace(this.body, h('h1', { id: 'overview-title' }, '概览'), h('p', { class: 'muted', role: 'status' }, '正在读取扫描状态…'));
      return;
    }
    const s = st.status;
    replace(
      this.body,
      this.heading(s),
      h('div', { class: 'overview-grid' }, this.stateCard(s), this.knownCard(s), this.coverageCard(s)),
      this.insightStrip(),
      this.issues(s),
      this.recentList(),
      this.hostCard(),
    );
  }

  private emptyState(): HTMLElement {
    return h(
      'div',
      { class: 'empty' },
      h('h1', { id: 'overview-title' }, '还没有扫描结果'),
      h('p', { class: 'lede' }, '选择一个文件夹并开始只读扫描后，这里会显示它占用了多少空间、哪些位置没能读取，以及结果的可信程度。'),
      this.newScanButton(true),
      this.insightStrip(),
    );
  }

  private newScanButton(primary = false): HTMLElement {
    const b = h('button', { type: 'button', id: 'ov-new', class: primary ? 'primary' : '' }, primary ? '选择扫描范围' : '新建扫描');
    b.addEventListener('click', () => this.app.navigate('scope'));
    return b;
  }

  private heading(s: ScanStatus): HTMLElement {
    return h(
      'div',
      { class: 'page-head' },
      h(
        'div',
        {},
        h('h1', { id: 'overview-title' }, '概览'),
        h('p', { class: 'scope-line' }, h('span', { class: 'muted' }, '范围 '), h('span', { class: 'path' }, s.root_display ?? '（范围路径不可用）')),
      ),
      h('div', { class: 'head-actions' }, this.newScanButton()),
    );
  }

  private stateCard(s: ScanStatus): HTMLElement {
    const running = !isTerminal(s.state);
    const ownedElsewhere = running && !s.hosted_here;
    const actions: HTMLElement[] = [];
    if (running) {
      const cancel = h('button', { type: 'button', id: 'ov-cancel', class: 'danger-quiet', disabled: s.cancel_requested || s.state === 'stopping' }, s.cancel_requested ? '已请求停止' : '取消扫描');
      cancel.addEventListener('click', async () => {
        try {
          this.cancelError = null;
          await this.app.cancelScan(s.scan_id);
        } catch (e) {
          this.cancelError = toIpcError(e).message;
          this.update();
        }
      });
      actions.push(cancel);
    }
    const disk = h('button', { type: 'button', id: 'ov-disk', class: running ? '' : 'primary' }, '浏览磁盘');
    disk.addEventListener('click', () => this.app.navigate('disk'));
    actions.push(disk);
    if (s.published) {
      const clean = h('button', { type: 'button', id: 'ov-cleanup' }, '查看清理候选');
      clean.addEventListener('click', () => this.app.navigate('cleanup'));
      actions.push(clean);
    }

    return h(
      'article',
      { class: 'panel state-card', 'data-tone': stateTone(s.state), 'aria-labelledby': 'state-title' },
      h('h2', { id: 'state-title', class: 'state-title' }, h('span', { class: 'state-dot', 'aria-hidden': 'true' }), STATE_LABEL[s.state]),
      h('p', {}, STATE_EXPLAIN[s.state]),
      running ? h('div', { class: 'indeterminate', role: 'progressbar', 'aria-label': '扫描进度（总量未知）', 'aria-busy': 'true' }) : null,
      h(
        'dl',
        { class: 'facts-list' },
        h('dt', {}, '已发现条目'),
        h('dd', { class: 'num' }, groupDigits(s.entries_seen)),
        h('dt', {}, '未能读取的范围'),
        h('dd', { class: 'num' }, groupDigits(s.skipped_ranges)),
        h('dt', {}, running ? '已用时间' : '用时'),
        h('dd', { 'data-elapsed': running ? '1' : null }, formatElapsed(s.started_at, s.finished_at)),
        h('dt', {}, '开始于'),
        h('dd', {}, formatTime(s.started_at)),
        s.finished_at ? h('dt', {}, '结束于') : null,
        s.finished_at ? h('dd', {}, formatTime(s.finished_at)) : null,
      ),
      ownedElsewhere ? h('p', { class: 'muted' }, `此扫描由另一个进程运行（${s.host_kind ?? '未知'}，PID ${s.host_pid ?? '?'}）。关闭本窗口不会影响它。`) : null,
      s.hosted_here && running ? h('p', { class: 'muted' }, '扫描由本窗口运行。切换页面不会中断它；关闭窗口会先请求安全停止，并保留已确认的结果。') : null,
      this.cancelError ? h('p', { class: 'form-error', role: 'alert' }, this.cancelError) : null,
      h('div', { class: 'card-actions' }, ...actions),
    );
  }

  private knownCard(s: ScanStatus): HTMLElement {
    if (!s.published) {
      return h(
        'article',
        { class: 'panel', 'aria-labelledby': 'known-title' },
        h('h2', { id: 'known-title' }, '已知空间'),
        h('p', { class: 'muted' }, '扫描发布结果后才显示大小。进行中的总数一直在变化，这里不展示会误导人的半成品数字。'),
      );
    }
    const sum = s.summary;
    const logical = sum ? parseBytes(sum.logical_known_bytes) : null;
    const allocatedExact = sum?.allocated_exact_bytes ?? null;
    return h(
      'article',
      { class: 'panel known-card', 'aria-labelledby': 'known-title' },
      h('h2', { id: 'known-title' }, '已知空间'),
      h('p', { class: 'big-number', title: formatExactBytes(logical) }, logical === null ? '—' : formatBytes(logical)),
      h('p', { class: 'muted' }, '逻辑大小：所有已读取文件的内容长度之和，硬链接按对象只计一次。'),
      h(
        'dl',
        { class: 'facts-list' },
        h('dt', {}, '精确值'),
        h('dd', { class: 'num' }, formatExactBytes(logical)),
        h('dt', {}, '分配大小'),
        h(
          'dd',
          { class: 'num' },
          allocatedExact !== null
            ? formatBytes(allocatedExact)
            : sum
              ? `未知（已知部分 ${formatBytes(sum.allocated_known_bytes)}，${groupDigits(sum.allocated_unknown_count)} 项缺少分配信息）`
              : '未知',
        ),
        h('dt', {}, '条目'),
        h('dd', { class: 'num' }, groupDigits(s.entries_seen)),
        h('dt', {}, '完整性'),
        h('dd', {}, s.completeness ? (COMPLETENESS_LABEL[s.completeness] ?? s.completeness) : '—'),
      ),
    );
  }

  private coverageCard(s: ScanStatus): HTMLElement {
    const unknown = s.summary?.logical_unknown_count ?? 0;
    const unread = s.summary?.unread_ranges ?? s.skipped_ranges;
    const clean = s.published && unknown === 0 && unread === 0;
    return h(
      'article',
      { class: 'panel coverage-card', 'aria-labelledby': 'coverage-title' },
      h('h2', { id: 'coverage-title' }, '覆盖与缺口'),
      !s.published
        ? h('p', { class: 'muted' }, '扫描结束后会在这里列出无法读取的位置和大小未知的条目。')
        : clean
          ? h('p', {}, '本次范围内没有记录到无法读取的位置或未知大小。这不代表范围之外的磁盘内容。')
          : h(
              'div',
              {},
              h(
                'dl',
                { class: 'facts-list' },
                h('dt', {}, '无法读取的范围'),
                h('dd', { class: 'num' }, groupDigits(unread)),
                h('dt', {}, '大小未知的条目'),
                h('dd', { class: 'num' }, groupDigits(Math.max(0, unknown - unread))),
              ),
              h(
                'p',
                { class: 'muted' },
                '这些位置的大小是未知，而不是 0：它们没有计入上面的已知空间，也不会在磁盘图中获得面积。常见原因是权限不足、文件正被占用，或云端文件尚未下载。',
              ),
            ),
      s.elevated_read.length
        ? h('p', { class: 'notice warn' }, `此结果在启用 ${s.elevated_read.join('、')} 时读取，不代表普通权限下可见的内容。`)
        : null,
      s.backend ? h('p', { class: 'muted small' }, `读取方式：${s.backend}${s.backend_reason ? `（${s.backend_reason}）` : ''}`) : null,
    );
  }

  private issues(s: ScanStatus): HTMLElement | null {
    if (!s.issues.length) return null;
    return h(
      'section',
      { class: 'panel issues', 'aria-labelledby': 'issues-title' },
      h('h2', { id: 'issues-title' }, `扫描记录的问题（${groupDigits(s.issues_total)}）`),
      h(
        'ul',
        { class: 'issue-list' },
        ...s.issues.map((i) => h('li', {}, h('span', { class: 'issue-code' }, i.code), h('span', { class: 'path' }, i.scope_display), h('span', { class: 'muted' }, i.message))),
      ),
      s.issues_total > s.issues.length ? h('p', { class: 'muted small' }, `仅显示前 ${s.issues.length} 条。`) : null,
    );
  }

  private recentList(): HTMLElement | null {
    const { recent, scanId } = this.app.store.state;
    const others = recent.filter((r) => r.scan_id !== scanId);
    if (!others.length) return null;
    return h(
      'section',
      { class: 'panel recent', 'aria-labelledby': 'recent-title' },
      h('h2', { id: 'recent-title' }, '其他扫描'),
      h(
        'ul',
        { class: 'recent-list' },
        ...others.slice(0, 8).map((r) => {
          const b = h(
            'button',
            { type: 'button', id: `recent-${r.scan_id}`, class: 'recent-item' },
            h('span', { class: 'path' }, r.root_display),
            h('span', { class: 'muted' }, `${formatTime(r.started_at)} · ${r.state ? STATE_LABEL[r.state] : '记录不可用'}`),
          );
          b.addEventListener('click', () => void this.app.openScan(r.scan_id, 'overview'));
          return h('li', {}, b);
        }),
      ),
    );
  }

  private hostCard(): HTMLElement | null {
    const caps = this.app.store.state.caps;
    if (!caps) return null;
    return h(
      'details',
      { class: 'panel host-card' },
      h('summary', {}, '权限与能力'),
      h(
        'dl',
        { class: 'facts-list' },
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
        h('dd', { class: 'path' }, caps.store_dir_display ?? '不可用'),
        h('dt', {}, '引擎'),
        h('dd', {}, `${caps.engine_version}（协议 ${caps.schema_version}）`),
      ),
      caps.store_problem ? h('p', { class: 'form-error' }, ERROR_HINT[caps.store_problem.reason] ?? caps.store_problem.message) : null,
      h(
        'ul',
        { class: 'cap-list' },
        ...caps.capabilities.map((c) => h('li', {}, h('code', {}, c.name), h('span', { class: 'muted' }, c.state === 'unverified' ? '尚未在真机验证' : c.state))),
      ),
    );
  }
}
