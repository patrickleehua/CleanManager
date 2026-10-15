// Words for the optional AI diagnosis. No label says "safe": a recommendation is conditional
// and the core decides every action. Every state has a word; colour never carries it alone.

import type { ClaimStatus, DiagState, Layer, NextAction, ProviderKind, Recommendation } from './diagTypes';

export const REC_LABEL: Record<Recommendation, string> = {
  reviewable: '可以进入计划审阅',
  needs_context: '需要补充信息',
  keep: '建议保留',
  unsupported: '无法判断',
};

export const REC_EXPLAIN: Record<Recommendation, string> = {
  reviewable: '核心有对应的清理候选。可以生成只含这一项的计划，在计划页审阅、批准，执行前还会再次核验。',
  needs_context: '现有依据不足以决定。回答下面的问题或按核实清单确认后再决定；在那之前文件保持原状。',
  keep: '受保护或删除代价高。建议保留，可以加入保护名单或在磁盘视图中定位。',
  unsupported: '诊断无法给出可用的判断。本机事实仍然有效，可以按核实清单自行确认。',
};

export const STATE_LABEL: Record<DiagState, string> = {
  running: '正在诊断',
  completed: '已完成',
  cancelled: '已取消',
  failed: '失败',
  expired: '已过期',
};

export const LAYER_LABEL: Record<Layer, string> = {
  observation: '本机观察',
  rule: '规则资料',
  protection: '保护',
  ownership: '应用归属',
  coverage: '覆盖',
  user_statement: '你的陈述',
};

export const CLAIM_LABEL: Record<ClaimStatus, string> = {
  supported: '有本机依据',
  user_statement: '你的陈述（未核验）',
  inference: '模型推测',
  unsupported: '缺少依据',
  conflicts_with_facts: '与本机事实冲突',
};

export const ACTION_LABEL: Record<NextAction, string> = {
  review_plan: '生成只含这一项的计划',
  keep: '保留，不做任何操作',
  protect: '加入保护名单',
  locate: '在磁盘视图中定位',
  verify: '按核实清单确认',
  app_management: '查看关联应用',
  archive: '移动或归档',
};

/** Short verbs for the next-step buttons (the row already names the step). */
export const ACTION_BUTTON: Record<NextAction, string> = {
  review_plan: '生成计划',
  keep: '保留',
  protect: '保护',
  locate: '定位',
  verify: '查看清单',
  app_management: '打开应用页',
  archive: '不可用',
};

export const PROVIDER_LABEL: Record<ProviderKind, string> = {
  anthropic: 'Anthropic API',
  openai_compatible: 'OpenAI 兼容端点',
  claude_code_cli: 'Claude Code CLI',
  codex_cli: 'Codex CLI',
};

export const CREDENTIAL_LABEL: Record<string, string> = {
  present: '密钥已保存在系统凭据中',
  absent: '没有保存密钥',
  not_required: '使用 CLI 自己的登录',
  unavailable: '系统凭据存储不可用',
};

export const SUPPORT_LABEL: Record<string, string> = {
  supported: '可用',
  unsupported: '不支持',
  unverified: '使用前检查',
};

export const EXCLUDED_LABEL: Record<string, string> = {
  file_contents: '文件内容',
  directory_tree: '整盘或整个目录树',
  credentials: '密钥、令牌与登录信息',
  sessions: '会话与聊天记录',
  cloud_hydration: '下载云端占位文件',
};

export const REDACTION_LABEL: Record<string, string> = {
  user_profile: '用户目录替换为 %USERPROFILE%',
  email: '邮箱地址',
  long_number: '长数字（账号、电话等）',
  secret_like: '类似密钥的字符串',
  control_characters: '控制字符',
  instruction_like: '疑似指令文本',
  truncated: '超长部分',
  parent_components: '较深的上级目录',
};

export const FAILURE_HINT: Record<string, string> = {
  not_configured: '还没有配置提供方。',
  credential_missing: '没有保存 API Key。',
  credential_store_unavailable: '系统凭据存储不可用。',
  auth_failed: '提供方拒绝了凭据。',
  model_unavailable: '提供方找不到这个模型或端点。',
  rate_limited: '提供方限流，可以稍后重试。',
  provider_error: '提供方返回错误。',
  refused: '提供方拒绝回答这个请求。',
  timeout: '在时限内没有收到完整回答。',
  network: '网络连接失败。',
  output_too_large: '回答超过长度上限。',
  invalid_structure: '回答格式无效，没有采用其中任何内容。',
  unsupported_mode: '这个接入方式无法被约束，没有启动。',
  process_failed: 'CLI 进程失败。',
};

export function recTone(r: Recommendation): 'good' | 'warn' | 'muted' | 'busy' {
  return r === 'reviewable' ? 'good' : r === 'needs_context' ? 'warn' : r === 'keep' ? 'busy' : 'muted';
}

export function stateTone(s: DiagState): 'good' | 'warn' | 'bad' | 'busy' | 'muted' {
  return s === 'running' ? 'busy' : s === 'completed' ? 'good' : s === 'failed' ? 'bad' : s === 'expired' ? 'warn' : 'muted';
}

/** "1.2 KB（1,234 字节）" for the payload size line. */
export function payloadSize(bytes: number): string {
  const kb = bytes / 1024;
  return `${kb < 10 ? kb.toFixed(1) : Math.round(kb)} KB（${bytes.toLocaleString('en-US')} 字节）`;
}

/** Readable form of the exact JSON body: the same bytes, indented (display only). */
export function prettyBody(body: string): string {
  try {
    const v = JSON.parse(body) as Record<string, unknown>;
    // The Anthropic/OpenAI body embeds the user message as a JSON string; expand it for reading.
    const expand = (m: unknown) => {
      if (m && typeof m === 'object' && 'content' in (m as Record<string, unknown>)) {
        const c = (m as Record<string, unknown>).content;
        if (typeof c === 'string' && c.startsWith('{')) {
          try {
            (m as Record<string, unknown>).content = JSON.parse(c);
          } catch {
            /* keep the string */
          }
        }
      }
    };
    if (Array.isArray(v.messages)) v.messages.forEach(expand);
    return JSON.stringify(v, null, 2);
  } catch {
    return body;
  }
}

/** Evidence ids cited by a claim that exist in the package (others are dropped by the core). */
export function citedLayers(refs: string[], items: { evidence_id: string; layer: Layer }[]): Layer[] {
  return refs.map((r) => items.find((i) => i.evidence_id === r)?.layer).filter((l): l is Layer => !!l);
}
