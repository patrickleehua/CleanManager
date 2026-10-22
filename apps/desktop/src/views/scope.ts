// P01 — first run and scan scope. One message: choose a folder, then start the read-only scan. No sizes
// are shown before a scan. Roots that cannot be scanned say why in Chinese before anything is submitted
// (a file, a link, a missing folder); nothing is silently substituted.

import type { App } from '../main';
import { toIpcError } from '../lib/api';
import { refusalText } from '../lib/codes';
import { h, icon, keepFocus, live, liveChildren, replace } from '../lib/dom';
import { formatTime, isTerminal, STATE_LABEL, stateTone } from '../lib/format';
import type { IpcError, RecentScan, RootOffer } from '../lib/types';
import { learnMore, pathText } from './ui';
import { t } from '../i18n';

const GROUPS: { kinds: RootOffer['kind'][]; label: string }[] = [
  { kinds: ['user_folder', 'user_home'], get label() { return t('scope.groups.0.label'); } },
  { kinds: ['volume'], get label() { return t('scope.groups.1.label'); } },
  { kinds: ['chosen'], get label() { return t('scope.groups.2.label'); } },
];

/**
 * The name of a suggested scan root in the interface language. The host names its suggestions in Chinese; the ones it
 * always makes (the user folders by their folder name, the home folder, drive letters, the system volume) are put in
 * words here, anything else (a volume name such as "Macintosh HD", a chosen folder) is shown as sent.
 */
export function rootLabel(o: { label: string; kind: string; path_display: string }): string {
  if (o.kind === 'user_folder') {
    const leaf = o.path_display.split(/[\\/]/).filter(Boolean).pop() ?? '';
    const known: Record<string, () => string> = { Downloads: () => t('scope.root.downloads'), Documents: () => t('scope.root.documents'), Desktop: () => t('scope.root.desktop') };
    return known[leaf]?.() ?? o.label;
  }
  if (o.kind === 'user_home') return t('scope.root.home');
  if (o.kind === 'volume') {
    const letter = /^([A-Za-z]): /.exec(o.label)?.[1];
    if (letter) return t('scope.root.drive', { letter });
    // The host's own word for "/" (a real volume name such as "Macintosh HD" is kept).
    if (o.path_display === '/' && /\p{Script=Han}/u.test(o.label)) return t('scope.root.systemVolume');
  }
  return o.label;
}

export class ScopePage {
  el: HTMLElement;
  private list: HTMLElement;
  private feedback: HTMLElement;
  private actions: HTMLElement;
  private aside: HTMLElement;
  private offers: RootOffer[] = [];
  private selected: string | null = null;
  private loading = false;
  private loaded = false;
  private starting = false;
  private loadError: IpcError | null = null;
  private startError: IpcError | null = null;
  /** The last pick from the folder picker, when the host refused it (a file, a link…). */
  private refused: RootOffer | null = null;

  constructor(private app: App) {
    this.list = h('div', { class: 'root-groups' });
    this.feedback = h('div', { class: 'scope-feedback' });
    this.actions = h('div', { class: 'scope-actions' });
    this.aside = h('aside', { class: 'scope-aside', 'aria-labelledby': 'scope-recent-title' });
    this.el = h(
      'section',
      { class: 'page page-scope', 'aria-labelledby': 'scope-title' },
      h(
        'div',
        { class: 'scope-main' },
        liveChildren(h('div', { class: 'scope-head' }), () => [
          h('h1', { id: 'scope-title' }, t('scope.chooseFolderScan')),
          h('p', { class: 'lede' }, t('scope.onlyNamesSizesTimes')),
          learnMore(
            h(
              'ul',
              { class: 'facts' },
              h('li', {}, h('b', {}, t('scope.standardRights')), t('scope.locationsCantReadRecorded')),
              h('li', {}, h('b', {}, t('scope.staysComputer')), t('scope.indexKeptUserData')),
              h('li', {}, h('b', {}, t('scope.stopAnyTime')), t('scope.cancellingKeepsConfirmedResults')),
            ),
          ),
        ]),
        h('fieldset', { class: 'scope-panel panel' }, live(h('legend', { class: 'sr-only' }), () => t('common.scanScope')), this.list, this.feedback, this.actions),
      ),
      this.aside,
    );
  }

