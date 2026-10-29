use super::*;

pub fn parse_sse_data(
    kind: ProviderKind,
    session_id: &str,
    data: &str,
) -> Result<Vec<AgentEvent>, ProviderClientError> {
    parse_sse_data_observed(kind, session_id, data, None, &mut None)
}

pub(super) fn parse_sse_data_observed(
    kind: ProviderKind,
    session_id: &str,
    data: &str,
    diagnostic: Option<&mut AttemptDiagnostic>,
    latest_usage: &mut Option<UsageSnapshot>,
) -> Result<Vec<AgentEvent>, ProviderClientError> {
    if data == "[DONE]" {
        return Ok(Vec::new());
    }
    let value: Value = serde_json::from_str(data)
        .map_err(|error| ProviderClientError::Parse(error.to_string()))?;
    let reasoning_tokens = frame_reasoning_tokens(&value);
    if let Some(diagnostic) = diagnostic {
        diagnostic.value(kind, &value);
        diagnostic.record_reasoning_tokens(reasoning_tokens);
    }
    // Recorded before any protocol judgement: a frame that fails later already
    // reported its usage, and that report must not be lost with it.
    if let Some(reasoning_tokens) = reasoning_tokens {
        latest_usage
            .get_or_insert_with(UsageSnapshot::default)
            .reasoning_tokens = Some(reasoning_tokens);
    }
    match kind {
        ProviderKind::Anthropic => parse_anthropic_event(session_id, &value),
        ProviderKind::OpenAi
        | ProviderKind::OpenAiCompatible
        | ProviderKind::DeepSeek
        | ProviderKind::Kimi
        | ProviderKind::Glm
        | ProviderKind::MiMo => parse_openai_event(kind, session_id, &value),
        ProviderKind::OpenAiResponses => parse_responses_event(session_id, &value),
    }
}

/// Reasoning-token details live in a protocol-specific side channel of the
/// usage object: OpenAI chat reports `completion_tokens_details.reasoning_tokens`
/// and the Responses shape reports `output_tokens_details.reasoning_tokens`.
/// A protocol that reports neither (Anthropic) yields `None`; no value is ever
/// inferred from the other token counts.
pub(super) fn reasoning_tokens(usage: &Value) -> Option<u64> {
    nested_u64(
        usage,
        &["completion_tokens_details", "output_tokens_details"],
        &["reasoning_tokens"],
    )
}

/// A decoded frame carries its usage object at the top level (OpenAI chat and
/// Anthropic `message_delta`), under `message` (Anthropic `message_start`), or
/// under `response` (OpenAI Responses terminal frames). The first reported
/// reasoning-token count wins.
fn frame_reasoning_tokens(value: &Value) -> Option<u64> {
    [
        value.get("usage"),
        value
            .get("message")
            .and_then(|message| message.get("usage")),
        value
            .get("response")
            .and_then(|response| response.get("usage")),
    ]
    .into_iter()
    .flatten()
    .find_map(reasoning_tokens)
}

/// Chat-completions `delta.content` is normally a string, but relays and the
/// Responses-compatible shape may send an array of content blocks. Concatenate
/// the `text` of the recognized text blocks, ignore every other block, and
/// treat an empty array as an empty string so a block-shaped reply is not
/// silently dropped.
pub(super) fn content_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    let blocks = content.as_array()?;
    let mut text = String::new();
    for block in blocks {
        let block_type = block.get("type").and_then(Value::as_str);
        // A block that carries `text` but omits the type is still text: some
        // relays drop it, and non-text blocks (image_url, tool_use) never carry
        // the field.
        let known_text = matches!(block_type, Some("text" | "output_text" | "input_text"));
        if let Some(field) = block
            .get("text")
            .and_then(Value::as_str)
            .filter(|_| known_text || block_type.is_none())
        {
            text.push_str(field);
        }
    }
    Some(text)
}

pub(super) fn parse_anthropic_event(
    session_id: &str,
    value: &Value,
) -> Result<Vec<AgentEvent>, ProviderClientError> {
    let mut events = parse_anthropic_visible_event(session_id, value)?;
    let private = match value["type"].as_str() {
        Some(
            "content_block_start" | "content_block_delta" | "content_block_stop" | "message_stop",
        ) => true,
        _ => false,
    };
    if private {
        events.push(AgentEvent::AnthropicThinkingFrame {
            session_id: session_id.to_string(),
            frame: value.clone(),
        });
    }
    Ok(events)
}

