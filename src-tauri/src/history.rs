#![allow(dead_code)]

use agent_core::{
    legacy_context_checkpoint_message, restore_context_checkpoint, AgentChatRequest,
    AgentContextCheckpoint, AgentContextTurn, AgentExecutionTarget, AgentMessage,
    AgentRuntimeState, AgentSteerRequest, AgentToolPolicy,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};

pub const HISTORY_SCHEMA_VERSION: i64 = 6;
const HISTORY_PROCESS_PAGE_SIZE: usize = 100;
const HISTORY_PROCESS_PAGE_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryTurnEventPage {
    pub events: Vec<Value>,
    pub next_cursor: Option<i64>,
    pub through_row_id: i64,
}
// Conversation messages are the durable record. Tool-stream events are a
// visual aid and can grow to tens of thousands of rows in one long turn.
// Restoring all of them blocks the WebView before the conversation is usable.
const MAX_RESTORED_SESSION_EVENTS: usize = 1_000;
// WAL 模式下读不再被写阻塞，但写写互斥仍在：大库（数百 MB、messages 整行
// 重写）上的长写事务可能超过旧的 3 秒档，把后续写顶成 BUSY 失败——终态
// 落库失败会放大成 steer 悬挂（N12）。15 秒给长事务留足余量。
const HISTORY_WRITE_BUSY_TIMEOUT: Duration = Duration::from_secs(15);
const TERMINAL_HISTORY_WRITE_ATTEMPTS: usize = 2;
const MAX_AGENT_QUEUE_ITEMS_PER_SESSION: i64 = 20;
const AGENT_QUEUE_LEASE_MS: i64 = 5 * 60 * 1_000;
const COLLABORATION_LEASE_MS: i64 = 20 * 60 * 1_000;
pub(crate) const MAX_COLLABORATION_CONCURRENCY: usize = 4;
pub(crate) const COLLABORATION_CAPACITY_ERROR: &str = "协作并发额度已满，子任务保持排队";
static HISTORY_ID_SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
const LEGACY_HISTORY_SCHEMA_VERSION: i64 = 1;

fn history_write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn current_history_schema_version() -> i64 {
    HISTORY_SCHEMA_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HistorySession {
    pub id: String,
    pub title: String,
    pub updated_at: i64,
    pub archived: bool,
    pub jira_context: Option<Value>,
    pub artifacts: Value,
    pub messages: Value,
    pub metadata: Value,
    pub report: Option<Value>,
    pub usage: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HistorySessionRecord {
    pub id: String,
    pub title: String,
    pub updated_at: i64,
    pub archived: bool,
    #[serde(default = "current_history_schema_version")]
    pub schema_version: i64,
    pub jira_context: Option<Value>,
    pub artifacts: Value,
    pub messages: Value,
    pub metadata: Value,
    pub report: Option<Value>,
    pub usage: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_runs: Vec<AgentRunSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_totals: Option<HistoryUsageTotals>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryUsageTotals {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// Provider-reported reasoning tokens summed over the session's turns.
    /// Absent when no provider reported the counter; never widened to 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    pub executions: Vec<HistoryUsageExecution>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryUsageExecution {
    pub turn_id: String,
    pub started_at: i64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
}

fn history_usage_tokens(usage: &Value) -> (Option<u64>, Option<u64>, Option<u64>) {
    let number = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| usage.get(key).and_then(Value::as_u64))
    };
    let input = number(&["inputTokens", "input_tokens", "input"]);
    let output = number(&["outputTokens", "output_tokens", "output"]);
    let reasoning = number(&["reasoningTokens", "reasoning_tokens"]);
    let kind = usage
        .get("providerKind")
        .or_else(|| usage.get("provider_kind"))
        .or_else(|| usage.pointer("/cacheStats/providerKind"))
        .and_then(|value| serde_json::from_value::<agent_core::ProviderKind>(value.clone()).ok());
    let input = if kind == Some(agent_core::ProviderKind::Anthropic) {
        input.map(|input| {
            input
                .saturating_add(
                    number(&["cachedInputTokens", "cached_input_tokens", "cacheRead"])
                        .unwrap_or_default(),
                )
                .saturating_add(
                    number(&[
                        "cacheCreationInputTokens",
                        "cache_creation_input_tokens",
                        "cacheCreation",
                    ])
                    .unwrap_or_default(),
                )
        })
    } else {
        input
    };
    (input, output, reasoning)
}

fn canonical_history_usage(value: Value) -> serde_json::Map<String, Value> {
    let mut object = value.as_object().cloned().unwrap_or_default();
    for keys in [
        &["inputTokens", "input_tokens", "input"][..],
        &["outputTokens", "output_tokens", "output"],
        &["cachedInputTokens", "cached_input_tokens", "cacheRead"],
        &[
            "cacheCreationInputTokens",
            "cache_creation_input_tokens",
            "cacheCreation",
        ],
        &["providerKind", "provider_kind"],
        &["contextTokens", "context_tokens"],
        &["contextWindow", "context_window"],
        &["reservedOutputTokens", "reserved_output_tokens"],
        &["availableTokens", "available_tokens"],
        &["inputCost", "input_cost"],
        &["outputCost", "output_cost"],
        &["totalCost", "total_cost"],
        &["reasoningTokens", "reasoning_tokens"],
    ] {
        let reported = keys
            .iter()
            .find_map(|key| object.get(*key).filter(|value| !value.is_null()).cloned());
        for key in keys {
            object.remove(*key);
        }
        if let Some(value) = reported {
            object.insert(keys[0].to_string(), value);
        }
    }
    object.retain(|_, value| !value.is_null());
    object
}

/// A run is the durable execution record for one Agent turn. Conversation
/// messages remain the transcript; runs own lifecycle and recovery state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunSummary {
    pub session_id: String,
    pub turn_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_message: Option<String>,
    pub started_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    pub resume_count: i64,
}

