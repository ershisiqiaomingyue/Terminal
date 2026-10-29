use super::super::budget::{estimate_message_tokens, estimate_text_tokens};
use super::consts::{MESSAGE_CONTENT_MARKER, TOOL_OUTPUT_MARKER};
use super::text_trim::truncate_text_to_token_budget;
use crate::{AgentMessage, MessageRole};
use std::collections::HashSet;

#[derive(Debug, Default)]
pub(super) struct FitOutcome {
    pub(super) truncated_message_count: usize,
    pub(super) dropped_recent_messages: usize,
}

fn latest_tool_group_indices(messages: &[AgentMessage]) -> HashSet<usize> {
    let Some(call_index) = messages.iter().rposition(|message| {
        message.role == MessageRole::Assistant && !message.tool_calls.is_empty()
    }) else {
        return HashSet::new();
    };
    let call_ids = messages[call_index]
        .tool_calls
        .iter()
        .map(|call| call.id.as_str())
        .collect::<HashSet<_>>();
    let mut protected = HashSet::from([call_index]);
    for (index, message) in messages.iter().enumerate().skip(call_index + 1) {
        if message.role == MessageRole::Assistant && !message.tool_calls.is_empty() {
            break;
        }
        if message.role == MessageRole::Tool
            && message
                .tool_call_id
                .as_deref()
                .is_some_and(|call_id| call_ids.contains(call_id))
        {
            protected.insert(index);
        }
    }
    protected
}

fn replace_message_tokens(
    messages: &[AgentMessage],
    token_counts: &mut [usize],
    total: &mut usize,
    index: usize,
) {
    let before = token_counts[index];
    let after = estimate_message_tokens(&messages[index]);
    token_counts[index] = after;
    *total = total.saturating_sub(before).saturating_add(after);
}

fn shrink_message_content(
    messages: &mut [AgentMessage],
    token_counts: &mut [usize],
    total: &mut usize,
    index: usize,
    max_tokens: usize,
    marker: &str,
) -> bool {
    if *total <= max_tokens {
        return false;
    }
    let Some(message) = messages.get(index) else {
        return false;
    };
    if !message.anthropic_content_blocks.is_empty() {
        return false;
    }
    let original = message.content.clone();
    if original.is_empty() {
        return false;
    }
    let source = original
        .strip_suffix(marker)
        .unwrap_or(original.as_str())
        .trim_end();
    let content_tokens = estimate_text_tokens(&original);
    let target = content_tokens
        .saturating_sub(total.saturating_sub(max_tokens))
        .saturating_sub(1);
    let mut bounded = truncate_text_to_token_budget(source, target, marker);
    if bounded == original {
        bounded = if estimate_text_tokens(marker) <= target {
            marker.to_string()
        } else {
            String::new()
        };
    }
    if bounded == original {
        return false;
    }
    messages[index].content = bounded;
    replace_message_tokens(messages, token_counts, total, index);
    true
}

fn shrink_ranked_content<F>(
    messages: &mut [AgentMessage],
    token_counts: &mut [usize],
    total: &mut usize,
    max_tokens: usize,
    marker: &str,
    mut eligible: F,
    truncated: &mut HashSet<usize>,
) where
    F: FnMut(usize, &AgentMessage) -> bool,
{
    let mut candidates = messages
        .iter()
        .enumerate()
        .filter(|(index, message)| eligible(*index, message) && !message.content.is_empty())
        .map(|(index, message)| (index, estimate_text_tokens(&message.content)))
        .collect::<Vec<_>>();
    candidates.sort_unstable_by_key(|(_, tokens)| std::cmp::Reverse(*tokens));
    for (index, _) in candidates {
        if *total <= max_tokens {
            break;
        }
        if shrink_message_content(messages, token_counts, total, index, max_tokens, marker) {
            truncated.insert(index);
        }
    }
}

fn removable_message_groups(messages: &[AgentMessage], protected: &[bool]) -> Vec<Vec<usize>> {
    let mut groups = Vec::new();
    let mut index = 0usize;
    while index < messages.len() {
        if messages[index].role == MessageRole::Assistant && !messages[index].tool_calls.is_empty()
        {
            let call_ids = messages[index]
                .tool_calls
                .iter()
                .map(|call| call.id.as_str())
                .collect::<HashSet<_>>();
            let mut group = vec![index];
            let mut next = index + 1;
            while next < messages.len() && messages[next].role == MessageRole::Tool {
                if messages[next]
                    .tool_call_id
                    .as_deref()
                    .is_some_and(|call_id| call_ids.contains(call_id))
                {
                    group.push(next);
                }
                next += 1;
            }
            if group.iter().all(|item| !protected[*item]) {
                groups.push(group);
            }
            index = next;
            continue;
        }
        if !protected[index] {
            groups.push(vec![index]);
        }
        index += 1;
    }
    groups
}

