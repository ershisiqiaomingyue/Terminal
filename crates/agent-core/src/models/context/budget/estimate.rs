use super::{
    ContextRetryBudget, ContextWindowResolution, ContextWindowStatus, DEFAULT_CONTEXT_WINDOW,
    FINAL_OUTPUT_TOKENS, MIN_CONTEXT_WINDOW, MIN_FINAL_OUTPUT_TOKENS, MIN_TOOL_OUTPUT_TOKENS,
    OUTPUT_LIMIT_THRESHOLD_PERCENT, TOOL_OUTPUT_TOKENS, UNKNOWN_CONTEXT_RETRY_WINDOWS,
};
use crate::{AgentMessage, ToolDefinition};

pub fn estimate_text_tokens(value: &str) -> usize {
    // Code and JSON are ASCII-heavy while Chinese text is close to one token
    // per character. The split is intentionally conservative for preflight.
    let non_ascii = value
        .chars()
        .filter(|character| !character.is_ascii())
        .count();
    let ascii = value.chars().count().saturating_sub(non_ascii);
    ascii.saturating_add(3) / 4 + non_ascii
}

pub fn estimate_message_tokens(message: &AgentMessage) -> usize {
    if !message.anthropic_content_blocks.is_empty() {
        return 8 + serde_json::to_string(&message.anthropic_content_blocks)
            .map(|value| estimate_text_tokens(&value))
            .unwrap_or_default();
    }
    8 + estimate_text_tokens(&message.content)
        + message
            .reasoning_content
            .as_deref()
            .map(estimate_text_tokens)
            .unwrap_or_default()
        + message
            .tool_call_id
            .as_deref()
            .map(estimate_text_tokens)
            .unwrap_or_default()
        + serde_json::to_string(&message.tool_calls)
            .map(|value| estimate_text_tokens(&value))
            .unwrap_or_default()
        + serde_json::to_string(&message.responses_reasoning_items)
            .map(|value| estimate_text_tokens(&value))
            .unwrap_or_default()
        + serde_json::to_string(&message.anthropic_thinking_blocks)
            .map(|value| estimate_text_tokens(&value))
            .unwrap_or_default()
}

pub fn estimate_messages_tokens(messages: &[AgentMessage]) -> usize {
    messages.iter().map(estimate_message_tokens).sum()
}

pub fn estimate_request_tokens(
    messages: &[AgentMessage],
    system_prompt: &str,
    tools: &[ToolDefinition],
) -> usize {
    estimate_messages_tokens(messages)
        .saturating_add(estimate_text_tokens(system_prompt))
        .saturating_add(estimate_tools_tokens(tools))
}

fn estimate_tools_tokens(tools: &[ToolDefinition]) -> usize {
    if tools.is_empty() {
        return 0;
    }
    serde_json::to_string(tools)
        .map(|value| estimate_text_tokens(&value))
        .unwrap_or_default()
}

pub fn resolve_context_window(context_window: Option<u32>) -> ContextWindowResolution {
    match context_window.filter(|value| (*value as usize) >= MIN_CONTEXT_WINDOW) {
        Some(value) => ContextWindowResolution {
            status: ContextWindowStatus::Known,
            reported_tokens: Some(value as usize),
            effective_tokens: (value as usize).max(MIN_CONTEXT_WINDOW),
            conservative: false,
        },
        None => ContextWindowResolution {
            status: ContextWindowStatus::Unknown,
            reported_tokens: None,
            effective_tokens: DEFAULT_CONTEXT_WINDOW,
            conservative: true,
        },
    }
}

pub fn effective_context_window(context_window: Option<u32>) -> usize {
    resolve_context_window(context_window).effective_tokens
}

pub fn output_reserve(context_window: Option<u32>, final_response_requested: bool) -> usize {
    let window = effective_context_window(context_window);
    let requested = if final_response_requested {
        FINAL_OUTPUT_TOKENS
    } else {
        TOOL_OUTPUT_TOKENS
    };
    let minimum = if final_response_requested {
        MIN_FINAL_OUTPUT_TOKENS
    } else {
        MIN_TOOL_OUTPUT_TOKENS
    };
    requested
        .min(window.saturating_mul(25) / 100)
        .max(minimum.min(window))
}

pub fn context_budget(
    context_window: Option<u32>,
    system_prompt: &str,
    tools: &[ToolDefinition],
    final_response_requested: bool,
) -> usize {
    let window = effective_context_window(context_window);
    let reserved = output_reserve(context_window, final_response_requested)
        .saturating_add(estimate_text_tokens(system_prompt))
        .saturating_add(estimate_tools_tokens(tools));
    window.saturating_sub(reserved).max(1)
}

/// Return the message-only budget for an unknown-window retry. Attempts are
/// one-based so they can be used directly with the runtime retry counter.
pub fn unknown_context_retry_budget(
    attempt: usize,
    system_prompt: &str,
    tools: &[ToolDefinition],
    image_tokens: usize,
    final_response_requested: bool,
) -> Option<ContextRetryBudget> {
    let target_context_window = *UNKNOWN_CONTEXT_RETRY_WINDOWS.get(attempt.checked_sub(1)?)?;
    let reserved_output_tokens = output_reserve(
        u32::try_from(target_context_window).ok(),
        final_response_requested,
    );
    let fixed_overhead_tokens = estimate_text_tokens(system_prompt)
        .saturating_add(estimate_tools_tokens(tools))
        .saturating_add(image_tokens)
        .saturating_add(reserved_output_tokens);
    let message_budget = target_context_window.checked_sub(fixed_overhead_tokens)?;
    (message_budget > 0).then_some(ContextRetryBudget {
        target_context_window,
        message_budget,
        fixed_overhead_tokens,
        reserved_output_tokens,
    })
}

pub fn available_tokens(
    context_window: Option<u32>,
    context_tokens: usize,
    reserved_output_tokens: usize,
) -> usize {
    effective_context_window(context_window)
        .saturating_sub(context_tokens)
        .saturating_sub(reserved_output_tokens)
}

pub fn usage_percent(context_tokens: usize, context_window: usize) -> u8 {
    if context_window == 0 {
        return 0;
    }
    ((context_tokens.saturating_mul(100) / context_window).min(100)) as u8
}

pub fn output_limit_reached(
    output_tokens: Option<u64>,
    max_output_tokens: u32,
    output_text: &str,
) -> bool {
    let max_tokens = max_output_tokens as u64;
    if max_tokens == 0 {
        return false;
    }
    if output_tokens.is_some_and(|tokens| tokens.saturating_add(1) >= max_tokens) {
        return true;
    }
    output_tokens.is_none()
        && (estimate_text_tokens(output_text) as u64).saturating_mul(100)
            >= max_tokens.saturating_mul(OUTPUT_LIMIT_THRESHOLD_PERCENT as u64)
}
