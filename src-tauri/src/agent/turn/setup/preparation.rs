use super::super::*;
use super::PreparedTurn;

pub(in crate::agent::turn) struct PreparedInput {
    pub request: AgentChatRequest,
    pub context: TurnSkillContext,
    pub permission_mode: String,
    pub workspace_context: Option<WorkspaceAgentContext>,
    pub global_instructions: Vec<(String, String)>,
    pub evidence: AgentLogEvidence,
    pub prompt: String,
    pub tool_policy: AgentToolPolicy,
    pub windows_paths: Vec<String>,
    pub excluded_paths: Vec<String>,
    pub plugin_turn: crate::plugin_host::PluginTurn,
    pub collaboration_tool: Option<Arc<CreateCollaborationJobTool>>,
    pub explicitly_disabled: bool,
    pub ssh_paths: Vec<String>,
    pub wsl_paths: Vec<String>,
    pub snapshot_paths: Vec<String>,
    pub mcp_tools: Vec<ConnectedMcpTool>,
    pub allow_external_paths: bool,
    pub output_sink: Option<CommandOutputSink>,
    pub remote_context: Option<agent_core::SshToolContext>,
    pub wsl_context: Option<LocalToolContext>,
    pub tool_cancellation: Arc<AtomicBool>,
    pub registry: ToolRegistry,
    pub approved_path_registry: Option<ToolRegistry>,
}

/// Normalizes questions and status for a turn after the remote environments
/// have been resolved. This returns owned inputs so the run loop can rebuild
/// its prompt and tool surface when a remote target becomes available.
pub(in crate::agent::turn) async fn prepare_turn_input(
    input: PreparedTurn<'_>,
) -> Result<PreparedInput, String> {
    let PreparedTurn {
        app,
        request,
        ssh_context,
        bound_ssh_hosts,
        wsl_root,
        wsl_context,
        default_environment,
        mcp_state,
        session_id,
        turn_id,
        cancellation_reason,
        cancellation_token,
        is_ssh_target,
        is_wsl_target,
    } = input;
    let permission_mode = request
        .permission_mode
        .as_deref()
        .unwrap_or("ask")
        .to_string();
    let workspace_context = turn_workspace_context(request, &permission_mode);
    let global_instructions =
        agent_core::load_global_agent_instructions(&crate::user_storage::root()?);
    let evidence = AgentLogEvidence::default();
    let mut prompt = compose_agent_prompt(request, &evidence);
    append_image_source_labels(&mut prompt, &request.attachments, "[本轮图片附件]");
    let skill_catalog_question = is_skill_catalog_question(&request.message);
    let effective_skill_ids = request.skill_ids.clone();
    let context = tokio::select! {
        _ = cancellation_token.cancelled() => {
            return Err(cancellation_message(cancellation_reason));
        }
        // Skills 清单在 spawn_blocking 里读取；取消时这里的 select 直接丢弃
        // JoinHandle，阻塞线程继续在后台跑完但本轮不再等待它（读取无副作用，
        // 结果随下一轮重建）。
        context = prepare_turn_skills(
            app,
            session_id,
            turn_id,
            skill_catalog_question,
            &effective_skill_ids,
        ) => context?,
    };
    let TurnSkillContext {
        selinux_enabled,
        selinux_scope,
        ..
    } = &context;
    let explicitly_disabled = request.tool_policy == AgentToolPolicy::Disabled;
    let tool_policy = if ((is_ssh_target || is_wsl_target) && !explicitly_disabled)
        || agent_core::should_enable_agent_tools(request)
    {
        AgentToolPolicy::Auto
    } else {
        AgentToolPolicy::Disabled
    };
    let mut windows_paths = request
        .artifacts
        .iter()
        .filter(|artifact| !is_virtual_artifact_path(&artifact.path))
        .map(|artifact| artifact.path.clone())
        .collect::<Vec<_>>();
    let excluded_paths = if *selinux_enabled {
        for root in &selinux_scope.roots {
            if !windows_paths.iter().any(|path| path == root) {
                windows_paths.push(root.clone());
            }
        }
        selinux_scope.excluded_paths.clone()
    } else {
        Vec::new()
    };
    let mut ssh_paths = ssh_context
        .map(|context| vec![context.root().to_string()])
        .unwrap_or_default();
    for host in bound_ssh_hosts.iter().filter(|host| host.is_ready()) {
        if !ssh_paths.iter().any(|path| path == &host.root) {
            ssh_paths.push(host.root.clone());
        }
    }
    let wsl_paths = wsl_root.map(str::to_string).into_iter().collect::<Vec<_>>();
    if is_ssh_target && ssh_paths.is_empty() {
        return Err("SSH 执行目标缺少远程工作目录".to_string());
    }
    if is_wsl_target && wsl_paths.is_empty() {
        return Err("WSL 执行目标缺少工作目录".to_string());
    }
    let snapshot_paths = if tool_policy == AgentToolPolicy::Disabled {
        Vec::new()
    } else {
        request_snapshot_paths(&request.artifacts)
    };
    let allow_external_paths = permission_mode == "full";
    let output_sink = Some(CommandOutputSink::for_tool(
        app,
        session_id,
        turn_id,
        "run_local_command",
    ));
    let remote_context = ssh_context.map(|context| {
        context.with_output_sink(output_sink.as_ref().map(|sink| sink.inner().clone()))
    });
    let wsl_context = wsl_context.map(|context| {
        context.with_output_sink(output_sink.as_ref().map(|sink| sink.inner().clone()))
    });
    let tool_cancellation = Arc::new(AtomicBool::new(false));
    let plugin_turn = crate::plugin_host::prepare_turn(
        session_id,
        turn_id,
        if tool_policy == AgentToolPolicy::Disabled {
            &[]
        } else {
            &windows_paths
        },
        &permission_mode,
    )
    .await;
    let mcp_tools = connected_mcp_tools(mcp_state)?;
    let (mut registry, mut approved_path_registry) =
        crate::agent::build_environment_tool_registries(
            default_environment,
            &windows_paths,
            &excluded_paths,
            &ssh_paths,
            &wsl_paths,
            &mcp_tools,
            tool_cancellation.clone(),
            allow_external_paths,
            &output_sink,
            remote_context.as_ref(),
            wsl_context.as_ref(),
            bound_ssh_hosts,
            app,
            session_id,
            turn_id,
            None,
        )?;
    let collaboration_tool = turn_collaboration_tool(
        app,
        request,
        session_id,
        turn_id,
        &permission_mode,
        &tool_cancellation,
    )
    .map(Arc::new);
    // Collaboration jobs are registered before the turn starts so that the
    // prepared registries and later rebuilds share one callable instance.
    // Remote MCP transports are intentionally not exposed here: the
    // desktop-hosted servers would execute locally for an SSH request.
    crate::agent::register_collaboration_tool(
        &mut registry,
        &mut approved_path_registry,
        (tool_policy == AgentToolPolicy::Auto)
            .then_some(collaboration_tool.as_deref())
            .flatten(),
        tool_cancellation.clone(),
    )?;
    Ok(PreparedInput {
        request: request.clone(),
        context,
        permission_mode,
        workspace_context,
        global_instructions,
        evidence,
        prompt,
        tool_policy,
        windows_paths,
        excluded_paths,
        plugin_turn,
        collaboration_tool,
        explicitly_disabled,
        ssh_paths,
        wsl_paths,
        snapshot_paths,
        mcp_tools,
        allow_external_paths,
        output_sink,
        remote_context,
        wsl_context,
        tool_cancellation,
        registry,
        approved_path_registry,
    })
}
