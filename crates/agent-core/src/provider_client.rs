use crate::{
    AgentEvent, AgentMessage, MessageRole, ProviderCapabilities, ProviderConfig, ProviderKind,
    ToolDefinition,
};
use async_stream::try_stream;
use futures_core::Stream;
use futures_util::StreamExt;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::pin::Pin;
use thiserror::Error;
use tokio::time::{sleep, Duration};

pub type AgentEventStream =
    Pin<Box<dyn Stream<Item = Result<AgentEvent, ProviderClientError>> + Send>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderImageAttachment {
    pub name: String,
    pub media_type: String,
    pub data_url: String,
    /// Stable id of the user message that originally supplied this image.
    pub source_message_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRequest {
    pub session_id: String,
    pub model: String,
    pub messages: Vec<AgentMessage>,
    pub system_prompt: Option<String>,
    pub tools: Vec<ToolDefinition>,
    pub max_tokens: u32,
    #[serde(default)]
    pub image_attachments: Vec<ProviderImageAttachment>,
    #[serde(default)]
    pub include_usage: bool,
    /// Some Responses-compatible relays implement streaming but reject the
    /// optional encrypted-reasoning include field. Keep it independently
    /// switchable so the runtime can retry that optional extension without
    /// changing the core Responses request.
    #[serde(default = "default_include_reasoning_encrypted_content")]
    pub include_reasoning_encrypted_content: bool,
    #[serde(rename = "reasoningEffort", default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub native_reasoning_effort: bool,
    #[serde(default)]
    pub prompt_cache_retention: Option<String>,
    #[serde(default)]
    pub prompt_cache_key: Option<String>,
}

fn default_include_reasoning_encrypted_content() -> bool {
    true
}

impl ProviderRequest {
    pub fn new(
        session_id: impl Into<String>,
        model: impl Into<String>,
        messages: Vec<AgentMessage>,
        tools: Vec<ToolDefinition>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            model: model.into(),
            messages,
            system_prompt: None,
            tools,
            max_tokens: 4096,
            image_attachments: Vec::new(),
            // Preserve the historical request-body default for callers that
            // build ProviderRequest directly. The desktop Agent explicitly
            // disables this for unknown OpenAI-compatible servers.
            include_usage: true,
            include_reasoning_encrypted_content: true,
            reasoning_effort: None,
            native_reasoning_effort: false,
            prompt_cache_retention: None,
            prompt_cache_key: None,
        }
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    pub fn with_image_attachments(mut self, attachments: Vec<ProviderImageAttachment>) -> Self {
        self.image_attachments = attachments;
        self
    }

    pub fn with_usage_reporting(mut self, enabled: bool) -> Self {
        self.include_usage = enabled;
        self
    }

    pub fn with_reasoning_encrypted_content(mut self, enabled: bool) -> Self {
        self.include_reasoning_encrypted_content = enabled;
        self
    }

    pub fn with_reasoning_effort(mut self, effort: Option<String>) -> Self {
        self.reasoning_effort = effort.filter(|value| !value.trim().is_empty());
        self
    }

    pub fn with_native_reasoning_effort(mut self, enabled: bool) -> Self {
        self.native_reasoning_effort = enabled;
        self
    }

    pub fn with_prompt_cache_retention(mut self, retention: Option<String>) -> Self {
        self.prompt_cache_retention = retention;
        self
    }

    pub fn with_prompt_cache_key(mut self, key: impl Into<String>) -> Self {
        let key = key.into();
        self.prompt_cache_key = (!key.trim().is_empty()).then_some(key);
        self
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProviderStatusError {
    #[error("provider rejected credentials")]
    Unauthorized,
    #[error("provider denied access")]
    Forbidden,
    #[error("provider endpoint or model was not found")]
    NotFound,
    #[error("provider rate limit exceeded")]
    RateLimited,
    #[error("provider internal server error")]
    InternalServerError,
    #[error("provider server failure")]
    ServerFailure,
    #[error("provider returned HTTP status {0}")]
    Unexpected(u16),
}

impl ProviderStatusError {
    pub fn from_status(status: u16) -> Self {
        match status {
            401 => Self::Unauthorized,
            403 => Self::Forbidden,
            404 => Self::NotFound,
            429 => Self::RateLimited,
            500 => Self::InternalServerError,
            501..=599 => Self::ServerFailure,
            other => Self::Unexpected(other),
        }
    }
}

#[derive(Debug, Error)]
pub enum ProviderClientError {
    #[error("provider request failed: {0}")]
    Network(String),
    #[error("{0}")]
    Timeout(String),
    #[error("{kind}: {message}")]
    Status {
        kind: ProviderStatusError,
        message: String,
    },
    #[error("provider response could not be parsed: {0}")]
    Parse(String),
    #[error("{message}")]
    Incomplete { reason: String, message: String },
    #[error("{0}")]
    Repetition(String),
    #[error("provider protocol is incompatible: {0}")]
    ProtocolMismatch(String),
    #[error("provider configuration is invalid: {0}")]
    Configuration(String),
}

impl ProviderClientError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Network(_) => "network",
            Self::Timeout(_) => "provider_timeout",
            Self::Status { kind, .. } => match kind {
                ProviderStatusError::Unauthorized => "unauthorized",
                ProviderStatusError::Forbidden => "forbidden",
                ProviderStatusError::NotFound => "not_found",
                ProviderStatusError::RateLimited => "rate_limited",
                ProviderStatusError::InternalServerError | ProviderStatusError::ServerFailure => {
                    "server_failure"
                }
                ProviderStatusError::Unexpected(_) => "http_error",
            },
            Self::Parse(_) => "parse",
            Self::Incomplete { .. } => "provider_incomplete",
            Self::Repetition(_) => "repetitive_output",
            Self::ProtocolMismatch(_) => "protocol_mismatch",
            Self::Configuration(_) => "configuration",
        }
    }
}

