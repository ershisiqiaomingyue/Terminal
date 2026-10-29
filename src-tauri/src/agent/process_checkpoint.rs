use super::*;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

// Stream text must survive without a tool boundary, terminal event or WebView
// save. Update the cumulative item in place rather than persisting every delta.
pub(super) fn checkpoint_streamed_process(app: &AppHandle, payload: &Value) {
    let kind = payload["type"].as_str().unwrap_or_default();
    if !matches!(kind, "text_delta" | "reasoning_delta") {
        return;
    }
    let (Some(session_id), Some(turn_id)) =
        (payload["session_id"].as_str(), payload["turn_id"].as_str())
    else {
        return;
    };
    static CHECKPOINTS: OnceLock<Mutex<HashMap<(String, String), Instant>>> = OnceLock::new();
    let now = Instant::now();
    {
        let Ok(mut checkpoints) = CHECKPOINTS.get_or_init(Mutex::default).lock() else {
            return;
        };
        checkpoints.retain(|_, at| now.duration_since(*at) < Duration::from_secs(60));
        let key = (session_id.to_string(), turn_id.to_string());
        if checkpoints
            .get(&key)
            .is_some_and(|at| now.duration_since(*at) < Duration::from_millis(250))
        {
            return;
        }
        checkpoints.insert(key, now);
    }
    let state = app.state::<AgentRuntimeState>();
    let Some(snapshot) = state.read_thread_snapshot(session_id) else {
        return;
    };
    let Some(event) = streamed_process_checkpoint(&snapshot, turn_id, kind) else {
        return;
    };
    let timestamp = payload["timestamp_ms"].as_i64().unwrap_or_default();
    if let Err(error) = crate::history::append_history_session_events(
        app.clone(),
        session_id.into(),
        timestamp,
        vec![event],
    ) {
        eprintln!("无法保存流式正文: {error}");
    }
}

pub(super) fn streamed_process_checkpoint(
    snapshot: &agent_core::ThreadSnapshot,
    turn_id: &str,
    kind: &str,
) -> Option<Value> {
    let turn = snapshot
        .active_turn
        .as_ref()
        .filter(|turn| turn.id == turn_id)?;
    let (order, item) = snapshot
        .items
        .iter()
        .enumerate()
        .rev()
        .find(|(_, item)| match item {
            ThreadItem::AgentMessage { .. } => kind == "text_delta",
            ThreadItem::TimelineEvent { data, .. } => {
                kind == "reasoning_delta" && data["type"] == "reasoning_delta"
            }
            _ => false,
        })?;
    let item = serde_json::to_value(item).ok()?;
    let id = item["id"].as_str()?;
    Some(json!({
        "type":"process_item", "session_id":snapshot.id, "turn_id":turn_id,
        "event_id":format!("process:{}:{id}", turn.started_at_ms),
        "process_order":order, "execution_started_at":turn.started_at_ms,
        "snapshot_revision":snapshot.revision, "timestamp_ms":turn.started_at_ms,
        "result_item":kind == "text_delta", "item":item,
    }))
}

pub(crate) fn flush_running_processes(app: &AppHandle) {
    let state = app.state::<AgentRuntimeState>();
    let turns = state
        .threads
        .lock()
        .map(|threads| {
            threads
                .values()
                .filter_map(|snapshot| {
                    snapshot
                        .active_turn
                        .as_ref()
                        .filter(|turn| turn.status == agent_core::TurnStatus::Running)
                        .map(|turn| (snapshot.id.clone(), turn.id.clone()))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for (session_id, turn_id) in turns {
        settle_thread_process(app, &session_id, &turn_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streamed_checkpoint_replaces_the_cumulative_item_and_fences_turns() {
        use agent_core::{ThreadEvent, ThreadSnapshot};
        let mut snapshot = ThreadSnapshot::new("stream");
        snapshot.apply(ThreadEvent::TurnStarted {
            thread_id: "stream".into(),
            turn_id: "t".into(),
        });
        snapshot.apply(ThreadEvent::AgentDelta {
            thread_id: "stream".into(),
            turn_id: "t".into(),
            item_id: "a".into(),
            text: "第一段".into(),
        });
        let first = streamed_process_checkpoint(&snapshot, "t", "text_delta").unwrap();
        snapshot.apply(ThreadEvent::AgentDelta {
            thread_id: "stream".into(),
            turn_id: "t".into(),
            item_id: "a".into(),
            text: "继续生成".into(),
        });
        let next = streamed_process_checkpoint(&snapshot, "t", "text_delta").unwrap();
        assert_eq!(first["event_id"], next["event_id"]);
        assert_eq!(next["item"]["content"], "第一段继续生成");
        assert!(next["snapshot_revision"].as_u64() > first["snapshot_revision"].as_u64());
        assert!(streamed_process_checkpoint(&snapshot, "other", "text_delta").is_none());
        assert!(streamed_process_checkpoint(&snapshot, "t", "reasoning_delta").is_none());
    }
}