  activate(): void {
    if (!this.loaded && !this.loading) void this.load();
    else this.update();
  }

  private async load(): Promise<void> {
    this.loading = true;
    this.loadError = null;
    this.update();
    try {
      this.offers = await this.app.backend.listScanRoots();
      if (!this.selected) {
        // The scan being looked at, when it is still offered; else the first scannable user folder. Never a blocked root.
        const current = this.app.store.state.status?.root_display;
        const same = this.offers.find((o) => !o.problem && o.path_display === current);
        const rec = this.offers.find((o) => !o.problem && (o.kind === 'user_folder' || o.kind === 'user_home'));
        this.selected = (same ?? rec)?.token ?? null;
      }
      this.loaded = true;
    } catch (e) {
      this.loadError = toIpcError(e);
    } finally {
      this.loading = false;
      this.update();
    }
  }

  private async choose(): Promise<void> {
    try {
      const o = await this.app.backend.chooseFolder();
      if (!o) return;
      this.startError = null;
      this.refused = o.problem ? o : null;
      // A refused pick is reported next to the start button and never listed or selected; the previous selection is
      // cleared too, so starting never scans something other than what was just chosen.
      if (o.problem) this.selected = null;
      else {
        this.offers = [...this.offers.filter((x) => x.token !== o.token), o];
        this.selected = o.token;
      }
      this.update();
      if (!o.problem) (this.list.querySelector(`input[value="${CSS.escape(o.token)}"]`) as HTMLInputElement | null)?.focus();
    } catch (e) {
      this.startError = toIpcError(e);
      this.update();
    }
  }

  private async start(): Promise<void> {
    if (!this.selected || this.starting) return;
    this.starting = true;
    this.startError = null;
    this.update();
    try {
      await this.app.startScan(this.selected);
    } catch (e) {
      this.startError = toIpcError(e);
    } finally {
      this.starting = false;
      this.update();
    }
  }

  update(): void {
    keepFocus(this.el, () => this.render());
  }

  private lastScanOf(o: RootOffer): RecentScan | undefined {
    return this.app.store.state.recent.find((r) => r.root_display === o.path_display && r.state);
  }

  private option(o: RootOffer): HTMLElement {
    const id = `root-${o.token}`;
    const input = h('input', { type: 'radio', name: 'scan-root', id, value: o.token, disabled: !!o.problem, 'aria-describedby': `${id}-desc` });
    input.checked = this.selected === o.token;
    input.addEventListener('change', () => {
      this.selected = o.token;
      this.startError = null;
      this.refused = null;
      this.update();
    });
    const last = this.lastScanOf(o);
    const kind = o.kind === 'user_home' ? t('scope.userFolder') : null;
    return h(
      'label',
      { class: `root-option${o.problem ? ' blocked' : ''}${input.checked ? ' checked' : ''}`, for: id },
      input,
      h(
        'span',
        { class: 'root-text' },
        h('span', { class: 'root-label' }, rootLabel(o), kind ? h('span', { class: 'root-kind' }, kind) : null),
        pathText(o.path_display, { max: 58, className: 'root-path' }),
        h(
          'span',
          { class: 'root-desc', id: `${id}-desc` },
          o.problem
            ? h('span', { class: 'root-problem' }, icon('warn'), refusalText(o.problem))
            : last?.state
              ? h('span', { class: 'root-last' }, t('scope.lastScannedStartedP', { started_at: formatTime(last.started_at), p: STATE_LABEL[last.state] }))
              : null,
        ),
      ),
    );
  }

