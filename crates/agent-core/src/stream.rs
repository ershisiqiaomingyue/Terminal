use crate::{merge_usage, AgentEvent, AgentUsage};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};

#[derive(Default)]
pub struct ToolStreamState {
    pub buffer: String,
    next_call_id: usize,
    tool_markup_buffer: String,
    next_tool_markup_call_id: usize,
    tool_markup_is_reasoning: bool,
}

pub type PendingToolCall = (String, (String, Value, String));

#[derive(Default)]
pub struct AgentRoundState {
    pub text: String,
    pub reasoning_content: String,
    pub output_tokens: Option<u64>,
    pub output_truncated: bool,
    pub usage: Option<AgentUsage>,
    pub pending_calls: Vec<PendingToolCall>,
    pub tool_ids_by_index: HashMap<usize, String>,
    pub responses_reasoning_items: Vec<Value>,
    pub anthropic_thinking_blocks: Vec<Value>,
    pub anthropic_content_blocks: Vec<Value>,
    anthropic_content_pending: HashMap<u64, Value>,
    anthropic_content_complete: BTreeMap<u64, Value>,
    anthropic_tool_json: HashMap<u64, String>,
    anthropic_replay_invalid: bool,
}

impl AgentRoundState {
    pub fn validate_anthropic_replay(&self) -> Result<(), &'static str> {
        if self.anthropic_replay_invalid
            || ((!self.anthropic_content_pending.is_empty()
                || !self.anthropic_content_complete.is_empty())
                && self.anthropic_content_blocks.is_empty())
        {
            return Err(
                "Claude 响应内容不完整或无法无损保留，已保留此前输出，未将本轮标记为成功。",
            );
        }
        Ok(())
    }

    pub fn ingest(&mut self, event: AgentEvent) -> (AgentEvent, bool) {
        let (event, mut should_emit) = canonicalize_tool_stream_event(
            event,
            &mut self.pending_calls,
            &mut self.tool_ids_by_index,
        );
        match &event {
            AgentEvent::TextDelta { text, .. } => self.text.push_str(text),
            AgentEvent::ReasoningDelta { text, .. } => self.reasoning_content.push_str(text),
            AgentEvent::OutputTruncated { .. } => {
                self.output_truncated = true;
                should_emit = false;
            }
            AgentEvent::Usage {
                input_tokens,
                output_tokens,
                cached_input_tokens,
                cache_creation_input_tokens,
                reasoning_tokens,
                ..
            } => {
                if output_tokens.is_some() {
                    self.output_tokens = *output_tokens;
                }
                merge_usage(
                    &mut self.usage,
                    AgentUsage {
                        input_tokens: *input_tokens,
                        output_tokens: *output_tokens,
                        generation_tokens: None,
                        generation_duration_ms: None,
                        generation_timing_source: None,
                        cached_input_tokens: *cached_input_tokens,
                        cache_creation_input_tokens: *cache_creation_input_tokens,
                        cache_stats_source: None,
                        cache_stats: None,
                        latest_request_usage: None,
                        provider_kind: None,
                        context_tokens: None,
                        context_window: None,
                        context_window_estimated: false,
                        reserved_output_tokens: None,
                        available_tokens: None,
                        input_cost: None,
                        output_cost: None,
                        total_cost: None,
                        currency: None,
                        reasoning_tokens: *reasoning_tokens,
                        cache_baseline_reset: false,
                    },
                );
            }
            AgentEvent::AnthropicThinkingFrame { frame, .. } => {
                self.ingest_anthropic_thinking(frame);
                should_emit = false;
            }
            AgentEvent::ResponsesReasoningItem { item, .. } => {
                let item_id = item.get("id").and_then(Value::as_str);
                if let Some(id) = item_id {
                    if let Some(existing) = self
                        .responses_reasoning_items
                        .iter_mut()
                        .find(|existing| existing.get("id").and_then(Value::as_str) == Some(id))
                    {
                        // The added event may not contain encrypted content;
                        // the done event is the authoritative replacement.
                        if item
                            .get("encrypted_content")
                            .is_some_and(|value| !value.is_null())
                        {
                            *existing = item.clone();
                        }
                    } else {
                        self.responses_reasoning_items.push(item.clone());
                    }
                } else {
                    self.responses_reasoning_items.push(item.clone());
                }
                should_emit = false;
            }
            _ => {}
        }
        (event, should_emit)
    }

    fn ingest_anthropic_thinking(&mut self, frame: &Value) {
        if frame["type"] == "message_stop" {
            if !self.anthropic_content_blocks.is_empty() {
                self.anthropic_replay_invalid = true;
            }
            if !self.anthropic_replay_invalid
                && self.anthropic_content_pending.is_empty()
                && self
                    .anthropic_content_complete
                    .keys()
                    .copied()
                    .eq(0..self.anthropic_content_complete.len() as u64)
            {
                self.anthropic_content_blocks =
                    self.anthropic_content_complete.values().cloned().collect();
            }
            return;
        }
        let Some(index) = frame.get("index").and_then(Value::as_u64) else {
            self.anthropic_replay_invalid = true;
            return;
        };
        match frame.get("type").and_then(Value::as_str) {
            Some("content_block_start") => {
                if let Some(block) = frame.get("content_block") {
                    if !block.is_object() || !block["type"].is_string() {
                        self.anthropic_replay_invalid = true;
                        return;
                    }
                    if self.anthropic_content_complete.contains_key(&index)
                        || self
                            .anthropic_content_pending
                            .insert(index, block.clone())
                            .is_some()
                    {
                        self.anthropic_replay_invalid = true;
                    }
                } else {
                    self.anthropic_replay_invalid = true;
                }
            }
            Some("content_block_delta") => {
                let Some(block) = self.anthropic_content_pending.get_mut(&index) else {
                    self.anthropic_replay_invalid = true;
                    return;
                };
                let delta = &frame["delta"];
                let field = match delta["type"].as_str() {
                    Some("text_delta") if block["type"] == "text" => "text",
                    Some("thinking_delta") if block["type"] == "thinking" => "thinking",
                    Some("signature_delta") if block["type"] == "thinking" => "signature",
                    Some("input_json_delta") if block["type"] == "tool_use" => {
                        if let Some(value) = delta["partial_json"].as_str() {
                            self.anthropic_tool_json
                                .entry(index)
                                .or_default()
                                .push_str(value);
                        } else {
                            self.anthropic_replay_invalid = true;
                        }
                        return;
                    }
                    Some("citations_delta") if block["type"] == "text" => {
                        if let Some(object) = block.as_object_mut() {
                            let citations = object.entry("citations").or_insert_with(|| json!([]));
                            if let Some(citations) = citations
                                .as_array_mut()
                                .filter(|_| delta["citation"].is_object())
                            {
                                citations.push(delta["citation"].clone());
                            } else {
                                self.anthropic_replay_invalid = true;
                            }
                        } else {
                            self.anthropic_replay_invalid = true;
                        }
                        return;
                    }
                    _ => {
                        self.anthropic_replay_invalid = true;
                        return;
                    }
                };
                if let Some(value) = delta[field].as_str() {
                    let target = block
                        .as_object_mut()
                        .unwrap()
                        .entry(field)
                        .or_insert_with(|| Value::String(String::new()));
                    if let Value::String(target) = target {
                        target.push_str(value);
                    } else {
                        self.anthropic_replay_invalid = true;
                    }
                } else {
                    self.anthropic_replay_invalid = true;
                }
            }
            Some("content_block_stop") => {
                if let Some(mut block) = self.anthropic_content_pending.remove(&index) {
                    if let Some(input) = self.anthropic_tool_json.remove(&index) {
                        match serde_json::from_str::<Value>(&input) {
                            Ok(input) if input.is_object() => block["input"] = input,
                            _ => self.anthropic_replay_invalid = true,
                        }
                    }
                    let complete = match block["type"].as_str() {
                        Some("thinking") => block["signature"]
                            .as_str()
                            .is_some_and(|value| !value.is_empty()),
                        Some("redacted_thinking") => block["data"]
                            .as_str()
                            .is_some_and(|value| !value.is_empty()),
                        Some("text") => block["text"].is_string(),
                        Some("tool_use") => {
                            block["id"].is_string()
                                && block["name"].is_string()
                                && block["input"].is_object()
                        }
                        _ => block.is_object() && block["type"].is_string(),
                    };
                    if complete {
                        if matches!(
                            block["type"].as_str(),
                            Some("thinking" | "redacted_thinking")
                        ) {
                            self.anthropic_thinking_blocks.push(block.clone());
                        }
                        self.anthropic_content_complete.insert(index, block);
                    } else {
                        self.anthropic_replay_invalid = true;
                    }
                } else {
                    self.anthropic_replay_invalid = true;
                }
            }
            _ => {}
        }
    }
}

