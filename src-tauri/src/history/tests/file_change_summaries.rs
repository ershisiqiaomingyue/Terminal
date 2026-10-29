use super::support::*;
use super::*;

#[test]
fn file_cards_restore_independently_of_process_pages_and_the_event_tail() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "files");
    let mut record = store.get_record("files").unwrap().unwrap();
    record.messages = serde_json::json!([
        {"id":"a1","role":"assistant","turnId":"old","content":"old result"},
        {"id":"a2","role":"assistant","turnId":"process","content":"process result"},
        {"id":"a3","role":"assistant","turnId":"receipt","content":"receipt result"},
        {"id":"a4","role":"assistant","turnId":"reverted","content":"reverted result"},
    ]);
    store.upsert_record(&record).unwrap();
    let mut events = vec![
        serde_json::json!({"type":"file_changes","event_id":"f1","turn_id":"old","timestamp_ms":1,"modified":["old.ts"]}),
        serde_json::json!({"type":"process_item","event_id":"f2","turn_id":"process","timestamp_ms":2,"item":{"kind":"file_changes","data":{"type":"file_changes","modified":["process.ts"]}}}),
        serde_json::json!({"type":"process_item","event_id":"f3","turn_id":"receipt","timestamp_ms":3,"item":{"kind":"tool_call","name":"windows__apply_patch","status":"done","output":{"path":"receipt.ts","appliedHunks":1}}}),
        serde_json::json!({"type":"tool_call_finished","event_id":"f4","turn_id":"receipt","timestamp_ms":4,"toolName":"windows__write_local_file","isError":true,"output":{"path":"failed.ts"}}),
        serde_json::json!({"type":"tool_call_finished","event_id":"f5","turn_id":"receipt","timestamp_ms":5,"toolName":"windows__write_local_file","output":"{\"path\":\"written.ts\",\"writtenBytes\":3}"}),
        serde_json::json!({"type":"tool_call_finished","event_id":"r1","turn_id":"reverted","timestamp_ms":6,"toolName":"windows__apply_patch","output":{"path":"reverted.ts","appliedHunks":1}}),
        serde_json::json!({"type":"file_changes","event_id":"r2","turn_id":"reverted","timestamp_ms":7,"snapshot_complete":true,"entries":[]}),
    ];
    events.extend((0..1100).map(|index| serde_json::json!({"type":"usage","event_id":format!("noise-{index}"),"turn_id":"new","timestamp_ms":index + 100})));
    store.append_events("files", 2000, &events).unwrap();
    let restored = store.get_record("files").unwrap().unwrap();
    let messages = restored.messages.as_array().unwrap();
    let card = |index: usize| messages[index]["events"].as_array().unwrap().iter().find(|event| event["type"] == "file_changes").unwrap().clone();
    assert_eq!(card(0)["modified"], serde_json::json!(["old.ts"]));
    assert_eq!(card(1)["modified"], serde_json::json!(["process.ts"]));
    assert_eq!(card(2)["modified"], serde_json::json!(["receipt.ts", "written.ts"]));
    assert_eq!(card(2)["review_available"], false);
    assert_eq!(card(2)["line_counts_available"], false);
    assert_eq!(card(3)["entries"], serde_json::json!([]));
    assert!(restored.events.unwrap().as_array().unwrap().iter().all(|event| event["type"] != "process_item"));
}
