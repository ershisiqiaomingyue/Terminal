use crate::{AgentContextCheckpointPayload, AgentMessage};
use serde::{Deserialize, Serialize};

mod estimate;
pub use estimate::*;

pub const DEFAULT_CONTEXT_WINDOW: usize = 64_000;
pub const FINAL_OUTPUT_TOKENS: usize = 2_048;
pub const TOOL_OUTPUT_TOKENS: usize = 8_192;
pub const MIN_FINAL_OUTPUT_TOKENS: usize = 1_024;
pub const MIN_TOOL_OUTPUT_TOKENS: usize = 512;
pub const OUTPUT_LIMIT_THRESHOLD_PERCENT: usize = 95;
pub const MIN_CONTEXT_WINDOW: usize = 1_024;
pub const UNKNOWN_CONTEXT_RETRY_WINDOWS: [usize; 3] = [48_000, 32_000, 16_000];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextRetryBudget {
    pub target_context_window: usize,
    pub message_budget: usize,
    pub fixed_overhead_tokens: usize,
    pub reserved_output_tokens: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextWindowStatus {
    Known,
    Unknown,
}

/// Separates a provider/model declaration from the conservative value used by
/// local preflight. `reported_tokens` remains `None` for unknown models, so a
/// caller never has to present the 64K safety value as model metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextWindowResolution {
    pub status: ContextWindowStatus,
    pub reported_tokens: Option<usize>,
    pub effective_tokens: usize,
    pub conservative: bool,
}

impl ContextWindowResolution {
    pub fn is_known(self) -> bool {
        self.status == ContextWindowStatus::Known
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionStatus {
    NotRequired,
    Checkpointed,
    CheckpointedWithTruncation,
    TruncatedToBudget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionStats {
    pub budget_tokens: usize,
    pub original_estimated_tokens: usize,
    pub compacted_estimated_tokens: usize,
    pub original_message_count: usize,
    pub compacted_message_count: usize,
    pub covered_message_count: usize,
    pub retained_recent_message_count: usize,
    pub checkpoint_estimated_tokens: usize,
    pub system_constraint_count: usize,
    pub pending_user_item_count: usize,
    pub tool_evidence_count: usize,
    pub historical_conclusion_count: usize,
    pub truncated_message_count: usize,
    pub checkpoint_truncated: bool,
    pub budget_satisfied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionResult {
    pub messages: Vec<AgentMessage>,
    pub compressed: bool,
    pub status: CompactionStatus,
    pub summary: Option<String>,
    pub covered_message_count: Option<usize>,
    pub covered_message_id: Option<String>,
    pub checkpoint_payload: Option<AgentContextCheckpointPayload>,
    pub stats: CompactionStats,
}
