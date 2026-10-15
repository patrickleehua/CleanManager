//! Optional AI diagnosis commands (task 15.7, AID-01/02/06/07/09/11/12/13).
//!
//! Safety properties of this IPC surface:
//! - The WebView names objects by scan id + entry id, previews by preview id and diagnoses by
//!   diagnosis id. It never sends a path, a payload body or a credential. A CLI executable is
//!   chosen in a native file dialog in this process; an API endpoint is a URL validated here.
//! - API keys are entered in the system credential prompt (Windows Security dialog) opened by
//!   this process and written straight to Credential Manager; no command returns a key.
//! - `diag_prepare` sends nothing. `diag_confirm` must name the preview and the digest shown to
//!   the person; the run happens on a worker thread and ends in one `diag-finished` event.
//!   Navigation never cancels a run; only `diag_cancel` does.
//! - `diag_handoff` only acts through existing core flows: a plan with exactly the diagnosed
//!   candidate (still unapproved), the protection list, or navigation hints. Model text cannot
//!   enable a step; availability is recomputed from core facts on every call.

use std::sync::Arc;

use cm_app::diag::{ConfigStatus, DiagAppError, PrepareRequest, ProbeReport, ProviderConfig, ScanContext, Service, current_observation};
use cm_core::diag::ground::CurrentObservation;
use cm_core::diag::{Diagnosis, DiagnosisSummary, Disclosure, NextAction, Origin, Preview, ProviderKind};
use cm_core::error::ErrorCode;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tauri_plugin_dialog::DialogExt;

use crate::args;
use crate::commands::{St, blocking};
use crate::dto::{IpcError, IpcResult};
use crate::host::{AppState, lock};

pub const EVENT_FINISHED: &str = "diag-finished";

impl From<DiagAppError> for IpcError {
    fn from(e: DiagAppError) -> Self {
        IpcError::new(e.code(), e.reason(), e.to_string())
    }
}

fn service(s: &AppState) -> IpcResult<Arc<Service>> {
    s.diag
        .clone()
        .ok_or_else(|| IpcError::new(ErrorCode::Unsupported, "diag_not_configured", "No per-user store directory, so AI diagnosis settings cannot be kept"))
}

fn preview_id(s: &str) -> IpcResult<String> {
    let id = args::record_id("preview", s)?;
    if !cm_core::diag::valid_preview_id(&id) {
        return Err(IpcError::new(ErrorCode::InvalidArgument, "invalid_argument", "malformed preview id"));
    }
    Ok(id)
}

fn diagnosis_id(s: &str) -> IpcResult<String> {
    let id = args::record_id("diagnosis", s)?;
    if !cm_core::diag::valid_diagnosis_id(&id) {
        return Err(IpcError::new(ErrorCode::InvalidArgument, "invalid_argument", "malformed diagnosis id"));
    }
    Ok(id)
}

fn provider_id(s: &str) -> IpcResult<String> {
    cm_platform::credentials::target_for(s).map_err(|_| IpcError::new(ErrorCode::InvalidArgument, "invalid_argument", "provider id must be 1-40 of a-z, 0-9, -"))?;
    Ok(s.to_owned())
}

fn bounded_text(what: &str, s: Option<String>, max: usize) -> IpcResult<Option<String>> {
    match s.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()) {
        None => Ok(None),
        Some(t) if t.chars().count() > max || t.chars().any(|c| c.is_control() && c != '\n') => {
            Err(IpcError::new(ErrorCode::InvalidArgument, "invalid_argument", format!("{what} must be at most {max} characters")))
        }
        Some(t) => Ok(Some(t)),
    }
}

// ---------------------------------------------------------------------------------------------
// Configuration

#[derive(Debug, Clone, Serialize)]
pub struct DiagStatusDto {
    #[serde(flatten)]
    pub status: ConfigStatus,
    /// The API key prompt and store exist on this platform build.
    pub key_prompt_available: bool,
}

