use super::super::*;
use super::super::tasks::emit_agent_status_for_turn;

pub(crate) fn finish_tool_call(
    app: &AppHandle,
    state: &AgentRuntimeState,
    session_id: Option<&str>,
    turn_id: Option<&str>,
    event_type: Option<&str>,
    payload: &Value,
) {
    let (Some(thread_id), Some(turn_id), Some("tool_call_finished")) =
        (session_id, turn_id, event_type)
    else {
        return;
    };
    let tool_name = payload
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("工具调用");
    let output = payload
        .get("output")
        .map(Value::to_string)
        .unwrap_or_default();
    let failed = payload
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || payload
            .get("error")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || output.contains("用户拒绝")
        || output.contains("权限被拒绝")
        || output.to_ascii_lowercase().contains("permission denied");
    if let Some(reason) = state.record_tool_call_finished(
        thread_id,
        turn_id,
        payload
            .get("call_id")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        tool_name,
        failed,
    ) {
        emit_agent_status_for_turn(
            app,
            thread_id,
            "circuit_breaker",
            format!("已触发熔断器：{reason}"),
            Some(turn_id),
        );
    }
}
