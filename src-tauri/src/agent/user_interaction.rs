use super::events::{
    emit_thread_snapshot_event, emit_thread_snapshot_for_payload, process_events_for_snapshot,
};
use super::*;

pub(super) const ASK_USER_TOOL: &str = "ask_user";
const QUESTION_SCOPE: &str = "interaction:ask_user";
/// A question that is never answered expires after this window and is treated
/// as skipped, so a forgotten prompt cannot hold the turn forever.
/// P6-B/P6-C：与工具权限超时同源配置（`agent-settings.json` 的
/// `permissionTimeoutSec`，0 = 不超时），缺省 300 秒。
fn question_timeout() -> Option<std::time::Duration> {
    super::strategy::tool_question_timeout()
}

/// 询问工具：向用户提问并等待真实回答。
///
/// 注意：这里刻意不接收工具级取消令牌（每轮工具调用的 `tool_cancellation`）。
/// `register_pending_permission` / `is_cancelled` 比较的是取消令牌的**身份**，
/// 工具级令牌永远不等于当前请求的令牌，会让每个询问都瞬间被判成「请求已取消」。
/// 因此询问只在执行时用 `session_id` + `turn_id` 解析当前活跃请求的令牌。
/// ask_user 的问题卡片 payload；N2 倒计时与等待超时同源，
/// P6-C 配置为 0（不超时）时不带 expires_at_ms。
fn json_payload_with_question_deadline(
    session_id: &str,
    request_id: &str,
    call_id: &str,
    question: &Value,
) -> serde_json::Value {
    let base = json!({
        "type": "permission_requested",
        "session_id": session_id,
        "request_id": request_id,
        "call_id": call_id,
        "tool_name": ASK_USER_TOOL,
        "input": question,
    });
    match question_timeout() {
        Some(timeout) => {
            let mut payload = base;
            if let serde_json::Value::Object(object) = &mut payload {
                object.insert(
                    "expires_at_ms".to_string(),
                    json!(std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|elapsed| elapsed.as_millis() as i64 + timeout.as_millis() as i64)
                        .unwrap_or_default()),
                );
            }
            payload
        }
        None => base,
    }
}

pub(super) struct AskUserTool {
    pub app: AppHandle,
    pub session_id: String,
    pub turn_id: String,
}

fn question_input(input: &Value) -> Result<Value, String> {
    let question = input
        .get("question")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.chars().count() <= 2_000)
        .ok_or("question 必须是 1–2000 字的明确问题")?;
    let options = match input.get("options") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(values)) if values.len() <= 6 => {
            let mut options = Vec::new();
            for value in values {
                let text = value
                    .as_str()
                    .map(str::trim)
                    .filter(|text| !text.is_empty() && text.chars().count() <= 200)
                    .ok_or("options 必须是不超过 200 字的非空字符串")?;
                if options.iter().any(|option| option == text) {
                    return Err("options 不能重复".into());
                }
                options.push(text.to_string());
            }
            options
        }
        _ => return Err("options 必须是最多 6 项的数组".into()),
    };
    Ok(json!({ "question": question, "options": options }))
}

