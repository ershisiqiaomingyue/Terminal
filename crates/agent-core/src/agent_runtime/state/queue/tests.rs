use super::super::super::{AgentRequestGuard, AgentRuntimeState};

#[test]
fn queued_requests_remain_available_until_dispatched_or_removed() {
    let state = AgentRuntimeState::default();
    let request = || AgentChatRequest {
        model_id: "provider:model".to_string(),
        message: "queued".to_string(),
        jira_context: None,
        artifacts: vec![],
        workspace_path: None,
        session_id: Some("session-a".to_string()),
        attachments: vec![],
        turn_id: None,
        permission_mode: None,
        analysis_mode: false,
        context: vec![],
        tool_policy: Default::default(),
        skill_ids: vec![],
        requested_tool: None,
        reasoning_effort: None,
        task_plan_mode: None,
        task_plan_min_steps: None,
        execution_target: None,
        execution_targets: Vec::new(),
        collaboration_depth: None,
    };
    let (snapshot, should_dispatch) = state
        .enqueue_agent_request("session-a", "queue-a".to_string(), request())
        .expect("enqueue should work");
    assert!(should_dispatch);
    assert_eq!(snapshot.queue.len(), 1);
    let (_, should_dispatch) = state
        .enqueue_agent_request("session-a", "queue-b".to_string(), request())
        .expect("enqueue should work");
    assert!(!should_dispatch);
    let reordered = state
        .reorder_queued_agent_requests("session-a", &["queue-b".into(), "queue-a".into()])
        .expect("queue should reorder");
    assert_eq!(
        reordered
            .queue
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec!["queue-b", "queue-a"]
    );
    assert!(state
        .reorder_queued_agent_requests("session-a", &["queue-b".into(), "queue-b".into()])
        .is_err());
    let (request, snapshot) = state
        .dequeue_agent_request("session-a")
        .expect("first queued request should remain available");
    assert_eq!(request.message, "queued");
    assert_eq!(snapshot.queue.len(), 1);
    let restored = state.restore_queued_agent_request("session-a", request);
    assert_eq!(restored.queue.len(), 2);
    assert!(state.dispatch_queued_request("session-a"));
    assert!(state.read_thread_snapshot("session-b").is_none());
}

#[test]
fn removing_a_queued_request_preserves_other_session_queues() {
    let state = AgentRuntimeState::default();
    let request = |session_id: &str| AgentChatRequest {
        model_id: "provider:model".to_string(),
        message: format!("queued-{session_id}"),
        jira_context: None,
        artifacts: vec![],
        workspace_path: None,
        session_id: Some(session_id.to_string()),
        attachments: vec![],
        turn_id: None,
        permission_mode: None,
        analysis_mode: false,
        context: vec![],
        tool_policy: Default::default(),
        skill_ids: vec![],
        requested_tool: None,
        reasoning_effort: None,
        task_plan_mode: None,
        task_plan_min_steps: None,
        execution_target: None,
        execution_targets: Vec::new(),
        collaboration_depth: None,
    };
    state
        .enqueue_agent_request("session-a", "queue-a".to_string(), request("session-a"))
        .expect("first queue should work");
    state
        .enqueue_agent_request("session-b", "queue-b".to_string(), request("session-b"))
        .expect("second queue should work");

    let snapshot = state
        .remove_queued_agent_request("session-a", "queue-a")
        .expect("target queue item should be removed");
    assert!(snapshot.queue.is_empty());
    assert_eq!(
        state
            .read_thread_snapshot("session-b")
            .expect("other session snapshot should remain")
            .queue,
        vec![crate::QueuedThreadItem {
            id: "queue-b".to_string(),
            content: "queued-session-b".to_string(),
            images: Vec::new(),
            artifacts: Vec::new(),
        }],
    );
}

#[test]
fn queued_session_ids_only_lists_sessions_with_pending_messages() {
    let state = AgentRuntimeState::default();
    let request = AgentChatRequest {
        model_id: "provider:model".to_string(),
        message: "queued".to_string(),
        jira_context: None,
        artifacts: vec![],
        workspace_path: None,
        session_id: Some("session-a".to_string()),
        attachments: vec![],
        turn_id: None,
        permission_mode: None,
        analysis_mode: false,
        context: vec![],
        tool_policy: Default::default(),
        skill_ids: vec![],
        requested_tool: None,
        reasoning_effort: None,
        task_plan_mode: None,
        task_plan_min_steps: None,
        execution_target: None,
        execution_targets: Vec::new(),
        collaboration_depth: None,
    };
    state
        .enqueue_agent_request("session-a", "queue-a".to_string(), request)
        .expect("queue should accept request");
    assert_eq!(state.queued_session_ids(), vec!["session-a".to_string()]);
}

