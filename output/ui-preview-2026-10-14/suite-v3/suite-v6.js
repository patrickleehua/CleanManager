'use strict';
/* CleanManager prototype v6: D14 five-tool IA + spec/gate status + new parity demos.
   Loaded after app.js, features-v4.js and ai-diagnostics-v5.js; those files are unchanged.
   All objects, identities, sizes and results are synthetic. Nothing touches the local machine. */

const V6 = {
  platform: 'win', last: {}, open: new Set(['un-cache', 'un-personal']), closed: new Set(),
  un: { app: 'cursor', phase: 'review1', stuck: false, backupFail: false, sel: null, seq: 1, steps: [], timer: null, result: null },
  mnt: { sel: null, phase: 'select', elevation: null, results: {}, order: [], q: '', timer: null },
  st: { sort: 'cpu', q: '', pinned: new Set(), ended: new Set() },
  res: { tray: false, installed: false, metrics: new Set(['cpu', 'mem']), awakeUntil: null, awakeMin: 60, mic: true, cam: false, battAlert: true, thresh: 20, alerted: false, cpuAlert: false },
  ins: { sel: new Set(), showDownloads: false },
  trash: [
    { id: 't-msi', name: '旧版安装包.msi', from: 'C:/Users/Lin/Downloads', when: '昨天 21:40', size: 180 },
    { id: 't-png', name: '截图 2026-08-02.png', from: 'C:/Users/Lin/Pictures/Screenshots', when: '3 天前', size: 3 },
    { id: 't-log', name: 'export-debug.log', from: 'D:/Work/tmp', when: '上周', size: 46 }
  ],
  tsel: new Set(), freeGiB: 41.2,
  pg: { sel: null, mode: 'trash' },
  inst: { sel: new Set(), src: 'all' },
  upd: { check: true, sel: new Set(), results: null, lastCheck: '今天 10:20' },
  su: { state: { onedrive: true, docker: true, adobe: true, oldtool: true, nas: true } },
  readyAck: false, settingsTab: null
};

/* ---------- capability status (derived from tasks.md checkboxes as of 2026-10-15) ---------- */
const V6_CAPS = {
  catalog: { s: 'exp', g: 'G1', reqs: 'CAT-01–09 · PRT-01/03 · GUI-09/13', note: '目录第 1 波属 G1，第 2 波属 G2' },
  aidata: { s: 'exp', g: 'G1', reqs: 'CAT-04 · PRT-01 · AID-02' },
  purge: { s: 'exp', g: 'G1', reqs: 'PUR-01–05 · CLI-12 · GUI-09' },
  installer: { s: 'exp', g: 'G1', reqs: 'INC-01–05 · GUI-09' },
  inventory: { s: 'exp', g: 'G2', mvp: true, reqs: 'APP-01–04（MVP）· UNI-10 · UPD-01' },
  uninstall: { s: 'spec', g: 'G3', reqs: 'UNI-01–10 · GUI-09/10/13/14' },
  updates: { s: 'spec', g: 'G3', reqs: 'UPD-01–04' },
  startup: { s: 'exp', g: 'G3', reqs: 'STA-01–04 · PRT-06' },
  maintenance: { s: 'spec', g: 'G3', reqs: 'MNT-01–05 · MNT-07 · PRT-06 · GUI-10' },
  perf: { s: 'spec', g: 'G3', reqs: 'MNT-06 · STS-04' },
  disk: { s: 'exp', g: 'G1', reqs: 'MTR-01/02/04/05 · DIN-01 · GUI-12' },
  trash: { s: 'exp', g: 'G1', reqs: 'MTR-01/03/05 · GUI-10/12' },
  insights: { s: 'exp', g: 'G1', reqs: 'DIN-01–04 · CAT-07' },
  status: { s: 'exp', g: 'G3', reqs: 'STS-01–06' },
  resident: { s: 'spec', g: 'G4', reqs: 'RES-01–07 · STS-05 · GUI-08' },
  plans: { s: 'exp', g: 'G1', mvp: true, reqs: 'PLN-01–07（MVP）· GUI-09' },
  activity: { s: 'exp', g: 'G1', mvp: true, reqs: 'REC-04/05（MVP）· MNT-07 · STA-02' },
  recovery: { s: 'exp', g: 'G1', mvp: true, reqs: 'REC-01–03（MVP）· UNI-06' },
  agent: { s: 'exp', g: 'G1', reqs: 'CLI-10–13' },
  diagnosis: { s: 'exp', g: null, reqs: 'AID-01–13（MVP 可选扩展）· STS-04' },
  settings: { s: 'exp', g: 'G2', mvp: true, reqs: 'PRT-01/02 · RES-07 · LIF-04/05 · GUI-08' }
};
const V6_TOOLS = [
  { id: 'clean', name: '清理', tabs: [['clean', '清理目录', 'catalog'], ['ai', 'AI 工具数据', 'aidata'], ['purge', '构建产物', 'purge'], ['installers', '安装包', 'installer']] },
  { id: 'software', name: '软件', tabs: [['apps', '应用清单', 'inventory'], ['uninstall', '卸载', 'uninstall'], ['updates', '更新', 'updates'], ['startup', '启动项', 'startup']] },
  { id: 'optimize', name: '优化', tabs: [['optimize', '维护任务', 'maintenance'], ['perf', '性能诊断', 'perf']] },
  { id: 'analyze', name: '分析', tabs: [['disk', '磁盘分析', 'disk'], ['insights', '空间洞察', 'insights'], ['trashbin', null, 'trash']] },
  { id: 'status', name: '状态', tabs: [['status', '系统状态', 'status'], ['resident', '常驻工具', 'resident']] }
];
const V6_EXT = [
  ['plans', '计划', '6', '<path d="M7 3h7l4 4v14H7z"/><path d="M14 3v4h4M10 12h5M10 16h5"/>'],
  ['activity', '活动', '7', '<circle cx="12" cy="12" r="8.5"/><path d="M12 7.5V12l3 2"/>'],
  ['recovery', '恢复', '8', '<path d="M5 9a8 8 0 1 1-1 5"/><path d="M4 4v5h5"/>'],
  ['agent', 'Agent', '9', '<rect x="3.5" y="5" width="17" height="14" rx="2"/><path d="m8 10 3 2-3 2M13 15h3"/>'],
  ['diagnosis', 'AI 诊断', '0', '<circle cx="10.5" cy="10.5" r="6"/><path d="m15 15 5 5M10.5 8v3M10.5 13.2v.1"/>'],
  ['settings', '设置', ',', '<path d="M4 7h10M18 7h2M4 17h4M12 17h8"/><circle cx="16" cy="7" r="2"/><circle cx="10" cy="17" r="2"/>']
];
const V6_PAGE_CAP = { run: 'plans', coverage: 'catalog', rules: 'catalog', capabilities: 'settings' };
Object.assign(pageNames, { purge: '构建产物', installers: '安装包', uninstall: '卸载', updates: '更新', startup: '启动项', optimize: '维护任务', perf: '性能诊断', insights: '空间洞察', trashbin: '回收站', resident: '常驻工具', status: '系统状态', clean: '清理目录', ai: 'AI 工具数据', apps: '应用清单', disk: '磁盘分析', plans: '计划', activity: '活动', recovery: '恢复' });

const v6Bin = () => V6.platform === 'mac' ? '废纸篓' : '回收站';
const v6Reveal = () => V6.platform === 'mac' ? '在访达中显示' : '在资源管理器中显示';
const v6Esc = s => esc(s);
function v6Family(page) { return V6_TOOLS.find(t => t.tabs.some(x => x[0] === page)); }
function v6CapKey(page) { const f = v6Family(page); if (f) return f.tabs.find(x => x[0] === page)[2]; return V6_CAPS[page] ? page : V6_PAGE_CAP[page]; }
function v6Status(s) { return `<span class="v6-status" data-s="${s}">${{ spec: '已规格', exp: '实验', na: '不可用' }[s]}</span>`; }
function v6Gate(g) {
  if (!g) return '<span class="v6-gate">不阻塞门禁</span>';
  const n = +g.slice(1);
  return `<span class="v6-gate" role="img" aria-label="发布门禁 ${g}">${[1, 2, 3, 4].map(i => `<i class="${i === n ? 'on' : ''}"></i>`).join('')}<b>${g}</b></span>`;
}
function v6Capline(key) {
  const c = V6_CAPS[key]; if (!c) return '';
  return `<div class="v6-capline">${v6Status(c.s)}${v6Gate(c.g)}${c.mvp ? '<span>MVP 能力</span>' : ''}<span class="v6-reqs">${c.reqs}</span><button class="v6-link" onclick="v6CapInfo('${key}')">状态说明</button></div>`;
}
function v6CapInfo(key) {
  const c = V6_CAPS[key];
  modal('规格与门禁状态', `<p class="v6-muted">原型不代表实现完成。状态按 <b>add-mole-parity-suite/tasks.md</b>（2026-10-15）与 MVP 变更的勾选情况整理。</p>
  <dl class="v6-facts"><dt>${v6Status('spec')}</dt><dd>已有规格与验收条件，尚无实现任务完成；能力在产品中关闭。</dd>
  <dt>${v6Status('exp')}</dt><dd>部分实现任务已勾选，但所属门禁的双平台真机证据未齐；能力默认关闭，只在开发构建中试用。</dd>
  <dt>${v6Status('na')}</dt><dd>当前平台、硬件或权限不提供可验证接口；在原位置说明原因，不隐藏入口（GUI-08）。</dd></dl>
  <h3 style="margin-top:16px">发布门禁（design D15）</h3>
  <dl class="v6-facts"><dt>${v6Gate('G1')}</dt><dd>核心/CLI：规则 v2、保护三层、目录第 1 波、手选回收站、构建产物、安装包、空间洞察</dd>
  <dt>${v6Gate('G2')}</dt><dd>桌面首版：五工具信息架构、统一选择/结果组件、权限就绪检查、目录第 2 波</dd>
  <dt>${v6Gate('G3')}</dt><dd>软件与系统：卸载两段计划、更新、启动项、维护与提权工作进程、状态采样</dd>
  <dt>${v6Gate('G4')}</dt><dd>常驻与特权：托盘组件、亮屏/隐私/电量提醒、特权 helper 与硬件矩阵、Touch ID</dd></dl>
  ${c ? `<div class="v6-notice">当前页面：${v6Status(c.s)} ${v6Gate(c.g)} · ${c.reqs}${c.note ? '<br>' + c.note : ''}</div>` : ''}`);
}

/* ---------- small shared pieces ---------- */
function v6Hash(str) { let h1 = 0x811c9dc5, out = ''; for (let r = 0; r < 8; r++) { for (const ch of str + r) { h1 ^= ch.charCodeAt(0); h1 = Math.imul(h1, 16777619) >>> 0; } out += h1.toString(16).padStart(8, '0'); } return out; }
function v6Sum(list) { const known = list.filter(i => i.size !== null && i.size !== undefined); return { known: known.reduce((n, i) => n + i.size, 0), unknown: list.length - known.length }; }
function v6Selbar(count, list, label, action, extra = '') {
  const s = v6Sum(list);
  return `<div class="v6-selbar" role="region" aria-label="选择栏"><div><b>已选 ${count} 项 · 已知 ${size(s.known)}${s.unknown ? ` · ${s.unknown} 项大小未知` : ''}</b><small>${extra || '选择不会修改文件；写动作先进入计划审阅'}</small></div><button class="primary" ${count ? '' : 'disabled'} onclick="${action}">${label}</button></div>`;
}
function v6Result(o) {
  return `<div class="v6-result"><div><span>已处理</span><b>${o.count}</b><small>${o.countNote || ''}</small></div><div><span>已处理逻辑量</span><b>${o.logical}</b><small>${o.logicalNote || ''}</small></div><div><span>预计释放</span><b>${o.release}</b><small>${o.releaseNote || ''}</small></div><div><span>卷空闲 前 → 后（观测）</span><b>${o.free}</b><small>${o.freeNote || ''}</small></div></div>`;
}
function v6Tier(t) { return `<span class="v6-tier" data-t="${t}">${{ default: '默认可选', review: '仅审阅', blocked: '拒绝', kept: '保留' }[t]}</span>`; }
function v6Out(o, label) { return `<span class="v6-out" data-o="${o}">${label || { applied: 'applied 已应用', unchanged: 'unchanged 无需更改', skipped: 'skipped 已跳过', unavailable: 'unavailable 不可用', attention: 'attention 需关注', failed: 'failed 失败', pending: '等待', running: '进行中' }[o]}</span>`; }
function v6Open(key, open) { open ? V6.open.add(key) : V6.open.delete(key); }
function v6Live(text) { const l = $('v6Live'); if (l) l.textContent = text; }
function v6Spark(seed, n = 28) { const v = Array.from({ length: n }, (_, i) => 16 - Math.sin(i * .5 + seed) * 6 - Math.cos(i * .21 + seed * 2) * 5); return `<svg class="v6-spark" viewBox="0 0 200 32" preserveAspectRatio="none" aria-hidden="true"><polyline fill="none" stroke="currentColor" stroke-width="1.6" points="${v.map((y, i) => (i * 200 / (n - 1)).toFixed(1) + ',' + y.toFixed(1)).join(' ')}"/></svg>`; }

/* ---------- legacy integration ---------- */
heading = function (title, subtitle = '', actions = '') { return '<div class="page-heading"><div><h1>' + title + '</h1><div class="subtitle">' + subtitle + '</div></div><div class="heading-actions">' + actions + '</div></div>'; };
const v6MakePlan = makePlan;
makePlan = function (kind, items, opts = {}) { v6MakePlan(kind, items, { ...opts, concept: false }); };
uninstallConcept = function (id) { V6.un.app = id; v6UnReset(); route('uninstall'); };
updateConcept = function () { route('updates'); };
startupConcept = function () { route('startup'); };
maintenanceInfo = function () { route('optimize'); };
showTray = function () { v6TrayDialog(); };
const v6StartScan = startScan;
startScan = function (...args) {
  if (V6.readyAck) return v6StartScan(...args);
  const mac = V6.platform === 'mac';
  modal('开始前：权限就绪检查', `<p class="v6-muted">扫描开始前一次性说明缺口；运行中不会再弹出可避免的权限请求（GUI-13）。</p>
  <div class="v6-rows">${(mac ? [['完全磁盘访问', '未授予', '邮件、Safari 与部分应用容器的缓存不可读，相应类目显示“不可读”'], ['管理员', '按需', '系统级日志仅在审阅计划时一次性请求']] : [['管理员', '当前未提升', '系统级日志、NVIDIA 安装缓存与 Windows 错误报告的系统部分不可读'], ['文件访问', '正常', '用户目录内所选范围可读']]).map(([n, s, d]) => `<div class="v6-row"><span class="v6-nobox" aria-hidden="true"></span><div class="v6-row-main"><span class="v6-row-name">${n} <span class="v6-pill ${s === '正常' || s === '按需' ? 'good' : 'warn'}">${s}</span></span><span class="v6-row-meta">${d}</span></div></div>`).join('')}</div>
  <div class="v6-notice">可以按当前权限继续，缺口会在结果中单列为“未扫描”，不显示为 0。</div>`,
  `<button onclick="closeDialog();toast('${mac ? '演示：打开系统设置 › 隐私与安全性 › 完全磁盘访问' : '演示：将在审阅计划时一次性请求管理员'}')">${mac ? '打开授权指引' : '了解管理员授权'}</button><button class="primary" onclick="V6.readyAck=true;closeDialog();startScan()">按当前权限继续扫描</button>`);
};

/* Disk analysis: manual selection → trash-only plan (MTR-01..05) */
function v6DiskPath(id) { const trail = []; (function walk(n, acc) { if (n.id === id) { trail.push(...acc, n); return true; } return (n.children || []).some(c => walk(c, [...acc, n])); })(tree, []); return trail; }
function v6NodePath(id) { const t = v6DiskPath(id); return 'C:/Users/Lin' + t.slice(1).map(n => '/' + n.name).join(''); }
function v6Descendants(n) { return (n.children || []).flatMap(c => [c, ...v6Descendants(c)]); }
function v6TrashCheck(id) {
  const n = findDisk(id), trail = v6DiskPath(id);
  if (!n) return { ok: false, why: '对象不存在' };
  if (n.id === 'root') return { ok: false, kind: 'deny', why: '用户主目录根属于平台硬拒绝清单（PRT-03），只能查看' };
  if (n.reason) return { ok: false, kind: 'gap', why: '未读取（' + n.reason + '），无法核验身份' };
  const prot = trail.find(x => x.protected);
  if (prot) return { ok: false, kind: 'protected', why: (prot === n ? '受保护对象' : '祖先“' + prot.name + '”受保护') + '：只能查看、定位与诊断（MTR-02）' };
  if (n.candidate) return { ok: false, kind: 'rule', why: '已命中清理规则，请在清理目录中按规则处理' };
  const d = v6Descendants(n), pd = d.find(x => x.protected), gap = d.find(x => x.reason || x.amount === null);
  if (gap) return { ok: false, kind: 'gap', why: '含未读取子树“' + gap.name + '”，拒绝整体处理；可改选已知后代' };
  if (pd) return { ok: false, kind: 'protected', why: '含受保护后代“' + pd.name + '”，拒绝整体处理；可改选其他已知后代' };
  return { ok: true };
}
const v6DiskMenuLegacy = diskMenu;
diskMenu = function (e, id) {
  v6DiskMenuLegacy(e, id);
  const m = $('contextMenu'), chk = v6TrashCheck(id);
  m.insertAdjacentHTML('beforeend', `<button onclick="toast('演示：${v6Reveal()}，不读取内容、不触发云下载');$('contextMenu').hidden=true">${v6Reveal()}</button><button onclick="toast('已复制路径：${esc(v6NodePath(id))}');$('contextMenu').hidden=true">复制路径</button>` +
    (chk.ok ? `<button onclick="$('contextMenu').hidden=true;v6TrashPlan(['${id}'])">移到${v6Bin()}…</button>` : `<button disabled title="${esc(chk.why)}">移到${v6Bin()}（不可用）</button>`));
};
const v6DiskDetailLegacy = diskDetail;
diskDetail = function (id) {
  v6DiskDetailLegacy(id);
  const chk = v6TrashCheck(id), acts = document.querySelector('#drawerContent .drawer-actions');
  if (!acts) return;
  acts.insertAdjacentHTML('beforebegin', `<div class="v6-notice ${chk.ok ? '' : 'warn'}"><b>手选处理</b><br>${chk.ok ? `可生成只含此对象的“移到${v6Bin()}”计划；不可改为隔离或永久删除，也不匹配预授权策略。` : esc(chk.why)}</div>`);
  acts.insertAdjacentHTML('afterbegin', (chk.ok ? `<button class="primary" onclick="v6TrashPlan(['${id}'])">移到${v6Bin()}…</button>` : '') + `<button onclick="toast('演示：${v6Reveal()}')">${v6Reveal()}</button>`);
};

