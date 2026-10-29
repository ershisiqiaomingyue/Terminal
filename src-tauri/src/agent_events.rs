use chrono::{Local, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

static EVENT_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static EVENT_INSTANCE: OnceLock<u64> = OnceLock::new();
static TOOL_GROUPS: OnceLock<Mutex<HashMap<(String, String), String>>> = OnceLock::new();
const EVENT_SCHEMA_VERSION: u64 = 1;

fn event_instance_id() -> u64 {
    *EVENT_INSTANCE.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or_default()
    })
}

fn tool_groups() -> &'static Mutex<HashMap<(String, String), String>> {
    TOOL_GROUPS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn begin_tool_group(session_id: &str, turn_id: &str) -> String {
    let group_sequence = EVENT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let group_id = format!("tool-group-{:x}-{}", event_instance_id(), group_sequence);
    if let Ok(mut groups) = tool_groups().lock() {
        groups.insert(
            (session_id.to_string(), turn_id.to_string()),
            group_id.clone(),
        );
    }
    group_id
}

pub fn end_tool_group(session_id: &str, turn_id: &str) {
    if let Ok(mut groups) = tool_groups().lock() {
        groups.remove(&(session_id.to_string(), turn_id.to_string()));
    }
}

fn current_tool_group(session_id: &str, turn_id: Option<&str>) -> Option<String> {
    let turn_id = turn_id.filter(|value| !value.trim().is_empty())?;
    tool_groups().lock().ok().and_then(|groups| {
        groups
            .get(&(session_id.to_string(), turn_id.to_string()))
            .cloned()
    })
}

/// Add the common replay envelope to every event emitted by the desktop
/// runtime. The sequence orders all channels in this process; the instance
/// prefix keeps persisted event IDs unique after an application restart.
pub fn stamp_payload(payload: Value) -> Value {
    stamp_payload_with_turn(payload, None)
}

pub fn stamp_payload_with_turn(mut payload: Value, turn_id: Option<&str>) -> Value {
    if let Value::Object(object) = &mut payload {
        object.insert(
            "schema_version".to_string(),
            Value::from(EVENT_SCHEMA_VERSION),
        );
        if let Some(turn_id) = turn_id.filter(|value| !value.trim().is_empty()) {
            object.insert("turn_id".to_string(), Value::String(turn_id.to_string()));
        }
        let event_type = object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if matches!(
            event_type,
            "tool_call_started"
                | "tool_call_input_delta"
                | "tool_call_finished"
                | "command_output"
                | "permission_requested"
                | "permission_decision"
        ) {
            if let Some(session_id) = object.get("session_id").and_then(Value::as_str) {
                if let Some(group_id) = current_tool_group(session_id, turn_id) {
                    object.insert("tool_group_id".to_string(), Value::String(group_id.clone()));
                    object.insert("parent_id".to_string(), Value::String(group_id));
                }
            }
            if let Some(call_id) = object.get("call_id").and_then(Value::as_str) {
                object.insert("item_id".to_string(), Value::String(call_id.to_string()));
            }
        }
        let sequence = EVENT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        object.insert(
            "event_id".to_string(),
            Value::String(format!(
                "agent-event-{:x}-{:x}",
                event_instance_id(),
                sequence
            )),
        );
        object.insert("sequence".to_string(), Value::from(sequence));
        object.insert(
            "timestamp".to_string(),
            Value::String(Local::now().to_rfc3339()),
        );
        object.insert(
            "timestamp_ms".to_string(),
            Value::from(Utc::now().timestamp_millis()),
        );
    }
    payload
}
