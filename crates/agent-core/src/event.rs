use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMessage {
    pub role: MessageRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// Opaque Responses reasoning items must be replayed when `store:false`.
    /// They are provider protocol state, not user-visible reasoning text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub responses_reasoning_items: Vec<Value>,
    /// Signed Claude thinking blocks are replayed verbatim, never synthesized from UI text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anthropic_thinking_blocks: Vec<Value>,
    /// Complete original Claude content in stream order, independent of UI text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anthropic_content_blocks: Vec<Value>,
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<AgentToolCall>,
    /// Internal history provenance. It is intentionally excluded from every
    /// provider request and only survives local context compaction.
    #[serde(default, skip_serializing, skip_deserializing)]
    pub source_message_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

impl AgentMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
            reasoning_content: None,
            responses_reasoning_items: Vec::new(),
            anthropic_thinking_blocks: Vec::new(),
            anthropic_content_blocks: Vec::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            source_message_id: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
            reasoning_content: None,
            responses_reasoning_items: Vec::new(),
            anthropic_thinking_blocks: Vec::new(),
            anthropic_content_blocks: Vec::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            source_message_id: None,
        }
    }

    pub fn assistant_tool_call(call: AgentToolCall) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: String::new(),
            reasoning_content: None,
            responses_reasoning_items: Vec::new(),
            anthropic_thinking_blocks: Vec::new(),
            anthropic_content_blocks: Vec::new(),
            tool_call_id: None,
            tool_calls: vec![call],
            source_message_id: None,
        }
    }

    pub fn tool(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Tool,
            content: content.into(),
            reasoning_content: None,
            responses_reasoning_items: Vec::new(),
            anthropic_thinking_blocks: Vec::new(),
            anthropic_content_blocks: Vec::new(),
            tool_call_id: Some(call_id.into()),
            tool_calls: Vec::new(),
            source_message_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    TextDelta {
        session_id: String,
        text: String,
    },
    ReasoningDelta {
        session_id: String,
        text: String,
    },
    /// Opaque Responses API reasoning item captured for stateless replay.
    /// The runtime intentionally does not emit this event to the UI.
    ResponsesReasoningItem {
        session_id: String,
        item: Value,
    },
    /// Private protocol state; must not be emitted to the conversation UI.
    AnthropicThinkingFrame {
        session_id: String,
        frame: Value,
    },
    Usage {
        session_id: String,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        cached_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
        /// Provider-reported reasoning/thinking tokens. Absent means the provider
        /// did not report a value; never derived from other counters.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_tokens: Option<u64>,
    },
    /// The provider ended the response because its output budget was spent.
    /// This is runtime control state and is not rendered as a user-visible event.
    OutputTruncated {
        session_id: String,
    },
    ToolCallStarted {
        session_id: String,
        call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_index: Option<usize>,
        tool_name: String,
        input: Value,
    },
    ToolCallInputDelta {
        session_id: String,
        call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_index: Option<usize>,
        delta: String,
    },
    ToolCallFinished {
        session_id: String,
        call_id: String,
        tool_name: String,
        input: Value,
        output: Value,
        is_error: bool,
    },
    FileChanges {
        session_id: String,
        created: Vec<String>,
        modified: Vec<String>,
        deleted: Vec<String>,
        added_lines: u64,
        removed_lines: u64,
        #[serde(default)]
        entries: Vec<FileChangeEntry>,
    },
    PermissionRequested {
        session_id: String,
        request_id: String,
        tool_name: String,
        input: Value,
        /// N2：本次权限等待的到期时刻（epoch 毫秒）。由轮次侧在发出前
        /// 盖章（turn.rs 的 TOOL_PERMISSION_TIMEOUT）；None = 未标注。
        #[serde(default)]
        expires_at_ms: Option<i64>,
    },
    Status {
        session_id: String,
        status: String,
        message: Option<String>,
    },
    Error {
        session_id: String,
        message: String,
    },
    Completed {
        session_id: String,
    },
    Cancelled {
        session_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeEntry {
    pub path: String,
    pub status: String,
    pub added_lines: u64,
    pub removed_lines: u64,
}
