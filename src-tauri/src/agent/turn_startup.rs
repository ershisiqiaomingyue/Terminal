use super::*;

/// 该对话绑定的一台 SSH 主机。
pub(super) struct BoundSshHost {
    pub connection_id: String,
    pub name: String,
    /// 已解析的远程根目录；未连通时为空串。
    pub root: String,
    /// 已建立的远程上下文；None 表示本轮未连接成功，不会暴露该主机的工具。
    pub context: Option<agent_core::SshToolContext>,
    /// 未连通的原因，会写进提示词，避免模型误以为可以远程操作。
    pub note: Option<String>,
}

impl BoundSshHost {
    pub(super) fn unavailable(connection_id: &str, name: &str, note: String) -> Self {
        Self {
            connection_id: connection_id.to_string(),
            name: name.to_string(),
            root: String::new(),
            context: None,
            note: Some(note),
        }
    }

    /// 能参与 `target` 路由的主机必须有可用的远程上下文。
    pub(super) fn is_ready(&self) -> bool {
        self.context.is_some()
    }
}

/// 本轮准备好的执行环境：默认目标，以及该对话额外绑定的远程主机。
pub(super) struct TurnEnvironments {
    pub ssh_context: Option<agent_core::SshToolContext>,
    pub ssh_connection_id: Option<String>,
    pub wsl_root: Option<String>,
    pub wsl_context: Option<LocalToolContext>,
    /// 默认执行目标就是 SSH 时该主机的（连接名, 连接 ID），用于让模型用连接名
    /// 或 ID 点名默认主机。
    pub default_ssh_host: Option<(String, String)>,
    /// 默认主机之外、该对话绑定的 SSH 主机，按绑定顺序。
    pub bound_ssh_hosts: Vec<BoundSshHost>,
}

impl TurnEnvironments {
    pub(super) fn empty() -> Self {
        Self {
            ssh_context: None,
            ssh_connection_id: None,
            wsl_root: None,
            wsl_context: None,
            default_ssh_host: None,
            bound_ssh_hosts: Vec::new(),
        }
    }
}

pub(super) const FINAL_PHASE_NOTICE: &str =
    "本轮是最终整理阶段。请只根据已有消息和工具结果直接输出最终答案，绝对不要再次调用工具。";

pub(super) struct TurnSkillContext {
    pub skill_catalog: Option<Vec<crate::skill_runtime::SkillView>>,
    pub skill_instructions: Vec<(String, String)>,
    pub selinux_enabled: bool,
    pub selinux_scope: crate::skill_runtime::SelinuxSourceScope,
}

pub(super) async fn prepare_turn_skills(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    skill_catalog_question: bool,
    effective_skill_ids: &[String],
) -> Result<TurnSkillContext, String> {
    if skill_catalog_question {
        emit_agent_status_for_turn(
            app,
            session_id,
            "preparing",
            "正在读取可用 Skills 清单",
            Some(turn_id),
        );
    }
    let app = app.clone();
    let effective_skill_ids = effective_skill_ids.to_vec();
    // Skills 清单读取在阻塞线程池里进行，无法从内部打断。取消语义由外层
    // turn.rs 的 select! 提供：取消瞬间丢弃 JoinHandle，本线程继续在后台
    // 跑完（纯读取、无副作用），本轮不再等待它。
    tauri::async_runtime::spawn_blocking(move || {
        let skill_catalog = Some(load_skill_catalog(&app)?);
        let skill_instructions = load_skill_instructions(&app, &effective_skill_ids)?;
        let selinux_enabled = effective_skill_ids.iter().any(|id| id == "selinux");
        let selinux_scope = if selinux_enabled {
            crate::skill_runtime::load_selinux_source_scope(&app)?
        } else {
            Default::default()
        };
        Ok(TurnSkillContext {
            skill_catalog,
            skill_instructions,
            selinux_enabled,
            selinux_scope,
        })
    })
    .await
    .map_err(|error| format!("Skills 准备任务失败：{error}"))?
}

