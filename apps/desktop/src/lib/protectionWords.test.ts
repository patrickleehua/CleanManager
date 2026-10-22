import { describe, expect, it } from 'vitest';
import { PROTECTION_CLASS_TEXT, protectionText } from './codes';
import { isRawHostText, metricReason } from './suiteWords';
// The core's own list of default protection classes is the source of the codes the host sends.
import analysis from '../../../../crates/cm-core/src/analysis.rs?raw';

describe('host codes shown in the main text have Chinese words', () => {
  it('every default protection class has words (candidate details, 设置 › 内置保护)', () => {
    const body = analysis.slice(analysis.indexOf('impl ProtectionClass'));
    const classes = [...body.matchAll(/ProtectionClass::\w+ => "([a-z_]+)"/g)].map((m) => m[1]);
    expect(classes.length).toBe(8);
    for (const c of classes) {
      expect(PROTECTION_CLASS_TEXT[c], c).toBeTruthy();
      expect(isRawHostText(protectionText(c)), c).toBe(false);
    }
    expect(protectionText('brand_new_class')).toBe('未识别的问题（代码 brand_new_class）');
  });

  it('Windows metric reasons read in Chinese; unknown host text is recognised as raw', () => {
    expect(isRawHostText(metricReason('No documented Windows interface reports GPU temperature without vendor software'))).toBe(false);
    expect(isRawHostText('the sensor service is not running')).toBe(true);
    expect(isRawHostText('系统没有提供这个读数')).toBe(false);
    expect(isRawHostText(null)).toBe(false);
  });
});
