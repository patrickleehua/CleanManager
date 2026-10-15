// DEVELOPMENT-ONLY fixture for the optional AI diagnosis in the browser preview (loaded through
// mock.ts behind `import.meta.env.DEV`). No provider is contacted and no key exists: answers are
// canned per object type and pass through the same states the host reports (preparing,
// running, completed, cancelled, failed, expired). Grounding results mirror the core's rules.

import type {
  Answer,
  DiagBackend,
  DiagFinishedEvent,
  DiagResult,
  DiagState,
  DiagStatus,
  DiagSummary,
  Diagnosis,
  Disclosure,
  Evidence,
  EvidenceItem,
  Gap,
  Handoff,
  NextAction,
  Origin,
  Preview,
  ProviderStatus,
  Recommendation,
} from './diagTypes';

export interface MockObject {
  name: string;
  path: string;
  kind: 'file' | 'directory' | string;
  logical: string | null;
  unknownDesc: number;
}

type Scenario = 'cache' | 'video' | 'model' | 'installer' | 'database' | 'partial' | 'generic';

const T_SCAN = '2026-10-15T01:12:41Z';

function err(code: string, reason: string, message: string) {
  return { code, reason, message };
}

function hex(n: number, len = 20): string {
  let s = '';
  let x = (Math.imul(n + 1, 2654435761) ^ 0x5bd1e995) >>> 0;
  while (s.length < len) {
    x = (Math.imul(x, 1103515245) + 12345) >>> 0;
    s += ((x >>> 16) & 0xf).toString(16);
  }
  return s;
}

function scenarioOf(o: MockObject): Scenario {
  const n = o.name.toLowerCase();
  if (o.unknownDesc > 0 && o.kind === 'directory') return 'partial';
  if (n.includes('cache')) return 'cache';
  if (/\.(mov|mp4|mkv)$/.test(n)) return 'video';
  if (/\.(safetensors|gguf|ckpt|onnx)$/.test(n)) return 'model';
  if (/\.(exe|msi|iso)$/.test(n) || n.includes('setup')) return 'installer';
  if (/\.(sqlite|db)$/.test(n)) return 'database';
  return 'generic';
}

function redactLocation(path: string): string {
  const parts = path.split('\\');
  parts.pop();
  const keep = parts.slice(-3);
  return (parts.length > 3 ? ['…', ...keep] : keep).join('\\');
}

