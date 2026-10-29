use super::super::budget::CompactionResult;
use super::compaction_core::{compact_messages, compact_messages_with_current};
use crate::{AgentContextTurn, AgentMessage, MessageRole};

pub fn context_turn_message(turn: &AgentContextTurn) -> Option<AgentMessage> {
    if turn.content.trim().is_empty() && turn.artifacts.is_empty() {
        return None;
    }
    let role = match turn.role.to_ascii_lowercase().as_str() {
        "assistant" => MessageRole::Assistant,
        "system" => MessageRole::System,
        _ => MessageRole::User,
    };
    let mut content = if role == MessageRole::Assistant {
        crate::output_channels::public_history_text(&turn.content)
    } else {
        turn.content.clone()
    };
    if role == MessageRole::User && !turn.artifacts.is_empty() {
        content.push_str("\n\n[此历史用户消息提供的本机来源元数据]\n");
        content.push_str(&crate::local_source_metadata_text(&turn.artifacts));
    }
    if content.trim().is_empty() {
        return None;
    }
    Some(AgentMessage {
        role,
        content: content.trim().to_string(),
        reasoning_content: None,
        responses_reasoning_items: Vec::new(),
        anthropic_thinking_blocks: Vec::new(),
        anthropic_content_blocks: Vec::new(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        source_message_id: turn.message_id.clone(),
    })
}

pub fn build_agent_messages(
    context: &[AgentContextTurn],
    prompt: &str,
    max_tokens: usize,
) -> (
    Vec<AgentMessage>,
    bool,
    Option<String>,
    Option<usize>,
    Option<String>,
) {
    let result = build_agent_messages_detailed(context, prompt, max_tokens);
    (
        result.messages,
        result.compressed,
        result.summary,
        result.covered_message_count,
        result.covered_message_id,
    )
}

pub fn build_agent_messages_detailed(
    context: &[AgentContextTurn],
    prompt: &str,
    max_tokens: usize,
) -> CompactionResult {
    let mut history = context
        .iter()
        .filter_map(context_turn_message)
        .collect::<Vec<_>>();
    if history.last().is_some_and(|message| {
        message.role == MessageRole::User && message.content.trim() == prompt.trim()
    }) {
        history.pop();
    }
    compact_messages_with_current(&history, AgentMessage::user(prompt), max_tokens)
}

pub fn compact_agent_messages(
    messages: &[AgentMessage],
    max_tokens: usize,
) -> (
    Vec<AgentMessage>,
    bool,
    Option<String>,
    Option<usize>,
    Option<String>,
) {
    let result = compact_messages(messages, max_tokens);
    (
        result.messages,
        result.compressed,
        result.summary,
        result.covered_message_count,
        result.covered_message_id,
    )
}

pub fn context_message_id_at_boundary(
    context: &[AgentContextTurn],
    covered_message_count: Option<usize>,
) -> Option<String> {
    let count = covered_message_count?;
    count
        .checked_sub(1)
        .and_then(|index| {
            context
                .iter()
                .filter(|turn| !turn.content.trim().is_empty())
                .nth(index)
                .and_then(|turn| turn.message_id.as_deref())
        })
        .map(str::to_string)
}
