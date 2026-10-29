use crate::{AgentChatRequest, AgentToolPolicy};

pub fn effective_tool_policy(request: &AgentChatRequest) -> AgentToolPolicy {
    request.tool_policy
}

/// The model receives the complete tool set for every tool-enabled turn and
/// chooses whether a function call is needed. Runtime code must not infer a
/// tool call from wording in the user message.
pub fn should_enable_agent_tools(request: &AgentChatRequest) -> bool {
    effective_tool_policy(request) == AgentToolPolicy::Auto
}