const DSML_FUNCTION_CALLS_MARKER: &str = "<|DSML|function_calls>";
const DSML_TOOL_CALLS_MARKER: &str = "<|DSML|tool_calls>";
const DSML_INVOKE_PREFIX: &str = "<|DSML|invoke name=\"";
const DSML_INVOKE_END: &str = "</|DSML|invoke>";
const DSML_PARAMETER_PREFIX: &str = "<|DSML|parameter name=\"";
const DSML_PARAMETER_END: &str = "<|DSML|parameter>";
const TOOL_CALL_MARKER: &str = "<tool_call>";
const TOOL_CALL_END: &str = "</tool_call>";

fn normalize_dsml_delimiters(value: &str) -> String {
    value.replace('\u{FF5C}', "|").replace("||DSML||", "|DSML|")
}

fn marker_start(value: &str) -> Option<usize> {
    [
        DSML_FUNCTION_CALLS_MARKER,
        DSML_TOOL_CALLS_MARKER,
        DSML_INVOKE_PREFIX,
    ]
    .iter()
    .filter_map(|marker| value.find(marker))
    .min()
}

fn marker_prefix(value: &str) -> Option<usize> {
    value.find("<|DSML")
}

fn marker_at(value: &str, offset: usize) -> Option<(&'static str, usize)> {
    [
        DSML_FUNCTION_CALLS_MARKER,
        DSML_TOOL_CALLS_MARKER,
        DSML_INVOKE_PREFIX,
    ]
    .iter()
    .find_map(|marker| {
        value[offset..]
            .starts_with(marker)
            .then_some((*marker, marker.len()))
    })
}