/* ---------- manual selection plan (trash only) ---------- */
let v6Pending = null;
function v6TrashPlan(ids, source = 'disk') {
  closeDrawer();
  const items = ids.map(id => {
    if (source === 'downloads') { const d = V6_DOWNLOADS.find(x => x.id === id); return { id, name: d.name, path: 'C:/Users/Lin/Downloads/' + d.name, size: d.size, locked: d.locked }; }
    const n = findDisk(id); return { id, name: n.name, path: v6NodePath(id), size: n.amount, locked: id === 'installer' };
  });
  const digest = v6Hash(items.map(i => i.path).join('|') + 'manual_selection');
  v6Pending = { items, digest, source };
  const total = v6Sum(items);
  modal(`审阅计划：移到${v6Bin()}`, `<div class="v6-statement" data-mode="trash"><p class="v6-statement-verb">移到${v6Bin()}：${items.length} 项手选对象</p>
  <dl class="v6-facts"><dt>对象</dt><dd>${items.map(i => `${esc(i.name)} <span class="v6-path">${esc(i.path)}</span>`).join('<br>')}</dd><dt>已知大小</dt><dd>${size(total.known)}${total.unknown ? ' · ' + total.unknown + ' 项未知' : ''}</dd></dl></div>
  <dl class="v6-facts"><dt>计划种类</dt><dd>manual_selection · 动作 fs_trash</dd><dt>处理方式</dt><dd>仅${v6Bin()}；此计划不可改为隔离或永久删除</dd><dt>授权</dt><dd>需本次人工授权${state.preauth ? '；已启用的有限策略不匹配手选计划' : '；不参与任何预授权策略'}</dd><dt>核验</dt><dd>移动前核验身份，移动后核验${v6Bin()}中的对象；不一致记为 unknown 并引导放回</dd><dt>空间</dt><dd>移到${v6Bin()}不释放空间；在${v6Bin()}中永久删除后才释放</dd><dt>摘要</dt><dd class="v6-digest">${digest.slice(0, 16)}…${digest.slice(-8)}</dd></dl>
  <label class="v6-row" style="border:0;padding-bottom:0"><input type="checkbox" id="v6TrashAck" onchange="$('v6TrashGo').disabled=!this.checked"><span class="v6-row-main"><span>我已核对对象与“仅${v6Bin()}”方式</span></span></label>`,
  `<button class="primary" id="v6TrashGo" disabled onclick="v6TrashApply()">授权并移到${v6Bin()}（演示）</button>`);
}
function v6TrashApply() {
  const p = v6Pending; if (!p) return;
  const moved = p.items.filter(i => !i.locked), locked = p.items.filter(i => i.locked);
  moved.forEach(i => V6.trash.unshift({ id: 't-' + i.id + '-' + Date.now(), name: i.name, from: i.path.split('/').slice(0, -1).join('/'), when: '刚刚', size: i.size }));
  state.history.unshift({ id: 'h' + Date.now(), title: `手选移到${v6Bin()} · ${p.items.length} 项`, time: '刚刚', status: locked.length ? '部分完成' : '已完成', amount: v6Sum(moved).known, kind: 'trash' });
  const s = v6Sum(moved);
  modal(locked.length ? '部分完成' : `已移到${v6Bin()}`, v6Result({ count: `${moved.length} / ${p.items.length} 项`, logical: size(s.known), release: '0 MiB', releaseNote: `仍在${v6Bin()}中占用`, free: `${V6.freeGiB} → ${V6.freeGiB} GiB`, freeNote: '同卷移动，空闲不变' }) +
    (locked.length ? `<div class="v6-notice bad"><b>${v6Out('failed', 'failed（被占用）')}</b><br>${locked.map(i => `${esc(i.name)} 内的 setup.log 被 <b>Setup.exe（PID 7720）</b>占用。关闭该程序后可重试；本产品不会结束占用进程（MTR-04）。`).join('<br>')}</div>` : '') +
    `<div class="v6-notice">可在${v6Bin()}中放回；永久删除需在${v6Bin()}页单独生成计划。</div>`,
    `${locked.length ? `<button onclick="v6TrashPlan(${JSON.stringify(locked.map(i => i.id)).replace(/"/g, "'")},'${p.source}')">关闭占用程序后重试</button>` : ''}<button class="primary" onclick="closeDialog();route('trashbin')">打开${v6Bin()}</button>`);
  if (p.source === 'downloads') V6.ins.sel.clear();
  v6Pending = null;
}

/* ---------- 清理 › 构建产物 ---------- */
const V6_PROJECTS = [
  { id: 'web', name: 'web-shop', marker: 'package.json · next.config.js', age: '23 天前修改', recent: false, items: [
    { id: 'pg-next', name: '.next/cache', size: 640, tier: 'default', why: '框架构建缓存 · 本地可重建 · 已实测 Next.js 14' },
    { id: 'pg-dist', name: 'dist', size: 120, tier: 'review', why: '构建输出 · 可能被部署脚本引用' },
    { id: 'pg-nm', name: 'node_modules', size: 1430, tier: 'review', why: '依赖目录 · 需联网重新安装（npm ci 约 2 分钟）' }] },
  { id: 'rust', name: 'rust-cli', marker: 'Cargo.toml', age: '41 天前修改', recent: false, items: [
    { id: 'pg-target', name: 'target', size: 3270, tier: 'default', why: '编译输出 · 带缓存目录签名 CACHEDIR.TAG' }] },
  { id: 'ml', name: 'ml-lab', marker: 'pyproject.toml', age: '2 天前修改 · 近期项目', recent: true, items: [
    { id: 'pg-venv', name: '.venv', size: 2340, tier: 'review', why: '虚拟环境 · 需联网重建；近期项目不预选' },
    { id: 'pg-pyc', name: '__pycache__', size: 12, tier: 'review', why: '字节码缓存 · 近期项目不预选' }] },
  { id: 'docs', name: 'docs-site', marker: 'package.json · .git', age: '64 天前修改', recent: false, items: [
    { id: 'pg-build', name: 'build', size: 85, tier: 'blocked', why: '含 Git 跟踪文件 build/CNAME，拒绝（发现时与执行前各检查一次）' }] },
  { id: 'legacy', name: 'legacy-app', marker: 'pom.xml', age: '120 天前修改', recent: false, items: [
    { id: 'pg-legacy', name: 'target', size: 410, tier: 'kept', why: 'Git 不可用，跟踪状态未知：保留并标记扫描不完整' }] }
];
function v6PgAll() { return V6_PROJECTS.flatMap(p => p.items.map(i => ({ ...i, project: p }))); }
function v6PurgePage() {
  if (!V6.pg.sel) V6.pg.sel = new Set(v6PgAll().filter(i => i.tier === 'default' && !i.project.recent).map(i => i.id));
  const sel = v6PgAll().filter(i => V6.pg.sel.has(i.id));
  return heading('构建产物', '在扫描根内按项目标记识别项目，产物按可重建性分级', '<button onclick="toast(\'演示：扫描根 D:/Code（深度 4），GUI 与 CLI 共用\')">扫描根 D:/Code</button>') +
    `<div class="v6-split"><section>${V6_PROJECTS.map(p => `<details class="v6-group" ${V6.closed.has('pg-' + p.id) ? '' : 'open'} ontoggle="this.open?V6.closed.delete('pg-${p.id}'):V6.closed.add('pg-${p.id}')"><summary><span class="v6-group-title">${p.name}${p.recent ? ' <span class="v6-pill warn">近 7 天</span>' : ''}</span><span class="v6-small v6-muted">${size(p.items.reduce((n, i) => n + i.size, 0))}</span><span class="v6-group-note">${p.marker} · ${p.age}</span></summary><div class="v6-group-body">${p.items.map(i => v6ObjRow(i, V6.pg.sel, 'pg', `D:/Code/${p.name}/${i.name}`)).join('')}</div></details>`).join('')}
    ${v6Selbar(sel.length, sel, '审阅清理计划', 'v6PurgePlan()', `方式：${{ trash: v6Bin(), quarantine: '同卷隔离', delete: '永久删除（已显式选择）' }[V6.pg.mode]} · 非交互调用不含依赖与近期项`)}</section>
    <aside class="v6-stack"><div class="v6-panel"><h2>处理方式</h2><p class="v6-sub">默认可恢复；永久删除必须显式选择。</p><div class="v6-chips" role="group" aria-label="处理方式">${[['trash', v6Bin()], ['quarantine', '同卷隔离'], ['delete', '永久删除']].map(([m, n]) => `<button aria-pressed="${V6.pg.mode === m}" data-f="pg-mode-${m}" onclick="V6.pg.mode='${m}';render()">${n}</button>`).join('')}</div>${V6.pg.mode === 'delete' ? '<div class="v6-notice bad">不可恢复。授权时需再次确认。</div>' : ''}</div>
    <div class="v6-panel"><h2>分级依据</h2><dl class="v6-facts"><dt>${v6Tier('default')}</dt><dd>本地可重建且已实测</dd><dt>${v6Tier('review')}</dt><dd>依赖目录、近期项目或可能被引用</dd><dt>${v6Tier('blocked')}</dt><dd>含版本控制跟踪文件或疑似密钥</dd><dt>${v6Tier('kept')}</dt><dd>无法判定，保留</dd></dl></div></aside></div>`;
}
function v6ObjRow(i, set, ns, path, extra = '') {
  const can = i.tier === 'default' || i.tier === 'review';
  return `<label class="v6-row ${can ? '' : 'is-off'}">${can ? `<input type="checkbox" data-f="${ns}-${i.id}" ${set.has(i.id) ? 'checked' : ''} onchange="this.checked?${'V6.' + ns}.sel.add('${i.id}'):${'V6.' + ns}.sel.delete('${i.id}');render()">` : '<span class="v6-nobox" aria-hidden="true"></span>'}<span class="v6-row-main"><span class="v6-row-name">${esc(i.name)} ${extra}</span><span class="v6-row-meta">${esc(i.why)}</span><span class="v6-path">${esc(path)}</span></span><span class="v6-row-side"><span class="num">${size(i.size)}</span>${v6Tier(i.tier)}</span></label>`;
}
function v6PurgePlan() {
  const items = v6PgAll().filter(i => V6.pg.sel.has(i.id)).map(i => ({ id: i.id, name: i.project.name + '/' + i.name, amount: i.size, root: 'D:/Code/' + i.project.name }));
  makePlan('purge', items, { mode: { trash: v6Bin(), quarantine: '同卷隔离', delete: '永久删除' }[V6.pg.mode] });
}

/* ---------- 清理 › 安装包 ---------- */
const V6_INSTALLERS = [
  { id: 'in-docker', name: 'Docker Desktop Installer.exe', src: '下载', size: 610, tier: 'review', why: '签名 Docker Inc · 已安装 Docker Desktop 4.34' },
  { id: 'in-node', name: 'node-v20.17.0-x64.msi', src: '下载', size: 28, tier: 'review', why: 'MSI 安装包 · 已安装 Node.js 20.17.0' },
  { id: 'in-wechat', name: 'wechat_setup.zip', src: '聊天工具', size: 230, tier: 'review', why: '压缩包前 20 个条目中含 WeChatSetup.exe' },
  { id: 'in-git', name: 'Git-2.46.0-64-bit.exe', src: '包管理器缓存', size: 66, tier: 'review', why: 'winget 下载缓存 · 签名 Johannes Schindelin' },
  { id: 'in-iso', name: 'Win11_23H2.iso', src: '桌面', size: 6350, tier: 'kept', why: '已挂载为 F: · 跳过；可先推出镜像（INC-04）' },
  { id: 'in-office', name: 'OfficeSetup.exe', src: '下载', size: 7, tier: 'kept', why: '正在被安装程序使用 · 跳过' }
];
function v6InstallersPage() {
  const srcs = ['all', ...new Set(V6_INSTALLERS.map(i => i.src))];
  const list = V6_INSTALLERS.filter(i => V6.inst.src === 'all' || i.src === V6.inst.src);
  const sel = V6_INSTALLERS.filter(i => V6.inst.sel.has(i.id));
  return heading('安装包', '在声明位置有限深度查找安装包与镜像；全部仅审阅，不预选', '') +
    `<div class="v6-split"><section><div class="v6-toolbar"><div class="v6-chips" role="group" aria-label="按来源筛选">${srcs.map(s => `<button aria-pressed="${V6.inst.src === s}" data-f="inst-src-${s}" onclick="V6.inst.src='${s}';render()">${s === 'all' ? '全部来源' : s}</button>`).join('')}</div>${V6.inst.src !== 'all' ? `<button class="v6-link" onclick="V6_INSTALLERS.filter(i=>i.src===V6.inst.src&&i.tier==='review').forEach(i=>V6.inst.sel.add(i.id));render()">选择此来源全部可选项</button>` : ''}</div>
    <div class="v6-panel"><div class="v6-rows">${list.map(i => v6ObjRow(i, V6.inst.sel, 'inst', '来源：' + i.src, `<span class="v6-pill">${i.src}</span>`)).join('')}</div></div>
    ${v6Selbar(sel.length, sel, '审阅安装包计划', 'v6InstPlan()', `默认移到${v6Bin()} · 执行前重新核验文件身份`)}</section>
    <aside class="v6-stack"><div class="v6-panel"><h2>未计入</h2><p class="v6-sub">这些文件不被当作安装包：</p><dl class="v6-facts"><dt>报价资料.zip</dt><dd>压缩包内只有文档</dd><dt>backup.7z</dt><dd>已加密，无法列出目录</dd><dt>setup.exe.txt</dt><dd>仅文件名相似，无安装程序特征</dd></dl></div></aside></div>`;
}
function v6InstPlan() { makePlan('installer', V6_INSTALLERS.filter(i => V6.inst.sel.has(i.id)).map(i => ({ id: i.id, name: i.name, amount: i.size })), { mode: v6Bin() }); }

