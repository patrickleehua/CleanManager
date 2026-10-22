// Tiny DOM builder. Text is always assigned as text nodes — file names and paths are
// untrusted and never reach innerHTML.

type Child = Node | string | number | null | undefined | false;
type Attrs = Record<string, string | number | boolean | null | undefined | EventListener>;

export function h<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Attrs = {}, ...children: Child[]): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === null || v === undefined || v === false) continue;
    if (k.startsWith('on') && typeof v === 'function') {
      el.addEventListener(k.slice(2).toLowerCase(), v as EventListener);
    } else if (k === 'class') {
      el.className = String(v);
    } else if (v === true) {
      el.setAttribute(k, '');
    } else {
      el.setAttribute(k, String(v));
    }
  }
  append(el, children);
  return el;
}

export function append(el: Element, children: Child[]): void {
  for (const c of children) {
    if (c === null || c === undefined || c === false) continue;
    el.append(typeof c === 'string' || typeof c === 'number' ? document.createTextNode(String(c)) : c);
  }
}

export function clear(el: Element): void {
  while (el.firstChild) el.removeChild(el.firstChild);
}

export function replace(el: Element, ...children: Child[]): void {
  clear(el);
  append(el, children);
}

// Interface text of elements that are built once (page constructors, the shell) and not re-rendered: each binding
// is re-applied after a language switch (LIF-05). Elements made in `render()` read their words afresh instead and
// must not register here.
const liveBindings: (() => void)[] = [];

/** Set `el`'s text (and/or attributes) now and again after every language switch. */
export function live<T extends Element>(el: T, text: (() => string) | null, attrs: Record<string, () => string | null> = {}): T {
  const apply = () => {
    if (text) el.textContent = text();
    for (const [k, v] of Object.entries(attrs)) {
      const value = v();
      if (value === null) el.removeAttribute(k);
      else el.setAttribute(k, value);
    }
  };
  apply();
  liveBindings.push(apply);
  return el;
}

/** Fill `el` with `children()` now and again after every language switch (static blocks with mixed content). */
export function liveChildren<T extends Element>(el: T, children: () => Child[]): T {
  const apply = () => replace(el, ...children());
  apply();
  liveBindings.push(apply);
  return el;
}

/** Re-apply every `live` binding (called by the shell after the language changed). */
export function refreshLive(): void {
  for (const apply of liveBindings) apply();
}

/** Inline SVG icon from a fixed internal path set (no external assets). */
const ICONS: Record<string, string> = {
  overview: 'M3 13h8V3H3v10Zm0 8h8v-6H3v6Zm10 0h8V11h-8v10Zm0-18v6h8V3h-8Z',
  disk: 'M4 4h7v9H4V4Zm9 0h7v5h-7V4Zm0 7h7v9h-7v-9ZM4 15h7v5H4v-5Z',
  scope: 'M4 6a2 2 0 0 1 2-2h4l2 2h6a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6Z',
  theme: 'M12 3a9 9 0 1 0 9 9 7 7 0 0 1-9-9Z',
  up: 'M12 5l-7 7h4v7h6v-7h4l-7-7Z',
  down: 'M12 19l-7-7h4V5h6v7h4l-7 7Z',
  close: 'M6 6l12 12M18 6 6 18',
  info: 'M12 8h.01M11 12h1v5h1',
  cleanup: 'M5 7h14M9 7V5h6v2M7 7l1 13h8l1-13M10.5 11v5M13.5 11v5',
  apps: 'M4.5 4.5h6v6h-6zM13.5 4.5h6v6h-6zM4.5 13.5h6v6h-6zM13.5 13.5h6v6h-6z',
  activity: 'M4 6h3M10 6h10M4 12h3M10 12h10M4 18h3M10 18h6',
  settings: 'M4 7h9M17 7h3M15 5v4M4 17h3M11 17h9M9 15v4',
  back: 'M14 6l-6 6 6 6',
  copy: 'M9 9h10v10H9zM5 15V5h10',
  shield: 'M12 3.5l7 2.8v5.4c0 4.1-2.9 7.3-7 8.8-4.1-1.5-7-4.7-7-8.8V6.3l7-2.8Z',
  check: 'M5 12.5l4.5 4.5L19 7.5',
  chevron: 'M9 6l6 6-6 6',
  diag: 'M10.5 4a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13ZM15.5 15.5 20 20M8 10.5h5M10.5 8v5',
  // Original tool and extension set (drawn for CleanManager; see docs/visual-identity.md).
  'tool-clean': 'M19.5 4.5 13 11M13 11c-1.8-1.1-4.4-.6-5.6 1.4L5 18.8c-.3.6.2 1.2.8 1.1l6.3-1.9c2-.6 3-2.9 2-4.8M9.2 16.5l-1-1.4M11.8 15.4l-1.3-1.5',
  'tool-software': 'M12 3.8 19.5 8v8L12 20.2 4.5 16V8L12 3.8ZM4.5 8 12 12.2 19.5 8M12 12.2v8',
  'tool-optimize': 'M4.2 16.5a7.8 7.8 0 1 1 15.6 0M12 16.5l3.6-4.6M8 20h8M6.6 11.2l1 .7M12 8.7v1.2',
  'tool-analyze': 'M11 4.2a7.8 7.8 0 1 0 8.8 8.8H11V4.2ZM14 3.4v6.6h6.6A6.8 6.8 0 0 0 14 3.4Z',
  'tool-status': 'M3.5 12.5h4l2.2-5.5 4.2 10.5 2.2-5h4.4',
  'ext-plans': 'M9 4.5h6v2.8H9zM9 5.8H6.5v14.7h11V5.8H15M9 12l1.8 1.8L14.5 10M9 16.8h6',
  'ext-activity': 'M12 8v4.2l2.8 1.8M4.6 12a7.4 7.4 0 1 0 2.2-5.3M4.6 4.5v3.4H8',
  'ext-recovery': 'M8.5 13.5 4.5 9.5l4-4M4.5 9.5h9.2a5.3 5.3 0 0 1 0 10.6H10',
  'ext-agent': 'M4 5.5h16v13H4zM7.5 10l2.6 2.2-2.6 2.2M12.5 14.5h4',
  keyboard: 'M3.5 7h17v10.5h-17zM7 10.5h.01M10 10.5h.01M13 10.5h.01M16 10.5h.01M8 14.2h8',
  reveal: 'M4 7.2c0-.9.7-1.7 1.6-1.7h3.6l2 2h7.2c.9 0 1.6.7 1.6 1.6v8.2c0 .9-.7 1.7-1.6 1.7H5.6c-.9 0-1.6-.8-1.6-1.7V7.2ZM12 10.5v5M9.6 13.2 12 15.6l2.4-2.4',
  bin: 'M5 7h14M9 7V5h6v2M7 7l1 13h8l1-13M10.5 11v5M13.5 11v5',
  warn: 'M12 4.2 20.5 19H3.5L12 4.2ZM12 10v4M12 16.8h.01',
  lock: 'M7 11V8.5a5 5 0 0 1 10 0V11M6 11h12v9H6zM12 14.5v2',
  // Waiting for an answer (a system privacy prompt).
  hourglass: 'M7 4h10M7 20h10M8 4v1.5c0 2.5 4 4 4 6.5s-4 4-4 6.5V20M16 4v1.5c0 2.5-4 4-4 6.5s4 4 4 6.5V20',
  refresh: 'M19.5 12a7.5 7.5 0 1 1-2.2-5.3M19.5 4.5v3.7h-3.7',
  // A drawing pin (kept on top), an outward arrow (opens a system page), a terminal prompt.
  pin: 'M9 4.5h6M10 4.5v5.2L7.5 13h9L14 9.7V4.5M12 13v6.5',
  external: 'M13.5 5.5h5v5M18.5 5.5 11 13M16.5 13.5v4.5c0 .6-.4 1-1 1H6.5c-.6 0-1-.4-1-1V9c0-.6.4-1 1-1H11',
  terminal: 'M4 5.5h16v13H4zM7.5 10l2.6 2.2-2.6 2.2M12.5 14.5h4',
};

