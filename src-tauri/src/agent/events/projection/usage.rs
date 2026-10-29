use super::super::*;

pub(crate) fn context_usage(
    session_id: Option<&str>,
    turn_id: Option<&str>,
    event_type: Option<&str>,
    _payload: &Value,
    number: impl Fn(&str, &str) -> Option<u64>,
) -> Option<ThreadEvent> {
    let (Some(thread_id), Some(turn_id), Some("context")) = (session_id, turn_id, event_type)
    else {
        return None;
    };
    Some(ThreadEvent::UsageUpdated {
        thread_id: thread_id.to_string(),
        turn_id: turn_id.to_string(),
        usage: ThreadUsageSnapshot {
            provider_kind: None,
            latest_request_usage: None,
            input_tokens: number("input_tokens", "inputTokens"),
            output_tokens: number("output_tokens", "outputTokens"),
            generation_tokens: None,
            generation_duration_ms: None,
            generation_timing_source: None,
            cached_input_tokens: number("cached_input_tokens", "cachedInputTokens"),
            cache_creation_input_tokens: number(
                "cache_creation_input_tokens",
                "cacheCreationInputTokens",
            ),
            context_window: number("context_window", "contextWindow"),
            context_tokens: number("context_tokens", "contextTokens"),
            reserved_output_tokens: number("reserved_output_tokens", "reservedOutputTokens"),
            available_tokens: number("available_tokens", "availableTokens"),
            // A context update carries no usage report of its own; the
            // previously reported reasoning count is preserved by merge.
            reasoning_tokens: None,
        },
    })
}
