use serde_json::Value;

pub(crate) const MAX_SNAPSHOT_TOOL_OUTPUT_BYTES: usize = 128 * 1024;

pub(super) fn merge_live_tool_output(current: &mut Option<Value>, incoming: Value) {
    let Value::Object(mut next) = incoming else {
        *current = Some(incoming);
        return;
    };
    let mut previous = current
        .take()
        .and_then(|value| match value {
            Value::Object(object) => Some(object),
            _ => None,
        })
        .unwrap_or_default();
    let mut text = previous
        .remove("text")
        .and_then(|value| match value {
            Value::String(text) => Some(text),
            _ => None,
        })
        .unwrap_or_default();
    let mut truncated = previous
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !truncated && next.get("phase").and_then(Value::as_str) == Some("chunk") {
        if let Some(chunk) = next.get("text").and_then(Value::as_str) {
            let remaining = MAX_SNAPSHOT_TOOL_OUTPUT_BYTES.saturating_sub(text.len());
            let mut end = chunk.len().min(remaining);
            while !chunk.is_char_boundary(end) {
                end -= 1;
            }
            text.push_str(&chunk[..end]);
            truncated = end < chunk.len();
        }
    }
    // Keep only the latest metadata/progress, but preserve bounded log text.
    next.insert("text".into(), Value::String(text));
    next.insert("truncated".into(), Value::Bool(truncated));
    *current = Some(Value::Object(next));
}
