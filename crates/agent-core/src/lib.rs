mod project_checks;
mod tool_images;
pub use tool_images::{message_owns_image, tool_image_owner};
mod browser_downloads;
mod browser_mutations;
mod contract;
mod conversation;
mod credentials;
pub mod dsh_native_provider;
pub mod dsh_provider;
mod event;
mod evidence;
mod git_browser;
mod intent;
mod models;
pub use browser_downloads::{
    browser_download_server_directory, inspect_browser_download, publish_browser_download,
    validate_browser_download_path, BrowserDownloadStatus,
};
pub use browser_mutations::{
    execute_local_browser_mutation, preview_local_browser_mutation, BrowserMutationPreview,
    BrowserMutationRequest,
};
pub use git_browser::{
    local_git_browser_details, local_git_browser_directory, GitBrowserDetails, GitBrowserDirectory,
};
mod local_tools;
mod log_engine;
mod mcp;
mod network;
mod output_channels;
mod owned_process;
mod path_policy;
mod permission;
mod plugin_tools;
pub mod transfer_archive;
pub use plugin_tools::{plugin_tool_name, plugin_tool_output};
mod agent_runtime;
pub mod background;
mod process;
mod provider;
mod provider_client;
mod provider_contract;
mod provider_runtime;
mod remote;
mod report;
mod round;
mod runtime;
#[cfg(windows)]
pub mod sandbox_windows;
mod skill;
pub mod tool_ledger {
    pub use crate::tooling::ledger::*;
}
pub mod turn_recovery {
    pub use crate::agent_runtime::response_recovery::*;
}
pub use tooling::file_changes;
mod stream;
mod timeline;
mod tool_output;
mod tooling;
pub use tool_output::prune_tool_result_for_model;
mod usage;
mod workspace;
#[cfg(windows)]
mod wsl_process;

