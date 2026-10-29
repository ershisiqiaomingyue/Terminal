use super::execution::{spawn_and_drive_unit, ExecutionUnit, TurnUnit};
use super::*;

#[derive(Debug, Clone)]
pub(super) struct CollaborationAgentAdmission {
    pub(super) job_id: String,
    pub(super) agent_id: String,
    pub(super) attempt: i64,
    pub(super) lease_id: String,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn finish_admitted_agent_turn(
    app: &AppHandle,
    admission: Option<&CollaborationAgentAdmission>,
    session_id: &str,
    turn_id: &str,
    status: &str,
    message: Option<&str>,
    assistant_content: Option<&str>,
    summary: Option<&str>,
    error: Option<&str>,
) -> Result<(), String> {
    if let Some(admission) = admission {
        crate::history::finish_collaboration_agent_turn(
            app,
            &admission.job_id,
            &admission.agent_id,
            admission.attempt,
            &admission.lease_id,
            session_id,
            turn_id,
            status,
            message,
            assistant_content,
            summary,
            error,
        )
        .map(|_| ())
    } else {
        crate::history::finish_agent_turn(
            app,
            session_id,
            turn_id,
            status,
            message,
            assistant_content,
        )
    }
}

/// N12：终态落库统一重试档位。三次尝试（200ms 间隔）仍失败时记录 diagnostics
/// 事件并返回 Err——调用方据此在终态 payload 上带 `terminal_persistence_failed`
/// 降级标记（与 `emit_durable_terminal_payload` 的同名标记同档）。此前非协作
/// 路径只 eprintln 就继续发 completed payload，run 停在非终态，是 N6 pending
/// steer 悬挂与重启误报 interrupted 的直接放大器。协作路径维持升级为 Err 的旧行为。
async fn finish_admitted_agent_turn_with_retry(
    app: &AppHandle,
    admission: Option<&CollaborationAgentAdmission>,
    session_id: &str,
    turn_id: &str,
    status: &str,
    message: Option<&str>,
    assistant_content: Option<&str>,
    summary: Option<&str>,
    error: Option<&str>,
) -> Result<(), String> {
    let mut last_error = String::from("未知错误");
    for attempt in 0..3 {
        match finish_admitted_agent_turn(
            app,
            admission,
            session_id,
            turn_id,
            status,
            message,
            assistant_content,
            summary,
            error,
        ) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = error;
                if attempt < 2 {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            }
        }
    }
    crate::diagnostics::record(
        "error",
        "agent.turn_terminal_persist",
        "native.agent",
        json!({
            "session_id": session_id,
            "turn_id": turn_id,
            "status": status,
            "attempts": 3,
            "error": last_error,
        }),
    );
    Err(last_error)
}

/// N6-B：turn 终态落库成功后，统一回收该轮所有未应用（pending/delivered）
/// 的追加要求，并逐条持久化+广播 `steer_rejected`——UI 把对应的「已受理」
/// 回执标成「未送达」。此前这些行要等下一轮 replay 才被逐条回收，用户停止
/// 后不再发消息就会永久悬挂。只在 run 已是终态时调用（finish 成功后）。
fn settle_pending_agent_steers(app: &AppHandle, session_id: &str, turn_id: &str) {
    match crate::history::reject_agent_steers_for_turn(app, session_id, turn_id) {
        Ok(steer_ids) => {
            for steer_id in steer_ids {
                emit_agent_payload_for_turn(
                    app,
                    json!({
                        "type": "steer_rejected",
                        "session_id": session_id,
                        "turn_id": turn_id,
                        "steer_id": steer_id,
                    }),
                    turn_id,
                );
            }
        }
        Err(error) => {
            eprintln!("无法回收已结束轮次的追加要求: {error}");
        }
    }
}

