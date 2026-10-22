// 分析 › 磁盘 → 手选移到回收站 (MTR-01..05, GUI-09/12/13). The person picks scanned entries;
// each is inspected read-only (size, protected descendants, unread parts, processes holding it),
// then the trashable ones are frozen into one manual-selection plan for the shared review page.
// Only entry ids cross IPC; the plan moves nothing until it is approved and applied.

import type { App } from '../main';
import { formatBytes, groupDigits } from '../lib/bytes';
import { h, icon, replace } from '../lib/dom';
import { RequestGate } from '../lib/gate';
import { words } from '../lib/platform';
import type { Entry, TrashInspection } from '../lib/types';
import { atLeast, ensureReady, sumSizes, toIpcError } from './kit';
import { errorText } from './parts';
import { refusalText as codeRefusalText } from '../lib/codes';

const MAX_ENTRIES = 50;

const REFUSAL: Record<string, string> = {
  protected: '这个对象受保护，只能查看或复制路径',
  protected_descendants: '其中有受保护的对象；请改选其中未受保护的子项',
  coverage_gap: '其中有未能读取的部分，无法确认全部内容',
  cloud_placeholder: '这是云端占位文件，没有下载到本机',
  hard_denied: '属于系统硬拒绝范围',
  no_recycle_bin: '所在的卷没有可用的回收站',
  mounted: '这是挂载点或正在挂载的映像',
  root: '不能选择扫描根目录本身',
};

function refusalText(i: TrashInspection): string {
  const r = i.refusal;
  if (!r) return '';
  // A reason without words here reads through the shared code table (never the host's raw message).
  const base = REFUSAL[r.reason] ?? codeRefusalText({ reason: r.reason });
  return base.replace('回收站', words().bin);
}