pub(super) fn parse_anthropic_visible_event(
    session_id: &str,
    value: &Value,
) -> Result<Vec<AgentEvent>, ProviderClientError> {
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match event_type {
        "error" => {
            return Err(ProviderClientError::Parse(provider_error_value_message(
                value,
                "Anthropic 流式响应返回错误",
            )))
        }
        "message_start" => {
            let usage = value
                .get("message")
                .and_then(|message| message.get("usage"))
                .unwrap_or(&Value::Null);
            if usage.is_object() {
                return Ok(vec![AgentEvent::Usage {
                    session_id: session_id.to_string(),
                    input_tokens: usage.get("input_tokens").and_then(Value::as_u64),
                    output_tokens: usage.get("output_tokens").and_then(Value::as_u64),
                    cached_input_tokens: usage
                        .get("cache_read_input_tokens")
                        .and_then(Value::as_u64),
                    cache_creation_input_tokens: usage
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_u64),
                    reasoning_tokens: reasoning_tokens(usage),
                }]);
            }
        }
        "content_block_start" => {
            let block = value.get("content_block").unwrap_or(&Value::Null);
            if block["type"] == "text" {
                if let Some(text) = block["text"].as_str().filter(|text| !text.is_empty()) {
                    return Ok(vec![AgentEvent::TextDelta {
                        session_id: session_id.to_string(),
                        text: text.to_string(),
                    }]);
                }
            }
            if block.get("type").and_then(Value::as_str) == Some("thinking") {
                if let Some(text) = block.get("thinking").and_then(Value::as_str) {
                    return Ok(vec![AgentEvent::ReasoningDelta {
                        session_id: session_id.to_string(),
                        text: text.to_string(),
                    }]);
                }
            }
            if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                let tool_index = value
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(|index| index as usize);
                let call_id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| tool_index.map(|index| format!("tool-index-{index}")))
                    .unwrap_or_default();
                return Ok(vec![AgentEvent::ToolCallStarted {
                    session_id: session_id.to_string(),
                    call_id: call_id.clone(),
                    tool_index,
                    tool_name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    input: block.get("input").cloned().unwrap_or_else(|| json!({})),
                }]);
            }
        }
        "content_block_delta" => {
            let delta = value.get("delta").unwrap_or(&Value::Null);
            match delta.get("type").and_then(Value::as_str) {
                Some("text_delta") => {
                    return Ok(vec![AgentEvent::TextDelta {
                        session_id: session_id.to_string(),
                        text: delta
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    }]);
                }
                Some("thinking_delta") => {
                    return Ok(vec![AgentEvent::ReasoningDelta {
                        session_id: session_id.to_string(),
                        text: delta
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    }]);
                }
                Some("input_json_delta") => {
                    let tool_index = value
                        .get("index")
                        .and_then(Value::as_u64)
                        .map(|index| index as usize);
                    return Ok(vec![AgentEvent::ToolCallInputDelta {
                        session_id: session_id.to_string(),
                        call_id: tool_index
                            .map(|index| format!("tool-index-{index}"))
                            .unwrap_or_default(),
                        tool_index,
                        delta: delta
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    }]);
                }
                _ => {}
            }
        }
        "message_stop" => {
            return Ok(vec![AgentEvent::Completed {
                session_id: session_id.to_string(),
            }]);
        }
        "message_delta" => {
            let mut events = Vec::new();
            match value["delta"]["stop_reason"].as_str() {
                Some("max_tokens") => events.push(AgentEvent::OutputTruncated {
                    session_id: session_id.to_string(),
                }),
                Some("model_context_window_exceeded") => return Err(ProviderClientError::Incomplete {
                    reason: "model_context_window_exceeded".into(),
                    message: "Claude 已达到模型上下文上限；已保留此前内容，需要压缩上下文后继续。".into(),
                }),
                Some("pause_turn") => return Err(ProviderClientError::Incomplete {
                    reason: "pause_turn".into(),
                    message: "Claude 的服务端工具流程已暂停，回复尚未完成；当前连接尚不支持该流程的自动续接。".into(),
                }),
                Some("end_turn" | "stop_sequence" | "tool_use" | "refusal") | None => {}
                Some(reason) => return Err(ProviderClientError::Incomplete {
                    reason: reason.into(),
                    message: format!("Claude 返回了未知终止原因（{reason}）；未将此次回复标记为完成。"),
                }),
            }
            let usage = value.get("usage").unwrap_or(&Value::Null);
            if usage.is_object() {
                events.push(AgentEvent::Usage {
                    session_id: session_id.to_string(),
                    input_tokens: usage.get("input_tokens").and_then(Value::as_u64),
                    output_tokens: usage.get("output_tokens").and_then(Value::as_u64),
                    cached_input_tokens: usage
                        .get("cache_read_input_tokens")
                        .and_then(Value::as_u64),
                    cache_creation_input_tokens: usage
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_u64),
                    reasoning_tokens: reasoning_tokens(usage),
                });
            }
            return Ok(events);
        }
        _ => {}
    }
    Ok(Vec::new())
}