function evidenceFor(sc: Scenario, o: MockObject, d: Disclosure, statements: string[]): Evidence {
  const items: EvidenceItem[] = [];
  const add = (layer: EvidenceItem['layer'], code: string, text: string) => items.push({ evidence_id: `ev_${items.length + 1}`, layer, code, text });
  add('observation', 'observation.scan', `索引观察时间 ${T_SCAN}；这次扫描${sc === 'partial' ? '部分范围未读取' : '范围已完整读取'}。`);
  add('observation', 'observation.size', o.logical ? `${o.kind === 'directory' ? '目录内已知逻辑大小合计' : '逻辑大小'} ${o.logical} 字节。` : '大小未知（有无法读取的子范围）。');
  if (o.kind !== 'directory') add('observation', 'observation.modified', '最后修改时间 2026-03-02T09:14:00Z（文件系统记录，不代表最后使用时间）。');
  if (sc === 'cache') {
    add('rule', 'rule.matched', '匹配规则 example.browser.code-cache@0.1.0（类别 app_caches）：下次启动时重新生成；可由所属程序在本机重建。');
    add('rule', 'rule.candidate_state', '核心判定：状态 allowed，层级 default_selected；原因 无。');
  } else if (sc === 'partial') {
    add('rule', 'rule.matched', '匹配规则 purge.target@1.0.0（类别 project_build_output）：构建输出，可在本机重新构建。');
    add('observation', 'observation.subtree', '目录下索引到 412 个文件、38 个子目录。1 处子范围无法读取。');
  } else {
    add('rule', 'rule.none', '没有已启用的清理规则匹配这个对象；这不说明它是垃圾，也不说明它重要。');
  }
  if (sc === 'model') add('protection', 'protection.builtin_self', '属于默认保护类别 models：Model weights (.gguf, .safetensors, .ckpt, .pt, .onnx)。');
  else if (sc === 'database') add('protection', 'protection.builtin_self', '属于默认保护类别 database：Database files and their journals。');
  else add('protection', 'protection.none', '硬性保护、默认保护类别和你的保护名单都没有命中。');
  if (sc === 'model') add('observation', 'observation.hard_links', '该文件有 2 个名称（硬链接）；只删除其中一个名称不会释放空间。');
  if (sc === 'model') add('ownership', 'ownership.app', '位于已安装应用「Example Model Runner」记录的安装位置，并与其他应用共享该位置。');
  else add('ownership', 'ownership.none', '没有安装记录指向这个位置；这不证明它是残留文件。');
  statements.forEach((t) => add('user_statement', 'user.statement', `用户陈述（未经核验）：${t}`));
  const gaps: Gap[] = [];
  const gap = (code: string, text: string) => gaps.push({ gap_id: `gap_${gaps.length + 1}`, code, text });
  if (sc === 'partial') {
    gap('partial_scan', '这次扫描有未读取的范围。');
    gap('unread_descendants', '目录内有无法读取的子范围，其中内容未知，不能视为可清理。');
  }
  gap('backup_unknown', 'CleanManager 不检测备份；是否已有可用备份未知。');
  gap('usage_unknown', '未检测是否有程序或工程正在使用它。');
  gap('reclaim_unknown', '删除后实际能释放多少空间未知（共享块、硬链接、回收站等）。');
  if (o.kind === 'directory' && !d.child_summary) gap('children_not_disclosed', '未发送子项摘要。');
  const ext = o.kind === 'directory' ? null : (o.name.split('.').pop() ?? null);
  return {
    version: 'diag-evidence/1',
    subject: {
      kind: o.kind,
      name: o.name,
      extension: ext,
      location: d.full_path ? o.path.split('\\').slice(0, -1).join('\\') : redactLocation(o.path),
      logical_bytes: o.logical,
      allocated_bytes: o.kind === 'directory' ? null : o.logical,
      size_basis: o.logical ? (o.kind === 'directory' ? 'subtree_known' : 'entity') : 'unknown',
      modified_at: o.kind === 'directory' ? null : '2026-03-02T09:14:00Z',
      link_count: sc === 'model' ? 2 : o.kind === 'directory' ? null : 1,
      cloud: 'not_cloud',
      identity_quality: 'strong',
    },
    children: o.kind === 'directory' && d.child_summary ? [{ name: 'js', kind: 'directory', logical_bytes: '210763776' }, { name: 'index-dir', kind: 'directory', logical_bytes: '4194304' }] : null,
    children_omitted: 0,
    items,
    gaps,
    redactions: d.full_path ? [] : [{ field: 'subject.location', rule: 'parent_components' }],
    excluded: ['file_contents', 'directory_tree', 'credentials', 'sessions', 'cloud_hydration'],
  };
}

