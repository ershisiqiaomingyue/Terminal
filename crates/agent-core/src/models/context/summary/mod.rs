use super::budget::{
    estimate_message_tokens, estimate_request_tokens, estimate_text_tokens, CompactionResult,
};
use super::checkpoint::model_summary_input;
use crate::{AgentMessage, MessageRole};
use serde_json::Value;
use std::collections::HashSet;

use super::compaction::consts::SUMMARY_INSTRUCTION;
pub const CONTEXT_SUMMARY_INPUT_LIMIT: usize = 8_192;

/// A bounded copy of a routed request prefix, never a cached model summary.
/// Only adapters whose leading wire messages are independent of the tail are
/// eligible. Anthropic's moving cache breakpoints require a separate contract.
#[derive(Debug, Clone)]
pub struct ContextSummaryPrefix {
    pub(super) provider_id: String,
    pub(super) provider_kind: crate::ProviderKind,
    pub(super) provider_endpoint: String,
    pub(super) request: crate::ProviderRequest,
    pub(super) wire_messages: Vec<Value>,
}

#[derive(Debug, Default)]
pub struct ContextSummaryPrefixCache {
    pub(super) entries: Vec<(String, std::time::Instant, ContextSummaryPrefix)>,
}

impl ContextSummaryPrefixCache {
    pub fn get(&self, session_id: &str) -> Option<&ContextSummaryPrefix> {
        self.entries
            .iter()
            .find(|(session, recorded, _)| {
                session == session_id && recorded.elapsed().as_secs() < 1_200
            })
            .map(|(_, _, prefix)| prefix)
    }

    pub fn commit(
        &mut self,
        session_id: &str,
        prefix: Option<ContextSummaryPrefix>,
        provider_events_received: bool,
    ) {
        if !provider_events_received {
            return;
        }
        self.entries.retain(|(session, recorded, _)| {
            session != session_id && recorded.elapsed().as_secs() < 1_200
        });
        if let Some(prefix) = prefix {
            if self.entries.len() >= 16 {
                self.entries.remove(0);
            }
            self.entries
                .push((session_id.to_string(), std::time::Instant::now(), prefix));
        }
    }
}

fn summary_prefix_end(messages: &[AgentMessage], max_tokens: usize) -> usize {
    let mut end = 0;
    let mut tokens = 0usize;
    let mut pending = HashSet::new();
    for (index, message) in messages.iter().enumerate() {
        if message.reasoning_content.is_some()
            || !message.responses_reasoning_items.is_empty()
            || !message.anthropic_thinking_blocks.is_empty()
            || !message.anthropic_content_blocks.is_empty()
        {
            break;
        }
        tokens = tokens.saturating_add(estimate_message_tokens(message));
        if tokens > max_tokens {
            break;
        }
        if !message.tool_calls.is_empty() {
            if !pending.is_empty() || message.role != MessageRole::Assistant {
                break;
            }
            for call in &message.tool_calls {
                if !pending.insert(call.id.as_str()) {
                    return end;
                }
            }
        } else if message.role == MessageRole::Tool {
            if !message
                .tool_call_id
                .as_deref()
                .is_some_and(|id| pending.remove(id))
            {
                break;
            }
        } else if !pending.is_empty() {
            break;
        }
        if pending.is_empty() {
            end = index + 1;
        }
    }
    end
}

fn summary_wire_messages(
    config: &crate::ProviderConfig,
    request: &crate::ProviderRequest,
) -> Option<Vec<Value>> {
    let body = crate::request_body(config, request).ok()?;
    body.get(if config.kind == crate::ProviderKind::OpenAiResponses {
        "input"
    } else {
        "messages"
    })?
    .as_array()
    .cloned()
}

