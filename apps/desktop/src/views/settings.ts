// P10 — capabilities, protection list and finite policies (PRT-*, PLN-05/06, GUI-04).
// OS read permission, write permission and product authorization are stated separately.
// Protection: add from host-held sources only; removal needs the entry id typed; built-in
// layers cannot be removed. Policies: none by default, created paused, enabling needs the id
// typed, revoking states its impact. A failed save leaves the stored settings unchanged.

import type { App } from '../main';
import { toIpcError } from '../lib/api';
import { formatBytes, groupDigits } from '../lib/bytes';
import { h, keepFocus, replace } from '../lib/dom';
import { gatedOff } from '../lib/nav';
import { words } from '../lib/platform';
import { formatTime } from '../lib/format';
import type { GateRow, IpcError, Policy, ProtectionItem, ProtectionList } from '../lib/types';
import { LAYER_LABEL, MATCH_LABEL, MODE_LABEL, POLICY_STATE_LABEL } from '../lib/words';
import { DiagSettings } from './diagSettings';
import { confirmInline, copyField, errorLine, errorText, pageHead, pill } from './parts';
import { protectionText, refusalText, unknownCodeText } from '../lib/codes';
import { learnMore, techBlock } from './ui';

/** Words of the settings page that are not data (one table for the language switch). */
const SETTINGS_TEXT = {
  experimentalLede: '下面这些工具还没有通过发布前验收，默认关闭；开启后只供你在这台电脑上评估。',
  capsSummary: (n: number, unverified: number) => (unverified ? `${n} 项读取与执行能力，其中 ${unverified} 项尚未在真机验证。` : `${n} 项读取与执行能力。`),
} as const;

type Pending = { kind: 'remove'; id: string } | { kind: 'enable'; id: string } | { kind: 'revoke'; id: string } | null;

/** What each gated capability is called where people meet it (D15). */
const CAPABILITY_LABEL: Record<string, () => string> = {
  'clean.catalog': () => '清理目录（命令行 clean）',
  'purge.artifacts': () => '构建产物',
  'installer.cleanup': () => '安装包',
  'trash.manual': () => `手选${words().bin}`,
  'insights.overview': () => '空间洞察',
  'apps.uninstall': () => '卸载',
  'apps.updates': () => '更新',
  'startup.toggle': () => '启动项',
  'maintenance.run': () => '维护任务与性能诊断',
  'status.metrics': () => '系统状态',
  'process.end': () => '进程',
  'resident.utilities': () => '常驻工具',
  'lifecycle.self_update': () => '自我更新（命令行 self update）',
  'diag.ai': () => 'AI 诊断',
};

const SECTIONS = [
  { id: 'set-caps', label: '能力与权限' },
  { id: 'set-experimental', label: '实验功能' },
  { id: 'set-protection', label: '保护清单' },
  { id: 'set-policies', label: '有限策略' },
  { id: 'set-data', label: '数据与 CLI' },
  { id: 'set-diag', label: 'AI 诊断' },
];

export class SettingsPage {
  el: HTMLElement;
  private body: HTMLElement;
  private protection: ProtectionList | null = null;
  private protectionError: IpcError | null = null;
  private policies: Policy[] | null = null;
  private policyError: IpcError | null = null;
  private pending: Pending = null;
  private saveError: string | null = null;
  private saved: string | null = null;
  private busy = false;
  private diag: DiagSettings;

  constructor(private app: App) {
    this.diag = new DiagSettings(app);
    this.body = h('div', { class: 'settings-body' });
    this.el = h('section', { class: 'page page-settings', 'aria-labelledby': 'settings-title' }, this.body);
  }

  activate(): void {
    void this.reload();
    if (this.diagOn()) void this.diag.reload();
  }

  private diagOn(): boolean {
    return !gatedOff('diag', this.app.store.state.caps?.gates);
  }

  update(): void {
    keepFocus(this.el, () => this.render());
  }

  private async reload(): Promise<void> {
    const [p, pol] = await Promise.allSettled([this.app.backend.listProtection(), this.app.backend.listPolicies()]);
    if (p.status === 'fulfilled') {
      this.protection = p.value;
      this.protectionError = null;
    } else this.protectionError = toIpcError(p.reason);
    if (pol.status === 'fulfilled') {
      this.policies = pol.value;
      this.policyError = null;
    } else this.policyError = toIpcError(pol.reason);
    this.update();
  }

