use crate::agent_errors::AgentCommandError;
use crate::agent_events::{
    begin_tool_group, end_tool_group, stamp_payload, stamp_payload_with_turn,
};
use crate::agent_executor::CommandOutputSink;
use crate::file_changes::{capture_file_snapshot_with_cancellation, diff_file_snapshots};
use crate::logs::{
    index_log_paths, read_log_range, read_log_range_with_cancellation, rebuild_timeline,
    rebuild_timeline_with_cancellation, search_log_paths, search_log_paths_with_cancellation,
};
use crate::lsp::{execute as execute_lsp, LspOperation};
use crate::mcp_runtime::{connected_mcp_tools, ConnectedMcpTool, McpRuntimeState};
use crate::model_capability_cache::{
    known_context_window, read_cached_capabilities, refresh_remote_capabilities,
};
use crate::skill_runtime::{load_skill_catalog, load_skill_instructions};
use agent_core::{
    available_tokens as available_agent_context_tokens, bind_workspace_artifact,
    collect_model_round_with_observer, compact_messages_with_overhead, compose_agent_prompt,
    context_budget as agent_context_budget, context_message_id_at_boundary, context_turn_message,
    effective_context_window as effective_agent_context_window, estimate_image_attachment_tokens,
    estimate_request_tokens as estimate_agent_request_tokens,
    estimate_text_tokens as estimate_agent_text_tokens, execute_tool_call,
    execute_tool_call_with_start, image_attachments_for_messages, is_allowed_agent_path,
    is_cancelled, is_virtual_artifact_path, load_workspace_agent_context, max_agent_output_tokens,
    max_agent_output_tokens_continued_output, mcp_call_tool, merge_usage as merge_agent_usage,
    messages_endpoint, model_protocol, models_endpoint, normalize_image_attachments,
    output_limit_reached, output_reserve as agent_output_reserve, parse_analysis_report,
    parse_tool_call_input, protocol_from_registry_name, protocol_label, provider_protocol,
    register_pending_permission, request_snapshot_paths,
    steer_message_content, take_tool_group_fallback, tool_call_signature,
    update_image_token_reserve, usage_percent as context_usage_percent,
    wait_for_agent_cancellation, wait_for_permission, workspace_context_prompt, AgentCacheStats,
    AgentCancellationStatus, AgentChatRequest, AgentChatResponse, AgentConnectionView,
    AgentContextCheckpoint, AgentContextTurn, AgentEvent, AgentImageAttachment, AgentLogEvidence,
    AgentMessage, AgentModelDiscoveryRequest, AgentModelOption, AgentModelSettings,
    AgentModelSummary, AgentProviderCheckRequest, AgentProviderSettings, AgentProviderSummary,
    AgentQuotaView, AgentRequestGuard, AgentRuntimeState, AgentSteerRequest, AgentToolCall,
    AgentToolPolicy, AgentUsage, AllowPermission, ConnectionStatus, ConversationCommand,
    DenyPermission, HttpProviderClient, JiraIssueContext, LocalArtifact, LocalToolContext,
    LogIndexEntry, LogRangeRequest, LogRangeResult, LogSearchHit, LogTimelineEvent,
    McpServerConfig, MessageRole, ModelProfile, ModelRoundError, PermissionAction, PermissionMode,
    PermissionResponse, PermissionWaitResult, ProviderCapabilities, ProviderConfig,
    ProviderImageAttachment, ProviderKind, ProviderRequest, ThreadEvent, ThreadItem,
    ThreadUsageSnapshot, TimelineSegment, ToolDefinition, ToolHandler, ToolRegistry, TurnStatus,
    WorkspaceAgentContext, WslRuntimeConfig, PATH_ACCESS_PROBE_TOOL,
};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use futures_util::StreamExt;
#[cfg(test)]
use jira::jira_description_to_text;
use keyring::{Entry, Error as KeyringError};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
use url::Url;

pub(crate) mod background_shell;
mod background_store;
mod cache_context;
mod prompt_composer;
mod ssh_connection;
mod user_interaction;

#[cfg(test)]
use agent_core::is_selected_path;

// Jira helpers remain available for the optional integration.
#[allow(dead_code)]
const JIRA_CREDENTIAL_SERVICE: &str = "android-tool-jira";
const PROVIDER_CREDENTIAL_SERVICE: &str = "android-tool-provider";
const PROVIDER_USAGE_TOKEN_SERVICE: &str = "android-tool-provider-usage-token";
const PROVIDER_CONFIG_FILE: &str = "agent-providers.json";
const MAX_LOCAL_IMAGE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_BROWSER_ATTACHMENT_BYTES: u64 = 128 * 1024 * 1024;
const MAX_AGENT_TRACE_BYTES: u64 = 256 * 1024 * 1024;
// Bound a single fan-in wait, not the lifetime of the parent turn or its jobs.
const COLLABORATION_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15 * 60);
const TOOL_CANCELLATION_GRACE: std::time::Duration = std::time::Duration::from_secs(10);
const MAX_PARALLEL_TOOL_EXECUTIONS: usize = 4;
// One API/New API relay quota is measured in internal units. This conversion
// is applied only to the documented `quota`/`used_quota` shape, never to an
// arbitrary balance field whose unit is unknown.
const NEW_API_QUOTA_UNITS_PER_CURRENCY: f64 = 500_000.0;

