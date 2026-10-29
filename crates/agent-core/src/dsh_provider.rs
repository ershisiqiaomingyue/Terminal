use crate::{
    AgentEvent, AgentMessage, AgentRoundState, AgentToolCall, InputUsageSemantics, MessageRole,
    ProviderCapabilities, ProviderKind, ProviderRequest, ToolDefinition,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Hash each wire prefix once. Cache markers are transport hints, not signed content.
fn anthropic_prefix_hashes(body: &Value) -> Result<Vec<String>, String> {
    fn without_cache_marker(value: &Value) -> Value {
        let mut value = value.clone();
        if let Some(object) = value.as_object_mut() {
            object.remove("cache_control");
        }
        value
    }
    fn append(hash: &mut Sha256, value: &Value) -> Result<(), String> {
        let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
        Ok(())
    }
    let mut hash = Sha256::new();
    for field in ["system", "tools"] {
        let value = match body[field].as_array() {
            Some(blocks) => Value::Array(blocks.iter().map(without_cache_marker).collect()),
            None => body[field].clone(),
        };
        append(&mut hash, &value)?;
    }
    let mut prefixes = vec![format!("{:x}", hash.clone().finalize())];
    for source in body["messages"]
        .as_array()
        .ok_or("Claude wire messages missing")?
    {
        let mut message = source.clone();
        if let Some(text) = message["content"].as_str() {
            message["content"] = json!([{"type":"text","text":text}]);
        } else if let Some(content) = message["content"].as_array() {
            message["content"] = Value::Array(content.iter().map(without_cache_marker).collect());
        }
        append(&mut hash, &message)?;
        prefixes.push(format!("{:x}", hash.clone().finalize()));
    }
    Ok(prefixes)
}

pub fn dsh_protocol(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::OpenAiResponses => "openai_responses",
        _ => "openai_chat",
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshModelRequest {
    pub session_id: String,
    pub model: String,
    pub protocol: String,
    pub route_id: Option<String>,
    pub messages: Vec<Value>,
    #[serde(default)]
    pub tools: Vec<DshToolSchema>,
    pub max_tokens: Option<u32>,
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub compaction_revision: u64,
    pub purpose: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DshToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str, String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("DSH message missing {name}"))
}

impl DshModelRequest {
    /// Client summaries change the signed prefix. Keep original logs intact and
    /// remove only affected thinking in the explicitly compacted wire projection.
    pub fn migrate_compacted_thinking(&mut self) -> Result<Vec<String>, String> {
        if self.compaction_revision == 0 || self.protocol != "anthropic" {
            return Ok(Vec::new());
        }
        let latest = self
            .messages
            .iter()
            .rposition(|message| message["role"] == "assistant");
        let mut migrated = Vec::new();
        for (index, source) in self.messages.iter_mut().enumerate() {
            let replay = &source["source"]["replayState"]["response"];
            if source["role"] != "assistant"
                || replay["protocol"] != "anthropic"
                || replay["model"].as_str() != Some(self.model.as_str())
                || replay["routeId"].as_str() != self.route_id.as_deref()
                || replay["compactionRevision"].as_u64().unwrap_or(0) >= self.compaction_revision
            {
                continue;
            }
            let has_thinking = ["anthropicContentBlocks", "anthropicThinkingBlocks"]
                .iter()
                .any(|key| {
                    replay[*key].as_array().is_some_and(|blocks| {
                        blocks.iter().any(|block| {
                            matches!(
                                block["type"].as_str(),
                                Some("thinking" | "redacted_thinking")
                            )
                        })
                    })
                });
            if !has_thinking {
                continue;
            }
            if self.purpose.as_deref() != Some("compaction")
                && latest == Some(index)
                && source["content"]
                    .as_array()
                    .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool-call"))
            {
                return Err("不能在 Claude 工具思考尚待继续时迁移其签名块".into());
            }
            let id = source["id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("assistant-{index}"));
            let replay = &mut source["source"]["replayState"]["response"];
            if let Some(blocks) = replay["anthropicContentBlocks"].as_array_mut() {
                blocks.retain(|block| {
                    !matches!(
                        block["type"].as_str(),
                        Some("thinking" | "redacted_thinking")
                    )
                });
            }
            replay["anthropicThinkingBlocks"] = json!([]);
            migrated.push(id);
        }
        Ok(migrated)
    }

    pub fn validate_anthropic_prefix(
        &self,
        config: &crate::ProviderConfig,
        native: &ProviderRequest,
    ) -> Result<String, String> {
        let body = crate::request_body(config, native)?;
        let hashes = anthropic_prefix_hashes(&body)?;
        let assistants = self
            .messages
            .iter()
            .filter(|message| message["role"] == "assistant")
            .collect::<Vec<_>>();
        let mut assistant_index = 0;
        for (index, message) in body["messages"]
            .as_array()
            .ok_or("Claude wire messages missing")?
            .iter()
            .enumerate()
        {
            if message["role"] != "assistant" {
                continue;
            }
            let source = assistants
                .get(assistant_index)
                .ok_or("Claude assistant projection mismatch")?;
            assistant_index += 1;
            if !message["content"].as_array().is_some_and(|blocks| {
                blocks.iter().any(|block| {
                    matches!(
                        block["type"].as_str(),
                        Some("thinking" | "redacted_thinking")
                    )
                })
            }) {
                continue;
            }
            let replay = &source["source"]["replayState"]["response"];
            if replay["protocol"] != "anthropic"
                || replay["model"].as_str() != Some(self.model.as_str())
                || replay["routeId"].as_str() != self.route_id.as_deref()
            {
                continue;
            }
            if let Some(expected) = replay["anthropicPrefixHash"].as_str() {
                if expected != hashes[index] {
                    return Err(
                        "Claude 的签名前文已变化，不能直接回传旧思考块；需要进行上下文迁移后继续。"
                            .into(),
                    );
                }
            }
        }
        if assistant_index != assistants.len() {
            return Err("Claude assistant projection mismatch".into());
        }
        hashes
            .last()
            .cloned()
            .ok_or("Claude prefix hash missing".into())
    }

    /// The native host validates the captured model route before selecting credentials.
    /// Unknown content fails closed until the attachment adapter is mounted.
    pub fn provider_request(
        &self,
        kind: ProviderKind,
        expected_model: &str,
    ) -> Result<ProviderRequest, String> {
        self.provider_request_with_vision(kind, expected_model, None)
    }

    pub fn provider_request_with_vision(
        &self,
        kind: ProviderKind,
        expected_model: &str,
        supports_vision: Option<bool>,
    ) -> Result<ProviderRequest, String> {
        if self.protocol != dsh_protocol(kind) || self.model != expected_model {
            return Err("DSH request does not match the captured native model route".into());
        }
        let mut messages = Vec::with_capacity(self.messages.len());
        let mut images = Vec::new();
        for source in &self.messages {
            let role = match field(source, "role")? {
                "system" => MessageRole::System,
                "user" => MessageRole::User,
                "assistant" => MessageRole::Assistant,
                "tool" => MessageRole::Tool,
                _ => return Err("DSH message role requires an explicit native adapter".into()),
            };
            let mut message = AgentMessage::user("");
            message.role = role;
            message.source_message_id = source.get("id").and_then(Value::as_str).map(str::to_owned);
            for block in source
                .get("content")
                .and_then(Value::as_array)
                .ok_or("DSH content must be blocks")?
            {
                match field(block, "type")? {
                    "text" => message.content.push_str(field(block, "text")?),
                    "reasoning" if message.role == MessageRole::Assistant => {
                        message.reasoning_content.get_or_insert_with(String::new).push_str(field(block, "text")?);
                    }
                    "tool-call" if message.role == MessageRole::Assistant => {
                        message.tool_calls.push(AgentToolCall {
                            id: field(block, "id")?.into(), name: field(block, "name")?.into(),
                            input: serde_json::from_str(field(block, "arguments")?).map_err(|_| "invalid DSH tool arguments")?,
                        });
                    }
                    "image" if message.role == MessageRole::User => {
                        let owner = message.source_message_id.as_deref().ok_or("image owner id missing")?;
                        let attachment = crate::AgentImageAttachment {
                            path: None,
                            name: block["attachment"]["name"].as_str().unwrap_or("image").into(),
                            media_type: field(&block["attachment"], "mediaType")?.into(),
                            data_url: field(block, "dataUrl")?.into(),
                        };
                        if supports_vision == Some(false) {
                            return Err("Text-only route received an unprojected DSH image".into());
                        }
                        images.extend(crate::normalize_image_attachments(&[attachment], None, owner)?);
                    }
                    _ => return Err("DSH content block requires an explicit native adapter; nothing was dropped".into()),
                }
            }
            if message.role == MessageRole::Tool {
                message.tool_call_id = Some(field(source, "toolCallId")?.into());
            }
            if message.role == MessageRole::User
                && source["source"]["nativeAttachmentsVersion"] != 1
            {
                if let Some(attachments) =
                    source["source"]["nativeRequest"]["attachments"].as_array()
                {
                    let attachments: Vec<crate::AgentImageAttachment> =
                        serde_json::from_value(json!(attachments))
                            .map_err(|_| "invalid durable native image attachments")?;
                    let owner = message
                        .source_message_id
                        .as_deref()
                        .ok_or("image owner id missing")?;
                    let normalized = crate::normalize_image_attachments(&attachments, None, owner)?;
                    if supports_vision == Some(false) {
                        for image in normalized {
                            message.content.push_str(&format!(
                                "\n[历史图片：{}；当前模型不支持查看图片]",
                                image.name
                            ));
                        }
                    } else {
                        images.extend(normalized);
                    }
                }
            }
            let replay = &source["source"]["replayState"]["response"];
            if message.role == MessageRole::Assistant
                && replay["protocol"].as_str() == Some(dsh_protocol(kind))
                && replay["model"].as_str() == Some(expected_model)
                && replay["routeId"].as_str() == self.route_id.as_deref()
            {
                if kind == ProviderKind::Anthropic {
                    message.anthropic_content_blocks = replay["anthropicContentBlocks"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    message.anthropic_thinking_blocks = replay["anthropicThinkingBlocks"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                } else if kind == ProviderKind::OpenAiResponses {
                    message.responses_reasoning_items = replay["responsesReasoningItems"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                }
            }
            messages.push(message);
        }
        let tools = self
            .tools
            .iter()
            .map(|tool| ToolDefinition::new(&tool.name, &tool.description, tool.parameters.clone()))
            .collect();
        Ok(
            ProviderRequest::new(&self.session_id, &self.model, messages, tools)
                .with_image_attachments(images)
                .with_max_tokens(
                    self.max_tokens
                        .filter(|value| *value > 0)
                        .ok_or("DSH output limit is unresolved")?,
                )
                .with_reasoning_effort(self.reasoning_effort.clone()),
        )
    }
}

/// Translate the host's already-normalized provider events to DSH blocks.
/// Permission checks and execution never happen in this transport adapter.
pub struct DshChunkEncoder {
    kind: ProviderKind,
    model: String,
    round: AgentRoundState,
    text_index: Option<usize>,
    reasoning_index: Option<usize>,
    next_index: usize,
    finished: bool,
    route_id: Option<String>,
}

impl DshChunkEncoder {
    pub fn new(kind: ProviderKind, model: impl Into<String>) -> Self {
        Self {
            kind,
            model: model.into(),
            round: AgentRoundState::default(),
            text_index: None,
            reasoning_index: None,
            next_index: 0,
            finished: false,
            route_id: None,
        }
    }

    pub fn with_route_id(mut self, route_id: Option<String>) -> Self {
        self.route_id = route_id;
        self
    }

    pub fn push(&mut self, event: AgentEvent) -> Result<Vec<Value>, String> {
        if self.finished {
            return Err("provider event arrived after DSH settlement".into());
        }
        match &event {
            AgentEvent::Error { message, .. } => {
                self.finished = true;
                return Err(message.clone());
            }
            AgentEvent::Cancelled { .. } => {
                self.finished = true;
                return Err("provider request cancelled".into());
            }
            _ => {}
        }
        let (event, _) = self.round.ingest(event);
        if let AgentEvent::ToolCallInputDelta { call_id, delta, .. } = &event {
            let (_, (_, _, arguments)) = self
                .round
                .pending_calls
                .iter_mut()
                .find(|(id, _)| id == call_id)
                .ok_or("provider tool arguments arrived without a matching call")?;
            arguments.push_str(delta);
        }
        let (slot, block_type, delta_type, text) = match &event {
            AgentEvent::TextDelta { text, .. } => {
                (&mut self.text_index, "text", "text-delta", text)
            }
            AgentEvent::ReasoningDelta { text, .. } => (
                &mut self.reasoning_index,
                "reasoning",
                "reasoning-delta",
                text,
            ),
            _ => return Ok(Vec::new()),
        };
        let mut chunks = Vec::new();
        let index = *slot.get_or_insert_with(|| {
            let index = self.next_index;
            self.next_index += 1;
            chunks.push(json!({ "type":"block-start", "index":index, "blockType":block_type }));
            index
        });
        if !text.is_empty() {
            chunks.push(json!({ "type":delta_type, "index":index, "text":text }));
        }
        Ok(chunks)
    }

    pub fn finish(&mut self) -> Result<Vec<Value>, String> {
        if self.finished {
            return Err("DSH stream already finished".into());
        }
        self.round.validate_anthropic_replay()?;
        self.finished = true;
        let mut chunks = Vec::new();
        let mut blocks = Vec::new();
        if let Some(index) = self.text_index {
            blocks.push((index, "text", &self.round.text));
        }
        if let Some(index) = self.reasoning_index {
            blocks.push((index, "reasoning", &self.round.reasoning_content));
        }
        blocks.sort_by_key(|(index, _, _)| *index);
        for (index, kind, text) in blocks {
            chunks.push(
                json!({ "type":"block-end", "index":index, "block":{ "type":kind, "text":text } }),
            );
        }
        for (id, (name, input, arguments)) in &self.round.pending_calls {
            let index = self.next_index;
            self.next_index += 1;
            let arguments = if arguments.is_empty() {
                input.to_string()
            } else {
                arguments.clone()
            };
            chunks.extend([
                json!({ "type":"block-start", "index":index, "blockType":"tool-call" }),
                json!({ "type":"tool-call-delta", "index":index, "id":id, "name":name, "argumentsDelta":arguments }),
                json!({ "type":"block-end", "index":index, "block":{ "type":"tool-call", "id":id, "name":name, "arguments":arguments } }),
            ]);
        }
        if let Some(usage) = &self.round.usage {
            if let (Some(input), Some(output)) = (usage.input_tokens, usage.output_tokens) {
                let read = usage.cached_input_tokens.unwrap_or(0);
                let write = usage.cache_creation_input_tokens.unwrap_or(0);
                let uncached = match ProviderCapabilities::for_kind(self.kind).input_usage {
                    InputUsageSemantics::TotalPrompt => {
                        input.checked_sub(read.saturating_add(write))
                    }
                    InputUsageSemantics::UncachedPrompt => Some(input),
                }
                .ok_or("provider cache counts exceed input total")?;
                let mut value = json!({ "inputTokens":uncached, "outputTokens":output });
                if let Some(read) = usage.cached_input_tokens {
                    value["cacheReadTokens"] = json!(read);
                }
                if let Some(write) = usage.cache_creation_input_tokens {
                    value["cacheWriteTokens"] = json!(write);
                }
                chunks.push(json!({ "type":"usage", "usage":value }));
            }
        }
        let reason = if self.round.output_truncated {
            "max-tokens"
        } else if self.round.pending_calls.is_empty() {
            "stop"
        } else {
            "tool-calls"
        };
        chunks.push(json!({ "type":"finish", "reason":{ "kind":reason },
            "replayState":{ "response":{ "protocol":dsh_protocol(self.kind), "model":self.model, "routeId":self.route_id,
                "responsesReasoningItems":self.round.responses_reasoning_items,
                "anthropicThinkingBlocks":self.round.anthropic_thinking_blocks,
                "anthropicContentBlocks":self.round.anthropic_content_blocks } } }));
        Ok(chunks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compacted_request(revision: u64, latest_tool: bool) -> DshModelRequest {
        serde_json::from_value(json!({"sessionId":"s","model":"claude","protocol":"anthropic",
            "routeId":"r","compactionRevision":revision,
            "messages":[{"role":"user","content":[{"type":"text","text":"checkpoint"}]},
                {"id":"old","role":"assistant","source":{"replayState":{"response":{
                    "protocol":"anthropic","model":"claude","routeId":"r","compactionRevision":1,
                    "anthropicContentBlocks":[{"type":"thinking","thinking":"private","signature":"signed"},
                        {"type":"text","text":"before"},{"type":"tool_use","id":"call","name":"read_file","input":{}},
                        {"type":"redacted_thinking","data":"opaque"},{"type":"text","text":"after"}],
                    "anthropicThinkingBlocks":[{"type":"thinking","thinking":"private","signature":"signed"}]} }},
                    "content":if latest_tool {json!([{"type":"tool-call","id":"call","name":"read_file","arguments":"{}"}])}
                        else {json!([{"type":"text","text":"before after"}])}}
            ]})).unwrap()
    }

    #[test]
    fn compaction_migration_preserves_text_tool_order_and_the_original_record() {
        let mut request = compacted_request(2, false);
        let original = request.messages.clone();
        assert_eq!(request.migrate_compacted_thinking().unwrap(), vec!["old"]);
        let replay = &request.messages[1]["source"]["replayState"]["response"];
        assert_eq!(
            replay["anthropicContentBlocks"],
            json!([
            {"type":"text","text":"before"},{"type":"tool_use","id":"call","name":"read_file","input":{}},
            {"type":"text","text":"after"}])
        );
        assert_eq!(replay["anthropicThinkingBlocks"], json!([]));
        assert_eq!(
            original[1]["source"]["replayState"]["response"]["anthropicContentBlocks"]
                .as_array()
                .unwrap()
                .len(),
            5
        );
        assert_eq!(request.messages[1]["content"], original[1]["content"]);
        assert!(request.migrate_compacted_thinking().unwrap().is_empty());
    }

    #[test]
    fn compaction_migration_requires_new_revision_and_same_route_model_protocol() {
        for revision in [0, 1] {
            let mut request = compacted_request(revision, false);
            let original = request.messages.clone();
            assert!(request.migrate_compacted_thinking().unwrap().is_empty());
            assert_eq!(request.messages, original);
        }
        for key in ["routeId", "model", "protocol"] {
            let mut request = compacted_request(2, false);
            request.messages[1]["source"]["replayState"]["response"][key] = json!("different");
            let original = request.messages.clone();
            assert!(request.migrate_compacted_thinking().unwrap().is_empty());
            assert_eq!(request.messages, original);
        }
    }

    #[test]
    fn compaction_migration_refuses_unfinished_tool_thinking_in_conversation_calls() {
        let mut request = compacted_request(2, true);
        let original = request.messages.clone();
        assert!(request.migrate_compacted_thinking().is_err());
        assert_eq!(request.messages, original);
        request.purpose = Some("compaction".into());
        assert_eq!(request.migrate_compacted_thinking().unwrap(), vec!["old"]);
    }

    #[test]
    fn private_image_owners_survive_multiturn_guidance_on_every_protocol() {
        for kind in [
            ProviderKind::Anthropic,
            ProviderKind::OpenAi,
            ProviderKind::OpenAiResponses,
        ] {
            let mut messages = Vec::new();
            for id in ["first", "guidance"] {
                messages.push(json!({"id":id,"role":"user","source":{"kind":"user","nativeRequest":{
                    "attachments":[{"name":format!("{id}.png"),"mediaType":"image/png","dataUrl":"data:image/png;base64,aGVsbG8="}]}},
                    "content":[{"type":"text","text":id}]}));
                if id == "first" {
                    messages.push(json!({"id":"answer","role":"assistant","content":[{"type":"text","text":"reply"}]}));
                }
            }
            let request: DshModelRequest = serde_json::from_value(
                json!({"sessionId":"s","model":"mock","protocol":dsh_protocol(kind),
                "routeId":"r","maxTokens":4096,"messages":messages}),
            )
            .unwrap();
            let native = request
                .provider_request_with_vision(kind, "mock", Some(true))
                .unwrap();
            assert_eq!(native.image_attachments.len(), 2);
            assert_eq!(native.image_attachments[0].source_message_id, "first");
            assert_eq!(native.image_attachments[1].source_message_id, "guidance");
            let config = crate::ProviderConfig::new(
                "p",
                "fixture",
                kind,
                "https://example.invalid",
                "private-ref",
            )
            .unwrap();
            let wire = crate::request_body(&config, &native).unwrap().to_string();
            assert_eq!(wire.matches("aGVsbG8=").count(), 2);
            assert!(!wire.contains("nativeRequest"));
            let nonvisual = request
                .provider_request_with_vision(kind, "mock", Some(false))
                .unwrap();
            assert!(nonvisual.image_attachments.is_empty());
            assert!(nonvisual.messages[0].content.contains("first.png"));
            assert!(nonvisual.messages[2].content.contains("guidance.png"));
            assert!(!nonvisual.messages[0].content.contains("aGVsbG8="));
        }
    }

    #[test]
    fn admitted_dsh_images_replace_private_bytes_without_duplicating_or_crossing_owners() {
        for kind in [
            ProviderKind::Anthropic,
            ProviderKind::OpenAi,
            ProviderKind::OpenAiResponses,
        ] {
            let mut request: DshModelRequest = serde_json::from_value(json!({"sessionId":"s","model":"mock",
                "protocol":dsh_protocol(kind),"routeId":"r","maxTokens":4096,
                "messages":[{"id":"first","role":"user","source":{"nativeAttachmentsVersion":1,
                    "nativeRequest":{"attachments":[{"name":"raw.png","mediaType":"image/png","dataUrl":"data:image/png;base64,cmF3"}]}},
                    "content":[{"type":"text","text":"question"},{"type":"image",
                        "attachment":{"name":"normalized.png","mediaType":"image/png","attachmentId":"sha256:ref"},
                        "dataUrl":"data:image/png;base64,bm9ybWFsaXplZA=="}]}]})).unwrap();
            let native = request
                .provider_request_with_vision(kind, "mock", Some(true))
                .unwrap();
            assert_eq!(native.image_attachments.len(), 1);
            assert_eq!(native.image_attachments[0].source_message_id, "first");
            assert_eq!(native.image_attachments[0].name, "normalized.png");
            assert!(native.image_attachments[0]
                .data_url
                .ends_with("bm9ybWFsaXplZA=="));
            assert!(request
                .provider_request_with_vision(kind, "mock", Some(false))
                .is_err());
            request.messages[0]["content"] =
                json!([{"type":"text","text":"image omitted for text model"}]);
            let text = request
                .provider_request_with_vision(kind, "mock", Some(false))
                .unwrap();
            assert!(text.image_attachments.is_empty());
            assert_eq!(text.messages[0].content, "image omitted for text model");
        }
    }

    #[test]
    fn private_native_request_and_step_identity_never_enter_provider_wire_content() {
        for kind in [
            ProviderKind::Anthropic,
            ProviderKind::OpenAi,
            ProviderKind::OpenAiResponses,
        ] {
            let request: DshModelRequest = serde_json::from_value(json!({
                "sessionId":"s", "model":"mock", "protocol":dsh_protocol(kind), "routeId":"r", "maxTokens":4096,
                "turn":1, "step":2, "nativeTurnId":"native-only-turn",
                "messages":[{"id":"user-id", "role":"user", "source":{"kind":"user",
                    "nativeRequest":{"workspacePath":"native-only-path", "permissionMode":"full"}},
                    "content":[{"type":"text","text":"actual question"}]}]
            })).unwrap();
            let native = request.provider_request(kind, "mock").unwrap();
            let config = crate::ProviderConfig::new(
                "p",
                "fixture",
                kind,
                "https://example.invalid",
                "private-ref",
            )
            .unwrap();
            let wire = crate::request_body(&config, &native).unwrap().to_string();
            assert!(wire.contains("actual question"));
            assert!(!wire.contains("native-only"));
            assert!(!wire.contains("nativeRequest"));
            assert!(!wire.contains("permissionMode"));
        }
    }

    #[test]
    fn claude_prefix_binding_allows_appended_guidance_but_rejects_edits_to_history_or_tools() {
        let config = crate::ProviderConfig::new(
            "p",
            "Claude",
            ProviderKind::Anthropic,
            "https://example.invalid",
            "private-ref",
        )
        .unwrap();
        let mut request: DshModelRequest = serde_json::from_value(json!({
            "sessionId":"s", "model":"claude", "protocol":"anthropic", "routeId":"r", "maxTokens":4096,
            "messages":[{"role":"system","content":[{"type":"text","text":"stable instructions"}]},
                {"role":"user","content":[{"type":"text","text":"original question"}]}],
            "tools":[{"name":"read_file","description":"Read only","parameters":{"type":"object"}}]
        })).unwrap();
        let native = request
            .provider_request(ProviderKind::Anthropic, "claude")
            .unwrap();
        let hash = request.validate_anthropic_prefix(&config, &native).unwrap();
        let raw = json!([
            {"type":"thinking","thinking":"","signature":"opaque"},
            {"type":"text","text":"commentary"},
            {"type":"tool_use","id":"call","name":"read_file","input":{}}
        ]);
        request.messages.push(json!({"role":"assistant",
            "content":[{"type":"text","text":"commentary"},{"type":"tool-call","id":"call","name":"read_file","arguments":"{}"}],
            "source":{"replayState":{"response":{"protocol":"anthropic","model":"claude","routeId":"r",
                "anthropicContentBlocks":raw,"anthropicPrefixHash":hash}}}}));
        request.messages.push(
            json!({"role":"tool","toolCallId":"call","content":[{"type":"text","text":"result"}]}),
        );
        request
            .messages
            .push(json!({"role":"user","content":[{"type":"text","text":"new guidance"}]}));
        let native = request
            .provider_request(ProviderKind::Anthropic, "claude")
            .unwrap();
        assert_eq!(json!(native.messages[2].anthropic_content_blocks), raw);
        assert!(request.validate_anthropic_prefix(&config, &native).is_ok());
        request.messages[1]["content"][0]["text"] = json!("rewritten question");
        let native = request
            .provider_request(ProviderKind::Anthropic, "claude")
            .unwrap();
        assert!(request.validate_anthropic_prefix(&config, &native).is_err());
        request.messages[1]["content"][0]["text"] = json!("original question");
        request.messages[0]["content"][0]["text"] = json!("changed instructions");
        let native = request
            .provider_request(ProviderKind::Anthropic, "claude")
            .unwrap();
        assert!(request.validate_anthropic_prefix(&config, &native).is_err());
        request.messages[0]["content"][0]["text"] = json!("stable instructions");
        request.tools[0].description = "Changed tool".into();
        let native = request
            .provider_request(ProviderKind::Anthropic, "claude")
            .unwrap();
        assert!(request.validate_anthropic_prefix(&config, &native).is_err());
    }

    #[test]
    fn claude_prefix_hash_ignores_cache_hints_but_not_tool_input_fields() {
        let string = json!({"messages":[{"role":"user","content":"question"}]});
        let block = json!({"messages":[{"role":"user","content":[{"type":"text","text":"question","cache_control":{"type":"ephemeral"}}]}]});
        assert_eq!(
            anthropic_prefix_hashes(&string).unwrap(),
            anthropic_prefix_hashes(&block).unwrap()
        );
        let body = json!({"system":[{"type":"text","text":"fixed"}], "tools":[{"name":"tool"}],
            "messages":[{"role":"assistant","content":[{"type":"tool_use","input":{"cache_control":"user data"}}]}]});
        let expected = anthropic_prefix_hashes(&body).unwrap();
        let mut cached = body.clone();
        cached["system"][0]["cache_control"] = json!({"type":"ephemeral"});
        cached["tools"][0]["cache_control"] = json!({"type":"ephemeral"});
        cached["messages"][0]["content"][0]["cache_control"] = json!({"type":"ephemeral"});
        assert_eq!(expected, anthropic_prefix_hashes(&cached).unwrap());
        cached["messages"][0]["content"][0]["input"]["cache_control"] = json!("changed user data");
        assert_ne!(expected, anthropic_prefix_hashes(&cached).unwrap());
    }

    #[test]
    fn cache_counts_are_disjoint_and_missing_usage_stays_unknown() {
        for (kind, input, expected) in [
            (ProviderKind::OpenAiResponses, 100, 60),
            (ProviderKind::Anthropic, 100, 100),
        ] {
            let mut encoder = DshChunkEncoder::new(kind, "model");
            encoder
                .push(AgentEvent::Usage {
                    session_id: "s".into(),
                    input_tokens: Some(input),
                    output_tokens: Some(5),
                    cached_input_tokens: Some(30),
                    cache_creation_input_tokens: Some(10),
                    reasoning_tokens: None,
                })
                .unwrap();
            let chunks = encoder.finish().unwrap();
            assert_eq!(chunks[0]["usage"]["inputTokens"], expected);
            assert_eq!(chunks[0]["usage"]["cacheReadTokens"], 30);
        }
        assert_eq!(
            DshChunkEncoder::new(ProviderKind::OpenAi, "model")
                .finish()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn native_request_preserves_signed_thinking_and_rejects_wrong_routes_or_unknown_blocks() {
        let signed = json!({"type":"thinking","thinking":"","signature":"opaque"});
        let mut request: DshModelRequest = serde_json::from_value(json!({ "sessionId":"s", "model":"claude", "protocol":"anthropic", "maxTokens":4096,
            "messages":[{"id":"m", "role":"assistant", "content":[{"type":"tool-call","id":"call","name":"read_file","arguments":"{}"}],
                "source":{"replayState":{"response":{"protocol":"anthropic", "model":"claude", "anthropicThinkingBlocks":[signed]}}}}] })).unwrap();
        let native = request
            .provider_request(ProviderKind::Anthropic, "claude")
            .unwrap();
        assert_eq!(native.messages[0].anthropic_thinking_blocks[0], signed);
        assert_eq!(native.messages[0].tool_calls[0].id, "call");
        assert!(request
            .provider_request(ProviderKind::OpenAiResponses, "claude")
            .is_err());
        assert!(request
            .provider_request(ProviderKind::Anthropic, "other-model")
            .is_err());
        request.messages[0]["content"] = json!([{"type":"unknown"}]);
        assert!(request
            .provider_request(ProviderKind::Anthropic, "claude")
            .is_err());
    }

    #[test]
    fn responses_replay_is_opaque_and_kept_on_the_finish_frame() {
        let item = json!({ "id":"rs_1", "type":"reasoning", "encrypted_content":"opaque" });
        let mut encoder = DshChunkEncoder::new(ProviderKind::OpenAiResponses, "model");
        assert!(encoder
            .push(AgentEvent::ResponsesReasoningItem {
                session_id: "s".into(),
                item: item.clone()
            })
            .unwrap()
            .is_empty());
        let chunks = encoder.finish().unwrap();
        assert_eq!(
            chunks[0]["replayState"]["response"]["responsesReasoningItems"][0],
            item
        );
        assert!(encoder.finish().is_err());
    }

    #[test]
    fn streamed_tool_arguments_are_not_replaced_with_an_empty_object() {
        let mut encoder = DshChunkEncoder::new(ProviderKind::OpenAi, "model");
        encoder
            .push(AgentEvent::ToolCallStarted {
                session_id: "s".into(),
                call_id: "call".into(),
                tool_index: Some(0),
                tool_name: "read_file".into(),
                input: json!({}),
            })
            .unwrap();
        for delta in ["{\"path\":", "\"file.md\"}"] {
            encoder
                .push(AgentEvent::ToolCallInputDelta {
                    session_id: "s".into(),
                    call_id: "".into(),
                    tool_index: Some(0),
                    delta: delta.into(),
                })
                .unwrap();
        }
        let chunks = encoder.finish().unwrap();
        assert_eq!(chunks[2]["block"]["arguments"], "{\"path\":\"file.md\"}");
    }

    #[test]
    fn error_or_cancelled_events_cannot_be_reported_as_successful_finish() {
        for event in [
            AgentEvent::Error {
                session_id: "s".into(),
                message: "failed".into(),
            },
            AgentEvent::Cancelled {
                session_id: "s".into(),
            },
        ] {
            let mut encoder = DshChunkEncoder::new(ProviderKind::Anthropic, "model");
            assert!(encoder.push(event).is_err());
            assert!(encoder.finish().is_err());
        }
    }

    #[test]
    fn opaque_replay_does_not_cross_credentials_or_route_generations() {
        for (kind, protocol, field_name, blocks) in [
            (
                ProviderKind::Anthropic,
                "anthropic",
                "anthropicThinkingBlocks",
                json!([{"type":"thinking", "thinking":"", "signature":"signed"}]),
            ),
            (
                ProviderKind::OpenAiResponses,
                "openai_responses",
                "responsesReasoningItems",
                json!([{"type":"reasoning", "encrypted_content":"opaque"}]),
            ),
        ] {
            let mut replay =
                json!({"protocol":protocol, "model":"same-model", "routeId":"old-credentials"});
            replay[field_name] = blocks;
            let mut request: DshModelRequest = serde_json::from_value(json!({"sessionId":"s", "model":"same-model",
                "protocol":protocol, "routeId":"new-credentials", "maxTokens":4096,
                "messages":[{"id":"m", "role":"assistant", "content":[{"type":"text", "text":"preserved"}],
                    "source":{"replayState":{"response":replay}}}]})).unwrap();
            let native = request.provider_request(kind, "same-model").unwrap();
            assert_eq!(native.messages[0].content, "preserved");
            assert!(native.messages[0].anthropic_thinking_blocks.is_empty());
            assert!(native.messages[0].responses_reasoning_items.is_empty());
            request.route_id = Some("old-credentials".into());
            let native = request.provider_request(kind, "same-model").unwrap();
            assert!(
                !native.messages[0].anthropic_thinking_blocks.is_empty()
                    || !native.messages[0].responses_reasoning_items.is_empty()
            );
        }
    }
}