/// N6-B（缺口补齐）：终态落库失败时的降级回收。
///
/// `settle_pending_agent_steers` 依赖 `reject_agent_steers_for_turn` 落库成功；一旦
/// 历史库写不进去（`terminal_persistence_failed`），就不能再走那条路径（会一起失败）。
/// 但「本轮已结束」这件事对用户是确定的——前端那条「已收到追加要求…」必须收尾，
/// 否则会悬挂到下一轮 replay。因此这里只做**广播**：把该轮所有 pending/delivered
/// 的 steer 逐条发 `steer_rejected`，库里保持原样，等重启后的
/// `reconcile_interrupted_agent_turns` 兜底收口（见 N12-A）。
fn rebroadcast_pending_agent_steers(app: &AppHandle, session_id: &str, turn_id: &str) {
    match crate::history::pending_agent_steer_ids_for_turn(app, session_id, turn_id) {
        Ok(steer_ids) => {
            for steer_id in steer_ids {
                emit_agent_payload_for_turn(
                    app,
                    json!({
                        "type": "steer_rejected",
                        "session_id": session_id,
                        "turn_id": turn_id,
                        "steer_id": steer_id,
                    }),
                    turn_id,
                );
            }
        }
        Err(error) => {
            eprintln!("无法读取待回收的追加要求: {error}");
        }
    }
}

pub(super) async fn execute_agent_request(
    app: AppHandle,
    request: AgentChatRequest,
) -> Result<AgentChatResponse, String> {
    execute_agent_request_with_admission(app, request, None, None, None).await
}

/// 把回合任务的硬中止句柄登记进控制表（P1-C 兜底）。只匹配当前活跃的
/// 回合身份（turn + 取消标志指针），绝不覆盖新一轮请求的句柄。
pub(super) fn register_turn_task_for_hard_abort(
    state: &AgentRuntimeState,
    session_id: &str,
    turn_id: &str,
    cancellation: &Arc<AtomicBool>,
    handle: tokio::task::AbortHandle,
) {
    if let Ok(mut requests) = state.requests.lock() {
        if let Some(control) = requests.get_mut(session_id) {
            if control.turn_id == turn_id && Arc::ptr_eq(&control.cancellation, cancellation) {
                control.set_task_abort_handle(handle);
            }
        }
    }
}

