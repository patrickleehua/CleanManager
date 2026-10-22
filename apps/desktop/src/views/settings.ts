// P10 — capabilities, protection list and finite policies (PRT-*, PLN-05/06, GUI-04).
// OS read permission, write permission and product authorization are stated separately.
// Protection: add from host-held sources only; removal needs the entry id typed; built-in
// layers cannot be removed. Policies: none by default, created paused, enabling needs the id
// typed, revoking states its impact. A failed save leaves the stored settings unchanged.

import type { App } from '../main';
import { toIpcError } from '../lib/api';
import { formatBytes } from '../lib/bytes';
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
import { storedLanguage, systemLocale, t, type LanguagePref, type Locale } from '../i18n';

/** Words of the settings page that are not data (one table for the language switch). */
const SETTINGS_TEXT = {
  get experimentalLede() { return t('settings.experimentalLede'); },
  capsSummary: (n: number, unverified: number) => (unverified ? t('settings.capsSummary.nUnverifiedNotYet', { n, unverified }) : t('settings.capsSummary.n', { n })),
} as const;

type Pending = { kind: 'remove'; id: string } | { kind: 'enable'; id: string } | { kind: 'revoke'; id: string } | null;

/** What each gated capability is called where people meet it (D15). */
const CAPABILITY_LABEL: Record<string, () => string> = {
  'clean.catalog': () => t('settings.capabilityLabel.clean.catalog'),
  'purge.artifacts': () => t('settings.capabilityLabel.purge.artifacts'),
  'installer.cleanup': () => t('settings.capabilityLabel.installer.cleanup'),
  'trash.manual': () => t('settings.capabilityLabel.trash.manual', { bin: words().bin }),
  'insights.overview': () => t('settings.capabilityLabel.insights.overview'),
  'apps.uninstall': () => t('settings.capabilityLabel.apps.uninstall'),
  'apps.updates': () => t('settings.capabilityLabel.apps.updates'),
  'startup.toggle': () => t('settings.capabilityLabel.startup.toggle'),
  'maintenance.run': () => t('settings.capabilityLabel.maintenance.run'),
  'status.metrics': () => t('settings.capabilityLabel.status.metrics'),
  'process.end': () => t('settings.capabilityLabel.process.end'),
  'resident.utilities': () => t('settings.capabilityLabel.resident.utilities'),
  'lifecycle.self_update': () => t('settings.capabilityLabel.lifecycle.self_update'),
  'diag.ai': () => t('settings.capabilityLabel.diag.ai'),
};

