// DEVELOPMENT-ONLY fixture for P04–P10 in the browser preview (imported by mock.ts, which is
// itself only loaded behind `import.meta.env.DEV`). It mirrors the host's semantics: nothing
// is pre-selected, unknown is never 0, approval needs the review token and the typed digest
// prefix, a same-volume quarantine frees nothing, the same plan never runs twice.

import type { Backend } from './api';
import type {
  ApplyFinishedEvent,
  ApplyProgressEvent,
  AppItem,
  Authorization,
  Candidate,
  CandidateSetInfo,
  Counts,
  Group,
  HistoryItem,
  IpcError,
  Mode,
  Operation,
  OperationItem,
  PlanDetail,
  PlanItem,
  Policy,
  ProtectionItem,
  QuarantineItem,
} from './types';

type OpsBackend = Pick<
  Backend,
  | 'candidateSummary'
  | 'analyzeScan'
  | 'listCandidates'
  | 'createPlan'
  | 'planDetail'
  | 'planItems'
  | 'approvePlan'
  | 'revokePlan'
  | 'listPolicies'
  | 'setPolicyState'
  | 'createPolicyFromPlan'
  | 'startApply'
  | 'cancelApply'
  | 'activeApply'
  | 'operationDetail'
  | 'operationItems'
  | 'reconcileOperation'
  | 'listOperations'
  | 'listPlans'
  | 'listQuarantine'
  | 'createRecoveryPlan'
  | 'listProtection'
  | 'addProtection'
  | 'removeProtection'
  | 'listApps'
  | 'onApplyProgress'
  | 'onApplyFinished'
> & {
  /** Fixture hook: freeze a plan of any kind (catalog, manual trash, startup) for the review page. */
  __plan(kind: PlanDetail['kind'], mode: Mode | null, items: PlanItem[], extra?: Partial<PlanDetail>): string;
};

const MiB = 1024n * 1024n;
const GiB = 1024n * MiB;
const err = (code: string, reason: string, message: string): IpcError => ({ code, reason, message });
const delay = (ms: number) => new Promise((r) => setTimeout(r, ms));
const HOME = 'C:\\Users\\Lin';
const LOCAL = `${HOME}\\AppData\\Local`;
const iso = (offsetMin: number) => new Date(Date.now() + offsetMin * 60_000).toISOString().replace(/\.\d+Z$/, 'Z');

function hex(seed: string): string {
  // FNV-1a over the whole seed, once per output word (preview ids only; not a real digest).
  let out = '';
  for (let word = 0; out.length < 64; word++) {
    let h = (2166136261 ^ Math.imul(word + 1, 0x9e3779b1)) >>> 0;
    for (let i = 0; i < seed.length; i++) {
      h ^= seed.charCodeAt(i);
      h = Math.imul(h, 16777619) >>> 0;
    }
    h ^= h >>> 15;
    h = Math.imul(h, 0x2c1b3c6d) >>> 0;
    out += (h >>> 0).toString(16).padStart(8, '0');
  }
  return out.slice(0, 64);
}

function candidate(id: string, path: string, rule: string, group: Group, bytes: bigint, over: Partial<Candidate> = {}): Candidate {
  const parts = path.split('\\');
  const base: Candidate = {
    candidate_id: id,
    entry_id: String(100 + id.length),
    name: parts[parts.length - 1],
    path_display: path,
    kind: 'directory',
    rules: [rule],
    category: 'app_cache',
    group,
    state: group === 'pending' ? 'pending_evidence' : group === 'blocked' ? 'blocked' : 'allowed',
    tier: group === 'low' ? 'default_selected' : group === 'blocked' ? 'blocked' : 'review_only',
    not_selectable: group === 'pending' ? 'pending_evidence' : group === 'blocked' ? 'blocked' : null,
    reasons: [],
    protection: null,
    logical_known_bytes: bytes.toString(),
    unknown_size_count: 0,
    logical_exact_bytes: bytes.toString(),
    allowed_modes: group === 'low' ? ['trash', 'quarantine', 'delete'] : group === 'review' ? ['trash', 'quarantine'] : [],
    impact: '',
    probes: [],
    row_kind: 'file',
    registry_status: group === 'blocked' ? null : 'covered',
    installed_version: null,
    rebuild_class: 'local_rebuildable',
    adapter_ref: null,
    probe_results: [],
    catalog_fresh: true,
  };
  return { ...base, ...over };
}

