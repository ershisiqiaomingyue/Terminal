use super::super::*;
use super::super::strategy::{context_window_budget_exceeded, tool_permission_timeout};
use super::strategy::take_recovery_reasoning_effort;
use super::finalization::{
    finalize_usage, save_transcript, CheckpointState, UsageFinalization,
};
use super::setup::{prepare_turn_input, PreparedInput, PreparedTurn};
use super::budget;

mod initialization;
use initialization::{initialize_context, ContextInitialization, ContextInput};

pub(in crate::agent::turn) async fn run_turn_body<'a>(
    input: super::turn_entry::TurnInput<'a>,
) -> Result<AgentChatResponse, String> {
    let super::turn_entry::TurnInput {
        app,
        state,
        mcp_state,
        mut request,
        session_id,
        turn_id,
        request_guard: _,
        cancellation,
        cancellation_reason,
        cancellation_token,
        steer_receiver,
        ssh_context,
        mut ssh_connection_id,
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
        mut image_attachments,
    } = input;
    let mut steer_receiver = steer_receiver.ok_or_else(|| {
        "[steering_unavailable] 指导消息接收器尚未初始化".to_string()
    })?;
    let current_user_message_id = format!("user:{turn_id}");
    let attachment_root = app.path().app_cache_dir()
        .map_err(|error| error.to_string())?
        .join("agent-attachments");
    for turn in &mut request.context {
        if turn.role.eq_ignore_ascii_case("user") {
            append_image_source_labels(
                &mut turn.content, &turn.attachments,
                "[历史消息中的图片来源]",
            );
        }
    }
    let diagnostic_session = session_id.clone();
    let diagnostic_turn = turn_id.clone();
    let client = create_turn_provider_client(&provider, protocol)?.with_diagnostic_observer(
        Arc::new(move |diagnostic| {
            let level = if diagnostic.failure_code.is_some() { "warn" } else { "info" };
            crate::diagnostics::record(
                level, "agent.provider_transport", "native.agent",
                json!({
                    "session_id": diagnostic_session,
                    "turn_id": diagnostic_turn,
                    "transport": diagnostic,
                }),
            );
        }),
    );
    let PreparedInput {
        request,
        context: TurnSkillContext {
            skill_catalog,
            skill_instructions,
            selinux_enabled,
            selinux_scope,
        },
        permission_mode,
        workspace_context,
        global_instructions,
        evidence,
        prompt,
        mut tool_policy,
        mut windows_paths,
        excluded_paths,
        mut plugin_turn,
        collaboration_tool,
        explicitly_disabled,
        mut ssh_paths,
        wsl_paths,
        snapshot_paths,
        mcp_tools,
        allow_external_paths,
        output_sink,
        mut remote_context,
        wsl_context,
        mut tool_cancellation,
        mut registry,
        mut approved_path_registry,
    } = prepare_turn_input(PreparedTurn {
        app: &app,
        request: &request,
        ssh_context: ssh_context.as_ref(),
        bound_ssh_hosts: &bound_ssh_hosts,
        wsl_root: wsl_root.as_deref(),
        wsl_context: wsl_context.as_ref(),
        default_environment,
        mcp_state: &mcp_state,
        session_id: &session_id,
        turn_id: &turn_id,
        cancellation_reason: &cancellation_reason,
        cancellation_token: &cancellation_token,
        is_ssh_target,
        is_wsl_target,
    })
    .await?;
    let permission_mode = permission_mode.as_str();
    let mut ssh_permission_namespace = ssh_connection_id
        .as_deref()
        .map(|id| format!("ssh:{id}"))
        // P7-A：回退 namespace 不再掺 turn_id——掺了它每个 turn 都是新 key，
        // 「本次会话允许」跨轮永远不命中。无连接期间统一用稳定回退值；
        // 连接就绪后由 question-card 路径迁往 ssh:{connection_id}（P7-D）。
        .unwrap_or_else(|| "ssh:unavailable".to_string());
    let wsl_permission_namespace = request
        .execution_target
        .as_ref()
        .and_then(|target| target.profile_id.as_deref())
        .map(|id| format!("wsl:{id}"))
        .unwrap_or_else(|| "wsl:unavailable".to_string());
    // A workspace snapshot can require hashing thousands of files. Do not put
    // that cost in front of every turn: capture it lazily just before the
    // first mutating tool call, where it is actually needed for file changes.
    let mut before_files = None;
    let mut last_file_snapshot = Vec::new();
    let mut file_backup_lease = None;
    let mut before_snapshot_attempted = false;
    let tool_definitions = registry
        .list_public()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let context_window = agent_core::resolved_model_context_window(&provider, &selected_model);
    let context_window_resolution = agent_core::resolve_context_window(context_window);
    let context_window_estimated = !context_window_resolution.is_known();
    let provider_capabilities = ProviderCapabilities::for_kind(protocol);
    let model_supports_tools = selected_model
        .supports_tools
        .unwrap_or(provider_capabilities.supports_tools);
    let model_supports_usage = selected_model
        .supports_usage
        .unwrap_or(provider_capabilities.supports_usage);
    let (native_reasoning_effort, reasoning_effort) =
        turn_reasoning_settings(&app, &request, &selected_model, protocol);
    // 恢复轮要用的「别再想」档位（多数模型上就是最低档）：只有真的走到
    // `reasoning_only` 那一轮才会被用掉一次，见 `take_recovery_reasoning_effort`。
    let recovery_reasoning_effort =
        reasoning_recovery_effort(&app, &selected_model, protocol, native_reasoning_effort);
    let mut request_tools = if tool_policy == AgentToolPolicy::Disabled || !model_supports_tools {
        Vec::new()
    } else {
        tool_definitions.clone()
    };
    let image_token_reserve =
        AtomicUsize::new(estimate_image_attachment_tokens(&image_attachments));
    let estimate_context_tokens =
        |messages: &[AgentMessage], system: &str, tools: &[ToolDefinition]| {
            estimate_agent_request_tokens(messages, system, tools)
                .saturating_add(image_token_reserve.load(Ordering::Relaxed))
        };
    let mut text = String::new();
    let mut usage = None;
    let model_request_count = std::cell::Cell::new(0u64);
    let mut compatible_usage_fallback = false;
    let mut responses_reasoning_include_fallback = false;
    // 一次性开关：`reasoning_only` 恢复轮把下一轮的推理档位降到最低（用完即复位）。
    let mut reasoning_off_once = false;
    let compose_turn_prompt = |tool_policy,
                               request_tools: &[ToolDefinition],
                               plugin_instruction: &str,
                               ssh_root: Option<&str>| {
        let mut composed = prompt_composer::compose(prompt_composer::AgentPromptContext {
            request: &request,
            provider_name: &provider.name,
            model_id: &selected_model.id,
            protocol,
            tool_policy,
            model_supports_tools,
            request_tools,
            selinux_enabled,
            selinux_has_roots: !selinux_scope.roots.is_empty(),
            skill_instructions: &skill_instructions,
            skill_catalog: skill_catalog.as_deref(),
            is_ssh_target,
            is_wsl_target,
            default_environment,
            ssh_root,
            wsl_root: wsl_root.as_deref(),
            bound_ssh_hosts: &bound_ssh_hosts,
            default_ssh_host: default_ssh_host.as_ref(),
            workspace_context: workspace_context.as_ref(),
            global_instructions: &global_instructions,
        });
        composed.runtime.push_str(plugin_instruction);
        composed
    };
    let plugin_instruction = if request_tools.is_empty() {
        String::new()
    } else {
        plugin_turn.prompt(default_environment == AgentToolEnvironment::Windows)
    };
    let composed_prompt = compose_turn_prompt(
        tool_policy,
        &request_tools,
        &plugin_instruction,
        remote_context.as_ref().map(|context| context.root()),
    );
    let context_budget = agent_context_budget(
        context_window,
        &composed_prompt.system,
        &request_tools,
        false,
    )
    .saturating_sub(image_token_reserve.load(Ordering::Relaxed))
    .saturating_sub(agent_core::estimate_messages_tokens(&[AgentMessage::system(
        composed_prompt.runtime.clone(),
    )]))
    .max(1);
    let ContextInitialization {
        base_system_prompt,
        mut runtime_context,
        mut transcript_scope,
        prompt_cache_key,
        mut cache_fingerprint,
        mut runtime_context_tokens,
        context_budget,
        telemetry_output_reserve,
        initial_context_tokens,
        initial_compaction_id,
        mut model_summary_attempted,
        model_summary_requested,
        mut planned_compaction_summary,
        initial_context_compressed,
        initial_context_summary,
        initial_context_checkpoint_payload,
        initial_covered_message_count,
        initial_covered_message_id,
        restored_checkpoint_context,
        mut messages,
        initial_context_status: initial_compaction_status,
        initial_context_stats: initial_compaction_stats,
    } = initialize_context(ContextInput {
        app: &app,
        state: &state,
        client: &client,
        request: &request,
        provider: &provider,
        selected_model: &selected_model,
        protocol,
        session_id: &session_id,
        turn_id: &turn_id,
        current_user_message_id: &current_user_message_id,
        prompt: &prompt,
        composed_prompt,
        context_window,
        context_budget,
        model_supports_usage,
        native_reasoning_effort,
        reasoning_effort: reasoning_effort.as_deref(),
        request_tools: &request_tools,
        image_token_reserve: &image_token_reserve,
        image_attachments: &mut image_attachments,
        ssh_context: ssh_context.as_ref(),
        wsl_root: wsl_root.as_deref(),
        ssh_connection_id: ssh_connection_id.as_deref(),
        cancellation: &cancellation,
        cancellation_reason: &cancellation_reason,
        compatible_usage_fallback,
        responses_reasoning_include_fallback,
        usage: &mut usage,
    })
    .await?;
    if model_summary_requested {
        model_request_count.set(model_request_count.get().saturating_add(1));
    }
    cache_context::retain_runtime(&mut messages, &runtime_context);
    let active_image_attachments = image_attachments_for_messages(&image_attachments, &messages);
    update_image_token_reserve(&active_image_attachments, &image_token_reserve);
    let mut checkpoint = CheckpointState {
        summary: initial_context_summary,
        payload: initial_context_checkpoint_payload,
        covered_through_message_id: initial_covered_message_id.or_else(|| {
            (!restored_checkpoint_context)
                .then(|| {
                    context_message_id_at_boundary(
                        &request.context,
                        initial_covered_message_count,
                    )
                })
                .flatten()
        }),
    };
    let profile = ModelProfile::new(&request.model_id, &provider.id, &selected_model.id)
        .with_protocol(protocol)
        .with_capabilities(
            context_window,
            model_supports_tools,
            selected_model.supports_vision.unwrap_or(false),
            provider_capabilities.supports_structured_output,
            model_supports_usage,
        );
    let estimated_input_tokens = u32::try_from(estimate_context_tokens(
        &messages,
        &base_system_prompt,
        &request_tools,
    ))
    .ok();
    let last_context_tokens =
        AtomicU64::new(
            estimate_context_tokens(&messages, &base_system_prompt, &request_tools) as u64,
        );
    profile
        // The report JSON is enforced by the agent prompt and parsed locally. We do not
        // send a provider-specific response_format/structured-output field, so this
        // request must not claim that capability is required at the wire level.
        .check_compatibility(!request_tools.is_empty(), false, estimated_input_tokens)
        .map_err(|error| format!("[protocol_mismatch] {error}"))?;
    // G5-B：压缩把历史整体替换掉之后，上一轮记下的「上次触发位置」已经没有
    // 参照意义（前缀不同了），必须复位，否则滞回线会一直压在低位、连续触发。
    if initial_context_compressed {
        if let Ok(mut hysteresis) = state.compaction_hysteresis.lock() {
            hysteresis
                .entry(format!("{session_id}:{}", selected_model.id))
                .or_default()
                .reset();
        }
    }
    if initial_context_compressed {
        emit_agent_context_for_turn(
            &app,
            json!({
                "session_id": session_id,
                "compaction_id": initial_compaction_id,
                "compaction_status": "completed",
                "compaction_reason": "history_budget",
                "before_context_tokens": initial_context_tokens,
                "after_context_tokens": estimate_context_tokens(&messages, &base_system_prompt, &request_tools),
                "reserved_output_tokens": telemetry_output_reserve,
                "compaction_result": initial_compaction_status,
                "compaction_stats": initial_compaction_stats,
                "contextCheckpoint": context_checkpoint_event_value(
                    &initial_compaction_id,
                    checkpoint.summary.as_deref(),
                    checkpoint.payload.as_ref(),
                    checkpoint.covered_through_message_id.as_deref(),
                    &selected_model.id,
                ),
                "available_tokens": available_agent_context_tokens(
                    context_window,
                    estimate_context_tokens(&messages, &base_system_prompt, &request_tools),
                    telemetry_output_reserve,
                ),
                "context_compressed": true,
            }),
            Some(&turn_id),
        );
        emit_agent_status_for_turn(
            &app,
            &session_id,
            "context_compacted",
            "上下文压缩完成，准备继续",
            Some(&turn_id),
        );
    }
    let mut tool_rounds = 0usize;
    let mut model_round_index = 0usize;
    let mut generation_metrics = agent_core::GenerationMetrics::default();
    let mut final_response_requested = false;
    // 最终整理通知已经在 messages 里追加过（避免重复追问式的要求堆积）。
    let mut final_phase_notice_pushed = false;
    let mut output_continuations = 0usize;
    let mut response_recovery = agent_core::turn_recovery::ModelResponseRecovery::default();
    let mut tool_loop_progress = agent_core::tool_ledger::ToolLoopProgress::default();
    let mut termination_reason = "model_finished";
    let mut provider_context_retry_count = 0usize;
    let mut pending_provider_startup_error = None;
    let mut malformed_tool_signatures = HashSet::new();
    let mut tool_call_ledger = HashMap::<String, ToolCallLedgerEntry>::new();
    let mut dsml_stream = agent_core::ToolStreamState::default();
    let mut timeline_segment: Option<TimelineSegment>;
    let mut pending_ssh_connection_id: Option<String> = None;
    // Provider-reported input+output charge for the most recent request of this turn.
    // The local estimator undercounts real usage, so a reported over-window value is
    // authoritative for the window guard below; it is cleared as soon as a
    // reported-driven hard compaction has consumed it.
    let mut reported_request_tokens: Option<usize> = None;
    // At most one reported-usage hard compaction per turn, never a compaction loop.
    let mut reported_usage_compacted = false;
    // Provider count over the local estimate of the same request, times 100. The
    // estimator runs under the real charge on tool-heavy transcripts, so the
    // compaction trigger is scaled by this measured drift; `None` until a round
    // has reported usage.
    let mut context_charge_percent: Option<usize> = None;
    // P9-E：本轮之前压缩改写过请求前缀，缓存读数要从头起算（只置一次）。
    let mut cache_baseline_reset = false;
    loop {
        if is_cancelled(&state, &session_id, &cancellation) {
            return Err(cancellation_message(&cancellation_reason));
        }
        // A connection selected through the question card becomes available in
        // this same turn. Rebuild both registries and the per-host permission
        // namespace; never change the default Windows/WSL execution target.
        if remote_context.is_none() && tool_policy != AgentToolPolicy::Disabled {
            if let Some(id) = pending_ssh_connection_id.take() {
                let mut ssh_request = request.clone();
                ssh_request.execution_target = Some(agent_core::AgentExecutionTarget {
                    kind: "ssh".into(),
                    connection_id: Some(id),
                    profile_id: None,
                    root: None,
                });
                match prepare_agent_ssh_context(&app, &ssh_request, cancellation.clone()).await {
                    Ok(Some((context, connection_id))) => {
                        ssh_paths = vec![context.root().to_string()];
                        // P7-B：namespace 换成稳定的连接值时不再重写历史授权——
                        // 授权在「本次会话允许」时已同时写入 `ssh::` 种类 key，
                        // 换连接照样命中；旧的 `ssh:unavailable::` key 保留，
                        // 断线回退时仍然有效。
                        ssh_permission_namespace = format!("ssh:{connection_id}");
                        ssh_connection_id = Some(connection_id);
                        remote_context = Some(context.with_output_sink(
                            output_sink.as_ref().map(|sink| sink.inner().clone()),
                        ));
                        (registry, approved_path_registry) = build_environment_tool_registries(
                            default_environment, &windows_paths, &excluded_paths, &ssh_paths,
                            &wsl_paths, &mcp_tools, tool_cancellation.clone(), allow_external_paths,
                            &output_sink, remote_context.as_ref(), wsl_context.as_ref(),
                            &bound_ssh_hosts,
                            &app, &session_id, &turn_id,
                            (tool_policy == AgentToolPolicy::Auto)
                                .then_some(collaboration_tool.as_deref()).flatten(),
                        )?;
                        request_tools = registry.list_public().into_iter().cloned().collect();
                        let plugin_instruction = plugin_turn.prompt(
                            default_environment == AgentToolEnvironment::Windows,
                        );
                        runtime_context_tokens = cache_context::update_runtime(
                            &mut messages, &mut runtime_context,
                            compose_turn_prompt(
                                tool_policy, &request_tools, &plugin_instruction,
                                remote_context.as_ref().map(|context| context.root()),
                            ).runtime,
                        );
                        transcript_scope = cache_context::scope(
                            &provider, &selected_model.id, protocol, &request,
                            remote_context.as_ref().unwrap().root(), ssh_connection_id.as_deref(),
                        );
                    }
                    Err(error) => messages.push(AgentMessage::system(format!("SSH 连接后的工作区准备失败：{error}。远程工具尚不可用，不能声称已执行远程操作。"))),
                    Ok(None) => {}
                }
            }
        }
        // A new model round starts a new timeline segment. If the model emits
        // another tool call without commentary, the runtime supplies one
        // fallback sentence for that new tool group only.
        timeline_segment = None;
        begin_tool_group(&session_id, &turn_id);
        emit_agent_status_for_turn(
            &app,
            &session_id,
            "requesting_model",
            if tool_rounds == 0 {
                "正在等待模型响应"
            } else {
                "文件已读取，模型正在分析并生成回答"
            },
            Some(&turn_id),
        );
        // Preserve system/tool declarations during finalization; append the
        // notice chronologically so the reusable request prefix stays intact.
        let system_prompt = base_system_prompt.to_string();
        // Append once per final phase. A new user steer clears the old notice.
        if final_response_requested && !final_phase_notice_pushed {
            messages.push(AgentMessage::system(FINAL_PHASE_NOTICE));
            final_phase_notice_pushed = true;
        }
        let request_tools_for_round: &[ToolDefinition] = if tool_policy == AgentToolPolicy::Disabled
        {
            &[]
        } else {
            request_tools.as_slice()
        };
        let active_image_attachments =
            image_attachments_for_messages(&image_attachments, &messages);
        update_image_token_reserve(&active_image_attachments, &image_token_reserve);
        // P8-C：触发点的漂移修正优先用 EMA 校准值（含历史阻尼）；校准器尚无
        // 观测（本会话首轮/换模型后）时回退单次观测值（旧语义）。
        let calibrated_charge_percent = {
            let calibrator_key = format!("{session_id}:{}", selected_model.id);
            state
                .token_calibrators
                .lock()
                .ok()
                .and_then(|calibrators| {
                    calibrators
                        .get(&calibrator_key)
                        .filter(|calibrator| calibrator.charge_percent() != 100)
                        .map(|calibrator| calibrator.charge_percent())
                })
                .or(context_charge_percent)
        };
        // P8-E：滞回——上一次实际触发的位置会把本轮触发线下移一段安全带。
        let round_hysteresis = state
            .compaction_hysteresis
            .lock()
            .ok()
            .and_then(|hysteresis| {
                hysteresis
                    .get(&format!("{session_id}:{}", selected_model.id))
                    .cloned()
            });
        let budget = budget::plan_round_budget(
            context_window,
            &system_prompt,
            request_tools_for_round,
            &messages,
            final_response_requested,
            runtime_context_tokens,
            image_token_reserve.load(Ordering::Relaxed),
            calibrated_charge_percent,
            round_hysteresis,
        );
        let round_context_budget = budget.context_budget;
        let round_output_reserve = budget.output_reserve;
        let before_context_tokens = budget.before_tokens;
        let round_request_overhead = budget.request_overhead;
        let round_compaction_trigger = budget.compaction_trigger;
        let mut compaction = agent_core::compact_messages_with_trigger(
            &messages,
            round_context_budget,
            round_compaction_trigger,
        );
        if compaction.compressed {
            let hysteresis_key = format!("{session_id}:{}", selected_model.id);
            if let Ok(mut hysteresis) = state.compaction_hysteresis.lock() {
                hysteresis
                    .entry(hysteresis_key)
                    .or_default()
                    .record_trigger(round_compaction_trigger);
            }
            // P9-E：压缩改写了请求前缀，之前攒下的缓存读数对「新前缀的命中率」
            // 不再有意义。打上标记，本轮观测会从头起算，避免命中率被旧数据稀释。
            cache_baseline_reset = true;
        }
        let round_compaction_id = format!("compaction-{}", uuid_suffix());
        // P9-F：本轮前缀即将被压缩改写，且当前请求已经顶到触发线——先发一个
        // 极小请求把完整前缀写成热缓存，紧跟的压缩请求仍能命中它；否则这次
        // 全价输入永远不会被复用（缓存要读一次才回本）。
        if cache_context::should_prewarm_cache(
            before_context_tokens,
            round_compaction_trigger,
            messages.len(),
        ) {
            let prewarm = ProviderRequest::new(
                &session_id,
                &selected_model.id,
                Vec::new(),
                request_tools_for_round.to_vec(),
            )
            .with_system_prompt(system_prompt.clone())
            .with_max_tokens(16)
            .with_usage_reporting(model_supports_usage && !compatible_usage_fallback);
            let messages_before_prewarm = messages.clone();
            let mut prewarm_messages = messages.clone();
            prewarm_messages.push(AgentMessage::user("P9-F prewarm"));
            let mut prewarm_request = prewarm.clone();
            prewarm_request.messages = prewarm_messages;
            match client.stream(prewarm_request).await {
                Ok(_) => {
                    model_request_count.set(model_request_count.get().saturating_add(1));
                    crate::diagnostics::record(
                        "info",
                        "agent.cache_prewarm",
                        "native.agent",
                        json!({
                            "session_id": session_id, "turn_id": turn_id,
                            "estimated_tokens": before_context_tokens,
                            "trigger_tokens": round_compaction_trigger,
                        }),
                    );
                }
                Err(error) => crate::diagnostics::record(
                    "warn",
                    "agent.cache_prewarm_failed",
                    "native.agent",
                    json!({"session_id": session_id, "turn_id": turn_id, "error": error.to_string()}),
                ),
            }
            // 预热只是旁路请求，绝不能改写本轮的对话历史。
            messages = messages_before_prewarm;
        }
        if compaction.compressed {
            emit_agent_context_for_turn(
                &app,
                json!({
                    "session_id": session_id, "compaction_id": round_compaction_id,
                    "compaction_status": "running", "compaction_reason": "context_budget",
                    "before_context_tokens": before_context_tokens,
                    "reserved_output_tokens": round_output_reserve, "context_compressed": true,
                }),
                Some(&turn_id),
            );
        }
        if compaction.compressed && !model_summary_attempted {
            // P9-G：上一轮已经预置过同一份检查点摘要时直接沿用，不为同一段历史
            // 再生成一次（第二次生成必然措辞不同，等于把前缀整体打散）。
            if planned_compaction_summary.is_some() {
                model_summary_attempted = true;
            } else {
                emit_agent_status_for_turn(
                    &app,
                    &session_id,
                    "compacting_context",
                    "正在整理上下文，保留约束与工具证据",
                    Some(&turn_id),
                );
                let processed_messages = compaction.stats.covered_message_count;
                // Same estimate as the initial round: the summary character budget
                // drives the running percentage.
                let round_summary_budget_chars = compaction
                    .stats
                    .budget_tokens
                    .saturating_sub(compaction.stats.compacted_estimated_tokens)
                    .saturating_sub(96)
                    .min(1_024)
                    .saturating_mul(3);
                super::events::emit_compaction_progress(
                    &app,
                    &session_id,
                    &turn_id,
                    &round_compaction_id,
                    0,
                    processed_messages,
                    before_context_tokens,
                    round_summary_budget_chars,
                );
                let (requested, summary_usage) = enrich_context_checkpoint_with_model(
                    &client,
                    ProviderRequest::new(
                        &session_id,
                        &selected_model.id,
                        Vec::new(),
                        request_tools_for_round.to_vec(),
                    )
                    .with_system_prompt(system_prompt.clone())
                    .with_reasoning_effort(reasoning_effort.clone())
                    .with_native_reasoning_effort(native_reasoning_effort)
                    .with_reasoning_encrypted_content(!responses_reasoning_include_fallback)
                    .with_usage_reporting(model_supports_usage && !compatible_usage_fallback),
                    &messages,
                    &mut compaction,
                    &mut model_summary_attempted,
                    &state,
                    &cancellation,
                    |characters| {
                        super::events::emit_compaction_progress(
                            &app,
                            &session_id,
                            &turn_id,
                            &round_compaction_id,
                            characters,
                            processed_messages,
                            before_context_tokens,
                            round_summary_budget_chars,
                        )
                    },
                )
                .await;
                if requested {
                    model_request_count.set(model_request_count.get().saturating_add(1));
                }
                if let Some(summary_usage) = summary_usage {
                    merge_agent_usage(&mut usage, summary_usage);
                }
                if is_cancelled(&state, &session_id, &cancellation) {
                    return Err(cancellation_message(&cancellation_reason));
                }
            }
        }
        // P9-G：模型摘要不可用时沿用预置的确定性检查点，保证压缩结果稳定可复现。
        if compaction.compressed && compaction.summary.is_none() {
            if let Some((summary, _)) = planned_compaction_summary.as_ref() {
                compaction.summary = Some(summary.clone());
            }
        }
        // P9-G：本轮压缩之后、下一轮压缩之前，先把下一段历史的检查点摘要算好
        // 并记住；下一轮压缩直接复用，不必再生成一份措辞不同的新摘要。
        if compaction.compressed {
            planned_compaction_summary = agent_core::plan_compaction_checkpoint(
                &compaction.messages,
                round_context_budget,
                round_compaction_trigger,
                round_request_overhead,
            );
        }
        let context_compressed = compaction.compressed;
        let compaction_summary = compaction.summary;
        let compaction_checkpoint_payload = compaction.checkpoint_payload;
        let compaction_covered_message_id = compaction.covered_message_id;
        let compaction_status = compaction.status;
        let compaction_stats = compaction.stats;
        let compacted_messages = compaction.messages;
        if context_compressed {
            messages = compacted_messages;
            cache_context::retain_runtime(&mut messages, &runtime_context);
            let active_image_attachments =
                image_attachments_for_messages(&image_attachments, &messages);
            update_image_token_reserve(&active_image_attachments, &image_token_reserve);
            if compaction_summary.is_some() {
                checkpoint.summary = compaction_summary;
            }
            if compaction_checkpoint_payload.is_some() {
                checkpoint.payload = compaction_checkpoint_payload;
            }
            if compaction_covered_message_id.is_some() {
                checkpoint.covered_through_message_id = compaction_covered_message_id;
            }
            let after_context_tokens =
                estimate_context_tokens(&messages, &system_prompt, request_tools_for_round);
            emit_agent_context_for_turn(
                &app,
                json!({
                    "session_id": session_id,
                    "compaction_id": round_compaction_id,
                    "compaction_status": "completed",
                    "compaction_reason": "context_budget",
                    "before_context_tokens": before_context_tokens,
                    "after_context_tokens": after_context_tokens,
                    "reserved_output_tokens": round_output_reserve,
                    "compaction_result": compaction_status,
                    "compaction_stats": compaction_stats,
                    "contextCheckpoint": context_checkpoint_event_value(
                        &round_compaction_id,
                        checkpoint.summary.as_deref(),
                        checkpoint.payload.as_ref(),
                        checkpoint.covered_through_message_id.as_deref(),
                        &selected_model.id,
                    ),
                    "available_tokens": Some(available_agent_context_tokens(
                        context_window,
                        after_context_tokens,
                        round_output_reserve,
                    )),
                    "context_compressed": true,
                }),
                Some(&turn_id),
            );
            emit_agent_status_for_turn(
                &app,
                &session_id,
                "context_compacted",
                "工具结果与较早上下文压缩完成，准备继续",
                Some(&turn_id),
            );
        }
        let context_tokens =
            estimate_context_tokens(&messages, &system_prompt, request_tools_for_round);
        last_context_tokens.store(context_tokens as u64, Ordering::Relaxed);
        let context_window_limit = effective_agent_context_window(context_window);
        let reserved_output_tokens = agent_output_reserve(context_window, final_response_requested);
        // The provider's own charge for the previous request is the only value that proves
        // the window was exceeded; never trust the estimate alone here.
        if context_window_budget_exceeded(
            context_tokens,
            reported_request_tokens,
            reported_usage_compacted,
            reserved_output_tokens,
            context_window_limit,
        ) {
            let tool_tokens = serde_json::to_string(request_tools_for_round)
                .map(|value| estimate_agent_text_tokens(&value))
                .unwrap_or_default();
            let hard_message_budget = context_window_limit
                .saturating_sub(reserved_output_tokens)
                .saturating_sub(estimate_agent_text_tokens(&system_prompt))
                .saturating_sub(tool_tokens)
                .saturating_sub(image_token_reserve.load(Ordering::Relaxed))
                .max(1);
            let hard_request_overhead =
                estimate_agent_request_tokens(&[], &system_prompt, request_tools_for_round)
                    .saturating_add(image_token_reserve.load(Ordering::Relaxed));
            let hard_compaction = compact_messages_with_overhead(
                &messages,
                hard_message_budget,
                hard_request_overhead,
            );
            let hard_compacted = hard_compaction.compressed;
            let hard_compaction_summary = hard_compaction.summary;
            let hard_compaction_checkpoint_payload = hard_compaction.checkpoint_payload;
            let hard_covered_message_id = hard_compaction.covered_message_id;
            let hard_compaction_status = hard_compaction.status;
            let hard_compaction_stats = hard_compaction.stats;
            let mut hard_compacted_messages = hard_compaction.messages;
            cache_context::retain_runtime(&mut hard_compacted_messages, &runtime_context);
            let after_context_tokens = estimate_context_tokens(
                &hard_compacted_messages,
                &system_prompt,
                request_tools_for_round,
            );
            if hard_compacted
                && after_context_tokens.saturating_add(reserved_output_tokens)
                    <= context_window_limit
            {
                let compaction_id = format!("compaction-{}", uuid_suffix());
                emit_agent_context_for_turn(
                    &app,
                    json!({
                        "session_id": session_id,
                        "compaction_id": compaction_id,
                        "compaction_status": "running",
                        "compaction_reason": "context_window_budget",
                        "before_context_tokens": context_tokens,
                        "reserved_output_tokens": reserved_output_tokens,
                        "context_compressed": true,
                    }),
                    Some(&turn_id),
                );
                messages = hard_compacted_messages;
                cache_context::retain_runtime(&mut messages, &runtime_context);
                let active_image_attachments =
                    image_attachments_for_messages(&image_attachments, &messages);
                update_image_token_reserve(&active_image_attachments, &image_token_reserve);
                if hard_compaction_summary.is_some() {
                    checkpoint.summary = hard_compaction_summary;
                }
                if hard_compaction_checkpoint_payload.is_some() {
                    checkpoint.payload = hard_compaction_checkpoint_payload;
                }
                if hard_covered_message_id.is_some() {
                    checkpoint.covered_through_message_id = hard_covered_message_id;
                }
                last_context_tokens.store(after_context_tokens as u64, Ordering::Relaxed);
                emit_agent_context_for_turn(
                    &app,
                    json!({
                        "session_id": session_id,
                        "compaction_id": compaction_id,
                        "compaction_status": "completed",
                        "compaction_reason": "context_window_budget",
                        "before_context_tokens": context_tokens,
                        "after_context_tokens": after_context_tokens,
                        "reserved_output_tokens": reserved_output_tokens,
                        "compaction_result": hard_compaction_status,
                        "compaction_stats": hard_compaction_stats,
                        "contextCheckpoint": context_checkpoint_event_value(
                            &compaction_id,
                            checkpoint.summary.as_deref(),
                            checkpoint.payload.as_ref(),
                            checkpoint.covered_through_message_id.as_deref(),
                            &selected_model.id,
                        ),
                        "available_tokens": Some(available_agent_context_tokens(
                            context_window,
                            after_context_tokens,
                            reserved_output_tokens,
                        )),
                        "context_compressed": true,
                    }),
                    Some(&turn_id),
                );
                emit_agent_status_for_turn(
                    &app,
                    &session_id,
                    "compacting_context",
                    "上下文接近模型上限，已按本轮请求预算压缩后继续",
                    Some(&turn_id),
                );
                // The report described the payload that was just replaced. A second
                // reported-driven compaction could never converge, so spend the single
                // allowance and drop the now-stale figure.
                reported_usage_compacted = true;
                reported_request_tokens = None;
                continue;
            }
            let reported_note = reported_request_tokens
                .filter(|reported| {
                    reported.saturating_add(reserved_output_tokens) > context_window_limit
                })
                .map(|reported| {
                    format!(
                        "provider 上一次请求实际上报 {reported} tokens 已超过窗口，本地估算偏低不应再被视为安全。"
                    )
                })
                .unwrap_or_default();
            let message = format!(
                "上下文压缩后仍超过模型窗口（估算 {} / {} tokens{}），已停止发送超预算请求。请缩短输入、移除图片或选择更大上下文的模型。{}",
                context_tokens,
                context_window_limit,
                if context_window.is_none() { "，窗口为本地保守估算" } else { "" },
                reported_note,
            );
            emit_agent_context_for_turn(
                &app,
                json!({
                    "session_id": session_id,
                    "compaction_status": "failed",
                    "compaction_reason": "context_window_too_small",
                    "context_tokens": context_tokens,
                    "context_window": context_window,
                    "context_window_estimated": context_window_estimated,
                    "context_window_status": context_window_resolution.status,
                    "effective_context_window": context_window_resolution.effective_tokens,
                    "context_window_conservative": context_window_resolution.conservative,
                    "context_percent": context_window
                        .map(|window| context_usage_percent(context_tokens, window as usize)),
                    "reserved_output_tokens": reserved_output_tokens,
                    "available_tokens": Some(available_agent_context_tokens(
                        context_window,
                        context_tokens,
                        reserved_output_tokens,
                    )),
                    "context_compressed": true,
                }),
                Some(&turn_id),
            );
            end_tool_group(&session_id, &turn_id);
            return Err(format!("[context_limit] {message}"));
        }
        let system_prompt_tokens = estimate_agent_text_tokens(&system_prompt);
        let tool_definition_tokens =
            estimate_agent_request_tokens(&[], "", request_tools_for_round);
        emit_agent_context_for_turn(
            &app,
            json!({
                "session_id": session_id,
                "context_tokens": context_tokens,
                "system_prompt_tokens": system_prompt_tokens,
                "tool_definition_tokens": tool_definition_tokens,
                "model_id": selected_model.id,
                "provider_id": provider.id,
                "context_window": context_window,
                "context_window_estimated": context_window_estimated,
                "context_window_status": context_window_resolution.status,
                "effective_context_window": context_window_resolution.effective_tokens,
                "context_window_conservative": context_window_resolution.conservative,
                "context_percent": context_window
                    .map(|window| context_usage_percent(context_tokens, window as usize)),
                "reserved_output_tokens": reserved_output_tokens,
                "available_tokens": Some(available_agent_context_tokens(
                    context_window,
                    context_tokens,
                    reserved_output_tokens,
                )),
            }),
            Some(&turn_id),
        );
        // 续写被截断的长回答时按工具轮的额度给输出预算：收尾阶段那 2048 在「推理也算
        // output」的协议上可能被整轮思考吃光，正文一字未出（见 turn_recovery 的
        // thinking_only 分支，以及 2026-10-25 那次 model_response_missing）。
        // 用户（或在发现模型时由上游接口带回）声明的单轮输出上限最后夹一刀：
        // 内置预算按上下文窗口推算，对「窗口很大但单轮只能出几 K」的模型会请求过大
        // 的 max_tokens，上游要么报错要么静默截断。
        let max_output_tokens = agent_core::clamp_declared_output_tokens(
            if output_continuations > 0 {
                max_agent_output_tokens_continued_output(context_window, context_tokens)
            } else {
                max_agent_output_tokens(context_window, context_tokens, final_response_requested)
            },
            selected_model.max_output_tokens,
        );
        if max_output_tokens == 0 {
            let minimum_output_tokens = if final_response_requested {
                MIN_AGENT_FINAL_OUTPUT_TOKENS
            } else {
                MIN_AGENT_TOOL_OUTPUT_TOKENS
            };
            emit_agent_context_for_turn(
                &app,
                json!({
                    "session_id": session_id,
                    "compaction_status": "failed",
                    "compaction_reason": "output_budget_too_small",
                    "context_tokens": context_tokens,
                    "context_window": context_window_limit,
                    "reserved_output_tokens": reserved_output_tokens,
                    "minimum_output_tokens": minimum_output_tokens,
                    "context_compressed": true,
                }),
                Some(&turn_id),
            );
            end_tool_group(&session_id, &turn_id);
            return Err(format!(
                "上下文压缩后可用输出预算不足，至少需要 {minimum_output_tokens} tokens；请缩短输入、移除图片或选择更大上下文的模型。"
            ));
        }
        let request_image_attachments =
            image_attachments_for_messages(&image_attachments, &messages);
        debug_assert_eq!(
            image_token_reserve.load(Ordering::Relaxed),
            estimate_image_attachment_tokens(&request_image_attachments),
        );
        let provider_request = ProviderRequest::new(
            &session_id,
            &selected_model.id,
            messages.clone(),
            request_tools_for_round.to_vec(),
        )
        .with_system_prompt(system_prompt.clone())
        .with_image_attachments(request_image_attachments)
        .with_usage_reporting(model_supports_usage && !compatible_usage_fallback)
        .with_reasoning_encrypted_content(!responses_reasoning_include_fallback)
        .with_reasoning_effort(take_recovery_reasoning_effort(
            &mut reasoning_off_once,
            recovery_reasoning_effort.as_ref(),
            &reasoning_effort,
        ))
        .with_native_reasoning_effort(native_reasoning_effort)
        .with_max_tokens(max_output_tokens)
        .with_prompt_cache_key(prompt_cache_key.clone())
        .with_prompt_cache_retention(provider.prompt_cache_retention.clone());
        cache_context::diagnostic(
            &app,
            &session_id,
            &turn_id,
            client.config(),
            &provider_request,
            &mut cache_fingerprint,
        );
        let summary_prefix_candidate =
            agent_core::ContextSummaryPrefix::capture(client.config(), &provider_request);
        emit_agent_status_for_turn(
            &app,
            &session_id,
            "requesting_model",
            "正在等待模型响应",
            Some(&turn_id),
        );
        let model_request_started_at = std::time::Instant::now();
        model_round_index = model_round_index.saturating_add(1);
        crate::diagnostics::record(
            "info",
            "agent.model_request",
            "native.agent",
            json!({
                "session_id": session_id, "turn_id": turn_id,
                "round_index": model_round_index, "model_id": selected_model.id,
                "protocol": protocol, "final_only": final_response_requested,
                "generation_limit": max_output_tokens, "reasoning_effort": reasoning_effort,
                "tool_definitions": request_tools_for_round.len(),
                "execution_boundary": termination_reason,
            }),
        );
        let stream_result = if let Some(error) = pending_provider_startup_error.take() {
            Err(error)
        } else {
            model_request_count.set(model_request_count.get().saturating_add(1));
            let provider_wait_started = tokio::time::Instant::now();
            let mut provider_stream = Box::pin(client.stream(provider_request));
            loop {
                tokio::select! {
                    _ = wait_for_agent_cancellation(&state, &session_id, &cancellation) => {
                        return Err(cancellation_message(&cancellation_reason));
                    }
                    result = &mut provider_stream => break result,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(10)) => {
                        let waited = provider_wait_started.elapsed().as_secs();
                        emit_agent_status_for_turn(
                            &app,
                            &session_id,
                            "requesting_model",
                            format!("正在等待模型服务的首个响应（已等待 {waited} 秒）"),
                            Some(&turn_id),
                        );
                    }
                }
            }
        };
        let events = match stream_result {
            Ok(events) => events,
            Err(error)
                if matches!(
                    protocol,
                    ProviderKind::OpenAi
                        | ProviderKind::OpenAiCompatible
                        | ProviderKind::DeepSeek
                        | ProviderKind::Kimi
                        | ProviderKind::Glm
                        | ProviderKind::MiMo
                ) && model_supports_usage
                    && !compatible_usage_fallback
                    && provider_usage_option_rejected(&error.to_string()) =>
            {
                compatible_usage_fallback = true;
                emit_agent_status_for_turn(
                    &app,
                    &session_id,
                    "requesting_model",
                    "中转不支持 usage 扩展，已关闭该扩展重试",
                    Some(&turn_id),
                );
                continue;
            }
            Err(error)
                if protocol == ProviderKind::OpenAiResponses
                    && !responses_reasoning_include_fallback
                    && responses_reasoning_include_rejected(&error.to_string()) =>
            {
                responses_reasoning_include_fallback = true;
                emit_agent_status_for_turn(
                    &app,
                    &session_id,
                    "requesting_model",
                    "中转不支持加密推理扩展，已关闭该可选字段重试",
                    Some(&turn_id),
                );
                continue;
            }
            Err(error)
                if provider_error_is_context_limit(&error.to_string())
                    && provider_context_retry_count < AGENT_PROVIDER_CONTEXT_RETRY_LIMIT =>
            {
                let before_context_tokens =
                    estimate_context_tokens(&messages, &system_prompt, request_tools_for_round);
                provider_context_retry_count += 1;
                let (tighter_budget, retry_target_window) = if context_window.is_none() {
                    let Some(retry_budget) = agent_core::unknown_context_retry_budget(
                        provider_context_retry_count,
                        &system_prompt,
                        request_tools_for_round,
                        image_token_reserve.load(Ordering::Relaxed),
                        final_response_requested,
                    ) else {
                        emit_agent_context_for_turn(
                            &app,
                            json!({
                                "session_id": session_id,
                                "compaction_status": "failed",
                                "compaction_reason": "fixed_context_overhead_too_large",
                                "before_context_tokens": before_context_tokens,
                                "reserved_output_tokens": reserved_output_tokens,
                                "context_window_estimated": true,
                            }),
                            Some(&turn_id),
                        );
                        return Err(format!(
                            "[context_limit] Provider 上下文上限低于当前系统提示、工具定义、图片和输出预留的固定开销，无法安全重试：{error}"
                        ));
                    };
                    (
                        retry_budget.message_budget,
                        retry_budget.target_context_window,
                    )
                } else {
                    let compaction_percent = match provider_context_retry_count {
                        1 => 60,
                        2 => 40,
                        _ => 25,
                    };
                    (
                        round_context_budget
                            .saturating_mul(compaction_percent)
                            .saturating_div(100)
                            .max(1)
                            .min(context_budget),
                        context_window
                            .map(|value| value as usize)
                            .unwrap_or_default(),
                    )
                };
                let provider_compaction = compact_messages_with_overhead(
                    &messages,
                    tighter_budget.saturating_sub(runtime_context_tokens).max(1),
                    estimate_agent_request_tokens(&[], &system_prompt, request_tools_for_round)
                        .saturating_add(image_token_reserve.load(Ordering::Relaxed)),
                );
                let compacted = provider_compaction.compressed;
                let compaction_budget_satisfied = provider_compaction.stats.budget_satisfied;
                let compaction_summary = provider_compaction.summary;
                let compaction_checkpoint_payload = provider_compaction.checkpoint_payload;
                let compaction_covered_message_id = provider_compaction.covered_message_id;
                let compaction_status = provider_compaction.status;
                let compaction_stats = provider_compaction.stats;
                let compacted_messages = provider_compaction.messages;
                let compaction_id = format!("compaction-{}", uuid_suffix());
                if compacted && compaction_budget_satisfied {
                    emit_agent_context_for_turn(
                        &app,
                        json!({
                            "session_id": session_id,
                            "compaction_id": compaction_id,
                            "compaction_status": "running",
                            "compaction_reason": "provider_context_limit_retry",
                            "before_context_tokens": before_context_tokens,
                            "reserved_output_tokens": reserved_output_tokens,
                            "context_compressed": true,
                        }),
                        Some(&turn_id),
                    );
                    messages = compacted_messages;
                    cache_context::retain_runtime(&mut messages, &runtime_context);
                    let active_image_attachments =
                        image_attachments_for_messages(&image_attachments, &messages);
                    update_image_token_reserve(&active_image_attachments, &image_token_reserve);
                    if compaction_summary.is_some() {
                        checkpoint.summary = compaction_summary;
                    }
                    if compaction_checkpoint_payload.is_some() {
                        checkpoint.payload = compaction_checkpoint_payload;
                    }
                    if compaction_covered_message_id.is_some() {
                        checkpoint.covered_through_message_id = compaction_covered_message_id;
                    }
                    let after_context_tokens =
                        estimate_context_tokens(&messages, &system_prompt, request_tools_for_round);
                    emit_agent_context_for_turn(
                        &app,
                        json!({
                            "session_id": session_id,
                            "compaction_id": compaction_id,
                            "compaction_status": "completed",
                            "compaction_reason": "provider_context_limit_retry",
                            "before_context_tokens": before_context_tokens,
                            "after_context_tokens": after_context_tokens,
                            "reserved_output_tokens": reserved_output_tokens,
                            "compaction_result": compaction_status,
                            "compaction_stats": compaction_stats,
                            "contextCheckpoint": context_checkpoint_event_value(
                                &compaction_id,
                                checkpoint.summary.as_deref(),
                                checkpoint.payload.as_ref(),
                                checkpoint.covered_through_message_id.as_deref(),
                                &selected_model.id,
                            ),
                            "available_tokens": available_agent_context_tokens(
                                context_window,
                                after_context_tokens,
                                reserved_output_tokens,
                            ),
                            "context_compressed": true,
                        }),
                        Some(&turn_id),
                    );
                    emit_agent_status_for_turn(
                        &app,
                        &session_id,
                        "compacting_context",
                        "Provider 报告上下文超限，已压缩后重试",
                        Some(&turn_id),
                    );
                    continue;
                }
                if compacted && !compaction_budget_satisfied {
                    emit_agent_context_for_turn(
                        &app,
                        json!({
                            "session_id": session_id,
                            "compaction_id": compaction_id,
                            "compaction_status": "failed",
                            "compaction_reason": "protected_context_exceeds_budget",
                            "before_context_tokens": before_context_tokens,
                            "target_context_window": retry_target_window,
                            "message_budget": tighter_budget,
                            "compaction_stats": compaction_stats,
                            "context_compressed": true,
                        }),
                        Some(&turn_id),
                    );
                    return Err(format!(
                        "[context_limit] 当前请求与最新工具调用组的最小完整结构仍超过安全重试预算，已停止以避免拆坏工具上下文：{error}"
                    ));
                }
                if context_window.is_none()
                    && provider_context_retry_count < AGENT_PROVIDER_CONTEXT_RETRY_LIMIT
                {
                    // The first adaptive target may still be larger than the
                    // provider's real limit (for example an 80K request on a
                    // 64K model). Advance to the next target instead of
                    // failing before a compaction opportunity exists.
                    emit_agent_status_for_turn(
                        &app,
                        &session_id,
                        "compacting_context",
                        format!(
                            "Provider 报告上下文超限，正在尝试更小窗口（{}K）",
                            retry_target_window / 1024
                        ),
                        Some(&turn_id),
                    );
                    continue;
                }
                emit_agent_context_for_turn(
                    &app,
                    json!({
                        "session_id": session_id,
                        "compaction_id": compaction_id,
                        "compaction_status": "failed",
                        "compaction_reason": "provider_context_limit",
                        "before_context_tokens": before_context_tokens,
                        "reserved_output_tokens": reserved_output_tokens,
                        "context_compressed": false,
                    }),
                    Some(&turn_id),
                );
                return Err(format!("[{}] {}", error.code(), error));
            }
            Err(error) => {
                let message = error.to_string();
                if provider_error_is_context_limit(&message) {
                    let compaction_id = format!("compaction-{}", uuid_suffix());
                    emit_agent_context_for_turn(
                        &app,
                        json!({
                            "session_id": session_id,
                            "compaction_id": compaction_id,
                            "compaction_status": "failed",
                            "compaction_reason": "provider_context_limit",
                            "before_context_tokens": context_tokens,
                            "reserved_output_tokens": reserved_output_tokens,
                            "context_compressed": false,
                        }),
                        Some(&turn_id),
                    );
                }
                return Err(format!("[{}] {message}", error.code()));
            }
        };
        // Native tool protocols can interleave commentary and calls in one
        // provider response. Hold only the current text segment until its
        // boundary is known, then emit it immediately before the following
        // call. This keeps the persisted event stream in the same order the
        // model produced it while still suppressing JSON-only tool payloads.
        let mut deferred_timeline_text = Vec::<AgentEvent>::new();
        let mut streamed_ascii_characters = 0usize;
        let mut streamed_non_ascii_characters = 0usize;
        let mut provider_stream_context_tokens = None::<usize>;
        // The provider's own *input* count for this request (no output), kept
        // apart from `provider_stream_context_tokens` so the compaction trigger
        // can be calibrated against a like-for-like comparison with the estimate.
        let mut provider_reported_input_tokens = None::<usize>;
        let mut last_stream_context_emit_tokens = context_tokens;
        let mut last_stream_context_emit_at = std::time::Instant::now();
        let tool_group_id = format!("tool-group:{}", uuid_suffix());
        let round_result = match collect_model_round_with_observer(
            events,
            &session_id,
            &mut dsml_stream,
            &mut steer_receiver,
            &cancellation,
            &request_tools_for_round
                .iter()
                .map(|tool| tool.name.clone())
                .collect::<Vec<_>>(),
            |event| {
                let streamed_delta = match event {
                    AgentEvent::TextDelta { text, .. }
                    | AgentEvent::ReasoningDelta { text, .. } => Some(text.as_str()),
                    AgentEvent::ToolCallInputDelta { delta, .. } => Some(delta.as_str()),
                    _ => None,
                };
                if let Some(delta) = streamed_delta {
                    for character in delta.chars() {
                        if character.is_ascii() {
                            streamed_ascii_characters = streamed_ascii_characters.saturating_add(1);
                        } else {
                            streamed_non_ascii_characters =
                                streamed_non_ascii_characters.saturating_add(1);
                        }
                    }
                }

                let mut provider_usage_received = false;
                if let AgentEvent::Usage {
                    input_tokens,
                    output_tokens,
                    ..
                } = event
                {
                    let provider_input_total = match event {
                        AgentEvent::Usage {
                            cached_input_tokens,
                            cache_creation_input_tokens,
                            ..
                        } => provider_capabilities.cache_input_total(
                            *input_tokens,
                            *cached_input_tokens,
                            *cache_creation_input_tokens,
                        ),
                        _ => None,
                    };
                    let estimated_output_tokens = streamed_ascii_characters
                        .saturating_add(3)
                        .saturating_div(4)
                        .saturating_add(streamed_non_ascii_characters);
                    provider_reported_input_tokens = provider_input_total
                        .map(|input| usize::try_from(input).unwrap_or(usize::MAX));
                    provider_stream_context_tokens = match (provider_input_total, *output_tokens) {
                        (Some(input), Some(output)) => Some(
                            usize::try_from(input.saturating_add(output)).unwrap_or(usize::MAX),
                        ),
                        (Some(input), None) => Some(
                            usize::try_from(input)
                                .unwrap_or(usize::MAX)
                                .saturating_add(estimated_output_tokens),
                        ),
                        (None, Some(output)) => Some(
                            context_tokens
                                .saturating_add(usize::try_from(output).unwrap_or(usize::MAX)),
                        ),
                        (None, None) => None,
                    };
                    provider_usage_received = provider_stream_context_tokens.is_some();
                }

                let estimated_output_tokens = streamed_ascii_characters
                    .saturating_add(3)
                    .saturating_div(4)
                    .saturating_add(streamed_non_ascii_characters);
                let streaming_context_tokens = provider_stream_context_tokens
                    .unwrap_or_else(|| context_tokens.saturating_add(estimated_output_tokens));
                last_context_tokens.store(streaming_context_tokens as u64, Ordering::Relaxed);
                let token_delta =
                    streaming_context_tokens.abs_diff(last_stream_context_emit_tokens);
                let snapshot_due = provider_usage_received
                    || (streaming_context_tokens != last_stream_context_emit_tokens
                        && (token_delta >= 64
                            || last_stream_context_emit_at.elapsed()
                                >= std::time::Duration::from_millis(250)));
                if snapshot_due {
                    emit_streaming_context_snapshot(
                        &app,
                        &session_id,
                        &turn_id,
                        streaming_context_tokens,
                        provider_stream_context_tokens.is_none(),
                        context_window,
                        reserved_output_tokens,
                        None,
                        None,
                        system_prompt_tokens,
                        tool_definition_tokens,
                        &selected_model.id,
                        &provider.id,
                        Some(round_compaction_trigger),
                    );
                    last_stream_context_emit_tokens = streaming_context_tokens;
                    last_stream_context_emit_at = std::time::Instant::now();
                }

                match event {
                    AgentEvent::TextDelta { text: delta, .. } if !delta.is_empty() => {
                        deferred_timeline_text.push(event.clone());
                        if !text_segment_may_be_plain_tool_payload(&deferred_timeline_text) {
                            timeline_segment = Some(TimelineSegment::Commentary);
                            flush_deferred_timeline_text(
                                &app,
                                &turn_id,
                                &mut deferred_timeline_text,
                            );
                        }
                    }
                    AgentEvent::ReasoningDelta { .. } | AgentEvent::Status { .. } => {
                        emit_agent_event_for_turn(&app, event, Some(&turn_id));
                    }
                    AgentEvent::ToolCallInputDelta { .. } => {
                        emit_model_agent_event_for_turn(&app, event, &turn_id, &tool_group_id);
                    }
                    AgentEvent::ToolCallStarted {
                        tool_name, input, ..
                    } => {
                        flush_deferred_timeline_text(&app, &turn_id, &mut deferred_timeline_text);
                        if let Some(commentary) = take_tool_group_fallback(
                            &mut timeline_segment,
                            tool_name,
                            input,
                            request.requested_tool.as_deref(),
                        ) {
                            // 运行时兜底旁白必须走思考通道（reasoning_delta），
                            // 不能走 text_delta。走 text_delta 时它和模型自己写的
                            // 正文用同一个事件类型，前端无法区分，于是被当成正文
                            // 段落渲染出来，用户看到的就是「我先运行一个受控命令，
                            // 获取需要的结果。」这类系统自述刷屏。
                            emit_agent_payload_for_turn(
                                &app,
                                json!({
                                    "type": "reasoning_delta",
                                    "session_id": session_id,
                                    "text": commentary,
                                    "origin": "runtime_fallback",
                                }),
                                &turn_id,
                            );
                        }
                        emit_model_agent_event_for_turn(&app, event, &turn_id, &tool_group_id);
                    }
                    _ => {}
                }
            },
        )
        .await
        {
            Ok(result) => result,
            Err(ModelRoundError::Cancelled { mut pending_calls }) => {
                flush_deferred_timeline_text(&app, &turn_id, &mut deferred_timeline_text);
                let reason = cancellation_message(&cancellation_reason);
                finish_pending_agent_tool_calls(
                    &app,
                    &session_id,
                    &turn_id,
                    default_environment,
                    &windows_paths,
                    &ssh_paths,
                    &wsl_paths,
                    &mut pending_calls,
                    "cancelled",
                    &reason,
                    true,
                );
                return Err(reason);
            }
            Err(ModelRoundError::Provider {
                error,
                mut pending_calls,
            }) => {
                let message = error.to_string();
                if streamed_ascii_characters == 0
                    && streamed_non_ascii_characters == 0
                    && pending_calls.is_empty()
                    && provider_startup_error_can_recover(
                        &message,
                        protocol,
                        model_supports_usage,
                        compatible_usage_fallback,
                        responses_reasoning_include_fallback,
                        provider_context_retry_count,
                    )
                {
                    // HTTP errors arrive while polling the lazy stream. Route
                    // pre-output failures through the same bounded recovery as
                    // stream creation, retaining images until a round succeeds.
                    pending_provider_startup_error = Some(error);
                    continue;
                }
                flush_deferred_timeline_text(&app, &turn_id, &mut deferred_timeline_text);
                finish_pending_agent_tool_calls(
                    &app,
                    &session_id,
                    &turn_id,
                    default_environment,
                    &windows_paths,
                    &ssh_paths,
                    &wsl_paths,
                    &mut pending_calls,
                    error.code(),
                    &message,
                    matches!(error.code(), "network" | "rate_limited" | "server_failure"),
                );
                return Err(format!("[{}] {message}", error.code()));
            }
        };
        let round_finished_at = std::time::Instant::now();
        remember_context_summary_prefix(
            &session_id,
            summary_prefix_candidate,
            round_result
                .events
                .iter()
                .any(|event| !matches!(event, AgentEvent::Status { .. })),
        );
        let final_stream_context_tokens = provider_stream_context_tokens.unwrap_or_else(|| {
            context_tokens
                .saturating_add(
                    streamed_ascii_characters
                        .saturating_add(3)
                        .saturating_div(4),
                )
                .saturating_add(streamed_non_ascii_characters)
        });
        last_context_tokens.store(final_stream_context_tokens as u64, Ordering::Relaxed);
        if provider_stream_context_tokens.is_some() {
            // The provider reported what it actually charged for this request (input plus
            // the output it billed). Keep the largest report of the turn so the next
            // request is guarded by a measured value, not only by the estimate.
            reported_request_tokens = Some(
                reported_request_tokens
                    .unwrap_or(0)
                    .max(final_stream_context_tokens),
            );
        }
        if let Some(reported_input) = provider_reported_input_tokens {
            // `context_tokens` is the local estimate of the very request that
            // produced this report, so the two are directly comparable.
            // P8-C：单次原始观测（clamp 100-400，向后兼容诊断）继续记录；
            // 触发点改用 per-(session, model) 的 EMA 校准值——双向修正
            //（旧实现只缩不放，估算偏大时永不回调），阻尼+限幅防单次异常
            // 跳变，爆窗由 provider 超窗硬压兜底。
            context_charge_percent = Some(
                reported_input
                    .saturating_mul(100)
                    .saturating_div(context_tokens.max(1))
                    .clamp(100, 400),
            );
            let calibrator_key = format!("{session_id}:{}", selected_model.id);
            if let Ok(mut calibrators) = state.token_calibrators.lock() {
                calibrators
                    .entry(calibrator_key)
                    .or_default()
                    .observe(reported_input as u64, context_tokens.max(1) as u64);
            }
        }
        if final_stream_context_tokens != last_stream_context_emit_tokens {
            emit_streaming_context_snapshot(
                &app,
                &session_id,
                &turn_id,
                final_stream_context_tokens,
                provider_stream_context_tokens.is_none(),
                context_window,
                reserved_output_tokens,
                None,
                None,
                system_prompt_tokens,
                tool_definition_tokens,
                &selected_model.id,
                &provider.id,
                None,
            );
        }
        let round_text = round_result.text.clone();
        crate::diagnostics::record(
            "info",
            "agent.model_round",
            "native.agent",
            json!({
                "session_id": session_id, "turn_id": turn_id, "model_id": selected_model.id,
                "round_index": model_round_index,
                "protocol": protocol, "output_tokens": round_result.output_tokens,
                "output_truncated": round_result.output_truncated,
                "tool_call_count": round_result.pending_calls.len(),
                "response_characters": round_text.chars().count(),
                "reasoning_characters": round_result.reasoning_content.chars().count(),
                "responses_reasoning_items": round_result.responses_reasoning_items.len(),
                "anthropic_thinking_blocks": round_result.anthropic_thinking_blocks.len(),
                "final_only": final_response_requested,
                "execution_boundary": termination_reason,
                "tool_definitions": request_tools_for_round.len(),
                "duration_ms": round_finished_at.duration_since(model_request_started_at).as_millis().min(u64::MAX as u128) as u64,
                "interruption_code": round_result.interruption.as_ref().map(|i| &i.code),
            }),
        );
        let round_output_truncated = round_result.output_truncated;
        let reasoning_content = round_result.reasoning_content;
        let round_output_tokens = round_result.output_tokens;
        generation_metrics = generation_metrics.with_request(
            round_output_tokens,
            round_finished_at
                .duration_since(model_request_started_at)
                .as_millis()
                .min(u64::MAX as u128) as u64,
        );
        emit_agent_payload_for_turn(
            &app,
            json!({
                "type": "usage", "session_id": session_id,
                "generation_tokens": generation_metrics.tokens,
                "generation_duration_ms": generation_metrics.duration_ms,
                "generation_timing_source": "request",
            }),
            &turn_id,
        );
        let round_interruption = round_result.interruption.clone();
        text.push_str(&round_result.text);
        if round_result.synthesized_plain_tool_calls {
            // The core converted an ordinary-text JSON array to safe tool
            // calls. That JSON must never enter the conversation timeline.
            deferred_timeline_text.clear();
            timeline_segment = None;
        } else {
            flush_deferred_timeline_text(&app, &turn_id, &mut deferred_timeline_text);
        }
        for event in &round_result.events {
            let already_emitted = match event {
                AgentEvent::ReasoningDelta { .. }
                | AgentEvent::TextDelta { .. }
                | AgentEvent::Status { .. } => true,
                AgentEvent::ToolCallStarted {
                    tool_name, input, ..
                } if round_result.synthesized_plain_tool_calls => {
                    if let Some(commentary) = take_tool_group_fallback(
                        &mut timeline_segment,
                        tool_name,
                        input,
                        request.requested_tool.as_deref(),
                    ) {
                        // 同上游流式路径：兜底旁白属于系统自述，走思考通道。
                        emit_agent_payload_for_turn(
                            &app,
                            json!({
                                "type": "reasoning_delta",
                                "session_id": session_id,
                                "text": commentary,
                                "origin": "runtime_fallback",
                            }),
                            &turn_id,
                        );
                    }
                    emit_model_agent_event_for_turn(&app, event, &turn_id, &tool_group_id);
                    true
                }
                AgentEvent::ToolCallInputDelta { .. }
                    if round_result.synthesized_plain_tool_calls =>
                {
                    emit_model_agent_event_for_turn(&app, event, &turn_id, &tool_group_id);
                    true
                }
                AgentEvent::ToolCallStarted { .. }
                | AgentEvent::ToolCallInputDelta { .. }
                | AgentEvent::Usage { .. } => true,
                _ => false,
            };
            if !already_emitted {
                emit_agent_event_for_turn(&app, event, Some(&turn_id));
            }
        }
        settle_thread_process(&app, &session_id, &turn_id);
        let mut pending_calls = round_result.pending_calls;
        let mut steer_message = round_result.steered;
        let latest_request_usage =
            agent_core::RequestCacheUsage::from_usage(protocol, round_result.usage.as_ref());
        if let Some(mut round_usage) = round_result.usage {
            round_usage.latest_request_usage = Some(latest_request_usage.clone());
            merge_agent_usage(&mut usage, round_usage);
            if let Some(total) = usage.as_ref() {
                emit_agent_payload_for_turn(
                    &app,
                    json!({
                        "type": "usage", "session_id": session_id,
                        "provider_kind": protocol,
                        "input_tokens": total.input_tokens, "output_tokens": total.output_tokens,
                        "cached_input_tokens": total.cached_input_tokens,
                        "cache_creation_input_tokens": total.cache_creation_input_tokens,
                        "reasoning_tokens": total.reasoning_tokens,
                        "latestRequestUsage": latest_request_usage,
                    }),
                    &turn_id,
                );
            }
        } else {
            if let Some(total) = usage.as_mut() {
                total.latest_request_usage = Some(latest_request_usage.clone());
            }
            emit_agent_payload_for_turn(
                &app,
                json!({
                    "type": "usage", "session_id": session_id,
                    "latestRequestUsage": latest_request_usage,
                }),
                &turn_id,
            );
        }
        if let Some(interruption) = round_interruption {
            finish_pending_agent_tool_calls(
                &app,
                &session_id,
                &turn_id,
                default_environment,
                &windows_paths,
                &ssh_paths,
                &wsl_paths,
                &mut pending_calls,
                &interruption.code,
                &interruption.message,
                interruption.retryable,
            );
            emit_agent_status_for_turn(
                &app,
                &session_id,
                &interruption.code,
                interruption.message,
                Some(&turn_id),
            );
            termination_reason = "repetitive_output";
            end_tool_group(&session_id, &turn_id);
            break;
        }
        if pending_calls.is_empty() && steer_message.is_none() {
            steer_message = steer_receiver.try_recv().ok();
        }
        if let Some(steer) = steer_message.take() {
            if is_cancelled(&state, &session_id, &cancellation) {
                finish_pending_agent_tool_calls(
                    &app,
                    &session_id,
                    &turn_id,
                    default_environment,
                    &windows_paths,
                    &ssh_paths,
                    &wsl_paths,
                    &mut pending_calls,
                    "cancelled",
                    "请求已取消",
                    true,
                );
                return Err(cancellation_message(&cancellation_reason));
            }
            // The completed assistant message must precede the new user input.
            // Tool groups are steered only after all results have been paired below.
            messages.push(AgentMessage {
                role: MessageRole::Assistant,
                content: round_text.clone(),
                reasoning_content: (!reasoning_content.is_empty())
                    .then(|| reasoning_content.clone()),
                responses_reasoning_items: round_result.responses_reasoning_items.clone(),
                anthropic_thinking_blocks: round_result.anthropic_thinking_blocks.clone(),
                anthropic_content_blocks: round_result.anthropic_content_blocks.clone(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                source_message_id: None,
            });
            apply_steer_runtime_inputs(
                &attachment_root,
                &steer,
                &mut messages,
                &mut windows_paths,
                &mut tool_policy,
                model_supports_tools,
                explicitly_disabled,
                selected_model.supports_vision,
                &mut image_attachments,
                &image_token_reserve,
            )?;
            if mark_agent_steer_applied(&app, &session_id, &steer) {
                if let Err(error) = replay_pending_agent_steers(&app, &state, &session_id) {
                    eprintln!("无法继续投递持久化追加要求: {error}");
                }
            }
            tool_cancellation = Arc::new(AtomicBool::new(false));
            drop(plugin_turn);
            plugin_turn = crate::plugin_host::prepare_turn(
                &session_id,
                &turn_id,
                if tool_policy == AgentToolPolicy::Disabled {
                    &[]
                } else {
                    &windows_paths
                },
                permission_mode,
            )
            .await;
            (registry, approved_path_registry) = build_environment_tool_registries(
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
                &bound_ssh_hosts,
                &app,
                &session_id,
                &turn_id,
                (tool_policy == AgentToolPolicy::Auto)
                    .then_some(collaboration_tool.as_deref())
                    .flatten(),
            )?;
            request_tools = registry.list_public().into_iter().cloned().collect();
            let plugin_instruction = if request_tools.is_empty() {
                String::new()
            } else {
                plugin_turn.prompt(default_environment == AgentToolEnvironment::Windows)
            };
            runtime_context_tokens = cache_context::update_runtime(
                &mut messages,
                &mut runtime_context,
                compose_turn_prompt(
                    tool_policy,
                    &request_tools,
                    &plugin_instruction,
                    remote_context.as_ref().map(|context| context.root()),
                )
                .runtime,
            );
            if is_cancelled(&state, &session_id, &cancellation) {
                return Err(cancellation_message(&cancellation_reason));
            }
            text.clear();
            final_response_requested = false;
            final_phase_notice_pushed = false;
            messages.retain(|message| {
                !(message.role == MessageRole::System && message.content == FINAL_PHASE_NOTICE)
            });
            termination_reason = "model_finished";
            tool_rounds = 0;
            output_continuations = 0;
            response_recovery = Default::default();
            tool_loop_progress = Default::default();
            provider_context_retry_count = 0;
            malformed_tool_signatures.clear();
            tool_call_ledger.clear();
            dsml_stream.buffer.clear();
            let mut steer_payload = json!({
                "type": "steered",
                "session_id": session_id,
                "message": steer.message,
                "attachments": steer.attachments,
                "artifacts": steer.artifacts,
            });
            if let Value::Object(object) = &mut steer_payload {
                object.insert("turn_id".to_string(), Value::String(turn_id.clone()));
            }
            emit_agent_payload_for_turn(&app, steer_payload, &turn_id);
            end_tool_group(&session_id, &turn_id);
            continue;
        }
        if is_cancelled(&state, &session_id, &cancellation) {
            finish_pending_agent_tool_calls(
                &app,
                &session_id,
                &turn_id,
                default_environment,
                &windows_paths,
                &ssh_paths,
                &wsl_paths,
                &mut pending_calls,
                "cancelled",
                "请求已取消",
                true,
            );
            return Err(cancellation_message(&cancellation_reason));
        }
        let output_was_limited = round_output_truncated
            || output_limit_reached(round_output_tokens, max_output_tokens, &round_text);
        if pending_calls.is_empty() && output_was_limited {
            if output_continuations < AGENT_OUTPUT_CONTINUATION_LIMIT {
                messages.push(AgentMessage {
                    role: MessageRole::Assistant,
                    content: round_text.clone(),
                    reasoning_content: (!reasoning_content.is_empty())
                        .then(|| reasoning_content.clone()),
                    responses_reasoning_items: round_result.responses_reasoning_items.clone(),
                    anthropic_thinking_blocks: round_result.anthropic_thinking_blocks.clone(),
                    anthropic_content_blocks: round_result.anthropic_content_blocks.clone(),
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                    source_message_id: None,
                });
                messages.push(AgentMessage::system(
                    "上一段回答达到单轮输出上限，内容可能尚未结束。请从中断位置继续输出，不要重复已经完成的内容，不要停下来重新规划或长时间思考，不要调用工具。",
                ));
                output_continuations += 1;
                final_response_requested = true;
                emit_agent_status_for_turn(
                    &app,
                    &session_id,
                    "continuing_output",
                    format!(
                        "回答达到单轮输出上限，正在继续生成（{}/{}）",
                        output_continuations, AGENT_OUTPUT_CONTINUATION_LIMIT
                    ),
                    Some(&turn_id),
                );
                end_tool_group(&session_id, &turn_id);
                continue;
            }
            emit_agent_status_for_turn(
                &app,
                &session_id,
                "output_limit_reached",
                "回答达到连续输出上限，已保留当前已生成内容",
                Some(&turn_id),
            );
            termination_reason = "output_limit_reached";
        }
        // A final-only phase can still receive tool calls from a provider.
        // Pair those calls as unexecuted before retrying, never silently drop them.
        if (pending_calls.is_empty() && round_text.trim().is_empty())
            || (final_response_requested && !pending_calls.is_empty())
        {
            // Empty text, thinking-only output, and rejected final-phase calls
            // need distinct diagnostics; none proves that a tool execution failed.
            let recovery_reason = if final_response_requested && !pending_calls.is_empty() {
                "final_phase_tool_calls"
            } else if !reasoning_content.trim().is_empty()
                || !round_result.responses_reasoning_items.is_empty()
                || !round_result.anthropic_thinking_blocks.is_empty()
            {
                "reasoning_only"
            } else {
                "empty_response"
            };
            let rejected_call_count = pending_calls.len();
            let rejected_calls = if final_response_requested {
                pending_calls
                    .iter()
                    .map(|(id, (name, input, arguments))| AgentToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        input: parse_tool_call_input(input.clone(), arguments, name)
                            .unwrap_or_else(|_| input.clone()),
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let response = AgentMessage {
                role: MessageRole::Assistant,
                content: round_text.clone(),
                reasoning_content: (!reasoning_content.is_empty())
                    .then(|| reasoning_content.clone()),
                responses_reasoning_items: round_result.responses_reasoning_items.clone(),
                anthropic_thinking_blocks: round_result.anthropic_thinking_blocks.clone(),
                anthropic_content_blocks: round_result.anthropic_content_blocks.clone(),
                tool_call_id: None,
                tool_calls: rejected_calls,
                source_message_id: None,
            };
            finish_pending_agent_tool_calls(
                &app,
                &session_id,
                &turn_id,
                default_environment,
                &windows_paths,
                &ssh_paths,
                &wsl_paths,
                &mut pending_calls,
                "final_phase_tool_call",
                "当前仅整理已有结果，此工具未执行",
                false,
            );
            let retry = response_recovery.retry(
                &mut messages,
                response,
                final_response_requested,
                recovery_reason == "reasoning_only",
            );
            let retry_delay = response_recovery.retry_delay();
            crate::diagnostics::record(
                if retry { "warn" } else { "error" },
                "agent.model_response_recovery",
                "native.agent",
                json!({
                    "session_id": session_id, "turn_id": turn_id,
                    "model_id": selected_model.id, "protocol": protocol,
                    "round_index": model_round_index, "reason": recovery_reason,
                    "action": if retry { "retry" } else { "exhausted" },
                    "final_only": final_response_requested,
                    "execution_boundary": termination_reason,
                    "rejected_tool_calls": rejected_call_count,
                    "retry_attempt": response_recovery.attempts(),
                    "max_retries": agent_core::turn_recovery::MODEL_RESPONSE_RECOVERY_LIMIT,
                    "retry_delay_ms": if retry { retry_delay.as_millis() as u64 } else { 0 },
                    // 下一轮会不会把推理档位降下来：诊断里留下，方便事后对账。
                    "reasoning_override": if recovery_reason == "reasoning_only" {
                        recovery_reasoning_effort.clone()
                    } else {
                        None
                    },
                }),
            );
            if retry {
                // 提示词挡不住「只想不写」，所以再加一道物理开关：下一轮的推理档位降到
                // 最低（用完 `take_recovery_reasoning_effort` 立刻复位）。
                if recovery_reason == "reasoning_only" {
                    reasoning_off_once = recovery_reasoning_effort.is_some();
                }
                emit_agent_status_for_turn(
                    &app,
                    &session_id,
                    "recovering_model_response",
                    if recovery_reason == "reasoning_only" {
                        "模型这一轮只输出了思考过程，正在要求它直接输出正文；已执行的操作不会重复执行"
                    } else if final_response_requested {
                        "模型未给出最终回答，正在重试整理；已执行的操作不会重复执行"
                    } else {
                        "模型返回空回复，正在请求继续处理任务"
                    },
                    Some(&turn_id),
                );
                end_tool_group(&session_id, &turn_id);
                tokio::select! {
                    _ = wait_for_agent_cancellation(&state, &session_id, &cancellation) => {
                        return Err(cancellation_message(&cancellation_reason));
                    }
                    _ = tokio::time::sleep(retry_delay) => {}
                }
                continue;
            }
            termination_reason = "model_response_missing";
            break;
        }
        if pending_calls.is_empty() {
            let response = cache_context::final_assistant(
                &round_text,
                &reasoning_content,
                round_result.responses_reasoning_items,
                round_result.anthropic_thinking_blocks,
                round_result.anthropic_content_blocks,
            );
            // All provider protocols share recovery after event normalization.
            // Ask the model to continue an announcement, never invent a call.
            let recovery = response_recovery.retry_progress_only(
                &mut messages,
                &response,
                agent_core::turn_recovery::ProgressRecoveryContext {
                    tools_available: !request_tools_for_round.is_empty(),
                    final_only: final_response_requested,
                    output_truncated: output_was_limited,
                },
            );
            crate::diagnostics::record(
                "info",
                "agent.model_decision",
                "native.agent",
                json!({
                    "session_id": session_id, "turn_id": turn_id, "round_index": model_round_index,
                    "decision": match recovery {
                        agent_core::turn_recovery::ProgressRecovery::Retry => "continue_action_announcement",
                        agent_core::turn_recovery::ProgressRecovery::Exhausted => "action_recovery_exhausted",
                        agent_core::turn_recovery::ProgressRecovery::NotNeeded => "accept_final_answer",
                    },
                    "progress_recovery_attempts": response_recovery.progress_attempts(),
                    "output_truncated": output_was_limited, "final_only": final_response_requested,
                }),
            );
            match recovery {
                agent_core::turn_recovery::ProgressRecovery::Retry => {
                    emit_agent_status_for_turn(
                        &app,
                        &session_id,
                        "recovering_model_response",
                        "模型仅返回了进度说明，正在请求继续执行实际操作",
                        Some(&turn_id),
                    );
                    end_tool_group(&session_id, &turn_id);
                    continue;
                }
                agent_core::turn_recovery::ProgressRecovery::Exhausted => {
                    messages.push(response);
                    termination_reason = "model_action_missing";
                    break;
                }
                agent_core::turn_recovery::ProgressRecovery::NotNeeded => {}
            }
            messages.push(response);
            if final_response_requested && termination_reason == "model_finished" {
                termination_reason = "final_response";
            }
            break;
        }
        let mut final_response_reason = None;
        let mut retry_truncated_tool_call = false;
        let mut executed_tool = false;
        let mut skipped_duplicate = false;
        // The provider protocol requires one assistant message containing the
        // whole tool group, followed by one tool result per call. Do not push
        // an assistant message inside the execution loop below.
        let assistant_tool_calls = pending_calls
            .iter()
            .map(|(call_id, (tool_name, initial_input, arguments))| {
                let input = parse_tool_call_input(initial_input.clone(), arguments, tool_name)
                    .unwrap_or_else(|_| initial_input.clone());
                AgentToolCall {
                    id: call_id.clone(),
                    name: tool_name.clone(),
                    input,
                }
            })
            .collect::<Vec<_>>();
        messages.push(AgentMessage {
            role: MessageRole::Assistant,
            content: round_text.clone(),
            reasoning_content: (!reasoning_content.is_empty()).then(|| reasoning_content.clone()),
            responses_reasoning_items: round_result.responses_reasoning_items,
            anthropic_thinking_blocks: round_result.anthropic_thinking_blocks,
            anthropic_content_blocks: round_result.anthropic_content_blocks,
            tool_call_id: None,
            tool_calls: assistant_tool_calls,
            source_message_id: None,
        });
        let mut deferred_file_read_notices = Vec::new();
        let mut parallel_signatures = HashSet::new();
        let can_parallelize_group = pending_calls.len() > 1
            && tool_policy != AgentToolPolicy::Disabled
            && pending_calls
                .iter()
                .all(|(_, (tool_name, initial_input, arguments))| {
                    let Ok(input) =
                        parse_tool_call_input(initial_input.clone(), arguments, tool_name)
                    else {
                        return false;
                    };
                    let scope = tool_execution_scope(
                        tool_name,
                        default_environment,
                        &windows_paths,
                        &ssh_paths,
                        &wsl_paths,
                    );
                    let signature = tool_call_signature(tool_name, &input);
                    let read_only = agent_core::is_read_only_tool(tool_name, &input);
                    tool_call_can_run(&tool_call_ledger, &signature, read_only)
                        && parallel_signatures.insert(signature)
                        && agent_core::is_parallel_safe_tool_for_target(
                            tool_name,
                            &input,
                            permission_mode,
                            scope.paths,
                            &registry,
                            scope.is_remote(),
                        )
                });
        if can_parallelize_group {
            executed_tool = true;
            let parallel_calls = pending_calls
                .iter()
                .map(|(call_id, (tool_name, initial_input, arguments))| {
                    let input = parse_tool_call_input(initial_input.clone(), arguments, tool_name)
                        .expect("parallel tool calls were validated above");
                    begin_tool_call(
                        &mut tool_call_ledger,
                        tool_call_signature(tool_name, &input),
                    );
                    (call_id.clone(), tool_name.clone(), input)
                })
                .collect::<Vec<_>>();
            emit_agent_status_for_turn(
                &app,
                &session_id,
                "executing_tool",
                format!("正在并行执行 {} 个只读工具", parallel_calls.len()),
                Some(&turn_id),
            );
            let mut parallel_execution = futures_util::stream::FuturesUnordered::new();
            let execution_lanes = ToolExecutionLanes::new(MAX_PARALLEL_TOOL_EXECUTIONS);
            let mut parallel_started = Vec::with_capacity(parallel_calls.len());
            for (index, (call_id, tool_name, input)) in parallel_calls.iter().enumerate() {
                let task_session_id = session_id.clone();
                let task_turn_id = turn_id.clone();
                let task_group_id = tool_group_id.clone();
                let task_call_id = call_id.clone();
                let task_tool_name = tool_name.clone();
                let public_input = input.clone();
                let start_app = app.clone();
                let task_lanes = execution_lanes.clone();
                let task_cancellation = cancellation.clone();
                let execution_started = Arc::new(AtomicBool::new(false));
                parallel_started.push(execution_started.clone());
                let scope = tool_execution_scope(
                    tool_name,
                    default_environment,
                    &windows_paths,
                    &ssh_paths,
                    &wsl_paths,
                );
                let is_remote = scope.is_remote();
                let mut execution_input = input.clone();
                if let Value::Object(object) = &mut execution_input {
                    object.insert(
                        "__agent_call_id".to_string(),
                        Value::String(call_id.clone()),
                    );
                }
                let initial_registry =
                    if agent_core::path_access_requires_external_execution_for_target(
                        tool_name,
                        input,
                        scope.paths,
                        scope.is_remote(),
                    ) {
                        approved_path_registry.as_ref().unwrap_or(&registry)
                    } else {
                        &registry
                    };
                parallel_execution.push(async move {
                    let _permit = tokio::select! {
                        biased;
                        _ = async {
                            while !task_cancellation.load(Ordering::Relaxed) {
                                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                            }
                        } => return (index, None),
                        permit = task_lanes.acquire(is_remote) => permit,
                    };
                    if task_cancellation.load(Ordering::Relaxed) {
                        return (index, None);
                    }
                    let result = execute_tool_call_with_start(
                        initial_registry,
                        &AllowPermission,
                        &task_session_id,
                        &task_call_id,
                        &task_tool_name,
                        execution_input,
                        || {
                            execution_started.store(true, Ordering::Relaxed);
                            emit_tool_execution_started(
                                &start_app,
                                &task_session_id,
                                &task_turn_id,
                                &task_group_id,
                                &task_call_id,
                                &task_tool_name,
                                &public_input,
                            );
                        },
                    )
                    .await;
                    (index, Some(result))
                });
            }
            let mut ordered_messages = vec![None; parallel_calls.len()];
            let mut ordered_notices = vec![None; parallel_calls.len()];
            let mut settle_parallel =
                |index: usize, result: Result<agent_core::ToolCallResult, String>| {
                    let (call_id, tool_name, input) = &parallel_calls[index];
                    let scope = tool_execution_scope(
                        tool_name,
                        default_environment,
                        &windows_paths,
                        &ssh_paths,
                        &wsl_paths,
                    );
                    let mut result = normalize_tool_execution_result(
                        &session_id,
                        call_id,
                        tool_name,
                        input,
                        agent_core::is_read_only_tool(tool_name, input),
                        result,
                    );
                    result.attach_images(selected_model.supports_vision, &mut image_attachments);
                    update_image_token_reserve(&image_attachments, &image_token_reserve);
                    complete_tool_call(
                        &mut tool_call_ledger,
                        &tool_call_signature(tool_name, input),
                        &result.events,
                    );
                    ordered_notices[index] =
                        failed_file_read_instruction(tool_name, input, &result.events);
                    let failed = result.events.iter().any(|event| {
                        matches!(event, AgentEvent::ToolCallFinished { is_error: true, .. })
                    });
                    for event in &result.events {
                        emit_agent_event_for_turn_with_file_evidence_path_key(
                            &app,
                            event,
                            Some(&turn_id),
                            scope.paths,
                            scope.is_remote(),
                        );
                    }
                    ordered_messages[index] = Some(result.message);
                    emit_agent_status_for_turn(
                        &app,
                        &session_id,
                        "requesting_model",
                        if failed {
                            format!("{tool_name} 执行失败，正在让模型根据错误继续处理")
                        } else {
                            format!("{tool_name} 已完成，正在整理结果")
                        },
                        Some(&turn_id),
                    );
                };
            let mut cancellation_requested = false;
            while !parallel_execution.is_empty() {
                let next = tokio::select! {
                    biased;
                    _ = wait_for_agent_cancellation(&state, &session_id, &cancellation) => {
                        cancellation_requested = true;
                        None
                    }
                    result = parallel_execution.next() => result,
                };
                match next {
                    Some((index, Some(result))) => settle_parallel(index, result),
                    Some((_, None)) => {
                        cancellation_requested = true;
                        break;
                    }
                    None => break,
                }
            }
            if cancellation_requested {
                tool_cancellation.store(true, Ordering::Relaxed);
                let deadline = tokio::time::Instant::now() + TOOL_CANCELLATION_GRACE;
                while !parallel_execution.is_empty() {
                    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    match tokio::time::timeout(remaining, parallel_execution.next()).await {
                        Ok(Some((index, Some(result)))) => settle_parallel(index, result),
                        Ok(Some((_, None))) => {}
                        Ok(None) | Err(_) => break,
                    }
                }
                drop(settle_parallel);
                for (index, (call_id, tool_name, input)) in parallel_calls.iter().enumerate() {
                    if ordered_messages[index].is_some() {
                        continue;
                    }
                    let scope = tool_execution_scope(
                        tool_name,
                        default_environment,
                        &windows_paths,
                        &ssh_paths,
                        &wsl_paths,
                    );
                    let interrupted = parallel_started[index].load(Ordering::Relaxed);
                    let output = json!({
                        "error": { "code": "cancelled", "message": "请求已取消，工具未能完成", "retryable": true },
                        "code": "cancelled",
                        "retryable": true,
                        "interrupted": interrupted,
                        "skipped": !interrupted,
                    });
                    let event = AgentEvent::ToolCallFinished {
                        session_id: session_id.clone(),
                        call_id: call_id.clone(),
                        tool_name: tool_name.clone(),
                        input: input.clone(),
                        output,
                        is_error: true,
                    };
                    complete_tool_call(
                        &mut tool_call_ledger,
                        &tool_call_signature(tool_name, input),
                        std::slice::from_ref(&event),
                    );
                    emit_agent_event_for_turn_with_file_evidence_path_key(
                        &app,
                        &event,
                        Some(&turn_id),
                        scope.paths,
                        scope.is_remote(),
                    );
                }
                return Err(cancellation_message(&cancellation_reason));
            }
            drop(settle_parallel);
            messages.extend(ordered_messages.into_iter().flatten());
            deferred_file_read_notices.extend(ordered_notices.into_iter().flatten());
        } else {
            let mut serial_calls = pending_calls.into_iter();
            while let Some((call_id, (tool_name, initial_input, arguments))) = serial_calls.next() {
                let scope = tool_execution_scope(
                    &tool_name,
                    default_environment,
                    &windows_paths,
                    &ssh_paths,
                    &wsl_paths,
                );
                if is_cancelled(&state, &session_id, &cancellation) {
                    let mut unfinished = vec![(call_id, (tool_name, initial_input, arguments))];
                    unfinished.extend(serial_calls);
                    finish_pending_agent_tool_calls(
                        &app,
                        &session_id,
                        &turn_id,
                        default_environment,
                        &windows_paths,
                        &ssh_paths,
                        &wsl_paths,
                        &mut unfinished,
                        "cancelled",
                        &cancellation_message(&cancellation_reason),
                        true,
                    );
                    return Err(cancellation_message(&cancellation_reason));
                }
                let input =
                    match parse_tool_call_input(initial_input.clone(), &arguments, &tool_name) {
                        Ok(input) => input,
                        Err(output) => {
                            let malformed_signature = format!(
                                "{tool_name}:{}:{}",
                                arguments.len(),
                                arguments.chars().take(96).collect::<String>(),
                            );
                            messages.push(AgentMessage::tool(&call_id, output.to_string()));
                            emit_agent_event_for_turn_with_file_evidence_path_key(
                                &app,
                                &AgentEvent::ToolCallFinished {
                                    session_id: session_id.clone(),
                                    call_id,
                                    tool_name,
                                    input: initial_input.clone(),
                                    output,
                                    is_error: true,
                                },
                                Some(&turn_id),
                                scope.paths,
                                scope.is_remote(),
                            );
                            let first_malformed_attempt =
                                malformed_tool_signatures.insert(malformed_signature);
                            if output_was_limited && first_malformed_attempt {
                                retry_truncated_tool_call = true;
                            }
                            if !first_malformed_attempt {
                                final_response_reason =
                                    Some("同一工具参数连续不完整，已停止重复重试".to_string());
                            }
                            continue;
                        }
                    };
                if tool_policy == AgentToolPolicy::Disabled {
                    let reason = "本轮已禁止访问本地文件、日志和工具，未执行此调用".to_string();
                    let output = json!({ "skipped": true, "reason": reason.clone() });
                    messages.push(AgentMessage::tool(&call_id, output.to_string()));
                    emit_agent_event_for_turn_with_file_evidence_path_key(
                        &app,
                        &AgentEvent::ToolCallFinished {
                            session_id: session_id.clone(),
                            call_id,
                            tool_name,
                            input: input.clone(),
                            output,
                            is_error: false,
                        },
                        Some(&turn_id),
                        scope.paths,
                        scope.is_remote(),
                    );
                    final_response_reason = Some(reason);
                    continue;
                }
                let signature = tool_call_signature(&tool_name, &input);
                let read_only = agent_core::is_read_only_tool(&tool_name, &input);
                let duplicate_call = !tool_call_can_run(&tool_call_ledger, &signature, read_only);
                if duplicate_call {
                    let reason = tool_call_skip_reason(&tool_call_ledger, &signature);
                    let output = json!({ "skipped": true, "reason": reason.clone() });
                    messages.push(AgentMessage::tool(&call_id, output.to_string()));
                    emit_agent_event_for_turn_with_file_evidence_path_key(
                        &app,
                        &AgentEvent::ToolCallFinished {
                            session_id: session_id.clone(),
                            call_id,
                            tool_name,
                            input: input.clone(),
                            output,
                            is_error: false,
                        },
                        Some(&turn_id),
                        scope.paths,
                        scope.is_remote(),
                    );
                    skipped_duplicate = true;
                    continue;
                }
                if !read_only {
                    agent_core::tool_ledger::invalidate_successful_file_reads(
                        &mut tool_call_ledger,
                    );
                }
                executed_tool = true;
                begin_tool_call(&mut tool_call_ledger, signature.clone());
                let scope_namespace = scope
                    .permission_namespace(&ssh_permission_namespace, &wsl_permission_namespace);
                let permissions = PermissionMode::new(
                    &session_id,
                    permission_mode,
                    scope.paths,
                    scope.is_remote(),
                    scope_namespace,
                    &state.session_approvals,
                    &state.one_time_approvals,
                    if scope.environment == AgentToolEnvironment::Windows {
                        workspace_context
                            .as_ref()
                            .map(|context| &context.permissions)
                    } else {
                        None
                    },
                );
                if is_cancelled(&state, &session_id, &cancellation) {
                    return Err(cancellation_message(&cancellation_reason));
                }
                if !before_snapshot_attempted
                    && scope.environment == AgentToolEnvironment::Windows
                    && !agent_core::is_read_only_tool(&tool_name, &input)
                    && tool_name != ssh_connection::CONNECT_SSH_TOOL
                    && !snapshot_paths.is_empty()
                {
                    before_snapshot_attempted = true;
                    let paths = snapshot_paths.clone();
                    let capture_cancellation = cancellation.clone();
                    let baseline = tauri::async_runtime::spawn_blocking(move || {
                        capture_file_snapshot_with_cancellation(&paths, &capture_cancellation)
                    })
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result);
                    match baseline {
                        Ok(files) => {
                            file_backup_lease = Some(crate::file_changes::stage_turn_file_backups(
                                &session_id,
                                &turn_id,
                                &files,
                            ));
                            last_file_snapshot = files.clone();
                            before_files = Some(files);
                        }
                        Err(error) => emit_agent_payload_for_turn(
                            &app,
                            json!({
                                "type": "file_snapshot_error",
                                "session_id": session_id,
                                "phase": "before",
                                "code": "file_snapshot",
                                "retryable": false,
                                "message": error,
                            }),
                            &turn_id,
                        ),
                    }
                }
                emit_agent_status_for_turn(
                    &app,
                    &session_id,
                    "preparing_tool",
                    format!("正在准备 {tool_name}"),
                    Some(&turn_id),
                );
                let mut execution_input = input.clone();
                if let Value::Object(object) = &mut execution_input {
                    object.insert(
                        "__agent_call_id".to_string(),
                        Value::String(call_id.clone()),
                    );
                }
                let initial_registry =
                    if agent_core::path_access_requires_external_execution_for_target(
                        &tool_name,
                        &input,
                        scope.paths,
                        scope.is_remote(),
                    ) {
                        approved_path_registry.as_ref().unwrap_or(&registry)
                    } else {
                        &registry
                    };
                let execution_started = Arc::new(AtomicBool::new(false));
                let start_flag = execution_started.clone();
                let start_app = app.clone();
                let start_session_id = session_id.clone();
                let start_turn_id = turn_id.clone();
                let start_group_id = tool_group_id.clone();
                let start_call_id = call_id.clone();
                let start_tool_name = tool_name.clone();
                let start_input = input.clone();
                let execution = execute_tool_call_interruptible(
                    initial_registry,
                    &permissions,
                    &state,
                    &session_id,
                    &call_id,
                    &tool_name,
                    execution_input.clone(),
                    &turn_id,
                    &cancellation,
                    &tool_cancellation,
                    move || {
                        start_flag.store(true, Ordering::Relaxed);
                        emit_tool_execution_started(
                            &start_app,
                            &start_session_id,
                            &start_turn_id,
                            &start_group_id,
                            &start_call_id,
                            &start_tool_name,
                            &start_input,
                        );
                    },
                )
                .await?;
                let mut result = match execution {
                    InterruptibleToolExecution::Cancelled => {
                        let interrupted = execution_started.load(Ordering::Relaxed);
                        let reason = cancellation_message(&cancellation_reason);
                        let output = json!({
                            "error": { "code": "cancelled", "message": reason, "retryable": true },
                            "code": "cancelled",
                            "retryable": true,
                            "interrupted": interrupted,
                            "skipped": !interrupted,
                        });
                        let event = AgentEvent::ToolCallFinished {
                            session_id: session_id.clone(),
                            call_id: call_id.clone(),
                            tool_name: tool_name.clone(),
                            input: input.clone(),
                            output,
                            is_error: true,
                        };
                        complete_tool_call(
                            &mut tool_call_ledger,
                            &signature,
                            std::slice::from_ref(&event),
                        );
                        emit_agent_event_for_turn_with_file_evidence_path_key(
                            &app,
                            &event,
                            Some(&turn_id),
                            scope.paths,
                            scope.is_remote(),
                        );
                        let mut unfinished = serial_calls.collect::<Vec<_>>();
                        finish_pending_agent_tool_calls(
                            &app,
                            &session_id,
                            &turn_id,
                            default_environment,
                            &windows_paths,
                            &ssh_paths,
                            &wsl_paths,
                            &mut unfinished,
                            "cancelled",
                            &reason,
                            true,
                        );
                        return Err(reason);
                    }
                    InterruptibleToolExecution::Finished(result) => match result {
                        Ok(result) => result,
                        Err(error) => {
                            let error = error.to_string();
                            let retryable = tool_execution_failure_retryable(read_only, &error);
                            let output = tool_execution_failure_output(&error, retryable);
                            let event = AgentEvent::ToolCallFinished {
                                session_id: session_id.clone(),
                                call_id: call_id.clone(),
                                tool_name: tool_name.clone(),
                                input: input.clone(),
                                output: output.clone(),
                                is_error: true,
                            };
                            complete_tool_call(
                                &mut tool_call_ledger,
                                &signature,
                                std::slice::from_ref(&event),
                            );
                            messages.push(AgentMessage::tool(&call_id, output.to_string()));
                            emit_agent_event_for_turn_with_file_evidence_path_key(
                                &app,
                                &event,
                                Some(&turn_id),
                                scope.paths,
                                scope.is_remote(),
                            );
                            emit_agent_status_for_turn(
                                &app,
                                &session_id,
                                "requesting_model",
                                format!("{tool_name} 执行失败，正在让模型根据错误继续处理"),
                                Some(&turn_id),
                            );
                            continue;
                        }
                    },
                };
                result.attach_images(selected_model.supports_vision, &mut image_attachments);
                update_image_token_reserve(&image_attachments, &image_token_reserve);
                // Register the waiter before publishing PermissionRequested. The
                // user can answer immediately after the event reaches the UI.
                let permission_wait = result
                    .permission_request
                    .as_ref()
                    .map(|request_id| {
                        register_pending_permission(
                            &state,
                            &session_id,
                            request_id.clone(),
                            agent_core::permission_scope_key_for_environment(
                                &tool_name,
                                &input,
                                scope.paths,
                                scope.is_remote(),
                                scope_namespace,
                            ),
                            call_id.clone(),
                            cancellation.clone(),
                            turn_id.clone(),
                            workspace_context
                                .as_ref()
                                .map(|context| context.root.to_string_lossy().to_string()),
                        )
                    })
                    .transpose()?;
                for event in &mut result.events {
                    if let AgentEvent::PermissionRequested {
                        tool_name,
                        input,
                        expires_at_ms,
                        ..
                    } = event
                    {
                        // N2：发出前盖截止章（等待超时常量在本文件），前端权限条
                        // 据此显示倒计时；超时后走 N1 的合成 Deny + resolved 事件。
                        // P6-C：配置为 0（不超时）时不盖章——权限条无倒计时，
                        // 只能由用户决策或停止回合收尾。
                        if expires_at_ms.is_none() {
                            if let Some(timeout) = tool_permission_timeout() {
                                *expires_at_ms = Some(
                                    std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|elapsed| {
                                            elapsed.as_millis() as i64 + timeout.as_millis() as i64
                                        })
                                        .unwrap_or_default(),
                                );
                            }
                        }
                        *input = agent_core::enrich_permission_input_for_target(
                            tool_name,
                            input,
                            permission_mode,
                            scope.paths,
                            scope.is_remote(),
                            scope_namespace,
                        );
                        if let Value::Object(object) = input {
                            object.remove("__agent_call_id");
                        }
                        emit_agent_event_for_turn_with_call_id(
                            &app,
                            event,
                            Some(&turn_id),
                            &call_id,
                        );
                    } else {
                        if let AgentEvent::ToolCallFinished {
                            input: event_input, ..
                        } = event
                        {
                            *event_input = input.clone();
                        }
                        emit_agent_event_for_turn_with_file_evidence_path_key(
                            &app,
                            event,
                            Some(&turn_id),
                            scope.paths,
                            scope.is_remote(),
                        );
                    }
                }
                if result.permission_request.is_none() && tool_name != PATH_ACCESS_PROBE_TOOL {
                    consume_one_time_path_approval(
                        &state,
                        &session_id,
                        &tool_name,
                        &input,
                        scope.paths,
                        scope.is_remote(),
                        scope_namespace,
                    );
                }
                if let Some(request_id) = result.permission_request.take() {
                    let receiver = permission_wait.ok_or_else(|| "权限等待器未创建".to_string())?;
                    // N1-A：request_id 会随等待移动，先留一份给超时补发事件用。
                    let permission_request_id = request_id.clone();
                    let permission_response = match wait_for_permission(
                        &state,
                        &session_id,
                        request_id,
                        cancellation.clone(),
                        receiver,
                        tool_permission_timeout(),
                    )
                    .await
                    {
                        Ok(response) => response,
                        Err(error) => {
                            let cancelled = error == "请求已取消";
                            emit_agent_event_for_turn_with_file_evidence_path_key(
                                &app,
                                &AgentEvent::ToolCallFinished {
                                    session_id: session_id.clone(),
                                    call_id: call_id.clone(),
                                    tool_name: tool_name.clone(),
                                    input: input.clone(),
                                    output: json!({
                                        "error": {
                                            "code": if cancelled { "cancelled" } else { "permission" },
                                            "message": error,
                                            "retryable": cancelled,
                                        },
                                        "code": if cancelled { "cancelled" } else { "permission" },
                                        "retryable": cancelled,
                                        "skipped": true,
                                        "interrupted": false,
                                    }),
                                    is_error: true,
                                },
                                Some(&turn_id),
                                scope.paths,
                                scope.is_remote(),
                            );
                            let mut unfinished = serial_calls.collect::<Vec<_>>();
                            finish_pending_agent_tool_calls(
                                &app,
                                &session_id,
                                &turn_id,
                                default_environment,
                                &windows_paths,
                                &ssh_paths,
                                &wsl_paths,
                                &mut unfinished,
                                if cancelled { "cancelled" } else { "permission" },
                                &error,
                                cancelled,
                            );
                            return Err(if cancelled {
                                "请求已取消".to_string()
                            } else {
                                error
                            });
                        }
                    };
                    let PermissionWaitResult::Decision(permission_response) = permission_response;
                    // N1-A：超时合成 Deny 时补发 permission_resolved——后端已移除
                    // pending 条目，不发事件前端权限条就会永久卡死（N1）。
                    if permission_response.timed_out {
                        emit_thread_snapshot_event(
                            &app,
                            &state,
                            ThreadEvent::PermissionResolved {
                                thread_id: session_id.clone(),
                                turn_id: turn_id.clone(),
                                request_id: permission_request_id,
                            },
                            None,
                        );
                    }
                    let mut permission_result = match permission_response.action {
                        PermissionAction::Once | PermissionAction::Session => {
                            let approved_registry =
                                if agent_core::path_access_requires_external_execution_for_target(
                                    &tool_name,
                                    &input,
                                    scope.paths,
                                    scope.is_remote(),
                                ) {
                                    approved_path_registry.as_ref().unwrap_or(&registry)
                                } else {
                                    &registry
                                };
                            let start_flag = execution_started.clone();
                            let start_app = app.clone();
                            let start_session_id = session_id.clone();
                            let start_turn_id = turn_id.clone();
                            let start_group_id = tool_group_id.clone();
                            let start_call_id = call_id.clone();
                            let start_tool_name = tool_name.clone();
                            let start_input = input.clone();
                            match execute_tool_call_interruptible(
                                approved_registry,
                                &AllowPermission,
                                &state,
                                &session_id,
                                &call_id,
                                &tool_name,
                                execution_input.clone(),
                                &turn_id,
                                &cancellation,
                                &tool_cancellation,
                                move || {
                                    start_flag.store(true, Ordering::Relaxed);
                                    emit_tool_execution_started(
                                        &start_app,
                                        &start_session_id,
                                        &start_turn_id,
                                        &start_group_id,
                                        &start_call_id,
                                        &start_tool_name,
                                        &start_input,
                                    );
                                },
                            )
                            .await?
                            {
                                InterruptibleToolExecution::Finished(result) => result?,
                                InterruptibleToolExecution::Cancelled => {
                                    let reason = cancellation_message(&cancellation_reason);
                                    let output = json!({
                                        "error": { "code": "cancelled", "message": reason, "retryable": true },
                                        "code": "cancelled",
                                        "retryable": true,
                                        "interrupted": true,
                                        "skipped": false,
                                    });
                                    let event = AgentEvent::ToolCallFinished {
                                        session_id: session_id.clone(),
                                        call_id: call_id.clone(),
                                        tool_name: tool_name.clone(),
                                        input: input.clone(),
                                        output,
                                        is_error: true,
                                    };
                                    complete_tool_call(
                                        &mut tool_call_ledger,
                                        &signature,
                                        std::slice::from_ref(&event),
                                    );
                                    emit_agent_event_for_turn_with_file_evidence_path_key(
                                        &app,
                                        &event,
                                        Some(&turn_id),
                                        scope.paths,
                                        scope.is_remote(),
                                    );
                                    let mut unfinished = serial_calls.collect::<Vec<_>>();
                                    finish_pending_agent_tool_calls(
                                        &app,
                                        &session_id,
                                        &turn_id,
                                        default_environment,
                                        &windows_paths,
                                        &ssh_paths,
                                        &wsl_paths,
                                        &mut unfinished,
                                        "cancelled",
                                        &reason,
                                        true,
                                    );
                                    return Err(reason);
                                }
                            }
                        }
                        PermissionAction::Deny => {
                            execute_tool_call(
                                &registry,
                                &DenyPermission {
                                    reason: "用户拒绝了 Agent 操作".to_string(),
                                },
                                &session_id,
                                &call_id,
                                &tool_name,
                                execution_input.clone(),
                            )
                            .await?
                        }
                        PermissionAction::Other => {
                            execute_tool_call(
                                &registry,
                                &DenyPermission {
                                    reason: permission_response
                                        .instruction
                                        .filter(|value| !value.trim().is_empty())
                                        .map(|value| {
                                            format!("用户未批准当前操作，并补充说明：{value}")
                                        })
                                        .unwrap_or_else(|| "用户未批准当前 Agent 操作".to_string()),
                                },
                                &session_id,
                                &call_id,
                                &tool_name,
                                execution_input,
                            )
                            .await?
                        }
                    };
                    if tool_name != PATH_ACCESS_PROBE_TOOL {
                        consume_one_time_path_approval(
                            &state,
                            &session_id,
                            &tool_name,
                            &input,
                            scope.paths,
                            scope.is_remote(),
                            scope_namespace,
                        );
                    }
                    permission_result
                        .attach_images(selected_model.supports_vision, &mut image_attachments);
                    update_image_token_reserve(&image_attachments, &image_token_reserve);
                    for event in &permission_result.events {
                        let mut event = event.clone();
                        if let AgentEvent::ToolCallFinished {
                            input: event_input, ..
                        } = &mut event
                        {
                            *event_input = input.clone();
                        }
                        emit_agent_event_for_turn_with_file_evidence_path_key(
                            &app,
                            &event,
                            Some(&turn_id),
                            scope.paths,
                            scope.is_remote(),
                        );
                    }
                    result = permission_result;
                }
                complete_tool_call(&mut tool_call_ledger, &signature, &result.events);
                if let Some(id) = ssh_connection::connected_result(&result.events) {
                    pending_ssh_connection_id = Some(id);
                }
                let read_failure_notice =
                    failed_file_read_instruction(&tool_name, &input, &result.events);
                messages.push(result.message);
                if !read_only && scope.environment == AgentToolEnvironment::Windows {
                    if let Some(before) = before_files.as_ref() {
                        if let Some(after) = refresh_turn_file_changes(
                            &app,
                            &session_id,
                            &turn_id,
                            &snapshot_paths,
                            before,
                            &last_file_snapshot,
                            false,
                        )
                        .await
                        {
                            last_file_snapshot = after;
                        }
                    }
                }
                if let Some(notice) = read_failure_notice {
                    deferred_file_read_notices.push(notice);
                }
                emit_agent_status_for_turn(
                    &app,
                    &session_id,
                    "requesting_model",
                    "工具已完成，正在整理结果",
                    Some(&turn_id),
                );
            }
        }
        messages.extend(deferred_file_read_notices);
        if steer_message.is_none() {
            if let Ok(message) = steer_receiver.try_recv() {
                steer_message = Some(message);
            }
        }
        if let Some(steer) = steer_message.take() {
            if is_cancelled(&state, &session_id, &cancellation) {
                return Err(cancellation_message(&cancellation_reason));
            }
            apply_steer_runtime_inputs(
                &attachment_root,
                &steer,
                &mut messages,
                &mut windows_paths,
                &mut tool_policy,
                model_supports_tools,
                explicitly_disabled,
                selected_model.supports_vision,
                &mut image_attachments,
                &image_token_reserve,
            )?;
            if mark_agent_steer_applied(&app, &session_id, &steer) {
                if let Err(error) = replay_pending_agent_steers(&app, &state, &session_id) {
                    eprintln!("无法继续投递持久化追加要求: {error}");
                }
            }
            tool_cancellation = Arc::new(AtomicBool::new(false));
            drop(plugin_turn);
            plugin_turn = crate::plugin_host::prepare_turn(
                &session_id,
                &turn_id,
                if tool_policy == AgentToolPolicy::Disabled {
                    &[]
                } else {
                    &windows_paths
                },
                permission_mode,
            )
            .await;
            (registry, approved_path_registry) = build_environment_tool_registries(
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
                &bound_ssh_hosts,
                &app,
                &session_id,
                &turn_id,
                (tool_policy == AgentToolPolicy::Auto)
                    .then_some(collaboration_tool.as_deref())
                    .flatten(),
            )?;
            request_tools = registry.list_public().into_iter().cloned().collect();
            let plugin_instruction = if request_tools.is_empty() {
                String::new()
            } else {
                plugin_turn.prompt(default_environment == AgentToolEnvironment::Windows)
            };
            runtime_context_tokens = cache_context::update_runtime(
                &mut messages,
                &mut runtime_context,
                compose_turn_prompt(
                    tool_policy,
                    &request_tools,
                    &plugin_instruction,
                    remote_context.as_ref().map(|context| context.root()),
                )
                .runtime,
            );
            if is_cancelled(&state, &session_id, &cancellation) {
                return Err(cancellation_message(&cancellation_reason));
            }
            text.clear();
            final_response_requested = false;
            final_phase_notice_pushed = false;
            messages.retain(|message| {
                !(message.role == MessageRole::System && message.content == FINAL_PHASE_NOTICE)
            });
            termination_reason = "model_finished";
            tool_rounds = 0;
            output_continuations = 0;
            response_recovery = Default::default();
            tool_loop_progress = Default::default();
            provider_context_retry_count = 0;
            malformed_tool_signatures.clear();
            tool_call_ledger.clear();
            dsml_stream.buffer.clear();
            let mut steer_payload = json!({
                "type": "steered",
                "session_id": session_id,
                "message": steer.message,
                "attachments": steer.attachments,
                "artifacts": steer.artifacts,
            });
            if let Value::Object(object) = &mut steer_payload {
                object.insert("turn_id".to_string(), Value::String(turn_id.clone()));
            }
            emit_agent_payload_for_turn(&app, steer_payload, &turn_id);
            end_tool_group(&session_id, &turn_id);
            continue;
        }
        if executed_tool {
            response_recovery = Default::default();
        }
        if tool_loop_progress.observe_round(executed_tool, skipped_duplicate) {
            final_response_reason = Some("连续三轮仅有重复工具调用，没有新的执行进展".to_string());
        }
        if retry_truncated_tool_call && output_continuations < AGENT_OUTPUT_CONTINUATION_LIMIT {
            messages.push(AgentMessage::system(
                "上一轮工具调用参数因达到单轮输出上限而不完整。请重新生成完整的工具调用参数，不要重复已经成功执行的工具。",
            ));
            output_continuations += 1;
            final_response_requested = false;
            emit_agent_status_for_turn(
                &app,
                &session_id,
                "retrying_tool_call",
                format!(
                    "工具参数输出不完整，正在重新生成（{}/{}）",
                    output_continuations, AGENT_OUTPUT_CONTINUATION_LIMIT
                ),
                Some(&turn_id),
            );
            end_tool_group(&session_id, &turn_id);
            continue;
        }
        tool_rounds = tool_rounds.saturating_add(1);
        if let Some(reason) = final_response_reason {
            termination_reason = if reason.contains("重复调用") {
                "duplicate_tool_call"
            } else if reason.contains("参数不完整") {
                "malformed_tool_call"
            } else {
                "tool_execution_boundary"
            };
            final_response_requested = true;
            emit_agent_status_for_turn(
                &app,
                &session_id,
                "finishing",
                format!("{reason}，正在根据已有结果生成回答"),
                Some(&turn_id),
            );
        }
        // A new model round starts a new contiguous tool segment. If the
        // model immediately emits another tool call without commentary, the
        // runtime still needs one fallback action note for that new group.
        end_tool_group(&session_id, &turn_id);
    }
    emit_agent_status_for_turn(
        &app,
        &session_id,
        "finishing",
        "正在整理最终结果",
        Some(&turn_id),
    );
    if is_cancelled(&state, &session_id, &cancellation) {
        return Err(cancellation_message(&cancellation_reason));
    }
    if termination_reason == "model_response_missing" {
        return Err("[model_response_missing] 模型连续未给出有效回答，自动恢复两次后仍未完成。已保留此前输出和工具执行记录；这不代表此前工具执行失败，可以发送消息继续。".to_string());
    }
    if termination_reason == "model_action_missing" {
        return Err("[model_action_missing] 模型仅返回了准备执行的说明，却没有请求工具；自动恢复两次后仍未执行。已保留此前输出和工具记录，本轮尚未完成。".to_string());
    }
    usage = finalize_usage(
        usage,
        UsageFinalization {
            protocol,
            provider: &provider,
            selected_model: &selected_model,
            model_supports_usage,
            provider_capabilities,
            context_window,
            context_window_estimated,
            final_response_requested,
            generation_metrics: &generation_metrics,
            last_context_tokens: &last_context_tokens,
            model_request_count: &model_request_count,
            cache_baseline_reset,
        },
    );
    let (report, report_error) = if request.analysis_mode {
        match parse_analysis_report(&text) {
            Ok(report) => (Some(report), None),
            Err(error) => (None, Some(error)),
        }
    } else {
        (None, None)
    };
    if request.analysis_mode {
        let mut report_payload = json!({
            "type": "final_report",
            "session_id": session_id,
            "report_error": report_error.clone(),
        });
        if let Value::Object(object) = &mut report_payload {
            if let Some(report) = report.as_ref() {
                object.insert(
                    "report".to_string(),
                    serde_json::to_value(report).unwrap_or(Value::Null),
                );
            }
        }
        emit_agent_payload_for_turn(&app, report_payload, &turn_id);
    }
    if let Some(payload) = checkpoint.payload.as_mut() {
        payload.producer_model_id = Some(selected_model.id.clone());
    }
    save_transcript(
        &app,
        &session_id,
        &turn_id,
        termination_reason,
        &messages,
        transcript_scope,
        runtime_context,
        &image_attachments,
        cache_fingerprint,
    )
    .await;
    if let Some(before_files) = before_files {
        let _ = refresh_turn_file_changes(
            &app,
            &session_id,
            &turn_id,
            &snapshot_paths,
            &before_files,
            &last_file_snapshot,
            true,
        )
        .await;
    }
    drop(file_backup_lease);
    Ok(AgentChatResponse {
        session_id,
        text,
        usage,
        report,
        report_error,
        evidence: Some(evidence),
        context_checkpoint: checkpoint.summary.map(|summary| AgentContextCheckpoint {
            id: format!("context-{}", uuid_suffix()),
            summary,
            evidence_policy_version: CONTEXT_CHECKPOINT_EVIDENCE_POLICY_VERSION,
            covered_through_message_id: checkpoint.covered_through_message_id.take(),
            payload: checkpoint.payload,
        }),
        turn_id,
        termination_reason: termination_reason.to_string(),
    })
}
