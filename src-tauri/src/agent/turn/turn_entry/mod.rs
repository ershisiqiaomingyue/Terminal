use super::super::*;
use tokio::sync::mpsc::Receiver;
use tokio_util::sync::CancellationToken;

pub(in crate::agent::turn) struct TurnInput<'a> {
    pub app: AppHandle,
    pub state: State<'a, AgentRuntimeState>,
    pub mcp_state: State<'a, McpRuntimeState>,
    pub request: AgentChatRequest,
    pub session_id: String,
    pub turn_id: String,
    pub request_guard: AgentRequestGuard<'a>,
    pub cancellation: Arc<AtomicBool>,
    pub cancellation_reason: Arc<Mutex<Option<String>>>,
    pub cancellation_token: CancellationToken,
    pub steer_receiver: Option<Receiver<AgentSteerRequest>>,
    pub image_attachments: Vec<ProviderImageAttachment>,
    pub ssh_context: Option<agent_core::SshToolContext>,
    pub ssh_connection_id: Option<String>,
    pub wsl_root: Option<String>,
    pub wsl_context: Option<LocalToolContext>,
    pub default_ssh_host: Option<(String, String)>,
    pub bound_ssh_hosts: Vec<BoundSshHost>,
    pub is_ssh_target: bool,
    pub is_wsl_target: bool,
    pub default_environment: AgentToolEnvironment,
    pub provider: agent_core::AgentProviderSettings,
    pub selected_model: agent_core::AgentModelSettings,
    pub protocol: agent_core::ProviderKind,
}
pub(in crate::agent::turn) async fn prepare_and_run<'a>(
    app: AppHandle,
    state: State<'a, AgentRuntimeState>,
    mcp_state: State<'a, McpRuntimeState>,
    request: AgentChatRequest,
    session_id: String,
    turn_id: String,
    mut request_guard: AgentRequestGuard<'a>,
) -> Result<AgentChatResponse, String> {
    let cancellation = request_guard.cancellation();
    let cancellation_reason = request_guard.cancellation_reason();
    let cancellation_token = request_guard.cancellation_token();
    let steer_receiver = request_guard.take_steer_receiver();
    let current_user_message_id = format!("user:{turn_id}");
    let attachment_root = app
        .path()
        .app_cache_dir()
        .map_err(|error| error.to_string())?
        .join("agent-attachments");
    let providers = load_providers(&app)?;
    let (provider, selected_model) = agent_core::resolve_agent_model(&providers, &request.model_id)
        .ok_or_else(|| {
            format!(
                "[model_not_found] 当前选择的模型 {} 不在已加载的供应商配置中，请重新选择模型。",
                request.model_id
            )
        })?;
    let protocol = model_protocol(&provider, &selected_model);
    let image_attachments = normalize_request_image_attachments(
        &attachment_root,
        &request.attachments,
        &request.context,
        selected_model.supports_vision,
        &current_user_message_id,
    )?;
    prepare_and_execute_turn(TurnInput {
        app,
        state,
        mcp_state,
        request,
        session_id,
        turn_id,
        request_guard,
        cancellation,
        cancellation_reason,
        cancellation_token,
        steer_receiver: Some(steer_receiver),
        image_attachments,
        ssh_context: None,
        ssh_connection_id: None,
        wsl_root: None,
        wsl_context: None,
        default_ssh_host: None,
        bound_ssh_hosts: Vec::new(),
        is_ssh_target: false,
        is_wsl_target: false,
        default_environment: AgentToolEnvironment::Windows,
        provider: provider.clone(),
        selected_model,
        protocol,
    })
    .await
}

async fn prepare_and_execute_turn<'a>(
    input: TurnInput<'a>,
) -> Result<AgentChatResponse, String> {
    let input = prepare_turn_entry(input).await?;
    super::body::run_turn_body(input).await
}