  /** Run a settings change; on failure the stored list is re-read (it was not changed). */
  private async change(work: () => Promise<void>, ok: string): Promise<void> {
    this.busy = true;
    this.saveError = null;
    this.saved = null;
    this.update();
    try {
      await work();
      this.pending = null;
      this.saved = ok;
      this.app.announce(ok);
    } catch (e) {
      const err = toIpcError(e);
      this.saveError = `未保存：${errorText(err)}。已生效的设置保持不变。`;
      await this.reload();
      throw err;
    } finally {
      this.busy = false;
      this.update();
    }
  }

  private async addFolder(): Promise<void> {
    try {
      const offer = await this.app.backend.chooseFolder();
      if (!offer) return;
      await this.change(async () => {
        this.protection = await this.app.backend.addProtection({ source: 'folder', root_token: offer.token }, null);
      }, `已保护 ${offer.path_display}`);
    } catch {
      /* reported by change() */
    }
  }

  private render(): void {
    const nav = h(
      'nav',
      { class: 'local-nav', 'aria-label': '设置分区' },
      ...SECTIONS.map((s) =>
        h(
          'button',
          {
            type: 'button',
            class: 'local-link',
            onclick: () => {
              const target = document.getElementById(`${s.id}-title`);
              target?.scrollIntoView({ behavior: 'smooth', block: 'start' });
              target?.focus({ preventScroll: true });
            },
          },
          s.label,
        ),
      ),
    );
    replace(
      this.body,
      pageHead({ id: 'settings-title', title: '设置' }),
      this.saveError ? h('div', { class: 'notice warn', role: 'alert' }, h('span', {}, this.saveError)) : null,
      this.saved ? h('div', { class: 'notice', role: 'status' }, h('span', {}, this.saved)) : null,
      h(
        'div',
        { class: 'settings-layout' },
        nav,
        h(
          'div',
          { class: 'settings-main' },
          this.caps(),
          this.experimentalSection(),
          this.protectionSection(),
          this.policySection(),
          this.dataSection(),
          this.diagOn()
            ? this.diag.el
            : this.section('set-diag', 'AI 诊断（可选）', h('p', { class: 'muted' }, 'AI 诊断尚未通过发布门禁 AID，默认关闭。开启上方的实验功能后，可以在这里配置提供方。')),
        ),
      ),
    );
  }

  private section(id: string, title: string, ...children: (Node | null | false)[]): HTMLElement {
    return h('section', { class: 'panel settings-section', id, 'aria-labelledby': `${id}-title` }, h('h2', { id: `${id}-title`, tabindex: '-1' }, title), ...children);
  }

  private caps(): HTMLElement {
    const c = this.app.store.state.caps;
    if (!c) return this.section('set-caps', '能力与权限', h('p', { class: 'muted' }, '能力信息不可用。'));
    const capState: Record<string, string> = { available: '可用', permission_required: '需要系统权限', unsupported: '不支持', unverified: '尚未在真机验证' };
    return this.section(
      'set-caps',
      '能力与权限',
      h(
        'dl',
        { class: 'facts-list' },
        h('dt', {}, '读取权限'),
        h('dd', {}, c.bypass_privileges_disabled === null ? '按系统普通权限读取' : c.bypass_privileges_disabled ? '启动时已关闭备份/还原特权；只读取普通权限可见的内容，不会自动提权' : `未能关闭绕过权限的特权：${c.bypass_privileges_note ?? ''}`),
        h('dt', {}, '写入操作'),
        h('dd', {}, c.file_actions_verified ? '只执行已存储且已授权的计划；每一项执行前重新检查对象与保护' : '此平台的文件操作尚未验证，执行与恢复不可用'),
        h('dt', {}, '产品授权'),
        h('dd', {}, '与系统权限分开：每份计划都需要人工批准，或由你启用的有限策略覆盖'),
        h('dt', {}, '授权身份'),
        h('dd', {}, h('code', { class: 'selectable' }, c.principal)),
      ),
      // The backend capability names are machine facts: one line in words, the list in 技术详情.
      h('p', { class: 'muted small' }, SETTINGS_TEXT.capsSummary(c.capabilities.length, c.capabilities.filter((x) => x.state === 'unverified').length)),
      techBlock(h('ul', { class: 'cap-list' }, ...c.capabilities.map((x) => h('li', {}, h('code', {}, x.name), h('span', { class: 'muted' }, capState[x.state] ?? unknownCodeText(x.state)))))),
    );
  }

