use super::*;
use crate::{MessageRole, ModelTranscript, ProviderConfig, ProviderKind, ProviderRequest};

fn row(order: u64, item: Value) -> Value {
    json!({"type":"process_item", "turn_id":"t", "execution_started_at":10,
        "snapshot_revision":1, "process_order":order, "item":item})
}

fn user() -> Value {
    row(
        0,
        json!({"kind":"user_message", "id":"user:t", "content":"只做接收展示"}),
    )
}

#[test]
fn native_replay_keeps_selected_local_directory_after_environment_or_model_change() {
    let events = [
        row(
            0,
            json!({"kind":"user_message", "id":"user:t", "content":"用这里的产物",
        "artifacts":[{"kind":"directory","name":"out","path":"D:/CarAdapter/out","content":"never replay inline bytes"}]}),
        ),
        row(
            1,
            json!({"kind":"agent_message", "id":"answer", "content":"已检查"}),
        ),
    ];
    let messages = replay_runtime_turn(&events, "t").unwrap();
    assert_eq!(messages[0].source_message_id.as_deref(), Some("user:t"));
    assert!(messages[0].content.contains("D:/CarAdapter/out"));
    assert!(!messages[0].content.contains("never replay inline bytes"));
    assert_protocols(&messages, &["D:/CarAdapter/out"]);
}

fn call(id: &str, status: &str) -> Value {
    json!({"kind":"tool_call", "id":id, "name":"write_local_file", "input":{"path":"MainActivity.java", "content":"actual source"}, "status":status})
}

fn assert_protocols(messages: &[AgentMessage], expected: &[&str]) {
    let snapshot = ModelTranscript {
        version: 1,
        scope: "portable".into(),
        turn_id: "t".into(),
        runtime_context: String::new(),
        messages: messages.to_vec(),
        images: vec![],
        last_request: None,
    };
    assert!(
        snapshot.is_replayable("portable"),
        "all calls must be paired"
    );
    assert!(messages
        .iter()
        .all(|message| message.responses_reasoning_items.is_empty()
            && message.anthropic_thinking_blocks.is_empty()
            && message.anthropic_content_blocks.is_empty()));
    for kind in [
        ProviderKind::OpenAi,
        ProviderKind::OpenAiCompatible,
        ProviderKind::DeepSeek,
        ProviderKind::OpenAiResponses,
        ProviderKind::Anthropic,
    ] {
        let config =
            ProviderConfig::new("new-provider", "p", kind, "https://example.com/v1", "key")
                .unwrap();
        let request = ProviderRequest::new("same-session", "new-model", messages.to_vec(), vec![]);
        let body = crate::request_body(&config, &request).unwrap().to_string();
        for text in expected {
            assert!(body.contains(text), "{kind:?}: missing {text}");
        }
        assert!(!body.contains("signature"));
    }
}

#[test]
fn ordered_replay_preserves_progress_tools_steers_and_final_after_a_route_change() {
    let mut events = vec![
        row(
            5,
            json!({"kind":"agent_message","id":"final","content":"布局已更新","pending":false}),
        ),
        user(),
        row(
            1,
            json!({"kind":"agent_message","id":"progress","content":"先恢复 Activity","pending":false}),
        ),
        row(2, call("write", "running")),
        row(
            3,
            json!({"kind":"user_message","id":"steer","content":"还要显示长按"}),
        ),
        row(
            4,
            json!({"kind":"agent_message","id":"ack","content":"继续写布局","pending":false}),
        ),
        json!({"type":"tool_call_finished","turn_id":"t","call_id":"write","tool_name":"write_local_file",
            "input":{"path":"MainActivity.java","content":"actual source"},"output":{"bytes":6292},"is_error":false}),
    ];
    let messages = replay_runtime_turn(&events, "t").unwrap();
    assert_eq!(
        messages
            .iter()
            .map(|message| message.role.clone())
            .collect::<Vec<_>>(),
        vec![
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::Assistant,
            MessageRole::Tool,
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::Assistant
        ]
    );
    assert_eq!(messages[1].content, "先恢复 Activity");
    assert!(messages[3].content.contains("6292"));
    assert_eq!(messages[4].content, "还要显示长按");
    assert_eq!(messages[6].content, "布局已更新");
    assert_protocols(&messages, &["MainActivity.java", "6292", "还要显示长按"]);
    let mut stale = events[2].clone();
    stale["snapshot_revision"] = json!(0);
    stale["item"]["content"] = json!("obsolete");
    events.push(stale);
    assert_eq!(replay_runtime_turn(&events, "t").unwrap(), messages);
    assert!(replay_runtime_turn(&events, "another-turn").is_none());
}

#[test]
fn dsh_style_recovery_distinguishes_never_started_from_unknown_effects() {
    let events = vec![
        user(),
        row(1, call("not-started", "pending")),
        row(2, call("started", "running")),
    ];
    let messages = replay_runtime_turn(&events, "t").unwrap();
    assert!(messages[2].content.contains("TOOL_NOT_STARTED"));
    assert!(messages[4].content.contains("TOOL_OUTCOME_UNKNOWN"));
    assert!(messages[4].content.contains("先核验外部状态"));
    assert_protocols(&messages, &["TOOL_NOT_STARTED", "TOOL_OUTCOME_UNKNOWN"]);
}

#[test]
fn failures_and_denials_remain_failures_and_incomplete_arguments_are_not_reconstructed() {
    let mut failed = call("failed", "error");
    failed["output"] = json!({"error":"permission denied"});
    let mut fragment = call("fragment", "pending");
    fragment["input"] = Value::Null;
    fragment["input_buffer"] = json!("{\"path\":");
    let messages = replay_runtime_turn(&[user(), row(1, failed), row(2, fragment)], "t").unwrap();
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.role == MessageRole::Tool)
            .count(),
        1
    );
    assert!(messages[2].content.contains("历史操作失败记录"));
    assert!(messages[2].content.contains("permission denied"));
    assert!(messages[3].content.contains("参数未完整保存"));
    assert_protocols(&messages, &["permission denied"]);
}

#[test]
fn replay_uses_latest_execution_and_requires_a_complete_user_anchor() {
    let mut old = user();
    old["execution_started_at"] = json!(1);
    old["item"]["content"] = json!("obsolete execution");
    let events = vec![old, user(), row(1, call("write", "running"))];
    let messages = replay_runtime_turn(&events, "t").unwrap();
    assert_eq!(messages[0].content, "只做接收展示");
    assert_eq!(messages.len(), 3);
    assert!(replay_runtime_turn(&events[2..], "t").is_none());
}
