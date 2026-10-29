use super::support::*;
use super::*;
use agent_core::{AgentToolCall, ModelTranscript};

fn transcript(turn: &str) -> ModelTranscript {
    let mut assistant = AgentMessage::assistant_tool_call(AgentToolCall {
        id: "read".into(),
        name: "read_local_file".into(),
        input: serde_json::json!({"path":"a.cpp"}),
    });
    assistant.anthropic_content_blocks = vec![
        serde_json::json!({"type":"thinking","thinking":"original","signature":"signed"}),
        serde_json::json!({"type":"tool_use","id":"read","name":"read_local_file","input":{"path":"a.cpp"}}),
    ];
    ModelTranscript {
        version: 1,
        scope: "same-route".into(),
        turn_id: turn.into(),
        runtime_context: "project settings".into(),
        images: vec![],
        last_request: None,
        messages: vec![
            AgentMessage::user("read a.cpp"),
            assistant,
            AgentMessage::tool("read", "actual original file bytes"),
        ],
    }
}

fn complete(store: &HistoryStore, session: &str, turn: &str) {
    let mut record = store.get_record(session).unwrap().unwrap();
    record.messages = serde_json::json!([
        {"id":format!("u:{turn}"),"role":"user","content":"read a.cpp","turnId":turn},
        {"id":format!("a:{turn}"),"role":"assistant","content":"done","turnId":turn}
    ]);
    store.upsert_record(&record).unwrap();
    store
        .append_events(
            session,
            1,
            &[
                serde_json::json!({"type":"completed","event_id":format!("e:{turn}"),
        "turn_id":turn,"text":"done","durable_terminal":true,"timestamp_ms":1}),
            ],
        )
        .unwrap();
}

#[test]
fn model_history_survives_restart_and_does_not_enter_display_messages() {
    let path = temporary_history_path("model-transcript");
    {
        let store = HistoryStore::open(&path).unwrap();
        seed_thread(&store, "s");
        complete(&store, "s", "t1");
        store.save_model_transcript("s", &transcript("t1")).unwrap();
    }
    {
        let store = HistoryStore::open(&path).unwrap();
        let restored = store
            .load_model_transcript("s", "same-route", "t2")
            .unwrap()
            .unwrap();
        assert_eq!(
            restored.messages[1].anthropic_content_blocks[0]["signature"],
            "signed"
        );
        assert_eq!(restored.messages[2].content, "actual original file bytes");
        assert!(!serde_json::to_string(&store.get_record("s").unwrap())
            .unwrap()
            .contains("actual original file bytes"));
        assert!(store
            .load_model_transcript("s", "other-model", "t2")
            .unwrap()
            .is_none());
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn uncommitted_stale_and_corrupt_model_snapshots_are_not_replayed() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "s");
    store.save_model_transcript("s", &transcript("t1")).unwrap();
    assert!(store
        .load_model_transcript("s", "same-route", "t2")
        .unwrap()
        .is_none());
    complete(&store, "s", "t1");
    assert!(store
        .load_model_transcript("s", "same-route", "t2")
        .unwrap()
        .is_some());
    complete(&store, "s", "t3");
    assert!(store
        .load_model_transcript("s", "same-route", "t4")
        .unwrap()
        .is_none());
    store
        .install_model_transcript("s", Some(b"corrupt gzip"))
        .unwrap();
    assert!(store
        .load_model_transcript("s", "same-route", "t4")
        .unwrap()
        .is_none());
}

#[test]
fn checkpoint_fork_rewind_and_delete_keep_model_history_boundaries() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "s");
    complete(&store, "s", "t1");
    store.save_model_transcript("s", &transcript("t1")).unwrap();
    let checkpoint = store
        .create_checkpoint("s", Some("t1".into()), None, None)
        .unwrap();
    complete(&store, "s", "t2");
    store.save_model_transcript("s", &transcript("t2")).unwrap();
    let child = store
        .fork_thread("s", Some(checkpoint.id.clone()), None, None)
        .unwrap();
    assert_eq!(
        store
            .conversation_thread(&child.id)
            .unwrap()
            .unwrap()
            .root_thread_id,
        "s"
    );
    assert_eq!(
        store
            .load_model_transcript(&child.id, "same-route", "new")
            .unwrap()
            .unwrap()
            .turn_id,
        "t1"
    );
    store.restore_checkpoint("s", &checkpoint.id).unwrap();
    assert_eq!(
        store
            .load_model_transcript("s", "same-route", "new")
            .unwrap()
            .unwrap()
            .turn_id,
        "t1"
    );
    store
        .rewind_before_message("s", "u:t1", Some("t1"))
        .unwrap();
    assert!(store.model_transcript_bytes("s").unwrap().is_none());
    store.delete(&child.id).unwrap();
    assert!(store.model_transcript_bytes(&child.id).unwrap().is_none());
}