#[async_trait]
impl ToolHandler for AskUserTool {
    fn definition(&self) -> ToolDefinition {
        // Keep this serial: two simultaneous prompts cannot own the same composer.
        ToolDefinition::new(ASK_USER_TOOL,
            "Ask the user for missing information, a preference or a required decision. Opens an interactive question and waits for their real answer. Use only when context and available tools cannot resolve the uncertainty. Do not use to request tool execution permissions; the runtime handles those. Never invent an answer or treat skipping as approval.",
            json!({ "type": "object", "properties": {
                "question": { "type": "string", "maxLength": 2000 },
                "options": { "type": "array", "maxItems": 6, "items": { "type": "string", "maxLength": 200 }, "description": "Optional suggested answers. Free text is always available." }
            }, "required": ["question"], "additionalProperties": false }))
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let question = question_input(&input)?;
        let call_id = input
            .get("__agent_call_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or("询问缺少运行时调用标识")?;
        let request_id = format!("question-{}", uuid_suffix());
        let state = self.app.state::<AgentRuntimeState>();
        // 用当前活跃请求的取消令牌登记与等待，工具级令牌会让询问立刻失效。
        let cancellation =
            agent_core::request_cancellation(&state, &self.session_id, &self.turn_id)?;
        let receiver = register_pending_permission(
            &state,
            &self.session_id,
            request_id.clone(),
            QUESTION_SCOPE.into(),
            call_id.into(),
            cancellation.clone(),
            self.turn_id.clone(),
            None,
        )?;
        emit_thread_snapshot_event(
            &self.app,
            &state,
            ThreadEvent::ItemAdded {
                thread_id: self.session_id.clone(),
                turn_id: self.turn_id.clone(),
                item: ThreadItem::AgentMessage {
                    id: format!("question:{request_id}"),
                    content: question["question"].as_str().unwrap_or_default().into(),
                    pending: false,
                },
            },
            None,
        );
        emit_agent_payload_for_turn(
            &self.app,
            json_payload_with_question_deadline(&self.session_id, &request_id, &call_id, &question),
            &self.turn_id,
        );
        settle_thread_process(&self.app, &self.session_id, &self.turn_id);
        // N1-A：超时合成的 Deny 也要发 permission_resolved，否则问题卡片
        // 永远等不到回应（与工具权限同型的 N1 卡死）。
        let question_request_id = request_id.clone();
        let PermissionWaitResult::Decision(response) = wait_for_permission(
            &state,
            &self.session_id,
            request_id,
            cancellation,
            receiver,
            question_timeout(),
        )
        .await?;
        if response.timed_out {
            emit_thread_snapshot_event(
                &self.app,
                &state,
                ThreadEvent::PermissionResolved {
                    thread_id: self.session_id.clone(),
                    turn_id: self.turn_id.clone(),
                    request_id: question_request_id,
                },
                None,
            );
        }
        Ok(question_result(response))
    }
}

fn question_result(response: PermissionResponse) -> Value {
    match response.action {
        PermissionAction::Other => {
            json!({ "status": "answered", "answer": response.instruction.unwrap_or_default() })
        }
        _ => json!({ "status": "skipped", "answer": null,
            "instruction": "用户跳过了这个问题，没有提供答案或授予任何权限。不要猜测答案；依赖该答案的操作必须暂停。" }),
    }
}

fn normalized_reply(
    action: PermissionAction,
    instruction: Option<String>,
    question: bool,
) -> Result<Option<String>, String> {
    if question && !matches!(action, PermissionAction::Other | PermissionAction::Deny) {
        return Err("询问只支持回答或跳过，不能授予工具权限".into());
    }
    let instruction = instruction
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    if instruction
        .as_ref()
        .is_some_and(|text| text.chars().count() > 8_000)
    {
        return Err("回答不能超过 8000 字".into());
    }
    if action == PermissionAction::Other && instruction.is_none() {
        return Err("请先输入回答或说明".into());
    }
    Ok(if action == PermissionAction::Other {
        instruction
    } else {
        None
    })
}

fn user_reply_event(
    action: PermissionAction,
    instruction: Option<&str>,
    session_id: &str,
    turn_id: &str,
    request_id: &str,
) -> Option<ThreadEvent> {
    (action == PermissionAction::Other).then(|| ThreadEvent::ItemAdded {
        thread_id: session_id.to_string(),
        turn_id: turn_id.to_string(),
        item: ThreadItem::UserMessage {
            id: format!("user:{turn_id}:reply:{request_id}"),
            content: instruction.unwrap_or_default().to_string(),
            attachments: vec![],
            artifacts: vec![],
        },
    })
}

pub(super) fn resolve_interaction(
    app: &AppHandle,
    state: &AgentRuntimeState,
    session_id: String,
    request_id: String,
    decision: String,
    instruction: Option<String>,
) -> Result<(), String> {
    let action = PermissionAction::parse(decision.trim())?;
    let key = (session_id.clone(), request_id.clone());
    // Serialize submissions until the durable reply and the reserved channel
    // slot are committed. Cancellation cannot remove this waiter mid-commit.
    let mut permissions = state
        .permissions
        .lock()
        .map_err(|_| "权限状态不可用".to_string())?;
    let pending = permissions.get_mut(&key).ok_or("询问不存在或已过期")?;
    if pending.response_sent {
        return Ok(());
    }
    if is_cancelled(state, &session_id, &pending.cancellation) {
        return Err("询问已取消".into());
    }
    let question = pending.approval_key == QUESTION_SCOPE;
    let instruction = normalized_reply(action, instruction, question)?;
    let permit = pending
        .sender
        .clone()
        .try_reserve_owned()
        .map_err(|_| "回答通道已关闭或正在处理".to_string())?;
    let turn_id = pending.turn_id.clone();
    let event = user_reply_event(
        action,
        instruction.as_deref(),
        &session_id,
        &turn_id,
        &request_id,
    );
    let mut snapshot = state
        .read_thread_snapshot(&session_id)
        .ok_or("对话状态已过期")?;
    if !snapshot.apply(ThreadEvent::PermissionResolved {
        thread_id: session_id.clone(),
        turn_id: turn_id.clone(),
        request_id: request_id.clone(),
    }) {
        return Err("当前询问已结束或切换，无法回答".into());
    }
    if let Some(event) = event.as_ref() {
        if !snapshot.apply(event.clone()) {
            return Err("当前对话已结束或切换，无法回答".into());
        }
    }
    let payload = stamp_payload_with_turn(
        json!({ "type": "permission_decision",
            "session_id": session_id, "request_id": request_id, "call_id": pending.call_id,
            "decision": action.as_str(), "instruction": instruction, "interaction_kind": if question { "question" } else { "permission" },
        }),
        Some(&turn_id),
    );
    let mut events = process_events_for_snapshot(&snapshot, &turn_id);
    events.push(payload.clone());
    // Publish and resume only after history accepts the answer. Retrying after
    // a persistence failure neither grants access nor duplicates a user turn.
    crate::history::append_history_session_events(
        app.clone(),
        session_id.clone(),
        payload["timestamp_ms"].as_i64().unwrap_or_default(),
        events,
    )?;
    if action == PermissionAction::Once {
        // P7-B：一次性授权同样补环境种类 key（换连接后仍是同一远程环境）。
        let mut keys = vec![pending.approval_key.clone()];
        if let Some(kind_key) = environment_kind_key(&pending.approval_key) {
            keys.push(kind_key);
        }
        let mut approvals = state
            .one_time_approvals
            .lock()
            .map_err(|_| "权限状态不可用")?;
        for key in keys {
            approvals.insert((session_id.clone(), key));
        }
    } else if action == PermissionAction::Session {
        let mut approvals = state
            .session_approvals
            .lock()
            .map_err(|_| "权限状态不可用".to_string())?;
        approvals.insert((session_id.clone(), pending.approval_key.clone()));
        // P7-B：同时记一层「环境种类 key」（`ssh::…` / `wsl::…`）。换连接或换
        // profile 后具体目标 id 变了，同一环境的授权仍然命中——用户感知里那是
        // 同一个远程环境，不该再点一次。
        if let Some(kind_key) = environment_kind_key(&pending.approval_key) {
            approvals.insert((session_id.clone(), kind_key));
        }
    }
    if let Some(event) = event {
        emit_thread_snapshot_event(app, state, event, None);
    }
    emit_thread_snapshot_for_payload(app, &payload);
    let _ = app.emit("agent:event", payload);
    pending.response_sent = true;
    permit.send(PermissionResponse {
        action,
        instruction,
        timed_out: false,
    });
    Ok(())
}

/// P7-B：把某个已存授权 key 收敛成「同一环境」的形态：去掉 namespace 里
/// 具体的目标 id，只保留环境种类。`ssh:connection-a::write_local_file:/x` →
/// `ssh::write_local_file:/x`；不带 namespace（本地）的 key 返回 `None`。
pub(super) fn environment_kind_key(key: &str) -> Option<String> {
    let kind = agent_core::permission_scope_namespace_kind(key);
    if kind.is_empty() {
        return None;
    }
    let (_, scope) = key.split_once("::")?;
    Some(format!("{kind}::{scope}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_kind_key_drops_only_the_concrete_target_id() {
        assert_eq!(
            environment_kind_key("ssh:connection-a::write_local_file:/remote/x").as_deref(),
            Some("ssh::write_local_file:/remote/x")
        );
        assert_eq!(
            environment_kind_key("wsl:ubuntu::remote-command:run_local_shell:bash:/home/x")
                .as_deref(),
            Some("wsl::remote-command:run_local_shell:bash:/home/x")
        );
        assert_eq!(
            environment_kind_key("ssh:unavailable::file:/x").as_deref(),
            Some("ssh::file:/x")
        );
        // 本地 key 没有 namespace，不该凭空造出种类 key。
        assert_eq!(environment_kind_key("file:/x"), None);
        assert_eq!(environment_kind_key("ssh::file:/x"), None);
    }

    #[test]
    fn questions_are_bounded_and_do_not_accept_malformed_options() {
        assert!(question_input(&json!({ "question": "   " })).is_err());
        assert!(question_input(&json!({ "question": "选哪个？", "options": ["a", "a"] })).is_err());
        assert!(question_input(&json!({ "question": "选哪个？", "options": [1] })).is_err());
        assert!(question_input(&json!({ "question": "x".repeat(2001) })).is_err());
        assert_eq!(
            question_input(&json!({ "question": "  编译哪个目标？ " })).unwrap(),
            json!({ "question": "编译哪个目标？", "options": [] })
        );
    }

    #[test]
    fn answers_never_become_execution_approvals() {
        assert!(normalized_reply(PermissionAction::Once, None, true).is_err());
        assert!(normalized_reply(PermissionAction::Session, None, true).is_err());
        assert!(normalized_reply(PermissionAction::Other, Some(" ".into()), true).is_err());
        assert!(normalized_reply(PermissionAction::Other, Some("x".repeat(8001)), false).is_err());
        let answer = normalized_reply(
            PermissionAction::Other,
            Some(" n60-userdebug ".into()),
            true,
        )
        .unwrap();
        assert_eq!(
            question_result(PermissionResponse {
                action: PermissionAction::Other,
                instruction: answer,
                timed_out: false,
            })["answer"],
            "n60-userdebug"
        );
        let skipped = question_result(PermissionResponse {
            action: PermissionAction::Deny,
            instruction: None,
            timed_out: false,
        });
        assert_eq!(skipped["status"], "skipped");
        assert!(skipped["answer"].is_null());
    }

    #[test]
    fn fixed_permission_decisions_do_not_become_chat_messages() {
        for action in [
            PermissionAction::Once,
            PermissionAction::Session,
            PermissionAction::Deny,
            PermissionAction::Other,
        ] {
            let mut snapshot = agent_core::ThreadSnapshot::new("s");
            snapshot.apply(ThreadEvent::TurnStarted {
                thread_id: "s".into(),
                turn_id: "t".into(),
            });
            snapshot.apply(ThreadEvent::PermissionRequested {
                thread_id: "s".into(),
                turn_id: "t".into(),
                request_id: "p".into(),
                tool_name: "list_local_files".into(),
                expires_at_ms: None,
            });
            assert!(snapshot.apply(ThreadEvent::PermissionResolved {
                thread_id: "s".into(),
                turn_id: "t".into(),
                request_id: "p".into()
            }));
            if let Some(event) = user_reply_event(action, Some("仅查看 src 目录"), "s", "t", "p")
            {
                assert!(snapshot.apply(event));
            }
            assert!(snapshot.pending_permission.is_none());
            assert_eq!(snapshot.status, agent_core::ThreadStatus::Running);
            if action == PermissionAction::Other {
                assert!(
                    matches!(&snapshot.items[..], [ThreadItem::UserMessage { content, .. }] if content == "仅查看 src 目录")
                );
            } else {
                assert!(snapshot.items.is_empty());
            }
        }
    }

    #[tokio::test]
    async fn question_answers_are_scoped_and_cancelled_waiters_do_not_resume() {
        let state = AgentRuntimeState::default();
        let first = AgentRequestGuard::start(&state, "first".into(), "t1".into()).unwrap();
        let second = AgentRequestGuard::start(&state, "second".into(), "t2".into()).unwrap();
        let first_cancel = state.requests.lock().unwrap()["first"].cancellation.clone();
        let second_cancel = state.requests.lock().unwrap()["second"]
            .cancellation
            .clone();
        let first_rx = register_pending_permission(
            &state,
            "first",
            "q".into(),
            QUESTION_SCOPE.into(),
            "c1".into(),
            first_cancel.clone(),
            "t1".into(),
            None,
        )
        .unwrap();
        let second_rx = register_pending_permission(
            &state,
            "second",
            "q".into(),
            QUESTION_SCOPE.into(),
            "c2".into(),
            second_cancel.clone(),
            "t2".into(),
            None,
        )
        .unwrap();
        state
            .permissions
            .lock()
            .unwrap()
            .get(&("first".into(), "q".into()))
            .unwrap()
            .sender
            .try_send(PermissionResponse {
                action: PermissionAction::Other,
                instruction: Some("n60".into()),
                timed_out: false,
            })
            .unwrap();
        let PermissionWaitResult::Decision(answer) =
            wait_for_permission(&state, "first", "q".into(), first_cancel, first_rx, None)
                .await
                .unwrap();
        assert_eq!(question_result(answer)["answer"], "n60");
        assert!(state
            .permissions
            .lock()
            .unwrap()
            .contains_key(&("second".into(), "q".into())));
        second_cancel.store(true, Ordering::Relaxed);
        assert!(
            wait_for_permission(&state, "second", "q".into(), second_cancel, second_rx, None)
                .await
                .is_err()
        );
        assert!(state.permissions.lock().unwrap().is_empty());
        assert!(state.session_approvals.lock().unwrap().is_empty());
        assert!(state.one_time_approvals.lock().unwrap().is_empty());
        drop((first, second));
    }

    /// 回归：询问的身份必须来自「当前活跃请求」，不能用每轮工具调用的取消令牌。
    /// 生产环境曾把工具级令牌交给 `AskUserTool`，而 `register_pending_permission`
    /// 比较的是令牌身份，于是每次询问都在几十毫秒内以「请求已取消」失败。
    #[tokio::test]
    async fn question_identity_comes_from_the_live_request_not_a_tool_token() {
        let state = AgentRuntimeState::default();
        let guard = AgentRequestGuard::start(&state, "session".into(), "turn".into()).unwrap();
        let tool_token = Arc::new(AtomicBool::new(false));
        let failure = register_pending_permission(
            &state,
            "session",
            "question-tool".into(),
            QUESTION_SCOPE.into(),
            "c1".into(),
            tool_token,
            "turn".into(),
            None,
        )
        .err()
        .expect("工具级取消令牌必须被拒绝");
        assert_eq!(failure, "请求已取消");
        assert!(state.permissions.lock().unwrap().is_empty());

        let cancellation = agent_core::request_cancellation(&state, "session", "turn").unwrap();
        let receiver = register_pending_permission(
            &state,
            "session",
            "question-request".into(),
            QUESTION_SCOPE.into(),
            "c2".into(),
            cancellation.clone(),
            "turn".into(),
            None,
        )
        .unwrap();
        state
            .permissions
            .lock()
            .unwrap()
            .get(&("session".into(), "question-request".into()))
            .unwrap()
            .sender
            .try_send(PermissionResponse {
                action: PermissionAction::Other,
                instruction: Some("n60-userdebug".into()),
                timed_out: false,
            })
            .unwrap();
        let PermissionWaitResult::Decision(answer) = wait_for_permission(
            &state,
            "session",
            "question-request".into(),
            cancellation,
            receiver,
            question_timeout(),
        )
        .await
        .unwrap();
        let answered = question_result(answer);
        assert_eq!(answered["status"], "answered");
        assert_eq!(answered["answer"], "n60-userdebug");
        assert!(state.permissions.lock().unwrap().is_empty());
        drop(guard);
    }
}
