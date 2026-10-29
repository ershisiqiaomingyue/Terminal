use super::super::*;

pub(crate) fn context_item_id(turn_id: &str, payload: &Value) -> String {
    match payload.get("compaction_id").and_then(Value::as_str) {
        Some(id) => format!("context:{turn_id}:{id}"),
        None => format!("context:{turn_id}"),
    }
}