pub(super) fn spawn_turn_idle_watchdog(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    cancellation: &Arc<AtomicBool>,
) {
    let idle_cancellation = cancellation.clone();
    let idle_app = app.clone();
    let idle_session_id = session_id.to_owned();
    let idle_turn_id = turn_id.to_owned();
    tokio::spawn(async move {
        const MAX_IDLE_DURATION: std::time::Duration =
            agent_core::PROVIDER_IDLE_TIMEOUT.saturating_add(std::time::Duration::from_secs(30));
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let state = idle_app.state::<AgentRuntimeState>();
            if let Some(reason) = state.cancel_if_turn_idle(
                &idle_session_id,
                &idle_turn_id,
                &idle_cancellation,
                MAX_IDLE_DURATION,
            ) {
                crate::diagnostics::record(
                    "warn",
                    "agent.cancellation_applied",
                    "native.agent",
                    json!({
                        "session_id": idle_session_id, "turn_id": idle_turn_id,
                        "source": "idle_watchdog", "code": "turn_idle_timeout",
                        "idle_timeout_seconds": MAX_IDLE_DURATION.as_secs(),
                    }),
                );
                emit_agent_status_for_turn(
                    &idle_app,
                    &idle_session_id,
                    "turn_idle_timeout",
                    format!("已触发熔断器：{reason}"),
                    Some(&idle_turn_id),
                );
                break;
            }
            if idle_cancellation.load(Ordering::Relaxed) {
                break;
            }
            let still_active = state.requests.lock().ok().is_some_and(|requests| {
                requests.get(&idle_session_id).is_some_and(|control| {
                    control.turn_id == idle_turn_id
                        && Arc::ptr_eq(&control.cancellation, &idle_cancellation)
                })
            });
            if !still_active {
                break;
            }
        }
    });
}

pub(super) fn create_turn_provider_client(
    provider: &AgentProviderSettings,
    protocol: ProviderKind,
) -> Result<HttpProviderClient, String> {
    let api_key = match provider_credential(&provider.id) {
        Ok(value) => value,
        Err(_error) if protocol == ProviderKind::OpenAiCompatible => String::new(),
        Err(error) => return Err(error),
    };
    let config = ProviderConfig::new(
        &provider.id,
        &provider.name,
        protocol,
        &provider.base_url,
        format!("keyring:{}", provider.id),
    )
    .map_err(|error| error.to_string())?;
    let client = HttpProviderClient::new(config, api_key).map_err(|error| error.to_string())?;
    Ok(client)
}

pub(super) fn turn_workspace_context(
    request: &AgentChatRequest,
    _permission_mode: &str,
) -> Option<WorkspaceAgentContext> {
    // Only an explicit local project loads local instructions. A remote root
    // must never adopt instructions from an incidental local attachment.
    let local_root = agent_core::local_workspace_path(request);
    // Prompt guidance does not grant tool access. Load it in every mode;
    // the existing permission evaluator still checks each tool operation.
    local_root.map(|root| agent_core::load_workspace_agent_context_at(std::path::Path::new(root)))
}

pub(super) fn turn_collaboration_tool(
    app: &AppHandle,
    request: &AgentChatRequest,
    session_id: &str,
    turn_id: &str,
    permission_mode: &str,
    tool_cancellation: &Arc<AtomicBool>,
) -> Option<CreateCollaborationJobTool> {
    let is_collaboration_worker = request
        .context
        .iter()
        .any(|turn| turn.source.as_deref() == Some("collaboration_job"));
    let collaboration_tool = (!is_collaboration_worker).then(|| CreateCollaborationJobTool {
        app: app.clone(),
        thread_id: session_id.to_owned(),
        parent_turn_id: turn_id.to_owned(),
        model_id: request.model_id.clone(),
        workspace_path: request.workspace_path.clone(),
        permission_mode: permission_mode.to_string(),
        execution_target: request
            .execution_target
            .as_ref()
            .and_then(|target| serde_json::to_value(target).ok()),
        analysis_mode: request.analysis_mode,
        skill_ids: request.skill_ids.clone(),
        reasoning_effort: request.reasoning_effort.clone(),
        task_plan_mode: request.task_plan_mode.clone(),
        task_plan_min_steps: request.task_plan_min_steps,
        cancellation: tool_cancellation.clone(),
        depth: request.collaboration_depth.unwrap_or(0),
    });
    collaboration_tool
}

