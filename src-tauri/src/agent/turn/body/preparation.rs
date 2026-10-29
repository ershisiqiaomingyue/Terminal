        let control = AgentMessage::system(runtime_context.clone());
        initial_context_messages.push(control.clone());
        initial_compaction.messages.push(control);
    }
    let initial_context_tokens = estimate_context_tokens(
        &initial_context_messages,
        &base_system_prompt,
        &request_tools,
    );
    let initial_compaction_id = format!("compaction-{}", uuid_suffix());
    let mut model_summary_attempted = false;
    // P9-G：上一轮压缩时预置的确定性检查点摘要，下一轮压缩直接复用同一份，
    // 避免「每轮现生成一份新的」把前缀打散。
    let mut planned_compaction_summary: Option<(
        String,
        Option<agent_core::AgentContextCheckpointPayload>,
    )> = None;
    if initial_compaction.compressed {
        emit_agent_context_for_turn(
            &app,
            json!({
                "session_id": session_id, "compaction_id": initial_compaction_id,
                "compaction_status": "running", "compaction_reason": "history_budget",
                "before_context_tokens": initial_context_tokens,
                "reserved_output_tokens": telemetry_output_reserve, "context_compressed": true,
            }),
            Some(&turn_id),
        );
        emit_agent_status_for_turn(
            &app,
            &session_id,
            "compacting_context",
            "正在整理较早上下文，模型摘要不可用时将保留确定性检查点",
            Some(&turn_id),
        );
        let processed_messages = initial_compaction.stats.covered_message_count;
        // Character budget for the model summary ≈ the token budget available
        // for it (enrich_context_checkpoint_with_model caps `available` at
        // 1024 tokens); used only to estimate the running percentage.
        let initial_summary_budget_chars = initial_compaction
            .stats
            .budget_tokens
            .saturating_sub(initial_compaction.stats.compacted_estimated_tokens)
            .saturating_sub(96)
            .min(1_024)
            .saturating_mul(3);
        super::events::emit_compaction_progress(
            &app,
            &session_id,
            &turn_id,
            &initial_compaction_id,
            0,
            processed_messages,
            initial_context_tokens,
            initial_summary_budget_chars,
        );
        let (requested, summary_usage) = enrich_context_checkpoint_with_model(
            &client,
            ProviderRequest::new(
                &session_id,
                &selected_model.id,
                Vec::new(),
                request_tools.clone(),
            )
            .with_system_prompt(base_system_prompt.clone())
            .with_reasoning_effort(reasoning_effort.clone())
            .with_native_reasoning_effort(native_reasoning_effort)
            .with_reasoning_encrypted_content(!responses_reasoning_include_fallback)
            .with_usage_reporting(model_supports_usage && !compatible_usage_fallback),
            &initial_context_messages,
            &mut initial_compaction,
            &mut model_summary_attempted,
            &state,
            &cancellation,
            |characters| {
                super::events::emit_compaction_progress(
                    &app,
                    &session_id,
                    &turn_id,
                    &initial_compaction_id,
                    characters,
                    processed_messages,
                    initial_context_tokens,
                    initial_summary_budget_chars,
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
    let initial_context_compressed = initial_compaction.compressed;
    let initial_context_summary = initial_compaction.summary;
    let initial_context_checkpoint_payload = initial_compaction.checkpoint_payload;
    let initial_covered_message_count = initial_compaction.covered_message_count;
    let initial_covered_message_id = initial_compaction.covered_message_id;
    let initial_compaction_status = initial_compaction.status;
    let initial_compaction_stats = initial_compaction.stats;
    let mut messages = initial_compaction.messages;
    cache_context::retain_runtime(&mut messages, &runtime_context);
    let active_image_attachments = image_attachments_for_messages(&image_attachments, &messages);
    update_image_token_reserve(&active_image_attachments, &image_token_reserve);
    let mut checkpoint = CheckpointState {
        summary: initial_context_summary,
        payload: initial_context_checkpoint_payload,
        covered_through_message_id: initial_covered_message_id.or_else(|| {
            (!restored_checkpoint_context)
                .then(|| {
                    context_message_id_at_boundary(&request.context, initial_covered_message_count)
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