#[test]
fn queue_preview_contains_cached_references_without_image_bytes() {
    use super::preview::queue_items;
    use crate::agent_runtime::state::queue::QueuedAgentRequest;
    use crate::QueuedThreadItem;
    use std::collections::VecDeque;
    let request = serde_json::from_value(serde_json::json!({
            "modelId": "model", "message": "follow up", "workspacePath":"D:/work", "artifacts": [
                {"name":"notes.txt","kind":"file","path":"D:/docs/notes.txt","size":25,"status":"user_attachment","content":"private file contents"},
                {"name":"project","kind":"directory","path":"D:/projects/project","status":"user_attachment"},
                {"name":"work","kind":"directory","path":"D:/work","status":"workspace_context"}
            ],
            "attachments": [
                {"name":"cached.png","mediaType":"image/png","path":"cache/image.png","dataUrl":"large-inline-payload"},
                {"name":"legacy.png","mediaType":"image/png","dataUrl":"legacy-inline-payload"}
            ]
        })).unwrap();
    let preview = queue_items(&VecDeque::from([QueuedAgentRequest {
        id: "queued".into(),
        request,
    }]));
    let encoded = serde_json::to_string(&preview).unwrap();
    assert!(encoded.contains("cache/image.png"));
    assert!(!encoded.contains("inline-payload"));
    assert_eq!(preview[0].images.len(), 1);
    assert_eq!(preview[0].images[0].name, "cached.png");
    assert_eq!(preview[0].artifacts.len(), 2);
    assert_eq!(preview[0].artifacts[0].name, "notes.txt");
    assert!(preview[0]
        .artifacts
        .iter()
        .all(|artifact| artifact.path != "D:/work"));
    assert!(!encoded.contains("private file contents"));
    // Existing durable snapshots predate thumbnail metadata.
    let legacy: QueuedThreadItem =
        serde_json::from_str(r#"{"id":"old","content":"text"}"#).unwrap();
    assert!(legacy.images.is_empty());
    assert!(legacy.artifacts.is_empty());
}

/// 后台任务完成通知会被塞进队列当作 turn 派发，但它不是用户写的消息。
/// 一旦漏进快照，追加消息卡就会为「用户没发过的消息」弹出来。
#[test]
fn queue_preview_hides_host_generated_background_results() {
    use super::preview::queue_items;
    use crate::agent_runtime::state::queue::QueuedAgentRequest;
    use std::collections::VecDeque;
    let user = serde_json::from_value(serde_json::json!({
        "modelId": "model", "message": "顺便把 lint 也跑一遍", "artifacts": []
    }))
    .unwrap();
    let background = serde_json::from_value(serde_json::json!({
        "modelId": "model",
        "message": "这是 Rust 后台任务完成通知，非用户新指令。",
        "artifacts": [],
        "turnId": format!("{}shell-1", crate::background::BACKGROUND_RESULT_PREFIX)
    }))
    .unwrap();
    let preview = queue_items(&VecDeque::from([
        QueuedAgentRequest {
            id: "user".into(),
            request: user,
        },
        QueuedAgentRequest {
            id: "background".into(),
            request: background,
        },
    ]));
    assert_eq!(preview.len(), 1);
    assert_eq!(preview[0].id, "user");
    assert_eq!(preview[0].content, "顺便把 lint 也跑一遍");
}

#[test]
fn queue_waits_for_an_active_request_in_the_same_session() {
    let state = AgentRuntimeState::default();
    let active = AgentRequestGuard::start(&state, "session-a".to_string(), "turn-a".to_string())
        .expect("request should start");
    let request = AgentChatRequest {
        model_id: "provider:model".to_string(),
        message: "queued".to_string(),
        jira_context: None,
        artifacts: vec![],
        workspace_path: None,
        session_id: Some("session-a".to_string()),
        attachments: vec![],
        turn_id: None,
        permission_mode: None,
        analysis_mode: false,
        context: vec![],
        tool_policy: Default::default(),
        skill_ids: vec![],
        requested_tool: None,
        reasoning_effort: None,
        task_plan_mode: None,
        task_plan_min_steps: None,
        execution_target: None,
        execution_targets: Vec::new(),
        collaboration_depth: None,
    };

    let (_, should_dispatch) = state
        .enqueue_agent_request("session-a", "queue-a".to_string(), request)
        .expect("enqueue should work");
    assert!(!should_dispatch);
    assert!(!state.dispatch_queued_request("session-a"));

    drop(active);
    assert!(state.dispatch_queued_request("session-a"));
}

#[test]
fn queued_request_keeps_its_execution_target() {
    let state = AgentRuntimeState::default();
    let request = AgentChatRequest {
        model_id: "provider:model".to_string(),
        message: "queued".to_string(),
        jira_context: None,
        artifacts: vec![],
        workspace_path: Some("/workspace".to_string()),
        session_id: Some("session-a".to_string()),
        attachments: vec![],
        turn_id: None,
        permission_mode: None,
        analysis_mode: false,
        context: vec![],
        tool_policy: Default::default(),
        skill_ids: vec![],
        requested_tool: None,
        reasoning_effort: None,
        task_plan_mode: None,
        task_plan_min_steps: None,
        execution_target: Some(AgentExecutionTarget {
            kind: "ssh".to_string(),
            connection_id: Some("ssh-a".to_string()),
            profile_id: None,
            root: Some("/workspace".to_string()),
        }),
        execution_targets: Vec::new(),
        collaboration_depth: None,
    };
    state
        .enqueue_agent_request("session-a", "queue-a".to_string(), request)
        .expect("enqueue should work");
    let (request, _) = state
        .dequeue_agent_request("session-a")
        .expect("queue should dequeue");
    assert!(
        matches!(request.execution_target, Some(AgentExecutionTarget { kind, connection_id: Some(connection_id), .. }) if kind == "ssh" && connection_id == "ssh-a")
    );
}

#[test]
fn queue_nack_restores_identity_and_original_order() {
    let state = AgentRuntimeState::default();
    let request = |message: &str| AgentChatRequest {
        model_id: "provider:model".to_string(),
        message: message.to_string(),
        jira_context: None,
        artifacts: vec![],
        workspace_path: None,
        session_id: Some("session-a".to_string()),
        attachments: vec![],
        turn_id: None,
        permission_mode: None,
        analysis_mode: false,
        context: vec![],
        tool_policy: Default::default(),
        skill_ids: vec![],
        requested_tool: None,
        reasoning_effort: None,
        task_plan_mode: None,
        task_plan_min_steps: None,
        execution_target: None,
        execution_targets: Vec::new(),
        collaboration_depth: None,
    };
    for (id, message) in [("queue-a", "one"), ("queue-b", "two"), ("queue-c", "three")] {
        state
            .enqueue_agent_request("session-a", id.to_string(), request(message))
            .expect("enqueue should work");
    }

    let (claim, snapshot) = state
        .claim_queued_agent_request("session-a", "queue-b", "lease-b".to_string())
        .expect("claim should not fail")
        .expect("queue-b should be claimable");
    assert_eq!(claim.id, "queue-b");
    assert_eq!(claim.request.message, "two");
    assert_eq!(
        snapshot
            .queue
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec!["queue-a", "queue-c"]
    );

    let restored = state
        .nack_agent_queue_claim("session-a", "queue-b", "lease-b")
        .expect("nack should restore the claim");
    assert_eq!(
        restored
            .queue
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec!["queue-a", "queue-b", "queue-c"]
    );
}

#[test]
fn queue_claim_has_single_consumer_and_ack_is_lease_checked() {
    let state = AgentRuntimeState::default();
    let request = AgentChatRequest {
        model_id: "provider:model".to_string(),
        message: "steer exactly once".to_string(),
        jira_context: None,
        artifacts: vec![],
        workspace_path: None,
        session_id: Some("session-a".to_string()),
        attachments: vec![],
        turn_id: None,
        permission_mode: None,
        analysis_mode: false,
        context: vec![],
        tool_policy: Default::default(),
        skill_ids: vec![],
        requested_tool: None,
        reasoning_effort: None,
        task_plan_mode: None,
        task_plan_min_steps: None,
        execution_target: None,
        execution_targets: Vec::new(),
        collaboration_depth: None,
    };
    state
        .enqueue_agent_request("session-a", "queue-a".to_string(), request)
        .expect("enqueue should work");
    let (claim, snapshot) = state
        .claim_queued_agent_request("session-a", "queue-a", "lease-a".to_string())
        .expect("first claim should work")
        .expect("message should be pending");
    assert!(snapshot.queue.is_empty());
    assert!(state
        .claim_queued_agent_request("session-a", "queue-a", "lease-b".to_string())
        .expect("second claim should not error")
        .is_none());
    assert!(state
        .ack_agent_queue_claim("session-a", &claim.id, "wrong-lease")
        .is_err());
    state
        .ack_agent_queue_claim("session-a", &claim.id, &claim.lease_id)
        .expect("owner should acknowledge the claim");
    assert!(state.queued_agent_request("session-a", "queue-a").is_none());
}

#[test]
fn interrupting_an_active_turn_does_not_remove_pending_queue_items() {
    let state = AgentRuntimeState::default();
    let active = AgentRequestGuard::start(&state, "session-a".to_string(), "turn-a".to_string())
        .expect("active request should start");
    let request = AgentChatRequest {
        model_id: "provider:model".to_string(),
        message: "keep after interrupt".to_string(),
        jira_context: None,
        artifacts: vec![],
        workspace_path: None,
        session_id: Some("session-a".to_string()),
        attachments: vec![],
        turn_id: None,
        permission_mode: None,
        analysis_mode: false,
        context: vec![],
        tool_policy: Default::default(),
        skill_ids: vec![],
        requested_tool: None,
        reasoning_effort: None,
        task_plan_mode: None,
        task_plan_min_steps: None,
        execution_target: None,
        execution_targets: Vec::new(),
        collaboration_depth: None,
    };
    state
        .enqueue_agent_request("session-a", "queue-a".to_string(), request)
        .expect("enqueue should work while another turn is active");
    active
        .cancellation()
        .store(true, std::sync::atomic::Ordering::Relaxed);
    drop(active);

    let queued = state
        .queued_agent_request("session-a", "queue-a")
        .expect("interrupt must preserve pending work");
    assert_eq!(queued.message, "keep after interrupt");
    assert!(state.dispatch_queued_request("session-a"));
}
use crate::{AgentChatRequest, AgentExecutionTarget};