async fn prepare_turn_entry<'a>(
    input: TurnInput<'a>,
) -> Result<TurnInput<'a>, String> {
    let TurnInput {
        provider,
        selected_model,
        protocol,
        app,
        state,
        mcp_state,
        request,
        session_id,
        turn_id,
        request_guard,
        cancellation,
        cancellation_reason,
        cancellation_token,
        steer_receiver,
        ssh_context: _,
        ssh_connection_id: _,
        wsl_root: _,
        wsl_context: _,
        default_ssh_host: _,
        bound_ssh_hosts: _,
        is_ssh_target: _,
        is_wsl_target: _,
        default_environment: _,
        image_attachments,
    } = input;
    let super::setup::TurnRequest {
        request,
        is_ssh_target,
        is_wsl_target,
        default_environment,
    } = super::setup::prepare_input(request)?;
    let _tool_group_cleanup = AgentToolGroupCleanup {
        session_id: session_id.clone(),
        turn_id: turn_id.clone(),
    };
    // P1-B：令牌与上面的标志同生同灭（见 `AgentRequestControl::request_cancellation`）。
    // 准备阶段的 `await` 用它即时打断，不再依赖执行流自己走到下一个
    // `is_cancelled` 检查点；轮循环内既有的标志检查点保持不变（P2 回归基线）。
    spawn_turn_idle_watchdog(&app, &session_id, &turn_id, &cancellation);
    // 准备阶段（本轮开头 → 首个模型请求）此前没有任何取消检查点：SSH 握手、
    // WSL 探测、Skills 索引读取一旦卡住，「停止」就要等它们自然结束。这里用
    // token 与准备 future 做 select：取消瞬间丢弃 future，卡住的 await 不再被
    // 等待（被丢弃的连接任务照常在后台自行收尾）。
    let environments = tokio::select! {
        _ = cancellation_token.cancelled() => {
            return Err(cancellation_message(&cancellation_reason));
        }
        environments = super::turn_prepare::prepare_with_timeout(
            super::turn_prepare::DEFAULT_PREPARE_TIMEOUT,
            &cancellation,
            super::setup::prepare_turn_environments(
                &app,
                &request,
                &session_id,
                &turn_id,
                &cancellation,
                is_ssh_target,
                is_wsl_target,
            ),
        ) => environments?,
    };
    let TurnEnvironments {
        ssh_context,
        ssh_connection_id,
        wsl_root,
        wsl_context,
        default_ssh_host,
        bound_ssh_hosts,
    } = environments;
    crate::diagnostics::record(
        "info",
        "agent.turn_context",
        "native.agent",
        json!({
            "session_id": session_id, "turn_id": turn_id,
            "provider_id": provider.id, "model_id": selected_model.id, "protocol": protocol,
            "context_window": agent_core::resolved_model_context_window(&provider, &selected_model),
            "execution_kind": if is_ssh_target { "ssh" } else if is_wsl_target { "wsl" } else { "local" },
            "connection_id": ssh_connection_id,
            "has_local_workspace": agent_core::local_workspace_path(&request).is_some(),
            "has_ssh_workspace": ssh_context.is_some(),
            "profile_id": request.execution_target.as_ref().and_then(|target| target.profile_id.as_deref()),
            "has_workspace": request.workspace_path.as_deref().is_some_and(|path| !path.trim().is_empty()),
            "has_execution_root": request.execution_target.as_ref().and_then(|target| target.root.as_deref()).is_some_and(|path| !path.trim().is_empty()),
            "turn_hooks_enabled": super::hooks::hooks_enabled(),
            "unit_kind": state.requests.lock().ok()
                .and_then(|requests| requests.get(&session_id).map(|control| control.unit_kind().as_str().to_string()))
                .unwrap_or_else(|| "turn".to_string()),
        }),
    );
    Ok(TurnInput {
        app,
        state,
        mcp_state,
        request,
        session_id,
        turn_id,
        request_guard,
        cancellation,
        cancellation_reason,
        cancellation_token,
        steer_receiver,
        ssh_context,
        ssh_connection_id,
        wsl_root,
        wsl_context,
        default_ssh_host,
        bound_ssh_hosts,
        is_ssh_target,
        is_wsl_target,
        default_environment,
        provider,
        selected_model,
        protocol,
        image_attachments,
    })
}
