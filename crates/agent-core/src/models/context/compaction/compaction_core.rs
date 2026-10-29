use super::super::budget::{
    estimate_messages_tokens, estimate_text_tokens, CompactionResult, CompactionStats,
    CompactionStatus,
};
use super::budget_fit::fit_messages_to_budget;
use super::checkpoint_messages::checkpoint_entries_to_messages;
use super::consts::CONTEXT_SUMMARY_PREFIX;
use super::history_window::{recent_history_start, unchanged_compaction_result};
pub(super) use super::semantic_checkpoint::build_semantic_checkpoint;
use super::triggers::{
    compaction_trigger_tokens, compaction_trigger_tokens_with_overhead, retained_recent_tokens,
    CHECKPOINT_TOKEN_CEILING,
};
use crate::{AgentContextCheckpointPayload, AgentMessage, MessageRole};

pub fn compact_messages_with_current(
    history: &[AgentMessage],
    current: AgentMessage,
    max_tokens: usize,
) -> CompactionResult {
    compact_messages_with_current_with_overhead(history, current, max_tokens, 0)
}

pub fn plan_compaction_checkpoint(
    source: &[AgentMessage],
    max_tokens: usize,
    trigger_tokens: usize,
    suffix_tokens: usize,
) -> Option<(String, Option<AgentContextCheckpointPayload>)> {
    if source.is_empty() || trigger_tokens == 0 {
        return None;
    }
    if estimate_messages_tokens(source) <= trigger_tokens {
        return None;
    }
    let checkpoint_budget = (max_tokens / 3)
        .max(24)
        .min(CHECKPOINT_TOKEN_CEILING)
        .min(max_tokens.saturating_sub(16))
        .max(1);
    let retain_tokens = retained_recent_tokens(max_tokens, checkpoint_budget);
    let recent_start = recent_history_start(source, retain_tokens.saturating_add(suffix_tokens));
    let checkpoint = build_semantic_checkpoint(&source[..recent_start], checkpoint_budget);
    checkpoint
        .payload()
        .map(|payload| (checkpoint.text.clone(), Some(payload)))
}

pub fn compact_messages_with_current_with_overhead(
    history: &[AgentMessage],
    current: AgentMessage,
    max_tokens: usize,
    overhead_tokens: usize,
) -> CompactionResult {
    compact_messages_with_current_at(
        history,
        current,
        max_tokens,
        compaction_trigger_tokens_with_overhead(max_tokens, overhead_tokens),
    )
}

fn compact_messages_with_current_at(
    history: &[AgentMessage],
    current: AgentMessage,
    max_tokens: usize,
    trigger_tokens: usize,
) -> CompactionResult {
    let mut full = history.to_vec();
    full.push(current.clone());
    let original_estimated_tokens = estimate_messages_tokens(&full);
    let original_message_count = full.len();
    if original_estimated_tokens <= trigger_tokens {
        return unchanged_compaction_result(full, max_tokens);
    }

    let prefix_tokens = estimate_text_tokens(CONTEXT_SUMMARY_PREFIX);
    let checkpoint_budget = (max_tokens / 3)
        .max((prefix_tokens + 24).min(max_tokens / 2))
        .min(CHECKPOINT_TOKEN_CEILING)
        .min(max_tokens.saturating_sub(16));
    let retain_tokens = retained_recent_tokens(max_tokens, checkpoint_budget);
    let recent_start = recent_history_start(history, retain_tokens);
    let covered_message_id = history[..recent_start]
        .iter()
        .rev()
        .find_map(|message| message.source_message_id.clone());
    let checkpoint = build_semantic_checkpoint(&history[..recent_start], checkpoint_budget);
    let checkpoint_payload = checkpoint.payload();
    let mut messages = Vec::with_capacity(history.len() + 1);
    if let Some(payload) = checkpoint_payload.as_ref() {
        messages.extend(checkpoint_entries_to_messages(&payload.entries));
    }
    messages.extend(history[recent_start..].iter().cloned());
    messages.push(current);
    let checkpoint_present = checkpoint_payload.is_some();
    let protected_current_index = messages.len().saturating_sub(1);
    let fit = fit_messages_to_budget(&mut messages, max_tokens, protected_current_index);
    let actual_summary = checkpoint_present.then(|| checkpoint.text.clone());
    let compacted_estimated_tokens = estimate_messages_tokens(&messages);
    let compacted_message_count = messages.len();
    let checkpoint_estimated_tokens = actual_summary
        .as_deref()
        .map(estimate_text_tokens)
        .unwrap_or_default();
    let summary_retained = actual_summary.is_some();
    let status = if !checkpoint_present {
        CompactionStatus::TruncatedToBudget
    } else if fit.truncated_message_count > 0 || fit.dropped_recent_messages > 0 {
        CompactionStatus::CheckpointedWithTruncation
    } else {
        CompactionStatus::Checkpointed
    };

    CompactionResult {
        messages,
        compressed: true,
        status,
        summary: actual_summary,
        covered_message_count: summary_retained.then_some(recent_start),
        covered_message_id: summary_retained.then_some(covered_message_id).flatten(),
        checkpoint_payload,
        stats: CompactionStats {
            budget_tokens: max_tokens,
            original_estimated_tokens,
            compacted_estimated_tokens,
            original_message_count,
            compacted_message_count,
            covered_message_count: if summary_retained { recent_start } else { 0 },
            retained_recent_message_count: history
                .len()
                .saturating_sub(recent_start)
                .saturating_sub(fit.dropped_recent_messages),
            checkpoint_estimated_tokens,
            system_constraint_count: checkpoint.system_constraint_count,
            pending_user_item_count: checkpoint.pending_user_item_count,
            tool_evidence_count: checkpoint.tool_evidence_count,
            historical_conclusion_count: checkpoint.historical_conclusion_count,
            truncated_message_count: fit.truncated_message_count + fit.dropped_recent_messages,
            checkpoint_truncated: checkpoint.truncated,
            budget_satisfied: compacted_estimated_tokens <= max_tokens,
        },
    }
}

