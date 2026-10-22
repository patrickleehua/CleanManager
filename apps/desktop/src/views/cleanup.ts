// P04 — cleanup candidates and the selection draft (GUI-03). Candidates come from the shared
// analysis of a published scan, grouped by risk with reasons, impact and sizes. Nothing is
// pre-selected; unknown sizes never count as 0 or rank as "largest"; blocked, protected and
// pending candidates explain why and cannot be selected. Creating a plan executes nothing.
//
// Order of the page (GUI-09): the summary — one sentence and every group's count and known size —
// then the groups, then the rows; the draft with the processing mode sits in the aside, and the
// shared selection bar carries the same count to the plan review.

import type { App } from '../main';
import { toIpcError } from '../lib/api';
import { formatExactBytes, groupDigits, parseBytes } from '../lib/bytes';
import { protectionText } from '../lib/codes';
import { sizeLabel } from '../lib/display';
import { h, icon, keepFocus, replace } from '../lib/dom';
import { Draft } from '../lib/draft';
import { COMPLETENESS_LABEL, formatTime } from '../lib/format';
import type { Candidate, CandidateSetInfo, Group, IpcError, Mode } from '../lib/types';
import { GROUP_EXPLAIN, GROUP_LABEL, MODE_LABEL, NOT_SELECTABLE_LABEL, PROBE_LABEL, PROBE_STATE_LABEL, REGISTRY_LABEL } from '../lib/words';
import { errorLine, errorText, exactTitle, groupMark, pageHead } from './parts';
import { atLeast, modeOptions, selectionBar, sumSizes, summaryStrip, tierBadge } from './kit';
import { emptyBlock, issueList, learnMore, pathText, stateLine, statusBanner } from './ui';

const PAGE = 50;
const GROUPS: Group[] = ['low', 'review', 'pending', 'blocked'];

/** Words of the cleanup page that are not data (one table for the language switch). */
export const CLEANUP_TEXT = {
  title: '清理',
  candidates: '清理候选',
  noScanTitle: '先扫描一个文件夹',
  noScanBody: '候选只来自已发布的扫描结果；扫描前这里不会显示任何“可清理”数字。',
  chooseScope: '选择扫描范围',
  unpublishedTitle: '扫描还没有发布结果',
  unpublishedBody: '扫描结束（或取消）并发布已确认的部分后才能按规则分析；进行中的数据不参与判断。',
  viewProgress: '查看扫描进度',
  loading: '正在读取分析结果…',
  noRoot: '（范围不可用）',
  analyze: '按规则分析这次扫描',
  analyzing: '正在分析…',
  analyzingLabel: '正在按规则分析，总量未知',
  reanalyze: '重新分析',
  notAnalyzedTitle: '还没有分析这次扫描',
  notAnalyzedBody: '分析只读取扫描索引，用已启用的规则找出缓存、临时文件等候选。',
  notAnalyzedMore: '每个候选都标出风险、影响和保护情况。分析不会触碰任何文件，也不会授权任何操作；生成计划之前没有任何东西会被处理。',
  noneTitle: '当前规则没有找到可处理项',
  noneBody: '这只说明已启用的规则在本次扫描范围内没有匹配，不代表磁盘上没有可以清理的数据。',
  browseDisk: '浏览磁盘',
  summary: (n: number) => `按规则找到 ${groupDigits(n)} 个候选`,
  summarySize: (size: string) => `低风险与需审阅合计 ${size}`,
  summaryNote: '没有预先勾选任何项目；大小未知的不按 0 计算，排在已知大小之后。',
  summaryLabel: '各风险组的候选数与已知大小',
  dismiss: '知道了',
  rulesChanged: '规则包在分析之后已更新，基于旧规则的候选不能执行；请重新分析。',
  partialScope: '这次扫描只覆盖了部分范围：未读取区域里的候选显示为“待判断”，不会进入计划。',
  issues: (n: number) => `分析说明（${n}）`,
  issuesLabel: '记录的问题',
  unknownIn: (n: number) => `${groupDigits(n)} 项未知`,
  selectLow: '选择全部可选的低风险项',
  groupEmpty: '没有这一类候选。',
  loadingMore: '正在加载…',
  showMore: (shown: number, total: number) => `显示更多（已显示 ${shown} / ${total}）`,
  loadingRows: '正在加载候选…',
  hideBasis: '收起依据',
  showBasis: '依据与影响',
  hintTag: '提示',
  adapterTag: '工具清理',
  draft: '选择草稿',
  draftEmpty: '没有预先勾选任何项目。勾选候选后，在这里选择处理方式。',
  draftCount: (n: number) => `${groupDigits(n)} 个候选的已知逻辑大小`,
  draftMore: (n: number) => `另外 ${n} 项`,
  clear: '清空选择',
  freezing: '正在冻结计划…',
  pickFirst: '先选择候选',
  pickMode: '再选择处理方式',
  create: (n: number) => `生成计划（${n} 个候选）`,
  createNote: '生成计划只冻结清单，不会执行；下一步由你逐项审阅并批准。',
} as const;

