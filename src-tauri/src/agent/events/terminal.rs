use super::super::*;

use super::process::process_events_for_snapshot;
use super::projection::emit_thread_snapshot_for_payload;
use super::text::{public_text_for_items, result_text_for_snapshot};
use super::timeline::display_segments_for_snapshot;

pub(crate) fn terminal_status_notice(payload: &Value) -> Option<Value> {
    let kind = payload["type"].as_str()?;
    let reason = payload["termination_reason"].as_str().unwrap_or_default();
    let message = match (kind, reason) {
        ("cancelled", _) => payload["message"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("本轮已停止，已保留此前输出和工具记录。需要继续时可发送消息。"),
        ("completed", "output_limit_reached") => {
            "已达到连续输出上限，本轮已停止。已保留生成内容，任务可能尚未完成，可以发送消息继续。"
        }
        ("completed", "duplicate_tool_call") => {
            "检测到工具重复调用，已停止继续执行并整理已有结果。任务可能尚未完成，请检查结果后继续。"
        }
        ("completed", "malformed_tool_call") => {
            "工具参数未能完整生成，已停止继续执行并保留已有结果。未完成的操作需要继续处理。"
        }
        ("completed", "tool_execution_boundary") => {
            "工具执行触及限制，已停止继续执行并整理已有结果。请检查回复中的限制和尚未完成的工作。"
        }
        // Repetitive output already emits a specific terminal diagnostic.
        _ => return None,
    };
    Some(json!({
        "type": "status", "status": if kind == "cancelled" { "cancelled" } else { reason },
        "session_id": payload["session_id"], "terminalDiagnostic": true,
        "termination_reason": reason, "message": message,
    }))
}

// A terminal event is the boundary used during restart recovery. Persist it
// before notifying the WebView so a completed backend turn cannot be restored
// as interrupted merely because the window closes before the frontend flushes.
pub(crate) async fn emit_durable_terminal_payload(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    payload: Value,
) {
    if let Some(notice) = terminal_status_notice(&payload) {
        emit_agent_payload_for_turn(app, notice, turn_id);
    }
    let snapshot = app
        .state::<AgentRuntimeState>()
        .read_thread_snapshot(session_id);
    let mut process = snapshot
        .as_ref()
        .map(|snapshot| process_events_for_snapshot(snapshot, turn_id))
        .unwrap_or_default();
    let mut payload = payload;
    let retain_partial = matches!(
        payload.get("type").and_then(Value::as_str),
        Some("cancelled" | "error")
    ) || payload.get("termination_reason").and_then(Value::as_str)
        == Some("repetitive_output");
    if let Some(snapshot) = snapshot.as_ref().filter(|snapshot| {
        snapshot
            .active_turn
            .as_ref()
            .is_some_and(|turn| turn.id == turn_id)
    }) {
        let turn = snapshot.active_turn.as_ref().expect("matching active turn");
        payload["execution_started_at"] = Value::from(turn.started_at_ms);
        if payload.get("usage").is_none() {
            payload["usage"] = serde_json::to_value(&snapshot.runtime.usage).unwrap_or(Value::Null);
        }
        let visible_text = if retain_partial {
            public_text_for_items(&snapshot.items)
        } else {
            result_text_for_snapshot(snapshot, turn_id).unwrap_or_default()
        };
        payload["display_text"] = Value::String(visible_text);
        payload["display_segments"] =
            Value::Array(display_segments_for_snapshot(snapshot, retain_partial));
        if retain_partial {
            for event in &mut process {
                if event
                    .get("item")
                    .and_then(|item| item.get("kind"))
                    .and_then(Value::as_str)
                    == Some("agent_message")
                {
                    event["result_item"] = Value::Bool(true);
                }
            }
        }
    }
    let payload = stamp_payload_with_turn(payload, Some(turn_id));
    let updated_at = payload
        .get("timestamp_ms")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let mut durable_payload = payload.clone();
    if let Value::Object(object) = &mut durable_payload {
        object.insert("durable_terminal".to_string(), Value::Bool(true));
    }
    // Process and terminal cross the same transaction boundary.
    process.push(durable_payload.clone());
    let payload = match crate::history::append_history_terminal_events_with_retry(
        app.clone(),
        session_id.to_string(),
        updated_at,
        process,
    )
    .await
    {
        Ok(()) => durable_payload,
        Err(error) => {
            // Keep the live response usable, but do not claim the terminal
            // crossed its persistence boundary when the database rejected it.
            eprintln!("无法持久化 Agent 终态事件: {error}");
            let mut volatile_payload = payload;
            if let Value::Object(object) = &mut volatile_payload {
                object.insert("durable_terminal".to_string(), Value::Bool(false));
                object.insert("terminal_persistence_failed".to_string(), Value::Bool(true));
            }
            volatile_payload
        }
    };
    crate::diagnostics::agent::record_lifecycle(&payload);
    emit_thread_snapshot_for_payload(app, &payload);
    let _ = app.emit("agent:event", payload);
}