pub(super) fn fit_messages_to_budget(
    messages: &mut Vec<AgentMessage>,
    max_tokens: usize,
    protected_current_index: usize,
) -> FitOutcome {
    let mut truncated = HashSet::new();
    let mut protected = vec![false; messages.len()];
    let mut current_flags = vec![false; messages.len()];
    if let Some(value) = protected.get_mut(protected_current_index) {
        *value = true;
    }
    if let Some(value) = current_flags.get_mut(protected_current_index) {
        *value = true;
    }
    for index in latest_tool_group_indices(messages) {
        if let Some(value) = protected.get_mut(index) {
            *value = true;
        }
    }
    let mut token_counts = messages
        .iter()
        .map(estimate_message_tokens)
        .collect::<Vec<_>>();
    let mut total = token_counts.iter().copied().sum::<usize>();

    for index in 0..messages.len() {
        if total <= max_tokens {
            break;
        }
        if !protected[index]
            && (messages[index].reasoning_content.take().is_some()
                || !messages[index].responses_reasoning_items.is_empty())
        {
            messages[index].responses_reasoning_items.clear();
            truncated.insert(index);
            replace_message_tokens(messages, &mut token_counts, &mut total, index);
        }
    }

    shrink_ranked_content(
        messages,
        &mut token_counts,
        &mut total,
        max_tokens,
        TOOL_OUTPUT_MARKER,
        |index, message| message.role == MessageRole::Tool && !protected[index],
        &mut truncated,
    );
    shrink_ranked_content(
        messages,
        &mut token_counts,
        &mut total,
        max_tokens,
        MESSAGE_CONTENT_MARKER,
        |index, message| {
            !protected[index]
                && message.role == MessageRole::Assistant
                && message.tool_calls.is_empty()
        },
        &mut truncated,
    );
    shrink_ranked_content(
        messages,
        &mut token_counts,
        &mut total,
        max_tokens,
        MESSAGE_CONTENT_MARKER,
        |index, message| {
            !protected[index] && matches!(message.role, MessageRole::User | MessageRole::System)
        },
        &mut truncated,
    );

    let mut call_candidates = messages
        .iter()
        .enumerate()
        .filter(|(index, message)| !protected[*index] && !message.tool_calls.is_empty())
        .map(|(index, _)| (index, token_counts[index]))
        .collect::<Vec<_>>();
    call_candidates.sort_unstable_by_key(|(_, tokens)| std::cmp::Reverse(*tokens));
    for (index, _) in call_candidates {
        if total <= max_tokens {
            break;
        }
        let mut changed = false;
        for call in &mut messages[index].tool_calls {
            if call.input != serde_json::json!({"_compacted": true}) {
                call.input = serde_json::json!({"_compacted": true});
                changed = true;
            }
        }
        if changed {
            truncated.insert(index);
            replace_message_tokens(messages, &mut token_counts, &mut total, index);
        }
    }

    let mut dropped_recent_messages = 0;
    let mut remove = vec![false; messages.len()];
    for group in removable_message_groups(messages, &protected) {
        if total <= max_tokens {
            break;
        }
        for index in group {
            if !remove[index] {
                remove[index] = true;
                total = total.saturating_sub(token_counts[index]);
                dropped_recent_messages += 1;
            }
        }
    }
    if dropped_recent_messages > 0 {
        let mut retained_messages = Vec::with_capacity(messages.len() - dropped_recent_messages);
        let mut retained_tokens = Vec::with_capacity(token_counts.len() - dropped_recent_messages);
        let mut retained_protected = Vec::with_capacity(protected.len() - dropped_recent_messages);
        let mut retained_current =
            Vec::with_capacity(current_flags.len() - dropped_recent_messages);
        for (index, message) in std::mem::take(messages).into_iter().enumerate() {
            if !remove[index] {
                retained_messages.push(message);
                retained_tokens.push(token_counts[index]);
                retained_protected.push(protected[index]);
                retained_current.push(current_flags[index]);
            }
        }
        *messages = retained_messages;
        token_counts = retained_tokens;
        protected = retained_protected;
        current_flags = retained_current;
    }

    let mut protected_call_candidates = messages
        .iter()
        .enumerate()
        .filter(|(index, message)| protected[*index] && !message.tool_calls.is_empty())
        .map(|(index, _)| (index, token_counts[index]))
        .collect::<Vec<_>>();
    protected_call_candidates.sort_unstable_by_key(|(_, tokens)| std::cmp::Reverse(*tokens));
    for (index, _) in protected_call_candidates {
        if total <= max_tokens {
            break;
        }
        let mut changed = false;
        for call in &mut messages[index].tool_calls {
            if call.input != serde_json::json!({"_compacted": true}) {
                call.input = serde_json::json!({"_compacted": true});
                changed = true;
            }
        }
        if changed {
            truncated.insert(index);
            replace_message_tokens(messages, &mut token_counts, &mut total, index);
        }
    }

    shrink_ranked_content(
        messages,
        &mut token_counts,
        &mut total,
        max_tokens,
        TOOL_OUTPUT_MARKER,
        |index, message| protected[index] && message.role == MessageRole::Tool,
        &mut truncated,
    );
    shrink_ranked_content(
        messages,
        &mut token_counts,
        &mut total,
        max_tokens,
        MESSAGE_CONTENT_MARKER,
        |index, _| current_flags[index],
        &mut truncated,
    );
    if total > max_tokens {
        for index in 0..messages.len() {
            if total <= max_tokens {
                break;
            }
            if !protected[index] {
                continue;
            }
            let reasoning_removed = messages[index].reasoning_content.take().is_some();
            let opaque_removed = !messages[index].responses_reasoning_items.is_empty();
            if reasoning_removed || opaque_removed {
                messages[index].responses_reasoning_items.clear();
                truncated.insert(index);
                replace_message_tokens(messages, &mut token_counts, &mut total, index);
            }
        }
    }
    FitOutcome {
        truncated_message_count: truncated.len(),
        dropped_recent_messages,
    }
}
