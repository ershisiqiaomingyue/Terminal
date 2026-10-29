//! Bounded transport metadata. No prompts, response bodies, or credentials.
use super::*;
use std::sync::Arc;
use std::time::Instant;

pub type ProviderDiagnosticObserver = Arc<dyn Fn(ProviderTransportDiagnostic) + Send + Sync>;

#[derive(Clone, Debug, Serialize)]
pub struct ProviderTransportDiagnostic {
    pub session_id: String,
    pub request_index: u64,
    pub attempt: usize,
    pub protocol: ProviderKind,
    pub http_status: Option<u16>,
    pub request_id: Option<String>,
    pub header_wait_ms: Option<u64>,
    pub first_frame_ms: Option<u64>,
    pub last_frame_ms: Option<u64>,
    pub elapsed_ms: u64,
    pub received_bytes: usize,
    pub sse_frames: usize,
    pub wire_text_characters: usize,
    pub wire_reasoning_characters: usize,
    pub unrecognized_content_frames: usize,
    pub tool_starts: usize,
    pub terminal_reason: Option<String>,
    pub reasoning_tokens: Option<u64>,
    pub saw_protocol_terminal: bool,
    pub saw_done: bool,
    pub ending: &'static str,
    pub failure_code: Option<&'static str>,
    pub retry_scheduled: bool,
}

pub(super) struct AttemptDiagnostic {
    observer: Option<ProviderDiagnosticObserver>,
    started: Instant,
    pub data: ProviderTransportDiagnostic,
}

impl AttemptDiagnostic {
    pub fn new(
        observer: Option<ProviderDiagnosticObserver>,
        session: &str,
        request: u64,
        attempt: usize,
        protocol: ProviderKind,
    ) -> Self {
        Self {
            observer,
            started: Instant::now(),
            data: ProviderTransportDiagnostic {
                session_id: session.into(),
                request_index: request,
                attempt,
                protocol,
                http_status: None,
                request_id: None,
                header_wait_ms: None,
                first_frame_ms: None,
                last_frame_ms: None,
                elapsed_ms: 0,
                received_bytes: 0,
                sse_frames: 0,
                wire_text_characters: 0,
                wire_reasoning_characters: 0,
                unrecognized_content_frames: 0,
                tool_starts: 0,
                terminal_reason: None,
                reasoning_tokens: None,
                saw_protocol_terminal: false,
                saw_done: false,
                ending: "consumer_dropped",
                failure_code: None,
                retry_scheduled: false,
            },
        }
    }

    fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis().min(u64::MAX as u128) as u64
    }

    pub fn headers(&mut self, response: &reqwest::Response) {
        self.data.http_status = Some(response.status().as_u16());
        self.data.header_wait_ms = Some(self.elapsed_ms());
        self.data.request_id = ["x-request-id", "request-id", "x-amzn-requestid"]
            .iter()
            .filter_map(|name| response.headers().get(*name)?.to_str().ok())
            .find(|id| {
                !id.is_empty()
                    && id.len() <= 128
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
            })
            .map(str::to_owned);
    }

    pub fn frame(&mut self, data: &str) {
        self.data.sse_frames = self.data.sse_frames.saturating_add(1);
        let elapsed = self.elapsed_ms();
        self.data.first_frame_ms.get_or_insert(elapsed);
        self.data.last_frame_ms = Some(elapsed);
        if data.trim() == "[DONE]" {
            self.data.saw_done = true;
        }
    }

    /// Reasoning-token details parsed from the provider's usage object. Kept on
    /// the record so a protocol that reports them can be told apart from one
    /// that does not (the latter stays `None`); nothing is inferred here.
    /// Recorded even without an observer so the stream state can use it.
    pub fn record_reasoning_tokens(&mut self, tokens: Option<u64>) {
        if tokens.is_some() {
            self.data.reasoning_tokens = tokens;
        }
    }

    // Called with the original decoded JSON before protocol event parsing.
    // This can expose discarded/unrecognized content without retaining text,
    // and avoids a second JSON parse for every streamed token.
    pub fn value(&mut self, kind: ProviderKind, value: &Value) {
        if self.observer.is_none() {
            return;
        }
        let chars = |v: &Value| v.as_str().map(|s| s.chars().count()).unwrap_or(0);
        match kind {
            ProviderKind::Anthropic => match value["type"].as_str() {
                Some("content_block_start") => {
                    let block = &value["content_block"];
                    self.data.wire_text_characters += chars(&block["text"]);
                    self.data.wire_reasoning_characters += chars(&block["thinking"]);
                    self.data.tool_starts += usize::from(block["type"] == "tool_use");
                }
                Some("content_block_delta") => {
                    self.data.wire_text_characters += chars(&value["delta"]["text"]);
                    self.data.wire_reasoning_characters += chars(&value["delta"]["thinking"]);
                }
                _ => {}
            },
            ProviderKind::OpenAiResponses => match value["type"].as_str() {
                Some("response.output_text.delta") => {
                    self.data.wire_text_characters += chars(&value["delta"])
                }
                Some("response.reasoning_text.delta" | "response.reasoning_summary_text.delta") => {
                    self.data.wire_reasoning_characters += chars(&value["delta"])
                }
                Some("response.output_item.added") => {
                    self.data.tool_starts += usize::from(value["item"]["type"] == "function_call")
                }
                _ => {}
            },
            _ => {
                let delta = &value["choices"][0]["delta"];
                self.data.wire_text_characters += chars(&delta["content"]);
                self.data.wire_reasoning_characters +=
                    ["reasoning_content", "reasoning", "thinking"]
                        .iter()
                        .find_map(|field| delta[*field].as_str())
                        .map(|s| s.chars().count())
                        .unwrap_or(0);
                if (!delta["content"].is_null() && !delta["content"].is_string())
                    || value["choices"][0]["message"].is_object()
                {
                    self.data.unrecognized_content_frames += 1;
                }
                if let Some(calls) = delta["tool_calls"].as_array() {
                    self.data.tool_starts += calls
                        .iter()
                        .filter(|call| {
                            call.get("id").is_some() || call["function"].get("name").is_some()
                        })
                        .count();
                }
            }
        }
        let reason = match kind {
            ProviderKind::Anthropic => value["delta"]["stop_reason"].as_str(),
            ProviderKind::OpenAiResponses => match value["type"].as_str() {
                Some("response.completed") => Some("completed"),
                Some("response.failed") => Some("failed"),
                Some("response.incomplete") => {
                    super::events::responses_incomplete_reason(&value).or(Some("incomplete"))
                }
                _ => None,
            },
            _ => value["choices"][0]["finish_reason"].as_str(),
        };
        if let Some(reason) = reason.filter(|reason| !reason.is_empty()) {
            // Unknown provider values must not smuggle response text into logs.
            let safe = match reason {
                "stop"
                | "tool_calls"
                | "length"
                | "content_filter"
                | "function_call"
                | "end_turn"
                | "stop_sequence"
                | "tool_use"
                | "refusal"
                | "max_tokens"
                | "model_context_window_exceeded"
                | "pause_turn"
                | "completed"
                | "failed"
                | "incomplete"
                | "max_output_tokens"
                | "server_error"
                | "network_error"
                | "timeout"
                | "temporarily_unavailable" => reason,
                // A finish reason we do not recognize is still worth keeping:
                // the turn now completes instead of failing, so the reason is
                // the only trace of why. Only the identifier shape every
                // protocol uses is accepted; free text would leak content.
                other if bounded_reason_token(other) => other,
                _ => "unknown",
            };
            self.data.terminal_reason = Some(safe.into());
        }
    }

    pub fn report(&mut self) {
        if let Some(observer) = self.observer.take() {
            self.data.elapsed_ms = self.elapsed_ms();
            observer(self.data.clone());
        }
    }
}

impl Drop for AttemptDiagnostic {
    fn drop(&mut self) {
        self.report();
    }
}

/// Protocol stop reasons are short identifiers (`future_stop`, `pause_turn`).
/// Anything longer, or carrying other bytes, is provider free text and must
/// never reach a record.
pub(super) fn bounded_reason_token(reason: &str) -> bool {
    !reason.is_empty()
        && reason.len() <= 32
        && reason
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_reasons_are_bounded_and_dropped_consumers_report_once() {
        let records = Arc::new(std::sync::Mutex::new(Vec::new()));
        for (kind, frame, expected) in [
            (
                ProviderKind::OpenAiCompatible,
                json!({"choices":[{"finish_reason":"length"}]}),
                "length",
            ),
            (
                ProviderKind::Anthropic,
                json!({"type":"message_delta","delta":{"stop_reason":"pause_turn"}}),
                "pause_turn",
            ),
            (
                ProviderKind::OpenAiResponses,
                json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}),
                "max_output_tokens",
            ),
            (
                ProviderKind::OpenAiCompatible,
                json!({"choices":[{"finish_reason":"private response text"}]}),
                "unknown",
            ),
        ] {
            let captured = records.clone();
            let mut diagnostic = AttemptDiagnostic::new(
                Some(Arc::new(move |data| captured.lock().unwrap().push(data))),
                "s",
                1,
                1,
                kind,
            );
            diagnostic.value(kind, &frame);
            assert_eq!(diagnostic.data.terminal_reason.as_deref(), Some(expected));
            diagnostic.value(
                ProviderKind::OpenAiCompatible,
                &json!({"choices":[{"delta":{"content":"private body"}}]}),
            );
            diagnostic.report();
            // Explicit report followed by Drop never duplicates a record.
        }
        {
            let captured = records.clone();
            let _cancelled = AttemptDiagnostic::new(
                Some(Arc::new(move |data| captured.lock().unwrap().push(data))),
                "s",
                2,
                1,
                ProviderKind::OpenAiCompatible,
            );
        }
        let records = records.lock().unwrap();
        assert_eq!(records.len(), 5);
        assert_eq!(records[4].ending, "consumer_dropped");
        assert!(!serde_json::to_string(&*records)
            .unwrap()
            .contains("private"));
    }
}