#[test]
fn fallback_history_keeps_real_tool_results_without_replaying_failed_or_current_calls() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "s");
    complete(&store, "s", "t1");
    let mut record = store.get_record("s").unwrap().unwrap();
    record.messages = serde_json::json!([
        {"id":"u1","role":"user","content":"read file","turnId":"t1"},
        {"id":"a1","role":"assistant","content":"done","turnId":"t1"},
        {"id":"u2","role":"user","content":"follow up","turnId":"t2"},
        {"id":"a2","role":"assistant","content":"continuing","turnId":"t2"}
    ]);
    let good = serde_json::json!({"type":"tool_call_finished", "turn_id":"t1", "call_id":"read", "tool_name":"read_local_file", "input":{"path":"a.java"}, "output":{"content":"actual file bytes"}, "is_error":false});
    let mut failed = good.clone();
    failed["call_id"] = serde_json::json!("failed");
    failed["is_error"] = serde_json::json!(true);
    failed["output"] = serde_json::json!({"error":"private failure"});
    let mut skipped = good.clone();
    skipped["call_id"] = serde_json::json!("skip");
    skipped["output"] = serde_json::json!({"skipped":true});
    let mut current = good.clone();
    current["turn_id"] = serde_json::json!("t2");
    current["call_id"] = serde_json::json!("current");
    record.events = Some(serde_json::json!([good, good, failed, skipped, current]));
    let restored = agent_context_from_record(&record, "any-model", 3, Some("t2")).unwrap();
    assert_eq!(restored.iter().filter(|m| m.role == MessageRole::Tool).count(), 1);
    assert!(restored.last().unwrap().content.contains("actual file bytes"));
    assert!(!serde_json::to_string(&restored).unwrap().contains("private failure"));
    assert!(restored.iter().all(|m| m.content != "follow up"));
    let request = agent_core::ProviderRequest::new("s", "model", restored, vec![]);
    for kind in [agent_core::ProviderKind::OpenAiCompatible, agent_core::ProviderKind::OpenAiResponses, agent_core::ProviderKind::Anthropic] {
        let config = agent_core::ProviderConfig::new("p", "p", kind, "https://example.com/v1", "key").unwrap();
        let body = agent_core::request_body(&config, &request).unwrap().to_string();
        assert!(body.contains("actual file bytes"), "{kind:?}");
        assert!(body.contains("history-"), "{kind:?}");
    }
}