/* ---------- 软件 › 卸载（两段计划） ---------- */
const V6_UN_APPS = [
  { id: 'cursor', name: 'Cursor', ver: '0.42.3', glyph: 'C', color: '#4f6b5e', src: '注册卸载程序', kind: 'vendor', scope: '当前用户', publisher: 'Anysphere, Inc. · 签名有效', cmd: '注册的 UninstallString · 路径存在、发布者与签名一致', procs: 'Cursor.exe（3 个进程）', body: 'C:/Users/Lin/AppData/Local/Programs/cursor', bodySize: 512, est: '约 3.4 GiB' },
  { id: 'git', name: 'Git', ver: '2.46.0', glyph: 'G', color: '#9a5a46', src: 'winget', kind: 'pkg', scope: '全部用户', publisher: 'winget 源清单 Git.Git', cmd: 'winget uninstall --id Git.Git --exact（固定参数适配）', procs: '', body: 'C:/Program Files/Git', bodySize: 340, est: '约 12 MiB', admin: true },
  { id: 'codex', name: 'Codex CLI', ver: '0.39.0', glyph: '›_', color: '#3f5a74', src: 'npm 全局包', kind: 'ai', scope: '当前用户', publisher: '@openai/codex · 最后使用 94 天前', cmd: 'npm uninstall -g @openai/codex（固定参数适配）', procs: '', body: 'C:/Users/Lin/AppData/Roaming/npm/node_modules/@openai/codex', bodySize: 88, est: '约 1.1 GiB' },
  { id: 'portable', name: 'Chrome · 便携版', ver: '129.0', glyph: 'C', color: '#6f8a70', src: '来源未知', kind: 'unknown', scope: '—', publisher: '无安装记录', cmd: '无可验证卸载程序', procs: '', body: 'D:/Apps/ChromePortable', bodySize: 1126, est: '—' },
  { id: 'orphan', name: '孤立残留', ver: '2 项', glyph: '∅', color: '#7a7466', src: '已卸载应用', kind: 'orphan', scope: '—', publisher: '—', cmd: '—', procs: '', body: '—', bodySize: 0, est: '—' },
  { id: 'edr', name: 'Contoso 终端防护', ver: '8.2', glyph: '◍', color: '#5f6480', src: '设备管理代理', kind: 'protected', scope: '系统', publisher: 'Contoso Ltd. · 由组织管理', cmd: '—', procs: '', body: 'C:/Program Files/Contoso/Endpoint', bodySize: 740, est: '—' }
];
const V6_UN_GROUPS = [['body', '应用本体（重新扫描后仍存在）'], ['cache', '缓存与日志'], ['settings', '设置与偏好'], ['personal', '个人数据 / 文档 / 模型'], ['startup', '启动与后台项'], ['system', '系统级位置']];
const V6_LEFT = {
  cursor: [
    { id: 'c-res', g: 'body', name: 'resources/app', path: 'C:/Users/Lin/AppData/Local/Programs/cursor/resources/app', size: 112, ev: '与卸载前本体同一路径；卸载程序未移除', act: 'fs_trash' },
    { id: 'c-cache', g: 'cache', name: 'Cache', path: '%APPDATA%/Cursor/Cache', size: 420, ev: '应用身份 + 已知缓存目录约定', act: 'fs_trash', def: true },
    { id: 'c-gpu', g: 'cache', name: 'GPUCache', path: '%APPDATA%/Cursor/GPUCache', size: 48, ev: '已知缓存目录约定', act: 'fs_trash', def: true },
    { id: 'c-code', g: 'cache', name: 'Code Cache', path: '%APPDATA%/Cursor/Code Cache', size: 96, ev: '已知缓存目录约定', act: 'fs_trash', def: true },
    { id: 'c-logs', g: 'cache', name: 'logs', path: '%APPDATA%/Cursor/logs', size: 36, ev: '日志目录约定', act: 'fs_trash', def: true },
    { id: 'c-dmp', g: 'cache', name: 'Cursor.exe.*.dmp（4 个）', path: '%LOCALAPPDATA%/CrashDumps', size: 22, ev: '崩溃转储文件名含可执行文件名与签名路径', act: 'fs_trash', def: true },
    { id: 'c-settings', g: 'settings', name: 'User/settings.json 与快捷键', path: '%APPDATA%/Cursor/User', size: 0.2, ev: '应用数据目录约定', act: 'fs_trash' },
    { id: 'c-ext', g: 'settings', name: 'extensions', path: 'C:/Users/Lin/.cursor/extensions', size: 310, ev: '应用数据目录约定 · 重新安装需联网', act: 'fs_trash' },
    { id: 'c-hkcu', g: 'settings', name: 'HKCU\\Software\\Cursor', path: '注册表 · 当前用户', size: null, ev: '卸载注册项中的产品标识', act: 'registry_backup_delete', reg: true },
    { id: 'c-models', g: 'personal', name: 'models', path: 'C:/Users/Lin/.cursor/models', size: 2150, ev: '应用数据目录约定 · 删除后需重新下载 2.1 GiB', act: 'fs_trash' },
    { id: 'c-state', g: 'personal', name: 'globalStorage/state.vscdb', path: '%APPDATA%/Cursor/User/globalStorage', size: 64, ev: '匹配敏感模式：含登录令牌与凭据', act: 'fs_trash', warn: '敏感：凭据' },
    { id: 'c-ws', g: 'personal', name: 'workspaceStorage', path: '%APPDATA%/Cursor/User/workspaceStorage', size: 180, ev: 'Cursor Nightly 0.43（测试版）仍安装并使用同一目录', owner: 'shared_owner' },
    { id: 'c-task', g: 'startup', name: '计划任务 “Cursor Updater”', path: '任务计划程序 · 登录触发 · 目标已移除', size: null, ev: '任务定义中的可执行路径与发布者', act: 'startup_toggle' },
    { id: 'c-lnk', g: 'startup', name: '开始菜单快捷方式 Cursor.lnk', path: '%APPDATA%/Microsoft/Windows/Start Menu/Programs', size: 0.01, ev: '快捷方式目标为已卸载本体', act: 'fs_trash' },
    { id: 'c-hklm', g: 'system', name: 'HKLM\\Software\\Classes\\cursor', path: '注册表 · 本机 · URL 协议', size: null, ev: '协议处理程序指向已卸载本体', act: 'registry_backup_delete', reg: true, admin: true },
    { id: 'c-pd', g: 'system', name: 'ProgramData/cursor-updater', path: 'C:/ProgramData/cursor-updater', size: 8, ev: '更新服务数据目录 · 发布者签名一致', act: 'fs_trash', admin: true },
    { id: 'c-shared', g: 'system', name: 'ProgramData/anysphere-shared', path: 'C:/ProgramData/anysphere-shared', size: 31, ev: '名称相近但无身份证据', owner: 'owner_unknown' }
  ],
  codex: [
    { id: 'x-cache', g: 'cache', name: 'cache', path: 'C:/Users/Lin/.codex/cache', size: 180, ev: '工具身份 + 缓存目录约定', act: 'fs_trash', def: true },
    { id: 'x-log', g: 'cache', name: 'log', path: 'C:/Users/Lin/.codex/log', size: 12, ev: '日志目录约定', act: 'fs_trash', def: true },
    { id: 'x-conf', g: 'settings', name: 'config.toml', path: 'C:/Users/Lin/.codex', size: 0.01, ev: '工具配置约定', act: 'fs_trash' },
    { id: 'x-sess', g: 'personal', name: 'sessions（312 个会话）', path: 'C:/Users/Lin/.codex/sessions', size: 870, ev: 'AI 会话：默认保护，需逐项确认（CAT-04 / UNI-07）', act: 'fs_trash', warn: '会话历史' },
    { id: 'x-wt', g: 'personal', name: 'worktrees/feature-login', path: 'C:/Users/Lin/.codex/worktrees/feature-login', size: 46, ev: 'Git 状态显示 3 个未提交修改', blocked: true }
  ],
  git: []
};
function v6UnApp() {
  const id = V6.un.app; let a = V6_UN_APPS.find(x => x.id === id);
  if (a) return a;
  const l = applications.find(x => x.id === id) || applications[0];
  a = { id: l.id, name: l.name, ver: l.version, glyph: l.icon, color: l.color, src: l.source === '安装记录' ? '注册卸载程序' : '来源未知', kind: l.source === '安装记录' ? 'vendor' : 'unknown', scope: '当前用户', publisher: '发布者签名 · 演示', cmd: '注册的 UninstallString · 已核验', procs: l.id === 'wechat' ? 'WeChat.exe' : '', body: l.path, bodySize: l.base, est: '约 ' + size(l.user + (l.items.length ? 300 : 0)) };
  if (!V6_LEFT[l.id]) V6_LEFT[l.id] = [...l.items.map(cid => { const c = getCandidate(cid); return { id: 'g-' + cid, g: 'cache', name: c.name, path: c.root, size: c.amount, ev: c.proof, act: 'fs_trash', def: true }; }), { id: 'g-user-' + l.id, g: 'personal', name: '用户数据', path: l.path + '/User Data', size: l.user, ev: '应用数据目录约定', act: 'fs_trash' }];
  return a;
}
function v6UnReset() { const u = V6.un; clearTimeout(u.timer); u.seq++; u.phase = 'review1'; u.sel = null; u.steps = []; u.result = null; }
function v6UnSelect(id) { const prev = V6.un.phase; V6.un.app = id; const old = V6.un.seq; v6UnReset(); if (prev === 'running') toast(`已切换应用；序号 #${old} 的旧扫描结果将被丢弃，不覆盖当前视图（GUI-14）`); render(); }
function v6UnTrack(a) {
  const ph = V6.un.phase, s1 = ['review1', 'running', 'stuck'].includes(ph) ? 'now' : 'done', s2 = ph === 'done1' ? 'now' : ['review2', 'done2'].includes(ph) ? 'done' : '', s3 = ['review2', 'done2'].includes(ph) ? 'now' : '';
  if (V6.platform === 'mac') return `<div class="v6-notice">macOS 为一个 uninstall 计划：退出应用、本体移到废纸篓、残留分组（默认仅缓存与日志选中）、登录项注销与注册缓存刷新（design D7）。下方以 Windows 两段计划演示。</div>`;
  return `<div class="v6-track" aria-label="卸载两段流程"><div class="v6-step" data-state="${s1}"><b><i>1</i>第一段：退出与卸载程序</b><small>app_quit · tool_adapter</small></div><span class="v6-track-gap" aria-hidden="true"></span><div class="v6-step" data-state="${s2}"><b><i>2</i>重新扫描本体与残留</b><small>以重扫结果为事实，不按退出码推断</small></div><span class="v6-track-gap" aria-hidden="true"></span><div class="v6-step" data-state="${s3}"><b><i>3</i>第二段：残留计划单独审阅</b><small>fs_trash · registry_backup_delete · startup_toggle</small></div></div>`;
}
function v6UninstallPage() {
  const a = v6UnApp(), u = V6.un;
  const list = V6_UN_APPS.some(x => x.id === a.id) ? V6_UN_APPS : [a, ...V6_UN_APPS];
  return heading('卸载', '按安装来源选择卸载方式；残留在卸载后重新扫描，并作为第二份计划单独审阅', '<button onclick="route(\'apps\')">应用清单</button>') +
    (['vendor', 'pkg', 'ai'].includes(a.kind) ? v6UnTrack(a) : '') +
    `<div class="v6-split v6-split-left"><aside class="v6-panel"><h2>选择应用</h2><p class="v6-sub">来源决定卸载方式</p><div class="v6-applist">${list.map(x => `<button class="v6-appbtn" aria-pressed="${x.id === a.id}" data-f="un-app-${x.id}" onclick="v6UnSelect('${x.id}')"><span class="v6-appicon" style="background:${x.color}" aria-hidden="true">${x.glyph}</span><span><b>${esc(x.name)}</b><small>${x.ver} · ${x.src}</small></span></button>`).join('')}</div></aside>
    <section class="v6-stack">${v6UnBody(a)}</section></div>`;
}
function v6UnBody(a) {
  const u = V6.un;
  if (a.kind === 'protected') return `<div class="v6-panel"><div class="v6-panel-head"><div><h2>${esc(a.name)}</h2><p>${a.publisher}</p></div>${v6Status('na')}</div><div class="v6-notice warn">终端安全与设备管理代理不提供卸载（UNI-10）。请联系组织管理员，或使用厂商的官方卸载流程。</div><div class="v6-actions"><button disabled>卸载（不可用）</button><button onclick="toast('演示：打开厂商官方卸载说明')">查看官方流程</button></div></div>`;
  if (a.kind === 'orphan') { const rows = [{ id: 'o-agent', name: '启动项 “SnapNote Helper”', path: 'HKCU Run → C:/Users/Lin/AppData/Local/SnapNote/helper.exe（路径不存在）', size: 0.01, why: '指向已不存在的程序；安装记录与所有已挂载应用位置均确认缺席，闲置 62 天', tier: 'review' }, { id: 'o-data', name: 'SnapNote 数据目录', path: 'C:/Users/Lin/AppData/Roaming/SnapNote', size: 240, why: '卸载注册项残留中的产品标识一致；闲置 62 天（阈值 30 天）', tier: 'review' }, { id: 'o-unk', name: 'NoteSync 缓存', path: 'C:/Users/Lin/AppData/Local/NoteSync', size: 55, why: '名称相近但无身份证据：归属未知，保留', tier: 'kept' }];
    V6.orph = V6.orph || new Set(); const sel = rows.filter(r => V6.orph.has(r.id));
    return `<div class="v6-panel"><div class="v6-panel-head"><div><h2>孤立残留</h2><p>对应应用已通过其他方式卸载；全部为仅审阅，默认不选（UNI-08）</p></div></div><div class="v6-rows">${rows.map(r => `<label class="v6-row ${r.tier === 'kept' ? 'is-off' : ''}">${r.tier === 'kept' ? '<span class="v6-nobox" aria-hidden="true"></span>' : `<input type="checkbox" data-f="orph-${r.id}" ${V6.orph.has(r.id) ? 'checked' : ''} onchange="this.checked?V6.orph.add('${r.id}'):V6.orph.delete('${r.id}');render()">`}<span class="v6-row-main"><span class="v6-row-name">${r.name}</span><span class="v6-row-meta">${r.why}</span><span class="v6-path">${r.path}</span></span><span class="v6-row-side"><span class="num">${size(r.size)}</span>${v6Tier(r.tier)}</span></label>`).join('')}</div>${v6Selbar(sel.length, sel, '审阅残留计划', `makePlan('uninstall',[...V6.orph].map(id=>({id,name:id==='o-agent'?'SnapNote Helper 启动项':'SnapNote 数据目录',amount:id==='o-agent'?0:240})),{mode:'${v6Bin()}'})`, '系统级残留需要管理员；归属不足保持未知')}</div>`; }
  if (a.kind === 'unknown') return `<div class="v6-panel"><div class="v6-panel-head"><div><h2>${esc(a.name)}</h2><p>没有可验证的安装来源记录</p></div><span class="v6-pill warn">能力受限</span></div><dl class="v6-facts"><dt>本体位置</dt><dd class="v6-path">${a.body}</dd><dt>大小</dt><dd>${size(a.bodySize)}</dd><dt>可用方式</dt><dd>只提供本体移到${v6Bin()}与能力说明；不以删除安装目录冒充卸载，残留不自动预选（UNI-01）</dd></dl><div class="v6-actions" style="margin-top:14px"><button onclick="state.diskPath=[tree];route('disk')">在磁盘分析中定位</button><button onclick="toast('演示：打开 设置 › 应用 › 已安装的应用')">打开系统设置</button></div></div>`;
  if (u.phase === 'review1') {
    const mac = V6.platform === 'mac';
    return `<div class="v6-panel"><div class="v6-panel-head"><div><h2>第一段计划：卸载 ${esc(a.name)}</h2><p>${a.ver} · ${a.src} · ${a.scope}</p></div><span class="v6-pill">等待审阅</span></div>
    <div class="v6-statement"><p class="v6-statement-verb">${a.kind === 'pkg' || a.kind === 'ai' ? '由包管理器卸载' : '运行注册的卸载程序'}${a.procs ? '，先请求应用退出' : ''}</p><dl class="v6-facts"><dt>卸载方式</dt><dd>${a.cmd}</dd><dt>发布者</dt><dd>${a.publisher}</dd><dt>本体</dt><dd>${size(a.bodySize)} <span class="v6-path">${a.body}</span></dd>${a.procs ? `<dt>运行中</dt><dd>${a.procs} · 先请求正常退出，最多等待 10 秒；不会自动强制结束</dd>` : ''}<dt>预计残留</dt><dd>${a.est} · <b>估计值</b>；第二段在卸载完成后重新扫描生成，此处不预先授权</dd></dl></div>
    <dl class="v6-facts"><dt>计划种类</dt><dd>uninstall · 动作 ${a.procs ? 'app_quit、' : ''}tool_adapter</dd><dt>权限</dt><dd>${a.admin ? '需要管理员：开始时一次性请求' : '第一段不需要管理员；第二段的本机注册表项需要时再一次性请求'}（GUI-13）</dd><dt>界面</dt><dd>${a.kind === 'vendor' ? '保留卸载程序自身界面；仅在适配已验证时使用静默参数' : '记录包管理器退出码与输出摘要'}</dd></dl>
    ${a.procs ? `<label class="v6-row" style="border:0"><input type="checkbox" data-f="un-stuck" ${u.stuck ? 'checked' : ''} onchange="V6.un.stuck=this.checked;render()"><span class="v6-row-main"><span>演示情景：应用在时限内未退出</span></span></label>` : ''}
    <div class="v6-actions" style="margin-top:8px"><button class="primary" onclick="v6UnRun()">授权并开始卸载（演示）</button><button onclick="makePlan('uninstall',[{id:'${a.id}',name:'${esc(a.name)} 第一段卸载',amount:${a.bodySize}}],{mode:'${mac ? '废纸篓' : '卸载程序'}'})">在计划页审阅</button></div></div>`;
  }
  if (u.phase === 'running' || u.phase === 'stuck') {
    return `<div class="v6-panel" aria-live="polite"><div class="v6-panel-head"><div><h2>正在执行第一段 · 序号 #${u.seq}</h2><p>分母未知，不显示百分比（GUI-14）</p></div></div>${u.phase === 'running' ? '<div class="v6-indet" aria-hidden="true"></div>' : ''}
    <ul class="v6-steps">${u.steps.map(s => `<li><span>${s.label}</span>${v6Out(s.o, s.word)}${s.note ? `<small>${s.note}</small>` : ''}</li>`).join('')}</ul>
    ${u.phase === 'stuck' ? `<div class="v6-notice warn"><b>${a.procs.split('（')[0]}（PID 9120、9188）仍在运行</b><br>正常退出请求已超时。强制结束可能丢失未保存内容；取消则不执行任何删除。</div><div class="v6-actions"><button onclick="v6UnCancel()">取消卸载</button><button class="v6-danger" onclick="v6UnForce()">强制结束并继续</button></div>` : ''}</div>`;
  }
  if (u.phase === 'cancelled') return `<div class="v6-panel"><h2>已取消</h2><p class="v6-sub">未执行任何删除，应用保持原状（UNI-02）。</p><ul class="v6-steps">${u.steps.map(s => `<li><span>${s.label}</span>${v6Out(s.o, s.word)}${s.note ? `<small>${s.note}</small>` : ''}</li>`).join('')}</ul><div class="v6-actions"><button onclick="v6UnReset();render()">重新审阅第一段</button></div></div>`;
  if (u.phase === 'done1') {
    const left = V6_LEFT[a.id] || [], partial = left.some(l => l.g === 'body');
    return `<div class="v6-panel"><div class="v6-panel-head"><div><h2>第一段${partial ? '部分完成' : '完成'}</h2><p>重扫序号 #${u.seq} · 卸载程序退出码 0 · 用时 14 秒</p></div>${v6Out(partial ? 'attention' : 'applied', partial ? '部分完成' : '已卸载')}</div>
    <ul class="v6-steps">${u.steps.map(s => `<li><span>${s.label}</span>${v6Out(s.o, s.word)}${s.note ? `<small>${s.note}</small>` : ''}</li>`).join('')}</ul>
    ${partial ? '<div class="v6-notice warn">卸载程序返回成功，但重新扫描仍发现安装目录的一部分。剩余对象已进入残留预览，不根据退出码报告“完全卸载”（UNI-03）。</div>' : ''}
    ${left.length ? `<div class="v6-actions"><button class="primary" onclick="V6.un.phase='review2';render()">审阅残留（第二段计划）</button></div>` : `<div class="v6-notice">未发现与 ${esc(a.name)} 身份关联的残留。</div>`}</div>`;
  }
  if (u.phase === 'review2') return v6UnReview2(a);
  if (u.phase === 'done2') return v6UnDone2(a);
  return '';
}
function v6UnRun() {
  const a = v6UnApp(), u = V6.un, seq = u.seq;
  u.phase = 'running';
  u.steps = [
    ...(a.procs ? [{ label: '请求应用正常退出', o: 'pending' }] : []),
    { label: a.kind === 'vendor' ? '运行注册卸载程序（保留其界面）' : '运行包管理器卸载适配', o: 'pending' },
    { label: '重新扫描本体与残留', o: 'pending' }];
  let i = 0;
  const tick = () => {
    if (u.seq !== seq || u.phase !== 'running') return;
    if (i > 0) u.steps[i - 1].o = 'applied', u.steps[i - 1].word = '完成';
    if (i === 1 && a.procs) { if (u.stuck) { u.steps[0].o = 'attention'; u.steps[0].word = '超时'; u.steps[0].note = '已等待 10 秒'; u.phase = 'stuck'; if (state.page === 'uninstall') render(); return; } u.steps[0].note = '3 个进程已退出'; }
    if (i >= u.steps.length) { const last = u.steps.at(-1); last.note = '生成序号 #' + seq; u.steps.at(-2).note = a.kind === 'vendor' ? '退出码 0 · 用时 14 秒' : '退出码 0 · 输出摘要已记录'; u.phase = 'done1'; v6Live('第一段完成'); if (state.page === 'uninstall') render(); return; }
    u.steps[i].o = 'running'; u.steps[i].word = '进行中'; i++;
    if (state.page === 'uninstall') render();
    u.timer = setTimeout(tick, state.reduceMotion ? 50 : 700);
  };
  tick();
}
function v6UnCancel() { const u = V6.un; u.steps.forEach(s => { if (s.o === 'pending') { s.o = 'skipped'; s.word = '未执行'; } }); u.phase = 'cancelled'; render(); }
function v6UnForce() {
  modal('强制结束进程？', '<p>将结束 Cursor.exe（PID 9120、9188）。未保存的内容会丢失。此确认只针对本次。</p>', `<button class="v6-danger" onclick="closeDialog();V6.un.steps[0].o='applied';V6.un.steps[0].word='已强制结束';V6.un.steps[0].note='用户逐次确认';V6.un.stuck=false;V6.un.phase='running';v6UnResume()">强制结束</button>`);
}
function v6UnResume() { const u = V6.un, seq = u.seq; let i = 1; const tick = () => { if (u.seq !== seq || u.phase !== 'running') return; if (i > 1) { u.steps[i - 1].o = 'applied'; u.steps[i - 1].word = '完成'; } if (i >= u.steps.length) { u.steps.at(-1).note = '生成序号 #' + seq; u.steps.at(-2).note = '退出码 0 · 用时 14 秒'; u.phase = 'done1'; render(); return; } u.steps[i].o = 'running'; u.steps[i].word = '进行中'; i++; render(); u.timer = setTimeout(tick, state.reduceMotion ? 50 : 700); }; tick(); }
function v6UnLeftSel(a) { const u = V6.un; if (!u.sel) u.sel = new Set((V6_LEFT[a.id] || []).filter(l => l.def).map(l => l.id)); return u.sel; }
function v6UnReview2(a) {
  const left = V6_LEFT[a.id] || [], sel = v6UnLeftSel(a), chosen = left.filter(l => sel.has(l.id));
  const admin = chosen.some(l => l.admin), regs = chosen.filter(l => l.reg).length;
  return `<div class="v6-panel"><div class="v6-panel-head"><div><h2>第二段：${esc(a.name)} 的残留</h2><p>来自重扫序号 #${V6.un.seq}；仅缓存与日志默认选中，个人数据、设置与模型默认不选</p></div><span class="v6-pill">单独审阅</span></div>
    ${V6_UN_GROUPS.map(([g, title]) => { const rows = left.filter(l => l.g === g); if (!rows.length) return ''; const n = rows.filter(r => sel.has(r.id)).length; return `<details class="v6-group" ${V6.open.has('un-' + g) || g === 'body' ? 'open' : ''} ontoggle="v6Open('un-${g}',this.open)"><summary><span class="v6-group-title">${title}${rows.some(r => r.admin) ? ' <span class="v6-pill warn">需管理员</span>' : ''}</span><span class="v6-small v6-muted">${n} / ${rows.length} 已选 · ${size(v6Sum(rows).known)}</span>${g === 'personal' ? '<span class="v6-group-note">默认不选；删除后无法由应用重新生成</span>' : g === 'cache' ? '<span class="v6-group-note">可由应用重新生成</span>' : ''}</summary><div class="v6-group-body">${rows.map(r => v6UnRow(r, sel)).join('')}</div></details>`; }).join('')}
    <div class="v6-notice">外部配置引用（只读提示）：<b>C:/Users/Lin/.gitconfig</b> 中 core.editor = cursor。产品不修改该文件，请在卸载后自行调整。</div>
    <label class="v6-row" style="border:0"><input type="checkbox" data-f="un-bfail" ${V6.un.backupFail ? 'checked' : ''} onchange="V6.un.backupFail=this.checked;render()"><span class="v6-row-main"><span>演示情景：注册表备份位置不可写</span><span class="v6-row-meta">备份失败会阻止对应的注册表删除，文件残留仍按计划处理（UNI-06）</span></span></label>
    ${v6Selbar(chosen.length, chosen, '审阅残留计划', 'v6UnPlan2()', `文件与快捷方式移到${v6Bin()} · ${regs} 个注册表项先备份${admin ? ' · 含需管理员项，开始时一次性请求' : ''}`)}</div>`;
}
function v6UnRow(r, sel) {
  const off = r.owner || r.blocked;
  const tag = r.owner ? `<span class="v6-pill">${r.owner}</span>` : r.blocked ? '<span class="v6-pill bad">blocked</span>' : '';
  return `<label class="v6-row ${off ? 'is-off' : ''}">${off ? '<span class="v6-nobox" aria-hidden="true"></span>' : `<input type="checkbox" data-f="un-${r.id}" ${sel.has(r.id) ? 'checked' : ''} onchange="this.checked?V6.un.sel.add('${r.id}'):V6.un.sel.delete('${r.id}');render()">`}<span class="v6-row-main"><span class="v6-row-name">${esc(r.name)} ${tag}${r.warn ? `<span class="v6-pill warn">${r.warn}</span>` : ''}${r.reg ? '<span class="v6-pill">先备份</span>' : ''}</span><span class="v6-row-meta">${esc(r.ev)}${off ? (r.owner === 'shared_owner' ? ' · 保留，不进入计划' : r.blocked ? ' · 拒绝，不进入计划' : ' · 归属不足，保留') : ''}</span><span class="v6-path">${esc(r.path)}</span></span><span class="v6-row-side"><span class="num">${r.size === null ? '未知' : size(r.size)}</span><span class="v6-small v6-muted">${r.act || '—'}</span></span></label>`;
}
function v6UnPlan2() {
  const a = v6UnApp(), chosen = (V6_LEFT[a.id] || []).filter(l => V6.un.sel.has(l.id));
  const files = chosen.filter(l => l.act === 'fs_trash'), regs = chosen.filter(l => l.reg), tasks = chosen.filter(l => l.act === 'startup_toggle'), admin = chosen.filter(l => l.admin);
  const digest = v6Hash(chosen.map(c => c.path).join('|') + 'uninstall-2');
  modal('审阅第二段计划', `<div class="v6-statement" data-mode="trash"><p class="v6-statement-verb">清理 ${esc(a.name)} 残留：${chosen.length} 项</p><dl class="v6-facts"><dt>移到${v6Bin()}</dt><dd>${files.length} 项 · ${size(v6Sum(files).known)}</dd><dt>备份后删除</dt><dd>${regs.length} 个注册表项（备份存入恢复存储，与审计同等保留）</dd><dt>停用并备份定义</dt><dd>${tasks.length} 个启动/后台项</dd></dl></div>
  <dl class="v6-facts"><dt>计划种类</dt><dd>uninstall（第二段）· 不复用第一段授权</dd><dt>管理员</dt><dd>${admin.length ? `${admin.length} 项需要，开始时一次性请求；拒绝则这些项 skipped` : '不需要'}</dd><dt>摘要</dt><dd class="v6-digest">${digest.slice(0, 16)}…${digest.slice(-8)}</dd></dl>`,
  `<button class="primary" onclick="closeDialog();v6UnApply2()">授权并执行（演示）</button>`);
}
function v6UnApply2() {
  const a = v6UnApp(), chosen = (V6_LEFT[a.id] || []).filter(l => V6.un.sel.has(l.id)), bf = V6.un.backupFail;
  const files = chosen.filter(l => l.act === 'fs_trash'), regs = chosen.filter(l => l.reg);
  V6.un.result = { chosen, files, regs, bf };
  if (!bf && regs.length) state.recovery.unshift({ id: 'r-reg-' + Date.now(), name: `${a.name} 注册表备份（${regs.length} 项）`, amount: 0, original: regs.map(r => r.name).join('；'), date: '刚刚', conflict: false });
  state.history.unshift({ id: 'h' + Date.now(), title: `卸载 ${a.name} · 残留计划`, time: '刚刚', status: a.id === 'cursor' || bf ? '部分完成' : '已完成', amount: v6Sum(files).known, kind: 'uninstall' });
  V6_LEFT[a.id] = (V6_LEFT[a.id] || []).filter(l => !V6.un.sel.has(l.id));
  V6.un.phase = 'done2'; render();
}
function v6UnDone2(a) {
  const r = V6.un.result, k = v6Sum(r.files);
  const has = id => r.chosen.some(c => c.id === id), cur = a.id === 'cursor';
  const teardown = cur ? [
    has('c-task') ? ['停用计划任务 “Cursor Updater”（定义已备份）', 'applied', '成功'] : ['计划任务 “Cursor Updater”', 'skipped', '未选择，保留', '目标程序已移除，可稍后在启动项页处理'],
    has('c-lnk') ? ['移除开始菜单快捷方式', 'applied', '成功'] : ['开始菜单快捷方式', 'skipped', '未选择，保留'],
    ['刷新应用注册缓存（文件关联）', 'applied', '成功'],
    ['注销后台更新服务 cursor-update-helper', 'failed', '失败', '服务控制管理器返回“拒绝访问”。手动处理：services.msc 中停止并删除该服务，或重新运行厂商卸载程序。']] : [['刷新应用注册缓存', 'applied', '成功'], ['移除快捷方式', 'applied', '成功']];
  const partial = teardown.some(t => t[1] === 'failed') || (r.bf && r.regs.length);
  return `<div class="v6-panel"><div class="v6-panel-head"><div><h2>${partial ? '卸载部分完成' : '卸载完成'}</h2><p>${esc(a.name)} · 第二段计划已执行</p></div>${partial ? v6Out('attention', '部分完成') : v6Out('applied', '完成')}</div>
  ${v6Result({ count: `${r.chosen.length - (r.bf ? r.regs.length : 0)} / ${r.chosen.length} 项`, logical: size(k.known), logicalNote: k.unknown ? `另 ${k.unknown} 项大小未知` : '', release: '0 MiB', releaseNote: `文件在${v6Bin()}中，仍占用空间`, free: `${V6.freeGiB} → ${V6.freeGiB} GiB`, freeNote: '同卷移动' })}
  ${r.regs.length ? `<div class="v6-notice ${r.bf ? 'bad' : ''}">${r.bf ? `<b>${r.regs.length} 个注册表项 skipped</b>：备份位置不可写，删除被阻止；文件残留已按计划处理。` : `<b>${r.regs.length} 个注册表项已备份后删除。</b>可在恢复页生成恢复计划；导入前先检查目标键是否已被重新创建（UNI-06）。`}</div>` : ''}
  <h3 style="margin:16px 0 6px">收尾步骤（逐项报告）</h3><ul class="v6-steps">${teardown.map(([l, o, w, n]) => `<li><span>${l}</span>${v6Out(o, w)}${n ? `<small>${n}</small>` : ''}</li>`).join('')}</ul>
  <div class="v6-actions" style="margin-top:14px">${!r.bf && r.regs.length ? '<button class="primary" onclick="v6RegRestore()">恢复注册表项…</button>' : ''}<button onclick="route('activity')">在活动中查看</button><button onclick="route('trashbin')">打开${v6Bin()}</button></div></div>`;
}
function v6RegRestore() {
  modal('恢复注册表项', `<p class="v6-muted">恢复以 recovery 计划执行，动作 registry_restore。导入前逐键检查目标是否已被重新创建。</p><div class="v6-rows"><div class="v6-row"><span class="v6-nobox"></span><div class="v6-row-main"><span class="v6-row-name">HKCU\\Software\\Cursor</span><span class="v6-row-meta">目标键不存在 · 将导入备份</span></div><div class="v6-row-side">${v6Out('pending', '待恢复')}</div></div><div class="v6-row"><span class="v6-nobox"></span><div class="v6-row-main"><span class="v6-row-name">HKLM\\Software\\Classes\\cursor</span><span class="v6-row-meta">目标键已被重新创建（Cursor Nightly 注册了同一协议）· 跳过，不覆盖</span></div><div class="v6-row-side">${v6Out('skipped', '将跳过')}</div></div></div>`, `<button class="primary" onclick="closeDialog();toast('演示：已生成恢复计划，等待审阅')">生成恢复计划</button>`);
}

