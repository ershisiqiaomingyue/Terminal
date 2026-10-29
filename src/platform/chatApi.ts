import { invoke } from "@/platform/runtime";
import { shareInFlight } from "./sharedRequest";
import { browserAttachmentInput } from "./browserAttachmentInput";

export type SandboxMode = "ask" | "risk" | "full";

export interface HistorySummary {
  id: string;
  title: string;
  updatedAt: number;
  archived: boolean;
  metadata: Record<string, unknown>;
  messageCount: number;
}

export interface HistoryRecord extends Omit<HistorySummary, "messageCount"> {
  schemaVersion: number;
  artifacts: unknown;
  messages: unknown;
  usage?: unknown;
  events?: unknown;
  agentRuns?: AgentRunSummary[];
  usageTotals?: SessionUsageTotals;
}

export interface SessionUsageTotals {
  inputTokens?: number | null;
  outputTokens?: number | null;
  /** Accumulated provider-reported reasoning tokens; absent when no turn reported any. */
  reasoningTokens?: number | null;
  executions: Array<{ turnId: string; startedAt: number; inputTokens?: number | null; outputTokens?: number | null; reasoningTokens?: number | null }>;
}

export interface AgentRunSummary {
  sessionId: string;
  turnId: string;
  status: "preparing" | "interrupted" | "completed" | "cancelled" | "failed" | string;
  statusMessage?: string | null;
  startedAt: number;
  updatedAt: number;
  finishedAt?: number | null;
  resumeCount: number;
}

export interface CollaborationAgentConfig {
  modelId: string;
  analysisMode?: boolean;
  toolPolicy?: "auto" | "disabled";
  skillIds?: string[];
  requestedTool?: string | null;
  reasoningEffort?: string | null;
  taskPlanMode?: string | null;
  taskPlanMinSteps?: number | null;
}

export interface CollaborationAgentInput {
  title: string;
  instruction: string;
  workspacePath?: string | null;
  permissionMode?: SandboxMode | null;
}

export interface CollaborationJobInput {
  threadId: string;
  parentTurnId?: string | null;
  goal: string;
  workspacePath?: string | null;
  permissionMode?: SandboxMode | null;
  executionTarget?: Record<string, unknown> | null;
  agentConfig: CollaborationAgentConfig;
  agents: CollaborationAgentInput[];
}

export interface CollaborationJobSummary {
  id: string;
  threadId: string;
  parentTurnId?: string | null;
  goal: string;
  workspacePath?: string | null;
  permissionMode: SandboxMode | string;
  executionTarget?: Record<string, unknown> | null;
  agentConfig: CollaborationAgentConfig;
  aggregateSummary?: string | null;
  status: "queued" | "running" | "interrupted" | "completed" | "completed_with_errors" | "cancelled" | string;
  createdAt: number;
  updatedAt: number;
  finishedAt?: number | null;
}

export interface CollaborationAgentSummary {
  id: string;
  jobId: string;
  title: string;
  status: "queued" | "running" | "interrupted" | "completed" | "failed" | "cancelled" | string;
  sessionId: string;
  turnId: string;
  workspacePath?: string | null;
  permissionMode: SandboxMode | string;
  summary?: string | null;
  error?: string | null;
  createdAt: number;
  updatedAt: number;
  finishedAt?: number | null;
}

export interface CollaborationJobView {
  job: CollaborationJobSummary;
  agents: CollaborationAgentSummary[];
}

export interface CollaborationAgentRunResult {
  job: CollaborationJobView;
  agent: CollaborationAgentSummary;
  response: {
    sessionId: string;
    turnId: string;
    text: string;
    terminationReason?: string;
  };
}

export interface ConversationThread {
  threadId: string;
  rootThreadId: string;
  parentThreadId?: string | null;
  forkedFromCheckpointId?: string | null;
  createdAt: number;
  updatedAt: number;
}

export interface ConversationCheckpointSummary {
  id: string;
  threadId: string;
  turnId?: string | null;
  label?: string | null;
  createdAt: number;
}

export interface HistoryInput {
  id: string;
  title: string;
  updatedAt: number;
  archived: boolean;
  schemaVersion: number;
  artifacts: unknown;
  messages: unknown;
  metadata: Record<string, unknown>;
  usage?: unknown;
  events?: unknown;
}

export interface AgentImageAttachment {
  name: string;
  mediaType: string;
  dataUrl?: string;
  path?: string | null;
}

