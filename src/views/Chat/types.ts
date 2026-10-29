import type { AgentEventPayload, AgentRunSummary, SessionUsageTotals } from "@/platform/chatApi";
import type { PreparedChatAttachment } from "./attachmentPreparation";

export type Role = "user" | "assistant";
export type SandboxMode = "ask" | "risk" | "full";
export type InferenceLevel = "low" | "medium" | "high" | "xhigh" | "max";
export type ChatSourceKind = "directory" | "image" | "code" | "text" | "archive" | "document";

export interface ChatSource {
  path: string;
  name: string;
  kind: ChatSourceKind;
  size?: number;
  dataUrl?: string;
  imagePath?: string;
  prepared?: PreparedChatAttachment;
  preparing?: boolean;
  preparationError?: string;
}

export interface ChatMessage {
  id: string;
  role: Role;
  content: string;
  createdAt: number;
  startedAt?: number;
  finishedAt?: number;
  pending?: boolean;
  error?: boolean;
  attachments?: Array<{ name: string; kind: ChatSourceKind; path?: string; dataUrl?: string; imagePath?: string; mediaType?: string; origin?: "input" }>;
  events?: AgentEventPayload[];
  turnId?: string;
  turnSegmentIndex?: number;
  turnSegmentCount?: number;
  processStart?: number;
  processEnd?: number;
}

export type ExecutionTarget = { kind: "local" | "ssh" | "wsl"; id?: string; label: string; root?: string };

export type ContextCheckpoint = {
  id?: string;
  evidencePolicyVersion?: number;
  beforeTokens?: number;
  afterTokens?: number;
  reservedOutputTokens?: number;
  availableTokens?: number;
  reason?: string;
  summary?: string;
  coveredThroughMessageId?: string;
};

export type ChatUsage = {
  providerKind?: string;
  cacheHitRate?: number;
  latestRequestUsage?: ChatUsage;
  input?: number;
  output?: number;
  /** Provider-reported reasoning tokens; absent when the provider did not report them. */
  reasoning?: number;
  generationTokens?: number;
  generationDurationMs?: number;
  generationTimingSource?: "request";
  cacheRead?: number;
  cacheCreation?: number;
  contextWindow?: number;
  contextTokens?: number;
  reservedOutputTokens?: number;
  availableTokens?: number;
  inputCost?: number;
  outputCost?: number;
  totalCost?: number;
  currency?: string;
};

export type LivePermissionRequest = {
  sessionId: string;
  requestId: string;
  toolName: string;
  input?: Record<string, unknown>;
  question?: { question: string; options: string[] };
  /** N2：等待到期时刻（epoch 毫秒），权限条据此显示倒计时。 */
  expiresAtMs?: number | null;
  unavailable?: boolean;
};

export type ComposerFileEntry = {
  path: string;
  status: "modified" | "created" | "deleted";
  addedLines?: number;
  removedLines?: number;
};

export type ComposerFileChanges = {
  sessionId?: string;
  turnId?: string;
  created: string[];
  modified: string[];
  deleted: string[];
  addedLines: number;
  removedLines: number;
  entries?: ComposerFileEntry[];
};

export type ComposerTaskStep = {
  id: string;
  title: string;
  status: "done" | "running" | "pending" | "error" | "cancelled";
  cancelId?: string;
};

export type TaskPlanMode = "auto" | "always" | "off";
export type TaskPlanSettings = { mode: TaskPlanMode; minSteps: number };

export interface ChatSession {
  id: string;
  title: string;
  updatedAt: number;
  archived: boolean;
  /** 会话创建时间；早期记录没有这一项，展示时用 updatedAt 兜底。 */
  createdAt?: number;
  /** 置顶的会话在侧栏里永远排在最前。 */
  pinned?: boolean;
  scope?: "workspace" | "standalone";
  workspacePath?: string;
  dshStorageId?: string;
  messages: ChatMessage[];
  historyMessageCount?: number;
  contextCheckpoint?: ContextCheckpoint;
  /** Native context telemetry is also stored outside the message transcript. */
  contextUsageEvents?: AgentEventPayload[];
  sources?: ChatSource[];
  automationProjectIds?: string[];
  artifacts?: Array<Record<string, unknown>>;
  usage?: ChatUsage;
  usageTurnId?: string;
  usageTotals?: SessionUsageTotals;
  executionTarget?: ExecutionTarget;
  /** 该对话绑定的远程执行环境（SSH / WSL），首项即本对话的默认执行环境。 */
  boundEnvironments?: ExecutionTarget[];
  timelineExpanded?: Record<string, boolean>;
  dismissedTaskPlanKey?: string;
  agentRuns?: AgentRunSummary[];
  /** Per-session model selection; overrides the global default when present. */
  modelSelection?: { providerId: string; modelId: string };
}