/** One short note per group in the summary strip. */
const GROUP_NOTE: Record<Group, string> = {
  low: '可以选择',
  review: '逐项确认',
  pending: '证据不完整',
  blocked: '只能查看',
};

interface GroupView {
  items: Candidate[];
  next: string | null;
  total: number;
  loading: boolean;
  error: IpcError | null;
}

export class CleanupPage {
  el: HTMLElement;
  private body: HTMLElement;
  private scanId: string | null = null;
  private info: CandidateSetInfo | null = null;
  private loadingInfo = false;
  private analyzing = false;
  private infoError: IpcError | null = null;
  private groups = new Map<Group, GroupView>();
  private collapsed = new Set<Group>(['blocked']);
  private expanded = new Set<string>();
  private draft: Draft | null = null;
  private mode: Mode | null = null;
  private creating = false;
  private createError: IpcError | null = null;
  private notice: string | null = null;

  constructor(private app: App) {
    this.body = h('div', { class: 'cleanup-body' });
    this.el = h('section', { class: 'page page-cleanup', 'aria-labelledby': 'cleanup-title' }, this.body);
  }

  activate(): void {
    const id = this.app.store.state.scanId;
    if (id !== this.scanId || (!this.info && !this.loadingInfo)) void this.bind(id);
  }

  update(): void {
    const id = this.app.store.state.scanId;
    if (id !== this.scanId) void this.bind(id);
    keepFocus(this.el, () => this.render());
  }

  // ---- data ------------------------------------------------------------------------------

  private async bind(scanId: string | null): Promise<void> {
    this.scanId = scanId;
    this.info = null;
    this.infoError = null;
    this.groups.clear();
    this.draft = null;
    this.mode = null;
    this.expanded.clear();
    if (!scanId) return;
    this.loadingInfo = true;
    this.update();
    try {
      const info = await this.app.backend.candidateSummary(scanId);
      if (this.scanId !== scanId) return;
      this.setInfo(info);
    } catch (e) {
      if (this.scanId === scanId) this.infoError = toIpcError(e);
    } finally {
      this.loadingInfo = false;
      this.update();
    }
  }

  private setInfo(info: CandidateSetInfo | null): void {
    this.info = info;
    this.groups.clear();
    // A new candidate set never inherits an old selection (it may be a different index).
    this.draft = info ? new Draft(info.candidate_set_id) : null;
    this.mode = null;
    if (info) for (const g of GROUPS) if (info.groups[g].count > 0 && !this.collapsed.has(g)) void this.loadGroup(g);
  }

  private async analyze(): Promise<void> {
    if (!this.scanId || this.analyzing) return;
    const scanId = this.scanId;
    this.analyzing = true;
    this.infoError = null;
    this.update();
    this.app.announce('正在按规则分析扫描结果');
    try {
      const info = await this.app.backend.analyzeScan(scanId);
      if (this.scanId !== scanId) return;
      this.setInfo(info);
      const total = GROUPS.reduce((a, g) => a + info.groups[g].count, 0);
      this.app.announce(`分析完成：${total} 个候选，未预先勾选任何项目`);
    } catch (e) {
      this.infoError = toIpcError(e);
    } finally {
      this.analyzing = false;
      this.update();
    }
  }

  private async loadGroup(g: Group, more = false): Promise<void> {
    const info = this.info;
    if (!info) return;
    const cur = this.groups.get(g) ?? { items: [], next: null, total: info.groups[g].count, loading: false, error: null };
    if (cur.loading) return;
    cur.loading = true;
    cur.error = null;
    this.groups.set(g, cur);
    this.update();
    try {
      const page = await this.app.backend.listCandidates(info.candidate_set_id, g, PAGE, more ? cur.next : null);
      if (this.info !== info) return;
      cur.items = more ? [...cur.items, ...page.items] : page.items;
      cur.next = page.next_cursor;
      cur.total = page.total;
    } catch (e) {
      cur.error = toIpcError(e);
    } finally {
      cur.loading = false;
      this.update();
    }
  }

