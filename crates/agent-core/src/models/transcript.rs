use crate::{AgentMessage, MessageRole, ProviderConfig, ProviderImageAttachment, ProviderRequest};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

/// Private model-facing history, independent of the display projection.
/// Store only bounded, completed tool groups; never reconstruct signed frames
/// from user-visible reasoning or assistant statements.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTranscript {
    pub version: u8,
    pub scope: String,
    pub turn_id: String,
    pub runtime_context: String,
    pub messages: Vec<AgentMessage>,
    pub images: Vec<ProviderImageAttachment>,
    pub last_request: Option<PromptCacheFingerprint>,
}

impl ModelTranscript {
    pub fn is_replayable(&self, scope: &str) -> bool {
        if self.version != 1 || self.scope != scope || self.messages.is_empty() {
            return false;
        }
        let mut pending = HashSet::new();
        let mut seen = HashSet::new();
        for message in &self.messages {
            if message.role == MessageRole::Tool {
                let Some(id) = message.tool_call_id.as_deref() else {
                    return false;
                };
                if !pending.remove(id) {
                    return false;
                }
            } else {
                if !pending.is_empty() {
                    return false;
                }
                for call in &message.tool_calls {
                    if call.id.is_empty() || !seen.insert(call.id.as_str()) {
                        return false;
                    }
                    pending.insert(call.id.as_str());
                }
            }
        }
        pending.is_empty()
            && self.images.iter().all(|image| {
                self.messages
                    .iter()
                    .any(|message| crate::message_owns_image(message, &image.source_message_id))
            })
    }
}

pub fn prompt_cache_hash(value: &impl Serialize) -> String {
    let bytes = serde_json::to_vec(value).expect("serializable cache identity");
    format!("{:x}", Sha256::digest(bytes))
}

/// A managed plugin keeps its identity while its loopback port changes.
/// Custom/external endpoints retain their exact route identity.
pub fn transcript_endpoint_identity(provider_id: &str, endpoint: &str) -> String {
    if provider_id.starts_with("plugin:") {
        if let Ok(mut url) = url::Url::parse(endpoint) {
            if matches!(url.scheme(), "http" | "https")
                && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
            {
                let _ = url.set_port(None);
                return url.to_string();
            }
        }
    }
    endpoint.to_string()
}

