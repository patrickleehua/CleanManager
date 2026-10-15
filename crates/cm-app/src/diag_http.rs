//! API adapters for the optional diagnosis (AID-06/08/12): Anthropic Messages API and
//! OpenAI-compatible `chat/completions` (local endpoints included). One request per run, no
//! automatic retry, no redirect, no fallback to another provider or model. Keys travel only in
//! the one request header and are scrubbed from every message this module produces.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cm_core::diag::prompt::ANTHROPIC_VERSION;
use cm_core::diag::{Failure, FailureCode, ProviderKind, budget};
use cm_platform::credentials::Secret;
use serde_json::Value;

/// Largest provider HTTP body read (the answer inside is bounded separately).
pub const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;

#[derive(Debug)]
pub enum RunError {
    Cancelled,
    Failed(Failure),
}

pub fn failure(code: FailureCode, message: impl Into<String>) -> Failure {
    let m: String = message.into();
    let message = if m.chars().count() > budget::MESSAGE_CHARS { m.chars().take(budget::MESSAGE_CHARS - 1).chain(std::iter::once('…')).collect() } else { m };
    Failure { code, retryable: code.retryable(), message }
}

/// Remove the secret and anything key-shaped from provider or transport text.
pub fn scrub(text: &str, secret: Option<&Secret>) -> String {
    let mut out = text.to_owned();
    if let Some(s) = secret
        && !s.expose().is_empty()
    {
        out = out.replace(s.expose(), "<redacted>");
    }
    out.split_inclusive(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ',')
        .map(|w| {
            let core = w.trim_end_matches(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ',');
            let lower = core.to_ascii_lowercase();
            if (lower.starts_with("sk-") || lower.starts_with("sk_")) && core.len() >= 12 { w.replacen(core, "<redacted>", 1) } else { w.to_owned() }
        })
        .collect()
}

fn agent(timeout: Duration) -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    let tls = TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .timeout_connect(Some(timeout.min(Duration::from_secs(15))))
        .http_status_as_error(false)
        .max_redirects(0)
        .tls_config(tls)
        .user_agent(format!("CleanManager/{} (diagnosis)", cm_core::ENGINE_VERSION))
        .build()
        .into()
}

/// One HTTP exchange. `body: None` sends a GET (connectivity probe, no file data).
fn exchange(url: &str, headers: &[(String, String)], body: Option<&str>, timeout: Duration, secret: Option<&Secret>) -> Result<(u16, Vec<u8>), Failure> {
    let a = agent(timeout);
    let result = match body {
        Some(b) => {
            let mut r = a.post(url);
            for (k, v) in headers {
                r = r.header(k, v);
            }
            r.send(b.as_bytes())
        }
        None => {
            let mut r = a.get(url);
            for (k, v) in headers {
                r = r.header(k, v);
            }
            r.call()
        }
    };
    let resp = result.map_err(|e| transport_failure(&e, secret))?;
    let status = resp.status().as_u16();
    let bytes = resp.into_body().with_config().limit(MAX_RESPONSE_BYTES).read_to_vec().map_err(|e| transport_failure(&e, secret))?;
    Ok((status, bytes))
}

fn transport_failure(e: &ureq::Error, secret: Option<&Secret>) -> Failure {
    let text = scrub(&e.to_string(), secret);
    match e {
        ureq::Error::Timeout(_) => failure(FailureCode::Timeout, "提供方在时限内没有返回完整回答。"),
        ureq::Error::BodyExceedsLimit(_) => failure(FailureCode::OutputTooLarge, "提供方返回的内容超过上限，已丢弃。"),
        ureq::Error::HostNotFound => failure(FailureCode::Network, "找不到提供方主机（DNS）。"),
        ureq::Error::ConnectionFailed => failure(FailureCode::Network, "无法连接到提供方。"),
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => failure(FailureCode::Timeout, "提供方在时限内没有返回完整回答。"),
        _ => failure(FailureCode::Network, format!("网络或 TLS 错误：{text}")),
    }
}

fn provider_message(body: &[u8], secret: Option<&Secret>) -> String {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let m = v["error"]["message"].as_str().or_else(|| v["message"].as_str()).unwrap_or("");
    let m: String = m.chars().filter(|c| !c.is_control()).take(160).collect();
    scrub(&m, secret)
}

fn status_failure(status: u16, body: &[u8], secret: Option<&Secret>) -> Failure {
    let detail = provider_message(body, secret);
    let suffix = if detail.is_empty() { String::new() } else { format!("（{detail}）") };
    match status {
        401 | 403 => failure(FailureCode::AuthFailed, format!("提供方拒绝了凭据（HTTP {status}）。请在设置中更新 API Key；没有尝试其他提供方。{suffix}")),
        404 => failure(FailureCode::ModelUnavailable, format!("提供方找不到该模型或端点（HTTP 404）。{suffix}")),
        408 => failure(FailureCode::Timeout, "提供方报告请求超时（HTTP 408）。"),
        429 => failure(FailureCode::RateLimited, format!("提供方限流（HTTP 429），可稍后按新请求重试。{suffix}")),
        402 => failure(FailureCode::ProviderError, format!("提供方报告计费问题（HTTP 402）。{suffix}")),
        _ => failure(FailureCode::ProviderError, format!("提供方返回错误（HTTP {status}）。{suffix}")),
    }
}

