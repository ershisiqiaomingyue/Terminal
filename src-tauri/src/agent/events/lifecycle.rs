use super::super::*;

pub(crate) fn finished_tool_status(payload: &Value) -> &'static str {
    let output = payload.get("output").unwrap_or(&Value::Null);
    let code = output
        .get("code")
        .or_else(|| output.get("error").and_then(|error| error.get("code")))
        .and_then(Value::as_str);
    if code == Some("cancelled") {
        "cancelled"
    } else if output.get("skipped").and_then(Value::as_bool) == Some(true) {
        "skipped"
    } else if payload.get("is_error").and_then(Value::as_bool) == Some(true) {
        "error"
    } else {
        "done"
    }
}

/// 工具调用事件里带的工具名，供状态文案细化使用。
pub(crate) fn tool_name_in(payload: &Value) -> Option<&str> {
    payload
        .get("tool_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

pub(crate) fn enrich_tool_lifecycle_payload(app: &AppHandle, payload: &mut Value) {
    let event_type = payload
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if !matches!(
        event_type.as_deref(),
        Some("tool_call_pending" | "tool_call_started" | "tool_call_finished")
    ) {
        return;
    }
    let session_id = payload.get("session_id").and_then(Value::as_str);
    let call_id = payload.get("call_id").and_then(Value::as_str);
    let previous = session_id.zip(call_id).and_then(|(session_id, call_id)| {
        app.state::<AgentRuntimeState>()
            .read_thread_snapshot(session_id)
            .and_then(|snapshot| {
                snapshot.items.into_iter().find_map(|item| match item {
                    ThreadItem::ToolCall {
                        id,
                        tool_group_id,
                        queued_at_ms,
                        started_at_ms,
                        ..
                    } if id == call_id => Some((tool_group_id, queued_at_ms, started_at_ms)),
                    _ => None,
                })
            })
    });
    let timestamp_ms = payload.get("timestamp_ms").and_then(Value::as_u64);
    let status = (event_type.as_deref() == Some("tool_call_finished"))
        .then(|| finished_tool_status(payload));
    let Some(object) = payload.as_object_mut() else {
        return;
    };
    if let Some((tool_group_id, queued_at_ms, started_at_ms)) = previous {
        if let Some(tool_group_id) = tool_group_id {
            object
                .entry("tool_group_id".to_string())
                .or_insert(Value::String(tool_group_id));
        }
        if let Some(queued_at_ms) = queued_at_ms {
            object
                .entry("queued_at_ms".to_string())
                .or_insert(Value::from(queued_at_ms));
        }
        if let Some(started_at_ms) = started_at_ms {
            object
                .entry("started_at_ms".to_string())
                .or_insert(Value::from(started_at_ms));
        }
    }
    match event_type.as_deref() {
        Some("tool_call_pending") => {
            object
                .entry("queued_at_ms".to_string())
                .or_insert_with(|| Value::from(timestamp_ms.unwrap_or_default()));
            object
                .entry("status".to_string())
                .or_insert_with(|| Value::String("pending".to_string()));
        }
        Some("tool_call_started") => {
            object
                .entry("started_at_ms".to_string())
                .or_insert_with(|| Value::from(timestamp_ms.unwrap_or_default()));
            object.insert("status".to_string(), Value::String("running".to_string()));
        }
        Some("tool_call_finished") => {
            object
                .entry("finished_at_ms".to_string())
                .or_insert_with(|| Value::from(timestamp_ms.unwrap_or_default()));
            object.insert(
                "status".to_string(),
                Value::String(status.unwrap_or("done").to_string()),
            );
        }
        _ => {}
    }
}
