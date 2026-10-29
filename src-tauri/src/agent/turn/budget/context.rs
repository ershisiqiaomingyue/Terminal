use super::super::*;

pub(super) use super::RoundBudget;

/// 本轮压缩状态；触发线、缓存预热和历史改写分开推进，保持原有事件顺序。
pub(super) struct RoundContextInput<'a, 'b> {
    pub app: &'a AppHandle,
    pub state: &'a AgentRuntimeState,
    pub client: &'a HttpProviderClient,
    pub cancellation: &'a AtomicBool,
    pub cancellation_reason: &'a Arc<Mutex<Option<String>>>,
    pub usage: &'b mut Option<AgentUsage>,
    pub messages: &'b mut Vec<AgentMessage>,
    pub checkpoint: &'b mut CheckpointState,
    pub image_attachments: &'a [AgentImageAttachment],
    pub image_token_reserve: &'a AtomicUsize,
    pub planned_compaction_summary: &'b mut Option<(
        String,
        Option<agent_core::AgentContextCheckpointPayload>,
    )>,
    pub session_id: &'a str,
    pub turn_id: &'a str,
    pub system_prompt: &'a str,
    pub selected_model: &'a agent_core::ModelProfile,
    pub request_tools: &'a [ToolDefinition],
    pub budget: &'a RoundBudget,
    pub runtime_context: &'a str,
    pub context_window: Option<u32>,
    pub model_supports_usage: bool,
    pub compatible_usage_fallback: bool,
    pub responses_reasoning_include_fallback: bool,
    pub native_reasoning_effort: Option<String>,
    pub reasoning_effort: &'a Option<String>,
    pub model_summary_attempted: &'b mut bool,
    pub model_request_count: &'a std::cell::Cell<u64>,
    pub cache_baseline_reset: bool,
    pub estimate_context_tokens: &'a dyn Fn(&[AgentMessage], &str, &[ToolDefinition]) -> usize,
}

pub(super) struct RoundContextResult {
    pub context_tokens: usize,
    pub compaction_trigger: usize,
    pub cache_baseline_reset: bool,
}