/* ---------- 软件 › 更新 ---------- */
const V6_UPD = [
  { id: 'u-code', name: 'Visual Studio Code', cur: '1.93.1', next: '1.94.0', ch: 'winget', run: true },
  { id: 'u-git', name: 'Git', cur: '2.46.0', next: '2.47.0', ch: 'winget' },
  { id: 'u-cursor', name: 'Cursor', cur: '0.42.3', next: '0.43.1', ch: 'Electron 更新源', note: '仅检测；签名与发布者一致时才执行', bad: true },
  { id: 'u-chrome', name: 'Google Chrome', cur: '129.0', next: '130.0', ch: '应用自带更新', note: '无法由本产品校验，打开应用自身更新' },
  { id: 'u-docker', name: 'Docker Desktop', cur: '4.34.0', next: null, ch: 'winget', note: '已是最新' },
  { id: 'u-portable', name: 'Chrome · 便携版', cur: '129.0', next: undefined, ch: '无', note: '无法检查：没有更新订阅，也不属于任何包源' }
];
function v6UpdatesPage() {
  const u = V6.upd;
  return heading('更新', '只通过可验证的官方通道更新；无法验证时打开应用自身或官方页面', `<label class="v6-actions v6-small"><span>更新检查</span>${`<label class="toggle"><input type="checkbox" aria-label="更新检查" data-f="upd-check" ${u.check ? 'checked' : ''} onchange="V6.upd.check=this.checked;V6.upd.results=null;render()"><span class="switch"></span></label>`}</label>`) +
    (u.check ? '' : `<div class="v6-notice warn">更新检查已关闭：不发起任何网络检测。下表为上次结果（${u.lastCheck}）。</div>`) +
    `<div class="v6-panel"><div class="v6-rows">${V6_UPD.map(i => { const can = u.check && i.next && i.ch === 'winget' || u.check && i.bad; return `<label class="v6-row ${can ? '' : 'is-off'}">${can ? `<input type="checkbox" data-f="upd-${i.id}" ${u.sel.has(i.id) ? 'checked' : ''} onchange="this.checked?V6.upd.sel.add('${i.id}'):V6.upd.sel.delete('${i.id}');render()">` : '<span class="v6-nobox" aria-hidden="true"></span>'}<span class="v6-row-main"><span class="v6-row-name">${i.name}${i.run ? ' <span class="v6-pill warn">运行中 · 先请求退出</span>' : ''}</span><span class="v6-row-meta">通道：${i.ch} · 检测于 ${u.lastCheck}${i.note ? ' · ' + i.note : ''}</span></span><span class="v6-row-side"><span>${i.cur}${i.next ? ' → <b>' + i.next + '</b>' : ''}</span>${i.next === undefined ? '<span class="v6-pill warn">无法检查</span>' : i.next === null ? '<span class="v6-pill good">最新</span>' : i.ch === '应用自带更新' ? `<button class="v6-link" onclick="event.preventDefault();toast('演示：打开 ${i.name} 自身的更新页面')">打开应用</button>` : ''}</span></label>`; }).join('')}</div>
    ${u.results ? `<h3 style="margin:16px 0 6px">批量更新结果</h3><ul class="v6-steps">${u.results.map(r => `<li><span>${r.name}</span>${v6Out(r.o, r.w)}${r.n ? `<small>${r.n}</small>` : ''}</li>`).join('')}</ul>` : ''}</div>` +
    v6Selbar(u.sel.size, [...u.sel].map(() => ({ size: null })), '审阅更新计划', 'v6UpdRun()', '计划种类 update · app_quit + tool_adapter · 单项失败不中止其他项');
}
function v6UpdRun() {
  const picked = V6_UPD.filter(i => V6.upd.sel.has(i.id));
  V6.upd.results = picked.map(i => i.bad ? { name: i.name, o: 'failed', w: '已拒绝', n: '下载包签名者为 “Anysphere Labs LLC”，与已安装应用的发布者不一致；原应用未改动（UPD-02）' } : i.id === 'u-git' ? { name: i.name, o: 'failed', w: '失败', n: 'winget 退出码 0x8A150011：安装程序哈希不匹配。其他项继续执行。' } : { name: i.name, o: 'applied', w: '已更新', n: i.run ? '已请求退出并在更新后重新打开' : '' });
  V6.upd.sel.clear(); render();
}

