use super::support::*;
use super::*;

#[test]
fn context_usage_restores_latest_checkpoint_outside_the_timeline_tail() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "context");
    let initial = serde_json::json!({
        "type":"context", "turn_id":"queued", "event_id":"context-initial", "timestamp_ms":10,
        "model_id":"model", "provider_id":"provider", "context_tokens":40000,
        "system_prompt_tokens":3900, "tool_definition_tokens":13700, "context_window":1000000,
    });
    let mut final_reading = initial.clone();
    final_reading["context_tokens"] = serde_json::json!(763500);
    final_reading["timestamp_ms"] = serde_json::json!(30);
    final_reading["streaming"] = serde_json::json!(true);
    store.append_events("context", 10, &[
        initial,
        serde_json::json!({"type":"process_item", "event_id":"final-context", "turn_id":"queued", "timestamp_ms":10,
            "item":{"kind":"context", "id":"context:queued", "data":final_reading}}),
    ]).unwrap();
    let tail: Vec<Value> = (0..MAX_RESTORED_SESSION_EVENTS + 10).map(|index| serde_json::json!({
        "type":"tool_call_finished", "event_id":format!("tool-{index}"), "turn_id":"queued", "timestamp_ms":100 + index,
    })).collect();
    store.append_events("context", 2000, &tail).unwrap();
    let record = store.get_record("context").unwrap().unwrap();
    assert!(!record.events.as_ref().unwrap().as_array().unwrap().iter().any(|event| event["type"] == "context"));
    let telemetry = record.metadata["contextUsageEvents"].as_array().unwrap();
    assert_eq!(telemetry.len(), 1);
    assert_eq!(telemetry[0]["context_tokens"], 763500);
    assert_eq!(telemetry[0]["system_prompt_tokens"], 3900);
    assert_eq!(telemetry[0]["tool_definition_tokens"], 13700);
    assert_eq!(telemetry[0]["turn_id"], "queued");
}

#[test]
fn context_usage_ignores_invalid_counts_and_other_sessions() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "one");
    seed_thread(&store, "two");
    let valid = serde_json::json!({
        "type":"context", "turn_id":"turn", "event_id":"valid", "timestamp_ms":10,
        "model_id":"model", "provider_id":"provider", "context_tokens":100,
        "system_prompt_tokens":20, "tool_definition_tokens":30,
    });
    let mut invalid = valid.clone();
    invalid["event_id"] = serde_json::json!("invalid");
    invalid["timestamp_ms"] = serde_json::json!(20);
    invalid["context_tokens"] = serde_json::json!(1);
    store.append_events("one", 20, &[valid.clone(), invalid]).unwrap();
    let mut other_session = valid;
    other_session["context_tokens"] = serde_json::json!(999);
    store.append_events("two", 30, &[other_session]).unwrap();
    let record = store.get_record("one").unwrap().unwrap();
    let telemetry = record.metadata["contextUsageEvents"].as_array().unwrap();
    assert_eq!(telemetry.len(), 1);
    assert_eq!(telemetry[0]["context_tokens"], 100);
}