pub fn compact_messages(messages: &[AgentMessage], max_tokens: usize) -> CompactionResult {
    compact_messages_with_trigger(messages, max_tokens, compaction_trigger_tokens(max_tokens))
}

/// [`compact_messages`] with the request's fixed overhead discounted from the
/// ceiling.
pub fn compact_messages_with_overhead(
    messages: &[AgentMessage],
    max_tokens: usize,
    overhead_tokens: usize,
) -> CompactionResult {
    compact_messages_with_trigger(
        messages,
        max_tokens,
        compaction_trigger_tokens_with_overhead(max_tokens, overhead_tokens),
    )
}

/// `compact_messages` with an explicit trigger, so the caller can pass one scaled
/// by the provider's measured drift over the local estimate (see
/// [`compaction_trigger_tokens_scaled`]).
pub fn compact_messages_with_trigger(
    messages: &[AgentMessage],
    max_tokens: usize,
    trigger_tokens: usize,
) -> CompactionResult {
    let original_estimated_tokens = estimate_messages_tokens(messages);
    if original_estimated_tokens <= trigger_tokens {
        return unchanged_compaction_result(messages.to_vec(), max_tokens);
    }

    let current_index = messages
        .iter()
        .rposition(|message| message.role == MessageRole::User)
        .unwrap_or(messages.len().saturating_sub(1));
    let history = &messages[..current_index];
    let current = messages
        .get(current_index)
        .cloned()
        .unwrap_or_else(|| AgentMessage::user(""));
    let suffix = messages.get(current_index + 1..).unwrap_or_default();
    let suffix_tokens = estimate_messages_tokens(suffix);
    let mut result = compact_messages_with_current_at(
        history,
        current,
        max_tokens
            .saturating_sub(suffix_tokens)
            .max(max_tokens / 2)
            .max(1)
            .min(max_tokens),
        // This caller gated on the *whole* request. The in-flight tool suffix
        // after the last user message cannot be summarized, so re-testing the
        // base against the same trigger leaves a request over the trigger with
        // nothing compacted whenever the base sits just under it and the suffix
        // pushes it over (measured on a 1.6K-message session: base 139K, suffix
        // 21K, request 174K → twelve rounds that announced a compaction and
        // replaced an identical transcript). Measuring the base against
        // `trigger - suffix` keeps the inner gate equivalent to
        // `base + suffix > trigger`, which is exactly what this caller already
        // decided on.
        trigger_tokens.saturating_sub(suffix_tokens).max(1),
    );
    // `compact_messages_with_current` places the current user request at the
    // end of its result. Preserve that index before appending any in-flight
    // tool suffix; otherwise the suffix's last tool result is mistaken for
    // the current request and the request can be evicted under pressure.
    let protected_current_index = result.messages.len().saturating_sub(1);
    result.messages.extend_from_slice(suffix);
    let checkpoint_present = result.checkpoint_payload.is_some();
    let fit = fit_messages_to_budget(&mut result.messages, max_tokens, protected_current_index);
    // The outer trigger measures the *whole* request, but the suffix above it is
    // not compactible. A rewrite of a base that is already at the floor hands
    // back the same transcript, so a compaction is only reported when the
    // request actually got smaller: otherwise the caller announces a compaction
    // (the strip renders "187,269 → 187,269 Token") and pays a summary
    // model round-trip for nothing.
    let compacted_estimated_tokens = estimate_messages_tokens(&result.messages);
    let changed = compacted_estimated_tokens < original_estimated_tokens
        || fit.truncated_message_count > 0
        || fit.dropped_recent_messages > 0;
    result.compressed = changed;
    result.status = if !changed {
        CompactionStatus::NotRequired
    } else if !checkpoint_present {
        CompactionStatus::TruncatedToBudget
    } else if result.status == CompactionStatus::CheckpointedWithTruncation
        || fit.truncated_message_count > 0
        || fit.dropped_recent_messages > 0
    {
        CompactionStatus::CheckpointedWithTruncation
    } else {
        CompactionStatus::Checkpointed
    };
    result.stats.budget_tokens = max_tokens;
    result.stats.original_estimated_tokens = original_estimated_tokens;
    result.stats.original_message_count = messages.len();
    result.stats.compacted_estimated_tokens = compacted_estimated_tokens;
    result.stats.compacted_message_count = result.messages.len();
    result.stats.checkpoint_estimated_tokens = result
        .summary
        .as_deref()
        .map(estimate_text_tokens)
        .unwrap_or_default();
    result.stats.retained_recent_message_count = result
        .stats
        .retained_recent_message_count
        .saturating_add(suffix.len())
        .saturating_sub(fit.dropped_recent_messages);
    result.stats.truncated_message_count = result
        .stats
        .truncated_message_count
        .saturating_add(fit.truncated_message_count)
        .saturating_add(fit.dropped_recent_messages);
    result.stats.budget_satisfied = result.stats.compacted_estimated_tokens <= max_tokens;
    result
}