/* ---------- 软件 › 启动项 ---------- */
const V6_SU = [
  { id: 'onedrive', name: 'Microsoft OneDrive', src: '注册表 Run（HKCU）', target: 'C:/Program Files/Microsoft OneDrive/OneDrive.exe', pub: 'Microsoft Corporation · 签名有效', scope: '用户', kind: 'toggle' },
  { id: 'docker', name: 'Docker Desktop', src: '启动文件夹', target: 'C:/Program Files/Docker/Docker Desktop.exe', pub: 'Docker Inc · 签名有效', scope: '用户', kind: 'toggle' },
  { id: 'adobe', name: 'Adobe Acrobat Update Task', src: '计划任务 · 登录触发', target: 'C:/Program Files (x86)/Common Files/Adobe/ARM/AdobeARM.exe', pub: 'Adobe Inc. · 签名有效', scope: '系统', kind: 'toggle', admin: true },
  { id: 'spotify', name: 'Spotify', src: '商店应用启动任务', target: '包 SpotifyAB.SpotifyMusic', pub: 'Spotify AB', scope: '用户', kind: 'settings' },
  { id: 'oldtool', name: 'QuickClip', src: '注册表 Run（HKCU）', target: 'C:/Tools/QuickClip/qclip.exe', pub: '无法读取', scope: '用户', kind: 'broken' },
  { id: 'nas', name: 'NAS Sync', src: '启动文件夹', target: 'E:/Apps/NasSync/sync.exe', pub: '—', scope: '用户', kind: 'offline' },
  { id: 'edr', name: 'Contoso 终端防护', src: '服务 · 自动启动', target: 'C:/Program Files/Contoso/Endpoint/agent.exe', pub: 'Contoso Ltd. · 由组织管理', scope: '系统', kind: 'protected' },
  { id: 'bg', name: '未匹配的后台项', src: '后台应用权限', target: '无法对应到具体程序', pub: '—', scope: '用户', kind: 'settings' }
];
function v6StartupPage() {
  return heading('启动项', '登录项、Run 键、启动文件夹、计划任务与商店启动任务；只对已验证条目提供开关', '') +
    `<div class="v6-panel"><div class="v6-rows">${V6_SU.map(i => { const on = V6.su.state[i.id];
      const side = i.kind === 'toggle' ? `<label class="toggle"><input type="checkbox" aria-label="${i.name} 开机启动" data-f="su-${i.id}" ${on ? 'checked' : ''} onchange="v6SuToggle('${i.id}',this.checked)"><span class="switch"></span></label>` : i.kind === 'settings' ? `<button onclick="toast('演示：打开 设置 › 应用 › 启动')">在系统设置中打开</button>` : i.kind === 'broken' ? `<button onclick="v6SuBroken('${i.id}')">备份后删除…</button>` : i.kind === 'protected' ? '<span class="v6-pill">受保护 · 只读</span>' : '<span class="v6-pill warn">目标不可达</span>';
      const state2 = i.kind === 'broken' ? (on ? '<span class="v6-pill bad">失效：目标不存在</span>' : '<span class="v6-pill">已删除 · 定义已备份</span>') : i.kind === 'offline' ? '<span class="v6-pill warn">卷离线：E: 未挂载，不判定为失效</span>' : i.kind === 'protected' ? '<span class="v6-pill">安全软件不可停用</span>' : i.kind === 'settings' ? '<span class="v6-pill">无受支持接口</span>' : (on ? '<span class="v6-pill good">启用</span>' : '<span class="v6-pill">已停用（可恢复）</span>');
      return `<div class="v6-row"><span class="v6-nobox" aria-hidden="true" style="opacity:0"></span><div class="v6-row-main"><span class="v6-row-name">${i.name} ${state2}${i.admin ? ' <span class="v6-pill warn">系统级 · 需管理员</span>' : ''}</span><span class="v6-row-meta">${i.src} · ${i.scope} · ${i.pub}</span><span class="v6-path">${i.target}</span></div><div class="v6-row-side">${side}</div></div>`; }).join('')}</div></div>
    <div class="v6-notice">停用使用系统自身的启用/停用记录，不删除定义；每次变更记录在活动中，可从历史恢复原状态（STA-02）。</div>`;
}
function v6SuToggle(id, on) {
  const i = V6_SU.find(x => x.id === id);
  if (i.admin && !on) { modal('需要管理员', `<p>${i.name} 是系统级计划任务。停用需要一次管理员授权；只执行 startup_toggle 这一项固定动作。</p>`, `<button class="primary" onclick="closeDialog();V6.su.state['${id}']=false;v6SuLog('${id}',false)">授权并停用（演示）</button>`); render(); return; }
  V6.su.state[id] = on; v6SuLog(id, on);
}
function v6SuLog(id, on) { const i = V6_SU.find(x => x.id === id); state.history.unshift({ id: 'h' + Date.now(), title: `${on ? '启用' : '停用'}启动项 · ${i.name}`, time: '刚刚', status: '已完成', amount: null, kind: 'startup' }); toast(`${on ? '已启用' : '已停用'} ${i.name}；可在活动中恢复原状态`); render(); }
function v6SuBroken(id) { const i = V6_SU.find(x => x.id === id); modal('删除失效启动项', `<dl class="v6-facts"><dt>条目</dt><dd>${i.name} · ${i.src}</dd><dt>目标</dt><dd>${i.target}（已检查所有已挂载卷，均不存在）</dd><dt>先备份</dt><dd>注册表值定义写入恢复存储；备份失败则不删除</dd></dl>`, `<button class="primary" onclick="closeDialog();V6.su.state['${id}']=false;v6SuLog('${id}',false)">备份后删除（演示）</button>`); }

/* ---------- 优化 › 维护任务 ---------- */
const V6_MNT = {
  win: [
    { id: 'startmenu', name: '重启开始菜单与搜索', problem: '开始菜单或任务栏搜索没有响应', impact: '开始菜单与搜索界面会短暂消失后重新出现', admin: false, eta: '约 10 秒', appl: '适用：StartMenuExperienceHost 与 SearchHost 正在运行', out: 'failed', why: 'SearchHost 未在 15 秒内重新注册；未做其他更改，可稍后重试' },
    { id: 'icons', name: '刷新图标与缩略图缓存', problem: '图标空白、缩略图显示旧内容', impact: '资源管理器重启一次，缩略图逐步重建', admin: false, eta: '约 20 秒', appl: '适用：缓存文件存在（186 MiB）', out: 'applied', why: '已重建 iconcache 与 thumbcache；资源管理器已恢复' },
    { id: 'dns', name: '刷新 DNS 解析缓存', problem: '网站仍解析到旧地址', impact: '下一次访问会重新解析域名', admin: false, eta: '约 1 秒', appl: '适用：DNS 客户端服务运行中', out: 'applied', why: '已清空 214 条解析缓存' },
    { id: 'trim', name: '到期的固态盘 TRIM', problem: '固态盘长期未执行优化', impact: '后台优化 C:，期间磁盘较忙', admin: true, eta: '约 1 分钟', appl: '先读取上次优化时间：C: 为固态盘，上次 3 天前，周期 7 天', out: 'unchanged', why: '未到期：上次优化 2026-10-12 08:14，下次到期 10-19' },
    { id: 'image', name: '系统映像健康检查（只读）', problem: '系统文件疑似损坏、更新反复失败', impact: '只读扫描组件存储，不自动修复', admin: true, eta: '约 3–8 分钟', appl: '适用：组件存储可读取', out: 'attention', why: '发现可修复的组件存储损坏。本产品不自动修复；可在管理员终端运行 DISM /Online /Cleanup-Image /RestoreHealth' },
    { id: 'broken', name: '失效启动项报告', problem: '开机后提示找不到程序', impact: '只读列出指向缺失程序的启动项，处理转到启动项页', admin: false, eta: '约 2 秒', appl: '依赖启动项能力', out: 'unavailable', why: '启动项能力在此构建为实验状态且未开启，无法生成报告' },
    { id: 'index', name: '检查搜索索引状态', problem: '搜索结果缺失', impact: '只读读取索引状态', admin: false, eta: '约 2 秒', appl: '适用', out: 'skipped', why: '用户保护：此任务已加入保护名单，不参与批量选择（PRT-06）', prot: true }
  ],
  mac: [
    { id: 'm-ql', name: '刷新快速预览与图标缓存', problem: '空格预览空白或图标错误', impact: '预览与图标逐步重建', admin: false, eta: '约 5 秒', appl: '适用', out: 'applied', why: '已重置 Quick Look 缓存' },
    { id: 'm-dns', name: '刷新 DNS 解析缓存', problem: '网站仍解析到旧地址', impact: '下一次访问重新解析', admin: true, eta: '约 1 秒', appl: '需要管理员', out: 'applied', why: '已刷新 mDNSResponder 缓存' },
    { id: 'm-spot', name: 'Spotlight 索引检查', problem: '搜索不到新文件', impact: '只读检查索引状态', admin: false, eta: '约 3 秒', appl: '适用', out: 'unchanged', why: '索引状态正常，无需重建' },
    { id: 'm-pref', name: '损坏偏好文件检测', problem: '应用设置反复丢失', impact: '只报告，可将损坏文件隔离', admin: false, eta: '约 10 秒', appl: '适用', out: 'attention', why: '发现 1 个无法解析的偏好文件 com.example.notes.plist；未自动处理' },
    { id: 'm-save', name: '清理过期保存状态', problem: '应用启动恢复旧窗口异常', impact: '移除 30 天前的保存状态', admin: false, eta: '约 2 秒', appl: '适用', out: 'failed', why: '1 个应用正在运行，其保存状态被占用；其余 11 项已清理' },
    { id: 'm-disk', name: '磁盘只读校验', problem: '磁盘错误提示', impact: '只读校验启动卷', admin: true, eta: '约 1 分钟', appl: '适用', out: 'unchanged', why: '卷结构未发现问题' },
    { id: 'm-la', name: '登录项与失效 LaunchAgent 报告', problem: '登录时报错', impact: '只读列出', admin: false, eta: '约 2 秒', appl: '依赖完全磁盘访问', out: 'unavailable', why: '缺少完全磁盘访问，无法读取其他用户作业', prot: false }
  ]
};
const V6_PROHIBITED = /注册表|优化|加速|提速|服务|精简|内存|释放|清理垃圾|registry|boost|tweak|debloat|memory/i;
function v6MntTasks() { return V6_MNT[V6.platform]; }
function v6OptimizePage() {
  const m = V6.mnt, tasks = v6MntTasks();
  if (!m.sel || m.platform !== V6.platform) { m.sel = new Set(tasks.filter(t => !t.prot).map(t => t.id)); m.platform = V6.platform; m.phase = 'select'; m.results = {}; m.elevation = null; }
  const q = m.q.trim(), shown = tasks.filter(t => !q || (t.name + t.problem).includes(q));
  const adminSel = tasks.filter(t => m.sel.has(t.id) && t.admin);
  const body = m.phase === 'select' ? `<div class="v6-toolbar"><label class="v6-search">⌕ <input aria-label="搜索维护任务" data-f="mnt-q" placeholder="按问题搜索，例如“图标”" value="${esc(m.q)}" oninput="V6.mnt.q=this.value;render()"></label><span class="v6-small v6-muted">${tasks.length} 项固定任务 · 目录版本 maintenance/1.0</span></div>
    ${q && !shown.length && V6_PROHIBITED.test(q) ? `<div class="v6-notice warn"><b>没有“${esc(q)}”相关任务。</b>产品不提供注册表性能/隐私调优、禁用或删除系统服务、系统精简、内存“释放”或重建全部应用注册数据库：这些动作风险高且效果无法验证（MNT-05）。</div>` : q && !shown.length ? `<div class="v6-notice">没有匹配的任务。换个说法描述问题，例如“开始菜单”“DNS”。</div>` : ''}
    <div class="v6-panel"><div class="v6-rows">${shown.map(t => `<label class="v6-row ${t.prot ? 'is-off' : ''}">${t.prot ? '<span class="v6-nobox" aria-hidden="true"></span>' : `<input type="checkbox" data-f="mnt-${t.id}" ${m.sel.has(t.id) ? 'checked' : ''} onchange="this.checked?V6.mnt.sel.add('${t.id}'):V6.mnt.sel.delete('${t.id}');render()">`}<span class="v6-row-main"><span class="v6-row-name">${t.name}${t.admin ? ' <span class="v6-pill warn">需管理员</span>' : ''}${t.prot ? ' <span class="v6-pill">用户保护</span>' : ''}</span><span class="v6-row-meta">解决：${t.problem} · 影响：${t.impact}</span><span class="v6-row-meta">${t.appl} · ${t.eta}</span></span><span class="v6-row-side"><button class="v6-link" onclick="event.preventDefault();v6MntProtect('${t.id}')">${t.prot ? '取消保护' : '加入保护'}</button></span></label>`).join('')}</div></div>
    ${v6Selbar(m.sel.size, [...m.sel].map(() => ({ size: 0 })), '开始维护', 'v6MntStart()', adminSel.length ? `${adminSel.length} 项需管理员：开始时一次性请求，拒绝则仅这些项 skipped` : '全部为用户级任务，不需要管理员')}`
    : v6MntReport();
  return heading('维护任务', '按具体问题选择固定任务；执行前判定适用性，逐项报告真实结果', '<button onclick="route(\'perf\')">性能诊断</button>') +
    `<div class="v6-split"><section>${body}</section><aside class="v6-stack"><div class="v6-panel"><h2>六种结果</h2><p class="v6-sub">每项任务只会是其中之一，并附原因。</p><div class="v6-rows">${['applied', 'unchanged', 'skipped', 'unavailable', 'attention', 'failed'].map(o => `<div style="padding:5px 0">${v6Out(o)}</div>`).join('')}</div></div>
    <div class="v6-panel"><h2>不提供</h2><p class="v6-sub">注册表“优化”、关闭服务、系统精简、内存释放、加速百分比。维护不承诺提速，也没有通用撤销（MNT-05/07）。</p></div></aside></div>`;
}
function v6MntProtect(id) { const t = v6MntTasks().find(x => x.id === id); t.prot = !t.prot; if (t.prot) { V6.mnt.sel.delete(id); t.out0 = t.out0 || t.out; t.out = 'skipped'; t.why0 = t.why0 || t.why; t.why = '用户保护：此任务已加入保护名单（PRT-06）'; } else { t.out = t.out0 || t.out; t.why = t.why0 || t.why; } toast(t.prot ? `已保护“${t.name}”，批量选择将跳过` : `已取消保护“${t.name}”`); render(); }
function v6MntStart() {
  const m = V6.mnt, admin = v6MntTasks().filter(t => m.sel.has(t.id) && t.admin);
  if (admin.length && !m.elevation) {
    modal(V6.platform === 'mac' ? '一次性管理员授权' : '一次性管理员授权（UAC）', `<p>以下 ${admin.length} 项需要管理员权限。开始时请求一次提权，启动短生命周期的工作进程，只接受本次计划内的固定任务 ID，运行结束即退出（MNT-04）。</p><ul class="v6-steps">${admin.map(t => `<li><span>${t.name}</span><span class="v6-small v6-muted">${t.id}</span></li>`).join('')}</ul><div class="v6-notice">拒绝时这些任务记为 skipped（未授权），其余用户级任务照常执行。</div>`,
      `<button onclick="closeDialog();V6.mnt.elevation='denied';v6MntRun()">拒绝（演示）</button><button class="primary" onclick="closeDialog();V6.mnt.elevation='granted';v6MntRun()">允许（演示）</button>`);
    return;
  }
  v6MntRun();
}
function v6MntRun() {
  const m = V6.mnt, tasks = v6MntTasks();
  m.order = [...tasks.filter(t => m.sel.has(t.id)), ...tasks.filter(t => t.prot)];
  m.results = {}; m.phase = 'running'; let i = 0;
  const tick = () => {
    if (m.phase !== 'running') return;
    if (i >= m.order.length) { m.phase = 'done'; const c = v6MntCounts(); state.history.unshift({ id: 'h' + Date.now(), title: `维护 · ${m.order.length} 项任务`, time: '刚刚', status: c.failed || c.attention ? '部分完成' : '已完成', amount: null, kind: 'maintenance' }); v6Live('维护完成'); if (state.page === 'optimize') render(); return; }
    const t = m.order[i];
    m.results[t.id] = t.admin && m.elevation === 'denied' ? { o: 'skipped', why: '未授权：管理员授权被拒绝' } : { o: t.out, why: t.why };
    i++; if (state.page === 'optimize') render();
    m.timer = setTimeout(tick, state.reduceMotion ? 30 : 380);
  };
  render(); tick();
}
function v6MntCounts() { const c = { applied: 0, unchanged: 0, skipped: 0, unavailable: 0, attention: 0, failed: 0 }; Object.values(V6.mnt.results).forEach(r => c[r.o]++); return c; }
function v6MntReport() {
  const m = V6.mnt, c = v6MntCounts(), done = m.phase === 'done';
  return `<div class="v6-panel" aria-live="polite"><div class="v6-panel-head"><div><h2>${done ? (c.failed || c.attention ? '维护未全部完成' : '维护完成') : '正在执行维护'}</h2><p>提权：${m.elevation === 'granted' ? '已授权一次，工作进程已退出' : m.elevation === 'denied' ? '已拒绝' : '不需要'} · 记录已写入活动</p></div>${done ? '' : '<span class="v6-pill">进行中</span>'}</div>
  ${done ? `<ul class="v6-outcount">${Object.entries(c).map(([o, n]) => `<li>${v6Out(o)}<b>${n}</b></li>`).join('')}</ul>${c.failed || c.attention ? `<div class="v6-notice warn">${c.failed} 项失败、${c.attention} 项需要关注，因此不显示“全部完成”。</div>` : ''}` : '<div class="v6-indet" aria-hidden="true"></div>'}
  <ul class="v6-steps">${m.order.map(t => { const r = m.results[t.id]; return `<li><span>${t.name}</span>${r ? v6Out(r.o) : v6Out('pending')}${r ? `<small>${r.why}</small>` : ''}</li>`; }).join('')}</ul>
  ${done ? `<div class="v6-actions" style="margin-top:12px"><button onclick="V6.mnt.phase='select';V6.mnt.elevation=null;render()">返回任务列表</button><button onclick="route('activity')">在活动中查看</button>${m.results.broken ? '<button onclick="route(\'startup\')">打开启动项</button>' : ''}</div>` : ''}</div>`;
}

