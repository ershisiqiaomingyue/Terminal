use super::*;

/// Protocol adapter shared by desktop and future non-Tauri transports. Keep
/// the legacy commands while callers migrate their control-plane operations.
#[tauri::command]
pub async fn execute_conversation_command(
    app: AppHandle,
    command: ConversationCommand,
) -> Result<Value, String> {
    match command {
        ConversationCommand::ThreadRead {
            thread_id,
            after_revision,
        } => {
            let state = app.state::<AgentRuntimeState>();
            serde_json::to_value(state.read_thread_snapshot_after(&thread_id, after_revision))
                .map_err(|error| error.to_string())
        }
        ConversationCommand::TurnInterrupt {
            thread_id,
            turn_id,
            source,
        } => {
            let state = app.state::<AgentRuntimeState>();
            let result =
                cancel_agent_request(app.clone(), state, thread_id, Some(turn_id), source).await?;
            serde_json::to_value(result).map_err(|error| error.to_string())
        }
        ConversationCommand::PermissionResolve {
            thread_id,
            request_id,
            action,
            instruction,
        } => {
            let state = app.state::<AgentRuntimeState>();
            resolve_agent_permission(
                app.clone(),
                state,
                thread_id,
                request_id,
                action,
                instruction,
            )?;
            Ok(Value::Null)
        }
        ConversationCommand::TurnStart {
            thread_id,
            turn_id,
            request,
        } => {
            let mut request = serde_json::from_value::<AgentChatRequest>(request)
                .map_err(|error| format!("启动请求无效: {error}"))?;
            if request
                .session_id
                .as_deref()
                .is_some_and(|value| value != thread_id)
                || request
                    .turn_id
                    .as_deref()
                    .is_some_and(|value| value != turn_id)
            {
                return Err("启动请求的会话或轮次与命令不一致".to_string());
            }
            request.session_id = Some(thread_id.clone());
            request.turn_id = Some(turn_id.clone());
            // N8：受理降级。前端以本地 sending 判断 start/queue，但后台汇报轮或
            // 协作轮可能先被 admit（TOCTOU）。撞上并发时自动转入持久队列而不是
            // 报错退稿（codex StartOrSteer 的「忙则排队」半边）。
            let state = app.state::<AgentRuntimeState>();
            let session_busy = state
                .requests
                .lock()
                .map(|requests| requests.contains_key(&thread_id))
                .unwrap_or(false);
            if session_busy {
                let queued = enqueue_agent_message(app.clone(), state, request).await?;
                return Ok(json!({
                    "turn_id": Value::Null,
                    "status": "queued",
                    "queue_id": queued.queue_id,
                }));
            }
            let (start_ack, admitted) = tokio::sync::oneshot::channel();
            // N9：早退失败（如历史上下文加载失败）发生在 run 行创建之前，合成
            // 失败快照的 finish 会因缺行落空。失败分支先用原请求兜底建行——
            // start_agent_run 对已存在的 turn_id 会报错，恰好幂等——保证任何
            // 启动失败路径都有 durable 终态记录。
            let audit_request = request.clone();
            let receipt_turn_id = turn_id.clone();
            // A turn belongs to the native runtime, not to the webview invoke
            // that starts it. Keeping this task detached makes navigation and
            // component unmounts unable to abort a running model stream.
            tauri::async_runtime::spawn(async move {
                let session_id = request_session_id(&request);
                let message = request.message.clone();
                let start_failure_baseline_revision = app
                    .state::<AgentRuntimeState>()
                    .read_thread_snapshot(&session_id)
                    .map(|snapshot| snapshot.revision);
                if let Err(error) = super::queue::execute_agent_request_with_admission(
                    app.clone(),
                    request,
                    None,
                    None,
                    Some(start_ack),
                )
                .await
                {
                    eprintln!("Agent 请求执行失败: {error}");
                    let _ = crate::history::start_agent_run(
                        &app,
                        &session_id,
                        &turn_id,
                        &audit_request,
                    );
                    emit_start_failure_snapshot(
                        &app,
                        &session_id,
                        &turn_id,
                        &message,
                        &error,
                        start_failure_baseline_revision,
                    );
                }
                dispatch_queued_agent_request(&app, &session_id);
            });
            admitted
                .await
                .map_err(|_| "Agent 启动未完成，请检查任务错误。".to_string())?;
            // N10：回执带 turn_id 与受理状态（对齐 codex TurnStartResponse）；
            // 数据本体走事件流，不再返回恒为空的占位字段。
            Ok(json!({ "turn_id": receipt_turn_id, "status": "started" }))
        }
        ConversationCommand::TurnEnqueue { thread_id, request } => {
            let mut request = serde_json::from_value::<AgentChatRequest>(request)
                .map_err(|error| format!("排队请求无效: {error}"))?;
            if request
                .session_id
                .as_deref()
                .is_some_and(|value| value != thread_id)
            {
                return Err("排队请求的会话与命令不一致".to_string());
            }
            request.session_id = Some(thread_id.clone());
            let state = app.state::<AgentRuntimeState>();
            let result = enqueue_agent_message(app.clone(), state, request).await?;
            serde_json::to_value(result).map_err(|error| error.to_string())
        }
        ConversationCommand::TurnQueueUpdate {
            thread_id,
            queue_id,
            content,
        } => {
            let content = content.trim().to_string();
            if content.is_empty() {
                return Err("待发送消息不能为空".to_string());
            }
            if !crate::history::update_agent_queue_content(
                app.clone(),
                thread_id.clone(),
                queue_id,
                content,
            )? {
                return Err("待发送消息不存在或已被调度器领取".to_string());
            }
            let state = app.state::<AgentRuntimeState>();
            let snapshot = hydrate_agent_queue_session(&app, &state, &thread_id)?;
            serde_json::to_value(snapshot).map_err(|error| error.to_string())
        }
        ConversationCommand::TurnQueueReorder {
            thread_id,
            queue_ids,
        } => {
            crate::history::reorder_agent_queue_items(&app, &thread_id, &queue_ids)?;
            let state = app.state::<AgentRuntimeState>();
            let snapshot = match state.reorder_queued_agent_requests(&thread_id, &queue_ids) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    let _ = hydrate_agent_queue_session(&app, &state, &thread_id);
                    return Err(error);
                }
            };
            let _ = app.emit("agent:thread_snapshot", snapshot.clone());
            serde_json::to_value(snapshot).map_err(|error| error.to_string())
        }
        ConversationCommand::TurnQueueGuide {
            thread_id,
            queue_id,
        } => {
            let state = app.state::<AgentRuntimeState>();
            let owner = crate::history::agent_queue_lease_owner();
            let lease =
                crate::history::claim_agent_queue_item_by_id(&app, &thread_id, &queue_id, owner)?
                    .ok_or_else(|| "待发送消息不存在或已被处理".to_string())?;
            let (claim, snapshot) = match state.claim_queued_agent_request(
                &thread_id,
                &queue_id,
                lease.lease_id.clone(),
            ) {
                Ok(Some(value)) => value,
                Ok(None) | Err(_) => {
                    let _ = crate::history::nack_agent_queue_item(
                        &app,
                        &thread_id,
                        &queue_id,
                        &lease.lease_id,
                        &lease.lease_owner,
                        Some("内存队列与持久队列不同步"),
                    );
                    let _ = hydrate_agent_queue_session(&app, &state, &thread_id);
                    return Err("待发送消息已被调度器领取".to_string());
                }
            };
            let _ = app.emit("agent:thread_snapshot", snapshot);
            let active_turn_id = match active_agent_turn_for_steer(&state, &thread_id, None) {
                Ok(turn_id) => turn_id,
                Err(error) => {
                    let _ = crate::history::nack_agent_queue_item(
                        &app,
                        &thread_id,
                        &queue_id,
                        &lease.lease_id,
                        &lease.lease_owner,
                        Some(&error),
                    );
                    let _ = state.nack_agent_queue_claim(&thread_id, &claim.id, &claim.lease_id);
                    return Err(error);
                }
            };
            let steer = AgentSteerRequest {
                message: lease.item.request.message.trim().to_string(),
                attachments: lease.item.request.attachments.clone(),
                artifacts: lease.item.request.artifacts.clone(),
                source_message_id: None,
            };
            let persisted = match crate::history::persist_queue_steer_and_ack(
                &app,
                &thread_id,
                &active_turn_id,
                &steer,
                &queue_id,
                &lease.lease_id,
                &lease.lease_owner,
            ) {
                Ok(persisted) => persisted,
                Err(error) => {
                    let _ = crate::history::nack_agent_queue_item(
                        &app,
                        &thread_id,
                        &queue_id,
                        &lease.lease_id,
                        &lease.lease_owner,
                        Some(&error),
                    );
                    if let Ok(snapshot) = app.state::<AgentRuntimeState>().nack_agent_queue_claim(
                        &thread_id,
                        &claim.id,
                        &claim.lease_id,
                    ) {
                        let _ = app.emit("agent:thread_snapshot", snapshot);
                    }
                    return Err(error);
                }
            };
            if let Ok(snapshot) = app.state::<AgentRuntimeState>().ack_agent_queue_claim(
                &thread_id,
                &claim.id,
                &claim.lease_id,
            ) {
                let _ = app.emit("agent:thread_snapshot", snapshot);
            } else if let Err(error) = hydrate_agent_queue_session(&app, &state, &thread_id) {
                eprintln!("无法在引导队列消息后重建内存队列: {error}");
            }
            let steer_message_id = format!("user:{active_turn_id}:steer:{}", persisted.id);
            schedule_persisted_steer_delivery(&app, &state, persisted);
            emit_thread_snapshot_event(
                &app,
                &state,
                ThreadEvent::ItemAdded {
                    thread_id: thread_id.clone(),
                    turn_id: active_turn_id.clone(),
                    item: user_message_item(
                        steer_message_id,
                        steer.message.clone(),
                        &steer.attachments,
                        &steer.artifacts,
                        lease.item.request.workspace_path.as_deref(),
                    ),
                },
                None,
            );
            Ok(Value::Null)
        }
        ConversationCommand::TurnQueueRemove {
            thread_id,
            queue_id,
        } => {
            let state = app.state::<AgentRuntimeState>();
            let owner = crate::history::agent_queue_lease_owner();
            let lease =
                crate::history::claim_agent_queue_item_by_id(&app, &thread_id, &queue_id, owner)?
                    .ok_or_else(|| "待发送消息不存在或已被处理".to_string())?;
            let (claim, snapshot) = match state.claim_queued_agent_request(
                &thread_id,
                &queue_id,
                lease.lease_id.clone(),
            ) {
                Ok(Some(value)) => value,
                Ok(None) | Err(_) => {
                    let _ = crate::history::nack_agent_queue_item(
                        &app,
                        &thread_id,
                        &queue_id,
                        &lease.lease_id,
                        &lease.lease_owner,
                        Some("内存队列与持久队列不同步"),
                    );
                    let _ = hydrate_agent_queue_session(&app, &state, &thread_id);
                    return Err("待发送消息已被调度器领取".to_string());
                }
            };
            let _ = app.emit("agent:thread_snapshot", snapshot);
            if let Err(error) = crate::history::ack_agent_queue_item(
                &app,
                &thread_id,
                &queue_id,
                &lease.lease_id,
                &lease.lease_owner,
                "removed",
            ) {
                let _ = state.nack_agent_queue_claim(&thread_id, &claim.id, &claim.lease_id);
                let _ = crate::history::nack_agent_queue_item(
                    &app,
                    &thread_id,
                    &queue_id,
                    &lease.lease_id,
                    &lease.lease_owner,
                    Some(&error),
                );
                return Err(error);
            }
            if let Ok(snapshot) =
                state.ack_agent_queue_claim(&thread_id, &claim.id, &claim.lease_id)
            {
                let _ = app.emit("agent:thread_snapshot", snapshot);
            } else if let Err(error) = hydrate_agent_queue_session(&app, &state, &thread_id) {
                eprintln!("无法在移除队列消息后重建内存队列: {error}");
            }
            Ok(Value::Null)
        }
        ConversationCommand::TurnResume {
            thread_id,
            turn_id,
            resume_turn_id,
            permission_mode,
        } => {
            let resumed_turn_id =
                resume_turn_id.unwrap_or_else(|| format!("resume-{}-{}", turn_id, uuid_suffix()));
            if resumed_turn_id.trim().is_empty() || resumed_turn_id == turn_id {
                return Err("恢复任务必须使用新的轮次标识".to_string());
            }
            let (start_ack, admitted) = tokio::sync::oneshot::channel();
            let receipt_turn_id = resumed_turn_id.clone();
            tauri::async_runtime::spawn(async move {
                let session_id = thread_id.clone();
                let start_failure_baseline_revision = app
                    .state::<AgentRuntimeState>()
                    .read_thread_snapshot(&session_id)
                    .map(|snapshot| snapshot.revision);
                if let Err(error) = super::queue::resume_agent_run_with_start_ack(
                    app.clone(),
                    thread_id,
                    turn_id,
                    Some(resumed_turn_id.clone()),
                    permission_mode,
                    Some(start_ack),
                )
                .await
                {
                    eprintln!("恢复 Agent 请求失败: {error}");
                    emit_start_failure_snapshot(
                        &app,
                        &session_id,
                        &resumed_turn_id,
                        "继续中断任务",
                        &error,
                        start_failure_baseline_revision,
                    );
                }
            });
            admitted
                .await
                .map_err(|_| "Agent 恢复未完成，请检查任务错误。".to_string())?;
            // N10：恢复回执同样带 turn_id 与状态。
            Ok(json!({ "turn_id": receipt_turn_id, "status": "started" }))
        }
        ConversationCommand::TurnSteer {
            thread_id,
            turn_id,
            request,
        } => {
            let request = serde_json::from_value::<AgentSteerRequest>(request)
                .map_err(|error| format!("续发请求无效: {error}"))?;
            let state = app.state::<AgentRuntimeState>();
            let receipt = steer_agent_request(
                app.clone(),
                state,
                thread_id,
                request.message,
                request.attachments,
                request.artifacts,
                Some(turn_id),
            )
            .await?;
            Ok(receipt)
        }
    }
}