pub struct HttpProviderClient {
    config: ProviderConfig,
    api_key: String,
    client: Client,
    response_start_timeout: Duration,
    cache_retention_rejected: std::sync::Arc<std::sync::atomic::AtomicBool>,
    diagnostic_observer: Option<ProviderDiagnosticObserver>,
    diagnostic_sequence: std::sync::atomic::AtomicU64,
}

const MAX_ERROR_BODY_BYTES: usize = 1024 * 1024;
const PROVIDER_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
// Reasoning relays may buffer headers or stream chunks while the model works.
pub const PROVIDER_IDLE_TIMEOUT: Duration = Duration::from_secs(180);
const PROVIDER_MAX_ATTEMPTS: usize = 5;
const PROVIDER_SERVER_FAILURE_MAX_ATTEMPTS: usize = 2;

/// 出站模型请求统一使用的 User-Agent。
///
/// 用产品名标注调用方形态，便于中转网关区分请求来源。
pub const PROVIDER_USER_AGENT: &str = "Android Toolkit";

fn provider_request_builder(
    client: &Client,
    url: &str,
    body: &Value,
    kind: ProviderKind,
    api_key: &str,
) -> reqwest::RequestBuilder {
    let builder = client
        .post(url)
        .header("content-type", "application/json")
        .header("user-agent", PROVIDER_USER_AGENT)
        .json(body);
    match kind {
        ProviderKind::Anthropic => builder
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01"),
        ProviderKind::OpenAi
        | ProviderKind::OpenAiResponses
        | ProviderKind::OpenAiCompatible
        | ProviderKind::DeepSeek
        | ProviderKind::Kimi
        | ProviderKind::Glm
        | ProviderKind::MiMo => {
            if api_key.trim().is_empty() {
                builder
            } else {
                builder.header("authorization", format!("Bearer {api_key}"))
            }
        }
    }
}

