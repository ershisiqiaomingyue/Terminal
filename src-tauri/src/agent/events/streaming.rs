use super::super::*;

use super::context::emit_agent_context_for_turn;

#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_streaming_context_snapshot(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    context_tokens: usize,
    context_tokens_estimated: bool,
    context_window: Option<u32>,
    reserved_output_tokens: usize,
    provider_input_tokens: Option<u64>,
    provider_output_tokens: Option<u64>,
    system_prompt_tokens: usize,
    tool_definition_tokens: usize,
    model_id: &str,
    provider_id: &str,
    // P8-F：本轮实际使用的压缩触发点（消息级预算扣固定前缀后）。
    // UI 据此显示「压缩将在 ~Z 触发」，与占用百分比的分母各自明确。
    compaction_trigger_tokens: Option<usize>,
) {
    let resolution = agent_core::resolve_context_window(context_window);
    emit_agent_context_for_turn(
        app,
        json!({
            "session_id": session_id,
            "context_tokens": context_tokens,
            "system_prompt_tokens": system_prompt_tokens,
            "tool_definition_tokens": tool_definition_tokens,
            "model_id": model_id,
            "provider_id": provider_id,
            "context_tokens_estimated": context_tokens_estimated,
            // Keep the declared window absent when model metadata is unknown.
            // The conservative runtime fallback is reported separately.
            "context_window": context_window,
            "context_window_estimated": !resolution.is_known(),
            "context_window_status": resolution.status,
            "effective_context_window": resolution.effective_tokens,
            "context_window_conservative": resolution.conservative,
            "context_percent": context_window
                .map(|window| context_usage_percent(context_tokens, window as usize)),
            "reserved_output_tokens": reserved_output_tokens,
            "available_tokens": available_agent_context_tokens(
                context_window,
                context_tokens,
                reserved_output_tokens,
            ),
            "input_tokens": provider_input_tokens,
            "output_tokens": provider_output_tokens,
            "compaction_trigger_tokens": compaction_trigger_tokens,
            "streaming": true,
        }),
        Some(turn_id),
    );
}