pub(super) fn merge_history_and_request_context(
    mut history: Vec<AgentContextTurn>,
    context: Vec<AgentContextTurn>,
) -> Vec<AgentContextTurn> {
    history.extend(context);
    history
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_collaboration_agent_request(
    app: AppHandle,
    request: AgentChatRequest,
    job_id: String,
    agent_id: String,
    attempt: i64,
    lease_id: String,
) -> Result<AgentChatResponse, String> {
    execute_agent_request_with_admission(
        app,
        request,
        None,
        Some(CollaborationAgentAdmission {
            job_id,
            agent_id,
            attempt,
            lease_id,
        }),
        None,
    )
    .await
}

pub(super) async fn execute_agent_request_with_admission(
    app: AppHandle,
    mut request: AgentChatRequest,
    queue_admission: Option<AgentQueueAdmission>,
    collaboration_admission: Option<CollaborationAgentAdmission>,
    start_ack: Option<tokio::sync::oneshot::Sender<()>>,
) -> Result<AgentChatResponse, String> {
    if queue_admission.is_some() && collaboration_admission.is_some() {
        return Err("Agent 请求不能同时来自队列和协作调度".to_string());
    }
    // Normalize before publishing/persisting the target, including old queued
    // requests which inherited global SSH for a Windows project.
    agent_core::normalize_local_project_target(&mut request, None);
    if !request
        .context
        .iter()
        .any(|turn| turn.source.as_deref() == Some("history"))
    {
        if let Some(session_id) = request
            .session_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            let history = crate::history::agent_context_for_session(&app, session_id)?;
            request.context = merge_history_and_request_context(history, request.context);
        }
    }
    let state = app.state::<AgentRuntimeState>();
    let session_id = request_session_id(&request);
    let turn_id = request
        .turn_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("turn-{}", uuid_suffix()));
    // Reserve the session before publishing a TurnStarted snapshot. Otherwise
    // a concurrent start can replace a live turn's snapshot before its own
    // request is rejected. The guard must be owned ('static) because the turn
    // body below now runs in a spawned task that can outlive this borrow
    // (P1-C：回合主体独立成任务，守卫需要随之移动)。
    let request_guard =
        match AgentRequestGuard::start_owned(&state, session_id.clone(), turn_id.clone()) {
            Ok(guard) => guard,
            Err(error) => {
                if let Some(admission) = queue_admission.as_ref() {
                    let _ = crate::history::nack_agent_queue_item(
                        &app,
                        &session_id,
                        &admission.queue_id,
                        &admission.lease_id,
                        &admission.lease_owner,
                        Some(&error),
                    );
                    if let Ok(snapshot) = state.nack_agent_queue_claim(
                        &session_id,
                        &admission.queue_id,
                        &admission.lease_id,
                    ) {
                        let _ = app.emit("agent:thread_snapshot", snapshot);
                    }
                }
                return Err(error);
            }
        };
    if let Some(admission) = queue_admission.as_ref() {
        if let Err(error) = crate::history::start_agent_run_from_queue(
            &app,
            &session_id,
            &turn_id,
            &request,
            &admission.queue_id,
            &admission.lease_id,
            &admission.lease_owner,
        ) {
            let _ = crate::history::nack_agent_queue_item(
                &app,
                &session_id,
                &admission.queue_id,
                &admission.lease_id,
                &admission.lease_owner,
                Some(&error),
            );
            if let Ok(snapshot) =
                state.nack_agent_queue_claim(&session_id, &admission.queue_id, &admission.lease_id)
            {
                let _ = app.emit("agent:thread_snapshot", snapshot);
            }
            drop(request_guard);
            return Err(error);
        }
        match state.ack_agent_queue_claim(&session_id, &admission.queue_id, &admission.lease_id) {
            Ok(snapshot) => {
                let _ = app.emit("agent:thread_snapshot", snapshot);
            }
            Err(error) => {
                // SQLite has already committed `dispatched`; do not resurrect
                // the request. Repair the cache immediately so the following
                // durable item can still be admitted without another trigger.
                eprintln!("无法确认内存 Agent 队列 lease: {error}");
                if let Err(hydration_error) = hydrate_agent_queue_session(&app, &state, &session_id)
                {
                    eprintln!("无法在确认 Agent 队列 lease 后重建内存队列: {hydration_error}");
                }
            }
        }
    } else if let Some(admission) = collaboration_admission.as_ref() {
        if let Err(error) = crate::history::start_collaboration_agent_run(
            &app,
            &admission.job_id,
            &admission.agent_id,
            admission.attempt,
            &admission.lease_id,
            &session_id,
            &turn_id,
            &request,
        ) {
            drop(request_guard);
            return Err(error);
        }
        // G5-A：协作 worker 的控制块标记为 Subagent 单元，并带上 job 关联的
        // goal——预算回写与诊断以此为锚点，不再反查 job 表。
        let job_goal = crate::history::collaboration_job_goal_id(&app, &admission.job_id)
            .ok()
            .flatten();
        if let Ok(requests) = state.requests.lock() {
            if let Some(control) = requests.get(&session_id) {
                control.describe_unit(agent_core::UnitKind::Subagent, job_goal);
            }
        }
    } else {
        crate::history::start_agent_run(&app, &session_id, &turn_id, &request)?;
    }
    if let Err(error) = replay_pending_agent_steers(&app, &state, &session_id) {
        eprintln!("无法重放持久化追加要求: {error}");
    }
    begin_agent_task(&app, &session_id, &turn_id, &request.message);
    // G4-A：UserPromptSubmit 通知——输入被受理后触发（无决策语义）。
    super::hooks::run_user_prompt_submit_hooks(&session_id, &turn_id, &request.message).await;
    emit_thread_snapshot_event(
        &app,
        &state,
        ThreadEvent::TurnStarted {
            thread_id: session_id.clone(),
            turn_id: turn_id.clone(),
        },
        None,
    );
    if !turn_id.starts_with(agent_core::background::BACKGROUND_RESULT_PREFIX) {
        emit_thread_snapshot_event(
            &app,
            &state,
            ThreadEvent::ExecutionTargetUpdated {
                thread_id: session_id.clone(),
                turn_id: turn_id.clone(),
                execution_target: request
                    .execution_target
                    .as_ref()
                    .and_then(|target| serde_json::to_value(target).ok()),
            },
            None,
        );
        emit_thread_snapshot_event(
            &app,
            &state,
            ThreadEvent::ItemAdded {
                thread_id: session_id.clone(),
                turn_id: turn_id.clone(),
                item: user_message_item(
                    format!("user:{turn_id}"),
                    request.message.clone(),
                    &request.attachments,
                    &request.artifacts,
                    request.workspace_path.as_deref(),
                ),
            },
            None,
        );
    }
    emit_thread_snapshot_event(
        &app,
        &state,
        ThreadEvent::ItemAdded {
            thread_id: session_id.clone(),
            turn_id: turn_id.clone(),
            item: ThreadItem::AgentMessage {
                id: format!("assistant:{turn_id}"),
                content: String::new(),
                pending: true,
            },
        },
        None,
    );
    // Acknowledge after admission and durable start, while the model task stays
    // detached. Stop/steer can now address an existing request immediately.
    if let Some(start_ack) = start_ack {
        let _ = start_ack.send(());
    }
    // P1-C（兜底）+ G5-A：回合主体作为 TurnUnit 交给生命周期驱动器
    // （execution.rs）——spawn、硬中止句柄登记、JoinError 映射、on_abort
    // 钩子、工具组收尾全部在驱动器内，queue 只做 admission 层的终态分支。
    // 软取消（标志 + CancellationToken）追不上卡死的等待时，
    // `cancel_agent_request` 会在 1500ms 宽限后硬中止该任务；abort 在这里
    // 表现为 JoinError，映射成与软取消一致的「请求已取消」，由下方既有
    // 分支完成 cancelled 终态落库与事件收尾。正常完成路径行为不变。
    let turn_cancellation = request_guard.cancellation();
    let turn_started_at = std::time::Instant::now();
    let unit: Arc<dyn ExecutionUnit> = Arc::new(TurnUnit::new(
        app.clone(),
        request,
        session_id.clone(),
        turn_id.clone(),
        request_guard,
        turn_cancellation.clone(),
    ));
    let result = spawn_and_drive_unit(&state, unit).await;
    match &result {
        Ok(response) => {
            // G2 预算归属：turn 的计费 token 累计进关联 goal——协作 turn 用
            // job.goal_id（单委托时由工具写入），普通 turn 用会话的活跃 goal
            //（父对话自身的花费也计入，预算才完整）。失败只记日志，绝不影响
            // 回合收尾。
            if let Some(usage) = response.usage.as_ref() {
                let billed = [
                    usage.input_tokens,
                    usage.cached_input_tokens,
                    usage.cache_creation_input_tokens,
                    usage.output_tokens,
                ]
                .into_iter()
                .flatten()
                .fold(0u64, |total, value| total.saturating_add(value));
                if billed > 0 {
                    let goal_id = match collaboration_admission.as_ref() {
                        Some(admission) => {
                            crate::history::collaboration_job_goal_id(&app, &admission.job_id)
                                .ok()
                                .flatten()
                        }
                        None => crate::history::agent_active_goal(&app, &session_id)
                            .ok()
                            .flatten()
                            .map(|goal| goal.id),
                    };
                    if let Some(goal_id) = goal_id {
                        if let Err(error) = crate::history::agent_record_goal_usage(
                            &app,
                            &goal_id,
                            billed as i64,
                            turn_started_at.elapsed().as_secs() as i64,
                        ) {
                            eprintln!("无法累计目标用量: {error}");
                        }
                    }
                }
            }
            let mut payload = json!({
                "type": "completed",
                "session_id": session_id.clone(),
                "text": response.text.clone(),
                "termination_reason": response.termination_reason.clone(),
                "report_error": response.report_error.clone(),
            });
            if let Value::Object(object) = &mut payload {
                if let Some(usage) = response.usage.as_ref() {
                    object.insert(
                        "usage".to_string(),
                        serde_json::to_value(usage).unwrap_or(Value::Null),
                    );
                }
                if let Some(report) = response.report.as_ref() {
                    object.insert(
                        "report".to_string(),
                        serde_json::to_value(report).unwrap_or(Value::Null),
                    );
                }
                if let Some(checkpoint) = response.context_checkpoint.as_ref() {
                    object.insert(
                        "contextCheckpoint".to_string(),
                        serde_json::to_value(checkpoint).unwrap_or(Value::Null),
                    );
                }
            }
            let mut terminal_persistence_failed = false;
            if let Err(error) = finish_admitted_agent_turn_with_retry(
                &app,
                collaboration_admission.as_ref(),
                &session_id,
                &turn_id,
                "completed",
                None,
                Some(&response.text),
                Some(&response.text),
                None,
            )
            .await
            {
                eprintln!("无法持久化 Agent 完成状态: {error}");
                terminal_persistence_failed = true;
                if collaboration_admission.is_some() {
                    let terminal_error = format!("无法原子保存协作 Agent 完成状态: {error}");
                    // P5-C：调用点收敛到收尾的唯一出口（文件末尾），这里只记错。
                    return Err(terminal_error);
                }
            }
            if terminal_persistence_failed {
                if let Value::Object(object) = &mut payload {
                    object.insert("terminal_persistence_failed".to_string(), Value::Bool(true));
                }
            }
            if response.termination_reason != "repetitive_output" {
                emit_thread_snapshot_event(
                    &app,
                    &state,
                    ThreadEvent::AgentFinal {
                        thread_id: session_id.clone(),
                        turn_id: turn_id.clone(),
                        item_id: format!("assistant:{turn_id}"),
                        content: response.text.clone(),
                        preserve_streamed: true,
                    },
                    None,
                );
            }
            emit_durable_terminal_payload(&app, &session_id, &turn_id, payload).await;
            if !terminal_persistence_failed {
                settle_pending_agent_steers(&app, &session_id, &turn_id);
            } else {
                rebroadcast_pending_agent_steers(&app, &session_id, &turn_id);
            }
        }
        Err(error) if error == "请求已取消" => {
            let mut terminal_persistence_failed = false;
            if let Err(error) = finish_admitted_agent_turn_with_retry(
                &app,
                collaboration_admission.as_ref(),
                &session_id,
                &turn_id,
                "cancelled",
                Some("本轮已停止，已保留此前输出和工具记录。"),
                None,
                None,
                Some("请求已取消"),
            )
            .await
            {
                eprintln!("无法持久化 Agent 取消状态: {error}");
                terminal_persistence_failed = true;
            }
            let mut payload = json!({ "type": "cancelled", "session_id": session_id.clone(),
                "termination_reason": "cancelled",
                "message": "本轮已停止，已保留此前输出和工具记录。需要继续时可发送消息。" });
            if terminal_persistence_failed {
                if let Value::Object(object) = &mut payload {
                    object.insert("terminal_persistence_failed".to_string(), Value::Bool(true));
                }
            }
            emit_durable_terminal_payload(&app, &session_id, &turn_id, payload).await;
            if !terminal_persistence_failed {
                settle_pending_agent_steers(&app, &session_id, &turn_id);
            } else {
                rebroadcast_pending_agent_steers(&app, &session_id, &turn_id);
            }
        }
        Err(error) => {
            let structured = AgentCommandError::runtime(error.clone());
            let mut payload = json!({
                "type": "error",
                "session_id": session_id.clone(),
                "message": structured.message,
                "code": structured.code,
                "retryable": structured.retryable,
                "termination_reason": structured.code,
            });
            let mut terminal_persistence_failed = false;
            if let Err(history_error) = finish_admitted_agent_turn_with_retry(
                &app,
                collaboration_admission.as_ref(),
                &session_id,
                &turn_id,
                "failed",
                Some(error),
                Some(error),
                None,
                Some(error),
            )
            .await
            {
                eprintln!("无法持久化 Agent 失败状态: {history_error}");
                terminal_persistence_failed = true;
            }
            if terminal_persistence_failed {
                if let Value::Object(object) = &mut payload {
                    object.insert("terminal_persistence_failed".to_string(), Value::Bool(true));
                }
            }
            emit_durable_terminal_payload(&app, &session_id, &turn_id, payload).await;
            // N6-B：failed 终态与 completed/cancelled 对称——异常失败同样结束了本轮，
            // 该轮的 pending/delivered 追加要求必须一起回收，否则前端「已收到追加要求」
            // 会一直悬挂到下一轮 replay（而用户已看到本轮失败）。
            if !terminal_persistence_failed {
                settle_pending_agent_steers(&app, &session_id, &turn_id);
            } else {
                rebroadcast_pending_agent_steers(&app, &session_id, &turn_id);
            }
        }
    }
    // G4-A：TurnEnd 通知——三个终态分支统一在收尾后触发（无决策语义）。
    let terminal_status = match &result {
        Ok(_) => "completed",
        Err(error) if error == "请求已取消" => "cancelled",
        Err(_) => "failed",
    };
    super::hooks::run_turn_end_hooks(&session_id, &turn_id, terminal_status).await;
    // P5-C：end_agent_task 的唯一调用点。三个终态分支此前各调一次（completed 的
    // 协作失配分支、failed 分支、加上这里的通用收尾），语义重复且容易漏；现在统一
    // 在这里按终态判定一次——成功/取消清空任务，失败带上错误文案。
    let task_error = result
        .as_ref()
        .err()
        .filter(|error| error.as_str() != "请求已取消")
        .map(String::as_str);
    end_agent_task(&app, &session_id, &turn_id, task_error);
    // 走完终态收尾才弹提示：此时历史记录已经落盘，点开提示能直接读回这一轮。
    // 用 `Ok(response).text` 而不是 payload，取消/失败不弹窗（只有真正完成才值得打扰）。
    if let Ok(response) = &result {
        notify_completion(&app, &session_id, Some(response.text.as_str()));
    }
    result
}

