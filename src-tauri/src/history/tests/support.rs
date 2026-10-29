use super::*;

pub(super) fn queued_request(session_id: &str, message: &str) -> AgentChatRequest {
    AgentChatRequest {
        model_id: "provider:model".to_string(),
        message: message.to_string(),
        jira_context: None,
        artifacts: vec![],
        workspace_path: Some("/workspace".to_string()),
        session_id: Some(session_id.to_string()),
        attachments: vec![],
        turn_id: None,
        permission_mode: None,
        analysis_mode: false,
        context: vec![],
        tool_policy: AgentToolPolicy::Auto,
        skill_ids: vec![],
        requested_tool: None,
        reasoning_effort: None,
        task_plan_mode: None,
        task_plan_min_steps: None,
        execution_target: Some(AgentExecutionTarget {
            kind: "ssh".to_string(),
            connection_id: Some("ssh-frozen".to_string()),
            profile_id: Some("profile-frozen".to_string()),
            root: Some("/workspace".to_string()),
        }),
        execution_targets: Vec::new(),
        collaboration_depth: None,
    }
}

pub(super) fn temporary_history_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "{}-{label}.sqlite3",
        next_history_id("history-test")
    ))
}

pub(super) fn typed_agent_checkpoint(
    id: &str,
    model_id: &str,
    covered_through_message_id: &str,
    format_version: u8,
) -> AgentContextCheckpoint {
    AgentContextCheckpoint {
        id: id.to_string(),
        summary: "legacy checkpoint summary".to_string(),
        evidence_policy_version: 3,
        covered_through_message_id: Some(covered_through_message_id.to_string()),
        payload: Some(AgentContextCheckpointPayload {
            format_version,
            producer_model_id: Some(model_id.to_string()),
            entries: vec![
                AgentContextCheckpointEntry::SystemConstraint {
                    content: "system constraint".to_string(),
                    source_message_id: Some("system-source".to_string()),
                },
                AgentContextCheckpointEntry::UserConstraint {
                    content: "user constraint".to_string(),
                    source_message_id: Some("user-source".to_string()),
                },
                AgentContextCheckpointEntry::AssistantConclusion {
                    content: "assistant conclusion".to_string(),
                    source_message_id: Some("assistant-source".to_string()),
                },
                AgentContextCheckpointEntry::ToolEvidence {
                    call_id: "call-1".to_string(),
                    tool_name: "read_local_file".to_string(),
                    input: serde_json::json!({"path":"D:/workspace/file.txt"}),
                    output: "durable tool evidence".to_string(),
                    source_message_id: Some("tool-source".to_string()),
                },
            ],
        }),
    }
}

pub(super) fn collaboration_input(agent_count: usize) -> CollaborationJobInput {
    CollaborationJobInput {
        thread_id: "parent-thread".to_string(),
        parent_turn_id: Some("parent-turn".to_string()),
        goal: "combine worker results".to_string(),
        goal_id: None,
        workspace_path: Some("D:/workspace/demo".to_string()),
        permission_mode: Some("ask".to_string()),
        execution_target: Some(serde_json::json!({ "kind": "local" })),
        agent_config: CollaborationAgentConfig {
            model_id: "provider:model".to_string(),
            analysis_mode: false,
            tool_policy: AgentToolPolicy::Auto,
            skill_ids: vec![],
            requested_tool: None,
            reasoning_effort: None,
            task_plan_mode: None,
            task_plan_min_steps: None,
            depth: 0,
        },
        agents: (0..agent_count)
            .map(|index| CollaborationAgentInput {
                title: format!("worker-{index}"),
                instruction: format!("do part {index}"),
                workspace_path: None,
                permission_mode: None,
            })
            .collect(),
    }
}

pub(super) fn seed_parent_thread(store: &HistoryStore) {
    seed_thread(store, "parent-thread");
}

pub(super) fn seed_thread(store: &HistoryStore, thread_id: &str) {
    store
        .upsert_input(HistorySessionInput {
            id: thread_id.to_string(),
            title: "Parent".to_string(),
            updated_at: 1,
            archived: false,
            schema_version: HISTORY_SCHEMA_VERSION,
            jira_context: None,
            artifacts: serde_json::json!([]),
            messages: serde_json::json!([]),
            metadata: serde_json::json!({}),
            report: None,
            usage: None,
            events: None,
        })
        .expect("parent thread should persist");
}