/// Request URLs for a configured base (`https://api.anthropic.com`, `http://127.0.0.1:11434/v1`).
pub fn endpoint(kind: ProviderKind, base: &str) -> String {
    let base = base.trim_end_matches('/');
    match kind {
        ProviderKind::Anthropic => format!("{base}/v1/messages"),
        _ => format!("{base}/chat/completions"),
    }
}

pub fn probe_url(kind: ProviderKind, base: &str, model: &str) -> String {
    let base = base.trim_end_matches('/');
    match kind {
        ProviderKind::Anthropic => format!("{base}/v1/models/{model}"),
        _ => format!("{base}/models"),
    }
}

fn auth_headers(kind: ProviderKind, secret: Option<&Secret>) -> Vec<(String, String)> {
    let mut h = Vec::new();
    match kind {
        ProviderKind::Anthropic => {
            h.push(("anthropic-version".into(), ANTHROPIC_VERSION.into()));
            if let Some(s) = secret {
                h.push(("x-api-key".into(), s.expose().into()));
            }
        }
        _ => {
            if let Some(s) = secret {
                h.push(("authorization".into(), format!("Bearer {}", s.expose())));
            }
        }
    }
    h
}

/// No-data connectivity probe: authenticates and checks the model or the model list. The
/// request has no body and carries no file information.
pub fn probe(kind: ProviderKind, base: &str, model: &str, secret: Option<&Secret>, timeout: Duration) -> Result<String, Failure> {
    let url = probe_url(kind, base, model);
    let (status, body) = exchange(&url, &auth_headers(kind, secret), None, timeout, secret)?;
    if status != 200 {
        return Err(status_failure(status, &body, secret));
    }
    let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    match kind {
        ProviderKind::Anthropic => Ok(format!("已连接；模型 {} 可用。", v["id"].as_str().unwrap_or(model))),
        _ => {
            let listed = v["data"].as_array().is_some_and(|d| d.iter().any(|m| m["id"].as_str() == Some(model)));
            Ok(if listed { format!("已连接；模型 {model} 在列表中。") } else { format!("已连接；模型列表中没有 {model}，运行时可能失败。") })
        }
    }
}

/// Send the confirmed body and return the provider's answer text. Cancellation stops waiting
/// at once; a late response is dropped by the caller's lifecycle check.
pub fn send(kind: ProviderKind, url: &str, body: &str, secret: Option<Secret>, timeout: Duration, cancel: &Arc<AtomicBool>) -> Result<String, RunError> {
    let url = url.to_owned();
    let body = body.to_owned();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::Builder::new().name("cm-diag-http".into()).spawn(move || {
        let mut headers = vec![("content-type".to_owned(), "application/json".to_owned())];
        headers.extend(auth_headers(kind, secret.as_ref()));
        let r = exchange(&url, &headers, Some(&body), timeout, secret.as_ref()).and_then(|(status, bytes)| {
            if status != 200 {
                return Err(status_failure(status, &bytes, secret.as_ref()));
            }
            answer_text(kind, &bytes)
        });
        drop(headers);
        let _ = tx.send(r);
    });
    if worker.is_err() {
        return Err(RunError::Failed(failure(FailureCode::ProviderError, "无法启动网络线程。")));
    }
    let start = Instant::now();
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(RunError::Cancelled);
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(r) => return r.map_err(RunError::Failed),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) if start.elapsed() > timeout + Duration::from_secs(5) => {
                return Err(RunError::Failed(failure(FailureCode::Timeout, "提供方在时限内没有返回完整回答。")));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Err(RunError::Failed(failure(FailureCode::ProviderError, "网络线程意外结束。"))),
        }
    }
}

/// Text of a successful response. A refusal, a truncated (`max_tokens`/`length`) or empty answer
/// is a failure, never a partial result (AID-10).
pub fn answer_text(kind: ProviderKind, body: &[u8]) -> Result<String, Failure> {
    let v: Value = serde_json::from_slice(body).map_err(|_| failure(FailureCode::InvalidStructure, "提供方回复不是 JSON。"))?;
    let (text, stop) = match kind {
        ProviderKind::Anthropic => {
            let text: String = v["content"]
                .as_array()
                .map(|blocks| blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join(""))
                .unwrap_or_default();
            (text, v["stop_reason"].as_str().unwrap_or("").to_owned())
        }
        _ => {
            let c = &v["choices"][0];
            (c["message"]["content"].as_str().unwrap_or("").to_owned(), c["finish_reason"].as_str().unwrap_or("").to_owned())
        }
    };
    match stop.as_str() {
        "refusal" | "content_filter" => Err(failure(FailureCode::Refused, "提供方拒绝回答这个请求；没有改用其他模型。")),
        "max_tokens" | "length" => Err(failure(FailureCode::InvalidStructure, "回答在完成前被截断，不作为结果使用。")),
        _ if text.trim().is_empty() => Err(failure(FailureCode::InvalidStructure, "提供方返回了空回答。")),
        _ if text.len() > budget::RAW_ANSWER_BYTES => Err(failure(FailureCode::OutputTooLarge, "回答超过长度上限，已丢弃。")),
        _ => Ok(text),
    }
}
