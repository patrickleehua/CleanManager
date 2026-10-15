// Settings > AI 诊断 (AID-01/06/12). Optional: nothing is configured by default. API keys are
// entered in the system credential prompt the host opens and stored in Windows Credential
// Manager; the WebView never sees or holds them. CLI executables are chosen in the host's file
// dialog. A connection test carries no file information. Changing a provider invalidates open
// previews, so a new disclosure is always confirmed against the current destination.

import type { App } from '../main';
import { toIpcError } from '../lib/api';
import { h, replace } from '../lib/dom';
import { formatTime } from '../lib/format';
import type { DiagStatus, ProbeReport, ProviderStatus } from '../lib/diagTypes';
import { CREDENTIAL_LABEL, PROVIDER_LABEL, SUPPORT_LABEL } from '../lib/diagWords';
import { errorText, pill } from './parts';

type Form = { kind: 'anthropic' | 'openai_compatible' } | { kind: 'cli' } | null;

export class DiagSettings {
  el: HTMLElement;
  private status: DiagStatus | null = null;
  private error: string | null = null;
  private note: string | null = null;
  private probes = new Map<string, ProbeReport>();
  private form: Form = null;
  private busy = false;

  constructor(private app: App) {
    this.el = h('section', { class: 'panel settings-section', id: 'set-diag', 'aria-labelledby': 'set-diag-title' });
  }

  async reload(): Promise<void> {
    try {
      this.status = await this.app.backend.diagStatus();
      this.error = null;
    } catch (e) {
      this.error = errorText(toIpcError(e));
    }
    this.render();
  }

  private async act(work: () => Promise<DiagStatus | null | void>, ok: string): Promise<void> {
    this.busy = true;
    this.error = null;
    this.note = null;
    this.render();
    try {
      const r = await work();
      if (r) this.status = r;
      this.note = ok;
      this.app.announce(ok);
    } catch (e) {
      this.error = `未保存：${errorText(toIpcError(e))}`;
    } finally {
      this.busy = false;
      this.render();
    }
  }

  render(): void {
    const st = this.status;
    replace(
      this.el,
      h('h2', { id: 'set-diag-title', tabindex: '-1' }, 'AI 诊断（可选）'),
      h(
        'p',
        { class: 'muted' },
        '用你自己的 API 或已安装的 CLI 解释单个文件或文件夹。不配置也能完整使用扫描、规则、计划、执行与恢复。它和清理页的「AI 与开发者」类目（AI 工具自己的缓存与会话）无关。',
      ),
      this.error ? h('p', { class: 'form-error', role: 'alert' }, this.error) : null,
      this.note ? h('p', { class: 'small', role: 'status' }, this.note) : null,
      !st
        ? h('p', { class: 'muted', role: 'status' }, '正在读取…')
        : st.providers.length
          ? h('ul', { class: 'prov-list' }, ...st.providers.map((p) => this.row(p, st)))
          : h('p', { class: 'small' }, '还没有配置提供方。诊断入口会显示“未配置”，不会发送任何内容。'),
      st ? h('p', { class: 'small muted' }, `密钥保存在：${st.credential_store === 'windows_credential_manager' ? 'Windows 凭据管理器' : st.credential_store}。设置文件只记录端点、模型和可执行文件位置，不含密钥。`) : null,
      this.addButtons(),
      this.formView(),
    );
  }