  /** Explicit action: add every selectable low-risk candidate (all pages, known sizes only). */
  private async selectAllLow(): Promise<void> {
    const g = this.groups.get('low');
    while (g && g.next && !g.loading) await this.loadGroup('low', true);
    const items = this.groups.get('low')?.items ?? [];
    let added = 0;
    for (const c of items) if (this.draft && !this.draft.has(c.candidate_id) && this.draft.add(c)) added += 1;
    this.syncMode();
    this.app.announce(added ? `已选择 ${added} 个低风险项目` : '没有可添加的低风险项目');
    this.update();
  }

  private toggle(c: Candidate): void {
    if (!this.draft) return;
    const on = this.draft.toggle(c);
    this.syncMode();
    this.app.announce(on ? `已选择 ${c.name}` : `已取消选择 ${c.name}`);
    this.update();
  }

  private syncMode(): void {
    const modes = this.draft?.modes() ?? [];
    if (this.mode && !modes.includes(this.mode)) this.mode = null;
    this.createError = null;
  }

  private async createPlan(): Promise<void> {
    if (!this.draft || !this.mode || this.creating) return;
    this.creating = true;
    this.createError = null;
    this.update();
    try {
      const r = await this.app.backend.createPlan(this.draft.setId, this.draft.items().map((c) => c.candidate_id), this.mode);
      this.app.announce('计划已生成，尚未执行。请审阅完整清单');
      this.app.openPlan(r.plan_id, 'cleanup');
    } catch (e) {
      this.createError = toIpcError(e);
    } finally {
      this.creating = false;
      this.update();
    }
  }

  private async protect(c: Candidate, what: 'entry' | 'rule'): Promise<void> {
    if (!this.scanId) return;
    try {
      const target =
        what === 'entry'
          ? ({ source: 'entry', scan_id: this.scanId, entry_id: c.entry_id, recursive: c.kind === 'directory' } as const)
          : ({ source: 'rule', rule_id: c.rules[0].split('@')[0] } as const);
      await this.app.backend.addProtection(target, null);
      this.draft?.remove(c.candidate_id);
      this.notice = what === 'entry' ? `已把“${c.name}”加入保护清单。重新分析后它会显示为受保护，执行前也会再次检查。` : `已保护规则 ${c.rules[0].split('@')[0]}。它的所有结果在执行前都会被拦下。`;
    } catch (e) {
      this.notice = `未能加入保护清单：${errorText(toIpcError(e))}`;
    }
    this.update();
  }

  // ---- rendering ---------------------------------------------------------------------------

  private render(): void {
    const st = this.app.store.state;
    const T = CLEANUP_TEXT;
    if (!st.scanId) {
      replace(
        this.body,
        h('h1', { id: 'cleanup-title' }, T.title),
        emptyBlock({ icon: 'tool-clean', title: T.noScanTitle, body: T.noScanBody, actions: [h('button', { type: 'button', class: 'primary', onclick: () => this.app.navigate('scope') }, T.chooseScope)] }),
      );
      return;
    }
    const status = st.status;
    if (status && !status.published) {
      replace(
        this.body,
        this.head(),
        h(
          'div',
          { class: 'ov-single' },
          statusBanner({ kind: 'running', level: 2, titleId: 'cleanup-state-title', title: T.unpublishedTitle, sentence: T.unpublishedBody, actions: [h('button', { type: 'button', onclick: () => this.app.navigate('overview') }, T.viewProgress)] }),
        ),
      );
      return;
    }
    if (this.loadingInfo && !this.info) {
      replace(this.body, this.head(), h('p', { class: 'muted', role: 'status' }, T.loading));
      return;
    }
    if (!this.info) {
      replace(this.body, this.head(), this.notAnalyzed());
      return;
    }
    const total = GROUPS.reduce((a, g) => a + this.info!.groups[g].count, 0);
    replace(
      this.body,
      this.head(),
      total === 0
        ? h(
            'div',
            { class: 'ov-single' },
            this.notices(),
            emptyBlock({ icon: 'tool-clean', title: T.noneTitle, body: T.noneBody, actions: [h('button', { type: 'button', onclick: () => this.app.navigate('disk') }, T.browseDisk)] }),
          )
        : h('div', { class: 'work-layout' }, h('div', { class: 'work-main' }, this.summary(total), this.notices(), ...GROUPS.map((g) => this.groupSection(g))), this.draftPanel()),
      this.bar(),
    );
  }

