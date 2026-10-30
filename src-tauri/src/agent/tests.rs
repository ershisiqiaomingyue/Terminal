#[test]
/// 新名字 `.android-toolkit` 优先；写在旧目录 `.android-tool` 里的项目配置
/// 在迁移之前也必须照常生效（2.0 升级期间两种目录会同时存在）。
fn workspace_agent_permissions_and_instructions_read_both_directory_names() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir_all(root.join(".android-toolkit")).unwrap();
    std::fs::create_dir_all(root.join(".android-tool")).unwrap();
    std::fs::write(
        root.join(".android-tool/permissions.json"),
        r#"{"allowedScopes":["read:legacy"]}"#,
    )
    .unwrap();
    std::fs::write(root.join(".android-tool/CODEX.md"), "旧目录规则").unwrap();
    assert_eq!(
        agent_core::load_workspace_permissions(&root).allowed_scopes,
        ["read:legacy"]
    );
    assert!(agent_core::load_workspace_agent_context_at(&root)
        .instructions
        .iter()
        .any(|(_, text)| text == "旧目录规则"));

    // 两个目录都在时新名字说了算，迁移后的写入不会被旧文件覆盖。
    std::fs::write(
        root.join(".android-toolkit/permissions.json"),
        r#"{"allowedScopes":["read:current"]}"#,
    )
    .unwrap();
    assert_eq!(
        agent_core::load_workspace_permissions(&root).allowed_scopes,
        ["read:current"]
    );
}

#[test]
fn instruction_files_load_in_every_permission_mode_without_granting_access() {
    let temp = tempfile::tempdir().unwrap();
    let global = temp.path().join("global");
    let project = temp.path().join("project");
    std::fs::create_dir_all(global.join("plugins")).unwrap();
    std::fs::create_dir_all(project.join(".android-tool")).unwrap();
    std::fs::write(global.join("agent.md"), "优先使用中文").unwrap();
    std::fs::write(global.join("CODEX.md"), "全局偏好").unwrap();
    std::fs::write(global.join("plugins/AGENTS.md"), "插件内部规则不自动注入").unwrap();
    std::fs::write(project.join("agent.md"), "项目规则").unwrap();
    std::fs::write(project.join(".android-tool/CODEX.md"), "项目配置目录规则").unwrap();
    std::fs::write(
        project.join(".android-tool/permissions.json"),
        r#"{"allowedScopes":["read:approved"]}"#,
    )
    .unwrap();
    let globals = agent_core::load_global_agent_instructions(&global);
    assert_eq!(globals.len(), 2);
    assert!(globals.iter().any(|(_, text)| text == "优先使用中文"));
    assert!(!globals.iter().any(|(_, text)| text.contains("插件内部")));
    assert!(agent_core::load_global_agent_instructions(&temp.path().join("missing")).is_empty());
    let mut request: agent_core::AgentChatRequest = serde_json::from_value(serde_json::json!({
        "modelId":"fixture", "message":"检查", "workspacePath":project, "artifacts":[]
    }))
    .unwrap();
    for mode in ["ask", "risk", "full"] {
        request.permission_mode = Some(mode.into());
        let context = super::turn_workspace_context(&request, mode).unwrap();
        assert!(
            context
                .instructions
                .iter()
                .any(|(_, text)| text == "项目规则"),
            "{mode}"
        );
        assert!(
            context
                .instructions
                .iter()
                .any(|(_, text)| text == "项目配置目录规则"),
            "{mode}"
        );
        assert_eq!(context.permissions.allowed_scopes, ["read:approved"]);
        assert_eq!(permission_requires_confirmation(mode, true), mode != "full");
    }
    // Reload each turn so edits to global preferences take effect immediately.
    std::fs::write(global.join("agent.md"), "新的中文偏好").unwrap();
    assert!(agent_core::load_global_agent_instructions(&global)
        .iter()
        .any(|(_, text)| text == "新的中文偏好"));
}

#[test]
fn dual_workspace_keeps_local_project_but_remote_roots_ignore_local_attachments() {
    let root = env!("CARGO_MANIFEST_DIR");
    let mut request: agent_core::AgentChatRequest = serde_json::from_value(serde_json::json!({
        "modelId": "fixture", "message": "检查项目", "workspacePath": root, "artifacts": []
    }))
    .unwrap();
    agent_core::bind_workspace_artifact(&mut request).unwrap();
    assert!(super::turn_workspace_context(&request, "ask").is_some());
    for kind in ["ssh", "wsl"] {
        request.execution_target = Some(agent_core::AgentExecutionTarget {
            kind: kind.into(),
            connection_id: Some("remote".into()),
            profile_id: Some("distribution".into()),
            root: Some("/remote-project".into()),
        });
        let local = super::turn_workspace_context(&request, "ask").unwrap();
        assert_eq!(local.root, std::path::PathBuf::from(root));
        request.workspace_path = Some("/remote-project".into());
        assert!(
            super::turn_workspace_context(&request, "full").is_none(),
            "{kind}"
        );
        request.workspace_path = Some(root.into());
    }
}

#[test]
fn project_configuration_supplements_instead_of_replacing_history() {
    let history = serde_json::from_value::<Vec<agent_core::AgentContextTurn>>(serde_json::json!([
        {"role":"user", "content":"Remember 4163", "source":"history", "messageId":"u"},
        {"role":"assistant", "content":"Remembered", "source":"history", "messageId":"a"}
    ]))
    .unwrap();
    let configuration = serde_json::from_value::<Vec<agent_core::AgentContextTurn>>(serde_json::json!([
            {"role":"system", "content":"Use the custom Gradle command", "source":"project_configuration"}
        ])).unwrap();
    let combined = super::merge_history_and_request_context(history, configuration);
    assert_eq!(combined.len(), 3);
    assert_eq!(combined[0].content, "Remember 4163");
    assert_eq!(combined[1].content, "Remembered");
    assert_eq!(combined[2].source.as_deref(), Some("project_configuration"));
    assert!(combined
        .iter()
        .any(|turn| turn.source.as_deref() == Some("history")));
}

#[test]
fn streaming_usage_does_not_replace_compaction_progress_item() {
    let running = serde_json::json!({"compaction_id":"c", "compaction_status":"running"});
    let completed = serde_json::json!({"compaction_id":"c", "compaction_status":"completed"});
    let usage = serde_json::json!({"streaming":true, "context_tokens":1234});
    assert_eq!(
        super::context_item_id("t", &running),
        super::context_item_id("t", &completed)
    );
    assert_ne!(
        super::context_item_id("t", &completed),
        super::context_item_id("t", &usage)
    );
    assert_ne!(
        super::context_item_id("t", &running),
        super::context_item_id("next", &running)
    );
}

#[test]
fn compaction_progress_reports_no_percentage_until_there_is_one_to_measure() {
    use super::events::compaction_progress_payload;
    // Opening frame: the strip must stay on the indeterminate sweep and keep
    // showing "等待摘要响应" instead of a determinate 0% bar.
    let opening = compaction_progress_payload("s", "c", 0, 12, 9_000, 3_072);
    assert_eq!(opening["compaction_status"], serde_json::json!("running"));
    assert_eq!(opening["compaction_summary_chars"], serde_json::json!(0));
    assert_eq!(opening["compaction_progress"], serde_json::Value::Null);
    // A summary with no character budget cannot be turned into a percentage.
    assert_eq!(
        compaction_progress_payload("s", "c", 400, 12, 9_000, 0)["compaction_progress"],
        serde_json::Value::Null
    );
    // 1536 / 3072 * 95 = 47.5 -> 48.
    assert_eq!(
        compaction_progress_payload("s", "c", 1_536, 12, 9_000, 3_072)["compaction_progress"],
        serde_json::json!(48)
    );
    // Overrun is clamped below the completed value.
    assert_eq!(
        compaction_progress_payload("s", "c", 99_999, 12, 9_000, 3_072)["compaction_progress"],
        serde_json::json!(95)
    );
}

#[test]
fn terminal_diagnostics_persist_without_persisting_transient_status_noise() {
    assert!(super::should_persist_agent_payload(&serde_json::json!({
        "type":"status", "status":"repetitive_output", "terminalDiagnostic":true
    })));
    assert!(!super::should_persist_agent_payload(&serde_json::json!({
        "type":"status", "status":"requesting_model"
    })));
}

#[test]
fn terminal_boundaries_keep_notices_but_normal_answers_do_not_claim_task_failure() {
    use super::events::terminal_status_notice;
    for reason in [
        "output_limit_reached",
        "duplicate_tool_call",
        "malformed_tool_call",
        "tool_execution_boundary",
    ] {
        let notice = terminal_status_notice(
            &json!({"type":"completed", "session_id":"s", "termination_reason":reason}),
        )
        .unwrap();
        assert_eq!(notice["terminalDiagnostic"], true);
        assert_eq!(notice["status"], reason);
        assert!(super::should_persist_agent_payload(&notice));
        assert!(!notice["message"].as_str().unwrap().is_empty());
    }
    for reason in ["model_finished", "final_response", "repetitive_output"] {
        assert!(
            terminal_status_notice(&json!({"type":"completed", "termination_reason":reason}))
                .is_none()
        );
    }
    let cancellation =
        terminal_status_notice(&json!({"type":"cancelled", "session_id":"s"})).unwrap();
    assert_eq!(cancellation["status"], "cancelled");
    assert!(cancellation["message"].as_str().unwrap().contains("已保留"));
    assert!(!cancellation["message"].as_str().unwrap().contains("用户"));
    assert!(terminal_status_notice(&json!({"type":"error", "message":"error"})).is_none());
}