/// Recover recorded tool observations when a native provider snapshot is
/// unavailable. No execution or provider-private reasoning is reconstructed.
pub fn append_historical_tool_results(
    messages: &mut Vec<AgentMessage>,
    events: &[Value],
    turn: &str,
) {
    let mut seen = messages
        .iter()
        .flat_map(|message| message.tool_calls.iter().map(|call| call.id.clone()))
        .collect::<HashSet<_>>();
    for event in events {
        if event["type"] != "tool_call_finished"
            || event["turn_id"].as_str() != Some(turn)
            || event["is_error"].as_bool() != Some(false)
            || event
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|s| !matches!(s, "done" | "completed"))
        {
            continue;
        }
        let (Some(id), Some(name), Some(input), Some(result)) = (
            event["call_id"].as_str().filter(|s| !s.is_empty()),
            event["tool_name"].as_str().filter(|s| !s.is_empty()),
            event.get("input").filter(|v| v.is_object()),
            event.get("output").filter(|v| !v.is_null()),
        ) else {
            continue;
        };
        if result["skipped"] == true || result.get("error").is_some_and(|v| !v.is_null()) {
            continue;
        }
        // Bound IDs for gateways with a 64-character call_id limit, while
        // keeping calls from different historical turns distinct.
        let replay_id = format!("history-{}", &prompt_cache_hash(&json!([turn, id]))[..32]);
        if seen.contains(id) || !seen.insert(replay_id.clone()) {
            continue;
        }
        messages.push(AgentMessage::assistant_tool_call(crate::AgentToolCall {
            id: replay_id.clone(),
            name: name.to_string(),
            input: input.clone(),
        }));
        let content = result
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| result.to_string());
        messages.push(AgentMessage::tool(
            replay_id,
            crate::prune_tool_result_for_model(&content),
        ));
    }
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;

    #[test]
    fn recovered_results_are_paired_deduplicated_and_work_across_protocols() {
        let good = json!({"type":"tool_call_finished", "turn_id":"old", "call_id":"read", "tool_name":"read_local_file", "input":{"path":"a.java"}, "output":{"content":"original bytes"}, "is_error":false});
        let mut failed = good.clone();
        failed["call_id"] = json!("bad");
        failed["is_error"] = json!(true);
        let mut skipped = good.clone();
        skipped["call_id"] = json!("skip");
        skipped["output"] = json!({"skipped":true});
        let mut current = good.clone();
        current["turn_id"] = json!("current");
        let events = vec![good.clone(), good, failed, skipped, current];
        let mut messages = vec![AgentMessage::user("old request")];
        append_historical_tool_results(&mut messages, &events, "old");
        append_historical_tool_results(&mut messages, &events, "old");
        assert_eq!(messages.len(), 3);
        let call = &messages[1].tool_calls[0];
        assert_eq!(messages[2].tool_call_id.as_deref(), Some(call.id.as_str()));
        assert!(call.id.len() < 64);
        assert!(messages[2].content.contains("original bytes"));
        let request = ProviderRequest::new("s", "model", messages, vec![]);
        for protocol in [
            crate::ProviderKind::OpenAiCompatible,
            crate::ProviderKind::OpenAiResponses,
            crate::ProviderKind::Anthropic,
        ] {
            let config =
                ProviderConfig::new("p", "p", protocol, "https://example.com/v1", "key").unwrap();
            let body = crate::request_body(&config, &request).unwrap().to_string();
            assert!(body.contains("original bytes"), "{protocol:?}");
            assert!(body.contains("history-"), "{protocol:?}");
        }
        // A checkpoint that already includes a call keeps the original ID.
        let mut retained = vec![
            AgentMessage::assistant_tool_call(crate::AgentToolCall {
                id: "read".into(),
                name: "read_local_file".into(),
                input: json!({"path":"a.java"}),
            }),
            AgentMessage::tool("read", "original bytes"),
        ];
        append_historical_tool_results(&mut retained, &events, "old");
        assert_eq!(retained.len(), 2);
    }

    #[test]
    fn managed_loopback_port_is_not_a_new_transcript_route() {
        for host in ["127.0.0.1", "localhost", "[::1]"] {
            assert_eq!(
                transcript_endpoint_identity(
                    "plugin:sample:account",
                    &format!("http://{host}:41001/cn/v1")
                ),
                transcript_endpoint_identity(
                    "plugin:sample:account",
                    &format!("http://{host}:51002/cn/v1")
                ),
            );
        }
        assert_ne!(
            transcript_endpoint_identity("plugin:p", "http://127.0.0.1:42/cn/v1"),
            transcript_endpoint_identity("plugin:p", "http://127.0.0.1:42/global/v1")
        );
        assert_ne!(
            transcript_endpoint_identity("custom", "http://localhost:42/v1"),
            transcript_endpoint_identity("custom", "http://localhost:43/v1")
        );
        assert_ne!(
            transcript_endpoint_identity("plugin:p", "https://relay.example:42/v1"),
            transcript_endpoint_identity("plugin:p", "https://relay.example:43/v1")
        );
    }
}

/// Hash transport prefixes without storing prompt text, images, or credentials.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptCacheFingerprint {
    pub route: String,
    pub settings: String,
    pub system: String,
    pub tools: String,
    pub messages: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptCacheDiagnostic {
    pub first_changed_section: String,
    pub matching_messages: usize,
    pub previous_messages: usize,
    pub current_messages: usize,
}

impl PromptCacheFingerprint {
    pub fn capture(config: &ProviderConfig, request: &ProviderRequest) -> Result<Self, String> {
        let mut body = crate::request_body(config, request)?;
        // Only strip transport hints on blocks. Tool inputs may legitimately
        // contain a field with the same name, which must remain part of the hash.
        fn strip_blocks(value: &mut Value) {
            if let Some(blocks) = value.as_array_mut() {
                for block in blocks {
                    if let Some(object) = block.as_object_mut() {
                        object.remove("cache_control");
                    }
                }
            }
        }
        strip_blocks(&mut body["system"]);
        strip_blocks(&mut body["tools"]);
        let source = if body["messages"].is_array() {
            "messages"
        } else {
            "input"
        };
        let mut messages = body[source].as_array().cloned().unwrap_or_default();
        for message in &mut messages {
            if let Some(text) = message["content"].as_str() {
                message["content"] = json!([{"type":"text", "text":text}]);
            }
            strip_blocks(&mut message["content"]);
        }
        Ok(Self {
            route: prompt_cache_hash(&(&config.id, &config.base_url, config.kind, &request.model)),
            settings: prompt_cache_hash(&json!({
                "thinking":body["thinking"], "output_config":body["output_config"],
                "prompt_cache_key":body["prompt_cache_key"],
                "reasoning":body["reasoning"], "reasoning_effort":body["reasoning_effort"],
                "tool_choice":body["tool_choice"], "response_format":body["response_format"],
                "text":body["text"], "parallel_tool_calls":body["parallel_tool_calls"],
            })),
            system: prompt_cache_hash(
                &json!({"system":body["system"], "instructions":body["instructions"],
                "leading":messages.iter().take_while(|m|matches!(m["role"].as_str(),Some("system" | "developer"))).collect::<Vec<_>>()}),
            ),
            tools: prompt_cache_hash(&body["tools"]),
            messages: messages.iter().map(prompt_cache_hash).collect(),
        })
    }