function answerFor(sc: Scenario, ev: Evidence, answered: boolean): { answer: Answer; result: Omit<DiagResult, 'answer'> } {
  const id = (code: string) => ev.items.find((i) => i.code === code)?.evidence_id ?? 'ev_1';
  const user = ev.items.filter((i) => i.layer === 'user_statement').map((i) => i.evidence_id);
  const base = (rec: Recommendation, summary: string, purpose: string, impact: string, rebuild: string): Answer => ({
    answer_version: 'diag-answer/1',
    summary,
    purpose,
    recommendation: rec,
    conditions: [],
    deletion_impact: impact,
    rebuild_cost: rebuild,
    claims: [],
    gaps: [],
    alternatives: [],
    questions: [],
  });
  switch (sc) {
    case 'cache': {
      const a = base('reviewable', '这是浏览器的脚本编译缓存；浏览器关闭时可以进入计划审阅。', '浏览器把网页脚本预先编译后存在这里，用来加快再次打开网页的速度。', '下次打开常用网页时首次加载会稍慢；网页数据、登录状态和书签不在这个目录里。', '浏览器会在本机自动重建，不需要下载。');
      a.conditions = ['浏览器已关闭（执行前会再次检查）'];
      a.claims = [
        { text: '规则说明该缓存可由浏览器在本机重建', basis: 'rule', refs: [id('rule.matched')] },
        { text: '大小来自这次扫描的索引', basis: 'observation', refs: [id('observation.size')] },
      ];
      a.alternatives = [{ action: 'review_plan', text: '在计划页审阅这一项' }];
      return { answer: a, result: { model_recommendation: 'reviewable', effective_recommendation: 'reviewable', grounding: [{ index: 0, status: 'supported', note: null }, { index: 1, status: 'supported', note: null }], conflicts: [], sanitized: [] } };
    }
    case 'video': {
      const a = base('needs_context', answered ? '你说已经备份；在确认备份能打开、剪辑工程不再引用之前，仍建议保留。' : '这是一段个人拍摄的视频素材；在确认备份和工程引用前，建议先保留。', '看起来是相机或无人机拍摄的原始素材，剪辑软件的工程文件常常直接引用这类文件。', '删除后无法从本机找回；如果剪辑工程仍引用它，工程会提示素材丢失。', '无法重建，只能从备份恢复。');
      a.conditions = ['备份能正常打开播放', '剪辑工程不再引用这段素材'];
      a.claims = [
        { text: '没有清理规则覆盖这类个人文件', basis: 'observation', refs: [id('rule.none')] },
        { text: '文件体积大不代表它是垃圾', basis: 'inference', refs: [] },
        ...(answered ? [{ text: '你说已经备份到移动硬盘', basis: 'user_statement' as const, refs: user.slice(0, 1) }] : []),
      ];
      a.alternatives = [{ action: 'verify', text: '打开备份确认文件可读' }, { action: 'protect', text: '加入保护名单，避免以后误选' }];
      a.questions = answered ? [] : [{ question_id: 'q1', text: '这段素材是否已经备份到另一块磁盘或云端？', choices: ['已备份并能打开', '没有备份', '不确定'] }, { question_id: 'q2', text: '剪辑工程是否还在使用它？', choices: ['还在使用', '已完成导出', '不清楚'] }];
      return {
        answer: a,
        result: {
          model_recommendation: 'needs_context',
          effective_recommendation: 'needs_context',
          grounding: [{ index: 0, status: 'supported', note: null }, { index: 1, status: 'inference', note: '模型推测' }, ...(answered ? [{ index: 2, status: 'user_statement' as const, note: '你的陈述，CleanManager 未核验' }] : [])],
          conflicts: [],
          sanitized: [],
        },
      };
    }
    case 'model': {
      const a = base('reviewable', '这是本地模型权重；它受默认保护，建议通过所属应用管理。', '大语言模型的权重分片，运行本地模型时由应用加载。', '删除后需要重新下载数 GB；离线时无法使用该模型。共享的硬链接意味着删除这个名称可能不释放空间。', '需要网络重新下载，耗时取决于带宽。');
      a.claims = [
        { text: '删除后可以释放 13 GB 空间', basis: 'observation', refs: [id('observation.size')] },
        { text: '该文件有两个名称，可能与其他应用共享', basis: 'observation', refs: [id('observation.hard_links')] },
      ];
      a.alternatives = [{ action: 'app_management', text: '在所属应用里移除不用的模型' }, { action: 'review_plan', text: '直接删除' }];
      return {
        answer: a,
        result: {
          model_recommendation: 'reviewable',
          effective_recommendation: 'keep',
          grounding: [{ index: 0, status: 'unsupported', note: '可释放空间未知' }, { index: 1, status: 'supported', note: null }],
          conflicts: [{ code: 'protected_object', text: '对象受保护（protected:models）；保护优先于任何建议。' }, { code: 'unverified_reclaim_claim', text: '回答给出了可释放空间，但独占回收量未知。' }],
          sanitized: [],
        },
      };
    }
    case 'installer': {
      const a = base('reviewable', '这可能是已经安装过的软件的安装包；确认来源和是否还需要离线重装后再决定。', '安装程序本身，不是已安装应用的一部分。', '删除后如需重装要重新下载；少数软件修复或卸载时会要求原安装包。', '可从官网重新下载（如果版本仍提供）。');
      a.claims = [{ text: '看起来是安装程序', basis: 'inference', refs: [] }];
      a.alternatives = [{ action: 'review_plan', text: '删除它' }, { action: 'verify', text: '确认软件已安装且能重新下载' }];
      a.questions = [{ question_id: 'q1', text: '这个软件现在还安装在电脑上吗？', choices: ['已安装', '已卸载', '不确定'] }];
      return {
        answer: a,
        result: { model_recommendation: 'reviewable', effective_recommendation: 'needs_context', grounding: [{ index: 0, status: 'inference', note: '模型推测' }], conflicts: [{ code: 'not_a_candidate', text: '没有已启用规则把它列为清理候选，核心无法为它生成计划。' }], sanitized: [] },
      };
    }
    case 'database': {
      const a = base('reviewable', '这是应用的历史数据库。', '保存应用的浏览或使用历史。', '删除会丢失历史记录，应用可能重建一个空库。', '无法重建原有内容。');
      a.claims = [{ text: '这是可丢弃的临时数据库，可以删除', basis: 'inference', refs: [] }];
      a.alternatives = [{ action: 'review_plan', text: '[已移除：回答包含批准/授权说法；授权只能由你在计划审阅中完成]' }];
      return {
        answer: a,
        result: {
          model_recommendation: 'reviewable',
          effective_recommendation: 'keep',
          grounding: [{ index: 0, status: 'conflicts_with_facts', note: '该对象受保护' }],
          conflicts: [{ code: 'protected_object', text: '对象受保护（protected:database）；保护优先于任何建议。' }],
          sanitized: [{ field: 'alternatives[0].text', reason: 'approval_instruction' }],
        },
      };
    }
    case 'partial': {
      const a = base('reviewable', '这是项目构建输出，但有子范围没能读取；先补齐扫描再决定。', '编译器生成的中间文件和产物，可以重新构建。', '删除后下次构建会更久；未读取的子目录里是什么不清楚。', '重新构建需要几分钟到几十分钟。');
      a.claims = [{ text: '规则把它识别为构建输出', basis: 'rule', refs: [id('rule.matched')] }];
      return {
        answer: a,
        result: { model_recommendation: 'reviewable', effective_recommendation: 'needs_context', grounding: [{ index: 0, status: 'supported', note: null }], conflicts: [{ code: 'partial_evidence', text: '证据不完整（未读子范围、云端占位、弱身份或保护层未核对）。' }], sanitized: [] },
      };
    }
    default: {
      const a = base('needs_context', '没有规则说明这个对象的用途；需要你确认它从哪里来。', '无法仅凭名称和大小判断用途。', '不确定。', '不确定。');
      a.claims = [{ text: '没有清理规则匹配', basis: 'observation', refs: [id('rule.none')] }];
      a.questions = [{ question_id: 'q1', text: '你记得这个文件是怎么来的吗？', choices: ['自己创建的', '下载的', '不记得'] }];
      return { answer: a, result: { model_recommendation: 'needs_context', effective_recommendation: 'needs_context', grounding: [{ index: 0, status: 'supported', note: null }], conflicts: [], sanitized: [] } };
    }
  }
}