fn status_dto(svc: &Service) -> IpcResult<DiagStatusDto> {
    let status = svc.status().map_err(IpcError::from)?;
    Ok(DiagStatusDto { status, key_prompt_available: cfg!(windows) })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn diag_status(state: St<'_>) -> IpcResult<DiagStatusDto> {
    let s = state.inner().clone();
    blocking(move || status_dto(&*service(&s)?)).await
}

/// API provider settings from the WebView. No executable (CLI paths come from the dialog).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiProviderInput {
    pub provider_id: String,
    pub label: String,
    /// `anthropic` or `openai_compatible`.
    pub kind: String,
    pub base_url: String,
    pub model: String,
    pub make_default: bool,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn diag_save_provider(state: St<'_>, input: ApiProviderInput) -> IpcResult<DiagStatusDto> {
    let s = state.inner().clone();
    blocking(move || {
        let svc = service(&s)?;
        let kind = match input.kind.as_str() {
            "anthropic" => ProviderKind::Anthropic,
            "openai_compatible" => ProviderKind::OpenaiCompatible,
            _ => return Err(IpcError::new(ErrorCode::InvalidArgument, "invalid_argument", "kind must be anthropic or openai_compatible")),
        };
        let p = ProviderConfig {
            provider_id: provider_id(&input.provider_id)?,
            label: input.label,
            kind,
            base_url: Some(input.base_url.trim().to_owned()),
            model: input.model.trim().to_owned(),
            executable: None,
            updated_at: String::new(),
        };
        svc.save_provider(p, input.make_default)?;
        status_dto(&svc)
    })
    .await
}

/// Pick a CLI executable in a native dialog and save it as a CLI provider. Codex is saved but
/// reported unsupported; nothing is executed here.
#[tauri::command(rename_all = "snake_case")]
pub async fn diag_choose_cli(app: AppHandle, state: St<'_>, kind: String, model: String, make_default: bool) -> IpcResult<Option<DiagStatusDto>> {
    let s = state.inner().clone();
    blocking(move || {
        let svc = service(&s)?;
        let (kind, id, label) = match kind.as_str() {
            "claude_code_cli" => (ProviderKind::ClaudeCodeCli, "claude-code-cli", "Claude Code CLI"),
            "codex_cli" => (ProviderKind::CodexCli, "codex-cli", "Codex CLI"),
            _ => return Err(IpcError::new(ErrorCode::InvalidArgument, "invalid_argument", "kind must be claude_code_cli or codex_cli")),
        };
        let mut dialog = app.dialog().file().set_title("选择 CLI 可执行文件（.exe）");
        if cfg!(windows) {
            dialog = dialog.add_filter("可执行文件", &["exe"]);
        }
        let Some(picked) = dialog.blocking_pick_file() else { return Ok(None) };
        let path = picked.into_path().map_err(|e| IpcError::new(ErrorCode::InvalidArgument, "invalid_argument", format!("unsupported file location: {e}")))?;
        cm_app::diag::cli::validate_executable(&path).map_err(|e| IpcError::new(ErrorCode::InvalidArgument, "cli_rejected", e))?;
        let p = ProviderConfig {
            provider_id: id.into(),
            label: label.into(),
            kind,
            base_url: None,
            model: if model.trim().is_empty() { "default".into() } else { model.trim().to_owned() },
            executable: Some(path.to_string_lossy().into_owned()),
            updated_at: String::new(),
        };
        svc.save_provider(p, make_default)?;
        Ok(Some(status_dto(&svc)?))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn diag_remove_provider(state: St<'_>, provider_id: String) -> IpcResult<DiagStatusDto> {
    let s = state.inner().clone();
    blocking(move || {
        let svc = service(&s)?;
        svc.remove_provider(&self::provider_id(&provider_id)?)?;
        status_dto(&svc)
    })
    .await
}

/// Open the system credential prompt and store the key it returns. Returns `false` when the
/// person cancels. The key never passes through the WebView.
#[tauri::command(rename_all = "snake_case")]
pub async fn diag_set_key(state: St<'_>, provider_id: String) -> IpcResult<bool> {
    let s = state.inner().clone();
    blocking(move || {
        let svc = service(&s)?;
        let id = self::provider_id(&provider_id)?;
        svc.provider(Some(&id))?;
        let secret = cm_platform::credentials::prompt_for_secret(
            "CleanManager AI 诊断",
            &format!("在“密码”栏粘贴 {id} 的 API Key（用户名可填任意内容）。它只保存在 Windows 凭据管理器中。"),
        )
        .map_err(|e| IpcError::new(ErrorCode::Unsupported, "credential_store", e.to_string()))?;
        match secret {
            None => Ok(false),
            Some(k) => {
                svc.store_key(&id, &k)?;
                Ok(true)
            }
        }
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn diag_forget_key(state: St<'_>, provider_id: String) -> IpcResult<DiagStatusDto> {
    let s = state.inner().clone();
    blocking(move || {
        let svc = service(&s)?;
        svc.forget_key(&self::provider_id(&provider_id)?)?;
        status_dto(&svc)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn diag_probe(state: St<'_>, provider_id: String) -> IpcResult<ProbeReport> {
    let s = state.inner().clone();
    blocking(move || Ok(service(&s)?.probe(&self::provider_id(&provider_id)?)?)).await
}

// ---------------------------------------------------------------------------------------------
// Preview, run, cancel

#[derive(Debug, Clone, Copy, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct DisclosureInput {
    pub full_path: bool,
    pub child_summary: bool,
}

fn origin(s: &str) -> IpcResult<Origin> {
    match s {
        "disk" => Ok(Origin::Disk),
        "cleanup" => Ok(Origin::Cleanup),
        "apps" => Ok(Origin::Apps),
        _ => Err(IpcError::new(ErrorCode::InvalidArgument, "invalid_argument", "origin must be disk, cleanup or apps")),
    }
}

/// The scan context, with this window's fresh catalog facts when the loaded candidate set
/// belongs to the same scan (so a plannable candidate can be recognised).
fn context(s: &AppState, scan_id: &str) -> IpcResult<ScanContext> {
    let mut ctx = s.with_stores(|st| ScanContext::load(st, scan_id).map_err(IpcError::from))?;
    if let Some(l) = lock(&s.candidates).as_ref().filter(|l| l.set.scan_id == scan_id && l.set.revision == ctx.revision) {
        ctx.set = Some(l.set.clone());
        ctx.catalog = l.catalog.clone();
    }
    Ok(ctx)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn diag_prepare(
    state: St<'_>, scan_id: String, entry_id: String, origin: String, provider_id: Option<String>, disclosure: DisclosureInput, question: Option<String>,
) -> IpcResult<Preview> {
    let s = state.inner().clone();
    blocking(move || {
        let svc = service(&s)?;
        let scan = args::scan_id(&scan_id)?;
        let entry = args::entry_id(&entry_id)?;
        let provider = provider_id.as_deref().map(self::provider_id).transpose()?;
        // Configuration first: an unconfigured extension fails before any scan work.
        svc.provider(provider.as_deref())?;
        let ctx = context(&s, &scan)?;
        Ok(svc.prepare(&ctx, PrepareRequest {
            scan_id: scan,
            entry_id: entry,
            origin: self::origin(&origin)?,
            provider_id: provider,
            disclosure: Disclosure { full_path: disclosure.full_path, child_summary: disclosure.child_summary },
            question: bounded_text("question", question, cm_core::diag::budget::USER_TEXT_CHARS)?,
            statements: vec![],
            parent_diagnosis_id: None,
        })?)
    })
    .await
}

/// A follow-up answer becomes a user statement in a new preview of the same object (AID-05).
#[tauri::command(rename_all = "snake_case")]
pub async fn diag_follow_up(state: St<'_>, diagnosis_id: String, question_id: Option<String>, answer: String) -> IpcResult<Preview> {
    let s = state.inner().clone();
    blocking(move || {
        let svc = service(&s)?;
        let id = self::diagnosis_id(&diagnosis_id)?;
        let qid = question_id.map(|q| args::record_id("question", &q)).transpose()?;
        let answer = bounded_text("answer", Some(answer), cm_core::diag::budget::USER_TEXT_CHARS)?
            .ok_or_else(|| IpcError::new(ErrorCode::InvalidArgument, "invalid_argument", "the answer is empty"))?;
        let parent = svc.get(&id, CurrentObservation::NotChecked)?;
        let ctx = context(&s, &parent.request.object.scan_id)?;
        Ok(svc.follow_up(&ctx, &id, qid.as_deref(), &answer)?)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn diag_discard(state: St<'_>, preview_id: String) -> IpcResult<bool> {
    let s = state.inner().clone();
    blocking(move || Ok(service(&s)?.discard(&self::preview_id(&preview_id)?))).await
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagFinishedEvent {
    pub diagnosis_id: String,
    pub state: cm_core::diag::DiagState,
}

/// Confirm the disclosure shown (preview id + its digest) and start the run on a worker thread.
#[tauri::command(rename_all = "snake_case")]
pub async fn diag_confirm(app: AppHandle, state: St<'_>, preview_id: String, payload_digest: String) -> IpcResult<Diagnosis> {
    let s = state.inner().clone();
    blocking(move || {
        let svc = service(&s)?;
        let pid = self::preview_id(&preview_id)?;
        let digest = args::record_id("digest", &payload_digest)?;
        let d = svc.confirm(&pid, &digest)?;
        let id = d.diagnosis_id.clone();
        let worker_svc = svc.clone();
        std::thread::Builder::new()
            .name("cm-diag".into())
            .spawn(move || {
                let state = match worker_svc.execute(&id) {
                    Ok(d) => d.state,
                    Err(_) => cm_core::diag::DiagState::Failed,
                };
                let _ = app.emit(EVENT_FINISHED, DiagFinishedEvent { diagnosis_id: id, state });
            })
            .map_err(|e| IpcError::new(ErrorCode::InternalError, "internal", format!("could not start the diagnosis: {e}")))?;
        Ok(d)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn diag_cancel(state: St<'_>, diagnosis_id: String) -> IpcResult<Diagnosis> {
    let s = state.inner().clone();
    blocking(move || Ok(service(&s)?.cancel(&self::diagnosis_id(&diagnosis_id)?)?)).await
}

fn get_fresh(s: &AppState, svc: &Service, id: &str) -> IpcResult<Diagnosis> {
    let d = svc.get(id, CurrentObservation::NotChecked)?;
    let current = s.with_stores(|st| Ok(current_observation(st, &d)))?;
    Ok(svc.get(id, current)?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn diag_get(state: St<'_>, diagnosis_id: String) -> IpcResult<Diagnosis> {
    let s = state.inner().clone();
    blocking(move || {
        let svc = service(&s)?;
        get_fresh(&s, &svc, &self::diagnosis_id(&diagnosis_id)?)
    })
    .await
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagListDto {
    pub items: Vec<DiagnosisSummary>,
    pub next_cursor: Option<String>,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn diag_list(state: St<'_>, cursor: Option<String>) -> IpcResult<DiagListDto> {
    let s = state.inner().clone();
    blocking(move || {
        let cursor = args::cursor(cursor.as_deref())?;
        let (items, next_cursor) = service(&s)?.list(50, cursor.as_deref())?;
        Ok(DiagListDto { items, next_cursor })
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn diag_save(state: St<'_>, diagnosis_id: String) -> IpcResult<Diagnosis> {
    let s = state.inner().clone();
    blocking(move || Ok(service(&s)?.save(&self::diagnosis_id(&diagnosis_id)?)?)).await
}

/// Remove one diagnosis (never the operation history).
#[tauri::command(rename_all = "snake_case")]
pub async fn diag_remove(state: St<'_>, diagnosis_id: String) -> IpcResult<bool> {
    let s = state.inner().clone();
    blocking(move || Ok(service(&s)?.remove(&self::diagnosis_id(&diagnosis_id)?)?)).await
}

/// Export preview: the JSON a person may copy; it never holds a credential.
#[tauri::command(rename_all = "snake_case")]
pub async fn diag_export(state: St<'_>, diagnosis_id: String) -> IpcResult<String> {
    let s = state.inner().clone();
    blocking(move || {
        let v = service(&s)?.export(&self::diagnosis_id(&diagnosis_id)?)?;
        Ok(serde_json::to_string_pretty(&v).expect("serializable"))
    })
    .await
}

// ---------------------------------------------------------------------------------------------
// Hand-off

#[derive(Debug, Clone, Serialize)]
pub struct HandoffDto {
    pub action: NextAction,
    /// `plan` (open the new plan), `protected`, `disk`, `apps` or `none`.
    pub outcome: &'static str,
    pub plan_id: Option<String>,
    pub scan_id: Option<String>,
    pub entry_id: Option<String>,
    pub app_instance_id: Option<String>,
}

/// Act on one available next step. Availability is recomputed from core facts now; a plan holds
/// exactly the diagnosed candidate and still needs review and approval on the plan page.
#[tauri::command(rename_all = "snake_case")]
pub async fn diag_handoff(state: St<'_>, diagnosis_id: String, action: String, mode: Option<String>) -> IpcResult<HandoffDto> {
    let s = state.inner().clone();
    blocking(move || {
        let svc = service(&s)?;
        let d = get_fresh(&s, &svc, &self::diagnosis_id(&diagnosis_id)?)?;
        let action: NextAction = serde_json::from_value(serde_json::Value::String(action))
            .map_err(|_| IpcError::new(ErrorCode::InvalidArgument, "invalid_argument", "unknown next step"))?;
        let step = d.handoff.iter().find(|h| h.action == action).ok_or_else(|| IpcError::new(ErrorCode::InvalidArgument, "invalid_argument", "unknown next step"))?;
        if !step.available {
            return Err(IpcError::new(ErrorCode::PolicyDenied, "handoff_unavailable", step.reason.clone()));
        }
        let base = HandoffDto { action, outcome: "none", plan_id: None, scan_id: None, entry_id: None, app_instance_id: None };
        let target = step.target.clone();
        match action {
            NextAction::ReviewPlan => {
                let t = target.ok_or_else(|| IpcError::new(ErrorCode::InternalError, "internal", "plan step without target"))?;
                let candidate = t.candidate_id.ok_or_else(|| IpcError::new(ErrorCode::InternalError, "internal", "plan step without candidate"))?;
                let set_id = lock(&s.candidates)
                    .as_ref()
                    .filter(|l| l.set.scan_id == t.scan_id && l.set.candidates.iter().any(|c| c.candidate_id == candidate))
                    .map(|l| l.set.candidate_set_id.clone())
                    .ok_or_else(|| IpcError::new(ErrorCode::PlanStale, "needs_reanalysis", "该候选不在当前分析结果中；请在清理页重新分析后再试"))?;
                let mode = args::parse_mode(mode.as_deref().unwrap_or("trash"))?;
                let created = s.with_stores(|st| crate::cleanup::create(&s, st, &set_id, std::slice::from_ref(&candidate), mode))?;
                Ok(HandoffDto { outcome: "plan", plan_id: Some(created.plan_id), ..base })
            }
            NextAction::Protect => {
                let t = target.ok_or_else(|| IpcError::new(ErrorCode::InternalError, "internal", "protect step without target"))?;
                crate::settings::add(
                    &s,
                    crate::settings::ProtectTarget::Entry { scan_id: t.scan_id.clone(), entry_id: t.entry_id.clone(), recursive: d.facts.kind == "directory" },
                    Some(format!("AI 诊断 {}", d.diagnosis_id)),
                )?;
                Ok(HandoffDto { outcome: "protected", scan_id: Some(t.scan_id), entry_id: Some(t.entry_id), ..base })
            }
            NextAction::Locate => {
                let t = target.ok_or_else(|| IpcError::new(ErrorCode::InternalError, "internal", "locate step without target"))?;
                Ok(HandoffDto { outcome: "disk", scan_id: Some(t.scan_id), entry_id: Some(t.entry_id), ..base })
            }
            NextAction::AppManagement => {
                let t = target.ok_or_else(|| IpcError::new(ErrorCode::InternalError, "internal", "app step without target"))?;
                Ok(HandoffDto { outcome: "apps", scan_id: Some(t.scan_id), app_instance_id: t.app_instance_id, ..base })
            }
            NextAction::Keep | NextAction::Verify | NextAction::Archive => Ok(base),
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_inputs_are_validated_before_use() {
        assert!(preview_id("pv_0123456789abcdef0123").is_ok());
        assert!(preview_id("C:\\Windows").is_err());
        assert!(diagnosis_id("dg_0123456789abcdef0123").is_ok());
        assert!(diagnosis_id("dg_../../x").is_err());
        assert!(provider_id("anthropic-default").is_ok());
        assert!(provider_id("Anthropic Default").is_err());
        assert!(origin("disk").is_ok() && origin("shell").is_err());
        assert!(bounded_text("q", Some("问".repeat(281)), 280).is_err());
        assert_eq!(bounded_text("q", Some("  ".into()), 280).unwrap(), None);
        // A provider input with an executable path is rejected by shape (CLI paths come from the dialog).
        let bad = serde_json::json!({"provider_id": "x", "label": "x", "kind": "anthropic", "base_url": "https://a", "model": "m", "make_default": true, "executable": "C:\\x.exe"});
        assert!(serde_json::from_value::<ApiProviderInput>(bad).is_err());
    }

    #[test]
    fn unconfigured_store_reports_a_clear_state() {
        let st = AppState::with_store(None, None, None);
        let e = service(&st).err().unwrap();
        assert_eq!(e.reason, "diag_not_configured");
    }
}
