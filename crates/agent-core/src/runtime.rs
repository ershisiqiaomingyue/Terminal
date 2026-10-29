use crate::{
    prune_tool_result_for_model, AgentEvent, AgentMessage, AgentToolCall, PermissionEvaluator,
    ToolExecutionError, ToolRegistry,
};
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct ToolCallResult {
    pub message: AgentMessage,
    /// Transient binary observations; never serialized into textual tool output.
    pub images: Vec<crate::AgentImageAttachment>,
    pub events: Vec<AgentEvent>,
    pub permission_request: Option<String>,
}

impl ToolCallResult {
    /// Called before emitting events, after permissions have settled. The owner is
    /// derived from the tool call id, so image replay also survives serialization.
    pub fn attach_images(
        &mut self,
        supports_vision: Option<bool>,
        attachments: &mut Vec<crate::ProviderImageAttachment>,
    ) {
        if self.images.is_empty() {
            return;
        }
        let count = self.images.len();
        let owner =
            crate::tool_image_owner(self.message.tool_call_id.as_deref().unwrap_or_default());
        let status = if supports_vision == Some(false) {
            "当前模型不支持图片输入：截图未发送给模型。请使用 DOM/文本证据或切换视觉模型；不能声称已看见截图。".to_string()
        } else {
            match crate::normalize_image_attachments(&self.images, supports_vision, &owner) {
                Ok(images) => {
                    attachments.extend(images);
                    format!("{count} 张工具图片已作为视觉输入附在本次工具结果中。")
                }
                Err(error) => format!("工具图片未发送：{error}"),
            }
        };
        self.images.clear();
        for event in &mut self.events {
            if let AgentEvent::ToolCallFinished { output, .. } = event {
                if !output.is_object() {
                    *output = json!({"result":output});
                }
                output["visualObservation"] = json!({"count":count,"message":status});
                self.message.content = prune_tool_result_for_model(&output.to_string());
            }
        }
    }
}

fn tool_error_code(error: &ToolExecutionError) -> (&'static str, bool) {
    match error {
        ToolExecutionError::NotFound(_) => ("tool_not_found", false),
        ToolExecutionError::NoHandler(_) => ("tool_not_implemented", false),
        ToolExecutionError::PermissionDenied { .. } => ("permission", false),
        ToolExecutionError::PermissionRequired { .. } => ("permission", false),
        ToolExecutionError::Handler(message) => {
            let normalized = message.to_ascii_lowercase();
            if message.starts_with(crate::process::PROCESS_CLEANUP_ERROR_PREFIX) {
                ("process_cleanup_unconfirmed", false)
            } else if message.contains("请求已取消") || normalized.contains("cancel") {
                ("cancelled", true)
            } else if message.contains("上下文")
                || normalized.contains("context")
                || normalized.contains("token")
            {
                ("context_compaction", true)
            } else if normalized.contains("rust-analyzer") || normalized.contains("lsp") {
                ("lsp_server", false)
            } else if normalized.contains("mcp") {
                ("mcp", true)
            } else {
                ("tool_execution", false)
            }
        }
    }
}

fn tool_output_is_error(tool_name: &str, output: &Value) -> bool {
    if output.get("success").and_then(Value::as_bool) == Some(false) || output["isError"] == true {
        return true;
    }
    if !matches!(
        crate::canonical_tool_name(tool_name),
        "run_local_command" | "run_local_shell" | "run_project_check"
    ) {
        return false;
    }
    output
        .get("exitCode")
        .or_else(|| output.get("exit_code"))
        .and_then(Value::as_i64)
        .is_some_and(|code| code != 0)
}

pub async fn execute_tool_call<E>(
    registry: &ToolRegistry,
    permissions: &E,
    session_id: impl Into<String>,
    call_id: impl Into<String>,
    tool_name: impl Into<String>,
    input: Value,
) -> Result<ToolCallResult, String>
where
    E: PermissionEvaluator + ?Sized,
{
    execute_tool_call_with_start(
        registry,
        permissions,
        session_id,
        call_id,
        tool_name,
        input,
        || {},
    )
    .await
}