impl ContextSummaryPrefix {
    pub fn capture(
        config: &crate::ProviderConfig,
        request: &crate::ProviderRequest,
    ) -> Option<Self> {
        if config.kind == crate::ProviderKind::Anthropic || !request.image_attachments.is_empty() {
            return None;
        }
        let overhead = estimate_request_tokens(
            &[],
            request.system_prompt.as_deref().unwrap_or_default(),
            &request.tools,
        );
        let budget = CONTEXT_SUMMARY_INPUT_LIMIT
            .checked_sub(overhead)?
            .checked_sub(256)?;
        let end = summary_prefix_end(&request.messages, budget);
        if end == 0 {
            return None;
        }
        // Do not clone a potentially 1M-token transcript to retain an 8K prefix.
        let retained = crate::ProviderRequest {
            prompt_cache_key: request.prompt_cache_key.clone(),
            prompt_cache_retention: request.prompt_cache_retention.clone(),
            session_id: request.session_id.clone(),
            model: request.model.clone(),
            messages: request.messages[..end].to_vec(),
            system_prompt: request.system_prompt.clone(),
            tools: request.tools.clone(),
            max_tokens: request.max_tokens,
            image_attachments: Vec::new(),
            include_usage: request.include_usage,
            include_reasoning_encrypted_content: request.include_reasoning_encrypted_content,
            reasoning_effort: request.reasoning_effort.clone(),
            native_reasoning_effort: request.native_reasoning_effort,
        };
        let wire_messages = summary_wire_messages(config, &retained)?;
        Some(Self {
            provider_id: config.id.clone(),
            provider_kind: config.kind,
            provider_endpoint: config.base_url.clone(),
            request: retained,
            wire_messages,
        })
    }

    pub fn summary_request(
        &self,
        config: &crate::ProviderConfig,
        profile: &crate::ProviderRequest,
        source: &[AgentMessage],
        compaction: &CompactionResult,
        max_input_tokens: usize,
        max_output_tokens: u32,
    ) -> Option<crate::ProviderRequest> {
        if self.provider_id != config.id
            || self.provider_kind != config.kind
            || self.provider_endpoint != config.base_url
            || self.request.model != profile.model
            || self.request.session_id != profile.session_id
            || self.request.system_prompt != profile.system_prompt
            || serde_json::to_value(&self.request.tools).ok()?
                != serde_json::to_value(&profile.tools).ok()?
            || self.request.reasoning_effort != profile.reasoning_effort
            || self.request.include_reasoning_encrypted_content
                != profile.include_reasoning_encrypted_content
            || self.request.include_usage != profile.include_usage
            || !profile.image_attachments.is_empty()
        {
            return None;
        }
        let max_input_tokens = max_input_tokens.min(CONTEXT_SUMMARY_INPUT_LIMIT);
        let covered = compaction.covered_message_count?.min(source.len());
        let available_prefix = self.request.messages.len().min(covered);
        let mut matching_prefix = 0;
        for (old, current) in self.request.messages[..available_prefix]
            .iter()
            .zip(&source[..available_prefix])
        {
            // Local message provenance does not participate in provider input.
            if serde_json::to_vec(old).ok()? != serde_json::to_vec(current).ok()? {
                break;
            }
            matching_prefix += 1;
        }
        let overhead = estimate_request_tokens(
            &[],
            profile.system_prompt.as_deref().unwrap_or_default(),
            &profile.tools,
        )
        .saturating_add(estimate_text_tokens(SUMMARY_INSTRUCTION))
        .saturating_add(64);
        let remaining = max_input_tokens.checked_sub(overhead)?;
        let end = summary_prefix_end(&self.request.messages[..matching_prefix], remaining / 2);
        if end == 0 {
            return None;
        }
        let mut request = self.request.clone();
        request.messages.truncate(end);
        request.max_tokens = max_output_tokens;
        let replay_wire = summary_wire_messages(config, &request)?;
        if !self.wire_messages.starts_with(&replay_wire) {
            return None;
        }
        let payload_budget = max_input_tokens
            .saturating_sub(estimate_request_tokens(
                &request.messages,
                request.system_prompt.as_deref().unwrap_or_default(),
                &request.tools,
            ))
            .saturating_sub(estimate_text_tokens(SUMMARY_INSTRUCTION))
            .saturating_sub(32);
        let input = model_summary_input(source, compaction, payload_budget)?;
        request.messages.push(AgentMessage::user(format!(
            "{SUMMARY_INSTRUCTION}\n\n{input}"
        )));
        let summary_wire = summary_wire_messages(config, &request)?;
        if !summary_wire.starts_with(&replay_wire)
            || estimate_request_tokens(
                &request.messages,
                request.system_prompt.as_deref().unwrap_or_default(),
                &request.tools,
            ) > max_input_tokens
        {
            return None;
        }
        Some(request)
    }
}