#[tauri::command]
pub async fn steer_agent_request(
    app: AppHandle,
    state: State<'_, AgentRuntimeState>,
    session_id: String,
    message: String,
    attachments: Vec<AgentImageAttachment>,
    artifacts: Vec<LocalArtifact>,
    turn_id: Option<String>,
) -> Result<Value, String> {
    persist_steer_agent_request(
        app,
        state,
        session_id,
        message,
        attachments,
        artifacts,
        turn_id,
    )
    .await
}

pub(super) async fn persist_steer_agent_request(
    app: AppHandle,
    state: State<'_, AgentRuntimeState>,
    session_id: String,
    message: String,
    attachments: Vec<AgentImageAttachment>,
    artifacts: Vec<LocalArtifact>,
    turn_id: Option<String>,
) -> Result<Value, String> {
    let message = message.trim().to_string();
    if message.is_empty() {
        return Err("追加要求不能为空".to_string());
    }
    let active_turn_id = active_agent_turn_for_steer(&state, &session_id, turn_id.as_deref())?;
    let steer = AgentSteerRequest {
        message: message.clone(),
        attachments,
        artifacts,
        source_message_id: None,
    };
    // 落库要先拿进程级 history 写锁再开一次 SQLite，而写锁会被工具事件持久化
    // 长时间占用。直接在 async command 里同步做，会把 tokio worker 卡住，前端
    // 连「追加气泡」都要等这条 insert 返回才出现。挪到阻塞线程池里排队，
    // 只让这一次 await 等待，不再拖住整个运行时。
    let persisted = {
        let app = app.clone();
        let session_id = session_id.clone();
        let active_turn_id = active_turn_id.clone();
        let steer = steer.clone();
        tauri::async_runtime::spawn_blocking(move || {
            crate::history::enqueue_agent_steer(&app, &session_id, &active_turn_id, &steer)
        })
        .await
        .map_err(|error| format!("追加要求保存失败: {error}"))??
    };
    // P3-A/P3-B：回执与投递同生共死。先投递，按投递结果决定回执——
    // 轮次已终止时绝不发 steer_accepted / 用户气泡（旧实现无条件先发回执
    // 再投递，正是「显示已发送但永不生效」的假回执根因）。
    let accepted_steer_id = persisted.id.clone();
    let delivery = schedule_persisted_steer_delivery(&app, &state, persisted);
    if let SteerDeliveryOutcome::TurnEnded(reason) = delivery {
        return Err(reason);
    }
    let pending_replay = matches!(delivery, SteerDeliveryOutcome::PendingReplay);
    let steer_message_id = format!("user:{active_turn_id}:steer:{accepted_steer_id}");
    // 追加要求不会打断当前轮（与追加消息的 steer 语义一致），它要等到模型下一次
    // 步骤边界、pending_calls 清空后才会被 apply，此时才发出 "steered" 事件。
    // 一条长时间运行的远程命令会让 pending_calls 长期非空，用户点完追加后在
    // 界面上完全看不到变化——所以在「投递成功」的瞬间补一条回执，让界面立刻
    // 显示「已收到，将在当前步骤结束后生效」。
    emit_agent_payload_for_turn(
        &app,
        json!({
            "type": "steer_accepted",
            "session_id": session_id,
            "event_id": format!("steer-accepted:{}", accepted_steer_id),
            "message": message,
            "steer_id": accepted_steer_id,
            "turn_id": active_turn_id,
            "pending_replay": pending_replay,
        }),
        &active_turn_id,
    );
    emit_thread_snapshot_event(
        &app,
        &state,
        ThreadEvent::ItemAdded {
            thread_id: session_id,
            turn_id: active_turn_id.clone(),
            item: user_message_item(
                steer_message_id,
                message,
                &steer.attachments,
                &steer.artifacts,
                None,
            ),
        },
        None,
    );
    Ok(json!({
        "turn_id": active_turn_id,
        "steer_id": accepted_steer_id,
        "status": if pending_replay { "pending_replay" } else { "steered" },
    }))
}