  private head(): HTMLElement {
    const info = this.info;
    const status = this.app.store.state.status;
    const root = info?.scan_root_display ?? status?.root_display ?? null;
    const completeness = info?.scan_completeness ?? status?.completeness;
    const meta = [
      root ? pathText(root, { max: 48 }) : CLEANUP_TEXT.noRoot,
      info?.created_at ? `分析于 ${formatTime(info.created_at)}` : null,
      completeness ? (COMPLETENESS_LABEL[completeness] ?? '') : null,
    ];
    const reanalyze = info ? h('button', { type: 'button', id: 'cleanup-analyze', disabled: this.analyzing }, this.analyzing ? CLEANUP_TEXT.analyzing : CLEANUP_TEXT.reanalyze) : null;
    reanalyze?.addEventListener('click', () => void this.analyze());
    return pageHead({ id: 'cleanup-title', title: CLEANUP_TEXT.candidates, meta, actions: [reanalyze] });
  }

  private notAnalyzed(): HTMLElement {
    const T = CLEANUP_TEXT;
    const btn = h('button', { type: 'button', id: 'cleanup-analyze', class: 'primary', disabled: this.analyzing }, this.analyzing ? T.analyzing : T.analyze);
    btn.addEventListener('click', () => void this.analyze());
    return h(
      'section',
      { class: 'panel analyze-panel', 'aria-labelledby': 'analyze-title' },
      h('h2', { id: 'analyze-title' }, T.notAnalyzedTitle),
      h('p', {}, T.notAnalyzedBody),
      learnMore(h('p', {}, T.notAnalyzedMore)),
      this.analyzing ? h('div', { class: 'indeterminate', role: 'progressbar', 'aria-label': T.analyzingLabel, 'aria-valuetext': T.analyzingLabel }) : null,
      errorLine(this.infoError),
      h('div', { class: 'card-actions' }, btn),
    );
  }

  /** GUI-09 summary first: every group's count and known size, each jumping to its group. */
  private summary(total: number): HTMLElement {
    const info = this.info!;
    const T = CLEANUP_TEXT;
    const known = GROUPS.filter((g) => g === 'low' || g === 'review').reduce((a, g) => a + (parseBytes(info.groups[g].known_logical_bytes) ?? 0n), 0n);
    const unknown = GROUPS.filter((g) => g === 'low' || g === 'review').reduce((a, g) => a + info.groups[g].unknown_size_count, 0);
    return h(
      'section',
      { class: 'cand-summary', 'aria-labelledby': 'cand-sum-title' },
      h('h2', { id: 'cand-sum-title', class: 'cs-sentence' }, T.summary(total), h('span', { class: 'cs-size num' }, T.summarySize(sizeLabel(known, { unknownCount: unknown })))),
      h('p', { class: 'cs-note' }, T.summaryNote),
      summaryStrip(
        T.summaryLabel,
        GROUPS.map((g) => {
          const c = info.groups[g];
          return {
            mark: groupMark(g),
            count: c.count,
            size: c.count && (g === 'low' || g === 'review') ? sizeLabel(c.known_logical_bytes, { unknownCount: c.unknown_size_count }) : null,
            note: GROUP_NOTE[g],
            id: `jump-${g}`,
            onJump: () => this.jumpTo(g),
          };
        }),
      ),
    );
  }

  private jumpTo(g: Group): void {
    if (this.collapsed.has(g)) {
      this.collapsed.delete(g);
      if (!this.groups.get(g) && this.info?.groups[g].count) void this.loadGroup(g);
      this.update();
    }
    requestAnimationFrame(() => {
      const t = document.getElementById(`group-toggle-${g}`);
      t?.scrollIntoView({ block: 'start', behavior: 'smooth' });
      t?.focus({ preventScroll: true });
    });
  }

