use super::support::seed_thread;
use super::*;

#[test]
fn interaction_reply_survives_reload_and_terminal_settlement_without_duplicates() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "interaction");
    let mut record = store.get_record("interaction").unwrap().unwrap();
    record.messages = serde_json::json!([{ "id":"assistant:t", "role":"assistant", "turnId":"t", "content":"", "pending":false }]);
    store.upsert_record(&record).unwrap();
    let process = [
        serde_json::json!({ "type":"process_item", "turn_id":"t", "event_id":"process:100:q", "execution_started_at":100, "process_order":0, "snapshot_revision":4, "item":{"kind":"agent_message", "id":"question:q", "content":"编译哪个目标？"}}),
        serde_json::json!({ "type":"process_item", "turn_id":"t", "event_id":"process:100:r", "execution_started_at":100, "process_order":1, "snapshot_revision":4, "item":{"kind":"user_message", "id":"user:t:reply:q", "content":"n60-userdebug", "attachments":[], "artifacts":[]}}),
    ];
    store.append_events("interaction", 101, &process).unwrap();
    store.append_events("interaction", 102, &process).unwrap();
    store.append_events("interaction", 103, &[
        serde_json::json!({ "type":"process_item", "turn_id":"t", "event_id":"process:100:a", "execution_started_at":100, "process_order":2, "snapshot_revision":6, "item":{"kind":"agent_message", "id":"assistant:continued", "content":"使用该目标继续。"}}),
        serde_json::json!({ "type":"completed", "turn_id":"t", "event_id":"done", "execution_started_at":100, "timestamp_ms":103, "durable_terminal":true }),
    ]).unwrap();
    let restored = store.get_record("interaction").unwrap().unwrap();
    let events = restored.events.as_ref().unwrap().as_array().unwrap();
    let segments = events
        .iter()
        .find(|event| event["type"] == "completed")
        .unwrap()["display_segments"]
        .as_array()
        .unwrap();
    assert_eq!(segments.len(), 3);
    assert_eq!(segments[1]["role"], "user");
    assert_eq!(segments[1]["content"], "n60-userdebug");
    assert_eq!(segments[1]["id"], "user:t:reply:q");
    let context = raw_agent_context_from_record(&restored);
    assert_eq!(
        context
            .iter()
            .filter(|message| message.role == "user" && message.content == "n60-userdebug")
            .count(),
        1
    );
    assert!(context
        .last()
        .unwrap()
        .content
        .ends_with("使用该目标继续。"));
}