export interface AgentArtifact {
  path?: string;
  name?: string;
  kind?: string;
  size?: number | null;
  modifiedAt?: string | null;
  status?: string;
  contentTruncated?: boolean;
  [key: string]: unknown;
}

export interface QueuedThreadArtifact extends Pick<AgentArtifact, "path" | "name" | "kind" | "size"> {
  path: string;
  name: string;
  kind: string;
}

export interface AgentQuota {
  status: "available" | "unsupported" | "notConfigured" | "error" | string;
  source?: string | null;
  remainingAmount?: number | null;
  remainingTokens?: number | null;
  totalAmount?: number | null;
  usedAmount?: number | null;
  unit?: string | null;
  currency?: string | null;
  planName?: string | null;
  message?: string | null;
  checkedAt?: string | null;
}

export interface WorkspaceEnvironment {
  path: string;
  workspaceName: string;
  git: { available: boolean; repository: boolean; branch?: string | null; dirty: boolean; changedFiles: number; addedLines: number; removedLines: number; githubCliAvailable: boolean; remote?: string | null };
}

export interface WorkspaceGitChange {
  path: string;
  status: "created" | "modified" | "deleted" | "renamed" | string;
  addedLines: number;
  removedLines: number;
}

export interface AgentEventPayload {
  type?: string;
  sessionId?: string;
  session_id?: string;
  turnId?: string;
  turn_id?: string;
  sequence?: number;
  timestampMs?: number;
  text?: string;
  message?: string;
  status?: string;
  toolName?: string;
  tool_name?: string;
  callId?: string;
  call_id?: string;
  requestId?: string;
  request_id?: string;
  inputTokens?: number;
  input_tokens?: number;
  outputTokens?: number;
  output_tokens?: number;
  cacheReadTokens?: number;
  cached_input_tokens?: number;
  cacheCreationTokens?: number;
  cache_creation_input_tokens?: number;
  evidencePathKey?: string;
  evidence_path_key?: string;
  [key: string]: unknown;
}

export type AgentThreadItem =
  | { kind: "user_message"; id: string; content: string; attachments?: AgentImageAttachment[]; artifacts?: AgentArtifact[] }
  | { kind: "agent_message"; id: string; content: string; pending: boolean }
  | {
      kind: "tool_call";
      id: string;
      name: string;
      input: unknown;
      input_buffer?: string;
      output?: unknown;
      live_output?: AgentEventPayload;
      status?: "pending" | "running" | "done" | "error" | "cancelled" | "skipped";
      tool_group_id?: string;
      queued_at_ms?: number;
      started_at_ms?: number;
      finished_at_ms?: number;
      is_error: boolean;
    }
  | { kind: "task_plan"; id: string; goal: string; steps: unknown[] }
  | { kind: "file_changes"; id: string; data: AgentEventPayload }
  | { kind: "context"; id: string; data: AgentEventPayload }
  | { kind: "final_report"; id: string; data: AgentEventPayload }
  | { kind: "timeline_event"; id: string; data: AgentEventPayload };

export interface AgentThreadSnapshot {
  id: string;
  revision: number;
  status: "idle" | "running" | "waiting_for_permission" | "completed" | "failed" | "cancelled";
  activeTurn?: { id: string; status: "running" | "completed" | "failed" | "cancelled"; error?: string | null; startedAtMs?: number; finishedAtMs?: number | null } | null;
  pendingPermission?: { requestId: string; toolName: string; expiresAtMs?: number | null } | null;
  runtime?: {
    activity: string;
    toolCallCount: number;
    executionTarget?: { kind: "local" | "ssh" | "wsl"; connectionId?: string | null; profileId?: string | null; root?: string | null } | null;
    usage?: {
      providerKind?: string | null;
      inputTokens?: number;
      outputTokens?: number;
      reasoningTokens?: number;
      generationTokens?: number;
      generationDurationMs?: number;
      generationTimingSource?: string | null;
      cachedInputTokens?: number;
      cacheCreationInputTokens?: number;
      latestRequestUsage?: {
        providerKind?: string | null;
        inputTokens?: number | null;
        cachedInputTokens?: number | null;
        cacheCreationInputTokens?: number | null;
      };
      contextWindow?: number;
      contextTokens?: number;
      reservedOutputTokens?: number;
      availableTokens?: number;
      inputCost?: number | null;
      outputCost?: number | null;
      totalCost?: number | null;
      currency?: string | null;
    };
  };
  items: AgentThreadItem[];
  queue?: Array<{ id: string; content: string; images?: AgentImageAttachment[]; artifacts?: QueuedThreadArtifact[] }>;
}