  /** D15: one switch for every capability whose release gate has not passed; shared with the CLI. */
  private experimentalSection(): HTMLElement {
    const c = this.app.store.state.caps;
    const on = !!c?.experimental_features;
    const toggle = h('input', { type: 'checkbox', role: 'switch', id: 'experimental-switch', disabled: this.busy || !c }) as HTMLInputElement;
    toggle.checked = on;
    toggle.addEventListener('change', () => void this.setExperimental(toggle.checked));
    const rows: GateRow[] = c?.gates ?? [];
    return this.section(
      'set-experimental',
      '实验功能',
      h('p', { class: 'muted' }, SETTINGS_TEXT.experimentalLede),
      learnMore(
        h('p', {}, '下面这些工具还没有通过发布门禁 G1、G3、G4 或 AID：它们还没有在 Windows 和 macOS 真机上完成验收，所以默认关闭，关闭时界面只说明原因，命令行也会拒绝。'),
        h('p', {}, '开启后它们只供你在这台电脑上评估，并不因此算作已发布；每项更改仍然要由你审阅计划。'),
        h('p', {}, `关闭时会停止常驻工具（${words().tray}、亮屏、清洁屏幕、隐私指示和电量提醒），并删除它们创建的登录启动项。`),
      ),
      h(
        'div',
        { class: 'switches' },
        h(
          'label',
          { class: 'switch-row', for: 'experimental-switch' },
          toggle,
          h('span', {}, h('b', {}, '开启实验功能'), h('span', { class: 'muted small' }, on ? '已开启：下列未通过门禁的工具可以在这台电脑上使用。' : '默认关闭：下列工具保留在原位置，打开时说明原因。')),
        ),
      ),
      !c ? h('p', { class: 'muted small' }, '能力信息不可用，暂时无法更改这个设置。') : null,
      rows.length
        ? h(
            'ul',
            { class: 'cap-list', 'aria-label': '未通过门禁的工具' },
            ...rows.map((r) =>
              h(
                'li',
                {},
                h('span', {}, CAPABILITY_LABEL[r.capability]?.() ?? r.capability, ' ', h('code', { class: 'muted small' }, r.capability)),
                h('span', { class: 'muted' }, `门禁 ${r.gate}${r.gate_passed ? ' 已通过' : ' 未通过'}，${r.enabled ? '已开启' : '已关闭'}`),
              ),
            ),
          )
        : null,
      copyField('命令行与桌面端共用这个设置', on ? 'cleanmanager experimental --state off' : 'cleanmanager experimental --state on'),
    );
  }

  private async setExperimental(on: boolean): Promise<void> {
    try {
      await this.change(async () => {
        const caps = await this.app.backend.setExperimentalFeatures(on);
        this.app.store.set({ caps });
      }, on ? '已开启实验功能' : '已关闭实验功能；未通过门禁的工具和常驻工具已停止');
      if (on && this.diagOn()) void this.diag.reload();
    } catch {
      /* reported by change() */
    }
  }

  private protectionSection(): HTMLElement {
    const p = this.protection;
    const add = h('button', { type: 'button', id: 'protect-folder', disabled: this.busy || !!p?.problem }, '保护一个文件夹…');
    add.addEventListener('click', () => void this.addFolder());
    if (!p) return this.section('set-protection', '保护清单', this.protectionError ? errorLine(this.protectionError) : h('p', { class: 'muted', role: 'status' }, '正在读取…'));
    const user = p.items.filter((i) => i.layer === 'user');
    const builtIn = p.items.filter((i) => i.layer !== 'user');
    return this.section(
      'set-protection',
      '保护清单',
      h('p', { class: 'muted' }, '受保护的对象不会成为候选，执行前也会再次检查。'),
      learnMore(h('p', {}, '保护只会收紧可清理的范围；保护单个位置或整条规则也可以在清理页的候选详情里完成。')),
      p.problem
        ? h('div', { class: 'notice warn', role: 'alert' }, h('span', {}, `保护清单无法读取（${p.problem}）。所有写操作已停止；文件 ${p.file_display ?? ''} 保持原样，修复后恢复。`))
        : null,
      h('div', { class: 'card-actions' }, add),
      h('h3', {}, `你的保护项（${groupDigits(user.length)}）`),
      user.length ? h('ul', { class: 'prot-list' }, ...user.map((i) => this.protectionRow(i))) : h('p', { class: 'muted small' }, '还没有自己的保护项。'),
      h(
        'details',
        { class: 'builtin' },
        h('summary', {}, `内置保护（${groupDigits(builtIn.length)}，不可移除）`),
        h('ul', { class: 'prot-list' }, ...builtIn.map((i) => this.protectionRow(i))),
      ),
    );
  }