  private notices(): HTMLElement {
    const info = this.info!;
    const T = CLEANUP_TEXT;
    const parts: (HTMLElement | null)[] = [];
    if (this.notice) {
      const close = h('button', { type: 'button', class: 'quiet', onclick: () => ((this.notice = null), this.update()) }, T.dismiss);
      parts.push(h('div', { class: 'notice', role: 'status' }, h('span', {}, this.notice), close));
    }
    if (info.rules_current === false) parts.push(stateLine({ kind: 'failed', text: T.rulesChanged }));
    if (info.scan_completeness === 'partial_scope') parts.push(stateLine({ kind: 'partial', text: T.partialScope }));
    if (this.infoError) parts.push(errorLine(this.infoError));
    if (info.issues.length) parts.push(h('details', { class: 'issues-details' }, h('summary', {}, T.issues(info.issues.length)), issueList(info.issues, { label: T.issuesLabel })));
    return h('div', { class: 'notices' }, ...parts);
  }

  private groupSection(g: Group): HTMLElement {
    const info = this.info!;
    const count = info.groups[g];
    const view = this.groups.get(g);
    const open = !this.collapsed.has(g);
    const T = CLEANUP_TEXT;
    const toggle = h(
      'button',
      { type: 'button', class: 'group-toggle', id: `group-toggle-${g}`, 'aria-expanded': open ? 'true' : 'false', 'aria-controls': `group-body-${g}` },
      h('span', { class: 'chev-icon', 'aria-hidden': 'true' }, icon('chevron')),
      groupMark(g),
      h('span', { class: 'group-count num' }, `${groupDigits(count.count)} 项`),
      count.count
        ? h(
            'span',
            { class: 'group-size num', title: exactTitle(count.known_logical_bytes) },
            sizeLabel(count.known_logical_bytes, { unknownCount: count.unknown_size_count }),
            count.unknown_size_count ? h('span', { class: 'group-unknown' }, T.unknownIn(count.unknown_size_count)) : null,
          )
        : null,
    );
    toggle.addEventListener('click', () => {
      if (open) this.collapsed.add(g);
      else {
        this.collapsed.delete(g);
        if (!this.groups.get(g) && count.count) void this.loadGroup(g);
      }
      this.update();
    });
    const extra = g === 'low' && count.count > 0 ? h('button', { type: 'button', class: 'quiet', id: 'select-low', onclick: () => void this.selectAllLow() }, T.selectLow) : null;
    const bodyEl = h(
      'div',
      { class: 'group-body', id: `group-body-${g}`, hidden: !open },
      h('p', { class: 'group-explain' }, GROUP_EXPLAIN[g]),
      count.count === 0 ? h('p', { class: 'muted small' }, T.groupEmpty) : this.table(g, view),
      view?.error ? errorLine(view.error) : null,
      view?.next
        ? h('button', { type: 'button', class: 'quiet more', id: `more-${g}`, disabled: view.loading, onclick: () => void this.loadGroup(g, true) }, view.loading ? T.loadingMore : T.showMore(view.items.length, view.total))
        : null,
    );
    return h('section', { class: 'panel cgroup', 'data-group': g, 'aria-labelledby': `group-toggle-${g}` }, h('div', { class: 'cgroup-head' }, h('h2', {}, toggle), extra), bodyEl);
  }

