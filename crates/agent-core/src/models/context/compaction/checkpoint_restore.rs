use super::checkpoint_messages::checkpoint_entries_to_messages;
use super::consts::{CONTEXT_CHECKPOINT_FORMAT_VERSION, LEGACY_CONTEXT_SUMMARY_PREFIX};
use crate::{AgentContextCheckpoint, AgentContextCheckpointEntry, AgentMessage, MessageRole};
use std::collections::HashSet;

pub fn restore_context_checkpoint(
    checkpoint: &AgentContextCheckpoint,
    _current_model_id: Option<&str>,
) -> Option<Vec<AgentMessage>> {
    let payload = checkpoint.payload.as_ref()?;
    if payload.format_version != CONTEXT_CHECKPOINT_FORMAT_VERSION {
        return None;
    }
    // Checkpoint entries contain portable text and paired tool observations,
    // never provider-private reasoning/signatures. The producer is provenance,
    // not a replay restriction. Native provider frames remain route-scoped.
    let mut call_ids = HashSet::new();
    for entry in &payload.entries {
        if let AgentContextCheckpointEntry::ToolEvidence {
            call_id, tool_name, ..
        } = entry
        {
            if call_id.trim().is_empty()
                || tool_name.trim().is_empty()
                || !call_ids.insert(call_id.as_str())
            {
                return None;
            }
        }
    }
    let messages = checkpoint_entries_to_messages(&payload.entries);
    (!messages.is_empty()).then_some(messages)
}

pub fn legacy_context_checkpoint_message(
    checkpoint: &AgentContextCheckpoint,
) -> Option<AgentMessage> {
    (!checkpoint.summary.trim().is_empty()).then(|| AgentMessage {
        role: MessageRole::Assistant,
        content: format!(
            "{LEGACY_CONTEXT_SUMMARY_PREFIX}{}",
            checkpoint.summary.trim()
        ),
        reasoning_content: None,
        responses_reasoning_items: Vec::new(),
        anthropic_thinking_blocks: Vec::new(),
        anthropic_content_blocks: Vec::new(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        source_message_id: checkpoint.covered_through_message_id.clone(),
    })
}
