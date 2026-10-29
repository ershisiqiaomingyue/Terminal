// Event adaptation is grouped by responsibility; each child owns one stage of the pipeline.
use super::*;

mod admission;
mod context;
mod evidence;
mod lifecycle;
mod payload;
mod persistence;
mod process;
mod progress;
mod projection;
mod publication;
mod recovery;
mod request;
mod serialization;
mod streaming;
mod tasks;
mod terminal;
mod text;
mod timeline;

pub(super) use admission::AgentQueueAdmission;
pub(super) use context::emit_agent_context_for_turn;
pub(super) use evidence::emit_tool_execution_started;
pub(crate) use payload::emit_agent_event_payload_for_turn;
pub(super) use payload::emit_agent_payload_for_turn;
pub(super) use process::{process_events_for_snapshot, settle_thread_process};
pub(super) use progress::emit_compaction_progress;
pub(super) use projection::emit_thread_snapshot_for_payload;
pub(super) use publication::emit_thread_snapshot_event;
pub(super) use recovery::emit_start_failure_snapshot;
pub(super) use request::request_session_id;
pub(super) use serialization::{
    emit_agent_event_for_turn, emit_agent_event_for_turn_with_call_id,
    emit_agent_event_for_turn_with_file_evidence_path_key, emit_model_agent_event_for_turn,
};
pub(super) use streaming::emit_streaming_context_snapshot;
pub(super) use tasks::{begin_agent_task, emit_agent_status_for_turn, end_agent_task};
pub(super) use terminal::emit_durable_terminal_payload;
pub(super) use timeline::user_message_item;

#[cfg(test)]
pub(super) use persistence::should_persist_agent_payload;
#[cfg(test)]
pub(super) use progress::compaction_progress_payload;
#[cfg(test)]
pub(super) use projection::context_item_id;
#[cfg(test)]
pub(super) use recovery::apply_unadmitted_start_failure_snapshot;
#[cfg(test)]
pub(super) use terminal::terminal_status_notice;
#[cfg(test)]
pub(super) use text::{public_text_for_items, result_text_for_snapshot};
#[cfg(test)]
pub(super) use timeline::display_segments_for_snapshot;