pub async fn execute_tool_call_with_start<E, F>(
    registry: &ToolRegistry,
    permissions: &E,
    session_id: impl Into<String>,
    call_id: impl Into<String>,
    tool_name: impl Into<String>,
    input: Value,
    on_execution_start: F,
) -> Result<ToolCallResult, String>
where
    E: PermissionEvaluator + ?Sized,
    F: FnOnce(),
{
    let session_id = session_id.into();
    let call_id = call_id.into();
    let tool_name = tool_name.into();
    let result = registry
        .execute_with_start(&tool_name, input.clone(), permissions, on_execution_start)
        .await;
    match result {
        Ok(mut output) => {
            let images = crate::tool_images::extract_tool_images(&mut output);
            let is_error = tool_output_is_error(&tool_name, &output);
            Ok(ToolCallResult {
                images,
                message: AgentMessage::tool(
                    &call_id,
                    prune_tool_result_for_model(&output.to_string()),
                ),
                events: vec![AgentEvent::ToolCallFinished {
                    session_id,
                    call_id,
                    tool_name,
                    input,
                    output,
                    is_error,
                }],
                permission_request: None,
            })
        }
        Err(ToolExecutionError::PermissionRequired { request_id }) => Ok(ToolCallResult {
            images: Vec::new(),
            message: AgentMessage::tool(&call_id, ""),
            events: vec![AgentEvent::PermissionRequested {
                session_id,
                request_id: request_id.clone(),
                tool_name,
                input,
                expires_at_ms: None,
            }],
            permission_request: Some(request_id),
        }),
        Err(error) => {
            let message = error.to_string();
            let (code, retryable) = tool_error_code(&error);
            let output = json!({
                "error": {
                    "code": code,
                    "message": message,
                    "retryable": retryable,
                },
                "code": code,
                "retryable": retryable,
            });
            Ok(ToolCallResult {
                images: Vec::new(),
                message: AgentMessage::tool(
                    &call_id,
                    prune_tool_result_for_model(&output.to_string()),
                ),
                events: vec![AgentEvent::ToolCallFinished {
                    session_id,
                    call_id,
                    tool_name,
                    input,
                    output,
                    is_error: true,
                }],
                permission_request: None,
            })
        }
    }
}

pub fn assistant_tool_message(
    call_id: impl Into<String>,
    tool_name: impl Into<String>,
    input: Value,
) -> AgentMessage {
    AgentMessage::assistant_tool_call(AgentToolCall {
        id: call_id.into(),
        name: tool_name.into(),
        input,
    })
}

#[cfg(test)]
mod tests {
    use super::{execute_tool_call, tool_output_is_error};
    use crate::{AgentEvent, AllowPermission, ToolDefinition, ToolHandler, ToolRegistry};
    use async_trait::async_trait;
    use serde_json::{json, Value};

    #[test]
    fn unconfirmed_process_cleanup_is_never_a_retryable_cancellation() {
        let error = crate::ToolExecutionError::Handler(format!(
            "{}请求已取消，但未确认进程停止",
            crate::process::PROCESS_CLEANUP_ERROR_PREFIX
        ));
        assert_eq!(
            super::tool_error_code(&error),
            ("process_cleanup_unconfirmed", false)
        );
    }

    struct FailedCommand;

    #[async_trait]
    impl ToolHandler for FailedCommand {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new(
                "run_local_command",
                "fixed command output",
                json!({ "type": "object" }),
            )
            .read_only()
        }