pub(crate) fn dispatch_queued_agent_request(app: &AppHandle, session_id: &str) {
    // A message queued while a turn is active cannot be claimed until the
    // request guard is released. Wake the durable queue driver after the
    // turn settles; otherwise the UI's "continue sending" queue would remain
    // pending forever. The in-memory dispatching flag makes this idempotent
    // across the normal completion, cancellation, and recovery call sites.
    let state = app.state::<AgentRuntimeState>();
    if state.dispatch_queued_request(session_id) {
        tauri::async_runtime::spawn(run_agent_queue(app.clone(), session_id.to_string()));
    }
}

pub(super) fn hydrate_agent_queue_session(
    app: &AppHandle,
    state: &AgentRuntimeState,
    session_id: &str,
) -> Result<agent_core::ThreadSnapshot, String> {
    let requests = crate::history::list_agent_queue_items(app, Some(session_id))?
        .into_iter()
        .filter(|item| item.status == "pending")
        .map(|item| (item.id, item.request))
        .collect::<Vec<_>>();
    let snapshot = state.replace_queued_agent_requests(session_id, requests)?;
    let _ = app.emit("agent:thread_snapshot", snapshot.clone());
    Ok(snapshot)
}

pub(crate) fn recover_durable_agent_queue(app: &AppHandle) -> Result<(), String> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = recover_persisted_agent_queue(&app) {
            eprintln!("Agent 待发送恢复失败：{error}");
        }
    });
    Ok(())
}