pub(super) fn parse_openai_event(
    kind: ProviderKind,
    session_id: &str,
    value: &Value,
) -> Result<Vec<AgentEvent>, ProviderClientError> {
    if value.get("error").is_some_and(|error| !error.is_null()) {
        return Err(ProviderClientError::Parse(provider_error_value_message(
            value,
            "OpenAI-compatible Provider 返回错误",
        )));
    }
    let mut events = Vec::new();
    if let Some(usage) = value.get("usage").filter(|usage| !usage.is_null()) {
        let cached_input_tokens = cached_input_tokens(usage);
        let cache_creation_input_tokens = cache_creation_input_tokens(usage);
        let uncached_input_tokens = uncached_input_tokens(usage);
        let input_tokens = ProviderCapabilities::for_kind(kind).input_tokens_from_usage(
            first_u64(usage, &["prompt_tokens", "input_tokens"]),
            cached_input_tokens,
            cache_creation_input_tokens,
            uncached_input_tokens,
        );
        events.push(AgentEvent::Usage {
            session_id: session_id.to_string(),
            input_tokens,
            output_tokens: first_u64(usage, &["completion_tokens", "output_tokens"]),
            cached_input_tokens,
            cache_creation_input_tokens,
            reasoning_tokens: reasoning_tokens(usage),
        });
    }
    let choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first());
    let Some(choice) = choice else {
        return Ok(events);
    };
    let delta = choice.get("delta").unwrap_or(&Value::Null);
    if let Some(text) = content_text(delta.get("content").unwrap_or(&Value::Null)) {
        let reasoning_channel = matches!(
            delta.get("channel").and_then(Value::as_str),
            Some("analysis" | "reasoning" | "thinking")
        );
        events.push(if reasoning_channel {
            AgentEvent::ReasoningDelta {
                session_id: session_id.to_string(),
                text: text.to_string(),
            }
        } else {
            AgentEvent::TextDelta {
                session_id: session_id.to_string(),
                text: text.to_string(),
            }
        });
    }
    if let Some(reasoning) = delta
        .get("reasoning_content")
        .and_then(Value::as_str)
        .or_else(|| delta.get("reasoning").and_then(Value::as_str))
        .or_else(|| delta.get("thinking").and_then(Value::as_str))
    {
        events.push(AgentEvent::ReasoningDelta {
            session_id: session_id.to_string(),
            text: reasoning.to_string(),
        });
    }
    if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
        for tool_call in tool_calls {
            let function = tool_call.get("function").unwrap_or(&Value::Null);
            let tool_index = tool_call
                .get("index")
                .and_then(Value::as_u64)
                .map(|index| index as usize);
            let call_id = tool_call
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| tool_index.map(|index| format!("tool-index-{index}")))
                .unwrap_or_default();
            if function.get("name").is_some() || tool_call.get("id").is_some() {
                events.push(AgentEvent::ToolCallStarted {
                    session_id: session_id.to_string(),
                    call_id: call_id.clone(),
                    tool_index,
                    tool_name: function
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    input: json!({}),
                });
            }
            if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                events.push(AgentEvent::ToolCallInputDelta {
                    session_id: session_id.to_string(),
                    call_id,
                    tool_index,
                    delta: arguments.to_string(),
                });
            }
        }
    }
    match choice
        .get("finish_reason")
        .filter(|reason| !reason.is_null())
        // Compatible relays may use an empty string on streaming deltas.
        // Only a later explicit stop/tool_calls/length finishes the response.
        .filter(|reason| reason.as_str() != Some(""))
    {
        Some(reason) if reason == "length" => events.push(AgentEvent::OutputTruncated {
            session_id: session_id.to_string(),
        }),
        Some(reason) if reason == "stop" || reason == "tool_calls" => {}
        // Explicit refusals keep failing the attempt: the provider stopped
        // without a usable answer, so reporting a normal completion would hide
        // filtered or unsupported output from the caller.
        Some(reason) if reason == "content_filter" || reason == "function_call" => {
            let reason = reason.as_str().unwrap_or_default();
            return Err(ProviderClientError::Incomplete {
                reason: reason.into(),
                message: if reason == "content_filter" {
                    "模型回复被服务端内容过滤中止，已保留此前内容。".into()
                } else {
                    "模型返回了尚不支持的旧版 function_call 终态；未将此次回复标记为完成。".into()
                },
            });
        }
        // Any other reason is provider-specific or newly introduced, but the
        // provider still declared the response finished. Finish normally and
        // let the bounded `terminal_reason` diagnostic record what it said,
        // instead of failing a turn that already produced content.
        Some(_) => {}
        None => return Ok(events),
    }
    events.push(AgentEvent::Completed {
        session_id: session_id.to_string(),
    });
    Ok(events)
}