        async fn execute(&self, _input: Value) -> Result<Value, String> {
            Ok(json!({ "success": false, "exitCode": 1, "stderr": "failed" }))
        }
    }

    #[tokio::test]
    async fn structured_command_failure_marks_tool_event_as_error() {
        let mut registry = ToolRegistry::new();
        registry
            .register_handler(FailedCommand)
            .expect("register failed command");
        let result = execute_tool_call(
            &registry,
            &AllowPermission,
            "session",
            "call",
            "run_local_command",
            json!({}),
        )
        .await
        .expect("execute command");

        assert!(matches!(
            result.events.as_slice(),
            [AgentEvent::ToolCallFinished { is_error: true, .. }]
        ));
    }

    #[test]
    fn non_command_exit_code_is_not_overclassified() {
        assert!(!tool_output_is_error(
            "query_android_device",
            &json!({ "exitCode": 1, "error": "diagnostic evidence" }),
        ));
        assert!(tool_output_is_error(
            "query_android_device",
            &json!({ "success": false }),
        ));
    }

    struct HugeOutput;

    #[async_trait]
    impl ToolHandler for HugeOutput {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new(
                "read_local_file",
                "huge result",
                json!({ "type": "object" }),
            )
            .read_only()
        }

        async fn execute(&self, _input: Value) -> Result<Value, String> {
            Ok(json!({ "content": "x".repeat(20_000) }))
        }
    }

    /// The model must not be re-sent a giant tool result verbatim, while the UI
    /// (and the durable session record) still receives the complete evidence
    /// through the tool event.
    #[tokio::test]
    async fn oversized_tool_result_is_pruned_for_the_model_but_kept_in_the_event() {
        let mut registry = ToolRegistry::new();
        registry
            .register_handler(HugeOutput)
            .expect("register huge output");
        let result = execute_tool_call(
            &registry,
            &AllowPermission,
            "session",
            "call",
            "read_local_file",
            json!({ "path": "a.txt" }),
        )
        .await
        .expect("execute huge output");

        assert!(result.message.content.contains("中间内容已裁剪"));
        assert_eq!(
            result.message.content.chars().count(),
            crate::tool_output::TOOL_RESULT_EDGE_CHARS * 2
                + crate::tool_output::TOOL_RESULT_ELISION_MARKER
                    .chars()
                    .count()
        );

        let [AgentEvent::ToolCallFinished { output, .. }] = result.events.as_slice() else {
            panic!("a tool result must produce one terminal event");
        };
        assert_eq!(output["content"].as_str().unwrap().len(), 20_000);
    }

    struct Screenshot;
    #[async_trait]
    impl ToolHandler for Screenshot {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new(
                "mcp__browser__screenshot",
                "mock browser evidence",
                json!({"type":"object"}),
            )
            .read_only()
        }
        async fn execute(&self, _: Value) -> Result<Value, String> {
            Ok(
                json!({"isError":false,"content":[{"type":"text","text":"DOM button found"},{"type":"image","mimeType":"image/png","data":"AQID"}]}),
            )
        }
    }

    #[tokio::test]
    async fn screenshot_evidence_reaches_all_protocols_after_parallel_tool_results_and_survives_replay(
    ) {
        use crate::{
            AgentMessage, AgentToolCall, ModelTranscript, ProviderConfig, ProviderKind,
            ProviderRequest,
        };
        let mut registry = ToolRegistry::new();
        registry.register_handler(Screenshot).unwrap();
        for vision in [Some(true), Some(false), None] {
            let mut result = execute_tool_call(
                &registry,
                &AllowPermission,
                "session",
                "screen",
                "mcp__browser__screenshot",
                json!({}),
            )
            .await
            .unwrap();
            assert!(!result.message.content.contains("AQID"));
            assert!(!serde_json::to_string(&result.events)
                .unwrap()
                .contains("AQID"));
            let mut images = Vec::new();
            result.attach_images(vision, &mut images);
            if vision == Some(false) {
                assert!(images.is_empty());
                assert!(result.message.content.contains("不支持图片输入"));
                continue;
            }
            let mut assistant = AgentMessage::assistant_tool_call(AgentToolCall {
                id: "screen".into(),
                name: "mcp__browser__screenshot".into(),
                input: json!({}),
            });
            assistant.tool_calls.push(AgentToolCall {
                id: "second".into(),
                name: "search_local_code".into(),
                input: json!({"query":"button"}),
            });
            let messages = vec![
                assistant,
                result.message,
                AgentMessage::tool("second", "source location"),
            ];
            let transcript = ModelTranscript {
                version: 1,
                scope: "scope".into(),
                turn_id: "turn".into(),
                runtime_context: String::new(),
                messages,
                images,
                last_request: None,
            };
            let restored: ModelTranscript =
                serde_json::from_str(&serde_json::to_string(&transcript).unwrap()).unwrap();
            assert!(restored.is_replayable("scope"));
            let request =
                ProviderRequest::new("session", "vision", restored.messages.clone(), Vec::new())
                    .with_image_attachments(restored.images.clone());
            for kind in [
                ProviderKind::OpenAi,
                ProviderKind::OpenAiCompatible,
                ProviderKind::OpenAiResponses,
                ProviderKind::Anthropic,
            ] {
                let config =
                    ProviderConfig::new("test", "test", kind, "https://test.invalid/v1", "test")
                        .unwrap();
                let body = crate::request_body(&config, &request).unwrap();
                match kind {
                    ProviderKind::OpenAiResponses => {
                        assert_eq!(body["input"][3]["type"], "function_call_output");
                        assert_eq!(
                            body["input"][4]["content"][1]["image_url"],
                            "data:image/png;base64,AQID"
                        );
                    }
                    ProviderKind::Anthropic => {
                        assert_eq!(body["messages"][1]["content"].as_array().unwrap().len(), 2);
                        assert_eq!(body["messages"][2]["content"][1]["source"]["data"], "AQID");
                    }
                    _ => {
                        assert_eq!(body["messages"][1]["role"], "tool");
                        assert_eq!(body["messages"][2]["role"], "tool");
                        assert_eq!(
                            body["messages"][3]["content"][1]["image_url"]["url"],
                            "data:image/png;base64,AQID"
                        );
                    }
                }
            }
            assert_eq!(
                crate::image_attachments_for_messages(&restored.images, &[]).len(),
                0
            );
            assert!(crate::estimate_image_attachment_tokens(&restored.images) >= 1024);
        }
        assert!(tool_output_is_error(
            "mcp__browser__screenshot",
            &json!({"isError":true})
        ));
    }
}