pub(super) fn recover_persisted_agent_queue(app: &AppHandle) -> Result<(), String> {
    let owner = crate::history::agent_queue_lease_owner();
    let recovered = crate::history::recover_agent_queue_items(app, owner)?;
    let mut sessions = HashMap::<String, Vec<(String, AgentChatRequest)>>::new();
    for item in recovered
        .into_iter()
        .filter(|item| item.status == "pending")
    {
        sessions
            .entry(item.session_id)
            .or_default()
            .push((item.id, item.request));
    }
    let state = app.state::<AgentRuntimeState>();
    let mut session_ids = sessions.keys().cloned().collect::<Vec<_>>();
    session_ids.sort();
    for session_id in &session_ids {
        let requests = sessions.remove(session_id).unwrap_or_default();
        let snapshot = state.replace_queued_agent_requests(session_id, requests)?;
        let _ = app.emit("agent:thread_snapshot", snapshot);
    }
    for session_id in session_ids {
        dispatch_queued_agent_request(app, &session_id);
    }
    Ok(())
}

/// N8-C：一次性模型调用（RPC 语义）——供临时会话的纯文本生成（规划器/
/// 命令草稿）同步等待结果。这不是对话消息通道：对话走
/// `execute_conversation_command`（turn_start/steer/enqueue，回执契约见 N10）。
/// 临时会话 id 各不相同，不会与用户主对话的 per-session guard 相撞。
#[tauri::command]
pub async fn agent_one_shot(
    app: AppHandle,
    request: AgentChatRequest,
) -> Result<AgentChatResponse, String> {
    let session_id = request_session_id(&request);
    let result = execute_agent_request(app.clone(), request).await;
    dispatch_queued_agent_request(&app, &session_id);
    result
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentQueueResponse {
    pub session_id: String,
    pub queue_id: String,
    pub queued: usize,
}

#[tauri::command]
pub async fn enqueue_agent_message(
    app: AppHandle,
    state: State<'_, AgentRuntimeState>,
    request: AgentChatRequest,
) -> Result<AgentQueueResponse, String> {
    enqueue_persisted_agent_message(app, state, request)
}

pub(super) fn enqueue_persisted_agent_message(
    app: AppHandle,
    state: State<'_, AgentRuntimeState>,
    mut request: AgentChatRequest,
) -> Result<AgentQueueResponse, String> {
    let session_id = request_session_id(&request);
    request.session_id = Some(session_id.clone());
    let idempotency_key = request
        .turn_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("queued-turn-{}", uuid_suffix()));
    request.turn_id = Some(idempotency_key.clone());
    let queue_id = format!("queue-{}", uuid_suffix());
    let durable = crate::history::enqueue_agent_queue_item(
        &app,
        &session_id,
        &queue_id,
        &idempotency_key,
        &request,
    )?;
    let queue_id = durable.item.id.clone();
    let mut should_dispatch = false;
    let snapshot = if durable.item.status == "pending"
        && state.queued_agent_request(&session_id, &queue_id).is_none()
    {
        match state.enqueue_agent_request(
            &session_id,
            queue_id.clone(),
            durable.item.request.clone(),
        ) {
            Ok((snapshot, dispatch)) => {
                should_dispatch = dispatch;
                snapshot
            }
            Err(_) => {
                let snapshot = hydrate_agent_queue_session(&app, &state, &session_id)?;
                should_dispatch = state.dispatch_queued_request(&session_id);
                snapshot
            }
        }
    } else {
        state
            .read_thread_snapshot(&session_id)
            .unwrap_or_else(|| agent_core::ThreadSnapshot::new(session_id.clone()))
    };
    let queued = snapshot.queue.len();
    let _ = app.emit("agent:thread_snapshot", snapshot);
    if should_dispatch {
        tauri::async_runtime::spawn(run_agent_queue(app.clone(), session_id.clone()));
    }
    Ok(AgentQueueResponse {
        session_id,
        queue_id,
        queued,
    })
}

