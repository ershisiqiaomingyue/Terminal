use super::super::*;

pub(crate) fn is_process_boundary(item: &ThreadItem) -> bool {
    match item {
        ThreadItem::ToolCall { .. } => true,
        ThreadItem::TimelineEvent { data, .. } => {
            data.get("type").and_then(Value::as_str) == Some("reasoning_delta")
                && data.get("origin").and_then(Value::as_str) != Some("runtime_fallback")
                && data
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.trim().is_empty())
        }
        _ => false,
    }
}

pub(crate) fn result_text_for_snapshot(
    snapshot: &agent_core::ThreadSnapshot,
    turn_id: &str,
) -> Option<String> {
    if snapshot.active_turn.as_ref()?.id != turn_id {
        return None;
    }
    let after_tool = snapshot
        .items
        .iter()
        .rposition(is_process_boundary)
        .map_or(0, |index| index + 1);
    let suffix = public_text_for_items(&snapshot.items[after_tool..]);
    let result = if suffix.trim().is_empty() {
        public_text_for_items(&snapshot.items)
    } else {
        suffix
    };
    (!result.trim().is_empty()).then_some(result)
}

pub(crate) fn public_text_for_items(items: &[ThreadItem]) -> String {
    let mut text = String::new();
    let mut paragraph_start = false;
    for item in items {
        match item {
            ThreadItem::AgentMessage { content, .. } if !content.is_empty() => {
                if paragraph_start && !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(content);
                paragraph_start = false;
            }
            ThreadItem::UserMessage { .. } => paragraph_start = true,
            item if is_process_boundary(item) => paragraph_start = true,
            _ => {}
        }
    }
    text
}
