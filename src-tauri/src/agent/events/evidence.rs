use super::super::*;

use super::payload::emit_agent_event_payload_for_turn;

pub(crate) fn emit_tool_execution_started(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    tool_group_id: &str,
    call_id: &str,
    tool_name: &str,
    input: &Value,
) {
    emit_agent_event_payload_for_turn(
        app,
        json!({
            "type": "tool_call_started",
            "session_id": session_id,
            "call_id": call_id,
            "tool_name": tool_name,
            "tool_group_id": tool_group_id,
            "status": "running",
            "input": input,
        }),
        Some(turn_id),
    );
}

pub(crate) fn attach_file_read_evidence_path_key(
    payload: &mut Value,
    roots: &[String],
    is_remote_target: bool,
) {
    let Some(object) = payload.as_object_mut() else {
        return;
    };
    if object.get("type").and_then(Value::as_str) != Some("tool_call_finished")
        || object.get("tool_name").and_then(Value::as_str) != Some("read_local_file")
    {
        return;
    }
    let evidence_path_key = object
        .get("input")
        .and_then(|input| file_read_path_key(input, roots, is_remote_target))
        .or_else(|| {
            object
                .get("output")
                .and_then(Value::as_object)
                .and_then(|output| output.get("path"))
                .and_then(Value::as_str)
                .filter(|path| !path.trim().is_empty())
                .map(|path| file_path_key(path, roots, is_remote_target))
        });
    if let Some(evidence_path_key) = evidence_path_key {
        object.insert(
            "evidence_path_key".to_string(),
            Value::String(evidence_path_key),
        );
    }
}