async fn provider_status_error(response: reqwest::Response) -> ProviderClientError {
    let status = response.status();
    let mut body_stream = response.bytes_stream();
    let mut error_body = Vec::with_capacity(MAX_ERROR_BODY_BYTES.min(16 * 1024));
    let mut truncated = false;
    while let Some(chunk) = body_stream.next().await {
        let Ok(chunk) = chunk else {
            truncated = true;
            break;
        };
        let remaining = MAX_ERROR_BODY_BYTES.saturating_sub(error_body.len());
        if remaining == 0 {
            truncated = true;
            break;
        }
        if chunk.len() > remaining {
            error_body.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        error_body.extend_from_slice(&chunk);
    }
    let body_text = String::from_utf8_lossy(&error_body);
    let mut message = provider_error_message(&body_text, "Provider HTTP 请求失败");
    // Keep the transport status in the safe diagnostic. The desktop host can
    // distinguish a relay's 503 from a bad model or credential without
    // exposing request headers or API keys.
    message = format!("HTTP {}: {}", status.as_u16(), message);
    if truncated {
        message.push_str(" (response body truncated after 1 MiB)");
    }
    ProviderClientError::Status {
        kind: ProviderStatusError::from_status(status.as_u16()),
        message,
    }
}

async fn send_provider_request(
    client: &Client,
    url: &str,
    body: &Value,
    kind: ProviderKind,
    api_key: &str,
    session_id: &str,
    response_start_timeout: Duration,
    diagnostic: &mut AttemptDiagnostic,
) -> Result<reqwest::Response, ProviderClientError> {
    let response = tokio::time::timeout(response_start_timeout, {
        let mut builder = provider_request_builder(client, url, body, kind, api_key);
        // Local model plugins preserve the host's conversation affinity upstream.
        if url::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .is_some_and(|h| h == "127.0.0.1" || h == "localhost")
        {
            builder = builder.header("x-android-tool-session", session_id);
        }
        builder.send()
    })
    .await
    .map_err(|_| {
        ProviderClientError::Timeout(format!(
            "等待模型服务返回响应头超过 {} 秒，已停止等待，未自动重发请求。",
            response_start_timeout.as_secs()
        ))
    })?
    .map_err(|error| {
        if error.is_timeout() && !error.is_connect() {
            ProviderClientError::Timeout(
                "等待模型服务响应超时，已停止等待，未自动重发请求。".into(),
            )
        } else {
            ProviderClientError::Network(error.to_string())
        }
    })?;
    diagnostic.headers(&response);
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(provider_status_error(response).await)
    }
}

fn provider_retry_delay(attempt: usize) -> Duration {
    let multiplier = 1_u64 << attempt.saturating_sub(1).min(3);
    Duration::from_millis(500 * multiplier)
}

fn provider_retry_delay_label(delay: Duration) -> String {
    if delay.as_millis() % 1_000 == 0 {
        format!("{} 秒", delay.as_secs())
    } else {
        format!("{:.1} 秒", delay.as_secs_f64())
    }
}

fn provider_error_is_retryable(error: &ProviderClientError) -> bool {
    match error {
        ProviderClientError::Network(_) => true,
        ProviderClientError::Status {
            kind:
                ProviderStatusError::RateLimited
                | ProviderStatusError::InternalServerError
                | ProviderStatusError::ServerFailure
                | ProviderStatusError::Unexpected(408 | 425),
            ..
        } => true,
        ProviderClientError::Incomplete { reason, .. } => matches!(
            reason.as_str(),
            "server_error" | "network_error" | "timeout" | "temporarily_unavailable"
        ),
        ProviderClientError::Parse(message) => message == "Provider 流式响应在终态前断开",
        _ => false,
    }
}