#[test]
fn watchdog_cause_remains_a_timeout_instead_of_becoming_user_cancellation() {
    let state = AgentRuntimeState::default();
    let guard = AgentRequestGuard::start(&state, "idle-test".into(), "turn".into()).unwrap();
    state
        .cancel_if_turn_idle(
            "idle-test",
            "turn",
            &guard.cancellation(),
            std::time::Duration::ZERO,
        )
        .unwrap();
    let message = super::cancellation_message(&guard.cancellation_reason());
    let failure = crate::agent_errors::AgentCommandError::runtime(message);
    assert_eq!(failure.code, "turn_idle_timeout");
    assert!(failure.retryable);
    assert!(failure.message.contains("未收到模型或工具进度"));
    assert!(!failure.message.contains("请求已取消"));
    assert_eq!(
        super::cancellation_message(&Arc::new(std::sync::Mutex::new(None))),
        "请求已取消"
    );
}
use std::path::Path;

fn image(name: &str) -> agent_core::AgentImageAttachment {
    agent_core::AgentImageAttachment {
        path: None,
        name: name.to_string(),
        media_type: "image/png".to_string(),
        data_url: "data:image/png;base64,AQID".to_string(),
    }
}

#[test]
fn prepared_images_refresh_changed_files_and_missing_snapshots() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("cache");
    let original = temp.path().join("screen.png");
    std::fs::write(&original, b"one").unwrap();
    let path = original.to_str().unwrap();
    let first = super::attachments::prepare_local_attachment(&root, path, None).unwrap();
    let repeated = super::attachments::prepare_local_attachment(&root, path, Some(&first)).unwrap();
    assert_eq!(
        first.image.as_ref().unwrap().path,
        repeated.image.as_ref().unwrap().path
    );
    assert!(first.image.as_ref().unwrap().data_url.is_empty());
    std::fs::write(&original, b"new longer image").unwrap();
    let changed = super::attachments::prepare_local_attachment(&root, path, Some(&first)).unwrap();
    assert_ne!(
        first.image.as_ref().unwrap().path,
        changed.image.as_ref().unwrap().path
    );
    std::fs::remove_file(changed.image.as_ref().unwrap().path.as_ref().unwrap()).unwrap();
    let repaired =
        super::attachments::prepare_local_attachment(&root, path, Some(&changed)).unwrap();
    assert_eq!(
        changed.image.as_ref().unwrap().path,
        repaired.image.as_ref().unwrap().path
    );
    let normalized = super::attachments::normalize_cached_images(
        &root,
        &[repaired.image.unwrap()],
        Some(true),
        "owner",
    )
    .unwrap();
    assert_eq!(normalized[0].source_message_id, "owner");
    std::fs::remove_file(&original).unwrap();
    assert!(super::attachments::prepare_local_attachment(&root, path, Some(&changed)).is_err());
}

#[test]
fn text_turn_on_non_vision_model_keeps_history_as_text_without_replaying_bytes() {
    let history = vec![agent_core::AgentContextTurn {
        role: "user".to_string(),
        content: "earlier screenshot".to_string(),
        source: Some("history".to_string()),
        message_id: Some("user-history".to_string()),
        turn_id: Some("turn-history".to_string()),
        model_id: Some("vision-model".to_string()),
        attachments: vec![image("history.png")],
        artifacts: Vec::new(),
    }];

    let restored = super::normalize_request_image_attachments(
        Path::new(""),
        &[],
        &history,
        Some(false),
        "user-current",
    )
    .expect("historical images must not reject a new text turn");
    assert!(restored.is_empty());
    assert!(super::normalize_request_image_attachments(
        Path::new(""),
        &[image("new.png")],
        &history,
        Some(false),
        "user-current",
    )
    .expect_err("a new image still requires vision support")
    .contains("不支持图片输入"));
}

fn terminal_stream_snapshot(
    state: &agent_core::AgentRuntimeState,
    status: agent_core::TurnStatus,
    error: Option<&str>,
) -> agent_core::ThreadSnapshot {
    let session_id = "start-failure-guard";
    let turn_id = "streamed-turn";
    state
        .apply_thread_event(agent_core::ThreadEvent::TurnStarted {
            thread_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
        })
        .expect("turn should start");
    state
        .apply_thread_event(agent_core::ThreadEvent::ItemAdded {
            thread_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            item: agent_core::ThreadItem::UserMessage {
                id: format!("user:{turn_id}"),
                content: "write a long response".to_string(),
                attachments: vec![],
                artifacts: vec![],
            },
        })
        .expect("user item should be recorded");
    state
        .apply_thread_event(agent_core::ThreadEvent::AgentDelta {
            thread_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            item_id: format!("assistant:{turn_id}"),
            text: "partial streamed body ".repeat(80),
        })
        .expect("streamed body should be recorded");
    state
        .apply_thread_event(agent_core::ThreadEvent::TurnCompleted {
            thread_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            status,
            error: error.map(str::to_string),
        })
        .expect("turn should settle");
    {
        let mut threads = state.threads.lock().expect("thread state lock");
        let turn = threads
            .get_mut(session_id)
            .and_then(|snapshot| snapshot.active_turn.as_mut())
            .expect("settled turn");
        turn.started_at_ms = 100;
        turn.finished_at_ms = Some(900);
    }
    state
        .read_thread_snapshot(session_id)
        .expect("settled snapshot")
}

#[test]
fn start_failure_fallback_does_not_reset_a_cancelled_stream() {
    let state = agent_core::AgentRuntimeState::default();
    let before = terminal_stream_snapshot(&state, agent_core::TurnStatus::Cancelled, None);
    assert!(super::public_text_for_items(&before.items).len() > 1_100);

    assert!(!super::apply_unadmitted_start_failure_snapshot(
        &state,
        "start-failure-guard",
        "streamed-turn",
        "write a long response",
        "请求已取消",
        Some(before.revision),
    ));
    assert_eq!(
        state.read_thread_snapshot("start-failure-guard"),
        Some(before)
    );
}

#[test]
fn start_failure_fallback_does_not_reset_a_failed_stream() {
    let state = agent_core::AgentRuntimeState::default();
    let before = terminal_stream_snapshot(
        &state,
        agent_core::TurnStatus::Failed,
        Some("provider failed"),
    );
    assert_eq!(before.active_turn.as_ref().unwrap().started_at_ms, 100);
    assert_eq!(
        before.active_turn.as_ref().unwrap().finished_at_ms,
        Some(900)
    );

    assert!(!super::apply_unadmitted_start_failure_snapshot(
        &state,
        "start-failure-guard",
        "streamed-turn",
        "write a long response",
        "provider failed",
        Some(before.revision),
    ));
    assert_eq!(
        state.read_thread_snapshot("start-failure-guard"),
        Some(before)
    );
}

#[test]
fn start_failure_fallback_only_materializes_an_unadmitted_turn() {
    let state = agent_core::AgentRuntimeState::default();
    assert!(super::apply_unadmitted_start_failure_snapshot(
        &state,
        "new-session",
        "never-started",
        "hello",
        "provider configuration is invalid",
        None,
    ));
    let failed = state.read_thread_snapshot("new-session").unwrap();
    assert_eq!(failed.active_turn.as_ref().unwrap().id, "never-started");
    assert_eq!(
        failed.active_turn.as_ref().unwrap().status,
        agent_core::TurnStatus::Failed
    );
    assert_eq!(
        super::public_text_for_items(&failed.items),
        "provider configuration is invalid"
    );
    let failure_revision = failed.revision;

    state
        .apply_thread_event(agent_core::ThreadEvent::TurnStarted {
            thread_id: "new-session".to_string(),
            turn_id: "new-live-turn".to_string(),
        })
        .expect("new turn should start");
    let before = state.read_thread_snapshot("new-session").unwrap();
    assert!(!super::apply_unadmitted_start_failure_snapshot(
        &state,
        "new-session",
        "never-started",
        "hello",
        "late failure",
        Some(failure_revision),
    ));
    assert_eq!(state.read_thread_snapshot("new-session"), Some(before));

    let owned = agent_core::AgentRuntimeState::default();
    let guard = agent_core::AgentRequestGuard::start(
        &owned,
        "owned-session".to_string(),
        "new-owner".to_string(),
    )
    .expect("request owner should be reserved");
    assert!(!super::apply_unadmitted_start_failure_snapshot(
        &owned,
        "owned-session",
        "late-turn",
        "hello",
        "late failure",
        None,
    ));
    assert!(owned.read_thread_snapshot("owned-session").is_none());
    drop(guard);
}