/* ---------- 优化 › 性能诊断 ---------- */
function v6PerfPage() {
  const rows = [
    ['持续高 CPU', 'updater_x64.exe 占用 22% CPU，已持续 14 分钟', '所属应用：未识别 · 未签名', `<button onclick="route('status');V6.st.q='updater';setTimeout(render)">在状态页查看</button>`],
    ['内存压力', '已用 12.9 / 16 GiB，交换 3.1 GiB，压力高', '最大占用：vmmem（WSL）2.4 GiB、Docker 1.8 GiB', `<button onclick="route('status')">查看内存</button>`],
    ['闲置虚拟机', 'WSL 发行版 Ubuntu 运行 6 小时无活动', '可在终端运行 wsl --shutdown 释放其内存（外部操作说明）', `<button onclick="toast('演示：复制命令 wsl --shutdown')">复制命令</button>`],
    ['未推出的磁盘镜像', 'Win11_23H2.iso 挂载为 F:', '推出后可在安装包页处理该镜像', `<button onclick="toast('演示：资源管理器 › F: › 右键推出')">推出指引</button>`]
  ];
  return heading('性能诊断', '只读：报告原因并给出可选入口；诊断本身不结束进程，也不更改设置', '<button onclick="toast(\'演示：已重新采样\')">重新诊断</button>') +
    `<div class="v6-panel"><div class="v6-rows">${rows.map(([k, a, b, act]) => `<div class="v6-row"><span class="v6-nobox" aria-hidden="true" style="opacity:0"></span><div class="v6-row-main"><span class="v6-row-name">${k}</span><span>${a}</span><span class="v6-row-meta">${b}</span></div><div class="v6-row-side">${act}</div></div>`).join('')}</div></div><div class="v6-notice">采样于 10:31:05（5 秒窗口）。这些发现与清理候选无关，不会被描述为“需要清理”。</div>`;
}

/* ---------- 分析 › 空间洞察 ---------- */
const V6_DOWNLOADS = [
  { id: 'd-mov', name: '素材视频-旧版.mov', size: 2150, mtime: '2026-03-02' },
  { id: 'd-rec', name: '会议录屏-0412.mp4', size: 380, mtime: '2026-04-12' },
  { id: 'd-zip', name: 'dataset-sample.zip', size: 210, mtime: '2026-05-20' },
  { id: 'd-shots', name: '截图合集', size: 120, mtime: '2026-06-01', folder: true },
  { id: 'd-setup', name: 'VSCodeUserSetup-1.82.exe', size: 96, mtime: '2026-02-11', installer: true },
  { id: 'd-pdf', name: '发票扫描.pdf', size: 2, mtime: '2026-01-08' }
];
function v6Insights() {
  const mac = V6.platform === 'mac';
  return [
    { id: 'downloads', kind: '旧下载 · 修改时间早于 90 天', name: 'Downloads 中 6 项', size: V6_DOWNLOADS.reduce((n, d) => n + d.size, 0), t: '今天 10:24', k: 'review', act: 'v6InsDownloads()' },
    mac ? { id: 'snap', kind: '本地快照', name: 'Time Machine 本地快照 4 个', size: null, unknownText: '4 个 · 大小不可得', t: '今天 10:24', k: 'guide' } : { id: 'hiber', kind: '休眠文件', name: 'hiberfil.sys', size: 6550, t: '今天 10:24', k: 'guide' },
    mac ? { id: 'vm', kind: '虚拟机与容器', name: 'Docker.raw', size: 18640, t: '今天 10:24', k: 'guide' } : { id: 'wsl', kind: 'WSL 虚拟磁盘', name: 'Ubuntu · ext4.vhdx', size: 18640, t: '今天 10:24', k: 'guide' },
    mac ? { id: 'backup', kind: '设备备份', name: 'iPhone 备份', size: 23240, t: '昨天 18:02', k: 'guide' } : { id: 'page', kind: '分页文件', name: 'pagefile.sys', size: 12288, t: '今天 10:24', k: 'guide' },
    mac ? { id: 'sysdata', kind: '系统数据', name: '系统数据类别', size: null, unknownText: '未知 · 系统未提供分项', t: '今天 10:24', k: 'guide' } : { id: 'winsxs', kind: '组件存储', name: 'WinSxS', size: null, unknownText: '未知 · 硬链接共享', t: '今天 10:24', k: 'guide' },
    mac ? null : { id: 'restore', kind: '系统还原点', name: '还原点 3 个', size: null, unknownText: '3 个 · 大小不可得', t: '今天 10:24', k: 'guide' },
    { id: 'models', kind: '模型库', name: 'Models', size: 40960, t: '今天 10:24', k: 'locate', act: "state.diskPath=[tree,findDisk('models')];state.diskSelected=null;route('disk')" },
    { id: 'devtools', kind: '开发工具大目录', name: '.gradle / .m2 / .cargo', size: 6810, t: '昨天 18:02', k: 'locate', act: "route('clean')" }
  ].filter(Boolean);
}
const V6_INS_TEXT = {
  hiber: ['保存休眠与快速启动时的内存内容。', '大小随物理内存与休眠类型变化。', '在“电源选项”中关闭休眠，或以管理员运行 powercfg /h /type reduced 缩小。直接删除会被系统重建。', true],
  page: ['虚拟内存的后备文件。', '由系统按内存压力自动管理。', '在“高级系统设置 › 性能 › 虚拟内存”中调整。不建议关闭。', true],
  wsl: ['包含 Linux 发行版的全部数据。', '发行版内文件增长后不会自动收缩。', '先在发行版内清理，再 wsl --shutdown 并用 Optimize-VHD 压缩。直接删除会丢失整个发行版。', true],
  winsxs: ['Windows 组件存储，大量文件为硬链接共享。', '逻辑大小之和远大于实际可回收量，因此显示为未知。', '使用系统自带的磁盘清理或 DISM /AnalyzeComponentStore 查看；本产品不执行组件清理。', true],
  restore: ['系统还原点的卷影副本。', '平台只列出数量，无法给出独占大小，不估算为 0。', '在“系统属性 › 系统保护”中管理。', true],
  snap: ['Time Machine 在本机保存的快照。', '平台只列出数量，独占大小不可得，不估算为 0 或逻辑和。', '在 Time Machine 设置中管理；系统会在空间紧张时自动清除。', false],
  vm: ['Docker Desktop 的虚拟磁盘。', '镜像与卷增长后不会自动收缩。', '在 Docker Desktop 中清理镜像与卷，或调整磁盘上限。', false],
  backup: ['设备的本地备份。', '每台设备一份，随设备数据增长。', '在访达的设备页面中管理备份。', false],
  sysdata: ['macOS 汇总的“系统数据”。', '系统未提供分项，显示为未知。', '在 系统设置 › 通用 › 储存空间 查看。', false]
};
function v6InsightsPage() {
  const cards = v6Insights();
  return heading('空间洞察', '解释体积大、看起来像垃圾却不该随手删的对象；每张卡显示已知大小或未知，以及最后观测时间', '<button onclick="route(\'disk\')">打开磁盘分析</button>') +
    `<div class="v6-insights">${cards.map(c => `<button class="v6-insight" data-k="${c.k}" onclick="${c.act || `v6InsDetail('${c.id}')`}"><span class="v6-ins-kind">${c.kind}</span><span class="v6-ins-name">${c.name}</span><span class="v6-ins-size ${c.size === null ? 'unknown' : ''}">${c.size === null ? c.unknownText : size(c.size)}</span><span class="v6-ins-time">观测于 ${c.t}</span></button>`).join('')}</div>
    <p class="v6-small v6-muted" style="margin-top:10px">左侧色条：橙色为审阅入口，绿色为在磁盘分析中定位，灰色为只读说明。</p>
    ${V6.ins.showDownloads ? v6DownloadsPanel() : ''}`;
}
function v6InsDetail(id) {
  const c = v6Insights().find(x => x.id === id), t = V6_INS_TEXT[id];
  drawer(c.name, `<span class="v6-pill">${c.kind}</span><dl class="v6-facts" style="margin-top:14px"><dt>大小</dt><dd>${c.size === null ? c.unknownText : size(c.size)}</dd><dt>观测</dt><dd>${c.t}</dd><dt>用途</dt><dd>${t[0]}</dd><dt>大小因素</dt><dd>${t[1]}</dd><dt>处理途径</dt><dd>${t[2]}</dd></dl><div class="v6-notice warn">只读说明：产品不修改系统设置、不删除快照、不缩减系统文件${t[3] ? '；上述系统工具步骤需要管理员权限' : ''}（DIN-02）。没有“移到${v6Bin()}”入口。</div>`, `<button class="primary" onclick="closeDrawer();toast('演示：打开对应的系统设置页面')">打开系统设置</button>`);
}
function v6InsDownloads() { V6.ins.showDownloads = true; render(); setTimeout(() => document.getElementById('v6Downloads')?.scrollIntoView({ block: 'start', behavior: state.reduceMotion ? 'auto' : 'smooth' }), 30); }
function v6DownloadsPanel() {
  const list = [...V6_DOWNLOADS].sort((a, b) => b.size - a.size), sel = list.filter(d => V6.ins.sel.has(d.id));
  return `<section class="v6-panel" id="v6Downloads" style="margin-top:22px"><div class="v6-panel-head"><div><h2>旧下载</h2><p>阈值：修改时间早于 90 天（规则 insights.old-downloads v1）。修改时间不等于最后使用时间。按大小排序，没有预选项。</p></div><button class="v6-link" onclick="V6.ins.showDownloads=false;render()">收起</button></div>
  <div class="v6-rows">${list.map(d => `<label class="v6-row"><input type="checkbox" data-f="dl-${d.id}" ${V6.ins.sel.has(d.id) ? 'checked' : ''} onchange="this.checked?V6.ins.sel.add('${d.id}'):V6.ins.sel.delete('${d.id}');render()"><span class="v6-row-main"><span class="v6-row-name">${d.name}${d.installer ? ' <span class="v6-pill">安装包</span>' : ''}${d.folder ? ' <span class="v6-pill">文件夹</span>' : ''}</span><span class="v6-row-meta">修改于 ${d.mtime}${d.installer ? ' · 也可在 清理 › 安装包 中按来源处理' : ''}</span></span><span class="v6-row-side"><span class="num">${size(d.size)}</span></span></label>`).join('')}</div>
  <p class="v6-small v6-muted">未列入：sync-conflict.tmp（时间戳不可读，不满足年龄条件，计入“未判定”）。</p>
  ${v6Selbar(sel.length, sel, `移到${v6Bin()}…`, `v6TrashPlan([...V6.ins.sel],'downloads')`, '走手选回收站流程，不进入默认清理候选')}</section>`;
}

/* ---------- 分析 › 回收站 ---------- */
function v6TrashPage() {
  const sel = V6.trash.filter(t => V6.tsel.has(t.id)), bin = v6Bin();
  return heading(bin, `只对已在${bin}中的对象提供永久删除；放回与清空整个${bin}各有入口`, `<button onclick="v6NoTrashDemo()">卷不支持${bin}时</button><button onclick="route('clean')">清空整个${bin}</button>`) +
    `<div class="v6-split"><section><div class="v6-panel">${V6.trash.length ? `<div class="v6-rows">${V6.trash.map(t => `<label class="v6-row"><input type="checkbox" data-f="tb-${t.id}" ${V6.tsel.has(t.id) ? 'checked' : ''} onchange="this.checked?V6.tsel.add('${t.id}'):V6.tsel.delete('${t.id}');render()"><span class="v6-row-main"><span class="v6-row-name">${esc(t.name)}</span><span class="v6-row-meta">删除于 ${t.when}</span><span class="v6-path">原位置 ${esc(t.from)}</span></span><span class="v6-row-side"><span class="num">${size(t.size)}</span><button class="v6-link" onclick="event.preventDefault();v6TrashRestore('${t.id}')">放回</button></span></label>`).join('')}</div>` : `<div class="empty-state"><h3>${bin}是空的</h3><p>在磁盘分析中右键对象，可生成“移到${bin}”计划。</p></div>`}</div>
    ${v6Selbar(sel.length, sel, '永久删除所选…', 'v6BinDeletePlan()', '独立的 manual_selection 计划 · 动作 fs_delete · 目标仅限' + bin)}</section>
    <aside class="v6-stack"><div class="v6-panel"><h2>边界</h2><dl class="v6-facts"><dt>磁盘分析</dt><dd>只能移到${bin}，没有直接永久删除</dd><dt>${bin}内</dt><dd>可选对象永久删除，需输入 DELETE 与摘要确认</dd><dt>占用</dt><dd>显示占用进程，不自动结束</dd></dl></div></aside></div>`;
}
function v6TrashRestore(id) { const t = V6.trash.find(x => x.id === id); V6.trash = V6.trash.filter(x => x.id !== id); V6.tsel.delete(id); toast(`已放回 ${t.name}（演示：系统${v6Bin()}的放回功能）`); render(); }
function v6NoTrashDemo() { modal('无法创建计划', `<div class="v6-notice bad">所选对象位于 <b>E:（exFAT 外接卷）</b>，该卷不支持${v6Bin()}。计划创建失败（MTR-03）。</div><p class="v6-muted">不会提供“直接删除”替代按钮。可以把对象复制到其他位置后，在外接卷上用系统文件管理器自行处理。</p>`); }
function v6BinDeletePlan() {
  const items = V6.trash.filter(t => V6.tsel.has(t.id)), digest = v6Hash(items.map(i => i.id).join('|') + 'fs_delete'), code = digest.slice(0, 4);
  v6Pending = { items, digest };
  modal(`从${v6Bin()}永久删除`, `<div class="v6-statement" data-mode="delete"><p class="v6-statement-verb">永久删除 ${items.length} 项，不可恢复</p><dl class="v6-facts"><dt>对象</dt><dd>${items.map(i => esc(i.name)).join('、')}</dd><dt>已知大小</dt><dd>${size(v6Sum(items).known)}</dd><dt>计划种类</dt><dd>manual_selection · fs_delete · 目标全部位于${v6Bin()}</dd></dl></div>
  <div class="v6-seal"><p>摘要</p><p class="v6-digest"><b>${code}</b>${digest.slice(4, 24)}…</p><label>输入 DELETE<input id="v6Del1" autocomplete="off" spellcheck="false" oninput="v6SealCheck('${code}')" aria-describedby="v6SealHelp"></label><label>输入摘要前 4 位<input id="v6Del2" autocomplete="off" spellcheck="false" oninput="v6SealCheck('${code}')"></label><p id="v6SealHelp" class="v6-muted">两项都匹配后才能执行。删除后无法从${v6Bin()}或恢复页找回。</p></div>`,
  `<button class="v6-danger" id="v6DelGo" disabled onclick="v6BinDelete()">永久删除</button>`);
}
function v6SealCheck(code) { const a = $('v6Del1'), b = $('v6Del2'), ok1 = a.value.trim() === 'DELETE', ok2 = b.value.trim().toLowerCase() === code; a.classList.toggle('match', ok1); b.classList.toggle('match', ok2); $('v6DelGo').disabled = !(ok1 && ok2); }
function v6BinDelete() {
  const p = v6Pending, s = v6Sum(p.items), before = V6.freeGiB; V6.freeGiB = +(V6.freeGiB + s.known / 1024).toFixed(1);
  V6.trash = V6.trash.filter(t => !p.items.includes(t)); V6.tsel.clear();
  state.history.unshift({ id: 'h' + Date.now(), title: `从${v6Bin()}永久删除 · ${p.items.length} 项`, time: '刚刚', status: '已完成', amount: s.known, kind: 'delete' });
  modal('已永久删除', v6Result({ count: `${p.items.length} 项`, logical: size(s.known), release: '至少 ' + size(Math.floor(s.known * .97)), releaseNote: '分配量部分未测', free: `${before} → ${V6.freeGiB} GiB`, freeNote: '卷空闲观测值' }), `<button class="primary" onclick="closeDialog();render()">完成</button>`);
  render();
}

