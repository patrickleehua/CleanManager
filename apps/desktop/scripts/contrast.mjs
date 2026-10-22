// WCAG 2.2 contrast check of the design-token pairs the UI actually uses, in both themes.
// Usage: node scripts/contrast.mjs  (prints a Markdown table; exits 1 on any failure).
// Also imported by src/lib/contrast.test.ts so the check runs with the unit tests.

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

/** [foreground token, background token, minimum ratio, where it is used] */
export const PAIRS = [
  ['text', 'bg', 4.5, '正文 / 页面底色'],
  ['text', 'surface', 4.5, '正文 / 面板'],
  ['text', 'soft', 4.5, '正文 / 浅底（表头、卡片）'],
  ['text', 'rail', 4.5, '导航文字 / 工具栏'],
  ['text', 'tint', 4.5, '选中行文字'],
  ['muted', 'bg', 4.5, '次要文字 / 页面底色'],
  ['muted', 'surface', 4.5, '次要文字 / 面板'],
  ['muted', 'soft', 4.5, '次要文字 / 浅底'],
  ['muted', 'rail', 4.5, '快捷键提示 / 工具栏'],
  ['muted', 'tint', 4.5, '选中行次要文字'],
  ['accent', 'surface', 4.5, '链接与强调 / 面板'],
  ['accent', 'bg', 4.5, '链接 / 页面底色'],
  ['accent', 'tint', 4.5, '“低风险”徽标与通知'],
  ['accent-ink', 'accent', 4.5, '主按钮文字'],
  ['warn', 'warn-bg', 4.5, '警告通知'],
  ['warn', 'surface', 4.5, '警告文字 / 面板'],
  ['danger', 'danger-bg', 4.5, '危险通知'],
  ['danger', 'surface', 4.5, '危险文字 / 面板'],
  ['danger-ink', 'danger', 4.5, '危险按钮文字'],
  ['text', 'danger-bg', 4.5, '“无法撤销”徽标文字 / 斜纹底'],
  ['warn', 'bg', 4.5, '大小未知提示 / 页面底色'],
  ['tile-ink', 'cat-folder', 4.5, '矩形图文字 / 文件夹'],
  ['tile-ink', 'cat-media', 4.5, '矩形图文字 / 媒体'],
  ['tile-ink', 'cat-archive', 4.5, '矩形图文字 / 压缩包'],
  ['tile-ink', 'cat-code', 4.5, '矩形图文字 / 代码'],
  ['tile-ink', 'cat-document', 4.5, '矩形图文字 / 文档'],
  ['tile-ink', 'cat-image', 4.5, '矩形图文字 / 图片'],
  ['tile-ink', 'cat-other', 4.5, '矩形图文字 / 其他'],
  ['control-line', 'surface', 3, '输入框与复选框边框（非文本）'],
  ['control-line', 'bg', 3, '输入框边框 / 页面底色（非文本）'],
  ['accent', 'soft', 3, '焦点环 / 浅底（非文本）'],
];

function block(css, selector) {
  const i = css.indexOf(selector);
  if (i < 0) throw new Error(`missing ${selector}`);
  const body = css.slice(css.indexOf('{', i) + 1, css.indexOf('}', i));
  const tokens = {};
  for (const m of body.matchAll(/--([a-z0-9-]+):\s*(#[0-9a-fA-F]{6})\b/g)) tokens[m[1]] = m[2];
  return tokens;
}

function lum(hex) {
  const c = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255).map((v) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
}

export function ratio(a, b) {
  const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
}

/** Every pair in both themes: { theme, fg, bg, ratio, min, ok, use }. */
export function check(css) {
  const light = block(css, ':root {');
  const dark = { ...light, ...block(css, ":root[data-theme='dark'] {") };
  const out = [];
  for (const [theme, t] of [['light', light], ['dark', dark]]) {
    for (const [fg, bg, min, use] of PAIRS) {
      if (!t[fg] || !t[bg]) throw new Error(`token missing in ${theme}: ${fg} / ${bg}`);
      const r = ratio(t[fg], t[bg]);
      out.push({ theme, fg, bg, fgHex: t[fg], bgHex: t[bg], ratio: r, min, ok: r >= min, use });
    }
  }
  return out;
}

/** Check the app stylesheet itself. */
export function checkStylesheet() {
  return check(readFileSync(new URL('../src/styles.css', import.meta.url), 'utf8'));
}

if (process.argv[1] && fileURLToPath(import.meta.url).replace(/\\/g, '/').endsWith(process.argv[1].replace(/\\/g, '/').split('/').slice(-2).join('/'))) {
  const css = readFileSync(new URL('../src/styles.css', import.meta.url), 'utf8');
  const rows = check(css);
  console.log('| 主题 | 前景 | 背景 | 对比度 | 要求 | 结果 | 用途 |');
  console.log('| --- | --- | --- | --- | --- | --- | --- |');
  for (const r of rows) console.log(`| ${r.theme === 'light' ? '浅色' : '深色'} | \`--${r.fg}\` ${r.fgHex} | \`--${r.bg}\` ${r.bgHex} | ${r.ratio.toFixed(2)}:1 | ${r.min}:1 | ${r.ok ? '通过' : '**未通过**'} | ${r.use} |`);
  const bad = rows.filter((r) => !r.ok);
  if (bad.length) {
    console.error(`${bad.length} pair(s) below the minimum`);
    process.exit(1);
  }
}
