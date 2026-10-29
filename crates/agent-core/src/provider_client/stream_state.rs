use super::*;

pub(super) const MAX_SSE_LINE_BYTES: usize = 8 * 1024 * 1024;
pub(super) const MAX_SSE_EVENT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Default)]
pub(super) struct SseDecoder {
    pub(super) line_buffer: Vec<u8>,
    pub(super) data_lines: Vec<String>,
    pub(super) data_bytes: usize,
}

impl SseDecoder {
    pub(super) fn push_chunk(&mut self, chunk: &[u8]) -> Result<Vec<String>, ProviderClientError> {
        self.line_buffer.extend_from_slice(chunk);
        if self.line_buffer.len() > MAX_SSE_LINE_BYTES && !self.line_buffer.contains(&b'\n') {
            return Err(ProviderClientError::Parse(
                "Provider SSE 行超过允许大小".to_string(),
            ));
        }

        let mut frames = Vec::new();
        while let Some(newline) = self.line_buffer.iter().position(|byte| *byte == b'\n') {
            let line_bytes = self.line_buffer.drain(..=newline).collect::<Vec<_>>();
            let line_bytes = &line_bytes[..line_bytes.len().saturating_sub(1)];
            let line_bytes = line_bytes.strip_suffix(&[b'\r']).unwrap_or(line_bytes);
            let line = std::str::from_utf8(line_bytes).map_err(|error| {
                ProviderClientError::Parse(format!("Provider SSE UTF-8 无效: {error}"))
            })?;
            if let Some(frame) = self.push_line(line)? {
                frames.push(frame);
            }
        }
        Ok(frames)
    }

    pub(super) fn finish(&mut self) -> Result<Vec<String>, ProviderClientError> {
        let mut frames = Vec::new();
        if !self.line_buffer.is_empty() {
            let line_bytes = std::mem::take(&mut self.line_buffer);
            let line = std::str::from_utf8(&line_bytes).map_err(|error| {
                ProviderClientError::Parse(format!("Provider SSE UTF-8 无效: {error}"))
            })?;
            if let Some(frame) = self.push_line(line)? {
                frames.push(frame);
            }
        }
        if let Some(frame) = self.take_frame() {
            frames.push(frame);
        }
        Ok(frames)
    }

    pub(super) fn push_line(&mut self, line: &str) -> Result<Option<String>, ProviderClientError> {
        if line.is_empty() {
            return Ok(self.take_frame());
        }
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.strip_prefix(' ').unwrap_or(data);
            self.data_bytes = self.data_bytes.saturating_add(data.len().saturating_add(1));
            if self.data_bytes > MAX_SSE_EVENT_BYTES {
                return Err(ProviderClientError::Parse(
                    "Provider SSE 事件超过允许大小".to_string(),
                ));
            }
            self.data_lines.push(data.to_string());
        }
        Ok(None)
    }

    pub(super) fn take_frame(&mut self) -> Option<String> {
        if self.data_lines.is_empty() {
            self.data_bytes = 0;
            return None;
        }
        self.data_bytes = 0;
        Some(self.data_lines.drain(..).collect::<Vec<_>>().join("\n"))
    }
}

#[derive(Default)]
pub(super) struct UsageSnapshot {
    pub(super) input_tokens: Option<u64>,
    pub(super) output_tokens: Option<u64>,
    pub(super) cached_input_tokens: Option<u64>,
    pub(super) cache_creation_input_tokens: Option<u64>,
    /// Parsed from the protocol usage object by `events::reasoning_tokens` and
    /// carried into `AgentEvent::Usage` so turn totals and persistence see it.
    /// `None` means no protocol reported the counter; nothing is inferred.
    pub(super) reasoning_tokens: Option<u64>,
}

impl UsageSnapshot {
    pub(super) fn update(&mut self, event: &AgentEvent) {
        let AgentEvent::Usage {
            input_tokens,
            output_tokens,
            cached_input_tokens,
            cache_creation_input_tokens,
            reasoning_tokens,
            ..
        } = event
        else {
            return;
        };
        if input_tokens.is_some() {
            self.input_tokens = *input_tokens;
        }
        if output_tokens.is_some() {
            self.output_tokens = *output_tokens;
        }
        if cached_input_tokens.is_some() {
            self.cached_input_tokens = *cached_input_tokens;
        }
        if cache_creation_input_tokens.is_some() {
            self.cache_creation_input_tokens = *cache_creation_input_tokens;
        }
        if reasoning_tokens.is_some() {
            self.reasoning_tokens = *reasoning_tokens;
        }
    }

    pub(super) fn into_event(self, session_id: &str) -> AgentEvent {
        AgentEvent::Usage {
            session_id: session_id.to_string(),
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cached_input_tokens: self.cached_input_tokens,
            cache_creation_input_tokens: self.cache_creation_input_tokens,
            reasoning_tokens: self.reasoning_tokens,
        }
    }
}