pub(super) async fn run_agent_queue(app: AppHandle, session_id: String) {
    loop {
        let owner = crate::history::agent_queue_lease_owner();
        let lease = match crate::history::claim_next_agent_queue_item(&app, &session_id, owner) {
            Ok(Some(lease)) => lease,
            Ok(None) => {
                let state = app.state::<AgentRuntimeState>();
                let _ = hydrate_agent_queue_session(&app, &state, &session_id);
                break;
            }
            Err(error) => {
                eprintln!("无法领取持久化 Agent 队列: {error}");
                break;
            }
        };
        let state = app.state::<AgentRuntimeState>();
        let live_claim = match state.claim_queued_agent_request(
            &session_id,
            &lease.item.id,
            lease.lease_id.clone(),
        ) {
            Ok(Some(value)) => value,
            Ok(None) | Err(_) => {
                let _ = crate::history::nack_agent_queue_item(
                    &app,
                    &session_id,
                    &lease.item.id,
                    &lease.lease_id,
                    &lease.lease_owner,
                    Some("内存队列与持久队列不同步，已重新载入"),
                );
                let _ = hydrate_agent_queue_session(&app, &state, &session_id);
                break;
            }
        };
        let (_claim, snapshot) = live_claim;
        let _ = app.emit("agent:thread_snapshot", snapshot);
        let admission = AgentQueueAdmission {
            queue_id: lease.item.id.clone(),
            lease_id: lease.lease_id,
            lease_owner: lease.lease_owner,
        };
        if let Err(error) = execute_agent_request_with_admission(
            app.clone(),
            lease.item.request,
            Some(admission),
            None,
            None,
        )
        .await
        {
            eprintln!("排队 Agent 请求执行失败: {error}");
            if error == "当前会话已有 Agent 正在执行" {
                break;
            }
        }
    }
}