#[test]
fn user_attachment_metadata_hides_workspace_but_keeps_explicit_files() {
    let artifact = |path: &str, kind: &str| agent_core::LocalArtifact {
        path: path.into(),
        name: "selected".into(),
        kind: kind.into(),
        size: Some(12),
        modified_at: None,
        status: "ready".into(),
        content: Some("private file body".into()),
        content_truncated: false,
    };
    let item = super::user_message_item(
        "user:t".into(),
        "look".into(),
        &[agent_core::AgentImageAttachment {
            path: Some("cache/images/i.png".into()),
            name: "i.png".into(),
            media_type: "image/png".into(),
            data_url: "data:image/png;base64,aA==".into(),
        }],
        &[
            artifact("D:/workspace/demo", "directory"),
            artifact("D:/workspace/demo/selected", "directory"),
            artifact("attachment://file.txt", "file"),
        ],
        Some("D:/workspace/demo"),
    );
    let value = serde_json::to_value(item).unwrap();
    assert_eq!(value["attachments"].as_array().unwrap().len(), 1);
    assert_eq!(value["attachments"][0]["path"], "cache/images/i.png");
    assert_eq!(value["artifacts"].as_array().unwrap().len(), 2);
    assert_eq!(value["artifacts"][0]["path"], "D:/workspace/demo/selected");
    assert!(value["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .all(|a| a.get("content").is_none()));

    let mut explicit = artifact("D:/workspace/demo", "directory");
    explicit.status = "user_attachment".into();
    let mut implicit = artifact("D:/local/project", "directory");
    implicit.status = "workspace_context".into();
    let value = serde_json::to_value(super::user_message_item(
        "user:t".into(),
        "look".into(),
        &[],
        &[explicit, implicit],
        Some("D:/workspace/demo"),
    ))
    .unwrap();
    assert_eq!(value["artifacts"].as_array().unwrap().len(), 1);
    assert_eq!(value["artifacts"][0]["path"], "D:/workspace/demo");
    assert_eq!(value["artifacts"][0]["status"], "user_attachment");
}

use super::{
    active_steer_sender, active_turn_owned_by, apply_steer_runtime_inputs,
    default_tool_environment, environment_tool_registry, execute_tool_call_interruptible,
    is_known_official_provider_base_url, is_selected_path, is_skill_catalog_question,
    jira_description_to_text, mcp_tool_name, output_limit_reached, parse_analysis_report,
    parse_jira_url_value, parse_relay_quota, provider_error_code, provider_model_is_listed,
    relay_auth_failure_message, relay_quota_endpoints,
    relay_response_declares_failure, tool_execution_scope, AgentModelSettings,
    AgentProviderSettings, AgentToolEnvironment, AgentToolPolicy, BoundSshHost,
    InterruptibleToolExecution, ToolExecutionLanes,
};
use agent_core::{
    expand_agent_models, expand_agent_providers, is_allowed_command,
    path_access_requires_confirmation, permission_requires_confirmation, resolve_agent_model,
    AgentMessage, AgentRequestGuard, AgentRuntimeState, AgentSteerRequest, AllowPermission,
    LocalArtifact, LocalToolContext, ProviderImageAttachment, ProviderKind, ToolDefinition,
    ToolHandler, ToolRegistry, WslRuntimeConfig,
};
use futures_util::StreamExt;
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

struct CancelAwareTool {
    cancellation: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
}

#[tokio::test]
async fn queued_steer_does_not_cancel_the_provider_or_its_accepted_context() {
    let mut parser = agent_core::ToolStreamState::default();
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    sender
        .send(agent_core::AgentSteerRequest {
            message: "Additional instruction.".into(),
            attachments: vec![],
            artifacts: vec![],
            source_message_id: None,
        })
        .await
        .unwrap();
    let result = agent_core::collect_model_round_with_observer(
        Box::pin(futures_util::stream::iter([
            Ok(agent_core::AgentEvent::TextDelta {
                session_id: "lazy-provider".into(),
                text: "Accepted response.".into(),
            }),
            Ok(agent_core::AgentEvent::Completed {
                session_id: "lazy-provider".into(),
            }),
        ])),
        "lazy-provider",
        &mut parser,
        &mut receiver,
        &AtomicBool::new(false),
        &[],
        |_| {},
    )
    .await
    .unwrap();
    assert!(super::provider_round_accepted_input(&result));
    assert_eq!(result.text, "Accepted response.");
    assert_eq!(
        receiver.try_recv().unwrap().message,
        "Additional instruction."
    );
}

#[tokio::test]
async fn lazy_stream_rejections_reach_the_bounded_startup_recovery_path() {
    use agent_core::{AgentEvent, ModelRoundError, ProviderClientError, ProviderStatusError};
    let events = futures_util::stream::iter([
        Ok(AgentEvent::Status {
            session_id: "lazy-provider".into(),
            status: "provider_waiting".into(),
            message: None,
        }),
        Err(ProviderClientError::Status {
            kind: ProviderStatusError::Unexpected(400),
            message: "context_length_exceeded".into(),
        }),
    ]);
    let mut parser = agent_core::ToolStreamState::default();
    let (_sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let cancellation = AtomicBool::new(false);
    let result = agent_core::collect_model_round_with_observer(
        Box::pin(events),
        "lazy-provider",
        &mut parser,
        &mut receiver,
        &cancellation,
        &[],
        |_| {},
    )
    .await;
    let Err(ModelRoundError::Provider {
        error,
        pending_calls,
    }) = result
    else {
        panic!("lazy HTTP rejection should be a provider round error");
    };
    assert!(pending_calls.is_empty());
    let message = error.to_string();
    assert!(super::provider_startup_error_can_recover(
        &message,
        ProviderKind::OpenAiCompatible,
        false,
        false,
        false,
        0
    ));
    assert!(!super::provider_startup_error_can_recover(
        &message,
        ProviderKind::OpenAiCompatible,
        false,
        false,
        false,
        3
    ));
    assert!(super::provider_startup_error_can_recover(
        "400 unknown field stream_options.include_usage",
        ProviderKind::OpenAiCompatible,
        true,
        false,
        false,
        0
    ));
    assert!(!super::provider_startup_error_can_recover(
        "400 unknown field stream_options.include_usage",
        ProviderKind::OpenAiCompatible,
        true,
        true,
        false,
        0
    ));
    assert!(super::provider_startup_error_can_recover(
        "400 unsupported reasoning.encrypted_content",
        ProviderKind::OpenAiResponses,
        false,
        false,
        false,
        0
    ));
    assert!(!super::provider_startup_error_can_recover(
        "400 unsupported reasoning.encrypted_content",
        ProviderKind::OpenAiResponses,
        false,
        false,
        true,
        0
    ));
    assert!(!super::provider_startup_error_can_recover(
        "401 unauthorized",
        ProviderKind::OpenAiCompatible,
        true,
        false,
        false,
        0
    ));
}

#[test]
fn localized_context_limit_diagnostics_still_enter_bounded_retry() {
    for message in [
        "Claude 已达到模型上下文上限；已保留此前内容，需要压缩上下文后继续。",
        "请求上下文长度超过限制",
        "model_context_window_exceeded",
    ] {
        assert!(super::provider_startup_error_can_recover(
            message,
            ProviderKind::Anthropic,
            false,
            false,
            false,
            0
        ));
        assert!(!super::provider_startup_error_can_recover(
            message,
            ProviderKind::Anthropic,
            false,
            false,
            false,
            3
        ));
    }
}

#[test]
fn process_settlement_preserves_order_and_only_projects_the_tail_result() {
    use agent_core::{ThreadEvent, ThreadItem, ThreadSnapshot};
    let mut snapshot = ThreadSnapshot::new("history");
    snapshot.apply(ThreadEvent::TurnStarted {
        thread_id: "history".into(),
        turn_id: "t".into(),
    });
    snapshot.items = vec![
        ThreadItem::UserMessage {
            id: "u".into(),
            content: "request".into(),
            attachments: vec![],
            artifacts: vec![],
        },
        ThreadItem::TimelineEvent {
            id: "r".into(),
            data: json!({ "type":"reasoning_delta", "text":"reasoning" }),
        },
        ThreadItem::AgentMessage {
            id: "a".into(),
            content: "Before.".into(),
            pending: false,
        },
        ThreadItem::ToolCall {
            id: "c".into(),
            name: "shell".into(),
            input: json!({}),
            input_buffer: None,
            output: Some(json!("ok")),
            live_output: None,
            status: "done".into(),
            tool_group_id: Some("g".into()),
            queued_at_ms: Some(100),
            started_at_ms: Some(110),
            finished_at_ms: Some(120),
            is_error: false,
        },
        ThreadItem::AgentMessage {
            id: "b".into(),
            content: "Final.".into(),
            pending: false,
        },
    ];
    let events = super::process_events_for_snapshot(&snapshot, "t");
    assert_eq!(events.len(), 5);
    assert_eq!(events[0]["item"]["kind"], "user_message");
    assert_eq!(events[1]["item"]["data"]["text"], "reasoning");
    assert_eq!(events[2]["process_order"], 2);
    assert_eq!(events[3]["item"]["output"], "ok");
    assert_eq!(events[3]["item"]["started_at_ms"], 110);
    assert_eq!(
        super::result_text_for_snapshot(&snapshot, "t").as_deref(),
        Some("Final.")
    );
    snapshot.items.pop();
    assert_eq!(
        super::result_text_for_snapshot(&snapshot, "t").as_deref(),
        Some("Before.")
    );
    assert!(super::process_events_for_snapshot(&snapshot, "other").is_empty());
    assert!(super::result_text_for_snapshot(&snapshot, "other").is_none());
    snapshot.items.push(ThreadItem::UserMessage {
            id: "u-steer".into(),
            content: "More detail".into(),
            attachments: vec![json!({ "name":"screen.png", "mediaType":"image/png", "dataUrl":"data:image/png;base64,YQ==" })],
            artifacts: vec![json!({ "path":"attachment://note.txt", "name":"note.txt", "kind":"text", "size":4 })],
        });
    snapshot.items.push(ThreadItem::AgentMessage {
        id: "after".into(),
        content: "Continued.".into(),
        pending: false,
    });
    let segments = super::display_segments_for_snapshot(&snapshot, true);
    assert_eq!(segments.len(), 4);
    assert_eq!(segments[0]["role"], "user");
    assert_eq!(segments[1]["content"], "Before.");
    assert_eq!(segments[1]["process_end"], 4);
    assert_eq!(segments[2]["id"], "u-steer");
    assert_eq!(segments[2]["attachments"][0]["name"], "screen.png");
    assert_eq!(segments[2]["artifacts"][0]["path"], "attachment://note.txt");
    assert_eq!(segments[3]["content"], "Continued.");
    assert_eq!(segments[3]["process_start"], 5);
    assert_eq!(
        super::public_text_for_items(&snapshot.items),
        "Before.\n\nContinued."
    );
}

#[test]
fn reasoning_after_a_reply_is_a_process_boundary_in_live_and_history_projections() {
    use agent_core::{ThreadEvent, ThreadItem, ThreadSnapshot};
    let mut snapshot = ThreadSnapshot::new("history");
    snapshot.apply(ThreadEvent::TurnStarted {
        thread_id: "history".into(),
        turn_id: "t".into(),
    });
    snapshot.items = vec![
        ThreadItem::AgentMessage {
            id: "a".into(),
            content: "已读到配置。".into(),
            pending: false,
        },
        ThreadItem::TimelineEvent {
            id: "r".into(),
            data: json!({"type":"reasoning_delta","origin":"model","text":"检查版本。"}),
        },
        ThreadItem::AgentMessage {
            id: "b".into(),
            content: "检查完成。".into(),
            pending: false,
        },
    ];
    let events = super::process_events_for_snapshot(&snapshot, "t");
    assert_eq!(events[0]["result_item"], false);
    assert_eq!(events[2]["result_item"], true);
    assert_eq!(
        super::result_text_for_snapshot(&snapshot, "t").as_deref(),
        Some("检查完成。")
    );
    assert_eq!(
        super::display_segments_for_snapshot(&snapshot, false)[0]["content"],
        "检查完成。"
    );
    assert_eq!(
        super::public_text_for_items(&snapshot.items),
        "已读到配置。\n\n检查完成。"
    );
    if let ThreadItem::TimelineEvent { data, .. } = &mut snapshot.items[1] {
        data["origin"] = json!("runtime_fallback");
    }
    assert_eq!(
        super::result_text_for_snapshot(&snapshot, "t").as_deref(),
        Some("已读到配置。检查完成。")
    );
}

#[test]
fn public_text_preserves_markdown_context_splits_and_tool_paragraphs() {
    let items: Vec<agent_core::ThreadItem> = serde_json::from_value(json!([
            {"kind":"agent_message", "id":"a", "content":"先检查。", "pending":false},
            {"kind":"tool_call", "id":"c", "name":"shell", "input":{}, "status":"done", "is_error":false},
            {"kind":"agent_message", "id":"b", "content":"结果**正", "pending":false},
            {"kind":"context", "id":"ctx", "data":{}},
            {"kind":"agent_message", "id":"d", "content":"常**。\n```ts\n", "pending":false},
            {"kind":"agent_message", "id":"space", "content":"    ", "pending":false},
            {"kind":"agent_message", "id":"e", "content":"x\n```", "pending":false}
        ])).unwrap();
    assert_eq!(
        super::public_text_for_items(&items),
        "先检查。\n\n结果**正常**。\n```ts\n    x\n```"
    );
}

#[test]
fn streaming_context_snapshots_are_not_persisted() {
    use super::should_persist_agent_payload;

    assert!(!should_persist_agent_payload(&json!({
        "type": "context",
        "streaming": true,
    })));
    assert!(should_persist_agent_payload(&json!({
        "type": "context",
    })));
    assert!(should_persist_agent_payload(&json!({
        "type": "context",
        "streaming": false,
    })));
    assert!(should_persist_agent_payload(&json!({
        "type": "context",
        "compaction_status": "completed",
        "context_compressed": true,
    })));
    assert!(should_persist_agent_payload(&json!({
        "type": "tool_call_started",
    })));
    assert!(!should_persist_agent_payload(&json!({
        "type": "text_delta",
    })));
}

#[async_trait::async_trait]
impl ToolHandler for CancelAwareTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "cancel_aware",
            "finish after a short delay",
            json!({ "type": "object" }),
        )
        .read_only()
    }

    async fn execute(&self, _input: serde_json::Value) -> Result<serde_json::Value, String> {
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        if self.cancellation.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("tool was unexpectedly cancelled".to_string());
        }
        self.stopped
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(json!({ "cancelled": true }))
    }
}