pub(super) fn active_agent_turn_for_steer(
    state: &AgentRuntimeState,
    session_id: &str,
    requested_turn_id: Option<&str>,
) -> Result<String, String> {
    {
        let requests = state
            .requests
            .lock()
            .map_err(|_| "请求状态不可用".to_string())?;
        let control = requests
            .get(session_id)
            .ok_or_else(|| "当前 Agent 已结束，无法追加要求".to_string())?;
        if requested_turn_id.is_some_and(|requested| requested != control.turn_id) {
            return Err("当前 Agent 已切换，无法追加要求".to_string());
        }
        if control.cancellation.load(Ordering::Relaxed) {
            return Err("当前 Agent 正在停止，无法追加要求".to_string());
        }
        Ok(control.turn_id.clone())
    }
}

/// P3-A：投递结果决定回执语义。
pub(super) enum SteerDeliveryOutcome {
    /// 已进入当前轮的 steer 通道，行已标 delivered。
    Delivered,
    /// 渠道瞬时打满或轮次中断待恢复：行保持 pending，由本轮步骤边界、下一轮
    /// 或 resume 重放投递——回执如实标注 pending_replay。
    PendingReplay,
    /// 轮次已终止且行已回收——回执必须报错，绝不发「已受理」。
    TurnEnded(String),
}

