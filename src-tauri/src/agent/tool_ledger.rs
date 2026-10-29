use super::*;

pub(super) use agent_core::tool_ledger::{
    begin_tool_call, complete_tool_call, normalize_tool_execution_result, tool_call_can_run,
    tool_call_skip_reason, tool_execution_failure_output, tool_execution_failure_retryable,
    ToolCallLedgerEntry,
};

pub(super) type PendingAgentToolCall = agent_core::PendingToolCall;

pub(super) fn finish_pending_agent_tool_calls(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    default_environment: AgentToolEnvironment,
    windows_paths: &[String],
    ssh_paths: &[String],
    wsl_paths: &[String],
    pending_calls: &mut Vec<PendingAgentToolCall>,
    code: &str,
    message: &str,
    retryable: bool,
) {
    for (call_id, (tool_name, initial_input, arguments)) in pending_calls.drain(..) {
        let scope = tool_execution_scope(
            &tool_name,
            default_environment,
            windows_paths,
            ssh_paths,
            wsl_paths,
        );
        let input = parse_tool_call_input(initial_input.clone(), &arguments, &tool_name)
            .unwrap_or(initial_input);
        emit_agent_event_for_turn_with_file_evidence_path_key(
            app,
            &AgentEvent::ToolCallFinished {
                session_id: session_id.to_string(),
                call_id,
                tool_name,
                input,
                output: json!({
                    "error": { "code": code, "message": message, "retryable": retryable },
                    "code": code,
                    "retryable": retryable,
                    "skipped": true,
                    "interrupted": false,
                }),
                is_error: true,
            },
            Some(turn_id),
            scope.paths,
            scope.is_remote(),
        );
    }
}

pub(super) fn requested_file_read_path(input: &Value) -> Option<String> {
    input
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(|path| path.trim().to_string())
}

pub(super) fn file_read_path_key(
    input: &Value,
    roots: &[String],
    is_remote_target: bool,
) -> Option<String> {
    requested_file_read_path(input).map(|path| file_path_key(&path, roots, is_remote_target))
}

pub(super) fn file_path_key(path: &str, roots: &[String], is_remote_target: bool) -> String {
    if is_remote_target {
        let path = path.trim().replace('\\', "/");
        let path = if path.starts_with('/') {
            path
        } else {
            let root = roots
                .first()
                .map(|root| agent_core::normalize_linux_path(root))
                .unwrap_or_else(|| "/".to_string());
            format!("{root}/{path}")
        };
        return agent_core::normalize_linux_path(&path);
    }
    let resolved = agent_core::resolve_selected_path(Path::new(path), roots);
    agent_core::policy_path_key(&resolved)
}

pub(super) fn is_successful_file_read_event(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::ToolCallFinished {
            tool_name,
            output,
            is_error: false,
            ..
        } if agent_core::canonical_tool_name(tool_name) == "read_local_file"
            && !output
                .get("error")
                .is_some_and(|value| !value.is_null())
            && !output
                .get("skipped")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            && !output
                .get("steered")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            && output
                .get("path")
                .and_then(Value::as_str)
                .is_some_and(|path| !path.trim().is_empty())
    )
}

pub(super) fn failed_file_read(events: &[AgentEvent]) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ToolCallFinished { tool_name, .. }
                if agent_core::canonical_tool_name(tool_name) == "read_local_file"
        ) && !is_successful_file_read_event(event)
    })
}

pub(super) fn failed_file_read_instruction(
    tool_name: &str,
    input: &Value,
    events: &[AgentEvent],
) -> Option<AgentMessage> {
    if agent_core::canonical_tool_name(tool_name) != "read_local_file" || !failed_file_read(events)
    {
        return None;
    }
    let path = requested_file_read_path(input).unwrap_or_else(|| "指定路径".to_string());
    Some(AgentMessage::system(format!(
        "本轮读取指定文件 `{path}` 未成功。不得声称本轮已经读取该文件，也不得把历史会话中同名文件的内容描述为本轮工具结果。请根据工具返回的失败原因自行判断下一步；如需继续，可搜索、列目录或读取其他由工具返回的精确路径。"
    )))
}
