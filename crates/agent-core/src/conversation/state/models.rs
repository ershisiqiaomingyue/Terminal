use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    Idle,
    Running,
    WaitingForPermission,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ThreadItem {
    UserMessage {
        id: String,
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        artifacts: Vec<Value>,
    },
    AgentMessage {
        id: String,
        content: String,
        pending: bool,
    },
    ToolCall {
        id: String,
        name: String,
        input: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_buffer: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        live_output: Option<Value>,
        #[serde(default)]
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_group_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        queued_at_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        started_at_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        finished_at_ms: Option<u64>,
        is_error: bool,
    },
    TaskPlan {
        id: String,
        goal: String,
        steps: Vec<Value>,
    },
    FileChanges {
        id: String,
        data: Value,
    },
    Context {
        id: String,
        data: Value,
    },
    FinalReport {
        id: String,
        data: Value,
    },
    TimelineEvent {
        id: String,
        data: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSnapshot {
    pub id: String,
    pub status: TurnStatus,
    pub error: Option<String>,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
}

pub(super) fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionSnapshot {
    pub request_id: String,
    pub tool_name: String,
    /// N2：等待到期时刻（epoch 毫秒），前端权限条据此显示倒计时。
    #[serde(default)]
    pub expires_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedThreadItem {
    pub id: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<QueuedThreadImage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<QueuedThreadArtifact>,
}

/// Only cached references travel in snapshots; thumbnail bytes are loaded on demand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedThreadImage {
    pub name: String,
    pub media_type: String,
    pub path: String,
}

/// Attachment references only; queued snapshots must not carry artifact contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedThreadArtifact {
    pub name: String,
    pub kind: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadUsageSnapshot {
    #[serde(default)]
    pub provider_kind: Option<crate::ProviderKind>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub generation_tokens: Option<u64>,
    #[serde(default)]
    pub generation_duration_ms: Option<u64>,
    #[serde(default)]
    pub generation_timing_source: Option<String>,
    pub cached_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    pub latest_request_usage: Option<crate::RequestCacheUsage>,
    pub context_window: Option<u64>,
    pub context_tokens: Option<u64>,
    pub reserved_output_tokens: Option<u64>,
    pub available_tokens: Option<u64>,
    /// Provider-reported reasoning/thinking tokens for the live turn. Absent
    /// when the provider did not report the counter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
}

impl ThreadUsageSnapshot {
    pub fn merge(&mut self, incoming: Self) {
        if incoming.provider_kind.is_some() {
            self.provider_kind = incoming.provider_kind;
        }
        if incoming.input_tokens.is_some() {
            self.input_tokens = incoming.input_tokens;
        }
        if incoming.output_tokens.is_some() {
            self.output_tokens = incoming.output_tokens;
        }
        if incoming.generation_tokens.is_some() {
            self.generation_tokens = incoming.generation_tokens;
        }
        if incoming.generation_duration_ms.is_some() {
            self.generation_duration_ms = incoming.generation_duration_ms;
        }
        if incoming.generation_tokens.is_some() || incoming.generation_duration_ms.is_some() {
            self.generation_timing_source = incoming.generation_timing_source;
        }
        if incoming.cached_input_tokens.is_some() {
            self.cached_input_tokens = incoming.cached_input_tokens;
        }
        if incoming.cache_creation_input_tokens.is_some() {
            self.cache_creation_input_tokens = incoming.cache_creation_input_tokens;
        }
        if incoming.latest_request_usage.is_some() {
            self.latest_request_usage = incoming.latest_request_usage;
        }
        if incoming.context_window.is_some() {
            self.context_window = incoming.context_window;
        }
        if incoming.context_tokens.is_some() {
            self.context_tokens = incoming.context_tokens;
        }
        if incoming.reserved_output_tokens.is_some() {
            self.reserved_output_tokens = incoming.reserved_output_tokens;
        }
        if incoming.available_tokens.is_some() {
            self.available_tokens = incoming.available_tokens;
        }
        if incoming.reasoning_tokens.is_some() {
            self.reasoning_tokens = incoming.reasoning_tokens;
        }
    }
}

impl Default for ThreadUsageSnapshot {
    fn default() -> Self {
        Self {
            provider_kind: None,
            input_tokens: None,
            output_tokens: None,
            generation_tokens: None,
            generation_duration_ms: None,
            generation_timing_source: None,
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            latest_request_usage: None,
            context_window: None,
            context_tokens: None,
            reserved_output_tokens: None,
            available_tokens: None,
            reasoning_tokens: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRuntimeSnapshot {
    pub activity: String,
    pub tool_call_count: usize,
    pub usage: ThreadUsageSnapshot,
    pub execution_target: Option<Value>,
}