const CANDIDATES: Candidate[] = [
  candidate('c_edge_code', `${LOCAL}\\Microsoft\\Edge\\User Data\\Default\\Code Cache`, 'microsoft.edge.code-cache@1.0.0', 'low', 812n * MiB, {
    reasons: ['Edge 的 JavaScript 字节码缓存', '应用身份与版本已在登记表中实测'],
    impact: 'Edge 下次打开网页时重新编译脚本，首次加载可能稍慢；不影响书签、登录或站点数据。',
    probes: ['owner_process'],
    installed_version: '141.0.3537.71',
    probe_results: [{ probe: 'owner_process', state: 'not_in_use', detail: 'msedge.exe is not running' }],
  }),
  candidate('c_chrome_code', `${LOCAL}\\Google\\Chrome\\User Data\\Default\\Code Cache`, 'google.chrome.code-cache@1.0.0', 'low', 640n * MiB + 12n * MiB, {
    reasons: ['Chrome 的 JavaScript 字节码缓存'],
    impact: 'Chrome 下次加载页面时重新生成；不影响站点数据。',
    probes: ['owner_process'],
    installed_version: '141.0.7390.66',
    probe_results: [{ probe: 'owner_process', state: 'not_in_use', detail: 'chrome.exe is not running' }],
  }),
  candidate('c_teams_cache', `${LOCAL}\\Microsoft\\Teams\\Cache`, 'microsoft.teams.cache@1.0.0', 'review', 318n * MiB, {
    reasons: ['Teams 的网页缓存'],
    impact: 'Teams 下次启动时重新下载界面资源。',
    probes: ['owner_process'],
    not_selectable: 'in_use',
    allowed_modes: [],
    installed_version: '25.1.0',
    registry_status: 'version_untested',
    probe_results: [{ probe: 'owner_process', state: 'in_use', detail: 'ms-teams.exe is running' }],
  }),
  candidate('c_downloads_iso', `${HOME}\\Downloads\\Win11_24H2.iso`, 'large.downloads@1.0.0', 'review', 5n * GiB + 400n * MiB, {
    kind: 'file',
    category: 'large_items',
    reasons: ['下载文件夹中超过 1 GiB、90 天未修改的大文件'],
    impact: '这是提示：请自行判断是否还需要它。CleanManager 不会把它放进计划。',
    not_selectable: 'hint',
    allowed_modes: [],
    row_kind: 'hint',
    registry_status: 'not_checked',
  }),
  candidate('c_temp_old', `${LOCAL}\\Temp\\vs-installer-2025`, 'windows.user-temp.stale-files@1.0.0', 'low', 1n * GiB + 210n * MiB, {
    category: 'temp',
    reasons: ['用户临时目录中 30 天未修改的文件'],
    impact: '安装程序的旧临时文件；正在运行的安装程序会占用文件，执行前会再次检查。',
    unknown_size_count: 3,
    logical_exact_bytes: null,
    not_selectable: 'unknown_size',
    allowed_modes: [],
  }),
  candidate('c_npm', `${LOCAL}\\npm-cache\\_cacache`, 'npm.cache@1.0.0', 'review', 2n * GiB + 300n * MiB, {
    category: 'dev_cache',
    reasons: ['npm 的下载缓存'],
    impact: '之后安装依赖需要重新下载；离线时无法安装尚未缓存的包。',
  }),
  candidate('c_pip', `${LOCAL}\\pip\\cache`, 'pip.cache@1.0.0', 'review', 486n * MiB, {
    category: 'dev_cache',
    reasons: ['pip 的 wheel 与 HTTP 缓存'],
    impact: '重新安装 Python 包时需要联网下载。',
  }),
  candidate('c_temp_locked', `${LOCAL}\\Temp\\locked-session`, 'windows.user-temp.stale-files@1.0.0', 'pending', 96n * MiB, {
    category: 'temp',
    reasons: ['子文件夹无法读取，年龄条件无法证明'],
    unknown_size_count: 1,
    logical_exact_bytes: null,
  }),
  candidate('c_claude', `${HOME}\\.claude\\projects`, 'ai.sessions@1.0.0', 'blocked', 1n * GiB + 40n * MiB, {
    category: 'ai_sessions',
    state: 'protected_reviewable',
    tier: 'review_only',
    not_selectable: 'protected_reviewable',
    reasons: ['AI 工具会话与配置'],
    protection: { class: 'ai_sessions_config', describe: 'AI tool sessions and configuration (.claude, .codex)', reviewable: true },
  }),
  candidate('c_edge_storage', `${LOCAL}\\Microsoft\\Edge\\User Data\\Default\\Local Storage`, 'microsoft.edge.code-cache@1.0.0', 'blocked', 58n * MiB, {
    reasons: ['浏览器站点存储'],
    protection: { class: 'browser_site_storage', describe: 'Browser site storage (IndexedDB, Local Storage, Cookies, Login Data, History...)', reviewable: false },
  }),
  candidate('c_models', `${HOME}\\models\\llama-3-8b.gguf`, 'ai.models@1.0.0', 'blocked', 4n * GiB + 600n * MiB, {
    kind: 'file',
    category: 'models',
    reasons: ['模型权重'],
    protection: { class: 'models', describe: 'Model weights (.gguf, .safetensors, .ckpt, .pt, .onnx)', reviewable: false },
  }),
];

interface MockPlan {
  detail: PlanDetail;
  items: PlanItem[];
  token: string | null;
  receipt: boolean;
  revoked: boolean;
  modes?: Mode;
}

interface MockOp {
  op: Operation;
  items: OperationItem[];
}

function counts(items: OperationItem[]): Counts {
  const c: Counts = { succeeded: 0, skipped: 0, failed: 0, not_started: 0, unknown: 0, in_flight: 0 };
  for (const i of items) {
    if (i.status === 'pending') continue;
    c[i.status] += 1;
  }
  return c;
}

function expand(c: Candidate, start: number): PlanItem[] {
  const names = ['index', 'data_0', 'data_1', 'data_2', 'f_000a1c', 'f_000b2d', 'js', 'wasm'];
  const n = c.candidate_id === 'c_npm' ? 8 : 6;
  const total = BigInt(c.logical_known_bytes);
  let left = total;
  return Array.from({ length: n }, (_, i) => {
    const size = i === n - 1 ? left : total / BigInt(n + i);
    left -= size;
    return {
      action_id: `a${String(start + i).padStart(7, '0')}`,
      name: names[i % names.length],
      path_display: `${c.path_display}\\${names[i % names.length]}`,
      logical_bytes: size.toString(),
      allocated_bytes: size.toString(),
      candidate_id: c.candidate_id,
      original_path_display: null,
      quarantine_id: null,
      item_kind: 'file',
    };
  });
}