  private protectionRow(i: ProtectionItem): HTMLElement {
    const removing = this.pending?.kind === 'remove' && this.pending.id === i.protection_id;
    const remove = i.removable && !removing ? h('button', { type: 'button', class: 'danger-quiet', id: `rm-${i.protection_id}`, disabled: this.busy }, '移除') : null;
    remove?.addEventListener('click', () => {
      this.pending = { kind: 'remove', id: i.protection_id };
      this.saveError = null;
      this.update();
    });
    return h(
      'li',
      { class: 'prot-row', 'data-layer': i.layer },
      h('div', { class: 'prot-main' }, h('span', { class: 'path', title: i.layer === 'default_category' ? i.display : null }, i.layer === 'default_category' ? protectionText(i.protection_id.replace(/^dc_/, '')) : i.display), h('span', { class: 'muted small' }, `${LAYER_LABEL[i.layer]}，${MATCH_LABEL[i.match] ?? i.match}${i.note ? `，备注：${i.note}` : ''}${i.added_at ? `，添加于 ${formatTime(i.added_at)}` : ''}`)),
      remove,
      removing
        ? confirmInline({
            id: `rm-confirm-${i.protection_id}`,
            prompt: '移除后，规则和计划可以再次处理这些对象（硬拒绝与默认保护仍然有效）。输入编号以确认：',
            expected: i.protection_id,
            action: '移除保护',
            danger: true,
            onConfirm: async (typed) => {
              await this.change(async () => {
                this.protection = await this.app.backend.removeProtection(i.protection_id, typed);
              }, `已移除保护项 ${i.protection_id}`);
            },
            onCancel: () => {
              this.pending = null;
              this.update();
              requestAnimationFrame(() => document.getElementById(`rm-${i.protection_id}`)?.focus());
            },
          })
        : null,
    );
  }

  private policySection(): HTMLElement {
    const list = this.policies;
    return this.section(
      'set-policies',
      '有限策略',
      h('p', { class: 'muted' }, '默认没有任何策略，每份计划都需要你批准。'),
      learnMore(h('p', {}, '策略只覆盖同一扫描范围、指定规则版本和处理方式下、全部为低风险项的计划，并受单次与 24 小时额度限制。'), h('p', {}, '在计划页可以为同类计划创建暂停状态的策略。')),
      this.policyError ? errorLine(this.policyError) : null,
      !list ? h('p', { class: 'muted', role: 'status' }, '正在读取…') : list.length ? h('ul', { class: 'policy-list' }, ...list.map((p) => this.policyRow(p))) : h('p', { class: 'small' }, '没有策略。所有计划都需要人工批准。'),
    );
  }

