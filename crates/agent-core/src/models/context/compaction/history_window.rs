use super::super::budget::{
    estimate_message_tokens, estimate_messages_tokens, CompactionResult, CompactionStats,
    CompactionStatus,
};
use super::triggers::RETAIN_MIN_MESSAGES;
use crate::{AgentMessage, MessageRole};

pub(super) fn recent_history_start(history: &[AgentMessage], retain_tokens: usize) -> usize {
    if history.is_empty() {
        return 0;
    }
    let mut start = history.len();
    let mut accumulated = 0usize;
    while start > 0 {
        let candidate = estimate_message_tokens(&history[start - 1]);
        let retained = history.len().saturating_sub(start);
        if retained >= RETAIN_MIN_MESSAGES && accumulated.saturating_add(candidate) > retain_tokens
        {
            break;
        }
        start -= 1;
        accumulated = accumulated.saturating_add(candidate);
    }
    while start < history.len().saturating_sub(1) && history[start].role == MessageRole::Tool {
        let owned = start.checked_sub(1).is_some_and(|index| {
            history[index].role == MessageRole::Assistant && !history[index].tool_calls.is_empty()
        });
        if owned {
            start -= 1;
            break;
        }
        start += 1;
    }
    if start > 0
        && history[start - 1].role == MessageRole::Assistant
        && !history[start - 1].tool_calls.is_empty()
    {
        return start - 1;
    }
    if history.last().is_some_and(|message| {
        message.role == MessageRole::Assistant && !message.tool_calls.is_empty()
    }) {
        return history.len() - 1;
    }
    start
}

pub(super) fn unchanged_compaction_result(
    messages: Vec<AgentMessage>,
    max_tokens: usize,
) -> CompactionResult {
    let estimated = estimate_messages_tokens(&messages);
    let message_count = messages.len();
    CompactionResult {
        messages,
        compressed: false,
        status: CompactionStatus::NotRequired,
        summary: None,
        covered_message_count: None,
        covered_message_id: None,
        checkpoint_payload: None,
        stats: CompactionStats {
            budget_tokens: max_tokens,
            original_estimated_tokens: estimated,
            compacted_estimated_tokens: estimated,
            original_message_count: message_count,
            compacted_message_count: message_count,
            covered_message_count: 0,
            retained_recent_message_count: message_count,
            checkpoint_estimated_tokens: 0,
            system_constraint_count: 0,
            pending_user_item_count: 0,
            tool_evidence_count: 0,
            historical_conclusion_count: 0,
            truncated_message_count: 0,
            checkpoint_truncated: false,
            budget_satisfied: estimated <= max_tokens,
        },
    }
}