function handoffFor(sc: Scenario, entryId: string, answer: Answer | null, fresh: boolean): Handoff[] {
  const sugg = new Set<NextAction>(answer?.alternatives.map((a) => a.action) ?? []);
  const t = { scan_id: 's_preview_1', entry_id: entryId, candidate_id: null as string | null, app_instance_id: null as string | null };
  const protectedObj = sc === 'model' || sc === 'database';
  const plan = sc === 'cache' && fresh;
  const planReason = protectedObj
    ? `受保护（protected:${sc === 'model' ? 'models' : 'database'}），核心不会为它生成计划；可以保留或定位。`
    : sc === 'cache'
      ? fresh
        ? '生成只含这一个候选的新计划；授权与现场核验在计划页独立进行。'
        : '诊断未完成或已过期；请先重新核验。'
      : sc === 'partial'
        ? '核心当前不允许把该候选放入计划。'
        : '没有已启用规则的候选；本扩展不开放任意路径删除。';
  const rows: Handoff[] = [
    { action: 'review_plan', available: plan, reason: planReason, suggested_by_model: false, target: plan ? { ...t, candidate_id: 'c_preview_cache' } : null },
    { action: 'keep', available: true, reason: '不做任何操作，文件保持原状。', suggested_by_model: false, target: null },
    { action: 'protect', available: !protectedObj, reason: protectedObj ? '已受默认保护。' : '加入你的保护名单；以后的规则与策略都不会选中它。', suggested_by_model: false, target: t },
    { action: 'locate', available: true, reason: '在磁盘视图中定位（只读）。', suggested_by_model: false, target: t },
    { action: 'verify', available: true, reason: '按核实清单确认备份、引用与来源。', suggested_by_model: false, target: null },
    { action: 'app_management', available: sc === 'model', reason: sc === 'model' ? '在应用页查看关联应用（只读清单）。' : '没有安装记录关联到这个位置。', suggested_by_model: false, target: sc === 'model' ? { ...t, app_instance_id: 'app_model_runner' } : null },
    { action: 'archive', available: false, reason: 'CleanManager 不移动或归档文件；请使用你自己的备份工具。', suggested_by_model: false, target: null },
  ];
  rows.forEach((r) => (r.suggested_by_model = sugg.has(r.action)));
  return rows;
}

