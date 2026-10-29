use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::models::{ThreadItem, ThreadUsageSnapshot, TurnStatus};

impl ThreadItem {
    pub(super) fn id(&self) -> &str {
        match self {
            Self::UserMessage { id, .. }
            | Self::AgentMessage { id, .. }
            | Self::ToolCall { id, .. }
            | Self::TaskPlan { id, .. }
            | Self::FileChanges { id, .. } => id,
            Self::Context { id, .. }
            | Self::FinalReport { id, .. }
            | Self::TimelineEvent { id, .. } => id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ThreadEvent {
    TurnStarted {
        thread_id: String,
        turn_id: String,
    },
    ItemAdded {
        thread_id: String,
        turn_id: String,
        item: ThreadItem,
    },
    ToolCallInputDelta {
        thread_id: String,
        turn_id: String,
        call_id: String,
        delta: String,
    },
    ToolCallOutput {
        thread_id: String,
        turn_id: String,
        call_id: String,
        output: Value,
    },
    AgentDelta {
        thread_id: String,
        turn_id: String,
        item_id: String,
        text: String,
    },
    AgentFinal {
        thread_id: String,
        turn_id: String,
        item_id: String,
        content: String,
        /// Streaming hosts already published authoritative ordered segments.
        /// Their aggregate omits runtime notes and may reset after a steer.
        #[serde(default)]
        preserve_streamed: bool,
    },
    PermissionRequested {
        thread_id: String,
        turn_id: String,
        request_id: String,
        tool_name: String,
        #[serde(default)]
        expires_at_ms: Option<i64>,
    },
    PermissionResolved {
        thread_id: String,
        turn_id: String,
        request_id: String,
    },
    TurnCompleted {
        thread_id: String,
        turn_id: String,
        status: TurnStatus,
        error: Option<String>,
    },
    RuntimeUpdated {
        thread_id: String,
        turn_id: String,
        activity: String,
        tool_call_count: usize,
    },
    UsageUpdated {
        thread_id: String,
        turn_id: String,
        usage: ThreadUsageSnapshot,
    },
    ExecutionTargetUpdated {
        thread_id: String,
        turn_id: String,
        execution_target: Option<Value>,
    },
}

impl ThreadEvent {
    pub fn source_thread_id(&self) -> &str {
        self.thread_id()
    }
    pub(super) fn thread_id(&self) -> &str {
        match self {
            Self::TurnStarted { thread_id, .. }
            | Self::ItemAdded { thread_id, .. }
            | Self::ToolCallInputDelta { thread_id, .. }
            | Self::ToolCallOutput { thread_id, .. }
            | Self::AgentDelta { thread_id, .. }
            | Self::AgentFinal { thread_id, .. }
            | Self::PermissionRequested { thread_id, .. }
            | Self::PermissionResolved { thread_id, .. }
            | Self::TurnCompleted { thread_id, .. }
            | Self::RuntimeUpdated { thread_id, .. }
            | Self::UsageUpdated { thread_id, .. }
            | Self::ExecutionTargetUpdated { thread_id, .. } => thread_id,
        }
    }
    pub(super) fn turn_id(&self) -> Option<&str> {
        match self {
            Self::TurnStarted { .. } => None,
            Self::ItemAdded { turn_id, .. }
            | Self::ToolCallInputDelta { turn_id, .. }
            | Self::ToolCallOutput { turn_id, .. }
            | Self::AgentDelta { turn_id, .. }
            | Self::AgentFinal { turn_id, .. }
            | Self::PermissionRequested { turn_id, .. }
            | Self::PermissionResolved { turn_id, .. }
            | Self::TurnCompleted { turn_id, .. }
            | Self::RuntimeUpdated { turn_id, .. }
            | Self::UsageUpdated { turn_id, .. }
            | Self::ExecutionTargetUpdated { turn_id, .. } => Some(turn_id),
        }
    }
}