export interface AgentQueueResponse {
  sessionId: string;
  queueId: string;
  queued: number;
}

export type AgentQueueRequest = Record<string, unknown>;

export const listHistorySummaries = (archived?: boolean) =>
  invoke<HistorySummary[]>("list_history_session_summaries", { archived: archived ?? null });

export const getHistorySession = shareInFlight((id: string) =>
  invoke<HistoryRecord | null>("get_history_session", { id }));

export interface HistoryTurnEventPage {
  events: AgentEventPayload[];
  nextCursor: number | null;
  throughRowId: number;
}

export const getHistoryTurnEvents = (sessionId: string, turnId: string, afterRowId?: number, throughRowId?: number) =>
  invoke<HistoryTurnEventPage>("get_history_turn_events", { sessionId, turnId, afterRowId: afterRowId ?? null, throughRowId: throughRowId ?? null });

export const saveHistorySession = (session: HistoryInput) =>
  invoke<void>("save_history_session", { session });

export const appendHistoryEvents = (id: string, updatedAt: number, events: unknown[]) =>
  invoke<void>("append_history_session_events", { id, updatedAt, events });

const recoverInterruptedAgentRunsRequest = shareInFlight((_key: undefined) =>
  invoke<AgentRunSummary[]>("recover_interrupted_agent_runs"));
export const recoverInterruptedAgentRuns = () => recoverInterruptedAgentRunsRequest(undefined);

export const createCollaborationJob = (input: CollaborationJobInput) =>
  invoke<CollaborationJobView>("create_collaboration_job", { input });

export const getCollaborationJob = (jobId: string) =>
  invoke<CollaborationJobView | null>("get_collaboration_job", { jobId });

export const listCollaborationJobs = (threadId?: string) =>
  invoke<CollaborationJobView[]>("list_collaboration_jobs", { threadId: threadId ?? null });

const recoverCollaborationAgentsRequest = shareInFlight((_key: undefined) =>
  invoke<CollaborationAgentSummary[]>("recover_collaboration_agents"));
export const recoverCollaborationAgents = () => recoverCollaborationAgentsRequest(undefined);

export const runCollaborationAgent = (jobId: string, agentId: string) =>
  invoke<CollaborationAgentRunResult>("run_collaboration_agent", { jobId, agentId });

export const listAgentRuns = (sessionId?: string) =>
  invoke<AgentRunSummary[]>("list_agent_runs", { sessionId: sessionId ?? null });

export const getConversationThread = (threadId: string) =>
  invoke<ConversationThread | null>("get_conversation_thread", { threadId });

export const listConversationCheckpoints = (threadId: string) =>
  invoke<ConversationCheckpointSummary[]>("list_conversation_checkpoints", { threadId });

export const createConversationCheckpoint = (threadId: string, turnId?: string, label?: string) =>
  invoke<ConversationCheckpointSummary>("create_conversation_checkpoint", {
    threadId,
    turnId: turnId ?? null,
    label: label ?? null,
  });

export const revertConversationToCheckpoint = (threadId: string, checkpointId: string) =>
  invoke<HistoryRecord>("revert_conversation_to_checkpoint", { threadId, checkpointId });

export const forkConversationThread = (sourceThreadId: string, checkpointId?: string, title?: string) =>
  invoke<HistoryRecord>("fork_conversation_thread", {
    sourceThreadId,
    checkpointId: checkpointId ?? null,
    title: title ?? null,
  });

export const searchHistorySessions = (query: string) =>
  invoke<HistoryRecord[]>("search_history_sessions", { query });

export const archiveHistorySession = (id: string, archived: boolean) =>
  invoke<void>("archive_history_session", { id, archived });

export const deleteHistorySession = (id: string) =>
  invoke<void>("delete_history_session", { id });

export const exportHistorySessions = () => invoke<string>("export_history_sessions");

export const storeBrowserAttachment = async (name: string, dataUrl: string) =>
  invoke<AgentArtifact>("store_browser_attachment", browserAttachmentInput(name, dataUrl));

export const prepareChatAttachment = (path: string, previous?: import("../views/Chat/attachmentPreparation").PreparedChatAttachment) =>
  invoke<import("../views/Chat/attachmentPreparation").PreparedChatAttachment>("prepare_chat_attachment", { path, previous: previous ?? null });

