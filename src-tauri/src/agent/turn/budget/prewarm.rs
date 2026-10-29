use super::super::*;

pub(super) struct PrewarmInput<'a> {
    pub app: &'a AppHandle,
    pub client: &'a HttpProviderClient,
    pub session_id: &'a str,
    pub turn_id: &'a str,
    pub selected_model_id: &'a str,
    pub messages: Vec<AgentMessage>,
    pub request_tools: &'a [ToolDefinition],
    pub system_prompt: &'a str,
    pub before_context_tokens: usize,
    pub round_compaction_trigger: usize,
    pub model_supports_usage: bool,
    pub compatible_usage_fallback: bool,
    pub model_request_count: &'a std::cell::Cell<u64>,
}

pub(super) async fn prewarm_before_compaction(input: PrewarmInput<'_>) {
    let PrewarmInput {
        app,
        client,
        session_id,
        turn_id,
        selected_model_id,
        mut messages,
        request_tools,
        system_prompt,
        before_context_tokens,
        round_compaction_trigger,
        model_supports_usage,
        compatible_usage_fallback,
        model_request_count,
    } = input;
    let mut prewarm = ProviderRequest::new(session_id, selected_model_id, Vec::new(), request_tools.to_vec())
        .with_system_prompt(system_prompt.to_string())
        .with_max_tokens(16)
        .with_usage_reporting(model_supports_usage && !compatible_usage_fallback);
    messages.push(AgentMessage::user("P9-F prewarm"));
    prewarm.messages = messages;
    match client.stream(prewarm).await {
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
}
