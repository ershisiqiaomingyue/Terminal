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
                            provider, &selected_model.id, protocol, &request,
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
