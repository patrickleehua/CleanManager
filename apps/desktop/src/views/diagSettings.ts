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
import { words } from '../lib/platform';
import { t } from '../i18n';

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
      this.error = t('diagSettings.notSavedErrortext', { errorText: errorText(toIpcError(e)) });
    } finally {
      this.busy = false;
      this.render();
    }
  }

  render(): void {
    const st = this.status;
    replace(
      this.el,
      h('h2', { id: 'set-diag-title', tabindex: '-1' }, t('common.aiDiagnosisOptional')),
      h(
        'p',
        { class: 'muted' },
        t('diagSettings.explainSingleFileFolder'),
      ),
      this.error ? h('p', { class: 'form-error', role: 'alert' }, this.error) : null,
      this.note ? h('p', { class: 'small', role: 'status' }, this.note) : null,
      !st
        ? h('p', { class: 'muted', role: 'status' }, t('common.reading'))
        : st.providers.length
          ? h('ul', { class: 'prov-list' }, ...st.providers.map((p) => this.row(p, st)))
          : h('p', { class: 'small' }, t('diagSettings.noProviderSetUp')),
      st ? h('p', { class: 'small muted' }, t('diagSettings.keysStoredPSettings', { p: st.credential_store === 'windows_credential_manager' ? t('diagSettings.windowsCredentialManager') : st.credential_store === 'macos_keychain' ? t('diagSettings.keychainLoginKeychain') : st.credential_store })) : null,
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
        btn(p.credential === 'present' ? t('diagSettings.changeKey') : t('diagSettings.saveKey'), 'key', () =>
          void this.act(async () => {
            const saved = await this.app.backend.diagSetKey(p.provider_id);
            if (!saved) throw { code: 'cancelled', reason: 'cancelled', message: t('diagSettings.cancelledKeyNotChanged') };
            return this.app.backend.diagStatus();
          }, t('diagSettings.savedKeyLabelSystem', { label: p.label })),
        ),
      );
    if (p.mode === 'api' && p.credential === 'present') actions.push(btn(t('diagSettings.removeKey'), 'forget', () => void this.act(() => this.app.backend.diagForgetKey(p.provider_id), t('diagSettings.removedKeyLabelSystem', { label: p.label })), 'quiet'));
    actions.push(
      btn(t('diagSettings.testConnection'), 'probe', () =>
        void this.act(async () => {
          const r = await this.app.backend.diagProbe(p.provider_id);
          this.probes.set(p.provider_id, r);
        }, t('diagSettings.connectionTestFinishedNo')),
      ),
    );
    if (!isDefault) actions.push(btn(t('diagSettings.makeDefault'), 'default', () => void this.act(() => this.makeDefault(p), t('diagSettings.defaultProviderLabel', { label: p.label })), 'quiet'));
    actions.push(btn(t('common.remove'), 'remove', () => void this.act(() => this.app.backend.diagRemoveProvider(p.provider_id), t('diagSettings.removedLabelDeletedKey', { label: p.label })), 'danger-quiet'));
    return h(
      'li',
      { class: 'prov-row' },
      h(
        'div',
        { class: 'prov-head' },
        h('b', {}, p.label),
        isDefault ? pill(t('common.default'), 'good') : null,
        pill(SUPPORT_LABEL[p.support.state] ?? p.support.state, p.support.state === 'supported' ? 'good' : p.support.state === 'unsupported' ? 'bad' : 'busy'),
      ),
      h(
        'dl',
        { class: 'facts-list small' },
        h('dt', {}, t('diagSettings.type')),
        h('dd', {}, PROVIDER_LABEL[p.kind]),
        h('dt', {}, t('common.model')),
        h('dd', {}, p.model),
        h('dt', {}, p.mode === 'cli' ? t('diagSettings.executable') : t('diagSettings.endpoint')),
        h('dd', { class: 'path' }, p.executable ?? p.endpoint ?? ''),
        h('dt', {}, t('diagSettings.credential')),
        h('dd', {}, CREDENTIAL_LABEL[p.credential] ?? p.credential),
        p.support.reason ? h('dt', {}, t('common.notes')) : null,
        p.support.reason ? h('dd', {}, p.support.reason) : null,
        probe ? h('dt', {}, t('diagSettings.connectionTest')) : null,
        probe ? h('dd', {}, pill(probe.ok ? t('diagSettings.succeeded') : t('diagSettings.failed'), probe.ok ? 'good' : 'bad'), ' ', probe.detail, h('small', {}, t('diagSettings.checkedNoFileDetails', { checked_at: formatTime(probe.checked_at) }))) : null,
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
      h('button', { type: 'button', id: 'diag-add-anthropic', disabled: this.busy, onclick: open({ kind: 'anthropic' }) }, t('diagSettings.addAnthropicApi')),
      h('button', { type: 'button', id: 'diag-add-openai', disabled: this.busy, onclick: open({ kind: 'openai_compatible' }) }, t('diagSettings.addCompatibleEndpointLocal')),
      h('button', { type: 'button', id: 'diag-add-cli', disabled: this.busy, onclick: open({ kind: 'cli' }) }, t('diagSettings.useInstalledCli')),
    );
  }

  private formView(): HTMLElement | null {
    const f = this.form;
    if (!f) return null;
    const cancel = h('button', { type: 'button', class: 'quiet', onclick: () => ((this.form = null), this.render()) }, t('common.cancel'));
    if (f.kind === 'cli') {
      const kind = h('select', { id: 'diag-form-first', 'aria-label': t('diagSettings.cliType') }, h('option', { value: 'claude_code_cli' }, t('diagSettings.claudeCodeClaudeP')), h('option', { value: 'codex_cli' }, t('diagSettings.codexShellToolCant'))) as HTMLSelectElement;
      const model = h('input', { type: 'text', id: 'diag-cli-model', placeholder: 'default', 'aria-label': t('diagSettings.modelOptional') }) as HTMLInputElement;
      const go = h('button', { type: 'button', class: 'primary', disabled: this.busy }, t('diagSettings.chooseExecutable'));
      go.addEventListener('click', () =>
        void this.act(async () => {
          const r = await this.app.backend.diagChooseCli(kind.value as 'claude_code_cli' | 'codex_cli', model.value.trim(), false);
          if (r) this.form = null;
          return r;
        }, t('diagSettings.cliSavedBeforeEach')),
      );
      return h(
        'div',
        { class: 'diag-form', role: 'group', 'aria-label': t('diagSettings.useInstalledCli') },
        h('label', { class: 'diag-label', for: 'diag-form-first' }, 'CLI'),
        kind,
        h('label', { class: 'diag-label', for: 'diag-cli-model' }, t('diagSettings.modelOptionalBlankUses')),
        model,
        h('p', { class: 'small muted' }, words().cliExecutable),
        h('div', { class: 'card-actions' }, go, cancel),
      );
    }
    const anthropic = f.kind === 'anthropic';
    const label = h('input', { type: 'text', id: 'diag-form-first', value: anthropic ? 'Anthropic API' : t('diagSettings.localModel'), maxlength: 80 }) as HTMLInputElement;
    const base = h('input', { type: 'url', id: 'diag-base', value: anthropic ? 'https://api.anthropic.com' : 'http://127.0.0.1:11434/v1', spellcheck: 'false' }) as HTMLInputElement;
    const model = h('input', { type: 'text', id: 'diag-model', value: anthropic ? 'claude-opus-5-5' : '', spellcheck: 'false', maxlength: 120 }) as HTMLInputElement;
    const pid = anthropic ? 'anthropic-default' : 'local-endpoint';
    const save = h('button', { type: 'button', class: 'primary', disabled: this.busy }, t('diagSettings.save'));
    save.addEventListener('click', () =>
      void this.act(async () => {
        const r = await this.app.backend.diagSaveProvider({ provider_id: pid, label: label.value.trim(), kind: f.kind as 'anthropic' | 'openai_compatible', base_url: base.value.trim(), model: model.value.trim(), make_default: !this.status?.default_provider_id });
        this.form = null;
        return r;
      }, anthropic ? t('diagSettings.anthropicApiSettingsSaved') : t('diagSettings.compatibleEndpointSaved')),
    );
    return h(
      'div',
      { class: 'diag-form', role: 'group', 'aria-label': anthropic ? t('diagSettings.addAnthropicApi') : t('diagSettings.addCompatibleEndpoint') },
      h('label', { class: 'diag-label', for: 'diag-form-first' }, t('common.name')),
      label,
      h('label', { class: 'diag-label', for: 'diag-base' }, anthropic ? t('diagSettings.apiUrl') : t('diagSettings.endpointOpenaiCompatibleV1')),
      base,
      h('label', { class: 'diag-label', for: 'diag-model' }, t('common.model')),
      model,
      h(
        'p',
        { class: 'small muted' },
        t('diagSettings.onlyHttpsAcceptedLocal'),
        anthropic ? words().keyPrompt : t('diagSettings.localEndpointsUsuallyDont'),
      ),
      h('div', { class: 'card-actions' }, save, cancel),
    );
  }
}
