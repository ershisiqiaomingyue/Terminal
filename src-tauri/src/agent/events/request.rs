use super::super::*;

pub(crate) fn request_session_id(request: &AgentChatRequest) -> String {
    request
        .session_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("agent-{}", uuid_suffix()))
}