fn provider_retry_limit(error: &ProviderClientError) -> usize {
    match error {
        ProviderClientError::Status {
            kind: ProviderStatusError::InternalServerError,
            message,
        } if !provider_status_is_context_limit(message) => PROVIDER_SERVER_FAILURE_MAX_ATTEMPTS,
        ProviderClientError::Status {
            kind: ProviderStatusError::InternalServerError,
            ..
        } => 1,
        _ => PROVIDER_MAX_ATTEMPTS,
    }
}

fn provider_status_is_context_limit(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "context window",
        "context length",
        "context_length_exceeded",
        "context limit",
        "context_limit",
        "maximum context",
        "too many tokens",
        "token limit",
        "request_too_large",
        "prompt is too long",
        "上下文上限",
        "上下文限制",
        "上下文长度",
        "输入过长",
    ]
    .iter()
    .any(|marker| message.contains(marker))
}

fn exhausted_provider_error(error: ProviderClientError, attempts: usize) -> ProviderClientError {
    let prefix = format!("模型服务连续 {attempts} 次请求失败：");
    match error {
        ProviderClientError::Network(message) => {
            ProviderClientError::Network(format!("{prefix}{message}"))
        }
        ProviderClientError::Status { kind, message } => ProviderClientError::Status {
            kind,
            message: format!("{prefix}{message}"),
        },
        ProviderClientError::Parse(message) => {
            ProviderClientError::Parse(format!("{prefix}{message}"))
        }
        ProviderClientError::Incomplete { reason, message } => ProviderClientError::Incomplete {
            reason,
            message: format!("{prefix}{message}"),
        },
        error @ ProviderClientError::Repetition(_) => error,
        error @ ProviderClientError::Timeout(_) => error,
        ProviderClientError::ProtocolMismatch(message) => {
            ProviderClientError::ProtocolMismatch(format!("{prefix}{message}"))
        }
        ProviderClientError::Configuration(message) => {
            ProviderClientError::Configuration(format!("{prefix}{message}"))
        }
    }
}

impl HttpProviderClient {
    pub fn new(
        config: ProviderConfig,
        api_key: impl Into<String>,
    ) -> Result<Self, ProviderClientError> {
        config
            .validate()
            .map_err(|error| ProviderClientError::Configuration(error.to_string()))?;
        let api_key = api_key.into();
        if api_key.trim().is_empty() && config.kind != ProviderKind::OpenAiCompatible {
            return Err(ProviderClientError::Configuration(
                "API key must not be empty".to_string(),
            ));
        }
        let builder =
            crate::network::http_client_builder().map_err(ProviderClientError::Configuration)?;
        // Local model/plugin bridges must not send their temporary credentials
        // through the configured remote HTTP proxy.
        let local = url::Url::parse(&config.base_url)
            .ok()
            .is_some_and(|url| match url.host() {
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
                None => false,
            });
        let client = (if local { builder.no_proxy() } else { builder })
            .connect_timeout(PROVIDER_CONNECT_TIMEOUT)
            .read_timeout(PROVIDER_IDLE_TIMEOUT)
            .build()
            .map_err(|error| ProviderClientError::Configuration(error.to_string()))?;
        Ok(Self {
            config,
            api_key,
            client,
            response_start_timeout: PROVIDER_IDLE_TIMEOUT,
            cache_retention_rejected: Default::default(),
            diagnostic_observer: None,
            diagnostic_sequence: Default::default(),
        })
    }

    /// Observe one bounded metadata record per HTTP attempt, including failed
    /// streams and streams dropped by cancellation. Never exposes bodies.
    pub fn with_diagnostic_observer(mut self, observer: ProviderDiagnosticObserver) -> Self {
        self.diagnostic_observer = Some(observer);
        self
    }

    pub fn config(&self) -> &ProviderConfig {
        &self.config
    }