  private render(): void {
    const s = this.app.store.state;
    if (this.loading && !this.loaded) {
      replace(this.list, h('p', { class: 'muted', role: 'status' }, t('scope.checkingAvailableLocations')));
    } else if (this.loadError) {
      replace(this.list, h('p', { class: 'form-error', role: 'alert' }, t('scope.couldntListLocationsLoaderror', { loadError: refusalText(this.loadError) })));
    } else {
      replace(
        this.list,
        ...GROUPS.map((g) => {
          const items = this.offers.filter((o) => g.kinds.includes(o.kind));
          if (!items.length) return null;
          const gid = `scope-group-${g.kinds[0]}`;
          return h('div', { class: 'root-group', role: 'group', 'aria-labelledby': gid }, h('h2', { class: 'group-label', id: gid }, g.label), h('div', { class: 'root-list' }, ...items.map((o) => this.option(o))));
        }),
      );
    }

    // Feedback before anything is submitted: a refused folder from the picker, or a start that was refused.
    const refused = this.refused;
    const offer = this.offers.find((o) => o.token === this.selected);
    const match = offer ? this.lastScanOf(offer) : undefined;
    const problem = this.startError ?? refused?.problem ?? null;
    replace(
      this.feedback,
      problem
        ? h(
            'div',
            { class: 'field-error', role: 'alert' },
            icon('warn'),
            h('div', {}, h('p', { class: 'fe-text' }, refusalText(problem)), refused && !this.startError ? pathText(refused.path_display, { max: 56, className: 'fe-path' }) : null),
          )
        : match?.state
          ? h(
              'div',
              { class: 'notice' },
              h('span', {}, t('scope.scopeScannedStartedResult', { started_at: formatTime(match.started_at), p: STATE_LABEL[match.state] })),
              h('button', { type: 'button', class: 'link', id: 'scope-open-existing', onclick: () => void this.app.openScan(match.scan_id, 'overview') }, t('scope.viewResults')),
            )
          : null,
    );

    const running = s.task && !isTerminal(s.task.state);
    const startBtn = h('button', { type: 'button', id: 'scope-start', class: 'primary', disabled: !this.selected || !!offer?.problem || this.starting || !!running }, this.starting ? t('scope.starting') : match ? t('common.scanAgain') : t('scope.startReadOnlyScan'));
    startBtn.addEventListener('click', () => void this.start());
    const chooseBtn = h('button', { type: 'button', id: 'scope-choose' }, this.refused ? t('scope.chooseFolderAgain') : t('scope.chooseAnotherFolder'));
    chooseBtn.addEventListener('click', () => void this.choose());
    replace(this.actions, startBtn, chooseBtn, running ? h('p', { class: 'muted' }, t('scope.scanAlreadyRunningStart')) : null);

    this.renderAside();
  }

  /** Recent scans: open a result again (starting another scan needs its folder chosen above). */
  private renderAside(): void {
    const recent = this.app.store.state.recent.filter((r) => r.state);
    this.aside.hidden = !recent.length;
    if (!recent.length) return;
    replace(
      this.aside,
      h('h2', { id: 'scope-recent-title' }, t('scope.recentScans')),
      h(
        'ul',
        { class: 'recent-list' },
        ...recent.slice(0, 6).map((r) =>
          h(
            'li',
            {},
            h(
              'button',
              { type: 'button', class: 'recent-item', id: `scope-recent-${r.scan_id}`, onclick: () => void this.app.openScan(r.scan_id, 'overview') },
              pathText(r.root_display, { max: 34, className: 'ri-path' }),
              h('span', { class: 'ri-meta' }, h('span', { class: 'state-word', 'data-tone': stateTone(r.state!) }, h('span', { class: 'sw-mark', 'aria-hidden': 'true' }), STATE_LABEL[r.state!]), formatTime(r.started_at)),
            ),
          ),
        ),
      ),
    );
  }
}