pub(super) fn uncached_input_tokens(usage: &Value) -> Option<u64> {
    nested_u64(
        usage,
        &["prompt_tokens_details", "input_tokens_details"],
        &[
            "prompt_cache_miss_tokens",
            "cache_miss_tokens",
            "uncached_tokens",
            "uncached_input_tokens",
        ],
    )
    .or_else(|| {
        first_u64(
            usage,
            &[
                "prompt_cache_miss_tokens",
                "cache_miss_tokens",
                "uncached_prompt_tokens",
                "uncached_input_tokens",
            ],
        )
    })
}

pub(super) fn cached_input_tokens(usage: &Value) -> Option<u64> {
    nested_u64(
        usage,
        &["prompt_tokens_details", "input_tokens_details"],
        &[
            "cached_tokens",
            "prompt_cache_hit_tokens",
            "cache_read_input_tokens",
            "cache_read_tokens",
        ],
    )
    .or_else(|| {
        first_u64(
            usage,
            &[
                "cached_tokens",
                "cached_prompt_tokens",
                "prompt_cache_hit_tokens",
                "cache_read_input_tokens",
                "cache_read_tokens",
            ],
        )
    })
}

pub(super) fn cache_creation_input_tokens(usage: &Value) -> Option<u64> {
    nested_u64(
        usage,
        &["prompt_tokens_details", "input_tokens_details"],
        &["cache_creation_input_tokens", "cache_write_input_tokens"],
    )
    .or_else(|| {
        first_u64(
            usage,
            &["cache_creation_input_tokens", "cache_write_input_tokens"],
        )
    })
}

pub(super) fn first_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|value| {
            value.as_u64().or_else(|| {
                value
                    .as_str()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .and_then(|text| text.parse::<u64>().ok())
            })
        })
    })
}

pub(super) fn nested_u64(value: &Value, objects: &[&str], keys: &[&str]) -> Option<u64> {
    objects.iter().find_map(|object| {
        value
            .get(*object)
            .and_then(|details| first_u64(details, keys))
    })
}

