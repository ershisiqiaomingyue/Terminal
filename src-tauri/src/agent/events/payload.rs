use super::super::*;

use super::super::process_checkpoint;
use super::lifecycle::enrich_tool_lifecycle_payload;
use super::persistence::persist_agent_payload;
use super::process::settle_thread_process;
use super::projection::emit_thread_snapshot_for_payload;

pub(crate) fn emit_agent_payload_for_turn(app: &AppHandle, payload: Value, turn_id: &str) {
    let payload = stamp_payload_with_turn(payload, Some(turn_id));
    persist_agent_payload(app, &payload);
    emit_thread_snapshot_for_payload(app, &payload);
    let _ = app.emit("agent:event", payload);
}

pub(crate) fn emit_agent_event_payload_for_turn(
    app: &AppHandle,
    payload: Value,
    turn_id: Option<&str>,
) {
    let mut payload = payload;
    if let Value::Object(object) = &mut payload {
        if object.get("type").and_then(Value::as_str) == Some("status")
            && object.get("status").and_then(Value::as_str) == Some("repetitive_output")
        {
            object.insert("terminalDiagnostic".to_string(), Value::Bool(true));
        }
        if object.get("type").and_then(Value::as_str) == Some("text_delta")
            || object.get("type").and_then(Value::as_str) == Some("reasoning_delta")
        {
            object
                .entry("origin".to_string())
                .or_insert_with(|| Value::String("model".to_string()));
        }
    }
    let mut payload = if turn_id.is_some() {
        stamp_payload_with_turn(payload, turn_id)
    } else {
        stamp_payload(payload)
    };
    enrich_tool_lifecycle_payload(app, &mut payload);
    persist_agent_payload(app, &payload);
    emit_thread_snapshot_for_payload(app, &payload);
    process_checkpoint::checkpoint_streamed_process(app, &payload);
    if payload.get("type").and_then(Value::as_str) == Some("tool_call_started") {
        if let (Some(session_id), Some(turn_id)) = (
            payload.get("session_id").and_then(Value::as_str),
            payload.get("turn_id").and_then(Value::as_str),
        ) {
            settle_thread_process(app, session_id, turn_id);
        }
    }
    let _ = app.emit("agent:event", payload);
}