/* ---------- 状态 › 系统状态 ---------- */
const V6_PROCS = [
  { id: 6120, name: 'updater_x64.exe', purpose: null, sig: 'unsigned', who: '未签名', app: '未识别', path: 'C:/Users/Lin/AppData/Local/Temp/upd/updater_x64.exe', parent: 'explorer.exe (4410)', cpu: 22.4, mem: 210, disk: 0.4, net: 1.2 },
  { id: 1042, name: 'Code.exe', purpose: '编辑器与扩展宿主', sig: 'valid', who: 'Microsoft Corporation', app: 'Visual Studio Code', path: 'C:/Users/Lin/AppData/Local/Programs/Microsoft VS Code/Code.exe', parent: 'explorer.exe (4410)', cpu: 6.1, mem: 680, disk: 0.2, net: 0 },
  { id: 2854, name: 'chrome.exe', purpose: '浏览器与网页渲染', sig: 'valid', who: 'Google LLC', app: 'Google Chrome', path: 'C:/Program Files/Google/Chrome/Application/chrome.exe', parent: 'explorer.exe (4410)', cpu: 4.2, mem: 1250, disk: 0.1, net: 0.6 },
  { id: 3421, name: 'com.docker.backend.exe', purpose: '容器运行环境后台', sig: 'valid', who: 'Docker Inc', app: 'Docker Desktop', path: 'C:/Program Files/Docker/Docker/resources/com.docker.backend.exe', parent: 'Docker Desktop.exe (3390)', cpu: 3.1, mem: 1860, disk: 1.8, net: 0.1 },
  { id: 5120, name: 'vmmem', purpose: 'WSL / 虚拟机内存', sig: 'valid', who: 'Microsoft Windows', app: 'Windows 子系统', path: '系统虚拟进程', parent: '系统', cpu: 2.0, mem: 2450, disk: 0.5, net: 0, prot: '系统托管：请用 wsl --shutdown 关闭' },
  { id: 3008, name: 'MsMpEng.exe', purpose: '防病毒实时保护', sig: 'valid', who: 'Microsoft Windows', app: 'Microsoft Defender', path: 'C:/ProgramData/Microsoft/Windows Defender/Platform/MsMpEng.exe', parent: 'services.exe (912)', cpu: 1.4, mem: 310, disk: 0.9, net: 0, prot: '安全软件，不可结束' },
  { id: 920, name: 'lsass.exe', purpose: '本地安全认证', sig: 'valid', who: 'Microsoft Windows', app: 'Windows', path: 'C:/Windows/System32/lsass.exe', parent: 'wininit.exe (800)', cpu: 0.3, mem: 34, disk: 0, net: 0, prot: '系统关键进程，不可结束' },
  { id: 781, name: 'CleanManager.exe', purpose: '本工具', sig: 'valid', who: 'CleanManager 演示签名', app: 'CleanManager', path: 'C:/Program Files/CleanManager/CleanManager.exe', parent: 'explorer.exe (4410)', cpu: 0.8, mem: 96, disk: 0, net: 0, prot: '本工具自身，不可结束' }
];
const V6_W = [['cpu', 'CPU', 30, 90], ['mem', '内存', 25, 48], ['disk', '磁盘', 20, 72], ['temp', '温度', 15, null], ['io', 'I/O', 10, 95]];
function v6StatusPage() {
  const mac = V6.platform === 'mac';
  const metrics = [
    ['CPU', 'available', '34', '%', '8 核 · 负载 2.7', 1],
    ['内存', 'available', '12.9', '/ 16 GiB', '压力高 · 交换 3.1 GiB · 已压缩 1.2 GiB', 2],
    ['GPU', 'available', '12', '%', mac ? 'Apple GPU' : '3D 引擎计数器', 3],
    ['磁盘 C:', 'available', '41.2', 'GiB 可用', '读 24 MiB/s · 写 3 MiB/s', 4],
    ['磁盘健康', mac ? 'available' : 'permission_required', mac ? '已验证' : null, '', mac ? 'SMART 状态正常' : '读取存储可靠性计数器需要管理员', 0],
    ['网络 Wi-Fi', 'available', '1.8', 'MiB/s ↓', '↑ 0.2 MiB/s · 以太网未连接', 5],
    ['电池', 'available', '86', '%', '健康 92% · 循环 318 次', 6],
    ['温度', mac ? 'available' : 'unavailable', mac ? '58' : null, mac ? '°C' : '', mac ? 'CPU 封装' : '此机型未通过公开接口提供温度读数', 0],
    ['风扇', 'unavailable', null, '', mac ? '此机型无风扇' : '平台不提供风扇读数', 0]
  ];
  const w = mac ? V6_W.map(x => x[0] === 'temp' ? [...x.slice(0, 3), 84] : x) : V6_W, known = w.filter(x => x[3] !== null), sumW = known.reduce((n, x) => n + x[2], 0);
  const score = Math.round(known.reduce((n, x) => n + x[2] * x[3], 0) / sumW);
  let procs = V6_PROCS.filter(p => !V6.st.ended.has(p.id) && (p.name + (p.purpose || '') + p.who).toLowerCase().includes(V6.st.q.toLowerCase()));
  procs.sort((a, b) => Number(V6.st.pinned.has(b.id)) - Number(V6.st.pinned.has(a.id)) || b[V6.st.sort] - a[V6.st.sort]);
  return heading('系统状态', '观察与诊断；健康分与清理候选无关，不宣称加速', '<button onclick="route(\'resident\')">常驻工具</button>') +
    `<div class="v6-diag" role="status"><span><b>诊断：</b>updater_x64.exe 持续占用 22% CPU 已 14 分钟，用途未识别。</span><button class="v6-link" onclick="v6ProcDetail(6120)">查看进程</button></div>
    <div class="v6-grid3 v6-metrics">${metrics.map(([n, a, v, u, note, seed]) => `<div class="v6-metric"><div class="v6-metric-head"><span>${n}</span><span class="v6-avail" data-a="${a}">${{ available: '可用', unavailable: '不可用', permission_required: '需要权限' }[a]}</span></div><div class="v6-metric-value ${v === null ? 'unknown' : ''}">${v === null ? '未知' : v}${v !== null && u ? `<small>${u}</small>` : ''}</div><p>${note}</p>${seed ? v6Spark(seed) : ''}</div>`).join('')}</div>
    <section class="v6-panel" style="margin-top:18px"><div class="v6-panel-head"><div><h2>健康分</h2><p>公开权重：CPU 30 · 内存 25 · 磁盘 20 · 温度 15 · I/O 10。阈值为本产品可调设计值。</p></div><button class="v6-link" onclick="v6ScoreInfo()">计算方法</button></div>
    <div class="v6-health"><div class="v6-score num">${score}<small>满分 100 · ${sumW < 100 ? `按已知 ${sumW}% 权重计算` : '全部维度已知'}</small></div><div><div class="v6-weights" role="img" aria-label="各维度权重与得分">${w.map(([k, n, wt, s]) => `<span class="v6-wseg ${s === null ? 'unknown' : s < 60 ? 'low' : ''}" style="flex:${wt}" title="${n} 权重 ${wt}，得分 ${s === null ? '未知' : s}">${s === null ? '' : `<i style="height:${s}%"></i>`}</span>`).join('')}</div><div class="v6-wlegend">${w.map(([k, n, wt, s]) => `<span style="flex:${wt}"><b>${n} ${s === null ? '未知' : s}</b>权重 ${wt}${s !== null && s < 60 ? ' · 拉低分数' : ''}</span>`).join('')}</div></div></div>
    <p class="v6-small v6-muted" style="margin:12px 0 0">内存维度当前值：压力高（已用 81%，交换 3.1 GiB）。${sumW < 100 ? '温度不可得时不计为 0 分，按其余维度重新归一。' : ''}</p></section>
    <section class="v6-panel" style="margin-top:18px"><div class="v6-panel-head"><div><h2>进程</h2><p>本地事实：位置、签名、父进程与已知用途；只提供“结束进程”，且逐次确认</p></div></div>
    <div class="v6-toolbar"><label class="v6-search">⌕ <input aria-label="搜索进程" data-f="st-q" placeholder="名称、用途或发布者" value="${esc(V6.st.q)}" oninput="V6.st.q=this.value;render()"></label><div class="v6-chips" role="group" aria-label="排序">${[['cpu', 'CPU'], ['mem', '内存'], ['disk', '磁盘'], ['net', '网络']].map(([k, n]) => `<button aria-pressed="${V6.st.sort === k}" data-f="st-sort-${k}" onclick="V6.st.sort='${k}';render()">${n}</button>`).join('')}</div></div>
    <div class="v6-proc"><div class="v6-proc-row head"><span>进程 / 用途</span><span class="v6-proc-sig">签名 / 发布者</span><span class="num">CPU</span><span class="num v6-col-mem">内存</span><span class="num v6-col-io">磁盘</span><span class="num v6-col-io">网络</span><span class="v6-proc-act">操作</span></div>
    ${procs.map(p => `<div class="v6-proc-row"><span class="v6-proc-name"><b>${V6.st.pinned.has(p.id) ? '⌖ ' : ''}${p.name}</b><small>${p.purpose ? p.purpose : '<span class="v6-pill warn">用途未识别</span>'} · PID ${p.id}${p.prot ? ' · <span class="v6-pill">受保护</span>' : ''}</small></span><span class="v6-proc-sig"><span>${p.sig === 'valid' ? '已签名' : '<span class="v6-pill bad">未签名</span>'}</span><small>${p.who}</small></span><span class="num">${p.cpu.toFixed(1)}%</span><span class="num v6-col-mem">${size(p.mem)}</span><span class="num v6-col-io">${p.disk} MiB/s</span><span class="num v6-col-io">${p.net} MiB/s</span><span class="v6-proc-act"><button data-f="st-pin-${p.id}" onclick="V6.st.pinned.has(${p.id})?V6.st.pinned.delete(${p.id}):V6.st.pinned.add(${p.id});render()" aria-label="${V6.st.pinned.has(p.id) ? '取消置顶' : '置顶'} ${p.name}">${V6.st.pinned.has(p.id) ? '取消置顶' : '置顶'}</button><button onclick="v6ProcDetail(${p.id})" aria-label="${p.name} 详情">详情</button></span></div>`).join('')}</div></section>
    <div class="v6-grid2" style="margin-top:18px"><section class="v6-panel"><h2>持续高占用提醒</h2><p class="v6-sub">默认关闭；仅在常驻组件运行时生效，阈值 80% · 窗口 5 分钟 · 冷却 30 分钟（STS-05）。</p><div class="v6-actions"><label class="toggle"><input type="checkbox" aria-label="持续高 CPU 提醒" data-f="st-alert" ${V6.res.cpuAlert ? 'checked' : ''} ${V6.res.tray ? '' : 'disabled'} onchange="V6.res.cpuAlert=this.checked;render()"><span class="switch"></span></label><span class="v6-small">${V6.res.tray ? (V6.res.cpuAlert ? '已开启' : '已关闭') : '需要先开启常驻组件'}</span>${V6.res.tray ? '' : '<button class="v6-link" onclick="route(\'resident\')">前往开启</button>'}</div></section>
    <section class="v6-panel"><h2>命令行状态流</h2><p class="v6-sub">与界面同一采样；不可用指标为 null 并带原因（STS-06）。</p><pre class="v6-code">cm status --watch 5s --json\n{"sampled_at":"2026-10-15T10:31:05Z","complete":false,\n "cpu":{"availability":"available","total":0.34},\n "temperature":{"availability":"unavailable","value":null,\n  "reason":"sensor_not_exposed"}}</pre></section></div>`;
}
function v6ScoreInfo() { modal('健康分计算方法', `<p>健康分 = Σ(维度权重 × 维度得分) ÷ Σ(已知维度权重)。维度得分按阈值映射到 0–100。</p><dl class="v6-facts">${V6_W.map(([k, n, w]) => `<dt>${n}</dt><dd>权重 ${w}</dd>`).join('')}</dl><div class="v6-notice">分数不随清理候选数量变化，也不用“清理后提升”作宣传（STS-02）。不可得的维度不计为 0。</div>`); }
function v6ProcDetail(id) {
  const p = V6_PROCS.find(x => x.id === id);
  drawer(p.name, `${p.purpose ? `<p class="v6-muted">${p.purpose}</p>` : '<div class="v6-notice warn">用途未识别：本地知识表中没有该名称、签名与路径的组合。</div>'}<dl class="v6-facts"><dt>PID</dt><dd>${p.id}</dd><dt>位置</dt><dd class="v6-path">${p.path}</dd><dt>签名</dt><dd>${p.sig === 'valid' ? '有效 · ' + p.who : '未签名'}</dd><dt>父进程</dt><dd>${p.parent}</dd><dt>所属应用</dt><dd>${p.app}</dd><dt>CPU / 内存</dt><dd>${p.cpu}% · ${size(p.mem)}</dd></dl>${p.prot ? `<div class="v6-notice">${p.prot}</div>` : ''}`,
    (p.purpose ? '' : `<button class="primary" onclick="closeDrawer();route('diagnosis')">AI 诊断此进程</button>`) + `<button ${p.prot ? 'disabled' : ''} class="${p.prot ? '' : 'v6-danger'}" onclick="v6ProcEnd(${p.id})">${p.prot ? '不可结束' : '结束进程…'}</button>`);
}
function v6ProcEnd(id) { const p = V6_PROCS.find(x => x.id === id); closeDrawer(); modal('结束进程？', `<p>结束 <b>${p.name}</b>（PID ${p.id}）。未保存的数据可能丢失。每次结束都需要单独确认。</p>`, `<button class="v6-danger" onclick="closeDialog();V6.st.ended.add(${id});toast('已结束 ${p.name}（演示）');render()">结束进程</button>`); }