/** The CleanManager mark: a sweep arc that gathers one particle (original artwork). */
export function appMark(size = 26): SVGSVGElement {
  const ns = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(ns, 'svg');
  svg.setAttribute('viewBox', '0 0 32 32');
  svg.setAttribute('width', String(size));
  svg.setAttribute('height', String(size));
  svg.setAttribute('class', 'app-mark');
  svg.setAttribute('aria-hidden', 'true');
  const tile = document.createElementNS(ns, 'rect');
  tile.setAttribute('x', '1');
  tile.setAttribute('y', '1');
  tile.setAttribute('width', '30');
  tile.setAttribute('height', '30');
  tile.setAttribute('rx', '9');
  tile.setAttribute('class', 'app-mark-tile');
  const arc = document.createElementNS(ns, 'path');
  arc.setAttribute('d', 'M22.6 10.2A8.6 8.6 0 1 0 22.6 21.8');
  arc.setAttribute('class', 'app-mark-arc');
  const sweep = document.createElementNS(ns, 'path');
  sweep.setAttribute('d', 'M12.2 16h5.4');
  sweep.setAttribute('class', 'app-mark-arc');
  const dot = document.createElementNS(ns, 'circle');
  dot.setAttribute('cx', '22.4');
  dot.setAttribute('cy', '16');
  dot.setAttribute('r', '2.1');
  dot.setAttribute('class', 'app-mark-dot');
  svg.append(tile, arc, sweep, dot);
  return svg;
}

export function icon(name: keyof typeof ICONS | string, label?: string): SVGSVGElement {
  const ns = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(ns, 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('class', `icon icon-${name}`);
  if (label) {
    svg.setAttribute('role', 'img');
    svg.setAttribute('aria-label', label);
  } else {
    svg.setAttribute('aria-hidden', 'true');
  }
  const p = document.createElementNS(ns, 'path');
  p.setAttribute('d', ICONS[name] ?? '');
  svg.append(p);
  return svg;
}

/**
 * Re-render part of the page without losing keyboard focus: if focus was on an element with
 * an id inside `container`, the element with the same id is focused again afterwards.
 */
export function keepFocus(container: Element, render: () => void): void {
  const active = document.activeElement as HTMLElement | null;
  const id = active && container.contains(active) ? active.id : '';
  render();
  if (id) (document.getElementById(id) as HTMLElement | null)?.focus({ preventScroll: true });
}