// A model that does not expose its limit should not be treated as a 16K model:
// that made ordinary 30K-40K local conversations compact immediately. Keep a
// provisional 64K window and fall back adaptively if the Provider rejects it.
// Never send a request whose output budget is effectively unusable. If this
// minimum cannot be reserved after compaction, the runtime stops locally
// instead of asking a provider for a one-token response.
const MIN_AGENT_FINAL_OUTPUT_TOKENS: usize = agent_core::MIN_FINAL_OUTPUT_TOKENS;
const MIN_AGENT_TOOL_OUTPUT_TOKENS: usize = agent_core::MIN_TOOL_OUTPUT_TOKENS;
const AGENT_OUTPUT_CONTINUATION_LIMIT: usize = 3;
const AGENT_PROVIDER_CONTEXT_RETRY_LIMIT: usize = 3;
const CONTEXT_CHECKPOINT_EVIDENCE_POLICY_VERSION: u8 = 3;

mod execution;
mod execution_scope;
use execution_scope::{
    consume_one_time_path_approval, default_tool_environment, tool_execution_scope,
    AgentToolEnvironment,
};
mod tool_execution;
#[cfg(test)]
use tool_execution::active_turn_owned_by;
use tool_execution::{
    cancellation_message, execute_tool_call_interruptible, InterruptibleToolExecution,
    ToolExecutionLanes,
};
mod jira;

#[cfg(test)]
use jira::parse_jira_url_value;
mod attachments;
pub use attachments::*;
use attachments::{
    append_image_source_labels, apply_steer_runtime_inputs, normalize_request_image_attachments,
    StoredProviderSettings,
};
mod turn_support;
#[cfg(test)]
use turn_support::provider_round_accepted_input;
use turn_support::{
    context_checkpoint_event_value, enrich_context_checkpoint_with_model,
    flush_deferred_timeline_text, is_skill_catalog_question, prepare_agent_ssh_context,
    provider_error_is_context_limit, provider_startup_error_can_recover,
    provider_usage_option_rejected, remember_context_summary_prefix,
    responses_reasoning_include_rejected, skill_catalog_prompt, uuid_suffix,
};
mod providers;
pub use providers::*;
use providers::{
    is_known_official_provider_base_url, load_providers, provider_credential, value_as_f64,
    value_as_u64,
};
#[cfg(test)]
use providers::{provider_error_code, provider_model_is_listed};
mod quota;
#[cfg(test)]
use quota::{
    custom_quota_token, parse_relay_quota, query_custom_quota_endpoint, relay_auth_failure_message,
    relay_quota_endpoints, relay_response_declares_failure,
};
use quota::{query_custom_quota, query_relay_quota, query_usage_script};
mod log_commands;
pub use log_commands::*;
mod application_tools;
mod hooks;
mod workflow;
use workflow::WorkflowTool;
mod skill_tools;
use application_tools::{
    CreateCollaborationJobTool, GoalTools, ReadLogRangeTool, RebuildTimelineTool,
    SearchLocalLogsTool, UpdateTaskPlanTool,
};
use skill_tools::LoadSkillTool;
mod tool_ledger;
use tool_ledger::{
    begin_tool_call, complete_tool_call, failed_file_read_instruction, file_path_key,
    file_read_path_key, finish_pending_agent_tool_calls, normalize_tool_execution_result,
    tool_call_can_run, tool_call_skip_reason, tool_execution_failure_output,
    tool_execution_failure_retryable, ToolCallLedgerEntry,
};
mod tool_registry;
pub use tool_registry::*;
use tool_registry::{build_environment_tool_registries, register_collaboration_tool};
#[cfg(test)]
use tool_registry::{local_tool_registry, mcp_tool_name};
mod conversation_commands;
#[cfg(test)]
use conversation_commands::active_steer_sender;
pub use conversation_commands::*;
use conversation_commands::{mark_agent_steer_applied, replay_pending_agent_steers};
mod events;
mod file_review;
use file_review::refresh_turn_file_changes;
mod process_checkpoint;
pub(crate) use events::emit_agent_event_payload_for_turn;
#[cfg(test)]
use events::{
    apply_unadmitted_start_failure_snapshot, context_item_id, display_segments_for_snapshot,
    process_events_for_snapshot, public_text_for_items, result_text_for_snapshot,
    should_persist_agent_payload,
};
use events::{
    begin_agent_task, emit_agent_context_for_turn, emit_agent_event_for_turn,
    emit_agent_event_for_turn_with_call_id, emit_agent_event_for_turn_with_file_evidence_path_key,
    emit_agent_payload_for_turn, emit_agent_status_for_turn, emit_durable_terminal_payload,
    emit_model_agent_event_for_turn, emit_start_failure_snapshot, emit_streaming_context_snapshot,
    emit_thread_snapshot_event, emit_tool_execution_started, end_agent_task, request_session_id,
    settle_thread_process, user_message_item, AgentQueueAdmission,
};
pub(crate) use process_checkpoint::flush_running_processes;
mod completion_notice;
pub(crate) use completion_notice::{
    __cmd__dismiss_completion_notice, __cmd__get_completion_notice, __cmd__open_completion_notice,
    __cmd__show_completion_notice, __tauri_command_name_dismiss_completion_notice,
    __tauri_command_name_get_completion_notice, __tauri_command_name_open_completion_notice,
    __tauri_command_name_show_completion_notice, dismiss_completion_notice, get_completion_notice,
    notify_completion, open_completion_notice, show_completion_notice, CompletionNoticeState,
};
mod queue;
#[cfg(test)]
use queue::merge_history_and_request_context;
pub use queue::*;
pub(crate) use queue::{
    dispatch_queued_agent_request, execute_collaboration_agent_request, recover_durable_agent_queue,
};
use queue::{
    hydrate_agent_queue_session, text_segment_may_be_plain_tool_payload, AgentToolGroupCleanup,
};
mod turn;
use turn::run_agent_turn;
mod agent_settings;
pub(crate) use agent_settings::{
    __cmd__get_agent_settings, __cmd__update_agent_settings,
    __tauri_command_name_get_agent_settings, __tauri_command_name_update_agent_settings,
    get_agent_settings, update_agent_settings,
};
#[cfg(test)]
mod tests;

