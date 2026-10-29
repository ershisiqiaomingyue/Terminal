use super::super::budget::{estimate_messages_tokens, estimate_text_tokens};
use super::consts::TOOL_OUTPUT_MARKER;
use super::text_trim::truncate_text_to_token_budget;
use crate::{AgentContextCheckpointEntry, AgentMessage, AgentToolCall, MessageRole};
use serde_json::Value;

fn compact_checkpoint_input(input: &Value) -> Value {
    let serialized = serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string());
    if estimate_text_tokens(&serialized) <= 192 {
        return input.clone();
    }
    serde_json::json!({"_compacted": true, "evidence": "[evidence excerpt omitted]"})
}

pub(crate) fn checkpoint_entries_to_messages(
    entries: &[AgentContextCheckpointEntry],
) -> Vec<AgentMessage> {
    let mut messages = Vec::new();
    for entry in entries {
        match entry {
            AgentContextCheckpointEntry::SystemConstraint {
                content,
                source_message_id,
            } => {
                let mut message = AgentMessage::system(content.clone());
                message.source_message_id = source_message_id.clone();
                messages.push(message);
            }
            AgentContextCheckpointEntry::UserConstraint {
                content,
                source_message_id,
            } => {
                let mut message = AgentMessage::user(content.clone());
                message.source_message_id = source_message_id.clone();
                messages.push(message);
            }
            AgentContextCheckpointEntry::AssistantConclusion {
                content,
                source_message_id,
            } => messages.push(AgentMessage {
                role: MessageRole::Assistant,
                content: content.clone(),
                reasoning_content: None,
                responses_reasoning_items: Vec::new(),
                anthropic_thinking_blocks: Vec::new(),
                anthropic_content_blocks: Vec::new(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                source_message_id: source_message_id.clone(),
            }),
            AgentContextCheckpointEntry::ToolEvidence {
                call_id,
                tool_name,
                input,
                output,
                source_message_id,
            } => {
                messages.push(AgentMessage::assistant_tool_call(AgentToolCall {
                    id: call_id.clone(),
                    name: tool_name.clone(),
                    input: compact_checkpoint_input(input),
                }));
                let mut result = AgentMessage::tool(
                    call_id,
                    truncate_text_to_token_budget(output, 512, TOOL_OUTPUT_MARKER),
                );
                result.source_message_id = source_message_id.clone();
                messages.push(result);
            }
        }
    }
    messages
}

/// Tokens one checkpoint entry contributes to the next request, measured the
/// way `checkpoint_entries_to_messages` will actually replay it.
pub(super) fn rebuilt_checkpoint_entry_tokens(entry: &AgentContextCheckpointEntry) -> usize {
    estimate_messages_tokens(&checkpoint_entries_to_messages(std::slice::from_ref(entry)))
}

/// The same entry with every replayed text field emptied, so measuring it gives
/// the irreducible cost of replaying this entry at all: per-message framing
/// (empty tool-call/reasoning/thinking lists still count) plus, for tool
/// evidence, the call id/name/input envelope.
fn checkpoint_entry_envelope(entry: &AgentContextCheckpointEntry) -> AgentContextCheckpointEntry {
    match entry {
        AgentContextCheckpointEntry::SystemConstraint {
            source_message_id, ..
        } => AgentContextCheckpointEntry::SystemConstraint {
            content: String::new(),
            source_message_id: source_message_id.clone(),
        },
        AgentContextCheckpointEntry::UserConstraint {
            source_message_id, ..
        } => AgentContextCheckpointEntry::UserConstraint {
            content: String::new(),
            source_message_id: source_message_id.clone(),
        },
        AgentContextCheckpointEntry::AssistantConclusion {
            source_message_id, ..
        } => AgentContextCheckpointEntry::AssistantConclusion {
            content: String::new(),
            source_message_id: source_message_id.clone(),
        },
        AgentContextCheckpointEntry::ToolEvidence {
            call_id,
            tool_name,
            input,
            source_message_id,
            ..
        } => AgentContextCheckpointEntry::ToolEvidence {
            call_id: call_id.clone(),
            tool_name: tool_name.clone(),
            input: input.clone(),
            output: String::new(),
            source_message_id: source_message_id.clone(),
        },
    }
}

/// The same entry with its text cut down until the rebuilt messages fit
/// `budget`.
///
/// `None` means the entry no longer fits — not even its framing, tool name and
/// call envelope — so an over-crowded section drops whole entries instead of
/// smuggling their full content past the checkpoint budget when the payload is
/// replayed on the next round.
pub(super) fn bound_checkpoint_entry(
    entry: &AgentContextCheckpointEntry,
    budget: usize,
) -> Option<AgentContextCheckpointEntry> {
    if budget == 0 {
        return None;
    }
    let envelope = rebuilt_checkpoint_entry_tokens(&checkpoint_entry_envelope(entry));
    if envelope >= budget {
        return None;
    }
    let text_budget = budget - envelope;
    let bounded = match entry {
        AgentContextCheckpointEntry::SystemConstraint {
            content,
            source_message_id,
        } => AgentContextCheckpointEntry::SystemConstraint {
            content: truncate_text_to_token_budget(content, text_budget, "…"),
            source_message_id: source_message_id.clone(),
        },
        AgentContextCheckpointEntry::UserConstraint {
            content,
            source_message_id,
        } => AgentContextCheckpointEntry::UserConstraint {
            content: truncate_text_to_token_budget(content, text_budget, "…"),
            source_message_id: source_message_id.clone(),
        },
        AgentContextCheckpointEntry::AssistantConclusion {
            content,
            source_message_id,
        } => AgentContextCheckpointEntry::AssistantConclusion {
            content: truncate_text_to_token_budget(content, text_budget, "…"),
            source_message_id: source_message_id.clone(),
        },
        AgentContextCheckpointEntry::ToolEvidence {
            call_id,
            tool_name,
            input,
            output,
            source_message_id,
        } => AgentContextCheckpointEntry::ToolEvidence {
            call_id: call_id.clone(),
            tool_name: tool_name.clone(),
            input: input.clone(),
            output: truncate_text_to_token_budget(output, text_budget, TOOL_OUTPUT_MARKER),
            source_message_id: source_message_id.clone(),
        },
    };
    (rebuilt_checkpoint_entry_tokens(&bounded) <= budget).then_some(bounded)
}
