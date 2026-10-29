use super::super::*;
use super::super::lifecycle::tool_name_in;

pub(crate) fn runtime_event(
    session_id: Option<&str>,
    turn_id: Option<&str>,
    event_type: Option<&str>,
    payload: &Value,
    tool_call_count: usize,
) -> Option<ThreadEvent> {
    let (Some(thread_id), Some(turn_id)) = (session_id, turn_id) else {
        return None;
    };
    let activity = match event_type {
        Some("status") => payload
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        Some("steered") => "当前 Agent 已接收追加要求，正在继续处理。".to_string(),
        Some("tool_call_pending") => {
            let tool_name = tool_name_in(payload).unwrap_or("工具");
            format!("已接收 {tool_name} 调用，正在准备参数")
        }
        Some("tool_call_started") => match tool_name_in(payload) {
            Some(name) => format!("正在执行 {name} ({tool_call_count})"),
            None => format!("正在执行命令 ({tool_call_count})"),
        },
        Some("tool_call_finished") => match tool_name_in(payload) {
            Some(name) => format!("已完成 {name}"),
            None => "已完成命令".to_string(),
        },
        Some("permission_requested") => {
            if payload.get("tool_name").and_then(Value::as_str) == Some("ask_user") {
                "等待用户回答".to_string()
            } else {
                "等待工具权限批准".to_string()
            }
        }
        Some("completed" | "cancelled" | "error") => String::new(),
        _ => return None,
    };
    Some(ThreadEvent::RuntimeUpdated {
        thread_id: thread_id.to_string(),
        turn_id: turn_id.to_string(),
        activity,
        tool_call_count,
    })
}