    pub async fn stream(
        &self,
        request: ProviderRequest,
    ) -> Result<AgentEventStream, ProviderClientError> {
        let url = endpoint(&self.config);
        let mut body =
            request_body(&self.config, &request).map_err(ProviderClientError::Configuration)?;
        let cache_retention_rejected = self.cache_retention_rejected.clone();
        if cache_retention_rejected.load(std::sync::atomic::Ordering::Relaxed) {
            remove_cache_retention(&mut body);
        }
        let session_id = request.session_id;
        let kind = self.config.kind;
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let response_start_timeout = self.response_start_timeout;
        let diagnostic_observer = self.diagnostic_observer.clone();
        let diagnostic_request = self
            .diagnostic_sequence
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        let stream = try_stream! {
            let mut attempt = 0usize;
            loop {
                attempt = attempt.saturating_add(1);
                let mut diagnostic = AttemptDiagnostic::new(diagnostic_observer.clone(), &session_id, diagnostic_request, attempt, kind);
                // A normal connection start is too brief to be useful in the
                // timeline. Only surface it once the provider is actually
                // slow, while retry events remain visible immediately.
                let request_started = tokio::time::Instant::now();
                let mut request = Box::pin(send_provider_request(&client, &url, &body, kind, &api_key, &session_id, response_start_timeout, &mut diagnostic));
                let mut reported_slow_connection = false;
                let response = loop {
                    let wait = if reported_slow_connection {
                        Duration::from_secs(10)
                    } else {
                        Duration::from_secs(8)
                    };
                    tokio::select! {
                        result = &mut request => break result,
                        _ = sleep(wait) => {
                            reported_slow_connection = true;
                            let elapsed = request_started.elapsed().as_secs();
                            yield AgentEvent::Status {
                                session_id: session_id.clone(),
                                status: "provider_waiting".to_string(),
                                message: Some(format!("模型服务响应较慢，仍在等待（已等待 {elapsed} 秒）")),
                            };
                        }
                    }
                };
                drop(request);
                let response = match response {
                    Ok(response) => response,
                    Err(error) => {
                        diagnostic.data.failure_code = Some(error.code());
                        diagnostic.data.ending = "request_failed";
                        if cache_retention_unsupported(&error) && remove_cache_retention(&mut body) {
                            diagnostic.data.retry_scheduled = true;
                            diagnostic.report();
                            cache_retention_rejected.store(true, std::sync::atomic::Ordering::Relaxed);
                            yield AgentEvent::Status { session_id:session_id.clone(), status:"cache_retention_fallback".into(),
                                message:Some("当前接口不支持延长缓存，已使用默认时长继续".into()) };
                            continue;
                        }
                        let retry_limit = provider_retry_limit(&error);
                        if provider_error_is_retryable(&error) && attempt < retry_limit {
                            diagnostic.data.retry_scheduled = true;
                            diagnostic.report();
                            let delay = provider_retry_delay(attempt);
                            yield AgentEvent::Status {
                                session_id: session_id.clone(),
                                status: "provider_retry".to_string(),
                                message: Some(format!(
                                    "模型服务第 {attempt}/{retry_limit} 次请求失败：{error}；{} 后自动重试",
                                    provider_retry_delay_label(delay),
                                )),
                            };
                            sleep(delay).await;
                            continue;
                        }
                        let error = if provider_error_is_retryable(&error) {
                            exhausted_provider_error(error, attempt)
                        } else {
                            error
                        };
                        diagnostic.report();
                        Err(error)?;
                        unreachable!("provider error should terminate the stream");
                    }
                };

                let mut chunks = response.bytes_stream();
                let mut decoder = SseDecoder::default();
                let mut response_item_call_ids = HashMap::<String, String>::new();
                let mut response_output_channels = ResponsesOutputChannels::default();
                let mut latest_usage = None::<UsageSnapshot>;
                let mut saw_terminal_event = false;
                let mut accepted_provider_event = false;
                let mut stream_error = None::<ProviderClientError>;
                let mut transport_complete = false;

                'stream: while let Some(chunk) = chunks.next().await {
                    let chunk = match chunk {
                        Ok(chunk) => chunk,
                        Err(error) => {
                            stream_error = Some(if error.is_timeout() {
                                ProviderClientError::Timeout("模型流式响应长时间没有新数据，已停止等待并保留此前内容，未自动重发请求。".into())
                            } else {
                                ProviderClientError::Network(error.to_string())
                            });
                            break;
                        }
                    };
                    diagnostic.data.received_bytes = diagnostic.data.received_bytes.saturating_add(chunk.len());
                    let frames = match decoder.push_chunk(&chunk) {
                        Ok(frames) => frames,
                        Err(error) => {
                            stream_error = Some(error);
                            break;
                        }
                    };
                    for data in frames {
                        diagnostic.frame(&data);
                        if data.trim() == "[DONE]" {
                            diagnostic.data.ending = "sse_done";
                            // `[DONE]` is the SSE-level end of the response:
                            // relays that never send a finish reason close the
                            // stream with it, so it is the terminal marker
                            // rather than a premature disconnect.
                            saw_terminal_event = true;
                            transport_complete = true;
                            break 'stream;
                        }
                        remember_responses_call_id(kind, &data, &mut response_item_call_ids);
                        response_output_channels.remember(kind, &data);
                        let events = match events::parse_sse_data_observed(kind, &session_id, &data, Some(&mut diagnostic), &mut latest_usage) {
                            Ok(events) => events,
                            Err(error) => {
                                accepted_provider_event |= remember_failed_usage(kind, &session_id, &data, &mut latest_usage);
                                stream_error = Some(error);
                                break 'stream;
                            }
                        };
                        accepted_provider_event |= !events.is_empty();
                        for event in events {
                            let event = normalize_responses_call_id(event, &response_item_call_ids);
                            let event = response_output_channels.route(kind, &data, event);
                            for event in collect_stream_event(
                                event,
                                &mut latest_usage,
                                &mut saw_terminal_event,
                            ) {
                                yield event;
                            }
                        }
                        if saw_terminal_event && matches!(kind, ProviderKind::Anthropic | ProviderKind::OpenAiResponses) {
                            diagnostic.data.ending = "protocol_terminal";
                            transport_complete = true;
                            break 'stream;
                        }
                    }
                }

                if stream_error.is_none() && !transport_complete {
                    diagnostic.data.ending = "eof";
                    match decoder.finish() {
                        Ok(frames) => {
                            for data in frames {
                                diagnostic.frame(&data);
                                if data.trim() == "[DONE]" {
                                    diagnostic.data.ending = "sse_done";
                                    // Same terminal judgement as the live loop.
                                    saw_terminal_event = true;
                                    break;
                                }
                                remember_responses_call_id(kind, &data, &mut response_item_call_ids);
                                response_output_channels.remember(kind, &data);
                                let events = match events::parse_sse_data_observed(kind, &session_id, &data, Some(&mut diagnostic), &mut latest_usage) {
                                    Ok(events) => events,
                                    Err(error) => {
                                        accepted_provider_event |= remember_failed_usage(kind, &session_id, &data, &mut latest_usage);
                                        stream_error = Some(error);
                                        break;
                                    }
                                };
                                accepted_provider_event |= !events.is_empty();
                                for event in events {
                                    let event = normalize_responses_call_id(event, &response_item_call_ids);
                                    let event = response_output_channels.route(kind, &data, event);
                                    for event in collect_stream_event(
                                        event,
                                        &mut latest_usage,
                                        &mut saw_terminal_event,
                                    ) {
                                        yield event;
                                    }
                                }
                                if saw_terminal_event && matches!(kind, ProviderKind::Anthropic | ProviderKind::OpenAiResponses) {
                                    diagnostic.data.ending = "protocol_terminal";
                                    break;
                                }
                            }
                        }
                        Err(error) => stream_error = Some(error),
                    }
                }

                diagnostic.data.saw_protocol_terminal = saw_terminal_event;
                if stream_error.is_none() && !saw_terminal_event {
                    stream_error = Some(ProviderClientError::Parse(
                        "Provider 流式响应在终态前断开".to_string(),
                    ));
                }
                if let Some(error) = stream_error {
                    diagnostic.data.failure_code = Some(error.code());
                    diagnostic.data.ending = "stream_failed";
                    if !accepted_provider_event
                        && provider_error_is_retryable(&error)
                        && attempt < provider_retry_limit(&error)
                    {
                        diagnostic.data.retry_scheduled = true;
                        diagnostic.report();
                        let delay = provider_retry_delay(attempt);
                        yield AgentEvent::Status {
                            session_id: session_id.clone(),
                            status: "provider_retry".to_string(),
                            message: Some(format!(
                                "模型服务第 {attempt}/{} 次响应失败：{error}；{} 后自动重试",
                                provider_retry_limit(&error),
                                provider_retry_delay_label(delay),
                            )),
                        };
                        sleep(delay).await;
                        continue;
                    }
                    let error = if !accepted_provider_event && provider_error_is_retryable(&error) {
                        exhausted_provider_error(error, attempt)
                    } else {
                        error
                    };
                    diagnostic.report();
                    if let Some(usage) = latest_usage.take() { yield usage.into_event(&session_id); }
                    Err(error)?;
                }
                diagnostic.report();
                if let Some(usage) = latest_usage {
                    yield usage.into_event(&session_id);
                }
                yield AgentEvent::Completed {
                    session_id: session_id.clone(),
                };
                break;
            }
        };

