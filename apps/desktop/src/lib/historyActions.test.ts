import { afterEach, describe, expect, it } from 'vitest';
import { HISTORY_ACTIONS } from './types';
import { ACTION_LABEL } from './words';
import { setPlatform } from './platform';
import { setLocale } from '../i18n';
import schemaRaw from '../../../../schemas/v1/protocol.schema.json?raw';

afterEach(() => setPlatform('windows'));

describe('activity history: every operation action has words (WFA-40)', () => {
  it('names exactly the actions of the protocol (operation_summary.action)', () => {
    const schema = JSON.parse(schemaRaw);
    const action = schema.$defs.operation_summary.properties.action;
    const variants: { enum?: string[] }[] = action.anyOf ?? action.oneOf ?? [action];
    const names = variants.flatMap((v) => v.enum ?? []);
    expect([...HISTORY_ACTIONS].sort()).toEqual([...names].sort());
  });

  it('each one reads as words in both languages and on both platforms (never the raw name)', () => {
    for (const locale of ['zh-CN', 'en'] as const) {
      setLocale(locale);
      for (const platform of ['windows', 'macos'] as const) {
        setPlatform(platform);
        for (const a of HISTORY_ACTIONS) {
          const label = ACTION_LABEL[a];
          expect(label, `${locale}/${platform}/${a}`).toBeTruthy();
          expect(label).not.toBe(a);
          expect(label.startsWith('words.')).toBe(false);
          if (locale === 'zh-CN') expect(/[一-鿿]/.test(label), a).toBe(true);
          else expect(/[一-鿿]/.test(label), a).toBe(false);
        }
      }
    }
  });
});
