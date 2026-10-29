//! Portable recovery from durable runtime events. Follows DSH's ordered
//! session projection and TOOL_NOT_STARTED / TOOL_OUTCOME_UNKNOWN distinction
//! (dsh-session/repair.ts). This module only projects records; it executes nothing.
use crate::{prompt_cache_hash, AgentMessage, AgentToolCall};
use serde_json::{json, Value};
use std::collections::HashMap;

fn turn_of(event: &Value) -> Option<&str> {
    event
        .get("turn_id")
        .or_else(|| event.get("turnId"))
        .and_then(Value::as_str)
}

/// Reconstruct a complete runtime snapshot in process order, independently of
/// the UI's final-only text. Old records without a user anchor use legacy replay.
pub fn replay_runtime_turn(events: &[Value], turn: &str) -> Option<Vec<AgentMessage>> {
    let relevant = events
        .iter()
        .filter(|event| turn_of(event) == Some(turn))
        .collect::<Vec<_>>();
    let execution = relevant
        .iter()
        .filter(|event| event["type"] == "process_item")
        .filter_map(|event| event["execution_started_at"].as_i64())
        .max();
    let mut items = HashMap::<&str, &Value>::new();
    let mut finished = HashMap::<&str, &Value>::new();
    let mut started = std::collections::HashSet::new();
    for event in &relevant {
        if event["type"] == "tool_call_finished" {
            if let Some(id) = event["call_id"].as_str() {
                finished.insert(id, event);
            }
        } else if event["type"] == "tool_call_started" {
            if let Some(id) = event["call_id"].as_str() {
                started.insert(id);
            }
        } else if event["type"] == "process_item"
            && event["execution_started_at"].as_i64() == execution
            && matches!(
                event["item"]["kind"].as_str(),
                Some("user_message" | "agent_message" | "tool_call")
            )
        {
            let Some(id) = event["item"]["id"].as_str() else {
                continue;
            };
            if items.get(id).is_none_or(|old| {
                event["snapshot_revision"].as_u64().unwrap_or(0)
                    >= old["snapshot_revision"].as_u64().unwrap_or(0)
            }) {
                items.insert(id, event);
            }
        }
    }
    let mut ordered = items.into_values().collect::<Vec<_>>();
    ordered.sort_by_key(|event| {
        (
            event["process_order"].as_u64().unwrap_or(u64::MAX),
            event["item"]["id"].as_str().unwrap_or_default(),
        )
    });
    if ordered
        .first()
        .is_none_or(|event| event["item"]["kind"] != "user_message")
        || !ordered.iter().any(|event| {
            matches!(
                event["item"]["kind"].as_str(),
                Some("agent_message" | "tool_call")
            )
        })
    {
        return None;
    }
    let mut messages = Vec::new();
    for event in ordered {
        let item = &event["item"];
        let id = item["id"].as_str().unwrap_or_default();
        match item["kind"].as_str() {
            Some("user_message") => {
                let mut message = AgentMessage::user(item["content"].as_str().unwrap_or_default());
                let sources = crate::user_local_source_metadata(item);
                if !sources.is_empty() {
                    message
                        .content
                        .push_str("\n\n[此历史用户消息提供的本机来源元数据]\n");
                    message
                        .content
                        .push_str(&crate::local_source_metadata_text(&sources));
                }
                message.source_message_id = Some(id.to_string());
                messages.push(message);
            }
            Some("agent_message") => {
                let text = item["content"].as_str().unwrap_or_default();
                if !text.trim().is_empty() {
                    let mut message = AgentMessage::user(text);
                    message.role = crate::MessageRole::Assistant;
                    message.source_message_id = Some(id.to_string());
                    messages.push(message);
                }
            }
            Some("tool_call") => {
                let completed = finished.get(id).copied();
                let input = completed
                    .and_then(|value| value.get("input"))
                    .filter(|input| input.is_object())
                    .unwrap_or(&item["input"]);
                let name = completed
                    .and_then(|value| value["tool_name"].as_str())
                    .or_else(|| item["name"].as_str())
                    .unwrap_or_default();
                // A stream can stop in the middle of JSON arguments. Never
                // turn that fragment into a valid-looking operation.
                if !input.is_object() || name.is_empty() {
                    messages.push(AgentMessage::system(
                        "历史工具调用参数未完整保存；不能据此认定操作已执行。",
                    ));
                    continue;
                }
                let result = completed
                    .and_then(|value| value.get("output"))
                    .filter(|output| !output.is_null())
                    .or_else(|| item.get("output").filter(|output| !output.is_null()));
                let status = completed
                    .and_then(|value| value["status"].as_str())
                    .or_else(|| item["status"].as_str())
                    .unwrap_or_default();
                let failed = completed.is_some_and(|value| value["is_error"] == true)
                    || item["is_error"] == true
                    || matches!(status, "error" | "cancelled" | "skipped" | "interrupted")
                    || result.is_some_and(|value| {
                        value["skipped"] == true
                            || value.get("error").is_some_and(|error| !error.is_null())
                    });
                let content = if let Some(result) = result {
                    if failed {
                        format!(
                            "[历史操作失败记录，不可作为事实证据]\n{}",
                            json!({"is_error":true,"status":status,"output":result})
                        )
                    } else {
                        result
                            .as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| result.to_string())
                    }
                } else {
                    let was_started = started.contains(id)
                        || item["started_at_ms"].is_number()
                        || matches!(status, "running" | "done" | "completed");
                    json!({"is_error":true,
                        "code":if was_started { "TOOL_OUTCOME_UNKNOWN" } else { "TOOL_NOT_STARTED" },
                        "message":if was_started {
                            "调用已开始，但结果未持久化，执行结果未知。继续前先核验外部状态，不要盲目重复有副作用的操作。"
                        } else { "没有记录到调用开始；不能认定操作已经执行。仅在仍有需要时重新请求调用。" }
                    }).to_string()
                };
                let replay_id = format!("history-{}", &prompt_cache_hash(&json!([turn, id]))[..32]);
                messages.push(AgentMessage::assistant_tool_call(AgentToolCall {
                    id: replay_id.clone(),
                    name: name.to_string(),
                    input: input.clone(),
                }));
                messages.push(AgentMessage::tool(
                    replay_id,
                    crate::prune_tool_result_for_model(&content),
                ));
            }
            _ => {}
        }
    }
    Some(messages)
}

#[cfg(test)]
#[path = "history_replay_tests.rs"]
mod tests;