        Ok(Box::pin(stream))
    }
}

#[allow(dead_code)]
fn _status_from_response(status: StatusCode) -> ProviderStatusError {
    ProviderStatusError::from_status(status.as_u16())
}

mod diagnostics;
mod stream_state;
use diagnostics::AttemptDiagnostic;
pub use diagnostics::{ProviderDiagnosticObserver, ProviderTransportDiagnostic};
use stream_state::{
    collect_stream_event, is_reasoning_channel, normalize_responses_call_id, remember_failed_usage,
    remember_responses_call_id, ResponsesOutputChannels, SseDecoder, UsageSnapshot,
};
mod requests;
pub use requests::*;

mod events;
pub use events::*;
use events::{parse_anthropic_visible_event, parse_openai_event, responses_terminal_events};
mod errors;
use errors::{provider_error_message, provider_error_value_message};
#[cfg(test)]
mod live;
#[cfg(test)]
mod tests;

fn cache_retention_unsupported(error: &ProviderClientError) -> bool {
    matches!(error, ProviderClientError::Status {kind:ProviderStatusError::Unexpected(400 | 422), message}
        if (message.to_ascii_lowercase().contains("ttl") || message.to_ascii_lowercase().contains("prompt_cache_retention"))
        && ["unsupported", "not supported", "unknown", "unrecognized", "not permitted", "not allowed", "extra", "invalid"]
            .iter().any(|word|message.to_ascii_lowercase().contains(word)))
}

fn remove_cache_retention(body: &mut Value) -> bool {
    let mut changed = false;
    if let Some(object) = body.as_object_mut() {
        changed |= object.remove("prompt_cache_retention").is_some();
        if let Some(control) = object
            .get_mut("cache_control")
            .and_then(Value::as_object_mut)
        {
            changed |= control.remove("ttl").is_some();
        }
        for field in ["system", "tools", "messages", "content"] {
            if let Some(value) = object.get_mut(field) {
                changed |= remove_cache_retention(value);
            }
        }
    } else if let Some(values) = body.as_array_mut() {
        for value in values {
            changed |= remove_cache_retention(value);
        }
    }
    changed
}

#[cfg(test)]
mod cache_tests;