  private table(g: Group, view: GroupView | undefined): HTMLElement {
    const T = CLEANUP_TEXT;
    if (!view || (view.loading && !view.items.length)) return h('p', { class: 'muted small', role: 'status' }, T.loadingRows);
    const rows: HTMLElement[] = [];
    for (const c of view.items) {
      const id = `cand-${c.candidate_id}`;
      const selectable = c.not_selectable === null && c.allowed_modes.length > 0;
      const check = h('input', {
        type: 'checkbox',
        id: `${id}-check`,
        disabled: !selectable,
        'aria-describedby': `${id}-why`,
        'aria-label': `选择 ${c.name}`,
      }) as HTMLInputElement;
      check.checked = !!this.draft?.has(c.candidate_id);
      check.addEventListener('change', () => this.toggle(c));
      const open = this.expanded.has(c.candidate_id);
      const more = h('button', { type: 'button', class: 'link-btn', id: `${id}-more`, 'aria-expanded': open ? 'true' : 'false', 'aria-controls': `${id}-detail` }, open ? T.hideBasis : T.showBasis);
      more.addEventListener('click', () => {
        if (open) this.expanded.delete(c.candidate_id);
        else this.expanded.add(c.candidate_id);
        this.update();
      });
      const size = h(
        'td',
        { class: 'col-size num', title: exactTitle(c.logical_known_bytes) },
        sizeLabel(c.logical_known_bytes, { unknownCount: c.unknown_size_count }),
        c.unknown_size_count ? h('small', {}, T.unknownIn(c.unknown_size_count)) : null,
      );
      const why = selectable
        ? h('span', { id: `${id}-why`, class: 'modes-line' }, c.allowed_modes.map((m) => MODE_LABEL[m]).join('、'))
        : h('span', { id: `${id}-why`, class: 'why-not' }, NOT_SELECTABLE_LABEL[c.not_selectable ?? 'no_mode']);
      rows.push(
        h(
          'tr',
          { class: `crow${check.checked ? ' selected' : ''}${selectable ? '' : ' locked'}` },
          h('td', { class: 'col-check' }, check),
          h(
            'td',
            { class: 'col-object' },
            h('label', { for: `${id}-check`, class: 'obj-name' }, c.name),
            pathText(c.path_display, { max: 64, className: 'obj-path' }),
            h('span', { class: 'obj-reason' }, c.row_kind === 'hint' ? h('span', { class: 'row-tag' }, T.hintTag) : null, c.row_kind === 'adapter' ? h('span', { class: 'row-tag' }, T.adapterTag) : null, c.reasons[0] ?? c.rules[0]),
          ),
          size,
          h('td', { class: 'col-status' }, tierBadge(c.tier), why, more),
        ),
      );
      if (open) rows.push(h('tr', { class: 'cdetail', id: `${id}-detail` }, h('td', { colspan: '4' }, this.detail(c))));
    }
    return h(
      'div',
      { class: 'table-wrap' },
      h(
        'table',
        { class: 'data-table ctable' },
        h('caption', { class: 'sr-only' }, `${GROUP_LABEL[g]}候选，按已知大小排序；大小未知的排在后面`),
        h('thead', {}, h('tr', {}, h('th', { scope: 'col', class: 'col-check' }, h('span', { class: 'sr-only' }, '选择')), h('th', { scope: 'col' }, '对象与依据'), h('th', { scope: 'col', class: 'col-size' }, '逻辑大小'), h('th', { scope: 'col' }, '可用方式'))),
        h('tbody', {}, ...rows),
      ),
    );
  }

  private detail(c: Candidate): HTMLElement {
    const protectEntry = h('button', { type: 'button', class: 'quiet', onclick: () => void this.protect(c, 'entry') }, icon('shield'), '保护此位置');
    const protectRule = h('button', { type: 'button', class: 'quiet', onclick: () => void this.protect(c, 'rule') }, icon('shield'), '保护整条规则');
    return h(
      'div',
      { class: 'cdetail-body' },
      h(
        'dl',
        { class: 'facts-list' },
        h('dt', {}, '规则'),
        h('dd', {}, h('code', { class: 'selectable' }, c.rules.join('，'))),
        h('dt', {}, '理由'),
        h('dd', {}, c.reasons.length ? c.reasons.join('；') : '规则匹配'),
        h('dt', {}, '影响'),
        h('dd', {}, c.impact || '规则未提供影响说明'),
        c.protection ? h('dt', {}, '保护') : null,
        c.protection ? h('dd', {}, protectionText(c.protection.class), c.protection.reviewable ? '（仅可逐项审阅，不进入批量计划或策略）' : '') : null,
        c.unknown_size_count ? h('dt', {}, '未知大小') : null,
        c.unknown_size_count ? h('dd', {}, `${groupDigits(c.unknown_size_count)} 个文件大小未知，未计入上面的数字，也不能冻结进计划。`) : null,
        c.registry_status ? h('dt', {}, '实测登记') : null,
        c.registry_status ? h('dd', {}, REGISTRY_LABEL[c.registry_status] ?? c.registry_status, c.installed_version ? h('small', {}, `已安装版本 ${c.installed_version}`) : null) : null,
        c.probes.length ? h('dt', {}, '占用检查') : null,
        c.probes.length
          ? h(
              'dd',
              {},
              c.probe_results.length
                ? h('ul', { class: 'probe-list' }, ...c.probe_results.map((r) => h('li', {}, h('span', { class: 'probe-state', 'data-state': r.state }, PROBE_STATE_LABEL[r.state] ?? r.state), ` ${PROBE_LABEL[r.probe] ?? r.probe}`)))
                : c.catalog_fresh
                  ? '本次分析未运行检查'
                  : '需要重新分析才有本次的检查结果',
              h('small', {}, '执行前每一项会再次检查；无法确认时按占用处理。'),
            )
          : null,
        c.row_kind === 'adapter' ? h('dt', {}, '清理方式') : null,
        c.row_kind === 'adapter' ? h('dd', {}, `由所属工具（${c.adapter_ref ?? '未知'}）自己清理；此版本只显示预览，不会直接删除目录。`) : null,
        c.row_kind === 'hint' ? h('dt', {}, '提示') : null,
        c.row_kind === 'hint' ? h('dd', {}, '这是大项提示，用于帮你决定是否手动处理；它永远不会进入计划。') : null,
      ),
      h('div', { class: 'card-actions' }, protectEntry, c.rules.length ? protectRule : null, this.diagButton(c)),
    );
  }