pub(super) fn turn_reasoning_settings(
    app: &AppHandle,
    request: &AgentChatRequest,
    selected_model: &AgentModelSettings,
    protocol: ProviderKind,
) -> (bool, Option<String>) {
    let model_supports_reasoning = agent_core::model_supports_reasoning(
        protocol,
        &selected_model.id,
        selected_model.supports_reasoning,
    )
    .unwrap_or(false);
    let capability_records = read_cached_capabilities(&app);
    let capability =
        agent_core::find_model_capability(&capability_records, &selected_model.id, None);
    let allowed_efforts = selected_model.reasoning_efforts.clone().or_else(|| {
        capability.map(|record| agent_core::selectable_reasoning_efforts(record, protocol))
    });
    let native_reasoning_effort = selected_model
        .reasoning_efforts
        .as_ref()
        .map(|efforts| !efforts.is_empty())
        .unwrap_or_else(|| capability.is_some_and(|record| !record.reasoning_efforts.is_empty()));
    let reasoning_effort = request
        .reasoning_effort
        .clone()
        .filter(|effort| matches!(effort.as_str(), "low" | "medium" | "high" | "xhigh" | "max"))
        .and_then(|effort| match &allowed_efforts {
            Some(allowed) if allowed.is_empty() => None,
            Some(allowed) if !allowed.contains(&effort) => allowed
                .iter()
                .find(|level| level.as_str() == "medium")
                .or_else(|| allowed.first())
                .cloned(),
            _ => Some(effort),
        })
        .filter(|_| model_supports_reasoning);
    (native_reasoning_effort, reasoning_effort)
}

/// 恢复轮专用的「别再想」档位。
///
/// 事故复盘：`reasoning_only` 连着三轮、每次重试都零正文，提示词里写「不要再次调用
/// 工具」也没能让模型开口 —— 提示词不是物理开关。真正能关掉推理的只有 effort 档位，
/// 所以恢复轮把它降到最低：模型/提供商自己声明了 `none` / `minimal` 就用它；否则在
/// 「提供商本来就会收到 effort 字段」（`native_reasoning_effort`）的前提下退回
/// `minimal`（Responses 语义下等同关断）。`None` 表示这一轮不覆盖，只靠提示词。
pub(super) fn reasoning_recovery_effort(
    app: &AppHandle,
    selected_model: &AgentModelSettings,
    protocol: ProviderKind,
    native_reasoning_effort: bool,
) -> Option<String> {
    // Anthropic 线路只认标准 5 档，塞未知档位有被 400 顶回的风险，那里不覆盖。
    if protocol == ProviderKind::Anthropic {
        return None;
    }
    // 原本就不发 effort 字段的提供商，硬加一个档位反而可能被拒；宁可不覆盖。
    if !native_reasoning_effort {
        return None;
    }
    let capability_records = read_cached_capabilities(app);
    let capability =
        agent_core::find_model_capability(&capability_records, &selected_model.id, None);
    let declared: Vec<String> = selected_model
        .reasoning_efforts
        .clone()
        .filter(|efforts| !efforts.is_empty())
        .or_else(|| capability.map(|record| record.reasoning_efforts.clone()))
        .unwrap_or_default();
    for candidate in ["none", "minimal", "low"] {
        if declared.iter().any(|level| level == candidate) {
            return Some(candidate.to_string());
        }
    }
    // 声明里没有更低档位（内建能力表里多数模型只声明 low..max）时仍然降：这是恢复轮，
    // 宁愿少想一点，也要先把正文写出来。
    Some("minimal".to_string())
}
