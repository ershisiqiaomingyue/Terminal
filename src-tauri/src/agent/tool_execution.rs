use super::*;

#[derive(Clone)]
pub(super) struct ToolExecutionLanes {
    pub(super) global: Arc<Semaphore>,
    pub(super) remote: Arc<Semaphore>,
}

pub(super) struct ToolExecutionPermit {
    pub(super) _remote: Option<OwnedSemaphorePermit>,
    pub(super) _global: OwnedSemaphorePermit,
}

impl ToolExecutionLanes {
    pub(super) fn new(max_parallel: usize) -> Self {
        Self {
            global: Arc::new(Semaphore::new(max_parallel.max(1))),
            remote: Arc::new(Semaphore::new(1)),
        }
    }

    pub(super) async fn acquire(&self, is_remote: bool) -> ToolExecutionPermit {
        // Wait for the remote lane before consuming a global slot. Otherwise
        // queued SSH reads could occupy every slot and head-of-line block a
        // fast Windows inspector.
        let remote = if is_remote {
            Some(
                self.remote
                    .clone()
                    .acquire_owned()
                    .await
                    .expect("tool remote lane remains open"),
            )
        } else {
            None
        };
        let global = self
            .global
            .clone()
            .acquire_owned()
            .await
            .expect("tool execution lane remains open");
        ToolExecutionPermit {
            _remote: remote,
            _global: global,
        }
    }
}

pub(super) fn cancellation_message(reason: &Arc<Mutex<Option<String>>>) -> String {
    reason
        .lock()
        .ok()
        .and_then(|value| value.clone())
        .unwrap_or_else(|| "请求已取消".to_string())
}

#[cfg(test)]
pub(super) fn active_turn_owned_by(
    state: &AgentRuntimeState,
    session_id: &str,
    turn_id: &str,
    cancellation: &Arc<AtomicBool>,
) -> bool {
    state
        .requests
        .lock()
        .map(|requests| {
            requests.get(session_id).is_some_and(|control| {
                control.turn_id == turn_id && Arc::ptr_eq(&control.cancellation, cancellation)
            })
        })
        .unwrap_or(false)
}

pub(super) enum InterruptibleToolExecution {
    Finished(Result<agent_core::ToolCallResult, String>),
    Cancelled,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_tool_call_interruptible<E, F>(
    registry: &ToolRegistry,
    permissions: &E,
    state: &AgentRuntimeState,
    session_id: &str,
    call_id: &str,
    tool_name: &str,
    input: Value,
    turn_id: &str,
    turn_cancellation: &Arc<AtomicBool>,
    tool_cancellation: &Arc<AtomicBool>,
    on_execution_start: F,
) -> Result<InterruptibleToolExecution, String>
where
    E: agent_core::PermissionEvaluator + ?Sized,
    F: FnOnce(),
{
    // G4-D：PreToolUse hook——唯一决策接入点（全部工具执行都过这里）。
    // Block 时工具不执行，返回带 skipped 语义的错误结果；Modify 替换入参。
    // 无 hooks.json 配置时此调用零开销直通。
    let mut input = input;
    match super::hooks::run_pre_tool_hooks(tool_name, &input).await {
        super::hooks::HookDecision::Block { reason } => {
            return Ok(InterruptibleToolExecution::Finished(Ok(
                agent_core::ToolCallResult {
                    images: Vec::new(),
                    message: agent_core::AgentMessage::tool(call_id, ""),
                    events: vec![agent_core::AgentEvent::ToolCallFinished {
                        session_id: session_id.to_string(),
                        call_id: call_id.to_string(),
                        tool_name: tool_name.to_string(),
                        input: input.clone(),
                        output: serde_json::json!({
                            "error": {
                                "code": "hook_blocked",
                                "message": format!("工具被 PreToolUse hook 拦截：{reason}"),
                                "retryable": false,
                            },
                            "code": "hook_blocked",
                            "retryable": false,
                            "skipped": true,
                            "hook_blocked": true,
                        }),
                        is_error: true,
                    }],
                    permission_request: None,
                },
            )));
        }
        super::hooks::HookDecision::Modify { input: modified } => {
            input = modified;
        }
        super::hooks::HookDecision::Allow => {}
    }
    // G4-A：同步 TurnHook 通道——外部 handler 若有匹配，它的 stdout 直接就是
    // 工具结果，内置实现不再执行。无配置时这里同样零开销。
    if let Some(outcome) =
        super::hooks::run_turn_hooks(session_id, turn_id, tool_name, &input).await
    {
        let (output, is_error) = match outcome {
            Ok(text) => (
                serde_json::json!({ "output": text, "served_by": "TurnHook" }),
                false,
            ),
            Err(reason) => (
                serde_json::json!({
                    "error": {
                        "code": "hook_failed",
                        "message": format!("TurnHook 执行失败：{reason}"),
                        "retryable": false,
                    },
                    "code": "hook_failed",
                    "retryable": false,
                    "hook_failed": true,
                }),
                true,
            ),
        };
        return Ok(InterruptibleToolExecution::Finished(Ok(
            agent_core::ToolCallResult {
                images: Vec::new(),
                message: agent_core::AgentMessage::tool(call_id, ""),
                events: vec![agent_core::AgentEvent::ToolCallFinished {
                    session_id: session_id.to_string(),
                    call_id: call_id.to_string(),
                    tool_name: tool_name.to_string(),
                    input: input.clone(),
                    output,
                    is_error,
                }],
                permission_request: None,
            },
        )));
    }
    let execution = execute_tool_call_with_start(
        registry,
        permissions,
        session_id,
        call_id,
        tool_name,
        input,
        on_execution_start,
    );
    tokio::pin!(execution);
    tokio::select! {
        biased;
        _ = wait_for_agent_cancellation(state, session_id, turn_cancellation) => {
            tool_cancellation.store(true, Ordering::Relaxed);
        }
        result = &mut execution => {
            // G4-D：PostToolUse hook——纯通知（失败只记日志），事件已经发生。
            if let Ok(outcome) = &result {
                for event in &outcome.events {
                    if let agent_core::AgentEvent::ToolCallFinished {
                        tool_name,
                        output,
                        is_error,
                        ..
                    } = event
                    {
                        super::hooks::run_post_tool_hooks(tool_name, output, *is_error).await;
                    }
                }
            }
            return Ok(InterruptibleToolExecution::Finished(result));
        }
    }
    let _ = tokio::time::timeout(TOOL_CANCELLATION_GRACE, &mut execution)
        .await
        .map_err(|_| {
            turn_cancellation.store(true, Ordering::Relaxed);
            // P1-B/P2：直接置位标志的路径也要同步取消令牌，保证标志与令牌
            // 永远一致（等待在 await 上的阶段才能被打断）。
            agent_core::cancel_request_token_if_matching(state, session_id, turn_cancellation);
            format!(
                "工具 {tool_name} 在取消后 {} 秒内未退出，已终止当前回合",
                TOOL_CANCELLATION_GRACE.as_secs()
            )
        })?;
    Ok(InterruptibleToolExecution::Cancelled)
}
