use super::super::budget::estimate_text_tokens;
use super::checkpoint_messages::{bound_checkpoint_entry, rebuilt_checkpoint_entry_tokens};
use super::consts::{
    CONTEXT_CHECKPOINT_FORMAT_VERSION, CONTEXT_SUMMARY_PREFIX, TOOL_OUTPUT_MARKER,
};
use super::text_trim::{
    semantic_excerpt, structured_evidence_excerpt, truncate_text_to_token_budget,
};
use crate::{
    AgentContextCheckpointEntry, AgentContextCheckpointPayload, AgentMessage, MessageRole,
};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone)]
struct CheckpointEntry {
    index: usize,
    text: String,
    typed: AgentContextCheckpointEntry,
}

#[derive(Debug, Default)]
pub(crate) struct CheckpointDraft {
    pub(super) text: String,
    entries: Vec<AgentContextCheckpointEntry>,
    pub(super) system_constraint_count: usize,
    pub(super) pending_user_item_count: usize,
    pub(super) tool_evidence_count: usize,
    pub(super) historical_conclusion_count: usize,
    pub(super) truncated: bool,
}

impl CheckpointDraft {
    pub(super) fn payload(&self) -> Option<AgentContextCheckpointPayload> {
        (!self.entries.is_empty()).then(|| AgentContextCheckpointPayload {
            format_version: CONTEXT_CHECKPOINT_FORMAT_VERSION,
            producer_model_id: None,
            entries: self.entries.clone(),
        })
    }
}

fn render_checkpoint_section(
    title: &str,
    entries: &[CheckpointEntry],
    budget_tokens: usize,
    latest_first: bool,
) -> (String, Vec<AgentContextCheckpointEntry>, bool) {
    if entries.is_empty() || budget_tokens == 0 {
        return (String::new(), Vec::new(), false);
    }
    let header = format!("[{title}]\n");
    let header_tokens = estimate_text_tokens(&header);
    if header_tokens.saturating_add(4) > budget_tokens {
        return (String::new(), Vec::new(), true);
    }
    let mut ordered = entries.iter().collect::<Vec<_>>();
    if latest_first {
        ordered.sort_by_key(|entry| std::cmp::Reverse(entry.index));
    } else {
        ordered.sort_by_key(|entry| entry.index);
    }
    let mut selected = Vec::new();
    let mut remaining = budget_tokens.saturating_sub(header_tokens);
    // `remaining` only covers the rendered summary text. The typed entries are a
    // second and larger cost: `checkpoint_entries_to_messages` turns each one back
    // into a real message (plus tool framing) for the next request, so they get
    // their own pool rather than being charged the summary's header and bullet
    // overhead — otherwise a tight section would drop every entry it covered.
    let mut payload_remaining = budget_tokens;
    let mut truncated = false;
    for (position, entry) in ordered.iter().enumerate() {
        if remaining <= 4 || payload_remaining == 0 {
            truncated = true;
            break;
        }
        let remaining_entries = ordered.len().saturating_sub(position).max(1);
        let fair_share = (remaining / remaining_entries).clamp(12, 512);
        let entry_budget = remaining.saturating_sub(3).min(fair_share);
        if entry_budget == 0 {
            truncated = true;
            break;
        }
        let bounded = truncate_text_to_token_budget(&entry.text, entry_budget, "…");
        if bounded.is_empty() {
            truncated = true;
            continue;
        }
        truncated |= bounded != entry.text;
        // The rendered line is only this entry's place in the summary text; the
        // typed copy is what `checkpoint_entries_to_messages` replays into the
        // next request, so the typed copy is what has to fit the budget.
        // Storing the original entry here is what let a 32K checkpoint come
        // back as a million tokens and refill the whole window.
        let Some(bounded_entry) =
            bound_checkpoint_entry(&entry.typed, payload_remaining / remaining_entries)
        else {
            truncated = true;
            continue;
        };
        let payload_tokens = rebuilt_checkpoint_entry_tokens(&bounded_entry);
        if payload_tokens == 0 || payload_tokens > payload_remaining {
            truncated = true;
            continue;
        }
        payload_remaining = payload_remaining.saturating_sub(payload_tokens);
        let rendered = format!("- {bounded}\n");
        remaining = remaining.saturating_sub(estimate_text_tokens(&rendered));
        selected.push((entry.index, rendered, bounded_entry));
    }
    selected.sort_by_key(|(index, _, _)| *index);
    let mut output = header;
    let count = selected.len();
    let mut typed = Vec::with_capacity(count);
    for (_, rendered, entry) in selected {
        output.push_str(&rendered);
        typed.push(entry);
    }
    truncated |= count < entries.len();
    (output, typed, truncated)
}

fn compact_checkpoint_input(input: &Value) -> Value {
    let serialized = serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string());
    if estimate_text_tokens(&serialized) <= 192 {
        return input.clone();
    }
    serde_json::json!({
        "_compacted": true,
        "evidence": structured_evidence_excerpt(&serialized, 160),
    })
}

