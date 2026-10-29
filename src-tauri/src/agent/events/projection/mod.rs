use super::super::*;
use serde_json::Value;

mod context_item;
mod finish;
mod inbound;
mod runtime;
mod usage;

pub(crate) use context_item::context_item_id;

pub(crate) fn emit_thread_snapshot_for_payload(app: &AppHandle, payload: &Value) {
    crate::plugin_host::observe(payload);
    let session_id = payload
        .get("session_id")
        .or_else(|| payload.get("sessionId"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    let turn_id = payload
        .get("turn_id")
        .or_else(|| payload.get("turnId"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    let event_type = payload.get("type").and_then(Value::as_str);
    let state = app.state::<AgentRuntimeState>();
    let number = |snake_case: &str, camel_case: &str| {
        payload
            .get(snake_case)
            .or_else(|| payload.get(camel_case))
            .and_then(Value::as_u64)
    };
    if let (Some(thread_id), Some(turn_id)) = (session_id, turn_id) {
        state.record_turn_progress(thread_id, turn_id);
    }
    let tool_call_count = match (session_id, turn_id, event_type) {
        (Some(thread_id), Some(turn_id), Some("tool_call_started")) => state
            .record_tool_call_started(
                thread_id,
                turn_id,
                payload
                    .get("call_id")
                    .or_else(|| payload.get("callId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .unwrap_or_else(|| state.tool_call_count(thread_id, turn_id)),
        (Some(thread_id), Some(turn_id), _) => state.tool_call_count(thread_id, turn_id),
        _ => 0,
    };
    if let Some(event) = inbound::inbound_events(session_id, turn_id, event_type, payload, &number)
    {
        let event_id = payload.get("event_id").and_then(Value::as_str);
        super::publication::emit_thread_snapshot_event(app, &state, event, event_id);
    }
    if let Some(event) = usage::context_usage(session_id, turn_id, event_type, payload, &number) {
        let event_id = payload.get("event_id").and_then(Value::as_str);
        super::publication::emit_thread_snapshot_event(app, &state, event, event_id);
    }
    if let Some(event) =
        runtime::runtime_event(session_id, turn_id, event_type, payload, tool_call_count)
    {
        super::publication::emit_thread_snapshot_event(app, &state, event, None);
    }
    finish::finish_tool_call(app, &state, session_id, turn_id, event_type, payload);
}
