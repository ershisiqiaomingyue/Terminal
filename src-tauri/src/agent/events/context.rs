use super::super::*;

use super::payload::emit_agent_event_payload_for_turn;

pub(crate) fn emit_agent_context_for_turn(app: &AppHandle, payload: Value, turn_id: Option<&str>) {
    let mut payload = payload;
    if let Value::Object(object) = &mut payload {
        object
            .entry("type".to_string())
            .or_insert_with(|| Value::String("context".to_string()));
        match object.get("compaction_status").and_then(Value::as_str) {
            Some("completed") => {
                object.insert("compaction_progress".into(), json!(100));
            }
            _ => {}
        }
    }
    if let Some(turn_id) = turn_id {
        emit_agent_payload_for_turn(app, payload, turn_id);
    } else {
        emit_agent_event_payload_for_turn(app, payload, None);
    }
}