pub(super) fn schedule_persisted_steer_delivery(
    app: &AppHandle,
    state: &AgentRuntimeState,
    steer: crate::history::PersistedAgentSteer,
) -> SteerDeliveryOutcome {
    let sender = active_steer_sender(state, &steer.session_id, &steer.turn_id);
    let Some(sender) = sender else {
        // The sender can disappear between the active-turn check and the
        // durable insert. Do not leave a steer attached forever to a turn
        // that has already reached a terminal state; interrupted runs stay
        // pending and will be replayed by resume_agent_run.
        return match crate::history::reject_agent_steer_if_terminal(app, &steer.id) {
            Ok(true) => {
                SteerDeliveryOutcome::TurnEnded("当前轮次已结束，追加要求未送达。".to_string())
            }
            Ok(false) => SteerDeliveryOutcome::PendingReplay,
            Err(error) => {
                eprintln!("无法回收已结束轮次的追加要求: {error}");
                SteerDeliveryOutcome::PendingReplay
            }
        };
    };
    let mut request = steer.request.clone();
    request.source_message_id = Some(format!("user:{}:steer:{}", steer.turn_id, steer.id));
    match sender.try_send(request) {
        Ok(()) => {
            if let Err(error) =
                crate::history::update_agent_steer_status(app, &steer.id, "delivered")
            {
                eprintln!("无法确认追加要求已投递: {error}");
            }
            SteerDeliveryOutcome::Delivered
        }
        // A full or closed channel leaves the row pending. The next run will
        // replay it in creation order instead of dropping an accepted message.
        Err(mpsc::error::TrySendError::Full(_)) => SteerDeliveryOutcome::PendingReplay,
        Err(mpsc::error::TrySendError::Closed(_)) => {
            match crate::history::reject_agent_steer_if_terminal(app, &steer.id) {
                Ok(true) => {
                    SteerDeliveryOutcome::TurnEnded("当前轮次已结束，追加要求未送达。".to_string())
                }
                outcome => {
                    if let Err(error) = outcome {
                        eprintln!("无法回收已结束轮次的追加要求: {error}");
                    }
                    SteerDeliveryOutcome::PendingReplay
                }
            }
        }
    }
}