pub(in crate::agent::turn) async fn prepare_round_context(
    input: RoundContextInput<'_, '_>,
) -> Result<RoundContextResult, String> {
    let RoundContextInput {
        app,
        state,
        client,
        cancellation,
        cancellation_reason,
        usage,
        messages,
        checkpoint,
        image_attachments,
        image_token_reserve,
        planned_compaction_summary,
        session_id,
        turn_id,
        system_prompt,
        selected_model,
        request_tools,
        budget,
        runtime_context,
        context_window,
        model_supports_usage,
        compatible_usage_fallback,
        responses_reasoning_include_fallback,
        native_reasoning_effort,
        reasoning_effort,
        model_summary_attempted,
        model_request_count,
        mut cache_baseline_reset,
        estimate_context_tokens,
    } = input;
    let before_context_tokens = budget.before_tokens;
    let round_output_reserve = budget.output_reserve;
    let round_request_overhead = budget.request_overhead;
    let round_compaction_trigger = budget.compaction_trigger;
    let mut compaction = agent_core::compact_messages_with_trigger(
        messages,
        budget.context_budget,
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
        cache_baseline_reset = true;
    }
    let round_compaction_id = format!("compaction-{}", uuid_suffix());
    if cache_context::should_prewarm_cache(
        before_context_tokens,
        round_compaction_trigger,
        messages.len(),
    ) {
        prewarm::prewarm_before_compaction(prewarm::PrewarmInput {
            app,
            client,
            session_id,
            turn_id,
            selected_model_id: &selected_model.id,
            messages: messages.clone(),
            request_tools,
            system_prompt,
            before_context_tokens,
            round_compaction_trigger,
            model_supports_usage,
            compatible_usage_fallback,
            model_request_count,
        })
        .await;
    }
    if compaction.compressed {
        emit_agent_context_for_turn(
            app,
            json!({
                "session_id": session_id, "compaction_id": round_compaction_id,
                "compaction_status": "running", "compaction_reason": "context_budget",
                "before_context_tokens": before_context_tokens,
                "reserved_output_tokens": round_output_reserve, "context_compressed": true,
            }),
            Some(turn_id),
        );
    }
    if compaction.compressed && !*model_summary_attempted {
        if planned_compaction_summary.is_some() {
            *model_summary_attempted = true;
        } else {
            emit_agent_status_for_turn(
                app,
                session_id,
                "compacting_context",
                "正在整理上下文，保留约束与工具证据",
                Some(turn_id),
            );
            let processed_messages = compaction.stats.covered_message_count;
            let summary_budget_chars = compaction
                .stats
                .budget_tokens
                .saturating_sub(compaction.stats.compacted_estimated_tokens)
                .saturating_sub(96)
                .min(1_024)
                .saturating_mul(3);
            emit_turn_compaction_progress(
                app,
                session_id,
                turn_id,
                &round_compaction_id,
                0,
                processed_messages,
                before_context_tokens,
                summary_budget_chars,
            );
            let (requested, summary_usage) = enrich_context_checkpoint_with_model(
                client,
                ProviderRequest::new(
                    session_id,
                    &selected_model.id,
                    Vec::new(),
                    request_tools.to_vec(),
                )
                .with_system_prompt(system_prompt.to_string())
                .with_reasoning_effort(reasoning_effort.clone())
                .with_native_reasoning_effort(native_reasoning_effort)
                .with_reasoning_encrypted_content(!responses_reasoning_include_fallback)
                .with_usage_reporting(model_supports_usage && !compatible_usage_fallback),
                messages,
                &mut compaction,
                model_summary_attempted,
                state,
                cancellation,
                |characters| {
                    emit_turn_compaction_progress(
                        app,
                        session_id,
                        turn_id,
                        &round_compaction_id,
                        characters,
                        processed_messages,
                        before_context_tokens,
                        summary_budget_chars,
                    )
                },
            )
            .await;
            if requested {
                model_request_count.set(model_request_count.get().saturating_add(1));
            }
            if let Some(summary_usage) = summary_usage {
                merge_agent_usage(usage, summary_usage);
            }
            if is_cancelled(state, session_id, cancellation) {
                return Err(cancellation_message(cancellation_reason));
            }
        }
    }
    if compaction.compressed && compaction.summary.is_none() {
        if let Some((summary, _)) = planned_compaction_summary.as_ref() {
            compaction.summary = Some(summary.clone());
        }
    }
    if compaction.compressed {
        *planned_compaction_summary = agent_core::plan_compaction_checkpoint(
            &compaction.messages,
            budget.context_budget,
            round_compaction_trigger,
            round_request_overhead,
        );
        *messages = compaction.messages;
        cache_context::retain_runtime(messages, runtime_context);
        let active_image_attachments = image_attachments_for_messages(image_attachments, messages);
        update_image_token_reserve(&active_image_attachments, image_token_reserve);
        if compaction.summary.is_some() {
            checkpoint.summary = compaction.summary;
        }
        if compaction.checkpoint_payload.is_some() {
            checkpoint.payload = compaction.checkpoint_payload;
        }
        if compaction.covered_message_id.is_some() {
            checkpoint.covered_through_message_id = compaction.covered_message_id;
        }
        let after_context_tokens = estimate_context_tokens(messages, system_prompt, request_tools);
        emit_agent_context_for_turn(
            app,
            json!({
                "session_id": session_id,
                "compaction_id": round_compaction_id,
                "compaction_status": "completed",
                "compaction_reason": "context_budget",
                "before_context_tokens": before_context_tokens,
                "after_context_tokens": after_context_tokens,
                "reserved_output_tokens": round_output_reserve,
                "compaction_result": compaction.status,
                "compaction_stats": compaction.stats,
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
            Some(turn_id),
        );
        emit_agent_status_for_turn(
            app,
            session_id,
            "context_compacted",
            "工具结果与较早上下文压缩完成，准备继续",
            Some(turn_id),
        );
    }
    let context_tokens = estimate_context_tokens(messages, system_prompt, request_tools);
    last_context_tokens.store(context_tokens as u64, Ordering::Relaxed);
    Ok(RoundContextResult {
        context_tokens,
        compaction_trigger: round_compaction_trigger,
        cache_baseline_reset,
    })
}
