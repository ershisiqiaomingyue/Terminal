use super::support::*;
use super::*;

fn process_item(id: &str, order: i64, revision: i64, item: Value) -> Value {
    serde_json::json!({
        "type":"process_item", "turn_id":"t", "event_id":format!("process:1000:{id}"),
        "execution_started_at":1000, "snapshot_revision":revision, "process_order":order,
        "timestamp_ms":1000, "item":item,
    })
}

#[test]
fn cold_restart_restores_interrupted_text_and_steer_order_without_a_terminal() {
    let path = temporary_history_path("interrupted-text");
    {
        let store = HistoryStore::open(&path).unwrap();
        seed_thread(&store, "restart");
        let mut request = queued_request("restart", "检查接口");
        request.turn_id = Some("t".into());
        store.start_agent_run("restart", "t", &request).unwrap();
        store
            .connection
            .execute(
                "UPDATE agent_runs SET started_at = 1000 WHERE session_id = 'restart'",
                [],
            )
            .unwrap();
        store.append_events("restart", 1001, &[
            process_item("u", 0, 1, serde_json::json!({"kind":"user_message","id":"user:t","content":"检查接口"})),
            process_item("a", 1, 1, serde_json::json!({"kind":"agent_message","id":"a","content":"先检查"})),
            process_item("c", 2, 2, serde_json::json!({"kind":"tool_call","id":"c","name":"read_local_file","status":"done","input":{"path":"api.rs"},"output":"tool evidence"})),
            process_item("b", 3, 3, serde_json::json!({"kind":"agent_message","id":"b","content":"已确认接收入口。"})),
            process_item("s", 4, 4, serde_json::json!({"kind":"user_message","id":"steer:t","content":"还要看外部接口"})),
            process_item("d", 5, 5, serde_json::json!({"kind":"agent_message","id":"d","content":"正在核对外部调用。"})),
        ]).unwrap();
        // Cumulative streaming checkpoint advances the same durable item.
        store.append_events("restart", 1002, &[
            process_item("a", 1, 6, serde_json::json!({"kind":"agent_message","id":"a","content":"先检查内部接口。"})),
        ]).unwrap();
    }
    {
        let store = HistoryStore::open(&path).unwrap();
        store.mark_agent_run_interrupted("restart", "t").unwrap();
        let record = store.get_record("restart").unwrap().unwrap();
        let assistant = record
            .messages
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == "assistant")
            .unwrap();
        assert_eq!(assistant["pending"], false);
        assert_eq!(
            assistant["content"],
            "先检查内部接口。\n\n已确认接收入口。\n\n正在核对外部调用。"
        );
        let events = record.events.as_ref().unwrap().as_array().unwrap();
        assert!(!events.iter().any(HistoryStore::is_durable_terminal));
        assert!(!events.iter().any(|event| event["type"] == "process_item"));
        let interrupted = events
            .iter()
            .find(|event| event["type"] == "interrupted")
            .unwrap();
        assert_eq!(interrupted["has_execution_details"], true);
        let segments = interrupted["display_segments"].as_array().unwrap();
        assert_eq!(
            segments
                .iter()
                .map(|part| part["content"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "检查接口",
                "先检查内部接口。\n\n已确认接收入口。",
                "还要看外部接口",
                "正在核对外部调用。"
            ]
        );
        assert!(record.agent_runs[0].finished_at.is_none());
        let page = store.turn_event_page("restart", "t", 0, None).unwrap();
        assert!(page
            .events
            .iter()
            .any(|event| event["item"]["output"] == "tool evidence"));
    }
    let _ = std::fs::remove_file(path);
}

#[test]
fn interrupted_legacy_process_without_user_items_still_restores_text() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "legacy-text");
    store
        .start_agent_run("legacy-text", "t", &queued_request("legacy-text", "check"))
        .unwrap();
    store
        .append_events(
            "legacy-text",
            1001,
            &[process_item(
                "a",
                0,
                1,
                serde_json::json!({"kind":"agent_message","id":"a","content":"保留已生成的回复。"}),
            )],
        )
        .unwrap();
    store
        .mark_agent_run_interrupted("legacy-text", "t")
        .unwrap();
    let record = store.get_record("legacy-text").unwrap().unwrap();
    let assistant = record
        .messages
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "assistant")
        .unwrap();
    assert_eq!(assistant["content"], "保留已生成的回复。");
    assert_eq!(assistant["pending"], false);
}
