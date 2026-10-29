use super::super::*;
use super::super::lifecycle::finished_tool_status;
use super::context_item_id;

fn text(payload: &Value, key: &str) -> String {
    payload
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn optional_text(payload: &Value, key: &str) -> Option<String> {
    payload.get(key).and_then(Value::as_str).map(str::to_string)
}

fn event_id(payload: &Value) -> String {
    text(payload, "event_id")
}

fn tool_call(
    thread_id: &str,
    turn_id: &str,
    payload: &Value,
    event_type: &str,
    number: &impl Fn(&str, &str) -> Option<u64>,
) -> ThreadEvent {
    let timestamp = || number("timestamp_ms", "timestampMs");
    let pending = event_type == "tool_call_pending";
    let finished = event_type == "tool_call_finished";
    ThreadEvent::ItemAdded {
        thread_id: thread_id.to_string(),
        turn_id: turn_id.to_string(),
        item: ThreadItem::ToolCall {
            id: text(payload, "call_id"),
            name: text(payload, "tool_name"),
            input: payload.get("input").cloned().unwrap_or(Value::Null),
            input_buffer: None,
            output: finished.then(|| payload.get("output").cloned().unwrap_or(Value::Null)),
            live_output: None,
            status: if finished {
                optional_text(payload, "status")
                    .unwrap_or_else(|| finished_tool_status(payload).to_string())
            } else if pending {
                "pending".to_string()
            } else {
                "running".to_string()
            },
            tool_group_id: optional_text(payload, "tool_group_id"),
            queued_at_ms: number("queued_at_ms", "queuedAtMs")
                .or_else(|| pending.then(timestamp).flatten()),
            started_at_ms: match event_type {
                "tool_call_pending" => None,
                "tool_call_started" => number("started_at_ms", "startedAtMs").or_else(timestamp),
                _ => number("started_at_ms", "startedAtMs"),
            },
            finished_at_ms: finished
                .then(|| number("finished_at_ms", "finishedAtMs").or_else(timestamp))
                .flatten(),
            is_error: finished
                && payload
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
        },
    }
}

fn usage(payload: &Value, number: &impl Fn(&str, &str) -> Option<u64>) -> ThreadUsageSnapshot {
    ThreadUsageSnapshot {
        provider_kind: payload
            .get("provider_kind")
            .or_else(|| payload.get("providerKind"))
            .and_then(|value| serde_json::from_value(value.clone()).ok()),
        input_tokens: number("input_tokens", "inputTokens"),
        output_tokens: number("output_tokens", "outputTokens"),
        generation_tokens: number("generation_tokens", "generationTokens"),
        generation_duration_ms: number("generation_duration_ms", "generationDurationMs"),
        generation_timing_source: payload
            .get("generation_timing_source")
            .or_else(|| payload.get("generationTimingSource"))
            .and_then(Value::as_str)
            .map(str::to_string),
        cached_input_tokens: number("cached_input_tokens", "cachedInputTokens"),
        latest_request_usage: payload
            .get("latestRequestUsage")
            .and_then(|value| serde_json::from_value(value.clone()).ok()),
        cache_creation_input_tokens: number(
            "cache_creation_input_tokens",
            "cacheCreationInputTokens",
        ),
        context_window: number("context_window", "contextWindow"),
        context_tokens: number("context_tokens", "contextTokens"),
        reserved_output_tokens: number("reserved_output_tokens", "reservedOutputTokens"),
        available_tokens: number("available_tokens", "availableTokens"),
        reasoning_tokens: number("reasoning_tokens", "reasoningTokens"),
    }
}

pub(crate) fn inbound_events(
    session_id: Option<&str>,
    turn_id: Option<&str>,
    event_type: Option<&str>,
    payload: &Value,
    number: &impl Fn(&str, &str) -> Option<u64>,
) -> Option<ThreadEvent> {
    let (Some(thread_id), Some(turn_id)) = (session_id, turn_id) else {
        return None;
    };
    let event_type = event_type?;
    let item = |item| ThreadEvent::ItemAdded {
        thread_id: thread_id.to_string(),
        turn_id: turn_id.to_string(),
        item,
    };
    Some(match event_type {
        "status"
            if payload.get("terminalDiagnostic").and_then(Value::as_bool) == Some(true)
                || payload.get("status").and_then(Value::as_str) == Some("repetitive_output") =>
        {
            let mut data = payload.clone();
            data["terminalDiagnostic"] = Value::Bool(true);
            item(ThreadItem::TimelineEvent {
                id: event_id(payload),
                data,
            })
        }
        "text_delta" => ThreadEvent::AgentDelta {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            item_id: format!("assistant:{turn_id}"),
            text: text(payload, "text"),
        },
        "permission_requested" => ThreadEvent::PermissionRequested {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            request_id: text(payload, "request_id"),
            tool_name: text(payload, "tool_name"),
            expires_at_ms: payload.get("expires_at_ms").and_then(Value::as_i64),
        },
        "permission_decision" => ThreadEvent::PermissionResolved {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            request_id: text(payload, "request_id"),
        },
        "task_plan" => item(ThreadItem::TaskPlan {
            id: format!("task-plan:{turn_id}"),
            goal: text(payload, "goal"),
            steps: payload
                .get("steps")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        }),
        "file_changes" => item(ThreadItem::FileChanges {
            id: format!("file-changes:{turn_id}"),
            data: payload.clone(),
        }),
        "context" => item(ThreadItem::Context {
            id: context_item_id(turn_id, payload),
            data: payload.clone(),
        }),
        "final_report" => item(ThreadItem::FinalReport {
            id: format!("final-report:{turn_id}"),
            data: payload.clone(),
        }),
        "reasoning_delta"
        | "steered"
        | "steer_accepted"
        | "file_snapshot_error"
        | "file_backup_error" => item(ThreadItem::TimelineEvent {
            id: event_id(payload),
            data: payload.clone(),
        }),
        "tool_call_pending" | "tool_call_started" | "tool_call_finished" => {
            tool_call(thread_id, turn_id, payload, event_type, number)
        }
        "tool_call_input_delta" => ThreadEvent::ToolCallInputDelta {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            call_id: text(payload, "call_id"),
            delta: text(payload, "delta"),
        },
        "command_output" => ThreadEvent::ToolCallOutput {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            call_id: payload
                .get("call_id")
                .and_then(Value::as_str)
                .filter(|call_id| !call_id.is_empty())?
                .to_string(),
            output: payload.clone(),
        },
        "completed" | "cancelled" | "error" => ThreadEvent::TurnCompleted {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            status: match event_type {
                "completed" => TurnStatus::Completed,
                "cancelled" => TurnStatus::Cancelled,
                _ => TurnStatus::Failed,
            },
            error: (event_type == "error")
                .then(|| optional_text(payload, "message"))
                .flatten(),
        },
        "usage" => ThreadEvent::UsageUpdated {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            usage: usage(payload, number),
        },
        _ => return None,
    })
}