export const readCachedImage = (image: AgentImageAttachment) =>
  invoke<AgentImageAttachment & { dataUrl: string }>("read_cached_image", { image });

export const readLocalImage = (path: string) =>
  invoke<AgentImageAttachment>("read_local_image", { path });

export interface AgentCancellationStatus {
  requested: boolean;
  completed: boolean;
  /** P1-C 兜底：true 表示软取消宽限（1500ms）超时后已硬中止回合任务。 */
  aborted?: boolean;
}

type ConversationCommand =
  | { method: "thread_read"; params: { thread_id: string; after_revision?: number } }
  | { method: "turn_start"; params: { thread_id: string; turn_id: string; request: Record<string, unknown> } }
  | { method: "turn_enqueue"; params: { thread_id: string; request: Record<string, unknown> } }
  | { method: "turn_queue_guide"; params: { thread_id: string; queue_id: string } }
  | { method: "turn_queue_update"; params: { thread_id: string; queue_id: string; content: string } }
  | { method: "turn_queue_remove"; params: { thread_id: string; queue_id: string } }
  | { method: "turn_queue_reorder"; params: { thread_id: string; queue_ids: string[] } }
  | { method: "turn_resume"; params: { thread_id: string; turn_id: string; resume_turn_id?: string; permission_mode?: "ask" | "risk" | "full" } }
  | { method: "turn_steer"; params: { thread_id: string; turn_id: string; request: Record<string, unknown> } }
  | { method: "turn_interrupt"; params: { thread_id: string; turn_id: string; source?: string } }
  | { method: "permission_resolve"; params: { thread_id: string; request_id: string; action: string; instruction?: string | null } };

const executeConversationCommand = <T>(command: ConversationCommand) =>
  invoke<T>("execute_conversation_command", { command });

export const startConversationTurn = <T>(sessionId: string, turnId: string, request: Record<string, unknown>) =>
  executeConversationCommand<T>({ method: "turn_start", params: { thread_id: sessionId, turn_id: turnId, request } });

export const resumeConversationTurn = <T>(sessionId: string, turnId: string, resumeTurnId?: string, permissionMode?: "ask" | "risk" | "full") =>
  executeConversationCommand<T>({ method: "turn_resume", params: { thread_id: sessionId, turn_id: turnId, resume_turn_id: resumeTurnId, permission_mode: permissionMode } });

export const cancelAgentRequest = (sessionId: string, turnId?: string | null, source?: "stop_button" | "delete_session" | "cancel_submission" | "collaboration_stop") =>
  turnId
    ? executeConversationCommand<AgentCancellationStatus>({ method: "turn_interrupt", params: { thread_id: sessionId, turn_id: turnId, source } })
    : invoke<AgentCancellationStatus>("cancel_agent_request", { sessionId, turnId: null, source });

export const getAgentThreadSnapshot = (sessionId: string, afterRevision?: number) =>
  executeConversationCommand<AgentThreadSnapshot | null>({ method: "thread_read", params: { thread_id: sessionId, after_revision: afterRevision } });

export const resolveConversationPermission = (sessionId: string, requestId: string, action: string, instruction?: string | null) =>
  executeConversationCommand<void>({ method: "permission_resolve", params: { thread_id: sessionId, request_id: requestId, action, instruction: instruction ?? null } });

export const enqueueAgentMessage = (request: AgentQueueRequest) =>
  typeof request.sessionId === "string" && request.sessionId
    ? executeConversationCommand<AgentQueueResponse>({ method: "turn_enqueue", params: { thread_id: request.sessionId, request } })
    : invoke<AgentQueueResponse>("enqueue_agent_message", { request });

export const guideQueuedAgentMessage = (sessionId: string, queueId: string) =>
  executeConversationCommand<void>({ method: "turn_queue_guide", params: { thread_id: sessionId, queue_id: queueId } });

export const updateQueuedAgentMessage = (sessionId: string, queueId: string, content: string) =>
  executeConversationCommand<void>({ method: "turn_queue_update", params: { thread_id: sessionId, queue_id: queueId, content } });

export const reorderQueuedAgentMessages = (sessionId: string, queueIds: string[]) =>
  executeConversationCommand<void>({ method: "turn_queue_reorder", params: { thread_id: sessionId, queue_ids: queueIds } });