/// Durable source of truth for one queued Agent request. `request` retains the
/// target selected when the message was enqueued; reconnecting SSH or changing
/// the UI's default target must not rewrite it later.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentQueueItem {
    pub id: String,
    pub session_id: String,
    pub idempotency_key: String,
    pub request: AgentChatRequest,
    pub status: String,
    pub sequence: i64,
    pub attempt_count: i64,
    pub enqueued_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct AgentQueueLease {
    pub item: AgentQueueItem,
    pub lease_id: String,
    pub lease_owner: String,
    pub lease_expires_at: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct AgentQueueEnqueueResult {
    pub item: AgentQueueItem,
    pub inserted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationAgentInput {
    pub title: String,
    pub instruction: String,
    #[serde(default)]
    pub workspace_path: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationJobInput {
    pub thread_id: String,
    #[serde(default)]
    pub parent_turn_id: Option<String>,
    pub goal: String,
    /// G2 预算归属：该 job 关联的 goal id（单委托自动带上）。
    #[serde(default)]
    pub goal_id: Option<String>,
    #[serde(default)]
    pub workspace_path: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub execution_target: Option<Value>,
    pub agent_config: CollaborationAgentConfig,
    pub agents: Vec<CollaborationAgentInput>,
}

/// The model and planning settings needed to restart a worker after the app
/// exits. Inputs that could inherit another session's content or authority are
/// deliberately absent from this durable configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationAgentConfig {
    #[serde(default)]
    pub model_id: String,
    #[serde(default)]
    pub analysis_mode: bool,
    #[serde(default)]
    pub tool_policy: AgentToolPolicy,
    #[serde(default)]
    pub skill_ids: Vec<String>,
    #[serde(default)]
    pub requested_tool: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub task_plan_mode: Option<String>,
    #[serde(default)]
    pub task_plan_min_steps: Option<u8>,
    /// G2-D：该子智能体所处的委派层（0 = 根 turn 直派）。子智能体运行时据此
    /// 计算自己还能否继续委派。
    #[serde(default)]
    pub depth: u8,
}

/// A collaboration job owns scheduling and aggregate state. Its workers have
/// distinct Agent sessions and never borrow the parent thread's permissions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationJobSummary {
    pub id: String,
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_turn_id: Option<String>,
    pub goal: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<String>,
    pub permission_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_target: Option<Value>,
    pub agent_config: CollaborationAgentConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregate_summary: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationAgentSummary {
    pub id: String,
    pub job_id: String,
    pub title: String,
    pub status: String,
    pub session_id: String,
    pub turn_id: String,
    pub workspace_path: Option<String>,
    pub permission_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationJobView {
    pub job: CollaborationJobSummary,
    pub agents: Vec<CollaborationAgentSummary>,
}

/// Stable read model for a parent Agent that needs to wait for all workers and
/// then merge their results. `updated_at` is a polling hint; callers stop once
/// `terminal` becomes true rather than assuming timestamps are unique.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationFanInState {
    pub job_id: String,
    pub status: String,
    pub terminal: bool,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregate_summary: Option<String>,
    pub pending_count: usize,
    pub running_count: usize,
    pub terminal_count: usize,
    pub agents: Vec<CollaborationAgentSummary>,
}

impl From<CollaborationJobView> for CollaborationFanInState {
    fn from(view: CollaborationJobView) -> Self {
        let pending_count = view
            .agents
            .iter()
            .filter(|agent| matches!(agent.status.as_str(), "queued" | "interrupted"))
            .count();
        let running_count = view
            .agents
            .iter()
            .filter(|agent| agent.status == "running")
            .count();
        let terminal_count = view.agents.len() - pending_count - running_count;
        Self {
            job_id: view.job.id,
            status: view.job.status.clone(),
            terminal: pending_count == 0 && running_count == 0,
            updated_at: view.job.updated_at,
            finished_at: view.job.finished_at,
            aggregate_summary: view.job.aggregate_summary,
            pending_count,
            running_count,
            terminal_count,
            agents: view.agents,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CollaborationDispatch {
    pub agent: CollaborationAgentSummary,
    pub job_goal: String,
    pub execution_target: Option<Value>,
    pub agent_config: CollaborationAgentConfig,
    pub instruction: String,
    pub attempt: i64,
    pub lease_id: String,
    pub lease_owner: String,
    pub lease_expires_at: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct PersistedAgentSteer {
    pub id: String,
    pub session_id: String,
    pub turn_id: String,
    pub request: AgentSteerRequest,
    pub status: String,
    pub source_queue_id: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct CollaborationCancellationTarget {
    pub job_id: String,
    pub agent_id: String,
    pub session_id: String,
    pub turn_id: String,
}

/// A conversation thread has its own durable lineage. A session's transcript
/// is still stored in `sessions`; this record describes how that transcript
/// was created or forked without making the chat view the source of truth.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationThread {
    pub thread_id: String,
    pub root_thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forked_from_checkpoint_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A checkpoint captures a transcript state. Restoring it never changes the
/// workspace; file reversal remains a separate, conflict-checked operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationCheckpointSummary {
    pub id: String,
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
struct StoredConversationCheckpoint {
    summary: ConversationCheckpointSummary,
    messages: Value,
    metadata: Value,
    artifacts: Value,
    report: Option<Value>,
    usage: Option<Value>,
    events: Vec<Value>,
    checkpoint_cut: Option<CheckpointCut>,
    model_transcript: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CheckpointCut {
    pub storage_id: String,
    pub event_count: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionBinding {
    pub storage_id: String,
    pub seed_from: Option<CheckpointCut>,
    pub deleted: bool,
}

#[derive(Debug, Clone)]
struct StoredAgentRun {
    summary: AgentRunSummary,
    request: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HistorySessionSummary {
    pub id: String,
    pub title: String,
    pub updated_at: i64,
    pub archived: bool,
    pub jira_context: Option<Value>,
    pub metadata: Value,
    pub message_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistorySessionInput {
    pub id: String,
    pub title: String,
    pub updated_at: i64,
    pub archived: bool,
    #[serde(default = "current_history_schema_version")]
    pub schema_version: i64,
    pub jira_context: Option<Value>,
    pub artifacts: Value,
    pub messages: Value,
    pub metadata: Value,
    pub report: Option<Value>,
    pub usage: Option<Value>,
    #[serde(default)]
    pub events: Option<Value>,
}

impl From<HistorySessionInput> for HistorySession {
    fn from(value: HistorySessionInput) -> Self {
        HistorySessionRecord::from(value).into()
    }
}

impl From<HistorySessionInput> for HistorySessionRecord {
    fn from(value: HistorySessionInput) -> Self {
        Self {
            id: value.id,
            title: value.title,
            updated_at: value.updated_at,
            archived: value.archived,
            schema_version: normalize_schema_version(value.schema_version),
            jira_context: value.jira_context,
            artifacts: value.artifacts,
            messages: value.messages,
            metadata: value.metadata,
            report: value.report,
            usage: value.usage,
            events: value.events,
            agent_runs: Vec::new(),
            usage_totals: None,
        }
    }
}

impl From<&HistorySession> for HistorySessionRecord {
    fn from(value: &HistorySession) -> Self {
        Self {
            id: value.id.clone(),
            title: value.title.clone(),
            updated_at: value.updated_at,
            archived: value.archived,
            schema_version: HISTORY_SCHEMA_VERSION,
            jira_context: value.jira_context.clone(),
            artifacts: value.artifacts.clone(),
            messages: value.messages.clone(),
            metadata: value.metadata.clone(),
            report: value.report.clone(),
            usage: value.usage.clone(),
            events: None,
            agent_runs: Vec::new(),
            usage_totals: None,
        }
    }
}

impl From<HistorySessionRecord> for HistorySession {
    fn from(value: HistorySessionRecord) -> Self {
        Self {
            id: value.id,
            title: value.title,
            updated_at: value.updated_at,
            archived: value.archived,
            jira_context: value.jira_context,
            artifacts: value.artifacts,
            messages: value.messages,
            metadata: value.metadata,
            report: value.report,
            usage: value.usage,
        }
    }
}

fn normalize_schema_version(version: i64) -> i64 {
    if version > 0 {
        version
    } else {
        HISTORY_SCHEMA_VERSION
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn next_history_id(prefix: &str) -> String {
    let sequence = HISTORY_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{}-{sequence}", now_ms(), std::process::id())
}

pub struct HistoryStore {
    connection: Connection,
}

#[allow(dead_code)]
fn _session_exists(store: &HistoryStore, id: &str) -> Result<bool, String> {
    store
        .connection
        .query_row(
            "SELECT 1 FROM sessions WHERE id = ?1",
            params![id],
            |_row| Ok(()),
        )
        .optional()
        .map(|value| value.is_some())
        .map_err(|error| error.to_string())
}

mod checkpoints;
mod collaboration_store;
mod event_store;
mod goal_store;
mod persistence;
mod queue_store;
mod run_store;
mod schema;
mod session_store;
mod display_recovery;
mod startup_recovery;
mod turn_repair;
pub(crate) use goal_store::GoalRecord;

/// G1：goal 工具的持久层门面（模式同 queue_commands）。
pub(crate) fn agent_active_goal(app: &AppHandle, session_id: &str) -> Result<Option<GoalRecord>, String> {
    open_store(app)?.active_goal(session_id)
}

pub(crate) fn agent_create_goal(app: &AppHandle, goal: &GoalRecord) -> Result<GoalRecord, String> {
    open_store(app)?.create_goal(goal)
}

pub(crate) fn agent_goal_by_id(app: &AppHandle, goal_id: &str) -> Result<Option<GoalRecord>, String> {
    open_store(app)?.goal_by_id(goal_id)
}

pub(crate) fn agent_update_goal_status(
    app: &AppHandle,
    goal_id: &str,
    status: &str,
    blocked_reason: Option<&str>,
    round_delta: i64,
    tokens_used_delta: i64,
    time_used_delta_seconds: i64,
) -> Result<bool, String> {
    open_store(app)?.update_goal_status(
        goal_id,
        status,
        blocked_reason,
        round_delta,
        tokens_used_delta,
        time_used_delta_seconds,
    )
}

pub(crate) fn agent_record_goal_usage(
    app: &AppHandle,
    goal_id: &str,
    tokens_delta: i64,
    seconds_delta: i64,
) -> Result<(), String> {
    open_store(app)?.record_goal_usage(goal_id, tokens_delta, seconds_delta)
}

/// G2 预算归属：协作 job 关联的 goal id（单委托时由工具自动写入）。
pub(crate) fn collaboration_job_goal_id(
    app: &AppHandle,
    job_id: &str,
) -> Result<Option<String>, String> {
    open_store(app)?.collaboration_job_goal_id(job_id)
}

pub(crate) fn now_ms_public() -> i64 {
    now_ms()
}
use persistence::{
    collect_session_events, decode, decode_optional, encode, encode_optional, history_event_key,
    json_array_len, open_store, row_to_agent_queue_item, row_to_session_record,
    row_to_session_summary,
};
mod turn_recovery;
use turn_recovery::initialize_turn_recovery;
// Retain the legacy recovery API at its original crate-visible path.
#[allow(unused_imports)]
pub(crate) use turn_recovery::*;
#[cfg(test)]
use turn_recovery::{
    ack_turn_outbox_record, completed_terminal, turn_recovery_requests,
    filter_pending_legacy_steer_context, legacy_steer_records, put_turn_outbox_record,
    retire_legacy_steer_record, revise_turn_outbox_record, settle_turn_recovery_record,
};
pub(crate) use persistence::schedule_storage_maintenance;
#[allow(unused_imports)]
pub(crate) use persistence::existing_session_binding;
mod queue_commands;
use queue_commands::AgentContextCheckpointCandidate;
pub(crate) use queue_commands::*;
mod context;
mod context_usage;
mod file_change_summaries;
mod model_transcript;
pub(crate) use model_transcript::{restore_model_transcript, save_model_transcript};
use context::raw_agent_context_from_record;
pub(crate) use context::*;
#[cfg(test)]
use context::{agent_context_from_record, raw_agent_context_turn};
mod collaboration_commands;
pub(crate) use collaboration_commands::*;
pub use collaboration_commands::*;

mod commands;
pub use commands::*;
pub(crate) use commands::append_history_terminal_events_with_retry;

#[cfg(test)]
mod tests;

// Tauri command wrappers remain available at the established registration path.
pub(crate) use collaboration_commands::{
    __cmd__create_collaboration_job, __cmd__get_collaboration_job, __cmd__list_agent_runs,
    __cmd__list_collaboration_jobs, __cmd__recover_collaboration_agents,
    __cmd__recover_interrupted_agent_runs, __tauri_command_name_create_collaboration_job,
    __tauri_command_name_get_collaboration_job, __tauri_command_name_list_agent_runs,
    __tauri_command_name_list_collaboration_jobs,
    __tauri_command_name_recover_collaboration_agents,
    __tauri_command_name_recover_interrupted_agent_runs,
};
pub(crate) use commands::{
    __cmd__append_history_session_events, __cmd__archive_history_session,
    __cmd__create_conversation_checkpoint, __cmd__delete_history_session,
    __cmd__export_history_sessions, __cmd__fork_conversation_thread,
    __cmd__get_conversation_thread, __cmd__get_history_session, __cmd__get_history_turn_events,
    __cmd__list_conversation_checkpoints, __cmd__list_history_session_summaries,
    __cmd__list_history_sessions, __cmd__revert_conversation_to_checkpoint,
    __cmd__rewind_conversation_before_message, __cmd__save_history_session,
    __cmd__search_history_sessions, __cmd__write_history_export,
    __tauri_command_name_append_history_session_events,
    __tauri_command_name_archive_history_session,
    __tauri_command_name_create_conversation_checkpoint,
    __tauri_command_name_delete_history_session, __tauri_command_name_export_history_sessions,
    __tauri_command_name_fork_conversation_thread, __tauri_command_name_get_conversation_thread,
    __tauri_command_name_get_history_session, __tauri_command_name_get_history_turn_events,
    __tauri_command_name_list_conversation_checkpoints,
    __tauri_command_name_list_history_session_summaries,
    __tauri_command_name_list_history_sessions,
    __tauri_command_name_revert_conversation_to_checkpoint,
    __tauri_command_name_rewind_conversation_before_message,
    __tauri_command_name_save_history_session, __tauri_command_name_search_history_sessions,
    __tauri_command_name_write_history_export,
};
