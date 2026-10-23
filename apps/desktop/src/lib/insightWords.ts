// NEW-13 (WFA-66): the space-insights page in the interface language. The host (cm-core insights.rs) describes each
// machine-wide object's official tools in English: the tool, how it is used and what that does. Each guidance line has
// its words in both resources, keyed by the object kind and position and matched by the English sentence (equal to the host text),
// so `hostWords.test.ts` fails if the host's guidance changes without its words. A pure command (`powercfg …`,
// `DISM …`) is shown and copied exactly as the host wrote it. Size notes the host writes are put in words where known;
// any other note keeps the host's text as the tooltip.

import { RESOURCES, hasText, t, type TextKey } from '../i18n';
import { isRawHostText } from './suiteWords';

function key(kind: string, part: 'tool' | 'steps' | 'impact', i: number): TextKey | null {
  const k = `insightWords.${part}.${kind}.${i}`;
  return hasText(k) ? k : null;
}

/** One official-tool line as shown: words where the table has them, the host's text otherwise (commands stay exact). */
export function guidanceText(kind: string, at: number, g: { tool: string; steps: string; impact: string }): { tool: string; steps: string; impact: string; raw: string | null } {
  // The line whose English resource is the host's own sentence (its position first): words never pair with another line.
  const same = (j: number) => {
    const k = key(kind, 'impact', j);
    return !!k && RESOURCES.en[k] === g.impact;
  };
  const i = same(at) ? at : [...Array(12).keys()].find(same) ?? -1;
  const tool = key(kind, 'tool', i);
  const steps = key(kind, 'steps', i);
  const impact = key(kind, 'impact', i);
  return {
    tool: tool ? t(tool) : g.tool,
    steps: steps ? t(steps) : g.steps,
    impact: impact ? t(impact) : isRawHostText(g.impact) ? t('insightWords.impactOther') : g.impact,
    raw: impact ? null : isRawHostText(g.impact) ? g.impact : null,
  };
}

const NOTES: [RegExp, () => string][] = [
  [/^Not measured: most WinSxS files are hard links/, () => t('insightWords.note.componentStore')],
  [/^Not measured: the folder is large and partly readable only by the system/, () => t('insightWords.note.systemFolder')],
  [/^WSL 1 distribution stored as ordinary files/, () => t('insightWords.note.wsl1')],
];

/** A size note in words, or a plain line with the host's note as the tooltip. */
export function sizeNoteText(note: string | null): { text: string; raw: string | null } | null {
  if (!note) return null;
  if (!isRawHostText(note)) return { text: note, raw: null };
  const inner = note.replace(/^Size unavailable: /, '');
  const hit = NOTES.find(([re]) => re.test(inner));
  if (hit) return { text: inner === note ? hit[1]() : t('insightWords.note.unavailable', { why: hit[1]() }), raw: null };
  if (/^Estimate: /.test(note)) return { text: t('insightWords.note.estimate'), raw: note };
  return { text: t('insightWords.note.notMeasured'), raw: note };
}