fn block_end(value: &str, marker: &str, offset: usize) -> Option<(usize, usize)> {
    let markers: &[&str] = if marker == DSML_INVOKE_PREFIX {
        &[
            DSML_INVOKE_END,
            "</|DSML|function_calls>",
            "</|DSML|tool_calls>",
        ]
    } else {
        &[
            DSML_FUNCTION_CALLS_MARKER,
            DSML_TOOL_CALLS_MARKER,
            DSML_INVOKE_END,
            "</|DSML|function_calls>",
            "</|DSML|tool_calls>",
        ]
    };
    markers
        .iter()
        .filter_map(|marker| {
            value[offset..]
                .find(marker)
                .map(|relative| (offset + relative, marker.len()))
        })
        .min_by_key(|(start, _)| *start)
}

fn decode_value(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn parse_parameter(value: &str, is_string: bool) -> Value {
    let value = decode_value(value.trim());
    if is_string {
        Value::String(value)
    } else {
        serde_json::from_str(&value).unwrap_or(Value::String(value))
    }
}

fn parse_invokes(block: &str, session_id: &str, next_call_id: &mut usize) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    let mut cursor = 0;
    while let Some(relative_start) = block[cursor..].find(DSML_INVOKE_PREFIX) {
        let start = cursor + relative_start;
        let name_start = start + DSML_INVOKE_PREFIX.len();
        let Some(name_end_relative) = block[name_start..].find('"') else {
            break;
        };
        let name_end = name_start + name_end_relative;
        let Some(header_end_relative) = block[name_end..].find('>') else {
            break;
        };
        let body_start = name_end + header_end_relative + 1;
        let body_end = block[body_start..]
            .find(DSML_INVOKE_PREFIX)
            .map(|offset| body_start + offset)
            .unwrap_or(block.len());
        let body = &block[body_start..body_end];
        let mut input = Map::new();
        let mut parameter_cursor = 0;
        while let Some(relative_parameter) = body[parameter_cursor..].find(DSML_PARAMETER_PREFIX) {
            let parameter_start = parameter_cursor + relative_parameter;
            let parameter_name_start = parameter_start + DSML_PARAMETER_PREFIX.len();
            let Some(parameter_name_end_relative) = body[parameter_name_start..].find('"') else {
                break;
            };
            let parameter_name_end = parameter_name_start + parameter_name_end_relative;
            let Some(parameter_header_end_relative) = body[parameter_name_end..].find('>') else {
                break;
            };
            let parameter_header_end = parameter_name_end + parameter_header_end_relative;
            let parameter_value_start = parameter_header_end + 1;
            let Some(parameter_value_end_relative) =
                body[parameter_value_start..].find(DSML_PARAMETER_END)
            else {
                break;
            };
            let parameter_value_end = parameter_value_start + parameter_value_end_relative;
            let is_string =
                body[parameter_name_end..parameter_header_end].contains("string=\"true\"");
            input.insert(
                decode_value(&body[parameter_name_start..parameter_name_end]),
                parse_parameter(&body[parameter_value_start..parameter_value_end], is_string),
            );
            parameter_cursor = parameter_value_end + DSML_PARAMETER_END.len();
        }
        let tool_name = decode_value(&block[name_start..name_end]);
        let call_id = format!("dsml-call-{}", *next_call_id);
        *next_call_id += 1;
        events.push(AgentEvent::ToolCallStarted {
            session_id: session_id.to_string(),
            call_id,
            tool_index: None,
            tool_name,
            input: Value::Object(input),
        });
        cursor = body_end;
    }
    events
}

