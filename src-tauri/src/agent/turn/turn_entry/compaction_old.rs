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