// Tauri command wrappers remain available at the established registration path.
pub(crate) use attachments::{
    __cmd__prepare_chat_attachment, __cmd__prepare_workspace_agent_context,
    __cmd__read_cached_image, __cmd__read_local_image, __cmd__store_browser_attachment,
    __tauri_command_name_prepare_chat_attachment,
    __tauri_command_name_prepare_workspace_agent_context, __tauri_command_name_read_cached_image,
    __tauri_command_name_read_local_image, __tauri_command_name_store_browser_attachment,
};
pub(crate) use conversation_commands::{
    __cmd__execute_conversation_command, __cmd__resolve_agent_permission,
    __cmd__steer_agent_request, __tauri_command_name_execute_conversation_command,
    __tauri_command_name_resolve_agent_permission, __tauri_command_name_steer_agent_request,
};
pub(crate) use log_commands::{
    __cmd__index_local_logs, __cmd__read_local_log_range, __cmd__rebuild_local_log_timeline,
    __cmd__search_local_logs, __tauri_command_name_index_local_logs,
    __tauri_command_name_read_local_log_range, __tauri_command_name_rebuild_local_log_timeline,
    __tauri_command_name_search_local_logs,
};
pub(crate) use providers::{
    __cmd__check_agent_provider, __cmd__discover_agent_models,
    __cmd__get_agent_model_context_window, __cmd__get_local_artifacts, __cmd__get_provider_quota,
    __cmd__refresh_agent_capabilities, __cmd__test_agent_provider_chat,
    __tauri_command_name_check_agent_provider, __tauri_command_name_discover_agent_models,
    __tauri_command_name_get_agent_model_context_window, __tauri_command_name_get_local_artifacts,
    __tauri_command_name_get_provider_quota, __tauri_command_name_refresh_agent_capabilities,
    __tauri_command_name_test_agent_provider_chat,
};
pub(crate) use queue::{
    __cmd__agent_one_shot, __cmd__enqueue_agent_message, __cmd__get_agent_thread_snapshot,
    __cmd__resume_agent_run, __tauri_command_name_agent_one_shot,
    __tauri_command_name_enqueue_agent_message, __tauri_command_name_get_agent_thread_snapshot,
    __tauri_command_name_resume_agent_run,
};
pub(crate) use tool_registry::{
    __cmd__cancel_agent_request, __tauri_command_name_cancel_agent_request,
};

mod strategy;
mod turn_prepare;
mod turn_startup;
use turn_startup::{
    create_turn_provider_client, prepare_turn_skills, reasoning_recovery_effort,
    spawn_turn_idle_watchdog, turn_collaboration_tool, turn_reasoning_settings,
    turn_workspace_context, BoundSshHost, TurnEnvironments, TurnSkillContext, FINAL_PHASE_NOTICE,
};
