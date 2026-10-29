use super::super::*;

pub(crate) fn settle_thread_process(app: &AppHandle, session_id: &str, turn_id: &str) {
    let state = app.state::<AgentRuntimeState>();
    let Some(snapshot) = state.read_thread_snapshot(session_id) else {
        return;
    };
    let events = process_events_for_snapshot(&snapshot, turn_id);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    if let Err(error) = crate::history::append_history_session_events(
        app.clone(),
        session_id.to_string(),
        timestamp,
        events,
    ) {
        eprintln!("无法结算 Agent 执行过程: {error}");
    }
}

fn is_process_boundary(item: &ThreadItem) -> bool {
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

pub(crate) fn process_events_for_snapshot(
    snapshot: &agent_core::ThreadSnapshot,
    turn_id: &str,
) -> Vec<Value> {
    let Some(turn) = snapshot
        .active_turn
        .as_ref()
        .filter(|turn| turn.id == turn_id)
    else {
        return Vec::new();
    };
    let after_tool = snapshot
        .items
        .iter()
        .rposition(is_process_boundary)
        .map_or(0, |index| index + 1);
    let has_tail = snapshot.items[after_tool..].iter().any(|item| matches!(item, ThreadItem::AgentMessage { content, .. } if !content.trim().is_empty()));
    snapshot.items.iter().enumerate().filter_map(|(order, item)| {
        let value = serde_json::to_value(item).ok()?;
        let item_id = value.get("id")?.as_str()?;
        Some(json!({
            "type":"process_item", "session_id":snapshot.id, "turn_id":turn_id,
            "event_id":format!("process:{}:{item_id}", turn.started_at_ms),
            "process_order":order, "execution_started_at":turn.started_at_ms,
            "snapshot_revision":snapshot.revision, "timestamp_ms":turn.started_at_ms,
            "result_item":matches!(item, ThreadItem::AgentMessage { .. }) && (!has_tail || order >= after_tool),
            "item":value,
        }))
    }).collect()
}