export function mockOps(): OpsBackend {
  const progress = new Set<(e: ApplyProgressEvent) => void>();
  const finished = new Set<(e: ApplyFinishedEvent) => void>();
  const plans = new Map<string, MockPlan>();
  const ops = new Map<string, MockOp>();
  const quarantine: QuarantineItem[] = [];
  let analyzed = true;
  let planCounter = 0;
  let active: { plan_id: string; operation_id: string; total: number; running: boolean; cancel: boolean } | null = null;

  const setInfo = (): CandidateSetInfo => {
    const g = (k: Group) => {
      const m = CANDIDATES.filter((c) => c.group === k);
      return {
        count: m.length,
        known_logical_bytes: m.reduce((a, c) => a + BigInt(c.logical_known_bytes), 0n).toString(),
        unknown_size_count: m.reduce((a, c) => a + c.unknown_size_count, 0),
      };
    };
    return {
      candidate_set_id: 'cs_preview',
      scan_id: 's_preview_1',
      revision: 1,
      created_at: iso(-42),
      rule_set_digest: hex('rules'),
      rules_current: true,
      scan_completeness: 'partial_scope',
      scan_root_display: HOME,
      groups: { low: g('low'), review: g('review'), pending: g('pending'), blocked: g('blocked') },
      issues: [
        { code: 'rule_inapplicable', scope_display: 'google.chrome.code-cache@1.0.0', message: '该规则的根目录不在本次扫描范围内' },
        { code: 'protected_reviewable', scope_display: 'AI sessions and worktrees', message: '默认保护的对象只列出供逐项审阅，不会被选中或匹配策略' },
      ],
    };
  };

  const authOf = (p: MockPlan): Authorization => {
    const expired = Date.parse(p.detail.expires_at) <= Date.now();
    const status = p.receipt && !p.revoked ? 'authorized' : expired ? 'expired' : p.revoked ? 'revoked' : 'required';
    return {
      status,
      source: status === 'authorized' ? 'interactive_review' : null,
      policy_id: null,
      policy_version: null,
      principal: status === 'authorized' ? 'user:DESKTOP\\Lin' : null,
      authorized_at: status === 'authorized' ? iso(-1) : null,
      expires_at: status === 'authorized' ? p.detail.expires_at : null,
    };
  };

  const makePlan = (kind: PlanDetail['kind'], mode: Mode | null, items: PlanItem[], createdMin: number, extra: Partial<PlanDetail> = {}): MockPlan => {
    planCounter += 1;
    const plan_id = `${kind === 'cleanup' ? 'p' : kind === 'manual_trash' ? 'mp' : kind === 'startup' ? 'sp' : 'rp'}_desk_4242_${1760000000000 + planCounter}`;
    const digest = hex(plan_id);
    const total = items.reduce((a, i) => a + BigInt(i.logical_bytes), 0n);
    const created = iso(createdMin);
    const expires = new Date(Date.parse(created) + 15 * 60_000).toISOString().replace(/\.\d+Z$/, 'Z');
    const detail: PlanDetail = {
      plan_id,
      contract: kind === 'cleanup' ? 'plan/v1' : 'recovery/v1',
      kind,
      mode,
      digest,
      digest_prefix: digest.slice(0, 8),
      created_at: created,
      expires_at: expires,
      expired: false,
      seconds_left: 0,
      scan_id: kind === 'cleanup' ? 's_preview_1' : null,
      scan_revision: kind === 'cleanup' ? 1 : null,
      scope_id: kind === 'cleanup' ? 'scope_desk_9f2c41e0' : null,
      scan_root_display: kind === 'cleanup' ? HOME : null,
      rule_refs: [],
      rule_set_digest: kind === 'cleanup' ? hex('rules') : null,
      stale_reason: null,
      item_count: items.length,
      total_logical_bytes: total.toString(),
      allocated_known_bytes: total.toString(),
      allocated_unknown_count: 0,
      default_selected_items: 0,
      authorization: { status: 'required', source: null, policy_id: null, policy_version: null, principal: null, authorized_at: null, expires_at: null },
      policies: [],
      not_included: null,
      operation: null,
      review_token: null,
      cli: { show: `cleanmanager plan show ${plan_id}`, review: `cleanmanager plan review ${plan_id}`, apply: `cleanmanager plan apply ${plan_id}` },
      ...extra,
    };
    const p: MockPlan = { detail, items, token: null, receipt: false, revoked: false };
    plans.set(plan_id, p);
    return p;
  };

  const policies: Policy[] = [
    {
      policy_id: 'pol_desk_browser_caches',
      version: 2,
      state: 'paused',
      expired: false,
      principal: 'user:DESKTOP\\Lin',
      scope_id: 'scope_desk_9f2c41e0',
      rules: ['google.chrome.code-cache@1.0.0', 'microsoft.edge.code-cache@1.0.0'],
      mode: 'quarantine',
      created_at: iso(-60 * 24 * 3),
      expires_at: iso(60 * 24 * 27),
      single_max_bytes: GiB.toString(),
      single_max_items: 1000,
      window_max_bytes: (2n * GiB).toString(),
      window_max_items: 2000,
      window_used_bytes: '0',
      window_used_items: 0,
    },
    {
      policy_id: 'pol_cli_temp_2026',
      version: 3,
      state: 'revoked',
      expired: false,
      principal: 'user:DESKTOP\\Lin',
      scope_id: 'scope_9d1e',
      rules: ['windows.user-temp.stale-files@1.0.0'],
      mode: 'trash',
      created_at: iso(-60 * 24 * 12),
      expires_at: iso(60 * 24 * 18),
      single_max_bytes: GiB.toString(),
      single_max_items: 1000,
      window_max_bytes: (2n * GiB).toString(),
      window_max_items: 2000,
      window_used_bytes: (310n * MiB).toString(),
      window_used_items: 41,
    },
  ];

  // History: a completed quarantine yesterday, a partial trash run, and one interrupted run.
  const seedOp = (kind: 'cleanup' | 'restore', mode: Mode | null, cands: Candidate[], statuses: OperationItem['status'][], state: string, minAgo: number) => {
    const items = cands.flatMap((c, i) => expand(c, i * 10));
    const p = makePlan(kind, mode, items, -minAgo);
    p.receipt = true;
    p.detail.rule_refs = [...new Set(cands.flatMap((c) => c.rules))];
    const opId = `op_${p.detail.digest.slice(0, 16)}`;
    const opItems: OperationItem[] = items.map((it, i) => {
      const st = statuses[i % statuses.length];
      return {
        action_id: it.action_id,
        name: it.name,
        path_display: it.path_display,
        original_path_display: null,
        logical_bytes: it.logical_bytes,
        status: st,
        code: st === 'succeeded' ? 'done' : st === 'skipped' ? 'precondition_in_use' : st === 'unknown' ? 'outcome_unconfirmed' : st === 'failed' ? 'access_denied' : 'cancelled',
        message: st === 'failed' ? 'Access is denied. (os error 5)' : 'see code',
        recovery: st === 'succeeded' ? (mode === 'quarantine' ? 'quarantine' : mode === 'trash' ? 'trash' : 'unavailable') : st === 'unknown' ? 'unknown' : 'not_applicable',
      };
    });
    const c = counts(opItems);
    const processed = opItems.filter((i) => i.status === 'succeeded').reduce((a, i) => a + BigInt(i.logical_bytes ?? '0'), 0n);
    ops.set(opId, {
      items: opItems,
      op: {
        operation_id: opId,
        plan_id: p.detail.plan_id,
        plan_kind: kind,
        mode,
        state,
        running_here: false,
        cancel_requested: false,
        started_at: iso(-minAgo + 1),
        finished_at: state === 'reconciliation_required' ? null : iso(-minAgo + 2),
        authorization_source: 'interactive_review',
        total: items.length,
        counts: c,
        report_available: state !== 'reconciliation_required',
        processed_logical_bytes: processed.toString(),
        quarantined_logical_bytes: mode === 'quarantine' ? processed.toString() : '0',
        estimated_freed_bytes: mode === 'delete' ? processed.toString() : '0',
        volume_free_before_bytes: (48n * GiB).toString(),
        volume_free_after_bytes: (48n * GiB + 2n * MiB).toString(),
        observed_free_delta_bytes: (2n * MiB).toString(),
        needs_reconcile: state === 'reconciliation_required' || c.unknown > 0,
      },
    });
    p.detail.operation = { operation_id: opId, state };
    if (mode === 'quarantine') {
      opItems
        .filter((i) => i.status === 'succeeded')
        .forEach((i, n) => {
          quarantine.push({
            quarantine_id: `q_${opId.slice(3, 15)}_${i.action_id}`,
            plan_id: p.detail.plan_id,
            operation_id: opId,
            name: i.name,
            original_path_display: i.path_display,
            location_display: `${LOCAL}\\CleanManager\\store\\quarantine\\${opId}\\${i.action_id}`,
            logical_bytes: i.logical_bytes ?? '0',
            quarantined_at: iso(-minAgo + 2),
            reminder_at: iso(-minAgo + 2 + 60 * 24 * 30),
            reminder_due: n === 0 && minAgo > 60 * 24 * 30,
            state: n === 2 ? 'target_conflict' : 'recoverable',
            actionable: true,
          });
        });
    }
    return opId;
  };
  seedOp('cleanup', 'quarantine', [CANDIDATES[0]], ['succeeded'], 'completed', 60 * 26);
  seedOp('cleanup', 'trash', [CANDIDATES[4]], ['succeeded', 'succeeded', 'skipped', 'succeeded', 'skipped', 'succeeded'], 'partial', 60 * 50);
  seedOp('cleanup', 'delete', [CANDIDATES[1]], ['succeeded', 'succeeded', 'unknown', 'not_started', 'not_started', 'not_started'], 'reconciliation_required', 60 * 24 * 4);

  const history = (): HistoryItem[] =>
    [...ops.values()]
      .map(({ op }) => ({
        operation_id: op.operation_id,
        plan_id: op.plan_id,
        contract: op.plan_kind === 'cleanup' ? 'plan/v1' : 'recovery/v1',
        action: (op.plan_kind === 'cleanup' ? op.mode : op.plan_kind) as HistoryItem['action'],
        state: op.state,
        started_at: op.started_at ?? '',
        finished_at: op.finished_at,
        authorization_source: op.authorization_source,
        rule_refs: plans.get(op.plan_id)?.detail.rule_refs ?? [],
        counts: op.report_available ? op.counts : null,
        processed_logical_bytes: op.processed_logical_bytes,
        availability: (op.needs_reconcile
          ? 'unknown'
          : op.counts.succeeded === 0
            ? 'not_applicable'
            : op.mode === 'quarantine'
              ? 'in_app_restore'
              : op.mode === 'trash'
                ? 'system_recycle_bin'
                : op.plan_kind === 'restore'
                  ? 'not_applicable'
                  : 'unavailable') as HistoryItem['availability'],
      }))
      .sort((a, b) => (a.started_at < b.started_at ? 1 : -1));

  const protection: ProtectionItem[] = [
    { protection_id: 'hd_textual', layer: 'hard_deny', match: 'glob', display: 'System directories, volume and share roots, device and administrative paths (hard-deny/3)', removable: false, note: null, added_at: null },
    { protection_id: 'hd_3c1f09ab22e1', layer: 'hard_deny', match: 'descendants', display: 'C:\\Windows (system_root)', removable: false, note: null, added_at: null },
    { protection_id: 'hd_8a0e4410bc93', layer: 'hard_deny', match: 'descendants', display: 'C:\\Program Files (program_files)', removable: false, note: null, added_at: null },
    { protection_id: 'hd_1d77e02f6a50', layer: 'hard_deny', match: 'exact', display: 'C:\\Users\\Lin (user_profile_root)', removable: false, note: null, added_at: null },
    ...(['credentials', 'database', 'browser_site_storage', 'ai_sessions_config', 'source_worktree', 'models', 'user_documents', 'system_repair'] as const).map(
      (c): ProtectionItem => ({ protection_id: `dc_${c}`, layer: 'default_category', match: 'descendants', display: CLASS_DESCRIBE[c], removable: false, note: null, added_at: null }),
    ),
    { protection_id: 'u_1', layer: 'user', match: 'descendants', display: 'D:\\Work\\Projects\\storefront', removable: true, note: '正在开发的项目', added_at: iso(-60 * 24 * 9) },
    { protection_id: 'u_2', layer: 'user', match: 'rule', display: 'pip.cache', removable: true, note: null, added_at: iso(-60 * 24 * 2) },
  ];
  let userCounter = 2;

  const apps: AppItem[] = [
    app('Microsoft Edge', '141.0.3537.71', 'Microsoft Corporation', 'registry_uninstall', 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application', 'known', 612n * MiB, 0),
    app('Google Chrome', '141.0.7390.66', 'Google LLC', 'registry_uninstall', 'C:\\Program Files\\Google\\Chrome\\Application', 'not_in_scan', null, null),
    app('Visual Studio Code', '1.105.1', 'Microsoft Corporation', 'registry_uninstall', `${LOCAL}\\Programs\\Microsoft VS Code`, 'known', 734n * MiB, 0),
    app('Node.js', '22.11.0', 'Node.js Foundation', 'msi', 'C:\\Program Files\\nodejs', 'not_in_scan', null, null),
    app('Python 3.12.7 (64-bit)', '3.12.7150.0', 'Python Software Foundation', 'msi', `${LOCAL}\\Programs\\Python\\Python312`, 'known', 214n * MiB, 2),
    app('Python Launcher', '3.12.7150.0', 'Python Software Foundation', 'msi', `${LOCAL}\\Programs\\Python\\Python312`, 'known', 214n * MiB, 2, ['Python 3.12.7 (64-bit)']),
    app('微信', '4.0.3.22', 'Tencent', 'registry_uninstall', null, 'no_location', null, null),
    app('Microsoft Visual C++ 2015-2022 Redistributable (x64)', '14.44.35211.0', 'Microsoft Corporation', 'registry_uninstall', null, 'no_location', null, null, [], 'shared_runtime'),
    app('Windows Terminal', '1.22.12111.0', 'Microsoft Corporation', 'msix_store', 'C:\\Program Files\\WindowsApps\\Microsoft.WindowsTerminal_1.22.12111.0_x64__8wekyb3d8bbwe', 'unresolved', null, null),
  ];
  apps[4].shared_with = ['Python Launcher'];

  const emitProgress = (opId: string) => {
    const m = ops.get(opId)!;
    const c = counts(m.items);
    progress.forEach((cb) => cb({ plan_id: m.op.plan_id, operation_id: opId, total: m.op.total, counts: c, cancel_requested: active?.cancel ?? false }));
  };

  return {
    __plan(kind, mode, items, extra = {}) {
      return makePlan(kind, mode, items, 0, extra).detail.plan_id;
    },
    async candidateSummary(scanId) {
      await delay(120);
      return analyzed && scanId === 's_preview_1' ? setInfo() : null;
    },
    async analyzeScan(scanId) {
      await delay(900);
      if (scanId !== 's_preview_1') throw err('invalid_argument', 'not_published', 'The scan has not published results yet');
      analyzed = true;
      return setInfo();
    },
    async listCandidates(_set, group, pageSize, cursor) {
      await delay(100);
      const m = CANDIDATES.filter((c) => c.group === group).sort((a, b) => {
        const ua = a.unknown_size_count > 0 ? 1 : 0;
        const ub = b.unknown_size_count > 0 ? 1 : 0;
        if (ua !== ub) return ua - ub;
        const da = BigInt(a.logical_known_bytes);
        const db = BigInt(b.logical_known_bytes);
        return da === db ? 0 : da > db ? -1 : 1;
      });
      const start = cursor ? Number(cursor.split(':o')[1]) : 0;
      const items = m.slice(start, start + pageSize);
      return { candidate_set_id: 'cs_preview', group, total: m.length, items, next_cursor: start + pageSize < m.length ? `cs_preview.${group}.${pageSize}:o${start + pageSize}` : null };
    },
    async createPlan(_set, ids, mode) {
      await delay(300);
      const chosen = ids.map((id) => CANDIDATES.find((c) => c.candidate_id === id));
      if (chosen.some((c) => !c || c.not_selectable)) throw err('policy_denied', 'not_selectable', 'candidate cannot be planned');
      let n = 0;
      const items = chosen.flatMap((c) => {
        const out = expand(c!, n);
        n += out.length;
        return out;
      });
      const p = makePlan('cleanup', mode, items, 0, {
        rule_refs: [...new Set(chosen.flatMap((c) => c!.rules))].sort(),
        default_selected_items: chosen.filter((c) => c!.tier === 'default_selected').reduce((a, c) => a + expand(c!, 0).length, 0),
        not_included: {
          low_unselected: CANDIDATES.filter((c) => c.group === 'low' && !ids.includes(c.candidate_id)).length,
          review_unselected: CANDIDATES.filter((c) => c.group === 'review' && !ids.includes(c.candidate_id)).length,
          pending: CANDIDATES.filter((c) => c.group === 'pending').length,
          blocked: CANDIDATES.filter((c) => c.group === 'blocked').length,
        },
      });
      p.detail.policies = policies.map((pol) => ({
        policy_id: pol.policy_id,
        version: pol.version,
        state: pol.state,
        covers: false,
        reason: pol.state !== 'enabled' ? 'not_enabled' : 'scope_differs',
        detail: null,
      }));
      return { plan_id: p.detail.plan_id };
    },
    async planDetail(planId) {
      await delay(140);
      const p = plans.get(planId);
      if (!p) throw err('invalid_argument', 'not_found', `not found: ${planId}`);
      const auth = authOf(p);
      const secondsLeft = Math.max(0, Math.round((Date.parse(p.detail.expires_at) - Date.now()) / 1000));
      if (auth.status === 'required' && !p.token) p.token = `rv${Math.random().toString(16).slice(2)}`;
      const stale = secondsLeft === 0 && !p.receipt ? 'expired' : null;
      return { ...p.detail, authorization: auth, seconds_left: secondsLeft, expired: secondsLeft === 0, stale_reason: p.detail.operation ? null : stale, review_token: auth.status === 'required' && !stale ? p.token : null };
    },
    async planItems(planId, pageSize, cursor) {
      await delay(100);
      const p = plans.get(planId);
      if (!p) throw err('invalid_argument', 'not_found', 'not found');
      const start = cursor ? Number(cursor.slice(1)) : 0;
      return { plan_id: planId, total: p.items.length, items: p.items.slice(start, start + pageSize), next_cursor: start + pageSize < p.items.length ? `o${start + pageSize}` : null };
    },
    async approvePlan(planId, token, typed) {
      await delay(250);
      const p = plans.get(planId);
      if (!p || !p.token || p.token !== token) throw err('invalid_argument', 'review_expired', 'This review session expired; reopen the plan to review it again');
      if (typed.toLowerCase() !== p.detail.digest_prefix) throw err('invalid_argument', 'confirmation_mismatch', 'The code does not match');
      p.receipt = true;
      p.revoked = false;
      p.token = null;
      return authOf(p);
    },
    async revokePlan(planId) {
      const p = plans.get(planId);
      if (!p) throw err('invalid_argument', 'not_found', 'not found');
      p.revoked = true;
      p.receipt = false;
      return authOf(p);
    },
    async listPolicies() {
      await delay(80);
      return policies.map((p) => ({ ...p }));
    },
    async setPolicyState(id, state, confirm) {
      await delay(150);
      const p = policies.find((x) => x.policy_id === id);
      if (!p) throw err('invalid_argument', 'not_found', 'Unknown policy');
      if (p.state === 'revoked') throw err('conflict', 'policy_revoked', 'A revoked policy cannot change state');
      if (state === 'enabled' && confirm !== id) throw err('invalid_argument', 'confirmation_mismatch', 'Type the policy id');
      p.state = state;
      p.version += 1;
      return policies.map((x) => ({ ...x }));
    },
    async createPolicyFromPlan(planId) {
      const plan = plans.get(planId);
      if (!plan || plan.detail.kind !== 'cleanup') throw err('invalid_argument', 'not_cleanup_plan', 'Recovery plans are never covered by a policy');
      policies.push({ ...policies[0], policy_id: `pol_desk_${Date.now()}`, version: 1, state: 'paused', rules: plan.detail.rule_refs, mode: plan.detail.mode!, created_at: iso(0), expires_at: iso(60 * 24 * 30), window_used_bytes: '0', window_used_items: 0 });
      return policies.map((x) => ({ ...x }));
    },
    async startApply(planId) {
      await delay(200);
      const p = plans.get(planId);
      if (!p) throw err('invalid_argument', 'not_found', 'not found');
      if (p.detail.operation) return { plan_id: planId, operation_id: p.detail.operation.operation_id, total: p.items.length, existing: true };
      if (authOf(p).status !== 'authorized') throw err('needs_approval', 'needs_approval', `plan ${planId} needs approval`);
      if (active?.running) throw err('conflict', 'operation_running', 'Another operation is still running');
      const opId = `op_${p.detail.digest.slice(0, 16)}`;
      const items: OperationItem[] = p.items.map((i) => ({
        action_id: i.action_id,
        name: i.name,
        path_display: i.path_display,
        original_path_display: i.original_path_display,
        logical_bytes: i.logical_bytes,
        status: 'pending',
        code: null,
        message: null,
        recovery: null,
      }));
      const mode = p.detail.mode;
      const op: Operation = {
        operation_id: opId,
        plan_id: planId,
        plan_kind: p.detail.kind,
        mode,
        state: 'running',
        running_here: true,
        cancel_requested: false,
        started_at: iso(0),
        finished_at: null,
        authorization_source: 'interactive_review',
        total: items.length,
        counts: counts(items),
        report_available: false,
        processed_logical_bytes: null,
        quarantined_logical_bytes: null,
        estimated_freed_bytes: null,
        volume_free_before_bytes: null,
        volume_free_after_bytes: null,
        observed_free_delta_bytes: null,
        needs_reconcile: false,
      };
      ops.set(opId, { op, items });
      p.detail.operation = { operation_id: opId, state: 'running' };
      active = { plan_id: planId, operation_id: opId, total: items.length, running: true, cancel: false };
      let i = 0;
      const tick = setInterval(() => {
        const cur = active!;
        if (i < items.length) {
          const it = items[i];
          if (cur.cancel) {
            it.status = 'not_started';
            it.code = 'cancelled';
          } else if (p.detail.kind === 'restore' && it.name === 'data_1') {
            it.status = 'skipped';
            it.code = 'target_conflict';
          } else if (p.detail.kind === 'cleanup' && i === 3) {
            it.status = 'skipped';
            it.code = 'precondition_in_use';
            it.message = 'the file is in use by another process';
          } else {
            it.status = 'succeeded';
            it.code = p.detail.kind === 'restore' ? 'restored' : p.detail.kind === 'purge' ? 'purged' : 'done';
            it.recovery = mode === 'quarantine' ? 'quarantine' : mode === 'trash' ? 'trash' : 'unavailable';
          }
          i += 1;
          op.counts = counts(items);
          emitProgress(opId);
          return;
        }
        clearInterval(tick);
        const c = counts(items);
        const processed = items.filter((x) => x.status === 'succeeded').reduce((a, x) => a + BigInt(x.logical_bytes ?? '0'), 0n);
        Object.assign(op, {
          state: c.succeeded === items.length ? 'completed' : cur.cancel ? 'cancelled' : 'partial',
          running_here: false,
          finished_at: iso(0),
          report_available: true,
          counts: c,
          processed_logical_bytes: processed.toString(),
          quarantined_logical_bytes: mode === 'quarantine' ? processed.toString() : '0',
          estimated_freed_bytes: mode === 'delete' || p.detail.kind === 'purge' ? processed.toString() : '0',
          volume_free_before_bytes: (48n * GiB).toString(),
          volume_free_after_bytes: (48n * GiB + MiB).toString(),
          observed_free_delta_bytes: MiB.toString(),
        });
        p.detail.operation = { operation_id: opId, state: op.state };
        if (mode === 'quarantine') {
          items
            .filter((x) => x.status === 'succeeded')
            .forEach((x) =>
              quarantine.push({
                quarantine_id: `q_${opId.slice(3, 15)}_${x.action_id}`,
                plan_id: planId,
                operation_id: opId,
                name: x.name,
                original_path_display: x.path_display,
                location_display: `${LOCAL}\\CleanManager\\store\\quarantine\\${opId}\\${x.action_id}`,
                logical_bytes: x.logical_bytes ?? '0',
                quarantined_at: iso(0),
                reminder_at: iso(60 * 24 * 30),
                reminder_due: false,
                state: 'recoverable',
                actionable: true,
              }),
            );
        }
        if (p.detail.kind === 'restore' || p.detail.kind === 'purge') {
          for (const x of items.filter((y) => y.status === 'succeeded')) {
            const qi = p.items.find((y) => y.action_id === x.action_id)?.quarantine_id;
            const at = quarantine.findIndex((q) => q.quarantine_id === qi);
            if (at >= 0) quarantine.splice(at, 1);
          }
        }
        cur.running = false;
        finished.forEach((cb) => cb({ plan_id: planId, operation_id: opId, status: op.state, error: null }));
      }, 380);
      return { plan_id: planId, operation_id: opId, total: items.length, existing: false };
    },
    async cancelApply(opId) {
      if (active && active.operation_id === opId && active.running) {
        active.cancel = true;
        const m = ops.get(opId);
        if (m) m.op.cancel_requested = true;
        emitProgress(opId);
        return true;
      }
      return false;
    },
    async activeApply() {
      return active ? { plan_id: active.plan_id, operation_id: active.operation_id, total: active.total, running: active.running, cancel_requested: active.cancel } : null;
    },
    async operationDetail(opId) {
      await delay(100);
      const m = ops.get(opId);
      if (!m) throw err('invalid_argument', 'not_found', `not found: ${opId}`);
      return { ...m.op, counts: counts(m.items) };
    },
    async operationItems(opId, filter, pageSize, cursor) {
      await delay(90);
      const m = ops.get(opId);
      if (!m) throw err('invalid_argument', 'not_found', 'not found');
      const all = m.items.filter((i) => (filter === 'all' ? true : filter === 'problems' ? i.status !== 'succeeded' : i.status === filter));
      const start = cursor ? Number(cursor.slice(1)) : 0;
      return { operation_id: opId, total: all.length, items: all.slice(start, start + pageSize).map((i) => ({ ...i })), next_cursor: start + pageSize < all.length ? `o${start + pageSize}` : null };
    },
    async reconcileOperation(opId) {
      await delay(700);
      const m = ops.get(opId);
      if (!m) throw err('invalid_argument', 'not_found', 'not found');
      for (const it of m.items) {
        if (it.status === 'unknown') {
          it.status = 'succeeded';
          it.code = 'reconciled_performed';
          it.recovery = 'unavailable';
        }
      }
      const c = counts(m.items);
      Object.assign(m.op, { state: c.succeeded === m.items.length ? 'completed' : 'partial', needs_reconcile: false, report_available: true, finished_at: iso(0), counts: c, observed_free_delta_bytes: null, volume_free_before_bytes: null, volume_free_after_bytes: null });
      return { ...m.op };
    },
    async listOperations(pageSize, cursor) {
      await delay(120);
      const all = history();
      const start = cursor ? Number(cursor.slice(1)) : 0;
      return { items: all.slice(start, start + pageSize), next_cursor: start + pageSize < all.length ? `o${start + pageSize}` : null };
    },
    async listPlans(pageSize) {
      await delay(110);
      const rows = [...plans.values()]
        .map((p) => ({
          plan_id: p.detail.plan_id,
          contract: p.detail.contract,
          kind: p.detail.kind,
          mode: p.detail.mode,
          item_count: p.items.length,
          created_at: p.detail.created_at,
          expires_at: p.detail.expires_at,
          expired: Date.parse(p.detail.expires_at) <= Date.now(),
          authorization: authOf(p).status,
          operation_id: p.detail.operation?.operation_id ?? null,
          problem: null,
        }))
        .sort((a, b) => (a.created_at < b.created_at ? 1 : -1));
      return { items: rows.slice(0, pageSize), next_cursor: null };
    },
    async listQuarantine(pageSize, cursor) {
      await delay(130);
      const start = cursor ? Number(cursor.slice(1)) : 0;
      const total = quarantine.reduce((a, q) => a + BigInt(q.logical_bytes), 0n);
      return {
        items: quarantine.slice(start, start + pageSize),
        next_cursor: start + pageSize < quarantine.length ? `o${start + pageSize}` : null,
        held_count: cursor ? null : quarantine.length,
        held_logical_bytes: cursor ? null : total.toString(),
        metadata_preserved: ['contents', 'name', 'attributes', 'timestamps', 'security_descriptor'],
        metadata_limits: ['the change time (ctime) is updated by the move'],
      };
    },
    async createRecoveryPlan(kind, ids) {
      await delay(250);
      const chosen = ids.map((id) => quarantine.find((q) => q.quarantine_id === id));
      if (chosen.some((q) => !q)) throw err('invalid_argument', 'not_found', 'unknown quarantine item');
      const bad = chosen.find((q) => q!.state !== 'recoverable');
      if (bad) throw err('conflict', 'not_recoverable', `quarantine item ${bad.quarantine_id} is ${bad.state}, not recoverable`);
      const items: PlanItem[] = chosen.map((q, i) => ({
        action_id: `a${String(i).padStart(7, '0')}`,
        name: q!.name,
        path_display: q!.location_display,
        logical_bytes: q!.logical_bytes,
        allocated_bytes: null,
        candidate_id: null,
        original_path_display: q!.original_path_display,
        quarantine_id: q!.quarantine_id,
        item_kind: 'file',
      }));
      const p = makePlan(kind, null, items, 0);
      return { plan_id: p.detail.plan_id };
    },
    async listProtection() {
      await delay(90);
      return { items: protection.map((p) => ({ ...p })), problem: null, file_display: `${LOCAL}\\CleanManager\\store\\protection.json` };
    },
    async addProtection(target, note) {
      await delay(200);
      userCounter += 1;
      const display = target.source === 'rule' ? target.rule_id : target.source === 'entry' ? `${HOME}\\entry-${target.entry_id}` : 'D:\\Work\\Archives';
      protection.push({
        protection_id: `u_${userCounter}`,
        layer: 'user',
        match: target.source === 'rule' ? 'rule' : target.source === 'entry' && !target.recursive ? 'exact' : 'descendants',
        display,
        removable: true,
        note,
        added_at: iso(0),
      });
      return { items: protection.map((p) => ({ ...p })), problem: null, file_display: `${LOCAL}\\CleanManager\\store\\protection.json` };
    },
    async removeProtection(id, confirm) {
      await delay(200);
      if (confirm !== id) throw err('invalid_argument', 'confirmation_mismatch', 'Type the entry id to remove this protection');
      const at = protection.findIndex((p) => p.protection_id === id && p.removable);
      if (at < 0) throw err('invalid_argument', 'not_removable', 'not removable');
      protection.splice(at, 1);
      return { items: protection.map((p) => ({ ...p })), problem: null, file_display: `${LOCAL}\\CleanManager\\store\\protection.json` };
    },
    async listApps(scanId) {
      await delay(400);
      return {
        items: apps.map((a) => ({ ...a, size: scanId ? a.size : { ...a.size, state: a.size.state === 'known' ? 'not_in_scan' : a.size.state, logical_known_bytes: null } })),
        issues: [{ code: 'package_query_partial', scope_display: 'msix', message: '2 个包的清单无法读取，显示为位置未解析' }],
        scan_id: scanId,
        scan_root_display: scanId ? HOME : null,
        supported: true,
      };
    },
    async onApplyProgress(cb) {
      progress.add(cb);
      return () => progress.delete(cb);
    },
    async onApplyFinished(cb) {
      finished.add(cb);
      return () => finished.delete(cb);
    },
  };
}

const CLASS_DESCRIBE: Record<string, string> = {
  credentials: 'Credentials and keys (.ssh, .gnupg, .aws, .azure, .kube, Credentials, Protect, Vault, Keychains, SystemCertificates)',
  database: 'Database files and their journals (.db, .sqlite, -wal, -shm, .mdb, .accdb)',
  browser_site_storage: 'Browser site storage (IndexedDB, Local Storage, Cookies, Login Data, History...)',
  ai_sessions_config: 'AI tool sessions and configuration (.claude, .codex); sessions are reviewable one by one',
  source_worktree: 'Git worktrees (directories with .git); a clean worktree is reviewable one by one, a dirty one is blocked',
  models: 'Model weights (.gguf, .safetensors, .ckpt, .pt, .onnx)',
  user_documents: 'User documents (resolved by identity)',
  system_repair: 'System repair and installation stores (resolved by identity)',
};

function app(
  name: string,
  version: string,
  publisher: string,
  source: AppItem['source'],
  location: string | null,
  state: AppItem['size']['state'],
  bytes: bigint | null,
  unknown: number | null,
  shared: string[] = [],
  prot: string | null = null,
): AppItem {
  return {
    instance_id: `app_${hex(name).slice(0, 32)}`,
    name,
    version,
    publisher,
    source,
    location_display: location,
    evidence: [
      source === 'msix_store' ? `package_full_name: ${name}` : `registry_key: HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\${name}`,
      `display_name: ${name}`,
      ...(location ? [`install_location: ${location}`] : []),
      ...(state === 'unresolved' ? ['location_unresolved: access denied'] : []),
    ],
    protected_reason: prot,
    uninstall_state: 'unsupported',
    uninstall_reason: 'Read-only first release: CleanManager does not run uninstallers or delete package folders; use Windows Settings or the vendor uninstaller',
    update_status: 'not_checked',
    shared_with: shared,
    size: { state, logical_known_bytes: bytes === null ? null : bytes.toString(), unknown_descendants: unknown, entry_id: state === 'known' ? '42' : null },
  };
}