pub(super) fn remember_failed_usage(
    kind: ProviderKind,
    session_id: &str,
    data: &str,
    latest: &mut Option<UsageSnapshot>,
) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        return false;
    };
    let events = match kind {
        ProviderKind::Anthropic => parse_anthropic_visible_event(
            session_id,
            &json!({"type":"message_start", "message":{"usage":value["usage"]}}),
        )
        .unwrap_or_default(),
        ProviderKind::OpenAiResponses => {
            responses_terminal_events(session_id, &value["response"], false)
        }
        _ => parse_openai_event(kind, session_id, &json!({"usage":value["usage"]}))
            .unwrap_or_default(),
    };
    let mut reported = false;
    for event in events {
        if matches!(event, AgentEvent::Usage { .. }) {
            latest
                .get_or_insert_with(UsageSnapshot::default)
                .update(&event);
            reported = true;
        }
    }
    reported
}

pub(super) fn collect_stream_event(
    event: AgentEvent,
    latest_usage: &mut Option<UsageSnapshot>,
    saw_terminal_event: &mut bool,
) -> Vec<AgentEvent> {
    match event {
        AgentEvent::Usage { .. } => {
            latest_usage
                .get_or_insert_with(UsageSnapshot::default)
                .update(&event);
            Vec::new()
        }
        AgentEvent::Completed { .. } => {
            *saw_terminal_event = true;
            Vec::new()
        }
        event => vec![event],
    }
}

pub(super) fn remember_responses_call_id(
    kind: ProviderKind,
    data: &str,
    mapping: &mut HashMap<String, String>,
) {
    if kind != ProviderKind::OpenAiResponses {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        return;
    };
    if value.get("type").and_then(Value::as_str) != Some("response.output_item.added") {
        return;
    }
    let Some(item) = value.get("item") else {
        return;
    };
    if item.get("type").and_then(Value::as_str) != Some("function_call") {
        return;
    }
    let Some(item_id) = item.get("id").and_then(Value::as_str) else {
        return;
    };
    let Some(call_id) = item.get("call_id").and_then(Value::as_str) else {
        return;
    };
    mapping.insert(item_id.to_string(), call_id.to_string());
}

pub(super) fn normalize_responses_call_id(
    mut event: AgentEvent,
    mapping: &HashMap<String, String>,
) -> AgentEvent {
    if let AgentEvent::ToolCallInputDelta { call_id, .. } = &mut event {
        if let Some(mapped) = mapping.get(call_id) {
            *call_id = mapped.clone();
        }
    }
    event
}

pub(super) fn is_reasoning_channel(value: &Value) -> bool {
    matches!(
        value.get("channel").and_then(Value::as_str),
        Some("analysis" | "reasoning" | "thinking")
    )
}

#[derive(Default)]
pub(super) struct ResponsesOutputChannels {
    pub(super) items: HashMap<String, bool>,
    pub(super) indexes: HashMap<u64, bool>,
}

impl ResponsesOutputChannels {
    pub(super) fn remember(&mut self, kind: ProviderKind, data: &str) {
        if kind != ProviderKind::OpenAiResponses {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            return;
        };
        if value.get("type").and_then(Value::as_str) != Some("response.output_item.added") {
            return;
        }
        let item = &value["item"];
        let reasoning = is_reasoning_channel(item)
            || is_reasoning_channel(&value)
            || item.get("type").and_then(Value::as_str) == Some("reasoning");
        if let Some(id) = item.get("id").and_then(Value::as_str) {
            self.items.insert(id.to_string(), reasoning);
        }
        if let Some(index) = value.get("output_index").and_then(Value::as_u64) {
            self.indexes.insert(index, reasoning);
        }
    }

    pub(super) fn route(&self, kind: ProviderKind, data: &str, event: AgentEvent) -> AgentEvent {
        if kind != ProviderKind::OpenAiResponses || !matches!(event, AgentEvent::TextDelta { .. }) {
            return event;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            return event;
        };
        let explicit = value
            .get("channel")
            .and_then(Value::as_str)
            .map(|_| is_reasoning_channel(&value));
        let item_channel = value
            .get("item_id")
            .and_then(Value::as_str)
            .and_then(|id| self.items.get(id))
            .copied();
        let index_channel = value
            .get("output_index")
            .and_then(Value::as_u64)
            .and_then(|index| self.indexes.get(&index))
            .copied();
        if explicit.or(item_channel).or(index_channel).unwrap_or(false) {
            if let AgentEvent::TextDelta { session_id, text } = event {
                return AgentEvent::ReasoningDelta { session_id, text };
            }
        }
        event
    }
}