#[tauri::command]
pub fn get_agent_thread_snapshot(
    state: State<'_, AgentRuntimeState>,
    session_id: String,
) -> Option<agent_core::ThreadSnapshot> {
    state.read_thread_snapshot(&session_id)
}

/// Restarts an interrupted turn from its durable request record. A provider
/// stream cannot be reattached after the app exits, so the resumed model is
/// explicitly told to validate the current workspace before continuing.
#[tauri::command]
pub async fn resume_agent_run(
    app: AppHandle,
    session_id: String,
    turn_id: String,
) -> Result<AgentChatResponse, String> {
    resume_agent_run_with_start_ack(app, session_id, turn_id, None, None, None).await
}

pub(super) async fn resume_agent_run_with_start_ack(
    app: AppHandle,
    session_id: String,
    turn_id: String,
    resume_turn_id: Option<String>,
    permission_mode: Option<String>,
    start_ack: Option<tokio::sync::oneshot::Sender<()>>,
) -> Result<AgentChatResponse, String> {
    let request = crate::history::request_for_interrupted_agent_run(&app, &session_id, &turn_id)?;
    let request = prepare_resumed_agent_request(
        request,
        &session_id,
        &turn_id,
        resume_turn_id,
        permission_mode,
    )?;
    let result =
        execute_agent_request_with_admission(app.clone(), request, None, None, start_ack).await;
    dispatch_queued_agent_request(&app, &session_id);
    result
}