pub(super) fn parse_responses_event(
    session_id: &str,
    value: &Value,
) -> Result<Vec<AgentEvent>, ProviderClientError> {
    if value.get("error").is_some_and(|error| !error.is_null()) {
        return Err(ProviderClientError::Parse(provider_error_value_message(
            value,
            "Responses API 返回错误",
        )));
    }
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match event_type {
        "response.output_text.delta" if !is_reasoning_channel(value) => {
            Ok(vec![AgentEvent::TextDelta {
                session_id: session_id.to_string(),
                text: value
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }])
        }
        "response.output_text.delta"
        | "response.reasoning_text.delta"
        | "response.reasoning_summary_text.delta" => Ok(vec![AgentEvent::ReasoningDelta {
            session_id: session_id.to_string(),
            text: value
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }]),
        "response.output_item.added" => {
            let item = value.get("item").unwrap_or(&Value::Null);
            if item.get("type").and_then(Value::as_str) == Some("reasoning") {
                return Ok(vec![AgentEvent::ResponsesReasoningItem {
                    session_id: session_id.to_string(),
                    item: item.clone(),
                }]);
            }
            if item.get("type").and_then(Value::as_str) != Some("function_call") {
                return Ok(Vec::new());
            }
            Ok(vec![AgentEvent::ToolCallStarted {
                session_id: session_id.to_string(),
                call_id: item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                tool_index: None,
                tool_name: item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                input: item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .and_then(|value| serde_json::from_str(value).ok())
                    .unwrap_or_else(|| json!({})),
            }])
        }
        "response.output_item.done" => {
            let item = value.get("item").unwrap_or(&Value::Null);
            if item.get("type").and_then(Value::as_str) == Some("reasoning") {
                return Ok(vec![AgentEvent::ResponsesReasoningItem {
                    session_id: session_id.to_string(),
                    item: item.clone(),
                }]);
            }
            Ok(Vec::new())
        }
        "response.function_call_arguments.delta" => Ok(vec![AgentEvent::ToolCallInputDelta {
            session_id: session_id.to_string(),
            call_id: value
                .get("call_id")
                .or_else(|| value.get("item_id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            tool_index: None,
            delta: value
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }]),
        "response.completed" => Ok(responses_terminal_events(
            session_id,
            value.get("response").unwrap_or(&Value::Null),
            false,
        )),
        "response.failed" => Err(ProviderClientError::Parse(provider_error_value_message(
            value,
            "Responses API 返回失败响应",
        ))),
        "response.incomplete"
            if responses_incomplete_reason(value) == Some("max_output_tokens") =>
        {
            Ok(responses_terminal_events(
                session_id,
                value.get("response").unwrap_or(&Value::Null),
                true,
            ))
        }
        "response.incomplete" => {
            let reason = responses_incomplete_reason(value)
                .unwrap_or("unknown")
                .to_string();
            Err(ProviderClientError::Incomplete {
                message: responses_incomplete_message(value),
                reason,
            })
        }
        _ => Ok(Vec::new()),
    }
}

pub(super) fn responses_terminal_events(
    session_id: &str,
    response: &Value,
    output_truncated: bool,
) -> Vec<AgentEvent> {
    let usage = response.get("usage").unwrap_or(&Value::Null);
    let mut events = Vec::new();
    if usage.is_object() {
        let cached_input_tokens = cached_input_tokens(usage);
        let cache_creation_input_tokens = cache_creation_input_tokens(usage);
        events.push(AgentEvent::Usage {
            session_id: session_id.to_string(),
            input_tokens: ProviderCapabilities::for_kind(ProviderKind::OpenAiResponses)
                .input_tokens_from_usage(
                    first_u64(usage, &["input_tokens", "prompt_tokens"]),
                    cached_input_tokens,
                    cache_creation_input_tokens,
                    uncached_input_tokens(usage),
                ),
            output_tokens: first_u64(usage, &["output_tokens", "completion_tokens"]),
            cached_input_tokens,
            cache_creation_input_tokens,
            reasoning_tokens: reasoning_tokens(usage),
        });
    }
    if output_truncated {
        events.push(AgentEvent::OutputTruncated {
            session_id: session_id.to_string(),
        });
    }
    events.push(AgentEvent::Completed {
        session_id: session_id.to_string(),
    });
    events
}

pub(super) fn responses_incomplete_reason(value: &Value) -> Option<&str> {
    value
        .get("response")
        .and_then(|response| response.get("incomplete_details"))
        .or_else(|| value.get("incomplete_details"))
        .and_then(|details| details.get("reason"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
}

pub(super) fn responses_incomplete_message(value: &Value) -> String {
    let mut message = provider_error_value_message(value, "Responses API 返回未完成响应");
    if let Some(reason) = responses_incomplete_reason(value) {
        let reason = match reason {
            "content_filter" => "内容被 Provider 的安全策略截断".to_string(),
            "max_output_tokens" => "模型输出达到上限".to_string(),
            other => format!("Provider 返回 {other}"),
        };
        message.push_str(&format!("（原因：{reason}）"));
    }
    message
}