/** Opens the review dialog for the selected entries; resolves when it closes. */
export async function reviewTrash(app: App, scanId: string, entries: Entry[]): Promise<void> {
  if (!entries.length) return;
  const w = words();
  if (!(await ensureReady(app, 'trash', w.moveToBin))) return;
  const picked = entries.slice(0, MAX_ENTRIES);
  const gate = new RequestGate();
  const ticket = gate.next();
  const results = new Map<string, TrashInspection | string>();
  let creating = false;
  let error: string | null = null;

  const dlg = h('dialog', { class: 'cm-dialog trash-review', 'aria-labelledby': 'trash-title' }) as HTMLDialogElement;
  const list = h('ul', { class: 'trash-list', 'aria-live': 'polite' });
  const status = h('p', { class: 'muted small', role: 'status' });
  const go = h('button', { type: 'button', class: 'primary', id: 'trash-create', disabled: true }, '生成计划');
  const cancel = h('button', { type: 'button', id: 'trash-cancel' }, '取消');
  const errEl = h('p', { class: 'form-error', role: 'alert', hidden: true });
  const close = () => {
    gate.invalidate(); // late inspections never write into a closed dialog
    dlg.close();
    dlg.remove();
  };
  cancel.addEventListener('click', close);
  dlg.addEventListener('cancel', (e) => {
    e.preventDefault();
    close();
  });

  const ok = () => [...results.values()].filter((r): r is TrashInspection => typeof r !== 'string' && r.trashable);
  const render = () => {
    replace(
      list,
      ...picked.map((e) => {
        const r = results.get(e.entry_id);
        const ins = typeof r === 'string' ? null : (r ?? null);
        const state = r === undefined ? 'pending' : typeof r === 'string' ? 'error' : r.trashable ? 'ok' : 'no';
        return h(
          'li',
          { 'data-state': state },
          h('span', { class: 'tr-mark', 'aria-hidden': 'true' }, state === 'ok' ? icon('check') : state === 'pending' ? null : icon(state === 'no' ? 'lock' : 'warn')),
          h(
            'span',
            { class: 'tr-main' },
            h('b', { class: 'tr-name' }, e.name),
            h('span', { class: 'path muted small' }, e.path_display),
            state === 'pending' ? h('span', { class: 'small muted' }, '正在检查…') : null,
            typeof r === 'string' ? h('span', { class: 'why-not' }, r) : null,
            ins && !ins.trashable ? h('span', { class: 'why-not' }, refusalText(ins)) : null,
            ins && ins.protected.length
              ? h(
                  'span',
                  { class: 'tr-sub small' },
                  `受保护的子项 ${groupDigits(ins.protected_count)} 个：`,
                  ins.protected
                    .slice(0, 3)
                    .map((p) => `${p.path_display}（${p.reason}）`)
                    .join('；'),
                  ins.protected_count > 3 ? ` 等` : '',
                )
              : null,
            ins && ins.gap_count ? h('span', { class: 'tr-sub small' }, `未读取的部分 ${groupDigits(ins.gap_count)} 处：${ins.coverage_gaps.slice(0, 2).map((g) => g.path_display).join('；')}`) : null,
            ins && ins.holders && ins.holders.length ? h('span', { class: 'tr-sub small' }, `正被使用：${ins.holders.join('、')}。执行时会跳过。`) : null,
            ins && !ins.recycle_bin_ready ? h('span', { class: 'why-not' }, ins.recycle_bin_problem ?? `所在的卷没有可用的${w.bin}`) : null,
          ),
          h('span', { class: 'tr-size num' }, ins ? (ins.logical_bytes === null ? '大小未知' : formatBytes(ins.logical_bytes)) : formatBytes(e.logical_bytes)),
        );
      }),
    );
    const good = ok();
    const done = results.size === picked.length;
    const sum = sumSizes(good.map((g) => ({ bytes: g.logical_bytes })));
    status.textContent = done
      ? `${groupDigits(good.length)} / ${groupDigits(picked.length)} 项可以${w.moveToBin}，${atLeast(sum)}。${entries.length > MAX_ENTRIES ? `一次最多 ${MAX_ENTRIES} 项，其余未包含。` : ''}`
      : `正在检查 ${groupDigits(results.size)} / ${groupDigits(picked.length)} 项（只读）…`;
    go.disabled = !done || good.length === 0 || creating;
    go.textContent = creating ? '正在冻结计划…' : good.length ? `生成计划（${groupDigits(good.length)} 项）` : '没有可以处理的对象';
    errEl.hidden = !error;
    errEl.textContent = error ?? '';
  };

  go.addEventListener('click', async () => {
    const good = ok();
    if (!good.length || creating) return;
    creating = true;
    error = null;
    render();
    try {
      const r = await app.backend.trashPlan(scanId, good.map((g) => g.entry_id));
      close();
      app.announce('计划已生成，尚未执行。请审阅完整清单');
      app.openPlan(r.plan_id, 'disk');
    } catch (e) {
      error = errorText(toIpcError(e));
      creating = false;
      render();
    }
  });

  dlg.append(
    h(
      'div',
      { class: 'dlg-head' },
      h('h2', { id: 'trash-title' }, `${w.moveToBin}：手选的 ${groupDigits(picked.length)} 项`),
      h('p', { class: 'muted' }, `先逐项只读检查，再把可以处理的对象冻结成一份计划。批准并执行后才会移动；之后可以${w.restoreFromBin}。`),
    ),
    h('div', { class: 'dlg-body' }, list, status, errEl),
    h('div', { class: 'dlg-actions' }, cancel, go),
  );
  document.body.append(dlg);
  render();
  dlg.showModal();

  for (const e of picked) {
    try {
      const r = await gate.run(ticket, app.backend.trashInspect(scanId, e.entry_id));
      if (r.stale) return;
      results.set(e.entry_id, r.value);
    } catch (err) {
      if (!gate.isCurrent(ticket)) return;
      results.set(e.entry_id, errorText(toIpcError(err)));
    }
    render();
  }
  requestAnimationFrame(() => (go.disabled ? cancel : go).focus());
}