    pub fn compare(&self, previous: Option<&Self>) -> PromptCacheDiagnostic {
        let matching = previous
            .map(|p| {
                self.messages
                    .iter()
                    .zip(&p.messages)
                    .take_while(|(a, b)| a == b)
                    .count()
            })
            .unwrap_or(0);
        let section = match previous {
            None => "first_request",
            Some(p) if p.route != self.route => "route",
            Some(p) if p.settings != self.settings => "settings",
            Some(p) if p.tools != self.tools => "tools",
            Some(p) if p.system != self.system => "system",
            Some(p) if matching < p.messages.len() => "history",
            Some(_) => "append_only",
        };
        PromptCacheDiagnostic {
            first_changed_section: section.into(),
            matching_messages: matching,
            previous_messages: previous.map(|p| p.messages.len()).unwrap_or(0),
            current_messages: self.messages.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentToolCall, ProviderKind, ToolDefinition};

    #[test]
    fn replay_rejects_missing_results_wrong_scope_and_orphan_images() {
        let call = AgentToolCall {
            id: "call".into(),
            name: "read".into(),
            input: json!({}),
        };
        let mut transcript = ModelTranscript {
            version: 1,
            scope: "route".into(),
            turn_id: "t".into(),
            runtime_context: "config".into(),
            images: vec![],
            last_request: None,
            messages: vec![
                AgentMessage::user("read"),
                AgentMessage::assistant_tool_call(call),
            ],
        };
        assert!(!transcript.is_replayable("route"));
        transcript
            .messages
            .push(AgentMessage::tool("call", "original result"));
        assert!(transcript.is_replayable("route"));
        assert!(!transcript.is_replayable("other-host"));
        transcript.images.push(ProviderImageAttachment {
            name: "a".into(),
            media_type: "image/png".into(),
            data_url: "data:image/png;base64,AA==".into(),
            source_message_id: "missing".into(),
        });
        assert!(!transcript.is_replayable("route"));
    }

    #[test]
    fn diagnostics_distinguish_append_history_system_tools_and_effort() {
        let config = ProviderConfig::new(
            "p",
            "p",
            ProviderKind::Anthropic,
            "https://example.com",
            "key",
        )
        .unwrap();
        let mut request = ProviderRequest::new(
            "s",
            "claude-sonnet-4-5",
            vec![AgentMessage::user("first")],
            vec![ToolDefinition::new(
                "read",
                "read",
                json!({"type":"object"}),
            )],
        )
        .with_system_prompt("stable");
        let first = PromptCacheFingerprint::capture(&config, &request).unwrap();
        request.messages.push(AgentMessage::user("second"));
        let appended = PromptCacheFingerprint::capture(&config, &request).unwrap();
        assert_eq!(
            appended.compare(Some(&first)).first_changed_section,
            "append_only"
        );
        request.messages[0].content = "rewritten".into();
        assert_eq!(
            PromptCacheFingerprint::capture(&config, &request)
                .unwrap()
                .compare(Some(&first))
                .first_changed_section,
            "history"
        );
        request.messages[0].content = "first".into();
        request.system_prompt = Some("changed".into());
        assert_eq!(
            PromptCacheFingerprint::capture(&config, &request)
                .unwrap()
                .compare(Some(&first))
                .first_changed_section,
            "system"
        );
        request.system_prompt = Some("stable".into());
        request.tools[0].description = "changed".into();
        assert_eq!(
            PromptCacheFingerprint::capture(&config, &request)
                .unwrap()
                .compare(Some(&first))
                .first_changed_section,
            "tools"
        );
        request.reasoning_effort = Some("high".into());
        assert_eq!(
            PromptCacheFingerprint::capture(&config, &request)
                .unwrap()
                .compare(Some(&first))
                .first_changed_section,
            "settings"
        );
    }
}
