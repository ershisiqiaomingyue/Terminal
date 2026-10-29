//! Context compaction: trigger calculation, bounded checkpoints, budget fitting,
//! checkpoint recovery, and the public history conversion entry points.

mod budget_fit;
mod calibrate;
mod checkpoint_messages;
mod checkpoint_restore;
mod compaction_core;
pub(crate) mod consts;
mod history;
mod history_window;
mod semantic_checkpoint;
mod text_trim;
mod triggers;

pub use calibrate::{CompactionHysteresis, TokenEstimateCalibrator};
pub use checkpoint_restore::{legacy_context_checkpoint_message, restore_context_checkpoint};
pub use consts::{CONTEXT_CHECKPOINT_FORMAT_VERSION, CONTEXT_SUMMARY_PREFIX};
pub use history::{
    build_agent_messages, build_agent_messages_detailed, compact_agent_messages,
    context_message_id_at_boundary, context_turn_message,
};
pub use triggers::{
    compaction_trigger_tokens, compaction_trigger_tokens_scaled,
    compaction_trigger_tokens_scaled_with_overhead, compaction_trigger_tokens_with_overhead,
};

pub use compaction_core::{
    compact_messages, compact_messages_with_current, compact_messages_with_current_with_overhead,
    compact_messages_with_overhead, compact_messages_with_trigger, plan_compaction_checkpoint,
};

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(super) use checkpoint_messages::checkpoint_entries_to_messages;
#[cfg(test)]
pub(super) use semantic_checkpoint::build_semantic_checkpoint;
#[cfg(test)]
pub(super) use triggers::{
    retained_recent_tokens, CHECKPOINT_TOKEN_CEILING, COMPACTION_TRIGGER_MIN_TOKENS,
    RETAIN_MIN_MESSAGES, RETAIN_TOKEN_CEILING,
};
