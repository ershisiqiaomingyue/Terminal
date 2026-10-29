use super::super::*;

pub(crate) fn emit_thread_snapshot_event(
    app: &AppHandle,
    state: &AgentRuntimeState,
    event: ThreadEvent,
    event_id: Option<&str>,
) {
    let deferred_thread = match &event {
        ThreadEvent::AgentDelta { thread_id, .. } => Some(thread_id.clone()),
        ThreadEvent::ToolCallInputDelta { thread_id, .. } => Some(thread_id.clone()),
        ThreadEvent::ToolCallOutput { thread_id, .. } => Some(thread_id.clone()),
        ThreadEvent::ItemAdded {
            thread_id,
            item: ThreadItem::TimelineEvent { data, .. },
            ..
        } if data.get("type").and_then(Value::as_str) == Some("reasoning_delta") => {
            Some(thread_id.clone())
        }
        _ => None,
    };
    if let Some(thread_id) = deferred_thread {
        if state.apply_thread_event_deferred(event, event_id)
            && state.schedule_thread_publication(&thread_id)
        {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                if let Some(update) = app
                    .state::<AgentRuntimeState>()
                    .take_thread_update(&thread_id)
                {
                    let _ = app.emit("agent:thread_update", update);
                }
            });
        }
        return;
    }
    let thread_id = event.source_thread_id().to_string();
    if state.apply_thread_event_deferred(event, event_id) {
        if let Some(update) = state.take_thread_update(&thread_id) {
            let _ = app.emit("agent:thread_update", update);
        }
    }
}
