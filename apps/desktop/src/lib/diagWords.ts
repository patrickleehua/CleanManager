// Words for the optional AI diagnosis. No label says "safe": a recommendation is conditional
// and the core decides every action. Every state has a word; colour never carries it alone.

import type { ClaimStatus, DiagState, Layer, NextAction, ProviderKind, Recommendation } from './diagTypes';
import { formatNumber, t } from '../i18n';

export const REC_LABEL: Record<Recommendation, string> = {
  get reviewable() { return t('diagWords.recLabel.reviewable'); },
  get needs_context() { return t('diagWords.recLabel.needs_context'); },
  get keep() { return t('diagWords.recLabel.keep'); },
  get unsupported() { return t('diagWords.recLabel.unsupported'); },
};

export const REC_EXPLAIN: Record<Recommendation, string> = {
  get reviewable() { return t('diagWords.recExplain.reviewable'); },
  get needs_context() { return t('diagWords.recExplain.needs_context'); },
  get keep() { return t('diagWords.recExplain.keep'); },
  get unsupported() { return t('diagWords.recExplain.unsupported'); },
};

export const STATE_LABEL: Record<DiagState, string> = {
  get running() { return t('diagWords.stateLabel.running'); },
  get completed() { return t('diagWords.stateLabel.completed'); },
  get cancelled() { return t('diagWords.stateLabel.cancelled'); },
  get failed() { return t('diagWords.stateLabel.failed'); },
  get expired() { return t('diagWords.stateLabel.expired'); },
};

export const LAYER_LABEL: Record<Layer, string> = {
  get observation() { return t('diagWords.layerLabel.observation'); },
  get rule() { return t('diagWords.layerLabel.rule'); },
  get protection() { return t('diagWords.layerLabel.protection'); },
  get ownership() { return t('diagWords.layerLabel.ownership'); },
  get coverage() { return t('diagWords.layerLabel.coverage'); },
  get user_statement() { return t('diagWords.layerLabel.user_statement'); },
};

export const CLAIM_LABEL: Record<ClaimStatus, string> = {
  get supported() { return t('diagWords.claimLabel.supported'); },
  get user_statement() { return t('diagWords.claimLabel.user_statement'); },
  get inference() { return t('diagWords.claimLabel.inference'); },
  get unsupported() { return t('diagWords.claimLabel.unsupported'); },
  get conflicts_with_facts() { return t('diagWords.claimLabel.conflicts_with_facts'); },
};

export const ACTION_LABEL: Record<NextAction, string> = {
  get review_plan() { return t('diagWords.actionLabel.review_plan'); },
  get keep() { return t('diagWords.actionLabel.keep'); },
  get protect() { return t('diagWords.actionLabel.protect'); },
  get locate() { return t('diagWords.actionLabel.locate'); },
  get verify() { return t('diagWords.actionLabel.verify'); },
  get app_management() { return t('diagWords.actionLabel.app_management'); },
  get archive() { return t('diagWords.actionLabel.archive'); },
};

/** Short verbs for the next-step buttons (the row already names the step). */
export const ACTION_BUTTON: Record<NextAction, string> = {
  get review_plan() { return t('diagWords.actionButton.review_plan'); },
  get keep() { return t('diagWords.actionButton.keep'); },
  get protect() { return t('diagWords.actionButton.protect'); },
  get locate() { return t('diagWords.actionButton.locate'); },
  get verify() { return t('diagWords.actionButton.verify'); },
  get app_management() { return t('diagWords.actionButton.app_management'); },
  get archive() { return t('diagWords.actionButton.archive'); },
};

export const PROVIDER_LABEL: Record<ProviderKind, string> = {
  anthropic: 'Anthropic API',
  get openai_compatible() { return t('diagWords.providerLabel.openai_compatible'); },
  claude_code_cli: 'Claude Code CLI',
  codex_cli: 'Codex CLI',
};

export const CREDENTIAL_LABEL: Record<string, string> = {
  get present() { return t('diagWords.credentialLabel.present'); },
  get absent() { return t('diagWords.credentialLabel.absent'); },
  get not_required() { return t('diagWords.credentialLabel.not_required'); },
  get unavailable() { return t('diagWords.credentialLabel.unavailable'); },
};

export const SUPPORT_LABEL: Record<string, string> = {
  get supported() { return t('diagWords.supportLabel.supported'); },
  get unsupported() { return t('diagWords.supportLabel.unsupported'); },
  get unverified() { return t('diagWords.supportLabel.unverified'); },
};

export const EXCLUDED_LABEL: Record<string, string> = {
  get file_contents() { return t('diagWords.excludedLabel.file_contents'); },
  get directory_tree() { return t('diagWords.excludedLabel.directory_tree'); },
  get credentials() { return t('diagWords.excludedLabel.credentials'); },
  get sessions() { return t('diagWords.excludedLabel.sessions'); },
  get cloud_hydration() { return t('diagWords.excludedLabel.cloud_hydration'); },
};

export const REDACTION_LABEL: Record<string, string> = {
  get user_profile() { return t('diagWords.redactionLabel.user_profile'); },
  get email() { return t('diagWords.redactionLabel.email'); },
  get long_number() { return t('diagWords.redactionLabel.long_number'); },
  get secret_like() { return t('diagWords.redactionLabel.secret_like'); },
  get control_characters() { return t('diagWords.redactionLabel.control_characters'); },
  get instruction_like() { return t('diagWords.redactionLabel.instruction_like'); },
  get truncated() { return t('diagWords.redactionLabel.truncated'); },
  get parent_components() { return t('diagWords.redactionLabel.parent_components'); },
};

export const FAILURE_HINT: Record<string, string> = {
  get not_configured() { return t('diagWords.failureHint.not_configured'); },
  get credential_missing() { return t('diagWords.failureHint.credential_missing'); },
  get credential_store_unavailable() { return t('diagWords.failureHint.credential_store_unavailable'); },
  get auth_failed() { return t('diagWords.failureHint.auth_failed'); },
  get model_unavailable() { return t('diagWords.failureHint.model_unavailable'); },
  get rate_limited() { return t('diagWords.failureHint.rate_limited'); },
  get provider_error() { return t('diagWords.failureHint.provider_error'); },
  get refused() { return t('diagWords.failureHint.refused'); },
  get timeout() { return t('diagWords.failureHint.timeout'); },
  get network() { return t('diagWords.failureHint.network'); },
  get output_too_large() { return t('diagWords.failureHint.output_too_large'); },
  get invalid_structure() { return t('diagWords.failureHint.invalid_structure'); },
  get unsupported_mode() { return t('diagWords.failureHint.unsupported_mode'); },
  get process_failed() { return t('diagWords.failureHint.process_failed'); },
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
  return t('diagWords.pKbTolocalestringBytes', { p: kb < 10 ? kb.toFixed(1) : Math.round(kb), toLocaleString: formatNumber(bytes) });
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
