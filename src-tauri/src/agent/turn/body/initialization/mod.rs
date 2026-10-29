use super::*;
use crate::agent::prompt_composer::ComposedAgentPrompt;

pub(in crate::agent::turn::body) struct ContextInput<'a> {
    pub app: &'a AppHandle,
    pub state: &'a AgentRuntimeState,
    pub client: &'a HttpProviderClient,
    pub request: &'a AgentChatRequest,
    pub provider: &'a agent_core::AgentProviderSettings,
    pub selected_model: &'a agent_core::AgentModelSettings,
    pub protocol: ProviderKind,
    pub session_id: &'a str,
    pub turn_id: &'a str,
    pub current_user_message_id: &'a str,
    pub prompt: &'a str,
    pub composed_prompt: ComposedAgentPrompt,
    pub context_window: Option<u32>,
    pub context_budget: usize,
    pub model_supports_usage: bool,
    pub native_reasoning_effort: bool,
    pub reasoning_effort: Option<&'a str>,
    pub request_tools: &'a [ToolDefinition],
    pub image_token_reserve: &'a AtomicUsize,
    pub image_attachments: &'a mut Vec<ProviderImageAttachment>,
    pub ssh_context: Option<&'a agent_core::SshToolContext>,
    pub wsl_root: Option<&'a str>,
    pub ssh_connection_id: Option<&'a str>,
    pub cancellation: &'a Arc<AtomicBool>,
    pub cancellation_reason: &'a Arc<Mutex<Option<String>>>,
    pub compatible_usage_fallback: bool,
    pub responses_reasoning_include_fallback: bool,
    pub usage: &'a mut Option<AgentUsage>,
}

pub(in crate::agent::turn::body) struct ContextInitialization {
    pub base_system_prompt: String,
    pub runtime_context: String,
    pub transcript_scope: String,
    pub prompt_cache_key: String,
    pub cache_fingerprint: Option<agent_core::PromptCacheFingerprint>,
    pub runtime_context_tokens: usize,
    pub telemetry_output_reserve: usize,
    pub context_budget: usize,
    pub initial_context_tokens: usize,
    pub initial_compaction_id: String,
    pub model_summary_attempted: bool,
    pub model_summary_requested: bool,
    pub planned_compaction_summary: Option<(String, Option<agent_core::AgentContextCheckpointPayload>)>,
    pub initial_context_compressed: bool,
    pub initial_context_summary: Option<String>,
    pub initial_context_checkpoint_payload: Option<agent_core::AgentContextCheckpointPayload>,
    pub initial_covered_message_count: Option<usize>,
    pub initial_covered_message_id: Option<String>,
    pub restored_checkpoint_context: bool,
    pub messages: Vec<AgentMessage>,
    pub initial_context_status: agent_core::CompactionStatus,
    pub initial_context_stats: agent_core::CompactionStats,
}