/* ---------- 状态 › 常驻工具 ---------- */
let v6AwakeTimer = null;
function v6ResidentPage() {
  const r = V6.res, mac = V6.platform === 'mac', area = mac ? '菜单栏' : '托盘';
  const remain = r.awakeUntil ? Math.max(0, Math.round((r.awakeUntil - Date.now()) / 1000)) : 0;
  const hw = [
    ['风扇控制', mac ? '当前机型 MacBookPro18,3 不在已验证列表：只显示读数' : '仅 macOS 已验证机型；Windows 首批不提供风扇写入（缺统一接口）', 'RES-05'],
    ['电池充电上限', mac ? '需要安装特权 helper；当前机型未验证' : '仅 macOS；Windows 不提供电池写入', 'RES-05'],
    ['网络低延迟模式', mac ? '需要安装特权 helper' : '仅 macOS', 'RES-05'],
    ['Touch ID 用于管理员认证', mac ? '需修改 sudo 配置：先备份、验证语法，失败回滚' : 'Windows 不适用', 'RES-06']
  ];
  return heading('常驻工具', `可选的${area}组件与日常小工具；默认关闭，关闭后不再后台采样`, '') +
    `<div class="v6-split"><section class="v6-stack">
    <div class="v6-panel"><div class="v6-panel-head"><div><h2>${area}组件</h2><p>显示所选指标并可展开小面板；不执行清理、卸载或维护写动作（RES-01）。</p></div><label class="toggle"><input type="checkbox" aria-label="开启${area}组件" data-f="res-tray" ${r.tray ? 'checked' : ''} onchange="v6TrayToggle(this.checked)"><span class="switch"></span></label></div>
    <dl class="v6-facts"><dt>安装状态</dt><dd>${r.installed ? '已安装 · 0.6.0（演示）' : '未安装'}</dd><dt>启动方式</dt><dd>${r.tray ? '登录时启动（已注册登录项）' : '未注册登录项'}</dd><dt>后台采样</dt><dd>${r.tray ? '运行中 · 每 2 秒' : '已停止'}</dd></dl>
    ${r.tray ? `<div class="v6-chips" style="margin-top:12px" role="group" aria-label="显示的指标">${[['cpu', 'CPU'], ['mem', '内存'], ['net', '网络'], ['bat', '电池']].map(([k, n]) => `<button aria-pressed="${r.metrics.has(k)}" data-f="res-m-${k}" onclick="V6.res.metrics.has('${k}')?V6.res.metrics.delete('${k}'):V6.res.metrics.add('${k}');render()">${n}</button>`).join('')}</div>` : ''}
    <div class="v6-actions" style="margin-top:12px">${r.installed ? `<button onclick="v6TrayRemove()">移除${area}组件</button>` : ''}<button onclick="v6TrayDialog()">预览面板</button></div></div>
    <div class="v6-panel"><h2>日常工具</h2>
    <div class="v6-util"><div><h3>保持亮屏 ${r.awakeUntil ? '<span class="v6-pill good">进行中</span>' : ''}</h3><p>${r.awakeUntil ? `剩余 <span id="v6AwakeLeft" class="num">${Math.floor(remain / 60)}:${String(remain % 60).padStart(2, '0')}</span> · 结束或退出时恢复系统电源设置` : '使用系统电源断言；可设时长，退出即释放（RES-02）'}</p></div><div class="v6-actions">${r.awakeUntil ? '<button onclick="v6AwakeStop()">结束</button>' : `<select aria-label="保持亮屏时长" data-f="res-awake-min" onchange="V6.res.awakeMin=+this.value">${[[30, '30 分钟'], [60, '1 小时'], [120, '2 小时']].map(([v, n]) => `<option value="${v}" ${r.awakeMin === v ? 'selected' : ''}>${n}</option>`).join('')}</select><button onclick="v6AwakeStart()">开始</button>`}</div></div>
    <div class="v6-util"><div><h3>清洁屏幕</h3><p>临时遮蔽显示并屏蔽键盘输入；按住 Esc 2 秒退出，30 秒后自动退出</p></div><div class="v6-actions"><button onclick="v6CleanScreen()">开始清洁</button></div></div>
    <div class="v6-util"><div><h3>隐私提示</h3><p>仅本地显示正在使用摄像头或麦克风的应用，不记录或上传历史（RES-03）</p></div><div class="v6-actions"><button onclick="V6.res.mic=!V6.res.mic;render()">${r.mic ? '模拟：麦克风停止使用' : '模拟：麦克风开始使用'}</button></div>
      <p style="grid-column:1/-1">${r.mic ? '<span class="v6-pill warn">麦克风</span> Teams 正在使用' : '麦克风：没有应用在使用'} · ${r.cam ? '摄像头使用中' : '摄像头：没有应用在使用'}</p></div>
    <div class="v6-util"><div><h3>设备电量</h3><p>低电量提醒阈值 <select aria-label="低电量阈值" data-f="res-thresh" onchange="V6.res.thresh=+this.value;V6.res.alerted=false;render()">${[10, 20, 30].map(v => `<option value="${v}" ${r.thresh === v ? 'selected' : ''}>${v}%</option>`).join('')}</select>（RES-04）</p></div><div class="v6-actions"><label class="toggle"><input type="checkbox" aria-label="低电量提醒" data-f="res-batt" ${r.battAlert ? 'checked' : ''} onchange="V6.res.battAlert=this.checked;render()"><span class="switch"></span></label></div>
      <p style="grid-column:1/-1">本机 86% · 蓝牙键盘 Keys K3 <b>18%</b>${18 < r.thresh && r.battAlert ? (r.tray ? ' · 已提醒一次，电量恢复后重置' : ' · 低于阈值；提醒需常驻组件运行') : ''} · 蓝牙耳机 电量不可读（设备未通过系统公开）</p></div></div>
    <div class="v6-panel"><div class="v6-panel-head"><div><h2>特权硬件控制</h2><p>${mac ? '经签名、版本匹配、只接受固定命令集的 helper 执行；未验证硬件不可用' : '这些功能只在 macOS 提供；在 Windows 原位显示原因，不隐藏'}</p></div>${v6Status(mac ? 'spec' : 'na')}</div>
    ${hw.map(([n, why, req]) => `<div class="v6-util is-na"><div><h3>${n} ${v6Status('na')}</h3><p>${why} · ${req}</p></div><div class="v6-actions"><button disabled>${mac ? '需要 helper' : '不适用'}</button></div></div>`).join('')}</div></section>
    <aside class="v6-stack"><div class="v6-panel"><h2>${area}面板预览</h2>${r.tray ? v6TrayPanel() : `<p class="v6-sub">组件未开启。开启后在${mac ? '菜单栏' : '任务栏通知区'}显示所选指标。</p>`}</div>
    <div class="v6-panel"><h2>生命周期</h2><p class="v6-sub">设置 › 常驻组件与 helper 中同样显示安装状态、版本、启动方式与移除入口；卸载本产品时一并移除并恢复系统设置（RES-07）。</p><button onclick="V6.settingsTab='v6res';route('settings')">打开设置</button></div></aside></div>`;
}
function v6TrayPanel() {
  const r = V6.res, m = { cpu: ['CPU', '34%'], mem: ['内存', '12.9 / 16 GiB'], net: ['网络', '↓1.8 ↑0.2 MiB/s'], bat: ['电池', '86%'] };
  return `<div class="v6-tray" role="group" aria-label="面板预览">${[...r.metrics].map(k => `<div class="v6-tray-row"><span>${m[k][0]}</span><b class="num">${m[k][1]}</b></div>`).join('') || '<span class="v6-muted">未选择指标</span>'}${r.mic ? '<div class="v6-tray-row"><span>麦克风</span><b>Teams</b></div>' : ''}${r.awakeUntil ? '<div class="v6-tray-row"><span>保持亮屏</span><b>进行中</b></div>' : ''}<div class="v6-tray-foot">只读状态与提醒 · 点击打开主窗口</div></div>`;
}
function v6TrayToggle(on) { V6.res.tray = on; if (on) V6.res.installed = true; if (!on) { V6.res.cpuAlert = false; v6AwakeStop(true); } toast(on ? '已开启：注册登录项并开始采样' : '已关闭：图标移除，采样与提醒停止，登录项已移除'); render(); }
function v6TrayRemove() { V6.res.tray = false; V6.res.installed = false; V6.res.cpuAlert = false; toast('已移除常驻组件与登录项'); render(); }
function v6TrayDialog() { const r = V6.res, area = V6.platform === 'mac' ? '菜单栏' : '托盘'; modal(`${area}面板`, r.tray ? v6TrayPanel() : `<p class="v6-muted">${area}组件未开启。它默认关闭，需要你主动开启；关闭后不再后台采样。</p>`, `<button class="primary" onclick="closeDialog();route('resident')">${r.tray ? '常驻工具设置' : '前往开启'}</button>`); }
function v6AwakeStart() { V6.res.awakeUntil = Date.now() + V6.res.awakeMin * 60000; clearInterval(v6AwakeTimer); v6AwakeTimer = setInterval(() => { const el = $('v6AwakeLeft'); if (!V6.res.awakeUntil) return clearInterval(v6AwakeTimer); const s = Math.max(0, Math.round((V6.res.awakeUntil - Date.now()) / 1000)); if (el) el.textContent = Math.floor(s / 60) + ':' + String(s % 60).padStart(2, '0'); if (!s) v6AwakeStop(); }, 1000); toast('保持亮屏已开始'); render(); }
function v6AwakeStop(silent) { if (!V6.res.awakeUntil) return; V6.res.awakeUntil = null; clearInterval(v6AwakeTimer); if (!silent) { toast('已结束，系统电源设置已恢复'); render(); } }
function v6CleanScreen() {
  const prev = document.activeElement, el = document.createElement('div');
  el.className = 'v6-cleanscreen'; el.tabIndex = -1; el.setAttribute('role', 'dialog'); el.setAttribute('aria-label', '清洁屏幕');
  let left = 30, holdStart = null, raf;
  el.innerHTML = `<div><b>清洁屏幕中</b><p>键盘输入已屏蔽。按住 Esc 2 秒退出。</p><p><span id="v6CsLeft">${left}</span> 秒后自动退出</p><div class="v6-hold"><i id="v6CsHold"></i></div></div>`;
  document.body.appendChild(el); el.focus();
  const exit = () => { clearInterval(iv); cancelAnimationFrame(raf); document.removeEventListener('keydown', kd, true); document.removeEventListener('keyup', ku, true); el.remove(); prev?.focus?.(); v6Live('已退出清洁屏幕'); };
  const iv = setInterval(() => { left--; const s = el.querySelector('#v6CsLeft'); if (s) s.textContent = left; if (left <= 0) exit(); }, 1000);
  const step = () => { if (holdStart === null) return; const p = Math.min(1, (performance.now() - holdStart) / 2000); el.querySelector('#v6CsHold').style.width = p * 100 + '%'; if (p >= 1) return exit(); raf = requestAnimationFrame(step); };
  const kd = e => { e.preventDefault(); e.stopPropagation(); if (e.key === 'Escape' && holdStart === null) { holdStart = performance.now(); raf = requestAnimationFrame(step); } };
  const ku = e => { e.preventDefault(); e.stopPropagation(); if (e.key === 'Escape') { holdStart = null; cancelAnimationFrame(raf); el.querySelector('#v6CsHold').style.width = '0'; } };
  document.addEventListener('keydown', kd, true); document.addEventListener('keyup', ku, true);
  el.addEventListener('dblclick', exit);
}

/* ---------- settings: resident lifecycle tab ---------- */
function v6SettingsResident() {
  const r = V6.res, mac = V6.platform === 'mac';
  return `<h2>常驻组件与 helper</h2><p class="muted">两者都是可独立移除的组件（RES-07）。</p><dl class="v6-facts" style="margin-top:12px"><dt>${mac ? '菜单栏' : '托盘'}组件</dt><dd>${r.installed ? '已安装 · 0.6.0 · ' + (r.tray ? '登录时启动' : '已停止') : '未安装'}</dd><dt>特权 helper</dt><dd>${mac ? '未安装 · 只接受风扇/电池/网络/维护分组的固定命令' : '不适用于 Windows；维护提权使用一次性工作进程，结束即退出'}</dd><dt>卸载本产品时</dt><dd>一并移除组件、helper 与登录项，并恢复其修改过的系统设置（如充电上限）</dd></dl><div class="feature-actions section-gap"><button onclick="route('resident')">打开常驻工具</button>${r.installed ? '<button onclick="v6TrayRemove()">移除常驻组件</button>' : ''}</div>`;
}

/* ---------- chrome: nav, rail, subnav, capline ---------- */
const v6Views = { purge: v6PurgePage, installers: v6InstallersPage, uninstall: v6UninstallPage, updates: v6UpdatesPage, startup: v6StartupPage, optimize: v6OptimizePage, perf: v6PerfPage, insights: v6InsightsPage, trashbin: v6TrashPage, status: v6StatusPage, resident: v6ResidentPage };
function v6Chrome(page) {
  const fam = v6Family(page) || (page === 'run' ? null : null);
  if (fam) V6.last[fam.id] = page;
  $('mainNav').innerHTML = V6_TOOLS.map((t, i) => `<button data-page="${V6.last[t.id] || t.tabs[0][0]}" class="${fam && fam.id === t.id ? 'active' : ''}" ${fam && fam.id === t.id ? 'aria-current="page"' : ''} title="${t.name} · Alt+${i + 1}" aria-keyshortcuts="Alt+${i + 1}">${t.name}<kbd>Alt ${i + 1}</kbd></button>`).join('');
  const extActive = page === 'run' ? 'plans' : ['coverage', 'rules'].includes(page) ? null : page === 'capabilities' ? 'settings' : page;
  $('v6Rail').innerHTML = '<div class="v6-rail-label">扩展</div>' + V6_EXT.map(([id, n, k, svg]) => `<button data-page="${id}" class="${extActive === id ? 'active' : ''}" ${extActive === id ? 'aria-current="page"' : ''} title="${n} · Alt+${k}" aria-keyshortcuts="Alt+${k}" aria-label="${n}"><svg viewBox="0 0 24 24" aria-hidden="true">${svg}</svg><span>${n}</span></button>`).join('');
  const main = $('main');
  main.querySelectorAll('.badge.concept').forEach(b => b.remove());
  if (page === 'apps') { const t = main.querySelector('.tabs'); if (t) t.remove(); }
  if (fam) main.insertAdjacentHTML('afterbegin', `<nav class="v6-subnav" aria-label="${fam.name}"><span class="v6-tool-name">${fam.name}</span>${fam.tabs.map(([id, n, cap]) => { const c = V6_CAPS[cap]; return `<button data-page="${id}" data-f="sub-${id}" class="${id === page ? 'active' : ''}" ${id === page ? 'aria-current="page"' : ''}>${n || v6Bin()}${v6Status(c.s)}${v6Gate(c.g)}</button>`; }).join('')}</nav>`);
  const key = v6CapKey(page), head = main.querySelector('.page-heading > div:first-child');
  if (head && key && !head.querySelector('.v6-capline')) head.insertAdjacentHTML('beforeend', v6Capline(key));
  if (page === 'settings') {
    const nav = document.querySelector('.settings-nav');
    if (nav) { nav.insertAdjacentHTML('beforeend', `<button class="${V6.settingsTab === 'v6res' ? 'active' : ''}" onclick="V6.settingsTab='v6res';render()">常驻组件与 helper</button>`); nav.querySelectorAll('button').forEach(b => { if (!b.textContent.includes('常驻组件')) b.addEventListener('click', () => { V6.settingsTab = null; }, { capture: true }); }); if (V6.settingsTab === 'v6res') { nav.querySelectorAll('button').forEach(b => b.classList.toggle('active', b.textContent.includes('常驻组件'))); const sec = document.querySelector('.settings-layout>section:last-child'); if (sec) sec.innerHTML = v6SettingsResident(); } }
  }
  main.querySelectorAll('*').forEach(el => { if (el.children.length === 0 && /扩展概念/.test(el.textContent)) el.textContent = el.textContent.replace(/\s*·?\s*扩展概念/g, ''); });
  const pb = $('platformButton'); if (pb) { pb.textContent = V6.platform === 'mac' ? 'Mac' : 'Win'; pb.setAttribute('aria-label', `预览平台：${V6.platform === 'mac' ? 'macOS' : 'Windows'}，点击切换`); }
}
const v6Legacy = { render };
render = function () {
  let page = location.hash.slice(1) || 'clean';
  if (page === 'maintenance') { location.replace('#optimize'); return; }
  if (page === 'home') { location.replace('#insights'); return; }
  const f = document.activeElement?.dataset?.f, sub = document.activeElement?.closest?.('.v6-subnav, #mainNav, #v6Rail') ? document.activeElement.dataset.page : null;
  if (v6Views[page]) {
    state.page = page; $('main').innerHTML = v6Views[page](); updateSelectionBar(); updateTaskChip();
    document.title = 'CleanManager · ' + pageNames[page];
  } else {
    if (page === 'apps') state.appTab = 'installed';
    v6Legacy.render();
  }
  v6Chrome(state.page);
  if (f) document.querySelector(`[data-f="${f}"]`)?.focus();
  else if (sub) document.querySelector(`.v6-subnav [data-page="${sub}"]`)?.focus() || document.querySelector(`#mainNav [data-page="${sub}"], #v6Rail [data-page="${sub}"]`)?.focus();
};
filterCommands = function (query) {
  const q = query.trim().toLowerCase(), aliases = { clean: '清理 缓存 目录 回收站', ai: 'AI 数据 会话 模型 工作树', purge: 'node_modules target 构建 产物 项目', installers: '安装包 msi iso dmg', apps: '应用 软件 清单', uninstall: '卸载 残留 注册表', updates: '更新 升级 winget', startup: '启动项 开机 登录项', optimize: '维护 DNS 图标 TRIM 优化', perf: '性能 诊断 卡顿 高占用', disk: '磁盘 分析 大文件 treemap', insights: '洞察 休眠 WSL 旧下载 快照', trashbin: '回收站 废纸篓 永久删除', status: '状态 CPU 内存 进程 健康分', resident: '托盘 菜单栏 亮屏 隐私 电量 风扇', plans: '计划 审阅 摘要', activity: '活动 记录 历史', recovery: '恢复 隔离', agent: 'Agent CLI Codex Claude', diagnosis: 'AI 诊断 能不能删', settings: '设置 保护 策略 能力' };
  const ids = [...V6_TOOLS.flatMap(t => t.tabs.map(x => x[0])), ...V6_EXT.map(x => x[0])];
  const list = ids.filter(id => (pageNames[id] + ' ' + (aliases[id] || '')).toLowerCase().includes(q));
  $('commandList').innerHTML = list.map(id => { const fam = v6Family(id); return `<button onclick="closeDialog();route('${id}')"><span>${fam ? fam.name + ' › ' : '扩展 › '}${id === 'trashbin' ? v6Bin() : esc(pageNames[id])}${q && aliases[id] ? `<small class="row-meta" style="display:block">${esc(aliases[id])}</small>` : ''}</span><span>↵</span></button>`; }).join('') || `<div class="empty-state">没有匹配功能${V6_PROHIBITED.test(q) ? '。产品不提供注册表优化、关闭服务或内存释放。' : ''}</div>`;
};
addEventListener('keydown', e => {
  if (!e.altKey || e.ctrlKey || e.metaKey) return;
  const n = '12345'.indexOf(e.key);
  if (n >= 0) { e.preventDefault(); const t = V6_TOOLS[n]; route(V6.last[t.id] || t.tabs[0][0]); return; }
  const ext = V6_EXT.find(x => x[2] === e.key); if (ext) { e.preventDefault(); route(ext[0]); }
});
$('trayButton').onclick = () => showTray();
$('platformButton').onclick = () => { V6.platform = V6.platform === 'mac' ? 'win' : 'mac'; state.system = V6.platform === 'mac' ? 'macOS' : 'Windows'; V6.mnt.sel = null; toast(`预览平台：${state.system}。名称与顺序不变，不适用的功能原位说明`); render(); };
document.querySelector('.brand').setAttribute('href', '#clean');
render();