  private row(p: ProviderStatus, st: DiagStatus): HTMLElement {
    const probe = this.probes.get(p.provider_id);
    const isDefault = st.default_provider_id === p.provider_id;
    const btn = (label: string, id: string, onclick: () => void, cls = '') => h('button', { type: 'button', class: cls, id: `${id}-${p.provider_id}`, disabled: this.busy, onclick }, label);
    const actions: (HTMLElement | null)[] = [];
    if (p.mode === 'api' && st.key_prompt_available)
      actions.push(
        btn(p.credential === 'present' ? '更换密钥…' : '保存密钥…', 'key', () =>
          void this.act(async () => {
            const saved = await this.app.backend.diagSetKey(p.provider_id);
            if (!saved) throw { code: 'cancelled', reason: 'cancelled', message: '已取消，没有更改密钥' };
            return this.app.backend.diagStatus();
          }, `已把 ${p.label} 的密钥保存到系统凭据`),
        ),
      );
    if (p.mode === 'api' && p.credential === 'present') actions.push(btn('移除密钥', 'forget', () => void this.act(() => this.app.backend.diagForgetKey(p.provider_id), `已从系统凭据移除 ${p.label} 的密钥`), 'quiet'));
    actions.push(
      btn('测试连接', 'probe', () =>
        void this.act(async () => {
          const r = await this.app.backend.diagProbe(p.provider_id);
          this.probes.set(p.provider_id, r);
        }, '连接测试已完成（没有发送任何文件资料）'),
      ),
    );
    if (!isDefault) actions.push(btn('设为默认', 'default', () => void this.act(() => this.makeDefault(p), `默认提供方：${p.label}`), 'quiet'));
    actions.push(btn('移除', 'remove', () => void this.act(() => this.app.backend.diagRemoveProvider(p.provider_id), `已移除 ${p.label}，其密钥也已从系统凭据删除`), 'danger-quiet'));
    return h(
      'li',
      { class: 'prov-row' },
      h(
        'div',
        { class: 'prov-head' },
        h('b', {}, p.label),
        isDefault ? pill('默认', 'good') : null,
        pill(SUPPORT_LABEL[p.support.state] ?? p.support.state, p.support.state === 'supported' ? 'good' : p.support.state === 'unsupported' ? 'bad' : 'busy'),
      ),
      h(
        'dl',
        { class: 'facts-list small' },
        h('dt', {}, '方式'),
        h('dd', {}, PROVIDER_LABEL[p.kind]),
        h('dt', {}, '模型'),
        h('dd', {}, p.model),
        h('dt', {}, p.mode === 'cli' ? '可执行文件' : '端点'),
        h('dd', { class: 'path' }, p.executable ?? p.endpoint ?? ''),
        h('dt', {}, '凭据'),
        h('dd', {}, CREDENTIAL_LABEL[p.credential] ?? p.credential),
        p.support.reason ? h('dt', {}, '说明') : null,
        p.support.reason ? h('dd', {}, p.support.reason) : null,
        probe ? h('dt', {}, '连接测试') : null,
        probe ? h('dd', {}, pill(probe.ok ? '成功' : '失败', probe.ok ? 'good' : 'bad'), ' ', probe.detail, h('small', {}, `${formatTime(probe.checked_at)}，未发送文件资料`)) : null,
      ),
      h('div', { class: 'card-actions' }, ...actions),
    );
  }

  private async makeDefault(p: ProviderStatus): Promise<DiagStatus | null> {
    if (p.mode === 'cli') return this.app.backend.diagChooseCli(p.kind as 'claude_code_cli' | 'codex_cli', p.model, true);
    const base = (p.endpoint ?? '').replace(/\/v1\/messages$/, '').replace(/\/chat\/completions$/, '');
    return this.app.backend.diagSaveProvider({ provider_id: p.provider_id, label: p.label, kind: p.kind as 'anthropic' | 'openai_compatible', base_url: base, model: p.model, make_default: true });
  }

  private addButtons(): HTMLElement {
    const open = (f: Form) => () => {
      this.form = f;
      this.render();
      requestAnimationFrame(() => document.getElementById('diag-form-first')?.focus());
    };
    return h(
      'div',
      { class: 'card-actions' },
      h('button', { type: 'button', id: 'diag-add-anthropic', disabled: this.busy, onclick: open({ kind: 'anthropic' }) }, '添加 Anthropic API'),
      h('button', { type: 'button', id: 'diag-add-openai', disabled: this.busy, onclick: open({ kind: 'openai_compatible' }) }, '添加兼容端点（含本机）'),
      h('button', { type: 'button', id: 'diag-add-cli', disabled: this.busy, onclick: open({ kind: 'cli' }) }, '使用已安装的 CLI'),
    );
  }