fn prepare_resumed_agent_request(
    mut request: AgentChatRequest,
    session_id: &str,
    turn_id: &str,
    resume_turn_id: Option<String>,
    permission_mode: Option<String>,
) -> Result<AgentChatRequest, String> {
    // Only an explicit current selection replaces the durable request's policy.
    // Legacy/automatic resume callers keep the original permission level.
    if let Some(mode) = permission_mode {
        if !matches!(mode.as_str(), "ask" | "risk" | "full") {
            return Err("无效的权限模式".to_string());
        }
        request.permission_mode = Some(mode);
    }
    request.session_id = Some(session_id.to_string());
    let resume_turn_id =
        resume_turn_id.unwrap_or_else(|| format!("resume-{}-{}", turn_id, uuid_suffix()));
    if resume_turn_id.trim().is_empty() || resume_turn_id == turn_id {
        return Err("恢复任务必须使用新的轮次标识".to_string());
    }
    request.turn_id = Some(resume_turn_id);
    request.message = format!("上一轮因应用关闭而中断。请检查当前工作区和已有证据后继续原始目标，不要重复可能已经生效的写入。\n\n{}",request.message);
    request.context.push(AgentContextTurn {
        role: "system".to_string(),
        content: "上一轮任务因应用关闭而中断。请先检查当前工作区和已有工具证据，再继续处理原始目标；不要假设此前写入尚未生效，也不要重复可能有副作用的操作。".to_string(),
        source: Some("run_recovery".to_string()),
        message_id: None,
        turn_id: None,
        model_id: None,
        attachments: Vec::new(),
        artifacts: Vec::new(),
    });
    Ok(request)
}

#[cfg(test)]
mod resume_tests {
    use super::*;

    #[test]
    fn resume_uses_current_permission_selection_and_legacy_calls_keep_saved_policy() {
        for saved in ["ask", "risk", "full"] {
            for selected in [None, Some("ask"), Some("risk"), Some("full")] {
                let request: AgentChatRequest = serde_json::from_value(json!({
                    "modelId": "provider:model", "message": "继续构建", "artifacts": [], "permissionMode": saved
                })).unwrap();
                let resumed = prepare_resumed_agent_request(
                    request,
                    "s",
                    "old",
                    Some("new".into()),
                    selected.map(str::to_string),
                )
                .unwrap();
                assert_eq!(
                    resumed.permission_mode.as_deref(),
                    Some(selected.unwrap_or(saved))
                );
                assert_eq!(resumed.turn_id.as_deref(), Some("new"));
                assert!(resumed.message.contains("继续构建"));
            }
        }
        let request: AgentChatRequest =
            serde_json::from_value(json!({"modelId": "p:m", "message": "继续", "artifacts": []}))
                .unwrap();
        assert!(
            prepare_resumed_agent_request(request, "s", "old", None, Some("invalid".into()))
                .is_err()
        );
    }
}

pub(super) struct AgentToolGroupCleanup {
    pub(super) session_id: String,
    pub(super) turn_id: String,
}

pub(super) fn text_segment_may_be_plain_tool_payload(events: &[AgentEvent]) -> bool {
    let mut text = String::new();
    for event in events {
        if let AgentEvent::TextDelta { text: delta, .. } = event {
            text.push_str(delta);
        }
    }
    let text = text.trim_start();
    let Some(remainder) = text.strip_prefix('[') else {
        return false;
    };
    let remainder = remainder.trim_start();
    remainder.is_empty() || remainder.starts_with('{')
}

impl Drop for AgentToolGroupCleanup {
    fn drop(&mut self) {
        end_tool_group(&self.session_id, &self.turn_id);
    }
}