const SECTIONS = [
  { id: 'set-language', get label() { return t('settings.sections.set-language.label'); } },
  { id: 'set-caps', get label() { return t('settings.sections.set-caps.label'); } },
  { id: 'set-experimental', get label() { return t('settings.sections.set-experimental.label'); } },
  { id: 'set-protection', get label() { return t('settings.sections.set-protection.label'); } },
  { id: 'set-policies', get label() { return t('settings.sections.set-policies.label'); } },
  { id: 'set-data', get label() { return t('settings.sections.set-data.label'); } },
  { id: 'set-diag', get label() { return t('settings.sections.set-diag.label'); } },
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

  /** LIF-05: a notice written before the switch would stay in the old language; the AI section draws itself again. */
  relocalize(): void {
    this.saved = null;
    this.saveError = null;
    this.diag.render();
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
      this.saveError = t('settings.notSavedErrSettings', { err: errorText(err) });
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
      }, t('settings.protectedPathDisplay', { path_display: offer.path_display }));
    } catch {
      /* reported by change() */
    }
  }

  private render(): void {
    const nav = h(
      'nav',
      { class: 'local-nav', 'aria-label': t('settings.settingsSections') },
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
      pageHead({ id: 'settings-title', title: t('common.settings') }),
      this.saveError ? h('div', { class: 'notice warn', role: 'alert' }, h('span', {}, this.saveError)) : null,
      this.saved ? h('div', { class: 'notice', role: 'status' }, h('span', {}, this.saved)) : null,
      h(
        'div',
        { class: 'settings-layout' },
        nav,
        h(
          'div',
          { class: 'settings-main' },
          this.languageSection(),
          this.caps(),
          this.experimentalSection(),
          this.protectionSection(),
          this.policySection(),
          this.dataSection(),
          this.diagOn()
            ? this.diag.el
            : this.section('set-diag', t('common.aiDiagnosisOptional'), h('p', { class: 'muted' }, t('settings.aiDiagnosisHasntPassed'))),
        ),
      ),
    );
  }

  /** LIF-05: the interface language — follow the system (default) or one of the two; applies at once, no restart. */
  private languageSection(): HTMLElement {
    const pref = storedLanguage();
    const names: Record<Locale, string> = { 'zh-CN': t('settings.language.zhCN'), en: t('settings.language.en') };
    const choices: [LanguagePref, string, Locale | null][] = [
      ['system', t('settings.language.system'), null],
      ['zh-CN', names['zh-CN'], 'zh-CN'],
      ['en', names.en, 'en'],
    ];
    const group = h(
      'fieldset',
      { class: 'seg lang-seg', 'aria-labelledby': 'set-language-title' },
      ...choices.map(([value, text, lang]) => {
        const input = h('input', { type: 'radio', name: 'cm-language', id: `lang-${value}`, value }) as HTMLInputElement;
        input.checked = value === pref;
        input.addEventListener('change', () => this.app.setLanguage(value));
        // The two language names are written in their own language (and marked so for screen readers).
        return h('label', { for: `lang-${value}` }, input, h('span', { lang }, text));
      }),
    );
    return this.section(
      'set-language',
      t('settings.language.title'),
      h('p', { class: 'muted' }, t('settings.language.lede')),
      group,
      h('p', { class: 'small muted lang-note' }, t('settings.language.systemNow', { name: names[systemLocale()] }), ' ', t('settings.language.hostNote')),
    );
  }

  private section(id: string, title: string, ...children: (Node | null | false)[]): HTMLElement {
    return h('section', { class: 'panel settings-section', id, 'aria-labelledby': `${id}-title` }, h('h2', { id: `${id}-title`, tabindex: '-1' }, title), ...children);
  }

  private caps(): HTMLElement {
    const c = this.app.store.state.caps;
    if (!c) return this.section('set-caps', t('settings.capabilitiesPermissions'), h('p', { class: 'muted' }, t('settings.capabilityInformationUnavailable')));
    const capState: Record<string, string> = { available: t('settings.caps.available'), permission_required: t('settings.caps.permission_required'), unsupported: t('settings.caps.unsupported'), unverified: t('settings.caps.unverified') };
    return this.section(
      'set-caps',
      t('settings.capabilitiesPermissions'),
      h(
        'dl',
        { class: 'facts-list' },
        h('dt', {}, t('settings.readAccess')),
        h('dd', {}, c.bypass_privileges_disabled === null ? t('settings.readsNormalSystemPermissions') : c.bypass_privileges_disabled ? t('settings.backupRestorePrivilegesDropped') : t('settings.couldntDropPrivilegesBypass', { p: c.bypass_privileges_note ?? '' })),
        h('dt', {}, t('common.writeOperations')),
        h('dd', {}, c.file_actions_verified ? t('settings.runsOnlyStoredAuthorized') : t('settings.fileOperationsPlatformArent')),
        h('dt', {}, t('settings.productAuthorization')),
        h('dd', {}, t('settings.separateSystemPermissionsEvery')),
        h('dt', {}, t('settings.authorized')),
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
      t('settings.experimentalFeatures'),
      h('p', { class: 'muted' }, SETTINGS_TEXT.experimentalLede),
      learnMore(
        h('p', {}, t('settings.toolsHaventPassedRelease')),
        h('p', {}, t('settings.turningThemOnlyEvaluating')),
        h('p', {}, t('settings.turningThemOffStops', { tray: words().tray })),
      ),
      h(
        'div',
        { class: 'switches' },
        h(
          'label',
          { class: 'switch-row', for: 'experimental-switch' },
          toggle,
          h('span', {}, h('b', {}, t('settings.turnExperimentalFeatures')), h('span', { class: 'muted small' }, on ? t('settings.toolsBelowHaventPassed') : t('settings.offDefaultToolsBelow'))),
        ),
      ),
      !c ? h('p', { class: 'muted small' }, t('settings.capabilityInformationUnavailableSo')) : null,
      rows.length
        ? h(
            'ul',
            { class: 'cap-list', 'aria-label': t('settings.toolsHaventPassedGates') },
            ...rows.map((r) =>
              h(
                'li',
                {},
                h('span', {}, CAPABILITY_LABEL[r.capability]?.() ?? r.capability, ' ', h('code', { class: 'muted small' }, r.capability)),
                h('span', { class: 'muted' }, t('settings.gateGatePP2', { gate: r.gate, p: r.gate_passed ? t('settings.passed') : t('settings.notPassed'), p2: r.enabled ? t('settings.onLower') : t('settings.off') })),
              ),
            ),
          )
        : null,
      copyField(t('settings.commandLineDesktopApp'), on ? 'cleanmanager experimental --state off' : 'cleanmanager experimental --state on'),
    );
  }

  private async setExperimental(on: boolean): Promise<void> {
    try {
      await this.change(async () => {
        const caps = await this.app.backend.setExperimentalFeatures(on);
        this.app.store.set({ caps });
      }, on ? t('settings.experimentalFeaturesTurned') : t('settings.experimentalFeaturesTurnedOff'));
      if (on && this.diagOn()) void this.diag.reload();
    } catch {
      /* reported by change() */
    }
  }

  private protectionSection(): HTMLElement {
    const p = this.protection;
    const add = h('button', { type: 'button', id: 'protect-folder', disabled: this.busy || !!p?.problem }, t('settings.protectFolder'));
    add.addEventListener('click', () => void this.addFolder());
    if (!p) return this.section('set-protection', t('common.protectionList'), this.protectionError ? errorLine(this.protectionError) : h('p', { class: 'muted', role: 'status' }, t('common.reading')));
    const user = p.items.filter((i) => i.layer === 'user');
    const builtIn = p.items.filter((i) => i.layer !== 'user');
    return this.section(
      'set-protection',
      t('common.protectionList'),
      h('p', { class: 'muted' }, t('settings.protectedObjectsNeverBecome')),
      learnMore(h('p', {}, t('settings.protectionOnlyNarrowsWhat'))),
      p.problem
        ? h('div', { class: 'notice warn', role: 'alert' }, h('span', {}, t('settings.protectionListCantRead', { problem: p.problem, p: p.file_display ?? '' })))
        : null,
      h('div', { class: 'card-actions' }, add),
      h('h3', {}, t('settings.protectedItemsLength', { length: user.length })),
      user.length ? h('ul', { class: 'prot-list' }, ...user.map((i) => this.protectionRow(i))) : h('p', { class: 'muted small' }, t('settings.haventProtectedAnythingYet')),
      h(
        'details',
        { class: 'builtin' },
        h('summary', {}, t('settings.builtProtectionLengthCant', { length: builtIn.length })),
        h('ul', { class: 'prot-list' }, ...builtIn.map((i) => this.protectionRow(i))),
      ),
    );
  }

  private protectionRow(i: ProtectionItem): HTMLElement {
    const removing = this.pending?.kind === 'remove' && this.pending.id === i.protection_id;
    const remove = i.removable && !removing ? h('button', { type: 'button', class: 'danger-quiet', id: `rm-${i.protection_id}`, disabled: this.busy }, t('common.remove')) : null;
    remove?.addEventListener('click', () => {
      this.pending = { kind: 'remove', id: i.protection_id };
      this.saveError = null;
      this.update();
    });
    return h(
      'li',
      { class: 'prot-row', 'data-layer': i.layer },
      h('div', { class: 'prot-main' }, h('span', { class: 'path', title: i.layer === 'default_category' ? i.display : null }, i.layer === 'default_category' ? protectionText(i.protection_id.replace(/^dc_/, '')) : i.display), h('span', { class: 'muted small' }, t('settings.pP2P3P4', { p: LAYER_LABEL[i.layer], p2: MATCH_LABEL[i.match] ?? i.match, p3: i.note ? t('settings.noteNote', { note: i.note }) : '', p4: i.added_at ? t('settings.addedAdded', { added_at: formatTime(i.added_at) }) : '' }))),
      remove,
      removing
        ? confirmInline({
            id: `rm-confirm-${i.protection_id}`,
            prompt: t('settings.onceRemovedRulesPlans'),
            expected: i.protection_id,
            action: t('settings.removeProtection'),
            danger: true,
            onConfirm: async (typed) => {
              await this.change(async () => {
                this.protection = await this.app.backend.removeProtection(i.protection_id, typed);
              }, t('settings.removedProtectedItemProtection', { protection_id: i.protection_id }));
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
      t('settings.limitedPolicies'),
      h('p', { class: 'muted' }, t('settings.noPoliciesDefaultEvery')),
      learnMore(h('p', {}, t('settings.policyOnlyCoversPlans')), h('p', {}, t('settings.planPageCreatePaused'))),
      this.policyError ? errorLine(this.policyError) : null,
      !list ? h('p', { class: 'muted', role: 'status' }, t('common.reading')) : list.length ? h('ul', { class: 'policy-list' }, ...list.map((p) => this.policyRow(p))) : h('p', { class: 'small' }, t('settings.noPoliciesEveryPlan')),
    );
  }

  private policyRow(p: Policy): HTMLElement {
    const effective = p.state === 'enabled' && p.expired ? t('settings.expired') : POLICY_STATE_LABEL[p.state];
    const tone = p.state === 'enabled' && !p.expired ? 'good' : p.state === 'revoked' ? 'muted' : 'busy';
    const actions: (HTMLElement | null)[] = [];
    const pend = this.pending && this.pending.id === p.policy_id ? this.pending.kind : null;
    if (p.state !== 'revoked' && !pend) {
      if (p.state === 'paused' && !p.expired)
        actions.push(h('button', { type: 'button', id: `pol-enable-${p.policy_id}`, disabled: this.busy, onclick: () => ((this.pending = { kind: 'enable', id: p.policy_id }), this.update()) }, t('common.enable')));
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
                }, t('settings.pausedPolicyPolicyId', { policy_id: p.policy_id })).catch(() => {}),
            },
            t('settings.pause'),
          ),
        );
      actions.push(h('button', { type: 'button', class: 'danger-quiet', id: `pol-revoke-${p.policy_id}`, disabled: this.busy, onclick: () => ((this.pending = { kind: 'revoke', id: p.policy_id }), this.update()) }, t('settings.revoke')));
    }
    let confirm: HTMLElement | null = null;
    if (pend === 'enable')
      confirm = confirmInline({
        id: `pol-confirm-${p.policy_id}`,
        prompt: t('settings.onceEnabledNewMatching'),
        expected: p.policy_id,
        action: t('settings.enablePolicy'),
        onConfirm: async (typed) => {
          await this.change(async () => {
            this.policies = await this.app.backend.setPolicyState(p.policy_id, 'enabled', typed);
          }, t('settings.enabledPolicyPolicyId', { policy_id: p.policy_id }));
        },
        onCancel: () => ((this.pending = null), this.update()),
      });
    if (pend === 'revoke') {
      const yes = h('button', { type: 'button', class: 'danger', id: `pol-revoke-yes-${p.policy_id}` }, t('settings.revokePolicy'));
      yes.addEventListener('click', () => void this.change(async () => {
        this.policies = await this.app.backend.setPolicyState(p.policy_id, 'revoked', null);
      }, t('settings.revokedPolicyPolicyId', { policy_id: p.policy_id })).catch(() => {}));
      const no = h('button', { type: 'button', class: 'quiet', onclick: () => ((this.pending = null), this.update()) }, t('common.cancel'));
      confirm = h(
        'div',
        { class: 'confirm-inline', role: 'group', 'aria-label': t('settings.whatRevoking') },
        h(
          'ul',
          { class: 'impact' },
          h('li', {}, t('settings.newRunsNoLonger')),
          h('li', {}, t('settings.completedActionsArentUndone')),
          h('li', {}, t('settings.revokingCantUndoneCreate')),
        ),
        h('div', { class: 'confirm-row' }, yes, no),
      );
      requestAnimationFrame(() => yes.focus());
    }
    return h(
      'li',
      { class: 'policy-row' },
      h('div', { class: 'policy-head' }, h('code', { class: 'selectable' }, p.policy_id), pill(effective, tone), h('span', { class: 'muted small' }, t('common.versionVersion', { version: p.version }))),
      h(
        'dl',
        { class: 'facts-list small' },
        h('dt', {}, t('settings.action')),
        h('dd', {}, MODE_LABEL[p.mode]),
        h('dt', {}, t('common.rules')),
        h('dd', {}, ...p.rules.map((r) => h('code', { class: 'rule-chip' }, r))),
        h('dt', {}, t('common.scanScope')),
        h('dd', {}, h('code', {}, p.scope_id)),
        h('dt', {}, t('settings.quota')),
        h('dd', {}, t('settings.perRunUpSingle', { single_max_bytes: formatBytes(p.single_max_bytes), single_max_items: p.single_max_items, window_max_bytes: formatBytes(p.window_max_bytes), window_max_items: p.window_max_items })),
        h('dt', {}, t('settings.used24Hours')),
        h('dd', {}, t('settings.windowUsedBytesWindow', { window_used_bytes: formatBytes(p.window_used_bytes), window_used_items: p.window_used_items })),
        h('dt', {}, t('settings.valid')),
        h('dd', {}, t('settings.createdExpires', { created_at: formatTime(p.created_at), expires_at: formatTime(p.expires_at) })),
      ),
      actions.length ? h('div', { class: 'card-actions' }, ...actions) : null,
      confirm,
    );
  }

  private dataSection(): HTMLElement {
    const c = this.app.store.state.caps;
    return this.section(
      'set-data',
      t('settings.dataCli'),
      h(
        'dl',
        { class: 'facts-list' },
        h('dt', {}, t('settings.indexAuditRecords')),
        h('dd', { class: 'path' }, c?.store_dir_display ?? t('common.unavailable'), h('small', {}, t('settings.storedComputerOnlyScans'))),
        h('dt', {}, t('settings.rulePack')),
        h('dd', { class: 'path' }, c?.rules_dir_display ?? t('settings.notFound'), h('small', {}, c?.rules_problem ? t('settings.unusableRulesProblem', { rules_problem: c.rules_problem }) : c?.rule_count !== null && c?.rule_count !== undefined ? t('settings.ruleCountLoaded', { rule_count: c.rule_count }) : '')),
        h('dt', {}, t('common.engine')),
        h('dd', {}, c ? t('common.engineVersionProtocolSchema', { engine_version: c.engine_version, schema_version: c.schema_version }) : '—'),
      ),
      c?.store_problem ? h('p', { class: 'form-error' }, refusalText(c.store_problem)) : null,
      h('h3', {}, t('settings.cliConnection')),
      h('p', { class: 'muted small' }, t('settings.cliDesktopAppRead')),
      copyField(t('settings.listPlansRuns'), 'cleanmanager operations list'),
      copyField(t('settings.viewQuarantine'), 'cleanmanager recovery list'),
      copyField(t('settings.listProtectedItems'), 'cleanmanager protect list'),
      h('h3', {}, t('settings.buildArtifactScanRoots')),
      h('p', { class: 'muted small' }, t('settings.usesSameSettingsClis')),
      h('div', { class: 'card-actions' }, h('button', { type: 'button', id: 'set-purge-roots', onclick: () => this.app.navigate('purge') }, t('settings.manageScanRoots'))),
      copyField(t('settings.viewTerminal'), 'cleanmanager purge roots'),
    );
  }
}