#[test]
fn recognizes_skill_catalog_questions_in_chinese_and_english() {
    assert!(is_skill_catalog_question("查一下当前有哪些 Skill"));
    assert!(is_skill_catalog_question(
        "How do I use the installed skills?"
    ));
    assert!(is_skill_catalog_question("有没有 selinux skill"));
    assert!(!is_skill_catalog_question("分析这个 SELinux AVC 问题"));
}

#[tokio::test]
async fn long_tool_sequence_keeps_progress_results_and_manual_cancellation() {
    use super::{begin_tool_call, complete_tool_call, tool_call_can_run, tool_call_signature};
    use std::collections::HashMap;

    let state = AgentRuntimeState::default();
    let guard = AgentRequestGuard::start(&state, "long-session".into(), "long-turn".into())
        .expect("request guard");
    let cancellation = guard.cancellation();
    let tool_cancellation = Arc::new(AtomicBool::new(false));
    let mut registry = ToolRegistry::new();
    registry
        .register_handler(CancelAwareTool {
            cancellation: tool_cancellation.clone(),
            stopped: Arc::new(AtomicBool::new(false)),
        })
        .expect("tool registration");
    let mut ledger = HashMap::new();

    // Exercise the execution path beyond both former limits without real
    // filesystem mutations, model charges, or weakening duplicate checks.
    for index in 0..80 {
        let call_id = format!("long-call-{index}");
        let input = json!({"step": index});
        let signature = tool_call_signature("cancel_aware", &input);
        assert!(tool_call_can_run(&ledger, &signature, true));
        begin_tool_call(&mut ledger, signature.clone());
        let outcome = execute_tool_call_interruptible(
            &registry,
            &AllowPermission,
            &state,
            "long-session",
            &call_id,
            "cancel_aware",
            input,
            "long-turn",
            &cancellation,
            &tool_cancellation,
            || {
                assert_eq!(
                    state.record_tool_call_started("long-session", "long-turn", &call_id),
                    Some(index + 1),
                );
            },
        )
        .await
        .expect("execution path");
        let InterruptibleToolExecution::Finished(Ok(result)) = outcome else {
            panic!("normal progress must remain executable");
        };
        assert_eq!(
            result.message.tool_call_id.as_deref(),
            Some(call_id.as_str())
        );
        assert!(result.permission_request.is_none());
        complete_tool_call(&mut ledger, &signature, &result.events);
        assert!(!tool_call_can_run(&ledger, &signature, true));
        assert!(
            state
                .record_tool_call_finished(
                    "long-session",
                    "long-turn",
                    &call_id,
                    "cancel_aware",
                    false,
                )
                .is_none()
        );
        assert!(!agent_core::is_cancelled(
            &state,
            "long-session",
            &cancellation
        ));
    }
    assert_eq!(state.tool_call_count("long-session", "long-turn"), 80);

    cancellation.store(true, Ordering::Relaxed);
    let outcome = execute_tool_call_interruptible(
        &registry,
        &AllowPermission,
        &state,
        "long-session",
        "cancelled-call",
        "cancel_aware",
        json!({"step": 80}),
        "long-turn",
        &cancellation,
        &tool_cancellation,
        || {},
    )
    .await
    .expect("manual cancellation");
    assert!(matches!(outcome, InterruptibleToolExecution::Cancelled));
    assert!(tool_cancellation.load(Ordering::Relaxed));
}

#[test]
fn parses_jira_host_and_issue_key() {
    let parsed = parse_jira_url_value("https://jira.bitech-auto.com/browse/BAIC-20392")
        .expect("valid Jira URL");
    assert_eq!(parsed.host, "jira.bitech-auto.com");
    assert_eq!(parsed.key, "BAIC-20392");
}

#[test]
fn rejects_invalid_scheme_and_issue_key() {
    assert!(parse_jira_url_value("file:///browse/BAIC-20392").is_err());
    assert!(parse_jira_url_value("https://jira.example.com/browse/baic-20392").is_err());
    assert!(parse_jira_url_value("https://jira.example.com/issues/BAIC-20392").is_err());
}

#[test]
fn converts_html_and_markdown_to_plain_text() {
    let text = jira_description_to_text(&json!(
        "<p>Crash <strong>after</strong> boot</p><ul><li>radio</li></ul>"
    ));
    assert_eq!(text, "Crash after boot\nradio");
}

#[test]
fn converts_adf_description_to_plain_text() {
    let text = jira_description_to_text(&json!({
        "type": "doc",
        "version": 1,
        "content": [
            { "type": "paragraph", "content": [{ "type": "text", "text": "First line" }] },
            { "type": "paragraph", "content": [{ "type": "text", "text": "Second line" }] }
        ]
    }));
    assert_eq!(text, "First line\nSecond line");
}