pub use agent_runtime::{
    cancel_request_token_if_matching, is_cancelled, register_pending_permission,
    request_cancellation, wait_for_agent_cancellation, wait_for_permission, AgentRequestControl,
    AgentRequestGuard, AgentRuntimeState, PendingPermission, PermissionAction,
    PermissionResponse, PermissionWaitResult, QueuedAgentClaim, UnitKind,
};
pub use contract::{
    AgentCancellationStatus, AgentChatRequest, AgentContextCheckpoint, AgentContextCheckpointEntry,
    AgentContextCheckpointPayload, AgentContextTurn, AgentExecutionTarget, AgentImageAttachment,
    AgentSteerRequest, AgentToolPolicy, AnalysisReport, JiraIssueContext, LocalArtifact,
};
pub use conversation::{
    replay_runtime_turn, ConversationCommand, PermissionSnapshot, QueuedThreadArtifact,
    QueuedThreadImage, QueuedThreadItem, ThreadEvent, ThreadItem, ThreadRuntimeSnapshot,
    ThreadSnapshot, ThreadStatus, ThreadUpdate, ThreadUsageSnapshot, TurnSnapshot, TurnStatus,
};
pub use credentials::{
    estimate_image_attachment_tokens, image_attachments_for_messages, normalize_image_attachments,
    steer_message_content, update_image_token_reserve, MAX_AGENT_IMAGE_DATA_URL_CHARS,
};
pub use credentials::{execute_usage_script, UsageScriptResult};
pub use event::{AgentEvent, AgentMessage, AgentToolCall, FileChangeEntry, MessageRole};
pub use evidence::{
    AgentChatResponse, AgentLogEvidence, LogIndexEntry, LogRangeRequest, LogRangeResult,
    LogSearchHit, LogTimelineEvent,
};
pub use intent::{effective_tool_policy, should_enable_agent_tools};
pub use local_tools::{register_local_tools, LocalToolContext, PATH_ACCESS_PROBE_TOOL};
pub use log_engine::{
    index_log_paths, read_log_range, read_log_range_with_cancellation, rebuild_timeline,
    rebuild_timeline_with_cancellation, search_log_paths, search_log_paths_with_cancellation,
};
pub use mcp::{
    mcp_call_request, mcp_call_tool, mcp_clear_session, mcp_initialize_request,
    mcp_initialized_notification, mcp_list_tools, mcp_tools_list_request, parse_mcp_result,
    McpCallResult, McpConfigError, McpHealth, McpRegistry, McpRegistryError, McpServerConfig,
    McpTool, McpTransport,
};
pub use models::{
    append_historical_tool_results, bundled_model_capabilities, fetch_model_capabilities,
    find_model_capability, known_context_window, known_reasoning_support,
    model_capability_http_client, model_supports_reasoning, parse_model_capability_document,
    prompt_cache_hash, read_model_capabilities, selectable_reasoning_efforts,
    transcript_endpoint_identity, write_model_capabilities, InputUsageSemantics,
    ModelCapabilityDocument, ModelCapabilityRecord, ProviderCapabilities,
};
pub use models::{
    available_tokens, build_agent_messages, build_agent_messages_detailed, collect_context_summary,
    collect_context_summary_with_progress, compact_agent_messages, compact_messages,
    compact_messages_with_current, compact_messages_with_current_with_overhead,
    compact_messages_with_overhead, compact_messages_with_trigger, compaction_trigger_tokens,
    compaction_trigger_tokens_scaled, compaction_trigger_tokens_scaled_with_overhead,
    compaction_trigger_tokens_with_overhead, context_budget, context_message_id_at_boundary,
    context_turn_message, effective_context_window, enrich_compaction_summary,
    estimate_message_tokens, estimate_messages_tokens, estimate_request_tokens,
    estimate_text_tokens, historical_local_sources_prompt, legacy_context_checkpoint_message,
    local_source_metadata_text, model_summary_input, output_limit_reached, output_reserve,
    plan_compaction_checkpoint, resolve_context_window, restore_context_checkpoint,
    unknown_context_retry_budget, usage_percent, user_local_source_metadata, CompactionHysteresis,
    CompactionResult, CompactionStats, CompactionStatus, ContextRetryBudget, ContextSummaryPrefix,
    ContextSummaryPrefixCache, ContextWindowResolution, ContextWindowStatus, ModelTranscript,
    PromptCacheDiagnostic, PromptCacheFingerprint, TokenEstimateCalibrator,
    CONTEXT_CHECKPOINT_FORMAT_VERSION, CONTEXT_SUMMARY_INPUT_LIMIT, CONTEXT_SUMMARY_PREFIX,
    DEFAULT_CONTEXT_WINDOW, FINAL_OUTPUT_TOKENS, MIN_FINAL_OUTPUT_TOKENS, MIN_TOOL_OUTPUT_TOKENS,
    OUTPUT_LIMIT_THRESHOLD_PERCENT, TOOL_OUTPUT_TOKENS, UNKNOWN_CONTEXT_RETRY_WINDOWS,
};
pub use network::{
    apply_child_environment, apply_child_environment_std, child_environment_path_entries,
    child_proxy_environment, http_client_builder, set_child_environment, set_http_proxy_settings,
    HttpProxySettings,
};
pub use path_policy::{
    is_allowed_agent_path, is_explicitly_selected_root, is_selected_path, normalized_policy_path,
    normalized_policy_path_from, policy_path_key, resolve_command_working_directory,
    resolve_environment_path, resolve_selected_path, resolve_user_directory_alias,
    selected_working_directory, strip_windows_extended_prefix,
};
pub use permission::{
    parse_tool_call_input, AllowPermission, DenyPermission, PermissionDecision,
    PermissionEvaluator, PermissionMode,
};
pub use process::{
    command_result_value, execute_process, execute_process_with_options, truncate_command_output,
    truncate_output_bytes, CommandExecutionResult, CommandOutputListener, CommandOutputSink,
    ProcessSandbox, MAX_COMMAND_OUTPUT_CHARS, MAX_COMMAND_TIMEOUT_MS,
};
pub use provider::{
    ConnectionStatus, ModelCompatibilityError, ModelProfile, ProviderConfig, ProviderConfigError,
    ProviderKind,
};
pub use provider_client::{
    endpoint, messages_endpoint, models_endpoint, parse_sse_data, request_body, HttpProviderClient,
    ProviderClientError, ProviderDiagnosticObserver, ProviderImageAttachment, ProviderRequest,
    ProviderStatusError, ProviderTransportDiagnostic, PROVIDER_IDLE_TIMEOUT, PROVIDER_USER_AGENT,
};
pub use provider_contract::{
    AgentConnectionView, AgentModelDiscoveryRequest, AgentModelOption, AgentModelSettings,
    AgentModelSummary, AgentProviderCheckRequest, AgentProviderSettings, AgentProviderSummary,
    AgentQuotaView, ProviderQuotaUrls,
};
pub use provider_runtime::{
    apply_cached_model_capabilities, clamp_declared_output_tokens, expand_agent_models,
    expand_agent_providers, infer_provider_brand, max_agent_output_tokens,
    max_agent_output_tokens_continued_output, migrate_legacy_provider_protocols, model_protocol,
    normalized_model_settings, normalized_protocol, protocol_from_registry_name, protocol_label,
    provider_protocol, resolve_agent_model, resolved_model_context_window,
    sanitized_model_settings,
};
pub use remote::{
    disconnect_ssh_connection, download_ssh_browser_path, download_ssh_browser_path_with_access,
    download_ssh_workspace_file, ensure_ssh_connected, execute_ssh_browser_mutation,
    inspect_ssh_browser_upload, list_ssh_browser_directory, list_ssh_directories,
    parse_wsl_resource_status, prepare_ssh_transport, preview_ssh_browser_mutation, query_ssh_cwd,
    query_ssh_host_key, query_ssh_status, read_ssh_browser_file, register_ssh_tools,
    remember_russh_transport_failure, reset_russh_transport_failure, resolve_ssh_root,
    run_ssh_workspace_command, run_ssh_workspace_command_streaming, russh_active_engine_label,
    russh_open_pty, russh_pool_alive, russh_pty_preferred, russh_run_command,
    russh_runtime_ensure_started, russh_transport_failing, ssh_browser_file_metadata,
    ssh_browser_file_version, ssh_browser_transfer_info, ssh_browser_transfer_info_with_access,
    ssh_git_browser_details, ssh_git_browser_directory, ssh_workspace_git, test_ssh_connection,
    transfer_connection, upload_ssh_browser_paths, write_ssh_browser_file, BrowserFileMetadata,
    BrowserTransferInfo, RemoteDiskMount, RemoteProcessStatus, RusshCallError, RusshErrorKind,
    RusshPtyCallbacks, RusshPtySession, SshAskPass, SshBrowserDirectory, SshCommandResult,
    SshConnectionConfig, SshDirectoryEntry, SshDownloadAccess, SshHostKeyInfo, SshRemoteStatus,
    SshToolContext, SshWorkspaceGit, TransferProgress, UploadConflict, UploadPolicy,
    WorkspaceGitChange, WorkspaceGitSummary, LINUX_RESOURCE_PROBE,
};
pub use report::{compose_agent_prompt, parse_analysis_report};
pub use round::{
    collect_model_round, collect_model_round_with_observer, ModelRoundError,
    ModelRoundInterruption, ModelRoundResult,
};
pub use runtime::{
    assistant_tool_message, execute_tool_call, execute_tool_call_with_start, ToolCallResult,
};
pub use skill::{scan_skill_repository, SkillPackage, SkillRuntime};
pub use stream::{
    canonicalize_tool_stream_event, finish_text_delta, finish_tool_markup_delta,
    normalize_stream_event, tool_call_signature, AgentRoundState, PendingToolCall, ToolStreamState,
};
pub use timeline::{fallback_commentary_for_tool, take_tool_group_fallback, TimelineSegment};
pub use tooling::{
    canonical_tool_name, collect_declared_path_values, command_contains_path, command_path_tokens,
    controlled_tool_for_command, enrich_permission_input, enrich_permission_input_for_target,
    external_path_scope_key, external_path_scope_key_for_environment,
    external_path_scope_key_for_target, is_allowed_command, is_allowed_windows_shell,
    is_dangerous_command, is_dangerous_shell, is_forbidden_destructive_command,
    is_forbidden_destructive_command_line, is_parallel_safe_tool, is_parallel_safe_tool_for_target,
    is_read_only_command, is_read_only_tool, is_windows_shell_command,
    namespaced_permission_scope_key, normalize_routing_target, path_access_candidates,
    path_access_requires_confirmation, path_access_requires_confirmation_for_target,
    path_access_requires_confirmation_with_read_only, path_access_requires_external_execution,
    path_access_requires_external_execution_for_target, path_access_tool,
    permission_requires_confirmation, permission_scope_key, permission_scope_key_for_environment,
    permission_scope_key_for_target, permission_scope_kind_key, permission_scope_namespace_kind,
    remote_path_access_candidates, scoped_tool_name, shell_process_args, ToolDefinition,
    ToolExecutionError, ToolHandler, ToolRegistry, ToolRegistryError,
    FORBIDDEN_DESTRUCTIVE_COMMAND_MESSAGE, ROUTING_TARGET_ARGUMENT,
};
pub use usage::{merge_usage, AgentCacheStats, AgentUsage, GenerationMetrics, RequestCacheUsage};
pub use workspace::{
    bind_workspace_artifact, is_linux_absolute_path, is_virtual_artifact_path,
    load_global_agent_instructions, load_workspace_agent_context, load_workspace_agent_context_at,
    load_workspace_permissions, local_workspace_path, normalize_linux_path,
    normalize_local_project_target, persist_workspace_scope, request_snapshot_paths,
    workspace_agent_dir, workspace_artifact_from_path, workspace_context_prompt,
    workspace_root_from_artifacts, WorkspaceAgentContext, WorkspacePermissionConfig,
    WslRuntimeConfig, LEGACY_WORKSPACE_AGENT_DIR, WORKSPACE_AGENT_DIR,
};

/// Drop host-only bookkeeping keys before an argument object crosses a tool
/// boundary. Plugin and MCP servers validate the schema they declared strictly
/// and reject unknown keys (the failure read `invalid arguments: unknown key
/// "__agent_call_id"`), which aborted every such call.
pub fn strip_host_tool_bookkeeping(input: &mut serde_json::Value) {
    if let serde_json::Value::Object(object) = input {
        object.remove("__agent_call_id");
    }
}

#[cfg(test)]
mod host_bookkeeping_tests {
    use super::strip_host_tool_bookkeeping;

    #[test]
    fn strips_host_bookkeeping_and_keeps_declared_arguments() {
        let mut input = serde_json::json!({
            "command": "git status",
            "description": "查看工作区",
            "__agent_call_id": "call-7",
        });
        strip_host_tool_bookkeeping(&mut input);
        assert_eq!(
            input,
            serde_json::json!({"command": "git status", "description": "查看工作区"})
        );
    }

    #[test]
    fn leaves_non_object_arguments_untouched() {
        let mut input = serde_json::json!("plain");
        strip_host_tool_bookkeeping(&mut input);
        assert_eq!(input, serde_json::json!("plain"));
    }
}