  /** Optional AI diagnosis of this candidate's object; the plan path stays on this page's rules. */
  private diagButton(c: Candidate): HTMLElement | null {
    const scanId = this.app.store.state.scanId;
    if (!scanId) return null;
    const name = c.name;
    return h('button', { type: 'button', class: 'quiet', id: `diag-${c.candidate_id}`, onclick: () => this.app.openDiagnosis({ scanId, entryId: c.entry_id, origin: 'cleanup', name }, 'cleanup') }, icon('diag'), 'AI 诊断');
  }

  /** The shared bottom selection bar (GUI-09): count, known size, unknown count, next step. */
  private bar(): HTMLElement {
    const items = this.draft?.items() ?? [];
    return selectionBar({
      count: items.length,
      sum: sumSizes(items.map((c) => ({ bytes: c.logical_known_bytes, unknown: c.unknown_size_count }))),
      onClear: () => {
        this.draft?.clear();
        this.syncMode();
        this.app.announce('已清空选择');
        this.update();
      },
      action: {
        id: 'cleanup-bar-go',
        text: this.mode ? '生成计划并审阅' : '选择处理方式',
        disabled: this.creating,
        danger: this.mode === 'delete',
        onClick: () => {
          if (this.mode) void this.createPlan();
          else document.querySelector<HTMLInputElement>('input[name="plan-mode"]:not(:disabled)')?.focus();
        },
      },
    });
  }

  private draftPanel(): HTMLElement {
    const T = CLEANUP_TEXT;
    const d = this.draft;
    const n = d?.size ?? 0;
    const modes = d?.modes() ?? [];
    const create = h(
      'button',
      { type: 'button', class: `${this.mode === 'delete' ? 'danger' : 'primary'} wide`, id: 'create-plan', disabled: n === 0 || !this.mode || this.creating },
      this.creating ? T.freezing : n === 0 ? T.pickFirst : !this.mode ? T.pickMode : T.create(n),
    );
    create.addEventListener('click', () => void this.createPlan());
    const list = d?.items() ?? [];
    const sum = sumSizes(list.map((c) => ({ bytes: c.logical_known_bytes, unknown: c.unknown_size_count })));
    return h(
      'aside',
      { class: 'panel work-aside draft', 'aria-labelledby': 'draft-title' },
      h('h2', { id: 'draft-title' }, T.draft),
      n === 0
        ? h('p', { class: 'muted small' }, T.draftEmpty)
        : h(
            'div',
            { class: 'draft-sum' },
            h('p', { class: 'draft-figure num', title: formatExactBytes(sum.known) }, atLeast(sum)),
            h('p', { class: 'muted small' }, T.draftCount(n)),
            h(
              'ul',
              { class: 'draft-list' },
              ...list.slice(0, 5).map((c) =>
                h('li', {}, h('span', { class: 'path' }, c.name), h('button', { type: 'button', class: 'icon-btn small-btn', 'aria-label': `移除 ${c.name}`, onclick: () => this.toggle(c) }, icon('close'))),
              ),
              list.length > 5 ? h('li', { class: 'muted small' }, T.draftMore(list.length - 5)) : null,
            ),
            h('button', { type: 'button', class: 'quiet', onclick: () => (this.draft?.clear(), this.syncMode(), this.update()) }, T.clear),
          ),
      modeOptions({
        name: 'plan-mode',
        id: (m) => `mode-${m}`,
        allowed: modes,
        current: this.mode,
        selected: n,
        onChange: (m) => {
          this.mode = m;
          this.createError = null;
          this.update();
        },
      }),
      errorLine(this.createError),
      create,
      h('p', { class: 'muted small aside-note' }, T.createNote),
    );
  }
}
