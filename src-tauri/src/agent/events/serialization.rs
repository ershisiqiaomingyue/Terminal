use super::super::*;

use super::evidence::attach_file_read_evidence_path_key;
use super::payload::emit_agent_event_payload_for_turn;

pub(crate) fn serialized_agent_event(event: &AgentEvent) -> Value {
    serde_json::to_value(event).unwrap_or_else(|_| {
        json!({
            "type": "error",
            "message": "Agent 事件序列化失败",
        })
    })
}

pub(crate) fn serialized_model_agent_event(event: &AgentEvent, tool_group_id: &str) -> Value {
    let mut payload = serialized_agent_event(event);
    if let Value::Object(object) = &mut payload {
        match object.get("type").and_then(Value::as_str) {
            Some("tool_call_started") => {
                object.insert(
                    "type".to_string(),
                    Value::String("tool_call_pending".to_string()),
                );
                object.insert("status".to_string(), Value::String("pending".to_string()));
                object.insert(
                    "tool_group_id".to_string(),
                    Value::String(tool_group_id.to_string()),
                );
            }
            Some("tool_call_input_delta") => {
                object.insert(
                    "tool_group_id".to_string(),
                    Value::String(tool_group_id.to_string()),
                );
            }
            _ => {}
        }
    }
    payload
}

pub(crate) fn emit_model_agent_event_for_turn(
    app: &AppHandle,
    event: &AgentEvent,
    turn_id: &str,
    tool_group_id: &str,
) {
    emit_agent_event_payload_for_turn(
        app,
        serialized_model_agent_event(event, tool_group_id),
        Some(turn_id),
    );
}

pub(crate) fn emit_agent_event_for_turn(
    app: &AppHandle,
    event: &AgentEvent,
    turn_id: Option<&str>,
) {
    let payload = serialized_agent_event(event);
    emit_agent_event_payload_for_turn(app, payload, turn_id);
}

pub(crate) fn emit_agent_event_for_turn_with_file_evidence_path_key(
    app: &AppHandle,
    event: &AgentEvent,
    turn_id: Option<&str>,
    roots: &[String],
    is_remote_target: bool,
) {
    let mut payload = serialized_agent_event(event);
    attach_file_read_evidence_path_key(&mut payload, roots, is_remote_target);
    emit_agent_event_payload_for_turn(app, payload, turn_id);
}

pub(crate) fn emit_agent_event_for_turn_with_call_id(
    app: &AppHandle,
    event: &AgentEvent,
    turn_id: Option<&str>,
    call_id: &str,
) {
    let mut payload = serde_json::to_value(event).unwrap_or_else(|_| {
        json!({
            "type": "error",
            "message": "Agent 事件序列化失败",
        })
    });
    if let Value::Object(object) = &mut payload {
        object.insert("call_id".to_string(), Value::String(call_id.to_string()));
    }
    emit_agent_event_payload_for_turn(app, payload, turn_id);
}