  private policyRow(p: Policy): HTMLElement {
    const effective = p.state === 'enabled' && p.expired ? '已到期' : POLICY_STATE_LABEL[p.state];
    const tone = p.state === 'enabled' && !p.expired ? 'good' : p.state === 'revoked' ? 'muted' : 'busy';
    const actions: (HTMLElement | null)[] = [];
    const pend = this.pending && this.pending.id === p.policy_id ? this.pending.kind : null;
    if (p.state !== 'revoked' && !pend) {
      if (p.state === 'paused' && !p.expired)
        actions.push(h('button', { type: 'button', id: `pol-enable-${p.policy_id}`, disabled: this.busy, onclick: () => ((this.pending = { kind: 'enable', id: p.policy_id }), this.update()) }, '启用…'));
      if (p.state === 'enabled')
        actions.push(
          h(
            'button',
            {
              type: 'button',
              disabled: this.busy,
              onclick: () =>
                void this.change(async () => {
                  this.policies = await this.app.backend.setPolicyState(p.policy_id, 'paused', null);
                }, `已暂停策略 ${p.policy_id}`).catch(() => {}),
            },
            '暂停',
          ),
        );
      actions.push(h('button', { type: 'button', class: 'danger-quiet', id: `pol-revoke-${p.policy_id}`, disabled: this.busy, onclick: () => ((this.pending = { kind: 'revoke', id: p.policy_id }), this.update()) }, '撤销…'));
    }
    let confirm: HTMLElement | null = null;
    if (pend === 'enable')
      confirm = confirmInline({
        id: `pol-confirm-${p.policy_id}`,
        prompt: '启用后，符合条件的新计划无需逐次批准即可执行（仍受额度限制）。输入策略编号以启用：',
        expected: p.policy_id,
        action: '启用策略',
        onConfirm: async (typed) => {
          await this.change(async () => {
            this.policies = await this.app.backend.setPolicyState(p.policy_id, 'enabled', typed);
          }, `已启用策略 ${p.policy_id}`);
        },
        onCancel: () => ((this.pending = null), this.update()),
      });
    if (pend === 'revoke') {
      const yes = h('button', { type: 'button', class: 'danger', id: `pol-revoke-yes-${p.policy_id}` }, '撤销策略');
      yes.addEventListener('click', () => void this.change(async () => {
        this.policies = await this.app.backend.setPolicyState(p.policy_id, 'revoked', null);
      }, `已撤销策略 ${p.policy_id}`).catch(() => {}));
      const no = h('button', { type: 'button', class: 'quiet', onclick: () => ((this.pending = null), this.update()) }, '取消');
      confirm = h(
        'div',
        { class: 'confirm-inline', role: 'group', 'aria-label': '撤销影响' },
        h(
          'ul',
          { class: 'impact' },
          h('li', {}, '新的执行不再按这条策略授权，需要人工批准。'),
          h('li', {}, '已经完成的动作不会被撤销；已开始的执行按开始时的授权继续到结束，可在执行页停止后续操作。'),
          h('li', {}, '撤销不可逆；以后需要时请创建新策略。'),
        ),
        h('div', { class: 'confirm-row' }, yes, no),
      );
      requestAnimationFrame(() => yes.focus());
    }
    return h(
      'li',
      { class: 'policy-row' },
      h('div', { class: 'policy-head' }, h('code', { class: 'selectable' }, p.policy_id), pill(effective, tone), h('span', { class: 'muted small' }, `版本 ${p.version}`)),
      h(
        'dl',
        { class: 'facts-list small' },
        h('dt', {}, '处理方式'),
        h('dd', {}, MODE_LABEL[p.mode]),
        h('dt', {}, '规则'),
        h('dd', {}, ...p.rules.map((r) => h('code', { class: 'rule-chip' }, r))),
        h('dt', {}, '扫描范围'),
        h('dd', {}, h('code', {}, p.scope_id)),
        h('dt', {}, '额度'),
        h('dd', {}, `单次最多 ${formatBytes(p.single_max_bytes)}、${groupDigits(p.single_max_items)} 项；24 小时内最多 ${formatBytes(p.window_max_bytes)}、${groupDigits(p.window_max_items)} 项`),
        h('dt', {}, '24 小时已用'),
        h('dd', {}, `${formatBytes(p.window_used_bytes)}，${groupDigits(p.window_used_items)} 项`),
        h('dt', {}, '有效期'),
        h('dd', {}, `${formatTime(p.created_at)} 至 ${formatTime(p.expires_at)}`),
      ),
      actions.length ? h('div', { class: 'card-actions' }, ...actions) : null,
      confirm,
    );
  }

  private dataSection(): HTMLElement {
    const c = this.app.store.state.caps;
    return this.section(
      'set-data',
      '数据与 CLI',
      h(
        'dl',
        { class: 'facts-list' },
        h('dt', {}, '索引与审计记录'),
        h('dd', { class: 'path' }, c?.store_dir_display ?? '不可用', h('small', {}, '只保存在本机。扫描会把这个目录记为“自身数据”跳过，不计入大小。')),
        h('dt', {}, '规则包'),
        h('dd', { class: 'path' }, c?.rules_dir_display ?? '未找到', h('small', {}, c?.rules_problem ? `无法使用：${c.rules_problem}` : c?.rule_count !== null && c?.rule_count !== undefined ? `已加载 ${c.rule_count} 条规则` : '')),
        h('dt', {}, '引擎'),
        h('dd', {}, c ? `${c.engine_version}（协议 ${c.schema_version}）` : '—'),
      ),
      c?.store_problem ? h('p', { class: 'form-error' }, refusalText(c.store_problem)) : null,
      h('h3', {}, 'CLI 连接'),
      h('p', { class: 'muted small' }, 'CLI 与桌面端读写同一个存储：在一侧生成的计划可以在另一侧审阅和执行，批准与策略对两侧同样有效。'),
      copyField('列出计划与执行', 'cleanmanager operations list'),
      copyField('查看隔离区', 'cleanmanager recovery list'),
      copyField('列出保护项', 'cleanmanager protect list'),
      h('h3', {}, '构建产物扫描根目录'),
      h('p', { class: 'muted small' }, '与命令行 purge roots 使用同一份设置；在“清理 › 构建产物”中添加或移除。'),
      h('div', { class: 'card-actions' }, h('button', { type: 'button', id: 'set-purge-roots', onclick: () => this.app.navigate('purge') }, '管理扫描根目录')),
      copyField('在终端中查看', 'cleanmanager purge roots'),
    );
  }
}