  private formView(): HTMLElement | null {
    const f = this.form;
    if (!f) return null;
    const cancel = h('button', { type: 'button', class: 'quiet', onclick: () => ((this.form = null), this.render()) }, '取消');
    if (f.kind === 'cli') {
      const kind = h('select', { id: 'diag-form-first', 'aria-label': 'CLI 类型' }, h('option', { value: 'claude_code_cli' }, 'Claude Code（claude -p，关闭全部工具）'), h('option', { value: 'codex_cli' }, 'Codex（无法关闭 shell 工具，不支持）')) as HTMLSelectElement;
      const model = h('input', { type: 'text', id: 'diag-cli-model', placeholder: 'default', 'aria-label': '模型（可留空）' }) as HTMLInputElement;
      const go = h('button', { type: 'button', class: 'primary', disabled: this.busy }, '选择可执行文件…');
      go.addEventListener('click', () =>
        void this.act(async () => {
          const r = await this.app.backend.diagChooseCli(kind.value as 'claude_code_cli' | 'codex_cli', model.value.trim(), false);
          if (r) this.form = null;
          return r;
        }, '已保存 CLI 接入；每次诊断前会用 --help 检查它能否被约束'),
      );
      return h(
        'div',
        { class: 'diag-form', role: 'group', 'aria-label': '使用已安装的 CLI' },
        h('label', { class: 'diag-label', for: 'diag-form-first' }, 'CLI'),
        kind,
        h('label', { class: 'diag-label', for: 'diag-cli-model' }, '模型（可留空，使用 CLI 默认）'),
        model,
        h('p', { class: 'small muted' }, '只接受 .exe；以固定参数运行、不经过 shell、在空的临时目录里、关闭全部工具与文件访问。无法确认能关闭工具的 CLI 不会被启动。是否复用你的订阅额度由 CLI 自己的规则决定。'),
        h('div', { class: 'card-actions' }, go, cancel),
      );
    }
    const anthropic = f.kind === 'anthropic';
    const label = h('input', { type: 'text', id: 'diag-form-first', value: anthropic ? 'Anthropic API' : '本机模型', maxlength: 80 }) as HTMLInputElement;
    const base = h('input', { type: 'url', id: 'diag-base', value: anthropic ? 'https://api.anthropic.com' : 'http://127.0.0.1:11434/v1', spellcheck: 'false' }) as HTMLInputElement;
    const model = h('input', { type: 'text', id: 'diag-model', value: anthropic ? 'claude-opus-5-5' : '', spellcheck: 'false', maxlength: 120 }) as HTMLInputElement;
    const pid = anthropic ? 'anthropic-default' : 'local-endpoint';
    const save = h('button', { type: 'button', class: 'primary', disabled: this.busy }, '保存');
    save.addEventListener('click', () =>
      void this.act(async () => {
        const r = await this.app.backend.diagSaveProvider({ provider_id: pid, label: label.value.trim(), kind: f.kind as 'anthropic' | 'openai_compatible', base_url: base.value.trim(), model: model.value.trim(), make_default: !this.status?.default_provider_id });
        this.form = null;
        return r;
      }, anthropic ? '已保存 Anthropic API 设置；接着用「保存密钥…」在系统凭据窗口里填写密钥' : '已保存兼容端点'),
    );
    return h(
      'div',
      { class: 'diag-form', role: 'group', 'aria-label': anthropic ? '添加 Anthropic API' : '添加兼容端点' },
      h('label', { class: 'diag-label', for: 'diag-form-first' }, '名称'),
      label,
      h('label', { class: 'diag-label', for: 'diag-base' }, anthropic ? 'API 地址' : '端点（OpenAI 兼容，…/v1）'),
      base,
      h('label', { class: 'diag-label', for: 'diag-model' }, '模型'),
      model,
      h(
        'p',
        { class: 'small muted' },
        '只接受 https://；本机端点可以用 http://127.0.0.1 或 localhost。失败时不会自动改用其他提供方或模型。',
        anthropic ? ' 密钥不在这里填写：保存后点「保存密钥…」，在 Windows 安全窗口的“密码”栏粘贴。' : ' 本机端点通常不需要密钥。',
      ),
      h('div', { class: 'card-actions' }, save, cancel),
    );
  }
}