#[test]
fn parses_structured_analysis_report_from_fenced_json() {
    let report = parse_analysis_report(r#"```json
{"conclusion":"radio timeout","suspectedCauses":["service restart"],"evidence":["line 2"],"timeline":["10:00 error"],"nextSteps":["collect modem logs"]}
```"#)
        .expect("fenced report should parse");
    assert_eq!(report.conclusion, "radio timeout");
    assert_eq!(report.suspected_causes, vec!["service restart"]);
    assert_eq!(report.next_steps, vec!["collect modem logs"]);
}

#[test]
fn rejects_report_without_required_conclusion() {
    assert!(parse_analysis_report(r#"{"evidence":["line 2"]}"#).is_err());
}

#[test]
fn wsl_registry_excludes_desktop_only_tools() {
    let context = LocalToolContext::new(
        vec!["/tmp".to_string()],
        Vec::new(),
        false,
        Arc::new(AtomicBool::new(false)),
        None,
    )
    .with_wsl(
        WslRuntimeConfig {
            distribution: "Ubuntu-22.04".to_string(),
            root: "/tmp".to_string(),
        }
        .normalized()
        .expect("test WSL configuration should be valid"),
    );
    let registry = super::local_tool_registry(
        &["/tmp".to_string()],
        &[],
        Vec::new(),
        Arc::new(AtomicBool::new(false)),
        false,
        None,
        None,
        Some(&context),
        None,
        None,
        None,
    )
    .expect("WSL tool registry should be created");

    assert!(registry.get("list_local_files").is_some());
    assert!(registry.get("run_local_shell").is_some());
    assert!(registry.get("get_git_status").is_some());
    assert!(registry.get("inspect_winscope_trace").is_none());
    assert!(registry.get("search_local_logs").is_none());
    assert!(registry.get("read_local_log_range").is_none());
    assert!(registry.get("rebuild_timeline").is_none());
    assert!(registry.get("lsp_diagnostics").is_none());
}

#[test]
fn desktop_registry_drops_android_tools_but_keeps_bounded_log_tools() {
    let registry = super::local_tool_registry(
        &["C:/workspace".to_string()],
        &[],
        Vec::new(),
        Arc::new(AtomicBool::new(false)),
        false,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .expect("desktop tool registry should be created");

    assert!(registry.get("search_local_logs").is_some());
    assert!(registry.get("read_local_log_range").is_some());
    assert!(registry.get("rebuild_timeline").is_some());
    assert!(registry.get("open_application_tool").is_none());
    for removed in [
        "query_android_logcat",
        "capture_android_logcat",
        "query_android_tasks",
        "query_android_archive_history",
        "start_android_archive_task",
        "inspect_winscope_trace",
        "query_android_perfetto",
        "query_android_diagnostics",
        "query_android_bugreport",
        "query_android_aosp_build",
        "query_android_simpleperf",
        "query_android_native_symbolizer",
        "query_android_method_profile",
        "query_android_heap_dump",
        "query_android_benchmark",
        "query_android_device",
    ] {
        assert!(registry.get(removed).is_none(), "{removed} should be removed");
    }
}

#[test]
fn environment_registry_keeps_windows_and_wsl_tools_available_together() {
    let context = LocalToolContext::new(
        vec!["/workspace".to_string()],
        Vec::new(),
        false,
        Arc::new(AtomicBool::new(false)),
        None,
    )
    .with_wsl(
        WslRuntimeConfig {
            distribution: "Ubuntu-22.04".to_string(),
            root: "/workspace".to_string(),
        }
        .normalized()
        .expect("valid WSL configuration"),
    );
    let registry = environment_tool_registry(
        AgentToolEnvironment::Wsl,
        &["C:/workspace".to_string()],
        &[],
        &[],
        &["/workspace".to_string()],
        Vec::new(),
        Arc::new(AtomicBool::new(false)),
        false,
        None,
        None,
        Some(&context),
        &[],
        None,
        None,
        None,
    )
    .expect("combined registry");

    assert!(registry.get("list_local_files").is_some());
    assert!(registry.get("wsl__list_local_files").is_some());
    assert!(registry.get("windows__list_local_files").is_some());
    assert!(registry.get("windows__inspect_winscope_trace").is_none());
    assert!(registry.get("wsl__inspect_winscope_trace").is_none());

    let windows = ["C:/workspace".to_string()];
    let wsl = ["/workspace".to_string()];
    let default = default_tool_environment(false, true);
    let unprefixed = tool_execution_scope("read_local_file", default, &windows, &[], &wsl);
    let explicit = tool_execution_scope("windows__read_local_file", default, &windows, &[], &wsl);
    assert_eq!(unprefixed.environment, AgentToolEnvironment::Wsl);
    assert_eq!(explicit.environment, AgentToolEnvironment::Windows);
    assert_eq!(explicit.paths, windows);
}

#[tokio::test]
async fn environment_registry_keeps_windows_and_ssh_tools_available_together() {
    let cancellation = Arc::new(AtomicBool::new(false));
    let context = agent_core::SshToolContext::new(
        agent_core::SshConnectionConfig {
            id: "ssh-test".to_string(),
            name: "SSH test".to_string(),
            host: "127.0.0.1".to_string(),
            port: 22,
            username: "tester".to_string(),
            private_key_path: None,
            known_hosts_path: None,
            strict_host_key: false,
            default_root: Some("/workspace".to_string()),
            reuse_connection: false,
            connect_timeout_secs: 1,
            operation_timeout_secs: 1,
            keepalive_interval_secs: 0,
        },
        "/workspace",
        None,
        cancellation.clone(),
        None,
    );
    for default_environment in [AgentToolEnvironment::Windows, AgentToolEnvironment::Ssh] {
        let local_root = env!("CARGO_MANIFEST_DIR").to_string();
        let disconnected = environment_tool_registry(
            AgentToolEnvironment::Windows,
            &[local_root.clone()],
            &[],
            &[],
            &[],
            Vec::new(),
            cancellation.clone(),
            false,
            None,
            None,
            None,
            &[],
            None,
            None,
            None,
        )
        .expect("registry before connecting");
        assert!(disconnected.get("ssh__run_local_command").is_none());
        assert!(disconnected.get("windows__run_local_command").is_some());
        let registry = environment_tool_registry(
            default_environment,
            &[local_root.clone()],
            &[],
            &["/workspace".to_string()],
            &[],
            Vec::new(),
            cancellation.clone(),
            false,
            None,
            Some(&context),
            None,
            &[],
            None,
            None,
            None,
        )
        .expect("combined registry");

        assert!(registry.get("run_local_command").is_some());
        assert!(registry.get("ssh__run_local_command").is_some());
        assert!(registry.get("windows__run_local_command").is_some());
        assert!(registry.get("windows__query_android_device").is_none());
        assert!(registry.get("ssh__query_android_device").is_none());
        // Even with SSH as default, the local alias executes against the selected
        // project. The test SSH context points to an unavailable host deliberately.
        let directory = registry
            .execute(
                "windows__get_working_directory",
                json!({}),
                &AllowPermission,
            )
            .await
            .unwrap();
        assert_eq!(
            std::path::PathBuf::from(directory["path"].as_str().unwrap()),
            std::path::PathBuf::from(&local_root)
        );
        let file = registry
            .execute(
                "windows__read_local_file",
                json!({"path":"Cargo.toml"}),
                &AllowPermission,
            )
            .await
            .unwrap();
        assert!(file["content"]
            .as_str()
            .unwrap()
            .contains("terminal-desktop"));
        assert!(registry
            .execute(
                "windows__read_local_file",
                json!({"path":"/remote-only/file"}),
                &AllowPermission
            )
            .await
            .is_err());
    }

    let windows = ["C:/workspace".to_string()];
    let ssh = ["/workspace".to_string()];
    let default = default_tool_environment(true, false);
    let unprefixed = tool_execution_scope("run_local_command", default, &windows, &ssh, &[]);
    let explicit = tool_execution_scope("windows__run_local_command", default, &windows, &ssh, &[]);
    assert_eq!(unprefixed.environment, AgentToolEnvironment::Ssh);
    assert_eq!(explicit.environment, AgentToolEnvironment::Windows);
    assert_eq!(explicit.paths, windows);
}

#[tokio::test]
async fn bound_ssh_hosts_become_target_routable_ssh_tools() {
    let cancellation = Arc::new(AtomicBool::new(false));
    let context_for = |id: &str, name: &str, root: &str| {
        agent_core::SshToolContext::new(
            agent_core::SshConnectionConfig {
                id: id.to_string(),
                name: name.to_string(),
                host: "127.0.0.1".to_string(),
                port: 22,
                username: "tester".to_string(),
                private_key_path: None,
                known_hosts_path: None,
                strict_host_key: false,
                default_root: Some(root.to_string()),
                reuse_connection: false,
                connect_timeout_secs: 1,
                operation_timeout_secs: 1,
                keepalive_interval_secs: 0,
            },
            root,
            None,
            cancellation.clone(),
            None,
        )
    };
    let bound = vec![
        BoundSshHost {
            connection_id: "conn-a".to_string(),
            name: "构建机".to_string(),
            root: "/srv/build".to_string(),
            context: Some(context_for("conn-a", "构建机", "/srv/build")),
            note: None,
        },
        BoundSshHost {
            connection_id: "conn-b".to_string(),
            name: "日志机".to_string(),
            root: "/var/log".to_string(),
            context: None,
            note: Some("连接超时".to_string()),
        },
    ];
    let local_root = env!("CARGO_MANIFEST_DIR").to_string();
    let registry = environment_tool_registry(
        AgentToolEnvironment::Windows,
        &[local_root],
        &[],
        &[],
        &[],
        Vec::new(),
        cancellation.clone(),
        false,
        None,
        None,
        None,
        &bound,
        None,
        None,
        None,
    )
    .expect("registry with a bound host");

    // 只有已连通的主机会进入 target 枚举；未连通的不会假装可用。
    let definition = registry
        .get("ssh__run_local_command")
        .expect("ssh alias routed through the bound host");
    let targets = definition.input_schema["properties"]["target"]["enum"]
        .as_array()
        .expect("routed alias exposes a target enum")
        .iter()
        .filter_map(|value| value.as_str())
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert!(targets.contains(&"构建机".to_string()));
    assert!(targets.contains(&"conn-a".to_string()));
    assert!(!targets.contains(&"日志机".to_string()));
    // 注入 target 不能改变原有 schema 的其余约束（这里只断言原有字段仍在）。
    assert!(definition.input_schema["properties"]["command"].is_object());
    assert_eq!(definition.input_schema["required"], json!(["command"]));

    // 没有绑定主机时保持普通别名，不带 target 参数。
    let plain = environment_tool_registry(
        AgentToolEnvironment::Windows,
        &[env!("CARGO_MANIFEST_DIR").to_string()],
        &[],
        &[],
        &[],
        Vec::new(),
        cancellation,
        false,
        None,
        None,
        None,
        &[],
        None,
        None,
        None,
    )
    .expect("registry without bound hosts");
    assert!(plain.get("ssh__run_local_command").is_none());
}

#[tokio::test]
async fn ordinary_steering_waits_for_the_started_tool_without_cancelling_it() {
    let state = AgentRuntimeState::default();
    let mut guard = AgentRequestGuard::start(
        &state,
        "session-cancel".to_string(),
        "turn-cancel".to_string(),
    )
    .expect("request guard");
    let sender = state
        .requests
        .lock()
        .expect("request state")
        .get("session-cancel")
        .expect("active request")
        .steer_sender
        .clone();
    let mut receiver = guard.take_steer_receiver();
    let tool_cancellation = Arc::new(AtomicBool::new(false));
    let stopped = Arc::new(AtomicBool::new(false));
    let mut registry = ToolRegistry::new();
    registry
        .register_handler(CancelAwareTool {
            cancellation: tool_cancellation.clone(),
            stopped: stopped.clone(),
        })
        .expect("tool registration");
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        sender
            .send(agent_core::AgentSteerRequest {
                message: "change direction".to_string(),
                attachments: Vec::new(),
                artifacts: Vec::new(),
                source_message_id: None,
            })
            .await
            .expect("steer send");
    });

    let outcome = execute_tool_call_interruptible(
        &registry,
        &AllowPermission,
        &state,
        "session-cancel",
        "call-cancel",
        "cancel_aware",
        json!({}),
        "cancel-turn",
        &guard.cancellation(),
        &tool_cancellation,
        || {},
    )
    .await
    .expect("interruptible execution");

    assert!(matches!(
        outcome,
        InterruptibleToolExecution::Finished(Ok(_))
    ));
    assert!(!tool_cancellation.load(std::sync::atomic::Ordering::Relaxed));
    assert!(stopped.load(std::sync::atomic::Ordering::Relaxed));
    let steer = receiver.try_recv().expect("steer remains queued");
    assert_eq!(steer.message, "change direction");
}

#[tokio::test]
async fn tool_execution_lanes_keep_windows_responsive_and_bound_parallelism() {
    let lanes = ToolExecutionLanes::new(2);
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();

    let remote_lanes = lanes.clone();
    let remote_active = active.clone();
    let remote_maximum = maximum.clone();
    let remote_sender = sender.clone();
    let first_remote = tokio::spawn(async move {
        let _permit = remote_lanes.acquire(true).await;
        let current = remote_active.fetch_add(1, Ordering::SeqCst) + 1;
        remote_maximum.fetch_max(current, Ordering::SeqCst);
        remote_sender.send("remote-started").expect("remote start");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        remote_active.fetch_sub(1, Ordering::SeqCst);
        remote_sender
            .send("remote-finished")
            .expect("remote finish");
    });
    assert_eq!(receiver.recv().await, Some("remote-started"));

    let queued_remote_lanes = lanes.clone();
    let queued_remote_sender = sender.clone();
    let second_remote = tokio::spawn(async move {
        let _permit = queued_remote_lanes.acquire(true).await;
        queued_remote_sender
            .send("second-remote-started")
            .expect("second remote start");
    });
    let windows_lanes = lanes.clone();
    let windows_active = active.clone();
    let windows_maximum = maximum.clone();
    let windows_sender = sender.clone();
    let windows = tokio::spawn(async move {
        let _permit = windows_lanes.acquire(false).await;
        let current = windows_active.fetch_add(1, Ordering::SeqCst) + 1;
        windows_maximum.fetch_max(current, Ordering::SeqCst);
        windows_sender
            .send("windows-finished")
            .expect("windows finish");
        windows_active.fetch_sub(1, Ordering::SeqCst);
    });

    let first_completion =
        tokio::time::timeout(std::time::Duration::from_millis(60), receiver.recv())
            .await
            .expect("Windows should not wait for the SSH lane");
    assert_eq!(first_completion, Some("windows-finished"));
    assert_eq!(maximum.load(Ordering::SeqCst), 2);
    assert_eq!(receiver.recv().await, Some("remote-finished"));
    assert_eq!(receiver.recv().await, Some("second-remote-started"));

    first_remote.await.expect("first remote task");
    second_remote.await.expect("second remote task");
    windows.await.expect("Windows task");
}

#[tokio::test]
async fn indexed_parallel_results_keep_model_order() {
    async fn result_after(
        index: usize,
        value: &'static str,
        delay_ms: u64,
    ) -> (usize, &'static str) {
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        (index, value)
    }
    let mut pending = futures_util::stream::FuturesUnordered::new();
    pending.push(result_after(0, "first", 30));
    pending.push(result_after(1, "second", 0));
    let mut ordered = vec![None; 2];
    while let Some((index, value)) = pending.next().await {
        ordered[index] = Some(value);
    }
    assert_eq!(
        ordered.into_iter().flatten().collect::<Vec<_>>(),
        vec!["first", "second"]
    );
}

#[test]
fn deadline_owner_check_rejects_retired_and_replaced_turns() {
    let state = AgentRuntimeState::default();
    let first = AgentRequestGuard::start(
        &state,
        "deadline-session".to_string(),
        "turn-one".to_string(),
    )
    .expect("first request guard");
    let first_cancellation = first.cancellation();
    assert!(active_turn_owned_by(
        &state,
        "deadline-session",
        "turn-one",
        &first_cancellation,
    ));

    drop(first);
    assert!(!active_turn_owned_by(
        &state,
        "deadline-session",
        "turn-one",
        &first_cancellation,
    ));

    let second = AgentRequestGuard::start(
        &state,
        "deadline-session".to_string(),
        "turn-two".to_string(),
    )
    .expect("replacement request guard");
    let second_cancellation = second.cancellation();
    assert!(!active_turn_owned_by(
        &state,
        "deadline-session",
        "turn-one",
        &first_cancellation,
    ));
    assert!(active_turn_owned_by(
        &state,
        "deadline-session",
        "turn-two",
        &second_cancellation,
    ));
}

#[test]
fn persisted_steer_delivery_requires_the_same_active_turn() {
    let state = AgentRuntimeState::default();
    let guard = AgentRequestGuard::start(
        &state,
        "steer-session".to_string(),
        "current-turn".to_string(),
    )
    .expect("request guard");

    assert!(active_steer_sender(&state, "steer-session", "current-turn").is_some());
    assert!(active_steer_sender(&state, "steer-session", "old-turn").is_none());
    guard.cancellation().store(true, Ordering::Relaxed);
    assert!(active_steer_sender(&state, "steer-session", "current-turn").is_none());
}

#[test]
fn prefixes_mcp_tool_names_with_server_id() {
    assert_eq!(
        mcp_tool_name("local-tools", "read_file"),
        "mcp__local-tools__read_file"
    );
}

#[test]
fn normalizes_provider_http_errors_to_stable_codes() {
    assert_eq!(provider_error_code(401), "unauthorized");
    assert_eq!(provider_error_code(403), "forbidden");
    assert_eq!(provider_error_code(404), "not_found");
    assert_eq!(provider_error_code(429), "rate_limited");
    assert_eq!(provider_error_code(503), "server_failure");
}

#[test]
fn provider_connection_check_verifies_selected_model_when_list_is_available() {
    let models = json!({
        "data": [
            { "id": "gpt-4o-mini" },
            { "id": "DeepSeek-Chat" }
        ]
    });

    assert_eq!(
        provider_model_is_listed(&models, "deepseek-chat"),
        Some(true)
    );
    assert_eq!(
        provider_model_is_listed(&models, "missing-model"),
        Some(false)
    );
    assert_eq!(
        provider_model_is_listed(&json!({ "status": "ok" }), "gpt-4o"),
        None
    );
}

#[test]
fn parses_new_api_quota_as_remaining_quota() {
    let quota = parse_relay_quota(
        &json!({
            "success": true,
            "data": {
                "quota": 100_000_000,
                "used_quota": 99_000_000
            }
        }),
        true,
    )
    .expect("New API quota should be reported");

    assert_eq!(quota.remaining_amount, Some(200.0));
    assert_eq!(quota.remaining_tokens, None);
    assert_eq!(quota.used_amount, Some(198.0));
}

#[test]
fn parses_new_api_token_usage_and_unlimited_tokens() {
    let body = json!({"code":true,"data":{"object":"token_usage",
            "total_granted":50_000_000,"total_used":12_500_000,"total_available":37_500_000,
            "unlimited_quota":false}});
    for is_new_api in [true, false] {
        let quota = parse_relay_quota(&body, is_new_api).unwrap();
        assert_eq!(quota.remaining_amount, Some(75.0));
        assert_eq!(quota.used_amount, Some(25.0));
        assert_eq!(quota.total_amount, Some(100.0));
    }
    let mut unlimited = body.clone();
    unlimited["data"]["unlimited_quota"] = json!(true);
    let quota = parse_relay_quota(&unlimited, true).unwrap();
    assert_eq!(quota.remaining_amount, None);
    assert_eq!(quota.total_amount, None);
    assert_eq!(quota.used_amount, Some(25.0));
    let mut failed = body;
    failed["success"] = json!(false);
    assert!(parse_relay_quota(&failed, true).is_none());
}

#[test]
fn custom_quota_never_leaks_model_key_across_origins() {
    let base = url::Url::parse("https://api.example/v1").unwrap();
    let same = url::Url::parse("https://api.example/balance").unwrap();
    let other = url::Url::parse("https://billing.example/usage").unwrap();
    assert_eq!(
        super::custom_quota_token(&base, &same, "model-key", None).unwrap(),
        "model-key"
    );
    assert!(super::custom_quota_token(&base, &other, "model-key", None).is_err());
    assert_eq!(
        super::custom_quota_token(&base, &other, "model-key", Some("billing-token")).unwrap(),
        "billing-token"
    );
}

#[test]
fn custom_quota_reads_spending_without_inventing_a_balance() {
    let values = parse_relay_quota(
        &json!({"data":{"consumption":"12.5","currency":"CNY"}}),
        false,
    )
    .unwrap();
    assert_eq!(values.used_amount, Some(12.5));
    assert_eq!(values.remaining_amount, None);
    let values = parse_relay_quota(
        &json!({"balance_infos":[{"total_balance":"25.6","currency":"CNY"}]}),
        false,
    )
    .unwrap();
    assert_eq!(values.remaining_amount, Some(25.6));
    assert_eq!(values.total_amount, None);
    assert_eq!(values.used_amount, None);
}

#[test]
fn custom_quota_reads_jieyou_balance_without_daily_spend_as_total() {
    let values = parse_relay_quota(
        &json!({"isValid":true,"remaining":830.25659419,"balance":830.25659419,
                "unit":"USD","daily_usage":0.07,"model_stats":[]}),
        false,
    )
    .unwrap();
    assert_eq!(values.remaining_amount, Some(830.25659419));
    assert_eq!(values.currency.as_deref(), Some("USD"));
    assert_eq!(values.used_amount, None);
}

#[test]
fn custom_quota_reads_litellm_budget_and_spend() {
    let values =
        parse_relay_quota(&json!({"info":{"max_budget":100.0,"spend":12.5}}), false).unwrap();
    assert_eq!(values.remaining_amount, Some(87.5));
    assert_eq!(values.used_amount, Some(12.5));
    assert_eq!(values.currency.as_deref(), Some("USD"));
    let unlimited =
        parse_relay_quota(&json!({"info":{"max_budget":null,"spend":0.0}}), false).unwrap();
    assert_eq!(unlimited.remaining_amount, None);
    assert_eq!(unlimited.used_amount, Some(0.0));
    let missing_spend = parse_relay_quota(&json!({"info":{"max_budget":100}}), false).unwrap();
    assert_eq!(missing_spend.remaining_amount, None);
    assert_eq!(missing_spend.used_amount, None);
}

#[tokio::test]
async fn custom_quota_http_get_reads_json_and_bearer_token() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/usage", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        let mut request = [0; 8192];
        let count = stream.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..count]).to_ascii_lowercase();
        assert!(request.starts_with("get /usage http/1.1"));
        assert!(request.contains("authorization: bearer query-token"));
        let body = r#"{"data":{"used_amount":7.25,"currency":"USD"}}"#;
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    let request: super::AgentProviderCheckRequest = serde_json::from_value(json!({
        "id":"test", "name":"test", "kind":"open_ai_compatible", "model":"test",
        "baseUrl": endpoint, "apiKey":null
    }))
    .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap();
    let (_, values) = super::query_custom_quota_endpoint(
        &client,
        &request,
        &request.base_url,
        "model-key",
        Some("query-token"),
        "消费",
    )
    .await;
    assert_eq!(values.unwrap().used_amount, Some(7.25));
    server.join().unwrap();
}

#[test]
fn does_not_invent_quota_from_used_or_total_without_remaining_value() {
    assert!(parse_relay_quota(&json!({"data": {"used_quota": 10_000}}), true,).is_none());
    assert!(parse_relay_quota(&json!({"data": {"total_quota": 10_000}}), true,).is_none());
}

#[test]
fn parser_does_not_use_quota_from_an_explicitly_failed_response() {
    let body = json!({
        "success": false,
        "data": { "quota": 100_000_000 }
    });

    assert!(relay_response_declares_failure(&body));
    assert!(parse_relay_quota(&body, true).is_none());
}

#[test]
fn parses_nested_generic_relay_balance_and_string_numbers() {
    let quota = parse_relay_quota(
        &json!({
            "ok": true,
            "data": {
                "balance": {
                    "remaining": "12.5",
                    "currency": "USD"
                }
            }
        }),
        false,
    )
    .expect("nested balance should be reported");

    assert_eq!(quota.remaining_amount, Some(12.5));
    assert_eq!(quota.currency.as_deref(), Some("USD"));
}

#[test]
fn recognizes_explicit_relay_auth_failures_but_not_generic_errors() {
    assert!(relay_auth_failure_message(&json!({
        "success": false,
        "code": 401,
        "message": "invalid token"
    }))
    .is_some());
    assert!(relay_auth_failure_message(&json!({
        "success": false,
        "message": "quota endpoint is not enabled"
    }))
    .is_none());
}

#[test]
fn builds_explicit_relay_endpoints_without_duplicate_paths() {
    let endpoints = relay_quota_endpoints("https://relay.example/v1");
    let urls = endpoints
        .iter()
        .map(|(url, _)| url.clone())
        .collect::<std::collections::HashSet<_>>();

    assert_eq!(urls.len(), endpoints.len());
    assert!(urls.contains("https://relay.example/api/user/self"));
    assert!(urls.contains("https://relay.example/api/usage/token/"));
    assert!(urls.contains("https://relay.example/api/usage"));
    assert!(!urls.contains("https://relay.example/v1/v1/usage"));
}

#[test]
fn skips_quota_probes_for_official_provider_hosts() {
    assert!(is_known_official_provider_base_url(
        "https://api.groq.com/openai/v1"
    ));
    assert!(is_known_official_provider_base_url(
        "https://api.openai.com/v1"
    ));
    assert!(!is_known_official_provider_base_url(
        "https://relay.example/v1"
    ));
}

#[test]
fn detects_output_limit_for_reported_and_local_usage() {
    assert!(output_limit_reached(Some(2_047), 2_048, "short"));
    assert!(!output_limit_reached(Some(1_000), 2_048, "short"));
    assert!(output_limit_reached(None, 2_048, &"中".repeat(2_000)));
}

#[test]
fn local_agent_tools_stay_inside_selected_roots() {
    let roots = vec![r"C:\workspace\logs".to_string()];
    assert!(is_selected_path(
        std::path::Path::new(r"C:\workspace\logs\app.log"),
        &roots
    ));
    assert!(!is_selected_path(
        std::path::Path::new(r"C:\workspace\other\secret.txt"),
        &roots
    ));
}

#[test]
fn command_tool_rejects_shell_interpreters() {
    assert!(is_allowed_command("git"));
    assert!(is_allowed_command("C:\\Program Files\\nodejs\\npm.cmd"));
    assert!(!is_allowed_command("powershell"));
    assert!(!is_allowed_command("cmd.exe"));
}

#[test]
fn expands_all_models_saved_under_one_provider() {
    let provider = AgentProviderSettings {
        prompt_cache_retention: None,
        id: "gateway".to_string(),
        name: "Team Gateway".to_string(),
        kind: ProviderKind::OpenAiCompatible,
        brand: None,
        base_url: "https://gateway.example/v1".to_string(),
        model: "gpt-4o-mini".to_string(),
        context_window: Some(128_000),
        input_price_per_million: None,
        output_price_per_million: None,
        currency: None,
        usage_script: None,
        usage_user_id: None,
        quota_urls: None,
        models: vec![
            AgentModelSettings {
                reasoning_efforts: None,
                icon_brand: None,
                billing: None,
                id: "gpt-4o-mini".to_string(),
                name: "GPT-4o mini".to_string(),
                protocol: None,
                protocols: Vec::new(),
                context_window: Some(128_000),
                max_output_tokens: None,
                supports_tools: None,
                supports_vision: None,
                supports_usage: None,
                supports_reasoning: None,
            },
            AgentModelSettings {
                reasoning_efforts: None,
                icon_brand: None,
                billing: None,
                id: "deepseek-chat".to_string(),
                name: "DeepSeek Chat".to_string(),
                protocol: None,
                protocols: Vec::new(),
                context_window: Some(64_000),
                max_output_tokens: None,
                supports_tools: None,
                supports_vision: None,
                supports_usage: None,
                supports_reasoning: None,
            },
        ],
    };

    let models = expand_agent_models(&[provider]);

    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "gateway:gpt-4o-mini");
    assert_eq!(models[1].id, "gateway:deepseek-chat");
    assert_eq!(models[1].context_window, Some(64_000));
}

#[test]
fn expands_legacy_single_model_provider_without_models_field() {
    let provider: AgentProviderSettings = serde_json::from_value(json!({
        "id": "legacy",
        "name": "Legacy Provider",
        "kind": "open_ai_compatible",
        "baseUrl": "https://legacy.example/v1",
        "model": "legacy-model",
        "contextWindow": 32000
    }))
    .expect("legacy provider should deserialize without models");

    let models = expand_agent_models(&[provider]);

    assert_eq!(models.len(), 1);
    assert_eq!(models[0].model, "legacy-model");
    assert_eq!(models[0].context_window, Some(32_000));
}

#[test]
fn resolves_the_selected_model_from_a_provider_model_id() {
    let provider = AgentProviderSettings {
        prompt_cache_retention: None,
        id: "gateway".to_string(),
        name: "Team Gateway".to_string(),
        kind: ProviderKind::OpenAiCompatible,
        brand: None,
        base_url: "https://gateway.example/v1".to_string(),
        model: "gpt-4o-mini".to_string(),
        context_window: Some(128_000),
        input_price_per_million: Some(1.0),
        output_price_per_million: Some(2.0),
        currency: Some("人民币".to_string()),
        usage_script: None,
        usage_user_id: None,
        quota_urls: None,
        models: vec![
            AgentModelSettings {
                reasoning_efforts: None,
                icon_brand: None,
                billing: None,
                id: "gpt-4o-mini".to_string(),
                name: "GPT-4o mini".to_string(),
                protocol: None,
                protocols: Vec::new(),
                context_window: Some(128_000),
                max_output_tokens: None,
                supports_tools: None,
                supports_vision: None,
                supports_usage: None,
                supports_reasoning: None,
            },
            AgentModelSettings {
                reasoning_efforts: None,
                icon_brand: None,
                billing: None,
                id: "deepseek-chat".to_string(),
                name: "DeepSeek Chat".to_string(),
                protocol: None,
                protocols: Vec::new(),
                context_window: Some(64_000),
                max_output_tokens: None,
                supports_tools: None,
                supports_vision: None,
                supports_usage: None,
                supports_reasoning: None,
            },
        ],
    };

    let providers = [provider];
    let (resolved_provider, resolved_model) =
        resolve_agent_model(&providers, "gateway:deepseek-chat").expect("model should resolve");

    assert_eq!(resolved_provider.id, "gateway");
    assert_eq!(resolved_model.id, "deepseek-chat");
    assert_eq!(resolved_model.context_window, Some(64_000));
}

#[test]
fn summarizes_one_provider_with_all_saved_models() {
    let provider = AgentProviderSettings {
        prompt_cache_retention: None,
        id: "gateway".to_string(),
        name: "Team Gateway".to_string(),
        kind: ProviderKind::OpenAiCompatible,
        brand: None,
        base_url: "https://gateway.example/v1".to_string(),
        model: "gpt-4o-mini".to_string(),
        context_window: Some(128_000),
        input_price_per_million: None,
        output_price_per_million: None,
        currency: None,
        usage_script: None,
        usage_user_id: None,
        quota_urls: None,
        models: vec![
            AgentModelSettings {
                reasoning_efforts: None,
                icon_brand: None,
                billing: None,
                id: "gpt-4o-mini".to_string(),
                name: "GPT-4o mini".to_string(),
                protocol: None,
                protocols: Vec::new(),
                context_window: Some(128_000),
                max_output_tokens: None,
                supports_tools: None,
                supports_vision: None,
                supports_usage: None,
                supports_reasoning: None,
            },
            AgentModelSettings {
                reasoning_efforts: None,
                icon_brand: None,
                billing: None,
                id: "deepseek-chat".to_string(),
                name: "DeepSeek Chat".to_string(),
                protocol: None,
                protocols: Vec::new(),
                context_window: Some(64_000),
                max_output_tokens: None,
                supports_tools: None,
                supports_vision: None,
                supports_usage: None,
                supports_reasoning: None,
            },
        ],
    };

    let providers = expand_agent_providers(&[provider]);

    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].id, "gateway");
    assert_eq!(providers[0].models.len(), 2);
}

#[test]
fn permission_mode_only_prompts_when_not_in_full_access() {
    assert!(!permission_requires_confirmation("full", true));
    assert!(permission_requires_confirmation("ask", true));
    assert!(permission_requires_confirmation("risk", true));
    assert!(!permission_requires_confirmation("ask", false));
}

#[test]
fn path_access_requires_confirmation_for_workspace_and_external_paths() {
    let roots = vec![r"C:\workspace\project".to_string()];
    let workspace_path = json!({ "path": r"C:\workspace\project" });
    let external_path = json!({ "path": r"C:\Users\tester\Desktop\notes.txt" });
    let external_command = json!({ "command": r"C:\Tools\node.exe" });

    assert!(path_access_requires_confirmation(
        "ask",
        "list_local_files",
        &workspace_path,
        &roots
    ));
    assert!(!path_access_requires_confirmation(
        "risk",
        "list_local_files",
        &workspace_path,
        &roots
    ));
    assert!(path_access_requires_confirmation(
        "ask",
        "read_local_file",
        &external_path,
        &[]
    ));
    assert!(path_access_requires_confirmation(
        "ask",
        "run_local_command",
        &external_command,
        &[]
    ));
    assert!(!path_access_requires_confirmation(
        "full",
        "read_local_file",
        &external_path,
        &[]
    ));
}

#[test]
fn path_access_requires_confirmation_for_paths_in_command_arguments() {
    let input = json!({
        "command": "npm",
        "args": ["--prefix", r"C:\outside\project"],
    });

    assert!(path_access_requires_confirmation(
        "ask",
        "run_local_command",
        &input,
        &[]
    ));
}

#[test]
fn path_access_checks_declared_source_and_destination_paths() {
    let input = json!({
        "source": r"C:\outside\input.txt",
        "destination": r"C:\workspace\project\output.txt",
    });

    assert!(path_access_requires_confirmation(
        "ask",
        "write_local_file",
        &input,
        &[r"C:\workspace\project".to_string()]
    ));
}

/// A steer that re-enables tools must not mutate the system prompt: that string
/// is the leading block of the provider prefix, and the provider caches by
/// content hash, so mutating it costs a full cold re-read of the whole prefix
/// for the rest of the turn. The notice rides the new user message instead.
#[test]
fn steer_tool_policy_notice_extends_user_message_and_leaves_system_prompt_alone() {
    let steer = AgentSteerRequest {
        message: "继续".to_string(),
        attachments: Vec::new(),
        artifacts: vec![LocalArtifact {
            path: "/tmp/notes.txt".to_string(),
            name: "notes.txt".to_string(),
            kind: "file".to_string(),
            size: Some(12),
            modified_at: None,
            status: "workspace_context".to_string(),
            content: Some("hello".to_string()),
            content_truncated: false,
        }],
        source_message_id: None,
    };
    let mut messages: Vec<AgentMessage> = Vec::new();
    let mut windows_paths: Vec<String> = Vec::new();
    let mut tool_policy = AgentToolPolicy::Disabled;
    let mut image_attachments: Vec<ProviderImageAttachment> = Vec::new();
    let image_token_reserve = AtomicUsize::new(0);

    apply_steer_runtime_inputs(
        Path::new(""),
        &steer,
        &mut messages,
        &mut windows_paths,
        &mut tool_policy,
        true,
        false,
        None,
        &mut image_attachments,
        &image_token_reserve,
    )
    .expect("steer should apply");

    assert_eq!(tool_policy, AgentToolPolicy::Auto);
    assert_eq!(messages.len(), 1);
    let content = &messages[0].content;
    assert!(
        content.starts_with("继续"),
        "the user's own text must still lead the message"
    );
    assert!(
        content.contains("本轮工具策略已更新"),
        "the tool-policy notice must travel with the transcript"
    );
}

/// The counterpart: when tools were already enabled the steer must not inject the
/// tool-policy notice. (Artifact labels are appended independently of the policy.)
#[test]
fn steer_omits_tool_policy_notice_when_tools_already_enabled() {
    let steer = AgentSteerRequest {
        message: "继续".to_string(),
        attachments: Vec::new(),
        artifacts: vec![LocalArtifact {
            path: "/tmp/notes.txt".to_string(),
            name: "notes.txt".to_string(),
            kind: "file".to_string(),
            size: Some(12),
            modified_at: None,
            status: "workspace_context".to_string(),
            content: Some("hello".to_string()),
            content_truncated: false,
        }],
        source_message_id: None,
    };
    let mut messages: Vec<AgentMessage> = Vec::new();
    let mut windows_paths: Vec<String> = Vec::new();
    let mut tool_policy = AgentToolPolicy::Auto;
    let mut image_attachments: Vec<ProviderImageAttachment> = Vec::new();
    let image_token_reserve = AtomicUsize::new(0);

    apply_steer_runtime_inputs(
        Path::new(""),
        &steer,
        &mut messages,
        &mut windows_paths,
        &mut tool_policy,
        true,
        false,
        None,
        &mut image_attachments,
        &image_token_reserve,
    )
    .expect("steer should apply");

    assert_eq!(messages.len(), 1);
    assert_eq!(tool_policy, AgentToolPolicy::Auto);
    assert!(
        !messages[0].content.contains("本轮工具策略已更新"),
        "no tool-policy notice is needed when tools were already enabled"
    );
}

/// A source-level guard against reintroducing a write to the cached prefix.
///
/// `agent.rs` is the module that owns the provider prefix, so scanning its own
/// source is the cheapest way to keep a future refactor from quietly putting a
/// per-round notice back into `system` or `tools`.
#[test]
fn final_phase_notice_rides_the_transcript_not_the_prefix() {
    let source = turn_source_text();
    let source = source.as_str();
    for pattern in ["base_system_prompt.push_str", "base_system_prompt.push("] {
        assert!(
                !source.contains(pattern),
                "`{pattern}` mutates the cached prefix; carry per-round instructions on a message instead"
            );
    }
    assert!(
            !source.contains("final_response_requested || tool_policy == AgentToolPolicy::Disabled"),
            "emptying `tools` changes the earliest hashed prefix block; keep the declaration list stable"
        );
    assert!(
        source.contains("AgentMessage::system(FINAL_PHASE_NOTICE)"),
        "the final-phase notice must be pushed onto the transcript"
    );
    assert!(
        !source.contains("\"本轮是最终整理阶段"),
        "the final-phase notice must not be concatenated into the system prompt"
    );
}

/// The window guard must not trust the estimator alone.
///
/// Incident this encodes: a 1,000,000-token window, an estimate of 940_000 that passed
/// the budget check, and a provider bill of 1_018_592 input tokens for the very request
/// that estimate had approved. The over-window request was sent three times, came back
/// with a single token each round, and the turn ended with `[model_response_missing]`.
#[test]
fn reported_usage_over_the_window_forces_one_hard_compaction() {
    let window = 1_000_000;
    let reserve = 8_192;
    let guard = super::strategy::context_window_budget_exceeded;

    // Round one has no measurement yet: the estimate alone still approves the request.
    assert!(!guard(940_000, None, false, reserve, window));
    // The provider then reports the real charge for that request. The estimator had been
    // low by roughly 8%, so the measured value is the only evidence the window broke.
    assert!(guard(940_000, Some(1_018_592), false, reserve, window));
    // The single hard compaction spends the allowance and drops the stale figure: the
    // report described the payload that was just replaced, so it cannot fire again.
    assert!(!guard(940_000, None, true, reserve, window));
    assert!(!guard(940_000, Some(1_018_592), true, reserve, window));
    // The estimate path is unchanged: a payload that really is too large still compacts,
    // while one that exactly fits the window does not.
    assert!(guard(1_000_000, None, true, reserve, window));
    assert!(!guard(window - reserve, None, false, reserve, window));
}

/// A source-level guard for the three live wirings that silently degrade to `None`.
#[test]
fn usage_guard_and_reasoning_counter_stay_wired_to_the_live_turn() {
    let source = turn_source_text();
    let source = source.as_str();
    assert!(
        source.contains("if context_window_budget_exceeded("),
        "the hard-cap branch must consult the measured guard, not only the estimate"
    );
    assert!(
        !source.contains(
            "if context_tokens.saturating_add(reserved_output_tokens) > context_window_limit"
        ),
        "the estimate-only gate is the defect the measured guard replaced"
    );
    assert!(
        source.contains("reported_request_tokens = Some("),
        "the provider-reported charge must be recorded for the next request"
    );
    assert!(
        source.contains("reported_usage_compacted = true;"),
        "a reported-driven compaction must spend the per-turn allowance"
    );
    assert!(
        source.contains("\"reasoning_tokens\": total.reasoning_tokens"),
        "the live usage payload must carry the provider-reported reasoning counter"
    );
    assert!(
        source.contains("reasoning_tokens: None"),
        "a turn without provider usage must leave the reasoning counter absent, never 0"
    );
}

fn turn_source_text() -> String {
    fn append_files(directory: &std::path::Path, output: &mut String) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                append_files(&path, output);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                output.push_str(&std::fs::read_to_string(path).unwrap());
                output.push('\n');
            }
        }
    }
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/agent");
    let mut source = String::new();
    for filename in ["turn.rs"] {
        source.push_str(&std::fs::read_to_string(directory.join(filename)).unwrap());
    }
    append_files(&directory.join("turn"), &mut source);
    source
}