pub(super) fn active_steer_sender(
    state: &AgentRuntimeState,
    session_id: &str,
    turn_id: &str,
) -> Option<mpsc::Sender<AgentSteerRequest>> {
    state.requests.lock().ok().and_then(|requests| {
        requests.get(session_id).and_then(|control| {
            (control.turn_id == turn_id && !control.cancellation.load(Ordering::Relaxed))
                .then(|| control.steer_sender.clone())
        })
    })
}

pub(super) fn replay_pending_agent_steers(
    app: &AppHandle,
    state: &AgentRuntimeState,
    session_id: &str,
) -> Result<(), String> {
    for steer in crate::history::pending_agent_steers(app, session_id)? {
        schedule_persisted_steer_delivery(app, state, steer);
    }
    Ok(())
}

pub(super) fn mark_agent_steer_applied(
    app: &AppHandle,
    session_id: &str,
    steer: &AgentSteerRequest,
) -> bool {
    match crate::history::mark_next_agent_steer_applied(app, session_id, steer) {
        Ok(_) => true,
        Err(error) => {
            eprintln!("无法确认追加要求已应用: {error}");
            false
        }
    }
}

#[tauri::command]
pub fn resolve_agent_permission(
    app: AppHandle,
    state: State<'_, AgentRuntimeState>,
    session_id: String,
    request_id: String,
    decision: String,
    instruction: Option<String>,
) -> Result<(), String> {
    user_interaction::resolve_interaction(
        &app,
        &state,
        session_id,
        request_id,
        decision,
        instruction,
    )
}