export const removeQueuedAgentMessage = (sessionId: string, queueId: string) =>
  executeConversationCommand<void>({ method: "turn_queue_remove", params: { thread_id: sessionId, queue_id: queueId } });

export const readLocalAttachmentForCopy = (path: string) =>
  invoke<string>("read_local_attachment_for_copy", { path });

export const rewindConversationBeforeMessage = (threadId: string, messageId: string, turnId?: string) =>
  invoke<HistoryRecord>("rewind_conversation_before_message", { threadId, messageId, turnId: turnId || null });

export interface LocalLogIndexEntry {
  path: string;
  name?: string;
  kind?: string;
  size?: number;
  modifiedAt?: string;
  [key: string]: unknown;
}

export interface LocalLogSearchHit {
  source: string;
  member?: string | null;
  line?: number;
  excerpt?: string;
  [key: string]: unknown;
}

export interface LocalLogRangeResult {
  source?: string;
  member?: string | null;
  startLine?: number;
  lines?: string[];
  text?: string;
  [key: string]: unknown;
}

export const indexLocalLogs = (paths: string[]) =>
  invoke<LocalLogIndexEntry[]>("index_local_logs", { paths });

export const searchLocalLogs = (paths: string[], query: string, maxResults = 32) =>
  invoke<LocalLogSearchHit[]>("search_local_logs", { paths, query, maxResults, maxExcerptBytes: 1024 });

export const readLocalLogRange = (request: Record<string, unknown>) =>
  invoke<LocalLogRangeResult>("read_local_log_range", { request });

export const rebuildLocalLogTimeline = (paths: string[], maxEvents = 64) =>
  invoke<Array<Record<string, unknown>>>("rebuild_local_log_timeline", { paths, maxEvents });

export const snapshotFileChanges = (paths: string[]) =>
  invoke<Array<Record<string, unknown>>>("snapshot_file_changes", { paths });

export const diffFileChanges = (before: Array<Record<string, unknown>>, after: Array<Record<string, unknown>>) =>
  invoke<Record<string, unknown>>("diff_file_changes", { before, after });

/** N10：turn_start/turn_resume 的受理回执。数据本体走事件流。 */
export type AgentTurnStartReceipt = {
  turn_id?: string | null;
  status?: "started" | "queued";
  queue_id?: string;
};

/** P3-A：turn_steer 的受理回执——与投递结果同生共死。 */
export type AgentSteerReceipt = {
  turn_id?: string;
  steer_id?: string;
  status?: "steered" | "pending_replay";
};

export const steerAgentRequest = (args: {
  sessionId: string;
  message: string;
  attachments?: AgentImageAttachment[];
  artifacts?: unknown[];
  turnId?: string | null;
}) => args.turnId
  ? executeConversationCommand<AgentSteerReceipt>({
    method: "turn_steer",
    params: {
      thread_id: args.sessionId,
      turn_id: args.turnId,
      request: { message: args.message, attachments: args.attachments || [], artifacts: args.artifacts || [] },
    },
  })
  : invoke<AgentSteerReceipt>("steer_agent_request", {
    sessionId: args.sessionId,
    message: args.message,
    attachments: args.attachments || [],
    artifacts: args.artifacts || [],
    turnId: null,
  });

const readProviderQuota = shareInFlight((key: string) =>
  invoke<AgentQuota>("get_provider_quota", { request: JSON.parse(key) }));
export const getProviderQuota = (request: Record<string, unknown>) => readProviderQuota(JSON.stringify(request));

export const checkAgentProvider = (request: Record<string, unknown>) =>
  invoke<{ id: string; name: string; status: { status: string; message?: string } }>("check_agent_provider", { request });

export const getSshStatus = (id: string, path?: string) =>
  invoke<Record<string, unknown>>("get_ssh_status", { id, path: path || null });

export const getWslStatus = (id: string, path?: string) =>
  invoke<Record<string, unknown>>("get_wsl_status", { id, path: path || null });

export const getWorkspaceEnvironment = (path: string) =>
  invoke<WorkspaceEnvironment>("get_workspace_environment", { path });

export const getWorkspaceGitChanges = (path: string) =>
  invoke<WorkspaceGitChange[]>("get_workspace_git_changes", { path });

export const getWorkspaceGitFileDiff = (path: string, filePath: string) =>
  invoke<string>("get_workspace_git_file_diff", { path, filePath });
