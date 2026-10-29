use super::super::*;

pub(crate) fn should_persist_agent_payload(payload: &Value) -> bool {
    let event_type = payload.get("type").and_then(Value::as_str);
    if event_type == Some("status")
        && payload.get("terminalDiagnostic").and_then(Value::as_bool) == Some(true)
    {
        return true;
    }
    if event_type == Some("context")
        && payload.get("streaming").and_then(Value::as_bool) == Some(true)
    {
        return false;
    }

    matches!(
        event_type,
        Some(
            "cache_diagnostic"
                | "tool_call_pending"
                | "tool_call_started"
                | "tool_call_finished"
                | "task_plan"
                | "permission_requested"
                | "permission_decision"
                | "context"
                | "file_changes"
                | "usage"
        )
    )
}

pub(crate) fn persist_agent_payload(app: &AppHandle, payload: &Value) {
    crate::diagnostics::agent::record_lifecycle(payload);
    if should_persist_agent_payload(payload) {
        if let Some(session_id) = payload.get("session_id").and_then(Value::as_str) {
            let updated_at = payload
                .get("timestamp_ms")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            if let Err(error) = crate::history::append_history_session_events(
                app.clone(),
                session_id.to_string(),
                updated_at,
                vec![payload.clone()],
            ) {
                eprintln!("无法持久化 Agent 运行事件: {error}");
                crate::diagnostics::record(
                    "error",
                    "agent.history_event_save_failed",
                    "native.agent",
                    json!({"session_id":session_id, "turn_id":payload.get("turn_id"), "event_type":payload.get("type")}),
                );
            }
        }
    }
}

// These endings stop the runtime without proving that the user's task finished.
// Keep the notice in the ordered snapshot as well as the durable terminal so
// live rendering, history pagination and restart recovery agree.
