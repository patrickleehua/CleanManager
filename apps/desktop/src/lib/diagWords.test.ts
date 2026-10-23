import { describe, expect, it } from 'vitest';

import { ACTION_BUTTON, ACTION_LABEL, CLAIM_LABEL, REC_EXPLAIN, REC_LABEL, STATE_LABEL, citedLayers, payloadSize, payloadView, prettyBody } from './diagWords';
import { mockDiag } from './mockDiag';

describe('diagnosis words', () => {
  it('never states that deleting is safe', () => {
    const all = [...Object.values(REC_LABEL), ...Object.values(REC_EXPLAIN), ...Object.values(CLAIM_LABEL), ...Object.values(ACTION_LABEL), ...Object.values(ACTION_BUTTON)];
    for (const w of all) {
      expect(w).not.toMatch(/安全删除|可以放心|safe/i);
      expect(w).not.toMatch(/\d+\s*%/);
    }
  });

  it('has a word for every lifecycle state', () => {
    expect(Object.keys(STATE_LABEL).sort()).toEqual(['cancelled', 'completed', 'expired', 'failed', 'running']);
  });

  it('shows the payload size exactly and expands the embedded user message for reading only', () => {
    expect(payloadSize(2701)).toBe('2.6 KB（2,701 字节）');
    const body = JSON.stringify({ model: 'm', messages: [{ role: 'user', content: JSON.stringify({ evidence: { a: 1 } }) }] });
    const pretty = prettyBody(body);
    expect(JSON.parse(pretty).messages[0].content.evidence.a).toBe(1);
    expect(prettyBody('not json')).toBe('not json');
  });

  it('NEW-17: the preview shows the body exactly as sent by default; indenting is a labelled view', () => {
    const body = JSON.stringify({ model: 'm', messages: [{ role: 'user', content: JSON.stringify({ name: '视频.mov', n: 1 }) }] });
    const exact = payloadView(body, false);
    expect(exact).toEqual({ text: body, exact: true });
    // Byte for byte: the UTF-8 length of what is shown is the length the host reports and sends.
    expect(new TextEncoder().encode(exact.text).length).toBe(new TextEncoder().encode(body).length);
    const view = payloadView(body, true);
    expect(view.exact).toBe(false);
    expect(view.text).not.toBe(body);
  });

  it('maps citations to layers and drops unknown ids', () => {
    expect(citedLayers(['ev_1', 'ev_9'], [{ evidence_id: 'ev_1', layer: 'rule' }])).toEqual(['rule']);
  });
});

describe('preview fixture', () => {
  it('prepares without sending, discloses no key and marks user additions', async () => {
    const b = mockDiag(() => ({ name: 'clip.mov', path: 'D:\\Work\\Videos\\clip.mov', kind: 'file', logical: '1000', unknownDesc: 0 }));
    const p = await b.diagPrepare('s', '5', 'disk', null, { full_path: false, child_summary: false }, '还能删吗？');
    expect(p.state).toBe('preparing');
    expect(p.contains_user_additions).toBe(true);
    expect(p.payload.headers.join('\n')).toContain('<stored in the system credential store');
    expect(p.evidence.excluded).toContain('file_contents');
    expect(await b.diagDiscard(p.preview_id)).toBe(true);
    await expect(b.diagConfirm(p.preview_id, p.payload_digest)).rejects.toMatchObject({ reason: 'diag_preview_changed' });
  });

  it('reports an unconfigured extension instead of preparing', async () => {
    const b = mockDiag(() => null);
    b.seed('unconfigured');
    expect((await b.diagStatus()).configured).toBe(false);
    await expect(b.diagPrepare('s', '1', 'disk', null, { full_path: false, child_summary: false }, null)).rejects.toMatchObject({ reason: 'diag_not_configured' });
  });
});