fn closing_marker_length(value: &str) -> Option<usize> {
    [
        DSML_INVOKE_END,
        "</|DSML|function_calls>",
        "</|DSML|tool_calls>",
    ]
    .iter()
    .find_map(|marker| value.starts_with(marker).then_some(marker.len()))
}

pub fn finish_text_delta(session_id: &str, state: &mut ToolStreamState) -> Vec<AgentEvent> {
    let remainder = std::mem::take(&mut state.buffer);
    if remainder.is_empty() || remainder.contains("<|DSML") || remainder.contains("</|DSML") {
        return Vec::new();
    }
    vec![AgentEvent::TextDelta {
        session_id: session_id.to_string(),
        text: remainder,
    }]
}

fn parse_tool_markup(block: &str) -> Option<(String, String)> {
    let value = serde_json::from_str::<Value>(block.trim().trim_matches('`').trim()).ok()?;
    let tool_name = value
        .get("name")
        .or_else(|| value.get("tool_name"))
        .or_else(|| value.get("tool"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())?
        .to_string();
    let arguments = value
        .get("arguments")
        .or_else(|| value.get("parameters"))
        .or_else(|| value.get("input"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let arguments = match arguments {
        Value::String(arguments) => arguments,
        value => serde_json::to_string(&value).ok()?,
    };
    Some((tool_name, arguments))
}

pub(crate) fn parse_plain_json_tool_calls(
    text: &str,
    allowed_tool_names: &[String],
) -> Option<Vec<(String, Value)>> {
    let value = serde_json::from_str::<Value>(text.trim()).ok()?;
    let calls = value.as_array()?;
    if calls.is_empty() {
        return None;
    }
    let mut parsed = Vec::with_capacity(calls.len());
    for call in calls {
        let object = call.as_object()?;
        let name = object
            .get("name")
            .or_else(|| object.get("tool_name"))
            .or_else(|| object.get("tool"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| allowed_tool_names.iter().any(|allowed| allowed == name))?
            .to_string();
        let arguments = object
            .get("arguments")
            .or_else(|| object.get("parameters"))
            .or_else(|| object.get("input"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let arguments = match arguments {
            Value::String(value) => serde_json::from_str(&value).ok()?,
            Value::Object(_) => arguments,
            _ => return None,
        };
        parsed.push((name, arguments));
    }
    Some(parsed)
}

fn markup_delta(
    session_id: &str,
    text: &str,
    is_reasoning: bool,
    state: &mut ToolStreamState,
) -> Vec<AgentEvent> {
    if state.tool_markup_buffer.is_empty() {
        state.tool_markup_is_reasoning = is_reasoning;
    }
    let is_reasoning = state.tool_markup_is_reasoning;
    state.tool_markup_buffer.push_str(text);
    let mut events = Vec::new();
    loop {
        let Some(start) = state.tool_markup_buffer.find(TOOL_CALL_MARKER) else {
            let partial_start = ["<tool", "<tool_call", "<tool_call>"]
                .iter()
                .filter_map(|prefix| state.tool_markup_buffer.rfind(prefix))
                .max();
            if let Some(partial_start) = partial_start {
                if partial_start > 0 {
                    let prefix = state.tool_markup_buffer[..partial_start].to_string();
                    state.tool_markup_buffer =
                        state.tool_markup_buffer[partial_start..].to_string();
                    events.push(if is_reasoning {
                        AgentEvent::ReasoningDelta {
                            session_id: session_id.to_string(),
                            text: prefix,
                        }
                    } else {
                        AgentEvent::TextDelta {
                            session_id: session_id.to_string(),
                            text: prefix,
                        }
                    });
                }
            } else if !state.tool_markup_buffer.is_empty() {
                let plain_text = std::mem::take(&mut state.tool_markup_buffer);
                events.push(if is_reasoning {
                    AgentEvent::ReasoningDelta {
                        session_id: session_id.to_string(),
                        text: plain_text,
                    }
                } else {
                    AgentEvent::TextDelta {
                        session_id: session_id.to_string(),
                        text: plain_text,
                    }
                });
            }
            break;
        };
        if start > 0 {
            let prefix = state.tool_markup_buffer[..start].to_string();
            state.tool_markup_buffer = state.tool_markup_buffer[start..].to_string();
            events.push(if is_reasoning {
                AgentEvent::ReasoningDelta {
                    session_id: session_id.to_string(),
                    text: prefix,
                }
            } else {
                AgentEvent::TextDelta {
                    session_id: session_id.to_string(),
                    text: prefix,
                }
            });
            continue;
        }
        let body_start = TOOL_CALL_MARKER.len();
        let Some(end_relative) = state.tool_markup_buffer[body_start..].find(TOOL_CALL_END) else {
            break;
        };
        let body_end = body_start + end_relative;
        let block = state.tool_markup_buffer[body_start..body_end].to_string();
        state.tool_markup_buffer =
            state.tool_markup_buffer[body_end + TOOL_CALL_END.len()..].to_string();
        let Some((tool_name, arguments)) = parse_tool_markup(&block) else {
            let text = format!("{TOOL_CALL_MARKER}{block}{TOOL_CALL_END}");
            events.push(if is_reasoning {
                AgentEvent::ReasoningDelta {
                    session_id: session_id.to_string(),
                    text,
                }
            } else {
                AgentEvent::TextDelta {
                    session_id: session_id.to_string(),
                    text,
                }
            });
            continue;
        };
        let call_id = format!("markup-call-{}", state.next_tool_markup_call_id);
        state.next_tool_markup_call_id = state.next_tool_markup_call_id.saturating_add(1);
        events.push(AgentEvent::ToolCallStarted {
            session_id: session_id.to_string(),
            call_id: call_id.clone(),
            tool_index: None,
            tool_name,
            input: json!({}),
        });
        events.push(AgentEvent::ToolCallInputDelta {
            session_id: session_id.to_string(),
            call_id,
            tool_index: None,
            delta: arguments,
        });
    }
    events
}

pub fn finish_tool_markup_delta(session_id: &str, state: &mut ToolStreamState) -> Vec<AgentEvent> {
    let remainder = std::mem::take(&mut state.tool_markup_buffer);
    if remainder.is_empty() {
        return Vec::new();
    }
    let is_reasoning = state.tool_markup_is_reasoning;
    let mut events = markup_delta(session_id, &remainder, is_reasoning, state);
    if !state.tool_markup_buffer.is_empty() {
        let unfinished = std::mem::take(&mut state.tool_markup_buffer);
        events.push(if is_reasoning {
            AgentEvent::ReasoningDelta {
                session_id: session_id.to_string(),
                text: unfinished,
            }
        } else {
            AgentEvent::TextDelta {
                session_id: session_id.to_string(),
                text: unfinished,
            }
        });
    }
    events
}

fn dsml_delta(session_id: &str, text: &str, state: &mut ToolStreamState) -> Vec<AgentEvent> {
    state.buffer.push_str(text);
    state.buffer = normalize_dsml_delimiters(&state.buffer);
    let mut events = Vec::new();
    loop {
        if let Some(length) = closing_marker_length(&state.buffer) {
            state.buffer = state.buffer[length..].to_string();
            continue;
        }
        let Some(start) = marker_start(&state.buffer) else {
            if let Some(partial_start) = marker_prefix(&state.buffer) {
                if partial_start > 0 {
                    let prefix = state.buffer[..partial_start].to_string();
                    state.buffer = state.buffer[partial_start..].to_string();
                    events.push(AgentEvent::TextDelta {
                        session_id: session_id.to_string(),
                        text: prefix,
                    });
                }
                break;
            }
            if !state.buffer.is_empty() {
                let plain_text = std::mem::take(&mut state.buffer);
                events.push(AgentEvent::TextDelta {
                    session_id: session_id.to_string(),
                    text: plain_text,
                });
            }
            break;
        };
        if start > 0 {
            let prefix = state.buffer[..start].to_string();
            state.buffer = state.buffer[start..].to_string();
            events.push(AgentEvent::TextDelta {
                session_id: session_id.to_string(),
                text: prefix,
            });
            continue;
        }
        let Some((marker, marker_len)) = marker_at(&state.buffer, 0) else {
            break;
        };
        let Some((end_start, end_length)) = block_end(&state.buffer, marker, marker_len) else {
            break;
        };
        let end = end_start + end_length;
        let block = state.buffer[..end].to_string();
        state.buffer = state.buffer[end..].to_string();
        events.extend(parse_invokes(&block, session_id, &mut state.next_call_id));
    }
    events
}

pub fn normalize_stream_event(event: AgentEvent, state: &mut ToolStreamState) -> Vec<AgentEvent> {
    match event {
        AgentEvent::TextDelta { session_id, text } => dsml_delta(&session_id, &text, state)
            .into_iter()
            .flat_map(|event| match event {
                AgentEvent::TextDelta { session_id, text } => {
                    markup_delta(&session_id, &text, false, state)
                }
                event => vec![event],
            })
            .collect(),
        AgentEvent::ReasoningDelta { session_id, text } => {
            markup_delta(&session_id, &text, true, state)
        }
        event => vec![event],
    }
}

pub fn tool_call_signature(tool_name: &str, input: &Value) -> String {
    serde_json::to_string(&json!({ "name": tool_name, "input": input }))
        .unwrap_or_else(|_| format!("{tool_name}:{input}"))
}

fn synthetic_call_id(call_id: &str) -> bool {
    call_id.starts_with("tool-index-")
}

pub fn canonicalize_tool_stream_event(
    mut event: AgentEvent,
    pending_calls: &mut Vec<PendingToolCall>,
    tool_ids_by_index: &mut HashMap<usize, String>,
) -> (AgentEvent, bool) {
    let should_emit = match &mut event {
        AgentEvent::ToolCallStarted {
            call_id,
            tool_index,
            tool_name,
            input,
            ..
        } => {
            let mut promoted_call_id = false;
            let provider_call_id = if call_id.trim().is_empty() {
                tool_index
                    .map(|index| format!("tool-index-{index}"))
                    .unwrap_or_else(|| format!("tool-call-{}", pending_calls.len()))
            } else {
                call_id.clone()
            };
            let canonical_call_id = if let Some(index) = *tool_index {
                if let Some(existing) = tool_ids_by_index.get(&index).cloned() {
                    if synthetic_call_id(&existing)
                        && !synthetic_call_id(&provider_call_id)
                        && existing != provider_call_id
                    {
                        if let Some(position) =
                            pending_calls.iter().position(|(id, _)| id == &existing)
                        {
                            pending_calls[position].0 = provider_call_id.clone();
                        }
                        tool_ids_by_index.insert(index, provider_call_id.clone());
                        promoted_call_id = true;
                        provider_call_id
                    } else {
                        existing
                    }
                } else {
                    tool_ids_by_index.insert(index, provider_call_id.clone());
                    provider_call_id
                }
            } else {
                provider_call_id
            };
            *call_id = canonical_call_id.clone();
            if let Some((_, (existing_name, existing_input, _))) = pending_calls
                .iter_mut()
                .find(|(id, _)| id == &canonical_call_id)
            {
                if !tool_name.is_empty() {
                    if existing_name.is_empty() || tool_name.starts_with(existing_name.as_str()) {
                        *existing_name = tool_name.clone();
                    } else if existing_name != tool_name {
                        existing_name.push_str(tool_name);
                    }
                }
                *tool_name = existing_name.clone();
                let input_has_values = input
                    .as_object()
                    .map(|values| !values.is_empty())
                    .unwrap_or(!input.is_null());
                if input_has_values {
                    *existing_input = input.clone();
                }
                promoted_call_id && !synthetic_call_id(call_id)
            } else {
                pending_calls.push((
                    canonical_call_id,
                    (tool_name.clone(), input.clone(), String::new()),
                ));
                !synthetic_call_id(call_id)
            }
        }
        AgentEvent::ToolCallInputDelta {
            call_id,
            tool_index,
            ..
        } => {
            if let Some(index) = *tool_index {
                if let Some(canonical_call_id) = tool_ids_by_index.get(&index) {
                    *call_id = canonical_call_id.clone();
                }
            } else if call_id.trim().is_empty() && pending_calls.len() == 1 {
                *call_id = pending_calls[0].0.clone();
            }
            !synthetic_call_id(call_id)
        }
        AgentEvent::Completed { .. } | AgentEvent::OutputTruncated { .. } => false,
        _ => true,
    };
    (event, should_emit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tool_markup_split_across_chunks() {
        let mut state = ToolStreamState::default();
        let first = normalize_stream_event(
            AgentEvent::TextDelta {
                session_id: "s".into(),
                text: "before <tool_".into(),
            },
            &mut state,
        );
        assert_eq!(first.len(), 1);
        let second = normalize_stream_event(
            AgentEvent::TextDelta {
                session_id: "s".into(),
                text:
                    "call>{\"name\":\"read_local_file\",\"arguments\":{\"path\":\"a\"}}</tool_call>"
                        .into(),
            },
            &mut state,
        );
        assert!(second.iter().any(|event| matches!(event, AgentEvent::ToolCallStarted { tool_name, .. } if tool_name == "read_local_file")));
    }
}