#[test]
fn model_recovery_keeps_writes_and_intent_outside_the_ui_event_window() {
    let path = temporary_history_path("history-event-window");
    {
        let store = HistoryStore::open(&path).unwrap();
        seed_thread(&store, "s");
        complete(&store, "s", "t1");
        let mut record = store.get_record("s").unwrap().unwrap();
        record.messages = serde_json::json!([
            {"id":"u1","role":"user","content":"只做接收展示，保留 Activity","turnId":"t1"},
            {"id":"a1","role":"assistant","content":"现在写布局和文案。","turnId":"t1"},
            {"id":"u2","role":"user","content":"继续","turnId":"t2"}
        ]);
        store.upsert_record(&record).unwrap();
        store.append_events("s", 2, &[serde_json::json!({
            "type":"tool_call_finished", "event_id":"write", "turn_id":"t1",
            "call_id":"write", "tool_name":"windows__write_local_file",
            "input":{"path":"D:/project/activity/MainActivity.java", "content":"actual Activity"},
            "output":{"path":"D:/project/activity/MainActivity.java", "bytes":6292},
            "is_error":false, "timestamp_ms":2
        })]).unwrap();
        store.save_model_transcript("s", &transcript("t1")).unwrap();
        let progress = (0..MAX_RESTORED_SESSION_EVENTS + 10).map(|index| serde_json::json!({
            "type":"usage", "event_id":format!("usage:{index}"),
            "turn_id":"t2", "timestamp_ms":index + 3
        })).collect::<Vec<_>>();
        store.append_events("s", 10_000, &progress).unwrap();
    }
    {
        let store = HistoryStore::open(&path).unwrap();
        let ui_record = store.get_record("s").unwrap().unwrap();
        assert!(ui_record.events.as_ref().unwrap().as_array().unwrap().iter()
            .all(|event| event["type"] == "usage"));
        assert!(agent_context_from_record(&ui_record, "other-model", 3, Some("t2")).is_none());
        let model_record = store.model_context_record("s").unwrap().unwrap();
        let restored = agent_context_from_record(&model_record, "other-model", 3, Some("t2")).unwrap();
        let serialized = serde_json::to_string(&restored).unwrap();
        assert!(serialized.contains("只做接收展示，保留 Activity"));
        assert!(serialized.contains("MainActivity.java"));
        assert!(serialized.contains("6292"));
        assert!(!serialized.contains("usage:"));
        assert!(!serialized.contains("继续"));
        assert_eq!(restored.iter().filter(|message| message.role == MessageRole::Tool).count(), 1);
        for kind in [agent_core::ProviderKind::OpenAiCompatible, agent_core::ProviderKind::DeepSeek,
            agent_core::ProviderKind::OpenAiResponses, agent_core::ProviderKind::Anthropic] {
            let config = agent_core::ProviderConfig::new("p", "p", kind, "https://example.com/v1", "key").unwrap();
            let request = agent_core::ProviderRequest::new("s", "model", restored.clone(), vec![]);
            let body = agent_core::request_body(&config, &request).unwrap().to_string();
            assert!(body.contains("MainActivity.java"), "{kind:?}");
            assert!(body.contains("6292"), "{kind:?}");
        }
        // Even the successful terminal was evicted from the UI window. The
        // native snapshot must remain valid without reconstructing all tools.
        assert!(store.load_model_transcript("s", "same-route", "t2").unwrap().is_some());
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn failed_turn_can_reuse_a_durable_compaction_checkpoint_after_model_switch() {
    let path = temporary_history_path("failed-compaction-checkpoint");
    {
        let store = HistoryStore::open(&path).unwrap();
        seed_thread(&store, "s");
        let mut record = store.get_record("s").unwrap().unwrap();
        record.messages = serde_json::json!([
            {"id":"u1","role":"user","content":"保留这个项目约束","turnId":"t1"},
            {"id":"a1","role":"assistant","content":"该轮剩余内容","turnId":"t1"},
            {"id":"u2","role":"user","content":"继续","turnId":"t2"}
        ]);
        store.upsert_record(&record).unwrap();
        store.append_events("s", 3, &[
            serde_json::json!({"type":"context","event_id":"compact","turn_id":"t2",
                "compaction_status":"completed","contextCheckpoint":{
                    "id":"checkpoint-compact","summary":"项目必须保留现有接收页面。",
                    "evidencePolicyVersion":3,
                    "payload":{"formatVersion":1,"producerModelId":"model-a","entries":[
                        {"kind":"user_constraint","content":"保留这个项目约束","sourceMessageId":"u1"},
                        {"kind":"tool_evidence","callId":"read-1","toolName":"read_local_file",
                            "input":{"path":"MainActivity.java"},"output":"压缩保留的工具证据",
                            "sourceMessageId":"u1"}
                    ]}
                }}),
            serde_json::json!({"type":"error","event_id":"failed","turn_id":"t2",
                "message":"上游返回 403","durable_terminal":true,"timestamp_ms":4}),
            serde_json::json!({"type":"tool_call_finished","event_id":"old-tool-1","turn_id":"t1",
                "call_id":"read-1","tool_name":"read_local_file","input":{"path":"MainActivity.java"},
                "output":{"content":"原始完整工具输出，不应在恢复时重新注入"},"is_error":false,"timestamp_ms":2}),
        ]).unwrap();
        let record = store.model_context_record("s").unwrap().unwrap();
        let restored = agent_context_from_record(&record, "different-model", 3, Some("t2")).unwrap();
        let serialized = serde_json::to_string(&restored).unwrap();
        assert!(serialized.contains("压缩保留的工具证据"));
        assert!(!serialized.contains("原始完整工具输出"));
        assert_eq!(restored.iter().filter(|message| message.role == MessageRole::Tool).count(), 1);
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn interrupted_runtime_history_survives_restart_provider_switch_and_ui_collapse() {
    let path = temporary_history_path("ordered-runtime-recovery");
    {
        let store = HistoryStore::open(&path).unwrap();
        seed_thread(&store, "s");
        let mut record = store.get_record("s").unwrap().unwrap();
        // The UI intentionally shows only the last response. It cannot be the
        // source for the model's intermediate progress or tool-call ordering.
        record.messages = serde_json::json!([
            {"id":"user:t1","role":"user","content":"恢复接收展示","turnId":"t1"},
            {"id":"assistant:t1","role":"assistant","content":"最后的界面文本","turnId":"t1"},
            {"id":"user:t2","role":"user","content":"继续","turnId":"t2"}
        ]);
        store.upsert_record(&record).unwrap();
        let row = |order, item| serde_json::json!({"type":"process_item","turn_id":"t1",
            "event_id":format!("process:{order}"), "process_order":order,
            "snapshot_revision":5,"execution_started_at":10,"timestamp_ms":10,"item":item});
        store.append_events("s", 20, &[
            row(0,serde_json::json!({"kind":"user_message","id":"user:t1","content":"恢复接收展示"})),
            row(1,serde_json::json!({"kind":"agent_message","id":"progress","content":"先写接收 Activity","pending":false})),
            row(2,serde_json::json!({"kind":"tool_call","id":"write","name":"write_local_file","input":{"path":"MainActivity.java"},"status":"running"})),
            row(3,serde_json::json!({"kind":"user_message","id":"steer","content":"不要删除接收页面"})),
            row(4,serde_json::json!({"kind":"agent_message","id":"next","content":"接着写布局","pending":false})),
            row(5,serde_json::json!({"kind":"tool_call","id":"layout","name":"write_local_file","input":{"path":"activity_main.xml"},"status":"running"})),
            serde_json::json!({"type":"tool_call_finished","event_id":"finished-write","turn_id":"t1","call_id":"write",
                "tool_name":"write_local_file","input":{"path":"MainActivity.java"},"output":{"bytes":6292},"is_error":false,"timestamp_ms":11}),
        ]).unwrap();
    }
    {
        let store = HistoryStore::open(&path).unwrap();
        assert!(store.load_model_transcript("s", "different-provider", "t2").unwrap().is_none());
        let record = store.model_context_record("s").unwrap().unwrap();
        let restored = agent_context_from_record(&record,"different-model",3,Some("t2")).unwrap();
        assert_eq!(restored[0].content,"恢复接收展示");
        assert_eq!(restored[1].content,"先写接收 Activity");
        assert!(restored[3].content.contains("6292"));
        assert_eq!(restored[4].content,"不要删除接收页面");
        assert_eq!(restored[5].content,"接着写布局");
        assert!(restored[7].content.contains("TOOL_OUTCOME_UNKNOWN"));
        assert_eq!(restored.len(),8);
        assert!(!serde_json::to_string(&restored).unwrap().contains("最后的界面文本"));
        for kind in [agent_core::ProviderKind::DeepSeek,agent_core::ProviderKind::OpenAiResponses,agent_core::ProviderKind::Anthropic] {
            let config = agent_core::ProviderConfig::new("different-provider","p",kind,"https://example.com/v1","key").unwrap();
            let request = agent_core::ProviderRequest::new("s","different-model",restored.clone(),vec![]);
            let body = agent_core::request_body(&config,&request).unwrap().to_string();
            assert!(body.contains("6292"));
            assert!(body.contains("TOOL_OUTCOME_UNKNOWN"));
        }
        // Restoration never executes calls or appends invented success events.
        let count: i64 = store.connection.query_row("SELECT COUNT(*) FROM session_events WHERE session_id='s'",[],|row|row.get(0)).unwrap();
        assert_eq!(count,7);
    }
    std::fs::remove_file(path).unwrap();
}