pub(in crate::agent::turn::body) async fn initialize_context(
    input: ContextInput<'_>,
) -> Result<ContextInitialization, String> {
    let ContextInput {
        app,
        state,
        client,
        request,
        provider,
        selected_model,
        protocol,
        session_id,
        turn_id,
        current_user_message_id,
        prompt,
        composed_prompt,
        context_window,
        context_budget,
        model_supports_usage,
        native_reasoning_effort,
        reasoning_effort,
        request_tools,
        image_token_reserve,
        image_attachments,
        ssh_context,
        wsl_root,
        ssh_connection_id,
        cancellation,
        cancellation_reason,
        compatible_usage_fallback,
        responses_reasoning_include_fallback,
        usage,
    } = input;
    let base_system_prompt = composed_prompt.system;
    let runtime_context = composed_prompt.runtime;
    let transcript_scope = cache_context::scope(
        provider,
        &selected_model.id,
        protocol,
        request,
        ssh_context
            .map(|context| context.root())
            .or(wsl_root)
            .or(request.workspace_path.as_deref())
            .unwrap_or(""),
        ssh_connection_id,
    );
    let prompt_cache_key =
        crate::history::get_conversation_thread(app.clone(), session_id.to_string())?
            .map(|thread| thread.root_thread_id)
            .unwrap_or_else(|| session_id.to_string());
    let native_transcript =
        crate::history::restore_model_transcript(app, session_id, &transcript_scope, turn_id)?;
    let runtime_changed = native_transcript.as_ref().is_none_or(|transcript| {
        transcript.runtime_context != runtime_context
            || !transcript.messages.iter().any(|message| {
                message.role == MessageRole::System && message.content == runtime_context
            })
    });
    let cache_fingerprint = native_transcript
        .as_ref()
        .and_then(|transcript| transcript.last_request.clone());
    if selected_model.supports_vision != Some(false) {
        cache_context::restore_images(image_attachments, native_transcript.as_ref());
    }
    update_image_token_reserve(image_attachments, image_token_reserve);
    let runtime_context_tokens =
        agent_core::estimate_messages_tokens(&[AgentMessage::system(runtime_context.clone())]);
    let telemetry_output_reserve = agent_output_reserve(context_window, false);
    let context_is_history_hydrated = request.context.iter().all(|turn| turn.role == "system")
        || request
            .context
            .iter()
            .any(|turn| turn.source.as_deref() == Some("history"));
    let native_history_restored = native_transcript.is_some();
    let restored_checkpoint_messages = if let Some(transcript) = native_transcript {
        Some(transcript.messages)
    } else if context_is_history_hydrated {
        crate::history::restored_agent_messages_for_session(
            app,
            session_id,
            &selected_model.id,
            CONTEXT_CHECKPOINT_EVIDENCE_POLICY_VERSION,
            Some(turn_id),
        )?
    } else {
        None
    };
    let restored_checkpoint_context = restored_checkpoint_messages.is_some();
    let mut current_user_message = AgentMessage::user(prompt.to_string());
    current_user_message.source_message_id = Some(current_user_message_id.to_string());
    // Token cost of everything in the request that is not a message: it rides
    // along on every call and never shrinks when messages are summarized, so the
    // compaction ceiling has to be reduced by it to bound the billed input.
    let request_overhead_tokens =
        estimate_agent_request_tokens(&[], &base_system_prompt, request_tools)
            .saturating_add(runtime_context_tokens)
            .saturating_add(image_token_reserve.load(Ordering::Relaxed));
    let (mut initial_context_messages, mut initial_compaction) = cache_context::initial_messages(
        restored_checkpoint_messages,
        request,
        prompt,
        turn_id,
        current_user_message,
        context_budget,
        request_overhead_tokens,
    );
    crate::diagnostics::record(
        "info",
        "agent.history_restored",
        "native.agent",
        json!({
            "session_id": session_id, "turn_id": turn_id,
            "source": if native_history_restored { "native_transcript" }
                else if restored_checkpoint_context { "durable_checkpoint_or_tools" }
                else { "conversation_text" },
            "history_context_turns": request.context.iter().filter(|turn| turn.source.as_deref() == Some("history")).count(),
            "messages_before_compaction": initial_context_messages.len(),
            "tool_results_before_compaction": initial_context_messages.iter().filter(|message| message.role == MessageRole::Tool).count(),
            "messages_after_compaction": initial_compaction.messages.len(),
            "tool_results_after_compaction": initial_compaction.messages.iter().filter(|message| message.role == MessageRole::Tool).count(),
            "compaction_budget_tokens": initial_compaction.stats.budget_tokens,
            "compaction_original_estimated_tokens": initial_compaction.stats.original_estimated_tokens,
            "compaction_result_estimated_tokens": initial_compaction.stats.compacted_estimated_tokens,
            "compaction_budget_satisfied": initial_compaction.stats.budget_satisfied,
            "compressed": initial_compaction.compressed,
        }),
    );
    if runtime_changed || initial_compaction.compressed {
        let control = AgentMessage::system(runtime_context.clone());
        initial_context_messages.push(control.clone());
        initial_compaction.messages.push(control);
    }
    let initial_context_tokens = estimate_agent_request_tokens(
        &initial_context_messages,
        &base_system_prompt,
        request_tools,
    )
    .saturating_add(image_token_reserve.load(Ordering::Relaxed));
    let initial_compaction_id = format!("compaction-{}", uuid_suffix());
    let mut model_summary_attempted = false;
    let mut model_summary_requested = false;
    // P9-G：上一轮压缩时预置的确定性检查点摘要，下一轮压缩直接复用同一份，
    // 避免「每轮现生成一份新的」把前缀打散。
    let planned_compaction_summary: Option<(
        String,
        Option<agent_core::AgentContextCheckpointPayload>,
    )> = None;
    if initial_compaction.compressed {
        let processed_messages = initial_compaction.stats.covered_message_count;
        let initial_summary_budget_chars = initial_compaction
            .stats
            .budget_tokens
            .saturating_sub(initial_compaction.stats.compacted_estimated_tokens)
            .saturating_sub(96)
            .min(1_024)
            .saturating_mul(3);
        emit_agent_context_for_turn(
            app,
            json!({
                "session_id": session_id, "compaction_id": initial_compaction_id,
                "compaction_status": "running", "compaction_reason": "history_budget",
                "before_context_tokens": initial_context_tokens,
                "reserved_output_tokens": telemetry_output_reserve, "context_compressed": true,
            }),
            Some(turn_id),
        );
        emit_agent_status_for_turn(
            app,
            session_id,
            "compacting_context",
            "正在整理较早上下文，模型摘要不可用时将保留确定性检查点",
            Some(turn_id),
        );
        super::events::emit_compaction_progress(
            app,
            session_id,
            turn_id,
            &initial_compaction_id,
            0,
            processed_messages,
            initial_context_tokens,
            initial_summary_budget_chars,
        );
        let (requested, summary_usage) = enrich_context_checkpoint_with_model(
            client,
            ProviderRequest::new(
                session_id,
                &selected_model.id,
                Vec::new(),
                request_tools.to_vec(),
            )
            .with_system_prompt(base_system_prompt.clone())
            .with_reasoning_effort(reasoning_effort.map(str::to_string))
            .with_native_reasoning_effort(native_reasoning_effort)
            .with_reasoning_encrypted_content(!responses_reasoning_include_fallback)
            .with_usage_reporting(model_supports_usage && !compatible_usage_fallback),
            &initial_context_messages,
            &mut initial_compaction,
            &mut model_summary_attempted,
            state,
            cancellation,
            |characters| {
                super::events::emit_compaction_progress(
                    app,
                    session_id,
                    turn_id,
                    &initial_compaction_id,
                    characters,
                    processed_messages,
                    initial_context_tokens,
                    initial_summary_budget_chars,
                )
            },
        )
        .await;
        model_summary_requested = requested;
        if let Some(summary_usage) = summary_usage {
            merge_agent_usage(usage, summary_usage);
        }
        if is_cancelled(state, session_id, cancellation) {
            return Err(cancellation_message(cancellation_reason));
        }
    }
    Ok(ContextInitialization {
        base_system_prompt,
        runtime_context,
        transcript_scope,
        prompt_cache_key,
        cache_fingerprint,
        runtime_context_tokens,
        telemetry_output_reserve,
        context_budget,
        initial_context_tokens,
        initial_compaction_id,
        model_summary_attempted,
        model_summary_requested,
        planned_compaction_summary,
        initial_context_compressed: initial_compaction.compressed,
        initial_context_summary: initial_compaction.summary,
        initial_context_checkpoint_payload: initial_compaction.checkpoint_payload,
        initial_covered_message_count: initial_compaction.covered_message_count,
        initial_covered_message_id: initial_compaction.covered_message_id,
        restored_checkpoint_context,
        messages: initial_compaction.messages,
        initial_context_status: initial_compaction.status,
        initial_context_stats: initial_compaction.stats,
    })
}