export function mockDiag(lookup: (entryId: string) => MockObject | null): DiagBackend & { seed(state: DiagState | 'unconfigured', scenario?: Scenario): string | null } {
  const finished = new Set<(e: DiagFinishedEvent) => void>();
  let providers: ProviderStatus[] = [
    {
      provider_id: 'anthropic-default',
      label: 'Anthropic API',
      kind: 'anthropic',
      mode: 'api',
      endpoint: 'https://api.anthropic.com/v1/messages',
      executable: null,
      model: 'claude-opus-5-5',
      credential: 'present',
      support: { state: 'supported', reason: null },
      updated_at: '2026-10-15T08:00:00Z',
    },
  ];
  let defaultId: string | null = 'anthropic-default';
  const previews = new Map<string, { p: Preview; sc: Scenario; parent: Diagnosis | null }>();
  const diags = new Map<string, { d: Diagnosis; sc: Scenario; timer: number | null }>();
  let n = 0;

  const status = (): DiagStatus => ({
    configured: providers.some((p) => p.support.state !== 'unsupported' && p.credential !== 'absent'),
    credential_store: 'windows_credential_manager',
    providers: providers.map((p) => ({ ...p })),
    default_provider_id: defaultId,
    key_prompt_available: true,
  });
  const provider = (id: string | null) => {
    const p = providers.find((x) => x.provider_id === (id ?? defaultId));
    if (!p) throw err('unsupported', 'diag_not_configured', 'no AI provider is configured; add one in Settings > AI 诊断 (local features stay available)');
    if (p.support.state === 'unsupported') throw err('unsupported', 'diag_unsupported', p.support.reason ?? 'unsupported');
    if (p.credential === 'absent' && p.kind === 'anthropic') throw err('unsupported', 'diag_not_configured', `no API key is stored for ${p.provider_id}`);
    return p;
  };
  const objectFor = (entryId: string): MockObject => lookup(entryId) ?? { name: 'Code Cache', path: 'C:\\Users\\Example\\AppData\\Local\\ExampleBrowser\\Code Cache', kind: 'directory', logical: '314572800', unknownDesc: 0 };

  const makePreview = (scanId: string, entryId: string, origin: Origin, providerId: string | null, d: Disclosure, question: string | null, statements: { statement_id: string; question_id: string | null; text: string; at: string }[], parent: Diagnosis | null): Preview => {
    const p = provider(providerId);
    const o = objectFor(entryId);
    const sc = scenarioOf(o);
    const ev = evidenceFor(sc, o, d, statements.map((s) => s.text));
    const user = JSON.stringify({ answer_format: 'diag-answer/1', evidence: ev, language: 'zh-CN', question, task: 'explain_one_object', template: 'diag-prompt/1', user_statements: statements.map(({ statement_id, question_id, text }) => ({ question_id, statement_id, text })) });
    const body = JSON.stringify({ max_tokens: 16000, messages: [{ content: user, role: 'user' }], model: p.model, output_config: { effort: 'medium' }, system: 'You explain ONE file or folder to a person who is deciding whether to keep it. …（固定系统提示，diag-prompt/1）' });
    n += 1;
    const now = new Date();
    const preview: Preview = {
      preview_id: `pv_${hex(n)}`,
      state: 'preparing',
      request: {
        contract: 'diag/1',
        object: { scan_id: scanId, revision: 1, entry_id: entryId, origin, candidate_id: sc === 'cache' ? 'c_preview_cache' : null, app_instance_id: sc === 'model' ? 'app_model_runner' : null },
        observed_at: T_SCAN,
        observation_fingerprint: hex(n + 7, 64),
        disclosure: d,
        question,
        statements,
        parent_diagnosis_id: parent?.diagnosis_id ?? null,
        prompt_template: 'diag-prompt/1',
      },
      evidence: ev,
      destination: { provider_id: p.provider_id, label: p.label, kind: p.kind, mode: p.mode, endpoint: p.endpoint, executable: p.executable, model: p.model, template: 'diag-prompt/1' },
      payload: {
        transport: p.endpoint?.startsWith('http://') ? 'http_post_loopback' : 'https_post',
        target: p.endpoint ?? '',
        headers: ['content-type: application/json', 'anthropic-version: 2023-06-01', `x-api-key: <stored in the system credential store as CleanManager:diag:${p.provider_id}>`],
        argv: [],
        body,
      },
      payload_digest: hex(n + 3, 64),
      payload_bytes: new TextEncoder().encode(body).length,
      contains_user_additions: !!question || statements.length > 0,
      created_at: now.toISOString().replace(/\.\d+Z$/, 'Z'),
      expires_at: new Date(now.getTime() + 600_000).toISOString().replace(/\.\d+Z$/, 'Z'),
    };
    previews.set(preview.preview_id, { p: preview, sc, parent });
    return preview;
  };

  const finish = (id: string, state: DiagState) => {
    const rec = diags.get(id);
    if (!rec || rec.d.state !== 'running') return;
    const now = new Date().toISOString().replace(/\.\d+Z$/, 'Z');
    const d = rec.d;
    if (state === 'completed' || state === 'expired') {
      const answered = d.request.statements.length > 0;
      const { answer, result } = answerFor(rec.sc, d.evidence, answered);
      d.result = { answer, ...result };
      d.freshness = state === 'expired' ? { state: 'stale', checked_at: now, reasons: ['observation_changed'] } : { state: 'current', checked_at: now, reasons: [] };
      d.handoff = handoffFor(rec.sc, d.request.object.entry_id, answer, state === 'completed');
    } else if (state === 'failed') {
      d.failure = { code: 'timeout', message: '提供方在时限内没有返回完整回答。可以按新请求重试；没有改用其他提供方。', retryable: true };
    }
    d.state = state;
    d.finished_at = now;
    d.updated_at = now;
    finished.forEach((cb) => cb({ diagnosis_id: id, state }));
  };

  const create = (pv: { p: Preview; sc: Scenario; parent: Diagnosis | null }): Diagnosis => {
    n += 1;
    const now = new Date();
    const iso = now.toISOString().replace(/\.\d+Z$/, 'Z');
    const d: Diagnosis = {
      diagnosis_id: `dg_${hex(n + 100)}`,
      state: 'running',
      request: pv.p.request,
      evidence: pv.p.evidence,
      facts: { entry_id: pv.p.request.object.entry_id, kind: pv.p.evidence.subject.kind, protection: pv.sc === 'model' ? 'protected:models' : pv.sc === 'database' ? 'protected:database' : null, candidate_id: pv.p.request.object.candidate_id, plannable: pv.sc === 'cache', app_instance_ids: pv.sc === 'model' ? ['app_model_runner'] : [] },
      destination: pv.p.destination,
      payload_digest: pv.p.payload_digest,
      created_at: iso,
      updated_at: iso,
      finished_at: null,
      expires_at: new Date(now.getTime() + 86_400_000).toISOString().replace(/\.\d+Z$/, 'Z'),
      retention: 'session',
      retain_until: new Date(now.getTime() + 86_400_000).toISOString().replace(/\.\d+Z$/, 'Z'),
      result: null,
      failure: null,
      freshness: { state: 'unknown', checked_at: iso, reasons: [] },
      handoff: handoffFor(pv.sc, pv.p.request.object.entry_id, null, false),
    };
    diags.set(d.diagnosis_id, { d, sc: pv.sc, timer: null });
    return d;
  };

  const summary = (d: Diagnosis): DiagSummary => ({
    diagnosis_id: d.diagnosis_id,
    state: d.state,
    object: d.request.object,
    subject_name: d.evidence.subject.name,
    provider_id: d.destination.provider_id,
    model: d.destination.model,
    effective_recommendation: d.result?.effective_recommendation ?? null,
    created_at: d.created_at,
    retention: d.retention,
    parent_diagnosis_id: d.request.parent_diagnosis_id,
  });
  const get = (id: string) => {
    const r = diags.get(id);
    if (!r) throw err('invalid_argument', 'diag_not_found', `not found: diagnosis ${id}`);
    return r;
  };
  const clone = (d: Diagnosis): Diagnosis => JSON.parse(JSON.stringify(d)) as Diagnosis;

  return {
    seed(state, scenario = 'cache') {
      if (state === 'unconfigured') {
        providers = [];
        defaultId = null;
        return null;
      }
      const entry = scenario === 'video' ? '3' : 'c';
      const pv = makePreview('s_preview_1', entry, scenario === 'cache' ? 'cleanup' : 'disk', null, { full_path: false, child_summary: false }, null, [], null);
      const d = create(previews.get(pv.preview_id)!);
      if (state !== 'running') finish(d.diagnosis_id, state === 'cancelled' ? 'cancelled' : state);
      return d.diagnosis_id;
    },
    async diagStatus() {
      return status();
    },
    async diagSaveProvider(input) {
      if (!/^https:\/\//.test(input.base_url) && !/^http:\/\/(127\.0\.0\.1|localhost|\[::1\])/.test(input.base_url)) throw err('invalid_argument', 'invalid_argument', 'invalid argument: the endpoint must use https:// (plain http:// is allowed only for 127.0.0.1, [::1] or localhost)');
      const base = input.base_url.replace(/\/+$/, '');
      const p: ProviderStatus = {
        provider_id: input.provider_id,
        label: input.label,
        kind: input.kind,
        mode: 'api',
        endpoint: input.kind === 'anthropic' ? `${base}/v1/messages` : `${base}/chat/completions`,
        executable: null,
        model: input.model,
        credential: input.kind === 'anthropic' ? 'absent' : 'absent',
        support: { state: 'supported', reason: null },
        updated_at: new Date().toISOString(),
      };
      providers = [...providers.filter((x) => x.provider_id !== p.provider_id), p];
      if (input.make_default || !defaultId) defaultId = p.provider_id;
      return status();
    },
    async diagChooseCli(kind, model, makeDefault) {
      const p: ProviderStatus = {
        provider_id: kind === 'codex_cli' ? 'codex-cli' : 'claude-code-cli',
        label: kind === 'codex_cli' ? 'Codex CLI' : 'Claude Code CLI',
        kind,
        mode: 'cli',
        endpoint: null,
        executable: kind === 'codex_cli' ? 'C:\\Users\\Example\\AppData\\Roaming\\npm\\node_modules\\@openai\\codex\\…\\codex.exe' : 'C:\\Users\\Example\\AppData\\Roaming\\npm\\node_modules\\@anthropic-ai\\claude-code\\bin\\claude.exe',
        model: model || 'default',
        credential: 'not_required',
        support:
          kind === 'codex_cli'
            ? { state: 'unsupported', reason: 'codex exec cannot disable its shell tool; even the read-only sandbox lets the model read any file, so it is not started' }
            : { state: 'unverified', reason: 'checked with --help when a diagnosis is prepared' },
        updated_at: new Date().toISOString(),
      };
      providers = [...providers.filter((x) => x.provider_id !== p.provider_id), p];
      if (makeDefault || !defaultId) defaultId = p.provider_id;
      return status();
    },
    async diagRemoveProvider(id) {
      providers = providers.filter((p) => p.provider_id !== id);
      if (defaultId === id) defaultId = providers[0]?.provider_id ?? null;
      return status();
    },
    async diagSetKey(id) {
      const p = providers.find((x) => x.provider_id === id);
      if (p) p.credential = 'present';
      return true;
    },
    async diagForgetKey(id) {
      const p = providers.find((x) => x.provider_id === id);
      if (p && p.mode === 'api') p.credential = 'absent';
      return status();
    },
    async diagProbe(id) {
      const p = providers.find((x) => x.provider_id === id);
      const ok = !!p && p.support.state !== 'unsupported' && (p.credential !== 'absent' || p.kind === 'openai_compatible');
      return {
        provider_id: id,
        ok,
        detail: ok ? `已连接；模型 ${p!.model} 可用。` : (p?.support.reason ?? '没有保存 API Key；请在设置中添加。'),
        failure: ok ? null : { code: p?.support.state === 'unsupported' ? 'unsupported_mode' : 'credential_missing', message: p?.support.reason ?? '没有保存 API Key。', retryable: false },
        checked_at: new Date().toISOString(),
        sent_file_data: false,
      };
    },
    async diagPrepare(scanId, entryId, origin, providerId, disclosure, question) {
      return makePreview(scanId, entryId, origin, providerId, disclosure, question, [], null);
    },
    async diagFollowUp(id, questionId, answer) {
      const parent = get(id).d;
      const statements = [...parent.request.statements, { statement_id: `u${parent.request.statements.length + 1}`, question_id: questionId, text: answer, at: new Date().toISOString() }];
      return makePreview(parent.request.object.scan_id, parent.request.object.entry_id, parent.request.object.origin, parent.destination.provider_id, parent.request.disclosure, parent.request.question, statements, parent);
    },
    async diagDiscard(id) {
      return previews.delete(id);
    },
    async diagConfirm(previewId, digest) {
      const pv = previews.get(previewId);
      if (!pv) throw err('conflict', 'diag_preview_changed', 'the preview expired or was already used; prepare it again');
      if (pv.p.payload_digest !== digest) throw err('conflict', 'diag_preview_changed', 'the confirmed digest does not match this preview');
      previews.delete(previewId);
      const d = create(pv);
      const rec = diags.get(d.diagnosis_id)!;
      rec.timer = window.setTimeout(() => finish(d.diagnosis_id, 'completed'), 1600);
      return clone(d);
    },
    async diagCancel(id) {
      const r = get(id);
      if (r.d.state === 'running') {
        if (r.timer) window.clearTimeout(r.timer);
        finish(id, 'cancelled');
      }
      return clone(r.d);
    },
    async diagGet(id) {
      return clone(get(id).d);
    },
    async diagList() {
      const items = [...diags.values()].map((r) => summary(r.d)).sort((a, b) => (a.created_at < b.created_at ? 1 : -1));
      return { items, next_cursor: null };
    },
    async diagSave(id) {
      const r = get(id);
      r.d.retention = 'saved';
      r.d.retain_until = null;
      return clone(r.d);
    },
    async diagRemove(id) {
      return diags.delete(id);
    },
    async diagExport(id) {
      const d = get(id).d;
      return JSON.stringify({ contract: 'diag/1', kind: 'diagnosis_export', includes: ['object_reference', 'redacted_evidence', 'provider_and_model', 'explanation', 'grounding', 'handoff'], excludes: ['credentials', 'file_contents', 'operation_audit'], diagnosis: d }, null, 2);
    },
    async diagHandoff(id, action) {
      const d = get(id).d;
      const h = d.handoff.find((x) => x.action === action);
      if (!h || !h.available) throw err('policy_denied', 'handoff_unavailable', h?.reason ?? 'unknown next step');
      const t = h.target;
      if (action === 'review_plan') return { action, outcome: 'plan', plan_id: 'p_preview_review', scan_id: null, entry_id: null, app_instance_id: null };
      if (action === 'protect') return { action, outcome: 'protected', plan_id: null, scan_id: t?.scan_id ?? null, entry_id: t?.entry_id ?? null, app_instance_id: null };
      if (action === 'locate') return { action, outcome: 'disk', plan_id: null, scan_id: t?.scan_id ?? null, entry_id: t?.entry_id ?? null, app_instance_id: null };
      if (action === 'app_management') return { action, outcome: 'apps', plan_id: null, scan_id: t?.scan_id ?? null, entry_id: null, app_instance_id: t?.app_instance_id ?? null };
      return { action, outcome: 'none', plan_id: null, scan_id: null, entry_id: null, app_instance_id: null };
    },
    async onDiagFinished(cb) {
      finished.add(cb);
      return () => finished.delete(cb);
    },
  };
}