pub(crate) fn build_semantic_checkpoint(
    history: &[AgentMessage],
    max_tokens: usize,
) -> CheckpointDraft {
    if history.is_empty() || max_tokens == 0 {
        return CheckpointDraft::default();
    }
    let mut systems = Vec::new();
    let mut users = Vec::new();
    let mut tools = Vec::new();
    let mut assistants = Vec::new();
    let mut tool_sources = HashMap::<String, (String, Value)>::new();
    for message in history {
        for call in &message.tool_calls {
            tool_sources.insert(call.id.clone(), (call.name.clone(), call.input.clone()));
        }
    }
    for (index, message) in history.iter().enumerate() {
        let source = message
            .source_message_id
            .as_deref()
            .map(|id| format!("source={id}; "))
            .unwrap_or_default();
        match message.role {
            MessageRole::System => {
                let content = semantic_excerpt(&message.content, 768, true);
                systems.push(CheckpointEntry {
                    index,
                    text: format!("{source}{content}"),
                    typed: AgentContextCheckpointEntry::SystemConstraint {
                        content,
                        source_message_id: message.source_message_id.clone(),
                    },
                });
            }
            MessageRole::User => {
                let content = semantic_excerpt(&message.content, 768, true);
                users.push(CheckpointEntry {
                    index,
                    text: format!("{source}{content}"),
                    typed: AgentContextCheckpointEntry::UserConstraint {
                        content,
                        source_message_id: message.source_message_id.clone(),
                    },
                });
            }
            MessageRole::Tool => {
                let call_id = message
                    .tool_call_id
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("checkpoint-tool-{index}"));
                let (name, input) = tool_sources.get(&call_id).cloned().unwrap_or_else(|| {
                    (
                        "unknown_tool".to_string(),
                        Value::Object(Default::default()),
                    )
                });
                let input = compact_checkpoint_input(&input);
                let serialized_input =
                    serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
                let output =
                    truncate_text_to_token_budget(&message.content, 512, TOOL_OUTPUT_MARKER);
                tools.push(CheckpointEntry {
                    index,
                    text: format!(
                        "tool={name}; call={call_id}; {source}input={}; evidence={}",
                        structured_evidence_excerpt(&serialized_input, 192),
                        structured_evidence_excerpt(&output, 512)
                    ),
                    typed: AgentContextCheckpointEntry::ToolEvidence {
                        call_id,
                        tool_name: name,
                        input,
                        output,
                        source_message_id: message.source_message_id.clone(),
                    },
                });
            }
            MessageRole::Assistant if message.tool_calls.is_empty() => {
                let content = semantic_excerpt(&message.content, 512, false);
                assistants.push(CheckpointEntry {
                    index,
                    text: format!("{source}{content}"),
                    typed: AgentContextCheckpointEntry::AssistantConclusion {
                        content,
                        source_message_id: message.source_message_id.clone(),
                    },
                });
            }
            MessageRole::Assistant => {}
        }
    }

    let prefix_tokens = estimate_text_tokens(CONTEXT_SUMMARY_PREFIX);
    if prefix_tokens >= max_tokens {
        return CheckpointDraft {
            text: truncate_text_to_token_budget(CONTEXT_SUMMARY_PREFIX, max_tokens, "…"),
            truncated: true,
            ..CheckpointDraft::default()
        };
    }
    let remaining = max_tokens - prefix_tokens;
    let sections = [
        ("系统约束", &systems, 30usize, false),
        ("用户未完成事项与约束", &users, 35usize, true),
        ("工具证据来源", &tools, 25usize, true),
        ("历史助手结论（非工具证据）", &assistants, 10usize, true),
    ];
    let active_weight = sections
        .iter()
        .filter(|(_, entries, _, _)| !entries.is_empty())
        .map(|(_, _, weight, _)| *weight)
        .sum::<usize>()
        .max(1);
    let mut draft = CheckpointDraft {
        text: CONTEXT_SUMMARY_PREFIX.to_string(),
        ..CheckpointDraft::default()
    };
    for (section_index, (title, entries, weight, latest_first)) in sections.iter().enumerate() {
        if entries.is_empty() {
            continue;
        }
        let already_used = estimate_text_tokens(&draft.text).saturating_sub(prefix_tokens);
        let available = remaining.saturating_sub(already_used);
        let share = if section_index == sections.len() - 1 {
            available
        } else {
            remaining.saturating_mul(*weight) / active_weight
        }
        .min(available);
        let (rendered, selected, truncated) =
            render_checkpoint_section(title, entries, share, *latest_first);
        draft.text.push_str(&rendered);
        draft.truncated |= truncated;
        let count = selected.len();
        draft.entries.extend(selected);
        match *title {
            "系统约束" => draft.system_constraint_count = count,
            "用户未完成事项与约束" => draft.pending_user_item_count = count,
            "工具证据来源" => draft.tool_evidence_count = count,
            _ => draft.historical_conclusion_count = count,
        }
    }
    let bounded = truncate_text_to_token_budget(&draft.text, max_tokens, "…");
    draft.truncated |= bounded != draft.text;
    draft.text = bounded;
    draft
}
