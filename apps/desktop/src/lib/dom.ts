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

/** Inline SVG icon from a fixed internal path set (no external assets). */
const ICONS: Record<string, string> = {
  overview: 'M3 13h8V3H3v10Zm0 8h8v-6H3v6Zm10 0h8V11h-8v10Zm0-18v6h8V3h-8Z',
  disk: 'M4 4h7v9H4V4Zm9 0h7v5h-7V4Zm0 7h7v9h-7v-9ZM4 15h7v5H4v-5Z',
  scope: 'M4 6a2 2 0 0 1 2-2h4l2 2h6a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6Z',
  theme: 'M12 3a9 9 0 1 0 9 9 7 7 0 0 1-9-9Z',
  up: 'M12 5l-7 7h4v7h6v-7h4l-7-7Z',
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
};

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
